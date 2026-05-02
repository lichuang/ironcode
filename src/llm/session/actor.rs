//! Chat session management module (Actor pattern).
//!
//! Manages a conversation with an LLM using the actor pattern.
//! The session runs in a dedicated tokio task and communicates via channels.

use std::env;
use std::future::pending;
use std::mem::take;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Datelike, Local, Timelike};
use log::{debug, error, info};
use tokio::sync::mpsc;

use crate::cli::runtime::Runtime;
use crate::error::Result;
use crate::llm::compaction::{Compaction, calculate_threshold, should_auto_compact};
use crate::llm::provider::LLMProvider;
use crate::llm::providers::KimiProvider;
use crate::llm::types::{ChatConfig, Message, ToolCall};
use crate::session::{SessionMeta, SessionMode, SessionStore};
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

use super::context::Context;
use super::persistence::SessionPersistence;
use super::stream::{StreamManager, format_user_friendly_error};
use super::tool_exec::ToolExecutor;
use super::{Question, QuestionOption, SessionCommand, SessionEvent, SessionHandle};

/// Internal state of the session actor
struct SessionActor {
  /// Session ID
  id: String,
  /// Message history managed by Context
  context: Context,
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
  /// Stream manager for LLM streaming with retry
  stream_manager: StreamManager,
  /// Tool call buffer for accumulating tool calls during streaming
  pending_tool_calls: Vec<ToolCall>,
  /// Precise token count from API usage (if available)
  precise_token_count: Option<u32>,
  /// Buffered persistence layer
  persistence: SessionPersistence,
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
  /// Tool executor for dispatching and previewing tool calls
  tool_executor: ToolExecutor,
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
      context: Context::from_messages(messages),
      event_tx,
      cmd_rx,
      current_response: String::new(),
      current_thinking: String::new(),
      is_streaming: false,
      stream_rx: None,
      stream_manager: StreamManager::new(provider, runtime.retry_config()),
      pending_tool_calls: Vec::new(),
      persistence: SessionPersistence::new(session_store),
      meta,
      precise_token_count: None,
      max_context_size,
      compaction: Compaction::default(),
      compaction_notified: false,
      yolo,
      auto_approve,
      tool_executor: ToolExecutor::new(
        env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        runtime.executable_registry.clone(),
      ),
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
        let user_msg = self.context.push_user(&content).clone();
        self.persistence.stage_message(&self.id, &user_msg);
        self.meta.update_title_from_message(&user_msg);
        self.meta.updated_at = Local::now();
        self.persistence.stage_meta(&self.meta);
        let _ = self.persistence.flush();
        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();

        // Check if compaction is needed before sending
        self.check_and_notify_compaction().await;

