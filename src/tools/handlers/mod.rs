//! Tool handlers implementation.
//!
//! Each handler implements the `ToolHandler` trait and provides
//! the actual implementation for a specific tool.

pub mod ask_user;
pub mod file;
pub mod plan;
pub mod shell;
pub mod todo;
pub mod web;

pub use ask_user::AskUserQuestionHandler;
pub use file::{GlobHandler, GrepHandler, ReadFileHandler, ReplaceFileHandler, WriteFileHandler};
pub use plan::{EnterPlanModeHandler, ExitPlanModeHandler};
pub use todo::SetTodoListHandler;
pub use web::{FetchURLHandler, SearchWebHandler};

// Export shell handlers based on platform
#[cfg(not(target_os = "windows"))]
pub use shell::BashHandler;
#[cfg(target_os = "windows")]
pub use shell::PowerShellHandler;
