//! Driving the frame loop: the real clock, and the work each phase does.
//!
//! [`narvo_core::frame`] owns the *order* of a frame and what each part cost.
//! This module owns what the parts actually are — ticking a world, reading it
//! into a buffer of scalars, and handing that to a renderer — because this is
//! the only crate that sees all three.
//!
//! The split is the boundary `narvo-core`'s own documentation draws: that crate
//! knows "nothing about windows, graphics APIs, audio devices or the
//! filesystem", and "anything that needs an operating-system resource ... belongs
//! in a sibling crate". A monotonic clock is an operating-system resource, so
//! [`Monotonic`] is here and the [`Clock`] trait it satisfies is there.

use std::time::{Duration, Instant};

use narvo_audio::Cue;
use narvo_core::frame::{Acquisition, Clock, FrameHost};
use narvo_ecs::{ComponentRegistry, EcsError, SystemContext, World};
#[cfg(test)]
use narvo_render2d::OffscreenTarget;
use narvo_render2d::glyph_atlas::{self, GlyphAtlas};
use narvo_render2d::text;
use narvo_render2d::{
    CameraView, FrameStart, Pixels, RenderError, SpriteBatch, SpriteInstance, SurfaceFrame,
    TextureRegion, WindowTarget,
};

/// The glyph size the overlay draws at.
///
/// 32 px, one of the two D10 anchors, and the one the click-counter scene
/// already draws through. A third size would need a new SHA anchor and a human's
/// blessing, so this is a choice between two rather than a free parameter.
const INSPECTOR_SIZE_PX: f32 = 32.0;

/// Distance from the canvas edge to the first glyph, in pixels.
const INSPECTOR_MARGIN_PX: f32 = 4.0;

/// Baseline-to-baseline distance, as a multiple of the glyph size.
///
/// 1.25 is the ordinary typographic leading for monospaced text and is a
/// presentation choice rather than a measurement — nothing in the atlas dictates
/// it.
const INSPECTOR_LINE_SPACING: f32 = 1.25;

use narvo_view2d::{camera_of, placements_of, regions_of};

use crate::audio;
use crate::sim::Simulation;

/// The real clock: [`Instant`], counted from when the loop started.
///
/// The only implementation of [`Clock`] that reads the operating system. Every
/// test in this workspace uses a staged one instead, which is why the loop's
/// arithmetic is asserted as equalities rather than sampled.
#[derive(Debug)]
pub struct Monotonic {
    origin: Instant,
}

impl Default for Monotonic {
    fn default() -> Self {
        Self::new()
    }
}

impl Monotonic {
    /// A clock whose origin is now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for Monotonic {
    fn now(&mut self) -> Duration {
        // `Instant` is documented as monotonically non-decreasing, which is
        // exactly the contract `Clock` asks for, so no clamping is needed here -
        // the loop's own saturation covers a platform that breaks the promise.
        self.origin.elapsed()
    }
}

/// Which part of the atlas a sprite shows.
///
/// # The transitional rule, and its retirement
///
/// M4.6 needed a picture before a scene could say what an entity looks like, so
/// it derived one: a sprite's position in draw order picked one of four
/// quadrants of the demo texture. Its own documentation called that a
/// transitional answer and named what would replace it — "referencing [the M4.4
/// contract] from a scene is **the next step**".
///
/// M4.8 is that step, and the rule is gone rather than kept beside its
/// successor. [`ByName`](Self::ByName) resolves the `region` of the eighth
/// registered component against an atlas packed from the scene's own `assets/`
/// directory, so what a thing looks like is content rather than a position in a
/// list. ADR-0024 records the retirement, and its one visible consequence:
/// **an entity with no `Sprite` no longer draws in this mode**.
#[derive(Debug, Clone, PartialEq)]
pub enum Regions {
    /// Every sprite shows the whole texture. What the window has always done,
    /// and what every blessed reference is drawn through.
    WholeTexture,
    /// Each entity shows the region its `Sprite` names.
    ByName(std::collections::BTreeMap<String, TextureRegion>),
}

/// Everything a frame needs that is not the loop.
///
/// Holds the simulation and whatever the frame draws into. The render target is
/// a type parameter so the same host — the same tick, the same extraction, the
/// same sprite building — drives a window and an offscreen target alike. That is
/// what lets an offscreen test exercise the real frame path: every phase above
/// the target is shared, and all three target phases differ. `acquire` triages a
/// swapchain on one side and is a constant `Ready` on the other; the windowed
/// draw submits and returns without waiting, where the offscreen one ends in a
/// blocking read-back - which is why a frame timed through it is not a frame
/// time. The windowed half of that difference is the part CLAUDE.md records as
/// deliberately uncovered; the offscreen half is what the smoke test runs.
///
/// # Why the simulation arrives as one value
///
/// The world, the registry that names its components and the scheduler are held
/// as the single [`Simulation`] the scene was built into, not as three fields
/// this struct assembles for itself. That is M6.4a's rule applied one runner
/// further along: `headless.rs` already carries a whole `Simulation` across its
/// agent seam "because a world and the registry that names its components have
/// to move together or the next `canonical_dump` fails, which is exactly what
/// `sim::Simulation` exists to prevent a caller from getting wrong"
/// (`headless.rs:437-442`).
///
/// The property that buys is narrow and worth stating exactly: **a caller
/// cannot hand this host a world from one place and a registry from another**,
/// because there is one parameter and no way to spell the mistake. A second
/// registry that merely looked right — built fresh here, or cloned once and
/// never followed through a reload — is the failure mode, and it is not
/// reachable through this signature.
#[derive(Debug)]
pub struct SceneHost<T: FrameTarget> {
    /// The world, the names its state is written under, and its systems.
    ///
    /// Held since M6.6a, which added only the middle one, for the window's own
    /// readers. **Both of the two M6.6a named have now happened and neither is
    /// [`Self::registry`]'s caller**: `extract_inspector` reads
    /// `self.simulation.registry` directly, because it is inside this type and
    /// needs no accessor, and M6.4b's screenshot turned out to need no names at
    /// all — it copies a texture. The field is read; the accessor beside it
    /// still is not, and says so.
    simulation: Simulation,
    /// The atlas every sprite samples.
    atlas: Pixels,
    /// Reused across frames so the extraction does not allocate a fresh vector
    /// every time. Cleared and refilled, never reallocated once it has grown.
    sprites: Vec<SpriteInstance>,
    /// The view the last extraction produced.
    camera: CameraView,
    /// The tick number of the next tick to run.
    tick: u64,
    /// Which part of the atlas each sprite shows.
    regions: Regions,
    /// What the ticks since the last drain asked to be heard.
    ///
    /// Filled once per tick — including every catch-up tick — and emptied once
    /// per frame by the runner. The two rates are the point: the *list* is
    /// decided by the simulation, and only its delivery to a device happens at
    /// frame rate.
    cues: Vec<Cue>,
    /// What [`audio::cues_of`] remembers between ticks.
    ///
    /// Here rather than in the world, because a counter that moved is world
    /// state but "the observer has already reacted to it" is not — ADR-0028
    /// forbids anything in the hash domain moving because a sound played.
    cue_memory: audio::CueMemory,
    /// The debug overlay: off, and holding nothing, until somebody asks for it.
    ///
    /// **Built lazily on purpose.** `glyph_atlas::rasterize` is not free, and
    /// every `SceneHost` in this workspace would pay for it at construction —
    /// including the seven in this file's tests and the one the drop-scene
    /// reference is drawn through. `SceneHost::new`'s signature is untouched
    /// (M6.6a paid ten sites for it), so the atlas cannot arrive as an argument
    /// either; it is rasterised the first time the overlay is switched on and
    /// kept from then on.
    inspector: Inspector,
    /// A screenshot that was asked for and the frame it was read out of.
    ///
    /// Two fields in one, because they are one thing at two moments: the
    /// request goes in from a key press, and the picture comes out at the end of
    /// the frame that served it. [`Capture::Wanted`] survives a frame with
    /// nowhere to draw rather than being dropped, so a press while the window is
    /// occluded is answered by the next frame that has an image.
    ///
    /// **The host never writes a file.** It reads the frame back and hands the
    /// bytes over; where they land is the runner's decision, and keeping it
    /// there is what stops a render path from acquiring a filesystem.
    capture: Capture,
    target: T,
}

/// Where a screenshot is between being asked for and being taken.
#[derive(Debug, Default)]
enum Capture {
    /// Nobody has asked.
    #[default]
    Idle,
    /// Asked for, not yet served.
    Wanted,
    /// Read back and waiting to be taken by the runner.
    Taken(Pixels),
}

/// The debug overlay's own state.
///
/// Separate from [`SceneHost`]'s other fields because it is the only group that
/// is *off* by default and can therefore be absent entirely.
#[derive(Debug, Default)]
struct Inspector {
    /// Whether the overlay draws.
    ///
    /// **Off by default**, and since ADR-0039 that is a cost decision rather
    /// than a correctness one: an empty second batch produces nothing, so the
    /// blessed references are safe either way. What an always-on overlay costs
    /// is every user who did not ask for it.
    on: bool,
    /// The glyph atlas, rasterised on first use and kept.
    ///
    /// `None` until the overlay is first switched on. D10 fixes the sizes;
    /// 32 px is the larger of the two and the one the click-counter scene
    /// already draws through.
    atlas: Option<GlyphAtlas>,
    /// Which entity is shown, taken modulo the entity count by `inspector`.
    ///
    /// A plain counter rather than an `EntityId`, so it survives a reload: a
    /// reconstituted world has different handles, and an index simply lands on
    /// whatever is there now.
    selected: usize,
    /// The glyphs of the last extraction, in drawing order.
    ///
    /// Cleared and refilled like [`SceneHost::sprites`], and **empty whenever
    /// the overlay is off** — which is what makes the second batch a no-op
    /// rather than a cheap one (ADR-0039).
    sprites: Vec<SpriteInstance>,
}

impl Inspector {
    /// The second batch for this frame, or `None` when there is nothing to draw.
    ///
    /// **`None` and an empty batch are the same thing to the renderer**
    /// (ADR-0039), and this returns `None` for both so the distinction never has
    /// to be made twice.
    ///
    /// # The camera is the identity, and that is the whole of M6b.4 here
    ///
    /// The overlay is laid out in **pixels of the target** — `extract_inspector`
    /// asks `FrameTarget::extent` and hands it to `text::sprites_for`, which is
    /// why M6.6d had to add that method at all. Screen-fixed is therefore the
    /// space the overlay was already being *authored* in; until M6b.4 it was
    /// then drawn through the scene's camera, so a world that pans or zooms
    /// carried the debug text off with it. `CameraView::IDENTITY` under the
    /// target's own projection is that authoring space exactly: origin at the
    /// centre, one unit per target pixel, y up.
    ///
    /// Nothing else in the frame path changed. The scene batch still draws
    /// through `self.camera`, which `extract` still takes from `camera_of`, so
    /// ADR-0017's single composition point is untouched — this reads a camera
    /// and writes none.
    fn batch(&self) -> Option<SpriteBatch<'_>> {
        if self.sprites.is_empty() {
            return None;
        }
        let atlas = self.atlas.as_ref()?;
        Some(SpriteBatch {
            image: atlas.pixels(),
            sprites: &self.sprites,
            camera: CameraView::IDENTITY,
        })
    }
}

/// What a frame can be drawn into.
///
/// Two implementations: a window, which acquires a swapchain image and presents
/// it, and an offscreen target, which always has somewhere to draw and shows
/// nobody. The trait exists so [`SceneHost`] has exactly one implementation of
/// the interesting phases.
pub trait FrameTarget {
    /// Obtains somewhere to draw, if there is one.
    ///
    /// # Errors
    ///
    /// Whatever the target's surface can raise.
    fn acquire(&mut self) -> Result<Acquisition, RenderError>;

    /// How large the drawing surface is, in pixels.
    ///
    /// Added in M6.6d, and the reason is a measured one rather than tidiness.
    /// The overlay lays its text out in **pixels** and `text::sprites_for`
    /// converts those to world units against the extent it is given — so an
    /// extent that is not the target's puts the text somewhere else. A first
    /// version of this used a constant 1280 by 720, the size the window asks
    /// for, and `the_overlay_reaches_the_frame_and_comes_from_the_glyph_atlas`
    /// failed on a 256 by 256 offscreen target with "switching the overlay on
    /// changed nothing": the text was not misplaced, it was **entirely outside
    /// the frame**.
    ///
    /// Both real targets already answered this question —
    /// `WindowTarget::size` and `OffscreenTarget::width`/`height` — so nothing
    /// in `narvo-render2d` had to change to ask it.
    fn extent(&self) -> (u32, u32);

