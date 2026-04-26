//! Diff panel rendering for file modification previews.
//!
//! Parses unified diff text and renders it as a bordered panel with
//! line numbers, background colors, and inline markers — matching
//! kimi-cli's diff display style.

use ratatui::{
  Frame,
  layout::Constraint,
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Cell, Row, Table},
};

/// Background color for added lines (dark green).
const DIFF_ADD_BG: Color = Color::Rgb(0x12, 0x26, 0x1e);
/// Background color for deleted lines (dark red).
const DIFF_DEL_BG: Color = Color::Rgb(0x2d, 0x12, 0x14);

/// A single line in a diff hunk.
#[derive(Debug, Clone)]
struct DiffLine {
  kind: DiffLineKind,
  old_num: Option<usize>,
  new_num: Option<usize>,
  content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLineKind {
  Context,
  Add,
  Delete,
}

/// A hunk of consecutive diff lines.
#[derive(Debug, Clone)]
struct DiffHunk {
  lines: Vec<DiffLine>,
}

/// A parsed diff for a single file.
#[derive(Debug, Clone)]
pub(crate) struct ParsedDiff {
  path: String,
  added: usize,
  removed: usize,
  hunks: Vec<DiffHunk>,
}

/// Parse unified diff text into structured diff data.
///
/// Supports:
/// - Standard unified diff from the `similar` crate (`--- a/...`, `+++ b/...`, `@@ ... @@`)
/// - "New file: path" preview format
pub fn parse_unified_diff(text: &str) -> Vec<ParsedDiff> {
  let mut result = Vec::new();

  // Handle "New file: path" format
  if let Some(first) = text.lines().next()
    && let Some(path) = first.strip_prefix("New file: ")
  {
    let mut diff_lines = Vec::new();
    for (idx, line) in text.lines().skip(2).enumerate() {
      diff_lines.push(DiffLine {
        kind: DiffLineKind::Add,
        old_num: None,
        new_num: Some(idx + 1),
        content: line.to_string(),
      });
    }
    if !diff_lines.is_empty() {
      result.push(ParsedDiff {
        path: path.to_string(),
        added: diff_lines.len(),
        removed: 0,
        hunks: vec![DiffHunk { lines: diff_lines }],
      });
    }
    return result;
  }

  // Standard unified diff format
  let lines: Vec<&str> = text.lines().collect();
  let mut i = 0;

  while i < lines.len() {
    // Look for file header
    let path = if let Some(p) = lines[i].strip_prefix("--- a/") {
      i += 1;
      if i < lines.len() && lines[i].starts_with("+++ b/") {
        i += 1;
      }
      p.to_string()
    } else if lines[i].starts_with("--- ") {
      // Could be "--- /dev/null" for new files
      i += 1;
      if i < lines.len() && lines[i].starts_with("+++ b/") {
        let path = lines[i].strip_prefix("+++ b/").unwrap_or("").to_string();
        i += 1;
        path
      } else {
        continue;
      }
    } else {
      i += 1;
      continue;
    };

    let mut hunks = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;

    while i < lines.len() {
      if lines[i].starts_with("--- ") || lines[i].starts_with("Diff: ") {
        break;
      }

      if let Some((old_start, old_count, new_start, new_count)) = parse_hunk_header(lines[i]) {
        i += 1;
        let mut hunk_lines = Vec::new();
        let mut old_num = old_start;
        let mut new_num = new_start;
        let mut old_seen = 0usize;
        let mut new_seen = 0usize;

        while i < lines.len() {
          let line = lines[i];
          if line.starts_with("@@ ") || line.starts_with("--- ") || line.starts_with("Diff: ") {
            break;
          }

          if line.is_empty() {
            i += 1;
            continue;
          }

          let first_char = line.chars().next().unwrap();
          match first_char {
            ' ' => {
              let content = if line.len() > 1 { &line[1..] } else { "" };
              hunk_lines.push(DiffLine {
                kind: DiffLineKind::Context,
                old_num: Some(old_num),
                new_num: Some(new_num),
                content: content.to_string(),
              });
              old_num += 1;
              new_num += 1;
              old_seen += 1;
              new_seen += 1;
            }
            '+' => {
              let content = if line.len() > 1 { &line[1..] } else { "" };
              hunk_lines.push(DiffLine {
                kind: DiffLineKind::Add,
                old_num: None,
                new_num: Some(new_num),
                content: content.to_string(),
              });
              new_num += 1;
              new_seen += 1;
              added += 1;
            }
            '-' => {
              let content = if line.len() > 1 { &line[1..] } else { "" };
              hunk_lines.push(DiffLine {
                kind: DiffLineKind::Delete,
                old_num: Some(old_num),
                new_num: None,
                content: content.to_string(),
              });
              old_num += 1;
              old_seen += 1;
              removed += 1;
            }
            '\\' => {
              // "\ No newline at end of file" — skip
            }
            _ => break,
          }
          i += 1;

          if old_seen >= old_count && new_seen >= new_count {
            break;
          }
        }

        if !hunk_lines.is_empty() {
          hunks.push(DiffHunk { lines: hunk_lines });
        }
      } else {
        i += 1;
      }
    }

    if !hunks.is_empty() {
      result.push(ParsedDiff {
        path,
        added,
        removed,
        hunks,
      });
    }
  }

  result
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
  if !line.starts_with("@@ ") || !line.ends_with(" @@") {
    return None;
  }

  let inner = &line[3..line.len() - 3];
  let parts: Vec<&str> = inner.split_whitespace().collect();
  if parts.len() != 2 {
    return None;
  }

  let (old_start, old_count) = parse_range(parts[0])?;
  let (new_start, new_count) = parse_range(parts[1])?;
  Some((old_start, old_count, new_start, new_count))
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
  let s = s.strip_prefix('-').or_else(|| s.strip_prefix('+'))?;
  if let Some((start, count)) = s.split_once(',') {
    Some((start.parse().ok()?, count.parse().ok()?))
  } else {
    Some((s.parse().ok()?, 1))
  }
}

/// Calculate the number of lines needed to render a diff panel.
pub fn diff_render_height(diff_text: &str) -> usize {
  let diffs = parse_unified_diff(diff_text);
  if diffs.is_empty() {
    return 0;
  }

  let mut total = 0;
  for diff in &diffs {
    let content_lines: usize = diff.hunks.iter().map(|h| h.lines.len()).sum();
    let separators = diff.hunks.len().saturating_sub(1);
    total += 2 + content_lines + separators; // top/bottom border + content
  }

  // Add gap between multiple file diffs
  if diffs.len() > 1 {
    total += diffs.len() - 1;
  }

  total
}

/// Render a diff panel into the given area.
pub fn render_diff_panel(f: &mut Frame, area: ratatui::layout::Rect, diff_text: &str) {
  let diffs = parse_unified_diff(diff_text);
  if diffs.is_empty() {
    return;
  }

  // For now, render only the first diff if multiple
  let diff = &diffs[0];

  // Build title: +N -M path
  let mut title_spans = Vec::new();
  if diff.added > 0 {
    title_spans.push(Span::styled(
      format!("+{} ", diff.added),
      Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD),
    ));
  }
  if diff.removed > 0 {
    title_spans.push(Span::styled(
      format!("-{} ", diff.removed),
      Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    ));
  }
  title_spans.push(Span::raw(&diff.path));
  let title = Line::from(title_spans);

