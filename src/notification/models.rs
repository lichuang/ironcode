//! Data models for the notification system.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

/// Severity level of a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationSeverity {
  /// Informational message.
  Info,
  /// Success / positive outcome.
  Success,
  /// Warning — something unexpected but not fatal.
  Warning,
  /// Error — something failed.
  Error,
}

impl fmt::Display for NotificationSeverity {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      NotificationSeverity::Info => write!(f, "info"),
      NotificationSeverity::Success => write!(f, "success"),
      NotificationSeverity::Warning => write!(f, "warning"),
      NotificationSeverity::Error => write!(f, "error"),
    }
  }
}

// ---------------------------------------------------------------------------
// Delivery status
// ---------------------------------------------------------------------------

/// Per-sink delivery status for a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDeliveryStatus {
  /// Waiting to be claimed by a sink.
  #[default]
  Pending,
  /// Claimed by a sink but not yet acked.
  Claimed,
  /// Delivered and acknowledged by the sink.
  Acked,
}

// ---------------------------------------------------------------------------
// Event
// ---------------------------------------------------------------------------

/// A notification event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEvent {
  /// Schema version.
  #[serde(default = "default_notification_version")]
  pub version: i32,
  /// Unique notification ID.
  pub id: String,
  /// Category (e.g. "task", "system").
  pub category: String,
  /// Event type (e.g. "task.completed", "task.failed").
  #[serde(rename = "type")]
  pub event_type: String,
  /// Source kind (e.g. "background_task").
  pub source_kind: String,
  /// Source identifier (e.g. task ID).
  pub source_id: String,
  /// Human-readable title.
  pub title: String,
  /// Human-readable body.
  pub body: String,
  /// Severity level.
  pub severity: NotificationSeverity,
  /// Creation timestamp (seconds since UNIX epoch).
  pub created_at: f64,
  /// Arbitrary JSON payload.
  pub payload: Value,
  /// Target sinks (e.g. ["llm", "wire", "shell"]).
  #[serde(default = "default_notification_targets")]
  pub targets: Vec<String>,
  /// Optional deduplication key.
  pub dedupe_key: Option<String>,
}

fn default_notification_version() -> i32 {
  1
}

pub(crate) fn default_notification_targets() -> Vec<String> {
  vec!["llm".to_string(), "wire".to_string(), "shell".to_string()]
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// Per-sink state tracking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationSinkState {
  /// Current delivery status.
  pub status: NotificationDeliveryStatus,
  /// When the sink claimed this notification.
  pub claimed_at: Option<f64>,
  /// When the sink acked this notification.
  pub acked_at: Option<f64>,
}

/// Delivery record for a notification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationDelivery {
  /// Map from sink name to its state.
  pub sinks: HashMap<String, NotificationSinkState>,
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Merged view of an event and its delivery state.
#[derive(Debug, Clone)]
pub struct NotificationView {
  pub event: NotificationEvent,
  pub delivery: NotificationDelivery,
}
