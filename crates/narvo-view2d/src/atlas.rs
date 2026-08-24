//! A directory of image files, packed into one atlas a world can name regions in.
//!
//! Read ADR-0024. This is the third thing in this crate that sees both a `World`
//! and a renderer, and it is where a region *name* in a component becomes a
//! rectangle in a texture — the same division of labour ADR-0015 gives
//! [`placements_of`](crate::placements_of) and ADR-0041 gives this crate.
//! `narvo-assets` knows nothing about `TextureRegion`, and `narvo-ecs` knows
//! nothing about atlases; both facts are what keeps this the only place the two
//! meet.
//!
//! # Where the directory comes from is **not** decided here
//!
//! [`load_for`] takes a directory and has no opinion about which one. The
//! convention this repository's own runner uses — `assets/` beside the scene
//! file, so a scene at `levels/one.ron` loads `levels/assets/` — stayed behind in
//! `narvo-app` as `ASSETS_DIRECTORY` and `directory_for`, because it is policy
//! and a game may choose its own. ADR-0018 keeps asset paths out of the scene
//! format either way, and a convention needs no syntax, no validation rule and no
//! escaping question.
//!
//! That split is the whole of M6b.2a. What moved is the mechanism and, with it,
//! [`AssetsError`] — a sentence naming the region a scene asked for and the ones
//! that exist. M6b.2's survey measured an external consumer rebuilding this
//! function's body from public parts, and the rebuild worked — every part of it
//! is public. What could not be rebuilt was the sentence, because a binary crate
//! has no lib target to hand it out from.
//!
//! **What a consumer gets instead was measured rather than argued** (M6b.2a):
//! skip the resolution check, index the region table by the name a scene asked
//! for, and `BTreeMap` panics with `no entry found for key` — no region name, no
//! directory, and no list of what does exist.
//!
//! # Every file is loaded, and an unused region is legal
//!
//! The directory is packed whole. A region no entity draws costs texture space
//! and nothing else, and M4.2 decided there are exactly two warning classes —
//! adding a third for "you have an asset you are not using" would be a lint
//! about content taste rather than about correctness.
//!
//! The reverse is not legal: a name no region carries is a **load error**,
//! because it is a scene that cannot be drawn as written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use narvo_ecs::World;
use narvo_render2d::{Pixels, TextureRegion};

/// One packed atlas and the regions a world can name in it.
#[derive(Debug)]
pub struct SceneAtlas {
    /// The texture to bind.
    pub texture: Pixels,
    /// Region name to the rectangle it occupies.
    pub regions: BTreeMap<String, TextureRegion>,
}

/// Something that stopped a scene's assets from becoming an atlas.
#[derive(Debug)]
pub enum AssetsError {
    /// The files could not be read, decoded or packed.
    Source(narvo_assets::AssetError),
    /// The packed atlas is not a texture the renderer will take.
    Texture(narvo_render2d::RenderError),
    /// A scene names a region the atlas does not carry.
    UnknownRegion {
        /// The name the scene asked for.
        wanted: String,
        /// The names that exist, in name order.
        known: Vec<String>,
        /// Where the assets came from.
        directory: PathBuf,
    },
}

impl std::fmt::Display for AssetsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(error) => write!(f, "{error}"),
            Self::Texture(error) => write!(f, "the packed atlas is not a usable texture: {error}"),
            Self::UnknownRegion {
                wanted,
                known,
                directory,
            } => {
                let known = if known.is_empty() {
                    "there are none".to_owned()
                } else {
                    format!(
                        "the known ones are {}",
                        known
                            .iter()
                            .map(|name| format!("\"{name}\""))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                write!(
                    f,
                    "a sprite asks for the region \"{wanted}\", which {} does not carry; \
                     {known}. A region is named by a file stem, so this needs a file called \
                     \"{wanted}.png\" there",
                    directory.display()
                )
            }
        }
    }
}

impl std::error::Error for AssetsError {}

impl From<narvo_assets::AssetError> for AssetsError {
    fn from(error: narvo_assets::AssetError) -> Self {
        Self::Source(error)
    }
}

