//! GPU-backed 2D rendering for Narvo, built on `wgpu`.
//!
//! Responsibility: owning the graphics device, queue, surface and swapchain,
//! managing GPU-side resources such as textures, buffers and pipelines, and
//! turning one frame's worth of 2D draw data — sprites, primitives, text — into
//! batched GPU work.
//!
//! API boundary: this is the only crate allowed to link against `wgpu` or to
//! hold graphics API types in its public API; no `wgpu` type crosses back out.
//! Callers hand it plain data (transforms, colours, asset handles) and receive
//! opaque handles in return, so a caller never learns which backend is in use.
//! It depends on `narvo-core` for timing and error types and on
//! `narvo-assets` for the raw pixel and shader data it uploads, but it never
//! loads a file itself and never reaches into game or application state.
//!
//! Everything that touches a graphics API sits behind the `gpu` feature, which
//! is on by default. Turning it off leaves a crate that compiles with no wgpu
//! anywhere in its dependency tree — that is what makes the headless rule in
//! CLAUDE.md checkable with `cargo tree` rather than by reading the source.

/// One cascade stage: probes, their intervals, their directions, and the
/// radiance they integrate.
///
/// **M8.5a, and the first thing in this crate that makes a field of `f32` out of
/// something other than a picture.** Its consumers are M8.5b's hierarchy, which
/// merges stages, and M8.8's game light. Re-exported flat like every other module
/// here, so `CascadeStage`, `Emission` and `RadianceField` read as
/// `narvo_render2d::CascadeStage` — three names that say what they are without
/// their module in front of them.
#[cfg(feature = "gpu")]
mod cascade;
/// Compute passes over fields, and the chain that runs several of them.
///
/// The crate's first compute path (M8.2). Private like every other module here;
/// its consumers — M8.3's jump flooding, M8.4's ray march, M8.5's cascades and
/// M8.6's write-back — are all inside this crate, so nothing is exported until
/// something outside needs it.
///
/// **M8.3a landed the first production caller and this attribute did not go
/// away, which is a measurement rather than an oversight.** `FieldKernel` and
/// `run` are called from [`OffscreenTarget::distance_field`] now; what is still
/// dead in a non-test build is the *transport* kernel — `TRANSPORT_WGSL`,
/// `TRANSPORT_ENTRY` and the `PassParams` comparison — because a kernel is
/// per-shader, jump flooding compiles its own, and the transport kernel is
/// M8.2's oracle rather than anything a production path will ever run. A
/// module-level expectation is fulfilled by *any* dead item inside the module,
/// so those keep it alive on their own.
///
/// **M8.2's stated reason was therefore not merely early, it was unreachable**,
/// and it is corrected below rather than left to be re-read as a promise. The
/// `expect`-not-`allow` mechanism did exactly its job: it is why the day M8.3
/// landed, the compiler had an opinion about which of these five attributes were
/// still true. Two of the five went; this is one of the three that stayed.
#[cfg(feature = "gpu")]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the transport kernel is M8.2's oracle: no production path compiles it, only tests do"
    )
)]
mod compute;
#[cfg(feature = "gpu")]
mod error;
/// The buffer a compute pass reads and the one it writes.
///
/// **Carried an `expect(dead_code)` from M8.2 until M8.3a, and the compiler took
/// it away.** [`OffscreenTarget::distance_field`] seeds a `FieldPair`, runs a
/// chain over it and reads it back, so `Field` and `FieldPair` are reached from
/// a public item and the module is no longer dead in a non-test build. It is
/// gone rather than relaxed, which is the whole reason M8.2 wrote `expect`
/// instead of `allow`.
#[cfg(feature = "gpu")]
mod field;
/// The M3.34 glyph atlas, and the single-line layout that consumes it.
///
/// **Public modules, where every other module in this crate is private and
/// re-exported flat at the root.** The deviation is deliberate and is the
/// smaller of two costs: flattening these would put `rasterize`, `FONT`,
/// `layout_line` and `sprites_for` into `narvo_render2d`'s root namespace,
/// where `FONT` and `rasterize` say nothing about what they are. Twelve render
/// types read well flat; two cohesive sub-namespaces do not.
///
/// Both arrived in M6.6b from `narvo-testkit` (ADR-0038).
#[cfg(feature = "gpu")]
pub mod glyph_atlas;
#[cfg(feature = "gpu")]
mod golden;
#[cfg(feature = "gpu")]
mod gpu;
/// A cascade of levels, and the two ways to merge one.
///
/// **M8.5b, and the reason M8.5a's stage exists.** Public because the choice
/// between the two merges is a plan decision rather than an engine one: both are
/// offered, both are costed by [`Cascade::budget`], and neither is preferred.
#[cfg(feature = "gpu")]
mod hierarchy;
/// The circle march: how far a ray gets before it meets an occluder.
///
/// M8.4's capability, and the first thing in this crate that reads the field
/// **without bringing it to the CPU**. Its consumers are M8.5's cascade, which
/// marches a probe's rays, and M8.8's game light.
#[cfg(feature = "gpu")]
mod march;
#[cfg(feature = "gpu")]
mod offscreen;
#[cfg(feature = "gpu")]
mod quad;
/// The distance field, computed by jump flooding over a [`field`] pair.
///
/// **The first consumer of the M8.2 machinery, and the first thing here that is
/// exported.** `compute`'s header says nothing leaves the crate until something
/// outside needs it; M8.3b turns occluders into seeds and M8.4 marches a ray
/// against the distances, and both are outside. Re-exported flat like every
/// other module rather than made `pub`, so `Seeds` and `SeedMap` read as
/// `narvo_render2d::Seeds` — two names that say what they are without their
/// module in front of them.
#[cfg(feature = "gpu")]
mod sdf;
#[cfg(feature = "gpu")]
mod sprite;
/// IEC 61966-2-1's transfer function, both directions.
///
/// **Public, and the only module here that is not behind `gpu`.** It is `f64`
/// arithmetic over a published constant and touches no graphics API, so gating
/// it would be a gate with nothing behind it — and a headless consumer
/// predicting a stored byte has the same right to it as a windowed one. Its own
/// header carries the rest, including why it left `narvo-testkit` in M7.0.
pub mod srgb;
/// Radiance written back as emission, and the colour that follows from it.
///
/// **M8.6's capability, and the reason M8.5b's cascade exists.** Its consumers
/// are M8.7's temporal accumulation, which needs a cache it can carry across a
/// frame boundary, and M8.8's game light. Re-exported flat like every other
/// module here, so `Albedo` and `SurfaceCache` read as
/// `narvo_render2d::SurfaceCache` — two names that say what they are without
/// their module in front of them.
#[cfg(feature = "gpu")]
mod surface;
#[cfg(feature = "gpu")]
pub mod text;
#[cfg(feature = "gpu")]
mod window;

