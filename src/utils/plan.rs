//! Plan file management for plan mode.
//!
//! Plans are stored as Markdown files in the data directory's `plans/` folder:
//! `{data_dir}/plans/{session_id}.md`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::config::Config;
use crate::config::loader::{data_dir, default_data_dir};

/// Get the plans directory path.
///
/// If `config` is provided, uses `data_dir(config)`; otherwise falls back to
/// `~/.ironcode/plans`.
#[allow(dead_code)]
pub fn plans_dir(config: Option<&Config>) -> PathBuf {
  let base = config
    .map(data_dir)
    .or_else(default_data_dir)
    .unwrap_or_else(|| PathBuf::from(".ironcode"));
  base.join("plans")
}

/// Get the plan file path for a given session ID.
#[allow(dead_code)]
pub fn plan_file_path(session_id: &str, config: Option<&Config>) -> PathBuf {
  plans_dir(config).join(format!("{}.md", session_id))
}

/// Read the plan file for a session.
///
/// Returns `None` if the file does not exist or cannot be read.
#[allow(dead_code)]
pub fn read_plan(session_id: &str, config: Option<&Config>) -> Option<String> {
  let path = plan_file_path(session_id, config);
  fs::read_to_string(&path).ok()
}

/// Write content to the plan file for a session.
///
/// Creates the `plans/` directory if it does not exist.
#[allow(dead_code)]
pub fn write_plan(session_id: &str, content: &str, config: Option<&Config>) -> std::io::Result<()> {
  let dir = plans_dir(config);
  if !dir.exists() {
    fs::create_dir_all(&dir)?;
  }
  let path = dir.join(format!("{}.md", session_id));
  let mut file = fs::OpenOptions::new()
    .create(true)
    .truncate(true)
    .write(true)
    .open(&path)?;
  file.write_all(content.as_bytes())?;
  file.flush()
}

/// Check whether a plan file exists for a session.
#[allow(dead_code)]
pub fn plan_exists(session_id: &str, config: Option<&Config>) -> bool {
  plan_file_path(session_id, config).exists()
}

/// Delete the plan file for a session.
///
/// Returns `true` if a file was deleted, `false` if it did not exist.
#[allow(dead_code)]
pub fn delete_plan(session_id: &str, config: Option<&Config>) -> std::io::Result<bool> {
  let path = plan_file_path(session_id, config);
  if path.exists() {
    fs::remove_file(&path)?;
    Ok(true)
  } else {
    Ok(false)
  }
}
