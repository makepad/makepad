//! The EGL display, the HAL's GLES 3 context and the window surfaces. Everything here runs on
//! the render thread: the context is made current there once (surfaceless, or on a 1x1 pbuffer
//! where EGL has no surfaceless contexts) and stays current; a window's surface is bound only
//! to present its finished image.
use super::surface::{EglSurfaceSource, NativeSurface, SurfaceState, PLATFORM_ANDROID, PLATFORM_SURFACELESS};
#[cfg(target_os = "linux")]
use super::surface::PLATFORM_WAYLAND;
use super::sys::{self, Gl};
#[cfg(target_os = "linux")]
use crate::backend::linux::media::module_loader::ModuleLoader;
use crate::Id;
use std::{collections::HashMap, ffi::{c_void, CStr}, ptr};

pub type Handle = *mut c_void;

const NONE: i32 = 0x3038;
const EXTENSIONS: i32 = 0x3055;
const SURFACE_TYPE: i32 = 0x3033;
const WINDOW_BIT: i32 = 0x0004;
const PBUFFER_BIT: i32 = 0x0001;
const DEPTH_SIZE: i32 = 0x3025;
const STENCIL_SIZE: i32 = 0x3026;
const SAMPLES: i32 = 0x3031;

#[cfg(target_os = "linux")]
const LIBRARY: &str = "libEGL.so.1";
#[cfg(target_os = "android")]
const LIBRARY: &str = "libEGL.so";

/// Android's libEGL and libGLESv3, opened once like Linux's and never closed.
#[cfg(target_os = "android")]
pub struct ModuleLoader(Handle);

#[cfg(target_os = "android")]
#[link(name = "dl")]
extern "C" {
    fn dlopen(name: *const std::ffi::c_char, flags: i32) -> Handle;
    fn dlsym(handle: Handle, name: *const std::ffi::c_char) -> Handle;
}

#[cfg(target_os = "android")]
impl ModuleLoader {
    pub fn load(path: &str) -> Result<Self, ()> {
        let path = std::ffi::CString::new(path).map_err(|_| ())?;
        // RTLD_LAZY, the one flag with the same value on 32- and 64-bit bionic
        let handle = unsafe { dlopen(path.as_ptr(), 1) };
        if handle.is_null() { Err(()) } else { Ok(Self(handle)) }
    }

    pub fn get_symbol<F: Sized>(&self, name: &str) -> Result<F, ()> {
        let name = std::ffi::CString::new(name).map_err(|_| ())?;
        let symbol = unsafe { dlsym(self.0, name.as_ptr()) };
        if symbol.is_null() { return Err(()); }
        Ok(unsafe { std::mem::transmute_copy::<_, F>(&symbol) })
    }
}

macro_rules! egl_functions {
    ($($field:ident: $symbol:literal ($($arg:ty),*) -> $ret:ty;)*) => {
        pub struct Egl {
            $(pub $field: unsafe extern "C" fn($($arg),*) -> $ret,)*
            /// EGL 1.5, which Linux's platform displays need; Android's libEGL only has it from API 29 on
            pub platform_display: Option<unsafe extern "C" fn(u32, Handle, *const isize) -> Handle>,
            _library: ModuleLoader,
            /// where Android's libEGL hands out no core GLES entry points
            #[cfg(target_os = "android")]
            gles: Option<ModuleLoader>,
        }
        impl Egl {
            /// The process's libEGL, loaded once and never unloaded: the driver it brings in keeps
            /// process-wide state (the display, TLS destructors of every thread that made a
            /// context current), so unloading it when one backend ends crashed the threads of
            /// the others and its own render thread's exit.
            pub fn load() -> Result<&'static Self, String> {
                static EGL: std::sync::OnceLock<Result<Egl, String>> = std::sync::OnceLock::new();
                EGL.get_or_init(|| {
                    let library = ModuleLoader::load(LIBRARY).map_err(|_| format!("cannot load {LIBRARY}"))?;
                    Ok(Self { $($field: library.get_symbol($symbol).map_err(|_| concat!("libEGL lacks ", $symbol))?,)*
                        platform_display: library.get_symbol("eglGetPlatformDisplay").ok(),
                        #[cfg(target_os = "android")]
                        gles: ModuleLoader::load("libGLESv3.so").ok(),
                        _library: library })
                }).as_ref().map_err(String::clone)
            }
        }
    }
}

