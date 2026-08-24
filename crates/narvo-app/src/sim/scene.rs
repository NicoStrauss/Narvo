//! The drawing scene: many sprites, and a camera that moves over them.
//!
//! The first simulation in this repository that is meant to be *drawn*. The
//! other three exist so a determinism comparison has something to run on and
//! deliberately draw nothing; this one exists so the frame loop has something to
//! draw, and so `ProjektPlan.md` §6/M3's 50 000-sprite figure has a scene to be
//! measured on.
//!
//! # The registration duty, met here for the first time
//!
//! `ProjektPlan.md` §12 carries a standing obligation, and `follow.rs` states it
//! in the crate that owns the components: **a scenario that draws sprites and
//! follows something registers `Transform`, `Layer`, `Sampling`, `Camera`,
//! `Follow` and `Shake` together, or its replay is not a replay.** Until now no
//! scenario was in that position, because none of them drew. This one is, and
//! [`build_registry`] registers all six plus this module's own [`Wander`].
//!
//! What the registration buys, stated precisely rather than by slogan: it puts
//! the state that decides the *picture* inside the canonical dump, and therefore
//! inside ADR-0008's state hash. A replay whose camera ended up somewhere else,
//! or whose sprites asked for a different sampler, would then differ in the hash
//! rather than only on screen. It does not by itself make anything replayable —
//! what does that is building from `(mode, seed)` with no clock and no OS
//! entropy anywhere, which is this module's other property and is the one
//! [`build`] is written to keep.
//!
//! # Two sizes of the same scene
//!
//! The scene is built by [`build_with`], which takes the sprite count. Two
//! callers give it two different numbers on purpose:
//!
//! - [`SPRITES`] — small, and what [`Mode::Scene`](crate::sim::Mode::Scene)
//!   uses. It is the size that runs inside the determinism suite and inside
//!   ordinary unit tests, where 10 000 ticks over 50 000 entities would cost
//!   more than the whole crate's test budget.
//! - [`MEASURED_SPRITES`] — 50 000, and what the frame-time measurement uses.
//!
//! **The duty is about which components are registered, not about how many
//! entities carry them**, so both sizes discharge it identically and the small
//! one is a faithful replay of the same construction. What the small size does
//! *not* exercise is the throughput; that is what the measurement is for.

use narvo_ecs::{
    Camera, ComponentRegistry, EcsError, Follow, Layer, Sampling, Scheduler, Shake, SystemContext,
    Transform, World, compose_camera,
};
use serde::{Deserialize, Serialize};

use super::Simulation;

/// Sprites in the scene as the replayable mode builds it.
///
/// Small enough that 10 000 ticks of it stay inside CLAUDE.md's five-second
/// single-crate test budget, and large enough that the grid has several rows and
/// the camera travels across rather than within one cell.
pub const SPRITES: usize = 512;

/// Sprites in the scene as the frame-time measurement builds it.
///
/// The number `ProjektPlan.md` §6/M3 names. It is below
/// `narvo-render2d`'s `MAX_SPRITES_PER_BATCH` of 65 536, so the whole scene is
/// one batch and the measurement is not secretly measuring batch splitting.
pub const MEASURED_SPRITES: usize = 50_000;

/// Edge length of one sprite in world units.
///
/// Sixteen, matching the sprite size the throughput test in
/// `narvo-render2d/tests/sprite_placement.rs` has always used, so the new
/// figures and the old ones describe sprites of the same size.
const SPRITE_EDGE: f32 = 16.0;

/// Distance between neighbouring sprites in the grid, in world units.
///
/// Larger than [`SPRITE_EDGE`], so **no two grid sprites overlap** and the
/// drawing order cannot change the picture they make. That is deliberate: an
/// overlapping field would make the image depend on the depth sort, and this
/// scene's job is to be measured rather than to test ordering — which
/// `layer_order_regions_128x128` already does, against a blessed reference.
///
/// The wander target is the exception, and it is drawn too, so it passes over
/// the grid and does overlap it. That is one sprite in fifty thousand, and one
/// world unit across rather than sixteen — a single pixel at this scene's zoom
/// of 1.0. Named here rather than left to contradict the sentence above. What
/// the window shows moving is the field, which the camera drags past by chasing
/// that target; the target itself is a pixel.
const SPACING: f32 = 24.0;

