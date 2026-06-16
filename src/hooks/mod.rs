//! User-configurable lifecycle hooks.
//!
//! Hooks are shell commands defined in `config.toml` under `[[hooks]]`. Each
//! hook specifies an event (e.g. `PreToolUse`), an optional regex matcher, and
//! a command to execute. The command receives a JSON payload on stdin and can
//! return a decision via exit code or structured stdout.
//!
//! Supported events:
//! - `PreToolUse`, `PostToolUse`, `PostToolUseFailure`
//! - `UserPromptSubmit`
//! - `Stop`, `StopFailure`
//! - `SessionStart`, `SessionEnd`
//! - `PreCompact`, `PostCompact`
//! - `Notification`

pub mod config;
pub mod engine;
pub mod events;
pub mod runner;

pub use config::{HookDef, HookEventType};
pub use engine::HookEngine;
pub use runner::HookDecision;
