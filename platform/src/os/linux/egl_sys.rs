#![allow(non_camel_case_types, non_snake_case, dead_code)]

pub type EGLNativeDisplayType = *mut ();
pub type EGLNativePixmapType = ::std::os::raw::c_ulong;
pub type EGLNativeWindowType = ::std::os::raw::c_ulong;

pub use core::ptr::null_mut;

pub const EGL_NO_CONTEXT: EGLContext = 0 as EGLContext;
pub const EGL_NO_SURFACE: EGLSurface = 0 as EGLSurface;

pub const EGL_WINDOW_BIT: u32 = 4;

pub const EGL_OPENGL_ES2_BIT: u32 = 4;

pub const EGL_SUCCESS: u32 = 12288;
pub const EGL_ALPHA_SIZE: u32 = 12321;
pub const EGL_BLUE_SIZE: u32 = 12322;
pub const EGL_GREEN_SIZE: u32 = 12323;
pub const EGL_RED_SIZE: u32 = 12324;
pub const EGL_DEPTH_SIZE: u32 = 12325;
pub const EGL_STENCIL_SIZE: u32 = 12326;
pub const EGL_NATIVE_VISUAL_ID: u32 = 12334;
pub const EGL_SURFACE_TYPE: u32 = 12339;
pub const EGL_NONE: u32 = 12344;
pub const EGL_RENDERABLE_TYPE: u32 = 12352;
pub const EGL_HEIGHT: u32 = 12374;
pub const EGL_WIDTH: u32 = 12375;
pub const EGL_CONTEXT_CLIENT_VERSION: u32 = 12440;
pub const EGL_OPENGL_ES_API: u32 = 12448;

pub const EGL_GL_TEXTURE_2D_KHR: u32 = 12465;

pub const EGL_PLATFORM_X11_EXT: u32 = 12757;
pub const EGL_PLATFORM_GBM_KHR: u32 = 12759;

pub const EGL_LINUX_DMA_BUF_EXT: u32 = 12912;
pub const EGL_LINUX_DRM_FOURCC_EXT: u32 = 12913;
pub const EGL_DMA_BUF_PLANE0_FD_EXT: u32 = 12914;
pub const EGL_DMA_BUF_PLANE0_OFFSET_EXT: u32 = 12915;
pub const EGL_DMA_BUF_PLANE0_PITCH_EXT: u32 = 12916;
pub const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: u32 = 13379;
pub const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: u32 = 13380;
pub const EGL_SWAP_BEHAVIOR: i32 = 0x3093;
pub const EGL_BUFFER_PRESERVED: i32 = 0x3094;
pub const EGL_BUFFER_DESTROYED: i32 = 0x3095;
pub type NativeDisplayType = EGLNativeDisplayType;
pub type NativePixmapType = EGLNativePixmapType;
pub type NativeWindowType = EGLNativeWindowType;
pub type EGLint = i32;
pub type EGLuint64KHR = u64;
pub type EGLenum = ::std::os::raw::c_uint;
pub type EGLBoolean = ::std::os::raw::c_uint;
pub type EGLDisplay = *mut ::std::os::raw::c_void;
pub type EGLConfig = *mut ::std::os::raw::c_void;
pub type EGLSurface = *mut ::std::os::raw::c_void;
pub type EGLContext = *mut ::std::os::raw::c_void;
pub type EGLClientBuffer = *mut ::std::os::raw::c_void;
pub type EGLImageKHR = *mut ::std::os::raw::c_void;
pub type __eglMustCastToProperFunctionPointerType = ::std::option::Option<unsafe extern "C" fn()>;
pub type PFNEGLBINDAPIPROC = ::std::option::Option<unsafe extern "C" fn(api: EGLenum) -> EGLBoolean>;
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
pub type PFNEGLCREATEIMAGEKHRPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    ctx: EGLContext,
    target: EGLenum,
    buffer: EGLClientBuffer,
    attrib_list: *const EGLint,
) -> EGLImageKHR,
>;
pub type PFNEGLDESTROYIMAGEKHRPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    image: EGLImageKHR,
) -> EGLBoolean,
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
) -> *mut ::std::os::raw::c_void,
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

