//! Input history management for user messages.
//!
//! Provides persistent storage of user input history with shell-like navigation
//! (Up/Down keys). History is stored as JSON Lines at `~/.ironcode/history.jsonl`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};

use crate::config::{Config, HistoryConfig};
use crate::config::loader::data_dir;

/// Filename for storing input history.
const HISTORY_FILENAME: &str = "history.jsonl";

/// Default maximum file size (1MB).
const DEFAULT_MAX_SIZE: usize = 1024 * 1024;

/// Default maximum number of entries.
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

/// Navigation state machine for browsing input history.
///
/// This struct manages the in-memory state for Up/Down navigation through
/// history entries. It does not handle persistence directly - use `InputHistory`
/// for file operations.
#[derive(Debug, Clone)]
pub struct InputHistoryNav {
    /// All history entries loaded from file plus pending entries from this session.
    entries: Vec<HistoryEntry>,
    /// Current navigation position.
    /// - None: Not browsing history (showing current user input).
    /// - Some(idx): Showing entries[idx].
    cursor: Option<usize>,
    /// Original input before starting navigation (restored when navigating past newest).
    original_input: String,
}

impl Default for InputHistoryNav {
    fn default() -> Self {
        Self::new()
    }
}

impl InputHistoryNav {
    /// Create a new empty navigation state.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            cursor: None,
            original_input: String::new(),
        }
    }

    /// Create with pre-loaded entries (from file).
    pub fn with_entries(entries: Vec<HistoryEntry>) -> Self {
        Self {
            entries,
            cursor: None,
            original_input: String::new(),
        }
    }

    /// Load history from file and create navigation state.
    pub fn load(config: &Config) -> Self {
        let history = InputHistory::new(config);
        let entries = history.load_entries();
        Self::with_entries(entries)
    }

    /// Returns true if Up/Down should navigate history for the given current input.
    ///
    /// Navigation is allowed when:
    /// - Input is empty, or
    /// - Input matches the currently displayed history entry (user hasn't modified it)
    pub fn should_navigate(&self, current_input: &str) -> bool {
        if current_input.is_empty() {
            return true;
        }
        match self.cursor {
            Some(idx) if idx < self.entries.len() => {
                self.entries[idx].text == current_input
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
    /// Returns the entry to display, or None if past newest (should restore original_input).
    pub fn navigate_down(&mut self) -> Option<&HistoryEntry> {
        match self.cursor {
            None => None, // Not browsing history
            Some(idx) => {
                if idx + 1 >= self.entries.len() {
                    // Past newest - exit browsing mode
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

    /// Get the current cursor position if browsing.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Get the original input (before navigation started).
    pub fn original_input(&self) -> &str {
        &self.original_input
    }

    /// Get all entries.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Record a new entry to the in-memory history.
    ///
    /// This does NOT persist to file - call `InputHistory::append_entry` for that.
    /// Deduplicates against the most recent entry.
    pub fn record_entry(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.is_empty() {
            return;
        }

        // Check for duplicate of most recent entry
        if let Some(last) = self.entries.last() {
            if last.text == text {
                return;
            }
        }

        self.entries.push(HistoryEntry::new(text));
        // Reset navigation when new entry added
        self.cursor = None;
        self.original_input.clear();
    }

    /// Reset navigation state (e.g., when user presses Escape).
    pub fn reset_navigation(&mut self) {
        self.cursor = None;
        self.original_input.clear();
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Persistent history manager.
///
/// Handles file I/O operations for loading and saving history entries.
#[derive(Debug, Clone)]
pub struct InputHistory {
    path: PathBuf,
    config: HistoryConfig,
}

impl InputHistory {
    /// Create a new history manager from config.
    pub fn new(config: &Config) -> Self {
        let path = data_dir(config).join(HISTORY_FILENAME);
        Self {
            path,
            config: config.history.clone(),
        }
    }

    /// Create a new history manager with explicit path (for testing).
    #[cfg(test)]
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
                    std::thread::sleep(std::time::Duration::from_millis(LOCK_RETRY_DELAY_MS));
                }
                Err(_) => {
                    log::warn!("Failed to acquire shared lock on history file after {} attempts", MAX_LOCK_RETRIES);
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
                match serde_json::from_str::<HistoryEntry>(line) {
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
        let line = format!("{}\n", serde_json::to_string(&entry)?);

        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open file for read/write (append mode doesn't work well with locking)
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.path)?;

        // Acquire exclusive lock (non-blocking with retries)
        for attempt in 0..MAX_LOCK_RETRIES {
            match file.try_lock_exclusive() {
                Ok(true) => break,
                Ok(false) if attempt < MAX_LOCK_RETRIES - 1 => {
                    std::thread::sleep(std::time::Duration::from_millis(LOCK_RETRY_DELAY_MS));
                }
                Ok(false) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        format!("Failed to acquire exclusive lock on history file after {} attempts", MAX_LOCK_RETRIES),
                    ));
                }
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to acquire exclusive lock on history file: {}", e),
                    ));
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

            (max_size > 0 && file_size > max_size) || 
            (max_entries > 0 && Self::count_entries_locked(&file)? > max_entries)
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

    /// Count entries in the file without loading them all.
    /// Uses a shared lock to ensure consistent read.
    fn count_entries(&self) -> std::io::Result<usize> {
        if !self.path.exists() {
            return Ok(0);
        }

        let file = File::open(&self.path)?;

        // Acquire shared lock
        for attempt in 0..MAX_LOCK_RETRIES {
            match file.try_lock_shared() {
                Ok(()) => break,
                Err(_) if attempt < MAX_LOCK_RETRIES - 1 => {
                    std::thread::sleep(std::time::Duration::from_millis(LOCK_RETRY_DELAY_MS));
                }
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        format!("Failed to acquire shared lock after {} attempts", MAX_LOCK_RETRIES),
                    ));
                }
            }
        }

        let mut content = String::new();
        file.take(u64::MAX).read_to_string(&mut content)?;
        Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
    }

    /// Trim the history file to size/entry limits.
    ///
    /// Opens the file, acquires an exclusive lock, and trims if necessary.
    /// This is used when trimming is requested independently of append.
    pub fn trim(&self) -> std::io::Result<()> {
        if !self.path.exists() {
            return Ok(());
        }

        // Open file for read/write
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)?;

        // Acquire exclusive lock
        for attempt in 0..MAX_LOCK_RETRIES {
            match file.try_lock_exclusive() {
                Ok(true) => break,
                Ok(false) if attempt < MAX_LOCK_RETRIES - 1 => {
                    std::thread::sleep(std::time::Duration::from_millis(LOCK_RETRY_DELAY_MS));
                }
                Ok(false) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        format!("Failed to acquire exclusive lock for trim after {} attempts", MAX_LOCK_RETRIES),
                    ));
                }
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to acquire exclusive lock for trim: {}", e),
                    ));
                }
            }
        }

        Self::trim_locked(&mut file, &self.config)
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
            .filter_map(|line| {
                if line.trim().is_empty() {
                    return None;
                }
                match serde_json::from_str::<HistoryEntry>(line) {
                    Ok(entry) => Some(entry),
                    Err(_) => None,
                }
            })
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
                let line = serde_json::to_string(entry)?;
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
            .map(|e| serde_json::to_string(e))
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
    pub fn clear(&self) -> std::io::Result<()> {
        if self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    /// Get the history file path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

/// Convenience function to append an entry to history.
pub fn save_input(text: impl Into<String>, config: &Config) -> std::io::Result<()> {
    let history = InputHistory::new(config);
    history.append_entry(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_config_with_history(dir: &TempDir, max_size: usize, max_entries: usize) -> Config {
        Config {
            dir: Some(dir.path().to_path_buf()),
            default_model: "test/model".to_string(),
            providers: HashMap::new(),
            models: HashMap::new(),
            logging: crate::config::LoggingConfig::default(),
            default_thinking: true,
            history: HistoryConfig {
                max_size,
                max_entries,
            },
        }
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
        let mut nav = InputHistoryNav::new();
        assert!(nav.is_empty());
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
        let mut nav = InputHistoryNav::with_entries(entries);

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
        let entries = vec![
            HistoryEntry::new("hello"),
            HistoryEntry::new("world"),
        ];
        let mut nav = InputHistoryNav::with_entries(entries);

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
        let mut nav = InputHistoryNav::with_entries(entries);

        // Navigate up with existing input
        nav.navigate_up("original");
        assert_eq!(nav.original_input(), "original");

        // Navigate down past newest restores original
        assert_eq!(nav.navigate_down(), None);
        assert_eq!(nav.original_input(), "original");
    }

    #[test]
    fn test_record_entry_dedup() {
        let mut nav = InputHistoryNav::new();
        
        nav.record_entry("hello");
        assert_eq!(nav.len(), 1);
        
        // Duplicate should be ignored
        nav.record_entry("hello");
        assert_eq!(nav.len(), 1);
        
        // New entry should be added
        nav.record_entry("world");
        assert_eq!(nav.len(), 2);
    }

    #[test]
    fn test_record_empty() {
        let mut nav = InputHistoryNav::new();
        nav.record_entry("");
        assert!(nav.is_empty());
    }

    #[test]
    fn test_history_load_save() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let history = InputHistory::new(&config);

        // Initially empty
        let entries = history.load_entries();
        assert!(entries.is_empty());

        // Append entries
        history.append_entry("first").unwrap();
        history.append_entry("second").unwrap();
        history.append_entry("third").unwrap();

        // Load and verify
        let entries = history.load_entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].text, "first");
        assert_eq!(entries[1].text, "second");
        assert_eq!(entries[2].text, "third");
    }

    #[test]
    fn test_history_trim_by_entries() {
        let temp_dir = TempDir::new().unwrap();
        // Create config with max_entries=2 and max_size=0 (unlimited)
        let config = create_test_config_with_history(&temp_dir, 0, 2);
        let history = InputHistory::new(&config);

        // Add 5 entries
        for i in 0..5 {
            history.append_entry(format!("entry{}", i)).unwrap();
        }

        // Should only keep last 2
        let entries = history.load_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "entry3");
        assert_eq!(entries[1].text, "entry4");
    }

    #[test]
    fn test_history_trim_by_size() {
        let temp_dir = TempDir::new().unwrap();
        // Create config with max_size=100 bytes and max_entries=0 (unlimited)
        let config = create_test_config_with_history(&temp_dir, 100, 0);
        let history = InputHistory::new(&config);

        // Add entries that will exceed size limit
        history.append_entry("short").unwrap();
        history.append_entry("this is a much longer entry that will take up space").unwrap();
        history.append_entry("another entry").unwrap();

        // File should be trimmed to fit within size limit (allow some tolerance for JSON overhead)
        let metadata = std::fs::metadata(history.path()).unwrap();
        // The limit is approximate since we keep whole entries
        assert!(metadata.len() as usize <= config.history.max_size.saturating_add(100),
            "File size {} should be approximately within limit {}", 
            metadata.len(), config.history.max_size);
    }

    #[test]
    fn test_history_clear() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let history = InputHistory::new(&config);

        history.append_entry("test").unwrap();
        assert!(history.path().exists());

        history.clear().unwrap();
        assert!(!history.path().exists());
    }

    #[test]
    fn test_save_input_convenience() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        save_input("hello", &config).unwrap();
        save_input("world", &config).unwrap();

        let history = InputHistory::new(&config);
        let entries = history.load_entries();
        assert_eq!(entries.len(), 2);
    }
}
