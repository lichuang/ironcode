mod cli;
mod config;
mod error;
mod llm;
mod tui;
mod utils;
mod view;

use anyhow::Result;
use clap::Parser;
use cli::{App, Args};
use config::Config;
use config::loader::{data_dir, load_config_from_dir};
use crossterm::event::KeyEventKind;
use futures::StreamExt;
use log::{info, warn};
use tui::{Tui, TuiEvent, TuiEventStream, init_terminal, restore_terminal};

// Re-export error types for convenience
pub use error::{Error, Result as IronResult};

/// Initialize logging based on configuration
/// 
/// Logs are always written to ${data_dir}/logs/ironcode.log
/// where data_dir is determined by the config.dir setting (defaults to ~/.ironcode/)
fn init_logging(config: &Config) {
  use env_logger::Target;
  use std::fs::OpenOptions;

  let mut builder = env_logger::Builder::new();

  // Parse RUST_LOG env var first, then fall back to config level
  if let Ok(rust_log) = std::env::var("RUST_LOG") {
    builder.parse_filters(&rust_log);
  } else {
    builder.parse_filters(&config.logging.level);
  }

  // Determine log file path: ${data_dir}/logs/ironcode.log
  let data_dir = data_dir(config);
  let logs_dir = data_dir.join("logs");
  let log_file = logs_dir.join("ironcode.log");

  // Create logs directory if it doesn't exist
  if !logs_dir.exists() {
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
      builder.init();
      warn!("Failed to create logs directory {:?}: {}", logs_dir, e);
      return;
    }
  }

  // Open log file and write to it
  match OpenOptions::new().create(true).append(true).open(&log_file) {
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

  // Load configuration
  // First, get the config file directory (either from -c arg or default ~/.ironcode/)
  let config_file_dir = args.config_dir();
  let config = load_config_from_dir(&config_file_dir)?;
  
  // Get the data directory from config (defaults to ~/.ironcode/ if not specified)
  let data_dir = data_dir(&config);

  // Initialize logging based on configuration
  init_logging(&config);
  info!("IronCode started successfully");
  info!("Config file dir: {:?}, Data dir: {:?}", config_file_dir, data_dir);

  // Initialize terminal
  init_terminal()?;

  // Create TUI infrastructure
  let mut tui = Tui::new()?;

  // Create app state with configuration
  // Pass data_dir for loading system prompt from data_dir/prompts/system.md
  let mut app = App::new(config, &data_dir)?;

  // Give the view a frame requester for animations
  app.set_frame_requester(tui.frame_requester());

  // Create event stream
  let mut event_stream = tui.create_event_stream();

  // Run the main event loop
  let result = run_app(&mut tui, &mut app, &mut event_stream).await;

  // Restore terminal settings
  restore_terminal()?;

  info!("IronCode exit");

  result
}

/// Run the main application loop
async fn run_app(
  tui: &mut Tui,
  app: &mut App,
  event_stream: &mut TuiEventStream,
) -> anyhow::Result<()> {
  // Initial draw
  tui.draw(|f| app.draw(f))?;

  // Process events from the stream
  while let Some(event) = event_stream.next().await {
    // First, drain any pending UI messages from background tasks
    while let Some(msg) = app.try_recv_message() {
      app.handle_message(msg);
    }

    // Process LLM stream events
    app.update_chat_session();

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
