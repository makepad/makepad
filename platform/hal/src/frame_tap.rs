//! The frame tap: a host-side API that receives a window's finished frames, scaled down to a
//! stream size, as RGBA8 pixels (what a host-embedded Desk Link endpoint encodes and streams).
//! The source is the HAL's own render (the frames handed to a window's presenter or swapchain),
//! never the OS screen.
//!
//! The Vulkan core hands each finished frame image of a tapped window, after its fence, to
//! [`offer`]: with no subscriber that is one atomic load; with one it clones the image's `Arc`
//! into the subscriber's latest-only slot (a newer frame replaces one not copied yet) and wakes
//! its thread. Nothing in the render or queue worker ever waits for a tap. Each subscriber's
//! thread paces itself to its fps, records its own GPU work (`backend::vulkan::tap`: a blit of
//! the region down to the stream size, a copy into host memory, its own fence) and holds the
//! frame's image only until that fence. A window whose scanout image a tap still holds gets
//! another from its scanout pool (the pool reuses only images nobody else holds).
//!
//! Windows a shell renders into its own targets (a WM-hosted window's shared images) are not
//! tapped: those images go back to the host when it releases them, whoever else holds them.

use crate::types::{clock_ns, Id};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which part of the window's frame a tap takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// The whole frame.
    Whole,
    /// Its left half: one eye of a stereo (XR) window, whose eyes are side by side.
    LeftHalf,
}

/// One blit of a tap: from array `layer` of the frame, its `src` rectangle ([x, y, w, h]) into the
/// `dst` rectangle of the stream picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part { pub layer: u32, pub src: [u32; 4], pub dst: [u32; 4] }

/// The blits that make a tap's picture (`size`) of a frame `px` wide and high with `layers` array
/// layers. A stereo window's eyes are side by side in one layer, or (multiview) one layer each:
/// the whole frame is then the layers side by side and the left half is layer 0.
pub fn parts(region: Region, px: [u32; 2], layers: u32, size: [u32; 2]) -> Vec<Part> {
    let [w, h] = px;
    let whole = [0, 0, size[0], size[1]];
    match (region, layers >= 2) {
        (Region::Whole, false) => vec![Part { layer: 0, src: [0, 0, w, h], dst: whole }],
        (Region::LeftHalf, false) => vec![Part { layer: 0, src: [0, 0, w / 2, h], dst: whole }],
        (Region::Whole, true) => {
            let half = size[0] / 2;
            vec![Part { layer: 0, src: [0, 0, w, h], dst: [0, 0, half, size[1]] }, Part { layer: 1, src: [0, 0, w, h], dst: [half, 0, size[0] - half, size[1]] }]
        }
        (Region::LeftHalf, true) => vec![Part { layer: 0, src: [0, 0, w, h], dst: whole }],
    }
}

/// One tapped frame: `width` x `height` RGBA8 (sRGB-encoded, as the window shows it), tightly
/// packed, and when the frame finished on the GPU (HAL clock, ns).
pub struct TapFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub captured_ns: u64,
}

/// A subscription: its frames arrive on `frames` (one waits at most; a consumer that is behind
/// misses frames, the tap never waits for it). Dropping it unsubscribes.
pub struct Tap {
    sub: Arc<Sub>,
    pub frames: Receiver<TapFrame>,
}

/// A slot that keeps only the newest value: `put` replaces one not taken yet (and returns it, to
/// be dropped outside the lock), `take` waits for one.
pub struct Latest<T> {
    slot: Mutex<Option<T>>,
    arrived: Condvar,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Latest { slot: Mutex::new(None), arrived: Condvar::new() }
    }
}

impl<T> Latest<T> {
    pub fn put(&self, value: T) -> Option<T> {
        let old = lock(&self.slot).replace(value);
        self.arrived.notify_all();
        old
    }

