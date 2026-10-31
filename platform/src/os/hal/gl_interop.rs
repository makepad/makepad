//! The GL interop API on the HAL: `Cx::with_gl`, the GL render bridge, `create_gl_render_texture`
//! and `Texture::adopt_gl_*`, with the signatures the native GL renderer had.
//!
//! Only the GLES backend (Linux) has GL: its `GlShare` event names its EGL share group. The app's
//! GL work runs on a context of that group made current on the Cx thread (never on the HAL's
//! render context, which lives on the HAL's render thread). Textures cross by GL name
//! (`TextureAdoptGl`) and every `with_gl` ends with a fence (`GlFence`) that the HAL's next frames
//! wait for on the GPU, so the HAL never samples a texture the app is still writing.
//!
//! On the Vulkan, Metal and D3D11 backends GL is not available: `with_gl` answers `None`, a
//! bridge has no context (no-op `make_current`, null proc addresses and EGL handles), a render
//! texture comes with GL name 0, and the adopt calls answer `Err`.

use crate::{
    cx::Cx,
    gl_render_bridge::GlRenderBridge,
    os::gl_sys::LibGl,
    texture::{Texture, TextureAlloc, TextureCategory, TextureFormat, TexturePixel, TextureSize},
};
use makepad_hal::format;

/// The GL interop state of a Cx: the share group the backend named, and the Cx thread's context.
#[derive(Default)]
pub(crate) struct GlInterop {
    pub(crate) share: Option<[u64; 3]>,
    #[cfg(target_os = "linux")]
    context: Option<(makepad_hal::gl_interop::GlContext, LibGl)>,
}

/// GL_RGBA8, the internal format of the render textures (the GL table has no constant for it)
const GL_RGBA8: i32 = 0x8058;

const NOT_AVAILABLE: &str = "GL interop needs the GLES HAL backend (MAKEPAD_HAL_BACKEND=gl, or a machine without Vulkan)";

impl Cx {
    /// Run `f` with Makepad's GL function table, on a GL context of the HAL's share group that is
    /// current on this (the Cx) thread. Textures made here can be adopted by name; the HAL's next
    /// frames wait for the work `f` issued. Returns `None` when the backend has no GL.
    pub fn with_gl<R>(&mut self, f: impl FnOnce(&LibGl) -> R) -> Option<R> {
        #[cfg(target_os = "linux")]
        {
            let share = self.hal.gl.share?;
            if self.hal.gl.context.is_none() {
                let context = makepad_hal::gl_interop::GlContext::new(share)?;
                if !context.make_current() {
                    crate::error!("GL interop: the share-group context cannot be made current");
                    return None;
                }
                let lib = LibGl::try_load(|names| {
                    names.iter().map(|n| context.proc_address(n)).find(|p| !p.is_null()).unwrap_or(std::ptr::null())
                }).map_err(|e| crate::error!("GL interop: {e}")).ok()?;
                self.hal.gl.context = Some((context, lib));
            }
            let (context, lib) = self.hal.gl.context.as_ref()?;
            if !context.make_current() {
                return None;
            }
            let out = f(lib);
            let sync = context.fence();
            self.hal.frames.w.gl_fence(sync);
            Some(out)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = f;
            None
        }
    }

    /// Create a texture renderable via GL and displayable by makepad: an RGBA8 GL texture of the
    /// share group that makepad owns (deleted with the texture). Returns (Texture handle, GL
    /// texture ID); the ID is 0 when the backend has no GL.
    pub fn create_gl_render_texture(&mut self, width: usize, height: usize) -> (Texture, u32) {
        let texture = Texture::new_with_format(self, TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width, height }, initial: true });
        if width == 0 || height == 0 {
            return (texture, 0);
        }
        let name = self.with_gl(|gl| unsafe {
            use crate::os::gl_sys as g;
            let mut name = 0;
            (gl.glGenTextures)(1, &mut name);
            (gl.glBindTexture)(g::TEXTURE_2D, name);
            (gl.glTexImage2D)(g::TEXTURE_2D, 0, GL_RGBA8, width as i32, height as i32, 0, g::RGBA, g::UNSIGNED_BYTE, std::ptr::null());
            (gl.glBindTexture)(g::TEXTURE_2D, 0);
            name
        });
        match name {
            Some(name) if name != 0 => {
                adopt(self, &texture, name, width, height, TexturePixel::BGRAu8, true);
                (texture, name)
            }
            _ => (texture, 0),
        }
    }

    /// Create a GL rendering bridge: a context of the HAL's share group for an external renderer
    /// (current on the thread that calls `make_current`). Without GL the bridge has no context.
    pub fn create_gl_render_bridge(&mut self) -> GlRenderBridge {
        #[cfg(target_os = "linux")]
        {
            let inner = self.hal.gl.share.and_then(|share| {
                makepad_hal::gl_interop::GlContext::new(share)
            });
            GlRenderBridge { inner }
        }
        #[cfg(not(target_os = "linux"))]
        GlRenderBridge {}
    }

    /// Create a texture renderable via GL (by the bridge's context, same share group) and
    /// displayable by makepad. Returns (Texture handle, GL texture ID); 0 without GL.
    pub fn create_gl_render_bridge_texture(&mut self, _bridge: &GlRenderBridge, width: usize, height: usize) -> (Texture, u32) {
        self.create_gl_render_texture(width, height)
    }

    /// Restore makepad's GL state after external GL rendering. The external renderer has its
    /// own context of the share group and makepad renders on the HAL's: there is nothing to
    /// restore.
    pub fn restore_gl_context(&mut self) {}
}

