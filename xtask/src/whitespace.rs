//! The guard for collapsed line continuations in string literals.
//!
//! # The damage class
//!
//! A Rust string literal that spans source lines is written with a backslash
//! before the newline; the lexer then swallows the newline *and the following
//! indentation*, so the literal reads as one flowing sentence:
//!
//! ```text
//! "a message that is too long for one line, so it \
//!  continues here"
//! ```
//!
//! Lose the backslash — a `sed`, a `re.sub`, a shell heredoc, a hand edit — and
//! the two lines join with the indentation still in them, giving a message with
//! fifteen spaces in the middle of a sentence. It compiles, it passes every
//! `contains` assertion, and it ships. `ProjektPlan.md` §12 records **fourteen**
//! instances of exactly this across M4.2 to M4.7, found by eye each time.
//!
//! §9.2 already carries the prevention rule — string literals are edited in an
//! editor, never through an escape-processing tool. This is the detection half,
//! because the rule has been broken four times *after* being written down.
//!
//! # The detection rule, and its deliberate limits
//!
//! Inside an ordinary (non-raw) string literal, on one source line: a run of
//! [`RUN`] or more spaces with a non-space on both sides.
//!
//! Every clause of that is load-bearing, and the reasons are the false positives
//! the alternatives produce in *this* tree:
//!
//! - **Non-raw only.** Raw strings hold scene RON, expected multi-line output
//!   and ASCII tables, where indentation is the content. A rule that read them
//!   would fire on the specimen.
//! - **On one source line.** A correct continuation has its indentation on the
//!   *next* line, after a backslash. Only the collapsed form puts a long run of
//!   spaces inline.
//! - **Not directly after `\n`.** `"...:\n    cargo build"` is deliberate
//!   formatting of printed output, and there are several.
//! - **Non-space on both sides.** A run at the very start or end of a literal is
//!   padding, not a collapsed sentence.
//! - **Six, not four.** Four-space runs are used for column alignment inside
//!   messages — `"found    sha256"` in `scene_anchor.rs` is one, and it is
//!   correct. Six is above every intentional alignment in this tree and below
//!   every collapsed continuation, all of which come from indentation of eight
//!   spaces or more.
//! - **On a line longer than [`MAX_WIDTH`], and this is the clause that makes
//!   the rule usable.** Without it the guard reports twenty places in this tree
//!   and thirteen of them are correct: aligned label columns like
//!   `"line      {}"`, a frame-timing table header, probe output with lined-up
//!   values. With it, seven — and all seven are real.
//!
//!   The threshold is **derived rather than fitted**. `rustfmt.toml` sets no
//!   overrides, so `max_width` is rustfmt's default of 100, and
//!   `cargo fmt --all --check` is step five of the verification set. A line over
//!   100 characters is therefore one rustfmt *could not* break — and the only
//!   thing it cannot break is a long string literal. A collapsed continuation is
//!   long by construction, because it merged two source lines into one;
//!   deliberate alignment lives on short lines, because rustfmt keeps them
//!   short. The measurement is in `target/reports/M4_9.md`: every genuine site
//!   is 109 to 171 characters, every false positive 26 to 77.
//!
//! **What it cannot see, stated rather than discovered later:** a collapsed
//! continuation whose indentation was shallow enough to leave fewer than [`RUN`]
//! spaces, a doubled space (the M4.2 instance was a *single* extra space and no
//! threshold catches that without drowning in noise), and anything inside a raw
//! string. It is a net with a named mesh size, not a proof.

use std::path::{Path, PathBuf};

/// Spaces in a row that make a run suspicious.
///
/// See the module documentation for why six and not four.
const RUN: usize = 6;

/// rustfmt's `max_width`, which this repository leaves at its default.
///
/// A line longer than this is one `cargo fmt --all --check` accepted only
/// because it could not be broken — which, for a line carrying a string
/// literal, means the literal itself is that long. See the module
/// documentation for why that is the discriminator.
const MAX_WIDTH: usize = 100;

/// One place the guard objects to.
pub(crate) struct Finding {
    /// The file, relative to the workspace root.
    pub(crate) path: PathBuf,
    /// One-based line number.
    pub(crate) line: usize,
    /// The offending source line, trimmed of leading indentation.
    pub(crate) text: String,
}

