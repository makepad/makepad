//! Compute kernels: per-element Splash programs over host buffers, compiled
//! ahead of time (see `lower::kernel` for the language side).
//!
//! ```ignore
//! let k = kernel::compile(src)?;
//! let mut call = k.call();
//! call.set_param("amp", 2.0);
//! call.input("hf", &heights)?;
//! call.output("pos", &mut positions)?;
//! let stats = call.run(count)?;          // or run_parallel(count, threads)
//! ```
//!
//! Elements run in fixed chunks of [`CHUNK`]; a reduce kernel combines its
//! chunk partials in chunk order, so every result is bit-identical whatever
//! the backend, thread count or scheduling.

use crate::ir::{self, Program, Region};
use crate::lower::kernel as kl;
pub use crate::lower::kernel::{
    Access, BufferDecl, FieldTy, KernelKind, Layout, LayoutField, ReduceOp, K_ACC, K_BASE, K_CANCEL, K_COUNT, K_OVERFLOW, K_PARAMS, K_SEED,
    K_TIME,
};
pub use crate::lower::ParamInfo;
use crate::{Backend, ShaderError};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Elements per chunk (the unit of scheduling and of reduce partials).
pub const CHUNK: usize = 4096;

/// The kernel prelude: hashing, noise, quaternions, matrices, curves and
/// buffer sampling, available to every kernel (only what is called is
/// compiled in).
pub const KERNEL_PRELUDE: &str = include_str!("kernel_prelude.splash");

/// A compiled kernel. Immutable and shareable across threads.
pub struct Kernel {
    pub kind: KernelKind,
    /// The entry's name: vertex, instance, element, primitive, reduce_*.
    pub entry: String,
    params: Vec<ParamInfo>,
    buffers: Vec<BufferDecl>,
    shared: Box<[u32]>,
    program: Program,
    /// Worst-case AIR ops per element.
    pub cost: u64,
    /// Element ranges may run on different threads.
    pub parallel_safe: bool,
    #[cfg(target_arch = "aarch64")]
    native: Option<crate::arm64::Code>,
}

impl std::fmt::Debug for Kernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Kernel({} {:?}, {} params, {} buffers, {:?})", self.entry, self.kind, self.params.len(), self.buffers.len(), self.backend())
    }
}

pub fn compile(src: &str) -> Result<Arc<Kernel>, Vec<ShaderError>> {
    compile_with(src, &[], Backend::Native)
}

/// Compiles with host layouts (GPU vertex/instance structs a kernel may
/// write by field) for a backend.
pub fn compile_with(src: &str, layouts: &[Layout], backend: Backend) -> Result<Arc<Kernel>, Vec<ShaderError>> {
    let toks = crate::parse::lex(src).map_err(|e| vec![e])?;
    let items = crate::parse::Parser::new(&toks).items().map_err(|e| vec![e])?;
    let all = crate::with_prelude(&items, crate::parse_prelude(KERNEL_PRELUDE, src.len()));
    let lowered = kl::lower_kernel(&all, src.len() + 1, layouts).map_err(|e| vec![e])?;
    let ctx_words = K_PARAMS as usize + lowered.params.len();
    let shared_words = lowered.shared_init.len().max(1);
    let sizes = move |r: Region| -> u32 {
        match r {
            Region::Ctx => ctx_words as u32,
            Region::State => 1,
            Region::Shared => shared_words as u32,
            Region::Frame | Region::Buf(_) => u32::MAX,
        }
    };
    if let Err(e) = ir::validate(&lowered.program, &sizes) {
        return Err(vec![ShaderError::new(0, 1, format!("internal compiler error: {}", e))]);
    }
    let mut shared = lowered.shared_init.clone();
    if shared.is_empty() {
        shared.push(0);
    }
    if let Some(init) = &lowered.init {
        let mut ctx = vec![0u32; ctx_words];
        let mut state = [0u32; 1];
        let mut scratch = vec![0u32; init.scratch_words()];
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        let mut mem = ir::Mem { ctx: &mut ctx, state: &mut state, shared: &mut shared, bufs: &mut [] };
        let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
        ir::run(init, &mut scratch, &mut mem, &mut io, 1);
    }
    #[cfg(target_arch = "aarch64")]
    let native = if backend == Backend::Native { crate::arm64::compile(&lowered.program) } else { None };
    #[cfg(not(target_arch = "aarch64"))]
    let _ = backend;
    Ok(Arc::new(Kernel {
        kind: lowered.kind,
        entry: lowered.entry,
        params: lowered.params,
        buffers: lowered.buffers,
        shared: shared.into_boxed_slice(),
        program: lowered.program,
        cost: lowered.cost,
        parallel_safe: lowered.parallel_safe,
        #[cfg(target_arch = "aarch64")]
        native,
    }))
}

