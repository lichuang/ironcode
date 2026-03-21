//! Kimi provider implementation
//!
//! Supports Kimi API with Coding Agent authentication headers.

use crate::error::{LlmError, Result};
use crate::llm::provider::LLMProvider;
use crate::llm::types::{ChatConfig, Message, Role};
use crate::tools::ToolRegistry;
use async_openai::{
  Client,
  config::OpenAIConfig,
  types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionResponseStream,
    CreateChatCompletionRequestArgs,
  },
};
use async_trait::async_trait;
use reqwest::header::HeaderMap;
use std::sync::Arc;

/// Kimi CLI version for Coding Agent authentication
const KIMI_CLI_VERSION: &str = "1.16.0";
/// User-Agent header value for Coding Agent
const KIMI_USER_AGENT: &str = "KimiCLI/1.16.0";

/// Kimi provider with Coding Agent support
#[derive(Debug, Clone)]
pub struct KimiProvider {
  client: Client<OpenAIConfig>,
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

    // Build config
    let openai_config = OpenAIConfig::new()
      .with_api_base(base_url)
      .with_api_key(api_key);

    // Build reqwest client with default headers
    let http_client = reqwest::Client::builder()
      .default_headers(custom_headers)
      .build()
      .map_err(|e| LlmError::InvalidConfig(format!("Failed to build HTTP client: {}", e)))?;

    let client = Client::with_config(openai_config).with_http_client(http_client);

    Ok(Self { client, config, tool_registry })
  }

  /// Convert our Message type to async-openai's message type
  fn convert_message(
    msg: Message,
  ) -> std::result::Result<ChatCompletionRequestMessage, async_openai::error::OpenAIError> {
    match msg.role {
      Role::System => ChatCompletionRequestSystemMessageArgs::default()
        .content(msg.content)
        .build()
        .map(Into::into),
      Role::User => ChatCompletionRequestUserMessageArgs::default()
        .content(msg.content)
        .build()
        .map(Into::into),
      Role::Assistant => ChatCompletionRequestSystemMessageArgs::default()
        .content(msg.content)
        .build()
        .map(Into::into),
    }
  }
}

#[async_trait]
impl LLMProvider for KimiProvider {
  async fn chat_stream(&self, messages: Vec<Message>) -> Result<ChatCompletionResponseStream> {
    log::info!(
      "KimiProvider: Sending chat request with {} messages",
      messages.len()
    );

    let request_messages: Vec<ChatCompletionRequestMessage> = messages
      .into_iter()
      .map(|msg| Self::convert_message(msg))
      .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut request = CreateChatCompletionRequestArgs::default();
    request
      .model(&self.config.model)
      .messages(request_messages)
      .stream(true);

    if let Some(max_tokens) = self.config.max_tokens {
      request.max_tokens(max_tokens);
    }

    if let Some(temperature) = self.config.temperature {
      request.temperature(temperature);
    }

    // Add tools if any
    if !self.tool_registry.is_empty() {
      request.tools(self.tool_registry.to_openai_tools());
    }

    let request = request
      .build()
      .map_err(|e| LlmError::BuildRequest { source: e })?;

    let stream = self.client.chat().create_stream(request).await?;

    Ok(stream)
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
