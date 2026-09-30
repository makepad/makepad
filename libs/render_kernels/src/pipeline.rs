//! Multi-pass kernels (KERNELS.md §3.5.3 "Multi-pass pipelines"): passes
//! chained by buffer name, a barrier between passes, `repeat` with
//! ping-pong pairs, and emitted records compacted in element order.
//!
//! The pipeline owns its named buffers (u32 words). For each pass, every
//! buffer the kernel declares is bound by name:
//! - an `input(..)` reads the pipeline buffer of that name (a host input,
//!   or an earlier pass's output: it is moved into the job, never copied);
//! - an `output(..)` writes the pipeline buffer of that name, created zeroed
//!   for `count` records when it does not exist yet;
//! - an `emit_buffer(..)` gets per-element slots; after the pass they are
//!   compacted in element order ([`compact_into`]) into the pipeline buffer
//!   of that name, and [`Pipeline::emitted`] gives the record count. Emit
//!   overflow is an error, never a shorter output.
//!
//! A pass may read any element of its inputs (neighbour reads for normals,
//! smoothing, generations); only its writes are element-local, which the
//! compiler proves, and a pass that is not runs on one thread. With
//! `repeat(n)` the whole pass list runs n times; a ping-pong pair (a pass
//! reads state `a` and writes the next state `b`) swaps its buffers after
//! every repetition, so `a` always holds the newest state.

use makepad_script_compute::kernel::{Access, Kernel, KernelError};
use makepad_script_compute::pipeline::compact_into;
use makepad_script_compute::sched::{Executor, Job, JobError};
use std::collections::HashMap;
use std::sync::Arc;

/// How many elements a pass runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Count {
    Fixed(usize),
    /// One element per record of a pipeline buffer (`stride` words each).
    Records { buffer: String, stride: usize },
    /// One element per record an earlier pass emitted into `buffer`.
    Emitted(String),
}

pub struct Pass {
    job: Option<Job>,
    count: Count,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PipelineError {
    /// A pass reads a buffer nothing provided.
    Missing { pass: usize, buffer: String },
    /// A pass's count names a buffer or emit that does not exist.
    NoCount { pass: usize },
    /// An element found its emit slots full.
    Overflow { pass: usize, buffer: String },
    Kernel { pass: usize, error: KernelError },
    Job { pass: usize, error: JobError },
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Missing { pass, buffer } => write!(f, "pass {} reads `{}`, which nothing provides", pass, buffer),
            PipelineError::NoCount { pass } => write!(f, "pass {}: its element count names no buffer", pass),
            PipelineError::Overflow { pass, buffer } => write!(f, "pass {}: an element emitted more records into `{}` than it has slots", pass, buffer),
            PipelineError::Kernel { pass, error } => write!(f, "pass {}: {}", pass, error),
            PipelineError::Job { pass, error } => write!(f, "pass {}: {}", pass, error),
        }
    }
}

#[derive(Default)]
pub struct Pipeline {
    passes: Vec<Pass>,
    repeat: usize,
    pingpong: Vec<(String, String)>,
    buffers: HashMap<String, Vec<u32>>,
    emitted: HashMap<String, usize>,
    counts: Vec<u32>,
}

impl Pipeline {
    pub fn new() -> Self {
        Pipeline { repeat: 1, ..Default::default() }
    }

    /// Appends a pass; returns its job to set params, time and seed.
    pub fn pass(&mut self, kernel: Arc<Kernel>, count: Count) -> &mut Job {
        self.passes.push(Pass { job: Some(Job::new(kernel, 0)), count });
        self.passes.last_mut().unwrap().job.as_mut().unwrap()
    }

    /// The job of pass `k` (params between runs).
    pub fn job(&mut self, k: usize) -> Option<&mut Job> {
        self.passes.get_mut(k)?.job.as_mut()
    }

    /// Runs the pass list `n` times.
    pub fn repeat(&mut self, n: usize) -> &mut Self {
        self.repeat = n.max(1);
        self
    }

    /// Swaps buffers `a` and `b` after every repetition.
    pub fn ping_pong(&mut self, a: &str, b: &str) -> &mut Self {
        self.pingpong.push((a.to_string(), b.to_string()));
        self
    }

    /// A host buffer (an input, or an output's initial contents).
    pub fn set_buffer(&mut self, name: &str, words: Vec<u32>) {
        self.buffers.insert(name.to_string(), words);
    }

    pub fn buffer(&self, name: &str) -> Option<&[u32]> {
        self.buffers.get(name).map(|v| &v[..])
    }

    pub fn take_buffer(&mut self, name: &str) -> Option<Vec<u32>> {
        self.buffers.remove(name)
    }

    /// Records the last run emitted into `name`.
    pub fn emitted(&self, name: &str) -> Option<usize> {
        self.emitted.get(name).copied()
    }

    /// Runs every pass (each on up to `threads` workers of `exec`).
    pub fn run(&mut self, exec: &dyn Executor, threads: usize) -> Result<(), PipelineError> {
        self.run_with(exec, threads, &mut |mut job: Job| match job.run(exec, threads) {
            Ok(_) => Ok(job),
            Err(e) => Err((job, JobError::Kernel(e))),
        })
    }

