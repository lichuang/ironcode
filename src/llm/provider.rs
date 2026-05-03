//! LLM Provider trait and related types
//!
//! Defines the interface for LLM providers (Kimi, OpenAI, etc.)

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::error::{LlmError, Result};
use crate::llm::types::Message;

/// Events emitted by an LLM stream.
///
/// Replaces the OpenAI-specific `CreateChatCompletionStreamResponse` with a
/// provider-agnostic event stream that natively supports reasoning content.
#[derive(Debug, Clone)]
pub enum LlmStreamEvent {
  /// A chunk of normal content text.
  Content(String),
  /// A chunk of thinking / reasoning content.
  Thinking(String),
  /// A partial tool call chunk.
  ToolCallChunk {
    index: u32,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
  },
  /// Token usage information.
  Usage {
    total_tokens: u32,
    prompt_tokens: u32,
    completion_tokens: u32,
  },
}

/// A stream of LLM events.
pub type LlmResponseStream = Pin<Box<dyn Stream<Item = Result<LlmStreamEvent>> + Send>>;

/// Trait for LLM providers
#[async_trait]
pub trait LLMProvider: Send + Sync {
  /// Send a chat completion request with streaming response
  async fn chat_stream(&self, messages: Vec<Message>) -> Result<LlmResponseStream>;

  #[allow(dead_code)]
  /// Get the provider name
  fn name(&self) -> &str;

  /// Called when a retryable error occurs to allow the provider to
  /// refresh its connection state (e.g. rebuild HTTP client).
  async fn on_retryable_error(&mut self, _error: &LlmError) {}

  /// Maximum context size (token limit) for the model this provider uses.
  fn max_context_size(&self) -> usize;
}
