//! ExitPlanMode tool handler.
//!
//! Lets the LLM submit a plan for user approval. Reads the plan file
//! written during plan mode and returns its contents along with any
//! implementation options provided by the LLM.

use std::collections::HashSet;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::{
  ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput, ToolPayload, parse_arguments,
};

/// Handler for the ExitPlanMode tool
pub struct ExitPlanModeHandler;

/// A selectable approach/option within the plan
#[derive(Debug, Deserialize)]
pub struct PlanOption {
  /// Short name for this option (1-8 words)
  pub label: String,
  /// Brief summary of this approach and its trade-offs
  #[serde(default)]
  #[allow(dead_code)]
  pub description: String,
}

/// Arguments for the ExitPlanMode tool
#[derive(Debug, Deserialize)]
pub struct ExitPlanModeArgs {
  /// When the plan contains multiple alternative approaches, list them here
  /// so the user can choose which one to execute. 2-3 options.
  #[serde(default)]
  pub options: Vec<PlanOption>,
}

/// Reserved option labels that cannot be used
const RESERVED_LABELS: &[&str] = &["reject", "revise", "approve", "reject and exit"];

#[async_trait]
impl ToolHandler for ExitPlanModeHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    // Exiting plan mode changes session state
    true
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    let arguments = match payload {
      ToolPayload::Function { arguments } => arguments,
    };

    let args: ExitPlanModeArgs = parse_arguments(&arguments)?;

    // Validate options count
    if args.options.len() > 3 {
      return Err(ToolError::RespondToModel(
        "At most 3 options are allowed".to_string(),
      ));
    }

    // Validate option labels are not reserved and unique
    let mut seen = HashSet::new();
    for opt in &args.options {
      let normalized = opt.label.trim().to_lowercase();
      if RESERVED_LABELS.contains(&normalized.as_str()) {
        return Err(ToolError::RespondToModel(format!(
          "Option label '{}' is reserved. Do not use 'Reject', 'Revise', 'Approve', or 'Reject and Exit' as option labels.",
          opt.label
        )));
      }
      if !seen.insert(normalized.clone()) {
        return Err(ToolError::RespondToModel(format!(
          "Duplicate option label: '{}'",
          opt.label
        )));
      }
    }

    // Read the plan file. We don't know the session ID here — the caller
    // (SessionActor) must inject it or we read without a specific session.
    // For now, we return a placeholder; SessionActor will enrich the result.
    Ok(ToolOutput::success(
      "ExitPlanMode called. Plan content will be read and presented for approval by the session.",
    ))
  }
}

impl ExitPlanModeHandler {
  /// Create a new ExitPlanModeHandler
  pub fn new() -> Self {
    Self
  }

  /// Build the full approval output from plan content and options.
  #[allow(dead_code)]
  pub fn build_output(plan_content: &str, options: &[PlanOption]) -> String {
    let mut output = format!("## Plan\n\n{}\n\n", plan_content.trim());

    if !options.is_empty() {
      output.push_str("## Implementation Options\n\n");
      for (i, opt) in options.iter().enumerate() {
        output.push_str(&format!(
          "{}. **{}**\n   {}\n\n",
          i + 1,
          opt.label,
          opt.description
        ));
      }
    }

    output.push_str("---\n");
    output.push_str(
      "Please choose: approve the plan, select an option, reject, or provide revision feedback.",
    );
    output
  }
}

impl Default for ExitPlanModeHandler {
  fn default() -> Self {
    Self::new()
  }
}
