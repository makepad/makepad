//! Webcam input: the platform camera (NV12 / YUY2 frames off the capture
//! thread) becomes a live preview thumb and, on "snap", a PNG input asset —
//! the same kind of input as a dropped screenshot. `auto-run` keeps snapping
//! and generating with the selected img2X preset while idle.
//!
//! The capture callback runs on the platform's camera thread: it only
//! converts the newest frame into BGRA and parks it in [`WebcamFrames`];
//! the UI thread pulls from there on its own schedule (no allocation churn
//! per frame beyond one buffer swap, no UI work off-thread).

use makepad_widgets::makepad_platform::video::{
    CameraFrameLayout, CameraFrameRef, VideoFormat, VideoInputDesc, VideoPixelFormat,
};
use std::sync::{Arc, Mutex};

/// Largest preview/snapshot edge we keep: snapshots go to image models as
/// PNG; a 1280-wide frame is plenty and keeps the per-frame conversion
/// cheap on the capture thread.
pub const MAX_CAPTURE_WIDTH: usize = 1280;

/// Newest converted frame: BGRA u32 pixels (makepad texture order) + size.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WebcamFrame {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u32>,
    /// Capture-thread frame counter; the UI uses it to skip re-uploading an
    /// unchanged frame.
    pub serial: u64,
}

/// Shared slot between the capture thread and the UI.
#[derive(Clone, Default)]
pub struct WebcamFrames {
    inner: Arc<Mutex<WebcamFrame>>,
}

impl WebcamFrames {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture thread: convert `frame` (NV12/YUY2, any stride) and park it.
    /// Unknown layouts are ignored (no panic on an exotic camera).
    pub fn push(&self, frame: &CameraFrameRef<'_>) {
        let Some((width, height, bgra)) = camera_frame_to_bgra(frame) else {
            return;
        };
        if let Ok(mut slot) = self.inner.lock() {
            slot.width = width;
            slot.height = height;
            slot.bgra = bgra;
            slot.serial += 1;
        }
    }

    /// UI thread: the newest frame if it changed since `seen_serial`.
    pub fn take_newer(&self, seen_serial: u64) -> Option<WebcamFrame> {
        let slot = self.inner.lock().ok()?;
        (slot.serial > seen_serial && !slot.bgra.is_empty()).then(|| slot.clone())
    }

    pub fn latest(&self) -> Option<WebcamFrame> {
        let slot = self.inner.lock().ok()?;
        (!slot.bgra.is_empty()).then(|| slot.clone())
    }
}

/// Pick the camera format to run: NV12 preferred (what the platform
/// delivers natively on Apple), else YUY2; the largest frame whose width
/// fits [`MAX_CAPTURE_WIDTH`], else the smallest available. `None` when the
/// device offers no raw YUV format (MJPEG-only cameras).
pub fn pick_format(desc: &VideoInputDesc) -> Option<&VideoFormat> {
    let rank = |f: &VideoFormat| match f.pixel_format {
        VideoPixelFormat::NV12 => 2,
        VideoPixelFormat::YUY2 => 1,
        _ => 0,
    };
    let candidates: Vec<&VideoFormat> = desc.formats.iter().filter(|f| rank(f) > 0).collect();
    if candidates.is_empty() {
        return None;
    }
    let best_rank = candidates.iter().map(|f| rank(f)).max().unwrap_or(0);
    let same_rank: Vec<&VideoFormat> =
        candidates.into_iter().filter(|f| rank(f) == best_rank).collect();
    same_rank
        .iter()
        .filter(|f| f.width <= MAX_CAPTURE_WIDTH)
        .max_by_key(|f| (f.width * f.height, f.frame_rate.map(|r| r as u64).unwrap_or(0)))
        .or_else(|| same_rank.iter().min_by_key(|f| f.width * f.height))
        .copied()
}

