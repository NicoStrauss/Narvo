//! Placing one sprite: world space to normalised device coordinates.
//!
//! Pure arithmetic. Nothing here touches `wgpu`, so every claim it makes is
//! testable without an adapter, and the one test that does need a GPU is left
//! to assert what only a rendered image can show.
//!
//! # Where the y directions are reconciled
//!
//! In exactly one place, and it is not in this file. ADR-0004 requires the
//! reconciliation to happen once; this module is what makes that affordable,
//! by *not* being a second place:
//!
//! - **World space and NDC agree.** Both put y up (ADR-0004, world-space
//!   amendment). [`Projection::world_to_ndc`] is therefore a scale, with no
//!   negation on either axis — the assertion `no_axis_is_negated_on_the_way_to_ndc`
//!   below is what keeps it that way.
//! - **NDC and texture coordinates disagree**, and that is reconciled by
//!   pairing the world-space *top* corners with `v = 0.0` in
//!   [`SPRITE_CORNERS`], which is the same pairing the screen-filling quad's
//!   `VERTICES` in `quad.rs` has always used.
//! - **NDC and the framebuffer disagree**, and that is reconciled by the
//!   fixed-function viewport transform inside the GPU, which is not our code.
//!
//! Since M3.9 a fourth pairing joins them, and it is deliberately not a fourth
//! reconciliation: a [`TextureRegion`] measures its top edge from the texture's
//! top row, the same direction `v` already runs, so the region composes with
//! the pairing above instead of arguing with it.

use std::fmt;

use crate::offscreen::Pixels;

/// Where a sprite sits, how it is turned and how big it is, in world units.
///
/// The renderer takes scalars rather than a component because it does not depend
/// on `narvo-ecs` and should not have to: ADR-0014 has registered components
/// hold bare scalars precisely so that they can cross a boundary like this one
/// without dragging a type with them.
///
/// # The rotation is a `(cos, sin)` pair, not an angle (M5b.4)
///
/// Until M5b.4 this was five fields, one per field of `narvo_ecs::Transform`,
/// and the rotation was a scalar angle that [`sprite_vertices`] resolved with
/// [`f32::sin_cos`]. ADR-0015 named the widening path — *"a renderer that needs
/// more per sprite widens `SpritePlacement`"* — and a rigid body is the consumer
/// that needed it.
///
/// **`narvo_ecs::RigidBody` holds its rotation as a `(cos, sin)` pair**, because
/// that is what rapier holds and because the round trip through an angle is
/// `atan2` out and `sin`/`cos` back — standard-library trigonometry, which
/// `enhanced-determinism` does not reach, and which M5b.2 measured to be lossy on
/// Windows and exact on Linux for one and the same value. That component's own
/// documentation states the consequence for this side: *"A consumer that wants a
/// `Transform`-shaped pose converts at its own call site, outside the world."*
///
/// Storing the angle here would have made the render path that call site, and the
/// conversion would then sit **between a body and its pixels**. That is outside
/// the state hash (ADR-0005: the render path only reads) and therefore outside
/// what a determinism dump can see — but a golden image is rendered on both
/// platforms and compared against one committed reference, so a platform-
/// dependent angle would land in pixels instead, where the instrument that
/// notices is a red reference on one side only.
///
/// So the pair travels: **from `RigidBody`'s `rot_cos`/`rot_sin`, through these
/// two fields, into the corner arithmetic, without a trigonometric operation
/// anywhere on the way.** The two fields carry the component's own names for
/// exactly that reason.
///
/// A caller that holds an *angle* — every `Transform` does — converts with
/// [`turned`](Self::turned), which is the same [`f32::sin_cos`] call the renderer
/// used to make, one stage earlier and now visible at the call site. Nothing
/// about the resulting pixels changed: `0.0_f32.sin_cos()` is `(0.0, 1.0)`
/// exactly, so an unturned sprite goes through the identical arithmetic, which
/// `the_untuned_pair_is_what_sin_cos_of_zero_returns` asserts on bit patterns
/// rather than argues.
///
/// **Nothing normalises the pair.** A caller that writes `(2.0, 0.0)` gets a
/// sprite scaled by two along its own axes, and one that writes `(0.0, 0.0)` gets
/// a sprite collapsed to a point. That is the same refusal `Transform` makes
/// about its rotation and `Layer` about a `NaN` depth: storing something other
/// than what was written puts a step between a caller and its own state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpritePlacement {
    /// Centre along x, growing to the right.
    pub x: f32,
    /// Centre along y, growing upward.
    pub y: f32,
    /// Cosine of the rotation, positive counter-clockwise.
    pub rot_cos: f32,
    /// Sine of the rotation, positive counter-clockwise.
    pub rot_sin: f32,
    /// Width in world units. Negative mirrors.
    pub scale_x: f32,
    /// Height in world units. Negative mirrors.
    pub scale_y: f32,
}

impl SpritePlacement {
    /// The pair that means "not turned": `(cos, sin)` of an angle of zero.
    pub const UNTURNED: (f32, f32) = (1.0, 0.0);

    /// A sprite of `width` x `height` world units at the origin, unturned.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            rot_cos: Self::UNTURNED.0,
            rot_sin: Self::UNTURNED.1,
            scale_x: width,
            scale_y: height,
        }
    }

    /// The same placement turned by an angle in radians.
    ///
    /// The one trigonometric call left in this crate, and it is here rather than
    /// in [`sprite_vertices`] so that a caller which already holds a pair never
    /// reaches it. See the type's documentation for why that distinction is
    /// load-bearing.
    #[must_use]
    pub fn turned(self, radians: f32) -> Self {
        let (rot_sin, rot_cos) = radians.sin_cos();
        Self {
            rot_cos,
            rot_sin,
            ..self
        }
    }
}

/// The sprite's own corners, as `[x, y, u, v]`, before any placement.
///
/// A unit square centred on its own origin, so [`SpritePlacement::scale_x`] is
/// the sprite's width in world units rather than a multiple of some other size.
///
/// The order matches `INDICES` in `quad.rs`, and the pairing of position with
/// texture coordinate is the reconciliation named in this module's
/// documentation: world `y = +0.5` — the top — is `v = 0.0`, because texture
/// rows run downward from the top left while world y runs up.
///
/// `pub(crate)` for symmetry with `quad::VERTICES`: both are read by the
/// convention guard in this module's tests and by nothing else outside their
/// own file.
pub(crate) const SPRITE_CORNERS: [[f32; 4]; 4] = [
    [-0.5, 0.5, 0.0, 0.0],  // top left
    [-0.5, -0.5, 0.0, 1.0], // bottom left
    [0.5, -0.5, 1.0, 1.0],  // bottom right
    [0.5, 0.5, 1.0, 0.0],   // top right
];

/// The part of a texture a sprite samples, as normalised coordinates.
///
/// # Texels in, normalised coordinates stored
///
/// An atlas is authored in texels — "the tree is the eight by eight block at
/// (8, 0)" — but the shader samples in normalised `uv`. Somebody has to divide
/// by the texture size, and **the whole design of this type is about that
/// division happening exactly once.**
///
/// It happens in [`TextureRegion::from_texels`], the only place in the crate
/// that turns texel coordinates into normalised ones, and it happens at
/// construction rather than at draw time. What is stored is already normalised,
/// so no later stage can convert a second time or forget to convert at all:
/// there is nothing left to convert. The fields are private for that reason —
/// a public `u_left` invites a caller to build one from texel numbers by hand,
/// which is the same class of mistake as a second y-flip (ADR-0004) and just as
/// invisible in a symmetric fixture.
///
/// `from_texels` takes the texture rather than two loose dimensions, so the two
/// numbers cannot be swapped or belong to different images. **It does not
/// establish that they are the right numbers**: nothing ties the texture a
/// region was measured against to the one
/// [`OffscreenTarget::render_sprites`](crate::OffscreenTarget::render_sprites)
/// later binds, so a region of one atlas drawn against another is silently
/// wrong. M3.9 reports that gap rather than closing it, because closing it
/// means storing the size in the region and rejecting a mismatch, which is a
/// new error variant and a decision of its own.
///
/// # Which way its y runs, and where that meets `SPRITE_CORNERS`
///
/// **Down, with the texture rows.** `top` is measured from the texture's top
/// row, as `v` is (ADR-0004). A region measured from the bottom would be a
/// second reconciliation of the two y directions, which that ADR forbids, and
/// it would be invisible against any texture symmetric in y.
///
/// It meets [`SPRITE_CORNERS`] in [`TextureRegion::sample_at`]: the table's
/// `v = 0.0` — carried by the corner at local `y = +0.5`, the sprite's top —
/// maps to the region's *top* edge, and `v = 1.0` to its bottom. The table
/// keeps the pairing it always had; the region only says which rows `0` and `1`
/// now mean. `every_corner_table_pairs_the_top_with_the_top_texture_row` in
/// this module's tests still reads that table unchanged.
///
/// # Edges, not centres
///
/// A boundary sits on a texel *edge*: `from_texels(8, 0, 8, 8)` of a 16 x 16
/// texture is `u` from `0.5` to `1.0`, not from `8.5/16` to `15.5/16`. Two
/// consequences, both wanted: adjacent regions tile without a gap and without
/// an overlap, and a region's extent in texels is the number it was given.
/// Sample points land at pixel centres, so with `Nearest` the first sampled
/// point of a region sits half an output pixel inside it and never on the edge
/// itself — the derivation for the M3.9 scene is in that task's report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureRegion {
    /// Left edge, normalised: `0.0` is the texture's left.
    u_left: f32,
    /// Top edge, normalised: `0.0` is the texture's **top** row.
    v_top: f32,
    /// Right edge, normalised.
    u_right: f32,
    /// Bottom edge, normalised: `1.0` is the texture's last row.
    v_bottom: f32,
}

impl TextureRegion {
    /// The whole texture — the case every sprite drew before M3.9.
    ///
    /// Not a second path and not a flag: it is an ordinary region whose edges
    /// happen to be the texture's own. [`TextureRegion::sample_at`] with these
    /// values is `0.0 + u * 1.0`, which returns `u` and `v` unchanged **bit for
    /// bit**, so a sprite drawn with it produces the vertices the corner table
    /// holds. `the_whole_texture_region_leaves_the_corner_table_untouched`
    /// below asserts exactly that, on bit patterns.
    pub const WHOLE_TEXTURE: Self = Self {
        u_left: 0.0,
        v_top: 0.0,
        u_right: 1.0,
        v_bottom: 1.0,
    };

    /// The `width` x `height` block of `texture` whose top left texel is
    /// (`left`, `top`).
    ///
    /// `left` and `top` count from the texture's top left, the same origin
    /// `Pixels::pixel`, PNG rows and `v` use (ADR-0004). Coordinates are texel
    /// edges, so `from_texels(0, 0, w, h, t)` for `t`'s own size is
    /// [`WHOLE_TEXTURE`](Self::WHOLE_TEXTURE) exactly.
    ///
    /// **A region reaching past the texture is not rejected here.** Inside the
    /// range where `left + width` does not overflow `u32` it produces
    /// coordinates above `1.0`, which the sampler clamps to the edge
    /// (`AddressMode::ClampToEdge` in `quad.rs`). At the very top of the range
    /// the addition itself is the limit: `left + width` panics in a build with
    /// overflow checks and wraps without them, and a wrapped `u_right` below
    /// `u_left` is an inverted region — a silent mirror rather than a clamp.
    /// Rejecting either would need an error variant and a decision about what a
    /// partially valid region means; M3.9 reports both gaps rather than
    /// deciding them in passing.
    #[must_use]
    pub fn from_texels(left: u32, top: u32, width: u32, height: u32, texture: &Pixels) -> Self {
        // The crate's only texel-to-normalised conversion, called four times
        // here and nowhere else. `f64` for the divide so that the numerator is
        // still exact above 2^24 — the largest integer `f32` counts in whole
        // numbers — which matters for an out-of-range `left + width` rather
        // than for a texture, since `Pixels` caps each dimension at
        // `OffscreenTarget::MAX_DIMENSION`. The result is narrowed once, at the
        // end.
        let normalise = |numerator: u32, denominator: u32| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a ratio of two texel counts is small; the f64 divide is what \
                          keeps the numerator exact, not the width of the result"
            )]
            {
                (f64::from(numerator) / f64::from(denominator)) as f32
            }
        };

        Self {
            u_left: normalise(left, texture.width()),
            v_top: normalise(top, texture.height()),
            u_right: normalise(left + width, texture.width()),
            v_bottom: normalise(top + height, texture.height()),
        }
    }

    /// The region's edges as `[u_left, v_top, u_right, v_bottom]`.
    ///
    /// The private fields' one way out. Tests assert against it, and a caller
    /// that has to hand these to something else can, without being able to
    /// construct an unnormalised region on the way back in.
    #[must_use]
    pub const fn uv_bounds(self) -> [f32; 4] {
        [self.u_left, self.v_top, self.u_right, self.v_bottom]
    }

    /// Where a corner of the unit square samples, inside this region.
    ///
    /// `u` and `v` come from [`SPRITE_CORNERS`] and are `0.0` or `1.0` today:
    /// `v = 0.0` is the sprite's top corner and maps to the region's top edge.
    /// With [`WHOLE_TEXTURE`](Self::WHOLE_TEXTURE) the edges are `0.0` and
    /// `1.0`, so this reduces to `0.0 + u * 1.0`, which returns both endpoints
    /// unchanged bit for bit — every step of it, including the subtraction, is
    /// exact in binary floating point.
    fn sample_at(self, u: f32, v: f32) -> [f32; 2] {
        [
            self.u_left + u * (self.u_right - self.u_left),
            self.v_top + v * (self.v_bottom - self.v_top),
        ]
    }
}

/// One sprite to draw: where it goes, and which part of the texture it shows.
///
/// # The naming convention, and why this type carries its role (M4.9)
///
/// **A component owns the plain name; a renderer type carries its role.** This
/// was `Sprite` until M4.9, when `narvo_ecs::Sprite` became the eighth
/// registered component and the two collided — `narvo-app` sees both, and it
/// spent a milestone importing one of them as `Sprite as SpriteComponent`.
///
/// The plain name went to the component, and this took the qualified one, for a
/// reason that is not seniority: **the component is what content says.** A
/// scene file writes `"sprite"`, a registry knows it under that name, and a
/// state hash covers it — so its Rust name should be the word an author already
/// uses. This type is an *instance handed to a draw call*: a placement, a
/// region and a sampler wish, assembled per frame and gone by the next. Naming
/// it for that is what makes the two readable side by side, and `narvo-app`
/// now imports both plainly.
///
/// The rename touched no registry name, no scene file, no blessed reference and
/// no state hash — the eight golden comparisons were the instrument that said
/// so.
///
/// # Why a pair rather than five more fields on `SpritePlacement`
///
/// [`SpritePlacement`] is where a sprite sits, how it is turned and how big it
/// is, in world-space scalars (ADR-0015). A texture region is not a property of a
/// placement, and putting it there would have two costs that are easy to miss:
/// `narvo-app` builds `SpritePlacement` field by field, so another field makes
/// this a two-crate change, which CLAUDE.md says to stop and report rather than
/// push through; and the type would then mean two things at once, so the next
/// reader has to ask which of its fields the ECS supplies.
///
/// M5b.4 paid that two-crate price once, deliberately and for the *pose* rather
/// than for the region: the rotation became a `(cos, sin)` pair so that a rigid
/// body reaches the corner arithmetic without a trigonometric operation. That is
/// the widening ADR-0015 names, and it is an argument about the pose the type
/// already carries — not a counter-example to the paragraph above.
///
/// **Rejected alternatives**, both of which keep `SpritePlacement` intact:
///
/// - *Two parallel slices*, `&[SpritePlacement]` and `&[TextureRegion]`. It
///   makes a length mismatch expressible and a silent off-by-one between the
///   two indexable, and there is no reason to pay that for a struct with two
///   fields.
/// - *A region on the draw call rather than on the sprite*, one region per
///   batch. That is the atlas's whole point inverted: the reason to have
///   regions is that one texture serves sprites that look different.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteInstance {
    /// Where it sits, how it is turned, how big it is.
    pub placement: SpritePlacement,
    /// Which part of the bound texture it samples.
    pub region: TextureRegion,
    /// How it wants that part sampled.
    ///
    /// **Honoured since M3.23**, both variants. Until then the renderer drew
    /// `Nearest` only and refused a batch that asked for anything else, so this
    /// was a wish rather than an instruction; `RenderError::UnsupportedFilter`
    /// was that refusal and is gone, because with both variants honoured there
    /// is no unsupported filter left for it to name.
    ///
    /// `batch_runs` cuts the drawing order wherever this changes, and each run
    /// is drawn with its own sampler — so a batch may mix the two freely and the
    /// visible order is still the sequence order.
    pub filter: SpriteFilter,
    /// The colour its texels are multiplied by.
    ///
    /// **Unlike [`Self::filter`], this cuts no run.** A sampler is pipeline-
    /// adjacent state that has to be bound, so a batch mixing two of them costs
    /// two draw calls; a tint is a vertex attribute, so a batch of a thousand
    /// sprites with a thousand different tints is still one draw call. That is
    /// the whole reason it lives in the vertex buffer rather than in a uniform:
    /// a uniform would have made the tint a second thing `batch_runs` cuts on,
    /// and D15's cut would have become a colour policy.
    ///
    /// It is simulation state and lives in the world, for the reason
    /// [`SpriteFilter`] gives: a wish that changes the image and lives only in
    /// render state would make a replay draw a different picture with the state
    /// hash blind to it. `narvo_ecs::Tint` carries it, and `narvo-view2d`'s
    /// `tint_of` is the translation (ADR-0015).
    pub tint: SpriteTint,
}

/// How a sprite wants its texels sampled.
///
/// **Both variants are honoured since M3.23.** Until then the renderer built one
/// sampler and refused a batch asking for anything else; it now builds one per
/// variant and `batch_runs` cuts the drawing order wherever this changes, so a
/// batch may mix them and each run is drawn with its own.
///
/// # It is simulation state, and lives in the world
///
/// A wish that changes the image and lives only in render state would make a
/// replay draw a different picture with the state hash blind to it. So it is
/// carried by `narvo_ecs::Sampling` — a bare `u8` with a written-down mapping
/// rather than this enum, because ADR-0014 keeps serde's choice of enum
/// representation out of the hash domain ADR-0008 defines. The translation lives
/// in `narvo-app`'s `filter_of`, the crate that sees both sides (ADR-0015).
///
/// M3.20 introduced this type and recorded that obligation as "not built",
/// because with one sampler the wish could not change a picture. M3.23 built the
/// second sampler, which is what made it fall due.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpriteFilter {
    /// One texel, unblended.
    #[default]
    Nearest,
    /// A bilinear blend of the four nearest texels.
    Linear,
}

impl SpriteFilter {
    /// This filter's position in the renderer's sampler table.
    ///
    /// Declaration order, and the two places that depend on it sit next to each
    /// other: `QuadPipeline` builds its sampler array in this order and
    /// `TextureBindings` indexes with this.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Nearest => 0,
            Self::Linear => 1,
        }
    }
}

