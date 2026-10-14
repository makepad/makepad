//! Backends: what a session's commands execute against. A backend is chosen at `hal_open`.

pub mod exec;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod main_lag;
pub mod headless;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub mod gl;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod vulkan;
#[cfg(target_vendor = "apple")]
pub mod apple;
#[cfg(windows)]
pub mod windows;

use crate::buffers::CmdView;

/// Makes one session's pump look: what a GPU completion calls for the session that owns it
/// (the pump that steps drains every completion; others' pumps sleep on).
/// The flag: a window frame or presentation, news also for the sessions that hear of the owner's
/// window frames (its parent, the host: `World::listeners`).
pub type SessionNotify = std::sync::Arc<dyn Fn(u32, bool) + Send + Sync>;

/// A frame of `kind` that is a window's (not a texture, pick, capture or snapshot pass): its
/// completion is news for the window's compositor.
pub fn window_frame(kind: u32) -> bool {
    kind & !(crate::types::target::CAPTURE | crate::types::target::SNAPSHOT) == crate::types::target::WINDOW
}
use crate::session::World;
use crate::types::{HalInfo, Id};

/// Startup/WSI diagnostics, cached so frame paths never read the environment.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_HAL_LOG").is_some())
}

/// When a device loss is simulated (HAL clock ns): `MAKEPAD_HAL_SIMULATE_DEVICE_LOSS_MS` after
/// now (the backend is being made). With `MAKEPAD_HAL_SIMULATE_DEVICE_LOSS_APP` only the
/// process whose executable has that name simulates it: one hosted app under a host that
/// passes its environment on (the host's own device stays).
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
pub(crate) fn simulated_loss_at() -> Option<u64> {
    let ms = std::env::var("MAKEPAD_HAL_SIMULATE_DEVICE_LOSS_MS").ok()?.parse::<u64>().ok()?;
    if let Ok(app) = std::env::var("MAKEPAD_HAL_SIMULATE_DEVICE_LOSS_APP") {
        let exe = std::env::current_exe().ok()?;
        if exe.file_stem().is_none_or(|stem| stem != app.as_str()) { return None; }
    }
    Some(crate::clock_ns() + ms * 1_000_000)
}

/// A window's pixels for the host: `w` x `h` RGBA8 rows, top row first.
pub struct Pixels {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Every backend reports completed frames through `World::frame_done`, including asynchronous
/// GPU completions, so presenters and parent compositors receive the same notifications.
pub trait Backend: Send {
    #[cfg(target_os = "linux")]
    fn dmabuf(&mut self) -> Option<&mut dyn crate::dmabuf::Api> { None }
    /// The capabilities `hal_open` answers with (session id filled in by the caller).
    fn info(&self) -> HalInfo;
    /// Executes one batch of commands. Borrowed slices in `view` are valid for this call only.
    fn submit(&mut self, world: &mut World, session: u32, view: &CmdView);
    /// Long driver work the batch's frames will need that can be done before it runs and
    /// without the HAL's lock (pipeline variants a frame uses first): `Hal::submit` runs it on
    /// the submitting session's thread with the lock released, so only that session waits for it.
    fn work_ahead(&mut self, _world: &World, _session: u32, _view: &CmdView) -> Option<Box<dyn FnOnce() + Send>> {
        None
    }
    /// One non-blocking step of the backend's loop: due timers, OS events, vsync, into the
    /// session's event queue. Blocking waits are the caller's (`Hal::pump`).
    fn step(&mut self, world: &mut World, session: u32);
    /// The nearest time (HAL clock ns) the backend has something due, if any (timers).
    fn next_due(&self, world: &World, session: u32) -> Option<u64>;
    /// The pixels of a window of the session (its newest finished frame), for the host side
    /// (automation snapshots). `None` when the backend has no raster or the window has no frame.
    /// Waits for the GPU: never called from the app's thread, only by the host.
    fn window_pixels(&mut self, _world: &mut World, _session: u32, _window: Option<Id>) -> Option<Pixels> {
        None
    }
    /// The device side of the host's own objects (the font service's atlas textures, which belong
    /// to session 0): the hooks the executor uses for a session's commands, for the HAL itself.
    fn host_gpu(&mut self) -> Option<&mut dyn exec::Gpu> {
        None
    }
    /// A session was opened: the backend sets up per-session state (the world has the session).
    fn session_opened(&mut self, _world: &mut World, _session: u32) {}
    /// A session is closing (also a relaunch of the same app key): free exactly the objects this
    /// session owns in the backend (headless: its table entries; the world frees them right
    /// after this call). Device-level state (device, queues, swapchains) and objects lent from
    /// the host stay.
    fn session_closed(&mut self, _world: &mut World, _session: u32) {}
    /// The host is about to end the process ([`crate::Hal::before_exit`]): driver work still
    /// running on the backend's own threads (pipeline compiles, fence waits) runs out first;
    /// exit() unloads the driver under it.
    fn before_exit(&mut self) {}
    /// Tests and automation: the next step handles the device as lost (removed by the OS) and
    /// replaces it as a real loss does (`exec::device_reset`). Backends without a device reset
    /// ignore it.
    fn simulate_device_loss(&mut self) {}
}

#[cfg(any(target_os = "android", test))]
pub mod android;