/// NV12 / YUY2 → BGRA u32 (BT.709 video range), honoring row strides.
pub fn camera_frame_to_bgra(frame: &CameraFrameRef<'_>) -> Option<(usize, usize, Vec<u32>)> {
    let (w, h) = (frame.width, frame.height);
    if w == 0 || h == 0 {
        return None;
    }
    let mut out = vec![0u32; w * h];
    match frame.layout {
        CameraFrameLayout::NV12 => {
            let y_plane = &frame.planes[0];
            let uv_plane = &frame.planes[1];
            if y_plane.bytes.len() < y_plane.row_stride * (h - 1) + w
                || uv_plane.bytes.len() < uv_plane.row_stride * (h / 2).max(1).saturating_sub(1) + w
            {
                return None;
            }
            for y in 0..h {
                let yrow = &y_plane.bytes[y * y_plane.row_stride..];
                let uvrow = &uv_plane.bytes[(y / 2) * uv_plane.row_stride..];
                let orow = &mut out[y * w..(y + 1) * w];
                for x in 0..w {
                    let yy = yrow[x];
                    let u = uvrow[(x / 2) * 2];
                    let v = uvrow[(x / 2) * 2 + 1];
                    orow[x] = yuv_to_bgra(yy, u, v);
                }
            }
        }
        CameraFrameLayout::YUY2 => {
            let plane = &frame.planes[0];
            if plane.bytes.len() < plane.row_stride * (h - 1) + w * 2 {
                return None;
            }
            for y in 0..h {
                let row = &plane.bytes[y * plane.row_stride..];
                let orow = &mut out[y * w..(y + 1) * w];
                for x in 0..w {
                    let pair = (x / 2) * 4;
                    let yy = row[pair + (x & 1) * 2];
                    let u = row[pair + 1];
                    let v = row[pair + 3];
                    orow[x] = yuv_to_bgra(yy, u, v);
                }
            }
        }
        _ => return None,
    }
    Some((w, h, out))
}

/// One BT.709 video-range pixel to 0xAARRGGBB.
#[inline]
pub fn yuv_to_bgra(y: u8, u: u8, v: u8) -> u32 {
    let yf = (y as f32 - 16.0) * 1.164_383;
    let cb = u as f32 - 128.0;
    let cr = v as f32 - 128.0;
    let r = yf + 1.792_741 * cr;
    let g = yf - 0.213_249 * cb - 0.532_909 * cr;
    let b = yf + 2.112_402 * cb;
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    0xff00_0000 | (c(r) << 16) | (c(g) << 8) | c(b)
}