/// Packs `directory` and resolves every region `world` names.
///
/// The check is done **here, before a window opens**, so an unknown name is a
/// message on the terminal rather than a sprite that silently does not draw.
///
/// # Errors
///
/// [`AssetsError::Source`] for anything the files do, [`AssetsError::Texture`]
/// if the packed atlas is not a texture the renderer takes, and
/// [`AssetsError::UnknownRegion`] naming the region a sprite asked for and the
/// ones that exist.
pub fn load_for(world: &World, directory: &Path) -> Result<SceneAtlas, AssetsError> {
    let regions = narvo_assets::regions_from_directory(directory)?;
    let atlas = narvo_assets::pack(regions).map_err(narvo_assets::AssetError::from)?;

    let texture = Pixels::from_rgba8(atlas.width(), atlas.height(), atlas.rgba().to_vec())
        .map_err(AssetsError::Texture)?;

    let regions: BTreeMap<String, TextureRegion> = atlas
        .regions()
        .map(|(name, place)| {
            (
                name.to_owned(),
                TextureRegion::from_texels(
                    place.left(),
                    place.top(),
                    place.width(),
                    place.height(),
                    &texture,
                ),
            )
        })
        .collect();

    for wanted in crate::region_names_of(world) {
        if !regions.contains_key(&wanted) {
            return Err(AssetsError::UnknownRegion {
                wanted,
                known: regions.keys().cloned().collect(),
                directory: directory.to_path_buf(),
            });
        }
    }

    Ok(SceneAtlas { texture, regions })
}

#[cfg(test)]
mod tests {
    use super::{AssetsError, load_for};
    use narvo_ecs::{Sprite, Transform, World};
    use std::path::{Path, PathBuf};

    fn scratch(case: &str) -> PathBuf {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("m48-assets")
            .join(case);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the scratch directory");
        directory
    }

    /// Straight-alpha RGBA8 as PNG bytes.
    fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("a header can be written");
            writer
                .write_image_data(rgba)
                .expect("the data is the size the header says");
        }
        bytes
    }

    fn world_wanting(regions: &[&str]) -> World {
        let mut world = World::new();
        for region in regions {
            let entity = world.spawn();
            world
                .insert(entity, Transform::IDENTITY)
                .expect("just spawned");
            world
                .insert(entity, Sprite::new(*region))
                .expect("just spawned");
        }
        world
    }

    #[test]
    fn a_world_resolves_every_name_it_asks_for() {
        let directory = scratch("resolves");
        std::fs::write(
            directory.join("hero.png"),
            encode(2, 2, &[255, 0, 0, 255].repeat(4)),
        )
        .expect("the file is written");
        std::fs::write(directory.join("coin.png"), encode(1, 1, &[0, 255, 0, 255]))
            .expect("the file is written");

        let atlas = load_for(&world_wanting(&["hero"]), &directory).expect("the assets load");

        // Both regions are in the atlas, including the one no entity draws:
        // every file is loaded, and an unused region is legal.
        assert!(atlas.regions.contains_key("hero"));
        assert!(atlas.regions.contains_key("coin"));
    }

    #[test]
    fn an_unknown_region_names_the_ones_that_exist() {
        let directory = scratch("unknown");
        std::fs::write(directory.join("coin.png"), encode(1, 1, &[0, 255, 0, 255]))
            .expect("the file is written");

        let error = load_for(&world_wanting(&["hero"]), &directory)
            .expect_err("a region nothing carries is refused");
        let message = error.to_string();

        assert!(
            message.contains("asks for the region \"hero\""),
            "{message}"
        );
        assert!(message.contains("the known ones are \"coin\""), "{message}");
        assert!(message.contains("\"hero.png\""), "{message}");
        assert!(matches!(error, AssetsError::UnknownRegion { .. }));
    }

    /// An empty directory produces the "there are none" wording rather than an
    /// empty list dressed up as a sentence.
    #[test]
    fn an_empty_directory_says_there_are_none() {
        let directory = scratch("none");

        let error = load_for(&world_wanting(&["hero"]), &directory)
            .expect_err("a region nothing carries is refused");
        let message = error.to_string();

        assert!(message.contains("there are none"), "{message}");
        assert!(!message.contains("the known ones"), "{message}");
    }

    /// A world with no sprites at all needs nothing from the directory.
    #[test]
    fn a_world_with_no_sprites_asks_for_nothing() {
        let directory = scratch("sprite-free");
        std::fs::write(directory.join("coin.png"), encode(1, 1, &[0, 255, 0, 255]))
            .expect("the file is written");

        let atlas = load_for(&World::new(), &directory).expect("nothing is asked for");
        assert_eq!(atlas.regions.len(), 1);
    }
}
