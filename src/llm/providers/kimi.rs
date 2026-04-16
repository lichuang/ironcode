//! Kimi provider implementation
//!
//! Supports Kimi API with Coding Agent authentication headers.

use std::collections::hash_map::DefaultHasher;
use std::env::consts::{ARCH, OS};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_openai::error::{OpenAIError, StreamError};
use async_openai::types::chat::{
  ChatChoiceStream, ChatCompletionMessageToolCallChunk, ChatCompletionResponseStream,
  ChatCompletionStreamResponseDelta, CompletionUsage, CreateChatCompletionStreamResponse,
  FinishReason, FunctionCallStream, FunctionType, Role as OpenAIRole,
};
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::unfold;

use hostname::get;
use reqwest::Client;
use reqwest::header::HeaderMap;
use reqwest_eventsource::{Error as EventSourceError, RequestBuilderExt};
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string_pretty};
use std::result::Result as StdResult;

use crate::error::{LlmError, Result, StreamErrorCategory};
use crate::llm::provider::LLMProvider;
use crate::llm::types::{ChatConfig, Message, Role};
use crate::tools::{Tool, ToolRegistry};

/// Custom delta that includes reasoning_content for Kimi API
#[derive(Debug, Clone, Deserialize)]
struct KimiDelta {
  #[serde(default)]
  content: Option<String>,
  #[serde(default)]
  reasoning_content: Option<String>,
  #[serde(default)]
  role: Option<OpenAIRole>,
  #[serde(default)]
  tool_calls: Option<Vec<KimiToolCall>>,
}

/// Tool call from Kimi API
#[derive(Debug, Clone, Deserialize)]
struct KimiToolCall {
  id: Option<String>,
  #[serde(rename = "type")]
  call_type: Option<String>,
  function: Option<KimiToolFunction>,
  index: Option<u32>,
}

/// Tool function from Kimi API
#[derive(Debug, Clone, Deserialize)]
struct KimiToolFunction {
  name: Option<String>,
  arguments: Option<String>,
}

/// Custom choice stream for Kimi API
#[derive(Debug, Clone, Deserialize)]
struct KimiChoice {
  index: u32,
  delta: KimiDelta,
  #[serde(default)]
  finish_reason: Option<FinishReason>,
}

/// Custom stream response for Kimi API
#[derive(Debug, Clone, Deserialize)]
struct KimiStreamResponse {
  id: String,
  object: String,
  #[serde(deserialize_with = "deserialize_created")]
  created: u32,
  model: String,
  choices: Vec<KimiChoice>,
  #[serde(skip_serializing_if = "Option::is_none")]
  usage: Option<KimiUsage>,
}

/// Token usage information from Kimi API
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct KimiUsage {
  prompt_tokens: u32,
  completion_tokens: u32,
  total_tokens: u32,
  #[serde(skip_serializing_if = "Option::is_none")]
  cached_tokens: Option<u32>,
}

/// Custom deserializer for created field (handles both i64 and u32)
fn deserialize_created<'de, D>(deserializer: D) -> StdResult<u32, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let value: i64 = serde::Deserialize::deserialize(deserializer)?;
  Ok(value as u32)
}

/// Kimi CLI version for Coding Agent authentication
const KIMI_CLI_VERSION: &str = "1.16.0";
/// User-Agent header value for Coding Agent
const KIMI_USER_AGENT: &str = "KimiCLI/1.16.0";

/// Thinking configuration for Kimi API
#[derive(Debug, Clone, Serialize)]
struct ThinkingConfig {
  #[serde(rename = "type")]
  thinking_type: String,
}

/// Content item for multi-modal messages
#[derive(Debug, Clone, Serialize)]
struct ContentItem {
  #[serde(rename = "type")]
  item_type: String,
  text: String,
}

/// Chat completion request message
#[derive(Debug, Clone, Serialize)]
struct ChatMessage {
  role: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  content: Option<serde_json::Value>,
  #[serde(skip_serializing_if = "Option::is_none")]
  tool_calls: Option<Vec<RequestToolCall>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  tool_call_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  reasoning_content: Option<String>,
}

/// Tool call in request
#[derive(Debug, Clone, Serialize)]
struct RequestToolCall {
  id: String,
  #[serde(rename = "type")]
  call_type: String,
  function: RequestToolFunction,
}

/// Tool function in request
#[derive(Debug, Clone, Serialize)]
struct RequestToolFunction {
  name: String,
  arguments: String,
}

/// Tool definition for function calling
#[derive(Debug, Clone, Serialize)]
struct ToolDefinition {
  #[serde(rename = "type")]
  tool_type: String,
  function: ToolFunction,
}

