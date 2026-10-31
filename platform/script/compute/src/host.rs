//! Host components: the few heavy Rust functions a kernel may call.
//!
//! Utilities are Splash (std modules, inlined). A host function exists only
//! for a component that is genuinely heavy or reads files (triangulation,
//! booleans, font outlines, spatial indices), and each one is registered
//! with a full contract:
//!
//! - **a typed signature**: scalar parameters and results (`Ty`), and
//!   buffer *slices* (a kernel buffer, a word offset and a word length),
//!   each with a maximum length and whether the function writes it;
//! - **a worst-case cost as a function of its input**: op-equivalents for
//!   the slice lengths of a call (monotone in each), and a count of
//!   host-memory touches. A kernel's cost and admission charge a call at
//!   its constant lengths, or at the per-call limit; at run time a call
//!   whose actual input costs more than the call's limit (ctx
//!   `K_HOST_LIMIT`, set from admission) is refused before it runs, with
//!   the error word set. Budgets reject oversized inputs; there is no
//!   capability cap beyond the memory bound `max_words`;
//! - **a determinism tier**: X (same bits on every machine) or D (same
//!   device); portable kernels may call only X functions.
//!
//! A call is the AIR statement [`crate::ir::Stmt::CallHost`]: the function
//! is a compile-time index into this registry (generated code never forms a
//! function address from data), and `ir::validate` checks the index, the
//! arity and types, and that every slice names a bound buffer with the
//! right write permission.
//!
//! At run time a slice is clamped to its buffer's real length and to the
//! signature's maximum, and the function sees it as a [`HostSlice`]: word
//! reads and writes through relaxed atomics, never a Rust reference over
//! kernel memory (one allocation may be bound twice, and parallel workers
//! share buffers). A function never unwinds into generated code: a panic or
//! an error sets the call's error word (ctx `K_HOST_ERR`) and its results
//! are zero. Functions are pure: no I/O at call time, no global state.
//!
//! The registry is append-only: built-in functions first, then any the host
//! registers at start-up (a text crate registering `text.outline`), before
//! kernels that call them compile.
use makepad_relax_api::relax_api;

use crate::ir::{RawBuf, Ty};
use std::sync::RwLock;

/// Determinism tier of a host function.
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Tier {
    /// The same bits on every machine and build.
    X,
    /// The same bits on the same device and build.
    D,
}

/// A slice parameter.
#[relax_api(plain)]
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SliceSig {
    /// Most words the function sees (a longer slice is clamped and the
    /// call's error word set).
    pub max_words: u32,
    pub writable: bool,
}

/// A kernel buffer range as a host function sees it.
#[relax_api(plain)]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct HostSlice {
    buf: RawBuf,
    start: usize,
    len: usize,
}

impl HostSlice {
    const EMPTY: HostSlice = HostSlice { buf: RawBuf { ptr: std::ptr::null_mut(), len: 0, writable: false }, start: 0, len: 0 };

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Word `i` (0 past the end).
    pub fn get(&self, i: usize) -> u32 {
        if i >= self.len {
            return 0;
        }
        self.buf.load_word(self.start + i)
    }

    pub fn get_f32(&self, i: usize) -> f32 {
        f32::from_bits(self.get(i))
    }

    /// Writes word `i` (ignored past the end or when read-only).
    pub fn set(&self, i: usize, x: u32) {
        if i < self.len {
            self.buf.store_word(self.start + i, x);
        }
    }

    pub fn set_f32(&self, i: usize, x: f32) {
        self.set(i, x.to_bits());
    }
}

/// Borrowed arguments for one callback. Only the runtime constructs this
/// frame; its views expire when the callback returns.
#[relax_api(plain)]
#[repr(C)]
pub struct HostCallFrame {
    args: *const u32,
    nargs: usize,
    slices: *const HostSlice,
    nslices: usize,
    rets: *mut u32,
    nrets: usize,
}
impl HostCallFrame {
    #[inline]
    pub fn parts(&mut self) -> (&[u32], &[HostSlice], &mut [u32]) {
        // SAFETY: invoke_counted lends these disjoint live slices for the
        // callback; the returned views cannot outlive this frame borrow.
        unsafe { (std::slice::from_raw_parts(self.args, self.nargs), std::slice::from_raw_parts(self.slices, self.nslices), std::slice::from_raw_parts_mut(self.rets, self.nrets)) }
    }
}

