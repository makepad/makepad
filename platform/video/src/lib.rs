//! Video FILE codec seam: encode raw frames (+ optional PCM audio) into an
//! MP4 on disk, and decode an MP4 back into raw frames (+ PCM audio).
//!
//! This is the platform-accelerated *offline/artifact* path. It is
//! deliberately free of `Cx` — any thread can construct and drive an encoder
//! or decoder without a running event loop. Lives in `makepad-video` so AI
//! content / H3 mux does not pull the full UI platform crate.
//!
//! Backends:
//! - Windows: Media Foundation sink writer / source reader with hardware
//!   transforms enabled. The OS auto-selects whatever hardware codec MFT the
//!   machine has (NVIDIA NVENC/NVDEC, AMD VCN, Intel QuickSync) and falls back
//!   to the software MFT when no hardware engine exists. The seam is strictly
//!   vendor-neutral: which transform engaged is *reported* (see
//!   [`VideoFileEncoder::video_transform`]) but never required.
//! - macOS: AVFoundation — AVAssetReader / AVAssetWriter with VideoToolbox
//!   codecs (hardware H.264 + H.265 engines on Apple Silicon) and the system
//!   AAC codec for the audio track.
//! - Other platforms: stubs returning [`VideoFileError`] for now. The facade
//!   is shaped so an Android/Quest MediaCodec backend can slot in later.
//!
//! # Integration shape (AI content video backend, H3 et al)
//!
//! A video generation backend produces decoded RGB frames (e.g. VAE output,
//! one `&[u8]` of `w*h*3` per frame at a fixed fps) and optionally a PCM wav
//! track. The encoder here is the artifact producer:
//!
//! ```ignore
//! let mut enc = VideoFileEncoder::new("out.mp4", VideoFileEncoderOptions {
//!     codec: VideoFileCodec::H265,
//!     width, height,
//!     fps_num: 24, fps_den: 1,
//!     video_bitrate_bps: 8_000_000,
//!     audio: Some(PcmAudioTrackOptions { sample_rate: 24_000, channels: 1, aac_bitrate_bps: 128_000 }),
//! })?;
//! for frame in frames {
//!     enc.push_frame_rgb8(&frame, None)?;      // None = timestamps derived from fps
//! }
//! enc.push_audio_i16(&wav_samples)?;           // interleaved i16 PCM, AAC-encoded by the OS
//! enc.finish()?;                               // finalized mp4 on disk
//! ```
//!
//! Frames can also be supplied pre-converted as NV12 (`push_frame_nv12`); the
//! RGB entry points use the BT.709 converters in [`nv12`]. Audio may be pushed
//! interleaved with video or in one block at the end for short clips.

pub mod annex_b;
pub mod nv12;
pub mod stream_decoder;
pub mod stream_encoder;

pub use stream_decoder::{DecodedFrame, VideoStreamDecoder};
pub use stream_encoder::{EncodedPacket, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};

use std::fmt;

/// Codec for the encoded video stream. H.265/HEVC is the primary target
/// (hardware-decodable on Quest-class devices, ~40% smaller); H.264 is the
/// compatibility fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoFileCodec {
    H265,
    H264,
}

/// Options for the optional AAC audio track, fed as 16-bit interleaved PCM.
#[derive(Clone, Copy, Debug)]
pub struct PcmAudioTrackOptions {
    pub sample_rate: u32,
    /// 1 (mono) or 2 (stereo).
    pub channels: u16,
    /// Target AAC bitrate in bits/second. Snapped to the nearest rate the
    /// platform AAC encoder supports (96/128/160/192 kbps on Windows).
    pub aac_bitrate_bps: u32,
}

impl Default for PcmAudioTrackOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            aac_bitrate_bps: 128_000,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VideoFileEncoderOptions {
    pub codec: VideoFileCodec,
    /// Frame width in pixels. Must be even (4:2:0 chroma).
    pub width: u32,
    /// Frame height in pixels. Must be even (4:2:0 chroma).
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub video_bitrate_bps: u32,
    pub audio: Option<PcmAudioTrackOptions>,
}

impl Default for VideoFileEncoderOptions {
    fn default() -> Self {
        Self {
            codec: VideoFileCodec::H265,
            width: 0,
            height: 0,
            fps_num: 30,
            fps_den: 1,
            video_bitrate_bps: 8_000_000,
            audio: None,
        }
    }
}

