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

use crate::ir::{self, Program};
use crate::lower::kernel as kl;
pub use crate::lower::kernel::{
    Access, BufferDecl, FieldTy, KernelKind, Layout, LayoutField, MathMode, ReduceOp, CANONICAL_NAN, ELEMENT_CAP, K_ACC, K_BASE, K_COUNT, K_OVERFLOW, K_PARAMS, K_SEED,
    K_TIME,
};
pub use crate::lower::ParamInfo;
use crate::{Backend, ShaderError};
use std::sync::atomic::{AtomicU32, Ordering};
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
    threads > 1 && kernel.parallel_safe && count > if kernel.reduce_init().1 == 0 { MIN_SPLIT } else { CHUNK }
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

/// The kernel prelude: hashing, noise, quaternions, matrices, curves and
/// buffer sampling, available to every kernel (only what is called is
/// compiled in).
pub const KERNEL_PRELUDE: &str = include_str!("kernel_prelude.splash");

/// A compiled kernel. Immutable and shareable across threads.
pub struct Kernel {
    pub kind: KernelKind,
    pub math: MathMode,
    /// The entry's name: vertex, instance, element, primitive, reduce_*.
    pub entry: String,
    params: Vec<ParamInfo>,
    buffers: Vec<BufferDecl>,
    shared: Box<[u32]>,
    program: Program,
    /// `program` with its calls inlined and optimized again (the same
    /// bits): what the interpreter and native code run.
    flat: Program,
    /// Worst-case AIR ops per element.
    pub cost: u64,
    /// Element ranges may run on different threads.
    pub parallel_safe: bool,
    #[cfg(target_arch = "aarch64")]
    native: Option<crate::arm64::Code>,
    /// Four elements per iteration (element-local kernels only).
    #[cfg(target_arch = "aarch64")]
    neon: Option<crate::arm64::Code>,
    /// Admission's worst-case element estimate, computed on first admit
    /// (the job path stays allocation-free).
    pub(crate) admission: std::sync::OnceLock<crate::admission::ElementCost>,
    /// Measured ns per element (f32 bits, a moving average; 0: none yet):
    /// how many threads a call is worth.
    speed: AtomicU32,
    /// Function table slots of this kernel's generated wasm (scalar, four
    /// wide; 0: none; a host links one of the two), once the host linked
    /// its document's module (see [`wasm_module`]).
    wasm_slots: [AtomicU32; 2],
    /// Edit mode: literals read from hidden parameters ([`compile_live`]),
    /// their values now, and the live literals that stayed constants.
    live: Box<[crate::lower::LiveParam]>,
    live_values: Box<[AtomicU32]>,
    folded: Box<[usize]>,
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
    compile_with_modules(src, layouts, backend, &[])
}

/// Compiles with host layouts and the modules the kernel's `use` items
/// name (the host resolved each to its text; std modules are built in).
pub fn compile_with_modules(src: &str, layouts: &[Layout], backend: Backend, modules: &[crate::module::Module]) -> Result<Arc<Kernel>, Vec<ShaderError>> {
    compile_live(src, layouts, backend, modules, None)
}

