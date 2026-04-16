//! Chat session management module (Actor pattern).
//!
//! Manages a conversation with an LLM using the actor pattern.
//! The session runs in a dedicated tokio task and communicates via channels.

use std::env;
use std::future::pending;
use std::mem::take;
use std::path::PathBuf;
use std::sync::Arc;

use async_openai::types::chat::{
  ChatCompletionMessageToolCallChunk, ChatCompletionResponseStream, FunctionCallStream,
};
use chrono::{Datelike, Local, Timelike};
use log::{debug, error, info, warn};
use tokio::sync::mpsc;

use crate::config::{CompactionConfig, Config, DEFAULT_MAX_CONTEXT_SIZE, RetryConfig};
use crate::error::{ConfigError, Result};
use crate::llm::compaction::{Compaction, calculate_threshold, should_auto_compact};
use crate::llm::provider::LLMProvider;
use crate::llm::providers::KimiProvider;
use crate::llm::types::{ChatConfig, Message, Role, ToolCall};
use crate::session::{SessionMeta, SessionMode, SessionStore};
use crate::tools::{ExecutableToolRegistry, ToolInvocation, ToolPayload, ToolRegistry};
use crate::utils::token_counter::estimate_llm_messages_tokens;

/// Maximum characters to display in user input log preview
const USER_INPUT_PREVIEW_LEN: usize = 50;

/// Generate a session ID based on timestamp and current directory
/// Format: dirname-YYYY.MM.DD:HH.MM.SS.microseconds
pub(crate) fn generate_session_id() -> String {
  let now = Local::now();

  // Get current directory name
  let dir_name = env::current_dir()
    .ok()
    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
    .unwrap_or_else(|| "unknown".to_string());

  format!(
    "{}-{:04}.{:02}.{:02}:{:02}.{:02}.{:02}.{:06}",
    dir_name,
    now.year(),
    now.month(),
    now.day(),
    now.hour(),
    now.minute(),
    now.second(),
    now.timestamp_subsec_micros()
  )
}

/// Commands sent to the session actor
#[derive(Debug)]
#[allow(dead_code)]
pub enum SessionCommand {
  /// Send a user message
  SendMessage { content: String },
  /// Cancel the current streaming request
  Cancel,
  /// Clear conversation history
  ClearHistory,
  /// Shutdown the session actor
  Shutdown,
}

/// Events emitted by the chat session
#[derive(Debug, Clone)]
pub enum SessionEvent {
  /// A chunk of content received from the stream
  ContentChunk(String),
  /// A chunk of thinking/reasoning content received from the stream
  ThinkingChunk(String),
  /// A tool call was received from the model
  ToolCallReceived {
    id: String,
    name: String,
    arguments: String,
  },
  /// A tool execution completed
  ToolCallCompleted { name: String, output: String },
  /// Stream completed successfully
  Completed,
  /// Error occurred during streaming
  Error(String),
  /// Session has been shutdown
  Shutdown,
  /// Compaction is needed (token limit approaching)
  CompactionNeeded {
    /// Current estimated token count
    current_tokens: usize,
    /// Token threshold that triggered this notification
    threshold: usize,
    /// Maximum context size for the current model
    max_context_size: usize,
  },
  /// Compaction completed (messages were compacted)
  CompactionCompleted {
    /// Number of messages before compaction
    message_count_before: usize,
    /// Number of messages after compaction
    message_count_after: usize,
    /// New estimated token count
    new_token_count: usize,
  },
  /// Token usage information from the API (sent when stream finishes)
  Usage {
    /// Total tokens in the conversation (prompt + completion)
    total_tokens: u32,
    /// Tokens in the prompt (input)
    prompt_tokens: u32,
    /// Tokens in the completion (output)
    completion_tokens: u32,
  },
}

/// Handle to interact with a running session actor
#[derive(Debug, Clone)]
pub struct SessionHandle {
  /// Session ID
  pub id: String,
  /// Channel to send commands to the session
  cmd_tx: mpsc::UnboundedSender<SessionCommand>,
}

#[allow(dead_code)]
impl SessionHandle {
  /// Send a user message to the session
  pub fn send_message(&self, content: impl Into<String>) {
    let _ = self.cmd_tx.send(SessionCommand::SendMessage {
      content: content.into(),
    });
  }

  /// Cancel the current streaming request
  pub fn cancel(&self) {
    let _ = self.cmd_tx.send(SessionCommand::Cancel);
  }

  /// Clear conversation history
  pub fn clear_history(&self) {
    let _ = self.cmd_tx.send(SessionCommand::ClearHistory);
  }

  /// Shutdown the session actor
  pub fn shutdown(&self) {
    let _ = self.cmd_tx.send(SessionCommand::Shutdown);
  }
}

