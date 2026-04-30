//! Error types for IronCode.
//!
//! This module defines all error types used throughout the application.
//! It uses `thiserror` for ergonomic error definition and `anyhow` for
//! convenient error handling at the application boundaries.

use std::path::PathBuf;

use async_openai::error::OpenAIError;

/// Result type alias using our Error type
pub type Result<T> = std::result::Result<T, Error>;

/// The main error type for IronCode
#[derive(thiserror::Error, Debug)]
pub enum Error {
  /// Configuration-related errors
  #[error(transparent)]
  Config(#[from] ConfigError),

  /// TUI/Terminal-related errors
  #[error(transparent)]
  Tui(#[from] TuiError),

  /// LLM-related errors
  #[error(transparent)]
  Llm(#[from] LlmError),

  /// OpenAI API errors
  #[error("OpenAI API error: {0}")]
  OpenAI(#[from] async_openai::error::OpenAIError),

  /// Session persistence errors
  #[error(transparent)]
  Session(#[from] SessionError),

  /// Runtime environment errors
  #[error(transparent)]
  Runtime(#[from] RuntimeError),

  /// IO errors
  #[error(transparent)]
  Io(#[from] std::io::Error),
}

/// Configuration errors
#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
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

/// TUI/Terminal errors
#[derive(thiserror::Error, Debug)]
pub enum TuiError {
  #[error("Failed to initialize terminal")]
  InitTerminal {
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to restore terminal")]
  RestoreTerminal {
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to create terminal backend")]
  CreateBackend {
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to draw frame")]
  DrawFrame {
    #[source]
    source: std::io::Error,
  },
}

/// LLM-related errors
#[derive(thiserror::Error, Debug)]
pub enum LlmError {
  #[error("OpenAI API error: {0}")]
  OpenAI(#[from] async_openai::error::OpenAIError),

  #[error("Failed to build chat completion request")]
  BuildRequest {
    #[source]
    source: async_openai::error::OpenAIError,
  },

  #[error("No response content from API")]
  EmptyResponse,

  #[error("Invalid model configuration: {0}")]
  InvalidConfig(String),

  /// SSE stream error that occurred after connection was established.
  ///
  /// The `category` field distinguishes the error cause for precise retry decisions,
  /// mirroring kimi-cli's error classification:
  /// - `Timeout` → kimi-cli's `APITimeoutError`
  /// - `Disconnected` → kimi-cli's `APIConnectionError`
  /// - `Http` → kimi-cli's `APIStatusError` (with `status_code`)
  /// - `Transport` → other transport errors
  /// - `Parse` → UTF-8/SSE parsing errors (not retryable)
  #[error("Stream {category}: {message}")]
  Stream {
    category: StreamErrorCategory,
    status_code: Option<u16>,
    message: String,
  },
}

/// Category of SSE stream error, used for precise retry decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamErrorCategory {
  /// Connection timeout during streaming.
  Timeout,
  /// Connection lost/reset during streaming.
  Disconnected,
  /// Server returned a non-2xx HTTP status during streaming.
  Http,
  /// Other transport error during streaming.
  Transport,
  /// UTF-8 or SSE protocol parsing error.
  Parse,
}

impl std::fmt::Display for StreamErrorCategory {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      StreamErrorCategory::Timeout => write!(f, "timeout"),
      StreamErrorCategory::Disconnected => write!(f, "disconnected"),
      StreamErrorCategory::Http => write!(f, "HTTP error"),
      StreamErrorCategory::Transport => write!(f, "transport error"),
      StreamErrorCategory::Parse => write!(f, "parse error"),
    }
  }
}

impl LlmError {
  /// Check if this error is retryable (transient network/server errors).
  ///
  /// Classification mirrors kimi-cli's `_is_retryable_error`:
  /// - `Network` (→ `APIConnectionError` / `APITimeoutError`) — always retryable
  /// - `Http` (→ `APIStatusError`) — retryable for 429, 500, 502, 503, 504
  /// - `Stream::Timeout` (→ `APITimeoutError`) — retryable
  /// - `Stream::Disconnected` (→ `APIConnectionError`) — retryable
  /// - `Stream::Http` (→ `APIStatusError`) — retryable for 429, 500, 502, 503, 504
  /// - `Stream::Transport` — retryable
  /// - `Stream::Parse` — NOT retryable
  /// - `EmptyResponse` (→ `APIEmptyResponseError`) — retryable
  /// - All others — not retryable
  pub fn is_retryable(&self) -> bool {
    match self {
      LlmError::Stream {
        category,
        status_code,
        ..
      } => match category {
        StreamErrorCategory::Timeout => true,
        StreamErrorCategory::Disconnected => true,
        StreamErrorCategory::Http => is_http_status_retryable(status_code.unwrap_or(0)),
        StreamErrorCategory::Transport => true,
        StreamErrorCategory::Parse => false,
      },
      LlmError::EmptyResponse => true,
      LlmError::InvalidConfig(_) => false,
      LlmError::BuildRequest { .. } => false,
      LlmError::OpenAI(err) => is_openai_error_retryable(err),
    }
  }
}

/// Check if an HTTP status code is retryable.
///
/// Matches kimi-cli's classification: 429 (rate limit) and 5xx (server errors).
fn is_http_status_retryable(status: u16) -> bool {
  matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Fallback classification for async-openai `OpenAIError` variants.
///
/// Kimi provider does not produce these errors (it uses reqwest directly),
/// but other providers using the async-openai client might.
fn is_openai_error_retryable(err: &OpenAIError) -> bool {
  match err {
    OpenAIError::Reqwest(reqwest_err) => {
      reqwest_err.is_timeout() || reqwest_err.is_connect() || reqwest_err.is_request()
    }
    OpenAIError::StreamError(_) => true,
    _ => false,
  }
}

/// Session persistence errors
#[derive(thiserror::Error, Debug)]
pub enum SessionError {
  #[error("Session '{id}' not found")]
  NotFound { id: String },

  #[error("Failed to serialize message: {source}")]
  SerializeMessage { source: serde_json::Error },

  #[error("Failed to serialize session meta: {source}")]
  SerializeMeta { source: serde_json::Error },

  #[error("Failed to deserialize message: {source}")]
  DeserializeMessage { source: serde_json::Error },

  #[error("Failed to deserialize session meta: {source}")]
  DeserializeMeta { source: serde_json::Error },

  #[error("Failed to read session meta for '{id}': {source}")]
  ReadMeta { id: String, source: std::io::Error },

  #[error("Failed to write session meta for '{id}': {source}")]
  WriteMeta { id: String, source: std::io::Error },
}

/// Runtime environment errors
#[derive(thiserror::Error, Debug)]
pub enum RuntimeError {
  #[error("Failed to get current directory")]
  GetCurrentDir {
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to read directory: {path}")]
  ReadDir {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to read file metadata: {path}")]
  ReadMetadata {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to read system prompt from: {path}")]
  ReadSystemPrompt {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Tool '{tool_name}' is defined in prompts but no handler is implemented")]
  MissingToolHandler { tool_name: String },
}

// Helper methods for error creation
impl ConfigError {
  /// Create a read file error with path
  pub fn read_file(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    ConfigError::ReadFile {
      path: path.into(),
      source,
    }
  }

  /// Create a parse TOML error with path
  pub fn parse_toml(path: impl Into<PathBuf>, source: toml::de::Error) -> Self {
    ConfigError::ParseToml {
      path: path.into(),
      source,
    }
  }

  /// Create a create directory error with path
  pub fn create_dir(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    ConfigError::CreateDir {
      path: path.into(),
      source,
    }
  }

  /// Create a write file error with path
  pub fn write_file(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    ConfigError::WriteFile {
      path: path.into(),
      source,
    }
  }
}

impl RuntimeError {
  /// Create a read directory error with path
  pub fn read_dir(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    RuntimeError::ReadDir {
      path: path.into(),
      source,
    }
  }

  /// Create a read metadata error with path
  pub fn read_metadata(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    RuntimeError::ReadMetadata {
      path: path.into(),
      source,
    }
  }

  /// Create a read system prompt error with path
  pub fn read_system_prompt(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    RuntimeError::ReadSystemPrompt {
      path: path.into(),
      source,
    }
  }
}
