//! Configuration management for IronCode.
//!
//! Configuration is loaded from TOML file at:
//! - ~/.ironcode/config.toml (default location)

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Default value for history max_size (1MB).
const DEFAULT_HISTORY_MAX_SIZE: usize = 1024 * 1024;
/// Default value for history max_entries.
const DEFAULT_HISTORY_MAX_ENTRIES: usize = 1000;
/// Default value for compaction trigger ratio (85%).
const DEFAULT_COMPACTION_TRIGGER_RATIO: f32 = 0.85;
/// Default value for reserved context size (50K tokens).
const DEFAULT_RESERVED_CONTEXT_SIZE: usize = 50_000;

pub mod loader;

// pub use loader::{data_dir, load_config_from_dir};  // Currently unused

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
  /// Data directory path for ironcode files (logs, prompts, etc.)
  /// Defaults to ~/.ironcode/ if not specified
  #[serde(default)]
  pub dir: Option<PathBuf>,

  /// Default model to use (format: "provider/model-name")
  /// Required field, cannot be empty
  pub default_model: String,

  /// Provider configurations
  #[serde(default)]
  pub providers: HashMap<String, ProviderConfig>,

  /// Model configurations
  #[serde(default)]
  pub models: HashMap<String, ModelConfig>,

  /// Logging settings
  #[serde(default)]
  pub logging: LoggingConfig,

  /// Default thinking mode (whether to use think by default)
  #[serde(default = "default_true")]
  pub default_thinking: bool,

  /// Input history configuration
  #[serde(default)]
  pub history: HistoryConfig,

  /// Compaction configuration for context management
  #[serde(default)]
  pub compaction: CompactionConfig,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      dir: None,
      default_model: String::new(),
      providers: HashMap::new(),
      models: HashMap::new(),
      logging: LoggingConfig::default(),
      default_thinking: true,
      history: HistoryConfig::default(),
      compaction: CompactionConfig::default(),
    }
  }
}

fn default_true() -> bool {
  true
}

fn default_history_max_size() -> usize {
  DEFAULT_HISTORY_MAX_SIZE
}

fn default_history_max_entries() -> usize {
  DEFAULT_HISTORY_MAX_ENTRIES
}

impl Config {
  /// Get a provider by name
  pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
    self.providers.get(name)
  }

  /// Get a model by name
  pub fn get_model(&self, name: &str) -> Option<&ModelConfig> {
    self.models.get(name)
  }

  /// Get the default model configuration
  pub fn default_model_config(&self) -> Option<&ModelConfig> {
    self.get_model(&self.default_model)
  }

  /// Resolve API key (handles env var substitution like "${OPENAI_API_KEY}")
  pub fn resolve_api_key(&self, key: &str) -> String {
    if key.starts_with("${") && key.ends_with("}") {
      let var_name = &key[2..key.len() - 1];
      env::var(var_name).unwrap_or_default()
    } else {
      key.to_string()
    }
  }
}

/// Provider configuration (connection settings)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
  /// Provider type: "kimi", "openai", "ollama", etc.
  #[serde(rename = "type")]
  pub provider_type: String,

  /// Base URL for the API
  pub base_url: String,

  /// API key (can be "${ENV_VAR}" for environment variable substitution)
  #[serde(skip_serializing_if = "Option::is_none")]
  pub api_key: Option<String>,

  /// API version (for Azure)
  #[serde(skip_serializing_if = "Option::is_none")]
  pub api_version: Option<String>,
}

/// Model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
  /// Reference to provider name
  pub provider: String,

  /// Model identifier (as expected by the provider API)
  pub model: String,

  /// Maximum context size in tokens
  #[serde(skip_serializing_if = "Option::is_none")]
  pub max_context_size: Option<usize>,

  /// Default temperature (0.0 - 2.0)
  #[serde(skip_serializing_if = "Option::is_none")]
  pub temperature: Option<f32>,

  /// Maximum tokens to generate
  #[serde(skip_serializing_if = "Option::is_none")]
  pub max_tokens: Option<u32>,

  /// Whether streaming is supported
  #[serde(default = "default_true")]
  pub supports_streaming: bool,

  /// Whether vision/multimodal is supported
  #[serde(default)]
  pub supports_vision: bool,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
  /// Log level: "trace", "debug", "info", "warn", "error"
  #[serde(default = "default_log_level")]
  pub level: String,
}

fn default_log_level() -> String {
  "info".to_string()
}

impl Default for LoggingConfig {
  fn default() -> Self {
    Self {
      level: default_log_level(),
    }
  }
}

/// Input history configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
  /// Maximum file size in bytes (0 = unlimited).
  /// When exceeded, older entries are removed.
  #[serde(default = "default_history_max_size")]
  pub max_size: usize,

  /// Maximum number of entries (0 = unlimited).
  /// When exceeded, older entries are removed.
  #[serde(default = "default_history_max_entries")]
  pub max_entries: usize,
}

impl Default for HistoryConfig {
  fn default() -> Self {
    Self {
      max_size: DEFAULT_HISTORY_MAX_SIZE,
      max_entries: DEFAULT_HISTORY_MAX_ENTRIES,
    }
  }
}

/// Compaction configuration for automatic context compression.
///
/// Controls when and how the conversation context should be compacted
/// to prevent exceeding the model's context window limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
  /// Reserved token count for LLM response generation.
  /// Auto-compaction triggers when context_tokens + reserved_context_size >= max_context_size.
  /// Default is 50000.
  #[serde(default = "default_reserved_context_size")]
  pub reserved_context_size: usize,

  /// Context usage ratio threshold for auto-compaction (0.5 - 0.99).
  /// Auto-compaction triggers when context_tokens >= max_context_size * trigger_ratio
  /// or when context_tokens + reserved_context_size >= max_context_size.
  /// Default is 0.85 (85%).
  #[serde(default = "default_compaction_trigger_ratio")]
  pub trigger_ratio: f32,

  /// Whether automatic compaction is enabled.
  /// Default is true.
  #[serde(default = "default_true")]
  pub enabled: bool,
}

fn default_reserved_context_size() -> usize {
  DEFAULT_RESERVED_CONTEXT_SIZE
}

fn default_compaction_trigger_ratio() -> f32 {
  DEFAULT_COMPACTION_TRIGGER_RATIO
}

impl Default for CompactionConfig {
  fn default() -> Self {
    Self {
      reserved_context_size: DEFAULT_RESERVED_CONTEXT_SIZE,
      trigger_ratio: DEFAULT_COMPACTION_TRIGGER_RATIO,
      enabled: true,
    }
  }
}
