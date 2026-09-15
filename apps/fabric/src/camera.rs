use makepad_widgets::{
    makepad_platform::video::{
        CameraFrameLayout, CameraFrameRef, VideoFormatId, VideoInputId, VideoInputsEvent,
        VideoPixelFormat,
    },
    Cx, CxMediaApi,
};
use std::{
    cmp::Reverse,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub const SEND_MAX_WIDTH: usize = 640;
const PREVIEW_MAX_WIDTH: usize = 320;
const PREVIEW_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq)]
pub struct CameraRgbFrame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub serial: u64,
}

#[derive(Default)]
struct PreviewSlot {
    frame: Option<CameraRgbFrame>,
    updated_at: Option<Instant>,
}

#[derive(Default)]
struct CameraMailboxInner {
    want: AtomicBool,
    serial: AtomicU64,
    frame: Mutex<Option<CameraRgbFrame>>,
    model_size: Mutex<Option<(u32, u32)>>,
    preview: Mutex<PreviewSlot>,
}

/// A one-frame handoff from the camera callback to the model worker. The
/// callback only converts a model-sized frame after the worker asks for one;
/// the smaller preview is independent and limited to ten updates per second.
#[derive(Clone, Default)]
pub struct CameraMailbox {
    inner: Arc<CameraMailboxInner>,
}

impl CameraMailbox {
    pub fn request(&self) {
        if let Ok(mut frame) = self.inner.frame.lock() {
            *frame = None;
        }
        self.inner.want.store(true, Ordering::Release);
    }

    pub fn take(&self) -> Option<CameraRgbFrame> {
        self.inner.frame.lock().ok()?.take()
    }

    pub fn peek_preview(&self) -> Option<CameraRgbFrame> {
        self.inner.preview.lock().ok()?.frame.clone()
    }

    pub fn model_size(&self) -> Option<(u32, u32)> {
        *self.inner.model_size.lock().ok()?
    }

    fn capture(&self, frame: &CameraFrameRef<'_>) {
        let wanted = self.inner.want.swap(false, Ordering::AcqRel);
        let preview_due = self
            .inner
            .preview
            .lock()
            .ok()
            .map(|slot| {
                slot.updated_at
                    .map(|updated| updated.elapsed() >= PREVIEW_INTERVAL)
                    .unwrap_or(true)
            })
            .unwrap_or(false);
        if !wanted && !preview_due {
            return;
        }

        if wanted {
            if let Some((rgb, width, height)) = frame_to_rgb(frame) {
                let serial = self.inner.serial.fetch_add(1, Ordering::Relaxed) + 1;
                if let Ok(mut size) = self.inner.model_size.lock() {
                    *size = Some((width, height));
                }
                if let Ok(mut slot) = self.inner.frame.lock() {
                    *slot = Some(CameraRgbFrame {
                        width,
                        height,
                        rgb,
                        serial,
                    });
                }
            }
        }

        if preview_due {
            if let Some((rgb, width, height)) = frame_to_rgb_max(frame, PREVIEW_MAX_WIDTH) {
                let serial = self.inner.serial.fetch_add(1, Ordering::Relaxed) + 1;
                if let Ok(mut slot) = self.inner.preview.lock() {
                    slot.frame = Some(CameraRgbFrame {
                        width,
                        height,
                        rgb,
                        serial,
                    });
                    slot.updated_at = Some(Instant::now());
                }
            }
        }
    }
}

/// Register the capture callback. Calling this also asks the platform to
/// enumerate cameras, which produces `Event::VideoInputs` on the UI thread.
pub fn install_camera(cx: &mut Cx, mailbox: CameraMailbox) {
    cx.camera_frame_input(0, move |frame| mailbox.capture(&frame));
}

/// Choose the first device's smallest raw-YUV format at least 640x360.
pub fn pick_camera(event: &VideoInputsEvent) -> Option<(VideoInputId, VideoFormatId)> {
    let device = event.descs.first()?;
    let format = device
        .formats
        .iter()
        .filter(|format| {
            format.width >= 640
                && format.height >= 360
                && matches!(
                    format.pixel_format,
                    VideoPixelFormat::NV12 | VideoPixelFormat::YUY2
                )
        })
        .min_by_key(|format| {
            (
                format.width.saturating_mul(format.height),
                if format.pixel_format == VideoPixelFormat::NV12 {
                    0
                } else {
                    1
                },
                Reverse((format.frame_rate.unwrap_or(0.0) * 1000.0) as u64),
            )
        })?;
    Some((device.input_id, format.format_id))
}

/// Convert NV12 or YUY2 to packed RGB8, using an integer sampling step so
/// the entire frame fits within [`SEND_MAX_WIDTH`].
pub fn frame_to_rgb(frame: &CameraFrameRef<'_>) -> Option<(Vec<u8>, u32, u32)> {
    frame_to_rgb_max(frame, SEND_MAX_WIDTH)
}