#[derive(Debug, Clone, Serialize)]
struct ToolFunction {
  name: String,
  description: String,
  parameters: serde_json::Value,
}

/// Chat completion request body
#[derive(Debug, Clone, Serialize)]
struct ChatCompletionRequest {
  model: String,
  messages: Vec<ChatMessage>,
  #[serde(skip_serializing_if = "Option::is_none")]
  max_tokens: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  temperature: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  stream: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  tools: Option<Vec<ToolDefinition>>,
  /// Thinking mode configuration for Kimi API
  #[serde(skip_serializing_if = "Option::is_none")]
  thinking: Option<ThinkingConfig>,
  /// Stream options to include usage information
  #[serde(skip_serializing_if = "Option::is_none")]
  stream_options: Option<StreamOptions>,
}

/// Stream options for controlling streaming behavior
#[derive(Debug, Clone, Serialize)]
struct StreamOptions {
  /// Include usage information in the final chunk
  include_usage: bool,
}

/// Kimi provider with Coding Agent support
pub struct KimiProvider {
  http_client: reqwest::Client,
  base_url: String,
  config: ChatConfig,
  tool_registry: Arc<ToolRegistry>,
  api_key: String,
  coding_agent: bool,
}

impl std::fmt::Debug for KimiProvider {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("KimiProvider")
      .field("base_url", &self.base_url)
      .field("config", &self.config)
      .field("tool_registry", &self.tool_registry)
      .field("coding_agent", &self.coding_agent)
      .field("api_key", &"***")
      .finish()
  }
}

impl Clone for KimiProvider {
  fn clone(&self) -> Self {
    Self {
      http_client: self.http_client.clone(),
      base_url: self.base_url.clone(),
      config: self.config.clone(),
      tool_registry: self.tool_registry.clone(),
      api_key: self.api_key.clone(),
      coding_agent: self.coding_agent,
    }
  }
}

impl KimiProvider {
  /// Create a new Kimi provider
  ///
  /// # Arguments
  /// * `base_url` - Kimi API base URL (e.g., "https://api.moonshot.cn/v1")
  /// * `api_key` - API key
  /// * `config` - Chat configuration (includes enable_thinking)
  /// * `coding_agent` - Whether to use Coding Agent headers for kimi-for-coding model
  /// * `tool_registry` - Tool registry for function calling (shared with Runtime)
  pub fn new(
    base_url: impl Into<String>,
    api_key: impl Into<String>,
    config: ChatConfig,
    coding_agent: bool,
    tool_registry: Arc<ToolRegistry>,
  ) -> Result<Self> {
    let base_url = base_url.into();
    let api_key = api_key.into();

    let http_client = Self::create_http_client(&api_key, coding_agent)?;

    Ok(Self {
      http_client,
      base_url,
      config,
      tool_registry,
      api_key,
      coding_agent,
    })
  }

  /// Build the HTTP client with the given API key and Coding Agent setting.
  fn create_http_client(api_key: &str, coding_agent: bool) -> Result<reqwest::Client> {
    let mut custom_headers = HeaderMap::new();

    custom_headers.insert(
      "Authorization",
      format!("Bearer {}", api_key)
        .parse()
        .map_err(|_| LlmError::InvalidConfig("Invalid Authorization header".to_string()))?,
    );
    custom_headers.insert(
      "Content-Type",
      "application/json"
        .parse()
        .map_err(|_| LlmError::InvalidConfig("Invalid Content-Type header".to_string()))?,
    );

    if coding_agent {
      log::info!("KimiProvider: Adding Coding Agent headers");

      let version = KIMI_CLI_VERSION;
      let user_agent = KIMI_USER_AGENT;

      let device_name = get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

      let device_model = format!("{}-{}", OS, ARCH);
      let os_version = OS.to_string();
      let device_id = generate_device_id(&device_name);

      custom_headers.insert(
        "User-Agent",
        user_agent
          .parse()
          .map_err(|_| LlmError::InvalidConfig("Invalid User-Agent".to_string()))?,
      );
      custom_headers.insert(
        "X-Msh-Platform",
        "kimi_cli"
          .parse()
          .map_err(|_| LlmError::InvalidConfig("Invalid X-Msh-Platform".to_string()))?,
      );
      custom_headers.insert(
        "X-Msh-Version",
        version
          .parse()
          .map_err(|_| LlmError::InvalidConfig("Invalid X-Msh-Version".to_string()))?,
      );
      custom_headers.insert(
        "X-Msh-Device-Name",
        device_name
          .parse()
          .map_err(|_| LlmError::InvalidConfig("Invalid X-Msh-Device-Name".to_string()))?,
      );
      custom_headers.insert(
        "X-Msh-Device-Model",
        device_model
          .parse()
          .map_err(|_| LlmError::InvalidConfig("Invalid X-Msh-Device-Model".to_string()))?,
      );
      custom_headers.insert(
        "X-Msh-Os-Version",
        os_version
          .parse()
          .map_err(|_| LlmError::InvalidConfig("Invalid X-Msh-Os-Version".to_string()))?,
      );
      custom_headers.insert(
        "X-Msh-Device-Id",
        device_id
          .parse()
          .map_err(|_| LlmError::InvalidConfig("Invalid X-Msh-Device-Id".to_string()))?,
      );

      log::info!("KimiProvider: Configured custom headers:");
      for (name, value) in &custom_headers {
        if let Ok(v) = value.to_str() {
          if name.as_str().to_lowercase() == "authorization" {
            log::info!("  {}: ***masked***", name);
          } else {
            log::info!("  {}: {}", name, v);
          }
        }
      }
    } else {
      log::info!("KimiProvider: Not using Coding Agent headers");
    }

    Client::builder()
      .default_headers(custom_headers)
      .build()
      .map_err(|e| LlmError::InvalidConfig(format!("Failed to build HTTP client: {}", e)).into())
  }