/// [`compile_with_modules`] in an editor's edit mode: the number and colour
/// literals of `src` that `live` accepts (by their byte offset) are read
/// from hidden parameters where the code computes with them, so
/// [`Kernel::set_live`] changes them for the next call with the same code;
/// a live literal that must stay a constant (an integer, a size, a value
/// computed once) is in [`Kernel::folds`]. `None` compiles exactly as
/// [`compile_with_modules`].
pub fn compile_live(src: &str, layouts: &[Layout], backend: Backend, modules: &[crate::module::Module], live: Option<&dyn Fn(usize) -> bool>) -> Result<Arc<Kernel>, Vec<ShaderError>> {
    let toks = crate::parse::lex(src).map_err(|e| vec![e])?;
    let lift = live.map(|live| {
        let mut lift = crate::lower::LiveLift::default();
        for t in &toks {
            let colour = matches!(t.tk, crate::parse::Tk::Color(_));
            if (colour || matches!(t.tk, crate::parse::Tk::Num(..))) && t.start < src.len() && live(t.start) {
                lift.live.insert(t.start);
                if colour {
                    lift.colours.insert(t.start);
                }
            }
        }
        lift
    });
    let items = crate::parse::Parser::new(&toks).items().map_err(|e| vec![e])?;
    let prelude = crate::parse_prelude(KERNEL_PRELUDE, src.len());
    // Module spans go past the prelude's, so errors inside them are
    // reported at the kernel's call like the prelude's.
    // The kernel's own items are the roots; library items (modules, the
    // prelude) are compiled in only when reached.
    let roots: std::collections::HashSet<String> = items.iter().filter(|i| !matches!(i, crate::parse::Item::Use { .. })).map(|i| i.name().to_string()).collect();
    let items = crate::module::resolve(items, src.len() + 1 + KERNEL_PRELUDE.len() + 1, modules, &prelude).map_err(|e| vec![e])?;
    let all = crate::module::prune(crate::with_prelude(&items, prelude, &roots), &roots);
    let lowered = kl::lower_kernel(&all, src.len() + 1, layouts, lift).map_err(|e| vec![e])?;
    let ctx_words = K_PARAMS as usize + lowered.params.len();
    let shared_words = lowered.shared_init.len().max(1);
    let regions = ir::Regions {
        ctx: ctx_words as u32,
        state: 1,
        shared: shared_words as u32,
        frame: lowered.program.frame_words,
        shared_writable: false,
        bufs: lowered.buffers.iter().map(|b| b.access != Access::Read).collect(),
        io: false,
    };
    if let Err(e) = ir::validate(&lowered.program, &regions) {
        return Err(vec![ShaderError::new(0, 1, format!("internal compiler error: {}", e))]);
    }
    let mut shared = lowered.shared_init.clone();
    if shared.is_empty() {
        shared.push(0);
    }
    if let Some(init) = &lowered.init {
        let init_regions = ir::Regions {
            ctx: ctx_words as u32,
            state: 1,
            shared: shared_words as u32,
            frame: init.frame_words,
            shared_writable: true,
            bufs: Vec::new(),
            io: false,
        };
        if let Err(e) = ir::validate(init, &init_regions) {
            return Err(vec![ShaderError::new(0, 1, format!("internal compiler error in init: {}", e))]);
        }
        let mut ctx = vec![0u32; ctx_words];
        let mut state = [0u32; 1];
        let mut scratch = vec![0u32; init.scratch_words()];
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        let mut mem = ir::Mem { ctx: &mut ctx, state: &mut state, shared: ir::Shared::Write(&mut shared), bufs: &[] };
        let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
        ir::run(init, &mut scratch, &mut mem, &mut io, 1);
    }
    let flat = ir::flat(&lowered.program).into_owned();
    #[cfg(target_arch = "aarch64")]
    let native = if backend == Backend::Native {
        let writable = lowered.buffers.iter().enumerate().fold(0u64, |m, (k, b)| if b.access != Access::Read && k < 64 { m | 1 << k } else { m });
        crate::arm64::compile_with(&flat, writable)
    } else {
        None
    };
    #[cfg(target_arch = "aarch64")]
    let neon = if native.is_some() && lowered.parallel_safe { crate::neon::compile(&flat) } else { None };
    #[cfg(not(target_arch = "aarch64"))]
    let _ = backend;
    let live_values: Box<[AtomicU32]> = lowered.live.iter().map(|p| AtomicU32::new(lowered.params[p.param as usize].default.to_bits())).collect();
    let kernel = Arc::new(Kernel {
        kind: lowered.kind,
        math: lowered.math,
        entry: lowered.entry,
        params: lowered.params,
        buffers: lowered.buffers,
        shared: shared.into_boxed_slice(),
        program: lowered.program,
        flat,
        cost: lowered.cost,
        parallel_safe: lowered.parallel_safe,
        #[cfg(target_arch = "aarch64")]
        native,
        #[cfg(target_arch = "aarch64")]
        neon,
        admission: std::sync::OnceLock::new(),
        speed: AtomicU32::new(0),
        wasm_slots: [AtomicU32::new(0), AtomicU32::new(0)],
        live_values,
        live: lowered.live.into_boxed_slice(),
        folded: lowered.folded.into_boxed_slice(),
    });
    #[cfg(target_arch = "wasm32")]
    match precompiled(&kernel.program) {
        Some((scalar, simd)) => kernel.set_wasm_slots(scalar, simd),
        None => wasm_queue::push(&kernel),
    }
    #[cfg(not(target_arch = "wasm32"))]
    if RECORDING.load(Ordering::Relaxed) {
        let modules = modules.iter().map(|m| (m.path.to_string(), m.source.to_string())).collect();
        recorded().0.send(Recorded { kernel: kernel.clone(), source: src.to_string(), layouts: layouts.to_vec(), modules }).ok();
    }
    Ok(kernel)
}

