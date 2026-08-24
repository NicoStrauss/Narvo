//! Input crossing from outside the simulation into it.
//!
//! Read ADR-0012 before changing anything here, in particular the reason an
//! input event names an *action* rather than a key: the engine's simulation must
//! not know what a keyboard is, and a recording must stay meaningful when the
//! key bindings change under it.
//!
//! The type lived in `narvo-ecs` until M5.1 and moved here unchanged in every
//! respect that is observable — same two fields, same names, same serialized
//! form, so the state hash of a world holding a buffer of them is the same
//! before and after. What changed is the crate it is in and the error its
//! constructor returns; ADR-0025 records why.

use serde::{Deserialize, Serialize};

use crate::error::InputError;

/// One thing that arrived from outside the simulation during one tick.
///
/// Deliberately two fields and no more. An input event is a *named action with a
/// magnitude*, which is the smallest shape that covers what a simulation
/// actually consumes:
///
/// | Source | Action | Value |
/// | --- | --- | --- |
/// | key down / up | `"jump"` | `1` / `0` |
/// | button, held | `"fire"` | `1` |
/// | axis, analogue | `"steer"` | a magnitude in whatever fixed-point unit the consumer agreed on |
/// | discrete choice | `"select"` | which one |
///
/// # Why an action and not a key
///
/// A recording that stored `KeyW` would stop meaning what it meant the moment a
/// key binding changed, and it would tie the deterministic core to a window
/// library it must not depend on. Naming the action instead keeps the
/// *simulation's* vocabulary in the recording, so device mapping stays a layer
/// above and can be rewritten without invalidating a single stored repro.
///
/// [`Mapping`](crate::Mapping) is that layer, and it is in this crate rather
/// than above it: a mapping's whole output is a list of these, and the rule that
/// an action name is an identifier is checked at the one place the two meet.
/// What M5 still has to decide is how an analogue axis is spelled in `value`;
/// see ADR-0012, which names it as open rather than guessing.
///
/// # Determinism
///
/// The type carries no clock, no device handle and no floating point. `value` is
/// an `i64` so that an event survives a text round trip exactly — a decimal
/// integer reads back as the bits it was written from, which a float only does
/// with care, and a recording is worth nothing if replaying it drifts.
///
/// # Examples
///
/// ```
/// use narvo_input::InputEvent;
///
/// let event = InputEvent::new("thrust", 2)?;
/// assert_eq!((event.action(), event.value()), ("thrust", 2));
///
/// // An action name is an identifier, so that every serialization of it -
/// // including a line-based recording - can hold it without quoting.
/// assert!(InputEvent::new("two words", 1).is_err());
/// # Ok::<(), narvo_input::InputError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputEvent {
    /// What happened, by name.
    action: String,
    /// How much of it. `1` and `0` stand in for pressed and released.
    value: i64,
}

impl InputEvent {
    /// Builds an event for `action` with magnitude `value`.
    ///
    /// # Errors
    ///
    /// [`InputError::InvalidActionName`] if `action` is not an identifier in the
    /// sense of [`is_valid_action`](Self::is_valid_action). Checked here, at the
    /// only place outside this crate an event can be constructed, so that no
    /// later stage has to ask whether it can represent what it was handed.
    pub fn new(action: &str, value: i64) -> Result<Self, InputError> {
        if !Self::is_valid_action(action) {
            return Err(InputError::InvalidActionName {
                binding: None,
                name: action.to_owned(),
            });
        }

        Ok(Self::from_checked(action.to_owned(), value))
    }

    /// Builds an event from a name a caller in this crate has already checked.
    ///
    /// The one reason it exists: [`Mapping`](crate::Mapping) validates every
    /// action name while the file is being loaded, so mapping a device event
    /// cannot fail, and a `map` returning a `Result` nobody can trigger would be
    /// a `Result` every caller unwraps. Keeping the constructor crate-private is
    /// what makes that safe to say — the invariant is enforced by the module
    /// boundary rather than by a comment asking callers to be careful.
    pub(crate) fn from_checked(action: String, value: i64) -> Self {
        debug_assert!(
            Self::is_valid_action(&action),
            "from_checked was given an unchecked name: {action:?}"
        );

        Self { action, value }
    }

    /// The action's name.
    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    /// The magnitude that came with it.
    #[must_use]
    pub fn value(&self) -> i64 {
        self.value
    }

    /// Whether `name` may be used as an action name.
    ///
    /// The rule: a non-empty run of ASCII letters, digits, `_`, `-` and `.`.
    ///
    /// It is narrow on purpose. An action name is written into a recording,
    /// which is a line-based text file a person is expected to edit by hand, and
    /// the alternative to a narrow charset is a quoting rule — one more thing to
    /// get wrong in a file whose entire value is that it is obvious. Excluding
    /// whitespace makes a line splittable; excluding `#` keeps the comment
    /// marker unambiguous; excluding control characters keeps a corrupted file
    /// from looking valid.
    #[must_use]
    pub fn is_valid_action(name: &str) -> bool {
        !name.is_empty()
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-.".contains(character))
    }
}

