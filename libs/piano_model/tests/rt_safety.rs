// Proof that Piano::process never allocates: a counting global allocator
// wraps the system allocator for this whole test binary; after construction
// and warm-up, two seconds of heavy rendering (notes, re-strikes, pedal
// churn) must leave the allocation counter untouched.
//
// (Locks/IO: the render path calls no std sync or IO APIs at all — verified
// by review; the multicore path is offline-only and documented as such.
// Panics: every slice access in the render path is bounds-checked against
// preallocated fixed sizes, and the adversarial test in verify.rs exercises
// the edge cases.)

use makepad_piano_model::{Piano, PianoEvent, TimedEvent};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCS: AtomicUsize = AtomicUsize::new(0);

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static A: CountingAlloc = CountingAlloc;

#[test]
fn render_path_never_allocates() {
    let mut piano = Piano::new(48000.0);
    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    let mut events: Vec<TimedEvent> = Vec::with_capacity(64);

    // Warm-up: touch every code path (strikes on every key, damper noise,
    // sympathetic banks, voice sleep/wake) before counting.
    for key in 21..=108u8 {
        events.clear();
        events.push(TimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity: 100 } });
        events.push(TimedEvent { offset: 256, event: PianoEvent::NoteOff { key } });
        piano.process(&events, &mut out_l, &mut out_r);
    }
    for _ in 0..200 {
        piano.process(&[], &mut out_l, &mut out_r);
    }

    let a0 = ALLOCS.load(Ordering::Relaxed);
    let d0 = DEALLOCS.load(Ordering::Relaxed);

    // Two seconds of dense playing through the counted section.
    let mut key = 21u8;
    for block in 0..188u32 {
        events.clear();
        if block % 2 == 0 {
            events.push(TimedEvent { offset: (block % 512) & 511, event: PianoEvent::NoteOn { key, velocity: 127 } });
            key = if key >= 108 { 21 } else { key + 1 };
        }
        if block % 3 == 0 {
            events.push(TimedEvent {
                offset: 511,
                event: PianoEvent::Sustain { value: if block % 6 == 0 { 1.0 } else { 0.0 } },
            });
        }
        if block % 7 == 0 {
            events.push(TimedEvent { offset: 100, event: PianoEvent::NoteOff { key: 21 + (block as u8 % 88) } });
        }
        events.sort_by_key(|e| e.offset);
        piano.process(&events, &mut out_l, &mut out_r);
    }

    let allocs = ALLOCS.load(Ordering::Relaxed) - a0;
    let deallocs = DEALLOCS.load(Ordering::Relaxed) - d0;
    assert_eq!(allocs, 0, "Piano::process allocated {allocs} times");
    assert_eq!(deallocs, 0, "Piano::process deallocated {deallocs} times");
    println!("render path: 0 allocations across 188 blocks of dense playing");
}
