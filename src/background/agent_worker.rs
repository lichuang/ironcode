//! Background agent worker — runs a subagent inside an independent worker process.
//!
//! The worker reads a `kind="agent"` task spec, reconstructs the subagent
//! runtime, drives a `SessionActor` to completion, and writes the final summary
//! to both the task `output.log` and the per-agent `SubagentStore` output file.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{error, info, warn};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio::time::{sleep, timeout};

use crate::background::models::{TaskSpecKind, TaskStatus};
use crate::background::store::BackgroundTaskStore;
use crate::cli::runtime::Runtime;
use crate::config::loader::{data_dir, default_data_dir, load_config_from_dir};
use crate::git_context::GitContext;
use crate::llm::session::ChatSession;
use crate::llm::types::Message;
use crate::subagents::store::SubagentStore;
use crate::subagents::{AgentTaskPayload, SubagentStatus};
use crate::utils::time::now_secs;
use crate::wire::{WireBus, WireMessage};

/// Synchronous entry point for the agent worker.
///
/// Called from `background::worker::run_background_task_worker` when the task
/// spec has `kind == "agent"`. It creates a tokio runtime and blocks on the
/// async agent loop.
pub fn run_agent_task_worker(
  task_dir: PathBuf,
  heartbeat_interval_ms: u64,
  control_poll_interval_ms: u64,
  kill_grace_period_ms: u64,
) {
  let rt = match tokio::runtime::Runtime::new() {
    Ok(rt) => rt,
    Err(e) => {
      eprintln!("Failed to create tokio runtime: {}", e);
      std::process::exit(1);
    }
  };

  rt.block_on(async {
    if let Err(e) = agent_worker_main(
      task_dir,
      heartbeat_interval_ms,
      control_poll_interval_ms,
      kill_grace_period_ms,
    )
    .await
    {
      error!("Background agent worker error: {}", e);
    }
  });
}

