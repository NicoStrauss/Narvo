//! 2D rigid-body physics behind an engine-owned facade.
//!
//! The crate takes a slice of bare scalars, advances it by one tick, and writes
//! bare scalars back. No `rapier` type appears in any public item here, in the
//! same way `narvo-ecs` keeps `hecs` behind a facade (ADR-0002) and
//! `narvo-render2d` takes a buffer of `SpritePlacement` rather than a `World`
//! (ADR-0015). This is that decision in the other direction: the simulation
//! hands physics its state and gets its state back, and nothing else crosses.
//!
//! # The state boundary
//!
//! [`Physics2d::step`] **rebuilds the whole rapier world from the slice on every
//! call.** That is not an implementation detail to be optimised away later; it
//! is the decision ADR-0029 records, and the reason is that everything rapier
//! keeps between steps — warm-start impulses, contact and intersection graphs,
//! island assignment, BVH optimisation bookkeeping — would otherwise be
//! behaviour-carrying state living outside the canonical dump. `ProjektPlan.md`
//! §5.2 puts the rule plainly: what lies outside the dump lies outside the hash,
//! and a divergence there would be invisible.
//!
//! The price is real and was measured rather than estimated (M5b.2, and the
//! figures are in `target/reports/M5b_2.md`):
//!
//! * a retained pipeline and a rebuilt one part company at the first tick that
//!   resolves a contact, and are wholly different within a hundred ticks;
//! * rebuilding costs about 4.9× the per-tick time of retaining at the size of
//!   this engine's demo worlds;
//! * without warm-starting a settled stack keeps a small residual motion instead
//!   of coming to rest.
//!
//! What it buys is the property [`Physics2d::step`]'s own test pins: a world
//! reconstituted from its scalars continues exactly as the uninterrupted one
//! would have. That is the replay question (ADR-0012) and the hot-reload
//! question (ADR-0022) in miniature, and it is true here by construction rather
//! than by hope.
//!
//! # Rotation is a pair, not an angle
//!
//! [`Body::rot_cos`] and [`Body::rot_sin`] hold the rotation as the cosine and
//! sine rapier itself stores, not as an angle. Both are plain `f32`, so a
//! component holding them stays inside ADR-0014. There is no `angle()`
//! accessor, and the reason is two measurements rather than a preference.
//!
//! **A rotation does not survive an angle round trip.** Feeding a stored pair
//! through `atan2` and back through `sin`/`cos` does not reproduce it, so a
//! simulation whose rotation went through an angle every tick left the exact
//! one at **tick 2** — before anything had touched anything (M5b.2).
//!
//! **And the standard library's trigonometry is outside the determinism domain
//! altogether.** `enhanced-determinism` routes rapier's and glam's own maths
//! through `libm` (M5b.1), and the probe confirmed that a simulation using
//! glam's `angle`/`from_angle` stays byte-identical across Windows and WSL. A
//! method written here with `f32::atan2` would use *neither* — it would use
//! `std`, which the flag does not reach. That is not a hypothesis: an earlier
//! draft of this crate had such a method and a test asserting its round trip was
//! lossy, and the test **passed on Windows and failed on Linux**, where the same
//! round trip came back exact. The WSL pre-check of ADR-0007 is what caught it.
//!
//! So the convenience is not offered. A consumer that wants to author a pose in
//! radians can convert at its own call site, where the choice of trigonometry is
//! visible, and M5b.4 is where that first comes up.

#![forbid(unsafe_code)]

use rapier2d::prelude::*;

/// Whether a body is moved by the solver or stands still.
///
/// A separate enum rather than a `bool` because a body that is *fixed* is not a
/// dynamic body with a flag: it has no mass, takes no impulse, and its scalars
/// are inputs to the tick rather than outputs of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// Never moved by the solver. Ground, walls, platforms.
    Fixed,
    /// Integrated and collided by the solver.
    Dynamic,
}

/// The collision shape of a body.
///
/// Deliberately a closed set of two. A shape vocabulary is content, and content
/// arrives with the consumer that needs it (M5b.4) rather than on speculation —
/// `ProjektPlan.md` §2's rule against building for stock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// A disc of the given radius, centred on the body.
    Ball {
        /// Radius in world units. Must be finite and greater than zero.
        radius: f32,
    },
    /// An axis-aligned box of the given half-extents, centred on the body.
    Cuboid {
        /// Half the width. Must be finite and greater than zero.
        half_x: f32,
        /// Half the height. Must be finite and greater than zero.
        half_y: f32,
    },
}

