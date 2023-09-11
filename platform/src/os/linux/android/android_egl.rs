#![allow(non_camel_case_types, non_snake_case, dead_code)]

#[cfg(target_os = "linux")]
pub type EGLNativeDisplayType = *mut crate::native::linux_x11::libx11::Display;
#[cfg(target_os = "linux")]
pub type EGLNativePixmapType = crate::native::linux_x11::libx11::Pixmap;
#[cfg(target_os = "linux")]
pub type EGLNativeWindowType = crate::native::linux_x11::libx11::Window;

#[cfg(target_os = "android")]
pub type EGLNativeDisplayType = *mut ();
#[cfg(target_os = "android")]
pub type EGLNativePixmapType = ::std::os::raw::c_ulong;
#[cfg(target_os = "android")]
pub type EGLNativeWindowType = ::std::os::raw::c_ulong;

pub use core::ptr::null_mut;

pub const EGL_SUCCESS: u32 = 12288;

pub const EGL_WINDOW_BIT: u32 = 4;

pub const EGL_ALPHA_SIZE: u32 = 12321;
pub const EGL_BLUE_SIZE: u32 = 12322;
pub const EGL_GREEN_SIZE: u32 = 12323;
pub const EGL_RED_SIZE: u32 = 12324;
pub const EGL_DEPTH_SIZE: u32 = 12325;
pub const EGL_STENCIL_SIZE: u32 = 12326;
pub const EGL_NATIVE_VISUAL_ID: u32 = 12334;
pub const EGL_WIDTH: u32 = 12375;
pub const EGL_HEIGHT: u32 = 12374;
pub const EGL_SURFACE_TYPE: u32 = 12339;
pub const EGL_NONE: u32 = 12344;
pub const EGL_CONTEXT_CLIENT_VERSION: u32 = 12440;

pub type NativeDisplayType = EGLNativeDisplayType;
pub type NativePixmapType = EGLNativePixmapType;
pub type NativeWindowType = EGLNativeWindowType;
pub type EGLint = i32;
pub type EGLBoolean = ::std::os::raw::c_uint;
pub type EGLDisplay = *mut ::std::os::raw::c_void;
pub type EGLConfig = *mut ::std::os::raw::c_void;
pub type EGLSurface = *mut ::std::os::raw::c_void;
pub type EGLContext = *mut ::std::os::raw::c_void;
pub type __eglMustCastToProperFunctionPointerType = ::std::option::Option<unsafe extern "C" fn()>;
pub type PFNEGLCHOOSECONFIGPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        attrib_list: *const EGLint,
        configs: *mut EGLConfig,
        config_size: EGLint,
        num_config: *mut EGLint,
    ) -> EGLBoolean,
>;
pub type PFNEGLCOPYBUFFERSPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        surface: EGLSurface,
        target: EGLNativePixmapType,
    ) -> EGLBoolean,
>;
pub type PFNEGLCREATECONTEXTPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        config: EGLConfig,
        share_context: EGLContext,
        attrib_list: *const EGLint,
    ) -> EGLContext,
>;
pub type PFNEGLCREATEPBUFFERSURFACEPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        config: EGLConfig,
        attrib_list: *const EGLint,
    ) -> EGLSurface,
>;
pub type PFNEGLCREATEPIXMAPSURFACEPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        config: EGLConfig,
        pixmap: EGLNativePixmapType,
        attrib_list: *const EGLint,
    ) -> EGLSurface,
>;
pub type PFNEGLCREATEWINDOWSURFACEPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        config: EGLConfig,
        win: EGLNativeWindowType,
        attrib_list: *const EGLint,
    ) -> EGLSurface,
>;
pub type PFNEGLDESTROYCONTEXTPROC =
    ::std::option::Option<unsafe extern "C" fn(dpy: EGLDisplay, ctx: EGLContext) -> EGLBoolean>;
