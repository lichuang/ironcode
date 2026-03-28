//! Tool loader from Markdown files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use super::Tool;

/// Load all tools from a directory
pub fn load_tools_from_dir(dir: impl AsRef<Path>) -> Result<super::ToolRegistry> {
  let dir = dir.as_ref();
  let mut registry = super::ToolRegistry::new();

  if !dir.exists() {
    log::warn!("Tools directory does not exist: {}", dir.display());
    return Ok(registry);
  }

  log::info!("Loading tools from: {}", dir.display());

  for entry in WalkDir::new(dir)
    .max_depth(2)
    .into_iter()
    .filter_map(|e| e.ok())
    .filter(|e| is_tool_file(e.path()))
  {
    let path = entry.path();
    match load_tool_from_file(path) {
      Ok(tool) => {
        log::info!("Loaded tool: {} from {}", tool.name, path.display());
        registry.add(tool);
      }
      Err(e) => {
        log::error!("Failed to load tool from {}: {}", path.display(), e);
      }
    }
  }

  log::info!("Loaded {} tools total", registry.len());
  Ok(registry)
}

/// Check if a file is a tool definition file
fn is_tool_file(path: &Path) -> bool {
  if let Some(ext) = path.extension() {
    ext == "md" || ext == "markdown"
  } else {
    false
  }
}

/// Load a single tool from a Markdown file
fn load_tool_from_file(path: &Path) -> Result<Tool> {
  let content =
    fs::read_to_string(path).with_context(|| format!("Failed to read file: {}", path.display()))?;

  parse_tool_from_markdown(&content, path)
}

/// Parse tool definition from Markdown content
fn parse_tool_from_markdown(content: &str, path: &Path) -> Result<Tool> {
  // Parse frontmatter
  let (frontmatter, body) = parse_frontmatter(content)
    .with_context(|| format!("Failed to parse frontmatter in {}", path.display()))?;

  // Extract name and description from frontmatter
  let name = frontmatter.get("name").cloned().unwrap_or_else(|| {
    // Fallback to filename without extension
    path
      .file_stem()
      .and_then(|s| s.to_str())
      .unwrap_or("unnamed")
      .to_string()
  });

  let description = frontmatter.get("description").cloned().unwrap_or_default();

  // Parse no_handler flag from frontmatter
  let no_handler = frontmatter
    .get("no_handler")
    .map(|v| v == "true" || v == "yes" || v == "1")
    .unwrap_or(false);

  // Parse parameters from body (JSON block after ## Parameters)
  let parameters = parse_parameters(body).unwrap_or_else(|| default_parameters_schema());

  Ok(Tool::new_with_no_handler(name, description, parameters, no_handler))
}

/// Parse YAML frontmatter from Markdown content
/// Returns (frontmatter_map, remaining_body)
fn parse_frontmatter(content: &str) -> Result<(HashMap<String, String>, &str)> {
  let mut frontmatter = HashMap::new();

  // Check if content starts with ---
  if !content.trim_start().starts_with("---") {
    return Ok((frontmatter, content));
  }

  // Find the end of frontmatter (second ---)
  let content = content.trim_start();
  let after_first = &content[3..]; // Skip first ---

  if let Some(end_pos) = after_first.find("---") {
    let yaml_content = &after_first[..end_pos].trim();
    let body = &after_first[end_pos + 3..];

    // Parse YAML
    for line in yaml_content.lines() {
      if let Some((key, value)) = line.split_once(':') {
        let key = key.trim().to_string();
        let value = value
          .trim()
          .trim_matches('"')
          .trim_matches('\'')
          .to_string();
        frontmatter.insert(key, value);
      }
    }

    Ok((frontmatter, body))
  } else {
    // No closing ---, treat all as body
    Ok((frontmatter, content))
  }
}

/// Parse parameters JSON from Markdown body
fn parse_parameters(body: &str) -> Option<serde_json::Value> {
  // Look for ```json block after ## Parameters
  let body_lower = body.to_lowercase();

  if let Some(params_start) = body_lower.find("## parameters") {
    let after_header = &body[params_start..];

    // Find ```json or ``` block
    if let Some(code_start) = after_header.find("```json") {
      let after_fence = &after_header[code_start + 7..];
      if let Some(code_end) = after_fence.find("```") {
        let json_str = after_fence[..code_end].trim();
        return serde_json::from_str(json_str).ok();
      }
    } else if let Some(code_start) = after_header.find("```") {
      let after_fence = &after_header[code_start + 3..];
      if let Some(code_end) = after_fence.find("```") {
        let json_str = after_fence[..code_end].trim();
        return serde_json::from_str(json_str).ok();
      }
    }
  }

  None
}

/// Default empty parameters schema
fn default_parameters_schema() -> serde_json::Value {
  serde_json::json!({
      "type": "object",
      "properties": {},
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_frontmatter() {
    let content = r#"---
name: TestTool
description: A test tool
---

## Parameters

```json
{"type": "object"}
```
"#;

    let (frontmatter, body) = parse_frontmatter(content).unwrap();
    assert_eq!(frontmatter.get("name"), Some(&"TestTool".to_string()));
    assert_eq!(
      frontmatter.get("description"),
      Some(&"A test tool".to_string())
    );
    assert!(body.contains("## Parameters"));
  }

  #[test]
  fn test_parse_parameters() {
    let body = r#"
## Parameters

```json
{
  "type": "object",
  "properties": {
    "thought": {
      "type": "string",
      "description": "A thought"
    }
  },
  "required": ["thought"]
}
```
"#;

    let params = parse_parameters(body).unwrap();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["thought"].is_object());
  }

  #[test]
  fn test_parse_tool_from_markdown() {
    let markdown = r#"---
name: Think
description: Use this tool to think
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "thought": {
      "type": "string"
    }
  },
  "required": ["thought"]
}
```
"#;

    let tool = parse_tool_from_markdown(markdown, Path::new("test.md")).unwrap();
    assert_eq!(tool.name, "Think");
    assert_eq!(tool.description, "Use this tool to think");
    assert_eq!(tool.parameters["type"], "object");
    assert!(!tool.no_handler);
  }

  #[test]
  fn test_parse_tool_with_no_handler() {
    let markdown = r#"---
name: Think
description: A thinking tool
no_handler: true
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "thought": {
      "type": "string"
    }
  }
}
```
"#;

    let tool = parse_tool_from_markdown(markdown, Path::new("test.md")).unwrap();
    assert_eq!(tool.name, "Think");
    assert!(tool.no_handler, "no_handler should be true");
  }
}
