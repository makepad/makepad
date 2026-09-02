//! Low-latency hardware H.264/HEVC DECODE seam: Annex-B access units in,
//! raw NV12 frames out — the realtime sibling of the file-based
//! [`crate::VideoFileDecoder`]. See [`crate::stream_encoder`] for the
//! encoder half and the backend notes (`VTDecompressionSession` on macOS,
//! an H.264 decoder MFT on Windows).

use crate::VideoFileError;
pub use crate::stream_encoder::StreamVideoCodec;

/// One decoded frame: tightly packed NV12, stride == width (same layout as
/// [`crate::DecodedVideoFrame`]).
#[derive(Clone, Debug)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
    pub pts_100ns: i64,
}

impl DecodedFrame {
    /// Convert to tightly packed RGB8 (BT.709).
    pub fn to_rgb8(&self) -> Vec<u8> {
        let mut out = Vec::new();
        crate::nv12::nv12_to_rgb8(&self.nv12, self.width, self.height, &mut out);
        out
    }
}

#[cfg(target_os = "macos")]
use crate::apple_stream_decoder::AppleStreamDecoder as OsStreamDecoder;
#[cfg(target_os = "windows")]
use crate::windows_stream_decoder::WindowsStreamDecoder as OsStreamDecoder;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const UNSUPPORTED: &str = "hardware video stream decode is not implemented on this platform yet";

pub struct VideoStreamDecoder {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    os: OsStreamDecoder,
}

impl VideoStreamDecoder {
    pub fn new(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let os = OsStreamDecoder::new(codec)?;
            return Ok(Self { os });
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = codec;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    /// Pushes one Annex-B access unit (as produced by [`crate::stream_
    /// encoder::VideoStreamEncoder`] — one or more start-code-delimited NAL
    /// units; a keyframe packet's SPS/PPS (re)initializes the decoder).
    /// Returns whatever frames the decoder produced for it — 0 or 1 in the
    /// synchronous single-frame-in/single-frame-out steady state; SPS/PPS-
    /// only packets or a mid-GOP decoder (re)initialization boundary
    /// legitimately produce 0.
    pub fn push_packet(&mut self, annex_b: &[u8], pts_100ns: i64) -> Result<Vec<DecodedFrame>, VideoFileError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        return self.os.push_packet(annex_b, pts_100ns);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (annex_b, pts_100ns);
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    /// Drains any frames still buffered inside the decoder (call at stream
    /// end; the synchronous push/flush-per-packet design used here means
    /// this is normally empty, but it is not a promise every backend keeps
    /// zero-latency).
    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        return self.os.flush();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Ok(Vec::new());
    }
}
