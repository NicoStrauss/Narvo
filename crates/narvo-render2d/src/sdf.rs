//! A distance field, computed by jump flooding.
//!
//! **Public, where `compute` and `field` are private.** Those two are the
//! machinery; this is the capability, and its consumers are outside the crate:
//! M8.3b turns occluders into world state and hands the seeds here, and M8.4
//! marches a ray against the distances that come back. `compute.rs`'s header
//! says nothing is exported until something outside needs it, and this is the
//! first thing that does.
//!
//! # What jump flooding computes, and what it does not
//!
//! Given a set of seed texels, it fills every texel with the coordinate of the
//! nearest one, in log2(n) passes of a fixed neighbourhood — and it is an
//! **approximation**. M8.3a measured that rather than assuming it, on eight
//! adapter/backend pairs in both profiles:
//!
//! | arrangement | texels naming a seed that is not the nearest |
//! |---|---|
//! | one seed, 128 x 128 | 0 of 16 384 |
//! | sixteen scattered points | 0 of 16 384 |
//! | two near clusters | 0 of 16 384 |
//! | a wall of 128 with one seed behind it | 0 of 16 384 |
//! | 64 points in 1024 x 1024 | 0 of 1 048 576 |
//! | **a rasterised circle** | **27 of 16 384**, none farther than **0.2425 texels** |
//!
//! So the oracle in this module is a *bound* and not an equality, and the bound
//! is measured. The dense ring was the arrangement that broke, and the one
//! deliberately built to be unfavourable — a wall with a straggler behind it —
//! was not; that is written down because the guess was wrong and the report
//! keeps it that way.
//!
//! **The error is an over-estimate**, which is the direction that matters
//! downstream: the seed a texel keeps is *farther* than the true nearest, so a
//! derived distance is too large and a sphere trace over it may step too far.
//! M8.4 is where that lands, and the lever is measured and not spent here: one
//! extra pass at step 1 takes 27 to 10, and two extra passes at 2 and 1 take it
//! to 3. Neither is used, because M8.3a has no consumer that needs the tighter
//! field and §5 of its brief forbids building for one that does not exist yet.
//!
//! # What it costs
//!
//! [`OffscreenTarget::distance_field`](crate::OffscreenTarget::distance_field)
//! compiles its pipeline and allocates its field pair on **every call**. That is
//! deliberate for a first consumer that computes a field once, and it is the
//! wrong shape for one that computes a field every frame. M8.3b is the task that
//! will know which it is; if it measures the per-frame cost as material, the
//! reopening is a compiled pass object holding both — which is a bigger public
//! surface, and so is not built on a guess.

use crate::error::RenderError;
use crate::field::FIELD_CHANNELS;

/// The jump-flooding kernel's source.
pub(crate) const JUMP_FLOOD_WGSL: &str = include_str!("shaders/jump_flood.wgsl");

/// The entry point in [`JUMP_FLOOD_WGSL`].
pub(crate) const JUMP_FLOOD_ENTRY: &str = "jump_flood";

/// Which texels of a field are seeds.
///
/// Plain data: no GPU is involved in building one, and none is needed to check
/// one. That is what lets the step schedule and the seed layout be tested on a
/// machine with no adapter at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seeds {
    width: u32,
    height: u32,
    /// One entry per texel, row-major from the top left, `true` where a seed is.
    occupied: Vec<bool>,
    count: usize,
}

