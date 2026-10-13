//! Realtime entries shared by the ALSA and PulseAudio transports.
use crate::media::{AudioBlock, AudioFn};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Entry {
    pub function: AudioFn,
    pub context: usize,
}

/// The realtime side never waits for the control thread. Replacement disables admission,
/// then waits only for the already admitted block, as required by AudioEntry's ABI.
#[derive(Default)]
pub struct Slot {
    busy: AtomicBool,
    changing: AtomicBool,
    entry: UnsafeCell<Option<Entry>>,
}
unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}

impl Slot {
    pub fn replace(&self, entry: Option<Entry>) {
        self.changing.store(true, Ordering::SeqCst);
        while self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            std::hint::spin_loop();
        }
        unsafe {
            *self.entry.get() = entry;
        }
        self.busy.store(false, Ordering::SeqCst);
        self.changing.store(false, Ordering::SeqCst);
    }

    pub fn call(&self, device: u64, rate: f64, buffer: &mut Buffer) {
        if self.changing.load(Ordering::SeqCst)
            || self
                .busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return;
        }
        // The client entry catches its own panic; unwinding through extern C is forbidden.
        // HAL-side conversion is also caught by each native callback before returning to C.
        if let Some(entry) = unsafe { *self.entry.get() } {
            for start in (0..buffer.frames).step_by(super::AUDIO_BLOCK_FRAMES) {
                let frames = (buffer.frames - start).min(super::AUDIO_BLOCK_FRAMES);
                let mut planes = unsafe {
                    [
                        buffer.samples.as_mut_ptr().add(start),
                        buffer.samples.as_mut_ptr().add(buffer.capacity + start),
                    ]
                };
                let mut block = AudioBlock {
                    device,
                    sample_rate: rate,
                    sample_time: 0.0,
                    host_time: 0,
                    rate_scalar: 1.0,
                    has_time: 0,
                    frames: frames as u32,
                    channels: 2,
                    planes: planes.as_mut_ptr(),
                };
                (entry.function)(entry.context as *mut _, &mut block);
            }
        }
        self.busy.store(false, Ordering::SeqCst);
    }
}

/// Negotiated transport periods are split into this preallocated planar storage. A larger
/// PulseAudio request is processed in more blocks, never allocated or discarded in a callback.
pub struct Buffer {
    samples: Vec<f32>,
    capacity: usize,
    pub frames: usize,
}
impl Buffer {
    pub fn new(frames: usize) -> Self {
        Self {
            samples: vec![0.0; frames * 2],
            capacity: frames,
            frames,
        }
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn clear(&mut self) {
        self.samples.fill(0.0);
    }
    pub fn read(&mut self, interleaved: &[f32]) {
        self.frames = interleaved.len() / 2;
        for (i, pair) in interleaved.chunks_exact(2).enumerate() {
            self.samples[i] = pair[0];
            self.samples[self.capacity + i] = pair[1];
        }
    }
    pub fn write(&self, interleaved: &mut [f32]) {
        for (i, pair) in interleaved.chunks_exact_mut(2).enumerate() {
            pair[0] = self.samples[i];
            pair[1] = self.samples[self.capacity + i];
        }
    }
}

#[derive(Clone, Default)]
pub struct Entries(pub [Vec<Arc<Slot>>; 2]);
impl Entries {
    pub fn slot(&mut self, direction: usize, index: usize) -> Arc<Slot> {
        let slots = &mut self.0[direction];
        slots.resize_with(slots.len().max(index + 1), || Arc::new(Slot::default()));
        slots[index].clone()
    }
    pub fn clear(&self) {
        for slot in self.0.iter().flatten() {
            slot.replace(None);
        }
    }
}