pub type PFNEGLDESTROYSURFACEPROC =
    ::std::option::Option<unsafe extern "C" fn(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean>;
pub type PFNEGLGETCONFIGATTRIBPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        config: EGLConfig,
        attribute: EGLint,
        value: *mut EGLint,
    ) -> EGLBoolean,
>;
pub type PFNEGLGETCONFIGSPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        configs: *mut EGLConfig,
        config_size: EGLint,
        num_config: *mut EGLint,
    ) -> EGLBoolean,
>;
pub type PFNEGLGETCURRENTDISPLAYPROC = ::std::option::Option<unsafe extern "C" fn() -> EGLDisplay>;
pub type PFNEGLGETCURRENTSURFACEPROC =
    ::std::option::Option<unsafe extern "C" fn(readdraw: EGLint) -> EGLSurface>;
pub type PFNEGLGETDISPLAYPROC =
    ::std::option::Option<unsafe extern "C" fn(display_id: EGLNativeDisplayType) -> EGLDisplay>;
pub type PFNEGLGETERRORPROC = ::std::option::Option<unsafe extern "C" fn() -> EGLint>;
pub type PFNEGLGETPROCADDRESSPROC = ::std::option::Option<
    unsafe extern "C" fn(
        procname: *const ::std::os::raw::c_char,
    ) ->  *mut c_void,
>;
pub type PFNEGLINITIALIZEPROC = ::std::option::Option<
    unsafe extern "C" fn(dpy: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> EGLBoolean,
>;
pub type PFNEGLMAKECURRENTPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        draw: EGLSurface,
        read: EGLSurface,
        ctx: EGLContext,
    ) -> EGLBoolean,
>;
pub type PFNEGLQUERYCONTEXTPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        ctx: EGLContext,
        attribute: EGLint,
        value: *mut EGLint,
    ) -> EGLBoolean,
>;
pub type PFNEGLQUERYSTRINGPROC = ::std::option::Option<
    unsafe extern "C" fn(dpy: EGLDisplay, name: EGLint) -> *const ::std::os::raw::c_char,
>;
pub type PFNEGLQUERYSURFACEPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        surface: EGLSurface,
        attribute: EGLint,
        value: *mut EGLint,
    ) -> EGLBoolean,
>;
pub type PFNEGLSWAPBUFFERSPROC =
    ::std::option::Option<unsafe extern "C" fn(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean>;
pub type PFNEGLTERMINATEPROC =
    ::std::option::Option<unsafe extern "C" fn(dpy: EGLDisplay) -> EGLBoolean>;
pub type PFNEGLWAITGLPROC = ::std::option::Option<unsafe extern "C" fn() -> EGLBoolean>;
pub type PFNEGLWAITNATIVEPROC =
    ::std::option::Option<unsafe extern "C" fn(engine: EGLint) -> EGLBoolean>;
pub type PFNEGLBINDTEXIMAGEPROC = ::std::option::Option<
    unsafe extern "C" fn(dpy: EGLDisplay, surface: EGLSurface, buffer: EGLint) -> EGLBoolean,
>;
pub type PFNEGLRELEASETEXIMAGEPROC = ::std::option::Option<
    unsafe extern "C" fn(dpy: EGLDisplay, surface: EGLSurface, buffer: EGLint) -> EGLBoolean,
>;
pub type PFNEGLSURFACEATTRIBPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: EGLDisplay,
        surface: EGLSurface,
        attribute: EGLint,
        value: EGLint,
    ) -> EGLBoolean,
>;
pub type PFNEGLSWAPINTERVALPROC =
    ::std::option::Option<unsafe extern "C" fn(dpy: EGLDisplay, interval: EGLint) -> EGLBoolean>;