fn frame_to_rgb_max(
    frame: &CameraFrameRef<'_>,
    max_width: usize,
) -> Option<(Vec<u8>, u32, u32)> {
    let (width, height) = (frame.width, frame.height);
    if width == 0 || height == 0 || max_width == 0 {
        return None;
    }
    let step = width.div_ceil(max_width).max(1);
    let out_width = width.div_ceil(step);
    let out_height = height.div_ceil(step);
    let mut rgb = Vec::with_capacity(out_width.checked_mul(out_height)?.checked_mul(3)?);

    for out_y in 0..out_height {
        let source_y = (out_y * step).min(height - 1);
        for out_x in 0..out_width {
            let source_x = (out_x * step).min(width - 1);
            let (y, u, v) = match frame.layout {
                CameraFrameLayout::NV12 => nv12_pixel(frame, source_x, source_y)?,
                CameraFrameLayout::YUY2 => yuy2_pixel(frame, source_x, source_y)?,
                _ => return None,
            };
            rgb.extend_from_slice(&yuv_to_rgb(y, u, v));
        }
    }

    Some((
        rgb,
        u32::try_from(out_width).ok()?,
        u32::try_from(out_height).ok()?,
    ))
}

fn nv12_pixel(frame: &CameraFrameRef<'_>, x: usize, y: usize) -> Option<(u8, u8, u8)> {
    if frame.plane_count < 2 {
        return None;
    }
    let y_plane = frame.planes[0];
    let uv_plane = frame.planes[1];
    let y_index = y
        .checked_mul(y_plane.row_stride)?
        .checked_add(x.checked_mul(y_plane.pixel_stride)?)?;
    let uv_index = (y / 2)
        .checked_mul(uv_plane.row_stride)?
        .checked_add((x / 2).checked_mul(uv_plane.pixel_stride)?)?;
    Some((
        *y_plane.bytes.get(y_index)?,
        *uv_plane.bytes.get(uv_index)?,
        *uv_plane.bytes.get(uv_index + 1)?,
    ))
}

fn yuy2_pixel(frame: &CameraFrameRef<'_>, x: usize, y: usize) -> Option<(u8, u8, u8)> {
    if frame.plane_count < 1 {
        return None;
    }
    let plane = frame.planes[0];
    let pixel_stride = plane.pixel_stride.max(2);
    let pair = y
        .checked_mul(plane.row_stride)?
        .checked_add((x / 2).checked_mul(pixel_stride.checked_mul(2)?)?)?;
    Some((
        *plane.bytes.get(pair + (x & 1) * pixel_stride)?,
        *plane.bytes.get(pair + 1)?,
        *plane.bytes.get(pair + pixel_stride + 1)?,
    ))
}

/// One BT.709, video-range YUV pixel to RGB8.
pub(crate) fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = i32::from(y) - 16;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let clip = |value: i32| ((value + 128) >> 8).clamp(0, 255) as u8;
    [
        clip(298 * c + 459 * e),
        clip(298 * c - 55 * d - 136 * e),
        clip(298 * c + 541 * d),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::video::{
        CameraColorMatrix, CameraFramePlaneRef,
    };

    #[test]
    fn yuv_grey_red_and_blue() {
        let grey = yuv_to_rgb(126, 128, 128);
        assert!(grey.iter().all(|channel| (126i16 - i16::from(*channel)).abs() <= 2));

        let red = yuv_to_rgb(81, 90, 240);
        assert!(red[0] > 220 && red[1] < 60 && red[2] < 60, "{red:?}");

        let blue = yuv_to_rgb(41, 240, 110);
        assert!(blue[2] > 220 && blue[0] < 60 && blue[1] < 60, "{blue:?}");
    }

    #[test]
    fn nv12_integer_downsample_keeps_the_whole_frame() {
        const WIDTH: usize = 64;
        const HEIGHT: usize = 32;
        let mut y_plane = vec![0u8; WIDTH * HEIGHT];
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                y_plane[y * WIDTH + x] = 16 + ((x + y) % 200) as u8;
            }
        }
        let uv_plane = vec![128u8; WIDTH * HEIGHT / 2];
        let frame = CameraFrameRef {
            timestamp_ns: 0,
            width: WIDTH,
            height: HEIGHT,
            layout: CameraFrameLayout::NV12,
            matrix: CameraColorMatrix::BT709,
            plane_count: 2,
            planes: [
                CameraFramePlaneRef {
                    bytes: &y_plane,
                    row_stride: WIDTH,
                    pixel_stride: 1,
                },
                CameraFramePlaneRef {
                    bytes: &uv_plane,
                    row_stride: WIDTH,
                    pixel_stride: 2,
                },
                CameraFramePlaneRef::empty(),
            ],
        };

        let (rgb, width, height) = frame_to_rgb_max(&frame, 16).unwrap();
        assert_eq!((width, height), (16, 8));
        assert_eq!(rgb.len(), 16 * 8 * 3);
        assert_eq!(&rgb[..3], &yuv_to_rgb(y_plane[0], 128, 128));
        let last_source = y_plane[28 * WIDTH + 60];
        assert_eq!(&rgb[rgb.len() - 3..], &yuv_to_rgb(last_source, 128, 128));
    }
}
