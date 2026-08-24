# Golden references — narvo-app

Same rules as `crates/narvo-render2d/tests/golden/`: these files belong to
the maintainer. No agent writes here. A red golden test means the renderer
changed; the reference is only ever updated in a separate, human-authored
commit after a human has looked at the image.

Why here and not in narvo-render2d: this scene is produced by
`placements_of`, which lives in narvo-app and has no library target, so the
test that renders it is a unit test in `src/sprite_batch.rs`. That forces the
kind of test, not the path. Since M3.11 its `.actual.png` lands in
`target/debug/golden/`, beside the artifacts of every other golden test in
the repository: `narvo_render2d::golden_artifact_dir()` derives that
directory from the running test binary, so it follows `CARGO_TARGET_DIR` when
that is set, and it needs no `CARGO_TARGET_TMPDIR` — which cargo sets only for
integration tests and benches, never for a unit test.