//! Agent tool handler — spawn subagents.
//!
//! Mirrors kimi-cli's `AgentTool`: allows the root agent to launch built-in
//! subagents (`coder`, `explore`, `plan`) as foreground tasks. The subagent
//! runs in its own `SessionActor` with an isolated `Context`, optional model
//! override, and a filtered tool registry.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::time::timeout;
use uuid::Uuid;

use crate::cli::runtime::Runtime;
use crate::git_context::GitContext;
use crate::hooks::{HookEventType, events as hook_events};
use crate::llm::session::ChatSession;
use crate::llm::types::Message;
use crate::subagents::AgentTypeDefinition;
use crate::tools::{
  ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, ToolPayload, parse_arguments,
};
use crate::wire::{WireBus, WireMessage};

/// Handler for the `Agent` tool.
pub struct AgentHandler {
  /// Holder used to break the Runtime construction cycle: Runtime creates the
  /// registry, then sets itself into the holder so the handler can access it.
  runtime_holder: Arc<OnceLock<Arc<Runtime>>>,
}

/// Arguments for the Agent tool.
#[derive(Debug, Deserialize)]
struct AgentArgs {
  /// Short task description.
  description: String,
  /// Full prompt for the subagent.
  prompt: String,
  /// Subagent type name (default "coder").
  #[serde(default = "default_subagent_type")]
  subagent_type: String,
  /// Optional model alias override.
  #[serde(default)]
  model: Option<String>,
  /// Optional agent id to resume.
  #[serde(default)]
  resume: Option<String>,
  /// Whether to run in background (Phase 2).
  #[serde(default)]
  run_in_background: bool,
  /// Optional timeout in seconds.
  #[serde(default)]
  timeout: Option<u64>,
}

fn default_subagent_type() -> String {
  "coder".to_string()
}

const MAX_FOREGROUND_TIMEOUT: u64 = 60 * 60;

#[async_trait]
impl ToolHandler for AgentHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    true
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    let arguments = match payload {
      ToolPayload::Function { arguments } => arguments,
    };

    let args: AgentArgs = parse_arguments(&arguments)?;

    let runtime = self
      .runtime_holder
      .get()
      .cloned()
      .ok_or_else(|| ToolError::Fatal("Agent handler runtime not initialized".to_string()))?;

    // Only root agents may spawn subagents.
    if !runtime.role.is_root() {
      return Err(ToolError::RespondToModel(
        "Subagents cannot launch other subagents.".to_string(),
      ));
    }

    if args.run_in_background {
      return Err(ToolError::RespondToModel(
        "Background subagent execution is not yet supported.".to_string(),
      ));
    }

    if let Some(t) = args.timeout
      && t > MAX_FOREGROUND_TIMEOUT
    {
      return Err(ToolError::RespondToModel(format!(
        "Timeout must be <= {} seconds",
        MAX_FOREGROUND_TIMEOUT
      )));
    }

    let type_def = runtime
      .labor_market
      .require(&args.subagent_type)
      .map_err(|e| ToolError::RespondToModel(e.to_string()))?
      .clone();

    let effective_model = args
      .model
      .clone()
      .or_else(|| type_def.default_model.clone())
      .or_else(|| runtime.model_override.clone());

    let agent_id = args
      .resume
      .unwrap_or_else(|| format!("a{}", Uuid::new_v4().simple()));

    run_foreground_subagent(
      runtime,
      type_def,
      agent_id,
      args.description,
      args.prompt,
      effective_model,
      args.timeout,
    )
    .await
  }
}

#[async_trait]
impl ToolHandler for Arc<AgentHandler> {
  fn kind(&self) -> ToolKind {
    self.as_ref().kind()
  }

  async fn is_mutating(&self, invocation: &ToolInvocation) -> bool {
    self.as_ref().is_mutating(invocation).await
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    self.as_ref().handle(invocation).await
  }
}

impl AgentHandler {
  /// Create a new Agent handler. The runtime is set later via
  /// `AgentHandler::bind_runtime` to break the construction cycle.
  pub fn new() -> Self {
    Self {
      runtime_holder: Arc::new(OnceLock::new()),
    }
  }

  /// Bind the runtime after `Runtime` construction.
  pub fn bind_runtime(&self, runtime: Arc<Runtime>) -> bool {
    self.runtime_holder.set(runtime).is_ok()
  }
}

impl Default for AgentHandler {
  fn default() -> Self {
    Self::new()
  }
}

