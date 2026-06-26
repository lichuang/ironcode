//! ToolExecutor — tool call execution and preview.

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::join_all;
use serde_json::Value;

use crate::hooks::{HookDecision, HookEngine, HookEventType, events};
use crate::llm::types::ToolCall;
use crate::subagents::ToolPolicy;
use crate::tools::{ExecutableToolRegistry, ToolError, ToolInvocation, ToolPayload};

/// Context needed to build hook payloads.
pub struct ToolExecutionContext {
  /// Session identifier.
  pub session_id: String,
  /// Working directory.
  pub cwd: PathBuf,
}

pub struct ToolExecutor {
  cwd: PathBuf,
  registry: Arc<ExecutableToolRegistry>,
  hook_engine: Arc<HookEngine>,
  ctx: Option<ToolExecutionContext>,
  tool_policy: Option<ToolPolicy>,
}

impl ToolExecutor {
  pub fn new(
    cwd: PathBuf,
    registry: Arc<ExecutableToolRegistry>,
    hook_engine: Arc<HookEngine>,
  ) -> Self {
    Self {
      cwd,
      registry,
      hook_engine,
      ctx: None,
      tool_policy: None,
    }
  }

  /// Bind session context for hook payloads.
  pub fn bind_context(&mut self, ctx: ToolExecutionContext) {
    self.ctx = Some(ctx);
  }

  /// Enforce a tool policy for this executor.
  pub fn set_tool_policy(&mut self, policy: ToolPolicy) {
    self.tool_policy = Some(policy);
  }

  pub async fn execute(&self, tool_call: &ToolCall) -> Result<String, ToolError> {
    if let Some(policy) = &self.tool_policy
      && !policy.allows(&tool_call.name)
    {
      return Err(ToolError::RespondToModel(format!(
        "Tool '{}' is not available to this subagent.",
        tool_call.name
      )));
    }

    self.run_pre_tool_use_hook(tool_call).await?;

    let invocation = ToolInvocation::new(
      &tool_call.name,
      &tool_call.id,
      ToolPayload::Function {
        arguments: tool_call.arguments.clone(),
      },
      &self.cwd,
    );

    match self.registry.dispatch(invocation).await {
      Ok(output) => {
        let response = output.into_response();
        self.run_post_tool_use_hook(tool_call, &response).await;
        Ok(response)
      }
      Err(err) => {
        let error_message = err.to_string();
        self
          .run_post_tool_use_failure_hook(tool_call, &error_message)
          .await;
        Err(err)
      }
    }
  }

  pub async fn preview(&self, tool_call: &ToolCall) -> Option<String> {
    let invocation = ToolInvocation::new(
      &tool_call.name,
      &tool_call.id,
      ToolPayload::Function {
        arguments: tool_call.arguments.clone(),
      },
      &self.cwd,
    );
    self.registry.preview(&invocation).await
  }

  /// Execute multiple tool calls concurrently.
  ///
  /// Returns results in the same order as the input slice.
  pub async fn execute_many(&self, tool_calls: &[&ToolCall]) -> Vec<Result<String, ToolError>> {
    let futures = tool_calls
      .iter()
      .map(|tc| async move { self.execute(tc).await });
    join_all(futures).await
  }

  async fn run_pre_tool_use_hook(&self, tool_call: &ToolCall) -> Result<(), ToolError> {
    if !self.hook_engine.has_hooks_for(HookEventType::PreToolUse) {
      return Ok(());
    }

    let tool_input = parse_tool_arguments(&tool_call.arguments);
    let (session_id, cwd) = self.context_strings();

    let results = self
      .hook_engine
      .trigger(
        HookEventType::PreToolUse,
        &tool_call.name,
        events::pre_tool_use(session_id, cwd, &tool_call.name, &tool_input, &tool_call.id),
      )
      .await;

    for result in results {
      if let HookDecision::Block { reason } = result.decision {
        return Err(ToolError::RespondToModel(if reason.is_empty() {
          "Blocked by PreToolUse hook".to_string()
        } else {
          reason
        }));
      }
    }

    Ok(())
  }

  async fn run_post_tool_use_hook(&self, tool_call: &ToolCall, output: &str) {
    if !self.hook_engine.has_hooks_for(HookEventType::PostToolUse) {
      return;
    }

    let tool_input = parse_tool_arguments(&tool_call.arguments);
    let (session_id, cwd) = self.context_strings();

    let _handle = self.hook_engine.trigger_fire_and_forget(
      HookEventType::PostToolUse,
      tool_call.name.clone(),
      events::post_tool_use(
        session_id,
        cwd,
        &tool_call.name,
        &tool_input,
        output,
        &tool_call.id,
      ),
    );
  }

  async fn run_post_tool_use_failure_hook(&self, tool_call: &ToolCall, error: &str) {
    if !self
      .hook_engine
      .has_hooks_for(HookEventType::PostToolUseFailure)
    {
      return;
    }

    let tool_input = parse_tool_arguments(&tool_call.arguments);
    let (session_id, cwd) = self.context_strings();

    let _handle = self.hook_engine.trigger_fire_and_forget(
      HookEventType::PostToolUseFailure,
      tool_call.name.clone(),
      events::post_tool_use_failure(
        session_id,
        cwd,
        &tool_call.name,
        &tool_input,
        error,
        &tool_call.id,
      ),
    );
  }

  fn context_strings(&self) -> (&str, &str) {
    match self.ctx {
      Some(ref ctx) => (&ctx.session_id, ctx.cwd.to_str().unwrap_or(".")),
      None => ("", "."),
    }
  }
}

fn parse_tool_arguments(arguments: &str) -> Value {
  serde_json::from_str(arguments).unwrap_or_else(|_| {
    serde_json::json!({
      "raw_arguments": arguments,
    })
  })
}
