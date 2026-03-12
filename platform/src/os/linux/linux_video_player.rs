//! Unified video player for Linux that wraps GStreamer native player,
//! software rav1d fallback, and V4L2 camera capture.

use {
    super::gl_sys::LibGl,
    super::gl_video_upload::upload_yuv_to_gl,
    super::linux_video_playback::{GStreamerVideoPlayer, YuvTextureIds},
    super::v4l2_camera_player::V4l2CameraPlayer,
    crate::{
        makepad_live_id::LiveId,
        texture::{CxTexturePool, Texture},
        video_decode::software_video::SoftwareVideoPlayer,
    },
};

#[derive(Clone)]
pub struct YuvTextureSet {
    pub tex_y: Texture,
    pub tex_u: Texture,
    pub tex_v: Texture,
    pub ids: YuvTextureIds,
}

impl YuvTextureSet {
    pub fn new(tex_y: Texture, tex_u: Texture, tex_v: Texture) -> Self {
        Self {
            ids: YuvTextureIds {
                tex_y_id: tex_y.texture_id(),
                tex_u_id: tex_u.texture_id(),
                tex_v_id: tex_v.texture_id(),
            },
            tex_y,
            tex_u,
            tex_v,
        }
    }
}

pub enum LinuxVideoPlayer {
    GStreamer {
        player: GStreamerVideoPlayer,
        yuv: Option<YuvTextureSet>,
    },
    Software {
        player: SoftwareVideoPlayer,
        yuv: YuvTextureSet,
        yuv_matrix: f32,
    },
    Camera(V4l2CameraPlayer),
}

impl LinuxVideoPlayer {
    pub fn video_id(&self) -> LiveId {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.video_id,
            LinuxVideoPlayer::Software { player: p, .. } => p.video_id,
            LinuxVideoPlayer::Camera(p) => p.video_id,
        }
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.check_prepared(),
            LinuxVideoPlayer::Software { player: p, .. } => p.check_prepared(),
            LinuxVideoPlayer::Camera(p) => p.check_prepared(),
        }
    }

    pub fn poll_frame(&mut self, gl: &LibGl, textures: &mut CxTexturePool) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.poll_frame(gl, textures),
            LinuxVideoPlayer::Software {
                player: p,
                yuv,
                yuv_matrix,
            } => {
                if !p.poll_frame() {
                    return false;
                }
                if let Some(planes) = p.take_yuv_frame() {
                    *yuv_matrix = planes.matrix.as_f32();
                    upload_yuv_to_gl(
                        gl,
                        textures,
                        yuv.ids.tex_y_id,
                        yuv.ids.tex_u_id,
                        yuv.ids.tex_v_id,
                        &planes,
                    );
                    true
                } else {
                    false
                }
            }
            LinuxVideoPlayer::Camera(p) => p.poll_frame(gl, textures),
        }
    }

    pub fn check_eos(&mut self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.check_eos(),
            LinuxVideoPlayer::Software { player: p, .. } => p.check_eos(),
            LinuxVideoPlayer::Camera(_) => false, // camera never ends
        }
    }

    pub fn is_active(&self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.is_active(),
            LinuxVideoPlayer::Software { player: p, .. } => p.is_active(),
            LinuxVideoPlayer::Camera(p) => p.is_active(),
        }
    }

    pub fn play(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.play(),
            LinuxVideoPlayer::Software { player: p, .. } => p.play(),
            LinuxVideoPlayer::Camera(_) => {} // camera is always playing
        }
    }

    pub fn pause(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.pause(),
            LinuxVideoPlayer::Software { player: p, .. } => p.pause(),
            LinuxVideoPlayer::Camera(_) => {} // no-op for camera
        }
    }

    pub fn resume(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.resume(),
            LinuxVideoPlayer::Software { player: p, .. } => p.resume(),
            LinuxVideoPlayer::Camera(_) => {} // no-op for camera
        }
    }

    pub fn mute(&self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.mute(),
            LinuxVideoPlayer::Software { .. } => {}
            LinuxVideoPlayer::Camera(_) => {}
        }
    }

    pub fn unmute(&self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.unmute(),
            LinuxVideoPlayer::Software { .. } => {}
            LinuxVideoPlayer::Camera(_) => {}
        }
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.seek_to(position_ms),
            LinuxVideoPlayer::Software { player: p, .. } => p.seek_to(position_ms),
            LinuxVideoPlayer::Camera(_) => {} // camera is not seekable
        }
    }

    pub fn set_volume(&self, volume: f64) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.set_volume(volume),
            LinuxVideoPlayer::Software { player: p, .. } => p.set_volume(volume),
            LinuxVideoPlayer::Camera(_) => {}
        }
    }

    pub fn set_playback_rate(&self, rate: f64) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.set_playback_rate(rate),
            LinuxVideoPlayer::Software { .. } => {}
            LinuxVideoPlayer::Camera(_) => {}
        }
    }

    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.seekable_ranges(),
            LinuxVideoPlayer::Software { player: p, .. } => p.seekable_ranges(),
            LinuxVideoPlayer::Camera(_) => vec![],
        }
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.buffered_ranges(),
            LinuxVideoPlayer::Software { player: p, .. } => p.buffered_ranges(),
            LinuxVideoPlayer::Camera(_) => vec![],
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.current_position_ms(),
            LinuxVideoPlayer::Software { player: p, .. } => p.current_position_ms(),
            LinuxVideoPlayer::Camera(_) => 0,
        }
    }

    pub fn cleanup(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.cleanup(),
            LinuxVideoPlayer::Software { player: p, .. } => p.cleanup(),
            LinuxVideoPlayer::Camera(p) => p.cleanup(),
        }
    }

    pub fn is_software_mode(&self) -> bool {
        matches!(self, LinuxVideoPlayer::Software { .. })
    }

    pub fn is_camera_mode(&self) -> bool {
        matches!(self, LinuxVideoPlayer::Camera(_))
    }

    pub fn is_yuv_mode(&self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player, .. } => player.is_yuv_mode(),
            LinuxVideoPlayer::Software { .. } => true,
            LinuxVideoPlayer::Camera(_) => true,
        }
    }

    pub fn yuv_texture_set(&self) -> Option<&YuvTextureSet> {
        match self {
            LinuxVideoPlayer::GStreamer { yuv: Some(yuv), .. } => Some(yuv),
            LinuxVideoPlayer::Software { yuv, .. } => Some(yuv),
            _ => None,
        }
    }

    pub fn yuv_matrix(&self) -> f32 {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.yuv_matrix(),
            LinuxVideoPlayer::Software { yuv_matrix, .. } => *yuv_matrix,
            LinuxVideoPlayer::Camera(_) => 1.0, // BT.601
        }
    }
}
