use {
    crate::{
        event::video_playback::VideoSource,
        makepad_live_id::LiveId,
        texture::TextureId,
        video::{
            CameraFrameRef, VideoDecodeError, VideoEncodeError, VideoEncoderConfig, VideoOutputFn,
        },
    },
    std::sync::{Arc, OnceLock},
};

#[cfg(not(target_arch = "wasm32"))]
use crate::video_decode::yuv::YuvPlaneData;

#[cfg(target_arch = "wasm32")]
pub struct YuvPlaneData;

// ---------------------------------------------------------------------------
// MSE (Media Source Extensions) player trait
// ---------------------------------------------------------------------------

/// Result of an `MsePlayer::append_data()` call.
pub struct MseAppendResult {
    /// True when an init segment (ftyp+moov) was successfully parsed.
    pub init_segment_parsed: bool,
    /// Video width from init segment (0 until parsed).
    pub width: u32,
    /// Video height from init segment (0 until parsed).
    pub height: u32,
    /// Duration in milliseconds from the init segment (0 if unknown/live).
    pub duration_ms: u128,
    /// Newly decoded frames ready for display.
    pub new_frames: Vec<MseDecodedFrame>,
    /// Updated buffered time ranges (seconds).
    pub buffered_ranges: Vec<(f64, f64)>,
}

/// A single decoded video frame from the MSE pipeline.
pub struct MseDecodedFrame {
    /// Presentation timestamp in milliseconds.
    pub pts_ms: u64,
    /// YUV plane data for texture upload.
    pub yuv: YuvPlaneData,
}

/// Push-based video player for MSE (Media Source Extensions).
///
/// Accepts incremental fMP4 data (init segments + media segments) and
/// produces decoded YUV frames. Codec selection is handled internally
/// based on the init segment: dav1d for AV1, platform decoders for H.264.
pub trait MsePlayer: Send {
    /// Push fMP4 data (init segment or media segment bytes).
    /// Returns decoded frames and metadata updates.
    fn append_data(&mut self, data: &[u8]) -> Result<MseAppendResult, String>;

    /// Signal end of stream. Flushes any buffered decoder output.
    fn end_of_stream(&mut self) -> Result<Vec<MseDecodedFrame>, String>;

    /// Remove buffered data in a time range (seconds).
    fn remove(&mut self, start: f64, end: f64);

    /// Current buffered time ranges (seconds).
    fn buffered_ranges(&self) -> Vec<(f64, f64)>;

    /// Flush decoder state (e.g. after seek).
    fn flush(&mut self);

    /// Clean up resources.
    fn cleanup(&mut self);
}

// ---------------------------------------------------------------------------
// Platform video frame decoder (H.264 etc.)
// ---------------------------------------------------------------------------

/// Codec identifier for platform decoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecoderCodec {
    H264,
}

/// Configuration for a platform video frame decoder.
pub struct FrameDecoderConfig {
    pub codec: FrameDecoderCodec,
    /// Codec-specific configuration data (e.g. AVCC record for H.264).
    pub codec_config: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Push-based video frame decoder abstraction.
///
/// Platform backends implement this trait to provide hardware-accelerated
/// decode. Feed compressed frames via `push_data`, pull decoded YUV via
/// `pull_frame`.
pub trait VideoFrameDecoder: Send {
    /// Push a compressed frame (Annex B NAL units for H.264).
    /// `pts_ms` is the presentation timestamp in milliseconds.
    fn push_data(&mut self, data: &[u8], pts_ms: u64) -> Result<(), String>;

    /// Pull a decoded YUV frame, if available.
    fn pull_frame(&mut self) -> Result<Option<MseDecodedFrame>, String>;

    /// Flush the decoder (e.g. for EOS or seek).
    fn flush(&mut self);
}

pub trait MediaVideoEncoder: Send + Sync {
    fn push_frame(&self, frame: CameraFrameRef<'_>);

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn push_apple_pixel_buffer(
        &self,
        _pixel_buffer: crate::os::apple::apple_sys::CVPixelBufferRef,
        _timestamp_ns: u64,
    ) -> bool {
        false
    }

    fn request_keyframe(&self) -> Result<(), VideoEncodeError>;
    fn stop(&self);
}

pub trait MediaSoftwareVideoPlayer {
    fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>>;
    fn poll_frame(&mut self) -> bool;
    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData>;
    fn check_eos(&mut self) -> bool;
    fn play(&mut self);
    fn pause(&mut self);
    fn resume(&mut self);
    fn is_playing(&self) -> bool;
    fn seek_to(&mut self, position_ms: u64);
    fn set_volume(&self, volume: f64);
    fn current_position_ms(&self) -> u128;
    fn mute(&self);
    fn unmute(&self);
    fn set_playback_rate(&self, rate: f64);
    fn seekable_ranges(&self) -> Vec<(f64, f64)>;
    fn buffered_ranges(&self) -> Vec<(f64, f64)>;
    fn is_active(&self) -> bool;
    fn cleanup(&mut self);
}

pub trait MediaPlugin: Send + Sync {
    fn create_video_encoder(
        &self,
        _config: VideoEncoderConfig,
        _output: VideoOutputFn,
    ) -> Option<Box<dyn MediaVideoEncoder>> {
        None
    }

