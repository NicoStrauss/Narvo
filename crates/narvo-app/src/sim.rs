//! The demo simulations the runners drive, and the components they share.
//!
//! Deliberately boring, the first three of them. Their job is to be something a
//! determinism comparison can be run *on*, not to be interesting. Gameplay
//! arrives with the vertical slice; what is being verified here is the
//! comparison path.
//!
//! [`Mode::Scene`] is the one that is not. It is in that comparison like the
//! others — `tests/determinism.rs`'s `MODES` lists all four — but it is also the
//! first simulation here meant to be *drawn*, and the frame-time figure in
//! `docs/perf/BASELINE.md` is measured on the same scene at
//! [`scene::MEASURED_SPRITES`]. See [`scene`].
//!
//! Nothing in these modules reads a clock or asks the operating system for
//! entropy. [`Mode::Chance`] draws random numbers, but from a generator seeded by
//! the caller whose state lives in the world like every other piece of
//! simulation state — see [`chance`] and ADR-0010.
//!
//! They live in `narvo-app` rather than in a library crate because they are
//! composition-root decisions: which components exist and which systems run in
//! which order is what an application defines.

use std::fmt;

use narvo_ecs::{ComponentRegistry, EcsError, Rng, Scheduler, SystemContext, World};
use narvo_input::InputEvent;
use serde::{Deserialize, Serialize};

use crate::recording::RecordedInput;

pub mod chance;
pub mod input;
pub mod motion;
pub mod physics;
pub mod scene;
pub mod scene_file;

/// Which demo simulation a run drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Integer movement plus one accumulating `f32`. The default.
    ///
    /// Unchanged since M2.2 and deliberately frozen: its state hash is the
    /// regression evidence that later work has not moved the ground underneath
    /// the instrument. Nothing may be added to it, and that includes components
    /// that would only be registered.
    #[default]
    Motion,
    /// Seeded randomness and events, driving everything that moves.
    ///
    /// Every entity starts at rest, so the whole evolution of the state is
    /// caused by drawn impulses that travel through an event buffer. If either
    /// the generator or the delivery broke, the world would simply sit still —
    /// which is what makes "ten thousand ticks changed something" a test of both
    /// rather than an observation about arithmetic.
    Chance,
    /// Driven entirely by input arriving from outside the simulation.
    ///
    /// Nothing inside this world decides anything: every entity starts at rest
    /// and only an [`InputEvent`] can change that. It is the mode a recording is
    /// worth making of, because a replay that fed the wrong inputs — or none —
    /// produces a visibly different state rather than a subtly different one.
    Input,
    /// A field of sprites and a camera that moves over them.
    ///
    /// **The first mode that is meant to be drawn**, and therefore the first one
    /// that discharges the registration duty of `ProjektPlan.md` §12: it
    /// registers `Transform`, `Layer`, `Sampling`, `Camera`, `Follow` and
    /// `Shake` together, so the state that decides its picture is inside the
    /// canonical dump rather than beside it. See [`scene`].
    ///
    /// It runs here at [`scene::SPRITES`]. The windowed runner builds the same
    /// scene through the same [`scene::build_with`] at whatever count it is
    /// given - [`scene::MEASURED_SPRITES`] by default, or what `--sprites` says.
    /// The sprite count is the only thing that differs between them.
    Scene,
    /// A world constituted from a scene **file** rather than from code.
    ///
    /// The first mode whose initial state is not written in Rust
    /// (`ProjektPlan.md` §6/M4). It registers the engine component set and wires
    /// `compose_camera`; what it contains is whatever the file says. A recording
    /// of such a run carries a scene identity anchor and refuses to replay
    /// against a different file — ADR-0019. See [`scene_file`].
    SceneFile,
    /// A stack of rigid bodies falling onto a ground, stepped by the solver.
    ///
    /// **The first mode whose state is computed by a dependency** rather than by
    /// arithmetic written in this crate. It exists to put physics into the
    /// cross-platform comparison on two foreign machines, which is the evidence
    /// ADR-0013's amendment leaves open — WSL and Windows share a CPU, and two
    /// CI runners do not.
    ///
    /// It drives `RigidBody` through [`crate::physics`], the seam M5b.3a built,
    /// rather than wiring the solver a second time. See [`physics`].
    Physics,
}

