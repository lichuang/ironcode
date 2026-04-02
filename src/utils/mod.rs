pub mod animation;
pub mod colors;
pub mod string;
pub mod style;
pub mod time;

pub use animation::{MOON_FRAMES, SPINNER_FRAMES};
// pub use colors::{ERROR as Error, PRIMARY as Primary, SECONDARY};  // Currently unused
pub use string::{char_display_width, string_display_width};
pub use style::{HIGHLIGHT, PRIMARY, PRIMARY_BORDER, THINKING};
