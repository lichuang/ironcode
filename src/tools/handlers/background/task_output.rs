//! TaskOutput tool handler.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::background::{BackgroundTaskManager, TaskView};
use crate::tools::{ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, parse_arguments};

pub struct TaskOutputHandler {
  manager: Arc<BackgroundTaskManager>,
}

#[derive(Debug, Deserialize)]
struct TaskOutputArgs {
  task_id: String,
  #[serde(default)]
  block: bool,
  #[serde(default = "default_timeout")]
  timeout: u64,
}

fn default_timeout() -> u64 {
  30
}

#[async_trait]
impl ToolHandler for TaskOutputHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    let crate::tools::ToolPayload::Function { arguments } = payload;

    let args: TaskOutputArgs = parse_arguments(&arguments)?;

    let view = self
      .manager
      .get_task(&args.task_id)
      .ok_or_else(|| ToolError::RespondToModel(format!("Task not found: {}", args.task_id)))?;

    let view = if args.block {
      self
        .manager
        .wait(&args.task_id, args.timeout)
        .await
        .unwrap_or(view)
    } else {
      view
    };

    let retrieval_status = if is_terminal(view.runtime.status) {
      "success"
    } else if args.block {
      "timeout"
    } else {
      "not_ready"
    };

    let (output, full_output_available, output_size, output_preview_bytes, output_truncated) =
      self.render_output_preview(&args.task_id);

    let output = format_task_output(
      &view,
      retrieval_status,
      &output,
      full_output_available,
      output_size,
      output_preview_bytes,
      output_truncated,
    );

    Ok(ToolOutput::success(output))
  }
}

impl TaskOutputHandler {
  pub fn new(manager: Arc<BackgroundTaskManager>) -> Self {
    Self { manager }
  }

  fn render_output_preview(&self, task_id: &str) -> (String, bool, usize, usize, bool) {
    let output_path = self.manager.output_path(task_id).unwrap_or_default();

    let output_size = if output_path.exists() {
      std::fs::metadata(&output_path)
        .map(|m| m.len() as usize)
        .unwrap_or(0)
    } else {
      0
    };

    let read_max_bytes = self.manager.config().read_max_bytes;
    let preview_offset = output_size.saturating_sub(read_max_bytes);
    let chunk = self
      .manager
      .read_output(task_id, preview_offset, read_max_bytes)
      .unwrap_or_else(|| crate::background::TaskOutputChunk {
        task_id: task_id.to_string(),
        offset: 0,
        next_offset: 0,
        text: String::new(),
        eof: true,
        status: crate::background::TaskStatus::Created,
      });

    let truncated = preview_offset > 0;
    (
      chunk.text.trim_end_matches('\n').to_string(),
      output_size > 0,
      output_size,
      chunk.next_offset - chunk.offset,
      truncated,
    )
  }
}

fn is_terminal(status: crate::background::TaskStatus) -> bool {
  matches!(
    status,
    crate::background::TaskStatus::Completed
      | crate::background::TaskStatus::Failed
      | crate::background::TaskStatus::Killed
      | crate::background::TaskStatus::Lost
  )
}

fn format_task_output(
  view: &TaskView,
  retrieval_status: &str,
  output: &str,
  full_output_available: bool,
  output_size_bytes: usize,
  output_preview_bytes: usize,
  output_truncated: bool,
) -> String {
  let terminal_reason = if view.runtime.timed_out {
    "timed_out"
  } else {
    &view.runtime.status.to_string()
  };

  let mut lines = vec![
    format!("retrieval_status: {}", retrieval_status),
    format!("task_id: {}", view.spec.id),
    format!("kind: {}", view.spec.kind),
    format!("status: {}", view.runtime.status),
    format!("description: {}", view.spec.description),
  ];

  if !view.spec.command.is_empty() {
    lines.push(format!("command: {}", view.spec.command));
  }

  lines.extend([
    format!("interrupted: {}", view.runtime.interrupted),
    format!("timed_out: {}", view.runtime.timed_out),
    format!("terminal_reason: {}", terminal_reason),
  ]);

  if let Some(exit_code) = view.runtime.exit_code {
    lines.push(format!("exit_code: {}", exit_code));
  }
  if let Some(ref reason) = view.runtime.failure_reason {
    lines.push(format!("reason: {}", reason));
  }

  let full_output_hint = if full_output_available {
    "full_output_hint: Use ReadFile with the task output path to inspect the full log in pages."
      .to_string()
  } else {
    "full_output_hint: No output file is currently available for this task.".to_string()
  };

  lines.extend([
    String::new(),
    format!("output_size_bytes: {}", output_size_bytes),
    format!("output_preview_bytes: {}", output_preview_bytes),
    format!("output_truncated: {}", output_truncated),
    String::new(),
    format!("full_output_available: {}", full_output_available),
    "full_output_tool: ReadFile".to_string(),
    full_output_hint,
  ]);

  let rendered_output = if output.is_empty() {
    "[no output available]".to_string()
  } else {
    output.to_string()
  };

  let final_output = if output_truncated {
    format!(
      "[Truncated. Use ReadFile to read the full log.]\n\n{}",
      rendered_output
    )
  } else {
    rendered_output
  };

  lines.extend([String::new(), "[output]".to_string(), final_output]);
  lines.join("\n")
}
