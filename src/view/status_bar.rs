//! Status bar component for displaying session and system information.

use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Modifier, Style},
  text::{Line, Span},
  widgets::Paragraph,
};

use crate::cli::AppData;
use crate::cli::app::CompactionWarning;
use crate::config::DEFAULT_MAX_CONTEXT_SIZE;
use crate::utils::colors::{CRITICAL, MUTED, PRIMARY, SUBTLE, TEXT, WARNING};
use crate::utils::token_counter::estimate_chat_messages_tokens;
use crate::view::chat::ChatDisplayState;

/// Height of the status bar in lines (1 for separator line + 2 for content)
pub const STATUS_BAR_HEIGHT: u16 = 3;

/// Token usage percentage threshold for warning level (yellow)
pub const COMPACTION_WARNING_THRESHOLD: usize = 75;
/// Token usage percentage threshold for critical level (red)
pub const COMPACTION_CRITICAL_THRESHOLD: usize = 85;

/// Calculate compaction warning level from warning data
///
/// # Arguments
/// * `warning` - Optional compaction warning data
/// * `log` - Whether to log debug information
///
/// # Returns
/// The appropriate warning level based on usage percentage
pub fn calculate_compaction_warning_level(
  warning: &Option<CompactionWarning>,
  log: bool,
) -> CompactionWarningLevel {
  if let Some(warning) = warning {
    let percentage = warning.usage_percentage();
    if log {
      log::info!(
        "calculate_compaction_warning_level: current_tokens={}, max_context_size={}, percentage={}",
        warning.current_tokens,
        warning.max_context_size,
        percentage
      );
    }
    if percentage >= COMPACTION_CRITICAL_THRESHOLD {
      if log {
        log::info!("Setting compaction_warning to Critical");
      }
      CompactionWarningLevel::Critical
    } else if percentage >= COMPACTION_WARNING_THRESHOLD {
      if log {
        log::info!("Setting compaction_warning to Warning");
      }
      CompactionWarningLevel::Warning
    } else {
      CompactionWarningLevel::None
    }
  } else {
    CompactionWarningLevel::None
  }
}

/// Compaction warning level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionWarningLevel {
  /// No warning (below threshold)
  None,
  /// Warning (approaching threshold, >= 75%)
  Warning,
  /// Critical (at or above threshold, >= 85%)
  Critical,
}

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
  /// Compaction warning level based on token usage
  pub compaction_warning: CompactionWarningLevel,
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
          .unwrap_or(DEFAULT_MAX_CONTEXT_SIZE);
        (model, max_size)
      })
      .unwrap_or_else(|| ("unknown".to_string(), DEFAULT_MAX_CONTEXT_SIZE));

    // Calculate estimated token count from all messages
    let token_count = estimate_chat_messages_tokens(&data.chat_history);

    // Determine compaction warning level
    let compaction_warning = calculate_compaction_warning_level(&data.compaction_warning, false);

    Self {
      session_id: short_id,
      model_name,
      state,
      token_count,
      max_context_size,
      history_active: false, // Will be set by ChatView if needed
      compaction_warning,
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

  // Left: Token usage with compaction warning
  let (token_text, token_color, warning_span) = if info.token_count > 0 {
    let percentage = (info.token_count as f64 / info.max_context_size as f64 * 100.0) as usize;
    // Format numbers in k units (e.g., 12k instead of 12345)
    let used_k = info.token_count / 1000;
    let max_k = info.max_context_size / 1000;
    let text = format!("Context: {}%({}k/{}k)", percentage, used_k, max_k);

    // Determine color and warning based on compaction level
    match info.compaction_warning {
      CompactionWarningLevel::Critical => {
        let warning = Span::styled(
          " ⚠ COMPACT",
          Style::default().fg(CRITICAL).add_modifier(Modifier::BOLD),
        );
        (text, CRITICAL, Some(warning))
      }
      CompactionWarningLevel::Warning => {
        let warning = Span::styled(" ⚠", Style::default().fg(WARNING));
        (text, WARNING, Some(warning))
      }
      CompactionWarningLevel::None => (text, MUTED, None),
    }
  } else {
    let max_k = info.max_context_size / 1000;
    (format!("Context: 0%(0k/{}k)", max_k), MUTED, None)
  };

  let mut left_spans = vec![Span::styled(token_text, Style::default().fg(token_color))];
  log::info!(
    "render_second_line: info.compaction_warning={:?}, warning_span.is_some()={}",
    info.compaction_warning,
    warning_span.is_some()
  );
  if let Some(warning) = warning_span {
    left_spans.push(warning);
  }
  let left_text = Line::from(left_spans);

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
