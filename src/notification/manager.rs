//! Notification manager — publish, claim, ack, and deduplication.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::config::NotificationConfig;

use super::models::{
  NotificationDelivery, NotificationDeliveryStatus, NotificationEvent, NotificationSinkState,
  NotificationView, Timestamp,
};
use super::store::NotificationStore;

/// Manager for notifications.
///
/// Notifications are persisted per-session under
/// `~/.ironcode/sessions/{session_id}/notifications/{id}/`.
#[derive(Debug)]
pub struct NotificationManager {
  data_dir: PathBuf,
  config: NotificationConfig,
  session_id: Mutex<Option<String>>,
}

impl NotificationManager {
  /// Create a new manager bound to the given data directory.
  pub fn new(data_dir: PathBuf, config: NotificationConfig) -> Self {
    Self {
      data_dir,
      config,
      session_id: Mutex::new(None),
    }
  }

  /// Bind the manager to a specific session.
  pub fn bind_session(&self, session_id: &str) {
    *self.session_id.lock().unwrap() = Some(session_id.to_string());
  }

  fn store(&self) -> Option<NotificationStore> {
    let session_id = self.session_id.lock().unwrap().clone()?;
    let root = self
      .data_dir
      .join("sessions")
      .join(session_id)
      .join("notifications");
    Some(NotificationStore::new(root))
  }

  /// Generate a new notification ID.
  pub fn new_id(&self) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap_or_default()
      .as_nanos()
      .hash(&mut hasher);
    format!("n{:08x}", hasher.finish())
  }

  /// Publish a notification event.
  ///
  /// If the event has a `dedupe_key` and a matching notification already exists,
  /// the existing view is returned instead of creating a duplicate.
  pub fn publish(&self, event: NotificationEvent) -> NotificationView {
    if let Some(ref key) = event.dedupe_key
      && let Some(existing) = self.find_by_dedupe_key(key)
    {
      return existing;
    }

    let store = match self.store() {
      Some(s) => s,
      None => {
        return NotificationView {
          event,
          delivery: NotificationDelivery::default(),
        };
      }
    };

    let delivery = initial_delivery(&event);
    store.create(&event, &delivery);
    NotificationView { event, delivery }
  }

  /// Find a notification by its deduplication key.
  pub fn find_by_dedupe_key(&self, key: &str) -> Option<NotificationView> {
    let store = self.store()?;
    store
      .list_views()
      .into_iter()
      .find(|view| view.event.dedupe_key.as_deref() == Some(key))
  }

  /// Check whether any notification has a pending delivery for the given sink.
  #[allow(dead_code)]
  pub fn has_pending_for_sink(&self, sink: &str) -> bool {
    let Some(store) = self.store() else {
      return false;
    };
    for view in store.list_views() {
      if let Some(state) = view.delivery.sinks.get(sink)
        && state.status == NotificationDeliveryStatus::Pending
      {
        return true;
      }
    }
    false
  }

  /// Claim pending notifications for a sink.
  ///
  /// Returns up to `limit` notifications whose status for `sink` is `Pending`.
  /// Their status is atomically updated to `Claimed`.
  ///
  /// Claims oldest pending notifications first (matching kimi-cli).
  pub fn claim_for_sink(&self, sink: &str, limit: usize) -> Vec<NotificationView> {
    self.recover();

    let Some(store) = self.store() else {
      return Vec::new();
    };

    let now = now_secs();
    let mut claimed = Vec::new();

    // list_views returns newest first; iterate in reverse to claim oldest first.
    for view in store.list_views().into_iter().rev() {
      if claimed.len() >= limit {
        break;
      }
      let Some(state) = view.delivery.sinks.get(sink) else {
        continue;
      };
      if state.status != NotificationDeliveryStatus::Pending {
        continue;
      }

      let mut delivery = view.delivery.clone();
      let target = delivery.sinks.get_mut(sink).unwrap();
      target.status = NotificationDeliveryStatus::Claimed;
      target.claimed_at = Some(now);
      store.write_delivery(&view.event.id, &delivery);

      claimed.push(NotificationView {
        event: view.event,
        delivery,
      });
    }

    claimed
  }

  /// Acknowledge a notification for a sink.
  ///
  /// Returns the updated view, or `None` if the notification does not exist.
  pub fn ack(&self, sink: &str, notification_id: &str) -> Option<NotificationView> {
    let store = self.store()?;
    let mut delivery = store.read_delivery(notification_id);
    if let Some(state) = delivery.sinks.get_mut(sink) {
      state.status = NotificationDeliveryStatus::Acked;
      state.acked_at = Some(now_secs());
      state.claimed_at = None;
      store.write_delivery(notification_id, &delivery);
    }
    Some(NotificationView {
      event: store.read_event(notification_id)?,
      delivery,
    })
  }

  /// Deliver pending notifications for a sink using a shared claim/ack flow.
  ///
  /// For each pending notification the handler is invoked; if it succeeds the
  /// notification is acked. If the handler returns an error the notification
  /// remains claimed and will be recovered later.
  pub fn deliver_pending<F, E>(
    &self,
    sink: &str,
    limit: usize,
    mut on_notification: F,
  ) -> Vec<NotificationView>
  where
    F: FnMut(&NotificationView) -> Result<(), E>,
    E: std::fmt::Display,
  {
    let mut delivered = Vec::new();
    for view in self.claim_for_sink(sink, limit) {
      if let Err(e) = on_notification(&view) {
        log::warn!(
          "Notification handler failed for {}/{}, leaving claimed for recovery: {}",
          sink,
          view.event.id,
          e
        );
        continue;
      }
      if let Some(acked) = self.ack(sink, &view.event.id) {
        delivered.push(acked);
      }
    }
    delivered
  }

  /// Recover stale claimed notifications.
  ///
  /// Notifications that have been in `Claimed` state for longer than
  /// `claim_stale_after_ms` are reset to `Pending` so they can be retried.
  pub fn recover(&self) {
    let stale_after_s = self.config.claim_stale_after_ms / 1000;
    let Some(store) = self.store() else {
      return;
    };
    let now = now_secs();
    for view in store.list_views() {
      let mut updated = false;
      let mut delivery = view.delivery.clone();
      for state in delivery.sinks.values_mut() {
        if state.status != NotificationDeliveryStatus::Claimed {
          continue;
        }
        let Some(claimed_at) = state.claimed_at else {
          continue;
        };
        if now - claimed_at <= stale_after_s {
          continue;
        }
        state.status = NotificationDeliveryStatus::Pending;
        state.claimed_at = None;
        updated = true;
      }
      if updated {
        store.write_delivery(&view.event.id, &delivery);
      }
    }
  }

  /// List all notification views, newest first.
  #[allow(dead_code)]
  pub fn list_views(&self) -> Vec<NotificationView> {
    self.store().map(|s| s.list_views()).unwrap_or_default()
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn initial_delivery(event: &NotificationEvent) -> NotificationDelivery {
  use std::collections::HashMap;
  let mut sinks = HashMap::new();
  for target in &event.targets {
    sinks.insert(target.clone(), NotificationSinkState::default());
  }
  NotificationDelivery { sinks }
}

fn now_secs() -> Timestamp {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