/// How far the wander travels from the origin along each axis, in world units.
const WANDER_RADIUS: f32 = 600.0;

/// Ticks the wander takes to complete one circuit.
///
/// A power of two, so `tick % PERIOD` and the division below are exact in f32
/// and the path repeats without drift.
const WANDER_PERIOD: u64 = 1_024;

/// Ticks between two shakes.
///
/// A pure function of the tick number rather than a draw from a generator: the
/// scene needs `Shake` to be *exercised* rather than merely registered, and a
/// shake armed once at build stops contributing long before a run ends.
///
/// How long is worth writing down, because it decides this constant.
/// `Shake::around` sets `decay` to 0.5 and `cutoff` to **0.0**, so the amplitude
/// halves every tick and `at_rest` becomes true only once it reaches exactly
/// zero — which repeated halving of 12.0 does after about 154 ticks, by
/// underflow rather than by meeting a threshold. Arming every 240 ticks
/// therefore leaves it moving for roughly the first two thirds of each cycle and
/// at rest for the remainder, which exercises both states.
const SHAKE_EVERY: u64 = 240;

/// The amplitude each re-arming asks for, in world units.
const SHAKE_AMPLITUDE: f32 = 12.0;

/// Where a wandering entity is going, as a closed form over the tick number.
///
/// A component rather than a hidden constant because it is simulation state that
/// decides the picture: two worlds whose wander differs draw differently, and
/// ADR-0008's hash has to be able to see that. It is this module's own type, in
/// the same way `motion.rs` owns `Wobble` — a demo's component belongs to the
/// demo.
///
/// It holds no phase. The position is a function of the tick alone, so there is
/// nothing to accumulate and nothing that can drift between a run and its
/// replay.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Wander {
    /// How far from the origin the path reaches along each axis.
    pub radius: f32,
    /// Ticks in one circuit.
    pub period: u64,
}

/// The triangle wave on `0.0 .. 4.0`, running 0 → 1 → 0 → −1 → 0.
///
/// The same shape and the same reasoning as `Shake`'s: comparisons and one
/// subtraction per branch, no table, no transcendental. `sin` would be a libm
/// call, and libm is exactly the kind of dependency ADR-0013's cross-platform
/// determinism cannot afford in the state hash — two platforms may round it
/// differently and nothing here would notice.
fn triangle(phase: f32) -> f32 {
    if phase < 1.0 {
        phase
    } else if phase < 3.0 {
        2.0 - phase
    } else {
        phase - 4.0
    }
}

/// Where the wander is at `tick`.
///
/// x and y are a quarter period apart, so the path is a diamond rather than a
/// line — the same trick `Shake::offset` uses, and for the same reason: a line
/// reads as a glitch.
///
/// Every operation here is IEEE 754 basic arithmetic on values converted from
/// integers, so two platforms compute it identically. `period` is a power of two
/// in this scene, which makes the divide exact as well, though nothing depends
/// on that.
#[must_use]
fn wander_at(wander: Wander, tick: u64) -> (f32, f32) {
    if wander.period == 0 {
        // A component does not validate its fields, and a zero period would
        // divide by zero. Standing still is the answer that draws something
        // rather than filling the world with `NaN`.
        return (0.0, 0.0);
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "the phase is taken modulo the period, which is 1024 here and \
                  exactly representable; a period beyond 2^24 loses resolution \
                  rather than determinism"
    )]
    let phase = (tick % wander.period) as f32 / wander.period as f32 * 4.0;
    let quarter = if phase + 1.0 >= 4.0 {
        phase + 1.0 - 4.0
    } else {
        phase + 1.0
    };

    (
        wander.radius * triangle(phase),
        wander.radius * triangle(quarter),
    )
}

