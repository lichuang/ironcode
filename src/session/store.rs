use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{from_str, to_string, to_string_pretty};

use crate::error::{Result, SessionError};
use crate::llm::types::Message;
use crate::session::SessionMeta;

/// Manages persistent storage of chat sessions
pub struct SessionStore {
  sessions_dir: PathBuf,
  /// Cached file handles for active sessions' context.jsonl files
  files: Mutex<HashMap<String, File>>,
}

const META_FILE: &str = "meta.json";
const CONTEXT_FILE: &str = "context.jsonl";

impl SessionStore {
  /// Create a new session store rooted at the given data directory
  pub fn new(data_dir: &Path) -> Self {
    Self {
      sessions_dir: data_dir.join("sessions"),
      files: Mutex::new(HashMap::new()),
    }
  }

  /// Create a new session directory with initial meta and empty context file
  pub fn create(&self, meta: &SessionMeta) -> Result<()> {
    let session_dir = self.sessions_dir.join(&meta.id);
    fs::create_dir_all(&session_dir)?;

    self.write_meta(&session_dir, meta)?;

    let context_path = session_dir.join(CONTEXT_FILE);
    let file = OpenOptions::new()
      .create(true)
      .truncate(true)
      .write(true)
      .open(&context_path)?;

    self.files.lock().unwrap().insert(meta.id.clone(), file);

    Ok(())
  }

  /// Append a single message to the session's context.jsonl
  pub fn append_message(&self, id: &str, message: &Message) -> Result<()> {
    let mut files = self.files.lock().unwrap();

    let file = if let Some(f) = files.get_mut(id) {
      f
    } else {
      let session_dir = self.session_dir(id)?;
      let context_path = session_dir.join(CONTEXT_FILE);
      let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .append(true)
        .open(&context_path)?;
      files.entry(id.to_string()).or_insert(f)
    };

    let line = to_string(message).map_err(|e| SessionError::SerializeMessage { source: e })?;

    writeln!(file, "{}", line)?;

    Ok(())
  }

