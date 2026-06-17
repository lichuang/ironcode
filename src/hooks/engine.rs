//! Hook engine — registry and execution orchestrator.
//!
//! Loads hook definitions from config, indexes them by event, and runs all
//! matching hooks in parallel when a lifecycle event fires. Returns the raw
//! list of results so callers can decide how to handle blocks.
//!
//! Supports two hook sources:
//! - Server-side (`config.toml` `[[hooks]]`): shell commands executed locally.
//! - Client-side (wire subscriptions): forwarded to a `WireHookDispatcher` and
//!   resolved when the client responds.
//!
//! Execution model:
//! 1. An event fires with a matcher value (e.g. the tool name for `PreToolUse`).
//! 2. The engine looks up matching server hooks and wire subscriptions.
//! 3. All matches run concurrently.
//! 4. Results are returned as `Vec<HookResult>`; any `Block` decision should be
//!    treated as a block by the caller.
//!
//! This design mirrors `kimi_cli.hooks.engine`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

use super::config::{HookDef, HookEventType};
use super::runner::{HookDecision, HookResult, run_hook};

/// A client-side hook subscription registered via wire initialize.
///
/// Unlike `HookDef`, wire subscriptions do not contain a shell command.
/// Instead they identify an external client that will receive the hook request
/// and decide whether to allow or block the operation.
#[derive(Debug, Clone)]
pub struct WireHookSubscription {
  /// Subscription identifier, used to correlate requests and responses.
  pub id: String,
  /// Which lifecycle event triggers this subscription.
  pub event: HookEventType,
  /// Regex pattern to filter targets. Empty matches everything.
  pub matcher: String,
  /// Timeout in seconds. If the client does not respond in time the hook is
  /// treated as `Allow` (fail-open).
  pub timeout: u64,
}

impl WireHookSubscription {
  /// Create a new wire hook subscription.
  ///
  /// Use `with_matcher` and `with_timeout` to customize filtering and timeout.
  #[allow(dead_code)]
  pub fn new(id: impl Into<String>, event: HookEventType) -> Self {
    Self {
      id: id.into(),
      event,
      matcher: String::new(),
      timeout: 30,
    }
  }

  /// Set the matcher regex.
  #[allow(dead_code)]
  pub fn with_matcher(mut self, matcher: impl Into<String>) -> Self {
    self.matcher = matcher.into();
    self
  }

  /// Set the timeout in seconds.
  #[allow(dead_code)]
  pub fn with_timeout(mut self, timeout: u64) -> Self {
    self.timeout = timeout;
    self
  }
}

/// A pending wire hook request waiting for the client to respond.
///
/// Acts as a request/response handle. The dispatcher sends the public fields
/// to the client; the client later calls `resolve(action, reason)` to complete
/// the request. The engine waits on `wait()`.
///
/// The handle can be cloned safely. Both clones share the same underlying
/// oneshot channel, so either side may consume the sender/receiver exactly
/// once.
#[derive(Clone)]
pub struct WireHookHandle {
  /// Request identifier, unique for this trigger invocation.
  pub id: String,
  /// Originating subscription identifier.
  pub subscription_id: String,
  /// Event that triggered the hook.
  pub event: HookEventType,
  /// Matcher value (target) the hook was triggered against.
  pub target: String,
  /// Input payload sent to the client.
  pub input_data: Value,
  /// Sender used by the client to resolve the hook.
  tx: Arc<Mutex<Option<oneshot::Sender<HookResult>>>>,
  /// Receiver used by the engine to await the client's decision.
  rx: Arc<Mutex<Option<oneshot::Receiver<HookResult>>>>,
}

impl std::fmt::Debug for WireHookHandle {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("WireHookHandle")
      .field("id", &self.id)
      .field("subscription_id", &self.subscription_id)
      .field("event", &self.event)
      .field("target", &self.target)
      .field("input_data", &self.input_data)
      .finish_non_exhaustive()
  }
}

