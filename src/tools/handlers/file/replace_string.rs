//! ReplaceFile tool handler.
//!
//! Replace specific strings within a specified file.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::fs;

use crate::tools::{ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, parse_arguments};

/// Handler for the ReplaceFile tool
pub struct ReplaceFileHandler;

/// A single edit operation
#[derive(Debug, Deserialize)]
struct Edit {
  /// The old string to replace
  old: String,
  /// The new string to replace with
  new: String,
  /// Whether to replace all occurrences
  #[serde(default)]
  replace_all: bool,
}

/// Arguments for the ReplaceFile tool
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplaceFileArgs {
  /// Path to the file to edit
  path: String,
  /// The edit(s) to apply
  #[serde(with = "edit_deserializer")]
  edit: Vec<Edit>,
}

/// Custom deserializer for edit field that can be a single edit or a list
mod edit_deserializer {
  use super::Edit;
  use serde::{Deserialize, Deserializer};

  pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Edit>, D::Error>
  where
    D: Deserializer<'de>,
  {
    let value: serde_json::Value = Deserialize::deserialize(deserializer)?;

    if let Some(array) = value.as_array() {
      // It's already an array
      let edits: Vec<Edit> = array
        .iter()
        .map(|v| serde_json::from_value(v.clone()).map_err(serde::de::Error::custom))
        .collect::<Result<_, _>>()?;
      Ok(edits)
    } else if value.is_object() {
      // It's a single edit object
      let edit: Edit = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
      Ok(vec![edit])
    } else {
      Err(serde::de::Error::custom(
        "edit must be an object or array of objects",
      ))
    }
  }
}

#[async_trait]
impl ToolHandler for ReplaceFileHandler {
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
          "ReplaceFile handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: ReplaceFileArgs = parse_arguments(&arguments)?;

    // Resolve path
    let path = PathBuf::from(&args.path);
    let resolved_path = if path.is_absolute() {
      path
    } else {
      cwd.join(&path)
    };

    // Check if file exists
    if !resolved_path.exists() {
      return Err(ToolError::RespondToModel(format!(
        "`{}` does not exist.",
        args.path
      )));
    }

    // Check if it's a file
    let metadata = fs::metadata(&resolved_path).await.map_err(|e| {
      ToolError::RespondToModel(format!("Failed to access file '{}': {}", args.path, e))
    })?;

    if !metadata.is_file() {
      return Err(ToolError::RespondToModel(format!(
        "`{}` is not a file.",
        args.path
      )));
    }

    // Read file content
    let content = fs::read_to_string(&resolved_path).await.map_err(|e| {
      ToolError::RespondToModel(format!("Failed to read file '{}': {}", args.path, e))
    })?;

    let original_content = content.clone();
    let mut total_replacements = 0;

    // Apply all edits
    let mut modified_content = content;
    for edit in &args.edit {
      let count = if edit.replace_all {
        let occurrences = modified_content.matches(&edit.old).count();
        modified_content = modified_content.replace(&edit.old, &edit.new);
        occurrences
      } else {
        if modified_content.contains(&edit.old) {
          modified_content = modified_content.replacen(&edit.old, &edit.new, 1);
          1
        } else {
          0
        }
      };
      total_replacements += count;
    }

    // Check if any changes were made
    if modified_content == original_content {
      return Err(ToolError::RespondToModel(
        "No replacements were made. The old string was not found in the file.".to_string(),
      ));
    }

    // Write the modified content back to the file
    fs::write(&resolved_path, modified_content)
      .await
      .map_err(|e| {
        ToolError::RespondToModel(format!("Failed to write file '{}': {}", args.path, e))
      })?;

    let message = format!(
      "File successfully edited. Applied {} edit(s) with {} total replacement(s).",
      args.edit.len(),
      total_replacements
    );

    Ok(ToolOutput::success(message))
  }
}

impl ReplaceFileHandler {
  /// Create a new ReplaceFileHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for ReplaceFileHandler {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  #[test]
  fn test_parse_arguments_single_edit() {
    let json = r#"{"path": "/tmp/test.txt", "edit": {"old": "foo", "new": "bar"}}"#;
    let args: ReplaceFileArgs = parse_arguments(json).unwrap();

    assert_eq!(args.path, "/tmp/test.txt");
    assert_eq!(args.edit.len(), 1);
    assert_eq!(args.edit[0].old, "foo");
    assert_eq!(args.edit[0].new, "bar");
    assert!(!args.edit[0].replace_all);
  }