/// Kernels compiled on wasm32 that wait for their generated code to be
/// linked by the host (whichever crate compiled them): the host's UI
/// thread takes them with [`take_unlinked`], builds one module
/// ([`wasm_module`]), links it and sets each kernel's slots.
/// The thread whose function table holds the linked kernel code: the host
/// thread that links (register_precompiled, or [`mark_wasm_link_thread`]
/// where a host links kernels itself).
#[cfg(target_arch = "wasm32")]
mod wasm_link_thread {
    thread_local! {
        static HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    pub fn mark() {
        HERE.with(|h| h.set(true));
    }
    pub fn is_here() -> bool {
        HERE.with(|h| h.get())
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_queue {
    use super::Kernel;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    fn queue() -> &'static (Sender<Weak<Kernel>>, Mutex<Receiver<Weak<Kernel>>>) {
        static Q: OnceLock<(Sender<Weak<Kernel>>, Mutex<Receiver<Weak<Kernel>>>)> = OnceLock::new();
        Q.get_or_init(|| {
            let (tx, rx) = channel();
            (tx, Mutex::new(rx))
        })
    }

    pub fn push(k: &Arc<Kernel>) {
        let _ = queue().0.send(Arc::downgrade(k));
    }

    pub fn take() -> Vec<Arc<Kernel>> {
        // Only the host's UI thread receives: the lock is never contended.
        let Ok(rx) = queue().1.try_lock() else { return Vec::new() };
        rx.try_iter().filter_map(|w| w.upgrade()).collect()
    }
}

/// Kernels compiled since the last call whose wasm code is not linked yet
/// (wasm32 hosts; empty elsewhere). Kernels already dropped are skipped.
pub fn take_unlinked() -> Vec<Arc<Kernel>> {
    #[cfg(target_arch = "wasm32")]
    return wasm_queue::take();
    #[cfg(not(target_arch = "wasm32"))]
    Vec::new()
}

static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A kernel compiled while recording, with what it was compiled from (so
/// a tool can compile it again elsewhere: its source, host layouts and the
/// modules its `use` items named).
pub struct Recorded {
    pub kernel: Arc<Kernel>,
    pub source: String,
    pub layouts: Vec<Layout>,
    /// (path, source) of each module.
    pub modules: Vec<(String, String)>,
}

fn recorded() -> &'static (std::sync::mpsc::Sender<Recorded>, std::sync::Mutex<std::sync::mpsc::Receiver<Recorded>>) {
    static Q: std::sync::OnceLock<(std::sync::mpsc::Sender<Recorded>, std::sync::Mutex<std::sync::mpsc::Receiver<Recorded>>)> = std::sync::OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        (tx, std::sync::Mutex::new(rx))
    })
}

/// Keeps every kernel compiled from now on (any thread, any compile site)
/// for [`take_recorded`], or stops keeping them: how a build tool learns
/// which kernels a document compiles while it evaluates it (desktop hosts;
/// a wasm host links its kernels instead).
pub fn record_compiled(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
}

/// The kernels compiled while recording, since the last call (a build
/// tool's thread; sending never waits).
pub fn take_recorded() -> Vec<Recorded> {
    let Ok(rx) = recorded().1.lock() else { return Vec::new() };
    rx.try_iter().collect()
}

/// A kernel program's identity across hosts: the same program has the
/// same key on every target, so a build tool on the desktop can compile a
/// film's kernels to wasm ahead of time and the wasm host find their code
/// by the program it lowered ([`register_precompiled`]). The generated
/// code depends on the program alone (a film build fixes
/// `wasm::Target::relaxed_fma` off: the probe answers per machine).
/// FNV-1a over the program's debug text, for now.
pub fn program_key(p: &Program) -> u64 {
    use std::fmt::Write;
    struct Fnv(u64);
    impl Write for Fnv {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for b in s.bytes() {
                self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
            Ok(())
        }
    }
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    let _ = write!(h, "{:?}", p);
    h.0
}

/// One kernel compiled ahead of time and linked into the host's function
/// table: its [`program_key`] and the table slot of its entry, as `simd`
/// when it is four wide, else as `scalar` (the other 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Precompiled {
    pub key: u64,
    pub scalar: u32,
    pub simd: u32,
}

static PRECOMPILED: std::sync::OnceLock<std::collections::HashMap<u64, (u32, u32)>> = std::sync::OnceLock::new();
static PRECOMPILED_HITS: AtomicU32 = AtomicU32::new(0);
static PRECOMPILED_MISSES: AtomicU32 = AtomicU32::new(0);

