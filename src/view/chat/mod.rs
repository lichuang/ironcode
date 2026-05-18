use std::env;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  text::{Line, Span, Text},
  widgets::{Paragraph, Wrap},
};

use crate::cli::AppData;
use crate::cli::runtime::Runtime;
use crate::history::InputHistoryManager;
use crate::llm::SessionHandle;
use crate::tui::{FrameRequester, TARGET_FRAME_INTERVAL};
use crate::utils::{
  HIGHLIGHT, MOON_FRAMES, PRIMARY, SPINNER_FRAMES, char_display_width, string_display_width,
};
use crate::view::chat::input::InputComponent;
use crate::view::diff::diff_render_height;
use crate::view::{STATUS_BAR_HEIGHT, StatusBarInfo, View, render_status_bar};

pub mod approval;
pub mod input;
pub mod messages;
pub mod questions;

pub use messages::{
  ChatMessage, StreamingChunk, SystemMessageLevel, ToolCallStatus, llm_messages_to_chat_history,
};
/// Error when creating ChatView without a valid session
#[derive(Debug)]
#[allow(dead_code)]
pub struct NoSessionError;

/// Chat display state machine
///
/// State transitions:
/// - User submits message → Animating (show moon animation)
/// - LLM starts responding with thinking content → Thinking (show "Thinking...")
/// - LLM starts responding with normal content → Streaming (show streaming content)
/// - Response completed → WaitingInput (show spinner waiting for input)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatDisplayState {
  /// Waiting for LLM to start responding: show moon animation
  Animating,
  /// LLM is streaming thinking/reasoning content: show "Thinking..."
  Thinking,
  /// LLM is streaming response: show streaming content
  Streaming,
  /// Waiting for user input: show bottom spinner
  WaitingInput,
}

/// Chat view state
pub struct ChatView {
  /// Input component managing text, cursor, and history
  pub input: InputComponent,
  /// Prompt string (username@directory)
  pub prompt: String,
  /// Frame requester for scheduling animations
  frame_requester: Option<FrameRequester>,
  /// Animation state
  animation_enabled: bool,
  /// Last time the spinner was updated
  last_spinner_update: Instant,
  /// Current spinner frame index
  spinner_frame: usize,
  /// Last time the moon was updated
  last_moon_update: Instant,
  /// Current moon frame index
  moon_frame: usize,
  /// Current display state (state machine driven)
  state: ChatDisplayState,
  /// Session handle for sending messages directly to LLM
  session_handle: SessionHandle,
  /// Status bar info (reused across draws)
  status_bar_info: StatusBarInfo,
}

impl ChatView {
  /// Create a new chat view
  ///
  /// Initialize state machine based on AppData state:
  /// - Has streaming response → Streaming state
  /// - Has user messages but waiting for AI response → Animating state
  /// - Otherwise → WaitingInput state
  ///
  /// # Arguments
  /// * `data` - Application data for determining initial state
  /// * `session_handle` - Handle to the chat session (must be valid)
  pub fn new(data: &AppData, session_handle: SessionHandle, runtime: &Runtime) -> Self {
    let prompt = Self::build_prompt();

    // Check if waiting for AI response (last message is from user)
    let waiting_for_ai = data
      .chat_history
      .last()
      .map(|msg| msg.is_user())
      .unwrap_or(false);

    // Determine initial state
    let state = if !data.streaming_response.is_empty() {
      // Check if currently streaming thinking content
      if data.streaming_response.iter().any(|c| c.is_thinking()) {
        ChatDisplayState::Thinking
      } else {
        ChatDisplayState::Streaming
      }
    } else if waiting_for_ai {
      ChatDisplayState::Animating
    } else {
      ChatDisplayState::WaitingInput
    };

    log::debug!("ChatView created with initial state: {:?}", state);

    // Initialize status bar info
    let status_bar_info = StatusBarInfo::from_app_data(data, &session_handle.id, state, runtime);

    Self {
      input: InputComponent::new(InputHistoryManager::with_config(runtime)),
      prompt,
      frame_requester: None,
      animation_enabled: true,
      last_spinner_update: Instant::now(),
      spinner_frame: 0,
      last_moon_update: Instant::now(),
      moon_frame: 0,
      state,
      session_handle,
      status_bar_info,
    }
  }