/// Error from the video file codec seam. `code` carries the platform error
/// (HRESULT on Windows) when one exists.
#[derive(Clone, Debug)]
pub struct VideoFileError {
    pub context: String,
    pub code: Option<i32>,
}

impl VideoFileError {
    pub fn new(context: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            code: None,
        }
    }

    pub fn with_code(context: impl Into<String>, code: i32) -> Self {
        Self {
            context: context.into(),
            code: Some(code),
        }
    }
}

impl fmt::Display for VideoFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "{} (os error {:#010x})", self.context, code as u32),
            None => write!(f, "{}", self.context),
        }
    }
}

impl std::error::Error for VideoFileError {}

/// Which codec transform the platform actually engaged for the video stream.
#[derive(Clone, Debug)]
pub struct VideoTransformInfo {
    /// Friendly name of the transform when the platform exposes one.
    pub name: String,
    /// True when a hardware codec engine (vendor-neutral: NVENC / VCN /
    /// QuickSync / ...) is doing the work; false = software fallback.
    pub is_hardware: bool,
}

#[cfg(target_os = "windows")]
mod windows_encoder;
#[cfg(target_os = "windows")]
mod windows_decoder;
#[cfg(target_os = "windows")]
mod windows_mft;
#[cfg(target_os = "windows")]
mod windows_stream_encoder;
#[cfg(target_os = "windows")]
mod windows_stream_decoder;
#[cfg(target_os = "windows")]
use windows_decoder::WindowsVideoFileDecoder as OsVideoFileDecoder;
#[cfg(target_os = "windows")]
use windows_encoder::WindowsVideoFileEncoder as OsVideoFileEncoder;

#[cfg(target_os = "macos")]
mod apple_decoder;
#[cfg(target_os = "macos")]
mod apple_encoder;
#[cfg(target_os = "macos")]
mod apple_stream_encoder;
#[cfg(target_os = "macos")]
mod apple_stream_decoder;
#[cfg(target_os = "macos")]
use apple_decoder::MacosVideoFileDecoder as OsVideoFileDecoder;
#[cfg(target_os = "macos")]
use apple_encoder::MacosVideoFileEncoder as OsVideoFileEncoder;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const UNSUPPORTED: &str = "video file codec is not implemented on this platform yet";

// ---------------------------------------------------------------------------
// Encoder facade
// ---------------------------------------------------------------------------

pub struct VideoFileEncoder {
    options: VideoFileEncoderOptions,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    os: OsVideoFileEncoder,
}

