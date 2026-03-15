//! Chat session management module (Actor pattern).
//!
//! Manages a conversation with an LLM using the actor pattern.
//! The session runs in a dedicated tokio task and communicates via channels.

use crate::config::Config;
use crate::error::{ConfigError, Result};
use crate::llm::openai::OpenAIClient;
use crate::llm::types::{ChatConfig, Message, Role};
use async_openai::types::chat::ChatCompletionResponseStream;
use chrono::{Datelike, Local, Timelike};
use log::{error, info};
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
  /// The LLM client
  client: OpenAIClient,
  /// Message history (including system, user, and assistant messages)
  messages: Vec<Message>,
  /// Channel to send events back to the caller
  event_tx: mpsc::UnboundedSender<SessionEvent>,
  /// Channel to receive commands
  cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
  /// Current response being accumulated
  current_response: String,
  /// Whether a streaming request is in progress
  is_streaming: bool,
  /// Event receiver for the current stream (if any)
  stream_rx: Option<mpsc::UnboundedReceiver<SessionEvent>>,
}

impl SessionActor {
  fn new(
    id: String,
    client: OpenAIClient,
    messages: Vec<Message>,
    event_tx: mpsc::UnboundedSender<SessionEvent>,
    cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
  ) -> Self {
    Self {
      id,
      client,
      messages,
      event_tx,
      cmd_rx,
      current_response: String::new(),
      is_streaming: false,
      stream_rx: None,
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
        let ellipsis = if content.len() > USER_INPUT_PREVIEW_LEN { "..." } else { "" };
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

        // Start streaming
        match self.client.chat_stream(self.messages.clone()).await {
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
        true
      }

      SessionCommand::Cancel => {
        if self.is_streaming {
          info!("Session {}: Cancelling stream", self.id);
          self.stream_rx = None;
          self.is_streaming = false;
          self.current_response.clear();
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

  /// Handle a streaming event from the LLM
  async fn handle_stream_event(&mut self, event: SessionEvent) {
    match &event {
      SessionEvent::ContentChunk(chunk) => {
        self.current_response.push_str(chunk);
        // Forward to caller
        let _ = self.event_tx.send(event);
      }
      SessionEvent::Completed => {
        // Add the complete assistant message to history
        let response = std::mem::take(&mut self.current_response);
        if !response.is_empty() {
          self.messages.push(Message::assistant(response));
        }
        self.is_streaming = false;
        self.stream_rx = None;
        // Forward to caller
        let _ = self.event_tx.send(event);
        info!("Session {}: Stream completed", self.id);
      }
      SessionEvent::Error(err) => {
        error!("Session {}: Stream error: {}", self.id, err);
        self.is_streaming = false;
        self.stream_rx = None;
        self.current_response.clear();
        // Forward to caller
        let _ = self.event_tx.send(event);
      }
      SessionEvent::Shutdown => {
        // Should not happen, but handle it
        let _ = self.event_tx.send(event);
      }
    }
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
  pub fn start(client: OpenAIClient, system_prompt: impl Into<String>) -> Self {
    let id = generate_session_id();
    let messages = vec![Message::system(system_prompt)];
    Self::start_with_messages(id, client, messages)
  }

  /// Start a new chat session from configuration and runtime system prompt
  pub fn from_config(config: &Config, system_prompt: impl Into<String>) -> Result<Self> {
    let client = Self::create_llm_client(config)?;
    let session = Self::start(client, system_prompt);
    info!("ChatSession {} created from config", session.handle.id);
    Ok(session)
  }

  /// Create LLM client from configuration
  fn create_llm_client(config: &Config) -> Result<OpenAIClient> {
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

    // Create client config
    let mut client_config = ChatConfig::new(&model_config.model);
    if let Some(max_tokens) = model_config.max_tokens {
      client_config = client_config.with_max_tokens(max_tokens);
    }
    if let Some(temperature) = model_config.temperature {
      client_config = client_config.with_temperature(temperature);
    }

    // Create client with appropriate configuration
    let client = if api_key.is_empty() {
      // No API key (e.g., local Ollama)
      OpenAIClient::with_base_url(&provider.base_url, "", client_config)
    } else {
      OpenAIClient::with_base_url(&provider.base_url, api_key, client_config)
    };

    Ok(client)
  }

  /// Start a new chat session without a system prompt
  pub fn start_without_system_prompt(client: OpenAIClient) -> Self {
    let id = generate_session_id();
    Self::start_with_messages(id, client, Vec::new())
  }

  /// Internal: start session with given messages
  fn start_with_messages(id: String, client: OpenAIClient, messages: Vec<Message>) -> Self {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    let handle = SessionHandle {
      id: id.clone(),
      cmd_tx,
    };

    let actor = SessionActor::new(id, client, messages, event_tx, cmd_rx);
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

  while let Some(result) = stream.next().await {
    match result {
      Ok(response) => {
        for choice in &response.choices {
          if let Some(content) = &choice.delta.content {
            if !content.is_empty() {
              if tx
                .send(SessionEvent::ContentChunk(content.clone()))
                .is_err()
              {
                // Receiver dropped, stop streaming
                return;
              }
            }
          }
        }
      }
      Err(e) => {
        let _ = tx.send(SessionEvent::Error(e.to_string()));
        return;
      }
    }
  }

  // Stream completed
  let _ = tx.send(SessionEvent::Completed);
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::llm::{ChatConfig, OpenAIClient};

  #[test]
  fn test_session_id_format() {
    let id = generate_session_id();
    // Should contain a hyphen separating dirname and timestamp
    assert!(id.contains('-'));
    // Should contain colons for time
    assert!(id.contains(':'));
  }

  #[tokio::test]
  async fn test_session_start_and_shutdown() {
    let client = OpenAIClient::new(ChatConfig::default()).expect("Failed to create client");
    let mut session = ChatSession::start(client, "You are a helpful assistant.");

    // Shutdown the session
    session.shutdown();

    // Wait a bit for shutdown
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Should receive shutdown event
    let event = session.poll_event();
    assert!(matches!(event, Some(SessionEvent::Shutdown)));
  }

  #[tokio::test]
  async fn test_clear_history() {
    let client = OpenAIClient::new(ChatConfig::default()).expect("Failed to create client");
    let mut session = ChatSession::start(client, "You are a helpful assistant.");

    // Clear history
    session.handle.clear_history();

    // No immediate event for clear_history, just shouldn't panic
    session.shutdown();
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
  }
}