    /// Draws `sprites` through `camera` and submits the work.
    ///
    /// `overlay` is a **second texture and its sprites**, drawn after `sprites`
    /// and therefore over them. Since M6.6c, because a draw call binds one
    /// texture and a frame that wants glyphs over a scene needs two.
    ///
    /// **`None`, and equally a batch with no sprites in it, must produce
    /// nothing**: no second bind group, no extra run, the same command sequence
    /// the target emitted before this parameter existed. Both implementations
    /// below get that from `narvo_render2d::batch_plan`, which is where the
    /// property is tested without a device.
    ///
    /// # Errors
    ///
    /// Whatever the target's draw path can raise.
    fn draw(
        &mut self,
        atlas: &Pixels,
        sprites: &[SpriteInstance],
        overlay: Option<SpriteBatch<'_>>,
        camera: CameraView,
    ) -> Result<(), RenderError>;

    /// Reads the frame that is about to be handed over back to the CPU.
    ///
    /// Called between [`Self::draw`] and [`Self::present`] and only when
    /// somebody asked for a screenshot, because it waits for the GPU.
    ///
    /// `None` means there was nothing to read. The loop only reaches
    /// [`Self::present`] after an acquisition answered `Ready` and the encode
    /// succeeded (`narvo-core/src/frame.rs:357-369`), so for the window that is
    /// unreachable; returning it rather than unwrapping keeps a caller that got
    /// the order wrong from panicking in a render loop, which is the shape
    /// [`Windowed::draw`] already uses.
    ///
    /// # Errors
    ///
    /// Whatever the target's read-back can raise — for a window, a surface that
    /// may not be copied out of, a format that is not four RGBA8 channels, or a
    /// failed map.
    fn capture(&self) -> Result<Option<Pixels>, RenderError>;

    /// Hands the drawn frame over.
    ///
    /// # Errors
    ///
    /// Whatever presenting can raise.
    fn present(&mut self) -> Result<(), RenderError>;
}

/// A [`FrameTarget`] that presents to a window.
///
/// The thin rim. Everything above it — the tick, the extraction, the sprite
/// building — is shared with [`Offscreen`], so what an offscreen test exercises
/// is the same frame path a window runs, minus the surface. These few lines are
/// the part that cannot be covered without a display, which CLAUDE.md already
/// records as deliberate for the present path.
#[derive(Debug)]
pub struct Windowed {
    target: WindowTarget,
    /// The image acquired this frame, held between `acquire` and `present`.
    ///
    /// `None` between frames and whenever the swapchain had nothing to give.
    /// Holding it here rather than passing it through the loop is what keeps
    /// `narvo-core` free of any knowledge that a swapchain exists.
    frame: Option<SurfaceFrame>,
}

impl Windowed {
    /// A target drawing into `target`.
    #[must_use]
    pub fn new(target: WindowTarget) -> Self {
        Self {
            target,
            frame: None,
        }
    }

    /// The present mode the surface actually took.
    #[must_use]
    pub fn present_mode(&self) -> &'static str {
        self.target.present_mode()
    }

    /// The adapter in use, for a measurement that has to name it.
    #[must_use]
    pub fn adapter_summary(&self) -> &str {
        self.target.adapter_summary()
    }

    /// Blocks until the GPU has finished everything submitted so far.
    ///
    /// Not part of an ordinary frame. See [`WindowTarget::drain`] for what
    /// calling it does to a measurement.
    ///
    /// # Errors
    ///
    /// If the device could not be polled.
    pub fn drain(&self) -> Result<(), RenderError> {
        self.target.drain()
    }

    /// Reconfigures the surface after the window changed size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.target.resize(width, height);
    }

    /// The surface's size in physical pixels.
    ///
    /// The click path's other half: winit reports a cursor in physical pixels,
    /// and a projection is built from the target's physical extent, so both
    /// sides of the conversion are in the same unit and no scale factor has to
    /// be guessed at.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.target.size()
    }
}

impl FrameTarget for Windowed {
    fn extent(&self) -> (u32, u32) {
        self.target.size()
    }

    fn acquire(&mut self) -> Result<Acquisition, RenderError> {
        match self.target.begin_frame()? {
            FrameStart::Ready(frame) => {
                self.frame = Some(frame);
                Ok(Acquisition::Ready)
            }
            FrameStart::Skipped | FrameStart::Reconfigured => {
                self.frame = None;
                Ok(Acquisition::Unavailable)
            }
        }
    }

    fn draw(
        &mut self,
        atlas: &Pixels,
        sprites: &[SpriteInstance],
        overlay: Option<SpriteBatch<'_>>,
        camera: CameraView,
    ) -> Result<(), RenderError> {
        let Some(frame) = self.frame.as_ref() else {
            // The loop only calls this after `acquire` answered `Ready`, so
            // there is always a frame here. Returning rather than unwrapping
            // keeps a future caller that got the order wrong from panicking in
            // a render loop.
            return Ok(());
        };

        self.target
            .draw_sprites(frame, atlas, sprites, overlay, camera)
    }

    fn capture(&self) -> Result<Option<Pixels>, RenderError> {
        self.frame
            .as_ref()
            .map(|frame| self.target.read_back(frame))
            .transpose()
    }

    fn present(&mut self) -> Result<(), RenderError> {
        if let Some(frame) = self.frame.take() {
            self.target.present(frame);
        }
        Ok(())
    }
}

/// A [`FrameTarget`] that draws into a texture and shows nobody.
///
/// Test-only. It exists so the offscreen smoke can drive the *production*
/// [`FrameLoop`](narvo_core::frame::FrameLoop) and the *production*
/// [`SceneHost`] with only the target swapped; nothing a user runs draws into
/// it, so gating it keeps it out of the shipped binary rather than leaving a
/// second render path there for nobody.
///
/// What the offscreen smoke runs the real loop into. It always has somewhere to
/// draw, so [`Acquisition::Unavailable`] never happens and the frame path is
/// exercised end to end without a window - though not without an adapter:
/// `OffscreenTarget::new` selects one, and the smoke skips wherever none is
/// usable.
///
/// **It reads the drawn frame back to the CPU**, which a windowed frame never
/// does, so a frame time measured through this target is not a frame time -
/// `docs/perf/BASELINE.md` has said so since M3.7. It is here to check that the
/// loop runs and that the picture is not black, not to be timed.
#[cfg(test)]
#[derive(Debug)]
pub struct Offscreen {
    target: OffscreenTarget,
    /// The last frame drawn, so a smoke test can look at it.
    last: Option<Pixels>,
}

#[cfg(test)]
impl Offscreen {
    /// A target drawing into `target`.
    #[must_use]
    pub fn new(target: OffscreenTarget) -> Self {
        Self { target, last: None }
    }

    /// The most recently drawn frame, if any frame has been drawn.
    #[must_use]
    pub fn last_frame(&self) -> Option<&Pixels> {
        self.last.as_ref()
    }
}

#[cfg(test)]
impl FrameTarget for Offscreen {
    fn extent(&self) -> (u32, u32) {
        (self.target.width(), self.target.height())
    }

    fn acquire(&mut self) -> Result<Acquisition, RenderError> {
        Ok(Acquisition::Ready)
    }

    fn draw(
        &mut self,
        atlas: &Pixels,
        sprites: &[SpriteInstance],
        overlay: Option<SpriteBatch<'_>>,
        camera: CameraView,
    ) -> Result<(), RenderError> {
        // Two doors into one implementation, and the choice is made here rather
        // than in the renderer: `render_sprites_viewed_by` keeps the signature
        // its thirteen callers already use, and `render_sprites_over` is the
        // form that carries a second batch. Both end in `render_batches`.
        self.last = Some(match overlay {
            None => self
                .target
                .render_sprites_viewed_by(atlas, sprites, camera)?,
            Some(overlay) => self
                .target
                .render_sprites_over(atlas, sprites, overlay, camera)?,
        });
        Ok(())
    }

    fn capture(&self) -> Result<Option<Pixels>, RenderError> {
        // The draw already read it back, so there is nothing left to copy. That
        // is what makes this target the one the capture *wiring* is tested
        // through: everything above `FrameTarget` - the request, the moment it
        // is served, what is in the picture - runs unchanged, and only the copy
        // itself is missing, which is the half `narvo-render2d`'s own tests own.
        Ok(self.last.clone())
    }

    fn present(&mut self) -> Result<(), RenderError> {
        // Nothing to present to. The draw above already blocked on the read
        // back, so the frame is complete by the time this runs.
        Ok(())
    }
}

/// Anything a frame can fail with.
#[derive(Debug)]
pub enum FrameError {
    /// The simulation could not be advanced or read.
    Simulation(EcsError),
    /// The renderer could not draw or present.
    Render(RenderError),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Simulation(error) => write!(f, "the simulation failed: {error}"),
            Self::Render(error) => write!(f, "the renderer failed: {error}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<EcsError> for FrameError {
    fn from(error: EcsError) -> Self {
        Self::Simulation(error)
    }
}

impl From<RenderError> for FrameError {
    fn from(error: RenderError) -> Self {
        Self::Render(error)
    }
}

impl<T: FrameTarget> SceneHost<T> {
    /// A host over `simulation`, drawing `atlas` into `target`.
    pub fn new(simulation: Simulation, atlas: Pixels, target: T) -> Self {
        let sprites = Vec::with_capacity(simulation.world.len() as usize);
        // Before the simulation moves in, because the baseline is the world as
        // it arrives — a scene that ships a counter already at five must not be
        // heard as five purchases.
        //
        // "Before" is load-bearing and it is also *checked*: both lines above
        // borrow `simulation`, so getting the order wrong is a borrow-checker
        // error rather than a silently different baseline.
        let cue_memory = audio::CueMemory::new(&simulation.world);
        Self {
            simulation,
            atlas,
            sprites,
            camera: CameraView::IDENTITY,
            tick: 0,
            regions: Regions::WholeTexture,
            cues: Vec::new(),
            cue_memory,
            inspector: Inspector::default(),
            capture: Capture::Idle,
            target,
        }
    }

    /// Asks for the next drawn frame to be read back.
    ///
    /// Served at the end of that frame, in [`Frame::present`] and before the
    /// image is handed to the compositor — the only moment at which the thing
    /// the window is about to show still exists on the GPU where it can be
    /// copied.
    ///
    /// Asking twice before either is served is one screenshot, not two: this is
    /// a request, not a queue. A key held down would otherwise produce a file
    /// per frame.
    pub fn request_capture(&mut self) {
        // Not over a picture that has been read back and not yet collected: the
        // runner takes that on the same turn of the loop, and overwriting it
        // would throw away a screenshot that had already cost a GPU wait.
        if matches!(self.capture, Capture::Idle) {
            self.capture = Capture::Wanted;
        }
    }

    /// Takes the screenshot the last frame produced, if it produced one.
    ///
    /// Drained rather than borrowed, for the reason [`Self::take_cues`] is: the
    /// runner writes one file per picture, and a picture left in place would be
    /// written again on the next frame.
    pub fn take_capture(&mut self) -> Option<Pixels> {
        match std::mem::replace(&mut self.capture, Capture::Idle) {
            Capture::Taken(pixels) => Some(pixels),
            // Put back rather than dropped: a request that has not been served
            // is still a request.
            other => {
                self.capture = other;
                None
            }
        }
    }

    /// Takes the cues the ticks since the last call produced.
    ///
    /// Drained rather than borrowed so a runner cannot deliver the same cue
    /// twice, and so the buffer keeps its capacity across frames.
    pub fn take_cues(&mut self) -> Vec<Cue> {
        std::mem::take(&mut self.cues)
    }

    /// The same host, showing `regions` of the atlas.
    #[must_use]
    pub fn showing(mut self, regions: Regions) -> Self {
        self.regions = regions;
        self
    }

