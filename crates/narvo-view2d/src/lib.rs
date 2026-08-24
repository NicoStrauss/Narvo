//! Turning a world into what a renderer draws, and answering where a click landed.
//!
//! Responsibility: the seam between `narvo-ecs` and `narvo-render2d`. It reads
//! components and produces the renderer's own scalars; it reads components and
//! answers which entity a world point belongs to. Nothing here mutates a world,
//! and nothing here names a `wgpu` type.
//!
//! API boundary: this is the crate that sees both sides, which is the position
//! ADR-0015 gives the extraction and ADR-0041 gives this crate. `narvo-render2d`
//! still does not depend on `narvo-ecs`; the two meet at [`SpritePlacement`],
//! which is the renderer's and is scalars.
//!
//! # Why the draw order and the hit test are one crate
//!
//! Because they are one decision seen from two sides. [`placements_of`] sorts by
//! `(depth_order(depth), id)` and draws in that order, so the last one is on top;
//! [`hit_test`] returns the greatest pair among those hit, which is that same
//! last one. [`depth_order`] is the shared half, and it is shared rather than
//! copied — a hit test that normalised `-0.0` differently from the draw order
//! would put a click on the sprite that is *not* in front, and only for depths of
//! `-0.0`.
//!
//! Splitting the two into separate crates was considered and rejected for
//! exactly that: it would put one copy of `depth_order` on each side of a crate
//! boundary, or make one crate depend on the other to borrow it. ADR-0041 records
//! the price that decision pays — a consumer wanting only [`hit_test`] carries
//! the graphics stack — and the condition under which it is taken again.
//!
//! # The atlas path arrived in M6b.2a
//!
//! [`load_for`] turns a directory of image files into a texture and a table of
//! named regions, and refuses a world that asks for a region no file carries. It
//! used to be `narvo-app`'s `assets.rs`, the other place that saw a world and
//! the renderer at once; M6b.2's survey measured what that cost an external
//! consumer, and it was not the mechanism — every part of it is public and a
//! consumer rebuilt the body from them. It was [`AssetsError`], whose sentence
//! names the region a scene asked for and the ones that exist, and which a
//! binary crate cannot hand out. ADR-0041's amendment records the move.
//!
//! **The convention did not come with it.** `assets/` beside the scene file is
//! policy, and it stayed in `narvo-app` as `ASSETS_DIRECTORY` and
//! `directory_for`; [`load_for`] takes whatever directory it is given.
//!
//! # What is deliberately *not* here
//!
//! - **The tally system.** Counting input events is a system of the demo, not a
//!   piece of this seam: it reads `Events<InputEvent>`, which would put
//!   `narvo-input` under every consumer of the renderer. A game writes its own.
//! - **The window.** Nothing here sees a winit type or a surface. A hit test
//!   takes a world and a world point; turning a cursor into that point is
//!   `Projection::screen_to_world`'s job, one layer out.

mod atlas;
mod hit;
mod occluder;
mod sprite_batch;

pub use crate::atlas::{AssetsError, SceneAtlas, load_for};
pub use crate::hit::{depth_order, hit_test};
pub use crate::occluder::seeds_of;
pub use crate::sprite_batch::{
    Drawn, DrawnRegion, camera_of, placements_of, region_names_of, regions_of,
};

#[cfg(test)]
mod tests {
    /// Smoke test: the crate builds and its test harness actually runs.
    #[test]
    fn crate_is_wired_into_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "narvo-view2d");
    }
}
