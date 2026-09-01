// Multicore rendering for offline/bounce use.
//
// Design and honest real-time analysis:
// - The REAL-TIME path is Piano::process — single-threaded SIMD. Measured
//   (tests/perf.rs) it renders all 88 keys ringing at once in a small
//   fraction of one core at 48 kHz, so the audio callback simply does not
//   need threads, and therefore takes zero scheduling risk: no locks, no
//   barriers, nothing that can priority-invert or stall the callback.
// - process_multicore parallelises the per-voice modal kernels across a
//   worker pool with two std::sync::Barrier crossings per 64-sample chunk.
//   Barriers BLOCK: a stalled worker stalls the bounce. That is the honest
//   trade — it is why this entry point is documented as offline-only and the
//   callback never uses it. (thread spawn also allocates; again: offline.)
// - Determinism: workers only run Voice::render for disjoint voice subsets;
//   every voice renders into its own buffers, and the main thread merges
//   them in fixed key order and runs all control logic itself — the same
//   code, in the same order, as the single-threaded path. The output is
//   bit-identical to Piano::process (proved in tests/verify.rs).
//
// Safety: workers touch voices only between the start and end barriers of a
// chunk; the main thread touches them only outside that window (event
// application, control ticks, merge). Both sides go through the same raw
// pointer, and no reference created from it lives across a barrier, so no
// two live &mut ever alias.

use crate::modal::MAX_CHUNK;
use crate::voice::Voice;
use crate::{Piano, TimedEvent};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Barrier;

struct VoicesPtr(*mut Voice);
unsafe impl Send for VoicesPtr {}
impl Clone for VoicesPtr {
    fn clone(&self) -> Self {
        VoicesPtr(self.0)
    }
}

impl Piano {
    /// Offline/bounce rendering: identical semantics and bit-identical
    /// output to `process`, with voice rendering spread over `workers`
    /// threads. Not real-time-safe (see module docs).
    pub fn process_multicore(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32], workers: usize) {
        let workers = workers.clamp(1, 32);
        if workers <= 1 {
            return self.process(events, out_l, out_r);
        }
        let len = out_l.len().min(out_r.len());
        debug_assert!(events.windows(2).all(|w| w[0].offset <= w[1].offset));
        let core = &mut self.core;
        let keys = &self.keys[..];
        let n_voices = self.voices.len();
        let voices_ptr = VoicesPtr(self.voices.as_mut_ptr());
        let barrier = Barrier::new(workers + 1);
        let chunk_n = AtomicUsize::new(0);
        let done = AtomicBool::new(false);
        let path = core.path;

        std::thread::scope(|scope| {
            for w in 0..workers {
                let vp = voices_ptr.clone();
                let barrier = &barrier;
                let chunk_n = &chunk_n;
                let done = &done;
                scope.spawn(move || {
                    let vp = vp; // move the pointer wrapper into the thread
                    loop {
                        barrier.wait(); // chunk start
                        if done.load(Ordering::Acquire) {
                            break;
                        }
                        let n = chunk_n.load(Ordering::Acquire);
                        let mut i = w;
                        while i < n_voices {
                            // SAFETY: workers own disjoint index sets (i ≡ w
                            // mod workers) and only touch them inside the
                            // barrier window; the main thread does not.
                            let v = unsafe { &mut *vp.0.add(i) };
                            if v.active {
                                v.render(&keys[v.key_idx], path, n);
                            }
                            i += workers;
                        }
                        barrier.wait(); // chunk end
                    }
                });
            }

            let mut pos = 0usize;
            let mut ev = 0usize;
            while pos < len {
                {
                    // SAFETY: workers are parked at the start barrier; the
                    // slice dies before we release them.
                    let voices = unsafe { std::slice::from_raw_parts_mut(voices_ptr.0, n_voices) };
                    if core.global_sample % MAX_CHUNK as u64 == 0 {
                        core.control_tick(keys, voices);
                    }
                    while ev < events.len() && (events[ev].offset as usize) <= pos {
                        core.apply_event(keys, voices, &events[ev].event);
                        ev += 1;
                    }
                }
                let next_ev = events.get(ev).map(|e| (e.offset as usize).min(len)).unwrap_or(len);
                let room = MAX_CHUNK - (core.global_sample % MAX_CHUNK as u64) as usize;
                let n = (len - pos).min(next_ev - pos).min(room);
                chunk_n.store(n, Ordering::Release);
                barrier.wait(); // release workers into the chunk
                barrier.wait(); // wait for them to finish it
                {
                    // SAFETY: workers are parked again.
                    let voices = unsafe { std::slice::from_raw_parts_mut(voices_ptr.0, n_voices) };
                    core.finish_chunk(keys, voices, n, &mut out_l[pos..pos + n], &mut out_r[pos..pos + n]);
                }
                pos += n;
                core.global_sample += n as u64;
            }
            done.store(true, Ordering::Release);
            barrier.wait(); // release workers to exit
        });
    }
}
