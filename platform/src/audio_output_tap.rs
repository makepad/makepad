//! Audio-output tap: a read-only fork of everything this app writes to its
//! program audio output.
//!
//! The app's own output callback (`CxMediaApi::audio_output`) fills a buffer
//! the device then plays. A tap is invoked with that same buffer right after
//! the app has filled it, so a recorder gets exactly what the speakers get —
//! no OS loopback device, no screen-recording permission, no other app's
//! sound. `platform/src/os/apple/audio_tap.rs` is the *other* thing: a
//! ScreenCaptureKit loopback of the whole machine.
//!
//! Taps hear output 0, the program; a second output is a monitor. They run
//! on that output's realtime thread, so a tap body must not block or
//! allocate: copy into a ring and get off the thread. A tap must never remove
//! itself from inside its own body — removal waits for the running callback
//! to finish with the tap, and that callback would be waiting for itself.
//!
//! The registry is a fixed row of slots the callback walks with plain atomic
//! loads: nothing on the realtime thread ever locks, allocates or frees.
//! Installing takes a free slot or refuses; removing empties the slot, waits
//! out any callback that may still be inside the tap, and only then drops
//! it, on the caller's thread. A tap that panics is dropped from its slot on
//! the spot and leaked, never freed on the realtime thread.

use crate::audio::{AudioBuffer, AudioInfo};
use std::cell::UnsafeCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

pub type AudioOutputTapFn = Box<dyn FnMut(AudioInfo, &AudioBuffer) + Send + 'static>;

/// Taps that can be installed at once. A fixed number, so the callback
/// walks a row and never a list that can grow under it.
pub const MAX_TAPS: usize = 4;
/// The output the taps hear.
const PROGRAM_OUTPUT: usize = 0;

struct Entry {
    id: u64,
    /// Called only from the program output's callback; dropped only after
    /// that callback has been seen to leave.
    tap: UnsafeCell<AudioOutputTapFn>,
}

static SLOTS: [AtomicPtr<Entry>; MAX_TAPS] = [const { AtomicPtr::new(null_mut()) }; MAX_TAPS];
/// Checked first, so an app with no tap installed pays one atomic load per
/// audio callback.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Odd while the callback is inside a feed, even between feeds: what a
/// remover waits on before it frees a tap the callback may still be running.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Install a tap on the app's program output. `None` when every slot is
/// taken — refused here, never in the callback. Otherwise the id to
/// [`remove_audio_output_tap`] it with.
pub fn add_audio_output_tap<F>(f: F) -> Option<u64>
where
    F: FnMut(AudioInfo, &AudioBuffer) + Send + 'static,
{
    add_audio_output_tap_box(Box::new(f))
}

pub fn add_audio_output_tap_box(f: AudioOutputTapFn) -> Option<u64> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let entry = Box::into_raw(Box::new(Entry { id, tap: UnsafeCell::new(f) }));
    for slot in SLOTS.iter() {
        if slot
            .compare_exchange(null_mut(), entry, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            ACTIVE.store(true, Ordering::Release);
            return Some(id);
        }
    }
    // SAFETY: the entry never reached a slot, so nothing else can see it.
    drop(unsafe { Box::from_raw(entry) });
    None
}

/// Take the tap out. Returns once the callback can no longer be inside it,
/// and drops the tap on this thread. An id that is no longer installed is a
/// no-op, and never reaches a later tap in the same slot.
pub fn remove_audio_output_tap(id: u64) {
    for slot in SLOTS.iter() {
        let entry = slot.load(Ordering::SeqCst);
        if entry.is_null() {
            continue;
        }
        // SAFETY: an entry stays allocated while its pointer is in a slot;
        // the id is written once, before the entry is published.
        if unsafe { (*entry).id } != id {
            continue;
        }
        if slot
            .compare_exchange(entry, null_mut(), Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Someone else took it out first.
            return;
        }
        ACTIVE.store(
            SLOTS.iter().any(|slot| !slot.load(Ordering::SeqCst).is_null()),
            Ordering::Release,
        );
        wait_for_callback_to_leave();
        // SAFETY: the slot is empty and every callback that could have
        // loaded the pointer has finished; this thread is the only owner.
        drop(unsafe { Box::from_raw(entry) });
        return;
    }
}

/// A callback that was inside a feed when the slot emptied may still be
/// running the tap: wait for that feed to end. A feed that starts later
/// sees the empty slot.
fn wait_for_callback_to_leave() {
    let seen = EPOCH.load(Ordering::SeqCst);
    if seen % 2 == 1 {
        while EPOCH.load(Ordering::SeqCst) == seen {
            std::thread::yield_now();
        }
    }
}

