//! Background task manager — creation, lifecycle, and querying.

use std::env;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{info, warn};
use tokio::time::sleep;

use crate::config::BackgroundConfig;

use super::ids::generate_task_id;
use super::models::{
  TaskOutputChunk, TaskSpec, TaskStatus, TaskView, Timestamp, is_terminal_status,
};
use super::store::BackgroundTaskStore;
use std::time::Duration;

/// Manager for background tasks.
///
/// Tasks are stored under the session's `tasks/` directory and survive the
/// main CLI process exit because they run in independent worker processes.
#[derive(Debug)]
pub struct BackgroundTaskManager {
  data_dir: PathBuf,
  session_id: std::sync::Mutex<Option<String>>,
  config: BackgroundConfig,
}

impl BackgroundTaskManager {
  /// Create a new manager bound to the given data directory.
  ///
  /// The actual task store path is resolved once `bind_session` is called.
  pub fn new(data_dir: PathBuf, config: BackgroundConfig) -> Self {
    Self {
      data_dir,
      session_id: std::sync::Mutex::new(None),
      config,
    }
  }

  /// Return a reference to the background task configuration.
  pub fn config(&self) -> &BackgroundConfig {
    &self.config
  }

  /// Bind the manager to a specific session.
  pub fn bind_session(&self, session_id: &str) {
    *self.session_id.lock().unwrap() = Some(session_id.to_string());
  }

  fn store(&self) -> Option<BackgroundTaskStore> {
    let session_id = self.session_id.lock().unwrap().clone()?;
    let tasks_dir = self
      .data_dir
      .join("sessions")
      .join(session_id)
      .join("tasks");
    Some(BackgroundTaskStore::new(tasks_dir))
  }

  /// Return the canonical output path for a task.
  pub fn output_path(&self, task_id: &str) -> Option<std::path::PathBuf> {
    Some(self.store()?.output_path(task_id))
  }

  // -------------------------------------------------------------------------
  // Task creation
  // -------------------------------------------------------------------------

  /// Create a new bash background task.
  #[allow(clippy::too_many_arguments)]
  pub fn create_bash_task(
    &self,
    command: &str,
    description: &str,
    timeout_s: u64,
    tool_call_id: &str,
    shell_name: &str,
    shell_path: &str,
    cwd: &str,
  ) -> Result<TaskView, String> {
    let store = self
      .store()
      .ok_or("Background tasks not available: no session bound")?;

    let session_id = self.session_id.lock().unwrap().clone().unwrap();
    let task_id = generate_task_id("bash");

    let spec = TaskSpec::new(
      &task_id,
      &session_id,
      description,
      tool_call_id,
      command,
      shell_name,
      shell_path,
      cwd,
      Some(timeout_s),
    );

    store.create_task(&spec);
    info!("Created background task {}: {}", task_id, description);

    // Launch worker process
    let task_dir = store.task_dir(&task_id);
    match self.launch_worker(&task_dir) {
      Ok(worker_pid) => {
        let mut runtime = store.read_runtime(&task_id);
        if runtime.finished_at.is_none()
          && (runtime.status == TaskStatus::Created
            || (runtime.status == TaskStatus::Starting && runtime.worker_pid.is_none()))
        {
          runtime.status = TaskStatus::Starting;
          runtime.worker_pid = Some(worker_pid);
          runtime.updated_at = now_secs();
          store.write_runtime(&task_id, &runtime);
        }
      }
      Err(e) => {
        let mut runtime = store.read_runtime(&task_id);
        runtime.status = TaskStatus::Failed;
        runtime.failure_reason = Some(format!("Failed to launch worker: {}", e));
        runtime.finished_at = Some(now_secs());
        runtime.updated_at = runtime.finished_at.unwrap();
        store.write_runtime(&task_id, &runtime);
        return Err(format!("Failed to launch worker: {}", e));
      }
    }

    Ok(store.merged_view(&task_id).unwrap())
  }

  fn launch_worker(&self, task_dir: &PathBuf) -> Result<u32, std::io::Error> {
    let exe = env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd
      .arg("--background-task-worker")
      .arg(task_dir.as_os_str())
      .arg("--worker-heartbeat-interval-ms")
      .arg(self.config.worker_heartbeat_interval_ms.to_string())
      .arg("--worker-control-poll-interval-ms")
      .arg(self.config.wait_poll_interval_ms.to_string())
      .arg("--worker-kill-grace-period-ms")
      .arg(self.config.kill_grace_period_ms.to_string())
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .current_dir(task_dir);

    #[cfg(target_os = "windows")]
    {
      use std::os::windows::process::CommandExt;
      const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
      cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(not(target_os = "windows"))]
    {
      // This is only available on Unix; we guard it.
      #[cfg(unix)]
      {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
      }
    }

    let child = cmd.spawn()?;
    Ok(child.id())
  }

