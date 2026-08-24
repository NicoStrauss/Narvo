//! Instance, adapter and device selection, shared by every render target.
//!
//! Both the offscreen target and the window target need the same thing: an
//! adapter that exists on this machine and a device from it. Keeping that in one
//! place is what stops the window path from becoming a second, differently
//! behaved renderer.

use crate::error::RenderError;

/// The adapter requests that are tried, in order.
///
/// Deliberately does not insist on a discrete GPU. `force_fallback_adapter` asks
/// explicitly for a software implementation such as lavapipe or WARP, which is
/// the rung that keeps headless CI working - see ADR-0004's neighbours in
/// `docs/decisions`, and the CI workflow that installs lavapipe.
const LADDER: [(&str, wgpu::PowerPreference, bool); 3] = [
    (
        "high-performance",
        wgpu::PowerPreference::HighPerformance,
        false,
    ),
    ("no preference", wgpu::PowerPreference::None, false),
    (
        "forced software fallback",
        wgpu::PowerPreference::None,
        true,
    ),
];

/// Walks the adapter ladder and returns the first adapter that answers.
///
/// `compatible_surface` constrains the search to adapters that can present to
/// that surface. The offscreen path passes `None`; the window path must pass its
/// surface, or it can end up with an adapter that cannot draw to the window.
///
/// # Errors
///
/// [`RenderError::NoAdapter`] if every rung failed, listing what was tried.
pub(crate) fn select_adapter<'surface>(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'surface>>,
) -> Result<(&'static str, wgpu::Adapter), RenderError> {
    let mut rejections = Vec::new();

    for (label, power_preference, force_fallback_adapter) in LADDER {
        let request = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference,
            force_fallback_adapter,
            compatible_surface,
            ..Default::default()
        });

        match pollster::block_on(request) {
            Ok(adapter) => return Ok((label, adapter)),
            Err(error) => rejections.push(format!("{label} ({error})")),
        }
    }

    Err(RenderError::NoAdapter {
        attempts: rejections.join(", "),
    })
}

/// Requests a device and queue from `adapter` with wgpu's default limits.
///
/// # Errors
///
/// [`RenderError::NoDevice`] if the adapter refuses, naming the adapter.
pub(crate) fn create_device(
    adapter: &wgpu::Adapter,
) -> Result<(wgpu::Device, wgpu::Queue), RenderError> {
    let info = adapter.get_info();

    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("narvo device"),
        ..Default::default()
    }))
    .map_err(|error| RenderError::NoDevice {
        adapter: format!("{} ({:?})", info.name, info.backend),
        source: Box::new(error),
    })
}

/// Human-readable description of an adapter and how it came to be chosen.
pub(crate) fn summarize(adapter: &wgpu::Adapter, selection: &str) -> String {
    let info = adapter.get_info();
    format!(
        "{} [{:?}, {:?}] chosen by: {selection}",
        info.name, info.backend, info.device_type
    )
}