    /// Replaces the simulation — world, registry and systems — keeping
    /// everything else.
    ///
    /// **The registry moves with the world, since M6.6a**, and that is the
    /// whole reason the parameter is a [`Simulation`] rather than the two
    /// pieces this used to take. A host that kept the registry the *first*
    /// scene was read against while running the world of the *second* would be
    /// two registries in one place, which is the failure [`Self::new`]'s
    /// signature already refuses at construction; a reload is the other way in,
    /// and it is closed the same way.
    ///
    /// **The swap of ADR-0022**, and it is one assignment each: the GPU target,
    /// the atlas and the sprite buffer are untouched, so a reload keeps the
    /// window, the swapchain and the device it was drawing with. The tick
    /// counter restarts, because a reconstituted world is a new run of the
    /// simulation rather than a continuation of the old one — there is no
    /// runtime state to carry over, which is exactly what §6/M4 decided when it
    /// chose reconstitution over patching.
    /// The cue memory resets with the tick counter and for the same reason: a
    /// reconstituted world is a new run, so its music starts again rather than
    /// continuing from what the previous world had already reached.
    pub fn replace_world(&mut self, simulation: Simulation) {
        self.cue_memory = audio::CueMemory::new(&simulation.world);
        self.simulation = simulation;
        self.tick = 0;
        self.sprites.clear();
        self.cues.clear();
    }

    /// The world, for a reader that is not the render path.
    ///
    /// Hit testing needs it: a click is answered from the same world the frame
    /// draws. It is `&self`, so the "render reads only" rule is unaffected.
    pub fn world(&self) -> &World {
        &self.simulation.world
    }

    /// The names the world's state is written under.
    ///
    /// The one reader of the field M6.6a added, and it has no caller in this
    /// crate yet — the two that will want it are named in [`SceneHost`]'s own
    /// documentation. `&self`, for the same reason [`Self::world`] is: reading
    /// what a component is called cannot move the simulation.
    ///
    /// It is the registry the running world was **read against**, not one
    /// assembled beside it. Nothing here enforces that at runtime, and nothing
    /// needs to: it arrives inside a [`Simulation`] and leaves this host only
    /// with the world it names.
    ///
    /// # The seam precedes its first caller
    ///
    /// M6.6a built the capability and deliberately did not use it, naming two
    /// consumers to come: M6.6b's entity inspector and M6.4b's screenshot.
    /// **Both have landed and neither calls this method.** `extract_inspector`
    /// reads the field directly — it is inside this type, so it needs no
    /// accessor — and M6.4b's screenshot copies a texture and asks for no names
    /// at all.
    /// The prediction is recorded as disproved rather than quietly dropped,
    /// which is why the sentence is still here.
    ///
    /// So in a non-test build this method still has no caller, and it carries
    /// `#[cfg_attr(not(test), expect(dead_code, …))]` saying exactly that —
    /// the same shape `sprite_batch::placements_of` carried until M3.32 gave it
    /// one.
    ///
    /// `expect` rather than `allow`, for the reason that file records: an
    /// `expect` that stops being needed becomes an unfulfilled-lint warning and
    /// the workspace denies warnings, so **the day a production caller arrives
    /// the compiler forces the attribute out**. `not(test)` rather than plain,
    /// because the tests below call it — under `--cfg test` the expectation
    /// would already be unfulfilled, which is the same warning from the other
    /// side.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no caller: both consumers M6.6a predicted have landed and neither needed it"
        )
    )]
    pub fn registry(&self) -> &ComponentRegistry {
        &self.simulation.registry
    }

    /// Switches the debug overlay on or off, rasterising the atlas on first use.
    ///
    /// Returns the state it switched *to*, so a caller can report it without
    /// asking again.
    ///
    /// The atlas is built here rather than in [`Self::new`] because nothing that
    /// never opens the overlay should pay for it — see [`Inspector::atlas`].
    pub fn toggle_inspector(&mut self) -> bool {
        self.inspector.on = !self.inspector.on;
        if self.inspector.on && self.inspector.atlas.is_none() {
            self.inspector.atlas = Some(glyph_atlas::rasterize(INSPECTOR_SIZE_PX));
        }
        if !self.inspector.on {
            // Emptied on the way out, not merely ignored: a stale glyph list
            // would be a second batch with sprites in it, and ADR-0039 makes
            // "no sprites" the thing that produces nothing.
            self.inspector.sprites.clear();
        }
        self.inspector.on
    }

    /// Shows the next entity, wrapping at the end.
    ///
    /// A counter rather than a handle, so it survives a reload — see
    /// [`Inspector::selected`].
    pub fn inspect_next(&mut self) {
        self.inspector.selected = self.inspector.selected.wrapping_add(1);
    }

    /// Whether the overlay is currently drawing.
    pub fn inspector_on(&self) -> bool {
        self.inspector.on
    }

    /// The lines the overlay would show right now.
    ///
    /// Public so a test can assert the content without a GPU, which is the half
    /// of an inspector that can be wrong.
    #[cfg(test)]
    pub fn inspector_lines(&self) -> Vec<String> {
        crate::inspector::lines_for(
            &self.simulation.world,
            &self.simulation.registry,
            self.inspector.selected,
        )
    }

    /// Lays the inspector's lines out as glyph sprites, once per extraction.
    ///
    /// # Where the multi-line layout lives, and why here
    ///
    /// `narvo_render2d::text::layout_line` lays out **one** line; this loop is
    /// the only thing between it and a list of them. It stays in the caller
    /// rather than becoming a `layout_lines` beside it, because a second line is
    /// not a layout capability — it is a *presentation* decision, and so is the
    /// spacing. There is also no second consumer, and §2 rules out building for
    /// an absent one. D10 is untouched either way: its scope names ASCII 32–126,
    /// two sizes, advance, and no shaping, kerning or hinting — **line breaking
    /// is not among them**, so widening M3.35's single-line limit reopens no
    /// decision.
    ///
    /// The spacing comes from the atlas rather than from a literal:
    /// `size_px()` is the em the glyphs were rasterised at, so lines stay apart
    /// in proportion to the text if the size ever changes.
    fn extract_inspector(&mut self) {
        self.inspector.sprites.clear();

        if !self.inspector.on {
            return;
        }
        let Some(atlas) = self.inspector.atlas.as_ref() else {
            // `toggle_inspector` builds the atlas whenever it switches on, so
            // this is unreachable by construction. Returning rather than
            // unwrapping keeps a future caller that set the flag directly from
            // panicking in a render loop.
            return;
        };

        let lines = crate::inspector::lines_for(
            &self.simulation.world,
            &self.simulation.registry,
            self.inspector.selected,
        );

        // Top left, in pixels down from the top of the canvas — `sprites_for`
        // takes the canvas extent and converts to the world coordinates the
        // renderer wants, so nothing here does that arithmetic twice.
        //
        // **The target's own extent**, asked each extraction rather than fixed:
        // a window can be resized, and a constant would put the text outside a
        // frame of any other size. See `FrameTarget::extent`.
        let (width, height) = self.target.extent();
        let spacing = atlas.size_px() * INSPECTOR_LINE_SPACING;

        for (row, line) in lines.iter().enumerate() {
            #[expect(
                clippy::cast_precision_loss,
                reason = "an overlay has a few dozen lines at most"
            )]
            let baseline = INSPECTOR_MARGIN_PX + atlas.size_px() + row as f32 * spacing;
            let placed = text::layout_line(line, atlas, INSPECTOR_MARGIN_PX, baseline);
            self.inspector.sprites.extend(text::sprites_for(
                &placed,
                atlas.pixels(),
                width,
                height,
            ));
        }
    }

    /// The world, mutably, so a runner can put a tick's input into it.
    ///
    /// The one writer outside the scheduler, and it exists for the one thing
    /// ADR-0012 Decision 5 requires: input is written into the world's event
    /// buffer *between* ticks. The render path is unaffected — it still only
    /// reads, and `extract` takes `&self`.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.simulation.world
    }

    /// How many sprites the last extraction produced.
    pub fn sprite_count(&self) -> usize {
        self.sprites.len()
    }

    /// The sprites the last extraction produced, in drawing order.
    #[cfg(test)]
    pub fn sprites(&self) -> &[SpriteInstance] {
        &self.sprites
    }

    /// The view the last extraction produced.
    ///
    /// No longer test-only: the click path needs it, because a screen point
    /// becomes a world point through the same camera the frame was drawn with
    /// (M5.4). Reading the *last extraction's* view rather than recomputing one
    /// is what makes a click agree with the picture the user clicked on.
    pub fn camera(&self) -> CameraView {
        self.camera
    }

    /// The target, for a caller that needs to ask it something.
    pub fn target(&self) -> &T {
        &self.target
    }

    /// The target, for a caller that needs to change it - a resize.
    pub fn target_mut(&mut self) -> &mut T {
        &mut self.target
    }
}

impl<T: FrameTarget> FrameHost for SceneHost<T> {
    type Error = FrameError;

    /// # The audio seam, and why it is here rather than beside the log line
    ///
    /// `FrameLoop` calls this once per due tick — `for _ in 0..ticks {
    /// host.tick()?; }` (`narvo-core/src/frame.rs:349-351`) — so a cue produced
    /// here belongs to the tick that produced it whatever the frame rate did.
    /// The obvious alternative was the runner's own per-frame call beside
    /// `tally_changes` (`window.rs`), and it would have been wrong: a frame owes
    /// up to eight ticks, and two buys inside one frame would have collapsed
    /// into a single observed change and therefore a single click.
    ///
    /// After the systems and before the increment, so the counters read are the
    /// ones this tick left behind and the cue carries this tick's number.
    fn tick(&mut self) -> Result<(), Self::Error> {
        self.simulation
            .scheduler
            .run(&mut self.simulation.world, &SystemContext::new(self.tick));
        self.cues.extend(audio::cues_of(
            &self.simulation.world,
            self.tick,
            &mut self.cue_memory,
        ));
        self.tick += 1;
        Ok(())
    }

    /// Reads the world into the renderer's own scalars.
    ///
    /// This is `placements_of`'s first production caller. Until now the seam
    /// existed and was compiled dead — both it and `camera_of` carried an
    /// `expect(dead_code)` whose reason read "the seam precedes its first
    /// caller". This is that caller, and removing those attributes was not
    /// optional: with a caller in place `dead_code` no longer fires, so the
    /// expectation is *unfulfilled* — rustc warns "this lint expectation is
    /// unfulfilled" under the warn-by-default `unfulfilled_lint_expectations`.
    /// That warning alone does not fail `cargo build`; the deny arrives on the
    /// command line, so it is step 4 of the verification set — `cargo clippy`
    /// with `-D warnings` — that turns it into an error.
    ///
    /// The world is enumerated twice, once by each function, which their own
    /// docs record as a known cost. It is left as it is: this task measures,
    /// and `docs/perf/BASELINE.md` carries what the extraction actually costs.
    fn extract(&mut self) -> Result<(), Self::Error> {
        self.camera = camera_of(&self.simulation.world);

        self.sprites.clear();

        match &self.regions {
            // Unchanged since M3.32, and deliberately so: this is the path every
            // blessed reference is drawn through, and M4.8's constraint was that
            // it stay exactly as it is. `placements_of` is not called by the
            // other arm at all.
            Regions::WholeTexture => {
                self.sprites
                    .extend(
                        placements_of(&self.simulation.world)
                            .into_iter()
                            .map(|drawn| {
                                SpriteInstance::new(drawn.placement, TextureRegion::WHOLE_TEXTURE)
                                    .sampled(drawn.filter)
                                    .tinted(drawn.tint)
                            }),
                    );
            }
            // The scene-file path. A name that is not in the table cannot happen
            // — `narvo_view2d::load_for` refuses the load before a window opens — so
            // this skips rather than guessing, and a skipped sprite would be the
            // symptom of that check having been bypassed rather than of content.
            Regions::ByName(table) => {
                self.sprites
                    .extend(
                        regions_of(&self.simulation.world)
                            .into_iter()
                            .filter_map(|drawn| {
                                let region = *table.get(&drawn.region)?;
                                Some(
                                    SpriteInstance::new(drawn.placement, region)
                                        .sampled(drawn.filter)
                                        .tinted(drawn.tint),
                                )
                            }),
                    );
            }
        }

        self.extract_inspector();

        Ok(())
    }

    fn acquire(&mut self) -> Result<Acquisition, Self::Error> {
        Ok(self.target.acquire()?)
    }

    fn encode(&mut self) -> Result<(), Self::Error> {
        // The second batch of ADR-0039, and the reason that seam was built. It
        // is `None` unless the overlay is on *and* produced glyphs, so the
        // ordinary frame emits exactly the command sequence it did before there
        // was an inspector — which is what keeps "the shared code is unchanged"
        // as the evidence for the ten blessed references.
        // On `self.inspector` rather than on `self`, so the borrow checker can
        // see that the batch and `self.target` touch different fields.
        let overlay = self.inspector.batch();
        self.target
            .draw(&self.atlas, &self.sprites, overlay, self.camera)?;
        Ok(())
    }

