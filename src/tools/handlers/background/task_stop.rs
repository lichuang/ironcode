//! TaskStop tool handler.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::background::BackgroundTaskManager;
use crate::background::models::is_terminal_status;
use crate::tools::{ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, ToolPayload, parse_arguments};

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

  async fn preview(&self, invocation: &ToolInvocation) -> Option<String> {
    let ToolPayload::Function { arguments } = &invocation.payload;
    let args: TaskStopArgs = parse_arguments(arguments).ok()?;
    let view = self.manager.get_task(&args.task_id)?;

    let mut lines = vec![
      format!("Stop background task `{}`", args.task_id),
      String::new(),
      format_task(&view, true),
    ];
    if is_terminal_status(view.runtime.status) {
      lines.push(String::new());
      lines.push(
        "Note: This task is already in a terminal state. Stopping will be a no-op.".to_string(),
      );
    }
    Some(lines.join("\n"))
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
