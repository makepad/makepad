//! Caller-owned kernel invocation, deterministic chunk orchestration and errors.
use makepad_script_compute::kernel::{Kernel, ReduceOp, Mode, K_ACC, K_COUNT, K_OVERFLOW, K_PARAMS, K_SEED, K_TIME};
use makepad_script_compute::boundary::BorrowedBuffer;
use makepad_script_compute::kernel::WorkCount;
pub use makepad_script_compute::kernel::{Access, KernelRef};
use makepad_script_compute::lower::kernel as kl;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Elements per chunk (the unit of scheduling and of reduce partials).
pub const CHUNK: usize = 4096;

/// The smallest range a map kernel's call is split into. A map kernel's
/// result does not depend on how its elements are split (each element
/// writes only its own records), so a call of a few thousand elements is
/// shared by every worker in ranges of at least this many (a multiple of 4,
/// so four-wide code runs whole groups); reduce kernels keep fixed
/// [`CHUNK`]s, whose partials combine in order.
pub const MIN_SPLIT: usize = 512;

/// Whether a call of `count` elements may run split across threads (what
/// admission checks buffer sizes for; [`split_threads`] decides how many
/// threads it actually takes).
pub(crate) fn splits(kernel: &Kernel, threads: usize, count: usize) -> bool {
    threads > 1 && kernel.parallel_safe() && count > if kernel.reduce_parts().1 == 0 { MIN_SPLIT } else { CHUNK }
}

/// Work one more thread must get before splitting pays: waking a parked
/// worker and joining it costs about 10-20 us (measured with
/// examples/bench_dispatch: a 4096-element multiply-add took 1.9 us on the
/// caller and 22 us fanned out over 8 threads).
pub const SPLIT_NS: f64 = 25_000.0;

/// Threads a call of `count` elements runs on (1: the caller alone): one
/// per SPLIT_NS of estimated work, at most `threads`. The estimate is the
/// kernel's measured time per element (see [`Kernel::ns_per_element`]).
/// The result never depends on it: any split gives the same bits.
pub(crate) fn split_threads(kernel: &Kernel, threads: usize, count: usize, mode: Mode) -> usize {
    if !splits(kernel, threads, count) {
        return 1;
    }
    let ns = kernel.ns_per_element(mode) * count as f64;
    ((ns / SPLIT_NS) as usize).clamp(1, threads)
}

/// Why a caller-owned kernel invocation could not finish.
#[derive(Clone, Debug, PartialEq)]
pub enum KernelError {
    /// A buffer the kernel uses was not bound (its name).
    Unbound(String),
    /// A bound buffer is empty.
    Empty(String),
    /// A bound buffer has 2^31 words or more.
    TooLarge(String),
    /// No buffer of that name in the kernel.
    NoSuchBuffer(String),
    /// Bound read-only but the kernel writes it.
    ReadOnly(String),
    /// Cancelled by its owner (or pre-empted) before it finished.
    Cancelled,
    /// An output buffer is smaller than the call needs (words).
    TooSmall { name: String, need: u64, have: usize },
    /// More elements than one call may run.
    TooMany(usize),
    /// Stopped: the ops it counted passed the call's limit.
    OverBudget { work: u64, limit: u64 },
}

impl std::fmt::Display for KernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KernelError::Unbound(n) => write!(f, "buffer `{}` is not bound", n),
            KernelError::Empty(n) => write!(f, "buffer `{}` is empty", n),
            KernelError::TooLarge(n) => write!(f, "buffer `{}` has 2^31 words or more", n),
            KernelError::NoSuchBuffer(n) => write!(f, "the kernel has no buffer `{}`", n),
            KernelError::ReadOnly(n) => write!(f, "buffer `{}` is written by the kernel but was bound read-only", n),
            KernelError::Cancelled => write!(f, "cancelled"),
            KernelError::TooSmall { name, need, have } => write!(f, "buffer `{}` has {} words; this call needs {}", name, have, need),
            KernelError::TooMany(n) => write!(f, "{} elements is more than one call may run", n),
            KernelError::OverBudget { work, limit } => write!(f, "stopped after {} ops: its limit is {} ops", work, limit),
        }
    }
}

