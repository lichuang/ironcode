mod app;
mod view;

use anyhow::Result;
use app::App;
use crossterm::{
  event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
  Terminal,
  backend::{Backend, CrosstermBackend},
};
use std::io;

fn main() -> Result<()> {
  // Enable raw mode for terminal UI
  enable_raw_mode()?;
  let mut stdout = io::stdout();
  // Enter alternate screen and enable mouse capture
  execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
  let backend = CrosstermBackend::new(stdout);
  let mut terminal = Terminal::new(backend)?;

  // Create app state
  let mut app = App::new();

  // Run the main app loop
  let result = run_app(&mut terminal, &mut app);

  // Restore terminal settings
  disable_raw_mode()?;
  execute!(
    terminal.backend_mut(),
    LeaveAlternateScreen,
    DisableMouseCapture
  )?;
  terminal.show_cursor()?;

  result
}

/// Run the main application loop
fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
  B::Error: Send + Sync + 'static,
{
  loop {
    // Draw the UI
    terminal.draw(|f| app.draw(f))?;

    // Handle keyboard events
    if let Event::Key(key) = event::read()? {
      // Only handle key press events to avoid duplicate processing
      if key.kind == KeyEventKind::Press {
        app.handle_key(key.code);
      }
    }

    // Check if we should exit
    if app.should_exit {
      return Ok(());
    }
  }
}
