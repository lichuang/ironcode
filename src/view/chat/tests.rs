use crossterm::event::{KeyCode, KeyEvent};
use tokio::sync::mpsc;

use crate::cli::app::PendingQuestions;
use crate::cli::runtime::Runtime;
use crate::config::Config;
use crate::llm::Question;
use crate::llm::session::SessionCommand;

use super::*;

fn make_session_handle() -> (SessionHandle, mpsc::UnboundedReceiver<SessionCommand>) {
  let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
  (
    SessionHandle::test_new("test-session".to_string(), cmd_tx),
    cmd_rx,
  )
}

fn make_pending_questions() -> PendingQuestions {
  PendingQuestions {
    tool_call_id: "call-123".to_string(),
    questions: vec![
      Question {
        question: "Which color?".to_string(),
        header: "Style".to_string(),
        options: vec![
          crate::llm::session::QuestionOption {
            label: "Red".to_string(),
            description: "Bold".to_string(),
          },
          crate::llm::session::QuestionOption {
            label: "Blue".to_string(),
            description: "Calm".to_string(),
          },
        ],
        multi_select: false,
        confirmation: false,
        default: Vec::new(),
        required: false,
      },
      Question {
        question: "Pick sizes".to_string(),
        header: "".to_string(),
        options: vec![
          crate::llm::session::QuestionOption {
            label: "Small".to_string(),
            description: "".to_string(),
          },
          crate::llm::session::QuestionOption {
            label: "Large".to_string(),
            description: "".to_string(),
          },
        ],
        multi_select: true,
        confirmation: false,
        default: Vec::new(),
        required: false,
      },
    ],
    current_question_idx: 0,
    answers: Vec::new(),
    selected_option_idx: 0,
  }
}

#[test]
fn test_question_keyboard_down_navigation() {
  let (session_handle, mut cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_pending_questions());

  // Press Down to move from option 0 to option 1
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Down));

  let pq = data.pending_questions.as_ref().unwrap();
  assert_eq!(pq.selected_option_idx, 1);
  assert!(cmd_rx.try_recv().is_err()); // no command sent yet
}

#[test]
fn test_question_single_select_enter() {
  let (session_handle, _cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_pending_questions());

  // Press Enter to confirm first question (single-select)
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));

  // Should move to next question
  let pq = data.pending_questions.as_ref().unwrap();
  assert_eq!(pq.current_question_idx, 1);
  assert_eq!(pq.answers.len(), 1);
  assert_eq!(pq.answers[0], vec![0]); // first option selected
}

#[test]
fn test_question_multi_select_toggle() {
  let (session_handle, _cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_pending_questions());

  // Move to second question (multi-select)
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));
  assert_eq!(
    data
      .pending_questions
      .as_ref()
      .unwrap()
      .current_question_idx,
    1
  );

  // Toggle option 0 with Space
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char(' ')));
  let pq = data.pending_questions.as_ref().unwrap();
  assert_eq!(pq.answers[1], vec![0]);

  // Move down and toggle option 1
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Down));
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char(' ')));
  let pq = data.pending_questions.as_ref().unwrap();
  assert_eq!(pq.answers[1], vec![0, 1]);

  // Toggle option 0 off
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Up));
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char(' ')));
  let pq = data.pending_questions.as_ref().unwrap();
  assert_eq!(pq.answers[1], vec![1]);
}

#[test]
fn test_question_complete_all_and_submit() {
  let (session_handle, mut cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_pending_questions());

  // Answer first question (single-select, option 1)
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Down));
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));

  // Answer second question (multi-select, toggle option 0)
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char(' ')));
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));

  // pending_questions should be cleared and command sent
  assert!(data.pending_questions.is_none());
  let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
  match cmd {
    SessionCommand::AnswerQuestions {
      tool_call_id,
      answers,
      dismissed,
    } => {
      assert_eq!(tool_call_id, "call-123");
      assert!(!dismissed);
      assert_eq!(answers.len(), 2);
      assert_eq!(answers[0], vec![1]); // Blue
      assert_eq!(answers[1], vec![0]); // Small
    }
    other => panic!("Expected AnswerQuestions, got {:?}", other),
  }
}

#[test]
fn test_question_dismiss_with_q() {
  let (session_handle, mut cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_pending_questions());

  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('q')));

  assert!(data.pending_questions.is_none());
  let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
  match cmd {
    SessionCommand::AnswerQuestions {
      tool_call_id,
      answers,
      dismissed,
    } => {
      assert_eq!(tool_call_id, "call-123");
      assert!(dismissed);
      assert!(answers.is_empty());
    }
    other => panic!("Expected AnswerQuestions, got {:?}", other),
  }
}