/// A borrowed diagnostic receiver. Message bytes are borrowed only until
/// the receiver returns. The kernel runtime records failure without allocating.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ErrorSink<'a> {
    context: *mut (),
    write: unsafe extern "C-unwind" fn(*mut (), *const u8, usize),
    borrow: std::marker::PhantomData<&'a mut ()>,
}
impl<'a> ErrorSink<'a> {
    /// # Safety
    /// The context and receiver must remain valid for 'a. The receiver
    /// must accept borrowed UTF-8 bytes, and concurrent calls if shared.
    pub unsafe fn new(context: *mut (), write: unsafe extern "C-unwind" fn(*mut (), *const u8, usize)) -> Self {
        Self { context, write, borrow: std::marker::PhantomData }
    }
    #[inline]
    pub fn report(&self, message: &str) {
        unsafe { (self.write)(self.context, message.as_ptr(), message.len()) }
    }
    /// Report a failure and return the callback's failure status.
    pub fn fail(&self, message: &str) -> bool {
        self.report(message);
        false
    }
}

/// Monotone worst-case op-equivalents for borrowed, clamped slice lengths.
pub type HostCost = unsafe extern "C-unwind" fn(context: *mut (), words: *const u32, len: usize) -> u64;
/// Returns success; reporting to the sink also marks the call failed.
pub type HostCall = unsafe extern "C-unwind" fn(context: *mut (), frame: &mut HostCallFrame, errors: ErrorSink<'_>) -> bool;

/// Process-lived callback code and context. No Rust function pointers or
/// owned Rust results cross this contract.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostCallbacks {
    context: *mut (),
    cost: HostCost,
    call: HostCall,
}
// SAFETY: constructing a table requires thread-safe process-lived callbacks.
unsafe impl Send for HostCallbacks {}
unsafe impl Sync for HostCallbacks {}
impl HostCallbacks {
    /// # Safety
    /// Code and context must remain live for the process (the registry is
    /// append-only). Both callbacks must support concurrent calls, accept
    /// the supplied views without retaining them, and access only their
    /// context and declared buffers. Cost must be monotone and bound work.
    pub const unsafe fn new(context: *mut (), cost: HostCost, call: HostCall) -> Self {
        Self { context, cost, call }
    }
    #[inline]
    fn cost_of(&self, lens: &[u32]) -> u64 {
        unsafe { (self.cost)(self.context, lens.as_ptr(), lens.len()) }
    }
}

/// A complete registration descriptor. Metadata has process lifetime;
/// the registry copies the descriptor and owns its single shared index.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostFn {
    /// Name used in kernel source.
    pub name: &'static str,
    /// Word scalar parameters, after the buffer slices.
    pub params: &'static [Ty],
    /// Each slice is supplied as buffer, word offset and word length.
    pub slices: &'static [SliceSig],
    /// At most one word result.
    pub rets: &'static [Ty],
    pub callbacks: HostCallbacks,
    /// Host-memory touches per call (charged as misses by admission).
    pub misses: u32,
    pub tier: Tier,
    /// One line for the kernel guide.
    pub doc: &'static str,
}

/// Registration status owned by value; no Rust error allocation crosses
/// the compiled boundary.
#[relax_api(plain)]
#[repr(C)]
pub struct Registration {
    index: u32,
    error: u32,
}
impl Registration {
    pub fn index(&self) -> Option<u16> { if self.error == 0 { Some(self.index as u16) } else { None } }
    pub fn error(&self) -> &'static str {
        match self.error {
            0 => "",
            1 => "host registry exhausted its u16 function indices",
            _ => "host function: at most 8 slices, 16 word parameters and one word result; f64 is unsupported",
        }
    }
    #[inline(always)]
    pub fn into_result(self) -> Result<u16, &'static str> {
        self.index().ok_or_else(|| self.error())
    }
}