/// A colour multiplied into a sprite's texels.
///
/// # Where the multiplication happens, which decides the space it happens in
///
/// In the fragment shader, on the value the sampler returned — and that fixes
/// the space rather than leaving it open. The atlas texture and the render
/// target are both `Rgba8UnormSrgb` (`quad.rs`'s `bind_texture` and
/// `offscreen.rs`'s `TARGET_FORMAT`), which wgpu documents as "Srgb-color
/// [0, 255] converted to/from linear-color float [0, 1] in shader"
/// (`wgpu-types-30.0.0/src/texture/format.rs:186`). The sample is therefore
/// already linear light when it reaches the multiply, and re-encoded on write.
///
/// **Measured rather than argued**, because the two readings are far apart and
/// a single render separates them: a white texel under a tint of `0.5` reads
/// back as **188** if the space is linear and as **128** if it is encoded.
/// `the_half_tint_lands_where_a_linear_multiply_puts_it` renders exactly that.
///
/// This is not ADR-0024 Decision 3 reopened. That decision governs
/// premultiplying an atlas's straight-alpha *bytes* at load, in integer
/// arithmetic, so the result can be hashed; this is a per-draw multiply in
/// `f32` that never touches a stored byte. The two operations sit on opposite
/// sides of the upload and neither moves the other.
///
/// # Premultiplied, like everything else the pipeline carries
///
/// ADR-0023 makes the pipeline consume premultiplied colour, whose invariant is
/// `rgb <= a`. A tint with an alpha of its own must therefore reach the colour
/// channels too, or a half-transparent tint would leave a fragment brighter
/// than its own coverage — the invariant would break exactly at the edges it
/// exists to protect.
///
/// Writing it out from straight alpha, which is where the factor comes from:
/// a premultiplied source `(C, A)` is the straight colour `c = C / A` at
/// coverage `A`. Tinting means `c * t` at coverage `A * t_a`, and putting that
/// back into premultiplied form gives
///
/// ```text
/// out_rgb = (C / A) * t_rgb * (A * t_a) = C * t_rgb * t_a
/// out_a   = A * t_a
/// ```
///
/// So the factor applied to the colour channels is `t_rgb * t_a` and the factor
/// applied to alpha is `t_a` — which is exactly the tint in premultiplied form.
/// [`Self::premultiplied`] builds it, on the CPU, once per sprite rather than
/// once per fragment, and the shader is then a plain component-wise product.
///
/// # The invariant it preserves, and the one condition
///
/// `out_rgb = C * t_rgb * t_a <= A * t_a = out_a` holds whenever `C <= A` and
/// `t_rgb <= 1`. The first is what the pipeline already guarantees; the second
/// is a condition on this type, and a channel above one breaks it. That is
/// stated as a limit rather than enforced: clamping here would silently change
/// a caller's value, and rejecting would make a colour a fallible thing to
/// hold. `a_tint_above_one_is_the_named_limit_and_not_a_promise` measures the
/// case rather than leaving it to a reader's confidence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteTint {
    /// The red multiplier.
    pub red: f32,
    /// The green multiplier.
    pub green: f32,
    /// The blue multiplier.
    pub blue: f32,
    /// The alpha multiplier, which scales coverage and colour alike.
    pub alpha: f32,
}

impl SpriteTint {
    /// The tint that changes nothing.
    ///
    /// All ones, and the identity **exactly**: IEEE-754 multiplication by `1.0`
    /// returns its other operand unchanged for every finite value, so a sprite
    /// carrying this draws the pixels it drew before this type existed. That is
    /// what lets a tint be added to the vertex format without moving a blessed
    /// reference, and it is asserted rather than assumed by
    /// `the_untinted_batch_is_the_untinted_batch_bit_for_bit`.
    pub const UNTINTED: Self = Self {
        red: 1.0,
        green: 1.0,
        blue: 1.0,
        alpha: 1.0,
    };

    /// An opaque tint of the three colour channels.
    #[must_use]
    pub const fn rgb(red: f32, green: f32, blue: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    /// This tint in the pipeline's own representation, as `[r, g, b, a]`.
    ///
    /// `[red * alpha, green * alpha, blue * alpha, alpha]` — the derivation is
    /// on the type. Multiplying a premultiplied texel by this is the whole of
    /// what tinting does.
    ///
    /// On the CPU and once per sprite, not once per fragment: the four vertices
    /// of a quad carry the same value, so this runs four times per sprite and
    /// the shader runs a product per fragment. The shader could do it instead,
    /// and the reason it does not is that the pipeline's representation is
    /// premultiplied everywhere else too — an attribute holding a straight tint
    /// would be the one value in the vertex buffer that is not what the blend
    /// state expects.
    #[must_use]
    pub const fn premultiplied(self) -> [f32; 4] {
        [
            self.red * self.alpha,
            self.green * self.alpha,
            self.blue * self.alpha,
            self.alpha,
        ]
    }
}

impl Default for SpriteTint {
    /// [`SpriteTint::UNTINTED`].
    ///
    /// Written out rather than derived: `f32::default()` is `0.0`, so a derived
    /// default would be a sprite nobody can see.
    fn default() -> Self {
        Self::UNTINTED
    }
}

impl SpriteInstance {
    /// A sprite showing `region` of the texture, sampled `Nearest` and
    /// untinted.
    ///
    /// The default is what every sprite did before this constructor gained a
    /// sibling, so no existing caller changes behaviour by not mentioning a
    /// filter.
    #[must_use]
    pub const fn new(placement: SpritePlacement, region: TextureRegion) -> Self {
        Self {
            placement,
            region,
            filter: SpriteFilter::Nearest,
            tint: SpriteTint::UNTINTED,
        }
    }

    /// The same sprite, wishing for `filter`.
    #[must_use]
    pub const fn sampled(self, filter: SpriteFilter) -> Self {
        Self { filter, ..self }
    }

    /// The same sprite, multiplied by `tint`.
    ///
    /// Named to sit beside [`Self::sampled`], because it is the same shape of
    /// thing: a per-sprite property the world owns, carried through the batch
    /// and honoured by the one pipeline.
    #[must_use]
    pub const fn tinted(self, tint: SpriteTint) -> Self {
        Self { tint, ..self }
    }

    /// A sprite showing the whole texture — what every sprite did before M3.9.
    ///
    /// The same code path as any other region, with
    /// [`TextureRegion::WHOLE_TEXTURE`]. This exists so that a caller with no
    /// atlas writes no atlas vocabulary, not so that it can take a shortcut.
    #[must_use]
    pub const fn whole_texture(placement: SpritePlacement) -> Self {
        Self::new(placement, TextureRegion::WHOLE_TEXTURE)
    }
}

/// The border a padded atlas puts around every region, in texels.
///
/// Padding is D13's decision (`ProjektPlan.md` §11: "`Linear` mit Padding");
/// **the width of one texel is not in it** — D13's entry names no texel count.
/// One comes from M3.17, where it was measured: a one-texel border of
/// duplicated edge texels moves the
/// bilinear blend partner at a region edge from a neighbouring region's texel
/// to the region's own, and sprite B's measured left edge went from 48.3760 to
/// exactly 48.0000. One is also the smallest border that does anything at all,
/// because bilinear reaches exactly one texel past the sample point.
///
/// It is a constant rather than a per-atlas number because nothing today
/// chooses: every padded fixture in the repository uses one texel, and a
/// second value would need a reason to exist. [`check_region_padding`] still
/// takes the border as a parameter, so the constant is the default the callers
/// agree on and not a limit built into the check.
pub const REGION_PADDING_TEXELS: u32 = 1;

/// Which part of a region's border a texel belongs to.
///
/// Eight cases rather than four: the corners are their own, because a corner
/// texel copies the content's corner and a wrong one there is the mistake an
/// edge-only loop makes. A failure names this so the reader is pointed at a
/// side of the region instead of at a bare coordinate pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionEdge {
    /// Left of the content block, level with it.
    Left,
    /// Right of the content block, level with it.
    Right,
    /// Above the content block, within its columns.
    Top,
    /// Below the content block, within its columns.
    Bottom,
    /// Above and left of the content block.
    TopLeft,
    /// Above and right of the content block.
    TopRight,
    /// Below and left of the content block.
    BottomLeft,
    /// Below and right of the content block.
    BottomRight,
}

impl RegionEdge {
    /// The name used in a failure message.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left edge",
            Self::Right => "right edge",
            Self::Top => "top edge",
            Self::Bottom => "bottom edge",
            Self::TopLeft => "top-left corner",
            Self::TopRight => "top-right corner",
            Self::BottomLeft => "bottom-left corner",
            Self::BottomRight => "bottom-right corner",
        }
    }
}

impl fmt::Display for RegionEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a region's claim to be padded does not hold.
///
/// `#[non_exhaustive]`, as `RenderError` and `GoldenError` both are: a third
/// kind of defect is a plausible addition and should not be a breaking change.
///
/// Returned by [`check_region_padding`], and deliberately not a
/// [`RenderError`](crate::RenderError) variant: nothing in the render path can
/// produce one. A padding defect is a fault in *content*, found by a caller
/// that chose to look; putting it in the error the draw calls return would say
/// that `render_sprites` can hand one back, and it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PaddingDefect {
    /// The padded footprint does not fit inside the texture.
    ///
    /// A region **flush against** the atlas rim — `left` or `top` at 0, or a
    /// far edge at the texture's own — cannot carry a border, so the claim is
    /// false before a single texel is read. A region *one* texel from the rim
    /// is the opposite case: it is exactly what a one-texel border fits, and
    /// `padded_block()` in this file's tests is that region. Reported rather
    /// than skipped: silently checking the three borders that do fit is how a
    /// guard passes while the property it names is broken.
    NoRoom {
        /// The content block's left edge, in texels.
        left: u32,
        /// The content block's top edge, in texels.
        top: u32,
        /// The content block's width, in texels.
        width: u32,
        /// The content block's height, in texels.
        height: u32,
        /// The border that was claimed, in texels.
        border: u32,
        /// The texture's width, in texels.
        texture_width: u32,
        /// The texture's height, in texels.
        texture_height: u32,
    },
    /// The region has no texels: a zero width or a zero height.
    ///
    /// Its own case rather than [`NoRoom`](Self::NoRoom), because the footprint
    /// of an empty region can fit perfectly well — `(1, 1)` 0 x 8 in a 10 x 10
    /// texture does — and saying "no room" of it would be a precise-sounding
    /// falsehood. There is no content texel for a border texel to copy, which
    /// is the actual complaint.
    EmptyRegion {
        /// The content block's width, in texels.
        width: u32,
        /// The content block's height, in texels.
        height: u32,
    },
    /// A border texel is not a copy of the content texel nearest to it.
    WrongTexel {
        /// Which side of the region the offending texel sits on.
        edge: RegionEdge,
        /// The offending border texel's column.
        x: u32,
        /// The offending border texel's row.
        y: u32,
        /// The column of the content texel it has to copy.
        source_x: u32,
        /// The row of the content texel it has to copy.
        source_y: u32,
        /// What the border texel holds.
        found: [u8; 4],
        /// What the content texel holds, and therefore what it must hold.
        expected: [u8; 4],
    },
}

impl fmt::Display for PaddingDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NoRoom {
                left,
                top,
                width,
                height,
                border,
                texture_width,
                texture_height,
            } => write!(
                f,
                "the region at ({left}, {top}), {width} x {height} texels, has no room for a \
                 border of {border}: its padded footprint would run from ({}, {}) to ({}, {}) \
                 in a texture of {texture_width} x {texture_height}",
                i64::from(left) - i64::from(border),
                i64::from(top) - i64::from(border),
                i64::from(left) + i64::from(width) + i64::from(border),
                i64::from(top) + i64::from(height) + i64::from(border),
            ),
            Self::EmptyRegion { width, height } => write!(
                f,
                "a region of {width} x {height} texels has no content for a border to copy"
            ),
            Self::WrongTexel {
                edge,
                x,
                y,
                source_x,
                source_y,
                found,
                expected,
            } => write!(
                f,
                "border texel ({x}, {y}) on the region's {edge} must copy content texel \
                 ({source_x}, {source_y}): expected {expected:?}, found {found:?}"
            ),
        }
    }
}

impl std::error::Error for PaddingDefect {}

/// Checks that the border around a region really is duplicated edge texels.
///
/// The arguments are [`TextureRegion::from_texels`]' own, in the same order,
/// plus the border: a caller writes the region and its check from one set of
/// numbers, so the two cannot drift into describing different rectangles.
/// `left`, `top`, `width` and `height` are the **content** block — padding does
/// not move a region's coordinates, it surrounds them.
///
/// Every texel of the `border`-wide frame is compared against the content texel
/// nearest to it, which is the content block's own corner for a corner texel
/// and the nearest edge texel otherwise. All four edges and all four corners,
/// which is why [`RegionEdge`] has eight cases.
///
/// # Why this is not on `TextureRegion`, and not inside `from_texels`
///
/// A [`TextureRegion`] cannot answer it. Its fields are normalised `f32` and
/// private on purpose (M3.9: "there is nothing left to convert"), so asking one
/// for its texels means multiplying by the texture's size and rounding — undoing
/// a narrowing conversion to recover integers that were thrown away deliberately.
///
/// And `from_texels` reads only `texture.width()` and `texture.height()`, never
/// a texel. Folding the check into it would change its cost from constant to
/// proportional to the region's perimeter and give a constructor that cannot
/// fail a failure mode, on every region including the unpadded ones — which are
/// legal (see below).
///
/// # An unpadded region is not an error
///
/// Nothing here or anywhere else requires a region to be padded, and three
/// reasons keep it that way:
///
/// - [`TextureRegion::WHOLE_TEXTURE`] can never be padded. Its border would
///   have to lie outside the texture. **One** of the five blessed images draws
///   it — `placed_sprite_quadrants_128x128`, through `render_sprites` (through
///   `render_sprite` until M3.27, which moved the call to set the wish). **And
///   since M3.27 it draws it at `Linear`**, which is the combination D13's
///   padding exists to make safe — safe here without padding because a whole
///   texture has no neighbouring region to bleed from, and
///   `AddressMode::ClampToEdge` makes the texel one step past the rim a copy of
///   the rim's own (`quad.rs:422-424` for the sampler,
///   `shaders/quad.wgsl:55-56` for where that step is recorded; M3.27 measured
///   the consequence on the blessed scene, whose rim pixels did not move).
///   (A second,
///   `textured_quad_quadrants_64x64`, draws a whole texture too but builds no
///   `TextureRegion` at all: `render_textured_quad` binds the quad pipeline's
///   own vertices. `offscreen.rs` records that conflating the two is a mistake
///   made once already and corrected in M3.9.) One image is enough: rejecting
///   unpadded regions would make the crate's own documented base case illegal.
/// - There is nowhere to put a warning. This crate's `[dependencies]` carry no
///   `log` and no `tracing`, and every `println!` in it is inside a
///   `#[cfg(test)]` module; a warning would mean deciding how this engine logs,
///   which is not this function's to decide.
/// - Glyph atlases (D10) will ask the same question and want the same answer:
///   whether a text rasteriser pads its glyphs is the rasteriser's business.
///
/// So being unpadded is legal, and **claiming to be padded while not being** is
/// what this catches. The obligation sits with the caller that makes the claim.
///
/// # Errors
///
/// - [`PaddingDefect::NoRoom`] if the content block plus its border does not
///   fit inside `texture`.
/// - [`PaddingDefect::EmptyRegion`] if `width` or `height` is zero.
/// - [`PaddingDefect::WrongTexel`] for the first border texel in row-major
///   order — left to right within a row, rows top to bottom — that is not a
///   copy of its nearest content texel.
pub fn check_region_padding(
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    border: u32,
    texture: &Pixels,
) -> Result<(), PaddingDefect> {
    let no_room = || PaddingDefect::NoRoom {
        left,
        top,
        width,
        height,
        border,
        texture_width: texture.width(),
        texture_height: texture.height(),
    };

    // Checked arithmetic throughout: a region near `u32::MAX`, or flush against
    // the rim, is exactly the case `from_texels` documents as wrapping rather
    // than clamping. A wrapped footprint here does not compare the wrong texels
    // — it makes the loop range empty, so the check would return `Ok(())`
    // having compared nothing at all. Measured, not assumed: with every
    // `checked_*` replaced by `wrapping_*`, `left = 0, top = 0, 8 x 8, border 1`
    // on a 10 x 10 texture compares 0 border texels and passes. A vacuous pass
    // is the failure this repository has been bitten by most, so it is the one
    // worth naming here.
    let right = left.checked_add(width).ok_or_else(no_room)?;
    let bottom = top.checked_add(height).ok_or_else(no_room)?;
    let outer_left = left.checked_sub(border).ok_or_else(no_room)?;
    let outer_top = top.checked_sub(border).ok_or_else(no_room)?;
    let outer_right = right.checked_add(border).ok_or_else(no_room)?;
    let outer_bottom = bottom.checked_add(border).ok_or_else(no_room)?;

    if width == 0 || height == 0 {
        // Before the fitting check, because an empty region's footprint can fit
        // and "no room" would then be a precise-sounding falsehood. It is also
        // what keeps `right - 1` below from underflowing.
        return Err(PaddingDefect::EmptyRegion { width, height });
    }
    if outer_right > texture.width() || outer_bottom > texture.height() {
        return Err(no_room());
    }

    for y in outer_top..outer_bottom {
        for x in outer_left..outer_right {
            let inside = (left..right).contains(&x) && (top..bottom).contains(&y);
            if inside {
                continue;
            }

            // The nearest content texel: clamped on each axis independently,
            // which gives the corner texel for a corner and the nearest edge
            // texel otherwise. `right` and `bottom` are exclusive, so the last
            // content index is one less.
            let source_x = x.clamp(left, right - 1);
            let source_y = y.clamp(top, bottom - 1);

            // At least one of the four is true: an in-range x and an in-range
            // y is the `inside` case, already skipped. The final arm is
            // therefore `y >= bottom` and nothing else.
            let edge = match (x < left, x >= right, y < top, y >= bottom) {
                (true, _, true, _) => RegionEdge::TopLeft,
                (_, true, true, _) => RegionEdge::TopRight,
                (true, _, _, true) => RegionEdge::BottomLeft,
                (_, true, _, true) => RegionEdge::BottomRight,
                (true, ..) => RegionEdge::Left,
                (_, true, ..) => RegionEdge::Right,
                (_, _, true, _) => RegionEdge::Top,
                _ => RegionEdge::Bottom,
            };

            let found = texture.pixel(x, y).ok_or_else(no_room)?;
            let expected = texture.pixel(source_x, source_y).ok_or_else(no_room)?;
            if found != expected {
                return Err(PaddingDefect::WrongTexel {
                    edge,
                    x,
                    y,
                    source_x,
                    source_y,
                    found,
                    expected,
                });
            }
        }
    }

    Ok(())
}

