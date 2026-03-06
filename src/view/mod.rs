use crossterm::event::KeyCode;
use ratatui::Frame;

use crate::app::App;

pub mod home;

pub use home::HomeView;

/// Trait for all views in the application
pub trait View {
  /// Handle keyboard events
  /// 
  /// # Arguments
  /// * `app` - The application state
  /// * `key` - The key code that was pressed
  fn handle_key(&mut self, app: &mut App, key: KeyCode);

  /// Draw the view on the frame
  /// 
  /// # Arguments
  /// * `f` - The frame to draw on
  fn draw(&self, f: &mut Frame);
}
