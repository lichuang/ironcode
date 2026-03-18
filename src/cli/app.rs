use crate::cli::runtime::Runtime;
use crate::config::Config;
use crate::error::Result;
use crate::llm::{ChatSession, SessionEvent, SessionHandle};
use crate::tui::{FrameRequester, MessageBroker, UiMessage};
use crate::view::{ChatView, HomeView, View};
use crossterm::event::KeyEvent;
use log::{error, info};
use std::path::PathBuf;

/// Application data that can be modified by views
pub struct AppData {
  /// Whether the app should exit
  pub(crate) should_exit: bool,
  /// Message history (for chat)
  pub(crate) messages: Vec<String>,
  /// Flag indicating that a new chat session should be initialized
  /// (set by views when switching to chat, cleared by App after initialization)
  pub(crate) init_session_requested: bool,
  /// First user message to send after session is initialized
  pub(crate) pending_first_message: Option<String>,
  /// Error message to display in the UI (e.g., session initialization failed)
  pub(crate) error_message: Option<String>,
  /// Current streaming response from LLM (for real-time display)
  pub(crate) streaming_response: Option<String>,
  /// Last completed AI response (for display after streaming ends)
  pub(crate) last_ai_response: Option<String>,
}

impl AppData {
  /// Create a new app data instance
  pub fn new() -> Self {
    Self {
      should_exit: false,
      messages: Vec::new(),
      init_session_requested: false,
      pending_first_message: None,
      error_message: None,
      streaming_response: None,
      last_ai_response: None,
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
  /// Chat session for LLM communication (initialized when first chat starts)
  chat_session: Option<ChatSession>,
  /// Current LLM response being accumulated (for streaming display)
  current_response: String,
}

impl App {
  /// Create a new app instance with the given configuration
  ///
  /// # Arguments
  /// * `config` - The loaded configuration
  /// * `config_dir` - The directory where config was loaded from (for loading system prompt)
  pub fn new(config: Config, config_dir: &PathBuf) -> Result<Self> {
    let runtime = Runtime::new(config_dir)?;

    Ok(Self {
      data: AppData::new(),
      view: Box::new(HomeView::new()),
      frame_requester: None,
      message_broker: MessageBroker::new(),
      runtime,
      config,
      chat_session: None,
      current_response: String::new(),
    })
  }

  pub fn should_exit(&self) -> bool {
    self.data.should_exit
  }

  /// Handle keyboard events
  pub fn handle_key(&mut self, key: KeyEvent) {
    if let Some(new_view) = self.view.handle_key(&mut self.data, key) {
      // Check if we need to initialize a chat session
      if self.data.init_session_requested {
        // Try to initialize session first before switching to chat view
        if let Err(e) = self.init_chat_session_from_runtime() {
          // Initialization failed - show error in UI and stay in current view
          let err_msg = format!("Failed to initialize chat session: {}", e);
          error!("{}", err_msg);
          self.data.error_message = Some(err_msg);
          self.data.init_session_requested = false;
          // Don't switch view - stay in HomeView to show the error
          return;
        }
        // Initialization succeeded - clear any previous error and switch to ChatView
        self.data.error_message = None;
        self.data.init_session_requested = false;
        self.view = Box::new(ChatView::new(&self.data));
      } else {
        // Normal view switch - clear error message
        self.data.error_message = None;
        self.view = new_view;
      }

      // Re-set frame requester when view changes
      if let Some(ref frame_requester) = self.frame_requester {
        self.view.set_frame_requester(frame_requester.clone());
      }
    }
  }

  /// Draw the current view
  pub fn draw(&mut self, f: &mut ratatui::Frame) {
    self.view.draw(f, &self.data);
  }

  /// Called when a new frame is about to be rendered
  pub fn on_frame(&mut self, frame_requester: &FrameRequester) {
    self.view.on_frame(frame_requester, &self.data);
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

  /// Update chat session state and process any pending events
  ///
  /// This should be called regularly in the main event loop to
  /// process streaming responses from the LLM.
  ///
  /// Returns true if any updates were processed.
  pub fn update_chat_session(&mut self) -> bool {
    let mut updated = false;

    if let Some(ref mut session) = self.chat_session {
      // Process all pending events
      while let Some(event) = session.poll_event() {
        updated = true;
        match event {
          SessionEvent::ContentChunk(chunk) => {
            // Accumulate content for streaming display
            self.current_response.push_str(&chunk);
            // Update streaming response for UI display
            self.data.streaming_response = Some(self.current_response.clone());
          }
          SessionEvent::Completed => {
            // Stream completed - save AI response separately from user messages
            if !self.current_response.is_empty() {
              info!(
                "LLM response completed, len={}",
                self.current_response.len()
              );
              log::debug!(
                "AI response content (first 100 chars): {}",
                &self.current_response[..self.current_response.len().min(100)]
              );
              self.data.last_ai_response = Some(self.current_response.clone());
            }
            // Clear streaming state
            self.data.streaming_response = None;
            self.current_response.clear();
          }
          SessionEvent::Error(err) => {
            // Log error and clear any partial response
            error!("LLM stream error: {}", err);
            self.current_response.clear();
            self.data.streaming_response = None;
          }
          SessionEvent::Shutdown => {
            // Session has been shutdown
            info!("ChatSession {} shutdown", session.handle.id);
          }
        }
      }

      // Trigger redraw if there were updates
      if updated {
        if let Some(ref fr) = self.frame_requester {
          fr.schedule_frame();
        }
      }
    }

    updated
  }

  /// Get the session handle if initialized
  pub fn session_handle(&self) -> Option<&SessionHandle> {
    self.chat_session.as_ref().map(|s| &s.handle)
  }

  /// Check if session has pending events
  pub fn session_has_event(&self) -> bool {
    self
      .chat_session
      .as_ref()
      .map(|s| s.has_event())
      .unwrap_or(false)
  }

  /// Initialize the chat session using runtime system prompt
  ///
  /// This is called when transitioning from HomeView to ChatView
  pub fn init_chat_session_from_runtime(&mut self) -> Result<()> {
    assert!(self.chat_session.is_none());

    // Get system prompt from runtime
    let system_prompt = self.runtime.render_system_prompt();

    // Create session from config and system prompt
    self.chat_session = Some(ChatSession::from_config(&self.config, system_prompt)?);

    // Send pending first message if exists
    if let Some(first_message) = self.data.pending_first_message.take() {
      // Add user message to history so it appears in ChatView
      self.data.messages.push(first_message.clone());
      self.send_to_llm(first_message);
    }

    Ok(())
  }

  /// Send a message to the LLM (non-blocking, queued)
  ///
  /// Returns true if the message was queued
  pub fn send_to_llm(&mut self, content: impl Into<String>) -> bool {
    if let Some(ref session) = self.chat_session {
      session.handle.send_message(content);
      true
    } else {
      error!("receive input but no active session");
      false
    }
  }

  /// Cancel the current LLM request if any
  pub fn cancel_llm_request(&self) {
    if let Some(ref session) = self.chat_session {
      session.handle.cancel();
    }
  }

  /// Shutdown the chat session
  pub fn shutdown_chat_session(&self) {
    if let Some(ref session) = self.chat_session {
      session.handle.shutdown();
    }
  }
}
