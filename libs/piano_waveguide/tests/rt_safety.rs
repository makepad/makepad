// WaveguidePiano::process never allocates: a counting global allocator wraps
// the system allocator for this test binary; after construction, rendering
// with strikes on every key, re-strikes, releases and pedal moves must leave
// the allocation counters untouched.

use makepad_piano_waveguide::{PianoEvent, TimedEvent, WaveguidePiano};
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
    let mut piano = WaveguidePiano::new(48000.0);
    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    let mut events: Vec<TimedEvent> = Vec::with_capacity(64);
    let a0 = ALLOCS.load(Ordering::Relaxed);
    let d0 = DEALLOCS.load(Ordering::Relaxed);
    for key in 21..=108u8 {
        events.clear();
        events.push(TimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity: 100 } });
        events.push(TimedEvent { offset: 100, event: PianoEvent::Sustain { value: (key % 3) as f32 * 0.5 } });
        events.push(TimedEvent { offset: 200, event: PianoEvent::NoteOn { key, velocity: 40 } });
        events.push(TimedEvent { offset: 256, event: PianoEvent::NoteOff { key } });
        events.push(TimedEvent { offset: 300, event: PianoEvent::SoftPedal { on: key % 2 == 0 } });
        events.push(TimedEvent { offset: 400, event: PianoEvent::Sostenuto { on: key % 5 == 0 } });
        piano.process(&events, &mut out_l, &mut out_r);
    }
    events.clear();
    events.push(TimedEvent { offset: 0, event: PianoEvent::Sustain { value: 0.0 } });
    events.push(TimedEvent { offset: 0, event: PianoEvent::Sostenuto { on: false } });
    piano.process(&events, &mut out_l, &mut out_r);
    // ring out until everything sleeps, then wake again
    for _ in 0..400 {
        piano.process(&[], &mut out_l, &mut out_r);
    }
    events.clear();
    events.push(TimedEvent { offset: 511, event: PianoEvent::NoteOn { key: 60, velocity: 127 } });
    events.push(TimedEvent { offset: 600, event: PianoEvent::AllSoundOff });
    piano.process(&events, &mut out_l, &mut out_r);
    piano.reset();
    assert_eq!(ALLOCS.load(Ordering::Relaxed), a0, "allocations in the render path");
    assert_eq!(DEALLOCS.load(Ordering::Relaxed), d0, "deallocations in the render path");
}
