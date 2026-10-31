//! Where the text lines are in a photo: the on-device text detector
//! (PP-OCRv5 mobile detection, run by makepad-ai-textdet on the GPU). It gives
//! the boxes; the vision model reads the words. The two are paired in reading
//! order so each translated line can be laid over its place in the photo.

use crate::reader::{Frame, TextBox};

/// The detector's file under a Makepad home (installed with the Camera model).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub const FILE: &str = "weights/ocr/ppocrv5_mobile_det.gguf";

/// Text-line boxes in `frame` (fractions of the picture, reading order); empty
/// when the detector is not on this phone or fails (the lines then show as a
/// list only).
#[cfg(target_os = "android")]
pub fn lines(frame: &Frame) -> Vec<(f32, f32, f32, f32)> {
    use std::sync::mpsc::{channel, Sender};
    type Request = (Frame, Sender<Vec<(f32, f32, f32, f32)>>);
    // The detector holds GPU state that stays on one thread: a worker owns it.
    static WORKER: std::sync::OnceLock<std::sync::Mutex<Sender<Request>>> = std::sync::OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = channel::<Request>();
        std::thread::spawn(move || {
            let mut detector = None;
            for (frame, reply) in rx {
                let _ = reply.send(detect_on(&mut detector, &frame));
            }
        });
        std::sync::Mutex::new(tx)
    });
    let (reply, answer) = channel();
    let frame = Frame { width: frame.width, height: frame.height, rgba: frame.rgba.clone() };
    if worker.lock().unwrap_or_else(|e| e.into_inner()).send((frame, reply)).is_err() {
        return Vec::new();
    }
    answer.recv().unwrap_or_default()
}

#[cfg(target_os = "android")]
fn detect_on(slot: &mut Option<makepad_ai_textdet::TextDetector>, frame: &Frame) -> Vec<(f32, f32, f32, f32)> {
    use makepad_ai_textdet::{Frame as DetFrame, TextDetector};
    if slot.is_none() {
        let Some(path) = path() else {
            makepad_widgets::log!("translator: no text detector on this phone ({FILE})");
            return Vec::new();
        };
        match TextDetector::load(&path) {
            Ok(d) => *slot = Some(d),
            Err(why) => {
                makepad_widgets::log!("translator: text detector did not load: {why}");
                return Vec::new();
            }
        }
    }
    let detector = slot.as_mut().unwrap();
    match detector.detect(&DetFrame { width: frame.width, height: frame.height, rgba: &frame.rgba }) {
        Ok(boxes) => {
            let t = detector.last_timings();
            makepad_widgets::log!(
                "translator: text detector: {} lines, network {:.0} ms, prepare {:.0} ms, boxes {:.0} ms",
                boxes.len(), t.network_ms, t.prepare_ms, t.boxes_ms
            );
            boxes.into_iter().map(|b| (b.x, b.y, b.w, b.h)).collect()
        }
        Err(why) => {
            makepad_widgets::log!("translator: text detector failed: {why}");
            Vec::new()
        }
    }
}

#[cfg(not(target_os = "android"))]
pub fn lines(_frame: &Frame) -> Vec<(f32, f32, f32, f32)> {
    Vec::new()
}

/// The detector file: under the home that holds the models, or under the
/// external files directory a computer can copy into.
#[cfg(target_os = "android")]
fn path() -> Option<std::path::PathBuf> {
    let mut homes = Vec::new();
    if let Ok(home) = crate::ondevice::activate() {
        homes.push(home);
    }
    if let Some(home) = crate::ondevice::install_home() {
        homes.push(home);
    }
    if let Ok(cmdline) = std::fs::read("/proc/self/cmdline") {
        let name = String::from_utf8_lossy(cmdline.split(|b| *b == 0).next().unwrap_or_default()).into_owned();
        let package = name.split(':').next().unwrap_or_default().to_string();
        homes.push(std::path::Path::new("/sdcard/Android/data").join(package).join("files/makepad"));
    }
    homes.into_iter().map(|h| h.join(FILE)).find(|p| p.is_file())
}

/// Puts the lines the vision model read onto the detected boxes, in reading
/// order. A line without a box gets width 0: it shows in the list only.
pub fn pair(lines: &mut [TextBox], boxes: &[(f32, f32, f32, f32)]) {
    for (i, line) in lines.iter_mut().enumerate() {
        match boxes.get(i) {
            Some(&(x, y, w, h)) => {
                line.x = x;
                line.y = y;
                line.w = w;
                line.h = h;
            }
            None => line.w = 0.0,
        }
    }
}
