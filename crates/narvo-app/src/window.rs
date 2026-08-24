//! The windowed runner: a window, and a frame loop driving a world into it.
//!
//! Deliberately thin, and thinner than it looks. What a frame *is* — every due
//! tick, then one extraction, then at most one drawing — belongs to
//! [`narvo_core::frame::FrameLoop`] and is covered by that crate's tests
//! without a GPU. What a frame's parts *do* belongs to [`crate::frame`]. What is
//! left here is what only a window has: opening one, forwarding four events, and
//! calling the loop once per redraw.
//!
//! **The whole frame runs inside one winit callback.** Spreading the phases
//! across several would put the ordering back into this file, where nothing can
//! test it; keeping it in one call leaves the order where `FrameLoop` asserts
//! it.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use narvo_audio::Sink as _;
use narvo_core::FixedTimestep;
use narvo_core::frame::{FrameLog, FrameLoop, FramePhase};
use narvo_render2d::{PresentPolicy, WindowTarget};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use narvo_input::Mapping;
use narvo_render2d::Projection;

use crate::frame::{Monotonic, Regions, SceneHost, Windowed};
use crate::input::{self, InputFeed};
use crate::screenshot;
use crate::sim::{scene, scene_file};
use crate::watch::Watcher;

/// How long an `acquire` has to take before `--resize-probe` names the frame.
///
/// Two orders of magnitude above an ordinary one and two below the block being
/// looked for: M7.1d measured an unblocked `acquire` at a 6 ms maximum and a
/// blocked one at about 2 000 ms, so anything in between is a gap nothing lands
/// in and the threshold does not have to be argued about.
const PROBE_BLOCKED: Duration = Duration::from_millis(100);

/// The size series `--resize-probe` walks: `(after this many frames, w, h)`.
///
/// Four changes of growing magnitude, the first of them four pixels.
///
/// **None of the four covers the screen, and that is what they are for.** M7.1d
/// measured that these four cost nothing at all on this machine — the block
/// belongs to [`PROBE_MAXIMISE`] below — so they are the negative half of the
/// measurement and are kept for exactly that. A series that only maximised could
/// not tell "a resize is expensive" from "*this* resize is expensive".
///
/// This corrects M7.1c, which recorded that a four-pixel change costs what a
/// maximise costs. That reading came from a run in which the window was already
/// maximised when it opened, so every step in the series crossed the boundary
/// this series is now built to stay inside of.
const RESIZE_PROBE: [(usize, u32, u32); 4] = [
    (120, 1284, 701),
    (240, 1584, 861),
    (360, 1264, 681),
    (480, 1600, 900),
];

/// The frame after which `--resize-probe` maximises the window.
///
/// Separate from [`RESIZE_PROBE`] and not a fifth row of it, because it is not a
/// size: a size in a table is a number somebody chose, and what this has to do
/// is cover *whatever* screen the run is on. `set_maximized` asks the window
/// manager, so the probe reproduces what a hand does and stays true on a display
/// this machine does not have.
const PROBE_MAXIMISE: usize = 600;

/// Window size the runner asks for, in logical pixels.
///
/// 1280 by 720 rather than the old 640 square: a scene measured at one size and
/// shown at another is two measurements, and this is the size
/// `docs/perf/BASELINE.md` records the frame times at.
const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 720.0;