  #[allow(dead_code)]
  /// Get current state
  pub fn state(&self) -> ChatDisplayState {
    self.state
  }

  /// State transition: enter Animating state
  fn enter_animating(&mut self) {
    let old_state = self.state;
    self.state = ChatDisplayState::Animating;
    log::debug!("State transition: {:?} → {:?}", old_state, self.state);
    // Reset moon animation frame
    self.moon_frame = 0;
    self.last_moon_update = Instant::now();
  }

  /// State transition: enter Thinking state
  fn enter_thinking(&mut self) {
    let old_state = self.state;
    self.state = ChatDisplayState::Thinking;
    log::debug!("State transition: {:?} → {:?}", old_state, self.state);
    // Reset spinner animation for "Thinking..." indicator
    self.spinner_frame = 0;
    self.last_spinner_update = Instant::now();
  }

  /// State transition: enter Streaming state
  fn enter_streaming(&mut self) {
    let old_state = self.state;
    self.state = ChatDisplayState::Streaming;
    log::debug!("State transition: {:?} → {:?}", old_state, self.state);
  }

  /// State transition: enter WaitingInput state
  fn enter_waiting_input(&mut self) {
    let old_state = self.state;
    self.state = ChatDisplayState::WaitingInput;
    log::debug!("State transition: {:?} → {:?}", old_state, self.state);
  }

  /// Build the prompt string (username@current_dir)
  fn build_prompt() -> String {
    let username = env::var("USER")
      .or_else(|_| env::var("USERNAME"))
      .unwrap_or_else(|_| "user".to_string());

    let current_dir = env::current_dir()
      .ok()
      .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
      .unwrap_or_else(|| "~".to_string());

    format!("{}@{}", username, current_dir)
  }

  /// Get the current spinner character based on animation state
  fn current_spinner(&self) -> char {
    if self.animation_enabled {
      SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()]
    } else {
      '>'
    }
  }

  /// Get the full prompt with spinner (for width calculation)
  fn full_prompt(&self) -> String {
    format!("{} {} ", self.prompt, self.current_spinner())
  }

  /// Handle character input
  pub fn insert_char(&mut self, c: char) {
    self.input.insert_char(c);
  }

  /// Save current input to history
  fn save_to_history(&mut self) {
    self.input.save_to_history();
  }

  /// Submit the current input as a message
  ///
  /// State transition: WaitingInput → Animating
  /// Sends message directly to LLM via SessionHandle
  pub fn submit_message(&mut self, data: &mut AppData) {
    if !self.input.is_empty() {
      // Save input to history before submitting
      self.save_to_history();

      let message = self.input.take_text();
      // Add user message to chat history
      data.chat_history.push(ChatMessage::User {
        content: message.clone(),
      });
      self.input.move_cursor_home();

      // Send message directly to LLM via SessionHandle
      let preview: String = message.chars().take(50).collect();
      log::debug!("Sending message to LLM: {}", preview);
      self.session_handle.send_message(message);

      // Enter Animating state (show moon animation)
      log::debug!("State will transition: WaitingInput → Animating");
      self.enter_animating();
    }
  }

  /// Get the current moon character
  fn current_moon(&self) -> char {
    MOON_FRAMES[self.moon_frame % MOON_FRAMES.len()]
  }

  /// Calculate the number of lines needed to display prompt + text with given width
  fn calculate_input_line_count(&self, text: &str, available_width: u16) -> usize {
    let prompt_width = string_display_width(&self.full_prompt());
    messages::calculate_line_count_with_prefix(text, prompt_width, available_width)
  }

  /// Find cursor position (line number and column within that line)
  fn find_cursor_position(&self, available_width: u16) -> (usize, usize) {
    if available_width == 0 {
      return (0, 0);
    }
    let available = available_width as usize;
    let prompt_width = string_display_width(&self.full_prompt());

    let mut line = 0;
    let mut col = prompt_width; // Start after prompt
    let mut current_line_width = prompt_width;

    for (idx, c) in self.input.text().chars().enumerate() {
      if idx >= self.input.cursor() {
        break;
      }

      let char_width = char_display_width(c);

      if c == '\n' {
        line += 1;
        col = 0;
        current_line_width = 0;
      } else if current_line_width + char_width > available {
        line += 1;
        col = char_width;
        current_line_width = char_width;
      } else {
        col = current_line_width + char_width;
        current_line_width += char_width;
      }
    }

    (line, col)
  }

  /// Render an input line (prompt + arrow/indicator + input) with wrapping
  ///
  /// # Arguments
  /// * `with_arrow` - If true, show ">" before input (user message style)
  ///   If false, show spinner (waiting for input style)
  fn render_input_line(&self, f: &mut Frame, area: Rect, input: &str, with_arrow: bool) {
    let text = if with_arrow {
      // User message style: prompt > input
      Text::from(vec![Line::from(vec![
        Span::styled(&self.prompt, *PRIMARY),
        Span::raw(" "),
        Span::styled(">", *HIGHLIGHT),
        Span::raw(" "),
        Span::raw(input),
      ])])
    } else {
      // Waiting for input style: prompt spinner
      let spinner = self.current_spinner();
      Text::from(vec![Line::from(vec![
        Span::styled(&self.prompt, *PRIMARY),
        Span::raw(" "),
        Span::styled(spinner.to_string(), *PRIMARY),
      ])])
    };

    let widget = Paragraph::new(text).wrap(Wrap { trim: false });
    f.render_widget(widget, area);
  }

  /// Update status bar info with current state
  ///
  /// This method updates the mutable fields of status_bar_info.
  /// Static fields (session_id, model_name) are set once during construction.
  fn update_status_bar_info(&mut self, data: &AppData) {
    use crate::utils::token_counter::estimate_chat_messages_tokens;
    use crate::view::status_bar::calculate_compaction_warning_level;

    self.status_bar_info.state = self.state;
    // Use precise token count from API if available, otherwise estimate
    self.status_bar_info.token_count = data
      .precise_token_count
      .map(|t| t as usize)
      .unwrap_or_else(|| estimate_chat_messages_tokens(&data.chat_history));

    // Update compaction warning level from AppData
    self.status_bar_info.compaction_warning =
      calculate_compaction_warning_level(&data.compaction_warning, true);

    // Update plan mode indicator
    self.status_bar_info.plan_mode = data.plan_mode;
  }
}

