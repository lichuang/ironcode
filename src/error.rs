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

  #[error("Streaming error: {0}")]
  StreamError(String),

  /// A transient error that can be retried (network, rate limit, server error).
  #[error("Retryable error: {message}")]
  Retryable {
    /// Human-readable error message
    message: String,
  },
}

impl LlmError {
  /// Check if this error is retryable (transient network/server errors).
  ///
  /// Returns true for:
  /// - Rate limit errors (HTTP 429)
  /// - Server errors (HTTP 5xx)
  /// - Network/timeout errors
  /// - Explicit `Retryable` variant
  ///
  /// Returns false for:
  /// - Authentication errors (HTTP 401/403)
  /// - Bad request errors (HTTP 400)
  /// - Configuration errors
  pub fn is_retryable(&self) -> bool {
    match self {
      // Explicit retryable error
      LlmError::Retryable { .. } => true,
      // Stream errors are transient by nature (connection interrupted mid-stream)
      LlmError::StreamError(_) => true,
      // OpenAI API errors: check for retryable conditions via enum matching
      LlmError::OpenAI(err) => is_openai_error_retryable(err),
      LlmError::BuildRequest { .. } => false,
      LlmError::EmptyResponse => false,
      LlmError::InvalidConfig(_) => false,
    }
  }
}

/// Check if an OpenAI API error is retryable using enum matching.
///
/// Note: async-openai's own client already retries 429/5xx internally, but
/// Kimi provider bypasses that client (uses `reqwest_eventsource` directly),
/// so we need our own retry classification here.
///
/// Classification strategy:
/// - `ApiError` — check `r#type` field for known retryable error types.
///   HTTP status codes are NOT stored in `ApiError`; for 5xx errors
///   async-openai constructs `ApiError` with all fields as `None`,
///   so we treat any `ApiError` where `r#type` indicates a server/rate-limit
///   issue as retryable. If `r#type` is `None` (typical for 5xx), we conservatively
///   treat it as retryable since the API did return an error.
/// - `Reqwest` — uses reqwest's `is_timeout()` / `is_connect()` methods
/// - `StreamError` — transient by nature (connection interrupted mid-stream)
pub fn is_openai_error_retryable(err: &OpenAIError) -> bool {
  match err {
    // API returned an error response — check the type field
    OpenAIError::ApiError(api_err) => is_api_error_type_retryable(api_err.r#type.as_deref()),

    // Network-level error from reqwest — use its precise classification
    OpenAIError::Reqwest(reqwest_err) => {
      reqwest_err.is_timeout() || reqwest_err.is_connect() || reqwest_err.is_request()
    }

    // Stream interrupted — transient by nature
    OpenAIError::StreamError(_) => true,

    // All other variants (deserialization, file I/O, argument, etc.) are not retryable
    _ => false,
  }
}

/// Check if an API error `type` field indicates a retryable condition.
///
/// The `r#type` field from OpenAI API error responses typically contains:
/// - `"server_error"` — 5xx server errors
/// - `"rate_limit_error"` — rate limiting (429)
/// - `"insufficient_quota"` — billing/quota exhaustion (NOT retryable)
/// - `"invalid_request_error"` — bad request (NOT retryable)
/// - `"authentication_error"` — auth failure (NOT retryable)
/// - `None` — unknown; for 5xx, async-openai constructs ApiError with all fields as None
///
/// When `r#type` is `None`, we conservatively treat it as retryable,
/// since async-openai sets all fields to None for server errors (5xx).
pub fn is_api_error_type_retryable(error_type: Option<&str>) -> bool {
  match error_type {
    // Known retryable types
    Some("server_error") | Some("rate_limit_error") | Some("timeout") => true,
    // Known non-retryable types
    Some("invalid_request_error")
    | Some("authentication_error")
    | Some("permission_error")
    | Some("insufficient_quota")
    | Some("model_not_found") => false,
    // None typically means 5xx (async-openai constructs ApiError with all fields as None)
    None => true,
    // Unknown types — conservatively treat as not retryable
    Some(_) => false,
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
