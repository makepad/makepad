//! Public Linux GPU video present helpers for app / [`crate::MediaPlaybackSession`]
//! backends (FFmpeg VAAPI, custom GStreamer pipelines, etc.).
//!
//! # Paths
//!
//! | Frame type | Present | Video widget metadata |
//! |------------|---------|------------------------|
//! | [`LinuxDmabufNv12Frame`] | [`present_dmabuf_nv12`] → `TEXTURE_EXTERNAL_OES` Y/UV | `VideoYuvMetadata { enabled, biplanar, external: true }` |
//! | [`LinuxGlMemoryRgbaFrame`] | [`present_gl_memory_rgba`] → main video texture | `rgba_gl_2d` / `sample_video` |
//!
//! These APIs mirror the platform GStreamer player’s DMA-Buf / GLMemory present
//! paths, but do **not** require `GStreamerVideoPlayer`. Call them from the UI /
//! video-poll thread with Makepad’s EGL context current (`OpenglCx::make_current`
//! or [`Cx::with_gl`](crate::Cx::with_gl)).

use std::{
    any::Any,
    ffi::c_void,
    os::fd::RawFd,
    sync::Arc,
};

use super::{
    egl_sys,
    gl_sys::{self, LibGl},
    opengl_cx::OpenglCx,
    va_dmabuf_modifier,
};
use crate::{
    texture::{
        CxTexturePool, TextureAlloc, TextureCategory, TextureFormat, TextureId, TexturePixel,
    },
    video_decode::yuv::YuvColorMatrix,
};

/// DRM fourcc from four ASCII bytes (little-endian), e.g. `drm_fourcc(*b"R8  ")`.
pub const fn drm_fourcc(bytes: [u8; 4]) -> u32 {
    u32::from_le_bytes(bytes)
}

/// Common plane fourccs for NV12 → EGLImage import.
pub const DRM_FORMAT_R8: u32 = drm_fourcc(*b"R8  ");
pub const DRM_FORMAT_RG88: u32 = drm_fourcc(*b"RG88");
pub const DRM_FORMAT_NV12: u32 = drm_fourcc(*b"NV12");
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// One DMA-Buf plane for NV12 Y or UV. `fd` is borrowed; keep it alive via the
/// parent frame’s `keep_alive` until the next successful present (plus one frame).
#[derive(Clone, Copy, Debug)]
pub struct LinuxDmabufPlane {
    pub fd: RawFd,
    pub offset: u32,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    /// Typically [`DRM_FORMAT_R8`] (Y) or [`DRM_FORMAT_RG88`] (UV).
    pub fourcc: u32,
}

/// Zero-copy NV12 DMA-Buf frame for Linux `Video` present
/// (`TEXTURE_EXTERNAL_OES` Y + UV planes).
///
/// Handed from a [`crate::MediaPlaybackSession`] via
/// [`crate::MediaPlaybackSession::take_linux_dmabuf_nv12_frame`].
pub struct LinuxDmabufNv12Frame {
    pub y: LinuxDmabufPlane,
    pub uv: LinuxDmabufPlane,
    pub width: u32,
    pub height: u32,
    /// DRM format modifier. `0` (LINEAR) may be wrong on NVIDIA; prefer
    /// [`LinuxDmabufNv12Frame::probe_modifier_if_needed`] before present.
    pub modifier: u64,
    pub matrix: YuvColorMatrix,
    pub full_range: bool,
    pub keep_alive: Arc<dyn Any + Send + Sync>,
}

unsafe impl Send for LinuxDmabufNv12Frame {}
unsafe impl Sync for LinuxDmabufNv12Frame {}

impl LinuxDmabufNv12Frame {
    /// If `modifier` is LINEAR/`0`, probe VA for the driver’s real NV12 modifier.
    pub fn probe_modifier_if_needed(&mut self) {
        if self.modifier == DRM_FORMAT_MOD_LINEAR {
            if let Some(probed) = va_dmabuf_modifier::probe_nv12_modifier(self.width, self.height) {
                self.modifier = probed;
            }
        }
    }

