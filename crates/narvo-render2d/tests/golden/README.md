# Golden reference images

The PNGs in this directory are the blessed output of the renderer. Tests compare
what they render against them, within the tolerance described in
`narvo_render2d::Tolerance`.

## The rule

**These files are maintainer-owned. Automated changes to them are not allowed.**

An agent or contributor working on the renderer must not create, overwrite or
delete anything in this directory — not even when a test is red, and not even
when the new output looks better. A red golden-image test means, first and
always, that **the renderer changed**. Investigate that before doubting the
reference.

If a reference really is out of date — because a change to the renderer was
intended and reviewed — say so, and hand the maintainer:

- the path of the rendered image that a failing run wrote,
- the path of the diff image, and
- what changed in the renderer and why the new output is the correct one.

The maintainer looks at the images and blesses the new reference. That human
step is the entire point: a self-blessed reference asserts only that the
renderer still agrees with whatever it happened to produce, which is not a test.

## Why the images matter beyond their colours

A reference image freezes the orientation convention of
[ADR-0004](../../../../docs/decisions/ADR-0004-orientation-conventions.md) as
firmly as it freezes the pixel values: NDC y up, framebuffer and image rows
running down from a top-left origin. Changing that convention invalidates every
file here at once, which is why ADR-0004 only permits it together with a
regeneration of all of them.

## Failure artifacts

Nothing in this directory is written by a test run. When a comparison fails, the
rendered image and a diff are written under the crate's Cargo target directory
instead, and the failure message names the exact paths. That location is covered
by the repository's existing `/target/` ignore rule, so a failing run cannot
leave anything behind that could be committed by accident.

In the diff image, magenta marks pixels that exceed the tolerance and everything
else is a dimmed greyscale of the reference, so the shape of a regression is
visible without reading coordinates.

Bless the golden reference for the sprite atlas scene (M3.9)

sprite_atlas_regions_128x128: three sprites, three regions of one 20 x 20
atlas texture, on a 128 x 128 target. Looked at and accepted: three
rectangles, each two shades of one colour, light above dark, and no yellow
anywhere.