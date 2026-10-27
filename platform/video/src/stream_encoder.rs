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
//! - Linux: the GPU's hardware H.264 encoder through Vulkan Video on the HAL's device
//!   (`makepad_hal::stream_encode`), Annex-B natively. Takes shared textures
//!   ([`VideoStreamEncoder::push_shared_texture`]) as well as CPU NV12.
//! - Android: the phone's MediaCodec encoder in the HAL host (`makepad_hal::stream_encode`, the
//!   same ops as Linux's), CPU NV12 pictures; it may keep one picture back (a push then returns
//!   no packet, the next one the picture before).
//! - Other platforms: [`VideoFileError`] (`UNSUPPORTED`).
//!
//! `keyint` 0 means no scheduled key frames: one at the start and on
//! [`VideoStreamEncoder::request_keyframe`] (a live view whose transport loses nothing).

use crate::VideoFileError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamVideoCodec {
    H264,
    /// H.265: Linux's encoder (Main, or Format range extensions 4:4:4), Apple's decoder.
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
    /// Frames between forced keyframes (also the GOP size); 0 = only on request.
    pub keyint: u32,
    /// Disables B-frame reordering and asks for the lowest-latency rate
    /// control the platform offers — the live-session use case cares about
    /// per-frame turnaround, not offline quality.
    pub low_latency: bool,
    /// 4:4:4 chroma (H.264 High 4:4:4 Predictive) instead of 4:2:0: sharp colour edges (text).
    /// Linux's encoder, from texture pictures.
    pub full_chroma: bool,
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
            full_chroma: false,
        }
    }
}

/// How a stream trades encode time, latency and bits (Linux's encoder; ignored elsewhere).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamTuning {
    /// The encoder's quality level: 0 its fastest; more than it has: its best.
    pub quality: u32,
    /// Constant bit rate instead of VBR: every picture's bits within `buffer_pictures` average
    /// pictures (a burst fits the link).
    pub constant_rate: bool,
    /// The rate control's buffer, in pictures.
    pub buffer_pictures: u32,
    /// The encoder's best pictures per bit (more encode time) instead of its fastest encode.
    pub sharp: bool,
}

impl Default for StreamTuning {
    fn default() -> Self {
        StreamTuning { quality: 0, constant_rate: false, buffer_pictures: 2, sharp: true }
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

/// A tile of a larger source picture (`source_height` rows): its pixels from column `x0`
/// (a multiple of 4) and row `y0` on, encoded as a stream of its own on encode engine `engine`:
/// the tiles of one picture encode at once on a GPU with several engines
/// ([`VideoStreamEncoder::engines`]), and a picture larger than one stream takes
/// ([`VideoStreamEncoder::max_size`]) is split into columns as well as bands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamTile {
    pub engine: u32,
    pub x0: u32,
    pub y0: u32,
    pub source_height: u32,
}

/// A stream encoder's QP delta map grid ([`VideoStreamEncoder::qp_grid`]): `cols` x `rows`
/// texels, each covering `texel` pixels of the picture, deltas from `min` to `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QpGrid {
    pub cols: u32,
    pub rows: u32,
    pub texel: [u32; 2],
    pub min: i32,
    pub max: i32,
}

/// When a shared texture's picture may be read: its producer's GPU work is complete, or it
/// completes when the sync_file signals (Linux: the release fence of the frame that rendered it,
/// `makepad_hal::dmabuf::hal_dmabuf_fence`).
pub enum GpuReady {
    Done,
    #[cfg(target_os = "linux")]
    SyncFile(std::os::fd::OwnedFd),
}

#[cfg(target_vendor = "apple")]
use crate::apple_stream_encoder::AppleStreamEncoder as OsStreamEncoder;
#[cfg(target_os = "windows")]
use crate::windows_stream_encoder::WindowsStreamEncoder as OsStreamEncoder;
#[cfg(any(target_os = "linux", target_os = "android"))]
use crate::hal_stream_encoder::HalStreamEncoder as OsStreamEncoder;

#[cfg(not(any(target_vendor = "apple", target_os = "windows", target_os = "linux", target_os = "android")))]
const UNSUPPORTED: &str = "hardware video stream encode is not implemented on this platform yet";

pub struct VideoStreamEncoder {
    options: VideoStreamEncoderOptions,
    /// The band of its pictures this encoder takes (all of them: `y0` 0, `source_height` its
    /// height).
    tile: StreamTile,
    #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux", target_os = "android"))]
    os: OsStreamEncoder,
}

