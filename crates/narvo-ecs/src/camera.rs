//! Which part of the world a frame shows.
//!
//! [`Camera`] is simulation state for the same reason [`Transform`](crate::Transform)
//! and [`Layer`](crate::Layer) are: what a frame looks like has to be
//! reproducible from a recording, and a view kept outside the world would not
//! be. It goes into the canonical dump and therefore into the state hash of any
//! simulation that registers it.

use serde::{Deserialize, Serialize};

/// Where the view sits and how far it is zoomed in.
///
/// # Why a component in the world rather than render state beside it
///
/// Two places could hold this, and they differ in what is **reproducible**, not
/// in what any single frame looks like. M3.12 picked this one:
///
/// - *This.* The view is in the canonical dump, so a replay of a recording
///   composes the same frame from the same world — the same rectangle of it,
///   at the same magnification. The *form* of the argument is ADR-0010's, for
///   the random generator: state a run can differ about while every component
///   still matches is state the hash reports agreement on at the exact moment
///   the runs have diverged. The two are not the same question — a generator's
///   state reaches components on the next draw, a camera's reaches only the
///   picture — and that difference is the whole of what makes this a decision
///   rather than a corollary.
/// - *Render state outside the world.* Rejected. Its best argument is real and
///   is not a technicality: a camera is a *view*, and two viewers of one
///   deterministic simulation — split screen, a spectator, a debug free camera —
///   are the ordinary case in a shipped engine. Putting the view in the world
///   makes it part of the state hash, so moving a debug camera would read as a
///   simulation divergence. That price is knowingly paid, because today there is
///   one view and the property being bought is that a recording reproduces the
///   *picture*. When a second, viewer-local view arrives, the answer is a second
///   type that is not this one — not moving this one out.
///
/// # Shake, built in M3.31 — and this section's forecast was half wrong
///
/// Shake is what makes the choice above load bearing, and this section argued
/// it out before there was one. The conclusion held; **the mechanism it
/// predicted did not**, and it is left standing rather than quietly rewritten
/// because the difference is the interesting part.
///
/// What held: a shake drawn outside the world makes the frame unreproducible
/// from a recording, which is the one property this component exists to buy, so
/// the offset belongs in the world. [`Shake`](crate::Shake) is that component.
///
/// **What did not hold is the sentence "a shake offset changes no simulation
/// behaviour".** It rested on "nothing in this workspace reads a camera", and
/// something does: `narvo-app`'s `camera_of` reads one, and since M3.30
/// [`compose_camera`](crate::compose_camera) — `follow_camera` until M3.36 —
/// *writes* one. The camera is world
/// state, so a shake that writes it writes world state — which makes the
/// ADR-0010 question sharper than this section thought, not softer.
///
/// **And the answer went the other way.** This section expected the shake to
/// draw from an [`Rng`](crate::Rng) of its own. M3.31 uses **no generator at
/// all**: a decaying triangular oscillation is reproducible by construction, so
/// there is nothing for ADR-0010 to govern and no generator to share. The price
/// this section was willing to pay — a render-only effect advancing a generator
/// in the canonical dump — turned out not to be due.
///
/// # Three bare `f32`
///
/// ADR-0014, as for every registered component: a maths library's type here
/// would put that library's serde format inside ADR-0008's stability domain.
///
/// # Larger `zoom` shows less
///
/// `zoom = 2.0` makes one world unit two pixels, so half as much world fits on
/// the target: the camera is zoomed **in**. The direction is a choice, and the
/// opposite would have been just as workable arithmetically; a field that made
/// the picture smaller as it grew would read backwards to anyone who has used a
/// camera.
///
/// It is deliberately **not** written into ADR-0004, and M3.12 reports that
/// rather than deciding it there. ADR-0004's M3.11 amendment records a convention
/// *because* a symmetric test image cannot see it, and that argument does not
/// reach a zoom: reverse this direction and every image drawn at a zoom other
/// than 1 changes wherever a sprite is drawn — not at literally every pixel,
/// since background stays background. It hides only in images drawn at zoom 1,
/// which today is every blessed one — so the exclusion is about which document
/// should hold the convention, not a claim that nothing could conceal it.
///
/// Zero and negative zoom are **not** rejected. The fields are public, so a
/// check in a constructor would be one assignment away from being bypassed and
/// would buy nothing; and both values are well defined and reproducible —
/// `zoom = 0.0` collapses the world to a point, `zoom < 0.0` mirrors it on both
/// axes. Both are a bug in whatever wrote them, and this type stores what it was
/// given, as [`Transform`](crate::Transform) does with an unnormalised rotation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// Where the view's centre sits along world x.
    pub x: f32,
    /// Where the view's centre sits along world y.
    pub y: f32,
    /// Magnification. Larger shows less; `1.0` is one world unit per pixel.
    pub zoom: f32,
}

impl Camera {
    /// Origin, no magnification — the values the fixed projection had before
    /// M3.12.
    ///
    /// This is what [`Default`] gives a `Camera`. It is **not** what makes a
    /// camera-less world render unchanged: that is `camera_of` in `narvo-app`,
    /// which falls back to `CameraView::IDENTITY` when it finds no camera at all
    /// and never constructs one of these.
    pub const DEFAULT: Self = Self {
        x: 0.0,
        y: 0.0,
        zoom: 1.0,
    };