/// Why a call could not run.
#[derive(Clone, Debug, PartialEq)]
pub enum KernelError {
    /// A buffer the kernel uses was not bound (its name).
    Unbound(String),
    /// A bound buffer is empty.
    Empty(String),
    /// No buffer of that name in the kernel.
    NoSuchBuffer(String),
    /// Bound read-only but the kernel writes it.
    ReadOnly(String),
    /// Cancelled (by the watchdog or the owner) before it finished.
    Cancelled,
}

impl std::fmt::Display for KernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KernelError::Unbound(n) => write!(f, "buffer `{}` is not bound", n),
            KernelError::Empty(n) => write!(f, "buffer `{}` is empty", n),
            KernelError::NoSuchBuffer(n) => write!(f, "the kernel has no buffer `{}`", n),
            KernelError::ReadOnly(n) => write!(f, "buffer `{}` is written by the kernel but was bound read-only", n),
            KernelError::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// What a finished call reports.
#[derive(Clone, Debug, Default)]
pub struct RunStats {
    pub elements: usize,
    /// An emit found an element's slots full (records were dropped).
    pub overflowed: bool,
    /// A reduce kernel's result (its lanes).
    pub reduced: Vec<f32>,
    pub nanos: u64,
}

impl Kernel {
    pub fn params(&self) -> &[ParamInfo] {
        &self.params
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }

    pub fn buffers(&self) -> &[BufferDecl] {
        &self.buffers
    }

    pub fn buffer_index(&self, name: &str) -> Option<usize> {
        self.buffers.iter().position(|b| b.name == name)
    }

    pub fn backend(&self) -> Backend {
        #[cfg(target_arch = "aarch64")]
        if self.native.is_some() {
            return Backend::Native;
        }
        Backend::Interp
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Ctx words: base, count, time, seed, cancel, overflow, reduce lanes,
    /// then the params.
    pub fn ctx_words(&self) -> usize {
        K_PARAMS as usize + self.params.len()
    }

    /// A call with every param at its default.
    pub fn call(&self) -> Call<'_> {
        let mut ctx = vec![0u32; self.ctx_words()];
        for (k, p) in self.params.iter().enumerate() {
            ctx[K_PARAMS as usize + k] = p.default.to_bits();
        }
        Call { kernel: self, ctx, bufs: vec![None; self.buffers.len()], cancel: Arc::new(AtomicU32::new(0)), _borrow: std::marker::PhantomData }
    }

    fn reduce_init(&self) -> (ReduceOp, usize, f32) {
        match self.kind {
            KernelKind::Reduce(op, n) => (
                op,
                n as usize,
                match op {
                    ReduceOp::Sum => 0.0,
                    ReduceOp::Min => f32::INFINITY,
                    ReduceOp::Max => f32::NEG_INFINITY,
                },
            ),
            KernelKind::Map => (ReduceOp::Sum, 0, 0.0),
        }
    }

    /// Runs elements [start, start + n) on this thread with `ctx` (the
    /// caller's copy) and the buffer table.
    fn run_range(&self, ctx: &mut [u32], table: &[u64], lens: &[usize], start: usize, n: usize, interp: bool, cancel: &AtomicU32) -> Result<(), KernelError> {
        let step = if interp { 256 } else { CHUNK };
        let mut at = 0;
        while at < n {
            if cancel.load(Ordering::Relaxed) != 0 {
                return Err(KernelError::Cancelled);
            }
            let k = (n - at).min(step);
            ctx[K_BASE as usize] = (start + at) as u32;
            self.run_raw(ctx, table, lens, k, interp);
            if ctx[K_CANCEL as usize] != 0 {
                return Err(KernelError::Cancelled);
            }
            at += k;
        }
        Ok(())
    }

