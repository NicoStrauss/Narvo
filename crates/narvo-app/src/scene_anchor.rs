//! The scene identity anchor: which file a recording was made against, and the
//! refusal when it is no longer that file.
//!
//! Read ADR-0019 before changing anything here. The division of labour it
//! records is the whole point and neither half subsumes the other:
//!
//! - **The anchor checks file identity, before tick 0.** It answers "is this the
//!   scene the recording was made against", and it can answer it before a single
//!   tick has run — which is when the answer is useful, because a replay against
//!   the wrong initial state is not a replay of anything.
//! - **The state hash checks semantics, from tick 0 on** (ADR-0008). It answers
//!   "did the two runs compute the same thing", and it cannot answer the first
//!   question: two different scene files can produce the same state at tick 0
//!   (a reordered file, a changed comment), and a recording replayed against a
//!   *differently* initialised world diverges later with nothing to say why.
//!
//! # Hash and load come from one read
//!
//! [`Anchor::read`] returns the anchor **and the text**, from a single
//! `read_to_string`. That is deliberate: hashing the file and then opening it
//! again to load it would leave a window in which the file changes, and the
//! world would be built from bytes the anchor never saw. One read closes it.

use std::fmt;
use std::path::{Path, PathBuf};

use narvo_core::sha256::{hex, sha256};

/// Which scene a run was constituted from.
///
/// # The path is stored in a normal form, and that is load bearing
///
/// Relative, with forward slashes, always — on every platform. The determinism
/// suite compares recordings **byte for byte between Windows and Linux**, so a
/// recording carrying `crates\…` on one and `crates/…` on the other would fail
/// that comparison for a reason that has nothing to do with the simulation. The
/// normal form is what keeps the artifact portable, and the cross-platform job
/// is where a mistake in it would surface.
///
/// An absolute path is refused rather than converted. Converting would need a
/// base to be relative *to*, and the only honest one is the working directory a
/// run happened to start in — which is not a property of the recording and would
/// travel wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    path: String,
    digest: String,
}

impl Anchor {
    /// Reads the scene at `path`, returning its anchor and its text.
    ///
    /// # Errors
    ///
    /// [`AnchorError::Missing`] when the file cannot be read, and
    /// [`AnchorError::AbsolutePath`] when the path could not be put into the
    /// normal form a recording needs.
    pub fn read(path: &Path) -> Result<(Self, String), AnchorError> {
        let normalized = normalize(path)?;

        let text = std::fs::read_to_string(path).map_err(|source| AnchorError::Missing {
            path: path.to_path_buf(),
            source,
        })?;

        // Hashed from the same bytes the caller is about to load. See the note
        // on one read, above.
        let digest = hex(&sha256(text.as_bytes()));

        Ok((
            Self {
                path: normalized,
                digest,
            },
            text,
        ))
    }

    /// Rebuilds an anchor from what a recording holds.
    #[must_use]
    pub fn from_parts(path: String, digest: String) -> Self {
        Self { path, digest }
    }

    /// The scene's path, in the normal form.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The scene's SHA-256, lower-case hex.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Checks the file this anchor names, and hands back its text.
    ///
    /// This is the refusal point of ADR-0019, and it runs **before the first
    /// tick**: the world is built from the text returned here, so a mismatch
    /// stops the run before any state exists to be compared.
    ///
    /// # Errors
    ///
    /// [`AnchorError::Missing`] when the scene is not where the recording says,
    /// and [`AnchorError::Mismatch`] when it is there and is a different file.
    pub fn verify(&self) -> Result<String, AnchorError> {
        let path = PathBuf::from(&self.path);

        let text = std::fs::read_to_string(&path)
            .map_err(|source| AnchorError::Missing { path, source })?;

        self.check(&text)?;

        Ok(text)
    }

