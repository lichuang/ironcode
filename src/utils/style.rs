//! Predefined styles for consistent UI styling across the application.

use std::sync::LazyLock;

use ratatui::style::{Modifier, Style};

use crate::utils::colors::{
  BLUE as BlueColor, ERROR as ErrorColor, GREEN as GreenColor, HIGHLIGHT as HighlightColor,
  PRIMARY as PrimaryColor, SUBTLE as SubtleColor,
};

/// Primary text style - for active elements and highlights
pub static PRIMARY: LazyLock<Style> = LazyLock::new(|| Style::default().fg(PrimaryColor));

/// Highlight text style - for important elements and keyboard shortcuts
pub static HIGHLIGHT: LazyLock<Style> = LazyLock::new(|| Style::default().fg(HighlightColor));

/// Thinking content style - italic subtle color
pub static THINKING: LazyLock<Style> = LazyLock::new(|| {
  Style::default()
    .fg(SubtleColor)
    .add_modifier(Modifier::ITALIC)
});

/// Primary border style
pub static PRIMARY_BORDER: LazyLock<Style> = LazyLock::new(|| Style::default().fg(PrimaryColor));

#[allow(dead_code)]
/// Error border style
pub static ERROR_BORDER: LazyLock<Style> = LazyLock::new(|| Style::default().fg(ErrorColor));

#[allow(dead_code)]
/// Blue text style - for tool names
pub static BLUE: LazyLock<Style> = LazyLock::new(|| Style::default().fg(BlueColor));

#[allow(dead_code)]
/// Green text style - for success indicators
pub static GREEN: LazyLock<Style> = LazyLock::new(|| Style::default().fg(GreenColor));
