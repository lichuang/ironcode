//! Chat session management module (Actor pattern).
//!
//! Manages a conversation with an LLM using the actor pattern.
//! The session runs in a dedicated tokio task and communicates via channels.

use std::env;
use std::future::pending;
use std::mem::take;
use std::path::PathBuf;
use std::sync::Arc;

use async_openai::error::OpenAIError;
use async_openai::types::chat::{
  ChatCompletionMessageToolCallChunk, ChatCompletionResponseStream, FunctionCallStream,
};
use chrono::{Datelike, Local, Timelike};
use log::{debug, error, info, warn};
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::cli::runtime::Runtime;
use crate::error::{Error, LlmError, Result, StreamErrorCategory};
use crate::llm::compaction::{Compaction, calculate_threshold, should_auto_compact};
use crate::llm::provider::LLMProvider;
use crate::llm::providers::KimiProvider;
use crate::llm::types::{ChatConfig, Message, Role, ToolCall};
use crate::session::{SessionMeta, SessionMode, SessionStore};
use crate::tools::{ToolInvocation, ToolPayload};
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

/// An option for a structured user question
#[derive(Debug, Clone)]
pub struct QuestionOption {
  /// Concise display text
  pub label: String,
  /// Brief explanation of trade-offs
  pub description: String,
}

/// A structured question to ask the user
#[derive(Debug, Clone)]
pub struct Question {
  /// The question text
  pub question: String,
  /// Short category tag
  pub header: String,
  /// Available options
  pub options: Vec<QuestionOption>,
  /// Whether multiple options can be selected
  pub multi_select: bool,
  /// Whether this is a yes/no confirmation dialog
  pub confirmation: bool,
  /// Default selected option indices (0-based)
  pub default: Vec<usize>,
  /// Whether the user must select at least one option
  pub required: bool,
}

