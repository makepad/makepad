use crate::{
    media_plugin::{media_plugin, MediaVideoEncoder},
    video::*,
};

pub struct VideoEncoder {
    source: VideoEncodeSource,
    codec: VideoCodec,
    backend: Box<dyn MediaVideoEncoder>,
}

impl VideoEncoder {
    pub fn start(config: VideoEncoderConfig, output: VideoOutputFn) -> Option<Self> {
        if config.width == 0 || config.height == 0 || config.fps_num == 0 {
            crate::error!("video encoder invalid config: {:?}", config);
            return None;
        }

        let Some(plugin) = media_plugin() else {
            crate::error!("video encoder unavailable: no media plugin installed");
            return None;
        };

        let source = config.source;
        let codec = config.codec;
        let backend = plugin.create_video_encoder(config, output)?;

        Some(Self {
            source,
            codec,
            backend,
        })
    }

    pub fn source(&self) -> VideoEncodeSource {
        self.source
    }

    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    pub fn push_frame(&self, frame: CameraFrameRef<'_>) {
        self.backend.push_frame(frame);
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn push_apple_pixel_buffer(
        &self,
        pixel_buffer: crate::os::apple::apple_sys::CVPixelBufferRef,
        timestamp_ns: u64,
    ) -> bool {
        self.backend.push_apple_pixel_buffer(pixel_buffer, timestamp_ns)
    }

    pub fn request_keyframe(&self) -> Result<(), VideoEncodeError> {
        self.backend.request_keyframe()
    }

    pub fn stop(&self) {
        self.backend.stop();
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn on_android_h264_packet(encoder_id: u64, pts_us: i64, flags: i32, data: Vec<u8>) {
    if let Some(plugin) = media_plugin() {
        plugin.on_android_h264_packet(encoder_id, pts_us, flags, data);
    }
}

pub fn on_android_h264_error(encoder_id: u64, message: String) {
    if let Some(plugin) = media_plugin() {
        plugin.on_android_h264_error(encoder_id, message);
    }
}
