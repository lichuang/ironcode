//! Compaction strategy implementations for context compression.

use crate::llm::types::{Message, Role};

/// Result of a compaction operation.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CompactionResult {
  /// The compacted message list (summary + preserved messages)
  pub messages: Vec<Message>,
  /// Whether compaction was actually performed
  pub did_compact: bool,
  /// Number of messages that were compacted into summary
  pub compacted_count: usize,
  /// Number of messages that were preserved
  pub preserved_count: usize,
}

/// Compaction strategy trait.
///
/// Implementations define how to compress conversation history.
#[allow(dead_code)]
pub trait CompactionStrategy: Send + Sync {
  /// Check if compaction should be applied to the given messages.
  ///
  /// Returns true if the strategy determines that compaction is needed
  /// based on message count, content, or other criteria.
  fn should_compact(&self, messages: &[Message]) -> bool;

  /// Perform compaction on the given messages.
  ///
  /// Returns a `CompactionResult` containing the new message list.
  /// If compaction is not needed, returns the original messages with
  /// `did_compact: false`.
  fn compact(&self, messages: &[Message]) -> CompactionResult;
}

/// Rolling window compaction strategy.
///
/// Keeps the most recent N user/assistant message pairs intact,
/// and compacts all older messages into a single summary message.
///
/// # Example
/// With `max_preserved_messages = 2`:
/// - Input: [sys, user1, asst1, user2, asst2, user3, asst3]
/// - Output: [sys, compacted_summary, user3, asst3]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RollingWindowStrategy {
  /// Maximum number of recent user/assistant messages to preserve
  max_preserved_messages: usize,
}

impl RollingWindowStrategy {
  /// Create a new rolling window strategy.
  ///
  /// # Arguments
  /// * `max_preserved_messages` - Number of recent messages to preserve
  ///
  /// # Panics
  /// Panics if `max_preserved_messages` is 0.
  pub fn new(max_preserved_messages: usize) -> Self {
    assert!(
      max_preserved_messages > 0,
      "max_preserved_messages must be greater than 0"
    );
    Self {
      max_preserved_messages,
    }
  }

  /// Get the maximum number of preserved messages.
  #[allow(dead_code)]
  pub fn max_preserved_messages(&self) -> usize {
    self.max_preserved_messages
  }
}

impl Default for RollingWindowStrategy {
  fn default() -> Self {
    // Default to preserving 2 messages (1 user/assistant pair)
    Self::new(2)
  }
}

impl CompactionStrategy for RollingWindowStrategy {
  fn should_compact(&self, messages: &[Message]) -> bool {
    if messages.len() <= self.max_preserved_messages {
      return false;
    }

    // Count user/assistant messages only (exclude system, tool)
    let user_asst_count = messages
      .iter()
      .filter(|m| matches!(m.role, Role::User | Role::Assistant))
      .count();

    user_asst_count > self.max_preserved_messages
  }

  fn compact(&self, messages: &[Message]) -> CompactionResult {
    if !self.should_compact(messages) {
      return CompactionResult {
        messages: messages.to_vec(),
        did_compact: false,
        compacted_count: 0,
        preserved_count: messages.len(),
      };
    }

    // Find system message (preserve at start if exists)
    let system_msg = messages.iter().find(|m| m.role == Role::System).cloned();

    // Find the split point for preserved messages (counting from end)
    let mut preserved_start_idx = messages.len();
    let mut preserved_count = 0;

    for (idx, msg) in messages.iter().enumerate().rev() {
      if matches!(msg.role, Role::User | Role::Assistant) {
        preserved_count += 1;
        if preserved_count == self.max_preserved_messages {
          preserved_start_idx = idx;
          break;
        }
      }
    }

    // Messages to compact
    let to_compact = &messages[..preserved_start_idx];
    let preserved = &messages[preserved_start_idx..];

    // Build compacted message (placeholder for now - will use LLM summary later)
    let compacted_content = build_compacted_content(to_compact);
    let compacted_msg = Message::user(format!(
      "<compacted_context>\n{}\n</compacted_context>",
      compacted_content
    ));

    // Build final message list
    let mut result = Vec::new();
    if let Some(sys) = system_msg {
      result.push(sys);
    }
    result.push(compacted_msg);
    result.extend_from_slice(preserved);

    CompactionResult {
      messages: result,
      did_compact: true,
      compacted_count: to_compact.len(),
      preserved_count: preserved.len(),
    }
  }
}

/// Build compacted content from messages (placeholder implementation).
///
/// In the future, this will call an LLM to generate a summary.
#[allow(dead_code)]
fn build_compacted_content(messages: &[Message]) -> String {
  let mut content = String::from("Previous conversation context:\n\n");

  for (i, msg) in messages.iter().enumerate() {
    if msg.role == Role::System {
      continue; // Skip system in compacted content
    }

    content.push_str(&format!(
      "[{}] {}: {}\n",
      i + 1,
      format_role(&msg.role),
      truncate(&msg.content, 200)
    ));
  }

  content.push_str(
    "\n(Note: This is a placeholder compaction. LLM-generated summary will be implemented in a future update.)"
  );

  content
}

#[allow(dead_code)]
fn format_role(role: &Role) -> &'static str {
  match role {
    Role::System => "System",
    Role::User => "User",
    Role::Assistant => "Assistant",
    Role::Tool => "Tool",
  }
}

#[allow(dead_code)]
fn truncate(s: &str, max_len: usize) -> String {
  if s.len() <= max_len {
    s.to_string()
  } else {
    format!("{}...", &s[..max_len])
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn create_test_messages(count: usize) -> Vec<Message> {
    (0..count)
      .map(|i| {
        if i % 2 == 0 {
          Message::user(format!("User message {}", i / 2))
        } else {
          Message::assistant(format!("Assistant response {}", i / 2))
        }
      })
      .collect()
  }

  #[test]
  fn test_rolling_window_should_compact() {
    let strategy = RollingWindowStrategy::new(2);

    // 4 messages (2 user/assistant pairs) > 2 preserved
    let messages = create_test_messages(4);
    assert!(strategy.should_compact(&messages));

    // 2 messages <= 2 preserved
    let messages = create_test_messages(2);
    assert!(!strategy.should_compact(&messages));
  }

  #[test]
  fn test_rolling_window_compact() {
    let strategy = RollingWindowStrategy::new(2);
    let messages = create_test_messages(6);

    let result = strategy.compact(&messages);

    assert!(result.did_compact);
    assert_eq!(result.compacted_count, 4); // First 4 messages compacted
    assert_eq!(result.preserved_count, 2); // Last 2 messages preserved
    assert_eq!(result.messages.len(), 3); // compacted_msg + 2 preserved
  }

  #[test]
  fn test_rolling_window_no_compact_needed() {
    let strategy = RollingWindowStrategy::new(2);
    let messages = create_test_messages(2);

    let result = strategy.compact(&messages);

    assert!(!result.did_compact);
    assert_eq!(result.messages.len(), 2);
  }

  #[test]
  fn test_rolling_window_preserves_system() {
    let strategy = RollingWindowStrategy::new(2);
    let mut messages = vec![Message::system("You are helpful")];
    messages.extend(create_test_messages(4));

    let result = strategy.compact(&messages);

    assert!(result.did_compact);
    // System should be first
    assert_eq!(result.messages[0].role, Role::System);
    assert_eq!(result.messages[0].content, "You are helpful");
  }
}
