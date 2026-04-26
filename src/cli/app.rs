use std::path::Path;
use std::sync::Arc;

use crossterm::event::KeyEvent;
use log::{error, info};
use ratatui::Frame;

use crate::cli::Args;
use crate::cli::runtime::Runtime;
use crate::config::global_config;
use crate::error::Result;
use crate::llm::{ChatSession, SessionEvent};
use crate::session::{SessionMode, SessionStore};
use crate::tui::FrameRequester;
use crate::view::chat::{
  ChatMessage, StreamingChunk, SystemMessageLevel, ToolCallStatus, llm_messages_to_chat_history,
};
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

  /// Precise token count from API usage (if available)
  pub(crate) precise_token_count: Option<u32>,
  /// Compaction warning state (token threshold approaching)
  pub(crate) compaction_warning: Option<CompactionWarning>,
  /// Pending tool call awaiting user approval
  pub(crate) pending_approval: Option<PendingApproval>,
}

/// Compaction warning information
#[derive(Debug, Clone)]
pub struct CompactionWarning {
  /// Current estimated token count
  pub current_tokens: usize,
  /// Token threshold that triggered the warning
  #[allow(dead_code)]
  pub threshold: usize,
  /// Maximum context size for the model
  pub max_context_size: usize,
}

/// Pending tool call awaiting user approval
#[derive(Debug, Clone)]
pub struct PendingApproval {
  /// Tool call ID
  pub tool_call_id: String,
  /// Tool name
  pub name: String,
  /// Tool arguments
  #[allow(dead_code)]
  pub arguments: String,
  /// Optional diff preview for file-modifying tools
  pub diff_preview: Option<String>,
}

impl AppData {
  /// Create a new app data instance
  pub fn new() -> Self {
    Self {
      should_exit: false,
      chat_history: Vec::new(),
      streaming_response: Arc::new(Vec::new()),

      precise_token_count: None,
      compaction_warning: None,
      pending_approval: None,
    }
  }
}

impl Default for AppData {
  fn default() -> Self {
    Self::new()
  }
}

impl CompactionWarning {
  /// Calculate usage percentage
  pub fn usage_percentage(&self) -> usize {
    (self.current_tokens as f64 / self.max_context_size as f64 * 100.0) as usize
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

  /// Chat session for LLM communication (initialized when first chat starts)
  chat_session: Option<ChatSession>,
  /// Current LLM response chunks being accumulated (for streaming display)
  /// Uses Arc for cheap cloning when sharing with AppData.
  current_chunks: Arc<Vec<StreamingChunk>>,
}

impl App {
  /// Create a new app instance
  ///
  /// # Arguments
  /// * `data_dir` - The data directory for loading system prompt (data_dir/prompts/system.md)
  /// * `args` - Command line arguments for session control
  /// * `session_store` - Persistent session storage
  pub fn new(data_dir: &Path, args: &Args, session_store: Arc<SessionStore>) -> Result<Self> {
    let runtime = Runtime::new(data_dir)?;
    let mut data = AppData::new();

    let system_prompt = runtime.render_system_prompt();

    let mode = if let Some(id) = &args.session {
      SessionMode::ResumeById(id.clone())
    } else if args.r#continue {
      SessionMode::ResumeLatest
    } else {
      SessionMode::New
    };

    let (chat_session, messages) =
      ChatSession::create_or_resume(global_config(), system_prompt, session_store, mode)?;

    data.chat_history = llm_messages_to_chat_history(&messages);
    let session_handle = chat_session.handle.clone();

    // Create ChatView directly
    let chat_view = ChatView::new(&data, session_handle);

    Ok(Self {
      data,
      view: Box::new(chat_view),
      frame_requester: None,
      runtime,
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
              status: ToolCallStatus::Running,
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
              *status = ToolCallStatus::Completed;
              tool_args = Some(arguments.clone());
            }
            // Add completed tool call to chat history so it persists
            if let Some(args) = tool_args {
              // Only show diff/output for file-modifying tools in chat history
              let display_output = match name.as_str() {
                "WriteFile" | "ReplaceFile" => Some(output.clone()),
                _ => None,
              };
              self.data.chat_history.push(ChatMessage::ToolCall {
                name: name.clone(),
                arguments: args,
                output: display_output,
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
          SessionEvent::StreamInterrupted { .. } => {
            // The actor is retrying; clear our partial streaming state so we
            // stay consistent and don't concatenate old chunks with the retry.
            self.current_chunks = Arc::new(Vec::new());
            self.data.streaming_response = Arc::new(Vec::new());
          }
          SessionEvent::ApprovalNeeded {
            id,
            name,
            arguments,
            diff_preview,
          } => {
            self.data.pending_approval = Some(PendingApproval {
              tool_call_id: id,
              name,
              arguments,
              diff_preview,
            });
          }
          SessionEvent::Error(err) => {
            // Log error and clear any partial response
            error!("LLM stream error: {}", err);
            self.current_chunks = Arc::new(Vec::new());
            self.data.streaming_response = Arc::new(Vec::new());
            // Show a user-friendly system message in the chat history
            self.data.chat_history.push(ChatMessage::System {
              content: err,
              level: SystemMessageLevel::Error,
            });
          }
          SessionEvent::Shutdown => {
            // Session has been shutdown
            info!("ChatSession {} shutdown", session.handle.id);
          }
          SessionEvent::Usage {
            total_tokens,
            prompt_tokens,
            completion_tokens,
          } => {
            log::info!(
              "App: Received precise token usage - total={}, prompt={}, completion={}",
              total_tokens,
              prompt_tokens,
              completion_tokens
            );
            // Store the precise token count for status bar display
            self.data.precise_token_count = Some(total_tokens);
          }
          SessionEvent::CompactionNeeded {
            current_tokens,
            threshold,
            max_context_size,
          } => {
            log::info!(
              "App: Compaction needed - {} tokens (threshold: {}, max: {})",
              current_tokens,
              threshold,
              max_context_size
            );
            // Store compaction warning for UI display
            self.data.compaction_warning = Some(CompactionWarning {
              current_tokens,
              threshold,
              max_context_size,
            });
            // Add system notification to chat history
            let percentage = (current_tokens as f64 / max_context_size as f64 * 100.0) as usize;
            let message = format!(
              "⚠️ Context usage is at {}% ({} / {} tokens). Consider starting a new session soon.",
              percentage, current_tokens, max_context_size
            );
            self.data.chat_history.push(ChatMessage::System {
              content: message,
              level: SystemMessageLevel::Warning,
            });
          }
          SessionEvent::CompactionCompleted {
            message_count_before,
            message_count_after,
            new_token_count,
          } => {
            log::info!(
              "App: Compaction completed - {} -> {} messages, ~{} tokens",
              message_count_before,
              message_count_after,
              new_token_count
            );
            // Clear the warning since we've compacted
            self.data.compaction_warning = None;
            // Add system notification
            let message = format!(
              "🗜️ Context compacted: {} messages -> {} messages (~{} tokens)",
              message_count_before, message_count_after, new_token_count
            );
            self.data.chat_history.push(ChatMessage::System {
              content: message,
              level: SystemMessageLevel::Info,
            });
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
