//! Configuration management for IronCode.
//!
//! Configuration is loaded from TOML file at:
//! - ~/.ironcode/config.toml (default location)

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::time::Duration;

/// Default value for history max_size (1MB).
const DEFAULT_HISTORY_MAX_SIZE: usize = 1024 * 1024;
/// Default value for history max_entries.
const DEFAULT_HISTORY_MAX_ENTRIES: usize = 1000;
/// Default value for compaction trigger ratio (85%).
const DEFAULT_COMPACTION_TRIGGER_RATIO: f32 = 0.85;
/// Default value for reserved context size (50K tokens).
const DEFAULT_RESERVED_CONTEXT_SIZE: usize = 50_000;
/// Default maximum context size in tokens (128K).
pub const DEFAULT_MAX_CONTEXT_SIZE: usize = 128_000;
/// Default maximum retry attempts for LLM requests.
const DEFAULT_RETRY_MAX_ATTEMPTS: u32 = 3;
/// Default initial retry delay in milliseconds.
const DEFAULT_RETRY_INITIAL_DELAY_MS: u64 = 300;
/// Default maximum retry delay in milliseconds.
const DEFAULT_RETRY_MAX_DELAY_MS: u64 = 5_000;
/// Default jitter in milliseconds.
const DEFAULT_RETRY_JITTER_MS: u64 = 500;

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

  /// Retry configuration for LLM requests
  #[serde(default)]
  pub retry: RetryConfig,

  /// YOLO mode: auto-approve all tool calls without confirmation
  #[serde(default)]
  pub yolo: bool,

  /// List of tool names to auto-approve even when YOLO mode is off
  #[serde(default)]
  pub auto_approve: Vec<String>,

  /// MCP (Model Context Protocol) server configurations
  #[serde(default)]
  pub mcp: McpConfig,
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
      retry: RetryConfig::default(),
      yolo: false,
      auto_approve: Vec::new(),
      mcp: McpConfig::default(),
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

/// Retry configuration for LLM API requests with exponential backoff.
///
/// Controls how failed requests are retried before giving up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
  /// Maximum number of retry attempts (0 = no retries).
  /// Default is 3.
  #[serde(default = "default_retry_max_attempts")]
  pub max_attempts: u32,

  /// Initial delay in milliseconds before the first retry.
  /// Subsequent retries use exponential backoff: delay * 2^attempt.
  /// Default is 1000ms (1 second).
  #[serde(default = "default_retry_initial_delay_ms")]
  pub initial_delay_ms: u64,

  /// Maximum delay in milliseconds between retries (caps exponential growth).
  /// Default is 5000ms (5 seconds).
  #[serde(default = "default_retry_max_delay_ms")]
  pub max_delay_ms: u64,

  /// Jitter in milliseconds added to each retry delay.
  /// Default is 500ms.
  #[serde(default = "default_retry_jitter_ms")]
  pub jitter_ms: u64,
}

fn default_retry_max_attempts() -> u32 {
  DEFAULT_RETRY_MAX_ATTEMPTS
}

fn default_retry_initial_delay_ms() -> u64 {
  DEFAULT_RETRY_INITIAL_DELAY_MS
}

fn default_retry_max_delay_ms() -> u64 {
  DEFAULT_RETRY_MAX_DELAY_MS
}

fn default_retry_jitter_ms() -> u64 {
  DEFAULT_RETRY_JITTER_MS
}

impl Default for RetryConfig {
  fn default() -> Self {
    Self {
      max_attempts: DEFAULT_RETRY_MAX_ATTEMPTS,
      initial_delay_ms: DEFAULT_RETRY_INITIAL_DELAY_MS,
      max_delay_ms: DEFAULT_RETRY_MAX_DELAY_MS,
      jitter_ms: DEFAULT_RETRY_JITTER_MS,
    }
  }
}

impl RetryConfig {
  /// Calculate the delay for a given attempt number (0-indexed).
  ///
  /// Uses exponential backoff: `initial_delay_ms * 2^attempt`, capped at `max_delay_ms`,
  /// plus a random jitter up to `jitter_ms`.
  pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
    let base_ms = self
      .initial_delay_ms
      .saturating_mul(2u64.saturating_pow(attempt))
      .min(self.max_delay_ms);

    let jitter = if self.jitter_ms > 0 {
      let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
      nanos % (self.jitter_ms + 1)
    } else {
      0
    };