async fn agent_worker_main(
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

  let params = match &spec.kind {
    TaskSpecKind::Agent(params) => params,
    TaskSpecKind::Bash(_) => {
      let mut runtime = store.read_runtime(&task_id);
      runtime.status = TaskStatus::Failed;
      runtime.finished_at = Some(now_secs());
      runtime.updated_at = runtime.finished_at.unwrap();
      runtime.failure_reason = Some(format!(
        "Agent worker received unsupported task kind: {}",
        spec.kind
      ));
      store.write_runtime(&task_id, &runtime);
      return Ok(());
    }
  };

  let payload: AgentTaskPayload =
    serde_json::from_value(params.kind_payload.clone()).map_err(|e| {
      let reason = format!("Failed to parse agent task payload: {}", e);
      fail_task(&store, &task_id, &reason);
      reason
    })?;

  // Mark as starting.
  {
    let mut runtime = store.read_runtime(&task_id);
    runtime.status = TaskStatus::Starting;
    runtime.worker_pid = Some(std::process::id());
    runtime.started_at = Some(now_secs());
    runtime.heartbeat_at = runtime.started_at;
    runtime.updated_at = runtime.started_at.unwrap();
    store.write_runtime(&task_id, &runtime);
  }

  info!(
    "Agent worker {} started for agent {} ({})",
    task_id, payload.agent_id, payload.subagent_type
  );

  // Reconstruct runtime from persisted config directory.
  let config_dir = params
    .config_dir
    .clone()
    .map(PathBuf::from)
    .or_else(default_data_dir)
    .ok_or_else(|| {
      let reason = "Could not determine config directory".to_string();
      fail_task(&store, &task_id, &reason);
      reason
    })?;

  let config = match load_config_from_dir(&config_dir) {
    Ok(c) => Arc::new(c),
    Err(e) => {
      let reason = format!("Failed to load config: {}", e);
      fail_task(&store, &task_id, &reason);
      return Ok(());
    }
  };

  let data_dir = data_dir(&config);
  let parent_runtime = match Runtime::new(&data_dir, config, &config_dir) {
    Ok(r) => Arc::new(r),
    Err(e) => {
      let reason = format!("Failed to create runtime: {}", e);
      fail_task(&store, &task_id, &reason);
      return Ok(());
    }
  };

  let type_def = match parent_runtime.labor_market.require(&payload.subagent_type) {
    Ok(d) => d.clone(),
    Err(e) => {
      let reason = format!("Unknown subagent type: {}", e);
      fail_task(&store, &task_id, &reason);
      return Ok(());
    }
  };

  // Build child runtime with tool policy and optional model override.
  let child_runtime = Arc::new(parent_runtime.copy_for_subagent(
    payload.agent_id.clone(),
    payload.subagent_type.clone(),
    payload.effective_model.clone(),
    &payload.tool_policy,
  ));

  // Resolve provider for the child runtime.
  let provider =
    match ChatSession::create_provider(&child_runtime, payload.effective_model.as_deref()) {
      Ok(p) => p,
      Err(e) => {
        let reason = format!("Failed to create subagent provider: {}", e);
        fail_task(&store, &task_id, &reason);
        return Ok(());
      }
    };

  // Build system prompt.
  let mut runtime_args = parent_runtime.args.clone();
  runtime_args.role_additional = type_def.role_additional.clone();
  let system_prompt = {
    let mut s = child_runtime.system_prompt_template.clone();
    s = s.replace("${IRONCODE_NOW}", &runtime_args.now);
    s = s.replace("${IRONCODE_WORK_DIR}", &runtime_args.work_dir);
    s = s.replace("${IRONCODE_WORK_DIR_LS}", &runtime_args.work_dir_ls);
    s = s.replace(
      "${IRONCODE_ADDITIONAL_DIRS_INFO}",
      &runtime_args.additional_dirs_info,
    );
    s = s.replace("${IRONCODE_AGENTS_MD}", &runtime_args.agents_md);
    s = s.replace("${IRONCODE_SKILLS}", &runtime_args.skills);
    s = s.replace("${ROLE_ADDITIONAL}", &runtime_args.role_additional);
    s
  };

  // For explore agents, prepend git context to the prompt.
  let user_prompt = if payload.subagent_type == "explore" {
    let git_ctx = GitContext::new(
      parent_runtime.config.git_context.clone(),
      PathBuf::from(&runtime_args.work_dir),
    )
    .collect()
    .await;
    if git_ctx.is_empty() {
      payload.prompt.clone()
    } else {
      format!("{}\n\n{}", git_ctx, payload.prompt)
    }
  } else {
    payload.prompt.clone()
  };

  let messages = vec![Message::system(system_prompt), Message::user(user_prompt)];

  // Prepare subagent store and update instance status.
  let session_dir = data_dir.join("sessions").join(&spec.session_id);
  let subagent_store = SubagentStore::new(session_dir);
  if let Err(e) = update_subagent_status(
    &subagent_store,
    &payload.agent_id,
    SubagentStatus::RunningBackground,
    Some(task_id.clone()),
  ) {
    warn!("Failed to update subagent status: {}", e);
  }

  // Open task output file.
  let output_path = store.output_path(&task_id);
  let mut output_file = match tokio::fs::File::create(&output_path).await {
    Ok(f) => f,
    Err(e) => {
      let reason = format!("Failed to open task output file: {}", e);
      fail_task(&store, &task_id, &reason);
      return Ok(());
    }
  };

  // Create a private wire bus for the subagent.
  let child_bus = WireBus::new(1024);
  let child_publisher = child_bus.publisher();
  let mut subscriber = child_bus.subscriber();

  // Spawn the subagent session with YOLO enabled so background agents do not
  // block on interactive approvals.
  let handle = ChatSession::start_subagent(
    payload.agent_id.clone(),
    provider,
    messages,
    child_runtime,
    child_publisher,
    Some(type_def.tool_policy.clone()),
    true, // yolo
  );

  // Send an empty user message to start the turn.
  handle.send_message("");

  // Shared cancellation for heartbeat and control loops.
  let stop = Arc::new(tokio::sync::Notify::new());
  let heartbeat_stop = stop.clone();
  let control_stop = stop.clone();

  // Start heartbeat loop.
  let heartbeat_handle = tokio::spawn(heartbeat_loop(
    store.root().to_path_buf(),
    task_id.clone(),
    heartbeat_interval_ms,
    heartbeat_stop,
  ));

  // Start control (kill) loop.
  let control_handle = tokio::spawn(control_loop(
    store.root().to_path_buf(),
    task_id.clone(),
    control_poll_interval_ms,
    kill_grace_period_ms,
    control_stop.clone(),
  ));

  // Collect the subagent response with optional timeout.
  let timeout_duration = payload.timeout_s.map(Duration::from_secs);
  let collect_fut = collect_subagent_response(&mut subscriber, &mut output_file);
  let (summary, timed_out) = match timeout_duration {
    Some(d) => match timeout(d, collect_fut).await {
      Ok(s) => (s, false),
      Err(_) => {
        warn!("Agent task {} timed out after {:?}", task_id, d);
        handle.shutdown();
        (
          Some(format!(
            "Background agent task timed out after {} seconds.",
            d.as_secs()
          )),
          true,
        )
      }
    },
    None => (collect_fut.await, false),
  };

  // Stop helper loops.
  stop.notify_waiters();
  let _ = heartbeat_handle.await;
  let _ = control_handle.await;

  // Write final summary block.
  let final_output = match summary {
    Some(ref text) if !text.is_empty() => {
      let block = format!("\n\n[summary]\n{}", text);
      let _ = output_file.write_all(block.as_bytes()).await;
      block
    }
    _ => {
      let block = "\n\n[summary]\nNo response from subagent.".to_string();
      let _ = output_file.write_all(block.as_bytes()).await;
      block
    }
  };

  // Flush and finalize.
  let _ = output_file.flush().await;
  drop(output_file);

  // Append the same output to the subagent store transcript.
  if let Ok(transcript) = tokio::fs::read_to_string(&output_path).await {
    let _ = subagent_store.append_output(&payload.agent_id, &transcript);
  }

  // Determine final status. Respect a Killed state written by the control loop.
  let (status, failure_reason) = {
    let control = store.read_control(&task_id);
    let runtime = store.read_runtime(&task_id);
    if runtime.status == TaskStatus::Killed || control.kill_requested_at.is_some() {
      (
        TaskStatus::Killed,
        control
          .kill_reason
          .clone()
          .or_else(|| runtime.failure_reason.clone())
          .or_else(|| Some("Killed by user".to_string())),
      )
    } else if timed_out {
      (TaskStatus::Failed, Some("Timed out".to_string()))
    } else if summary.is_none() {
      (
        TaskStatus::Failed,
        Some("Subagent produced no response".to_string()),
      )
    } else {
      (TaskStatus::Completed, None)
    }
  };

  {
    let mut runtime = store.read_runtime(&task_id);
    runtime.status = status;
    runtime.timed_out = timed_out;
    runtime.finished_at = Some(now_secs());
    runtime.updated_at = runtime.finished_at.unwrap();
    runtime.failure_reason = failure_reason;
    store.write_runtime(&task_id, &runtime);
  }

  let subagent_status = match status {
    TaskStatus::Completed => SubagentStatus::Completed,
    TaskStatus::Failed => SubagentStatus::Failed,
    TaskStatus::Killed => SubagentStatus::Killed,
    _ => SubagentStatus::Failed,
  };
  if let Err(e) = update_subagent_status(
    &subagent_store,
    &payload.agent_id,
    subagent_status,
    Some(task_id.clone()),
  ) {
    warn!("Failed to update final subagent status: {}", e);
  }

  info!("Agent worker {} finished with status {:?}", task_id, status);

  // Suppress unused warning for final_output; it is kept for future hook use.
  let _ = final_output;

  Ok(())
}

