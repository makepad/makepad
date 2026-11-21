//! The box's own camera. While no guest is live the big screen shows it,
//! redrawn with the current prompt like a guest's camera (the live thread's
//! `Live::box_camera`). This picks the first camera with a format it can
//! read and, on the capture thread, hands the party's [`CameraFeed`] a
//! frame converted to RGB at the redraw's size whenever the live thread
//! asks for one: a few times a second, as fast as the hub redraws. Nothing
//! of it runs on the UI thread per frame.

use makepad_app_party::image::{self, Rgb};
use makepad_app_party::live::CameraFeed;
use makepad_widgets::makepad_platform::{
    permission::{Permission, PermissionStatus},
    video::{CameraFrameLayout, CameraFrameRef, VideoFormat, VideoFormatId, VideoInputDesc, VideoInputId, VideoPixelFormat},
};
use makepad_widgets::*;
use std::sync::Arc;

#[derive(Default)]
pub struct BoxCamera {
    /// The camera and format picked, once the box listed its cameras.
    source: Option<(VideoInputId, VideoFormatId)>,
    permitted: bool,
    /// What capture was started for.
    started: Option<(VideoInputId, VideoFormatId)>,
}

impl BoxCamera {
    /// Picks the camera and starts its capture into `feed` (the running
    /// party's) once the box lists its cameras and grants the permission.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, feed: Option<&Arc<CameraFeed>>) {
        match event {
            Event::Startup => {
                cx.request_permission(Permission::Camera);
                match cx.video_inputs() {
                    Some(inputs) => self.select(&inputs),
                    None => cx.use_video_input(&[]),
                }
            }
            Event::VideoInputs(ev) => self.select(&ev.descs),
            Event::PermissionResult(ev) if ev.permission == Permission::Camera => {
                self.permitted = ev.status == PermissionStatus::Granted;
                if !self.permitted {
                    log!("party: the box camera is not permitted: the screen plays the loops");
                }
            }
            _ => return,
        }
        self.start(cx, feed);
    }

    fn select(&mut self, inputs: &[VideoInputDesc]) {
        let picked = inputs.iter().find_map(|desc| Some((desc, pick_format(desc)?)));
        match picked {
            Some((desc, f)) => {
                let fps = f.frame_rate.map_or_else(|| "unknown".to_owned(), |r| format!("{r:.0}"));
                log!("party: the box camera: {} at {}x{} {:?}, {} fps", desc.name, f.width, f.height, f.pixel_format, fps);
                self.source = Some((desc.input_id, f.format_id));
            }
            None => {
                log!("party: no camera on this box ({} inputs): the screen plays the loops", inputs.len());
                self.source = None;
            }
        }
    }

    fn start(&mut self, cx: &mut Cx, feed: Option<&Arc<CameraFeed>>) {
        let (Some(source), Some(feed), true) = (self.source, feed, self.permitted) else { return };
        if self.started == Some(source) {
            return;
        }
        self.started = Some(source);
        cx.use_video_input(&[source]);
        let feed = feed.clone();
        cx.camera_frame_input(0, move |frame: CameraFrameRef<'_>| {
            if let Some(size) = feed.seen(frame.width as u32, frame.height as u32) {
                feed.give(to_rgb(&frame, size));
            }
        });
    }
}

/// The format for the redraw (512 px on the long side): the camera's own
/// aspect (its largest format's), 15 fps or more, at least 640 wide, the
/// smallest such, uncompressed before MJPEG.
fn pick_format(desc: &VideoInputDesc) -> Option<&VideoFormat> {
    let readable = |f: &&VideoFormat| {
        f.width > 0
            && f.height > 0
            && matches!(f.pixel_format, VideoPixelFormat::NV12 | VideoPixelFormat::YUY2 | VideoPixelFormat::YUV420 | VideoPixelFormat::MJPEG)
    };
    let largest = desc.formats.iter().filter(readable).max_by_key(|f| f.width * f.height)?;
    desc.formats.iter().filter(readable).max_by_key(|f| {
        let native = f.width * largest.height == f.height * largest.width;
        let fast = f.frame_rate.is_some_and(|r| r >= 15.0);
        let raw = f.pixel_format != VideoPixelFormat::MJPEG;
        (native, fast, f.width >= 640, raw, std::cmp::Reverse(f.width * f.height))
    })
}

/// `frame` as RGB at `size`, sampled nearest (the redraw repaints it).
fn to_rgb(frame: &CameraFrameRef<'_>, (w, h): (u32, u32)) -> Option<Rgb> {
    let (w, h, fw, fh) = (w as usize, h as usize, frame.width, frame.height);
    if fw == 0 || fh == 0 {
        return None;
    }
    let p = &frame.planes;
    if frame.layout == CameraFrameLayout::Mjpeg {
        return image::decode_jpeg(p[0].bytes).ok().map(|rgb| image::resize(&rgb, w, h));
    }
    let stride = |i: usize, least: usize| p[i].row_stride.max(least);
    let mut data = Vec::with_capacity(w * h * 3);
    for ty in 0..h {
        let sy = ty * fh / h;
        for tx in 0..w {
            let sx = tx * fw / w;
            let (y, u, v) = match frame.layout {
                CameraFrameLayout::YUY2 => {
                    let at = sy * stride(0, fw * 2) + (sx & !1) * 2;
                    let b = p[0].bytes.get(at..at + 4)?;
                    (b[(sx & 1) * 2], b[1], b[3])
                }
                CameraFrameLayout::NV12 => {
                    let uv = (sy / 2) * stride(1, fw) + (sx & !1);
                    (*p[0].bytes.get(sy * stride(0, fw) + sx)?, *p[1].bytes.get(uv)?, *p[1].bytes.get(uv + 1)?)
                }
                CameraFrameLayout::I420 => {
                    let c = |i: usize| (sy / 2) * stride(i, fw.div_ceil(2)) + sx / 2;
                    (*p[0].bytes.get(sy * stride(0, fw) + sx)?, *p[1].bytes.get(c(1))?, *p[2].bytes.get(c(2))?)
                }
                _ => return None,
            };
            data.extend_from_slice(&yuv_to_rgb(y, u, v));
        }
    }
    Some(Rgb { width: w, height: h, data })
}

/// BT.601, limited range: what USB cameras send.
fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (c, d, e) = (298 * (y as i32 - 16), u as i32 - 128, v as i32 - 128);
    let q = |x: i32| ((x + 128) >> 8).clamp(0, 255) as u8;
    [q(c + 409 * e), q(c - 100 * d - 208 * e), q(c + 516 * d)]
}
