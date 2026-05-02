//! Input history management for user messages.
//!
//! Provides persistent storage of user input history with shell-like navigation
//! (Up/Down keys). History is stored as JSON Lines at `~/.ironcode/history.jsonl`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string};

use crate::cli::runtime::Runtime;
use crate::config::HistoryConfig;
use crate::config::loader::data_dir;

/// Filename for storing input history.
const HISTORY_FILENAME: &str = "history.jsonl";

/// Default maximum file size (1MB).
#[allow(dead_code)]
const DEFAULT_MAX_SIZE: usize = 1024 * 1024;

/// Default maximum number of entries.
#[allow(dead_code)]
const DEFAULT_MAX_ENTRIES: usize = 1000;

/// Maximum retries for acquiring file lock.
const MAX_LOCK_RETRIES: usize = 10;
/// Retry delay in milliseconds.
const LOCK_RETRY_DELAY_MS: u64 = 10;

/// A single history entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
  /// The input text.
  pub text: String,
  /// Unix timestamp when the entry was created.
  pub ts: u64,
}

impl HistoryEntry {
  /// Create a new history entry with current timestamp.
  pub fn new(text: impl Into<String>) -> Self {
    let ts = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.as_secs())
      .unwrap_or(0);
    Self {
      text: text.into(),
      ts,
    }
  }
}

/// Input history manager.
///
/// Manages the in-memory state for Up/Down navigation through
/// history entries and handles persistence automatically.
#[derive(Debug)]
pub struct InputHistoryManager {
  /// All history entries loaded from file plus pending entries from this session.
  entries: Vec<HistoryEntry>,
  /// Current navigation position.
  /// - None: Not browsing history (showing current user input).
  /// - Some(idx): Showing entries[idx].
  cursor: Option<usize>,
  /// Original input before starting navigation (restored when navigating past newest).
  original_input: String,
  /// Persistent storage handler.
  storage: InputHistoryStorage,
}

impl Default for InputHistoryManager {
  fn default() -> Self {
    Self::new()
  }
}

impl InputHistoryManager {
  /// Create a new empty navigation state without persistence.
  pub fn new() -> Self {
    Self {
      entries: Vec::new(),
      cursor: None,
      original_input: String::new(),
      storage: InputHistoryStorage::with_path(PathBuf::new(), HistoryConfig::default()),
    }
  }

  /// Create with runtime for persistence.
  pub fn with_config(runtime: &Runtime) -> Self {
    let storage = InputHistoryStorage::new(runtime);
    let entries = storage.load_entries();
    Self {
      entries,
      cursor: None,
      original_input: String::new(),
      storage,
    }
  }

  /// Create with pre-loaded entries (from file).
  #[cfg(test)]
  pub fn with_entries(entries: Vec<HistoryEntry>) -> Self {
    Self {
      entries,
      cursor: None,
      original_input: String::new(),
      storage: InputHistoryStorage::with_path(PathBuf::new(), HistoryConfig::default()),
    }
  }

  /// Returns true if Up/Down should navigate history for the given current input.
  ///
  /// Navigation is allowed when:
  /// - Input is empty, or
  /// - Currently browsing history and input matches the displayed entry, or
  /// - Not browsing and input matches the original input (allows restarting navigation)
  pub fn should_navigate(&self, current_input: &str) -> bool {
    if current_input.is_empty() {
      return true;
    }
    match self.cursor {
      Some(idx) if idx < self.entries.len() => self.entries[idx].text == current_input,
      None => {
        // Not browsing - allow if input matches original (allows restart after exiting)
        self.original_input == current_input
      }
      _ => false,
    }
  }

  /// Navigate to previous (older) entry.
  ///
  /// If not currently browsing, saves the current input as `original_input`
  /// and returns the most recent history entry.
  ///
  /// Returns the entry to display, or None if already at oldest entry.
  pub fn navigate_up(&mut self, current_input: &str) -> Option<&HistoryEntry> {
    if self.entries.is_empty() {
      return None;
    }

    // Save original input if starting navigation
    if self.cursor.is_none() {
      self.original_input = current_input.to_string();
    }

    let new_idx = match self.cursor {
      None => self.entries.len().checked_sub(1)?,
      Some(0) => return None, // Already at oldest
      Some(idx) => idx - 1,
    };

    self.cursor = Some(new_idx);
    self.entries.get(new_idx)
  }

