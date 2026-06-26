//! Persistent storage for subagent instances.
//!
//! Mirrors kimi-cli's `SubagentStore`: each agent instance gets its own
//! directory under `<session_dir>/subagents/<agent_id>/`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use super::AgentInstanceRecord;

/// File-system backed store for subagent instance data.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SubagentStore {
  root: PathBuf,
}

#[allow(dead_code)]
impl SubagentStore {
  /// Create or open a subagent store rooted at the given session directory.
  pub fn new(session_dir: impl Into<PathBuf>) -> Self {
    Self {
      root: session_dir.into().join("subagents"),
    }
  }

  /// Root path of the store: `<session_dir>/subagents`.
  pub fn root(&self) -> &Path {
    &self.root
  }

  /// Directory for a specific agent instance.
  pub fn agent_dir(&self, agent_id: &str) -> PathBuf {
    self.root.join(agent_id)
  }

  /// Path to the agent's context file.
  pub fn context_path(&self, agent_id: &str) -> PathBuf {
    self.agent_dir(agent_id).join("context.jsonl")
  }

  /// Path to the agent's persisted metadata.
  pub fn meta_path(&self, agent_id: &str) -> PathBuf {
    self.agent_dir(agent_id).join("meta.json")
  }

  /// Path to the prompt snapshot.
  pub fn prompt_path(&self, agent_id: &str) -> PathBuf {
    self.agent_dir(agent_id).join("prompt.txt")
  }

  /// Path to the human-readable output transcript.
  pub fn output_path(&self, agent_id: &str) -> PathBuf {
    self.agent_dir(agent_id).join("output")
  }

  /// Path to the wire message log.
  pub fn wire_path(&self, agent_id: &str) -> PathBuf {
    self.agent_dir(agent_id).join("wire.jsonl")
  }

  /// Ensure the agent instance directory exists.
  pub fn ensure_agent_dir(&self, agent_id: &str) -> anyhow::Result<PathBuf> {
    let dir = self.agent_dir(agent_id);
    fs::create_dir_all(&dir)
      .with_context(|| format!("Failed to create subagent dir: {:?}", dir))?;
    Ok(dir)
  }

  /// Persist an agent instance record.
  pub fn save_meta(&self, record: &AgentInstanceRecord) -> anyhow::Result<()> {
    self.ensure_agent_dir(&record.agent_id)?;
    let path = self.meta_path(&record.agent_id);
    let content = serde_json::to_string_pretty(record)?;
    fs::write(&path, content).with_context(|| format!("Failed to write meta: {:?}", path))?;
    Ok(())
  }

  /// Load an agent instance record.
  pub fn load_meta(&self, agent_id: &str) -> anyhow::Result<Option<AgentInstanceRecord>> {
    let path = self.meta_path(agent_id);
    if !path.exists() {
      return Ok(None);
    }
    let content =
      fs::read_to_string(&path).with_context(|| format!("Failed to read meta: {:?}", path))?;
    let record = serde_json::from_str(&content)
      .with_context(|| format!("Failed to parse meta: {:?}", path))?;
    Ok(Some(record))
  }

  /// Save the prompt snapshot for an agent instance.
  pub fn save_prompt(&self, agent_id: &str, prompt: &str) -> anyhow::Result<()> {
    self.ensure_agent_dir(agent_id)?;
    let path = self.prompt_path(agent_id);
    fs::write(&path, prompt).with_context(|| format!("Failed to write prompt: {:?}", path))?;
    Ok(())
  }

  /// Append a line to the agent's output transcript.
  pub fn append_output(&self, agent_id: &str, line: &str) -> anyhow::Result<()> {
    self.ensure_agent_dir(agent_id)?;
    let path = self.output_path(agent_id);
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
      .create(true)
      .append(true)
      .truncate(false)
      .open(&path)
      .with_context(|| format!("Failed to open output: {:?}", path))?;
    writeln!(file, "{}", line).with_context(|| format!("Failed to write output: {:?}", path))?;
    Ok(())
  }

  /// List all agent ids currently persisted.
  pub fn list_agent_ids(&self) -> anyhow::Result<Vec<String>> {
    if !self.root.exists() {
      return Ok(vec![]);
    }
    let mut ids = Vec::new();
    for entry in fs::read_dir(&self.root)
      .with_context(|| format!("Failed to read subagents dir: {:?}", self.root))?
    {
      let entry = entry?;
      if entry.file_type()?.is_dir() {
        ids.push(entry.file_name().to_string_lossy().to_string());
      }
    }
    Ok(ids)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_store_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SubagentStore::new(tmp.path());
    assert_eq!(
      store.context_path("a123"),
      tmp
        .path()
        .join("subagents")
        .join("a123")
        .join("context.jsonl")
    );
  }

  #[test]
  fn test_save_and_load_meta() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SubagentStore::new(tmp.path());
    let record = AgentInstanceRecord {
      agent_id: "a123".to_string(),
      subagent_type: "coder".to_string(),
      status: "running".to_string(),
      description: "test".to_string(),
      created_at: 1.0,
      updated_at: 2.0,
      last_task_id: None,
      launch_spec: super::super::AgentLaunchSpec {
        agent_id: "a123".to_string(),
        subagent_type: "coder".to_string(),
        model_override: None,
        effective_model: None,
        created_at: 1.0,
      },
    };
    store.save_meta(&record).unwrap();
    let loaded = store.load_meta("a123").unwrap().unwrap();
    assert_eq!(loaded.agent_id, "a123");
    assert_eq!(loaded.subagent_type, "coder");
  }
}