impl WireHookHandle {
  /// Create a new pending wire hook handle.
  pub fn new(
    id: impl Into<String>,
    subscription_id: impl Into<String>,
    event: HookEventType,
    target: impl Into<String>,
    input_data: Value,
  ) -> Self {
    let (tx, rx) = oneshot::channel();
    Self {
      id: id.into(),
      subscription_id: subscription_id.into(),
      event,
      target: target.into(),
      input_data,
      tx: Arc::new(Mutex::new(Some(tx))),
      rx: Arc::new(Mutex::new(Some(rx))),
    }
  }

  /// Wait for the client to resolve this hook.
  ///
  /// Returns `Allow` if the channel is already consumed or the sender is
  /// dropped without sending a result.
  pub async fn wait(&self) -> HookResult {
    let rx = self.rx.lock().await.take();
    match rx {
      Some(rx) => rx.await.unwrap_or_else(|_| HookResult::allow()),
      None => HookResult::allow(),
    }
  }

  /// Resolve the hook with the client's decision.
  ///
  /// `action` should be `"allow"` or `"block"`. Any other value is treated
  /// as allow. If the channel has already been consumed this call is a no-op.
  #[allow(dead_code)]
  pub fn resolve(&self, action: &str, reason: &str) {
    let decision = if action == "block" {
      HookDecision::Block {
        reason: reason.to_string(),
      }
    } else {
      HookDecision::Allow
    };
    let result = HookResult {
      decision,
      stdout: String::new(),
      stderr: String::new(),
      exit_code: None,
      timed_out: false,
    };
    if let Ok(mut guard) = self.tx.try_lock()
      && let Some(tx) = guard.take()
    {
      let _ = tx.send(result);
    }
  }
}

/// Dispatcher for client-side wire hooks.
///
/// Implementations are responsible for transporting the `WireHookHandle` to
/// the external client and arranging for `handle.resolve(...)` to be called
/// when the client makes a decision. The engine handles timeouts.
#[async_trait]
pub trait WireHookDispatcher: Send + Sync {
  /// Send the hook request to the client.
  ///
  /// The implementation should eventually call `handle.resolve(action, reason)`
  /// on the same or a cloned handle. If it never resolves, the engine will
  /// time out and treat the hook as `Allow`.
  async fn dispatch_wire_hook(&self, handle: WireHookHandle);
}

/// Metadata about a registered hook for display or debugging.
///
/// Returned by `HookEngine::details()` grouped by event. Useful for UI
/// listings such as `/hooks` slash commands or status panels.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HookDetail {
  /// Source of the hook (`server` or `wire`).
  pub source: &'static str,
  /// Regex matcher, or `"(all)"` if empty.
  pub matcher: String,
  /// Server-side command, or `"(client-side)"` for wire hooks.
  pub command: String,
}

/// Telemetry callback signatures.
///
/// - `OnHookTriggered`: `(event, matcher_value, total_hook_count)`
/// - `OnHookResolved`: `(event, matcher_value, action, reason, duration_ms)`
pub type OnHookTriggered = Arc<dyn Fn(HookEventType, &str, usize) + Send + Sync>;
pub type OnHookResolved = Arc<dyn Fn(HookEventType, &str, &str, &str, u64) + Send + Sync>;

/// In-process hook registry and executor.
///
/// Keeps server-side hooks and wire subscriptions in separate indexes so that
/// trigger lookups are O(1) by event. All mutations to the dispatcher and
/// callbacks use interior mutability (`RwLock`) so that `HookEngine` can be
/// cloned cheaply and shared across async tasks.
#[derive(Clone)]
pub struct HookEngine {
  /// Server-side hook definitions loaded from config.
  hooks: Vec<HookDef>,
  /// Client-side hook subscriptions registered via wire initialize.
  wire_subs: Vec<WireHookSubscription>,
  /// Working directory for server-side shell commands.
  cwd: Option<PathBuf>,
  /// Index of server hooks by event.
  index: HashMap<HookEventType, Vec<HookDef>>,
  /// Index of wire subscriptions by event.
  wire_index: HashMap<HookEventType, Vec<WireHookSubscription>>,
  /// Dispatcher for client-side hooks. Set at runtime after the wire bus is
  /// created.
  dispatcher: Arc<RwLock<Option<Arc<dyn WireHookDispatcher>>>>,
  /// Optional callback invoked before hooks start executing.
  on_triggered: Arc<RwLock<Option<OnHookTriggered>>>,
  /// Optional callback invoked after all hooks have resolved.
  on_resolved: Arc<RwLock<Option<OnHookResolved>>>,
}

