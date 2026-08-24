//! Where this runner looks for a scene's assets.
//!
//! # This is policy, and that is why it is still here
//!
//! `assets/` beside the scene file. A scene at `levels/one.ron` loads
//! `levels/assets/`. Nothing in the scene format names it, which is deliberate:
//! ADR-0018 keeps asset paths out of the format, and a convention needs no
//! syntax, no validation rule and no escaping question.
//!
//! **The mechanism left in M6b.2a.** Packing a directory, turning it into a
//! texture and a region table, and refusing a world that names a region no file
//! carries are [`narvo_view2d::load_for`]'s, because they are the same for every
//! game and `narvo-app` is a binary that nothing outside can call. What stayed
//! is the sentence above: *which* directory. A game that wants its assets
//! somewhere else writes its own two lines and calls the same loader.
//!
//! ADR-0041's amendment records the split and why it runs here.

use std::path::{Path, PathBuf};

/// The directory a scene's assets live in, relative to the scene file.
pub const ASSETS_DIRECTORY: &str = "assets";

/// The assets directory belonging to `scene`.
///
/// A bare filename's parent is the *empty* path rather than `None`, so
/// `one.ron` gives `assets` and not `./assets`. Both name the same directory;
/// the shorter one is what `Path` produces and what an error message therefore
/// shows, and saying so here is cheaper than a reader wondering which it is.
#[must_use]
pub fn directory_for(scene: &Path) -> PathBuf {
    scene
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(ASSETS_DIRECTORY)
}

#[cfg(test)]
mod tests {
    use super::directory_for;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_assets_directory_sits_beside_the_scene() {
        assert_eq!(
            directory_for(Path::new("levels/one.ron")),
            PathBuf::from("levels").join("assets")
        );
        // A bare filename's parent is the empty path, so this is `assets` and
        // not `./assets` — the same directory, spelled the way `Path` spells it.
        assert_eq!(directory_for(Path::new("one.ron")), PathBuf::from("assets"));
    }
}
