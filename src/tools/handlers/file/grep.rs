//! Grep tool handler.
//!
//! A powerful search tool based-on ripgrep for searching patterns in file contents.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Handler for the Grep tool
pub struct GrepHandler;

/// Output mode for grep results
#[derive(Debug, Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum OutputMode {
  /// Show matching lines
  #[default]
  Content,
  /// Show file paths only
  FilesWithMatches,
  /// Show match counts
  CountMatches,
}

/// Arguments for the Grep tool
#[derive(Debug, Deserialize)]
struct GrepArgs {
  /// The regular expression pattern to search for
  pattern: String,
  /// File or directory to search in (defaults to current directory)
  #[serde(default = "default_path")]
  path: String,
  /// Glob pattern to filter files
  #[serde(default)]
  glob: Option<String>,
  /// Output mode
  #[serde(default)]
  output_mode: OutputMode,
  /// Number of lines to show before each match
  #[serde(default)]
  before_context: Option<usize>,
  /// Number of lines to show after each match
  #[serde(default)]
  after_context: Option<usize>,
  /// Number of lines to show before and after each match
  #[serde(default)]
  context: Option<usize>,
  /// Show line numbers in output
  #[serde(default)]
  line_number: bool,
  /// Case insensitive search
  #[serde(default)]
  ignore_case: bool,
  /// File type to search
  #[serde(default)]
  r#type: Option<String>,
  /// Limit output to first N lines
  #[serde(default)]
  head_limit: Option<usize>,
  /// Enable multiline mode
  #[serde(default)]
  multiline: bool,
}

fn default_path() -> String {
  ".".to_string()
}

#[async_trait]
impl ToolHandler for GrepHandler {
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
          "Grep handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: GrepArgs = parse_arguments(&arguments)?;

    // Resolve path
    let path = PathBuf::from(&args.path);
    let resolved_path = if path.is_absolute() {
      path
    } else {
      cwd.join(&path)
    };

    // Build ripgrep command
    let mut cmd = Command::new("rg");

    // Set pattern and path
    cmd.arg(&args.pattern);
    cmd.arg(&resolved_path);

    // Apply search options
    if args.ignore_case {
      cmd.arg("-i");
    }
    if args.multiline {
      cmd.arg("-U");
      cmd.arg("--multiline-dotall");
    }

    // Apply output mode options (only for content mode)
    match args.output_mode {
      OutputMode::Content => {
        if let Some(before) = args.before_context {
          cmd.arg("-B");
          cmd.arg(before.to_string());
        }
        if let Some(after) = args.after_context {
          cmd.arg("-A");
          cmd.arg(after.to_string());
        }
        if let Some(context) = args.context {
          cmd.arg("-C");
          cmd.arg(context.to_string());
        }
        if args.line_number {
          cmd.arg("-n");
        }
      }
      OutputMode::FilesWithMatches => {
        cmd.arg("-l");
      }
      OutputMode::CountMatches => {
        cmd.arg("-c");
      }
    }

    // Apply file filtering options
    if let Some(glob) = &args.glob {
      cmd.arg("-g");
      cmd.arg(glob);
    }
    if let Some(file_type) = &args.r#type {
      cmd.arg("-t");
      cmd.arg(file_type);
    }

    // Execute search
    let output = cmd.output().await.map_err(|e| {
      ToolError::Fatal(format!("Failed to execute ripgrep: {}", e))
    })?;

    if !output.status.success() {
      // Check if it's a "no matches found" case (exit code 1)
      let exit_code = output.status.code();
      if exit_code == Some(1) {
        return Ok(ToolOutput::success("No matches found."));
      }
      // Otherwise it's an error
      let stderr = String::from_utf8_lossy(&output.stderr);
      return Err(ToolError::RespondToModel(format!(
        "ripgrep error: {}",
        stderr
      )));
    }

    // Get stdout as string
    let mut result = String::from_utf8_lossy(&output.stdout).to_string();

    // Trim trailing newline
    if result.ends_with('\n') {
      result.pop();
    }

    // Apply head limit if specified
    if let Some(limit) = args.head_limit {
      let lines: Vec<&str> = result.lines().collect();
      if lines.len() > limit {
        let truncated: Vec<&str> = lines.into_iter().take(limit).collect();
        result = truncated.join("\n");
        result.push_str(&format!("\n... (results truncated to {} lines)", limit));
      }
    }

    if result.is_empty() {
      Ok(ToolOutput::success("No matches found."))
    } else {
      Ok(ToolOutput::success(result))
    }
  }
}

