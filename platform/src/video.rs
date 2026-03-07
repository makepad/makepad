use {
    crate::{
        makepad_live_id::{FromLiveId, LiveId},
        texture::TextureId,
    },
    std::sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

pub type VideoInputFn = Box<dyn FnMut(VideoBufferRef) + Send + 'static>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoEncodeError {
    UnsupportedCodec,
    UnsupportedSource,
    CodecUnavailable,
    EncoderNotStarted,
    InvalidTexture,
    UnsupportedTextureFormat,
    InvalidTextureSize,
}

/// Copy a single image plane from a strided source into a tightly-packed destination buffer.
pub fn copy_strided_plane(
    src: CameraFramePlaneRef<'_>,
    width: usize,
    height: usize,
    dst: &mut Vec<u8>,
) {
    dst.resize(width * height, 0);
    if src.bytes.is_empty() || width == 0 || height == 0 {
        return;
    }

    if src.pixel_stride == 1 && src.row_stride == width {
        let n = dst.len().min(src.bytes.len());
        dst[..n].copy_from_slice(&src.bytes[..n]);
        return;
    }

    for row in 0..height {
        let src_row = row.saturating_mul(src.row_stride);
        let dst_row = row * width;
        for col in 0..width {
            let src_idx = src_row + col.saturating_mul(src.pixel_stride.max(1));
            dst[dst_row + col] = src.bytes.get(src_idx).copied().unwrap_or(0);
        }
    }
}

pub fn convert_bgra_8888_to_i420(
    src: &[u8],
    width: usize,
    height: usize,
    timestamp_ns: u64,
    matrix: CameraColorMatrix,
    out: &mut CameraFrameOwned,
) -> bool {
    if width == 0 || height == 0 || src.len() < width.saturating_mul(height).saturating_mul(4) {
        return false;
    }

    out.timestamp_ns = timestamp_ns;
    out.width = width;
    out.height = height;
    out.layout = CameraFrameLayout::I420;
    out.matrix = matrix;
    out.plane_count = 3;

    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);

    out.planes[0].row_stride = width;
    out.planes[0].pixel_stride = 1;
    out.planes[0].bytes.resize(width * height, 16);

    out.planes[1].row_stride = cw;
    out.planes[1].pixel_stride = 1;
    out.planes[1].bytes.resize(cw * ch, 128);

    out.planes[2].row_stride = cw;
    out.planes[2].pixel_stride = 1;
    out.planes[2].bytes.resize(cw * ch, 128);

    #[inline]
    fn clamp_u8(v: i32) -> u8 {
        v.clamp(0, 255) as u8
    }

    #[inline]
    fn rgb_to_yuv(r: i32, g: i32, b: i32) -> (u8, u8, u8) {
        let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
        let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
        let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
        (clamp_u8(y), clamp_u8(u), clamp_u8(v))
    }

    for y in 0..height {
        let row = y * width;
        let src_row = y * width * 4;
        for x in 0..width {
            let i = src_row + x * 4;
            let b = src[i] as i32;
            let g = src[i + 1] as i32;
            let r = src[i + 2] as i32;
            let (yy, _, _) = rgb_to_yuv(r, g, b);
            out.planes[0].bytes[row + x] = yy;
        }
    }

    for by in 0..ch {
        for bx in 0..cw {
            let mut u_acc = 0u32;
            let mut v_acc = 0u32;
            let mut count = 0u32;
            let y0 = by * 2;
            let x0 = bx * 2;
            for oy in 0..2 {
                for ox in 0..2 {
                    let py = y0 + oy;
                    let px = x0 + ox;
                    if py >= height || px >= width {
                        continue;
                    }
                    let i = (py * width + px) * 4;
                    let b = src[i] as i32;
                    let g = src[i + 1] as i32;
                    let r = src[i + 2] as i32;
                    let (_, u, v) = rgb_to_yuv(r, g, b);
                    u_acc += u as u32;
                    v_acc += v as u32;
                    count += 1;
                }
            }
            if count > 0 {
                out.planes[1].bytes[by * cw + bx] = (u_acc / count) as u8;
                out.planes[2].bytes[by * cw + bx] = (v_acc / count) as u8;
            }
        }
    }

    true
}

