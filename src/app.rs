use crossterm::event::KeyCode;

/// Application state
pub struct App {
  /// Current input text
  pub input: String,
  /// Message history
  pub messages: Vec<String>,
  /// Cursor position in the input
  pub cursor_position: usize,
  /// Whether the app should exit
  pub should_exit: bool,
}

impl App {
  /// Create a new app instance
  pub fn new() -> Self {
    Self {
      input: String::new(),
      messages: Vec::new(),
      cursor_position: 0,
      should_exit: false,
    }
  }

  /// Handle keyboard events
  pub fn handle_key(&mut self, key: KeyCode) {
    match key {
      // ESC to exit
      KeyCode::Esc => {
        self.should_exit = true;
      }
      // Enter to submit input
      KeyCode::Enter => {
        if !self.input.is_empty() {
          let message = self.input.clone();
          self.messages.push(format!("You: {}", message));
          // Process user input here, e.g., send to AI
          println!("\rUser input: {}", message); // For debugging
          self.input.clear();
          self.cursor_position = 0;
        }
      }
      // Backspace to delete character before cursor
      KeyCode::Backspace => {
        if self.cursor_position > 0 {
          self.input.remove(self.cursor_position - 1);
          self.cursor_position -= 1;
        }
      }
      // Delete to delete character after cursor
      KeyCode::Delete => {
        if self.cursor_position < self.input.len() {
          self.input.remove(self.cursor_position);
        }
      }
      // Left arrow to move cursor left
      KeyCode::Left => {
        if self.cursor_position > 0 {
          self.cursor_position -= 1;
        }
      }
      // Right arrow to move cursor right
      KeyCode::Right => {
        if self.cursor_position < self.input.len() {
          self.cursor_position += 1;
        }
      }
      // Home to move cursor to start of line
      KeyCode::Home => {
        self.cursor_position = 0;
      }
      // End to move cursor to end of line
      KeyCode::End => {
        self.cursor_position = self.input.len();
      }
      // Character input
      KeyCode::Char(c) => {
        self.input.insert(self.cursor_position, c);
        self.cursor_position += 1;
      }
      // Ignore other keys
      _ => {}
    }
  }
}

impl Default for App {
  fn default() -> Self {
    Self::new()
  }
}
