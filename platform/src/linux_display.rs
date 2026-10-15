//! Display inventory and controls of the HAL direct (DRM/KMS) shell, as Cx sees them over the pipe.
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

pub use makepad_hal::direct::{DisplayArrangement, DisplayPacing, LinuxDisplayFrameTiming, LinuxDisplayOutput, LinuxDisplaySnapshot, LINUX_VULKAN_DEVICE_ENV};

pub use makepad_hal::direct::DisplayCommand as LinuxDisplayCommand;

/// Host acceptance of a queued display request. Device/modeset failures that
/// occur later are reported by the inventory's per-output status.
#[derive(Clone, Debug)]
pub struct LinuxDisplayResultEvent {
    pub request: u64,
    pub command: LinuxDisplayCommand,
    pub result: Result<(), String>,
}

impl crate::cx::Cx {
    pub fn linux_display_snapshot(&self) -> LinuxDisplaySnapshot {
        self.hal.direct_display.snapshot.clone()
    }
    pub fn linux_display_generation(&self) -> u64 {
        self.hal.direct_display.generation
    }
    pub fn linux_display_frame_timing(&self, name: &str) -> Option<LinuxDisplayFrameTiming> {
        let display = &self.hal.direct_display;
        let index = display.snapshot.outputs.iter().position(|o| o.name == name)?;
        let (last_present, interval) = display.timings.get(index).copied().flatten()?;
        let now = Self::monotonic_now();
        Some(LinuxDisplayFrameTiming {
            present_time: now + interval - (now - last_present).rem_euclid(interval),
            refresh_interval: interval,
        })
    }
    /// Queue a primary/source request. `Ok(())` means queued locally; host acceptance
    /// arrives as `Event::LinuxDisplayResult`. Its command includes these arguments.
    pub fn linux_set_display_source(&mut self, name: &str) -> Result<(), String> {
        self.hal_queue_display(LinuxDisplayCommand::Source(name.into()))
    }
    /// Queue an order request; the host's `Result` arrives as `Event::LinuxDisplayResult`.
    pub fn linux_set_display_order(&mut self, order: &[String]) -> Result<(), String> {
        self.hal_queue_display(LinuxDisplayCommand::Order(order.to_vec()))
    }
    /// Validate mode syntax locally and queue it; host acceptance arrives as `Event::LinuxDisplayResult`.
    pub fn linux_set_display_mode(&mut self, name: &str, mode: Option<&str>) -> Result<(), String> {
        self.hal_queue_display(LinuxDisplayCommand::Mode { name: name.into(), mode: mode.map(str::to_owned) })
    }
    /// Extended desktop, or mirror every output optimised for one screen (the desktop is
    /// rendered at its mode, the others show it letterboxed). Host acceptance arrives as
    /// `Event::LinuxDisplayResult`; the snapshot's `arrangement` follows.
    pub fn linux_set_display_arrangement(&mut self, arrangement: &DisplayArrangement) -> Result<(), String> {
        self.hal_queue_display(LinuxDisplayCommand::Arrangement(arrangement.clone()))
    }
    /// Queue the pacing setting (which screen's refresh sets the render rate; no modeset).
    /// Host acceptance arrives as `Event::LinuxDisplayResult`; the snapshot's `pacing` and
    /// `paced_by` follow.
    pub fn linux_set_display_pacing(&mut self, pacing: &DisplayPacing) -> Result<(), String> {
        self.hal_queue_display(LinuxDisplayCommand::Pacing(pacing.clone()))
    }
    /// The id assigned by the last successful display setter, for correlating replies.
    pub fn linux_display_request_id(&self) -> u64 {
        self.hal.direct_display.next_request
    }
}
