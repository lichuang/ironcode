//! Background task management system.
//!
//! Inspired by kimi-cli's background task system. Tasks are spawned as
//! independent worker processes so they survive the main CLI process exit.
//! Each task is persisted on disk under the session directory:
//!   ~/.ironcode/sessions/{session_id}/tasks/{task_id}/
//!
//! Files per task:
//! - `spec.json`: task specification (command, timeout, etc.)
//! - `runtime.json`: mutable runtime state (status, pid, heartbeat, etc.)
//! - `control.json`: control signals (kill requests)
//! - `consumer.json`: consumer viewing state
//! - `output.log`: captured stdout/stderr

pub mod ids;
pub mod manager;
pub mod models;
pub mod store;
pub mod worker;

pub use manager::BackgroundTaskManager;
pub use models::{TaskOutputChunk, TaskStatus, TaskView};
pub use worker::run_background_task_worker;
