//! SessionPersistence — buffered persistence layer for messages and metadata.

use std::sync::Arc;

use crate::error::Result;
use crate::llm::types::Message;
use crate::session::{SessionMeta, SessionStore};

pub struct SessionPersistence {
  store: Arc<SessionStore>,
  message_buffer: Vec<(String, Message)>,
  meta_buffer: Option<SessionMeta>,
}

impl SessionPersistence {
  pub fn new(store: Arc<SessionStore>) -> Self {
    Self {
      store,
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
    let _ = self.store.reset_messages(session_id, messages);
  }

  /// Flush staged messages and meta to disk.
  pub fn flush(&mut self) -> Result<()> {
    for (sid, msg) in self.message_buffer.drain(..) {
      self.store.append_message(&sid, &msg)?;
    }
    if let Some(meta) = self.meta_buffer.take() {
      self.store.update_meta(&meta)?;
    }
    Ok(())
  }
}
