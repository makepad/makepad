//! Compute kernels: per-element Splash programs over host buffers, compiled
//! ahead of time (see `lower::kernel` for the language side).
//!
//! ```ignore
//! let k = kernel::compile(src)?;
//! use makepad_script_compute_sched::kernel::KernelExt;
//! let mut call = k.call();
//! call.set_param("amp", 2.0);
//! call.input("hf", &heights)?;
//! call.output("pos", &mut positions)?;
//! let stats = call.run(count)?;          // or run_parallel(count, threads)
//! ```
//!
//! The host scheduler runs fixed chunks; a reduce kernel combines its
//! chunk partials in chunk order, so every result is bit-identical whatever
//! the backend, thread count or scheduling.
use makepad_relax_api::relax_api;

use crate::ir::{self, Program};
pub use crate::boundary::KernelRef;
use crate::boundary::{forever, BufferInfoView, ParamInfoView};
use crate::lower::kernel as kl;
pub use crate::lower::kernel::{
    Access, BufferDecl, FieldTy, KernelKind, Layout, LayoutField, MathMode, ReduceOp, CANONICAL_NAN, ELEMENT_CAP, K_ACC, K_BASE, K_COUNT, K_OVERFLOW, K_PARAMS, K_SEED,
    K_TIME,
};
pub use crate::lower::ParamInfo;
use crate::{Backend, ShaderError};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

const RUN_SLICE_MAX: usize = 4096;

/// Hashing, noise, quaternions, matrices, curves and buffer sampling.
/// Available to every kernel; only reached functions are compiled in.
pub const KERNEL_PRELUDE: &str = include_str!("kernel_prelude.splash");

/// A compiled kernel's plain face: what callers read, laid out so that
/// Relax-compiled code reads it itself instead of calling into the
/// compiler's code. It is the first field of its [`KernelBody`] (which holds
/// the programs, the machine code and the caches) and derefs to it; a
/// kernel is reached through a [`KernelRef`], which keeps it alive.
#[relax_api(plain)]
#[repr(C)]
pub struct Kernel {
    pub kind: KernelKind,
    pub math: MathMode,
    /// Element ranges may run on different threads.
    pub parallel_safe: bool,
    /// Worst-case AIR ops per element.
    pub cost: u64,
    ctx_words: usize,
    /// The entry's name: vertex, instance, element, primitive, reduce_*.
    entry: &'static str,
    params: &'static [ParamInfoView<'static>],
    buffers: &'static [BufferInfoView<'static>],
    /// Edit mode: per live literal, the parameter it reads and, in a cell
    /// the compiler's code and this one read and write as an `AtomicU32`,
    /// its value now.
    live_param: &'static [u32],
    live_value: &'static [u32],
    /// Measured ns per element (f32 bits, a moving average; 0: none yet):
    /// how many threads a call is worth. An `AtomicU32` cell.
    speed: *mut u32,
    /// Has four-wide code (element-local kernels only). An `AtomicU8` cell.
    simd: *mut u8,
    /// The body this face is the first field of.
    body: *const KernelBody,
}

// SAFETY: the face is immutable apart from its cells, which are only ever
// used as atomics, and `body` points to the allocation it lives in, which is
// shared across threads the same way.
unsafe impl Send for Kernel {}
unsafe impl Sync for Kernel {}

/// The kernel's cells that change while it is shared: the plain face points
/// at them (the std's atomics are no plain data).
struct KernelCells {
    speed: AtomicU32,
    simd: AtomicU8,
}

impl Kernel {
    #[inline]
    pub fn kind(&self) -> KernelKind { self.kind }
    #[inline]
    pub fn math(&self) -> MathMode { self.math }
    #[inline]
    pub fn entry(&self) -> &str { self.entry }
    #[inline]
    pub fn cost(&self) -> u64 { self.cost }
    #[inline]
    pub fn parallel_safe(&self) -> bool { self.parallel_safe }

    /// Ctx words: base, count, time, seed, host work, overflow, reduce lanes,
    /// then the params.
    #[inline]
    pub fn ctx_words(&self) -> usize { self.ctx_words }

