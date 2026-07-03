//! Time-related utilities.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A very long duration used as a fallback timeout.
/// Approximately one year in seconds.
pub const ONE_YEAR: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// Unix timestamp in seconds.
pub type Timestamp = u64;

/// Return the current time as a Unix timestamp in seconds.
pub fn now_secs() -> Timestamp {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
