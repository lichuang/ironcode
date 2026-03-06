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
    // Note: self.view.handle_key needs &mut self.view and &mut self
    // This is a bit tricky because we can't borrow self mutably twice
    // We need to temporarily take ownership of the view
    let mut view = std::mem::replace(&mut self.view, Box::new(NullView));
    view.handle_key(self, key);
    self.view = view;
  }

  /// Draw the current view
  pub fn draw(&self, f: &mut ratatui::Frame) {
    self.view.draw(f);
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
  fn handle_key(&mut self, _app: &mut App, _key: crossterm::event::KeyCode) {}

  fn draw(&self, _f: &mut ratatui::Frame) {}
}
