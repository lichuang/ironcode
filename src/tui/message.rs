//! UI Message System
//!
//! Provides a message passing mechanism for background tasks to communicate
//! with the UI thread. Messages are processed in the main event loop.

/// Messages that can be sent to the UI thread from background tasks.
#[derive(Debug, Clone)]
pub enum UiMessage {
  /// Append a new chat message
  AppendChat { content: String },
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_ui_message_clone() {
    let msg = UiMessage::AppendChat {
      content: "test".to_string(),
    };
    let cloned = msg.clone();
    match cloned {
      UiMessage::AppendChat { content } => assert_eq!(content, "test"),
    }
  }
}
