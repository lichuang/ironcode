//! Notification system for delivering async events to sinks (LLM, shell, wire).
//!
//! Inspired by kimi-cli's notification architecture:
//! - Events are persisted on disk per session.
//! - Each event targets one or more sinks (e.g. `"llm"`, `"wire"`, `"shell"`).
//! - Sinks claim pending notifications, process them, then ack.
//! - Deduping via `dedupe_key` prevents duplicate notifications.

pub mod llm;
pub mod manager;
pub mod models;
pub mod store;

pub use manager::NotificationManager;
#[allow(unused_imports)]
pub use models::{
  NotificationDelivery, NotificationDeliveryStatus, NotificationEvent, NotificationSeverity,
  NotificationSinkState, NotificationView,
};