/// Internal state of the session actor
struct SessionActor {
  /// Session ID
  id: String,
  /// The LLM provider
  provider: Box<dyn LLMProvider>,
  /// Message history (including system, user, and assistant messages)
  messages: Vec<Message>,
  /// Channel to send events back to the caller
  event_tx: mpsc::UnboundedSender<SessionEvent>,
  /// Channel to receive commands
  cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
  /// Current response being accumulated
  current_response: String,
  /// Current thinking content being accumulated
  current_thinking: String,
  /// Whether a streaming request is in progress
  is_streaming: bool,
  /// Event receiver for the current stream (if any)
  stream_rx: Option<mpsc::UnboundedReceiver<SessionEvent>>,
  /// Tool call buffer for accumulating tool calls during streaming
  pending_tool_calls: Vec<ToolCall>,
  /// Executable tool registry for handling tool calls (shared)
  tool_registry: Arc<ExecutableToolRegistry>,
  /// Working directory for tool execution
  cwd: PathBuf,
  /// Precise token count from API usage (if available)
  precise_token_count: Option<u32>,
  /// Session store for persistence
  session_store: Arc<SessionStore>,
  /// Session metadata for persistence
  meta: SessionMeta,
  /// Compaction configuration
  compaction_config: CompactionConfig,
  /// Maximum context size for the current model
  max_context_size: usize,
  /// Compaction handler (used for actual compaction)
  compaction: Compaction,
  /// Whether compaction has been notified for current threshold
  compaction_notified: bool,
  /// Retry configuration for LLM requests
  retry_config: RetryConfig,
}

impl SessionActor {
  #[allow(clippy::too_many_arguments)]
  fn new(
    id: String,
    provider: Box<dyn LLMProvider>,
    messages: Vec<Message>,
    event_tx: mpsc::UnboundedSender<SessionEvent>,
    cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    tool_registry: Arc<ExecutableToolRegistry>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    compaction_config: CompactionConfig,
    max_context_size: usize,
    retry_config: RetryConfig,
  ) -> Self {
    Self {
      id,
      provider,
      messages,
      event_tx,
      cmd_rx,
      current_response: String::new(),
      current_thinking: String::new(),
      is_streaming: false,
      stream_rx: None,
      pending_tool_calls: Vec::new(),
      tool_registry,
      cwd: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
      session_store,
      meta,
      precise_token_count: None,
      compaction_config,
      max_context_size,
      compaction: Compaction::default(),
      compaction_notified: false,
      retry_config,
    }
  }

  /// Main actor loop
  async fn run(mut self) {
    info!("SessionActor {} started", self.id);

    loop {
      tokio::select! {
        // Process incoming commands
        Some(cmd) = self.cmd_rx.recv() => {
          if !self.handle_command(cmd).await {
            break;
          }
        }

        // Process streaming events if active
        Some(event) = async {
          match &mut self.stream_rx {
            Some(rx) => rx.recv().await,
            None => pending().await,
          }
        } => {
          self.handle_stream_event(event).await;
        }

        // If no streaming and channel closed, exit
        else => {
          if self.stream_rx.is_none() {
            info!("SessionActor {}: command channel closed, exiting", self.id);
            break;
          }
        }
      }
    }

    info!("SessionActor {} stopped", self.id);
  }

  /// Handle a command from the handle
  /// Returns false if the actor should shutdown
  async fn handle_command(&mut self, cmd: SessionCommand) -> bool {
    match cmd {
      SessionCommand::SendMessage { content } => {
        // Log user input (truncated if too long)
        let preview: String = content.chars().take(USER_INPUT_PREVIEW_LEN).collect();
        let ellipsis = if content.len() > USER_INPUT_PREVIEW_LEN {
          "..."
        } else {
          ""
        };
        info!(
          "Session {}: Received user input: {}{}",
          self.id, preview, ellipsis
        );

        if self.is_streaming {
          error!("Session {}: Cannot send message while streaming", self.id);
          let _ = self.event_tx.send(SessionEvent::Error(
            "Cannot send message while another request is in progress".to_string(),
          ));
          return true;
        }

        // Add user message to history
        let user_msg = Message::user(&content);
        self.messages.push(user_msg.clone());
        let _ = self.session_store.append_message(&self.id, &user_msg);
        self.meta.update_title_from_message(&user_msg);
        self.meta.updated_at = Local::now();
        let _ = self.session_store.update_meta(&self.meta);
        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();

        // Check if compaction is needed before sending
        self.check_and_notify_compaction().await;

        // Log current message history for debugging
        info!("Session {}: Current message history:", self.id);
        for (i, msg) in self.messages.iter().enumerate() {
          let content_preview: String = msg.content.chars().take(100).collect();
          let tool_calls_info = if let Some(ref tc) = msg.tool_calls {
            format!(" [tool_calls: {}]", tc.len())
          } else {
            String::new()
          };
          let tool_call_id_info = if let Some(ref id) = msg.tool_call_id {
            format!(" [tool_call_id: {}]", id)
          } else {
            String::new()
          };
          info!(
            "  [{}] {:?}: {}{}{}",
            i, msg.role, content_preview, tool_calls_info, tool_call_id_info
          );
        }

        // Start streaming
        self.start_chat_stream().await;
        true
      }

      SessionCommand::Cancel => {
        if self.is_streaming {
          info!("Session {}: Cancelling stream", self.id);
          self.stream_rx = None;
          self.is_streaming = false;
          self.current_response.clear();
          self.current_thinking.clear();
        }
        true
      }

      SessionCommand::ClearHistory => {
        info!("Session {}: Clearing history", self.id);
        // Keep only the system message if it exists
        let system_msg = self.messages.first().and_then(|m| {
          if m.role == Role::System {
            Some(m.clone())
          } else {
            None
          }
        });

        self.messages.clear();
        if let Some(sys) = system_msg {
          self.messages.push(sys);
        }

        self.meta.updated_at = Local::now();
        let _ = self.session_store.reset_messages(&self.id, &self.messages);
        let _ = self.session_store.update_meta(&self.meta);

        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();
        if self.is_streaming {
          self.stream_rx = None;
          self.is_streaming = false;
        }
        true
      }

      SessionCommand::Shutdown => {
        info!("Session {}: Shutdown requested", self.id);
        let _ = self.event_tx.send(SessionEvent::Shutdown);
        false
      }
    }
  }