    /// # Where the screenshot is taken, and why it is here
    ///
    /// After `encode` and before the frame is handed over. That is the only
    /// moment at which the image the window is about to show exists somewhere it
    /// can be copied from: `acquire` has one but it holds the previous
    /// occupant's contents, and after `present` the swapchain owns it again.
    ///
    /// **Nothing is drawn a second time and nothing is drawn differently.** The
    /// capture is a copy of the texture `encode` just resolved into, so the
    /// overlay is in the picture exactly when it was in the frame, and the draw
    /// order is the one that was drawn. An implementation that re-rendered the
    /// scene into its own texture would agree with the window only by an
    /// argument about the inputs; this one agrees by being the same bytes.
    ///
    /// A failed read-back fails the frame, like a failed present. The runner
    /// treats a frame error as fatal, which is heavier than a screenshot
    /// deserves — so the surface that cannot be copied at all is refused
    /// *before* the copy, by `WindowTarget::read_back`'s own check, and reaches
    /// the runner as an ordinary error before any frame is spent on it.
    fn present(&mut self) -> Result<(), Self::Error> {
        if matches!(self.capture, Capture::Wanted)
            && let Some(pixels) = self.target.capture()?
        {
            self.capture = Capture::Taken(pixels);
        }

        self.target.present()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameTarget, Offscreen, Regions, SceneHost};
    use crate::sim::{Simulation, scene};
    use narvo_core::FixedTimestep;
    use narvo_core::frame::{Acquisition, Clock, FrameHost as _, FrameLoop};
    use narvo_ecs::{Camera, Follow, Shake, SystemContext, Transform, canonical_dump, state_hash};
    use narvo_render2d::{
        CameraView, OffscreenTarget, Pixels, RenderError, SpriteBatch, SpriteInstance,
        TextureRegion,
    };
    use std::cell::Cell;
    use std::time::Duration;

    /// A clock that stands still until the test moves it.
    ///
    /// The smoke test uses this rather than [`super::Monotonic`] so that the
    /// number of ticks per frame is decided by the test rather than by how fast
    /// the machine happens to be - a real clock would make the picture depend on
    /// the GPU's mood, which is the opposite of what a smoke test wants.
    ///
    /// **It does not advance on being read**, which is the whole point and was
    /// worth a wrong first version: `FrameLoop::step` reads the clock once per
    /// phase, so a clock that ticked on every read ran six times the intended
    /// simulation and walked the camera clean off the scene.
    struct HeldClock {
        now: Duration,
    }

    impl Clock for HeldClock {
        fn now(&mut self) -> Duration {
            self.now
        }
    }

    /// The offscreen target, or `None` on a machine with no usable adapter.
    ///
    /// The same skip the renderer's own golden tests use. CI sets
    /// `NARVO_REQUIRE_GPU`, which turns a skip into a failure there.
    fn target_or_skip(width: u32, height: u32) -> Option<OffscreenTarget> {
        match OffscreenTarget::new(width, height) {
            Ok(target) => Some(target),
            Err(error) => {
                assert!(
                    std::env::var_os("NARVO_REQUIRE_GPU").is_none(),
                    "NARVO_REQUIRE_GPU is set and no adapter was usable: {error}"
                );
                eprintln!("skipping: no usable adapter ({error})");
                None
            }
        }
    }

    /// Whether one channel clearly dominates the other two.
    ///
    /// A structure probe, not a colour comparison: the thresholds are wide
    /// enough that MSAA, the sRGB surface and any rasteriser disagreement leave
    /// the answer unchanged.
    fn dominant(pixel: [u8; 4], channel: usize) -> bool {
        let value = u32::from(pixel[channel]);
        value > 150
            && (0..3)
                .filter(|index| *index != channel)
                .all(|index| u32::from(pixel[index]) + 80 < value)
    }

    /// A target that counts what it was asked to do and draws nothing.
    #[derive(Debug, Default)]
    struct Counting {
        draws: usize,
        /// How many of those draws carried a non-empty second batch.
        ///
        /// The app-side half of M6.6c's instrument. `batch_plan` proves that an
        /// empty batch produces no run; this proves that `SceneHost` does not
        /// send one in the first place.
        overlays: usize,
        /// How many times a frame was read back.
        ///
        /// A [`Cell`], because `FrameTarget::capture` takes `&self` — it copies
        /// a texture and must not be able to change what is in it. Counting is
        /// the one thing a test target wants to do there anyway.
        ///
        /// The number matters as much as the picture: a read-back waits for the
        /// GPU, so a frame nobody asked about must produce **zero** of these.
        captures: Cell<usize>,
        /// Answer `None` — "there was nothing to read".
        ///
        /// The window reaches that state only through a bug in the call order,
        /// so it is unreachable from a runner and still has to be decided: a
        /// request that could not be served is kept, not dropped.
        blind: bool,
        /// Whether this frame has already been handed over.
        ///
        /// **This exists because a red flank found nothing.** M6.4b moved the
        /// capture in `SceneHost::present` to *after* `self.target.present()`
        /// and the whole suite stayed green — `Offscreen::present` does nothing,
        /// so its picture survives a handover that a swapchain image does not.
        /// On a window that ordering is fatal and silent: `Windowed::present`
        /// takes the frame out, so a capture afterwards finds `None`, returns
        /// no picture, and leaves the request pending for ever.
        ///
        /// A window cannot be put in a test, so the constraint is modelled here
        /// instead, in the target whose job is to watch the call order.
        presented: Cell<bool>,
    }

    impl FrameTarget for Counting {
        fn extent(&self) -> (u32, u32) {
            // It draws nothing, so any extent is as good as another; a square
            // one keeps a layout computed against it easy to reason about.
            (256, 256)
        }

        fn acquire(&mut self) -> Result<Acquisition, RenderError> {
            self.presented.set(false);
            Ok(Acquisition::Ready)
        }

        fn draw(
            &mut self,
            _atlas: &Pixels,
            _sprites: &[SpriteInstance],
            overlay: Option<SpriteBatch<'_>>,
            _camera: CameraView,
        ) -> Result<(), RenderError> {
            self.draws += 1;
            // Counted separately from `draws`, so a test can say not just that a
            // frame was drawn but whether it carried a second batch. `SceneHost`
            // passes `None` today, and `the_host_asks_for_no_overlay` holds it
            // to that — the guard that would fail the day something starts
            // sending one without a decision.
            if overlay.is_some_and(|batch| !batch.sprites.is_empty()) {
                self.overlays += 1;
            }
            Ok(())
        }

        fn capture(&self) -> Result<Option<Pixels>, RenderError> {
            assert!(
                !self.presented.get(),
                "the capture ran after the frame had been handed over. On a \
                 window there is nothing left to copy at that point, and the \
                 request would go unserved without an error anywhere"
            );
            self.captures.set(self.captures.get() + 1);

            if self.blind {
                return Ok(None);
            }

            // One black texel. This target draws nothing, so there is nothing
            // truthful to hand back; what the tests using it check is the
            // wiring — when a capture happens and how often — and the picture
            // itself is `Offscreen`'s to check.
            Ok(Some(
                Pixels::from_rgba8(1, 1, vec![0, 0, 0, 255]).expect("a 1x1 image is valid"),
            ))
        }

        fn present(&mut self) -> Result<(), RenderError> {
            self.presented.set(true);
            Ok(())
        }
    }

    /// **The overlay is off until it is asked for, and then it is on.**
    ///
    /// This is M6.6c's `the_host_asks_for_no_overlay` in its successor form. It
    /// used to assert only that the host sent no second batch, and it was
    /// written to be the first test that fails the day an inspector arrives —
    /// "sending a batch is a decision, not a detail". The inspector has arrived,
    /// so the decision is recorded here rather than the absence:
    ///
    /// - **off:** no second batch at all, so the frame emits exactly the command
    ///   sequence it emitted before there was an inspector. That is what keeps
    ///   "the shared code is unchanged" the evidence for the ten blessed
    ///   references (ADR-0039), and it is asserted rather than argued.
    /// - **on:** a second batch, every frame, carrying glyphs.
    ///
    /// The off half is the one that guards the references; the on half is the
    /// counter-proof without which the off half would pass for a host that
    /// simply never draws an overlay.
    #[test]
    fn the_overlay_is_off_until_asked_for_and_then_on() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        assert!(!host.inspector_on(), "an overlay nobody asked for is off");

        host.extract().expect("extraction cannot fail");
        for _ in 0..3 {
            host.encode().expect("encoding into a counter cannot fail");
        }

        assert_eq!(host.target().draws, 3, "three frames were encoded");
        assert_eq!(
            host.target().overlays,
            0,
            "the host sent a second batch while the overlay was off"
        );

        assert!(host.toggle_inspector(), "toggling from off turns it on");
        host.extract().expect("extraction cannot fail");
        for _ in 0..2 {
            host.encode().expect("encoding into a counter cannot fail");
        }

        assert_eq!(host.target().draws, 5);
        assert_eq!(
            host.target().overlays,
            2,
            "an overlay that is on must reach the renderer every frame"
        );

        assert!(!host.toggle_inspector(), "toggling again turns it off");
        host.extract().expect("extraction cannot fail");
        host.encode().expect("encoding into a counter cannot fail");