impl std::fmt::Debug for HostFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HostFn({}: {} slices, {:?} -> {:?}, cost at most {}, {:?})", self.name, self.slices.len(), self.params, self.rets, self.max_cost(), self.tier)
    }
}

impl HostFn {
    /// The cost at the slices' maximum lengths (the worst any call may be).
    pub fn max_cost(&self) -> u64 {
        let mut lens = [0u32; 8];
        for (k, s) in self.slices.iter().enumerate().take(8) {
            lens[k] = s.max_words;
        }
        self.callbacks.cost_of(&lens[..self.slices.len().min(8)])
    }

    /// The cost of a call with these slice lengths (clamped to the maxima).
    pub fn cost_of(&self, lens: &[u32]) -> u64 {
        let mut l = [0u32; 8];
        for (k, s) in self.slices.iter().enumerate().take(8) {
            l[k] = lens.get(k).copied().unwrap_or(s.max_words).min(s.max_words);
        }
        self.callbacks.cost_of(&l[..self.slices.len().min(8)])
    }
}

static REGISTRY: RwLock<Vec<HostFn>> = RwLock::new(Vec::new());

fn registry() -> std::sync::RwLockReadGuard<'static, Vec<HostFn>> {
    {
        let r = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
        if !r.is_empty() {
            return r;
        }
    }
    let mut w = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    if w.is_empty() {
        w.extend_from_slice(BUILTIN);
    }
    drop(w);
    REGISTRY.read().unwrap_or_else(|e| e.into_inner())
}

/// Registers a host function (at start-up); returns its index. A name
/// registered twice keeps its first entry (and index).
#[relax_api]
pub fn register(f: HostFn) -> Registration {
    // The built-in entries first (their indices never move).
    drop(registry());
    let mut w = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = w.iter().position(|h| h.name == f.name) {
        return Registration { index: k as u32, error: 0 };
    }
    if w.len() >= u16::MAX as usize {
        return Registration { index: 0, error: 1 };
    }
    if f.rets.len() > 1 || f.slices.len() > 8 || f.params.len() > 16 || f.params.contains(&Ty::F64) || f.rets.contains(&Ty::F64) {
        return Registration { index: 0, error: 2 };
    }
    w.push(f);
    Registration { index: w.len() as u32 - 1, error: 0 }
}

/// The entry at `index`.
#[relax_api]
pub fn get(index: u16) -> Option<HostFn> {
    registry().get(index as usize).copied()
}

/// The index of a host function by name.
pub fn find(name: &str) -> Option<u16> {
    registry().iter().position(|h| h.name == name).map(|k| k as u16)
}

/// The registry's entries in index order (the registry is append-only),
/// through [`get`] alone: it never reads the registry statics.
#[inline(always)]
pub fn entries() -> impl Iterator<Item = HostFn> {
    (0..=u16::MAX).map_while(|index| get(index))
}

/// A slice argument at run time: buffer index, word offset, word length.
#[relax_api(plain)]
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SliceRaw {
    pub buf: u32,
    pub off: u32,
    pub len: u32,
}

/// Calls host function `f` (checked: index, arity, slices clamped to their
/// buffers and maxima). Returns false when the call failed (bad index,
/// clamped slice, error or panic): results are then zero and the caller
/// sets the error word. Used by the interpreter and, through
/// [`trampoline`], native code.
pub fn invoke(f: u16, args: &[u32], slices: &[SliceRaw], rets: &mut [u32], bufs: &[RawBuf]) -> bool {
    invoke_limited(f, args, slices, rets, bufs, 0)
}

/// [`invoke`] under a per-call budget (op-equivalents; 0: none): a call
/// whose actual input costs more is refused before it runs.
pub fn invoke_limited(f: u16, args: &[u32], slices: &[SliceRaw], rets: &mut [u32], bufs: &[RawBuf], limit: u64) -> bool {
    invoke_counted(f, args, slices, rets, bufs, limit).0
}

