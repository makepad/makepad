//! Native Linux display inventory for the direct (DRM/KMS) Vulkan backend.
//!
//! The direct backend drives one logical desktop: the primary connector's
//! native pixels divided by the global DPI factor. Every other connector on
//! the same GPU clones that desktop, letterboxed to its own aspect ratio.
//! This module is the UI-facing contract: the window manager reads the
//! snapshot to list displays and their state; it never touches Vulkan.
//!
//! Outside the direct Vulkan backend (Wayland/X11 builds, hosted children)
//! the snapshot is empty with `direct == false`.

/// One DRM connector as the renderer currently sees it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LinuxDisplayOutput {
    /// The sysfs connector name, e.g. `card1-DP-2` or `card0-eDP-1`.
    pub name: String,
    /// Selected mode width in native pixels (0 when no mode was selected).
    pub width: u32,
    /// Selected mode height in native pixels (0 when no mode was selected).
    pub height: u32,
    /// Selected mode refresh in Hz (0.0 when no mode was selected).
    pub refresh_hz: f64,
    /// This connector defines the logical desktop size.
    pub primary: bool,
    /// The connector has a live swapchain and its last presentation succeeded.
    pub active: bool,
    /// Human-readable state: `active`, `unsupported: …`, `failed: …`, `lost: …`.
    /// Failure text is the actual Vulkan/DRM error, never a guess from sysfs.
    pub status: String,
}

/// Everything the renderer knows about connected displays.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LinuxDisplaySnapshot {
    /// `true` only while the direct Vulkan backend owns the displays.
    pub direct: bool,
    /// Outputs in primary-preference order: built-in panels first, then by name.
    pub outputs: Vec<LinuxDisplayOutput>,
}

/// Environment variable carrying the direct renderer's Vulkan device UUID
/// (32 lowercase hex digits). The direct backend sets it in its own process
/// before `Event::Startup`, so child processes spawned with an inherited
/// environment pick the same GPU for their offscreen renderer instead of the
/// discrete-first default. A host that scrubs or rebuilds the child
/// environment must forward this variable explicitly.
pub const LINUX_VULKAN_DEVICE_ENV: &str = "MAKEPAD_VULKAN_DEVICE_UUID";

#[cfg(all(not(gpusim), linux_direct, use_vulkan))]
impl crate::cx::Cx {
    /// The direct backend's current display inventory. Cheap: it copies the
    /// renderer's bookkeeping, no Vulkan or DRM queries happen here.
    pub fn linux_display_snapshot(&self) -> LinuxDisplaySnapshot {
        self.os
            .vulkan
            .as_ref()
            .map(|vulkan| vulkan.direct_display_snapshot())
            .unwrap_or_default()
    }

    /// Ask the renderer to render natively for the named output ("optimize
    /// for"): its native mode becomes the composition resolution and every
    /// other output clones that frame through the GPU blit. `name` is an
    /// `outputs[].name` from the snapshot.
    ///
    /// `Ok` means the request was accepted and queued, not applied: the
    /// renderer switches at its next safe frame boundary (no mode setting or
    /// GPU waits happen inside the caller's event handler). Once applied, the
    /// snapshot marks that output `primary`, the main window's geometry
    /// changes through the ordinary `WindowGeomChange` event at the same
    /// effective DPI, and the preferred name is kept so the output is chosen
    /// again after a reconnect. `Err` reports why the output is not eligible
    /// (not connected, on another GPU, or currently failed).
    pub fn linux_set_display_source(&mut self, name: &str) -> Result<(), String> {
        let vulkan = self
            .os
            .vulkan
            .as_mut()
            .ok_or_else(|| "the direct Vulkan renderer is not initialized".to_string())?;
        vulkan.direct_request_display_source(name)?;
        // The switch happens on the paint path; make sure one runs soon even
        // when the desktop is idle.
        self.redraw_all();
        Ok(())
    }
}

#[cfg(not(all(not(gpusim), linux_direct, use_vulkan)))]
impl crate::cx::Cx {
    /// Windowed, hosted and headless builds do not own displays; the snapshot
    /// is empty.
    pub fn linux_display_snapshot(&self) -> LinuxDisplaySnapshot {
        LinuxDisplaySnapshot::default()
    }

    /// Only the direct Vulkan backend (`MAKEPAD=linux_direct,vulkan`) drives
    /// displays; windowed and hosted builds cannot choose a render source.
    pub fn linux_set_display_source(&mut self, name: &str) -> Result<(), String> {
        Err(format!(
            "cannot select display source {name}: this build is not the direct Vulkan backend"
        ))
    }
}
