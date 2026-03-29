//! PowerShell tool handler.
//!
//! Execute PowerShell commands in a fresh shell environment (Windows only).

use std::process::Stdio;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{self, Duration};

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Handler for the PowerShell tool
pub struct PowerShellHandler;

/// Maximum timeout in seconds
const MAX_TIMEOUT: u64 = 300;

/// Default timeout in seconds
const DEFAULT_TIMEOUT: u64 = 60;

/// Arguments for the PowerShell tool
#[derive(Debug, Deserialize)]
struct PowerShellArgs {
  /// The PowerShell command to execute
  command: String,
  /// Timeout in seconds
  #[serde(default = "default_timeout")]
  timeout: u64,
}

fn default_timeout() -> u64 {
  DEFAULT_TIMEOUT
}

#[async_trait]
impl ToolHandler for PowerShellHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    // PowerShell commands may modify files/system
    true
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, cwd, .. } = invocation;

    // Extract arguments from payload
    let arguments = match payload {
      crate::tools::ToolPayload::Function { arguments } => arguments,
      _ => {
        return Err(ToolError::RespondToModel(
          "PowerShell handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: PowerShellArgs = parse_arguments(&arguments)?;

    // Validate command is not empty
    if args.command.trim().is_empty() {
      return Err(ToolError::RespondToModel("Command cannot be empty.".to_string()));
    }

    // Validate timeout
    let timeout = args.timeout.min(MAX_TIMEOUT);

    // Execute the command
    let output_result = execute_powershell_command(&args.command, &cwd, timeout).await;

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

/// Execute a PowerShell command with timeout
async fn execute_powershell_command(
  command: &str,
  cwd: &std::path::Path,
  timeout_secs: u64,
) -> anyhow::Result<(String, String, i32)> {
  let mut cmd = Command::new("powershell.exe");
  cmd.arg("-Command").arg(command).current_dir(cwd);

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

impl PowerShellHandler {
  /// Create a new PowerShellHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for PowerShellHandler {
  fn default() -> Self {
    Self::new()
  }
}
