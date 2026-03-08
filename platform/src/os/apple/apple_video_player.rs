use {
    super::apple_sys::*,
    super::apple_video_playback::AppleVideoPlayer,
    super::apple_yuv_metal::AppleYuvMetal,
    crate::{
        event::video_playback::VideoSource,
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureId},
        video_decode::software_video::SoftwareVideoPlayer,
        video_decode::yuv::{YuvColorMatrix, YuvPlaneData},
    },
};

pub struct AppleUnifiedVideoPlayer {
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    yuv_matrix: f32,
    source: VideoSource,
    autoplay: bool,
    is_looping: bool,
    mode: ApplePlayerMode,
    null_frame_count: u32,
    yuv_metal: AppleYuvMetal,
}

enum ApplePlayerMode {
    Native(AppleVideoPlayer),
    Software(SoftwareVideoPlayer),
}

impl AppleUnifiedVideoPlayer {
    pub fn new(
        metal_device: ObjcId,
        video_id: LiveId,
        texture_id: TextureId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        let yuv_metal = AppleYuvMetal::new(metal_device, "apple unified player");

        let force_software = std::env::var_os("MAKEPAD_FORCE_SOFTWARE_VIDEO").is_some();
        let mode = if force_software {
            crate::log!("VIDEO: MAKEPAD_FORCE_SOFTWARE_VIDEO set, using software video decoder");
            ApplePlayerMode::Software(SoftwareVideoPlayer::new(
                video_id,
                texture_id,
                source.clone(),
                autoplay,
                is_looping,
            ))
        } else {
            ApplePlayerMode::Native(AppleVideoPlayer::new(
                metal_device,
                video_id,
                texture_id,
                source.clone(),
                autoplay,
                is_looping,
            ))
        };

        Self {
            video_id,
            texture_id,
            tex_y_id,
            tex_u_id,
            tex_v_id,
            yuv_matrix: YuvColorMatrix::BT709.as_f32(),
            source,
            autoplay,
            is_looping,
            mode,
            null_frame_count: 0,
            yuv_metal,
        }
    }

    fn switch_to_software(&mut self, reason: &str) {
        crate::log!(
            "VIDEO: Apple native playback failed, falling back to software video decoder: {}",
            reason
        );
        self.mode = ApplePlayerMode::Software(SoftwareVideoPlayer::new(
            self.video_id,
            self.texture_id,
            self.source.clone(),
            self.autoplay,
            self.is_looping,
        ));
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => match player.check_prepared() {
                Some(Err(err)) => {
                    self.switch_to_software(&err);
                    if let ApplePlayerMode::Software(software) = &mut self.mode {
                        software.check_prepared()
                    } else {
                        Some(Err(err))
                    }
                }
                other => other,
            },
            ApplePlayerMode::Software(player) => player.check_prepared(),
        }
    }

    pub fn poll_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => {
                let got_frame = player.poll_frame(textures);
                if got_frame {
                    self.null_frame_count = 0;
                    return true;
                }
                self.null_frame_count += 1;
                if self.null_frame_count >= 60 {
                    self.switch_to_software("native player produced no frames after 60 polls");
                    self.null_frame_count = 0;
                } else {
                    return false;
                }
                self.poll_software_frame(textures)
            }
            ApplePlayerMode::Software(_) => self.poll_software_frame(textures),
        }
    }

    fn poll_software_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        let yuv_planes = {
            let player = match &mut self.mode {
                ApplePlayerMode::Software(p) => p,
                _ => return false,
            };

            if !player.poll_frame() {
                return false;
            }

            player.take_yuv_frame()
        };

        if let Some(planes) = yuv_planes {
            self.yuv_matrix = planes.matrix.as_f32();
            self.upload_yuv_to_metal(textures, &planes);
            return true;
        }

        false
    }

    fn upload_yuv_to_metal(&mut self, textures: &mut CxTexturePool, planes: &YuvPlaneData) {
        let (cw, ch) = planes.layout.chroma_size(planes.width, planes.height);
        self.yuv_metal.upload_r8_plane(
            textures,
            self.tex_y_id,
            &planes.y,
            planes.width,
            planes.height,
        );
        self.yuv_metal
            .upload_r8_plane(textures, self.tex_u_id, &planes.u, cw, ch);
        self.yuv_metal
            .upload_r8_plane(textures, self.tex_v_id, &planes.v, cw, ch);
    }

    pub fn is_software_mode(&self) -> bool {
        matches!(self.mode, ApplePlayerMode::Software(_))
    }

    pub fn yuv_biplanar(&self) -> f32 {
        if self.yuv_metal.has_biplanar_wrap() {
            1.0
        } else {
            0.0
        }
    }

    pub fn yuv_matrix(&self) -> f32 {
        self.yuv_matrix
    }

    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.seekable_ranges(),
            ApplePlayerMode::Software(player) => player.seekable_ranges(),
        }
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.buffered_ranges(),
            ApplePlayerMode::Software(player) => player.buffered_ranges(),
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.current_position_ms(),
            ApplePlayerMode::Software(player) => player.current_position_ms(),
        }
    }

    pub fn play(&mut self) {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.play(),
            ApplePlayerMode::Software(player) => player.play(),
        }
    }

    pub fn pause(&mut self) {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.pause(),
            ApplePlayerMode::Software(player) => player.pause(),
        }
    }

    pub fn resume(&mut self) {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.resume(),
            ApplePlayerMode::Software(player) => player.resume(),
        }
    }

    pub fn mute(&self) {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.mute(),
            ApplePlayerMode::Software(player) => player.mute(),
        }
    }

    pub fn unmute(&self) {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.unmute(),
            ApplePlayerMode::Software(player) => player.unmute(),
        }
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.seek_to(position_ms),
            ApplePlayerMode::Software(player) => player.seek_to(position_ms),
        }
    }

    pub fn set_volume(&self, volume: f64) {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.set_volume(volume),
            ApplePlayerMode::Software(player) => player.set_volume(volume),
        }
    }

    pub fn set_playback_rate(&self, rate: f64) {
        match &self.mode {
            ApplePlayerMode::Native(player) => player.set_playback_rate(rate),
            ApplePlayerMode::Software(player) => player.set_playback_rate(rate),
        }
    }

    pub fn cleanup(&mut self) {
        self.yuv_metal.cleanup();
        match &mut self.mode {
            ApplePlayerMode::Native(player) => player.cleanup(),
            ApplePlayerMode::Software(player) => player.cleanup(),
        }
    }
}

impl Drop for AppleUnifiedVideoPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
