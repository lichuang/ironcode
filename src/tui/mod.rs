//! Terminal UI infrastructure.
//!
//! Provides the building blocks for an async event-driven terminal interface,
//! including frame scheduling and unified event handling.

use std::io::Stdout;
use std::io::stdout;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::broadcast;

mod event_stream;
mod frame_rate_limiter;
mod frame_requester;
mod message;
mod message_broker;

pub use event_stream::TuiEventBroker;
pub use event_stream::TuiEventStream;
pub use frame_requester::FrameRequester;
pub use message::UiMessage;
pub use message_broker::MessageBroker;

/// Target frame interval for UI redraw scheduling (120 FPS max).
pub const TARGET_FRAME_INTERVAL: std::time::Duration = frame_rate_limiter::MIN_FRAME_INTERVAL;

/// Events that can be processed by the TUI event loop.
#[derive(Debug, Clone)]
pub enum TuiEvent {
  /// A keyboard event.
  Key(KeyEvent),
  /// A request to redraw the UI.
  Draw,
  /// Text pasted into the terminal.
  Paste(String),
}

/// The TUI instance, holding the terminal and frame scheduling infrastructure.
pub struct Tui {
  terminal: Terminal<CrosstermBackend<Stdout>>,
  frame_requester: FrameRequester,
  draw_tx: broadcast::Sender<()>,
  event_broker: Arc<TuiEventBroker>,
  terminal_focused: Arc<AtomicBool>,
}

impl Tui {
  /// Create a new TUI instance.
  pub fn new() -> Result<Self> {
    let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let event_broker = Arc::new(TuiEventBroker::new());
    let terminal_focused = Arc::new(AtomicBool::new(true));

    // Create a broadcast channel for draw events
    let (draw_tx, _draw_rx) = broadcast::channel(16);
    let frame_requester = FrameRequester::new(draw_tx.clone());

    Ok(Self {
      terminal,
      frame_requester,
      draw_tx,
      event_broker,
      terminal_focused,
    })
  }

  /// Get a clone of the frame requester handle.
  pub fn frame_requester(&self) -> FrameRequester {
    self.frame_requester.clone()
  }

  /// Create a new event stream for polling TUI events.
  pub fn create_event_stream(&self) -> TuiEventStream {
    TuiEventStream::new(
      self.event_broker.clone(),
      self.draw_tx.subscribe(),
      self.terminal_focused.clone(),
    )
  }

  /// Get a reference to the terminal.
  #[allow(dead_code)]
  pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
    &mut self.terminal
  }

  /// Draw the UI using the provided function.
  pub fn draw<F>(&mut self, f: F) -> Result<()>
  where
    F: FnOnce(&mut ratatui::Frame),
  {
    self.terminal.draw(f)?;
    Ok(())
  }
}

/// Initialize the terminal for TUI mode.
pub fn init_terminal() -> Result<()> {
  crossterm::terminal::enable_raw_mode()?;
  crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
  Ok(())
}

/// Restore the terminal to normal mode.
pub fn restore_terminal() -> Result<()> {
  crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
  crossterm::terminal::disable_raw_mode()?;
  Ok(())
}
