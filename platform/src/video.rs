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

    //TODO make SIMD version of this

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

/// One camera frame as BGRA pixels (the `TextureFormat::VecBGRAu8_32` layout:
/// a `u32` is `a<<24 | r<<16 | g<<8 | b`), whatever layout the device sent.
#[derive(Default)]
pub struct CameraBgraFrame {
    /// The capture time on [`camera_clock_ns`]'s clock (0: unknown).
    pub timestamp_ns: u64,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl CameraBgraFrame {
    /// Fill this frame from `src` (I420, NV12, YUY2 or MJPEG). The pixel
    /// buffer is reused, so a steady stream allocates nothing.
    pub fn convert(&mut self, src: CameraFrameRef<'_>) -> Result<(), String> {
        let (w, h) = (src.width, src.height);
        if w == 0 || h == 0 {
            return Err("empty camera frame".into());
        }
        self.timestamp_ns = src.timestamp_ns;
        match src.layout {
            CameraFrameLayout::Mjpeg => return self.decode_mjpeg(src),
            CameraFrameLayout::I420 | CameraFrameLayout::NV12 | CameraFrameLayout::YUY2 => {}
            CameraFrameLayout::Unknown => return Err("camera frame layout unknown".into()),
        }
        self.width = w;
        self.height = h;
        self.pixels.resize(w * h, 0);
        let coefficients = YuvCoefficients::new(src.matrix);
        let plane = |i: usize| src.planes[i];
        // The byte of `plane` at (row, col), as the plane's strides lay it out.
        let at = |plane: CameraFramePlaneRef<'_>, row: usize, col: usize| -> i32 {
            plane.bytes.get(row * plane.row_stride + col * plane.pixel_stride.max(1)).copied().unwrap_or(128) as i32
        };
        match src.layout {
            CameraFrameLayout::I420 => {
                if src.plane_count < 3 {
                    return Err("I420 frame without three planes".into());
                }
                for row in 0..h {
                    let out = &mut self.pixels[row * w..(row + 1) * w];
                    for (col, pixel) in out.iter_mut().enumerate() {
                        *pixel = coefficients.bgra(at(plane(0), row, col), at(plane(1), row / 2, col / 2), at(plane(2), row / 2, col / 2));
                    }
                }
            }
            CameraFrameLayout::NV12 => {
                if src.plane_count < 2 {
                    return Err("NV12 frame without two planes".into());
                }
                let uv = plane(1);
                let pair = uv.pixel_stride.max(2);
                for row in 0..h {
                    let out = &mut self.pixels[row * w..(row + 1) * w];
                    for (col, pixel) in out.iter_mut().enumerate() {
                        let base = (row / 2) * uv.row_stride + (col / 2) * pair;
                        let u = uv.bytes.get(base).copied().unwrap_or(128) as i32;
                        let v = uv.bytes.get(base + 1).copied().unwrap_or(128) as i32;
                        *pixel = coefficients.bgra(at(plane(0), row, col), u, v);
                    }
                }
            }
            CameraFrameLayout::YUY2 => {
                let packed = plane(0);
                for row in 0..h {
                    let line = row * packed.row_stride;
                    let out = &mut self.pixels[row * w..(row + 1) * w];
                    for (col, pixel) in out.iter_mut().enumerate() {
                        let base = line + (col / 2) * 4;
                        let y = packed.bytes.get(base + (col & 1) * 2).copied().unwrap_or(16) as i32;
                        let u = packed.bytes.get(base + 1).copied().unwrap_or(128) as i32;
                        let v = packed.bytes.get(base + 3).copied().unwrap_or(128) as i32;
                        *pixel = coefficients.bgra(y, u, v);
                    }
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn decode_mjpeg(&mut self, src: CameraFrameRef<'_>) -> Result<(), String> {
        use makepad_zune_jpeg::makepad_zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
        let bytes = src.planes[0].bytes;
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
        let mut decoder = makepad_zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
        decoder.decode_headers().map_err(|error| format!("MJPEG headers: {error:?}"))?;
        let (width, height) = decoder.dimensions().ok_or("MJPEG frame without dimensions")?;
        // A camera frame is the size the device was asked for; anything else
        // is a corrupt frame, not a new size.
        if (src.width, src.height) != (0, 0) && (width, height) != (src.width, src.height) {
            return Err(format!("MJPEG frame is {width}x{height}, the stream is {}x{}", src.width, src.height));
        }
        self.width = width;
        self.height = height;
        self.pixels.resize(width * height, 0);
        // SAFETY: a `u32` slice is a valid `u8` slice of four times the length.
        let out = unsafe { std::slice::from_raw_parts_mut(self.pixels.as_mut_ptr() as *mut u8, self.pixels.len() * 4) };
        decoder.decode_into(out).map_err(|error| format!("MJPEG decode: {error:?}"))
    }
}

impl CameraBgraFrame {
    /// A copy of every `step`-th pixel in each direction, turned clockwise by
    /// `quarter_turns` quarter turns (a camera's `VideoInputDesc::sensor_orientation`
    /// / 90 makes it upright).
    pub fn rotated(&self, step: usize, quarter_turns: usize) -> CameraBgraFrame {
        let step = step.max(1);
        let (sw, sh) = (self.width / step, self.height / step);
        let turns = quarter_turns % 4;
        let (ow, oh) = if turns % 2 == 1 { (sh, sw) } else { (sw, sh) };
        let mut pixels = vec![0u32; ow * oh];
        for sy in 0..sh {
            for sx in 0..sw {
                let p = self.pixels[sy * step * self.width + sx * step];
                let (dx, dy) = match turns {
                    0 => (sx, sy),
                    1 => (sh - 1 - sy, sx),
                    2 => (sw - 1 - sx, sh - 1 - sy),
                    _ => (sy, sw - 1 - sx),
                };
                pixels[dy * ow + dx] = p;
            }
        }
        CameraBgraFrame { timestamp_ns: self.timestamp_ns, width: ow, height: oh, pixels }
    }
}

/// Limited-range YCbCr to RGB in 16.16 fixed point for one colour matrix.
struct YuvCoefficients {
    r_v: i32,
    g_u: i32,
    g_v: i32,
    b_u: i32,
}

impl YuvCoefficients {
    fn new(matrix: CameraColorMatrix) -> Self {
        // (Kr, Kb) of the matrix.
        let (kr, kb): (f64, f64) = match matrix {
            CameraColorMatrix::BT709 => (0.2126, 0.0722),
            CameraColorMatrix::BT2020 => (0.2627, 0.0593),
            CameraColorMatrix::BT601 | CameraColorMatrix::Unknown => (0.299, 0.114),
        };
        let kg = 1.0 - kr - kb;
        let scale = 255.0 / 224.0 * 65536.0;
        Self {
            r_v: (2.0 * (1.0 - kr) * scale).round() as i32,
            g_u: (2.0 * kb * (1.0 - kb) / kg * scale).round() as i32,
            g_v: (2.0 * kr * (1.0 - kr) / kg * scale).round() as i32,
            b_u: (2.0 * (1.0 - kb) * scale).round() as i32,
        }
    }

    #[inline]
    fn bgra(&self, y: i32, u: i32, v: i32) -> u32 {
        let y = ((y - 16) * 298).max(0) + 128;
        let (u, v) = (u - 128, v - 128);
        let r = (y + ((self.r_v * v) >> 8)) >> 8;
        let g = (y - ((self.g_u * u) >> 8) - ((self.g_v * v) >> 8)) >> 8;
        let b = (y + ((self.b_u * u) >> 8)) >> 8;
        0xff00_0000 | (r.clamp(0, 255) as u32) << 16 | (g.clamp(0, 255) as u32) << 8 | b.clamp(0, 255) as u32
    }
}

/// The newest converted frame of one camera: the capture thread publishes
/// (a frame not taken yet is replaced, so a slow consumer never builds a
/// queue), the render thread takes.
pub struct CameraLatest {
    inner: Mutex<CameraLatestInner>,
}

#[derive(Default)]
struct CameraLatestInner {
    latest: Option<CameraBgraFrame>,
    spare: Vec<CameraBgraFrame>,
    received: u64,
    replaced: u64,
    failed: u64,
    last_error: Option<String>,
}

impl CameraLatest {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(CameraLatestInner::default()) })
    }

    /// The callback to install for a camera slot (`camera_frame_input`): it
    /// converts on the capture thread and keeps only the newest frame.
    pub fn input_fn(self: &Arc<Self>) -> CameraFrameInputFn {
        let this = self.clone();
        Box::new(move |src: CameraFrameRef<'_>| {
            let mut frame = this.inner.lock().unwrap().spare.pop().unwrap_or_default();
            let result = frame.convert(src);
            let mut inner = this.inner.lock().unwrap();
            inner.received += 1;
            match result {
                Ok(()) => {
                    if let Some(old) = inner.latest.replace(frame) {
                        inner.replaced += 1;
                        inner.spare.push(old);
                    }
                }
                Err(error) => {
                    inner.failed += 1;
                    inner.last_error = Some(error);
                    inner.spare.push(frame);
                }
            }
            inner.spare.truncate(3);
        })
    }

