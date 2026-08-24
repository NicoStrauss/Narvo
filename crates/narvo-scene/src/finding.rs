//! What [`validate`](crate::validate) reports: one finding per thing found, and
//! the report that holds them.

use std::fmt;

use crate::error::SceneError;
use crate::location::Location;

/// How much a finding matters.
///
/// Two levels and no more. A third would need a rule for what belongs in it, and
/// `ProjektPlan.md` §2's no-stockpiling rule applies to lint classes as much as
/// to features: an "info" nobody acts on is noise that trains a reader to skim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The scene will not load. [`from_str`](crate::from_str) would return this.
    Error,
    /// The scene loads, and something about it is probably not intended.
    Warning,
}

impl fmt::Display for Severity {
    /// Lower case, because it is rendered inside a `file:line:col: severity:`
    /// line where every other part is lower case too.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
        })
    }
}

/// One thing found in a scene.
///
/// The three parts are what a reader needs and nothing else: how much it
/// matters, where it is, and what it is. They are kept **apart** rather than
/// pre-rendered into one string, because the two consumers want different
/// arrangements — a CLI puts the position in front, and M6's introspection would
/// want it as data — and a joined string can only be taken apart again by
/// parsing it.
#[derive(Debug, Clone)]
pub struct Finding {
    severity: Severity,
    location: Option<Location>,
    message: String,
}

impl Finding {
    /// A warning at `location`.
    pub(crate) fn warning(location: Option<Location>, message: String) -> Self {
        Self {
            severity: Severity::Warning,
            location,
            message,
        }
    }

    /// The error, as a finding.
    ///
    /// The message is the error's own, without its position, because the
    /// position travels in its own field. Rendering `4:9:` into both would put
    /// it on the line twice.
    pub(crate) fn from_error(error: &SceneError) -> Self {
        Self {
            severity: Severity::Error,
            location: error.location(),
            message: error.message(),
        }
    }

    /// How much this finding matters.
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// Where in the file it is, when it could be located.
    #[must_use]
    pub fn location(&self) -> Option<Location> {
        self.location
    }

    /// What it is, without the position.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Finding {
    /// `line:column: severity: message`, or `severity: message` unlocated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(location) = self.location {
            write!(f, "{location}: ")?;
        }
        write!(f, "{}: {}", self.severity, self.message)
    }
}

/// Everything one pass over a scene found.
///
/// Ordered as the walk found it: entities in file order, and within an entity,
/// names before components before references. That is the order an author reads
/// the file in, which is the useful one — sorting by severity would separate two
/// findings about the same line.
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    pub(crate) findings: Vec<Finding>,
}

impl ValidationReport {
    /// Everything found, in file order.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// How many findings are errors.
    #[must_use]
    pub fn errors(&self) -> usize {
        self.count(Severity::Error)
    }

    /// How many findings are warnings.
    #[must_use]
    pub fn warnings(&self) -> usize {
        self.count(Severity::Warning)
    }

    /// Whether the scene would load.
    ///
    /// Warnings do not make a scene invalid — that is what separates them from
    /// errors — so this asks about errors alone. A caller that wants warnings to
    /// count says so itself; the validation CLI's `--deny-warnings` is exactly
    /// that caller, and it does the arithmetic where the policy lives.
    #[must_use]
    pub fn loads(&self) -> bool {
        self.errors() == 0
    }

    /// Whether nothing at all was found.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.severity == severity)
            .count()
    }
}
