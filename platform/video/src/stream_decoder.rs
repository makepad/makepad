//! Low-latency hardware H.264/HEVC DECODE seam: Annex-B access units in,
//! raw NV12 frames out — the realtime sibling of the file-based
//! [`crate::VideoFileDecoder`]. See [`crate::stream_encoder`] for the
//! encoder half and the backend notes (`VTDecompressionSession` on macOS,
//! an H.264 decoder MFT on Windows, MediaCodec through the HAL on Android,
//! GStreamer's decodebin on Linux).

use crate::{VideoColor, VideoFileError};
pub use crate::stream_encoder::StreamVideoCodec;

/// One decoded frame: tightly packed NV12, stride == width (same layout as
/// [`crate::DecodedVideoFrame`]), or, from a decoder made with
/// [`VideoStreamDecoder::new_gpu`] on a backend that has one, the decoder's own GPU picture
/// (`gpu`, and `nv12` is empty).
#[derive(Clone, Debug)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
    pub pts_100ns: i64,
    pub gpu: Option<DecodedPicture>,
    /// The picture's colour as its stream says (SPS VUI, as the decoder reports it), in the
    /// range of the decoded samples: what a viewer draws it with.
    pub color: VideoColor,
}

/// A plane of a biplanar 4:2:0 picture: luma (full size) or the interleaved chroma (half).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicturePlane {
    Luma,
    Chroma,
}

/// A decoded picture in the decoder's own GPU memory, biplanar 4:2:0 (NV12 layout; its colour
/// is the frame's `color`). `handle` names it to the platform's texture import
/// (`Cx::import_picture_plane`; macOS: its global IOSurface id). The decoder does not reuse
/// the picture while any clone of this is alive.
#[derive(Clone)]
pub struct DecodedPicture {
    pub handle: u64,
    /// 4:4:4: the chroma plane is full size (else half each way).
    pub full_chroma: bool,
    pub(crate) _keep: std::sync::Arc<dyn Send + Sync>,
}

impl std::fmt::Debug for DecodedPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DecodedPicture({:#x})", self.handle)
    }
}

impl DecodedFrame {
    /// Convert to tightly packed RGB8 in the frame's colour.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let mut out = Vec::new();
        crate::nv12::nv12_to_rgb8(&self.nv12, self.width, self.height, self.color, &mut out);
        out
    }
}

#[cfg(target_vendor = "apple")]
use crate::apple_stream_decoder::AppleStreamDecoder as OsStreamDecoder;
#[cfg(target_os = "windows")]
use crate::windows_stream_decoder::WindowsStreamDecoder as OsStreamDecoder;
#[cfg(target_os = "android")]
use crate::hal_stream_decoder::HalStreamDecoder as OsStreamDecoder;
#[cfg(target_os = "linux")]
use crate::linux_stream_decoder::LinuxStreamDecoder as OsStreamDecoder;

#[cfg(not(any(target_vendor = "apple", target_os = "windows", target_os = "android", target_os = "linux")))]
const UNSUPPORTED: &str = "hardware video stream decode is not implemented on this platform yet";

pub struct VideoStreamDecoder {
    #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "android", target_os = "linux"))]
    os: OsStreamDecoder,
}

impl VideoStreamDecoder {
    pub fn new(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        Self::with_output(codec, false)
    }

    /// A decoder whose frames stay on the GPU (`DecodedFrame::gpu`) where the backend can
    /// (macOS); elsewhere its frames are CPU NV12 as from [`Self::new`].
    pub fn new_gpu(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        Self::with_output(codec, true)
    }

    /// A decoder for a stream of about `size` pictures: Android opens its codec for that size
    /// (the stream's own size wins); elsewhere the same as [`Self::new`].
    pub fn with_size_hint(codec: StreamVideoCodec, size: [u32; 2]) -> Result<Self, VideoFileError> {
        #[cfg(target_os = "android")]
        return Ok(Self { os: OsStreamDecoder::new(codec, size)? });
        #[cfg(not(target_os = "android"))]
        {
            let _ = size;
            Self::with_output(codec, false)
        }
    }

    /// [`Self::with_size_hint`] whose frames stay on the GPU (`DecodedFrame::gpu`): Android's
    /// decoder renders into GPU-sampled hardware buffers; elsewhere as [`Self::new_gpu`].
    pub fn with_size_hint_gpu(codec: StreamVideoCodec, size: [u32; 2]) -> Result<Self, VideoFileError> {
        #[cfg(target_os = "android")]
        return Ok(Self { os: OsStreamDecoder::new_with(codec, size, true)? });
        #[cfg(not(target_os = "android"))]
        {
            let _ = size;
            Self::with_output(codec, true)
        }
    }

    /// The hardware decoder's name, where the backend reports it (Android).
    pub fn name(&self) -> Option<String> {
        #[cfg(target_os = "android")]
        return Some(self.os.name());
        #[cfg(not(target_os = "android"))]
        return None;
    }

    fn with_output(codec: StreamVideoCodec, gpu: bool) -> Result<Self, VideoFileError> {
        #[cfg(target_vendor = "apple")]
        {
            let os = OsStreamDecoder::new(codec, gpu)?;
            return Ok(Self { os });
        }
        #[cfg(target_os = "windows")]
        {
            let _ = gpu;
            let os = OsStreamDecoder::new(codec)?;
            return Ok(Self { os });
        }
        #[cfg(target_os = "android")]
        {
            let _ = gpu;
            return Ok(Self { os: OsStreamDecoder::new(codec, [1920, 1080])? });
        }
        #[cfg(target_os = "linux")]
        {
            let _ = gpu;
            return Ok(Self { os: OsStreamDecoder::new(codec)? });
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "windows", target_os = "android", target_os = "linux")))]
        {
            let _ = (codec, gpu);
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
        #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "android", target_os = "linux"))]
        return self.os.push_packet(annex_b, pts_100ns);
        #[cfg(not(any(target_vendor = "apple", target_os = "windows", target_os = "android", target_os = "linux")))]
        {
            let _ = (annex_b, pts_100ns);
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    /// Whether pictures decode on a hardware decoder (None: not known yet, or not reported on
    /// this platform).
    pub fn hardware(&self) -> Option<bool> {
        #[cfg(target_vendor = "apple")]
        return self.os.hardware();
        #[cfg(not(target_vendor = "apple"))]
        return None;
    }

    /// Drains any frames still buffered inside the decoder (call at stream
    /// end; the synchronous push/flush-per-packet design used here means
    /// this is normally empty, but it is not a promise every backend keeps
    /// zero-latency).
    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "android", target_os = "linux"))]
        return self.os.flush();
        #[cfg(not(any(target_vendor = "apple", target_os = "windows", target_os = "android", target_os = "linux")))]
        return Ok(Vec::new());
    }
}
