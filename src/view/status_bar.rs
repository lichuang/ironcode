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

/// Height of the status bar in lines (1 for separator line + 2 for content)
pub const STATUS_BAR_HEIGHT: u16 = 3;

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
  /// Maximum context size for the model
  pub max_context_size: usize,
  /// Whether history navigation is active
  #[allow(dead_code)]
  pub history_active: bool,
}

impl StatusBarInfo {
  /// Create status bar info from AppData
  pub fn from_app_data(data: &AppData, session_id: &str, state: ChatDisplayState) -> Self {
    // Get short session ID (first 8 characters)
    let short_id = session_id.chars().take(8).collect();

    // Get model name and max context size from config
    let (model_name, max_context_size) = data
      .config
      .as_ref()
      .map(|c| {
        let model = c.default_model.clone();
        // Try to get max_context_size from model config, default to 128k
        let max_size = c
          .models
          .get(&c.default_model)
          .and_then(|m| m.max_context_size)
          .unwrap_or(128_000);
        (model, max_size)
      })
      .unwrap_or_else(|| ("unknown".to_string(), 128_000));

    // Calculate estimated token count from all messages
    let token_count = estimate_chat_messages_tokens(&data.chat_history);

    Self {
      session_id: short_id,
      model_name,
      state,
      token_count,
      max_context_size,
      history_active: false, // Will be set by ChatView if needed
    }
  }
}

/// Render the status bar at the bottom of the screen
///
/// Layout:
/// - Line 0: Full-width horizontal separator line (─)
/// - Line 1: Session ID, Model, Status
/// - Line 2: Token usage and Keyboard shortcuts
pub fn render_status_bar(f: &mut Frame, area: Rect, info: &StatusBarInfo) {
  // Split area into separator line and two content lines
  let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Length(1),
      Constraint::Length(1),
      Constraint::Length(1),
    ])
    .split(area);

  let line_area = chunks[0];
  let first_line_area = chunks[1];
  let second_line_area = chunks[2];

  // Draw full-width horizontal separator line
  let separator_line = "─".repeat(line_area.width as usize);
  let separator_widget = Paragraph::new(separator_line).style(Style::default().fg(SUBTLE));
  f.render_widget(separator_widget, line_area);

  // Render first line: Session, Model, Status
  render_first_line(f, first_line_area, info);

  // Render second line: Token usage and Shortcuts
  render_second_line(f, second_line_area, info);
}

/// Render the first line: Session ID, Model, and Status
fn render_first_line(f: &mut Frame, area: Rect, info: &StatusBarInfo) {
  // Split into left (Session/Model) and right (Status)
  let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
    .split(area);

  // Left: Session ID and Model
  let left_text = Line::from(vec![
    Span::styled("Session: ", Style::default().fg(SUBTLE)),
    Span::styled(&info.session_id, Style::default().fg(PRIMARY)),
    Span::styled(" | ", Style::default().fg(SUBTLE)),
    Span::styled("Model: ", Style::default().fg(SUBTLE)),
    Span::styled(&info.model_name, Style::default().fg(TEXT)),
  ]);

  // Right: Status
  let state_text = format_state(&info.state);
  let right_text = Line::from(vec![
    Span::styled("Status: ", Style::default().fg(SUBTLE)),
    Span::styled(state_text, Style::default().fg(state_color(&info.state))),
  ]);

  f.render_widget(Paragraph::new(left_text), chunks[0]);
  f.render_widget(
    Paragraph::new(right_text).alignment(ratatui::layout::Alignment::Right),
    chunks[1],
  );
}

/// Render the second line: Token usage and Keyboard shortcuts
fn render_second_line(f: &mut Frame, area: Rect, info: &StatusBarInfo) {
  // Split into left (Token usage) and right (Shortcuts)
  let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
    .split(area);

  // Left: Token usage
  let token_text = if info.token_count > 0 {
    let percentage = (info.token_count as f64 / info.max_context_size as f64 * 100.0) as usize;
    format!(
      "Tokens: {}/{} ({}%)",
      info.token_count, info.max_context_size, percentage
    )
  } else {
    "Tokens: 0".to_string()
  };
  let left_text = Line::from(vec![Span::styled(token_text, Style::default().fg(MUTED))]);

  // Right: Keyboard shortcuts
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

  f.render_widget(Paragraph::new(left_text), chunks[0]);
  f.render_widget(
    Paragraph::new(right_text).alignment(ratatui::layout::Alignment::Right),
    chunks[1],
  );
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
