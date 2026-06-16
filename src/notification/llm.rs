//! Helpers for delivering notifications into the LLM context.
//!
//! Mirrors kimi-cli's `notifications/llm.py`: notifications are rendered as
//! XML-like blocks so the model can identify them, and notification IDs can be
//! extracted from the conversation history to acknowledge delivery.

use crate::background::BackgroundTaskManager;

use super::models::NotificationView;

/// Build the system/user message text for a notification.
///
/// The format matches kimi-cli:
///
/// ```text
/// <notification id="..." category="..." type="..." source_kind="..." source_id="...">
/// Title: ...
/// Severity: ...
/// {body}
/// <task-notification>
/// ...
/// </task-notification>
/// </notification>
/// ```
pub fn build_notification_message(
  view: &NotificationView,
  manager: Option<&BackgroundTaskManager>,
) -> String {
  let event = &view.event;
  let mut lines = vec![format!(
    "<notification id=\"{}\" category=\"{}\" type=\"{}\" source_kind=\"{}\" source_id=\"{}\">",
    event.id, event.category, event.event_type, event.source_kind, event.source_id
  )];

  lines.push(format!("Title: {}", event.title));
  lines.push(format!("Severity: {}", event.severity));
  lines.push(event.body.clone());

  if event.category == "task"
    && event.source_kind == "background_task"
    && let Some(mgr) = manager
    && let Some(task_view) = mgr.get_task(&event.source_id)
  {
    let max_lines = mgr.config().notification_tail_lines;
    let max_bytes = mgr.config().read_max_bytes;
    let tail = mgr
      .tail_output(&task_view.spec.id, max_bytes, max_lines)
      .unwrap_or_default();

    lines.push("<task-notification>".to_string());
    lines.push(format!("Task ID: {}", task_view.spec.id));
    lines.push(format!("Task Type: {}", task_view.spec.kind));
    lines.push(format!("Description: {}", task_view.spec.description));
    lines.push(format!("Status: {}", task_view.runtime.status));
    if let Some(code) = task_view.runtime.exit_code {
      lines.push(format!("Exit code: {}", code));
    }
    if let Some(ref reason) = task_view.runtime.failure_reason {
      lines.push(format!("Failure reason: {}", reason));
    }
    if !tail.is_empty() {
      lines.push("Output tail:".to_string());
      lines.push(tail);
    }
    lines.push("</task-notification>".to_string());
  }

  lines.push("</notification>".to_string());
  lines.join("\n")
}

/// Extract all notification IDs embedded in a message.
pub fn extract_notification_ids(text: &str) -> Vec<String> {
  let prefix = r#"<notification id=""#;
  let mut ids = Vec::new();
  let mut rest = text;

  while let Some(start) = rest.find(prefix) {
    let after_prefix = &rest[start + prefix.len()..];
    if let Some(end) = after_prefix.find('"') {
      ids.push(after_prefix[..end].to_string());
      rest = &after_prefix[end..];
    } else {
      break;
    }
  }

  ids
}

/// Returns true if the message text looks like a notification block.
pub fn is_notification_message(text: &str) -> bool {
  text.trim_start().starts_with("<notification ")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::notification::models::{
    NotificationDelivery, NotificationEvent, NotificationSeverity, NotificationView,
  };

  fn make_view(id: &str, category: &str, source_id: &str) -> NotificationView {
    NotificationView {
      event: NotificationEvent {
        version: 1,
        id: id.to_string(),
        category: category.to_string(),
        event_type: "task.completed".to_string(),
        source_kind: "background_task".to_string(),
        source_id: source_id.to_string(),
        title: "Test".to_string(),
        body: "Body".to_string(),
        severity: NotificationSeverity::Success,
        created_at: 0.0,
        payload: serde_json::Value::Object(serde_json::Map::new()),
        targets: vec!["llm".to_string()],
        dedupe_key: None,
      },
      delivery: NotificationDelivery::default(),
    }
  }

  #[test]
  fn test_build_notification_message() {
    let view = make_view("n12345678", "task", "task-1");
    let msg = build_notification_message(&view, None);
    assert!(msg.contains("<notification id=\"n12345678\""));
    assert!(msg.contains("category=\"task\""));
    assert!(msg.contains("source_id=\"task-1\""));
    assert!(msg.contains("Title: Test"));
    assert!(msg.contains("Severity: success"));
    assert!(msg.contains("Body"));
    assert!(msg.ends_with("</notification>"));
  }

  #[test]
  fn test_extract_notification_ids() {
    let text = r#"<notification id="nabcdef12" ...>foo</notification>
<notification id="n34567890">bar</notification>"#;
    let ids = extract_notification_ids(text);
    assert_eq!(ids, vec!["nabcdef12", "n34567890"]);
  }

  #[test]
  fn test_is_notification_message() {
    assert!(is_notification_message("<notification id=\"x\">"));
    assert!(!is_notification_message("hello world"));
  }
}
