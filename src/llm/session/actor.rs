//! Chat session management module (Actor pattern).
//!
//! Manages a conversation with an LLM using the actor pattern.
//! The session runs in a dedicated tokio task and communicates via channels.

use std::env;
use std::future::pending;
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Datelike, Local, Timelike};
use log::{debug, error, info, warn};
use tokio::sync::mpsc;

use super::approval::{ApprovalDecision, ApprovalService};
use super::compaction::CompactionService;
use super::state::ActorState;
use crate::cli::runtime::Runtime;
use crate::error::Result;
use crate::hooks::{HookDecision, HookEventType, events as hook_events};
use crate::llm::provider::LLMProvider;
use crate::llm::providers::KimiProvider;
use crate::llm::types::{ChatConfig, Message, ToolCall};
use crate::notification::llm::{
  build_notification_message, extract_notification_ids, is_notification_message,
};
use crate::session::{SessionMeta, SessionMode, SessionStore};
use crate::utils::plan::{plan_file_path, read_plan};
use crate::utils::token_counter::estimate_llm_messages_tokens;

/// Maximum characters to display in user input log preview
const USER_INPUT_PREVIEW_LEN: usize = 50;

/// Default maximum steps per turn to prevent infinite tool-call loops.
const DEFAULT_MAX_STEPS_PER_TURN: usize = 10;

/// Tracks whether the actor is running inside an explicit agent loop.
#[derive(Debug, Clone, Copy)]
enum AgentLoopState {
  /// No agent loop is active.
  Idle,
  /// Inside a turn loop; `step_count` is the number of LLM calls made so far.
  InProgress { step_count: usize },
}

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
use super::tool_exec::{ToolExecutionContext, ToolExecutor};
use super::{Question, QuestionOption, SessionCommand, SessionEvent, SessionHandle};
use crate::notification::NotificationManager;
use crate::wire::{WireMessage, WirePublisher};

/// Acknowledge any notification IDs embedded in the given messages.
///
/// Used on session resume to avoid redelivering notifications that are already
/// part of the persisted context. Mirrors kimi-cli's `extract_notification_ids`.
fn ack_notification_ids_in_context(manager: &NotificationManager, messages: &[Message]) {
  for msg in messages {
    if msg.role != crate::llm::types::Role::User {
      continue;
    }
    if !is_notification_message(&msg.content) {
      continue;
    }
    for id in extract_notification_ids(&msg.content) {
      manager.ack("llm", &id);
    }
  }
}

/// Internal state of the session actor
struct SessionActor {
  /// Session ID
  id: String,
  /// Message history managed by Context
  context: Context,
  /// Wire publisher for sending events to the UI layer
  wire_publisher: WirePublisher,
  /// Channel to receive commands
  cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
  /// Current response being accumulated
  current_response: String,
  /// Current thinking content being accumulated
  current_thinking: String,
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
  /// Tool executor for dispatching and previewing tool calls
  tool_executor: ToolExecutor,
  /// Approval policy service for tool call decisions
  approval_service: ApprovalService,
  /// Compaction monitoring and execution service
  compaction_service: CompactionService,
  /// Runtime context (config, tool registries, etc.)
  #[allow(dead_code)]
  runtime: Arc<Runtime>,
  /// Explicit actor state replacing is_streaming + tool_call_execution_state + pending_answers_state
  state: ActorState,
  /// Agent loop state — tracks whether we're in an explicit turn loop and step count.
  agent_loop_state: AgentLoopState,
  /// Maximum steps (LLM calls) allowed per turn to prevent infinite loops.
  max_steps_per_turn: usize,
  /// Whether plan mode is active (read-only tools only).
  plan_mode: bool,
  /// Guard to prevent recursive Stop hook re-triggering.
  stop_hook_active: bool,
}

