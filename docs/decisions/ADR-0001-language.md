# ADR-0001: Implementation language is Rust

Status: accepted · Date: 2026-08 · Scope: whole project

## Context

Narvo is built primarily *by* AI agents (Claude Code), with a human as
reviewer and product owner. The binding constraint for agentic development is
not how much training data exists for a language, but the quality of the
feedback loop: how fast and how precisely an agent learns whether its code is
correct. The candidates were C++ (larger corpus, dominant in engines) and
Rust.

## Decision

Rust, for the entire engine and all tooling.

## Rationale

1. **Error class shifted to compile time.** Memory and lifetime bugs that
   surface in C++ as delayed runtime failures — the class agents debug worst —
   become precise compiler messages via the borrow checker. That is exactly
   the feedback format an agent can self-correct against.
2. **Uniform tooling.** `cargo build/test/clippy/fmt/doc` is one command
   surface with no per-project build configuration for agents to burn turns
   on. C++ means CMake variance per machine and per dependency.
3. **Ecosystem is sufficient.** wgpu, winit, rapier, egui cover the
   infrastructure layer; nothing on the roadmap requires C++ middleware.

## Consequences

- Compile time becomes the main iteration-latency risk. It is treated as a
  budgeted, measured quantity from day one (dev-profile tuning, fast linkers,
  feature-gated headless builds, small crates; see `docs/perf/BASELINE.md`).
- Gameplay code is Rust as well; no embedded scripting language in v1.

## Revision condition

Reopen only if the project were forced to link against C++-only middleware
(e.g. PhysX, native FMOD). This is explicitly out of scope for Narvo, so no
revision is expected.