/// One texture, the sprites drawn from it, and the view they are seen through.
///
/// A draw call binds one texture, so a frame that wants glyphs over a scene
/// needs two of these rather than one buffer with both — M6.6c measured what
/// happens otherwise: appended glyph sprites draw cutouts of the scene atlas at
/// glyph positions.
///
/// Borrowed rather than owned, and a plain struct rather than a tuple, because
/// `Option<(&Pixels, &[SpriteInstance])>` reads as two unrelated things at every
/// call site.
///
/// # Why the camera is a field here and not a parameter beside the batch
///
/// M6b.4 gave the second batch its own coordinate space, and the choice of
/// *where to put it* is the whole of that task's public surface. Both
/// alternatives were available; this one is what the compiler can check.
///
/// - A fifth parameter on `render_sprites_over` would sit next to the fourth
///   with the **same type** — `(…, overlay, camera, overlay_camera)`, two
///   adjacent `CameraView`s. Swapping them compiles, renders, and puts the HUD
///   in world space. A named field cannot be swapped.
/// - The overlay is an `Option` at every entry point, so a separate parameter
///   would admit "an overlay camera and no overlay". As a field the camera
///   cannot outlive the batch it describes.
///
/// The rejected alternative that is *not* about spelling — a component on each
/// sprite, choosing between two projections per sprite — is recorded with its
/// price in `ProjektPlan.md` §6/M6b: it would need either a further vertex
/// attribute, on a vertex M6b.3 has just grown from 16 bytes to 32, or a second
/// cutting criterion in `batch_runs`, which is what M6b.3 declined to do to
/// D15's sampler cut. ADR-0039 had already drawn a scene/overlay line; this puts
/// "world" and "screen" on the line that exists instead of adding a third axis.
/// **The reopening condition is sharp**: the first consumer that needs two
/// overlays with *different* cameras — damage numbers pinned to enemies and a
/// bar pinned to the screen, in one frame. Until then a world-fixed overlay
/// belongs in the scene batch.
#[derive(Debug, Clone, Copy)]
pub struct SpriteBatch<'a> {
    /// The texture every sprite in this batch samples.
    pub image: &'a Pixels,
    /// The sprites, in drawing order.
    pub sprites: &'a [SpriteInstance],
    /// The view this batch is seen through, independently of the scene's.
    ///
    /// **[`CameraView::IDENTITY`] is the screen-fixed layer**, and that is a
    /// consequence of [`Projection`] rather than a convention added here: the
    /// projection is built from the *target's* own width and height, so the
    /// identity view puts the origin at the centre of the target and makes one
    /// world unit one target pixel, with y up (ADR-0004). A batch drawn through
    /// it does not move when the scene's camera does.
    ///
    /// Passing the scene's camera reproduces what this batch drew before the
    /// field existed, **bit for bit** — the overlay projection is the scene
    /// projection with its camera field replaced, so an equal camera replaces it
    /// with itself and no arithmetic runs at all.
    ///
    /// # What "screen-fixed" does and does not fix
    ///
    /// It is **centre-anchored**. An element keeps its pixel offset from the
    /// centre of the target across target sizes; it does not keep its offset
    /// from an *edge*. A bar authored 48 px below the centre is 48 px below the
    /// centre at 1280 x 720 and at 640 x 360 alike, which is what makes it a HUD
    /// at all — but it is not 16 px above the *bottom* in both. Anchoring to
    /// edges is layout, and layout is `ProjektPlan.md` §6/M6b's last item.
    pub camera: CameraView,
}

/// Which batch a run belongs to.
///
/// Two, not `n`: two batches are what has a consumer, and a list would be
/// stock (§2). The day a third is wanted, this enum is where the compiler
/// starts asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchOf {
    /// The scene: the sprites and texture every caller has always passed.
    First,
    /// The overlay: a second texture, drawn after the first.
    Second,
}

/// The draw calls a frame will make, decided without a GPU.
///
/// # Why this exists as its own function
///
/// It is the instrument for the property M6.6c's halt made a condition of this
/// seam: **an empty second batch must produce nothing at all** — no bind group,
/// no run, the same command sequence as before there was a second batch. That is
/// what makes "the shared code is unchanged" the regression evidence for the ten
/// blessed references, rather than the weaker "the overlay happens to be off".
///
/// Stated as an equality a test can assert without a device:
///
/// ```
/// # use narvo_render2d::{BatchOf, batch_plan, batch_runs};
/// # fn check(scene: &[narvo_render2d::SpriteInstance]) {
/// let alone: Vec<_> = batch_runs(scene).into_iter().map(|r| (r, BatchOf::First)).collect();
/// assert_eq!(batch_plan(scene, &[]), alone);
/// # }
/// ```
///
/// The second batch's ranges are offset by `first.len()` because both batches
/// share one vertex and one index buffer — [`batch_vertices`] is called for each
/// and the results concatenated, so the second batch's sprite `i` is index
/// `first.len() + i` in the buffer `encode_runs` walks.
///
/// **Runs never span the split**, which is what keeps a run's texture
/// unambiguous: each batch is cut on its own with [`batch_runs`], so a run is
/// wholly inside one batch and names one texture.
#[must_use]
pub fn batch_plan(
    first: &[SpriteInstance],
    second: &[SpriteInstance],
) -> Vec<(std::ops::Range<usize>, BatchOf)> {
    let mut plan: Vec<(std::ops::Range<usize>, BatchOf)> = batch_runs(first)
        .into_iter()
        .map(|run| (run, BatchOf::First))
        .collect();

    // The guard is on emptiness rather than on `Option`, and that is the whole
    // point: a caller that hands over a batch with no sprites in it must be
    // indistinguishable from one that hands over nothing. `batch_runs` already
    // returns no runs for an empty slice, so this loop simply adds none — the
    // emptiness is handled by not having anything to add, not by a branch that
    // could drift from the one in the callers.
    let offset = first.len();
    plan.extend(
        batch_runs(second)
            .into_iter()
            .map(|run| (run.start + offset..run.end + offset, BatchOf::Second)),
    );

    plan
}

/// Cuts a drawn sequence into runs of equal filter, one draw call each.
///
/// **This is D15.** The sequence arrives already in drawing order —
/// `placements_of` sorted it by depth with `EntityId` breaking ties — and this
/// only *cuts* it. It never reorders, never sorts and never moves a sprite past
/// another, so the visible order after the cut is the visible order before it.
/// `the_runs_are_the_input_in_order` holds that property; this comment does not.
///
/// Returns half-open index ranges covering `sprites` exactly, in order. An empty
/// input gives no runs, which is the only case that yields none: a non-empty
/// input always yields at least one.
#[must_use]
pub fn batch_runs(sprites: &[SpriteInstance]) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    let mut start = 0;

    for (index, sprite) in sprites.iter().enumerate() {
        if index > 0 && sprite.filter != sprites[index - 1].filter {
            runs.push(start..index);
            start = index;
        }
    }
    if !sprites.is_empty() {
        runs.push(start..sprites.len());
    }

    runs
}

/// Where the camera sits and how far it is zoomed in, in world units.
///
/// Three bare `f32`, the unpacked form of `narvo_ecs::Camera`, for the same
/// reason [`SpritePlacement`] is the unpacked form of `Transform`: this crate
/// does not depend on the ECS and should not have to (ADR-0015). That the
/// component is made of bare scalars in the first place is ADR-0014, decided for
/// its own reason — keeping a foreign type's serde format out of the state hash —
/// and it is what makes the hand-written conversion four lines rather than a
/// dependency.
///
/// # Larger `zoom` shows less
///
/// `zoom = 2.0` makes one world unit two pixels, so the visible rectangle halves
/// **on each axis** — a quarter of the world area: the camera is zoomed **in**.
/// `zoom = 0.5` doubles each axis and shows four times as much. The
/// direction is a choice — a field that made the picture smaller as it grew
/// would read backwards to anyone who has used a camera — and it is stated here
/// rather than left to be inferred from the arithmetic.
///
/// It is deliberately **not** in ADR-0004, and M3.12 reports that rather than
/// deciding it there. ADR-0004's M3.11 amendment records a convention *because* a
/// symmetric test image cannot see it, and that argument does not reach a zoom:
/// reverse this direction and every image drawn at a zoom other than 1 changes
/// wherever a sprite is drawn — not at literally every pixel, since background
/// stays background — which the probes in `tests/camera_scene.rs` catch at once.
/// It is
/// invisible only in images drawn at [`CameraView::IDENTITY`], which is the four
/// blessed ones — so the exclusion is an argument about which document should
/// hold it, not a claim that nothing could hide it.
///
/// # Not a transform stack
///
/// Position and zoom only. Follow and shake are still ahead: `ProjektPlan.md`
/// §6/M3 lists both inside M3's camera scope and does not number the task, so no
/// milestone number is asserted here. Camera *rotation* is in that scope not at
/// all, and it would need its own measurement first — it takes sprite edges off
/// the **axes** as well as off the grid, and `BASELINE.md`'s off-grid section
/// measured six axis-aligned cameras and nothing turned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    /// Where the camera's centre sits along world x.
    pub x: f32,
    /// Where the camera's centre sits along world y.
    pub y: f32,
    /// Magnification. Larger shows less; `1.0` is one world unit per pixel.
    pub zoom: f32,
}

impl CameraView {
    /// The view every render used before M3.12: origin, no magnification.
    ///
    /// [`Projection::world_to_ndc`] with this view returns what the fixed
    /// projection returned, **bit for bit** — asserted on `to_bits` in
    /// `the_identity_camera_is_the_projection_that_preceded_it`, not argued
    /// from the algebra.
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        zoom: 1.0,
    };

    /// A view centred on `(x, y)` at `zoom`.
    #[must_use]
    pub const fn new(x: f32, y: f32, zoom: f32) -> Self {
        Self { x, y, zoom }
    }
}

/// The orthographic mapping from world units to NDC, through a camera.
///
/// # One world unit is one pixel of the render target at `zoom = 1`, origin at
/// the camera
///
/// A `w` x `h` target therefore shows world x in `cam_x - w/(2 zoom) ..= cam_x +
/// w/(2 zoom)` and world y likewise. Two consequences make this the choice
/// rather than a taste:
///
/// - A sprite scaled to `(64.0, 64.0)` covers 64 by 64 pixels, so what a
///   reference image should contain can be worked out on paper before anything
///   renders. That is what makes a pixel probe a prediction instead of a
///   recording of whatever happened.
/// - It introduces no constant. Anything of the form "the view is 10 units
///   tall" is a camera parameter wearing a projection's clothes. Since M3.12 the
///   camera exists and says so in its own field, [`CameraView::zoom`]; the
///   projection still contributes no constant of its own.
///
/// Non-square targets need no aspect-ratio policy under this mapping, because
/// the unit is the pixel and pixels are square. NDC is anisotropic on such a
/// target; world space is not, which is the property a caller wants. `zoom`
/// multiplies both axes by the same number and leaves that true.
///
/// # What M3.12 changed, and what it did not
///
/// The mapping gained a subtraction and a multiplication. It gained **no
/// negation and no second reconciliation**: `world_to_ndc` is still a translate
/// and a scale on each axis, so ADR-0004's rule that the two opposing y
/// directions meet exactly once — in `SPRITE_CORNERS` and `quad.rs`'s
/// `VERTICES` — is untouched, and `no_axis_is_negated_on_the_way_to_ndc` still
/// asserts it directly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// Half the target width, in pixels. Scaled world x is divided by this.
    half_width: f32,
    /// Half the target height, in pixels. Scaled world y is divided by this.
    half_height: f32,
    /// Where the view sits and how far it is zoomed in.
    camera: CameraView,
}

impl Projection {
    /// The projection for a `width` x `height` render target, seen from
    /// [`CameraView::IDENTITY`].
    ///
    /// A zero dimension would divide by zero; no target can have one
    /// (`OffscreenTarget::new` rejects it), and the maximum is clamped away
    /// from the point where `f32` stops representing consecutive integers.
    #[must_use]
    pub fn for_target(width: u32, height: u32) -> Self {
        Self {
            half_width: width as f32 / 2.0,
            half_height: height as f32 / 2.0,
            camera: CameraView::IDENTITY,
        }
    }

    /// The same projection seen from `camera`.
    #[must_use]
    pub const fn viewed_by(self, camera: CameraView) -> Self {
        Self { camera, ..self }
    }

    /// The view this projection renders through.
    #[must_use]
    pub const fn camera(&self) -> CameraView {
        self.camera
    }

    /// World point to normalised device coordinates.
    ///
    /// Subtract the camera, scale by the zoom, divide by the half-extent — and
    /// nothing else. **No negation**: world y and NDC y both point up, so a sign
    /// here would be the second reconciliation ADR-0004 forbids, and it would be
    /// invisible in every test that only looks at a symmetric image.
    ///
    /// **`CameraView::IDENTITY` returns what the fixed projection returned, bit
    /// for bit.** `x - 0.0` and `x * 1.0` are exact for every finite `f32` and
    /// for both zeros and both infinities, so the added arithmetic cannot move a
    /// vertex of the three blessed images that go through a projection. Asserted
    /// rather than reasoned:
    /// `the_identity_camera_is_the_projection_that_preceded_it` compares
    /// `to_bits` against the pre-M3.12 expression over a list of values chosen
    /// to stress it.
    #[must_use]
    pub fn world_to_ndc(&self, x: f32, y: f32) -> [f32; 2] {
        [
            (x - self.camera.x) * self.camera.zoom / self.half_width,
            (y - self.camera.y) * self.camera.zoom / self.half_height,
        ]
    }

    /// A point on the render target, in pixels from its top left, to world
    /// units.
    ///
    /// The inverse of the chain a sprite takes to the screen, and it lives here
    /// **because [`world_to_ndc`](Self::world_to_ndc) does**. A screen-to-world
    /// conversion written anywhere else would be a second piece of camera
    /// mathematics: correct on the day it was written, and free afterwards to
    /// disagree with this one about a zoom, a half-extent or a sign. Sharing the
    /// struct means it cannot — the two read the same three fields, and
    /// `a_world_point_survives_the_round_trip_through_the_screen` composes them
    /// and asserts identity.
    ///
    /// # The y flip, and why it is not the second reconciliation ADR-0004 forbids
    ///
    /// Pixel rows run **down** from the top of the target and NDC y runs **up**,
    /// so this function negates y and `world_to_ndc` does not. That asymmetry is
    /// exactly right and is worth stating, because ADR-0004's rule — the two
    /// opposing y directions meet exactly once — invites the opposite conclusion.
    ///
    /// On the way *out*, that meeting is the fixed-function viewport transform
    /// inside the GPU, which is not this project's code. On the way *back* there
    /// is no GPU to do it, so the inverse of that transform has to be written
    /// down, and here is the only place it is. It is the same single
    /// reconciliation, read backwards, rather than a new one: nothing else on
    /// this path flips anything, and if a second flip were ever added the round
    /// trip above would stop being the identity.
    #[must_use]
    pub fn screen_to_world(&self, x: f32, y: f32) -> [f32; 2] {
        let ndc_x = x / self.half_width - 1.0;
        let ndc_y = 1.0 - y / self.half_height;

        [
            ndc_x * self.half_width / self.camera.zoom + self.camera.x,
            ndc_y * self.half_height / self.camera.zoom + self.camera.y,
        ]
    }

    /// A world point to its position on the render target, in pixels from the
    /// top left.
    ///
    /// The inverse of [`screen_to_world`](Self::screen_to_world), and the answer
    /// to "which pixel is this sprite on" — a HUD placing a label beside a unit,
    /// a tool drawing a marker over a frame, a test naming the texel it is about
    /// to read.
    ///
    /// # It exists because it was measured to be missing
    ///
    /// M6b.9 built a game outside this repository against the engine's public
    /// surface and counted what it had to write itself that looked like engine
    /// work. Six of those lines were this function. The risk it named was not
    /// the size: a consumer's copy and `screen_to_world` are free to disagree
    /// about a half-extent or a sign, and the disagreement shows up as a label
    /// that drifts off its unit at one zoom and sits right at another.
    ///
    /// # The coordinate it returns, exactly
    ///
    /// The same space `screen_to_world` takes: `0.0` is the target's **left
    /// edge**, `width` its right edge, `0.0` the **top edge** and `height` the
    /// bottom. So these are pixel *edges*, not pixel centres, and the centre of
    /// column `i` is `i as f32 + 0.5` — which is the value to use when reading
    /// a texel back, because a returned `640.0` sits on the boundary between
    /// columns 639 and 640 rather than inside either.
    ///
    /// **Y runs down**, the framebuffer's own direction (ADR-0004: "row 0 is the
    /// top"), while world y runs up. A world point *above* the camera therefore
    /// gets a *smaller* number back, which is the sign
    /// `the_screen_y_axis_runs_down_while_the_world_y_axis_runs_up` pins.
    ///
    /// Nothing is clamped and nothing is rounded: a point off the left of the
    /// target comes back negative, and a caller that wants a texel index decides
    /// for itself how to floor it. Clamping here would hide the off-screen case
    /// from exactly the caller that needs to know about it.
    ///
    /// # What it shares, and the one half it cannot
    ///
    /// It calls [`world_to_ndc`](Self::world_to_ndc) rather than restating it,
    /// so the camera, the zoom and the half-extents enter this answer through
    /// the single expression the render path itself uses. That is the half where
    /// a disagreement would be silent.
    ///
    /// The remaining half — NDC to pixels — is the fixed-function viewport
    /// transform, which the GPU performs on the way out and nothing performs on
    /// the way back. Its inverse is written inside `screen_to_world`, and its
    /// forward form is written here, so the struct states it twice. That is the
    /// arrangement this file already uses in the other direction: `screen_to_world`
    /// writes out the inverse of `world_to_ndc` instead of sharing it, because an
    /// inverse cannot be expressed by calling the function it inverts. What ties
    /// the two statements together is composition, not form —
    /// `a_world_point_returns_from_the_pixel_it_was_sent_to` runs them into each
    /// other over four cameras and three target shapes.
    ///
    /// **A round trip alone would not be enough**, and this is worth saying
    /// because it is the trap: two functions that share a wrong half-extent
    /// compose to the identity just as happily as two that share a right one. So
    /// `a_world_point_lands_on_the_pixel_it_can_be_worked_out_to` asserts hand-
    /// computed pixel positions with `assert_eq!` — absolute answers, worked out
    /// on paper from the mapping's own definition, which a shared error cannot
    /// satisfy.
    ///
    /// # One hand-written copy is still in the tree, knowingly
    ///
    /// `drop_screen_point` in `narvo-app`'s `frame.rs` states this whole mapping
    /// again, and its own comment says why: "written out because the crate offers
    /// only the one direction". **That reason has now expired**, so it is a
    /// candidate to fold — but folding it is not free, because the test it serves
    /// round-trips it against `screen_to_world` precisely to justify it being a
    /// second copy, and calling this function for both directions would replace an
    /// independent statement with a composition. M7.0 measured it, left it, and
    /// addressed the decision to a task that owns `narvo-app`.
    #[must_use]
    pub fn world_to_screen(&self, x: f32, y: f32) -> [f32; 2] {
        let [ndc_x, ndc_y] = self.world_to_ndc(x, y);

        // NDC to pixels: x from -1..1 to 0..width, y from 1..-1 to 0..height.
        // The negation is the same single reconciliation `screen_to_world`
        // describes, read forwards.
        [
            (ndc_x + 1.0) * self.half_width,
            (1.0 - ndc_y) * self.half_height,
        ]
    }

