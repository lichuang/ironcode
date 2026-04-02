use std::path::Path;
use std::sync::Arc;

use crossterm::event::KeyEvent;
use log::{error, info};
use ratatui::Frame;

use crate::cli::runtime::Runtime;
use crate::config::Config;
use crate::error::Result;
use crate::llm::{ChatSession, SessionEvent};
use crate::tui::FrameRequester;
use crate::view::chat::{ChatMessage, StreamingChunk};
use crate::view::{ChatView, View};

/// Application data that can be modified by views
pub struct AppData {
  /// Whether the app should exit
  pub(crate) should_exit: bool,
  /// Complete chat history (user messages and AI responses)
  pub(crate) chat_history: Vec<ChatMessage>,
  /// Current streaming content chunks from LLM (for real-time display)
  /// Contains both normal and thinking content. Empty when not streaming.
  /// Uses Arc for cheap cloning when sharing between App and ChatView.
  pub(crate) streaming_response: Arc<Vec<StreamingChunk>>,
  /// Application configuration (shared with views)
  pub(crate) config: Option<Config>,
}

impl AppData {
  /// Create a new app data instance
  pub fn new() -> Self {
    Self {
      should_exit: false,
      chat_history: Vec::new(),
      streaming_response: Arc::new(Vec::new()),
      config: None,
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
  /// Runtime data loaded at startup
  #[allow(dead_code)]
  pub(crate) runtime: Runtime,
  /// Application configuration
  #[allow(dead_code)]
  pub(crate) config: Config,
  /// Chat session for LLM communication (initialized when first chat starts)
  chat_session: Option<ChatSession>,
  /// Current LLM response chunks being accumulated (for streaming display)
  /// Uses Arc for cheap cloning when sharing with AppData.
  current_chunks: Arc<Vec<StreamingChunk>>,
}

impl App {
  /// Create a new app instance with the given configuration
  ///
  /// # Arguments
  /// * `config` - The loaded configuration
  /// * `data_dir` - The data directory for loading system prompt (data_dir/prompts/system.md)
  pub fn new(config: Config, data_dir: &Path) -> Result<Self> {
    let runtime = Runtime::new(data_dir)?;
    let mut data = AppData::new();
    data.config = Some(config.clone());

    // Initialize chat session immediately
    let system_prompt = runtime.render_system_prompt();
    let chat_session = ChatSession::create(
      &config,
      system_prompt,
      runtime.tool_registry.clone(),
      runtime.executable_tool_registry.clone(),
    )?;
    let session_handle = chat_session.handle.clone();

    // Create ChatView directly
    let chat_view = ChatView::new(&data, session_handle, &config);

    Ok(Self {
      data,
      view: Box::new(chat_view),
      frame_requester: None,
      runtime,
      config,
      chat_session: Some(chat_session),
      current_chunks: Arc::new(Vec::new()),
    })
  }

  pub fn should_exit(&self) -> bool {
    self.data.should_exit
  }

  /// Handle keyboard events
  pub fn handle_key(&mut self, key: KeyEvent) {
    if let Some(new_view) = self.view.handle_key(&mut self.data, key) {
      // View wants to switch - just switch to the new view
      // (currently only ChatView is used, and it returns None on view switches)
      self.view = new_view;

      // Re-set frame requester when view changes
      if let Some(ref frame_requester) = self.frame_requester {
        self.view.set_frame_requester(frame_requester.clone());
      }
    }
  }

  /// Draw the current view
  pub fn draw(&mut self, f: &mut Frame) {
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

  /// Update chat session state and process any pending events
  ///
  /// This should be called regularly in the main event loop to:
  /// 1. Send pending user messages to LLM
  /// 2. Process streaming responses from the LLM
  ///
  /// Returns true if any updates were processed.
  pub fn update_chat_session(&mut self) -> bool {
    let mut updated = false;

    if let Some(ref mut session) = self.chat_session {
      // Process all pending events from LLM
      while let Some(event) = session.poll_event() {
        updated = true;
        match event {
          SessionEvent::ContentChunk(chunk) => {
            log::debug!(
              "App: Received ContentChunk, len={}, content={}",
              chunk.len(),
              &chunk[..chunk.len().min(100)]
            );
            // Add normal content chunk - make_mut to clone only if needed
            Arc::make_mut(&mut self.current_chunks).push(StreamingChunk::Normal(chunk));
            // Update streaming response for UI display - cheap Arc clone
            self.data.streaming_response = self.current_chunks.clone();
          }
          SessionEvent::ThinkingChunk(chunk) => {
            log::info!(
              "App: Received ThinkingChunk, len={}, content={}",
              chunk.len(),
              &chunk[..chunk.len().min(100)]
            );
            // Add thinking content chunk - make_mut to clone only if needed
            Arc::make_mut(&mut self.current_chunks).push(StreamingChunk::Thinking(chunk));
            // Update streaming response for UI display - cheap Arc clone
            self.data.streaming_response = self.current_chunks.clone();
          }
          SessionEvent::ToolCallReceived {
            id,
            name,
            arguments,
          } => {
            log::info!(
              "App: Tool call received: id={}, name={}, args={}",
              id,
              name,
              arguments
            );
            // Add tool call indicator to streaming response
            Arc::make_mut(&mut self.current_chunks).push(StreamingChunk::ToolCall {
              name: name.clone(),
              arguments,
              status: crate::view::chat::ToolCallStatus::Running,
            });
            self.data.streaming_response = self.current_chunks.clone();
          }
          SessionEvent::ToolCallCompleted { name, output } => {
            log::info!(
              "App: Tool call completed: name={}, output_len={}",
              name,
              output.len()
            );
            // Update the last tool call chunk to completed status
            let chunks = Arc::make_mut(&mut self.current_chunks);
            let mut tool_args = None;
            if let Some(StreamingChunk::ToolCall {
              name: n,
              arguments,
              status,
              ..
            }) = chunks.last_mut()
              && n == &name
            {
              *status = crate::view::chat::ToolCallStatus::Completed;
              tool_args = Some(arguments.clone());
            }
            // Add completed tool call to chat history so it persists
            if let Some(args) = tool_args {
              self.data.chat_history.push(ChatMessage::ToolCall {
                name: name.clone(),
                arguments: args,
              });
            }
            self.data.streaming_response = self.current_chunks.clone();
          }
          SessionEvent::Completed => {
            // Extract normal and thinking content from chunks
            let normal_content: String = self
              .current_chunks
              .iter()
              .filter_map(|c| match c {
                StreamingChunk::Normal(s) => Some(s.as_str()),
                _ => None,
              })
              .collect();
            let thinking_content: String = self
              .current_chunks
              .iter()
              .filter_map(|c| match c {
                StreamingChunk::Thinking(s) => Some(s.as_str()),
                _ => None,
              })
              .collect();
            log::info!(
              "App: Stream completed, normal_len={}, thinking_len={}",
              normal_content.len(),
              thinking_content.len()
            );
            // Stream completed - save AI response to chat history
            if !normal_content.is_empty() || !thinking_content.is_empty() {
              // Add AI response to chat history (with thinking content if any)
              self.data.chat_history.push(ChatMessage::Assistant {
                content: normal_content,
                thinking_content: if thinking_content.is_empty() {
                  None
                } else {
                  Some(thinking_content)
                },
              });
            }
            // Clear streaming state
            self.data.streaming_response = Arc::new(Vec::new());
            self.current_chunks = Arc::new(Vec::new());
          }
          SessionEvent::Error(err) => {
            // Log error and clear any partial response
            error!("LLM stream error: {}", err);
            self.current_chunks = Arc::new(Vec::new());
            self.data.streaming_response = Arc::new(Vec::new());
          }
          SessionEvent::Shutdown => {
            // Session has been shutdown
            info!("ChatSession {} shutdown", session.handle.id);
          }
        }
      }

      // Trigger redraw if there were updates
      if updated && let Some(ref fr) = self.frame_requester {
        fr.schedule_frame();
      }
    }

    updated
  }
}