/// What a windowed run is for.
#[derive(Debug, Clone)]
pub struct RunPlan {
    /// The scene file to constitute the world from, and watch.
    ///
    /// `None` is the sprite-field demo, which is what the window has always
    /// shown. `Some` is M4.6's scene-file mode: the world comes from the file,
    /// and the file is watched for changes (ADR-0022).
    pub scene: Option<PathBuf>,
    /// Sprites in the scene.
    pub sprites: usize,
    /// Stop after this many measured frames, or run until the window closes.
    pub frames: Option<usize>,
    /// Frames to discard before measuring.
    pub warmup: usize,
    /// Whether to take a present mode that does not wait for the display.
    pub uncapped: bool,
    /// Whether to walk [`RESIZE_PROBE`]'s size series as the run goes.
    ///
    /// **An instrument, not a feature** (M7.1c). What it does is ask the window
    /// for four sizes at fixed frame counts, so the `acquire` row of the report
    /// below is measured against a surface reconfigure rather than against a
    /// still window — the case nobody had measured through M1 to M6b.
    pub resize_probe: bool,
    /// Whether to block on the GPU each frame.
    pub drain: bool,
    /// The mapping the keyboard is bound through, when one was given.
    ///
    /// `None` is the window as it behaved before M5.3: no key produces an input
    /// event and only the built-in Escape does anything. There is no default
    /// mapping, deliberately.
    pub mapping: Option<Mapping>,
}

impl RunPlan {
    /// The plan an ordinary `narvo` window runs: the scene, vsync, no end.
    #[must_use]
    pub fn demo(sprites: usize) -> Self {
        Self {
            scene: None,
            sprites,
            frames: None,
            warmup: 0,
            uncapped: false,
            resize_probe: false,
            drain: false,
            mapping: None,
        }
    }
}

impl RunPlan {
    /// A plan that constitutes its world from `path` and watches it.
    ///
    /// The read here is a pre-flight and nothing else: it exists so that a path
    /// that is wrong is reported on the terminal the command was typed into,
    /// rather than by a window that opens and closes again. The text it returns
    /// is thrown away, and the world is built from the read in `resumed` — with
    /// the watcher started from *that* text, so what the window shows and what
    /// the watcher compares against are the same bytes.
    ///
    /// # Errors
    ///
    /// A message naming the path, when the scene cannot be read.
    pub fn from_scene(path: &Path) -> Result<Self, String> {
        // Read directly rather than through `Anchor::read`. An anchor is a
        // *recording's* way of naming the file it was made against (ADR-0019),
        // and it refuses an absolute path so that a recording travels between
        // machines. A window records nothing, so that rule would only forbid a
        // perfectly ordinary way to point at a scene. ADR-0022 records the
        // division: the anchor belongs to the band, the watcher to the window.
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;

        Ok(Self {
            scene: Some(path.to_path_buf()),
            frames: None,
            warmup: 0,
            sprites: 0,
            uncapped: false,
            resize_probe: false,
            drain: false,
            mapping: None,
        })
    }
}

/// What a finished run measured.
#[derive(Debug)]
pub struct RunReport {
    /// Every measured frame, warm-up already discarded.
    pub log: FrameLog,
    /// The present mode the surface actually took.
    pub present_mode: &'static str,
    /// The adapter the frames were drawn on.
    pub adapter: String,
    /// Sprites the scene drew.
    pub sprites: usize,
    /// Frames discarded before measuring.
    pub warmup: usize,
    /// Whether the GPU was drained every frame.
    pub drained: bool,
}

/// Opens a window, runs `plan`, and returns what it measured.
///
/// # Errors
///
/// If the event loop cannot start, the window cannot be created, or the renderer
/// cannot use it. A failure inside the loop stops it and is returned afterwards,
/// because winit's callbacks cannot return one themselves.
pub fn run(plan: RunPlan) -> Result<Option<RunReport>, Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    // **Poll, not Wait.** What actually produces frames is the `request_redraw`
    // at the end of every one, not the control flow: `draw` runs only on
    // `WindowEvent::RedrawRequested`, `resumed` asks for the first, and each
    // frame asks for the next.
    //
    // `Wait` would very likely work too. `Poll` is taken anyway because it
    // starts the next iteration without suspending, leaving the frame rate to
    // the loop and the present mode rather than to the windowing system's idea
    // of when a paint is due. Whether any backend *needs* it is **uncertain**:
    // nothing here measures a `Wait` run, and the window path has no automated
    // coverage by design.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut runner = Runner {
        plan,
        window: None,
        state: None,
        failure: None,
        report: None,
    };
    event_loop.run_app(&mut runner)?;

    match runner.failure {
        Some(failure) => Err(failure.into()),
        None => Ok(runner.report),
    }
}

