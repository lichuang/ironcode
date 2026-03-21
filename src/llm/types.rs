/// A message in a conversation
#[derive(Debug, Clone)]
pub struct Message {
  /// The role of the message author
  pub role: Role,
  /// The content of the message
  pub content: String,
}

impl Message {
  /// Create a new message
  pub fn new(role: Role, content: impl Into<String>) -> Self {
    Self {
      role,
      content: content.into(),
    }
  }

  /// Create a system message
  pub fn system(content: impl Into<String>) -> Self {
    Self::new(Role::System, content)
  }

  /// Create a user message
  pub fn user(content: impl Into<String>) -> Self {
    Self::new(Role::User, content)
  }

  /// Create an assistant message
  pub fn assistant(content: impl Into<String>) -> Self {
    Self::new(Role::Assistant, content)
  }
}

/// The role of a message author
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
  /// System message (e.g., instructions)
  System,
  /// User message
  User,
  /// Assistant message
  Assistant,
}

impl From<Role> for async_openai::types::chat::Role {
  fn from(role: Role) -> Self {
    match role {
      Role::System => async_openai::types::chat::Role::System,
      Role::User => async_openai::types::chat::Role::User,
      Role::Assistant => async_openai::types::chat::Role::Assistant,
    }
  }
}

/// Configuration for chat completion requests
#[derive(Debug, Clone)]
pub struct ChatConfig {
  /// The model to use (e.g., "gpt-4", "gpt-3.5-turbo")
  pub model: String,
  /// Maximum number of tokens to generate
  pub max_tokens: Option<u32>,
  /// Sampling temperature (0.0 - 2.0)
  pub temperature: Option<f32>,
  /// Whether to enable thinking mode (for models that support it, like kimi)
  pub enable_thinking: bool,
}

impl ChatConfig {
  /// Create a new config with the specified model
  pub fn new(model: impl Into<String>) -> Self {
    Self {
      model: model.into(),
      max_tokens: None,
      temperature: None,
      enable_thinking: false,
    }
  }

  /// Set max tokens
  pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
    self.max_tokens = Some(max_tokens);
    self
  }

  /// Set temperature
  pub fn with_temperature(mut self, temperature: f32) -> Self {
    self.temperature = Some(temperature);
    self
  }

  /// Set thinking mode
  pub fn with_thinking(mut self, enable: bool) -> Self {
    self.enable_thinking = enable;
    self
  }
}

impl Default for ChatConfig {
  fn default() -> Self {
    Self::new("gpt-4o")
  }
}
