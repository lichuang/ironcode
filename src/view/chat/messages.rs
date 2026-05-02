//! MessageListComponent — rendering helpers for chat messages, animations, and streaming content.

use ratatui::{
  Frame,
  layout::Rect,
  symbols::border,
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph},
};

use crate::llm::types::Message;
use crate::utils::char_display_width;
use crate::utils::colors::{
  BLUE, CRITICAL, GREEN, HIGHLIGHT as HIGHLIGHT_COLOR, TEXT as TEXT_COLOR, WARNING,
};
use crate::utils::{HIGHLIGHT, PRIMARY_BORDER, THINKING};
use crate::view::diff::render_diff_panel;

/// Status of a tool call
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
  /// Tool call is being executed
  Running,
  /// Tool call completed successfully
  Completed,
  #[allow(dead_code)]
  /// Tool call failed
  Failed,
}

/// A chunk of streaming content from LLM
#[derive(Debug, Clone)]
pub enum StreamingChunk {
  /// Normal response content
  Normal(String),
  /// Thinking/reasoning content
  Thinking(String),
  /// Tool call indicator
  ToolCall {
    /// Tool name
    name: String,
    /// Tool arguments (JSON)
    arguments: String,
    /// Current status
    status: ToolCallStatus,
  },
}

#[allow(dead_code)]
impl StreamingChunk {
  /// Get the content of the chunk
  pub fn content(&self) -> &str {
    match self {
      StreamingChunk::Normal(s) => s,
      StreamingChunk::Thinking(s) => s,
      StreamingChunk::ToolCall { .. } => "",
    }
  }

  /// Check if this is a thinking chunk
  pub fn is_thinking(&self) -> bool {
    matches!(self, StreamingChunk::Thinking(_))
  }

  #[allow(dead_code)]
  /// Check if this is a tool call chunk
  pub fn is_tool_call(&self) -> bool {
    matches!(self, StreamingChunk::ToolCall { .. })
  }
}

/// A message in the chat history
#[derive(Debug, Clone)]
pub enum ChatMessage {
  /// User message
  User { content: String },
  /// AI assistant response (with optional thinking content)
  Assistant {
    /// The main response content
    content: String,
    /// The thinking/reasoning content (if any)
    thinking_content: Option<String>,
  },
  /// Tool call result
  ToolCall {
    /// Tool name
    name: String,
    /// Tool arguments (JSON)
    arguments: String,
    /// Tool execution output (e.g., diff or success message)
    output: Option<String>,
  },
  /// System notification (e.g., compaction warning)
  System {
    /// The notification content
    content: String,
    /// Notification level (info, warning, error)
    level: SystemMessageLevel,
  },
}

impl ChatMessage {
  /// Check if this is a user message
  pub fn is_user(&self) -> bool {
    matches!(self, ChatMessage::User { .. })
  }
}

/// Level for system messages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemMessageLevel {
  /// Informational message
  Info,
  /// Warning message
  Warning,
  /// Error/Critical message
  Error,
}

/// Convert LLM messages into UI chat history.
///
/// This is used when resuming a persisted session to rebuild the visual chat log.
pub fn llm_messages_to_chat_history(messages: &[Message]) -> Vec<ChatMessage> {
  use crate::llm::types::Role;

  let mut history = Vec::new();
  for msg in messages {
    match msg.role {
      Role::System => {}
      Role::User => {
        history.push(ChatMessage::User {
          content: msg.content.clone(),
        });
      }
      Role::Assistant => {
        let content = msg.content.clone();
        let (thinking_content, content) = if let Some(start) = content.find("<think>")
          && let Some(end) = content.find("</think>")
        {
          let think = content[start + 7..end].to_string();
          let after = content[end + 8..].to_string();
          (Some(think), after)
        } else {
          (None, content)
        };
        if !content.is_empty() || thinking_content.is_some() {
          history.push(ChatMessage::Assistant {
            content,
            thinking_content,
          });
        }
      }
      Role::Tool => {
        // Tool results are not rendered directly in the chat history;
        // the corresponding tool call indicator is added via ToolCallCompleted events.
      }
    }
  }
  history
}

/// Wrap text into lines based on available width.
pub fn wrap_text(text: &str, available_width: u16) -> Vec<String> {
  if available_width == 0 {
    return vec![text.to_string()];
  }
  let available = available_width as usize;
  let mut lines: Vec<String> = vec![];
  let mut current_line = String::new();
  let mut current_width = 0;

  for c in text.chars() {
    let char_width = char_display_width(c);

    if c == '\n' {
      lines.push(current_line);
      current_line = String::new();
      current_width = 0;
    } else if current_width + char_width > available {
      lines.push(current_line);
      current_line = c.to_string();
      current_width = char_width;
    } else {
      current_line.push(c);
      current_width += char_width;
    }
  }

  if !current_line.is_empty() {
    lines.push(current_line);
  }

  lines
}

/// Calculate the number of lines needed to display text with given width.
pub fn calculate_line_count(text: &str, available_width: u16) -> usize {
  wrap_text(text, available_width).len().max(1)
}