impl Mode {
    /// The mode's name on the command line and in diagnostics.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Motion => "motion",
            Self::Chance => "chance",
            Self::Input => "input",
            Self::Scene => "scene",
            Self::SceneFile => "scene-file",
            Self::Physics => "physics",
        }
    }

    /// The action names this mode understands, sorted.
    ///
    /// Empty for a mode that consumes no input at all, which is also how
    /// [`consumes_input`](Self::consumes_input) is answered — there is one list
    /// rather than a list and a flag that can disagree.
    #[must_use]
    pub fn actions(self) -> &'static [&'static str] {
        match self {
            Self::Motion | Self::Chance | Self::Scene | Self::SceneFile | Self::Physics => &[],
            Self::Input => &input::ACTIONS,
        }
    }

    /// Whether anything from outside a tick reaches this simulation.
    #[must_use]
    pub fn consumes_input(self) -> bool {
        !self.actions().is_empty()
    }

    /// Parses a mode name.
    ///
    /// Written as a search over [`ALL`](Self::ALL) rather than as a second match
    /// on the names, so that it is the exact inverse of [`name`](Self::name) by
    /// construction: a mode added to the enum and to `ALL` is parseable
    /// immediately, and there is no second list to forget.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }

    /// Every mode, in the order they are listed to the user.
    pub const ALL: [Self; 6] = [
        Self::Motion,
        Self::Chance,
        Self::Input,
        Self::Scene,
        Self::SceneFile,
        Self::Physics,
    ];
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Where something is.
#[derive(Debug, Serialize, Deserialize)]
pub struct Position {
    x: i64,
    y: i64,
}

/// How far it moves per tick.
#[derive(Debug, Serialize, Deserialize)]
pub struct Velocity {
    dx: i64,
    dy: i64,
}

/// Integer motion: position advances by velocity, once per tick.
///
/// Shared by both modes. Over the tick counts these runners use the coordinates
/// stay far inside `i64`, so nothing wraps and the arithmetic is exact on every
/// platform.
fn movement(world: &mut World, _context: &SystemContext) {
    let mut moving = world.query::<(&mut Position, &Velocity)>();
    for (_id, (position, velocity)) in moving.iter() {
        position.x += velocity.dx;
        position.y += velocity.dy;
    }
}

/// A built simulation: the world, the names its state is written under, and the
/// order its systems run in.
///
/// The three are returned together because they only make sense together — a
/// registry that does not cover the world's components makes
/// [`canonical_dump`](narvo_ecs::canonical_dump) fail, which is the intended
/// behaviour and not something a caller should be able to assemble by accident.
pub struct Simulation {
    /// The entities and their components.
    pub world: World,
    /// Stable names for every component type the world contains.
    pub registry: ComponentRegistry,
    /// The systems, in the order they run.
    pub scheduler: Scheduler,
}

/// Written out rather than derived, because [`ComponentRegistry`] is not
/// `Debug` — its `ComponentInfo` carries function pointers, and `narvo-ecs`
/// derives only `Default` and `Clone` on it (`registry.rs:189`).
///
/// Deriving here would therefore mean adding `Debug` in a second crate, which
/// CLAUDE.md treats as an architecture signal rather than a convenience. Doing
/// it in this one place instead is what keeps every holder of a `Simulation`
/// able to derive its own `Debug` — [`crate::frame::SceneHost`] is the first,
/// since M6.6a.
///
/// The registry appears as the number of components it names. That is what a
/// reader of a `{:?}` line can use: whether the names are there at all, and how
/// many. The names themselves are one `iter()` away and are not this line's job.
impl fmt::Debug for Simulation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Simulation")
            .field("world", &self.world)
            .field(
                "registry",
                &format_args!("{} components", self.registry.len()),
            )
            .field("scheduler", &self.scheduler)
            .finish()
    }
}

