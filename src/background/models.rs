//! Data models for background tasks.

use std::fmt;

use serde::{Deserialize, Serialize};

pub use crate::utils::time::Timestamp;
use crate::utils::time::now_secs;

// ---------------------------------------------------------------------------
// Task status
// ---------------------------------------------------------------------------

/// Status of a background task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
  /// Task has been created but worker has not started yet.
  Created,
  /// Worker process is starting.
  Starting,
  /// Worker is actively running the command.
  Running,
  /// Task is waiting for user approval (e.g., sub-agent approval).
  AwaitingApproval,
  /// Task completed successfully.
  Completed,
  /// Task failed (non-zero exit code or error).
  Failed,
  /// Task was explicitly stopped.
  Killed,
  /// Worker was lost (no heartbeat, process disappeared).
  Lost,
}

impl fmt::Display for TaskStatus {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let s = match self {
      TaskStatus::Created => "created",
      TaskStatus::Starting => "starting",
      TaskStatus::Running => "running",
      TaskStatus::AwaitingApproval => "awaiting_approval",
      TaskStatus::Completed => "completed",
      TaskStatus::Failed => "failed",
      TaskStatus::Killed => "killed",
      TaskStatus::Lost => "lost",
    };
    write!(f, "{}", s)
  }
}

/// Return true if the status is terminal (no further state changes expected).
pub fn is_terminal_status(status: TaskStatus) -> bool {
  matches!(
    status,
    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Killed | TaskStatus::Lost
  )
}

// ---------------------------------------------------------------------------
// Task spec
// ---------------------------------------------------------------------------

/// Parameters specific to a bash background task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashTaskParams {
  /// The shell command to execute.
  pub command: String,
  /// Shell name (e.g., "bash").
  pub shell_name: String,
  /// Absolute path to the shell binary.
  pub shell_path: String,
  /// Working directory for the command.
  pub cwd: String,
}

/// Parameters specific to an agent background task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskParams {
  /// Kind-specific payload (subagent launch spec).
  pub kind_payload: serde_json::Value,
  /// Configuration directory used to reconstruct the runtime.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub config_dir: Option<String>,
}

/// Kind of a background task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskSpecKind {
  Bash(BashTaskParams),
  Agent(AgentTaskParams),
}

impl TaskSpecKind {
  /// Return the canonical string representation.
  pub fn as_str(&self) -> &'static str {
    match self {
      Self::Bash(_) => "bash",
      Self::Agent(_) => "agent",
    }
  }

  /// Return true if this is a bash task.
  #[allow(dead_code)]
  pub fn is_bash(&self) -> bool {
    matches!(self, Self::Bash(_))
  }

  /// Return true if this is an agent task.
  pub fn is_agent(&self) -> bool {
    matches!(self, Self::Agent(_))
  }
}

impl std::fmt::Display for TaskSpecKind {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

/// Specification for a background task — immutable after creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
  pub version: i32,
  pub id: String,
  pub session_id: String,
  pub description: String,
  pub tool_call_id: String,
  /// Kind-specific parameters, flattened into the top-level JSON.
  #[serde(flatten)]
  pub kind: TaskSpecKind,
  /// Timeout in seconds (None = no timeout).
  pub timeout_s: Option<u64>,
  /// Creation timestamp (seconds since UNIX epoch).
  pub created_at: Timestamp,
}

impl TaskSpec {
  #[allow(clippy::too_many_arguments)]
  pub fn new(
    id: impl Into<String>,
    session_id: impl Into<String>,
    description: impl Into<String>,
    tool_call_id: impl Into<String>,
    command: impl Into<String>,
    shell_name: impl Into<String>,
    shell_path: impl Into<String>,
    cwd: impl Into<String>,
    timeout_s: Option<u64>,
  ) -> Self {
    Self {
      version: 1,
      id: id.into(),
      session_id: session_id.into(),
      description: description.into(),
      tool_call_id: tool_call_id.into(),
      kind: TaskSpecKind::Bash(BashTaskParams {
        command: command.into(),
        shell_name: shell_name.into(),
        shell_path: shell_path.into(),
        cwd: cwd.into(),
      }),
      timeout_s,
      created_at: now_secs(),
    }
  }

