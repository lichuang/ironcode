//! Typed payload builders for each hook event type.
//!
//! These mirror the payloads produced by `kimi_cli.hooks.events` so that
//! user-defined hooks see the same JSON shape regardless of implementation
//! language.

use serde_json::{Value, json};

fn base(event: &str, session_id: &str, cwd: &str) -> Value {
  json!({
    "hook_event_name": event,
    "session_id": session_id,
    "cwd": cwd,
  })
}

/// Payload for the `PreToolUse` event.
pub fn pre_tool_use(
  session_id: &str,
  cwd: &str,
  tool_name: &str,
  tool_input: &Value,
  tool_call_id: &str,
) -> Value {
  let mut payload = base("PreToolUse", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("tool_name".to_string(), json!(tool_name));
    obj.insert("tool_input".to_string(), tool_input.clone());
    obj.insert("tool_call_id".to_string(), json!(tool_call_id));
  }
  payload
}

/// Payload for the `PostToolUse` event.
pub fn post_tool_use(
  session_id: &str,
  cwd: &str,
  tool_name: &str,
  tool_input: &Value,
  tool_output: &str,
  tool_call_id: &str,
) -> Value {
  let mut payload = base("PostToolUse", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("tool_name".to_string(), json!(tool_name));
    obj.insert("tool_input".to_string(), tool_input.clone());
    obj.insert("tool_output".to_string(), json!(tool_output));
    obj.insert("tool_call_id".to_string(), json!(tool_call_id));
  }
  payload
}

/// Payload for the `PostToolUseFailure` event.
pub fn post_tool_use_failure(
  session_id: &str,
  cwd: &str,
  tool_name: &str,
  tool_input: &Value,
  error: &str,
  tool_call_id: &str,
) -> Value {
  let mut payload = base("PostToolUseFailure", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("tool_name".to_string(), json!(tool_name));
    obj.insert("tool_input".to_string(), tool_input.clone());
    obj.insert("error".to_string(), json!(error));
    obj.insert("tool_call_id".to_string(), json!(tool_call_id));
  }
  payload
}

/// Payload for the `UserPromptSubmit` event.
pub fn user_prompt_submit(session_id: &str, cwd: &str, prompt: &str) -> Value {
  let mut payload = base("UserPromptSubmit", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("prompt".to_string(), json!(prompt));
  }
  payload
}

/// Payload for the `Stop` event.
pub fn stop(session_id: &str, cwd: &str, stop_hook_active: bool) -> Value {
  let mut payload = base("Stop", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("stop_hook_active".to_string(), json!(stop_hook_active));
  }
  payload
}

/// Payload for the `StopFailure` event.
#[allow(dead_code)]
pub fn stop_failure(session_id: &str, cwd: &str, error_type: &str, error_message: &str) -> Value {
  let mut payload = base("StopFailure", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("error_type".to_string(), json!(error_type));
    obj.insert("error_message".to_string(), json!(error_message));
  }
  payload
}

/// Payload for the `SessionStart` event.
pub fn session_start(session_id: &str, cwd: &str, source: &str) -> Value {
  let mut payload = base("SessionStart", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("source".to_string(), json!(source));
  }
  payload
}

/// Payload for the `SessionEnd` event.
pub fn session_end(session_id: &str, cwd: &str, reason: &str) -> Value {
  let mut payload = base("SessionEnd", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("reason".to_string(), json!(reason));
  }
  payload
}

/// Payload for the `SubagentStart` event.
#[allow(dead_code)]
pub fn subagent_start(session_id: &str, cwd: &str, agent_name: &str, prompt: &str) -> Value {
  let mut payload = base("SubagentStart", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("agent_name".to_string(), json!(agent_name));
    obj.insert("prompt".to_string(), json!(prompt));
  }
  payload
}

/// Payload for the `SubagentStop` event.
#[allow(dead_code)]
pub fn subagent_stop(session_id: &str, cwd: &str, agent_name: &str, response: &str) -> Value {
  let mut payload = base("SubagentStop", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("agent_name".to_string(), json!(agent_name));
    obj.insert("response".to_string(), json!(response));
  }
  payload
}

/// Payload for the `PreCompact` event.
#[allow(dead_code)]
pub fn pre_compact(session_id: &str, cwd: &str, trigger: &str, token_count: usize) -> Value {
  let mut payload = base("PreCompact", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("trigger".to_string(), json!(trigger));
    obj.insert("token_count".to_string(), json!(token_count));
  }
  payload
}

/// Payload for the `PostCompact` event.
#[allow(dead_code)]
pub fn post_compact(
  session_id: &str,
  cwd: &str,
  trigger: &str,
  estimated_token_count: usize,
) -> Value {
  let mut payload = base("PostCompact", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("trigger".to_string(), json!(trigger));
    obj.insert(
      "estimated_token_count".to_string(),
      json!(estimated_token_count),
    );
  }
  payload
}

/// Payload for the `Notification` event.
#[allow(dead_code)]
pub fn notification(
  session_id: &str,
  cwd: &str,
  sink: &str,
  notification_type: &str,
  title: &str,
  body: &str,
  severity: &str,
) -> Value {
  let mut payload = base("Notification", session_id, cwd);
  if let Some(obj) = payload.as_object_mut() {
    obj.insert("sink".to_string(), json!(sink));
    obj.insert("notification_type".to_string(), json!(notification_type));
    obj.insert("title".to_string(), json!(title));
    obj.insert("body".to_string(), json!(body));
    obj.insert("severity".to_string(), json!(severity));
  }
  payload
}
