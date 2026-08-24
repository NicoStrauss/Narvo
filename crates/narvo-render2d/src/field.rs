//! A field: one texture a compute pass reads and another one writes.
//!
//! Internal. Nothing here is part of the crate's public API and no `wgpu` type
//! leaves it, which is the same boundary [`crate::quad`] keeps.
//!
//! # What a field is for
//!
//! M8.3 needs a distance field computed by jump flooding, M8.4 marches a ray
//! against it, M8.5 stores probe cascades and merges them, and M8.6 writes the
//! result back into the albedo. All four move a screen-sized buffer of scalars
//! from one compute pass to the next, and none of them is a draw. That buffer is
//! a [`Field`]; the alternation between two of them is a [`FieldPair`].
//!
//! # Why two textures and not one
//!
//! A pass that read and wrote one texture would be reading texels its own
//! workgroups may not have written yet — the result would depend on the
//! scheduler, which is exactly what §4's measurement rules out for anything this
//! engine computes (see [`crate::compute`]'s header for the rule and the
//! numbers). Two textures make that unrepresentable: [`FieldPair::read`] and
//! [`FieldPair::write`] never name the same one, and the swap between passes is
//! the only thing that moves.

use crate::error::RenderError;

/// The format every field is created with.
///
/// **`Rgba32Float`, measured rather than chosen from the specification.** The
/// M8.2 probe built a `texture_storage_2d<..., write>` pipeline and a texture
/// for twelve candidate formats on eight adapter/backend pairs — AMD RX 9070 XT
/// and an integrated AMD part on Vulkan, DX12 and GL, WARP on DX12, and lavapipe
/// under WSL on Vulkan and GL. Three results decided this:
///
/// - **`Rgba8UnormSrgb` cannot be a storage texture at all.** Every one of the
///   eight reported `STORAGE_BINDING` absent from its allowed usages. That is
///   also the offscreen target's `TARGET_FORMAT`, so the format a field uses was
///   never going to be the format the scene is drawn in.
/// - **`Rg32Float` — the obvious jump-flooding format, two channels for one seed
///   coordinate — is refused on both GL adapters**, at pipeline creation:
///   "WriteOnly access to storage textures with format Rg32Float is not
///   supported". It works on Vulkan and DX12. A format that depends on the
///   backend is a format that depends on the machine.
/// - **`Rgba32Float` was accepted on all eight**, with the usage set below.
///
/// `Rgba16Float` was accepted on all eight too and is half the size. It is not
/// used because a half-float is exact only for integers up to 2048, and M8.3
/// stores a *pixel coordinate* per texel: a field wider than 2048 would round
/// its own seed positions. Four `f32` channels are exact to 2^24 and hold a
/// radiance or an albedo just as well, so one format serves all four consumers.
///
/// The price is named rather than hidden: **sixteen bytes a texel**, four times
/// `Rgba8Unorm`. One field at 1920 x 1080 is 33.2 MB and a pair is 66.4 MB —
/// the number to weigh when M8.5 asks for a cascade per level.
pub(crate) const FIELD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// Channels in one texel of [`FIELD_FORMAT`].
pub(crate) const FIELD_CHANNELS: usize = 4;

/// What every field is created with, and deliberately no more.
///
/// `STORAGE_BINDING` so a pass can write it, `TEXTURE_BINDING` so the next pass
/// can `textureLoad` it, and the two copy usages so a test can seed one with
/// `write_texture` and read it back.
///
/// **`RENDER_ATTACHMENT` is absent, and its absence is measured rather than
/// tidy.** No consumer needs it: M8.3 to M8.6 are compute passes, so nothing
/// draws into a field with a raster pass. Asking for it anyway would cost
/// something real — the same probe found `Rgba32Float` *rejected* with
/// `RENDER_ATTACHMENT` added on GL under lavapipe ("Texture usages
/// TextureUsages(...) are not allowed"), while the four usages below were
/// accepted there. So the flag nobody needs is the flag that would have made the
/// format machine-dependent again.
///
/// If a later slice wants a raster pass to draw into a field, this is where that
/// reopens — and it reopens the format question with it.
pub(crate) const FIELD_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::STORAGE_BINDING
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::COPY_DST);

