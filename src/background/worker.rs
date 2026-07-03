//! Background task worker — runs inside an independent child process.
//!
//! The worker reads the task spec, executes the shell command, and maintains
//! heartbeat / control loops until the command finishes or is killed.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use log::{error, info};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::sleep;

use super::models::{TaskSpecKind, TaskStatus, Timestamp};
use super::store::BackgroundTaskStore;
use crate::utils::time::now_secs;

/// Synchronous entry point for the worker process.
pub fn run_background_task_worker(
  task_dir: PathBuf,
  heartbeat_interval_ms: u64,
  control_poll_interval_ms: u64,
  kill_grace_period_ms: u64,
) {
  let task_id = task_dir
    .file_name()
    .and_then(|n| n.to_str())
    .ok_or("Invalid task_dir")
    .map_or_else(
      |e| {
        eprintln!("{}", e);
        std::process::exit(1);
      },
      |s| s.to_string(),
    );
  let store = BackgroundTaskStore::new(task_dir.parent().unwrap().to_path_buf());

  let spec = match store.read_spec(&task_id) {
    Some(s) => s,
    None => {
      eprintln!("Spec not found for task {}", task_id);
      std::process::exit(1);
    }
  };

  match spec.kind {
    TaskSpecKind::Agent(_) => {
      crate::background::agent_worker::run_agent_task_worker(
        task_dir,
        heartbeat_interval_ms,
        control_poll_interval_ms,
        kill_grace_period_ms,
      );
    }
    TaskSpecKind::Bash(_) => {
      let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
          eprintln!("Failed to create tokio runtime: {}", e);
          std::process::exit(1);
        }
      };

      rt.block_on(async {
        if let Err(e) = bash_worker_main(
          task_dir,
          heartbeat_interval_ms,
          control_poll_interval_ms,
          kill_grace_period_ms,
        )
        .await
        {
          error!("Background worker error: {}", e);
        }
      });
    }
  }
}

