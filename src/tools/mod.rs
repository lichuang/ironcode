//! Tool definitions and loading from Markdown files.
//!
//! Tools are defined in Markdown files located in `prompts/tools/` directory.
//! Each Markdown file should have the following format:
//!
//! ```markdown
//! ---
//! name: ToolName
//! description: Tool description here
//! ---
//!
//! ## Parameters
//!
//! ```json
//! {
//!   "type": "object",
//!   "properties": {
//!     "param1": {
//!       "type": "string",
//!       "description": "Parameter description"
//!     }
//!   },
//!   "required": ["param1"]
//! }
//! ```
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use serde::{Deserialize, Serialize};

pub mod handlers;
pub mod loader;

/// A tool definition for function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
  /// Tool name (must be unique)
  pub name: String,
  /// Tool description (shown to LLM to help it decide when to use)
  pub description: String,
  /// JSON Schema for parameters
  pub parameters: serde_json::Value,
}

impl Tool {
  /// Create a new tool
  pub fn new(
    name: impl Into<String>,
    description: impl Into<String>,
    parameters: serde_json::Value,
  ) -> Self {
    Self {
      name: name.into(),
      description: description.into(),
      parameters,
    }
  }

  /// Convert to OpenAI ChatCompletionTools format
  pub fn to_openai_tool(&self) -> ChatCompletionTools {
    ChatCompletionTools::Function(ChatCompletionTool {
      function: FunctionObject {
        name: self.name.clone(),
        description: Some(self.description.clone()),
        parameters: Some(self.parameters.clone()),
        strict: None,
      },
    })
  }
}

/// Tool registry holding all loaded tools
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
  tools: HashMap<String, Tool>,
}

impl ToolRegistry {
  /// Create a new empty registry
  pub fn new() -> Self {
    Self {
      tools: HashMap::new(),
    }
  }

  /// Add a tool to the registry
  pub fn add(&mut self, tool: Tool) {
    self.tools.insert(tool.name.clone(), tool);
  }

  /// Get a tool by name
  pub fn get(&self, name: &str) -> Option<&Tool> {
    self.tools.get(name)
  }

  /// Get all tools as a vector
  pub fn all(&self) -> Vec<&Tool> {
    self.tools.values().collect()
  }

  /// Get tools count
  pub fn len(&self) -> usize {
    self.tools.len()
  }

  /// Check if registry is empty
  pub fn is_empty(&self) -> bool {
    self.tools.is_empty()
  }

  /// Convert all tools to OpenAI format
  pub fn to_openai_tools(&self) -> Vec<ChatCompletionTools> {
    self.tools.values().map(|t| t.to_openai_tool()).collect()
  }

  /// Load tools from a specific directory
  pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self> {
    loader::load_tools_from_dir(dir)
  }

  /// Load tools from the default directory (`prompts/tools/`)
  pub fn load_default() -> Result<Self> {
    let tools_dir = PathBuf::from("prompts/tools");
    Self::load_from_dir(&tools_dir)
  }
}

// ============================================================================
// Tool Execution Framework (inspired by codex-rs)
// ============================================================================

use async_trait::async_trait;
use thiserror::Error;

/// Errors that can occur during tool execution
#[derive(Debug, Error)]
pub enum ToolError {
  /// Error that should be reported back to the model (e.g., invalid arguments)
  #[error("{0}")]
  RespondToModel(String),

  /// Fatal error that should stop execution
  #[error("Fatal error: {0}")]
  Fatal(String),
}

/// The kind of tool
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
  Function,
  Mcp,
}

/// Input payload for tool invocation
#[derive(Debug, Clone)]
pub enum ToolPayload {
  /// Standard function call with JSON arguments
  Function { arguments: String },
  /// MCP tool call
  Mcp {
    server: String,
    tool: String,
    raw_arguments: String,
  },
}

impl ToolPayload {
  /// Get the arguments as a string for logging
  pub fn log_payload(&self) -> String {
    match self {
      ToolPayload::Function { arguments } => arguments.clone(),
      ToolPayload::Mcp { raw_arguments, .. } => raw_arguments.clone(),
    }
  }
}