  /// Create an agent-kind task spec.
  #[allow(clippy::too_many_arguments)]
  pub fn new_agent(
    id: impl Into<String>,
    session_id: impl Into<String>,
    description: impl Into<String>,
    tool_call_id: impl Into<String>,
    timeout_s: Option<u64>,
    kind_payload: serde_json::Value,
    config_dir: Option<String>,
  ) -> Self {
    Self {
      version: 1,
      id: id.into(),
      session_id: session_id.into(),
      description: description.into(),
      tool_call_id: tool_call_id.into(),
      kind: TaskSpecKind::Agent(AgentTaskParams {
        kind_payload,
        config_dir,
      }),
      timeout_s,
      created_at: now_secs(),
    }
  }

  /// Return bash-specific parameters if this is a bash task.
  pub fn bash_params(&self) -> Option<&BashTaskParams> {
    match &self.kind {
      TaskSpecKind::Bash(params) => Some(params),
      _ => None,
    }
  }

  /// Return agent-specific parameters if this is an agent task.
  #[allow(dead_code)]
  pub fn agent_params(&self) -> Option<&AgentTaskParams> {
    match &self.kind {
      TaskSpecKind::Agent(params) => Some(params),
      _ => None,
    }
  }
}

impl Default for TaskSpec {
  fn default() -> Self {
    Self::new("", "", "", "", "", "", "", "", None)
  }
}

// ---------------------------------------------------------------------------
// Task runtime
// ---------------------------------------------------------------------------

/// Mutable runtime state for a background task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRuntime {
  pub status: TaskStatus,
  pub worker_pid: Option<u32>,
  pub child_pid: Option<u32>,
  pub started_at: Option<Timestamp>,
  pub heartbeat_at: Option<Timestamp>,
  pub updated_at: Timestamp,
  pub finished_at: Option<Timestamp>,
  pub exit_code: Option<i32>,
  pub interrupted: bool,
  pub timed_out: bool,
  pub failure_reason: Option<String>,
}

impl Default for TaskRuntime {
  fn default() -> Self {
    Self {
      status: TaskStatus::Created,
      worker_pid: None,
      child_pid: None,
      started_at: None,
      heartbeat_at: None,
      updated_at: now_secs(),
      finished_at: None,
      exit_code: None,
      interrupted: false,
      timed_out: false,
      failure_reason: None,
    }
  }
}

// ---------------------------------------------------------------------------
// Task control
// ---------------------------------------------------------------------------

/// Control signals written by the manager and read by the worker.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskControl {
  pub kill_requested_at: Option<Timestamp>,
  pub kill_reason: Option<String>,
  pub force: bool,
}

// ---------------------------------------------------------------------------
// Task consumer state
// ---------------------------------------------------------------------------

/// Tracks what the consumer (LLM / UI) has already seen.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskConsumerState {
  pub last_seen_output_size: usize,
  pub last_viewed_at: Option<Timestamp>,
}

// ---------------------------------------------------------------------------
// Task view
// ---------------------------------------------------------------------------

/// A merged view of all task state.
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
  pub spec: TaskSpec,
  pub runtime: TaskRuntime,
  #[allow(dead_code)]
  pub control: TaskControl,
  #[allow(dead_code)]
  pub consumer: TaskConsumerState,
}

// ---------------------------------------------------------------------------
// Task output chunk
// ---------------------------------------------------------------------------

/// A slice of task output returned by the store.
#[derive(Debug, Clone)]
pub struct TaskOutputChunk {
  #[allow(dead_code)]
  pub task_id: String,
  pub offset: usize,
  pub next_offset: usize,
  pub text: String,
  #[allow(dead_code)]
  pub eof: bool,
  #[allow(dead_code)]
  pub status: TaskStatus,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
