//! FetchURL tool handler.
//!
//! Fetches a web page from a URL and extracts main text content from it.

use std::io::Cursor;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Handler for the FetchURL tool
pub struct FetchURLHandler;

/// Arguments for the FetchURL tool
#[derive(Debug, Deserialize)]
struct FetchURLArgs {
  /// The URL to fetch content from
  url: String,
}

#[async_trait]
impl ToolHandler for FetchURLHandler {
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
          "FetchURL handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: FetchURLArgs = parse_arguments(&arguments)?;

    // Validate URL
    let url = match url::Url::parse(&args.url) {
      Ok(u) => u,
      Err(e) => {
        return Err(ToolError::RespondToModel(format!(
          "Invalid URL '{}': {}",
          args.url, e
        )));
      }
    };

    // Fetch the URL
    let client = match reqwest::Client::builder()
      .user_agent(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36",
      )
      .build()
    {
      Ok(c) => c,
      Err(e) => {
        return Err(ToolError::Fatal(format!(
          "Failed to build HTTP client: {}",
          e
        )));
      }
    };

    let response = match client.get(url.clone()).send().await {
      Ok(r) => r,
      Err(e) => {
        return Err(ToolError::RespondToModel(format!(
          "Failed to fetch URL due to network error: {}. \
           This may indicate the URL is invalid or the server is unreachable.",
          e
        )));
      }
    };

    let status = response.status();
    if status.as_u16() >= 400 {
      return Err(ToolError::RespondToModel(format!(
        "Failed to fetch URL. Status: {}. \
         This may indicate the page is not accessible or the server is down.",
        status
      )));
    }

    // Check content type
    let content_type = response
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|v| v.to_str().ok())
      .unwrap_or("")
      .to_lowercase();

    let body_bytes = match response.bytes().await {
      Ok(b) => b,
      Err(e) => {
        return Err(ToolError::RespondToModel(format!(
          "Failed to read response body: {}",
          e
        )));
      }
    };

    if body_bytes.is_empty() {
      return Ok(ToolOutput::success("The response body is empty."));
    }

    // For plain text or markdown, return directly
    if content_type.starts_with("text/plain") || content_type.starts_with("text/markdown") {
      let text = String::from_utf8_lossy(&body_bytes);
      return Ok(ToolOutput::success(text.to_string()));
    }

    // For HTML, extract main content using readability
    let mut cursor = Cursor::new(&body_bytes);
    match readability::extractor::extract(&mut cursor, &url) {
      Ok(product) => {
        if product.text.trim().is_empty() {
          Err(ToolError::RespondToModel(
            "Failed to extract meaningful content from the page. \
             This may indicate the page content is not suitable for text extraction, \
             or the page requires JavaScript to render its content."
              .to_string(),
          ))
        } else {
          Ok(ToolOutput::success(product.text))
        }
      }
      Err(e) => Err(ToolError::RespondToModel(format!(
        "Failed to extract content from the page: {}",
        e
      ))),
    }
  }
}

impl FetchURLHandler {
  /// Create a new FetchURLHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for FetchURLHandler {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  #[test]
  fn test_parse_arguments() {
    let json = r#"{"url": "https://example.com"}"#;
    let args: FetchURLArgs = parse_arguments(json).unwrap();
    assert_eq!(args.url, "https://example.com");
  }

  #[tokio::test]
  async fn test_fetch_url_invalid_url() {
    let temp_dir = std::env::temp_dir();
    let handler = FetchURLHandler::new();

    let invocation = ToolInvocation::new(
      "FetchURL",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{"url": "not-a-valid-url"}"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Invalid URL"));
  }
}
