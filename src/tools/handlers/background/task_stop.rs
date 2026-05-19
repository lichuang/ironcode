//! TaskStop tool handler.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::background::BackgroundTaskManager;
use crate::tools::{ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, parse_arguments};

use super::format_task;

pub struct TaskStopHandler {
  manager: Arc<BackgroundTaskManager>,
}

#[derive(Debug, Deserialize)]
struct TaskStopArgs {
  task_id: String,
  #[serde(default = "default_reason")]
  reason: String,
}

fn default_reason() -> String {
  "Stopped by TaskStop".to_string()
}

#[async_trait]
impl ToolHandler for TaskStopHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    let crate::tools::ToolPayload::Function { arguments } = payload;

    let args: TaskStopArgs = parse_arguments(&arguments)?;
    let reason = args.reason.trim();
    let reason = if reason.is_empty() {
      "Stopped by TaskStop"
    } else {
      reason
    };

    let view = self
      .manager
      .kill(&args.task_id, reason)
      .map_err(ToolError::RespondToModel)?;

    let output = format_task(&view, true);
    Ok(ToolOutput::success(output))
  }
}

impl TaskStopHandler {
  pub fn new(manager: Arc<BackgroundTaskManager>) -> Self {
    Self { manager }
  }
}