/// What a finished call reports.
#[derive(Clone, Debug, Default)]
pub struct RunStats {
    pub elements: usize,
    /// An emit found an element's slots full (records were dropped).
    pub overflowed: bool,
    /// A host function call failed (a clamped slice, an error, a panic):
    /// its results were zero.
    pub host_error: bool,
    /// A reduce kernel's result (its lanes).
    pub reduced: Vec<f32>,
    /// The ops it counted (see [`makepad_script_compute::work`]): the same on every machine.
    pub work: u64,
    pub nanos: u64,
}

// Borrowed result accessors for callers that do not need the owned fields.
impl RunStats {
    pub fn elements(&self) -> usize { self.elements }
    pub fn overflowed(&self) -> bool { self.overflowed }
    pub fn host_error(&self) -> bool { self.host_error }
    pub fn reduced(&self) -> &[f32] { &self.reduced }
    pub fn work(&self) -> u64 { self.work }
    pub fn nanos(&self) -> u64 { self.nanos }
}

/// Elapsed time of a run (wasm32 has no clock in std: 0 there, and the
/// host times its calls itself).
struct Clock {
    #[cfg(not(target_arch = "wasm32"))]
    t0: std::time::Instant,
}

impl Clock {
    fn now() -> Clock {
        Clock {
            #[cfg(not(target_arch = "wasm32"))]
            t0: std::time::Instant::now(),
        }
    }

    fn nanos(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        return self.t0.elapsed().as_nanos() as u64;
        #[cfg(target_arch = "wasm32")]
        0
    }
}

/// One chunk's result, written by whichever worker ran it.
#[derive(Default)]
pub(crate) struct ChunkCell {
    acc: [AtomicU32; 16],
}

/// The ctx words a worker copies per chunk (params included).
const MAX_CTX: usize = K_PARAMS as usize + 256;

/// Checks a call before it runs: the element count, and that every buffer
/// written per element holds `count` records (always for emit buffers; for
/// plain outputs when the run is split across threads or four elements run
/// at once, so no two of them ever share a word). `lens[k]` is buffer k's
/// bound length. (Work is counted while it runs, not estimated here.)
pub(crate) fn check(kernel: &Kernel, lens: &[usize], count: usize, split: bool) -> Result<(), KernelError> {
    if count > kl::ELEMENT_CAP as usize {
        return Err(KernelError::TooMany(count));
    }
    for (k, b) in kernel.buffer_views().enumerate().skip(1) {
        let per = match b.access {
            Access::Read => continue,
            Access::Write if !split => continue,
            Access::Write => b.stride as u64,
            Access::Emit { width, capacity } => width as u64 * capacity as u64,
            Access::EmitCount => 1,
        };
        let need = per.saturating_mul(count as u64);
        let have = lens.get(k).copied().unwrap_or(0);
        // Offsets are 32-bit element arithmetic: keep them below 2^31.
        if need > have as u64 || need > i32::MAX as u64 {
            return Err(KernelError::TooSmall { name: b.name.to_string(), need, have });
        }
    }
    Ok(())
}

/// What a finished [`run_chunks`] reports: overflowed, host error, the
/// reduce lanes, the counted ops.
pub(crate) type ChunksDone = (bool, bool, [f32; 16], u64);

