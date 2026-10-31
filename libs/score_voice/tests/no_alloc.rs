//! The realtime path allocates nothing: after construction, notes, releases,
//! steals and renders of a choir go through a counting allocator with zero
//! allocations.

use makepad_score_voice::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

struct Counting;

static ARMED: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn the_audio_path_never_allocates() {
    let profiles = [Arc::new(VoiceProfile::named("tenor").unwrap()), Arc::new(VoiceProfile::named("bass").unwrap())];
    let mut choir = Choir::new(&profiles, 8, 48_000.0, ChoirStyle { parts: 2, ..ChoirStyle::default() }, 11);
    let syllables = [PhonemeSeq::ipa("stɹɔŋ"), PhonemeSeq::ipa("ʧɝʧ"), PhonemeSeq::ipa("aɪ"), PhonemeSeq::ipa("spɹiŋz").with_flags(PhonemeSeq::LEGATO)];
    let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
    let mut serial = 0;
    ARMED.store(true, Ordering::SeqCst);
    for i in 0..400 {
        if i % 25 == 0 {
            let syl = syllables[(i / 25) % syllables.len()];
            // Steals the previous note mid-syllable half of the time.
            serial = choir.note_on((i % 7) as u32 * 13, SungNote { note: 55.0 + (i % 12) as f32, vel: 0.7, dur: 0.2, syl }, (i / 25 % 3) as u8);
        }
        if i % 25 == 20 {
            choir.release(5, serial);
        }
        choir.render_stereo(&mut l, &mut r);
    }
    ARMED.store(false, Ordering::SeqCst);
    assert_eq!(COUNT.load(Ordering::SeqCst), 0, "allocations on the audio path");
    assert!(l.iter().all(|v| v.is_finite()));
}