/// Calculate the number of lines needed to display text with prefix and given width.
pub fn calculate_line_count_with_prefix(
  text: &str,
  prefix_width: usize,
  available_width: u16,
) -> usize {
  if available_width == 0 {
    return 1;
  }
  let available = available_width as usize;
  let mut lines = 1;
  let mut current_width = prefix_width;

  for c in text.chars() {
    let char_width = char_display_width(c);

    if c == '\n' {
      lines += 1;
      current_width = 0;
    } else if current_width + char_width > available {
      lines += 1;
      current_width = char_width;
    } else {
      current_width += char_width;
    }
  }

  lines
}

/// Render a message in a box.
pub fn render_message_box(f: &mut Frame, area: Rect, message: &str) {
  let block = Block::default()
    .borders(Borders::ALL)
    .border_set(border::ROUNDED)
    .border_style(*PRIMARY_BORDER);

  let inner_area = block.inner(area);

  // Render the border block
  f.render_widget(block, area);

  // Manually wrap text to ensure consistency with line count calculation
  let inner_width = inner_area.width;
  let wrapped_lines = wrap_text(message, inner_width);

  // Convert to Lines for rendering
  let lines: Vec<Line> = wrapped_lines.into_iter().map(Line::from).collect();

  let text = Paragraph::new(Text::from(lines));
  f.render_widget(text, inner_area);
}

/// Render the moon animation.
pub fn render_moon_animation(f: &mut Frame, area: Rect, moon: char) {
  let text = Text::from(vec![Line::from(vec![
    Span::raw("  "),
    Span::styled(moon.to_string(), *HIGHLIGHT),
  ])]);

  let widget = Paragraph::new(text);
  f.render_widget(widget, area);
}

/// Render the thinking indicator ("Thinking..." with spinner).
pub fn render_thinking_indicator(f: &mut Frame, area: Rect, spinner: char) {
  let text = Text::from(vec![Line::from(vec![
    Span::raw("  "),
    Span::styled(spinner.to_string(), *THINKING),
    Span::raw(" "),
    Span::styled("Thinking...", *THINKING),
  ])]);

  let widget = Paragraph::new(text);
  f.render_widget(widget, area);
}

/// Render AI response as plain text (without box).
pub fn render_ai_response(f: &mut Frame, area: Rect, response: &str) {
  let wrapped_lines = wrap_text(response, area.width);
  let lines: Vec<Line> = wrapped_lines.into_iter().map(Line::from).collect();
  let text = Paragraph::new(Text::from(lines));
  f.render_widget(text, area);
}

/// Render thinking content with grey italic style.
pub fn render_thinking_content(f: &mut Frame, area: Rect, content: &str) {
  let wrapped_lines = wrap_text(content, area.width);
  let lines: Vec<Line> = wrapped_lines
    .into_iter()
    .map(|line| Line::from(vec![Span::styled(line, *THINKING)]))
    .collect();
  let text = Paragraph::new(Text::from(lines));
  f.render_widget(text, area);
}

/// Render a tool call message.
pub fn render_tool_call(
  f: &mut Frame,
  area: Rect,
  name: &str,
  arguments: &str,
  output: Option<&str>,
) {
  use ratatui::style::{Modifier, Style};

  if let Some(out) = output {
    let layout = ratatui::layout::Layout::default()
      .direction(ratatui::layout::Direction::Vertical)
      .constraints([
        ratatui::layout::Constraint::Length(1),
        ratatui::layout::Constraint::Min(0),
      ])
      .split(area);

    // Render title line
    let title_text = Text::from(vec![Line::from(vec![
      Span::styled(
        "• ",
        Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
      ),
      Span::styled("Used ", Style::default().fg(TEXT_COLOR)),
      Span::styled(name, Style::default().fg(BLUE).add_modifier(Modifier::BOLD)),
      Span::styled("(", Style::default().fg(TEXT_COLOR)),
      Span::styled(arguments, Style::default().fg(HIGHLIGHT_COLOR)),
      Span::styled(")", Style::default().fg(TEXT_COLOR)),
    ])]);
    f.render_widget(Paragraph::new(title_text), layout[0]);

    // Render diff panel
    render_diff_panel(f, layout[1], out);
  } else {
    let text = Text::from(vec![Line::from(vec![
      Span::styled(
        "• ",
        Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
      ),
      Span::styled("Used ", Style::default().fg(TEXT_COLOR)),
      Span::styled(name, Style::default().fg(BLUE).add_modifier(Modifier::BOLD)),
      Span::styled("(", Style::default().fg(TEXT_COLOR)),
      Span::styled(arguments, Style::default().fg(HIGHLIGHT_COLOR)),
      Span::styled(")", Style::default().fg(TEXT_COLOR)),
    ])]);
    let widget = Paragraph::new(text);
    f.render_widget(widget, area);
  }
}

/// Render a system notification message.
pub fn render_system_message(f: &mut Frame, area: Rect, content: &str, level: SystemMessageLevel) {
  use ratatui::style::{Modifier, Style};
  let color = match level {
    SystemMessageLevel::Info => HIGHLIGHT_COLOR,
    SystemMessageLevel::Warning => WARNING,
    SystemMessageLevel::Error => CRITICAL,
  };
  let text = Text::from(vec![Line::from(vec![Span::styled(
    content,
    Style::default().fg(color).add_modifier(Modifier::BOLD),
  )])]);
  let widget = Paragraph::new(text).alignment(ratatui::layout::Alignment::Center);
  f.render_widget(widget, area);
}