    /// Build default Y/UV plane layouts for `width`×`height` with `n_fds` DMA-Bufs.
    ///
    /// When `n_fds >= 2`, UV uses fd index 1 at offset 0; otherwise UV is packed
    /// after Y with 64-byte row alignment (Intel VA style guess).
    pub fn default_nv12_planes(
        y_fd: RawFd,
        uv_fd: Option<RawFd>,
        width: u32,
        height: u32,
        y_pitch: Option<u32>,
        uv_pitch: Option<u32>,
        y_offset: u32,
        uv_offset: Option<u32>,
    ) -> (LinuxDmabufPlane, LinuxDmabufPlane) {
        let pitch_y = y_pitch.unwrap_or_else(|| ((width as usize + 63) & !63) as u32);
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        let y = LinuxDmabufPlane {
            fd: y_fd,
            offset: y_offset,
            pitch: pitch_y,
            width,
            height,
            fourcc: DRM_FORMAT_R8,
        };
        let uv = LinuxDmabufPlane {
            fd: uv_fd.unwrap_or(y_fd),
            offset: uv_offset.unwrap_or(if uv_fd.is_some() {
                0
            } else {
                pitch_y.saturating_mul(height)
            }),
            pitch: uv_pitch.unwrap_or(pitch_y),
            width: cw,
            height: ch,
            fourcc: DRM_FORMAT_RG88,
        };
        (y, uv)
    }
}

/// GL texture target for a GStreamer GLMemory (or app-owned) RGBA frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxGlTextureTarget {
    /// `GL_TEXTURE_2D` — Video widget uses `rgba_gl_2d` / `sampler2D`.
    Texture2D,
    /// `GL_TEXTURE_EXTERNAL_OES` — Video widget uses `sample_video`.
    ExternalOes,
}

/// Zero-copy RGBA GL texture frame (typically GStreamer `memory:GLMemory` after
/// `GST_MAP_GL` on the Gst GL thread).
pub struct LinuxGlMemoryRgbaFrame {
    pub tex_id: u32,
    pub target: LinuxGlTextureTarget,
    pub width: u32,
    pub height: u32,
    /// Must keep the producer buffer / GstSample alive while Makepad samples.
    pub keep_alive: Arc<dyn Any + Send + Sync>,
}

unsafe impl Send for LinuxGlMemoryRgbaFrame {}
unsafe impl Sync for LinuxGlMemoryRgbaFrame {}

/// Retains EGLImages + producer keep-alive across presents so the on-screen
/// texture cannot be unbound while the previous frame is still drawn.
#[derive(Default)]
pub struct LinuxDmabufPresentCache {
    egl_images: Vec<*mut c_void>,
    egl_display: egl_sys::EGLDisplay,
    egl_destroy: Option<unsafe extern "C" fn(egl_sys::EGLDisplay, egl_sys::EGLImageKHR) -> egl_sys::EGLBoolean>,
    /// Current + previous producer frames (dual retain, same idea as GStreamer path).
    keep_alive_current: Option<Arc<dyn Any + Send + Sync>>,
    keep_alive_previous: Option<Arc<dyn Any + Send + Sync>>,
}

impl Drop for LinuxDmabufPresentCache {
    fn drop(&mut self) {
        self.release_egl_images();
    }
}

impl LinuxDmabufPresentCache {
    pub fn release_egl_images(&mut self) {
        if let Some(destroy) = self.egl_destroy {
            if !self.egl_display.is_null() {
                for image in self.egl_images.drain(..) {
                    if !image.is_null() {
                        unsafe {
                            destroy(self.egl_display, image);
                        }
                    }
                }
            }
        } else {
            self.egl_images.clear();
        }
        self.egl_display = std::ptr::null_mut();
        self.egl_destroy = None;
    }

    fn retain_keep_alive(&mut self, keep: Arc<dyn Any + Send + Sync>) {
        self.keep_alive_previous = self.keep_alive_current.take();
        self.keep_alive_current = Some(keep);
    }
}

/// Keep-alive for GLMemory present (producer buffer must outlive sampling).
#[derive(Default)]
pub struct LinuxGlMemoryPresentCache {
    keep_alive_current: Option<Arc<dyn Any + Send + Sync>>,
    keep_alive_previous: Option<Arc<dyn Any + Send + Sync>>,
}

impl LinuxGlMemoryPresentCache {
    fn retain(&mut self, keep: Arc<dyn Any + Send + Sync>) {
        self.keep_alive_previous = self.keep_alive_current.take();
        self.keep_alive_current = Some(keep);
    }
}

fn should_pass_modifier(modifier: u64) -> bool {
    modifier != DRM_FORMAT_MOD_LINEAR
}