/// Runs `count` elements in fixed chunks of [`CHUNK`]: element-local
/// kernels on up to `threads` workers of `exec` (the caller included),
/// others in order on the caller. `ctx` is the call's ctx (count, time,
/// seed, params); every chunk starts from a copy of it, and the chunk
/// results are combined in chunk order, so the outcome is the same for any
/// thread count. `cells` holds at least one cell per chunk. The ops counted
/// past `limit` stop the call ([`KernelError::OverBudget`]). Allocates
/// nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_chunks(
    kernel: &Kernel,
    ctx: &[u32],
    buffers: &[BorrowedBuffer<'_>],
    count: usize,
    mode: Mode,
    cancel: &AtomicU32,
    cells: &[ChunkCell],
    exec: &dyn crate::sched::Executor,
    threads: usize,
    limit: u64,
) -> Result<ChunksDone, KernelError> {
    let (op, lanes, init) = kernel.reduce_parts();
    // Map kernels split into equal ranges, several per thread (any split
    // gives the same bits); reduce kernels into the fixed chunks.
    let unit = if lanes == 0 && threads > 1 { count.div_ceil(threads * 4).next_multiple_of(4).clamp(MIN_SPLIT, CHUNK) } else { CHUNK };
    let chunks = count.div_ceil(unit);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let any_overflow = AtomicBool::new(false);
    let any_host_error = AtomicBool::new(false);
    let counted = WorkCount::new(limit);
    let helpers = if kernel.parallel_safe() { threads.max(1).min(chunks.max(1)) } else { 1 };
    let words = ctx.len().min(MAX_CTX);
    let work = |_worker: usize| {
        let mut buf = [0u32; MAX_CTX];
        let wctx = &mut buf[..words];
        loop {
            if failed.load(Ordering::Relaxed) {
                return;
            }
            let c = next.fetch_add(1, Ordering::Relaxed);
            if c >= chunks {
                return;
            }
            wctx.copy_from_slice(&ctx[..words]);
            wctx[K_OVERFLOW as usize] = 0;
            wctx[kl::K_HOST_ERR as usize] = 0;
            for l in 0..lanes {
                wctx[K_ACC as usize + l] = init.to_bits();
            }
            let start = c * unit;
            let k = (count - start).min(unit);
            if !unsafe { kernel.run_range(wctx, buffers, start, k, mode, cancel.as_ptr(), &counted, helpers) } {
                failed.store(true, Ordering::Relaxed);
                return;
            }
            if wctx[K_OVERFLOW as usize] != 0 {
                any_overflow.store(true, Ordering::Relaxed);
            }
            if wctx[kl::K_HOST_ERR as usize] != 0 {
                any_host_error.store(true, Ordering::Relaxed);
            }
            if lanes > 0 {
                let cell = &cells[c];
                for l in 0..lanes {
                    cell.acc[l].store(wctx[K_ACC as usize + l], Ordering::Relaxed);
                }
            }
        }
    };
    if helpers <= 1 {
        work(0);
    } else {
        exec.fan_out(helpers, &work);
    }
    if counted.over() {
        // The stop was the count's, not the owner's: the token is free again.
        cancel.store(0, Ordering::Relaxed);
        return Err(KernelError::OverBudget { work: counted.used(), limit });
    }
    if failed.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) != 0 {
        return Err(KernelError::Cancelled);
    }
    let mut reduced = [init; 16];
    // fan_out returned: every range's stores happened before (its join).
    let overflowed = any_overflow.load(Ordering::Relaxed);
    let host_error = any_host_error.load(Ordering::Relaxed);
    if lanes > 0 {
        for cell in &cells[..chunks] {
            for (l, a) in reduced.iter_mut().enumerate().take(lanes) {
                *a = combine(op, *a, f32::from_bits(cell.acc[l].load(Ordering::Relaxed)));
            }
        }
    }
    Ok((overflowed, host_error, reduced, counted.used()))
}

/// One invocation: params and bound buffers, borrowed for `'a`.
pub struct Call<'a> {
    kernel: &'a Kernel,
    ctx: Vec<u32>,
    /// (pointer, length in words) per kernel buffer.
    bufs: Vec<Option<BorrowedBuffer<'a>>>,
    cancel: Arc<AtomicU32>,
    work_limit: u64,
    /// Use NEON ×4 code when the call allows it (on by default).
    simd: bool,
    _borrow: std::marker::PhantomData<&'a mut [u32]>,
}

/// A call's work limit unless its owner sets one: the counted ops it may
/// run before it is stopped.
pub const DEFAULT_WORK_LIMIT: u64 = 1 << 40;

// SAFETY: the raw pointers are borrows held for 'a; a call is moved, not
// shared, and parallel runs only split element-local work.
unsafe impl Send for Call<'_> {}

/// Cancels a running call from another thread (its owner).
#[derive(Clone)]
pub struct CancelToken(Arc<AtomicU32>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(1, Ordering::Relaxed);
    }
}

impl<'a> Call<'a> {
    pub fn new(kernel: &'a Kernel) -> Self {
        let mut ctx = vec![0u32; kernel.ctx_words()];
        kernel.init_context(&mut ctx);
        Self { kernel, ctx, bufs: vec![None; kernel.buffer_count()], cancel: Arc::new(AtomicU32::new(0)), work_limit: DEFAULT_WORK_LIMIT, simd: true, _borrow: std::marker::PhantomData }
    }