impl std::fmt::Debug for HookEngine {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("HookEngine")
      .field("hooks", &self.hooks.len())
      .field("wire_subs", &self.wire_subs.len())
      .field("cwd", &self.cwd)
      .finish_non_exhaustive()
  }
}

impl HookEngine {
  /// Create a new engine from hook definitions.
  pub fn new(hooks: Vec<HookDef>, cwd: Option<PathBuf>) -> Self {
    let mut engine = Self {
      hooks,
      wire_subs: Vec::new(),
      cwd,
      index: HashMap::new(),
      wire_index: HashMap::new(),
      dispatcher: Arc::new(RwLock::new(None)),
      on_triggered: Arc::new(RwLock::new(None)),
      on_resolved: Arc::new(RwLock::new(None)),
    };
    engine.rebuild_index();
    engine
  }

  /// Create an empty engine.
  #[allow(dead_code)]
  pub fn empty() -> Self {
    Self::new(Vec::new(), None)
  }

  /// Add server-side hooks at runtime. Rebuilds index.
  #[allow(dead_code)]
  pub fn add_hooks(&mut self, hooks: Vec<HookDef>) {
    self.hooks.extend(hooks);
    self.rebuild_index();
  }

  /// Register client-side hook subscriptions from wire initialize.
  #[allow(dead_code)]
  pub fn add_wire_subscriptions(&mut self, subs: Vec<WireHookSubscription>) {
    self.wire_subs.extend(subs);
    self.rebuild_index();
  }

  /// Set the dispatcher used for client-side wire hooks.
  #[allow(dead_code)]
  pub fn set_dispatcher(&self, dispatcher: Arc<dyn WireHookDispatcher>) {
    *self.dispatcher.write().expect("dispatcher lock poisoned") = Some(dispatcher);
  }

  /// Set the callback invoked when hooks are triggered.
  #[allow(dead_code)]
  pub fn set_on_triggered(&self, callback: OnHookTriggered) {
    *self.on_triggered.write().expect("triggered lock poisoned") = Some(callback);
  }

  /// Set the callback invoked when all hooks have resolved.
  #[allow(dead_code)]
  pub fn set_on_resolved(&self, callback: OnHookResolved) {
    *self.on_resolved.write().expect("resolved lock poisoned") = Some(callback);
  }

  /// Returns true if any hooks are registered.
  #[allow(dead_code)]
  pub fn has_hooks(&self) -> bool {
    !self.hooks.is_empty() || !self.wire_subs.is_empty()
  }

  /// Returns true if any hooks match the given event.
  pub fn has_hooks_for(&self, event: HookEventType) -> bool {
    self.index.get(&event).is_some_and(|v| !v.is_empty())
      || self.wire_index.get(&event).is_some_and(|v| !v.is_empty())
  }

  /// Event -> count of registered hooks (server + wire).
  #[allow(dead_code)]
  pub fn summary(&self) -> HashMap<HookEventType, usize> {
    let mut counts: HashMap<HookEventType, usize> = self
      .index
      .iter()
      .map(|(event, hooks)| (*event, hooks.len()))
      .collect();
    for (event, subs) in &self.wire_index {
      *counts.entry(*event).or_insert(0) += subs.len();
    }
    counts
  }

