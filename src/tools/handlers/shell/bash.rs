//! Bash tool handler.
//!
//! Execute bash commands in a fresh shell environment.
//! Supports both foreground (blocking) and background execution.

use std::process::Stdio;
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{self, Duration};

use crate::background::BackgroundTaskManager;
use crate::background::models::TaskView;
use crate::tools::handlers::background::format_task;
use crate::tools::{
  ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, ToolPayload, parse_arguments,
};

/// Handler for the Bash tool
pub struct BashHandler {
  manager: Arc<BackgroundTaskManager>,
}

/// Maximum timeout for foreground commands (5 minutes).
const MAX_FOREGROUND_TIMEOUT: u64 = 5 * 60;
/// Maximum timeout for background commands (24 hours).
const MAX_BACKGROUND_TIMEOUT: u64 = 24 * 60 * 60;
/// Default timeout in seconds.
const DEFAULT_TIMEOUT: u64 = 60;

/// Arguments for the Bash tool
#[derive(Debug, Deserialize)]
struct BashArgs {
  /// The bash command to execute
  command: String,
  /// Timeout in seconds
  #[serde(default = "default_timeout")]
  timeout: u64,
  /// Whether to run as a background task
  #[serde(default)]
  run_in_background: bool,
  /// Description for the background task (required when run_in_background=true)
  #[serde(default)]
  description: String,
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

  async fn preview(&self, invocation: &ToolInvocation) -> Option<String> {
    let args = self.parse_args(invocation).ok()?;
    let cmd = args.command.trim();
    if cmd.is_empty() {
      return None;
    }
    if args.run_in_background {
      Some(format!("bash -c '{}' (background)", cmd))
    } else {
      Some(format!("bash -c '{}'", cmd))
    }
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation {
      payload,
      cwd,
      tool_call_id,
      ..
    } = invocation;

    let arguments = match payload {
      ToolPayload::Function { arguments } => arguments,
    };

    let args: BashArgs = parse_arguments(&arguments)?;

    // Validate command is not empty
    if args.command.trim().is_empty() {
      return Err(ToolError::RespondToModel(
        "Command cannot be empty.".to_string(),
      ));
    }

    // Validate background fields
    if args.run_in_background && args.description.trim().is_empty() {
      return Err(ToolError::RespondToModel(
        "description is required when run_in_background is true".to_string(),
      ));
    }

    if !args.run_in_background && args.timeout > MAX_FOREGROUND_TIMEOUT {
      return Err(ToolError::RespondToModel(format!(
        "timeout must be <= {}s for foreground commands; use run_in_background=true for longer timeouts (up to {}s)",
        MAX_FOREGROUND_TIMEOUT, MAX_BACKGROUND_TIMEOUT
      )));
    }

    if args.run_in_background {
      return self.run_in_background(&args, &tool_call_id, &cwd).await;
    }

    // Foreground execution
    let timeout = args.timeout.min(MAX_FOREGROUND_TIMEOUT);
    let output_result = execute_shell_command(&args.command, &cwd, timeout).await;

    match output_result {
      Ok((stdout, stderr, exit_code)) => {
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
            combined_output
          };
          Ok(ToolOutput::success(format!(
            "{}\n<system>Exit code: {}</system>",
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

impl BashHandler {
  /// Create a new BashHandler
  pub fn new(manager: Arc<BackgroundTaskManager>) -> Self {
    Self { manager }
  }

  fn parse_args(&self, invocation: &ToolInvocation) -> Result<BashArgs, ToolError> {
    let arguments = match &invocation.payload {
      ToolPayload::Function { arguments } => arguments.clone(),
    };
    parse_arguments(&arguments)
  }

  async fn run_in_background(
    &self,
    args: &BashArgs,
    tool_call_id: &str,
    cwd: &std::path::Path,
  ) -> Result<ToolOutput, ToolError> {
    let view = self
      .manager
      .create_bash_task(
        &args.command,
        &args.description,
        args.timeout.min(MAX_BACKGROUND_TIMEOUT),
        tool_call_id,
        "bash",
        "/bin/bash",
        &cwd.to_string_lossy(),
      )
      .map_err(|e| ToolError::RespondToModel(format!("Failed to start background task: {}", e)))?;

    let output = format_background_task_response(&view);
    Ok(ToolOutput::success(output))
  }
}

impl Default for BashHandler {
  fn default() -> Self {
    Self {
      manager: Arc::new(BackgroundTaskManager::new(
        std::path::PathBuf::from("."),
        crate::config::BackgroundConfig::default(),
      )),
    }
  }
}

fn format_background_task_response(view: &TaskView) -> String {
  let task_info = format_task(view, true);
  format!(
    "{}\n\nautomatic_notification: true\nnext_step: You will be automatically notified when it completes.\nnext_step: Use TaskOutput with this task_id for a non-blocking status/output snapshot. Only set block=true when you intentionally want to wait.\nnext_step: Use TaskStop only if the task must be cancelled.",
    task_info
  )
}

/// Execute a shell command with timeout
async fn execute_shell_command(
  command: &str,
  cwd: &std::path::Path,
  timeout_secs: u64,
) -> anyhow::Result<(String, String, i32)> {
  let mut cmd = Command::new("bash");
  cmd.arg("-c").arg(command).current_dir(cwd);

  cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

  let mut child = cmd.spawn()?;

  let stdout = child
    .stdout
    .take()
    .ok_or_else(|| anyhow!("Failed to capture stdout"))?;
  let stderr = child
    .stderr
    .take()
    .ok_or_else(|| anyhow!("Failed to capture stderr"))?;

  let stdout_reader = BufReader::new(stdout);
  let stderr_reader = BufReader::new(stderr);

  let mut stdout_lines = stdout_reader.lines();
  let mut stderr_lines = stderr_reader.lines();

  let mut stdout_output = String::new();
  let mut stderr_output = String::new();

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
            Err(e) => return Err(anyhow!("Error reading stdout: {}", e)),
          }
        }
        line = stderr_lines.next_line() => {
          match line {
            Ok(Some(l)) => {
              stderr_output.push_str(&l);
              stderr_output.push('\n');
            }
            Ok(None) => break,
            Err(e) => return Err(anyhow!("Error reading stderr: {}", e)),
          }
        }
        status = child.wait() => {
          let exit_code = status?.code().unwrap_or(-1);

          while let Ok(Some(l)) = stdout_lines.next_line().await {
            stdout_output.push_str(&l);
            stdout_output.push('\n');
          }

          while let Ok(Some(l)) = stderr_lines.next_line().await {
            stderr_output.push_str(&l);
            stderr_output.push('\n');
          }

          return Ok((stdout_output, stderr_output, exit_code));
        }
      }
    }

    let status = child.wait().await?;
    let exit_code = status.code().unwrap_or(-1);

    Ok((stdout_output, stderr_output, exit_code))
  })
  .await;

