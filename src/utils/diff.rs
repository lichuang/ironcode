//! Diff generation utilities for file modification previews.

use similar::TextDiff;

/// Compute a unified diff preview for a file modification.
///
/// Returns `None` when old and new content are identical.
pub fn compute_file_diff(path: &str, old: &str, new: &str) -> Option<String> {
  if old == new {
    return None;
  }

  let diff = TextDiff::from_lines(old, new);
  let output = format!(
    "{}",
    diff
      .unified_diff()
      .header(&format!("a/{path}"), &format!("b/{path}"))
  );

  if output.trim().is_empty() {
    return None;
  }

  Some(output)
}

/// Build a preview string for a write-file operation.
pub fn preview_write_file(path: &str, old: Option<&str>, new: &str) -> Option<String> {
  match old {
    Some(old_content) => compute_file_diff(path, old_content, new),
    None => {
      let preview = new.lines().take(20).collect::<Vec<_>>().join("\n");
      Some(format!("New file: {path}\n{preview}"))
    }
  }
}

/// Build a preview string for a replace-file operation.
pub fn preview_replace_file(path: &str, old: &str, new: &str) -> Option<String> {
  compute_file_diff(path, old, new)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_compute_file_diff_no_change() {
    assert!(compute_file_diff("test.rs", "fn main() {}", "fn main() {}").is_none());
  }

  #[test]
  fn test_compute_file_diff_add_lines() {
    let old = "line1\nline2\n";
    let new = "line1\nline2\nline3\n";
    let result = compute_file_diff("test.txt", old, new).unwrap();
    assert!(result.contains("--- a/test.txt"));
    assert!(result.contains("+++ b/test.txt"));
    assert!(result.contains("+line3"));
  }

  #[test]
  fn test_compute_file_diff_delete_lines() {
    let old = "line1\nline2\nline3\n";
    let new = "line1\nline3\n";
    let result = compute_file_diff("test.txt", old, new).unwrap();
    assert!(result.contains("-line2"));
  }

  #[test]
  fn test_compute_file_diff_modify_lines() {
    let old = "fn old() {}\n";
    let new = "fn new() {}\n";
    let result = compute_file_diff("test.rs", old, new).unwrap();
    assert!(result.contains("-fn old() {}"));
    assert!(result.contains("+fn new() {}"));
  }

  #[test]
  fn test_compute_file_diff_hunk_header() {
    let old = "fn main() {\n    let x = 1;\n}\n";
    let new = "fn main() {\n    let x = 2;\n}\n";
    let result = compute_file_diff("test.rs", old, new).unwrap();
    assert!(result.contains("@@"));
  }

  #[test]
  fn test_preview_write_file_new() {
    let result = preview_write_file("src/main.rs", None, "fn main() {}").unwrap();
    assert!(result.starts_with("New file: src/main.rs"));
    assert!(result.contains("fn main() {}"));
  }

  #[test]
  fn test_preview_write_file_overwrite() {
    let old = "fn old() {}\n";
    let new = "fn new() {}\n";
    let result = preview_write_file("src/main.rs", Some(old), new).unwrap();
    assert!(result.contains("--- a/src/main.rs"));
    assert!(result.contains("+++ b/src/main.rs"));
    assert!(result.contains("-fn old() {}"));
    assert!(result.contains("+fn new() {}"));
  }

  #[test]
  fn test_preview_replace_file() {
    let old = "foo bar baz\n";
    let new = "foo qux baz\n";
    let result = preview_replace_file("test.txt", old, new).unwrap();
    assert!(result.contains("-foo bar baz"));
    assert!(result.contains("+foo qux baz"));
  }
}
