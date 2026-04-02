/// Unicode range for CJK (Chinese, Japanese, Korean) Unified Ideographs
/// These characters typically have a display width of 2 in terminals
const CJK_UNIFIED_IDEOGRAPH_START: char = '\u{4e00}';
const CJK_UNIFIED_IDEOGRAPH_END: char = '\u{9fff}';

/// Check if a character is a CJK (Chinese, Japanese, Korean) unified ideograph.
/// CJK characters typically have a display width of 2 in terminals.
///
/// # Examples
///
/// ```
/// use crate::utils::string::is_cjk_char;
///
/// assert!(is_cjk_char('中'));
/// assert!(is_cjk_char('日'));
/// assert!(!is_cjk_char('a'));
/// assert!(!is_cjk_char('1'));
/// ```
pub fn is_cjk_char(c: char) -> bool {
  c >= CJK_UNIFIED_IDEOGRAPH_START && c <= CJK_UNIFIED_IDEOGRAPH_END
}

/// Calculate the display width of a character.
/// CJK characters have a width of 2, others have a width of 1.
///
/// # Examples
///
/// ```
/// use crate::utils::string::char_display_width;
///
/// assert_eq!(char_display_width('中'), 2);
/// assert_eq!(char_display_width('a'), 1);
/// ```
pub fn char_display_width(c: char) -> usize {
  if is_cjk_char(c) { 2 } else { 1 }
}

/// Calculate the display width of a string.
/// Sums up the display width of all characters.
///
/// # Examples
///
/// ```
/// use crate::utils::string::string_display_width;
///
/// assert_eq!(string_display_width("abc"), 3);
/// assert_eq!(string_display_width("中文"), 4);
/// ```
pub fn string_display_width(s: &str) -> usize {
  s.chars().map(char_display_width).sum()
}

/// Calculate the display width of the first `n` characters in a string.
/// Returns the sum of display widths for the first `n` characters.
/// If `n` is greater than the string length, calculates for all characters.
///
/// # Examples
///
/// ```
/// use crate::utils::string::prefix_display_width;
///
/// assert_eq!(prefix_display_width("中文abc", 0), 0);
/// assert_eq!(prefix_display_width("中文abc", 2), 4); // "中文" = 2+2
/// assert_eq!(prefix_display_width("中文abc", 5), 7); // "中文abc" = 2+2+1+1+1
/// ```
#[allow(dead_code)]
pub fn prefix_display_width(s: &str, n: usize) -> usize {
  s.chars().take(n).map(char_display_width).sum()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_is_cjk_char() {
    // CJK Unified Ideographs (Chinese, Japanese Kanji)
    assert!(is_cjk_char('中'));
    assert!(is_cjk_char('文'));
    assert!(is_cjk_char('日'));
    assert!(is_cjk_char('本'));
    assert!(is_cjk_char('漢')); // Kanji
    assert!(is_cjk_char('字')); // Character
    assert!(is_cjk_char('国')); // Country

    // Non-CJK characters (ASCII)
    assert!(!is_cjk_char('a'));
    assert!(!is_cjk_char('Z'));
    assert!(!is_cjk_char('1'));
    assert!(!is_cjk_char('!'));
    assert!(!is_cjk_char(' '));
    assert!(!is_cjk_char('\n'));

    // Non-CJK characters (Other Unicode scripts)
    assert!(!is_cjk_char('あ')); // Hiragana (Japanese)
    assert!(!is_cjk_char('한')); // Korean Hangul
    assert!(!is_cjk_char('🎉')); // Emoji

    // Boundary checks
    assert!(is_cjk_char('\u{4e00}')); // First CJK Unified Ideograph
    assert!(is_cjk_char('\u{9fff}')); // Last CJK Unified Ideograph
    assert!(!is_cjk_char('\u{3fff}')); // Just before CJK range
    assert!(!is_cjk_char('\u{a000}')); // Just after CJK range
  }

  #[test]
  fn test_char_display_width() {
    assert_eq!(char_display_width('a'), 1);
    assert_eq!(char_display_width('A'), 1);
    assert_eq!(char_display_width('1'), 1);
    assert_eq!(char_display_width(' '), 1);
    assert_eq!(char_display_width('!'), 1);

    assert_eq!(char_display_width('中'), 2);
    assert_eq!(char_display_width('文'), 2);
    assert_eq!(char_display_width('日'), 2);
    assert_eq!(char_display_width('本'), 2);
    assert_eq!(char_display_width('漢'), 2);
  }

  #[test]
  fn test_string_display_width() {
    assert_eq!(string_display_width(""), 0);
    assert_eq!(string_display_width("abc"), 3);
    assert_eq!(string_display_width("ABC"), 3);
    assert_eq!(string_display_width("123"), 3);
    assert_eq!(string_display_width("Hello World"), 11);

    assert_eq!(string_display_width("中文"), 4);
    assert_eq!(string_display_width("中"), 2);
    assert_eq!(string_display_width("中文abc"), 7); // 2+2+1+1+1 = 7
    assert_eq!(string_display_width("a中文b"), 6); // 1+2+2+1 = 6

    // Mixed content
    assert_eq!(string_display_width("Hello中文"), 9); // 5+2+2 = 9
    assert_eq!(string_display_width("日本語ABC"), 9); // 2+2+2+1+1+1 = 9
  }

  #[test]
  fn test_prefix_display_width() {
    // Empty cases
    assert_eq!(prefix_display_width("", 0), 0);
    assert_eq!(prefix_display_width("abc", 0), 0);

    // ASCII only
    assert_eq!(prefix_display_width("abc", 1), 1); // "a"
    assert_eq!(prefix_display_width("abc", 2), 2); // "ab"
    assert_eq!(prefix_display_width("abc", 3), 3); // "abc"
    assert_eq!(prefix_display_width("abc", 10), 3); // exceeds length, return full width

    // CJK only
    assert_eq!(prefix_display_width("中文", 0), 0);
    assert_eq!(prefix_display_width("中文", 1), 2); // "中" = 2
    assert_eq!(prefix_display_width("中文", 2), 4); // "中文" = 2+2

    // Mixed content
    assert_eq!(prefix_display_width("中文abc", 2), 4); // "中文" = 2+2
    assert_eq!(prefix_display_width("中文abc", 3), 5); // "中文a" = 2+2+1
    assert_eq!(prefix_display_width("中文abc", 5), 7); // "中文abc" = 2+2+1+1+1

    // Cursor position at various points
    assert_eq!(prefix_display_width("a中文b", 0), 0); // cursor at start
    assert_eq!(prefix_display_width("a中文b", 1), 1); // after "a"
    assert_eq!(prefix_display_width("a中文b", 2), 3); // after "a中" = 1+2
    assert_eq!(prefix_display_width("a中文b", 3), 5); // after "a中文" = 1+2+2
    assert_eq!(prefix_display_width("a中文b", 4), 6); // after "a中文b" = 1+2+2+1
  }
}