/// Commands sent to the session actor
#[derive(Debug)]
pub enum SessionCommand {
  /// Send a user message
  SendMessage { content: String },
  /// Cancel the current streaming request
  Cancel,
  /// Clear conversation history
  ClearHistory,
  /// Approve or deny a pending tool call
  ApproveToolCall {
    /// Tool call ID being approved
    tool_call_id: String,
    /// Whether the user approved the execution
    approved: bool,
  },
  /// Answer pending structured questions
  AnswerQuestions {
    /// Tool call ID that asked the questions
    tool_call_id: String,
    /// Selected option indices for each question
    answers: Vec<Vec<usize>>,
    /// Whether the user dismissed without answering
    dismissed: bool,
  },
  /// Enable YOLO mode for the current session (persisted to meta)
  EnableSessionYolo,
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
  /// Stream was interrupted mid-way; the actor may attempt to retry
  StreamInterrupted {
    /// Error message
    error: String,
    /// Whether the actor should attempt to retry
    is_retryable: bool,
  },
  /// A tool call requires user approval before execution
  ApprovalNeeded {
    /// Tool call ID
    id: String,
    /// Tool name
    name: String,
    /// Tool arguments
    arguments: String,
    /// Optional diff preview for file-modifying tools
    diff_preview: Option<String>,
  },
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
  /// Structured questions need to be presented to the user
  QuestionsAsked {
    /// Tool call ID
    tool_call_id: String,
    /// Questions to present
    questions: Vec<Question>,
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

  /// Approve or deny a pending tool call
  pub fn approve_tool_call(&self, tool_call_id: impl Into<String>, approved: bool) {
    let _ = self.cmd_tx.send(SessionCommand::ApproveToolCall {
      tool_call_id: tool_call_id.into(),
      approved,
    });
  }

  /// Enable YOLO mode for the current session
  pub fn enable_session_yolo(&self) {
    let _ = self.cmd_tx.send(SessionCommand::EnableSessionYolo);
  }

  /// Send answers to pending structured questions
  pub fn answer_questions(
    &self,
    tool_call_id: impl Into<String>,
    answers: Vec<Vec<usize>>,
    dismissed: bool,
  ) {
    let _ = self.cmd_tx.send(SessionCommand::AnswerQuestions {
      tool_call_id: tool_call_id.into(),
      answers,
      dismissed,
    });
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
  /// Current retry attempt counter for the active stream
  stream_retry_attempt: u32,
  /// Tool call buffer for accumulating tool calls during streaming
  pending_tool_calls: Vec<ToolCall>,
  /// Working directory for tool execution
  cwd: PathBuf,
  /// Precise token count from API usage (if available)
  precise_token_count: Option<u32>,
  /// Session store for persistence
  session_store: Arc<SessionStore>,
  /// Session metadata for persistence
  meta: SessionMeta,
  /// Maximum context size for the current model (read from global config at creation)
  max_context_size: usize,
  /// Compaction handler (used for actual compaction)
  compaction: Compaction,
  /// Whether compaction has been notified for current threshold
  compaction_notified: bool,
  /// YOLO mode: auto-approve all tool calls
  yolo: bool,
  /// List of tools to auto-approve when YOLO is off
  auto_approve: Vec<String>,
  /// Runtime context (config, tool registries, etc.)
  runtime: Arc<Runtime>,
  /// State for resuming tool call execution after approval
  tool_call_execution_state: Option<ToolCallExecutionState>,
  /// State for resuming after user answers structured questions
  pending_answers_state: Option<PendingAnswersState>,
}

/// State kept while waiting for user answers to structured questions.
#[derive(Debug, Clone)]
struct PendingAnswersState {
  /// The tool call ID that asked the questions
  tool_call_id: String,
}

/// State kept while executing a batch of tool calls so that execution can
/// be paused for approval and resumed later.
struct ToolCallExecutionState {
  /// The tool calls to execute
  tool_calls: Vec<ToolCall>,
  /// Index of the next tool call to execute
  current_index: usize,
}

impl SessionActor {
  #[allow(clippy::too_many_arguments)]
  fn new(
    id: String,
    provider: Box<dyn LLMProvider>,
    messages: Vec<Message>,
    event_tx: mpsc::UnboundedSender<SessionEvent>,
    cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    yolo: bool,
    auto_approve: Vec<String>,
    runtime: Arc<Runtime>,
  ) -> Self {
    let max_context_size = provider.max_context_size();
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
      stream_retry_attempt: 0,
      pending_tool_calls: Vec::new(),
      cwd: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
      session_store,
      meta,
      precise_token_count: None,
      max_context_size,
      compaction: Compaction::default(),
      compaction_notified: false,
      yolo,
      auto_approve,
      runtime,
      tool_call_execution_state: None,
      pending_answers_state: None,
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

      SessionCommand::ApproveToolCall {
        tool_call_id,
        approved,
      } => {
        self.handle_approval(tool_call_id, approved).await;
        true
      }

      SessionCommand::AnswerQuestions {
        tool_call_id,
        answers,
        dismissed,
      } => {
        self
          .handle_question_answers(tool_call_id, answers, dismissed)
          .await;
        true
      }

      SessionCommand::EnableSessionYolo => {
        info!("Session {}: Enabling YOLO mode for this session", self.id);
        self.yolo = true;
        self.meta.yolo = true;
        if let Err(e) = self.session_store.update_meta(&self.meta) {
          error!(
            "Session {}: Failed to persist yolo mode to meta: {}",
            self.id, e
          );
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
      self.runtime.config.compaction.enabled
    );

    // Check if we should trigger compaction
    let should_compact = should_auto_compact(
      current_tokens,
      self.max_context_size,
      &self.runtime.config.compaction,
    );
    log::info!(
      "check_and_notify_compaction: should_auto_compact={}",
      should_compact
    );

    if should_compact {
      if !self.compaction_notified {
        let threshold = calculate_threshold(self.max_context_size, &self.runtime.config.compaction);
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
  ///
  /// Mirrors kimi-cli's two-layer retry: connection recovery (rebuild client)
  /// followed by tenacity-style exponential backoff.
  async fn start_chat_stream(&mut self) {
    self.stream_retry_attempt = 0;
    self.attempt_chat_stream("starting").await;
  }

  /// Attempt to start (or resume) a chat stream, with retries.
  ///
  /// This is the inner retry loop shared by initial connection attempts and
  /// mid-stream interruption recovery.  It respects `stream_retry_attempt`
  /// so that the total number of retries for a single step never exceeds
  /// `retry_config.max_attempts`.
  async fn attempt_chat_stream(&mut self, context: &str) {
    let max_attempts = self.runtime.config.retry.max_attempts.max(1);

    while self.stream_retry_attempt < max_attempts {
      match self.run_chat_stream_with_recovery().await {
        Ok(stream) => {
          if self.stream_retry_attempt > 0 {
            info!(
              "Session {}: stream {} on attempt {}/{}",
              self.id,
              context,
              self.stream_retry_attempt + 1,
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
        Err((err, recovery_exhausted)) => {
          let err_string = err.to_string();
          let is_retryable = !recovery_exhausted && is_error_retryable(&err);
          self.stream_retry_attempt += 1;

          if !is_retryable {
            error!(
              "Session {}: non-retryable error on attempt {}/{}: {}",
              self.id, self.stream_retry_attempt, max_attempts, err_string
            );
            let _ = self
              .event_tx
              .send(SessionEvent::Error(format_user_friendly_error(&err_string)));
            return;
          }

          if self.stream_retry_attempt >= max_attempts {
            error!(
              "Session {}: all {} attempts exhausted, last error: {}",
              self.id, max_attempts, err_string
            );
            let friendly = format_user_friendly_error(&err_string);
            let _ = self.event_tx.send(SessionEvent::Error(format!(
              "{} (tried {} times)",
              friendly, max_attempts
            )));
            return;
          }

          let delay = self
            .runtime
            .config
            .retry
            .delay_for_attempt(self.stream_retry_attempt - 1);
          warn!(
            "Session {}: attempt {}/{} failed ({}), retrying in {:?}",
            self.id, self.stream_retry_attempt, max_attempts, err_string, delay
          );
          sleep(delay).await;
        }
      }
    }
  }

  /// Attempt to start a chat stream once, with immediate connection recovery.
  ///
  /// If the error is a connection-level error (timeout, disconnect, transport),
  /// calls `provider.on_retryable_error()` to refresh the connection and retries
  /// exactly once immediately. If that retry also fails, `recovery_exhausted`
  /// is returned as `true`, signaling the outer loop not to retry further.
  async fn run_chat_stream_with_recovery(
    &mut self,
  ) -> std::result::Result<ChatCompletionResponseStream, (Error, bool)> {
    match self.provider.chat_stream(self.messages.clone()).await {
      Ok(stream) => Ok(stream),
      Err(err) => {
        let is_connection_error = matches!(
          &err,
          Error::Llm(LlmError::Stream {
            category: StreamErrorCategory::Timeout
              | StreamErrorCategory::Disconnected
              | StreamErrorCategory::Transport,
            ..
          }) | Error::Llm(LlmError::EmptyResponse)
        );

        if is_connection_error {
          info!(
            "Session {}: connection error, attempting immediate recovery: {}",
            self.id, err
          );
          if let Error::Llm(ref llm_err) = err {
            self.provider.on_retryable_error(llm_err).await;
          }

          match self.provider.chat_stream(self.messages.clone()).await {
            Ok(stream) => {
              info!("Session {}: connection recovery succeeded", self.id);
              Ok(stream)
            }
            Err(retry_err) => {
              warn!(
                "Session {}: connection recovery failed: {}",
                self.id, retry_err
              );
              // Only mark recovery as exhausted if the retry error is also a
              // connection-level error.  If it is now an HTTP status error
              // (e.g. 503) the outer retry loop should still attempt backoff.
              let is_still_connection = matches!(
                &retry_err,
                Error::Llm(LlmError::Stream {
                  category: StreamErrorCategory::Timeout
                    | StreamErrorCategory::Disconnected
                    | StreamErrorCategory::Transport,
                  ..
                }) | Error::Llm(LlmError::EmptyResponse)
              );
              Err((retry_err, is_still_connection))
            }
          }
        } else {
          Err((err, false))
        }
      }
    }
  }

  /// Handle a streaming event from the LLM
  async fn handle_stream_event(&mut self, event: SessionEvent) {
    match &event {
      SessionEvent::ContentChunk(chunk) => {
        let preview: String = chunk.chars().take(100).collect();
        debug!(
          "Session {}: Stream content: len={}, content={}",
          self.id,
          chunk.len(),
          preview
        );
        self.current_response.push_str(chunk);
        // Forward to caller
        if self.event_tx.send(event).is_err() {
          error!("Session {}: Failed to forward ContentChunk", self.id);
        }
      }
      SessionEvent::ThinkingChunk(chunk) => {
        let preview: String = chunk.chars().take(100).collect();
        info!(
          "Session {}: Stream thinking received: len={}, content={}",
          self.id,
          chunk.len(),
          preview
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
      SessionEvent::StreamInterrupted {
        error,
        is_retryable,
      } => {
        error!("Session {}: Stream interrupted: {}", self.id, error);
        self.is_streaming = false;
        self.stream_rx = None;
        // Do NOT retain partial content — mirrors kimi-cli behaviour
        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();

        if *is_retryable {
          self.attempt_chat_stream("resumed after interrupt").await;
        } else {
          let _ = self
            .event_tx
            .send(SessionEvent::Error(format_user_friendly_error(error)));
        }
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
      SessionEvent::ApprovalNeeded { .. } => {
        // ApprovalNeeded is never emitted by the stream task,
        // but the match must be exhaustive.
      }
      SessionEvent::QuestionsAsked { .. } => {
        // Forward to UI
        if self.event_tx.send(event).is_err() {
          error!(
            "Session {}: Failed to forward QuestionsAsked event",
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

  /// Check whether a tool call should be executed without user confirmation.
  fn should_auto_approve(&self, tool_name: &str) -> bool {
    self.yolo || self.auto_approve.iter().any(|n| n == tool_name)
  }

  /// Execute pending tool calls and continue the conversation.
  ///
  /// If YOLO mode is off and a tool is not in the auto-approve list,
  /// execution pauses and an `ApprovalNeeded` event is sent to the UI.
  async fn execute_tool_calls(&mut self) {
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

    self.tool_call_execution_state = Some(ToolCallExecutionState {
      tool_calls,
      current_index: 0,
    });
    self.continue_tool_call_execution().await;
  }

  /// Resume tool call execution from the pending state.
  async fn continue_tool_call_execution(&mut self) {
    let state = match self.tool_call_execution_state.take() {
      Some(s) => s,
      None => {
        error!(
          "Session {}: continue_tool_call_execution called with no state",
          self.id
        );
        return;
      }
    };

    let tool_calls = state.tool_calls.clone();
    for (i, tool_call) in tool_calls.iter().enumerate().skip(state.current_index) {
      let _ = self.event_tx.send(SessionEvent::ToolCallReceived {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        arguments: tool_call.arguments.clone(),
      });

      // Special handling for AskUserQuestion: pause execution and wait for answers
      if tool_call.name == "AskUserQuestion" {
        if self.yolo {
          info!(
            "Session {}: AskUserQuestion in YOLO mode, auto-dismissing",
            self.id
          );
          let output = r#"{"answers": {}, "note": "Running in non-interactive (yolo) mode. Make your own decision."}"#.to_string();
          let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
            name: tool_call.name.clone(),
            output: output.clone(),
          });
          let tool_msg = Message::tool(&output, &tool_call.id);
          self.messages.push(tool_msg.clone());
          let _ = self.session_store.append_message(&self.id, &tool_msg);
          continue;
        }

        match parse_ask_user_questions(&tool_call.arguments) {
          Ok(questions) => {
            info!(
              "Session {}: AskUserQuestion paused, waiting for user answers",
              self.id
            );
            self.pending_answers_state = Some(PendingAnswersState {
              tool_call_id: tool_call.id.clone(),
            });
            self.tool_call_execution_state = Some(ToolCallExecutionState {
              tool_calls: state.tool_calls.clone(),
              current_index: i,
            });
            let _ = self.event_tx.send(SessionEvent::QuestionsAsked {
              tool_call_id: tool_call.id.clone(),
              questions,
            });
            return;
          }
          Err(e) => {
            error!(
              "Session {}: Failed to parse AskUserQuestion arguments: {}",
              self.id, e
            );
            let error_msg = format!("Error: Invalid AskUserQuestion arguments: {}", e);
            let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
              name: tool_call.name.clone(),
              output: error_msg.clone(),
            });
            let tool_msg = Message::tool(&error_msg, &tool_call.id);
            self.messages.push(tool_msg.clone());
            let _ = self.session_store.append_message(&self.id, &tool_msg);
            continue;
          }
        }
      }

      if !self.should_auto_approve(&tool_call.name) {
        info!(
          "Session {}: Tool {} requires approval, pausing execution",
          self.id, tool_call.name
        );
        let invocation = ToolInvocation::new(
          &tool_call.id,
          ToolPayload::Function {
            arguments: tool_call.arguments.clone(),
          },
          &self.cwd,
        );
        let diff_preview = self.runtime.executable_registry.preview(&invocation).await;
        self.tool_call_execution_state = Some(ToolCallExecutionState {
          tool_calls: state.tool_calls.clone(),
          current_index: i,
        });
        let _ = self.event_tx.send(SessionEvent::ApprovalNeeded {
          id: tool_call.id.clone(),
          name: tool_call.name.clone(),
          arguments: tool_call.arguments.clone(),
          diff_preview,
        });
        return;
      }

      self.execute_single_tool_call(tool_call).await;
    }

    info!(
      "Session {}: Continuing conversation after tool execution",
      self.id
    );

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

  /// Execute a single tool call and store the result.
  async fn execute_single_tool_call(&mut self, tool_call: &ToolCall) {
    let invocation = ToolInvocation::new(
      &tool_call.id,
      ToolPayload::Function {
        arguments: tool_call.arguments.clone(),
      },
      &self.cwd,
    );

    match self.runtime.executable_registry.dispatch(invocation).await {
      Ok(output) => {
        let output_str = output.into_response();

        let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
          name: tool_call.name.clone(),
          output: output_str.clone(),
        });

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

        let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
          name: tool_call.name.clone(),
          output: error_msg.clone(),
        });

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

  /// Handle user approval or denial of a pending tool call.
  async fn handle_approval(&mut self, tool_call_id: String, approved: bool) {
    let state = match self.tool_call_execution_state.take() {
      Some(s) => s,
      None => {
        error!(
          "Session {}: Received approval but no tool execution is pending",
          self.id
        );
        return;
      }
    };

    let current_index = state.current_index;
    let tool_call = &state.tool_calls[current_index];

    if tool_call.id != tool_call_id {
      error!(
        "Session {}: Approval tool_call_id mismatch: expected {}, got {}",
        self.id, tool_call.id, tool_call_id
      );
      self.tool_call_execution_state = Some(state);
      return;
    }

    if approved {
      info!("Session {}: User approved tool {}", self.id, tool_call.name);
      self.execute_single_tool_call(tool_call).await;
    } else {
      let denied_msg = format!("User declined to execute tool: {}", tool_call.name);
      info!(
        "Session {}: User denied tool {}: {}",
        self.id, tool_call.name, denied_msg
      );

      let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
        name: tool_call.name.clone(),
        output: denied_msg.clone(),
      });

      let tool_msg = Message::tool(&denied_msg, &tool_call.id);
      self.messages.push(tool_msg.clone());
      let _ = self.session_store.append_message(&self.id, &tool_msg);
    }

    let next_index = current_index + 1;
    self.tool_call_execution_state = Some(ToolCallExecutionState {
      tool_calls: state.tool_calls,
      current_index: next_index,
    });
    self.continue_tool_call_execution().await;
  }

  /// Handle user answers to structured questions from AskUserQuestion.
  async fn handle_question_answers(
    &mut self,
    tool_call_id: String,
    answers: Vec<Vec<usize>>,
    dismissed: bool,
  ) {
    let state = match self.tool_call_execution_state.take() {
      Some(s) => s,
      None => {
        error!(
          "Session {}: Received question answers but no tool execution is pending",
          self.id
        );
        return;
      }
    };

    let pending = match self.pending_answers_state.take() {
      Some(p) => p,
      None => {
        error!(
          "Session {}: Received question answers but no questions are pending",
          self.id
        );
        self.tool_call_execution_state = Some(state);
        return;
      }
    };

    if pending.tool_call_id != tool_call_id {
      error!(
        "Session {}: Question answer tool_call_id mismatch: expected {}, got {}",
        self.id, pending.tool_call_id, tool_call_id
      );
      self.tool_call_execution_state = Some(state);
      self.pending_answers_state = Some(pending);
      return;
    }

    let current_index = state.current_index;
    let tool_call = &state.tool_calls[current_index];

    let output = if dismissed || answers.is_empty() {
      info!("Session {}: User dismissed AskUserQuestion", self.id);
      r#"{"answers": {}, "note": "User dismissed the question without answering."}"#.to_string()
    } else {
      // Build the answers JSON matching kimi-cli format:
      // {"answers": {"Question text": "Selected label"}} for single-select
      // {"answers": {"Question text": ["Label A", "Label B"]}} for multi-select
      let mut answers_map = serde_json::Map::new();
      // We need the original questions to map indices to labels.
      // Re-parse the arguments to get the question texts and option labels.
      if let Ok(args) = parse_ask_user_question_args(&tool_call.arguments) {
        for (q_idx, selected) in answers.iter().enumerate() {
          if let Some(q) = args.questions.get(q_idx) {
            let key = &q.question;
            if q.multi_select {
              let labels: Vec<String> = selected
                .iter()
                .filter_map(|&idx| q.options.get(idx).map(|o| o.label.clone()))
                .collect();
              answers_map.insert(key.clone(), serde_json::json!(labels));
            } else if let Some(&idx) = selected.first()
              && let Some(opt) = q.options.get(idx)
            {
              answers_map.insert(key.clone(), serde_json::json!(opt.label));
            }
          }
        }
      }
      let obj = serde_json::json!({"answers": answers_map});
      obj.to_string()
    };

    info!(
      "Session {}: AskUserQuestion answered, output_len={}",
      self.id,
      output.len()
    );

    let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
      name: tool_call.name.clone(),
      output: output.clone(),
    });

    let tool_msg = Message::tool(&output, &tool_call.id);
    self.messages.push(tool_msg.clone());
    let _ = self.session_store.append_message(&self.id, &tool_msg);

    let next_index = current_index + 1;
    self.tool_call_execution_state = Some(ToolCallExecutionState {
      tool_calls: state.tool_calls,
      current_index: next_index,
    });
    self.continue_tool_call_execution().await;
  }
}

// ---------------------------------------------------------------------------
// AskUserQuestion argument parsing (used by SessionActor)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct AskUserQuestionArgOption {
  label: String,
  #[serde(default)]
  description: String,
}

#[derive(Debug, serde::Deserialize)]
struct AskUserQuestionArgQuestion {
  question: String,
  #[serde(default)]
  header: String,
  #[serde(default)]
  options: Vec<AskUserQuestionArgOption>,
  #[serde(default)]
  multi_select: bool,
  #[serde(default)]
  confirmation: bool,
  #[serde(default)]
  default: Vec<usize>,
  #[serde(default)]
  required: bool,
}

#[derive(Debug, serde::Deserialize)]
struct AskUserQuestionArgs {
  questions: Vec<AskUserQuestionArgQuestion>,
}

/// Parse AskUserQuestion arguments from JSON.
fn parse_ask_user_questions(arguments: &str) -> std::result::Result<Vec<Question>, String> {
  let args: AskUserQuestionArgs =
    serde_json::from_str(arguments).map_err(|e| format!("JSON parse error: {}", e))?;

  if args.questions.is_empty() {
    return Err("At least one question is required.".to_string());
  }

  if args.questions.len() > 4 {
    return Err("Maximum 4 questions allowed per call.".to_string());
  }

  for (idx, q) in args.questions.iter().enumerate() {
    if q.question.trim().is_empty() {
      return Err(format!(
        "Question {}: question text cannot be empty.",
        idx + 1
      ));
    }

    if !q.confirmation {
      if q.options.len() < 2 {
        return Err(format!(
          "Question {}: at least 2 options are required.",
          idx + 1
        ));
      }

      if q.options.len() > 4 {
        return Err(format!("Question {}: maximum 4 options allowed.", idx + 1));
      }

      for (opt_idx, opt) in q.options.iter().enumerate() {
        if opt.label.trim().is_empty() {
          return Err(format!(
            "Question {}: option {} label cannot be empty.",
            idx + 1,
            opt_idx + 1
          ));
        }
      }

      // Validate default indices
      let effective_option_count = if q.confirmation { 2 } else { q.options.len() };
      for (d_idx, &default_idx) in q.default.iter().enumerate() {
        if default_idx >= effective_option_count {
          return Err(format!(
            "Question {}: default index {} (value: {}) is out of range (max: {}).",
            idx + 1,
            d_idx + 1,
            default_idx,
            effective_option_count.saturating_sub(1)
          ));
        }
      }
      if !q.multi_select && !q.confirmation && q.default.len() > 1 {
        return Err(format!(
          "Question {}: single-select question can have at most one default index.",
          idx + 1
        ));
      }
    }
  }

  let questions = args
    .questions
    .into_iter()
    .map(|q| {
      let options = if q.confirmation {
        vec![
          QuestionOption {
            label: "Yes".to_string(),
            description: String::new(),
          },
          QuestionOption {
            label: "No".to_string(),
            description: String::new(),
          },
        ]
      } else {
        q.options
          .into_iter()
          .map(|o| QuestionOption {
            label: o.label,
            description: o.description,
          })
          .collect()
      };
      Question {
        question: q.question,
        header: q.header,
        options,
        multi_select: q.multi_select,
        confirmation: q.confirmation,
        default: q.default,
        required: q.required,
      }
    })
    .collect();

  Ok(questions)
}

fn parse_ask_user_question_args(
  arguments: &str,
) -> std::result::Result<AskUserQuestionArgs, String> {
  serde_json::from_str(arguments).map_err(|e| format!("JSON parse error: {}", e))
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
    runtime: Arc<Runtime>,
    system_prompt: String,
    session_store: Arc<SessionStore>,
  ) -> Result<(Self, Vec<Message>)> {
    let config = &runtime.config;
    let mut meta = SessionMeta::new(generate_session_id(), &system_prompt);
    meta.yolo = config.yolo;
    session_store.create(&meta)?;
    let session = Self::create_with_store(runtime, system_prompt, session_store, meta)?;
    Ok((session, Vec::new()))
  }

  fn resume(
    id: String,
    runtime: Arc<Runtime>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    messages: Vec<Message>,
  ) -> Result<(Self, Vec<Message>)> {
    let config = &runtime.config;
    let provider = Self::create_provider(&runtime)?;

    let yolo = meta.yolo;
    let session = Self::start_with_messages(
      id,
      provider,
      messages.clone(),
      session_store,
      meta,
      yolo,
      config.auto_approve.clone(),
      runtime,
    );
    Ok((session, messages))
  }

  /// Create or resume a chat session using the given mode and persistent store.
  ///
  /// Returns the session and the loaded message history (empty for new sessions).
  pub fn create_or_resume(
    runtime: Arc<Runtime>,
    system_prompt: String,
    session_store: Arc<SessionStore>,
    mode: SessionMode,
  ) -> Result<(Self, Vec<Message>)> {
    match mode {
      SessionMode::New => Self::new_session(runtime, system_prompt, session_store),
      SessionMode::ResumeById(id) => {
        let (meta, messages) = session_store.load(&id)?;
        Self::resume(id, runtime, session_store, meta, messages)
      }
      SessionMode::ResumeLatest => match session_store.latest_id()? {
        Some(id) => {
          let (meta, messages) = session_store.load(&id)?;
          Self::resume(id, runtime, session_store, meta, messages)
        }
        None => Self::new_session(runtime, system_prompt, session_store),
      },
    }
  }

  /// Create LLM provider from configuration
  ///
  /// # Arguments
  /// * `config` - The application configuration
  pub(crate) fn create_provider(runtime: &Runtime) -> Result<Box<dyn LLMProvider>> {
    let config = &runtime.config;
    let tool_registry = runtime.tool_registry.clone();

    // Get default model configuration
    let model_config = config
      .default_model_config()
      .ok_or(crate::config::Error::MissingDefaultModel)?;

    // Get provider configuration
    let provider = config.get_provider(&model_config.provider).ok_or_else(|| {
      crate::config::Error::ProviderNotFound {
        provider: model_config.provider.clone(),
        model: config.default_model.clone(),
      }
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

    // Get max context size from default model config
    let max_context_size = model_config
      .max_context_size
      .unwrap_or(crate::config::DEFAULT_MAX_CONTEXT_SIZE);

    // Create provider based on type
    let provider: Box<dyn LLMProvider> = match provider.provider_type.as_str() {
      "kimi" => Box::new(KimiProvider::new(
        &provider.base_url,
        api_key,
        chat_config,
        coding_agent,
        max_context_size,
        tool_registry,
      )?),
      _ => {
        return Err(
          crate::config::Error::ProviderNotFound {
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
    runtime: Arc<Runtime>,
    system_prompt: impl Into<String>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
  ) -> Result<Self> {
    let config = &runtime.config;
    let provider = Self::create_provider(&runtime)?;
    let system_prompt = system_prompt.into();
    let messages = vec![Message::system(system_prompt.clone())];

    let yolo = meta.yolo;
    let session = Self::start_with_messages(
      meta.id.clone(),
      provider,
      messages,
      session_store,
      meta,
      yolo,
      config.auto_approve.clone(),
      runtime,
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
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    yolo: bool,
    auto_approve: Vec<String>,
    runtime: Arc<Runtime>,
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
      session_store,
      meta,
      yolo,
      auto_approve,
      runtime,
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
            let preview: String = content.chars().take(100).collect();
            log::debug!(
              "Session: Received content chunk: len={}, content={}",
              content.len(),
              preview
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
        let is_retryable = is_stream_error_retryable(&e);
        let _ = tx.send(SessionEvent::StreamInterrupted {
          error: e.to_string(),
          is_retryable,
        });
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
/// - `Llm` errors → delegates to `LlmError::is_retryable()`
/// - All other error types → not retryable
fn is_error_retryable(err: &Error) -> bool {
  match err {
    Error::Llm(llm_err) => llm_err.is_retryable(),
    _ => false,
  }
}

/// Convert a raw LLM error string into a user-friendly message.
fn format_user_friendly_error(err: &str) -> String {
  if err.contains("Stream timeout") || err.contains("timed out") {
    "Connection timed out while waiting for the response. Please check your network and try again."
      .to_string()
  } else if err.contains("Connection lost") {
    "The connection was interrupted. Please check your network and try again.".to_string()
  } else if err.contains("Transport error") {
    "A network error occurred. Please check your connection and try again.".to_string()
  } else if err.contains("HTTP 429") {
    "Rate limit exceeded. Please wait a moment and try again.".to_string()
  } else if err.contains("HTTP 500")
    || err.contains("HTTP 502")
    || err.contains("HTTP 503")
    || err.contains("HTTP 504")
  {
    "The server is temporarily unavailable. Please try again later.".to_string()
  } else if err.contains("Parse error") {
    "Received an invalid response from the server. Please try again.".to_string()
  } else if err.contains("EmptyResponse") || err.contains("No response content") {
    "The server returned an empty response. Please try again.".to_string()
  } else if err.contains("non-retryable error") {
    err.to_string()
  } else {
    format!("An error occurred: {}", err)
  }
}

/// Check if a mid-stream `OpenAIError` is retryable.
///
/// Used by `handle_stream` to decide whether a stream interruption should
/// trigger a step-level retry.
fn is_stream_error_retryable(err: &OpenAIError) -> bool {
  match err {
    OpenAIError::Reqwest(reqwest_err) => {
      reqwest_err.is_timeout() || reqwest_err.is_connect() || reqwest_err.is_request()
    }
    OpenAIError::StreamError(stream_err) => {
      // KimiProvider embeds LlmError::Stream messages here.
      // Parse errors are NOT retryable.
      let msg = stream_err.to_string();
      !msg.contains("parse error") && !msg.contains("Parse error")
    }
    _ => false,
  }
}

#[cfg(test)]
impl SessionHandle {
  /// Create a test session handle with a given command sender.
  pub fn test_new(id: String, cmd_tx: mpsc::UnboundedSender<SessionCommand>) -> Self {
    Self { id, cmd_tx }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::error::LlmError;

  #[test]
  fn test_session_id_format() {
    let id = generate_session_id();
    assert!(id.contains('-'));
    assert!(id.contains(':'));
  }

  #[test]
  fn test_is_error_retryable() {
    use crate::error::StreamErrorCategory;

    // --- Stream errors (→ kimi-cli APITimeoutError / APIConnectionError / APIStatusError) ---

    // Timeout — retryable (→ APITimeoutError)
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Timeout,
      status_code: None,
      message: "stream timeout".to_string(),
    });
    assert!(is_error_retryable(&err));

    // Disconnected — retryable (→ APIConnectionError)
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Disconnected,
      status_code: None,
      message: "connection lost".to_string(),
    });
    assert!(is_error_retryable(&err));

    // Transport — retryable
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Transport,
      status_code: None,
      message: "transport error".to_string(),
    });
    assert!(is_error_retryable(&err));

    // Http with 429 — retryable (→ APIStatusError)
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Http,
      status_code: Some(429),
      message: "rate limited".to_string(),
    });
    assert!(is_error_retryable(&err));

    // Http with 500, 502, 503, 504 — retryable
    for code in [500, 502, 503, 504] {
      let err = Error::Llm(LlmError::Stream {
        category: StreamErrorCategory::Http,
        status_code: Some(code),
        message: "server error".to_string(),
      });
      assert!(
        is_error_retryable(&err),
        "Stream HTTP {} should be retryable",
        code
      );
    }

    // Http with 400, 401, 403, 404 — NOT retryable
    for code in [400, 401, 403, 404] {
      let err = Error::Llm(LlmError::Stream {
        category: StreamErrorCategory::Http,
        status_code: Some(code),
        message: "client error".to_string(),
      });
      assert!(
        !is_error_retryable(&err),
        "Stream HTTP {} should NOT be retryable",
        code
      );
    }

    // Parse — NOT retryable
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Parse,
      status_code: None,
      message: "invalid UTF-8".to_string(),
    });
    assert!(!is_error_retryable(&err));

    // --- EmptyResponse (→ kimi-cli APIEmptyResponseError) ---
    let err = Error::Llm(LlmError::EmptyResponse);
    assert!(is_error_retryable(&err));

    // --- InvalidConfig — NOT retryable ---
    let err = Error::Llm(LlmError::InvalidConfig("bad config".to_string()));
    assert!(!is_error_retryable(&err));

    // --- BuildRequest — NOT retryable ---
    let err = Error::Llm(LlmError::BuildRequest {
      source: OpenAIError::InvalidArgument("test".to_string()),
    });
    assert!(!is_error_retryable(&err));
  }

  #[test]
  fn test_parse_ask_user_questions_valid() {
    let json = r#"{
      "questions": [
        {
          "question": "Which option?",
          "header": "Test",
          "options": [
            {"label": "A", "description": "First"},
            {"label": "B"}
          ],
          "multi_select": false
        }
      ]
    }"#;
    let questions = parse_ask_user_questions(json).unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].question, "Which option?");
    assert_eq!(questions[0].header, "Test");
    assert!(!questions[0].multi_select);
    assert_eq!(questions[0].options.len(), 2);
    assert_eq!(questions[0].options[0].label, "A");
    assert_eq!(questions[0].options[0].description, "First");
    assert_eq!(questions[0].options[1].label, "B");
  }

  #[test]
  fn test_parse_ask_user_questions_multi_select() {
    let json = r#"{
      "questions": [
        {
          "question": "Pick colors",
          "options": [
            {"label": "Red"},
            {"label": "Green"},
            {"label": "Blue"}
          ],
          "multi_select": true
        }
      ]
    }"#;
    let questions = parse_ask_user_questions(json).unwrap();
    assert_eq!(questions.len(), 1);
    assert!(questions[0].multi_select);
    assert_eq!(questions[0].options.len(), 3);
  }

  #[test]
  fn test_parse_ask_user_questions_empty_questions() {
    let json = r#"{"questions": []}"#;
    let err = parse_ask_user_questions(json).unwrap_err();
    assert!(err.contains("At least one question"));
  }

  #[test]
  fn test_parse_ask_user_questions_too_many_questions() {
    let json = r#"{
      "questions": [
        {"question": "Q1?", "options": [{"label": "A"}, {"label": "B"}]},
        {"question": "Q2?", "options": [{"label": "A"}, {"label": "B"}]},
        {"question": "Q3?", "options": [{"label": "A"}, {"label": "B"}]},
        {"question": "Q4?", "options": [{"label": "A"}, {"label": "B"}]},
        {"question": "Q5?", "options": [{"label": "A"}, {"label": "B"}]}
      ]
    }"#;
    let err = parse_ask_user_questions(json).unwrap_err();
    assert!(err.contains("Maximum 4 questions"));
  }

  #[test]
  fn test_parse_ask_user_questions_too_few_options() {
    let json = r#"{
      "questions": [
        {"question": "Q?", "options": [{"label": "Only"}]}
      ]
    }"#;
    let err = parse_ask_user_questions(json).unwrap_err();
    assert!(err.contains("at least 2 options"));
  }

  #[test]
  fn test_parse_ask_user_questions_empty_option_label() {
    let json = r#"{
      "questions": [
        {"question": "Q?", "options": [{"label": ""}, {"label": "B"}]}
      ]
    }"#;
    let err = parse_ask_user_questions(json).unwrap_err();
    assert!(err.contains("label cannot be empty"));
  }

  #[test]
  fn test_parse_ask_user_question_args_roundtrip() {
    let json = r#"{"questions": [{"question": "Q?", "options": [{"label": "A"}]}]}"#;
    let args = parse_ask_user_question_args(json).unwrap();
    assert_eq!(args.questions.len(), 1);
    assert_eq!(args.questions[0].question, "Q?");
  }

  #[test]
  fn test_parse_ask_user_questions_confirmation_defaults_to_yes_no() {
    let json = r#"{
      "questions": [
        {
          "question": "Are you sure?",
          "confirmation": true
        }
      ]
    }"#;
    let questions = parse_ask_user_questions(json).unwrap();
    assert_eq!(questions.len(), 1);
    assert!(questions[0].confirmation);
    assert_eq!(questions[0].options.len(), 2);
    assert_eq!(questions[0].options[0].label, "Yes");
    assert_eq!(questions[0].options[1].label, "No");
  }

  #[test]
  fn test_parse_ask_user_questions_confirmation_ignores_provided_options() {
    let json = r#"{
      "questions": [
        {
          "question": "Proceed?",
          "confirmation": true,
          "options": [{"label": "Maybe"}]
        }
      ]
    }"#;
    let questions = parse_ask_user_questions(json).unwrap();
    assert!(questions[0].confirmation);
    // Provided options are ignored, replaced with Yes/No
    assert_eq!(questions[0].options[0].label, "Yes");
    assert_eq!(questions[0].options[1].label, "No");
  }
}