  // Compute max line number width
  let mut max_ln = 0usize;
  for hunk in &diff.hunks {
    for line in &hunk.lines {
      if let Some(n) = line.old_num {
        max_ln = max_ln.max(n);
      }
      if let Some(n) = line.new_num {
        max_ln = max_ln.max(n);
      }
    }
  }
  let num_width = max_ln.to_string().len().max(2);

  // Build table rows
  let mut rows = Vec::new();
  for (hunk_idx, hunk) in diff.hunks.iter().enumerate() {
    if hunk_idx > 0 {
      rows.push(
        Row::new(vec![Cell::from("⋮"), Cell::from(""), Cell::from("")])
          .style(Style::default().fg(Color::DarkGray)),
      );
    }

    for line in &hunk.lines {
      let (num_text, marker, bg, num_style): (_, _, _, _) = match line.kind {
        DiffLineKind::Add => (
          line.new_num.map(|n| n.to_string()).unwrap_or_default(),
          Span::styled(" + ", Style::default().fg(Color::Green)),
          DIFF_ADD_BG,
          Style::default(),
        ),
        DiffLineKind::Delete => (
          line.old_num.map(|n| n.to_string()).unwrap_or_default(),
          Span::styled(" - ", Style::default().fg(Color::Red)),
          DIFF_DEL_BG,
          Style::default(),
        ),
        DiffLineKind::Context => (
          line.new_num.map(|n| n.to_string()).unwrap_or_default(),
          Span::raw("   "),
          Color::Reset,
          Style::default().fg(Color::DarkGray),
        ),
      };

      rows.push(
        Row::new(vec![
          Cell::from(Span::styled(num_text, num_style)),
          Cell::from(marker),
          Cell::from(line.content.as_str()),
        ])
        .style(Style::default().bg(bg)),
      );
    }
  }

