//! The audio callbacks' halves of a call never allocate or free: a counting allocator watches
//! the calling thread while it pushes a microphone, renders the mix (mono and planar), and takes a
//! new set of peers in the middle. A test target of its own, so its allocator touches no other test.

use makepad_call::driver::voice::{voice_in, voice_out, VoiceDecoder, VoiceMarks};
use makepad_teamtalk::jitter::JitterRing;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct Counting;

static EVENTS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

fn note() {
    if WATCHING.with(Cell::get) {
        EVENTS.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: every call goes straight to the system allocator; counting touches only an atomic and a
// const-initialised thread local, which allocate nothing.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn watched<R>(f: impl FnOnce() -> R) -> R {
    WATCHING.with(|w| w.set(true));
    let r = f();
    WATCHING.with(|w| w.set(false));
    r
}

#[test]
fn the_audio_callbacks_never_allocate_or_free() {
    let (mut input, frames) = voice_in(Instant::now(), Arc::new(AtomicBool::new(false)), Arc::new(VoiceMarks::default()));
    let (mut output, mut control) = voice_out();
    let ring = Arc::new(JitterRing::new());
    let marks = Arc::new(VoiceMarks::default());
    control.set([(1, ring.clone(), marks.clone())]);
    let mut decoder = VoiceDecoder::default();
    let mut payload = Vec::new();
    let (mut phase, mut block, mut mono, mut planar) = (0f32, [0f32; 480], [0f32; 480], [0f32; 960]);
    let mut loud = 0;
    for n in 0..1000 {
        if n == 500 {
            // a second peer joins: the callback swaps the new set in and gives the old one back
            control.set([(1, ring.clone(), marks.clone()), (2, Arc::new(JitterRing::new()), Arc::new(VoiceMarks::default()))]);
        }
        let talking = n % 200 < 150;
        for s in block.iter_mut() {
            *s = if talking { 0.3 * (phase * std::f32::consts::TAU).sin() } else { 0.0 };
            phase = (phase + 440.0 / 48_000.0).fract();
        }
        watched(|| input.push_mono(48_000.0, &block));
        while let Some(f) = frames.pop(&mut payload) {
            decoder.insert(&ring, f.seq, f.flags, &payload);
        }
        control.tick();
        // the device's block, mono and planar stereo in turn (each 10 ms, as the input)
        let heard = if n % 2 == 0 {
            mono.fill(0.0);
            watched(|| output.mix_into_mono(48_000.0, &mut mono));
            &mono[..]
        } else {
            planar.fill(0.0);
            watched(|| output.mix_into_planar(48_000.0, 480, 2, &mut planar));
            &planar[..480]
        };
        loud += usize::from(heard.iter().map(|s| s * s).sum::<f32>() / 480.0 > 0.01);
    }
    assert!(loud > 500, "the tone must play through the mix (loud in {loud} of 1000 blocks), or the test proves nothing");
    let events = EVENTS.load(Ordering::Relaxed);
    assert_eq!(events, 0, "the input and output callbacks allocated or freed {events} times");
}
