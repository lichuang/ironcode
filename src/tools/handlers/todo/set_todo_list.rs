//! SetTodoList tool handler.
//!
//! Update the whole todo list to track progress across subtasks.

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Handler for the SetTodoList tool
pub struct SetTodoListHandler;

/// A single todo item
#[derive(Debug, Deserialize)]
struct TodoItem {
  /// The title of the todo
  title: String,
  /// The status of the todo
  status: String,
}

/// Arguments for the SetTodoList tool
#[derive(Debug, Deserialize)]
struct SetTodoListArgs {
  /// The updated todo list
  todos: Vec<TodoItem>,
}

#[async_trait]
impl ToolHandler for SetTodoListHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    // This tool doesn't modify files/system, it just updates the todo list display
    false
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    // Extract arguments from payload
    let arguments = match payload {
      crate::tools::ToolPayload::Function { arguments } => arguments,
      _ => {
        return Err(ToolError::RespondToModel(
          "SetTodoList handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: SetTodoListArgs = parse_arguments(&arguments)?;

    // Validate each todo item
    for (idx, todo) in args.todos.iter().enumerate() {
      if todo.title.trim().is_empty() {
        return Err(ToolError::RespondToModel(format!(
          "Todo item {}: title cannot be empty.",
          idx + 1
        )));
      }

      let status = todo.status.trim();
      if !matches!(status, "pending" | "in_progress" | "done") {
        return Err(ToolError::RespondToModel(format!(
          "Todo item {}: invalid status '{}'. Must be one of: pending, in_progress, done.",
          idx + 1,
          todo.status
        )));
      }
    }

    // Build formatted output
    let mut lines = Vec::new();
    lines.push("Todo list updated.".to_string());
    lines.push(String::new());

    for todo in &args.todos {
      let icon = match todo.status.as_str() {
        "done" => "[x]",
        "in_progress" => "[~]",
        _ => "[ ]",
      };
      lines.push(format!("{} {}", icon, todo.title));
    }

    Ok(ToolOutput::success(lines.join("\n")))
  }
}

impl SetTodoListHandler {
  /// Create a new SetTodoListHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for SetTodoListHandler {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  #[test]
  fn test_parse_arguments() {
    let json = r#"{
      "todos": [
        {"title": "Implement feature A", "status": "in_progress"},
        {"title": "Write tests", "status": "pending"},
        {"title": "Update docs", "status": "done"}
      ]
    }"#;
    let args: SetTodoListArgs = parse_arguments(json).unwrap();

    assert_eq!(args.todos.len(), 3);
    assert_eq!(args.todos[0].title, "Implement feature A");
    assert_eq!(args.todos[0].status, "in_progress");
    assert_eq!(args.todos[2].status, "done");
  }

  #[tokio::test]
  async fn test_set_todo_list_handler() {
    let temp_dir = std::env::temp_dir();
    let handler = SetTodoListHandler::new();

    let invocation = ToolInvocation::new(
      "SetTodoList",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{
          "todos": [
            {"title": "Task 1", "status": "done"},
            {"title": "Task 2", "status": "in_progress"},
            {"title": "Task 3", "status": "pending"}
          ]
        }"#
        .to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("Todo list updated"));
    assert!(output.contains("[x] Task 1"));
    assert!(output.contains("[~] Task 2"));
    assert!(output.contains("[ ] Task 3"));
  }

  #[tokio::test]
  async fn test_empty_title_validation() {
    let temp_dir = std::env::temp_dir();
    let handler = SetTodoListHandler::new();

    let invocation = ToolInvocation::new(
      "SetTodoList",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{
          "todos": [
            {"title": "", "status": "pending"}
          ]
        }"#
        .to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("title cannot be empty"));
  }

  #[tokio::test]
  async fn test_invalid_status_validation() {
    let temp_dir = std::env::temp_dir();
    let handler = SetTodoListHandler::new();

    let invocation = ToolInvocation::new(
      "SetTodoList",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{
          "todos": [
            {"title": "Task 1", "status": "invalid"}
          ]
        }"#
        .to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("invalid status"));
  }
}
