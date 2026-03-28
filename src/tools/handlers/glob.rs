//! Glob tool handler.
//!
//! Find files and directories using glob patterns.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Maximum number of matches to return
const MAX_MATCHES: usize = 1000;

/// Handler for the Glob tool
pub struct GlobHandler;

/// Arguments for the Glob tool
#[derive(Debug, Deserialize)]
struct GlobArgs {
  /// Glob pattern to match files/directories
  pattern: String,
  /// Absolute path to the directory to search in
  #[serde(default)]
  directory: Option<String>,
  /// Whether to include directories in results
  #[serde(default = "default_include_dirs")]
  include_dirs: bool,
}

fn default_include_dirs() -> bool {
  true
}

#[async_trait]
impl ToolHandler for GlobHandler {
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
          "Glob handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: GlobArgs = parse_arguments(&arguments)?;

    // Validate pattern safety - reject patterns starting with "**"
    if args.pattern.starts_with("**") {
      // List top-level directory contents for convenience
      let ls_result = list_directory(&cwd).await.unwrap_or_default();
      return Err(ToolError::RespondToModel(format!(
        "Pattern `{}` starts with '**' which is not allowed. \
         This would recursively search all directories and may include large \
         directories like `node_modules`. Use more specific patterns instead. \
         For your convenience, a list of all files and directories in the \
         top level of the working directory is provided below:\n\n{}",
        args.pattern, ls_result
      )));
    }

    // Determine search directory
    let search_dir = if let Some(dir) = &args.directory {
      let path = PathBuf::from(dir);
      if !path.is_absolute() {
        return Err(ToolError::RespondToModel(format!(
          "`{}` is not an absolute path. You must provide an absolute path to search.",
          dir
        )));
      }
      path
    } else {
      cwd.clone()
    };

    // Validate directory exists
    if !search_dir.exists() {
      return Err(ToolError::RespondToModel(format!(
        "`{}` does not exist.",
        search_dir.display()
      )));
    }
    if !search_dir.is_dir() {
      return Err(ToolError::RespondToModel(format!(
        "`{}` is not a directory.",
        search_dir.display()
      )));
    }

    // Perform glob search
    let pattern = &args.pattern;
    let glob_pattern = search_dir.join(pattern);
    let glob_str = glob_pattern.to_string_lossy();

    let matches: Vec<PathBuf> = match glob::glob(&glob_str) {
      Ok(paths) => paths
        .filter_map(|p| p.ok())
        .filter(|p| {
          // Filter out directories if not requested
          if args.include_dirs {
            true
          } else {
            p.is_file()
          }
        })
        .collect(),
      Err(e) => {
        return Err(ToolError::RespondToModel(format!(
          "Invalid glob pattern '{}': {}",
          pattern, e
        )));
      }
    };

    // Sort for consistent output
    let mut matches = matches;
    matches.sort();

    // Build message
    let match_count = matches.len();
    let mut message = if match_count > 0 {
      format!("Found {} matches for pattern `{}`.", match_count, pattern)
    } else {
      format!("No matches found for pattern `{}`.", pattern)
    };

    // Limit matches and update message
    let limited_matches: Vec<PathBuf> = if matches.len() > MAX_MATCHES {
      message.push_str(&format!(
        " Only the first {} matches are returned. You may want to use a more specific pattern.",
        MAX_MATCHES
      ));
      matches.into_iter().take(MAX_MATCHES).collect()
    } else {
      matches
    };

    // Format output as relative paths from search directory
    let output: Vec<String> = limited_matches
      .iter()
      .map(|p| {
        p.strip_prefix(&search_dir)
          .map(|r| r.to_string_lossy().to_string())
          .unwrap_or_else(|_| p.to_string_lossy().to_string())
      })
      .collect();

    if output.is_empty() {
      Ok(ToolOutput::success(message))
    } else {
      Ok(ToolOutput::success(format!("{}\n{}", message, output.join("\n"))))
    }
  }
}

/// List directory contents for error messages
async fn list_directory(dir: &PathBuf) -> Result<String, std::io::Error> {
  let mut entries = Vec::new();

  let mut read_dir = tokio::fs::read_dir(dir).await?;
  while let Some(entry) = read_dir.next_entry().await? {
    let name = entry.file_name().to_string_lossy().to_string();
    let metadata = entry.metadata().await?;
    let is_dir = metadata.is_dir();

    let prefix = if is_dir { "d" } else { "-" };
    entries.push(format!("{} {}", prefix, name));
  }

  entries.sort();
  Ok(entries.join("\n"))
}

impl GlobHandler {
  /// Create a new GlobHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for GlobHandler {
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
    let json = r#"{"pattern": "*.rs", "directory": "/tmp", "include_dirs": false}"#;
    let args: GlobArgs = parse_arguments(json).unwrap();

    assert_eq!(args.pattern, "*.rs");
    assert_eq!(args.directory, Some("/tmp".to_string()));
    assert!(!args.include_dirs);
  }

  #[test]
  fn test_parse_arguments_defaults() {
    let json = r#"{"pattern": "*.rs"}"#;
    let args: GlobArgs = parse_arguments(json).unwrap();

    assert_eq!(args.pattern, "*.rs");
    assert_eq!(args.directory, None);
    assert!(args.include_dirs);
  }

  #[tokio::test]
  async fn test_glob_handler_files_only() {
    let handler = GlobHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Glob",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "*.toml", "include_dirs": false}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("Cargo.toml"));
  }

  #[tokio::test]
  async fn test_glob_handler_recursive() {
    let handler = GlobHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Glob",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "src/**/*.rs"}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    // Should find Rust files in src directory
    assert!(output.contains("src/"));
  }

  #[tokio::test]
  async fn test_glob_handler_no_matches() {
    let handler = GlobHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Glob",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "*.nonexistent"}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("No matches found"));
  }

  #[tokio::test]
  async fn test_glob_handler_unsafe_pattern() {
    let handler = GlobHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Glob",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "**/*.rs"}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());

    let error = result.unwrap_err().to_string();
    assert!(error.contains("starts with '**'"));
  }

  #[tokio::test]
  async fn test_glob_handler_invalid_directory() {
    let handler = GlobHandler::new();
    let cwd = PathBuf::from(".");
    let invocation = ToolInvocation::new(
      "Glob",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"pattern": "*.rs", "directory": "/nonexistent/path/12345"}"#.to_string(),
      },
      &cwd,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());

    let error = result.unwrap_err().to_string();
    assert!(error.contains("does not exist"));
  }
}