    #[inline]
    pub fn params(&self) -> &[ParamInfoView<'_>] { self.params }
    #[inline]
    pub fn param_count(&self) -> usize { self.params.len() }
    #[inline]
    pub fn param_view(&self, index: usize) -> ParamInfoView<'_> { self.params[index] }
    #[inline]
    pub fn param_views(&self) -> impl Iterator<Item = ParamInfoView<'_>> {
        self.params.iter().copied()
    }
    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }

    #[inline]
    pub fn buffers(&self) -> &[BufferInfoView<'_>] { self.buffers }
    #[inline]
    pub fn buffer_count(&self) -> usize { self.buffers.len() }
    #[inline]
    pub fn buffer_view(&self, index: usize) -> BufferInfoView<'_> { self.buffers[index] }
    #[inline]
    pub fn buffer_views(&self) -> impl Iterator<Item = BufferInfoView<'_>> {
        self.buffers.iter().copied()
    }
    pub fn buffer_index(&self, name: &str) -> Option<usize> {
        self.buffers.iter().position(|b| b.name == name)
    }

    /// Has NEON x4 code (used for element-local calls whose outputs hold
    /// every element).
    #[inline]
    pub fn simd(&self) -> bool {
        // SAFETY: the cell is an AtomicU8 of the body, which outlives this face.
        unsafe { &*(self.simd as *const AtomicU8) }.load(Ordering::Relaxed) != 0
    }

    /// The moving average of measured ns per element.
    #[inline]
    fn speed(&self) -> &AtomicU32 {
        // SAFETY: the cell is an AtomicU32 of the body, which outlives this face.
        unsafe { &*(self.speed as *const AtomicU32) }
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
        let s = f32::from_bits(self.speed().load(Ordering::Relaxed));
        if s > 0.0 {
            s as f64
        } else {
            self.cost as f64 * per_op
        }
    }

    /// Records a native run of `count` elements on `threads` threads that
    /// took `nanos` (one-thread-equivalent time per element, averaged).
    pub fn observe(&self, count: usize, threads: usize, nanos: u64, mode: Mode) {
        if mode == Mode::Interp || count < 256 {
            return;
        }
        let per = (nanos as f64 * threads as f64 / count as f64) as f32;
        let old = f32::from_bits(self.speed().load(Ordering::Relaxed));
        let new = if old > 0.0 { old * 0.75 + per * 0.25 } else { per };
        self.speed().store(new.to_bits(), Ordering::Relaxed);
    }

    /// The program's identity across hosts ([`program_key`]).
    pub fn program_key(&self) -> u64 {
        let mut key = 0;
        kernel_ctl(self, KernelCtl::ProgramKey(&mut key));
        key
    }

    /// The reduce kind, lanes and identity.
    pub fn reduce_parts(&self) -> (ReduceOp, usize, f32) {
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

    /// Initialize parameters in zeroed caller-owned context storage, including current live literals.
    pub fn init_context(&self, ctx: &mut [u32]) {
        assert!(ctx.len() >= self.ctx_words);
        for (k, p) in self.params.iter().enumerate() { ctx[K_PARAMS as usize + k] = p.default.to_bits(); }
        for (param, value) in self.live_param.iter().zip(self.live_value.iter()) {
            // SAFETY: the cell is an AtomicU32 of the body, which outlives this face.
            ctx[K_PARAMS as usize + *param as usize] = unsafe { &*(value as *const u32 as *const AtomicU32) }.load(Ordering::Relaxed);
        }
    }
}

impl std::ops::Deref for Kernel {
    type Target = KernelBody;
    #[inline]
    fn deref(&self) -> &KernelBody {
        // SAFETY: a Kernel only exists as the first field of a KernelBody,
        // which `body` points to.
        unsafe { &*self.body }
    }
}

impl std::fmt::Debug for Kernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Kernel({} {:?}, {} params, {} buffers)", self.entry, self.kind, self.params.len(), self.buffers.len())
    }
}

