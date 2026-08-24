//! The two scenes `narvo-app` blesses, and the probes that keep them honest.
//!
//! **This module has no production code.** It is the half of what used to be
//! `sprite_batch.rs` that did not move to `narvo-view2d` in M6b.1 (ADR-0041):
//! the tests that render a world and compare the result against a committed PNG,
//! plus the fixtures and probes those tests rest on. They stayed because their
//! reference images are here — `crates/narvo-app/tests/golden/` — and a golden
//! test belongs beside the file it is blessed against.
//!
//! The extraction they drive now lives one crate down and is reached through
//! `narvo_view2d` rather than through `super`. That is the whole of what the
//! move did to them, and it is the point: **the same worlds, the same renders,
//! the same two reference images, byte for byte.**
//!
//! # Why these are unit tests rather than integration tests
//!
//! Because one of them cannot be anything else. `counter_world_at_three` drives
//! the click counter through `crate::sim::scene_file::count_actions`, which is
//! `pub(crate)` and stays that way — ADR-0041 keeps the tally out of the seam,
//! because it reads an `Events<InputEvent>` and would put `narvo-input` under
//! every consumer of the renderer. An integration test under `tests/` cannot
//! reach a `pub(crate)` item, and `narvo-app` has no library target to reach it
//! through. So the file holding that test has to sit inside `src/`, and keeping
//! its sibling beside it is what lets one atlas fixture serve both.
//!
//! Gated on `render` like the module it came from: every test here needs an
//! `OffscreenTarget`, or a fixture built for one.

#[cfg(test)]
mod tests {
    use narvo_ecs::{
        Camera, Follow, Layer, Sampling, Shake, Sprite, SystemContext, Transform, World,
        compose_camera,
    };
    use narvo_render2d::{
        Golden, OffscreenTarget, Pixels, RenderError, SpriteInstance, SpritePlacement,
        TextureRegion, Tolerance, golden_artifact_dir,
    };
    use narvo_view2d::{camera_of, placements_of, regions_of};

    // --- The overlapping scene, and the reference image it asks for ---------
    //
    // Everything from here to the perf guards is the scene M3.10 built and
    // M3.11 made legible. It lives in this module, not under `tests/`, because
    // `narvo-app` is a binary crate with no library target: an integration
    // test cannot reach `placements_of`, which ADR-0015 records as a
    // consequence of the seam sitting here. This is the only place a world can
    // be rendered through the ordering M3.10 added.

    /// Set this, to anything, and a missing adapter fails instead of skipping.
    const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

    /// Printed when a test cannot run for lack of an adapter.
    const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

    /// The name this scene's reference image is blessed under.
    ///
    /// New in M3.11, because the image is new. M3.10's scene drew all three
    /// sprites from the same texture and was blessed under the name
    /// `layer_order_overlap_128x128` — though the file went in as
    /// `layer_order_overlap_128x128..png`, which `Golden::reference_path` never
    /// resolves, so that test never found its reference. `1e4a53e` removed it;
    /// no test refers to the name any more.
    const OVERLAP_SCENE: &str = "layer_order_regions_128x128";

    /// The render target's edge, in pixels. Square, so one number.
    const TARGET_SIZE: u32 = 128;

    /// Every sprite's edge, in world units — and therefore in pixels, since the
    /// projection makes one world unit one pixel.
    const SPRITE_SIZE: f32 = 64.0;

    /// This scene's atlas layout: four 8 x 8 content regions, each with a
    /// one-texel border of duplicated edge texels (D13), from the shared
    /// fixture place (D16, ADR-0016).
    const LAYOUT: narvo_testkit::AtlasLayout = narvo_testkit::AtlasLayout::PADDED;

    /// The atlas's eight colours: upper half then lower half of each quadrant.
    ///
    /// Named for the sprite that samples them rather than for their place in the
    /// texture, because that is what every probe and every failure message here
    /// is about. `UNUSED_*` is sampled by nothing.
    const A_UPPER: [u8; 4] = narvo_testkit::TOP_LEFT_UPPER;
    const A_LOWER: [u8; 4] = narvo_testkit::TOP_LEFT_LOWER;
    const B_UPPER: [u8; 4] = narvo_testkit::TOP_RIGHT_UPPER;
    const B_LOWER: [u8; 4] = narvo_testkit::TOP_RIGHT_LOWER;
    const C_UPPER: [u8; 4] = narvo_testkit::BOTTOM_LEFT_UPPER;
    const C_LOWER: [u8; 4] = narvo_testkit::BOTTOM_LEFT_LOWER;
    const UNUSED_UPPER: [u8; 4] = narvo_testkit::BOTTOM_RIGHT_UPPER;
    const UNUSED_LOWER: [u8; 4] = narvo_testkit::BOTTOM_RIGHT_LOWER;

    /// What the target is cleared to, and therefore what "no sprite" looks like.
    const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

    /// The smallest difference two sprites' colours may show in their
    /// most-different channel.
    ///
    /// Far above the golden thresholds — a noise floor of 4 counts and a
    /// per-pixel cap of 24 — so no tolerance can absorb a swap between two
    /// sprites, and far enough above them that a human reading the image is not
    /// being asked to judge a shade. The relation to those two numbers is
    /// asserted rather than asserted about, in
    /// [`no_two_sprites_can_show_the_same_colour_anywhere`].
    const MIN_SPRITE_COLOUR_DISTANCE: u8 = 64;

    /// The 20 x 20 padded atlas, from the shared place (D16, ADR-0016).
    ///
    /// Four 8 x 8 content quadrants, each split into an upper and a lower half
    /// of four rows, and each carrying a one-texel border of its own duplicated
    /// edge texels. Until M3.22 this was the third hand-written copy of that
    /// shape; M3.21 padded the other two and left this one describing a layout
    /// that had stopped being shared, which is the finding D16 answers.
    ///
    /// | content texels | cell | quadrant | rows 0-3 | rows 4-7 | sampled by |
    /// | --- | --- | --- | --- | --- | --- |
    /// | (1, 1) 8 x 8 | (0, 0) | top left | red | dark red | A |
    /// | (11, 1) 8 x 8 | (1, 0) | top right | green | dark green | B |
    /// | (1, 11) 8 x 8 | (0, 1) | bottom left | blue | dark blue | C |
    /// | (11, 11) 8 x 8 | (1, 1) | bottom right | yellow | olive | nobody |
    ///
    /// The half-and-half split does two things at once. It gives each sprite an
    /// interior edge, so a region drawn upside down is visible inside a single
    /// sprite; and it leaves each sprite a colour *family* of its own, which is
    /// the property `no_two_sprites_can_show_the_same_colour_anywhere` turns
    /// into an assertion. **The fourth quadrant is sampled by nothing**, so
    /// yellow and olive may not appear anywhere in a correct frame — the M3.9
    /// detector for "the region was computed and then ignored", kept because it
    /// costs one loop and covers all 16 384 pixels rather than the nine somebody
    /// thought to probe.
    ///
    /// **`Nearest` reads no border texel**, which is why this scene's blessed
    /// reference did not move when the border arrived. Only this scene: the
    /// claim is about `layer_order_regions_128x128`, drawn through
    /// `render_sprites`, and not about any other image or draw path.
    fn atlas() -> Pixels {
        LAYOUT.atlas()
    }