  /// Convert our Message type to ChatMessage
  fn convert_message(msg: &Message) -> ChatMessage {
    let role = match msg.role {
      Role::System => "system",
      Role::User => "user",
      Role::Assistant => "assistant",
      Role::Tool => "tool",
    };

    // Convert tool_calls if present
    let tool_calls = msg.tool_calls.as_ref().map(|calls| {
      calls
        .iter()
        .map(|call| RequestToolCall {
          id: call.id.clone(),
          call_type: "function".to_string(),
          function: RequestToolFunction {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
          },
        })
        .collect()
    });

    // Build content based on message type and role
    // - Tool messages: content is an array of text items
    // - Assistant messages with tool_calls: content is null (but reasoning_content may be set)
    // - Other messages: content is string or null
    let (content, reasoning_content) = match msg.role {
      Role::Tool => {
        // Tool messages must use array format for content
        let text = if msg.content.is_empty() {
          "(Empty result)".to_string()
        } else {
          msg.content.clone()
        };
        let content_array = vec![ContentItem {
          item_type: "text".to_string(),
          text,
        }];
        (
          Some(serde_json::to_value(content_array).unwrap_or(serde_json::Value::Null)),
          None,
        )
      }
      Role::Assistant if msg.tool_calls.is_some() => {
        // Assistant messages with tool_calls:
        // - content is null (not empty string, not array)
        // - reasoning_content is extracted from <think> tags if present
        let reasoning = Self::extract_reasoning(&msg.content);
        (None, reasoning)
      }
      _ => {
        // Other messages: content is string (or null if empty)
        let content = if msg.content.is_empty() {
          None
        } else {
          Some(serde_json::Value::String(msg.content.clone()))
        };
        (content, None)
      }
    };

    ChatMessage {
      role: role.to_string(),
      content,
      tool_calls,
      tool_call_id: msg.tool_call_id.clone(),
      reasoning_content,
    }
  }

  /// Extract reasoning content from message (content between <think> tags)
  fn extract_reasoning(content: &str) -> Option<String> {
    if let Some(start) = content.find("<think>")
      && let Some(end) = content.find("</think>")
    {
      let reasoning = content[start + 7..end].trim().to_string();
      if !reasoning.is_empty() {
        return Some(reasoning);
      }
    }
    None
  }

  /// Convert tools to ToolDefinition format
  fn convert_tools(tools: &[&Tool]) -> Vec<ToolDefinition> {
    tools
      .iter()
      .map(|tool| ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunction {
          name: tool.name.clone(),
          description: tool.description.clone(),
          parameters: tool.parameters.clone(),
        },
      })
      .collect()
  }
}

