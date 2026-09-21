// Proof that DrumKit::process and DrumKit::trigger never allocate: a
// counting global allocator wraps the system allocator for this test binary;
// after construction and warm-up, four seconds of dense playing (every
// voice, voice stealing, every block size) must leave the counter untouched.
//
// (Locks/IO: the render path calls no std sync or IO APIs at all. Panics:
// every slice access in the render path is bounds-checked against
// preallocated fixed sizes; sound.rs exercises the polyphony and sample-rate
// edges.)

use makepad_drumkit_phys::{DrumKit, DrumVoice};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
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
fn trigger_and_process_never_allocate() {
    let mut kit = DrumKit::new(48000.0);
    let mut out = vec![[0.0f32; 2]; 512];
    // warm-up: every voice once
    for v in DrumVoice::ALL {
        kit.trigger(v, 0.9);
        kit.process(&mut out);
    }
    let before = ALLOCS.load(Ordering::Relaxed);
    let mut i = 0usize;
    for block in 0..375u32 {
        // 375 x 512 = 4 s; a new hit every second block, stealing after 16
        if block % 2 == 0 {
            kit.trigger(DrumVoice::ALL[i % DrumVoice::ALL.len()], 0.3 + 0.7 * ((i % 7) as f32 / 6.0));
            i += 1;
        }
        let n = match block % 4 {
            0 => 512,
            1 => 64,
            2 => 1,
            _ => 300,
        };
        out[..n].iter_mut().for_each(|f| *f = [0.0; 2]);
        kit.process(&mut out[..n]);
    }
    kit.all_off();
    kit.process(&mut out);
    let after = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(before, after, "the render path allocated {} times", after - before);
}
