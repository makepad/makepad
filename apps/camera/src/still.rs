//! A still from the camera's frames: which camera and format to run, the
//! frame the shutter keeps, and how a kept frame becomes a JPEG.
//!
//! The HAL has no still-capture request yet (no one-shot full-resolution
//! capture beside the preview), so a shot is the first camera frame that
//! arrives after the shutter is pressed, at the stream's resolution. The
//! stream is chosen for that: the largest frame of the camera's own aspect
//! up to [`MAX_STREAM_PIXELS`], at a live frame rate.

use makepad_widgets::makepad_platform::video::{
    CameraBgraFrame, CameraColorMatrix, CameraFrameLayout, CameraFramePlaneRef, CameraFrameRef, VideoFormat, VideoInputDesc,
    VideoPixelFormat,
};

/// The most pixels the running stream may have. Every frame of the stream
/// crosses from the camera to the app, so a full-sensor stream (12 MP on a
/// phone) would copy ~20 MB thirty times a second; 3.2 MP keeps that to
/// what a phone does without noticing and still makes a decent picture.
pub const MAX_STREAM_PIXELS: usize = 3_200_000;

/// The JPEG quality shots are written at.
pub const JPEG_QUALITY: u8 = 90;

/// Whether the app can turn frames of this pixel format into pictures.
pub fn decodable(pixel: VideoPixelFormat) -> bool {
    matches!(pixel, VideoPixelFormat::YUV420 | VideoPixelFormat::NV12 | VideoPixelFormat::YUY2 | VideoPixelFormat::MJPEG)
}

/// The format to run `desc` in: a decodable one at a live rate (24 fps or
/// unknown), then the most pixels up to [`MAX_STREAM_PIXELS`] (above it:
/// the fewest), then the sensor's own 4:3 aspect, then planar YUV over
/// packed over MJPEG. `allow_mjpeg` false leaves the JPEG streams out (a
/// phone's JPEG stream stalls the preview beside it). `None` when the
/// camera offers nothing decodable.
pub fn pick_format(desc: &VideoInputDesc, allow_mjpeg: bool) -> Option<VideoFormat> {
    fn rank(pixel: VideoPixelFormat) -> u8 {
        match pixel {
            VideoPixelFormat::YUV420 | VideoPixelFormat::NV12 => 3,
            VideoPixelFormat::YUY2 => 2,
            _ => 1,
        }
    }
    let key = |f: &VideoFormat| {
        let pixels = f.width * f.height;
        let live = f.frame_rate.map_or(true, |fps| fps >= 23.5);
        let four_three = f.width * 3 == f.height * 4 || f.height * 3 == f.width * 4;
        let fits = pixels <= MAX_STREAM_PIXELS;
        // Within the cap bigger is better; above it smaller is.
        let size = if fits { pixels as i64 } else { -(pixels as i64) };
        (live, fits, size, four_three, rank(f.pixel_format))
    };
    desc.formats
        .iter()
        .filter(|f| decodable(f.pixel_format) && f.width > 0 && f.height > 0)
        .filter(|f| allow_mjpeg || f.pixel_format != VideoPixelFormat::MJPEG)
        .max_by_key(|f| key(f)).copied()
}

/// A front-facing camera (the selfie camera): its frames turn the other
/// way. The phone names its cameras by facing ("Front Camera").
pub fn is_front(desc: &VideoInputDesc) -> bool {
    desc.name.to_ascii_lowercase().contains("front")
}

/// The order the switch walks the cameras in: back cameras first, then the
/// rest as the platform lists them; the app opens on the first.
pub fn camera_order(descs: &[VideoInputDesc]) -> Vec<usize> {
    let back = |d: &VideoInputDesc| d.name.to_ascii_lowercase().contains("back");
    let mut order: Vec<usize> = (0..descs.len()).filter(|&i| back(&descs[i])).collect();
    order.extend((0..descs.len()).filter(|&i| !back(&descs[i])));
    order
}

/// One camera frame kept past its callback: the planes copied with their
/// strides, so any layout the camera sends can be turned into a picture
/// later, off the UI thread.
#[derive(Default)]
pub struct HeldFrame {
    pub timestamp_ns: u64,
    pub width: usize,
    pub height: usize,
    pub layout: CameraFrameLayout,
    pub matrix: CameraColorMatrix,
    planes: Vec<(Vec<u8>, usize, usize)>,
}