        // Log current message history for debugging
        info!("Session {}: Current message history:", self.id);
        for (i, msg) in self.context.messages().iter().enumerate() {
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
        self.start_stream().await;
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
        self.context.clear_non_system();

        self.meta.updated_at = Local::now();
        self
          .persistence
          .reset_messages(&self.id, self.context.messages());
        self.persistence.stage_meta(&self.meta);
        let _ = self.persistence.flush();

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
        self.persistence.stage_meta(&self.meta);
        if let Err(e) = self.persistence.flush() {
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
      let recent_tokens = estimate_llm_messages_tokens(
        &self.context.messages()[self.context.len().saturating_sub(2)..],
      );
      log::info!(
        "check_and_notify_compaction: precise={}, recent={}, total={}",
        precise,
        recent_tokens,
        precise as usize + recent_tokens
      );
      precise as usize + recent_tokens
    } else {
      let estimated = self.context.estimate_tokens();
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
      self.runtime.enable_compaction()
    );

    // Check if we should trigger compaction
    let should_compact = should_auto_compact(
      current_tokens,
      self.max_context_size,
      self.runtime.compaction_config(),
    );
    log::info!(
      "check_and_notify_compaction: should_auto_compact={}",
      should_compact
    );

    if should_compact {
      if !self.compaction_notified {
        let threshold =
          calculate_threshold(self.max_context_size, self.runtime.compaction_config());
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
    let message_count_before = self.context.len();

    // Check if compaction should be performed
    if !self.compaction.should_compact(self.context.messages()) {
      log::info!(
        "Session {}: Compaction strategy decided not to compact ({} messages)",
        self.id,
        message_count_before
      );
      return;
    }

    // Perform compaction
    log::info!("Session {}: Executing compaction...", self.id);
    let result = self.compaction.compact(self.context.messages());

    if !result.did_compact {
      log::info!("Session {}: No compaction performed by strategy", self.id);
      return;
    }

    // Update messages
    *self.context.messages_mut() = result.messages;
    let message_count_after = self.context.len();

    // Estimate new token count
    let new_token_count = self.context.estimate_tokens();

    log::info!(
      "Session {}: Compaction completed - {} messages -> {} messages, ~{} tokens",
      self.id,
      message_count_before,
      message_count_after,
      new_token_count
    );

    // Save compacted messages to session store
    self
      .persistence
      .reset_messages(&self.id, self.context.messages());

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
  /// Start streaming the current context messages via the StreamManager.
  async fn start_stream(&mut self) {
    let messages = self.context.messages().to_vec();
    match self.stream_manager.start(messages).await {
      Ok(rx) => {
        self.stream_rx = Some(rx);
        self.is_streaming = true;
      }
      Err(err) => {
        error!("Session {}: failed to start stream: {}", self.id, err);
        let _ = self
          .event_tx
          .send(SessionEvent::Error(format_user_friendly_error(
            &err.to_string(),
          )));
      }
    }
  }

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
          let thinking_opt = if thinking.is_empty() {
            None
          } else {
            Some(thinking)
          };
          let assistant_msg = self
            .context
            .push_assistant(response, thinking_opt, tool_calls)
            .clone();
          self.persistence.stage_message(&self.id, &assistant_msg);
          self.meta.updated_at = Local::now();
          self.persistence.stage_meta(&self.meta);
          let _ = self.persistence.flush();
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
        if let Some(msg) = self.context.messages().last()
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
          self.start_stream().await;
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
    let tool_calls = match self.context.messages().last() {
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
          self.context.push_tool_result(&tool_call.id, &output);
          let tool_msg = Message::tool(&output, &tool_call.id);
          self.persistence.stage_message(&self.id, &tool_msg);
          let _ = self.persistence.flush();
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
            self.context.push_tool_result(&tool_call.id, &error_msg);
            let tool_msg = Message::tool(&error_msg, &tool_call.id);
            self.persistence.stage_message(&self.id, &tool_msg);
            let _ = self.persistence.flush();
            continue;
          }
        }
      }

      if !self.should_auto_approve(&tool_call.name) {
        info!(
          "Session {}: Tool {} requires approval, pausing execution",
          self.id, tool_call.name
        );
        let diff_preview = self.tool_executor.preview(tool_call).await;
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
    for (i, msg) in self.context.messages().iter().enumerate() {
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
    self.start_stream().await;
  }

  /// Execute a single tool call and store the result.
  async fn execute_single_tool_call(&mut self, tool_call: &ToolCall) {
    match self.tool_executor.execute(tool_call).await {
      Ok(output) => {
        let output_str = output;

        let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
          name: tool_call.name.clone(),
          output: output_str.clone(),
        });

        info!(
          "Session {}: Adding tool result message: tool_call_id={}, output_preview={}...",
          self.id,
          tool_call.id,
          output_str.chars().take(100).collect::<String>()
        );
        self.context.push_tool_result(&tool_call.id, &output_str);
        let tool_msg = Message::tool(&output_str, &tool_call.id);
        self.persistence.stage_message(&self.id, &tool_msg);
        let _ = self.persistence.flush();
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

        info!(
          "Session {}: Adding tool error message: tool_call_id={}, error={}",
          self.id, tool_call.id, error_msg
        );
        self.context.push_tool_result(&tool_call.id, &error_msg);
        let tool_msg = Message::tool(&error_msg, &tool_call.id);
        self.persistence.stage_message(&self.id, &tool_msg);
        let _ = self.persistence.flush();
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

      self.context.push_tool_result(&tool_call.id, &denied_msg);
      let tool_msg = Message::tool(&denied_msg, &tool_call.id);
      self.persistence.stage_message(&self.id, &tool_msg);
      let _ = self.persistence.flush();
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

    self.context.push_tool_result(&tool_call.id, &output);
    let tool_msg = Message::tool(&output, &tool_call.id);
    self.persistence.stage_message(&self.id, &tool_msg);
    let _ = self.persistence.flush();

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
    let mut meta = SessionMeta::new(generate_session_id(), &system_prompt);
    meta.yolo = runtime.yolo();
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
    let provider = Self::create_provider(&runtime)?;

    let yolo = meta.yolo;
    let session = Self::start_with_messages(
      id,
      provider,
      messages.clone(),
      session_store,
      meta,
      yolo,
      runtime.auto_approve(),
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
    let tool_registry = runtime.tool_registry.clone();

    // Get default model configuration
    let model_config = runtime
      .default_model_config()
      .ok_or(crate::config::Error::MissingDefaultModel)?;

    // Get provider configuration
    let provider = runtime
      .get_provider(&model_config.provider)
      .ok_or_else(|| crate::config::Error::ProviderNotFound {
        provider: model_config.provider.clone(),
        model: runtime.default_model(),
      })?;

    // Resolve API key (may contain env var references like ${OPENAI_API_KEY})
    let api_key = provider
      .api_key
      .as_ref()
      .map(|key| runtime.resolve_api_key(key))
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
    chat_config = chat_config.with_thinking(runtime.default_thinking());

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
            model: runtime.default_model(),
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
      runtime.auto_approve(),
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
