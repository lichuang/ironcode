//! Configuration management for IronCode.
//!
//! Configuration is loaded from TOML files at:
//! - ~/.config/ironcode/config.toml (primary)
//! - ./ironcode.toml (project-local, overrides primary)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub mod loader;

pub use loader::load_config;

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
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
}

impl Default for Config {
  fn default() -> Self {
    Self {
      default_model: String::new(),
      providers: HashMap::new(),
      models: HashMap::new(),
      logging: LoggingConfig::default(),
    }
  }
}

fn default_true() -> bool {
  true
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
      std::env::var(var_name).unwrap_or_default()
    } else {
      key.to_string()
    }
  }
}

/// Provider configuration (connection settings)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
  /// Provider type: currently only "openai-compatible" is supported
  #[serde(rename = "type")]
  pub provider_type: ProviderType,

  /// Base URL for the API
  pub base_url: String,

  /// API key (can be "${ENV_VAR}" for environment variable substitution)
  #[serde(skip_serializing_if = "Option::is_none")]
  pub api_key: Option<String>,

  /// API version (for Azure)
  #[serde(skip_serializing_if = "Option::is_none")]
  pub api_version: Option<String>,
}

/// Provider types
/// 
/// Currently only OpenAI-compatible APIs are supported.
/// This includes OpenAI official API, Azure OpenAI, Ollama, and other compatible providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderType {
  /// OpenAI-compatible API (covers OpenAI, Azure, Ollama, and other compatible providers)
  #[serde(rename = "openai-compatible")]
  OpenaiCompatible,
}

impl ProviderType {
  /// Check if this provider requires an API key
  /// 
  /// OpenAI-compatible providers typically require an API key,
  /// except for local instances like Ollama which can be configured without one.
  pub fn requires_api_key(&self) -> bool {
    // OpenAI-compatible providers may or may not require an API key
    // depending on the actual provider (e.g., Ollama locally doesn't need one)
    true
  }
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

  /// Optional log file path
  #[serde(skip_serializing_if = "Option::is_none")]
  pub log_file: Option<PathBuf>,
}

fn default_log_level() -> String {
  "info".to_string()
}

impl Default for LoggingConfig {
  fn default() -> Self {
    Self {
      level: default_log_level(),
      log_file: None,
    }
  }
}