impl GrepHandler {
  /// Create a new GrepHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for GrepHandler {
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
    let json = r#"{"pattern": "test", "path": "/tmp", "ignore_case": true}"#;
    let args: GrepArgs = parse_arguments(json).unwrap();

    assert_eq!(args.pattern, "test");
    assert_eq!(args.path, "/tmp");
    assert!(args.ignore_case);
  }

  #[test]
  fn test_parse_arguments_defaults() {
    let json = r#"{"pattern": "test"}"#;
    let args: GrepArgs = parse_arguments(json).unwrap();

    assert_eq!(args.pattern, "test");
    assert_eq!(args.path, ".");
    assert!(!args.ignore_case);
    assert!(!args.line_number);
    assert!(!args.multiline);
  }

  #[test]
  fn test_parse_arguments_output_modes() {
    let json = r#"{"pattern": "test", "output_mode": "content"}"#;
    let args: GrepArgs = parse_arguments(json).unwrap();
    assert!(matches!(args.output_mode, OutputMode::Content));

    let json = r#"{"pattern": "test", "output_mode": "files_with_matches"}"#;
    let args: GrepArgs = parse_arguments(json).unwrap();
    assert!(matches!(args.output_mode, OutputMode::FilesWithMatches));

    let json = r#"{"pattern": "test", "output_mode": "count_matches"}"#;
    let args: GrepArgs = parse_arguments(json).unwrap();
    assert!(matches!(args.output_mode, OutputMode::CountMatches));
  }

  #[tokio::test]
  async fn test_grep_handler_files_with_matches() {
    let handler = GrepHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Grep",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "ReadFile", "output_mode": "files_with_matches"}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    // Should find read_file.rs or similar
    assert!(!output.is_empty());
  }

  #[tokio::test]
  async fn test_grep_handler_content() {
    let handler = GrepHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Grep",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "ReadFile", "output_mode": "content", "line_number": true, "path": "src/tools/handlers/read_file.rs"}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("ReadFile"));
  }

  #[tokio::test]
  async fn test_grep_handler_no_matches() {
    let handler = GrepHandler::new();
    let cwd = PathBuf::from(".");
    // Use a pattern that is very unlikely to exist in the codebase
    let pattern = format!("NO_MATCH_{}_PATTERN_{}", std::process::id(), std::time::SystemTime::now().elapsed().unwrap().as_secs());
    let args = format!(r#"{{"pattern": "{}", "path": "src"}}"#, pattern);
    let invocation = ToolInvocation::new(
      "Grep",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: args,
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok(), "Handler returned error: {:?}", result);

    let output = result.unwrap().into_response();
    assert!(output.contains("No matches found"), "Expected 'No matches found' in output, got: {:?}", output);
  }

  #[tokio::test]
  async fn test_grep_handler_with_glob() {
    let handler = GrepHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Grep",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "ReadFile", "glob": "*.rs", "output_mode": "files_with_matches"}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());
  }

  #[tokio::test]
  async fn test_grep_handler_head_limit() {
    let handler = GrepHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Grep",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "use", "path": "src/tools", "output_mode": "content", "head_limit": 3}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    if !output.contains("No matches found") {
      // If there are results, check for truncation message
      assert!(output.contains("truncated") || output.lines().count() <= 4);
    }
  }
}
