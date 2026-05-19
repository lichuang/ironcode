//! Background task ID generation.

use rand::Rng;

const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Generate a task ID with the given kind prefix.
pub fn generate_task_id(kind: &str) -> String {
  let suffix: String = (0..8)
    .map(|_| {
      let idx = rand::thread_rng().gen_range(0..ALPHABET.len());
      ALPHABET[idx] as char
    })
    .collect();
  format!("{}-{}", kind, suffix)
}
