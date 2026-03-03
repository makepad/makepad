use crate::makepad_live_id::{FromLiveId, LiveId};

pub type VideoInputFn = Box<dyn FnMut(VideoBufferRef) + Send + 'static>;

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
