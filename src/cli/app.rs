use std::path::Path;
use std::sync::Arc;

use crossterm::event::KeyEvent;
use log::{error, info};
use ratatui::Frame;

use crate::cli::Args;
use crate::cli::runtime::Runtime;
use crate::error::Result;
use crate::hooks::wire::WireBusHookDispatcher;
use crate::hooks::{HookEventType, events as hook_events, wire::WireHookResponse};
use crate::llm::{ChatSession, Question};
use crate::session::{SessionMode, SessionStore};
use crate::tui::FrameRequester;
use crate::view::chat::{
  ChatMessage, StreamingChunk, SystemMessageLevel, ToolCallStatus, llm_messages_to_chat_history,
};
use crate::view::{ChatView, View};
use crate::wire::{WireBus, WireMessage};

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
  /// Pending structured questions awaiting user answers
  pub(crate) pending_questions: Option<PendingQuestions>,
  /// Whether plan mode is active
  pub(crate) plan_mode: bool,
  /// Pending pager output to display (set by TaskBrowserView, consumed by run_app)
  pub(crate) pending_pager_output: Option<String>,
  /// Channel used to respond to client-side (wire) hook requests.
  /// When a `WireMessage::HookRequest` is received, the UI sends an allow/block
  /// response via this sender.
  pub(crate) hook_response_tx: Option<tokio::sync::mpsc::UnboundedSender<WireHookResponse>>,
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
  /// Optional diff preview for file-modifying tools
  pub diff_preview: Option<String>,
  /// Position of this tool in the overall execution list (1-based)
  pub position: usize,
  /// Total number of tools in the current execution batch
  pub total: usize,
}

/// Pending structured questions from AskUserQuestion
#[derive(Debug, Clone)]
pub struct PendingQuestions {
  /// Tool call ID
  pub tool_call_id: String,
  /// Questions to present
  pub questions: Vec<Question>,
  /// Index of the currently focused question
  pub current_question_idx: usize,
  /// Selected option indices for each question answered so far
  pub answers: Vec<Vec<usize>>,
  /// Currently selected option index within the focused question
  pub selected_option_idx: usize,
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
      pending_questions: None,
      plan_mode: false,
      pending_pager_output: None,
      hook_response_tx: None,
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
  /// Runtime data loaded at startup (read-only application context)
  #[allow(dead_code)]
  pub(crate) runtime: Arc<Runtime>,

  /// Chat session for LLM communication (initialized when first chat starts)
  pub(crate) chat_session: Option<ChatSession>,
  /// Wire subscriber for receiving events from the session actor
  wire_subscriber: tokio::sync::broadcast::Receiver<WireMessage>,
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
  pub async fn new(data_dir: &Path, args: &Args, runtime: Arc<Runtime>) -> Result<Self> {
    let mut data = AppData::new();

    let system_prompt = runtime.render_system_prompt();

    let mode = if let Some(id) = &args.session {
      SessionMode::ResumeById(id.clone())
    } else if args.r#continue {
      SessionMode::ResumeLatest
    } else {
      SessionMode::New
    };

    let session_store = Arc::new(SessionStore::new(data_dir));

    // Create wire bus for decoupling session actor from UI
    let wire_bus = WireBus::new(WireBus::DEFAULT_CAPACITY);
    let wire_publisher = wire_bus.publisher();
    let wire_subscriber = wire_bus.subscriber();

    // Wire up the client-side hook dispatcher. The UI will auto-respond allow
    // to hook requests for now; a future enhancement can prompt the user.
    let (wire_hook_dispatcher, hook_response_tx) =
      WireBusHookDispatcher::new(wire_publisher.clone());
    runtime
      .hook_engine()
      .set_dispatcher(Arc::new(wire_hook_dispatcher));
    data.hook_response_tx = Some(hook_response_tx);

    let (chat_session, messages) = ChatSession::create_or_resume(
      runtime.clone(),
      system_prompt,
      session_store,
      mode.clone(),
      wire_publisher,
    )?;

    data.chat_history = llm_messages_to_chat_history(&messages);
    let session_handle = chat_session.handle.clone();

    // Bind background task manager to this session
    runtime.background_manager.bind_session(&session_handle.id);
    runtime
      .notification_manager
      .bind_session(&session_handle.id);

    // Recover stale background tasks from previous sessions
    let recovered = runtime.background_manager.recover();
    if recovered > 0 {
      data
        .chat_history
        .push(crate::view::chat::messages::ChatMessage::System {
          content: format!(
            "Recovered {} background task(s) from a previous session.",
            recovered
          ),
          level: crate::view::chat::messages::SystemMessageLevel::Info,
        });
    }

    // Publish notifications for terminal tasks discovered during recovery
    runtime.publish_task_notifications();

    // Trigger SessionStart hook (awaited; result does not block UI).
    let session_source = if matches!(mode, SessionMode::New) {
      "startup"
    } else {
      "resume"
    };
    let _ = runtime
      .hook_engine()
      .trigger(
        HookEventType::SessionStart,
        session_source,
        hook_events::session_start(&session_handle.id, &runtime.args.work_dir, session_source),
      )
      .await;

