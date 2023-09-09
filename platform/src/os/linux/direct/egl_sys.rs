use std::os::raw::{
    c_void,
    c_uint,
    c_ulong,
};

pub const EGL_PLATFORM_GBM_KHR: u32 = 12759;
pub const EGL_OPENGL_ES_API: u32 = 12448;
pub const EGL_SURFACE_TYPE: u32 = 12339;
pub const EGL_WINDOW_BIT: u32 = 4;
pub const EGL_RED_SIZE: u32 = 12324;
pub const EGL_GREEN_SIZE: u32 = 12323;
pub const EGL_BLUE_SIZE: u32 = 12322;
pub const EGL_ALPHA_SIZE: u32 = 12321;
pub const EGL_DEPTH_SIZE: u32 = 12325;
pub const EGL_RENDERABLE_TYPE: u32 = 12352;
pub const EGL_OPENGL_ES2_BIT: u32 = 4;
pub const EGL_NONE: u32 = 12344;
pub const EGL_NATIVE_VISUAL_ID: u32 = 12334;
pub const EGL_CONTEXT_CLIENT_VERSION: u32 = 12440;
pub const EGL_NO_CONTEXT: EGLContext = 0 as *mut c_void;
  
pub type EGLint = i32;
pub type EGLenum = c_uint;
pub type EGLDisplay = *mut c_void;
pub type EGLConfig = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLSurface = *mut c_void;
pub type EGLBoolean = c_uint;
pub type EGLClientBuffer = *mut c_void;
pub type EGLImageKHR = *mut c_void;
pub type EGLSyncKHR = *mut c_void;
pub type EGLTimeKHR = u64;
pub type EGLNativeWindowType = c_ulong;

pub type PFNEGLINITIALIZEPROC = ::std::option::Option<
unsafe extern "C" fn(dpy: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> EGLBoolean,
>;

pub type PFNEGLGETPLATFORMDISPLAYEXTPROC = ::std::option::Option<
unsafe extern "C" fn(
    platform: EGLenum,
    native_display: *mut c_void,
    attrib_list: *const EGLint,
) -> EGLDisplay,
>;

pub type PFNEGLCREATEIMAGEKHRPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    ctx: EGLContext,
    target: EGLenum,
    buffer: EGLClientBuffer,
    attrib_list: *const EGLint,
) -> EGLImageKHR,
>;

pub type PFNEGLDESTROYIMAGEKHRPROC =
::std::option::Option<unsafe extern "C" fn(dpy: EGLDisplay, image: EGLImageKHR) -> EGLBoolean>;
pub type PFNEGLLOCKSURFACEKHRPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    surface: EGLSurface,
    attrib_list: *const EGLint,
) -> EGLBoolean,
>;

pub type PFNEGLCREATESYNCKHRPROC = ::std::option::Option<
unsafe extern "C" fn(dpy: EGLDisplay, type_: EGLenum, attrib_list: *const EGLint) -> EGLSyncKHR,
>;

pub type PFNEGLDESTROYSYNCKHRPROC =
::std::option::Option<unsafe extern "C" fn(dpy: EGLDisplay, sync: EGLSyncKHR) -> EGLBoolean>;

pub type PFNEGLWAITSYNCKHRPROC = ::std::option::Option<
unsafe extern "C" fn(dpy: EGLDisplay, sync: EGLSyncKHR, flags: EGLint) -> EGLint,
>;

pub type PFNEGLCLIENTWAITSYNCKHRPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    sync: EGLSyncKHR,
    flags: EGLint,
    timeout: EGLTimeKHR,
) -> EGLint,
>;

pub type PFNEGLDUPNATIVEFENCEFDANDROIDPROC =
::std::option::Option<unsafe extern "C" fn(dpy: EGLDisplay, sync: EGLSyncKHR) -> EGLint>;

pub type PFNEGLGETCONFIGSPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    configs: *mut EGLConfig,
    config_size: EGLint,
    num_config: *mut EGLint,
) -> EGLBoolean,
>;

pub type PFNEGLCHOOSECONFIGPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    attrib_list: *const u32,
    configs: *mut EGLConfig,
    config_size: EGLint,
    num_config: *mut EGLint,
) -> EGLBoolean,
>;

pub type PFNEGLGETCONFIGATTRIBPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    config: EGLConfig,
    attribute: u32,
    value: *mut u32,
) -> EGLBoolean,
>;

#[link(name = "EGL")]
extern "C" {
    pub fn eglGetProcAddress(procname: *const u8) -> *mut c_void;
    pub fn eglBindAPI(api: EGLenum) -> EGLBoolean;
    pub fn eglCreateContext(
        dpy: EGLDisplay,
        config: EGLConfig,
        share_context: EGLContext,
        attrib_list: *const u32,
    ) -> EGLContext;
    pub fn eglCreateWindowSurface(
        dpy: EGLDisplay,
        config: EGLConfig,
        win: EGLNativeWindowType,
        attrib_list: *const EGLint,
    ) -> EGLSurface;
    pub fn eglMakeCurrent(
        dpy: EGLDisplay,
        draw: EGLSurface,
        read: EGLSurface,
        ctx: EGLContext,
    ) -> EGLBoolean;
    pub fn eglSwapBuffers(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean;
}
