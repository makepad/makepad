//! Audio-output tap: a read-only fork of everything this app writes to its
//! audio output device.
//!
//! The app's own output callback (`CxMediaApi::audio_output`) fills a buffer
//! the device then plays. A tap is invoked with that same buffer right after
//! the app has filled it, so a recorder gets exactly what the speakers get —
//! no OS loopback device, no screen-recording permission, no other app's
//! sound. `platform/src/os/apple/audio_tap.rs` is the *other* thing: a
//! ScreenCaptureKit loopback of the whole machine.
//!
//! Taps run on the realtime audio thread. A tap body must not block, must
//! not allocate more than it has to, and must never call back into
//! `add_audio_output_tap` / `remove_audio_output_tap` (the registry lock is
//! held while it runs). Copy into a channel and get off the thread.

use crate::audio::{AudioBuffer, AudioInfo};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub type AudioOutputTapFn = Box<dyn FnMut(AudioInfo, &AudioBuffer) + Send + 'static>;

/// Checked before the registry lock so an app with no tap installed pays one
/// relaxed atomic load per audio callback.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn taps() -> &'static Mutex<HashMap<u64, AudioOutputTapFn>> {
    static TAPS: OnceLock<Mutex<HashMap<u64, AudioOutputTapFn>>> = OnceLock::new();
    TAPS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Install a tap on the app's audio output. Returns the id to
/// [`remove_audio_output_tap`] it with.
pub fn add_audio_output_tap<F>(f: F) -> u64
where
    F: FnMut(AudioInfo, &AudioBuffer) + Send + 'static,
{
    add_audio_output_tap_box(Box::new(f))
}

pub fn add_audio_output_tap_box(f: AudioOutputTapFn) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut taps = taps().lock().unwrap();
    taps.insert(id, f);
    ACTIVE.store(true, Ordering::Release);
    id
}

pub fn remove_audio_output_tap(id: u64) {
    let mut taps = taps().lock().unwrap();
    taps.remove(&id);
    ACTIVE.store(!taps.is_empty(), Ordering::Release);
}

/// True while at least one tap is installed.
pub fn audio_output_tap_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Called from the output callback wrapper in `media_api.rs` once the app has
/// filled `buffer`. Realtime thread.
pub(crate) fn feed_audio_output_tap(info: AudioInfo, buffer: &AudioBuffer) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    // A tap that panics would poison the lock and silence every later frame
    // of audio for the app itself; take what we can get and carry on.
    let Ok(mut taps) = taps().lock() else {
        return;
    };
    for f in taps.values_mut() {
        f(info, buffer);
    }
}