impl VideoStreamEncoder {
    pub fn new(options: VideoStreamEncoderOptions) -> Result<Self, VideoFileError> {
        Self::new_tile(options, StreamTile { engine: 0, x0: 0, y0: 0, source_height: options.height })
    }

    /// The hardware encode engines of this machine: how many tiles a picture is best split into
    /// (1 where there is one, or where pictures are not split).
    pub fn engines() -> u32 {
        #[cfg(target_os = "linux")]
        return makepad_hal::stream_encode::hal_stream_encoder_engines().max(1);
        #[cfg(not(target_os = "linux"))]
        return 1;
    }

    /// The largest picture one stream encodes (4:4:4 when `full_chroma`), `None` when not known
    /// (no hardware encoder, or a platform that does not say).
    pub fn max_size(codec: StreamVideoCodec, full_chroma: bool) -> Option<[u32; 2]> {
        #[cfg(target_os = "linux")]
        {
            use makepad_hal::stream_encode as hal;
            let v = hal::hal_stream_encoder_max_size(if full_chroma { hal::FULL_CHROMA } else { 0 } | if codec == StreamVideoCodec::Hevc { hal::HEVC } else { 0 });
            return (v != 0).then(|| [(v >> 32) as u32, v as u32]);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (codec, full_chroma);
            return None;
        }
    }

    /// An encoder of the band `tile` of the shared-texture pictures pushed to it
    /// (`options.height` rows of them). Bands other than the whole picture are Linux's (the
    /// other platforms have one engine).
    pub fn new_tile(options: VideoStreamEncoderOptions, tile: StreamTile) -> Result<Self, VideoFileError> {
        Self::new_tuned(options, tile, StreamTuning::default())
    }