async fn bash_worker_main(
  task_dir: PathBuf,
  heartbeat_interval_ms: u64,
  control_poll_interval_ms: u64,
  kill_grace_period_ms: u64,
) -> Result<(), String> {
  let task_id = task_dir
    .file_name()
    .and_then(|n| n.to_str())
    .ok_or("Invalid task_dir")?
    .to_string();
  let store = BackgroundTaskStore::new(task_dir.parent().unwrap().to_path_buf());

  let spec = store
    .read_spec(&task_id)
    .ok_or_else(|| format!("Spec not found for task {}", task_id))?;

  // Extract bash-specific parameters up front.
  let params = match &spec.kind {
    TaskSpecKind::Bash(params) => params,
    TaskSpecKind::Agent(_) => {
      let mut runtime = store.read_runtime(&task_id);
      runtime.status = TaskStatus::Failed;
      runtime.finished_at = Some(now_secs());
      runtime.updated_at = runtime.finished_at.unwrap();
      runtime.failure_reason = Some(format!(
        "Bash worker received unsupported task kind: {}",
        spec.kind
      ));
      store.write_runtime(&task_id, &runtime);
      return Ok(());
    }
  };
  let command = params.command.as_str();
  let shell_path = params.shell_path.as_str();
  let cwd = params.cwd.as_str();

  // Mark as starting
  let mut runtime = store.read_runtime(&task_id);
  runtime.status = TaskStatus::Starting;
  runtime.worker_pid = Some(std::process::id());
  runtime.started_at = Some(now_secs());
  runtime.heartbeat_at = runtime.started_at;
  runtime.updated_at = runtime.started_at.unwrap();
  store.write_runtime(&task_id, &runtime);

  info!("Worker {} started for command: {:?}", task_id, command);

  // Check early kill request
  let control = store.read_control(&task_id);
  if control.kill_requested_at.is_some() {
    let mut runtime = store.read_runtime(&task_id);
    runtime.status = TaskStatus::Killed;
    runtime.interrupted = true;
    runtime.finished_at = Some(now_secs());
    runtime.updated_at = runtime.finished_at.unwrap();
    runtime.failure_reason = control
      .kill_reason
      .clone()
      .or_else(|| Some("Killed before command start".to_string()));
    store.write_runtime(&task_id, &runtime);
    return Ok(());
  }

  // Validate spec
  if command.is_empty() || shell_path.is_empty() || cwd.is_empty() {
    let mut runtime = store.read_runtime(&task_id);
    runtime.status = TaskStatus::Failed;
    runtime.finished_at = Some(now_secs());
    runtime.updated_at = runtime.finished_at.unwrap();
    runtime.failure_reason = Some("Task spec is incomplete for bash worker".to_string());
    store.write_runtime(&task_id, &runtime);
    return Ok(());
  }

  // Open output file
  let output_path = store.output_path(&task_id);
  let output_file = match tokio::fs::File::create(&output_path).await {
    Ok(f) => f,
    Err(e) => {
      let mut runtime = store.read_runtime(&task_id);
      runtime.status = TaskStatus::Failed;
      runtime.finished_at = Some(now_secs());
      runtime.updated_at = runtime.finished_at.unwrap();
      runtime.failure_reason = Some(format!("Failed to open output file: {}", e));
      store.write_runtime(&task_id, &runtime);
      return Ok(());
    }
  };

  // Spawn shell process
  let shell_name = params.shell_name.as_str();
  let args = if shell_name == "Windows PowerShell" {
    (shell_path.to_string(), "-command", command.to_string())
  } else {
    (shell_path.to_string(), "-c", command.to_string())
  };

  let mut child = match Command::new(&args.0)
    .arg(args.1)
    .arg(args.2)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .current_dir(cwd)
    .kill_on_drop(false)
    .spawn()
  {
    Ok(c) => c,
    Err(e) => {
      let mut runtime = store.read_runtime(&task_id);
      runtime.status = TaskStatus::Failed;
      runtime.finished_at = Some(now_secs());
      runtime.updated_at = runtime.finished_at.unwrap();
      runtime.failure_reason = Some(format!("Failed to spawn shell: {}", e));
      store.write_runtime(&task_id, &runtime);
      return Ok(());
    }
  };

  let child_pid = child.id().unwrap_or(0);

  // Mark as running
  let mut runtime = store.read_runtime(&task_id);
  runtime.status = TaskStatus::Running;
  runtime.child_pid = Some(child_pid);
  runtime.updated_at = now_secs();
  runtime.heartbeat_at = Some(runtime.updated_at);
  store.write_runtime(&task_id, &runtime);

  // Start heartbeat and control loops
  let stop = std::sync::Arc::new(tokio::sync::Notify::new());
  let control_stop = stop.clone();
  let heartbeat_handle = tokio::spawn(heartbeat_loop(
    store.root().to_path_buf(),
    task_id.clone(),
    heartbeat_interval_ms,
    stop.clone(),
  ));

  let control_handle = tokio::spawn(control_loop(
    store.root().to_path_buf(),
    task_id.clone(),
    control_poll_interval_ms,
    kill_grace_period_ms,
    child_pid,
    control_stop.clone(),
  ));

  // Pipe stdout/stderr to output file
  let stdout = child.stdout.take().unwrap();
  let stderr = child.stderr.take().unwrap();
  let mut output_file = output_file;

  let stdout_handle = tokio::spawn(async move {
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut buf = [0u8; 4096];
    loop {
      match tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await {
        Ok(0) => break,
        Ok(n) => {
          let _ = output_file.write_all(&buf[..n]).await;
        }
        Err(_) => break,
      }
    }
  });

  let mut stderr_file = match tokio::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&output_path)
    .await
  {
    Ok(f) => f,
    Err(_) => tokio::fs::File::create(&output_path).await.unwrap(),
  };
  let stderr_handle = tokio::spawn(async move {
    let mut reader = tokio::io::BufReader::new(stderr);
    let mut buf = [0u8; 4096];
    loop {
      match tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await {
        Ok(0) => break,
        Ok(n) => {
          let _ = stderr_file.write_all(&buf[..n]).await;
        }
        Err(_) => break,
      }
    }
  });

  // Wait for process with optional timeout
  let timeout = spec.timeout_s.map(Duration::from_secs);
  let exit_code: Option<i32>;
  let mut timed_out = false;
  let mut timeout_reason: Option<String> = None;

  if let Some(timeout) = timeout {
    match tokio::time::timeout(timeout, child.wait()).await {
      Ok(Ok(status)) => {
        exit_code = status.code();
      }
      Ok(Err(e)) => {
        error!("Child wait error: {}", e);
        exit_code = Some(-1);
      }
      Err(_) => {
        timed_out = true;
        timeout_reason = Some(format!(
          "Command timed out after {}s",
          spec.timeout_s.unwrap()
        ));
        // Try graceful kill first
        best_effort_kill(child_pid, false);
        match tokio::time::timeout(Duration::from_millis(kill_grace_period_ms), child.wait()).await
        {
          Ok(Ok(status)) => exit_code = status.code(),
          _ => {
            best_effort_kill(child_pid, true);
            let final_status = child.wait().await;
            exit_code = final_status.ok().and_then(|s| s.code());
          }
        }
      }
    }
  } else {
    match child.wait().await {
      Ok(status) => exit_code = status.code(),
      Err(e) => {
        error!("Child wait error: {}", e);
        exit_code = Some(-1);
      }
    }
  }

  // Wait for stdout/stderr drains
  let _ = stdout_handle.await;
  let _ = stderr_handle.await;

  // Stop loops
  stop.notify_waiters();
  control_stop.notify_waiters();
  let _ = heartbeat_handle.await;
  let _ = control_handle.await;

  // Finalize runtime
  let mut runtime = store.read_runtime(&task_id);
  let control = store.read_control(&task_id);
  runtime.finished_at = Some(now_secs());
  runtime.updated_at = runtime.finished_at.unwrap();
  runtime.exit_code = exit_code;
  runtime.heartbeat_at = runtime.finished_at;

  if timed_out {
    runtime.status = TaskStatus::Failed;
    runtime.interrupted = true;
    runtime.timed_out = true;
    runtime.failure_reason = timeout_reason;
  } else if control.kill_requested_at.is_some() {
    runtime.status = TaskStatus::Killed;
    runtime.interrupted = true;
    runtime.failure_reason = control
      .kill_reason
      .clone()
      .or_else(|| Some("Killed".to_string()));
  } else if exit_code == Some(0) {
    runtime.status = TaskStatus::Completed;
    runtime.failure_reason = None;
  } else {
    runtime.status = TaskStatus::Failed;
    runtime.failure_reason = Some(format!(
      "Command failed with exit code {}",
      exit_code.unwrap_or(-1)
    ));
  }

  store.write_runtime(&task_id, &runtime);
  info!(
    "Worker {} finished with status: {}, exit_code: {:?}",
    task_id, runtime.status, exit_code
  );

  Ok(())
}