  // -------------------------------------------------------------------------
  // Querying
  // -------------------------------------------------------------------------

  /// List tasks, optionally filtering by status and limiting results.
  pub fn list_tasks(&self, active_only: bool, limit: usize) -> Result<Vec<TaskView>, String> {
    let store = self.store().ok_or("Background tasks not available")?;
    let mut views = store.list_views();
    if active_only {
      views.retain(|v| !is_terminal_status(v.runtime.status));
    }
    views.truncate(limit);
    Ok(views)
  }

  /// Get a single task by ID.
  pub fn get_task(&self, task_id: &str) -> Option<TaskView> {
    self.store()?.merged_view(task_id)
  }

  /// Read output from a task.
  pub fn read_output(
    &self,
    task_id: &str,
    offset: usize,
    max_bytes: usize,
  ) -> Option<TaskOutputChunk> {
    let store = self.store()?;
    let status = store.read_runtime(task_id).status;
    Some(store.read_output(task_id, offset, max_bytes, status))
  }

  /// Tail output from a task.
  #[allow(dead_code)]
  pub fn tail_output(&self, task_id: &str, max_bytes: usize, max_lines: usize) -> Option<String> {
    let store = self.store()?;
    Some(store.tail_output(task_id, max_bytes, max_lines))
  }

  /// Wait for a task to reach a terminal status (or timeout).
  pub async fn wait(&self, task_id: &str, timeout_s: u64) -> Option<TaskView> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_s);
    let poll_interval = Duration::from_millis(self.config.wait_poll_interval_ms);
    loop {
      let view = self.get_task(task_id)?;
      if is_terminal_status(view.runtime.status) {
        return Some(view);
      }
      if tokio::time::Instant::now() >= deadline {
        return Some(view);
      }
      sleep(poll_interval).await;
    }
  }

  // -------------------------------------------------------------------------
  // Killing
  // -------------------------------------------------------------------------

  /// Stop a running background task.
  pub fn kill(&self, task_id: &str, reason: &str) -> Result<TaskView, String> {
    let store = self.store().ok_or("Background tasks not available")?;

    let view = store
      .merged_view(task_id)
      .ok_or_else(|| format!("Task not found: {}", task_id))?;

    if is_terminal_status(view.runtime.status) {
      return Ok(view);
    }

    // Write kill request to control file
    let mut control = store.read_control(task_id);
    control.kill_requested_at = Some(now_secs());
    control.kill_reason = Some(reason.to_string());
    control.force = false;
    store.write_control(task_id, &control);

    // Best-effort signal the child process directly
    if let Some(child_pid) = view.runtime.child_pid {
      best_effort_kill(child_pid);
    }

    // If the worker never heartbeated, mark it lost immediately.
    let runtime = store.read_runtime(task_id);
    if runtime.heartbeat_at.is_none() && runtime.started_at.is_none() {
      let mut runtime = runtime;
      runtime.status = TaskStatus::Killed;
      runtime.interrupted = true;
      runtime.finished_at = Some(now_secs());
      runtime.updated_at = runtime.finished_at.unwrap();
      runtime.failure_reason = Some(reason.to_string());
      store.write_runtime(task_id, &runtime);
    }

    Ok(store.merged_view(task_id).unwrap())
  }

  /// Kill all non-terminal tasks. Returns the IDs of tasks that were killed.
  pub fn kill_all_active(&self, reason: &str) -> Vec<String> {
    let Some(store) = self.store() else {
      return Vec::new();
    };
    let mut killed = Vec::new();
    for view in store.list_views() {
      if is_terminal_status(view.runtime.status) {
        continue;
      }
      if self.kill(&view.spec.id, reason).is_ok() {
        killed.push(view.spec.id);
      }
    }
    killed
  }

  // -------------------------------------------------------------------------
  // Recovery
  // -------------------------------------------------------------------------

  /// Scan for stale tasks and mark them lost.
  ///
  /// Called on app restart to reconnect with the on-disk task state.
  /// Returns the number of tasks that were updated.
  pub fn recover(&self) -> usize {
    let stale_after_s = self.config.worker_stale_after_ms / 1000;
    let Some(store) = self.store() else {
      return 0;
    };
    let now = now_secs();
    let mut updated = 0;
    for view in store.list_views() {
      if is_terminal_status(view.runtime.status) {
        continue;
      }

      // Fast-path: if the worker or child PID is known and dead, mark lost immediately.
      let pid_dead = view
        .runtime
        .worker_pid
        .or(view.runtime.child_pid)
        .is_some_and(|pid| !is_process_alive(pid));

      if !pid_dead {
        let last_progress = view
          .runtime
          .heartbeat_at
          .or(view.runtime.started_at)
          .or(Some(view.runtime.updated_at))
          .or(Some(view.spec.created_at))
          .unwrap_or(now);

        if now - last_progress <= stale_after_s {
          continue;
        }
      }

      // Re-read runtime to narrow race window
      let fresh = store.read_runtime(&view.spec.id);
      if is_terminal_status(fresh.status) {
        continue;
      }

      if !pid_dead {
        let fresh_progress = fresh
          .heartbeat_at
          .or(fresh.started_at)
          .or(Some(fresh.updated_at))
          .or(Some(view.spec.created_at))
          .unwrap_or(now);

        if now - fresh_progress <= stale_after_s {
          continue;
        }
      }

      let mut runtime = fresh;
      runtime.finished_at = Some(now);
      runtime.updated_at = now;
      let control = store.read_control(&view.spec.id);
      if control.kill_requested_at.is_some() {
        runtime.status = TaskStatus::Killed;
        runtime.interrupted = true;
        runtime.failure_reason = control
          .kill_reason
          .clone()
          .or_else(|| Some("Killed during recovery".to_string()));
      } else {
        runtime.status = TaskStatus::Lost;
        runtime.failure_reason = if pid_dead {
          Some("Background worker process exited".to_string())
        } else if runtime.heartbeat_at.is_none() {
          Some("Background worker never heartbeat after startup".to_string())
        } else {
          Some("Background worker heartbeat expired".to_string())
        };
      }
      store.write_runtime(&view.spec.id, &runtime);
      warn!(
        "Marked background task {} as {} during recovery",
        view.spec.id, runtime.status
      );
      updated += 1;
    }
    updated
  }

  /// Reconcile on-disk task state with the UI.
  ///
  /// Runs `recover()`, publishes terminal-task notifications, and returns
  /// the terminal tasks.
  pub fn reconcile(
    &self,
    notification_manager: &crate::notification::NotificationManager,
  ) -> Vec<TaskView> {
    let _updated = self.recover();
    let terminal: Vec<TaskView> = self
      .list_tasks(false, usize::MAX)
      .unwrap_or_default()
      .into_iter()
      .filter(|v| is_terminal_status(v.runtime.status))
      .collect();
    for view in &terminal {
      publish_terminal_notification(notification_manager, view);
    }
    terminal
  }
}

