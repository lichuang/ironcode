//! WriteFile tool handler.
//!
//! Writes content to a file at the specified path.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::fs;

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Handler for the WriteFile tool
pub struct WriteFileHandler;

/// Arguments for the WriteFile tool
#[derive(Debug, Deserialize)]
struct WriteFileArgs {
  /// Path to the file to write
  path: String,
  /// Content to write to the file
  content: String,
  /// Write mode: "overwrite" or "append"
  #[serde(default = "default_mode")]
  mode: String,
}

fn default_mode() -> String {
  "overwrite".to_string()
}

#[async_trait]
impl ToolHandler for WriteFileHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    true
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, cwd, .. } = invocation;

    // Extract arguments from payload
    let arguments = match payload {
      crate::tools::ToolPayload::Function { arguments } => arguments,
      _ => {
        return Err(ToolError::RespondToModel(
          "WriteFile handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: WriteFileArgs = parse_arguments(&arguments)?;

    // Validate mode
    if args.mode != "overwrite" && args.mode != "append" {
      return Err(ToolError::RespondToModel(format!(
        "Invalid write mode: `{}`. Mode must be either `overwrite` or `append`.",
        args.mode
      )));
    }

    // Resolve path
    let path = PathBuf::from(&args.path);
    let resolved_path = if path.is_absolute() {
      path
    } else {
      cwd.join(&path)
    };

    // Check if parent directory exists
    let parent = resolved_path.parent().ok_or_else(|| {
      ToolError::RespondToModel(format!("`{}` has no parent directory", args.path))
    })?;

    if !parent.exists() {
      return Err(ToolError::RespondToModel(format!(
        "`{}` parent directory does not exist",
        args.path
      )));
    }

    // Check if path is a directory
    if resolved_path.exists() {
      let metadata = fs::metadata(&resolved_path).await.map_err(|e| {
        ToolError::RespondToModel(format!("Failed to access file '{}': {}", args.path, e))
      })?;

      if metadata.is_dir() {
        return Err(ToolError::RespondToModel(format!(
          "`{}` is a directory, not a file",
          args.path
        )));
      }
    }

    // Perform write operation
    let write_result = match args.mode.as_str() {
      "overwrite" => fs::write(&resolved_path, &args.content).await,
      "append" => {
        use tokio::io::AsyncWriteExt;
        match fs::OpenOptions::new().append(true).create(true).open(&resolved_path).await {
          Ok(mut file) => file.write_all(args.content.as_bytes()).await,
          Err(e) => Err(e),
        }
      }
      _ => unreachable!(),
    };

    if let Err(e) = write_result {
      return Err(ToolError::RespondToModel(format!(
        "Failed to write to file '{}': {}",
        args.path, e
      )));
    }

    // Get file size for success message
    let file_size = match fs::metadata(&resolved_path).await {
      Ok(m) => m.len(),
      Err(_) => 0,
    };

    let action = if args.mode == "overwrite" {
      "overwritten"
    } else {
      "appended to"
    };

    let message = format!(
      "File successfully {}. Current size: {} bytes.",
      action, file_size
    );

    Ok(ToolOutput::success(message))
  }
}

impl WriteFileHandler {
  /// Create a new WriteFileHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for WriteFileHandler {
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
    let json = r#"{"path": "/tmp/test.txt", "content": "Hello World", "mode": "append"}"#;
    let args: WriteFileArgs = parse_arguments(json).unwrap();

    assert_eq!(args.path, "/tmp/test.txt");
    assert_eq!(args.content, "Hello World");
    assert_eq!(args.mode, "append");
  }

  #[test]
  fn test_parse_arguments_defaults() {
    let json = r#"{"path": "/tmp/test.txt", "content": "Hello World"}"#;
    let args: WriteFileArgs = parse_arguments(json).unwrap();

    assert_eq!(args.path, "/tmp/test.txt");
    assert_eq!(args.content, "Hello World");
    assert_eq!(args.mode, "overwrite"); // default
  }

  #[tokio::test]
  async fn test_write_file_handler_overwrite() {
    // Create a temporary directory
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("ironcode_test_write_file.txt");

    // Clean up if exists
    let _ = fs::remove_file(&test_file).await;

    let handler = WriteFileHandler::new();
    let invocation = ToolInvocation::new(
      "WriteFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "content": "Hello World", "mode": "overwrite"}}"#,
          test_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("successfully overwritten"));
    assert!(output.contains("bytes"));

    // Verify file content
    let content = fs::read_to_string(&test_file).await.unwrap();
    assert_eq!(content, "Hello World");

    // Cleanup
    fs::remove_file(&test_file).await.unwrap();
  }

  #[tokio::test]
  async fn test_write_file_handler_append() {
    // Create a temporary directory
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("ironcode_test_write_file_append.txt");

    // Clean up if exists
    let _ = fs::remove_file(&test_file).await;

    // First write
    fs::write(&test_file, "Hello ").await.unwrap();

    let handler = WriteFileHandler::new();
    let invocation = ToolInvocation::new(
      "WriteFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "content": "World", "mode": "append"}}"#,
          test_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("successfully appended"));

    // Verify file content
    let content = fs::read_to_string(&test_file).await.unwrap();
    assert_eq!(content, "Hello World");

    // Cleanup
    fs::remove_file(&test_file).await.unwrap();
  }

  #[tokio::test]
  async fn test_write_file_handler_parent_not_exists() {
    let temp_dir = std::env::temp_dir();
    let non_existent_parent = temp_dir.join("non_existent_dir_12345");
    let test_file = non_existent_parent.join("test.txt");

    let handler = WriteFileHandler::new();
    let invocation = ToolInvocation::new(
      "WriteFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "content": "Hello"}}"#,
          test_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());

    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("parent directory does not exist"));
  }
}
