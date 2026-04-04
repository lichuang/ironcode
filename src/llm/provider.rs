//! LLM Provider trait and related types
//!
//! Defines the interface for LLM providers (Kimi, OpenAI, etc.)

use async_openai::types::chat::ChatCompletionResponseStream;
use async_trait::async_trait;

use crate::error::Result;
use crate::llm::types::Message;

/// Trait for LLM providers
#[async_trait]
pub trait LLMProvider: Send + Sync {
  /// Send a chat completion request with streaming response
  async fn chat_stream(&self, messages: Vec<Message>) -> Result<ChatCompletionResponseStream>;

  #[allow(dead_code)]
  /// Get the provider name
  fn name(&self) -> &str;
}
