//! ReadFile tool handler.
//!
//! Reads the contents of a file at the specified path.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::fs;

use crate::tools::{ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, parse_arguments};

/// Handler for the ReadFile tool
pub struct ReadFileHandler;

/// Maximum number of lines to read in one call
const MAX_LINES: usize = 1000;

/// Maximum line length (truncate longer lines)
const MAX_LINE_LENGTH: usize = 2000;

/// Arguments for the ReadFile tool
#[derive(Debug, Deserialize)]
struct ReadFileArgs {
  /// Path to the file to read
  path: String,
  /// Line number to start reading from (1-indexed)
  #[serde(default = "default_offset")]
  offset: usize,
  /// Maximum number of lines to read
  #[serde(default = "default_limit")]
  limit: usize,
}

fn default_offset() -> usize {
  1
}

fn default_limit() -> usize {
  100
}

#[async_trait]
impl ToolHandler for ReadFileHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    false
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, cwd, .. } = invocation;

    // Extract arguments from payload
    let arguments = match payload {
      crate::tools::ToolPayload::Function { arguments } => arguments,
      _ => {
        return Err(ToolError::RespondToModel(
          "ReadFile handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: ReadFileArgs = parse_arguments(&arguments)?;

    // Validate offset
    if args.offset == 0 {
      return Err(ToolError::RespondToModel(
        "offset must be a 1-indexed line number (>= 1)".to_string(),
      ));
    }

    // Validate limit
    if args.limit == 0 {
      return Err(ToolError::RespondToModel(
        "limit must be greater than zero".to_string(),
      ));
    }

    // Cap limit to MAX_LINES
    let limit = args.limit.min(MAX_LINES);

    // Resolve path
    let path = PathBuf::from(&args.path);
    let resolved_path = if path.is_absolute() {
      path
    } else {
      cwd.join(&path)
    };

    // Check if file exists
    let metadata = match fs::metadata(&resolved_path).await {
      Ok(m) => m,
      Err(e) => {
        return Err(ToolError::RespondToModel(format!(
          "Failed to access file '{}': {}",
          args.path, e
        )));
      }
    };

    // Check if it's a file
    if !metadata.is_file() {
      return Err(ToolError::RespondToModel(format!(
        "'{}' is not a file",
        args.path
      )));
    }

    // Read file content
    let content = match fs::read_to_string(&resolved_path).await {
      Ok(c) => c,
      Err(e) => {
        return Err(ToolError::RespondToModel(format!(
          "Failed to read file '{}': {}",
          args.path, e
        )));
      }
    };

    // Process lines
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Calculate start and end indices (convert 1-indexed to 0-indexed)
    let start_idx = (args.offset - 1).min(total_lines);
    let end_idx = (start_idx + limit).min(total_lines);

    // Extract requested lines
    let selected_lines = &lines[start_idx..end_idx];

    // Format output with line numbers (like `cat -n`)
    let mut result_lines = Vec::new();
    for (idx, line) in selected_lines.iter().enumerate() {
      let line_num = start_idx + idx + 1; // 1-indexed line number

      // Truncate line if too long
      let truncated_line = if line.chars().count() > MAX_LINE_LENGTH {
        let truncated: String = line.chars().take(MAX_LINE_LENGTH).collect();
        format!("{}...", truncated)
      } else {
        line.to_string()
      };

      // Format with line number (6-digit width, right-aligned, with tab separator)
      result_lines.push(format!("{:6}\t{}", line_num, truncated_line));
    }

    // Build response message
    let lines_read = result_lines.len();
    let mut message = format!(
      "{} lines read from file starting from line {}.",
      lines_read, args.offset
    );

    if lines_read == 0 {
      message = "No lines read from file.".to_string();
    } else if end_idx >= total_lines && start_idx < total_lines {
      message.push_str(" End of file reached.");
    }

    if lines_read >= limit && args.limit > limit {
      message.push_str(&format!(" Max {} lines reached.", MAX_LINES));
    }

    // Join result lines with newlines
    let output = result_lines.join("\n");

    Ok(ToolOutput::success(output))
  }
}

impl ReadFileHandler {
  /// Create a new ReadFileHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for ReadFileHandler {
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
    let json = r#"{"path": "/tmp/test.txt", "offset": 5, "limit": 10}"#;
    let args: ReadFileArgs = parse_arguments(json).unwrap();

    assert_eq!(args.path, "/tmp/test.txt");
    assert_eq!(args.offset, 5);
    assert_eq!(args.limit, 10);
  }

  #[test]
  fn test_parse_arguments_defaults() {
    let json = r#"{"path": "/tmp/test.txt"}"#;
    let args: ReadFileArgs = parse_arguments(json).unwrap();

    assert_eq!(args.path, "/tmp/test.txt");
    assert_eq!(args.offset, 1); // default
    assert_eq!(args.limit, 100); // default
  }

  #[tokio::test]
  async fn test_read_file_handler() {
    // Create a temporary file
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("ironcode_test_read_file.txt");
    let test_content = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\n";
    fs::write(&test_file, test_content).await.unwrap();

    let handler = ReadFileHandler::new();
    let invocation = ToolInvocation::new(
      "ReadFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "offset": 2, "limit": 3}}"#,
          test_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("Line 2"));
    assert!(output.contains("Line 3"));
    assert!(output.contains("Line 4"));
    assert!(!output.contains("Line 1")); // Should not include line 1
    assert!(!output.contains("Line 5")); // Should not include line 5

    // Cleanup
    fs::remove_file(&test_file).await.unwrap();
  }
}
