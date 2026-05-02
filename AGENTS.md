# AGENTS.md

## Project Goal

This project is a Rust re-implementation of **kimi-cli** (https://github.com/moonshot-ai/kimi-cli). When implementing features, aim to match kimi-cli's behavior, architecture, and user experience as closely as possible. Reference the Python implementation for design decisions, feature parity, and interaction patterns. Adaptations should only be made where Rust's ownership model, type system, or ecosystem constraints require a different approach.

## Code Quality Requirements

### Formatting

All code must pass `cargo fmt`:

```bash
cargo fmt --all
```

Check formatting without modifying files:
```bash
cargo fmt --all -- --check
```

### Linting

All code must pass clippy with strict warnings:

```bash
cargo clippy --all-features -- -D warnings
```

This includes:
- No unused imports
- No dead code (unless explicitly marked with `#[allow(dead_code)]`)
- No clippy warnings of any kind

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

### 5. Path Types

Use `&Path` instead of `&PathBuf` for function parameters:

```rust
// ❌ Wrong
pub fn load_config(path: &PathBuf) -> Config { }

// ✅ Correct
use std::path::Path;
pub fn load_config(path: &Path) -> Config { }
```

### 6. Let Chains

Prefer let-chains over nested `if let`:

```rust
// ❌ Wrong
if let Some(value) = option {
    if condition {
        // ...
    }
}

// ✅ Correct
if let Some(value) = option && condition {
    // ...
}
```

### 7. Error Handling

Use `is_err()` instead of pattern matching:

```rust
// ❌ Wrong
if let Err(_) = result {
    // ...
}

// ✅ Correct
if result.is_err() {
    // ...
}
```

### 8. Dead Code

Mark intentionally unused code with `#[allow(dead_code)]`:

```rust
#[allow(dead_code)]
pub fn unused_but_keep_for_api() { }

#[allow(dead_code)]
pub struct ReservedForFutureUse { }
```

### 9. IO Errors

Use `std::io::Error::other()` for custom IO errors:

```rust
// ❌ Wrong
return Err(std::io::Error::new(
    std::io::ErrorKind::Other,
    format!("Failed: {}", e),
));

// ✅ Correct
return Err(std::io::Error::other(format!("Failed: {}", e)));
```

### 10. Redundant Closures

Use function references instead of closures:

```rust
// ❌ Wrong
items.iter().map(|e| serde_json::to_string(e))

// ✅ Correct
items.iter().map(serde_json::to_string)
```

### 11. File Open Options

Explicitly specify truncate behavior:

```rust
// ❌ Wrong
let file = OpenOptions::new()
    .create(true)
    .write(true)
    .open(&path)?;

// ✅ Correct
let file = OpenOptions::new()
    .create(true)
    .truncate(false)
    .write(true)
    .open(&path)?;
```

### 12. Unwrap Or Default

Use `unwrap_or_default()` instead of match:

```rust
// ❌ Wrong
match fs::read_to_string(&path) {
    Ok(content) => content,
    Err(_) => String::new(),
}

// ✅ Correct
fs::read_to_string(&path).unwrap_or_default()
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

Tools are loaded from `prompts/tools/*.md` at `Runtime` initialization and registered into `ExecutableToolRegistry`. Do **not** access tools through global state; pass the registry explicitly:

```rust
use crate::cli::runtime::Runtime;
use crate::tools::{ToolInvocation, ToolPayload};

// Get the pre-loaded registry from Runtime
let registry = runtime.executable_registry();

// Create invocation context
let invocation = ToolInvocation::new(
    "ReadFile",
    "call-123",
    ToolPayload::Function {
        arguments: r#"{"path": "/path/to/file.txt"}"#.to_string(),
    },
    std::env::current_dir().unwrap(),
);

// Execute tool via the registry
match registry.dispatch(invocation).await {
    Ok(output) => println!("Result: {}", output.into_response()),
    Err(e) => eprintln!("Error: {}", e),
}
```

`ToolExecutor` (used by `SessionActor`) receives `Arc<ExecutableToolRegistry>` at construction time and handles all tool call dispatching internally.


## Architecture Overview

### Session Layer (`src/llm/session/`)

`SessionActor` is a **coordinator** (not a God Object). It delegates all work to sub-components:

```
src/llm/session/
├── mod.rs          # SessionHandle, SessionCommand, SessionEvent exports
├── actor.rs        # SessionActor — tokio::select! loop + state machine coordination
├── state.rs        # ActorState enum (Idle | Streaming | ExecutingTools | WaitingApproval | WaitingAnswers)
├── context.rs      # Context — in-memory message history
├── stream.rs       # StreamManager — LLM stream + connection recovery + exponential backoff
├── tool_exec.rs    # ToolExecutor — tool calls + diff preview
├── approval.rs     # ApprovalService — approval policy (pure functions)
├── persistence.rs  # SessionPersistence — batch buffering + async flush
└── compaction.rs   # CompactionService
```

- **No global state**: `GLOBAL_CONFIG`, `GLOBAL_TOOL_REGISTRY`, and `GLOBAL_EXECUTABLE_TOOL_REGISTRY` have been removed. All state is passed explicitly.
- **`WireBus`** (`src/wire/`): `SessionActor` publishes `WireMessage` via `WirePublisher`; `App` subscribes via `WireSubscriber`. No direct `mpsc` coupling between `llm/` and `view/`.

### View Layer (`src/view/chat/`)

`ChatView` is a **container** that assembles sub-components:

```
src/view/chat/
├── mod.rs         # ChatView container + View trait implementation
├── input.rs       # InputComponent — text editing, cursor, history navigation
├── messages.rs    # MessageListComponent — ChatMessage/StreamingChunk types + rendering helpers
├── approval.rs    # ApprovalPanel — approval prompt rendering
├── questions.rs   # QuestionsPanel — structured question rendering
└── tests.rs       # ChatView unit tests
```

- `Component` trait (`src/view/mod.rs`) is the abstraction for UI sub-components.
- All rendering helpers moved out of `ChatView` into their respective component modules.

### Error Handling

`src/error.rs` is a thin boundary type. Each module owns its own error enum:
- `config::loader::Error`
- `session::store::Error`
- `tools::ToolError`

No `ConfigError`, `SessionError`, or `RuntimeError` variants in `src/error.rs`.

## Project Maintenance Rules

### TODO.md Summary Updates

Whenever you complete a batch of tasks from `TODO.md`, you **must** update the summary statistics at the bottom of that file.

Run the following commands to recalculate:

```bash
total=$(grep -cE '^\s*- \[[x ]\]' TODO.md)
undone=$(grep -cE '^\s*- \[ \]' TODO.md)
done=$(grep -cE '^\s*- \[x\]' TODO.md)
pct=$(awk "BEGIN {printf \"%.1f\", ($done/$total)*100}")
echo "Total: $total, Done: $done, Remaining: $undone, Progress: $pct%"
```

Then update the **Summary** table in `TODO.md` with the new numbers.
