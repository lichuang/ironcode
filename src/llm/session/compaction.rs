//! CompactionService — token monitoring and compaction execution.

use crate::config::CompactionConfig;
use crate::llm::compaction::{Compaction, calculate_threshold, should_auto_compact};
use crate::llm::types::Message;
use crate::utils::token_counter::estimate_llm_messages_tokens;

/// Trigger information when compaction should be performed.
pub struct CompactionTrigger {
  /// Current estimated token count
  pub current_tokens: usize,
  /// Token threshold that triggered this notification
  pub threshold: usize,
  /// Maximum context size for the current model
  pub max_context_size: usize,
}

/// Result of executing compaction on the message history.
pub struct CompactionResult {
  /// Number of messages before compaction
  pub message_count_before: usize,
  /// Number of messages after compaction
  pub message_count_after: usize,
  /// New estimated token count
  pub new_token_count: usize,
}

/// Token monitoring and compaction execution service.
pub struct CompactionService {
  config: CompactionConfig,
  compaction: Compaction,
  notified: bool,
}

impl CompactionService {
  /// Create a new compaction service with the given configuration.
  pub fn new(config: CompactionConfig) -> Self {
    Self {
      config,
      compaction: Compaction::default(),
      notified: false,
    }
  }

  /// Check if compaction should be triggered.
  ///
  /// Returns `Some(CompactionTrigger)` when compaction is needed and has not
  /// been notified yet. Returns `None` when below threshold or already notified.
  pub fn check(
    &mut self,
    current_tokens: usize,
    max_context_size: usize,
  ) -> Option<CompactionTrigger> {
    if !self.config.enabled || max_context_size == 0 {
      return None;
    }

    if should_auto_compact(current_tokens, max_context_size, &self.config) {
      if !self.notified {
        self.notified = true;
        let threshold = calculate_threshold(max_context_size, &self.config);
        Some(CompactionTrigger {
          current_tokens,
          threshold,
          max_context_size,
        })
      } else {
        None
      }
    } else {
      self.notified = false;
      None
    }
  }

  /// Execute compaction on the given messages.
  ///
  /// Replaces `messages` with the compacted result if compaction was performed.
  /// Returns `Some(CompactionResult)` if compaction occurred, `None` otherwise.
  pub fn execute(&mut self, messages: &mut Vec<Message>) -> Option<CompactionResult> {
    if !self.compaction.should_compact(messages) {
      return None;
    }

    let before = messages.len();
    let result = self.compaction.compact(messages);

    if !result.did_compact {
      return None;
    }

    *messages = result.messages;
    self.notified = false;

    Some(CompactionResult {
      message_count_before: before,
      message_count_after: messages.len(),
      new_token_count: estimate_llm_messages_tokens(messages),
    })
  }
}