impl HeldFrame {
    /// Copy a frame the camera lent to a callback.
    pub fn keep(frame: CameraFrameRef<'_>) -> HeldFrame {
        HeldFrame {
            timestamp_ns: frame.timestamp_ns,
            width: frame.width,
            height: frame.height,
            layout: frame.layout,
            matrix: frame.matrix,
            planes: frame.planes[..frame.plane_count.min(3)]
                .iter()
                .map(|p| (p.bytes.to_vec(), p.row_stride, p.pixel_stride))
                .collect(),
        }
    }

    /// The frame again, as the camera lent it.
    pub fn frame(&self) -> CameraFrameRef<'_> {
        let plane = |i: usize| match self.planes.get(i) {
            Some((bytes, row_stride, pixel_stride)) => CameraFramePlaneRef { bytes, row_stride: *row_stride, pixel_stride: *pixel_stride },
            None => CameraFramePlaneRef::empty(),
        };
        CameraFrameRef {
            timestamp_ns: self.timestamp_ns,
            width: self.width,
            height: self.height,
            layout: self.layout,
            matrix: self.matrix,
            plane_count: self.planes.len(),
            planes: [plane(0), plane(1), plane(2)],
        }
    }
}

/// The picture in a kept frame, upright: `quarter_turns` clockwise (the
/// camera's sensor orientation against the display, `camera_upright_quarter_turns`).
pub fn develop(frame: &HeldFrame, quarter_turns: usize) -> Result<CameraBgraFrame, String> {
    let mut picture = CameraBgraFrame::default();
    picture.convert(frame.frame())?;
    Ok(if quarter_turns % 4 == 0 { picture } else { picture.rotated(1, quarter_turns) })
}

/// A picture as JPEG bytes.
pub fn encode_jpeg(picture: &CameraBgraFrame, quality: u8) -> Result<Vec<u8>, String> {
    let (w, h) = (picture.width, picture.height);
    if w == 0 || h == 0 || w > u16::MAX as usize || h > u16::MAX as usize {
        return Err(format!("a {w}x{h} picture cannot be a JPEG"));
    }
    let mut rgb = Vec::with_capacity(w * h * 3);
    for p in &picture.pixels {
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    let mut out = Vec::with_capacity(w * h / 4);
    jpeg_encoder::Encoder::new(&mut out, quality)
        .encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| format!("JPEG encode: {e}"))?;
    Ok(out)
}

