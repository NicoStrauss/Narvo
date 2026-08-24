//! A burst is seen, and it is seen changing.
//!
//! **The oracle §6/M6b asks for, in both directions.** M6b.5 measured that a
//! moved state hash is *not* evidence that anything visible moved — a hold
//! counter running while the picture is frozen moves the canonical dump and
//! proves nothing. What a burst does is visible, so the oracle here reads
//! pixels:
//!
//! - the same world at the same age renders to the **same bytes**;
//! - the same world at **different ages renders to different bytes** — without
//!   that half, a burst that does nothing passes the first;
//! - a **replay reproduces the burst itself** and not merely the state it ends
//!   in, checked at four ages inside the burst rather than at its last one;
//! - a **spent** burst renders exactly what a world with no burst renders, which
//!   is what keeps a finished effect out of every later frame.
//!
//! # Gated on `render`, and that is not decoration
//!
//! `#![cfg(feature = "render")]`. M4.8 shipped a defect by leaving it off: an
//! integration test importing a render-gated crate compiled locally and broke CI
//! on both platforms. Steps seven to nine of the verification set are what would
//! catch it now, and this file is written not to need them.
//!
//! # No blessed reference, deliberately
//!
//! Every comparison here is between two renders of **this** build. Nothing is
//! committed and nothing is compared against a stored image, which is M6b.5's
//! precedent for the same question: two runs agreeing at the same tick and
//! disagreeing at different ticks is a stronger statement than one picture, and
//! it needs no human in the loop. ADR-0008's rule about stored values is the
//! reason there is no third option.

#![cfg(feature = "render")]

use narvo_ecs::{Burst, Layer, Sprite, SystemContext, Tint, Transform, World, advance_bursts};
use narvo_render2d::{OffscreenTarget, Pixels, RenderError, SpriteInstance, TextureRegion};
use narvo_view2d::regions_of;

/// The environment variable that turns a missing adapter into a failure.
///
/// The same name `blessed_scenes` uses and the same meaning: CI sets it at
/// workflow level, so a runner without an adapter is a red run rather than a
/// silent skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// What a skipped render prints, so a reader of the log can find it.
const SKIP_MARKER: &str = "skipped";

/// The canvas. Small enough to be quick, large enough that a burst travelling
/// two units a tick has somewhere to travel.
const EDGE: u32 = 128;

/// The burst under test: enough particles to fill a frame, a life long enough
/// that four sample ages sit inside it.
fn a_burst() -> Burst {
    Burst::new(48, 0x5eed_1234_abcd_9876, 40, 1.75, 1.0)
}

/// The ages the picture is sampled at. All inside the burst's life.
const SAMPLES: [u64; 4] = [1, 7, 19, 33];

/// Builds a target, or reports that this machine cannot host one.
fn target_or_skip() -> Option<OffscreenTarget> {
    match OffscreenTarget::new(EDGE, EDGE) {
        Ok(target) => Some(target),
        Err(error @ RenderError::NoAdapter { .. }) => {
            assert!(
                std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a \
                 failure rather than a skip: {error}"
            );
            println!("{SKIP_MARKER}: {error}");
            None
        }
        Err(other) => panic!("the offscreen target failed for another reason: {other}"),
    }
}

/// A four-texel atlas: one opaque white texel and three that are not sampled.
///
/// White, so the tint is what decides the colour and a dimmed particle is
/// visibly dimmer rather than merely different.
fn atlas() -> Pixels {
    Pixels::from_rgba8(
        2,
        2,
        vec![
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ],
    )
    .expect("a 2x2 atlas of sixteen bytes")
}

/// One emitter at the origin, armed, with a colour so the fade is visible.
fn world_with_a_burst() -> World {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Transform {
                x: 0.0,
                y: 0.0,
                rotation: 0.0,
                scale_x: 6.0,
                scale_y: 6.0,
            },
        )
        .expect("just spawned");
    world.insert(entity, Layer::at(0.0)).expect("just spawned");
    world
        .insert(
            entity,
            Sprite {
                region: "spark".to_owned(),
            },
        )
        .expect("just spawned");
    world
        .insert(entity, Tint::rgb(1.0, 0.75, 0.25))
        .expect("just spawned");
    world.insert(entity, a_burst()).expect("just spawned");
    world
}

/// Runs `ticks` ticks of the one system a burst needs.
fn age(world: &mut World, ticks: u64) {
    for tick in 0..ticks {
        advance_bursts(world, &SystemContext::new(tick));
    }
}

/// Draws the world through the extraction the scene-file path uses.
fn render(target: &OffscreenTarget, world: &World) -> Pixels {
    let atlas = atlas();
    let sprites: Vec<SpriteInstance> = regions_of(world)
        .into_iter()
        .map(|drawn| {
            SpriteInstance::new(drawn.placement, TextureRegion::WHOLE_TEXTURE)
                .sampled(drawn.filter)
                .tinted(drawn.tint)
        })
        .collect();

    target
        .render_sprites(&atlas, &sprites)
        .expect("a burst of forty-eight is far inside the batch limit")
}