/// Write a failure state to the task runtime.
fn fail_task(store: &BackgroundTaskStore, task_id: &str, reason: &str) {
  let mut runtime = store.read_runtime(task_id);
  runtime.status = TaskStatus::Failed;
  runtime.finished_at = Some(now_secs());
  runtime.updated_at = runtime.finished_at.unwrap();
  runtime.failure_reason = Some(reason.to_string());
  store.write_runtime(task_id, &runtime);
  error!("Background agent task {} failed: {}", task_id, reason);
}

/// Update the persisted subagent instance status.
fn update_subagent_status(
  store: &SubagentStore,
  agent_id: &str,
  status: SubagentStatus,
  task_id: Option<String>,
) -> anyhow::Result<()> {
  let mut record = store
    .load_meta(agent_id)?
    .ok_or_else(|| std::io::Error::other(format!("Agent instance {} not found", agent_id)))?;
  record.status = status;
  record.updated_at = now_secs();
  if let Some(id) = task_id {
    record.last_task_id = Some(id);
  }
  store.save_meta(&record)
}

/// Collect the final assistant response from child wire events and stream the
/// transcript to the task output file.
async fn collect_subagent_response(
  subscriber: &mut broadcast::Receiver<WireMessage>,
  output_file: &mut tokio::fs::File,
) -> Option<String> {
  let mut in_turn = false;
  let mut current_response = String::new();

  loop {
    let msg = match subscriber.recv().await {
      Ok(m) => m,
      Err(_) => break,
    };

    match msg {
      WireMessage::TurnBegin => {
        in_turn = true;
        current_response.clear();
      }
      WireMessage::ContentChunk { text } => {
        if in_turn {
          current_response.push_str(&text);
          let _ = output_file.write_all(text.as_bytes()).await;
        }
      }
      WireMessage::TurnEnd => {
        if in_turn && !current_response.is_empty() {
          return Some(current_response);
        }
        in_turn = false;
      }
      WireMessage::ToolCallEnd { output, .. } => {
        if current_response.is_empty() {
          current_response.push_str(&output);
        }
      }
      WireMessage::Error { message } => {
        let _ = output_file
          .write_all(format!("\nSubagent error: {}\n", message).as_bytes())
          .await;
        return Some(format!("Subagent error: {}", message));
      }
      _ => {}
    }
  }

  if !current_response.is_empty() {
    Some(current_response)
  } else {
    None
  }
}