/// Everything that exists only once the window does.
struct Live {
    frames: FrameLoop,
    clock: Monotonic,
    host: SceneHost<Windowed>,
    /// The scene file being watched, when the world came from one.
    watcher: Option<Watcher>,
    /// The keyboard, when `--mapping` gave it one.
    feed: Option<InputFeed>,
    /// The counter values last reported, so a change is logged once rather than
    /// once per frame.
    ///
    /// Window-only. The headless runner writes one report to stdout and nothing
    /// else — `run_headless`'s own documentation is explicit that a progress
    /// line there would break `narvo --hash > run.txt` as a comparison — so
    /// this snapshot and the line it drives exist on this side of the runner
    /// only.
    tallies: std::collections::BTreeMap<narvo_ecs::EntityId, i64>,
    /// Where the pointer is, and what a press there means.
    ///
    /// Since M6b.8 a type in [`crate::input`] rather than a bare `Option` here
    /// (D23, Weg C). What moved is the composition; what stayed is the winit
    /// event arm below, which is the one place a `CursorMoved` is unwrapped.
    pointer: input::Pointer,
    /// Where the cues go, when this machine has an output to send them to.
    ///
    /// `None` is an ordinary outcome rather than a failure: cpal's
    /// `default_output_device` returns an `Option` and `AudioManager::new`
    /// returns a `Result` (ADR-0028), so a machine with no sound card runs the
    /// demo silently. The note beside each cue is still printed either way,
    /// which is what keeps the hearing check meaningful — a human can see that
    /// a cue was issued even when nothing came out.
    sink: Option<narvo_audio::kira_sink::KiraSink>,
    /// The sounds this run can play, and the only thing that names one.
    ///
    /// **Beside the sink rather than inside it**, because the note below is
    /// printed whether or not a device opened and it needs the library to turn a
    /// handle back into `click`. A sink that owned the library would leave the
    /// silent path with nothing to name a cue with, which is exactly the path the
    /// hearing check depends on.
    library: narvo_audio::SoundLibrary,
    log: FrameLog,
    /// Frames still to be discarded before measuring starts.
    warmup_left: usize,
    /// Measured frames still wanted, or `None` to run until the window closes.
    wanted: Option<usize>,
}

/// Window, renderer and the run in progress.
struct Runner {
    plan: RunPlan,
    window: Option<Arc<Window>>,
    state: Option<Live>,
    /// Set when something failed inside a callback, which cannot return an
    /// error. [`run`] turns it into one after the loop has stopped.
    failure: Option<String>,
    /// Filled when the run finished on its own terms rather than being closed.
    report: Option<RunReport>,
}

