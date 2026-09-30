//! Native ARM64 backend: AIR -> AArch64 machine code (scalar FP/integer;
//! SIMD across voices is the next step), hand-encoded, in W^X executable
//! memory. Bit-identical to [`crate::ir::run`]: each AIR op maps to IEEE
//! single-precision instructions with no fusion (FMADD only for AIR's
//! explicit `Fma`, which `math: fast` kernels contain), integer ops
//! with AIR's total semantics (SDIV gives 0 on a zero divisor, shifts take
//! their amount mod 32, FCVTZS saturates with NaN -> 0 exactly like Rust's
//! `as`), selects for min/max, and clamped memory offsets.
//!
//! ABI: `extern "C" fn(ctx, state, shared, io: *const [*mut f32; 4], n)`;
//! io = [in_l, in_r, out_l, out_r]. The frame region and spill slots live
//! in the native stack frame.
//!
//! Register allocation is linear scan over the pre-order numbering of the
//! structured program. A value or variable whose first reference opens its
//! block and whose references all stay in that block is dead at the block
//! entry, so its interval grows only over loops nested inside that block;
//! otherwise it grows over every loop around a reference. One-iteration
//! loops (inlined-function wrappers) never repeat and do not extend.

use crate::ir::{Bin, Block, Cmp, Op, Program, Region, Stmt, Ty, Un, Val};

// =========================================================================
// Executable memory
// =========================================================================

mod sys {
    use std::ffi::c_void;
    pub const PROT_READ: i32 = 1;
    pub const PROT_WRITE: i32 = 2;
    pub const PROT_EXEC: i32 = 4;
    pub const MAP_PRIVATE: i32 = 0x02;
    #[cfg(target_os = "macos")]
    pub const MAP_ANON: i32 = 0x1000;
    #[cfg(target_os = "macos")]
    pub const MAP_JIT: i32 = 0x800;
    #[cfg(not(target_os = "macos"))]
    pub const MAP_ANON: i32 = 0x20;
    extern "C" {
        pub fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> i32;
        #[cfg(not(target_os = "macos"))]
        pub fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
        #[cfg(target_os = "macos")]
        pub fn pthread_jit_write_protect_np(enabled: i32);
        #[cfg(target_os = "macos")]
        pub fn sys_icache_invalidate(start: *mut c_void, len: usize);
        #[cfg(not(target_os = "macos"))]
        pub fn __clear_cache(start: *mut c_void, end: *mut c_void);
    }
}

/// Native code for one program. Immutable once built; the pages are
/// unmapped on drop (off the audio thread: shaders are dropped by their
/// owner's graph swap).
pub struct Code {
    ptr: *mut u8,
    map_len: usize,
    len: usize,
}

// SAFETY: the mapping is immutable after construction and the code is
// reentrant (all mutable data comes in through arguments).
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

type Entry = unsafe extern "C" fn(*mut u32, *mut u32, *mut u32, *const *mut f32, u32);

impl Code {
    pub(crate) fn new(words: &[u32]) -> Option<Code> {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = words;
            return None;
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        unsafe {
            let len = words.len() * 4;
            let map_len = (len + 16383) & !16383;
            #[cfg(target_os = "macos")]
            let ptr = sys::mmap(
                std::ptr::null_mut(),
                map_len,
                sys::PROT_READ | sys::PROT_WRITE | sys::PROT_EXEC,
                sys::MAP_PRIVATE | sys::MAP_ANON | sys::MAP_JIT,
                -1,
                0,
            );
            #[cfg(not(target_os = "macos"))]
            let ptr = sys::mmap(std::ptr::null_mut(), map_len, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_PRIVATE | sys::MAP_ANON, -1, 0);
            if ptr.is_null() || ptr as isize == -1 {
                return None;
            }
            #[cfg(target_os = "macos")]
            sys::pthread_jit_write_protect_np(0);
            std::ptr::copy_nonoverlapping(words.as_ptr() as *const u8, ptr as *mut u8, len);
            #[cfg(target_os = "macos")]
            {
                sys::pthread_jit_write_protect_np(1);
                sys::sys_icache_invalidate(ptr, len);
            }
            #[cfg(not(target_os = "macos"))]
            {
                if sys::mprotect(ptr, map_len, sys::PROT_READ | sys::PROT_EXEC) != 0 {
                    sys::munmap(ptr, map_len);
                    return None;
                }
                sys::__clear_cache(ptr, (ptr as *mut u8).add(len) as *mut _);
            }
            Some(Code { ptr: ptr as *mut u8, map_len, len })
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The machine code as instruction words (for disassembly and tests).
    pub fn words(&self) -> &[u32] {
        // SAFETY: the mapping holds `len` bytes of code, readable for the
        // life of `self`.
        unsafe { std::slice::from_raw_parts(self.ptr as *const u32, self.len / 4) }
    }

    /// Runs the program. The caller guarantees the slice sizes the program
    /// was compiled for (checked by `AudioShader::run`) and n >= 1.
    ///
    /// # Safety
    /// `shared` points at the program's shared tables, alive for the call;
    /// it is written only by an `init()` program (validated: no other
    /// program stores to shared memory).
    pub unsafe fn run(&self, ctx: &mut [u32], state: &mut [u32], shared: *mut u32, ins: [&[f32]; 2], outs: [&mut [f32]; 2], n: u32) {
        let [o0, o1] = outs;
        let io = [ins[0].as_ptr() as *mut f32, ins[1].as_ptr() as *mut f32, o0.as_mut_ptr(), o1.as_mut_ptr()];
        unsafe {
            let f: Entry = std::mem::transmute(self.ptr);
            f(ctx.as_mut_ptr(), state.as_mut_ptr(), shared, io.as_ptr(), n);
        }
    }
}

impl Code {
    /// Runs a kernel: `table` holds (pointer, length in words) per host
    /// buffer, at least two entries (the prologue reads four words).
    ///
    /// # Safety
    /// The pointers and lengths must describe live buffers the kernel may
    /// read and write, and ctx/state/shared must be the sizes the program
    /// was compiled for.
    pub unsafe fn run_kernel(&self, ctx: *mut u32, state: *mut u32, shared: *mut u32, table: *const u64, n: u32) {
        let f: Entry = std::mem::transmute(self.ptr);
        f(ctx, state, shared, table as *const *mut f32, n);
    }
}

impl Drop for Code {
    fn drop(&mut self) {
        unsafe {
            sys::munmap(self.ptr as *mut _, self.map_len);
        }
    }
}

// =========================================================================
// Liveness and register allocation
// =========================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Loc {
    /// W register (GPR) or S register (FP) number.
    Reg(u8),
    /// Byte offset of a 4-byte spill slot from sp.
    Stack(u32),
}

const GPR_POOL: &[u8] = &[9, 10, 11, 12, 13, 14, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28];
const FP_POOL: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 8, 9, 10, 11, 12, 13, 14, 15];
/// Scratch registers: x15/x16/x17 and s29/s30/s31 (x15 holds a host
/// buffer's base pointer).
const XS2: u8 = 15;
const XS0: u8 = 16;
const XS1: u8 = 17;
const FS0: u8 = 30;
const FS1: u8 = 31;
const FS2: u8 = 29;

/// Allocation entity: a Val, a Var or a loop counter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Ent {
    Val(u32),
    Var(u32),
    Counter(u32),
}

struct Refs {
    first: u32,
    last: u32,
    /// Spill cost: each reference weighs 8^(repeating loops around it).
    weight: u64,
    /// Block (index into `blocks`) holding the first reference.
    block: u32,
    /// Every reference position.
    at: Vec<u32>,
}

