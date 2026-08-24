//! The device vocabulary: what a mapping file binds *from*.
//!
//! Everything in this module is this crate's own. No window library's types
//! appear here and none may: the whole point of the layer is that a mapping file
//! and everything below it keep meaning what they mean when the window library
//! is replaced, and a re-exported foreign enum would make the file format an
//! alias for that library's release notes.
//!
//! Nothing here can be serialized *out*. [`Control`] derives `Deserialize` and
//! not `Serialize`, because a mapping file names controls and nothing in this
//! crate ever writes one; [`DeviceEvent`] and [`Edge`] carry no `serde` at all.
//! That is deliberate and it is the code half of a decision that is otherwise
//! only prose: the one reason to give these types an output form would be to
//! write device terms into a file, which is exactly the question D8 is open on
//! (`ProjektPlan.md` §11, decided in M5.2). Until it is decided, this crate
//! cannot take a step towards either answer by accident.

use serde::Deserialize;

/// One physical control on an input device, by the name a mapping file uses.
///
/// # Why a closed set and not a string
///
/// A name the loader cannot check is a name a typo survives: `KeyW` misspelt as
/// `Keyw` would bind nothing, silently, and the file would look right. As an
/// enum, a misspelling is a load error that names what was written and lists
/// every control there is — which is the message this crate would otherwise have
/// had to write and keep in step by hand.
///
/// # What is in it, and what is deliberately not
///
/// The set below is the smallest one that lets a mapping file for this project's
/// own headless demo be written: a movement cluster, the arrows beside it, three
/// keys with no letter on them, and the ten digits for a discrete choice.
///
/// Everything else a keyboard has — function keys, the numpad, modifiers,
/// punctuation — and every device that is not a keyboard is **absent on
/// purpose**, not forgotten. `ProjektPlan.md` §2 rules out building on stock, and
/// a vocabulary guessed at before there is a device to be faithful to is stock:
/// M5.3 is where winit's key list first has to be translated into this one, and
/// that is where the set's completeness gets decided against something real. The
/// enum is `#[non_exhaustive]` so that growing it is not a breaking change, and
/// ADR-0025 records the growth as additive by design.
///
/// # The names
///
/// A control is named by its **position** on the device, not by the character it
/// produces: `KeyW` is the key where `W` is on a US layout, whatever it types on
/// another one. A binding is about where a finger goes, and a layout that moves
/// the letters should move the letters and not the movement cluster.
///
/// The spellings follow the W3C UI Events `code` convention (`KeyW`, `Digit0`,
/// `ArrowUp`), which is what most of the ecosystem uses. That is convergence and
/// not a dependency: nothing here reads a foreign type, and the day this crate
/// needs a name that convention does not have, it takes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[non_exhaustive]
pub enum Control {
    /// The key where `W` is on a US layout.
    KeyW,
    /// The key where `A` is on a US layout.
    KeyA,
    /// The key where `S` is on a US layout.
    KeyS,
    /// The key where `D` is on a US layout.
    KeyD,
    /// The up arrow.
    ArrowUp,
    /// The down arrow.
    ArrowDown,
    /// The left arrow.
    ArrowLeft,
    /// The right arrow.
    ArrowRight,
    /// The space bar.
    Space,
    /// The enter or return key.
    Enter,
    /// The escape key.
    Escape,
    /// The `0` key on the main row.
    Digit0,
    /// The `1` key on the main row.
    Digit1,
    /// The `2` key on the main row.
    Digit2,
    /// The `3` key on the main row.
    Digit3,
    /// The `4` key on the main row.
    Digit4,
    /// The `5` key on the main row.
    Digit5,
    /// The `6` key on the main row.
    Digit6,
    /// The `7` key on the main row.
    Digit7,
    /// The `8` key on the main row.
    Digit8,
    /// The `9` key on the main row.
    Digit9,
}

/// Which way a control just moved.
///
/// The whole of the device model in M5.1: a control is down or it is up, and the
/// only thing that ever happens is a change between the two. Analogue axes and
/// positions are outside this milestone by decision (`ProjektPlan.md` §6/M5,
/// delegated decision 3) rather than by oversight, and ADR-0012 leaves the
/// spelling of an analogue magnitude open until there is a device to be faithful
/// to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Edge {
    /// The control went down.
    Press,
    /// The control came back up.
    Release,
}

/// One thing a device did: a control, and which way it moved.
///
/// # It carries no time and no repeat
///
/// A tick's worth of these is a slice, and its order is the order they happened
/// in — that is the whole of the timing information [`Mapping::map`] needs, and
/// deliberately all it gets. Two consequences worth stating, because both are
/// somebody else's job rather than nobody's:
///
/// - **Auto-repeat is not filtered here.** An operating system that reports a
///   held key as a stream of presses produces a stream of events, and this crate
///   maps every one of them. Whoever reads the device knows whether a press is a
///   repeat; a pure function over a slice does not, and giving it the state to
///   find out would make the mapping stateful for one caller's convenience.
/// - **A press with no matching release is not an error.** The mapping has no
///   opinion about what a device is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceEvent {
    control: Control,
    edge: Edge,
}

impl DeviceEvent {
    /// `control` went down.
    #[must_use]
    pub const fn press(control: Control) -> Self {
        Self {
            control,
            edge: Edge::Press,
        }
    }

    /// `control` came back up.
    #[must_use]
    pub const fn release(control: Control) -> Self {
        Self {
            control,
            edge: Edge::Release,
        }
    }

    /// Which control it was.
    #[must_use]
    pub const fn control(self) -> Control {
        self.control
    }

    /// Which way it moved.
    #[must_use]
    pub const fn edge(self) -> Edge {
        self.edge
    }
}

#[cfg(test)]
mod tests {
    use super::{Control, DeviceEvent, Edge};

    #[test]
    fn a_control_is_spelled_in_a_file_exactly_as_debug_prints_it() {
        // Both spellings come from the one variant identifier - `serde`'s derive
        // takes the name from it, and so does `Debug`. That is what lets
        // `InputError::DuplicateBinding` name the control with `{control:?}` and
        // still be quoting the file. A representative sample rather than every
        // variant: this pins the *rule*, and a new variant cannot break it
        // without breaking every other one too.
        for control in [
            Control::KeyW,
            Control::ArrowLeft,
            Control::Space,
            Control::Digit7,
        ] {
            let printed = format!("{control:?}");

            assert_eq!(
                ron::from_str::<Control>(&printed).expect("a control's own name parses"),
                control,
                "{printed} should read back as the control it names"
            );
        }
    }

    #[test]
    fn an_unknown_control_is_refused_and_the_message_lists_the_known_ones() {
        let error = ron::from_str::<Control>("Keyw").expect_err("the vocabulary is closed");
        let message = error.code.to_string();

        assert!(message.contains("Keyw"), "{message}");
        assert!(message.contains("KeyW"), "{message}");
        assert!(message.contains("Digit9"), "{message}");
    }

    #[test]
    fn a_device_event_reports_what_it_was_built_from() {
        let pressed = DeviceEvent::press(Control::KeyA);
        assert_eq!(
            (pressed.control(), pressed.edge()),
            (Control::KeyA, Edge::Press)
        );

        let released = DeviceEvent::release(Control::KeyA);
        assert_eq!(
            (released.control(), released.edge()),
            (Control::KeyA, Edge::Release)
        );
    }
}