    /// The newest frame since the last take.
    pub fn take(&self) -> Option<CameraBgraFrame> {
        self.inner.lock().unwrap().latest.take()
    }

    /// Hand a taken frame back, so its pixel buffer is reused.
    pub fn recycle(&self, frame: CameraBgraFrame) {
        let mut inner = self.inner.lock().unwrap();
        if inner.spare.len() < 3 {
            inner.spare.push(frame);
        }
    }

    /// (frames received, frames replaced before anyone took them, frames
    /// that failed to convert, the last conversion error).
    pub fn stats(&self) -> (u64, u64, u64, Option<String>) {
        let inner = self.inner.lock().unwrap();
        (inner.received, inner.replaced, inner.failed, inner.last_error.clone())
    }
}

/// The clock camera frames' `timestamp_ns` is on, now, in nanoseconds
/// (`None` where the platform does not stamp frames on a clock the app can
/// read). A frame's age is this minus its stamp.
/// The display's rotation in quarter turns (Android's Surface.ROTATION_*),
/// kept by the platform as the device turns; 0 where the platform does not
/// rotate the display under the app.
static DISPLAY_ROTATION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn set_display_rotation(quarter_turns: u32) {
    DISPLAY_ROTATION.store(quarter_turns % 4, std::sync::atomic::Ordering::Relaxed);
}