impl Texture {
    /// Adopt an externally owned `GL_TEXTURE_2D` id (a texture of the HAL's share group, RGBA
    /// colour) for ordinary `texture_2d` / `sample` shaders. Makepad will **not** delete `tex_id`.
    pub fn adopt_gl_texture_2d(&self, cx: &mut Cx, tex_id: u32, width: usize, height: usize) -> Result<(), String> {
        check(cx, "adopt_gl_texture_2d", tex_id, width, height)?;
        adopt(cx, self, tex_id, width, height, TexturePixel::BGRAu8, false);
        Ok(())
    }

    /// Adopt an externally owned `GL_TEXTURE_2D` (RGBA colour) as
    /// [`TextureFormat::VideoExternal`] for `sample_video` shaders (e.g. GStreamer GLMemory / app
    /// hard-decode RGBA). Makepad will **not** delete `tex_id`; keep the producer buffer alive
    /// until the next adopt (plus one frame of present latency).
    pub fn adopt_gl_video_external(&self, cx: &mut Cx, tex_id: u32, width: usize, height: usize) -> Result<(), String> {
        check(cx, "adopt_gl_video_external", tex_id, width, height)?;
        adopt(cx, self, tex_id, width, height, TexturePixel::VideoExternal, false);
        Ok(())
    }

    /// Adopt an externally owned single-channel `GL_TEXTURE_2D` (R8) as a YUV plane slot
    /// ([`TextureFormat::VideoYuvPlane`]): planar Y, U or V. Makepad will **not** delete `tex_id`.
    pub fn adopt_gl_r8_plane(&self, cx: &mut Cx, tex_id: u32, width: usize, height: usize) -> Result<(), String> {
        check(cx, "adopt_gl_r8_plane", tex_id, width, height)?;
        adopt(cx, self, tex_id, width, height, TexturePixel::Ru8, false);
        Ok(())
    }

    /// Adopt an externally owned RG8 `GL_TEXTURE_2D` as a biplanar UV plane for NV12-style
    /// present ([`TextureFormat::VideoYuvPlane`]). Makepad will **not** delete `tex_id`.
    pub fn adopt_gl_rg8_plane(&self, cx: &mut Cx, tex_id: u32, width: usize, height: usize) -> Result<(), String> {
        check(cx, "adopt_gl_rg8_plane", tex_id, width, height)?;
        adopt(cx, self, tex_id, width, height, TexturePixel::RGu8, false);
        Ok(())
    }
}

fn check(cx: &Cx, api: &str, tex_id: u32, width: usize, height: usize) -> Result<(), String> {
    if cx.hal.gl.share.is_none() {
        return Err(format!("{api}: {NOT_AVAILABLE}"));
    }
    if tex_id == 0 {
        return Err(format!("{api}: tex_id must be non-zero"));
    }
    if width == 0 || height == 0 {
        return Err(format!("{api}: width/height must be non-zero"));
    }
    Ok(())
}

/// The pool texture shows GL texture `name` from now on: its format says how shaders sample it,
/// the HAL texture of its slot is the adopted name.
fn adopt(cx: &mut Cx, texture: &Texture, name: u32, width: usize, height: usize, pixel: TexturePixel, owned: bool) {
    let id = texture.texture_id();
    let (format, category, hal_format) = match pixel {
        TexturePixel::Ru8 => (TextureFormat::VideoYuvPlane, TextureCategory::Video, format::R8),
        TexturePixel::RGu8 => (TextureFormat::VideoYuvPlane, TextureCategory::Video, format::RG8),
        TexturePixel::VideoExternal => (TextureFormat::VideoExternal, TextureCategory::Video, format::RGBA8),
        _ => (TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width, height }, initial: false }, TextureCategory::Render, format::RGBA8),
    };
    let tex = &mut cx.textures[id];
    tex.format = format;
    tex.alloc = Some(TextureAlloc { width, height, pixel, category });
    let hal = cx.hal.frames.external_texture(id.0, id.generation());
    cx.hal.frames.w.texture_adopt_gl(hal, name, hal_format, width as u32, height as u32, owned);
    cx.redraw_all();
}
