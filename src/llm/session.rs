//! Chat session management module (Actor pattern).
//!
//! Manages a conversation with an LLM using the actor pattern.
//! The session runs in a dedicated tokio task and communicates via channels.

use crate::config::Config;
use crate::error::{ConfigError, Result};
use crate::llm::provider::LLMProvider;
use crate::llm::providers::KimiProvider;
use crate::llm::types::{ChatConfig, Message, Role, ToolCall};
use crate::tools::{ExecutableToolRegistry, ToolInvocation, ToolPayload};
use async_openai::types::chat::ChatCompletionResponseStream;
use chrono::{Datelike, Local, Timelike};
use log::{debug, error, info};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Maximum characters to display in user input log preview
const USER_INPUT_PREVIEW_LEN: usize = 50;

/// Generate a session ID based on timestamp and current directory
/// Format: dirname-YYYY.MM.DD:HH.MM.SS.microseconds
fn generate_session_id() -> String {
  let now = Local::now();

  // Get current directory name
  let dir_name = std::env::current_dir()
    .ok()
    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
    .unwrap_or_else(|| "unknown".to_string());

  format!(
    "{}-{:04}.{:02}.{:02}:{:02}.{:02}.{:02}.{:06}",
    dir_name,
    now.year(),
    now.month(),
    now.day(),
    now.hour(),
    now.minute(),
    now.second(),
    now.timestamp_subsec_micros()
  )
}

/// Commands sent to the session actor
#[derive(Debug)]
pub enum SessionCommand {
  /// Send a user message
  SendMessage { content: String },
  /// Cancel the current streaming request
  Cancel,
  /// Clear conversation history
  ClearHistory,
  /// Shutdown the session actor
  Shutdown,
}

/// Events emitted by the chat session
#[derive(Debug, Clone)]
pub enum SessionEvent {
  /// A chunk of content received from the stream
  ContentChunk(String),
  /// A chunk of thinking/reasoning content received from the stream
  ThinkingChunk(String),
  /// A tool call was received from the model
  ToolCallReceived {
    id: String,
    name: String,
    arguments: String,
  },
  /// A tool execution completed
  ToolCallCompleted { name: String, output: String },
  /// Stream completed successfully
  Completed,
  /// Error occurred during streaming
  Error(String),
  /// Session has been shutdown
  Shutdown,
}

/// Handle to interact with a running session actor
#[derive(Debug, Clone)]
pub struct SessionHandle {
  /// Session ID
  pub id: String,
  /// Channel to send commands to the session
  cmd_tx: mpsc::UnboundedSender<SessionCommand>,
}

impl SessionHandle {
  /// Send a user message to the session
  pub fn send_message(&self, content: impl Into<String>) {
    let _ = self.cmd_tx.send(SessionCommand::SendMessage {
      content: content.into(),
    });
  }

  /// Cancel the current streaming request
  pub fn cancel(&self) {
    let _ = self.cmd_tx.send(SessionCommand::Cancel);
  }

  /// Clear conversation history
  pub fn clear_history(&self) {
    let _ = self.cmd_tx.send(SessionCommand::ClearHistory);
  }

  /// Shutdown the session actor
  pub fn shutdown(&self) {
    let _ = self.cmd_tx.send(SessionCommand::Shutdown);
  }
}

/// Internal state of the session actor
struct SessionActor {
  /// Session ID
  id: String,
  /// The LLM provider
  provider: Box<dyn LLMProvider>,
  /// Message history (including system, user, and assistant messages)
  messages: Vec<Message>,
  /// Channel to send events back to the caller
  event_tx: mpsc::UnboundedSender<SessionEvent>,
  /// Channel to receive commands
  cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
  /// Current response being accumulated
  current_response: String,
  /// Current thinking content being accumulated
  current_thinking: String,
  /// Whether a streaming request is in progress
  is_streaming: bool,
  /// Event receiver for the current stream (if any)
  stream_rx: Option<mpsc::UnboundedReceiver<SessionEvent>>,
  /// Tool call buffer for accumulating tool calls during streaming
  pending_tool_calls: Vec<ToolCall>,
  /// Executable tool registry for handling tool calls (shared)
  tool_registry: Arc<ExecutableToolRegistry>,
  /// Working directory for tool execution
  cwd: std::path::PathBuf,
}

