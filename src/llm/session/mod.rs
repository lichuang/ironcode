//! Session module.
//!
//! Re-exports public session types and delegates actor implementation.

use tokio::sync::mpsc;

pub mod actor;
pub mod approval;
pub mod compaction;
pub mod context;
pub mod persistence;
pub mod state;
pub mod stream;
pub mod tool_exec;

pub use actor::ChatSession;

#[allow(unused_imports)]
pub(crate) use actor::generate_session_id;

/// An option for a structured user question
#[derive(Debug, Clone)]
pub struct QuestionOption {
  /// Concise display text
  pub label: String,
  /// Brief explanation of trade-offs
  pub description: String,
}

/// A structured question to ask the user
#[derive(Debug, Clone)]
pub struct Question {
  /// The question text
  pub question: String,
  /// Short category tag
  pub header: String,
  /// Available options
  pub options: Vec<QuestionOption>,
  /// Whether multiple options can be selected
  pub multi_select: bool,
  /// Whether this is a yes/no confirmation dialog
  pub confirmation: bool,
  /// Default selected option indices (0-based)
  pub default: Vec<usize>,
  /// Whether the user must select at least one option
  pub required: bool,
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
  /// Approve or deny a pending tool call
  ApproveToolCall {
    /// Tool call ID being approved
    tool_call_id: String,
    /// Whether the user approved the execution
    approved: bool,
  },
  /// Answer pending structured questions
  AnswerQuestions {
    /// Tool call ID that asked the questions
    tool_call_id: String,
    /// Selected option indices for each question
    answers: Vec<Vec<usize>>,
    /// Whether the user dismissed without answering
    dismissed: bool,
  },
  /// Enable YOLO mode for the current session (persisted to meta)
  EnableSessionYolo,
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
  /// Stream was interrupted mid-way; the actor may attempt to retry
  StreamInterrupted {
    /// Error message
    error: String,
    /// Whether the actor should attempt to retry
    is_retryable: bool,
  },
  /// A tool call requires user approval before execution
  ApprovalNeeded {
    /// Tool call ID
    id: String,
    /// Tool name
    name: String,
    /// Tool arguments
    arguments: String,
    /// Optional diff preview for file-modifying tools
    diff_preview: Option<String>,
  },
  /// Session has been shutdown
  Shutdown,
  /// Compaction is needed (token limit approaching)
  CompactionNeeded {
    /// Current estimated token count
    current_tokens: usize,
    /// Token threshold that triggered this notification
    threshold: usize,
    /// Maximum context size for the current model
    max_context_size: usize,
  },
  /// Compaction completed (messages were compacted)
  CompactionCompleted {
    /// Number of messages before compaction
    message_count_before: usize,
    /// Number of messages after compaction
    message_count_after: usize,
    /// New estimated token count
    new_token_count: usize,
  },
  /// Structured questions need to be presented to the user
  QuestionsAsked {
    /// Tool call ID
    tool_call_id: String,
    /// Questions to present
    questions: Vec<Question>,
  },
  /// Token usage information from the API (sent when stream finishes)
  Usage {
    /// Total tokens in the conversation (prompt + completion)
    total_tokens: u32,
    /// Tokens in the prompt (input)
    prompt_tokens: u32,
    /// Tokens in the completion (output)
    completion_tokens: u32,
  },
}

/// Handle to interact with a running session actor
#[derive(Debug, Clone)]
pub struct SessionHandle {
  /// Session ID
  pub id: String,
  /// Channel to send commands to the session
  cmd_tx: mpsc::UnboundedSender<SessionCommand>,
}

#[allow(dead_code)]
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

  /// Approve or deny a pending tool call
  pub fn approve_tool_call(&self, tool_call_id: impl Into<String>, approved: bool) {
    let _ = self.cmd_tx.send(SessionCommand::ApproveToolCall {
      tool_call_id: tool_call_id.into(),
      approved,
    });
  }

  /// Enable YOLO mode for the current session
  pub fn enable_session_yolo(&self) {
    let _ = self.cmd_tx.send(SessionCommand::EnableSessionYolo);
  }

  /// Send answers to pending structured questions
  pub fn answer_questions(
    &self,
    tool_call_id: impl Into<String>,
    answers: Vec<Vec<usize>>,
    dismissed: bool,
  ) {
    let _ = self.cmd_tx.send(SessionCommand::AnswerQuestions {
      tool_call_id: tool_call_id.into(),
      answers,
      dismissed,
    });
  }
}

#[cfg(test)]
impl SessionHandle {
  /// Create a test session handle with a given command sender.
  pub fn test_new(id: String, cmd_tx: mpsc::UnboundedSender<SessionCommand>) -> Self {
    Self { id, cmd_tx }
  }
}