egl_functions! {
    get_proc: "eglGetProcAddress" (*const std::ffi::c_char) -> Handle;
    get_display: "eglGetDisplay" (Handle) -> Handle;
    initialize: "eglInitialize" (Handle, *mut i32, *mut i32) -> u32;
    query_string: "eglQueryString" (Handle, i32) -> *const std::ffi::c_char;
    bind_api: "eglBindAPI" (u32) -> u32;
    choose_config: "eglChooseConfig" (Handle, *const i32, *mut Handle, i32, *mut i32) -> u32;
    config_attrib: "eglGetConfigAttrib" (Handle, Handle, i32, *mut i32) -> u32;
    create_context: "eglCreateContext" (Handle, Handle, Handle, *const i32) -> Handle;
    create_window: "eglCreateWindowSurface" (Handle, Handle, usize, *const i32) -> Handle;
    create_pbuffer: "eglCreatePbufferSurface" (Handle, Handle, *const i32) -> Handle;
    query_surface: "eglQuerySurface" (Handle, Handle, i32, *mut i32) -> u32;
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
        Gl::load(|name: &CStr| {
            let f = unsafe { (self.get_proc)(name.as_ptr()) };
            #[cfg(target_os = "android")]
            if f.is_null() {
                return self.gles.as_ref().and_then(|lib| lib.get_symbol(&name.to_string_lossy()).ok()).unwrap_or(ptr::null_mut());
            }
            f
        })
    }
}

#[cfg(target_os = "linux")]
struct Wayland {
    create: unsafe extern "C" fn(Handle, i32, i32) -> Handle,
    resize: unsafe extern "C" fn(Handle, i32, i32, i32, i32),
    destroy: unsafe extern "C" fn(Handle),
    _library: ModuleLoader,
}

// SAFETY: entry points and a library handle that is never closed.
#[cfg(target_os = "linux")]
unsafe impl Send for Wayland {}
#[cfg(target_os = "linux")]
unsafe impl Sync for Wayland {}

#[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
    wayland: Option<&'static Wayland>,
    source: Box<dyn EglSurfaceSource>,
    windows: HashMap<(u32, Id), Window>,
    current: Handle,
    /// what is bound while no window presents: nothing, or a 1x1 pbuffer without surfaceless contexts
    idle: Handle,
}

