//! Tool handlers implementation.
//!
//! Each handler implements the `ToolHandler` trait and provides
//! the actual implementation for a specific tool.

pub mod ask_user;
pub mod file;
pub mod shell;

pub use ask_user::AskUserQuestionHandler;
pub use file::{GlobHandler, GrepHandler, ReadFileHandler, ReplaceFileHandler, WriteFileHandler};

// Export shell handlers based on platform
#[cfg(target_os = "windows")]
pub use shell::PowerShellHandler;
#[cfg(not(target_os = "windows"))]
pub use shell::BashHandler;
