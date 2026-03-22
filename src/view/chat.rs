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
use crate::llm::SessionHandle;
use crate::tui::{FrameRequester, TARGET_FRAME_INTERVAL};
use crate::utils::{
  HIGHLIGHT, MOON_FRAMES, PRIMARY, PRIMARY_BORDER, SECONDARY, SPINNER_FRAMES, THINKING,
  char_display_width, string_display_width,
};
use crate::view::View;

/// Error when creating ChatView without a valid session
#[derive(Debug)]
pub struct NoSessionError;

/// A chunk of streaming content from LLM
#[derive(Debug, Clone)]
pub enum StreamingChunk {
  /// Normal response content
  Normal(String),
  /// Thinking/reasoning content
  Thinking(String),
}

impl StreamingChunk {
  /// Get the content of the chunk
  pub fn content(&self) -> &str {
    match self {
      StreamingChunk::Normal(s) => s,
      StreamingChunk::Thinking(s) => s,
    }
  }

  /// Check if this is a thinking chunk
  pub fn is_thinking(&self) -> bool {
    matches!(self, StreamingChunk::Thinking(_))
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
}

impl ChatMessage {
  /// Get the content of the message
  pub fn content(&self) -> &str {
    match self {
      ChatMessage::User { content } => content,
      ChatMessage::Assistant { content, .. } => content,
    }
  }

  /// Get the thinking content (if any)
  pub fn thinking_content(&self) -> Option<&str> {
    match self {
      ChatMessage::User { .. } => None,
      ChatMessage::Assistant { thinking_content, .. } => thinking_content.as_deref(),
    }
  }

  /// Check if this is a user message
  pub fn is_user(&self) -> bool {
    matches!(self, ChatMessage::User { .. })
  }

  /// Check if this is an assistant message
  pub fn is_assistant(&self) -> bool {
    matches!(self, ChatMessage::Assistant { .. })
  }
}



/// Chat display state machine
/// 
/// State transitions:
/// - User submits message → Animating (show moon animation)
/// - LLM starts responding → Streaming (show streaming content)
/// - Response completed → WaitingInput (show spinner waiting for input)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatDisplayState {
  /// Waiting for LLM to start responding: show moon animation
  Animating,
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
      ChatDisplayState::Streaming
    } else if waiting_for_ai {
      ChatDisplayState::Animating
    } else {
      ChatDisplayState::WaitingInput
    };
    
    log::debug!("ChatView created with initial state: {:?}", state);
    
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
    }
  }

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
    let username = std::env::var("USER")
      .or_else(|_| std::env::var("USERNAME"))
      .unwrap_or_else(|_| "user".to_string());

    let current_dir = std::env::current_dir()
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

  /// Submit the current input as a message
  /// 
  /// State transition: WaitingInput → Animating
  /// Sends message directly to LLM via SessionHandle
  pub fn submit_message(&mut self, data: &mut AppData) {
    if !self.input.is_empty() {
      let message = std::mem::take(&mut self.input);
      // Add user message to chat history
      data.chat_history.push(ChatMessage::User { content: message.clone() });
      self.cursor_position = 0;
      
      // Send message directly to LLM via SessionHandle
      log::debug!("Sending message to LLM: {}", &message[..message.len().min(50)]);
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
  ///                  If false, show spinner (waiting for input style)
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
        Span::styled(spinner.to_string(), *PRIMARY)
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
      Span::styled(moon.to_string(), *HIGHLIGHT)
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
      .map(|line| {
        Line::from(vec![Span::styled(
          line,
          *THINKING,
        )])
      })
      .collect();
    let text = Paragraph::new(Text::from(lines));
    f.render_widget(text, area);
  }
}

impl View for ChatView {
  fn handle_key(&mut self, data: &mut AppData, key: KeyEvent) -> Option<Box<dyn View>> {
    match key.code {
      KeyCode::Esc => {
        // Return to home view
        return Some(Box::new(crate::view::HomeView::new()));
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
        // Animating → Streaming: LLM starts responding
        if !data.streaming_response.is_empty() {
          self.enter_streaming();
        }
        // Animating → WaitingInput: AI response added to history
        if data.streaming_response.is_empty() && !last_is_user {
          self.enter_waiting_input();
        }
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

    // Schedule next frame for smooth animation
    frame_requester.schedule_frame_in(TARGET_FRAME_INTERVAL);
  }

  fn draw(&mut self, f: &mut Frame, data: &AppData) {
    
    let area = f.area();
    let available_width = area.width;

    // Calculate input height (dynamic based on content, including prompt width)
    // No height limit - content will wrap naturally based on available width
    let input_height = self.calculate_input_line_count(&self.input, available_width).max(1);

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
        ChatMessage::Assistant { content, thinking_content } => {
          // AI message: thinking content (if any) + main content
          if let Some(thinking) = thinking_content {
            let thinking_lines = Self::calculate_line_count(thinking, available_width);
            constraints.push(Constraint::Length(thinking_lines as u16));
          }
          let content_lines = Self::calculate_line_count(content, available_width);
          constraints.push(Constraint::Length(content_lines as u16));
        }
      }
    }
    // Moon animation row (only shown in Animating state)
    if self.state == ChatDisplayState::Animating {
      constraints.push(Constraint::Length(1));
    }

    // Streaming response chunks (if any) - both thinking and normal content
    if !data.streaming_response.is_empty() {
      // Calculate total height for all chunks
      let total_lines: usize = data.streaming_response.iter()
        .map(|c| Self::calculate_line_count(c.content(), available_width))
        .sum();
      if total_lines > 0 {
        constraints.push(Constraint::Length(total_lines as u16));
      }
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
      })
      .sum::<usize>();
    // Add moon animation height if in Animating state
    if self.state == ChatDisplayState::Animating {
      total_fixed_height += 1;
    }
    // Add streaming response chunks height if present
    if !data.streaming_response.is_empty() {
      let chunks_height: usize = data.streaming_response.iter()
        .map(|c| Self::calculate_line_count(c.content(), available_width))
        .sum();
      total_fixed_height += chunks_height;
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
      .split(area);

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
        ChatMessage::Assistant { content, thinking_content } => {
          // AI message: thinking content (if any) + main content
          if let Some(thinking) = thinking_content {
            if chunk_idx < chunks.len() {
              self.render_thinking_content(f, chunks[chunk_idx], thinking);
              chunk_idx += 1;
            }
          }
          if chunk_idx < chunks.len() {
            self.render_ai_response(f, chunks[chunk_idx], content);
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
    
    // Render streaming response chunks if present
    if !data.streaming_response.is_empty() {
      // Combine consecutive chunks of the same type for rendering
      let mut combined: Vec<(bool, String)> = Vec::new(); // (is_thinking, content)
      
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
        }
      }
      
      // Render combined chunks
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
    
    // Waiting for user input: prompt + spinner (no input box yet)
    // Only shown in WaitingInput state
    if self.state == ChatDisplayState::WaitingInput && chunk_idx < chunks.len() {
      // Show empty input line with spinner
      self.render_input_line(f, chunks[chunk_idx], "", false);
      chunk_idx += 1;
    }

    // Render current input line (only shown in WaitingInput state)
    if self.state == ChatDisplayState::WaitingInput && chunk_idx < chunks.len() && !self.input.is_empty() {
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