/// One body, as the scalars that fully determine it.
///
/// Every field is a bare scalar or a plain enum over scalars, which is what lets
/// a registered component hold the same values without a foreign type entering
/// the canonical dump (ADR-0014). The struct is the seam: it is what
/// `SpritePlacement` is to the renderer (ADR-0015).
///
/// **This is the whole of the state.** If a quantity is not in this struct, it
/// does not survive a call to [`Physics2d::step`] — which is the point, not a
/// limitation to be worked around.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// Whether the solver moves this body.
    pub kind: BodyKind,
    /// The collision shape.
    pub shape: Shape,
    /// World-space x of the body's centre.
    pub x: f32,
    /// World-space y of the body's centre.
    pub y: f32,
    /// Cosine of the body's rotation. See the crate docs for why this is a pair
    /// and not an angle.
    pub rot_cos: f32,
    /// Sine of the body's rotation.
    pub rot_sin: f32,
    /// Linear velocity along x, world units per second.
    pub vel_x: f32,
    /// Linear velocity along y, world units per second.
    pub vel_y: f32,
    /// Angular velocity, radians per second.
    pub angvel: f32,
    /// Bounciness, 0 for none. Combined with the other body's by rapier's
    /// default rule.
    pub restitution: f32,
    /// Coulomb friction coefficient.
    pub friction: f32,
    /// Mass per unit area, from which the solver derives mass and inertia.
    /// Ignored for [`BodyKind::Fixed`].
    pub density: f32,
}

impl Body {
    /// A dynamic body at rest at the origin with the given shape.
    ///
    /// The rotation starts as the identity — `rot_cos = 1.0`, `rot_sin = 0.0` —
    /// which is the pair form of an angle of zero.
    #[must_use]
    pub fn dynamic(shape: Shape) -> Self {
        Self {
            kind: BodyKind::Dynamic,
            shape,
            x: 0.0,
            y: 0.0,
            rot_cos: 1.0,
            rot_sin: 0.0,
            vel_x: 0.0,
            vel_y: 0.0,
            angvel: 0.0,
            restitution: 0.0,
            friction: 0.5,
            density: 1.0,
        }
    }

    /// A fixed body at the origin with the given shape.
    #[must_use]
    pub fn fixed(shape: Shape) -> Self {
        Self {
            kind: BodyKind::Fixed,
            ..Self::dynamic(shape)
        }
    }

    /// Places the body, keeping everything else.
    #[must_use]
    pub fn at(mut self, x: f32, y: f32) -> Self {
        self.x = x;
        self.y = y;
        self
    }

    /// Sets the linear velocity, keeping everything else.
    #[must_use]
    pub fn with_velocity(mut self, vel_x: f32, vel_y: f32) -> Self {
        self.vel_x = vel_x;
        self.vel_y = vel_y;
        self
    }

    /// Sets the angular velocity, keeping everything else.
    #[must_use]
    pub fn with_angvel(mut self, angvel: f32) -> Self {
        self.angvel = angvel;
        self
    }

    /// Sets the material scalars, keeping everything else.
    #[must_use]
    pub fn with_material(mut self, restitution: f32, friction: f32, density: f32) -> Self {
        self.restitution = restitution;
        self.friction = friction;
        self.density = density;
        self
    }

    // There is deliberately no `angle()` and no `with_angle()`. Both would be
    // one line of `f32::atan2` and `f32::sin`/`cos`, and the crate docs' section
    // "Rotation is a pair, not an angle" records what measuring them found: the
    // standard library's trigonometry is **not** covered by rapier's
    // `enhanced-determinism`, and it does not agree across Windows and Linux.
    // A convenience method with that property in a crate whose whole subject is
    // determinism is a trap, and nothing consumes it yet, so it is not here.
}

/// The physics facade.
///
/// Holds the simulation's constants and nothing that survives a tick. It is a
/// struct rather than a free function so that the constants have a home and so
/// that a future allocation-reuse optimisation has somewhere to live — but see
/// the type-level warning: reusing an *allocation* would be fine, reusing
/// *state* would silently undo ADR-0029, and
/// `the_facade_does_not_retain_solver_state` is the test that notices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Physics2d {
    gravity_x: f32,
    gravity_y: f32,
    dt_seconds: f32,
}

