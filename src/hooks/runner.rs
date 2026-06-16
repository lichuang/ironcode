//! Hook command execution.
//!
//! Hooks are shell commands that receive a JSON payload on stdin and may
//! produce a structured decision on stdout. Exit code 2 means "block";
//! exit 0 with JSON `{ "hookSpecificOutput": { "permissionDecision": "deny" } }`
//! also blocks.

use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

/// Decision produced by a hook execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecision {
  /// Allow the operation to continue.
  Allow,
  /// Block the operation with a reason.
  Block { reason: String },
}

/// Result of executing a single hook.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HookResult {
  /// Final decision.
  pub decision: HookDecision,
  /// Raw stdout from the command.
  pub stdout: String,
  /// Raw stderr from the command.
  pub stderr: String,
  /// Exit code, if the process completed.
  pub exit_code: Option<i32>,
  /// Whether the hook timed out.
  pub timed_out: bool,
}

impl HookResult {
  /// Create a default allow result.
  pub fn allow() -> Self {
    Self {
      decision: HookDecision::Allow,
      stdout: String::new(),
      stderr: String::new(),
      exit_code: Some(0),
      timed_out: false,
    }
  }

  /// Create a block result.
  #[allow(dead_code)]
  pub fn block(reason: impl Into<String>) -> Self {
    Self {
      decision: HookDecision::Block {
        reason: reason.into(),
      },
      stdout: String::new(),
      stderr: String::new(),
      exit_code: Some(2),
      timed_out: false,
    }
  }
}

/// Run a single hook command.
///
/// The command receives `input_data` as JSON on stdin. Failures (spawn errors,
/// non-zero exits without block semantics, timeouts) are returned as `Allow`
/// with diagnostic fields set.
pub async fn run_hook(
  command: &str,
  input_data: &Value,
  timeout_s: u64,
  cwd: Option<&str>,
) -> HookResult {
  let input_json = match serde_json::to_vec(input_data) {
    Ok(v) => v,
    Err(e) => {
      log::warn!("Failed to serialize hook input: {}", e);
      return HookResult::allow();
    }
  };

  let mut cmd = Command::new("sh");
  cmd
    .arg("-c")
    .arg(command)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  if let Some(cwd) = cwd {
    cmd.current_dir(cwd);
  }

  let mut child = match cmd.spawn() {
    Ok(c) => c,
    Err(e) => {
      log::warn!("Failed to spawn hook '{}': {}", command, e);
      return HookResult::allow();
    }
  };

  // Feed stdin in a background task so wait_with_output can consume stdout/stderr.
  if let Some(mut stdin) = child.stdin.take() {
    tokio::spawn(async move {
      let _ = stdin.write_all(&input_json).await;
      let _ = stdin.shutdown().await;
    });
  }

  let result = timeout(Duration::from_secs(timeout_s), child.wait_with_output()).await;

  match result {
    Ok(Ok(output)) => parse_hook_output(output),
    Ok(Err(e)) => {
      log::warn!("Hook process error '{}': {}", command, e);
      HookResult::allow()
    }
    Err(_) => {
      log::warn!("Hook timed out after {}s: {}", timeout_s, command);
      HookResult {
        decision: HookDecision::Allow,
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        timed_out: true,
      }
    }
  }
}

fn parse_hook_output(output: std::process::Output) -> HookResult {
  let stdout = String::from_utf8_lossy(&output.stdout).to_string();
  let stderr = String::from_utf8_lossy(&output.stderr).to_string();
  let exit_code = output.status.code();

  // Exit 2 = explicit block (stderr as reason).
  if exit_code == Some(2) {
    return HookResult {
      decision: HookDecision::Block {
        reason: stderr.trim().to_string(),
      },
      stdout: stdout.clone(),
      stderr: stderr.clone(),
      exit_code,
      timed_out: false,
    };
  }

  // Exit 0 + JSON stdout = structured decision.
  if exit_code == Some(0)
    && !stdout.trim().is_empty()
    && let Ok(value) = serde_json::from_str::<Value>(&stdout)
    && let Some(hook_output) = value.get("hookSpecificOutput")
    && hook_output
      .get("permissionDecision")
      .and_then(|d| d.as_str())
      == Some("deny")
  {
    let reason = hook_output
      .get("permissionDecisionReason")
      .and_then(|r| r.as_str())
      .unwrap_or("")
      .to_string();
    return HookResult {
      decision: HookDecision::Block { reason },
      stdout: stdout.clone(),
      stderr: stderr.clone(),
      exit_code,
      timed_out: false,
    };
  }

  HookResult {
    decision: HookDecision::Allow,
    stdout,
    stderr,
    exit_code,
    timed_out: false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[tokio::test]
  async fn test_run_hook_allow() {
    let input = json!({"tool_name": "ReadFile"});
    let result = run_hook("cat", &input, 5, None).await;
    assert_eq!(result.decision, HookDecision::Allow);
    assert!(result.stdout.contains("ReadFile"));
  }

  #[tokio::test]
  async fn test_run_hook_block_exit_2() {
    let input = json!({});
    let result = run_hook("echo 'blocked' >&2; exit 2", &input, 5, None).await;
    assert_eq!(
      result.decision,
      HookDecision::Block {
        reason: "blocked".to_string()
      }
    );
  }

  #[tokio::test]
  async fn test_run_hook_block_json_deny() {
    let input = json!({});
    let result = run_hook(
      r#"echo '{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"no"}}'"#,
      &input,
      5,
      None,
    )
    .await;
    assert_eq!(
      result.decision,
      HookDecision::Block {
        reason: "no".to_string()
      }
    );
  }

  #[tokio::test]
  async fn test_run_hook_timeout() {
    let input = json!({});
    let result = run_hook("sleep 10", &input, 1, None).await;
    assert!(result.timed_out);
    assert_eq!(result.decision, HookDecision::Allow);
  }
}
