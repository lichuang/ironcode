//! Command line argument parsing

use clap::Parser;
use std::path::PathBuf;

/// IronCode - AI-powered terminal code assistant
#[derive(Debug, Parser)]
#[command(name = "ironcode")]
#[command(about = "AI-powered terminal code assistant")]
#[command(version)]
pub struct Args {
  /// Path to configuration file
  ///
  /// If not specified, defaults to ~/.ironcode/config.toml
  #[arg(short = 'c', long, value_name = "FILE")]
  pub config: Option<PathBuf>,
}

impl Args {
  /// Get the configuration file path
  ///
  /// Returns the user-specified path or the default location
  pub fn config_path(&self) -> PathBuf {
    self.config.clone().unwrap_or_else(|| {
      dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".ironcode")
        .join("config.toml")
    })
  }
}
