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

  /// Get the configuration file path (config.toml in the config directory)
  pub fn config_path(&self) -> PathBuf {
    self.config_dir().join("config.toml")
  }
}
