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
use tokio::sync::Notify;
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

use crate::background::manager::BackgroundTaskManager;
use crate::background::models::TaskStatus;
use crate::cli::runtime::Runtime;
use crate::git_context::GitContext;
use crate::hooks::{HookEventType, events as hook_events};
use crate::llm::provider::LLMProvider;
use crate::llm::session::ChatSession;
use crate::llm::types::Message;
use crate::subagents::store::SubagentStore;
use crate::subagents::{AgentTaskPayload, AgentTypeDefinition, SubagentStatus, ToolPolicy};
use crate::tools::{
  ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, ToolPayload, parse_arguments,
};
use crate::utils::time::now_secs;
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

    let type_def = runtime
      .labor_market
      .require(&args.subagent_type)
      .map_err(|e| ToolError::RespondToModel(e.to_string()))?
      .clone();

    if args.run_in_background && !type_def.supports_background {
      return Err(ToolError::RespondToModel(format!(
        "Subagent type '{}' does not support background execution",
        args.subagent_type
      )));
    }

    if args.run_in_background && args.description.trim().is_empty() {
      return Err(ToolError::RespondToModel(
        "description is required when run_in_background is true".to_string(),
      ));
    }

    if !args.run_in_background
      && let Some(t) = args.timeout
      && t > MAX_FOREGROUND_TIMEOUT
    {
      return Err(ToolError::RespondToModel(format!(
        "Timeout must be <= {} seconds",
        MAX_FOREGROUND_TIMEOUT
      )));
    }

    let effective_model = args
      .model
      .clone()
      .or_else(|| type_def.default_model.clone())
      .or_else(|| runtime.model_override.clone());

    let agent_id = args
      .resume
      .unwrap_or_else(|| format!("a{}", Uuid::new_v4().simple()));

    if args.run_in_background {
      return run_background_subagent(
        runtime,
        type_def,
        agent_id,
        args.description,
        args.prompt,
        effective_model,
        args.timeout,
      )
      .await;
    }

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
    false,
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

