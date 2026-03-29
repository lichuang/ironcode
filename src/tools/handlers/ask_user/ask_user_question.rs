//! AskUserQuestion tool handler.
//!
//! Ask the user questions with structured options during execution.

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::{parse_arguments, ToolError, ToolHandler, ToolInvocation, ToolKind, ToolOutput};

/// Handler for the AskUserQuestion tool
pub struct AskUserQuestionHandler;

/// A question option
#[derive(Debug, Deserialize)]
struct QuestionOption {
  /// Concise display text (1-5 words)
  label: String,
  /// Brief explanation of trade-offs or implications
  #[serde(default)]
  description: String,
}

/// A question to ask the user
#[derive(Debug, Deserialize)]
struct Question {
  /// The question text
  question: String,
  /// Short category tag (max 12 chars)
  #[serde(default)]
  header: String,
  /// Available options (2-4 items)
  options: Vec<QuestionOption>,
  /// Whether multiple options can be selected
  #[serde(default)]
  multi_select: bool,
}

/// Arguments for the AskUserQuestion tool
#[derive(Debug, Deserialize)]
struct AskUserQuestionArgs {
  /// The questions to ask (1-4 questions)
  questions: Vec<Question>,
}

#[async_trait]
impl ToolHandler for AskUserQuestionHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    // This tool doesn't modify files/system, it just asks questions
    false
  }

  async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
    let ToolInvocation { payload, .. } = invocation;

    // Extract arguments from payload
    let arguments = match payload {
      crate::tools::ToolPayload::Function { arguments } => arguments,
      _ => {
        return Err(ToolError::RespondToModel(
          "AskUserQuestion handler received unsupported payload".to_string(),
        ));
      }
    };

    // Parse arguments
    let args: AskUserQuestionArgs = parse_arguments(&arguments)?;

    // Validate questions count
    if args.questions.is_empty() {
      return Err(ToolError::RespondToModel(
        "At least one question is required.".to_string(),
      ));
    }

    if args.questions.len() > 4 {
      return Err(ToolError::RespondToModel(
        "Maximum 4 questions allowed per call.".to_string(),
      ));
    }

    // Validate each question
    for (idx, question) in args.questions.iter().enumerate() {
      if question.question.trim().is_empty() {
        return Err(ToolError::RespondToModel(format!(
          "Question {}: question text cannot be empty.",
          idx + 1
        )));
      }

      if question.options.len() < 2 {
        return Err(ToolError::RespondToModel(format!(
          "Question {}: at least 2 options are required.",
          idx + 1
        )));
      }

      if question.options.len() > 4 {
        return Err(ToolError::RespondToModel(format!(
          "Question {}: maximum 4 options allowed.",
          idx + 1
        )));
      }

      for (opt_idx, option) in question.options.iter().enumerate() {
        if option.label.trim().is_empty() {
          return Err(ToolError::RespondToModel(format!(
            "Question {}: option {} label cannot be empty.",
            idx + 1,
            opt_idx + 1
          )));
        }
      }
    }

    // For now, interactive questions are not supported in the TUI
    // Return an error instructing the model to ask directly in text
    Err(ToolError::RespondToModel(
      "Interactive questions are not supported. Please ask the user directly in your text response instead.".to_string()
    ))

    // TODO: When TUI supports interactive questions:
    // 1. Send questions to UI layer via message broker
    // 2. Wait for user response (async channel)
    // 3. Return the selected answers as JSON
  }
}

impl AskUserQuestionHandler {
  /// Create a new AskUserQuestionHandler
  pub fn new() -> Self {
    Self
  }
}

impl Default for AskUserQuestionHandler {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  #[test]
  fn test_parse_arguments() {
    let json = r#"{
      "questions": [
        {
          "question": "What would you like to do?",
          "header": "Action",
          "options": [
            {"label": "Option A", "description": "First option"},
            {"label": "Option B"}
          ],
          "multi_select": false
        }
      ]
    }"#;
    let args: AskUserQuestionArgs = parse_arguments(json).unwrap();

