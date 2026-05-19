//! Background task tool handlers.

pub mod task_list;
pub mod task_output;
pub mod task_stop;

pub use task_list::TaskListHandler;
pub use task_output::TaskOutputHandler;
pub use task_stop::TaskStopHandler;

use crate::background::TaskView;

/// Format a single task for display.
pub fn format_task(view: &TaskView, include_command: bool) -> String {
  let mut lines = vec![
    format!("task_id: {}", view.spec.id),
    format!("kind: {}", view.spec.kind),
    format!("status: {}", view.runtime.status),
    format!("description: {}", view.spec.description),
  ];
  if include_command && !view.spec.command.is_empty() {
    lines.push(format!("command: {}", view.spec.command));
  }
  if let Some(exit_code) = view.runtime.exit_code {
    lines.push(format!("exit_code: {}", exit_code));
  }
  if let Some(ref reason) = view.runtime.failure_reason {
    lines.push(format!("reason: {}", reason));
  }
  lines.join("\n")
}

/// Format a list of tasks.
pub fn format_task_list(views: &[TaskView], active_only: bool, include_command: bool) -> String {
  let header = if active_only {
    "active_background_tasks"
  } else {
    "background_tasks"
  };
  if views.is_empty() {
    return format!("{}: 0\n[no tasks]", header);
  }

  let mut lines = vec![format!("{}: {}", header, views.len()), String::new()];
  for (index, view) in views.iter().enumerate() {
    lines.push(format!("[{}]", index + 1));
    lines.push(format_task(view, include_command));
    lines.push(String::new());
  }
  lines.join("\n").trim_end().to_string()
}
