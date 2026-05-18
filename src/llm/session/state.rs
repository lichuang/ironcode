//! ActorState — explicit state machine for SessionActor.
//!
//! Replaces the implicit state combination of `is_streaming`,
//! `tool_call_execution_state`, and `pending_answers_state` with a single
//! enum that the compiler can exhaustively match on.

use crate::llm::types::ToolCall;

/// Explicit state of the session actor.
///
/// The actor can only be in one state at a time, and state transitions
/// are managed centrally rather than through scattered boolean/option flags.
pub enum ActorState {
  /// No request is in progress.
  Idle,
  /// Receiving a streaming response from the LLM.
  Streaming,
  /// Executing a batch of tool calls.
  ExecutingTools {
    /// The tool calls to execute.
    tool_calls: Vec<ToolCall>,
    /// Index of the next tool call to execute.
    current_index: usize,
  },
  /// Paused waiting for user approval of a tool call.
  WaitingApproval {
    /// The tool calls being executed.
    tool_calls: Vec<ToolCall>,
    /// Index of the tool call pending approval.
    current_index: usize,
    /// Indices of tools already approved in this queue.
    approved_indices: Vec<usize>,
    /// Indices of tools already rejected in this queue.
    rejected_indices: Vec<usize>,
  },
  /// Paused waiting for user answers to AskUserQuestion.
  WaitingAnswers {
    /// The tool calls being executed.
    tool_calls: Vec<ToolCall>,
    /// Index of the AskUserQuestion tool call.
    current_index: usize,
    /// Tool call ID of the question.
    tool_call_id: String,
  },
  /// Paused waiting for user confirmation to enter plan mode.
  WaitingEnterPlanMode {
    /// The tool calls being executed.
    tool_calls: Vec<ToolCall>,
    /// Index of the EnterPlanMode tool call.
    current_index: usize,
    /// Tool call ID.
    tool_call_id: String,
  },
  /// Paused waiting for user approval of a plan.
  WaitingExitPlanMode {
    /// The tool calls being executed.
    tool_calls: Vec<ToolCall>,
    /// Index of the ExitPlanMode tool call.
    current_index: usize,
    /// Tool call ID.
    tool_call_id: String,
    /// Whether the plan had custom options.
    has_options: bool,
    /// Labels of custom options (for mapping answers).
    option_labels: Vec<String>,
    /// All option labels in order (including built-in ones).
    all_option_labels: Vec<String>,
  },
}
