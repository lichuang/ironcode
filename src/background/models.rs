//! Data models for background tasks.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Unix timestamp in seconds.
pub type Timestamp = u64;

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

/// Specification for a background task — immutable after creation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskSpec {
  pub version: i32,
  pub id: String,
  pub kind: String,
  pub session_id: String,
  pub description: String,
  pub tool_call_id: String,
  /// The shell command to execute (bash-only for now).
  pub command: String,
  /// Shell name (e.g., "bash").
  pub shell_name: String,
  /// Absolute path to the shell binary.
  pub shell_path: String,
  /// Working directory for the command.
  pub cwd: String,
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
      kind: "bash".to_string(),
      session_id: session_id.into(),
      description: description.into(),
      tool_call_id: tool_call_id.into(),
      command: command.into(),
      shell_name: shell_name.into(),
      shell_path: shell_path.into(),
      cwd: cwd.into(),
      timeout_s,
      created_at: now_secs(),
    }
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

fn now_secs() -> Timestamp {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
