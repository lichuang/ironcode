//! Status bar component for displaying session and system information.

use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Modifier, Style},
  text::{Line, Span},
  widgets::Paragraph,
};

use crate::cli::AppData;
use crate::utils::colors::{MUTED, PRIMARY, SUBTLE, TEXT};
use crate::utils::token_counter::estimate_chat_messages_tokens;
use crate::view::chat::ChatDisplayState;

/// Height of the status bar in lines (1 for separator line + 1 for content)
pub const STATUS_BAR_HEIGHT: u16 = 2;

/// Information to display in the status bar
#[derive(Debug, Clone)]
pub struct StatusBarInfo {
  /// Short session ID (first 8 chars)
  pub session_id: String,
  /// Current model name
  pub model_name: String,
  /// Current display state
  pub state: ChatDisplayState,
  /// Estimated token count for all messages
  pub token_count: usize,
  /// Whether history navigation is active
  #[allow(dead_code)]
  pub history_active: bool,
}

impl StatusBarInfo {
  /// Create status bar info from AppData
  pub fn from_app_data(data: &AppData, session_id: &str, state: ChatDisplayState) -> Self {
    // Get short session ID (first 8 characters)
    let short_id = session_id.chars().take(8).collect();

    // Get model name from config
    let model_name = data
      .config
      .as_ref()
      .map(|c| c.default_model.clone())
      .unwrap_or_else(|| "unknown".to_string());

    // Calculate estimated token count from all messages
    let token_count = estimate_chat_messages_tokens(&data.chat_history);

    Self {
      session_id: short_id,
      model_name,
      state,
      token_count,
      history_active: false, // Will be set by ChatView if needed
    }
  }
}

/// Render the status bar at the bottom of the screen
///
/// Layout:
/// - Line 0: Full-width horizontal separator line (─)
/// - Line 1: Status bar content (Session, Model, Status, Shortcuts)
pub fn render_status_bar(f: &mut Frame, area: Rect, info: &StatusBarInfo) {
  // Split area into separator line (top) and content (bottom)
  let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Length(1), Constraint::Length(1)])
    .split(area);

  let line_area = chunks[0];
  let content_area = chunks[1];

  // Draw full-width horizontal separator line
  let separator_line = "─".repeat(line_area.width as usize);
  let separator_widget = Paragraph::new(separator_line).style(Style::default().fg(SUBTLE));
  f.render_widget(separator_widget, line_area);

  // Render status bar content
  render_status_content(f, content_area, info);
}

/// Render the status bar content (without separator line)
fn render_status_content(f: &mut Frame, area: Rect, info: &StatusBarInfo) {
  // Left section: Session ID and Model
  let left_text = Line::from(vec![
    Span::styled("Session: ", Style::default().fg(SUBTLE)),
    Span::styled(&info.session_id, Style::default().fg(PRIMARY)),
    Span::styled(" | ", Style::default().fg(SUBTLE)),
    Span::styled("Model: ", Style::default().fg(SUBTLE)),
    Span::styled(&info.model_name, Style::default().fg(TEXT)),
  ]);

  // Center section: Current state and token count
  let state_text = format_state(&info.state);
  let center_text = Line::from(vec![
    Span::styled("Status: ", Style::default().fg(SUBTLE)),
    Span::styled(state_text, Style::default().fg(state_color(&info.state))),
    if info.token_count > 0 {
      Span::styled(
        format!(" | Tokens: ~{}", info.token_count),
        Style::default().fg(MUTED),
      )
    } else {
      Span::raw("")
    },
  ]);

  // Right section: Keyboard shortcuts
  let right_text = Line::from(vec![
    Span::styled(
      "Esc",
      Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
    ),
    Span::styled("=exit ", Style::default().fg(SUBTLE)),
    Span::styled(
      "↑↓",
      Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
    ),
    Span::styled("=history", Style::default().fg(SUBTLE)),
  ]);

  // Render each section with proportional widths
  // Give center more space to prevent token count truncation
  let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
      Constraint::Min(30), // Left: session and model (min 30 chars)
      Constraint::Min(25), // Center: status and token count (min 25 chars)
      Constraint::Min(15), // Right: shortcuts (min 15 chars)
    ])
    .split(area);

  let left_widget = Paragraph::new(left_text);
  let center_widget = Paragraph::new(center_text).alignment(ratatui::layout::Alignment::Center);
  let right_widget = Paragraph::new(right_text).alignment(ratatui::layout::Alignment::Right);

  f.render_widget(left_widget, chunks[0]);
  f.render_widget(center_widget, chunks[1]);
  f.render_widget(right_widget, chunks[2]);
}

/// Format the display state for the status bar
fn format_state(state: &ChatDisplayState) -> String {
  match state {
    ChatDisplayState::Animating => "Waiting for AI...".to_string(),
    ChatDisplayState::Thinking => "Thinking...".to_string(),
    ChatDisplayState::Streaming => "Generating...".to_string(),
    ChatDisplayState::WaitingInput => "Ready".to_string(),
  }
}

/// Get the color for a state
fn state_color(state: &ChatDisplayState) -> ratatui::style::Color {
  use ratatui::style::Color;
  match state {
    ChatDisplayState::Animating => Color::Yellow,
    ChatDisplayState::Thinking => Color::Cyan,
    ChatDisplayState::Streaming => Color::Green,
    ChatDisplayState::WaitingInput => Color::Gray,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_format_state() {
    assert_eq!(
      format_state(&ChatDisplayState::Animating),
      "Waiting for AI..."
    );
    assert_eq!(format_state(&ChatDisplayState::Thinking), "Thinking...");
    assert_eq!(format_state(&ChatDisplayState::Streaming), "Generating...");
    assert_eq!(format_state(&ChatDisplayState::WaitingInput), "Ready");
  }
}