    /// One sprite of the scene.
    #[derive(Debug, Clone, Copy)]
    struct SceneSprite {
        /// What the derivation and the failure messages call it.
        name: &'static str,
        /// Its `Layer` depth.
        depth: f32,
        /// Centre along x, in world units.
        x: f32,
        /// Centre along y, in world units.
        y: f32,
        /// Its part of the atlas, as the cell index `(cell_x, cell_y)`.
        ///
        /// A cell rather than a texel rectangle since M3.22: the border moved
        /// every content origin, and a cell index is the coordinate that did
        /// not move. [`LAYOUT`] turns it back into texels.
        region: (u32, u32),
    }

    /// Three overlapping sprites, **spawned in an order the depths do not agree
    /// with**, each showing a region of the atlas nobody else shows.
    ///
    /// The array order is spawn order — C, A, B — and draw order has to come
    /// out A, B, C. That is the whole point of the fixture: if the sort were
    /// missing, ignored or reversed, the entity ids alone would produce a
    /// different image, and the probes say which.
    ///
    /// | sprite | spawn | depth | centre | region | covers pixels |
    /// | --- | --- | --- | --- | --- | --- |
    /// | C | 0 | 2.0 | (0, -8) | bottom left, cell (0, 1) | x 32-95, y 40-103 |
    /// | A | 1 | 0.0 | (-16, +16) | top left, cell (0, 0) | x 16-79, y 16-79 |
    /// | B | 2 | 1.0 | (+16, +16) | top right, cell (1, 0) | x 48-111, y 16-79 |
    ///
    /// The covered pixels follow from the projection: one world unit is one
    /// pixel with the origin at the target's centre, so a sprite of centre `c`
    /// and edge 64 covers the pixels whose centres lie in `(c + 31.5, c + 95.5)`
    /// along x and in `(31.5 - c, 95.5 - c)` along y.
    ///
    /// **The geometry is M3.10's, value for value.** M3.11 changes what the
    /// sprites *show*, not where they are, so the depth structure this scene was
    /// built to exercise is unchanged and the probe derivation below only had to
    /// be redone from the region up.
    const SCENE: [SceneSprite; 3] = [
        SceneSprite {
            name: "C",
            depth: 2.0,
            x: 0.0,
            y: -8.0,
            region: (0, 1),
        },
        SceneSprite {
            name: "A",
            depth: 0.0,
            x: -16.0,
            y: 16.0,
            region: (0, 0),
        },
        SceneSprite {
            name: "B",
            depth: 1.0,
            x: 16.0,
            y: 16.0,
            region: (1, 0),
        },
    ];

    /// [`SCENE`] as a world, spawned in array order.
    ///
    /// # `Sampling::linear()` since M3.28, and it lives here on purpose
    ///
    /// The third of the four blessed scenes converted to `Linear` (D13, order in
    /// `ProjektPlan.md` §12), and the only one of them that comes from a world.
    /// So the wish had a choice of homes, and §12 asked for the choice rather
    /// than the result: it could be a `Sampling` component here, or a
    /// `SpriteFilter` set on the `SpriteInstance` in [`render_the_overlapping_scene`].
    ///
    /// **Here, because the other place would have to overwrite this one.** That
    /// function already reads the world's wish — it maps `.sampled(drawn.filter)`
    /// over what [`placements_of`] returns, and `placements_of` has read a
    /// `Sampling` per entity since M3.23. Putting the wish on the `SpriteInstance`
    /// instead would mean replacing `drawn.filter` with a constant, which severs
    /// the one connection this scene is uniquely placed to cover: **world →
    /// extraction → `SpriteInstance` → run → sampler binding**, end to end, on a blessed
    /// image. Three lines here and none there; a constant there and a dead read
    /// here.
    ///
    /// The rejected way has one real argument and it is worth writing down: a
    /// sampler wish is arguably a property of how a fixture is *drawn*, not of
    /// what the simulation *is*, and this scene's subject is depth ordering — so
    /// keeping the world at `Transform` + `Layer` would keep the fixture minimal
    /// and the test's subject uncluttered. What decides against it is that
    /// `Sampling` is already simulation state by its own type's argument —
    /// `narvo_ecs::Sampling`'s doc: *"simulation state for the same reason
    /// `Layer` is: what a frame looks like has to be reproducible from a
    /// recording"* — so the minimal world would be the one that disagrees with
    /// the type it is leaving out. (ADR-0008 is the instrument that argument
    /// reaches for, not where it is made: it fixes what the state hash covers.)
    ///
    /// # What this world registers: nothing, and that is enough here
    ///
    /// It calls no `register_component`. Registration is what puts a component
    /// into the canonical dump and therefore into the state hash (ADR-0008), and
    /// this world is never dumped, never hashed, never recorded and never
    /// replayed — it is built inside this test, read once by `placements_of`,
    /// and dropped. `World::insert` does not require it.
    ///
    /// **This is not the registration duty being ignored.** That duty
    /// (`ProjektPlan.md` §12, the M3.23 surface) reads: *"Sobald ein Szenario
    /// Sprites zeichnet, registriert es `Transform`, `Layer` und `Sampling`
    /// zusammen — sonst ist sein Replay kein Replay."* It is about **scenarios**
    /// — the things `narvo-app` records and replays, which today are the three
    /// demo sims in `sim/`, and none of them draws a sprite. A fixture world in
    /// a golden test is not a scenario and has no replay to be wrong about.
    /// Registering here would not discharge the duty either: with no dump to
    /// compare, the calls would assert nothing, and a later reader would find
    /// the duty apparently met in the one place it does not apply. The duty
    /// stays open, and it falls due the day a scenario draws sprites.
    fn overlapping_world() -> World {
        let mut world = World::new();

        for sprite in SCENE {
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform {
                        x: sprite.x,
                        y: sprite.y,
                        rotation: 0.0,
                        scale_x: SPRITE_SIZE,
                        scale_y: SPRITE_SIZE,
                    },
                )
                .expect("the entity was just spawned");
            world
                .insert(entity, Layer::at(sprite.depth))
                .expect("the entity was just spawned");
            world
                .insert(entity, Sampling::linear())
                .expect("the entity was just spawned");
        }

