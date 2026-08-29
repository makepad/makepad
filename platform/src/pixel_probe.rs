//! Pixel probes: "what colour is the window at this point?" answered from
//! the next presented frame. The design tweaker's eyedropper rides the
//! screenshot pipeline — one request id, one sampled pixel, no PNG.

use crate::cx::Cx;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

struct Probe {
    x: u32,
    y: u32,
    result: Option<[u8; 4]>,
}

fn probes() -> &'static Mutex<HashMap<u64, Probe>> {
    static PROBES: OnceLock<Mutex<HashMap<u64, Probe>>> = OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 40);

impl Cx {
    /// Ask for the colour under device pixel (x, y) of the next frame.
    /// Poll `take_pixel_probe` with the returned id.
    pub fn probe_pixel(&mut self, x: u32, y: u32) -> u64 {
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        probes().lock().unwrap().insert(id, Probe { x, y, result: None });
        self.screenshot_requests
            .push(makepad_studio_protocol::ScreenshotRequest {
                request_id: id,
                kind_id: 0,
            });
        self.redraw_all();
        id
    }
}

/// `Some(Some(rgba))` once the frame has been read back (the entry is
/// consumed), `Some(None)` while pending, `None` for an unknown id.
pub fn take_pixel_probe(id: u64) -> Option<Option<[u8; 4]>> {
    let mut map = probes().lock().unwrap();
    match map.get(&id) {
        None => None,
        Some(p) if p.result.is_none() => Some(None),
        Some(_) => map.remove(&id).map(|p| p.result),
    }
}

/// Called on the readback path with the frame's RGBA. Answers every probe
/// in `request_ids` and removes those ids so no PNG is made for them.
pub(crate) fn answer_pixel_probes(request_ids: &mut Vec<u64>, width: usize, height: usize, rgba: &[u8]) {
    let mut map = probes().lock().unwrap();
    request_ids.retain(|id| {
        let Some(p) = map.get_mut(id) else {
            return true;
        };
        let x = (p.x as usize).min(width.saturating_sub(1));
        let y = (p.y as usize).min(height.saturating_sub(1));
        let o = (y * width + x) * 4;
        if o + 4 <= rgba.len() {
            p.result = Some([rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]);
        } else {
            p.result = Some([0, 0, 0, 0]);
        }
        false
    });
}
