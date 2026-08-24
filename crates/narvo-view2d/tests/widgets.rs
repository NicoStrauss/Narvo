//! The two oracles M6b.8 owes, both rendered and both two-directional.
//!
//! # Why the pixels and not the components
//!
//! M6b.7's lesson, quoted in this task's own prompt: an oracle that compares
//! "with the widget" against "without the widget" is blind to a widget that does
//! not move. What a widget *does* is a statement about **two states**, so every
//! assertion here renders twice and compares the two frames against each other.
//!
//! And the sharper half, which the prompt states and which is easy to get wrong:
//! a button that changes when the pointer arrives is only half of "it responds".
//! A button that looks different *always* passes that half too. So each case
//! also renders the pointer somewhere else and asserts the frame is **unchanged**
//! — byte for byte, at the same pixels.
//!
//! # Why this crate
//!
//! A HUD button is a world (`Transform`, `Sprite`, `HitRect`, `Tint`), a hit
//! test and an extraction. `narvo-view2d` is the crate that sees a world and
//! the renderer at once (ADR-0041), and it is the only one that can name all
//! three. `narvo-app` was refused on purpose: `ProjektPlan.md` §6/M6b's last
//! item requires what M6b.8 builds to be reachable from outside the tree, and a
//! binary has no lib target.

use narvo_ecs::{HitRect, Layer, Sprite, Tint, Transform, World};
use narvo_render2d::{
    CameraView, OffscreenTarget, Pixels, Projection, RenderError, ScreenAnchor, SpriteBatch,
    SpriteInstance, TextureRegion,
};
use narvo_view2d::{hit_test, regions_of};

/// Printed instead of failing when this machine has no GPU adapter at all.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// Set in CI, where a missing adapter is a failure rather than a skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Builds a target, or reports that this machine cannot host one.
///
/// Transcribed from `narvo-render2d`'s own tests rather than shared: this crate
/// has no dev-dependency on `narvo-testkit`, and adding one to carry nine lines
/// would close a second cycle for no gain.
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
        Err(other) => panic!(
            "creating the offscreen target failed for a reason other than a \
             missing adapter: {other}"
        ),
    }
}

/// A one-texel white texture, so a tint is the only thing that colours a sprite.
///
/// White and opaque on purpose: `SpriteTint` multiplies, so every visible
/// difference between two frames here is the tint and nothing else.
fn white() -> Pixels {
    Pixels::from_rgba8(1, 1, vec![255, 255, 255, 255]).expect("a one-texel texture")
}

/// The three looks a button has. The values are this test's, not the engine's.
///
/// Distinct in all three channels so that a frame comparison cannot pass by a
/// rounding coincidence in one of them.
const IDLE: Tint = Tint {
    red: 0.25,
    green: 0.25,
    blue: 0.35,
    alpha: 1.0,
};
const HOVER: Tint = Tint {
    red: 0.70,
    green: 0.70,
    blue: 0.85,
    alpha: 1.0,
};
const PRESS: Tint = Tint {
    red: 0.95,
    green: 0.95,
    blue: 0.40,
    alpha: 1.0,
};

/// The canvas both oracles render into.
const EDGE: u32 = 128;

/// A world with one button, anchored `inset` in from `anchor` on a `size` target.
///
/// The button is 32 x 16 and its hit rectangle covers it exactly, so "the
/// pointer is over the button" and "the pointer is over the drawn pixels" are
/// the same statement.
fn world_with_button(size: (u32, u32), anchor: ScreenAnchor, inset: (f32, f32)) -> World {
    let [x, y] = Projection::for_target(size.0, size.1).anchor(anchor, inset.0, inset.1);

    let mut world = World::new();
    let button = world.spawn();
    world
        .insert(
            button,
            Transform {
                x,
                y,
                rotation: 0.0,
                scale_x: 32.0,
                scale_y: 16.0,
            },
        )
        .expect("a fresh entity takes a transform");
    world
        .insert(button, Sprite::new("flat"))
        .expect("a fresh entity takes a sprite");
    world
        .insert(button, HitRect::new(16.0, 8.0, "buy", 1))
        .expect("a fresh entity takes a hit rectangle");
    world
        .insert(button, Layer::at(0.0))
        .expect("a fresh entity takes a layer");
    world
        .insert(button, IDLE)
        .expect("a fresh entity takes a tint");
    world
}

