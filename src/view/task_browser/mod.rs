//! Task browser view — interactive background task manager.
//!
//! Opened via the `/task` slash command. Shows a three-column layout:
//! left = task list, middle = task details, right = output preview.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Modifier, Style},
  text::{Line, Span, Text},
  widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::background::models::is_terminal_status;
use crate::background::{BackgroundTaskManager, TaskView};
use crate::cli::AppData;
use crate::cli::runtime::Runtime;
use crate::llm::SessionHandle;
use crate::tui::FrameRequester;
use crate::utils::colors::{CRITICAL, GREEN, MUTED, PRIMARY, SUBTLE, TEXT, WARNING};
use crate::view::{ChatView, View};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const OUTPUT_PREVIEW_LINES: usize = 6;
const OUTPUT_PREVIEW_BYTES: usize = 4 * 1024;
const OUTPUT_FULL_LINES: usize = 4000;
const OUTPUT_FULL_BYTES: usize = 200 * 1024;

pub struct TaskBrowserView {
  manager: Arc<BackgroundTaskManager>,
  session_handle: SessionHandle,
  runtime: Arc<Runtime>,
  tasks: Vec<TaskView>,
  selected_idx: usize,
  active_only: bool,
  last_refresh: Instant,
  status_message: Option<String>,
  frame_requester: Option<FrameRequester>,
  stop_confirming: bool,
}

impl TaskBrowserView {
  pub fn new(
    manager: Arc<BackgroundTaskManager>,
    session_handle: SessionHandle,
    runtime: Arc<Runtime>,
  ) -> Self {
    let tasks = Self::fetch_tasks(&manager, false);
    Self {
      manager,
      session_handle,
      runtime,
      tasks,
      selected_idx: 0,
      active_only: false,
      last_refresh: Instant::now(),
      status_message: None,
      frame_requester: None,
      stop_confirming: false,
    }
  }

  fn fetch_tasks(manager: &BackgroundTaskManager, active_only: bool) -> Vec<TaskView> {
    // Reconcile on-disk state before listing (marks stale tasks as lost)
    manager.recover();
    match manager.list_tasks(active_only, 100) {
      Ok(mut views) => {
        // Sort: non-terminal first (by creation time), then terminal (by finish time, newest first)
        views.sort_by(|a, b| {
          let a_terminal = is_terminal_status(a.runtime.status);
          let b_terminal = is_terminal_status(b.runtime.status);
          match (a_terminal, b_terminal) {
            (false, true) => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
            (false, false) => {
              // Non-terminal: by creation time (newest first)
              b.spec.created_at.cmp(&a.spec.created_at)
            }
            (true, true) => {
              // Terminal: by finish time (newest first)
              let a_fin = a.runtime.finished_at.unwrap_or(0);
              let b_fin = b.runtime.finished_at.unwrap_or(0);
              b_fin.cmp(&a_fin)
            }
          }
        });
        views
      }
      Err(_) => Vec::new(),
    }
  }

  fn refresh(&mut self) {
    // Reconcile terminal tasks and publish notifications before fetching
    self.manager.reconcile(&self.runtime.notification_manager());
    self.tasks = Self::fetch_tasks(&self.manager, self.active_only);
    if self.selected_idx >= self.tasks.len() && !self.tasks.is_empty() {
      self.selected_idx = self.tasks.len() - 1;
    }
    self.last_refresh = Instant::now();
  }

  fn selected_task(&self) -> Option<&TaskView> {
    self.tasks.get(self.selected_idx)
  }

