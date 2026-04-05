//! Token counting estimation for LLM context management.
//!
//! Provides approximate token counting without requiring heavy dependencies
//! like tiktoken-rs. Uses a simple estimation algorithm based on character
//! and byte patterns.

use crate::view::chat::ChatMessage;

/// Estimate token count for a given text.
///
/// This is a rough estimation algorithm:
/// - ASCII characters: ~4 chars per token (includes space/punctuation)
/// - CJK characters: ~1 char per token (Chinese, Japanese, Korean)
/// - Mixed content: weighted average based on character types
///
/// # Arguments
/// * `text` - The text to estimate tokens for
///
/// # Returns
/// Estimated token count (always at least 1 for non-empty text)
///
/// # Examples
/// ```
/// use ironcode::utils::token_counter::estimate_tokens;
///
/// // English: ~4 chars per token
/// assert_eq!(estimate_tokens("hello world"), 3); // 11 chars / 4 ≈ 3
///
/// // Chinese: ~1 char per token
/// assert_eq!(estimate_tokens("你好世界"), 4); // 4 CJK chars
///
/// // Empty string
/// assert_eq!(estimate_tokens(""), 0);
/// ```
pub fn estimate_tokens(text: &str) -> usize {
  if text.is_empty() {
    return 0;
  }

  let mut ascii_count = 0usize;
  let mut cjk_count = 0usize;
  let mut other_count = 0usize;

  for c in text.chars() {
    if c.is_ascii() {
      ascii_count += 1;
    } else if is_cjk(c) {
      cjk_count += 1;
    } else {
      // Other Unicode characters (e.g., emoji, accented chars)
      other_count += 1;
    }
  }

  // Estimation:
  // - ASCII: 4 chars per token
  // - CJK: 1 char per token
  // - Other: 2 chars per token (rough estimate)
  let ascii_tokens = ascii_count.div_ceil(4);
  let cjk_tokens = cjk_count;
  let other_tokens = other_count.div_ceil(2);

  let total = ascii_tokens + cjk_tokens + other_tokens;
  total.max(1) // At least 1 token for non-empty text
}

/// Check if a character is CJK (Chinese, Japanese, Korean)
fn is_cjk(c: char) -> bool {
  // CJK Unified Ideographs
  ('\u{4e00}'..='\u{9fff}').contains(&c)
    // CJK Unified Ideographs Extension A
    || ('\u{3400}'..='\u{4dbf}').contains(&c)
    // CJK Unified Ideographs Extension B
    || ('\u{20000}'..='\u{2a6df}').contains(&c)
    // Hiragana
    || ('\u{3040}'..='\u{309f}').contains(&c)
    // Katakana
    || ('\u{30a0}'..='\u{30ff}').contains(&c)
    // Hangul Syllables
    || ('\u{ac00}'..='\u{d7af}').contains(&c)
    // Hangul Jamo
    || ('\u{1100}'..='\u{11ff}').contains(&c)
    // Full-width ASCII variants
    || ('\u{ff01}'..='\u{ff60}').contains(&c)
    // Half-width Katakana
    || ('\u{ff65}'..='\u{ff9f}').contains(&c)
}

/// Calculate total tokens for a collection of messages.
///
/// Adds a fixed overhead per message to account for formatting/role markers.
#[allow(dead_code)]
pub fn estimate_messages_tokens(messages: &[impl AsRef<str>]) -> usize {
  let mut total = 0usize;

  for msg in messages {
    // Each message has ~4 tokens overhead (role markers, formatting)
    total += 4;
    total += estimate_tokens(msg.as_ref());
  }

  total
}

use crate::llm::types::Message;

/// Estimate total tokens for LLM messages.
///
/// This is used by the session to check compaction thresholds.
pub fn estimate_llm_messages_tokens(messages: &[Message]) -> usize {
  let mut total = 0usize;

  for msg in messages {
    // Each message has ~4 tokens overhead (role markers, formatting)
    total += 4;
    total += estimate_tokens(&msg.content);

    // Count tool calls if present
    if let Some(tool_calls) = &msg.tool_calls {
      for tc in tool_calls {
        total += estimate_tokens(&tc.name);
        total += estimate_tokens(&tc.arguments);
      }
    }
  }

  total
}

/// Estimate total tokens for chat messages (used by status bar).
///
/// This is the canonical implementation used by both status_bar.rs and
/// chat.rs to ensure consistent token counting across the application.
pub fn estimate_chat_messages_tokens(messages: &[ChatMessage]) -> usize {
  let mut total = 0usize;

  for msg in messages {
    match msg {
      ChatMessage::User { content } => {
        total += estimate_tokens(content);
      }
      ChatMessage::Assistant {
        content,
        thinking_content,
      } => {
        // Count both content and thinking content
        total += estimate_tokens(content);
        if let Some(thinking) = thinking_content {
          total += estimate_tokens(thinking);
        }
      }
      ChatMessage::ToolCall { name, arguments } => {
        // Tool calls: count name + arguments
        total += estimate_tokens(name);
        total += estimate_tokens(arguments);
      }
    }
  }

  total
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_empty_string() {
    assert_eq!(estimate_tokens(""), 0);
  }

  #[test]
  fn test_ascii_short() {
    // "hi" - 2 ASCII chars, should be 1 token
    assert_eq!(estimate_tokens("hi"), 1);
  }

  #[test]
  fn test_ascii_long() {
    // "hello world" - 11 ASCII chars, ~3 tokens
    assert_eq!(estimate_tokens("hello world"), 3);
  }

  #[test]
  fn test_cjk() {
    // "你好世界" - 4 CJK chars, 4 tokens
    assert_eq!(estimate_tokens("你好世界"), 4);
  }

  #[test]
  fn test_mixed() {
    // "hello 你好" - 6 ASCII + 2 CJK = ~2 + 2 = 4 tokens
    assert_eq!(estimate_tokens("hello 你好"), 4);
  }

  #[test]
  fn test_code() {
    let code = "fn main() { println!(\"Hello\"); }";
    // ~34 ASCII chars / 4 = ~9 tokens
    let tokens = estimate_tokens(code);
    assert!(
      tokens >= 8 && tokens <= 10,
      "Expected ~9 tokens, got {}",
      tokens
    );
  }

  #[test]
  fn test_messages() {
    let msgs = vec!["Hello", "World"];
    // 2 messages * 4 overhead + 2 + 2 = 12 tokens
    // Each "Hello"/"World" is 5 chars -> 2 tokens, plus 4 overhead each
    assert_eq!(estimate_messages_tokens(&msgs), 12);
  }

  #[test]
  fn test_japanese() {
    // "こんにちは" (Konnichiwa) - 5 hiragana chars
    assert_eq!(estimate_tokens("こんにちは"), 5);
  }

  #[test]
  fn test_korean() {
    // "안녕하세요" (Annyeonghaseyo) - 5 hangul syllables
    assert_eq!(estimate_tokens("안녕하세요"), 5);
  }
}