  /// Navigate to next (newer) entry.
  ///
  /// Returns the entry to display, or None to indicate should exit browsing.
  /// When at newest entry, returns None but stays in browsing mode (caller should
  /// check if input changed to detect "exit" request).
  pub fn navigate_down(&mut self) -> Option<&HistoryEntry> {
    match self.cursor {
      None => None, // Not browsing history
      Some(idx) => {
        if idx + 1 >= self.entries.len() {
          // At newest - signal caller to exit browsing mode
          self.cursor = None;
          None
        } else {
          let new_idx = idx + 1;
          self.cursor = Some(new_idx);
          self.entries.get(new_idx)
        }
      }
    }
  }

  /// Check if we're currently browsing history.
  pub fn is_browsing(&self) -> bool {
    self.cursor.is_some()
  }

  #[allow(dead_code)]
  pub fn cursor(&self) -> Option<usize> {
    self.cursor
  }

  pub fn original_input(&self) -> &str {
    &self.original_input
  }

  /// Record a new entry to the history (both memory and persistent storage).
  ///
  /// Deduplicates against the most recent entry.
  /// Persists to file automatically if the history was initialized with a config.
  pub fn record_entry(&mut self, text: impl Into<String>) {
    let text = text.into();
    if text.is_empty() {
      return;
    }

    // Check for duplicate of most recent entry
    if let Some(last) = self.entries.last()
      && last.text == text
    {
      return;
    }

    // Add to in-memory entries
    self.entries.push(HistoryEntry::new(&text));

    // Persist to file (ignore errors, just log them)
    if let Err(e) = self.storage.append_entry(&text) {
      log::warn!("Failed to persist input to history file: {}", e);
    }

    // Reset navigation when new entry added
    self.cursor = None;
    self.original_input.clear();
  }
}

/// Persistent history storage.
///
/// Handles file I/O operations for loading and saving history entries.
#[derive(Debug, Clone)]
pub struct InputHistoryStorage {
  path: PathBuf,
  config: HistoryConfig,
}

impl InputHistoryStorage {
  /// Create a new history storage from runtime.
  pub fn new(runtime: &Runtime) -> Self {
    let config = &runtime.config;
    let path = data_dir(config).join(HISTORY_FILENAME);
    Self {
      path,
      config: config.history.clone(),
    }
  }

  /// Create a new history storage with explicit path.
  pub fn with_path(path: PathBuf, config: HistoryConfig) -> Self {
    Self { path, config }
  }

  /// Load all entries from the history file.
  ///
  /// Returns an empty vector if the file doesn't exist or can't be read.
  /// Uses a shared lock to prevent reading during write operations.
  pub fn load_entries(&self) -> Vec<HistoryEntry> {
    if !self.path.exists() {
      return Vec::new();
    }

    // Open file
    let file = match File::open(&self.path) {
      Ok(f) => f,
      Err(e) => {
        log::warn!("Failed to open history file: {}", e);
        return Vec::new();
      }
    };

    // Acquire shared lock (non-blocking with retries)
    for attempt in 0..MAX_LOCK_RETRIES {
      match file.try_lock_shared() {
        Ok(()) => break,
        Err(_) if attempt < MAX_LOCK_RETRIES - 1 => {
          sleep(Duration::from_millis(LOCK_RETRY_DELAY_MS));
        }
        Err(_) => {
          log::warn!(
            "Failed to acquire shared lock on history file after {} attempts",
            MAX_LOCK_RETRIES
          );
          return Vec::new();
        }
      }
    }

    // Read file content
    let mut content = String::new();
    if let Err(e) = file.take(u64::MAX).read_to_string(&mut content) {
      log::warn!("Failed to read history file: {}", e);
      return Vec::new();
    }

    // Lock is automatically released when file is dropped
    content
      .lines()
      .filter_map(|line| {
        if line.trim().is_empty() {
          return None;
        }
        match from_str::<HistoryEntry>(line) {
          Ok(entry) => Some(entry),
          Err(e) => {
            log::warn!("Failed to parse history entry: {}", e);
            None
          }
        }
      })
      .collect()
  }

