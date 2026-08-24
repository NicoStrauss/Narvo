//! The asset contract: source regions in, one packed atlas out.
//!
//! Read ADR-0020 before changing anything here. What a consumer is promised is
//! written there and implemented here; the short form is that a packed atlas is
//! a **function of its regions alone** — not of the order they arrived in, not
//! of the machine, not of the run.

use std::collections::BTreeMap;
use std::fmt;

use narvo_core::sha256::{hex, sha256};

/// Bytes per pixel. RGBA8, which is what the renderer's textures are.
pub const BYTES_PER_PIXEL: usize = 4;

/// The border the packer puts around every region, in texels.
///
/// # Where the number comes from
///
/// Not from here. It is `ceil((k+1)/2)` texels for `k` texels per pixel,
/// derived in `narvo-testkit`'s glyph atlas module for the M3.34 generator and
/// quoted rather than re-derived: at `k ≤ 1` — magnification and 1:1, which is
/// every sampler configuration this engine ships — that is **1**. The same
/// number lives in `narvo_render2d::REGION_PADDING_TEXELS`, and
/// `the_padding_width_agrees_with_the_renderers` asserts the two have not
/// drifted, because this crate cannot depend on that one and a silently
/// different border would produce atlases the renderer's own guard rejects.
///
/// Minification (`k > 1`) needs more and is a named non-goal of ADR-0020: D17
/// carries it as the front where `Linear` needs mipmaps, and a mipmapped atlas
/// changes the question rather than the border.
pub const REGION_PADDING_TEXELS: u32 = 1;

/// The largest atlas the packer will grow to, per side.
///
/// 8192 is the smallest maximum texture dimension every WebGPU adapter must
/// support, so an atlas at or under it is one every target can sample. Growing
/// past it would produce a texture that packs beautifully and cannot be
/// uploaded.
pub const MAX_ATLAS_SIDE: u32 = 8192;

