//! Wire protocol layer — decouples core session logic from UI via broadcast bus.
//!
//! The wire module provides a message bus that allows the session actor
//! (producer) and the UI (consumer) to communicate without direct coupling.
//! This enables the same core logic to drive TUI, print mode, or a future web UI.

pub mod bus;
pub mod protocol;

pub use bus::{WireBus, WirePublisher};
pub use protocol::WireMessage;