pub struct LibEgl {
    pub module: crate::native::module::Module,
    pub eglChooseConfig: PFNEGLCHOOSECONFIGPROC,
    pub eglCopyBuffers: PFNEGLCOPYBUFFERSPROC,
    pub eglCreateContext: PFNEGLCREATECONTEXTPROC,
    pub eglCreatePbufferSurface: PFNEGLCREATEPBUFFERSURFACEPROC,
    pub eglCreatePixmapSurface: PFNEGLCREATEPIXMAPSURFACEPROC,
    pub eglCreateWindowSurface: PFNEGLCREATEWINDOWSURFACEPROC,
    pub eglDestroyContext: PFNEGLDESTROYCONTEXTPROC,
    pub eglDestroySurface: PFNEGLDESTROYSURFACEPROC,
    pub eglGetConfigAttrib: PFNEGLGETCONFIGATTRIBPROC,
    pub eglGetConfigs: PFNEGLGETCONFIGSPROC,
    pub eglGetCurrentDisplay: PFNEGLGETCURRENTDISPLAYPROC,
    pub eglGetCurrentSurface: PFNEGLGETCURRENTSURFACEPROC,
    pub eglGetDisplay: PFNEGLGETDISPLAYPROC,
    pub eglGetError: PFNEGLGETERRORPROC,
    pub eglGetProcAddress: PFNEGLGETPROCADDRESSPROC,
    pub eglInitialize: PFNEGLINITIALIZEPROC,
    pub eglMakeCurrent: PFNEGLMAKECURRENTPROC,
    pub eglQueryContext: PFNEGLQUERYCONTEXTPROC,
    pub eglQueryString: PFNEGLQUERYSTRINGPROC,
    pub eglQuerySurface: PFNEGLQUERYSURFACEPROC,
    pub eglSwapBuffers: PFNEGLSWAPBUFFERSPROC,
    pub eglTerminate: PFNEGLTERMINATEPROC,
    pub eglWaitGL: PFNEGLWAITGLPROC,
    pub eglWaitNative: PFNEGLWAITNATIVEPROC,
    pub eglBindTexImage: PFNEGLBINDTEXIMAGEPROC,
    pub eglReleaseTexImage: PFNEGLRELEASETEXIMAGEPROC,
    pub eglSurfaceAttrib: PFNEGLSURFACEATTRIBPROC,
    pub eglSwapInterval: PFNEGLSWAPINTERVALPROC,
}

impl LibEgl {
    pub fn try_load() -> Option<LibEgl> {
        module::Module::load("libEGL.so")
            .or_else(|_| module::Module::load("libEGL.so.1"))
            .map(|module| LibEgl {
                eglChooseConfig: module.get_symbol("eglChooseConfig").ok(),
                eglCopyBuffers: module.get_symbol("eglCopyBuffers").ok(),
                eglCreateContext: module.get_symbol("eglCreateContext").ok(),
                eglCreatePbufferSurface: module.get_symbol("eglCreatePbufferSurface").ok(),
                eglCreatePixmapSurface: module.get_symbol("eglCreatePixmapSurface").ok(),
                eglCreateWindowSurface: module.get_symbol("eglCreateWindowSurface").ok(),
                eglDestroyContext: module.get_symbol("eglDestroyContext").ok(),
                eglDestroySurface: module.get_symbol("eglDestroySurface").ok(),
                eglGetConfigAttrib: module.get_symbol("eglGetConfigAttrib").ok(),
                eglGetConfigs: module.get_symbol("eglGetConfigs").ok(),
                eglGetCurrentDisplay: module.get_symbol("eglGetCurrentDisplay").ok(),
                eglGetCurrentSurface: module.get_symbol("eglGetCurrentSurface").ok(),
                eglGetDisplay: module.get_symbol("eglGetDisplay").ok(),
                eglGetError: module.get_symbol("eglGetError").ok(),
                eglGetProcAddress: module.get_symbol("eglGetProcAddress").ok(),
                eglInitialize: module.get_symbol("eglInitialize").ok(),
                eglMakeCurrent: module.get_symbol("eglMakeCurrent").ok(),
                eglQueryContext: module.get_symbol("eglQueryContext").ok(),
                eglQueryString: module.get_symbol("eglQueryString").ok(),
                eglQuerySurface: module.get_symbol("eglQuerySurface").ok(),
                eglSwapBuffers: module.get_symbol("eglSwapBuffers").ok(),
                eglTerminate: module.get_symbol("eglTerminate").ok(),
                eglWaitGL: module.get_symbol("eglWaitGL").ok(),
                eglWaitNative: module.get_symbol("eglWaitNative").ok(),
                eglBindTexImage: module.get_symbol("eglBindTexImage").ok(),
                eglReleaseTexImage: module.get_symbol("eglReleaseTexImage").ok(),
                eglSurfaceAttrib: module.get_symbol("eglSurfaceAttrib").ok(),
                eglSwapInterval: module.get_symbol("eglSwapInterval").ok(),
                module,
            })
            .ok()
    }
}