  let table = Table::new(
    rows,
    [
      Constraint::Length(num_width as u16),
      Constraint::Length(3),
      Constraint::Min(0),
    ],
  );

  let block = Block::default()
    .borders(Borders::ALL)
    .title(title)
    .border_style(Style::default().fg(Color::DarkGray));

  let table_with_block = table.block(block);
  f.render_widget(table_with_block, area);
}

/// Calculate the height of a compact preview for the approval panel.
/// Handles both unified diffs and plain text (e.g. shell command previews).
pub fn diff_preview_compact_height(preview_text: &str) -> usize {
  let diffs = parse_unified_diff(preview_text);
  if !diffs.is_empty() {
    let diff = &diffs[0];
    let changed: usize = diff
      .hunks
      .iter()
      .map(|h| {
        h.lines
          .iter()
          .filter(|l| l.kind != DiffLineKind::Context)
          .count()
      })
      .sum();
    if changed == 0 {
      return 0;
    }
    return 1 + changed; // header line + changed lines
  }

  // Fallback: plain text preview (e.g. shell command)
  let lines = preview_text.lines().count();
  if lines == 0 {
    return 0;
  }
  1 + lines.min(8) // title line + up to 8 lines of content
}

/// Render a compact preview for the approval panel.
/// Handles both unified diffs and plain text (e.g. shell command previews).
pub fn render_diff_preview_compact(f: &mut Frame, area: ratatui::layout::Rect, preview_text: &str) {
  let diffs = parse_unified_diff(preview_text);
  if !diffs.is_empty() {
    render_diff_compact(f, area, &diffs[0]);
    return;
  }

  // Fallback: render plain text preview as a code block (e.g. shell command)
  render_plain_preview_compact(f, area, preview_text);
}

fn render_diff_compact(f: &mut Frame, area: ratatui::layout::Rect, diff: &ParsedDiff) {
  // Build header: +N -M path
  let mut header_spans = Vec::new();
  if diff.added > 0 {
    header_spans.push(Span::styled(
      format!("+{} ", diff.added),
      Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD),
    ));
  }
  if diff.removed > 0 {
    header_spans.push(Span::styled(
      format!("-{} ", diff.removed),
      Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    ));
  }
  header_spans.push(Span::raw(&diff.path));

  let mut text_lines = vec![Line::from(header_spans)];

  // Collect only changed lines
  let mut changed = Vec::new();
  for hunk in &diff.hunks {
    for line in &hunk.lines {
      if line.kind != DiffLineKind::Context {
        changed.push(line);
      }
    }
  }

  // Compute line number width from shown lines
  let max_ln = changed
    .iter()
    .map(|dl| dl.old_num.unwrap_or(0).max(dl.new_num.unwrap_or(0)))
    .max()
    .unwrap_or(0);
  let num_width = max_ln.to_string().len().max(2);

  for line in &changed {
    let (ln, marker_style, marker_char, content, bg) = match line.kind {
      DiffLineKind::Add => (
        line.new_num.unwrap_or(0),
        Style::default().fg(Color::Green),
        '+',
        line.content.as_str(),
        DIFF_ADD_BG,
      ),
      DiffLineKind::Delete => (
        line.old_num.unwrap_or(0),
        Style::default().fg(Color::Red),
        '-',
        line.content.as_str(),
        DIFF_DEL_BG,
      ),
      DiffLineKind::Context => continue,
    };

    let num_str = format!("{:>width$}", ln, width = num_width);
    text_lines.push(Line::from(vec![
      Span::styled(num_str, Style::default().fg(Color::DarkGray)),
      Span::styled(format!(" {marker_char} "), marker_style),
      Span::styled(content, Style::default().bg(bg)),
    ]));
  }

  let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Text::from(text_lines));
  f.render_widget(paragraph, area);
}

fn render_plain_preview_compact(f: &mut Frame, area: ratatui::layout::Rect, text: &str) {
  use ratatui::widgets::Block;

  let mut lines = Vec::new();
  for line in text.lines().take(8) {
    lines.push(Line::from(vec![
      Span::styled("$ ", Style::default().fg(Color::DarkGray)),
      Span::styled(line, Style::default().fg(Color::White)),
    ]));
  }

  let block = Block::default()
    .borders(ratatui::widgets::Borders::LEFT)
    .border_style(Style::default().fg(Color::DarkGray));

  let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Text::from(lines)).block(block);
  f.render_widget(paragraph, area);
}
