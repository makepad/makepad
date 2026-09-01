// Proof that LearnedPiano::process never allocates, in its own test binary
// (a counting global allocator is per-process, and the default parallel
// test runner would bleed other tests' allocations into the count — the
// same reason rt_safety.rs stands alone for the physical engine).

use makepad_piano_model::learned::LearnedPiano;
use makepad_piano_model::{PianoEvent, TimedEvent};
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
fn learned_render_path_never_allocates() {
    let mut p = LearnedPiano::new(48000.0);
    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    let mut events: Vec<TimedEvent> = Vec::with_capacity(64);
    // Warm-up over every key, both slots (re-strike), pedal churn.
    for key in 21..=108u8 {
        events.clear();
        events.push(TimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity: 100 } });
        events.push(TimedEvent { offset: 200, event: PianoEvent::NoteOn { key, velocity: 60 } });
        events.push(TimedEvent { offset: 400, event: PianoEvent::NoteOff { key } });
        p.process(&events, &mut out_l, &mut out_r);
    }
    for _ in 0..200 {
        p.process(&[], &mut out_l, &mut out_r);
    }

    let a0 = ALLOCS.load(Ordering::Relaxed);
    let d0 = DEALLOCS.load(Ordering::Relaxed);

    let mut key = 21u8;
    for block in 0..188u32 {
        events.clear();
        if block % 2 == 0 {
            events.push(TimedEvent { offset: block % 512, event: PianoEvent::NoteOn { key, velocity: 127 } });
            key = if key >= 108 { 21 } else { key + 1 };
        }
        if block % 3 == 0 {
            events.push(TimedEvent {
                offset: 511,
                event: PianoEvent::Sustain { value: if block % 6 == 0 { 1.0 } else { 0.0 } },
            });
        }
        if block % 5 == 0 {
            events.push(TimedEvent { offset: 200, event: PianoEvent::SoftPedal { on: block % 10 == 0 } });
        }
        if block % 7 == 0 {
            events.push(TimedEvent { offset: 100, event: PianoEvent::NoteOff { key: 21 + (block as u8 % 88) } });
        }
        events.sort_by_key(|e| e.offset);
        p.process(&events, &mut out_l, &mut out_r);
    }

    let allocs = ALLOCS.load(Ordering::Relaxed) - a0;
    let deallocs = DEALLOCS.load(Ordering::Relaxed) - d0;
    assert_eq!(allocs, 0, "LearnedPiano::process allocated {allocs} times");
    assert_eq!(deallocs, 0, "LearnedPiano::process deallocated {deallocs} times");
    println!("learned render path: 0 allocations across 188 blocks of dense playing");
}