/// Installs the kernels a host linked ahead of time, once, before any
/// kernel compiles (a later call is ignored: false). A kernel whose program
/// is among them runs that code from its compile on; any other is linked
/// at run time as before ([`take_unlinked`]).
/// This thread links kernel modules into its function table (wasm32): only
/// here do kernels run their linked code; other threads interpret.
#[cfg(target_arch = "wasm32")]
pub fn mark_wasm_link_thread() {
    wasm_link_thread::mark();
}

pub fn register_precompiled(entries: &[Precompiled]) -> bool {
    #[cfg(target_arch = "wasm32")]
    wasm_link_thread::mark();
    PRECOMPILED.set(entries.iter().map(|e| (e.key, (e.scalar, e.simd))).collect()).is_ok()
}

/// The linked slots of a precompiled program, counting hits and misses
/// ([`precompiled_stats`]); None when nothing was registered or the program
/// is not among them.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn precompiled(p: &Program) -> Option<(u32, u32)> {
    let table = PRECOMPILED.get()?;
    let found = table.get(&program_key(p)).copied();
    let (count, what) = if found.is_some() { (&PRECOMPILED_HITS, "found") } else { (&PRECOMPILED_MISSES, "not found, linking at run time") };
    if count.fetch_add(1, Ordering::Relaxed) == 0 {
        eprintln!("kernel: precompiled code {what} (first such kernel)");
    }
    found
}

/// Compiles that found their precompiled code, and compiles that did not
/// (and were linked at run time), since start.
pub fn precompiled_stats() -> (u32, u32) {
    (PRECOMPILED_HITS.load(Ordering::Relaxed), PRECOMPILED_MISSES.load(Ordering::Relaxed))
}

