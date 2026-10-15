//! The EGL display, the HAL's GLES 3 context and the window surfaces. Everything here runs on
//! the render thread: the context is made current there once (surfaceless) and stays current;
//! a window's surface is bound only to present its finished image.
use super::surface::{EglSurfaceSource, NativeSurface, SurfaceState, PLATFORM_SURFACELESS, PLATFORM_WAYLAND};
use super::sys::{self, Gl};
use crate::backend::linux::media::module_loader::ModuleLoader;
use crate::Id;
use std::{collections::HashMap, ffi::{c_void, CStr}, ptr};

pub type Handle = *mut c_void;

const NONE: i32 = 0x3038;
const EXTENSIONS: i32 = 0x3055;

macro_rules! egl_functions {
    ($($field:ident: $symbol:literal ($($arg:ty),*) -> $ret:ty;)*) => {
        pub struct Egl {
            $(pub $field: unsafe extern "C" fn($($arg),*) -> $ret,)*
            _library: ModuleLoader,
        }
        impl Egl {
            /// The process's libEGL, loaded once and never unloaded: the driver it brings in keeps
            /// process-wide state (the display, TLS destructors of every thread that made a
            /// context current), so unloading it when one backend ends crashed the threads of
            /// the others and its own render thread's exit.
            pub fn load() -> Result<&'static Self, String> {
                static EGL: std::sync::OnceLock<Result<Egl, String>> = std::sync::OnceLock::new();
                EGL.get_or_init(|| {
                    let library = ModuleLoader::load("libEGL.so.1").map_err(|_| "cannot load libEGL.so.1")?;
                    Ok(Self { $($field: library.get_symbol($symbol).map_err(|_| concat!("libEGL lacks ", $symbol))?,)* _library: library })
                }).as_ref().map_err(String::clone)
            }
        }
    }
}

egl_functions! {
    get_proc: "eglGetProcAddress" (*const std::ffi::c_char) -> Handle;
    platform_display: "eglGetPlatformDisplay" (u32, Handle, *const isize) -> Handle;
    initialize: "eglInitialize" (Handle, *mut i32, *mut i32) -> u32;
    query_string: "eglQueryString" (Handle, i32) -> *const std::ffi::c_char;
    bind_api: "eglBindAPI" (u32) -> u32;
    choose_config: "eglChooseConfig" (Handle, *const i32, *mut Handle, i32, *mut i32) -> u32;
    config_attrib: "eglGetConfigAttrib" (Handle, Handle, i32, *mut i32) -> u32;
    create_context: "eglCreateContext" (Handle, Handle, Handle, *const i32) -> Handle;
    create_window: "eglCreateWindowSurface" (Handle, Handle, usize, *const i32) -> Handle;
    make_current: "eglMakeCurrent" (Handle, Handle, Handle, Handle) -> u32;
    swap: "eglSwapBuffers" (Handle, Handle) -> u32;
    interval: "eglSwapInterval" (Handle, i32) -> u32;
    error: "eglGetError" () -> i32;
    destroy_surface: "eglDestroySurface" (Handle, Handle) -> u32;
    destroy_context: "eglDestroyContext" (Handle, Handle) -> u32;
}

// SAFETY: entry points and a library handle that is never closed; EGL is thread-safe per its spec.
unsafe impl Send for Egl {}
unsafe impl Sync for Egl {}

impl Egl {
    pub fn gl(&self) -> Result<Gl, String> {
        Gl::load(|name: &CStr| unsafe { (self.get_proc)(name.as_ptr()) })
    }
}

struct Wayland {
    create: unsafe extern "C" fn(Handle, i32, i32) -> Handle,
    resize: unsafe extern "C" fn(Handle, i32, i32, i32, i32),
    destroy: unsafe extern "C" fn(Handle),
    _library: ModuleLoader,
}

// SAFETY: entry points and a library handle that is never closed.
unsafe impl Send for Wayland {}
unsafe impl Sync for Wayland {}

impl Wayland {
    /// Loaded once for the process, like libEGL (never unloaded under the driver).
    fn load() -> Result<&'static Self, String> {
        static WAYLAND: std::sync::OnceLock<Result<Wayland, String>> = std::sync::OnceLock::new();
        WAYLAND.get_or_init(|| {
            let lib = ModuleLoader::load("libwayland-egl.so.1").map_err(|_| "cannot load libwayland-egl.so.1")?;
            Ok(Self {
                create: lib.get_symbol("wl_egl_window_create").map_err(|_| "libwayland-egl lacks wl_egl_window_create")?,
                resize: lib.get_symbol("wl_egl_window_resize").map_err(|_| "libwayland-egl lacks wl_egl_window_resize")?,
                destroy: lib.get_symbol("wl_egl_window_destroy").map_err(|_| "libwayland-egl lacks wl_egl_window_destroy")?,
                _library: lib,
            })
        }).as_ref().map_err(String::clone)
    }
}

