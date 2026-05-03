//! StreamManager — LLM streaming with connection recovery and exponential backoff.

use std::sync::Arc;

use async_openai::types::chat::{ChatCompletionMessageToolCallChunk, FunctionCallStream};
use futures::StreamExt;
use log::{error, info, warn};
use tokio::sync::{Mutex, mpsc};

use crate::config::RetryConfig;
use crate::error::{Error, LlmError, Result, StreamErrorCategory};
use crate::llm::provider::{LLMProvider, LlmResponseStream, LlmStreamEvent};
use crate::llm::types::Message;

use super::SessionEvent;

pub struct StreamManager {
  provider: Box<dyn LLMProvider>,
  retry_config: RetryConfig,
  /// Persistent tool call buffer across stream attempts within a single step.
  /// If a stream is interrupted while tool calls are being received, the
  /// partial buffer remains here so that the next `start()` can detect and
  /// discard it rather than silently losing the fragments.
  tool_call_buffer: Arc<Mutex<Vec<ChatCompletionMessageToolCallChunk>>>,
}

impl StreamManager {
  pub fn new(provider: Box<dyn LLMProvider>, retry_config: RetryConfig) -> Self {
    Self {
      provider,
      retry_config,
      tool_call_buffer: Arc::new(Mutex::new(Vec::new())),
    }
  }

  /// Start the stream with two-layer retry (connection recovery + exponential backoff).
  /// Returns a receiver that the caller consumes in a select! loop.
  pub async fn start(
    &mut self,
    messages: Vec<Message>,
  ) -> Result<mpsc::UnboundedReceiver<SessionEvent>> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut attempt = 0u32;
    let max_attempts = self.retry_config.max_attempts.max(1);

    while attempt < max_attempts {
      match self.try_start_stream(messages.clone(), &tx).await {
        Ok(()) => return Ok(rx),
        Err((err, recovery_exhausted)) => {
          attempt += 1;
          let is_retryable = !recovery_exhausted && is_error_retryable(&err);
          if !is_retryable || attempt >= max_attempts {
            return Err(err);
          }
          let delay = self.retry_config.delay_for_attempt(attempt - 1);
          warn!(
            "Stream: attempt {}/{} failed ({}), retrying in {:?}",
            attempt, max_attempts, err, delay
          );
          tokio::time::sleep(delay).await;
        }
      }
    }
    unreachable!()
  }

  async fn try_start_stream(
    &mut self,
    messages: Vec<Message>,
    tx: &mpsc::UnboundedSender<SessionEvent>,
  ) -> std::result::Result<(), (Error, bool)> {
    let stream = match self.provider.chat_stream(messages.clone()).await {
      Ok(s) => s,
      Err(err) => {
        let is_connection_error = matches!(
          &err,
          Error::Llm(LlmError::Stream {
            category: StreamErrorCategory::Timeout
              | StreamErrorCategory::Disconnected
              | StreamErrorCategory::Transport,
            ..
          }) | Error::Llm(LlmError::EmptyResponse)
        );

        if is_connection_error {
          info!(
            "Stream: connection error, attempting immediate recovery: {}",
            err
          );
          if let Error::Llm(ref llm_err) = err {
            self.provider.on_retryable_error(llm_err).await;
          }
          match self.provider.chat_stream(messages).await {
            Ok(s) => {
              info!("Stream: connection recovery succeeded");
              s
            }
            Err(retry_err) => {
              warn!("Stream: connection recovery failed: {}", retry_err);
              let is_still_connection = matches!(
                &retry_err,
                Error::Llm(LlmError::Stream {
                  category: StreamErrorCategory::Timeout
                    | StreamErrorCategory::Disconnected
                    | StreamErrorCategory::Transport,
                  ..
                }) | Error::Llm(LlmError::EmptyResponse)
              );
              return Err((retry_err, is_still_connection));
            }
          }
        } else {
          return Err((err, false));
        }
      }
    };

    // If a previous stream left an incomplete tool-call buffer, log and clear it.
    {
      let mut buffer = self.tool_call_buffer.lock().await;
      if !buffer.is_empty() {
        warn!(
          "Stream: discarding {} incomplete tool-call fragment(s) from previous attempt",
          buffer.len()
        );
        buffer.clear();
      }
    }

    let tx = tx.clone();
    let buffer = self.tool_call_buffer.clone();
    tokio::spawn(async move {
      handle_stream(stream, tx, buffer).await;
    });
    Ok(())
  }
}