  #[test]
  fn test_parse_arguments_multiple_edits() {
    let json = r#"{"path": "/tmp/test.txt", "edit": [{"old": "foo", "new": "bar"}, {"old": "baz", "new": "qux", "replace_all": true}]}"#;
    let args: ReplaceFileArgs = parse_arguments(json).unwrap();

    assert_eq!(args.path, "/tmp/test.txt");
    assert_eq!(args.edit.len(), 2);
    assert_eq!(args.edit[0].old, "foo");
    assert_eq!(args.edit[0].new, "bar");
    assert!(!args.edit[0].replace_all);
    assert_eq!(args.edit[1].old, "baz");
    assert_eq!(args.edit[1].new, "qux");
    assert!(args.edit[1].replace_all);
  }

  #[tokio::test]
  async fn test_str_replace_file_handler_single_edit() {
    // Create a temporary directory
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("ironcode_test_str_replace.txt");

    // Create test file
    fs::write(&test_file, "Hello World\nFoo Bar\n")
      .await
      .unwrap();

    let handler = ReplaceFileHandler::new();
    let invocation = ToolInvocation::new(
      "ReplaceFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "edit": {{"old": "World", "new": "Rust"}}}}"#,
          test_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("successfully edited"));
    assert!(output.contains("1 edit(s)"));
    assert!(output.contains("1 total replacement"));

    // Verify file content
    let content = fs::read_to_string(&test_file).await.unwrap();
    assert_eq!(content, "Hello Rust\nFoo Bar\n");

    // Cleanup
    fs::remove_file(&test_file).await.unwrap();
  }

  #[tokio::test]
  async fn test_str_replace_file_handler_replace_all() {
    // Create a temporary directory
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("ironcode_test_str_replace_all.txt");

    // Create test file with multiple occurrences
    fs::write(&test_file, "foo bar foo baz foo\n")
      .await
      .unwrap();

    let handler = ReplaceFileHandler::new();
    let invocation = ToolInvocation::new(
      "ReplaceFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "edit": {{"old": "foo", "new": "qux", "replace_all": true}}}}"#,
          test_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("successfully edited"));
    assert!(output.contains("3 total replacement"));

    // Verify file content
    let content = fs::read_to_string(&test_file).await.unwrap();
    assert_eq!(content, "qux bar qux baz qux\n");

    // Cleanup
    fs::remove_file(&test_file).await.unwrap();
  }

  #[tokio::test]
  async fn test_str_replace_file_handler_multiple_edits() {
    // Create a temporary directory
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("ironcode_test_str_replace_multi.txt");

    // Create test file
    fs::write(&test_file, "Hello World\nFoo Bar\n")
      .await
      .unwrap();

    let handler = ReplaceFileHandler::new();
    let invocation = ToolInvocation::new(
      "ReplaceFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "edit": [{{"old": "Hello", "new": "Hi"}}, {{"old": "Bar", "new": "Baz"}}]}}"#,
          test_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("successfully edited"));
    assert!(output.contains("2 edit(s)"));

    // Verify file content
    let content = fs::read_to_string(&test_file).await.unwrap();
    assert_eq!(content, "Hi World\nFoo Baz\n");

    // Cleanup
    fs::remove_file(&test_file).await.unwrap();
  }

  #[tokio::test]
  async fn test_str_replace_file_handler_no_match() {
    // Create a temporary directory
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("ironcode_test_str_replace_no_match.txt");

    // Create test file
    fs::write(&test_file, "Hello World\n").await.unwrap();

    let handler = ReplaceFileHandler::new();
    let invocation = ToolInvocation::new(
      "ReplaceFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "edit": {{"old": "NonExistent", "new": "Replacement"}}}}"#,
          test_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());

    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("No replacements were made"));

    // Cleanup
    fs::remove_file(&test_file).await.unwrap();
  }

  #[tokio::test]
  async fn test_str_replace_file_handler_file_not_found() {
    let temp_dir = std::env::temp_dir();
    let non_existent_file = temp_dir.join("non_existent_file_12345.txt");

    let handler = ReplaceFileHandler::new();
    let invocation = ToolInvocation::new(
      "ReplaceFile",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: format!(
          r#"{{"path": "{}", "edit": {{"old": "foo", "new": "bar"}}}}"#,
          non_existent_file.display()
        ),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());

    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("does not exist"));
  }
}