/// Run a subagent in the background and return an immediate task summary.
///
/// The subagent executes as an in-process tokio task that shares the parent's
/// `ApprovalRuntime`, so interactive tool approvals are presented in the same
/// UI as the root session.
async fn run_background_subagent(
  parent_runtime: Arc<Runtime>,
  type_def: AgentTypeDefinition,
  agent_id: String,
  description: String,
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

  // Persist the agent instance record.
  let session_dir = parent_runtime
    .background_manager()
    .store()
    .map(|s| s.root().parent().unwrap().to_path_buf())
    .ok_or_else(|| ToolError::Fatal("Background tasks not available".to_string()))?;
  let subagent_store = Arc::new(SubagentStore::new(session_dir));

  let record = crate::subagents::AgentInstanceRecord {
    agent_id: agent_id.clone(),
    subagent_type: subagent_type.clone(),
    status: SubagentStatus::RunningBackground,
    description: description.clone(),
    created_at: now_secs(),
    updated_at: now_secs(),
    last_task_id: None,
    launch_spec: crate::subagents::AgentLaunchSpec {
      agent_id: agent_id.clone(),
      subagent_type: subagent_type.clone(),
      model_override: effective_model.clone(),
      effective_model: effective_model.clone(),
      created_at: now_secs(),
    },
  };
  if let Err(e) = subagent_store.save_meta(&record) {
    return Err(ToolError::Fatal(format!(
      "Failed to persist subagent instance: {}",
      e
    )));
  }
  if let Err(e) = subagent_store.save_prompt(&agent_id, &prompt) {
    return Err(ToolError::Fatal(format!(
      "Failed to persist subagent prompt: {}",
      e
    )));
  }

  let payload = AgentTaskPayload {
    agent_id: agent_id.clone(),
    subagent_type: subagent_type.clone(),
    prompt: prompt.clone(),
    model_override: effective_model.clone(),
    effective_model: effective_model.clone(),
    tool_policy: type_def.tool_policy.clone(),
    description: description.clone(),
    resumed: false,
    timeout_s: timeout_seconds,
  };

  let kind_payload = serde_json::to_value(payload)
    .map_err(|e| ToolError::Fatal(format!("Failed to serialize agent task payload: {}", e)))?;

  let config_dir = parent_runtime.config_dir().to_string_lossy().to_string();
  let view = match parent_runtime.background_manager().create_agent_task(
    &description,
    &agent_id,
    timeout_seconds,
    kind_payload,
    Some(config_dir),
  ) {
    Ok(v) => v,
    Err(e) => {
      let _ = update_subagent_status(&subagent_store, &agent_id, SubagentStatus::Failed, None);
      return Err(ToolError::RespondToModel(format!(
        "Failed to create background agent task: {}",
        e
      )));
    }
  };
  let task_id = view.spec.id.clone();

  // Build subagent setup.
  let (child_runtime, provider, messages) = build_subagent_setup(
    &parent_runtime,
    &type_def,
    &agent_id,
    &prompt,
    effective_model,
  )
  .await?;

  // Spawn the in-process runner and register it for cleanup.
  let manager = parent_runtime.background_manager();
  let runner = run_in_process_agent_task(
    manager.clone(),
    subagent_store.clone(),
    child_runtime,
    provider,
    messages,
    task_id.clone(),
    agent_id.clone(),
    subagent_type.clone(),
    Some(type_def.tool_policy.clone()),
    timeout_seconds,
  );
  let task_id_for_runner = task_id.clone();
  let handle = tokio::spawn(async move {
    runner.await;
    manager.unregister_agent_task(&task_id_for_runner);
  });
  parent_runtime
    .background_manager()
    .register_agent_task(&task_id, handle.abort_handle());

  let output = format!(
    "agent_id: {}
resumed: false
actual_subagent_type: {}
status: running_background
background_task_id: {}

The subagent is running in the background. Use TaskList / TaskOutput / TaskStop to monitor it; a notification will be delivered when it completes.",
    agent_id, subagent_type, task_id
  );

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

fn update_subagent_status(
  store: &SubagentStore,
  agent_id: &str,
  status: SubagentStatus,
  task_id: Option<String>,
) -> anyhow::Result<()> {
  let mut record = store
    .load_meta(agent_id)?
    .ok_or_else(|| std::io::Error::other(format!("Agent instance {} not found", agent_id)))?;
  record.status = status;
  record.updated_at = now_secs();
  if let Some(id) = task_id {
    record.last_task_id = Some(id);
  }
  store.save_meta(&record)
}

/// Build the child runtime, provider, and initial messages for a subagent.
async fn build_subagent_setup(
  parent_runtime: &Arc<Runtime>,
  type_def: &AgentTypeDefinition,
  agent_id: &str,
  prompt: &str,
  effective_model: Option<String>,
) -> Result<(Arc<Runtime>, Box<dyn LLMProvider>, Vec<Message>), ToolError> {
  let subagent_type = type_def.name.clone();

  // Build child runtime with tool policy and optional model override.
  let child_runtime = Arc::new(parent_runtime.copy_for_subagent(
    agent_id.to_string(),
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
      prompt.to_string()
    } else {
      format!(
        "{}

{}",
        git_ctx, prompt
      )
    }
  } else {
    prompt.to_string()
  };

  let messages = vec![Message::system(system_prompt), Message::user(user_prompt)];
  Ok((child_runtime, provider, messages))
}

/// Run a background agent task in-process.
///
/// Streams transcript chunks to the task output log, honors kill requests from
/// the control file, and finalizes both the task runtime and subagent store
/// status when the agent completes, times out, or is killed.
#[allow(clippy::too_many_arguments)]
async fn run_in_process_agent_task(
  manager: Arc<BackgroundTaskManager>,
  subagent_store: Arc<SubagentStore>,
  child_runtime: Arc<Runtime>,
  provider: Box<dyn LLMProvider>,
  messages: Vec<Message>,
  task_id: String,
  agent_id: String,
  _subagent_type: String,
  tool_policy: Option<ToolPolicy>,
  timeout_seconds: Option<u64>,
) {
  let store = match manager.store() {
    Some(s) => s,
    None => return,
  };

  // Mark task as running.
  {
    let mut runtime = store.read_runtime(&task_id);
    runtime.status = TaskStatus::Running;
    runtime.started_at = Some(now_secs());
    runtime.updated_at = runtime.started_at.unwrap();
    store.write_runtime(&task_id, &runtime);
  }

  if let Err(e) = update_subagent_status(
    &subagent_store,
    &agent_id,
    SubagentStatus::RunningBackground,
    Some(task_id.clone()),
  ) {
    log::warn!("Failed to update subagent status: {}", e);
  }

  // Create a private wire bus for the subagent.
  let child_bus = WireBus::new(1024);
  let child_publisher = child_bus.publisher();
  let mut subscriber = child_bus.subscriber();

  // Spawn the subagent session. `yolo` is false so approval requests flow
  // through the shared ApprovalRuntime to the parent's interactive UI.
  let handle = ChatSession::start_subagent(
    agent_id.clone(),
    provider,
    messages,
    child_runtime,
    child_publisher,
    tool_policy,
    false,
  );

  // Send an empty user message to start the turn.
  handle.send_message("");

  // Watch the control file for kill requests.
  let kill_notify = Arc::new(Notify::new());
  let kill_notify_watcher = kill_notify.clone();
  let manager_watcher = manager.clone();
  let task_id_watcher = task_id.clone();
  tokio::spawn(async move {
    let store = match manager_watcher.store() {
      Some(s) => s,
      None => return,
    };
    let poll_interval = Duration::from_secs(1);
    loop {
      tokio::select! {
        _ = kill_notify_watcher.notified() => break,
        _ = sleep(poll_interval) => {
          let control = store.read_control(&task_id_watcher);
          if control.kill_requested_at.is_some() {
            let mut runtime = store.read_runtime(&task_id_watcher);
            if !is_terminal_task_status(runtime.status) {
              runtime.status = TaskStatus::Killed;
              runtime.interrupted = true;
              runtime.finished_at = Some(now_secs());
              runtime.updated_at = runtime.finished_at.unwrap();
              runtime.failure_reason = control
                .kill_reason
                .clone()
                .or_else(|| Some("Killed by user".to_string()));
              store.write_runtime(&task_id_watcher, &runtime);
            }
            break;
          }
        }
      }
    }
  });

  let deadline = timeout_seconds.map(|s| Instant::now() + Duration::from_secs(s));
  let mut in_turn = false;
  let mut current_response = String::new();
  let mut timed_out = false;
  let mut killed = false;

  loop {
    let recv_fut = subscriber.recv();
    tokio::select! {
      biased;
      _ = kill_notify.notified() => {
        killed = true;
        break;
      }
      result = recv_fut => {
        let msg = match result {
          Ok(m) => m,
          Err(_) => break,
        };

        match msg {
          WireMessage::TurnBegin => {
            in_turn = true;
            current_response.clear();
          }
          WireMessage::ContentChunk { text } => {
            if in_turn {
              current_response.push_str(&text);
              store.append_output(&task_id, &text);
            }
          }
          WireMessage::TurnEnd => {
            if in_turn && !current_response.is_empty() {
              break;
            }
            in_turn = false;
          }
          WireMessage::ToolCallEnd { output, .. } => {
            if current_response.is_empty() {
              current_response.push_str(&output);
            }
          }
          WireMessage::Error { message } => {
            let error_text = format!("
Subagent error: {}
", message);
            store.append_output(&task_id, &error_text);
            current_response = format!("Subagent error: {}", message);
            break;
          }
          _ => {}
        }
      }
    }

    if let Some(d) = deadline
      && Instant::now() >= d
    {
      timed_out = true;
      break;
    }
  }

  handle.shutdown();

  // Write final summary block.
  let summary = if current_response.is_empty() {
    "No response from subagent.".to_string()
  } else {
    current_response
  };
  let summary_block = format!(
    "

[summary]
{}",
    summary
  );
  store.append_output(&task_id, &summary_block);

  // Append transcript to subagent store.
  if let Ok(transcript) = std::fs::read_to_string(store.output_path(&task_id)) {
    let _ = subagent_store.append_output(&agent_id, &transcript);
  }

  // Determine final status.
  let control = store.read_control(&task_id);
  let runtime = store.read_runtime(&task_id);
  let (status, failure_reason) =
    if runtime.status == TaskStatus::Killed || control.kill_requested_at.is_some() || killed {
      (
        TaskStatus::Killed,
        control
          .kill_reason
          .clone()
          .or_else(|| runtime.failure_reason.clone())
          .or_else(|| Some("Killed by user".to_string())),
      )
    } else if timed_out {
      (TaskStatus::Failed, Some("Timed out".to_string()))
    } else if summary == "No response from subagent." {
      (
        TaskStatus::Failed,
        Some("Subagent produced no response".to_string()),
      )
    } else {
      (TaskStatus::Completed, None)
    };

  {
    let mut runtime = store.read_runtime(&task_id);
    runtime.status = status;
    runtime.timed_out = timed_out;
    runtime.finished_at = Some(now_secs());
    runtime.updated_at = runtime.finished_at.unwrap();
    runtime.failure_reason = failure_reason;
    store.write_runtime(&task_id, &runtime);
  }

  let subagent_status = match status {
    TaskStatus::Completed => SubagentStatus::Completed,
    TaskStatus::Failed => SubagentStatus::Failed,
    TaskStatus::Killed => SubagentStatus::Killed,
    _ => SubagentStatus::Failed,
  };
  if let Err(e) = update_subagent_status(
    &subagent_store,
    &agent_id,
    subagent_status,
    Some(task_id.clone()),
  ) {
    log::warn!("Failed to update final subagent status: {}", e);
  }

  log::info!(
    "Background agent task {} finished with status {:?}",
    task_id,
    status
  );
}

fn is_terminal_task_status(status: TaskStatus) -> bool {
  use crate::background::models::is_terminal_status;
  is_terminal_status(status)
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