/// One image going in, named.
///
/// Pixels are RGBA8, row-major from the top left — the same layout
/// `narvo_render2d::Pixels` takes, so a consumer moves bytes rather than
/// converting them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRegion {
    name: String,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl SourceRegion {
    /// A region named `name`, `width` × `height`, from `rgba`.
    ///
    /// # Errors
    ///
    /// [`PackError::EmptyRegion`] for a zero side, and
    /// [`PackError::PixelCount`] when the bytes do not match the size — which is
    /// the mistake that would otherwise become a panic deep inside the packer or,
    /// worse, an atlas built from whatever followed in memory.
    pub fn new(
        name: impl Into<String>,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<Self, PackError> {
        let name = name.into();

        if width == 0 || height == 0 {
            return Err(PackError::EmptyRegion {
                name,
                width,
                height,
            });
        }

        let expected = width as usize * height as usize * BYTES_PER_PIXEL;
        if rgba.len() != expected {
            return Err(PackError::PixelCount {
                name,
                width,
                height,
                expected,
                found: rgba.len(),
            });
        }

        Ok(Self {
            name,
            width,
            height,
            rgba,
        })
    }

    /// A region of one solid colour, which is what the pixel probes use.
    ///
    /// # Errors
    ///
    /// Everything [`new`](Self::new) returns.
    pub fn solid(
        name: impl Into<String>,
        width: u32,
        height: u32,
        colour: [u8; 4],
    ) -> Result<Self, PackError> {
        let count = width as usize * height as usize;
        let mut rgba = Vec::with_capacity(count * BYTES_PER_PIXEL);
        for _ in 0..count {
            rgba.extend_from_slice(&colour);
        }

        Self::new(name, width, height, rgba)
    }

    /// What it is called. Unique within one pack.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its width in texels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Its height in texels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Its pixels, RGBA8 row-major from the top left.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// Where one region ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

impl Placement {
    /// Left edge, in texels from the atlas's left.
    #[must_use]
    pub fn left(self) -> u32 {
        self.left
    }

    /// Top edge, in texels from the atlas's top.
    #[must_use]
    pub fn top(self) -> u32 {
        self.top
    }

    /// Width in texels, excluding the padding around it.
    #[must_use]
    pub fn width(self) -> u32 {
        self.width
    }

    /// Height in texels, excluding the padding around it.
    #[must_use]
    pub fn height(self) -> u32 {
        self.height
    }
}

/// A packed atlas: the texture, and where everything in it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atlas {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Name to placement. A `BTreeMap` so iteration is by name and therefore
    /// canonical, which is what the anchor is taken over.
    regions: BTreeMap<String, Placement>,
}

impl Atlas {
    /// The atlas width in texels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The atlas height in texels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The texture, RGBA8 row-major from the top left.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Where `name` ended up, if it is in this atlas.
    #[must_use]
    pub fn region(&self, name: &str) -> Option<Placement> {
        self.regions.get(name).copied()
    }

    /// Every region, by name, in name order.
    pub fn regions(&self) -> impl ExactSizeIterator<Item = (&str, Placement)> {
        self.regions
            .iter()
            .map(|(name, place)| (name.as_str(), *place))
    }

    /// How many regions it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// The bytes the SHA-256 anchor is taken over.
    ///
    /// **Pixels and table together, in one fixed order** — the shape M3.34's
    /// glyph anchor established, for the reason it gives: either half alone
    /// misses what the other half is for. A moved region changes no pixel if the
    /// pixels happen to be identical, and a recoloured region moves no
    /// placement; an anchor over one of them would sleep through the other.
    ///
    /// Every number goes in little-endian, and the name goes in as its bytes
    /// preceded by its length. The length matters: without it `"ab"` + `"c"` and
    /// `"a"` + `"bc"` would produce the same blob, so two different atlases
    /// would share an anchor.
    #[must_use]
    pub fn anchor_bytes(&self) -> Vec<u8> {
        let mut blob = Vec::new();

        blob.extend_from_slice(&self.width.to_le_bytes());
        blob.extend_from_slice(&self.height.to_le_bytes());
        blob.extend_from_slice(&self.rgba);

        // The count before the entries, so a truncated table cannot hash like a
        // shorter one that was written on purpose.
        let count = u32::try_from(self.regions.len()).unwrap_or(u32::MAX);
        blob.extend_from_slice(&count.to_le_bytes());

        for (name, place) in &self.regions {
            let length = u32::try_from(name.len()).unwrap_or(u32::MAX);
            blob.extend_from_slice(&length.to_le_bytes());
            blob.extend_from_slice(name.as_bytes());
            blob.extend_from_slice(&place.left.to_le_bytes());
            blob.extend_from_slice(&place.top.to_le_bytes());
            blob.extend_from_slice(&place.width.to_le_bytes());
            blob.extend_from_slice(&place.height.to_le_bytes());
        }

        blob
    }

    /// The anchor itself, as lower-case hex.
    #[must_use]
    pub fn anchor(&self) -> String {
        hex(&sha256(&self.anchor_bytes()))
    }

    /// Which of its regions are the frames of an animation (M6b.5).
    ///
    /// A convenience over [`clips`](crate::clips), which is the function that
    /// decides anything; this only hands it the names. It is here because the
    /// holder of an atlas is the caller most likely to want the answer, and
    /// because the free function's argument is *names* rather than an atlas —
    /// which is what lets a consumer holding some other region table (an
    /// `narvo_view2d::SceneAtlas`, say) ask the same question without this
    /// crate learning that the other one exists.
    ///
    /// Nothing about the packed table changes by asking.
    #[must_use]
    pub fn clips(&self) -> BTreeMap<String, crate::Clip> {
        crate::clips(self.regions.keys().map(String::as_str))
    }
}

/// Packs `regions` into one atlas.
///
/// # The order they arrive in does not matter
///
/// The packer sorts by name before it places anything, so the same set in a
/// different order produces the same atlas — the same placements, the same
/// bytes, the same anchor. `permuting_the_input_does_not_move_anything` is the
/// property; ADR-0020 argues the decision and names what it gives up.
///
/// Names are unique within a pack, which is what makes the key total.
///
/// # How it packs
///
/// Shelves: regions in name order, each placed to the right of the last on the
/// current shelf, a region that does not fit starting a new shelf whose height
/// is its own. The same algorithm the glyph atlas uses, and for the same reason
/// — it is the simplest one that carries the consumers, and nothing measured
/// about the result feeds back into a placement.
///
/// The atlas is square and a power of two, grown by doubling from 1 until
/// everything fits or [`MAX_ATLAS_SIDE`] is passed.
///
/// # Errors
///
/// [`PackError`], naming the numbers that explain the refusal.
pub fn pack(regions: Vec<SourceRegion>) -> Result<Atlas, PackError> {
    let mut sorted = regions;
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    for pair in sorted.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(PackError::DuplicateName {
                name: pair[0].name.clone(),
            });
        }
    }

    let border = REGION_PADDING_TEXELS;
    let footprint = |region: &SourceRegion| -> Option<(u32, u32)> {
        Some((
            region.width.checked_add(2 * border)?,
            region.height.checked_add(2 * border)?,
        ))
    };

    // A region whose own footprint cannot fit is refused before any layout is
    // attempted, so the message is about that region rather than about a total.
    for region in &sorted {
        let Some((width, height)) = footprint(region) else {
            return Err(PackError::RegionTooLarge {
                name: region.name.clone(),
                width: region.width,
                height: region.height,
                border,
                limit: MAX_ATLAS_SIDE,
            });
        };
        if width > MAX_ATLAS_SIDE || height > MAX_ATLAS_SIDE {
            return Err(PackError::RegionTooLarge {
                name: region.name.clone(),
                width: region.width,
                height: region.height,
                border,
                limit: MAX_ATLAS_SIDE,
            });
        }
    }

    let mut side = 1_u32;
    loop {
        if let Some(placements) = lay_out(&sorted, side, border) {
            return Ok(paint(&sorted, &placements, side, border));
        }
        if side >= MAX_ATLAS_SIDE {
            let used: u64 = sorted
                .iter()
                .map(|region| {
                    u64::from(region.width + 2 * border) * u64::from(region.height + 2 * border)
                })
                .sum();
            return Err(PackError::DoesNotFit {
                regions: sorted.len(),
                padded_area: used,
                limit: MAX_ATLAS_SIDE,
                limit_area: u64::from(MAX_ATLAS_SIDE) * u64::from(MAX_ATLAS_SIDE),
            });
        }
        side *= 2;
    }
}