    Duration::from_millis(base_ms.saturating_add(jitter))
  }

  /// Returns true if retries are enabled (max_attempts > 0).
  #[allow(dead_code)]
  pub fn is_enabled(&self) -> bool {
    self.max_attempts > 0
  }
}

// ---------------------------------------------------------------------------
// MCP (Model Context Protocol) configuration
// ---------------------------------------------------------------------------

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
  /// Optional path to an external MCP JSON config file
  /// If set, this file will be loaded in addition to inline servers
  #[serde(skip_serializing_if = "Option::is_none")]
  pub config_file: Option<PathBuf>,

  /// MCP server definitions keyed by server name
  #[serde(default)]
  pub servers: HashMap<String, McpServerConfig>,
}

/// Individual MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
  /// Transport type for the MCP server
  pub transport: McpTransport,

  // HTTP-specific fields
  /// URL for HTTP-based MCP servers
  #[serde(skip_serializing_if = "Option::is_none")]
  pub url: Option<String>,
  /// Headers to send with HTTP requests
  #[serde(default)]
  pub headers: HashMap<String, String>,

  // stdio-specific fields
  /// Command to execute for stdio-based MCP servers
  #[serde(skip_serializing_if = "Option::is_none")]
  pub command: Option<String>,
  /// Arguments to pass to the command
  #[serde(default)]
  pub args: Vec<String>,
  /// Environment variables for the command
  #[serde(default)]
  pub env: HashMap<String, String>,

  // Common fields
  /// Whether this server is temporarily disabled
  #[serde(default)]
  pub disabled: bool,
  /// List of tool names to auto-approve from this server
  #[serde(default)]
  pub auto_approve: Vec<String>,
}

/// Transport type for MCP servers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
  /// Standard I/O transport (spawn a subprocess)
  Stdio,
  /// HTTP/SSE transport (connect to a remote server)
  Http,
}

// ---------------------------------------------------------------------------
// Configuration errors
// ---------------------------------------------------------------------------

/// Configuration-related errors
#[derive(thiserror::Error, Debug)]
pub enum Error {
  #[error("Failed to determine home directory")]
  HomeDirNotFound,

  #[error("Failed to determine config directory")]
  ConfigDirNotFound,

  #[error("Failed to read config file: {path}")]
  ReadFile {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to parse TOML config from: {path}")]
  ParseToml {
    path: PathBuf,
    #[source]
    source: toml::de::Error,
  },

  #[error("Failed to create config directory: {path}")]
  CreateDir {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to write default config to: {path}")]
  WriteFile {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error(
    "Missing required field: default_model. Please specify a default model in your configuration."
  )]
  MissingDefaultModel,

  #[error("Default model '{model}' not found in [models] section.")]
  ModelNotFound { model: String },

  #[error("Provider '{provider}' not found for model '{model}'")]
  ProviderNotFound { provider: String, model: String },

  #[error("API key is required for provider '{provider}' but not provided")]
  MissingApiKey { provider: String },

  #[error("Failed to parse MCP JSON config: {message}")]
  ParseMcpJson { message: String },
}

impl Error {
  /// Create a read file error with path
  pub fn read_file(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Error::ReadFile {
      path: path.into(),
      source,
    }
  }

  /// Create a parse TOML error with path
  pub fn parse_toml(path: impl Into<PathBuf>, source: toml::de::Error) -> Self {
    Error::ParseToml {
      path: path.into(),
      source,
    }
  }

  /// Create a create directory error with path
  pub fn create_dir(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Error::CreateDir {
      path: path.into(),
      source,
    }
  }

  /// Create a write file error with path
  pub fn write_file(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Error::WriteFile {
      path: path.into(),
      source,
    }
  }
}

// ---------------------------------------------------------------------------
// Global configuration
// ---------------------------------------------------------------------------

use std::sync::OnceLock;

static GLOBAL_CONFIG: OnceLock<Config> = OnceLock::new();

/// Initialize the global configuration.
///
/// Must be called exactly once at program startup, after loading the
/// configuration from file.
pub fn init_global_config(config: Config) {
  if GLOBAL_CONFIG.set(config).is_err() {
    panic!("global config already initialized");
  }
}

/// Access the global configuration.
///
/// # Panics
/// Panics if called before `init_global_config`.
pub fn global_config() -> &'static Config {
  GLOBAL_CONFIG.get().expect("global config not initialized")
}