  fn status_counts(&self) -> (usize, usize, usize, usize, usize, usize) {
    let mut running = 0;
    let mut starting = 0;
    let mut completed = 0;
    let mut failed = 0;
    let mut killed = 0;
    let mut lost = 0;
    for t in &self.tasks {
      match t.runtime.status {
        crate::background::TaskStatus::Running => running += 1,
        crate::background::TaskStatus::Starting => starting += 1,
        crate::background::TaskStatus::Completed => completed += 1,
        crate::background::TaskStatus::Failed => failed += 1,
        crate::background::TaskStatus::Killed => killed += 1,
        crate::background::TaskStatus::Lost => lost += 1,
        _ => {}
      }
    }
    (running, starting, completed, failed, killed, lost)
  }

  fn format_duration(secs: u64) -> String {
    if secs < 60 {
      format!("{}s", secs)
    } else if secs < 3600 {
      format!("{}m {}s", secs / 60, secs % 60)
    } else {
      format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
  }
}

impl View for TaskBrowserView {
  fn handle_key(&mut self, _data: &mut AppData, key: KeyEvent) -> Option<Box<dyn View>> {
    // Stop confirmation mode
    if self.stop_confirming {
      match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
          if let Some(task) = self.selected_task() {
            let _ = self
              .manager
              .kill(&task.spec.id, "Stopped from task browser");
            self.status_message = Some(format!("Stop requested for {}", task.spec.id));
          }
          self.stop_confirming = false;
          self.refresh();
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
          self.stop_confirming = false;
          self.status_message = Some("Stop cancelled".to_string());
        }
        _ => {}
      }
      return None;
    }

    // Normal list mode
    match key.code {
      KeyCode::Char('q') | KeyCode::Esc => {
        return Some(Box::new(ChatView::new(
          _data,
          self.session_handle.clone(),
          self.runtime.clone(),
        )));
      }
      KeyCode::Up => {
        if self.selected_idx > 0 {
          self.selected_idx -= 1;
        }
      }
      KeyCode::Down => {
        if self.selected_idx + 1 < self.tasks.len() {
          self.selected_idx += 1;
        }
      }
      KeyCode::Char('r') | KeyCode::Char('R') => {
        self.refresh();
        self.status_message = Some("Refreshed".to_string());
      }
      KeyCode::Char('\t') => {
        self.active_only = !self.active_only;
        self.refresh();
        self.selected_idx = 0;
        self.status_message = Some(if self.active_only {
          "Filter: active only".to_string()
        } else {
          "Filter: all tasks".to_string()
        });
      }
      KeyCode::Char('s') | KeyCode::Char('S') => {
        if let Some(task) = self.selected_task() {
          if is_terminal_status(task.runtime.status) {
            self.status_message = Some(format!("{} is already terminal", task.spec.id));
          } else {
            self.stop_confirming = true;
          }
        }
      }
      KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('O') => {
        if let Some(task) = self.selected_task() {
          let output = self
            .manager
            .tail_output(&task.spec.id, OUTPUT_FULL_BYTES, OUTPUT_FULL_LINES)
            .unwrap_or_default();
          let header = format!(
            "=== Task: {} | Status: {:?} | Description: {} ===\n\n",
            task.spec.id, task.runtime.status, task.spec.description
          );
          _data.pending_pager_output = Some(header + &output);
        }
      }
      _ => {}
    }
    None
  }

  fn draw(&mut self, f: &mut Frame, _data: &AppData) {
    let area = f.area();

    // Title bar + main + footer
    let chunks = Layout::default()
      .direction(Direction::Vertical)
      .constraints([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
      ])
      .split(area);

    self.draw_header(f, chunks[0]);
    self.draw_main(f, chunks[1]);
    self.draw_footer(f, chunks[2]);
  }

  fn on_frame(&mut self, frame_requester: &FrameRequester, _data: &AppData) {
    self.frame_requester = Some(frame_requester.clone());
    if self.last_refresh.elapsed() >= REFRESH_INTERVAL {
      self.refresh();
      frame_requester.schedule_frame();
    }
  }

  fn set_frame_requester(&mut self, frame_requester: FrameRequester) {
    self.frame_requester = Some(frame_requester);
  }
}

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