/// A screen-sized buffer of scalars that a compute pass reads or writes.
///
/// Holds its own device and queue, both of which are handle types `wgpu` clones
/// cheaply, so seeding and reading back need no second object to be threaded
/// through. That is the same shape [`crate::OffscreenTarget`] already has.
#[derive(Debug)]
pub(crate) struct Field {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Field {
    /// Creates a `width` x `height` field on `device`.
    ///
    /// # Errors
    ///
    /// [`RenderError::InvalidSize`] if a dimension is zero or above
    /// [`crate::OffscreenTarget::MAX_DIMENSION`]. Checked here rather than left
    /// to the driver, for the reason `OffscreenTarget::with_format` gives.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        label: &str,
    ) -> Result<Self, RenderError> {
        let max = crate::OffscreenTarget::MAX_DIMENSION;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(RenderError::InvalidSize { width, height, max });
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            // One. A field is never multisampled: a storage texture cannot be,
            // and there is nothing to antialias in a buffer of scalars.
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FIELD_FORMAT,
            usage: FIELD_USAGE,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            texture,
            view,
            width,
            height,
        })
    }

    /// Width in texels.
    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    /// Height in texels.
    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    /// The view a bind group binds, as either side of a pass.
    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Fills the field with `texels`, four `f32` per texel, row-major from the
    /// top left.
    ///
    /// This is how a pattern gets in without anything having been drawn, which
    /// is what makes a multi-pass chain checkable at all: a value that no sprite,
    /// no blend and no sampler could have produced goes in one end, and whatever
    /// comes out the other end came out of the chain rather than out of a
    /// rendering.
    ///
    /// # Errors
    ///
    /// [`RenderError::FieldTexelCount`] if `texels` is not exactly
    /// `width * height * 4` long.
    pub(crate) fn write(&self, texels: &[f32]) -> Result<(), RenderError> {
        let expected = self.width as usize * self.height as usize * FIELD_CHANNELS;
        if texels.len() != expected {
            return Err(RenderError::FieldTexelCount {
                width: self.width,
                height: self.height,
                expected,
                actual: texels.len(),
            });
        }

        // Native byte order, and the same on the way back out in `read_back`.
        // Both platforms this project verifies on are little-endian, so this is
        // a round trip through one representation rather than a claim about two.
        let bytes: Vec<u8> = texels
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * Self::BYTES_PER_TEXEL),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        // `write_texture` is queued rather than immediate. Submitting nothing is
        // what flushes it, and a pass encoded afterwards would flush it too; the
        // explicit submit here means a caller that only writes and reads back has
        // the same guarantee as one that runs a chain in between.
        self.queue.submit(std::iter::empty());
        Ok(())
    }

    /// Copies the field back to the CPU as `width * height * 4` floats.
    ///
    /// # Errors
    ///
    /// [`RenderError::Readback`] if the device could not be polled, the transfer
    /// buffer could not be mapped, or the mapping could not be read — the same
    /// three steps [`crate::offscreen::read_back_texture`] names.
    pub(crate) fn read_back(&self) -> Result<Vec<f32>, RenderError> {
        // The same 256-byte row alignment `read_back_texture` handles, over a
        // sixteen-byte texel rather than a four-byte one. Not shared with that
        // function because it returns `Pixels`, which is RGBA8 by construction
        // and cannot carry a float.
        let unpadded_bytes_per_row = self.width * Self::BYTES_PER_TEXEL;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

        let transfer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("narvo field read-back transfer"),
            size: u64::from(padded_bytes_per_row) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo field read-back"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &transfer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = transfer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|error| RenderError::Readback {
                step: "waiting for the GPU to finish the field copy",
                source: Box::new(error),
            })?;

        match receiver.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(RenderError::Readback {
                    step: "mapping the field transfer buffer",
                    source: Box::new(error),
                });
            }
            Err(error) => {
                return Err(RenderError::Readback {
                    step: "waiting for the field mapping callback after a blocking poll",
                    source: Box::new(error),
                });
            }
        }

        let mapped = slice
            .get_mapped_range()
            .map_err(|error| RenderError::Readback {
                step: "reading the mapped field transfer buffer",
                source: Box::new(error),
            })?;

        let unpadded = unpadded_bytes_per_row as usize;
        let padded = padded_bytes_per_row as usize;
        let mut values =
            Vec::with_capacity(self.width as usize * self.height as usize * FIELD_CHANNELS);
        for row in mapped.chunks_exact(padded) {
            for scalar in row[..unpadded].chunks_exact(4) {
                values.push(f32::from_ne_bytes([
                    scalar[0], scalar[1], scalar[2], scalar[3],
                ]));
            }
        }
        drop(mapped);
        transfer.unmap();

        Ok(values)
    }

    /// Bytes one texel of [`FIELD_FORMAT`] occupies.
    const BYTES_PER_TEXEL: u32 = 16;
}