    fn create_software_video_player(
        &self,
        _video_id: LiveId,
        _texture_id: TextureId,
        _source: VideoSource,
        _autoplay: bool,
        _is_looping: bool,
    ) -> Result<Box<dyn MediaSoftwareVideoPlayer>, VideoDecodeError> {
        Err(VideoDecodeError::UnsupportedCodec)
    }

    fn video_capabilities(&self) -> crate::video::VideoCapabilities {
        crate::video::VideoCapabilities::default()
    }

    fn on_android_h264_packet(&self, _encoder_id: u64, _pts_us: i64, _flags: i32, _data: Vec<u8>) {}

    fn on_android_h264_error(&self, _encoder_id: u64, _message: String) {}

    /// Create an MSE player for the given MIME type (e.g. `video/mp4; codecs="av01.0.04M.08"`).
    /// Returns `Err` if the MIME type is unsupported.
    fn create_mse_player(&self, _mime: &str) -> Result<Box<dyn MsePlayer>, String> {
        Err("MSE not supported by this media plugin".into())
    }

    /// Create a platform video frame decoder for the given codec.
    /// Used by MSE player to decode H.264 via GStreamer/VideoToolbox/MediaCodec.
    fn create_video_frame_decoder(
        &self,
        _config: FrameDecoderConfig,
    ) -> Result<Box<dyn VideoFrameDecoder>, String> {
        Err("platform video frame decoder not available".into())
    }
}

static MEDIA_PLUGIN: OnceLock<Arc<dyn MediaPlugin>> = OnceLock::new();

pub fn register_media_plugin(plugin: Arc<dyn MediaPlugin>) -> bool {
    MEDIA_PLUGIN.set(plugin).is_ok()
}

pub fn media_plugin() -> Option<&'static Arc<dyn MediaPlugin>> {
    MEDIA_PLUGIN.get()
}

pub fn media_video_capabilities() -> crate::video::VideoCapabilities {
    media_plugin()
        .map(|p| p.video_capabilities())
        .unwrap_or_default()
}

pub fn merge_video_capabilities(
    mut base: crate::video::VideoCapabilities,
    extra: crate::video::VideoCapabilities,
) -> crate::video::VideoCapabilities {
    use crate::video::VideoCodecSupport;

    fn merge_codec(a: &mut VideoCodecSupport, b: VideoCodecSupport) {
        a.encode_hardware |= b.encode_hardware;
        a.encode_software |= b.encode_software;
        a.decode_hardware |= b.decode_hardware;
        a.decode_software |= b.decode_software;

        for fmt in b.encode_formats {
            if !a.encode_formats.contains(&fmt) {
                a.encode_formats.push(fmt);
            }
        }
        for fmt in b.decode_formats {
            if !a.decode_formats.contains(&fmt) {
                a.decode_formats.push(fmt);
            }
        }

        a.supports_camera_source |= b.supports_camera_source;
        a.supports_texture_source |= b.supports_texture_source;
        a.supports_cpu_frames_source |= b.supports_cpu_frames_source;
        a.supports_keyframe_request |= b.supports_keyframe_request;
        a.supports_dynamic_resolution |= b.supports_dynamic_resolution;

        if a.width_alignment.is_none() {
            a.width_alignment = b.width_alignment;
        }
        if a.height_alignment.is_none() {
            a.height_alignment = b.height_alignment;
        }
        if a.max_width.is_none() {
            a.max_width = b.max_width;
        }
        if a.max_height.is_none() {
            a.max_height = b.max_height;
        }
        if a.max_fps.is_none() {
            a.max_fps = b.max_fps;
        }
        if a.max_bitrate.is_none() {
            a.max_bitrate = b.max_bitrate;
        }
    }

    for incoming in extra.codecs {
        if let Some(existing) = base.codecs.iter_mut().find(|c| c.codec == incoming.codec) {
            merge_codec(existing, incoming);
        } else {
            base.codecs.push(incoming);
        }
    }

    base
}