    /// [`Self::new_tile`] with the rate control and quality level of `tuning` (Linux).
    pub fn new_tuned(options: VideoStreamEncoderOptions, tile: StreamTile, tuning: StreamTuning) -> Result<Self, VideoFileError> {
        if tile.x0 % 4 != 0 || tile.y0 % 2 != 0 || tile.y0 + options.height > tile.source_height {
            return Err(VideoFileError::new(format!("tile rows {}..+{} outside a {} tall picture", tile.y0, options.height, tile.source_height)));
        }
        #[cfg(not(target_os = "linux"))]
        if tile.x0 != 0 || tile.y0 != 0 || tile.source_height != options.height || tile.engine != 0 {
            return Err(VideoFileError::new("tiled stream encoding is Linux's"));
        }
        #[cfg(not(target_os = "linux"))]
        if options.full_chroma {
            return Err(VideoFileError::new("4:4:4 stream encoding is Linux's"));
        }
        if options.width == 0 || options.height == 0 || options.width % 2 != 0 || options.height % 2 != 0 {
            return Err(VideoFileError::new(format!(
                "invalid stream encoder frame size {}x{} (must be nonzero and even)",
                options.width, options.height
            )));
        }
        if options.fps == 0 {
            return Err(VideoFileError::new("invalid stream encoder fps 0"));
        }
        #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux", target_os = "android"))]
        {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            let os = OsStreamEncoder::new(&options, tile.engine, tuning)?;
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            let os = {
                let _ = tuning;
                OsStreamEncoder::new(&options)?
            };
            return Ok(Self { options, tile, os });
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "windows", target_os = "linux", target_os = "android")))]
        {
            let _ = (&options, tile, tuning);
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    pub fn options(&self) -> &VideoStreamEncoderOptions {
        &self.options
    }

    /// The band of the pushed pictures this encoder takes.
    pub fn tile(&self) -> StreamTile {
        self.tile
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
        crate::nv12::rgb8_to_nv12(rgb, self.options.width, self.options.height, crate::VideoColor::BT709, &mut nv12);
        self.push_frame_nv12(&nv12, pts_100ns)
    }

    /// Encodes one tightly packed NV12 frame (NV24, full-size interleaved chroma, for a 4:4:4
    /// stream on Linux).
    pub fn push_frame_nv12(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        let expected = if self.options.full_chroma { self.options.width as usize * self.options.height as usize * 3 } else { crate::nv12::nv12_frame_size(self.options.width, self.options.height) };
        if nv12.len() != expected {
            return Err(VideoFileError::new(format!(
                "nv12 frame size {} != expected {expected}",
                nv12.len()
            )));
        }
        #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux", target_os = "android"))]
        return self.os.push_frame_nv12(nv12, pts_100ns);
        #[cfg(not(any(target_vendor = "apple", target_os = "windows", target_os = "linux", target_os = "android")))]
        {
            let _ = pts_100ns;
            return Err(VideoFileError::new(UNSUPPORTED));
        }
    }

    /// Encodes one picture straight from a shared GPU texture holding NV12 packed into BGRA8
    /// texels (`width/4` texels a row, the `height` luma rows then the `height/2` chroma rows:
    /// the layout of [`crate::GpuPictureMaker::nv12_picture`]), once `ready`. `handle` is the
    /// platform's handle of it (Linux: a DMA-BUF registered with
    /// `makepad_hal::dmabuf::hal_dmabuf_register`; Apple: an IOSurface id, whose surface may
    /// also hold a plain NV12 or BGRA picture, as a screen capture's frames do; Windows: the
    /// wire key of a keyed-mutex shared texture holding a BGRA picture,
    /// `makepad_hal::shared_texture`). No pixel crosses the CPU.
    pub fn push_shared_texture(&mut self, handle: u64, ready: GpuReady, pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        #[cfg(target_os = "linux")]
        return self.os.push_shared_texture(handle, ready, pts_100ns, self.tile.source_height, self.tile.x0, self.tile.y0);
        #[cfg(any(target_vendor = "apple", target_os = "windows"))]
        {
            let GpuReady::Done = ready;
            return self.os.push_shared_texture(handle, pts_100ns);
        }
        #[cfg(not(any(target_os = "linux", target_vendor = "apple", target_os = "windows")))]
        {
            let _ = (handle, ready, pts_100ns);
            return Err(VideoFileError::new("stream encoding from a shared texture is not implemented on this platform yet"));
        }
    }

    /// The rate for the pictures from the next one on.
    pub fn set_bitrate_kbps(&mut self, kbps: u32) {
        self.options.bitrate_kbps = kbps;
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple", target_os = "windows"))]
        self.os.set_bitrate_kbps(kbps);
    }

    /// The last shared-texture picture's stages in µs, where the platform splits them (Windows:
    /// copy out of the shared slot, NV12 conversion, encode); zeros elsewhere.
    pub fn stages(&self) -> [u32; 3] {
        #[cfg(target_os = "windows")]
        return self.os.stages();
        #[cfg(not(target_os = "windows"))]
        return [0; 3];
    }

    /// The QP delta map's grid, when the encoder takes one (Linux, with the GPU's quantization
    /// maps): foveated or region-weighted encode with [`Self::set_qp_map`].
    pub fn qp_grid(&self) -> Option<QpGrid> {
        #[cfg(target_os = "linux")]
        return self.os.qp_grid();
        #[cfg(not(target_os = "linux"))]
        return None;
    }

    /// A QP delta per texel of [`Self::qp_grid`] (row by row; negative: more bits there,
    /// positive: fewer) for the pictures from the next one on. False when the encoder takes none
    /// or `deltas` is not the grid's size.
    pub fn set_qp_map(&mut self, deltas: &[i8]) -> bool {
        #[cfg(target_os = "linux")]
        return self.os.set_qp_map(deltas);
        #[cfg(not(target_os = "linux"))]
        {
            let _ = deltas;
            return false;
        }
    }

    /// The last picture's time on the hardware encoder (µs; Linux, from GPU timestamps; 0
    /// elsewhere).
    pub fn last_encode_us(&self) -> u32 {
        #[cfg(target_os = "linux")]
        return self.os.last_gpu_us();
        #[cfg(not(target_os = "linux"))]
        return 0;
    }

    /// What encodes (codec, size, rate, device), for logs and status lines.
    pub fn description(&self) -> String {
        #[cfg(any(target_os = "linux", target_os = "android", target_os = "windows"))]
        return self.os.description();
        #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "windows")))]
        return format!("{:?} {}x{} {} fps {} kbit/s", self.options.codec, self.options.width, self.options.height, self.options.fps, self.options.bitrate_kbps);
    }

    /// Makes the NEXT pushed shared-texture picture a refinement: the same picture as the last
    /// one (a still desktop) again at a near-lossless QP, sharp text and edges at once (Linux; a
    /// no-op elsewhere).
    pub fn request_refinement(&mut self) {
        #[cfg(target_os = "linux")]
        self.os.request_refinement();
    }

    /// Forces the NEXT pushed frame to be a keyframe (SPS/PPS + IDR) — call
    /// this when a new receiver joins a live stream so it can start
    /// decoding immediately instead of waiting for the next scheduled
    /// keyframe (up to `keyint` frames away).
    pub fn request_keyframe(&mut self) {
        #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux", target_os = "android"))]
        self.os.request_keyframe();
    }
}