impl SessionActor {
  fn new(
    id: String,
    provider: Box<dyn LLMProvider>,
    messages: Vec<Message>,
    event_tx: mpsc::UnboundedSender<SessionEvent>,
    cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    tool_registry: Arc<ExecutableToolRegistry>,
  ) -> Self {
    Self {
      id,
      provider,
      messages,
      event_tx,
      cmd_rx,
      current_response: String::new(),
      current_thinking: String::new(),
      is_streaming: false,
      stream_rx: None,
      pending_tool_calls: Vec::new(),
      tool_registry,
      cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    }
  }

  /// Main actor loop
  async fn run(mut self) {
    info!("SessionActor {} started", self.id);

    loop {
      tokio::select! {
        // Process incoming commands
        Some(cmd) = self.cmd_rx.recv() => {
          if !self.handle_command(cmd).await {
            break;
          }
        }

        // Process streaming events if active
        Some(event) = async {
          match &mut self.stream_rx {
            Some(rx) => rx.recv().await,
            None => std::future::pending().await,
          }
        } => {
          self.handle_stream_event(event).await;
        }

        // If no streaming and channel closed, exit
        else => {
          if self.stream_rx.is_none() {
            info!("SessionActor {}: command channel closed, exiting", self.id);
            break;
          }
        }
      }
    }

    info!("SessionActor {} stopped", self.id);
  }

  /// Handle a command from the handle
  /// Returns false if the actor should shutdown
  async fn handle_command(&mut self, cmd: SessionCommand) -> bool {
    match cmd {
      SessionCommand::SendMessage { content } => {
        // Log user input (truncated if too long)
        let preview: String = content.chars().take(USER_INPUT_PREVIEW_LEN).collect();
        let ellipsis = if content.len() > USER_INPUT_PREVIEW_LEN {
          "..."
        } else {
          ""
        };
        info!(
          "Session {}: Received user input: {}{}",
          self.id, preview, ellipsis
        );

        if self.is_streaming {
          error!("Session {}: Cannot send message while streaming", self.id);
          let _ = self.event_tx.send(SessionEvent::Error(
            "Cannot send message while another request is in progress".to_string(),
          ));
          return true;
        }

        // Add user message to history
        self.messages.push(Message::user(&content));
        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();

        // Log current message history for debugging
        info!("Session {}: Current message history:", self.id);
        for (i, msg) in self.messages.iter().enumerate() {
          let content_preview: String = msg.content.chars().take(100).collect();
          let tool_calls_info = if let Some(ref tc) = msg.tool_calls {
            format!(" [tool_calls: {}]", tc.len())
          } else {
            String::new()
          };
          let tool_call_id_info = if let Some(ref id) = msg.tool_call_id {
            format!(" [tool_call_id: {}]", id)
          } else {
            String::new()
          };
          info!(
            "  [{}] {:?}: {}{}{}",
            i, msg.role, content_preview, tool_calls_info, tool_call_id_info
          );
        }

        // Start streaming
        self.start_chat_stream().await;
        true
      }

      SessionCommand::Cancel => {
        if self.is_streaming {
          info!("Session {}: Cancelling stream", self.id);
          self.stream_rx = None;
          self.is_streaming = false;
          self.current_response.clear();
          self.current_thinking.clear();
        }
        true
      }

      SessionCommand::ClearHistory => {
        info!("Session {}: Clearing history", self.id);
        // Keep only the system message if it exists
        let system_msg = self.messages.first().and_then(|m| {
          if m.role == Role::System {
            Some(m.clone())
          } else {
            None
          }
        });

        self.messages.clear();
        if let Some(sys) = system_msg {
          self.messages.push(sys);
        }

        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();
        if self.is_streaming {
          self.stream_rx = None;
          self.is_streaming = false;
        }
        true
      }

      SessionCommand::Shutdown => {
        info!("Session {}: Shutdown requested", self.id);
        let _ = self.event_tx.send(SessionEvent::Shutdown);
        false
      }
    }
  }

