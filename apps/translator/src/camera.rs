//! The camera frame the app keeps, as picture pixels.
//!
//! The platform delivers and converts frames (`CameraLatest`: any layout to BGRA
//! on the capture thread; `CameraBgraFrame::rotated`: upright by the camera's
//! reported sensor orientation). This module keeps the newest one and wraps it
//! for the app.

use makepad_widgets::makepad_platform::video::{CameraBgraFrame, CameraLatest};
use std::sync::{Arc, Mutex};

/// The newest frame of the camera in use.
#[derive(Clone)]
pub struct LatestFrame {
    source: Arc<CameraLatest>,
    kept: Arc<Mutex<(u64, Option<CameraBgraFrame>)>>,
}

impl Default for LatestFrame {
    fn default() -> Self {
        LatestFrame { source: CameraLatest::new(), kept: Default::default() }
    }
}

impl LatestFrame {
    /// The callback for `cx.camera_frame_input`.
    pub fn input_fn(&self) -> makepad_widgets::makepad_platform::video::CameraFrameInputFn {
        self.source.input_fn()
    }

    /// Take what arrived since the last call (and remember it); the sequence
    /// number counts frames taken. Cheap when nothing is new.
    pub fn refresh(&self) -> u64 {
        let mut kept = self.kept.lock().unwrap();
        if let Some(frame) = self.source.take() {
            if let Some(old) = kept.1.replace(frame) {
                self.source.recycle(old);
            }
            kept.0 += 1;
        }
        kept.0
    }

    pub fn clear(&self) {
        let mut kept = self.kept.lock().unwrap();
        kept.0 = 0;
        kept.1 = None;
    }

    /// Upright RGBA of the newest frame, every `step`-th pixel, `quarter_turns` clockwise.
    pub fn rgba(&self, step: usize, quarter_turns: usize) -> Option<Rgba> {
        let kept = self.kept.lock().ok()?;
        let frame = kept.1.as_ref()?;
        let turned = frame.rotated(step, quarter_turns);
        let mut pixels = Vec::with_capacity(turned.pixels.len() * 4);
        for p in &turned.pixels {
            pixels.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
        }
        Some(Rgba { width: turned.width, height: turned.height, pixels })
    }
}

pub struct Rgba {
    pub width: usize,
    pub height: usize,
    /// R, G, B, A per pixel, row-major.
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// The pixels as the BGRA words an image texture takes.
    pub fn bgra_words(&self) -> Vec<u32> {
        self.pixels
            .chunks_exact(4)
            .map(|p| (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)
            .collect()
    }
}