impl Physics2d {
    /// A world with the given gravity, stepping `dt_seconds` per tick.
    ///
    /// `dt_seconds` is the caller's fixed timestep. `narvo-core`'s 60 Hz step
    /// is 16 666 666 ns rather than a sixtieth of a second (ADR-0003), so a
    /// caller passes what its own `FixedTimestep` says and not `1.0 / 60.0`.
    ///
    /// # Panics
    ///
    /// Panics if `dt_seconds` is not finite and positive. A non-positive or NaN
    /// timestep produces a world that is silently frozen or entirely NaN, and
    /// diagnosing that from its symptoms costs far more than failing here.
    #[must_use]
    pub fn new(gravity_x: f32, gravity_y: f32, dt_seconds: f32) -> Self {
        assert!(
            dt_seconds.is_finite() && dt_seconds > 0.0,
            "timestep must be finite and positive, got {dt_seconds} - a zero, \
             negative or NaN step does not advance a simulation, it corrupts one"
        );
        Self {
            gravity_x,
            gravity_y,
            dt_seconds,
        }
    }

    /// Advances every body by one tick, in place.
    ///
    /// The rapier world is built from `bodies`, stepped once, and discarded. The
    /// slice is both the input and the output, and it is the entirety of what
    /// carries from one call to the next — see the crate docs and ADR-0029.
    ///
    /// Body order is preserved: `bodies[i]` after the call is the same body as
    /// `bodies[i]` before it.
    ///
    /// # Panics
    ///
    /// Panics if a body carries a shape extent that is not finite and positive.
    /// rapier's own behaviour on a zero or NaN extent is to produce a degenerate
    /// collider whose effects appear several ticks later somewhere else, which
    /// is precisely the kind of failure this engine's error messages exist to
    /// pre-empt.
    pub fn step(&self, bodies: &mut [Body]) {
        let mut rapier_bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();
        let mut handles = Vec::with_capacity(bodies.len());

        for (index, body) in bodies.iter().enumerate() {
            let builder = match body.kind {
                BodyKind::Fixed => RigidBodyBuilder::fixed(),
                BodyKind::Dynamic => RigidBodyBuilder::dynamic(),
            };
            // `from_cos_sin_unchecked` takes the stored pair back exactly. Going
            // through an angle here would be the lossy path measured in M5b.2.
            let pose = Pose::from_parts(
                Vector::new(body.x, body.y),
                Rotation::from_cos_sin_unchecked(body.rot_cos, body.rot_sin),
            );
            let handle = rapier_bodies.insert(
                builder
                    .pose(pose)
                    .linvel(Vector::new(body.vel_x, body.vel_y))
                    .angvel(body.angvel)
                    // Sleeping is bookkeeping that would survive a tick, and
                    // nothing in `Body` records it. Rather than let a body sleep
                    // and lose that fact on the next rebuild, no body sleeps.
                    // ADR-0029 carries the reasoning and the cost.
                    .can_sleep(false)
                    .build(),
            );
            colliders.insert_with_parent(collider_of(body, index), handle, &mut rapier_bodies);
            handles.push(handle);
        }

        let params = IntegrationParameters {
            dt: self.dt_seconds,
            ..IntegrationParameters::default()
        };

        // Every one of these is constructed here and dropped at the end of the
        // call. That is the decision, written where it happens.
        PhysicsPipeline::new().step(
            Vector::new(self.gravity_x, self.gravity_y),
            &params,
            &mut IslandManager::new(),
            &mut BroadPhaseBvh::new(),
            &mut NarrowPhase::new(),
            &mut rapier_bodies,
            &mut colliders,
            &mut ImpulseJointSet::new(),
            &mut MultibodyJointSet::new(),
            &mut CCDSolver::new(),
            &(),
            &(),
        );

        for (body, handle) in bodies.iter_mut().zip(handles) {
            let updated = &rapier_bodies[handle];
            let translation = updated.translation();
            let rotation = updated.rotation();
            let velocity = updated.linvel();
            body.x = translation.x;
            body.y = translation.y;
            body.rot_cos = rotation.re;
            body.rot_sin = rotation.im;
            body.vel_x = velocity.x;
            body.vel_y = velocity.y;
            body.angvel = updated.angvel();
        }
    }
}