  /// Start a chat stream with the current messages
  async fn start_chat_stream(&mut self) {
    match self.provider.chat_stream(self.messages.clone()).await {
      Ok(stream) => {
        let (tx, rx) = mpsc::unbounded_channel();
        self.stream_rx = Some(rx);
        self.is_streaming = true;
        tokio::spawn(handle_stream(stream, tx));
        info!("Session {}: Started streaming for message", self.id);
      }
      Err(e) => {
        error!("Session {}: Failed to start streaming: {}", self.id, e);
        let _ = self.event_tx.send(SessionEvent::Error(e.to_string()));
      }
    }
  }

  /// Handle a streaming event from the LLM
  async fn handle_stream_event(&mut self, event: SessionEvent) {
    match &event {
      SessionEvent::ContentChunk(chunk) => {
        debug!(
          "Session {}: Stream content: len={}, content={}",
          self.id,
          chunk.len(),
          &chunk[..chunk.len().min(100)]
        );
        self.current_response.push_str(chunk);
        // Forward to caller
        if let Err(_) = self.event_tx.send(event) {
          error!("Session {}: Failed to forward ContentChunk", self.id);
        }
      }
      SessionEvent::ThinkingChunk(chunk) => {
        info!(
          "Session {}: Stream thinking received: len={}, content={}",
          self.id,
          chunk.len(),
          &chunk[..chunk.len().min(100)]
        );
        self.current_thinking.push_str(chunk);
        // Forward to caller without storing in session messages
        if let Err(_) = self.event_tx.send(event) {
          error!("Session {}: Failed to forward ThinkingChunk", self.id);
        }
      }
      SessionEvent::ToolCallReceived {
        id,
        name,
        arguments,
      } => {
        info!(
          "Session {}: Tool call received: id={}, name={}, args={}",
          self.id, id, name, arguments
        );

        // Add to pending tool calls
        self
          .pending_tool_calls
          .push(ToolCall::new(id, name, arguments));

        // Forward to caller
        if let Err(_) = self.event_tx.send(event.clone()) {
          error!("Session {}: Failed to forward ToolCallReceived", self.id);
        }
      }
      SessionEvent::ToolCallCompleted { name, output } => {
        info!(
          "Session {}: Tool call completed: name={}, output_len={}",
          self.id,
          name,
          output.len()
        );
        // Forward to caller
        if let Err(_) = self.event_tx.send(event.clone()) {
          error!("Session {}: Failed to forward ToolCallCompleted", self.id);
        }
      }
      SessionEvent::Completed => {
        // Add the complete assistant message to history (with tool calls if any)
        let response = std::mem::take(&mut self.current_response);
        let thinking = std::mem::take(&mut self.current_thinking);
        let tool_calls = std::mem::take(&mut self.pending_tool_calls);

        let has_content = !response.is_empty() || !thinking.is_empty();
        let has_tool_calls = !tool_calls.is_empty();

        if has_content || has_tool_calls {
          // Build message content (include thinking if present)
          let content = if !thinking.is_empty() {
            format!("<think>{}</think>{}", thinking, response)
          } else {
            response
          };

          // Create assistant message with or without tool calls
          let assistant_msg = if has_tool_calls {
            Message::assistant_with_tools(content, tool_calls)
          } else {
            Message::assistant(content)
          };

          self.messages.push(assistant_msg);
          info!(
            "Session {}: Added assistant message, content_len={}, tool_calls={}",
            self.id,
            self.messages.last().unwrap().content.len(),
            has_tool_calls
          );
        }

        self.is_streaming = false;
        self.stream_rx = None;

        // Check if we have tool calls to execute
        if let Some(msg) = self.messages.last() {
          if msg.tool_calls.is_some() && !msg.tool_calls.as_ref().unwrap().is_empty() {
            info!("Session {}: Executing tool calls", self.id);
            self.execute_tool_calls().await;
            return; // Don't send Completed yet, we'll continue after tool execution
          }
        }

        // Forward to caller
        if let Err(_) = self.event_tx.send(event) {
          error!("Session {}: Failed to forward Completed event", self.id);
        }
        info!("Session {}: Stream completed", self.id);
      }
      SessionEvent::Error(err) => {
        error!("Session {}: Stream error: {}", self.id, err);
        self.is_streaming = false;
        self.stream_rx = None;
        self.current_response.clear();
        self.current_thinking.clear();
        self.pending_tool_calls.clear();
        // Forward to caller
        if let Err(_) = self.event_tx.send(event) {
          error!("Session {}: Failed to forward Error event", self.id);
        }
      }
      SessionEvent::Shutdown => {
        // Should not happen, but handle it
        if let Err(_) = self.event_tx.send(event) {
          error!("Session {}: Failed to forward Shutdown event", self.id);
        }
      }
    }
  }