#[async_trait]
impl LLMProvider for KimiProvider {
  async fn chat_stream(&self, messages: Vec<Message>) -> Result<ChatCompletionResponseStream> {
    log::info!(
      "KimiProvider: Sending chat request with {} messages",
      messages.len()
    );
    log::info!(
      "KimiProvider: Thinking mode enabled: {}",
      self.config.enable_thinking
    );

    // Convert messages
    let chat_messages: Vec<ChatMessage> = messages.iter().map(Self::convert_message).collect();

    // Build request
    let mut request = ChatCompletionRequest {
      model: self.config.model.clone(),
      messages: chat_messages,
      max_tokens: self.config.max_tokens,
      temperature: self.config.temperature,
      stream: Some(true),
      tools: None,
      thinking: None,
      stream_options: Some(StreamOptions {
        include_usage: true,
      }),
    };

    // Add thinking configuration if enabled
    if self.config.enable_thinking {
      request.thinking = Some(ThinkingConfig {
        thinking_type: "enabled".to_string(),
      });
      log::info!("KimiProvider: Added thinking config to request");
    }

    // Add tools if any
    if !self.tool_registry.is_empty() {
      let tools = self.tool_registry.all();
      request.tools = Some(Self::convert_tools(&tools));
      log::info!("KimiProvider: Added {} tools to request", tools.len());
    }

    // Build URL
    let url = format!("{}/chat/completions", self.base_url);
    log::info!("KimiProvider: Sending request to {}", url);

    // Print request details
    if let Ok(request_json) = to_string_pretty(&request) {
      log::info!("KimiProvider: Request body:\n{}", request_json);
    }

    // Send request with SSE
    let event_source = self
      .http_client
      .post(&url)
      .json(&request)
      .eventsource()
      .map_err(|_| {
        LlmError::InvalidConfig(
          "Failed to create event source: request is not cloneable".to_string(),
        )
      })?;

    // Convert EventSource to ChatCompletionResponseStream
    let stream = unfold(event_source, |mut es| async move {
      loop {
        match es.next().await {
          Some(Ok(reqwest_eventsource::Event::Open)) => {
            // Connection opened, continue
            continue;
          }
          Some(Ok(reqwest_eventsource::Event::Message(message))) => {
            log::debug!("KimiProvider: Received SSE message: {}", message.data);
            if message.data == "[DONE]" {
              // End of stream
              log::debug!("KimiProvider: Received [DONE]");
              return None;
            }
            // Parse using Kimi's custom format that includes reasoning_content
            match from_str::<KimiStreamResponse>(&message.data) {
              Ok(kimi_response) => {
                log::debug!(
                  "KimiProvider: Parsed Kimi response: id={}, model={}, choices={}",
                  kimi_response.id,
                  kimi_response.model,
                  kimi_response.choices.len()
                );
                for (i, choice) in kimi_response.choices.iter().enumerate() {
                  log::debug!(
                    "KimiProvider: Choice[{}]: content={:?}, reasoning_content={:?}, tool_calls={:?}",
                    i,
                    choice.delta.content,
                    choice.delta.reasoning_content,
                    choice.delta.tool_calls
                  );
                }
                // Convert Kimi response to standard OpenAI format
                let converted = convert_kimi_response(kimi_response);

                // Log the converted response
                if let Ok(response_json) = to_string_pretty(&converted) {
                  log::info!("KimiProvider: Converted response:\n{}", response_json);
                }

                return Some((Ok(converted), es));
              }
              Err(e) => {
                log::error!("KimiProvider: Failed to parse response: {}", e);
                return Some((Err(OpenAIError::JSONDeserialize(e, message.data)), es));
              }
            }
          }
          Some(Err(e)) => {
            log::error!("KimiProvider: Event source error: {}", e);
            let llm_err = classify_eventsource_stream_error(&e);
            return Some((
              Err(OpenAIError::StreamError(Box::new(
                StreamError::EventStream(llm_err.to_string()),
              ))),
              es,
            ));
          }
          None => {
            // Stream ended
            log::debug!("KimiProvider: Stream ended");
            return None;
          }
        }
      }
    });

    // Box the stream
    let boxed_stream: ChatCompletionResponseStream = Box::pin(stream);

    Ok(boxed_stream)
  }

  fn name(&self) -> &str {
    "kimi"
  }

  async fn on_retryable_error(&mut self, error: &LlmError) {
    log::info!(
      "KimiProvider: Rebuilding HTTP client due to retryable error: {}",
      error
    );
    match Self::create_http_client(&self.api_key, self.coding_agent) {
      Ok(client) => {
        self.http_client = client;
        log::info!("KimiProvider: HTTP client rebuilt successfully");
      }
      Err(e) => {
        log::error!("KimiProvider: Failed to rebuild HTTP client: {}", e);
      }
    }
  }
}

/// Generate a pseudo-device ID based on hostname
fn generate_device_id(hostname: &str) -> String {
  let mut hasher = DefaultHasher::new();
  hostname.hash(&mut hasher);
  format!("{:016x}", hasher.finish())
}