/// The collider for one body, with the extent check that keeps a degenerate
/// shape from reaching rapier.
fn collider_of(body: &Body, index: usize) -> ColliderBuilder {
    let check = |value: f32, name: &str| {
        assert!(
            value.is_finite() && value > 0.0,
            "body {index} has {name} = {value}, which must be finite and greater \
             than zero - a degenerate collider does not fail here, it produces \
             wrong contacts several ticks later"
        );
    };
    let shape = match body.shape {
        Shape::Ball { radius } => {
            check(radius, "a radius");
            ColliderBuilder::ball(radius)
        }
        Shape::Cuboid { half_x, half_y } => {
            check(half_x, "a half-width");
            check(half_y, "a half-height");
            ColliderBuilder::cuboid(half_x, half_y)
        }
    };
    shape
        .restitution(body.restitution)
        .friction(body.friction)
        .density(body.density)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checkpoints in the M2.3 shape: a comparison that only looks at the end
    /// establishes *that* two runs match and never *where* they stopped
    /// matching.
    const CHECKPOINTS: [usize; 4] = [1, 10, 60, 120];
    const TICKS: usize = 120;

    /// `narvo-core`'s 60 Hz step, written the way ADR-0003 says it comes out:
    /// 16 666 666 ns, not a sixtieth of a second.
    fn engine_dt() -> f32 {
        (core::time::Duration::from_secs(1) / 60).as_secs_f32()
    }

    fn world() -> Physics2d {
        Physics2d::new(0.0, -9.81, engine_dt())
    }

    /// A ground and six bodies that land on it and on each other. The start
    /// values are deliberately not exactly representable in binary - the
    /// ADR-0013 reasoning about `motion`'s 0.017: a scene of halves and
    /// quarters would agree by construction and demonstrate nothing.
    fn scene() -> Vec<Body> {
        let mut bodies = vec![
            Body::fixed(Shape::Cuboid {
                half_x: 20.0,
                half_y: 0.5,
            })
            .at(0.0, -0.5)
            .with_material(0.0, 0.73, 1.0),
        ];
        for i in 0..6 {
            let shape = if i % 2 == 0 {
                Shape::Ball { radius: 0.37 }
            } else {
                Shape::Cuboid {
                    half_x: 0.41,
                    half_y: 0.29,
                }
            };
            bodies.push(
                Body::dynamic(shape)
                    .at(-1.9 + i as f32 * 0.73, 0.9 + i as f32 * 1.1)
                    .with_velocity((i as f32 - 2.0) * 0.31, -0.17)
                    .with_angvel((i as f32 - 3.0) * 0.19)
                    .with_material(0.42, 0.73, 1.3),
            );
        }
        bodies
    }

    /// The bit patterns of everything a body carries. `==` on `f32` calls `0.0`
    /// and `-0.0` equal and two NaNs unequal, so it cannot see what a hash sees
    /// (ADR-0014's round-trip consequence). This can.
    fn bits(bodies: &[Body]) -> Vec<[u32; 7]> {
        bodies
            .iter()
            .map(|b| {
                [
                    b.x.to_bits(),
                    b.y.to_bits(),
                    b.rot_cos.to_bits(),
                    b.rot_sin.to_bits(),
                    b.vel_x.to_bits(),
                    b.vel_y.to_bits(),
                    b.angvel.to_bits(),
                ]
            })
            .collect()
    }

    fn run_to(ticks: usize) -> Vec<Body> {
        let physics = world();
        let mut bodies = scene();
        for _ in 0..ticks {
            physics.step(&mut bodies);
        }
        bodies
    }

    #[test]
    fn two_runs_agree_at_every_checkpoint() {
        let physics = world();
        let (mut left, mut right) = (scene(), scene());
        for tick in 1..=TICKS {
            physics.step(&mut left);
            physics.step(&mut right);
            if CHECKPOINTS.contains(&tick) {
                assert_eq!(
                    bits(&left),
                    bits(&right),
                    "two runs of the same scene diverged at tick {tick}"
                );
            }
        }
    }

    /// **The test the state-boundary decision rests on** (ADR-0029).
    ///
    /// A world stopped halfway, reduced to nothing but its scalars, and resumed
    /// from them reaches the same state as one that never stopped. That is the
    /// replay question of ADR-0012 and the hot-reload question of ADR-0022 in
    /// miniature, and it is the property a retained pipeline would not have: the
    /// M5b.2 probe measured a retained and a rebuilt world parting company at
    /// the first tick that resolves a contact.
    #[test]
    fn a_world_reconstituted_from_its_scalars_continues_identically() {
        let physics = world();
        let uninterrupted = run_to(TICKS);

        for cut in [1, 20, 60, 119] {
            // Everything the halfway world knows, as plain scalars.
            let mut resumed = run_to(cut);
            for _ in cut..TICKS {
                physics.step(&mut resumed);
            }
            assert_eq!(
                bits(&uninterrupted),
                bits(&resumed),
                "a world reconstituted at tick {cut} did not continue like the \
                 uninterrupted one - something outside `Body` is carrying \
                 behaviour across a tick, which is what ADR-0029 forbids"
            );
        }
    }

    /// The guard that notices if ADR-0029 is silently reversed.
    ///
    /// A retained pipeline computes a *different* trajectory from a rebuilt one,
    /// because warm-start impulses, the contact graph and island assignment all
    /// survive a step and change what the next one computes - rapier's own
    /// source says so at `rapier2d-0.35.1/src/geometry/contact_pair.rs:558`,
    /// where solver contacts are serialised despite being derivable because
    /// skipping them "breaks post-snapshot determinism".
    ///
    /// So this test asserts the two **differ**. It is the odd shape of assertion
    /// that is exactly right here: the day somebody makes the facade retain a
    /// `NarrowPhase` between calls to save the rebuild cost, this test goes red
    /// and names the ADR, rather than the change landing silently and taking the
    /// reconstitution property with it.
    #[test]
    fn the_facade_does_not_retain_solver_state() {
        let facade = run_to(TICKS);

        // The same scene through one pipeline that keeps everything.
        let mut rapier_bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();
        let mut handles = Vec::new();
        for (index, body) in scene().iter().enumerate() {
            let builder = match body.kind {
                BodyKind::Fixed => RigidBodyBuilder::fixed(),
                BodyKind::Dynamic => RigidBodyBuilder::dynamic(),
            };
            let handle = rapier_bodies.insert(
                builder
                    .pose(Pose::from_parts(
                        Vector::new(body.x, body.y),
                        Rotation::from_cos_sin_unchecked(body.rot_cos, body.rot_sin),
                    ))
                    .linvel(Vector::new(body.vel_x, body.vel_y))
                    .angvel(body.angvel)
                    .can_sleep(false)
                    .build(),
            );
            colliders.insert_with_parent(collider_of(body, index), handle, &mut rapier_bodies);
            handles.push(handle);
        }
        let params = IntegrationParameters {
            dt: engine_dt(),
            ..IntegrationParameters::default()
        };
        let (mut pipeline, mut islands, mut broad, mut narrow, mut ccd) = (
            PhysicsPipeline::new(),
            IslandManager::new(),
            BroadPhaseBvh::new(),
            NarrowPhase::new(),
            CCDSolver::new(),
        );
        let (mut impulse_joints, mut multibody_joints) =
            (ImpulseJointSet::new(), MultibodyJointSet::new());
        for _ in 0..TICKS {
            pipeline.step(
                Vector::new(0.0, -9.81),
                &params,
                &mut islands,
                &mut broad,
                &mut narrow,
                &mut rapier_bodies,
                &mut colliders,
                &mut impulse_joints,
                &mut multibody_joints,
                &mut ccd,
                &(),
                &(),
            );
        }

        let retained: Vec<[u32; 7]> = handles
            .iter()
            .map(|&h| {
                let b = &rapier_bodies[h];
                [
                    b.translation().x.to_bits(),
                    b.translation().y.to_bits(),
                    b.rotation().re.to_bits(),
                    b.rotation().im.to_bits(),
                    b.linvel().x.to_bits(),
                    b.linvel().y.to_bits(),
                    b.angvel().to_bits(),
                ]
            })
            .collect();

        assert_ne!(
            bits(&facade),
            retained,
            "the facade now matches a retained pipeline over {TICKS} ticks. \
             Either the scene stopped producing contacts - in which case this \
             test measures nothing and needs a better scene - or `step` began \
             keeping solver state between calls, which undoes ADR-0029 and with \
             it `a_world_reconstituted_from_its_scalars_continues_identically`"
        );
    }

    /// The scene has to actually rotate, or the headline test is weaker than it
    /// reads.
    ///
    /// `a_world_reconstituted_from_its_scalars_continues_identically` compares
    /// all seven scalars, but it can only exercise the rotation pair if the
    /// bodies turn. A scene that degenerated into pure translation — an edited
    /// start value, a shape change — would leave that test green while it had
    /// quietly stopped covering two of the seven fields. This is the guard
    /// against that, and it is the reason the scene mixes balls with boxes and
    /// starts every body spinning.
    ///
    /// *This test replaced one that asserted an angle round trip is lossy. That
    /// assertion passed on Windows and failed on Linux, because `f32::atan2` is
    /// `std`'s and `enhanced-determinism` does not reach `std` — see the crate
    /// docs. The property was platform-dependent, so it was not a property.*
    #[test]
    fn the_scene_rotates_so_the_reconstitution_test_covers_the_pair() {
        let after = run_to(30);
        let turned = after
            .iter()
            .filter(|b| b.kind == BodyKind::Dynamic)
            .filter(|b| b.rot_sin.to_bits() != 0.0f32.to_bits())
            .count();
        assert!(
            turned >= 3,
            "only {turned} of the scene's dynamic bodies have turned by tick 30. \
             The reconstitution test compares `rot_cos` and `rot_sin` along with \
             everything else, and a scene that stopped rotating would stop \
             covering them without going red"
        );
    }

    /// The enforcement point for ADR-0013's consequence.
    ///
    /// `enhanced-determinism` is not one of rapier's default features
    /// (`rapier2d-0.35.1/Cargo.toml:66-71`), so dropping it from the manifest is
    /// a silent opt-out: everything still builds, every test still passes, and
    /// the requirement is simply gone. Nothing else in this repository would
    /// notice - `cargo deny` is about licences, and the determinism suite
    /// compares two runs of one build, which agree either way.
    ///
    /// The guard reads the manifest at run time, in the drift-guard shape this
    /// repository already uses in `xtask`: `fs::read_to_string`, never
    /// `include_str!`. The difference matters - `include_str!` bakes the file
    /// into the test binary at compile time, so a stale artefact could pass
    /// while the file on disk had changed.
    ///
    /// **What it cannot do, stated rather than glossed:** it guards the string
    /// in this manifest, not the flag's effect. M5b.1 measured that in a
    /// 32-body scene the default configuration was byte-identical to this one
    /// across Windows and WSL, so there is no behavioural difference for a test
    /// to observe and no behavioural guard is available.
    #[test]
    fn the_manifest_still_asks_for_enhanced_determinism() {
        let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", manifest_path.display()));

        let dependency = manifest
            .lines()
            .find(|line| line.starts_with("rapier2d = "))
            .unwrap_or_else(|| {
                panic!(
                    "{} has no line starting `rapier2d = `. If the dependency was \
                     restated in another form, restate this guard with it - it is \
                     the only thing keeping ADR-0013's `enhanced-determinism` \
                     requirement from being dropped silently",
                    manifest_path.display()
                )
            });

        assert!(
            dependency.contains("\"enhanced-determinism\""),
            "{} declares rapier2d without `enhanced-determinism`:\n  {dependency}\n\
             ADR-0013 makes that feature a requirement, and it is not one of \
             rapier's defaults, so leaving it out is a silent opt-out rather \
             than a build error. Put it back, or supersede ADR-0013 first",
            manifest_path.display()
        );
    }

    #[test]
    #[should_panic(expected = "timestep must be finite and positive")]
    fn a_zero_timestep_is_refused() {
        let _ = Physics2d::new(0.0, -9.81, 0.0);
    }

    #[test]
    #[should_panic(expected = "must be finite and greater than zero")]
    fn a_degenerate_shape_is_refused() {
        let physics = world();
        let mut bodies = vec![Body::dynamic(Shape::Ball { radius: 0.0 })];
        physics.step(&mut bodies);
    }
}
