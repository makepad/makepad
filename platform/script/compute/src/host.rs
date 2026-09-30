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
//! - **a worst-case cost** in op-equivalents (bounded by the slices'
//!   maximum lengths) and a count of host-memory touches, which the
//!   kernel's cost and admission charge per call;
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

use crate::ir::{RawBuf, Ty};
use std::sync::RwLock;

/// Determinism tier of a host function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The same bits on every machine and build.
    X,
    /// The same bits on the same device and build.
    D,
}

/// A slice parameter.
#[derive(Clone, Copy, Debug)]
pub struct SliceSig {
    /// Most words the function sees (a longer slice is clamped and the
    /// call's error word set).
    pub max_words: u32,
    pub writable: bool,
}

/// Why a host function failed (its results are zero, the error word set).
#[derive(Clone, Debug, PartialEq)]
pub struct HostError(pub String);

/// A kernel buffer range as a host function sees it.
#[derive(Clone, Copy)]
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

/// The function a registry entry calls: scalar arguments (one word each;
/// f64 has none), slices, and one word per result.
pub type HostCall = fn(args: &[u32], slices: &[HostSlice], rets: &mut [u32]) -> Result<(), HostError>;

/// A registered host function.
#[derive(Clone, Copy)]
pub struct HostFn {
    /// The name a kernel calls (`poly.triangulate`).
    pub name: &'static str,
    /// Scalar parameters, in order, after the slices.
    pub params: &'static [Ty],
    /// Slice parameters, first in the call: each is written in the kernel
    /// as three arguments `buffer, word_offset, word_len`.
    pub slices: &'static [SliceSig],
    /// Results (at most one today: the call's value).
    pub rets: &'static [Ty],
    /// Worst-case op-equivalents of one call at the slices' maximum sizes.
    pub max_cost: u64,
    /// Host-memory touches per call (admission charges them as misses).
    pub misses: u32,
    pub tier: Tier,
    pub call: HostCall,
    /// One line for the kernel guide.
    pub doc: &'static str,
}

impl std::fmt::Debug for HostFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HostFn({}: {} slices, {:?} -> {:?}, cost {}, {:?})", self.name, self.slices.len(), self.params, self.rets, self.max_cost, self.tier)
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
pub fn register(f: HostFn) -> Result<u16, HostError> {
    // The built-in entries first (their indices never move).
    drop(registry());
    let mut w = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = w.iter().position(|h| h.name == f.name) {
        return Ok(k as u16);
    }
    if w.len() >= u16::MAX as usize || f.rets.len() > 1 || f.slices.len() > 8 || f.params.len() > 16 || f.params.contains(&Ty::F64) || f.rets.contains(&Ty::F64) {
        return Err(HostError(format!("host function `{}`: at most 8 slices, 16 word parameters and one word result", f.name)));
    }
    w.push(f);
    Ok(w.len() as u16 - 1)
}

/// The entry at `index`.
pub fn get(index: u16) -> Option<HostFn> {
    registry().get(index as usize).copied()
}

/// The index of a host function by name.
pub fn find(name: &str) -> Option<u16> {
    registry().iter().position(|h| h.name == name).map(|k| k as u16)
}

/// Every registered function (the kernel guide lists them).
pub fn all() -> Vec<HostFn> {
    registry().clone()
}

/// A slice argument at run time: buffer index, word offset, word length.
#[derive(Clone, Copy, Debug)]
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
    rets.fill(0);
    let Some(h) = get(f) else { return false };
    if args.len() != h.params.len() || slices.len() != h.slices.len() || rets.len() != h.rets.len() {
        return false;
    }
    let mut ok = true;
    let mut views = [HostSlice::EMPTY; 8];
    for (k, (s, sig)) in slices.iter().zip(h.slices).enumerate() {
        let Some(b) = bufs.get(s.buf as usize) else { return false };
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
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (h.call)(args, views, rets)));
    match r {
        Ok(Ok(())) => ok,
        _ => {
            rets.fill(0);
            false
        }
    }
}

/// The record native code builds on its stack for a call (words):
/// `[f, nargs, nslices, nrets, args..., (buf, off, len) per slice, rets...]`.
pub const REC_HEADER: usize = 4;