  /// Check if compaction is needed and send notification if so.
  async fn check_and_notify_compaction(&mut self) {
    if self.max_context_size == 0 {
      log::info!("check_and_notify_compaction: max_context_size is 0, skipping");
      return;
    }

    // Estimate current token count
    let current_tokens = if let Some(precise) = self.precise_token_count {
      let recent_tokens =
        estimate_llm_messages_tokens(&self.messages[self.messages.len().saturating_sub(2)..]);
      log::info!(
        "check_and_notify_compaction: precise={}, recent={}, total={}",
        precise,
        recent_tokens,
        precise as usize + recent_tokens
      );
      precise as usize + recent_tokens
    } else {
      let estimated = estimate_llm_messages_tokens(&self.messages);
      log::info!(
        "check_and_notify_compaction: no precise count, estimated={}",
        estimated
      );
      estimated
    };

    log::info!(
      "check_and_notify_compaction: current_tokens={}, max_context_size={}, compaction_config.enabled={}",
      current_tokens,
      self.max_context_size,
      self.compaction_config.enabled
    );

    // Check if we should trigger compaction
    let should_compact = should_auto_compact(
      current_tokens,
      self.max_context_size,
      &self.compaction_config,
    );
    log::info!(
      "check_and_notify_compaction: should_auto_compact={}",
      should_compact
    );

    if should_compact {
      if !self.compaction_notified {
        let threshold = calculate_threshold(self.max_context_size, &self.compaction_config);
        info!(
          "Session {}: Compaction needed - {} tokens (threshold: {})",
          self.id, current_tokens, threshold
        );
        let _ = self.event_tx.send(SessionEvent::CompactionNeeded {
          current_tokens,
          threshold,
          max_context_size: self.max_context_size,
        });
        self.compaction_notified = true;

        // Execute compaction immediately
        self.execute_compaction().await;
      }
    } else {
      // Reset notification flag when below threshold
      self.compaction_notified = false;
    }
  }

  /// Execute compaction on the current messages.
  ///
  /// Uses the configured compaction strategy to compress message history.
  /// Updates the session store and notifies the UI of completion.
  async fn execute_compaction(&mut self) {
    let message_count_before = self.messages.len();

    // Check if compaction should be performed
    if !self.compaction.should_compact(&self.messages) {
      log::info!(
        "Session {}: Compaction strategy decided not to compact ({} messages)",
        self.id,
        message_count_before
      );
      return;
    }

    // Perform compaction
    log::info!("Session {}: Executing compaction...", self.id);
    let result = self.compaction.compact(&self.messages);

    if !result.did_compact {
      log::info!("Session {}: No compaction performed by strategy", self.id);
      return;
    }

    // Update messages
    self.messages = result.messages;
    let message_count_after = self.messages.len();

    // Estimate new token count
    let new_token_count = estimate_llm_messages_tokens(&self.messages);

    log::info!(
      "Session {}: Compaction completed - {} messages -> {} messages, ~{} tokens",
      self.id,
      message_count_before,
      message_count_after,
      new_token_count
    );

    // Save compacted messages to session store
    if let Err(e) = self.session_store.reset_messages(&self.id, &self.messages) {
      error!(
        "Session {}: Failed to save compacted messages: {}",
        self.id, e
      );
    }

    // Notify UI
    let _ = self.event_tx.send(SessionEvent::CompactionCompleted {
      message_count_before,
      message_count_after,
      new_token_count,
    });

    // Reset the notification flag since we've compacted
    self.compaction_notified = false;
  }

  /// Start a chat stream with the current messages, with retry support.
  ///
  /// If the initial request fails with a retryable error (network timeout,
  /// rate limit, server error), it will be retried with exponential backoff
  /// according to the retry configuration.
  async fn start_chat_stream(&mut self) {
    let max_attempts = self.retry_config.max_attempts.max(1);
    let mut last_error: Option<String> = None;

    for attempt in 0..max_attempts {
      match self.provider.chat_stream(self.messages.clone()).await {
        Ok(stream) => {
          if attempt > 0 {
            info!(
              "Session {}: stream started on attempt {}/{}",
              self.id,
              attempt + 1,
              max_attempts
            );
          }
          let (tx, rx) = mpsc::unbounded_channel();
          self.stream_rx = Some(rx);
          self.is_streaming = true;
          tokio::spawn(handle_stream(stream, tx));
          info!("Session {}: Started streaming for message", self.id);
          return;
        }
        Err(e) => {
          let err_string = e.to_string();
          let is_retryable = is_error_retryable(&e);
          let is_last = attempt + 1 >= max_attempts;

          if !is_retryable {
            error!(
              "Session {}: non-retryable error on attempt {}/{}: {}",
              self.id,
              attempt + 1,
              max_attempts,
              err_string
            );
            let _ = self.event_tx.send(SessionEvent::Error(err_string));
            return;
          }

          if is_last {
            error!(
              "Session {}: all {} attempts exhausted, last error: {}",
              self.id, max_attempts, err_string
            );
            let _ = self.event_tx.send(SessionEvent::Error(err_string));
            return;
          }

          let delay = self.retry_config.delay_for_attempt(attempt);
          warn!(
            "Session {}: attempt {}/{} failed ({}), retrying in {:?}",
            self.id,
            attempt + 1,
            max_attempts,
            err_string,
            delay
          );
          last_error = Some(err_string);
          tokio::time::sleep(delay).await;
        }
      }
    }

    // Unreachable, but safety net
    if let Some(err) = last_error {
      let _ = self.event_tx.send(SessionEvent::Error(err));
    }
  }