impl TaskBrowserView {
  fn draw_header(&self, f: &mut Frame, area: Rect) {
    let (running, starting, completed, failed, killed, lost) = self.status_counts();
    let filter_label = if self.active_only { "ACTIVE" } else { "ALL" };
    let header = format!(
      " TASK BROWSER  filter={}  {} running  {} starting  {} completed  {} failed  {} killed  {} lost  {} total ",
      filter_label,
      running,
      starting,
      completed,
      failed,
      killed,
      lost,
      self.tasks.len()
    );
    let text = Text::from(Line::from(vec![Span::styled(
      header,
      Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
    )]));
    let widget = Paragraph::new(text);
    f.render_widget(widget, area);
  }

  fn draw_main(&self, f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
      .direction(Direction::Horizontal)
      .constraints([
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(35),
      ])
      .split(area);

    self.draw_task_list(f, chunks[0]);
    self.draw_task_detail(f, chunks[1]);
    self.draw_output_preview(f, chunks[2]);
  }

  fn draw_task_list(&self, f: &mut Frame, area: Rect) {
    let block = Block::default().title("Tasks").borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let items: Vec<ListItem> = self
      .tasks
      .iter()
      .enumerate()
      .map(|(i, view)| {
        let status_str = format!("[{:?}]", view.runtime.status);
        let status_color = match view.runtime.status {
          crate::background::TaskStatus::Running => GREEN,
          crate::background::TaskStatus::Starting => PRIMARY,
          crate::background::TaskStatus::Completed => GREEN,
          crate::background::TaskStatus::Failed => CRITICAL,
          crate::background::TaskStatus::Killed => WARNING,
          crate::background::TaskStatus::Lost => CRITICAL,
          _ => MUTED,
        };
        let desc = if view.spec.description.len() > 20 {
          format!("{}...", &view.spec.description[..20])
        } else {
          view.spec.description.clone()
        };
        let line = Line::from(vec![
          Span::styled(status_str, Style::default().fg(status_color)),
          Span::styled(
            format!(" {} · {}", desc, view.spec.id),
            Style::default().fg(TEXT),
          ),
        ]);
        let mut item = ListItem::new(line);
        if i == self.selected_idx {
          item = item.style(Style::default().add_modifier(Modifier::BOLD).bg(SUBTLE));
        }
        item
      })
      .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
  }

