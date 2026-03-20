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
