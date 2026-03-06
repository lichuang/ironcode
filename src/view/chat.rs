use crossterm::event::KeyCode;
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Color, Style},
  symbols::border,
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::AppData;
use crate::view::View;

/// Chat view state
pub struct ChatView {
  /// Current input text
  pub input: String,
  /// Cursor position in the input (character index, not byte index)
  pub cursor_position: usize,
  /// Prompt string (username@directory)
  pub prompt: String,
}

impl ChatView {
  /// Create a new chat view
  pub fn new() -> Self {
    let prompt = Self::build_prompt();
    Self {
      input: String::new(),
      cursor_position: 0,
      prompt,
    }
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

    format!("{}@{}>", username, current_dir)
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
    }
  }

  /// Render an input line (prompt + input)
  fn render_input_line(&self, f: &mut Frame, area: Rect, input: &str) {
    let prompt_span = Span::styled(&self.prompt, Style::default().fg(Color::Green));
    let input_span = Span::raw(input);

    let line = Line::from(vec![prompt_span, input_span]);
    let widget = Paragraph::new(line);

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

    // Render the message text inside
    let text = Paragraph::new(message).wrap(Wrap { trim: true });
    f.render_widget(text, inner_area);
  }
}

impl View for ChatView {
  fn handle_key(&mut self, data: &mut AppData, key: KeyCode) -> Option<Box<dyn View>> {
    match key {
      KeyCode::Esc => {
        // Return to home view
        return Some(Box::new(crate::view::HomeView::new()));
      }
      KeyCode::Enter => {
        self.submit_message(data);
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

    // Calculate layout:
    // For each message: 1 line (prompt + input) + 3 lines (box) = 4 lines
    // For current input: 1 line
    let message_count = data.messages.len();
    let history_lines = message_count * 4; // Each message takes 4 lines
    let total_lines = history_lines + 1; // +1 for current input

    // Build constraints
    let mut constraints: Vec<Constraint> = (0..message_count)
      .flat_map(|_| vec![Constraint::Length(1), Constraint::Length(3)])
      .collect();
    constraints.push(Constraint::Length(1)); // Current input line

    // Add remaining space
    let available_height = area.height as usize;
    if total_lines < available_height {
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

    // Render current input line
    if chunk_idx < chunks.len() {
      self.render_input_line(f, chunks[chunk_idx], &self.input);

      // Set cursor position (after the prompt + input position)
      let prompt_display_width: usize = self
        .prompt
        .chars()
        .map(|c| {
          if c >= '\u{4e00}' && c <= '\u{9fff}' {
            2
          } else {
            1
          }
        })
        .sum();

      let input_display_width: usize = self
        .input
        .chars()
        .take(self.cursor_position)
        .map(|c| {
          if c >= '\u{4e00}' && c <= '\u{9fff}' {
            2
          } else {
            1
          }
        })
        .sum();

      let cursor_x = chunks[chunk_idx].x + prompt_display_width as u16 + input_display_width as u16;
      let cursor_y = chunks[chunk_idx].y;
      f.set_cursor_position((cursor_x, cursor_y));
    }
  }
}

impl Default for ChatView {
  fn default() -> Self {
    Self::new()
  }
}
