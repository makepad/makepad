use makepad_drumkit::{DrumKit, DrumVoice, SampleBank};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

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

fn bank() -> Arc<SampleBank> {
    static BANK: OnceLock<Arc<SampleBank>> = OnceLock::new();
    BANK.get_or_init(|| {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("drumkit-rt-safety-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        // Half a second keeps voices overlapping through stealing and fades.
        // The loader still reads every layer and round-robin of the sample kit.
        let frames = 24_000u32;
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(frames * 2).to_le_bytes());
        for i in 0..frames {
            let envelope = 1.0 - i as f32 / frames as f32;
            let sample = ((std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()
                * envelope * 16_000.0) as i16;
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        for (voice, layers, takes) in [
            ("kick", &["P", "F", "FF"][..], 2),
            ("snare", &["Ghost", "MP", "F", "FF"][..], 3),
            ("snareStick", &["F"][..], 1),
            ("hihatClosed", &["P", "F"][..], 1),
            ("hihatOpen", &["P", "F", "FF"][..], 1),
            ("hihatFoot", &["MP"][..], 1),
            ("hiTom", &["P", "F", "FF"][..], 1),
            ("loTom", &["PP", "MP", "FF"][..], 1),
            ("crash1", &["P", "FF"][..], 1),
            ("crash2", &["FF"][..], 1),
            ("ride1", &["MP", "FF"][..], 1),
            ("ride1Bell", &["F"][..], 1),
        ] {
            for layer in layers {
                for take in 1..=takes {
                    std::fs::write(dir.join(format!("{voice}_OH_{layer}_{take}.wav")), &wav).unwrap();
                }
            }
        }
        let loaded = SampleBank::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        Arc::new(loaded.expect("load generated drum samples"))
    }).clone()
}

#[test]
fn trigger_and_process_allocate_nothing_for_four_seconds() {
    let bank = bank();
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
