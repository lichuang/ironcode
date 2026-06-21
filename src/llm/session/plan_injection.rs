//! Plan mode dynamic reminder injection.
//!
//! Mirrors kimi-cli's `soul/dynamic_injections/plan_mode.py`: while plan mode is
//! active, periodically inject reminder messages into the context so the model
//! remembers it can only read and edit the plan file.

use std::path::PathBuf;

use crate::llm::types::Message;

/// Inject a reminder every N assistant turns.
const TURN_INTERVAL: usize = 5;
/// Every N-th reminder is the full version; others are sparse.
const FULL_EVERY_N: usize = 5;

/// A dynamic reminder to be appended to the context as a user message.
#[derive(Debug, Clone)]
pub struct PlanModeReminder {
  /// Text content (will be wrapped in `<system-reminder>` tags).
  pub content: String,
}

/// State carried by the actor for plan-mode reminder throttling.
#[derive(Debug, Default)]
pub struct PlanModeInjectionState {
  inject_count: usize,
  pending_activation: bool,
}

impl PlanModeInjectionState {
  /// Schedule a one-shot activation reminder for the next LLM step.
  pub fn schedule_activation(&mut self) {
    self.pending_activation = true;
  }

  /// Consume the pending activation flag.
  pub fn consume_pending_activation(&mut self) -> bool {
    let pending = self.pending_activation;
    self.pending_activation = false;
    pending
  }

  /// Collect a reminder if one is due for the current plan mode state.
  pub fn collect(
    &mut self,
    plan_mode: bool,
    plan_file_path: Option<&PathBuf>,
    history: &[Message],
  ) -> Option<PlanModeReminder> {
    if !plan_mode {
      self.inject_count = 0;
      self.pending_activation = false;
      return None;
    }

    let plan_exists = plan_file_path.is_some_and(|p| p.exists());
    let plan_path_str = plan_file_path.map(|p| p.to_string_lossy().to_string());

    // Manual toggles / restored sessions schedule a one-shot activation reminder.
    if self.consume_pending_activation() {
      self.inject_count = 1;
      if plan_exists {
        return Some(PlanModeReminder {
          content: reentry_reminder(plan_path_str.as_deref()),
        });
      }
      return Some(PlanModeReminder {
        content: full_reminder(plan_path_str.as_deref(), plan_exists),
      });
    }

    // Scan history backwards to find the last plan mode reminder and count
    // assistant messages since then.
    let mut turns_since_last = 0;
    let mut found_previous = false;
    for msg in history.iter().rev() {
      if msg.role == crate::llm::types::Role::User && has_plan_reminder(msg) {
        found_previous = true;
        break;
      }
      if msg.role == crate::llm::types::Role::Assistant {
        turns_since_last += 1;
      }
    }

    // First time (no reminder in history yet) -> inject full version.
    if !found_previous {
      self.inject_count = 1;
      return Some(PlanModeReminder {
        content: full_reminder(plan_path_str.as_deref(), plan_exists),
      });
    }

    // Not enough turns since last reminder -> skip.
    if turns_since_last < TURN_INTERVAL {
      return None;
    }

    self.inject_count += 1;
    let is_full = self.inject_count % FULL_EVERY_N == 1;
    let content = if is_full {
      full_reminder(plan_path_str.as_deref(), plan_exists)
    } else {
      sparse_reminder(plan_path_str.as_deref())
    };

    Some(PlanModeReminder { content })
  }
}

/// Check whether a message contains a plan mode reminder.
fn has_plan_reminder(msg: &Message) -> bool {
  let keys = [
    sparse_reminder(None)
      .split('.')
      .next()
      .unwrap_or("")
      .to_string(),
    full_reminder(None, false)
      .split('\n')
      .next()
      .unwrap_or("")
      .to_string(),
  ];
  msg
    .content
    .lines()
    .any(|line| keys.iter().any(|key| line.contains(key)))
}

fn full_reminder(plan_file_path: Option<&str>, plan_exists: bool) -> String {
  let mut lines: Vec<String> = Vec::new();
  lines.push(
    "Plan mode is active. You MUST NOT make any edits (with the exception of the plan file below), \
     run non-readonly tools, or otherwise make changes to the system. \
     This supersedes any other instructions you have received."
      .to_string(),
  );

  if let Some(path) = plan_file_path {
    lines.push(String::new());
    if plan_exists {
      lines.push(format!(
        "Plan file: {path} (exists — read first, then update it with WriteFile or StrReplaceFile)"
      ));
    } else {
      lines.push(format!(
        "Plan file: {path} (create it with WriteFile; once it exists, you can modify it with \
         WriteFile or StrReplaceFile)"
      ));
    }
    lines.push("This is the only file you are allowed to edit.".to_string());
  }

  lines.extend([
    String::new(),
    "Workflow:".to_string(),
    "1. Understand — explore the codebase with Glob, Grep, ReadFile".to_string(),
    "2. Design — converge on the best approach; consider trade-offs but aim for a single recommendation".to_string(),
    "3. Review — re-read key files to verify understanding".to_string(),
    "4. Write Plan — modify the plan file with WriteFile or StrReplaceFile. Use WriteFile if the plan file does not exist yet".to_string(),
    "5. Exit — call ExitPlanMode for user approval".to_string(),
    String::new(),
    "## Handling multiple approaches".to_string(),
    "Keep it focused: at most 2-3 meaningfully different approaches. Do NOT pad with minor variations — if one approach is clearly superior, just propose that one.".to_string(),
    "When the best approach depends on user preferences, constraints, or context you don't have, use AskUserQuestion to clarify first. This helps you write a better, more targeted plan rather than dumping multiple options for the user to sort through.".to_string(),
    "When you do include multiple approaches in the plan, you MUST pass them as the `options` parameter when calling ExitPlanMode, so the user can select which approach to execute at approval time.".to_string(),
    "NEVER write multiple approaches in the plan and call ExitPlanMode without the `options` parameter — the user will only see Approve/Reject with no way to choose.".to_string(),
    String::new(),
    "AskUserQuestion is for clarifying missing requirements or user preferences that affect the plan.".to_string(),
    "Never ask about plan approval via text or AskUserQuestion.".to_string(),
    "Your turn must end with either AskUserQuestion (to clarify requirements or preferences) or ExitPlanMode (to request plan approval). Do NOT end your turn any other way.".to_string(),
    "Do NOT use AskUserQuestion to ask about plan approval or reference \"the plan\" — the user cannot see the plan until you call ExitPlanMode.".to_string(),
  ]);

  lines.join("\n")
}

