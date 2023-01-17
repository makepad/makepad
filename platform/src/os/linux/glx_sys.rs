use crate::os::linux::x11_sys::*;

pub type GLXDrawable = XID;
pub type GLXContext = *mut __GLXcontextRec;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __GLXcontextRec {
    _unused: [u8; 0],
}

extern "C" {
    pub fn glXMakeCurrent(
        dpy: *mut Display,
        drawable: GLXDrawable,
        ctx: GLXContext,
    ) -> ::std::os::raw::c_int;

    pub fn glXSwapBuffers(dpy: *mut Display, drawable: GLXDrawable);
} 
