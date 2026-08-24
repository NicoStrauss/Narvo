//! The mapping file, and the pure function that turns device events into input
//! events.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::control::{Control, DeviceEvent, Edge};
use crate::error::InputError;
use crate::event::InputEvent;

/// What a binding puts on the wire, and on which edge.
///
/// # Why the file says this rather than the consumer assuming it
///
/// A key is down or up, so a binding could always emit on both edges — press
/// `1`, release `0` — and let a simulation ignore the halves it does not want.
/// That is what the table in [`InputEvent`]'s documentation describes, and for a
/// control that is *held* it is exactly right: thrust has to stop when the key
/// comes up, and only a second event can say so.
///
/// It is wrong for the other kind. A click that buys an upgrade happens once,
/// and a binding that also emitted `buy 0` on release would hand every consumer
/// an event it has to know to ignore. "An event the consumer must ignore" is the
/// shape of bug this project spends its time avoiding: nothing announces it,
/// nothing fails, and the first consumer that forgets buys twice. The pending
/// buffer is also part of the state hash (ADR-0011), so the ignored half is not
/// free — it is one more piece of simulation state per release.
///
/// So the file says which kind a binding is, and there is no default. Both
/// variants are spelled out where they are written, and a reader never has to
/// know a rule to know what a line does.
///
/// A third policy — fire on release and not on press, which is what a button in
/// a user interface usually does — has no consumer yet and is not here.
/// `ProjektPlan.md` §2 rules out building on stock; the enum is
/// `#[non_exhaustive]` so that adding it later is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum Emit {
    /// One event when the control goes down, and nothing when it comes back up.
    ///
    /// For an action that *happens*: a click, a selection, a jump.
    OnPress(i64),
    /// One event on each edge, with the value the file gives for that edge.
    ///
    /// For an action that is *held*: thrust, a steering direction, a modifier.
    /// The release value is almost always `0`, but it is written rather than
    /// assumed, because a binding that steers left with `-1` may well want to
    /// come to rest at something other than zero and nothing here should decide
    /// that for it.
    OnPressAndRelease {
        /// The magnitude the down edge sends.
        press: i64,
        /// The magnitude the up edge sends.
        release: i64,
    },
}

/// One line of a mapping file: a control, an action, and what to emit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// The physical control this binding listens to.
    control: Control,
    /// The action name the events it produces carry.
    action: String,
    /// What it puts on the wire, and on which edge.
    emit: Emit,
}

impl Binding {
    /// The physical control this binding listens to.
    #[must_use]
    pub fn control(&self) -> Control {
        self.control
    }

    /// The action name the events it produces carry.
    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    /// What it puts on the wire, and on which edge.
    #[must_use]
    pub fn emit(&self) -> Emit {
        self.emit
    }
}

/// The mapping file's outer shape.
///
/// A separate type from [`Mapping`] because the two are not the same thing: this
/// is a list of bindings in the order they were written, and a `Mapping` is that
/// list after the two rules this crate enforces have been checked. Nothing
/// outside the module ever holds one of these.
///
/// It is renamed to `Mapping` for `serde` because a file may open with a struct
/// name and `ron` **checks the one it finds** — a file beginning `Mapping(`
/// against a type called `MappingFile` fails with "Expected struct `MappingFile`
/// but found `Mapping`". That was measured, not assumed: the first version of
/// this crate's own probe wrote the bare `(` form, which parses either way and
/// hid the rule. So the name a file writes is the name a reader would expect it
/// to write, and the private Rust type keeps the name that says what it is.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename = "Mapping")]
struct MappingFile {
    /// The bindings, in file order.
    bindings: Vec<Binding>,
}

/// A loaded mapping: every binding, checked, and a lookup from control to
/// binding.
///
/// # What being loaded guarantees
///
/// Two things, and both of them are why [`map`](Self::map) cannot fail:
///
/// 1. **Every action name is an identifier**, so an [`InputEvent`] built from
///    one cannot be rejected.
/// 2. **Every control is bound at most once**, so one device event produces at
///    most one input event.
///
/// # Order
///
/// [`map`](Self::map) emits in the order of the device events it was given, and
/// nothing else. Rule 2 is what makes that a complete statement: with one
/// binding per control there is never a set of bindings to put in some order, so
/// no container's iteration order can reach the output. The lookup is a
/// `BTreeMap` rather than a `HashMap` for the same reason one layer down — the
/// iteration order of the one is a property of the data and of the other a
/// property of a seed — even though on this path only lookups happen.
///
/// [`bindings`](Self::bindings) yields file order, which is the order an author
/// reads and the order the error messages count in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// Every binding, in file order.
    bindings: Vec<Binding>,
    /// Which binding a control resolves to, as an index into `bindings`.
    by_control: BTreeMap<Control, usize>,
}