/// Two fields, one read and one written, swapped between passes.
///
/// **The ping-pong, and the whole of it.** M8.3's jump flooding runs log2(n)
/// passes over one buffer, each reading what the last one wrote; M8.5's cascade
/// merge does the same over its levels. Both are this, and neither needs
/// anything more general — a render graph with aliased resources and derived
/// barriers was weighed and rejected (v1.51), because it has no consumer here
/// that would exercise its generality and would be measured against a toy.
#[derive(Debug)]
pub(crate) struct FieldPair {
    fields: [Field; 2],
    /// Index of the field a pass **reads**. The other one is written.
    front: usize,
}

impl FieldPair {
    /// Creates two `width` x `height` fields on `device`.
    ///
    /// # Errors
    ///
    /// As [`Field::new`].
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        label: &str,
    ) -> Result<Self, RenderError> {
        Ok(Self {
            fields: [
                Field::new(device, queue, width, height, &format!("{label} a"))?,
                Field::new(device, queue, width, height, &format!("{label} b"))?,
            ],
            front: 0,
        })
    }

    /// The field the next pass reads.
    pub(crate) fn read(&self) -> &Field {
        &self.fields[self.front]
    }

    /// The field the next pass writes.
    ///
    /// **Never the same object [`Self::read`] returns**, which is the structural
    /// half of the guarantee that a pass does not read what it is writing. The
    /// arithmetic `1 - front` says so for every value `front` can hold, and
    /// `front` is only ever moved by [`Self::swap`].
    pub(crate) fn write(&self) -> &Field {
        &self.fields[1 - self.front]
    }

    /// Makes what was written the thing the next pass reads.
    ///
    /// Called once per pass by [`crate::compute::FieldKernel::run`], between the
    /// passes rather than inside one: a pass is over when its
    /// `wgpu::ComputePass` is dropped, and the encoder orders what follows after
    /// it.
    pub(crate) fn swap(&mut self) {
        self.front = 1 - self.front;
    }

    /// Width in texels. Both fields have it.
    pub(crate) fn width(&self) -> u32 {
        self.fields[0].width()
    }

    /// Height in texels. Both fields have it.
    pub(crate) fn height(&self) -> u32 {
        self.fields[0].height()
    }
}

/// Four floats per texel, filled with values a rendering could not make.
///
/// **The injected pattern of §2, and every value in it is impossible on the draw
/// path.** The first two channels are exact integers of a thousand and two
/// thousand plus the texel's own coordinate; the last two are negative. Nothing
/// this crate draws can produce any of them: the render targets are `…UnormSrgb`
/// and clamp to `0.0..=1.0`, premultiplied `OVER` never leaves that range
/// (ADR-0023), and a nearest sampler returns a texel rather than inventing one.
/// So a field holding these values holds something that was *put* there, and a
/// chain that hands them back moved data rather than rendering it.
///
/// Shared between this module's round-trip test and `compute.rs`'s multi-pass
/// oracle, so the two speak about one pattern.
#[cfg(test)]
pub(crate) fn pattern(width: u32, height: u32) -> Vec<f32> {
    let mut texels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            texels.push(1000.0 + f32::from(u16::try_from(x).expect("test sizes are small")));
            texels.push(2000.0 + f32::from(u16::try_from(y).expect("test sizes are small")));
            texels.push(-1.0);
            texels.push(-2.0);
        }
    }
    texels
}

#[cfg(test)]
mod tests {
    use super::{FIELD_FORMAT, FIELD_USAGE, Field, pattern};
    use crate::{OffscreenTarget, RenderError};

    /// A target to borrow a device from, or `None` on a machine with no adapter.
    fn device_or_skip() -> Option<OffscreenTarget> {
        match OffscreenTarget::new(8, 8) {
            Ok(target) => Some(target),
            Err(RenderError::NoAdapter { .. }) => None,
            Err(other) => {
                panic!("the offscreen target failed for a reason that is not absence: {other}")
            }
        }
    }