  /// Execute pending tool calls and continue the conversation
  async fn execute_tool_calls(&mut self) {
    // Get the last assistant message with tool calls
    let tool_calls = match self.messages.last() {
      Some(msg) => msg.tool_calls.clone().unwrap_or_default(),
      None => {
        error!("Session {}: No assistant message with tool calls", self.id);
        let _ = self.event_tx.send(SessionEvent::Completed);
        return;
      }
    };

    info!(
      "Session {}: Executing {} tool calls",
      self.id,
      tool_calls.len()
    );

    // Execute each tool call
    for tool_call in &tool_calls {
      // Notify UI about tool call
      let _ = self.event_tx.send(SessionEvent::ToolCallReceived {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        arguments: tool_call.arguments.clone(),
      });

      // Create invocation
      let invocation = ToolInvocation::new(
        &tool_call.name,
        &tool_call.id,
        ToolPayload::Function {
          arguments: tool_call.arguments.clone(),
        },
        &self.cwd,
      );

      // Execute tool
      match self.tool_registry.dispatch(invocation).await {
        Ok(output) => {
          let output_str = output.into_response();

          // Notify UI about completion
          let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
            name: tool_call.name.clone(),
            output: output_str.clone(),
          });

          // Add tool result to messages
          let tool_msg = Message::tool(&output_str, &tool_call.id);
          info!(
            "Session {}: Adding tool result message: tool_call_id={}, output_preview={}...",
            self.id,
            tool_call.id,
            output_str.chars().take(100).collect::<String>()
          );
          self.messages.push(tool_msg);
          info!(
            "Session {}: Tool {} executed successfully, output_len={}",
            self.id,
            tool_call.name,
            output_str.len()
          );
        }
        Err(e) => {
          let error_msg = format!("Error: {}", e);

          // Notify UI about completion (with error)
          let _ = self.event_tx.send(SessionEvent::ToolCallCompleted {
            name: tool_call.name.clone(),
            output: error_msg.clone(),
          });

          // Add error result to messages
          let tool_msg = Message::tool(&error_msg, &tool_call.id);
          info!(
            "Session {}: Adding tool error message: tool_call_id={}, error={}",
            self.id, tool_call.id, error_msg
          );
          self.messages.push(tool_msg);
          error!("Session {}: Tool {} failed: {}", self.id, tool_call.name, e);
        }
      }
    }

    // Continue the conversation with the tool results
    info!(
      "Session {}: Continuing conversation after tool execution",
      self.id
    );

    // Log updated message history
    info!(
      "Session {}: Updated message history for next request:",
      self.id
    );
    for (i, msg) in self.messages.iter().enumerate() {
      let content_preview: String = msg.content.chars().take(100).collect();
      let tool_calls_info = if let Some(ref tc) = msg.tool_calls {
        format!(" [tool_calls: {}]", tc.len())
      } else {
        String::new()
      };
      let tool_call_id_info = if let Some(ref id) = msg.tool_call_id {
        format!(" [tool_call_id: {}]", id)
      } else {
        String::new()
      };
      info!(
        "  [{}] {:?}: {}{}{}",
        i, msg.role, content_preview, tool_calls_info, tool_call_id_info
      );
    }

    self.current_response.clear();
    self.current_thinking.clear();
    self.start_chat_stream().await;
  }
}

/// Handle to receive events from the session
pub type EventReceiver = mpsc::UnboundedReceiver<SessionEvent>;