async fn heartbeat_loop(
  store_root: PathBuf,
  task_id: String,
  interval_ms: u64,
  stop: std::sync::Arc<tokio::sync::Notify>,
) {
  let store = BackgroundTaskStore::new(store_root);
  let interval = Duration::from_millis(interval_ms);
  loop {
    tokio::select! {
      _ = sleep(interval) => {}
      _ = stop.notified() => break,
    }
    let mut runtime = store.read_runtime(&task_id);
    if runtime.finished_at.is_some() {
      break;
    }
    runtime.heartbeat_at = Some(now_secs());
    runtime.updated_at = runtime.heartbeat_at.unwrap();
    store.write_runtime(&task_id, &runtime);
  }
}

async fn control_loop(
  store_root: PathBuf,
  task_id: String,
  poll_interval_ms: u64,
  kill_grace_period_ms: u64,
  child_pid: u32,
  stop: std::sync::Arc<tokio::sync::Notify>,
) {
  let store = BackgroundTaskStore::new(store_root);
  let poll = Duration::from_millis(poll_interval_ms);
  let mut kill_sent_at: Option<Timestamp> = None;
  let grace_period_s = kill_grace_period_ms / 1000;

  loop {
    tokio::select! {
      _ = sleep(poll) => {}
      _ = stop.notified() => break,
    }
    let runtime = store.read_runtime(&task_id);
    if runtime.finished_at.is_some() {
      break;
    }

    let control = store.read_control(&task_id);
    if control.kill_requested_at.is_some() {
      best_effort_kill(child_pid, control.force);
      let now = now_secs();
      kill_sent_at = kill_sent_at.or(Some(now));
      if let Some(sent) = kill_sent_at
        && !control.force
        && now - sent >= grace_period_s
      {
        best_effort_kill(child_pid, true);
      }
    }
  }
}

#[cfg(unix)]
fn best_effort_kill(pid: u32, force: bool) {
  unsafe {
    let pgid = libc::getpgid(pid as i32);
    let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
    if pgid > 0 {
      libc::killpg(pgid, sig);
    } else {
      libc::kill(pid as i32, sig);
    }
  }
}

#[cfg(not(unix))]
fn best_effort_kill(pid: u32, force: bool) {
  let _ = (pid, force);
}
