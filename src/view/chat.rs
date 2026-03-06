use crossterm::event::KeyCode;
use ratatui::{
  layout::{Constraint, Direction, Layout},
  style::{Color, Style},
  text::Text,
  widgets::{Block, Borders, Clear, Paragraph, Wrap},
  Frame,
};

use crate::app::App;
use crate::view::View;

/// Chat view state
pub struct ChatView {
  /// Current input text
  pub input: String,
  /// Cursor position in the input (character index, not byte index)
  pub cursor_position: usize,
}

impl ChatView {
  /// Create a new chat view
  pub fn new() -> Self {
    Self {
      input: String::new(),
      cursor_position: 0,
    }
  }

  /// Get byte position from character position
  fn char_pos_to_byte_pos(&self, char_pos: usize) -> usize {
    self.input
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
  pub fn submit_message(&mut self, app: &mut App) {
    if !self.input.is_empty() {
      let message = std::mem::take(&mut self.input);
      app.messages.push(format!("You: {}", message));
      self.cursor_position = 0;
    }
  }
}

impl View for ChatView {
  fn handle_key(&mut self, app: &mut App, key: KeyCode) -> Option<Box<dyn View>> {
    match key {
      KeyCode::Esc => {
        // Return to home view
        return Some(Box::new(crate::view::HomeView::new()));
      }
      KeyCode::Enter => {
        self.submit_message(app);
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

  fn draw(&self, f: &mut Frame, app: &App) {
    let area = f.area();

    // Clear the background
    f.render_widget(Clear, area);

    // Create vertical layout: messages on top, input at bottom
    let chunks = Layout::default()
      .direction(Direction::Vertical)
      .constraints([
        Constraint::Min(3),    // Message display area
        Constraint::Length(3), // Input area
      ])
      .split(area);

    // Render message display area
    let messages_text = if app.messages.is_empty() {
      Text::from("Type a message and press Enter to send, ESC to go back\n")
    } else {
      Text::from(app.messages.join("\n"))
    };

    let messages_widget = Paragraph::new(messages_text)
      .block(
        Block::default()
          .title(" Chat ")
          .borders(Borders::ALL)
          .border_style(Style::default().fg(Color::Cyan)),
      )
      .wrap(Wrap { trim: true });

    f.render_widget(messages_widget, chunks[0]);

    // Render input area
    let input_widget = Paragraph::new(self.input.as_str())
      .block(
        Block::default()
          .title(" Input ")
          .borders(Borders::ALL)
          .border_style(Style::default().fg(Color::Yellow)),
      )
      .style(Style::default().fg(Color::White));

    f.render_widget(input_widget, chunks[1]);

    // Set cursor position
    // Calculate display width (accounting for wide characters)
    let display_width: usize = self.input.chars().take(self.cursor_position).map(|c| {
      // CJK characters are width 2, others are width 1
      if c >= '\u{4e00}' && c <= '\u{9fff}' {
        2
      } else {
        1
      }
    }).sum();
    let cursor_x = chunks[1].x + display_width as u16 + 2;
    let cursor_y = chunks[1].y + 1;
    f.set_cursor_position((cursor_x, cursor_y));
  }
}

impl Default for ChatView {
  fn default() -> Self {
    Self::new()
  }
}