  /// Handle a streaming event from the LLM
  async fn handle_stream_event(&mut self, event: SessionEvent) {
    match &event {
      SessionEvent::ContentChunk(chunk) => {
        debug!(
          "Session {}: Stream content: len={}, content={}",
          self.id,
          chunk.len(),
          &chunk[..chunk.len().min(100)]
        );
        self.current_response.push_str(chunk);
        // Forward to caller
        if self.event_tx.send(event).is_err() {
          error!("Session {}: Failed to forward ContentChunk", self.id);
        }
      }
      SessionEvent::ThinkingChunk(chunk) => {
        info!(
          "Session {}: Stream thinking received: len={}, content={}",
          self.id,
          chunk.len(),
          &chunk[..chunk.len().min(100)]
        );
        self.current_thinking.push_str(chunk);
        // Forward to caller without storing in session messages
        if self.event_tx.send(event).is_err() {
          error!("Session {}: Failed to forward ThinkingChunk", self.id);
        }
      }
      SessionEvent::ToolCallReceived {
        id,
        name,
        arguments,
      } => {
        info!(
          "Session {}: Tool call received: id={}, name={}, args={}",
          self.id, id, name, arguments
        );

        // Add to pending tool calls
        self
          .pending_tool_calls
          .push(ToolCall::new(id, name, arguments));

        // Forward to caller
        if self.event_tx.send(event.clone()).is_err() {
          error!("Session {}: Failed to forward ToolCallReceived", self.id);
        }
      }
      SessionEvent::ToolCallCompleted { name, output } => {
        info!(
          "Session {}: Tool call completed: name={}, output_len={}",
          self.id,
          name,
          output.len()
        );
        // Forward to caller
        if self.event_tx.send(event.clone()).is_err() {
          error!("Session {}: Failed to forward ToolCallCompleted", self.id);
        }
      }
      SessionEvent::Usage {
        total_tokens,
        prompt_tokens,
        completion_tokens,
      } => {
        info!(
          "Session {}: Precise token usage - total={}, prompt={}, completion={}",
          self.id, total_tokens, prompt_tokens, completion_tokens
        );
        // Store the precise token count
        self.precise_token_count = Some(*total_tokens);
        // Check compaction with precise token count
        self.check_and_notify_compaction().await;
        // Forward to caller
        if self.event_tx.send(event.clone()).is_err() {
          error!("Session {}: Failed to forward Usage", self.id);
        }
      }
      SessionEvent::Completed => {
        // Add the complete assistant message to history (with tool calls if any)
        let response = take(&mut self.current_response);
        let thinking = take(&mut self.current_thinking);
        let tool_calls = take(&mut self.pending_tool_calls);

        let has_content = !response.is_empty() || !thinking.is_empty();
        let has_tool_calls = !tool_calls.is_empty();

        if has_content || has_tool_calls {
          // Build message content (include thinking if present)
          let content = if !thinking.is_empty() {
            format!("<think>{}</think>{}", thinking, response)
          } else {
            response
          };

          // Create assistant message with or without tool calls
          let assistant_msg = if has_tool_calls {
            Message::assistant_with_tools(content, tool_calls)
          } else {
            Message::assistant(content)
          };

          self.messages.push(assistant_msg.clone());
          let _ = self.session_store.append_message(&self.id, &assistant_msg);
          self.meta.updated_at = Local::now();
          let _ = self.session_store.update_meta(&self.meta);
          info!(
            "Session {}: Added assistant message, content_len={}, tool_calls={}",
            self.id,
            assistant_msg.content.len(),
            has_tool_calls
          );
        }

        self.is_streaming = false;
        self.stream_rx = None;

        // Check if we have tool calls to execute
        if let Some(msg) = self.messages.last()
          && msg.tool_calls.is_some()
          && !msg.tool_calls.as_ref().unwrap().is_empty()
        {
          info!("Session {}: Executing tool calls", self.id);
          self.execute_tool_calls().await;
          return; // Don't send Completed yet, we'll continue after tool execution
        }

        // Forward to caller
        if self.event_tx.send(event).is_err() {
          error!("Session {}: Failed to forward Completed event", self.id);
        }
        info!("Session {}: Stream completed", self.id);
      }
      SessionEvent::Error(err) => {
        error!("Session {}: Stream error: {}", self.id, err);
        self.is_streaming = false;
        self.stream_rx = None;
        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();
        // Forward to caller
        if self.event_tx.send(event).is_err() {
          error!("Session {}: Failed to forward Error event", self.id);
        }
      }
      SessionEvent::Shutdown => {
        // Should not happen, but handle it
        if self.event_tx.send(event).is_err() {
          error!("Session {}: Failed to forward Shutdown event", self.id);
        }
      }
      SessionEvent::CompactionNeeded { .. } => {
        // Forward compaction notification to UI
        if self.event_tx.send(event).is_err() {
          error!(
            "Session {}: Failed to forward CompactionNeeded event",
            self.id
          );
        }
      }
      SessionEvent::CompactionCompleted { .. } => {
        // Forward compaction completion to UI
        if self.event_tx.send(event).is_err() {
          error!(
            "Session {}: Failed to forward CompactionCompleted event",
            self.id
          );
        }
      }
    }
  }