    /// The value, waiting up to `wait` for one; `None` when none came (or on `wake`).
    pub fn take(&self, wait: Duration) -> Option<T> {
        let deadline = Instant::now() + wait;
        let mut slot = lock(&self.slot);
        loop {
            if let Some(v) = slot.take() {
                return Some(v);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            slot = self.arrived.wait_timeout(slot, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Wakes a waiting `take` (it returns what is there, or `None`).
    pub fn wake(&self) {
        self.arrived.notify_all();
    }

    /// Drops what is in the slot.
    pub fn clear(&self) -> Option<T> {
        lock(&self.slot).take()
    }
}

type FrameImage = Arc<crate::backend::vulkan::Image>;

struct Sub {
    session: u32,
    window: Id,
    region: Region,
    size: [u32; 2],
    period: Duration,
    slot: Latest<(FrameImage, u64)>,
    stop: AtomicBool,
}

/// How many taps there are: the core's hook looks at nothing else while there are none.
static TAPS: AtomicUsize = AtomicUsize::new(0);
static SUBS: Mutex<Vec<Arc<Sub>>> = Mutex::new(Vec::new());
static NEXT_LOG: AtomicU64 = AtomicU64::new(0);

/// Taps `window` of `session` (the session and window the core presents it as: the host window
/// is `(session::HOST, its id)`): its frames' `region` scaled to `size` (even, at least 2x2),
/// at most `fps` a second. Frames start with the window's next frame.
pub fn subscribe(session: u32, window: Id, region: Region, size: [u32; 2], fps: u32) -> Result<Tap, String> {
    if size[0] < 2 || size[1] < 2 || size[0] % 2 != 0 || size[1] % 2 != 0 {
        return Err(format!("tap size {}x{} must be even and at least 2x2", size[0], size[1]));
    }
    let sub = Arc::new(Sub {
        session,
        window,
        region,
        size,
        period: Duration::from_nanos(1_000_000_000 / fps.clamp(1, 240) as u64),
        slot: Latest::default(),
        stop: AtomicBool::new(false),
    });
    let (tx, frames) = sync_channel(1);
    let thread_sub = sub.clone();
    std::thread::Builder::new()
        .name("hal-frame-tap".into())
        .spawn(move || run(thread_sub, tx))
        .map_err(|e| format!("frame tap thread: {e}"))?;
    lock(&SUBS).push(sub.clone());
    TAPS.fetch_add(1, Ordering::AcqRel);
    Ok(Tap { sub, frames })
}

impl Drop for Tap {
    fn drop(&mut self) {
        let mut subs = lock(&SUBS);
        let before = subs.len();
        subs.retain(|s| !Arc::ptr_eq(s, &self.sub));
        if subs.len() < before {
            TAPS.fetch_sub(1, Ordering::AcqRel);
        }
        drop(subs);
        self.sub.stop.store(true, Ordering::Release);
        // the image a frame not copied yet holds goes back now
        drop(self.sub.slot.clear());
        self.sub.slot.wake();
    }
}

/// Whether a tap wants `window`'s frames (the core then makes sure a frame of it reaches
/// [`offer`] as an image nothing renders into again while it is held).
pub(crate) fn wanted(session: u32, window: Id) -> bool {
    TAPS.load(Ordering::Acquire) != 0 && lock(&SUBS).iter().any(|s| s.session == session && s.window == window)
}

/// A finished frame image of `window` (its GPU work complete, in TRANSFER_SRC_OPTIMAL, never
/// written again while held): into the slot of each tap of it.
pub(crate) fn offer(session: u32, window: Id, image: &FrameImage) {
    if TAPS.load(Ordering::Acquire) == 0 {
        return;
    }
    let now = clock_ns();
    let mut replaced = Vec::new();
    for sub in lock(&SUBS).iter().filter(|s| s.session == session && s.window == window) {
        replaced.extend(sub.slot.put((image.clone(), now)));
    }
    // frames a tap had not copied yet go back outside the lock
    drop(replaced);
}

/// A subscriber's thread: the newest frame, at most one per period, read back and sent.
fn run(sub: Arc<Sub>, tx: SyncSender<TapFrame>) {
    let mut reader: Option<crate::backend::vulkan::tap::Reader> = None;
    let mut next = Instant::now();
    while !sub.stop.load(Ordering::Acquire) {
        let Some((image, captured_ns)) = sub.slot.take(Duration::from_millis(250)) else { continue };
        if sub.stop.load(Ordering::Acquire) {
            break;
        }
        let started = Instant::now();
        if reader.as_ref().is_none_or(|r| !r.on_device(&image.dev)) {
            reader = match crate::backend::vulkan::tap::Reader::new(image.dev.clone(), sub.size) {
                Ok(r) => Some(r),
                Err(e) => {
                    say(&format!("[hal-frame-tap] {e}"));
                    None
                }
            };
        }
        let Some(r) = reader.as_mut() else {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };
        let parts = parts(sub.region, [image.desc.w, image.desc.h], image.desc.depth, sub.size);
        // the image is held until the copy's fence, then given back
        let read = r.read(&image, &parts);
        drop(image);
        match read {
            Ok(rgba) => match tx.try_send(TapFrame { rgba, width: sub.size[0], height: sub.size[1], captured_ns }) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => break,
            },
            Err(e) => {
                say(&format!("[hal-frame-tap] read: {e}"));
                reader = None;
            }
        }
        // the next frame no earlier than a period after this one started
        next = (next + sub.period).max(started);
        let wait = next.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

/// An error line, at most one every few seconds (a tap that cannot read says so, not every frame).
fn say(text: &str) {
    let now = clock_ns();
    let due = NEXT_LOG.load(Ordering::Relaxed);
    if now >= due && NEXT_LOG.compare_exchange(due, now + 5_000_000_000, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        eprintln!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slot_keeps_only_the_newest() {
        let slot = Latest::default();
        assert_eq!(slot.put(1), None);
        assert_eq!(slot.put(2), Some(1), "a frame not taken yet is replaced (and handed back)");
        assert_eq!(slot.take(Duration::ZERO), Some(2));
        assert_eq!(slot.take(Duration::from_millis(5)), None, "nothing new: the take times out");
    }

    #[test]
    fn a_take_waits_for_a_put() {
        let slot = Arc::new(Latest::default());
        let s = slot.clone();
        let t = std::thread::spawn(move || s.take(Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(20));
        slot.put(7u32);
        assert_eq!(t.join().unwrap(), Some(7));
    }

    #[test]
    fn a_cleared_slot_gives_its_value_back() {
        let slot = Latest::default();
        let held = Arc::new(5);
        slot.put(held.clone());
        assert_eq!(Arc::strong_count(&held), 2);
        drop(slot.clear());
        assert_eq!(Arc::strong_count(&held), 1, "the image goes back to its pool");
    }

    #[test]
    fn no_tap_wants_nothing() {
        assert!(!wanted(0, Id { index: 1, gen: 1 }));
        assert!(subscribe(0, Id { index: 1, gen: 1 }, Region::Whole, [3, 2], 30).is_err(), "odd sizes are refused");
    }

    #[test]
    fn multiview_frames_are_tapped_per_layer() {
        // eyes side by side in one layer
        assert_eq!(parts(Region::LeftHalf, [3360, 1760], 1, [1440, 1440]), vec![Part { layer: 0, src: [0, 0, 1680, 1760], dst: [0, 0, 1440, 1440] }]);
        // multiview: one eye per layer
        assert_eq!(parts(Region::LeftHalf, [1680, 1760], 2, [1440, 1440]), vec![Part { layer: 0, src: [0, 0, 1680, 1760], dst: [0, 0, 1440, 1440] }]);
        assert_eq!(parts(Region::Whole, [1680, 1760], 2, [2880, 1440]),
            vec![Part { layer: 0, src: [0, 0, 1680, 1760], dst: [0, 0, 1440, 1440] }, Part { layer: 1, src: [0, 0, 1680, 1760], dst: [1440, 0, 1440, 1440] }]);
    }
}