/// Writes the look a pointer at `at` produces, and says which it was.
///
/// The engine deliberately provides none of this: M6b.8's S1 measured hover and
/// press to be a consumer's own state, and §2 keeps them out of the engine until
/// a second consumer asks. What the engine provides is [`hit_test`], and this is
/// the whole of what a consumer builds on top of it.
fn apply_look(world: &mut World, at: Option<(f32, f32)>, pressed: bool) -> Tint {
    let hot = at
        .and_then(|(x, y)| hit_test(world, x, y))
        .is_some_and(|entity| {
            world
                .get::<HitRect>(entity)
                .is_ok_and(|rect| rect.action == "buy")
        });

    let look = match (hot, pressed) {
        (false, _) => IDLE,
        (true, false) => HOVER,
        (true, true) => PRESS,
    };

    let entity = world.entity_ids()[0];
    world.insert(entity, look).expect("the button takes a tint");
    look
}

/// Renders `world` as a screen-fixed overlay over an empty scene.
fn render(target: &OffscreenTarget, world: &World, texture: &Pixels) -> Pixels {
    let sprites: Vec<SpriteInstance> = regions_of(world)
        .into_iter()
        .map(|drawn| {
            SpriteInstance::new(drawn.placement, TextureRegion::WHOLE_TEXTURE)
                .sampled(drawn.filter)
                .tinted(drawn.tint)
        })
        .collect();

    target
        .render_sprites_over(
            texture,
            &[],
            SpriteBatch {
                image: texture,
                sprites: &sprites,
                camera: CameraView::IDENTITY,
            },
            CameraView::IDENTITY,
        )
        .expect("a frame")
}

/// A button looks different under the pointer — **and the same when it is not**.
///
/// Both directions at the same pixels, which is the requirement. The second
/// assertion is the one that carries the weight: without it a button that was
/// always `HOVER` would pass.
#[test]
fn a_button_changes_under_the_pointer_and_only_under_the_pointer() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };
    let texture = white();
    let size = (EDGE, EDGE);
    let anchor = ScreenAnchor::BottomLeft;
    let inset = (24.0, 16.0);

    // Where the button actually is, asked of the same function that placed it.
    let [bx, by] = Projection::for_target(size.0, size.1).anchor(anchor, inset.0, inset.1);

    let frame_for = |at: Option<(f32, f32)>, pressed: bool| {
        let mut world = world_with_button(size, anchor, inset);
        let look = apply_look(&mut world, at, pressed);
        (render(&target, &world, &texture), look)
    };

    let (idle, idle_look) = frame_for(None, false);
    let (hovered, hover_look) = frame_for(Some((bx, by)), false);
    let (pressed, press_look) = frame_for(Some((bx, by)), true);
    let (elsewhere, elsewhere_look) = frame_for(Some((bx + 60.0, by + 40.0)), false);

    // The looks themselves, so a pixel difference has a named cause.
    assert_eq!(idle_look, IDLE);
    assert_eq!(hover_look, HOVER);
    assert_eq!(press_look, PRESS);
    assert_eq!(elsewhere_look, IDLE);

    // Direction one: it changes when the pointer arrives.
    assert_ne!(
        idle.rgba(),
        hovered.rgba(),
        "the button did not change when the pointer arrived"
    );
    assert_ne!(
        hovered.rgba(),
        pressed.rgba(),
        "the button did not change when the pointer pressed"
    );

    // Direction two, and the half a always-different button would fail: it does
    // **not** change when the pointer is somewhere else. Byte for byte.
    assert_eq!(
        idle.rgba(),
        elsewhere.rgba(),
        "the button changed for a pointer that was not on it"
    );

    // And the change is where the button is, not somewhere else in the frame.
    // Without this, a frame that differed in an unrelated corner would pass.
    let centre = (
        (bx + EDGE as f32 / 2.0) as u32,
        (EDGE as f32 / 2.0 - by) as u32,
    );
    assert_ne!(
        idle.pixel(centre.0, centre.1),
        hovered.pixel(centre.0, centre.1),
        "the frames differ, but not at the button's own centre {centre:?}"
    );
}