impl Seeds {
    /// An empty `width` x `height` seed set.
    ///
    /// # Errors
    ///
    /// [`RenderError::InvalidSize`] if a dimension is zero or above
    /// [`OffscreenTarget::MAX_DIMENSION`](crate::OffscreenTarget::MAX_DIMENSION),
    /// checked here so that a caller finds out before it has filled a buffer the
    /// GPU will refuse.
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let max = crate::OffscreenTarget::MAX_DIMENSION;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(RenderError::InvalidSize { width, height, max });
        }
        Ok(Self {
            width,
            height,
            occupied: vec![false; width as usize * height as usize],
            count: 0,
        })
    }

    /// Marks the texel at `(x, y)` as a seed. Marking one twice changes nothing.
    ///
    /// # Errors
    ///
    /// [`RenderError::SeedOutsideField`] if the point is outside the field.
    pub fn set(&mut self, x: u32, y: u32) -> Result<(), RenderError> {
        if x >= self.width || y >= self.height {
            return Err(RenderError::SeedOutsideField {
                x,
                y,
                width: self.width,
                height: self.height,
            });
        }
        let index = y as usize * self.width as usize + x as usize;
        if !self.occupied[index] {
            self.occupied[index] = true;
            self.count += 1;
        }
        Ok(())
    }

    /// Whether `(x, y)` is a seed. `false` for a point outside the field.
    #[must_use]
    pub fn is_seed(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.occupied[y as usize * self.width as usize + x as usize]
    }

    /// Width in texels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in texels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// How many texels are seeds.
    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    /// The field texels a seeded field starts from.
    ///
    /// Four floats a texel: the seed's own coordinate, a one, and a zero. A
    /// texel that is not a seed is four zeros, so an all-zero buffer is the
    /// empty seed set — the shader's header carries why that beats a negative
    /// sentinel.
    pub(crate) fn texels(&self) -> Vec<f32> {
        let mut texels = vec![0.0_f32; self.occupied.len() * FIELD_CHANNELS];
        for (index, seeded) in self.occupied.iter().enumerate() {
            if !seeded {
                continue;
            }
            let x = index as u32 % self.width;
            let y = index as u32 / self.width;
            let base = index * FIELD_CHANNELS;
            // Exact: a coordinate is below `MAX_DIMENSION`, an `f32` holds every
            // integer below 2^24, and 8192 is a long way below it.
            texels[base] = x as f32;
            texels[base + 1] = y as f32;
            texels[base + 2] = 1.0;
            texels[base + 3] = 0.0;
        }
        texels
    }
}

/// For every texel, the nearest seed — or none, if the field had no seeds.
///
/// The **coordinate** is what is stored, and the distance is derived from it.
/// That is not a convenience: a chain that propagated distances would have
/// thrown away the thing the next pass needs, so the coordinate is what travels
/// and the distance is computed once, here, at the end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedMap {
    width: u32,
    height: u32,
    /// One entry per texel, row-major, `None` where the field had no seed at all.
    nearest: Vec<Option<(u32, u32)>>,
}

impl SeedMap {
    /// The nearest seed to `(x, y)`, or `None` outside the field and in a field
    /// with no seeds.
    #[must_use]
    pub fn nearest(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.nearest[y as usize * self.width as usize + x as usize]
    }

    /// The **squared** distance from `(x, y)` to its nearest seed.
    ///
    /// Exact, in integers, and therefore the form to compare across machines:
    /// it is the same arithmetic the kernel does, and M8.3a measured that field
    /// to be byte-identical in 16 of 16 adapter/backend/profile cells. Prefer it
    /// to [`Self::distance`] anywhere a value is hashed, compared or stored.
    #[must_use]
    pub fn distance_squared(&self, x: u32, y: u32) -> Option<u64> {
        let (sx, sy) = self.nearest(x, y)?;
        let dx = i64::from(sx) - i64::from(x);
        let dy = i64::from(sy) - i64::from(y);
        // Both fit: a coordinate difference is below 2^13, so the sum of squares
        // is below 2^27 and nowhere near `i64`'s range.
        Some((dx * dx + dy * dy) as u64)
    }

    /// The Euclidean distance from `(x, y)` to its nearest seed.
    ///
    /// The square root of [`Self::distance_squared`], taken in `f64` on the CPU.
    /// **No square root appears in WGSL anywhere in this crate**, which is what
    /// keeps the field itself inside the subset M8.0 measured as reproducible;
    /// this is the one place a root is taken and it is taken once, after the
    /// chain has finished.
    ///
    /// Whether two platforms agree on the *rounded* value here is **not
    /// measured** by M8.3a. What was measured is the field it is derived from.
    /// A caller that needs a value to be equal across machines should use
    /// [`Self::distance_squared`], which is an integer.
    #[must_use]
    pub fn distance(&self, x: u32, y: u32) -> Option<f32> {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a squared distance in this field is below 2^27, which an f64 holds exactly"
        )]
        let squared = self.distance_squared(x, y)? as f64;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the root of a value below 2^27 is below 2^14, well inside f32's range"
        )]
        let distance = squared.sqrt() as f32;
        Some(distance)
    }

    /// Width in texels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in texels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Reads a field's texels back into a seed map.
    ///
    /// The inverse of [`Seeds::texels`]: channel z is the validity flag, and the
    /// first two channels are the coordinate. A texel whose flag is zero had no
    /// seed reach it, which in a field with at least one seed cannot happen —
    /// and is not asserted here, because the chain's own oracles are where that
    /// belongs.
    pub(crate) fn from_texels(width: u32, height: u32, texels: &[f32]) -> Self {
        let nearest = texels
            .chunks_exact(FIELD_CHANNELS)
            .map(|texel| {
                if texel[2] == 0.0 {
                    None
                } else {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "the kernel writes exact non-negative integers below MAX_DIMENSION"
                    )]
                    let point = (texel[0] as u32, texel[1] as u32);
                    Some(point)
                }
            })
            .collect();
        Self {
            width,
            height,
            nearest,
        }
    }
}

