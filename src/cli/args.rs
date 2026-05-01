//! Command line argument parsing

use clap::Parser;
use std::path::PathBuf;

/// IronCode - AI-powered terminal code assistant
#[derive(Debug, Parser)]
#[command(name = "ironcode")]
#[command(about = "AI-powered terminal code assistant")]
#[command(version)]
pub struct Args {
  /// Path to configuration directory
  ///
  /// If not specified, defaults to ~/.ironcode/
  /// The directory should contain config.toml and optionally prompts/system.md
  ///
  /// Note: This specifies where to find the config file. The actual data directory
  /// (for logs, prompts, etc.) can be configured via the `dir` option in config.toml.
  #[arg(short = 'c', long, value_name = "DIR")]
  pub config: Option<PathBuf>,

  /// Start a new session instead of continuing the last one
  #[arg(long)]
  pub new_session: bool,

  /// Load a specific session by ID
  #[arg(long, value_name = "ID")]
  pub session: Option<String>,

  /// Continue the most recent session
  #[arg(long)]
  pub r#continue: bool,

  /// Enable YOLO mode: auto-approve all tool calls without confirmation
  #[arg(long)]
  pub yolo: bool,

  /// Path to an MCP server configuration JSON file
  ///
  /// The file should follow the standard MCP client format:
  /// { "mcpServers": { "name": { "command": "...", "args": [...] } } }
  #[arg(long, value_name = "PATH")]
  pub mcp_config_file: Option<PathBuf>,
}

impl Args {
  /// Get the configuration directory path
  ///
  /// Returns the user-specified directory or the default location (~/.ironcode/)
  /// This is where the config.toml file is loaded from.
  pub fn config_dir(&self) -> PathBuf {
    self.config.clone().unwrap_or_else(|| {
      dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".ironcode")
    })
  }
}