  /// Execute pending tool calls and continue the conversation
  async fn execute_tool_calls(&mut self) {
    // Get the last assistant message with tool calls
    let tool_calls = match self.messages.last() {
      Some(msg) => msg.tool_calls.clone().unwrap_or_default(),
      None => {
        error!("Session {}: No assistant message with tool calls", self.id);
        let _ = self.event_tx.send(SessionEvent::Completed);
        return;
      }
    };

    info!(
      "Session {}: Executing {} tool calls",
      self.id,
      tool_calls.len()
    );

    // Execute each tool call
    for tool_call in &tool_calls {
      // Notify UI about tool call
      let _ = self.event_tx.send(SessionEvent::ToolCallReceived {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        arguments: tool_call.arguments.clone(),
      });

      // Create invocation
      let invocation = ToolInvocation::new(
        &tool_call.name,
        &tool_call.id,
        ToolPayload::Function {
          arguments: tool_call.arguments.clone(),
        },
        &self.cwd,
      );

      // Execute tool
      match self.tool_registry.dispatch(invocation).await {
        Ok(output) => {
          let output_str = output.into_response();

          // Notify UI about completion
          let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
            name: tool_call.name.clone(),
            output: output_str.clone(),
          });

          // Add tool result to messages
          let tool_msg = Message::tool(&output_str, &tool_call.id);
          info!(
            "Session {}: Adding tool result message: tool_call_id={}, output_preview={}...",
            self.id,
            tool_call.id,
            output_str.chars().take(100).collect::<String>()
          );
          self.messages.push(tool_msg.clone());
          let _ = self.session_store.append_message(&self.id, &tool_msg);
          info!(
            "Session {}: Tool {} executed successfully, output_len={}",
            self.id,
            tool_call.name,
            output_str.len()
          );
        }
        Err(e) => {
          let error_msg = format!("Error: {}", e);

          // Notify UI about completion (with error)
          let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
            name: tool_call.name.clone(),
            output: error_msg.clone(),
          });

          // Add error result to messages
          let tool_msg = Message::tool(&error_msg, &tool_call.id);
          info!(
            "Session {}: Adding tool error message: tool_call_id={}, error={}",
            self.id, tool_call.id, error_msg
          );
          self.messages.push(tool_msg.clone());
          let _ = self.session_store.append_message(&self.id, &tool_msg);
          error!("Session {}: Tool {} failed: {}", self.id, tool_call.name, e);
        }
      }
    }

    // Continue the conversation with the tool results
    info!(
      "Session {}: Continuing conversation after tool execution",
      self.id
    );

    // Log updated message history
    info!(
      "Session {}: Updated message history for next request:",
      self.id
    );
    for (i, msg) in self.messages.iter().enumerate() {
      let content_preview: String = msg.content.chars().take(100).collect();
      let tool_calls_info = if let Some(ref tc) = msg.tool_calls {
        format!(" [tool_calls: {}]", tc.len())
      } else {
        String::new()
      };
      let tool_call_id_info = if let Some(ref id) = msg.tool_call_id {
        format!(" [tool_call_id: {}]", id)
      } else {
        String::new()
      };
      info!(
        "  [{}] {:?}: {}{}{}",
        i, msg.role, content_preview, tool_calls_info, tool_call_id_info
      );
    }

    self.current_response.clear();
    self.current_thinking.clear();
    self.start_chat_stream().await;
  }
}

/// Handle to receive events from the session
pub type EventReceiver = mpsc::UnboundedReceiver<SessionEvent>;

/// ChatSession that runs as an actor
#[derive(Debug)]
pub struct ChatSession {
  /// Session handle for sending commands
  pub handle: SessionHandle,
  /// Event receiver
  pub event_rx: EventReceiver,
}

impl ChatSession {
  fn new_session(
    config: &Config,
    system_prompt: String,
    tool_registry: Arc<crate::tools::ToolRegistry>,
    executable_tool_registry: Arc<ExecutableToolRegistry>,
    session_store: Arc<SessionStore>,
  ) -> Result<(Self, Vec<Message>)> {
    let meta = SessionMeta::new(generate_session_id(), &system_prompt);
    session_store.create(&meta)?;
    let session = Self::create_with_store(
      config,
      system_prompt,
      tool_registry,
      executable_tool_registry,
      session_store,
      meta,
    )?;
    Ok((session, Vec::new()))
  }

  fn resume(
    id: String,
    config: &Config,
    tool_registry: Arc<ToolRegistry>,
    executable_tool_registry: Arc<ExecutableToolRegistry>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    messages: Vec<Message>,
  ) -> Result<(Self, Vec<Message>)> {
    let provider = Self::create_provider(config, tool_registry)?;

    // Get max context size from default model config
    let max_context_size = config
      .default_model_config()
      .and_then(|m| m.max_context_size)
      .unwrap_or(DEFAULT_MAX_CONTEXT_SIZE);

    let session = Self::start_with_messages(
      id,
      provider,
      messages.clone(),
      executable_tool_registry,
      session_store,
      meta,
      config.compaction.clone(),
      max_context_size,
      config.retry.clone(),
    );
    Ok((session, messages))
  }