async fn handle_stream(
  mut stream: LlmResponseStream,
  tx: mpsc::UnboundedSender<SessionEvent>,
  tool_call_buffer: Arc<Mutex<Vec<ChatCompletionMessageToolCallChunk>>>,
) {
  while let Some(result) = stream.next().await {
    match result {
      Ok(LlmStreamEvent::Content(chunk)) => {
        if !chunk.is_empty() && tx.send(SessionEvent::ContentChunk(chunk)).is_err() {
          return;
        }
      }
      Ok(LlmStreamEvent::Thinking(chunk)) => {
        if !chunk.is_empty() && tx.send(SessionEvent::ThinkingChunk(chunk)).is_err() {
          return;
        }
      }
      Ok(LlmStreamEvent::ToolCallChunk {
        index,
        id,
        name,
        arguments,
      }) => {
        let mut buffer = tool_call_buffer.lock().await;
        let idx = index as usize;
        while buffer.len() <= idx {
          let len = buffer.len();
          buffer.push(ChatCompletionMessageToolCallChunk {
            index: len as u32,
            id: None,
            r#type: None,
            function: None,
          });
        }
        let existing = &mut buffer[idx];
        if let Some(id) = id {
          existing.id = Some(id);
        }
        if let Some(name) = name {
          if existing.function.is_none() {
            existing.function = Some(FunctionCallStream {
              name: None,
              arguments: None,
            });
          }
          if let Some(ref mut func) = existing.function {
            func.name = Some(name);
          }
        }
        if let Some(args) = arguments {
          if existing.function.is_none() {
            existing.function = Some(FunctionCallStream {
              name: None,
              arguments: None,
            });
          }
          if let Some(ref mut func) = existing.function {
            if let Some(ref existing_args) = func.arguments {
              func.arguments = Some(format!("{}{}", existing_args, args));
            } else {
              func.arguments = Some(args);
            }
          }
        }
      }
      Ok(LlmStreamEvent::Usage {
        total_tokens,
        prompt_tokens,
        completion_tokens,
      }) => {
        let _ = tx.send(SessionEvent::Usage {
          total_tokens,
          prompt_tokens,
          completion_tokens,
        });
      }
      Err(e) => {
        error!("Stream: Stream error: {}", e);
        let buffer = tool_call_buffer.lock().await;
        if !buffer.is_empty() {
          warn!(
            "Stream: discarding {} incomplete tool-call fragment(s) due to interruption",
            buffer.len()
          );
        }
        drop(buffer);
        let is_retryable = is_error_retryable(&e);
        let _ = tx.send(SessionEvent::StreamInterrupted {
          error: e.to_string(),
          is_retryable,
        });
        return;
      }
    }
  }

  // Stream ended — flush any remaining tool calls and signal completion.
  let mut buffer = tool_call_buffer.lock().await;
  for tool_call in buffer.drain(..) {
    if let (Some(id), Some(function)) = (tool_call.id, tool_call.function)
      && let (Some(name), Some(arguments)) = (function.name, function.arguments)
      && !id.is_empty()
      && !name.is_empty()
    {
      let _ = tx.send(SessionEvent::ToolCallReceived {
        id,
        name,
        arguments,
      });
    }
  }
  let _ = tx.send(SessionEvent::Completed);
}

fn is_error_retryable(err: &Error) -> bool {
  match err {
    Error::Llm(llm_err) => llm_err.is_retryable(),
    _ => false,
  }
}