  /// Append a single entry to the history file.
  ///
  /// Creates the file and parent directories if they don't exist.
  /// Automatically trims the file if it exceeds size limits.
  /// Uses an exclusive lock to prevent concurrent modifications.
  pub fn append_entry(&self, text: impl Into<String>) -> std::io::Result<()> {
    let entry = HistoryEntry::new(text);
    let line = format!("{}\n", to_string(&entry)?);

    // Ensure parent directory exists
    if let Some(parent) = self.path.parent() {
      fs::create_dir_all(parent)?;
    }

    // Open file for read/write (append mode doesn't work well with locking)
    let mut file = OpenOptions::new()
      .create(true)
      .truncate(false)
      .read(true)
      .write(true)
      .open(&self.path)?;

    // Acquire exclusive lock (non-blocking with retries)
    for attempt in 0..MAX_LOCK_RETRIES {
      match file.try_lock_exclusive() {
        Ok(true) => break,
        Ok(false) if attempt < MAX_LOCK_RETRIES - 1 => {
          sleep(Duration::from_millis(LOCK_RETRY_DELAY_MS));
        }
        Ok(false) => {
          return Err(std::io::Error::other(format!(
            "Failed to acquire exclusive lock on history file after {} attempts",
            MAX_LOCK_RETRIES
          )));
        }
        Err(e) => {
          return Err(std::io::Error::other(format!(
            "Failed to acquire exclusive lock on history file: {}",
            e
          )));
        }
      }
    }

    // Seek to end and append
    file.seek(SeekFrom::End(0))?;
    file.write_all(line.as_bytes())?;
    file.flush()?;

    // Check if we need to trim (while still holding the lock)
    let should_trim = {
      let metadata = file.metadata()?;
      let file_size = metadata.len() as usize;
      let max_size = self.config.max_size;
      let max_entries = self.config.max_entries;

      (max_size > 0 && file_size > max_size)
        || (max_entries > 0 && Self::count_entries_locked(&file)? > max_entries)
    };

    if should_trim {
      Self::trim_locked(&mut file, &self.config)?;
    }

    // Lock is automatically released when file is dropped
    Ok(())
  }

  /// Count entries in an already locked file.
  fn count_entries_locked(file: &File) -> std::io::Result<usize> {
    let mut content = String::new();
    // Read from current position
    let mut file_ref = file.try_clone()?;
    file_ref.seek(SeekFrom::Start(0))?;
    file_ref.read_to_string(&mut content)?;
    Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
  }

  /// Trim the history file using an already locked file.
  ///
  /// The file must be locked exclusively by the caller.
  fn trim_locked(file: &mut File, config: &HistoryConfig) -> std::io::Result<()> {
    // Read all entries from the file
    let mut content = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut content)?;

    let entries: Vec<HistoryEntry> = content
      .lines()
      .filter(|line| !line.trim().is_empty())
      .filter_map(|line| serde_json::from_str::<HistoryEntry>(line).ok())
      .collect();

    if entries.is_empty() {
      return Ok(());
    }

    let max_size = config.max_size;
    let max_entries = config.max_entries;

    // Determine how many entries to keep
    let mut keep_count = entries.len();

    // Check entry count limit
    if max_entries > 0 && keep_count > max_entries {
      keep_count = max_entries;
    }

    // Check size limit
    if max_size > 0 {
      let mut size = 0;
      let mut count = 0;
      for entry in entries.iter().rev().take(keep_count) {
        let line = to_string(entry)?;
        size += line.len() + 1; // +1 for newline
        if size > max_size {
          break;
        }
        count += 1;
      }
      keep_count = count;
    }

    if keep_count >= entries.len() {
      return Ok(());
    }

    // Keep only the last `keep_count` entries
    let start_idx = entries.len() - keep_count;
    let kept_entries = &entries[start_idx..];

    // Rewrite file
    let new_content = kept_entries
      .iter()
      .map(to_string)
      .collect::<Result<Vec<_>, _>>()?
      .join("\n");

    // Add trailing newline if content is not empty
    let new_content = if new_content.is_empty() {
      new_content
    } else {
      format!("{}\n", new_content)
    };

    // Truncate and rewrite
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(new_content.as_bytes())?;
    file.flush()?;

    log::info!(
      "Trimmed history file: removed {} old entries, kept {}",
      start_idx,
      keep_count
    );

    Ok(())
  }

  /// Clear all history.
  #[allow(dead_code)]
  pub fn clear(&self) -> std::io::Result<()> {
    if self.path.exists() {
      fs::remove_file(&self.path)?;
    }
    Ok(())
  }

  #[allow(dead_code)]
  /// Get the history file path.
  pub fn path(&self) -> &PathBuf {
    &self.path
  }
}

