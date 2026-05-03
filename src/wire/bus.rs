//! WireBus — broadcast channel for WireMessage distribution.

use tokio::sync::broadcast;

use super::protocol::WireMessage;

const MIN_CAPACITY: usize = 1024;

/// Broadcast bus for distributing WireMessages from producers to consumers.
///
/// Backed by `tokio::sync::broadcast` so that multiple subscribers can
/// receive the same message stream (e.g. TUI + logger + future web UI).
pub struct WireBus {
  tx: broadcast::Sender<WireMessage>,
}

impl WireBus {
  /// Default capacity used when creating a new bus.
  pub const DEFAULT_CAPACITY: usize = 4096;

  /// Create a new bus with the given channel capacity.
  ///
  /// When the channel is full, the oldest message is dropped.
  /// Capacity is clamped to at least `MIN_CAPACITY` to reduce message loss under load.
  pub fn new(capacity: usize) -> Self {
    let capacity = capacity.max(MIN_CAPACITY);
    let (tx, _) = broadcast::channel(capacity);
    Self { tx }
  }

  /// Obtain a publisher handle for sending messages.
  pub fn publisher(&self) -> WirePublisher {
    WirePublisher {
      tx: self.tx.clone(),
    }
  }

  /// Obtain a subscriber receiver for consuming messages.
  pub fn subscriber(&self) -> broadcast::Receiver<WireMessage> {
    self.tx.subscribe()
  }
}

/// Cloneable handle for publishing WireMessages.
///
/// Dropping all publishers (and the original WireBus) does not close the
/// channel — subscribers can still drain buffered messages.
#[derive(Clone)]
pub struct WirePublisher {
  tx: broadcast::Sender<WireMessage>,
}

impl WirePublisher {
  /// Send a message to all current subscribers.
  ///
  /// Silently drops the message if there are no active subscribers.
  pub fn send(&self, msg: WireMessage) {
    let _ = self.tx.send(msg);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn test_bus_pub_sub() {
    let bus = WireBus::new(16);
    let pub1 = bus.publisher();
    let mut sub = bus.subscriber();

    pub1.send(WireMessage::TurnBegin);

    let msg = sub.recv().await.unwrap();
    assert!(matches!(msg, WireMessage::TurnBegin));
  }

  #[tokio::test]
  async fn test_multiple_subscribers() {
    let bus = WireBus::new(16);
    let pub1 = bus.publisher();
    let mut sub1 = bus.subscriber();
    let mut sub2 = bus.subscriber();

    pub1.send(WireMessage::TurnEnd);

    assert!(matches!(sub1.recv().await.unwrap(), WireMessage::TurnEnd));
    assert!(matches!(sub2.recv().await.unwrap(), WireMessage::TurnEnd));
  }

  #[tokio::test]
  async fn test_no_subscriber_no_panic() {
    let bus = WireBus::new(16);
    let pub1 = bus.publisher();
    // No subscribers — should not panic.
    pub1.send(WireMessage::TurnBegin);
  }
}
