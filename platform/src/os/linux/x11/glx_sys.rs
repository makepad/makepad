#![allow(non_upper_case_globals)]
use self::super::x11_sys::*;
use self::super::super::gl_sys::*;

use std::os::raw::{
    c_int,
    c_char,
};

pub type GLXDrawable = XID;
pub type GLXContext = *mut __GLXcontextRec;

pub const GLX_DOUBLEBUFFER: u32 = 5; 
pub const GLX_RED_SIZE: u32 = 8;
pub const GLX_GREEN_SIZE: u32 = 9;
pub const GLX_BLUE_SIZE: u32 = 10;
pub const GLX_ALPHA_SIZE: u32 = 11;
pub const GLX_DEPTH_SIZE: u32 = 12;
pub const None: u32 = 0;
pub const True: u32 = 1;
pub const False: u32 = 0;
pub const GLX_CONTEXT_MAJOR_VERSION_ARB: u32 = 8337;
pub const GLX_CONTEXT_MINOR_VERSION_ARB: u32 = 8338;
pub const GLX_CONTEXT_PROFILE_MASK_ARB: u32 = 37158;
pub const GLX_CONTEXT_ES_PROFILE_BIT_EXT: u32 = 4;

pub type __GLXextFuncPtr = ::std::option::Option<unsafe extern "C" fn()>;
pub type GLXFBConfig = *mut __GLXFBConfigRec;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __GLXcontextRec {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __GLXFBConfigRec {
    _unused: [u8; 0],
}
pub type PFNGLXCREATECONTEXTATTRIBSARBPROC = ::std::option::Option<
    unsafe extern "C" fn(
        dpy: *mut Display,
        config: GLXFBConfig,
        share_context: GLXContext,
        direct: c_int,
        attrib_list: *const c_int,
    ) -> GLXContext,
>;

#[link(name = "GLX")]
extern "C" {
    pub fn glXGetProcAddressARB(arg1: *const GLubyte) -> __GLXextFuncPtr;
    
    pub fn glXChooseFBConfig(
        dpy: *mut Display,
        screen: c_int,
        attribList: *const c_int,
        nitems: *mut c_int,
    ) -> *mut GLXFBConfig;
        
    pub fn glXMakeCurrent(
        dpy: *mut Display,
        drawable: GLXDrawable,
        ctx: GLXContext,
    ) -> c_int;

    pub fn glXSwapBuffers(dpy: *mut Display, drawable: GLXDrawable);
    
    pub fn glXQueryVersion(
        dpy: *mut Display,
        maj: *mut c_int,
        min: *mut c_int,
    ) -> c_int;    

    pub fn glXQueryExtensionsString(
        dpy: *mut Display,
        screen: c_int,
    ) -> *const c_char;

    pub fn glXGetVisualFromFBConfig(dpy: *mut Display, config: GLXFBConfig) -> *mut XVisualInfo;
} 
