//! InputComponent — text input with cursor management and history navigation.

use crate::history::InputHistoryManager;

/// Text input component managing editing state, cursor position, and history.
pub struct InputComponent {
  text: String,
  cursor: usize,
  history: InputHistoryManager,
}

impl InputComponent {
  /// Create a new input component with the given history manager.
  pub fn new(history: InputHistoryManager) -> Self {
    Self {
      text: String::new(),
      cursor: 0,
      history,
    }
  }

  /// Current input text.
  pub fn text(&self) -> &str {
    &self.text
  }

  /// Current cursor position (character index).
  pub fn cursor(&self) -> usize {
    self.cursor
  }

  /// Convert character position to byte position.
  fn char_pos_to_byte_pos(&self, char_pos: usize) -> usize {
    self
      .text
      .char_indices()
      .nth(char_pos)
      .map(|(i, _)| i)
      .unwrap_or(self.text.len())
  }

  /// Insert a character at cursor position.
  pub fn insert_char(&mut self, c: char) {
    let byte_pos = self.char_pos_to_byte_pos(self.cursor);
    self.text.insert(byte_pos, c);
    self.cursor += 1;
  }

  /// Delete character before cursor (backspace).
  pub fn backspace(&mut self) {
    if self.cursor > 0 {
      let byte_pos = self.char_pos_to_byte_pos(self.cursor - 1);
      self.text.remove(byte_pos);
      self.cursor -= 1;
    }
  }

  /// Delete character at cursor.
  pub fn delete(&mut self) {
    if self.cursor < self.text.chars().count() {
      let byte_pos = self.char_pos_to_byte_pos(self.cursor);
      self.text.remove(byte_pos);
    }
  }

  /// Move cursor left by one character.
  pub fn move_cursor_left(&mut self) {
    if self.cursor > 0 {
      self.cursor -= 1;
    }
  }

  /// Move cursor right by one character.
  pub fn move_cursor_right(&mut self) {
    if self.cursor < self.text.chars().count() {
      self.cursor += 1;
    }
  }

  /// Move cursor to start of line.
  pub fn move_cursor_home(&mut self) {
    self.cursor = 0;
  }

  /// Move cursor to end of line.
  pub fn move_cursor_end(&mut self) {
    self.cursor = self.text.chars().count();
  }

  /// Navigate to previous (older) history entry.
  pub fn navigate_up(&mut self) {
    if self.history.should_navigate(&self.text)
      && let Some(entry) = self.history.navigate_up(&self.text)
    {
      self.text = entry.text.clone();
      self.cursor = self.text.chars().count();
    }
  }

  /// Navigate to next (newer) history entry.
  pub fn navigate_down(&mut self) {
    let was_browsing = self.history.is_browsing();

    if let Some(entry) = self.history.navigate_down() {
      self.text = entry.text.clone();
      self.cursor = self.text.chars().count();
    } else if was_browsing {
      self.text = self.history.original_input().to_string();
      self.cursor = self.text.chars().count();
    }
  }

  /// Save current text to history.
  pub fn save_to_history(&mut self) {
    self.history.record_entry(&self.text);
  }

  /// Take ownership of the current text, clearing the input.
  pub fn take_text(&mut self) -> String {
    self.cursor = 0;
    std::mem::take(&mut self.text)
  }

  /// Replace the current text with the given text, moving cursor to end.
  pub fn replace_text(&mut self, text: String) {
    self.text = text;
    self.cursor = self.text.chars().count();
  }

  /// Check if input is empty.
  pub fn is_empty(&self) -> bool {
    self.text.is_empty()
  }
}
