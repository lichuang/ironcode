//! File-system persistence for notifications.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use log::warn;

use super::models::{NotificationDelivery, NotificationEvent, NotificationView};

const EVENT_FILE: &str = "event.json";
const DELIVERY_FILE: &str = "delivery.json";

/// On-disk store for notifications.
pub struct NotificationStore {
  root: PathBuf,
}

impl NotificationStore {
  /// Open (or create) a store rooted at the given directory.
  pub fn new(root: PathBuf) -> Self {
    Self { root }
  }

  fn ensure_root(&self) {
    let _ = fs::create_dir_all(&self.root);
  }

  fn notification_dir(&self, id: &str) -> PathBuf {
    self.ensure_root();
    let path = self.root.join(id);
    let _ = fs::create_dir_all(&path);
    path
  }

  fn event_path(&self, id: &str) -> PathBuf {
    self.notification_dir(id).join(EVENT_FILE)
  }

  fn delivery_path(&self, id: &str) -> PathBuf {
    self.notification_dir(id).join(DELIVERY_FILE)
  }

  /// Persist a new notification.
  pub fn create(&self, event: &NotificationEvent, delivery: &NotificationDelivery) {
    let _ = atomic_write_json(&self.event_path(&event.id), event);
    let _ = atomic_write_json(&self.delivery_path(&event.id), delivery);
  }

  /// List all valid notification IDs.
  pub fn list_ids(&self) -> Vec<String> {
    let mut ids = Vec::new();
    if !self.root.exists() {
      return ids;
    }
    if let Ok(entries) = fs::read_dir(&self.root) {
      for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
          && path.join(EVENT_FILE).is_file()
          && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
          ids.push(name.to_string());
        }
      }
    }
    ids
  }

  /// Read an event.
  pub fn read_event(&self, id: &str) -> Option<NotificationEvent> {
    let path = self.event_path(id);
    if !path.exists() {
      return None;
    }
    match fs::read_to_string(&path) {
      Ok(text) => serde_json::from_str(&text).ok(),
      Err(_) => None,
    }
  }

  /// Write an event.
  #[allow(dead_code)]
  pub fn write_event(&self, event: &NotificationEvent) {
    let _ = atomic_write_json(&self.event_path(&event.id), event);
  }

  /// Read a delivery record.
  pub fn read_delivery(&self, id: &str) -> NotificationDelivery {
    read_json(&self.delivery_path(id)).unwrap_or_default()
  }

  /// Write a delivery record.
  pub fn write_delivery(&self, id: &str, delivery: &NotificationDelivery) {
    let _ = atomic_write_json(&self.delivery_path(id), delivery);
  }

  /// Return a merged view.
  pub fn merged_view(&self, id: &str) -> Option<NotificationView> {
    let event = self.read_event(id)?;
    let delivery = self.read_delivery(id);
    Some(NotificationView { event, delivery })
  }

  /// List all views, sorted by newest first.
  pub fn list_views(&self) -> Vec<NotificationView> {
    let mut views = Vec::new();
    for id in self.list_ids() {
      if let Some(view) = self.merged_view(&id) {
        views.push(view);
      }
    }
    views.sort_by(|a, b| {
      b.event
        .created_at
        .partial_cmp(&a.event.created_at)
        .unwrap_or(std::cmp::Ordering::Equal)
    });
    views
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
  let json = serde_json::to_string_pretty(value)
    .map_err(|e| std::io::Error::other(format!("JSON serialization failed: {}", e)))?;
  let temp = path.with_extension("tmp");
  let mut file = fs::OpenOptions::new()
    .create(true)
    .truncate(true)
    .write(true)
    .open(&temp)?;
  file.write_all(json.as_bytes())?;
  file.sync_all()?;
  drop(file);
  fs::rename(&temp, path)?;
  Ok(())
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Option<T> {
  if !path.exists() {
    return Some(T::default());
  }
  match fs::read_to_string(path) {
    Ok(text) => match serde_json::from_str(&text) {
      Ok(v) => Some(v),
      Err(e) => {
        warn!("Failed to parse JSON from {}: {}", path.display(), e);
        Some(T::default())
      }
    },
    Err(e) => {
      warn!("Failed to read {}: {}", path.display(), e);
      Some(T::default())
    }
  }
}

use serde::{Deserialize, Serialize};