struct Liveness {
    /// (start, end, repeats) of every Loop stmt in pre-order.
    loops: Vec<(u32, u32, bool)>,
    /// (start, end) of every block.
    blocks: Vec<(u32, u32)>,
    refs: std::collections::HashMap<Ent, Refs>,
    pos: u32,
    loop_count: u32,
    /// Repeating loops around the current statement.
    depth: u32,
}

impl Liveness {
    fn touch(&mut self, e: Ent, block: u32) {
        let pos = self.pos;
        let w = 8u64.pow(self.depth.min(6));
        let r = self.refs.entry(e).or_insert(Refs { first: pos, last: pos, weight: 0, block, at: Vec::new() });
        r.last = pos;
        r.weight += w;
        r.at.push(pos);
    }

    fn walk(&mut self, b: &Block) {
        let bid = self.blocks.len() as u32;
        self.blocks.push((self.pos, 0));
        for s in b {
            self.pos += 1;
            match s {
                Stmt::Def(v, op) => {
                    for u in crate::ir::op_uses(op) {
                        self.touch(Ent::Val(u.0), bid);
                    }
                    if let Op::Get(var) = op {
                        self.touch(Ent::Var(var.0), bid);
                    }
                    self.touch(Ent::Val(v.0), bid);
                }
                Stmt::Set(var, v) => {
                    self.touch(Ent::Val(v.0), bid);
                    self.touch(Ent::Var(var.0), bid);
                }
                Stmt::Store { off, val, .. } => {
                    if let Some(o) = off {
                        self.touch(Ent::Val(o.0), bid);
                    }
                    self.touch(Ent::Val(val.0), bid);
                }
                Stmt::Out { idx, val, .. } => {
                    self.touch(Ent::Val(idx.0), bid);
                    self.touch(Ent::Val(val.0), bid);
                }
                Stmt::If(c, t, e) => {
                    self.touch(Ent::Val(c.0), bid);
                    self.walk(t);
                    self.walk(e);
                }
                Stmt::Loop { cap, body } => {
                    let id = self.loop_count;
                    self.loop_count += 1;
                    let li = self.loops.len();
                    self.loops.push((self.pos, 0, *cap > 1));
                    self.touch(Ent::Counter(id), bid);
                    self.depth += (*cap > 1) as u32;
                    self.walk(body);
                    self.depth -= (*cap > 1) as u32;
                    self.pos += 1;
                    self.touch(Ent::Counter(id), bid);
                    self.loops[li].1 = self.pos;
                }
                Stmt::Break(_) | Stmt::Continue(_) => {}
                Stmt::CallHost { args, slices, rets, .. } => {
                    for a in args {
                        self.touch(Ent::Val(a.0), bid);
                    }
                    for x in slices {
                        self.touch(Ent::Val(x.off.0), bid);
                        self.touch(Ent::Val(x.len.0), bid);
                    }
                    for r in rets {
                        self.touch(Ent::Val(r.0), bid);
                    }
                }
            }
        }
        self.pos += 1;
        self.blocks[bid as usize].1 = self.pos;
    }

    /// The live interval of an entity.
    fn interval(&self, r: &Refs) -> (u32, u32) {
        let (mut s, mut e) = (r.first, r.last);
        let (bs, be) = self.blocks[r.block as usize];
        let fresh = r.at.iter().all(|p| *p >= bs && *p <= be);
        for &(ls, le, repeats) in &self.loops {
            if !repeats {
                continue;
            }
            // `at` is in walk order (ascending): the first reference past
            // the loop's start decides whether one lies inside it.
            let next = r.at.partition_point(|p| *p <= ls);
            let encloses_ref = r.at.get(next).is_some_and(|p| *p < le);
            if !encloses_ref {
                continue;
            }
            let encloses_first = r.first > ls && r.first < le;
            if fresh && encloses_first {
                continue;
            }
            s = s.min(ls);
            e = e.max(le);
        }
        (s, e)
    }
}

pub(crate) struct Alloc {
    pub(crate) locs: std::collections::HashMap<Ent, Loc>,
    pub(crate) spill_bytes: u32,
}

fn allocate(p: &Program) -> Alloc {
    let ty = |e: Ent| match e {
        Ent::Val(v) => Some(p.vals[v as usize]),
        Ent::Var(v) => Some(p.vars[v as usize]),
        Ent::Counter(_) => None,
    };
    let class = |e: Ent| matches!(ty(e), Some(Ty::F32 | Ty::F64)) as usize;
    let slot = |e: Ent| if ty(e) == Some(Ty::F64) { 8 } else { 4 };
    allocate_with(p, class, &[GPR_POOL, FP_POOL], slot, false)
}

/// Linear-scan allocation of every entity into register class
/// `class(e)` (an index into `pools`), spilling to stack slots of
/// `slot(e)` bytes (aligned to their size).
/// `reuse_at_def`: a definition may take the register of an operand whose
/// last use it is (the backend reads every operand before writing the
/// result).
pub(crate) fn allocate_with(p: &Program, class: impl Fn(Ent) -> usize, pools: &[&[u8]], slot: impl Fn(Ent) -> u32, reuse_at_def: bool) -> Alloc {
    // With reuse, a fused multiply-add's result prefers its addend's
    // register (the accumulator then needs no copy).
    let mut hint: std::collections::HashMap<Ent, Ent> = std::collections::HashMap::new();
    if reuse_at_def {
        fn walk(b: &Block, hint: &mut std::collections::HashMap<Ent, Ent>) {
            for s in b {
                match s {
                    Stmt::Def(v, Op::Fma(_, _, _, c)) => {
                        hint.insert(Ent::Val(v.0), Ent::Val(c.0));
                    }
                    Stmt::If(_, t, e) => {
                        walk(t, hint);
                        walk(e, hint);
                    }
                    Stmt::Loop { body, .. } => walk(body, hint),
                    _ => {}
                }
            }
        }
        walk(&p.body, &mut hint);
    }
    let mut lv = Liveness { loops: Vec::new(), blocks: Vec::new(), refs: Default::default(), pos: 0, loop_count: 0, depth: 0 };
    lv.walk(&p.body);
    let mut ivs: Vec<(u32, u32, Ent)> = lv.refs.iter().map(|(e, r)| {
        let (s, t) = lv.interval(r);
        (s, t, *e)
    }).collect();
    // Spill priority: references (weighted by loop depth) per position
    // covered; the lowest is spilled first (a param or constant used once
    // per element over the whole loop, never a hot temporary or a counter).
    let density: std::collections::HashMap<Ent, f64> = ivs.iter().map(|(s, t, e)| (*e, lv.refs[e].weight as f64 / (t - s + 1) as f64)).collect();
    // The entity's name breaks ties: formatted once per entity, not per
    // comparison (a large kernel has thousands).
    ivs.sort_by_cached_key(|(s, e, ent)| (*s, *e, format!("{:?}", ent)));
    let mut locs = std::collections::HashMap::new();
    let mut spill_bytes = 0u32;
    let new_slot = |spill: &mut u32, size: u32| {
        *spill = (*spill + size - 1) & !(size - 1);
        let at = *spill;
        *spill += size;
        Loc::Stack(at)
    };
    for (ci, pool) in pools.iter().enumerate() {
        let mut free: Vec<u8> = pool.iter().rev().copied().collect();
        // (end, ent, reg)
        let mut active: Vec<(u32, Ent, u8)> = Vec::new();
        for &(s, e, ent) in ivs.iter().filter(|iv| class(iv.2) == ci) {
            active.retain(|(end, _, reg)| {
                if *end < s || (reuse_at_def && *end == s) {
                    free.push(*reg);
                    false
                } else {
                    true
                }
            });
            let hinted = hint.get(&ent).and_then(|h| match locs.get(h) {
                Some(Loc::Reg(r)) => free.iter().position(|x| x == r),
                _ => None,
            });
            if let Some(k) = hinted {
                let reg = free.remove(k);
                active.push((e, ent, reg));
                locs.insert(ent, Loc::Reg(reg));
                continue;
            }
            if let Some(reg) = free.pop() {
                active.push((e, ent, reg));
                locs.insert(ent, Loc::Reg(reg));
                continue;
            }
            // Spill whichever of (the sparsest active, this) is used least
            // per position it covers (ties: the one ending last).
            let key = |end: u32, en: Ent| (density[&en], std::cmp::Reverse(end));
            let (k, &(far_end, far_ent, far_reg)) = active.iter().enumerate().min_by(|(_, a), (_, b)| key(a.0, a.1).partial_cmp(&key(b.0, b.1)).unwrap()).unwrap();
            if key(far_end, far_ent) < key(e, ent) {
                locs.insert(far_ent, new_slot(&mut spill_bytes, slot(far_ent)));
                active.remove(k);
                active.push((e, ent, far_reg));
                locs.insert(ent, Loc::Reg(far_reg));
            } else {
                locs.insert(ent, new_slot(&mut spill_bytes, slot(ent)));
            }
        }
    }
    Alloc { locs, spill_bytes }
}

