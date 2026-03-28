pub mod animation;
pub mod colors;
pub mod string;
pub mod style;
pub mod time;

pub use animation::{MOON_FRAMES, SPINNER_FRAMES};
pub use colors::{ERROR as Error, PRIMARY as Primary, SECONDARY};
pub use string::{char_display_width, prefix_display_width, string_display_width};
pub use style::{
  BLUE, ERROR, ERROR_BORDER, GREEN, HIGHLIGHT, HIGHLIGHT_BORDER, MUTED, PRIMARY, PRIMARY_BORDER,
  SUBTLE, TEXT, THINKING, TITLE,
};
