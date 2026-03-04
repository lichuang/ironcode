mod app;

use anyhow::Result;
use app::App;
use crossterm::{
  event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
  Frame, Terminal,
  backend::{Backend, CrosstermBackend},
  layout::{Constraint, Direction, Layout},
  style::{Color, Style},
  text::Text,
  widgets::{Block, Borders, Paragraph, Wrap},
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
    terminal.draw(|f| draw_ui(f, app))?;

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

/// Draw the user interface
fn draw_ui(f: &mut Frame, app: &App) {
  // Create vertical layout: messages on top, input at bottom
  let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Min(3),    // Message display area, minimum height 3
      Constraint::Length(3), // Input area, fixed height 3
    ])
    .split(f.area());

  // Render message display area
  let messages_text = if app.messages.is_empty() {
    Text::from("Press ESC to exit, type a message and press Enter to send\n")
  } else {
    Text::from(app.messages.join("\n"))
  };

  let messages_widget = Paragraph::new(messages_text)
    .block(
      Block::default()
        .title(" Talos - AI Coding Agent ")
        .borders(Borders::ALL),
    )
    .wrap(Wrap { trim: true });

  f.render_widget(messages_widget, chunks[0]);

  // Render input area
  let input_widget = Paragraph::new(app.input.as_str())
    .block(Block::default().title(" Input ").borders(Borders::ALL))
    .style(Style::default().fg(Color::Yellow));

  f.render_widget(input_widget, chunks[1]);

  // Set cursor position so user can see where they are typing
  // +2 for left border and padding
  let cursor_x = chunks[1].x + app.cursor_position as u16 + 2;
  let cursor_y = chunks[1].y + 1;
  f.set_cursor_position((cursor_x, cursor_y));
}
