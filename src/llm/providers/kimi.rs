//! Kimi provider implementation
//!
//! Supports Kimi API with Coding Agent authentication headers.

use crate::error::{LlmError, Result};
use crate::llm::provider::LLMProvider;
use crate::llm::types::{ChatConfig, Message, Role};
use crate::tools::ToolRegistry;
use async_openai::error::OpenAIError;
use async_openai::types::chat::{ChatCompletionResponseStream, FinishReason};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::HeaderMap;
use reqwest_eventsource::RequestBuilderExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Custom delta that includes reasoning_content for Kimi API
#[derive(Debug, Clone, Deserialize)]
struct KimiDelta {
  #[serde(default)]
  content: Option<String>,
  #[serde(default)]
  reasoning_content: Option<String>,
  #[serde(default)]
  role: Option<async_openai::types::chat::Role>,
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
}

/// Custom deserializer for created field (handles both i64 and u32)
fn deserialize_created<'de, D>(deserializer: D) -> std::result::Result<u32, D::Error>
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

/// Chat completion request message
#[derive(Debug, Clone, Serialize)]
struct ChatMessage {
  role: String,
  content: String,
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
}

/// Kimi provider with Coding Agent support
#[derive(Debug, Clone)]
pub struct KimiProvider {
  http_client: reqwest::Client,
  base_url: String,
  config: ChatConfig,
  tool_registry: Arc<ToolRegistry>,
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

    // Build Coding Agent headers
    let mut custom_headers = HeaderMap::new();

    // Add Authorization header
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

      // Use constants for Coding Agent authentication
      let version = KIMI_CLI_VERSION;
      let user_agent = KIMI_USER_AGENT;

      // Device name (hostname)
      let device_name = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

      // Device model (OS + ARCH)
      let device_model = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);

      // OS version
      let os_version = std::env::consts::OS.to_string();

      // Device ID (hashed hostname)
      let device_id = generate_device_id(&device_name);

      // Add all headers to the map
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

      // Log all configured headers
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

    // Build reqwest client with default headers
    let http_client = reqwest::Client::builder()
      .default_headers(custom_headers)
      .build()
      .map_err(|e| LlmError::InvalidConfig(format!("Failed to build HTTP client: {}", e)))?;

    Ok(Self {
      http_client,
      base_url,
      config,
      tool_registry,
    })
  }

  /// Convert our Message type to ChatMessage
  fn convert_message(msg: Message) -> ChatMessage {
    let role = match msg.role {
      Role::System => "system",
      Role::User => "user",
      Role::Assistant => "assistant",
    };
    ChatMessage {
      role: role.to_string(),
      content: msg.content,
    }
  }

  /// Convert tools to ToolDefinition format
  fn convert_tools(tools: &[&crate::tools::Tool]) -> Vec<ToolDefinition> {
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
    let chat_messages: Vec<ChatMessage> = messages.into_iter().map(Self::convert_message).collect();

    // Build request
    let mut request = ChatCompletionRequest {
      model: self.config.model.clone(),
      messages: chat_messages,
      max_tokens: self.config.max_tokens,
      temperature: self.config.temperature,
      stream: Some(true),
      tools: None,
      thinking: None,
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

    // Send request with SSE
    let event_source = self
      .http_client
      .post(&url)
      .json(&request)
      .eventsource()
      .map_err(|e| LlmError::StreamError(format!("Failed to create event source: {}", e)))?;

    // Convert EventSource to ChatCompletionResponseStream
    let stream = futures::stream::unfold(event_source, |mut es| async move {
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
            match serde_json::from_str::<KimiStreamResponse>(&message.data) {
              Ok(kimi_response) => {
                log::debug!("KimiProvider: Parsed Kimi response: id={}, model={}, choices={}", 
                  kimi_response.id, kimi_response.model, kimi_response.choices.len());
                for (i, choice) in kimi_response.choices.iter().enumerate() {
                  log::debug!("KimiProvider: Choice[{}]: content={:?}, reasoning_content={:?}",
                    i, choice.delta.content, choice.delta.reasoning_content);
                }
                // Convert Kimi response to standard OpenAI format
                let converted = convert_kimi_response(kimi_response);
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
            return Some((
              Err(OpenAIError::StreamError(Box::new(
                async_openai::error::StreamError::EventStream(format!("Event source error: {}", e)),
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
}

/// Generate a pseudo-device ID based on hostname
fn generate_device_id(hostname: &str) -> String {
  use std::collections::hash_map::DefaultHasher;
  use std::hash::{Hash, Hasher};

  let mut hasher = DefaultHasher::new();
  hostname.hash(&mut hasher);
  format!("{:016x}", hasher.finish())
}

/// Convert Kimi stream response to standard OpenAI format
/// This embeds reasoning_content as special markers within content for downstream processing
fn convert_kimi_response(kimi: KimiStreamResponse) -> async_openai::types::chat::CreateChatCompletionStreamResponse {
  use async_openai::types::chat::{ChatChoiceStream, ChatCompletionStreamResponseDelta};

  let choices = kimi
    .choices
    .into_iter()
    .map(|choice| {
      // Build content that includes reasoning_content wrapped in markers
      let content = match (choice.delta.reasoning_content, choice.delta.content) {
        (Some(ref reasoning), Some(ref content)) if !reasoning.is_empty() => {
          // Both reasoning and content present
          let combined = format!("<think>{}</think>{}", reasoning, content);
          log::debug!("KimiProvider: Combined reasoning + content: len={}", combined.len());
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
            log::debug!("KimiProvider: Only content: len={}", content.as_ref().map(|s| s.len()).unwrap_or(0));
          }
          content
        }
      };

      ChatChoiceStream {
        index: choice.index,
        delta: ChatCompletionStreamResponseDelta {
          content,
          role: choice.delta.role,
          refusal: None,
          tool_calls: None,
          #[allow(deprecated)]
          function_call: None,
        },
        finish_reason: choice.finish_reason,
        #[allow(unused)]
        logprobs: None,
      }
    })
    .collect();

  async_openai::types::chat::CreateChatCompletionStreamResponse {
    id: kimi.id,
    object: kimi.object,
    created: kimi.created,
    model: kimi.model,
    choices,
    #[allow(unused)]
    usage: None,
    #[allow(unused)]
    system_fingerprint: None,
    #[allow(unused)]
    service_tier: None,
  }
}