        world
    }

    /// The scene entry a placement came from, identified by its centre.
    ///
    /// `placements_of` returns geometry and nothing else: the seam carries a
    /// `Transform`, and ADR-0015 has a renderer that needs more per sprite widen
    /// `SpritePlacement` rather than reach for a component. M3.11 does not widen
    /// it — it changes a scene, not the seam — so the region is paired back on
    /// here, after the sort, on the one field that survives it unchanged.
    ///
    /// The three centres are distinct, so this is total and cannot silently pick
    /// the wrong region; `the_scene_is_the_fixture_the_probes_assume` asserts
    /// that rather than leaving it to the reader. Compared on bit patterns
    /// because the extraction copies the field verbatim
    /// (`every_transform_becomes_a_placement_field_for_field`), so equality here
    /// is exact and not a tolerance question.
    fn scene_sprite_at(placement: SpritePlacement) -> SceneSprite {
        *SCENE
            .iter()
            .find(|sprite| {
                sprite.x.to_bits() == placement.x.to_bits()
                    && sprite.y.to_bits() == placement.y.to_bits()
            })
            .unwrap_or_else(|| {
                panic!(
                    "no sprite of the scene is centred at ({}, {}), so the \
                     extraction returned a placement this scene did not put in",
                    placement.x, placement.y
                )
            })
    }

    /// [`SCENE`]'s region for one entry, against the atlas it was measured on.
    fn region_of(sprite: SceneSprite, atlas: &Pixels) -> TextureRegion {
        let (cell_x, cell_y) = sprite.region;
        LAYOUT.region(cell_x, cell_y, atlas)
    }

    /// The probes for [`SCENE`], **derived before anything rendered** and
    /// rederived from scratch in M3.11: M3.10's nine expected the quadrant
    /// colours of one shared texture, and do not survive the change.
    ///
    /// For a pixel `(px, py)` of a 128 x 128 target the sample point is the
    /// pixel centre, at world `(px - 63.5, 63.5 - py)`. Inside a sprite whose
    /// left edge sits at pixel coordinate `l` and top edge at `t`, the
    /// unit-square coordinates are `u1 = (px - l) / 64` and `v1 = (py - t) / 64`;
    /// the region maps them to the atlas texel `(rx + u1 * 8, ry + v1 * 8)`,
    /// where `(rx, ry)` is the region's top-left **content** texel. `Nearest`
    /// rounds that down. The edges, from the table above: A `l = 15.5,
    /// t = 15.5`; B `l = 47.5, t = 15.5`; C `l = 31.5, t = 39.5`.
    ///
    /// **The border moved every texel below by a whole number and no fraction**
    /// (M3.22). A content origin goes from `cell * 8` to `cell * 10 + 1`, so
    /// `(rx, ry)` gains 1 for a first cell on an axis and 3 for a second, while
    /// the `8 * u1` term is unchanged — the atlas grew from 16 to 20 texels and
    /// the region's span stayed 8 of them. `Nearest` rounds down and the
    /// fraction decides the texel, so the texel indices below are the ones M3.11
    /// derived, each shifted by its cell's constant. This scene magnifies 8
    /// output pixels to the texel, so a sample sits at worst 1/16 of a texel
    /// from a boundary, and `u1` never reaches 1 — the index stays in 0..7 and
    /// never lands on the border.
    ///
    /// | probe | pixel | covered by | in front | texel | expected |
    /// | --- | --- | --- | --- | --- | --- |
    /// | background | (8, 8) | nothing | — | — | black |
    /// | A alone, upper | (24, 24) | A | A | (2.06, 2.06) | red |
    /// | A alone, lower | (24, 60) | A | A | (2.06, 6.56) | dark red |
    /// | B alone, lower | (104, 60) | B | B | (18.06, 6.56) | dark green |
    /// | C alone, lower | (40, 90) | C | C | (2.06, 17.31) | dark blue |
    /// | A under B | (60, 24) | A, B | B | (12.56, 2.06) | green |
    /// | A under C | (40, 60) | A, C | C | (2.06, 13.56) | blue |
    /// | B under C | (88, 60) | B, C | C | (8.06, 13.56) | blue |
    /// | all three | (60, 60) | A, B, C | C | (4.56, 13.56) | blue |
    ///
    /// **One triple-overlap probe, where M3.10 spent two.** There, all three
    /// sprites sampled one texture, so a contested pixel named the winner only
    /// where the three happened to fall in different quadrants, and the second
    /// of its two triple probes went to C's other quadrant (its own label:
    /// "all three, C in front, other half of C"). Here every pixel of every
    /// sprite carries that sprite's own colour family, so a single contested
    /// pixel names the winner outright, and the probe that frees up goes to a
    /// second single-sprite probe: (24, 24) puts A's upper half under one of
    /// its own, while the three pairwise probes M3.10 already had are kept —
    /// every pair of sprites is still probed where exactly those two meet.
    ///
    /// **No sample lands on a texel edge, and that holds for every pixel rather
    /// than for the nine below.** Each sprite is 64 pixels wide for 8 texels, so
    /// a texel coordinate is `rx + (px - l) / 8` across and `ry + (py - t) / 8`
    /// down, with `(rx, ry)` the region's top-left texel. Every edge listed
    /// above ends in `.5`, so the term added to that corner is
    /// `(2 px - 2 l) / 16` with `2 l` odd — 31 across and down A, 95 across B
    /// and 31 down it, 63 across C and 79 down it — and an odd numerator over
    /// 16 is never a whole number. `rx` and `ry` are whole, so neither is the
    /// sum. The tightest margin to a *region* boundary among the nine is 0.94
    /// texels, and to the light/dark split inside a region 1.44 texels.
    ///
    /// **Which failure each probe catches.** Draw order is A, B, C; the failures
    /// M3.10 named produce C, A, B ("no sort" and "depth read but not applied",
    /// which are the same image) and C, B, A ("reversed").
    ///
    /// | failure | first probe that reddens | what it shows instead |
    /// | --- | --- | --- |
    /// | sort missing | (40, 60) | dark red — A won a pixel C must own |
    /// | sort reversed | (60, 24) | red — A won a pixel B must own |
    /// | tie-break nondeterministic | none, by construction | no two sprites share a depth here; six GPU-free tests cover it |
    /// | depth read, not applied | (40, 60) | identical to "sort missing" |
    ///
    /// The four "alone" probes and the background probe are red under *none* of
    /// them, which is deliberate: they answer the different question of whether
    /// a sprite is present at all, and a scene that loses one has to fail
    /// somewhere other than where it is contested.
    const OVERLAP_PROBES: [(u32, u32, [u8; 4], &str); 9] = [
        (8, 8, BACKGROUND, "background, outside every sprite"),
        (24, 24, A_UPPER, "A alone, upper half of its region"),
        (24, 60, A_LOWER, "A alone, lower half of its region"),
        (104, 60, B_LOWER, "B alone, lower half of its region"),
        (40, 90, C_LOWER, "C alone, lower half of its region"),
        (60, 24, B_UPPER, "A under B, B in front"),
        (40, 60, C_UPPER, "A under C, C in front"),
        (88, 60, C_UPPER, "B under C, C in front"),
        (60, 60, C_UPPER, "all three, C in front"),
    ];

    /// Builds a target, or reports that this machine cannot host one.
    fn target_or_skip(width: u32, height: u32) -> Option<OffscreenTarget> {
        match OffscreenTarget::new(width, height) {
            Ok(target) => {
                println!("adapter in use: {}", target.adapter_summary());
                Some(target)
            }
            Err(error @ RenderError::NoAdapter { .. }) => {
                assert!(
                    std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                    "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a \
                     failure rather than a skip: {error}"
                );

                println!("{SKIP_MARKER}: {error}");
                None
            }
            Err(other) => panic!(
                "creating the offscreen target failed for a reason other than a \
                 missing adapter: {other}"
            ),
        }
    }

    /// Renders [`SCENE`] through the extraction M3.10 changed.
    fn render_the_overlapping_scene(target: &OffscreenTarget) -> Pixels {
        let atlas = atlas();
        let sprites: Vec<SpriteInstance> = placements_of(&overlapping_world())
            .into_iter()
            .map(|drawn| {
                SpriteInstance::new(
                    drawn.placement,
                    region_of(scene_sprite_at(drawn.placement), &atlas),
                )
                .sampled(drawn.filter)
                .tinted(drawn.tint)
            })
            .collect();

        target
            .render_sprites(&atlas, &sprites)
            .expect("three sprites are far inside the batch limit")
    }

    /// The largest single-channel difference between two colours.
    fn channel_distance(left: [u8; 4], right: [u8; 4]) -> u8 {
        left.iter()
            .zip(right.iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .expect("a colour has four channels")
    }

    /// Every colour the atlas can hand a sprite that samples `region`.
    ///
    /// A texel belongs to the region when its *centre* lies inside the region's
    /// bounds, which is what `Nearest` can return for a sample point strictly
    /// inside them. Read off the region's own normalised bounds rather than off
    /// the texel numbers [`SCENE`] passed to `from_texels`, so this measures what
    /// a sprite will sample rather than what the table meant to say.
    fn colours_in(region: TextureRegion, atlas: &Pixels) -> Vec<[u8; 4]> {
        let [u_left, v_top, u_right, v_bottom] = region.uv_bounds();
        let mut colours: Vec<[u8; 4]> = Vec::new();

        for y in 0..atlas.height() {
            for x in 0..atlas.width() {
                let u = (f64::from(x) + 0.5) / f64::from(atlas.width());
                let v = (f64::from(y) + 0.5) / f64::from(atlas.height());

                if u > f64::from(u_left)
                    && u < f64::from(u_right)
                    && v > f64::from(v_top)
                    && v < f64::from(v_bottom)
                {
                    let colour = atlas
                        .pixel(x, y)
                        .expect("x and y come from the atlas's own dimensions");
                    if !colours.contains(&colour) {
                        colours.push(colour);
                    }
                }
            }
        }

        colours
    }

    /// **Every sprite is distinguishable from every other at every pixel it
    /// covers.**
    ///
    /// Runs without a GPU, and it is a statement about the regions rather than
    /// about the renderer: whatever texel a sprite samples, the colour comes
    /// from its own region, so pairwise disjoint region palettes make *any*
    /// contested pixel name the sprite that won it — not just the pixels the
    /// probes happen to visit.
    ///
    /// **This is the property M3.10's scene did not have, and the cost of not
    /// having it was a failed visual inspection.** All three of its sprites
    /// showed the same texture, so wherever two of them met carrying the same
    /// quadrant colour the boundary between them vanished; the image read as an
    /// irregular puzzle instead of three squares, and the human blessing a
    /// *correct* reference took it for a broken one (`ProjektPlan.md` §10).
    /// Looking at the image is the one verification step no machine performs, and
    /// this is the machine-checked precondition for it being possible at all.
    ///
    /// The bound is a distance rather than mere inequality: two colours that
    /// differ by one count are distinct and neither a tolerance nor an eye can
    /// use that.
    #[test]
    fn no_two_sprites_can_show_the_same_colour_anywhere() {
        let atlas = atlas();
        let tolerance = Tolerance::default();

        assert!(
            MIN_SPRITE_COLOUR_DISTANCE > tolerance.max_channel_deviation
                && MIN_SPRITE_COLOUR_DISTANCE > tolerance.channel,
            "the distance this test demands ({MIN_SPRITE_COLOUR_DISTANCE}) has to \
             stay above the golden tolerance — noise floor {}, per-pixel cap {} — \
             or a comparison could absorb one sprite showing another's colour",
            tolerance.channel,
            tolerance.max_channel_deviation
        );

        let palettes: Vec<(&str, Vec<[u8; 4]>)> = SCENE
            .iter()
            .map(|sprite| (sprite.name, colours_in(region_of(*sprite, &atlas), &atlas)))
            .collect();

        for (name, colours) in &palettes {
            println!("{name}: {colours:?}");
            assert!(
                !colours.is_empty(),
                "sprite {name}'s region contains no whole texel, so it has no \
                 colours of its own and nothing below is checking anything"
            );

            for colour in colours {
                let distance = channel_distance(*colour, BACKGROUND);
                assert!(
                    distance >= MIN_SPRITE_COLOUR_DISTANCE,
                    "sprite {name} can show {colour:?}, {distance} counts from the \
                     cleared background {BACKGROUND:?}. A sprite that close to the \
                     background is a sprite whose absence is not visible."
                );
            }
        }

        for (index, (left_name, left)) in palettes.iter().enumerate() {
            for (right_name, right) in palettes.iter().skip(index + 1) {
                for a in left {
                    for b in right {
                        let distance = channel_distance(*a, *b);
                        assert!(
                            distance >= MIN_SPRITE_COLOUR_DISTANCE,
                            "sprite {left_name} can show {a:?} where sprite \
                             {right_name} can show {b:?}, only {distance} counts \
                             apart. Then a pixel those two contest does not say \
                             which of them won it, and neither a probe nor a human \
                             can read the draw order out of the image."
                        );
                    }
                }
            }
        }
    }

    /// The scene is the fixture the probe derivation assumed.
    ///
    /// Runs without a GPU, and it is four separate claims the derivation leans
    /// on: the sprites overlap pairwise, at least three of them overlap at once,
    /// their depths disagree with their spawn order, and their centres are
    /// distinct. Without the overlaps no pixel is contested; without the
    /// disagreement an unsorted extraction would produce the same image as a
    /// sorted one and every probe would pass with no sort at all; without
    /// distinct centres [`scene_sprite_at`] could pair a placement with the
    /// wrong region.
    #[test]
    fn the_scene_is_the_fixture_the_probes_assume() {
        let world = overlapping_world();
        let placements = placements_of(&world);
        assert_eq!(placements.len(), SCENE.len());

        let extents: Vec<(f32, f32, f32, f32)> = placements
            .iter()
            .map(|drawn| {
                let p = drawn.placement;
                (
                    p.x - p.scale_x / 2.0,
                    p.x + p.scale_x / 2.0,
                    p.y - p.scale_y / 2.0,
                    p.y + p.scale_y / 2.0,
                )
            })
            .collect();

        for (i, a) in extents.iter().enumerate() {
            for (j, b) in extents.iter().enumerate().skip(i + 1) {
                let apart = a.1 <= b.0 || b.1 <= a.0 || a.3 <= b.2 || b.3 <= a.2;
                assert!(
                    !apart,
                    "sprites {i} and {j} do not overlap: {a:?} against {b:?}. This \
                     scene exists to make draw order visible, and disjoint sprites \
                     would make every probe pass whatever the order was."
                );
            }
        }

        // The intersection of all three, as a rectangle. Non-empty is what makes
        // the triple-overlap probe a triple-overlap probe.
        let left = extents.iter().map(|e| e.0).fold(f32::MIN, f32::max);
        let right = extents.iter().map(|e| e.1).fold(f32::MAX, f32::min);
        let bottom = extents.iter().map(|e| e.2).fold(f32::MIN, f32::max);
        let top = extents.iter().map(|e| e.3).fold(f32::MAX, f32::min);
        assert!(
            left < right && bottom < top,
            "the three sprites do not all overlap anywhere: the common rectangle \
             is x {left}..{right}, y {bottom}..{top}. The sharpest probe of the \
             scene sits inside it."
        );

        // Read off the centres, which identify the sprites: A is at -16, B at
        // +16, C at 0. `draw_order` above cannot be used here — it reads a
        // marker index out of `x`, and in this scene `x` is a world coordinate.
        let centres: Vec<f32> = placements.iter().map(|p| p.placement.x).collect();
        assert_eq!(
            centres,
            vec![-16.0, 16.0, 0.0],
            "draw order must be A (depth 0), B (depth 1), C (depth 2), which is \
             spawn order 1, 2, 0. If this ever came out in spawn order the scene \
             would no longer distinguish a sorted extraction from an unsorted one."
        );

        for (i, a) in SCENE.iter().enumerate() {
            for b in SCENE.iter().skip(i + 1) {
                assert!(
                    a.x.to_bits() != b.x.to_bits() || a.y.to_bits() != b.y.to_bits(),
                    "sprites {} and {} share the centre ({}, {}), so pairing a \
                     placement back to its region would be a guess",
                    a.name,
                    b.name,
                    a.x,
                    a.y
                );
            }
        }
    }

    /// Each sprite samples its own region, and none samples the whole atlas.
    ///
    /// Runs without a GPU. Without it, "every sprite has a region of its own"
    /// would rest on reading three entries of a table, which is the kind of claim
    /// this project has watched go stale.
    #[test]
    fn the_scene_gives_each_sprite_its_own_region_of_the_atlas() {
        let atlas = atlas();
        let regions: Vec<(&str, TextureRegion)> = SCENE
            .iter()
            .map(|sprite| (sprite.name, region_of(*sprite, &atlas)))
            .collect();

        for (name, region) in &regions {
            assert_ne!(
                region.uv_bounds(),
                TextureRegion::WHOLE_TEXTURE.uv_bounds(),
                "sprite {name} samples the whole atlas, so it would show every \
                 other sprite's colours as well as its own"
            );
        }

        for (index, (left_name, left)) in regions.iter().enumerate() {
            for (right_name, right) in regions.iter().skip(index + 1) {
                assert_ne!(
                    left.uv_bounds(),
                    right.uv_bounds(),
                    "sprites {left_name} and {right_name} share a region, so a swap \
                     between them would be invisible"
                );
            }
        }
    }

    /// Every region of this scene's atlas carries the border it claims.
    ///
    /// GPU-free, and one line because the property lives at the shared fixture
    /// place (D16, ADR-0016).
    ///
    /// **The conditional this comment used to carry is discharged.** It said
    /// `Nearest` reads no border texel, so a border built from the wrong texels
    /// renders a byte-identical `layer_order_regions_128x128` "today" and would
    /// first appear as a colour fringe "under `Linear`". Since M3.28 this scene
    /// *is* drawn at `Linear` ([`overlapping_world`]), and bilinear reaches one
    /// texel past the sample point, so the border is read at every region edge
    /// and a wrong one would move the blessed image. **By how much is not
    /// measured** — nothing in this task rendered a deliberately broken border
    /// against the reference. So the guard has stopped being redundant with the
    /// reference and has not become superfluous either.
    ///
    /// The claim is about this scene, drawn through `render_sprites`, and about
    /// no other image or draw path.
    #[test]
    fn every_region_of_the_atlas_carries_its_border() {
        LAYOUT.assert_every_region_is_padded(&atlas());
    }

    /// The atlas really is laid out the way the probe derivation assumed.
    ///
    /// Runs without a GPU. Each expectation in [`OVERLAP_PROBES`] was computed as
    /// "this pixel samples that texel, and that texel is this colour"; this
    /// asserts the second half against the fixture, so the derivation cannot
    /// quietly stop describing it.
    ///
    /// Written in **content** coordinates since M3.22, put through
    /// [`LAYOUT`]`.content_texel`, with the atlas texel each one lands on given
    /// as the last two columns and asserted rather than recomputed. Without
    /// those columns the table and the fixture would share one layout constant
    /// and move together, so a wrong border width would leave both this test
    /// and the derivation above stale with nothing left to catch it.
    #[test]
    fn the_atlas_puts_each_colour_where_the_derivation_expects_it() {
        let atlas = atlas();

        for (cell_x, cell_y, in_x, in_y, expected, at_x, at_y, what) in [
            (0, 0, 0, 0, A_UPPER, 1, 1, "A's first content texel"),
            (0, 0, 1, 1, A_UPPER, 2, 2, "the (24, 24) probe's texel"),
            (0, 0, 1, 5, A_LOWER, 2, 6, "the (24, 60) probe's texel"),
            (1, 0, 1, 1, B_UPPER, 12, 2, "the (60, 24) probe's texel"),
            (1, 0, 7, 5, B_LOWER, 18, 6, "the (104, 60) probe's texel"),
            (0, 1, 1, 2, C_UPPER, 2, 13, "the (40, 60) probe's texel"),
            (0, 1, 7, 2, C_UPPER, 8, 13, "the (88, 60) probe's texel"),
            (0, 1, 3, 2, C_UPPER, 4, 13, "the (60, 60) probe's texel"),
            (0, 1, 1, 6, C_LOWER, 2, 17, "the (40, 90) probe's texel"),
            (
                1,
                1,
                4,
                4,
                UNUSED_LOWER,
                15,
                15,
                "the quadrant no sprite samples",
            ),
        ] {
            let (x, y) = LAYOUT.content_texel(cell_x, cell_y, in_x, in_y);
            assert_eq!(
                (x, y),
                (at_x, at_y),
                "content texel ({in_x}, {in_y}) of cell ({cell_x}, {cell_y}) is atlas \
                 texel ({at_x}, {at_y}) in this file's derivation"
            );
            let actual = atlas
                .pixel(x, y)
                .unwrap_or_else(|| panic!("({x}, {y}) is outside the atlas"));
            assert_eq!(
                actual, expected,
                "atlas texel ({x}, {y}), {what}: expected {expected:?}, fixture \
                 has {actual:?}"
            );
        }
    }

    #[test]
    fn the_overlapping_scene_draws_back_to_front() {
        let Some(target) = target_or_skip(TARGET_SIZE, TARGET_SIZE) else {
            return;
        };

        let rendered = render_the_overlapping_scene(&target);

        for (x, y, expected, why) in OVERLAP_PROBES {
            let actual = rendered.pixel(x, y).unwrap_or_else(|| {
                panic!("({x}, {y}) is outside the {TARGET_SIZE} x {TARGET_SIZE} target")
            });

            println!("({x}, {y}) {why}: {actual:?}");
            assert_eq!(
                actual, expected,
                "pixel ({x}, {y}), {why}: expected {expected:?}, rendered {actual:?}. \
                 The expectation was derived from the projection, the regions and the \
                 depths before this ran. A contested pixel showing the wrong sprite \
                 means the extraction sorted the wrong way, did not sort, or read the \
                 depth without applying it; an uncontested one showing the wrong colour \
                 means a sprite is sampling somebody else's region."
            );
        }
    }

    /// No pixel of the frame comes from the quadrant no sprite asked for.
    ///
    /// The whole-frame form of "the region was computed and then ignored". A
    /// sprite drawn full-area reaches into all four quadrants, and the fourth is
    /// in no sprite's region, so its two colours cannot legitimately appear
    /// anywhere. This looks at all 16 384 pixels rather than the nine the probes
    /// name, which is the difference between catching the mistake and catching it
    /// where somebody happened to look.
    #[test]
    fn no_pixel_comes_from_the_quadrant_no_sprite_uses() {
        let Some(target) = target_or_skip(TARGET_SIZE, TARGET_SIZE) else {
            return;
        };

        let rendered = render_the_overlapping_scene(&target);

        for y in 0..rendered.height() {
            for x in 0..rendered.width() {
                let pixel = rendered
                    .pixel(x, y)
                    .expect("x and y come from the image's own dimensions");
                assert!(
                    pixel != UNUSED_UPPER && pixel != UNUSED_LOWER,
                    "pixel ({x}, {y}) is {pixel:?}, a colour from the atlas's \
                     bottom-right quadrant. No sprite in this scene uses that \
                     quadrant, so it can only appear if a sprite sampled the whole \
                     atlas instead of its region."
                );
            }
        }
    }

    /// The blessed reference for the overlapping scene.
    ///
    /// **Expected to fail until the maintainer blesses the reference**, as
    /// M3.5's, M3.9's and M3.10's did. Nothing here writes one.
    ///
    /// The failure artifacts go where every other golden test in the repository
    /// puts them, `narvo_render2d::golden_artifact_dir()` — under the cargo
    /// target directory, derived from this binary's own path. M3.10 reached for
    /// `std::env::temp_dir()` instead, because `CARGO_TARGET_TMPDIR` is set only
    /// for integration tests and benches, and this has to be a unit test:
    /// `placements_of` is reachable from nowhere else (ADR-0015). What that
    /// produced was the one artifact only a human can act on, the image to be
    /// blessed, sitting in `%TEMP%` where Windows Search does not index it. The
    /// forced part was the kind of test, never the path.
    /// The click-counter scene, blessed at counter three (M5.5).
    ///
    /// **The whole M5 chain, in one picture.** The world is built here in code
    /// rather than loaded from `scenes/click_counter.ron`, because a fixture
    /// that is a copy of the shipped content would move whenever the content
    /// did; the `.ron` is the game and this is the specification of a state it
    /// can reach. The two are deliberately not the same file.
    ///
    /// The counter is driven to three over the **real** paths, not by writing
    /// `3` into the component: two clicks answered by `hit_test` against the
    /// button's own `HitRect`, and one key press mapped through a real
    /// `Mapping`. All three arrive as `buy 1`, and nothing downstream can tell
    /// which was which — which is the property the scene exists to show.
    const COUNTER_SCENE: &str = "click_counter_state3_128x128";

    /// The digit that has to be legible in the blessed image.
    const COUNTER_TARGET: i64 = 3;

    /// The texture the counter scene samples: glyphs, with the colour strip
    /// appended to their right.
    ///
    /// One texture, because a draw call binds one. `ground_beside` is the same
    /// fixture `text_over_scene` uses and is a documented scoped exception to
    /// hand-transcription.
    fn counter_texture() -> Pixels {
        let glyphs = narvo_testkit::glyph_atlas::rasterize(32.0);
        narvo_testkit::blend::ground_beside(glyphs.pixels()).0
    }

    /// The backdrop's and the button's regions in [`counter_texture`].
    ///
    /// Derived the way `ground_beside` derives its own: the strip starts at the
    /// glyph texture's width and each cell is `CELL_TEXELS` wide. Cell 0 is the
    /// red ground and cell 1 the black one, so the button is red on black.
    fn counter_regions(texture: &Pixels) -> (TextureRegion, TextureRegion) {
        use narvo_testkit::blend::CELL_TEXELS;

        let strip = narvo_testkit::glyph_atlas::rasterize(32.0).pixels().width();
        let height = texture.height();

        let button = TextureRegion::from_texels(strip, 0, CELL_TEXELS, height, texture);
        let backdrop =
            TextureRegion::from_texels(strip + CELL_TEXELS, 0, CELL_TEXELS, height, texture);

        (backdrop, button)
    }

    /// The counter world: a backdrop, a button that can be clicked, a counter.
    fn counter_world() -> World {
        use narvo_ecs::{Events, HitRect, Tally};
        use narvo_input::InputEvent;

        let mut world = World::new();

        let backdrop = world.spawn();
        world
            .insert(
                backdrop,
                Transform {
                    x: 0.0,
                    y: 0.0,
                    rotation: 0.0,
                    scale_x: 128.0,
                    scale_y: 128.0,
                },
            )
            .expect("the entity was just spawned");
        world
            .insert(backdrop, Layer::at(0.0))
            .expect("the entity was just spawned");
        world
            .insert(backdrop, Sprite::new("backdrop"))
            .expect("the entity was just spawned");

        let button = world.spawn();
        world
            .insert(
                button,
                Transform {
                    x: 0.0,
                    y: -32.0,
                    rotation: 0.0,
                    scale_x: 64.0,
                    scale_y: 64.0,
                },
            )
            .expect("the entity was just spawned");
        world
            .insert(button, Layer::at(1.0))
            .expect("the entity was just spawned");
        world
            .insert(button, Sprite::new("button"))
            .expect("the entity was just spawned");
        world
            .insert(button, HitRect::new(32.0, 32.0, "buy", 1))
            .expect("the entity was just spawned");

        let counter = world.spawn();
        world
            .insert(counter, Tally::new("buy"))
            .expect("the entity was just spawned");
        world
            .insert(counter, Events::<InputEvent>::new())
            .expect("the entity was just spawned");

        world
    }

    /// Drives [`counter_world`] to [`COUNTER_TARGET`] over the real paths.
    ///
    /// Two clicks and one key, each through the machinery a window uses, and one
    /// tick per input so the rotation delivers each exactly once (ADR-0011).
    fn counter_world_at_three() -> World {
        use crate::input::InputFeed;
        use narvo_ecs::{HitRect, Scheduler, Tally};
        use narvo_input::{Control, DeviceEvent, InputEvent};
        use narvo_view2d::hit_test;

        let mut world = counter_world();

        let mut scheduler = Scheduler::new();
        scheduler
            .add_system("input/rotate", narvo_ecs::rotate_events::<InputEvent>)
            .expect("the first system by that name");
        scheduler
            .add_system("tally", crate::sim::scene_file::count_actions)
            .expect("the first system by that name");

        let mapping = narvo_input::from_str(
            r#"Mapping(bindings: [(control: Space, action: "buy", emit: OnPress(1))])"#,
        )
        .expect("the mapping is valid");
        let mut feed = InputFeed::new(mapping);

        // Two clicks on the button, at the world point its rectangle covers.
        for tick in 0..2 {
            let hit = hit_test(&world, 0.0, -32.0).expect("the button is under that point");
            let event = {
                let rect = world
                    .get::<HitRect>(hit)
                    .expect("the hit entity carries the rectangle");
                InputEvent::new(&rect.action, rect.value).expect("an identifier")
            };

            feed.push_event(event);
            feed.deliver(&mut world);
            scheduler.run(&mut world, &SystemContext::new(tick));
        }

        // One key press, through the mapping.
        feed.push(DeviceEvent::press(Control::Space));
        feed.deliver(&mut world);
        scheduler.run(&mut world, &SystemContext::new(2));

        let count = world
            .entity_ids()
            .into_iter()
            .find_map(|entity| world.get::<Tally>(entity).ok().map(|tally| tally.count))
            .expect("the world carries a counter");
        assert_eq!(
            count, COUNTER_TARGET,
            "the scene has to reach the target over the real paths, not by assignment"
        );

        world
    }

    /// The counter scene as sprites: the world's two, then the digit.
    fn counter_sprites(world: &World, texture: &Pixels) -> Vec<SpriteInstance> {
        use narvo_ecs::Tally;

        let (backdrop, button) = counter_regions(texture);

        let mut sprites: Vec<SpriteInstance> = regions_of(world)
            .into_iter()
            .map(|drawn| {
                let region = match drawn.region.as_str() {
                    "backdrop" => backdrop,
                    "button" => button,
                    other => panic!("the counter scene has no region called {other:?}"),
                };
                SpriteInstance::new(drawn.placement, region)
                    .sampled(drawn.filter)
                    .tinted(drawn.tint)
            })
            .collect();

        let count = world
            .entity_ids()
            .into_iter()
            .find_map(|entity| world.get::<Tally>(entity).ok().map(|tally| tally.count))
            .expect("the world carries a counter");

        let glyphs = narvo_testkit::glyph_atlas::rasterize(32.0);
        let placed = narvo_testkit::text::layout_line(&count.to_string(), &glyphs, 52.0, 40.0);
        sprites.extend(narvo_testkit::text::sprites_for(&placed, texture, 128, 128));

        sprites
    }

    #[test]
    fn the_click_counter_scene_matches_its_golden_reference() {
        let Some(target) = target_or_skip(TARGET_SIZE, TARGET_SIZE) else {
            return;
        };

        let texture = counter_texture();
        let world = counter_world_at_three();
        let sprites = counter_sprites(&world, &texture);

        let rendered = target
            .render_sprites(&texture, &sprites)
            .expect("a handful of sprites is far inside the batch limit");

        let references = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
        let output = golden_artifact_dir();
        let golden = Golden::new(&references, &output);

        match golden.verify(COUNTER_SCENE, &rendered) {
            Ok(report) => println!(
                "golden match for {COUNTER_SCENE}: {}",
                report.measured_against(golden.tolerance)
            ),
            Err(error) => panic!("{error}"),
        }
    }

    #[test]
    fn the_overlapping_scene_matches_its_golden_reference() {
        let Some(target) = target_or_skip(TARGET_SIZE, TARGET_SIZE) else {
            return;
        };

        let rendered = render_the_overlapping_scene(&target);

        let references = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
        let output = golden_artifact_dir();
        let golden = Golden::new(&references, &output);

        match golden.verify(OVERLAP_SCENE, &rendered) {
            Ok(report) => println!(
                "golden match for \"{OVERLAP_SCENE}\": {}",
                report.measured_against(golden.tolerance)
            ),
            Err(error) => panic!("{error}"),
        }
    }

    /// The one rendered proof of M3.30: a followed camera moves the picture in
    /// quarter-pixel steps.
    ///
    /// **Not a blessed scene and not a candidate for one.** It writes no
    /// reference, reads none, and lives here rather than under `tests/` for the
    /// reason `placements_of` does: `narvo-app` is a binary crate with no
    /// library target, so an integration test cannot reach [`camera_of`], and
    /// reaching it is the point — this exercises the whole seam,
    /// `compose_camera` → [`camera_of`] → `render_sprites_viewed_by`, rather than
    /// a camera the test set by hand.
    ///
    /// # What it measures, and whose method that is
    ///
    /// The motion instrument's method, used rather than rebuilt
    /// (`camera_pan_steps.rs` is untouched): with a **uniform** texture a pixel
    /// is coverage times colour and nothing else, so the stored byte reads the
    /// coverage directly. The file is M3.15's (`8190c50`) and the uniform
    /// fixture is M3.25's — its own header corrects a report that called it
    /// M3.16's, so getting the attribution right here is the least this can do.
    /// At
    /// `SAMPLE_COUNT` = 4 the levels are `[0, 137, 188, 225, 255]` — the sRGB
    /// encodings of 0, ¼, ½, ¾ and 1 resolved in linear light.
    ///
    /// # The derivation, written before it ran
    ///
    /// One sprite 48 world units square at the origin, camera at zoom 1, so it
    /// covers pixels 40..88 and column 39 is empty. The target starts at the
    /// origin and moves `+0.25` world units per tick; the follow is immediate,
    /// so the camera is the target and the sprite's screen position moves
    /// `-0.25` px per tick. Its left edge therefore lands at 40.0, 39.75, 39.5,
    /// 39.25, 39.0, and the four sample offsets 0.125/0.375/0.625/0.875 put
    /// **0, 1, 2, 3 then 4** of column 39's samples inside it.
    ///
    /// So column 39 must read `[0, 137, 188, 225, 255]`, one level per tick.
    /// **That is D14's purchase**, and this is the first time it is bought by a
    /// camera that followed something rather than by a camera set directly.
    #[test]
    fn a_followed_camera_moves_the_picture_in_quarter_pixel_steps() {
        let Some(target) = target_or_skip(TARGET_SIZE, TARGET_SIZE) else {
            return;
        };

        // Uniform, so that coverage is the only thing a pixel can report. Built
        // here rather than taken from the testkit for the reason `camera_pan_steps`
        // gives for its own copy: the fixture is the instrument.
        let texture = Pixels::from_rgba8(8, 8, [255_u8, 255, 255, 255].repeat(8 * 8))
            .expect("the generated buffer matches its dimensions");

        let mut world = World::new();
        let followed = world.spawn();
        world
            .insert(followed, Transform::IDENTITY)
            .expect("the entity was just spawned");
        let eye = world.spawn();
        world
            .insert(eye, Camera::at(0.0, 0.0))
            .expect("the entity was just spawned");
        world
            .insert(eye, Follow::immediate(followed, 0.0, 0.0))
            .expect("the entity was just spawned");

        let sprites = [SpriteInstance::new(
            SpritePlacement {
                x: 0.0,
                y: 0.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 48.0,
                scale_y: 48.0,
            },
            TextureRegion::WHOLE_TEXTURE,
        )];

        let mut seen = Vec::new();
        for tick in 0..=4_u64 {
            if tick > 0 {
                let mut transform = world
                    .get_mut::<Transform>(followed)
                    .expect("the target is alive");
                transform.x += 0.25;
                drop(transform);
                compose_camera(&mut world, &SystemContext::new(tick));
            }

            let view = camera_of(&world);
            let rendered = target
                .render_sprites_viewed_by(&texture, &sprites, view)
                .expect("one sprite is far inside the batch limit");
            let pixel = rendered.pixel(39, 64).expect("39, 64 is inside the target");
            let where_is_it = world
                .get::<Camera>(eye)
                .expect("the eye carries a camera")
                .x;
            println!("tick {tick}: camera x {where_is_it}, column 39 reads {pixel:?}");
            seen.push(pixel[0]);
        }

        // The one count of slack is not politeness, and it is not mine to
        // rediscover: `BASELINE.md` records that llvmpipe reads 187 where AMD
        // reads 188 at the half-coverage level, because sRGB(0.5) is 187.52 of
        // 255 and the two adapters round it opposite ways. `camera_pan_steps`
        // allows exactly this and for exactly this reason. Asserting the bytes
        // rigidly would encode driver-unspecified behaviour, which is the class
        // `ProjektPlan.md` §12 keeps from M3.24.
        let expected = [0_u8, 137, 188, 225, 255];
        for (tick, (&got, &want)) in seen.iter().zip(expected.iter()).enumerate() {
            assert!(
                got.abs_diff(want) <= 1,
                "tick {tick}: column 39 read {got}, expected {want} give or take \
                 the one count sRGB(0.5) costs between adapters. A followed \
                 camera must move the silhouette one coverage level per quarter \
                 pixel; anything else means the motion is being quantised to \
                 whole pixels somewhere between the world and the rasteriser"
            );
        }
        assert_eq!(
            seen.len(),
            expected.len(),
            "one reading per tick, or the loop above did not run"
        );
        // Strictly increasing is the part that has no slack: the levels may each
        // be off by a count, but they may not repeat or go backwards, because
        // that would be a camera that stopped moving.
        assert!(
            seen.windows(2).all(|pair| pair[1] > pair[0]),
            "the coverage levels must rise once per quarter pixel: {seen:?}"
        );
    }

    /// M3.31's rendered witness: an impulse displaces the silhouette by a
    /// quarter-pixel step and gives it back.
    ///
    /// **This is a witness and not a guard**, and the distinction is one M3.30
    /// learned the hard way — an injected drift of a thousandth of a pixel left
    /// a witness like this one green, because it measures a *quantum* and not a
    /// position. The guards for shake are the bit-exact trail and composition
    /// tests in `narvo-ecs`'s `follow.rs` and `shake.rs`. What this adds is the
    /// one thing they cannot: that the composed camera reaches a rasteriser and
    /// moves a real pixel.
    ///
    /// It carries the same one count of adapter slack for the same recorded
    /// reason (`BASELINE.md`: llvmpipe 187 against AMD 188 at half coverage).
    ///
    /// # The derivation, before it ran
    ///
    /// Same geometry as the follow witness above: one uniform sprite of 48
    /// world units at the origin, camera at zoom 1, covering pixels 40..88 with
    /// column 39 empty. A `Shake` of amplitude 1.5, frequency 1.0, decay 0.5 and
    /// cutoff 0.4, around base (0, 0) and with no follow:
    ///
    /// - **tick 1** — amplitude 0.75, phase 1, `triangle(1) = 1`, so the offset
    ///   is `(0.75, 0.0)`. The camera moves right by three quarters of a pixel,
    ///   the sprite's left edge lands at 39.25, and three of column 39's four
    ///   samples fall inside it: **225**.
    /// - **tick 2** — the amplitude would be 0.375, which is at or below the
    ///   cutoff, so the shake ends and the offset is exactly `(0.0, 0.0)`.
    ///   Column 39 is empty again: **0**.
    ///
    /// And the last frame must be **byte-identical** to the frame before the
    /// impulse — not merely similar. That is what "the shake is over" means, and
    /// unlike the coverage bytes it needs no slack at all, because it compares
    /// two renders from the same adapter.
    #[test]
    fn a_shake_impulse_displaces_the_silhouette_and_gives_it_back() {
        let Some(target) = target_or_skip(TARGET_SIZE, TARGET_SIZE) else {
            return;
        };

        let texture = Pixels::from_rgba8(8, 8, [255_u8, 255, 255, 255].repeat(8 * 8))
            .expect("the generated buffer matches its dimensions");
        let sprites = [SpriteInstance::new(
            SpritePlacement {
                x: 0.0,
                y: 0.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 48.0,
                scale_y: 48.0,
            },
            TextureRegion::WHOLE_TEXTURE,
        )];

        let mut world = World::new();
        let eye = world.spawn();
        world
            .insert(eye, Camera::at(0.0, 0.0))
            .expect("the entity was just spawned");
        world
            .insert(eye, Shake::new(1.5, 1.0, 0.5, 0.4))
            .expect("the entity was just spawned");

        let render = |world: &World| {
            target
                .render_sprites_viewed_by(&texture, &sprites, camera_of(world))
                .expect("one sprite is far inside the batch limit")
        };

        let at_rest = render(&world);
        let mut seen = vec![at_rest.pixel(39, 64).expect("inside the target")[0]];
        let mut frames = Vec::new();
        for tick in 1..=2_u64 {
            compose_camera(&mut world, &SystemContext::new(tick));
            let frame = render(&world);
            let pixel = frame.pixel(39, 64).expect("inside the target");
            println!("tick {tick}: column 39 reads {pixel:?}");
            seen.push(pixel[0]);
            frames.push(frame);
        }

        for (index, want) in [0_u8, 225, 0].into_iter().enumerate() {
            assert!(
                seen[index].abs_diff(want) <= 1,
                "reading {index}: column 39 read {}, expected {want} give or take \
                 the count sRGB rounding costs between adapters. Readings: {seen:?}",
                seen[index]
            );
        }

        let after = frames.pop().expect("two frames were rendered");
        assert_eq!(
            after.rgba(),
            at_rest.rgba(),
            "once the shake is over the frame must be byte-identical to the one \
             before the impulse; a camera that does not come all the way back is \
             a shake that never ended"
        );
        assert!(
            world
                .get::<Shake>(eye)
                .expect("the eye carries a shake")
                .at_rest(),
            "the shake should have expired on the second tick"
        );
    }
}
