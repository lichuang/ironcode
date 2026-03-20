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