    /// The format is the one the M8.2 probe measured, not whatever happened to
    /// compile.
    ///
    /// A literal against a literal, which is worth exactly one thing: a session
    /// that changes the format has to change this line too, and then has to say
    /// in the commit message which eight adapters it re-measured.
    #[test]
    fn a_field_is_rgba32float_and_carries_no_render_attachment() {
        assert_eq!(FIELD_FORMAT, wgpu::TextureFormat::Rgba32Float);
        assert!(FIELD_USAGE.contains(wgpu::TextureUsages::STORAGE_BINDING));
        assert!(FIELD_USAGE.contains(wgpu::TextureUsages::TEXTURE_BINDING));
        assert!(FIELD_USAGE.contains(wgpu::TextureUsages::COPY_SRC));
        assert!(FIELD_USAGE.contains(wgpu::TextureUsages::COPY_DST));
        assert!(
            !FIELD_USAGE.contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
            "a field asked for RENDER_ATTACHMENT, which lavapipe's GL backend \
             refuses for this format — the measurement is in this module's header"
        );
    }

    /// A field refuses a size it cannot be, before any GPU work.
    #[test]
    fn a_field_refuses_a_size_it_cannot_be() {
        let Some(target) = device_or_skip() else {
            return;
        };
        let too_big = OffscreenTarget::MAX_DIMENSION + 1;
        for (width, height) in [(0, 8), (8, 0), (too_big, 8), (8, too_big)] {
            let outcome = target.field(width, height, "narvo test field");
            assert!(
                matches!(outcome, Err(RenderError::InvalidSize { .. })),
                "a {width} x {height} field was not refused"
            );
        }
    }

    /// What goes in with `write` comes back out with `read_back`, unchanged.
    ///
    /// The round trip on its own, with no pass in between — so that when the
    /// multi-pass oracle in `compute.rs` fails, this test says whether the
    /// transport or the seeding is at fault.
    ///
    /// **Nine texels wide on purpose.** `9 * 16 = 144` bytes a row, which is not
    /// a multiple of `COPY_BYTES_PER_ROW_ALIGNMENT`, so the read-back has to
    /// strip padding. A width of sixteen would have hidden that.
    #[test]
    fn a_field_hands_back_the_floats_it_was_given() {
        let Some(target) = device_or_skip() else {
            return;
        };
        let field = target
            .field(9, 3, "narvo round trip")
            .expect("a 9 x 3 field");
        let seeded = pattern(9, 3);

        field.write(&seeded).expect("the buffer matches the field");
        let read = field.read_back().expect("the read-back succeeds");

        assert_eq!(read.len(), seeded.len());
        assert_eq!(
            read, seeded,
            "a float changed on the way through the GPU, with nothing having \
             touched it in between"
        );
    }

    /// A buffer of the wrong length is refused rather than silently truncated.
    #[test]
    fn a_field_refuses_a_buffer_that_is_not_its_own_size() {
        let Some(target) = device_or_skip() else {
            return;
        };
        let field = target
            .field(4, 4, "narvo size guard")
            .expect("a 4 x 4 field");

        let outcome = field.write(&vec![0.0; 4 * 4 * 4 - 1]);
        let Err(RenderError::FieldTexelCount {
            expected, actual, ..
        }) = outcome
        else {
            panic!("a short buffer was accepted");
        };
        assert_eq!((expected, actual), (64, 63));
    }

    /// The pair never hands out one field as both sides, and the swap is what
    /// moves.
    ///
    /// No device: this is arithmetic over an index, and the point is that it
    /// holds without one.
    #[test]
    fn the_two_sides_of_a_pair_are_never_the_same_field() {
        let Some(target) = device_or_skip() else {
            return;
        };
        let mut pair = target.field_pair(4, 4, "narvo pair").expect("a 4 x 4 pair");

        let first_read = std::ptr::from_ref::<Field>(pair.read());
        let first_write = std::ptr::from_ref::<Field>(pair.write());
        assert_ne!(
            first_read, first_write,
            "a pass would have read the very texture it writes"
        );

        pair.swap();
        assert_eq!(
            std::ptr::from_ref::<Field>(pair.read()),
            first_write,
            "the swap did not make what was written the thing the next pass reads"
        );
        assert_eq!(
            std::ptr::from_ref::<Field>(pair.write()),
            first_read,
            "the swap did not free the field that was just read"
        );

        pair.swap();
        assert_eq!(
            std::ptr::from_ref::<Field>(pair.read()),
            first_read,
            "two swaps are not the identity"
        );
    }
}