/// Builds the simulation for `mode`.
///
/// `seed` is used by [`Mode::Chance`] and ignored by [`Mode::Motion`], which has
/// nothing random in it. Ignoring it rather than refusing it keeps the runner's
/// signature the same for both modes; the summary line reports which mode ran,
/// so a seed passed to `motion` is visible rather than silently effective.
///
/// # Errors
///
/// Anything the world or the registry can raise while being built — in practice
/// a component type registered twice or a stable name used twice, both of which
/// are programming mistakes in the lines just below.
pub fn build(mode: Mode, seed: u64) -> Result<Simulation, EcsError> {
    match mode {
        Mode::Motion => motion::build(),
        Mode::Chance => chance::build(seed),
        Mode::Input => input::build(),
        Mode::Scene => scene::build(),
        Mode::Physics => physics::build(),
        // A scene-file world has no code to build it - that is the whole point
        // of the mode - so it is built by `scene_file::build` from the file's
        // text, and `headless::run` dispatches on the loaded scene before it
        // ever consults the mode. Unreachable by construction rather than by
        // hope, and loud rather than silently wrong if that construction ever
        // changes.
        Mode::SceneFile => unreachable!(
            "a scene-file world is built from its file by sim::scene_file::build; \
             headless::run dispatches on the loaded scene and never reaches here"
        ),
    }
}

/// Draws one tick's worth of synthetic input for `mode`.
///
/// The stand-in for a player during a recorded run. A mode that consumes no
/// input produces none, so a recording of it is a valid, empty one rather than a
/// special case.
///
/// The generator lives in the runner, not in the world. ADR-0012 explains why
/// that is required rather than convenient: on replay this function is not
/// called at all, and if its state were part of the simulation the replayed
/// world would differ from the recorded one in exactly that state.
pub fn pilot(mode: Mode, rng: &mut Rng) -> Vec<InputEvent> {
    match mode {
        Mode::Motion | Mode::Chance | Mode::Scene | Mode::SceneFile | Mode::Physics => Vec::new(),
        Mode::Input => input::pilot(rng),
    }
}

/// Puts one tick's input where `mode`'s simulation will read it.
///
/// # Panics
///
/// Panics if `events` is not empty for a mode that consumes no input. That
/// cannot arise from a file — [`validate_recording`] rejects such a recording
/// with a message naming the tick and the action — so reaching it means the
/// runner fed something it built itself, which is a bug rather than a condition.
///
/// # Errors
///
/// Anything reaching the simulation's input buffer can raise.
pub fn feed(mode: Mode, world: &mut World, events: Vec<InputEvent>) -> Result<(), EcsError> {
    if events.is_empty() {
        return Ok(());
    }

    match mode {
        Mode::Input => input::feed(world, events),
        Mode::Motion | Mode::Chance | Mode::Scene | Mode::SceneFile | Mode::Physics => {
            unreachable!(
                "mode {mode} consumes no input, so nothing should ever be fed to it; \
             a recording carrying input for it is rejected before the run starts"
            )
        }
    }
}

/// Checks that `mode` can actually consume everything in `inputs`.
///
/// Run before a replay starts, so a mismatch is reported once, up front, naming
/// the tick — rather than as ten thousand silently ignored events and a state
/// hash that differs for no visible reason.
///
/// # Errors
///
/// [`RecordingMismatch`] for the first input the mode cannot consume.
pub fn validate_recording(mode: Mode, inputs: &[RecordedInput]) -> Result<(), RecordingMismatch> {
    for input in inputs {
        let action = input.event.action();

        if !mode.consumes_input() {
            return Err(RecordingMismatch::ModeConsumesNoInput {
                mode,
                tick: input.tick,
                action: action.to_owned(),
            });
        }
        if !mode.actions().contains(&action) {
            return Err(RecordingMismatch::UnknownAction {
                mode,
                tick: input.tick,
                action: action.to_owned(),
            });
        }
    }

    Ok(())
}

/// A recording that this build cannot replay faithfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingMismatch {
    /// The recording carries input for a mode that has no way to consume it.
    ModeConsumesNoInput {
        /// The mode named in the recording.
        mode: Mode,
        /// The tick of the first such input.
        tick: u64,
        /// Its action name.
        action: String,
    },
    /// The recording names an action this mode does not have.
    UnknownAction {
        /// The mode named in the recording.
        mode: Mode,
        /// The tick of the first such input.
        tick: u64,
        /// Its action name.
        action: String,
    },
}