/// Classify a `reqwest_eventsource::Error` from the streaming phase
/// into a structured `LlmError` with precise error category.
///
/// Mirrors kimi-cli's error classification:
/// - `InvalidStatusCode` → `Stream::Http` (→ kimi-cli's `APIStatusError`)
/// - `Transport` + `is_timeout()` → `Stream::Timeout` (→ kimi-cli's `APITimeoutError`)
/// - `Transport` + `is_connect()` → `Stream::Disconnected` (→ kimi-cli's `APIConnectionError`)
/// - `Transport` other → `Stream::Transport`
/// - `Utf8` / `Parser` → `Stream::Parse` (NOT retryable)
fn classify_eventsource_stream_error(e: &EventSourceError) -> LlmError {
  match e {
    EventSourceError::InvalidStatusCode(code, _response) => LlmError::Stream {
      category: StreamErrorCategory::Http,
      status_code: Some(code.as_u16()),
      message: format!("HTTP {} during stream", code),
    },
    EventSourceError::Transport(reqwest_err) => {
      if reqwest_err.is_timeout() {
        LlmError::Stream {
          category: StreamErrorCategory::Timeout,
          status_code: None,
          message: format!("Stream timeout: {}", reqwest_err),
        }
      } else if reqwest_err.is_connect() {
        LlmError::Stream {
          category: StreamErrorCategory::Disconnected,
          status_code: None,
          message: format!("Connection lost: {}", reqwest_err),
        }
      } else {
        LlmError::Stream {
          category: StreamErrorCategory::Transport,
          status_code: None,
          message: format!("Transport error: {}", reqwest_err),
        }
      }
    }
    EventSourceError::Utf8(_) | EventSourceError::Parser(_) => LlmError::Stream {
      category: StreamErrorCategory::Parse,
      status_code: None,
      message: format!("Parse error: {}", e),
    },
    _ => LlmError::Stream {
      category: StreamErrorCategory::Transport,
      status_code: None,
      message: format!("Stream error: {}", e),
    },
  }
}

/// Convert Kimi stream response to standard OpenAI format
/// This embeds reasoning_content as special markers within content for downstream processing
fn convert_kimi_response(kimi: KimiStreamResponse) -> CreateChatCompletionStreamResponse {
  let choices = kimi
    .choices
    .into_iter()
    .map(|choice| {
      // Build content that includes reasoning_content wrapped in markers
      let content = match (choice.delta.reasoning_content, choice.delta.content) {
        (Some(ref reasoning), Some(ref content)) if !reasoning.is_empty() => {
          // Both reasoning and content present
          let combined = format!("<think>{}</think>{}", reasoning, content);
          log::debug!(
            "KimiProvider: Combined reasoning + content: len={}",
            combined.len()
          );
          Some(combined)
        }
        (Some(ref reasoning), _) if !reasoning.is_empty() => {
          // Only reasoning present
          let marked = format!("<think>{}</think>", reasoning);
          log::debug!("KimiProvider: Only reasoning: len={}", marked.len());
          Some(marked)
        }
        (_, content) => {
          if content.is_some() {
            log::debug!(
              "KimiProvider: Only content: len={}",
              content.as_ref().map(|s| s.len()).unwrap_or(0)
            );
          }
          content
        }
      };

      // Convert tool calls - use ChatCompletionMessageToolCallChunk for streaming
      let tool_calls: Option<Vec<ChatCompletionMessageToolCallChunk>> =
        choice.delta.tool_calls.map(|calls| {
          calls
            .into_iter()
            .map(|call| ChatCompletionMessageToolCallChunk {
              index: call.index.unwrap_or(0),
              id: call.id,
              r#type: call.call_type.map(|_t| FunctionType::Function),
              function: call.function.map(|f| FunctionCallStream {
                name: f.name,
                arguments: f.arguments,
              }),
            })
            .collect()
        });

      ChatChoiceStream {
        index: choice.index,
        delta: ChatCompletionStreamResponseDelta {
          content,
          role: choice.delta.role,
          refusal: None,
          tool_calls,
          #[allow(deprecated)]
          function_call: None,
        },
        finish_reason: choice.finish_reason,
        #[allow(unused)]
        logprobs: None,
      }
    })
    .collect();

  // Convert usage if present
  let usage = kimi.usage.map(|u| CompletionUsage {
    prompt_tokens: u.prompt_tokens,
    completion_tokens: u.completion_tokens,
    total_tokens: u.total_tokens,
    prompt_tokens_details: None,
    completion_tokens_details: None,
  });

  CreateChatCompletionStreamResponse {
    id: kimi.id,
    object: kimi.object,
    created: kimi.created,
    model: kimi.model,
    choices,
    usage,
    #[allow(unused)]
    // system_fingerprint is deprecated, but we keep it for compatibility
    #[allow(deprecated)]
    system_fingerprint: None,
    #[allow(unused)]
    service_tier: None,
  }
}