    pub fn set_param(&mut self, name: &str, value: f32) -> bool {
        match self.kernel.param_index(name) {
            Some(k) => {
                let p = &self.kernel.param_view(k);
                let v = if value.is_finite() { value.clamp(p.min, p.max) } else { p.default };
                self.ctx[K_PARAMS as usize + k] = v.to_bits();
                true
            }
            None => false,
        }
    }

    pub fn set_time(&mut self, t: f32) {
        self.ctx[K_TIME as usize] = t.to_bits();
    }

    pub fn set_seed(&mut self, seed: u32) {
        self.ctx[K_SEED as usize] = seed;
    }

    /// The counted ops this call may run (see [`makepad_script_compute::work`]): past them
    /// it stops with [`KernelError::OverBudget`] (untrusted kernels get the
    /// limit their admission ticket gives).
    pub fn set_work_limit(&mut self, ops: u64) {
        self.work_limit = ops;
    }

    /// The op-equivalents one host call may cost for its actual input (0:
    /// no limit): larger inputs are refused before the call runs, with
    /// `RunStats::host_error` set. Admission sets it for untrusted origins.
    pub fn set_host_call_limit(&mut self, ops: u64) {
        self.ctx[kl::K_HOST_LIMIT as usize] = ops.min(u32::MAX as u64) as u32;
    }

    /// Allows (default) or forbids the NEON ×4 code (differential tests;
    /// results are bit-identical either way).
    pub fn set_simd(&mut self, on: bool) {
        self.simd = on;
    }

    /// The mode a native run of `count` elements takes: vector code only
    /// for element-local kernels whose written buffers hold every record
    /// (four elements run at once).
    fn native_mode(&self, count: usize) -> Mode {
        if self.simd && self.kernel.simd() && self.kernel.parallel_safe() && self.admit(count, true).is_ok() {
            Mode::Vector
        } else {
            Mode::Scalar
        }
    }

    pub fn cancel_token(&self) -> CancelToken {
        CancelToken(self.cancel.clone())
    }

    fn bind(&mut self, name: &str, buffer: BorrowedBuffer<'a>, writable: bool) -> Result<(), KernelError> {
        let len = buffer.len();
        let k = self.kernel.buffer_index(name).filter(|k| *k > 0).ok_or_else(|| KernelError::NoSuchBuffer(name.into()))?;
        if len == 0 {
            return Err(KernelError::Empty(name.into()));
        }
        // Word offsets are 32-bit arithmetic (and every backend reads the
        // length as a 32-bit word).
        if len > i32::MAX as usize {
            return Err(KernelError::TooLarge(name.into()));
        }
        if !writable && self.kernel.buffer_view(k).access != Access::Read {
            return Err(KernelError::ReadOnly(name.into()));
        }
        self.bufs[k] = Some(buffer);
        Ok(())
    }

    /// Binds a read-only buffer.
    pub fn input(&mut self, name: &str, data: &'a [f32]) -> Result<(), KernelError> {
        self.bind(name, BorrowedBuffer::input(unsafe { std::slice::from_raw_parts(data.as_ptr().cast(), data.len()) }), false)
    }

    pub fn input_u32(&mut self, name: &str, data: &'a [u32]) -> Result<(), KernelError> {
        self.bind(name, BorrowedBuffer::input(unsafe { std::slice::from_raw_parts(data.as_ptr().cast(), data.len()) }), false)
    }

    /// Binds a buffer the kernel writes (and may read).
    pub fn output(&mut self, name: &str, data: &'a mut [f32]) -> Result<(), KernelError> {
        self.bind(name, BorrowedBuffer::output(unsafe { std::slice::from_raw_parts_mut(data.as_mut_ptr().cast(), data.len()) }), true)
    }

    pub fn output_u32(&mut self, name: &str, data: &'a mut [u32]) -> Result<(), KernelError> {
        self.bind(name, BorrowedBuffer::output(data), true)
    }

