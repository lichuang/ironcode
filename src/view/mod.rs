use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::cli::AppData;
use crate::tui::FrameRequester;

pub mod chat;
pub mod diff;
pub mod status_bar;
pub mod task_browser;

/// Trait for UI components within a view.
///
/// Components encapsulate a focused area of responsibility (e.g. text input,
/// message list, approval panel) and can be composed into larger views.
#[allow(dead_code)]
pub trait Component {
  /// Handle a keyboard event.
  ///
  /// Returns `true` if the event was consumed by this component.
  fn handle_key(&mut self, key: KeyEvent) -> bool;

  /// Render the component into the given area.
  fn draw(&self, f: &mut Frame, area: Rect);
}

pub use chat::ChatView;
pub use status_bar::{STATUS_BAR_HEIGHT, StatusBarInfo, render_status_bar};

/// Trait for all views in the application
pub trait View {
  /// Handle keyboard events
  ///
  /// # Arguments
  /// * `data` - The application data that can be modified
  /// * `key` - The key code that was pressed
  ///
  /// # Returns
  /// * `Some(Box<dyn View>)` - If the view wants to switch to a new view
  /// * `None` - If no view switch is needed
  fn handle_key(&mut self, data: &mut AppData, key: KeyEvent) -> Option<Box<dyn View>>;

  /// Draw the view on the frame
  ///
  /// # Arguments
  /// * `f` - The frame to draw on
  /// * `data` - The application data (for accessing messages, etc.)
  fn draw(&mut self, f: &mut Frame, data: &AppData);

  /// Called when a new frame is about to be rendered.
  ///
  /// Views can use this to update animation state or request additional frames.
  /// The default implementation does nothing.
  ///
  /// # Arguments
  /// * `frame_requester` - Use this to schedule additional frames if animation is needed
  /// * `data` - The application data (for checking streaming state, etc.)
  fn on_frame(&mut self, _frame_requester: &FrameRequester, _data: &AppData) {}

  /// Set the frame requester for this view.
  ///
  /// Views should store this to request redraws for animations.
  /// The default implementation does nothing.
  fn set_frame_requester(&mut self, _frame_requester: FrameRequester) {}
}
