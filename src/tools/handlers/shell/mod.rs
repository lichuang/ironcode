//! Shell-related tool handlers.
//!
//! These handlers implement tools for executing shell commands.

pub mod bash;

#[cfg(target_os = "windows")]
pub mod powershell;

pub use bash::BashHandler;

#[cfg(target_os = "windows")]
pub use powershell::PowerShellHandler;