/// Output from a tool execution
#[derive(Debug, Clone)]
pub enum ToolOutput {
  /// Successful function output
  Function { output: String },
  /// Error output
  Error { message: String },
}

impl ToolOutput {
  /// Create a successful output
  pub fn success(output: impl Into<String>) -> Self {
    ToolOutput::Function {
      output: output.into(),
    }
  }

  /// Create an error output
  pub fn error(message: impl Into<String>) -> Self {
    ToolOutput::Error {
      message: message.into(),
    }
  }

  /// Convert to response string for the model
  pub fn into_response(self) -> String {
    match self {
      ToolOutput::Function { output } => output,
      ToolOutput::Error { message } => format!("Error: {}", message),
    }
  }

  /// Check if this is a success output
  pub fn is_success(&self) -> bool {
    matches!(self, ToolOutput::Function { .. })
  }
}

/// Context for tool invocation
#[derive(Debug, Clone)]
pub struct ToolInvocation {
  /// Tool name
  pub tool_name: String,
  /// Call ID from the model
  pub call_id: String,
  /// Tool payload (arguments)
  pub payload: ToolPayload,
  /// Working directory
  pub cwd: PathBuf,
}

impl ToolInvocation {
  /// Create a new tool invocation
  pub fn new(
    tool_name: impl Into<String>,
    call_id: impl Into<String>,
    payload: ToolPayload,
    cwd: impl AsRef<Path>,
  ) -> Self {
    Self {
      tool_name: tool_name.into(),
      call_id: call_id.into(),
      payload,
      cwd: cwd.as_ref().to_path_buf(),
    }
  }
}

/// Trait for tool handlers
#[async_trait]
pub trait ToolHandler: Send + Sync {
  /// Returns the kind of tool this handler handles
  fn kind(&self) -> ToolKind;

  /// Returns true if the tool might mutate the environment
  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    false
  }

  /// Check if this handler can handle the given payload
  fn matches_kind(&self, payload: &ToolPayload) -> bool {
    matches!(
      (self.kind(), payload),
      (ToolKind::Function, ToolPayload::Function { .. }) | (ToolKind::Mcp, ToolPayload::Mcp { .. })
    )
  }

  /// Execute the tool
  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError>;
}

/// Registry for executable tools
pub struct ExecutableToolRegistry {
  handlers: HashMap<String, Box<dyn ToolHandler>>,
}

impl std::fmt::Debug for ExecutableToolRegistry {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ExecutableToolRegistry")
      .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
      .finish()
  }
}

impl ExecutableToolRegistry {
  /// Create a new empty registry
  pub fn new() -> Self {
    Self {
      handlers: HashMap::new(),
    }
  }

  /// Register a tool handler
  pub fn register(&mut self, name: impl Into<String>, handler: Box<dyn ToolHandler>) {
    let name = name.into();
    if self.handlers.insert(name.clone(), handler).is_some() {
      log::warn!("Overwriting handler for tool: {}", name);
    }
  }

  /// Get a handler by name
  pub fn get(&self, name: &str) -> Option<&dyn ToolHandler> {
    self.handlers.get(name).map(|b| b.as_ref())
  }

  /// Dispatch a tool invocation to the appropriate handler
  pub async fn dispatch(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let tool_name = &invocation.tool_name;

    let handler = self
      .handlers
      .get(tool_name)
      .ok_or_else(|| ToolError::RespondToModel(format!("Unknown tool: {}", tool_name)))?;

    if !handler.matches_kind(&invocation.payload) {
      return Err(ToolError::Fatal(format!(
        "Tool {} invoked with incompatible payload",
        tool_name
      )));
    }

    handler.handle(invocation).await
  }

  /// Check if a tool is registered
  pub fn has(&self, name: &str) -> bool {
    self.handlers.contains_key(name)
  }
}

impl Default for ExecutableToolRegistry {
  fn default() -> Self {
    Self::new()
  }
}

/// Helper function to parse JSON arguments
pub fn parse_arguments<T>(arguments: &str) -> Result<T, ToolError>
where
  T: for<'de> Deserialize<'de>,
{
  serde_json::from_str(arguments)
    .map_err(|err| ToolError::RespondToModel(format!("Failed to parse arguments: {}", err)))
}