/// Moves every wandering entity to where its tick says it should be.
///
/// Runs before [`compose_camera`], which is what makes the camera follow the
/// position this tick produced rather than the one before it. The scheduler runs
/// systems in registration order, and [`build_scheduler`] registers them in that
/// order.
fn wander(world: &mut World, context: &SystemContext) {
    let tick = context.tick();

    // `entity_ids` rather than a query: it is the canonical ascending
    // enumeration, and a query's archetype order is documented as unstable. The
    // writes here are independent per entity, so the order does not change the
    // result - but reading them in an unstable order is the habit this
    // repository has decided against, and `placements_of` gives the reason.
    for entity in world.entity_ids() {
        let Ok(wander) = world.get::<Wander>(entity).map(|found| *found) else {
            continue;
        };

        let (x, y) = wander_at(wander, tick);
        if let Ok(mut transform) = world.get_mut::<Transform>(entity) {
            transform.x = x;
            transform.y = y;
        }
    }
}

/// Re-arms every shake on a schedule that is a function of the tick.
///
/// Runs before [`compose_camera`] for the same reason `wander` does: `Shake`
/// advances and composes inside that system, so arming has to have happened
/// first or the impulse is a tick late.
///
/// No generator is involved. ADR-0010 would require a seeded one whose state
/// lives in the world, and a schedule needs neither — which is also what
/// `Shake` itself decided in M3.31.
fn arm_shakes(world: &mut World, context: &SystemContext) {
    if !context.tick().is_multiple_of(SHAKE_EVERY) {
        return;
    }

    for entity in world.entity_ids() {
        if let Ok(mut shake) = world.get_mut::<Shake>(entity) {
            shake.arm(SHAKE_AMPLITUDE);
        }
    }
}

/// The scene at [`SPRITES`], which is what the replayable mode runs.
///
/// # Errors
///
/// [`EcsError`] if a component is registered twice or a stable name is reused.
pub fn build() -> Result<Simulation, EcsError> {
    build_with(SPRITES)
}

/// The scene with `sprites` drawn sprites.
///
/// The camera and its target are extra, so the world holds `sprites + 2`
/// entities. Only the sprites carry a [`Transform`] *and* a [`Layer`]; the
/// camera carries none, so it is not itself drawn.
///
/// # Errors
///
/// [`EcsError`] if a component is registered twice or a stable name is reused.
pub fn build_with(sprites: usize) -> Result<Simulation, EcsError> {
    Ok(Simulation {
        world: build_world(sprites)?,
        registry: build_registry()?,
        scheduler: build_scheduler()?,
    })
}

/// The grid, the target it follows, and the camera that follows it.
fn build_world(sprites: usize) -> Result<World, EcsError> {
    let mut world = World::new();

    // A square-ish grid. `isqrt` is floor(sqrt), so this is the largest side
    // whose square does not exceed the count: 512 sprites give 22 columns and
    // therefore 24 rows, which is oblong rather than square. That is fine - what
    // matters is that the field is two-dimensional and deterministic, not that
    // it is exactly square. Integer arithmetic, so no float enters the layout.
    let columns = sprites.isqrt().max(1);

    for index in 0..sprites {
        let column = index % columns;
        let row = index / columns;

        #[expect(
            clippy::cast_precision_loss,
            reason = "a grid coordinate of a 50 000-entity scene is far inside \
                      f32's exactly-representable integers"
        )]
        let (x, y) = (column as f32, row as f32);
        // Centred on the origin, so the camera starts in the middle of the field
        // rather than at a corner looking at nothing.
        #[expect(
            clippy::cast_precision_loss,
            reason = "as above; the grid is at most a few hundred cells a side"
        )]
        let half = columns as f32 / 2.0;

        let entity = world.spawn();
        world.insert(
            entity,
            Transform {
                x: (x - half) * SPACING,
                y: (y - half) * SPACING,
                rotation: 0.0,
                scale_x: SPRITE_EDGE,
                scale_y: SPRITE_EDGE,
            },
        )?;
        // One layer for the whole grid: the sprites do not overlap, so nothing
        // here depends on the depth order. Carrying the component anyway is what
        // puts it in the dump.
        world.insert(entity, Layer::at(0.0))?;
        world.insert(entity, sampling_for(index, sprites))?;
    }

    // The wander target. It carries no `Layer` and no `Sampling`, so it is drawn
    // at the defaults - and it *is* drawn, because it carries a `Transform`:
    // `placements_of` keeps every entity that has one. It inherits
    // `Transform::at`'s scale of 1.0, so it is one pixel at this scene's zoom
    // and is not what a viewer notices moving; what moves visibly is the field,
    // dragged past by the camera chasing this entity.
    let target = world.spawn();
    world.insert(target, Transform::at(0.0, 0.0))?;
    world.insert(
        target,
        Wander {
            radius: WANDER_RADIUS,
            period: WANDER_PERIOD,
        },
    )?;

    // The camera. It has no `Transform`, so it draws nothing, and it is the
    // lowest id carrying a `Camera` because it is the only one - `camera_of`
    // takes the lowest and ignores the rest.
    let camera = world.spawn();
    world.insert(camera, Camera::new(0.0, 0.0, 1.0))?;
    // Damped rather than immediate, so the camera lags the target and the
    // picture moves smoothly instead of snapping - which is what a frame-time
    // measurement wants to be looking at.
    world.insert(camera, Follow::damped(target, 0.0, 0.0, 0.125))?;
    world.insert(camera, Shake::around(0.0, 0.0))?;

    Ok(world)
}