/// A compiled kernel's programs, machine code and caches: the compiler's
/// own state, reached through [`Kernel`]'s deref. Immutable and shareable
/// across threads.
#[relax_api(handle)]
#[repr(C)]
pub struct KernelBody {
    /// First: see [`Kernel`].
    pub(crate) head: Kernel,
    entry: String,
    params: Vec<ParamInfo>,
    param_views: Box<[ParamInfoView<'static>]>,
    buffers: Vec<BufferDecl>,
    buffer_views: Box<[BufferInfoView<'static>]>,
    shared: Box<[u32]>,
    program: Program,
    /// `program` with its calls inlined and optimized again (the same
    /// bits): what the interpreter and native code run.
    flat: Program,
    /// What each element counts by itself; its loops count at run time
    /// (ctx `K_WORK`).
    element_work: u64,
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    native: Option<crate::native::Code>,
    /// Four elements per iteration (element-local kernels only).
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    x4: Option<crate::native::Code>,
    /// Admission's worst-case element estimate, computed on first admit
    /// (the job path stays allocation-free).
    pub(crate) admission: std::sync::OnceLock<crate::cost::ElementCost>,
    /// One element's worst case in counted ops (slices are sized by it).
    slice_ops: std::sync::OnceLock<u64>,
    /// Function table slots of this kernel's generated wasm (scalar, four
    /// wide; 0: none; a host links one of the two), once the host linked
    /// its document's module (see [`wasm_module`]).
    wasm_slots: [AtomicU32; 2],
    /// Edit mode: literals read from hidden parameters ([`compile_live`]),
    /// their values now, and the live literals that stayed constants.
    live: Box<[crate::lower::LiveParam]>,
    live_param: Box<[u32]>,
    live_values: Box<[AtomicU32]>,
    folded: Box<[usize]>,
    cells: Box<KernelCells>,
}

#[inline(always)]
pub fn compile(src: &str) -> Result<KernelRef, Vec<ShaderError>> {
    compile_with(src, &[], Backend::Native)
}

/// Compiles with host layouts (GPU vertex/instance structs a kernel may
/// write by field) for a backend.
#[inline(always)]
pub fn compile_with(src: &str, layouts: &[crate::boundary::LayoutView<'_>], backend: Backend) -> Result<KernelRef, Vec<ShaderError>> {
    compile_with_modules(src, layouts, backend, &[])
}

/// Compiles with host layouts and the modules the kernel's `use` items
/// name (the host resolved each to its text; std modules are built in).
#[inline(always)]
pub fn compile_with_modules(src: &str, layouts: &[crate::boundary::LayoutView<'_>], backend: Backend, modules: &[crate::module::Module]) -> Result<KernelRef, Vec<ShaderError>> {
    compile_live(src, layouts, backend, modules, None)
}

/// [`compile_with_modules`] in an editor's edit mode: the number and colour
/// literals of `src` that `live` accepts (by their byte offset) are read
/// from hidden parameters where the code computes with them, so
/// [`Kernel::set_live`] changes them for the next call with the same code;
/// a live literal that must stay a constant (an integer, a size, a value
/// computed once) is in [`Kernel::folds`]. `None` compiles exactly as
/// [`compile_with_modules`].
#[inline(always)]
pub fn compile_live(src: &str, layouts: &[crate::boundary::LayoutView<'_>], backend: Backend, modules: &[crate::module::Module], live: Option<&dyn Fn(usize) -> bool>) -> Result<KernelRef, Vec<ShaderError>> {
    use crate::boundary::LiveFilter;
    unsafe extern "C-unwind" fn include(context: *const (), offset: usize) -> bool {
        // The reference lives in compile_live's caller-side stack frame.
        let filter = unsafe { &*(context as *const &dyn Fn(usize) -> bool) };
        filter(offset)
    }
    let filter = match live.as_ref() {
        Some(filter) => LiveFilter { context: filter as *const _ as *const (), include: Some(include) },
        None => LiveFilter { context: std::ptr::null(), include: None },
    };
    // SAFETY: views and the callback context are borrowed for this synchronous call.
    let mut compiled = unsafe { crate::boundary::compile_kernel(src, layouts, backend, modules, filter) };
    if let Some(kernel) = compiled.take_kernel() {
        return Ok(kernel);
    }
    Err((0..compiled.error_count()).map(|index| {
        let error = compiled.error(index);
        ShaderError::new(error.start, error.end, error.message.to_string())
    }).collect())
}

pub(crate) fn compile_live_impl(src: &str, layouts: &[Layout], backend: Backend, modules: &[crate::module::Module], live: Option<&dyn Fn(usize) -> bool>) -> Result<Arc<KernelBody>, Vec<ShaderError>> {
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
    compile_items_impl(items, src.len(), src, layouts, backend, modules, lift)
}

/// Compiles a kernel's syntax tree: `items` (the kernel's own items, their
/// spans below `src_len`) with the prelude and the modules they use.
/// `source` is the kernel's text when it was written as text (empty for
/// a kernel built from Splash values), kept by the recorder.
pub(crate) fn compile_items_impl(items: Vec<crate::parse::Item>, src_len: usize, source: &str, layouts: &[Layout], backend: Backend, modules: &[crate::module::Module], lift: Option<crate::lower::LiveLift>) -> Result<Arc<KernelBody>, Vec<ShaderError>> {
    let prelude = crate::parse_prelude(KERNEL_PRELUDE, src_len);
    // Module spans go past the prelude's, so errors inside them are
    // reported at the kernel's call like the prelude's.
    // The kernel's own items are the roots; library items (modules, the
    // prelude) are compiled in only when reached.
    let roots: std::collections::HashSet<String> = items.iter().filter(|i| !matches!(i, crate::parse::Item::Use { .. })).map(|i| i.name().to_string()).collect();
    let items = crate::module::resolve(items, src_len + 1 + KERNEL_PRELUDE.len() + 1, modules, &prelude).map_err(|e| vec![e])?;
    let all = crate::module::prune(crate::with_prelude(&items, prelude, &roots), &roots);
    let lowered = kl::lower_kernel(&all, src_len + 1, layouts, lift).map_err(|e| vec![e])?;
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
    let flat = lowered.flat;
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    let native = if backend == Backend::Native {
        let writable = lowered.buffers.iter().enumerate().fold(0u64, |m, (k, b)| if b.access != Access::Read && k < 64 { m | 1 << k } else { m });
        crate::native::compile_with(&flat, writable)
    } else {
        None
    };
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    let x4 = if native.is_some() && lowered.parallel_safe { crate::native_vec::compile(&flat) } else { None };
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    let _ = backend;
    let live_values: Box<[AtomicU32]> = lowered.live.iter().map(|p| AtomicU32::new(lowered.params[p.param as usize].default.to_bits())).collect();
    let live_param: Box<[u32]> = lowered.live.iter().map(|p| p.param).collect();
    let (params, buffers, entry) = (lowered.params, lowered.buffers, lowered.entry);
    // SAFETY (all four views): the names and boxes they borrow belong to the
    // body built below and are never changed or dropped before it; a
    // Kernel hands them out only for the length of a borrow of itself.
    let param_views: Box<[ParamInfoView<'static>]> = params.iter().map(|p| ParamInfoView { name: unsafe { forever(p.name.as_str()) }, default: p.default, min: p.min, max: p.max }).collect();
    let buffer_views: Box<[BufferInfoView<'static>]> = buffers.iter().map(|b| BufferInfoView { name: unsafe { forever(b.name.as_str()) }, access: b.access, stride: b.stride }).collect();
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    let simd = x4.is_some();
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    let simd = false;
    let cells = Box::new(KernelCells { speed: AtomicU32::new(0), simd: AtomicU8::new(simd as u8) });
    let kernel = Arc::new_cyclic(|body| {
        let head = Kernel {
            kind: lowered.kind,
            math: lowered.math,
            parallel_safe: lowered.parallel_safe,
            cost: lowered.cost,
            ctx_words,
            entry: unsafe { forever(entry.as_str()) },
            params: unsafe { forever(&*param_views) },
            buffers: unsafe { forever(&*buffer_views) },
            live_param: unsafe { forever(&*live_param) },
            live_value: unsafe { std::slice::from_raw_parts(live_values.as_ptr() as *const u32, live_values.len()) },
            speed: cells.speed.as_ptr(),
            simd: cells.simd.as_ptr(),
            body: body.as_ptr(),
        };
        KernelBody {
            head,
            entry,
            params,
            param_views,
            buffers,
            buffer_views,
            shared: shared.into_boxed_slice(),
            program: lowered.program,
            flat,
            element_work: lowered.element_work,
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            native,
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            x4,
            admission: std::sync::OnceLock::new(),
            slice_ops: std::sync::OnceLock::new(),
            wasm_slots: [AtomicU32::new(0), AtomicU32::new(0)],
            live_values,
            live_param,
            live: lowered.live.into_boxed_slice(),
            folded: lowered.folded.into_boxed_slice(),
            cells,
        }
    });
    #[cfg(target_arch = "wasm32")]
    match precompiled(&kernel.program) {
        Some((scalar, simd)) => kernel.set_wasm_slots(scalar, simd),
        None => wasm_queue::push(&kernel),
    }
    #[cfg(not(target_arch = "wasm32"))]
    if RECORDING.load(Ordering::Relaxed) {
        let modules = modules.iter().map(|m| (m.path.to_string(), m.source.to_string())).collect();
        recorded().0.send(Recorded { kernel: KernelRef::from_arc(kernel.clone()), source: source.to_string(), layouts: layouts.to_vec(), modules }).ok();
    }
    #[cfg(target_arch = "wasm32")]
    let _ = source;
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
    use super::KernelBody;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    fn queue() -> &'static (Sender<Weak<KernelBody>>, Mutex<Receiver<Weak<KernelBody>>>) {
        static Q: OnceLock<(Sender<Weak<KernelBody>>, Mutex<Receiver<Weak<KernelBody>>>)> = OnceLock::new();
        Q.get_or_init(|| {
            let (tx, rx) = channel();
            (tx, Mutex::new(rx))
        })
    }

    pub fn push(k: &Arc<KernelBody>) {
        let _ = queue().0.send(Arc::downgrade(k));
    }

    pub fn take() -> Vec<Arc<KernelBody>> {
        // Only the host's UI thread receives: the lock is never contended.
        let Ok(rx) = queue().1.try_lock() else { return Vec::new() };
        rx.try_iter().filter_map(|w| w.upgrade()).collect()
    }
}

/// Kernels compiled since the last call whose wasm code is not linked yet
/// (wasm32 hosts; empty elsewhere). Kernels already dropped are skipped.
pub fn take_unlinked() -> Vec<KernelRef> {
    #[cfg(target_arch = "wasm32")]
    return wasm_queue::take().into_iter().map(KernelRef::from_arc).collect();
    #[cfg(not(target_arch = "wasm32"))]
    Vec::new()
}

static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A kernel compiled while recording, with what it was compiled from (so
/// a tool can compile it again elsewhere: its source, host layouts and the
/// modules its `use` items named).
pub struct Recorded {
    pub kernel: KernelRef,
    /// Its text; empty for a kernel built from Splash values (the kernel's
    /// program is what a build tool compiles).
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
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
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

impl KernelBody {
    /// Elements per slice when `remaining` ops of the call's limit are left
    /// to `threads` workers: as many as can run (at their worst case)
    /// within each worker's share, so the slices running at once cannot
    /// pass the limit; at least four (four-wide code runs whole groups: a
    /// call stops at most four elements a worker past its limit), at most
    /// a chunk, and a slice's loop count fits the u32 ctx word. Far from
    /// the limit, whole chunks run with no more looks at the count than
    /// one a chunk.
    pub(crate) fn slice(&self, remaining: u64, threads: usize) -> usize {
        let fit = remaining / threads.max(1) as u64 / self.slice_ops();
        let word = (u32::MAX as u64 / self.slice_ops()).max(4);
        (fit.min(word).min(RUN_SLICE_MAX as u64) as usize).max(4) & !3
    }

    /// One element's worst case, computed on first ask.
    fn element_cost(&self) -> crate::cost::ElementCost {
        *self.admission.get_or_init(|| crate::cost::calculate_cost(self))
    }

    /// One element's worst case with every host call at its largest input.
    fn element_ops(&self) -> u64 {
        let c = self.element_cost();
        c.fixed_ops.saturating_add(c.open_max_ops)
    }

    /// One element's worst case in counted ops.
    fn slice_ops(&self) -> u64 {
        *self.slice_ops.get_or_init(|| self.element_ops().max(1))
    }

    pub fn backend(&self) -> Backend {
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
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

    /// The scalar and four-wide machine code as instruction words (for
    /// disassembly; empty where the kernel has none).
    pub fn code_words(&self) -> (&[u32], &[u32]) {
        #[cfg(target_arch = "aarch64")]
        return (self.native.as_ref().map_or(&[][..], |c| c.words()), self.x4.as_ref().map_or(&[][..], |c| c.words()));
        #[cfg(not(target_arch = "aarch64"))]
        (&[], &[])
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
    #[inline(always)]
    pub fn live_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.live_offset_count()).map(|index| self.live_offset(index))
    }

    pub fn live_offset_count(&self) -> usize { self.live.len() }
    pub fn live_offset(&self, index: usize) -> usize { self.live[index].offset }

    /// Records where the host linked this kernel's generated wasm: the
    /// function table slot of its scalar or of its four-wide entry (0: that
    /// form is absent; [`wasm_module`] makes one of them). From then on its
    /// runs call that code.
    pub fn set_wasm_slots(&self, scalar: u32, simd: u32) {
        self.wasm_slots[1].store(simd, Ordering::Relaxed);
        self.wasm_slots[0].store(scalar, Ordering::Relaxed);
        #[cfg(target_arch = "wasm32")]
        self.cells.simd.store((simd != 0) as u8, Ordering::Relaxed);
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
                // One element a call: each stores its own loops' count.
                let base = ctx[K_BASE as usize];
                let w = kl::K_WORK as usize;
                let mut sum = 0u32;
                for e in 0..n {
                    ctx[K_BASE as usize] = base.wrapping_add(e as u32);
                    ctx[w] = 0;
                    call(simd, args(1, frame, ctx));
                    sum = sum.wrapping_add(ctx[w]);
                }
                ctx[K_BASE as usize] = base;
                ctx[w] = sum;
            }
        });
        true
    }

    /// Runs `n` elements and returns the ops they counted: each element's
    /// own, and what their loops counted (ctx `K_WORK`).
    fn run_raw(&self, ctx: &mut [u32], table: &[u64], n: usize, mode: Mode) -> u64 {
        ctx[kl::K_WORK as usize] = 0;
        self.run_once(ctx, table, n, mode);
        self.element_work * n as u64 + ctx[kl::K_WORK as usize] as u64
    }

    /// One run of the generated code (or the interpreter).
    fn run_once(&self, ctx: &mut [u32], table: &[u64], n: usize, mode: Mode) {
        #[cfg(target_arch = "wasm32")]
        if mode != Mode::Interp && self.run_wasm(ctx, table, n, mode) {
            return;
        }
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if mode == Mode::Vector && n >= 4 {
            if let (Some(code), Some(_)) = (&self.x4, &self.native) {
                let mut state = [0u32; 1];
                let n4 = n & !3;
                // SAFETY: as below; the caller checked that the kernel is
                // element-local and its written buffers hold every record.
                unsafe { code.run_kernel(ctx.as_mut_ptr(), state.as_mut_ptr(), self.shared.as_ptr() as *mut u32, table.as_ptr(), n4 as u32) };
                if n4 < n {
                    // The tail on scalar code, its loops' count added.
                    let w = kl::K_WORK as usize;
                    let lanes = ctx[w];
                    let base = ctx[K_BASE as usize];
                    ctx[K_BASE as usize] = base.wrapping_add(n4 as u32);
                    ctx[w] = 0;
                    self.run_once(ctx, table, n - n4, Mode::Scalar);
                    ctx[K_BASE as usize] = base;
                    ctx[w] = ctx[w].wrapping_add(lanes);
                }
                return;
            }
        }
        let interp = mode == Mode::Interp;
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
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
            for k in 0..self.buffers.len() {
                bufs[k] = ir::RawBuf { ptr: table[2 * k] as *mut u32, len: table[2 * k + 1] as usize, writable: self.buffers[k].access != Access::Read };
            }
            let mut mem = ir::Mem { ctx, state: &mut state, shared: ir::Shared::Read(&self.shared), bufs: &bufs[..self.buffers.len()] };
            let zeros = [0f32; 1];
            let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
            let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
            ir::run(&self.flat, &mut scratch[..need], &mut mem, &mut io, n as u32);
        });
    }

}

/// The run entry: the one door the scheduler runs kernel code through.
#[relax_api]
impl KernelBody {
    /// Runs elements [start, start + n) on this thread with `ctx` (the
    /// caller's copy) and the buffer table, in slices of [`KernelBody::slice`]
    /// elements (`threads`: the workers sharing the call's limit).
    /// After each slice its counted ops join `count`; once those pass the
    /// limit the call stops (the other workers at their next element: the
    /// kernel polls the cancel token, the control buffer, every element).
    /// # Safety
    /// `cancel` must point to a live, aligned AtomicU32 for the entire call.
    /// All bindings must be live for this call and writable where declared.
    /// Concurrent calls require an element-local kernel, disjoint element
    /// ranges, and writable buffers large enough for every addressed record.
    /// Descriptors derive from the parent's borrow once; workers never create
    /// mutable slices over the complete parent buffers.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn run_range(&self, ctx: &mut [u32], buffers: &[crate::boundary::BorrowedBuffer<'_>], start: usize, n: usize, mode: Mode, cancel: *mut u32, count: &WorkCount, threads: usize) -> bool {
        let cancel = &*cancel.cast::<AtomicU32>();
        assert!(ctx.len() >= self.head.ctx_words);
        assert!(buffers.len() >= self.buffers.len().max(2));
        assert!(start <= ELEMENT_CAP as usize && n <= ELEMENT_CAP as usize - start);
        // BorrowedBuffer is repr(C), exactly two u64 words plus a zero-sized
        // lifetime marker. No payload or table copy and no mutable reborrow of
        // the parent buffers: all workers read the same immutable table.
        let table = std::slice::from_raw_parts(buffers.as_ptr().cast::<u64>(), 2 * buffers.len());
        assert_eq!(table[0], cancel.as_ptr() as u64);
        // The IEEE environment is part of the result: round to nearest, no
        // flush-to-zero, no default-NaN, whatever the host thread had.
        let _fp = FpEnv::pin();
        let mut at = 0;
        while at < n {
            if cancel.load(Ordering::Relaxed) != 0 {
                return false;
            }
            // Its worst case is held while it runs (reserved against what
            // is left of the limit in one step), so the slices running at
            // once stay within it.
            let (k, held) = count.reserve(|left| {
                let k = (n - at).min(self.slice(left, threads));
                (k, self.slice_ops().saturating_mul(k as u64))
            });
            ctx[K_BASE as usize] = (start + at) as u32;
            ctx[kl::K_HOST_WORK as usize] = 0;
            let ops = self.run_raw(ctx, table, k, mode) + ctx[kl::K_HOST_WORK as usize] as u64;

            at += k;
            let over = count.add(ops);
            // The reservation becomes what it counted (never more).
            count.committed.fetch_sub(held - ops.min(held), Ordering::AcqRel);
            if over {
                cancel.store(1, Ordering::Relaxed);
                return false;
            }
        }
        if cancel.load(Ordering::Relaxed) != 0 {
            return false;
        }
        true
    }
}

/// A small query only the compiler can answer, from state it holds.
#[relax_api(plain)]
#[repr(C, u8)]
pub enum KernelCtl<'a> {
    /// One element's worst case, computed on first ask.
    Cost(&'a mut crate::cost::ElementCost),
    /// The program's identity ([`program_key`]).
    ProgramKey(&'a mut u64),
}

/// Answers a [`KernelCtl`].
#[relax_api]
pub fn kernel_ctl(kernel: &Kernel, ctl: KernelCtl<'_>) {
    match ctl {
        KernelCtl::Cost(out) => *out = kernel.element_cost(),
        KernelCtl::ProgramKey(out) => *out = program_key(&kernel.program),
    }
}

/// The FP environment pinned for a run (restored on drop): IEEE round to
/// nearest, no flush-to-zero, no default NaN (FPCR = 0 on AArch64).
struct FpEnv {
    #[cfg(target_arch = "aarch64")]
    saved: u64,
}

/// The thread's FP control register (FPCR) and its write (Relax assembles it).
#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn fpcr_read() -> u64 {
    let v: u64;
    // SAFETY: reads this thread's FP control bits.
    unsafe { std::arch::asm!("mrs {0}, fpcr", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn fpcr_write(v: u64) {
    // SAFETY: writes this thread's FP control bits; callers restore what they read.
    unsafe { std::arch::asm!("msr fpcr, {0}", in(reg) v, options(nomem, nostack, preserves_flags)) };
}

impl FpEnv {
    #[inline(always)]
    fn pin() -> FpEnv {
        #[cfg(target_arch = "aarch64")]
        {
            let saved = fpcr_read();
            if saved != 0 {
                fpcr_write(0);
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
            fpcr_write(self.saved);
        }
    }
}

/// A call's counted work: the ops its slices reported, against its limit.
/// The caller makes it ([`WorkCount::new`]), the run entry updates it, and
/// the caller reads it ([`WorkCount::state`]) once the workers joined.
#[relax_api(handle)]
pub struct WorkCount {
    used: AtomicU64,
    /// What finished slices counted plus the worst case of those running
    /// (reserved before each starts): one word, so a look at what is left
    /// sees both at once.
    committed: AtomicU64,
    limit: u64,
    over: AtomicBool,
}

/// What a [`WorkCount`] holds.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WorkState {
    pub used: u64,
    pub over: bool,
}

#[relax_api]
impl WorkCount {
    pub fn new(limit: u64) -> WorkCount {
        WorkCount { used: AtomicU64::new(0), committed: AtomicU64::new(0), limit, over: AtomicBool::new(false) }
    }

    pub fn state(&self) -> WorkState {
        WorkState { used: self.used.load(Ordering::Relaxed), over: self.over.load(Ordering::Relaxed) }
    }
}

impl WorkCount {
    #[inline]
    pub fn over(&self) -> bool { self.state().over }

    #[inline]
    pub fn used(&self) -> u64 { self.state().used }

    /// Adds a slice's ops; true when the call is now over its limit. Whether
    /// a call ends over its limit depends only on its total, not on how its
    /// slices ran.
    fn add(&self, ops: u64) -> bool {
        let used = self.used.fetch_add(ops, Ordering::Relaxed).saturating_add(ops);
        if used > self.limit {
            self.over.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Reserves what `size(what is left)` says (a slice and its worst
    /// case) against the limit, atomically. With nothing left while other
    /// slices still run, it waits for them (they may end the call); with
    /// nothing left and nothing running, the smallest slice goes past the
    /// limit and stops the call.
    fn reserve(&self, size: impl Fn(u64) -> (usize, u64)) -> (usize, u64) {
        let mut seen = self.committed.load(Ordering::Acquire);
        loop {
            let left = self.limit.saturating_sub(seen);
            if left == 0 && seen != self.used.load(Ordering::Acquire) && !self.over.load(Ordering::Acquire) {
                std::thread::yield_now();
                seen = self.committed.load(Ordering::Acquire);
                continue;
            }
            let (k, held) = size(left);
            match self.committed.compare_exchange_weak(seen, seen.saturating_add(held), Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return (k, held),
                Err(now) => seen = now,
            }
        }
    }
}

/// How a range of elements runs.
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    Interp,
    Scalar,
    /// NEON ×4 where the kernel has it, else scalar.
    Vector,
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
