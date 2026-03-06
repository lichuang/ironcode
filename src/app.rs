use crate::view::{HomeView, View};

/// Application state
pub struct App {
  /// Whether the app should exit
  pub should_exit: bool,
  /// Current view (dynamic dispatch)
  pub view: Box<dyn View>,
  /// Message history (for chat)
  pub messages: Vec<String>,
}

impl App {
  /// Create a new app instance
  pub fn new() -> Self {
    Self {
      should_exit: false,
      view: Box::new(HomeView::new()),
      messages: Vec::new(),
    }
  }

  /// Handle keyboard events
  pub fn handle_key(&mut self, key: crossterm::event::KeyCode) {
    // Take ownership of the view temporarily
    let mut view = std::mem::replace(&mut self.view, Box::new(NullView));
    
    // Handle the key and check if we need to switch views
    if let Some(new_view) = view.handle_key(self, key) {
      self.view = new_view;
    } else {
      self.view = view;
    }
  }

  /// Draw the current view
  pub fn draw(&self, f: &mut ratatui::Frame) {
    self.view.draw(f, self);
  }
}

impl Default for App {
  fn default() -> Self {
    Self::new()
  }
}

/// A null view placeholder used during view swapping
struct NullView;

impl View for NullView {
  fn handle_key(&mut self, _app: &mut App, _key: crossterm::event::KeyCode) -> Option<Box<dyn View>> {
    None
  }

  fn draw(&self, _f: &mut ratatui::Frame, _app: &App) {}
}
