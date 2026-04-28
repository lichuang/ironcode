use std::env;
use std::mem::take;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  symbols::border,
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::cli::AppData;
use crate::cli::app::PendingQuestions;
use crate::config::global_config;
use crate::history::InputHistoryManager;
use crate::llm::SessionHandle;
use crate::llm::types::Message;
use crate::tui::{FrameRequester, TARGET_FRAME_INTERVAL};
use crate::utils::colors::{
  BLUE, CRITICAL, GREEN, HIGHLIGHT as HIGHLIGHT_COLOR, TEXT as TEXT_COLOR, WARNING,
};
use crate::utils::{
  HIGHLIGHT, MOON_FRAMES, PRIMARY, PRIMARY_BORDER, SPINNER_FRAMES, THINKING, char_display_width,
  string_display_width,
};
use crate::view::diff::{
  diff_preview_compact_height, diff_render_height, render_diff_panel, render_diff_preview_compact,
};
use crate::view::{STATUS_BAR_HEIGHT, StatusBarInfo, View, render_status_bar};

/// Error when creating ChatView without a valid session
#[derive(Debug)]
#[allow(dead_code)]
pub struct NoSessionError;

/// Status of a tool call
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
  /// Tool call is being executed
  Running,
  /// Tool call completed successfully
  Completed,
  #[allow(dead_code)]
  /// Tool call failed
  Failed,
}

/// A chunk of streaming content from LLM
#[derive(Debug, Clone)]
pub enum StreamingChunk {
  /// Normal response content
  Normal(String),
  /// Thinking/reasoning content
  Thinking(String),
  /// Tool call indicator
  ToolCall {
    /// Tool name
    name: String,
    /// Tool arguments (JSON)
    arguments: String,
    /// Current status
    status: ToolCallStatus,
  },
}

#[allow(dead_code)]
impl StreamingChunk {
  /// Get the content of the chunk
  pub fn content(&self) -> &str {
    match self {
      StreamingChunk::Normal(s) => s,
      StreamingChunk::Thinking(s) => s,
      StreamingChunk::ToolCall { .. } => "",
    }
  }

  /// Check if this is a thinking chunk
  pub fn is_thinking(&self) -> bool {
    matches!(self, StreamingChunk::Thinking(_))
  }

  #[allow(dead_code)]
  /// Check if this is a tool call chunk
  pub fn is_tool_call(&self) -> bool {
    matches!(self, StreamingChunk::ToolCall { .. })
  }
}

/// A message in the chat history
#[derive(Debug, Clone)]
pub enum ChatMessage {
  /// User message
  User { content: String },
  /// AI assistant response (with optional thinking content)
  Assistant {
    /// The main response content
    content: String,
    /// The thinking/reasoning content (if any)
    thinking_content: Option<String>,
  },
  /// Tool call result
  ToolCall {
    /// Tool name
    name: String,
    /// Tool arguments (JSON)
    arguments: String,
    /// Tool execution output (e.g., diff or success message)
    output: Option<String>,
  },
  /// System notification (e.g., compaction warning)
  System {
    /// The notification content
    content: String,
    /// Notification level (info, warning, error)
    level: SystemMessageLevel,
  },
}

/// Level for system messages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SystemMessageLevel {
  /// Informational message
  Info,
  /// Warning message
  Warning,
  /// Error/Critical message
  Error,
}

/// Convert LLM messages into UI chat history.
///
/// This is used when resuming a persisted session to rebuild the visual chat log.
pub fn llm_messages_to_chat_history(messages: &[Message]) -> Vec<ChatMessage> {
  use crate::llm::types::Role;

  let mut history = Vec::new();
  for msg in messages {
    match msg.role {
      Role::System => {}
      Role::User => {
        history.push(ChatMessage::User {
          content: msg.content.clone(),
        });
      }
      Role::Assistant => {
        let content = msg.content.clone();
        let (thinking_content, content) = if let Some(start) = content.find("<think>")
          && let Some(end) = content.find("</think>")
        {
          let think = content[start + 7..end].to_string();
          let after = content[end + 8..].to_string();
          (Some(think), after)
        } else {
          (None, content)
        };
        if !content.is_empty() || thinking_content.is_some() {
          history.push(ChatMessage::Assistant {
            content,
            thinking_content,
          });
        }
      }
      Role::Tool => {
        // Tool results are not rendered directly in the chat history;
        // the corresponding tool call indicator is added via ToolCallCompleted events.
      }
    }
  }
  history
}

#[allow(dead_code)]
impl ChatMessage {
  /// Get the content of the message
  pub fn content(&self) -> &str {
    match self {
      ChatMessage::User { content } => content,
      ChatMessage::Assistant { content, .. } => content,
      ChatMessage::ToolCall { output, .. } => output.as_deref().unwrap_or(""),
      ChatMessage::System { content, .. } => content,
    }
  }

