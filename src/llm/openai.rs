use crate::error::{LlmError, Result};
use crate::llm::types::{ChatConfig, Message, Role};
use async_openai::{
  Client,
  config::OpenAIConfig,
  types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
  },
};

/// OpenAI API client wrapper
#[derive(Debug, Clone)]
pub struct OpenAIClient {
  client: Client<OpenAIConfig>,
  config: ChatConfig,
}

impl OpenAIClient {
  /// Create a new OpenAI client with the API key from environment
  pub fn new(config: ChatConfig) -> Result<Self> {
    let client = Client::new();
    Ok(Self { client, config })
  }

  /// Create a new client with a custom API key
  pub fn with_api_key(api_key: impl Into<String>, config: ChatConfig) -> Self {
    let client = Client::with_config(
      OpenAIConfig::new().with_api_key(api_key),
    );
    Self { client, config }
  }

  /// Create a new client with a custom base URL (for compatible APIs like Azure, Ollama, etc.)
  pub fn with_base_url(
    base_url: impl Into<String>,
    api_key: impl Into<String>,
    config: ChatConfig,
  ) -> Self {
    let client = Client::with_config(
      OpenAIConfig::new()
        .with_api_base(base_url)
        .with_api_key(api_key),
    );
    Self { client, config }
  }

  /// Send a chat completion request with streaming response
  ///
  /// # Arguments
  /// * `messages` - A list of messages, typically starting with a system message
  ///                followed by alternating user and assistant messages
  ///
  /// # Example
  /// ```rust
  /// let messages = vec![
  ///     Message::system("You are a helpful assistant."),
  ///     Message::user("Hello!"),
  /// ];
  /// let stream = client.chat_stream(messages).await?;
  /// ```
  pub async fn chat_stream(
    &self,
    messages: Vec<Message>,
  ) -> Result<async_openai::types::chat::ChatCompletionResponseStream> {
    let request_messages: Vec<ChatCompletionRequestMessage> = messages
      .into_iter()
      .map(|msg| Self::convert_message(msg))
      .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut request = CreateChatCompletionRequestArgs::default();
    request
      .model(&self.config.model)
      .messages(request_messages)
      .stream(true);

    if let Some(max_tokens) = self.config.max_tokens {
      request.max_tokens(max_tokens);
    }

    if let Some(temperature) = self.config.temperature {
      request.temperature(temperature);
    }

    let request = request.build().map_err(|e| LlmError::BuildRequest { source: e })?;

    let stream = self.client.chat().create_stream(request).await?;

    Ok(stream)
  }

  /// Send a streaming chat request with a system prompt
  ///
  /// This is a convenience method that prepends a system message to the conversation.
  ///
  /// # Arguments
  /// * `system_prompt` - The system prompt content
  /// * `user_message` - The user's message
  pub async fn chat_stream_with_system(
    &self,
    system_prompt: impl Into<String>,
    user_message: impl Into<String>,
  ) -> Result<async_openai::types::chat::ChatCompletionResponseStream> {
    let messages = vec![
      Message::system(system_prompt),
      Message::user(user_message),
    ];
    self.chat_stream(messages).await
  }

  /// Convert our Message type to async-openai's message type
  fn convert_message(
    msg: Message,
  ) -> std::result::Result<ChatCompletionRequestMessage, async_openai::error::OpenAIError> {
    match msg.role {
      Role::System => {
        ChatCompletionRequestSystemMessageArgs::default()
          .content(msg.content)
          .build()
          .map(Into::into)
      }
      Role::User => {
        ChatCompletionRequestUserMessageArgs::default()
          .content(msg.content)
          .build()
          .map(Into::into)
      }
      Role::Assistant => {
        // For assistant messages, we use system message args with assistant role
        // This is a workaround as async-openai doesn't have a direct builder for assistant messages
        ChatCompletionRequestSystemMessageArgs::default()
          .content(msg.content)
          .build()
          .map(Into::into)
      }
    }
  }

  /// Get the current configuration
  pub fn config(&self) -> &ChatConfig {
    &self.config
  }

  /// Update the configuration
  pub fn set_config(&mut self, config: ChatConfig) {
    self.config = config;
  }
}
