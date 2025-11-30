#![allow(non_upper_case_globals)]
use self::super::x11_sys::*;

use std::os::raw::{
    c_int, c_void
};

pub(crate) type GLXDrawable = XID;
pub(crate) type GLXContext = *mut c_void;

pub(crate) const True: u32 = 1;

#[link(name = "GLX")]
extern "C" {
    pub(crate) fn glXCreateContext(
        dpy: *mut Display,
        vis: *mut XVisualInfo,
        shareList: GLXContext,
        direct: c_int,
    ) -> GLXContext;

    pub(crate) fn glXMakeCurrent(
        dpy: *mut Display,
        drawable: GLXDrawable,
        ctx: GLXContext,
    ) -> c_int;

    pub(crate) fn glXSwapBuffers(dpy: *mut Display, drawable: GLXDrawable);
} 