pub fn convert_rgba_8888_to_i420(
    src: &[u8],
    width: usize,
    height: usize,
    timestamp_ns: u64,
    matrix: CameraColorMatrix,
    out: &mut CameraFrameOwned,
) -> bool {
    if width == 0 || height == 0 || src.len() < width.saturating_mul(height).saturating_mul(4) {
        return false;
    }

    out.timestamp_ns = timestamp_ns;
    out.width = width;
    out.height = height;
    out.layout = CameraFrameLayout::I420;
    out.matrix = matrix;
    out.plane_count = 3;

    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);

    out.planes[0].row_stride = width;
    out.planes[0].pixel_stride = 1;
    out.planes[0].bytes.resize(width * height, 16);

    out.planes[1].row_stride = cw;
    out.planes[1].pixel_stride = 1;
    out.planes[1].bytes.resize(cw * ch, 128);

    out.planes[2].row_stride = cw;
    out.planes[2].pixel_stride = 1;
    out.planes[2].bytes.resize(cw * ch, 128);

    #[inline]
    fn clamp_u8(v: i32) -> u8 {
        v.clamp(0, 255) as u8
    }

    #[inline]
    fn rgb_to_yuv(r: i32, g: i32, b: i32) -> (u8, u8, u8) {
        let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
        let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
        let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
        (clamp_u8(y), clamp_u8(u), clamp_u8(v))
    }

    for y in 0..height {
        let row = y * width;
        let src_row = y * width * 4;
        for x in 0..width {
            let i = src_row + x * 4;
            let r = src[i] as i32;
            let g = src[i + 1] as i32;
            let b = src[i + 2] as i32;
            let (yy, _, _) = rgb_to_yuv(r, g, b);
            out.planes[0].bytes[row + x] = yy;
        }
    }

    for by in 0..ch {
        for bx in 0..cw {
            let mut u_acc = 0u32;
            let mut v_acc = 0u32;
            let mut count = 0u32;
            let y0 = by * 2;
            let x0 = bx * 2;
            for oy in 0..2 {
                for ox in 0..2 {
                    let py = y0 + oy;
                    let px = x0 + ox;
                    if py >= height || px >= width {
                        continue;
                    }
                    let i = (py * width + px) * 4;
                    let r = src[i] as i32;
                    let g = src[i + 1] as i32;
                    let b = src[i + 2] as i32;
                    let (_, u, v) = rgb_to_yuv(r, g, b);
                    u_acc += u as u32;
                    v_acc += v as u32;
                    count += 1;
                }
            }
            if count > 0 {
                out.planes[1].bytes[by * cw + bx] = (u_acc / count) as u8;
                out.planes[2].bytes[by * cw + bx] = (v_acc / count) as u8;
            }
        }
    }

    true
}

pub const MAX_VIDEO_DEVICE_INDEX: usize = 32;

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct VideoInputId(pub LiveId);

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct VideoFormatId(pub LiveId);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VideoPixelFormat {
    RGB24,
    YUY2,
    NV12,
    YUV420,
    GRAY,
    MJPEG,
    Unsupported(u32),
}

impl VideoPixelFormat {
    fn quality_priority(&self) -> usize {
        match self {
            Self::RGB24 => 6,
            Self::YUY2 => 5,
            Self::NV12 => 4,
            Self::YUV420 => 3,
            Self::MJPEG => 2,
            Self::GRAY => 1,
            Self::Unsupported(_) => 0,
        }
    }

    //TODO make SIMD version of this
    pub fn buffer_to_bgra_32(
        &self,
        input: &[u32],
        width: usize,
        height: usize,
        rgba: &mut Vec<u32>,
    ) {
        fn yuv_to_rgb(y: i32, u: i32, v: i32) -> u32 {
            fn clip(a: i32) -> u32 {
                if a < 0 {
                    return 0;
                }
                if a > 255 {
                    return 255;
                }
                return a as u32;
            }
            let c = y as i32 - 16;
            let d = v as i32 - 128;
            let e = u as i32 - 128;
            return (clip((298 * c + 516 * d + 128) >> 8) << 16)
                | (clip((298 * c - 100 * d - 208 * e + 128) >> 8) << 8)
                | (clip((298 * c + 409 * e + 128) >> 8) << 0)
                | (255 << 24);
        }

        match self {
            Self::NV12 => {
                rgba.resize(width * height, 0u32);
                for y in 0..height {
                    for x in (0..width).step_by(2) {
                        let d = input[y * (width >> 1) + (x >> 1)];
                        let y1 = (d >> 16) & 0xff;
                        let y2 = (d >> 0) & 0xff;
                        let u = (d >> 8) & 0xff;
                        let v = (d >> 24) & 0xff;
                        rgba[y * width + x] = yuv_to_rgb(y1 as i32, u as i32, v as i32);
                        rgba[y * width + x + 1] = yuv_to_rgb(y2 as i32, u as i32, v as i32);
                    }
                }
            }
            _ => {
                crate::error!("convert to bgra not supported");
            }
        }
    }

