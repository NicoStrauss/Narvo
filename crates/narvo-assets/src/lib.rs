//! Asset identity, loading and hot reload for Narvo.
//!
//! Responsibility: handing out cheap, copyable handles that name an asset
//! independently of whether it is loaded yet, resolving those handles to
//! decoded data, tracking the source files behind them, and swapping the
//! contents of a handle when a file changes on disk so a running session picks
//! the change up without a restart.
//!
//! API boundary: this is the only crate that touches the asset filesystem or
//! virtual filesystem, and the only place where a path turns into an in-memory
//! asset. It produces decoded, backend-agnostic data — pixels, shader source,
//! samples, scene descriptions — and creates no GPU or audio resources at all;
//! `narvo-render2d` uploads what it is handed. It depends on `narvo-core`
//! for error types and timing and knows nothing about `wgpu` or the window.
//!
//! # What is here today: the asset contract (M4.4, ADR-0020)
//!
//! [`atlas`] is the first of it — the **contract** §6/M4 named, in the direction
//! that has a consumer: source regions given in code go in, and one packed
//! atlas comes out, padded to the rule the renderer's own guard checks and
//! anchored by a SHA-256 that moves whenever a pixel or a placement does.
//!
//! # The file source (M4.8, ADR-0024)
//!
//! [`source`] is the second producer, and it is exactly the step ADR-0020
//! predicted: *"a decoder becomes one more way to produce a `SourceRegion`, and
//! nothing downstream of the packer learns that a file existed."* PNG bytes
//! become RGBA8 by an expansion that is exact or refused, the result is
//! premultiplied in byte space, and a `SourceRegion` comes out. The packer, the
//! padding rule and the anchor are unchanged and unaware.
//!
//! # Clips (M6b.5)
//!
//! [`clip`] is the third thing here and the smallest: it reads region *names*
//! and answers which of them are the frames of one animation, in order. It is a
//! **declaration and not a player** — nothing in it advances a counter or holds
//! a frame, because those are a game's and this crate has no world.
//!
//! It closes a gap M6b.5 measured rather than supposed. Animating already needs
//! no engine change: a system that keeps a counter and rewrites `Sprite`'s
//! region *is* an animation, deterministic because the counter is world state.
//! The one thing a consumer could not obtain was **how many frames a clip has**
//! — a fact that lives on the disk and in the packed table — so it wrote the
//! number by hand, and a frame added beside the others was loaded into the atlas
//! and silently never drawn.
//!
//! Nothing here re-interprets an existing name: a region in no clip is a region
//! exactly as it was, and the packed table is untouched.
//!
//! Still deliberately not here, and named in ADR-0024 rather than forgotten: any
//! on-disk format for a packed atlas, a watcher for asset files, formats other
//! than PNG, and mip or compression handling.

pub mod atlas;
pub mod clip;
pub mod source;

pub use atlas::{
    Atlas, BYTES_PER_PIXEL, MAX_ATLAS_SIDE, PackError, Placement, REGION_PADDING_TEXELS,
    SourceRegion, pack,
};
pub use clip::{Clip, clips};
pub use source::{AssetError, decode, premultiply, region_from_file, regions_from_directory};
