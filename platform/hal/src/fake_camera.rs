//! A synthetic camera for tests and headless runs, inert unless `MAKEPAD_HAL_FAKE_CAMERA` names it
//! (`MAKEPAD_HAL_FAKE_CAMERA="OBSBOT Meet 2"`). It is listed like a real camera under that name, with
//! a 1920x1080 and a 1280x720 YUY2 format at 30 fps, and a capture of it runs a thread that makes a
//! moving test picture (a gradient that scrolls and a frame counter bar) 30 times a second, keeping the
//! newest packed frame for the worker to take. Dropping the capture stops the thread and waits for it
//! (the close completes before the drop returns), exactly as a real device is released.

use crate::media::{
    camera_layout, name_hash, pack_camera_frame, CameraFrameData, CameraPlane, VideoFormatInfo, VideoInputInfo, VideoPixel,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

pub const ENV: &str = "MAKEPAD_HAL_FAKE_CAMERA";
const FPS: f64 = 30.0;
const SIZES: [(u32, u32); 2] = [(1920, 1080), (1280, 720)];

/// The synthetic camera as the camera list shows it, when the environment asks for one.
pub fn listed() -> Option<VideoInputInfo> {
    let name = std::env::var(ENV).ok().filter(|name| !name.trim().is_empty())?;
    let formats = SIZES
        .iter()
        .map(|&(width, height)| VideoFormatInfo {
            id: name_hash(&format!("fake {width} {height}")),
            width,
            height,
            frame_rate: Some(FPS),
            pixel: VideoPixel::YUY2,
        })
        .collect();
    Some(VideoInputInfo { id: name_hash(&format!("fake-camera:{name}")), name, sensor_orientation: 0, formats })
}

pub struct FakeCapture {
    name: String,
    format: VideoFormatInfo,
    latest: Arc<Mutex<Option<Vec<u8>>>>,
    made: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    delivered: u64,
}

impl FakeCapture {
    pub fn open(name: &str, format: VideoFormatInfo) -> FakeCapture {
        let latest = Arc::new(Mutex::new(None));
        let made = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (l, m, s, f) = (latest.clone(), made.clone(), stop.clone(), format.clone());
        let thread = std::thread::Builder::new()
            .name("fake-camera".into())
            .spawn(move || {
                let period = std::time::Duration::from_secs_f64(1.0 / FPS);
                let mut next = std::time::Instant::now();
                while !s.load(Ordering::Acquire) {
                    let n = m.fetch_add(1, Ordering::AcqRel);
                    let frame = test_frame(&f, n);
                    *l.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
                    next += period;
                    let now = std::time::Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        next = now;
                    }
                }
            })
            .ok();
        FakeCapture { name: name.to_string(), format, latest, made, stop, thread, delivered: 0 }
    }

    pub fn format(&self) -> &VideoFormatInfo {
        &self.format
    }

    /// The newest frame made since the last call.
    pub fn poll(&mut self) -> Option<Vec<u8>> {
        let frame = self.latest.lock().unwrap_or_else(|e| e.into_inner()).take();
        if frame.is_some() {
            self.delivered += 1;
        }
        frame
    }

    pub fn delivered(&self) -> u64 {
        self.delivered
    }
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        eprintln!("[hal-camera] fake {:?}: closed ({} frames made, {} delivered)", self.name, self.made.load(Ordering::Acquire), self.delivered);
    }
}

/// Frame `n`: luma is a horizontal gradient that scrolls 4 levels a frame, chroma drifts, and the top
/// rows carry `n` in binary (one 16-pixel cell per bit, white for 1) so two frames always differ.
fn test_frame(format: &VideoFormatInfo, n: u64) -> Vec<u8> {
    let (w, h) = (format.width as usize, format.height as usize);
    let mut yuy2 = vec![0u8; w * h * 2];
    for (y, row) in yuy2.chunks_exact_mut(w * 2).enumerate() {
        for (pair, px) in row.chunks_exact_mut(4).enumerate() {
            let x = pair * 2;
            let luma = |x: usize| -> u8 {
                if y < 32 {
                    let bit = x / 16;
                    return if bit < 48 && (n >> bit) & 1 == 1 { 235 } else { 16 };
                }
                ((x * 255 / w) as u64).wrapping_add(n.wrapping_mul(4)) as u8
            };
            px[0] = luma(x);
            px[1] = (128 + (y * 100 / h) as u64 + n % 28) as u8;
            px[2] = luma(x + 1);
            px[3] = (128u64 + 100 - (x * 100 / w) as u64 + n % 28) as u8;
        }
    }
    let timestamp_ns = crate::clock_ns();
    pack_camera_frame(&CameraFrameData {
        timestamp_ns,
        width: w as u32,
        height: h as u32,
        layout: camera_layout::YUY2,
        matrix: 1,
        display_rotation: 0,
        planes: vec![CameraPlane { row_stride: (w * 2) as u32, pixel_stride: 2, bytes: &yuy2 }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_makes_moving_frames_and_stops_when_dropped() {
        let format = VideoFormatInfo { id: 1, width: 64, height: 48, frame_rate: Some(FPS), pixel: VideoPixel::YUY2 };
        let mut cap = FakeCapture::open("test", format);
        let mut frames = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while frames.len() < 3 && std::time::Instant::now() < deadline {
            if let Some(frame) = cap.poll() {
                frames.push(frame);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(frames.len() >= 3, "frames arrive");
        assert_ne!(frames[0], frames[1]);
        drop(cap);
    }
}