/// The jump distances one chain runs, in order.
///
/// Descending powers of two, from the largest below the longer side down to one.
/// A field one texel on its longest side needs no pass at all: the answer is
/// whatever was seeded.
///
/// **Descending, and every step present.** Both halves are load-bearing and both
/// are guarded: an ascending schedule propagates locally before it propagates
/// far, and a chain that stops at two never lets a seed reach its immediate
/// neighbour.
#[must_use]
pub(crate) fn jump_flood_steps(width: u32, height: u32) -> Vec<u32> {
    let longest = width.max(height);
    if longest < 2 {
        return Vec::new();
    }
    // The largest power of two strictly below `longest`. `leading_zeros` of
    // `longest - 1` is exact for every `u32` above zero, so no float and no
    // logarithm is involved.
    let mut step = 1_u32 << (31 - (longest - 1).leading_zeros());
    let mut steps = Vec::new();
    loop {
        steps.push(step);
        if step == 1 {
            break;
        }
        step /= 2;
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::{JUMP_FLOOD_ENTRY, JUMP_FLOOD_WGSL, SeedMap, Seeds, jump_flood_steps};
    use crate::compute::WORKGROUP_SIDE;
    use crate::{OffscreenTarget, RenderError};

    /// A target to borrow a device from, or `None` on a machine with no adapter.
    fn target_or_skip() -> Option<OffscreenTarget> {
        match OffscreenTarget::new(8, 8) {
            Ok(target) => Some(target),
            Err(RenderError::NoAdapter { .. }) => None,
            Err(other) => {
                panic!("the offscreen target failed for a reason that is not absence: {other}")
            }
        }
    }

    /// A tiny deterministic generator, so "scattered" is one scatter everywhere.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32
        }
    }

    /// The seed arrangements the oracles run over.
    ///
    /// Generated rather than stored, so the same seeds go in on every machine —
    /// and they are the same seven the M8.3a probe measured, which is what lets
    /// the numbers in this module's header be the numbers these tests assert.
    fn arrangement(name: &str) -> (u32, u32, Vec<(u32, u32)>) {
        match name {
            "single" => (128, 128, vec![(37, 91)]),
            "circle" => {
                let mut seeds = Vec::new();
                for y in 0..128_u32 {
                    for x in 0..128_u32 {
                        let dx = f64::from(x) - 64.0;
                        let dy = f64::from(y) - 64.0;
                        if ((dx * dx + dy * dy).sqrt() - 30.0).abs() < 0.5 {
                            seeds.push((x, y));
                        }
                    }
                }
                (128, 128, seeds)
            }
            "scatter16" => {
                let mut lcg = Lcg(0x5eed_0000_0000_0001);
                let mut seeds = Vec::new();
                while seeds.len() < 16 {
                    let point = (lcg.next() % 128, lcg.next() % 128);
                    if !seeds.contains(&point) {
                        seeds.push(point);
                    }
                }
                (128, 128, seeds)
            }
            "clusters" => {
                let mut seeds = Vec::new();
                for i in 0..8_u32 {
                    seeds.push((20 + i % 3, 60 + i / 3));
                    seeds.push((28 + i % 3, 62 + i / 3));
                }
                seeds.sort_unstable();
                seeds.dedup();
                (128, 128, seeds)
            }
            "wall_and_straggler" => {
                let mut seeds: Vec<(u32, u32)> = (0..128_u32).map(|y| (64, y)).collect();
                seeds.push((127, 64));
                (128, 128, seeds)
            }
            other => panic!("no arrangement called {other}"),
        }
    }

    fn seeds_of(name: &str) -> Seeds {
        let (width, height, points) = arrangement(name);
        let mut seeds = Seeds::new(width, height).expect("the arrangement's size is legal");
        for (x, y) in points {
            seeds.set(x, y).expect("a generated seed is inside");
        }
        seeds
    }

    /// Brute force: for every texel, the nearest seed under the kernel's own
    /// total order — squared distance, then the seed's row, then its column.
    ///
    /// `i64` throughout, so the oracle is not the thing under test.
    fn brute_force(seeds: &Seeds) -> Vec<Option<(u32, u32)>> {
        let mut points = Vec::new();
        for y in 0..seeds.height() {
            for x in 0..seeds.width() {
                if seeds.is_seed(x, y) {
                    points.push((x, y));
                }
            }
        }
        let mut out = Vec::with_capacity(seeds.width() as usize * seeds.height() as usize);
        for y in 0..seeds.height() {
            for x in 0..seeds.width() {
                let mut best: Option<(i64, u32, u32)> = None;
                for &(sx, sy) in &points {
                    let dx = i64::from(sx) - i64::from(x);
                    let dy = i64::from(sy) - i64::from(y);
                    let candidate = (dx * dx + dy * dy, sy, sx);
                    if best.is_none_or(|current| candidate < current) {
                        best = Some(candidate);
                    }
                }
                out.push(best.map(|(_, sy, sx)| (sx, sy)));
            }
        }
        out
    }

    /// How far the seed a texel kept is from the texel, minus how far the
    /// nearest one is. Never negative when the map is compared against brute
    /// force, because brute force is the minimum.
    fn distance_error(x: u32, y: u32, have: Option<(u32, u32)>, want: Option<(u32, u32)>) -> f64 {
        let to = |point: Option<(u32, u32)>| -> f64 {
            match point {
                None => f64::INFINITY,
                Some((sx, sy)) => {
                    let dx = f64::from(sx) - f64::from(x);
                    let dy = f64::from(sy) - f64::from(y);
                    (dx * dx + dy * dy).sqrt()
                }
            }
        };
        to(have) - to(want)
    }

    // -- CPU only ----------------------------------------------------------

    /// The schedule descends by halves from the largest power of two below the
    /// longer side, and ends at one.
    ///
    /// **The guard for two named defects at once.** A chain that stops at two
    /// never lets a seed reach the texel beside it, and an ascending chain
    /// propagates locally before it propagates far. Both were injected and both
    /// were seen to fall here before this test was kept.
    #[test]
    fn the_schedule_descends_by_halves_and_ends_at_one() {
        assert_eq!(jump_flood_steps(128, 128), vec![64, 32, 16, 8, 4, 2, 1]);
        assert_eq!(jump_flood_steps(129, 5), vec![128, 64, 32, 16, 8, 4, 2, 1]);
        assert_eq!(jump_flood_steps(9, 5), vec![8, 4, 2, 1]);
        assert_eq!(jump_flood_steps(2, 1), vec![1]);

        for (width, height) in [(128, 128), (129, 5), (9, 5), (1024, 768), (8192, 2)] {
            let steps = jump_flood_steps(width, height);
            assert_eq!(
                *steps.last().expect("a field above one texel has a step"),
                1,
                "a {width}x{height} chain does not end at 1, so a seed never \
                 reaches the texel beside it"
            );
            assert!(
                steps.windows(2).all(|pair| pair[0] == pair[1] * 2),
                "a {width}x{height} chain is not descending powers of two: {steps:?}"
            );
            assert!(
                steps[0] < width.max(height),
                "a {width}x{height} chain's first step is not below the longer side"
            );
            assert!(
                steps[0] * 2 >= width.max(height),
                "a {width}x{height} chain's first step is more than a halving \
                 below the longer side, so the far corner is unreachable"
            );
        }
    }

    /// A field one texel on its longest side has nothing to propagate.
    #[test]
    fn a_one_texel_field_runs_no_passes() {
        assert!(jump_flood_steps(1, 1).is_empty());
    }

    /// A seed set is its own size, marks only what it was told to, and refuses a
    /// point outside itself.
    #[test]
    fn a_seed_set_marks_only_its_seeds_and_refuses_a_point_outside() {
        let mut seeds = Seeds::new(9, 5).expect("a 9 x 5 seed set");
        assert_eq!((seeds.width(), seeds.height(), seeds.count()), (9, 5, 0));

        seeds.set(0, 0).expect("the corner is inside");
        seeds.set(8, 4).expect("the far corner is inside");
        seeds.set(8, 4).expect("marking one twice is allowed");
        assert_eq!(seeds.count(), 2, "a texel marked twice was counted twice");
        assert!(seeds.is_seed(0, 0) && seeds.is_seed(8, 4));
        assert!(!seeds.is_seed(1, 0) && !seeds.is_seed(9, 4));

        let Err(RenderError::SeedOutsideField {
            x,
            y,
            width,
            height,
        }) = seeds.set(9, 0)
        else {
            panic!("a seed one past the right edge was accepted");
        };
        assert_eq!((x, y, width, height), (9, 0, 9, 5));

        assert!(matches!(
            Seeds::new(0, 5),
            Err(RenderError::InvalidSize { .. })
        ));
        assert!(matches!(
            Seeds::new(1, OffscreenTarget::MAX_DIMENSION + 1),
            Err(RenderError::InvalidSize { .. })
        ));
    }

    /// The texels a seed set starts from say what the shader reads.
    ///
    /// Coordinate, coordinate, one, zero at a seed; four zeros elsewhere. The
    /// second half is the one that matters: an all-zero buffer has to be the
    /// empty seed set, or a field nobody wrote would read as every texel
    /// claiming the seed at the origin.
    #[test]
    fn an_unseeded_texel_is_four_zeros() {
        let mut seeds = Seeds::new(3, 2).expect("a 3 x 2 seed set");
        seeds.set(2, 1).expect("inside");
        let texels = seeds.texels();
        assert_eq!(texels.len(), 3 * 2 * 4);
        assert_eq!(&texels[..20], &[0.0; 20], "an unseeded texel is not zero");
        assert_eq!(&texels[20..], &[2.0, 1.0, 1.0, 0.0]);
    }

    /// Brute force agrees with the closed form where the closed form applies.
    ///
    /// The oracle checked against something simpler than itself, so that a
    /// failure of the GPU tests below can be read as being about the GPU.
    #[test]
    fn brute_force_is_the_closed_form_for_one_seed() {
        let seeds = seeds_of("single");
        let truth = brute_force(&seeds);
        for y in 0..128_u32 {
            for x in 0..128_u32 {
                assert_eq!(
                    truth[y as usize * 128 + x as usize],
                    Some((37, 91)),
                    "with one seed at (37, 91), texel ({x}, {y}) named something else"
                );
            }
        }
    }

    // -- source reads ------------------------------------------------------

    /// The shader's `@workgroup_size` and the dispatch arithmetic agree.
    ///
    /// The same guard `compute.rs` keeps over the transport kernel, and it is
    /// needed once per shader: a shader declaring sixteen against a dispatch
    /// computed for eight would run four times the invocations, each writing the
    /// texel it owns, and the field would be right.
    #[test]
    fn the_kernel_and_the_dispatch_agree_on_the_workgroup() {
        let declared = format!("@workgroup_size({WORKGROUP_SIDE}, {WORKGROUP_SIDE})");
        assert!(
            JUMP_FLOOD_WGSL.contains(&declared),
            "the jump-flooding shader does not declare `{declared}`, which is \
             what `FieldKernel::run` divides the dispatch by"
        );
        assert!(
            JUMP_FLOOD_WGSL.contains(&format!("fn {JUMP_FLOOD_ENTRY}(")),
            "the shader has no `{JUMP_FLOOD_ENTRY}` entry point"
        );
    }

    /// **§2's decision, pinned in the one form that can hold it.**
    ///
    /// A comparison of outputs cannot see the difference between integer and
    /// `f32` arithmetic below the magnitude at which `f32` stops being exact —
    /// M8.3a measured exactly that: the two variants produced byte-identical
    /// fields on six of seven arrangements. What separates them is a squared
    /// distance at or above 2^24, and the field that reaches it is 8192 texels
    /// wide, which no test here builds. So the decision is held by reading the
    /// source, the way `compute.rs` holds M8.0's reproducible subset.
    ///
    /// A literal against a literal, worth one thing: a session that moves this
    /// arithmetic has to move this line too, and then has to say in the commit
    /// message which adapters it re-measured.
    #[test]
    fn the_kernel_compares_in_integer_arithmetic() {
        let body: String = JUMP_FLOOD_WGSL
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            body.contains("var best_d: i32 = 0;"),
            "the kernel's distance accumulator is not declared `i32`, so the \
             comparison M8.3a decided by measurement has moved"
        );
        assert!(
            body.contains("let dx = sx - x;")
                && body.contains("let dy = sy - y;")
                && body.contains("let d = dx * dx + dy * dy;"),
            "the kernel no longer computes its squared distance from integer \
             coordinates in one expression"
        );
        assert!(
            !body.contains("f32(dx)") && !body.contains("f32(dy)"),
            "the kernel converts a coordinate difference to f32 before squaring \
             it, which is the variant M8.3a measured to be wrong above a squared \
             distance of 2^24 and wrong differently on different backends"
        );
        for forbidden in [
            "sqrt",
            "inverseSqrt",
            "pow",
            "exp",
            "log",
            "sin",
            "cos",
            "fma",
            "distance(",
            "length(",
        ] {
            assert!(
                !body.contains(forbidden),
                "the jump-flooding kernel contains `{forbidden}`, which is \
                 outside the subset M8.0 measured as reproducible"
            );
        }
        assert!(
            !body.contains("atomic") && !body.contains("workgroupBarrier"),
            "the jump-flooding kernel reaches for order-dependent arithmetic, \
             which M8.2 measured as irreproducible in 32 of 32 cells"
        );
    }

    // -- GPU ---------------------------------------------------------------

    /// **Oracle (a): one seed against the closed form, exactly.**
    ///
    /// With a single seed there is no approximation left in jump flooding — the
    /// only candidate any probe can offer is that seed — so this is the one case
    /// where the oracle is equality and not a bound. It is also where the
    /// rasterisation caveat below does not apply: the seed *is* a texel, so
    /// `|p - s|` is the closed form rather than an approximation of one.
    #[test]
    fn one_seed_gives_the_closed_form_distance() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let seeds = seeds_of("single");
        let map = target.distance_field(&seeds).expect("the chain runs");

        for y in 0..128_u32 {
            for x in 0..128_u32 {
                assert_eq!(
                    map.nearest(x, y),
                    Some((37, 91)),
                    "with one seed, texel ({x}, {y}) named another"
                );
                let dx = i64::from(x) - 37;
                let dy = i64::from(y) - 91;
                assert_eq!(
                    map.distance_squared(x, y),
                    Some((dx * dx + dy * dy) as u64),
                    "texel ({x}, {y}) derived a squared distance that is not \
                     |p - s|^2"
                );
            }
        }
    }

    /// **Oracle (a) on a circle, against the closed form, within a bound that is
    /// derived from the rasterisation rather than chosen.**
    ///
    /// The signed distance to a circle of centre `c` and radius `r` is
    /// `|p - c| - r`. Jump flooding does not measure that: it measures the
    /// distance to the nearest *seeded texel*, and the seeds are the texels whose
    /// own distance to `c` is within half a texel of `r` — that is what
    /// `arrangement("circle")` selects. So the two differ by at most however far
    /// a seed texel sits from the ideal circle, which is **0.5 texels** by that
    /// selection rule, plus however far jump flooding's answer sits from the
    /// nearest seed, which the next test bounds at **0.2425**. The bound is
    /// therefore `0.5 + 0.2425 = 0.7425`, and it is rounded up to **0.75**.
    ///
    /// Checked outside the ring only. Inside it the nearest seed is on the far
    /// side of nothing, but a texel at the centre is 30 texels from every seed
    /// while `|p - c| - r` is -30, and comparing a magnitude against a signed
    /// value would be comparing two different quantities. Jump flooding computes
    /// an unsigned distance; the sign is a consumer's business and M8.3b's.
    #[test]
    fn a_circle_matches_the_closed_form_within_the_rasterisation_bound() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let seeds = seeds_of("circle");
        let map = target.distance_field(&seeds).expect("the chain runs");

        let mut worst = 0.0_f64;
        let mut worst_at = (0_u32, 0_u32);
        for y in 0..128_u32 {
            for x in 0..128_u32 {
                let dx = f64::from(x) - 64.0;
                let dy = f64::from(y) - 64.0;
                let to_centre = (dx * dx + dy * dy).sqrt();
                if to_centre < 30.0 {
                    continue;
                }
                let closed_form = to_centre - 30.0;
                let measured = f64::from(map.distance(x, y).expect("a seeded field answers"));
                let error = (measured - closed_form).abs();
                if error > worst {
                    worst = error;
                    worst_at = (x, y);
                }
            }
        }
        assert!(
            worst <= 0.75,
            "the field is {worst} from the closed form at {worst_at:?}, past the \
             0.5 texels the rasterisation allows plus the 0.2425 jump flooding \
             was measured to add"
        );
    }

    /// **Oracle (b): jump flooding against brute force, as a measured bound.**
    ///
    /// The plan wrote this down as though equality were the expectation. It is
    /// not, and M8.3a measured the difference rather than assuming either way:
    /// jump flooding is an approximation, exact for one seed and capable of
    /// keeping a farther seed when a nearer one's influence has to travel through
    /// ground another seed already owns.
    ///
    /// Four of the five arrangements come back exact. The rasterised ring does
    /// not: **27 of 16 384 texels**, none of them naming a seed more than
    /// **0.2425 texels** farther than the nearest. Both numbers were identical on
    /// all eight adapter/backend pairs, in both profiles, and the bounds here are
    /// those measurements rounded outward — 32 and 0.5 — so that a machine that
    /// differs a little is a finding rather than a red build for nothing, while a
    /// machine that differs a lot still fails.
    ///
    /// The arrangement built on purpose to be unfavourable, a wall of 128 seeds
    /// with one straggler behind it, is **exact**. That guess was wrong and the
    /// test keeps it so that the next reader does not repeat it.
    #[test]
    fn jump_flooding_agrees_with_brute_force_within_a_measured_bound() {
        let Some(target) = target_or_skip() else {
            return;
        };

        // (name, most texels allowed to differ, most any of them may be farther)
        let bounds = [
            ("single", 0_usize, 0.0_f64),
            ("scatter16", 0, 0.0),
            ("clusters", 0, 0.0),
            ("wall_and_straggler", 0, 0.0),
            ("circle", 32, 0.5),
        ];

        for (name, allowed_texels, allowed_error) in bounds {
            let seeds = seeds_of(name);
            let map = target.distance_field(&seeds).expect("the chain runs");
            let truth = brute_force(&seeds);

            let mut differing = 0_usize;
            let mut worst = 0.0_f64;
            let mut worst_at = (0_u32, 0_u32);
            for y in 0..seeds.height() {
                for x in 0..seeds.width() {
                    let index = y as usize * seeds.width() as usize + x as usize;
                    let have = map.nearest(x, y);
                    let want = truth[index];
                    if have == want {
                        continue;
                    }
                    differing += 1;
                    let error = distance_error(x, y, have, want);
                    assert!(
                        error >= 0.0,
                        "on {name}, texel ({x}, {y}) named a seed *nearer* than \
                         brute force found, so the two are not computing the same \
                         thing"
                    );
                    if error > worst {
                        worst = error;
                        worst_at = (x, y);
                    }
                }
            }

            assert!(
                differing <= allowed_texels,
                "on {name}, {differing} texels name a seed that is not the \
                 nearest, past the {allowed_texels} M8.3a measured"
            );
            assert!(
                worst <= allowed_error,
                "on {name}, texel {worst_at:?} names a seed {worst} texels \
                 farther than the nearest, past the {allowed_error} M8.3a measured"
            );
        }
    }

    /// **Oracle (c): the triangle inequality between neighbouring texels.**
    ///
    /// `|d(a) - d(b)| <= |a - b|`, and for a four-neighbourhood `|a - b|` is
    /// one. Cheap, and it catches almost any propagation fault: a texel that kept
    /// a seed its neighbour could have offered it shows up as a step of more than
    /// one.
    ///
    /// **The bound is 1.25 and not 1, and that is derived from (b) rather than
    /// chosen.** The inequality is exact for the *true* distance field. Jump
    /// flooding over-estimates by at most the 0.2425 texels (b) measured, and an
    /// over-estimate on one side of a pair and not the other adds straight onto
    /// the step — so the reachable bound is `1 + 0.2425 = 1.2425`, rounded
    /// outward to 1.25. It was measured before it was written: the worst step
    /// over the five arrangements is **1.216476** on the ring, between (65, 71)
    /// and (65, 72), and every other arrangement stays at exactly 1. That texel
    /// is the same one (b) reports as its worst over-estimate, which is what
    /// says the two oracles are bounded by one quantity rather than by two.
    ///
    /// A guard at exactly 1 was written first and **fell on that texel**, which
    /// is how the bound came to be measured instead of assumed. It is worth
    /// naming as its own finding: three of the four oracles the plan called
    /// certainties are bounded by jump flooding's approximation, not just (b).
    #[test]
    fn neighbouring_texels_obey_the_triangle_inequality() {
        let Some(target) = target_or_skip() else {
            return;
        };
        for name in [
            "single",
            "circle",
            "scatter16",
            "clusters",
            "wall_and_straggler",
        ] {
            let seeds = seeds_of(name);
            let map = target.distance_field(&seeds).expect("the chain runs");
            let mut worst = 0.0_f64;
            let mut worst_at = ((0_u32, 0_u32), (0_u32, 0_u32));
            for y in 0..seeds.height() {
                for x in 0..seeds.width() {
                    let here = f64::from(map.distance(x, y).expect("a seeded field answers"));
                    for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                        if nx >= seeds.width() || ny >= seeds.height() {
                            continue;
                        }
                        let there =
                            f64::from(map.distance(nx, ny).expect("a seeded field answers"));
                        let step = (here - there).abs();
                        if step > worst {
                            worst = step;
                            worst_at = ((x, y), (nx, ny));
                        }
                    }
                }
            }
            assert!(
                worst <= 1.25,
                "on {name}, {:?} and {:?} are {worst} apart in distance with one \
                 texel between them, past the 1 the true field guarantees plus \
                 the 0.2425 jump flooding was measured to add",
                worst_at.0,
                worst_at.1,
            );
        }
    }

    /// **Oracle (d): adding a seed never raises a distance.**
    ///
    /// A new occluder can only bring the nearest one closer or leave it where it
    /// was. Checked on the squared form, which is exact, so this oracle carries
    /// no tolerance at all.
    ///
    /// **It is checked against jump flooding's own answer both times**, and that
    /// is deliberate: monotonicity is a property of the *field this crate
    /// computes*, so an approximation that violated it would be reported here
    /// even though brute force would not have violated it.
    #[test]
    fn adding_a_seed_never_raises_a_distance() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let before = seeds_of("scatter16");
        let mut after = before.clone();
        after.set(100, 20).expect("inside a 128 x 128 field");
        assert_eq!(
            after.count(),
            before.count() + 1,
            "the added seed was already there, so the test would prove nothing"
        );

        let before_map = target.distance_field(&before).expect("the chain runs");
        let after_map = target.distance_field(&after).expect("the chain runs");

        for y in 0..128_u32 {
            for x in 0..128_u32 {
                let was = before_map
                    .distance_squared(x, y)
                    .expect("a seeded field answers");
                let now = after_map
                    .distance_squared(x, y)
                    .expect("a seeded field answers");
                assert!(
                    now <= was,
                    "adding a seed at (100, 20) moved texel ({x}, {y}) from \
                     {was} to {now}, which is farther"
                );
            }
        }
    }

    /// A field with no seeds names none, everywhere.
    ///
    /// The empty case, and the reason the "no seed" flag is a zero rather than a
    /// negative sentinel: this is what a field nobody wrote would also produce,
    /// so an unwritten field is obviously empty instead of plausibly claiming the
    /// origin.
    #[test]
    fn an_unseeded_field_names_no_seed_anywhere() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let seeds = Seeds::new(9, 5).expect("a 9 x 5 seed set");
        assert_eq!(seeds.count(), 0);

        let map = target.distance_field(&seeds).expect("the chain runs");
        for y in 0..5_u32 {
            for x in 0..9_u32 {
                assert_eq!(
                    map.nearest(x, y),
                    None,
                    "texel ({x}, {y}) of an empty field named a seed"
                );
                assert_eq!(map.distance_squared(x, y), None);
                assert_eq!(map.distance(x, y), None);
            }
        }
    }

    /// Two runs over one seed set agree, and a field is its own size.
    ///
    /// **Nine by five on purpose.** Neither dimension is a multiple of the
    /// workgroup side, so the dispatch rounds up and the last workgroup runs
    /// partly past the edge; and `9 * 16 = 144` bytes a row is not a multiple of
    /// `COPY_BYTES_PER_ROW_ALIGNMENT`, so the read-back has to strip padding.
    /// A size of 8 x 8 would have hidden both.
    #[test]
    fn two_runs_over_one_seed_set_agree() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let mut seeds = Seeds::new(9, 5).expect("a 9 x 5 seed set");
        for (x, y) in [(0, 0), (8, 4), (4, 2)] {
            seeds.set(x, y).expect("inside");
        }

        let first = target.distance_field(&seeds).expect("the chain runs");
        let second = target.distance_field(&seeds).expect("the chain runs again");

        assert_eq!((first.width(), first.height()), (9, 5));
        assert_eq!(
            first, second,
            "two runs over one seed set produced two fields"
        );

        // And the answer is the brute-force one, which at this size is exact.
        let truth = brute_force(&seeds);
        for y in 0..5_u32 {
            for x in 0..9_u32 {
                assert_eq!(
                    first.nearest(x, y),
                    truth[y as usize * 9 + x as usize],
                    "texel ({x}, {y}) of a 9 x 5 field is not the brute-force answer"
                );
            }
        }
    }

    /// A seed map read back from texels is the map those texels describe.
    ///
    /// The inverse of `Seeds::texels`, on its own and with no GPU in the way, so
    /// that a failure above can be read as being about the chain rather than
    /// about the two conversions on either side of it.
    #[test]
    fn a_seed_map_is_the_texels_it_was_read_from() {
        let map = SeedMap::from_texels(
            2,
            2,
            &[
                3.0, 4.0, 1.0, 0.0, //
                0.0, 0.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0, //
                7.0, 1.0, 1.0, 0.0,
            ],
        );
        assert_eq!(map.nearest(0, 0), Some((3, 4)));
        assert_eq!(map.nearest(1, 0), None, "a zero flag is not `no seed`");
        assert_eq!(
            map.nearest(0, 1),
            Some((0, 0)),
            "a seed at the origin was read as absent"
        );
        assert_eq!(map.nearest(1, 1), Some((7, 1)));
        assert_eq!(map.nearest(2, 0), None, "a point outside answered");

        // (7 - 1)^2 + (1 - 1)^2 = 36, and its root is exactly six.
        assert_eq!(map.distance_squared(1, 1), Some(36));
        assert_eq!(map.distance(1, 1), Some(6.0));
    }
}
