//! Animation frames for UI loading indicators.

/// Spinner animation frames (classic terminal loading spinner)
/// 
/// Cycles through braille patterns for a smooth spinning animation.
pub const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Moon phase animation frames (🌑 → 🌕 → 🌑)
/// 
/// Shows the moon cycling through its phases, used when waiting for
/// LLM response to start streaming.
pub const MOON_FRAMES: &[char] = &['🌑', '🌒', '🌓', '🌔', '🌕', '🌖', '🌗', '🌘'];
