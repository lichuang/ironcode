use crossterm::event::KeyCode;
use ratatui::{
  Frame,
  layout::{Alignment, Constraint, Direction, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span, Text},
  widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::AppData;
use crate::view::{ChatView, View};

/// Home view state
pub struct HomeView {
  /// Current input text in the chat input box
  pub input: String,
  /// Cursor position in the input (character index, not byte index)
  pub cursor_position: usize,
}

impl HomeView {
  /// Create a new home view
  pub fn new() -> Self {
    Self {
      input: String::new(),
      cursor_position: 0,
    }
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

  /// Check if input is empty
  pub fn is_input_empty(&self) -> bool {
    self.input.is_empty()
  }

  /// Take the input (clear and return)
  pub fn take_input(&mut self) -> String {
    let input = std::mem::take(&mut self.input);
    self.cursor_position = 0;
    input
  }

  /// Render the title block
  fn render_title(&self, f: &mut Frame, area: Rect) {
    let title_text = Text::from(vec![
      Line::from(""),
      Line::from(vec![Span::styled(
        "Talos",
        Style::default()
          .fg(Color::Cyan)
          .add_modifier(Modifier::BOLD),
      )])
      .alignment(Alignment::Center),
      Line::from(Span::styled(
        "AI Coding Agent",
        Style::default().fg(Color::Gray),
      ))
      .alignment(Alignment::Center),
    ]);

    let title = Paragraph::new(title_text).block(
      Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan)),
    );

    f.render_widget(title, area);
  }

  /// Render the chat input box
  fn render_input(&self, f: &mut Frame, area: Rect) {
    let input_widget = Paragraph::new(self.input.as_str())
      .block(
        Block::default()
          .title(" Start a chat ")
          .borders(Borders::ALL)
          .border_style(Style::default().fg(Color::Yellow)),
      )
      .style(Style::default().fg(Color::White));

    f.render_widget(input_widget, area);
  }

  /// Render the status bar
  fn render_status_bar(&self, f: &mut Frame, area: Rect) {
    let status_text = Text::from(Line::from(vec![
      Span::raw("Press "),
      Span::styled("Enter", Style::default().fg(Color::Yellow)),
      Span::raw(" to chat, "),
      Span::styled("ESC", Style::default().fg(Color::Yellow)),
      Span::raw(" to exit"),
    ]));

    let status_bar = Paragraph::new(status_text).style(Style::default().fg(Color::Gray));

    f.render_widget(status_bar, area);
  }
}

impl View for HomeView {
  fn handle_key(&mut self, data: &mut AppData, key: KeyCode) -> Option<Box<dyn View>> {
    match key {
      KeyCode::Esc => {
        data.should_exit = true;
      }
      KeyCode::Enter => {
        if !self.is_input_empty() {
          let input = self.take_input();
          data.messages.push(input);
          // Switch to ChatView
          return Some(Box::new(ChatView::new()));
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

  fn draw(&self, f: &mut Frame, _data: &AppData) {
    let area = f.area();

    // Clear the background
    f.render_widget(Clear, area);

    // Create main layout: title, input, status bar
    let main_chunks = Layout::default()
      .direction(Direction::Vertical)
      .constraints([
        Constraint::Length(5), // Title area
        Constraint::Length(3), // Chat input box
        Constraint::Length(1), // Status bar
      ])
      .split(area);

    // Render title
    self.render_title(f, main_chunks[0]);

    // Render chat input box
    self.render_input(f, main_chunks[1]);

    // Render status bar
    self.render_status_bar(f, main_chunks[2]);

    // Set cursor position in input box
    // Calculate display width (CJK characters are width 2, others are width 1)
    let display_width: usize = self
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
    let input_chunk = main_chunks[1];
    let cursor_x = input_chunk.x + display_width as u16 + 2;
    let cursor_y = input_chunk.y + 1;
    f.set_cursor_position((cursor_x, cursor_y));
  }
}

impl Default for HomeView {
  fn default() -> Self {
    Self::new()
  }
}