pub fn format_user_friendly_error(err: &str) -> String {
  if err.contains("Stream timeout") || err.contains("timed out") {
    "Connection timed out while waiting for the response. Please check your network and try again."
      .to_string()
  } else if err.contains("Connection lost") {
    "The connection was interrupted. Please check your network and try again.".to_string()
  } else if err.contains("Transport error") {
    "A network error occurred. Please check your connection and try again.".to_string()
  } else if err.contains("HTTP 429") {
    "Rate limit exceeded. Please wait a moment and try again.".to_string()
  } else if err.contains("HTTP 500")
    || err.contains("HTTP 502")
    || err.contains("HTTP 503")
    || err.contains("HTTP 504")
  {
    "The server is temporarily unavailable. Please wait a moment and try again.".to_string()
  } else if err.contains("HTTP 401") || err.contains("HTTP 403") {
    "Authentication failed. Please check your API key and try again.".to_string()
  } else if err.contains("HTTP 400") {
    "The request was invalid. Please check your input and try again.".to_string()
  } else {
    format!("An error occurred: {}. Please try again.", err)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::error::LlmError;

  #[test]
  fn test_is_error_retryable_stream_timeout() {
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Timeout,
      status_code: None,
      message: "timeout".to_string(),
    });
    assert!(is_error_retryable(&err));
  }

  #[test]
  fn test_is_error_retryable_stream_disconnected() {
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Disconnected,
      status_code: None,
      message: "disconnected".to_string(),
    });
    assert!(is_error_retryable(&err));
  }

  #[test]
  fn test_is_error_retryable_stream_transport() {
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Transport,
      status_code: None,
      message: "transport".to_string(),
    });
    assert!(is_error_retryable(&err));
  }

  #[test]
  fn test_is_error_retryable_stream_http_500() {
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Http,
      status_code: Some(500),
      message: "500".to_string(),
    });
    assert!(is_error_retryable(&err));
  }

  #[test]
  fn test_is_error_retryable_stream_http_429() {
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Http,
      status_code: Some(429),
      message: "429".to_string(),
    });
    assert!(is_error_retryable(&err));
  }

  #[test]
  fn test_is_error_retryable_stream_http_400() {
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Http,
      status_code: Some(400),
      message: "bad request".to_string(),
    });
    assert!(!is_error_retryable(&err));
  }

  #[test]
  fn test_is_error_retryable_stream_parse() {
    let err = Error::Llm(LlmError::Stream {
      category: StreamErrorCategory::Parse,
      status_code: None,
      message: "parse error".to_string(),
    });
    // Parse errors are retryable: a corrupted SSE chunk may succeed on retry.
    assert!(is_error_retryable(&err));
  }

  #[test]
  fn test_is_error_retryable_non_llm_error() {
    let err = Error::Llm(LlmError::EmptyResponse);
    assert!(is_error_retryable(&err));
  }

  #[test]
  fn test_format_user_friendly_error_timeout() {
    assert!(format_user_friendly_error("Stream timeout").contains("timed out"));
  }

  #[test]
  fn test_format_user_friendly_error_connection_lost() {
    assert!(format_user_friendly_error("Connection lost").contains("interrupted"));
  }

  #[test]
  fn test_format_user_friendly_error_rate_limit() {
    assert!(format_user_friendly_error("HTTP 429").contains("Rate limit exceeded"));
  }

  #[test]
  fn test_format_user_friendly_error_server_error() {
    assert!(format_user_friendly_error("HTTP 503").contains("temporarily unavailable"));
  }

  #[test]
  fn test_format_user_friendly_error_auth_error() {
    assert!(format_user_friendly_error("HTTP 401").contains("Authentication failed"));
  }

  #[test]
  fn test_format_user_friendly_error_generic() {
    let result = format_user_friendly_error("Some random error");
    assert!(result.contains("Some random error"));
    assert!(result.contains("Please try again"));
  }
}
