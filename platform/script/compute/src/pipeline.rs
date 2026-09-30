//! Multi-pass kernels over emitted records: deterministic parallel
//! compaction, and generations (each pass reads the previous generation's
//! records and emits the next: trees, L-systems, growth, routers).
//!
//! Emits land in per-element slots; compaction moves every element's
//! records, in element order, into one dense buffer. The prefix sum runs in
//! fixed chunks (chunk totals in parallel, their running sum in order, then
//! each chunk's copy in parallel), so the result is the same for any thread
//! count.

use crate::kernel::{Kernel, KernelError, CHUNK};
use crate::sched::{Executor, Job};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A destination several workers write disjoint ranges of.
struct Dst {
    ptr: *mut u32,
    len: usize,
}
// SAFETY: every worker writes only its own chunk's range (offsets from the
// prefix sum), within `len`.
unsafe impl Sync for Dst {}

impl Dst {
    fn write(&self, at: usize, src: &[u32]) {
        if at + src.len() <= self.len {
            // SAFETY: in bounds, and no other worker writes this range.
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), self.ptr.add(at), src.len()) };
        }
    }
}

/// Compacts emit slots (`capacity` records of `width` words per element,
/// `counts[e]` used, clamped to `capacity` and to `data`) into `out` in
/// element order. Returns the records written. `out` is resized (it keeps
/// its allocation across calls).
pub fn compact_into(data: &[u32], counts: &[u32], width: usize, capacity: usize, out: &mut Vec<u32>, exec: &dyn Executor, threads: usize) -> usize {
    let n = counts.len().min(data.len() / (width * capacity).max(1));
    let chunks = n.div_ceil(CHUNK);
    let count = |e: usize| (counts[e] as usize).min(capacity);
    // Chunk totals (parallel), then their exclusive running sum (in order).
    let totals: Vec<AtomicU32> = (0..chunks).map(|_| AtomicU32::new(0)).collect();
    let sum_chunk = |c: usize| {
        let s: usize = (c * CHUNK..((c + 1) * CHUNK).min(n)).map(count).sum();
        totals[c].store(s as u32, Ordering::Relaxed);
    };
    run(exec, threads, chunks, &sum_chunk);
    let mut starts = Vec::with_capacity(chunks);
    let mut total = 0usize;
    for t in &totals {
        starts.push(total);
        total += t.load(Ordering::Relaxed) as usize;
    }
    out.clear();
    out.resize(total * width, 0);
    let dst = Dst { ptr: out.as_mut_ptr(), len: out.len() };
    let copy_chunk = |c: usize| {
        let mut at = starts[c] * width;
        for e in c * CHUNK..((c + 1) * CHUNK).min(n) {
            let k = count(e);
            let from = e * capacity * width;
            dst.write(at, &data[from..from + k * width]);
            at += k * width;
        }
    };
    run(exec, threads, chunks, &copy_chunk);
    total
}

/// Runs `f(c)` for every chunk c on up to `threads` workers.
fn run(exec: &dyn Executor, threads: usize, chunks: usize, f: &(dyn Fn(usize) + Sync)) {
    if threads <= 1 || chunks <= 1 {
        for c in 0..chunks {
            f(c);
        }
        return;
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    exec.fan_out(threads.min(chunks), &|_w| loop {
        let c = next.fetch_add(1, Ordering::Relaxed);
        if c >= chunks {
            return;
        }
        f(c);
    });
}

/// Every generation's records, back to back, and where each starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grown {
    /// Records of `width` words: the seed, then generation 1, 2, ...
    pub records: Vec<u32>,
    /// Record index where each generation starts (the seed is 0).
    pub starts: Vec<usize>,
    /// An element found its emit slots full in some generation.
    pub overflowed: bool,
}

/// A kernel run as generations: it reads the previous generation from its
/// input `prev` (records of `width` words, one element per record) and
/// emits the next into its emit buffer `next` (`capacity` slots per
/// element). Buffers named otherwise are bound by `setup`.
pub struct Generations {
    job: Job,
    prev: String,
    next: String,
    width: usize,
    capacity: usize,
    slots: Vec<u32>,
    counts: Vec<u32>,
    dense: Vec<u32>,
}

impl Generations {
    pub fn new(kernel: Arc<Kernel>, prev: &str, next: &str) -> Result<Generations, KernelError> {
        use crate::kernel::Access;
        let find = |n: &str| kernel.buffer_index(n).ok_or_else(|| KernelError::NoSuchBuffer(n.into()));
        let (kp, kn) = (find(prev)?, find(next)?);
        let Access::Emit { width, capacity } = kernel.buffers()[kn].access else {
            return Err(KernelError::NoSuchBuffer(format!("{} (an emit_buffer)", next)));
        };
        if kernel.buffers()[kp].access != Access::Read {
            return Err(KernelError::ReadOnly(prev.into()));
        }
        Ok(Generations { job: Job::new(kernel, 0), prev: prev.into(), next: next.into(), width: width as usize, capacity: capacity as usize, slots: Vec::new(), counts: Vec::new(), dense: Vec::new() })
    }

    /// The job, to bind other buffers and params once.
    pub fn job(&mut self) -> &mut Job {
        &mut self.job
    }

    /// Grows `depth` generations from `seed` (whole records); `setup` runs
    /// before each generation (its index from 1) to set params such as the
    /// generation's time.
    pub fn grow(&mut self, seed: &[u32], depth: usize, exec: &dyn Executor, threads: usize, mut setup: impl FnMut(&mut Job, usize)) -> Result<Grown, KernelError> {
        let w = self.width;
        let mut grown = Grown { records: seed[..seed.len() / w * w].to_vec(), starts: vec![0], overflowed: false };
        let mut prev: Arc<[u32]> = grown.records.clone().into();
        for g in 1..=depth {
            let n = prev.len() / w;
            if n == 0 {
                break;
            }
            self.slots.clear();
            self.slots.resize(n * self.capacity * w, 0);
            self.counts.clear();
            self.counts.resize(n, 0);
            self.job.set_count(n);
            self.job.input_u32(&self.prev, prev.clone())?;
            self.job.output_u32(&self.next, std::mem::take(&mut self.slots))?;
            self.job.output_u32(&format!("{}_count", self.next), std::mem::take(&mut self.counts))?;
            setup(&mut self.job, g);
            let r = self.job.run(exec, threads).map(|s| s.overflowed);
            self.slots = self.job.take_output_u32(&self.next).unwrap_or_default();
            self.counts = self.job.take_output_u32(&format!("{}_count", self.next)).unwrap_or_default();
            grown.overflowed |= r?;
            let made = compact_into(&self.slots, &self.counts, w, self.capacity, &mut self.dense, exec, threads);
            grown.starts.push(grown.records.len() / w);
            grown.records.extend_from_slice(&self.dense[..made * w]);
            prev = self.dense[..made * w].to_vec().into();
        }
        Ok(grown)
    }
}