    /// The comparison itself, against text already in hand.
    ///
    /// Split out from [`verify`](Self::verify) so that the part with a decision
    /// in it can be tested without a file system, and so that a caller which
    /// already holds the text — a hot reload watching a buffer, M6 over an IPC
    /// socket — can ask the question without a second read.
    ///
    /// # Errors
    ///
    /// [`AnchorError::Mismatch`], naming both digests.
    pub fn check(&self, text: &str) -> Result<(), AnchorError> {
        let found = hex(&sha256(text.as_bytes()));

        if found == self.digest {
            return Ok(());
        }

        Err(AnchorError::Mismatch {
            path: self.path.clone(),
            expected: self.digest.clone(),
            found,
        })
    }
}

/// Puts a path into the form a recording carries.
///
/// Separators become forward slashes; anything absolute is refused.
fn normalize(path: &Path) -> Result<String, AnchorError> {
    if path.is_absolute() {
        return Err(AnchorError::AbsolutePath {
            path: path.to_path_buf(),
        });
    }

    let text = path.to_string_lossy().replace('\\', "/");

    if text.is_empty() {
        return Err(AnchorError::AbsolutePath {
            path: path.to_path_buf(),
        });
    }

    Ok(text)
}

/// What can go wrong establishing or checking a scene anchor.
#[derive(Debug)]
pub enum AnchorError {
    /// The scene file is not where it is expected.
    Missing {
        /// The path that was tried.
        path: PathBuf,
        /// The operating system's own error.
        source: std::io::Error,
    },
    /// The scene file is there, and is a different file.
    Mismatch {
        /// The scene's path, as the recording holds it.
        path: String,
        /// What the recording says the scene hashed to.
        expected: String,
        /// What it hashes to now.
        found: String,
    },
    /// A scene path could not be put into the recording's normal form.
    AbsolutePath {
        /// The path that was given.
        path: PathBuf,
    },
}

impl fmt::Display for AnchorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { path, source } => write!(
                f,
                "the scene this recording was made against is not at {}: {source}. A replay \
                 constitutes its world from that file, so without it there is nothing to \
                 replay against; restore the file, or run from the directory the recording \
                 was made in",
                path.display()
            ),
            Self::Mismatch {
                path,
                expected,
                found,
            } => write!(
                f,
                "the scene at {path} is not the one this recording was made against.\n  \
                 expected sha256 {expected}\n  \
                 found    sha256 {found}\n\
                 The recording was made against a different version of this file, so replaying \
                 it would start from a world it never saw and diverge for a reason no state \
                 hash could explain. Restore that version of the scene, or record again \
                 against this one."
            ),
            Self::AbsolutePath { path } => write!(
                f,
                "{} is an absolute path, and a recording carries its scene path relative and \
                 with forward slashes so that the file means the same thing on another machine \
                 and another platform. Give the scene as a path relative to the working \
                 directory",
                path.display()
            ),
        }
    }
}