    assert_eq!(args.questions.len(), 1);
    assert_eq!(args.questions[0].question, "What would you like to do?");
    assert_eq!(args.questions[0].header, "Action");
    assert_eq!(args.questions[0].options.len(), 2);
    assert_eq!(args.questions[0].options[0].label, "Option A");
    assert_eq!(args.questions[0].options[0].description, "First option");
    assert!(!args.questions[0].multi_select);
  }

  #[test]
  fn test_parse_arguments_defaults() {
    let json = r#"{
      "questions": [
        {
          "question": "Simple question?",
          "options": [
            {"label": "Yes"},
            {"label": "No"}
          ]
        }
      ]
    }"#;
    let args: AskUserQuestionArgs = parse_arguments(json).unwrap();

    assert_eq!(args.questions[0].header, ""); // default
    assert!(!args.questions[0].multi_select); // default
    assert_eq!(args.questions[0].options[0].description, ""); // default
  }

  #[test]
  fn test_parse_arguments_multiple_questions() {
    let json = r#"{
      "questions": [
        {"question": "Q1?", "options": [{"label": "A1"}, {"label": "A2"}]},
        {"question": "Q2?", "options": [{"label": "B1"}, {"label": "B2"}]}
      ]
    }"#;
    let args: AskUserQuestionArgs = parse_arguments(json).unwrap();

    assert_eq!(args.questions.len(), 2);
    assert_eq!(args.questions[0].question, "Q1?");
    assert_eq!(args.questions[1].question, "Q2?");
  }

  #[tokio::test]
  async fn test_ask_user_handler_validation() {
    let temp_dir = std::env::temp_dir();
    let handler = AskUserQuestionHandler::new();

    // Valid question but handler returns error (interactive not supported)
    let invocation = ToolInvocation::new(
      "AskUserQuestion",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{
          "questions": [
            {
              "question": "What to do?",
              "options": [{"label": "Option 1"}, {"label": "Option 2"}]
            }
          ]
        }"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("not supported"));
  }

  #[tokio::test]
  async fn test_ask_user_handler_empty_question() {
    let temp_dir = std::env::temp_dir();
    let handler = AskUserQuestionHandler::new();

    let invocation = ToolInvocation::new(
      "AskUserQuestion",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{
          "questions": [
            {
              "question": "",
              "options": [{"label": "Option 1"}, {"label": "Option 2"}]
            }
          ]
        }"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("cannot be empty"));
  }

  #[tokio::test]
  async fn test_ask_user_handler_too_few_options() {
    let temp_dir = std::env::temp_dir();
    let handler = AskUserQuestionHandler::new();

    let invocation = ToolInvocation::new(
      "AskUserQuestion",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{
          "questions": [
            {
              "question": "What to do?",
              "options": [{"label": "Only option"}]
            }
          ]
        }"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("at least 2 options"));
  }

  #[tokio::test]
  async fn test_ask_user_handler_too_many_questions() {
    let temp_dir = std::env::temp_dir();
    let handler = AskUserQuestionHandler::new();

    let invocation = ToolInvocation::new(
      "AskUserQuestion",
      "test-call-id",
      crate::tools::ToolPayload::Function {
        arguments: r#"{
          "questions": [
            {"question": "Q1?", "options": [{"label": "A"}, {"label": "B"}]},
            {"question": "Q2?", "options": [{"label": "A"}, {"label": "B"}]},
            {"question": "Q3?", "options": [{"label": "A"}, {"label": "B"}]},
            {"question": "Q4?", "options": [{"label": "A"}, {"label": "B"}]},
            {"question": "Q5?", "options": [{"label": "A"}, {"label": "B"}]}
          ]
        }"#.to_string(),
      },
      &temp_dir,
    );

    let result = handler.handle(invocation).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Maximum 4 questions"));
  }
}