/// An edge-anchored button keeps its distance to the edge; a centre-anchored one
/// keeps its distance to the centre. Rendered, at two target sizes, and the two
/// checked against each other.
///
/// The unit tests in `narvo-render2d` assert this on the arithmetic. This
/// asserts it on pixels, which is the thing a HUD author actually cares about
/// and the thing that would still catch an extraction or a projection that
/// disagreed with `anchor`.
///
/// # Both axes, and the red edge is why
///
/// The first version of this test measured the leftmost lit **column** only.
/// M6b.8's second red edge — `Edge::Low => inset_y`, which anchors a bottom to
/// the top — left that column exactly where it was, because `BottomLeft`'s
/// horizontal arm is untouched by it. **The test passed under the injection it
/// was written to catch.** Four unit tests in `narvo-render2d` did report, but
/// the rendered oracle, which is the one that does not merely restate the
/// formula, did not.
///
/// So it measures the bottom gap too. A one-axis oracle for a two-axis anchor
/// was the defect, and it was found by the edge rather than by argument.
#[test]
fn an_edge_anchored_button_holds_its_edge_across_target_sizes() {
    let sizes = [(128u32, 128u32), (192, 160)];
    let inset = (24.0, 16.0);

    // `(left gap, bottom gap)` per size, per anchoring.
    let mut edge_gaps: Vec<(u32, u32)> = Vec::new();
    let mut centre_gaps: Vec<(u32, u32)> = Vec::new();

    for size in sizes {
        let Some(target) = target_or_skip(size.0, size.1) else {
            return;
        };
        let texture = white();

        for (anchor, gaps) in [
            (ScreenAnchor::BottomLeft, &mut edge_gaps),
            (ScreenAnchor::Centre, &mut centre_gaps),
        ] {
            let world = world_with_button(size, anchor, inset);
            let frame = render(&target, &world, &texture);

            // "Lit" is "not the colour the corner has", so the cleared
            // background does not count as content.
            let background = frame.pixel(0, 0).expect("a corner");
            let lit = |x: u32, y: u32| frame.pixel(x, y).is_some_and(|p| p != background);

            let left = (0..size.0)
                .find(|x| (0..size.1).any(|y| lit(*x, y)))
                .expect("the button is somewhere in the frame");
            // Distance from the *bottom* edge to the button's lowest lit row.
            let lowest = (0..size.1)
                .rev()
                .find(|y| (0..size.0).any(|x| lit(x, *y)))
                .expect("the button is somewhere in the frame");

            gaps.push((left, size.1 - 1 - lowest));
        }
    }

    // The edge-anchored button is the same distance from the left edge **and
    // from the bottom edge** on both targets. That is the capability this task
    // added, and both halves are asserted because the anchor has two axes.
    assert_eq!(
        edge_gaps[0], edge_gaps[1],
        "an edge-anchored button moved relative to its edges: {edge_gaps:?}"
    );

    // The centre-anchored one is **not**, and that is what makes the assertion
    // above mean something rather than being satisfied by any placement at all.
    assert_ne!(
        centre_gaps[0], centre_gaps[1],
        "a centre-anchored button held its edges, so the comparison proves nothing: \
         {centre_gaps:?}"
    );

    // The centre-anchored button keeps its distance to the *centre* instead,
    // which is the other half of the same statement — on both axes.
    let centre_offsets: Vec<(i64, i64)> = sizes
        .iter()
        .zip(&centre_gaps)
        .map(|((w, h), (left, bottom))| {
            (
                i64::from(*left) - i64::from(w / 2),
                i64::from(*bottom) - i64::from(h / 2),
            )
        })
        .collect();
    assert_eq!(
        centre_offsets[0], centre_offsets[1],
        "a centre-anchored button moved relative to the centre: {centre_offsets:?}"
    );
}