/// Why a call could not run.
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
    /// Cancelled (by the watchdog or the owner) before it finished.
    Cancelled,
    /// An output buffer is smaller than the call needs (words).
    TooSmall { name: String, need: u64, have: usize },
    /// More elements than one call may run.
    TooMany(usize),
    /// Worst-case work (ops per element x elements) exceeds the call's limit.
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
            KernelError::OverBudget { work, limit } => write!(f, "worst-case work {} ops exceeds the limit of {}", work, limit),
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
        if self.wasm_linked() {
            return Backend::Native;
        }
        Backend::Interp
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Has NEON ×4 code (used for element-local calls whose outputs hold
    /// every element).
    pub fn simd(&self) -> bool {
        #[cfg(target_arch = "aarch64")]
        return self.neon.is_some();
        #[cfg(target_arch = "wasm32")]
        return self.wasm_slots[1].load(Ordering::Relaxed) != 0;
        #[cfg(not(any(target_arch = "aarch64", target_arch = "wasm32")))]
        false
    }

    /// The scalar and NEON ×4 machine code as instruction words (for
    /// disassembly; empty where the kernel has none).
    pub fn code_words(&self) -> (&[u32], &[u32]) {
        #[cfg(target_arch = "aarch64")]
        return (self.native.as_ref().map_or(&[][..], |c| c.words()), self.neon.as_ref().map_or(&[][..], |c| c.words()));
        #[cfg(not(target_arch = "aarch64"))]
        (&[], &[])
    }

    /// Estimated ns per element on one thread: the moving average of
    /// native runs, or before any, the worst-case op count at about 0.03
    /// ns per op for NEON code (0.1 for scalar, 20 for the interpreter).
    pub fn ns_per_element(&self, mode: Mode) -> f64 {
        let per_op = match mode {
            Mode::Interp => return self.cost as f64 * 20.0,
            Mode::Vector if self.simd() => 0.03,
            _ => 0.1,
        };
        let s = f32::from_bits(self.speed.load(Ordering::Relaxed));
        if s > 0.0 {
            s as f64
        } else {
            self.cost as f64 * per_op
        }
    }

    /// Records a native run of `count` elements on `threads` threads that
    /// took `nanos` (one-thread-equivalent time per element, averaged).
    pub(crate) fn observe(&self, count: usize, threads: usize, nanos: u64, mode: Mode) {
        if mode == Mode::Interp || count < 256 {
            return;
        }
        let per = (nanos as f64 * threads as f64 / count as f64) as f32;
        let old = f32::from_bits(self.speed.load(Ordering::Relaxed));
        let new = if old > 0.0 { old * 0.75 + per * 0.25 } else { per };
        self.speed.store(new.to_bits(), Ordering::Relaxed);
    }

    /// Words of the read-only shared tables.
    pub fn shared_words(&self) -> usize {
        self.shared.len()
    }

    /// The read-only shared tables (what a backend outside this crate,
    /// such as generated wasm, reads as its shared region).
    pub fn shared_table(&self) -> &[u32] {
        &self.shared
    }

    /// Ctx words: base, count, time, seed, cancel, overflow, reduce lanes,
    /// then the params.
    pub fn ctx_words(&self) -> usize {
        K_PARAMS as usize + self.params.len()
    }

    /// Set the live literal at source offset `offset` ([`compile_live`])
    /// for the calls from now on: a number takes `values[0]`, a colour its
    /// four channels. How many parameters changed (0: not a live literal of
    /// this kernel).
    pub fn set_live(&self, offset: usize, values: &[f32]) -> usize {
        let mut changed = 0;
        for (p, v) in self.live.iter().zip(self.live_values.iter()) {
            if p.offset != offset {
                continue;
            }
            let Some(x) = values.get(p.channel.unwrap_or(0) as usize) else { continue };
            v.store(x.to_bits(), Ordering::Relaxed);
            changed += 1;
        }
        changed
    }

    /// Whether the live literal at `offset` stayed a constant in this
    /// kernel (changing it needs a new compile).
    pub fn folds(&self, offset: usize) -> bool {
        self.folded.contains(&offset)
    }

    /// The source offsets of the literals this kernel reads live.
    pub fn live_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.live.iter().map(|p| p.offset)
    }

    /// A call with every param at its default.
    pub fn call(&self) -> Call<'_> {
        let mut ctx = vec![0u32; self.ctx_words()];
        for (k, p) in self.params.iter().enumerate() {
            ctx[K_PARAMS as usize + k] = p.default.to_bits();
        }
        for (p, v) in self.live.iter().zip(self.live_values.iter()) {
            ctx[K_PARAMS as usize + p.param as usize] = v.load(Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicU32::new(0));
        let mut bufs = vec![None; self.buffers.len()];
        // Buffer 0 is the control word: the cancel token itself, which the
        // kernel reads every element.
        if !bufs.is_empty() {
            bufs[0] = Some((cancel.as_ptr(), 1));
        }
        Call { kernel: self, ctx, bufs, cancel, work_limit: DEFAULT_WORK_LIMIT, simd: true, _borrow: std::marker::PhantomData }
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
    /// caller's copy) and the buffer table. The kernel itself polls the
    /// cancel token (the control buffer) every element.
    pub(crate) fn run_range(&self, ctx: &mut [u32], table: &[u64], lens: &[usize], start: usize, n: usize, mode: Mode, cancel: &AtomicU32) -> Result<(), KernelError> {
        // The IEEE environment is part of the result: round to nearest, no
        // flush-to-zero, no default-NaN, whatever the host thread had.
        let _fp = FpEnv::pin();
        let interp = mode == Mode::Interp;
        let step = if interp { 256 } else { CHUNK };
        let mut at = 0;
        while at < n {
            if cancel.load(Ordering::Relaxed) != 0 {
                return Err(KernelError::Cancelled);
            }
            let k = (n - at).min(step);
            ctx[K_BASE as usize] = (start + at) as u32;
            self.run_raw(ctx, table, lens, k, mode);
            at += k;
        }
        if cancel.load(Ordering::Relaxed) != 0 {
            return Err(KernelError::Cancelled);
        }
        Ok(())
    }

    /// Records where the host linked this kernel's generated wasm: the
    /// function table slot of its scalar or of its four-wide entry (0: that
    /// form is absent; [`wasm_module`] makes one of them). From then on its
    /// runs call that code.
    pub fn set_wasm_slots(&self, scalar: u32, simd: u32) {
        self.wasm_slots[1].store(simd, Ordering::Relaxed);
        self.wasm_slots[0].store(scalar, Ordering::Relaxed);
    }

    /// The generated wasm is linked (runs do not use the interpreter).
    pub fn wasm_linked(&self) -> bool {
        self.wasm_slots.iter().any(|s| s.load(Ordering::Relaxed) != 0)
    }

    /// Runs `n` elements on linked wasm code (true), or false when none is
    /// linked. The table becomes the wasm form: (byte address, length) i32
    /// pairs. A four-wide entry runs all n elements (its last group of
    /// four masked); a scalar-mode run on it (outputs that may not hold
    /// every record) calls it once per element, in order.
    #[cfg(target_arch = "wasm32")]
    fn run_wasm(&self, ctx: &mut [u32], table: &[u64], n: usize, mode: Mode) -> bool {
        // A table slot names a function in the linking thread's instance
        // only (wasm tables are per thread); elsewhere (a worker in a
        // threaded build) the kernel interprets.
        if !wasm_link_thread::is_here() {
            return false;
        }
        let scalar = self.wasm_slots[0].load(Ordering::Relaxed);
        let simd = self.wasm_slots[1].load(Ordering::Relaxed);
        if scalar == 0 && simd == 0 {
            return false;
        }
        thread_local! {
            static FRAME: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        let mut t32 = [0u32; 2 * kl::MAX_BUFFERS];
        for (k, w) in table.iter().take(2 * kl::MAX_BUFFERS).enumerate() {
            t32[k] = *w as u32;
        }
        type Entry = extern "C" fn(u32, u32, u32, u32, u32, u32);
        FRAME.with(|frame| {
            let mut frame = frame.borrow_mut();
            let need = crate::wasm::frame_words(&self.program, true).max(crate::wasm::frame_words(&self.program, false)).max(1);
            if frame.len() < need + 3 {
                frame.resize(need + 3, 0);
            }
            // 16-byte aligned (lane word w of lane l at 16 w + 4 l).
            let skip = (4 - (frame.as_ptr() as usize / 4) % 4) % 4;
            let frame = &mut frame[skip..];
            let mut state = [0u32; 1];
            let sp = state.as_mut_ptr() as u32;
            let args = |n: usize, frame: &mut [u32], ctx: &mut [u32]| (ctx.as_mut_ptr() as u32, sp, self.shared.as_ptr() as u32, t32.as_ptr() as u32, n as u32, frame.as_mut_ptr() as u32);
            let call = |slot: u32, a: (u32, u32, u32, u32, u32, u32)| {
                // SAFETY: `slot` is a function table index the host linked
                // for this kernel's entry of type (i32 x 6) -> () (the
                // engine checks the type at the call); the code clamps
                // every access into the regions and buffers passed.
                let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(slot as usize) };
                f(a.0, a.1, a.2, a.3, a.4, a.5);
            };
            if simd != 0 && mode == Mode::Vector {
                call(simd, args(n, frame, ctx));
            } else if scalar != 0 {
                call(scalar, args(n, frame, ctx));
            } else {
                let base = ctx[K_BASE as usize];
                for e in 0..n {
                    ctx[K_BASE as usize] = base.wrapping_add(e as u32);
                    call(simd, args(1, frame, ctx));
                }
                ctx[K_BASE as usize] = base;
            }
        });
        true
    }

    fn run_raw(&self, ctx: &mut [u32], table: &[u64], lens: &[usize], n: usize, mode: Mode) {
        #[cfg(target_arch = "wasm32")]
        if mode != Mode::Interp && self.run_wasm(ctx, table, n, mode) {
            return;
        }
        #[cfg(target_arch = "aarch64")]
        if mode == Mode::Vector && n >= 4 {
            if let (Some(code), Some(_)) = (&self.neon, &self.native) {
                let mut state = [0u32; 1];
                let n4 = n & !3;
                // SAFETY: as below; the caller checked that the kernel is
                // element-local and its written buffers hold every record.
                unsafe { code.run_kernel(ctx.as_mut_ptr(), state.as_mut_ptr(), self.shared.as_ptr() as *mut u32, table.as_ptr(), n4 as u32) };
                if n4 < n {
                    let base = ctx[K_BASE as usize];
                    ctx[K_BASE as usize] = base.wrapping_add(n4 as u32);
                    self.run_raw(ctx, table, lens, n - n4, Mode::Scalar);
                    ctx[K_BASE as usize] = base;
                }
                return;
            }
        }
        let interp = mode == Mode::Interp;
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
        // Scratch is per thread and reused (no allocation per batch once it
        // has grown to the largest program run on this thread).
        thread_local! {
            static SCRATCH: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            let need = self.flat.scratch_words();
            if scratch.len() < need {
                scratch.resize(need, 0);
            }
            let mut state = [0u32; 1];
            // No references over host memory: word access through atomics.
            let mut bufs = [ir::RawBuf { ptr: std::ptr::null_mut(), len: 0, writable: false }; kl::MAX_BUFFERS];
            for (k, len) in lens.iter().enumerate().take(kl::MAX_BUFFERS) {
                bufs[k] = ir::RawBuf { ptr: table[2 * k] as *mut u32, len: *len, writable: self.buffers[k].access != Access::Read };
            }
            let mut mem = ir::Mem { ctx, state: &mut state, shared: ir::Shared::Read(&self.shared), bufs: &bufs[..lens.len().min(kl::MAX_BUFFERS)] };
            let zeros = [0f32; 1];
            let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
            let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
            ir::run(&self.flat, &mut scratch[..need], &mut mem, &mut io, n as u32);
        });
    }

    /// The reduce kind, lanes and identity.
    pub(crate) fn reduce_parts(&self) -> (ReduceOp, usize, f32) {
        self.reduce_init()
    }
}

