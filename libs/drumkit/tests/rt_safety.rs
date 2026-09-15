use makepad_drumkit::{DrumKit, DrumVoice, SampleBank};
use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn trigger_and_process_allocate_nothing_for_four_seconds() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/score-corpus/drums/OH");
    if !dir.is_dir() {
        eprintln!("skipping realtime allocation test: {} is absent", dir.display());
        return;
    }
    let bank = Arc::new(SampleBank::load(&dir).expect("load local Salamander corpus"));
    let mut kit = DrumKit::new(48_000.0);
    kit.set_bank(bank);
    let mut output = [[0.0f32; 2]; 512];
    for voice in DrumVoice::ALL {
        kit.trigger(voice, 0.8);
        kit.process(&mut output[..64]);
    }

    let before = ALLOCS.load(Ordering::Relaxed);
    for block in 0..375usize {
        if block % 2 == 0 {
            let index = block / 2;
            kit.trigger(DrumVoice::ALL[index % DrumVoice::ALL.len()], 0.2 + (index % 8) as f32 * 0.1);
        }
        output.fill([0.0; 2]);
        kit.process(&mut output);
    }
    kit.all_off();
    kit.process(&mut output);
    let after = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(before, after, "trigger/process allocated {} times", after - before);
}