impl Mapping {
    /// Every binding, in the order the file wrote them.
    #[must_use]
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// The binding for `control`, if the file bound it.
    #[must_use]
    pub fn binding(&self, control: Control) -> Option<&Binding> {
        self.by_control
            .get(&control)
            .map(|index| &self.bindings[*index])
    }

    /// Turns one tick's worth of device events into the input events they mean.
    ///
    /// Pure and total: the same mapping and the same slice produce the same
    /// vector, on every platform and in every run. It reads no clock, holds no
    /// state between calls and cannot fail — see the type's documentation for
    /// the two load-time rules that pay for the last of those.
    ///
    /// A device event whose control the file does not bind produces nothing.
    /// That is not an error and must not become one: a mapping is a statement
    /// about the controls a game uses, not about the controls a keyboard has.
    ///
    /// # Examples
    ///
    /// ```
    /// use narvo_input::{Control, DeviceEvent, Mapping};
    ///
    /// let mapping = narvo_input::from_str(
    ///     r#"Mapping(bindings: [
    ///         (control: KeyW, action: "thrust", emit: OnPressAndRelease(press: 1, release: 0)),
    ///         (control: Digit3, action: "select", emit: OnPress(3)),
    ///     ])"#,
    /// )?;
    ///
    /// let events = mapping.map(&[
    ///     DeviceEvent::press(Control::KeyW),
    ///     DeviceEvent::press(Control::Digit3),
    ///     DeviceEvent::release(Control::Digit3),
    ///     DeviceEvent::release(Control::KeyW),
    ///     DeviceEvent::press(Control::Escape),
    /// ]);
    ///
    /// let seen: Vec<(&str, i64)> = events.iter().map(|e| (e.action(), e.value())).collect();
    /// assert_eq!(seen, vec![("thrust", 1), ("select", 3), ("thrust", 0)]);
    /// # Ok::<(), narvo_input::InputError>(())
    /// ```
    #[must_use]
    pub fn map(&self, stream: &[DeviceEvent]) -> Vec<InputEvent> {
        let mut events = Vec::new();

        for device in stream {
            let Some(binding) = self.binding(device.control()) else {
                continue;
            };

            let value = match (binding.emit, device.edge()) {
                (Emit::OnPress(press), Edge::Press) => press,
                (Emit::OnPress(_), Edge::Release) => continue,
                (Emit::OnPressAndRelease { press, .. }, Edge::Press) => press,
                (Emit::OnPressAndRelease { release, .. }, Edge::Release) => release,
            };

            events.push(InputEvent::from_checked(binding.action.clone(), value));
        }

        events
    }
}

/// Reads a mapping from the text of a mapping file.
///
/// # The file
///
/// ```ron
/// Mapping(
///     bindings: [
///         (control: KeyW, action: "thrust", emit: OnPressAndRelease(press: 1, release: 0)),
///         (control: KeyS, action: "thrust", emit: OnPressAndRelease(press: -1, release: 0)),
///         (control: Digit1, action: "select", emit: OnPress(1)),
///     ],
/// )
/// ```
///
/// # Errors
///
/// [`InputError::Syntax`] for anything the parser rejects — malformed RON, a
/// field a binding does not have, a control that is not in the vocabulary, a
/// missing field — carrying `ron`'s own line, column and description.
///
/// [`InputError::InvalidActionName`] for an action name that is not an
/// identifier, and [`InputError::DuplicateBinding`] for a control bound twice,
/// both naming the binding by its position in the list.
///
/// The two are checked in that order, and each stops at the first offender
/// rather than collecting. A mapping file is short and its mistakes are
/// independent; collecting them into a report is what `narvo-scene`'s
/// `validate` does for scenes, and there is no consumer for one here yet.
pub fn from_str(text: &str) -> Result<Mapping, InputError> {
    let file: MappingFile = ron::from_str(text).map_err(|error| InputError::Syntax {
        line: error.span.start.line,
        column: error.span.start.col,
        message: error.code.to_string(),
    })?;

    for (index, binding) in file.bindings.iter().enumerate() {
        if !InputEvent::is_valid_action(&binding.action) {
            return Err(InputError::InvalidActionName {
                binding: Some(index),
                name: binding.action.clone(),
            });
        }
    }

    let mut by_control = BTreeMap::new();
    for (index, binding) in file.bindings.iter().enumerate() {
        if let Some(first) = by_control.insert(binding.control, index) {
            return Err(InputError::DuplicateBinding {
                control: format!("{:?}", binding.control),
                first,
                second: index,
            });
        }
    }

    Ok(Mapping {
        bindings: file.bindings,
        by_control,
    })
}