impl fmt::Display for RecordingMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModeConsumesNoInput { mode, tick, action } => write!(
                f,
                "this recording has input at tick {tick} (\"{action}\"), but simulation mode \
                 {mode} consumes no input at all; replaying it would drop every one of them"
            ),
            Self::UnknownAction { mode, tick, action } => write!(
                f,
                "this recording has the action \"{action}\" at tick {tick}, which simulation mode \
                 {mode} does not know; it understands {}",
                mode.actions().join(", ")
            ),
        }
    }
}

impl std::error::Error for RecordingMismatch {}

/// Fails naming *where* two states first differ, instead of printing both.
///
/// A dump is a couple of thousand bytes; `assert_eq!` on two of them buries the
/// one changed number in a wall of text that has to be diffed by eye. The
/// locating is [`narvo_ecs::first_difference`]'s, which is where it belongs —
/// the dump is that crate's format, so the tool that reads it moves when the
/// format does. All this adds is the caller's own label.
///
/// Shared by this module's tests and [`motion`]'s. The integration suite in
/// `tests/determinism.rs` has its own five-line copy because it is a separate
/// crate target and `narvo-app` has no library target to hold a helper.
#[cfg(test)]
pub(crate) fn assert_same_state(label: &str, left: &str, right: &str) {
    if let Some(difference) = narvo_ecs::first_difference(left, right) {
        panic!("{label}: the two states differ.\n{difference}");
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, assert_same_state, build};
    use narvo_ecs::canonical_dump;

    /// Every mode `build` can build — which is every mode but `scene-file`,
    /// whose world comes from a file and whose builder is `scene_file::build`.
    ///
    /// Kept as its own list rather than as `Mode::ALL` minus one, so that adding
    /// a mode is a deliberate edit here as well.
    const CODE_BUILT: [Mode; 4] = [Mode::Motion, Mode::Chance, Mode::Input, Mode::Scene];

    #[test]
    fn every_mode_builds_and_dumps() {
        // The direct check of the failure mode this whole milestone exists to
        // prevent: an unregistered component would make `canonical_dump` fail
        // here rather than silently drop part of the state from the hash.
        for mode in CODE_BUILT {
            let simulation = build(mode, 1).expect("the demo simulations always build");
            canonical_dump(&simulation.world, &simulation.registry)
                .unwrap_or_else(|error| panic!("mode {} does not dump: {error}", mode.name()));
        }
    }

    #[test]
    fn every_mode_is_built_identically_every_time() {
        for mode in CODE_BUILT {
            let first = build(mode, 1).expect("the demo simulations always build");
            let second = build(mode, 1).expect("the demo simulations always build");

            assert_same_state(
                &format!("mode {} built twice", mode.name()),
                &canonical_dump(&first.world, &first.registry).expect("everything is registered"),
                &canonical_dump(&second.world, &second.registry).expect("everything is registered"),
            );
        }
    }

    #[test]
    fn mode_names_round_trip_and_an_unknown_one_is_rejected() {
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.name()), Some(mode));
        }

        assert_eq!(Mode::parse("chaos"), None);
        assert_eq!(Mode::parse(""), None);
    }

    #[test]
    fn the_default_mode_is_the_one_that_carries_the_regression_evidence() {
        // M2.2's recorded hash was taken on `motion`. If the default ever moved,
        // `narvo --ticks 10000 --hash` would silently start reporting a
        // different simulation and the comparison against that record would
        // quietly stop meaning anything.
        assert_eq!(Mode::default(), Mode::Motion);
    }

    #[test]
    fn the_two_modes_are_different_simulations() {
        let motion = build(Mode::Motion, 1).expect("builds");
        let chance = build(Mode::Chance, 1).expect("builds");

        assert_ne!(
            canonical_dump(&motion.world, &motion.registry).expect("registered"),
            canonical_dump(&chance.world, &chance.registry).expect("registered")
        );
    }
}