    /// Runs every pass through `engine` at `priority` (the scheduler's
    /// admission, watchdog and priority classes apply to each pass).
    pub fn run_on(&mut self, engine: &crate::KernelEngine, priority: makepad_script_compute::sched::Priority, budget: std::time::Duration) -> Result<(), PipelineError> {
        self.run_with(engine.executor(), engine.threads(), &mut |job: Job| engine.run(job, priority, budget))
    }

    fn run_with(&mut self, exec: &dyn Executor, threads: usize, run: &mut dyn FnMut(Job) -> Result<Job, (Job, JobError)>) -> Result<(), PipelineError> {
        self.emitted.clear();
        for _ in 0..self.repeat {
            for p in 0..self.passes.len() {
                self.run_pass(p, exec, threads, run)?;
            }
            // `b` was written: it becomes the state the next repetition
            // (or the caller) reads as `a`.
            for (a, b) in &self.pingpong {
                let va = self.buffers.remove(a);
                let vb = self.buffers.remove(b);
                if let Some(v) = vb {
                    self.buffers.insert(a.clone(), v);
                }
                if let Some(v) = va {
                    self.buffers.insert(b.clone(), v);
                }
            }
        }
        Ok(())
    }

    fn run_pass(&mut self, p: usize, exec: &dyn Executor, threads: usize, run: &mut dyn FnMut(Job) -> Result<Job, (Job, JobError)>) -> Result<(), PipelineError> {
        let mut job = self.passes[p].job.take().expect("a pass's job is parked between runs");
        let count = match &self.passes[p].count {
            Count::Fixed(n) => Some(*n),
            Count::Records { buffer, stride } => self.buffers.get(buffer).map(|b| b.len() / (*stride).max(1)),
            Count::Emitted(name) => self.emitted.get(name).copied(),
        };
        let Some(count) = count else {
            self.passes[p].job = Some(job);
            return Err(PipelineError::NoCount { pass: p });
        };
        job.set_count(count);
        let kernel = job.kernel().clone();
        let mut emits: Vec<(String, usize, usize)> = Vec::new();
        let mut bound: Vec<String> = Vec::new();
        let mut fail = None;
        for b in kernel.buffers().iter().skip(1) {
            let r = match b.access {
                Access::Read => match self.buffers.remove(&b.name) {
                    Some(v) => job.input_vec_u32(&b.name, v),
                    None => {
                        fail = Some(PipelineError::Missing { pass: p, buffer: b.name.clone() });
                        break;
                    }
                },
                Access::Write => {
                    let words = count * b.stride as usize;
                    let mut v = self.buffers.remove(&b.name).unwrap_or_default();
                    if v.len() < words {
                        v.resize(words, 0);
                    }
                    job.output_u32(&b.name, v)
                }
                Access::Emit { width, capacity } => {
                    let mut v = self.buffers.remove(&format!("{}#slots", b.name)).unwrap_or_default();
                    v.clear();
                    v.resize((count * width as usize * capacity as usize).max(1), 0);
                    emits.push((b.name.clone(), width as usize, capacity as usize));
                    job.output_u32(&b.name, v)
                }
                Access::EmitCount => {
                    let mut v = std::mem::take(&mut self.counts);
                    v.clear();
                    v.resize(count.max(1), 0);
                    job.output_u32(&b.name, v)
                }
            };
            bound.push(b.name.clone());
            if let Err(e) = r {
                fail = Some(PipelineError::Kernel { pass: p, error: e });
                break;
            }
        }
        let result = match fail {
            Some(e) => Err((job, e)),
            None if count == 0 => Ok(job),
            None => run(job).map_err(|(job, e)| (job, PipelineError::Job { pass: p, error: e })),
        };
        let (mut job, err) = match result {
            Ok(job) => (job, None),
            Err((job, e)) => (job, Some(e)),
        };
        let overflowed = job.stats().overflowed;
        // Every buffer goes back to the pipeline, emits compacted.
        for name in bound {
            let Some(v) = job.take_buffer_u32(&name) else { continue };
            if let Some((_, width, capacity)) = emits.iter().find(|e| e.0 == name) {
                if count > 0 && err.is_none() {
                    let counts = job.take_buffer_u32(&format!("{}_count", name)).unwrap_or_default();
                    let mut dense = self.buffers.remove(&name).unwrap_or_default();
                    let n = compact_into(&v, &counts[..count.min(counts.len())], *width, *capacity, &mut dense, exec, threads);
                    self.emitted.insert(name.clone(), n);
                    self.buffers.insert(name.clone(), dense);
                    self.counts = counts;
                } else {
                    self.emitted.insert(name.clone(), 0);
                    self.buffers.insert(name.clone(), Vec::new());
                }
                self.buffers.insert(format!("{}#slots", name), v);
            } else if name.ends_with("_count") && emits.iter().any(|e| format!("{}_count", e.0) == name) {
                self.counts = v;
            } else {
                self.buffers.insert(name, v);
            }
        }
        self.passes[p].job = Some(job);
        if let Some(e) = err {
            return Err(e);
        }
        if overflowed {
            let buffer = emits.first().map(|e| e.0.clone()).unwrap_or_default();
            return Err(PipelineError::Overflow { pass: p, buffer });
        }
        Ok(())
    }
}