#[allow(dead_code)]
/// Convenience function to append an entry to history.
pub fn save_input(text: impl Into<String>, runtime: &Runtime) -> std::io::Result<()> {
  let storage = InputHistoryStorage::new(runtime);
  storage.append_entry(text)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::{CompactionConfig, Config, HistoryConfig, LoggingConfig, RetryConfig};
  use std::collections::HashMap;
  use tempfile::TempDir;

  fn create_test_config_with_history(dir: &TempDir, max_size: usize, max_entries: usize) -> Config {
    Config {
      dir: Some(dir.path().to_path_buf()),
      default_model: "test/model".to_string(),
      providers: HashMap::new(),
      models: HashMap::new(),
      logging: LoggingConfig::default(),
      default_thinking: true,
      history: HistoryConfig {
        max_size,
        max_entries,
      },
      compaction: CompactionConfig::default(),
      retry: RetryConfig::default(),
      yolo: false,
      auto_approve: Vec::new(),
      mcp: crate::config::McpConfig::default(),
    }
  }

  #[test]
  fn test_manager_new() {
    let manager = InputHistoryManager::new();
    assert!(manager.entries.is_empty());
    assert!(!manager.is_browsing());
  }

  #[test]
  fn test_manager_record_and_navigate() {
    let mut manager = InputHistoryManager::with_entries(vec![
      HistoryEntry::new("first"),
      HistoryEntry::new("second"),
    ]);

    // Navigate up
    assert!(manager.should_navigate(""));
    let entry = manager.navigate_up("").unwrap();
    assert_eq!(entry.text, "second");

    // Record new entry resets navigation
    manager.record_entry("third");
    assert!(!manager.is_browsing());
    assert_eq!(manager.entries.len(), 3);
  }

  fn create_test_config(dir: &TempDir) -> Config {
    create_test_config_with_history(dir, DEFAULT_MAX_SIZE, DEFAULT_MAX_ENTRIES)
  }

  #[test]
  fn test_history_entry_new() {
    let entry = HistoryEntry::new("hello world");
    assert_eq!(entry.text, "hello world");
    assert!(entry.ts > 0);
  }

  #[test]
  fn test_nav_empty() {
    let mut nav = InputHistoryManager::new();
    assert!(nav.entries.is_empty());
    assert!(!nav.is_browsing());
    assert_eq!(nav.navigate_up(""), None);
    assert_eq!(nav.navigate_down(), None);
  }

  #[test]
  fn test_nav_basic() {
    let entries = vec![
      HistoryEntry::new("first"),
      HistoryEntry::new("second"),
      HistoryEntry::new("third"),
    ];
    let mut nav = InputHistoryManager::with_entries(entries);

    // Navigate up from empty input
    assert!(nav.should_navigate(""));
    let entry = nav.navigate_up("").unwrap();
    assert_eq!(entry.text, "third");
    assert!(nav.is_browsing());

    // Navigate up again
    let entry = nav.navigate_up("third").unwrap();
    assert_eq!(entry.text, "second");

    // Navigate down
    let entry = nav.navigate_down().unwrap();
    assert_eq!(entry.text, "third");

    // Navigate past newest
    assert_eq!(nav.navigate_down(), None);
    assert!(!nav.is_browsing());
  }

  #[test]
  fn test_nav_should_navigate() {
    let entries = vec![HistoryEntry::new("hello"), HistoryEntry::new("world")];
    let mut nav = InputHistoryManager::with_entries(entries);

    // Empty input allows navigation
    assert!(nav.should_navigate(""));

    // Modified input doesn't allow navigation
    assert!(!nav.should_navigate("modified"));

    // Navigate to an entry
    nav.navigate_up("");
    assert!(nav.should_navigate("world")); // Matches current
    assert!(!nav.should_navigate("modified"));
  }

  #[test]
  fn test_nav_original_input() {
    let entries = vec![HistoryEntry::new("history")];
    let mut nav = InputHistoryManager::with_entries(entries);

    // Navigate up with existing input
    nav.navigate_up("original");
    assert_eq!(nav.original_input(), "original");

    // Navigate down past newest restores original
    assert_eq!(nav.navigate_down(), None);
    assert_eq!(nav.original_input(), "original");
  }

  #[test]
  fn test_record_entry_dedup() {
    let mut nav = InputHistoryManager::new();

    nav.record_entry("hello");
    assert_eq!(nav.entries.len(), 1);

    // Duplicate should be ignored
    nav.record_entry("hello");
    assert_eq!(nav.entries.len(), 1);

    // New entry should be added
    nav.record_entry("world");
    assert_eq!(nav.entries.len(), 2);
  }

  #[test]
  fn test_navigate_after_exit() {
    // Test the scenario: two entries, navigate down to exit, then navigate up again
    let entries = vec![HistoryEntry::new("first"), HistoryEntry::new("second")];
    let mut nav = InputHistoryManager::with_entries(entries);

    // Navigate up to second
    let entry = nav.navigate_up("").unwrap();
    assert_eq!(entry.text, "second");
    assert!(nav.is_browsing());

    // Navigate up to first
    let entry = nav.navigate_up("second").unwrap();
    assert_eq!(entry.text, "first");
    assert!(nav.is_browsing());

    // Navigate down to second
    let entry = nav.navigate_down().unwrap();
    assert_eq!(entry.text, "second");
    assert!(nav.is_browsing());

    // Navigate down to exit (returns None, cursor becomes None)
    assert_eq!(nav.navigate_down(), None);
    assert!(!nav.is_browsing());

    // Now navigate up again - should work even after exit
    assert!(nav.should_navigate(""));
    let entry = nav.navigate_up("").unwrap();
    assert_eq!(entry.text, "second");
  }

  #[test]
  fn test_navigate_with_modified_input_after_exit() {
    // Test: user modifies input after exiting, then tries to navigate
    let entries = vec![HistoryEntry::new("first"), HistoryEntry::new("second")];
    let mut nav = InputHistoryManager::with_entries(entries);

    // Navigate up
    nav.navigate_up("").unwrap(); // second
    nav.navigate_up("second").unwrap(); // first

    // Navigate down to exit
    nav.navigate_down().unwrap(); // second
    assert_eq!(nav.navigate_down(), None); // exit
    assert!(!nav.is_browsing());

    // User modifies input
    let modified_input = "modified";
    // should_navigate should return false because input doesn't match original
    assert!(!nav.should_navigate(modified_input));

    // But should_navigate should return true for original input
    assert!(nav.should_navigate(""));
  }

  #[test]
  fn test_record_empty() {
    let mut nav = InputHistoryManager::new();
    nav.record_entry("");
    assert!(nav.entries.is_empty());
  }

  #[test]
  fn test_storage_load_save() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let runtime = Runtime::for_test(config.clone());
    let storage = InputHistoryStorage::new(&runtime);

    // Initially empty
    let entries = storage.load_entries();
    assert!(entries.is_empty());

    // Append entries
    storage.append_entry("first").unwrap();
    storage.append_entry("second").unwrap();
    storage.append_entry("third").unwrap();

    // Load and verify
    let entries = storage.load_entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].text, "first");
    assert_eq!(entries[1].text, "second");
    assert_eq!(entries[2].text, "third");
  }

  #[test]
  fn test_storage_trim_by_entries() {
    let temp_dir = TempDir::new().unwrap();
    // Create config with max_entries=2 and max_size=0 (unlimited)
    let config = create_test_config_with_history(&temp_dir, 0, 2);
    let runtime = Runtime::for_test(config.clone());
    let storage = InputHistoryStorage::new(&runtime);

    // Add 5 entries
    for i in 0..5 {
      storage.append_entry(format!("entry{}", i)).unwrap();
    }

    // Should only keep last 2
    let entries = storage.load_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].text, "entry3");
    assert_eq!(entries[1].text, "entry4");
  }

  #[test]
  fn test_storage_trim_by_size() {
    let temp_dir = TempDir::new().unwrap();
    // Create config with max_size=100 bytes and max_entries=0 (unlimited)
    let config = create_test_config_with_history(&temp_dir, 100, 0);
    let runtime = Runtime::for_test(config.clone());
    let storage = InputHistoryStorage::new(&runtime);

    // Add entries that will exceed size limit
    storage.append_entry("short").unwrap();
    storage
      .append_entry("this is a much longer entry that will take up space")
      .unwrap();
    storage.append_entry("another entry").unwrap();

    // File should be trimmed to fit within size limit (allow some tolerance for JSON overhead)
    let metadata = fs::metadata(storage.path()).unwrap();
    // The limit is approximate since we keep whole entries
    assert!(
      metadata.len() as usize <= config.history.max_size.saturating_add(100),
      "File size {} should be approximately within limit {}",
      metadata.len(),
      config.history.max_size
    );
  }

  #[test]
  fn test_storage_clear() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let runtime = Runtime::for_test(config.clone());
    let storage = InputHistoryStorage::new(&runtime);

    storage.append_entry("test").unwrap();
    assert!(storage.path().exists());

    storage.clear().unwrap();
    assert!(!storage.path().exists());
  }

  #[test]
  fn test_save_input_convenience() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let runtime = Runtime::for_test(config);

    save_input("hello", &runtime).unwrap();
    save_input("world", &runtime).unwrap();

    let storage = InputHistoryStorage::new(&runtime);
    let entries = storage.load_entries();
    assert_eq!(entries.len(), 2);
  }
}