/// Reads a mapping file from disk.
///
/// A thin hull over [`from_str`], the way `narvo-scene`'s `from_file` is over
/// its own: the format is text and the reading of it is not this crate's
/// subject.
///
/// # Errors
///
/// [`InputError::Io`] naming the path, plus everything [`from_str`] returns.
pub fn from_file(path: &Path) -> Result<Mapping, InputError> {
    let text = std::fs::read_to_string(path).map_err(|source| InputError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    from_str(&text)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Emit, from_str};
    use crate::control::{Control, DeviceEvent};
    use crate::error::InputError;

    /// The mapping every test that does not care about the file works from.
    const DEMO: &str = r#"Mapping(
    bindings: [
        (control: KeyW, action: "thrust", emit: OnPressAndRelease(press: 1, release: 0)),
        (control: KeyS, action: "thrust", emit: OnPressAndRelease(press: -1, release: 0)),
        (control: Digit1, action: "select", emit: OnPress(1)),
    ],
)"#;

    #[test]
    fn a_file_loads_into_the_bindings_it_names() {
        let mapping = from_str(DEMO).expect("the demo mapping is valid");

        assert_eq!(mapping.bindings().len(), 3);

        let first = &mapping.bindings()[0];
        assert_eq!(first.control(), Control::KeyW);
        assert_eq!(first.action(), "thrust");
        assert_eq!(
            first.emit(),
            Emit::OnPressAndRelease {
                press: 1,
                release: 0
            }
        );

        assert_eq!(mapping.bindings()[2].emit(), Emit::OnPress(1));
    }

    #[test]
    fn bindings_come_back_in_file_order_not_in_control_order() {
        // `Digit1` sorts before `KeyS` in the lookup, and after it in the file.
        // The accessor promises the file's order, which is the one the error
        // messages count in.
        let mapping = from_str(DEMO).expect("the demo mapping is valid");

        let controls: Vec<Control> = mapping
            .bindings()
            .iter()
            .map(super::Binding::control)
            .collect();
        assert_eq!(
            controls,
            vec![Control::KeyW, Control::KeyS, Control::Digit1]
        );
    }

    #[test]
    fn an_unbound_control_produces_nothing_and_is_not_an_error() {
        let mapping = from_str(DEMO).expect("the demo mapping is valid");

        assert!(
            mapping
                .map(&[DeviceEvent::press(Control::Escape)])
                .is_empty()
        );
        assert!(mapping.binding(Control::Escape).is_none());
    }

    #[test]
    fn on_press_is_silent_on_the_up_edge_and_on_press_and_release_is_not() {
        let mapping = from_str(DEMO).expect("the demo mapping is valid");

        let momentary = mapping.map(&[
            DeviceEvent::press(Control::Digit1),
            DeviceEvent::release(Control::Digit1),
        ]);
        assert_eq!(momentary.len(), 1, "{momentary:?}");
        assert_eq!((momentary[0].action(), momentary[0].value()), ("select", 1));

        let held = mapping.map(&[
            DeviceEvent::press(Control::KeyW),
            DeviceEvent::release(Control::KeyW),
        ]);
        assert_eq!(
            held.iter()
                .map(|event| (event.action(), event.value()))
                .collect::<Vec<_>>(),
            vec![("thrust", 1), ("thrust", 0)]
        );
    }

    #[test]
    fn the_output_order_is_the_input_order_and_nothing_else() {
        // The ordering guarantee, stated as a test: two controls that sort one
        // way in the lookup are emitted the other way round when that is how
        // they were pressed. If the lookup's order could reach the output, this
        // is where it would show.
        let mapping = from_str(DEMO).expect("the demo mapping is valid");

        let forwards = mapping.map(&[
            DeviceEvent::press(Control::KeyW),
            DeviceEvent::press(Control::Digit1),
        ]);
        let backwards = mapping.map(&[
            DeviceEvent::press(Control::Digit1),
            DeviceEvent::press(Control::KeyW),
        ]);

        assert_eq!(
            forwards
                .iter()
                .map(super::InputEvent::action)
                .collect::<Vec<_>>(),
            vec!["thrust", "select"]
        );
        assert_eq!(
            backwards
                .iter()
                .map(super::InputEvent::action)
                .collect::<Vec<_>>(),
            vec!["select", "thrust"]
        );
    }

    #[test]
    fn mapping_the_same_stream_twice_gives_the_same_events() {
        let mapping = from_str(DEMO).expect("the demo mapping is valid");
        let stream = [
            DeviceEvent::press(Control::KeyW),
            DeviceEvent::press(Control::KeyS),
            DeviceEvent::press(Control::Digit1),
            DeviceEvent::release(Control::KeyS),
            DeviceEvent::release(Control::KeyW),
        ];

        assert_eq!(mapping.map(&stream), mapping.map(&stream));

        // ... and so does the same file loaded a second time. Determinism is a
        // property of the pair, not of one loaded instance.
        let reloaded = from_str(DEMO).expect("the demo mapping is valid");
        assert_eq!(mapping, reloaded);
        assert_eq!(mapping.map(&stream), reloaded.map(&stream));
    }

    #[test]
    fn two_bindings_may_share_an_action_and_stay_separate_controls() {
        // `KeyW` and `KeyS` both send `thrust`. That is the ordinary case of one
        // action with two directions, and nothing about it is a duplicate.
        let mapping = from_str(DEMO).expect("the demo mapping is valid");

        let events = mapping.map(&[
            DeviceEvent::press(Control::KeyW),
            DeviceEvent::press(Control::KeyS),
        ]);
        assert_eq!(
            events
                .iter()
                .map(|event| (event.action(), event.value()))
                .collect::<Vec<_>>(),
            vec![("thrust", 1), ("thrust", -1)]
        );
    }

    #[test]
    fn a_control_bound_twice_names_both_bindings() {
        let error = from_str(
            r#"Mapping(bindings: [
                (control: KeyW, action: "thrust", emit: OnPress(1)),
                (control: Digit1, action: "select", emit: OnPress(1)),
                (control: KeyW, action: "brake", emit: OnPress(1)),
            ])"#,
        )
        .expect_err("a control is bound once");

        match &error {
            InputError::DuplicateBinding {
                control,
                first,
                second,
            } => {
                assert_eq!(control, "KeyW");
                assert_eq!((*first, *second), (0, 2));
            }
            other => panic!("expected a duplicate binding, got {other:?}"),
        }

        assert_eq!(
            error.to_string(),
            "binding 0 and binding 2 both bind KeyW; a control is bound once, or the events one \
             press produces would be ordered by a container rather than by the file. Give the \
             second one a different control, or delete it"
        );
    }

    #[test]
    fn an_action_name_that_no_recording_could_hold_is_refused_at_load() {
        let error = from_str(
            r#"Mapping(bindings: [
                (control: KeyW, action: "thrust", emit: OnPress(1)),
                (control: KeyS, action: "two words", emit: OnPress(1)),
            ])"#,
        )
        .expect_err("an action name is an identifier");

        match &error {
            InputError::InvalidActionName { binding, name } => {
                assert_eq!(*binding, Some(1));
                assert_eq!(name, "two words");
            }
            other => panic!("expected an invalid action name, got {other:?}"),
        }

        assert_eq!(
            error.to_string(),
            "binding 1 names the action \"two words\", which is not a usable action name; an \
             action name is a non-empty run of ASCII letters, digits, `_`, `-` and `.`, so that \
             a recording can hold it on one line without a quoting rule"
        );
    }

    #[test]
    fn an_empty_action_name_is_refused_too() {
        let error =
            from_str(r#"Mapping(bindings: [(control: KeyW, action: "", emit: OnPress(1))])"#)
                .expect_err("an action name is non-empty");

        assert!(
            matches!(
                error,
                InputError::InvalidActionName {
                    binding: Some(0),
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn a_field_a_binding_does_not_have_is_refused_where_it_stands() {
        let error = from_str(
            "Mapping(\n    bindings: [\n        (control: KeyW, action: \"go\", emit: \
             OnPress(1), repeat: true),\n    ],\n)",
        )
        .expect_err("a binding has three fields");

        match &error {
            InputError::Syntax {
                line,
                column,
                message,
            } => {
                // Hand-counted: line 3 is the binding, and `repeat` starts at
                // character 57 of it — eight of indent, then the fifty-six
                // characters up to and including the space after the comma.
                // Pinned rather than read off the failure, so that a position
                // going quietly missing is a red test rather than a message
                // that got one clause shorter.
                assert_eq!((*line, *column), (3, 57));
                assert!(message.contains("repeat"), "{message}");
                assert!(message.contains("Binding"), "{message}");
                assert!(message.contains("control"), "{message}");
            }
            other => panic!("expected a syntax error, got {other:?}"),
        }

        assert!(
            error
                .to_string()
                .starts_with("3:57: the mapping is not valid RON: "),
            "{error}"
        );
    }

    #[test]
    fn a_control_outside_the_vocabulary_is_refused_and_the_message_lists_it() {
        let error = from_str(
            "Mapping(\n    bindings: [\n        (control: KeyQ, action: \"go\", emit: \
             OnPress(1)),\n    ],\n)",
        )
        .expect_err("the vocabulary is closed");

        match &error {
            InputError::Syntax {
                line,
                column,
                message,
            } => {
                assert_eq!((*line, *column), (3, 19));
                assert!(message.contains("KeyQ"), "{message}");
                assert!(message.contains("KeyW"), "{message}");
            }
            other => panic!("expected a syntax error, got {other:?}"),
        }
    }

    #[test]
    fn a_file_that_is_not_ron_says_where_it_stopped() {
        let error = from_str("Mapping(\n    bindings: [\n        (control: KeyW action: \"go\")\n")
            .expect_err("a comma is missing");

        match &error {
            InputError::Syntax { line, column, .. } => assert_eq!((*line, *column), (3, 24)),
            other => panic!("expected a syntax error, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_field_names_the_field_and_the_binding_type() {
        let error = from_str("Mapping(\n    bindings: [\n        (control: KeyW),\n    ],\n)")
            .expect_err("a binding has three fields");

        match &error {
            InputError::Syntax { message, .. } => {
                assert!(message.contains("action"), "{message}");
                assert!(message.contains("Binding"), "{message}");
            }
            other => panic!("expected a syntax error, got {other:?}"),
        }
    }

    #[test]
    fn a_field_the_mapping_itself_does_not_have_is_refused() {
        let error = from_str("Mapping(bindings: [], version: 2)").expect_err("there is no version");

        match &error {
            InputError::Syntax { message, .. } => {
                assert!(message.contains("version"), "{message}");
                assert!(message.contains("Mapping"), "{message}");
                assert!(message.contains("bindings"), "{message}");
            }
            other => panic!("expected a syntax error, got {other:?}"),
        }
    }

    #[test]
    fn a_mapping_with_no_bindings_loads_and_maps_nothing() {
        // Not an error: a file that binds nothing is a file that says a game
        // takes no input, and refusing it would be this crate having an opinion
        // about a game.
        let mapping = from_str("Mapping(bindings: [])").expect("an empty mapping is valid");

        assert!(mapping.bindings().is_empty());
        assert!(mapping.map(&[DeviceEvent::press(Control::KeyW)]).is_empty());
    }

    #[test]
    fn a_file_that_is_not_there_names_the_path() {
        let missing = Path::new("mappings").join("no-such-mapping.ron");
        let error = super::from_file(&missing).expect_err("the file does not exist");

        match &error {
            InputError::Io { path, .. } => assert_eq!(path, &missing),
            other => panic!("expected an io error, got {other:?}"),
        }

        assert!(
            error
                .to_string()
                .starts_with(&missing.display().to_string()),
            "{error}"
        );
    }
}
