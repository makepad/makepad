use {
    super::apple_sys::*,
    super::apple_video_playback::AppleVideoPlayer,
    crate::{
        event::video_playback::VideoSource,
        makepad_live_id::LiveId,
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
        video_decode::software_av1::SoftwareAv1Player,
    },
    std::{ffi::c_void, ptr::NonNull},
};

pub struct AppleUnifiedVideoPlayer {
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    metal_device: ObjcId,
    source: VideoSource,
    autoplay: bool,
    is_looping: bool,
    mode: ApplePlayerMode,
    bgra_buf: Vec<u8>,
}

enum ApplePlayerMode {
    Native(AppleVideoPlayer),
    Software(SoftwareAv1Player),
}

impl AppleUnifiedVideoPlayer {
    pub fn new(
        metal_device: ObjcId,
        video_id: LiveId,
        texture_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        let force_software = std::env::var_os("MAKEPAD_FORCE_SOFTWARE_AV1").is_some();
        let mode = if force_software {
            crate::log!("VIDEO: MAKEPAD_FORCE_SOFTWARE_AV1 set, using software AV1 decoder");
            ApplePlayerMode::Software(SoftwareAv1Player::new(
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
            metal_device,
            source,
            autoplay,
            is_looping,
            mode,
            bgra_buf: Vec::new(),
        }
    }

    fn switch_to_software(&mut self, reason: &str) {
        crate::log!(
            "VIDEO: Apple native playback failed, falling back to software AV1 decoder: {}",
            reason
        );
        self.mode = ApplePlayerMode::Software(SoftwareAv1Player::new(
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
        let frame = match &mut self.mode {
            ApplePlayerMode::Native(player) => return player.poll_frame(textures),
            ApplePlayerMode::Software(player) => {
                if !player.poll_frame() {
                    return false;
                }
                player.take_frame().map(|(rgba, w, h)| (rgba.to_vec(), w, h))
            }
        };
        if let Some((rgba, width, height)) = frame {
            self.upload_rgba_to_metal(textures, &rgba, width, height);
            true
        } else {
            false
        }
    }

    fn upload_rgba_to_metal(
        &mut self,
        textures: &mut CxTexturePool,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) {
        let w = width as usize;
        let h = height as usize;
        let expected = w.saturating_mul(h).saturating_mul(4);
        if rgba.len() < expected {
            return;
        }

        self.bgra_buf.resize(expected, 0);
        for i in (0..expected).step_by(4) {
            self.bgra_buf[i] = rgba[i + 2];
            self.bgra_buf[i + 1] = rgba[i + 1];
            self.bgra_buf[i + 2] = rgba[i];
            self.bgra_buf[i + 3] = rgba[i + 3];
        }

        unsafe {
            let cxtexture = &mut textures[self.texture_id];
            let need_alloc = cxtexture
                .alloc
                .as_ref()
                .map_or(true, |alloc| alloc.width != w || alloc.height != h)
                || cxtexture.os.texture.is_none();

            if need_alloc {
                let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
                let _: () = msg_send![descriptor, setTextureType: MTLTextureType::D2];
                let _: () = msg_send![descriptor, setDepth: 1u64];
                let _: () = msg_send![descriptor, setStorageMode: MTLStorageMode::Shared];
                let _: () = msg_send![descriptor, setUsage: MTLTextureUsage::ShaderRead];
                let _: () = msg_send![descriptor, setWidth: width as u64];
                let _: () = msg_send![descriptor, setHeight: height as u64];
                let _: () = msg_send![descriptor, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
                let texture: ObjcId =
                    msg_send![self.metal_device, newTextureWithDescriptor: descriptor];
                let _: () = msg_send![descriptor, release];

                if texture.is_null() {
                    return;
                }

                cxtexture.os.texture = Some(RcObjcId::from_owned(NonNull::new(texture).unwrap()));
                cxtexture.alloc = Some(TextureAlloc {
                    width: w,
                    height: h,
                    pixel: TexturePixel::VideoRGB,
                    category: TextureCategory::Video,
                });
            }

            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: width as u64,
                    height: height as u64,
                    depth: 1,
                },
            };

            let texture = cxtexture.os.texture.as_ref().unwrap().as_id();
            let _: () = msg_send![
                texture,
                replaceRegion: region
                mipmapLevel: 0u64
                withBytes: self.bgra_buf.as_ptr() as *const c_void
                bytesPerRow: (width * 4) as u64
            ];
        }
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
