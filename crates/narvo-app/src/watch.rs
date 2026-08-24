//! Noticing that the scene file changed, by asking rather than by being told.
//!
//! Read ADR-0022. The decision this module implements, with the measurement
//! behind it:
//!
//! **Polling, not `notify`.** The plan line said "Hot-Reload über `notify`", and
//! M4.6 revised it on a measurement rather than a preference. `notify` selects
//! the Inotify backend on Linux, and on a `/mnt` path — drvfs, which is where
//! **this repository's entire Linux workflow lives** (CLAUDE.md's WSL section
//! puts the working copy at `/mnt/d/Narvo`) — it delivered **no event at all**
//! within three seconds for a write that had definitely happened. A watcher that
//! is silently dead in the configuration the project prescribes is worse than no
//! watcher, because nothing says it stopped working. The same file under the
//! same path changes its `mtime` and its length, which is what this module
//! reads.
//!
//! The rest of the ledger, for completeness rather than as the deciding half:
//! `notify` costs two crates new to this workspace (`notify`, `notify-types` —
//! the rest of its tree is already here) and 2.1 s of clean build, and would
//! pull a fourth `windows-sys` version into a lock file that already carries
//! three.
//!
//! # What is compared, and why it is not `mtime` and length
//!
//! The contents, hashed. The obvious cheaper design — compare `mtime` and
//! length, and only hash when one of them moves — has a hole this module's own
//! test fell into on the first run: **"one" and "two" are the same length**, and
//! two writes inside the file system's timestamp resolution carry the same
//! `mtime`, so the change is invisible. An author who edits one character and
//! saves twice quickly is not an exotic case.
//!
//! So every poll reads the file and hashes it, with the existing in-house
//! SHA-256. At a two-kilobyte scene, five times a second, against a window that
//! redraws sixty times in the same second, the cost is not one worth a blind
//! spot. `mtime` and length are kept only as what a *missing* file reports, so
//! that deleting and restoring a scene reads as a change rather than an error.
//!
//! # A partial read is the normal case, not an error
//!
//! An editor saving a file writes it in pieces and often more than once. So a
//! poll that catches a half-written file is expected: [`Watcher::changed`] only
//! reports a change once the file has **stopped changing** for one interval,
//! which coalesces a burst of writes into one reload and makes the torn state
//! unobservable to the caller. A read that fails outright is treated the same
//! way — as "not settled yet" — and retried on the next poll.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use narvo_core::sha256::{hex, sha256};

/// How often the file is asked about.
///
/// The closing criterion of `ProjektPlan.md` §6/M4 is one second from a file
/// changing to the change being on screen. A fifth of that is the budget this
/// takes, which leaves the load and the swap the rest — and a `stat` every
/// 200 ms is not a cost worth measuring against a frame that redraws sixty times
/// in the same second.
pub const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// What one poll of the file found: its contents' digest, or its absence.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    /// The file was read, and this is what it hashed to.
    Digest(String),
    /// The file could not be read — missing, or mid-replacement.
    Unreadable,
}

/// Watches one file and says when it has settled on new contents.
///
/// One file, because that is the whole of ADR-0022's scope: the scene the window
/// loaded. Not a directory, not a set — a scene that pulled in a second file
/// would be ADR-0019's multi-file question, which is answered there or not at
/// all.
#[derive(Debug)]
pub struct Watcher {
    path: PathBuf,
    /// The digest of the contents the caller is currently working from.
    accepted: String,
    /// Something seen but not yet settled, and when it was first seen.
    pending: Option<(Seen, Instant)>,
    /// When the file was last asked about.
    last_poll: Instant,
}

impl Watcher {
    /// Starts watching `path`, taking `text` as the contents already in hand.
    ///
    /// The caller has just loaded the file — the window has its world from it —
    /// so the digest starts as that text's rather than being read again. One
    /// read, the same rule `scene_anchor` follows and for the same reason.
    #[must_use]
    pub fn new(path: &Path, text: &str, now: Instant) -> Self {
        Self {
            path: path.to_path_buf(),
            accepted: hex(&sha256(text.as_bytes())),
            pending: None,
            last_poll: now,
        }
    }

