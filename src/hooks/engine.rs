//! Hook engine — registry and execution orchestrator.
//!
//! Loads hook definitions from config, indexes them by event, and runs all
//! matching hooks in parallel when a lifecycle event fires. Returns the raw
//! list of results so callers can decide how to handle blocks.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;
use tokio::task::JoinHandle;

use super::config::{HookDef, HookEventType};
use super::runner::{HookDecision, HookResult, run_hook};

/// In-process hook registry and executor.
#[derive(Debug, Clone)]
pub struct HookEngine {
  hooks: Vec<HookDef>,
  cwd: Option<PathBuf>,
  index: HashMap<HookEventType, Vec<HookDef>>,
}

impl HookEngine {
  /// Create a new engine from hook definitions.
  pub fn new(hooks: Vec<HookDef>, cwd: Option<PathBuf>) -> Self {
    let mut engine = Self {
      hooks,
      cwd,
      index: HashMap::new(),
    };
    engine.rebuild_index();
    engine
  }

  /// Create an empty engine.
  #[allow(dead_code)]
  pub fn empty() -> Self {
    Self::new(Vec::new(), None)
  }

  /// Add hooks at runtime and rebuild the index.
  #[allow(dead_code)]
  pub fn add_hooks(&mut self, hooks: Vec<HookDef>) {
    self.hooks.extend(hooks);
    self.rebuild_index();
  }

  /// Returns true if any hooks are registered.
  #[allow(dead_code)]
  pub fn has_hooks(&self) -> bool {
    !self.hooks.is_empty()
  }

  /// Returns true if any hooks match the given event.
  pub fn has_hooks_for(&self, event: HookEventType) -> bool {
    self.index.get(&event).is_some_and(|v| !v.is_empty())
  }

  /// Event -> count of registered hooks.
  #[allow(dead_code)]
  pub fn summary(&self) -> HashMap<HookEventType, usize> {
    self
      .index
      .iter()
      .map(|(event, hooks)| (*event, hooks.len()))
      .collect()
  }

  fn rebuild_index(&mut self) {
    self.index.clear();
    for hook in &self.hooks {
      self.index.entry(hook.event).or_default().push(hook.clone());
    }
  }

  /// Trigger all matching hooks for an event and return their results.
  ///
  /// Hooks are executed in parallel. Callers are responsible for checking
  /// whether any result has `HookDecision::Block`.
  pub async fn trigger(
    &self,
    event: HookEventType,
    matcher_value: &str,
    input_data: Value,
  ) -> Vec<HookResult> {
    let matched = self.match_hooks(event, matcher_value);
    if matched.is_empty() {
      return Vec::new();
    }

    let mut tasks = Vec::with_capacity(matched.len());
    for hook in matched {
      let cwd = self.cwd.as_ref().map(|p| p.to_string_lossy().to_string());
      let input_data = input_data.clone();
      tasks.push(tokio::spawn(async move {
        run_hook(&hook.command, &input_data, hook.timeout, cwd.as_deref()).await
      }));
    }

    let mut results = Vec::with_capacity(tasks.len());
    for task in tasks {
      match task.await {
        Ok(result) => results.push(result),
        Err(e) => {
          log::warn!("Hook task panicked for {}: {}", event, e);
          results.push(HookResult::allow());
        }
      }
    }

    for result in &results {
      if let HookDecision::Block { ref reason } = result.decision {
        log::warn!(
          "Hook blocked {} (matcher={}): {}",
          event,
          matcher_value,
          reason
        );
      }
    }

    results
  }

  /// Trigger hooks without waiting for the result (fire-and-forget).
  ///
  /// Useful for events like `PostToolUse` where the result does not affect
  /// control flow.
  pub fn trigger_fire_and_forget(
    &self,
    event: HookEventType,
    matcher_value: String,
    input_data: Value,
  ) -> JoinHandle<Vec<HookResult>> {
    let engine = self.clone();
    tokio::spawn(async move { engine.trigger(event, &matcher_value, input_data).await })
  }

  fn match_hooks(&self, event: HookEventType, matcher_value: &str) -> Vec<HookDef> {
    let Some(hooks) = self.index.get(&event) else {
      return Vec::new();
    };

    hooks
      .iter()
      .filter(|hook| match_regex(&hook.matcher, matcher_value))
      .cloned()
      .collect()
  }
}

fn match_regex(pattern: &str, value: &str) -> bool {
  if pattern.is_empty() {
    return true;
  }

  match regex::Regex::new(pattern) {
    Ok(re) => re.is_match(value),
    Err(e) => {
      log::warn!("Invalid hook matcher regex '{}': {}", pattern, e);
      false
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn test_engine_indexes_hooks() {
    let engine = HookEngine::new(
      vec![
        HookDef::new(HookEventType::PreToolUse, "exit 2"),
        HookDef::new(HookEventType::UserPromptSubmit, "cat"),
      ],
      None,
    );
    assert!(engine.has_hooks_for(HookEventType::PreToolUse));
    assert!(engine.has_hooks_for(HookEventType::UserPromptSubmit));
    assert!(!engine.has_hooks_for(HookEventType::Stop));
  }

  #[tokio::test]
  async fn test_trigger_blocks_when_any_hook_blocks() {
    let engine = HookEngine::new(
      vec![
        HookDef::new(HookEventType::PreToolUse, "exit 0"),
        HookDef::new(HookEventType::PreToolUse, "echo 'nope' >&2; exit 2"),
      ],
      None,
    );
    let results = engine
      .trigger(HookEventType::PreToolUse, "ReadFile", json!({}))
      .await;
    assert!(
      results
        .iter()
        .any(|r| matches!(r.decision, HookDecision::Block { .. }))
    );
  }

  #[tokio::test]
  async fn test_trigger_allows_when_no_block() {
    let engine = HookEngine::new(
      vec![HookDef::new(HookEventType::PreToolUse, "exit 0")],
      None,
    );
    let results = engine
      .trigger(HookEventType::PreToolUse, "ReadFile", json!({}))
      .await;
    assert!(
      results
        .iter()
        .all(|r| matches!(r.decision, HookDecision::Allow))
    );
  }

  #[tokio::test]
  async fn test_matcher_filters_hooks() {
    let engine = HookEngine::new(
      vec![
        HookDef::new(HookEventType::PreToolUse, "exit 2").with_matcher("^Write"),
        HookDef::new(HookEventType::PreToolUse, "exit 0"),
      ],
      None,
    );
    let results = engine
      .trigger(HookEventType::PreToolUse, "ReadFile", json!({}))
      .await;
    assert!(
      results
        .iter()
        .all(|r| matches!(r.decision, HookDecision::Allow))
    );

    let results = engine
      .trigger(HookEventType::PreToolUse, "WriteFile", json!({}))
      .await;
    assert!(
      results
        .iter()
        .any(|r| matches!(r.decision, HookDecision::Block { .. }))
    );
  }
}
