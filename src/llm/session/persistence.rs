//! SessionPersistence — buffered persistence layer for messages and metadata.

use std::sync::Arc;

use crate::error::Result;
use crate::llm::types::Message;
use crate::session::{SessionMeta, SessionStore};

pub struct SessionPersistence {
  store: Option<Arc<SessionStore>>,
  message_buffer: Vec<(String, Message)>,
  meta_buffer: Option<SessionMeta>,
}

impl SessionPersistence {
  pub fn new(store: Arc<SessionStore>) -> Self {
    Self {
      store: Some(store),
      message_buffer: Vec::new(),
      meta_buffer: None,
    }
  }

  /// Create an in-memory-only persistence layer for subagents.
  pub fn new_in_memory() -> Self {
    Self {
      store: None,
      message_buffer: Vec::new(),
      meta_buffer: None,
    }
  }

  pub fn stage_message(&mut self, session_id: &str, message: &Message) {
    self
      .message_buffer
      .push((session_id.to_string(), message.clone()));
  }

  pub fn stage_meta(&mut self, meta: &SessionMeta) {
    self.meta_buffer = Some(meta.clone());
  }

  pub fn reset_messages(&mut self, session_id: &str, messages: &[Message]) {
    self.message_buffer.clear();
    if let Some(store) = &self.store {
      let _ = store.reset_messages(session_id, messages);
    }
  }

  /// Flush staged messages and meta to disk.
  pub fn flush(&mut self) -> Result<()> {
    let Some(store) = &self.store else {
      self.message_buffer.clear();
      self.meta_buffer = None;
      return Ok(());
    };
    for (sid, msg) in self.message_buffer.drain(..) {
      store.append_message(&sid, &msg)?;
    }
    if let Some(meta) = self.meta_buffer.take() {
      store.update_meta(&meta)?;
    }
    Ok(())
  }
}