// =========================================================================
// Encoding
// =========================================================================

pub(crate) mod enc {
    pub fn fp3(base: u32, d: u8, n: u8, m: u8) -> u32 {
        base | (m as u32) << 16 | (n as u32) << 5 | d as u32
    }
    pub fn fp2(base: u32, d: u8, n: u8) -> u32 {
        base | (n as u32) << 5 | d as u32
    }
    pub const FADD: u32 = 0x1E20_2800;
    pub const FSUB: u32 = 0x1E20_3800;
    pub const FMUL: u32 = 0x1E20_0800;
    pub const FDIV: u32 = 0x1E20_1800;
    pub const FABS: u32 = 0x1E20_C000;
    pub const FNEG: u32 = 0x1E21_4000;
    pub const FSQRT: u32 = 0x1E21_C000;
    pub const FRINTM: u32 = 0x1E25_4000;
    pub const FRINTP: u32 = 0x1E24_C000;
    pub const FRINTZ: u32 = 0x1E25_C000;
    pub const FRINTA: u32 = 0x1E26_4000;
    pub const FMOV: u32 = 0x1E20_4000;
    pub const FCVTZS: u32 = 0x1E38_0000;
    pub const SCVTF: u32 = 0x1E22_0000;
    pub const FMOV_WS: u32 = 0x1E26_0000;
    pub const FMOV_SW: u32 = 0x1E27_0000;
    pub fn fcmp(n: u8, m: u8) -> u32 {
        0x1E20_2000 | (m as u32) << 16 | (n as u32) << 5
    }
    pub fn fcsel(d: u8, n: u8, m: u8, cond: u8) -> u32 {
        0x1E20_0C00 | (m as u32) << 16 | (cond as u32) << 12 | (n as u32) << 5 | d as u32
    }
    pub const ADD: u32 = 0x0B00_0000;
    pub const SUB: u32 = 0x4B00_0000;
    pub const MUL: u32 = 0x1B00_7C00;
    pub const SDIV: u32 = 0x1AC0_0C00;
    pub const AND: u32 = 0x0A00_0000;
    pub const ORR: u32 = 0x2A00_0000;
    pub const EOR: u32 = 0x4A00_0000;
    pub const LSLV: u32 = 0x1AC0_2000;
    pub const LSRV: u32 = 0x1AC0_2400;
    pub const ASRV: u32 = 0x1AC0_2800;
    pub fn cmp(n: u8, m: u8) -> u32 {
        0x6B00_001F | (m as u32) << 16 | (n as u32) << 5
    }
    pub fn cmp_imm(n: u8, imm: u32) -> u32 {
        0x7100_001F | imm << 10 | (n as u32) << 5
    }
    pub fn csel(d: u8, n: u8, m: u8, cond: u8) -> u32 {
        0x1A80_0000 | (m as u32) << 16 | (cond as u32) << 12 | (n as u32) << 5 | d as u32
    }
    pub fn cset(d: u8, cond: u8) -> u32 {
        0x1A9F_07E0 | ((cond ^ 1) as u32) << 12 | d as u32
    }
    pub fn add_imm(d: u8, n: u8, imm: u32) -> u32 {
        0x1100_0000 | imm << 10 | (n as u32) << 5 | d as u32
    }
    pub fn movz(d: u8, imm: u32, hw: u32) -> u32 {
        0x5280_0000 | hw << 21 | (imm & 0xFFFF) << 5 | d as u32
    }
    pub fn movk(d: u8, imm: u32, hw: u32) -> u32 {
        0x7280_0000 | hw << 21 | (imm & 0xFFFF) << 5 | d as u32
    }
    pub fn mov(d: u8, m: u8) -> u32 {
        ORR | (m as u32) << 16 | 31 << 5 | d as u32
    }
    /// ldr/str S or W, [Xn, #imm] (imm a multiple of 4 below 16384).
    pub fn ldst_imm(fp: bool, load: bool, t: u8, n: u8, byte_off: u32) -> u32 {
        let base = match (fp, load) {
            (true, true) => 0xBD40_0000,
            (true, false) => 0xBD00_0000,
            (false, true) => 0xB940_0000,
            (false, false) => 0xB900_0000,
        };
        base | (byte_off / 4) << 10 | (n as u32) << 5 | t as u32
    }
    /// ldr/str S or W, [Xn, Wm, UXTW #2].
    pub fn ldst_reg(fp: bool, load: bool, t: u8, n: u8, m: u8) -> u32 {
        let base = match (fp, load) {
            (true, true) => 0xBC60_5800,
            (true, false) => 0xBC20_5800,
            (false, true) => 0xB860_5800,
            (false, false) => 0xB820_5800,
        };
        base | (m as u32) << 16 | (n as u32) << 5 | t as u32
    }
    /// ldr/str D, [Xn, #imm] (imm a multiple of 8 below 32768).
    pub fn ldst_d(load: bool, t: u8, n: u8, byte_off: u32) -> u32 {
        (if load { 0xFD40_0000 } else { 0xFD00_0000 }) | (byte_off / 8) << 10 | (n as u32) << 5 | t as u32
    }
    /// The double-precision form of a single-precision FP data-processing
    /// or compare instruction (type = 01).
    pub const DBL: u32 = 0x0040_0000;
    pub const FCVT_DS: u32 = 0x1E22_C000;
    pub const FCVT_SD: u32 = 0x1E62_4000;
    pub const FMOV_XD: u32 = 0x9E66_0000;
    pub const FMOV_DX: u32 = 0x9E67_0000;
    pub fn ldr_x(t: u8, n: u8, byte_off: u32) -> u32 {
        0xF940_0000 | (byte_off / 8) << 10 | (n as u32) << 5 | t as u32
    }
    pub const EQ: u8 = 0;
    pub const NE: u8 = 1;
    pub const HS: u8 = 2;
    pub const MI: u8 = 4;
    pub const LS: u8 = 9;
    pub const GE: u8 = 10;
    pub const LT: u8 = 11;
    pub const GT: u8 = 12;
    pub const LE: u8 = 13;
    pub const RET: u32 = 0xD65F_03C0;
}