pub type PFNEGLGETPLATFORMDISPLAYEXTPROC = ::std::option::Option<
unsafe extern "C" fn(
    platform: EGLenum,
    native_display: *mut ::std::os::raw::c_void,
    attrib_list: *const EGLint,
) -> EGLDisplay,
>;

pub type PFNEGLEXPORTDMABUFIMAGEQUERYMESAPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    image: EGLImageKHR,
    fourcc: *mut i32,
    num_planes: *mut i32,
    modifiers: *mut EGLuint64KHR,
) -> EGLBoolean,
>;
pub type PFNEGLEXPORTDMABUFIMAGEMESAPROC = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    image: EGLImageKHR,
    fds: *mut i32,
    strides: *mut EGLint,
    offsets: *mut EGLint,
) -> EGLBoolean,
>;

// HACK(eddyb) this is actually an OpenGL extension function.
type PFNGLEGLIMAGETARGETTEXTURE2DOESPROC = ::std::option::Option<
unsafe extern "C" fn(
    super::gl_sys::GLenum,
    EGLImageKHR,
),
>;

type PFNEGLPRESENTATIONTIMEANDROID = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    surface: EGLSurface,
    time: i64
),
>;

type PFNEGLSURFACEATTRIB = ::std::option::Option<
unsafe extern "C" fn(
    dpy: EGLDisplay,
    surface: EGLSurface,
    attrib: EGLint,
    value: EGLint
)->EGLBoolean,
>;


struct Module(::std::ptr::NonNull<::std::os::raw::c_void>);

pub struct LibEgl {
    pub eglPresentationTimeANDROID: PFNEGLPRESENTATIONTIMEANDROID,
    pub eglBindAPI: PFNEGLBINDAPIPROC,
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

    pub eglCreateImageKHR: PFNEGLCREATEIMAGEKHRPROC,
    pub eglDestroyImageKHR: PFNEGLDESTROYIMAGEKHRPROC,
    pub eglExportDMABUFImageQueryMESA: PFNEGLEXPORTDMABUFIMAGEQUERYMESAPROC,
    pub eglExportDMABUFImageMESA: PFNEGLEXPORTDMABUFIMAGEMESAPROC,
    pub eglGetPlatformDisplayEXT: PFNEGLGETPLATFORMDISPLAYEXTPROC,

    // HACK(eddyb) this is actually an OpenGL extension function.
    pub glEGLImageTargetTexture2DOES: PFNGLEGLIMAGETARGETTEXTURE2DOESPROC,

    _keep_module_alive: Module,
}

use self::super::libc_sys::{dlclose, dlopen, dlsym, RTLD_LAZY, RTLD_LOCAL};
use std::{
    ffi::{CString, CStr},
    ptr::NonNull,
};

impl Module {
    pub fn load(path: &str) -> Result<Self,()> {
        let path = CString::new(path).unwrap();
                        
        let module = unsafe {dlopen(path.as_ptr(), RTLD_LAZY | RTLD_LOCAL)};
        if module.is_null() {
            Err(())
        } else {
            Ok(Module(unsafe {NonNull::new_unchecked(module)}))
        }
    }
                
    pub fn get_symbol<F: Sized>(&self, name: &str) -> Result<F, ()> {
        let name = CString::new(name).unwrap();
                        
        let symbol = unsafe {dlsym(self.0.as_ptr(), name.as_ptr())};
                        
        if symbol.is_null() {
            return Err(());
        }
                        
        Ok(unsafe {std::mem::transmute_copy::<_, F>(&symbol)})
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        unsafe {dlclose(self.0.as_ptr())};
    }
}

