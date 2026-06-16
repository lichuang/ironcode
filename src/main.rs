mod background;
mod cli;
mod config;
mod error;
mod history;
mod hooks;
mod llm;
mod notification;
mod session;
mod tools;
mod tui;
mod utils;
mod view;
mod wire;

use std::env;
use std::fs::{self, OpenOptions};

use chrono::Local;
use env_logger::Target;

use anyhow::Result;
use clap::Parser;
use crossterm::event::KeyEventKind;
use futures::StreamExt;
use log::{info, warn};

use cli::{App, Args, runtime::Runtime};
use config::Config;
use config::loader::{data_dir, load_config_from_dir};
use hooks::{HookEventType, events as hook_events};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tui::{Tui, TuiEvent, init_terminal, restore_terminal};

// Re-export error types for convenience
pub use error::{Error, Result as IronResult};

/// Initialize logging based on configuration
///
/// Logs are always written to ${data_dir}/logs/ironcode.log
/// where data_dir is determined by the config.dir setting (defaults to ~/.ironcode/)
fn init_logging(config: &Config) {
  let mut builder = env_logger::Builder::new();

  // Parse RUST_LOG env var first, then fall back to config level
  if let Ok(rust_log) = env::var("RUST_LOG") {
    builder.parse_filters(&rust_log);
  } else {
    builder.parse_filters(&config.logging.level);
  }

  // Set custom format with local timezone
  builder.format(|buf, record| {
    use std::io::Write;
    let timestamp = Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z");
    writeln!(
      buf,
      "[{} {} {}] {}",
      timestamp,
      record.level(),
      record.target(),
      record.args()
    )
  });

  // Determine log file path: ${data_dir}/logs/ironcode.log
  let data_dir = data_dir(config);
  let logs_dir = data_dir.join("logs");
  let log_file = logs_dir.join("ironcode.log");

  // Create logs directory if it doesn't exist
  if !logs_dir.exists()
    && let Err(e) = fs::create_dir_all(&logs_dir)
  {
    builder.init();
    warn!("Failed to create logs directory {:?}: {}", logs_dir, e);
    return;
  }

  // Open log file and write to it
  match OpenOptions::new()
    .create(true)
    .truncate(false)
    .append(true)
    .open(&log_file)
  {
    Ok(file) => {
      builder.target(Target::Pipe(Box::new(file)));
    }
    Err(e) => {
      // Initialize default logger first, then log the warning
      builder.init();
      warn!("Failed to open log file {:?}: {}", log_file, e);
      return;
    }
  }

  builder.init();
}

#[tokio::main]
async fn main() -> Result<()> {
  // Parse command line arguments
  let args = Args::parse();

  // Background task worker mode — run independently and exit.
  if let Some(ref task_dir) = args.background_task_worker {
    background::run_background_task_worker(
      task_dir.clone(),
      args.worker_heartbeat_interval_ms.unwrap_or(5000),
      args.worker_control_poll_interval_ms.unwrap_or(500),
      args.worker_kill_grace_period_ms.unwrap_or(2000),
    );
    return Ok(());
  }

  // Load configuration
  let config_file_dir = args.config_dir();
  let user_config = load_config_from_dir(&config_file_dir)?;

  // Build layered configuration (user config + CLI overrides)
  let app_config = config::AppConfig {
    user: user_config,
    overrides: config::CliOverrides {
      yolo: if args.yolo { Some(true) } else { None },
      mcp_config_file: args.mcp_config_file.clone(),
      session_id: args.session.clone(),
      r#continue: args.r#continue,
    },
  };
  let config = app_config.effective();

  // Get the data directory from config (defaults to ~/.ironcode/ if not specified)
  let data_dir = data_dir(&config);

  // Initialize logging based on configuration
  init_logging(&config);
  info!("IronCode started successfully");
  info!(
    "Config file dir: {:?}, Data dir: {:?}",
    config_file_dir, data_dir
  );

  // Initialize terminal
  init_terminal()?;

  // Create TUI infrastructure
  let mut tui = Tui::new()?;

  // Create runtime (loads system prompt, tool registries, etc.)
  let runtime = Arc::new(Runtime::new(&data_dir, Arc::new(config))?);

  // Create app state
  let mut app = App::new(&data_dir, &args, runtime).await?;

  // Give the view a frame requester for animations
  app.set_frame_requester(tui.frame_requester());

  // Run the main event loop
  let result = run_app(&mut tui, &mut app).await;

  // Trigger SessionEnd hook (best-effort, 5s timeout).
  let session_id = app
    .chat_session
    .as_ref()
    .map(|s| s.handle.id.clone())
    .unwrap_or_default();
  let _ = timeout(
    Duration::from_secs(5),
    app.runtime.hook_engine().trigger(
      HookEventType::SessionEnd,
      "exit",
      hook_events::session_end(&session_id, &app.runtime.args.work_dir, "exit"),
    ),
  )
  .await;

  // Cleanup: kill any active background tasks
  app.cleanup_background_tasks();

  // Restore terminal settings
  restore_terminal()?;

  info!("IronCode exit");

  result
}

/// Run the system pager with the given content.
fn run_pager(content: &str) -> Result<()> {
  let pager = env::var("PAGER").unwrap_or_else(|_| {
    if cfg!(target_os = "windows") {
      "more".to_string()
    } else {
      "less".to_string()
    }
  });

  let mut cmd = std::process::Command::new(&pager);
  if pager == "less" {
    cmd.arg("-R"); // Preserve ANSI colors
  }
  cmd.stdin(std::process::Stdio::piped());

  let mut child = cmd
    .spawn()
    .map_err(|e| anyhow::anyhow!("Failed to start pager '{}': {}", pager, e))?;

  if let Some(mut stdin) = child.stdin.take() {
    use std::io::Write;
    stdin.write_all(content.as_bytes())?;
  }

  child.wait()?;
  Ok(())
}

/// Run the main application loop
async fn run_app(tui: &mut Tui, app: &mut App) -> Result<()> {
  // Create event stream
  let mut event_stream = tui.create_event_stream();

  // Initial draw
  tui.draw(|f| app.draw(f))?;

  // Process events from the stream
  while let Some(event) = event_stream.next().await {
    // Process LLM stream events
    app.update_chat_session();

    // Handle pending pager output (from /task browser)
    if let Some(output) = app.take_pager_output() {
      restore_terminal()?;
      if let Err(e) = run_pager(&output) {
        warn!("Failed to run pager: {}", e);
      }
      init_terminal()?;
    }

    match event {
      TuiEvent::Key(key) => {
        // Only handle key press events to avoid duplicate processing
        if key.kind == KeyEventKind::Press {
          app.handle_key(key);
        }
      }
      TuiEvent::Paste(_text) => {
        // Handle paste events - for now just insert as if typed
        // This could be enhanced to handle multi-line paste specially
        // TODO: Implement proper paste handling in View trait
      }
      TuiEvent::Draw => {
        // Frame draw request - update animation state and redraw
        let frame_requester = tui.frame_requester();
        app.on_frame(&frame_requester);
      }
    }

    // Check if we should exit
    if app.should_exit() {
      return Ok(());
    }

    // Redraw the UI
    tui.draw(|f| app.draw(f))?;
  }

  Ok(())
}
