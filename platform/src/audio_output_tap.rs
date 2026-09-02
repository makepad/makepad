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
//! allocate: copy into a ring and get off the thread.
//!
//! The registry is a fixed row of slots the callback walks with plain atomic
//! loads: nothing on the realtime thread ever locks, allocates or frees, and
//! nothing on the caller's thread ever waits for it. Installing takes a free
//! slot or refuses. Removing empties the slot, so no later buffer reaches the
//! tap, and returns at once; a buffer the callback was already feeding may
//! still be inside the tap, so the tap is dropped on the caller's side once
//! that buffer is done -- right away when no buffer is being fed, otherwise
//! by the next call into the registry after it. A tap that panics is dropped
//! from its slot on the spot and leaked, never freed on the realtime thread.

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
    /// The feed epoch that was running when the entry left its slot: once
    /// the epoch has moved past it, the callback is done with the entry.
    retired_at: AtomicU64,
}

static SLOTS: [AtomicPtr<Entry>; MAX_TAPS] = [const { AtomicPtr::new(null_mut()) }; MAX_TAPS];
/// Removed taps a feed may still be inside, waiting to be dropped.
const MAX_RETIRED: usize = 2 * MAX_TAPS;
static RETIRED: [AtomicPtr<Entry>; MAX_RETIRED] =
    [const { AtomicPtr::new(null_mut()) }; MAX_RETIRED];
/// Checked first, so an app with no tap installed pays one atomic load per
/// audio callback.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Odd while the callback is inside a feed, even between feeds: what tells
/// a remover whether the callback may still be running a tap it took out.
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
    reclaim_retired();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let entry = Box::into_raw(Box::new(Entry {
        id,
        tap: UnsafeCell::new(f),
        retired_at: AtomicU64::new(0),
    }));
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

/// Take the tap out. Returns at once: no buffer fed after this reaches the
/// tap. The tap is dropped on this side, never on the realtime thread --
/// here when no buffer is being fed, otherwise by the first call into the
/// registry after that buffer is done. An id that is no longer installed is
/// a no-op, and never reaches a later tap in the same slot.
pub fn remove_audio_output_tap(id: u64) {
    reclaim_retired();
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
        // A feed that starts after the slot emptied sees it empty; only a
        // feed already running (an odd epoch) can still be inside the tap.
        let seen = EPOCH.load(Ordering::SeqCst);
        if seen % 2 == 0 {
            // SAFETY: the slot is empty and no feed is running; this thread
            // is the only owner.
            drop(unsafe { Box::from_raw(entry) });
        } else {
            retire(entry, seen);
        }
        return;
    }
}

/// Park a removed entry until the feed that may be inside it has ended.
fn retire(entry: *mut Entry, seen: u64) {
    // SAFETY: the entry left its slot, so only this thread and possibly the
    // running feed can reach it, and the feed never touches `retired_at`.
    unsafe { (*entry).retired_at.store(seen, Ordering::SeqCst) };
    for parked in RETIRED.iter() {
        if parked
            .compare_exchange(null_mut(), entry, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return;
        }
    }
    // Every parking place is taken, which takes a feed stuck inside one
    // buffer across eight removals: leak this tap rather than wait for the
    // feed or free the tap under it.
    // SAFETY: as above; the id is written once, before the entry was published.
    let id = unsafe { (*entry).id };
    crate::warning!("audio output tap {id}: nowhere to park it after removal, leaked");
}

/// Drop every parked entry whose feed has ended since it was parked.
fn reclaim_retired() {
    for parked in RETIRED.iter() {
        let entry = parked.load(Ordering::SeqCst);
        if entry.is_null() {
            continue;
        }
        // SAFETY: a parked entry stays allocated until it is taken out here.
        let seen = unsafe { (*entry).retired_at.load(Ordering::SeqCst) };
        if EPOCH.load(Ordering::SeqCst) == seen {
            continue;
        }
        if parked
            .compare_exchange(entry, null_mut(), Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            // SAFETY: the feed that was running when it was parked has ended
            // and no later feed can see it; this thread took it out.
            drop(unsafe { Box::from_raw(entry) });
        }
    }
}

/// True while at least one tap is installed. Also drops removed taps whose
/// last buffer has since been fed.
pub fn audio_output_tap_active() -> bool {
    reclaim_retired();
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
    use std::sync::{Arc, Barrier};
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

    /// Removing never waits for the realtime thread: it returns while a
    /// buffer is still inside the tap, no later buffer reaches the tap, and
    /// the tap is dropped only after that buffer is done -- by the next call
    /// into the registry, on the caller's side.
    #[test]
    fn removing_returns_at_once_and_drops_the_tap_once_the_running_buffer_is_done() {
        let _serial = serial();
        struct Dropped(Arc<AtomicUsize>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(1, Ordering::Release);
            }
        }
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let calls = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let (started_tap, release_tap) = (started.clone(), release.clone());
        let calls_tap = calls.clone();
        let guard = Dropped(dropped.clone());
        let id = add_audio_output_tap(move |_, _| {
            let _keep = &guard;
            if calls_tap.fetch_add(1, Ordering::AcqRel) == 0 {
                started_tap.wait();
                release_tap.wait();
            }
        })
        .expect("a slot");
        let callback = std::thread::spawn(|| {
            feed_audio_output_tap(0, info(), &AudioBuffer::new_with_size(64, 2));
        });
        started.wait();
        let began = Instant::now();
        remove_audio_output_tap(id);
        assert!(began.elapsed() < Duration::from_millis(100), "remove waited for the callback");
        assert_eq!(dropped.load(Ordering::Acquire), 0, "dropped with the callback inside it");
        assert!(!audio_output_tap_active());
        assert_eq!(dropped.load(Ordering::Acquire), 0, "reclaimed with the callback inside it");
        release.wait();
        callback.join().unwrap();
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert!(!audio_output_tap_active());
        assert_eq!(dropped.load(Ordering::Acquire), 1, "the buffer is done; the tap was kept");
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