/// A small copy of a picture for the last-shot button: every n-th pixel,
/// at most `max_side` across.
pub fn thumbnail(picture: &CameraBgraFrame, max_side: usize) -> CameraBgraFrame {
    let longest = picture.width.max(picture.height).max(1);
    let step = longest.div_ceil(max_side.max(1)).max(1);
    picture.rotated(step, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::video::{VideoFormatId, VideoInputId};
    use makepad_widgets::makepad_platform::makepad_live_id::LiveId;

    fn format(id: u64, width: usize, height: usize, pixel: VideoPixelFormat, fps: Option<f64>) -> VideoFormat {
        VideoFormat { format_id: VideoFormatId(LiveId(id)), width, height, frame_rate: fps, pixel_format: pixel }
    }

    fn camera(name: &str, formats: Vec<VideoFormat>) -> VideoInputDesc {
        VideoInputDesc { input_id: VideoInputId(LiveId(1)), name: name.into(), formats, sensor_orientation: 90 }
    }

    #[test]
    fn a_phone_runs_the_largest_raw_stream_under_the_cap() {
        // The Pixel's back camera: full sensor YUV, 4:3 and 16:9 sizes, JPEG.
        let back = camera("Back Camera", vec![
            format(1, 4080, 3072, VideoPixelFormat::YUV420, None),
            format(2, 1920, 1080, VideoPixelFormat::YUV420, None),
            format(3, 1920, 1440, VideoPixelFormat::YUV420, None),
            format(4, 1280, 960, VideoPixelFormat::YUV420, None),
            format(5, 4080, 3072, VideoPixelFormat::MJPEG, None),
            format(6, 2048, 1536, VideoPixelFormat::MJPEG, None),
        ]);
        let picked = pick_format(&back, false).unwrap();
        assert_eq!((picked.width, picked.height, picked.pixel_format), (1920, 1440, VideoPixelFormat::YUV420));
        // Where JPEG streams are fine the bigger one under the cap wins.
        let picked = pick_format(&back, true).unwrap();
        assert_eq!((picked.width, picked.height, picked.pixel_format), (2048, 1536, VideoPixelFormat::MJPEG));
        // Same size: 4:3 over 16:9.
        let same = camera("Back Camera", vec![format(1, 1600, 900, VideoPixelFormat::YUV420, None), format(2, 1600, 1200, VideoPixelFormat::YUV420, None), format(3, 1440, 1000, VideoPixelFormat::YUV420, None)]);
        assert_eq!(pick_format(&same, false).unwrap().height, 1200);
    }

    #[test]
    fn a_webcam_runs_live_over_big() {
        let cam = camera("HD Webcam", vec![
            format(1, 1920, 1080, VideoPixelFormat::YUY2, Some(5.0)),
            format(2, 1920, 1080, VideoPixelFormat::MJPEG, Some(30.0)),
            format(3, 640, 480, VideoPixelFormat::YUY2, Some(30.0)),
            format(4, 1280, 720, VideoPixelFormat::RGB24, Some(30.0)),
        ]);
        // The slow 1080p YUY2 loses to the live 1080p MJPEG.
        assert_eq!(pick_format(&cam, true).unwrap().format_id, cam.formats[1].format_id);
        assert_eq!(pick_format(&cam, false).unwrap().format_id, cam.formats[2].format_id);
        assert!(pick_format(&camera("odd", vec![cam.formats[3]]), true).is_none());
        // Only streams above the cap: the smallest of those.
        let huge = camera("Back Camera", vec![format(1, 4080, 3072, VideoPixelFormat::YUV420, None), format(2, 4000, 3000, VideoPixelFormat::YUV420, None)]);
        assert_eq!(pick_format(&huge, false).unwrap().width, 4000);
    }

    #[test]
    fn back_cameras_come_first() {
        let descs = vec![camera("Front Camera", vec![]), camera("Back Camera", vec![]), camera("External Camera", vec![])];
        assert_eq!(camera_order(&descs), vec![1, 0, 2]);
        assert!(is_front(&descs[0]));
        assert!(!is_front(&descs[1]));
    }

    /// An I420 frame of one colour: Y, U and V planes of 2x2 blocks.
    fn i420(width: usize, height: usize, y: u8, u: u8, v: u8) -> HeldFrame {
        let mut frame = HeldFrame { width, height, layout: CameraFrameLayout::I420, matrix: CameraColorMatrix::BT601, ..Default::default() };
        frame.planes = vec![
            (vec![y; width * height], width, 1),
            (vec![u; width / 2 * height / 2], width / 2, 1),
            (vec![v; width / 2 * height / 2], width / 2, 1),
        ];
        frame
    }

    #[test]
    fn a_kept_frame_develops_upright_and_encodes() {
        // Limited-range white and a strong red.
        let white = develop(&i420(8, 4, 235, 128, 128), 1).unwrap();
        assert_eq!((white.width, white.height), (4, 8));
        assert!(white.pixels.iter().all(|p| p & 0xff_ffff == 0xff_ffff));
        let red = develop(&i420(16, 16, 81, 90, 240), 0).unwrap();
        let p = red.pixels[0];
        assert!((p >> 16) & 0xff > 200 && (p >> 8) & 0xff < 60 && p & 0xff < 60, "{p:08x}");
        let jpeg = encode_jpeg(&red, JPEG_QUALITY).unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xff, 0xd9]);
        let thumb = thumbnail(&red, 4);
        assert_eq!((thumb.width, thumb.height), (4, 4));
        // A kept frame is the camera's frame again.
        let held = i420(8, 4, 1, 2, 3);
        let again = held.frame();
        assert_eq!((again.plane_count, again.planes[1].bytes.len(), again.planes[2].bytes[0]), (3, 8, 3));
    }
}