    /// A camera at `(x, y)`, unzoomed.
    #[must_use]
    pub const fn at(x: f32, y: f32) -> Self {
        Self { x, y, zoom: 1.0 }
    }

    /// A camera at `(x, y)`, magnified by `zoom`.
    #[must_use]
    pub const fn new(x: f32, y: f32, zoom: f32) -> Self {
        Self { x, y, zoom }
    }
}

impl Default for Camera {
    /// [`Camera::DEFAULT`], written out rather than derived.
    ///
    /// `#[derive(Default)]` would give `zoom = 0.0`, which shows nothing at all.
    /// The value "no camera" means is a decision this type owns.
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::Camera;

    /// Values chosen to stress the text round trip rather than to look tidy —
    /// the same set `Transform`'s and `Layer`'s round trips use.
    fn probes() -> Vec<f32> {
        let one_tenth = 0.1_f32;
        vec![
            one_tenth,
            f32::from_bits(one_tenth.to_bits() + 1),
            -1.0 / 3.0,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::MAX,
            -f32::MAX,
            -0.0,
            0.0,
            core::f32::consts::PI,
        ]
    }

    #[test]
    fn the_ron_round_trip_is_bit_exact_on_values_that_stress_it() {
        for value in probes() {
            let original = Camera::new(value, -value, value);
            let text = ron::to_string(&original).expect("a camera serializes");
            let returned: Camera = ron::from_str(&text)
                .unwrap_or_else(|error| panic!("{text} does not parse back: {error}"));

            println!("{:#010x} -> {text}", value.to_bits());
            for (field, before, after) in [
                ("x", original.x, returned.x),
                ("y", original.y, returned.y),
                ("zoom", original.zoom, returned.zoom),
            ] {
                assert_eq!(
                    before.to_bits(),
                    after.to_bits(),
                    "{field} did not survive the round trip bit for bit: {:#010x} became \
                     {:#010x}. A camera that loses bits here loses them in the canonical \
                     dump too, and a replay built on it frames a different picture while \
                     every hash still reports agreement.",
                    before.to_bits(),
                    after.to_bits()
                );
            }
        }
    }

    /// `-0.0` survives, and the test above can tell that it did.
    #[test]
    fn a_dropped_sign_on_negative_zero_would_be_visible_in_the_bits() {
        let negative = Camera::at(-0.0, -0.0);
        let positive = Camera::at(0.0, 0.0);

        assert_eq!(negative, positive, "`==` cannot tell these apart");
        assert_ne!(
            negative.x.to_bits(),
            positive.x.to_bits(),
            "the bit comparison must be able to, or the round-trip test above is \
             checking nothing for this value"
        );
    }

    #[test]
    fn the_default_camera_is_the_projection_that_preceded_it() {
        assert_eq!(Camera::DEFAULT.x.to_bits(), 0.0_f32.to_bits());
        assert_eq!(Camera::DEFAULT.y.to_bits(), 0.0_f32.to_bits());
        assert_eq!(Camera::DEFAULT.zoom.to_bits(), 1.0_f32.to_bits());
        assert_eq!(Camera::default(), Camera::DEFAULT);
        assert_eq!(Camera::at(3.0, 4.0).zoom.to_bits(), 1.0_f32.to_bits());
    }

    /// A world that uses no `Camera` dumps and hashes exactly as it did before
    /// this component existed.
    ///
    /// M3.12's regression evidence for ADR-0008, stated as a comparison rather
    /// than as a stored value because that ADR forbids committing a hash. The
    /// same shape as `Layer`'s in M3.10, and it holds for the same reason:
    /// `canonical_dump` asks each registered component to serialize itself for
    /// each entity and skips the `None`, so a registered type that no entity
    /// carries contributes nothing. The test pins that property rather than
    /// discovering it — an implementation that emitted an empty entry, or listed
    /// the registry's names as a header, would move every hash in the project
    /// the day a new component type was registered anywhere.
    #[test]
    fn registering_the_camera_type_changes_nothing_for_a_world_that_has_none() {
        use crate::{ComponentRegistry, Transform, World, canonical_dump, state_hash};

        let mut world = World::new();
        for index in 0..4_u8 {
            let entity = world.spawn();
            world
                .insert(entity, Transform::at(f32::from(index), 0.0))
                .expect("the entity was just spawned");
        }

        let mut without = ComponentRegistry::new();
        without
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts a first component");

        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts a first component");
        with.register_component::<Camera>("camera")
            .expect("camera is a second, differently named component");

        let dump_without = canonical_dump(&world, &without).expect("every component is registered");
        let dump_with = canonical_dump(&world, &with).expect("every component is registered");

        assert_eq!(
            dump_without, dump_with,
            "registering a component type that no entity carries changed the \
             canonical dump. Then adding `Camera` to this project would move the \
             state hash of every simulation that merely knows about it, and \
             ADR-0008's promise about what a hash comparison means would be \
             weaker than it says."
        );
        assert_eq!(state_hash(&dump_without), state_hash(&dump_with));
        assert!(
            !dump_with.contains("camera"),
            "the dump of a world with no Camera must not mention one; it reads:\n{dump_with}"
        );
    }
}
