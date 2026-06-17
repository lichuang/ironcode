//! Wire dispatcher for client-side hooks.
//!
//! Publishes `WireMessage::HookRequest` on the wire bus and listens for
//! `WireHookResponse` messages on a dedicated channel. This allows a client
//! (TUI, web UI, IDE plugin) to participate in hook decisions.
//!
//! Flow:
//! 1. `HookEngine` creates a `WireHookHandle` and calls `dispatch_wire_hook`.
//! 2. `WireBusHookDispatcher` stores the handle in a pending map and broadcasts
//!    `WireMessage::HookRequest`.
//! 3. A client receives the request and sends a `WireHookResponse` back via
//!    the response channel.
//! 4. The dispatcher's background task resolves the matching handle.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};

use crate::hooks::{WireHookDispatcher, WireHookHandle};
use crate::wire::{WireMessage, WirePublisher};

/// A response to a pending wire hook request.
///
/// Sent by the client back to the dispatcher to resolve a previously issued
/// `WireMessage::HookRequest`.
#[derive(Debug, Clone)]
pub struct WireHookResponse {
  /// Request identifier matching the `HookRequest`.
  pub id: String,
  /// Client decision: `"allow"` or `"block"`.
  pub action: String,
  /// Optional reason when blocked.
  pub reason: String,
}

/// Dispatcher that forwards hook requests over the wire bus.
///
/// Implements `WireHookDispatcher` by broadcasting `HookRequest` wire messages
/// and waiting for matching `WireHookResponse` messages on a private channel.
pub struct WireBusHookDispatcher {
  /// Publisher used to broadcast `WireMessage::HookRequest` to subscribers.
  publisher: WirePublisher,
  /// Pending handles keyed by request id. The background response task resolves
  /// them when the client replies.
  pending: Arc<Mutex<HashMap<String, WireHookHandle>>>,
  /// Background task that drains the response channel. Kept alive as long as
  /// the dispatcher is alive.
  _response_task: Arc<tokio::task::JoinHandle<()>>,
}

impl WireBusHookDispatcher {
  /// Create a new dispatcher and the channel used to send responses back.
  ///
  /// The returned sender should be given to whoever consumes
  /// `WireMessage::HookRequest` (e.g. the TUI or an external client adapter).
  pub fn new(publisher: WirePublisher) -> (Self, mpsc::UnboundedSender<WireHookResponse>) {
    let (response_tx, mut response_rx) = mpsc::unbounded_channel::<WireHookResponse>();
    let pending: Arc<Mutex<HashMap<String, WireHookHandle>>> = Arc::new(Mutex::new(HashMap::new()));
    let pending_task = pending.clone();

    let response_task = tokio::spawn(async move {
      while let Some(response) = response_rx.recv().await {
        if let Some(handle) = pending_task.lock().await.remove(&response.id) {
          handle.resolve(&response.action, &response.reason);
        } else {
          log::warn!(
            "Received HookResponse for unknown wire hook id: {}",
            response.id
          );
        }
      }
    });

    let dispatcher = Self {
      publisher,
      pending,
      _response_task: Arc::new(response_task),
    };

    (dispatcher, response_tx)
  }
}

#[async_trait]
impl WireHookDispatcher for WireBusHookDispatcher {
  async fn dispatch_wire_hook(&self, handle: WireHookHandle) {
    // Store the handle before broadcasting so the response task can resolve it
    // even if the client replies extremely quickly.
    let id = handle.id.clone();
    self.pending.lock().await.insert(id.clone(), handle.clone());

    let msg = WireMessage::HookRequest {
      id,
      event: handle.event.to_string(),
      target: handle.target.clone(),
      input_data: handle.input_data.clone(),
    };
    self.publisher.send(msg);
  }
}

impl std::fmt::Debug for WireBusHookDispatcher {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("WireBusHookDispatcher")
      .field("pending", &self.pending.try_lock().map(|g| g.len()))
      .finish_non_exhaustive()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::hooks::{HookDecision, HookEventType, WireHookSubscription};
  use crate::wire::WireBus;

  #[tokio::test]
  async fn test_wire_bus_dispatcher_round_trip() {
    let bus = WireBus::new(16);
    let publisher = bus.publisher();
    let mut subscriber = bus.subscriber();

    let (dispatcher, response_tx) = WireBusHookDispatcher::new(publisher);

    let mut engine = crate::hooks::HookEngine::new(Vec::new(), None);
    engine.add_wire_subscriptions(vec![WireHookSubscription::new(
      "sub-1",
      HookEventType::PreToolUse,
    )]);
    engine.set_dispatcher(Arc::new(dispatcher));

    let trigger_task = tokio::spawn(async move {
      engine
        .trigger(HookEventType::PreToolUse, "ReadFile", serde_json::json!({}))
        .await
    });

    // Simulate the client receiving the request and responding.
    let msg = subscriber.recv().await.unwrap();
    if let WireMessage::HookRequest { id, .. } = msg {
      response_tx
        .send(WireHookResponse {
          id,
          action: "block".to_string(),
          reason: "denied".to_string(),
        })
        .unwrap();
    } else {
      panic!("expected HookRequest");
    }

    let results = trigger_task.await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
      results[0].decision,
      HookDecision::Block {
        reason: "denied".to_string()
      }
    );
  }
}