impl Runner {
    /// Records a failure and asks the loop to stop.
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: String) {
        self.failure = Some(message);
        event_loop.exit();
    }
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `resumed` fires again on platforms that suspend; the window is built
        // once.
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title("Narvo")
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT));

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("the window could not be opened: {error}"),
                );
                return;
            }
        };

        let size = window.inner_size();
        let policy = if self.plan.uncapped {
            PresentPolicy::Uncapped
        } else {
            PresentPolicy::VSync
        };

        let target = match WindowTarget::with_present_policy(
            Arc::clone(&window),
            size.width,
            size.height,
            policy,
        ) {
            Ok(target) => target,
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("the renderer could not use the window: {error}"),
                );
                return;
            }
        };

        // Two ways to get a world, and only one of them watches a file or has an
        // atlas of its own.
        let (simulation, regions, texture, watcher) = match &self.plan.scene {
            None => match scene::build_with(self.plan.sprites) {
                Ok(simulation) => (
                    simulation,
                    Regions::WholeTexture,
                    crate::content::demo_texture(),
                    None,
                ),
                Err(error) => {
                    self.fail(event_loop, format!("the scene could not be built: {error}"));
                    return;
                }
            },
            Some(path) => {
                let text = match std::fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(error) => {
                        self.fail(event_loop, format!("{}: {error}", path.display()));
                        return;
                    }
                };
                let simulation = match scene_file::build(&text) {
                    Ok(simulation) => simulation,
                    Err(error) => {
                        self.fail(event_loop, format!("{error}"));
                        return;
                    }
                };

                // The assets, and the check that every region a sprite names
                // exists — here, before a window opens, so an unknown name is a
                // sentence on the terminal rather than a sprite that quietly
                // does not draw (ADR-0024).
                //
                // Two crates, and the split is M6b.2a's: *which* directory is
                // this runner's convention and stays here; packing it and
                // resolving the names is every game's and lives in the seam.
                let directory = crate::assets::directory_for(path);
                let atlas = match narvo_view2d::load_for(&simulation.world, &directory) {
                    Ok(atlas) => atlas,
                    Err(error) => {
                        self.fail(event_loop, format!("{error}"));
                        return;
                    }
                };

                let watcher = Watcher::new(path, &text, Instant::now());
                println!(
                    "watching {} for changes; edit it and the window reloads",
                    path.display()
                );
                println!(
                    "{} region(s) packed from {}",
                    atlas.regions.len(),
                    directory.display()
                );

                (
                    simulation,
                    Regions::ByName(atlas.regions),
                    atlas.texture,
                    Some(watcher),
                )
            }
        };

        let windowed = Windowed::new(target);
        println!("adapter in use: {}", windowed.adapter_summary());
        // The one number the click path assumes and nothing measured until now:
        // winit reports the cursor in physical pixels and the projection is
        // built from the target's physical extent, so the two agree whatever
        // this is — but that had only ever been exercised at 1.0. Printing it
        // makes the assumption checkable by whoever runs the window.
        println!(
            "scale factor: {}, surface {}x{} physical",
            window.scale_factor(),
            size.width,
            size.height
        );
        // "requested", not "sprites", and the word is the whole M5.7 fix. This
        // prints `plan.sprites`, which is what the *command line* asked the
        // sprite-field demo to build — `scene::build_with(self.plan.sprites)` is
        // its only consumer. A `--scene` run never reaches that builder and
        // `RunPlan::from_scene` leaves the field at 0, so the old wording said
        // "0 sprites" about a window that was drawing two of them. True about
        // the plan, false about the run.
        //
        // The run's actual count is not available here: the host is built a few
        // lines below and its sprite buffer stays empty until the first
        // extraction. Printing it would mean moving this line past the first
        // frame, which is a behaviour path rather than a label.
        println!(
            "present mode: {}, {} sprites requested",
            windowed.present_mode(),
            self.plan.sprites
        );
        if self.plan.frames.is_none() {
            println!("close the window or press Escape to quit");
        }

        // The whole simulation, not two of its three fields. Until M6.6a this
        // line read `simulation.world, simulation.scheduler` and the registry
        // the scene had just been read against was dropped here — the window
        // had no way back to the names of its own components. Handing the
        // struct over is what keeps them, and it is also what makes the
        // alternative unspellable: there is no second place to get a registry
        // from.
        let host = SceneHost::new(simulation, texture, windowed).showing(regions);

        if self.plan.mapping.is_some() {
            println!("keyboard bound through the mapping given with --mapping");
        }

        // One tick's worth of wall time, which is the only number the backend
        // needs to turn a cue's `tween_ticks` into a duration.
        let timestep = FixedTimestep::default();

        // The demo plays only the three sounds this build synthesises, so it
        // registers nothing of its own. A game would register here, before the
        // sink is built, and hand out the handles its systems put in cues.
        let library = narvo_audio::SoundLibrary::new();

        let sink = match narvo_audio::kira_sink::KiraSink::new(timestep.step(), &library) {
            Ok(sink) => {
                println!("audio: output opened, one track per channel");
                Some(sink)
            }
            Err(error) => {
                // Not a failure of the run. The window opens, the demo works,
                // and the cue lines below still say what would have been heard.
                println!("audio: running silently, {error}");
                None
            }
        };

        self.state = Some(Live {
            watcher,
            feed: self.plan.mapping.clone().map(InputFeed::new),
            tallies: std::collections::BTreeMap::new(),
            pointer: input::Pointer::new(),
            sink,
            library,
            frames: FrameLoop::new(timestep),
            clock: Monotonic::new(),
            host,
            log: FrameLog::with_capacity(self.plan.frames.unwrap_or(0)),
            warmup_left: self.plan.warmup,
            wanted: self.plan.frames,
        });

        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::KeyboardInput { event, .. } => {
                // Escape closes the window and is *not* routed through the
                // mapping. It is the runner's own key, not the game's: a
                // mapping that bound Escape to an action would otherwise take
                // away the only way to close a window that is not responding,
                // and a binding cannot be allowed to do that.
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape)
                {
                    event_loop.exit();
                    return;
                }

                // **F1 is the runner's second own key**, and it is here for the
                // reason Escape is: a debug overlay belongs to whoever is
                // running the engine, not to the game. Routing it through the
                // mapping would let a binding take it away, and a mapping that
                // bound F1 to an action would then have two meanings for one
                // press — the third-sha256 shape, in a keyboard.
                //
                // Off until asked for. An overlay that drew by default would
                // cost every user who never wanted it; since ADR-0039 an empty
                // second batch produces nothing at all, so "off" is free rather
                // than merely cheap.
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::F1)
                    && let Some(state) = self.state.as_mut()
                {
                    let on = state.host.toggle_inspector();
                    println!("inspector: {}", if on { "on" } else { "off" });
                    return;
                }

                // F2 steps to the next entity while the overlay is up. Ignored
                // when it is down, so the key does nothing surprising off screen.
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::F2)
                    && let Some(state) = self.state.as_mut()
                    && state.host.inspector_on()
                {
                    state.host.inspect_next();
                    return;
                }

                // **F3 is the runner's fourth own key**, for the reason Escape,
                // F1 and F2 are: a screenshot belongs to whoever is running the
                // engine, not to the game, and a mapping that bound F3 to an
                // action would otherwise give one press two meanings.
                //
                // The press only *asks*. The picture is read back at the end of
                // the frame that draws next, because that is the one moment the
                // image the window is about to show can still be copied, and it
                // is collected in `draw` below.
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::F3)
                    && let Some(state) = self.state.as_mut()
                {
                    state.host.request_capture();
                    println!("screenshot: requested, written after the next frame");
                    return;
                }

                // The one translation boundary. Everything past it is
                // `narvo-input`'s vocabulary, and everything before it stays
                // in this match arm.
                if let Some(state) = self.state.as_mut()
                    && let Some(feed) = state.feed.as_mut()
                    && let Some(device) = input::translate(&event)
                {
                    feed.push(device);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                if let Some(state) = self.state.as_mut() {
                    // The unwrapping of a winit type, and the whole of what is
                    // left here after D23. What "remembered, never acted on"
                    // means is documented on `Pointer::moved_to`, where a test
                    // can reach it.
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "winit reports a physical pixel; f64 to f32 is the \
                                  conversion this boundary has always made"
                    )]
                    state.pointer.moved_to(position.x as f32, position.y as f32);
                }
            }

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.click(),

            WindowEvent::Resized(size) => {
                if let Some(state) = self.state.as_mut() {
                    state.host.target_mut().resize(size.width, size.height);
                    // The probe's answer is worth nothing without this line. A
                    // window may be *asked* for a size and not get it — a tiling
                    // compositor refuses outright — and a refused resize is
                    // indistinguishable from a free one in the table below. So
                    // the probe says what the surface actually became, and a run
                    // that shows no such line reconfigured nothing (M7.1c).
                    if self.plan.resize_probe {
                        let (width, height) = state.host.target().size();
                        println!("resize probe: the surface is now {width}x{height}");
                    }
                }
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => self.draw(event_loop),

            _ => {}
        }
    }
}