    // Create ChatView directly
    let chat_view = ChatView::new(&data, session_handle, runtime.clone());

    Ok(Self {
      data,
      view: Box::new(chat_view),
      frame_requester: None,
      runtime,
      chat_session: Some(chat_session),
      wire_subscriber,
      current_chunks: Arc::new(Vec::new()),
    })
  }

  pub fn should_exit(&self) -> bool {
    self.data.should_exit
  }

  /// Take any pending pager output (for /task browser).
  pub fn take_pager_output(&mut self) -> Option<String> {
    self.data.pending_pager_output.take()
  }

  /// Kill all active background tasks on exit.
  pub fn cleanup_background_tasks(&self) {
    let killed = self
      .runtime
      .background_manager
      .kill_all_active("CLI session ended");
    if !killed.is_empty() {
      info!("Killed {} background tasks on exit", killed.len());
    }
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

    let session_id = self.chat_session.as_ref().map(|s| s.handle.id.clone());
    loop {
      match self.wire_subscriber.try_recv() {
        Ok(msg) => {
          updated = true;
          self.handle_wire_message(msg, session_id.as_deref());
        }
        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
          log::warn!("Wire bus lagged, dropped {} messages", n);
          continue;
        }
        Err(_) => break,
      }
    }

    // Deliver pending shell notifications as system messages.
    let shell_delivered = self
      .runtime
      .notification_manager()
      .deliver_pending("shell", 8, |view| {
        self
          .data
          .chat_history
          .push(crate::view::chat::messages::ChatMessage::System {
            content: format!("{}\n\n{}", view.event.title, view.event.body),
            level: match view.event.severity {
              crate::notification::NotificationSeverity::Error => {
                crate::view::chat::messages::SystemMessageLevel::Error
              }
              crate::notification::NotificationSeverity::Warning => {
                crate::view::chat::messages::SystemMessageLevel::Warning
              }
              _ => crate::view::chat::messages::SystemMessageLevel::Info,
            },
          });
        Ok::<(), std::convert::Infallible>(())
      });
    if !shell_delivered.is_empty() {
      updated = true;
    }

    // Wire sink notifications are destined for ACP/web clients; in the TUI
    // mode we simply claim and ack them so they do not remain pending forever.
    let wire_delivered = self
      .runtime
      .notification_manager()
      .deliver_pending("wire", 8, |_view| Ok::<(), std::convert::Infallible>(()));
    if !wire_delivered.is_empty() {
      updated = true;
    }

    if updated && let Some(ref fr) = self.frame_requester {
      fr.schedule_frame();
    }

    updated
  }

  fn handle_wire_message(&mut self, msg: WireMessage, session_id: Option<&str>) {
    match msg {
      WireMessage::ContentChunk { text } => self.on_content_chunk(text),
      WireMessage::ThinkingChunk { text } => self.on_thinking_chunk(text),
      WireMessage::ToolCallBegin {
        id,
        name,
        arguments,
      } => {
        self.on_tool_call_received(id, name, arguments);
      }
      WireMessage::ToolCallEnd { name, output, .. } => {
        self.on_tool_call_completed(name, output);
      }
      WireMessage::TurnEnd => self.on_stream_completed(),
      WireMessage::ApprovalRequest {
        id,
        name,
        diff_preview,
        position,
        total,
      } => {
        self.on_approval_needed(id, name, String::new(), diff_preview, position, total);
      }
      WireMessage::QuestionsAsked {
        tool_call_id,
        questions,
      } => {
        self.on_questions_asked(tool_call_id, questions);
      }
      WireMessage::Error { ref message } => {
        self.on_session_error(message.clone());
        if message == "Session shutdown"
          && let Some(id) = session_id
        {
          self.on_session_shutdown(id);
        }
      }
      WireMessage::Usage {
        total_tokens,
        prompt_tokens,
        completion_tokens,
      } => self.on_usage(total_tokens, prompt_tokens, completion_tokens),
      WireMessage::CompactionWarning {
        current_tokens,
        threshold,
        max_context_size,
      } => self.on_compaction_needed(current_tokens, threshold, max_context_size),
      WireMessage::CompactionCompleted {
        before,
        after,
        tokens,
      } => {
        self.on_compaction_completed(before, after, tokens);
      }
      WireMessage::TurnBegin => {}
      WireMessage::PlanModeChanged { active } => {
        self.data.plan_mode = active;
        info!("App: Plan mode changed to {}", active);
      }
      WireMessage::PlanDisplay { content, file_path } => {
        self.data.chat_history.push(ChatMessage::System {
          content: format!("## Plan\n\n{}\n\n_Plan file: {}_", content, file_path),
          level: SystemMessageLevel::Info,
        });
      }
      WireMessage::HookRequest { id, .. } => {
        // Auto-respond allow for now. A future UI can prompt the user.
        if let Some(ref tx) = self.data.hook_response_tx {
          let _ = tx.send(WireHookResponse {
            id,
            action: "allow".to_string(),
            reason: String::new(),
          });
        }
      }
      WireMessage::HookResponse { .. } => {
        // Responses are consumed by the wire hook dispatcher task.
      }
    }
  }

  fn on_content_chunk(&mut self, chunk: String) {
    let preview: String = chunk.chars().take(100).collect();
    log::debug!(
      "App: Received ContentChunk, len={}, content={}",
      chunk.len(),
      preview
    );
    Arc::make_mut(&mut self.current_chunks).push(StreamingChunk::Normal(chunk));
    self.data.streaming_response = self.current_chunks.clone();
  }

  fn on_thinking_chunk(&mut self, chunk: String) {
    let preview: String = chunk.chars().take(100).collect();
    log::info!(
      "App: Received ThinkingChunk, len={}, content={}",
      chunk.len(),
      preview
    );
    Arc::make_mut(&mut self.current_chunks).push(StreamingChunk::Thinking(chunk));
    self.data.streaming_response = self.current_chunks.clone();
  }

  fn on_tool_call_received(&mut self, id: String, name: String, arguments: String) {
    log::info!(
      "App: Tool call received: id={}, name={}, args={}",
      id,
      name,
      arguments
    );
    Arc::make_mut(&mut self.current_chunks).push(StreamingChunk::ToolCall {
      name: name.clone(),
      arguments,
      status: ToolCallStatus::Running,
    });
    self.data.streaming_response = self.current_chunks.clone();
  }

  fn on_tool_call_completed(&mut self, name: String, output: String) {
    log::info!(
      "App: Tool call completed: name={}, output_len={}",
      name,
      output.len()
    );
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
    if let Some(args) = tool_args {
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

  fn on_stream_completed(&mut self) {
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
    if !normal_content.is_empty() || !thinking_content.is_empty() {
      self.data.chat_history.push(ChatMessage::Assistant {
        content: normal_content,
        thinking_content: if thinking_content.is_empty() {
          None
        } else {
          Some(thinking_content)
        },
      });
    }
    self.data.streaming_response = Arc::new(Vec::new());
    self.current_chunks = Arc::new(Vec::new());
  }

  #[allow(dead_code)]
  fn on_stream_interrupted(&mut self) {
    self.current_chunks = Arc::new(Vec::new());
    self.data.streaming_response = Arc::new(Vec::new());
  }

  fn on_approval_needed(
    &mut self,
    id: String,
    name: String,
    _arguments: String,
    diff_preview: Option<String>,
    position: usize,
    total: usize,
  ) {
    self.data.pending_approval = Some(PendingApproval {
      tool_call_id: id,
      name,
      diff_preview,
      position,
      total,
    });
  }

  fn on_session_error(&mut self, err: String) {
    error!("LLM stream error: {}", err);
    self.current_chunks = Arc::new(Vec::new());
    self.data.streaming_response = Arc::new(Vec::new());
    self.data.chat_history.push(ChatMessage::System {
      content: err,
      level: SystemMessageLevel::Error,
    });
  }

  fn on_session_shutdown(&mut self, session_id: &str) {
    info!("ChatSession {} shutdown", session_id);
  }

  fn on_usage(&mut self, total_tokens: u32, prompt_tokens: u32, completion_tokens: u32) {
    log::info!(
      "App: Received precise token usage - total={}, prompt={}, completion={}",
      total_tokens,
      prompt_tokens,
      completion_tokens
    );
    self.data.precise_token_count = Some(total_tokens);
  }

  fn on_compaction_needed(
    &mut self,
    current_tokens: usize,
    threshold: usize,
    max_context_size: usize,
  ) {
    log::info!(
      "App: Compaction needed - {} tokens (threshold: {}, max: {})",
      current_tokens,
      threshold,
      max_context_size
    );
    self.data.compaction_warning = Some(CompactionWarning {
      current_tokens,
      threshold,
      max_context_size,
    });
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

  fn on_questions_asked(&mut self, tool_call_id: String, questions: Vec<Question>) {
    log::info!(
      "App: Questions asked - {} questions from tool_call_id={}",
      questions.len(),
      tool_call_id
    );
    let selected_option_idx = questions
      .first()
      .and_then(|q| q.default.first().copied())
      .unwrap_or(0);
    let answers: Vec<Vec<usize>> = questions.iter().map(|q| q.default.clone()).collect();
    self.data.pending_questions = Some(PendingQuestions {
      tool_call_id,
      questions,
      current_question_idx: 0,
      answers,
      selected_option_idx,
    });
  }

  fn on_compaction_completed(
    &mut self,
    message_count_before: usize,
    message_count_after: usize,
    new_token_count: usize,
  ) {
    log::info!(
      "App: Compaction completed - {} -> {} messages, ~{} tokens",
      message_count_before,
      message_count_after,
      new_token_count
    );
    self.data.compaction_warning = None;
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
