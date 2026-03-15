//! LLM (Large Language Model) integration module.
//!
//! Provides a unified interface for interacting with various LLM providers.
//! Currently supports OpenAI-compatible APIs.

pub mod openai;
pub mod session;
pub mod types;

pub use openai::OpenAIClient;
pub use session::{ChatSession, SessionCommand, SessionEvent, SessionHandle};
pub use types::*;
