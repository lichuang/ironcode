//! Hook configuration definitions.
//!
//! Hooks are user-defined shell commands registered in `config.toml` under
//! `[[hooks]]` and triggered at specific lifecycle events.
//!
//! Each hook specifies the event it listens to, a shell command to run, an
//! optional regex matcher to filter targets, and a timeout. The command
//! receives a JSON payload on stdin and communicates its decision via exit
//! code or structured stdout.

use serde::{Deserialize, Serialize};

/// Lifecycle event that can trigger a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEventType {
  /// Before a tool is executed.
  PreToolUse,
  /// After a tool succeeds.
  PostToolUse,
  /// After a tool fails.
  PostToolUseFailure,
  /// After the user submits a prompt but before it is sent to the LLM.
  UserPromptSubmit,
  /// On graceful stop (Ctrl+C or explicit exit).
  Stop,
  /// When graceful stop fails.
  StopFailure,
  /// When a session starts.
  SessionStart,
  /// When a session ends.
  SessionEnd,
  /// Before context compaction runs.
  PreCompact,
  /// After context compaction runs.
  PostCompact,
  /// When a notification is delivered to a sink.
  Notification,
  /// When a subagent starts.
  SubagentStart,
  /// When a subagent stops.
  SubagentStop,
}

impl std::fmt::Display for HookEventType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let s = match self {
      HookEventType::PreToolUse => "PreToolUse",
      HookEventType::PostToolUse => "PostToolUse",
      HookEventType::PostToolUseFailure => "PostToolUseFailure",
      HookEventType::UserPromptSubmit => "UserPromptSubmit",
      HookEventType::Stop => "Stop",
      HookEventType::StopFailure => "StopFailure",
      HookEventType::SessionStart => "SessionStart",
      HookEventType::SessionEnd => "SessionEnd",
      HookEventType::PreCompact => "PreCompact",
      HookEventType::PostCompact => "PostCompact",
      HookEventType::Notification => "Notification",
      HookEventType::SubagentStart => "SubagentStart",
      HookEventType::SubagentStop => "SubagentStop",
    };
    write!(f, "{}", s)
  }
}

/// A single server-side hook definition loaded from `config.toml`.
///
/// Server-side hooks are executed as local shell commands. The command
/// receives the event payload on stdin and may block the operation by
/// exiting with code 2 or by printing a structured `deny` decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDef {
  /// Which lifecycle event triggers this hook.
  pub event: HookEventType,
  /// Shell command to execute. Receives JSON on stdin.
  pub command: String,
  /// Optional regex pattern to filter targets.
  ///
  /// The value matched against depends on the event:
  /// - `PreToolUse` / `PostToolUse`: tool name
  /// - `UserPromptSubmit`: the prompt text
  /// - `Notification`: the notification's `event_type`
  /// - most others: empty string
  ///
  /// An empty pattern matches every target.
  #[serde(default)]
  pub matcher: String,
  /// Timeout in seconds. Fail-open on timeout.
  #[serde(default = "default_hook_timeout")]
  pub timeout: u64,
}

const fn default_hook_timeout() -> u64 {
  30
}

impl HookDef {
  /// Create a new hook definition.
  #[allow(dead_code)]
  pub fn new(event: HookEventType, command: impl Into<String>) -> Self {
    Self {
      event,
      command: command.into(),
      matcher: String::new(),
      timeout: default_hook_timeout(),
    }
  }

  /// Set the matcher regex.
  #[allow(dead_code)]
  pub fn with_matcher(mut self, matcher: impl Into<String>) -> Self {
    self.matcher = matcher.into();
    self
  }

  /// Set the timeout in seconds.
  #[allow(dead_code)]
  pub fn with_timeout(mut self, timeout: u64) -> Self {
    self.timeout = timeout;
    self
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_hook_event_type_display() {
    assert_eq!(HookEventType::PreToolUse.to_string(), "PreToolUse");
    assert_eq!(
      HookEventType::UserPromptSubmit.to_string(),
      "UserPromptSubmit"
    );
  }

  #[test]
  fn test_hook_def_defaults() {
    let def = HookDef::new(HookEventType::PreToolUse, "echo test");
    assert_eq!(def.timeout, 30);
    assert!(def.matcher.is_empty());
  }

  #[test]
  fn test_hook_def_builder() {
    let def = HookDef::new(HookEventType::UserPromptSubmit, "cat")
      .with_matcher("^hello")
      .with_timeout(10);
    assert_eq!(def.matcher, "^hello");
    assert_eq!(def.timeout, 10);
  }
}