impl Runner {
    /// Turns the last known pointer position into an action, if it hit anything.
    ///
    /// **The resolution itself is [`input::Pointer::press`]** since M6b.8, and
    /// what is left here is this runner's own three decisions: which projection
    /// the press is answered through, that a window with no `--mapping` has no
    /// queue to put the result in, and what to print when the rectangle names an
    /// action the vocabulary refuses. D23's Weg C is exactly that split — the
    /// part a test can drive moved to where the tests are.
    ///
    /// **The click itself never reaches a recording** — only the action does.
    /// That is ADR-0012's M5.2 amendment applied to a second device: a repro
    /// says `buy 1`, not "the left button went down at (412, 233)", and it
    /// therefore survives a window of a different size.
    ///
    /// A click that hits nothing produces nothing. A window with no `--mapping`
    /// still has no feed, so it produces nothing either — hit testing is not
    /// switched on by a mapping, but the queue that carries its result is.
    fn click(&mut self) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let (width, height) = state.host.target().size();
        let projection = Projection::for_target(width, height).viewed_by(state.host.camera());
        let resolved = state.pointer.press(state.host.world(), projection);

        let Some(feed) = state.feed.as_mut() else {
            return;
        };
        match resolved {
            Some(Ok(event)) => feed.push_event(event),
            Some(Err(error)) => {
                eprintln!("the hit rectangle carries an unusable action: {error}");
            }
            None => {}
        }
    }

    /// Reloads the scene if its file has settled on new contents.
    ///
    /// # A failure keeps the old world
    ///
    /// `ProjektPlan.md` §7 is explicit that a component may produce a wrong
    /// picture and must never stop the simulation, and a half-saved scene is the
    /// ordinary case rather than the exceptional one. So a scene that does not
    /// load is *logged, with the position M4.2 gives it*, and the world that was
    /// already running keeps running — unchanged, not half-swapped. The next
    /// change tries again.
    ///
    /// # The return value
    ///
    /// `true` exactly when the world was swapped, which is what tells the caller
    /// to throw an undelivered input queue away. A refused reload returns
    /// `false`, and rightly: the world that was running is still running, so
    /// input aimed at it is still aimed at it.
    fn reload_if_changed(state: &mut Live) -> bool {
        let Some(watcher) = state.watcher.as_mut() else {
            return false;
        };
        let Some(change) = watcher.changed(Instant::now()) else {
            return false;
        };

        match scene_file::build(&change.text) {
            Ok(simulation) => {
                let swapped = Instant::now();
                // The reloaded scene's own registry goes in with its world.
                // A reload is where "the same registry" could quietly stop
                // being true — the host would keep naming the components of a
                // world it is no longer running — so the swap moves all three
                // together, exactly as the construction above does.
                state.host.replace_world(simulation);
                let wall = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |since| since.as_secs_f64());
                println!(
                    "reload: swapped at {wall:.3} ({:.0} ms after the change was noticed) — {}",
                    swapped.duration_since(change.noticed).as_secs_f64() * 1000.0,
                    watcher.path().display()
                );
                true
            }
            Err(error) => {
                // Accepted anyway: this exact text has been judged and found
                // wanting, so it is not worth judging again every 200 ms. The
                // next *different* text is.
                watcher.accept(&change.text);
                eprintln!("reload refused, the running world is unchanged: {error}");
                false
            }
        }
    }

    /// One frame, start to finish, inside one callback.
    fn draw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        // **The tick boundary.** A frame owes zero or more ticks and runs them
        // inside `step`, so here — before it — is between the last tick of the
        // previous frame and the first of this one. The swap is one assignment
        // and cannot be seen half-done (ADR-0022).
        let reloaded = Self::reload_if_changed(state);

        // Input goes in at the same seam, and after the reload rather than
        // before it, so the two decisions compose in the one order that is
        // defensible: a swap throws the queue away (ADR-0022 — a reconstituted
        // world is a new run), and what arrives afterwards belongs to the world
        // that is now running. Delivering first would put a keystroke meant for
        // the old world into the new one at a tick number that has just been
        // reset.
        if let Some(feed) = state.feed.as_mut() {
            if reloaded {
                feed.discard();
            }
            feed.deliver(state.host.world_mut());
        }

        let outcome = state.frames.step(&mut state.clock, &mut state.host);

        let record = match outcome {
            Ok(record) => record,
            Err(error) => {
                self.fail(event_loop, format!("a frame failed: {error}"));
                return;
            }
        };

        if self.plan.drain
            && let Err(error) = state.host.target().drain()
        {
            self.fail(event_loop, format!("waiting for the GPU failed: {error}"));
            return;
        }

        // The screenshot F3 asked for, if the frame just drawn produced one.
        //
        // **The file is written here rather than in the host**, which read the
        // bytes back and stopped: a render path that could open a file would be
        // a render path with a filesystem in it. The line printed names the path
        // `write_window_capture` returned — not one recomputed beside it, which
        // is how a message comes to describe a file that was never written.
        //
        // A failure is loud and does not end the run. A screenshot is a
        // convenience; the window, the world and the frame loop are not, and
        // killing them over an unwritable directory would be the convenience
        // costing the thing it was convenient for. That is the same call
        // `resumed` makes when the audio device will not open.
        if let Some(pixels) = state.host.take_capture() {
            match Self::write_capture(&pixels) {
                Ok(path) => println!("screenshot: written to {}", path.display()),
                Err(error) => println!("screenshot: not written, {error}"),
            }
        }

        // What the ticks just did to the counters, one line per change rather
        // than one per frame. The window cannot draw a number yet, so this is
        // the only place a click is observable at all — named as a surface in
        // the M5.5 report rather than left as a gap somebody trips over.
        for (action, count) in scene_file::tally_changes(state.host.world(), &mut state.tallies) {
            println!("{action}: {count}");
        }

        // The cues the ticks just produced, in the order they produced them.
        //
        // The *list* was decided per tick inside `SceneHost::tick`; this is only
        // its delivery, which is why draining it once per frame does not make it
        // frame-rate dependent. One line per cue, and the line is printed
        // whether or not there is a device to play it on: it is the bridge the
        // hearing check compares against, so it has to exist even on a machine
        // that stays silent.
        for cue in state.host.take_cues() {
            println!("{}", cue.note(&state.library));
            if let Some(sink) = state.sink.as_mut() {
                sink.submit(&cue, &state.library);
            }
        }

        // Which frames blocked, and for how long. The summary table at the end
        // gives a p99 and a max and cannot say *where* they fell — and the
        // question this instrument exists for is per resize, not per run. So the
        // probe names each frame whose acquire crossed the threshold, and the
        // lines interleave with the surface-size lines above them (M7.1d).
        if self.plan.resize_probe {
            let acquire = record.phase(FramePhase::Acquire);
            if acquire >= PROBE_BLOCKED {
                println!(
                    "resize probe: frame {} blocked {:.3} ms in acquire",
                    state.log.len(),
                    acquire.as_secs_f64() * 1_000.0
                );
            }
        }

        if state.warmup_left > 0 {
            state.warmup_left -= 1;
        } else {
            state.log.push(record);
        }

        if let Some(wanted) = state.wanted
            && state.log.len() >= wanted
        {
            self.finish();
            event_loop.exit();
            return;
        }

        // The instrument M7.1c added, and it is off unless asked for. A frame
        // count rather than a wall clock, so two machines of different speeds
        // still run the same sequence against the same frames.
        if self.plan.resize_probe
            && let Some(&(_, width, height)) = RESIZE_PROBE
                .iter()
                .find(|(at, _, _)| *at == state.log.len())
            && let Some(window) = self.window.as_ref()
        {
            println!("resize probe: asking the window for {width}x{height}");
            let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(width, height));
        }

        if self.plan.resize_probe
            && state.log.len() == PROBE_MAXIMISE
            && let Some(window) = self.window.as_ref()
        {
            println!("resize probe: asking the window to maximise");
            window.set_maximized(true);
        }

        // Ask for the next one. `ControlFlow::Poll` alone would also keep the
        // loop turning, and asking as well is what makes the frame rate the
        // loop's rather than the platform's idea of when a redraw is warranted.
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Writes a captured frame under `target/screenshots/` and says where.
    ///
    /// The wall clock is read **here**, in the runner, and never inside a tick:
    /// a screenshot's file name is not world state, nothing derived from it
    /// reaches a hash, and the determinism rule that bans `SystemTime` from
    /// simulation logic is untouched by a name.
    ///
    /// A clock before the epoch is not an error worth a variant. It cannot
    /// happen on a machine whose clock is anywhere near right, and a screenshot
    /// called `window-0.png` is a readable answer to it.
    fn write_capture(
        pixels: &narvo_render2d::Pixels,
    ) -> Result<PathBuf, narvo_render2d::RenderError> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());

        screenshot::write_window_capture(Path::new(screenshot::WINDOW_DIR), stamp, pixels)
    }

    /// Moves the finished run's numbers out of the live state.
    fn finish(&mut self) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        self.report = Some(RunReport {
            log: std::mem::take(&mut state.log),
            present_mode: state.host.target().present_mode(),
            adapter: state.host.target().adapter_summary().to_owned(),
            sprites: state.host.sprite_count(),
            warmup: self.plan.warmup,
            drained: self.plan.drain,
        });
    }
}