unsafe fn egl_image_from_dmabuf(
    opengl_cx: &OpenglCx,
    create_image: unsafe extern "C" fn(
        egl_sys::EGLDisplay,
        egl_sys::EGLContext,
        egl_sys::EGLenum,
        egl_sys::EGLClientBuffer,
        *const egl_sys::EGLint,
    ) -> egl_sys::EGLImageKHR,
    plane: &LinuxDmabufPlane,
    modifier: u64,
) -> *mut c_void {
    let mut attribs: Vec<i32> = vec![
        egl_sys::EGL_LINUX_DRM_FOURCC_EXT as i32,
        plane.fourcc as i32,
        egl_sys::EGL_WIDTH as i32,
        plane.width as i32,
        egl_sys::EGL_HEIGHT as i32,
        plane.height as i32,
        egl_sys::EGL_DMA_BUF_PLANE0_FD_EXT as i32,
        plane.fd as i32,
        egl_sys::EGL_DMA_BUF_PLANE0_OFFSET_EXT as i32,
        plane.offset as i32,
        egl_sys::EGL_DMA_BUF_PLANE0_PITCH_EXT as i32,
        plane.pitch as i32,
    ];
    if should_pass_modifier(modifier) {
        attribs.push(egl_sys::EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT as i32);
        attribs.push(modifier as i32);
        attribs.push(egl_sys::EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT as i32);
        attribs.push((modifier >> 32) as i32);
    }
    attribs.push(egl_sys::EGL_NONE as i32);
    create_image(
        opengl_cx.egl_display,
        std::ptr::null_mut(),
        egl_sys::EGL_LINUX_DMA_BUF_EXT,
        std::ptr::null_mut(),
        attribs.as_ptr() as _,
    )
}

unsafe fn bind_egl_image_to_oes(
    gl: &LibGl,
    textures: &mut CxTexturePool,
    texture_id: TextureId,
    egl_image: *mut c_void,
    target_tex: unsafe extern "C" fn(gl_sys::GLenum, egl_sys::EGLImageKHR),
    _width: usize,
    _height: usize,
) -> bool {
    let cxtexture = &mut textures[texture_id];
    let gl_texture = cxtexture.os.gl_texture.get_or_insert_with(|| {
        let mut t = std::mem::MaybeUninit::uninit();
        (gl.glGenTextures)(1, t.as_mut_ptr());
        t.assume_init()
    });
    cxtexture.os.gl_texture_owned = true;
    let target = gl_sys::TEXTURE_EXTERNAL_OES;
    while (gl.glGetError)() != 0 {}
    (gl.glBindTexture)(target, *gl_texture);
    (gl.glTexParameteri)(target, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_EDGE as i32);
    (gl.glTexParameteri)(target, gl_sys::TEXTURE_WRAP_T, gl_sys::CLAMP_TO_EDGE as i32);
    (gl.glTexParameteri)(target, gl_sys::TEXTURE_MIN_FILTER, gl_sys::LINEAR as i32);
    (gl.glTexParameteri)(target, gl_sys::TEXTURE_MAG_FILTER, gl_sys::LINEAR as i32);
    target_tex(target, egl_image);
    let err = (gl.glGetError)();
    (gl.glBindTexture)(target, 0);
    if err != 0 {
        return false;
    }
    cxtexture.format = TextureFormat::VideoExternal;
    cxtexture.alloc = Some(TextureAlloc {
        width: 0,
        height: 0,
        pixel: TexturePixel::VideoExternal,
        category: TextureCategory::Video,
    });
    true
}

