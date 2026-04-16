//! Retry utility with exponential backoff.
//!
//! Provides a generic retry mechanism for async operations that may fail
//! transiently (network errors, rate limits, server errors).

use log::{info, warn};
use tokio::time::sleep;

use crate::config::RetryConfig;

/// Run an async operation with exponential backoff retry.
///
/// The `operation` closure is called repeatedly until it succeeds or the
/// maximum number of attempts is exhausted. Only errors classified as
/// retryable (via the `is_retryable` predicate) trigger retries; non-retryable
/// errors are returned immediately.
///
/// # Arguments
/// * `config` - Retry configuration (max attempts, delays)
/// * `operation` - The async operation to attempt
/// * `is_retryable` - Predicate that returns true for errors that should be retried
/// * `label` - Human-readable label for logging
///
/// # Returns
/// * `Ok(T)` on success
/// * `Err(E)` after exhausting all retries or encountering a non-retryable error
#[allow(dead_code)]
pub async fn retry_with_backoff<T, E, F, Fut, P>(
  config: &RetryConfig,
  operation: F,
  is_retryable: P,
  label: &str,
) -> Result<T, E>
where
  F: Fn() -> Fut,
  Fut: std::future::Future<Output = Result<T, E>>,
  P: Fn(&E) -> bool,
  E: std::fmt::Display,
{
  let max_attempts = config.max_attempts.max(1);
  let mut last_error: Option<E> = None;

  for attempt in 0..max_attempts {
    match operation().await {
      Ok(value) => {
        if attempt > 0 {
          info!(
            "{}: succeeded on attempt {}/{}",
            label,
            attempt + 1,
            max_attempts
          );
        }
        return Ok(value);
      }
      Err(err) => {
        let is_last = attempt + 1 >= max_attempts;

        if !is_retryable(&err) {
          warn!(
            "{}: non-retryable error on attempt {}/{}: {}",
            label,
            attempt + 1,
            max_attempts,
            err
          );
          return Err(err);
        }

        if is_last {
          warn!(
            "{}: all {} attempts exhausted, last error: {}",
            label, max_attempts, err
          );
          return Err(err);
        }

        let delay = config.delay_for_attempt(attempt);
        warn!(
          "{}: attempt {}/{} failed ({}), retrying in {:?}: {}",
          label,
          attempt + 1,
          max_attempts,
          err,
          delay,
          err
        );
        last_error = Some(err);
        sleep(delay).await;
      }
    }
  }

  // Should be unreachable, but just in case
  Err(last_error.unwrap())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;
  use std::sync::atomic::{AtomicU32, Ordering};

  #[tokio::test]
  async fn test_retry_succeeds_first_try() {
    let config = RetryConfig {
      max_attempts: 3,
      initial_delay_ms: 10,
      max_delay_ms: 100,
    };
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let result = retry_with_backoff(
      &config,
      move || {
        let c = counter_clone.clone();
        async move {
          c.fetch_add(1, Ordering::SeqCst);
          Ok::<_, String>(42)
        }
      },
      |_| true,
      "test",
    )
    .await;

    assert_eq!(result.unwrap(), 42);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn test_retry_succeeds_after_failures() {
    let config = RetryConfig {
      max_attempts: 3,
      initial_delay_ms: 10,
      max_delay_ms: 100,
    };
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let result = retry_with_backoff(
      &config,
      move || {
        let c = counter_clone.clone();
        async move {
          let n = c.fetch_add(1, Ordering::SeqCst);
          if n < 2 {
            Err("transient error".to_string())
          } else {
            Ok::<_, String>(99)
          }
        }
      },
      |_| true,
      "test",
    )
    .await;

    assert_eq!(result.unwrap(), 99);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
  }

  #[tokio::test]
  async fn test_retry_exhausted() {
    let config = RetryConfig {
      max_attempts: 2,
      initial_delay_ms: 10,
      max_delay_ms: 100,
    };
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let result = retry_with_backoff(
      &config,
      move || {
        let c = counter_clone.clone();
        async move {
          c.fetch_add(1, Ordering::SeqCst);
          Err::<i32, _>("always fails".to_string())
        }
      },
      |_| true,
      "test",
    )
    .await;

    assert!(result.is_err());
    assert_eq!(counter.load(Ordering::SeqCst), 2);
  }

  #[tokio::test]
  async fn test_retry_non_retryable_error() {
    let config = RetryConfig {
      max_attempts: 3,
      initial_delay_ms: 10,
      max_delay_ms: 100,
    };
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let result = retry_with_backoff(
      &config,
      move || {
        let c = counter_clone.clone();
        async move {
          c.fetch_add(1, Ordering::SeqCst);
          Err::<i32, _>("fatal".to_string())
        }
      },
      |err| !err.contains("fatal"),
      "test",
    )
    .await;

    assert!(result.is_err());
    // Should only attempt once (non-retryable)
    assert_eq!(counter.load(Ordering::SeqCst), 1);
  }

  #[test]
  fn test_delay_for_attempt() {
    let config = RetryConfig {
      max_attempts: 5,
      initial_delay_ms: 1000,
      max_delay_ms: 30_000,
    };

    assert_eq!(config.delay_for_attempt(0).as_millis(), 1000);
    assert_eq!(config.delay_for_attempt(1).as_millis(), 2000);
    assert_eq!(config.delay_for_attempt(2).as_millis(), 4000);
    assert_eq!(config.delay_for_attempt(3).as_millis(), 8000);
    // Capped at max_delay_ms
    assert_eq!(config.delay_for_attempt(10).as_millis(), 30_000);
  }

  #[test]
  fn test_is_enabled() {
    let enabled = RetryConfig {
      max_attempts: 3,
      initial_delay_ms: 1000,
      max_delay_ms: 30_000,
    };
    let disabled = RetryConfig {
      max_attempts: 0,
      initial_delay_ms: 1000,
      max_delay_ms: 30_000,
    };
    assert!(enabled.is_enabled());
    assert!(!disabled.is_enabled());
  }
}
