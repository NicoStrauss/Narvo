//! The seam: an entity's `Transform` decides where a sprite is drawn.
//!
//! This lives in `narvo-app` because it is the only crate that sees both
//! sides. `narvo-render2d` depends on `narvo-core` and deliberately not on
//! `narvo-ecs` — the renderer takes the five scalars of a `Transform` rather
//! than the component itself, which is the boundary ADR-0014 makes possible by
//! keeping registered components free of foreign types. Nothing in the render
//! crate can therefore test this, and nothing in the ECS crate can either.
//!
//! What the render crate's own probes establish is that a `SpritePlacement`
//! lands where the convention says. What this establishes is the one step
//! before it: that the placement is read off an entity rather than invented.

#![cfg(feature = "render")]

use narvo_ecs::{Transform, World};
use narvo_render2d::{OffscreenTarget, RenderError, SpritePlacement};

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

const TOP_LEFT: [u8; 4] = narvo_testkit::QUADRANT_TOP_LEFT;
const TOP_RIGHT: [u8; 4] = narvo_testkit::QUADRANT_TOP_RIGHT;
const BOTTOM_LEFT: [u8; 4] = narvo_testkit::QUADRANT_BOTTOM_LEFT;
const BOTTOM_RIGHT: [u8; 4] = narvo_testkit::QUADRANT_BOTTOM_RIGHT;
const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

fn target_or_skip(width: u32, height: u32) -> Option<OffscreenTarget> {
    match OffscreenTarget::new(width, height) {
        Ok(target) => {
            println!("adapter in use: {}", target.adapter_summary());
            Some(target)
        }
        Err(error @ RenderError::NoAdapter { .. }) => {
            assert!(
                std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a failure \
                 rather than a skip: {error}"
            );
            println!("{SKIP_MARKER}: {error}");
            None
        }
        Err(other) => panic!("creating the offscreen target failed: {other}"),
    }
}

/// Reads the one transform in `world` and unpacks it for the renderer.
///
/// The unpacking is the conversion ADR-0014 puts at the point of use: the
/// component holds bare scalars precisely so this is five field reads and not a
/// dependency.
fn placement_of_the_only_entity(world: &World) -> SpritePlacement {
    let mut transforms = world.query::<&Transform>();
    let mut found = transforms.iter();

    let (_id, transform) = found.next().expect("the world holds one transform");
    assert!(
        found.next().is_none(),
        "more than one sprite is batching, which is a later milestone"
    );

    SpritePlacement {
        x: transform.x,
        y: transform.y,
        ..SpritePlacement::new(transform.scale_x, transform.scale_y).turned(transform.rotation)
    }
}

/// Builds a one-entity world carrying `transform`.
fn world_with(transform: Transform) -> World {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, transform)
        .expect("the entity was just spawned");
    world
}

/// Two entities differing only in their `Transform` render differently.
///
/// The prediction, from the convention and written before this first ran: a
/// sprite of 32 x 32 world units centred on the origin covers the middle half
/// of a 64 x 64 target, so pixel (20, 20) sits in its upper-left quadrant and
/// is red. Moved 16 world units to the right — half the sprite's width — that
/// pixel is no longer covered and shows the background.
#[test]
fn the_entity_transform_decides_where_the_sprite_lands() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let texture = narvo_testkit::quadrant_texture(8);

    let centred = world_with(Transform {
        scale_x: 32.0,
        scale_y: 32.0,
        ..Transform::IDENTITY
    });
    let moved = world_with(Transform {
        x: 16.0,
        scale_x: 32.0,
        scale_y: 32.0,
        ..Transform::IDENTITY
    });

    let at_origin = target
        .render_sprite(&texture, placement_of_the_only_entity(&centred))
        .expect("drawing must succeed");
    let shifted = target
        .render_sprite(&texture, placement_of_the_only_entity(&moved))
        .expect("drawing must succeed");

    println!("centred (20, 20): {:?}", at_origin.pixel(20, 20));
    println!("moved   (20, 20): {:?}", shifted.pixel(20, 20));

    assert_eq!(
        at_origin.pixel(20, 20),
        Some(TOP_LEFT),
        "an entity at the origin puts the texture's top-left quadrant here"
    );
    assert_eq!(
        shifted.pixel(20, 20),
        Some(BACKGROUND),
        "the same texture with a different Transform must produce a different \
         image; if these two agree, the transform is not reaching the renderer"
    );

    // And the moved sprite is still whole, just elsewhere: its own upper-left
    // quadrant has travelled 16 pixels to the right.
    assert_eq!(shifted.pixel(36, 20), Some(TOP_LEFT));
    assert_eq!(shifted.pixel(60, 44), Some(BOTTOM_RIGHT));
}

/// A quarter turn counter-clockwise moves the corners the way the convention says.
///
/// Prediction, again before running: with world y up and x right, `+π/2` sends
/// what was the sprite's upper-left quadrant to its lower left. In framebuffer
/// terms — y down — the red quadrant therefore moves from the upper left of the
/// sprite to its lower left, and the green one from the upper right to the
/// upper left.
#[test]
fn a_positive_rotation_turns_the_sprite_counter_clockwise() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let world = world_with(Transform {
        rotation: core::f32::consts::FRAC_PI_2,
        scale_x: 32.0,
        scale_y: 32.0,
        ..Transform::IDENTITY
    });

    let rendered = target
        .render_sprite(
            &narvo_testkit::quadrant_texture(8),
            placement_of_the_only_entity(&world),
        )
        .expect("drawing must succeed");

    for (x, y) in [(20_u32, 20_u32), (44, 20), (20, 44), (44, 44)] {
        println!("({x}, {y}): {:?}", rendered.pixel(x, y));
    }

    assert_eq!(
        rendered.pixel(20, 20),
        Some(TOP_RIGHT),
        "after +90 degrees the green quadrant occupies the upper left"
    );
    assert_eq!(rendered.pixel(20, 44), Some(TOP_LEFT));
    assert_eq!(rendered.pixel(44, 44), Some(BOTTOM_LEFT));
    assert_eq!(rendered.pixel(44, 20), Some(BOTTOM_RIGHT));
}