use enc::*;

#[derive(Clone, Copy)]
enum Fix {
    B,
    BCond,
    Cbz,
}

struct Emit<'a> {
    p: &'a Program,
    code: Vec<u32>,
    locs: std::collections::HashMap<Ent, Loc>,
    /// Word index where the frame region starts (above the spill slots).
    frame_word0: u32,
    labels: Vec<Option<usize>>,
    fixups: Vec<(usize, usize, Fix)>,
    /// (continue label, break label) per enclosing loop.
    loop_labels: Vec<(usize, usize)>,
    loop_id: u32,
    /// Proven upper bound of each integer value (wraps, masks, and small
    /// constant scalings of them), when there is one: an offset proven
    /// below its extent needs no clamp.
    bounds: Vec<Option<u32>>,
    /// Byte offset from sp of the host-call area (register save area, then
    /// the call record), and the kernel's buffer writability mask.
    call_base: u32,
    writable: u64,
}

/// The host-call area: x0..x14 and d0..d7, d16..d28 saved (36 words of 8
/// bytes), then the call record (64 words).
const CALL_SAVE: u32 = 36 * 8;
const CALL_AREA: u32 = CALL_SAVE + 64 * 4;

/// Upper bounds of offsets (see [`crate::ir::bounds`]).
pub(crate) fn bounds(p: &Program) -> Vec<Option<u32>> {
    crate::ir::bounds(p)
}

impl<'a> Emit<'a> {
    fn e(&mut self, w: u32) {
        self.code.push(w);
    }

    fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }

    fn bind(&mut self, l: usize) {
        self.labels[l] = Some(self.code.len());
    }

    fn jump(&mut self, kind: Fix, word: u32, l: usize) {
        self.fixups.push((self.code.len(), l, kind));
        self.e(word);
    }

    fn mov_imm(&mut self, d: u8, v: u32) {
        self.e(movz(d, v & 0xFFFF, 0));
        if v >> 16 != 0 {
            self.e(movk(d, v >> 16, 1));
        }
    }

    fn loc(&self, e: Ent) -> Loc {
        self.locs[&e]
    }

    // -- operand access --------------------------------------------------

    /// The register holding GPR entity `e` (loading a spilled one into
    /// `scratch`).
    fn gsrc(&mut self, e: Ent, scratch: u8) -> u8 {
        match self.loc(e) {
            Loc::Reg(r) => r,
            Loc::Stack(off) => {
                self.e(ldst_imm(false, true, scratch, 31, off));
                scratch
            }
        }
    }

    fn fsrc(&mut self, e: Ent, scratch: u8) -> u8 {
        match self.loc(e) {
            Loc::Reg(r) => r,
            Loc::Stack(off) => {
                let w = if self.wide(e) { ldst_d(true, scratch, 31, off) } else { ldst_imm(true, true, scratch, 31, off) };
                self.e(w);
                scratch
            }
        }
    }

    /// An f64 entity (D register view, 8-byte spill slot).
    fn wide(&self, e: Ent) -> bool {
        match e {
            Ent::Val(v) => self.p.vals[v as usize] == Ty::F64,
            Ent::Var(v) => self.p.vars[v as usize] == Ty::F64,
            Ent::Counter(_) => false,
        }
    }

    /// Destination register for entity `e` (a scratch for spilled ones;
    /// call `gdone`/`fdone` after writing it).
    fn gdst(&self, e: Ent) -> u8 {
        match self.loc(e) {
            Loc::Reg(r) => r,
            Loc::Stack(_) => XS0,
        }
    }

    fn fdst(&self, e: Ent) -> u8 {
        match self.loc(e) {
            Loc::Reg(r) => r,
            Loc::Stack(_) => FS0,
        }
    }

    fn gdone(&mut self, e: Ent, r: u8) {
        if let Loc::Stack(off) = self.loc(e) {
            self.e(ldst_imm(false, false, r, 31, off));
        }
    }

    fn fdone(&mut self, e: Ent, r: u8) {
        if let Loc::Stack(off) = self.loc(e) {
            let w = if self.wide(e) { ldst_d(false, r, 31, off) } else { ldst_imm(true, false, r, 31, off) };
            self.e(w);
        }
    }

    fn is_fp(&self, v: Val) -> bool {
        matches!(self.p.vals[v.0 as usize], Ty::F32 | Ty::F64)
    }

    /// DBL for an f64 value (the double form of an FP instruction).
    fn dbl(&self, v: Val) -> u32 {
        if self.p.vals[v.0 as usize] == Ty::F64 {
            DBL
        } else {
            0
        }
    }

    // -- memory -----------------------------------------------------------

    /// Base register of a region.
    fn region_reg(r: Region) -> u8 {
        match r {
            Region::Ctx => 0,
            Region::State => 1,
            Region::Shared => 2,
            Region::Frame => 31,
            Region::Buf(_) => XS2,
        }
    }

    /// Emits the address computation; returns (base reg, Some(byte imm)) or
    /// (base reg, None) with the word index in w16.
    fn addr(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>) -> (u8, Option<u32>) {
        if let Region::Buf(k) = region {
            // Host buffer k: the table at x3 holds (pointer, length) pairs.
            // index = min(base + off, len - 1), in 64 bits (no wrap), like
            // the interpreter; the result is below 2^32, so the load uses
            // its low word zero-extended.
            match off {
                Some(o) => {
                    let ro = self.gsrc(Ent::Val(o.0), XS0);
                    // mov w16, w(ro): zero-extends into x16.
                    self.e(mov(XS0, ro));
                }
                None => self.e(mov(XS0, 31)),
            }
            if base > 0 {
                if base < 4096 {
                    self.e(0x9100_0000 | base << 10 | (XS0 as u32) << 5 | XS0 as u32);
                } else {
                    self.mov_imm(XS1, base);
                    self.e(0x8B00_0000 | (XS1 as u32) << 16 | (XS0 as u32) << 5 | XS0 as u32);
                }
            }
            self.e(ldst_imm(false, true, XS1, 3, k as u32 * 16 + 8));
            // sub x17, x17, #1; cmp x16, x17; csel x16, x16, x17, ls
            self.e(0xD100_0000 | 1 << 10 | (XS1 as u32) << 5 | XS1 as u32);
            self.e(0xEB00_001F | (XS1 as u32) << 16 | (XS0 as u32) << 5);
            self.e(0x9A80_0000 | (XS1 as u32) << 16 | (LS as u32) << 12 | (XS0 as u32) << 5 | XS0 as u32);
            self.e(ldr_x(XS2, 3, k as u32 * 16));
            return (XS2, None);
        }
        let rb = Self::region_reg(region);
        let base = if region == Region::Frame { base + self.frame_word0 } else { base };
        match off {
            None => {
                if base * 4 < 16384 {
                    (rb, Some(base * 4))
                } else {
                    self.mov_imm(XS0, base);
                    (rb, None)
                }
            }
            Some(o) if self.bounds[o.0 as usize].is_some_and(|b| b < extent) => {
                // Proven in range: no clamp.
                let ro = self.gsrc(Ent::Val(o.0), XS0);
                if base == 0 {
                    if ro != XS0 {
                        self.e(mov(XS0, ro));
                    }
                } else if base < 4096 {
                    self.e(add_imm(XS0, ro, base));
                } else {
                    self.mov_imm(XS1, base);
                    self.e(ADD | (XS1 as u32) << 16 | (ro as u32) << 5 | XS0 as u32);
                }
                (rb, None)
            }
            Some(o) => {
                let ro = self.gsrc(Ent::Val(o.0), XS0);
                self.mov_imm(XS1, extent - 1);
                self.e(cmp(ro, XS1));
                // Unsigned: a negative offset clamps to the top as well.
                self.e(csel(XS0, ro, XS1, LS));
                if base > 0 {
                    if base < 4096 {
                        self.e(add_imm(XS0, XS0, base));
                    } else {
                        self.mov_imm(XS1, base);
                        self.e(ADD | (XS1 as u32) << 16 | (XS0 as u32) << 5 | XS0 as u32);
                    }
                }
                (rb, None)
            }
        }
    }

    /// Clamps frame index `idx` into 0..n into w16.
    fn io_index(&mut self, idx: Val) {
        let ri = self.gsrc(Ent::Val(idx.0), XS0);
        // w17 = n - 1
        self.e(0x5100_0000 | 1 << 10 | 4 << 5 | XS1 as u32);
        self.e(cmp(ri, XS1));
        self.e(csel(XS0, ri, XS1, LS));
    }

    // -- statements ---------------------------------------------------------

    fn block(&mut self, b: &Block) {
        for s in b {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Def(v, op) => self.def(*v, op),
            Stmt::Set(var, v) => {
                let fp = self.is_fp(*v);
                let dst = Ent::Var(var.0);
                if fp {
                    let d = self.fdst(dst);
                    let r = self.fsrc(Ent::Val(v.0), d);
                    if r != d {
                        self.e(fp2(FMOV | self.dbl(*v), d, r));
                    }
                    self.fdone(dst, d);
                } else {
                    let d = self.gdst(dst);
                    let r = self.gsrc(Ent::Val(v.0), d);
                    if r != d {
                        self.e(mov(d, r));
                    }
                    self.gdone(dst, d);
                }
            }
            Stmt::Store { region, base, extent, off, val } => {
                let fp = self.is_fp(*val);
                // Load the value first: the address uses both scratch GPRs.
                let r = if fp { self.fsrc(Ent::Val(val.0), FS1) } else {
                    match self.loc(Ent::Val(val.0)) {
                        Loc::Reg(r) => r,
                        Loc::Stack(o) => {
                            // Park it in s31 as raw bits; stored as S.
                            self.e(ldst_imm(true, true, FS1, 31, o));
                            let (rb, imm) = self.addr(*region, *base, *extent, *off);
                            match imm {
                                Some(i) => self.e(ldst_imm(true, false, FS1, rb, i)),
                                None => self.e(ldst_reg(true, false, FS1, rb, XS0)),
                            }
                            return;
                        }
                    }
                };
                let (rb, imm) = self.addr(*region, *base, *extent, *off);
                match imm {
                    Some(i) => self.e(ldst_imm(fp, false, r, rb, i)),
                    None => self.e(ldst_reg(fp, false, r, rb, XS0)),
                }
            }
            Stmt::Out { ch, idx, val } => {
                let r = self.fsrc(Ent::Val(val.0), FS1);
                self.io_index(*idx);
                let rb = 7 + *ch;
                self.e(ldst_reg(true, true, FS0, rb, XS0));
                self.e(fp3(FADD, FS0, FS0, r));
                self.e(ldst_reg(true, false, FS0, rb, XS0));
            }
            Stmt::If(c, t, e) => {
                let rc = self.gsrc(Ent::Val(c.0), XS0);
                let else_l = self.label();
                let end_l = self.label();
                self.jump(Fix::Cbz, 0x3400_0000 | rc as u32, else_l);
                self.block(t);
                if !e.is_empty() {
                    self.jump(Fix::B, 0x1400_0000, end_l);
                }
                self.bind(else_l);
                self.block(e);
                self.bind(end_l);
            }
            Stmt::Loop { cap, body } => {
                let id = self.loop_id;
                self.loop_id += 1;
                let ce = Ent::Counter(id);
                let top = self.label();
                let exit = self.label();
                let d = self.gdst(ce);
                self.e(mov(d, 31));
                self.gdone(ce, d);
                self.bind(top);
                let rc = self.gsrc(ce, XS0);
                if *cap < 4096 {
                    self.e(cmp_imm(rc, *cap));
                } else {
                    self.mov_imm(XS1, *cap);
                    self.e(cmp(rc, XS1));
                }
                self.jump(Fix::BCond, 0x5400_0000 | HS as u32, exit);
                let d = self.gdst(ce);
                self.e(add_imm(d, rc, 1));
                self.gdone(ce, d);
                self.loop_labels.push((top, exit));
                self.block(body);
                self.loop_labels.pop();
                self.jump(Fix::B, 0x1400_0000, top);
                self.bind(exit);
            }
            Stmt::Break(d) => {
                let (_, exit) = self.loop_labels[self.loop_labels.len() - 1 - *d as usize];
                self.jump(Fix::B, 0x1400_0000, exit);
            }
            Stmt::CallHost { f, args, slices, rets } => self.call_host(*f, args, slices, rets),
            Stmt::Continue(d) => {
                let (top, _) = self.loop_labels[self.loop_labels.len() - 1 - *d as usize];
                self.jump(Fix::B, 0x1400_0000, top);
            }
        }
    }

    fn def(&mut self, v: Val, op: &Op) {
        let dst = Ent::Val(v.0);
        let ev = |x: Val| Ent::Val(x.0);
        match *op {
            Op::ConstF(x) => {
                let d = self.fdst(dst);
                let bits = x.to_bits();
                if bits == 0 {
                    self.e(FMOV_SW | 31 << 5 | d as u32);
                } else {
                    self.mov_imm(XS0, bits);
                    self.e(fp2(FMOV_SW, d, XS0));
                }
                self.fdone(dst, d);
            }
            Op::ConstD(x) => {
                let d = self.fdst(dst);
                let bits = x.to_bits();
                // x16 = bits (movz + movk per non-zero halfword), fmov d, x16.
                self.e(0xD280_0000 | ((bits & 0xFFFF) as u32) << 5 | XS0 as u32);
                for hw in 1..4u32 {
                    let h = ((bits >> (16 * hw)) & 0xFFFF) as u32;
                    if h != 0 {
                        self.e(0xF280_0000 | hw << 21 | h << 5 | XS0 as u32);
                    }
                }
                self.e(fp2(FMOV_DX, d, XS0));
                self.fdone(dst, d);
            }
            Op::ConstI(x) => {
                let d = self.gdst(dst);
                self.mov_imm(d, x as u32);
                self.gdone(dst, d);
            }
            Op::ConstB(x) => {
                let d = self.gdst(dst);
                self.mov_imm(d, x as u32);
                self.gdone(dst, d);
            }
            Op::Get(var) => {
                let src = Ent::Var(var.0);
                if self.is_fp(v) {
                    let d = self.fdst(dst);
                    let r = self.fsrc(src, d);
                    if r != d {
                        self.e(fp2(FMOV | self.dbl(v), d, r));
                    }
                    self.fdone(dst, d);
                } else {
                    let d = self.gdst(dst);
                    let r = self.gsrc(src, d);
                    if r != d {
                        self.e(mov(d, r));
                    }
                    self.gdone(dst, d);
                }
            }
            Op::Un(u, a) => match u {
                Un::F2I | Un::BitsFI => {
                    let ra = self.fsrc(ev(a), FS0);
                    let d = self.gdst(dst);
                    self.e(fp2(if u == Un::F2I { FCVTZS } else { FMOV_WS }, d, ra));
                    self.gdone(dst, d);
                }
                Un::I2F | Un::BitsIF => {
                    let ra = self.gsrc(ev(a), XS0);
                    let d = self.fdst(dst);
                    self.e(fp2(if u == Un::I2F { SCVTF } else { FMOV_SW }, d, ra));
                    self.fdone(dst, d);
                }
                Un::NegI => {
                    let ra = self.gsrc(ev(a), XS0);
                    let d = self.gdst(dst);
                    self.e(SUB | (ra as u32) << 16 | 31 << 5 | d as u32);
                    self.gdone(dst, d);
                }
                Un::NotB => {
                    // (a == 0): any non-zero word is true, like the
                    // interpreter (a bool loaded from memory may hold any
                    // bits).
                    let ra = self.gsrc(ev(a), XS0);
                    self.e(cmp_imm(ra, 0));
                    let d = self.gdst(dst);
                    self.e(cset(d, EQ));
                    self.gdone(dst, d);
                }
                Un::D2I | Un::LoD | Un::HiD => {
                    let ra = self.fsrc(ev(a), FS0);
                    let d = self.gdst(dst);
                    match u {
                        Un::D2I => self.e(fp2(FCVTZS | DBL, d, ra)),
                        // The S view of the D register is its low word.
                        Un::LoD => self.e(fp2(FMOV_WS, d, ra)),
                        _ => {
                            // fmov x16, d; lsr xd, x16, #32
                            self.e(fp2(FMOV_XD, XS0, ra));
                            self.e(0xD360_FC00 | (XS0 as u32) << 5 | d as u32);
                        }
                    }
                    self.gdone(dst, d);
                }
                Un::I2D => {
                    let ra = self.gsrc(ev(a), XS0);
                    let d = self.fdst(dst);
                    self.e(fp2(SCVTF | DBL, d, ra));
                    self.fdone(dst, d);
                }
                Un::F2D | Un::D2F => {
                    let ra = self.fsrc(ev(a), FS0);
                    let d = self.fdst(dst);
                    self.e(fp2(if u == Un::F2D { FCVT_DS } else { FCVT_SD }, d, ra));
                    self.fdone(dst, d);
                }
                _ => {
                    let base = match u {
                        Un::NegF => FNEG,
                        Un::AbsF => FABS,
                        Un::SqrtF => FSQRT,
                        Un::FloorF => FRINTM,
                        Un::CeilF => FRINTP,
                        Un::TruncF => FRINTZ,
                        Un::RoundF => FRINTA,
                        Un::NegD => FNEG | DBL,
                        Un::AbsD => FABS | DBL,
                        Un::SqrtD => FSQRT | DBL,
                        Un::FloorD => FRINTM | DBL,
                        Un::CeilD => FRINTP | DBL,
                        Un::TruncD => FRINTZ | DBL,
                        Un::RoundD => FRINTA | DBL,
                        _ => unreachable!(),
                    };
                    let ra = self.fsrc(ev(a), FS0);
                    let d = self.fdst(dst);
                    self.e(fp2(base, d, ra));
                    self.fdone(dst, d);
                }
            },
            Op::Bin(b, x, y) => match b {
                Bin::AddF | Bin::SubF | Bin::MulF | Bin::DivF | Bin::AddD | Bin::SubD | Bin::MulD | Bin::DivD => {
                    let base = match b {
                        Bin::AddF => FADD,
                        Bin::SubF => FSUB,
                        Bin::MulF => FMUL,
                        Bin::DivF => FDIV,
                        Bin::AddD => FADD | DBL,
                        Bin::SubD => FSUB | DBL,
                        Bin::MulD => FMUL | DBL,
                        _ => FDIV | DBL,
                    };
                    let rx = self.fsrc(ev(x), FS0);
                    let ry = self.fsrc(ev(y), FS1);
                    let d = self.fdst(dst);
                    self.e(fp3(base, d, rx, ry));
                    self.fdone(dst, d);
                }
                Bin::MinF | Bin::MaxF | Bin::MinD | Bin::MaxD => {
                    let dbl = self.dbl(x);
                    let rx = self.fsrc(ev(x), FS0);
                    let ry = self.fsrc(ev(y), FS1);
                    self.e(fcmp(rx, ry) | dbl);
                    let d = self.fdst(dst);
                    self.e(fcsel(d, rx, ry, if matches!(b, Bin::MinF | Bin::MinD) { MI } else { GT }) | dbl);
                    self.fdone(dst, d);
                }
                Bin::MakeD => {
                    // x16 = zero-extended lo | hi << 32; fmov d, x16.
                    let rhi = self.gsrc(ev(x), XS1);
                    let rlo = self.gsrc(ev(y), XS0);
                    self.e(mov(XS0, rlo));
                    self.e(0xAA00_8000 | (rhi as u32) << 16 | (XS0 as u32) << 5 | XS0 as u32);
                    let d = self.fdst(dst);
                    self.e(fp2(FMOV_DX, d, XS0));
                    self.fdone(dst, d);
                }
                Bin::RemI => {
                    // x - (x / y) * y, with x parked in s29 so both
                    // scratches stay usable.
                    let rx = self.gsrc(ev(x), XS0);
                    let ry = self.gsrc(ev(y), XS1);
                    self.e(fp2(FMOV_SW, FS2, rx));
                    self.e(SDIV | (ry as u32) << 16 | (rx as u32) << 5 | XS0 as u32);
                    self.e(MUL | (ry as u32) << 16 | (XS0 as u32) << 5 | XS0 as u32);
                    self.e(fp2(FMOV_WS, XS1, FS2));
                    let d = self.gdst(dst);
                    self.e(SUB | (XS0 as u32) << 16 | (XS1 as u32) << 5 | d as u32);
                    self.gdone(dst, d);
                }
                _ => {
                    let base = match b {
                        Bin::AddI => ADD,
                        Bin::SubI => SUB,
                        Bin::MulI => MUL,
                        Bin::DivI => SDIV,
                        Bin::AndI | Bin::AndB => AND,
                        Bin::OrI | Bin::OrB => ORR,
                        Bin::XorI => EOR,
                        Bin::ShlI => LSLV,
                        Bin::ShrI => ASRV,
                        Bin::ShrUI => LSRV,
                        _ => unreachable!(),
                    };
                    let rx = self.gsrc(ev(x), XS0);
                    let ry = self.gsrc(ev(y), XS1);
                    let d = self.gdst(dst);
                    self.e(base | (ry as u32) << 16 | (rx as u32) << 5 | d as u32);
                    self.gdone(dst, d);
                }
            },
            Op::CmpF(cc, x, y) | Op::CmpD(cc, x, y) => {
                let dbl = self.dbl(x);
                let rx = self.fsrc(ev(x), FS0);
                let ry = self.fsrc(ev(y), FS1);
                self.e(fcmp(rx, ry) | dbl);
                let cond = match cc {
                    Cmp::Lt => MI,
                    Cmp::Le => LS,
                    Cmp::Gt => GT,
                    Cmp::Ge => GE,
                    Cmp::Eq => EQ,
                    Cmp::Ne => NE,
                };
                let d = self.gdst(dst);
                self.e(cset(d, cond));
                self.gdone(dst, d);
            }
            Op::CmpI(cc, x, y) => {
                let rx = self.gsrc(ev(x), XS0);
                let ry = self.gsrc(ev(y), XS1);
                self.e(cmp(rx, ry));
                let cond = match cc {
                    Cmp::Lt => LT,
                    Cmp::Le => LE,
                    Cmp::Gt => GT,
                    Cmp::Ge => GE,
                    Cmp::Eq => EQ,
                    Cmp::Ne => NE,
                };
                let d = self.gdst(dst);
                self.e(cset(d, cond));
                self.gdone(dst, d);
            }
            Op::Sel(c, x, y) => {
                let rc = self.gsrc(ev(c), XS0);
                self.e(cmp_imm(rc, 0));
                if self.is_fp(x) {
                    let dbl = self.dbl(x);
                    let rx = self.fsrc(ev(x), FS0);
                    let ry = self.fsrc(ev(y), FS1);
                    let d = self.fdst(dst);
                    self.e(fcsel(d, rx, ry, NE) | dbl);
                    self.fdone(dst, d);
                } else {
                    // The flags survive the operand reloads (plain loads).
                    let rx = self.gsrc(ev(x), XS0);
                    let ry = self.gsrc(ev(y), XS1);
                    let d = self.gdst(dst);
                    self.e(csel(d, rx, ry, NE));
                    self.gdone(dst, d);
                }
            }
            Op::Wrap(x, len) => {
                let rx = self.gsrc(ev(x), XS0);
                let d = self.gdst(dst);
                if len.is_power_of_two() {
                    self.mov_imm(XS1, len - 1);
                    self.e(AND | (XS1 as u32) << 16 | (rx as u32) << 5 | d as u32);
                } else if len < 1 << 31 {
                    // q = x / len by the magic multiplier (as SDIV rounds),
                    // r = x - q * len, then r < 0 ? r + len : r.
                    let rx = if rx == XS0 || rx == XS1 {
                        self.e(mov(XS2, rx));
                        XS2
                    } else {
                        rx
                    };
                    self.magic_div(rx, len);
                    self.mov_imm(XS1, len);
                    self.e(0x1B00_8000 | (XS1 as u32) << 16 | (rx as u32) << 10 | (XS0 as u32) << 5 | XS0 as u32);
                    self.e(ADD | (XS0 as u32) << 16 | (XS1 as u32) << 5 | XS1 as u32);
                    self.e(cmp_imm(XS0, 0));
                    self.e(csel(d, XS1, XS0, LT));
                } else {
                    // r = x - (x / len) * len (x parked in s29).
                    self.e(fp2(FMOV_SW, FS2, rx));
                    self.mov_imm(XS1, len);
                    self.e(SDIV | (XS1 as u32) << 16 | (rx as u32) << 5 | XS0 as u32);
                    self.e(MUL | (XS1 as u32) << 16 | (XS0 as u32) << 5 | XS0 as u32);
                    self.e(fp2(FMOV_WS, XS1, FS2));
                    self.e(SUB | (XS0 as u32) << 16 | (XS1 as u32) << 5 | XS0 as u32);
                    self.mov_imm(XS1, len);
                    // r in x16, len in x17: r < 0 ? r + len : r
                    self.e(ADD | (XS0 as u32) << 16 | (XS1 as u32) << 5 | XS1 as u32);
                    self.e(cmp_imm(XS0, 0));
                    self.e(csel(d, XS1, XS0, LT));
                }
                self.gdone(dst, d);
            }
            Op::Load { region, base, extent, off } => {
                let fp = self.p.vals[v.0 as usize] == Ty::F32;
                let (rb, imm) = self.addr(region, base, extent, off);
                if fp {
                    let d = self.fdst(dst);
                    match imm {
                        Some(i) => self.e(ldst_imm(true, true, d, rb, i)),
                        None => self.e(ldst_reg(true, true, d, rb, XS0)),
                    }
                    self.fdone(dst, d);
                } else {
                    // A spilled GPR destination loads into x17 (x16 may
                    // hold the index).
                    let d = match self.loc(dst) {
                        Loc::Reg(r) => r,
                        Loc::Stack(_) => XS1,
                    };
                    match imm {
                        Some(i) => self.e(ldst_imm(false, true, d, rb, i)),
                        None => self.e(ldst_reg(false, true, d, rb, XS0)),
                    }
                    self.gdone(dst, d);
                }
            }
            Op::In { ch, idx } => {
                self.io_index(idx);
                let d = self.fdst(dst);
                self.e(ldst_reg(true, true, d, 5 + ch, XS0));
                self.fdone(dst, d);
            }
            Op::FrameCount => {
                let d = self.gdst(dst);
                self.e(mov(d, 4));
                self.gdone(dst, d);
            }
            Op::Fma(k, a, b, c) => {
                // fmadd d = c + a*b; fmsub d = c - a*b; fnmsub d = a*b - c.
                let ra = self.fsrc(ev(a), FS0);
                let rb = self.fsrc(ev(b), FS1);
                let rc = self.fsrc(ev(c), FS2);
                let d = self.fdst(dst);
                let base = match k {
                    crate::ir::Fma::Add => 0x1F00_0000,
                    crate::ir::Fma::SubFrom => 0x1F00_8000,
                    crate::ir::Fma::Sub => 0x1F20_8000,
                };
                self.e(base | (rb as u32) << 16 | (rc as u32) << 10 | (ra as u32) << 5 | d as u32);
                self.fdone(dst, d);
            }
            Op::BufLen(k) => {
                let d = self.gdst(dst);
                self.e(ldst_imm(false, true, d, 3, k as u32 * 16 + 8));
                self.gdone(dst, d);
            }
        }
    }

    /// x(d) = a 64-bit immediate.
    /// w16 = x / d (truncating, as SDIV) for a constant d in 2..2^31, by
    /// the magic multiplier; `rx` is not x16 or x17 (clobbers x17).
    fn magic_div(&mut self, rx: u8, d: u32) {
        let (m, sh) = crate::ir::magic_s32(d);
        self.mov_imm(XS1, m as u32);
        // smull x16, w(rx), w17; asr x16, x16, #32
        self.e(0x9B20_7C00 | (XS1 as u32) << 16 | (rx as u32) << 5 | XS0 as u32);
        self.e(0x9360_FC00 | (XS0 as u32) << 5 | XS0 as u32);
        if m < 0 {
            self.e(ADD | (rx as u32) << 16 | (XS0 as u32) << 5 | XS0 as u32);
        }
        if sh > 0 {
            self.e(0x1300_7C00 | sh << 16 | (XS0 as u32) << 5 | XS0 as u32);
        }
        // add w16, w16, w(rx), lsr #31
        self.e(0x0B40_7C00 | (rx as u32) << 16 | (XS0 as u32) << 5 | XS0 as u32);
    }

    fn mov_imm64(&mut self, d: u8, v: u64) {
        self.e(0xD280_0000 | ((v & 0xFFFF) as u32) << 5 | d as u32);
        for hw in 1..4u32 {
            let h = ((v >> (16 * hw)) & 0xFFFF) as u32;
            if h != 0 {
                self.e(0xF280_0000 | hw << 21 | h << 5 | d as u32);
            }
        }
    }

    /// A host call: the record on the stack, caller-saved registers saved,
    /// `host::trampoline(record, table, ctx, writable)`, registers restored,
    /// the result loaded. The trampoline's address is a constant of the
    /// code (never formed from data); it checks the call and never unwinds.
    fn call_host(&mut self, f: u16, args: &[Val], slices: &[crate::ir::SliceArg], rets: &[Val]) {
        let rec = self.call_base + CALL_SAVE;
        // x15 = the record's address (sp + rec).
        self.sp_plus(XS2, rec);
        let word = |em: &mut Self, k: usize, r: u8, fp: bool| em.e(ldst_imm(fp, false, r, XS2, 4 * k as u32));
        for (k, hdr) in [f as u32, args.len() as u32, slices.len() as u32, rets.len() as u32].into_iter().enumerate() {
            self.mov_imm(XS0, hdr);
            word(self, k, XS0, false);
        }
        let mut at = crate::host::REC_HEADER;
        for a in args {
            if self.is_fp(*a) {
                let r = self.fsrc(Ent::Val(a.0), FS0);
                word(self, at, r, true);
            } else {
                let r = self.gsrc(Ent::Val(a.0), XS0);
                word(self, at, r, false);
            }
            at += 1;
        }
        for x in slices {
            self.mov_imm(XS0, x.buf as u32);
            word(self, at, XS0, false);
            let r = self.gsrc(Ent::Val(x.off.0), XS0);
            word(self, at + 1, r, false);
            let r = self.gsrc(Ent::Val(x.len.0), XS0);
            word(self, at + 2, r, false);
            at += 3;
        }
        let ret_at = at;
        // Save x0..x14 and d0..d7, d16..d28 (x15 still points at the record).
        self.sp_plus(XS1, self.call_base);
        let gpr: Vec<u8> = (0..15).collect();
        let fpr: Vec<u8> = (0..8).chain(16..29).collect();
        for (k, r) in gpr.iter().enumerate() {
            self.e(0xF900_0000 | (k as u32) << 10 | (XS1 as u32) << 5 | *r as u32);
        }
        for (k, r) in fpr.iter().enumerate() {
            self.e(ldst_d(false, *r, XS1, 8 * (15 + k as u32)));
        }
        // x1 = table, x2 = ctx, x3 = writable mask, x0 = record.
        self.e(0xAA00_03E0 | 3 << 16 | 1); // mov x1, x3
        self.e(0xAA00_03E0 | 2); // mov x2, x0
        self.mov_imm64(3, self.writable);
        self.e(0xAA00_03E0 | (XS2 as u32) << 16); // mov x0, x15
        self.mov_imm64(XS0, crate::host::trampoline as *const () as usize as u64);
        self.e(0xD63F_0000 | (XS0 as u32) << 5); // blr x16
        // Restore (x17 is caller-saved: recompute the save area's address).
        self.sp_plus(XS1, self.call_base);
        for (k, r) in fpr.iter().enumerate() {
            self.e(ldst_d(true, *r, XS1, 8 * (15 + k as u32)));
        }
        for (k, r) in gpr.iter().enumerate() {
            self.e(0xF940_0000 | (k as u32) << 10 | (XS1 as u32) << 5 | *r as u32);
        }
        for v in rets {
            self.sp_plus(XS2, rec);
            let e = Ent::Val(v.0);
            if self.is_fp(*v) {
                let d = self.fdst(e);
                self.e(ldst_imm(true, true, d, XS2, 4 * ret_at as u32));
                self.fdone(e, d);
            } else {
                let d = match self.loc(e) {
                    Loc::Reg(r) => r,
                    Loc::Stack(_) => XS1,
                };
                self.e(ldst_imm(false, true, d, XS2, 4 * ret_at as u32));
                self.gdone(e, d);
            }
        }
    }

    /// x(d) = sp + byte offset.
    fn sp_plus(&mut self, d: u8, off: u32) {
        if off < 4096 {
            self.e(0x9100_0000 | off << 10 | 31 << 5 | d as u32);
        } else {
            self.mov_imm(d, off);
            // add xd, sp, xd (extended register, UXTX)
            self.e(0x8B20_63E0 | (d as u32) << 16 | d as u32);
        }
    }

    fn patch(&mut self) -> bool {
        for &(at, l, kind) in &self.fixups {
            let Some(target) = self.labels[l] else { return false };
            let delta = target as i64 - at as i64;
            let w = &mut self.code[at];
            match kind {
                Fix::B => {
                    if delta.abs() >= 1 << 25 {
                        return false;
                    }
                    *w |= (delta as u32) & 0x03FF_FFFF;
                }
                Fix::BCond | Fix::Cbz => {
                    if delta.abs() >= 1 << 18 {
                        return false;
                    }
                    *w |= ((delta as u32) & 0x7FFFF) << 5;
                }
            }
        }
        true
    }
}