/// Heartbeat loop that updates `runtime.heartbeat_at` until cancelled.
async fn heartbeat_loop(
  root: PathBuf,
  task_id: String,
  interval_ms: u64,
  stop: Arc<tokio::sync::Notify>,
) {
  let store = BackgroundTaskStore::new(root);
  let interval = Duration::from_millis(interval_ms);
  loop {
    tokio::select! {
      _ = stop.notified() => break,
      _ = sleep(interval) => {
        let mut runtime = store.read_runtime(&task_id);
        if is_terminal_status(runtime.status) {
          break;
        }
        runtime.heartbeat_at = Some(now_secs());
        runtime.updated_at = runtime.heartbeat_at.unwrap();
        store.write_runtime(&task_id, &runtime);
      }
    }
  }
}

/// Control loop that checks for kill requests and signals cancellation.
async fn control_loop(
  root: PathBuf,
  task_id: String,
  poll_interval_ms: u64,
  _kill_grace_period_ms: u64,
  stop: Arc<tokio::sync::Notify>,
) {
  let store = BackgroundTaskStore::new(root);
  let interval = Duration::from_millis(poll_interval_ms);
  loop {
    tokio::select! {
      _ = stop.notified() => break,
      _ = sleep(interval) => {
        let control = store.read_control(&task_id);
        if control.kill_requested_at.is_some() {
          let mut runtime = store.read_runtime(&task_id);
          if !is_terminal_status(runtime.status) {
            runtime.status = TaskStatus::Killed;
            runtime.interrupted = true;
            runtime.finished_at = Some(now_secs());
            runtime.updated_at = runtime.finished_at.unwrap();
            runtime.failure_reason = control
              .kill_reason
              .clone()
              .or_else(|| Some("Killed by user".to_string()));
            store.write_runtime(&task_id, &runtime);
          }
          break;
        }
      }
    }
  }
}

use crate::background::models::is_terminal_status;