// ---------------------------------------------------------------------------
// Notification helpers
// ---------------------------------------------------------------------------

fn publish_terminal_notification(
  manager: &crate::notification::NotificationManager,
  view: &TaskView,
) {
  use crate::notification::models::{NotificationEvent, NotificationSeverity};

  let status = view.runtime.status;
  let status_str = status.to_string();
  let terminal_reason: &str = if view.runtime.timed_out {
    "timed_out"
  } else {
    status_str.as_str()
  };

  let (severity, title) = match terminal_reason {
    "completed" => (
      NotificationSeverity::Success,
      format!("Background task completed: {}", view.spec.description),
    ),
    "timed_out" => (
      NotificationSeverity::Error,
      format!("Background task timed out: {}", view.spec.description),
    ),
    "failed" => (
      NotificationSeverity::Error,
      format!("Background task failed: {}", view.spec.description),
    ),
    "killed" => (
      NotificationSeverity::Warning,
      format!("Background task stopped: {}", view.spec.description),
    ),
    "lost" => (
      NotificationSeverity::Warning,
      format!("Background task lost: {}", view.spec.description),
    ),
    _ => return,
  };

  let mut body_lines = vec![
    format!("Task ID: {}", view.spec.id),
    format!("Status: {}", status),
    format!("Description: {}", view.spec.description),
  ];
  if terminal_reason != status.to_string() {
    body_lines.push(format!("Terminal reason: {}", terminal_reason));
  }
  if let Some(code) = view.runtime.exit_code {
    body_lines.push(format!("Exit code: {}", code));
  }
  if let Some(ref reason) = view.runtime.failure_reason {
    body_lines.push(format!("Failure reason: {}", reason));
  }

  let event = NotificationEvent {
    version: 1,
    id: manager.new_id(),
    category: "task".to_string(),
    event_type: format!("task.{}", terminal_reason),
    source_kind: "background_task".to_string(),
    source_id: view.spec.id.clone(),
    title,
    body: body_lines.join("\n"),
    severity,
    created_at: now_secs(),
    payload: serde_json::to_value(view).unwrap_or_default(),
    targets: crate::notification::models::default_notification_targets(),
    dedupe_key: Some(format!(
      "background_task:{}:{}",
      view.spec.id, terminal_reason
    )),
  };

  manager.publish(event);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> Timestamp {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}

/// Check whether a process with the given PID is still alive.
#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
  unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn is_process_alive(pid: u32) -> bool {
  // On Windows we fall back to heartbeat-based stale detection.
  // A full implementation would use OpenProcess + GetExitCodeProcess.
  let _ = pid;
  true
}

#[cfg(unix)]
fn best_effort_kill(pid: u32) {
  // Try process group first (worker spawns with new session / process group)
  unsafe {
    let pgid = libc::getpgid(pid as i32);
    if pgid > 0 {
      libc::killpg(pgid, libc::SIGTERM);
    } else {
      libc::kill(pid as i32, libc::SIGTERM);
    }
  }
}

#[cfg(not(unix))]
fn best_effort_kill(pid: u32) {
  // On Windows we rely on the control file + worker's own control loop.
  // Direct cross-process signaling is more complex on Windows.
  let _ = pid;
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::background::models::{TaskRuntime, TaskSpec, TaskStatus};
  use std::time::{SystemTime, UNIX_EPOCH};

  fn now_secs() -> Timestamp {
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_secs()
  }

  #[test]
  fn test_recover_marks_stale_task_as_lost() {
    let tmp = tempfile::tempdir().unwrap();
    let config = BackgroundConfig {
      max_running_tasks: 10,
      worker_heartbeat_interval_ms: 5000,
      wait_poll_interval_ms: 1000,
      kill_grace_period_ms: 5000,
      worker_stale_after_ms: 1000, // 1s stale threshold for test
      read_max_bytes: 1024,
      notification_tail_lines: 20,
    };
    let manager = BackgroundTaskManager::new(tmp.path().to_path_buf(), config);
    manager.bind_session("test-session");

    // Create a task directly via the store
    let store = manager.store().unwrap();
    let spec = TaskSpec {
      version: 1,
      id: "bash-test-001".to_string(),
      kind: "bash".to_string(),
      session_id: "test-session".to_string(),
      description: "Test task".to_string(),
      tool_call_id: "call-1".to_string(),
      command: "sleep 10".to_string(),
      shell_name: "bash".to_string(),
      shell_path: "/bin/bash".to_string(),
      cwd: "/".to_string(),
      timeout_s: Some(60),
      created_at: now_secs() - 10,
    };
    store.create_task(&spec);

    // Set runtime to Running with an old heartbeat
    let mut runtime = TaskRuntime::default();
    runtime.status = TaskStatus::Running;
    runtime.worker_pid = Some(999999); // Non-existent PID
    runtime.heartbeat_at = Some(now_secs() - 5);
    runtime.updated_at = now_secs() - 5;
    store.write_runtime(&spec.id, &runtime);

    // Recover should mark the stale task as lost
    let updated = manager.recover();
    assert_eq!(updated, 1);

    let view = store.merged_view(&spec.id).unwrap();
    assert_eq!(view.runtime.status, TaskStatus::Lost);
    assert!(view.runtime.finished_at.is_some());
    assert!(
      view
        .runtime
        .failure_reason
        .as_ref()
        .unwrap()
        .contains("worker")
    );
  }

  #[test]
  fn test_recover_skips_terminal_tasks() {
    let tmp = tempfile::tempdir().unwrap();
    let config = BackgroundConfig {
      max_running_tasks: 10,
      worker_heartbeat_interval_ms: 5000,
      wait_poll_interval_ms: 1000,
      kill_grace_period_ms: 5000,
      worker_stale_after_ms: 1000,
      read_max_bytes: 1024,
      notification_tail_lines: 20,
    };
    let manager = BackgroundTaskManager::new(tmp.path().to_path_buf(), config);
    manager.bind_session("test-session");

    let store = manager.store().unwrap();
    let spec = TaskSpec {
      version: 1,
      id: "bash-test-002".to_string(),
      kind: "bash".to_string(),
      session_id: "test-session".to_string(),
      description: "Completed task".to_string(),
      tool_call_id: "call-2".to_string(),
      command: "echo done".to_string(),
      shell_name: "bash".to_string(),
      shell_path: "/bin/bash".to_string(),
      cwd: "/".to_string(),
      timeout_s: Some(60),
      created_at: now_secs() - 10,
    };
    store.create_task(&spec);

    let mut runtime = TaskRuntime::default();
    runtime.status = TaskStatus::Completed;
    runtime.finished_at = Some(now_secs() - 5);
    store.write_runtime(&spec.id, &runtime);

    let updated = manager.recover();
    assert_eq!(updated, 0);

    let view = store.merged_view(&spec.id).unwrap();
    assert_eq!(view.runtime.status, TaskStatus::Completed);
  }
}