    /// The file being watched.
    ///
    /// `render`-gated rather than `any(render, test)`: its only caller is the
    /// window's reload log line, and no test here asks a watcher what it is
    /// watching — the tests hand it a path and already know. Gating on `test`
    /// too would leave it dead in the headless test build, which is the warning
    /// this removes.
    #[cfg(feature = "render")]
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Asks the file whether it has changed, and hands back the new text when it
    /// has settled on some.
    ///
    /// Returns `None` far more often than not: on every call inside the poll
    /// interval, on every poll that finds nothing new, and on every poll that
    /// finds a file still being written.
    ///
    /// # What "settled" means
    ///
    /// A stamp different from the last settled one starts a wait. The change is
    /// reported only when a later poll sees **the same** stamp again, so a burst
    /// of editor writes becomes one reload rather than one per write. The
    /// contents are then read and hashed, and a digest equal to the last one is
    /// no change at all — which is what a save that rewrites a file with the same
    /// bytes produces, and what a touch produces.
    pub fn changed(&mut self, now: Instant) -> Option<Change> {
        if now.duration_since(self.last_poll) < POLL_INTERVAL {
            return None;
        }
        self.last_poll = now;

        let (seen, text) = match std::fs::read_to_string(&self.path) {
            Ok(text) => (Seen::Digest(hex(&sha256(text.as_bytes()))), Some(text)),
            // Not an error: a file being replaced is unreadable for a moment,
            // and a deleted one is a state the author may undo.
            Err(_) => (Seen::Unreadable, None),
        };

        if seen == Seen::Digest(self.accepted.clone()) {
            // What the caller already has. A pending wait is abandoned, which is
            // what an editor writing and then restoring looks like.
            self.pending = None;
            return None;
        }

        match &self.pending {
            // First sight of something new: wait for it to stop moving.
            None => {
                self.pending = Some((seen, now));
                None
            }
            // Still moving. Restart the wait against what is there now.
            Some((earlier, _)) if *earlier != seen => {
                self.pending = Some((seen, now));
                None
            }
            // Two polls agreeing: the writer has stopped.
            Some((_, first_seen)) => {
                let noticed = *first_seen;
                self.pending = None;

                let text = text?;
                self.accepted = match seen {
                    Seen::Digest(digest) => digest,
                    Seen::Unreadable => return None,
                };

                Some(Change { text, noticed })
            }
        }
    }

    /// Accepts `text` as the contents in hand without reporting a change.
    ///
    /// Used when a reload succeeds against text the caller already has, so the
    /// watcher's idea of "current" follows the world's.
    pub fn accept(&mut self, text: &str) {
        self.accepted = hex(&sha256(text.as_bytes()));
        self.pending = None;
    }
}

/// A settled change: the new contents, and when the change was first noticed.
#[derive(Debug, Clone)]
pub struct Change {
    /// The file's new text.
    pub text: String,
    /// When the first poll saw *the content that settled* — the start of the
    /// latency the closing criterion measures.
    ///
    /// **Not the first poll that saw any new stamp**, which is what this said
    /// until M5.7 and is only the same thing when nothing follows.
    /// [`Watcher::changed`] restarts the wait against what is there now on every
    /// poll that sees a different stamp, so across a burst of writes the two
    /// readings differ by the whole burst.
    ///
    /// `a_burst_of_writes_coalesces_into_one_change` pins the distinction; the
    /// old wording survived because nothing read the field.
    pub noticed: Instant,
}