/// Tries to lay `regions` out on a `side` × `side` atlas, in the given order.
///
/// `None` when they do not fit. Pure: it reads sizes and returns coordinates,
/// so the caller can try a size and discard the attempt without side effects.
fn lay_out(regions: &[SourceRegion], side: u32, border: u32) -> Option<Vec<Placement>> {
    let mut placements = Vec::with_capacity(regions.len());

    // The shelf being filled: where its content row starts, how tall it is, and
    // how far along it we are. All three in outer coordinates, borders included.
    let mut shelf_top = 0_u32;
    let mut shelf_height = 0_u32;
    let mut pen = 0_u32;

    for region in regions {
        let width = region.width + 2 * border;
        let height = region.height + 2 * border;

        if pen + width > side {
            // Next shelf. The one just closed keeps its height, so nothing
            // placed on it can be overlapped by what follows.
            shelf_top = shelf_top.checked_add(shelf_height)?;
            shelf_height = 0;
            pen = 0;
        }
        if pen + width > side || shelf_top.checked_add(height)? > side {
            return None;
        }

        placements.push(Placement {
            left: pen + border,
            top: shelf_top + border,
            width: region.width,
            height: region.height,
        });

        pen += width;
        shelf_height = shelf_height.max(height);
    }

    Some(placements)
}

