use std::ffi::c_void;

/// GL API type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlApi {
    /// Desktop OpenGL.
    GL,
    /// OpenGL ES (Linux, Android).
    GLES,
}

/// Cross-platform GL rendering bridge.
///
/// Manages a GL context that shares GPU memory with makepad's native renderer.
/// External code renders via GL; makepad displays the result with zero-copy.
///
/// Linux: a context in the share group of the HAL's GL backend (present only when the HAL runs
/// on GL); elsewhere the bridge has no context and its calls do nothing.
pub struct GlRenderBridge {
    /// A context of the GLES HAL backend's share group; none on the other HAL backends.
    #[cfg(target_os = "linux")]
    pub(crate) inner: Option<makepad_hal::gl_interop::GlContext>,
}

#[cfg(not(target_os = "linux"))]
impl GlRenderBridge {
    pub fn make_current(&self) {}

    pub fn get_proc_address(&self, _name: &str) -> *const c_void {
        std::ptr::null()
    }

    pub fn gl_api(&self) -> GlApi {
        GlApi::GLES
    }
}

/// The GLES HAL backend: the bridge's context, when the backend is GL.
#[cfg(target_os = "linux")]
impl GlRenderBridge {
    /// Make this GL context current on the calling thread (no-op without GL).
    pub fn make_current(&self) {
        if let Some(context) = &self.inner {
            context.make_current();
        }
    }

    /// Look up a GL/EGL function by name (null without GL).
    pub fn get_proc_address(&self, name: &str) -> *const c_void {
        self.inner.as_ref().map_or(std::ptr::null(), |c| c.proc_address(name))
    }

    pub fn gl_api(&self) -> GlApi {
        GlApi::GLES
    }

    pub fn egl_display(&self) -> *mut c_void {
        self.inner.as_ref().map_or(std::ptr::null_mut(), |c| c.display())
    }

    pub fn egl_config(&self) -> *mut c_void {
        self.inner.as_ref().map_or(std::ptr::null_mut(), |c| c.config())
    }

    pub fn egl_context(&self) -> *mut c_void {
        self.inner.as_ref().map_or(std::ptr::null_mut(), |c| c.context())
    }
}