impl LibEgl {
    pub fn try_load() -> Option<LibEgl> {
        
        let module = Module::load("libEGL.so").or_else(|_| Module::load("libEGL.so.1")).ok()?;

        let eglGetProcAddress: PFNEGLGETPROCADDRESSPROC = module.get_symbol("eglGetProcAddress").ok();
        macro_rules! get_ext_fn {
            ($name:literal) => {
                eglGetProcAddress.and_then(|gpa| unsafe {
                    std::mem::transmute(gpa(CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()).unwrap().as_ptr()))
                })
            }
        }

        Some(LibEgl {
            eglPresentationTimeANDROID: module.get_symbol("eglPresentationTimeANDROID").ok(),
            eglBindAPI: module.get_symbol("eglBindAPI").ok(),
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
            eglGetProcAddress,
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

            eglCreateImageKHR: get_ext_fn!("eglCreateImageKHR"),
            eglDestroyImageKHR: get_ext_fn!("eglDestroyImageKHR"),
            eglExportDMABUFImageQueryMESA: get_ext_fn!("eglExportDMABUFImageQueryMESA"),
            eglExportDMABUFImageMESA: get_ext_fn!("eglExportDMABUFImageMESA"),
            eglGetPlatformDisplayEXT: get_ext_fn!("eglGetPlatformDisplayEXT"),

            glEGLImageTargetTexture2DOES: get_ext_fn!("glEGLImageTargetTexture2DOES"),

            _keep_module_alive: module,
        })
    }
}

#[derive(Debug)]
pub enum EglError {
    NoDisplay,
    InitializeFailed,
    CreateContextFailed,
    ChooseConfigFailed,
}

pub struct Egl {}

#[cfg(target_os="android")]
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

    let alpha_size = if alpha {8} else {0};
    #[rustfmt::skip]
    let cfg_attributes = vec![
        EGL_SURFACE_TYPE,
        EGL_WINDOW_BIT,
        EGL_RED_SIZE,
        8,
        EGL_GREEN_SIZE,
        8,
        EGL_BLUE_SIZE,
        8,
        EGL_ALPHA_SIZE,
        alpha_size,
        EGL_DEPTH_SIZE,
        24,
        EGL_STENCIL_SIZE,
        0,
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

#[cfg(target_env="ohos")]
pub unsafe  fn create_egl_context(
    egl: &mut LibEgl
) -> Result<(EGLContext, EGLConfig, EGLDisplay), EglError> {
    let display = (egl.eglGetDisplay.unwrap())(null_mut());
    if display == null_mut() {
        return Err(EglError::NoDisplay);
    }

    if (egl.eglInitialize.unwrap())(display,null_mut(),null_mut()) == 0 {
        return Err(EglError::InitializeFailed);
    }

    #[rustfmt::skip]
    let cfg_attributes = vec![
        EGL_SURFACE_TYPE,
        EGL_WINDOW_BIT,
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, 8,
        EGL_RENDERABLE_TYPE,
        EGL_OPENGL_ES2_BIT,
        EGL_DEPTH_SIZE, 0,
        EGL_STENCIL_SIZE, 0,
        EGL_NONE
    ];
    let available_cfgs: Vec<EGLConfig> = vec![null_mut(); 1];
    let mut cfg_count = 0;

    if (egl.eglChooseConfig.unwrap())(
        display,
        cfg_attributes.as_ptr() as _,
        available_cfgs.as_ptr() as _,
        1,&mut cfg_count as *mut _ as *mut _,
    ) == 0 {
        return Err(EglError::ChooseConfigFailed);
    }

    assert!(cfg_count > 0);

    let config = available_cfgs[0];

    let ctx_attributes = vec![EGL_CONTEXT_CLIENT_VERSION,2,EGL_NONE];
    let context = (egl.eglCreateContext.unwrap())(
        display,
        config,
        /* EGL_NO_CONTEXT */ null_mut(),
        ctx_attributes.as_ptr() as _
    );
    if context.is_null(){
        return Err(EglError::CreateContextFailed);
    }
    crate::log!("create elg context success");
    return Ok((context, config, display));
}