  fn draw_task_detail(&self, f: &mut Frame, area: Rect) {
    let block = Block::default().title("Details").borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let text = if let Some(view) = self.selected_task() {
      let (command, shell_name, shell_path, cwd) = match view.spec.bash_params() {
        Some(p) => (
          p.command.as_str(),
          p.shell_name.as_str(),
          p.shell_path.as_str(),
          p.cwd.as_str(),
        ),
        None => ("N/A", "N/A", "N/A", "N/A"),
      };

      let mut lines = vec![
        Line::from(vec![Span::styled("Task ID: ", Style::default().fg(MUTED))]),
        Line::from(vec![Span::styled(&view.spec.id, Style::default().fg(TEXT))]),
        Line::from(""),
        Line::from(vec![Span::styled("Status: ", Style::default().fg(MUTED))]),
        Line::from(vec![Span::styled(
          format!("{:?}", view.runtime.status),
          Style::default().fg(TEXT),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
          "Terminal Reason: ",
          Style::default().fg(MUTED),
        )]),
        Line::from(vec![Span::styled(
          if view.runtime.timed_out {
            "timed_out".to_string()
          } else {
            format!("{:?}", view.runtime.status)
          },
          Style::default().fg(TEXT),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
          "Description: ",
          Style::default().fg(MUTED),
        )]),
        Line::from(vec![Span::styled(
          &view.spec.description,
          Style::default().fg(TEXT),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled("Kind: ", Style::default().fg(MUTED))]),
        Line::from(vec![Span::styled(
          view.spec.kind.as_str(),
          Style::default().fg(TEXT),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled("Command: ", Style::default().fg(MUTED))]),
        Line::from(vec![Span::styled(command, Style::default().fg(TEXT))]),
        Line::from(""),
        Line::from(vec![Span::styled("Shell: ", Style::default().fg(MUTED))]),
        Line::from(vec![Span::styled(
          format!("{} ({})", shell_name, shell_path),
          Style::default().fg(TEXT),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled("CWD: ", Style::default().fg(MUTED))]),
        Line::from(vec![Span::styled(cwd, Style::default().fg(TEXT))]),
      ];

      if let Some(started) = view.runtime.started_at {
        let now = std::time::SystemTime::now()
          .duration_since(std::time::UNIX_EPOCH)
          .unwrap_or_default()
          .as_secs();
        let elapsed = now - started;
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
          "Duration: ",
          Style::default().fg(MUTED),
        )]));
        lines.push(Line::from(vec![Span::styled(
          Self::format_duration(elapsed),
          Style::default().fg(TEXT),
        )]));
      }

      if let Some(exit_code) = view.runtime.exit_code {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
          "Exit Code: ",
          Style::default().fg(MUTED),
        )]));
        lines.push(Line::from(vec![Span::styled(
          exit_code.to_string(),
          Style::default().fg(TEXT),
        )]));
      }

      if let Some(ref reason) = view.runtime.failure_reason {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
          "Failure Reason: ",
          Style::default().fg(MUTED),
        )]));
        lines.push(Line::from(vec![Span::styled(
          reason.clone(),
          Style::default().fg(CRITICAL),
        )]));
      }

      Text::from(lines)
    } else {
      Text::from("[no task selected]")
    };

    let widget = Paragraph::new(text).wrap(Wrap { trim: true });
    f.render_widget(widget, inner);
  }

  fn draw_output_preview(&self, f: &mut Frame, area: Rect) {
    let block = Block::default()
      .title("Output Preview")
      .borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let text = if let Some(view) = self.selected_task() {
      let output = self
        .manager
        .tail_output(&view.spec.id, OUTPUT_PREVIEW_BYTES, OUTPUT_PREVIEW_LINES)
        .unwrap_or_default();
      if output.is_empty() {
        Text::from("[no output]")
      } else {
        Text::from(output)
      }
    } else {
      Text::from("[no task selected]")
    };

    let widget = Paragraph::new(text).wrap(Wrap { trim: true });
    f.render_widget(widget, inner);
  }

  fn draw_footer(&self, f: &mut Frame, area: Rect) {
    let mut spans = vec![];

    if self.stop_confirming {
      spans.push(Span::styled(
        "Confirm stop? [Y] confirm  [N] cancel",
        Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
      ));
    } else {
      spans.extend([
        Span::styled("[↑/↓] ", Style::default().fg(MUTED)),
        Span::styled("navigate  ", Style::default().fg(TEXT)),
        Span::styled("[Enter/O] ", Style::default().fg(MUTED)),
        Span::styled("output  ", Style::default().fg(TEXT)),
        Span::styled("[S] ", Style::default().fg(MUTED)),
        Span::styled("stop  ", Style::default().fg(TEXT)),
        Span::styled("[Tab] ", Style::default().fg(MUTED)),
        Span::styled("filter  ", Style::default().fg(TEXT)),
        Span::styled("[R] ", Style::default().fg(MUTED)),
        Span::styled("refresh  ", Style::default().fg(TEXT)),
        Span::styled("[Q] ", Style::default().fg(MUTED)),
        Span::styled("exit", Style::default().fg(TEXT)),
      ]);
    }

    if let Some(ref msg) = self.status_message {
      spans.push(Span::styled(
        format!("  |  {}", msg),
        Style::default().fg(WARNING),
      ));
    }

    let text = Text::from(Line::from(spans));
    let widget = Paragraph::new(text);
    f.render_widget(widget, area);
  }
}
