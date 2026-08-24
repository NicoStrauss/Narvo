//! What can go wrong reading a mapping file, and how it says so.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;

/// Something that stopped a mapping from being read.
///
/// # These messages are the product
///
/// A mapping file is written by hand, by a person or by an agent, and the
/// message is the whole of the feedback either of them gets. So every variant
/// here answers three questions — **where**, **what** and **what to do** — and
/// every one of them is asserted on in a test, wording included, because a
/// message nobody checks drifts into uselessness one refactor at a time. That is
/// the standard `narvo-scene` set in M4.2 and this crate is held to it.
///
/// # Where is spelled two ways
///
/// [`Syntax`](Self::Syntax) carries a line and a column, because `ron` knows
/// them: everything the parser itself rejects — malformed RON, a field the
/// binding does not have, a control name that is not in the vocabulary, a
/// missing field — arrives with a position already computed, and throwing it
/// away would be a loss.
///
/// This crate's *own* two rules — the action-name charset and the one-binding-
/// per-control rule — are checked after parsing, on the list `ron` produced, and
/// they name the **binding's position in the `bindings` list**, counting from
/// zero. `narvo-scene` spells the same distinction the same way and for the
/// same reason (see its `SceneError`), with one honest difference worth stating:
/// there, an entity's index is also its slot in the loaded world, so it locates
/// the mistake in the file *and* in the world. Here it locates it in the file
/// alone. It is the position in a list the author wrote, which is enough to find
/// it and never wrong.
///
/// A line and a column for those two is reachable — `ron` attaches a position to
/// a `serde::de::Error::custom` raised from inside a `Deserialize` impl, which
/// was measured rather than assumed (M5.1 report). It is not done here because
/// carrying a *structured* variant back out of a deserializer needs a side
/// channel, and a variant a test can match on is worth more than a column.
#[derive(Debug)]
#[non_exhaustive]
pub enum InputError {
    /// The file is not a mapping `ron` can read.
    ///
    /// Covers everything the parser rejects on its own: malformed RON, an
    /// unknown field, an unknown control, a missing field, a value of the wrong
    /// type. Position and cause both come from `ron`'s own spanned error,
    /// unchanged — and its wording already names what it found and lists what it
    /// expected instead, which is the message this crate would otherwise have
    /// written by hand and kept in step with the vocabulary.
    Syntax {
        /// The line the parser stopped on, counting from one.
        line: usize,
        /// The column the parser stopped on, counting from one, in characters.
        column: usize,
        /// `ron`'s own description of what it expected.
        message: String,
    },
    /// An action name is not an identifier.
    ///
    /// The constraint is ADR-0012's, and it is enforced here rather than
    /// inherited: an action name has to survive a line-based recording without a
    /// quoting rule. Nothing in *this* crate records anything — a mapping is
    /// never written to a recording, and D8 is open — but a name that could not
    /// be recorded would be a name no later recording could carry, and finding
    /// that out at load time costs one comparison.
    InvalidActionName {
        /// The binding it was written on, counting from zero, when it came from
        /// a mapping file. `None` when the name was handed to
        /// [`InputEvent::new`](crate::InputEvent::new) directly, where there is
        /// no list for a position to refer to.
        binding: Option<usize>,
        /// The name that was rejected.
        name: String,
    },
    /// Two bindings claim the same control.
    ///
    /// A hard error rather than a precedence rule. Both alternatives are worse:
    /// last-wins makes a file's meaning depend on line order in a way nothing
    /// announces, and firing both makes one control produce two events whose
    /// order is a property of a container rather than of the file.
    DuplicateBinding {
        /// The contested control, spelled as the file spells it.
        control: String,
        /// Position of the binding that claimed it first, counting from zero.
        first: usize,
        /// Position of the binding that was turned away.
        second: usize,
    },
    /// A mapping file could not be read.
    Io {
        /// The path that was being read.
        path: PathBuf,
        /// The operating system's own error.
        source: std::io::Error,
    },
}

/// The charset rule, written once.
///
/// Both action-name messages end in it, and the wording is the same sentence
/// `narvo-ecs` carried before the type moved here. Two copies of a rule are two
/// chances to describe it differently.
const CHARSET: &str = "an action name is a non-empty run of ASCII letters, digits, `_`, `-` and \
                       `.`, so that a recording can hold it on one line without a quoting rule";

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax {
                line,
                column,
                message,
            } => write!(
                f,
                "{line}:{column}: the mapping is not valid RON: {message}"
            ),
            Self::InvalidActionName {
                binding: Some(binding),
                name,
            } => write!(
                f,
                "binding {binding} names the action \"{name}\", which is not a usable action \
                 name; {CHARSET}"
            ),
            Self::InvalidActionName {
                binding: None,
                name,
            } => write!(f, "\"{name}\" is not a usable action name; {CHARSET}"),
            Self::DuplicateBinding {
                control,
                first,
                second,
            } => write!(
                f,
                "binding {first} and binding {second} both bind {control}; a control is bound \
                 once, or the events one press produces would be ordered by a container rather \
                 than by the file. Give the second one a different control, or delete it"
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl Error for InputError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Syntax { .. }
            | Self::InvalidActionName { .. }
            | Self::DuplicateBinding { .. } => None,
            Self::Io { source, .. } => Some(source),
        }
    }
}
