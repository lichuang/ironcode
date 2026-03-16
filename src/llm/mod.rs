//! LLM (Large Language Model) integration module.
//!
//! Provides a unified interface for interacting with various LLM providers.
//! Currently supports OpenAI-compatible APIs.

pub mod openai;
pub mod provider;
pub mod providers;
pub mod session;
pub mod types;

pub use openai::OpenAIClient;
pub use provider::LLMProvider;
pub use providers::KimiProvider;
pub use session::{ChatSession, SessionCommand, SessionEvent, SessionHandle};
pub use types::*;