#[cfg(feature = "gpu")]
pub use crate::cascade::{CascadeStage, Emission, RadianceField, StageLayout};
#[cfg(feature = "gpu")]
pub use crate::error::RenderError;
#[cfg(feature = "gpu")]
pub use crate::golden::{
    Golden, GoldenError, GoldenReport, MismatchDetails, Tolerance, WorstPixel, golden_artifact_dir,
};
#[cfg(feature = "gpu")]
pub use crate::hierarchy::{
    Cascade, CascadeBudget, CascadeLayout, ENTRY_BYTES, LevelBudget, MAX_STORAGE_BINDING_BYTES,
    MergeForm,
};
#[cfg(feature = "gpu")]
pub use crate::march::{MarchHit, MarchVerdict, Ray};
#[cfg(feature = "gpu")]
pub use crate::offscreen::{ClearColor, OffscreenTarget, Pixels};
pub use crate::sdf::{SeedMap, Seeds};
#[cfg(feature = "gpu")]
pub use crate::sprite::{
    BatchOf, CameraView, MAX_SPRITES_PER_BATCH, PaddingDefect, Projection, REGION_PADDING_TEXELS,
    RegionEdge, ScreenAnchor, SpriteBatch, SpriteFilter, SpriteInstance, SpritePlacement,
    SpriteTint, TextureRegion, batch_plan, batch_runs, check_region_padding,
};
#[cfg(feature = "gpu")]
pub use crate::surface::{Albedo, SurfaceCache};
#[cfg(feature = "gpu")]
pub use crate::window::{FrameOutcome, FrameStart, PresentPolicy, SurfaceFrame, WindowTarget};

/// Samples per pixel in every render pass this crate records.
///
/// Four, and not a queried maximum. `TextureFormat::guaranteed_format_features`
/// hands `MULTISAMPLE_X4 | MULTISAMPLE_RESOLVE` to `Rgba8UnormSrgb`, which the
/// offscreen target pins (`offscreen.rs`'s `TARGET_FORMAT`), and to
/// `Bgra8UnormSrgb`, which is what a surface typically offers first
/// (`wgpu-types-30.0.0/src/texture/format.rs:976` and `:981`). x2, x8 and x16
/// are separate flags in the same bitset and are guaranteed for neither, so any
/// other count would mean a capability query, a fallback path, and golden images
/// that depend on which machine rendered them.
///
/// **The window path is not pinned to a format**: `choose_format` returns the
/// first sRGB format the surface offers and otherwise whatever is first in the
/// list, so this covers the common case rather than every surface. Nothing here
/// queries, which is the trade being made and not an oversight.
///
/// There is no way to turn this off. A toggle would be two render paths where
/// the golden images pin one and nothing checks the other — which is what
/// `QuadPipeline::encode_pass`'s doc means by keeping the draw in one place so
/// the window cannot become a second renderer that drifts. **That reference said
/// `encode_pass_with` until M3.36 and pointed at the wrong item**: the argument
/// is on `encode_pass`, the shared entry point ("Keeping the draw here is what
/// stops the window from becoming a second renderer that drifts away from the
/// one the tests cover"); `encode_pass_with` is the arbitrary-buffer form
/// underneath it and its doc is about batching one `draw_indexed`. D14
/// (`ProjektPlan.md` §11) decided smooth camera movement through coverage
/// antialiasing; a build that ships it off does not have that.
#[cfg(feature = "gpu")]
pub const SAMPLE_COUNT: u32 = 4;

#[cfg(test)]
mod tests {
    /// Smoke test: the crate builds and its test harness actually runs.
    #[test]
    fn crate_is_wired_into_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "narvo-render2d");
    }
}