/// Compiles a program to native code (None if it cannot: too far branches,
/// no executable memory).
pub fn compile(p: &Program) -> Option<Code> {
    compile_with(p, 0)
}

fn has_host_call(b: &Block) -> bool {
    b.iter().any(|s| match s {
        Stmt::CallHost { .. } => true,
        Stmt::If(_, t, e) => has_host_call(t) || has_host_call(e),
        Stmt::Loop { body, .. } => has_host_call(body),
        _ => false,
    })
}

/// Compiles a kernel whose host buffers have write permission `writable`
/// (bit k: buffer k), which host calls pass on to the trampoline.
pub fn compile_with(p: &Program, writable: u64) -> Option<Code> {
    let alloc = allocate(p);
    if alloc.spill_bytes >= 16384 {
        return None;
    }
    let frame_bytes = p.frame_words * 4;
    let calls = has_host_call(&p.body);
    let call_base = (alloc.spill_bytes + frame_bytes + 15) & !15;
    let total = call_base + if calls { CALL_AREA } else { 0 };
    let total = (total + 15) & !15;
    let mut em = Emit {
        p,
        code: Vec::new(),
        locs: alloc.locs,
        frame_word0: alloc.spill_bytes / 4,
        labels: Vec::new(),
        fixups: Vec::new(),
        loop_labels: Vec::new(),
        loop_id: 0,
        bounds: bounds(p),
        call_base,
        writable,
    };
    // Prologue: frame record, callee-saved x19..x28 and d8..d15.
    em.e(0xA9BF_7BFD); // stp x29, x30, [sp, #-16]!
    em.e(0x9100_03FD); // mov x29, sp
    let pre = |t1: u8, t2: u8, fp: bool| -> u32 {
        (if fp { 0x6D80_0000 } else { 0xA980_0000 }) | 0x7E << 15 | (t2 as u32) << 10 | 31 << 5 | t1 as u32
    };
    let post = |t1: u8, t2: u8, fp: bool| -> u32 {
        (if fp { 0x6CC0_0000 } else { 0xA8C0_0000 }) | 2 << 15 | (t2 as u32) << 10 | 31 << 5 | t1 as u32
    };
    for (a, b) in [(19, 20), (21, 22), (23, 24), (25, 26), (27, 28)] {
        em.e(pre(a, b, false));
    }
    for (a, b) in [(8, 9), (10, 11), (12, 13), (14, 15)] {
        em.e(pre(a, b, true));
    }
    let sub_sp = |em: &mut Emit, add: bool| {
        if total == 0 {
            return;
        }
        if total < 4096 {
            em.e((if add { 0x9100_03FF } else { 0xD100_03FF }) | total << 10);
        } else {
            em.mov_imm(XS0, total);
            // add/sub sp, sp, x16 (extended register, UXTX)
            em.e((if add { 0x8B20_63FF } else { 0xCB20_63FF }) | (XS0 as u32) << 16);
        }
    };
    if total > 1 << 20 {
        return None;
    }
    sub_sp(&mut em, false);
    for (k, r) in [5u8, 6, 7, 8].iter().enumerate() {
        em.e(ldr_x(*r, 3, k as u32 * 8));
    }
    em.block(&p.body);
    sub_sp(&mut em, true);
    for (a, b) in [(14, 15), (12, 13), (10, 11), (8, 9)] {
        em.e(post(a, b, true));
    }
    for (a, b) in [(27, 28), (25, 26), (23, 24), (21, 22), (19, 20)] {
        em.e(post(a, b, false));
    }
    em.e(0xA8C1_7BFD); // ldp x29, x30, [sp], #16
    em.e(RET);
    if !em.patch() {
        return None;
    }
    Code::new(&em.code)
}