/// The FP environment pinned for a run (restored on drop): IEEE round to
/// nearest, no flush-to-zero, no default NaN (FPCR = 0 on AArch64).
struct FpEnv {
    #[cfg(target_arch = "aarch64")]
    saved: u64,
}

impl FpEnv {
    #[inline(always)]
    fn pin() -> FpEnv {
        #[cfg(target_arch = "aarch64")]
        {
            let saved: u64;
            // SAFETY: reading and writing FPCR only changes this thread's
            // FP control bits; the guard restores them.
            unsafe {
                std::arch::asm!("mrs {0}, fpcr", out(reg) saved, options(nomem, nostack, preserves_flags));
                if saved != 0 {
                    std::arch::asm!("msr fpcr, xzr", options(nomem, nostack, preserves_flags));
                }
            }
            FpEnv { saved }
        }
        #[cfg(not(target_arch = "aarch64"))]
        FpEnv {}
    }
}

impl Drop for FpEnv {
    fn drop(&mut self) {
        #[cfg(target_arch = "aarch64")]
        if self.saved != 0 {
            // SAFETY: restores the value read in `pin`.
            unsafe { std::arch::asm!("msr fpcr, {0}", in(reg) self.saved, options(nomem, nostack, preserves_flags)) };
        }
    }
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

/// Checks a call before it runs: the element count, the worst-case work
/// against `work_limit`, and that every buffer written per element holds
/// `count` records (always for emit buffers; for plain outputs when the
/// run is split across threads or four elements run at once, so no two
/// of them ever share a word). `lens[k]` is buffer k's bound length.
pub(crate) fn check(kernel: &Kernel, lens: &[usize], count: usize, split: bool, work_limit: u64) -> Result<(), KernelError> {
    if count > kl::ELEMENT_CAP as usize {
        return Err(KernelError::TooMany(count));
    }
    let work = kernel.cost.saturating_mul(count as u64);
    if work > work_limit {
        return Err(KernelError::OverBudget { work, limit: work_limit });
    }
    for (k, b) in kernel.buffers.iter().enumerate().skip(1) {
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
            return Err(KernelError::TooSmall { name: b.name.clone(), need, have });
        }
    }
    Ok(())
}

