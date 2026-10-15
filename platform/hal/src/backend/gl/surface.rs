//! The shell seam of the GL backend: which EGL platform display to open and the native window a
//! session's window presents into. A native window is leased (kept alive) until the render
//! thread has destroyed its EGL surface and, on Wayland, its wl_egl_window.
use crate::Id;
use std::sync::Arc;

pub const PLATFORM_X11: u32 = 0x31D5;
pub const PLATFORM_WAYLAND: u32 = 0x31D8;
pub const PLATFORM_SURFACELESS: u32 = 0x31DD;

#[derive(Clone, Copy, Debug)]
pub struct Display {
    /// EGL_PLATFORM_* of `native`
    pub platform: u32,
    /// the native display (Xlib `Display*`, `wl_display*`); 0 = the default display
    pub native: usize,
    /// X11: the visual id every window is created with (the config must match it); 0 = any
    pub visual: u32,
}

pub struct NativeSurface {
    /// changes when the shell replaces the native window
    pub generation: u64,
    /// X11 Window id or `wl_surface*`
    pub window: usize,
    pub size: [u32; 2],
    pub keep: Arc<dyn std::any::Any + Send + Sync>,
}

pub enum SurfaceState {
    Offscreen,
    Lost,
    Native(NativeSurface),
}

pub trait EglSurfaceSource: Send {
    fn display(&self) -> Result<Display, String>;
    /// Never waits; sizes are physical pixels.
    fn state(&mut self, session: u32, window: Id) -> SurfaceState;
}

/// Hidden runs and tests: windows are render targets only, the display is the surfaceless one.
pub struct Offscreen;

impl EglSurfaceSource for Offscreen {
    fn display(&self) -> Result<Display, String> {
        Ok(Display { platform: PLATFORM_SURFACELESS, native: 0, visual: 0 })
    }
    fn state(&mut self, _: u32, _: Id) -> SurfaceState {
        SurfaceState::Offscreen
    }
}