        assert_eq!(host.target().draws, 6);
        assert_eq!(
            host.target().overlays,
            2,
            "switching the overlay off must stop the second batch, not merely \
             empty the picture"
        );
    }

    /// The overlay lays every line out, and the glyphs land in the second batch.
    ///
    /// The seam between the content half (`crate::inspector`, tested without a
    /// device) and the drawing half: this asserts that *what the lines say*
    /// becomes *sprites*, in proportion, and that the count grows with the
    /// number of drawable characters rather than staying at one line's worth.
    #[test]
    fn every_line_of_the_inspector_reaches_the_sprite_buffer() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        host.toggle_inspector();
        host.extract().expect("extraction cannot fail");

        let lines = host.inspector_lines();
        assert!(lines.len() > 1, "the scene world has components to show");

        // A space has an advance and no region (M3.34), so it moves the pen and
        // produces no sprite. Every *other* drawable character is one sprite.
        let expected: usize = lines
            .iter()
            .map(|line| line.chars().filter(|ch| *ch != ' ').count())
            .sum();

        assert_eq!(
            host.inspector.sprites.len(),
            expected,
            "one sprite per non-space character, across all {} lines",
            lines.len()
        );
    }

    /// Switching the overlay off empties the glyphs rather than leaving them.
    ///
    /// A stale glyph list would be a second batch with sprites in it, and
    /// ADR-0039 makes "no sprites" the thing that produces nothing — so the
    /// emptying is what makes the off state a no-op rather than an invisible
    /// draw.
    #[test]
    fn switching_the_overlay_off_empties_its_glyphs() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        host.toggle_inspector();
        host.extract().expect("extraction cannot fail");
        assert!(
            !host.inspector.sprites.is_empty(),
            "the overlay drew glyphs"
        );

        host.toggle_inspector();
        assert!(
            host.inspector.sprites.is_empty(),
            "the glyphs survived the overlay being switched off"
        );
    }

    /// **The overlay follows the world, frame by frame.**
    ///
    /// The failure this guards is the one an inspector is worst at revealing:
    /// text that is *plausible* but stale. The lines are rebuilt from
    /// `&self.simulation.world` on every extraction rather than cached, and this
    /// is what holds that — tick the world, extract again, and the drawn text
    /// must have moved with it.
    ///
    /// The scene world is the right subject: its camera chases a wandering
    /// target, so a `transform` changes every tick and the change reaches the
    /// value RON writes.
    #[test]
    fn the_overlay_follows_the_world_rather_than_caching_it() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        // **The camera, not entity 0.** The scene's sprites are static; only the
        // camera and the wander target move, which a first version of this test
        // found the hard way by comparing an unmoving transform with itself.
        // Stepping until the head names an entity whose text actually changes
        // would be a search inside a test; naming the moving one is not
        // available either, since the selection is an index. So: step through
        // every entity and require that *some* drawn overlay moved.
        host.toggle_inspector();

        let mut before = Vec::new();
        for _ in 0..host.simulation.world.len() {
            host.extract().expect("extraction cannot fail");
            // **The drawn glyphs, not a fresh call to `lines_for`.** An earlier
            // version of this test re-computed the lines and compared those,
            // which passes even when `extract_inspector` caches — the injection
            // in M6.6d's red flank (a) is what found that. What the overlay
            // *shows* is this buffer.
            before.push(host.inspector.sprites.clone());
            host.inspect_next();
        }

        for _ in 0..16 {
            host.tick().expect("a tick cannot fail");
        }

        let mut after = Vec::new();
        for _ in 0..host.simulation.world.len() {
            host.extract().expect("extraction cannot fail");
            after.push(host.inspector.sprites.clone());
            host.inspect_next();
        }

        assert_ne!(
            before, after,
            "sixteen ticks moved the world and every drawn overlay stayed the same"
        );
        assert!(
            before.iter().all(|glyphs| !glyphs.is_empty()),
            "an overlay that is on must draw something for every entity"
        );
    }

    /// **The overlay is drawn from the glyph atlas, through a real target.**
    ///
    /// M6.6c's red flank (c) covers the *seam* — that a second batch samples its
    /// own texture — in `narvo-render2d`'s own tests. It does not cover this
    /// crate's *use* of it, and M6.6d's red flank (c) measured exactly that gap:
    /// pointing the overlay at the scene atlas instead of the glyph atlas broke
    /// nothing, because no test rendered the inspector through a target that
    /// produces pixels.
    ///
    /// This is that test. It is not a golden image and writes nothing to
    /// `tests/golden/`: what it asserts is that switching the overlay on
    /// *changes the frame*, and that the change is glyph-shaped — text is drawn
    /// in the top-left corner, where the scene's own sprites are not.
    #[test]
    fn the_overlay_reaches_the_frame_and_comes_from_the_glyph_atlas() {
        let Some(target) = target_or_skip(256, 256) else {
            return;
        };

        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Offscreen::new(target),
        );

        host.extract().expect("extraction cannot fail");
        host.encode().expect("encoding cannot fail");
        let without = host
            .target()
            .last_frame()
            .expect("a frame was drawn")
            .clone();

        host.toggle_inspector();
        host.extract().expect("extraction cannot fail");
        host.encode().expect("encoding cannot fail");
        let with = host.target().last_frame().expect("a frame was drawn");

        assert_ne!(
            with.rgba(),
            without.rgba(),
            "switching the overlay on changed nothing in the frame"
        );

        // **Where, not merely whether.** The glyphs are laid out from the top
        // left with a four-pixel margin, so the change has to be there. A frame
        // that differed only elsewhere would mean the overlay drew something,
        // somewhere, which is what "it renders" alone would let through.
        let changed_top_left = (0..64)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .any(|(x, y)| with.pixel(x, y) != without.pixel(x, y));
        assert!(
            changed_top_left,
            "the overlay changed the frame, but not where its text is laid out"
        );

        // **From the glyph atlas, not merely from somewhere.** The rasteriser
        // writes `[value, value, value, value]` — greyscale, premultiplied
        // (`glyph_atlas.rs`'s `write_texel`) — so a fully covered glyph texel is
        // `[255, 255, 255, 255]` and composites to white over anything. The
        // scene atlas is red, green, blue and brown (`content.rs`), whose
        // brightest pixel is `[255, 0, 0, 255]`: **drawing the overlay from it
        // could not produce a white pixel.** M6.6d's red flank (c) is what this
        // assertion answers — without it, pointing the overlay at the scene
        // atlas broke nothing.
        assert!(
            (0..256)
                .flat_map(|y| (0..256).map(move |x| (x, y)))
                .any(|(x, y)| with.pixel(x, y) == Some([255, 255, 255, 255])
                    && without.pixel(x, y) != Some([255, 255, 255, 255])),
            "no pixel turned white, so the overlay did not draw glyph coverage"
        );

        // And the scene is still under it rather than replaced by it: **some**
        // pixel is unchanged.
        //
        // Not a named corner. A first version asserted the bottom-right 64
        // square was untouched and failed: at 32 px on a 256 by 256 target the
        // five lines of text span nearly the whole frame, so no corner is free
        // of it. That is a legibility question for a human (§ the named
        // boundary) and not a defect — but it means the property to assert here
        // is composition, not location.
        let untouched = (0..256)
            .flat_map(|y| (0..256).map(move |x| (x, y)))
            .any(|(x, y)| with.pixel(x, y) == without.pixel(x, y));
        assert!(
            untouched,
            "every pixel changed, so the overlay replaced the frame rather than \
             composing over it"
        );
    }

    /// Stepping moves to another entity, and the head says which.
    #[test]
    fn stepping_shows_the_next_entity() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        host.toggle_inspector();
        let first = host.inspector_lines();
        host.inspect_next();
        let second = host.inspector_lines();

        assert_ne!(first, second, "stepping did not change what is shown");
        assert!(first[0].starts_with("inspector 1/"));
        assert!(second[0].starts_with("inspector 2/"));
    }

    #[test]
    fn the_real_loop_draws_frames_offscreen_without_panicking() {
        // The offscreen smoke: N frames through the same `FrameLoop` and the
        // same `SceneHost` a window runs, with only the target swapped. It is
        // not a golden image and writes nothing to `tests/golden/` - what it
        // asserts is that the loop runs, that every frame found somewhere to
        // draw, and that the last frame is a picture rather than a black field.
        let Some(target) = target_or_skip(256, 256) else {
            return;
        };

        // Large enough that the camera cannot leave it. The wander reaches
        // `WANDER_RADIUS` = 600 world units from the origin and the camera is a
        // damped chase of it, so the camera stays inside +-600; this grid runs
        // from -768 to +744 (64 columns, centred on the origin) and the view is
        // 128 units either side, so there are sprites in front of the camera
        // whatever the tick. A smaller grid makes this test depend on how many
        // ticks happened to run, which is how the first version of it failed.
        let simulation = scene::build_with(4_096).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Offscreen::new(target),
        );

        // One tick per frame exactly: the clock is moved on by one tick length
        // between frames and stands still inside them, so the accumulator hands
        // out exactly one tick each time after the first.
        let step = FixedTimestep::default().step();
        let mut clock = HeldClock {
            now: Duration::ZERO,
        };
        let mut frames = FrameLoop::new(FixedTimestep::default());

        const FRAMES: usize = 8;
        let mut log = narvo_core::frame::FrameLog::with_capacity(FRAMES);
        for _ in 0..FRAMES {
            clock.now += step;
            log.push(
                frames
                    .step(&mut clock, &mut host)
                    .expect("no frame may fail"),
            );
        }

        assert_eq!(log.len(), FRAMES);
        assert_eq!(
            log.drawn(),
            FRAMES,
            "an offscreen target always has somewhere to draw"
        );
        assert_eq!(
            host.sprite_count(),
            4_097,
            "every sprite plus the wander target, which carries a Transform too"
        );

        let frame = host.target().last_frame().expect("eight frames were drawn");
        assert_eq!((frame.width(), frame.height()), (256, 256));

        // The structure probe. Not a reference and not a threshold: the atlas is
        // four coloured quadrants, so a frame that drew it has pixels where red,
        // green and blue each dominate. A black frame, a frame that missed the
        // atlas, and a frame drawn from an empty batch all fail this.
        let mut found = [false; 3];
        for y in 0..frame.height() {
            for x in 0..frame.width() {
                let pixel = frame.pixel(x, y).expect("inside the image");
                for (channel, seen) in found.iter_mut().enumerate() {
                    *seen = *seen || dominant(pixel, channel);
                }
            }
        }

        assert!(
            found.iter().all(|seen| *seen),
            "the last frame is missing one of the atlas's colours: {found:?}"
        );
    }

    #[test]
    fn the_frame_path_is_the_first_production_caller_of_the_extraction_seam() {
        // `placements_of` and `camera_of` carried `expect(dead_code)` with the
        // reason "the seam precedes its first caller" until this task. This pins
        // that they are now genuinely reached through the frame path rather than
        // only from their own module's tests: one sprite comes out per entity
        // carrying a `Transform`, which is what the extraction promises.
        let simulation = scene::build_with(16).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        host.extract().expect("extraction cannot fail");

        assert_eq!(host.sprite_count(), 17, "sixteen sprites plus the target");

        // `camera_of` too, and asserted separately: the sprite count would be
        // exactly the same if the camera seam were never reached at all. The
        // scene's camera starts at the origin and the wander pulls it away, so a
        // view still reading as the identity after eight ticks means nothing was
        // extracted from it.
        for _ in 0..8 {
            host.tick().expect("a tick cannot fail");
        }
        host.extract().expect("extraction cannot fail");
        assert_ne!(
            host.camera(),
            CameraView::IDENTITY,
            "the extraction produced the identity view, so `camera_of` is not reached"
        );
    }

    /// The default is the whole texture, which is what leaves every blessed
    /// reference valid.
    ///
    /// The half of M4.7's display-rule test that survives its rule's retirement:
    /// `QuadrantBySlot` is gone (ADR-0024) and this assertion is not about it —
    /// it is about the path six blessed images are drawn through still being the
    /// path a freshly built host takes.
    #[test]
    fn a_fresh_host_draws_the_whole_texture() {
        let simulation = scene::build_with(6).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        host.extract().expect("extraction cannot fail");
        assert_eq!(host.sprites.len(), 7, "six sprites plus the wander target");
        assert!(
            host.sprites
                .iter()
                .all(|sprite| sprite.region == TextureRegion::WHOLE_TEXTURE),
            "the default is what the window has always drawn"
        );
    }

    /// Under `ByName`, an entity shows the region it names — and one with no
    /// `Sprite` shows nothing at all.
    ///
    /// The successor rule, and the second assertion is the visible consequence
    /// ADR-0024 records: the sprite-field demo's entities carry no `Sprite`, so
    /// in this mode they draw nothing. Under `QuadrantBySlot` every transform
    /// drew something whether or not the content asked for it.
    #[test]
    fn under_by_name_an_entity_draws_what_it_names_and_nothing_otherwise() {
        use narvo_ecs::{Sprite, Transform};
        use std::collections::BTreeMap;

        let atlas = crate::content::demo_texture();
        let left = TextureRegion::from_texels(0, 0, 4, 4, &atlas);
        let right = TextureRegion::from_texels(4, 0, 4, 4, &atlas);
        let table = BTreeMap::from([("left".to_owned(), left), ("right".to_owned(), right)]);

        let mut world = narvo_ecs::World::new();
        for (index, name) in ["right", "left"].iter().enumerate() {
            let entity = world.spawn();
            world
                .insert(entity, Transform::IDENTITY)
                .expect("just spawned");
            world
                .insert(entity, Sprite::new(*name))
                .expect("just spawned");
            // Depth puts "left" first, so the order is content's rather than
            // spawn order's — the same rule `placements_of` follows.
            world
                .insert(entity, narvo_ecs::Layer::at(1.0 - index as f32))
                .expect("just spawned");
        }
        // A transform with no sprite: present in the world, absent from the
        // picture.
        let bare = world.spawn();
        world
            .insert(bare, Transform::IDENTITY)
            .expect("just spawned");

        // The one host in this file built from a hand-made world rather than
        // from a builder, so the registry has to be named explicitly. It is the
        // engine set, which is what covers the three components spawned above —
        // the point of `Simulation` being the parameter is that a world and the
        // names of its components arrive together, and that holds here too.
        let mut host = SceneHost::new(
            Simulation {
                world,
                registry: engine_registry(),
                scheduler: narvo_ecs::Scheduler::new(),
            },
            crate::content::demo_texture(),
            Counting::default(),
        )
        .showing(Regions::ByName(table));

        host.extract().expect("extraction cannot fail");

        assert_eq!(
            host.sprites.len(),
            2,
            "the entity with no `Sprite` should not have been drawn"
        );
        assert_eq!(host.sprites[0].region, left, "depth 0.0 is drawn first");
        assert_eq!(host.sprites[1].region, right);
    }

    #[test]
    fn a_reload_swaps_the_world_and_starts_the_tick_count_over() {
        // `replace_world` is the swap of ADR-0022. What it must do: exchange
        // the world and its systems, drop what the old world extracted, and
        // restart the tick counter, because a reconstituted world is a new run
        // rather than a continuation. What it must *not* do is touch the target
        // — the window, the swapchain and the device outlive a reload.
        let first = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(first, crate::content::demo_texture(), Counting::default());

        for _ in 0..3 {
            host.tick().expect("a tick cannot fail");
        }
        host.extract().expect("extraction cannot fail");
        assert_eq!(host.tick, 3);
        assert_eq!(host.sprite_count(), 5);

        let second = scene::build_with(11).expect("the scene builds");
        host.replace_world(second);

        assert_eq!(host.tick, 0, "a reconstituted world is a new run");
        assert_eq!(
            host.sprite_count(),
            0,
            "the sprite buffer still holds what the old world extracted"
        );

        host.extract().expect("extraction cannot fail");
        assert_eq!(
            host.sprite_count(),
            12,
            "the next frame draws the world that was loaded, not the one that was replaced"
        );
    }

    /// The engine's own component set, for the one world in this file that is
    /// hand-built rather than produced by a builder.
    fn engine_registry() -> narvo_ecs::ComponentRegistry {
        let mut registry = narvo_ecs::ComponentRegistry::new();
        narvo_ecs::register_engine_components(&mut registry).expect("a fresh registry cannot fail");
        registry
    }

    /// The address of a registry's storage, which survives moving the value.
    ///
    /// # Why identity is asked this way
    ///
    /// The claim under test is not "the host has a registry" and not "the host
    /// has a registry that looks like the scene's". It is that the host holds
    /// **the same one** — and two registries built by the same builder agree on
    /// every *value* the type reports: `len`, `contains`, `name_of_type`, and
    /// the `ComponentInfo`s `iter` walks. So no comparison of contents can tell
    /// them apart, which is precisely the failure this task exists to rule out.
    ///
    /// A `ComponentRegistry` is two `BTreeMap`s (`narvo-ecs/src/registry.rs:189-196`),
    /// so its entries live behind a heap pointer that a move of the owning
    /// value copies rather than changes. The address of the first entry is
    /// therefore stable across `Simulation` moving into `SceneHost`, and it is
    /// **not** stable across a fresh construction or a `Clone` — which are the
    /// two ways a second registry could appear. Comparing it is the sharpest
    /// available answer and it needs no `unsafe`: taking a `*const` from a
    /// reference and comparing two of them are both safe operations.
    ///
    /// A clone counts as a second registry here, deliberately. One that is
    /// taken once and never followed through a reload can diverge from the
    /// world it names, which is the same defect by a slower route.
    fn storage_address(registry: &narvo_ecs::ComponentRegistry) -> *const narvo_ecs::ComponentInfo {
        registry
            .iter()
            .next()
            .expect("every registry in this file names at least one component")
    }

    /// The registry in the host is the scene's own, not a second one beside it.
    #[test]
    fn the_host_keeps_the_registry_the_scene_was_read_against() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let names = simulation.registry.len();
        let before = storage_address(&simulation.registry);

        let host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        assert_eq!(
            storage_address(host.registry()),
            before,
            "the host is naming components out of a different registry than the one the scene \
             was read against"
        );
        // Beside the identity, not instead of it: this one would also pass for a
        // second registry that happened to be built the same way, which is why
        // it is not the assertion this test is named after.
        assert_eq!(host.registry().len(), names);
    }

    /// And a reload carries the new scene's registry, not the previous one.
    ///
    /// The other way a second registry gets in. Construction is closed by the
    /// signature of `new`; a swap is a separate door and `replace_world` takes
    /// a whole [`Simulation`] for exactly this reason.
    #[test]
    fn a_reload_carries_the_registry_of_the_scene_it_loaded() {
        let first = scene::build_with(4).expect("the scene builds");
        let first_registry = storage_address(&first.registry);
        let mut host = SceneHost::new(first, crate::content::demo_texture(), Counting::default());
        assert_eq!(storage_address(host.registry()), first_registry);

        let second = scene::build_with(11).expect("the scene builds");
        let second_registry = storage_address(&second.registry);
        // Two builds of the same scene produce registries with the same
        // contents, so a content check could not tell this apart from doing
        // nothing. The addresses differ, which is what makes the assertion
        // below mean something — and they must, rather than happening to: the
        // first registry is alive inside `host` while the second is built, so
        // the allocator cannot hand out the one address for both.
        assert_ne!(
            second_registry, first_registry,
            "the two builds returned the same allocation, so this test cannot distinguish them"
        );

        host.replace_world(second);

        assert_eq!(
            storage_address(host.registry()),
            second_registry,
            "the reload swapped the world but left the host naming the components of the scene \
             that is no longer running"
        );
    }

    #[test]
    fn the_scene_costs_three_draw_calls_at_every_size_that_is_measured() {
        // **This guard exists because its absence let a defect through.** The
        // scene asks half its sprites for `Nearest` and half for `Linear`, and
        // an earlier revision assigned them by `index % 2` - alternating every
        // sprite. `batch_runs` cuts wherever the sampler wish changes, so that
        // is one run *per sprite*: at 50 000 sprites, 50 001 draw calls instead
        // of three. Everything still drew correctly and every test stayed green,
        // because nothing asserted the run count; only the frame-time
        // measurement noticed, and only because it was six times too slow.
        //
        // Three, not two: the two sampler blocks, plus the wander target, which
        // carries no `Sampling`, falls to the `Nearest` default and is drawn
        // last because it is spawned last.
        //
        // Asserted across three sizes, because a layout that happens to be
        // contiguous at one count and not at another is the failure this is
        // guarding against. Not "however many": `sampling_for` puts a one-sprite
        // scene entirely on `Nearest`, so `build_with(1)` yields one run and
        // `build_with(0)` yields one. Both are degenerate and neither is drawn
        // by anything; the sizes below bracket what is.
        for sprites in [16_usize, 512, 4_096] {
            let simulation = scene::build_with(sprites).expect("the scene builds");
            let mut host = SceneHost::new(
                simulation,
                crate::content::demo_texture(),
                Counting::default(),
            );

            host.extract().expect("extraction cannot fail");

            let runs = narvo_render2d::batch_runs(host.sprites());
            assert_eq!(
                runs.len(),
                3,
                "{sprites} sprites produced {} draw calls rather than three; \
                 the sampler wishes are not two contiguous blocks",
                runs.len()
            );
        }
    }

    #[test]
    fn the_scene_registers_every_component_any_entity_carries() {
        // The registration duty, proven mechanically rather than by reading the
        // list back: `canonical_dump` refuses any component whose type is not in
        // the registry, so a dump that succeeds proves none is missing.
        //
        // Checked after ticks as well as at build, because a system may insert a
        // component the build never did - and the dump reads the types an entity
        // carries now, not the ones it was created with.
        let simulation = scene::build_with(32).expect("the scene builds");
        let mut world = simulation.world;
        let registry = simulation.registry;

        let built = canonical_dump(&world, &registry).expect("everything is registered at build");

        for tick in 0..600 {
            simulation
                .scheduler
                .run(&mut world, &SystemContext::new(tick));
        }

        let ticked =
            canonical_dump(&world, &registry).expect("everything is registered after ticks");

        // Each of the six the standing duty names, plus this scene's own, by the
        // stable name it was registered under. `canonical_dump` succeeding says
        // nothing is unregistered; this says the six are actually *carried*,
        // which a registration alone does not.
        for name in [
            "transform",
            "layer",
            "sampling",
            "camera",
            "follow",
            "shake",
            "wander",
        ] {
            assert!(
                built.contains(name),
                "the dump never mentions `{name}`, so the duty is met only on paper"
            );
        }

        assert_ne!(
            state_hash(&built),
            state_hash(&ticked),
            "600 ticks changed nothing, so the scene is not simulating at all"
        );
    }

    #[test]
    fn the_camera_follows_the_wander_and_the_shake_keeps_firing() {
        // What makes this scene worth measuring: the camera moves. A still
        // camera would cost the same to extract but would leave the coverage
        // argument of D14 untouched.
        //
        // It also pins that `Shake` is exercised rather than merely registered -
        // a shake armed once at build has halved itself to zero after about
        // 154 ticks, so the scene re-arms it every 240 on a schedule that is a
        // function of the tick number rather than a draw from a generator.
        let simulation = scene::build_with(8).expect("the scene builds");
        let mut world = simulation.world;

        let camera_entity = world
            .entity_ids()
            .into_iter()
            .find(|entity| world.get::<Camera>(*entity).is_ok())
            .expect("the scene has a camera");

        let start = *world.get::<Camera>(camera_entity).expect("it is there");

        // Past the second arming: the schedule fires at ticks 0, 240, 480, and a
        // shake decays to its cutoff well inside that gap. Stopping just after
        // 240 is what distinguishes "re-armed on a schedule" from "armed once at
        // build and long dead", which is what the first version of this test
        // failed to tell apart.
        for tick in 0..=240 {
            simulation
                .scheduler
                .run(&mut world, &SystemContext::new(tick));
        }

        let moved = *world.get::<Camera>(camera_entity).expect("it is there");
        assert_ne!(
            (start.x, start.y),
            (moved.x, moved.y),
            "the camera never moved, so nothing followed anything"
        );

        // The target really is wandering, rather than the camera drifting.
        let follow = *world.get::<Follow>(camera_entity).expect("it follows");
        assert!(!follow.lost, "the follow lost its target");
        let target = *world
            .get::<Transform>(follow.target)
            .expect("the target has a transform");
        assert!(
            target.x != 0.0 || target.y != 0.0,
            "the wander target never left the origin"
        );

        // And the shake is alive 240 ticks in, which it can only be because it
        // was armed again: the one from tick 0 is long past its cutoff.
        let shake = *world.get::<Shake>(camera_entity).expect("it shakes");
        assert!(
            !shake.at_rest(),
            "the shake expired and was never re-armed, so it is registered but dead"
        );
    }

    // --- The drop scene, drawn (M5b.4) --------------------------------------
    //
    // The picture half of `scenes/physics_drop.ron`; the simulation half is in
    // `sim::scene_file`, and both take the file and the tick from
    // `sim::scene_file::drop_scene` so that neither can drift into describing a
    // different frame.
    //
    // **It goes through this module's own host rather than around it.** The
    // frame is produced by `SceneHost::tick` and `SceneHost::extract` — the
    // production phases, calling the production `regions_of` and `camera_of` —
    // with only the target swapped for `Offscreen`. A test that assembled sprites
    // by hand would bless a picture no runner draws.

    /// The name the drop scene's reference image is blessed under.
    ///
    /// It carries the tick, because the tick is a choice this scene's own
    /// documentation argues for and a reader of `tests/golden/` should not have
    /// to open a source file to learn which frame is blessed.
    const DROP_REFERENCE: &str = "physics_drop_tick45_128x128";

    /// The reference's edge, in pixels. Square, so one number.
    const DROP_EDGE: u32 = 128;

    /// Every region the scene names, with the colour its sprite shows.
    ///
    /// **Five regions for five bodies, no two alike** — `ProjektPlan.md` §9.1's
    /// rule, and here it is what makes a stack legible: where `box_b` rests on
    /// `box_a` the boundary is a colour change, and with one shared texture it
    /// would be nothing at all.
    ///
    /// Opaque, so premultiplication is the identity (ADR-0023) and a region's
    /// texels reach the frame verbatim — which is what lets the probes below
    /// compare against these bytes rather than against a tolerance.
    const DROP_REGIONS: [(&str, [u8; 4]); 5] = [
        ("ground", [64, 64, 72, 255]),
        ("box_a", [220, 60, 60, 255]),
        ("box_b", [60, 190, 90, 255]),
        ("box_c", [70, 120, 240, 255]),
        ("box_d", [235, 200, 60, 255]),
    ];

    /// What the target is cleared to, and therefore what "no sprite" looks like.
    const DROP_BACKGROUND: [u8; 4] = [0, 0, 0, 255];

    /// The scene's atlas, packed by the production packer, and its region table.
    ///
    /// Built from [`SourceRegion::solid`](narvo_assets::SourceRegion::solid)
    /// rather than from PNG files on disk: the scene's `assets/` directory does
    /// not exist in the repository (ADR-0024) and the demo test writes it under
    /// `target/`. What matters for the picture is the *packing* — the M4.4
    /// contract, one texel of duplicated-edge padding per region — and that is
    /// the same call either way.
    ///
    /// A four-by-four source for a sprite drawn eighteen pixels wide, so every
    /// texel is magnified far past one pixel. With a solid colour that changes
    /// nothing: `Linear` between two identical texels returns the texel, and the
    /// padding makes that true at a region's edge too (D13).
    fn drop_atlas() -> (Pixels, std::collections::BTreeMap<String, TextureRegion>) {
        let sources: Vec<narvo_assets::SourceRegion> = DROP_REGIONS
            .iter()
            .map(|(name, colour)| {
                narvo_assets::SourceRegion::solid(*name, 4, 4, *colour)
                    .expect("a four-by-four solid is a valid source region")
            })
            .collect();

        let atlas = narvo_assets::pack(sources).expect("five small regions pack");
        let texture = Pixels::from_rgba8(atlas.width(), atlas.height(), atlas.rgba().to_vec())
            .expect("the packed atlas is a texture");
        let table = atlas
            .regions()
            .map(|(name, place)| {
                (
                    name.to_owned(),
                    TextureRegion::from_texels(
                        place.left(),
                        place.top(),
                        place.width(),
                        place.height(),
                        &texture,
                    ),
                )
            })
            .collect();

        (texture, table)
    }

    /// The drop scene at [`drop_scene::TICK`], drawn through the production host.
    fn render_the_drop_scene(target: OffscreenTarget) -> (Pixels, SceneHost<Offscreen>) {
        use crate::sim::scene_file::drop_scene;
        use narvo_core::frame::FrameHost as _;

        let simulation =
            crate::sim::scene_file::build(&drop_scene::text()).expect("the shipped scene loads");
        let (texture, table) = drop_atlas();

        let mut host = SceneHost::new(simulation, texture, Offscreen::new(target))
            .showing(Regions::ByName(table));

        // One tick per call, exactly as `FrameLoop` hands them out, and the tick
        // budget rather than a clock decides how many: the reference is a picture
        // of tick `TICK` and nothing about frame pacing may reach it.
        for _ in 0..drop_scene::TICK {
            host.tick().expect("a tick cannot fail");
        }

        host.extract().expect("extraction cannot fail");
        assert_eq!(
            host.acquire().expect("an offscreen target always has one"),
            Acquisition::Ready
        );
        host.encode().expect("the draw cannot fail");
        host.present().expect("there is nothing to present to");

        let frame = host
            .target()
            .last_frame()
            .expect("one frame was drawn")
            .clone();

        (frame, host)
    }

    /// Where a world point lands on the frame, in continuous pixel coordinates.
    ///
    /// The inverse of [`Projection::screen_to_world`], written out because the
    /// crate offers only the one direction — and **checked against it** at every
    /// call site below rather than trusted, which is the cheapest way to keep a
    /// second piece of camera arithmetic from disagreeing with the first
    /// (`narvo-render2d`'s own argument for keeping both on `Projection`).
    fn drop_screen_point(camera: CameraView, x: f32, y: f32) -> (f32, f32) {
        let half = DROP_EDGE as f32 / 2.0;
        (
            (x - camera.x) * camera.zoom + half,
            half - (y - camera.y) * camera.zoom,
        )
    }

    /// Every rigid body of a world, in canonical entity order, with its region.
    fn drop_bodies(world: &narvo_ecs::World) -> Vec<(String, narvo_ecs::RigidBody)> {
        world
            .entity_ids()
            .into_iter()
            .filter_map(|entity| {
                let body = *world.get::<narvo_ecs::RigidBody>(entity).ok()?;
                let region = world.get::<narvo_ecs::Sprite>(entity).ok()?.region.clone();
                Some((region, body))
            })
            .collect()
    }

    /// The colour [`DROP_REGIONS`] gives a region name.
    fn drop_colour(region: &str) -> [u8; 4] {
        DROP_REGIONS
            .iter()
            .find(|(name, _)| *name == region)
            .unwrap_or_else(|| panic!("the drop scene names a region {region:?} with no colour"))
            .1
    }

    /// **Every body is drawn where its own scalars say, in its own colour.**
    ///
    /// The seam M5b.4 built, asserted end to end and without a reference image:
    /// a `RigidBody`'s position crosses into a `SpritePlacement`, the placement
    /// becomes four corners, and the corners become pixels. The probe is each
    /// body's *centre*, projected through the camera the frame was drawn with, so
    /// a placement that lost the position, took its size from somewhere else, or
    /// picked up another body's region lands on a different colour.
    ///
    /// It is a prediction about the drawing, not about the simulation: where the
    /// bodies *are* at this tick is rapier's answer and this test takes it as
    /// given. What it predicts is that the picture agrees with it.
    #[test]
    fn every_body_is_drawn_where_its_own_scalars_say() {
        let Some(target) = target_or_skip(DROP_EDGE, DROP_EDGE) else {
            return;
        };

        let (frame, host) = render_the_drop_scene(target);
        let camera = host.camera();
        let projection =
            narvo_render2d::Projection::for_target(DROP_EDGE, DROP_EDGE).viewed_by(camera);

        for (region, body) in drop_bodies(host.world()) {
            let (sx, sy) = drop_screen_point(camera, body.x, body.y);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "every body of this scene is inside the frame, which the \
                          round trip below is what establishes"
            )]
            let (px, py) = (sx as u32, sy as u32);

            // The round trip, which is what makes the line above a use of the
            // production projection rather than a second copy of it.
            let [back_x, back_y] = projection.screen_to_world(
                f32::from(u16::try_from(px).unwrap()) + 0.5,
                f32::from(u16::try_from(py).unwrap()) + 0.5,
            );
            let tolerance = 1.0 / camera.zoom;
            assert!(
                (back_x - body.x).abs() < tolerance && (back_y - body.y).abs() < tolerance,
                "pixel ({px}, {py}) maps back to world ({back_x}, {back_y}), which is \
                 not within one pixel of {region}'s centre ({}, {}); the local \
                 projection and `screen_to_world` disagree",
                body.x,
                body.y
            );

            let actual = frame.pixel(px, py).unwrap_or_else(|| {
                panic!("{region}'s centre is outside the frame at ({px}, {py})")
            });
            let expected = drop_colour(&region);

            println!("  {region:8} centre pixel ({px:3}, {py:3}): {actual:?}");
            assert_eq!(
                actual, expected,
                "the pixel at {region}'s own centre is {actual:?} and not {expected:?}. \
                 Either the body's position did not reach its placement, its extents \
                 did not become the sprite's size, or the sprite is showing another \
                 body's region."
            );
        }
    }

    /// **A turned body is drawn turned**, and this is what says so.
    ///
    /// Every other probe here would pass on a renderer that dropped the rotation
    /// pair entirely: a sprite's centre does not move when it turns. This
    /// measures the **footprint** of one body's colour — how many rows of the
    /// frame it reaches — against the height a level sprite of the same extents
    /// could reach, over the whole frame rather than at a point somebody chose.
    ///
    /// A rectangle of half-extents `(hx, hy)` turned by an angle spans
    /// `2(hx·|sin| + hy·|cos|)` rows, which for this body is about 18.7 pixels
    /// against the 12 a level one spans. The margin below is three pixels, which
    /// is well inside that gap and well outside what an edge could contribute.
    ///
    /// Exact colour equality, deliberately: a solid region under `Linear` with
    /// padding reaches the frame verbatim in its interior, while an edge pixel is
    /// blended with the background by MSAA and will not match. So the footprint
    /// measured here is an **undercount** of the sprite's true one, which makes
    /// the assertion conservative rather than generous.
    #[test]
    fn a_turned_body_covers_more_rows_than_a_level_one_could() {
        let Some(target) = target_or_skip(DROP_EDGE, DROP_EDGE) else {
            return;
        };

        let (frame, host) = render_the_drop_scene(target);

        // The most-turned body of the scene, chosen from the state rather than
        // named: which box turns furthest is the solver's business.
        let (region, body) = drop_bodies(host.world())
            .into_iter()
            .max_by(|left, right| left.1.rot_sin.abs().total_cmp(&right.1.rot_sin.abs()))
            .expect("the scene has bodies");
        let colour = drop_colour(&region);

        let rows: Vec<u32> = (0..frame.height())
            .filter(|py| (0..frame.width()).any(|px| frame.pixel(px, *py) == Some(colour)))
            .collect();
        let first = *rows.first().expect("the body is drawn somewhere");
        let last = *rows.last().expect("the body is drawn somewhere");
        let footprint = last - first + 1;

        // What a level sprite of these extents would span, in pixels, and what
        // the turned one should: `2(hx·|sin| + hy·|cos|)`.
        let level = 2.0 * body.half_y * host.camera().zoom;
        let turned = 2.0
            * (body.half_x * body.rot_sin.abs() + body.half_y * body.rot_cos.abs())
            * host.camera().zoom;

        println!(
            "  {region} at rot_sin {:.4} reaches rows {first}..={last} ({footprint} rows); \
             level would span {level:.2}, turned {turned:.2}",
            body.rot_sin
        );

        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a sprite of a scene this size spans tens of pixels, not billions"
        )]
        let level_rows = level.ceil() as u32;
        assert!(
            footprint >= level_rows + 3,
            "{region} has rot_sin {:.4} and its colour reaches only {footprint} rows, \
             which a level sprite of the same extents could reach on its own \
             ({level_rows}). A renderer that dropped the rotation pair would draw \
             exactly that, and no other probe in this file would notice.",
            body.rot_sin
        );
    }

    /// The blessed reference for the drop scene.
    ///
    /// **Expected to fail until the maintainer blesses it**, as every reference
    /// in this repository was. Nothing here writes one: `Golden::verify`'s own
    /// documentation says the missing-reference case "is never resolved by
    /// writing one here", and §9.1 keeps blessing with the human.
    #[test]
    fn the_drop_scene_matches_its_golden_reference() {
        let Some(target) = target_or_skip(DROP_EDGE, DROP_EDGE) else {
            return;
        };

        let (frame, _host) = render_the_drop_scene(target);

        let references = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
        let output = narvo_render2d::golden_artifact_dir();
        let golden = narvo_render2d::Golden::new(&references, &output);

        match golden.verify(DROP_REFERENCE, &frame) {
            Ok(report) => println!(
                "golden match for {DROP_REFERENCE}: {}",
                report.measured_against(golden.tolerance)
            ),
            Err(error) => panic!("{error}"),
        }
    }

    /// The background is still the background, so the frame is a scene and not a
    /// wall of sprite.
    ///
    /// The counterpart to the probes above: they can all pass on a frame where
    /// one sprite is drawn far too large and happens to cover every probe. This
    /// looks at a corner, which no body of this scene is near.
    #[test]
    fn the_drop_scene_leaves_its_background_alone() {
        let Some(target) = target_or_skip(DROP_EDGE, DROP_EDGE) else {
            return;
        };

        let (frame, _host) = render_the_drop_scene(target);

        for (px, py) in [(2, 2), (DROP_EDGE - 3, 2)] {
            assert_eq!(
                frame.pixel(px, py),
                Some(DROP_BACKGROUND),
                "pixel ({px}, {py}) is not the cleared background, so something in \
                 this scene is drawn far larger than its extents"
            );
        }
    }

    /// Runs one whole frame's phases in the order [`FrameLoop`] runs them.
    ///
    /// Written out rather than driven through `FrameLoop::step` so the tests
    /// below need no clock: what they are about is which phase serves a
    /// capture, and that is this order.
    fn one_frame<T: FrameTarget>(host: &mut SceneHost<T>) {
        host.extract().expect("extraction cannot fail");
        assert!(
            matches!(
                host.acquire().expect("acquiring cannot fail"),
                Acquisition::Ready
            ),
            "these targets always have somewhere to draw"
        );
        host.encode().expect("encoding cannot fail");
        host.present().expect("presenting cannot fail");
    }

    /// **A frame nobody asked about is never read back.**
    ///
    /// The cost, not the correctness: a read-back waits for the GPU, so an
    /// ordinary frame that paid for one would make every window slower for a
    /// convenience almost nobody uses in any given second. Off has to be free,
    /// which is the same property ADR-0039 gives the overlay.
    #[test]
    fn a_frame_nobody_asked_about_is_never_read_back() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        for _ in 0..3 {
            one_frame(&mut host);
        }

        assert_eq!(host.target().draws, 3, "three frames were drawn");
        assert_eq!(
            host.target().captures.get(),
            0,
            "a frame nobody asked to capture must not wait for the GPU"
        );
        assert!(
            host.take_capture().is_none(),
            "and there is nothing to collect"
        );
    }

    /// **One press, one screenshot** — and the next frame costs nothing again.
    #[test]
    fn a_request_is_served_by_the_next_frame_and_by_no_frame_after_it() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        host.request_capture();
        one_frame(&mut host);

        assert_eq!(host.target().captures.get(), 1);
        assert!(
            host.take_capture().is_some(),
            "the frame that served the request must hand a picture over"
        );

        one_frame(&mut host);

        assert_eq!(
            host.target().captures.get(),
            1,
            "a served request must not be served again"
        );
        assert!(host.take_capture().is_none(), "and there is nothing left");
    }

    /// Asking twice before a frame runs is one screenshot, not two.
    ///
    /// A held key repeats. Without this, the answer would be a file per frame
    /// for as long as a finger stayed down.
    #[test]
    fn asking_twice_before_a_frame_produces_one_picture() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting::default(),
        );

        host.request_capture();
        host.request_capture();
        one_frame(&mut host);

        assert_eq!(host.target().captures.get(), 1);
        assert!(host.take_capture().is_some());
        assert!(host.take_capture().is_none(), "one press, one picture");
    }

    /// A request that could not be served is kept rather than dropped.
    #[test]
    fn a_request_that_could_not_be_served_waits_for_a_frame_that_can() {
        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Counting {
                blind: true,
                ..Counting::default()
            },
        );

        host.request_capture();
        one_frame(&mut host);

        assert_eq!(host.target().captures.get(), 1, "it was tried");
        assert!(host.take_capture().is_none(), "and produced nothing");

        one_frame(&mut host);
        assert_eq!(
            host.target().captures.get(),
            2,
            "an unserved request is still a request"
        );
    }

    /// **The screenshot shows what the frame drew, overlay included.**
    ///
    /// M6.4b's red flank (c), and it is a measurement rather than a design goal:
    /// the capture is a copy of the texture `encode` resolved into, so the
    /// overlay is in it exactly when it was in the frame. Nothing in the capture
    /// path consults `inspector.on`, and nothing should — this asserts that the
    /// picture follows the frame rather than a second opinion about it.
    ///
    /// It also pins the *moment*. The first frame is drawn without the overlay
    /// and without a request; the overlay is then switched on and a capture
    /// asked for. A capture served anywhere before `encode` would hand back the
    /// first frame, and the first assertion below is what that fails on.
    #[test]
    fn the_screenshot_carries_the_overlay_exactly_when_the_frame_did() {
        let Some(target) = target_or_skip(256, 256) else {
            return;
        };

        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Offscreen::new(target),
        );

        // A frame with the overlay off, captured.
        host.request_capture();
        one_frame(&mut host);
        let plain = host.take_capture().expect("the request was served");

        // The same scene with the overlay on, captured.
        host.toggle_inspector();
        host.request_capture();
        one_frame(&mut host);
        let overlaid = host.take_capture().expect("the request was served");

        assert_eq!(
            (plain.width(), plain.height()),
            (overlaid.width(), overlaid.height())
        );
        assert_ne!(
            plain.rgba(),
            overlaid.rgba(),
            "the screenshot did not follow the overlay - either it was taken \
             before the frame was encoded, or it came from somewhere other than \
             the frame"
        );

        // **From the glyph atlas**, by the same argument
        // `the_overlay_reaches_the_frame_and_comes_from_the_glyph_atlas` makes:
        // the rasteriser writes `[value; 4]`, so a covered glyph texel is white,
        // and the scene atlas' brightest pixel is `[255, 0, 0, 255]`.
        assert!(
            (0..256)
                .flat_map(|y| (0..256).map(move |x| (x, y)))
                .any(|(x, y)| overlaid.pixel(x, y) == Some([255, 255, 255, 255])
                    && plain.pixel(x, y) != Some([255, 255, 255, 255])),
            "no pixel of the screenshot turned white, so what it shows is not \
             the overlay that was switched on"
        );

        // And the picture is the frame's, not a fresh render of an empty scene:
        // something is drawn in both.
        assert!(
            plain
                .rgba()
                .chunks_exact(4)
                .any(|texel| texel != [0, 0, 0, 255]),
            "the screenshot with no overlay is a blank frame, so it is not the \
             scene that was drawn"
        );
    }

    /// The screenshot is the frame, byte for byte.
    ///
    /// Tautological for this target — `Offscreen::capture` hands back the frame
    /// it just read — and that is exactly what makes it worth writing down: it
    /// is the property the *window* has by copying the swapchain image rather
    /// than re-rendering, stated where a machine can hold it. The day somebody
    /// makes a capture re-render the scene, this is what says the two are no
    /// longer the same picture.
    #[test]
    fn the_screenshot_is_the_frame_that_was_drawn() {
        let Some(target) = target_or_skip(64, 64) else {
            return;
        };

        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Offscreen::new(target),
        );

        host.request_capture();
        one_frame(&mut host);

        let captured = host.take_capture().expect("the request was served");
        let drawn = host.target().last_frame().expect("a frame was drawn");

        assert_eq!(captured.rgba(), drawn.rgba());
    }

    /// Where two pictures of one size first part, and how much of them does.
    ///
    /// **A diagnostic, and it exists because the red flank produced an
    /// unreadable one.** `assert_eq!` over two frames prints both buffers: at
    /// 64 by 64 that is 16 384 bytes a side on a single line, and a reader
    /// learns nothing from it. It is the same failure ADR-0044 measured at
    /// 81 723 bytes and rejected a whole design over, met again in a test's own
    /// output.
    ///
    /// `None` means the two are identical. Otherwise the count of differing
    /// pixels and the first of them, which is what says whether a capture came
    /// from another moment (most pixels moved) or a copy lost a row (a band
    /// starting at a known y).
    fn first_difference(left: &Pixels, right: &Pixels) -> Option<String> {
        assert_eq!(
            (left.width(), left.height()),
            (right.width(), right.height()),
            "two pictures of different sizes cannot be compared pixel by pixel"
        );

        let pairs = || {
            left.rgba()
                .chunks_exact(4)
                .zip(right.rgba().chunks_exact(4))
        };
        let differing = pairs().filter(|(left, right)| left != right).count();
        let (index, (found, wanted)) = pairs()
            .enumerate()
            .find(|(_, (left, right))| left != right)?;

        let index = u32::try_from(index).expect("a 64 by 64 frame has 4 096 pixels");
        let (x, y) = (index % left.width(), index / left.width());
        let total = left.width() * left.height();

        Some(format!(
            "{differing} of {total} pixels differ; the first is ({x}, {y}), \
             {found:?} against {wanted:?}"
        ))
    }

    /// How far the world is moved between an encode and the capture it serves.
    ///
    /// Not a fitted number. The scene's camera follows a wandering target, so
    /// the picture moves with the tick count rather than jumping at a
    /// threshold, and what the guard below needs is only that A and B are
    /// *different* — which it asserts rather than assumes. Raising this cannot
    /// make the guard pass for a wrong reason; lowering it to zero would make it
    /// vacuous, and the second assertion is what says so.
    const MOVED_TICKS: u32 = 240;

    /// **The picture is the frame that was encoded, not a rendering of whatever
    /// the host holds when the capture is served.**
    ///
    /// ADR-0040's origin axis, stated where a machine can fail on it. Until this
    /// existed the axis was held by nothing:
    /// `the_screenshot_is_the_frame_that_was_drawn` says so in its own doc
    /// comment ("Tautological for this target"), and it is — it compares the
    /// capture with `Offscreen::last_frame`, which is the same field
    /// `Offscreen::capture` hands back. A capture that re-rendered the scene
    /// into that same target would move both sides together and pass.
    ///
    /// # Why moving the world is the discriminator
    ///
    /// A second render fed *identical* inputs is byte-identical to a copy, and
    /// no test anywhere can tell those two apart — that is what ADR-0040 means
    /// by "agree only by an argument about the inputs being equal". What a test
    /// can tell apart is a second render whose inputs have moved, and that is
    /// the realistic defect: the capture is served in `present`, several phases
    /// after the encode, and a re-render there reads `self.sprites` and
    /// `self.camera` as they are *then*.
    ///
    /// So the frame is driven as far as the encode, and then everything a second
    /// render would read is moved: the world is ticked on and the sprite buffer
    /// re-extracted from it. A copy of the texture cannot be anything but the
    /// encoded frame; a re-render at this point produces the moved state and
    /// fails on the first assertion.
    ///
    /// The extra `extract` is deliberate and is not the order `FrameLoop` runs —
    /// `one_frame` above already writes the phases out by hand for the same
    /// reason. It is what makes the two implementations distinguishable at all.
    ///
    /// # What it does not reach
    ///
    /// A window. `Offscreen::capture` returns the picture its `draw` already read
    /// back, so what runs here is the host's wiring rather than the copy itself;
    /// the copy is `read_back_texture`'s, shared with the window path and covered
    /// by `narvo-render2d`'s own tests. And that the copied bytes are the bytes
    /// a compositor put on a screen is below what anything here can observe,
    /// which ADR-0040 says of itself.
    #[test]
    fn a_capture_is_the_encoded_frame_and_not_the_state_it_is_served_in() {
        let Some(target) = target_or_skip(64, 64) else {
            return;
        };

        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Offscreen::new(target),
        );

        // One frame as far as the encode. State A is on the GPU from here on.
        host.request_capture();
        host.extract().expect("extraction cannot fail");
        assert!(
            matches!(
                host.acquire().expect("acquiring cannot fail"),
                Acquisition::Ready
            ),
            "this target always has somewhere to draw"
        );
        host.encode().expect("encoding cannot fail");
        let encoded = host
            .target()
            .last_frame()
            .cloned()
            .expect("the encode drew a frame");

        // Everything a second render would read now says B.
        for _ in 0..MOVED_TICKS {
            host.tick().expect("a tick of the scene cannot fail");
        }
        host.extract().expect("extraction cannot fail");

        host.present().expect("presenting cannot fail");
        let captured = host.take_capture().expect("the request was served");

        if let Some(parting) = first_difference(&captured, &encoded) {
            panic!(
                "the capture is not the frame that was encoded, so it was \
                 produced from the host's state at the moment it was served \
                 rather than copied from the texture the encode resolved into \
                 — {parting}"
            );
        }

        // And not vacuously: the state really did move, so the assertion above
        // was between two different pictures rather than between one picture and
        // itself.
        host.request_capture();
        one_frame(&mut host);
        let later = host.take_capture().expect("the second request was served");
        assert!(
            first_difference(&encoded, &later).is_some(),
            "{MOVED_TICKS} ticks did not change what the scene draws, so the \
             comparison above proves nothing about which moment the capture came \
             from"
        );
    }

    /// **Asking for a screenshot changes no pixel of the frame.**
    ///
    /// ADR-0040's fourth axis — "nothing is drawn differently because a capture
    /// was asked for" — on the *picture* rather than on the cost.
    /// `a_frame_nobody_asked_about_is_never_read_back` holds the cost half, and
    /// it is a count: it says a read-back did not happen, not that the frame
    /// that was drawn is the same one.
    ///
    /// Two frames of one unticked world, so the drawn state is identical by
    /// construction and the only difference between them is the request. Byte
    /// equality rather than a tolerance, for the reason
    /// `a_bgra_frame_normalises_to_exactly_what_the_rgba_path_renders` gives:
    /// this is one device running the same arithmetic twice, not two rasterisers
    /// being compared.
    ///
    /// What it catches that the count cannot: a capture that renders into the
    /// target it was supposed to copy from — the shortcut a session reaching for
    /// "we do not need `COPY_SRC` after all" would take — and any frame that
    /// draws a marker, drops the overlay, or changes a clear colour because a
    /// picture was wanted.
    #[test]
    fn asking_for_a_screenshot_changes_no_pixel_of_the_frame() {
        let Some(target) = target_or_skip(64, 64) else {
            return;
        };

        let simulation = scene::build_with(4).expect("the scene builds");
        let mut host = SceneHost::new(
            simulation,
            crate::content::demo_texture(),
            Offscreen::new(target),
        );

        // A frame nobody asked about.
        one_frame(&mut host);
        let unasked = host
            .target()
            .last_frame()
            .cloned()
            .expect("the first frame was drawn");
        assert!(
            host.take_capture().is_none(),
            "nobody asked, so nothing may have been read back"
        );

        // The same world, drawn again, with a request standing.
        host.request_capture();
        one_frame(&mut host);
        let asked = host
            .target()
            .last_frame()
            .cloned()
            .expect("the second frame was drawn");
        let captured = host.take_capture().expect("the request was served");

        if let Some(parting) = first_difference(&asked, &unasked) {
            panic!(
                "the frame drawn while a screenshot was pending differs from \
                 the frame drawn without one, so asking for a picture changed \
                 what the window would have shown — {parting}"
            );
        }
        if let Some(parting) = first_difference(&captured, &asked) {
            panic!(
                "the picture handed over is not the frame that was drawn \
                 beside it — {parting}"
            );
        }

        // And not vacuously: an all-black frame would satisfy both equalities
        // whatever the capture did.
        assert!(
            unasked
                .rgba()
                .chunks_exact(4)
                .any(|texel| texel != [0, 0, 0, 255]),
            "the frames compared above are blank, so the equalities hold for a \
             reason that has nothing to do with the capture"
        );
    }
}
