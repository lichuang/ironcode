//! Slash command parser.

/// Metadata for a slash command shown in the completion menu.
pub struct SlashCommandInfo {
  /// Command name (without the leading `/`).
  pub name: &'static str,
  /// Short description for the completion menu.
  pub description: &'static str,
}

/// Available slash commands for autocomplete.
pub const SLASH_COMMANDS: &[SlashCommandInfo] = &[
  SlashCommandInfo {
    name: "task",
    description: "Browse and manage background tasks",
  },
  SlashCommandInfo {
    name: "clear",
    description: "Clear conversation context",
  },
  SlashCommandInfo {
    name: "reset",
    description: "Clear conversation context",
  },
];

/// Parsed slash command invocation.
pub struct SlashCommandCall {
  /// Command name (without the leading `/`).
  pub name: String,
  /// Arguments after the command name.
  pub args: String,
  /// Raw input including the leading `/`.
  #[allow(dead_code)]
  pub raw_input: String,
}

/// Parse a slash command from user input.
///
/// Rules:
/// - Input must start with `/`.
/// - Command name matches `[a-zA-Z0-9_-]+`.
/// - The character after the command name must be whitespace or end-of-string
///   (this prevents `/path/to/file` from being parsed as a slash command).
/// - Everything after the command name (stripped) becomes `args`.
pub fn parse_slash_command(input: &str) -> Option<SlashCommandCall> {
  let trimmed = input.trim();
  if !trimmed.starts_with('/') {
    return None;
  }
  let without_slash = &trimmed[1..];
  let (name, args) = without_slash
    .split_once(char::is_whitespace)
    .unwrap_or((without_slash, ""));
  if name.is_empty()
    || !name
      .chars()
      .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
  {
    return None;
  }
  Some(SlashCommandCall {
    name: name.to_lowercase(),
    args: args.trim().to_string(),
    raw_input: trimmed.to_string(),
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_slash_command_basic() {
    let cmd = parse_slash_command("/task").unwrap();
    assert_eq!(cmd.name, "task");
    assert_eq!(cmd.args, "");
    assert_eq!(cmd.raw_input, "/task");
  }

  #[test]
  fn test_parse_slash_command_with_args() {
    let cmd = parse_slash_command("/task foo bar").unwrap();
    assert_eq!(cmd.name, "task");
    assert_eq!(cmd.args, "foo bar");
  }

  #[test]
  fn test_parse_slash_command_not_a_command() {
    assert!(parse_slash_command("/path/to/file").is_none());
    assert!(parse_slash_command("hello /task").is_none());
    assert!(parse_slash_command("").is_none());
    assert!(parse_slash_command("  ").is_none());
  }

  #[test]
  fn test_parse_slash_command_case_insensitive() {
    let cmd = parse_slash_command("/TASK").unwrap();
    assert_eq!(cmd.name, "task");
  }
}