  #[allow(dead_code)]
  /// Get the thinking content (if any)
  pub fn thinking_content(&self) -> Option<&str> {
    match self {
      ChatMessage::User { .. } => None,
      ChatMessage::Assistant {
        thinking_content, ..
      } => thinking_content.as_deref(),
      ChatMessage::ToolCall { .. } => None,
      ChatMessage::System { .. } => None,
    }
  }

  /// Check if this is a user message
  pub fn is_user(&self) -> bool {
    matches!(self, ChatMessage::User { .. })
  }

  #[allow(dead_code)]
  /// Check if this is an assistant message
  pub fn is_assistant(&self) -> bool {
    matches!(self, ChatMessage::Assistant { .. })
  }

  #[allow(dead_code)]
  /// Check if this is a tool call message
  pub fn is_tool_call(&self) -> bool {
    matches!(self, ChatMessage::ToolCall { .. })
  }

  /// Check if this is a system message
  pub fn is_system(&self) -> bool {
    matches!(self, ChatMessage::System { .. })
  }
}

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
  /// Current input text
  pub input: String,
  /// Cursor position in the input (character index, not byte index)
  pub cursor_position: usize,
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
  /// Input history manager
  history: InputHistoryManager,
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
  pub fn new(data: &AppData, session_handle: SessionHandle) -> Self {
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
    let status_bar_info = StatusBarInfo::from_app_data(data, &session_handle.id, state);

    Self {
      input: String::new(),
      cursor_position: 0,
      prompt,
      frame_requester: None,
      animation_enabled: true,
      last_spinner_update: Instant::now(),
      spinner_frame: 0,
      last_moon_update: Instant::now(),
      moon_frame: 0,
      state,
      session_handle,
      history: InputHistoryManager::with_config(global_config()),
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

  /// Get byte position from character position
  fn char_pos_to_byte_pos(&self, char_pos: usize) -> usize {
    self
      .input
      .char_indices()
      .nth(char_pos)
      .map(|(i, _)| i)
      .unwrap_or(self.input.len())
  }

  /// Handle character input
  pub fn insert_char(&mut self, c: char) {
    let byte_pos = self.char_pos_to_byte_pos(self.cursor_position);
    self.input.insert(byte_pos, c);
    self.cursor_position += 1;
  }

  /// Handle backspace
  pub fn backspace(&mut self) {
    if self.cursor_position > 0 {
      let byte_pos = self.char_pos_to_byte_pos(self.cursor_position - 1);
      self.input.remove(byte_pos);
      self.cursor_position -= 1;
    }
  }

  /// Handle delete
  pub fn delete(&mut self) {
    if self.cursor_position < self.input.chars().count() {
      let byte_pos = self.char_pos_to_byte_pos(self.cursor_position);
      self.input.remove(byte_pos);
    }
  }

  /// Move cursor left
  pub fn move_cursor_left(&mut self) {
    if self.cursor_position > 0 {
      self.cursor_position -= 1;
    }
  }

  /// Move cursor right
  pub fn move_cursor_right(&mut self) {
    if self.cursor_position < self.input.chars().count() {
      self.cursor_position += 1;
    }
  }

  /// Move cursor to start
  pub fn move_cursor_home(&mut self) {
    self.cursor_position = 0;
  }

  /// Move cursor to end
  pub fn move_cursor_end(&mut self) {
    self.cursor_position = self.input.chars().count();
  }

  /// Navigate to previous (older) history entry
  fn navigate_up(&mut self) {
    if self.history.should_navigate(&self.input)
      && let Some(entry) = self.history.navigate_up(&self.input)
    {
      self.input = entry.text.clone();
      self.cursor_position = self.input.chars().count();
    }
  }

  /// Navigate to next (newer) history entry
  fn navigate_down(&mut self) {
    // Check if we were browsing before navigation
    let was_browsing = self.history.is_browsing();

    if let Some(entry) = self.history.navigate_down() {
      self.input = entry.text.clone();
      self.cursor_position = self.input.chars().count();
    } else if was_browsing {
      // navigate_down returned None and we were browsing - this means we exited browsing mode
      // Restore original input
      self.input = self.history.original_input().to_string();
      self.cursor_position = self.input.chars().count();
    }
  }

  /// Save current input to history
  fn save_to_history(&mut self) {
    self.history.record_entry(&self.input);
  }

  /// Submit the current input as a message
  ///
  /// State transition: WaitingInput → Animating
  /// Sends message directly to LLM via SessionHandle
  pub fn submit_message(&mut self, data: &mut AppData) {
    if !self.input.is_empty() {
      // Save input to history before submitting
      self.save_to_history();

      let message = take(&mut self.input);
      // Add user message to chat history
      data.chat_history.push(ChatMessage::User {
        content: message.clone(),
      });
      self.cursor_position = 0;

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

  /// Calculate display width of a string (CJK characters are width 2)
  fn display_width(s: &str) -> usize {
    string_display_width(s)
  }

  /// Wrap text into lines based on available width
  fn wrap_text(text: &str, available_width: u16) -> Vec<String> {
    if available_width == 0 {
      return vec![text.to_string()];
    }
    let available = available_width as usize;
    let mut lines: Vec<String> = vec![];
    let mut current_line = String::new();
    let mut current_width = 0;

    for c in text.chars() {
      let char_width = char_display_width(c);

      if c == '\n' {
        lines.push(current_line);
        current_line = String::new();
        current_width = 0;
      } else if current_width + char_width > available {
        lines.push(current_line);
        current_line = c.to_string();
        current_width = char_width;
      } else {
        current_line.push(c);
        current_width += char_width;
      }
    }

    if !current_line.is_empty() {
      lines.push(current_line);
    }

    lines
  }

  /// Calculate the number of lines needed to display text with given width
  fn calculate_line_count(text: &str, available_width: u16) -> usize {
    Self::wrap_text(text, available_width).len().max(1)
  }

  /// Calculate the number of lines needed to display text with prefix (like prompt) and given width
  fn calculate_line_count_with_prefix(
    text: &str,
    prefix_width: usize,
    available_width: u16,
  ) -> usize {
    if available_width == 0 {
      return 1;
    }
    let available = available_width as usize;
    let mut lines = 1;
    let mut current_width = prefix_width;

    for c in text.chars() {
      let char_width = char_display_width(c);

      if c == '\n' {
        lines += 1;
        current_width = 0;
      } else if current_width + char_width > available {
        lines += 1;
        current_width = char_width;
      } else {
        current_width += char_width;
      }
    }

    lines
  }

  /// Calculate the number of lines needed to display prompt + text with given width
  fn calculate_input_line_count(&self, text: &str, available_width: u16) -> usize {
    let prompt_width = Self::display_width(&self.full_prompt());
    Self::calculate_line_count_with_prefix(text, prompt_width, available_width)
  }

  /// Find cursor position (line number and column within that line)
  fn find_cursor_position(&self, available_width: u16) -> (usize, usize) {
    if available_width == 0 {
      return (0, 0);
    }
    let available = available_width as usize;
    let prompt_width = Self::display_width(&self.full_prompt());

    let mut line = 0;
    let mut col = prompt_width; // Start after prompt
    let mut current_line_width = prompt_width;

    for (idx, c) in self.input.chars().enumerate() {
      if idx >= self.cursor_position {
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

  /// Render a message in a box
  fn render_message_box(&self, f: &mut Frame, area: Rect, message: &str) {
    let block = Block::default()
      .borders(Borders::ALL)
      .border_set(border::ROUNDED)
      .border_style(*PRIMARY_BORDER);

    let inner_area = block.inner(area);

    // Render the border block
    f.render_widget(block, area);

    // Manually wrap text to ensure consistency with line count calculation
    let inner_width = inner_area.width;
    let wrapped_lines = Self::wrap_text(message, inner_width);

    // Convert to Lines for rendering
    let lines: Vec<Line> = wrapped_lines.into_iter().map(Line::from).collect();

    let text = Paragraph::new(Text::from(lines));
    f.render_widget(text, inner_area);
  }

  /// Render the moon animation
  fn render_moon_animation(&self, f: &mut Frame, area: Rect) {
    let moon = self.current_moon();
    let text = Text::from(vec![Line::from(vec![
      Span::raw("  "),
      Span::styled(moon.to_string(), *HIGHLIGHT),
    ])]);

    let widget = Paragraph::new(text);
    f.render_widget(widget, area);
  }

  /// Render the thinking indicator ("Thinking..." with spinner)
  fn render_thinking_indicator(&self, f: &mut Frame, area: Rect) {
    let spinner = self.current_spinner();
    let text = Text::from(vec![Line::from(vec![
      Span::raw("  "),
      Span::styled(spinner.to_string(), *THINKING),
      Span::raw(" "),
      Span::styled("Thinking...", *THINKING),
    ])]);

    let widget = Paragraph::new(text);
    f.render_widget(widget, area);
  }

  /// Render AI response as plain text (without box)
  fn render_ai_response(&self, f: &mut Frame, area: Rect, response: &str) {
    let wrapped_lines = Self::wrap_text(response, area.width);
    let lines: Vec<Line> = wrapped_lines.into_iter().map(Line::from).collect();
    let text = Paragraph::new(Text::from(lines));
    f.render_widget(text, area);
  }

  /// Render thinking content with grey italic style
  fn render_thinking_content(&self, f: &mut Frame, area: Rect, content: &str) {
    let wrapped_lines = Self::wrap_text(content, area.width);
    let lines: Vec<Line> = wrapped_lines
      .into_iter()
      .map(|line| Line::from(vec![Span::styled(line, *THINKING)]))
      .collect();
    let text = Paragraph::new(Text::from(lines));
    f.render_widget(text, area);
  }

  /// Render the structured questions panel for AskUserQuestion
  fn render_questions_panel(&self, f: &mut Frame, area: Rect, pq: &PendingQuestions) {
    use ratatui::style::{Modifier, Style};

    let block = Block::default()
      .title("❓ Questions")
      .borders(Borders::ALL)
      .border_style(Style::default().fg(BLUE));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    for (q_idx, q) in pq.questions.iter().enumerate() {
      let is_current = q_idx == pq.current_question_idx;
      let _is_answered = q_idx < pq.answers.len() && !pq.answers[q_idx].is_empty();

      // Header
      if !q.header.is_empty() {
        lines.push(Line::from(vec![Span::styled(
          format!("[{}]", q.header),
          Style::default()
            .fg(HIGHLIGHT_COLOR)
            .add_modifier(Modifier::BOLD),
        )]));
      }

      // Question text
      let q_style = if is_current {
        Style::default().fg(TEXT_COLOR).add_modifier(Modifier::BOLD)
      } else {
        Style::default().fg(TEXT_COLOR)
      };
      lines.push(Line::from(vec![Span::styled(
        format!("{}.", q.question),
        q_style,
      )]));

      // Options
      if q.confirmation {
        // Compact confirmation dialog: [y] Yes  [n] No
        let yes_style = if is_current {
          Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
        } else {
          Style::default().fg(TEXT_COLOR)
        };
        let no_style = if is_current {
          Style::default().fg(CRITICAL).add_modifier(Modifier::BOLD)
        } else {
          Style::default().fg(TEXT_COLOR)
        };
        lines.push(Line::from(vec![
          Span::styled("[y] ", yes_style),
          Span::styled("Yes  ", yes_style),
          Span::styled("[n] ", no_style),
          Span::styled("No", no_style),
        ]));
      } else {
        for (opt_idx, opt) in q.options.iter().enumerate() {
          let is_selected = is_current && pq.selected_option_idx == opt_idx;
          let is_checked = if q_idx < pq.answers.len() {
            pq.answers[q_idx].contains(&opt_idx)
          } else {
            false
          };

          let prefix = if q.multi_select {
            if is_checked { "[x] " } else { "[ ] " }
          } else if is_selected {
            "> "
          } else {
            "  "
          };

          let label_style = if is_selected && is_current {
            Style::default().fg(BLUE).add_modifier(Modifier::BOLD)
          } else if is_checked {
            Style::default().fg(GREEN)
          } else {
            Style::default().fg(TEXT_COLOR)
          };

          let mut spans = vec![
            Span::styled(prefix, label_style),
            Span::styled(&opt.label, label_style),
          ];

          if !opt.description.is_empty() {
            spans.push(Span::styled(
              format!(" - {}", opt.description),
              Style::default().fg(TEXT_COLOR),
            ));
          }

          lines.push(Line::from(spans));
        }
      }

      // Spacing between questions
      if q_idx + 1 < pq.questions.len() {
        lines.push(Line::from(""));
      }
    }

    // Hint line
    if let Some(q) = pq.questions.get(pq.current_question_idx) {
      let hint = if q.confirmation {
        "[y] yes  [n] no  [q] dismiss"
      } else if q.multi_select {
        "[Space] toggle  [Enter] confirm  [q] dismiss"
      } else {
        "[Enter] confirm  [1-4] quick select  [q] dismiss"
      };
      lines.push(Line::from(vec![Span::styled(
        hint,
        Style::default().fg(TEXT_COLOR),
      )]));
    }

    let text = Text::from(lines);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
    f.render_widget(paragraph, inner);
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
            .approve_tool_call(&approval.tool_call_id, true);
          data.pending_approval = None;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
          self
            .session_handle
            .approve_tool_call(&approval.tool_call_id, false);
          data.pending_approval = None;
        }
        KeyCode::Char('a') => {
          self.session_handle.enable_session_yolo();
          self
            .session_handle
            .approve_tool_call(&approval.tool_call_id, true);
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
            if !q.multi_select {
              questions.answers[q_idx] = vec![questions.selected_option_idx];
            }
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
        self.navigate_up();
      }
      KeyCode::Down => {
        self.navigate_down();
      }
      KeyCode::Backspace => {
        self.backspace();
      }
      KeyCode::Delete => {
        self.delete();
      }
      KeyCode::Left => {
        self.move_cursor_left();
      }
      KeyCode::Right => {
        self.move_cursor_right();
      }
      KeyCode::Home => {
        self.move_cursor_home();
      }
      KeyCode::End => {
        self.move_cursor_end();
      }
      KeyCode::Char(c) => {
        self.insert_char(c);
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
      .calculate_input_line_count(&self.input, available_width)
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
          let box_content_lines = Self::calculate_line_count(content, box_inner_width);
          let box_height = box_content_lines + 2; // +2 for top and bottom borders
          constraints.push(Constraint::Length(box_height as u16));
        }
        ChatMessage::Assistant {
          content,
          thinking_content,
        } => {
          // AI message: thinking content (if any) + main content
          if let Some(thinking) = thinking_content {
            let thinking_lines = Self::calculate_line_count(thinking, available_width);
            constraints.push(Constraint::Length(thinking_lines as u16));
          }
          let content_lines = Self::calculate_line_count(content, available_width);
          constraints.push(Constraint::Length(content_lines as u16));
        }
        ChatMessage::ToolCall {
          name,
          arguments,
          output,
        } => {
          // Tool call: showing tool name, arguments, and optional diff panel
          let tool_text = format!("• Used {}({})", name, arguments);
          let mut lines = Self::calculate_line_count(&tool_text, available_width);
          if let Some(out) = output {
            lines += diff_render_height(out);
          }
          constraints.push(Constraint::Length(lines.max(1) as u16));
        }
        ChatMessage::System { content, .. } => {
          // System message: single line notification
          let lines = Self::calculate_line_count(content, available_width);
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
        .map(|c| Self::calculate_line_count(c.content(), available_width))
        .sum();
      if total_lines > 0 {
        constraints.push(Constraint::Length(total_lines as u16));
      }
    }

    // Approval prompt (if pending)
    let approval_height = if let Some(ref approval) = data.pending_approval {
      let diff_lines = approval
        .diff_preview
        .as_ref()
        .map(|d| diff_preview_compact_height(d))
        .unwrap_or(0);
      (2 + diff_lines).min(20) as u16
    } else {
      0
    };
    if approval_height > 0 {
      constraints.push(Constraint::Length(approval_height));
    }

    // Structured questions panel (if pending)
    let question_height = if let Some(ref pq) = data.pending_questions {
      let mut h = 3; // border + title + padding
      for q in &pq.questions {
        if !q.header.is_empty() {
          h += 1;
        }
        h += Self::calculate_line_count(&q.question, available_width.saturating_sub(4));
        h += q.options.len();
        h += 1; // spacing between questions
      }
      h += 1; // hint line
      h.min(30) as u16
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
    let prompt_width = Self::display_width(&self.full_prompt());
    let mut total_fixed_height: usize = data
      .chat_history
      .iter()
      .map(|msg| match msg {
        ChatMessage::User { content } => {
          Self::calculate_line_count_with_prefix(content, prompt_width, available_width)
            + Self::calculate_line_count(content, box_inner_width)
            + 2
        }
        ChatMessage::Assistant { content, .. } => {
          Self::calculate_line_count(content, available_width)
        }
        ChatMessage::ToolCall {
          name,
          arguments,
          output,
        } => {
          let tool_text = format!("• Used {}({})", name, arguments);
          let output_lines = output.as_ref().map(|o| o.lines().count()).unwrap_or(0);
          Self::calculate_line_count(&tool_text, available_width) + output_lines
        }
        ChatMessage::System { content, .. } => Self::calculate_line_count(content, available_width),
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
        .map(|c| Self::calculate_line_count(c.content(), available_width))
        .sum();
      total_fixed_height += chunks_height;
    }
    // Add approval prompt height if pending
    if let Some(ref approval) = data.pending_approval {
      let diff_lines = approval
        .diff_preview
        .as_ref()
        .map(|d| diff_preview_compact_height(d))
        .unwrap_or(0);
      total_fixed_height += 2 + diff_lines;
    }
    // Add question panel height if pending
    if let Some(ref pq) = data.pending_questions {
      let mut h = 3;
      for q in &pq.questions {
        if !q.header.is_empty() {
          h += 1;
        }
        h += Self::calculate_line_count(&q.question, available_width.saturating_sub(4));
        h += q.options.len();
        h += 1;
      }
      h += 1;
      total_fixed_height += h.min(30);
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
            self.render_message_box(f, chunks[chunk_idx], content);
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
            self.render_thinking_content(f, chunks[chunk_idx], thinking);
            chunk_idx += 1;
          }
          if chunk_idx < chunks.len() {
            self.render_ai_response(f, chunks[chunk_idx], content);
            chunk_idx += 1;
          }
        }
        ChatMessage::ToolCall {
          name,
          arguments,
          output,
        } => {
          // Tool call message: render as "• Used ToolName(Params)" with colors:
          // • - green, Used - white, ToolName - blue, Params - yellow
          // If output contains a diff, render it as a bordered panel with line numbers.
          if chunk_idx < chunks.len() {
            use ratatui::style::{Modifier, Style};
            let area = chunks[chunk_idx];
            if let Some(out) = output {
              let layout = ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(area);

              // Render title line
              let title_text = Text::from(vec![Line::from(vec![
                Span::styled(
                  "• ",
                  Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
                ),
                Span::styled("Used ", Style::default().fg(TEXT_COLOR)),
                Span::styled(name, Style::default().fg(BLUE).add_modifier(Modifier::BOLD)),
                Span::styled("(", Style::default().fg(TEXT_COLOR)),
                Span::styled(arguments, Style::default().fg(HIGHLIGHT_COLOR)),
                Span::styled(")", Style::default().fg(TEXT_COLOR)),
              ])]);
              f.render_widget(Paragraph::new(title_text), layout[0]);

              // Render diff panel
              render_diff_panel(f, layout[1], out);
            } else {
              let text = Text::from(vec![Line::from(vec![
                Span::styled(
                  "• ",
                  Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
                ),
                Span::styled("Used ", Style::default().fg(TEXT_COLOR)),
                Span::styled(name, Style::default().fg(BLUE).add_modifier(Modifier::BOLD)),
                Span::styled("(", Style::default().fg(TEXT_COLOR)),
                Span::styled(arguments, Style::default().fg(HIGHLIGHT_COLOR)),
                Span::styled(")", Style::default().fg(TEXT_COLOR)),
              ])]);
              let widget = Paragraph::new(text);
              f.render_widget(widget, area);
            }
            chunk_idx += 1;
          }
        }
        ChatMessage::System { content, level } => {
          // System notification: render with appropriate color based on level
          if chunk_idx < chunks.len() {
            use ratatui::style::{Modifier, Style};
            let color = match level {
              SystemMessageLevel::Info => HIGHLIGHT_COLOR,
              SystemMessageLevel::Warning => WARNING,
              SystemMessageLevel::Error => CRITICAL,
            };
            let text = Text::from(vec![Line::from(vec![Span::styled(
              content.clone(),
              Style::default().fg(color).add_modifier(Modifier::BOLD),
            )])]);
            let widget = Paragraph::new(text).alignment(ratatui::layout::Alignment::Center);
            f.render_widget(widget, chunks[chunk_idx]);
            chunk_idx += 1;
          }
        }
      }
    }

    // Render moon animation if in Animating state
    if self.state == ChatDisplayState::Animating && chunk_idx < chunks.len() {
      self.render_moon_animation(f, chunks[chunk_idx]);
      chunk_idx += 1;
    }

    // Render thinking indicator if in Thinking state
    if self.state == ChatDisplayState::Thinking && chunk_idx < chunks.len() {
      self.render_thinking_indicator(f, chunks[chunk_idx]);
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
            self.render_thinking_content(f, chunks[chunk_idx], &content);
          } else {
            self.render_ai_response(f, chunks[chunk_idx], &content);
          }
          chunk_idx += 1;
        }
      }
    }

    // Render questions panel if pending
    if let Some(ref pq) = data.pending_questions
      && chunk_idx < chunks.len()
    {
      self.render_questions_panel(f, chunks[chunk_idx], pq);
      chunk_idx += 1;
    }

    // Render approval prompt if pending
    if let Some(ref approval) = data.pending_approval
      && chunk_idx < chunks.len()
    {
      use ratatui::style::{Modifier, Style};
      let area = chunks[chunk_idx];

      if let Some(ref diff) = approval.diff_preview {
        let layout = ratatui::layout::Layout::default()
          .direction(ratatui::layout::Direction::Vertical)
          .constraints([Constraint::Length(2), Constraint::Min(0)])
          .split(area);

        // Render approval header
        let header_lines = vec![
          Line::from(vec![
            Span::styled(
              "⏸ ",
              Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
              format!("{} requires approval", approval.name),
              Style::default().fg(TEXT_COLOR).add_modifier(Modifier::BOLD),
            ),
          ]),
          Line::from(vec![
            Span::styled(
              "[y] ",
              Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled("approve  ", Style::default().fg(TEXT_COLOR)),
            Span::styled(
              "[n] ",
              Style::default().fg(CRITICAL).add_modifier(Modifier::BOLD),
            ),
            Span::styled("deny  ", Style::default().fg(TEXT_COLOR)),
            Span::styled(
              "[a] ",
              Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            ),
            Span::styled("allow session", Style::default().fg(TEXT_COLOR)),
          ]),
        ];
        let text = Text::from(header_lines);
        let widget = Paragraph::new(text).alignment(ratatui::layout::Alignment::Center);
        f.render_widget(widget, layout[0]);

        // Render compact diff preview
        render_diff_preview_compact(f, layout[1], diff);
      } else {
        let lines = vec![
          Line::from(vec![
            Span::styled(
              "⏸ ",
              Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
              format!("{} requires approval", approval.name),
              Style::default().fg(TEXT_COLOR).add_modifier(Modifier::BOLD),
            ),
          ]),
          Line::from(vec![
            Span::styled(
              "[y] ",
              Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled("approve  ", Style::default().fg(TEXT_COLOR)),
            Span::styled(
              "[n] ",
              Style::default().fg(CRITICAL).add_modifier(Modifier::BOLD),
            ),
            Span::styled("deny  ", Style::default().fg(TEXT_COLOR)),
            Span::styled(
              "[a] ",
              Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            ),
            Span::styled("allow session", Style::default().fg(TEXT_COLOR)),
          ]),
        ];
        let text = Text::from(lines);
        let widget = Paragraph::new(text).alignment(ratatui::layout::Alignment::Center);
        f.render_widget(widget, area);
      }
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
      self.render_input_line(f, chunks[chunk_idx], &self.input, true);

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
mod tests {
  use std::sync::Once;

  use crossterm::event::{KeyCode, KeyEvent};
  use tokio::sync::mpsc;

  use crate::cli::app::PendingQuestions;
  use crate::config::{Config, init_global_config};
  use crate::llm::Question;
  use crate::llm::session::SessionCommand;

  use super::*;

  static INIT_CONFIG: Once = Once::new();

  fn init_test_config() {
    INIT_CONFIG.call_once(|| {
      init_global_config(Config::default());
    });
  }

  fn make_session_handle() -> (SessionHandle, mpsc::UnboundedReceiver<SessionCommand>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    (
      SessionHandle::test_new("test-session".to_string(), cmd_tx),
      cmd_rx,
    )
  }

  fn make_pending_questions() -> PendingQuestions {
    PendingQuestions {
      tool_call_id: "call-123".to_string(),
      questions: vec![
        Question {
          question: "Which color?".to_string(),
          header: "Style".to_string(),
          options: vec![
            crate::llm::session::QuestionOption {
              label: "Red".to_string(),
              description: "Bold".to_string(),
            },
            crate::llm::session::QuestionOption {
              label: "Blue".to_string(),
              description: "Calm".to_string(),
            },
          ],
          multi_select: false,
          confirmation: false,
        },
        Question {
          question: "Pick sizes".to_string(),
          header: "".to_string(),
          options: vec![
            crate::llm::session::QuestionOption {
              label: "Small".to_string(),
              description: "".to_string(),
            },
            crate::llm::session::QuestionOption {
              label: "Large".to_string(),
              description: "".to_string(),
            },
          ],
          multi_select: true,
          confirmation: false,
        },
      ],
      current_question_idx: 0,
      answers: Vec::new(),
      selected_option_idx: 0,
    }
  }

  #[test]
  fn test_question_keyboard_down_navigation() {
    init_test_config();
    let (session_handle, mut cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_pending_questions());

    // Press Down to move from option 0 to option 1
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Down));

    let pq = data.pending_questions.as_ref().unwrap();
    assert_eq!(pq.selected_option_idx, 1);
    assert!(cmd_rx.try_recv().is_err()); // no command sent yet
  }

  #[test]
  fn test_question_single_select_enter() {
    init_test_config();
    let (session_handle, _cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_pending_questions());

    // Press Enter to confirm first question (single-select)
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));

    // Should move to next question
    let pq = data.pending_questions.as_ref().unwrap();
    assert_eq!(pq.current_question_idx, 1);
    assert_eq!(pq.answers.len(), 1);
    assert_eq!(pq.answers[0], vec![0]); // first option selected
  }

  #[test]
  fn test_question_multi_select_toggle() {
    init_test_config();
    let (session_handle, _cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_pending_questions());

    // Move to second question (multi-select)
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));
    assert_eq!(
      data
        .pending_questions
        .as_ref()
        .unwrap()
        .current_question_idx,
      1
    );

    // Toggle option 0 with Space
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Char(' ')));
    let pq = data.pending_questions.as_ref().unwrap();
    assert_eq!(pq.answers[1], vec![0]);

    // Move down and toggle option 1
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Down));
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Char(' ')));
    let pq = data.pending_questions.as_ref().unwrap();
    assert_eq!(pq.answers[1], vec![0, 1]);

    // Toggle option 0 off
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Up));
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Char(' ')));
    let pq = data.pending_questions.as_ref().unwrap();
    assert_eq!(pq.answers[1], vec![1]);
  }

  #[test]
  fn test_question_complete_all_and_submit() {
    init_test_config();
    let (session_handle, mut cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_pending_questions());

    // Answer first question (single-select, option 1)
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Down));
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));

    // Answer second question (multi-select, toggle option 0)
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Char(' ')));
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));

    // pending_questions should be cleared and command sent
    assert!(data.pending_questions.is_none());
    let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
    match cmd {
      SessionCommand::AnswerQuestions {
        tool_call_id,
        answers,
        dismissed,
      } => {
        assert_eq!(tool_call_id, "call-123");
        assert!(!dismissed);
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0], vec![1]); // Blue
        assert_eq!(answers[1], vec![0]); // Small
      }
      other => panic!("Expected AnswerQuestions, got {:?}", other),
    }
  }

  #[test]
  fn test_question_dismiss_with_q() {
    init_test_config();
    let (session_handle, mut cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_pending_questions());

    view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('q')));

    assert!(data.pending_questions.is_none());
    let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
    match cmd {
      SessionCommand::AnswerQuestions {
        tool_call_id,
        answers,
        dismissed,
      } => {
        assert_eq!(tool_call_id, "call-123");
        assert!(dismissed);
        assert!(answers.is_empty());
      }
      other => panic!("Expected AnswerQuestions, got {:?}", other),
    }
  }

  #[test]
  fn test_question_dismiss_with_esc() {
    init_test_config();
    let (session_handle, mut cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_pending_questions());

    view.handle_key(&mut data, KeyEvent::from(KeyCode::Esc));

    assert!(data.pending_questions.is_none());
    let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
    match cmd {
      SessionCommand::AnswerQuestions {
        tool_call_id,
        answers,
        dismissed,
      } => {
        assert_eq!(tool_call_id, "call-123");
        assert!(dismissed);
        assert!(answers.is_empty());
      }
      other => panic!("Expected AnswerQuestions, got {:?}", other),
    }
  }

  #[test]
  fn test_question_digit_quick_select() {
    init_test_config();
    let (session_handle, _cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_pending_questions());

    // Press '2' to select option 1 (0-indexed) and auto-confirm single-select
    view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('2')));

    let pq = data.pending_questions.as_ref().unwrap();
    assert_eq!(pq.current_question_idx, 1); // moved to next question
    assert_eq!(pq.answers[0], vec![1]); // Blue selected
  }

  fn make_confirmation_question() -> PendingQuestions {
    PendingQuestions {
      tool_call_id: "call-confirm".to_string(),
      questions: vec![Question {
        question: "Are you sure?".to_string(),
        header: "Confirm".to_string(),
        options: vec![
          crate::llm::session::QuestionOption {
            label: "Yes".to_string(),
            description: String::new(),
          },
          crate::llm::session::QuestionOption {
            label: "No".to_string(),
            description: String::new(),
          },
        ],
        multi_select: false,
        confirmation: true,
      }],
      current_question_idx: 0,
      answers: Vec::new(),
      selected_option_idx: 0,
    }
  }

  #[test]
  fn test_question_confirmation_yes() {
    init_test_config();
    let (session_handle, mut cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_confirmation_question());

    view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('y')));

    assert!(data.pending_questions.is_none());
    let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
    match cmd {
      SessionCommand::AnswerQuestions {
        tool_call_id,
        answers,
        dismissed,
      } => {
        assert_eq!(tool_call_id, "call-confirm");
        assert!(!dismissed);
        assert_eq!(answers, vec![vec![0]]); // Yes = index 0
      }
      other => panic!("Expected AnswerQuestions, got {:?}", other),
    }
  }

  #[test]
  fn test_question_confirmation_no() {
    init_test_config();
    let (session_handle, mut cmd_rx) = make_session_handle();
    let mut data = AppData::new();
    let mut view = ChatView::new(&data, session_handle);
    data.pending_questions = Some(make_confirmation_question());

    view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('n')));

    assert!(data.pending_questions.is_none());
    let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
    match cmd {
      SessionCommand::AnswerQuestions {
        tool_call_id,
        answers,
        dismissed,
      } => {
        assert_eq!(tool_call_id, "call-confirm");
        assert!(!dismissed);
        assert_eq!(answers, vec![vec![1]]); // No = index 1
      }
      other => panic!("Expected AnswerQuestions, got {:?}", other),
    }
  }
}