#[cfg(test)]
mod tests {
    use super::InputEvent;
    use crate::InputError;

    #[test]
    fn an_event_reports_what_it_was_built_from() {
        let event = InputEvent::new("thrust", -3).expect("a plain name is valid");

        assert_eq!(event.action(), "thrust");
        assert_eq!(event.value(), -3);
    }

    #[test]
    fn identifier_names_are_accepted() {
        for name in ["a", "jump", "move_left", "camera-zoom", "player.1", "x9"] {
            assert!(
                InputEvent::new(name, 0).is_ok(),
                "{name} should be a valid action name"
            );
        }
    }

    #[test]
    fn a_name_that_would_break_a_line_based_recording_is_rejected() {
        // Each of these would either split a line, start a comment or make a
        // corrupted file parse as something valid.
        for name in [
            "",
            "two words",
            "tab\there",
            "new\nline",
            "#comment",
            "semi;colon",
            "quote\"d",
        ] {
            let error = InputEvent::new(name, 0)
                .expect_err("{name} must not be accepted as an action name");

            match error {
                InputError::InvalidActionName {
                    binding: None,
                    name: reported,
                } => assert_eq!(reported, name),
                other => panic!("expected an invalid action name error, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_error_names_what_was_given_and_says_what_is_allowed() {
        let error = InputEvent::new("no spaces please", 0).expect_err("spaces are not allowed");
        let message = error.to_string();

        assert_eq!(
            message,
            "\"no spaces please\" is not a usable action name; an action name is a non-empty run \
             of ASCII letters, digits, `_`, `-` and `.`, so that a recording can hold it on one \
             line without a quoting rule"
        );
    }

    /// The set the code accepts is exactly the set every message names.
    ///
    /// **The agreement M5.7 found missing.** The charset is written once as code
    /// (`is_valid_action`) and restated in prose in five places — this module's
    /// doc comment and its error, `mapping`'s binding error, `narvo-app`'s
    /// recording error and its hitrect error. Each of those sentences is pinned
    /// by a wording test, and the examples above pin a handful of accepted and
    /// rejected names, but nothing compared the *whole* accepted set against the
    /// sentence. A charset that quietly grew a character would have left every
    /// one of those tests green while five messages started lying.
    ///
    /// The expected set is built here from the sentence's own words — letters,
    /// digits, `_`, `-`, `.` — rather than by calling the function under test,
    /// which is what keeps this a comparison of two independent statements.
    #[test]
    fn the_accepted_characters_are_exactly_the_ones_the_message_names() {
        let named: Vec<char> = ('a'..='z')
            .chain('A'..='Z')
            .chain('0'..='9')
            .chain(['_', '-', '.'])
            .collect();

        // Every code point a byte can be, so nothing is accepted quietly.
        for byte in 0_u8..=127 {
            let character = byte as char;
            let name: String = character.to_string();
            assert_eq!(
                InputEvent::is_valid_action(&name),
                named.contains(&character),
                "{character:?} (0x{byte:02x}) is accepted by one statement and not the other"
            );
        }

        // And the emptiness rule, which is the other half of the sentence.
        assert!(!InputEvent::is_valid_action(""), "a name must be non-empty");

        // Non-ASCII stays out, which "ASCII letters" says and `is_alphanumeric`
        // would not have: `char::is_alphanumeric` is true for 'ä' and 'あ'.
        for character in ['ä', 'あ', 'Ω', '💥'] {
            let name: String = character.to_string();
            assert!(
                !InputEvent::is_valid_action(&name),
                "{character:?} is not an ASCII letter"
            );
        }
    }

    #[test]
    fn an_event_survives_a_serde_round_trip() {
        let event = InputEvent::new("steer", i64::MIN).expect("valid");

        let text = ron::to_string(&event).expect("an event always serializes");
        assert_eq!(text, format!("(action:\"steer\",value:{})", i64::MIN));

        let restored: InputEvent = ron::from_str(&text).expect("what was written reads back");
        assert_eq!(restored, event);
    }

    #[test]
    fn the_serialized_form_did_not_move_with_the_type() {
        // The reason the M5.1 move is invisible to every state hash: a world's
        // canonical dump renders a component through `serde`, and `serde`
        // renders this one from its field names alone. Neither the crate nor the
        // module path appears, so a buffer of these serializes byte for byte the
        // way it did while the type lived in `narvo-ecs`.
        let event = InputEvent::new("thrust", 1).expect("valid");

        assert_eq!(
            ron::to_string(&event).expect("an event always serializes"),
            "(action:\"thrust\",value:1)"
        );
    }
}