    /// The world point `inset` in from `anchor`, in target pixels.
    ///
    /// The building block M6b.4 addressed to M6b.8 and nothing more. A
    /// screen-fixed batch is **centre-anchored** — [`SpriteBatch::camera`] says
    /// so in its own words — so an element authored 22 px above the centre stays
    /// 22 px above the centre at every target size, and an element that wants to
    /// stay 22 px above the **bottom** has had no way to say it. This is that
    /// way, and it is one function returning one point.
    ///
    /// # It is not layout, deliberately
    ///
    /// It holds no state, owns no tree, and knows nothing about what is placed
    /// at the point it returns. `ProjektPlan.md` §2 asks for building blocks a
    /// game composes until a second consumer shows a system is needed, and a
    /// consumer composes this by putting the result into a `Transform` it was
    /// going to write anyway. What it removes is the arithmetic, which was
    /// measured in M6b.8's S1 probe as the one thing a HUD had to work out for
    /// itself and get silently wrong at a second target size.
    ///
    /// # `inset` always points inward
    ///
    /// A positive inset moves **away from the named edge, into the target**, on
    /// both axes and for every anchor. So `(8.0, 8.0)` is eight pixels in from
    /// whichever corner was asked for, and the same pair reads the same way for
    /// all four of them — which is the property that makes a HUD's four corners
    /// spell alike instead of each carrying its own sign.
    ///
    /// On an axis that is centred rather than anchored to an edge there is no
    /// "inward", so the inset is an ordinary offset along the world axis:
    /// positive x to the right, positive y up. [`ScreenAnchor::Centre`] with
    /// `(0.0, 0.0)` is therefore the camera's own position, which is what
    /// centre anchoring already gave.
    ///
    /// # The camera is part of the answer, and a HUD wants none
    ///
    /// This returns the world point that *currently appears* at that screen
    /// position under this projection, so a panned or zoomed camera moves it.
    /// That is the honest general answer and it is the wrong one for a HUD: a
    /// screen-fixed element is drawn through [`CameraView::IDENTITY`], so the
    /// projection to anchor against is [`Projection::for_target`] with no
    /// [`viewed_by`](Self::viewed_by) applied — the same view the batch carries.
    /// Anchoring against the scene's camera and drawing through the identity is
    /// the mistake this paragraph exists to name; `an_anchor_moves_with_the_camera`
    /// asserts that the two really do differ, so the warning is measured rather
    /// than merely written.
    ///
    /// # It adds no arithmetic, and inherits a rounding
    ///
    /// The body picks a pixel and calls [`screen_to_world`](Self::screen_to_world).
    /// That is deliberate: a second piece of camera mathematics here would be
    /// free to disagree with that one about a zoom, a half-extent or a sign,
    /// which is the failure `screen_to_world`'s own documentation describes one
    /// method up.
    ///
    /// **The price is exactness, and it is paid knowingly.** `screen_to_world`
    /// goes through NDC, so `x / half_width - 1.0` is inexact whenever
    /// `half_width` is not a power of two, and the value that comes back is off
    /// by about one part in a million — `anchor(Centre, 8.0, 8.0)` on a 192-wide
    /// target returns `8.000004` rather than `8.0`. An exact formula here would
    /// be the second implementation the paragraph above refuses, and it would
    /// make this method and the click path disagree about the same pixel. So the
    /// rounding is kept, measured in
    /// `an_anchor_carries_the_rounding_of_the_conversion_it_shares`, and named
    /// here rather than discovered later by whoever compares two placements for
    /// equality.
    #[must_use]
    pub fn anchor(&self, anchor: ScreenAnchor, inset_x: f32, inset_y: f32) -> [f32; 2] {
        let (width, height) = (self.half_width * 2.0, self.half_height * 2.0);

        let x = match anchor.horizontal() {
            Edge::Low => inset_x,
            Edge::Middle => self.half_width + inset_x,
            Edge::High => width - inset_x,
        };
        // y is the axis where "low" is the *bottom*, because a world y grows
        // upward while a pixel row grows downward. The flip lives in
        // `screen_to_world` and is not repeated here; what this match does is
        // choose which pixel row to ask about.
        let y = match anchor.vertical() {
            Edge::Low => height - inset_y,
            Edge::Middle => self.half_height - inset_y,
            Edge::High => inset_y,
        };

        self.screen_to_world(x, y)
    }
}

/// Which side of one axis an anchor sits on.
///
/// Private: it is the shape of [`ScreenAnchor`]'s answer rather than a
/// vocabulary a caller uses, and exposing it would offer a second way to spell
/// the same nine anchors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// Left on x, bottom on y.
    Low,
    /// The centre of the axis.
    Middle,
    /// Right on x, top on y.
    High,
}

/// A corner, an edge midpoint or the centre of the render target.
///
/// The vocabulary [`Projection::anchor`] takes, and a closed one: nine positions
/// is every combination of three per axis, so there is nothing a tenth variant
/// could name. A caller wanting a point that is not one of these adds an inset
/// to the nearest one, which is what the inset is for.
///
/// **Named `ScreenAnchor` rather than `Anchor`** because it is a position on the
/// render target and not on a sprite or in a world, and this crate's root
/// namespace is flat — a bare `Anchor` there would not say which of the three it
/// meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenAnchor {
    /// The target's top left corner.
    TopLeft,
    /// The middle of the target's top edge.
    Top,
    /// The target's top right corner.
    TopRight,
    /// The middle of the target's left edge.
    Left,
    /// The centre of the target, which is what a screen-fixed batch already
    /// anchors to.
    Centre,
    /// The middle of the target's right edge.
    Right,
    /// The target's bottom left corner.
    BottomLeft,
    /// The middle of the target's bottom edge.
    Bottom,
    /// The target's bottom right corner.
    BottomRight,
}

impl ScreenAnchor {
    /// Which side of the x axis this anchor sits on.
    const fn horizontal(self) -> Edge {
        match self {
            Self::TopLeft | Self::Left | Self::BottomLeft => Edge::Low,
            Self::Top | Self::Centre | Self::Bottom => Edge::Middle,
            Self::TopRight | Self::Right | Self::BottomRight => Edge::High,
        }
    }

    /// Which side of the y axis this anchor sits on, with `Low` the bottom.
    const fn vertical(self) -> Edge {
        match self {
            Self::BottomLeft | Self::Bottom | Self::BottomRight => Edge::Low,
            Self::Left | Self::Centre | Self::Right => Edge::Middle,
            Self::TopLeft | Self::Top | Self::TopRight => Edge::High,
        }
    }
}

/// The sprite's four corners as `[ndc_x, ndc_y, u, v, r, g, b, a]`, ready for
/// the vertex buffer.
///
/// **The last four are the same four on every corner** — the premultiplied
/// tint, repeated. A vertex attribute is how a per-sprite value reaches the
/// fragment shader without a uniform and without cutting the batch, and the
/// repetition is what that costs: sixteen bytes per vertex, sixty-four per
/// sprite. `quad.wgsl` reads it with `@interpolate(flat, first)`, so the four
/// copies are never averaged and the value the shader sees is the value written
/// here, bit for bit.
///
/// Scale, then rotate, then translate, then project — the order that makes the
/// rotation a rotation about the sprite's own centre rather than about the
/// world origin.
///
/// **No trigonometry.** The `(cos, sin)` pair arrives in the placement and is
/// used as it stands; until M5b.4 this function opened with
/// `placement.rotation.sin_cos()`, and [`SpritePlacement`] records why the call
/// moved to the caller.
///
/// The texture coordinates take the same trip: the corner table's unit `u, v`
/// are remapped into the sprite's [`TextureRegion`], which for
/// [`TextureRegion::WHOLE_TEXTURE`] returns them unchanged. There is no branch
/// on "has a region" and no second function for the full-texture case — that
/// case is a region whose edges are the texture's own.
#[must_use]
pub(crate) fn sprite_vertices(sprite: SpriteInstance, projection: Projection) -> [[f32; 8]; 4] {
    let placement = sprite.placement;
    let (sin, cos) = (placement.rot_sin, placement.rot_cos);
    let [tint_r, tint_g, tint_b, tint_a] = sprite.tint.premultiplied();

    SPRITE_CORNERS.map(|[local_x, local_y, u, v]| {
        let scaled_x = local_x * placement.scale_x;
        let scaled_y = local_y * placement.scale_y;

        // Counter-clockwise for positive `rotation`, which is what x-right with
        // y-up makes it (ADR-0004, world-space amendment).
        let world_x = scaled_x * cos - scaled_y * sin + placement.x;
        let world_y = scaled_x * sin + scaled_y * cos + placement.y;

        let [ndc_x, ndc_y] = projection.world_to_ndc(world_x, world_y);
        let [region_u, region_v] = sprite.region.sample_at(u, v);
        [
            ndc_x, ndc_y, region_u, region_v, tint_r, tint_g, tint_b, tint_a,
        ]
    })
}

/// Most sprites one batch will draw.
///
/// Not a hardware limit and not arbitrary: at four vertices of **thirty-two**
/// bytes each, this is eight megabytes of vertex data in one buffer, which is
/// the order where "one draw call" stops being obviously the right shape. It
/// sits above the 50 000 sprites `ProjektPlan.md` §6/M3 sets as the throughput
/// target, so reaching that target needs no change here.
///
/// **The figure was four megabytes until M6b.3**, at sixteen bytes a vertex,
/// and the tint doubled the vertex without moving this constant. Whether the
/// cap should move with it is a throughput question and not this task's: the
/// number was chosen as an order of magnitude where one draw call stops being
/// obviously right, and eight megabytes is still that order. What is recorded
/// here is that the *reason* now supports a smaller number than it did, so
/// anybody revisiting the cap starts from a measured byte count rather than
/// from a stale one.
///
/// Exceeding it is [`RenderError::BatchTooLarge`], never a silent truncation. A
/// batch that quietly drew the first N of M sprites would look like a rendering
/// bug anywhere except where it is.
pub const MAX_SPRITES_PER_BATCH: usize = 65_536;

/// The cap has to stay above what §6/M3 asks the renderer to reach; lowering it
/// below that is a throughput decision and not an implementation detail. A
/// compile-time check rather than a test, because there is nothing to run.
const _: () = assert!(
    MAX_SPRITES_PER_BATCH >= 50_000,
    "MAX_SPRITES_PER_BATCH is below the 50 000 sprites ProjektPlan.md §6/M3 sets \
     as the throughput target, which would make that target unreachable without \
     changing this constant"
);

/// Every sprite's corners, concatenated, as `[ndc_x, ndc_y, u, v, r, g, b, a]`.
///
/// Sprite `i` owns vertices `4i ..= 4i + 3`, in the order of
/// [`SPRITE_CORNERS`], which is what lets one index buffer address all of them
/// and what `batch_indices` in `quad.rs` assumes.
///
/// Per sprite this is [`sprite_vertices`] and nothing else — the same function
/// the single-sprite path used before batching existed. That is deliberate and
/// is the whole regression argument: a batch of one cannot differ from what
/// M3.4 drew, because it is the same call.
#[must_use]
pub(crate) fn batch_vertices(sprites: &[SpriteInstance], projection: Projection) -> Vec<[f32; 8]> {
    let mut vertices = Vec::with_capacity(sprites.len() * 4);

    for sprite in sprites {
        vertices.extend_from_slice(&sprite_vertices(*sprite, projection));
    }

    vertices
}

#[cfg(test)]
mod tests {
    use super::{
        BatchOf, CameraView, PaddingDefect, Pixels, Projection, REGION_PADDING_TEXELS, RegionEdge,
        SPRITE_CORNERS, ScreenAnchor, SpriteFilter, SpriteInstance, SpritePlacement, SpriteTint,
        TextureRegion, batch_plan, batch_runs, batch_vertices, check_region_padding,
        sprite_vertices,
    };
    use std::hint::black_box;
    use std::time::Instant;

