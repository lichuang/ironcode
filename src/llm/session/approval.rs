//! ApprovalService — pure policy component for tool call approval decisions.

use crate::llm::types::ToolCall;

/// Decision outcome for a tool call approval request.
pub enum ApprovalDecision {
  /// The tool call is approved for immediate execution.
  Approved,
  /// The tool call requires manual user approval.
  NeedsApproval {
    /// Tool call ID
    tool_call_id: String,
    /// Tool name
    name: String,
    /// Optional diff preview for file-modifying tools
    diff_preview: Option<String>,
  },
}

/// Pure policy component for deciding whether a tool call requires user approval.
///
/// Stateless aside from the configuration it holds (YOLO mode and auto-approve list).
pub struct ApprovalService {
  yolo: bool,
  auto_approve: Vec<String>,
}

impl ApprovalService {
  /// Create a new approval service with the given configuration.
  pub fn new(yolo: bool, auto_approve: Vec<String>) -> Self {
    Self { yolo, auto_approve }
  }

  /// Decide whether a tool call should be approved or needs manual approval.
  pub fn decide(&self, tool_call: &ToolCall, diff_preview: Option<String>) -> ApprovalDecision {
    if self.yolo || self.auto_approve.iter().any(|n| n == &tool_call.name) {
      ApprovalDecision::Approved
    } else {
      ApprovalDecision::NeedsApproval {
        tool_call_id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        diff_preview,
      }
    }
  }

  /// Returns true if YOLO mode is enabled.
  pub fn is_yolo(&self) -> bool {
    self.yolo
  }

  /// Enable or disable YOLO mode.
  pub fn set_yolo(&mut self, yolo: bool) {
    self.yolo = yolo;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn make_tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
      id: id.to_string(),
      name: name.to_string(),
      arguments: "{}".to_string(),
    }
  }

  #[test]
  fn test_yolo_approves_all() {
    let svc = ApprovalService::new(true, vec![]);
    let tc = make_tool_call("1", "WriteFile");
    match svc.decide(&tc, None) {
      ApprovalDecision::Approved => {}
      _ => panic!("Expected Approved in YOLO mode"),
    }
  }

  #[test]
  fn test_auto_approve_list() {
    let svc = ApprovalService::new(false, vec!["ReadFile".to_string()]);
    let tc = make_tool_call("1", "ReadFile");
    match svc.decide(&tc, None) {
      ApprovalDecision::Approved => {}
      _ => panic!("Expected Approved for ReadFile"),
    }
  }

  #[test]
  fn test_needs_approval() {
    let svc = ApprovalService::new(false, vec!["ReadFile".to_string()]);
    let tc = make_tool_call("1", "WriteFile");
    match svc.decide(&tc, Some("diff".to_string())) {
      ApprovalDecision::NeedsApproval {
        tool_call_id,
        name,
        diff_preview,
      } => {
        assert_eq!(tool_call_id, "1");
        assert_eq!(name, "WriteFile");
        assert_eq!(diff_preview, Some("diff".to_string()));
      }
      _ => panic!("Expected NeedsApproval"),
    }
  }

  #[test]
  fn test_set_yolo() {
    let mut svc = ApprovalService::new(false, vec![]);
    assert!(!svc.is_yolo());
    svc.set_yolo(true);
    assert!(svc.is_yolo());
  }
}
