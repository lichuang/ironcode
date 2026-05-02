//! QuestionsPanel — rendering for structured AskUserQuestion panels.

use ratatui::{
  Frame,
  layout::Rect,
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::cli::app::PendingQuestions;
use crate::utils::colors::{
  BLUE, CRITICAL, GREEN, HIGHLIGHT as HIGHLIGHT_COLOR, TEXT as TEXT_COLOR,
};
use crate::view::chat::messages::calculate_line_count;

/// Questions panel component for rendering structured question UI.
pub struct QuestionsPanel;

impl QuestionsPanel {
  /// Calculate the height needed for the questions panel.
  pub fn height(pq: &PendingQuestions, available_width: u16) -> u16 {
    let mut h = 3; // border + title + padding
    for q in &pq.questions {
      if !q.header.is_empty() {
        h += 1;
      }
      h += calculate_line_count(&q.question, available_width.saturating_sub(4));
      h += q.options.len();
      h += 1; // spacing between questions
    }
    h += 1; // hint line
    h.min(30) as u16
  }

  /// Render the structured questions panel.
  pub fn render(f: &mut Frame, area: Rect, pq: &PendingQuestions) {
    use ratatui::style::{Modifier, Style};

    let block = Block::default()
      .title("❓ Questions")
      .borders(Borders::ALL)
      .border_style(Style::default().fg(BLUE));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    for (q_idx, q) in pq.questions.iter().enumerate() {
      let is_current = q_idx == pq.current_question_idx;
      let _is_answered = q_idx < pq.answers.len() && !pq.answers[q_idx].is_empty();

      // Header
      if !q.header.is_empty() {
        lines.push(Line::from(vec![Span::styled(
          format!("[{}]", q.header),
          Style::default()
            .fg(HIGHLIGHT_COLOR)
            .add_modifier(Modifier::BOLD),
        )]));
      }

      // Question text
      let q_style = if is_current {
        Style::default().fg(TEXT_COLOR).add_modifier(Modifier::BOLD)
      } else {
        Style::default().fg(TEXT_COLOR)
      };
      lines.push(Line::from(vec![Span::styled(
        format!("{}.", q.question),
        q_style,
      )]));

      // Options
      if q.confirmation {
        // Compact confirmation dialog: [y] Yes  [n] No
        let yes_style = if is_current {
          Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
        } else {
          Style::default().fg(TEXT_COLOR)
        };
        let no_style = if is_current {
          Style::default().fg(CRITICAL).add_modifier(Modifier::BOLD)
        } else {
          Style::default().fg(TEXT_COLOR)
        };
        lines.push(Line::from(vec![
          Span::styled("[y] ", yes_style),
          Span::styled("Yes  ", yes_style),
          Span::styled("[n] ", no_style),
          Span::styled("No", no_style),
        ]));
      } else {
        for (opt_idx, opt) in q.options.iter().enumerate() {
          let is_selected = is_current && pq.selected_option_idx == opt_idx;
          let is_checked = if q_idx < pq.answers.len() {
            pq.answers[q_idx].contains(&opt_idx)
          } else {
            false
          };

          let prefix = if q.multi_select {
            if is_checked { "[x] " } else { "[ ] " }
          } else if is_selected {
            "> "
          } else {
            "  "
          };

          let label_style = if is_selected && is_current {
            Style::default().fg(BLUE).add_modifier(Modifier::BOLD)
          } else if is_checked {
            Style::default().fg(GREEN)
          } else {
            Style::default().fg(TEXT_COLOR)
          };

          let mut spans = vec![
            Span::styled(prefix, label_style),
            Span::styled(&opt.label, label_style),
          ];

          if !opt.description.is_empty() {
            spans.push(Span::styled(
              format!(" - {}", opt.description),
              Style::default().fg(TEXT_COLOR),
            ));
          }

          lines.push(Line::from(spans));
        }
      }

      // Spacing between questions
      if q_idx + 1 < pq.questions.len() {
        lines.push(Line::from(""));
      }
    }

    // Hint line
    if let Some(q) = pq.questions.get(pq.current_question_idx) {
      let hint = if q.confirmation {
        "[y] yes  [n] no  [q] dismiss"
      } else if q.multi_select {
        "[Space] toggle  [Enter] confirm  [q] dismiss"
      } else {
        "[Enter] confirm  [1-4] quick select  [q] dismiss"
      };
      lines.push(Line::from(vec![Span::styled(
        hint,
        Style::default().fg(TEXT_COLOR),
      )]));
    }

    let text = Text::from(lines);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
    f.render_widget(paragraph, inner);
  }
}