impl SessionActor {
  #[allow(clippy::too_many_arguments)]
  fn new(
    id: String,
    provider: Box<dyn LLMProvider>,
    messages: Vec<Message>,
    wire_publisher: WirePublisher,
    cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    runtime: Arc<Runtime>,
  ) -> Self {
    let max_context_size = provider.max_context_size();
    let yolo = meta.yolo;
    let plan_mode = meta.plan_mode;

    // Ack any notification IDs that are already present in the resumed
    // context so they are not redelivered to the LLM.
    // This mirrors kimi-cli's extract_notification_ids behaviour.
    ack_notification_ids_in_context(&runtime.notification_manager(), &messages);

    Self {
      id: id.clone(),
      context: Context::from_messages(messages),
      wire_publisher,
      cmd_rx,
      current_response: String::new(),
      current_thinking: String::new(),
      stream_rx: None,
      stream_manager: StreamManager::new(provider, runtime.retry_config()),
      pending_tool_calls: Vec::new(),
      persistence: SessionPersistence::new(session_store),
      meta,
      precise_token_count: None,
      max_context_size,
      tool_executor: {
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut executor = ToolExecutor::new(
          cwd.clone(),
          runtime.executable_registry.clone(),
          runtime.hook_engine(),
        );
        executor.bind_context(ToolExecutionContext {
          session_id: id.clone(),
          cwd,
        });
        executor
      },
      approval_service: ApprovalService::new(yolo, runtime.auto_approve()),
      compaction_service: CompactionService::new(runtime.compaction_config().clone()),
      runtime,
      state: ActorState::Idle,
      agent_loop_state: AgentLoopState::Idle,
      max_steps_per_turn: DEFAULT_MAX_STEPS_PER_TURN,
      plan_mode,
      stop_hook_active: false,
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

  /// Trigger `UserPromptSubmit` hooks.
  ///
  /// Returns `Some(reason)` if any hook blocked the prompt, or `None` if the
  /// prompt should proceed.
  async fn run_user_prompt_submit_hook(&self, content: &str) -> Option<String> {
    let results = self
      .runtime
      .hook_engine()
      .trigger(
        HookEventType::UserPromptSubmit,
        content,
        hook_events::user_prompt_submit(&self.id, &self.runtime.args.work_dir, content),
      )
      .await;

    for result in results {
      if let HookDecision::Block { reason } = result.decision {
        return Some(if reason.is_empty() {
          "Prompt blocked by hook.".to_string()
        } else {
          reason
        });
      }
    }

    None
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

        if !matches!(self.state, ActorState::Idle) {
          error!(
            "Session {}: Cannot send message while another request is in progress",
            self.id
          );
          self.wire_publisher.send(WireMessage::Error {
            message: "Cannot send message while another request is in progress".to_string(),
          });
          return true;
        }

        // Trigger UserPromptSubmit hooks; hooks may block the prompt.
        if let Some(reason) = self.run_user_prompt_submit_hook(&content).await {
          self.wire_publisher.send(WireMessage::TurnBegin);
          self
            .wire_publisher
            .send(WireMessage::ContentChunk { text: reason });
          self.wire_publisher.send(WireMessage::TurnEnd);
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

        // Start the explicit agent loop for this turn.
        self.start_turn().await;
        true
      }

      SessionCommand::Cancel => {
        if matches!(self.state, ActorState::Streaming) {
          info!("Session {}: Cancelling stream", self.id);
          self.stream_rx = None;
          self.state = ActorState::Idle;
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
        if matches!(self.state, ActorState::Streaming) {
          self.stream_rx = None;
          self.state = ActorState::Idle;
        }
        true
      }

      SessionCommand::ApproveToolCall {
        tool_call_id,
        approved,
        reject_remaining,
      } => {
        self
          .handle_approval(tool_call_id, approved, reject_remaining)
          .await;
        true
      }

      SessionCommand::AnswerQuestions {
        tool_call_id,
        answers,
        dismissed,
      } => {
        match &self.state {
          ActorState::WaitingEnterPlanMode { .. } => {
            self
              .handle_plan_mode_enter_answer(tool_call_id, answers, dismissed)
              .await;
          }
          ActorState::WaitingExitPlanMode { .. } => {
            self
              .handle_plan_mode_exit_answer(tool_call_id, answers, dismissed)
              .await;
          }
          _ => {
            self
              .handle_question_answers(tool_call_id, answers, dismissed)
              .await;
          }
        }
        true
      }

      SessionCommand::EnableSessionYolo => {
        info!("Session {}: Enabling YOLO mode for this session", self.id);
        self.approval_service.set_yolo(true);
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
        self.wire_publisher.send(WireMessage::Error {
          message: "Session shutdown".to_string(),
        });
        false
      }
    }
  }

  /// Check if compaction is needed and send notification if so.
  async fn check_and_notify_compaction(&mut self) {
    // Estimate current token count
    let current_tokens = if let Some(precise) = self.precise_token_count {
      let recent_tokens = estimate_llm_messages_tokens(
        &self.context.messages()[self.context.len().saturating_sub(2)..],
      );
      precise as usize + recent_tokens
    } else {
      self.context.estimate_tokens()
    };

    if let Some(trigger) = self
      .compaction_service
      .check(current_tokens, self.max_context_size)
    {
      info!(
        "Session {}: Compaction needed - {} tokens (threshold: {})",
        self.id, trigger.current_tokens, trigger.threshold
      );
      self.wire_publisher.send(WireMessage::CompactionWarning {
        current_tokens: trigger.current_tokens,
        threshold: trigger.threshold,
        max_context_size: trigger.max_context_size,
      });
      self.execute_compaction().await;
    }
  }

  /// Execute compaction on the current messages.
  ///
  /// Uses the configured compaction strategy to compress message history.
  /// Updates the session store and notifies the UI of completion.
  async fn execute_compaction(&mut self) {
    if let Some(result) = self.compaction_service.execute(self.context.messages_mut()) {
      info!(
        "Session {}: Compaction completed - {} messages -> {} messages, ~{} tokens",
        self.id, result.message_count_before, result.message_count_after, result.new_token_count
      );
      self
        .persistence
        .reset_messages(&self.id, self.context.messages());
      self.wire_publisher.send(WireMessage::CompactionCompleted {
        before: result.message_count_before,
        after: result.message_count_after,
        tokens: result.new_token_count,
      });
    }
  }

  /// Start a new turn with the explicit agent loop.
  ///
  /// Initializes loop state, sends `TurnBegin`, and kicks off the first
  /// iteration.  The loop continues (via `agent_loop_continue`) until the
  /// model responds without tool calls or a limit is reached.
  async fn start_turn(&mut self) {
    info!("Session {}: Starting new turn", self.id);
    self.wire_publisher.send(WireMessage::TurnBegin);
    self.agent_loop_state = AgentLoopState::InProgress { step_count: 0 };
    self.agent_loop_iteration().await;
  }

  /// Single iteration of the agent loop.
  ///
  /// Checks `max_steps_per_turn`, checks compaction, then starts the stream.
  /// This is called at the beginning of every step (including retries).
  async fn agent_loop_iteration(&mut self) {
    let step_count = match self.agent_loop_state {
      AgentLoopState::InProgress { step_count } => step_count,
      _ => {
        // Not inside an agent loop — fall back to a direct stream start.
        self.start_stream().await;
        return;
      }
    };

    if step_count >= self.max_steps_per_turn {
      let msg = format!(
        "Reached maximum steps per turn ({})",
        self.max_steps_per_turn
      );
      error!("Session {}: {}", self.id, msg);
      self
        .wire_publisher
        .send(WireMessage::Error { message: msg });
      self.agent_loop_end().await;
      return;
    }

    info!(
      "Session {}: Agent loop step {}/{}",
      self.id, step_count, self.max_steps_per_turn
    );

    // Check compaction before *every* step, not just the first one.
    self.check_and_notify_compaction().await;

    self.start_stream().await;
  }

  /// Continue the agent loop after tool execution.
  ///
  /// Increments the step counter and starts the next iteration.
  async fn agent_loop_continue(&mut self) {
    if let AgentLoopState::InProgress { ref mut step_count } = self.agent_loop_state {
      *step_count += 1;
    }
    self.agent_loop_iteration().await;
  }

  /// End the current turn.
  ///
  /// Resets agent-loop state and sends `TurnEnd` to the UI. Then triggers the
  /// `Stop` lifecycle hook; if it blocks with a reason, that reason is injected
  /// as a new user message and one additional turn is run.
  async fn agent_loop_end(&mut self) {
    info!("Session {}: Turn ended", self.id);
    self.agent_loop_state = AgentLoopState::Idle;
    self.wire_publisher.send(WireMessage::TurnEnd);

    // --- Stop hook (max 1 re-trigger to prevent infinite loop) ---
    if !self.stop_hook_active {
      let results = self
        .runtime
        .hook_engine()
        .trigger(
          HookEventType::Stop,
          "",
          hook_events::stop(&self.id, &self.runtime.args.work_dir, false),
        )
        .await;
      for result in results {
        if let HookDecision::Block { reason } = result.decision
          && !reason.is_empty()
        {
          self.stop_hook_active = true;
          let user_msg = self.context.push_user(&reason).clone();
          self.persistence.stage_message(&self.id, &user_msg);
          self.meta.update_title_from_message(&user_msg);
          self.meta.updated_at = Local::now();
          self.persistence.stage_meta(&self.meta);
          let _ = self.persistence.flush();
          self.current_response.clear();
          self.current_thinking.clear();
          self.pending_tool_calls.clear();
          Box::pin(self.start_turn()).await;
          self.stop_hook_active = false;
          return;
        }
      }
    }
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
    // Deliver pending LLM notifications: claim, append to context as user
    // messages, then ack. This mirrors kimi-cli's behaviour where
    // notifications are user-role messages interleaved with history.
    let manager = self.runtime.background_manager();
    self
      .runtime
      .notification_manager()
      .deliver_pending("llm", 8, |view| {
        let text = build_notification_message(view, Some(&*manager));
        self.context.messages_mut().push(Message::user(text));
        Ok::<(), std::convert::Infallible>(())
      });

    let messages = self.context.messages().to_vec();

    match self.stream_manager.start(messages).await {
      Ok(rx) => {
        self.stream_rx = Some(rx);
        self.state = ActorState::Streaming;
      }
      Err(err) => {
        error!("Session {}: failed to start stream: {}", self.id, err);
        self.wire_publisher.send(WireMessage::Error {
          message: format_user_friendly_error(&err.to_string()),
        });
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
        self.wire_publisher.send(WireMessage::ContentChunk {
          text: chunk.clone(),
        });
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
        self.wire_publisher.send(WireMessage::ThinkingChunk {
          text: chunk.clone(),
        });
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
        self.wire_publisher.send(WireMessage::ToolCallBegin {
          id: id.clone(),
          name: name.clone(),
          arguments: arguments.clone(),
        });
      }
      SessionEvent::ToolCallCompleted { name, output } => {
        info!(
          "Session {}: Tool call completed: name={}, output_len={}",
          self.id,
          name,
          output.len()
        );
        // Forward to caller
        // Note: id is not available in SessionEvent::ToolCallCompleted, use empty string
        self.wire_publisher.send(WireMessage::ToolCallEnd {
          id: String::new(),
          name: name.clone(),
          output: output.clone(),
        });
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
        self.wire_publisher.send(WireMessage::Usage {
          total_tokens: *total_tokens,
          prompt_tokens: *prompt_tokens,
          completion_tokens: *completion_tokens,
        });
      }
      SessionEvent::Completed => {
        // Add the complete assistant message to history (with tool calls if any)
        let response = mem::take(&mut self.current_response);
        let thinking = mem::take(&mut self.current_thinking);
        let tool_calls = mem::take(&mut self.pending_tool_calls);

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
            .push_assistant(response, thinking_opt, tool_calls.clone())
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

        self.state = ActorState::Idle;
        self.stream_rx = None;

        // Use the tool_calls we already have in hand instead of re-reading
        // from context.messages().last().
        if !tool_calls.is_empty() {
          info!(
            "Session {}: Executing {} tool calls",
            self.id,
            tool_calls.len()
          );
          self.state = ActorState::ExecutingTools {
            tool_calls,
            current_index: 0,
          };
          self.continue_tool_call_execution().await;
          return;
        }

        // End the turn via the explicit agent loop.
        self.agent_loop_end().await;
      }
      SessionEvent::StreamInterrupted {
        error,
        is_retryable,
      } => {
        error!("Session {}: Stream interrupted: {}", self.id, error);
        self.state = ActorState::Idle;
        self.stream_rx = None;
        // Do NOT retain partial content — mirrors kimi-cli behaviour
        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();

        if *is_retryable {
          if matches!(self.agent_loop_state, AgentLoopState::InProgress { .. }) {
            // Retry the same step (do not increment step_count).
            self.agent_loop_iteration().await;
          } else {
            self.start_stream().await;
          }
        } else {
          self.wire_publisher.send(WireMessage::Error {
            message: format_user_friendly_error(error),
          });
          if matches!(self.agent_loop_state, AgentLoopState::InProgress { .. }) {
            self.agent_loop_end().await;
          }
        }
      }
      SessionEvent::Error(err) => {
        error!("Session {}: Stream error: {}", self.id, err);
        self.state = ActorState::Idle;
        self.stream_rx = None;
        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();
        // Forward to caller
        self.wire_publisher.send(WireMessage::Error {
          message: err.clone(),
        });
        if matches!(self.agent_loop_state, AgentLoopState::InProgress { .. }) {
          self.agent_loop_end().await;
        }
      }
      SessionEvent::Shutdown => {
        // Should not happen, but handle it
        self.wire_publisher.send(WireMessage::Error {
          message: "Session shutdown".to_string(),
        });
      }
      SessionEvent::CompactionNeeded {
        current_tokens,
        threshold,
        max_context_size,
      } => {
        // Forward compaction notification to UI
        self.wire_publisher.send(WireMessage::CompactionWarning {
          current_tokens: *current_tokens,
          threshold: *threshold,
          max_context_size: *max_context_size,
        });
      }
      SessionEvent::ApprovalNeeded {
        id,
        name,
        arguments: _,
        diff_preview,
      } => {
        // ApprovalNeeded is never emitted by the stream task,
        // but the match must be exhaustive.
        self.wire_publisher.send(WireMessage::ApprovalRequest {
          id: id.clone(),
          name: name.clone(),
          diff_preview: diff_preview.clone(),
          position: 1,
          total: 1,
        });
      }
      SessionEvent::QuestionsAsked {
        tool_call_id,
        questions,
      } => {
        // Forward to UI
        self.wire_publisher.send(WireMessage::QuestionsAsked {
          tool_call_id: tool_call_id.clone(),
          questions: questions.clone(),
        });
      }
      SessionEvent::CompactionCompleted {
        message_count_before,
        message_count_after,
        new_token_count,
      } => {
        // Forward compaction completion to UI
        self.wire_publisher.send(WireMessage::CompactionCompleted {
          before: *message_count_before,
          after: *message_count_after,
          tokens: *new_token_count,
        });
      }
    }
  }

  /// Resume tool call execution from the pending state.
  ///
  /// Tools that require UI interaction (AskUserQuestion or manual approval) are
  /// handled serially because they may pause the actor.  All other approved
  /// tools are executed concurrently via `execute_tool_batch`.
  async fn continue_tool_call_execution(&mut self) {
    let (tool_calls, current_index) = match mem::replace(&mut self.state, ActorState::Idle) {
      ActorState::ExecutingTools {
        tool_calls,
        current_index,
      } => (tool_calls, current_index),
      _ => {
        error!(
          "Session {}: continue_tool_call_execution called with no state",
          self.id
        );
        return;
      }
    };

    let mut batch: Vec<&ToolCall> = Vec::new();

    for (i, tool_call) in tool_calls.iter().enumerate().skip(current_index) {
      self.wire_publisher.send(WireMessage::ToolCallBegin {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        arguments: tool_call.arguments.clone(),
      });

      // Plan mode: reject non-read-only tools immediately
      if self.plan_mode && !self.is_tool_allowed_in_plan_mode(&tool_call.name) {
        let error_msg = format!(
          "Tool '{}' is not available in plan mode. \
           Only read-only tools (ReadFile, Glob, Grep, SearchWeb, FetchURL) \
           are allowed while planning.",
          tool_call.name
        );
        self.wire_publisher.send(WireMessage::ToolCallEnd {
          id: tool_call.id.clone(),
          name: tool_call.name.clone(),
          output: error_msg.clone(),
        });
        self.context.push_tool_result(&tool_call.id, &error_msg);
        let tool_msg = Message::tool(&error_msg, &tool_call.id);
        self.persistence.stage_message(&self.id, &tool_msg);
        let _ = self.persistence.flush();
        warn!(
          "Session {}: Blocked tool {} in plan mode",
          self.id, tool_call.name
        );
        continue;
      }

      // Special handling for EnterPlanMode: pause execution and wait for user confirmation
      if tool_call.name == "EnterPlanMode" {
        // Guard: already in plan mode
        if self.plan_mode {
          let error_msg =
            "Already in plan mode. Use ExitPlanMode when your plan is ready.".to_string();
          self.wire_publisher.send(WireMessage::ToolCallEnd {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            output: error_msg.clone(),
          });
          self.context.push_tool_result(&tool_call.id, &error_msg);
          let tool_msg = Message::tool(&error_msg, &tool_call.id);
          self.persistence.stage_message(&self.id, &tool_msg);
          let _ = self.persistence.flush();
          continue;
        }

        // YOLO mode: auto-approve
        if self.approval_service.is_yolo() {
          if !batch.is_empty() {
            self.execute_tool_batch(std::mem::take(&mut batch)).await;
          }
          self.handle_plan_mode_transition(true).await;
          let plan_path = plan_file_path(&self.id, None);
          let output = format!(
            "Plan mode activated (auto-approved in non-interactive mode).\n\
             Plan file: {}\n\
             Workflow: identify key questions about the codebase → \
             use Agent(subagent_type='explore') to investigate if needed → \
             design approach → \
             modify the plan file with WriteFile or StrReplaceFile \
             (create it with WriteFile first if it does not exist) → \
             call ExitPlanMode.\n",
            plan_path.display()
          );
          self.wire_publisher.send(WireMessage::ToolCallEnd {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            output: output.clone(),
          });
          self.context.push_tool_result(&tool_call.id, &output);
          let tool_msg = Message::tool(&output, &tool_call.id);
          self.persistence.stage_message(&self.id, &tool_msg);
          let _ = self.persistence.flush();
          continue;
        }

        // Flush pending batch before pausing.
        if !batch.is_empty() {
          self.execute_tool_batch(std::mem::take(&mut batch)).await;
        }

        info!(
          "Session {}: EnterPlanMode paused, waiting for user confirmation",
          self.id
        );
        self.state = ActorState::WaitingEnterPlanMode {
          tool_calls: tool_calls.clone(),
          current_index: i,
          tool_call_id: tool_call.id.clone(),
        };
        self.wire_publisher.send(WireMessage::QuestionsAsked {
          tool_call_id: tool_call.id.clone(),
          questions: vec![Question {
            question: "Enter plan mode?".to_string(),
            header: "Plan Mode".to_string(),
            options: vec![
              QuestionOption {
                label: "Yes".to_string(),
                description: "Enter plan mode to explore and design an approach".to_string(),
              },
              QuestionOption {
                label: "No".to_string(),
                description: "Skip planning, start implementing now".to_string(),
              },
            ],
            multi_select: false,
            confirmation: true,
            default: vec![0],
            required: true,
          }],
        });
        return;
      }

      // Special handling for ExitPlanMode: pause execution and wait for user approval
      if tool_call.name == "ExitPlanMode" {
        // Guard: only works in plan mode
        if !self.plan_mode {
          let error_msg =
            "Not in plan mode. ExitPlanMode is only available during plan mode.".to_string();
          self.wire_publisher.send(WireMessage::ToolCallEnd {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            output: error_msg.clone(),
          });
          self.context.push_tool_result(&tool_call.id, &error_msg);
          let tool_msg = Message::tool(&error_msg, &tool_call.id);
          self.persistence.stage_message(&self.id, &tool_msg);
          let _ = self.persistence.flush();
          continue;
        }

        // Parse arguments to get custom options
        let exit_args = parse_exit_plan_mode_args(&tool_call.arguments);
        let has_options = exit_args
          .as_ref()
          .map(|a| a.options.as_ref().map(|o| o.len() >= 2).unwrap_or(false))
          .unwrap_or(false);
        let custom_options: Vec<(String, String)> = if has_options {
          exit_args
            .unwrap()
            .options
            .unwrap()
            .into_iter()
            .map(|o| (o.label, o.description))
            .collect()
        } else {
          Vec::new()
        };

        // Read the plan file
        let plan_path = plan_file_path(&self.id, None);
        let plan_content = read_plan(&self.id, None);

        if plan_content.is_none() {
          let error_msg = format!(
            "No plan file found. Write your plan to {} first, then call ExitPlanMode.",
            plan_path.display()
          );
          self.wire_publisher.send(WireMessage::ToolCallEnd {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            output: error_msg.clone(),
          });
          self.context.push_tool_result(&tool_call.id, &error_msg);
          let tool_msg = Message::tool(&error_msg, &tool_call.id);
          self.persistence.stage_message(&self.id, &tool_msg);
          let _ = self.persistence.flush();
          continue;
        }
        let plan_content = plan_content.unwrap();

        // YOLO mode: auto-approve
        if self.approval_service.is_yolo() {
          if !batch.is_empty() {
            self.execute_tool_batch(std::mem::take(&mut batch)).await;
          }
          self.handle_plan_mode_transition(false).await;
          let output = format!(
            "Plan approved (auto-approved in non-interactive mode). \
             Plan mode deactivated. All tools are now available.\n\
             Plan saved to: {}\n\n\
             ## Approved Plan:\n{}",
            plan_path.display(),
            plan_content
          );
          self.wire_publisher.send(WireMessage::ToolCallEnd {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            output: output.clone(),
          });
          self.context.push_tool_result(&tool_call.id, &output);
          let tool_msg = Message::tool(&output, &tool_call.id);
          self.persistence.stage_message(&self.id, &tool_msg);
          let _ = self.persistence.flush();
          continue;
        }

        // Flush pending batch before pausing.
        if !batch.is_empty() {
          self.execute_tool_batch(std::mem::take(&mut batch)).await;
        }

        // Build question options
        let mut question_options = Vec::new();
        if has_options {
          for (label, description) in &custom_options {
            question_options.push(QuestionOption {
              label: label.clone(),
              description: description.clone(),
            });
          }
        } else {
          question_options.push(QuestionOption {
            label: "Approve".to_string(),
            description: "Exit plan mode and start execution".to_string(),
          });
        }
        question_options.push(QuestionOption {
          label: "Reject".to_string(),
          description: "Reject and stay in plan mode".to_string(),
        });
        question_options.push(QuestionOption {
          label: "Reject and Exit".to_string(),
          description: "Reject and exit plan mode".to_string(),
        });
        question_options.push(QuestionOption {
          label: "Revise".to_string(),
          description: "Stay in plan mode and provide feedback".to_string(),
        });

        // Display plan content inline
        self.wire_publisher.send(WireMessage::PlanDisplay {
          content: plan_content.clone(),
          file_path: plan_path.display().to_string(),
        });

        info!(
          "Session {}: ExitPlanMode paused, waiting for user approval",
          self.id
        );
        let all_option_labels: Vec<String> =
          question_options.iter().map(|o| o.label.clone()).collect();
        self.state = ActorState::WaitingExitPlanMode {
          tool_calls: tool_calls.clone(),
          current_index: i,
          tool_call_id: tool_call.id.clone(),
          has_options,
          option_labels: custom_options.into_iter().map(|(l, _)| l).collect(),
          all_option_labels,
        };
        self.wire_publisher.send(WireMessage::QuestionsAsked {
          tool_call_id: tool_call.id.clone(),
          questions: vec![Question {
            question: "Approve this plan".to_string(),
            header: "Plan".to_string(),
            options: question_options,
            multi_select: false,
            confirmation: false,
            default: vec![0],
            required: true,
          }],
        });
        return;
      }

      // Special handling for AskUserQuestion: pause execution and wait for answers
      if tool_call.name == "AskUserQuestion" {
        if self.approval_service.is_yolo() {
          info!(
            "Session {}: AskUserQuestion in YOLO mode, auto-dismissing",
            self.id
          );

          // Flush any pending batch before handling the inline response.
          if !batch.is_empty() {
            self.execute_tool_batch(std::mem::take(&mut batch)).await;
          }

          let output = r#"{"answers": {}, "note": "Running in non-interactive (yolo) mode. Make your own decision."}"#.to_string();
          self.wire_publisher.send(WireMessage::ToolCallEnd {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            output: output.clone(),
          });
          self.context.push_tool_result(&tool_call.id, &output);
          let tool_msg = Message::tool(&output, &tool_call.id);
          self.persistence.stage_message(&self.id, &tool_msg);
          let _ = self.persistence.flush();
          continue;
        }

        // Flush pending batch before pausing.
        if !batch.is_empty() {
          self.execute_tool_batch(std::mem::take(&mut batch)).await;
        }

        match parse_ask_user_questions(&tool_call.arguments) {
          Ok(questions) => {
            info!(
              "Session {}: AskUserQuestion paused, waiting for user answers",
              self.id
            );
            self.state = ActorState::WaitingAnswers {
              tool_calls: tool_calls.clone(),
              current_index: i,
              tool_call_id: tool_call.id.clone(),
            };
            self.wire_publisher.send(WireMessage::QuestionsAsked {
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
            self.wire_publisher.send(WireMessage::ToolCallEnd {
              id: tool_call.id.clone(),
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

      let diff_preview = self.tool_executor.preview(tool_call).await;
      match self.approval_service.decide(tool_call, diff_preview) {
        ApprovalDecision::Approved => {
          batch.push(tool_call);
        }
        ApprovalDecision::NeedsApproval {
          tool_call_id,
          name,
          diff_preview,
        } => {
          // Flush pending batch before pausing for approval.
          if !batch.is_empty() {
            self.execute_tool_batch(std::mem::take(&mut batch)).await;
          }

          info!(
            "Session {}: Tool {} requires approval, pausing execution",
            self.id, name
          );
          self.state = ActorState::WaitingApproval {
            tool_calls: tool_calls.clone(),
            current_index: i,
            approved_indices: vec![],
            rejected_indices: vec![],
          };
          self.wire_publisher.send(WireMessage::ApprovalRequest {
            id: tool_call_id,
            name,
            diff_preview,
            position: i + 1,
            total: tool_calls.len(),
          });
          return;
        }
      }
    }

    // Execute any remaining approved tools concurrently.
    if !batch.is_empty() {
      self.execute_tool_batch(batch).await;
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
    self.agent_loop_continue().await;
  }

  /// Execute a batch of approved tool calls concurrently.
  ///
  /// Results are processed in the original order so that context messages
  /// remain deterministic. Also handles plan mode state transitions for
  /// EnterPlanMode and ExitPlanMode.
  async fn execute_tool_batch(&mut self, tool_calls: Vec<&ToolCall>) {
    let results = self.tool_executor.execute_many(&tool_calls).await;

    for (tool_call, result) in tool_calls.iter().zip(results.into_iter()) {
      match result {
        Ok(output) => {
          self.wire_publisher.send(WireMessage::ToolCallEnd {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            output: output.clone(),
          });
          info!(
            "Session {}: Adding tool result message: tool_call_id={}, output_preview={}...",
            self.id,
            tool_call.id,
            output.chars().take(100).collect::<String>()
          );
          self.context.push_tool_result(&tool_call.id, &output);
          let tool_msg = Message::tool(&output, &tool_call.id);
          self.persistence.stage_message(&self.id, &tool_msg);
          let _ = self.persistence.flush();
          info!(
            "Session {}: Tool {} executed successfully, output_len={}",
            self.id,
            tool_call.name,
            output.len()
          );

          // Handle plan mode state transitions for tools executed via batch
          if tool_call.name == "EnterPlanMode" {
            self.handle_plan_mode_transition(true).await;
          } else if tool_call.name == "ExitPlanMode" {
            self.handle_plan_mode_transition(false).await;
          }
        }
        Err(e) => {
          let error_msg = format!("Error: {}", e);
          self.wire_publisher.send(WireMessage::ToolCallEnd {
            id: tool_call.id.clone(),
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
  }

  /// Toggle plan mode state and notify UI.
  async fn handle_plan_mode_transition(&mut self, entering: bool) {
    if entering {
      if !self.plan_mode {
        self.plan_mode = true;
        self.meta.plan_mode = true;
        self.persistence.stage_meta(&self.meta);
        let _ = self.persistence.flush();
        self
          .wire_publisher
          .send(WireMessage::PlanModeChanged { active: true });
        info!("Session {}: Entered plan mode", self.id);
      }
    } else if self.plan_mode {
      self.plan_mode = false;
      self.meta.plan_mode = false;
      self.persistence.stage_meta(&self.meta);
      let _ = self.persistence.flush();
      self
        .wire_publisher
        .send(WireMessage::PlanModeChanged { active: false });
      info!("Session {}: Exited plan mode", self.id);
    }
  }

  /// Returns true if the given tool is allowed in plan mode.
  fn is_tool_allowed_in_plan_mode(&self, tool_name: &str) -> bool {
    const ALLOWED: &[&str] = &[
      "ReadFile",
      "Glob",
      "Grep",
      "SearchWeb",
      "FetchURL",
      "AskUserQuestion",
      "EnterPlanMode",
      "ExitPlanMode",
    ];
    ALLOWED.contains(&tool_name)
  }

  /// Execute a single tool call and store the result.
  async fn execute_single_tool_call(&mut self, tool_call: &ToolCall) {
    match self.tool_executor.execute(tool_call).await {
      Ok(output) => {
        let output_str = output;

        self.wire_publisher.send(WireMessage::ToolCallEnd {
          id: tool_call.id.clone(),
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

        self.wire_publisher.send(WireMessage::ToolCallEnd {
          id: tool_call.id.clone(),
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
  ///
  /// Records the decision in the approval queue and advances to the next tool
  /// that still needs manual approval.  Only when the queue is exhausted are
  /// all approved tools executed in a single batch.
  async fn handle_approval(
    &mut self,
    tool_call_id: String,
    approved: bool,
    reject_remaining: bool,
  ) {
    let (tool_calls, current_index, mut approved_indices, mut rejected_indices) =
      match mem::replace(&mut self.state, ActorState::Idle) {
        ActorState::WaitingApproval {
          tool_calls,
          current_index,
          approved_indices,
          rejected_indices,
        } => (
          tool_calls,
          current_index,
          approved_indices,
          rejected_indices,
        ),
        _ => {
          error!(
            "Session {}: Received approval but no tool execution is pending",
            self.id
          );
          return;
        }
      };

    let tool_call = &tool_calls[current_index];

    if tool_call.id != tool_call_id {
      error!(
        "Session {}: Approval tool_call_id mismatch: expected {}, got {}",
        self.id, tool_call.id, tool_call_id
      );
      self.state = ActorState::WaitingApproval {
        tool_calls,
        current_index,
        approved_indices,
        rejected_indices,
      };
      return;
    }

    if approved {
      info!(
        "Session {}: User approved tool {} (queued for batch execution)",
        self.id, tool_call.name
      );
      approved_indices.push(current_index);
    } else {
      let denied_msg = format!("User declined to execute tool: {}", tool_call.name);
      info!(
        "Session {}: User denied tool {}: {}",
        self.id, tool_call.name, denied_msg
      );

      self.wire_publisher.send(WireMessage::ToolCallEnd {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        output: denied_msg.clone(),
      });

      self.context.push_tool_result(&tool_call.id, &denied_msg);
      let tool_msg = Message::tool(&denied_msg, &tool_call.id);
      self.persistence.stage_message(&self.id, &tool_msg);
      let _ = self.persistence.flush();

      rejected_indices.push(current_index);
    }

    // Bulk-deny: reject every remaining non-interactive tool.
    if !approved && reject_remaining {
      for (i, tc) in tool_calls.iter().enumerate().skip(current_index + 1) {
        if matches!(
          tc.name.as_str(),
          "AskUserQuestion" | "EnterPlanMode" | "ExitPlanMode"
        ) {
          continue;
        }
        rejected_indices.push(i);
        let denied_msg = format!("User declined to execute tool: {}", tc.name);
        self.wire_publisher.send(WireMessage::ToolCallEnd {
          id: tc.id.clone(),
          name: tc.name.clone(),
          output: denied_msg.clone(),
        });
        self.context.push_tool_result(&tc.id, &denied_msg);
        let tool_msg = Message::tool(&denied_msg, &tc.id);
        self.persistence.stage_message(&self.id, &tool_msg);
      }
      let _ = self.persistence.flush();
    }

    // Scan forward to find the next tool that still needs manual approval.
    let start_index = if reject_remaining {
      tool_calls.len()
    } else {
      current_index + 1
    };

    let total = tool_calls.len();
    for i in start_index..total {
      if matches!(
        tool_calls[i].name.as_str(),
        "AskUserQuestion" | "EnterPlanMode" | "ExitPlanMode"
      ) {
        continue;
      }

      let diff_preview = self.tool_executor.preview(&tool_calls[i]).await;
      match self
        .approval_service
        .decide(&tool_calls[i], diff_preview.clone())
      {
        ApprovalDecision::NeedsApproval {
          tool_call_id: next_id,
          name: next_name,
          diff_preview: next_diff,
        } => {
          self.state = ActorState::WaitingApproval {
            tool_calls,
            current_index: i,
            approved_indices,
            rejected_indices,
          };
          self.wire_publisher.send(WireMessage::ApprovalRequest {
            id: next_id,
            name: next_name,
            diff_preview: next_diff,
            position: i + 1,
            total,
          });
          return;
        }
        ApprovalDecision::Approved => {
          // User may have enabled YOLO since the last scan, so subsequent
          // tools can flip to auto-approved.  Queue them for batch execution.
          approved_indices.push(i);
        }
      }
    }

    // Queue exhausted — execute every approved tool in order.
    for &i in &approved_indices {
      self.execute_single_tool_call(&tool_calls[i]).await;
    }

    info!(
      "Session {}: Approval queue complete — {} approved, {} rejected",
      self.id,
      approved_indices.len(),
      rejected_indices.len()
    );

    let total = tool_calls.len();
    self.state = ActorState::ExecutingTools {
      tool_calls,
      current_index: total,
    };
    self.continue_tool_call_execution().await;
  }

  /// Handle user answers to structured questions from AskUserQuestion.
  async fn handle_question_answers(
    &mut self,
    tool_call_id: String,
    answers: Vec<Vec<usize>>,
    dismissed: bool,
  ) {
    let (tool_calls, current_index, expected_id) =
      match mem::replace(&mut self.state, ActorState::Idle) {
        ActorState::WaitingAnswers {
          tool_calls,
          current_index,
          tool_call_id: expected_id,
        } => (tool_calls, current_index, expected_id),
        _ => {
          error!(
            "Session {}: Received question answers but no questions are pending",
            self.id
          );
          return;
        }
      };

    if expected_id != tool_call_id {
      error!(
        "Session {}: Question answer tool_call_id mismatch: expected {}, got {}",
        self.id, expected_id, tool_call_id
      );
      self.state = ActorState::WaitingAnswers {
        tool_calls,
        current_index,
        tool_call_id: expected_id,
      };
      return;
    }

    let tool_call = &tool_calls[current_index];

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

    self.wire_publisher.send(WireMessage::ToolCallEnd {
      id: tool_call.id.clone(),
      name: tool_call.name.clone(),
      output: output.clone(),
    });

    self.context.push_tool_result(&tool_call.id, &output);
    let tool_msg = Message::tool(&output, &tool_call.id);
    self.persistence.stage_message(&self.id, &tool_msg);
    let _ = self.persistence.flush();

    let next_index = current_index + 1;
    self.state = ActorState::ExecutingTools {
      tool_calls,
      current_index: next_index,
    };
    self.continue_tool_call_execution().await;
  }

  /// Handle user confirmation to enter plan mode.
  async fn handle_plan_mode_enter_answer(
    &mut self,
    tool_call_id: String,
    answers: Vec<Vec<usize>>,
    dismissed: bool,
  ) {
    let (tool_calls, current_index, expected_id) =
      match mem::replace(&mut self.state, ActorState::Idle) {
        ActorState::WaitingEnterPlanMode {
          tool_calls,
          current_index,
          tool_call_id: expected_id,
        } => (tool_calls, current_index, expected_id),
        _ => {
          error!(
            "Session {}: Received plan mode enter answer but no confirmation is pending",
            self.id
          );
          return;
        }
      };

    if expected_id != tool_call_id {
      error!(
        "Session {}: Plan mode enter tool_call_id mismatch: expected {}, got {}",
        self.id, expected_id, tool_call_id
      );
      self.state = ActorState::WaitingEnterPlanMode {
        tool_calls,
        current_index,
        tool_call_id: expected_id,
      };
      return;
    }

    let tool_call = &tool_calls[current_index];
    let plan_path = plan_file_path(&self.id, None);

    // Map answer: index 0 = Yes, index 1 = No
    let approved = !dismissed && answers.first().and_then(|a| a.first()).copied() == Some(0);

    let output = if dismissed {
      info!("Session {}: User dismissed EnterPlanMode", self.id);
      "User dismissed without choosing. Proceed with implementation directly.".to_string()
    } else if approved {
      self.handle_plan_mode_transition(true).await;
      info!("Session {}: User approved entering plan mode", self.id);
      format!(
        "Plan mode activated. You MUST NOT edit code files — only read and plan.\n\
         Plan file: {}\n\
         Workflow: identify key questions about the codebase → \
         use Agent(subagent_type='explore') to investigate if needed → \
         design approach → \
         modify the plan file with WriteFile or StrReplaceFile \
         (create it with WriteFile first if it does not exist) → \
         call ExitPlanMode.\n\
         Use AskUserQuestion only to clarify missing requirements or choose \
         between approaches.\n\
         Do NOT use AskUserQuestion to ask about plan approval.",
        plan_path.display()
      )
    } else {
      info!("Session {}: User declined entering plan mode", self.id);
      "User declined to enter plan mode. Please check with user whether \
       to proceed with implementation directly."
        .to_string()
    };

    self.wire_publisher.send(WireMessage::ToolCallEnd {
      id: tool_call.id.clone(),
      name: tool_call.name.clone(),
      output: output.clone(),
    });

    self.context.push_tool_result(&tool_call.id, &output);
    let tool_msg = Message::tool(&output, &tool_call.id);
    self.persistence.stage_message(&self.id, &tool_msg);
    let _ = self.persistence.flush();

    let next_index = current_index + 1;
    self.state = ActorState::ExecutingTools {
      tool_calls,
      current_index: next_index,
    };
    self.continue_tool_call_execution().await;
  }

  /// Handle user approval of a plan (ExitPlanMode).
  async fn handle_plan_mode_exit_answer(
    &mut self,
    tool_call_id: String,
    answers: Vec<Vec<usize>>,
    dismissed: bool,
  ) {
    let (tool_calls, current_index, expected_id, has_options, option_labels, all_option_labels) =
      match mem::replace(&mut self.state, ActorState::Idle) {
        ActorState::WaitingExitPlanMode {
          tool_calls,
          current_index,
          tool_call_id: expected_id,
          has_options,
          option_labels,
          all_option_labels,
        } => (
          tool_calls,
          current_index,
          expected_id,
          has_options,
          option_labels,
          all_option_labels,
        ),
        _ => {
          error!(
            "Session {}: Received plan mode exit answer but no approval is pending",
            self.id
          );
          return;
        }
      };

    if expected_id != tool_call_id {
      error!(
        "Session {}: Plan mode exit tool_call_id mismatch: expected {}, got {}",
        self.id, expected_id, tool_call_id
      );
      self.state = ActorState::WaitingExitPlanMode {
        tool_calls,
        current_index,
        tool_call_id: expected_id,
        has_options,
        option_labels,
        all_option_labels,
      };
      return;
    }

    let tool_call = &tool_calls[current_index];
    let plan_path = plan_file_path(&self.id, None);
    let plan_content = read_plan(&self.id, None).unwrap_or_default();

    // Map answer index to option label
    let choice = if dismissed || answers.is_empty() {
      String::new()
    } else if let Some(first_answer) = answers.first()
      && let Some(&idx) = first_answer.first()
      && let Some(label) = all_option_labels.get(idx)
    {
      label.clone()
    } else {
      String::new()
    };

    let output = if dismissed || choice.is_empty() {
      info!("Session {}: User dismissed ExitPlanMode", self.id);
      "User dismissed without choosing. Plan mode remains active. \
       Continue working on your plan or call ExitPlanMode again when ready."
        .to_string()
    } else if choice == "Reject and Exit" {
      self.handle_plan_mode_transition(false).await;
      info!(
        "Session {}: User rejected plan and exited plan mode",
        self.id
      );
      "Plan rejected by user. Plan mode deactivated. \
       All tools are now available. Wait for the user's next message."
        .to_string()
    } else if choice == "Reject" {
      info!(
        "Session {}: User rejected plan, staying in plan mode",
        self.id
      );
      "Plan rejected by user. Stay in plan mode. \
       The user will provide feedback via conversation. \
       Wait for the user's next message before revising."
        .to_string()
    } else if has_options && option_labels.contains(&choice) {
      self.handle_plan_mode_transition(false).await;
      info!(
        "Session {}: User approved plan with selected approach: {}",
        self.id, choice
      );
      format!(
        "Plan approved by user. Selected approach: \"{}\"\n\
         Plan mode deactivated. All tools are now available.\n\
         Plan saved to: {}\n\n\
         IMPORTANT: Execute ONLY the selected approach \"{}\". \
         Ignore other approaches in the plan.\n\n\
         ## Approved Plan:\n{}",
        choice,
        plan_path.display(),
        choice,
        plan_content
      )
    } else if choice == "Approve" {
      self.handle_plan_mode_transition(false).await;
      info!("Session {}: User approved plan", self.id);
      format!(
        "Plan approved by user. Plan mode deactivated. \
         All tools are now available.\n\
         Plan saved to: {}\n\n\
         ## Approved Plan:\n{}",
        plan_path.display(),
        plan_content
      )
    } else {
      // Revise or other free-text feedback
      info!("Session {}: User wants to revise plan", self.id);
      if choice.is_empty() {
        "User wants to revise the plan. Stay in plan mode. \
         Wait for the user's next message with feedback before revising."
          .to_string()
      } else {
        format!(
          "User wants to revise the plan. Stay in plan mode. \
           Revise based on the feedback below.\n\n\
           User feedback: {}",
          choice
        )
      }
    };

    self.wire_publisher.send(WireMessage::ToolCallEnd {
      id: tool_call.id.clone(),
      name: tool_call.name.clone(),
      output: output.clone(),
    });

    self.context.push_tool_result(&tool_call.id, &output);
    let tool_msg = Message::tool(&output, &tool_call.id);
    self.persistence.stage_message(&self.id, &tool_msg);
    let _ = self.persistence.flush();

    let next_index = current_index + 1;
    self.state = ActorState::ExecutingTools {
      tool_calls,
      current_index: next_index,
    };
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

// ---------------------------------------------------------------------------
// ExitPlanMode argument parsing (used by SessionActor)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct ExitPlanModeArgOption {
  label: String,
  #[serde(default)]
  description: String,
}

#[derive(Debug, serde::Deserialize)]
struct ExitPlanModeArgs {
  #[serde(default)]
  options: Option<Vec<ExitPlanModeArgOption>>,
}

fn parse_exit_plan_mode_args(arguments: &str) -> std::result::Result<ExitPlanModeArgs, String> {
  serde_json::from_str(arguments).map_err(|e| format!("JSON parse error: {}", e))
}

/// ChatSession that runs as an actor
#[derive(Debug)]
pub struct ChatSession {
  /// Session handle for sending commands
  pub handle: SessionHandle,
}

impl ChatSession {
  fn new_session(
    runtime: Arc<Runtime>,
    system_prompt: String,
    session_store: Arc<SessionStore>,
    wire_publisher: WirePublisher,
  ) -> Result<(Self, Vec<Message>)> {
    let mut meta = SessionMeta::new(generate_session_id(), &system_prompt);
    meta.yolo = runtime.yolo();
    session_store.create(&meta)?;
    let session =
      Self::create_with_store(runtime, system_prompt, session_store, meta, wire_publisher)?;
    Ok((session, Vec::new()))
  }

  fn resume(
    id: String,
    runtime: Arc<Runtime>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    messages: Vec<Message>,
    wire_publisher: WirePublisher,
  ) -> Result<(Self, Vec<Message>)> {
    let provider = Self::create_provider(&runtime)?;

    let session = Self::start_with_messages(
      id,
      provider,
      messages.clone(),
      session_store,
      meta,
      runtime,
      wire_publisher,
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
    wire_publisher: WirePublisher,
  ) -> Result<(Self, Vec<Message>)> {
    match mode {
      SessionMode::New => Self::new_session(runtime, system_prompt, session_store, wire_publisher),
      SessionMode::ResumeById(id) => {
        let (meta, messages) = session_store.load(&id)?;
        Self::resume(id, runtime, session_store, meta, messages, wire_publisher)
      }
      SessionMode::ResumeLatest => match session_store.latest_id()? {
        Some(id) => {
          let (meta, messages) = session_store.load(&id)?;
          Self::resume(id, runtime, session_store, meta, messages, wire_publisher)
        }
        None => Self::new_session(runtime, system_prompt, session_store, wire_publisher),
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
    wire_publisher: WirePublisher,
  ) -> Result<Self> {
    let provider = Self::create_provider(&runtime)?;
    let system_prompt = system_prompt.into();
    let messages = vec![Message::system(system_prompt.clone())];

    let session = Self::start_with_messages(
      meta.id.clone(),
      provider,
      messages,
      session_store,
      meta,
      runtime,
      wire_publisher,
    );
    info!(
      "ChatSession {} created from config with store",
      session.handle.id
    );
    Ok(session)
  }

  /// Internal: start session with given messages and persistence
  fn start_with_messages(
    id: String,
    provider: Box<dyn LLMProvider>,
    messages: Vec<Message>,
    session_store: Arc<SessionStore>,
    meta: SessionMeta,
    runtime: Arc<Runtime>,
    wire_publisher: WirePublisher,
  ) -> Self {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

    let handle = SessionHandle {
      id: id.clone(),
      cmd_tx,
    };

    let actor = SessionActor::new(
      id,
      provider,
      messages,
      wire_publisher,
      cmd_rx,
      session_store,
      meta,
      runtime,
    );
    tokio::spawn(actor.run());

    Self { handle }
  }

  /// Shutdown the session
  #[allow(dead_code)]
  pub fn shutdown(&self) {
    self.handle.shutdown();
  }
}
