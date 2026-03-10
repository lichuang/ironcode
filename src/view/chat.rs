use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Color, Style},
  symbols::border,
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::AppData;
use crate::tui::{FrameRequester, TARGET_FRAME_INTERVAL};
use crate::utils::{char_display_width, string_display_width};
use crate::view::View;

/// Spinner animation frames (classic terminal loading)
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Moon phase animation frames (🌑 → 🌕 → 🌑)
const MOON_FRAMES: &[char] = &['🌑', '🌒', '🌓', '🌔', '🌕', '🌖', '🌗', '🌘'];

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
  /// Last time the moon was updated (None means animation is disabled)
  last_moon_update: Option<Instant>,
  /// Current moon frame index
  moon_frame: usize,
}

impl ChatView {
  /// Create a new chat view
  pub fn new() -> Self {
    let prompt = Self::build_prompt();
    Self {
      input: String::new(),
      cursor_position: 0,
      prompt,
      frame_requester: None,
      animation_enabled: true,
      last_spinner_update: Instant::now(),
      spinner_frame: 0,
      last_moon_update: None,
      moon_frame: 0,
    }
  }

  /// Check if moon animation is currently enabled
  fn is_moon_animation_enabled(&self) -> bool {
    self.last_moon_update.is_some()
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
  pub fn submit_message(&mut self, data: &mut AppData) {
    if !self.input.is_empty() {
      let message = std::mem::take(&mut self.input);
      data.messages.push(message);
      self.cursor_position = 0;
      // Start moon animation after submitting
      self.start_moon_animation();
    }
  }

  /// Start the moon phase animation
  pub fn start_moon_animation(&mut self) {
    self.moon_frame = 0;
    self.last_moon_update = Some(Instant::now());
  }

  /// Stop the moon phase animation
  pub fn stop_moon_animation(&mut self) {
    self.last_moon_update = None;
  }

  /// Get the current moon character
  fn current_moon(&self) -> char {
    if self.is_moon_animation_enabled() {
      MOON_FRAMES[self.moon_frame % MOON_FRAMES.len()]
    } else {
      ' '
    }
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

  /// Render an input line (prompt + spinner + input) with wrapping
  fn render_input_line(&self, f: &mut Frame, area: Rect, input: &str) {
    // Build prompt with styled spinner
    // Prompt: green, Spinner: cyan (to make it stand out)
    let spinner = self.current_spinner();
    let text = Text::from(vec![Line::from(vec![
      Span::styled(&self.prompt, Style::default().fg(Color::Green)),
      Span::raw(" "),
      Span::styled(spinner.to_string(), Style::default().fg(Color::Cyan)),
      Span::raw(" "),
      Span::raw(input),
    ])]);

    let widget = Paragraph::new(text).wrap(Wrap { trim: false });
    f.render_widget(widget, area);
  }

  /// Render a message in a box
  fn render_message_box(&self, f: &mut Frame, area: Rect, message: &str) {
    let block = Block::default()
      .borders(Borders::ALL)
      .border_set(border::ROUNDED)
      .border_style(Style::default().fg(Color::Cyan));

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
    if !self.is_moon_animation_enabled() {
      return;
    }

    let moon = self.current_moon();
    let text = Text::from(vec![Line::from(vec![
      Span::raw("  "),
      Span::styled(moon.to_string(), Style::default().fg(Color::Yellow)),
    ])]);

    let widget = Paragraph::new(text);
    f.render_widget(widget, area);
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

  fn draw(&self, f: &mut Frame, data: &AppData) {
    let area = f.area();
    let available_width = area.width;

    // Calculate input height (dynamic based on content, including prompt width)
    let input_height = self.calculate_input_line_count(&self.input, available_width);
    // Ensure at least 1 line and cap at reasonable max (e.g., 10 lines or half screen)
    let max_input_height = std::cmp::min(10, area.height / 2).max(1) as usize;
    let input_height = input_height.min(max_input_height);

    // Calculate layout:
    // For each message: prompt line + box
    // Moon animation after last message (if enabled)
    // For current input: dynamic lines (if not showing moon animation)
    let mut constraints: Vec<Constraint> = Vec::new();

    // History messages: prompt line + box
    // Box has borders on both sides, so inner width is available_width - 2
    let box_inner_width = available_width.saturating_sub(2);
    for message in &data.messages {
      // Prompt line height (prompt + message)
      let prompt_lines = self.calculate_input_line_count(message, available_width);
      constraints.push(Constraint::Length(prompt_lines as u16));
      // Box height = border top (1) + content lines + border bottom (1)
      let box_content_lines = Self::calculate_line_count(message, box_inner_width);
      let box_height = box_content_lines + 2; // +2 for top and bottom borders
      constraints.push(Constraint::Length(box_height as u16));
    }
    // Moon animation row (shown after the last message if enabled)
    if self.is_moon_animation_enabled() && !data.messages.is_empty() {
      constraints.push(Constraint::Length(1));
    }

    // Current input
    constraints.push(Constraint::Length(input_height as u16));

    // Add remaining space
    let prompt_width = Self::display_width(&self.full_prompt());
    let mut total_fixed_height: usize = data
      .messages
      .iter()
      .map(|m| {
        Self::calculate_line_count_with_prefix(m, prompt_width, available_width)
          + Self::calculate_line_count(m, box_inner_width)
          + 2
      })
      .sum::<usize>();
    // Add moon animation height if enabled
    if self.is_moon_animation_enabled() && !data.messages.is_empty() {
      total_fixed_height += 1;
    }
    // Only add input height if not showing moon animation (waiting for response)
    if !self.is_moon_animation_enabled() {
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

    // Render history: for each message, show input line then box
    let mut chunk_idx = 0;
    for message in &data.messages {
      // Input line (prompt + message)
      if chunk_idx < chunks.len() {
        self.render_input_line(f, chunks[chunk_idx], message);
        chunk_idx += 1;
      }
      // Box with message
      if chunk_idx < chunks.len() {
        self.render_message_box(f, chunks[chunk_idx], message);
        chunk_idx += 1;
      }
    }
    // Moon animation (shown after all messages if enabled)
    if self.is_moon_animation_enabled() && !data.messages.is_empty() && chunk_idx < chunks.len() {
      self.render_moon_animation(f, chunks[chunk_idx]);
      chunk_idx += 1;
    }

    // Render current input line (only if not showing moon animation)
    if !self.is_moon_animation_enabled() && chunk_idx < chunks.len() {
      self.render_input_line(f, chunks[chunk_idx], &self.input);

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

  fn on_frame(&mut self, frame_requester: &FrameRequester) {
    if !self.animation_enabled {
      return;
    }

    let now = Instant::now();

    // Update spinner animation
    let elapsed = now.duration_since(self.last_spinner_update);
    // Update spinner frame every 200ms (relaxed rotation)
    const SPINNER_INTERVAL: Duration = Duration::from_millis(200);
    if elapsed >= SPINNER_INTERVAL {
      self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
      self.last_spinner_update = now;
    }

    // Update moon animation
    if let Some(last_update) = self.last_moon_update {
      let moon_elapsed = now.duration_since(last_update);
      // Moon cycles slower than spinner - one phase every 300ms
      const MOON_INTERVAL: Duration = Duration::from_millis(300);
      if moon_elapsed >= MOON_INTERVAL {
        self.moon_frame = (self.moon_frame + 1) % MOON_FRAMES.len();
        self.last_moon_update = Some(now);
      }
    }

    // Schedule next frame for smooth animation
    frame_requester.schedule_frame_in(TARGET_FRAME_INTERVAL);
  }

  fn set_frame_requester(&mut self, frame_requester: FrameRequester) {
    self.frame_requester = Some(frame_requester.clone());
    // Start animation loop immediately
    if self.animation_enabled {
      frame_requester.schedule_frame_in(TARGET_FRAME_INTERVAL);
    }
  }
}

impl Default for ChatView {
  fn default() -> Self {
    Self::new()
  }
}
