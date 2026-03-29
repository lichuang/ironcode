//! Web-related tool handlers.
//!
//! These handlers implement tools for web operations like
//! fetching URLs and searching the web.

pub mod fetch_url;
pub mod search_web;

pub use fetch_url::FetchURLHandler;
pub use search_web::SearchWebHandler;