    pub fn buffer_to_rgb_8(
        &self,
        input: &[u32],
        rgb: &mut Vec<u8>,
        in_width: usize,
        _in_height: usize,
        left: usize,
        top: usize,
        out_width: usize,
        out_height: usize,
    ) {
        fn yuv_to_rgb(y: i32, u: i32, v: i32) -> (u8, u8, u8) {
            fn clip(a: i32) -> u32 {
                if a < 0 {
                    return 0;
                }
                if a > 255 {
                    return 255;
                }
                return a as u32;
            }
            let c = y as i32 - 16;
            let d = v as i32 - 128;
            let e = u as i32 - 128;
            let r = clip((298 * c + 516 * d + 128) >> 8) as u8;
            let g = clip((298 * c - 100 * d - 208 * e + 128) >> 8) as u8;
            let b = clip((298 * c + 409 * e + 128) >> 8) as u8;
            (r, g, b)
        }

        match self {
            Self::NV12 => {
                rgb.clear();
                rgb.reserve(out_width * out_height * 3);
                for y in top..top + out_height {
                    for x in (left..left + out_width).step_by(2) {
                        let d = input[y * (in_width >> 1) + (x >> 1)];
                        let y1 = (d >> 16) & 0xff;
                        let y2 = (d >> 0) & 0xff;
                        let u = (d >> 8) & 0xff;
                        let v = (d >> 24) & 0xff;
                        let (r, g, b) = yuv_to_rgb(y1 as i32, u as i32, v as i32);
                        rgb.push(r);
                        rgb.push(g);
                        rgb.push(b);
                        let (r, g, b) = yuv_to_rgb(y2 as i32, u as i32, v as i32);
                        rgb.push(r);
                        rgb.push(g);
                        rgb.push(b);
                    }
                }
            }
            _ => {
                crate::error!("convert to bgra not supported");
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CameraFrameLayout {
    I420,
    NV12,
    YUY2,
    Mjpeg,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CameraColorMatrix {
    BT709,
    BT601,
    BT2020,
    #[default]
    Unknown,
}

impl CameraColorMatrix {
    pub fn as_yuv_uniform(self) -> f32 {
        match self {
            Self::BT709 => 0.0,
            Self::BT601 => 1.0,
            Self::BT2020 => 2.0,
            Self::Unknown => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CameraFramePlaneRef<'a> {
    pub bytes: &'a [u8],
    pub row_stride: usize,
    pub pixel_stride: usize,
}

impl<'a> CameraFramePlaneRef<'a> {
    pub fn empty() -> Self {
        Self {
            bytes: &[],
            row_stride: 0,
            pixel_stride: 1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CameraFrameRef<'a> {
    pub timestamp_ns: u64,
    pub width: usize,
    pub height: usize,
    pub layout: CameraFrameLayout,
    pub matrix: CameraColorMatrix,
    pub plane_count: usize,
    pub planes: [CameraFramePlaneRef<'a>; 3],
}

impl<'a> CameraFrameRef<'a> {
    pub fn empty() -> Self {
        Self {
            timestamp_ns: 0,
            width: 0,
            height: 0,
            layout: CameraFrameLayout::Unknown,
            matrix: CameraColorMatrix::Unknown,
            plane_count: 0,
            planes: [
                CameraFramePlaneRef::empty(),
                CameraFramePlaneRef::empty(),
                CameraFramePlaneRef::empty(),
            ],
        }
    }
}

pub type CameraFrameInputFn = Box<dyn for<'a> FnMut(CameraFrameRef<'a>) + Send + 'static>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Vp8,
    Vp9,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoBitstreamFormat {
    AnnexB,
    Avcc,
    Av1Obu,
    RawAccessUnit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoQueuePolicy {
    /// Keep latency bounded by keeping only the newest frames.
    LatestWins,
}

impl Default for VideoQueuePolicy {
    fn default() -> Self {
        Self::LatestWins
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoEncodeSource {
    Camera {
        input_id: VideoInputId,
        format_id: VideoFormatId,
    },
    Texture {
        texture_id: TextureId,
    },
    CpuFrames {
        layout: CameraFrameLayout,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct VideoEncoderConfig {
    pub codec: VideoCodec,
    pub source: VideoEncodeSource,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub target_bitrate: u32,
    pub keyint: i32,
    pub latency_realtime: bool,
    /// Backend-specific quality/speed hint.
    ///
    /// `makepad-platform` currently uses this for AV1 software encode mode.
    pub codec_mode: i32,
    pub queue_policy: VideoQueuePolicy,
    pub queue_capacity: usize,
}

impl Default for VideoEncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::Av1,
            source: VideoEncodeSource::CpuFrames {
                layout: CameraFrameLayout::I420,
            },
            width: 1280,
            height: 720,
            fps_num: 30,
            fps_den: 1,
            target_bitrate: 2_000_000,
            keyint: 120,
            latency_realtime: true,
            codec_mode: 8,
            queue_policy: VideoQueuePolicy::LatestWins,
            queue_capacity: 2,
        }
    }
}

/// Encoded elementary packet emitted by realtime encoder sessions.
///
/// Semantics:
/// - `is_config`: codec parameter payload (SPS/PPS/VPS/sequence-header) that
///   applies to `config_id`.
/// - `is_key`: independently decodable key access unit for current `config_id`.
/// - `config_id`: monotonic generation identifier; increment when codec
///   configuration changes.
#[derive(Clone, Copy, Debug)]
pub struct EncodedVideoPacketRef<'a> {
    pub codec: VideoCodec,
    pub format: VideoBitstreamFormat,
    pub pts_ns: u64,
    pub dts_ns: Option<u64>,
    pub is_key: bool,
    pub is_config: bool,
    pub is_eos: bool,
    pub config_id: u32,
    pub data: &'a [u8],
}

#[derive(Clone, Debug)]
pub struct EncodedVideoPacketOwned {
    pub codec: VideoCodec,
    pub format: VideoBitstreamFormat,
    pub pts_ns: u64,
    pub dts_ns: Option<u64>,
    pub is_key: bool,
    pub is_config: bool,
    pub is_eos: bool,
    pub config_id: u32,
    pub data: Vec<u8>,
}

impl Default for EncodedVideoPacketOwned {
    fn default() -> Self {
        Self {
            codec: VideoCodec::Av1,
            format: VideoBitstreamFormat::Av1Obu,
            pts_ns: 0,
            dts_ns: None,
            is_key: false,
            is_config: false,
            is_eos: false,
            config_id: 0,
            data: Vec::new(),
        }
    }
}

pub type VideoOutputFn = Box<dyn for<'a> FnMut(EncodedVideoPacketRef<'a>) + Send + 'static>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoDecodeOutput {
    Texture {
        texture_id: TextureId,
    },
    YuvPlanes {
        tex_y: TextureId,
        tex_u: TextureId,
        tex_v: TextureId,
    },
    CpuFrames,
}

#[derive(Clone, Copy, Debug)]
pub struct VideoDecoderConfig {
    pub codec: VideoCodec,
    pub expected_format: VideoBitstreamFormat,
    pub output: VideoDecodeOutput,
    pub width_hint: Option<u32>,
    pub height_hint: Option<u32>,
    pub latency_realtime: bool,
}

/// Encoded packet input for decoder sessions.
///
/// Decoder contract:
/// - decoder must receive config payloads for a `config_id` before dependent
///   non-config packets
/// - `is_key` marks random-access recovery points
/// - a changed `config_id` indicates stream reconfiguration
#[derive(Clone, Copy, Debug)]
pub struct VideoDecoderPacketRef<'a> {
    pub pts_ns: u64,
    pub dts_ns: Option<u64>,
    pub is_key: bool,
    pub is_config: bool,
    pub config_id: u32,
    pub data: &'a [u8],
}

pub type VideoDecodedFrameOutputFn = Box<dyn for<'a> FnMut(CameraFrameRef<'a>) + Send + 'static>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoDecodeError {
    UnsupportedCodec,
    UnsupportedOutput,
    DecoderNotStarted,
    InvalidPacket,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoCodecSupport {
    pub codec: VideoCodec,
    pub encode_hardware: bool,
    pub encode_software: bool,
    pub decode_hardware: bool,
    pub decode_software: bool,
    pub encode_formats: Vec<VideoBitstreamFormat>,
    pub decode_formats: Vec<VideoBitstreamFormat>,
    pub supports_camera_source: bool,
    pub supports_texture_source: bool,
    pub supports_cpu_frames_source: bool,
    pub supports_keyframe_request: bool,
    pub supports_dynamic_resolution: bool,
    pub width_alignment: Option<u32>,
    pub height_alignment: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub max_fps: Option<u32>,
    pub max_bitrate: Option<u32>,
}

impl VideoCodecSupport {
    pub fn unsupported(codec: VideoCodec) -> Self {
        Self {
            codec,
            encode_hardware: false,
            encode_software: false,
            decode_hardware: false,
            decode_software: false,
            encode_formats: Vec::new(),
            decode_formats: Vec::new(),
            supports_camera_source: false,
            supports_texture_source: false,
            supports_cpu_frames_source: false,
            supports_keyframe_request: false,
            supports_dynamic_resolution: false,
            width_alignment: None,
            height_alignment: None,
            max_width: None,
            max_height: None,
            max_fps: None,
            max_bitrate: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VideoCapabilities {
    pub codecs: Vec<VideoCodecSupport>,
}

#[derive(Default)]
pub struct CameraFramePlaneOwned {
    pub bytes: Vec<u8>,
    pub row_stride: usize,
    pub pixel_stride: usize,
}

#[derive(Default)]
pub struct CameraFrameOwned {
    pub timestamp_ns: u64,
    pub width: usize,
    pub height: usize,
    pub layout: CameraFrameLayout,
    pub matrix: CameraColorMatrix,
    pub plane_count: usize,
    pub planes: [CameraFramePlaneOwned; 3],
}

impl CameraFrameOwned {
    pub fn reset(&mut self) {
        self.timestamp_ns = 0;
        self.width = 0;
        self.height = 0;
        self.layout = CameraFrameLayout::Unknown;
        self.matrix = CameraColorMatrix::Unknown;
        self.plane_count = 0;
        for plane in &mut self.planes {
            plane.row_stride = 0;
            plane.pixel_stride = 1;
            plane.bytes.clear();
        }
    }

    pub fn copy_from_ref(&mut self, src: CameraFrameRef<'_>) {
        self.timestamp_ns = src.timestamp_ns;
        self.width = src.width;
        self.height = src.height;
        self.layout = src.layout;
        self.matrix = src.matrix;
        self.plane_count = src.plane_count.min(3);

        for i in 0..self.plane_count {
            let src_plane = src.planes[i];
            let (plane_w, plane_h) = self.plane_size(i);
            let dst_plane = &mut self.planes[i];
            dst_plane.row_stride = plane_w;
            dst_plane.pixel_stride = 1;
            dst_plane.bytes.resize(plane_w * plane_h, 0);

            if src_plane.bytes.is_empty() || plane_w == 0 || plane_h == 0 {
                continue;
            }

            if src_plane.pixel_stride == 1 && src_plane.row_stride == plane_w {
                let max_copy = dst_plane.bytes.len().min(src_plane.bytes.len());
                dst_plane.bytes[..max_copy].copy_from_slice(&src_plane.bytes[..max_copy]);
                continue;
            }

            for row in 0..plane_h {
                let src_row_start = row.saturating_mul(src_plane.row_stride);
                let dst_row_start = row * plane_w;
                for col in 0..plane_w {
                    let src_idx = src_row_start + col.saturating_mul(src_plane.pixel_stride.max(1));
                    dst_plane.bytes[dst_row_start + col] =
                        src_plane.bytes.get(src_idx).copied().unwrap_or(0);
                }
            }
        }
    }

    /// Convert any supported camera frame layout to I420 (three separate R8
    /// planes: Y full-res, U/V quarter-res).  Returns `false` if conversion
    /// is not possible (unknown layout, empty source, etc.).
    pub fn convert_to_i420(&mut self, src: CameraFrameRef<'_>) -> bool {
        let w = src.width;
        let h = src.height;
        if w == 0 || h == 0 {
            return false;
        }

        self.timestamp_ns = src.timestamp_ns;
        self.width = w;
        self.height = h;
        self.layout = CameraFrameLayout::I420;
        self.matrix = src.matrix;
        self.plane_count = 3;
        self.planes[0].row_stride = w;
        self.planes[0].pixel_stride = 1;
        self.planes[1].row_stride = w.div_ceil(2);
        self.planes[1].pixel_stride = 1;
        self.planes[2].row_stride = w.div_ceil(2);
        self.planes[2].pixel_stride = 1;

        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);

        match src.layout {
            CameraFrameLayout::I420 => {
                if src.plane_count < 3 {
                    return false;
                }
                copy_strided_plane(src.planes[0], w, h, &mut self.planes[0].bytes);
                copy_strided_plane(src.planes[1], cw, ch, &mut self.planes[1].bytes);
                copy_strided_plane(src.planes[2], cw, ch, &mut self.planes[2].bytes);
                true
            }
            CameraFrameLayout::NV12 => {
                if src.plane_count < 2 {
                    return false;
                }
                copy_strided_plane(src.planes[0], w, h, &mut self.planes[0].bytes);

                self.planes[1].bytes.resize(cw * ch, 128);
                self.planes[2].bytes.resize(cw * ch, 128);
                let uv = src.planes[1];
                for row in 0..ch {
                    let src_row = row.saturating_mul(uv.row_stride);
                    let dst_row = row * cw;
                    for col in 0..cw {
                        let base = src_row + col.saturating_mul(uv.pixel_stride.max(2));
                        self.planes[1].bytes[dst_row + col] =
                            uv.bytes.get(base).copied().unwrap_or(128);
                        self.planes[2].bytes[dst_row + col] =
                            uv.bytes.get(base + 1).copied().unwrap_or(128);
                    }
                }
                true
            }
            CameraFrameLayout::YUY2 => {
                if src.plane_count < 1 {
                    return false;
                }
                let packed = src.planes[0];
                self.planes[0].bytes.resize(w * h, 16);
                self.planes[1].bytes.resize(cw * ch, 128);
                self.planes[2].bytes.resize(cw * ch, 128);

                let half_w = w.div_ceil(2);
                let mut u_full = vec![128u8; half_w * h];
                let mut v_full = vec![128u8; half_w * h];

                for row in 0..h {
                    let src_row = row.saturating_mul(packed.row_stride);
                    for pair in 0..half_w {
                        let base = src_row + pair * 4;
                        let y0 = packed.bytes.get(base).copied().unwrap_or(16);
                        let u = packed.bytes.get(base + 1).copied().unwrap_or(128);
                        let y1 = packed.bytes.get(base + 2).copied().unwrap_or(16);
                        let v = packed.bytes.get(base + 3).copied().unwrap_or(128);

                        let px0 = row * w + pair * 2;
                        if px0 < self.planes[0].bytes.len() {
                            self.planes[0].bytes[px0] = y0;
                        }
                        let px1 = px0 + 1;
                        if px1 < self.planes[0].bytes.len() {
                            self.planes[0].bytes[px1] = y1;
                        }

                        u_full[row * half_w + pair] = u;
                        v_full[row * half_w + pair] = v;
                    }
                }

                for row in 0..ch {
                    let r0 = row * 2;
                    let r1 = (r0 + 1).min(h - 1);
                    for col in 0..cw {
                        let u0 = u_full[r0 * half_w + col.min(half_w - 1)] as u16;
                        let u1 = u_full[r1 * half_w + col.min(half_w - 1)] as u16;
                        self.planes[1].bytes[row * cw + col] = ((u0 + u1 + 1) / 2) as u8;

                        let v0 = v_full[r0 * half_w + col.min(half_w - 1)] as u16;
                        let v1 = v_full[r1 * half_w + col.min(half_w - 1)] as u16;
                        self.planes[2].bytes[row * cw + col] = ((v0 + v1 + 1) / 2) as u8;
                    }
                }
                true
            }
            _ => false,
        }
    }

    pub fn plane_size(&self, plane_index: usize) -> (usize, usize) {
        if self.width == 0 || self.height == 0 {
            return (0, 0);
        }
        match self.layout {
            CameraFrameLayout::I420 | CameraFrameLayout::NV12 => {
                if plane_index == 0 {
                    (self.width, self.height)
                } else {
                    (self.width.div_ceil(2), self.height.div_ceil(2))
                }
            }
            CameraFrameLayout::YUY2 => {
                if plane_index == 0 {
                    (self.width, self.height)
                } else {
                    (0, 0)
                }
            }
            CameraFrameLayout::Mjpeg | CameraFrameLayout::Unknown => {
                if plane_index == 0 {
                    (self.width, self.height)
                } else {
                    (0, 0)
                }
            }
        }
    }
}

pub struct CameraFramePool {
    free: Vec<CameraFrameOwned>,
    latest: Option<CameraFrameOwned>,
    max_free: usize,
}

impl CameraFramePool {
    pub fn new(max_free: usize) -> Self {
        Self {
            free: Vec::new(),
            latest: None,
            max_free,
        }
    }

    pub fn checkout(&mut self) -> CameraFrameOwned {
        self.free.pop().unwrap_or_default()
    }

    pub fn publish_latest(&mut self, frame: CameraFrameOwned) {
        if let Some(old) = self.latest.replace(frame) {
            self.recycle(old);
        }
    }

    pub fn take_latest(&mut self) -> Option<CameraFrameOwned> {
        self.latest.take()
    }

    pub fn recycle(&mut self, mut frame: CameraFrameOwned) {
        frame.reset();
        if self.free.len() < self.max_free {
            self.free.push(frame);
        }
    }
}

/// Fixed-size latest-frame ring for camera preview paths.
///
/// - Producer: camera callback thread.
/// - Consumer: UI/render thread.
/// - Semantics: latest-wins; stale frames are dropped.
/// - Bounded memory: slot_count preallocated frame storage.
pub struct CameraFrameRing {
    slots: Vec<Mutex<CameraFrameOwned>>,
    next_write_slot: AtomicUsize,
    latest_seq: AtomicU64,
}

impl CameraFrameRing {
    pub fn new(slot_count: usize) -> Self {
        let slot_count = slot_count.max(2);
        let mut slots = Vec::with_capacity(slot_count);
        for _ in 0..slot_count {
            slots.push(Mutex::new(CameraFrameOwned::default()));
        }
        Self {
            slots,
            next_write_slot: AtomicUsize::new(0),
            latest_seq: AtomicU64::new(0),
        }
    }

    pub fn publish_i420_copy(&self, frame_ref: CameraFrameRef<'_>) -> bool {
        if frame_ref.layout != CameraFrameLayout::I420 || frame_ref.plane_count < 3 {
            return false;
        }
        self.publish_with(frame_ref, |slot, src| {
            slot.copy_from_ref(src);
            true
        })
    }

    pub fn publish_i420_converted(&self, frame_ref: CameraFrameRef<'_>) -> bool {
        self.publish_with(frame_ref, |slot, src| slot.convert_to_i420(src))
    }

    fn take_latest(&self, last_seen_seq: &mut u64, out: &mut CameraFrameOwned) -> bool {
        let seq = self.latest_seq.load(Ordering::Acquire);
        if seq == 0 || seq == *last_seen_seq {
            return false;
        }

        let idx = ((seq - 1) as usize) % self.slots.len();
        let mut slot = match self.slots[idx].try_lock() {
            Ok(slot) => slot,
            Err(_) => return false,
        };

        // If producer advanced while we were acquiring the slot lock,
        // skip this poll and let the next poll read the latest slot.
        if self.latest_seq.load(Ordering::Acquire) != seq {
            return false;
        }

        std::mem::swap(out, &mut *slot);
        *last_seen_seq = seq;
        true
    }

    fn publish_with(
        &self,
        frame_ref: CameraFrameRef<'_>,
        write_frame: impl FnOnce(&mut CameraFrameOwned, CameraFrameRef<'_>) -> bool,
    ) -> bool {
        let idx = self.next_write_slot.fetch_add(1, Ordering::Relaxed) % self.slots.len();
        let mut slot = match self.slots[idx].try_lock() {
            Ok(slot) => slot,
            Err(_) => return false,
        };

        if !write_frame(&mut slot, frame_ref) {
            return false;
        }

        self.latest_seq.fetch_add(1, Ordering::Release);
        true
    }
}

/// Consumer-side latest-frame view over a [`CameraFrameRing`].
/// Stores cursor and pending frame state so platform players keep one field.
pub struct CameraFrameLatest {
    ring: Arc<CameraFrameRing>,
    frame: CameraFrameOwned,
    last_seen_seq: u64,
    has_pending: bool,
}

impl CameraFrameLatest {
    pub fn new(slot_count: usize) -> Self {
        Self {
            ring: Arc::new(CameraFrameRing::new(slot_count)),
            frame: CameraFrameOwned::default(),
            last_seen_seq: 0,
            has_pending: false,
        }
    }

    pub fn ring(&self) -> Arc<CameraFrameRing> {
        self.ring.clone()
    }

    pub fn prime_pending_from_latest(&mut self) -> bool {
        if self
            .ring
            .take_latest(&mut self.last_seen_seq, &mut self.frame)
        {
            self.has_pending = true;
            true
        } else {
            false
        }
    }

    pub fn take_pending_or_latest(&mut self) -> Option<&CameraFrameOwned> {
        if !self.has_pending && !self.prime_pending_from_latest() {
            return None;
        }
        self.has_pending = false;
        Some(&self.frame)
    }

    pub fn pending_frame(&self) -> Option<&CameraFrameOwned> {
        if self.has_pending {
            Some(&self.frame)
        } else {
            None
        }
    }
}

pub enum VideoBufferRefData<'a> {
    U8(&'a [u8]),
    U32(&'a [u32]),
}

pub struct VideoBufferRef<'a> {
    pub format: VideoFormat,
    pub data: VideoBufferRefData<'a>,
}

impl<'a> VideoBufferRef<'a> {
    pub fn to_buffer(&self) -> VideoBuffer {
        VideoBuffer {
            format: self.format.clone(),
            data: match self.data {
                VideoBufferRefData::U8(data) => VideoBufferData::U8(data.to_vec()),
                VideoBufferRefData::U32(data) => VideoBufferData::U32(data.to_vec()),
            },
        }
    }

    pub fn as_slice_u32(&mut self) -> Option<&[u32]> {
        match &mut self.data {
            VideoBufferRefData::U32(v) => return Some(v),
            _ => return None,
        }
    }
    pub fn as_slice_u8(&mut self) -> Option<&[u8]> {
        match &mut self.data {
            VideoBufferRefData::U8(v) => return Some(v),
            _ => return None,
        }
    }
}

pub enum VideoBufferData {
    U8(Vec<u8>),
    U32(Vec<u32>),
}

pub struct VideoBuffer {
    pub format: VideoFormat,
    pub data: VideoBufferData,
}

impl VideoBuffer {
    pub fn as_vec_u32(&mut self) -> Option<&mut Vec<u32>> {
        match &mut self.data {
            VideoBufferData::U32(v) => return Some(v),
            _ => return None,
        }
    }
    pub fn as_vec_u8(&mut self) -> Option<&mut Vec<u8>> {
        match &mut self.data {
            VideoBufferData::U8(v) => return Some(v),
            _ => return None,
        }
    }
}

impl VideoBuffer {
    pub fn into_vec_u32(self) -> Option<Vec<u32>> {
        match self.data {
            VideoBufferData::U32(v) => return Some(v),
            _ => return None,
        }
    }
    pub fn into_vec_u8(self) -> Option<Vec<u8>> {
        match self.data {
            VideoBufferData::U8(v) => return Some(v),
            _ => return None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VideoFormat {
    pub format_id: VideoFormatId,
    pub width: usize,
    pub height: usize,
    pub frame_rate: Option<f64>,
    pub pixel_format: VideoPixelFormat,
}

#[derive(Clone, Debug)]
pub struct VideoInputDesc {
    pub input_id: VideoInputId,
    pub name: String,
    pub formats: Vec<VideoFormat>,
}

#[derive(Clone)]
pub struct VideoInputsEvent {
    pub descs: Vec<VideoInputDesc>,
}

impl VideoInputsEvent {
    pub fn find_device(&self, name: &str) -> usize {
        if let Some(position) = self.descs.iter().position(|v| v.name == name) {
            return position;
        }
        return 0;
    }

    pub fn find_highest(&self, device_index: usize) -> Vec<(VideoInputId, VideoFormatId)> {
        if let Some(device) = self.descs.get(device_index) {
            let mut max_pixels = 0;
            let mut max_frame_rate = 0.0;
            let mut max_quality = 0;
            let mut format_id = None;
            for format in &device.formats {
                let pixels = format.width * format.height;
                if pixels >= max_pixels {
                    max_pixels = pixels
                }
            }
            for format in &device.formats {
                if let Some(frame_rate) = format.frame_rate {
                    let pixels = format.width * format.height;
                    if pixels == max_pixels && frame_rate >= max_frame_rate {
                        max_frame_rate = frame_rate;
                    }
                }
            }
            for format in &device.formats {
                let pixels = format.width * format.height;
                let quality = format.pixel_format.quality_priority();
                if pixels == max_pixels
                    && format.frame_rate.unwrap_or(0.0) == max_frame_rate
                    && quality >= max_quality
                {
                    max_quality = quality;
                    format_id = Some(format.format_id)
                }
            }
            if let Some(format_id) = format_id {
                return vec![(device.input_id, format_id)];
            }
        }
        vec![]
    }

    pub fn find_highest_at_res(
        &self,
        device_index: usize,
        width: usize,
        height: usize,
        max_fps: f64,
    ) -> Vec<(VideoInputId, VideoFormatId)> {
        if let Some(device) = self.descs.get(device_index) {
            let mut max_frame_rate = 0.0;
            let mut max_quality = 0;
            let mut format_id = None;

            for format in &device.formats {
                if let Some(frame_rate) = format.frame_rate {
                    if width == format.width
                        && height == format.height
                        && frame_rate >= max_frame_rate
                        && frame_rate <= max_fps
                    {
                        max_frame_rate = frame_rate;
                    }
                }
            }
            for format in &device.formats {
                let quality = format.pixel_format.quality_priority();
                if width == format.width
                    && height == format.height
                    && format.frame_rate.unwrap_or(0.0) == max_frame_rate
                    && quality >= max_quality
                {
                    max_quality = quality;
                    format_id = Some(format.format_id)
                }
            }
            if let Some(format_id) = format_id {
                return vec![(device.input_id, format_id)];
            }
        }
        vec![]
    }

    pub fn find_format(
        &self,
        device_index: usize,
        width: usize,
        height: usize,
        pixel_format: VideoPixelFormat,
    ) -> Vec<(VideoInputId, VideoFormatId)> {
        if let Some(device) = self.descs.get(device_index) {
            let mut max_frame_rate = 0.0;
            let mut format_id = None;

            for format in &device.formats {
                if let Some(frame_rate) = format.frame_rate {
                    if format.pixel_format == pixel_format
                        && width == format.width
                        && height == format.height
                        && frame_rate >= max_frame_rate
                    {
                        max_frame_rate = frame_rate;
                    }
                }
            }
            for format in &device.formats {
                if format.pixel_format == pixel_format
                    && width == format.width
                    && height == format.height
                    && format.frame_rate.unwrap_or(0.0) == max_frame_rate
                {
                    format_id = Some(format.format_id)
                }
            }
            if let Some(format_id) = format_id {
                return vec![(device.input_id, format_id)];
            }
        }
        vec![]
    }
}

impl std::fmt::Debug for VideoInputsEvent {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        for desc in &self.descs {
            write!(f, "Capture Device: {}\n", desc.name).unwrap();
            for format in &desc.formats {
                write!(
                    f,
                    "    format: w:{} h:{} framerate:{:?} pixel:{:?} \n",
                    format.width, format.height, format.frame_rate, format.pixel_format
                )
                .unwrap();
            }
        }
        Ok(())
    }
}
