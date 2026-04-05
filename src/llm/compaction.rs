//! Context compaction for managing LLM token limits.
//!
//! Provides strategies for compressing conversation history when approaching
//! token limits.

pub use strategy::RollingWindowStrategy;

use crate::config::CompactionConfig;

mod strategy;

/// Check if auto-compaction should be triggered based on current token usage.
///
/// Returns true when either condition is met:
/// - Ratio-based: `token_count >= max_context_size * trigger_ratio`
/// - Reserved-based: `token_count + reserved_context_size >= max_context_size`
///
/// # Arguments
/// * `token_count` - Current estimated token count
/// * `max_context_size` - Maximum context size of the model
/// * `config` - Compaction configuration
///
/// # Examples
/// ```
/// use ironcode::config::CompactionConfig;
/// use ironcode::llm::compaction::should_auto_compact;
///
/// let config = CompactionConfig {
///     enabled: true,
///     trigger_ratio: 0.85,
///     reserved_context_size: 50000,
/// };
///
/// // 200K model, at 170K tokens (85% threshold)
/// assert!(should_auto_compact(170_000, 200_000, &config));
///
/// // Below threshold
/// assert!(!should_auto_compact(150_000, 200_000, &config));
/// ```
pub fn should_auto_compact(
  token_count: usize,
  max_context_size: usize,
  config: &CompactionConfig,
) -> bool {
  if !config.enabled {
    return false;
  }

  if max_context_size == 0 {
    return false;
  }

  // Ratio-based threshold
  let ratio_threshold = (max_context_size as f32 * config.trigger_ratio) as usize;
  let ratio_triggered = token_count >= ratio_threshold;

  // Reserved-based threshold
  let reserved_triggered =
    token_count.saturating_add(config.reserved_context_size) >= max_context_size;

  ratio_triggered || reserved_triggered
}

/// Calculate the token threshold at which compaction will trigger.
///
/// Returns the minimum of:
/// - Ratio threshold: `max_context_size * trigger_ratio`
/// - Reserved threshold: `max_context_size - reserved_context_size`
///
/// # Examples
/// ```
/// use ironcode::config::CompactionConfig;
/// use ironcode::llm::compaction::calculate_threshold;
///
/// let config = CompactionConfig {
///     enabled: true,
///     trigger_ratio: 0.85,
///     reserved_context_size: 50000,
/// };
///
/// // For 200K model: min(170K, 150K) = 150K
/// assert_eq!(calculate_threshold(200_000, &config), 150_000);
///
/// // For 1M model: min(850K, 950K) = 850K
/// assert_eq!(calculate_threshold(1_000_000, &config), 850_000);
/// ```
/// Calculate the token threshold at which compaction will trigger.
pub fn calculate_threshold(max_context_size: usize, config: &CompactionConfig) -> usize {
  if max_context_size == 0 {
    return 0;
  }

  let ratio_threshold = (max_context_size as f32 * config.trigger_ratio) as usize;
  let reserved_threshold = max_context_size.saturating_sub(config.reserved_context_size);

  ratio_threshold.min(reserved_threshold)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::CompactionConfig;

  #[test]
  fn test_should_auto_compact_ratio_threshold() {
    let config = CompactionConfig {
      enabled: true,
      trigger_ratio: 0.85,
      reserved_context_size: 50000,
    };

    // For 200K model:
    // - ratio threshold: 200K * 0.85 = 170K
    // - reserved threshold: 200K - 50K = 150K
    // reserved threshold triggers first at 150K

    // At 150K, reserved threshold triggers
    assert!(should_auto_compact(150_000, 200_000, &config));
    // Below both thresholds
    assert!(!should_auto_compact(149_999, 200_000, &config));

    // For 1M model, ratio threshold (850K) triggers before reserved (950K)
    assert!(should_auto_compact(850_000, 1_000_000, &config));
    assert!(!should_auto_compact(849_999, 1_000_000, &config));
  }

  #[test]
  fn test_should_auto_compact_reserved_threshold() {
    let config = CompactionConfig {
      enabled: true,
      trigger_ratio: 0.85,
      reserved_context_size: 50000,
    };

    // 200K - 50K = 150K reserved threshold, should trigger
    assert!(should_auto_compact(150_000, 200_000, &config));
    // Just below
    assert!(!should_auto_compact(149_999, 200_000, &config));
  }

  #[test]
  fn test_should_auto_compact_disabled() {
    let config = CompactionConfig {
      enabled: false,
      trigger_ratio: 0.85,
      reserved_context_size: 50000,
    };

    assert!(!should_auto_compact(200_000, 200_000, &config));
  }

  #[test]
  fn test_calculate_threshold_200k() {
    let config = CompactionConfig {
      enabled: true,
      trigger_ratio: 0.85,
      reserved_context_size: 50000,
    };

    // min(170K, 150K) = 150K
    assert_eq!(calculate_threshold(200_000, &config), 150_000);
  }

  #[test]
  fn test_calculate_threshold_1m() {
    let config = CompactionConfig {
      enabled: true,
      trigger_ratio: 0.85,
      reserved_context_size: 50000,
    };

    // min(850K, 950K) = 850K
    assert_eq!(calculate_threshold(1_000_000, &config), 850_000);
  }
}
