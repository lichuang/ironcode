//! Color constants for consistent UI styling across the application.

use ratatui::style::Color;

/// Primary accent color - used for active elements and highlights
pub const PRIMARY: Color = Color::Cyan;

#[allow(dead_code)]
/// Secondary accent color - used for prompts and success states
pub const SECONDARY: Color = Color::Green;

/// Highlight color - used for important elements and keyboard shortcuts
pub const HIGHLIGHT: Color = Color::Yellow;

#[allow(dead_code)]
/// Error color - used for error messages and failed states
pub const ERROR: Color = Color::Red;

#[allow(dead_code)]
/// Muted text color - used for secondary information
pub const MUTED: Color = Color::Gray;

/// Subtle text color - used for hints and metadata
pub const SUBTLE: Color = Color::DarkGray;

/// Default text color
pub const TEXT: Color = Color::White;

#[allow(dead_code)]
/// Blue color - used for tool names in tool call indicators
pub const BLUE: Color = Color::Blue;

#[allow(dead_code)]
/// Green color - used for success indicators (bullet points, etc.)
pub const GREEN: Color = Color::Green;
