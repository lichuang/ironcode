//! LLM (Large Language Model) integration module.
//!
//! Provides a unified interface for interacting with various LLM providers.
//! Currently supports OpenAI-compatible APIs.

pub mod openai;
pub mod types;

pub use openai::OpenAIClient;
pub use types::*;
