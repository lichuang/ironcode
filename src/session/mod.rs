pub mod store;
pub use store::SessionStore;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// How to initialize a chat session
#[derive(Debug, Clone)]
pub enum SessionMode {
  /// Always create a brand-new session
  New,
  /// Resume the most recently updated session, or create one if none exist
  ResumeLatest,
  /// Resume a specific session by ID
  ResumeById(String),
}

use crate::llm::types::{Message, Role};

/// Metadata for a persisted session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
  /// Session identifier
  pub id: String,
  /// Human-readable title (derived from first user message)
  pub title: String,
  /// When the session was created
  pub created_at: DateTime<Local>,
  /// When the session was last updated
  pub updated_at: DateTime<Local>,
  /// The system prompt used for this session
  pub system_prompt: String,
  /// Whether YOLO mode is enabled for this session
  #[serde(default)]
  pub yolo: bool,
}

impl SessionMeta {
  /// Create a new session metadata
  pub fn new(id: impl Into<String>, system_prompt: impl Into<String>) -> Self {
    let now = Local::now();
    Self {
      id: id.into(),
      title: String::new(),
      created_at: now,
      updated_at: now,
      system_prompt: system_prompt.into(),
      yolo: false,
    }
  }

  /// Update title from the first user message if title is still empty
  pub fn update_title_from_message(&mut self, message: &Message) {
    if !self.title.is_empty() {
      return;
    }
    if message.role == Role::User {
      let text = message.content.trim();
      if !text.is_empty() {
        self.title = if text.chars().count() > 50 {
          format!("{}...", text.chars().take(47).collect::<String>())
        } else {
          text.to_string()
        };
        self.updated_at = Local::now();
      }
    }
  }
}
