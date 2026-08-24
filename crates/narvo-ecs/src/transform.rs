//! Where a thing is, how it is turned and how big it is.
//!
//! [`Transform`] is simulation state, not render state. It is produced by
//! systems, it goes into the canonical dump and therefore into the state hash,
//! and a render path reads it without ever writing it (CLAUDE.md, "The render
//! path only *reads* simulation state").

use serde::{Deserialize, Serialize};

/// Position, rotation and scale of an entity in world space.
///
/// # Bare `f32`, and never a foreign type
///
/// Every field is a plain `f32`, and that is a decision rather than an
/// oversight (ADR-0014, D11). A maths library's own type in this struct would
/// put that library's serde representation inside the stability domain ADR-0008
/// names, so the next version bump of it would move every state hash in the
/// project without anything being wrong. What a computation library is for is
/// computing; it can be converted to at the point of use and never stored here.
///
/// # Orientation
///
/// **x grows right, y grows up, and positive [`rotation`](Self::rotation) turns
/// counter-clockwise.** This is not decided here: it is ADR-0004's world-space
/// amendment, which that document carries because a convention split across two
/// documents is how the two come to disagree.
///
/// The reason y points up rather than down is the rule the same ADR already
/// had: the two opposing y directions are reconciled **exactly once**. World
/// space agreeing with NDC is what keeps that true, and a world with y down
/// would need a second flip somewhere between this struct and the vertex table.
///
/// M3.4 is where it stopped being a stated intention. `Projection` and
/// `SpritePlacement` in `narvo-render2d` map a transform to NDC with no
/// negation on either axis, and pixel probes against an asymmetric texture —
/// with expectations derived from the convention before anything rendered —
/// show a sprite landing where this says it should. What is still open is
/// everything past one sprite: nothing here has been drawn under a camera, at a
/// non-integer position, or next to a second sprite.
///
/// # Units and ranges
///
/// [`rotation`](Self::rotation) is in **radians**. Every trigonometric function
/// in `std` takes radians, so a consumer converts nothing; turns would move a
/// multiplication to every call site, and a conversion applied in one place and
/// forgotten in another is the same class of defect as a second y-flip. The
/// cost of the choice, stated rather than hidden: a quarter turn is `π/2`,
/// which no `f32` represents exactly, so two ways of writing "90°" can differ
/// in their last bits. That matters for authoring content, which is D3's open
/// question, and not for the state hash, which compares two runs of one input.
///
/// No field is range-checked and none wraps. A rotation of `100.0` is a legal
/// value that means what `100.0 rad` means; nothing here normalises it into
/// `0..2π`, because normalising would make the stored value differ from the
/// written one and put a rounding step between a system and its own state.
///
/// # Scale is per axis
///
/// Two fields rather than one. A single uniform factor cannot express a
/// horizontal flip, which in 2D is how a character turns around and is about
/// the most ordinary thing a sprite does. Widening it later would change the
/// canonical form of a component already inside the state hash, which is a far
/// more expensive edit than carrying the second field from the start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Position along x, growing to the right.
    pub x: f32,
    /// Position along y, growing upward.
    pub y: f32,
    /// Rotation in radians, positive counter-clockwise.
    pub rotation: f32,
    /// Scale along x. Negative mirrors.
    pub scale_x: f32,
    /// Scale along y. Negative mirrors.
    pub scale_y: f32,
}

impl Transform {
    /// The identity: at the origin, unturned, unscaled.
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        rotation: 0.0,
        scale_x: 1.0,
        scale_y: 1.0,
    };

    /// A transform at `(x, y)`, unturned and unscaled.
    #[must_use]
    pub const fn at(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            ..Self::IDENTITY
        }
    }
}

impl Default for Transform {
    /// [`Transform::IDENTITY`], written out rather than derived.
    ///
    /// `#[derive(Default)]` would give every field its own default and produce a
    /// transform scaled by zero — an entity that exists, hashes, and is
    /// invisible for a reason nobody would look for. The identity is the only
    /// default that composes.
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::Transform;

    /// Values chosen to stress the text round trip rather than to look tidy.
    ///
    /// `1.0` and `0.0` survive any encoder, including a broken one, so they
    /// would demonstrate nothing. These would not:
    ///
    /// - `0.1` and `-1.0 / 3.0`, whose decimal expansions do not terminate in
    ///   binary,
    /// - [`f32::MIN_POSITIVE`] and a subnormal below it, where the exponent
    ///   runs out and precision is carried by the mantissa alone,
    /// - a magnitude near [`f32::MAX`], where the gap between neighbours is
    ///   larger than most whole numbers,
    /// - `-0.0`, which `==` cannot tell from `0.0` and whose sign a formatter
    ///   can silently drop,
    /// - a pair differing only in the last mantissa bit, which is the smallest
    ///   difference that exists at that magnitude.
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

    /// Builds a transform whose five fields all carry `value`.
    fn filled(value: f32) -> Transform {
        Transform {
            x: value,
            y: value,
            rotation: value,
            scale_x: value,
            scale_y: value,
        }
    }