/// True while at least one tap is installed.
pub fn audio_output_tap_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Called from the output callback wrapper in `media_api.rs` once the app has
/// filled `buffer` for output `output`. Realtime thread.
pub(crate) fn feed_audio_output_tap(output: usize, info: AudioInfo, buffer: &AudioBuffer) {
    if output != PROGRAM_OUTPUT || !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    EPOCH.fetch_add(1, Ordering::SeqCst);
    for slot in SLOTS.iter() {
        let entry = slot.load(Ordering::SeqCst);
        if entry.is_null() {
            continue;
        }
        // SAFETY: one thread feeds the program output, so nothing else calls
        // the tap; the entry outlives this walk because removal waits for
        // the epoch to move on.
        let survived =
            catch_unwind(AssertUnwindSafe(|| unsafe { (*(*entry).tap.get())(info, buffer) }))
                .is_ok();
        if !survived {
            // Out of its slot on the spot and leaked: nothing frees on the
            // realtime thread, and a tap that panics has forfeited its turn.
            let _ = slot.compare_exchange(entry, null_mut(), Ordering::SeqCst, Ordering::SeqCst);
        }
    }
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// The registry is one per process, so its tests take turns -- at module
/// scope rather than inside the test module, because the seam that wraps
/// this feed has a test of its own and it has to take the same turn.
#[cfg(test)]
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioDeviceId;
    use crate::live_id::LiveId;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::{Duration, Instant};

    fn info() -> AudioInfo {
        AudioInfo { device_id: AudioDeviceId(LiveId(7)), time: None, sample_rate: 48_000.0 }
    }

    #[test]
    fn a_tap_hears_every_program_buffer_and_nothing_from_the_phones() {
        let _serial = serial();
        let heard = Arc::new(AtomicUsize::new(0));
        let count = heard.clone();
        let id = add_audio_output_tap(move |_, buffer| {
            count.fetch_add(buffer.frame_count(), Ordering::Relaxed);
        })
        .expect("a slot");
        let buffer = AudioBuffer::new_with_size(256, 2);
        feed_audio_output_tap(0, info(), &buffer);
        feed_audio_output_tap(0, info(), &buffer);
        feed_audio_output_tap(1, info(), &buffer);
        assert_eq!(heard.load(Ordering::Relaxed), 512, "the program output, twice; the phones never");
        remove_audio_output_tap(id);
        feed_audio_output_tap(0, info(), &buffer);
        assert_eq!(heard.load(Ordering::Relaxed), 512, "removed: silent from then on");
        assert!(!audio_output_tap_active());
    }

    #[test]
    fn the_registry_holds_a_fixed_number_of_taps_and_refuses_the_next() {
        let _serial = serial();
        let ids: Vec<u64> = (0..MAX_TAPS)
            .map(|_| add_audio_output_tap(|_, _| {}).expect("a slot"))
            .collect();
        assert!(add_audio_output_tap(|_, _| {}).is_none(), "full: refused at add time, never in the callback");
        remove_audio_output_tap(ids[1]);
        let again = add_audio_output_tap(|_, _| {}).expect("the freed slot");
        for id in ids.iter().copied().chain([again]) {
            remove_audio_output_tap(id);
        }
        assert!(!audio_output_tap_active());
    }

    #[test]
    fn removing_returns_only_after_the_running_callback_has_finished_with_the_tap() {
        let _serial = serial();
        let started = Arc::new(Barrier::new(2));
        let finished = Arc::new(AtomicUsize::new(0));
        let (started_tap, finished_tap) = (started.clone(), finished.clone());
        let id = add_audio_output_tap(move |_, _| {
            started_tap.wait();
            std::thread::sleep(Duration::from_millis(150));
            finished_tap.store(1, Ordering::Release);
        })
        .expect("a slot");
        let callback = std::thread::spawn(|| {
            let buffer = AudioBuffer::new_with_size(64, 2);
            feed_audio_output_tap(0, info(), &buffer);
        });
        started.wait();
        let began = Instant::now();
        remove_audio_output_tap(id);
        assert_eq!(finished.load(Ordering::Acquire), 1, "the tap was still running when remove returned");
        assert!(began.elapsed() >= Duration::from_millis(100), "remove waited for the callback");
        callback.join().unwrap();
    }

    #[test]
    fn a_stale_id_never_removes_the_taps_successor_in_the_same_slot() {
        let _serial = serial();
        let first = add_audio_output_tap(|_, _| {}).expect("a slot");
        remove_audio_output_tap(first);
        let heard = Arc::new(AtomicUsize::new(0));
        let count = heard.clone();
        let second = add_audio_output_tap(move |_, _| {
            count.fetch_add(1, Ordering::Relaxed);
        })
        .expect("the same slot again");
        remove_audio_output_tap(first);
        feed_audio_output_tap(0, info(), &AudioBuffer::new_with_size(8, 2));
        assert_eq!(heard.load(Ordering::Relaxed), 1, "the old id must not reach the new tap");
        remove_audio_output_tap(second);
    }

    #[test]
    fn a_tap_that_panics_is_dropped_and_the_others_keep_hearing() {
        let _serial = serial();
        let heard = Arc::new(AtomicUsize::new(0));
        let count = heard.clone();
        let quiet = add_audio_output_tap(move |_, _| {
            count.fetch_add(1, Ordering::Relaxed);
        })
        .expect("a slot");
        let mut first = true;
        let loud = add_audio_output_tap(move |_, _| {
            if first {
                first = false;
                panic!("a tap that misbehaves");
            }
        })
        .expect("a slot");
        let buffer = AudioBuffer::new_with_size(8, 2);
        feed_audio_output_tap(0, info(), &buffer);
        feed_audio_output_tap(0, info(), &buffer);
        assert_eq!(heard.load(Ordering::Relaxed), 2, "the quiet tap heard both buffers");
        remove_audio_output_tap(loud);
        remove_audio_output_tap(quiet);
        assert!(!audio_output_tap_active());
    }
}