/// [`invoke_limited`], and the op-equivalents the call counts: its declared
/// cost at the input it was actually given (0 when it did not run). A
/// kernel adds them to its counted work (`K_HOST_WORK`).
pub fn invoke_counted(f: u16, args: &[u32], slices: &[SliceRaw], rets: &mut [u32], bufs: &[RawBuf], limit: u64) -> (bool, u64) {
    rets.fill(0);
    let Some(h) = get(f) else { return (false, 0) };
    if args.len() != h.params.len() || slices.len() != h.slices.len() || rets.len() != h.rets.len() {
        return (false, 0);
    }
    let mut ok = true;
    let mut views = [HostSlice::EMPTY; 8];
    for (k, (s, sig)) in slices.iter().zip(h.slices).enumerate() {
        let Some(b) = bufs.get(s.buf as usize) else { return (false, 0) };
        let start = (s.off as usize).min(b.len);
        let mut len = (s.len as usize).min(b.len - start);
        if len > sig.max_words as usize {
            len = sig.max_words as usize;
            ok = false;
        }
        if (s.len as usize) > len {
            // Clamped: the function runs on what fits, and it is reported.
            ok = false;
        }
        let buf = RawBuf { ptr: b.ptr, len: b.len, writable: b.writable && sig.writable };
        views[k] = HostSlice { buf, start, len };
    }
    let views = &views[..slices.len()];
    let mut lens = [0u32; 8];
    for (k, v) in views.iter().enumerate() {
        lens[k] = v.len as u32;
    }
    let cost = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.callbacks.cost_of(&lens[..views.len()]))) {
        Ok(cost) => cost,
        Err(_) => return (false, 0),
    };
    if limit != 0 && cost > limit {
        return (false, 0);
    }
    let mut frame = HostCallFrame { args: args.as_ptr(), nargs: args.len(), slices: views.as_ptr(), nslices: views.len(), rets: rets.as_mut_ptr(), nrets: rets.len() };
    let mut failed = false;
    unsafe extern "C-unwind" fn report(context: *mut (), _message: *const u8, _len: usize) { *context.cast::<bool>() = true; }
    // SAFETY: this stack receiver is used synchronously on this invocation.
    let errors = unsafe { ErrorSink::new((&mut failed as *mut bool).cast(), report) };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { (h.callbacks.call)(h.callbacks.context, &mut frame, errors) }));
    match r {
        Ok(true) if !failed => (ok, cost),
        _ => {
            rets.fill(0);
            (false, cost)
        }
    }
}

/// The record native code builds on its stack for a call (words):
/// `[f, nargs, nslices, nrets, args..., (buf, off, len) per slice, rets...]`.
pub const REC_HEADER: usize = 4;

/// Native code's entry into the registry: `rec` is the call record (see
/// [`REC_HEADER`]), `table` the kernel's buffer table ((ptr, len) pairs,
/// `nbufs` of them, writability in `writable`), `ctx` the kernel ctx
/// (`K_HOST_ERR` is set when the call fails). Never unwinds. On x86-64 it takes the System V
/// ABI the code generator emits for host calls on every OS (Windows' C ABI passes in rcx/rdx).
///
/// # Safety
/// `rec` points at a record of the size its header states, `table` at
/// `nbufs` live (ptr, len) pairs, `ctx` at a kernel ctx.
#[cfg(target_arch = "x86_64")]
pub unsafe extern "sysv64" fn trampoline(rec: *mut u32, table: *const u64, ctx: *mut u32, writable: u64) {
    trampoline_body(rec, table, ctx, writable)
}

/// [`trampoline`] with the platform C ABI (arm64: AAPCS64, what compute's arm64 back end calls).
///
/// # Safety
/// As [`trampoline`].
#[cfg(not(target_arch = "x86_64"))]
pub unsafe extern "C" fn trampoline(rec: *mut u32, table: *const u64, ctx: *mut u32, writable: u64) {
    trampoline_body(rec, table, ctx, writable)
}

