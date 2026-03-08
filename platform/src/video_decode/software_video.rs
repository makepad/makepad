use {
    crate::{
        event::video_playback::VideoSource,
        makepad_live_id::LiveId,
        media_plugin::{media_plugin, MediaSoftwareVideoPlayer},
        texture::TextureId,
        video::VideoDecodeError,
        video_decode::yuv::YuvPlaneData,
    },
};

pub struct SoftwareVideoPlayer {
    pub video_id: LiveId,
    pub texture_id: TextureId,
    inner: Option<Box<dyn MediaSoftwareVideoPlayer>>,
    failed: Option<String>,
}

impl SoftwareVideoPlayer {
    pub fn new(
        video_id: LiveId,
        texture_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        let (inner, failed) = match media_plugin() {
            Some(plugin) => match plugin.create_software_video_player(
                video_id,
                texture_id,
                source,
                autoplay,
                is_looping,
            ) {
                Ok(player) => (Some(player), None),
                Err(err) => (None, Some(format!("software video unavailable: {:?}", err))),
            },
            None => (
                None,
                Some("software video unavailable: no media plugin installed".to_string()),
            ),
        };

        Self {
            video_id,
            texture_id,
            inner,
            failed,
        }
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        if let Some(inner) = &mut self.inner {
            return inner.check_prepared();
        }
        self.failed.take().map(Err)
    }

    pub fn poll_frame(&mut self) -> bool {
        self.inner.as_mut().map(|p| p.poll_frame()).unwrap_or(false)
    }

    pub fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.inner.as_mut().and_then(|p| p.take_yuv_frame())
    }

    pub fn check_eos(&mut self) -> bool {
        self.inner.as_mut().map(|p| p.check_eos()).unwrap_or(false)
    }

    pub fn play(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.play();
        }
    }

    pub fn pause(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.pause();
        }
    }

    pub fn resume(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.resume();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.inner.as_ref().map(|p| p.is_playing()).unwrap_or(false)
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        if let Some(inner) = &mut self.inner {
            inner.seek_to(position_ms);
        }
    }

    pub fn set_volume(&self, volume: f64) {
        if let Some(inner) = &self.inner {
            inner.set_volume(volume);
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        self.inner
            .as_ref()
            .map(|p| p.current_position_ms())
            .unwrap_or(0)
    }

    pub fn mute(&self) {
        if let Some(inner) = &self.inner {
            inner.mute();
        }
    }

    pub fn unmute(&self) {
        if let Some(inner) = &self.inner {
            inner.unmute();
        }
    }

    pub fn set_playback_rate(&self, rate: f64) {
        if let Some(inner) = &self.inner {
            inner.set_playback_rate(rate);
        }
    }

    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        self.inner
            .as_ref()
            .map(|p| p.seekable_ranges())
            .unwrap_or_default()
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.inner
            .as_ref()
            .map(|p| p.buffered_ranges())
            .unwrap_or_default()
    }

    pub fn is_active(&self) -> bool {
        self.inner.as_ref().map(|p| p.is_active()).unwrap_or(false)
    }

    pub fn cleanup(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.cleanup();
        }
    }

    pub fn decode_error(&self) -> Result<(), VideoDecodeError> {
        if self.inner.is_some() {
            Ok(())
        } else {
            Err(VideoDecodeError::UnsupportedCodec)
        }
    }
}