/// Writes the regions and their padding into a fresh atlas.
///
/// Padding is **edge extension**: every border texel is a copy of the content
/// texel nearest to it, clamped on each axis. That is not a choice made here —
/// it is what `narvo_render2d::check_region_padding` verifies, and the packer
/// exists to satisfy that guard by construction.
fn paint(regions: &[SourceRegion], placements: &[Placement], side: u32, border: u32) -> Atlas {
    let mut rgba = vec![0_u8; side as usize * side as usize * BYTES_PER_PIXEL];
    let mut table = BTreeMap::new();

    for (region, place) in regions.iter().zip(placements) {
        // The padded footprint, painted in one pass: for every texel of it, the
        // source texel is the content texel nearest to it. Inside the content
        // that is the texel itself, so one loop covers both.
        let outer_left = place.left - border;
        let outer_top = place.top - border;
        let outer_right = place.left + place.width + border;
        let outer_bottom = place.top + place.height + border;

        for y in outer_top..outer_bottom {
            for x in outer_left..outer_right {
                let source_x = x.clamp(place.left, place.left + place.width - 1) - place.left;
                let source_y = y.clamp(place.top, place.top + place.height - 1) - place.top;

                let from = (source_y as usize * region.width as usize + source_x as usize)
                    * BYTES_PER_PIXEL;
                let into = (y as usize * side as usize + x as usize) * BYTES_PER_PIXEL;
                rgba[into..into + BYTES_PER_PIXEL]
                    .copy_from_slice(&region.rgba[from..from + BYTES_PER_PIXEL]);
            }
        }

        table.insert(region.name.clone(), *place);
    }

    Atlas {
        width: side,
        height: side,
        rgba,
        regions: table,
    }
}

/// Why a set of regions could not become an atlas.
///
/// Every variant carries the numbers that explain the refusal, which is M4.2's
/// standard for a message: an author who is told "does not fit" learns nothing,
/// and one who is told how much was asked for against how much there is knows
/// whether to shrink an icon or split an atlas.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PackError {
    /// Two regions share a name.
    DuplicateName {
        /// The contested name.
        name: String,
    },
    /// A region has a zero side.
    EmptyRegion {
        /// What it is called.
        name: String,
        /// Its width.
        width: u32,
        /// Its height.
        height: u32,
    },
    /// A region's pixels do not match its size.
    PixelCount {
        /// What it is called.
        name: String,
        /// Its width.
        width: u32,
        /// Its height.
        height: u32,
        /// How many bytes that size needs.
        expected: usize,
        /// How many were given.
        found: usize,
    },
    /// One region, padded, is larger than any atlas may be.
    RegionTooLarge {
        /// What it is called.
        name: String,
        /// Its width.
        width: u32,
        /// Its height.
        height: u32,
        /// The border added on each side.
        border: u32,
        /// The largest side an atlas may have.
        limit: u32,
    },
    /// Every region fits on its own, and the set does not fit together.
    DoesNotFit {
        /// How many regions were asked for.
        regions: usize,
        /// Their total area including padding, in texels.
        padded_area: u64,
        /// The largest side an atlas may have.
        limit: u32,
        /// That atlas's area, in texels.
        limit_area: u64,
    },
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { name } => write!(
                f,
                "two regions are both called \"{name}\"; a name identifies exactly one region in \
                 an atlas, and it is what the packer sorts by, so a repeat would make the layout \
                 depend on which of the two came first"
            ),
            Self::EmptyRegion {
                name,
                width,
                height,
            } => write!(
                f,
                "region \"{name}\" is {width}x{height} and has no texels; an empty region has \
                 nothing to place and no edge to pad against"
            ),
            Self::PixelCount {
                name,
                width,
                height,
                expected,
                found,
            } => write!(
                f,
                "region \"{name}\" says it is {width}x{height}, which is {expected} bytes of \
                 RGBA8, and carries {found}"
            ),
            Self::RegionTooLarge {
                name,
                width,
                height,
                border,
                limit,
            } => write!(
                f,
                "region \"{name}\" is {width}x{height}, which with a {border}-texel border on \
                 each side needs {}x{} - and an atlas may be at most {limit} on a side. Shrink \
                 the region; no atlas can hold it",
                width + 2 * border,
                height + 2 * border
            ),
            Self::DoesNotFit {
                regions,
                padded_area,
                limit,
                limit_area,
            } => write!(
                f,
                "{regions} regions need {padded_area} texels once padded, and the largest atlas \
                 is {limit}x{limit} = {limit_area}. Every region fits on its own, so this is the \
                 set being too big rather than any one of them; split it across atlases"
            ),
        }
    }
}

impl std::error::Error for PackError {}