unsafe fn trampoline_body(rec: *mut u32, table: *const u64, ctx: *mut u32, writable: u64) {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let head = std::slice::from_raw_parts(rec, REC_HEADER);
        let (f, na, ns, nr) = (head[0], head[1] as usize, head[2] as usize, head[3] as usize);
        if na > 16 || ns > 8 || nr > 1 {
            return false;
        }
        let args = std::slice::from_raw_parts(rec.add(REC_HEADER), na);
        let sl = std::slice::from_raw_parts(rec.add(REC_HEADER + na), 3 * ns);
        let rets = std::slice::from_raw_parts_mut(rec.add(REC_HEADER + na + 3 * ns), nr);
        let mut slices = [SliceRaw { buf: 0, off: 0, len: 0 }; 8];
        let mut bufs = [RawBuf { ptr: std::ptr::null_mut(), len: 0, writable: false }; 64];
        let mut nb = 0;
        for k in 0..ns {
            slices[k] = SliceRaw { buf: sl[3 * k], off: sl[3 * k + 1], len: sl[3 * k + 2] };
            nb = nb.max(sl[3 * k] as usize + 1);
        }
        for (k, b) in bufs.iter_mut().enumerate().take(nb.min(64)) {
            *b = RawBuf { ptr: *table.add(2 * k) as *mut u32, len: *table.add(2 * k + 1) as usize, writable: writable >> k & 1 != 0 };
        }
        let limit = *ctx.add(crate::lower::kernel::K_HOST_LIMIT as usize) as u64;
        let (ok, cost) = invoke_counted(f as u16, args, &slices[..ns], rets, &bufs[..nb.min(64)], limit);
        let counted = ctx.add(crate::lower::kernel::K_HOST_WORK as usize);
        *counted = (*counted as u64).saturating_add(cost).min(u32::MAX as u64) as u32;
        ok
    }));
    if !matches!(r, Ok(true)) {
        *ctx.add(crate::lower::kernel::K_HOST_ERR as usize) = 1;
    }
}

// =========================================================================
// Built-in components
// =========================================================================

/// Most polygon points `poly.triangulate` takes (a memory bound; the cost
/// grows as n²: budgets decide what an untrusted kernel may pass).
pub const TRIANGULATE_MAX_POINTS: u32 = 1 << 16;

/// Ear clipping's worst case for `words` words of points, in the ops
/// [`triangulate_ops`] counts: the area pass, every vertex's first ear test
/// (a scan of at most n points, 12 ops a point test), the two neighbours'
/// re-tests at each of the n clips (the same scans), and the ear searches
/// (at most n steps of 4 ops each).
unsafe extern "C-unwind" fn triangulate_cost(_context: *mut (), words: *const u32, len: usize) -> u64 {
    let lens = std::slice::from_raw_parts(words, len);
    let n = (lens.first().copied().unwrap_or(0) / 2) as u64;
    12 * 3 * n * n + 4 * n * n + 64 * n + 256
}

static BUILTIN: &[HostFn] = &[HostFn {
    name: "poly.triangulate",
    params: &[],
    slices: &[SliceSig { max_words: 2 * TRIANGULATE_MAX_POINTS, writable: false }, SliceSig { max_words: 3 * TRIANGULATE_MAX_POINTS, writable: true }],
    rets: &[Ty::I32],
    // SAFETY: stateless process-lived callbacks; all memory comes from the frame.
    callbacks: unsafe { HostCallbacks::new(std::ptr::null_mut(), triangulate_cost, triangulate) },
    misses: 4,
    tier: Tier::X,
    doc: "poly.triangulate(pts, off, words, tris, off, words) -> triangles: a simple polygon (vec2 points, either winding) into triangle indices (3 i32 words each); -1 when it is not simple",
}];