    /// `count` sprites, spread so no two are identical.
    fn many(count: usize) -> Vec<SpriteInstance> {
        (0..count)
            .map(|index| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "the index only has to vary; exactness is irrelevant here"
                )]
                let offset = (index % 64) as f32;
                SpriteInstance::whole_texture(SpritePlacement {
                    x: offset,
                    y: -offset,
                    rot_cos: 1.0,
                    rot_sin: 0.0,
                    scale_x: 8.0,
                    scale_y: 4.0,
                })
            })
            .collect()
    }

    /// A `size` x `size` texture, contents irrelevant.
    ///
    /// [`TextureRegion::from_texels`] reads only its dimensions, and these
    /// tests assert coordinates rather than colours.
    fn texture_of(size: u32) -> Pixels {
        let texels = size as usize * size as usize;
        Pixels::from_rgba8(size, size, vec![0; texels * 4])
            .expect("the generated buffer matches its dimensions")
    }

    /// Nanoseconds of the fastest of `rounds` runs.
    ///
    /// Best-of rather than mean, as `docs/perf/BASELINE.md` already does for
    /// build times: the fastest run is the one least disturbed by whatever else
    /// the machine was doing, and on a shared CI runner that is the only
    /// estimator worth having.
    ///
    /// Both ways of measuring nothing are refused. Zero rounds would leave an
    /// empty series that any later `min` or ratio would happily consume, and a
    /// best time of zero means the clock could not see the work at all — a
    /// guard built on either would be green while checking nothing.
    fn best_of(rounds: usize, mut work: impl FnMut()) -> u128 {
        assert!(
            rounds > 0,
            "a measurement of zero rounds produces an empty sample series, and \
             every guard downstream of it would pass on no data"
        );

        // One unmeasured run first. The first call pays for page faults on a
        // freshly grown heap and for a cold instruction cache, neither of which
        // is what is being measured, and both of which land entirely in the
        // first sample.
        work();

        let mut samples = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let start = Instant::now();
            work();
            samples.push(start.elapsed().as_nanos());
        }

        let best = samples
            .iter()
            .copied()
            .min()
            .expect("rounds is greater than zero, so there is at least one sample");

        assert!(
            best > 0,
            "the fastest of {rounds} rounds took zero nanoseconds, so the clock \
             could not resolve the work. Every ratio built on this would divide by \
             zero or compare noise against noise; raise the workload rather than \
             trusting the number"
        );

        best
    }

    /// Three sprites with three different placements, the second and third
    /// non-uniformly scaled. The same scene the pixel probes render.
    fn three_placements() -> [SpritePlacement; 3] {
        [
            SpritePlacement {
                x: -32.0,
                y: 32.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 32.0,
                scale_y: 32.0,
            },
            SpritePlacement {
                x: 24.0,
                y: 32.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 48.0,
                scale_y: 24.0,
            },
            SpritePlacement {
                x: -16.0,
                y: -40.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 24.0,
                scale_y: 40.0,
            },
        ]
    }

    /// A batch of one is the single-sprite path, vertex for vertex.
    ///
    /// This is the regression argument in its cheapest form. The golden image
    /// blessed in M3.5, `placed_sprite_quadrants_128x128`, is drawn through
    /// `sprite_vertices`; if a batch of one produces the same vertices, the
    /// batch cannot move it. Compared on bit patterns, because a difference in
    /// the last mantissa bit is a difference the rasteriser is entitled to act
    /// on.
    ///
    /// **One image, not two.** Until M3.9 this said "the golden images blessed
    /// in M3.4 and M3.5", which was wrong about the M1 one:
    /// `textured_quad_quadrants_64x64` is rendered by
    /// `OffscreenTarget::render_textured_quad`, which never reaches this
    /// function. That it stays green while this path is perturbed is a property
    /// of a *different* path, and saying otherwise made a two-path argument
    /// read as a one-path one (`ProjektPlan.md` §12).
    #[test]
    fn a_batch_of_one_is_the_single_sprite_path_bit_for_bit() {
        let projection = Projection::for_target(128, 128);

        for placement in three_placements() {
            let sprite = SpriteInstance::whole_texture(placement);
            let single = sprite_vertices(sprite, projection);
            let batched = batch_vertices(&[sprite], projection);

            assert_eq!(batched.len(), 4);
            for (index, (one, many)) in single.iter().zip(batched.iter()).enumerate() {
                for (component, (a, b)) in one.iter().zip(many.iter()).enumerate() {
                    assert_eq!(
                        a.to_bits(),
                        b.to_bits(),
                        "vertex {index} component {component} differs between the \
                         single-sprite path and a batch of one: {a:?} against {b:?}. \
                         The blessed golden image of M3.5 is drawn through the first \
                         of these, so any difference here moves it."
                    );
                }
            }
        }
    }

    /// Sprite `i` owns vertices `4i ..= 4i + 3`, and they are *its* vertices.
    ///
    /// This is the assertion a pixel probe cannot make. Two sprites that swap
    /// placements draw the same set of rectangles from the same texture, so the
    /// image is identical and no probe can see it; the per-index correspondence
    /// can, and only on this side of the GPU.
    #[test]
    fn each_sprite_owns_its_own_four_vertices_in_order() {
        let projection = Projection::for_target(128, 128);
        let placements = three_placements().map(SpriteInstance::whole_texture);
        let batched = batch_vertices(&placements, projection);

        assert_eq!(batched.len(), placements.len() * 4);

        for (index, placement) in placements.iter().enumerate() {
            let expected = sprite_vertices(*placement, projection);
            let slice = &batched[index * 4..index * 4 + 4];

            assert_eq!(
                slice,
                expected.as_slice(),
                "sprite {index} does not own vertices {}..={}. Either the batch \
                 dropped a sprite, gave one of them another's placement, or \
                 emitted them in another order — none of which a rendered image \
                 can distinguish when every sprite samples the same texture.",
                index * 4,
                index * 4 + 3
            );
        }
    }

    /// An empty batch is empty, not a panic and not one stray quad.
    #[test]
    fn an_empty_batch_produces_no_vertices() {
        assert!(batch_vertices(&[], Projection::for_target(128, 128)).is_empty());
    }

    /// **The guarded regression class: one allocation for the batch, not one per
    /// sprite.**
    ///
    /// `batch_vertices` asks for `4 * n` slots up front and writes exactly that
    /// many, so the returned vector's capacity is its length. Drop the
    /// `with_capacity` and the vector grows by doubling instead: the capacity
    /// becomes the next power of two above `4 * n`, and the work becomes
    /// `log2(4n)` allocations plus that many copies of everything written so
    /// far. That is invisible in the output, invisible in every image, and
    /// exactly the kind of thing a throughput task later finds by accident.
    ///
    /// A count rather than a time, deliberately. It is identical on a fast
    /// runner and a slow one, so there is no threshold anybody will be tempted
    /// to raise after the third red build — which is the failure mode a
    /// wall-clock budget on shared CI has.
    ///
    /// What it rests on: that `Vec::with_capacity(k)` followed by exactly `k`
    /// writes leaves `capacity() == k`. The standard library promises only *at
    /// least* `k`, so this is an observed property of the pinned toolchain
    /// rather than a guarantee. A future `std` that over-allocates would turn
    /// this red without a regression; the failure message says so, and the fix
    /// would be to compare against the observed constant instead.
    #[test]
    fn a_batch_allocates_once_and_not_once_per_sprite() {
        for count in [1_usize, 7, 64, 1_000] {
            let vertices = batch_vertices(&many(count), Projection::for_target(128, 128));

            assert_eq!(vertices.len(), count * 4);
            assert_eq!(
                vertices.capacity(),
                count * 4,
                "a batch of {count} sprites produced a vector of capacity {} for \
                 {} vertices. Exact capacity means the buffer was asked for once \
                 and never grew; a larger capacity means it grew by doubling, so \
                 the batch now allocates about log2({}) times and copies what it \
                 had written on each of them.\n  \
                 If `batch_vertices` still preallocates and this is red anyway, \
                 the toolchain's `Vec::with_capacity` began over-allocating - it \
                 promises only *at least* the requested capacity - and this \
                 assertion, not the code, is what needs changing.",
                vertices.capacity(),
                vertices.len(),
                count * 4
            );
        }
    }

    /// **The second guarded class: the preparation stays linear in the sprite
    /// count.**
    ///
    /// A ratio, not a duration. Ten times the sprites should cost about ten
    /// times the work; a step to quadratic would cost about a hundred times.
    /// Whatever the runner's speed is, it appears in both measurements and
    /// cancels — which is the whole reason this is a ratio and not a budget in
    /// milliseconds. A budget would have to be set for the slowest runner, and
    /// then it would catch nothing on a fast one.
    ///
    /// The bound has room for the one honest reason the ratio exceeds ten:
    /// 50 000 sprites are 3.2 MB of vertex data and no longer fit the caches
    /// that 5 000 do, so the larger run pays for memory the smaller one did not.
    /// Measured on the reference machine it sits well under the bound; the
    /// figures are in `docs/perf/BASELINE.md`.
    #[test]
    fn building_a_batch_stays_linear_in_the_sprite_count() {
        const SMALL: usize = 5_000;
        // Seven, not more. A green run costs seven times half a millisecond;
        // a red one costs seven times whatever the regression made it, and a
        // quadratic step at 50 000 sprites is seconds per round.
        const LARGE: usize = 50_000;
        const ROUNDS: usize = 7;
        /// Linear is 10. Quadratic would be 100. Anything under this is a
        /// constant factor, not a change of shape.
        ///
        /// Forty rather than twenty-five, and the number is measured rather than
        /// guessed: nine runs on the reference machine produced ratios from 8 to
        /// 24. The spread is almost entirely in the *small* case, which varied
        /// from 17 500 to 53 200 ns while the large one held between 412 800 and
        /// 456 000 - 5 000 sprites are 320 KB of output and fit L2, 50 000 are
        /// 3.2 MB and do not, so how well the small run is cached decides the
        /// ratio. Twenty-five would have flaked; forty leaves 1.7x over the worst
        /// observation and still sits 2.5x below where a quadratic step lands.
        const BOUND: u128 = 40;

        let projection = Projection::for_target(1920, 1080);
        let small = many(SMALL);
        let large = many(LARGE);

        let small_ns = best_of(ROUNDS, || {
            black_box(batch_vertices(black_box(&small), projection));
        });
        let large_ns = best_of(ROUNDS, || {
            black_box(batch_vertices(black_box(&large), projection));
        });

        let ratio = large_ns / small_ns;
        println!(
            "batch_vertices: {SMALL} -> {small_ns} ns, {LARGE} -> {large_ns} ns, \
             ratio {ratio} (linear is {})",
            LARGE / SMALL
        );

        assert!(
            ratio < BOUND,
            "batch_vertices took {large_ns} ns for {LARGE} sprites against \
             {small_ns} ns for {SMALL}, a ratio of {ratio}. Ten times the sprites \
             is ten times the work when the preparation is linear; {BOUND} is the \
             bound and a quadratic step would land near {}. The runner's speed is \
             in both numbers and cancels, so this is a change of shape rather than \
             a slow machine.",
            (LARGE / SMALL) * (LARGE / SMALL)
        );
    }

    /// Prints the frame-preparation cost, split into its parts.
    ///
    /// Not a gate — it asserts only that it measured something. It exists so the
    /// numbers reach the CI log on both platforms, the way the golden-image
    /// margin does since M3.1, and so `BASELINE.md` can be filled from a run
    /// rather than from a recollection.
    #[test]
    fn the_cost_of_preparing_a_batch_is_recorded() {
        // Enough rounds that the smallest case is not reported out of the
        // clock's granularity. Windows resolves `Instant` to about a hundred
        // nanoseconds, and a hundred sprites take a few hundred, so a handful of
        // rounds would report a number that is mostly quantisation.
        const ROUNDS: usize = 25;
        let projection = Projection::for_target(1920, 1080);

        println!("sprites | batch_vertices ns | placement copy ns | ns per sprite");
        for count in [100_usize, 1_000, 10_000, 50_000] {
            let placements = many(count);

            let build_ns = best_of(ROUNDS, || {
                black_box(batch_vertices(black_box(&placements), projection));
            });
            // The copy on its own: the same bytes moved, none of the arithmetic.
            // This is the part D12 pays for, isolated from the part it buys.
            let copy_ns = best_of(ROUNDS, || {
                black_box(black_box(&placements).to_vec());
            });

            #[expect(
                clippy::cast_precision_loss,
                reason = "a nanosecond count divided by a sprite count needs two \
                          significant digits, not sixteen"
            )]
            let per_sprite = build_ns as f64 / count as f64;
            println!("{count:7} | {build_ns:17} | {copy_ns:17} | {per_sprite:.1}");

            assert!(
                build_ns > 0 && copy_ns > 0,
                "a measurement of zero is not one"
            );
        }
    }

    /// The whole texture is the corner table, bit for bit.
    ///
    /// "Full area is a special case of the region, not a second path" is the
    /// load-bearing claim of M3.9, and this is what makes it checkable rather
    /// than a sentence in a doc comment. If `sample_at` ever stopped being the
    /// identity on [`TextureRegion::WHOLE_TEXTURE`] — a `mul_add`, a clamp, a
    /// half-texel inset — every sprite drawn before M3.9 would sample somewhere
    /// else, by less than a texel, which is the size of change a tolerance
    /// absorbs and an eye does not catch.
    #[test]
    fn the_whole_texture_region_leaves_the_corner_table_untouched() {
        let projection = Projection::for_target(128, 128);

        for placement in three_placements() {
            let vertices = sprite_vertices(SpriteInstance::whole_texture(placement), projection);

            for (index, corner) in SPRITE_CORNERS.iter().enumerate() {
                assert_eq!(
                    vertices[index][2].to_bits(),
                    corner[2].to_bits(),
                    "corner {index}: u came out as {} where the table holds {}",
                    vertices[index][2],
                    corner[2]
                );
                assert_eq!(
                    vertices[index][3].to_bits(),
                    corner[3].to_bits(),
                    "corner {index}: v came out as {} where the table holds {}. The \
                     whole-texture region must be the identity on the table's \
                     coordinates, or the blessed image of M3.5 moves.",
                    vertices[index][3],
                    corner[3]
                );
            }
        }
    }

    /// The region covering everything *is* the constant, not merely close.
    #[test]
    fn a_region_covering_the_whole_texture_is_the_whole_texture_constant() {
        let texture = texture_of(16);
        let built = TextureRegion::from_texels(0, 0, 16, 16, &texture).uv_bounds();
        let constant = TextureRegion::WHOLE_TEXTURE.uv_bounds();

        for (index, (built, constant)) in built.iter().zip(constant.iter()).enumerate() {
            assert_eq!(
                built.to_bits(),
                constant.to_bits(),
                "edge {index}: from_texels gave {built} where the constant is \
                 {constant}. A texture's own extent has to normalise to 0 and 1 \
                 exactly, or the two ways of saying \"all of it\" draw differently."
            );
        }
    }

    /// Two regions of the same texture sample different parts of it.
    #[test]
    fn two_different_regions_sample_different_parts_of_the_texture() {
        let texture = texture_of(16);
        let projection = Projection::for_target(128, 128);
        let placement = SpritePlacement::new(32.0, 32.0);

        let left = sprite_vertices(
            SpriteInstance::new(placement, TextureRegion::from_texels(0, 0, 8, 8, &texture)),
            projection,
        );
        let right = sprite_vertices(
            SpriteInstance::new(placement, TextureRegion::from_texels(8, 0, 8, 8, &texture)),
            projection,
        );

        for index in 0..4 {
            assert_eq!(
                left[index][0].to_bits(),
                right[index][0].to_bits(),
                "corner {index}: the placement is the same, so the geometry must be"
            );
            assert_eq!(left[index][1].to_bits(), right[index][1].to_bits());
            assert_ne!(
                left[index][2], right[index][2],
                "corner {index}: two regions side by side in the texture must differ \
                 in u. Equal u here means the region is being ignored and every \
                 sprite samples the same thing — the failure a scene of one sprite \
                 cannot show."
            );
        }
    }

    /// A region and its vertical mirror are told apart — the y-flip test of the
    /// region layer.
    ///
    /// The mirror of the texture's upper half is its lower half. If `top` were
    /// ever measured from the bottom row, these two would swap and every atlas
    /// would come out upside down inside its cells while the sprite geometry,
    /// the projection and both existing golden images stayed perfectly right.
    #[test]
    fn a_region_and_its_vertical_mirror_are_distinguishable() {
        let texture = texture_of(16);
        let projection = Projection::for_target(128, 128);
        let placement = SpritePlacement::new(32.0, 32.0);

        let upper = TextureRegion::from_texels(0, 0, 16, 8, &texture);
        let lower = TextureRegion::from_texels(0, 8, 16, 8, &texture);

        assert_eq!(
            upper.uv_bounds(),
            [0.0, 0.0, 1.0, 0.5],
            "the upper half must start at the texture's first row, v = 0.0"
        );
        assert_eq!(
            lower.uv_bounds(),
            [0.0, 0.5, 1.0, 1.0],
            "the lower half must start halfway down, v = 0.5. Both bounds swapped \
             would mean `top` counts from the bottom, against ADR-0004."
        );

        let vs = |sprite| sprite_vertices(sprite, projection).map(|corner| corner[3]);
        let above = vs(SpriteInstance::new(placement, upper));
        let below = vs(SpriteInstance::new(placement, lower));

        assert_ne!(above, below, "the two halves must produce different v");

        let lowest_above = above.iter().copied().fold(f32::MIN, f32::max);
        let highest_below = below.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            lowest_above <= highest_below,
            "the region at the top of the texture must sample smaller v than the \
             one below it: v grows downward while world y grows up, and the region \
             follows v (ADR-0004). Got {lowest_above} against {highest_below}"
        );
    }

    /// Region boundaries land on texel edges, at the values the division gives.
    #[test]
    fn region_boundaries_land_on_the_texel_edges_the_derivation_gives() {
        let texture = texture_of(16);

        assert_eq!(
            TextureRegion::from_texels(8, 0, 8, 8, &texture).uv_bounds(),
            [0.5, 0.0, 1.0, 0.5],
            "the top-right quadrant of a 16 x 16 texture is u 8/16..16/16, v 0..8/16"
        );
        assert_eq!(
            TextureRegion::from_texels(0, 8, 8, 8, &texture).uv_bounds(),
            [0.0, 0.5, 0.5, 1.0],
            "the bottom-left quadrant"
        );
        assert_eq!(
            TextureRegion::from_texels(3, 5, 1, 1, &texture).uv_bounds(),
            [3.0 / 16.0, 5.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0],
            "one texel spans one texel's width, edge to edge — not centre to centre"
        );

        // A size that is not a power of two, so the division cannot be a shift
        // that happens to look right.
        let odd = texture_of(10);
        assert_eq!(
            TextureRegion::from_texels(2, 4, 3, 1, &odd).uv_bounds(),
            [0.2, 0.4, 0.5, 0.5]
        );
    }

    /// The sprite's top corner samples the region's top edge.
    ///
    /// The region-level twin of `every_corner_table_pairs_the_top_with_the_top_texture_row`.
    /// That guard reads the corner table, whose `v` M3.9 turned into a
    /// region-local coordinate; this asserts what the table's `v = 0.0` now
    /// resolves to, so the two together still cover the whole way from a world
    /// corner to a texture row.
    #[test]
    fn the_sprites_top_corner_samples_the_regions_top_edge() {
        let texture = texture_of(16);
        let region = TextureRegion::from_texels(8, 4, 8, 8, &texture);
        let [_, v_top, _, v_bottom] = region.uv_bounds();
        assert!(v_top < v_bottom, "the fixture region must have height");

        let vertices = sprite_vertices(
            SpriteInstance::new(SpritePlacement::new(32.0, 32.0), region),
            Projection::for_target(64, 64),
        );

        for (index, [_, ndc_y, _, v, ..]) in vertices.into_iter().enumerate() {
            let expected = if ndc_y > 0.0 { v_top } else { v_bottom };
            assert_eq!(
                v, expected,
                "corner {index} sits at NDC y = {ndc_y} and samples v = {v}, but a \
                 corner above the centre must sample the region's top edge \
                 ({v_top}) and one below it the bottom edge ({v_bottom})"
            );
        }
    }

    /// A region built from texels never puts its bottom above its top.
    ///
    /// The M3.5 pairing guard now reads a table of region-*local* coordinates,
    /// and its conclusion — top corner samples the earlier texture row — needs
    /// this to carry over to the texture. It holds for every region whose
    /// `left + width` and `top + height` do not overflow `u32`, which is every
    /// region of a real texture: `Pixels` caps a dimension at
    /// `OffscreenTarget::MAX_DIMENSION`. It is asserted rather than left at
    /// "holds by construction", because that phrase is how the four false
    /// claims of `ProjektPlan.md` §12 were written. The overflowing case is
    /// **not** covered here and is reported instead: it wraps to an inverted
    /// region in a build without overflow checks.
    #[test]
    fn a_region_built_from_texels_never_puts_its_bottom_above_its_top() {
        let texture = texture_of(16);

        for (left, top, width, height) in [
            (0, 0, 16, 16),
            (8, 0, 8, 8),
            (0, 8, 8, 8),
            (3, 5, 1, 1),
            (0, 15, 16, 1),
            (15, 15, 1, 1),
            (0, 0, 0, 0),
        ] {
            let [u_left, v_top, u_right, v_bottom] =
                TextureRegion::from_texels(left, top, width, height, &texture).uv_bounds();

            assert!(
                u_left <= u_right && v_top <= v_bottom,
                "from_texels({left}, {top}, {width}, {height}) produced \
                 [{u_left}, {v_top}, {u_right}, {v_bottom}], which is inverted in at \
                 least one axis. An inverted region mirrors the sprite silently, and \
                 the corner-pairing guard would keep passing while the image flipped."
            );
        }
    }

    /// **The identity camera is the projection that preceded it, bit for bit.**
    ///
    /// The whole regression argument for M3.12 in one assertion. Three of the
    /// four blessed reference images are drawn through `world_to_ndc`, and after
    /// this milestone they are drawn through it *with a camera applied* —
    /// `render_sprites` calls `render_sprites_viewed_by` with
    /// [`CameraView::IDENTITY`]. If `(x - 0.0) * 1.0 / half` were not the same
    /// bits as `x / half`, all three would be rendered from different vertex data
    /// than the human blessed them from. How far the pixels would move is not
    /// known and is not the point; that they would be a different render is.
    ///
    /// Compared on bit patterns against the expression this function had before
    /// M3.12, written out here rather than referred to, because the point is
    /// that the two agree and only one of them still exists. The values are
    /// chosen to stress it: both zeros (where `x - 0.0` could plausibly lose a
    /// sign), both infinities, a subnormal, the largest finite value, and
    /// ordinary coordinates.
    ///
    /// `NaN` is deliberately absent. IEEE 754 does not fix which `NaN` payload
    /// an operation returns, so a bit comparison on it would assert a property
    /// of the hardware rather than of this function — and a `NaN` world
    /// coordinate is a bug in whatever wrote it, which `Transform` does not
    /// normalise away.
    #[test]
    fn the_identity_camera_is_the_projection_that_preceded_it() {
        let projection = Projection::for_target(128, 64);
        assert_eq!(projection.camera(), CameraView::IDENTITY);

        // What `world_to_ndc` was before the camera existed.
        let before = |value: f32, half: f32| value / half;

        for value in [
            0.0_f32,
            -0.0,
            1.0,
            -1.0,
            63.5,
            -63.5,
            0.1,
            -1.0 / 3.0,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::MAX,
            -f32::MAX,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            let [ndc_x, ndc_y] = projection.world_to_ndc(value, value);

            assert_eq!(
                ndc_x.to_bits(),
                before(value, 64.0).to_bits(),
                "x = {value:?}: the identity camera produced {ndc_x:?} where the \
                 pre-M3.12 projection produced {:?}. The three blessed images that \
                 go through a projection are drawn with this camera on every run.",
                before(value, 64.0)
            );
            assert_eq!(
                ndc_y.to_bits(),
                before(value, 32.0).to_bits(),
                "y = {value:?}: {ndc_y:?} against {:?}",
                before(value, 32.0)
            );
        }
    }

    /// A camera that is not the identity actually moves and scales the picture,
    /// and does so on both axes.
    ///
    /// The negative control for the test above: without it, a `world_to_ndc`
    /// that ignored its camera entirely would pass that one and every blessed
    /// image, and only a rendered scene would notice.
    #[test]
    fn a_camera_translates_and_scales_and_does_both_on_both_axes() {
        let projection = Projection::for_target(128, 128);

        // Half the target away from the camera is the edge of the view.
        let moved = projection.viewed_by(CameraView::new(16.0, -8.0, 1.0));
        assert_eq!(moved.world_to_ndc(16.0, -8.0), [0.0, 0.0]);
        assert_eq!(moved.world_to_ndc(80.0, 56.0), [1.0, 1.0]);

        // Zoom 2 puts the edge of the view half as far out.
        let zoomed = projection.viewed_by(CameraView::new(0.0, 0.0, 2.0));
        assert_eq!(zoomed.world_to_ndc(32.0, 32.0), [1.0, 1.0]);
        assert_eq!(
            zoomed.world_to_ndc(64.0, 64.0),
            [2.0, 2.0],
            "larger zoom must show *less*: what filled the view at zoom 1 has to \
             fall outside it at zoom 2. A projection where this came back inside \
             would have the convention reversed."
        );

        // Both axes, separately, so a zoom applied to one only is visible.
        let [x_only, y_zero] = zoomed.world_to_ndc(16.0, 0.0);
        let [x_zero, y_only] = zoomed.world_to_ndc(0.0, 16.0);
        assert_eq!(x_only, 0.5);
        assert_eq!(y_only, 0.5);
        assert_eq!(y_zero, 0.0);
        assert_eq!(x_zero, 0.0);
    }

    /// **Re-viewing a projection through the camera it already has returns it
    /// unchanged, field for field.**
    ///
    /// The instrument for M6b.4's load-bearing assurance: a caller that hands
    /// the overlay batch the scene's own camera gets the pre-M6b.4 frame back,
    /// bit for bit. Both render paths build the overlay's projection as
    /// `projection.viewed_by(batch.camera)` rather than by calling
    /// [`Projection::for_target`] a second time, and
    /// [`Projection::viewed_by`] is `Self { camera, ..self }` — a move of the
    /// camera field and no arithmetic at all. So the half-extents are not
    /// recomputed and cannot drift, and the equality below is structural rather
    /// than a statement about floating point.
    ///
    /// Checked over cameras chosen to have nothing convenient about them:
    /// negative, fractional, and a zoom that is not a power of two.
    #[test]
    fn a_projection_viewed_by_its_own_camera_is_itself() {
        for camera in [
            CameraView::IDENTITY,
            CameraView::new(16.0, -8.0, 1.5),
            CameraView::new(-123.75, 0.125, 0.3),
        ] {
            let projection = Projection::for_target(1280, 720).viewed_by(camera);
            assert_eq!(
                projection.viewed_by(projection.camera()),
                projection,
                "re-viewing through the camera already in place changed the \
                 projection, so an overlay handed the scene's camera would not \
                 reproduce the scene's geometry"
            );
        }
    }

    /// The same statement one level up, on the bytes a vertex buffer receives.
    ///
    /// `PartialEq` on [`Projection`] compares `f32`s, where `NaN` is unequal to
    /// itself and the two zeros are equal to each other — so the test above,
    /// alone, is not quite a statement about *bits*. This one is: it compares
    /// `to_bits` of every float [`batch_vertices`] emits, which is what actually
    /// reaches the GPU and therefore what a blessed reference is a function of.
    #[test]
    fn an_overlay_camera_equal_to_the_scenes_moves_no_vertex_bit() {
        let camera = CameraView::new(-40.5, 12.25, 1.75);
        let projection = Projection::for_target(192, 80).viewed_by(camera);
        let sprites = many(37);

        let through_the_scenes = batch_vertices(&sprites, projection);
        let through_the_overlays = batch_vertices(&sprites, projection.viewed_by(camera));

        assert!(!through_the_scenes.is_empty(), "the fixture drew nothing");
        let bits = |vertices: &[[f32; 8]]| -> Vec<[u32; 8]> {
            vertices
                .iter()
                .map(|corner| corner.map(f32::to_bits))
                .collect()
        };
        assert_eq!(
            bits(&through_the_scenes),
            bits(&through_the_overlays),
            "an overlay carrying the scene's camera produced different vertex \
             bits, so existing callers do not reproduce their frames"
        );
    }

    /// The guard against a silent second y-flip.
    ///
    /// A projection that negated an axis would still produce a plausible image
    /// — upside down, and only detectably so against an asymmetric texture. This
    /// asserts the sign directly, where no image is involved and no tolerance
    /// can absorb it.
    #[test]
    fn no_axis_is_negated_on_the_way_to_ndc() {
        let projection = Projection::for_target(64, 64);

        let [right_x, right_y] = projection.world_to_ndc(16.0, 0.0);
        assert!(right_x > 0.0, "world +x must be NDC +x, got {right_x}");
        assert_eq!(right_y, 0.0);

        let [up_x, up_y] = projection.world_to_ndc(0.0, 16.0);
        assert!(
            up_y > 0.0,
            "world +y must be NDC +y: both point up (ADR-0004). A negative here \
             is the second reconciliation that ADR forbids, and it would show up \
             only as an upside-down image against an asymmetric texture. Got {up_y}"
        );
        assert_eq!(up_x, 0.0);
    }

    /// The inverse really is the inverse, through every camera.
    ///
    /// The guard that keeps `screen_to_world` from becoming a second piece of
    /// camera mathematics: it composes the two directions and asserts identity,
    /// so a sign, a zoom or a half-extent that disagreed between them shows up
    /// here rather than as a click that lands somewhere near the sprite.
    #[test]
    fn a_world_point_survives_the_round_trip_through_the_screen() {
        for camera in [
            CameraView::IDENTITY,
            CameraView::new(100.0, -50.0, 1.0),
            CameraView::new(0.0, 0.0, 2.0),
            CameraView::new(-12.5, 7.25, 0.5),
        ] {
            let projection = Projection::for_target(1280, 720).viewed_by(camera);

            for (x, y) in [(0.0, 0.0), (100.0, 100.0), (-320.0, 180.0), (3.5, -2.25)] {
                let [ndc_x, ndc_y] = projection.world_to_ndc(x, y);

                // NDC back to pixels is the viewport transform, which the GPU
                // does on the way out and nothing does on the way back.
                let px = (ndc_x + 1.0) * 640.0;
                let py = (1.0 - ndc_y) * 360.0;

                let [back_x, back_y] = projection.screen_to_world(px, py);

                assert!(
                    (back_x - x).abs() < 1e-3 && (back_y - y).abs() < 1e-3,
                    "({x}, {y}) through {camera:?} came back as ({back_x}, {back_y})"
                );
            }
        }
    }

    /// Hand-computed pixel positions, asserted exactly.
    ///
    /// **This is the test that is not vacuous.** A round trip cannot tell a
    /// correct pair of functions from two that share the same wrong half-extent,
    /// so the anchor for `world_to_screen` has to be an absolute answer worked
    /// out from the mapping's definition rather than from either function. Every
    /// value below is exact in `f32` — the half-extents are 640 and 360, the NDC
    /// values are 0, ±0.5 and ±1, and every operation on the path is a subtract,
    /// a multiply by a power-of-two-friendly ratio or an add of 1.0 — so this is
    /// `assert_eq!` and not a tolerance.
    ///
    /// It doubles as the non-vacuity check the round trip needs: a function that
    /// returned its argument unchanged fails on the very first pair, since
    /// `(0.0, 0.0)` maps to the centre of the target and not to its corner.
    #[test]
    fn a_world_point_lands_on_the_pixel_it_can_be_worked_out_to() {
        let plain = Projection::for_target(1280, 720);

        // The centre of the target is the camera's own position, the corners are
        // the half-extents, and y is mirrored on the way out.
        assert_eq!(plain.world_to_screen(0.0, 0.0), [640.0, 360.0]);
        assert_eq!(plain.world_to_screen(640.0, 360.0), [1280.0, 0.0]);
        assert_eq!(plain.world_to_screen(-640.0, -360.0), [0.0, 720.0]);
        assert_eq!(plain.world_to_screen(320.0, 180.0), [960.0, 180.0]);
        assert_eq!(plain.world_to_screen(-320.0, -180.0), [320.0, 540.0]);

        // A panned camera moves the world point that lands in the centre, and
        // moves nothing else about the mapping.
        let panned = plain.viewed_by(CameraView::new(100.0, -50.0, 1.0));
        assert_eq!(panned.world_to_screen(100.0, -50.0), [640.0, 360.0]);
        assert_eq!(panned.world_to_screen(740.0, 310.0), [1280.0, 0.0]);

        // Zoom halves the world distance a pixel covers, so the corner sits at
        // half the world coordinate it did at zoom 1.
        let zoomed = plain.viewed_by(CameraView::new(0.0, 0.0, 2.0));
        assert_eq!(zoomed.world_to_screen(320.0, 180.0), [1280.0, 0.0]);
        assert_eq!(zoomed.world_to_screen(160.0, 90.0), [960.0, 180.0]);

        // Both at once, on values chosen to stay exact.
        let both = plain.viewed_by(CameraView::new(-12.5, 7.25, 0.5));
        assert_eq!(both.world_to_screen(-12.5, 7.25), [640.0, 360.0]);
        assert_eq!(both.world_to_screen(1267.5, 727.25), [1280.0, 0.0]);
    }

    /// The sign, on its own, so a flip cannot hide inside a symmetric picture.
    ///
    /// World y runs up and pixel rows run down (ADR-0004), so moving *up* in the
    /// world must move *towards row zero*. Asserted as an ordering rather than as
    /// values, because an ordering is what a caller depends on and it stays true
    /// under any target size.
    ///
    /// X is here too, as the control: it is **not** flipped, and a test that only
    /// looked at y would pass on an implementation that mirrored both.
    #[test]
    fn the_screen_y_axis_runs_down_while_the_world_y_axis_runs_up() {
        let projection = Projection::for_target(1280, 720);

        let [centre_x, centre_y] = projection.world_to_screen(0.0, 0.0);

        let [_, above] = projection.world_to_screen(0.0, 100.0);
        let [_, below] = projection.world_to_screen(0.0, -100.0);

        assert!(
            above < centre_y && centre_y < below,
            "a point above the camera must land nearer row zero: \
             {above}, {centre_y}, {below}"
        );

        let [left, _] = projection.world_to_screen(-100.0, 0.0);
        let [right, _] = projection.world_to_screen(100.0, 0.0);
        assert!(
            left < centre_x && centre_x < right,
            "x is not mirrored: {left}, {centre_x}, {right}"
        );
    }

    /// The two directions compose to the identity, over four cameras and three
    /// target shapes.
    ///
    /// # The tolerance is derived, not chosen
    ///
    /// The path performs ten `f32` operations: three in `world_to_ndc`
    /// (subtract, multiply, divide), two in `world_to_screen` (add, multiply)
    /// and five in `screen_to_world` (divide, subtract, multiply, divide, add).
    /// Each rounds to nearest, so each contributes at most half an ulp — a
    /// relative error of `u = 2^-24`, which is `f32::EPSILON / 2`.
    ///
    /// The NDC intermediate passes through `+ 1.0` and `- 1.0`, which turns its
    /// relative error into an **absolute** one at the scale of 1.0, and the way
    /// back then multiplies it by `half_extent / zoom`. The world-space error is
    /// therefore bounded by `n * u * (half_extent / zoom + |x - camera|)`, with
    /// the second term covering the cancellation in `x - camera` itself.
    ///
    /// Sixteen is used for `n` rather than the ten actually counted, so the
    /// bound does not become wrong the day an operation is added — `16 * u` is
    /// `8 * f32::EPSILON`. Sizes that are not powers of two are in the list on
    /// purpose: `192 / 2` makes `x / half_width` inexact, which is the same
    /// rounding ADR-0045 measured for `anchor`.
    #[test]
    fn a_world_point_returns_from_the_pixel_it_was_sent_to() {
        for (width, height) in [(1280u32, 720u32), (192, 128), (100, 64)] {
            for camera in [
                CameraView::IDENTITY,
                CameraView::new(100.0, -50.0, 1.0),
                CameraView::new(0.0, 0.0, 2.0),
                CameraView::new(-12.5, 7.25, 0.5),
            ] {
                let projection = Projection::for_target(width, height).viewed_by(camera);
                let half_x = width as f32 / 2.0;
                let half_y = height as f32 / 2.0;

                for (x, y) in [(0.0, 0.0), (37.0, -19.0), (-320.0, 180.0), (3.5, -2.25)] {
                    let [px, py] = projection.world_to_screen(x, y);
                    let [back_x, back_y] = projection.screen_to_world(px, py);

                    let tol_x = 8.0 * f32::EPSILON * (half_x / camera.zoom + (x - camera.x).abs());
                    let tol_y = 8.0 * f32::EPSILON * (half_y / camera.zoom + (y - camera.y).abs());

                    assert!(
                        (back_x - x).abs() <= tol_x && (back_y - y).abs() <= tol_y,
                        "({x}, {y}) through {camera:?} on {width}x{height} went to \
                         ({px}, {py}) and came back as ({back_x}, {back_y})"
                    );
                }
            }
        }
    }

    /// `anchor` and `world_to_screen` agree about which pixel was asked for.
    ///
    /// The sharing obligation, checked through a second entry point rather than
    /// by reading the source. `anchor` picks a pixel and converts it inward;
    /// `world_to_screen` converts the result back out. If either had its own idea
    /// of a half-extent or a sign, the pixel that comes back would not be the one
    /// the anchor named — and the four corners are here because a single corner
    /// is satisfied by a mapping that is wrong on both axes at once.
    ///
    /// The tolerance is the pixel-space form of the one derived in
    /// `a_world_point_returns_from_the_pixel_it_was_sent_to`, and the values are
    /// small: `anchor`'s own documented rounding is about four parts in a million
    /// of a world unit on this target.
    #[test]
    fn an_anchored_point_maps_back_to_the_pixel_it_was_anchored_to() {
        let projection = Projection::for_target(192, 128);
        let tol = 8.0 * f32::EPSILON * 192.0;

        for (anchor, expected) in [
            (ScreenAnchor::BottomLeft, [8.0, 120.0]),
            (ScreenAnchor::TopLeft, [8.0, 8.0]),
            (ScreenAnchor::TopRight, [184.0, 8.0]),
            (ScreenAnchor::BottomRight, [184.0, 120.0]),
        ] {
            let world = projection.anchor(anchor, 8.0, 8.0);
            let [px, py] = projection.world_to_screen(world[0], world[1]);

            assert!(
                (px - expected[0]).abs() <= tol && (py - expected[1]).abs() <= tol,
                "{anchor:?} named pixel {expected:?} but the world point \
                 {world:?} maps back to ({px}, {py})"
            );
        }
    }

    /// The whole of M6b.8's S2, in one assertion pair.
    ///
    /// An edge-anchored point keeps its distance to its **edge** across target
    /// sizes; a centre-anchored point keeps its distance to the **centre**. Both
    /// halves are here on purpose and neither is enough alone: a broken `anchor`
    /// that returned the centre-anchored point would satisfy the second and fail
    /// the first, and one that returned a constant would satisfy the first at a
    /// single size and fail across two. Checking them against each other is what
    /// separates "it sits right" from "it sits right by accident".
    #[test]
    fn an_edge_anchor_holds_its_edge_while_a_centre_anchor_holds_its_centre() {
        let small = Projection::for_target(192, 128);
        let large = Projection::for_target(256, 160);

        // Edge-anchored: eight pixels in from the bottom left corner. In world
        // units that is (-half_w + 8, -half_h + 8), which differs between the
        // two targets — and that difference is exactly the point, because the
        // *pixel* distance to the corner is what stays.
        let [small_x, small_y] = small.anchor(ScreenAnchor::BottomLeft, 8.0, 8.0);
        let [large_x, large_y] = large.anchor(ScreenAnchor::BottomLeft, 8.0, 8.0);

        assert_eq!((small_x, small_y), (-88.0, -56.0));
        assert_eq!((large_x, large_y), (-120.0, -72.0));

        // The distance to the corner, in pixels, is the same on both.
        assert_eq!(small_x - (-96.0), large_x - (-128.0));
        assert_eq!(small_y - (-64.0), large_y - (-80.0));

        // Centre-anchored with the same inset: the same world point on both, to
        // within the rounding `anchor` inherits from `screen_to_world` — see
        // `an_anchor_carries_the_rounding_of_the_conversion_it_shares`, which
        // measures that error rather than tolerating it silently. A *different*
        // distance to the corner on each is the consequence.
        let centred_small = small.anchor(ScreenAnchor::Centre, 8.0, 8.0);
        let centred_large = large.anchor(ScreenAnchor::Centre, 8.0, 8.0);

        assert!(
            (centred_small[0] - centred_large[0]).abs() < 1e-4
                && (centred_small[1] - centred_large[1]).abs() < 1e-4,
            "a centre anchor moved between target sizes: {centred_small:?} vs {centred_large:?}"
        );

        // And the two disagree by a whole target, which is the whole reason the
        // anchor exists. Exact, because this difference is not a rounding one.
        assert_ne!([small_x, small_y], centred_small);
        assert!((small_x - centred_small[0]).abs() > 90.0);
    }

    /// `anchor` rounds exactly as `screen_to_world` rounds, and no better.
    ///
    /// The property is deliberate and is the reason `anchor`'s body picks a
    /// pixel and delegates: an exact formula here would be a *second* piece of
    /// camera mathematics, and it would disagree with the one the click path and
    /// the round-trip test already use. Agreeing bit for bit with the shared
    /// conversion is worth more than agreeing with the arithmetic on paper.
    ///
    /// The error is measured here so that "negligible" is a number rather than
    /// an opinion, and so that a change which made `anchor` exact would fail
    /// this test and have to argue with the paragraph above.
    #[test]
    fn an_anchor_carries_the_rounding_of_the_conversion_it_shares() {
        // 192 is not a power of two, so `x / half_width - 1.0` is inexact.
        let projection = Projection::for_target(192, 128);

        let [x, _] = projection.anchor(ScreenAnchor::Centre, 8.0, 8.0);
        assert_ne!(x, 8.0, "the inexact case stopped being inexact");
        assert!((x - 8.0).abs() < 1e-5, "the error grew: {x}");

        // Bit for bit what the shared conversion returns for the same pixel.
        assert_eq!(
            projection.anchor(ScreenAnchor::Centre, 8.0, 8.0),
            projection.screen_to_world(104.0, 56.0)
        );

        // A power-of-two target divides exactly, and then so does the anchor.
        let exact = Projection::for_target(256, 128);
        assert_eq!(exact.anchor(ScreenAnchor::Centre, 8.0, 8.0), [8.0, 8.0]);
    }

    /// Every anchor, and the inset pointing inward on all nine.
    ///
    /// The property that makes a HUD's four corners spell alike: the same
    /// `(8.0, 8.0)` reads as "eight pixels in" wherever it is used, so no call
    /// site carries a sign of its own.
    #[test]
    fn a_positive_inset_moves_inward_from_every_anchor() {
        let projection = Projection::for_target(200, 100);
        let (half_w, half_h) = (100.0, 50.0);

        for (anchor, expected) in [
            (ScreenAnchor::TopLeft, [-half_w + 8.0, half_h - 8.0]),
            (ScreenAnchor::Top, [8.0, half_h - 8.0]),
            (ScreenAnchor::TopRight, [half_w - 8.0, half_h - 8.0]),
            (ScreenAnchor::Left, [-half_w + 8.0, 8.0]),
            (ScreenAnchor::Centre, [8.0, 8.0]),
            (ScreenAnchor::Right, [half_w - 8.0, 8.0]),
            (ScreenAnchor::BottomLeft, [-half_w + 8.0, -half_h + 8.0]),
            (ScreenAnchor::Bottom, [8.0, -half_h + 8.0]),
            (ScreenAnchor::BottomRight, [half_w - 8.0, -half_h + 8.0]),
        ] {
            let [x, y] = projection.anchor(anchor, 8.0, 8.0);
            assert!(
                (x - expected[0]).abs() < 1e-4 && (y - expected[1]).abs() < 1e-4,
                "{anchor:?} did not put a positive inset inward: got [{x}, {y}], \
                 wanted {expected:?}"
            );
        }
    }

    /// A zero inset lands exactly on the named corner, edge or centre.
    ///
    /// The boundary case, and it is the one that pins the *sign convention* to
    /// the target's own extent rather than to a half-extent chosen here.
    #[test]
    fn a_zero_inset_is_the_anchor_itself() {
        let projection = Projection::for_target(64, 32);

        assert_eq!(
            projection.anchor(ScreenAnchor::TopLeft, 0.0, 0.0),
            [-32.0, 16.0]
        );
        assert_eq!(
            projection.anchor(ScreenAnchor::BottomRight, 0.0, 0.0),
            [32.0, -16.0]
        );
        assert_eq!(
            projection.anchor(ScreenAnchor::Centre, 0.0, 0.0),
            [0.0, 0.0]
        );
    }

    /// The trap named in `anchor`'s documentation, asserted rather than written.
    ///
    /// Anchoring against a panned camera and drawing through the identity is the
    /// mistake a HUD author can make; this says the two really do differ, so the
    /// warning in the doc comment is a measurement and not an opinion.
    #[test]
    fn an_anchor_moves_with_the_camera() {
        let fixed = Projection::for_target(192, 128);
        let panned = fixed.viewed_by(CameraView::new(100.0, -50.0, 1.0));
        let zoomed = fixed.viewed_by(CameraView::new(0.0, 0.0, 2.0));

        let here = fixed.anchor(ScreenAnchor::BottomLeft, 8.0, 8.0);

        assert_ne!(panned.anchor(ScreenAnchor::BottomLeft, 8.0, 8.0), here);
        assert_ne!(zoomed.anchor(ScreenAnchor::BottomLeft, 8.0, 8.0), here);

        // And the identity projection is the one a screen-fixed batch wants:
        // `Projection::for_target` already carries `CameraView::IDENTITY`.
        assert_eq!(fixed.camera(), CameraView::IDENTITY);
    }

    /// The anchor is `screen_to_world` and adds no arithmetic of its own.
    ///
    /// Asserted on the pixel each anchor names, so that a later change which
    /// inlined the conversion — and could then drift from it — fails here.
    #[test]
    fn an_anchor_is_the_screen_point_it_names() {
        let projection = Projection::for_target(192, 128);

        assert_eq!(
            projection.anchor(ScreenAnchor::BottomLeft, 8.0, 8.0),
            projection.screen_to_world(8.0, 120.0)
        );
        assert_eq!(
            projection.anchor(ScreenAnchor::TopRight, 8.0, 8.0),
            projection.screen_to_world(184.0, 8.0)
        );
        assert_eq!(
            projection.anchor(ScreenAnchor::Centre, 0.0, 0.0),
            projection.screen_to_world(96.0, 64.0)
        );
    }

    /// Screen y runs down while world y runs up, and the centre is the camera.
    ///
    /// Stated as three separate facts rather than inferred from the round trip,
    /// because a round trip is satisfied by two mistakes that cancel.
    #[test]
    fn the_screen_direction_is_down_and_the_world_direction_is_up() {
        let projection = Projection::for_target(1280, 720);

        let [centre_x, centre_y] = projection.screen_to_world(640.0, 360.0);
        assert_eq!(
            (centre_x, centre_y),
            (0.0, 0.0),
            "the middle of the target is the camera"
        );

        let [_, above] = projection.screen_to_world(640.0, 0.0);
        assert!(above > 0.0, "the top row is world +y, got {above}");

        let [_, below] = projection.screen_to_world(640.0, 720.0);
        assert!(below < 0.0, "the bottom row is world -y, got {below}");

        let [right, _] = projection.screen_to_world(1280.0, 360.0);
        assert!(right > 0.0, "the right edge is world +x, got {right}");
    }

    /// Zoom shrinks what a pixel is worth, and the camera moves the origin.
    #[test]
    fn zoom_and_offset_reach_the_inverse_too() {
        let zoomed = Projection::for_target(1280, 720).viewed_by(CameraView::new(0.0, 0.0, 2.0));
        let [x, _] = zoomed.screen_to_world(1280.0, 360.0);
        assert_eq!(x, 320.0, "at zoom 2 the right edge is half as far out");

        let moved = Projection::for_target(1280, 720).viewed_by(CameraView::new(100.0, 50.0, 1.0));
        let [cx, cy] = moved.screen_to_world(640.0, 360.0);
        assert_eq!((cx, cy), (100.0, 50.0), "the centre is where the camera is");
    }

    #[test]
    fn the_target_edges_are_half_the_target_size_away_in_world_units() {
        let projection = Projection::for_target(64, 32);

        assert_eq!(projection.world_to_ndc(32.0, 16.0), [1.0, 1.0]);
        assert_eq!(projection.world_to_ndc(-32.0, -16.0), [-1.0, -1.0]);
        assert_eq!(projection.world_to_ndc(0.0, 0.0), [0.0, 0.0]);
    }

    /// Every corner table in this crate that pairs a height with a texture row.
    ///
    /// Two entries, and the guard below walks all of them. A third path — a
    /// batch, a nine-slice — adds one line here and is covered from that moment,
    /// which is the property the verification-set guards in `xtask` have and
    /// this convention did not.
    const PAIRING_TABLES: [(&str, [[f32; 4]; 4]); 2] = [
        ("quad.rs VERTICES", crate::quad::VERTICES),
        ("sprite.rs SPRITE_CORNERS", SPRITE_CORNERS),
    ];

    /// Renders both tables for a failure message. A difference is unreadable
    /// without the two things that differ.
    fn both_tables() -> String {
        PAIRING_TABLES
            .iter()
            .map(|(name, table)| format!("  {name}: {table:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The corner that carries the top of the image carries `v = 0.0` — in
    /// **every** table, not just the one nearest to hand.
    ///
    /// This is the pairing that reconciles NDC y-up against texture rows running
    /// down, and it is written out twice: once for the screen-filling quad and
    /// once for a placed sprite. Two separate draw paths, so it is not a
    /// violation of ADR-0004's "exactly once" — but it is one convention in two
    /// places that have to move together, which is the shape this project
    /// already failed at once, in M2.8, when a trigger list lived in two files
    /// and only one of them was updated.
    ///
    /// Until M3.5 the assertion below read only `SPRITE_CORNERS`. `VERTICES` was
    /// covered only by tests that need a GPU adapter and compare an image, which
    /// skip on a machine that has none and can be weakened by changing a
    /// fixture. This costs nothing and holds everywhere.
    #[test]
    fn every_corner_table_pairs_the_top_with_the_top_texture_row() {
        for (name, table) in PAIRING_TABLES {
            for [x, y, _u, v] in table {
                let expected = if y > 0.0 { 0.0 } else { 1.0 };

                assert_eq!(
                    v,
                    expected,
                    "{name} pairs the corner at ({x}, {y}) with v = {v}, but a \
                     corner at y = {y} must carry v = {expected}: NDC and world y \
                     point up while texture rows run down, so the *top* corner \
                     samples the *first* row (ADR-0004). Both tables carry this \
                     pairing and have to agree; flipping one of them would show \
                     the same texture the other way up depending on which draw \
                     path produced it.\n{}",
                    both_tables()
                );
            }
        }
    }

    /// The tables have to stay comparable, or the guard above compares nothing.
    ///
    /// It reads the sign of the second column in each table. If one table ever
    /// changed what that column means — normalised device coordinates in one,
    /// local sprite units in the other, both centred on zero today — the guard
    /// would keep passing while checking a different property in each.
    #[test]
    fn both_corner_tables_are_centred_on_zero_so_the_sign_means_the_same_thing() {
        for (name, table) in PAIRING_TABLES {
            let ys: Vec<f32> = table.iter().map(|corner| corner[1]).collect();
            let sum: f32 = ys.iter().sum();

            assert_eq!(
                sum,
                0.0,
                "{name} is not centred on zero (its y values sum to {sum}), so the \
                 sign of a corner's y no longer separates top from bottom and the \
                 pairing guard is reading something else.\n{}",
                both_tables()
            );
            assert_eq!(ys.len(), 4, "{name} must have four corners");
        }
    }

    #[test]
    fn an_unturned_sprite_covers_exactly_its_scale_in_pixels() {
        // 32 x 32 world units on a 64 x 64 target: half the width each way, so
        // NDC -0.5 ..= 0.5.
        let vertices = sprite_vertices(
            SpriteInstance::whole_texture(SpritePlacement::new(32.0, 32.0)),
            Projection::for_target(64, 64),
        );

        let xs: Vec<f32> = vertices.iter().map(|v| v[0]).collect();
        let ys: Vec<f32> = vertices.iter().map(|v| v[1]).collect();

        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), -0.5);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 0.5);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), -0.5);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 0.5);
    }

    /// A quarter turn counter-clockwise sends the top-left corner to the bottom
    /// left, not to the top right.
    #[test]
    fn positive_rotation_turns_counter_clockwise() {
        let placement = SpritePlacement::new(32.0, 32.0).turned(core::f32::consts::FRAC_PI_2);
        let vertices = sprite_vertices(
            SpriteInstance::whole_texture(placement),
            Projection::for_target(64, 64),
        );

        // SPRITE_CORNERS[0] is the top-left corner, local (-0.5, +0.5). Turned a
        // quarter turn counter-clockwise about the origin it lands at
        // (-0.5, -0.5): bottom left.
        let [x, y, ..] = vertices[0];
        assert!(
            x < 0.0 && y < 0.0,
            "the top-left corner must land bottom-left after +90 degrees, got ({x}, {y})"
        );
    }

    #[test]
    fn translation_moves_the_sprite_and_leaves_its_size_alone() {
        let projection = Projection::for_target(64, 64);
        let centred = sprite_vertices(
            SpriteInstance::whole_texture(SpritePlacement::new(32.0, 32.0)),
            projection,
        );
        let moved = sprite_vertices(
            SpriteInstance::whole_texture(SpritePlacement {
                x: 16.0,
                ..SpritePlacement::new(32.0, 32.0)
            }),
            projection,
        );

        for (before, after) in centred.iter().zip(moved.iter()) {
            assert_eq!(
                after[0] - before[0],
                0.5,
                "16 world units is half of 32 NDC-wise"
            );
            assert_eq!(after[1], before[1], "moving along x must not move y");
        }
    }

    // --- D15: the decomposition, proved without a GPU ----------------------

    /// A sprite at `x` wishing for `filter`. Only x varies, so a sprite's
    /// identity is readable in one number.
    fn wishing(x: f32, filter: SpriteFilter) -> SpriteInstance {
        SpriteInstance::new(
            SpritePlacement {
                x,
                y: 0.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 1.0,
                scale_y: 1.0,
            },
            TextureRegion::WHOLE_TEXTURE,
        )
        .sampled(filter)
    }

    #[test]
    fn a_sequence_without_a_switch_is_one_run() {
        let sprites: Vec<SpriteInstance> = (0..5)
            .map(|i| wishing(i as f32, SpriteFilter::Nearest))
            .collect();
        assert_eq!(batch_runs(&sprites), vec![0..5]);
    }

    /// The M3.19 worst case: every sprite forces a switch.
    #[test]
    fn alternating_filters_give_one_run_per_sprite() {
        let sprites: Vec<SpriteInstance> = (0..8)
            .map(|i| {
                wishing(
                    i as f32,
                    if i % 2 == 0 {
                        SpriteFilter::Nearest
                    } else {
                        SpriteFilter::Linear
                    },
                )
            })
            .collect();

        let runs = batch_runs(&sprites);
        assert_eq!(runs.len(), sprites.len(), "runs: {runs:?}");
        assert_eq!(runs, (0..8).map(|i| i..i + 1).collect::<Vec<_>>());
    }

    #[test]
    fn the_cut_falls_exactly_at_the_switch() {
        // Three Nearest, then two Linear, then one Nearest.
        let sprites = vec![
            wishing(0.0, SpriteFilter::Nearest),
            wishing(1.0, SpriteFilter::Nearest),
            wishing(2.0, SpriteFilter::Nearest),
            wishing(3.0, SpriteFilter::Linear),
            wishing(4.0, SpriteFilter::Linear),
            wishing(5.0, SpriteFilter::Nearest),
        ];
        assert_eq!(batch_runs(&sprites), vec![0..3, 3..5, 5..6]);
    }

    #[test]
    fn an_empty_sequence_has_no_runs() {
        assert!(batch_runs(&[]).is_empty());
        // And the only case that yields none: one sprite yields one run.
        assert_eq!(
            batch_runs(&[wishing(0.0, SpriteFilter::Nearest)]),
            vec![0..1]
        );
    }

    /// **The D15 guard.** The draw-call boundaries do not change the visible
    /// order.
    ///
    /// The visible order is the sequence order — there is no depth buffer, so a
    /// later draw covers an earlier one, and the runs are issued in order. So
    /// the property is checkable here, without rendering: the runs, concatenated
    /// in order, must be the input **bit for bit**.
    ///
    /// `to_bits()` rather than `==`, because a placement is `f32` and two values
    /// that compare equal can carry different bits — and a decomposition that
    /// replaced a sprite with an equal-but-not-identical one would still be
    /// wrong. It also refuses to be satisfied by `NaN`, which compares unequal
    /// to itself.
    ///
    /// **Which half of this test does the work, honestly.** While `batch_runs`
    /// returns *ranges into the caller's slice*, "the runs are in order" and
    /// "the ranges tile 0..len contiguously and ascending" are the same
    /// statement, so the tiling checks below are what a reordering trips — the
    /// concatenation never gets the chance. Demonstrated: reversing the returned
    /// runs fails at `runs.first()` with `Some(4)` against `Some(0)`. The
    /// concatenation check is kept because it is the property D15 actually
    /// names, and because it is what would still hold if `batch_runs` ever
    /// returned sprites instead of ranges — at which point the tiling checks
    /// would no longer apply and this would be the only guard left.
    ///
    /// Nothing here is anchored to a blessed artifact (§7): the property is
    /// about the function's own input and output.
    /// **An empty second batch produces nothing at all.**
    ///
    /// M6.6c's halt made this a condition of the seam, and the reason is the
    /// regression evidence for the ten blessed references: if an empty batch
    /// still produced a run, the references would only be safe because the
    /// overlay happens to be off, which is the weaker of the two claims. Stated
    /// as an equality against the plan for the scene alone, it is the stronger
    /// one — the command sequence is the same sequence.
    ///
    /// Asserted for three shapes, because "no runs" and "one run" and "several
    /// runs" fail differently: an off-by-one in the offset would pass the first
    /// two.
    #[test]
    fn an_empty_second_batch_adds_nothing_to_the_plan() {
        for scene in [
            Vec::new(),
            vec![sprite(SpriteFilter::Nearest)],
            vec![
                sprite(SpriteFilter::Nearest),
                sprite(SpriteFilter::Linear),
                sprite(SpriteFilter::Nearest),
            ],
        ] {
            let alone: Vec<_> = batch_runs(&scene)
                .into_iter()
                .map(|run| (run, BatchOf::First))
                .collect();

            assert_eq!(
                batch_plan(&scene, &[]),
                alone,
                "an empty second batch changed the plan for a scene of {} sprites",
                scene.len()
            );
        }
    }

    /// The counter-proof: a **non-empty** second batch does add to the plan.
    ///
    /// Without this, the test above would pass for a `batch_plan` that ignored
    /// its second argument entirely — an instrument that always reports
    /// "nothing was produced" is not an instrument.
    #[test]
    fn a_second_batch_adds_its_runs_offset_past_the_first() {
        let scene = vec![sprite(SpriteFilter::Nearest), sprite(SpriteFilter::Linear)];
        let overlay = vec![sprite(SpriteFilter::Nearest), sprite(SpriteFilter::Nearest)];

        let plan = batch_plan(&scene, &overlay);

        assert_eq!(
            plan,
            vec![
                (0..1, BatchOf::First),
                (1..2, BatchOf::First),
                (2..4, BatchOf::Second),
            ],
            "the second batch must be cut on its own and offset by the first's length"
        );
    }

    /// The two batches never share a run, whatever their filters.
    ///
    /// The property that keeps a run's texture unambiguous: `encode_runs` binds
    /// once per run, so a run spanning the split would have to sample two
    /// textures at once. Here both batches are entirely `Nearest`, which is the
    /// case a filter-only cut would happily merge.
    #[test]
    fn a_run_never_spans_the_two_batches() {
        let scene = vec![sprite(SpriteFilter::Nearest), sprite(SpriteFilter::Nearest)];
        let overlay = vec![sprite(SpriteFilter::Nearest)];

        let plan = batch_plan(&scene, &overlay);

        assert_eq!(plan.len(), 2, "one run each, not one merged run");
        assert_eq!(plan[0], (0..2, BatchOf::First));
        assert_eq!(plan[1], (2..3, BatchOf::Second));
    }

    /// The plan covers every sprite of both batches exactly once, in order.
    ///
    /// `batch_runs` promises this for one slice; the concatenation has to keep
    /// it, because the ranges index one shared vertex buffer and a gap or an
    /// overlap would draw the wrong quads rather than fail.
    #[test]
    fn the_plan_covers_both_batches_exactly_once() {
        let scene = vec![
            sprite(SpriteFilter::Nearest),
            sprite(SpriteFilter::Linear),
            sprite(SpriteFilter::Linear),
        ];
        let overlay = vec![sprite(SpriteFilter::Linear), sprite(SpriteFilter::Nearest)];

        let mut next = 0;
        for (run, _) in batch_plan(&scene, &overlay) {
            assert_eq!(run.start, next, "the plan skipped or repeated an index");
            next = run.end;
        }
        assert_eq!(next, scene.len() + overlay.len());
    }

    /// A sprite with `filter`, at the origin, showing the whole texture.
    ///
    /// The plan is decided by filters and lengths alone, so nothing else about
    /// the sprite matters here.
    fn sprite(filter: SpriteFilter) -> SpriteInstance {
        SpriteInstance::new(SpritePlacement::new(1.0, 1.0), TextureRegion::WHOLE_TEXTURE)
            .sampled(filter)
    }

    #[test]
    fn the_runs_are_the_input_in_order() {
        // A deliberately awkward sequence: switches at both ends, a long middle
        // run, and a placement carrying bits that `==` alone would not pin.
        let mut sprites = vec![
            wishing(f32::MIN_POSITIVE, SpriteFilter::Linear),
            wishing(-0.0, SpriteFilter::Nearest),
            wishing(0.0, SpriteFilter::Nearest),
            wishing(f32::INFINITY, SpriteFilter::Nearest),
            wishing(1.5, SpriteFilter::Linear),
        ];
        sprites.push(wishing(-2.5, SpriteFilter::Linear));

        let runs = batch_runs(&sprites);

        // The ranges tile the input: contiguous, ascending, covering it exactly.
        assert_eq!(runs.first().map(|r| r.start), Some(0));
        assert_eq!(runs.last().map(|r| r.end), Some(sprites.len()));
        for pair in runs.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "runs are not contiguous");
        }
        for run in &runs {
            assert!(run.start < run.end, "a run must not be empty: {run:?}");
        }

        // And concatenating them reproduces the input bit for bit.
        let concatenated: Vec<SpriteInstance> = runs
            .iter()
            .flat_map(|run| sprites[run.clone()].iter().copied())
            .collect();
        assert_eq!(concatenated.len(), sprites.len());
        for (index, (got, want)) in concatenated.iter().zip(sprites.iter()).enumerate() {
            assert_eq!(
                got.placement.x.to_bits(),
                want.placement.x.to_bits(),
                "sprite {index} moved or changed: the decomposition reordered the \
                 sequence, and with no depth buffer the sequence order *is* the \
                 visible order"
            );
            assert_eq!(got.filter, want.filter, "sprite {index} changed filter");
        }
    }

    /// Every sprite lands in exactly one run, and in its own place.
    ///
    /// The guard above already excludes overlaps and gaps, by asserting the
    /// ranges tile 0..len contiguously — so this is not covering a hole in it
    /// today. It is here because it is the check that survives a `batch_runs`
    /// which stopped returning ranges, where the tiling assertions would no
    /// longer apply, and because it is the only one whose failure names the
    /// offending index.
    #[test]
    fn every_sprite_belongs_to_exactly_one_run() {
        let sprites: Vec<SpriteInstance> = (0..7)
            .map(|i| {
                wishing(
                    i as f32,
                    if i == 3 {
                        SpriteFilter::Linear
                    } else {
                        SpriteFilter::Nearest
                    },
                )
            })
            .collect();

        let mut seen = vec![0_u32; sprites.len()];
        for run in batch_runs(&sprites) {
            for index in run {
                seen[index] += 1;
            }
        }
        assert!(
            seen.iter().all(|&n| n == 1),
            "each sprite must be drawn exactly once: {seen:?}"
        );
    }

    /// What the decomposition costs, beside the figure M3.19 bounded it by.
    ///
    /// M3.19 *calculated* that a run-length walk over the already-sorted list is
    /// the same shape of work as the "pure copy of the placements" column of
    /// `BASELINE.md`'s preparation table — 14.6 µs at 50 000 sprites, 1.4 % of
    /// preparation. This measures it instead. Best of five after one unmeasured
    /// warm-up, the shape the preparation table uses.
    ///
    /// Printed rather than asserted against a budget: a duration is a property
    /// of the machine, and `BASELINE.md` is where the number belongs. What *is*
    /// asserted is the run itself — `vec![0..50_000]`, not merely a length of
    /// one, which `vec![0..0]` would also satisfy.
    #[test]
    fn the_cost_of_the_decomposition_is_recorded() {
        let sprites: Vec<SpriteInstance> = (0..50_000)
            .map(|i| wishing(i as f32, SpriteFilter::Nearest))
            .collect();

        let _ = batch_runs(&sprites);

        let mut best = u128::MAX;
        for _ in 0..5 {
            let start = Instant::now();
            let runs = black_box(batch_runs(black_box(&sprites)));
            let elapsed = start.elapsed().as_nanos();
            assert_eq!(runs, vec![0..50_000], "one filter in the batch is one run");
            best = best.min(elapsed);
        }

        #[expect(
            clippy::cast_precision_loss,
            reason = "a microsecond figure needs three significant digits"
        )]
        let micros = best as f64 / 1000.0;
        assert!(best > 0, "the fastest of five rounds took zero nanoseconds");
        println!(
            "batch_runs over 50 000 sprites, one filter: {micros:.1} us, best of five (the copy column M3.19 bounded it by is 14.6 us)"
        );
    }

    /// A sprite's filter defaults to what every sprite did before D15.
    #[test]
    fn a_sprite_wishes_for_nearest_unless_it_says_otherwise() {
        let placement = SpritePlacement::new(1.0, 1.0);
        assert_eq!(
            SpriteInstance::whole_texture(placement).filter,
            SpriteFilter::Nearest
        );
        assert_eq!(
            SpriteInstance::new(placement, TextureRegion::WHOLE_TEXTURE).filter,
            SpriteFilter::Nearest
        );
        assert_eq!(SpriteFilter::default(), SpriteFilter::Nearest);
        assert_eq!(
            SpriteInstance::whole_texture(placement)
                .sampled(SpriteFilter::Linear)
                .filter,
            SpriteFilter::Linear
        );
    }

    // --- D13: the border a padded atlas owes each region -------------------

    /// A 10 x 10 texture holding one 8 x 8 region at (1, 1) with a correct
    /// one-texel border.
    ///
    /// Every content texel carries a different colour, `[x * 16, y * 16, 0,
    /// 255]` for its position inside the region, so a border texel copied from
    /// the wrong place differs from the right one. A flat fixture would satisfy
    /// every assertion below while being blind to the mistake they exist for —
    /// the same reason `offscreen.rs`'s quadrant texture is asymmetric.
    fn padded_block() -> Pixels {
        let mut rgba = vec![0_u8; 10 * 10 * 4];
        for y in 0..10_u32 {
            for x in 0..10_u32 {
                // The clamp *is* the duplication: 0 and 9 both land on the
                // content's own outermost texel.
                let content_x = x.clamp(1, 8) - 1;
                let content_y = y.clamp(1, 8) - 1;
                let at = ((y * 10 + x) * 4) as usize;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "content_x and content_y are 0..8, so the product is 0..112"
                )]
                let colour = [(content_x * 16) as u8, (content_y * 16) as u8, 0, 255];
                rgba[at..at + 4].copy_from_slice(&colour);
            }
        }
        Pixels::from_rgba8(10, 10, rgba).expect("the buffer matches its dimensions")
    }

    /// The same texture with one border texel replaced.
    fn with_texel(x: u32, y: u32, colour: [u8; 4]) -> Pixels {
        let source = padded_block();
        let mut rgba = source.rgba().to_vec();
        let at = ((y * 10 + x) * 4) as usize;
        rgba[at..at + 4].copy_from_slice(&colour);
        Pixels::from_rgba8(10, 10, rgba).expect("the buffer matches its dimensions")
    }

    #[test]
    fn a_correctly_padded_region_passes() {
        assert_eq!(
            check_region_padding(1, 1, 8, 8, REGION_PADDING_TEXELS, &padded_block()),
            Ok(())
        );
    }

    /// All four edges and all four corners, each named.
    ///
    /// One case per side rather than one for "a wrong texel somewhere": a loop
    /// that walks the frame but forgets the corners passes the four-edge test
    /// and is exactly the mistake [`RegionEdge`]'s eight cases exist for.
    #[test]
    fn a_wrong_texel_is_reported_with_its_edge_and_its_source() {
        let wrong = [9, 9, 9, 255];

        for (x, y, edge, source_x, source_y) in [
            (0_u32, 4_u32, RegionEdge::Left, 1_u32, 4_u32),
            (9, 4, RegionEdge::Right, 8, 4),
            (4, 0, RegionEdge::Top, 4, 1),
            (4, 9, RegionEdge::Bottom, 4, 8),
            (0, 0, RegionEdge::TopLeft, 1, 1),
            (9, 0, RegionEdge::TopRight, 8, 1),
            (0, 9, RegionEdge::BottomLeft, 1, 8),
            (9, 9, RegionEdge::BottomRight, 8, 8),
        ] {
            let texture = with_texel(x, y, wrong);
            let expected = padded_block()
                .pixel(source_x, source_y)
                .expect("the source texel is inside the texture");

            assert_eq!(
                check_region_padding(1, 1, 8, 8, REGION_PADDING_TEXELS, &texture),
                Err(PaddingDefect::WrongTexel {
                    edge,
                    x,
                    y,
                    source_x,
                    source_y,
                    found: wrong,
                    expected,
                }),
                "a wrong texel at ({x}, {y}) must be reported on the {edge}"
            );
        }
    }

    /// The message says which texel, which side, and what it should have held.
    #[test]
    fn the_message_names_the_texel_the_edge_and_both_colours() {
        let defect = check_region_padding(
            1,
            1,
            8,
            8,
            REGION_PADDING_TEXELS,
            &with_texel(4, 0, [9, 9, 9, 255]),
        )
        .expect_err("the top border texel was replaced");

        assert_eq!(
            defect.to_string(),
            "border texel (4, 0) on the region's top edge must copy content texel (4, 1): \
             expected [48, 0, 0, 255], found [9, 9, 9, 255]"
        );
    }

    /// A region against the texture's own rim cannot carry a border, and says
    /// so instead of checking the three sides that happen to fit.
    #[test]
    fn a_region_with_no_room_for_its_border_is_reported() {
        let texture = padded_block();

        assert_eq!(
            check_region_padding(0, 0, 10, 10, 1, &texture),
            Err(PaddingDefect::NoRoom {
                left: 0,
                top: 0,
                width: 10,
                height: 10,
                border: 1,
                texture_width: 10,
                texture_height: 10,
            }),
            "the whole texture has no room for a border"
        );

        // And on the far side: the content fits, the border does not.
        assert!(matches!(
            check_region_padding(1, 1, 9, 8, 1, &texture),
            Err(PaddingDefect::NoRoom { .. })
        ));

        assert_eq!(
            check_region_padding(0, 0, 10, 10, 1, &texture)
                .expect_err("no room")
                .to_string(),
            "the region at (0, 0), 10 x 10 texels, has no room for a border of 1: its padded \
             footprint would run from (-1, -1) to (11, 11) in a texture of 10 x 10"
        );
    }

    /// A region claiming no border is trivially right, and the check says so
    /// rather than reading texels that are not its business.
    #[test]
    fn a_border_of_zero_holds_for_any_region_that_fits() {
        assert_eq!(check_region_padding(1, 1, 8, 8, 0, &padded_block()), Ok(()));
        assert_eq!(
            check_region_padding(0, 0, 10, 10, 0, &padded_block()),
            Ok(())
        );
        // Still not a licence to describe a region that is not there.
        assert!(matches!(
            check_region_padding(0, 0, 11, 10, 0, &padded_block()),
            Err(PaddingDefect::NoRoom { .. })
        ));
    }

    /// A region with no texels says so, instead of claiming there is no room.
    ///
    /// The audit case: `(1, 1)` 0 x 8 in a 10 x 10 texture has a padded
    /// footprint of 0..2 x 0..10, which fits perfectly well. `NoRoom` would
    /// have printed a footprint and called it too large — a precise-sounding
    /// falsehood, which is worse than a vague one because it survives review.
    #[test]
    fn an_empty_region_is_its_own_defect_rather_than_a_claim_about_room() {
        let texture = padded_block();

        assert_eq!(
            check_region_padding(1, 1, 0, 8, 1, &texture),
            Err(PaddingDefect::EmptyRegion {
                width: 0,
                height: 8
            })
        );
        assert_eq!(
            check_region_padding(1, 1, 8, 0, 1, &texture),
            Err(PaddingDefect::EmptyRegion {
                width: 8,
                height: 0
            })
        );
        assert_eq!(
            check_region_padding(1, 1, 0, 8, 1, &texture)
                .expect_err("an empty region")
                .to_string(),
            "a region of 0 x 8 texels has no content for a border to copy"
        );
    }

    /// A 12 x 12 texture holding an 8 x 8 region at (2, 2) with a correct
    /// **two**-texel border.
    ///
    /// Both rings clamp onto the same content edge texel, which is what a
    /// border wider than one means and what no other test here exercises.
    fn twice_padded_block() -> Pixels {
        let mut rgba = vec![0_u8; 12 * 12 * 4];
        for y in 0..12_u32 {
            for x in 0..12_u32 {
                let content_x = x.clamp(2, 9) - 2;
                let content_y = y.clamp(2, 9) - 2;
                let at = ((y * 12 + x) * 4) as usize;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "content_x and content_y are 0..8, so the product is 0..112"
                )]
                let colour = [(content_x * 16) as u8, (content_y * 16) as u8, 0, 255];
                rgba[at..at + 4].copy_from_slice(&colour);
            }
        }
        Pixels::from_rgba8(12, 12, rgba).expect("the buffer matches its dimensions")
    }

    /// A border wider than one texel replicates every ring onto the same edge.
    ///
    /// Without this the comparison loop never ran at a border other than 0 or
    /// 1: `border = 2` on the 10 x 10 fixture fails in `checked_sub` before a
    /// texel is read, so "the check takes the border as a parameter" rested on
    /// bounds arithmetic alone.
    #[test]
    fn a_two_texel_border_replicates_both_rings_onto_the_same_content_edge() {
        assert_eq!(
            check_region_padding(2, 2, 8, 8, 2, &twice_padded_block()),
            Ok(())
        );

        // The outer ring is reported against the same content texel as the
        // inner one: column 0 and column 1 both copy column 2.
        let mut rgba = twice_padded_block().rgba().to_vec();
        let at = ((5 * 12) * 4) as usize; // texel (0, 5), the outer left ring
        rgba[at..at + 4].copy_from_slice(&[9, 9, 9, 255]);
        let broken = Pixels::from_rgba8(12, 12, rgba).expect("the buffer matches its dimensions");

        assert_eq!(
            check_region_padding(2, 2, 8, 8, 2, &broken),
            Err(PaddingDefect::WrongTexel {
                edge: RegionEdge::Left,
                x: 0,
                y: 5,
                source_x: 2,
                source_y: 5,
                found: [9, 9, 9, 255],
                expected: [0, 48, 0, 255],
            })
        );
    }

    /// The constant is one, and a wider border is refused rather than ignored.
    ///
    /// This observes the *bounds* arithmetic only — `checked_sub` underflows
    /// before a texel is read. That the comparison loop honours a wider border
    /// is `a_two_texel_border_replicates_both_rings_onto_the_same_content_edge`
    /// above, on a fixture where two rings fit.
    #[test]
    fn the_border_is_one_texel_and_a_wider_one_is_refused_where_it_cannot_fit() {
        assert_eq!(REGION_PADDING_TEXELS, 1);

        // A two-texel border over a fixture that carries one: the outer ring
        // has no room in a 10 x 10 texture.
        assert!(matches!(
            check_region_padding(1, 1, 8, 8, 2, &padded_block()),
            Err(PaddingDefect::NoRoom { .. })
        ));
    }

    // --- The tint, and the invariant it has to keep (M6b.3) ----------------

    /// The five cases S2 asked for by hand, as `(texel, tint, expected)`.
    ///
    /// A texel is `(rgb, a)` **already premultiplied**, which is what the
    /// pipeline carries (ADR-0023) and what ADR-0024 writes into the atlas at
    /// load. The expectation is worked out from the straight-alpha derivation on
    /// [`SpriteTint`]: `out_rgb = C * t_rgb * t_a` and `out_a = A * t_a`.
    ///
    /// The third row is the one this whole test exists for. An opaque texel
    /// under a half-transparent tint must come out at `0.5` on **both** the
    /// colour and the alpha; a tint applied to the alpha alone would leave the
    /// colour at `1.0` against an alpha of `0.5`, which is `rgb > a` — a
    /// fragment brighter than its own coverage, and the exact defect
    /// premultiplied blending cannot survive.
    const HAND_WORKED: [(f32, f32, SpriteTint, f32, f32); 5] = [
        // Opaque texel, opaque tint: the identity.
        (1.0, 1.0, SpriteTint::UNTINTED, 1.0, 1.0),
        // Half-transparent texel, opaque tint: coverage untouched.
        (0.5, 0.5, SpriteTint::UNTINTED, 0.5, 0.5),
        // Opaque texel, half-transparent tint: both halve together.
        (
            1.0,
            1.0,
            SpriteTint {
                red: 1.0,
                green: 1.0,
                blue: 1.0,
                alpha: 0.5,
            },
            0.5,
            0.5,
        ),
        // Both half.
        (
            0.5,
            0.5,
            SpriteTint {
                red: 1.0,
                green: 1.0,
                blue: 1.0,
                alpha: 0.5,
            },
            0.25,
            0.25,
        ),
        // Alpha zero on both sides. ADR-0024's load arithmetic guarantees the
        // colour is zero wherever the alpha is, so this is the whole of the
        // "dirty transparent" case as the pipeline can meet it.
        (
            0.0,
            0.0,
            SpriteTint {
                red: 1.0,
                green: 1.0,
                blue: 1.0,
                alpha: 0.0,
            },
            0.0,
            0.0,
        ),
    ];

    /// The hand-worked cases come out where the derivation says they do.
    ///
    /// Runs without a GPU: this is the arithmetic the shader performs, done in
    /// the same `f32` on the same premultiplied representation. What it cannot
    /// say is that the shader performs it — `tests/tint.rs` renders the same
    /// cases and is where that is measured.
    #[test]
    fn the_hand_worked_tint_cases_come_out_where_the_derivation_says() {
        for (colour, alpha, tint, expected_colour, expected_alpha) in HAND_WORKED {
            let [tint_r, _, _, tint_a] = tint.premultiplied();

            assert!(
                (colour * tint_r - expected_colour).abs() < f32::EPSILON,
                "texel ({colour}, {alpha}) under {tint:?} should leave the colour at \
                 {expected_colour}, got {}",
                colour * tint_r
            );
            assert!(
                (alpha * tint_a - expected_alpha).abs() < f32::EPSILON,
                "texel ({colour}, {alpha}) under {tint:?} should leave the alpha at \
                 {expected_alpha}, got {}",
                alpha * tint_a
            );
        }
    }

    /// `rgb <= a` survives the tint, for every premultiplied texel and every
    /// tint inside the range the type names.
    ///
    /// **This is the only guard on the invariant**, and it is a sweep rather
    /// than five cases because the failure it looks for is a *missing factor*
    /// and a missing factor is invisible wherever that factor happens to be one.
    /// The five hand-worked cases above include three where `t_a == 1.0`; a
    /// shader that forgot `* t_a` on the colour would pass all three.
    ///
    /// Runs without a GPU. 17 alpha steps by 17 colour steps by 9 tint alphas by
    /// 9 tint colours, which is 23 409 combinations and about a millisecond.
    #[test]
    fn the_premultiplied_tint_keeps_rgb_at_or_below_alpha() {
        let steps = |count: u32| {
            (0..=count).map(move |index| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "the counts are small enough to be exact in f32"
                )]
                let value = index as f32 / count as f32;
                value
            })
        };

        let mut checked = 0_u32;
        for alpha in steps(16) {
            // A premultiplied texel's colour never exceeds its alpha; that is
            // the invariant the atlas arrives with, so the sweep respects it
            // rather than testing the renderer against inputs it cannot get.
            for colour in steps(16).map(|fraction| fraction * alpha) {
                for tint_alpha in steps(8) {
                    for tint_colour in steps(8) {
                        let tint = SpriteTint {
                            red: tint_colour,
                            green: tint_colour,
                            blue: tint_colour,
                            alpha: tint_alpha,
                        };
                        let [factor_rgb, _, _, factor_a] = tint.premultiplied();

                        let out_colour = colour * factor_rgb;
                        let out_alpha = alpha * factor_a;
                        assert!(
                            out_colour <= out_alpha,
                            "texel ({colour}, {alpha}) under {tint:?} produced \
                             rgb {out_colour} above alpha {out_alpha}, which is a \
                             fragment brighter than its own coverage"
                        );
                        checked += 1;
                    }
                }
            }
        }

        assert_eq!(
            checked, 23_409,
            "the sweep did not cover what it says it covers"
        );
    }

    /// A tint above one is a named limit, not a promise, and this measures it.
    ///
    /// Runs without a GPU. Two statements, and the second is why the first is
    /// worth having: [`SpriteTint::premultiplied`] does **not** clamp, and the
    /// invariant does **not** hold above one. Writing that down as a test is
    /// what stops the documented limit from decaying into "probably fine" —
    /// and it is what a future decision to clamp would have to turn red on
    /// purpose.
    #[test]
    fn a_tint_above_one_is_the_named_limit_and_not_a_promise() {
        let bright = SpriteTint::rgb(2.0, 2.0, 2.0);
        assert_eq!(
            bright.premultiplied(),
            [2.0, 2.0, 2.0, 1.0],
            "premultiplied() clamps, which the type says it does not"
        );

        // An opaque white texel, premultiplied, is (1.0, 1.0). Under this tint
        // the colour doubles and the alpha does not.
        let (colour, alpha) = (1.0_f32, 1.0_f32);
        let [factor_rgb, _, _, factor_a] = bright.premultiplied();
        assert!(
            colour * factor_rgb > alpha * factor_a,
            "a tint above one no longer breaks the premultiplied invariant, so \
             the limit SpriteTint records has stopped being true and the type's \
             documentation is now wrong"
        );
    }

    /// The identity tint writes exactly `1.0` into all four tint slots, on all
    /// four corners.
    ///
    /// Runs without a GPU. The bit-level half of V1: a multiplication by `1.0`
    /// is the IEEE-754 identity, so a vertex carrying these four floats draws
    /// what it drew before the tint existed — but only if these are the floats
    /// it carries. `assert_eq!` on `f32` is exact here on purpose; a `1.0` that
    /// arrived through arithmetic and came out at `0.999_999_9` would move a
    /// blessed reference, and this is the assertion that would say so first.
    #[test]
    fn the_untinted_batch_is_the_untinted_batch_bit_for_bit() {
        let sprite =
            SpriteInstance::new(SpritePlacement::new(8.0, 4.0), TextureRegion::WHOLE_TEXTURE);
        assert_eq!(sprite.tint, SpriteTint::UNTINTED);

        let vertices = sprite_vertices(sprite, Projection::for_target(64, 64));
        for (index, corner) in vertices.iter().enumerate() {
            assert_eq!(
                corner[4..8],
                [1.0, 1.0, 1.0, 1.0],
                "corner {index} of an untinted sprite carries {:?} rather than the \
                 identity",
                &corner[4..8]
            );
        }
    }

    /// A tint reaches all four corners with the same value, premultiplied.
    ///
    /// Runs without a GPU. The counterpart to the test above: it is not enough
    /// that the identity survives; a real tint has to arrive, and it has to
    /// arrive *premultiplied*, because the shader multiplies and does not
    /// premultiply. A straight tint in the vertex buffer would look right on
    /// every opaque case and break exactly at a half-transparent edge.
    #[test]
    fn a_tint_reaches_every_corner_already_premultiplied() {
        let tint = SpriteTint {
            red: 1.0,
            green: 0.5,
            blue: 0.0,
            alpha: 0.5,
        };
        let sprite =
            SpriteInstance::new(SpritePlacement::new(8.0, 4.0), TextureRegion::WHOLE_TEXTURE)
                .tinted(tint);

        let vertices = sprite_vertices(sprite, Projection::for_target(64, 64));
        for (index, corner) in vertices.iter().enumerate() {
            assert_eq!(
                corner[4..8],
                [0.5, 0.25, 0.0, 0.5],
                "corner {index} carries {:?}, which is not the premultiplied tint",
                &corner[4..8]
            );
        }
    }

    /// Two sprites with different tints share one batch, and therefore one draw
    /// call.
    ///
    /// Runs without a GPU. The structural claim [`SpriteInstance::tint`] makes:
    /// a tint is a vertex attribute, so unlike a filter it cuts no run. If a
    /// later change moved the tint into a uniform, `batch_runs` would have to
    /// cut on it and this would be the test that noticed.
    #[test]
    fn two_tints_in_one_batch_stay_one_run() {
        let placement = SpritePlacement::new(4.0, 4.0);
        let sprites = [
            SpriteInstance::new(placement, TextureRegion::WHOLE_TEXTURE)
                .tinted(SpriteTint::rgb(1.0, 0.0, 0.0)),
            SpriteInstance::new(placement, TextureRegion::WHOLE_TEXTURE)
                .tinted(SpriteTint::rgb(0.0, 0.0, 1.0)),
        ];

        assert_eq!(batch_runs(&sprites), vec![0..2]);
    }
}
