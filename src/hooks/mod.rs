//! User-configurable lifecycle hooks.
//!
//! Hooks are shell commands defined in `config.toml` under `[[hooks]]`. Each
//! hook specifies an event (e.g. `PreToolUse`), an optional regex matcher, and
//! a command to execute. The command receives a JSON payload on stdin and can
//! return a decision via exit code or structured stdout.
//!
//! In addition to server-side shell hooks, the engine supports client-side
//! "wire" hooks. Wire subscriptions are registered dynamically and forward
//! hook requests to an external client (TUI, Web UI, IDE plugin) via the
//! `WireHookDispatcher` trait. See `wire.rs` for the bus-based dispatcher.
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
pub mod wire;

pub use config::{HookDef, HookEventType};
#[allow(unused_imports)]
pub use engine::{
  HookDetail, HookEngine, OnHookResolved, OnHookTriggered, WireHookDispatcher, WireHookHandle,
  WireHookSubscription,
};
pub use runner::HookDecision;
#[allow(unused_imports)]
pub use wire::{WireBusHookDispatcher, WireHookResponse};