    fn run_raw(&self, ctx: &mut [u32], table: &[u64], lens: &[usize], n: usize, interp: bool) {
        #[cfg(target_arch = "aarch64")]
        if !interp {
            if let Some(code) = &self.native {
                let mut state = [0u32; 1];
                // SAFETY: the table holds the bound buffers' live pointers
                // and lengths (at least two entries); every access clamps
                // into them; ctx has ctx_words() words.
                unsafe { code.run_kernel(ctx.as_mut_ptr(), state.as_mut_ptr(), self.shared.as_ptr() as *mut u32, table.as_ptr(), n as u32) };
                return;
            }
        }
        let _ = interp;
        let mut scratch = vec![0u32; self.program.scratch_words()];
        let mut state = [0u32; 1];
        let shared = unsafe { std::slice::from_raw_parts_mut(self.shared.as_ptr() as *mut u32, self.shared.len()) };
        let mut bufs: Vec<&mut [u32]> =
            lens.iter().enumerate().map(|(k, len)| unsafe { std::slice::from_raw_parts_mut(table[2 * k] as *mut u32, *len) }).collect();
        let mut mem = ir::Mem { ctx, state: &mut state, shared, bufs: &mut bufs };
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
        ir::run(&self.program, &mut scratch, &mut mem, &mut io, n as u32);
    }
}

/// One invocation: params and bound buffers, borrowed for `'a`.
pub struct Call<'a> {
    kernel: &'a Kernel,
    ctx: Vec<u32>,
    /// (pointer, length in words) per kernel buffer.
    bufs: Vec<Option<(*mut u32, usize)>>,
    cancel: Arc<AtomicU32>,
    _borrow: std::marker::PhantomData<&'a mut [u32]>,
}

// SAFETY: the raw pointers are borrows held for 'a; a call is moved, not
// shared, and parallel runs only split element-local work.
unsafe impl Send for Call<'_> {}

/// Cancels a running call from another thread (the watchdog).
#[derive(Clone)]
pub struct CancelToken(Arc<AtomicU32>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(1, Ordering::Relaxed);
    }
}

