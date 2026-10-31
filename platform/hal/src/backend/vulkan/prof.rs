//! The GPU profiler's Vulkan side (`crate::gpu_prof`): a profiled frame's batch writes a
//! timestamp pair around every render pass and copy, in the batch's own query pool (sized for
//! the frame when the batch is made, reused with its slot), and the completion worker turns the
//! pairs into host-clock times once the batch's fence signalled.
//!
//! A split frame cuts each render pass into one render pass per run of draws of one pipeline on
//! the same framebuffer (every run after the first loads what the one before stored), plus one
//! empty render pass that loads and stores the attachments (the baseline), each timed alone.
//! Timestamps are written at the bottom of the pipe on both sides: each waits for the work before
//! it, so the times of consecutive runs tile their pass and the shares of a split pass sum to it.

use super::device::Device;
use crate::gpu_prof::{self, FrameKey, PassGpu, SplitRuns};
use crate::types::*;
use makepad_vulkan::vk;

/// One timed pass or copy of a profiled batch: its index in the recorded frame and its first
/// query (start; end is the next), or (a split pass) the queries of its runs.
pub(super) struct ProfPass {
    pub pass: u16,
    pub query: Option<u32>,
    pub split: Option<SplitRuns<u32>>,
}

/// The profiler's part of a batch that encodes a recorded frame.
pub(super) struct BatchProf {
    pub key: FrameKey,
    /// queries in the batch's pool (0: no timestamps on this queue)
    capacity: u32,
    /// queries reserved so far; 0 and 1 are the batch's own start and end
    used: u32,
    pub passes: Vec<ProfPass>,
}

/// Queries a profiled frame's batch asks for: its own two, a pair per pass and per copy, and on
/// a split frame a pair per run of draws and per baseline (at most one run per draw).
pub(super) fn queries_for(key: FrameKey, passes: usize, draws: usize) -> u32 {
    // the copies of a frame: mip chains, the scanout or composition copy, a capture
    let copies = passes + 4;
    let split = if key.split { draws + passes } else { 0 };
    (2 + 2 * (passes + copies + split)) as u32
}

impl BatchProf {
    pub fn new(key: FrameKey, capacity: u32) -> Self {
        BatchProf { key, capacity, used: 2, passes: Vec::new() }
    }

    /// The first of `pairs` free query pairs, `None` when the pool cannot hold them.
    pub fn reserve(&mut self, pairs: u32) -> Option<u32> {
        let q = self.used;
        (q + 2 * pairs <= self.capacity).then(|| {
            self.used += 2 * pairs;
            q
        })
    }

    /// The batch completed (or failed): every timed pass's GPU result goes to the recorder.
    pub fn finish(self, dev: &Device, pool: vk::QueryPool, failed: bool) {
        let timed = self.passes.len();
        let mut values = vec![0u64; self.used as usize];
        let read = !failed && pool != vk::QueryPool::null() && self.used > 2 && unsafe { dev.raw.get_query_pool_results(pool, 0, &mut values, vk::QueryResultFlags::TYPE_64) }.is_ok();
        if !read {
            for _ in 0..timed {
                gpu_prof::gpu_none(self.key);
            }
            return;
        }
        let bits = dev.timestamp_bits;
        let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        let period = dev.properties.limits.timestamp_period as f64;
        // The device clock placed on the host's: a calibrated pair sampled now, else the batch's
        // own end at the time its fence was seen (late by the completion worker's wake-up).
        let (anchor_ticks, anchor_ns) = dev.calibrate().unwrap_or((values[1], crate::clock_ns()));
        // every timestamp of the batch is before the anchor: host time = anchor - elapsed ticks
        let host = |t: u64| anchor_ns.saturating_sub(((anchor_ticks.wrapping_sub(t) & mask) as f64 * period) as u64);
        let span = |q: u32| {
            let (a, b) = (values[q as usize], values[q as usize + 1]);
            // a pair whose end is not after its start (a wrapped or unwritten query) is no time
            let ticks = b.wrapping_sub(a) & mask;
            (ticks < mask / 2).then(|| (host(a), host(a) + (ticks as f64 * period) as u64))
        };
        for p in self.passes {
            let mut g = PassGpu::default();
            if let Some((a, b)) = p.query.and_then(span) {
                (g.start, g.end) = (a, b);
            }
            if let Some(runs) = &p.split {
                runs.result(&mut g, span);
            }
            gpu_prof::gpu_pass(self.key, p.pass, g);
        }
    }
}

/// A pass's attachments for the GPU profiler: "bgra8 clear/store, depth32f load/store", and the
/// bytes they load and store (every Vulkan attachment here is stored).
pub(super) fn describe_attachments(pass: &Pass, colors: &[(usize, u32)], depth: Option<u32>, pixels: u64) -> (String, u64) {
    let load_name = |l: u8| match l { pass_load::LOAD => "load", pass_load::CLEAR => "clear", _ => "dontcare" };
    let mut parts = Vec::new();
    let mut bytes = 0u64;
    for (i, f) in colors {
        let load = pass.load[*i];
        bytes += pixels * format::texel_bytes(*f) * ((load == pass_load::LOAD) as u64 + 1);
        parts.push(format!("{} {}/store", format::name(*f).to_lowercase(), load_name(load)));
    }
    if let Some(f) = depth {
        let load = pass.load[4];
        bytes += pixels * format::texel_bytes(f) * ((load == pass_load::LOAD) as u64 + 1);
        parts.push(format!("{} {}/store", format::name(f).to_lowercase(), load_name(load)));
    }
    (parts.join(", "), bytes)
}
