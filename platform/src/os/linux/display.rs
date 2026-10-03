//! Native Linux display inventory for the direct (DRM/KMS) Vulkan backend.
//!
//! The direct backend drives one wide desktop: every connector of the
//! rendering GPU side by side, left to right along their top edges, at their
//! native pixels divided by the global DPI factor (see `linux_wide_desktop`).
//! Each connector shows its own rectangle of it. A connector that does not
//! fit within the GPU's limits shows the whole desktop letterboxed.
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
    /// The main screen: the dock goes here. It may be on any GPU. A host
    /// with its own per-screen placement may still send new windows and
    /// menus to whichever screen is active (the one the pointer or the
    /// focused window is on) instead.
    pub primary: bool,
    /// The DRM card driving this connector, e.g. `card1`.
    pub card: String,
    /// Top-left corner of this connector's rectangle in the wide desktop, in
    /// native pixels; `None` when it is not part of the desktop.
    pub desktop_position: Option<(u32, u32)>,
    /// The connector has a live swapchain and its last presentation succeeded.
    pub active: bool,
    /// Human-readable state: `active`, `unsupported: …`, `failed: …`, `lost: …`.
    /// Failure text is the actual Vulkan/DRM error, never a guess from sysfs.
    pub status: String,
    /// Every mode the connector offers (width, height, refresh Hz),
    /// deduplicated and sorted by area then refresh, both descending. Empty
    /// for a row this GPU has not acquired (unsupported or not yet started).
    pub modes: Vec<(u32, u32, f64)>,
    /// This connector's entry in the runtime mode-override table
    /// (`MAKEPAD_DRM_MODES` / `linux_set_display_mode`), if any, regardless
    /// of whether the connector has started yet.
    pub mode_override: Option<String>,
    /// The basename of `canonicalize(/sys/class/drm/<card>/device)`: the
    /// card's PCI address, for joining this screen to a host's own GPU
    /// list by a key that survives card renumbering. `None` when the
    /// sysfs link cannot be resolved.
    pub pci: Option<String>,
}

/// Everything the renderer knows about connected displays.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LinuxDisplaySnapshot {
    /// `true` only while the direct Vulkan backend owns the displays.
    pub direct: bool,
    /// Outputs in default order: built-in panels first, then by name.
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

    /// Bumps whenever the direct backend's screens, their positions, modes,
    /// statuses or main screen may have changed since the last call; cheap,
    /// for a caller to poll instead of re-reading and diffing the snapshot
    /// every frame. `0` for a build with no direct backend.
    pub fn linux_display_generation(&self) -> u64 {
        self.os
            .vulkan
            .as_ref()
            .map_or(0, |vulkan| vulkan.direct_layout_generation())
    }

    /// Ask the renderer to make the named output the main screen ("optimize
    /// for"): its native mode becomes the composition resolution. `name` is
    /// an `outputs[].name` from the snapshot and may be a screen on any GPU
    /// of the wide desktop, not only the rendering GPU's own. When it is an
    /// active screen of the rendering GPU, that output also becomes the one
    /// that paces frames; choosing a peer's screen moves the main screen
    /// there without changing which output paces frames.
    ///
    /// `Ok` means the request was accepted and queued, not applied: the
    /// renderer switches at its next safe frame boundary (no mode setting or
    /// GPU waits happen inside the caller's event handler). Once applied, the
    /// snapshot marks that output `primary` (immediately once the output is
    /// active, ahead of any composition resize the switch needs), the main
    /// window's geometry changes through the ordinary `WindowGeomChange`
    /// event at the same effective DPI, and the preferred name is kept so
    /// the output is chosen again after a reconnect. `Err` reports why the
    /// output is not eligible (not connected, on another GPU, or currently
    /// failed).
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

    /// Lay the screens out left to right in this order of connector names
    /// (`outputs[].name` from the snapshot). Names not listed follow in the
    /// default order; unknown names are ignored. Like the display source,
    /// the request is applied at the next safe frame boundary; when the
    /// desktop size changes the main window gets a `WindowGeomChange`.
    pub fn linux_set_display_order(&mut self, order: &[String]) -> Result<(), String> {
        let vulkan = self
            .os
            .vulkan
            .as_mut()
            .ok_or_else(|| "the direct Vulkan renderer is not initialized".to_string())?;
        vulkan.direct_request_display_order(order)?;
        self.redraw_all();
        Ok(())
    }

    /// Show the named output in `mode` (`"3840x2160"`, `"3840x2160@30"` or
    /// `"3840x2160-30"`), or in its fastest native mode with `None`. The
    /// output is reacquired at the next safe frame boundary, on whichever GPU
    /// drives it; a mode it does not offer leaves it failed until changed.
    pub fn linux_set_display_mode(&mut self, name: &str, mode: Option<&str>) -> Result<(), String> {
        let vulkan = self
            .os
            .vulkan
            .as_mut()
            .ok_or_else(|| "the direct Vulkan renderer is not initialized".to_string())?;
        vulkan.direct_request_display_mode(name, mode)?;
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

    /// Builds with no direct backend never change their (empty) snapshot.
    pub fn linux_display_generation(&self) -> u64 {
        0
    }

    /// Only the direct Vulkan backend (`MAKEPAD=linux_direct,vulkan`) drives
    /// displays; windowed and hosted builds cannot choose a render source.
    pub fn linux_set_display_source(&mut self, name: &str) -> Result<(), String> {
        Err(format!(
            "cannot select display source {name}: this build is not the direct Vulkan backend"
        ))
    }

    /// Only the direct Vulkan backend arranges displays.
    pub fn linux_set_display_order(&mut self, _order: &[String]) -> Result<(), String> {
        Err("cannot arrange displays: this build is not the direct Vulkan backend".to_string())
    }

    /// Only the direct Vulkan backend sets display modes.
    pub fn linux_set_display_mode(&mut self, name: &str, _mode: Option<&str>) -> Result<(), String> {
        Err(format!("cannot set the mode of {name}: this build is not the direct Vulkan backend"))
    }
}