  /// Create or resume a chat session using the given mode and persistent store.
  ///
  /// Returns the session and the loaded message history (empty for new sessions).
  pub fn create_or_resume(
    config: &Config,
    system_prompt: String,
    tool_registry: Arc<ToolRegistry>,
    executable_tool_registry: Arc<ExecutableToolRegistry>,
    session_store: Arc<SessionStore>,
    mode: SessionMode,
  ) -> Result<(Self, Vec<Message>)> {
    match mode {
      SessionMode::New => Self::new_session(
        config,
        system_prompt,
        tool_registry,
        executable_tool_registry,
        session_store,
      ),
      SessionMode::ResumeById(id) => {
        let (meta, messages) = session_store.load(&id)?;
        Self::resume(
          id,
          config,
          tool_registry,
          executable_tool_registry,
          session_store,
          meta,
          messages,
        )
      }
      SessionMode::ResumeLatest => match session_store.latest_id()? {
        Some(id) => {
          let (meta, messages) = session_store.load(&id)?;
          Self::resume(
            id,
            config,
            tool_registry,
            executable_tool_registry,
            session_store,
            meta,
            messages,
          )
        }
        None => Self::new_session(
          config,
          system_prompt,
          tool_registry,
          executable_tool_registry,
          session_store,
        ),
      },
    }
  }

  /// Create LLM provider from configuration
  ///
  /// # Arguments
  /// * `config` - The application configuration
  /// * `tool_registry` - Shared tool registry for function calling
  pub(crate) fn create_provider(
    config: &Config,
    tool_registry: Arc<ToolRegistry>,
  ) -> Result<Box<dyn LLMProvider>> {
    // Get default model configuration
    let model_config = config
      .default_model_config()
      .ok_or(ConfigError::MissingDefaultModel)?;

    // Get provider configuration
    let provider =
      config
        .get_provider(&model_config.provider)
        .ok_or_else(|| ConfigError::ProviderNotFound {
          provider: model_config.provider.clone(),
          model: config.default_model.clone(),
        })?;

    // Resolve API key (may contain env var references like ${OPENAI_API_KEY})
    let api_key = provider
      .api_key
      .as_ref()
      .map(|key| config.resolve_api_key(key))
      .unwrap_or_default();

    // Create chat config
    let mut chat_config = ChatConfig::new(&model_config.model);
    if let Some(max_tokens) = model_config.max_tokens {
      chat_config = chat_config.with_max_tokens(max_tokens);
    }
    if let Some(temperature) = model_config.temperature {
      chat_config = chat_config.with_temperature(temperature);
    }
    // Set thinking mode from config
    chat_config = chat_config.with_thinking(config.default_thinking);

    // Determine if we need Coding Agent headers
    // Currently only enable for kimi-for-coding model
    let coding_agent = model_config.model == "kimi-for-coding";

    // Create provider based on type
    let provider: Box<dyn LLMProvider> = match provider.provider_type.as_str() {
      "kimi" => Box::new(KimiProvider::new(
        &provider.base_url,
        api_key,
        chat_config,
        coding_agent,
        tool_registry,
      )?),
      _ => {
        return Err(
          ConfigError::ProviderNotFound {
            provider: provider.provider_type.clone(),
            model: config.default_model.clone(),
          }
          .into(),
        );
      }
    };

    Ok(provider)
  }

  /// Start a new chat session from configuration with persistence
  fn create_with_store(
    config: &Config,
    system_prompt: impl Into<String>,
    tool_registry: Arc<ToolRegistry>,
    executable_tool_registry: Arc<ExecutableToolRegistry>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
  ) -> Result<Self> {
    let provider = Self::create_provider(config, tool_registry)?;
    let system_prompt = system_prompt.into();
    let messages = vec![Message::system(system_prompt.clone())];

    // Get max context size from default model config
    let max_context_size = config
      .default_model_config()
      .and_then(|m| m.max_context_size)
      .unwrap_or(DEFAULT_MAX_CONTEXT_SIZE);

    let session = Self::start_with_messages(
      meta.id.clone(),
      provider,
      messages,
      executable_tool_registry,
      session_store,
      meta,
      config.compaction.clone(),
      max_context_size,
      config.retry.clone(),
    );
    info!(
      "ChatSession {} created from config with store",
      session.handle.id
    );
    Ok(session)
  }

  /// Internal: start session with given messages and persistence
  #[allow(clippy::too_many_arguments)]
  fn start_with_messages(
    id: String,
    provider: Box<dyn LLMProvider>,
    messages: Vec<Message>,
    tool_registry: Arc<ExecutableToolRegistry>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    compaction_config: CompactionConfig,
    max_context_size: usize,
    retry_config: RetryConfig,
  ) -> Self {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    let handle = SessionHandle {
      id: id.clone(),
      cmd_tx,
    };

    let actor = SessionActor::new(
      id,
      provider,
      messages,
      event_tx,
      cmd_rx,
      tool_registry,
      session_store,
      meta,
      compaction_config,
      max_context_size,
      retry_config,
    );
    tokio::spawn(actor.run());

    Self { handle, event_rx }
  }

  /// Poll for the next event from the session
  ///
  /// Returns:
  /// - `Some(SessionEvent)` - An event occurred
  /// - `None` - Session has shutdown and no more events
  pub fn poll_event(&mut self) -> Option<SessionEvent> {
    self.event_rx.try_recv().ok()
  }

