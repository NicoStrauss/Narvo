# Narvo

[![CI](https://github.com/NicoStrauss/Narvo/actions/workflows/ci.yml/badge.svg)](https://github.com/NicoStrauss/Narvo/actions/workflows/ci.yml)

Narvo is an AI-first 2D game engine written in Rust. AI-first means the engine is
built so that humans and coding agents can both work on it and with it: a small set
of crates with explicit, documented API boundaries, deterministic behaviour by
construction (fixed timestep, seeded RNG, an explicit event queue), and a headless
mode in which a whole session can be driven and verified without a window or a GPU —
so a change can be checked by running it, not just by reading it.

## The workspace

Ten engine crates, a binary, a shared test-fixture crate and the tooling:

| Crate | Owns |
|---|---|
| `narvo-core` | time, the fixed timestep, the frame loop and its phase timing, error types |
| `narvo-ecs` | entities, components, queries, the system scheduler, the component registry, seeded RNG, events, and the handle bookkeeping a save restores |
| `narvo-render2d` | `wgpu`-backed 2D rendering, the glyph atlas and single-line text layout |
| `narvo-assets` | asset identity, loading, hot reload, and which regions are the frames of one clip |
| `narvo-input` | the device vocabulary, RON action mapping, `InputEvent` |
| `narvo-audio` | the cue vocabulary, synthesised sounds, a null sink, and a `kira` backend behind an off-by-default feature |
| `narvo-physics2d` | 2D rigid bodies over `rapier2d`, rebuilt every tick and keeping no state between them |
| `narvo-scene` | two RON formats over a world — the scene an author writes, and the save a run produces |
| `narvo-view2d` | the seam between a world and the renderer: sprite and camera extraction, and the hit test that mirrors draw order |
| `narvo-ipc` | the agent protocol as data, and the client that speaks it |
| `narvo-testkit` | the shared test fixtures, dev-only |
| `narvo-app` | the binary `narvo`: windowed and headless runner |
| `tools/narvo-cli`, `tools/narvo-mcp` | the command-line client, and an MCP server over the same protocol |
| `xtask` | the verification set as one command |

## What runs today

The engine is at **M8**. What follows is what the tree does, not a roadmap.

- **A simulation core** — a fixed-timestep accumulator, a component registry, a
  sequential system scheduler, seeded RNG that is itself world state, and an
  event queue with a defined one-tick delivery window.
- **A 2D renderer** on `wgpu`: textured sprites from a packed atlas, premultiplied
  alpha blending, a camera with one composition point, a screen-fixed overlay
  batch, and single-line text from a glyph atlas.
- **Scenes and saves** — RON the author writes, with prefabs, symbolic entity
  references and hot reload by reconstitution; and a separate save format that
  restores a world's entity handles exactly, tick included.
- **Assets** — a deterministic atlas packer, PNG sources premultiplied at load,
  and a file watcher that polls and hashes.
- **Input** — a closed device vocabulary mapped to named actions through a RON
  file, hit-testing that answers a click in draw order, and recordings taken at
  the action level.
- **Audio** — a cue vocabulary where a sound is named by a handle the registry
  issues, with a real backend behind a feature and a null sink without it.
- **Physics** — 2D rigid bodies over `rapier2d`.
- **An agent interface** — the protocol as a data crate, a gated localhost
  transport, a command-line client, and a hand-written MCP server.
- **A vertical slice**, built on all of it and living outside this repository as
  an ordinary consumer with a path dependency on these crates.

Crate boundaries, module layout and every public API should still be expected to
change before there is a version worth using.

## Verification

One command runs the whole set:

```
cargo xtask ci
```

It runs eleven cargo commands in a fixed order — build, tests, doctests, clippy,
fmt, `cargo deny`, the headless configuration (build, test and a dependency-tree
check that fails if a graphics crate reaches it), the agent transport, and a lint
of audio without a device — and stops at the first failure, naming it in a form
that pastes straight back into a shell. `CLAUDE.md` documents the set and the
reason each step exists.

Determinism is checked rather than asserted: two runs of a mode agree over 10 000
ticks, different seeds do not, a replay reproduces its original, a tampered
recording does not, and two platforms are compared against each other in CI.
**No expected hash is ever stored** — every comparison is between two runs
produced from one commit. Twelve blessed reference images cover the render path
the same way.

## Documentation

Architectural decisions are recorded as ADRs in `docs/decisions/`, forty-seven of
them; performance baselines are in `docs/perf/`. The working agreement for anyone
— human or agent — changing this repository is `CLAUDE.md`.

The milestone plan, the open-decision table and the project history live in a
private repository and are not published. Prose here cites them by name
(`ProjektPlan.md` §11, `docs/history/`) where that is the honest account of where
something was decided; those names are deliberately not links, because they would
not resolve.

## License

**All rights reserved.** No licence has been chosen for Narvo yet, and until one
is, none is granted: use, copying, modification and distribution by anyone else
are not permitted. The full notice is in [LICENSE](LICENSE).

Narvo carried an `MIT OR Apache-2.0` dual licence from M0 until 14.08.2026 —
added by `88ca1fb`, which is M0's second commit and not the repository's first.
It was removed rather than replaced, because the two questions are separate:
what Narvo will be licensed under is open and is a human's decision
(`ProjektPlan.md` §11, D5), while a grant already made cannot be taken back from
copies published under it. Removing it while nothing has been published is what
keeps that decision open at all.

Third-party material keeps its own terms — every dependency, and the DejaVu
Sans Mono font under `crates/narvo-render2d/assets/`, whose own licence sits
beside it because its terms require it to travel with the font.

### Contribution

There is nothing to contribute under. Narvo is not accepting outside
contributions while it has no licence, because a contribution needs terms on
both sides and there are none to point at.
