//! Error types for IronCode.
//!
//! This module defines all error types used throughout the application.
//! It uses `thiserror` for ergonomic error definition and `anyhow` for
//! convenient error handling at the application boundaries.

use async_openai::error::OpenAIError;
use log::warn;

/// Result type alias using our Error type
pub type Result<T> = std::result::Result<T, Error>;

/// The main error type for IronCode
#[derive(thiserror::Error, Debug)]
pub enum Error {
  /// Configuration-related errors
  #[error(transparent)]
  Config(#[from] crate::config::Error),

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
  Session(#[from] crate::session::Error),

  /// Runtime environment errors
  #[error(transparent)]
  Runtime(#[from] crate::cli::runtime::Error),

  /// IO errors
  #[error(transparent)]
  Io(#[from] std::io::Error),
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
        StreamErrorCategory::Http => {
          let status = status_code.unwrap_or(0);
          let retry = is_http_status_retryable(status);
          if !retry && status != 0 {
            warn!("HTTP {} error classified as non-retryable", status);
          }
          retry
        }
        StreamErrorCategory::Transport => true,
        StreamErrorCategory::Parse => true,
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
/// Classification:
/// - Always retry: 429 (rate limit), 5xx (server errors)
/// - Never retry: 400/422 (client request error), 401/403 (auth error)
/// - Conservative: unknown status codes are retried
fn is_http_status_retryable(status: u16) -> bool {
  match status {
    429 | 500 | 502 | 503 | 504 => true,
    400 | 401 | 403 | 422 => false,
    _ => true, // conservative: retry unknown status codes
  }
}

/// Fallback classification for async-openai `OpenAIError` variants.
///
/// Kimi provider does not produce these errors (it uses reqwest directly),
/// but other providers using the async-openai client might.
fn is_openai_error_retryable(err: &OpenAIError) -> bool {
  match err {
    OpenAIError::Reqwest(reqwest_err) => {
      // If the error carries an HTTP status code, use the same rules as
      // our native HTTP classification.  This prevents 401/403/400 from
      // being incorrectly retried because `is_request()` returns true.
      if let Some(status) = reqwest_err.status() {
        return is_http_status_retryable(status.as_u16());
      }
      // Network-level errors without a status code (timeout, connection reset).
      reqwest_err.is_timeout() || reqwest_err.is_connect()
    }
    OpenAIError::StreamError(_) => true,
    _ => false,
  }
}
