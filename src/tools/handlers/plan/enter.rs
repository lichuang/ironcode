//! EnterPlanMode tool handler.
//!
//! Lets the LLM request to enter plan mode. When executed, the session
//! switches into plan mode where only read-only tools are available.

use async_trait::async_trait;

use crate::tools::{ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, ToolPayload};

/// Handler for the EnterPlanMode tool
pub struct EnterPlanModeHandler;

#[async_trait]
impl ToolHandler for EnterPlanModeHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    // Entering plan mode changes session state
    true
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    let _arguments = match payload {
      ToolPayload::Function { arguments } => arguments,
    };

    // EnterPlanMode takes no parameters — the actual state change happens in
    // SessionActor after it sees this tool result.
    Ok(ToolOutput::success(
      "Plan mode requested. Awaiting approval to enter read-only planning phase.",
    ))
  }
}

impl EnterPlanModeHandler {
  /// Create a new EnterPlanModeHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for EnterPlanModeHandler {
  fn default() -> Self {
    Self::new()
  }
}