pub fn display_rotation() -> u32 {
    DISPLAY_ROTATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Clockwise quarter turns that make a camera's frames upright on the screen
/// as it is held now: the sensor's orientation against the display's current
/// rotation (a front camera turns the other way; its mirror is the caller's).
/// Recompute per frame: the phone may turn while the camera runs.
pub fn camera_upright_quarter_turns(sensor_orientation: u16, front_facing: bool) -> usize {
    let sensor = (sensor_orientation as u32 / 90) % 4;
    let display = display_rotation();
    (if front_facing { sensor + display } else { sensor + 4 - display } % 4) as usize
}

pub fn camera_clock_ns() -> Option<u64> {
    #[cfg(target_os = "linux")]
    { Some(makepad_hal::clock_ns()) }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

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
}

impl VideoBuffer {
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
    /// Clockwise rotation in degrees (0, 90, 180, 270) that turns this camera's
    /// frames upright for a device held in its natural orientation. 0 for
    /// cameras that deliver upright frames (webcams).
    pub sensor_orientation: u16,
}

#[derive(Clone)]
pub struct VideoInputsEvent {
    pub descs: Vec<VideoInputDesc>,
}

impl VideoInputsEvent {



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

#[cfg(test)]
mod camera_frame_tests {
    use super::*;

    fn plane(bytes: &[u8], row_stride: usize, pixel_stride: usize) -> CameraFramePlaneRef<'_> {
        CameraFramePlaneRef { bytes, row_stride, pixel_stride }
    }

    fn frame<'a>(layout: CameraFrameLayout, planes: [CameraFramePlaneRef<'a>; 3], count: usize) -> CameraFrameRef<'a> {
        CameraFrameRef { timestamp_ns: 7, width: 2, height: 2, layout, matrix: CameraColorMatrix::BT601, plane_count: count, planes }
    }

    fn channels(pixel: u32) -> (u32, u32, u32) {
        ((pixel >> 16) & 255, (pixel >> 8) & 255, pixel & 255)
    }

    #[test]
    fn limited_range_black_and_white_reach_the_ends() {
        let coefficients = YuvCoefficients::new(CameraColorMatrix::BT601);
        assert_eq!(channels(coefficients.bgra(16, 128, 128)), (0, 0, 0));
        assert_eq!(channels(coefficients.bgra(235, 128, 128)), (255, 255, 255));
        assert_eq!(coefficients.bgra(81, 90, 240) >> 24, 255, "alpha is opaque");
        let (r, g, b) = channels(coefficients.bgra(81, 90, 240));
        assert!(r > 240 && g < 20 && b < 20, "BT.601 red is red: {r} {g} {b}");
    }

    #[test]
    fn i420_nv12_and_yuy2_of_the_same_picture_agree() {
        // Left column red-ish, right column blue-ish; chroma is per 2x2.
        let y = [81u8, 41, 81, 41];
        let i420 = frame(CameraFrameLayout::I420, [plane(&y, 2, 1), plane(&[128], 1, 1), plane(&[128], 1, 1)], 3);
        let nv12 = frame(CameraFrameLayout::NV12, [plane(&y, 2, 1), plane(&[128, 128], 2, 2), CameraFramePlaneRef::empty()], 2);
        let yuy2_bytes = [81u8, 128, 41, 128, 81, 128, 41, 128];
        let yuy2 = frame(CameraFrameLayout::YUY2, [plane(&yuy2_bytes, 4, 2), CameraFramePlaneRef::empty(), CameraFramePlaneRef::empty()], 1);
        let mut a = CameraBgraFrame::default();
        let mut b = CameraBgraFrame::default();
        let mut c = CameraBgraFrame::default();
        a.convert(i420).unwrap();
        b.convert(nv12).unwrap();
        c.convert(yuy2).unwrap();
        assert_eq!((a.width, a.height, a.timestamp_ns), (2, 2, 7));
        assert_eq!(a.pixels, b.pixels);
        assert_eq!(a.pixels, c.pixels);
    }

    #[test]
    fn a_frame_nobody_took_is_replaced_by_the_newer_one() {
        let latest = CameraLatest::new();
        let mut input = latest.input_fn();
        let y = [16u8; 4];
        for stamp in [1u64, 2, 3] {
            let mut f = frame(CameraFrameLayout::I420, [plane(&y, 2, 1), plane(&[128], 1, 1), plane(&[128], 1, 1)], 3);
            f.timestamp_ns = stamp;
            input(f);
        }
        let taken = latest.take().expect("a frame");
        assert_eq!(taken.timestamp_ns, 3);
        assert!(latest.take().is_none());
        let (received, replaced, failed, _) = latest.stats();
        assert_eq!((received, replaced, failed), (3, 2, 0));
        latest.recycle(taken);
    }

    #[test]
    fn a_garbage_mjpeg_frame_is_counted_not_shown() {
        let latest = CameraLatest::new();
        let mut input = latest.input_fn();
        let junk = [1u8, 2, 3, 4];
        input(CameraFrameRef { timestamp_ns: 1, width: 2, height: 2, layout: CameraFrameLayout::Mjpeg, matrix: CameraColorMatrix::Unknown, plane_count: 1, planes: [plane(&junk, 4, 1), CameraFramePlaneRef::empty(), CameraFramePlaneRef::empty()] });
        assert!(latest.take().is_none());
        assert_eq!(latest.stats().2, 1);
    }
}

