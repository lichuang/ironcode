//! WireMessage definitions — the protocol spoken on the wire bus.

/// Messages emitted by the session actor and consumed by the UI layer.
///
/// WireMessage replaces the direct `mpsc::UnboundedSender<SessionEvent>`
/// coupling between `SessionActor` and `App`, allowing the same core to
/// drive multiple front-ends (TUI, print mode, web UI, etc.).
#[derive(Debug, Clone)]
pub enum WireMessage {
  /// A new user/assistant turn has started.
  TurnBegin,
  /// A chunk of assistant content.
  ContentChunk {
    /// Text content received from the LLM.
    text: String,
  },
  /// A chunk of thinking/reasoning content.
  ThinkingChunk {
    /// Thinking content received from the LLM.
    text: String,
  },
  /// A tool call has started.
  ToolCallBegin {
    /// Tool call ID.
    id: String,
    /// Tool name.
    name: String,
    /// Tool arguments JSON.
    arguments: String,
  },
  /// A tool call has finished.
  ToolCallEnd {
    /// Tool call ID.
    id: String,
    /// Tool output.
    output: String,
  },
  /// A tool call requires user approval before execution.
  ApprovalRequest {
    /// Tool call ID.
    id: String,
    /// Tool name.
    name: String,
    /// Optional diff preview for file-modifying tools.
    diff_preview: Option<String>,
  },
  /// Compaction threshold crossed — context is approaching token limit.
  CompactionWarning {
    /// Current estimated token count.
    current_tokens: usize,
    /// Maximum context size for the current model.
    max_context_size: usize,
  },
  /// Compaction completed — messages were compressed.
  CompactionCompleted {
    /// Number of messages before compaction.
    before: usize,
    /// Number of messages after compaction.
    after: usize,
    /// New estimated token count.
    tokens: usize,
  },
  /// The current turn has ended.
  TurnEnd,
  /// An error occurred.
  Error {
    /// Error message.
    message: String,
  },
}