/// The budget one frame has at 60 Hz, as the criterion states it.
///
/// `ProjektPlan.md` §6/M3 names 60 fps; this is that, written as a division so a
/// report compares against the rate rather than against a typed-out constant.
///
/// It is **not** the exact reciprocal: integer division truncates 16 666 666.67
/// to 16 666 666 ns, two thirds of a nanosecond short. That is deliberate rather
/// than tolerated — it is bit for bit the step `FixedTimestep::new(60)` produces
/// (`time.rs:48-50` records the same truncation and why), so the budget and the
/// simulation's own step are the same number on every machine.
pub const SIXTY_HERTZ_BUDGET: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// A human-readable millisecond figure.
#[must_use]
pub fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Prints what a run measured, as the table `docs/perf/BASELINE.md` carries.
pub fn report(report: &RunReport) {
    use narvo_core::frame::FramePhase;

    let log = &report.log;
    println!();
    println!(
        "frames {} (after {} discarded), sprites {}, present mode {}, drain {}",
        log.len(),
        report.warmup,
        report.sprites,
        report.present_mode,
        if report.drained { "on" } else { "off" }
    );
    println!("adapter: {}", report.adapter);
    println!();
    println!("phase          |      p50 |      p99 |      max");
    println!("---------------|----------|----------|---------");

    for phase in FramePhase::ALL {
        let at = |quantile| log.quantile(phase, quantile).map_or(0.0, millis);
        println!(
            "{:14} | {:8.3} | {:8.3} | {:8.3}",
            phase.name(),
            at(0.5),
            at(0.99),
            at(1.0)
        );
    }

    let work = |quantile: f64| log.work_quantile(quantile).map_or(0.0, millis);
    let interval = |quantile: f64| log.frame_time_quantile(quantile).map_or(0.0, millis);
    println!("---------------|----------|----------|---------");
    println!(
        "{:14} | {:8.3} | {:8.3} | {:8.3}",
        "work",
        work(0.5),
        work(0.99),
        work(1.0)
    );
    println!(
        "{:14} | {:8.3} | {:8.3} | {:8.3}",
        "interval",
        interval(0.5),
        interval(0.99),
        interval(1.0)
    );

    println!();
    println!(
        "budget {:.3} ms; intervals over budget: {} of {}; frames drawn: {}; ticks: {}",
        millis(SIXTY_HERTZ_BUDGET),
        log.frames_longer_than(SIXTY_HERTZ_BUDGET),
        log.len(),
        log.drawn(),
        log.ticks()
    );
}