/// Run a subagent in the foreground and return its final summary.
async fn run_foreground_subagent(
  parent_runtime: Arc<Runtime>,
  type_def: AgentTypeDefinition,
  agent_id: String,
  _description: String,
  prompt: String,
  effective_model: Option<String>,
  timeout_seconds: Option<u64>,
) -> Result<ToolOutput, ToolError> {
  let subagent_type = type_def.name.clone();

  parent_runtime.hook_engine().trigger_fire_and_forget(
    HookEventType::SubagentStart,
    subagent_type.clone(),
    hook_events::subagent_start(
      &agent_id,
      &parent_runtime.args.work_dir,
      &subagent_type,
      &prompt,
    ),
  );

  // Build child runtime with tool policy and optional model override.
  let child_runtime = Arc::new(parent_runtime.copy_for_subagent(
    agent_id.clone(),
    subagent_type.clone(),
    effective_model.clone(),
    &type_def.tool_policy,
  ));

  // Resolve provider for the child runtime.
  let provider = ChatSession::create_provider(&child_runtime, effective_model.as_deref())
    .map_err(|e| ToolError::Fatal(format!("Failed to create subagent provider: {}", e)))?;

  // Build system prompt for the subagent.
  let mut runtime_args = parent_runtime.args.clone();
  runtime_args.role_additional = type_def.role_additional.clone();
  let system_prompt = {
    let mut s = child_runtime.system_prompt_template.clone();
    s = s.replace("${IRONCODE_NOW}", &runtime_args.now);
    s = s.replace("${IRONCODE_WORK_DIR}", &runtime_args.work_dir);
    s = s.replace("${IRONCODE_WORK_DIR_LS}", &runtime_args.work_dir_ls);
    s = s.replace(
      "${IRONCODE_ADDITIONAL_DIRS_INFO}",
      &runtime_args.additional_dirs_info,
    );
    s = s.replace("${IRONCODE_AGENTS_MD}", &runtime_args.agents_md);
    s = s.replace("${IRONCODE_SKILLS}", &runtime_args.skills);
    s = s.replace("${ROLE_ADDITIONAL}", &runtime_args.role_additional);
    s
  };

  // For explore agents, prepend git context to the prompt.
  let user_prompt = if subagent_type == "explore" {
    let git_ctx = GitContext::new(
      parent_runtime.config.git_context.clone(),
      PathBuf::from(&runtime_args.work_dir),
    )
    .collect()
    .await;
    if git_ctx.is_empty() {
      prompt
    } else {
      format!("{}\n\n{}", git_ctx, prompt)
    }
  } else {
    prompt
  };

  let messages = vec![Message::system(system_prompt), Message::user(user_prompt)];

  // Create a private wire bus for the subagent.
  let child_bus = WireBus::new(1024);
  let child_publisher = child_bus.publisher();
  let mut subscriber = child_bus.subscriber();

  // Spawn the subagent session.
  let handle = ChatSession::start_subagent(
    agent_id.clone(),
    provider,
    messages,
    child_runtime,
    child_publisher,
    Some(type_def.tool_policy.clone()),
  );

  // Send the prompt to start the turn.
  handle.send_message("");

  // Collect the final assistant response from child wire events.
  let result = collect_subagent_response(&mut subscriber, timeout_seconds).await;

  handle.shutdown();

  let output = match result {
    Some(summary) => format!(
      "agent_id: {}\nresumed: false\nactual_subagent_type: {}\nstatus: completed\n\n[summary]\n{}",
      agent_id, subagent_type, summary
    ),
    None => format!(
      "agent_id: {}\nresumed: false\nactual_subagent_type: {}\nstatus: completed\n\n[summary]\nNo response from subagent.",
      agent_id, subagent_type
    ),
  };

  parent_runtime.hook_engine().trigger_fire_and_forget(
    HookEventType::SubagentStop,
    subagent_type.clone(),
    hook_events::subagent_stop(
      &agent_id,
      &parent_runtime.args.work_dir,
      &subagent_type,
      &output,
    ),
  );

  Ok(ToolOutput::success(output))
}

/// Subscribe to child wire events and return the final assistant text.
async fn collect_subagent_response(
  subscriber: &mut tokio::sync::broadcast::Receiver<WireMessage>,
  timeout_seconds: Option<u64>,
) -> Option<String> {
  let deadline = timeout_seconds.map(Duration::from_secs);
  let mut in_turn = false;
  let mut current_response = String::new();

  loop {
    let maybe_msg = match deadline {
      Some(d) => match timeout(d, subscriber.recv()).await {
        Ok(Ok(msg)) => Some(msg),
        Ok(Err(_)) | Err(_) => break,
      },
      None => match subscriber.recv().await {
        Ok(msg) => Some(msg),
        Err(_) => break,
      },
    };

    let msg = match maybe_msg {
      Some(m) => m,
      None => continue,
    };

    match msg {
      WireMessage::TurnBegin => {
        in_turn = true;
        current_response.clear();
      }
      WireMessage::ContentChunk { text } => {
        if in_turn {
          current_response.push_str(&text);
        }
      }
      WireMessage::TurnEnd => {
        if in_turn && !current_response.is_empty() {
          return Some(current_response);
        }
        in_turn = false;
      }
      WireMessage::ToolCallEnd { output, .. } => {
        // Accumulate tool results if no content follows.
        if current_response.is_empty() {
          current_response.push_str(&output);
        }
      }
      WireMessage::Error { message } => {
        return Some(format!("Subagent error: {}", message));
      }
      _ => {}
    }
  }

  if !current_response.is_empty() {
    Some(current_response)
  } else {
    None
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_agent_args_parse() {
    let json = r#"{"description":"test task","prompt":"do this","subagent_type":"coder"}"#;
    let args: AgentArgs = serde_json::from_str(json).unwrap();
    assert_eq!(args.subagent_type, "coder");
    assert_eq!(args.description, "test task");
  }
}