/// Ear clipping of a simple polygon in O(n²): a vertex's ear status is
/// computed once and re-tested only when a neighbour is clipped; the next
/// ear is found by walking forward from the last clipped one. IEEE
/// arithmetic only (tier X).
unsafe extern "C-unwind" fn triangulate(_context: *mut (), frame: &mut HostCallFrame, errors: ErrorSink<'_>) -> bool {
    let (_args, s, rets) = frame.parts();
    let (pts, out) = (&s[0], &s[1]);
    let n = pts.len() / 2;
    let p: Vec<(f64, f64)> = (0..n).map(|i| (pts.get_f32(2 * i) as f64, pts.get_f32(2 * i + 1) as f64)).collect();
    let need = 3 * n.saturating_sub(2);
    if n >= 3 && out.len() < need {
        return errors.fail(&format!("the triangle buffer holds {} words; {} points need {}", out.len(), n, need));
    }
    let (r, _) = ear_clip(&p, &mut |at, w| out.set(at, w));
    rets[0] = r as u32;
    true
}

/// The ops the triangulation runs on `pts` (vec2 points), counted as
/// [`triangulate_cost`] declares them: what its declaration is held to.
pub fn triangulate_ops(pts: &[f32]) -> u64 {
    let p: Vec<(f64, f64)> = pts.chunks_exact(2).map(|c| (c[0] as f64, c[1] as f64)).collect();
    ear_clip(&p, &mut |_, _| {}).1
}

/// Triangles of `p` into `out(word, value)`: the triangle count (-1: not
/// simple), and the ops counted (12 a point test, 4 an ear-search step,
/// 64 a vertex's setup).
fn ear_clip(p: &[(f64, f64)], out: &mut dyn FnMut(usize, u32)) -> (i32, u64) {
    let n = p.len();
    if n < 3 {
        return (0, 0);
    }
    let tests = std::cell::Cell::new(0u64);
    let mut scans = 0u64;
    let ops = |tests: u64, scans: u64| 12 * tests + 4 * scans + 64 * n as u64;
    let mut area = 0.0f64;
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        area += a.0 * b.1 - b.0 * a.1;
    }
    if !(area.abs() > 0.0) {
        return (-1, ops(0, 0));
    }
    let sign = if area > 0.0 { 1.0 } else { -1.0 };
    let cross = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)) * sign;
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let is_ear = |i: usize, next: &[usize], prev: &[usize]| {
        let (ia, ic) = (prev[i], next[i]);
        let (a, b, c) = (p[ia], p[i], p[ic]);
        tests.set(tests.get() + 1);
        if cross(a, b, c) <= 0.0 {
            return false;
        }
        // No other remaining vertex inside or on the triangle (vertices
        // equal to a corner are allowed: duplicated points).
        let mut j = next[ic];
        while j != ia {
            tests.set(tests.get() + 1);
            let q = p[j];
            if q != a && q != b && q != c && cross(a, b, q) >= 0.0 && cross(b, c, q) >= 0.0 && cross(c, a, q) >= 0.0 {
                return false;
            }
            j = next[j];
        }
        true
    };
    let mut ear: Vec<bool> = (0..n).map(|i| is_ear(i, &next, &prev)).collect();
    let mut m = n;
    let mut at = 0usize;
    let mut cur = 0usize;
    let mut written = 0usize;
    while m > 3 {
        let mut i = cur;
        let mut found = false;
        for _ in 0..m {
            scans += 1;
            if ear[i] {
                found = true;
                break;
            }
            i = next[i];
        }
        if !found {
            return (-1, ops(tests.get(), scans));
        }
        let (a, c) = (prev[i], next[i]);
        out(at, a as u32);
        out(at + 1, i as u32);
        out(at + 2, c as u32);
        at += 3;
        written += 1;
        next[a] = c;
        prev[c] = a;
        m -= 1;
        ear[a] = is_ear(a, &next, &prev);
        ear[c] = is_ear(c, &next, &prev);
        cur = c;
    }
    out(at, prev[cur] as u32);
    out(at + 1, cur as u32);
    out(at + 2, next[cur] as u32);
    ((written + 1) as i32, ops(tests.get(), scans))
}

