//! Load-time precompute (PDOOM-PARITY AK5): a stateful stepper run ahead
//! of time over [t0, t0 + steps · dt], every step's state kept in a
//! history table that render-time code samples by t, so the document stays
//! stateless and adaptive sampling stays legal (`Sim{precompute}`; a
//! `Kernel{once}` is the one-step case).
//!
//! The step kernel reads the previous state as `input(State)` named `prev`
//! and writes the next as `output(State)` named `next` (`count` records of
//! the layout's stride), may read any element of `prev` (neighbours), and
//! sees the step's time as `time`. The runner ping-pongs the two buffers,
//! appends every state to the table, and samples it with
//! `std.anim.history_at` in kernels ([`History::at`] on the host).

use makepad_script_compute::kernel::{Kernel, KernelError};
use makepad_script_compute::sched::{Executor, Job};
use std::sync::Arc;

/// Every step's state: record `i` of step `s` at words
/// `(s * count + i) * stride ..`. Step 0 is the initial state at `t0`.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    pub t0: f32,
    pub dt: f32,
    pub steps: usize,
    pub count: usize,
    pub stride: usize,
    pub table: Vec<u32>,
}

impl History {
    /// Word `word` of element `i` at time `t`, linear between steps and
    /// held at the ends (what `std.anim.history_at` computes).
    pub fn at(&self, i: usize, word: usize, t: f32) -> f32 {
        let f = |s: usize| f32::from_bits(self.table[(s * self.count + i) * self.stride + word]);
        let last = self.steps.saturating_sub(1);
        let x = ((t - self.t0) / self.dt).clamp(0.0, last as f32);
        let k = (x.floor() as usize).min(last.saturating_sub(1));
        if last == 0 {
            return f(0);
        }
        let u = x - k as f32;
        f(k) + (f(k + 1) - f(k)) * u
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PrecomputeError {
    Kernel(KernelError),
    /// A step reported an overflow (an emit past its slots, or a loop past
    /// its cap): the history would be wrong from there.
    Overflow { step: usize },
}

impl std::fmt::Display for PrecomputeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrecomputeError::Kernel(e) => write!(f, "{}", e),
            PrecomputeError::Overflow { step } => write!(f, "step {} overflowed (an emit past its slots or a loop past its cap)", step),
        }
    }
}

impl From<KernelError> for PrecomputeError {
    fn from(e: KernelError) -> Self {
        PrecomputeError::Kernel(e)
    }
}

/// Runs `kernel` from `init` (`count` records of `stride` words) for
/// `steps - 1` steps of `dt` from `t0`; `setup` sets params once. Runs on
/// up to `threads` workers of `exec` per step (a one-element stepper runs
/// on the caller).
pub fn precompute(kernel: Arc<Kernel>, init: Vec<u32>, count: usize, stride: usize, steps: usize, t0: f32, dt: f32, exec: &dyn Executor, threads: usize, setup: impl FnOnce(&mut Job)) -> Result<History, PrecomputeError> {
    let steps = steps.max(1);
    let words = count * stride;
    let mut table = Vec::with_capacity(words * steps);
    let mut prev = init;
    prev.resize(words, 0);
    table.extend_from_slice(&prev);
    let mut job = Job::new(kernel, count);
    setup(&mut job);
    let mut next = vec![0u32; words];
    for s in 1..steps {
        job.set_time(t0 + dt * s as f32);
        job.input_vec_u32("prev", prev)?;
        job.output_u32("next", next)?;
        job.run(exec, threads)?;
        if job.stats().overflowed {
            return Err(PrecomputeError::Overflow { step: s });
        }
        next = job.take_buffer_u32("next").unwrap_or_default();
        prev = job.take_buffer_u32("prev").unwrap_or_default();
        table.extend_from_slice(&next[..words]);
        std::mem::swap(&mut prev, &mut next);
    }
    Ok(History { t0, dt, steps, count, stride, table })
}
