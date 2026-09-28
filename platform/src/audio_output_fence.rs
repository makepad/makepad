//! What a panicking output callback costs: its own turn, and nothing else.
//!
//! The app's output closure runs on a realtime device thread, and until this
//! module existed a panic in it unwound straight into the backend. What that
//! costs depends on the backend, and on two of them it costs the process:
//! the desktop backends call the closure with a lock held, so the unwind
//! poisons it and every later buffer fails the same way forever, reported as
//! contention; the mobile, web and Apple backends call it from an `extern
//! "C"` frame, where an unwind is undefined behaviour and aborts.
//!
//! So the closure runs inside a fence. A panic costs the buffer it was
//! filling -- handed over as silence, because the backends refill and reuse
//! one buffer and a half-written one plays as a stuck loop at full level --
//! and it retires that output for the rest of the run. Retired and not
//! retried: a closure that has already unwound once has state nobody can
//! vouch for, and the app is told so it can say so out loud.
//!
//! Nothing is said from here. Every logging macro in this crate goes through
//! a ring that locks and allocates, which is exactly what a realtime thread
//! may not do. The panic's own words are parked in a slot the UI thread
//! takes them out of, and a count is bumped; saying it is the app's job.
//!
//! Two limits, stated rather than hidden. Under `panic = "abort"` (the
//! `small` profile) and on the web there is no unwinding to catch, so the
//! fence is inert and the process still dies at the panic site: a
//! distribution build is not protected by this. And a retired closure is
//! kept rather than dropped -- freeing an app's whole mix graph is not
//! something a realtime thread may do -- so it holds what it holds until
//! the process ends.

use crate::audio::{AudioBuffer, AudioInfo, AudioOutputFn, MAX_AUDIO_DEVICE_INDEX};
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static PANICS: [AtomicU64; MAX_AUDIO_DEVICE_INDEX] =
    [const { AtomicU64::new(0) }; MAX_AUDIO_DEVICE_INDEX];

/// The panic's own words, for the UI thread to say out loud. Parked only
/// into an EMPTY slot: a second panic on one output would otherwise drop the
/// first payload here, on the realtime thread, and a drop is a free.
static NOTES: [Mutex<Option<Box<dyn Any + Send>>>; MAX_AUDIO_DEVICE_INDEX] =
    [const { Mutex::new(None) }; MAX_AUDIO_DEVICE_INDEX];

/// One app output closure and what has become of it.
pub(crate) struct FencedOutput {
    index: usize,
    f: AudioOutputFn,
    retired: bool,
}

impl FencedOutput {
    pub(crate) fn new(index: usize, f: AudioOutputFn) -> FencedOutput {
        FencedOutput { index, f, retired: false }
    }

    pub(crate) fn call(&mut self, info: AudioInfo, buffer: &mut AudioBuffer) {
        // Retired: silence, and the closure is never entered again. Kept and
        // not dropped -- see the module note.
        if self.retired {
            buffer.zero();
            return;
        }
        let f = &mut self.f;
        // `AssertUnwindSafe` earned rather than waved through: on the panic
        // path the closure is retired unread and undropped, and the buffer
        // is zeroed on this same call before the backend can copy it out.
        // What the app's own UI thread goes on reading is the state it
        // shares with the callback, and the two structures that carry it --
        // a single-producer ring and a sequence-guarded slot -- have no
        // panic point between the read and the index store, so neither can
        // be left half-updated. Anyone who adds a retry, drops the retired
        // closure, or reads anything back out of it has invalidated that
        // argument.
        if let Err(said) = catch_unwind(AssertUnwindSafe(|| f(info, buffer))) {
            self.retired = true;
            // The interrupted buffer goes out as silence, not as whatever
            // was half-written into it.
            buffer.zero();
            park(self.index, said);
        }
    }
}

/// Park the words and count the fault. Realtime thread: no allocation, no
/// blocking, and nothing freed.
fn park(index: usize, said: Box<dyn Any + Send>) {
    let mut said = Some(said);
    if let Some(slot) = NOTES.get(index) {
        if let Ok(mut slot) = slot.try_lock() {
            if slot.is_none() {
                *slot = said.take();
            }
        }
    }
    if let Some(lost) = said {
        // Leaked rather than dropped, the same rule the tap registry next
        // door keeps: nothing frees on the realtime thread. The count still
        // moves, so a fault whose words were lost is still reported.
        std::mem::forget(lost);
    }
    if let Some(count) = PANICS.get(index) {
        count.fetch_add(1, Ordering::Release);
    }
}

/// How many times one output's callback has panicked.
pub fn audio_output_panics(index: usize) -> u64 {
    PANICS.get(index).map_or(0, |count| count.load(Ordering::Acquire))
}

/// The same across every output, for a health report that has one number.
pub fn audio_output_panics_total() -> u64 {
    PANICS.iter().map(|count| count.load(Ordering::Acquire)).sum()
}

/// The parked words for one output, taken. UI thread only.
///
/// A count that has moved with no note here is a fault whose words were
/// lost, and the caller says so rather than staying silent.
pub fn take_audio_output_panic_note(index: usize) -> Option<String> {
    let taken = NOTES.get(index).and_then(|slot| slot.lock().ok()?.take())?;
    match taken.downcast::<String>() {
        Ok(said) => Some(*said),
        Err(other) => match other.downcast::<&'static str>() {
            Ok(said) => Some((*said).to_string()),
            Err(_) => Some("a panic with words this app cannot read".to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    /// A realtime thread may not say anything: every logging macro in this
    /// crate goes through a ring that locks and allocates. There is no
    /// witness for that but the source, so the source is what this reads --
    /// its own body only, with the test module cut off the end so the scan
    /// cannot match itself.
    #[test]
    fn the_fence_says_nothing_from_the_audio_thread() {
        let source = include_str!("audio_output_fence.rs");
        // Assembled rather than written whole: a scan written out in full
        // would find its own copy and cut the body at the wrong place.
        let split = format!("mod {}", "tests {");
        let body = source.split(&split).next().expect("the module has a body");
        for said in ["log!(", "error!(", "warning!(", "println!(", "eprintln!("] {
            assert!(!body.contains(said), "the fence says {said} on the audio thread");
        }
        // And it frees nothing there either.
        for freed in ["drop(self.f", "self.f.take("] {
            assert!(!body.contains(freed), "the fence frees {freed} on the audio thread");
        }
    }
}