#[test]
fn test_question_dismiss_with_esc() {
  let (session_handle, mut cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_pending_questions());

  view.handle_key(&mut data, KeyEvent::from(KeyCode::Esc));

  assert!(data.pending_questions.is_none());
  let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
  match cmd {
    SessionCommand::AnswerQuestions {
      tool_call_id,
      answers,
      dismissed,
    } => {
      assert_eq!(tool_call_id, "call-123");
      assert!(dismissed);
      assert!(answers.is_empty());
    }
    other => panic!("Expected AnswerQuestions, got {:?}", other),
  }
}

#[test]
fn test_question_digit_quick_select() {
  let (session_handle, _cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_pending_questions());

  // Press '2' to select option 1 (0-indexed) and auto-confirm single-select
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('2')));

  let pq = data.pending_questions.as_ref().unwrap();
  assert_eq!(pq.current_question_idx, 1); // moved to next question
  assert_eq!(pq.answers[0], vec![1]); // Blue selected
}

fn make_confirmation_question() -> PendingQuestions {
  PendingQuestions {
    tool_call_id: "call-confirm".to_string(),
    questions: vec![Question {
      question: "Are you sure?".to_string(),
      header: "Confirm".to_string(),
      options: vec![
        crate::llm::session::QuestionOption {
          label: "Yes".to_string(),
          description: String::new(),
        },
        crate::llm::session::QuestionOption {
          label: "No".to_string(),
          description: String::new(),
        },
      ],
      multi_select: false,
      confirmation: true,
      default: Vec::new(),
      required: false,
    }],
    current_question_idx: 0,
    answers: Vec::new(),
    selected_option_idx: 0,
  }
}

#[test]
fn test_question_confirmation_yes() {
  let (session_handle, mut cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_confirmation_question());

  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('y')));

  assert!(data.pending_questions.is_none());
  let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
  match cmd {
    SessionCommand::AnswerQuestions {
      tool_call_id,
      answers,
      dismissed,
    } => {
      assert_eq!(tool_call_id, "call-confirm");
      assert!(!dismissed);
      assert_eq!(answers, vec![vec![0]]); // Yes = index 0
    }
    other => panic!("Expected AnswerQuestions, got {:?}", other),
  }
}

#[test]
fn test_question_confirmation_no() {
  let (session_handle, mut cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(make_confirmation_question());

  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('n')));

  assert!(data.pending_questions.is_none());
  let cmd = cmd_rx.try_recv().expect("Expected AnswerQuestions command");
  match cmd {
    SessionCommand::AnswerQuestions {
      tool_call_id,
      answers,
      dismissed,
    } => {
      assert_eq!(tool_call_id, "call-confirm");
      assert!(!dismissed);
      assert_eq!(answers, vec![vec![1]]); // No = index 1
    }
    other => panic!("Expected AnswerQuestions, got {:?}", other),
  }
}

#[test]
fn test_question_default_value_preselected() {
  let (session_handle, _cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(PendingQuestions {
    tool_call_id: "call-def".to_string(),
    questions: vec![Question {
      question: "Pick one".to_string(),
      header: "Choice".to_string(),
      options: vec![
        crate::llm::session::QuestionOption {
          label: "A".to_string(),
          description: String::new(),
        },
        crate::llm::session::QuestionOption {
          label: "B".to_string(),
          description: String::new(),
        },
      ],
      multi_select: false,
      confirmation: false,
      default: vec![1], // default is B
      required: false,
    }],
    current_question_idx: 0,
    answers: Vec::new(),
    selected_option_idx: 0,
  });

  // Default should auto-select option 1 and move to next/submit
  // Since it's single-select with default, the default is already applied
  // Check that pressing Enter submits the default
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));
  assert!(data.pending_questions.is_none());
}

#[test]
fn test_question_required_blocks_empty() {
  let (session_handle, _cmd_rx) = make_session_handle();
  let mut data = AppData::new();
  let mut view = ChatView::new(
    &data,
    session_handle,
    std::sync::Arc::new(Runtime::for_test(Config::default())),
  );
  data.pending_questions = Some(PendingQuestions {
    tool_call_id: "call-req".to_string(),
    questions: vec![Question {
      question: "Required?".to_string(),
      header: "".to_string(),
      options: vec![
        crate::llm::session::QuestionOption {
          label: "Opt1".to_string(),
          description: String::new(),
        },
        crate::llm::session::QuestionOption {
          label: "Opt2".to_string(),
          description: String::new(),
        },
      ],
      multi_select: false,
      confirmation: false,
      default: Vec::new(),
      required: true,
    }],
    current_question_idx: 0,
    answers: Vec::new(),
    selected_option_idx: 0,
  });

  view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));

  // Should still be pending because required and nothing selected
  assert!(data.pending_questions.is_some());
  let pq = data.pending_questions.as_ref().unwrap();
  assert_eq!(pq.current_question_idx, 0);

  // Now select an option
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Char('1')));
  view.handle_key(&mut data, KeyEvent::from(KeyCode::Enter));
  assert!(data.pending_questions.is_none());
}