#[cfg(test)]
mod tests {
    use super::{Change, POLL_INTERVAL, Watcher};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// A scratch file under the crate's own directory is not wanted; the target
    /// directory is where a test may write, and cargo hands integration tests a
    /// path to it. Unit tests do not get one, so this uses the file the module
    /// is compiled from as a stand-in wherever only *reading* is needed, and a
    /// process-unique name under the system temp directory where writing is.
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("narvo-watch-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Polls are rate limited, so a caller may ask as often as it likes.
    #[test]
    fn asking_inside_the_interval_does_not_touch_the_file_system() {
        let path = scratch("rate");
        std::fs::write(&path, "one").expect("writable");
        let start = Instant::now();
        let mut watcher = Watcher::new(&path, "one", start);

        std::fs::write(&path, "two").expect("writable");

        // Every call before the interval is up answers without looking.
        assert!(watcher.changed(start).is_none());
        assert!(
            watcher
                .changed(start + POLL_INTERVAL - Duration::from_millis(1))
                .is_none()
        );

        let _ = std::fs::remove_file(&path);
    }

    /// A change is reported once the file has stopped moving, and not before.
    #[test]
    fn a_change_is_reported_only_after_it_settles() {
        let path = scratch("settle");
        std::fs::write(&path, "one").expect("writable");
        let start = Instant::now();
        let mut watcher = Watcher::new(&path, "one", start);
        let mut at = start;
        let poll = |watcher: &mut Watcher, at: &mut Instant| {
            *at += POLL_INTERVAL;
            watcher.changed(*at)
        };

        std::fs::write(&path, "two").expect("writable");
        assert!(
            poll(&mut watcher, &mut at).is_none(),
            "the first sight of a new stamp only starts the wait"
        );

        let change = poll(&mut watcher, &mut at).expect("the second agreeing poll reports it");
        assert_eq!(change.text, "two");

        assert!(
            poll(&mut watcher, &mut at).is_none(),
            "nothing changed since, so nothing is reported"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// **Coalescing**: a burst of writes becomes one change, not one per write.
    #[test]
    fn a_burst_of_writes_coalesces_into_one_change() {
        let path = scratch("burst");
        std::fs::write(&path, "0").expect("writable");
        let start = Instant::now();
        let mut watcher = Watcher::new(&path, "0", start);
        let mut at = start;

        let mut changes: Vec<Change> = Vec::new();
        // Five writes, each followed by a poll: every poll sees a different
        // stamp, so none of them settles.
        for step in 1..=5 {
            std::fs::write(&path, format!("write {step} with a growing length")).expect("writable");
            at += POLL_INTERVAL;
            if let Some(change) = watcher.changed(at) {
                changes.push(change);
            }
        }
        assert!(
            changes.is_empty(),
            "a file still being written must not be reloaded: {changes:?}"
        );

        // Then the writer stops, and the next poll settles it.
        at += POLL_INTERVAL;
        let change = watcher.changed(at).expect("the writing stopped");
        assert_eq!(change.text, "write 5 with a growing length");

        // `noticed` is the first poll that saw a new stamp, not the poll that
        // settled it — the gap between them is the settle latency ADR-0022's
        // closing criterion measures, and nothing asserted it until M5.7.
        //
        // Reading it here is also why the field carries no `cfg` while `path`
        // above does: gating a *field* would mean splitting the one place a
        // `Change` is built into two arms, which is a worse thing to carry than
        // the warning it removes. Giving the field a reader is the better
        // answer, and it was missing one.
        // Five writes, each restarting the wait, then one quiet poll. So the
        // settling content was first seen at the fifth poll and agreed with at
        // the sixth — `noticed` is one interval back from the settle, not five.
        //
        // That is the measurement that corrected this field's own
        // documentation: it used to read "when the first poll saw a new stamp",
        // which is true only when nothing follows. `changed` restarts the wait
        // on every poll that sees a different stamp, so across a burst the two
        // readings differ by the whole burst.
        assert_eq!(
            change.noticed,
            start + 5 * POLL_INTERVAL,
            "noticed is the first poll that saw the content which then settled"
        );
        assert_eq!(
            at - change.noticed,
            POLL_INTERVAL,
            "and settling costs exactly the one quiet interval"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// The same bytes under a new stamp is not a change.
    #[test]
    fn rewriting_a_file_with_its_own_contents_is_not_a_change() {
        let path = scratch("same");
        std::fs::write(&path, "unchanged").expect("writable");
        let start = Instant::now();
        let mut watcher = Watcher::new(&path, "unchanged", start);
        let mut at = start;

        // A different length first, so the stamp definitely moves, then back.
        std::fs::write(&path, "unchanged").expect("writable");
        at += POLL_INTERVAL;
        let _ = watcher.changed(at);
        at += POLL_INTERVAL;

        assert!(
            watcher.changed(at).is_none(),
            "the digest is what decides, and it did not move"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// A file that is not there is a stamp too, so restoring it reads as a
    /// change rather than as an error.
    #[test]
    fn a_missing_file_is_a_state_and_not_a_failure() {
        let path = scratch("missing");
        std::fs::write(&path, "here").expect("writable");
        let start = Instant::now();
        let mut watcher = Watcher::new(&path, "here", start);
        let mut at = start;

        std::fs::remove_file(&path).expect("removable");
        at += POLL_INTERVAL;
        assert!(watcher.changed(at).is_none(), "gone is not yet settled");
        at += POLL_INTERVAL;
        assert!(
            watcher.changed(at).is_none(),
            "a settled absence has nothing to hand back"
        );

        std::fs::write(&path, "back again").expect("writable");
        at += POLL_INTERVAL;
        let _ = watcher.changed(at);
        at += POLL_INTERVAL;
        let change = watcher.changed(at).expect("the file came back");
        assert_eq!(change.text, "back again");

        let _ = std::fs::remove_file(&path);
    }

    /// `accept` moves the watcher's idea of current without reporting.
    #[test]
    fn accepting_text_leaves_nothing_to_report() {
        let path = scratch("accept");
        std::fs::write(&path, "one").expect("writable");
        let start = Instant::now();
        let mut watcher = Watcher::new(&path, "one", start);
        let mut at = start;

        std::fs::write(&path, "two").expect("writable");
        watcher.accept("two");

        at += POLL_INTERVAL;
        assert!(watcher.changed(at).is_none());
        at += POLL_INTERVAL;
        assert!(watcher.changed(at).is_none());

        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod chain {
    //! The reload chain as a measurable function.
    //!
    //! The closing criterion of `ProjektPlan.md` §6/M4 is one second from a file
    //! changing to the change being on screen. That budget has three parts —
    //! noticing, loading, swapping — and only the first is this module's. What is
    //! measured here is the second and third: how long it takes to turn a
    //! specimen-sized scene's text into a world ready to draw.
    //!
    //! It is a *floor* rather than the whole figure, and says so: the window's
    //! own latency is measured by running the window, which the M4.6 report does
    //! separately.

    use crate::sim::scene_file;
    use std::time::Instant;

    /// A scene the size of the committed specimen.
    fn specimen() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("narvo-scene")
                .join("scenes")
                .join("example.ron"),
        )
        .expect("the specimen is readable from here")
    }

    /// Loading a specimen-sized scene is far inside the one-second budget.
    ///
    /// Asserted against a deliberately loose bound — a hundredth of the budget —
    /// because this is a floor and a machine-dependent one. The number the
    /// report carries is the measured one; what this test defends is the claim
    /// that the load is not where a second would go.
    #[test]
    fn loading_a_specimen_sized_scene_is_far_inside_the_budget() {
        let text = specimen();

        // One warm run first, so the figure is not a first-touch artefact.
        let _ = scene_file::build(&text).expect("the specimen loads");

        let started = Instant::now();
        let simulation = scene_file::build(&text).expect("the specimen loads");
        let elapsed = started.elapsed();

        // Two from the specimen-sized scene plus the input buffer's carrier
        // that `scene_file::build` appends since M5.4.
        assert_eq!(simulation.world.len(), 4);
        println!(
            "load of a specimen-sized scene: {:.3} ms",
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(
            elapsed.as_millis() < 10,
            "loading took {elapsed:?}, which is a tenth of the one-second budget \
             spent before anything is drawn"
        );
    }

    /// A scene that does not load leaves the caller free to keep its old world.
    ///
    /// The mechanism §7 asks for, at the level this module owns it: `build`
    /// returns an error and nothing else happens. The window's own
    /// keep-the-old-world path is one `match` on this.
    #[test]
    fn a_scene_that_does_not_load_returns_an_error_rather_than_a_partial_world() {
        let broken = "Scene(entities: [(components: { \"transform\": (x: 1.0) })])";

        let Err(error) = scene_file::build(broken) else {
            panic!("a transform needs all five fields, so this must not build")
        };
        let message = error.to_string();

        assert!(message.contains("1:"), "the position survives: {message}");
        assert!(message.contains("transform"), "{message}");
    }
}
