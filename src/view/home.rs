use crossterm::event::KeyCode;
use ratatui::{
  layout::{Alignment, Constraint, Direction, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span, Text},
  widgets::{Block, Borders, Clear, Paragraph},
  Frame,
};

use crate::app::App;
use crate::view::View;

/// Home view state
pub struct HomeView {
  /// Current input text in the chat input box
  pub input: String,
  /// Cursor position in the input
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

  /// Handle character input
  pub fn insert_char(&mut self, c: char) {
    self.input.insert(self.cursor_position, c);
    self.cursor_position += 1;
  }

  /// Handle backspace
  pub fn backspace(&mut self) {
    if self.cursor_position > 0 {
      self.input.remove(self.cursor_position - 1);
      self.cursor_position -= 1;
    }
  }

  /// Handle delete
  pub fn delete(&mut self) {
    if self.cursor_position < self.input.len() {
      self.input.remove(self.cursor_position);
    }
  }

  /// Move cursor left
  pub fn move_cursor_left(&mut self) {
    if self.cursor_position > 0 {
      self.cursor_position -= 1;
    }
  }

  /// Move cursor right(&mut self)
  pub fn move_cursor_right(&mut self) {
    if self.cursor_position < self.input.len() {
      self.cursor_position += 1;
    }
  }

  /// Move cursor to start
  pub fn move_cursor_home(&mut self) {
    self.cursor_position = 0;
  }

  /// Move cursor to end
  pub fn move_cursor_end(&mut self) {
    self.cursor_position = self.input.len();
  }

  /// Check if input is empty
  pub fn is_input_empty(&self) -> bool {
    self.input.is_empty()
  }

  /// Take the input (clear and return)
  pub fn take_input(&mut self) -> String {
    let input = self.input.clone();
    self.input.clear();
    self.cursor_position = 0;
    input
  }

  /// Render the title block
  fn render_title(&self, f: &mut Frame, area: Rect) {
    let title_text = Text::from(vec![
      Line::from(""),
      Line::from(vec![
        Span::styled(
          "Talos",
          Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        ),
      ])
      .alignment(Alignment::Center),
      Line::from(
        Span::styled(
          "AI Coding Agent",
          Style::default().fg(Color::Gray),
        ),
      )
      .alignment(Alignment::Center),
    ]);

    let title = Paragraph::new(title_text)
      .block(
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

    let status_bar = Paragraph::new(status_text)
      .style(Style::default().fg(Color::Gray));

    f.render_widget(status_bar, area);
  }
}

impl View for HomeView {
  fn handle_key(&mut self, app: &mut App, key: KeyCode) {
    match key {
      KeyCode::Esc => {
        app.should_exit = true;
      }
      KeyCode::Enter => {
        if !self.is_input_empty() {
          let input = self.take_input();
          app.messages.push(format!("You: {}", input));
          // TODO: Switch to Chat view when implemented
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
  }

  fn draw(&self, f: &mut Frame) {
    let area = f.area();

    // Clear the background
    f.render_widget(Clear, area);

    // Create main layout: title, input, status bar
    let main_chunks = Layout::default()
      .direction(Direction::Vertical)
      .constraints([
        Constraint::Length(5),  // Title area
        Constraint::Length(3),  // Chat input box
        Constraint::Length(1),  // Status bar
      ])
      .split(area);

    // Render title
    self.render_title(f, main_chunks[0]);

    // Render chat input box
    self.render_input(f, main_chunks[1]);

    // Render status bar
    self.render_status_bar(f, main_chunks[2]);

    // Set cursor position in input box
    let input_chunk = main_chunks[1];
    let cursor_x = input_chunk.x + self.cursor_position as u16 + 2;
    let cursor_y = input_chunk.y + 1;
    f.set_cursor_position((cursor_x, cursor_y));
  }
}

impl Default for HomeView {
  fn default() -> Self {
    Self::new()
  }
}
