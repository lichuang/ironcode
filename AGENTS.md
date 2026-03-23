# AGENTS.md

## Code Style Guidelines

### 1. Type Import Rules

**DO NOT** use long path references to types directly in code:

```rust
// ❌ Wrong
pub fn to_openai_tool(&self) -> async_openai::types::chat::ChatCompletionTools {
    // ...
}

// ❌ Wrong
pub fn process_response(response: async_openai::types::chat::CreateChatCompletionResponse) {
    // ...
}
```

**MUST** use `use` to import types at the top of the file, then use short names:

```rust
// ✅ Correct
use async_openai::types::chat::ChatCompletionTools;

pub fn to_openai_tool(&self) -> ChatCompletionTools {
    // ...
}

// ✅ Correct
use async_openai::types::chat::CreateChatCompletionResponse;

pub fn process_response(response: CreateChatCompletionResponse) {
    // ...
}
```

### 2. Import Grouping Rules

Group imports in the following order:

1. Standard library (`std::`)
2. Third-party crates
3. Internal modules (`crate::`)

```rust
use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::tools::Tool;
use crate::utils::string::display_width;
```

### 3. Type Renaming

If type names conflict or are too long, use `as` to rename them:

```rust
use async_openai::types::chat::ChatCompletionTools as OpenAITool;
use async_openai::types::chat::FunctionObject as OpenAIFunction;
```

### 4. Exceptions

The following situations allow using full paths:

- When full path is needed in type definitions or documentation comments
- When macros require full paths
- When two modules have types with the same name and need to be distinguished

```rust
// Allowed: In doc comments to specify type origin
/// Converts to [`async_openai::types::chat::ChatCompletionTools`] format

// Allowed: Distinguish between same-named types
fn convert(a: crate::tools::Tool, b: async_openai::types::chat::Tool) {
    // ...
}
```

## Tool Implementation Guidelines

### Tool Architecture

Tools are implemented following a handler pattern inspired by codex-rs:

```
src/tools/
├── mod.rs           # Tool definitions, ToolHandler trait, registries
├── loader.rs        # Loading tools from Markdown files
└── handlers/        # Tool implementations
    ├── mod.rs       # Re-exports all handlers
    └── read_file.rs # ReadFile tool implementation
```

### Implementing a New Tool

1. **Define the tool in `prompts/tools/{tool_name}.md`**:

```markdown
---
name: ToolName
description: Description of what the tool does
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "param1": {
      "type": "string",
      "description": "Description of param1"
    }
  },
  "required": ["param1"]
}
```
```

2. **Create a handler in `src/tools/handlers/{tool_name}.rs`**:

```rust
//! ToolName tool handler.

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::{
    parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput,
};

/// Handler for the ToolName tool
pub struct ToolNameHandler;

/// Arguments for the ToolName tool
#[derive(Debug, Deserialize)]
struct ToolNameArgs {
    /// Parameter description
    param1: String,
}

#[async_trait]
impl ToolHandler for ToolNameHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Return true if tool modifies files/system
        false
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
        let ToolInvocation { payload, cwd, .. } = invocation;

        // Extract arguments
        let arguments = match payload {
            crate::tools::ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::RespondToModel(
                    "ToolName handler received unsupported payload".to_string(),
                ));
            }
        };

        // Parse arguments
        let args: ToolNameArgs = parse_arguments(&arguments)?;

        // Implement tool logic
        let result = do_something(&args.param1).await?;

        Ok(ToolOutput::success(result))
    }
}

impl ToolNameHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ToolNameHandler {
    fn default() -> Self {
        Self::new()
    }
}
```

3. **Export the handler in `src/tools/handlers/mod.rs`**:

```rust
pub mod tool_name;
pub use tool_name::ToolNameHandler;
```

### Tool Handler Conventions

1. **Arguments Struct**: Use serde's `Deserialize` with default functions:

```rust
#[derive(Debug, Deserialize)]
struct ToolArgs {
    required_param: String,
    #[serde(default = "default_optional")]
    optional_param: usize,
}

fn default_optional() -> usize {
    100
}
```

2. **Error Handling**: Use `ToolError` variants appropriately:
   - `ToolError::RespondToModel`: For user-facing errors (invalid args, file not found)
   - `ToolError::Fatal`: For system errors that should stop execution

3. **Path Handling**: Resolve relative paths against `invocation.cwd`:

```rust
let path = PathBuf::from(&args.path);
let resolved_path = if path.is_absolute() {
    path
} else {
    cwd.join(&path)
};
```

4. **Testing**: Include unit tests in the handler module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_handler() {
        let handler = ToolNameHandler::new();
        let invocation = ToolInvocation::new(
            "ToolName",
            "test-call-id",
            ToolPayload::Function {
                arguments: r#"{"param1": "value"}"#.to_string(),
            },
            std::env::current_dir().unwrap(),
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());
    }
}
```

### Using Tools

Register and execute tools via `ExecutableToolRegistry`:

```rust
use crate::tools::{
    ExecutableToolRegistry, ToolInvocation, ToolPayload,
    handlers::ReadFileHandler,
};

// Create registry and register handlers
let mut registry = ExecutableToolRegistry::new();
registry.register("ReadFile", Box::new(ReadFileHandler::new()));

// Create invocation context
let invocation = ToolInvocation::new(
    "ReadFile",
    "call-123",
    ToolPayload::Function {
        arguments: r#"{"path": "/path/to/file.txt"}"#.to_string(),
    },
    std::env::current_dir().unwrap(),
);

// Execute tool
match registry.dispatch(invocation).await {
    Ok(output) => println!("Result: {}", output.into_response()),
    Err(e) => eprintln!("Error: {}", e),
}
```