impl std::error::Error for AnchorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Missing { source, .. } => Some(source),
            Self::Mismatch { .. } | Self::AbsolutePath { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Anchor, AnchorError, normalize};
    use narvo_core::sha256::{hex, sha256};
    use std::path::{Path, PathBuf};

    #[test]
    fn a_windows_separator_becomes_a_forward_slash() {
        assert_eq!(
            normalize(Path::new("crates\\narvo-app\\scenes\\case.ron")).expect("relative"),
            "crates/narvo-app/scenes/case.ron"
        );
        assert_eq!(
            normalize(Path::new("crates/narvo-app/scenes/case.ron")).expect("relative"),
            "crates/narvo-app/scenes/case.ron"
        );
    }

    /// The property the cross-platform comparison rests on: whichever separator
    /// a platform hands in, the recording carries one form.
    ///
    /// This is the assertion that would have caught a path written verbatim.
    /// The CI determinism job would catch it too, one platform pair later and
    /// with a much longer diagnosis; this catches it here.
    #[test]
    fn both_platforms_normal_forms_agree() {
        let windows = normalize(Path::new("a\\b\\c.ron")).expect("relative");
        let unix = normalize(Path::new("a/b/c.ron")).expect("relative");

        assert_eq!(windows, unix);
        assert!(!windows.contains('\\'));
    }

    #[test]
    fn an_absolute_path_is_refused_by_name() {
        let absolute = if cfg!(windows) {
            PathBuf::from("C:\\scenes\\case.ron")
        } else {
            PathBuf::from("/scenes/case.ron")
        };

        let error = normalize(&absolute).expect_err("absolute paths do not travel");
        assert!(matches!(error, AnchorError::AbsolutePath { .. }));
        assert!(error.to_string().contains("relative"), "{error}");
    }

    /// The everyday green case: unchanged text checks out.
    #[test]
    fn text_that_has_not_changed_checks_out() {
        let text = "Scene(entities: [])
";
        let anchor =
            Anchor::from_parts("scenes/case.ron".to_owned(), hex(&sha256(text.as_bytes())));

        assert!(anchor.check(text).is_ok());
    }

    /// One changed byte - a comment, which changes nothing about the world -
    /// is a mismatch that names both digests.
    ///
    /// The comment is the point. A scene's *meaning* is unchanged, so no state
    /// hash could tell the two apart at tick 0; the anchor is about file
    /// identity and says so.
    #[test]
    fn a_changed_comment_is_a_mismatch_that_names_both_hashes() {
        let before = "Scene(entities: [])
";
        let after = "// a note that changes no world
Scene(entities: [])
";
        let anchor = Anchor::from_parts(
            "scenes/case.ron".to_owned(),
            hex(&sha256(before.as_bytes())),
        );

        let error = anchor.check(after).expect_err("a different file");
        match &error {
            AnchorError::Mismatch {
                path,
                expected,
                found,
            } => {
                assert_eq!(path, "scenes/case.ron");
                assert_eq!(expected, anchor.digest());
                assert_eq!(found, &hex(&sha256(after.as_bytes())));
                assert_ne!(found, expected);
            }
            other => panic!("expected a mismatch, got {other:?}"),
        }

        let message = error.to_string();
        assert!(message.contains("scenes/case.ron"), "{message}");
        assert!(message.contains("expected sha256"), "{message}");
        assert!(message.contains("found    sha256"), "{message}");
        assert!(message.contains("record again"), "{message}");
    }

    /// A missing scene is its own case, with its own message.
    #[test]
    fn a_missing_scene_is_its_own_case() {
        let anchor = Anchor::from_parts("nowhere/at/all.ron".to_owned(), "0".repeat(64));

        let error = anchor.verify().expect_err("there is no such file");
        assert!(matches!(error, AnchorError::Missing { .. }));

        let message = error.to_string();
        assert!(message.contains("nowhere/at/all.ron"), "{message}");
        assert!(message.contains("restore the file"), "{message}");
        assert!(
            !message.contains("sha256"),
            "a missing file is not a mismatch and must not read like one: {message}"
        );
    }

    /// Reading an anchor hands back the very text it hashed.
    ///
    /// The one-read property, checked against the file this crate is compiled
    /// from - a file that certainly exists, needs no scratch directory, and is
    /// reached by a relative path because that is all an anchor accepts.
    #[test]
    fn reading_an_anchor_hands_back_the_text_it_hashed() {
        let (anchor, text) =
            Anchor::read(Path::new("src/scene_anchor.rs")).expect("this file is being compiled");

        assert_eq!(anchor.path(), "src/scene_anchor.rs");
        assert_eq!(anchor.digest(), hex(&sha256(text.as_bytes())));
        assert!(
            text.contains("the scene identity anchor"),
            "wrong file read"
        );
        assert!(anchor.check(&text).is_ok());
    }

    #[test]
    fn a_scene_that_is_not_there_when_read_is_a_missing_scene() {
        let error = Anchor::read(Path::new("no/such/scene.ron")).expect_err("not there");

        assert!(matches!(error, AnchorError::Missing { .. }));
    }
}
