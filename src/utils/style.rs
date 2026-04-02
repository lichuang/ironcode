//! Predefined styles for consistent UI styling across the application.

use std::sync::LazyLock;

use ratatui::style::{Modifier, Style};

use crate::utils::colors::{
  BLUE as BlueColor, ERROR as ErrorColor, GREEN as GreenColor, HIGHLIGHT as HighlightColor,
  MUTED as MutedColor, PRIMARY as PrimaryColor, SUBTLE as SubtleColor, TEXT as TextColor,
};

/// Primary text style - for active elements and highlights
pub static PRIMARY: LazyLock<Style> = LazyLock::new(|| Style::default().fg(PrimaryColor));

/// Highlight text style - for important elements and keyboard shortcuts
pub static HIGHLIGHT: LazyLock<Style> = LazyLock::new(|| Style::default().fg(HighlightColor));

#[allow(dead_code)]
/// Muted text style - for secondary information
pub static MUTED: LazyLock<Style> = LazyLock::new(|| Style::default().fg(MutedColor));

#[allow(dead_code)]
/// Subtle text style - for hints and metadata
pub static SUBTLE: LazyLock<Style> = LazyLock::new(|| Style::default().fg(SubtleColor));

#[allow(dead_code)]
/// Error text style - for error messages
pub static ERROR: LazyLock<Style> = LazyLock::new(|| Style::default().fg(ErrorColor));

#[allow(dead_code)]
/// Default text style
pub static TEXT: LazyLock<Style> = LazyLock::new(|| Style::default().fg(TextColor));

#[allow(dead_code)]
/// Title style - bold primary color
pub static TITLE: LazyLock<Style> = LazyLock::new(|| {
  Style::default()
    .fg(PrimaryColor)
    .add_modifier(Modifier::BOLD)
});

/// Thinking content style - italic subtle color
pub static THINKING: LazyLock<Style> = LazyLock::new(|| {
  Style::default()
    .fg(SubtleColor)
    .add_modifier(Modifier::ITALIC)
});

/// Primary border style
pub static PRIMARY_BORDER: LazyLock<Style> = LazyLock::new(|| Style::default().fg(PrimaryColor));

#[allow(dead_code)]
/// Highlight border style
pub static HIGHLIGHT_BORDER: LazyLock<Style> =
  LazyLock::new(|| Style::default().fg(HighlightColor));

#[allow(dead_code)]
/// Error border style
pub static ERROR_BORDER: LazyLock<Style> = LazyLock::new(|| Style::default().fg(ErrorColor));

#[allow(dead_code)]
/// Blue text style - for tool names
pub static BLUE: LazyLock<Style> = LazyLock::new(|| Style::default().fg(BlueColor));

#[allow(dead_code)]
/// Green text style - for success indicators
pub static GREEN: LazyLock<Style> = LazyLock::new(|| Style::default().fg(GreenColor));