impl Context {
    pub fn new(source: Box<dyn EglSurfaceSource>) -> Result<Self, String> {
        let native = source.display()?;
        let egl = Egl::load()?;
        #[cfg(target_os = "linux")]
        let wayland = if native.platform == PLATFORM_WAYLAND { Some(Wayland::load()?) } else { None };
        unsafe {
            // Android's one display is the default one, which an EGL 1.4 driver opens too
            let display = if native.platform == PLATFORM_ANDROID {
                (egl.get_display)(ptr::null_mut())
            } else {
                (egl.platform_display.ok_or("libEGL lacks eglGetPlatformDisplay")?)(native.platform, native.native as Handle, ptr::null())
            };
            let (mut major, mut minor) = (0, 0);
            if display.is_null() || (egl.initialize)(display, &mut major, &mut minor) == 0 {
                return Err(format!("eglInitialize (platform {:#x}): {:#x}", native.platform, (egl.error)()));
            }
            let extensions = (egl.query_string)(display, EXTENSIONS);
            let extensions = if extensions.is_null() { String::new() } else { CStr::from_ptr(extensions).to_string_lossy().into_owned() };
            let surfaceless = extensions.split(' ').any(|e| e == "EGL_KHR_surfaceless_context");
            if !surfaceless && native.platform == PLATFORM_SURFACELESS {
                return Err("EGL lacks EGL_KHR_surfaceless_context".into());
            }
            if (egl.bind_api)(0x30A0) == 0 {
                return Err("eglBindAPI(OpenGL ES) failed".into());
            }
            // windows need WINDOW_BIT; the surfaceless platform makes none
            let surface_type = if native.platform == PLATFORM_SURFACELESS { 0 } else { WINDOW_BIT }
                | if surfaceless { 0 } else { PBUFFER_BIT };
            let attrs = [0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3021, 8, SURFACE_TYPE, surface_type, 0x3040, 0x0040, NONE];
            let mut count = 0;
            if (egl.choose_config)(display, attrs.as_ptr(), ptr::null_mut(), 0, &mut count) == 0 || count <= 0 {
                return Err("no EGL RGBA8 GLES3 config".into());
            }
            let mut configs = vec![ptr::null_mut(); count as usize];
            if (egl.choose_config)(display, attrs.as_ptr(), configs.as_mut_ptr(), count, &mut count) == 0 {
                return Err(format!("eglChooseConfig: {:#x}", (egl.error)()));
            }
            configs.truncate(count.max(0) as usize);
            // every pass renders into textures, so a window's own depth, stencil or samples
            // would only cost memory (EGL's own order puts them last, but not every driver's)
            let attrib = |config: Handle, name: i32| { let mut v = 0; (egl.config_attrib)(display, config, name, &mut v); v };
            configs.sort_by_key(|config| (attrib(*config, DEPTH_SIZE) + attrib(*config, STENCIL_SIZE) > 0, attrib(*config, SAMPLES) > 0));
            // X11 windows are made with the shell's 32-bit visual: the config must render to it
            let config = configs.into_iter().find(|config| {
                let mut visual = 0;
                native.visual == 0 || ((egl.config_attrib)(display, *config, 0x302E, &mut visual) != 0 && visual as u32 == native.visual)
            }).ok_or("no EGL config renders to the X11 windows' visual")?;
            let context = (egl.create_context)(display, config, ptr::null_mut(), [0x3098, 3, NONE].as_ptr());
            if context.is_null() {
                return Err(format!("eglCreateContext(GLES 3): {:#x}", (egl.error)()));
            }
            let idle = if surfaceless { ptr::null_mut() } else { (egl.create_pbuffer)(display, config, [0x3057, 1, 0x3056, 1, NONE].as_ptr()) };
            if !surfaceless && idle.is_null() {
                (egl.destroy_context)(display, context);
                return Err(format!("eglCreatePbufferSurface (no surfaceless contexts): {:#x}", (egl.error)()));
            }
            let release = || {
                (egl.make_current)(display, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
                if !idle.is_null() { (egl.destroy_surface)(display, idle); }
                (egl.destroy_context)(display, context);
            };
            if (egl.make_current)(display, idle, idle, context) == 0 {
                let error = (egl.error)();
                release();
                return Err(format!("eglMakeCurrent ({}): {error:#x}", if surfaceless { "surfaceless" } else { "pbuffer" }));
            }
            let gl = match egl.gl() {
                Ok(gl) => gl,
                Err(e) => {
                    release();
                    return Err(e);
                }
            };
            Ok(Self { gl, share: std::sync::Arc::new(Share { egl, display, config, context }), #[cfg(target_os = "linux")] wayland, source,
                windows: HashMap::new(), current: idle, idle })
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
            let _ = self.bind(self.idle);
        }
        unsafe {
            (self.share.egl.destroy_surface)(self.share.display, window.surface);
            #[cfg(target_os = "linux")]
            if let Some(wl) = &self.wayland { (wl.destroy)(window.wl_window); }
        }
        // the native lease (window.native.keep) drops after EGL and wl_egl_window let go of it
    }

    pub fn close_session(&mut self, sid: u32) {
        let keys: Vec<_> = self.windows.keys().filter(|k| k.0 == sid).copied().collect();
        for key in keys { self.close(key); }
    }

    /// Lets go of every native window, before Android's `surfaceDestroyed` returns.
    #[cfg(target_os = "android")]
    pub fn close_all(&mut self) {
        let keys: Vec<_> = self.windows.keys().copied().collect();
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
            #[cfg(target_os = "linux")]
            let wl_window = match &self.wayland {
                Some(wl) => {
                    let w = unsafe { (wl.create)(native.window as Handle, native.size[0] as i32, native.size[1] as i32) };
                    if w.is_null() { return Err("wl_egl_window_create failed".into()); }
                    w
                }
                None => ptr::null_mut(),
            };
            #[cfg(target_os = "linux")]
            let window = if wl_window.is_null() { native.window } else { wl_window as usize };
            #[cfg(target_os = "android")]
            let window = native.window;
            let surface = unsafe { (share.egl.create_window)(share.display, share.config, window, ptr::null()) };
            if surface.is_null() {
                let error = unsafe { (share.egl.error)() };
                #[cfg(target_os = "linux")]
                if let Some(wl) = &self.wayland { unsafe { (wl.destroy)(wl_window) } }
                return Err(format!("eglCreateWindowSurface: {error:#x}"));
            }
            let size = native.size;
            self.windows.insert(key, Window { surface, #[cfg(target_os = "linux")] wl_window, size, native });
            self.bind(surface)?;
            // the shell paces frames (FrameReady); a swap never waits for the compositor. On
            // Android a swap waits for a free buffer, like Vulkan's FIFO swapchain there.
            unsafe { (self.share.egl.interval)(self.share.display, if cfg!(target_os = "android") { 1 } else { 0 }) };
        } else {
            let w = self.windows.get_mut(&key).unwrap();
            if w.size != native.size {
                #[cfg(target_os = "linux")]
                if let Some(wl) = &self.wayland { unsafe { (wl.resize)(w.wl_window, native.size[0] as i32, native.size[1] as i32, 0, 0) } }
                w.size = native.size;
            }
            w.native = native;
        }
        let (surface, out) = { let w = &self.windows[&key]; (w.surface, w.size) };
        self.bind(surface)?;
        // the surface's own size, which can be newer than the shell's (Android hands out buffers
        // of a rotated size before `surfaceChanged` tells us)
        let out = unsafe {
            let (s, mut w, mut h) = (&self.share, 0, 0);
            if (s.egl.query_surface)(s.display, surface, 0x3057, &mut w) != 0 && (s.egl.query_surface)(s.display, surface, 0x3056, &mut h) != 0 && w > 0 && h > 0 {
                [w as u32, h as u32]
            } else { out }
        };
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
                // a surface Android abandoned (surfaceDestroyed) is not an error worth reporting
                if matches!(error, 0x300B | 0x300D) && matches!(self.source.state(key.0, key.1), SurfaceState::Lost) { return Ok(false); }
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
            if !self.idle.is_null() { (s.egl.destroy_surface)(s.display, self.idle); }
            (s.egl.destroy_context)(s.display, s.context);
        }
        // the display stays initialized: it is the process's, other contexts may use it
    }
}
