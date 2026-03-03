//! Unified video player for Linux that wraps GStreamer native player
//! and software rav1d fallback.

use {
    super::gl_sys::LibGl,
    super::linux_video_playback::GStreamerVideoPlayer,
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureId},
        video_decode::software_av1::SoftwareAv1Player,
        video_decode::yuv::YuvPlaneData,
    },
    std::ffi::c_void,
};

pub enum LinuxVideoPlayer {
    GStreamer(GStreamerVideoPlayer),
    Software {
        player: SoftwareAv1Player,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        yuv_matrix: f32,
    },
}

impl LinuxVideoPlayer {
    pub fn video_id(&self) -> LiveId {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.video_id,
            LinuxVideoPlayer::Software { player: p, .. } => p.video_id,
        }
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.check_prepared(),
            LinuxVideoPlayer::Software { player: p, .. } => p.check_prepared(),
        }
    }

    pub fn poll_frame(&mut self, gl: &LibGl, textures: &mut CxTexturePool) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.poll_frame(gl, textures),
            LinuxVideoPlayer::Software { player: p, tex_y_id, tex_u_id, tex_v_id, yuv_matrix } => {
                if !p.poll_frame() {
                    return false;
                }
                if let Some(planes) = p.take_yuv_frame() {
                    *yuv_matrix = planes.matrix.as_f32();
                    upload_yuv_to_gl(gl, textures, *tex_y_id, *tex_u_id, *tex_v_id, &planes);
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn check_eos(&mut self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.check_eos(),
            LinuxVideoPlayer::Software { player: p, .. } => p.check_eos(),
        }
    }

    pub fn is_active(&self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.is_active(),
            LinuxVideoPlayer::Software { player: p, .. } => p.is_active(),
        }
    }

    pub fn play(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.play(),
            LinuxVideoPlayer::Software { player: p, .. } => p.play(),
        }
    }

    pub fn pause(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.pause(),
            LinuxVideoPlayer::Software { player: p, .. } => p.pause(),
        }
    }

    pub fn resume(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.resume(),
            LinuxVideoPlayer::Software { player: p, .. } => p.resume(),
        }
    }

    pub fn mute(&self) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.mute(),
            LinuxVideoPlayer::Software { .. } => {}
        }
    }

    pub fn unmute(&self) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.unmute(),
            LinuxVideoPlayer::Software { .. } => {}
        }
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.seek_to(position_ms),
            LinuxVideoPlayer::Software { player: p, .. } => p.seek_to(position_ms),
        }
    }

    pub fn set_volume(&self, volume: f64) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.set_volume(volume),
            LinuxVideoPlayer::Software { player: p, .. } => p.set_volume(volume),
        }
    }

    pub fn set_playback_rate(&self, rate: f64) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.set_playback_rate(rate),
            LinuxVideoPlayer::Software { .. } => {}
        }
    }

    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.seekable_ranges(),
            LinuxVideoPlayer::Software { player: p, .. } => p.seekable_ranges(),
        }
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.buffered_ranges(),
            LinuxVideoPlayer::Software { player: p, .. } => p.buffered_ranges(),
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.current_position_ms(),
            LinuxVideoPlayer::Software { player: p, .. } => p.current_position_ms(),
        }
    }

    pub fn cleanup(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer(p) => p.cleanup(),
            LinuxVideoPlayer::Software { player: p, .. } => p.cleanup(),
        }
    }

    pub fn is_software_mode(&self) -> bool {
        matches!(self, LinuxVideoPlayer::Software { .. })
    }

    pub fn yuv_matrix(&self) -> f32 {
        match self {
            LinuxVideoPlayer::GStreamer(_) => 0.0,
            LinuxVideoPlayer::Software { yuv_matrix, .. } => *yuv_matrix,
        }
    }
}

pub(crate) fn upload_yuv_to_gl(
    gl: &LibGl,
    textures: &mut CxTexturePool,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    planes: &YuvPlaneData,
) {
    let (cw, ch) = planes.layout.chroma_size(planes.width, planes.height);
    upload_r8_plane_to_gl(gl, textures, tex_y_id, &planes.y, planes.width, planes.height);
    upload_r8_plane_to_gl(gl, textures, tex_u_id, &planes.u, cw, ch);
    upload_r8_plane_to_gl(gl, textures, tex_v_id, &planes.v, cw, ch);
}

pub(crate) fn upload_r8_plane_to_gl(
    gl: &LibGl,
    textures: &mut CxTexturePool,
    texture_id: TextureId,
    data: &[u8],
    width: u32,
    height: u32,
) {
    use super::gl_sys;
    use crate::texture::{TextureAlloc, TextureCategory, TexturePixel};

    let w = width as usize;
    let h = height as usize;
    if data.len() < w * h {
        return;
    }

    unsafe {
        let cxtexture = &mut textures[texture_id];
        let needs_alloc = if cxtexture.os.gl_texture.is_none() {
            let mut gl_texture = std::mem::MaybeUninit::uninit();
            (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
            let gl_texture = gl_texture.assume_init();
            cxtexture.os.gl_texture = Some(gl_texture);

            (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_WRAP_S,
                gl_sys::CLAMP_TO_EDGE as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_WRAP_T,
                gl_sys::CLAMP_TO_EDGE as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_MIN_FILTER,
                gl_sys::LINEAR as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_MAG_FILTER,
                gl_sys::LINEAR as i32,
            );
            true
        } else {
            cxtexture
                .alloc
                .as_ref()
                .map_or(true, |a| a.width != w || a.height != h)
        };

        let gl_texture = cxtexture.os.gl_texture.unwrap();
        (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
        (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 1);
        (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
        (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
        (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);

        if needs_alloc {
            (gl.glTexImage2D)(
                gl_sys::TEXTURE_2D,
                0,
                gl_sys::R8 as i32,
                w as i32,
                h as i32,
                0,
                gl_sys::RED,
                gl_sys::UNSIGNED_BYTE,
                data.as_ptr() as *const c_void,
            );
        } else {
            (gl.glTexSubImage2D)(
                gl_sys::TEXTURE_2D,
                0,
                0,
                0,
                w as i32,
                h as i32,
                gl_sys::RED,
                gl_sys::UNSIGNED_BYTE,
                data.as_ptr() as *const c_void,
            );
        }

        (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);

        cxtexture.alloc = Some(TextureAlloc {
            width: w,
            height: h,
            pixel: TexturePixel::Ru8,
            category: TextureCategory::Video,
        });
    }
}