/// Native code's entry into the registry: `rec` is the call record (see
/// [`REC_HEADER`]), `table` the kernel's buffer table ((ptr, len) pairs,
/// `nbufs` of them, writability in `writable`), `ctx` the kernel ctx
/// (`K_HOST_ERR` is set when the call fails). Never unwinds.
///
/// # Safety
/// `rec` points at a record of the size its header states, `table` at
/// `nbufs` live (ptr, len) pairs, `ctx` at a kernel ctx.
pub unsafe extern "C" fn trampoline(rec: *mut u32, table: *const u64, ctx: *mut u32, writable: u64) {
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
        invoke(f as u16, args, &slices[..ns], rets, &bufs[..nb.min(64)])
    }));
    if !matches!(r, Ok(true)) {
        *ctx.add(crate::lower::kernel::K_HOST_ERR as usize) = 1;
    }
}

// =========================================================================
// Built-in components
// =========================================================================

/// Most polygon points `poly.triangulate` takes (ear clipping is O(n²)).
pub const TRIANGULATE_MAX_POINTS: u32 = 256;

static BUILTIN: &[HostFn] = &[HostFn {
    name: "poly.triangulate",
    params: &[],
    slices: &[SliceSig { max_words: 2 * TRIANGULATE_MAX_POINTS, writable: false }, SliceSig { max_words: 3 * (TRIANGULATE_MAX_POINTS - 2), writable: true }],
    rets: &[Ty::I32],
    // Ear clipping: n ears, each a scan of n candidates testing n points.
    max_cost: 24 * TRIANGULATE_MAX_POINTS as u64 * TRIANGULATE_MAX_POINTS as u64,
    misses: 4,
    tier: Tier::X,
    call: triangulate,
    doc: "poly.triangulate(pts, off, words, tris, off, words) -> triangles: a simple polygon (vec2 points, either winding, up to 256) into triangle indices (3 i32 words each); -1 when it is not simple",
}];

/// Ear clipping of a simple polygon: IEEE arithmetic only (tier X).
fn triangulate(_args: &[u32], s: &[HostSlice], rets: &mut [u32]) -> Result<(), HostError> {
    let (pts, out) = (&s[0], &s[1]);
    let n = pts.len() / 2;
    if n < 3 {
        rets[0] = 0;
        return Ok(());
    }
    let p = |i: usize| (pts.get_f32(2 * i), pts.get_f32(2 * i + 1));
    // Winding by the shoelace sum (in point order, for determinism).
    let mut area = 0.0f64;
    for i in 0..n {
        let (a, b) = (p(i), p((i + 1) % n));
        area += a.0 as f64 * b.1 as f64 - b.0 as f64 * a.1 as f64;
    }
    let ccw = area > 0.0;
    let cross = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| (b.0 as f64 - a.0 as f64) * (c.1 as f64 - a.1 as f64) - (b.1 as f64 - a.1 as f64) * (c.0 as f64 - a.0 as f64);
    let mut idx = [0u16; TRIANGULATE_MAX_POINTS as usize];
    for (i, x) in idx.iter_mut().enumerate().take(n) {
        *x = i as u16;
    }
    let mut m = n;
    let mut written = 0usize;
    let mut guard = 0;
    let mut i = 0;
    while m > 3 {
        guard += 1;
        if guard > 2 * n * n {
            rets[0] = (-1i32) as u32;
            return Ok(());
        }
        let (ia, ib, ic) = (idx[(i + m - 1) % m] as usize, idx[i % m] as usize, idx[(i + 1) % m] as usize);
        let (a, b, c) = (p(ia), p(ib), p(ic));
        let turn = cross(a, b, c);
        let convex = if ccw { turn > 0.0 } else { turn < 0.0 };
        let mut ear = convex;
        if ear {
            for &j in idx.iter().take(m) {
                let j = j as usize;
                if j == ia || j == ib || j == ic {
                    continue;
                }
                let q = p(j);
                let (d1, d2, d3) = (cross(a, b, q), cross(b, c, q), cross(c, a, q));
                let inside = if ccw { d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0 } else { d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0 };
                if inside {
                    ear = false;
                    break;
                }
            }
        }
        if ear {
            if written + 3 > out.len() {
                return Err(HostError("the triangle buffer is too small".into()));
            }
            out.set(written, ia as u32);
            out.set(written + 1, ib as u32);
            out.set(written + 2, ic as u32);
            written += 3;
            let at = i % m;
            for k in at..m - 1 {
                idx[k] = idx[k + 1];
            }
            m -= 1;
            i = if at == 0 { 0 } else { at - 1 };
        } else {
            i = (i + 1) % m;
        }
    }
    if written + 3 > out.len() {
        return Err(HostError("the triangle buffer is too small".into()));
    }
    out.set(written, idx[0] as u32);
    out.set(written + 1, idx[1] as u32);
    out.set(written + 2, idx[2] as u32);
    rets[0] = (written / 3 + 1) as u32;
    Ok(())
}