  /// Detailed listing of all registered hooks, grouped by event.
  #[allow(dead_code)]
  pub fn details(&self) -> HashMap<HookEventType, Vec<HookDetail>> {
    let mut result: HashMap<HookEventType, Vec<HookDetail>> = HashMap::new();
    for (event, hooks) in &self.index {
      let entries = result.entry(*event).or_default();
      for hook in hooks {
        entries.push(HookDetail {
          source: "server",
          matcher: if hook.matcher.is_empty() {
            "(all)".to_string()
          } else {
            hook.matcher.clone()
          },
          command: hook.command.clone(),
        });
      }
    }
    for (event, subs) in &self.wire_index {
      let entries = result.entry(*event).or_default();
      for sub in subs {
        entries.push(HookDetail {
          source: "wire",
          matcher: if sub.matcher.is_empty() {
            "(all)".to_string()
          } else {
            sub.matcher.clone()
          },
          command: "(client-side)".to_string(),
        });
      }
    }
    result
  }

  /// Rebuild both event indexes after hooks or subscriptions change.
  fn rebuild_index(&mut self) {
    self.index.clear();
    for hook in &self.hooks {
      self.index.entry(hook.event).or_default().push(hook.clone());
    }
    self.wire_index.clear();
    for sub in &self.wire_subs {
      self
        .wire_index
        .entry(sub.event)
        .or_default()
        .push(sub.clone());
    }
  }