#[cfg(test)]
mod camera_rotation_tests {
    use super::*;

    #[test]
    fn a_quarter_turn_moves_the_top_left_to_the_top_right() {
        let mut f = CameraBgraFrame { timestamp_ns: 0, width: 4, height: 2, pixels: vec![0; 8] };
        f.pixels[0] = 0xffff_ffff;
        let turned = f.rotated(1, 1);
        assert_eq!((turned.width, turned.height), (2, 4));
        assert_eq!(turned.pixels[1], 0xffff_ffff);
        let half = f.rotated(2, 0);
        assert_eq!((half.width, half.height), (2, 1));
    }

    /// Android's back camera, sensor 90 (its top along the device's right edge):
    /// a mark in the sensor image's top-left corner is at the device's top-right,
    /// and must land top-right on screen with the phone upright; held in
    /// landscape (display turned 90) the picture needs no turn at all.
    #[test]
    fn back_camera_marks_land_where_android_says() {
        let w = 4;
        let h = 2;
        let mut f = CameraBgraFrame { timestamp_ns: 0, width: w, height: h, pixels: vec![0; w * h] };
        f.pixels[0] = 0xffff_0000; // the mark, top-left of the sensor image
        set_display_rotation(0);
        let up = f.rotated(1, camera_upright_quarter_turns(90, false));
        assert_eq!((up.width, up.height), (h, w));
        assert_eq!(up.pixels[up.width - 1], 0xffff_0000, "portrait: top-right");
        set_display_rotation(1);
        let land = f.rotated(1, camera_upright_quarter_turns(90, false));
        assert_eq!(land.pixels[0], 0xffff_0000, "landscape (ROTATION_90): unturned");
        set_display_rotation(3);
        let rev = f.rotated(1, camera_upright_quarter_turns(90, false));
        assert_eq!(rev.pixels[w * h - 1], 0xffff_0000, "reverse landscape: a half turn");
        set_display_rotation(0);
    }
}