/// Which sampler the sprite at `index` asks for.
///
/// **The first half of the grid asks for `Nearest` and the second half for
/// `Linear`, as two contiguous blocks.** Draw order is `placements_of`'s, which
/// is by depth and then by ascending `EntityId`; every sprite here shares a
/// depth and they are spawned in index order, so the drawing order is this
/// order and the two blocks stay contiguous in it. `batch_runs` therefore cuts
/// the batch into **three** runs — the two blocks, plus the wander target, which
/// carries no `Sampling` at all, is drawn last because it is spawned last, and
/// falls to the `Nearest` default. Three draw calls for the frame.
///
/// **Alternating the two samplers per sprite is the thing to avoid, and it was
/// briefly what this function did.** `batch_runs` cuts wherever the wish
/// changes, so alternating turns one batch into one run *per sprite*: at 50 000
/// sprites, 50 001 draw calls per frame. `sprite.rs`'s
/// `alternating_filters_give_one_run_per_sprite` has proved that cutting
/// behaviour GPU-free since M3.20, but nothing had ever *timed* it — M3.20's own
/// measurement is headed "50 000 sprites, one filter". The first M3.32
/// measurement was accidentally taken in the alternating state, and its figures
/// are kept in `docs/perf/BASELINE.md` beside the real ones, because the
/// difference between them is what the D15 decomposition is worth.
const fn sampling_for(index: usize, sprites: usize) -> Sampling {
    // Strictly less than half, so a scene of one sprite is all `Nearest` rather
    // than all `Linear` - it keeps the degenerate case on the side every scene
    // before this one was on.
    if index * 2 < sprites {
        Sampling::nearest()
    } else {
        Sampling::linear()
    }
}

/// Every component this scene puts in the canonical dump.
///
/// All six of the standing obligation, plus this module's own [`Wander`].
///
/// # Errors
///
/// [`EcsError`] if a component is registered twice or a stable name is reused.
fn build_registry() -> Result<ComponentRegistry, EcsError> {
    let mut registry = ComponentRegistry::new();

    registry.register_component::<Transform>("transform")?;
    registry.register_component::<Layer>("layer")?;
    registry.register_component::<Sampling>("sampling")?;
    registry.register_component::<Camera>("camera")?;
    registry.register_component::<Follow>("follow")?;
    registry.register_component::<Shake>("shake")?;
    registry.register_component::<Wander>("wander")?;

    Ok(registry)
}

/// The three systems, in the order they must run.
///
/// `wander` and `arm_shakes` write the inputs `compose_camera` reads, so both
/// come before it. `compose_camera` is the single writer of the camera (M3.31),
/// and it advances `Shake` itself — so nothing here advances a shake, only arms
/// one.
///
/// # Errors
///
/// [`EcsError`] if a system name is used twice.
fn build_scheduler() -> Result<Scheduler, EcsError> {
    let mut scheduler = Scheduler::new();

    scheduler.add_system("wander", wander)?;
    scheduler.add_system("arm_shakes", arm_shakes)?;
    scheduler.add_system("compose_camera", compose_camera)?;

    Ok(scheduler)
}