#[derive(Debug)]
pub enum EglError {
    NoDisplay,
    InitializeFailed,
    CreateContextFailed,
}

pub struct Egl {}

pub unsafe fn create_egl_context(
    egl: &mut LibEgl,
    display: *mut std::ffi::c_void,
    alpha: bool,
) -> Result<(EGLContext, EGLConfig, EGLDisplay), EglError> {
    let display = (egl.eglGetDisplay.unwrap())(display as _);
    if display == /* EGL_NO_DISPLAY */ null_mut() {
        return Err(EglError::NoDisplay);
    }

    if (egl.eglInitialize.unwrap())(display, null_mut(), null_mut()) == 0 {
        return Err(EglError::InitializeFailed);
    }

    let alpha_size = if alpha { 8 } else { 0 };
    #[rustfmt::skip]
    let cfg_attributes = vec![
        EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, alpha_size,
        EGL_DEPTH_SIZE, 16,
        EGL_STENCIL_SIZE, 0,
        EGL_NONE,
    ];
    let mut available_cfgs: Vec<EGLConfig> = vec![null_mut(); 32];
    let mut cfg_count = 0;

    (egl.eglChooseConfig.unwrap())(
        display,
        cfg_attributes.as_ptr() as _,
        available_cfgs.as_ptr() as _,
        32,
        &mut cfg_count as *mut _ as *mut _,
    );
    assert!(cfg_count > 0);
    assert!(cfg_count <= 32);

    // find config with 8-bit rgb buffer if available, ndk sample does not trust egl spec
    let mut config: EGLConfig = null_mut();
    let mut exact_cfg_found = false;
    for c in &mut available_cfgs[0..cfg_count] {
        let mut r: i32 = 0;
        let mut g: i32 = 0;
        let mut b: i32 = 0;
        let mut a: i32 = 0;
        let mut d: i32 = 0;
        if (egl.eglGetConfigAttrib.unwrap())(display, *c, EGL_RED_SIZE as _, &mut r) == 1
            && (egl.eglGetConfigAttrib.unwrap())(display, *c, EGL_GREEN_SIZE as _, &mut g) == 1
            && (egl.eglGetConfigAttrib.unwrap())(display, *c, EGL_BLUE_SIZE as _, &mut b) == 1
            && (egl.eglGetConfigAttrib.unwrap())(display, *c, EGL_ALPHA_SIZE as _, &mut a) == 1
            && (egl.eglGetConfigAttrib.unwrap())(display, *c, EGL_DEPTH_SIZE as _, &mut d) == 1
            && r == 8
            && g == 8
            && b == 8
            && (alpha_size == 0 || a == alpha_size as _)
            && d == 16
        {
            exact_cfg_found = true;
            config = *c;
            break;
        }
    }
    if !exact_cfg_found {
        config = available_cfgs[0];
    }
    let ctx_attributes = vec![EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
    let context = (egl.eglCreateContext.unwrap())(
        display,
        config,
        /* EGL_NO_CONTEXT */ null_mut(),
        ctx_attributes.as_ptr() as _,
    );
    if context.is_null() {
        return Err(EglError::CreateContextFailed);
    }

    return Ok((context, config, display));
}
