//! ApprovalPanel — rendering for tool call approval prompts.

use ratatui::{
  Frame,
  layout::{Constraint, Rect},
  text::{Line, Span, Text},
  widgets::Paragraph,
};

use crate::cli::app::PendingApproval;
use crate::utils::colors::{BLUE, CRITICAL, GREEN, TEXT as TEXT_COLOR, WARNING};
use crate::view::diff::{diff_preview_compact_height, render_diff_preview_compact};

/// Approval panel component for rendering tool call approval UI.
pub struct ApprovalPanel;

impl ApprovalPanel {
  /// Calculate the height needed for the approval panel.
  pub fn height(approval: &PendingApproval) -> u16 {
    let diff_lines = approval
      .diff_preview
      .as_ref()
      .map(|d| diff_preview_compact_height(d))
      .unwrap_or(0);
    (2 + diff_lines).min(20) as u16
  }

  /// Render the approval prompt.
  pub fn render(f: &mut Frame, area: Rect, approval: &PendingApproval) {
    use ratatui::style::{Modifier, Style};

    if let Some(ref diff) = approval.diff_preview {
      let layout = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(area);

      // Render approval header
      let header_lines = vec![
        Line::from(vec![
          Span::styled(
            "⏸ ",
            Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
          ),
          Span::styled(
            format!("{} requires approval", approval.name),
            Style::default().fg(TEXT_COLOR).add_modifier(Modifier::BOLD),
          ),
        ]),
        Line::from(vec![
          Span::styled(
            "[y] ",
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
          ),
          Span::styled("approve  ", Style::default().fg(TEXT_COLOR)),
          Span::styled(
            "[n] ",
            Style::default().fg(CRITICAL).add_modifier(Modifier::BOLD),
          ),
          Span::styled("deny  ", Style::default().fg(TEXT_COLOR)),
          Span::styled(
            "[a] ",
            Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
          ),
          Span::styled("allow session", Style::default().fg(TEXT_COLOR)),
        ]),
      ];
      let text = Text::from(header_lines);
      let widget = Paragraph::new(text).alignment(ratatui::layout::Alignment::Center);
      f.render_widget(widget, layout[0]);

      // Render compact diff preview
      render_diff_preview_compact(f, layout[1], diff);
    } else {
      let lines = vec![
        Line::from(vec![
          Span::styled(
            "⏸ ",
            Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
          ),
          Span::styled(
            format!("{} requires approval", approval.name),
            Style::default().fg(TEXT_COLOR).add_modifier(Modifier::BOLD),
          ),
        ]),
        Line::from(vec![
          Span::styled(
            "[y] ",
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
          ),
          Span::styled("approve  ", Style::default().fg(TEXT_COLOR)),
          Span::styled(
            "[n] ",
            Style::default().fg(CRITICAL).add_modifier(Modifier::BOLD),
          ),
          Span::styled("deny  ", Style::default().fg(TEXT_COLOR)),
          Span::styled(
            "[a] ",
            Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
          ),
          Span::styled("allow session", Style::default().fg(TEXT_COLOR)),
        ]),
      ];
      let text = Text::from(lines);
      let widget = Paragraph::new(text).alignment(ratatui::layout::Alignment::Center);
      f.render_widget(widget, area);
    }
  }
}
