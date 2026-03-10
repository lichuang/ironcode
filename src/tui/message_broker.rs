//! Message broker for UI communication.
//!
//! Provides a channel-based message passing system for background tasks
//! to send messages to the UI thread.

use tokio::sync::mpsc;

use super::message::UiMessage;

/// A message broker that bridges background tasks and the UI thread.
///
/// Clone the `Sender` to pass to background tasks, and use `try_recv`
/// in the main event loop to process messages.
#[derive(Debug)]
pub struct MessageBroker {
  tx: mpsc::UnboundedSender<UiMessage>,
  rx: mpsc::UnboundedReceiver<UiMessage>,
}

impl MessageBroker {
  /// Create a new message broker with an unbounded channel.
  pub fn new() -> Self {
    let (tx, rx) = mpsc::unbounded_channel();
    Self { tx, rx }
  }

  /// Get a clone of the sender handle.
  ///
  /// This can be passed to background tasks to send messages to the UI.
  pub fn sender(&self) -> mpsc::UnboundedSender<UiMessage> {
    self.tx.clone()
  }

  /// Try to receive a message without blocking.
  ///
  /// Returns `Some(message)` if a message is available, `None` otherwise.
  /// This should be called in the main event loop to drain the queue.
  pub fn try_recv(&mut self) -> Option<UiMessage> {
    self.rx.try_recv().ok()
  }
}

impl Default for MessageBroker {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_message_broker_send_recv() {
    let mut broker = MessageBroker::new();
    let sender = broker.sender();

    // Send a message
    let msg = UiMessage::AppendChat {
      content: "Hello".to_string(),
    };
    sender.send(msg.clone()).unwrap();

    // Receive the message
    let received = broker.try_recv();
    assert!(received.is_some());
    match received.unwrap() {
      UiMessage::AppendChat { content } => assert_eq!(content, "Hello"),
    }
  }

  #[test]
  fn test_message_broker_empty() {
    let mut broker = MessageBroker::new();
    assert!(broker.try_recv().is_none());
  }
}
