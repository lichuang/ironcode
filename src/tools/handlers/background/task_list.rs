//! TaskList tool handler.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::background::BackgroundTaskManager;
use crate::tools::{ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, parse_arguments};

use super::format_task_list;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

pub struct TaskListHandler {
  manager: Arc<BackgroundTaskManager>,
}

#[derive(Debug, Deserialize)]
struct TaskListArgs {
  #[serde(default = "default_active_only")]
  active_only: bool,
  #[serde(default = "default_limit")]
  limit: usize,
}

fn default_active_only() -> bool {
  true
}

fn default_limit() -> usize {
  DEFAULT_LIMIT
}

#[async_trait]
impl ToolHandler for TaskListHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    let crate::tools::ToolPayload::Function { arguments } = payload;

    let args: TaskListArgs = parse_arguments(&arguments)?;
    let limit = args.limit.clamp(1, MAX_LIMIT);

    let views = self
      .manager
      .list_tasks(args.active_only, limit)
      .map_err(ToolError::RespondToModel)?;

    let output = format_task_list(&views, args.active_only, true);
    Ok(ToolOutput::success(output))
  }
}

impl TaskListHandler {
  pub fn new(manager: Arc<BackgroundTaskManager>) -> Self {
    Self { manager }
  }
}