  #[allow(dead_code)]
  /// Check if there's an event ready without consuming it
  pub fn has_event(&self) -> bool {
    !self.event_rx.is_empty()
  }

  #[allow(dead_code)]
  /// Shutdown the session
  pub fn shutdown(&self) {
    self.handle.shutdown();
  }
}

/// Handle the streaming response from the LLM
async fn handle_stream(
  mut stream: ChatCompletionResponseStream,
  tx: mpsc::UnboundedSender<SessionEvent>,
) {
  use futures::StreamExt;

  // Buffer for accumulating content across chunks (for parsing think tags)
  let mut buffer = String::new();
  let mut in_thinking_mode = false;
  let mut has_received_thinking = false;

  // Buffer for accumulating tool calls
  let mut tool_call_buffer: Vec<ChatCompletionMessageToolCallChunk> = Vec::new();

  while let Some(result) = stream.next().await {
    match result {
      Ok(response) => {
        log::debug!("recv raw response: {:?}", response);

        log::debug!(
          "Session: Received stream response: id={}, model={}, choices={}",
          response.id,
          response.model,
          response.choices.len()
        );

        // Check for usage information (only present in the final chunk)
        if let Some(usage) = &response.usage {
          log::info!(
            "Session: Received usage - total={}, prompt={}, completion={}",
            usage.total_tokens,
            usage.prompt_tokens,
            usage.completion_tokens
          );
          let _ = tx.send(SessionEvent::Usage {
            total_tokens: usage.total_tokens,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
          });
        }

        for (i, choice) in response.choices.iter().enumerate() {
          log::debug!(
            "Session: Choice[{}]: delta={:?}, finish_reason={:?}",
            i,
            choice.delta,
            choice.finish_reason
          );
        }
        for choice in &response.choices {
          // Handle tool calls
          if let Some(ref tool_calls) = choice.delta.tool_calls {
            for tool_call in tool_calls {
              log::info!(
                "Session: Received tool call chunk: index={:?}, id={:?}",
                tool_call.index,
                tool_call.id
              );

              // Get the index for this tool call (u32, not Option<u32>)
              let idx = tool_call.index as usize;

              // Ensure buffer has enough slots
              while tool_call_buffer.len() <= idx {
                tool_call_buffer.push(ChatCompletionMessageToolCallChunk {
                  index: tool_call_buffer.len() as u32,
                  id: None,
                  r#type: None,
                  function: None,
                });
              }

              // Update the tool call at this index
              let existing = &mut tool_call_buffer[idx];

              // Update ID if provided
              if let Some(ref id) = tool_call.id {
                existing.id = Some(id.clone());
              }

              // Update type if provided
              if let Some(ref call_type) = tool_call.r#type {
                existing.r#type = Some(call_type.clone());
              }

              // Update function if provided
              if let Some(ref function) = tool_call.function {
                if existing.function.is_none() {
                  existing.function = Some(FunctionCallStream {
                    name: None,
                    arguments: None,
                  });
                }

                if let Some(ref existing_func) = existing.function {
                  let mut updated_func = existing_func.clone();

                  if let Some(ref name) = function.name {
                    updated_func.name = Some(name.clone());
                  }
                  if let Some(ref args) = function.arguments {
                    if let Some(ref existing_args) = updated_func.arguments {
                      updated_func.arguments = Some(format!("{}{}", existing_args, args));
                    } else {
                      updated_func.arguments = Some(args.clone());
                    }
                  }

                  existing.function = Some(updated_func);
                }
              }
            }
          }

          if let Some(content) = &choice.delta.content
            && !content.is_empty()
          {
            log::debug!(
              "Session: Received content chunk: len={}, content={}",
              content.len(),
              &content[..content.len().min(100)]
            );

            // Parse content for <think> tags (Kimi thinking mode)
            buffer.push_str(content);
            log::debug!(
              "Session: Buffer len={}, in_thinking_mode={}",
              buffer.len(),
              in_thinking_mode
            );

            // Process the buffer to extract thinking content
            loop {
              if in_thinking_mode {
                // Look for </think> closing tag
                if let Some(end_pos) = buffer.find("</think>") {
                  // Extract thinking content
                  let thinking = buffer[..end_pos].to_string();
                  if !thinking.is_empty() {
                    if !has_received_thinking {
                      log::info!(
                        "Session: First thinking content received: len={}",
                        thinking.len()
                      );
                      has_received_thinking = true;
                    }
                    log::debug!("Session: Sending ThinkingChunk: len={}", thinking.len());
                    if tx.send(SessionEvent::ThinkingChunk(thinking)).is_err() {
                      return;
                    }
                  }
                  // Remove processed part including closing tag
                  buffer = buffer[end_pos + 8..].to_string();
                  in_thinking_mode = false;
                  log::debug!(
                    "Session: Exited thinking mode, remaining buffer len={}",
                    buffer.len()
                  );
                } else {
                  // Still in thinking mode, send what we have so far
                  if !buffer.is_empty() {
                    if !has_received_thinking {
                      log::info!(
                        "Session: First thinking content received (partial): len={}",
                        buffer.len()
                      );
                      has_received_thinking = true;
                    }
                    log::debug!(
                      "Session: Sending ThinkingChunk (partial): len={}",
                      buffer.len()
                    );
                    if tx
                      .send(SessionEvent::ThinkingChunk(buffer.clone()))
                      .is_err()
                    {
                      return;
                    }
                    buffer.clear();
                  }
                  break;
                }
              } else {
                // Look for <think> opening tag
                if let Some(start_pos) = buffer.find("<think>") {
                  log::info!("Session: Found <think> tag at position {}", start_pos);
                  // Send any content before <think> as regular content
                  if start_pos > 0 {
                    let before = buffer[..start_pos].to_string();
                    if !before.is_empty() {
                      log::debug!(
                        "Session: Sending ContentChunk (before think): len={}",
                        before.len()
                      );
                      if tx.send(SessionEvent::ContentChunk(before)).is_err() {
                        return;
                      }
                    }
                  }
                  // Enter thinking mode
                  buffer = buffer[start_pos + 7..].to_string();
                  in_thinking_mode = true;
                  log::info!("Session: Entered thinking mode");
                } else {
                  // No <think> tag, send as regular content
                  if !buffer.is_empty() {
                    log::debug!("Session: Sending ContentChunk: len={}", buffer.len());
                    if tx.send(SessionEvent::ContentChunk(buffer.clone())).is_err() {
                      return;
                    }
                    buffer.clear();
                  }
                  break;
                }
              }
            }
          }
        }
      }
      Err(e) => {
        log::error!("Session: Stream error: {}", e);
        let _ = tx.send(SessionEvent::Error(e.to_string()));
        return;
      }
    }
  }

  // Flush any remaining content in buffer
  if !buffer.is_empty() {
    if in_thinking_mode {
      log::info!(
        "Session: Flushing final thinking content: len={}",
        buffer.len()
      );
      let _ = tx.send(SessionEvent::ThinkingChunk(buffer));
    } else {
      log::debug!("Session: Flushing final content: len={}", buffer.len());
      let _ = tx.send(SessionEvent::ContentChunk(buffer));
    }
  }

  // Send any accumulated tool calls
  for tool_call in tool_call_buffer {
    if let (Some(id), Some(function)) = (tool_call.id, tool_call.function)
      && let (Some(name), Some(arguments)) = (function.name, function.arguments)
      && !id.is_empty()
      && !name.is_empty()
    {
      log::info!(
        "Session: Sending accumulated tool call: id={}, name={}, args={}",
        id,
        name,
        arguments
      );
      // Store the tool call info for later use
      let _ = tx.send(SessionEvent::ToolCallReceived {
        id: id.clone(),
        name: name.clone(),
        arguments: arguments.clone(),
      });
    }
  }

  log::info!(
    "Session: Stream completed, received_thinking={}",
    has_received_thinking
  );
  // Stream completed
  let _ = tx.send(SessionEvent::Completed);
}

