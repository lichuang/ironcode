mod cli;
mod llm;
mod tui;
mod utils;
mod view;

use anyhow::Result;
use cli::App;
use crossterm::event::KeyEventKind;
use futures::StreamExt;
use tui::{Tui, TuiEvent, TuiEventStream, init_terminal, restore_terminal};

#[tokio::main]
async fn main() -> Result<()> {
  // Initialize terminal
  init_terminal()?;

  // Create TUI infrastructure
  let mut tui = Tui::new()?;

  // Create app state
  let mut app = App::new()?;

  // Give the view a frame requester for animations
  app.set_frame_requester(tui.frame_requester());

  // Create event stream
  let mut event_stream = tui.create_event_stream();

  // Run the main event loop
  let result = run_app(&mut tui, &mut app, &mut event_stream).await;

  // Restore terminal settings
  restore_terminal()?;

  result
}

/// Run the main application loop
async fn run_app(tui: &mut Tui, app: &mut App, event_stream: &mut TuiEventStream) -> Result<()> {
  // Initial draw
  tui.draw(|f| app.draw(f))?;

  // Process events from the stream
  while let Some(event) = event_stream.next().await {
    // First, drain any pending UI messages from background tasks
    while let Some(msg) = app.try_recv_message() {
      app.handle_message(msg);
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
