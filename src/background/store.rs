//! File-system persistence for background tasks.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use log::warn;

use super::models::{
  TaskConsumerState, TaskControl, TaskOutputChunk, TaskRuntime, TaskSpec, TaskStatus, TaskView,
};

/// On-disk store for a single background task.
pub struct BackgroundTaskStore {
  root: PathBuf,
}

const SPEC_FILE: &str = "spec.json";
const RUNTIME_FILE: &str = "runtime.json";
const CONTROL_FILE: &str = "control.json";
const CONSUMER_FILE: &str = "consumer.json";
const OUTPUT_FILE: &str = "output.log";

impl BackgroundTaskStore {
  /// Open (or create) a store rooted at the given directory.
  pub fn new(root: PathBuf) -> Self {
    Self { root }
  }

  pub fn root(&self) -> &Path {
    &self.root
  }

  // -------------------------------------------------------------------------
  // Paths
  // -------------------------------------------------------------------------

  pub fn task_dir(&self, task_id: &str) -> PathBuf {
    let path = self.root.join(task_id);
    let _ = fs::create_dir_all(&path);
    path
  }

  fn spec_path(&self, task_id: &str) -> PathBuf {
    self.task_dir(task_id).join(SPEC_FILE)
  }

  fn runtime_path(&self, task_id: &str) -> PathBuf {
    self.task_dir(task_id).join(RUNTIME_FILE)
  }

  fn control_path(&self, task_id: &str) -> PathBuf {
    self.task_dir(task_id).join(CONTROL_FILE)
  }

  fn consumer_path(&self, task_id: &str) -> PathBuf {
    self.task_dir(task_id).join(CONSUMER_FILE)
  }

  pub fn output_path(&self, task_id: &str) -> PathBuf {
    self.task_dir(task_id).join(OUTPUT_FILE)
  }

  // -------------------------------------------------------------------------
  // CRUD
  // -------------------------------------------------------------------------

  /// Create all on-disk files for a new task.
  pub fn create_task(&self, spec: &TaskSpec) {
    let dir = self.task_dir(&spec.id);
    let _ = atomic_write_json(&dir.join(SPEC_FILE), spec);
    let _ = atomic_write_json(&dir.join(RUNTIME_FILE), &TaskRuntime::default());
    let _ = atomic_write_json(&dir.join(CONTROL_FILE), &TaskControl::default());
    let _ = atomic_write_json(&dir.join(CONSUMER_FILE), &TaskConsumerState::default());
    let _ = fs::OpenOptions::new()
      .create(true)
      .truncate(true)
      .write(true)
      .open(dir.join(OUTPUT_FILE));
  }

