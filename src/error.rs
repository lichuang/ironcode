//! Error types for IronCode.
//!
//! This module defines all error types used throughout the application.
//! It uses `thiserror` for ergonomic error definition and `anyhow` for
//! convenient error handling at the application boundaries.

use std::path::PathBuf;

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

  #[error("Missing required field: default_model. Please specify a default model in your configuration.")]
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