fn sparse_reminder(plan_file_path: Option<&str>) -> String {
  let mut parts = vec!["Plan mode still active (see full instructions earlier).".to_string()];
  if let Some(path) = plan_file_path {
    parts.push(format!("Read-only except plan file ({path})."));
  } else {
    parts.push("Read-only.".to_string());
  }
  parts.extend([
    "Use WriteFile or StrReplaceFile to modify the plan file. If it does not exist yet, create it with WriteFile first.".to_string(),
    "Use AskUserQuestion to clarify user preferences when it helps you write a better plan.".to_string(),
    "If the plan has multiple approaches, pass options to ExitPlanMode so the user can choose.".to_string(),
    "End turns with AskUserQuestion (for clarifications) or ExitPlanMode (for approval).".to_string(),
    "Never ask about plan approval via text or AskUserQuestion.".to_string(),
  ]);
  parts.join(" ")
}

fn reentry_reminder(plan_file_path: Option<&str>) -> String {
  let mut lines: Vec<String> = Vec::new();
  lines.push(
    "Plan mode is active. You MUST NOT make any edits (with the exception of the plan file below), \
     run non-readonly tools, or otherwise make changes to the system. \
     This supersedes any other instructions you have received."
      .to_string(),
  );
  lines.extend([String::new(), "## Re-entering Plan Mode".to_string()]);
  if let Some(path) = plan_file_path {
    lines.push(format!(
      "A plan file exists at {path} from a previous planning session."
    ));
  } else {
    lines.push("A plan file from a previous planning session already exists.".to_string());
  }
  lines.extend([
    "Before proceeding:".to_string(),
    "1. Read the existing plan file to understand what was previously planned".to_string(),
    "2. Evaluate the user's current request against that plan".to_string(),
    "3. If different task: replace the old plan with a fresh one. If same task: update the existing plan.".to_string(),
    "4. You may use WriteFile or StrReplaceFile to modify the plan file. If the file does not exist yet, create it with WriteFile first.".to_string(),
    "5. Use AskUserQuestion to clarify missing requirements or user preferences that affect the plan.".to_string(),
    "6. Always edit the plan file before calling ExitPlanMode.".to_string(),
    String::new(),
    "Your turn must end with either AskUserQuestion (to clarify requirements) or ExitPlanMode (to request plan approval).".to_string(),
  ]);
  lines.join("\n")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_no_injection_when_plan_mode_inactive() {
    let mut state = PlanModeInjectionState::default();
    assert!(state.collect(false, None, &[]).is_none());
  }

  #[test]
  fn test_first_reminder_is_full() {
    let mut state = PlanModeInjectionState::default();
    let reminder = state.collect(true, None, &[]).expect("first reminder");
    assert!(reminder.content.starts_with("Plan mode is active."));
    assert!(reminder.content.contains("Workflow:"));
  }

  #[test]
  fn test_reminder_throttled_by_assistant_turns() {
    let mut state = PlanModeInjectionState::default();
    let first = state.collect(true, None, &[]).expect("first reminder");

    // History starts with the first reminder, then TURN_INTERVAL assistant turns
    // interleaved with ordinary user messages.
    let mut history = vec![Message::user(format!(
      "<system-reminder>\n{}\n</system-reminder>",
      first.content
    ))];
    for _ in 0..TURN_INTERVAL {
      history.push(Message::assistant("ok"));
      history.push(Message::user("next"));
    }

    let second = state
      .collect(true, None, &history)
      .expect("second reminder");
    assert!(second.content.starts_with("Plan mode still active"));
  }

  #[test]
  fn test_activation_schedules_reentry_reminder() {
    let mut state = PlanModeInjectionState::default();
    state.schedule_activation();

    let tmp = tempfile::tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    std::fs::write(&plan_path, "# plan").unwrap();

    let reminder = state
      .collect(true, Some(&plan_path), &[])
      .expect("activation reminder");
    assert!(reminder.content.contains("Re-entering Plan Mode"));
  }

  fn make_history_with_reminder(reminder: &PlanModeReminder) -> Vec<Message> {
    vec![Message::user(format!(
      "<system-reminder>\n{}\n</system-reminder>",
      reminder.content
    ))]
  }

  #[test]
  fn test_sparse_reminder_after_interval() {
    let mut state = PlanModeInjectionState::default();
    let first = state.collect(true, None, &[]).unwrap();
    let history = make_history_with_reminder(&first);

    let mut history_with_turns = history.clone();
    for _ in 0..TURN_INTERVAL {
      history_with_turns.push(Message::assistant("ok"));
      history_with_turns.push(Message::user("next"));
    }

    let sparse = state
      .collect(true, None, &history_with_turns)
      .expect("sparse reminder");
    assert!(sparse.content.starts_with("Plan mode still active"));
  }
}
