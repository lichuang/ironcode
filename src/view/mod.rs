use crossterm::event::KeyCode;
use ratatui::Frame;

use crate::app::App;

pub mod chat;
pub mod home;

pub use chat::ChatView;
pub use home::HomeView;

/// Trait for all views in the application
pub trait View {
  /// Handle keyboard events
  /// 
  /// # Arguments
  /// * `app` - The application state
  /// * `key` - The key code that was pressed
  /// 
  /// # Returns
  /// * `Some(Box<dyn View>)` - If the view wants to switch to a new view
  /// * `None` - If no view switch is needed
  fn handle_key(&mut self, app: &mut App, key: KeyCode) -> Option<Box<dyn View>>;

  /// Draw the view on the frame
  /// 
  /// # Arguments
  /// * `f` - The frame to draw on
  /// * `app` - The application state (for accessing messages, etc.)
  fn draw(&self, f: &mut Frame, app: &App);
}
