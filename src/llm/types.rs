/// A message in a conversation
#[derive(Debug, Clone)]
pub struct Message {
  /// The role of the message author
  pub role: Role,
  /// The content of the message
  pub content: String,
  /// Tool calls requested by the assistant (only for assistant messages)
  pub tool_calls: Option<Vec<ToolCall>>,
  /// The ID of the tool call this message is responding to (only for tool messages)
  pub tool_call_id: Option<String>,
}

impl Message {
  /// Create a new message
  pub fn new(role: Role, content: impl Into<String>) -> Self {
    Self {
      role,
      content: content.into(),
      tool_calls: None,
      tool_call_id: None,
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

  /// Create an assistant message with tool calls
  pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
    Self {
      role: Role::Assistant,
      content: content.into(),
      tool_calls: Some(tool_calls),
      tool_call_id: None,
    }
  }

  /// Create a tool result message
  pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
    Self {
      role: Role::Tool,
      content: content.into(),
      tool_calls: None,
      tool_call_id: Some(tool_call_id.into()),
    }
  }
}

/// A tool call requested by the assistant
#[derive(Debug, Clone)]
pub struct ToolCall {
  /// The ID of the tool call
  pub id: String,
  /// The name of the tool to call
  pub name: String,
  /// The arguments for the tool call (JSON string)
  pub arguments: String,
}

impl ToolCall {
  /// Create a new tool call
  pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: impl Into<String>) -> Self {
    Self {
      id: id.into(),
      name: name.into(),
      arguments: arguments.into(),
    }
  }
}

/// The result of a tool execution
#[derive(Debug, Clone)]
pub struct ToolResult {
  /// The ID of the tool call this result is for
  pub tool_call_id: String,
  /// The output from the tool
  pub output: String,
  /// Whether this is an error result
  pub is_error: bool,
}

impl ToolResult {
  /// Create a successful tool result
  pub fn success(tool_call_id: impl Into<String>, output: impl Into<String>) -> Self {
    Self {
      tool_call_id: tool_call_id.into(),
      output: output.into(),
      is_error: false,
    }
  }

  /// Create an error tool result
  pub fn error(tool_call_id: impl Into<String>, message: impl Into<String>) -> Self {
    Self {
      tool_call_id: tool_call_id.into(),
      output: message.into(),
      is_error: true,
    }
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
  /// Tool result message
  Tool,
}

use async_openai::types::chat::Role as OpenAIRole;

impl From<Role> for OpenAIRole {
  fn from(role: Role) -> Self {
    match role {
      Role::System => OpenAIRole::System,
      Role::User => OpenAIRole::User,
      Role::Assistant => OpenAIRole::Assistant,
      Role::Tool => OpenAIRole::Tool,
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
