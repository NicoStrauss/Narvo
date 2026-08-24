//! Device input, mapped to the named actions a simulation consumes.
//!
//! Responsibility: everything between a physical control and an
//! [`InputEvent`]. What a *world* is belongs to `narvo-ecs`; what a *window* is
//! will belong to the runner (M5.3). This crate owns the layer between them —
//! the vocabulary of controls, the file that binds them to actions, and the pure
//! function that applies the one to the other.
//!
//! Read ADR-0025 before changing the mapping format, and ADR-0012 before
//! changing [`InputEvent`].
//!
//! # The two rules that make this layer worth having
//!
//! 1. **Nothing below this crate knows what a keyboard is.** An
//!    [`InputEvent`] names an action, and a recording of one keeps meaning what
//!    it meant after the bindings change under it (ADR-0012).
//! 2. **Nothing in this crate knows what a window is.** [`Control`] is this
//!    crate's own vocabulary, not a re-export, so the mapping file is not an
//!    alias for a window library's release notes. Translating one into the other
//!    is M5.3's job and it happens in the runner, above here.
//!
//! # Determinism
//!
//! Nothing here reads a clock or a source of entropy. [`Mapping::map`] is a pure
//! function of a mapping and a slice of device events, and its output order is
//! the slice's order — see its documentation for why that is a complete
//! statement rather than an approximation.
//!
//! # What this crate deliberately does not do
//!
//! It does not record anything, and it holds no format field that a recording
//! could grow into. Whether a repro should store device events or the actions
//! they map to is **D8**, open in `ProjektPlan.md` §11 and due in M5.2 with a
//! real mapping in hand, which is what ADR-0012's own revision condition asks
//! for. This crate is that mapping and is deliberately neutral on the question:
//! [`DeviceEvent`] and [`Edge`] have no `serde` at all, and [`Control`] can be
//! read from a file but not written to one.
//!
//! # Examples
//!
//! ```
//! use narvo_input::{Control, DeviceEvent};
//!
//! let mapping = narvo_input::from_str(
//!     r#"Mapping(bindings: [
//!         (control: KeyW, action: "thrust", emit: OnPressAndRelease(press: 1, release: 0)),
//!     ])"#,
//! )?;
//!
//! let events = mapping.map(&[DeviceEvent::press(Control::KeyW)]);
//! assert_eq!((events[0].action(), events[0].value()), ("thrust", 1));
//! # Ok::<(), narvo_input::InputError>(())
//! ```

mod control;
mod error;
mod event;
mod mapping;

pub use control::{Control, DeviceEvent, Edge};
pub use error::InputError;
pub use event::InputEvent;
pub use mapping::{Binding, Emit, Mapping, from_file, from_str};
