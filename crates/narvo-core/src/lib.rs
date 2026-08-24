//! Core runtime primitives shared by every other Narvo crate.
//!
//! Responsibility: the deterministic, platform-independent foundation of the
//! engine. Clocks and frame timing, the fixed-timestep scheduler that decouples
//! simulation rate from frame rate, the event queue that subsystems use to talk
//! to each other, seeded random number generation, and the error types the rest
//! of the workspace returns.
//!
//! API boundary: this crate knows nothing about windows, graphics APIs, audio
//! devices or the filesystem, and it depends on no other Narvo crate. It sits
//! at the bottom of the dependency graph — everything above may depend on it,
//! it depends on nothing. Anything that needs an operating-system resource, or
//! that only makes sense for one backend, belongs in a sibling crate. Its API
//! is expected to stay callable from a headless test with no I/O at all, which
//! is what makes engine behaviour reproducible.

pub mod frame;
pub mod sha256;
pub mod time;

pub use frame::{Acquisition, Clock, FrameHost, FrameLog, FrameLoop, FramePhase, FrameRecord};
pub use sha256::{hex, sha256};
pub use time::FixedTimestep;

#[cfg(test)]
mod tests {
    /// Smoke test: the crate builds and its test harness actually runs.
    #[test]
    fn crate_is_wired_into_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "narvo-core");
    }
}
