//! Continuous window readback: every presented frame of a window, as raw
//! RGBA, on the GPU completion thread.
//!
//! The one-shot cousins of this ride the same wire: `--remote` `/g` grabs,
//! studio screenshots, `Cx::capture_next_frame_to_file` and
//! [`crate::pixel_probe`] all queue a *request* that one pass answers and
//! then forget it. A capture sink is instead standing permission: while one
//! is installed for a window, that window's pass blits its drawable into a
//! shared texture every frame and hands the bytes here — which is what a
//! screen recorder needs and a request queue cannot express.
//!
//! Sinks run on the backend's frame-completion thread (Metal's
//! `addCompletedHandler`, the equivalent elsewhere). Copy the bytes and get
//! off it; do not block, and do not call `add_screen_capture` /
//! `remove_screen_capture` from inside a sink (the registry lock is held).
//!
//! Backend coverage: macOS/Metal, Windows/D3D11, Linux/OpenGL + Vulkan —
//! the same places that can already answer a `/g` grab.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub struct ScreenCaptureFrame<'a> {
    /// The window the presenting pass belongs to, when the backend knows it.
    pub window_id: Option<usize>,
    /// Device pixels, not layout points.
    pub width: u32,
    pub height: u32,
    /// Tightly packed top-down RGBA8, `width * height * 4` bytes.
    pub rgba: &'a [u8],
    /// Monotonic nanoseconds since the first capture-carrying frame of the
    /// process. The recorder's frame clock — wall time, not a frame counter,
    /// so a dropped or duplicated present lands at the time it happened.
    pub time_ns: u64,
}

pub type ScreenCaptureFn = Box<dyn FnMut(&ScreenCaptureFrame) + Send + 'static>;

#[derive(Clone, Copy, Debug)]
pub struct ScreenCaptureOptions {
    /// Capture only this window's passes. `None` takes whichever window
    /// presents — right for a single-window app, wrong for a recorder that
    /// means one specific window.
    pub window_id: Option<usize>,
    /// Upper bound on delivered frames per second. The readback is a full
    /// drawable blit plus a CPU copy, so a 120Hz window recorded at 30 does
    /// a quarter of the work. `0.0` = every presented frame.
    pub max_fps: f64,
}

impl Default for ScreenCaptureOptions {
    fn default() -> Self {
        Self {
            window_id: None,
            max_fps: 0.0,
        }
    }
}

struct Sink {
    window_id: Option<usize>,
    min_interval_ns: u64,
    last_ns: Option<u64>,
    f: ScreenCaptureFn,
}

impl Sink {
    fn matches(&self, window_id: Option<usize>) -> bool {
        match self.window_id {
            None => true,
            Some(want) => window_id == Some(want),
        }
    }
    /// Is this sink owed a frame yet?
    ///
    /// The gate is 3/4 of the interval, not the whole of it. A recorder asking
    /// for exactly the display's refresh gets presents spaced one refresh
    /// apart *give or take jitter*, and a strict `>=` would reject the ones
    /// that land a hair early and take the NEXT one instead — turning a
    /// 60-on-60Hz request into a ragged 30. Three quarters accepts every
    /// present at the asked-for rate while still halving a 120Hz window down
    /// to a 60fps recording.
    fn due(&self, now_ns: u64) -> bool {
        match self.last_ns {
            None => true,
            Some(last) => now_ns.saturating_sub(last) >= self.min_interval_ns / 4 * 3,
        }
    }
}

/// Read before the registry lock so a window with no recorder attached pays
/// one relaxed atomic load per pass.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn sinks() -> &'static Mutex<HashMap<u64, Sink>> {
    static SINKS: OnceLock<Mutex<HashMap<u64, Sink>>> = OnceLock::new();
    SINKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ns() -> u64 {
    (crate::Cx::monotonic_now() * 1_000_000_000.0) as u64
}

pub fn add_screen_capture<F>(options: ScreenCaptureOptions, f: F) -> u64
where
    F: FnMut(&ScreenCaptureFrame) + Send + 'static,
{
    add_screen_capture_box(options, Box::new(f))
}

pub fn add_screen_capture_box(options: ScreenCaptureOptions, f: ScreenCaptureFn) -> u64 {
    let min_interval_ns = if options.max_fps > 0.0 {
        (1_000_000_000.0 / options.max_fps) as u64
    } else {
        0
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut sinks = sinks().lock().unwrap();
    sinks.insert(
        id,
        Sink {
            window_id: options.window_id,
            min_interval_ns,
            last_ns: None,
            f,
        },
    );
    ACTIVE.store(true, Ordering::Release);
    id
}

pub fn remove_screen_capture(id: u64) {
    let mut sinks = sinks().lock().unwrap();
    sinks.remove(&id);
    ACTIVE.store(!sinks.is_empty(), Ordering::Release);
}

/// True while at least one sink is installed, for any window.
pub fn screen_capture_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Should this pass pay for a readback? Called on the render thread while
/// encoding the pass, i.e. one frame BEFORE `deliver_capture_frame` runs.
pub fn capture_wants_window(window_id: Option<usize>) -> bool {
    if !ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let Ok(sinks) = sinks().lock() else {
        return false;
    };
    let now = now_ns();
    sinks
        .values()
        .any(|sink| sink.matches(window_id) && sink.due(now))
}

/// Hand the presented frame's RGBA to every sink that wants it. Called from
/// the backend's frame-completion path.
pub fn deliver_capture_frame(window_id: Option<usize>, width: u32, height: u32, rgba: &[u8]) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
        return;
    }
    let Ok(mut sinks) = sinks().lock() else {
        return;
    };
    let now = now_ns();
    let frame = ScreenCaptureFrame {
        window_id,
        width,
        height,
        rgba,
        time_ns: now,
    };
    for sink in sinks.values_mut() {
        if !sink.matches(window_id) || !sink.due(now) {
            continue;
        }
        sink.last_ns = Some(now);
        (sink.f)(&frame);
    }
}
