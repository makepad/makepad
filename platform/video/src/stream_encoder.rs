//! Low-latency hardware H.264/HEVC ENCODE seam: raw frames in, Annex-B
//! access units out — no container, no file. This is the realtime sibling
//! of the file-based [`crate::VideoFileEncoder`] (which writes a finished
//! mp4 through `AVAssetWriter`/`IMFSinkWriter`); this one hands the caller
//! compressed packets frame-by-frame so a live network stream (e.g.
//! `makepad-asset-ai`'s realtime session) can push them out immediately.
//!
//! Backends:
//! - macOS: `VTCompressionSession` directly (not `AVAssetWriter` — that API
//!   has no raw-NAL-access seam). AVCC output is converted to Annex-B (see
//!   [`crate::annex_b`]).
//! - Windows: an H.264 encoder MFT (hardware via `MFTEnumEx`, falling back
//!   to the Microsoft software encoder), driven directly with
//!   `ProcessInput`/`ProcessOutput` — Annex-B output natively, no
//!   conversion needed.
//! - Other platforms: [`VideoFileError`] (`UNSUPPORTED`).

use crate::VideoFileError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamVideoCodec {
    H264,
    /// Compile-time supported on both backends' code paths but not
    /// currently wired up end-to-end — H.264 is the shipped codec.
    Hevc,
}

#[derive(Clone, Copy, Debug)]
pub struct VideoStreamEncoderOptions {
    pub codec: StreamVideoCodec,
    /// Must be even (4:2:0 chroma) — same constraint as the file encoder.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// Frames between forced keyframes (also the GOP size).
    pub keyint: u32,
    /// Disables B-frame reordering and asks for the lowest-latency rate
    /// control the platform offers — the live-session use case cares about
    /// per-frame turnaround, not offline quality.
    pub low_latency: bool,
}

impl Default for VideoStreamEncoderOptions {
    fn default() -> Self {
        Self {
            codec: StreamVideoCodec::H264,
            width: 0,
            height: 0,
            fps: 30,
            bitrate_kbps: 4_000,
            keyint: 30,
            low_latency: true,
        }
    }
}

/// One encoded access unit: Annex-B NAL units (start-code delimited). A
/// keyframe packet's NALs always begin with SPS + PPS, then the IDR slice —
/// a decoder can (re)initialize from any keyframe packet alone.
#[derive(Clone, Debug)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub pts_100ns: i64,
    pub is_key: bool,
}

#[cfg(target_os = "macos")]
use crate::apple_stream_encoder::AppleStreamEncoder as OsStreamEncoder;
#[cfg(target_os = "windows")]
use crate::windows_stream_encoder::WindowsStreamEncoder as OsStreamEncoder;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const UNSUPPORTED: &str = "hardware video stream encode is not implemented on this platform yet";

pub struct VideoStreamEncoder {
    options: VideoStreamEncoderOptions,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    os: OsStreamEncoder,
}

impl VideoStreamEncoder {
    pub fn new(options: VideoStreamEncoderOptions) -> Result<Self, VideoFileError> {
        if options.width == 0 || options.height == 0 || options.width % 2 != 0 || options.height % 2 != 0 {
            return Err(VideoFileError::new(format!(
                "invalid stream encoder frame size {}x{} (must be nonzero and even)",
                options.width, options.height
            )));
        }
        if options.fps == 0 {
            return Err(VideoFileError::new("invalid stream encoder fps 0"));
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let os = OsStreamEncoder::new(&options)?;
            return Ok(Self { options, os });
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = &options;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    pub fn options(&self) -> &VideoStreamEncoderOptions {
        &self.options
    }

    /// Encodes one tightly packed RGB8 (`width*height*3` bytes) frame.
    /// Returns whatever packets the encoder produced for it — usually
    /// exactly one (the encoder is driven synchronously: this call does not
    /// return until the platform has finished encoding the frame).
    pub fn push_frame_rgb8(&mut self, rgb: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        let expected = self.options.width as usize * self.options.height as usize * 3;
        if rgb.len() != expected {
            return Err(VideoFileError::new(format!(
                "rgb frame size {} != expected {expected}",
                rgb.len()
            )));
        }
        let mut nv12 = Vec::new();
        crate::nv12::rgb8_to_nv12(rgb, self.options.width, self.options.height, &mut nv12);
        self.push_frame_nv12(&nv12, pts_100ns)
    }

    /// Encodes one tightly packed NV12 frame.
    pub fn push_frame_nv12(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        let expected = crate::nv12::nv12_frame_size(self.options.width, self.options.height);
        if nv12.len() != expected {
            return Err(VideoFileError::new(format!(
                "nv12 frame size {} != expected {expected}",
                nv12.len()
            )));
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        return self.os.push_frame_nv12(nv12, pts_100ns);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = pts_100ns;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    /// Forces the NEXT pushed frame to be a keyframe (SPS/PPS + IDR) — call
    /// this when a new receiver joins a live stream so it can start
    /// decoding immediately instead of waiting for the next scheduled
    /// keyframe (up to `keyint` frames away).
    pub fn request_keyframe(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.os.request_keyframe();
    }
}