/// Every `.rs` file under `root`, excluding anything cargo generated.
///
/// A hand-rolled walk rather than a crate: this binary's manifest keeps its
/// dependencies deliberately empty, and the whole job is "recurse, skip
/// `target`".
fn rust_files(root: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();

    for path in paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            // `target` is cargo's, `.git` is git's, and neither holds source
            // this repository wrote.
            if name == "target" || name == ".git" {
                continue;
            }
            rust_files(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// Scans one file's text for collapsed continuations.
///
/// The scanner tracks just enough Rust to tell the three string forms apart:
/// ordinary `"..."` with `\` escapes, raw `r"..."` / `r#"..."#` with any number
/// of hashes, and `//` line comments. It is not a lexer and does not need to be
/// — the property is "does this source line contain a long run of spaces inside
/// an ordinary string", and everything it gets wrong it gets wrong by staying
/// quiet.
pub(crate) fn scan(text: &str) -> Vec<(usize, String)> {
    let mut findings = Vec::new();
    // `Some(n)` while inside a raw string closed by `"` followed by n hashes.
    let mut raw_hashes: Option<usize> = None;

    for (index, line) in text.lines().enumerate() {
        let bytes: Vec<char> = line.chars().collect();
        // Short lines are ones rustfmt was free to lay out, so any run in them
        // is deliberate. Raw-string state still has to be tracked through them,
        // which is why this is a flag rather than a `continue`.
        let long_enough = bytes.len() > MAX_WIDTH;
        let mut position = 0;
        let mut in_string = false;
        let mut flagged = false;
        // Where the current ordinary string started, so a run can be tested for
        // having content on both sides *within the literal*.
        let mut run: usize = 0;
        let mut previous_was_newline_escape = false;
        // Whether the current literal has had any content before the run being
        // measured. A run with nothing before it is padding at the start of a
        // literal, not a sentence that was joined.
        let mut seen_content = false;

        while position < bytes.len() {
            let ch = bytes[position];

            // Inside a raw string: look only for its terminator.
            if let Some(hashes) = raw_hashes {
                if ch == '"' && bytes[position + 1..].iter().take(hashes).all(|c| *c == '#') {
                    raw_hashes = None;
                    position += 1 + hashes;
                    continue;
                }
                position += 1;
                continue;
            }

            if !in_string {
                // A line comment ends the line for our purposes.
                if ch == '/' && bytes.get(position + 1) == Some(&'/') {
                    break;
                }

                // A raw string opener: `r`, some hashes, a quote.
                if ch == 'r' {
                    let mut hashes = 0;
                    while bytes.get(position + 1 + hashes) == Some(&'#') {
                        hashes += 1;
                    }
                    if bytes.get(position + 1 + hashes) == Some(&'"') {
                        raw_hashes = Some(hashes);
                        position += 2 + hashes;
                        continue;
                    }
                }

                if ch == '"' {
                    in_string = true;
                    run = 0;
                    previous_was_newline_escape = false;
                    seen_content = false;
                }
                position += 1;
                continue;
            }

            // Inside an ordinary string.
            match ch {
                '\\' => {
                    // An escape. `\n` is the one that matters: indentation
                    // directly after it is deliberate formatting.
                    previous_was_newline_escape = bytes.get(position + 1) == Some(&'n');
                    run = 0;
                    seen_content = true;
                    position += 2;
                    continue;
                }
                '"' => {
                    in_string = false;
                    position += 1;
                    continue;
                }
                ' ' => {
                    run += 1;
                    position += 1;
                    continue;
                }
                _ => {
                    if long_enough
                        && run >= RUN
                        && seen_content
                        && !previous_was_newline_escape
                        && !flagged
                    {
                        // A run this long, with content before it and content
                        // after it — this character.
                        findings.push((index + 1, line.trim_start().to_owned()));
                        flagged = true;
                    }
                    run = 0;
                    seen_content = true;
                    previous_was_newline_escape = false;
                    position += 1;
                    continue;
                }
            }
        }
    }

    findings
}

/// Scans the workspace and returns everything the rule objects to.
pub(crate) fn findings(root: &Path) -> Vec<Finding> {
    let mut files = Vec::new();
    rust_files(root, &mut files);

    let mut findings = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line, text) in scan(&text) {
            findings.push(Finding {
                path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                line,
                text,
            });
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::{MAX_WIDTH, RUN, scan};

    /// A source line of `content`, padded past [`MAX_WIDTH`] with a trailing
    /// comment so the length clause is satisfied by construction.
    ///
    /// Every fixture goes through this. Writing long lines directly would put
    /// the very defect this module hunts into its own tests — and it did, on the
    /// first calibration run, where the guard reported one of its own fixtures.
    fn long_line(content: &str) -> String {
        let mut line = content.to_owned();
        if line.len() <= MAX_WIDTH {
            line.push_str("  // ");
            while line.len() <= MAX_WIDTH {
                line.push('x');
            }
        }
        line
    }

    #[test]
    fn a_collapsed_continuation_is_found() {
        let padding = " ".repeat(RUN + 8);
        let source = long_line(&format!(
            "let message = \"a sentence that was continued{padding}and lost its backslash\";"
        ));
        assert_eq!(scan(&source).len(), 1, "{:?}", scan(&source));
    }

    /// A correct continuation puts its indentation on the *next* line, after a
    /// backslash, so neither line carries a run inside a literal.
    ///
    /// Assembled from pieces rather than written out, and that is not
    /// fastidiousness: the first version of this fixture was a single long
    /// literal, and **the guard reported it** — because a shell heredoc used to
    /// write this file ate one of its backslashes, turning the fixture into the
    /// very defect it was meant to describe. §9.2's rule, demonstrated on the
    /// guard for §9.2's rule.
    #[test]
    fn a_correct_continuation_is_not_found() {
        let backslash = '\\';
        let indent = " ".repeat(RUN + 8);
        let source =
            format!("let message = \"continued {backslash}\n{indent}properly and at length\";");
        assert!(scan(&source).is_empty(), "{:?}", scan(&source));
    }

    #[test]
    fn indentation_after_a_newline_escape_is_left_alone() {
        let padding = " ".repeat(RUN + 4);
        let source = long_line(&format!(
            "println!(\"build it first:\n\n{padding}cargo build --workspace\n\");"
        ));
        assert!(scan(&source).is_empty(), "{:?}", scan(&source));
    }

    /// Raw strings hold scene RON and expected output, where indentation is the
    /// content.
    #[test]
    fn a_raw_string_is_left_alone() {
        let padding = " ".repeat(RUN + 6);
        let source = long_line(&format!(
            "let scene = r#\"Scene(entities: [(name:{padding}\"x\")])\"#;"
        ));
        assert!(scan(&source).is_empty(), "{:?}", scan(&source));
    }

    /// A raw string spanning lines keeps its state across them.
    #[test]
    fn a_multi_line_raw_string_stays_raw_to_its_terminator() {
        let padding = " ".repeat(RUN + 8);
        let inner = long_line(&format!("    entities: [(name:{padding}\"x\")],"));
        let source = format!("let scene = r#\"Scene(\n{inner}\n)\"#;");
        assert!(scan(&source).is_empty(), "{:?}", scan(&source));
    }

    #[test]
    fn a_comment_is_not_a_string() {
        let padding = " ".repeat(RUN + 8);
        let source = long_line(&format!("// a comment with{padding}a long run in it"));
        assert!(scan(&source).is_empty(), "{:?}", scan(&source));
    }

    /// The length clause: the same run on a short line is deliberate alignment.
    ///
    /// This is the discriminator the module documentation derives, asserted
    /// rather than described — the identical content is flagged when long and
    /// left alone when short.
    #[test]
    fn the_same_run_is_flagged_only_on_a_line_rustfmt_could_not_break() {
        let padding = " ".repeat(RUN + 8);
        let content = format!("let x = \"label{padding}value and some more words here\";");
        assert!(content.len() <= MAX_WIDTH, "the short form must be short");

        assert!(scan(&content).is_empty(), "{:?}", scan(&content));
        assert_eq!(scan(&long_line(&content)).len(), 1);
    }

    #[test]
    fn alignment_shorter_than_the_threshold_is_left_alone() {
        let padding = " ".repeat(RUN - 1);
        let source = long_line(&format!(
            "assert!(message.contains(\"found{padding}sha256 and rather more text\"));"
        ));
        assert!(scan(&source).is_empty(), "{:?}", scan(&source));
    }

    #[test]
    fn a_run_at_the_edge_of_a_literal_is_padding_rather_than_a_sentence() {
        let padding = " ".repeat(RUN + 4);
        let leading = long_line(&format!("let x = \"{padding}text\";"));
        let trailing = long_line(&format!("let x = \"text{padding}\";"));
        assert!(scan(&leading).is_empty(), "{:?}", scan(&leading));
        assert!(scan(&trailing).is_empty(), "{:?}", scan(&trailing));
    }

    /// One finding per line, however many runs it carries.
    ///
    /// A collapsed continuation of three source lines leaves two runs on one
    /// line, and counting it twice would make the total a poor measure of how
    /// many places are wrong.
    #[test]
    fn a_line_is_reported_once() {
        let padding = " ".repeat(RUN + 2);
        let source = long_line(&format!("let x = \"one{padding}two{padding}three\";"));
        assert_eq!(scan(&source).len(), 1);
    }

    #[test]
    fn the_reported_line_number_is_one_based() {
        let padding = " ".repeat(RUN + 8);
        let second = long_line(&format!("    let x = \"a{padding}b and some more words\";"));
        let source = format!("fn f() {{\n{second}\n}}");
        assert_eq!(scan(&source)[0].0, 2);
    }

    /// An escaped quote does not end the literal.
    #[test]
    fn an_escaped_quote_does_not_end_the_literal() {
        let padding = " ".repeat(RUN + 8);
        let source = long_line(&format!(
            "let x = \"a quoted \\\"word\\\" and then{padding}a continuation\";"
        ));
        assert_eq!(scan(&source).len(), 1, "{:?}", scan(&source));
    }
}