    fn buffers(&self) -> Result<Vec<BorrowedBuffer<'_>>, KernelError> {
        let mut buffers = Vec::with_capacity(self.bufs.len().max(2));
        // SAFETY: self retains the cancel word for the entire synchronous run.
        buffers.push(unsafe { BorrowedBuffer::control(self.cancel.as_ptr()) });
        for (k, b) in self.bufs.iter().enumerate().skip(1) {
            buffers.push(b.ok_or_else(|| KernelError::Unbound(self.kernel.buffer_view(k).name.to_string()))?);
        }
        while buffers.len() < 2 { buffers.push(BorrowedBuffer::empty()); }
        Ok(buffers)
    }

    /// Runs `count` elements on this thread.
    pub fn run(&mut self, count: usize) -> Result<RunStats, KernelError> {
        self.run_with(count, false)
    }

    /// Runs on the reference interpreter (for tests and hosts without
    /// executable memory).
    pub fn run_interp(&mut self, count: usize) -> Result<RunStats, KernelError> {
        self.run_with(count, true)
    }

    fn lens(&self) -> Vec<usize> {
        self.bufs.iter().map(|b| b.map_or(0, |b| b.len())).collect()
    }

    /// Capacity (see [`check`]).
    fn admit(&self, count: usize, split: bool) -> Result<(), KernelError> {
        check(self.kernel, &self.lens(), count, split)
    }

    fn run_with(&mut self, count: usize, interp: bool) -> Result<RunStats, KernelError> {
        self.run_on(count, interp, &crate::sched::InlineExecutor, 1)
    }

    fn run_on(&mut self, count: usize, interp: bool, exec: &dyn crate::sched::Executor, threads: usize) -> Result<RunStats, KernelError> {
        let t0 = Clock::now();
        self.ctx[K_COUNT as usize] = count as u32;
        let buffers = self.buffers()?;
        let split = splits(self.kernel, threads, count);
        self.admit(count, split)?;
        let mode = if interp { Mode::Interp } else { self.native_mode(count) };
        let cells: Vec<ChunkCell> = (0..count.div_ceil(CHUNK)).map(|_| ChunkCell::default()).collect();
        let t = split_threads(self.kernel, threads, count, mode);
        let t1 = Clock::now();
        let (overflowed, host_error, reduced, work) = run_chunks(self.kernel, &self.ctx, &buffers, count, mode, &self.cancel, &cells, exec, t, self.work_limit)?;
        self.kernel.observe(count, t, t1.nanos(), mode);
        let lanes = self.kernel.reduce_parts().1;
        Ok(RunStats { elements: count, overflowed, host_error, reduced: reduced[..lanes].to_vec(), work, nanos: t0.nanos() })
    }

    /// Runs `count` elements split across up to `threads` workers of the
    /// process-wide [`crate::sched::ThreadExecutor`] (chunk-aligned; no
    /// thread is spawned per call). Kernels that are not element-local run
    /// on one thread. The result is the same for any thread count.
    pub fn run_parallel(&mut self, count: usize, threads: usize) -> Result<RunStats, KernelError> {
        self.run_on(count, false, crate::sched::ThreadExecutor::shared(), threads)
    }

    /// Runs on a host executor (Makepad's TaskPool through
    /// [`crate::sched::FnExecutor`]) with up to `threads` workers.
    pub fn run_on_executor(&mut self, count: usize, exec: &dyn crate::sched::Executor, threads: usize) -> Result<RunStats, KernelError> {
        self.run_on(count, false, exec, threads)
    }
}

fn combine(op: ReduceOp, a: f32, b: f32) -> f32 {
    match op {
        ReduceOp::Sum => a + b,
        ReduceOp::Min => {
            if a < b {
                a
            } else {
                b
            }
        }
        ReduceOp::Max => {
            if a > b {
                a
            } else {
                b
            }
        }
    }
}

/// Compacts an emit buffer: element e's `counts[e]` records (of `width`
/// words, `capacity` slots per element) in element order.
pub fn compact(data: &[f32], counts: &[u32], width: usize, capacity: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for (e, c) in counts.iter().enumerate() {
        let n = (*c as usize).min(capacity);
        let at = e * capacity * width;
        out.extend_from_slice(&data[at..at + n * width]);
    }
    out
}


/// Convenience invocation on an immutable compiled kernel.
pub trait KernelExt { fn call(&self) -> Call<'_>; }
impl KernelExt for Kernel { fn call(&self) -> Call<'_> { Call::new(self) } }