    /// Compares two transforms on bit patterns, field by field.
    ///
    /// `==` is the wrong instrument here twice over: it calls `0.0` and `-0.0`
    /// equal, and it calls two NaNs unequal. A state hash sees bits, so a test
    /// that guards the hash has to see bits too.
    fn assert_same_bits(left: Transform, right: Transform, case: &str) {
        for (name, l, r) in [
            ("x", left.x, right.x),
            ("y", left.y, right.y),
            ("rotation", left.rotation, right.rotation),
            ("scale_x", left.scale_x, right.scale_x),
            ("scale_y", left.scale_y, right.scale_y),
        ] {
            assert_eq!(
                l.to_bits(),
                r.to_bits(),
                "{case}: field `{name}` did not survive the round trip bit for bit: \
                 {l:?} ({:#010x}) became {r:?} ({:#010x}). A field that loses bits here \
                 loses them in the canonical dump too, and a replay built on it drifts \
                 while every hash still reports agreement.",
                l.to_bits(),
                r.to_bits()
            );
        }
    }

    #[test]
    fn the_ron_round_trip_is_bit_exact_on_values_that_stress_it() {
        for value in probes() {
            let original = filled(value);
            let text = ron::to_string(&original).expect("a transform serializes");
            let returned: Transform = ron::from_str(&text)
                .unwrap_or_else(|error| panic!("{text} does not parse back: {error}"));

            println!("{:#010x} -> {text}", value.to_bits());
            assert_same_bits(original, returned, &format!("{value:?}"));
        }
    }

    /// The negative control: the comparison above would notice a lossy encoder.
    ///
    /// Without this, `the_ron_round_trip_is_bit_exact_on_values_that_stress_it`
    /// proves only that *something* passed. A test whose failure mode has never
    /// been observed is a test nobody has checked, which is how a green run
    /// comes to mean nothing.
    #[test]
    fn a_lossy_encoder_would_be_caught_by_the_same_comparison() {
        // Six decimals reads as generous and is not: an f32 near 0.1 needs nine
        // significant digits before the bits come back unchanged.
        let lossy = |value: f32| -> f32 {
            format!("{value:.6}")
                .parse()
                .expect("a fixed-point decimal parses back as f32")
        };

        let caught = probes()
            .into_iter()
            .filter(|value| lossy(*value).to_bits() != value.to_bits())
            .count();

        assert!(
            caught > 0,
            "a six-decimal round trip changed none of the probe values, so the \
             probes are too easy and the bit comparison above is not being \
             exercised by them"
        );

        // And the assertion itself has to reject what the filter found.
        let victim = probes()
            .into_iter()
            .find(|value| lossy(*value).to_bits() != value.to_bits())
            .expect("checked above");

        let outcome = std::panic::catch_unwind(|| {
            assert_same_bits(filled(victim), filled(lossy(victim)), "negative control");
        });
        assert!(
            outcome.is_err(),
            "assert_same_bits accepted {victim:?} against its six-decimal \
             approximation, so it is not comparing bits"
        );
    }

    /// Builds a one-entity world and returns its canonical dump and hash.
    ///
    /// `None` spawns the entity without a transform, which is what makes the
    /// "does the hash see it at all" question answerable.
    fn hashed(transform: Option<Transform>) -> (String, u64) {
        let mut registry = crate::ComponentRegistry::new();
        registry
            .register_component::<Transform>("transform")
            .expect("a fresh registry has no transform in it");

        let mut world = crate::World::new();
        let entity = world.spawn();
        if let Some(transform) = transform {
            world
                .insert(entity, transform)
                .expect("the entity was just spawned");
        }

        let dump = crate::canonical_dump(&world, &registry).expect("the component is registered");
        let hash = crate::state_hash(&dump);
        (dump, hash)
    }

    /// The smallest difference that exists at all has to reach the hash.
    ///
    /// If it does not, the hash is not reading the component's floats, and every
    /// cross-platform comparison built on it would report agreement about a
    /// quantity it never looked at — a green check that verifies nothing, which
    /// is the failure this project keeps meeting. One mantissa bit is the
    /// hardest version of the question and therefore the one worth asking.
    #[test]
    fn one_mantissa_bit_of_a_transform_changes_the_state_hash() {
        let base = Transform::at(0.1, 0.0);
        let nudged = Transform {
            x: f32::from_bits(base.x.to_bits() + 1),
            ..base
        };

        assert_ne!(
            base.x.to_bits(),
            nudged.x.to_bits(),
            "the two inputs must differ, or this test asserts nothing"
        );

        let (without, hash_without) = hashed(None);
        let (with, hash_with) = hashed(Some(base));
        let (nudged_dump, hash_nudged) = hashed(Some(nudged));

        println!("without a transform:\n{without}");
        println!("with {:#010x}:\n{with}", base.x.to_bits());
        println!("with {:#010x}:\n{nudged_dump}", nudged.x.to_bits());

        assert_ne!(
            hash_without, hash_with,
            "adding a Transform left the state hash unchanged, so the component \
             is outside the hash and nothing about it is being compared"
        );
        assert_ne!(
            hash_with, hash_nudged,
            "two transforms differing in the last mantissa bit of `x` hashed the \
             same, so the hash is not reading the float's bits. Every later claim \
             that two platforms agree would then be a claim about a quantity that \
             was never looked at."
        );
        assert_ne!(
            with, nudged_dump,
            "the dumps are identical, so the difference is lost before the hash \
             rather than inside it"
        );
    }

    #[test]
    fn the_default_is_the_identity_and_not_a_zero_scale() {
        assert_eq!(Transform::default(), Transform::IDENTITY);
        assert_eq!(Transform::default().scale_x.to_bits(), 1.0_f32.to_bits());
        assert_eq!(Transform::default().scale_y.to_bits(), 1.0_f32.to_bits());
        assert_eq!(Transform::at(3.0, 4.0).scale_x.to_bits(), 1.0_f32.to_bits());
    }
}