  /// Append text to the task output log.
  pub fn append_output(&self, task_id: &str, text: &str) {
    let path = self.output_path(task_id);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
      let _ = file.write_all(text.as_bytes());
    }
  }

  #[allow(dead_code)]
  pub fn write_spec(&self, spec: &TaskSpec) {
    let _ = atomic_write_json(&self.spec_path(&spec.id), spec);
  }

  pub fn read_spec(&self, task_id: &str) -> Option<TaskSpec> {
    read_json(&self.spec_path(task_id))
  }

  pub fn write_runtime(&self, task_id: &str, runtime: &TaskRuntime) {
    let _ = atomic_write_json(&self.runtime_path(task_id), runtime);
  }

  pub fn read_runtime(&self, task_id: &str) -> TaskRuntime {
    read_json(&self.runtime_path(task_id)).unwrap_or_default()
  }

  pub fn write_control(&self, task_id: &str, control: &TaskControl) {
    let _ = atomic_write_json(&self.control_path(task_id), control);
  }

  pub fn read_control(&self, task_id: &str) -> TaskControl {
    read_json(&self.control_path(task_id)).unwrap_or_default()
  }

  #[allow(dead_code)]
  pub fn write_consumer(&self, task_id: &str, consumer: &TaskConsumerState) {
    let _ = atomic_write_json(&self.consumer_path(task_id), consumer);
  }

  pub fn read_consumer(&self, task_id: &str) -> TaskConsumerState {
    read_json(&self.consumer_path(task_id)).unwrap_or_default()
  }

  /// Return a merged view of all task state.
  pub fn merged_view(&self, task_id: &str) -> Option<TaskView> {
    let spec = self.read_spec(task_id)?;
    let runtime = self.read_runtime(task_id);
    let control = self.read_control(task_id);
    let consumer = self.read_consumer(task_id);
    Some(TaskView {
      spec,
      runtime,
      control,
      consumer,
    })
  }

  // -------------------------------------------------------------------------
  // Listing
  // -------------------------------------------------------------------------

  /// List all valid task IDs under the store root.
  pub fn list_task_ids(&self) -> Vec<String> {
    let mut ids = Vec::new();
    if !self.root.exists() {
      return ids;
    }
    if let Ok(entries) = fs::read_dir(&self.root) {
      for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
          && path.join(SPEC_FILE).is_file()
          && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
          ids.push(name.to_string());
        }
      }
    }
    ids
  }

  /// List all task views, sorted by most recently updated first.
  pub fn list_views(&self) -> Vec<TaskView> {
    let mut views = Vec::new();
    for id in self.list_task_ids() {
      if let Some(view) = self.merged_view(&id) {
        views.push(view);
      }
    }
    views.sort_by(|a, b| b.runtime.updated_at.cmp(&a.runtime.updated_at));
    views
  }

  // -------------------------------------------------------------------------
  // Output reading
  // -------------------------------------------------------------------------

  /// Read up to `max_bytes` of output starting at `offset`.
  pub fn read_output(
    &self,
    task_id: &str,
    offset: usize,
    max_bytes: usize,
    status: TaskStatus,
  ) -> TaskOutputChunk {
    let path = self.output_path(task_id);
    if !path.exists() {
      return TaskOutputChunk {
        task_id: task_id.to_string(),
        offset,
        next_offset: offset,
        text: String::new(),
        eof: true,
        status,
      };
    }

    let mut file = match fs::File::open(&path) {
      Ok(f) => f,
      Err(_) => {
        return TaskOutputChunk {
          task_id: task_id.to_string(),
          offset,
          next_offset: offset,
          text: String::new(),
          eof: true,
          status,
        };
      }
    };

    let total_size = match file.seek(std::io::SeekFrom::End(0)) {
      Ok(pos) => pos as usize,
      Err(_) => 0,
    };

    let bounded_offset = offset.min(total_size);
    let _ = file.seek(std::io::SeekFrom::Start(bounded_offset as u64));

    let mut buf = vec![0u8; max_bytes];
    let n = file.read(&mut buf).unwrap_or_default();
    buf.truncate(n);

    let text = String::from_utf8_lossy(&buf).into_owned();
    let next_offset = bounded_offset + n;

    TaskOutputChunk {
      task_id: task_id.to_string(),
      offset: bounded_offset,
      next_offset,
      text,
      eof: next_offset >= total_size,
      status,
    }
  }

  /// Return the last up-to `max_bytes` / `max_lines` of output.
  #[allow(dead_code)]
  pub fn tail_output(&self, task_id: &str, max_bytes: usize, max_lines: usize) -> String {
    let path = self.output_path(task_id);
    if !path.exists() {
      return String::new();
    }

    let mut file = match fs::File::open(&path) {
      Ok(f) => f,
      Err(_) => return String::new(),
    };

    let total_size = match file.seek(std::io::SeekFrom::End(0)) {
      Ok(pos) => pos as usize,
      Err(_) => return String::new(),
    };

    let start = total_size.saturating_sub(max_bytes);
    let _ = file.seek(std::io::SeekFrom::Start(start as u64));

    let mut buf = vec![0u8; total_size - start];
    let _ = file.read_exact(&mut buf);

    let text = String::from_utf8_lossy(&buf).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > max_lines {
      lines[lines.len() - max_lines..].join("\n")
    } else {
      text
    }
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