impl VideoFileEncoder {
    /// Create an mp4 encoder writing to `path`. The container is finalized by
    /// [`VideoFileEncoder::finish`]; dropping without `finish` attempts a
    /// best-effort finalize but reports no errors.
    pub fn new(path: &str, options: VideoFileEncoderOptions) -> Result<Self, VideoFileError> {
        if options.width == 0
            || options.height == 0
            || options.width % 2 != 0
            || options.height % 2 != 0
        {
            return Err(VideoFileError::new(format!(
                "invalid encoder frame size {}x{} (must be nonzero and even)",
                options.width, options.height
            )));
        }
        if options.fps_num == 0 || options.fps_den == 0 {
            return Err(VideoFileError::new("invalid encoder fps 0"));
        }
        if let Some(audio) = &options.audio {
            if audio.sample_rate == 0 || !(audio.channels == 1 || audio.channels == 2) {
                return Err(VideoFileError::new(format!(
                    "invalid audio track options: {} Hz, {} channels",
                    audio.sample_rate, audio.channels
                )));
            }
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            let os = OsVideoFileEncoder::new(path, &options)?;
            return Ok(Self { options, os });
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = path;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    pub fn options(&self) -> &VideoFileEncoderOptions {
        &self.options
    }

    /// The codec transform the platform selected for the video stream, when
    /// known. Resolved during `new`.
    pub fn video_transform(&self) -> Option<&VideoTransformInfo> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.video_transform();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        return None;
    }

    /// Push one frame of tightly packed RGB, `width*height*3` bytes.
    /// `pts_100ns: None` derives the timestamp from the frame counter and fps.
    pub fn push_frame_rgb8(&mut self, rgb: &[u8], pts_100ns: Option<i64>) -> Result<(), VideoFileError> {
        let expected = self.options.width as usize * self.options.height as usize * 3;
        if rgb.len() != expected {
            return Err(VideoFileError::new(format!(
                "rgb frame size {} != expected {} ({}x{}x3)",
                rgb.len(),
                expected,
                self.options.width,
                self.options.height
            )));
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.push_frame_rgb(rgb, 3, pts_100ns);
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = pts_100ns;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    /// Push one frame of tightly packed RGBA (alpha ignored), `width*height*4` bytes.
    pub fn push_frame_rgba8(&mut self, rgba: &[u8], pts_100ns: Option<i64>) -> Result<(), VideoFileError> {
        let expected = self.options.width as usize * self.options.height as usize * 4;
        if rgba.len() != expected {
            return Err(VideoFileError::new(format!(
                "rgba frame size {} != expected {} ({}x{}x4)",
                rgba.len(),
                expected,
                self.options.width,
                self.options.height
            )));
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.push_frame_rgb(rgba, 4, pts_100ns);
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = pts_100ns;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    /// Push one frame of tightly packed NV12 (`nv12::nv12_frame_size` bytes).
    pub fn push_frame_nv12(&mut self, nv12: &[u8], pts_100ns: Option<i64>) -> Result<(), VideoFileError> {
        let expected = nv12::nv12_frame_size(self.options.width, self.options.height);
        if nv12.len() != expected {
            return Err(VideoFileError::new(format!(
                "nv12 frame size {} != expected {}",
                nv12.len(),
                expected
            )));
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.push_frame_nv12(nv12, pts_100ns);
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = pts_100ns;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    /// Push interleaved 16-bit PCM for the audio track. Timestamps are derived
    /// from the running sample count. Requires `options.audio`.
    pub fn push_audio_i16(&mut self, samples: &[i16]) -> Result<(), VideoFileError> {
        let Some(audio) = self.options.audio else {
            return Err(VideoFileError::new(
                "push_audio_i16 called but encoder was created without an audio track",
            ));
        };
        if samples.len() % audio.channels as usize != 0 {
            return Err(VideoFileError::new(format!(
                "audio sample count {} not a multiple of {} channels",
                samples.len(),
                audio.channels
            )));
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.push_audio_i16(samples);
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        return Err(VideoFileError::new(UNSUPPORTED));
    }

    /// Flush and finalize the container. Must be called for a playable mp4.
    pub fn finish(mut self) -> Result<(), VideoFileError> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.finish();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = &mut self;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder facade
// ---------------------------------------------------------------------------

/// Stream information for an opened video file.
#[derive(Clone, Debug)]
pub struct VideoFileInfo {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Total duration in 100ns units (0 when the container does not report one).
    pub duration_100ns: i64,
    /// The compressed video codec when it is one we name.
    pub video_codec: Option<VideoFileCodec>,
    /// FourCC / first GUID dword of the compressed video subtype, for diagnostics.
    pub video_codec_fourcc: u32,
    pub has_audio: bool,
    pub audio_sample_rate: u32,
    pub audio_channels: u16,
}

/// One decoded video frame: tightly packed NV12, stride == width.
pub struct DecodedVideoFrame {
    pub pts_100ns: i64,
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
}

impl DecodedVideoFrame {
    /// Convert to tightly packed RGB8 (BT.709).
    pub fn to_rgb8(&self) -> Vec<u8> {
        let mut out = Vec::new();
        nv12::nv12_to_rgb8(&self.nv12, self.width, self.height, &mut out);
        out
    }
}

/// One decoded chunk of interleaved 16-bit PCM audio.
pub struct DecodedAudioChunk {
    pub pts_100ns: i64,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

pub struct VideoFileDecoder {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    os: OsVideoFileDecoder,
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    info: VideoFileInfo,
}

impl VideoFileDecoder {
    /// Open an mp4 (or any container the platform demuxes) for decoding.
    pub fn open(path: &str) -> Result<Self, VideoFileError> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            let os = OsVideoFileDecoder::open(path)?;
            return Ok(Self { os });
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = path;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    pub fn info(&self) -> &VideoFileInfo {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.info();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        return &self.info;
    }

    /// Pull the next decoded video frame; `Ok(None)` at end of stream.
    pub fn next_frame(&mut self) -> Result<Option<DecodedVideoFrame>, VideoFileError> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.next_frame();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        return Err(VideoFileError::new(UNSUPPORTED));
    }

    /// Pull the next decoded PCM audio chunk; `Ok(None)` at end of stream.
    pub fn next_audio(&mut self) -> Result<Option<DecodedAudioChunk>, VideoFileError> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return self.os.next_audio();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        return Err(VideoFileError::new(UNSUPPORTED));
    }
}
