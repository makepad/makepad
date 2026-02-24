use std::ffi::c_void;

use crate::cx::Cx;
use crate::texture::Texture;

/// GL API type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlApi {
    /// Desktop OpenGL (macOS).
    GL,
    /// OpenGL ES (Linux, Android, Windows via ANGLE).
    GLES,
}

/// Cross-platform GL rendering bridge.
///
/// Manages a GL context that shares GPU memory with makepad's native renderer.
/// External code renders via GL; makepad displays the result with zero-copy.
///
/// Platform backends:
/// - Linux/Android: wraps makepad's existing EGL context
/// - Windows: ANGLE EGL context on makepad's D3D11 device (via libEGL.dll)
/// - macOS: standalone CGL context bridged to Metal via IOSurface
pub struct GlRenderBridge {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) inner: crate::os::linux::opengl::EglRenderBridge,
    #[cfg(target_os = "windows")]
    pub(crate) inner: crate::os::windows::angle::AngleRenderBridge,
    #[cfg(target_os = "macos")]
    pub(crate) inner: crate::os::apple::metal::CglRenderBridge,
}

impl GlRenderBridge {
    /// Make this GL context current on the calling thread.
    pub fn make_current(&self) {
        self.inner.make_current()
    }

    /// Look up a GL/EGL function by name.
    pub fn get_proc_address(&self, name: &str) -> *const c_void {
        self.inner.get_proc_address(name)
    }

    /// GL API type (GL on macOS, GLES on Linux/Android/Windows).
    pub fn gl_api(&self) -> GlApi {
        self.inner.gl_api()
    }
}

// EGL platform accessors (Linux, Android, Windows)
#[cfg(any(target_os = "linux", target_os = "android", target_os = "windows"))]
impl GlRenderBridge {
    pub fn egl_display(&self) -> *mut c_void {
        self.inner.egl_display()
    }

    pub fn egl_config(&self) -> *mut c_void {
        self.inner.egl_config()
    }

    pub fn egl_context(&self) -> *mut c_void {
        self.inner.egl_context()
    }
}

// CGL platform accessors (macOS)
#[cfg(target_os = "macos")]
impl GlRenderBridge {
    pub fn cgl_pixel_format(&self) -> *mut c_void {
        self.inner.cgl_pixel_format()
    }

    pub fn cgl_context(&self) -> *mut c_void {
        self.inner.cgl_context()
    }
}

// Cx methods: Linux/Android
#[cfg(any(target_os = "linux", target_os = "android"))]
impl Cx {
    /// Create a GL rendering bridge wrapping makepad's existing EGL context.
    pub fn create_gl_render_bridge(&mut self) -> GlRenderBridge {
        let opengl_cx = self.os.opengl_cx.as_ref().expect("OpenGL context not initialized");
        GlRenderBridge {
            inner: crate::os::linux::opengl::EglRenderBridge::new(opengl_cx),
        }
    }

    /// Create a texture renderable via GL and displayable by makepad.
    /// Returns (Texture handle, GL texture ID).
    pub fn create_gl_render_bridge_texture(
        &mut self,
        _bridge: &GlRenderBridge,
        width: usize,
        height: usize,
    ) -> (Texture, u32) {
        self.create_gl_render_texture(width, height)
    }

    /// Restore makepad's own GL context as current.
    pub fn restore_gl_context(&mut self) {
        let opengl_cx = self.os.opengl_cx.as_ref().expect("OpenGL context not initialized");
        opengl_cx.make_current();
    }
}

// Cx methods: macOS
#[cfg(target_os = "macos")]
impl Cx {
    /// Create a GL rendering bridge with a standalone CGL context (GL 3.2 Core).
    pub fn create_gl_render_bridge(&mut self) -> GlRenderBridge {
        GlRenderBridge {
            inner: crate::os::apple::metal::CglRenderBridge::new(),
        }
    }

    /// Create a texture renderable via GL (CGL) and displayable by makepad (Metal).
    /// Returns (Texture handle, GL texture ID for rendering into).
    pub fn create_gl_render_bridge_texture(
        &mut self,
        bridge: &GlRenderBridge,
        width: usize,
        height: usize,
    ) -> (Texture, u32) {
        bridge.inner.make_current();
        let (texture, iosurface_ref, _iosurface_id) =
            self.create_iosurface_render_texture(width, height);
        let gl_texture_id = bridge.inner.bind_iosurface_to_gl_texture(
            iosurface_ref,
            width,
            height,
        );
        (texture, gl_texture_id)
    }

    /// Restore makepad's own rendering context. No-op on macOS (CGL and Metal are independent).
    pub fn restore_gl_context(&mut self) {}
}

// Cx methods: Windows
#[cfg(target_os = "windows")]
impl Cx {
    /// Create a GL rendering bridge via ANGLE on makepad's D3D11 device.
    pub fn create_gl_render_bridge(&mut self) -> GlRenderBridge {
        let d3d11_device = self.os.d3d11_device.as_ref()
            .expect("D3D11 device not initialized")
            .clone();
        GlRenderBridge {
            inner: crate::os::windows::angle::AngleRenderBridge::new(d3d11_device),
        }
    }

    /// Create a D3D11 render target texture importable into ANGLE as a GL texture.
    /// Returns (Texture handle, GL texture ID).
    pub fn create_gl_render_bridge_texture(
        &mut self,
        bridge: &GlRenderBridge,
        width: usize,
        height: usize,
    ) -> (Texture, u32) {
        bridge.inner.create_render_texture(self, width, height)
    }

    /// Restore makepad's own rendering context. No-op on Windows (ANGLE and D3D11 are independent).
    pub fn restore_gl_context(&mut self) {}
}
