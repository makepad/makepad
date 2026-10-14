//! The app side of the GLES backend's GL interop: a GLES 3 context of the HAL's share group
//! (the `GlShare` event names it), current on the app's own thread. GL work of the app never runs
//! on the HAL's render context. Texture and sync names are shared within the group: textures
//! cross by name (`TextureAdoptGl`), and a fence (`GlFence`) after the app's writes makes the
//! HAL's frames wait for them on the GPU.
use crate::backend::gl::egl::{Egl, Handle};
use makepad_relax_api::relax_api;
use std::ffi::{c_void, CString};

/// A handle to JIT-compiled code (makepad run): the libEGL it uses is this crate's one load.
#[relax_api(handle)]
pub struct GlContext {
    egl: &'static Egl,
    display: Handle,
    config: Handle,
    context: Handle,
    fence_sync: unsafe extern "C" fn(u32, u32) -> *const c_void,
    flush: unsafe extern "C" fn(),
}

// SAFETY: EGL contexts may be made current on any (one) thread at a time; the handles are values.
unsafe impl Send for GlContext {}

#[relax_api]
impl GlContext {
    /// A context sharing the HAL's names: `share` is the `GlShare` event's display, config, context.
    /// None when it cannot be made (the reason is logged); an op, so the one libEGL load and the
    /// EGL entry points stay on this crate's side.
    pub fn new(share: [u64; 3]) -> Option<Self> {
        Self::make(share).map_err(|e| eprintln!("[hal-gl] GL interop context: {e}")).ok()
    }

    fn make(share: [u64; 3]) -> Result<Self, String> {
        let egl = Egl::load()?;
        let [display, config, shared] = share.map(|v| v as Handle);
        let context = unsafe { (egl.create_context)(display, config, shared, [0x3098, 3, 0x3038].as_ptr()) };
        if context.is_null() {
            return Err(format!("eglCreateContext (HAL share group): {:#x}", unsafe { (egl.error)() }));
        }
        let proc = |name: &std::ffi::CStr| unsafe { (egl.get_proc)(name.as_ptr()) };
        let (fence_sync, flush) = (proc(c"glFenceSync"), proc(c"glFlush"));
        if fence_sync.is_null() || flush.is_null() {
            unsafe { (egl.destroy_context)(display, context) };
            return Err("the GLES driver lacks glFenceSync / glFlush".into());
        }
        // SAFETY: GLES 3.0 entry points of these signatures
        let (fence_sync, flush) = unsafe { (std::mem::transmute(fence_sync), std::mem::transmute(flush)) };
        Ok(Self { egl, display, config, context, fence_sync, flush })
    }

    /// Makes the context current on the calling thread, with no surface.
    pub fn make_current(&self) -> bool {
        unsafe { (self.egl.make_current)(self.display, std::ptr::null_mut(), std::ptr::null_mut(), self.context) != 0 }
    }

    /// A GL / EGL entry point (null when the driver has none).
    pub fn proc_address(&self, name: &str) -> *const c_void {
        let Ok(name) = CString::new(name) else { return std::ptr::null() };
        unsafe { (self.egl.get_proc)(name.as_ptr()) as *const c_void }
    }

    /// A fence after the commands issued so far on the current context, flushed so the HAL's
    /// context can wait for it. The HAL deletes it (`GlFence`).
    pub fn fence(&self) -> u64 {
        unsafe {
            let sync = (self.fence_sync)(0x9117, 0);
            (self.flush)();
            sync as u64
        }
    }

    pub fn display(&self) -> *mut c_void {
        self.display
    }
    pub fn config(&self) -> *mut c_void {
        self.config
    }
    pub fn context(&self) -> *mut c_void {
        self.context
    }
}

impl Drop for GlContext {
    fn drop(&mut self) {
        // EGL destroys a context that is current somewhere once it is released there
        unsafe { (self.egl.destroy_context)(self.display, self.context) };
    }
}