/// ChatSession that runs as an actor
#[derive(Debug)]
pub struct ChatSession {
  /// Session handle for sending commands
  pub handle: SessionHandle,
  /// Event receiver
  pub event_rx: EventReceiver,
}

impl ChatSession {
  /// Start a new chat session with a system prompt
  ///
  /// Returns a handle to control the session and a receiver for events
  pub fn start(
    provider: Box<dyn LLMProvider>,
    system_prompt: impl Into<String>,
    tool_registry: Arc<ExecutableToolRegistry>,
  ) -> Self {
    let id = generate_session_id();
    let messages = vec![Message::system(system_prompt)];
    Self::start_with_messages(id, provider, messages, tool_registry)
  }

  /// Start a new chat session from configuration and runtime system prompt
  ///
  /// # Arguments
  /// * `config` - The application configuration
  /// * `system_prompt` - The system prompt to use
  /// * `tool_registry` - Shared tool registry for function calling (for LLM)
  /// * `executable_tool_registry` - Shared executable tool registry (for handling tool calls)
  pub fn create(
    config: &Config,
    system_prompt: impl Into<String>,
    tool_registry: Arc<crate::tools::ToolRegistry>,
    executable_tool_registry: Arc<ExecutableToolRegistry>,
  ) -> Result<Self> {
    let provider = Self::create_provider(config, tool_registry)?;
    let session = Self::start(provider, system_prompt, executable_tool_registry);
    info!("ChatSession {} created from config", session.handle.id);
    Ok(session)
  }

  /// Create LLM provider from configuration
  ///
  /// # Arguments
  /// * `config` - The application configuration
  /// * `tool_registry` - Shared tool registry for function calling
  fn create_provider(
    config: &Config,
    tool_registry: Arc<crate::tools::ToolRegistry>,
  ) -> Result<Box<dyn LLMProvider>> {
    // Get default model configuration
    let model_config = config
      .default_model_config()
      .ok_or_else(|| ConfigError::MissingDefaultModel)?;

    // Get provider configuration
    let provider =
      config
        .get_provider(&model_config.provider)
        .ok_or_else(|| ConfigError::ProviderNotFound {
          provider: model_config.provider.clone(),
          model: config.default_model.clone(),
        })?;

    // Resolve API key (may contain env var references like ${OPENAI_API_KEY})
    let api_key = provider
      .api_key
      .as_ref()
      .map(|key| config.resolve_api_key(key))
      .unwrap_or_default();

    // Create chat config
    let mut chat_config = ChatConfig::new(&model_config.model);
    if let Some(max_tokens) = model_config.max_tokens {
      chat_config = chat_config.with_max_tokens(max_tokens);
    }
    if let Some(temperature) = model_config.temperature {
      chat_config = chat_config.with_temperature(temperature);
    }
    // Set thinking mode from config
    chat_config = chat_config.with_thinking(config.default_thinking);

    // Determine if we need Coding Agent headers
    // Currently only enable for kimi-for-coding model
    let coding_agent = model_config.model == "kimi-for-coding";

    // Create provider based on type
    let provider: Box<dyn LLMProvider> = match provider.provider_type.as_str() {
      "kimi" => Box::new(KimiProvider::new(
        &provider.base_url,
        api_key,
        chat_config,
        coding_agent,
        tool_registry,
      )?),
      _ => {
        return Err(
          ConfigError::ProviderNotFound {
            provider: provider.provider_type.clone(),
            model: config.default_model.clone(),
          }
          .into(),
        );
      }
    };

    Ok(provider)
  }

  /// Start a new chat session without a system prompt
  pub fn start_without_system_prompt(
    provider: Box<dyn LLMProvider>,
    tool_registry: Arc<ExecutableToolRegistry>,
  ) -> Self {
    let id = generate_session_id();
    Self::start_with_messages(id, provider, Vec::new(), tool_registry)
  }

  /// Internal: start session with given messages
  fn start_with_messages(
    id: String,
    provider: Box<dyn LLMProvider>,
    messages: Vec<Message>,
    tool_registry: Arc<ExecutableToolRegistry>,
  ) -> Self {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    let handle = SessionHandle {
      id: id.clone(),
      cmd_tx,
    };

    let actor = SessionActor::new(id, provider, messages, event_tx, cmd_rx, tool_registry);
    tokio::spawn(actor.run());

    Self { handle, event_rx }
  }

