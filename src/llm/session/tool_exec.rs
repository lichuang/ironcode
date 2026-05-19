//! ToolExecutor — tool call execution and preview.

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::join_all;

use crate::llm::types::ToolCall;
use crate::tools::{ExecutableToolRegistry, ToolError, ToolInvocation, ToolPayload};

pub struct ToolExecutor {
  cwd: PathBuf,
  registry: Arc<ExecutableToolRegistry>,
}

impl ToolExecutor {
  pub fn new(cwd: PathBuf, registry: Arc<ExecutableToolRegistry>) -> Self {
    Self { cwd, registry }
  }

  pub async fn execute(&self, tool_call: &ToolCall) -> Result<String, crate::tools::ToolError> {
    let invocation = ToolInvocation::new(
      &tool_call.name,
      &tool_call.id,
      ToolPayload::Function {
        arguments: tool_call.arguments.clone(),
      },
      &self.cwd,
    );
    let output = self.registry.dispatch(invocation).await?;
    Ok(output.into_response())
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
}
