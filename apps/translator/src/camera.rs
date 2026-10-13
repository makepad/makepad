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

/// A PNG as RGBA (the camera test picture goes through the same Rgba as a frame).
pub fn decode_png_rgba(bytes: &[u8]) -> Result<Rgba, String> {
    use makepad_zune_core::options::DecoderOptions;
    use makepad_zune_png::PngDecoder;
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    decoder.decode_headers().map_err(|e| format!("png: {e:?}"))?;
    let info = decoder.info().cloned().ok_or("png: no header")?;
    let comps = decoder.colorspace().ok_or("png: no colorspace")?.num_components();
    let raw = decoder.decode_raw().map_err(|e| format!("png: {e:?}"))?;
    let n = info.width * info.height;
    if comps == 0 || raw.len() < n * comps {
        return Err("png: short data".into());
    }
    let mut pixels = vec![255u8; n * 4];
    for i in 0..n {
        let s = &raw[i * comps..i * comps + comps];
        let (r, g, b) = if comps >= 3 { (s[0], s[1], s[2]) } else { (s[0], s[0], s[0]) };
        pixels[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, if comps == 4 { s[3] } else { 255 }]);
    }
    Ok(Rgba { width: info.width, height: info.height, pixels })
}

/// The camera test picture: a Chinese menu (resources/fixtures/menu.png).
pub const TEST_PICTURE: &[u8] = include_bytes!("../resources/fixtures/menu.png");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_picture_decodes() {
        let p = decode_png_rgba(TEST_PICTURE).unwrap();
        assert_eq!((p.width, p.height), (810, 1080));
        assert_eq!(p.pixels.len(), 810 * 1080 * 4);
    }
}