  /// Load session metadata and all messages
  pub fn load(&self, id: &str) -> Result<(SessionMeta, Vec<Message>)> {
    let session_dir = self.session_dir(id)?;
    if !session_dir.exists() {
      return Err(SessionError::NotFound { id: id.to_string() }.into());
    }

    let meta_path = session_dir.join(META_FILE);
    let meta_content = fs::read_to_string(&meta_path).map_err(|e| SessionError::ReadMeta {
      id: id.to_string(),
      source: e,
    })?;
    let meta: SessionMeta =
      from_str(&meta_content).map_err(|e| SessionError::DeserializeMeta { source: e })?;

    let context_path = session_dir.join(CONTEXT_FILE);
    let mut messages = Vec::new();

    if context_path.exists() {
      let file = File::open(&context_path)?;
      let reader = BufReader::new(file);
      for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
          continue;
        }
        let message: Message =
          from_str(&line).map_err(|e| SessionError::DeserializeMessage { source: e })?;
        messages.push(message);
      }
    }

    Ok((meta, messages))
  }

  /// List all sessions, sorted by most recently updated first
  pub fn list(&self) -> Result<Vec<SessionMeta>> {
    let mut sessions = Vec::new();

    if !self.sessions_dir.exists() {
      return Ok(sessions);
    }

    for entry in fs::read_dir(&self.sessions_dir)? {
      let entry = entry?;
      let path = entry.path();
      if !path.is_dir() {
        continue;
      }

      let meta_path = path.join(META_FILE);
      if !meta_path.exists() {
        continue;
      }

      let content = fs::read_to_string(&meta_path)?;
      let meta: SessionMeta =
        from_str(&content).map_err(|e| SessionError::DeserializeMeta { source: e })?;
      sessions.push(meta);
    }

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(sessions)
  }

  /// Rewrite the session's meta.json
  pub fn update_meta(&self, meta: &SessionMeta) -> Result<()> {
    let session_dir = self.session_dir(&meta.id)?;
    self.write_meta(&session_dir, meta)
  }

  /// Replace the entire context.jsonl with the given messages
  pub fn reset_messages(&self, id: &str, messages: &[Message]) -> Result<()> {
    let session_dir = self.session_dir(id)?;
    let context_path = session_dir.join(CONTEXT_FILE);

    let mut file = OpenOptions::new()
      .create(true)
      .truncate(true)
      .write(true)
      .open(&context_path)?;

    for message in messages {
      let line = to_string(message).map_err(|e| SessionError::SerializeMessage { source: e })?;
      writeln!(file, "{}", line)?;
    }

    self.files.lock().unwrap().insert(id.to_string(), file);

    Ok(())
  }

  /// Delete a session directory and all its contents
  #[allow(dead_code)]
  pub fn delete(&self, id: &str) -> Result<()> {
    let session_dir = self.session_dir(id)?;
    if session_dir.exists() {
      fs::remove_dir_all(&session_dir)?;
    }
    self.files.lock().unwrap().remove(id);
    Ok(())
  }

  /// Get the ID of the most recently updated session
  pub fn latest_id(&self) -> Result<Option<String>> {
    let sessions = self.list()?;
    Ok(sessions.into_iter().next().map(|m| m.id))
  }

  fn session_dir(&self, id: &str) -> Result<PathBuf> {
    Ok(self.sessions_dir.join(id))
  }

  fn write_meta(&self, session_dir: &Path, meta: &SessionMeta) -> Result<()> {
    let meta_path = session_dir.join(META_FILE);
    let content = to_string_pretty(meta).map_err(|e| SessionError::SerializeMeta { source: e })?;
    fs::write(&meta_path, content).map_err(|e| SessionError::WriteMeta {
      id: meta.id.clone(),
      source: e,
    })?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use std::thread;
  use std::time::Duration;

  use chrono::Local;
  use tempfile::TempDir;

  use super::*;
  use crate::llm::types::{Message, Role};

  #[test]
  fn test_create_and_load() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let meta = SessionMeta::new("test-session", "You are a helpful assistant");
    store.create(&meta).unwrap();

    let (loaded_meta, messages) = store.load("test-session").unwrap();
    assert_eq!(loaded_meta.id, "test-session");
    assert_eq!(loaded_meta.system_prompt, "You are a helpful assistant");
    assert!(messages.is_empty());
  }

  #[test]
  fn test_append_and_load_messages() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let meta = SessionMeta::new("test-session", "system");
    store.create(&meta).unwrap();

    store
      .append_message("test-session", &Message::system("system prompt"))
      .unwrap();
    store
      .append_message("test-session", &Message::user("hello"))
      .unwrap();
    store
      .append_message("test-session", &Message::assistant("hi"))
      .unwrap();

    let (_, messages) = store.load("test-session").unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, Role::System);
    assert_eq!(messages[1].content, "hello");
    assert_eq!(messages[2].content, "hi");
  }

  #[test]
  fn test_title_generation() {
    let mut meta = SessionMeta::new("test", "system");
    assert!(meta.title.is_empty());

    meta.update_title_from_message(&Message::user("Implement session store"));
    assert_eq!(meta.title, "Implement session store");

    // Should not overwrite
    meta.update_title_from_message(&Message::user("Another message"));
    assert_eq!(meta.title, "Implement session store");
  }

  #[test]
  fn test_list_and_latest() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let mut meta1 = SessionMeta::new("session-1", "system");
    meta1.title = "First".to_string();
    store.create(&meta1).unwrap();

    thread::sleep(Duration::from_millis(10));

    let mut meta2 = SessionMeta::new("session-2", "system");
    meta2.title = "Second".to_string();
    store.create(&meta2).unwrap();

    let sessions = store.list().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id, "session-2"); // More recent first

    assert_eq!(store.latest_id().unwrap(), Some("session-2".to_string()));
  }

  #[test]
  fn test_reset_messages() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let meta = SessionMeta::new("test", "system");
    store.create(&meta).unwrap();

    store
      .append_message("test", &Message::user("hello"))
      .unwrap();
    store
      .reset_messages("test", &[Message::system("system")])
      .unwrap();

    let (_, messages) = store.load("test").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::System);
  }

  #[test]
  fn test_delete() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let meta = SessionMeta::new("test", "system");
    store.create(&meta).unwrap();
    assert!(store.load("test").is_ok());

    store.delete("test").unwrap();
    assert!(store.load("test").is_err());
  }

  #[test]
  fn test_latest_id_empty_store() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());
    assert_eq!(store.latest_id().unwrap(), None);
  }

  #[test]
  fn test_latest_id_single_session() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let meta = SessionMeta::new("lonely-session", "system");
    store.create(&meta).unwrap();

    assert_eq!(
      store.latest_id().unwrap(),
      Some("lonely-session".to_string())
    );
  }

  #[test]
  fn test_latest_id_after_delete_all() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let meta1 = SessionMeta::new("session-a", "system");
    store.create(&meta1).unwrap();

    let meta2 = SessionMeta::new("session-b", "system");
    store.create(&meta2).unwrap();

    assert!(store.latest_id().unwrap().is_some());

    store.delete("session-a").unwrap();
    store.delete("session-b").unwrap();

    assert_eq!(store.latest_id().unwrap(), None);
  }

  #[test]
  fn test_latest_id_reflects_meta_update() {
    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let mut meta1 = SessionMeta::new("older", "system");
    meta1.title = "Old".to_string();
    store.create(&meta1).unwrap();

    thread::sleep(Duration::from_millis(10));

    let mut meta2 = SessionMeta::new("newer", "system");
    meta2.title = "New".to_string();
    store.create(&meta2).unwrap();

    // Initially newer is latest
    assert_eq!(store.latest_id().unwrap(), Some("newer".to_string()));

    // Wait and update older's meta
    thread::sleep(Duration::from_millis(10));
    meta1.title = "Old but updated".to_string();
    meta1.updated_at = Local::now();
    store.update_meta(&meta1).unwrap();

    // Now older should be the latest
    assert_eq!(store.latest_id().unwrap(), Some("older".to_string()));
  }

  #[test]
  fn test_latest_id_with_multiple_sessions() {
    use crate::llm::session::generate_session_id;

    let temp_dir = TempDir::new().unwrap();
    let store = SessionStore::new(temp_dir.path());

    let id_a = generate_session_id();
    let id_b = generate_session_id();
    let id_c = generate_session_id();

    // Create three sessions with small delays
    let mut meta_a = SessionMeta::new(&id_a, "system");
    meta_a.title = "A".to_string();
    store.create(&meta_a).unwrap();
    thread::sleep(Duration::from_millis(10));

    let mut meta_b = SessionMeta::new(&id_b, "system");
    meta_b.title = "B".to_string();
    store.create(&meta_b).unwrap();
    thread::sleep(Duration::from_millis(10));

    let mut meta_c = SessionMeta::new(&id_c, "system");
    meta_c.title = "C".to_string();
    store.create(&meta_c).unwrap();

    // latest_id should return the most recently created session
    assert_eq!(store.latest_id().unwrap(), Some(id_c.clone()));

    // Update session-b to make it the latest
    thread::sleep(Duration::from_millis(10));
    meta_b.title = "B updated".to_string();
    meta_b.updated_at = Local::now();
    store.update_meta(&meta_b).unwrap();

    assert_eq!(store.latest_id().unwrap(), Some(id_b.clone()));

    // Delete the latest and verify fallback to session-c
    store.delete(&id_b).unwrap();
    assert_eq!(store.latest_id().unwrap(), Some(id_c.clone()));

    // Delete all remaining
    store.delete(&id_a).unwrap();
    store.delete(&id_c).unwrap();
    assert_eq!(store.latest_id().unwrap(), None);
  }
}