impl View for ChatView {
  fn handle_key(&mut self, data: &mut AppData, key: KeyEvent) -> Option<Box<dyn View>> {
    // Handle pending tool call approval first
    if let Some(ref approval) = data.pending_approval {
      match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
          self
            .session_handle
            .approve_tool_call(&approval.tool_call_id, true, false);
          data.pending_approval = None;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
          self
            .session_handle
            .approve_tool_call(&approval.tool_call_id, false, false);
          data.pending_approval = None;
        }
        KeyCode::Char('a') => {
          self.session_handle.enable_session_yolo();
          self
            .session_handle
            .approve_tool_call(&approval.tool_call_id, true, false);
          data.pending_approval = None;
        }
        KeyCode::Char('q') => {
          self
            .session_handle
            .approve_tool_call(&approval.tool_call_id, false, true);
          data.pending_approval = None;
        }
        _ => {}
      }
      return None;
    }

    // Handle pending structured questions
    if let Some(ref mut questions) = data.pending_questions {
      // Check if current question is a confirmation dialog
      let is_confirmation = questions
        .questions
        .get(questions.current_question_idx)
        .map(|q| q.confirmation)
        .unwrap_or(false);

      match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
          self
            .session_handle
            .answer_questions(&questions.tool_call_id, Vec::new(), true);
          data.pending_questions = None;
        }
        KeyCode::Char('y') if is_confirmation => {
          let q_idx = questions.current_question_idx;
          while questions.answers.len() <= q_idx {
            questions.answers.push(Vec::new());
          }
          questions.answers[q_idx] = vec![0]; // Yes = index 0
          questions.current_question_idx += 1;
          questions.selected_option_idx = 0;
          if questions.current_question_idx >= questions.questions.len() {
            let answers = std::mem::take(&mut questions.answers);
            let tool_call_id = questions.tool_call_id.clone();
            self
              .session_handle
              .answer_questions(tool_call_id, answers, false);
            data.pending_questions = None;
          }
        }
        KeyCode::Char('n') if is_confirmation => {
          let q_idx = questions.current_question_idx;
          while questions.answers.len() <= q_idx {
            questions.answers.push(Vec::new());
          }
          questions.answers[q_idx] = vec![1]; // No = index 1
          questions.current_question_idx += 1;
          questions.selected_option_idx = 0;
          if questions.current_question_idx >= questions.questions.len() {
            let answers = std::mem::take(&mut questions.answers);
            let tool_call_id = questions.tool_call_id.clone();
            self
              .session_handle
              .answer_questions(tool_call_id, answers, false);
            data.pending_questions = None;
          }
        }
        KeyCode::Up => {
          if !is_confirmation && questions.selected_option_idx > 0 {
            questions.selected_option_idx -= 1;
          }
        }
        KeyCode::Down => {
          if !is_confirmation
            && let Some(q) = questions.questions.get(questions.current_question_idx)
            && questions.selected_option_idx + 1 < q.options.len()
          {
            questions.selected_option_idx += 1;
          }
        }
        KeyCode::Char(' ') => {
          if let Some(q) = questions.questions.get(questions.current_question_idx)
            && q.multi_select
          {
            let q_idx = questions.current_question_idx;
            while questions.answers.len() <= q_idx {
              questions.answers.push(Vec::new());
            }
            let selected = questions.selected_option_idx;
            let ans = &mut questions.answers[q_idx];
            if let Some(pos) = ans.iter().position(|&x| x == selected) {
              ans.remove(pos);
            } else {
              ans.push(selected);
            }
          }
        }
        KeyCode::Enter => {
          if let Some(q) = questions.questions.get(questions.current_question_idx) {
            let q_idx = questions.current_question_idx;
            while questions.answers.len() <= q_idx {
              questions.answers.push(Vec::new());
            }
            // Check required validation
            if q.required && questions.answers[q_idx].is_empty() {
              // Don't advance, keep the question open
              // (UI will show it's still pending)
              return None;
            }
            if !q.multi_select {
              questions.answers[q_idx] = vec![questions.selected_option_idx];
            }
            questions.current_question_idx += 1;
            questions.selected_option_idx = questions
              .questions
              .get(questions.current_question_idx)
              .and_then(|q| q.default.first().copied())
              .unwrap_or(0);
            if questions.current_question_idx >= questions.questions.len() {
              let answers = std::mem::take(&mut questions.answers);
              let tool_call_id = questions.tool_call_id.clone();
              self
                .session_handle
                .answer_questions(tool_call_id, answers, false);
              data.pending_questions = None;
            }
          }
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
          let digit = (c as usize).saturating_sub('1' as usize);
          if let Some(q) = questions.questions.get(questions.current_question_idx)
            && digit < q.options.len()
          {
            questions.selected_option_idx = digit;
            let q_idx = questions.current_question_idx;
            while questions.answers.len() <= q_idx {
              questions.answers.push(Vec::new());
            }
            if q.multi_select {
              let ans = &mut questions.answers[q_idx];
              if let Some(pos) = ans.iter().position(|&x| x == digit) {
                ans.remove(pos);
              } else {
                ans.push(digit);
              }
            } else {
              questions.answers[q_idx] = vec![digit];
              questions.current_question_idx += 1;
              questions.selected_option_idx = questions
                .questions
                .get(questions.current_question_idx)
                .and_then(|q| q.default.first().copied())
                .unwrap_or(0);
              if questions.current_question_idx >= questions.questions.len() {
                let answers = std::mem::take(&mut questions.answers);
                let tool_call_id = questions.tool_call_id.clone();
                self
                  .session_handle
                  .answer_questions(tool_call_id, answers, false);
                data.pending_questions = None;
              }
            }
          }
        }
        _ => {}
      }
      return None;
    }

    match key.code {
      KeyCode::Esc => {
        // Exit the application
        data.should_exit = true;
      }
      KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
        data.should_exit = true;
      }
      KeyCode::Enter => {
        // Shift+Enter or Alt+Enter to insert newline, Enter alone to submit
        if key.modifiers.contains(KeyModifiers::SHIFT) || key.modifiers.contains(KeyModifiers::ALT)
        {
          self.insert_char('\n');
        } else {
          self.submit_message(data);
        }
      }
      KeyCode::Up => {
        self.input.navigate_up();
      }
      KeyCode::Down => {
        self.input.navigate_down();
      }
      KeyCode::Backspace => {
        self.input.backspace();
      }
      KeyCode::Delete => {
        self.input.delete();
      }
      KeyCode::Left => {
        self.input.move_cursor_left();
      }
      KeyCode::Right => {
        self.input.move_cursor_right();
      }
      KeyCode::Home => {
        self.input.move_cursor_home();
      }
      KeyCode::End => {
        self.input.move_cursor_end();
      }
      KeyCode::Char(c) => {
        self.input.insert_char(c);
      }
      _ => {}
    }
    None
  }

  fn on_frame(&mut self, frame_requester: &FrameRequester, data: &AppData) {
    if !self.animation_enabled {
      return;
    }

    // Check if last message is from user (waiting for AI response)
    let last_is_user = data
      .chat_history
      .last()
      .map(|msg| msg.is_user())
      .unwrap_or(false);

    // State machine transition logic
    match self.state {
      ChatDisplayState::Animating => {
        // Check if LLM started responding
        if !data.streaming_response.is_empty() {
          // Animating → Thinking: first chunk is thinking content
          // Animating → Streaming: first chunk is normal content
          if data.streaming_response.iter().any(|c| c.is_thinking()) {
            self.enter_thinking();
          } else {
            self.enter_streaming();
          }
        }
        // Animating → WaitingInput: AI response added to history
        else if data.streaming_response.is_empty() && !last_is_user {
          self.enter_waiting_input();
        }
      }
      ChatDisplayState::Thinking => {
        // Thinking → Streaming: received normal content (thinking completed)
        // or stream ended but we have thinking content to display
        if data.streaming_response.iter().any(|c| !c.is_thinking()) {
          self.enter_streaming();
        }
        // Thinking → WaitingInput: streaming response completed and empty
        else if data.streaming_response.is_empty() {
          self.enter_waiting_input();
        }
        // Note: if stream ends with only thinking content, it will be handled
        // when streaming_response becomes empty (moved to history)
      }
      ChatDisplayState::Streaming => {
        // Streaming → WaitingInput: streaming response completed
        if data.streaming_response.is_empty() {
          self.enter_waiting_input();
        }
      }
      ChatDisplayState::WaitingInput => {
        // WaitingInput → Animating: user submits message (handled in submit_message)
        // Additional logic: check if need to enter Animating (e.g., when switching from HomeView)
        if last_is_user && data.streaming_response.is_empty() {
          self.enter_animating();
        }
      }
    }

    let now = Instant::now();

    // Update spinner animation (for waiting user input prompt)
    let elapsed = now.duration_since(self.last_spinner_update);
    // Update spinner frame every 200ms (relaxed rotation)
    const SPINNER_INTERVAL: Duration = Duration::from_millis(200);
    if elapsed >= SPINNER_INTERVAL {
      self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
      self.last_spinner_update = now;
    }

    // Update moon animation (only in Animating state)
    if self.state == ChatDisplayState::Animating {
      let moon_elapsed = now.duration_since(self.last_moon_update);
      // Moon cycles slower than spinner - one phase every 300ms
      const MOON_INTERVAL: Duration = Duration::from_millis(300);
      if moon_elapsed >= MOON_INTERVAL {
        self.moon_frame = (self.moon_frame + 1) % MOON_FRAMES.len();
        self.last_moon_update = now;
      }
    }

    // Update thinking spinner animation (only in Thinking state)
    if self.state == ChatDisplayState::Thinking {
      let spinner_elapsed = now.duration_since(self.last_spinner_update);
      const THINKING_SPINNER_INTERVAL: Duration = Duration::from_millis(200);
      if spinner_elapsed >= THINKING_SPINNER_INTERVAL {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
        self.last_spinner_update = now;
      }
    }

    // Schedule next frame for smooth animation
    frame_requester.schedule_frame_in(TARGET_FRAME_INTERVAL);
  }

  fn draw(&mut self, f: &mut Frame, data: &AppData) {
    let area = f.area();

    // Split area into main content (top) and status bar (bottom)
    // Status bar has 2 lines: 1 for separator line, 1 for content
    let main_chunks = Layout::default()
      .direction(Direction::Vertical)
      .constraints([Constraint::Min(0), Constraint::Length(STATUS_BAR_HEIGHT)])
      .split(area);

    let main_area = main_chunks[0];
    let status_area = main_chunks[1];
    let available_width = main_area.width;

    // Calculate input height (dynamic based on content, including prompt width)
    // No height limit - content will wrap naturally based on available width
    let input_height = self
      .calculate_input_line_count(self.input.text(), available_width)
      .max(1);

    // Calculate layout:
    // For each message: prompt line + box
    // Moon animation after last message (if enabled)
    // For current input: dynamic lines (if not showing moon animation)
    let mut constraints: Vec<Constraint> = Vec::new();

    // History messages: render based on message type
    // Box has borders on both sides, so inner width is available_width - 2
    let box_inner_width = available_width.saturating_sub(2);
    for message in &data.chat_history {
      match message {
        ChatMessage::User { content } => {
          // User message: prompt line + box
          let prompt_lines = self.calculate_input_line_count(content, available_width);
          constraints.push(Constraint::Length(prompt_lines as u16));
          let box_content_lines = messages::calculate_line_count(content, box_inner_width);
          let box_height = box_content_lines + 2; // +2 for top and bottom borders
          constraints.push(Constraint::Length(box_height as u16));
        }
        ChatMessage::Assistant {
          content,
          thinking_content,
        } => {
          // AI message: thinking content (if any) + main content
          if let Some(thinking) = thinking_content {
            let thinking_lines = messages::calculate_line_count(thinking, available_width);
            constraints.push(Constraint::Length(thinking_lines as u16));
          }
          let content_lines = messages::calculate_line_count(content, available_width);
          constraints.push(Constraint::Length(content_lines as u16));
        }
        ChatMessage::ToolCall {
          name,
          arguments,
          output,
        } => {
          // Tool call: showing tool name, arguments, and optional diff panel
          let tool_text = format!("• Used {}({})", name, arguments);
          let mut lines = messages::calculate_line_count(&tool_text, available_width);
          if let Some(out) = output {
            lines += diff_render_height(out);
          }
          constraints.push(Constraint::Length(lines.max(1) as u16));
        }
        ChatMessage::System { content, .. } => {
          // System message: single line notification
          let lines = messages::calculate_line_count(content, available_width);
          constraints.push(Constraint::Length(lines.max(1) as u16));
        }
      }
    }
    // Moon animation row (only shown in Animating state)
    if self.state == ChatDisplayState::Animating {
      constraints.push(Constraint::Length(1));
    }

    // Thinking indicator row (only shown in Thinking state)
    if self.state == ChatDisplayState::Thinking {
      constraints.push(Constraint::Length(1));
    }

    // Streaming response chunks (if any) - only in Streaming state
    // In Thinking state, content is accumulated but not displayed yet
    if self.state == ChatDisplayState::Streaming && !data.streaming_response.is_empty() {
      // Calculate total height for all chunks
      let total_lines: usize = data
        .streaming_response
        .iter()
        .map(|c| messages::calculate_line_count(c.content(), available_width))
        .sum();
      if total_lines > 0 {
        constraints.push(Constraint::Length(total_lines as u16));
      }
    }

    // Approval prompt (if pending)
    let approval_height = if let Some(ref approval) = data.pending_approval {
      approval::ApprovalPanel::height(approval)
    } else {
      0
    };
    if approval_height > 0 {
      constraints.push(Constraint::Length(approval_height));
    }

    // Structured questions panel (if pending)
    let question_height = if let Some(ref pq) = data.pending_questions {
      questions::QuestionsPanel::height(pq, available_width)
    } else {
      0
    };
    if question_height > 0 {
      constraints.push(Constraint::Length(question_height));
    }

    // Waiting for user input: spinner line (1 line height)
    // Only shown in WaitingInput state
    if self.state == ChatDisplayState::WaitingInput {
      constraints.push(Constraint::Length(1));
    }

    // Current input (only if there's actual input text)
    if !self.input.is_empty() {
      constraints.push(Constraint::Length(input_height as u16));
    }

    // Add remaining space
    let prompt_width = string_display_width(&self.full_prompt());
    let mut total_fixed_height: usize = data
      .chat_history
      .iter()
      .map(|msg| match msg {
        ChatMessage::User { content } => {
          messages::calculate_line_count_with_prefix(content, prompt_width, available_width)
            + messages::calculate_line_count(content, box_inner_width)
            + 2
        }
        ChatMessage::Assistant { content, .. } => {
          messages::calculate_line_count(content, available_width)
        }
        ChatMessage::ToolCall {
          name,
          arguments,
          output,
        } => {
          let tool_text = format!("• Used {}({})", name, arguments);
          let output_lines = output.as_ref().map(|o| o.lines().count()).unwrap_or(0);
          messages::calculate_line_count(&tool_text, available_width) + output_lines
        }
        ChatMessage::System { content, .. } => {
          messages::calculate_line_count(content, available_width)
        }
      })
      .sum::<usize>();
    // Add moon animation height if in Animating state
    if self.state == ChatDisplayState::Animating {
      total_fixed_height += 1;
    }
    // Add thinking indicator height if in Thinking state
    // Note: In Thinking state, we only show the indicator, not the actual content
    if self.state == ChatDisplayState::Thinking {
      total_fixed_height += 1;
    }
    // Add streaming response chunks height if present (only in Streaming state)
    // In Thinking state, we don't show the streaming content yet
    if self.state == ChatDisplayState::Streaming && !data.streaming_response.is_empty() {
      let chunks_height: usize = data
        .streaming_response
        .iter()
        .map(|c| messages::calculate_line_count(c.content(), available_width))
        .sum();
      total_fixed_height += chunks_height;
    }
    // Add approval prompt height if pending
    if let Some(ref approval) = data.pending_approval {
      total_fixed_height += approval::ApprovalPanel::height(approval) as usize;
    }
    // Add question panel height if pending
    if let Some(ref pq) = data.pending_questions {
      total_fixed_height += questions::QuestionsPanel::height(pq, available_width) as usize;
    }
    // Add spinner line height if in WaitingInput state
    if self.state == ChatDisplayState::WaitingInput {
      total_fixed_height += 1;
    }
    // Only add input height if there's actual input
    if !self.input.is_empty() {
      total_fixed_height += input_height;
    }

    let available_height = area.height as usize;
    if total_fixed_height < available_height {
      constraints.push(Constraint::Min(0));
    }

    let chunks = Layout::default()
      .direction(Direction::Vertical)
      .constraints(constraints)
      .split(main_area);

    // Render history: user messages and AI responses
    let mut chunk_idx = 0;
    for message in &data.chat_history {
      match message {
        ChatMessage::User { content } => {
          // User message: input line with ">" then box
          if chunk_idx < chunks.len() {
            self.render_input_line(f, chunks[chunk_idx], content, true);
            chunk_idx += 1;
          }
          if chunk_idx < chunks.len() {
            messages::render_message_box(f, chunks[chunk_idx], content);
            chunk_idx += 1;
          }
        }
        ChatMessage::Assistant {
          content,
          thinking_content,
        } => {
          // AI message: thinking content (if any) + main content
          if let Some(thinking) = thinking_content
            && chunk_idx < chunks.len()
          {
            messages::render_thinking_content(f, chunks[chunk_idx], thinking);
            chunk_idx += 1;
          }
          if chunk_idx < chunks.len() {
            messages::render_ai_response(f, chunks[chunk_idx], content);
            chunk_idx += 1;
          }
        }
        ChatMessage::ToolCall {
          name,
          arguments,
          output,
        } => {
          if chunk_idx < chunks.len() {
            messages::render_tool_call(f, chunks[chunk_idx], name, arguments, output.as_deref());
            chunk_idx += 1;
          }
        }
        ChatMessage::System { content, level } => {
          if chunk_idx < chunks.len() {
            messages::render_system_message(f, chunks[chunk_idx], content, *level);
            chunk_idx += 1;
          }
        }
      }
    }

    // Render moon animation if in Animating state
    if self.state == ChatDisplayState::Animating && chunk_idx < chunks.len() {
      messages::render_moon_animation(f, chunks[chunk_idx], self.current_moon());
      chunk_idx += 1;
    }

    // Render thinking indicator if in Thinking state
    if self.state == ChatDisplayState::Thinking && chunk_idx < chunks.len() {
      messages::render_thinking_indicator(f, chunks[chunk_idx], self.current_spinner());
      chunk_idx += 1;
    }

    // Render streaming response chunks if present (only in Streaming state)
    // In Thinking state, we only show the "Thinking..." indicator without content
    if self.state == ChatDisplayState::Streaming && !data.streaming_response.is_empty() {
      // Combine consecutive chunks of the same type for rendering
      // (is_thinking, content) - is_thinking=true for thinking, false for normal
      // For tool calls, we render them separately
      let mut combined: Vec<(bool, String)> = Vec::new();

      for chunk in data.streaming_response.iter() {
        match chunk {
          StreamingChunk::Normal(content) => {
            if let Some((false, last_content)) = combined.last_mut() {
              last_content.push_str(content);
            } else {
              combined.push((false, content.clone()));
            }
          }
          StreamingChunk::Thinking(content) => {
            if let Some((true, last_content)) = combined.last_mut() {
              last_content.push_str(content);
            } else {
              combined.push((true, content.clone()));
            }
          }
          StreamingChunk::ToolCall { .. } => {
            // Tool calls are now added to chat_history when completed,
            // so we don't render them here to avoid duplication
          }
        }
      }

      // Render combined content chunks
      for (is_thinking, content) in combined {
        if chunk_idx < chunks.len() && !content.is_empty() {
          if is_thinking {
            messages::render_thinking_content(f, chunks[chunk_idx], &content);
          } else {
            messages::render_ai_response(f, chunks[chunk_idx], &content);
          }
          chunk_idx += 1;
        }
      }
    }

    // Render questions panel if pending
    if let Some(ref pq) = data.pending_questions
      && chunk_idx < chunks.len()
    {
      questions::QuestionsPanel::render(f, chunks[chunk_idx], pq);
      chunk_idx += 1;
    }

    // Render approval prompt if pending
    if let Some(ref approval) = data.pending_approval
      && chunk_idx < chunks.len()
    {
      approval::ApprovalPanel::render(f, chunks[chunk_idx], approval);
      chunk_idx += 1;
    }

    // Waiting for user input: prompt + spinner (no input box yet)
    // Only shown in WaitingInput state
    if self.state == ChatDisplayState::WaitingInput && chunk_idx < chunks.len() {
      // Show empty input line with spinner
      self.render_input_line(f, chunks[chunk_idx], "", false);
      chunk_idx += 1;
    }

    // Render current input line (only shown in WaitingInput state)
    if self.state == ChatDisplayState::WaitingInput
      && chunk_idx < chunks.len()
      && !self.input.is_empty()
    {
      self.render_input_line(f, chunks[chunk_idx], self.input.text(), true);

      // Set cursor position
      let (cursor_line, cursor_col) = self.find_cursor_position(available_width);
      let cursor_x = chunks[chunk_idx].x + cursor_col as u16;
      let cursor_y = chunks[chunk_idx].y + cursor_line as u16;

      // Ensure cursor is within bounds
      let max_x = chunks[chunk_idx].x + chunks[chunk_idx].width;
      let max_y = chunks[chunk_idx].y + chunks[chunk_idx].height;
      let cursor_x = cursor_x.min(max_x.saturating_sub(1));
      let cursor_y = cursor_y.min(max_y.saturating_sub(1));

      f.set_cursor_position((cursor_x, cursor_y));
    }

    // Update and render status bar
    self.update_status_bar_info(data);
    render_status_bar(f, status_area, &self.status_bar_info);
  }

  fn set_frame_requester(&mut self, frame_requester: FrameRequester) {
    self.frame_requester = Some(frame_requester.clone());
    // Start animation loop immediately
    if self.animation_enabled {
      frame_requester.schedule_frame_in(TARGET_FRAME_INTERVAL);
    }
  }
}

// Note: ChatView cannot implement Default because it requires a SessionHandle.
// Use ChatView::new(data, session_handle) to create an instance.

#[cfg(test)]
mod tests;
