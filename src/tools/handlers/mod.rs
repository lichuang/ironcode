//! Tool handlers implementation.
//!
//! Each handler implements the `ToolHandler` trait and provides
//! the actual implementation for a specific tool.

pub mod file;
pub mod shell;

pub use file::{GlobHandler, GrepHandler, ReadFileHandler, ReplaceFileHandler, WriteFileHandler};

// Export shell handlers based on platform
#[cfg(target_os = "windows")]
pub use shell::PowerShellHandler;
#[cfg(not(target_os = "windows"))]
pub use shell::BashHandler;