  match result {
    Ok(Ok((stdout, stderr, exit_code))) => Ok((stdout, stderr, exit_code)),
    Ok(Err(e)) => Err(e),
    Err(_) => {
      let _ = child.kill().await;
      Err(anyhow!("timeout"))
    }
  }
}

#[cfg(test)]
mod tests {
  use std::env;
  use std::fs;

  use super::*;

  #[test]
  fn test_parse_arguments() {
    let json = r#"{"command": "echo hello", "timeout": 30}"#;
    let args: BashArgs = parse_arguments(json).unwrap();

    assert_eq!(args.command, "echo hello");
    assert_eq!(args.timeout, 30);
    assert!(!args.run_in_background);
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
    let temp_dir = env::temp_dir();
    let manager = Arc::new(BackgroundTaskManager::new(temp_dir.clone(), crate::config::BackgroundConfig::default()));
    let handler = BashHandler::new(manager);
    let invocation = ToolInvocation::new(
      "Bash",
      "call-test",
      ToolPayload::Function {
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
    let temp_dir = env::temp_dir();
    let manager = Arc::new(BackgroundTaskManager::new(temp_dir.clone(), crate::config::BackgroundConfig::default()));
    let handler = BashHandler::new(manager);
    let invocation = ToolInvocation::new(
      "Bash",
      "call-test",
      ToolPayload::Function {
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
    let temp_dir = env::temp_dir();
    let manager = Arc::new(BackgroundTaskManager::new(temp_dir.clone(), crate::config::BackgroundConfig::default()));
    let handler = BashHandler::new(manager);
    let invocation = ToolInvocation::new(
      "Bash",
      "call-test",
      ToolPayload::Function {
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
    let temp_dir = env::temp_dir();
    let manager = Arc::new(BackgroundTaskManager::new(temp_dir.clone(), crate::config::BackgroundConfig::default()));
    let handler = BashHandler::new(manager);
    let invocation = ToolInvocation::new(
      "Bash",
      "call-test",
      ToolPayload::Function {
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
    let temp_dir = env::temp_dir();
    let test_dir = temp_dir.join("ironcode_bash_test_");
    let _ = fs::create_dir(&test_dir);

    let manager = Arc::new(BackgroundTaskManager::new(temp_dir.clone(), crate::config::BackgroundConfig::default()));
    let handler = BashHandler::new(manager);
    let invocation = ToolInvocation::new(
      "Bash",
      "call-test",
      ToolPayload::Function {
        arguments: r#"{"command": "pwd"}"#.to_string(),
      },
      &test_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_ok());

    // Cleanup
    let _ = fs::remove_dir(&test_dir);
  }
}