  /// Trigger all matching hooks for an event and return their results.
  ///
  /// Server-side hooks are executed as shell commands; wire subscriptions are
  /// forwarded to the configured dispatcher. All matches run concurrently.
  /// Callers are responsible for checking whether any result has
  /// `HookDecision::Block`.
  ///
  /// Returns an empty vector if no hooks match so callers can skip logging.
  pub async fn trigger(
    &self,
    event: HookEventType,
    matcher_value: &str,
    input_data: Value,
  ) -> Vec<HookResult> {
    let target = matcher_value.to_string();
    let server_matched = self.match_server_hooks(event, &target);
    let wire_matched = self.match_wire_hooks(event, &target);

    let total = server_matched.len() + wire_matched.len();
    if total == 0 {
      return Vec::new();
    }

    let on_triggered = self
      .on_triggered
      .read()
      .expect("triggered lock poisoned")
      .clone();
    if let Some(cb) = on_triggered {
      cb(event, matcher_value, total);
    }

    let mut tasks = Vec::with_capacity(total);
    let start = Instant::now();

    // Spawn one task per matching server-side hook.
    for hook in server_matched {
      let cwd = self.cwd.as_ref().map(|p| p.to_string_lossy().to_string());
      let input_data = input_data.clone();
      tasks.push(tokio::spawn(async move {
        run_hook(&hook.command, &input_data, hook.timeout, cwd.as_deref()).await
      }));
    }

    // Spawn one task per matching wire subscription. The dispatcher is cloned
    // so the engine remains independent of any specific transport.
    let dispatcher = self
      .dispatcher
      .read()
      .expect("dispatcher lock poisoned")
      .clone();
    for sub in wire_matched {
      let input_data = input_data.clone();
      let dispatcher = dispatcher.clone();
      let target = target.clone();
      tasks.push(tokio::spawn(async move {
        Self::dispatch_wire_hook(dispatcher, &sub, event, &target, input_data).await
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

    let (action, reason) = aggregate_action(&results);
    if let HookDecision::Block { ref reason } = action {
      log::warn!(
        "Hook blocked {} (matcher={}): {}",
        event,
        matcher_value,
        reason
      );
    }

    let on_resolved = self
      .on_resolved
      .read()
      .expect("resolved lock poisoned")
      .clone();
    if let Some(cb) = on_resolved {
      cb(
        event,
        matcher_value,
        action_str(&action),
        reason,
        start.elapsed().as_millis() as u64,
      );
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

  /// Find matching server-side hooks for an event.
  ///
  /// Filters by regex matcher and deduplicates by command so that the same
  /// shell command is never executed twice for the same trigger.
  fn match_server_hooks(&self, event: HookEventType, matcher_value: &str) -> Vec<HookDef> {
    let Some(hooks) = self.index.get(&event) else {
      return Vec::new();
    };

    let mut seen: HashSet<&str> = HashSet::new();
    hooks
      .iter()
      .filter(|hook| match_regex(&hook.matcher, matcher_value))
      .filter(|hook| seen.insert(&hook.command))
      .cloned()
      .collect()
  }

  /// Find matching wire subscriptions for an event.
  fn match_wire_hooks(
    &self,
    event: HookEventType,
    matcher_value: &str,
  ) -> Vec<WireHookSubscription> {
    let Some(subs) = self.wire_index.get(&event) else {
      return Vec::new();
    };

    subs
      .iter()
      .filter(|sub| match_regex(&sub.matcher, matcher_value))
      .cloned()
      .collect()
  }

  /// Dispatch a single wire hook and wait for the client decision.
  ///
  /// Spawns the dispatcher in its own task so that the dispatcher's own I/O
  /// (e.g. sending over a network) does not starve the timeout. If the client
  /// does not resolve the handle before `sub.timeout`, the result is `Allow`.
  async fn dispatch_wire_hook(
    dispatcher: Option<Arc<dyn WireHookDispatcher>>,
    sub: &WireHookSubscription,
    event: HookEventType,
    target: &str,
    input_data: Value,
  ) -> HookResult {
    // No dispatcher registered yet: fail-open.
    let Some(dispatcher) = dispatcher else {
      return HookResult::allow();
    };

    let handle = WireHookHandle::new(
      format!("wh-{}", rand::random::<u64>()),
      &sub.id,
      event,
      target,
      input_data,
    );

    // Run the dispatcher in a detached task. The timeout applies to the full
    // round-trip (dispatch + client response), not just the dispatch call.
    let dispatch_task = tokio::spawn({
      let dispatcher = dispatcher.clone();
      let handle = handle.clone();
      async move { dispatcher.dispatch_wire_hook(handle).await }
    });

    match timeout(Duration::from_secs(sub.timeout), handle.wait()).await {
      Ok(result) => result,
      Err(_) => {
        log::warn!(
          "Wire hook timed out after {}s: {} {}",
          sub.timeout,
          event,
          target
        );
        dispatch_task.abort();
        HookResult {
          decision: HookDecision::Allow,
          stdout: String::new(),
          stderr: String::new(),
          exit_code: None,
          timed_out: true,
        }
      }
    }
  }
}

/// Aggregate a list of hook results into a single decision.
///
/// The first `Block` wins; its reason is returned. If no hook blocks, the
/// result is `Allow` with an empty reason.
fn aggregate_action(results: &[HookResult]) -> (HookDecision, &str) {
  for result in results {
    if let HookDecision::Block { ref reason } = result.decision {
      return (
        HookDecision::Block {
          reason: if reason.is_empty() {
            String::new()
          } else {
            reason.clone()
          },
        },
        reason.as_str(),
      );
    }
  }
  (HookDecision::Allow, "")
}

/// Convert a decision back into the action string used by callbacks.
fn action_str(decision: &HookDecision) -> &'static str {
  match decision {
    HookDecision::Allow => "allow",
    HookDecision::Block { .. } => "block",
  }
}

/// Match a hook matcher regex against a value.
///
/// An empty pattern matches everything. Invalid patterns are logged and treated
/// as non-matching (fail-closed for that specific hook).
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
  use std::sync::atomic::{AtomicUsize, Ordering};

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

  #[tokio::test]
  async fn test_duplicate_commands_are_deduped() {
    let engine = HookEngine::new(
      vec![
        HookDef::new(HookEventType::PreToolUse, "exit 2"),
        HookDef::new(HookEventType::PreToolUse, "exit 2"),
      ],
      None,
    );
    let results = engine
      .trigger(HookEventType::PreToolUse, "ReadFile", json!({}))
      .await;
    assert_eq!(results.len(), 1);
  }

  #[tokio::test]
  async fn test_trigger_callbacks_fire() {
    let triggered = Arc::new(AtomicUsize::new(0));
    let resolved = Arc::new(AtomicUsize::new(0));

    let t = triggered.clone();
    let r = resolved.clone();

    let mut engine = HookEngine::new(
      vec![HookDef::new(HookEventType::PreToolUse, "exit 0")],
      None,
    );
    engine.set_on_triggered(Arc::new(move |_, _, _| {
      t.fetch_add(1, Ordering::SeqCst);
    }));
    engine.set_on_resolved(Arc::new(move |_, _, _, _, _| {
      r.fetch_add(1, Ordering::SeqCst);
    }));

    let _ = engine
      .trigger(HookEventType::PreToolUse, "ReadFile", json!({}))
      .await;

    assert_eq!(triggered.load(Ordering::SeqCst), 1);
    assert_eq!(resolved.load(Ordering::SeqCst), 1);
  }

  #[test]
  fn test_details() {
    let mut engine = HookEngine::new(
      vec![HookDef::new(HookEventType::PreToolUse, "exit 2").with_matcher("^Write")],
      None,
    );
    engine.add_wire_subscriptions(vec![WireHookSubscription::new(
      "sub-1",
      HookEventType::PreToolUse,
    )]);

    let details = engine.details();
    let entries = details.get(&HookEventType::PreToolUse).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|d| d.source == "server"));
    assert!(entries.iter().any(|d| d.source == "wire"));
  }

  struct MockDispatcher {
    action: &'static str,
    reason: &'static str,
  }

  #[async_trait]
  impl WireHookDispatcher for MockDispatcher {
    async fn dispatch_wire_hook(&self, handle: WireHookHandle) {
      handle.resolve(self.action, self.reason);
    }
  }

  #[tokio::test]
  async fn test_wire_subscription_allow() {
    let mut engine = HookEngine::new(Vec::new(), None);
    engine.add_wire_subscriptions(vec![WireHookSubscription::new(
      "sub-1",
      HookEventType::PreToolUse,
    )]);
    engine.set_dispatcher(Arc::new(MockDispatcher {
      action: "allow",
      reason: "",
    }));

    let results = engine
      .trigger(HookEventType::PreToolUse, "ReadFile", json!({}))
      .await;
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].decision, HookDecision::Allow));
  }

  #[tokio::test]
  async fn test_wire_subscription_block() {
    let mut engine = HookEngine::new(Vec::new(), None);
    engine.add_wire_subscriptions(vec![WireHookSubscription::new(
      "sub-1",
      HookEventType::PreToolUse,
    )]);
    engine.set_dispatcher(Arc::new(MockDispatcher {
      action: "block",
      reason: "denied by client",
    }));

    let results = engine
      .trigger(HookEventType::PreToolUse, "ReadFile", json!({}))
      .await;
    assert_eq!(results.len(), 1);
    assert_eq!(
      results[0].decision,
      HookDecision::Block {
        reason: "denied by client".to_string()
      }
    );
  }

  #[tokio::test]
  async fn test_wire_subscription_timeout() {
    struct SlowDispatcher;
    #[async_trait]
    impl WireHookDispatcher for SlowDispatcher {
      async fn dispatch_wire_hook(&self, _handle: WireHookHandle) {
        tokio::time::sleep(Duration::from_secs(10)).await;
      }
    }

    let mut engine = HookEngine::new(Vec::new(), None);
    engine.add_wire_subscriptions(vec![
      WireHookSubscription::new("sub-1", HookEventType::PreToolUse).with_timeout(1),
    ]);
    engine.set_dispatcher(Arc::new(SlowDispatcher));

    let results = engine
      .trigger(HookEventType::PreToolUse, "ReadFile", json!({}))
      .await;
    assert_eq!(results.len(), 1);
    assert!(results[0].timed_out);
    assert!(matches!(results[0].decision, HookDecision::Allow));
  }
}
