//! Bash tool handler.
//!
//! Execute bash commands in a fresh shell environment.

use std::process::Stdio;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{self, Duration};

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Handler for the Bash tool
pub struct BashHandler;

/// Maximum timeout in seconds
const MAX_TIMEOUT: u64 = 300;

/// Default timeout in seconds
const DEFAULT_TIMEOUT: u64 = 60;

/// Arguments for the Bash tool
#[derive(Debug, Deserialize)]
struct BashArgs {
  /// The bash command to execute
  command: String,
  /// Timeout in seconds
  #[serde(default = "default_timeout")]
  timeout: u64,
}

fn default_timeout() -> u64 {
  DEFAULT_TIMEOUT
}

#[async_trait]
impl ToolHandler for BashHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    // Bash commands may modify files/system
    true
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, cwd, .. } = invocation;

    // Extract arguments from payload
    let arguments = match payload {
      crate::tools::ToolPayload::Function { arguments } => arguments,
      _ => {
        return Err(ToolError::RespondToModel(
          "Bash handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: BashArgs = parse_arguments(&arguments)?;

    // Validate command is not empty
    if args.command.trim().is_empty() {
      return Err(ToolError::RespondToModel("Command cannot be empty.".to_string()));
    }

    // Validate timeout
    let timeout = args.timeout.min(MAX_TIMEOUT);

    // Execute the command
    let output_result = execute_shell_command(&args.command, &cwd, timeout).await;

    match output_result {
      Ok((stdout, stderr, exit_code)) => {
        // Combine stdout and stderr
        let mut combined_output = String::new();
        if !stdout.is_empty() {
          combined_output.push_str(&stdout);
        }
        if !stderr.is_empty() {
          if !combined_output.is_empty() && !combined_output.ends_with('\n') {
            combined_output.push('\n');
          }
          combined_output.push_str(&stderr);
        }

        if exit_code == 0 {
          Ok(ToolOutput::success(combined_output))
        } else {
          let message = if combined_output.is_empty() {
            format!("Command failed with exit code: {}", exit_code)
          } else {
            format!("{}", combined_output)
          };
          Ok(ToolOutput::success(format!(
            "{}
<system>Exit code: {}</system>",
            message, exit_code
          )))
        }
      }
      Err(e) => {
        if e.to_string().contains("timeout") {
          Err(ToolError::RespondToModel(format!(
            "Command killed by timeout ({}s)",
            timeout
          )))
        } else {
          Err(ToolError::RespondToModel(format!(
            "Failed to execute command: {}",
            e
          )))
        }
      }
    }
  }
}

/// Execute a shell command with timeout
async fn execute_shell_command(
  command: &str,
  cwd: &std::path::Path,
  timeout_secs: u64,
) -> anyhow::Result<(String, String, i32)> {
  let mut cmd = Command::new("bash");
  cmd.arg("-c").arg(command).current_dir(cwd);

  // Set up pipes for stdout and stderr
  cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

  // Spawn the process
  let mut child = cmd.spawn()?;

  let stdout = child
    .stdout
    .take()
    .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
  let stderr = child
    .stderr
    .take()
    .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

  // Create buffered readers
  let stdout_reader = BufReader::new(stdout);
  let stderr_reader = BufReader::new(stderr);

  let mut stdout_lines = stdout_reader.lines();
  let mut stderr_lines = stderr_reader.lines();

  let mut stdout_output = String::new();
  let mut stderr_output = String::new();

  // Read output with timeout
  let result = time::timeout(Duration::from_secs(timeout_secs), async {
    loop {
      tokio::select! {
        line = stdout_lines.next_line() => {
          match line {
            Ok(Some(l)) => {
              stdout_output.push_str(&l);
              stdout_output.push('\n');
            }
            Ok(None) => break,
            Err(e) => return Err(anyhow::anyhow!("Error reading stdout: {}", e)),
          }
        }
        line = stderr_lines.next_line() => {
          match line {
            Ok(Some(l)) => {
              stderr_output.push_str(&l);
              stderr_output.push('\n');
            }
            Ok(None) => break,
            Err(e) => return Err(anyhow::anyhow!("Error reading stderr: {}", e)),
          }
        }
        status = child.wait() => {
          // Process finished, drain remaining output
          let exit_code = status?.code().unwrap_or(-1);

          // Drain remaining stdout
          while let Ok(Some(l)) = stdout_lines.next_line().await {
            stdout_output.push_str(&l);
            stdout_output.push('\n');
          }

          // Drain remaining stderr
          while let Ok(Some(l)) = stderr_lines.next_line().await {
            stderr_output.push_str(&l);
            stderr_output.push('\n');
          }

          return Ok((stdout_output, stderr_output, exit_code));
        }
      }
    }

    // If we reach here, both stdout and stderr are closed
    let status = child.wait().await?;
    let exit_code = status.code().unwrap_or(-1);

    Ok((stdout_output, stderr_output, exit_code))
  })
  .await;

  match result {
    Ok(Ok((stdout, stderr, exit_code))) => Ok((stdout, stderr, exit_code)),
    Ok(Err(e)) => Err(e),
    Err(_) => {
      // Timeout - kill the process
      let _ = child.kill().await;
      Err(anyhow::anyhow!("timeout"))
    }
  }
}

impl BashHandler {
  /// Create a new BashHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for BashHandler {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_arguments() {
    let json = r#"{"command": "echo hello", "timeout": 30}"#;
    let args: BashArgs = parse_arguments(json).unwrap();

    assert_eq!(args.command, "echo hello");
    assert_eq!(args.timeout, 30);
  }

  #[test]
  fn test_parse_arguments_defaults() {
    let json = r#"{"command": "ls"}"#;
    let args: BashArgs = parse_arguments(json).unwrap();

    assert_eq!(args.command, "ls");
    assert_eq!(args.timeout, DEFAULT_TIMEOUT);
  }

  #[tokio::test]
  async fn test_bash_handler_echo() {
    let temp_dir = std::env::temp_dir();
    let handler = BashHandler::new();
    let invocation = ToolInvocation::new(
      "Bash",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"command": "echo 'Hello World'"}"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("Hello World"));
  }

  #[tokio::test]
  async fn test_bash_handler_exit_code() {
    let temp_dir = std::env::temp_dir();
    let handler = BashHandler::new();
    let invocation = ToolInvocation::new(
      "Bash",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"command": "exit 42"}"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("Exit code: 42"));
  }

  #[tokio::test]
  async fn test_bash_handler_empty_command() {
    let temp_dir = std::env::temp_dir();
    let handler = BashHandler::new();
    let invocation = ToolInvocation::new(
      "Bash",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"command": "   "}"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());

    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("cannot be empty"));
  }

  #[tokio::test]
  async fn test_bash_handler_chained_commands() {
    let temp_dir = std::env::temp_dir();
    let handler = BashHandler::new();
    let invocation = ToolInvocation::new(
      "Bash",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"command": "echo 'line1' && echo 'line2'"}"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    let output = result.unwrap().into_response();
    assert!(output.contains("line1"));
    assert!(output.contains("line2"));
  }

  #[tokio::test]
  async fn test_bash_handler_working_directory() {
    let temp_dir = std::env::temp_dir();
    let test_dir = temp_dir.join("ironcode_bash_test_");
    let _ = std::fs::create_dir(&test_dir);

    let handler = BashHandler::new();
    let invocation = ToolInvocation::new(
      "Bash",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"command": "pwd"}"#.to_string(),
      },
      &test_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    // Cleanup
    let _ = std::fs::remove_dir(&test_dir);
  }
}