impl<'a> Call<'a> {
    pub fn set_param(&mut self, name: &str, value: f32) -> bool {
        match self.kernel.param_index(name) {
            Some(k) => {
                let p = &self.kernel.params[k];
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

    pub fn cancel_token(&self) -> CancelToken {
        CancelToken(self.cancel.clone())
    }

    fn bind(&mut self, name: &str, ptr: *mut u32, len: usize, writable: bool) -> Result<(), KernelError> {
        let k = self.kernel.buffer_index(name).ok_or_else(|| KernelError::NoSuchBuffer(name.into()))?;
        if len == 0 {
            return Err(KernelError::Empty(name.into()));
        }
        if !writable && self.kernel.buffers[k].access != Access::Read {
            return Err(KernelError::ReadOnly(name.into()));
        }
        self.bufs[k] = Some((ptr, len));
        Ok(())
    }

    /// Binds a read-only buffer.
    pub fn input(&mut self, name: &str, data: &'a [f32]) -> Result<(), KernelError> {
        self.bind(name, data.as_ptr() as *mut u32, data.len(), false)
    }

    pub fn input_u32(&mut self, name: &str, data: &'a [u32]) -> Result<(), KernelError> {
        self.bind(name, data.as_ptr() as *mut u32, data.len(), false)
    }

    /// Binds a buffer the kernel writes (and may read).
    pub fn output(&mut self, name: &str, data: &'a mut [f32]) -> Result<(), KernelError> {
        self.bind(name, data.as_mut_ptr() as *mut u32, data.len(), true)
    }

    pub fn output_u32(&mut self, name: &str, data: &'a mut [u32]) -> Result<(), KernelError> {
        self.bind(name, data.as_mut_ptr(), data.len(), true)
    }

    fn table(&self) -> Result<(Vec<u64>, Vec<usize>), KernelError> {
        let mut table = Vec::with_capacity(2 * self.bufs.len().max(2));
        let mut lens = Vec::new();
        for (k, b) in self.bufs.iter().enumerate() {
            let (p, len) = b.ok_or_else(|| KernelError::Unbound(self.kernel.buffers[k].name.clone()))?;
            table.push(p as u64);
            table.push(len as u64);
            lens.push(len);
        }
        // The native prologue reads four words.
        while table.len() < 4 {
            table.push(0);
        }
        Ok((table, lens))
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

    fn run_with(&mut self, count: usize, interp: bool) -> Result<RunStats, KernelError> {
        let t0 = std::time::Instant::now();
        let (table, lens) = self.table()?;
        self.ctx[K_COUNT as usize] = count as u32;
        self.ctx[K_OVERFLOW as usize] = 0;
        self.ctx[K_CANCEL as usize] = 0;
        let (op, lanes, init) = self.kernel.reduce_init();
        let mut reduced = vec![init; lanes];
        let mut at = 0;
        while at < count {
            let k = (count - at).min(CHUNK);
            for l in 0..lanes {
                self.ctx[K_ACC as usize + l] = init.to_bits();
            }
            let mut ctx = std::mem::take(&mut self.ctx);
            let r = self.kernel.run_range(&mut ctx, &table, &lens, at, k, interp, &self.cancel);
            self.ctx = ctx;
            r?;
            for (l, acc) in reduced.iter_mut().enumerate() {
                *acc = combine(op, *acc, f32::from_bits(self.ctx[K_ACC as usize + l]));
            }
            at += k;
        }
        Ok(RunStats { elements: count, overflowed: self.ctx[K_OVERFLOW as usize] != 0, reduced, nanos: t0.elapsed().as_nanos() as u64 })
    }

    /// Runs `count` elements split across `threads` threads (chunk-aligned
    /// ranges). Kernels that are not element-local run on one thread.
    pub fn run_parallel(&mut self, count: usize, threads: usize) -> Result<RunStats, KernelError> {
        if threads <= 1 || !self.kernel.parallel_safe || count <= CHUNK {
            return self.run(count);
        }
        let t0 = std::time::Instant::now();
        let (table, lens) = self.table()?;
        self.ctx[K_COUNT as usize] = count as u32;
        let chunks = count.div_ceil(CHUNK);
        let (op, lanes, init) = self.kernel.reduce_init();
        let next = AtomicU32::new(0);
        let results: Vec<std::sync::Mutex<Option<(Vec<u32>, bool)>>> = (0..chunks).map(|_| std::sync::Mutex::new(None)).collect();
        let failed = AtomicU32::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads.min(chunks) {
                s.spawn(|| {
                    let mut ctx = self.ctx.clone();
                    ctx[K_CANCEL as usize] = 0;
                    loop {
                        let c = next.fetch_add(1, Ordering::Relaxed) as usize;
                        if c >= chunks {
                            break;
                        }
                        ctx[K_OVERFLOW as usize] = 0;
                        for l in 0..lanes {
                            ctx[K_ACC as usize + l] = init.to_bits();
                        }
                        let start = c * CHUNK;
                        let k = (count - start).min(CHUNK);
                        if self.kernel.run_range(&mut ctx, &table, &lens, start, k, false, &self.cancel).is_err() {
                            failed.store(1, Ordering::Relaxed);
                            break;
                        }
                        let acc = ctx[K_ACC as usize..K_ACC as usize + lanes].to_vec();
                        *results[c].lock().unwrap() = Some((acc, ctx[K_OVERFLOW as usize] != 0));
                    }
                });
            }
        });
        if failed.load(Ordering::Relaxed) != 0 {
            return Err(KernelError::Cancelled);
        }
        let mut reduced = vec![init; lanes];
        let mut overflowed = false;
        for r in &results {
            let (acc, of) = r.lock().unwrap().take().unwrap();
            overflowed |= of;
            for (l, a) in reduced.iter_mut().enumerate() {
                *a = combine(op, *a, f32::from_bits(acc[l]));
            }
        }
        Ok(RunStats { elements: count, overflowed, reduced, nanos: t0.elapsed().as_nanos() as u64 })
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
