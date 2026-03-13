use crate::cli::runtime::Runtime;
use crate::config::Config;
use crate::error::Result;
use crate::tui::{FrameRequester, MessageBroker, UiMessage};
use crate::view::{HomeView, View};

/// Application data that can be modified by views
pub struct AppData {
  /// Whether the app should exit
  pub(crate) should_exit: bool,
  /// Message history (for chat)
  pub(crate) messages: Vec<String>,
}

impl AppData {
  /// Create a new app data instance
  pub fn new() -> Self {
    Self {
      should_exit: false,
      messages: Vec::new(),
    }
  }
}

impl Default for AppData {
  fn default() -> Self {
    Self::new()
  }
}

/// Application state
pub struct App {
  /// Application data
  data: AppData,
  /// Current view (dynamic dispatch)
  pub view: Box<dyn View>,
  /// Frame requester for animation scheduling
  frame_requester: Option<FrameRequester>,
  /// Message broker for UI communication
  message_broker: MessageBroker,
  /// Runtime data loaded at startup
  pub(crate) runtime: Runtime,
  /// Application configuration
  pub(crate) config: Config,
}

impl App {
  /// Create a new app instance with the given configuration
  pub fn new(config: Config) -> Result<Self> {
    let runtime = Runtime::new()?;

    Ok(Self {
      data: AppData::new(),
      view: Box::new(HomeView::new()),
      frame_requester: None,
      message_broker: MessageBroker::new(),
      runtime,
      config,
    })
  }

  pub fn should_exit(&self) -> bool {
    self.data.should_exit
  }

  /// Handle keyboard events
  pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
    if let Some(new_view) = self.view.handle_key(&mut self.data, key) {
      self.view = new_view;
      // Re-set frame requester when view changes
      if let Some(ref frame_requester) = self.frame_requester {
        self.view.set_frame_requester(frame_requester.clone());
      }
    }
  }

  /// Draw the current view
  pub fn draw(&self, f: &mut ratatui::Frame) {
    self.view.draw(f, &self.data);
  }

  /// Called when a new frame is about to be rendered
  pub fn on_frame(&mut self, frame_requester: &FrameRequester) {
    self.view.on_frame(frame_requester);
  }

  /// Set the frame requester for the current view
  pub fn set_frame_requester(&mut self, frame_requester: FrameRequester) {
    self.frame_requester = Some(frame_requester.clone());
    self.view.set_frame_requester(frame_requester);
  }

  /// Handle an incoming UI message
  ///
  /// This is called by the main event loop to process messages
  /// from background tasks.
  pub fn handle_message(&mut self, msg: UiMessage) {
    match msg {
      UiMessage::AppendChat { content } => {
        self.data.messages.push(content);
      }
    }
    // Trigger a redraw after handling the message
    if let Some(ref fr) = self.frame_requester {
      fr.schedule_frame();
    }
  }

  /// Get a clone of the message sender for background tasks
  pub fn message_sender(&self) -> tokio::sync::mpsc::UnboundedSender<UiMessage> {
    self.message_broker.sender()
  }

  /// Try to receive a pending message from the queue
  ///
  /// Returns `Some(msg)` if available, `None` otherwise.
  /// This should be called in the main event loop.
  pub fn try_recv_message(&mut self) -> Option<UiMessage> {
    self.message_broker.try_recv()
  }
}