/// Runs `count` elements in fixed chunks of [`CHUNK`]: element-local
/// kernels on up to `threads` workers of `exec` (the caller included),
/// others in order on the caller. `ctx` is the call's ctx (count, time,
/// seed, params); every chunk starts from a copy of it, and the chunk
/// results are combined in chunk order, so the outcome is the same for any
/// thread count. `cells` holds at least one cell per chunk. Allocates
/// nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_chunks(
    kernel: &Kernel,
    ctx: &[u32],
    table: &[u64],
    lens: &[usize],
    count: usize,
    mode: Mode,
    cancel: &AtomicU32,
    cells: &[ChunkCell],
    exec: &dyn crate::sched::Executor,
    threads: usize,
) -> Result<(bool, bool, [f32; 16]), KernelError> {
    let (op, lanes, init) = kernel.reduce_init();
    // Map kernels split into equal ranges, several per thread (any split
    // gives the same bits); reduce kernels into the fixed chunks.
    let unit = if lanes == 0 && threads > 1 { count.div_ceil(threads * 4).next_multiple_of(4).clamp(MIN_SPLIT, CHUNK) } else { CHUNK };
    let chunks = count.div_ceil(unit);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failed = std::sync::atomic::AtomicBool::new(false);
    let any_overflow = std::sync::atomic::AtomicBool::new(false);
    let any_host_error = std::sync::atomic::AtomicBool::new(false);
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
            if kernel.run_range(wctx, table, lens, start, k, mode, cancel).is_err() {
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
    let helpers = if kernel.parallel_safe { threads.max(1).min(chunks.max(1)) } else { 1 };
    if helpers <= 1 {
        work(0);
    } else {
        exec.fan_out(helpers, &work);
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
    let _ = op;
    Ok((overflowed, host_error, reduced))
}

/// How a range of elements runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Interp,
    Scalar,
    /// NEON ×4 where the kernel has it, else scalar.
    Vector,
}

/// One invocation: params and bound buffers, borrowed for `'a`.
pub struct Call<'a> {
    kernel: &'a Kernel,
    ctx: Vec<u32>,
    /// (pointer, length in words) per kernel buffer.
    bufs: Vec<Option<(*mut u32, usize)>>,
    cancel: Arc<AtomicU32>,
    work_limit: u64,
    /// Use NEON ×4 code when the call allows it (on by default).
    simd: bool,
    _borrow: std::marker::PhantomData<&'a mut [u32]>,
}