/// Import NV12 DMA-Buf planes into Makepad Y/UV `VideoExternal` (OES) textures.
///
/// `y_oes_id` / `uv_oes_id` must come from textures allocated as
/// [`TextureFormat::VideoExternal`] and published via
/// [`crate::event::VideoYuvTexturesReady::with_external`].
pub fn present_dmabuf_nv12(
    opengl_cx: &OpenglCx,
    gl: &LibGl,
    textures: &mut CxTexturePool,
    y_oes_id: TextureId,
    uv_oes_id: TextureId,
    frame: &LinuxDmabufNv12Frame,
    cache: &mut LinuxDmabufPresentCache,
) -> Result<(), String> {
    if frame.width == 0 || frame.height == 0 {
        return Err("present_dmabuf_nv12: width/height must be non-zero".into());
    }
    if frame.y.fd < 0 || frame.uv.fd < 0 {
        return Err("present_dmabuf_nv12: invalid DMA-Buf fd".into());
    }
    let Some(create_image) = opengl_cx.libegl.eglCreateImageKHR else {
        return Err("present_dmabuf_nv12: eglCreateImageKHR unavailable".into());
    };
    let Some(target_tex) = opengl_cx.libegl.glEGLImageTargetTexture2DOES else {
        return Err("present_dmabuf_nv12: glEGLImageTargetTexture2DOES unavailable".into());
    };

    opengl_cx.make_current();

    let mut frame = LinuxDmabufNv12Frame {
        y: frame.y,
        uv: frame.uv,
        width: frame.width,
        height: frame.height,
        modifier: frame.modifier,
        matrix: frame.matrix,
        full_range: frame.full_range,
        keep_alive: Arc::clone(&frame.keep_alive),
    };
    frame.probe_modifier_if_needed();

    unsafe {
        let y_image = egl_image_from_dmabuf(opengl_cx, create_image, &frame.y, frame.modifier);
        let uv_image = egl_image_from_dmabuf(opengl_cx, create_image, &frame.uv, frame.modifier);
        if y_image.is_null() || uv_image.is_null() {
            let err = opengl_cx
                .libegl
                .eglGetError
                .map(|f| f())
                .unwrap_or(0);
            if !y_image.is_null() {
                if let Some(destroy) = opengl_cx.libegl.eglDestroyImageKHR {
                    destroy(opengl_cx.egl_display, y_image);
                }
            }
            if !uv_image.is_null() {
                if let Some(destroy) = opengl_cx.libegl.eglDestroyImageKHR {
                    destroy(opengl_cx.egl_display, uv_image);
                }
            }
            return Err(format!(
                "present_dmabuf_nv12: eglCreateImageKHR failed (egl=0x{err:x})"
            ));
        }

        let y_ok = bind_egl_image_to_oes(
            gl,
            textures,
            y_oes_id,
            y_image,
            target_tex,
            frame.width as usize,
            frame.height as usize,
        );
        let uv_ok = y_ok
            && bind_egl_image_to_oes(
                gl,
                textures,
                uv_oes_id,
                uv_image,
                target_tex,
                frame.width.div_ceil(2) as usize,
                frame.height.div_ceil(2) as usize,
            );
        if !uv_ok {
            if let Some(destroy) = opengl_cx.libegl.eglDestroyImageKHR {
                destroy(opengl_cx.egl_display, y_image);
                destroy(opengl_cx.egl_display, uv_image);
            }
            return Err("present_dmabuf_nv12: bind EGLImage → EXTERNAL_OES failed".into());
        }

        cache.release_egl_images();
        cache.egl_images = vec![y_image, uv_image];
        cache.egl_display = opengl_cx.egl_display;
        cache.egl_destroy = opengl_cx.libegl.eglDestroyImageKHR;
        cache.retain_keep_alive(frame.keep_alive);
    }
    Ok(())
}

/// Adopt a share-group GL texture into Makepad’s main video texture slot.
///
/// For `Texture2D`, set `VideoTextureUpdatedEvent.rgba_gl_2d = true`.
/// For `ExternalOes`, leave `rgba_gl_2d = false` so the widget uses `sample_video`.
pub fn present_gl_memory_rgba(
    gl: &LibGl,
    textures: &mut CxTexturePool,
    texture_id: TextureId,
    frame: &LinuxGlMemoryRgbaFrame,
    cache: &mut LinuxGlMemoryPresentCache,
) -> Result<(), String> {
    if frame.tex_id == 0 {
        return Err("present_gl_memory_rgba: tex_id must be non-zero".into());
    }
    if frame.width == 0 || frame.height == 0 {
        return Err("present_gl_memory_rgba: width/height must be non-zero".into());
    }

    let bind_target = match frame.target {
        LinuxGlTextureTarget::Texture2D => gl_sys::TEXTURE_2D,
        LinuxGlTextureTarget::ExternalOes => gl_sys::TEXTURE_EXTERNAL_OES,
    };

    unsafe {
        let cxtexture = &mut textures[texture_id];
        if let Some(old) = cxtexture.os.gl_texture {
            if old != frame.tex_id && cxtexture.os.gl_texture_owned {
                (gl.glDeleteTextures)(1, &old);
            }
        }
        (gl.glBindTexture)(bind_target, frame.tex_id);
        (gl.glTexParameteri)(
            bind_target,
            gl_sys::TEXTURE_MIN_FILTER,
            gl_sys::LINEAR as i32,
        );
        (gl.glTexParameteri)(
            bind_target,
            gl_sys::TEXTURE_MAG_FILTER,
            gl_sys::LINEAR as i32,
        );
        (gl.glTexParameteri)(
            bind_target,
            gl_sys::TEXTURE_WRAP_S,
            gl_sys::CLAMP_TO_EDGE as i32,
        );
        (gl.glTexParameteri)(
            bind_target,
            gl_sys::TEXTURE_WRAP_T,
            gl_sys::CLAMP_TO_EDGE as i32,
        );
        (gl.glBindTexture)(bind_target, 0);

        cxtexture.format = TextureFormat::VideoExternal;
        cxtexture.os.gl_texture = Some(frame.tex_id);
        cxtexture.os.gl_texture_owned = false;
        cxtexture.alloc = Some(TextureAlloc {
            width: 0,
            height: 0,
            pixel: TexturePixel::VideoExternal,
            category: TextureCategory::Video,
        });
    }

    cache.retain(Arc::clone(&frame.keep_alive));
    Ok(())
}