/// The world aged to `ticks`, rendered.
fn frame_at(target: &OffscreenTarget, ticks: u64) -> Pixels {
    let mut world = world_with_a_burst();
    age(&mut world, ticks);
    render(target, &world)
}

/// The colour an untouched pixel has.
///
/// **Opaque black, not transparent black**, and getting that wrong is how this
/// file's first draft counted every pixel of the frame as drawn: `quad.rs:418`
/// clears with `wgpu::Color::BLACK`, whose alpha is one. A test asking "is the
/// alpha non-zero" therefore answers yes for a frame with nothing in it, and
/// would have passed for a burst drawn entirely off the canvas — which is
/// exactly the case it exists to catch.
const CLEARED: [u8; 4] = [0, 0, 0, 255];

/// How many pixels of the frame are not the clear colour.
///
/// A count rather than a comparison, for the one claim a comparison cannot make:
/// that there is something to see at all.
fn drawn_pixels(frame: &Pixels) -> usize {
    let mut lit = 0;
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            if frame.pixel(x, y).expect("inside the frame") != CLEARED {
                lit += 1;
            }
        }
    }
    lit
}

/// **Direction one: the same age draws the same bytes.**
#[test]
fn the_same_burst_at_the_same_age_draws_the_same_frame() {
    let Some(target) = target_or_skip() else {
        return;
    };

    for ticks in SAMPLES {
        let first = frame_at(&target, ticks);
        let second = frame_at(&target, ticks);
        assert_eq!(
            first.rgba(),
            second.rgba(),
            "two renders of age {ticks} differ"
        );
    }
}

/// **Direction two, and without it the first passes for a burst that does
/// nothing.**
///
/// Every pair of the sampled ages has to differ. Not "some pair" — every one, so
/// a burst that moves once and then freezes cannot hide inside an aggregate.
#[test]
fn a_burst_at_two_different_ages_draws_two_different_frames() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let frames: Vec<(u64, Pixels)> = SAMPLES
        .iter()
        .map(|&ticks| (ticks, frame_at(&target, ticks)))
        .collect();

    for (index, (left_age, left)) in frames.iter().enumerate() {
        for (right_age, right) in &frames[index + 1..] {
            assert_ne!(
                left.rgba(),
                right.rgba(),
                "age {left_age} and age {right_age} draw the same frame, so the \
                 burst is not moving between them"
            );
        }
    }
}

/// There is something to see, which neither comparison above can say.
///
/// Two identical empty frames satisfy the first test and two differently empty
/// ones cannot exist, so without this a burst drawn entirely outside the canvas
/// would pass both.
///
/// **The emitter alone is not enough to pass**, and that is why the threshold is
/// a comparison rather than `> 0`: a world whose particles all missed the canvas
/// would still draw the emitter's own sprite. The count with the burst has to
/// beat the count without it.
#[test]
fn a_burst_actually_draws_something_the_emitter_alone_does_not() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let mut bare = world_with_a_burst();
    let entity = bare.entity_ids()[0];
    bare.remove::<Burst>(entity)
        .expect("the emitter carries one");
    let emitter_only = drawn_pixels(&render(&target, &bare));
    assert!(emitter_only > 0, "even the emitter drew nothing");

    for ticks in SAMPLES {
        let lit = drawn_pixels(&frame_at(&target, ticks));
        assert!(
            lit > emitter_only,
            "age {ticks} lit {lit} pixels, no more than the {emitter_only} the \
             emitter lights on its own — the particles are not on the canvas"
        );
    }
}

/// **A replay reproduces the burst, not only the state it ends in.**
///
/// The "replay" here is a second run of the same construction from the same
/// state, which is what a recording replays back into: nothing in the burst
/// reads a clock or an OS source, so the second run has to agree at every age
/// inside the burst and not merely at its last one. Comparing only the end would
/// pass a run that took a different path to the same rest.
#[test]
fn a_second_run_reproduces_the_burst_at_every_age_and_not_only_at_its_end() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let mut original = world_with_a_burst();
    let mut replay = world_with_a_burst();

    let last = *SAMPLES.last().expect("four samples");
    let mut checked = 0;
    for tick in 0..=last {
        age(&mut original, 1);
        age(&mut replay, 1);

        if SAMPLES.contains(&tick) {
            assert_eq!(
                render(&target, &original).rgba(),
                render(&target, &replay).rgba(),
                "the replay parted from the original at tick {tick}"
            );
            checked += 1;
        }
    }

    assert_eq!(checked, SAMPLES.len(), "not every sample age was compared");
}

/// A spent burst draws what a world with no burst draws.
///
/// The frame, not the placements — `narvo-view2d` checks the vector and this
/// checks the pixels, so a difference that survived the extraction would still
/// be caught here.
#[test]
fn a_spent_burst_draws_what_no_burst_draws() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let mut spent = world_with_a_burst();
    age(&mut spent, a_burst().life);

    let mut without = world_with_a_burst();
    let entity = without.entity_ids()[0];
    without
        .remove::<Burst>(entity)
        .expect("the emitter carries one");

    assert_eq!(
        render(&target, &spent).rgba(),
        render(&target, &without).rgba(),
        "a finished burst is still drawing something"
    );
}