/// Default admission limit: worst-case ops per element x elements.
pub const DEFAULT_WORK_LIMIT: u64 = 1 << 40;

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

    /// Admission: refuse calls whose worst-case work (ops per element x
    /// elements) exceeds `ops` (untrusted kernels get a host-chosen limit).
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
        if self.simd && self.kernel.simd() && self.kernel.parallel_safe && self.admit(count, true).is_ok() {
            Mode::Vector
        } else {
            Mode::Scalar
        }
    }

    pub fn cancel_token(&self) -> CancelToken {
        CancelToken(self.cancel.clone())
    }

    fn bind(&mut self, name: &str, ptr: *mut u32, len: usize, writable: bool) -> Result<(), KernelError> {
        let k = self.kernel.buffer_index(name).filter(|k| *k > 0).ok_or_else(|| KernelError::NoSuchBuffer(name.into()))?;
        if len == 0 {
            return Err(KernelError::Empty(name.into()));
        }
        // Word offsets are 32-bit arithmetic (and every backend reads the
        // length as a 32-bit word).
        if len > i32::MAX as usize {
            return Err(KernelError::TooLarge(name.into()));
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

    fn lens(&self) -> Vec<usize> {
        self.bufs.iter().map(|b| b.map_or(0, |(_, len)| len)).collect()
    }

    /// Admission and capacity (see [`check`]).
    fn admit(&self, count: usize, split: bool) -> Result<(), KernelError> {
        check(self.kernel, &self.lens(), count, split, self.work_limit)
    }

    fn run_with(&mut self, count: usize, interp: bool) -> Result<RunStats, KernelError> {
        self.run_on(count, interp, &crate::sched::InlineExecutor, 1)
    }

    fn run_on(&mut self, count: usize, interp: bool, exec: &dyn crate::sched::Executor, threads: usize) -> Result<RunStats, KernelError> {
        let t0 = Clock::now();
        let (table, lens) = self.table()?;
        let split = splits(self.kernel, threads, count);
        self.admit(count, split)?;
        let mode = if interp { Mode::Interp } else { self.native_mode(count) };
        self.ctx[K_COUNT as usize] = count as u32;
        let cells: Vec<ChunkCell> = (0..count.div_ceil(CHUNK)).map(|_| ChunkCell::default()).collect();
        let t = split_threads(self.kernel, threads, count, mode);
        let t1 = Clock::now();
        let (overflowed, host_error, reduced) = run_chunks(self.kernel, &self.ctx, &table, &lens, count, mode, &self.cancel, &cells, exec, t)?;
        self.kernel.observe(count, t, t1.nanos(), mode);
        let lanes = self.kernel.reduce_init().1;
        Ok(RunStats { elements: count, overflowed, host_error, reduced: reduced[..lanes].to_vec(), nanos: t0.nanos() })
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

/// One wasm module holding the code of several kernels (a document's), for
/// a host that links generated wasm (a browser: the platform's
/// `wasm_link`): one entry per kernel, export `k` (the order in which the
/// link returns their table slots) kernel k's, and per kernel whether that
/// entry is four wide (element-local kernels the backend vectorizes) or
/// scalar. After linking, the host calls [`Kernel::set_wasm_slots`] on
/// each (`(0, slot)` for a four-wide entry, `(slot, 0)` for a scalar one).
pub fn wasm_module(kernels: &[&Kernel], target: crate::wasm::Target) -> Option<(Vec<u8>, Vec<bool>)> {
    let simd: Vec<bool> = kernels.iter().map(|k| k.parallel_safe && crate::wasm::simd_supported(&k.program)).collect();
    let entries: Vec<crate::wasm::Entry> = kernels.iter().zip(&simd).map(|(k, s)| crate::wasm::Entry { program: &k.program, simd: *s }).collect();
    crate::wasm::module(&entries, target).map(|m| (m, simd))
}
