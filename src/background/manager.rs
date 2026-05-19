//! Background task manager — creation, lifecycle, and querying.

use std::env;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{info, warn};
use tokio::time::sleep;

use crate::config::BackgroundConfig;

use super::ids::generate_task_id;
use super::models::{TaskOutputChunk, TaskSpec, TaskStatus, TaskView, is_terminal_status};
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
  #[allow(dead_code)]
  pub fn recover(&self) {
    let stale_after_s = self.config.worker_stale_after_ms / 1000;
    let Some(store) = self.store() else {
      return;
    };
    let now = now_secs();
    for view in store.list_views() {
      if is_terminal_status(view.runtime.status) {
        continue;
      }

      let last_progress = view
        .runtime
        .heartbeat_at
        .or(view.runtime.started_at)
        .or(Some(view.runtime.updated_at))
        .or(Some(view.spec.created_at))
        .unwrap_or(now);

      if now - last_progress <= stale_after_s as f64 {
        continue;
      }

      // Re-read runtime to narrow race window
      let fresh = store.read_runtime(&view.spec.id);
      if is_terminal_status(fresh.status) {
        continue;
      }
      let fresh_progress = fresh
        .heartbeat_at
        .or(fresh.started_at)
        .or(Some(fresh.updated_at))
        .or(Some(view.spec.created_at))
        .unwrap_or(now);

      if now - fresh_progress <= stale_after_s as f64 {
        continue;
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
        runtime.failure_reason = if runtime.heartbeat_at.is_none() {
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
    }
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> f64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs_f64()
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
