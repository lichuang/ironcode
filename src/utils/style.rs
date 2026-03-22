//! Predefined styles for consistent UI styling across the application.

use std::sync::LazyLock;

use ratatui::style::{Modifier, Style};

use crate::utils::colors::{
  ERROR as ErrorColor, HIGHLIGHT as HighlightColor, MUTED as MutedColor,
  PRIMARY as PrimaryColor, SUBTLE as SubtleColor, TEXT as TextColor,
};

/// Primary text style - for active elements and highlights
pub static PRIMARY: LazyLock<Style> = LazyLock::new(|| Style::default().fg(PrimaryColor));

/// Highlight text style - for important elements and keyboard shortcuts
pub static HIGHLIGHT: LazyLock<Style> = LazyLock::new(|| Style::default().fg(HighlightColor));

/// Muted text style - for secondary information
pub static MUTED: LazyLock<Style> = LazyLock::new(|| Style::default().fg(MutedColor));

/// Subtle text style - for hints and metadata
pub static SUBTLE: LazyLock<Style> = LazyLock::new(|| Style::default().fg(SubtleColor));

/// Error text style - for error messages
pub static ERROR: LazyLock<Style> = LazyLock::new(|| Style::default().fg(ErrorColor));

/// Default text style
pub static TEXT: LazyLock<Style> = LazyLock::new(|| Style::default().fg(TextColor));

/// Title style - bold primary color
pub static TITLE: LazyLock<Style> = LazyLock::new(|| Style::default().fg(PrimaryColor).add_modifier(Modifier::BOLD));

/// Thinking content style - italic subtle color
pub static THINKING: LazyLock<Style> = LazyLock::new(|| Style::default().fg(SubtleColor).add_modifier(Modifier::ITALIC));

/// Primary border style
pub static PRIMARY_BORDER: LazyLock<Style> = LazyLock::new(|| Style::default().fg(PrimaryColor));

/// Highlight border style
pub static HIGHLIGHT_BORDER: LazyLock<Style> = LazyLock::new(|| Style::default().fg(HighlightColor));

/// Error border style
pub static ERROR_BORDER: LazyLock<Style> = LazyLock::new(|| Style::default().fg(ErrorColor));