struct Window {
    surface: Handle,
    wl_window: Handle,
    size: [u32; 2],
    native: NativeSurface,
}

/// The display, config and share group every GL context of the process belongs to.
pub struct Share {
    pub egl: &'static Egl,
    pub display: Handle,
    pub config: Handle,
    pub context: Handle,
}

// SAFETY: EGL handles are process-wide values; the functions are thread-safe per the EGL spec.
unsafe impl Send for Share {}
unsafe impl Sync for Share {}

pub struct Context {
    pub gl: Gl,
    pub share: std::sync::Arc<Share>,
    wayland: Option<&'static Wayland>,
    source: Box<dyn EglSurfaceSource>,
    windows: HashMap<(u32, Id), Window>,
    current: Handle,
}

impl Context {
    pub fn new(source: Box<dyn EglSurfaceSource>) -> Result<Self, String> {
        let native = source.display()?;
        let egl = Egl::load()?;
        let wayland = if native.platform == PLATFORM_WAYLAND { Some(Wayland::load()?) } else { None };
        unsafe {
            let display = (egl.platform_display)(native.platform, native.native as Handle, ptr::null());
            let (mut major, mut minor) = (0, 0);
            if display.is_null() || (egl.initialize)(display, &mut major, &mut minor) == 0 {
                return Err(format!("eglInitialize (platform {:#x}): {:#x}", native.platform, (egl.error)()));
            }
            let extensions = (egl.query_string)(display, EXTENSIONS);
            let extensions = if extensions.is_null() { String::new() } else { CStr::from_ptr(extensions).to_string_lossy().into_owned() };
            if !extensions.split(' ').any(|e| e == "EGL_KHR_surfaceless_context") {
                return Err("EGL lacks EGL_KHR_surfaceless_context".into());
            }
            if (egl.bind_api)(0x30A0) == 0 {
                return Err("eglBindAPI(OpenGL ES) failed".into());
            }
            // windows need WINDOW_BIT; the surfaceless platform makes none
            let surface_type = if native.platform == PLATFORM_SURFACELESS { 0 } else { 0x0004 };
            let attrs = [0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3021, 8, 0x3033, surface_type, 0x3040, 0x0040, NONE];
            let mut count = 0;
            if (egl.choose_config)(display, attrs.as_ptr(), ptr::null_mut(), 0, &mut count) == 0 || count <= 0 {
                return Err("no EGL RGBA8 GLES3 config".into());
            }
            let mut configs = vec![ptr::null_mut(); count as usize];
            if (egl.choose_config)(display, attrs.as_ptr(), configs.as_mut_ptr(), count, &mut count) == 0 {
                return Err(format!("eglChooseConfig: {:#x}", (egl.error)()));
            }
            // X11 windows are made with the shell's 32-bit visual: the config must render to it
            let config = configs.into_iter().take(count as usize).find(|config| {
                let mut visual = 0;
                native.visual == 0 || ((egl.config_attrib)(display, *config, 0x302E, &mut visual) != 0 && visual as u32 == native.visual)
            }).ok_or("no EGL config renders to the X11 windows' visual")?;
            let context = (egl.create_context)(display, config, ptr::null_mut(), [0x3098, 3, NONE].as_ptr());
            if context.is_null() {
                return Err(format!("eglCreateContext(GLES 3): {:#x}", (egl.error)()));
            }
            if (egl.make_current)(display, ptr::null_mut(), ptr::null_mut(), context) == 0 {
                (egl.destroy_context)(display, context);
                return Err(format!("eglMakeCurrent (surfaceless): {:#x}", (egl.error)()));
            }
            let gl = match egl.gl() {
                Ok(gl) => gl,
                Err(e) => {
                    (egl.make_current)(display, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
                    (egl.destroy_context)(display, context);
                    return Err(e);
                }
            };
            Ok(Self { gl, share: std::sync::Arc::new(Share { egl, display, config, context }), wayland, source, windows: HashMap::new(), current: ptr::null_mut() })
        }
    }

    /// A software rasterizer (Mesa's llvmpipe / softpipe): its EGL device says so
    /// (EGL_EXT_device_query + EGL_MESA_device_software).
    pub fn software(&self) -> bool {
        let s = &self.share;
        unsafe {
            let attrib = (s.egl.get_proc)(c"eglQueryDisplayAttribEXT".as_ptr());
            let string = (s.egl.get_proc)(c"eglQueryDeviceStringEXT".as_ptr());
            if attrib.is_null() || string.is_null() { return false; }
            let attrib: unsafe extern "C" fn(Handle, i32, *mut isize) -> u32 = std::mem::transmute(attrib);
            let string: unsafe extern "C" fn(Handle, i32) -> *const std::ffi::c_char = std::mem::transmute(string);
            let mut device = 0isize;
            if attrib(s.display, 0x322C, &mut device) == 0 || device == 0 { return false; }
            let extensions = string(device as Handle, EXTENSIONS);
            !extensions.is_null() && CStr::from_ptr(extensions).to_string_lossy().split(' ').any(|e| e == "EGL_MESA_device_software")
        }
    }

    fn bind(&mut self, surface: Handle) -> Result<(), String> {
        if self.current == surface { return Ok(()); }
        let s = &self.share;
        if unsafe { (s.egl.make_current)(s.display, surface, surface, s.context) } == 0 {
            return Err(format!("eglMakeCurrent: {:#x}", unsafe { (s.egl.error)() }));
        }
        self.current = surface;
        Ok(())
    }

    pub fn close(&mut self, key: (u32, Id)) {
        let Some(window) = self.windows.remove(&key) else { return };
        if self.current == window.surface {
            let _ = self.bind(ptr::null_mut());
        }
        unsafe {
            (self.share.egl.destroy_surface)(self.share.display, window.surface);
            if let Some(wl) = &self.wayland { (wl.destroy)(window.wl_window); }
        }
        // the native lease (window.native.keep) drops after EGL and wl_egl_window let go of it
    }

    pub fn close_session(&mut self, sid: u32) {
        let keys: Vec<_> = self.windows.keys().filter(|k| k.0 == sid).copied().collect();
        for key in keys { self.close(key); }
    }

    /// Blits the whole finished image (`framebuffer`, rows top first) into the window's surface
    /// and swaps. Ok(false): the window has no drawable surface now (closed, zero sized).
    pub fn present(&mut self, key: (u32, Id), framebuffer: u32, size: [u32; 2]) -> Result<bool, String> {
        let native = match self.source.state(key.0, key.1) {
            SurfaceState::Offscreen => return Ok(true),
            SurfaceState::Lost => { self.close(key); return Ok(false); }
            SurfaceState::Native(n) => n,
        };
        if native.size.contains(&0) { return Ok(false); }
        if self.windows.get(&key).is_some_and(|w| w.native.generation != native.generation) { self.close(key); }
        if !self.windows.contains_key(&key) {
            let share = &self.share;
            let wl_window = match &self.wayland {
                Some(wl) => {
                    let w = unsafe { (wl.create)(native.window as Handle, native.size[0] as i32, native.size[1] as i32) };
                    if w.is_null() { return Err("wl_egl_window_create failed".into()); }
                    w
                }
                None => ptr::null_mut(),
            };
            let window = if wl_window.is_null() { native.window } else { wl_window as usize };
            let surface = unsafe { (share.egl.create_window)(share.display, share.config, window, ptr::null()) };
            if surface.is_null() {
                let error = unsafe { (share.egl.error)() };
                if let Some(wl) = &self.wayland { unsafe { (wl.destroy)(wl_window) } }
                return Err(format!("eglCreateWindowSurface: {error:#x}"));
            }
            let size = native.size;
            self.windows.insert(key, Window { surface, wl_window, size, native });
            self.bind(surface)?;
            // the shell paces frames (FrameReady); a swap never waits for the compositor
            unsafe { (self.share.egl.interval)(self.share.display, 0) };
        } else {
            let w = self.windows.get_mut(&key).unwrap();
            if w.size != native.size {
                if let Some(wl) = &self.wayland { unsafe { (wl.resize)(w.wl_window, native.size[0] as i32, native.size[1] as i32, 0, 0) } }
                w.size = native.size;
            }
            w.native = native;
        }
        let (surface, out) = { let w = &self.windows[&key]; (w.surface, w.size) };
        self.bind(surface)?;
        let gl = &self.gl;
        unsafe {
            (gl.glDisable)(sys::SCISSOR_TEST);
            (gl.glBindFramebuffer)(sys::READ_FRAMEBUFFER, framebuffer);
            (gl.glBindFramebuffer)(sys::DRAW_FRAMEBUFFER, 0);
            // image row 0 is the top; the window's framebuffer counts rows from the bottom
            (gl.glBlitFramebuffer)(0, 0, size[0] as i32, size[1] as i32, 0, out[1] as i32, out[0] as i32, 0, sys::COLOR_BUFFER_BIT, sys::NEAREST);
            (gl.glBindFramebuffer)(sys::READ_FRAMEBUFFER, 0);
            if (self.share.egl.swap)(self.share.display, surface) == 0 {
                let error = (self.share.egl.error)();
                self.close(key);
                return Err(format!("eglSwapBuffers: {error:#x}"));
            }
        }
        Ok(true)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        let keys: Vec<_> = self.windows.keys().copied().collect();
        for key in keys { self.close(key); }
        let s = &self.share;
        unsafe {
            (s.egl.make_current)(s.display, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            (s.egl.destroy_context)(s.display, s.context);
        }
        // the display stays initialized: it is the process's, other contexts may use it
    }
}
