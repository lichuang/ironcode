//! SearchWeb tool handler.
//!
//! Search on the internet to get latest information.

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Handler for the SearchWeb tool
pub struct SearchWebHandler;

/// Arguments for the SearchWeb tool
#[derive(Debug, Deserialize)]
struct SearchWebArgs {
  /// The query text to search for
  query: String,
  /// The number of results to return
  #[serde(default = "default_limit")]
  limit: usize,
  /// Whether to include the content of the web pages in the results
  #[serde(default)]
  include_content: bool,
}

fn default_limit() -> usize {
  5
}

#[async_trait]
impl ToolHandler for SearchWebHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    false
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    // Extract arguments from payload
    let arguments = match payload {
      crate::tools::ToolPayload::Function { arguments } => arguments,
      _ => {
        return Err(ToolError::RespondToModel(
          "SearchWeb handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: SearchWebArgs = parse_arguments(&arguments)?;

    // Validate query
    if args.query.trim().is_empty() {
      return Err(ToolError::RespondToModel(
        "Search query cannot be empty.".to_string(),
      ));
    }

    // Validate limit
    if args.limit == 0 || args.limit > 20 {
      return Err(ToolError::RespondToModel(
        "Limit must be between 1 and 20.".to_string(),
      ));
    }

    // Perform search using DuckDuckGo
    let search = duckduckgo_search::DuckDuckGoSearch::new();
    let results = match search.search(&args.query).await {
      Ok(r) => r,
      Err(e) => {
        return Err(ToolError::RespondToModel(format!(
          "Failed to search: {}. This may indicate the search service is currently unavailable.",
          e
        )));
      }
    };

    if results.is_empty() {
      return Ok(ToolOutput::success("No search results found."));
    }

    // Limit results
    let limit = args.limit.min(results.len());
    let mut lines = Vec::new();

    for (i, result) in results.iter().take(limit).enumerate() {
      if i > 0 {
        lines.push("---".to_string());
        lines.push(String::new());
      }

      // Parse result format: ("1. text", "URL: url")
      let text = result.0.strip_prefix(&format!("{}. ", i + 1)).unwrap_or(&result.0);
      let url = result.1.strip_prefix("URL: ").unwrap_or(&result.1);

      lines.push(format!("Title: {}", text.trim()));
      lines.push(format!("URL: {}", url.trim()));

      // Optionally fetch content for each result
      if args.include_content {
        match fetch_page_content(url.trim()).await {
          Ok(content) => {
            if !content.trim().is_empty() {
              lines.push(String::new());
              lines.push(content);
            }
          }
          Err(e) => {
            lines.push(format!("\nFailed to fetch content: {}", e));
          }
        }
      }
    }

    Ok(ToolOutput::success(lines.join("\n")))
  }
}

/// Helper to fetch page content for include_content mode
async fn fetch_page_content(url: &str) -> Result<String, String> {
  let parsed_url = url::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;

  let client = reqwest::Client::builder()
    .user_agent(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
       (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36",
    )
    .build()
    .map_err(|e| format!("Failed to build client: {}", e))?;

  let response = client
    .get(parsed_url.clone())
    .send()
    .await
    .map_err(|e| format!("Network error: {}", e))?;

  if !response.status().is_success() {
    return Err(format!("HTTP {}", response.status()));
  }

  let content_type = response
    .headers()
    .get(reqwest::header::CONTENT_TYPE)
    .and_then(|v| v.to_str().ok())
    .unwrap_or("")
    .to_lowercase();

  let body_bytes = response.bytes().await.map_err(|e| format!("Read error: {}", e))?;

  if content_type.starts_with("text/plain") || content_type.starts_with("text/markdown") {
    return Ok(String::from_utf8_lossy(&body_bytes).to_string());
  }

  let mut cursor = std::io::Cursor::new(&body_bytes);
  match readability::extractor::extract(&mut cursor, &parsed_url) {
    Ok(product) => Ok(product.text),
    Err(e) => Err(format!("Extraction error: {}", e)),
  }
}

impl SearchWebHandler {
  /// Create a new SearchWebHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for SearchWebHandler {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_arguments() {
    let json = r#"{"query": "Rust programming language", "limit": 3, "include_content": false}"#;
    let args: SearchWebArgs = parse_arguments(json).unwrap();
    assert_eq!(args.query, "Rust programming language");
    assert_eq!(args.limit, 3);
    assert!(!args.include_content);
  }

  #[test]
  fn test_parse_arguments_defaults() {
    let json = r#"{"query": "Rust"}"#;
    let args: SearchWebArgs = parse_arguments(json).unwrap();
    assert_eq!(args.query, "Rust");
    assert_eq!(args.limit, 5); // default
    assert!(!args.include_content); // default
  }

  #[tokio::test]
  async fn test_search_web_empty_query() {
    let temp_dir = std::env::temp_dir();
    let handler = SearchWebHandler::new();

    let invocation = ToolInvocation::new(
      "SearchWeb",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"query": ""}"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("cannot be empty"));
  }

  #[tokio::test]
  async fn test_search_web_invalid_limit() {
    let temp_dir = std::env::temp_dir();
    let handler = SearchWebHandler::new();

    let invocation = ToolInvocation::new(
      "SearchWeb",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"query": "Rust", "limit": 25}"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Limit must be between 1 and 20"));
  }
}