  /// Poll for the next event from the session
  ///
  /// Returns:
  /// - `Some(SessionEvent)` - An event occurred
  /// - `None` - Session has shutdown and no more events
  pub fn poll_event(&mut self) -> Option<SessionEvent> {
    self.event_rx.try_recv().ok()
  }

  /// Check if there's an event ready without consuming it
  pub fn has_event(&self) -> bool {
    !self.event_rx.is_empty()
  }

  /// Shutdown the session
  pub fn shutdown(&self) {
    self.handle.shutdown();
  }
}

/// Handle the streaming response from the LLM
async fn handle_stream(
  mut stream: ChatCompletionResponseStream,
  tx: mpsc::UnboundedSender<SessionEvent>,
) {
  use futures::StreamExt;

  // Buffer for accumulating content across chunks (for parsing think tags)
  let mut buffer = String::new();
  let mut in_thinking_mode = false;
  let mut has_received_thinking = false;

  // Buffer for accumulating tool calls
  let mut tool_call_buffer: Vec<async_openai::types::chat::ChatCompletionMessageToolCallChunk> =
    Vec::new();

  while let Some(result) = stream.next().await {
    match result {
      Ok(response) => {
        log::debug!("recv raw response: {:?}", response);

        log::debug!(
          "Session: Received stream response: id={}, model={}, choices={}",
          response.id,
          response.model,
          response.choices.len()
        );
        for (i, choice) in response.choices.iter().enumerate() {
          log::debug!(
            "Session: Choice[{}]: delta={:?}, finish_reason={:?}",
            i,
            choice.delta,
            choice.finish_reason
          );
        }
        for choice in &response.choices {
          // Handle tool calls
          if let Some(ref tool_calls) = choice.delta.tool_calls {
            for tool_call in tool_calls {
              log::info!(
                "Session: Received tool call chunk: index={:?}, id={:?}",
                tool_call.index,
                tool_call.id
              );

              // Get the index for this tool call (u32, not Option<u32>)
              let idx = tool_call.index as usize;

              // Ensure buffer has enough slots
              while tool_call_buffer.len() <= idx {
                tool_call_buffer.push(
                  async_openai::types::chat::ChatCompletionMessageToolCallChunk {
                    index: tool_call_buffer.len() as u32,
                    id: None,
                    r#type: None,
                    function: None,
                  },
                );
              }

              // Update the tool call at this index
              let existing = &mut tool_call_buffer[idx];

              // Update ID if provided
              if let Some(ref id) = tool_call.id {
                existing.id = Some(id.clone());
              }

              // Update type if provided
              if let Some(ref call_type) = tool_call.r#type {
                existing.r#type = Some(call_type.clone());
              }

              // Update function if provided
              if let Some(ref function) = tool_call.function {
                if existing.function.is_none() {
                  existing.function = Some(async_openai::types::chat::FunctionCallStream {
                    name: None,
                    arguments: None,
                  });
                }

                if let Some(ref existing_func) = existing.function {
                  let mut updated_func = existing_func.clone();

                  if let Some(ref name) = function.name {
                    updated_func.name = Some(name.clone());
                  }
                  if let Some(ref args) = function.arguments {
                    if let Some(ref existing_args) = updated_func.arguments {
                      updated_func.arguments = Some(format!("{}{}", existing_args, args));
                    } else {
                      updated_func.arguments = Some(args.clone());
                    }
                  }

                  existing.function = Some(updated_func);
                }
              }
            }
          }

          if let Some(content) = &choice.delta.content {
            if !content.is_empty() {
              log::debug!(
                "Session: Received content chunk: len={}, content={}",
                content.len(),
                &content[..content.len().min(100)]
              );

              // Parse content for <think> tags (Kimi thinking mode)
              buffer.push_str(content);
              log::debug!(
                "Session: Buffer len={}, in_thinking_mode={}",
                buffer.len(),
                in_thinking_mode
              );

              // Process the buffer to extract thinking content
              loop {
                if in_thinking_mode {
                  // Look for </think> closing tag
                  if let Some(end_pos) = buffer.find("</think>") {
                    // Extract thinking content
                    let thinking = buffer[..end_pos].to_string();
                    if !thinking.is_empty() {
                      if !has_received_thinking {
                        log::info!(
                          "Session: First thinking content received: len={}",
                          thinking.len()
                        );
                        has_received_thinking = true;
                      }
                      log::debug!("Session: Sending ThinkingChunk: len={}", thinking.len());
                      if tx.send(SessionEvent::ThinkingChunk(thinking)).is_err() {
                        return;
                      }
                    }
                    // Remove processed part including closing tag
                    buffer = buffer[end_pos + 8..].to_string();
                    in_thinking_mode = false;
                    log::debug!(
                      "Session: Exited thinking mode, remaining buffer len={}",
                      buffer.len()
                    );
                  } else {
                    // Still in thinking mode, send what we have so far
                    if !buffer.is_empty() {
                      if !has_received_thinking {
                        log::info!(
                          "Session: First thinking content received (partial): len={}",
                          buffer.len()
                        );
                        has_received_thinking = true;
                      }
                      log::debug!(
                        "Session: Sending ThinkingChunk (partial): len={}",
                        buffer.len()
                      );
                      if tx
                        .send(SessionEvent::ThinkingChunk(buffer.clone()))
                        .is_err()
                      {
                        return;
                      }
                      buffer.clear();
                    }
                    break;
                  }
                } else {
                  // Look for <think> opening tag
                  if let Some(start_pos) = buffer.find("<think>") {
                    log::info!("Session: Found <think> tag at position {}", start_pos);
                    // Send any content before <think> as regular content
                    if start_pos > 0 {
                      let before = buffer[..start_pos].to_string();
                      if !before.is_empty() {
                        log::debug!(
                          "Session: Sending ContentChunk (before think): len={}",
                          before.len()
                        );
                        if tx.send(SessionEvent::ContentChunk(before)).is_err() {
                          return;
                        }
                      }
                    }
                    // Enter thinking mode
                    buffer = buffer[start_pos + 7..].to_string();
                    in_thinking_mode = true;
                    log::info!("Session: Entered thinking mode");
                  } else {
                    // No <think> tag, send as regular content
                    if !buffer.is_empty() {
                      log::debug!("Session: Sending ContentChunk: len={}", buffer.len());
                      if tx.send(SessionEvent::ContentChunk(buffer.clone())).is_err() {
                        return;
                      }
                      buffer.clear();
                    }
                    break;
                  }
                }
              }
            }
          }
        }
      }
      Err(e) => {
        log::error!("Session: Stream error: {}", e);
        let _ = tx.send(SessionEvent::Error(e.to_string()));
        return;
      }
    }
  }

  // Flush any remaining content in buffer
  if !buffer.is_empty() {
    if in_thinking_mode {
      log::info!(
        "Session: Flushing final thinking content: len={}",
        buffer.len()
      );
      let _ = tx.send(SessionEvent::ThinkingChunk(buffer));
    } else {
      log::debug!("Session: Flushing final content: len={}", buffer.len());
      let _ = tx.send(SessionEvent::ContentChunk(buffer));
    }
  }

  // Send any accumulated tool calls
  for tool_call in tool_call_buffer {
    if let (Some(id), Some(function)) = (tool_call.id, tool_call.function) {
      if let (Some(name), Some(arguments)) = (function.name, function.arguments) {
        if !id.is_empty() && !name.is_empty() {
          log::info!(
            "Session: Sending accumulated tool call: id={}, name={}, args={}",
            id,
            name,
            arguments
          );
          // Store the tool call info for later use
          let _ = tx.send(SessionEvent::ToolCallReceived {
            id: id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
          });
        }
      }
    }
  }

  log::info!(
    "Session: Stream completed, received_thinking={}",
    has_received_thinking
  );
  // Stream completed
  let _ = tx.send(SessionEvent::Completed);
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_session_id_format() {
    let id = generate_session_id();
    // Should contain a hyphen separating dirname and timestamp
    assert!(id.contains('-'));
    // Should contain colons for time
    assert!(id.contains(':'));
  }
}
