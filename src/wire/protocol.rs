//! WireMessage definitions — the protocol spoken on the wire bus.

use serde_json::Value;

use crate::llm::Question;

/// Messages emitted by the session actor and consumed by the UI layer.
///
/// WireMessage replaces the direct `mpsc::UnboundedSender<SessionEvent>`
/// coupling between `SessionActor` and `App`, allowing the same core to
/// drive multiple front-ends (TUI, print mode, web UI, etc.).
#[derive(Debug, Clone)]
#[allow(dead_code)]
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
    /// Tool name.
    name: String,
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
    /// Position of this tool in the overall execution list (1-based).
    position: usize,
    /// Total number of tools in the current execution batch.
    total: usize,
  },
  /// Structured questions need to be presented to the user.
  QuestionsAsked {
    /// Tool call ID.
    tool_call_id: String,
    /// Questions to present.
    questions: Vec<Question>,
  },
  /// Compaction threshold crossed — context is approaching token limit.
  CompactionWarning {
    /// Current estimated token count.
    current_tokens: usize,
    /// Token threshold that triggered the warning.
    threshold: usize,
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
  /// Token usage information from the API.
  Usage {
    /// Total tokens in the conversation.
    total_tokens: u32,
    /// Tokens in the prompt.
    prompt_tokens: u32,
    /// Tokens in the completion.
    completion_tokens: u32,
  },
  /// The current turn has ended.
  TurnEnd,
  /// Plan mode state has changed.
  PlanModeChanged {
    /// Whether plan mode is now active.
    active: bool,
  },
  /// A plan's content should be displayed inline in the chat.
  PlanDisplay {
    /// The full markdown content of the plan.
    content: String,
    /// The path to the plan file for reference.
    file_path: String,
  },
  /// A client-side hook request is waiting for a response.
  HookRequest {
    /// Request identifier.
    id: String,
    /// Event that triggered the hook.
    event: String,
    /// Matcher value (target) the hook was triggered against.
    target: String,
    /// Input payload sent to the client.
    input_data: Value,
  },
  /// A response to a client-side hook request.
  HookResponse {
    /// Request identifier matching the `HookRequest`.
    id: String,
    /// Client decision: `"allow"` or `"block"`.
    action: String,
    /// Optional reason when blocked.
    reason: String,
  },
  /// An error occurred.
  Error {
    /// Error message.
    message: String,
  },
}