/// Check if an error from `chat_stream` is retryable.
///
/// Inspects the actual error enum variants rather than string matching:
/// - `Llm` errors → delegates to `LlmError::is_retryable()`
/// - `OpenAI` errors → delegates to `is_openai_error_retryable()`
/// - All other error types → not retryable
fn is_error_retryable(err: &crate::error::Error) -> bool {
  use crate::error::{Error, is_openai_error_retryable};

  match err {
    Error::Llm(llm_err) => llm_err.is_retryable(),
    Error::OpenAI(openai_err) => is_openai_error_retryable(openai_err),
    _ => false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::error::LlmError;

  #[test]
  fn test_session_id_format() {
    let id = generate_session_id();
    // Should contain a hyphen separating dirname and timestamp
    assert!(id.contains('-'));
    // Should contain colons for time
    assert!(id.contains(':'));
  }

  #[test]
  fn test_is_error_retryable() {
    // LlmError::StreamError is always retryable (transient by nature)
    let err = crate::error::Error::Llm(LlmError::StreamError("some stream error".to_string()));
    assert!(is_error_retryable(&err));

    // LlmError::InvalidConfig should NOT be retryable
    let err = crate::error::Error::Llm(LlmError::InvalidConfig("bad config".to_string()));
    assert!(!is_error_retryable(&err));

    // LlmError::EmptyResponse should NOT be retryable
    let err = crate::error::Error::Llm(LlmError::EmptyResponse);
    assert!(!is_error_retryable(&err));

    // LlmError::Retryable should be retryable
    let err = crate::error::Error::Llm(LlmError::Retryable {
      message: "transient".to_string(),
    });
    assert!(is_error_retryable(&err));
  }

  #[test]
  fn test_is_api_error_type_retryable() {
    use crate::error::is_api_error_type_retryable;

    // Known retryable types
    assert!(is_api_error_type_retryable(Some("server_error")));
    assert!(is_api_error_type_retryable(Some("rate_limit_error")));
    assert!(is_api_error_type_retryable(Some("timeout")));

    // Known non-retryable types
    assert!(!is_api_error_type_retryable(Some("invalid_request_error")));
    assert!(!is_api_error_type_retryable(Some("authentication_error")));
    assert!(!is_api_error_type_retryable(Some("permission_error")));
    assert!(!is_api_error_type_retryable(Some("insufficient_quota")));
    assert!(!is_api_error_type_retryable(Some("model_not_found")));

    // None = typically 5xx, conservatively retryable
    assert!(is_api_error_type_retryable(None));

    // Unknown type — not retryable
    assert!(!is_api_error_type_retryable(Some("unknown_error")));
  }
}
