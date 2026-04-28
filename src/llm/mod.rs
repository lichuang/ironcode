//! LLM (Large Language Model) integration module.
//!
//! Provides a unified interface for interacting with various LLM providers.
//! Currently supports OpenAI-compatible APIs.

pub mod compaction;
pub mod openai;
pub mod provider;
pub mod providers;
pub mod session;
pub mod types;

// pub use openai::OpenAIClient;  // Currently unused
// pub use provider::LLMProvider;  // Currently unused
// pub use providers::KimiProvider;  // Currently unused
pub use session::{ChatSession, Question, SessionEvent, SessionHandle};
// pub use types::*;  // Currently unused - import specific types when needed
