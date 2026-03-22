//! Color constants for consistent UI styling across the application.

use ratatui::style::Color;

/// Primary accent color - used for active elements and highlights
pub const PRIMARY: Color = Color::Cyan;

/// Secondary accent color - used for prompts and success states
pub const SECONDARY: Color = Color::Green;

/// Highlight color - used for important elements and keyboard shortcuts
pub const HIGHLIGHT: Color = Color::Yellow;

/// Error color - used for error messages and failed states
pub const ERROR: Color = Color::Red;

/// Muted text color - used for secondary information
pub const MUTED: Color = Color::Gray;

/// Subtle text color - used for hints and metadata
pub const SUBTLE: Color = Color::DarkGray;

/// Default text color
pub const TEXT: Color = Color::White;