/// BGRA u32 → tightly packed RGBA8 (for the PNG encoder).
pub fn bgra_to_rgba8(bgra: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bgra.len() * 4);
    for px in bgra {
        out.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, *px as u8, 0xff]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::video::{
        CameraColorMatrix, CameraFramePlaneRef, VideoFormatId, VideoInputId,
    };

    #[test]
    fn yuv_extremes_map_to_black_white_and_saturated_red() {
        assert_eq!(yuv_to_bgra(16, 128, 128) & 0xffffff, 0x000000);
        assert_eq!(yuv_to_bgra(235, 128, 128) & 0xffffff, 0xffffff);
        let red = yuv_to_bgra(81, 90, 240);
        let (r, g, b) = ((red >> 16) & 0xff, (red >> 8) & 0xff, red & 0xff);
        assert!(r > 200 && g < 60 && b < 60, "{r},{g},{b}");
        assert_eq!(red >> 24, 0xff, "opaque");
    }

    #[test]
    fn nv12_and_yuy2_decode_with_strides() {
        // 4x2 NV12, Y plane stride 8 (padding), UV stride 8.
        let y_bytes = [235u8, 16, 235, 16, 0, 0, 0, 0, 16, 235, 16, 235, 0, 0, 0, 0];
        let uv_bytes = [128u8, 128, 128, 128, 0, 0, 0, 0];
        let frame = CameraFrameRef {
            timestamp_ns: 0,
            width: 4,
            height: 2,
            layout: CameraFrameLayout::NV12,
            matrix: CameraColorMatrix::BT709,
            plane_count: 2,
            planes: [
                CameraFramePlaneRef { bytes: &y_bytes, row_stride: 8, pixel_stride: 1 },
                CameraFramePlaneRef { bytes: &uv_bytes, row_stride: 8, pixel_stride: 2 },
                CameraFramePlaneRef::empty(),
            ],
        };
        let (w, h, px) = camera_frame_to_bgra(&frame).unwrap();
        assert_eq!((w, h), (4, 2));
        assert_eq!(px[0] & 0xffffff, 0xffffff);
        assert_eq!(px[1] & 0xffffff, 0x000000);
        assert_eq!(px[4] & 0xffffff, 0x000000);
        assert_eq!(px[5] & 0xffffff, 0xffffff);

        // 2x1 YUY2: Y0 U Y1 V.
        let packed = [235u8, 128, 16, 128];
        let frame = CameraFrameRef {
            timestamp_ns: 0,
            width: 2,
            height: 1,
            layout: CameraFrameLayout::YUY2,
            matrix: CameraColorMatrix::BT709,
            plane_count: 1,
            planes: [
                CameraFramePlaneRef { bytes: &packed, row_stride: 4, pixel_stride: 2 },
                CameraFramePlaneRef::empty(),
                CameraFramePlaneRef::empty(),
            ],
        };
        let (_, _, px) = camera_frame_to_bgra(&frame).unwrap();
        assert_eq!(px[0] & 0xffffff, 0xffffff);
        assert_eq!(px[1] & 0xffffff, 0x000000);
        // Short buffers are refused, never read past the end.
        let short = CameraFrameRef {
            planes: [
                CameraFramePlaneRef { bytes: &packed[..2], row_stride: 4, pixel_stride: 2 },
                CameraFramePlaneRef::empty(),
                CameraFramePlaneRef::empty(),
            ],
            ..frame
        };
        assert!(camera_frame_to_bgra(&short).is_none());
    }

    #[test]
    fn frames_slot_hands_out_only_newer_frames() {
        let frames = WebcamFrames::new();
        assert!(frames.take_newer(0).is_none());
        let packed = [235u8, 128, 16, 128];
        let frame = CameraFrameRef {
            timestamp_ns: 0,
            width: 2,
            height: 1,
            layout: CameraFrameLayout::YUY2,
            matrix: CameraColorMatrix::BT709,
            plane_count: 1,
            planes: [
                CameraFramePlaneRef { bytes: &packed, row_stride: 4, pixel_stride: 2 },
                CameraFramePlaneRef::empty(),
                CameraFramePlaneRef::empty(),
            ],
        };
        frames.push(&frame);
        let first = frames.take_newer(0).unwrap();
        assert_eq!(first.serial, 1);
        assert!(frames.take_newer(first.serial).is_none());
        frames.push(&frame);
        assert_eq!(frames.take_newer(first.serial).unwrap().serial, 2);
        assert_eq!(bgra_to_rgba8(&first.bgra)[..4], [0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn format_pick_prefers_nv12_within_the_width_cap() {
        let fmt = |id: u64, w: usize, h: usize, pf: VideoPixelFormat| VideoFormat {
            format_id: VideoFormatId(makepad_widgets::LiveId(id)),
            width: w,
            height: h,
            frame_rate: Some(30.0),
            pixel_format: pf,
        };
        let desc = VideoInputDesc {
            input_id: VideoInputId(makepad_widgets::LiveId(1)),
            name: "cam".into(),
            formats: vec![
                fmt(1, 1920, 1080, VideoPixelFormat::NV12),
                fmt(2, 1280, 720, VideoPixelFormat::YUY2),
                fmt(3, 1280, 720, VideoPixelFormat::NV12),
                fmt(4, 640, 480, VideoPixelFormat::NV12),
            ],
        };
        assert_eq!(pick_format(&desc).unwrap().format_id.0, makepad_widgets::LiveId(3));
        let only_big = VideoInputDesc {
            formats: vec![fmt(9, 3840, 2160, VideoPixelFormat::YUY2)],
            ..desc.clone()
        };
        assert_eq!(pick_format(&only_big).unwrap().format_id.0, makepad_widgets::LiveId(9));
        let mjpeg = VideoInputDesc { formats: vec![fmt(5, 640, 480, VideoPixelFormat::MJPEG)], ..desc };
        assert!(pick_format(&mjpeg).is_none());
    }
}
