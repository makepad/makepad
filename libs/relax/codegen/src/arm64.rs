//! AArch64 backend (O1): instruction selection and encoding over the shared
//! register allocator (regalloc.rs), plus the runtime glue code the live image
//! needs (lazy-compile entry, stubs, thunks, enter/leave, panic trampolines).
//!
//! ABI (Relax's own = AAPCS64 for host calls): int args x0-x7, float args v0-v7,
//! int returns x0 x1, float returns v0 v1. Callee-saved x19-x28 and the low 64 bits
//! of v8-v15. x18 is the platform register on Darwin and never touched. Frame
//! pointers always on: every function stores its frame record (x29, x30) at x29.
//! Scratch, never allocated: x14 x15 (operands), x16 (call targets, immediates),
//! x17 (address offsets), v29 v30 v31.
//!
//! Frame (fp = x29 after the prologue):
//!   [fp + 8]  return address, [fp] caller's fp
//!   [fp - 8 * k]  callee-saved registers (ints then floats)
//!   below: spill area, stack slots, C ABI buffers/staging cells, outgoing stack arguments;
//!   sp = fp - frame (16-aligned)
//! The body addresses the frame through sp with positive offsets.
//!
//! Encodings follow the hand-written encoders of platform/script/compute/src/arm64.rs,
//! extended to 64-bit integer forms.

use crate::cabi::{self, ArgPlan, PLoc, Plan, RetPlan};
use crate::regalloc::{self, Alloc, Loc, MLoc, RegConfig, Remat};
use crate::rir::*;

pub const THUNK_SIZE: usize = 32;
pub const STUB_SIZE: usize = 32;

const SP: u8 = 31;
const ZR: u8 = 31;
const FP: u8 = 29;
const LR: u8 = 30;
const S0: u8 = 14; // operand scratch
const S1: u8 = 15; // operand scratch
const IP0: u8 = 16; // call targets, immediates
const IP1: u8 = 17; // address offsets
const FS0: u8 = 30; // float scratch
const FS1: u8 = 31;
/// third float scratch (fused multiply-add's addend)
const FS2: u8 = 29;
/// x8: indirect result register of the C ABI; otherwise a scratch for atomics loops
const X8: u8 = 8;

const INT_ARGS: usize = 8;
const FLT_ARGS: usize = 8;

/// arm64 isel switches for measurements: `MAKEPAD_RUN_A64=none` or `-fuse,-fconst`.
const A64_FUSE: u8 = 1;
const A64_FCONST: u8 = 2;

fn isel_flags(options: &relax_session::Options) -> u8 {
    let mut f = A64_FUSE | A64_FCONST;
    for part in &options.a64 {
        match part.as_str() {
            "none" => f = 0,
            "-fuse" => f &= !A64_FUSE,
            "-fconst" => f &= !A64_FCONST,
            _ => {}
        }
    }
    f
}

pub fn reg_config() -> RegConfig {
    RegConfig {
        int_caller: vec![9, 10, 11, 12, 13, 2, 3, 4, 5, 6, 7],
        int_callee: vec![19, 20, 21, 22, 23, 24, 25, 26, 27, 28],
        flt_caller: vec![16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28],
        flt_callee: vec![8, 9, 10, 11, 12, 13, 14, 15],
        slot_index_free: false,
        clobbers: None,
        sym_class: 0,
    }
}

/// x64 records RIR offsets for asm dumps; arm64 keeps the switch for one embedder API
pub fn set_rir_map(_on: bool) {}

pub struct Compiled {
    pub code: Vec<u8>,
    /// (code offset, source position) in ascending offset order
    pub pc_map: Vec<(u32, u64)>,
    pub frame_size: u32,
    /// bytes of instructions (the float literal pool follows)
    pub text_len: u32,
    /// regalloc::alloc_sig of the allocation (0 unless regalloc::SIG_ON)
    pub alloc_sig: u64,
    /// the relocated fields, emitted zeroed: the embedder links the code (`rir::link`) where it
    /// installs it
    pub relocs: Vec<RelocAt>,
    /// x64 only (tools/run/tools/asmcmp); empty here
    pub rir_map: Vec<(u32, u32, u32)>,
    pub rir_emitted: String,
}

/// Addresses the backend needs from the runtime.
pub struct Env {
    /// where the function slots' tables and thunks are
    pub slots: SlotsPtr,
    /// pthread TSD slot holding this thread's thread-local block (0 = none yet)
    pub tls_key: u64,
    /// glue that allocates the calling thread's block: returns it in x16, preserves all else
    pub tls_slow: u64,
    /// `Term::Resume`'s call target (the runtime's resume trampoline; never returns)
    pub resume: u64,
    /// thorough register allocation (live-range splitting at loops, copy coalescing): for code
    /// that runs hot (O3, compute kernels); first compiles take the quicker allocation
    pub opt_regs: bool,
    /// the embedder's symbol value table (u64 per `Sym::id`)
    pub syms: u64,
    /// how each symbol may be encoded (`SymClass` as u8 per `Sym::id`; 0 = no table: all Far)
    pub sym_class: u64,
}

impl Env {
    /// The current value of a relocated word.
    pub fn reloc_value(&self, r: Reloc) -> u64 {
        match r {
            Reloc::Sym(s) => unsafe { s.value(self.syms) },
            Reloc::Slot(id) => self.slots.table(id),
            Reloc::Thunk(id) => self.slots.callable(id),
            // the TSD slot's byte offset (ldr x16, [x16, x17])
            Reloc::TlsKey => self.tls_key * 8,
            Reloc::TlsSlow => self.tls_slow,
            Reloc::Resume => self.resume,
            Reloc::Value(v) => v,
        }
    }
}

// ------------------------------------------------------------ encoder

// condition codes
const EQ: u8 = 0;
const NE: u8 = 1;
const HS: u8 = 2;
const LO: u8 = 3;
const MI: u8 = 4;
const HI: u8 = 8;
const LS: u8 = 9;
const GE: u8 = 10;
const LT: u8 = 11;
const GT: u8 = 12;
const LE: u8 = 13;

// load/store, unsigned scaled offset forms
const LDRB: u32 = 0x3940_0000;
const LDRSB: u32 = 0x3980_0000;
const STRB: u32 = 0x3900_0000;
const LDRH: u32 = 0x7940_0000;
const LDRSH: u32 = 0x7980_0000;
const STRH: u32 = 0x7900_0000;
const LDRW: u32 = 0xB940_0000;
const LDRSW: u32 = 0xB980_0000;
const STRW: u32 = 0xB900_0000;
const LDRX: u32 = 0xF940_0000;
const STRX: u32 = 0xF900_0000;
const LDRS: u32 = 0xBD40_0000;
const STRS: u32 = 0xBD00_0000;
const LDRD: u32 = 0xFD40_0000;
const STRD: u32 = 0xFD00_0000;

// float data processing (single; | DBL for double)
const DBL: u32 = 0x0040_0000;
const FADD: u32 = 0x1E20_2800;
const FSUB: u32 = 0x1E20_3800;
const FMUL: u32 = 0x1E20_0800;
const FDIV: u32 = 0x1E20_1800;
// added by the compiler lane for FOp::Min/Max (IEEE minNum/maxNum = Rust min/max)
const FMAXNM: u32 = 0x1E20_6800;
const FMINNM: u32 = 0x1E20_7800;
const FABS: u32 = 0x1E20_C000;
const FNEG: u32 = 0x1E21_4000;
const FSQRT: u32 = 0x1E21_C000;
const FRINTN: u32 = 0x1E24_4000;
const FRINTP: u32 = 0x1E24_C000;
const FRINTM: u32 = 0x1E25_4000;
const FRINTZ: u32 = 0x1E25_C000;
const FRINTA: u32 = 0x1E26_4000;
const FMOVR: u32 = 0x1E20_4000;

struct Asm {
    w: Vec<u32>,
}

impl Asm {
    fn e(&mut self, x: u32) {
        self.w.push(x);
    }
    fn pos(&self) -> usize {
        self.w.len()
    }

    // ---- integer moves and constants
    fn mov(&mut self, d: u8, m: u8) {
        if d != m {
            // orr xd, xzr, xm
            self.e(0xAA00_03E0 | (m as u32) << 16 | d as u32);
        }
    }
    /// mov to/from sp (add xd, xn, #0)
    fn mov_sp(&mut self, d: u8, n: u8) {
        self.e(0x9100_0000 | (n as u32) << 5 | d as u32);
    }
    fn mov_imm(&mut self, d: u8, v: i64) {
        let u = v as u64;
        let mut zeros = 0;
        let mut ones = 0;
        for k in 0..4 {
            let h = (u >> (16 * k)) & 0xffff;
            if h == 0 {
                zeros += 1;
            }
            if h == 0xffff {
                ones += 1;
            }
        }
        if ones > zeros {
            // movn then movk the halves that are not 0xffff
            let mut first = true;
            for k in 0..4u32 {
                let h = ((u >> (16 * k)) & 0xffff) as u32;
                if h == 0xffff {
                    continue;
                }
                if first {
                    self.e(0x9280_0000 | k << 21 | ((!h) & 0xffff) << 5 | d as u32);
                    first = false;
                } else {
                    self.e(0xF280_0000 | k << 21 | h << 5 | d as u32);
                }
            }
            if first {
                // all ones
                self.e(0x9280_0000 | d as u32);
            }
        } else {
            let mut first = true;
            for k in 0..4u32 {
                let h = ((u >> (16 * k)) & 0xffff) as u32;
                if h == 0 {
                    continue;
                }
                if first {
                    self.e(0xD280_0000 | k << 21 | h << 5 | d as u32);
                    first = false;
                } else {
                    self.e(0xF280_0000 | k << 21 | h << 5 | d as u32);
                }
            }
            if first {
                self.e(0xD280_0000 | d as u32);
            }
        }
    }
    /// Always 4 instructions (patchable absolute address).
    fn mov_imm64_fixed(&mut self, d: u8, v: u64) {
        self.e(0xD280_0000 | ((v & 0xffff) as u32) << 5 | d as u32);
        for k in 1..4u32 {
            self.e(0xF280_0000 | k << 21 | (((v >> (16 * k)) & 0xffff) as u32) << 5 | d as u32);
        }
    }

    // ---- integer arithmetic (64-bit)
    fn rrr(&mut self, op: u32, d: u8, n: u8, m: u8) {
        self.e(op | (m as u32) << 16 | (n as u32) << 5 | d as u32);
    }
    fn add_imm(&mut self, d: u8, n: u8, imm: u32) {
        self.e(0x9100_0000 | imm << 10 | (n as u32) << 5 | d as u32);
    }
    fn sub_imm(&mut self, d: u8, n: u8, imm: u32) {
        self.e(0xD100_0000 | imm << 10 | (n as u32) << 5 | d as u32);
    }
    fn add_imm_lsl12(&mut self, d: u8, n: u8, imm: u32) {
        self.e(0x9140_0000 | imm << 10 | (n as u32) << 5 | d as u32);
    }
    fn sub_imm_lsl12(&mut self, d: u8, n: u8, imm: u32) {
        self.e(0xD140_0000 | imm << 10 | (n as u32) << 5 | d as u32);
    }
    /// d = n + v for any v (n and d may be sp); uses IP1 when v does not fit.
    fn add_any(&mut self, d: u8, n: u8, v: i64) {
        let (neg, a) = if v < 0 { (true, (-v) as u64) } else { (false, v as u64) };
        if a < (1 << 24) {
            let lo = (a & 0xfff) as u32;
            let hi = (a >> 12) as u32;
            let mut src = n;
            if hi != 0 {
                if neg {
                    self.sub_imm_lsl12(d, src, hi);
                } else {
                    self.add_imm_lsl12(d, src, hi);
                }
                src = d;
            }
            if lo != 0 || src != d {
                if neg {
                    self.sub_imm(d, src, lo);
                } else {
                    self.add_imm(d, src, lo);
                }
            }
        } else {
            self.mov_imm(IP1, v);
            // add xd, xn, x17, uxtx (extended form accepts sp)
            self.e(0x8B20_6000 | (IP1 as u32) << 16 | (n as u32) << 5 | d as u32);
        }
    }
    fn cmp(&mut self, n: u8, m: u8) {
        self.e(0xEB00_001F | (m as u32) << 16 | (n as u32) << 5);
    }
    fn cset(&mut self, d: u8, cond: u8) {
        self.e(0x9A9F_07E0 | ((cond ^ 1) as u32) << 12 | d as u32);
    }
    fn csel(&mut self, d: u8, n: u8, m: u8, cond: u8) {
        self.e(0x9A80_0000 | (m as u32) << 16 | (cond as u32) << 12 | (n as u32) << 5 | d as u32);
    }
    fn sbfm(&mut self, d: u8, n: u8, immr: u32, imms: u32) {
        self.e(0x9340_0000 | immr << 16 | imms << 10 | (n as u32) << 5 | d as u32);
    }
    fn ubfm(&mut self, d: u8, n: u8, immr: u32, imms: u32) {
        self.e(0xD340_0000 | immr << 16 | imms << 10 | (n as u32) << 5 | d as u32);
    }
    /// sign/zero extension of the low `bits` of n into d
    fn extend(&mut self, d: u8, n: u8, bits: u8, signed: bool) {
        if bits >= 64 {
            self.mov(d, n);
            return;
        }
        let imms = bits as u32 - 1;
        if signed {
            self.sbfm(d, n, 0, imms);
        } else {
            self.ubfm(d, n, 0, imms);
        }
    }

    // ---- memory
    /// Load/store with any offset. `op` is the unsigned-offset form, `size` the access size.
    fn ldst(&mut self, op: u32, size: u32, t: u8, n: u8, off: i64) {
        if off >= 0 && off % size as i64 == 0 && off / (size as i64) < 4096 {
            self.e(op | ((off / size as i64) as u32) << 10 | (n as u32) << 5 | t as u32);
        } else if (-256..256).contains(&off) {
            self.e((op & !0x0100_0000) | ((off as u32) & 0x1ff) << 12 | (n as u32) << 5 | t as u32);
        } else {
            self.mov_imm(IP1, off);
            self.e((op & !0x0100_0000) | 0x0020_6800 | (IP1 as u32) << 16 | (n as u32) << 5 | t as u32);
        }
    }
    fn stp_pre(&mut self, t1: u8, t2: u8, n: u8, off: i32) {
        self.e(0xA980_0000 | (((off / 8) as u32) & 0x7f) << 15 | (t2 as u32) << 10 | (n as u32) << 5 | t1 as u32);
    }
    fn ldp_post(&mut self, t1: u8, t2: u8, n: u8, off: i32) {
        self.e(0xA8C0_0000 | (((off / 8) as u32) & 0x7f) << 15 | (t2 as u32) << 10 | (n as u32) << 5 | t1 as u32);
    }
    fn stp(&mut self, fp: bool, t1: u8, t2: u8, n: u8, off: i32) {
        let op = if fp { 0x6D00_0000 } else { 0xA900_0000 };
        self.e(op | (((off / 8) as u32) & 0x7f) << 15 | (t2 as u32) << 10 | (n as u32) << 5 | t1 as u32);
    }
    fn ldp(&mut self, fp: bool, t1: u8, t2: u8, n: u8, off: i32) {
        let op = if fp { 0x6D40_0000 } else { 0xA940_0000 };
        self.e(op | (((off / 8) as u32) & 0x7f) << 15 | (t2 as u32) << 10 | (n as u32) << 5 | t1 as u32);
    }

    // ---- control flow
    fn blr(&mut self, n: u8) {
        self.e(0xD63F_0000 | (n as u32) << 5);
    }
    fn br(&mut self, n: u8) {
        self.e(0xD61F_0000 | (n as u32) << 5);
    }
    fn ret(&mut self) {
        self.e(0xD65F_03C0);
    }
    fn udf(&mut self) {
        self.e(0x0000_0000);
    }
    /// b (patched later); returns the word index
    fn b(&mut self) -> usize {
        self.e(0x1400_0000);
        self.pos() - 1
    }
    fn bcond(&mut self, cond: u8) -> usize {
        self.e(0x5400_0000 | cond as u32);
        self.pos() - 1
    }
    fn cbnz(&mut self, t: u8) -> usize {
        self.e(0xB500_0000 | t as u32);
        self.pos() - 1
    }
    fn cbz(&mut self, t: u8) -> usize {
        self.e(0xB400_0000 | t as u32);
        self.pos() - 1
    }
    fn patch(&mut self, at: usize, target: usize) {
        let rel = target as i64 - at as i64;
        let w = self.w[at];
        // b: imm26 (+-128 MB); b.cond / cbz / cbnz / ldr literal: imm19 (+-1 MB). An offset out
        // of range is never truncated: it flags the emission, which compile() repeats with
        // relaxed branches (or fails past imm26). Only these encodings are patched here (tbz /
        // tbnz have imm14 and adr imm21: none is emitted).
        let is_b = w & 0xFC00_0000 == 0x1400_0000;
        let is_imm19 = w & 0xFF00_0010 == 0x5400_0000 || w & 0x7E00_0000 == 0x3400_0000 || w & 0x3B00_0000 == 0x1800_0000;
        assert!(is_b || is_imm19, "arm64 patch of an unknown encoding {:08x}", w);
        let lim = if w & 0xFC00_0000 == 0x1400_0000 { 1i64 << 25 } else { 1i64 << 18 };
        if rel < -lim || rel >= lim {
            BRANCH_RANGE.with(|c| c.set(true));
        }
        if w & 0xFC00_0000 == 0x1400_0000 {
            self.w[at] = 0x1400_0000 | (rel as u32 & 0x03FF_FFFF);
        } else {
            // b.cond / cbz / cbnz / ldr literal: imm19 at bit 5
            self.w[at] = (w & !(0x7FFFF << 5)) | ((rel as u32) & 0x7FFFF) << 5;
        }
    }

    // ---- float
    fn fp2(&mut self, op: u32, dbl: bool, d: u8, n: u8) {
        self.e(op | if dbl { DBL } else { 0 } | (n as u32) << 5 | d as u32);
    }
    fn fp3(&mut self, op: u32, dbl: bool, d: u8, n: u8, m: u8) {
        self.e(op | if dbl { DBL } else { 0 } | (m as u32) << 16 | (n as u32) << 5 | d as u32);
    }
    fn fmov(&mut self, d: u8, n: u8) {
        if d != n {
            // full 64-bit move (holds either precision)
            self.fp2(FMOVR, true, d, n);
        }
    }
    fn fcmp(&mut self, dbl: bool, n: u8, m: u8) {
        self.e(0x1E20_2000 | if dbl { DBL } else { 0 } | (m as u32) << 16 | (n as u32) << 5);
    }
    /// fmov dd, xn / sd, wn
    fn fmov_from_gp(&mut self, dbl: bool, d: u8, n: u8) {
        self.e(if dbl { 0x9E67_0000 } else { 0x1E27_0000 } | (n as u32) << 5 | d as u32);
    }
    /// fmov xd, dn / wd, sn
    fn fmov_to_gp(&mut self, dbl: bool, d: u8, n: u8) {
        self.e(if dbl { 0x9E66_0000 } else { 0x1E26_0000 } | (n as u32) << 5 | d as u32);
    }
}

fn icc(c: Cond, signed: bool) -> u8 {
    match (c, signed) {
        (Cond::Eq, _) => EQ,
        (Cond::Ne, _) => NE,
        (Cond::Lt, true) => LT,
        (Cond::Le, true) => LE,
        (Cond::Gt, true) => GT,
        (Cond::Ge, true) => GE,
        (Cond::Lt, false) => LO,
        (Cond::Le, false) => LS,
        (Cond::Gt, false) => HI,
        (Cond::Ge, false) => HS,
    }
}

/// Float compare conditions after `fcmp a, b`; all false on unordered except Ne.
fn fcc(c: Cond) -> u8 {
    match c {
        Cond::Eq => EQ,
        Cond::Ne => NE,
        Cond::Lt => MI,
        Cond::Le => LS,
        Cond::Gt => GT,
        Cond::Ge => GE,
    }
}

/// The 8-bit FMOV immediate for a float, when it has one: +-(16..31)/16 * 2^(-3..4).
fn fp_imm8(bits: u64, f64_: bool) -> Option<u32> {
    let (sign, exp, frac, ebias, fbits) = if f64_ {
        ((bits >> 63) as u32, ((bits >> 52) & 0x7ff) as i32, bits & ((1u64 << 52) - 1), 1023, 52)
    } else {
        (((bits >> 31) & 1) as u32, ((bits >> 23) & 0xff) as i32, bits & ((1u64 << 23) - 1), 127, 23)
    };
    // only the top 4 fraction bits may be set
    if frac & ((1u64 << (fbits - 4)) - 1) != 0 {
        return None;
    }
    let e = exp - ebias;
    if !(-3..=4).contains(&e) {
        return None;
    }
    // imm8 = a:b:cd:efgh with exponent = NOT(b):cc..c:de (3 bits used: b, cd)
    let e3 = ((e + 3) as u32) ^ 0b100; // b = NOT(top bit) encoding of e in -3..4
    let top4 = (frac >> (fbits - 4)) as u32;
    Some(sign << 7 | e3 << 4 | top4)
}

/// One-instruction encodings of a 4 x 32-bit splat (without Rd): movi / mvni with a
/// shifted byte, or fmov with a float immediate.
fn vimm(bits: u32) -> Option<u32> {
    let enc = |op: u32, v: u32| -> Option<u32> {
        (0..4u32).find(|k| v & !(0xFF << (8 * k)) == 0).map(|k| {
            let imm8 = (v >> (8 * k)) & 0xFF;
            op | (imm8 >> 5) << 16 | (k * 2) << 12 | (imm8 & 31) << 5
        })
    };
    if let Some(w) = enc(0x4F00_0400, bits) {
        return Some(w);
    }
    if let Some(w) = enc(0x6F00_0400, !bits) {
        return Some(w);
    }
    fp_imm8(bits as u64, false).map(|imm8| 0x4F00_F400 | (imm8 >> 5) << 16 | (imm8 & 31) << 5)
}

/// Does an immediate-operand instruction encode without materialising its constant (see ibin_imm,
/// cmp_imm)?
fn imm_fits(i: &Inst) -> bool {
    match *i {
        Inst::ICmpI(_, _, _, _, v) => (-4095..4096).contains(&v),
        Inst::IBinI(op, it, _, _, v) => match op {
            IOp::Add | IOp::Sub => v.unsigned_abs() < (1 << 24),
            IOp::Shl | IOp::Shr => true,
            IOp::Mul => v > 0 && (v & (v - 1)) == 0,
            IOp::And | IOp::Or | IOp::Xor => {
                (op == IOp::And && (v == 0xff || v == 0xffff || v == 0xffff_ffff)) || ((it.bits == 64 || (it == U32 && (0..1i64 << 32).contains(&v))) && bitmask_imm(v as u64).is_some())
            }
            _ => false,
        },
        _ => true,
    }
}

/// The (N, immr, imms) fields of a logical-instruction immediate for a 64-bit value: a repeating
/// element (2..64 bits) that is a rotated run of ones. None for 0, all ones and other values.
fn bitmask_imm(v: u64) -> Option<(u32, u32, u32)> {
    if v == 0 || v == !0 {
        return None;
    }
    let mut e = 64u32;
    // smallest period
    while e > 2 {
        let h = e / 2;
        let m = (1u64 << h) - 1;
        if (v & m) != ((v >> h) & m) {
            break;
        }
        e = h;
    }
    let mask = if e == 64 { !0u64 } else { (1u64 << e) - 1 };
    let x = v & mask;
    let c = x.count_ones();
    if c == 0 || c == e {
        return None;
    }
    let ones = (1u64 << c) - 1;
    let ror = |y: u64, r: u32| -> u64 {
        if r == 0 {
            y
        } else {
            ((y >> r) | (y << (e - r))) & mask
        }
    };
    for immr in 0..e {
        if ror(ones, immr) == x {
            let n = if e == 64 { 1 } else { 0 };
            let imms = ((((-(e as i32)) << 1) & 0x3f) as u32) | (c - 1);
            return Some((n, immr, imms));
        }
    }
    None
}

fn mem_op(m: Mem, load: bool) -> (u32, u32) {
    match (m, load) {
        (Mem::Int(1, s), true) => (if s { LDRSB } else { LDRB }, 1),
        (Mem::Int(2, s), true) => (if s { LDRSH } else { LDRH }, 2),
        (Mem::Int(4, s), true) => (if s { LDRSW } else { LDRW }, 4),
        (Mem::Int(_, _), true) => (LDRX, 8),
        (Mem::Int(1, _), false) => (STRB, 1),
        (Mem::Int(2, _), false) => (STRH, 2),
        (Mem::Int(4, _), false) => (STRW, 4),
        (Mem::Int(_, _), false) => (STRX, 8),
        (Mem::F32, true) => (LDRS, 4),
        (Mem::F32, false) => (STRS, 4),
        (Mem::F64, true) => (LDRD, 8),
        (Mem::F64, false) => (STRD, 8),
    }
}

// ------------------------------------------------------------ code generation

#[derive(Clone, Copy)]
enum CondBr {
    B(u8),
    Z(u8),
    Nz(u8),
}

struct Gen<'a> {
    /// the block ends `t = x & K; t ==/!= 0; branch`: emitted as tst x, #K (the And skipped)
    test_fuse: Option<(VReg, i64)>,
    /// the last spill store emitted: (sp offset, register, word position right after it)
    last_spill: Option<(i64, u8, usize)>,
    /// spill windows (Alloc::windows): vreg -> (first use pos, last use pos, register); and the
    /// loads to emit before the instruction at a position
    win: std::collections::HashMap<u32, Vec<(u32, u32, u8)>>,
    win_start: std::collections::HashMap<u32, Vec<(u32, u8)>>,
    /// jump tables: head block -> (x, min, target per value min..=max, default)
    jt_plan: std::collections::HashMap<u32, (VReg, i64, Vec<u32>, u32)>,
    /// table words to fill once blocks are placed: (word index, table start, target block)
    jt_fix: Vec<(usize, usize, u32)>,
    a: Asm,
    f: &'a Func,
    al: Alloc,
    /// total frame size below fp (sp = fp - frame)
    frame: i64,
    /// bytes of callee-saved registers below fp
    saved: i64,
    slot_off: Vec<i64>,
    block_pos: Vec<usize>,
    next_block: Vec<u32>,
    fixups: Vec<(usize, u32)>,
    /// float literal pool: (ldr word index, bits)
    fpool: Vec<(usize, u64)>,
    /// vector literal pool: (ldr q word index, lane bits splatted to 4 lanes)
    vpool: Vec<(usize, u32)>,
    /// address pool: (ldr x word index, relocated value)
    apool: Vec<(usize, Reloc)>,
    /// `Env::sym_class`: how each symbol is encoded (SymClass::Domain: an offset in the domain block)
    sym_class: u64,
    /// sp offset of the frame word holding the domain base (-1: the function names no domain symbol)
    dom_slot: i64,
    relocs: Vec<RelocAt>,
    text_words: usize,
    pc_map: Vec<(u32, u64)>,
    cur_pos: u32,
    /// C ABI body: its plan, per-argument buffer (sp offset, -1 = none), result buffer
    cplan: Option<Plan>,
    cbuf: Vec<i64>,
    cret_buf: i64,
    /// sp offset of the C call staging cells (8 bytes each)
    stage: i64,
    /// word index right after the last call's branch-and-link (its return address)
    last_call_ret: usize,
    /// invoked calls: (return word index, caller-saved values to reload, cleanup pad block)
    pads: Vec<(usize, Vec<(VReg, i32)>, u32)>,
    /// branch relaxation (a function past imm19's +-1 MB): conditional block branches become
    /// an inverted short branch over a `b` (+-128 MB), constants are built in registers
    /// instead of loaded from the literal pools after the code
    long: bool,
}

impl<'a> Gen<'a> {
    /// sp offset of spill slot `o` (Loc::Stack(o))
    fn spill_sp(&self, o: i32) -> i64 {
        self.frame - (self.saved + o as i64)
    }

    fn cls(&self, v: VReg) -> Cls {
        self.f.vregs[v.0 as usize]
    }

    fn use_i(&mut self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => r,
            Loc::Stack(o) => {
                // held in a register over this run of uses (Alloc::windows)
                if let Some(ws) = self.win.get(&v.0) {
                    let p = self.cur_pos;
                    if let Some(&(_, _, r)) = ws.iter().find(|(a, z, _)| *a <= p && p <= *z) {
                        return r;
                    }
                }
                let off = self.spill_sp(o);
                // stored from a register by the very last instruction emitted (same block)
                if let Some((d, r, p)) = self.last_spill {
                    if d == off && p == self.a.pos() {
                        if r != scratch {
                            self.a.mov(scratch, r);
                        }
                        return scratch;
                    }
                }
                self.a.ldst(LDRX, 8, scratch, SP, off);
                scratch
            }
            // recomputed at each use (Alloc::remat)
            _ => match self.al.remat[v.0 as usize] {
                Remat::Const(k) => {
                    // never xzr for 0: register 31 is sp in add-immediate and address operands
                    self.a.mov_imm(scratch, k);
                    scratch
                }
                Remat::Slot(s) => {
                    let off = self.slot_off[s as usize];
                    self.a.add_any(scratch, SP, off);
                    scratch
                }
                Remat::Addr(a) => {
                    self.ldr_reloc(scratch, Reloc::Sym(a));
                    scratch
                }
                Remat::FnAddr(id) => {
                    self.ldr_reloc(scratch, Reloc::Thunk(id));
                    scratch
                }
                Remat::No => ZR,
            },
        }
    }
    /// A stack-slot address without a register (Alloc::remat) as a load/store base: sp + its offset.
    fn slot_base(&self, base: VReg) -> Option<i64> {
        match (self.al.loc[base.0 as usize], self.al.remat[base.0 as usize]) {
            (Loc::None, Remat::Slot(s)) => Some(self.slot_off[s as usize]),
            _ => None,
        }
    }
    /// [base + (idx << sh) + off] for an indexed access: the register-offset form when the shift
    /// is 0 or the access size and nothing is added, else ip1 = the address and the offset.
    fn index_addr(&mut self, base: VReg, ri: u8, sh: u8, off: i32, size: u32) -> (u8, Option<(u8, bool)>, i64) {
        let scaled = sh as u32 == size.trailing_zeros();
        match self.slot_base(base) {
            Some(so) => {
                let off = so + off as i64;
                if off == 0 && (sh == 0 || scaled) {
                    return (SP, Some((ri, sh != 0)), 0);
                }
                // ADD (shifted register) reads register 31 as xzr: sp goes through add_any first
                self.a.add_any(IP1, SP, off);
                if sh == 0 || scaled {
                    return (IP1, Some((ri, sh != 0)), 0);
                }
                self.a.e(0x8B00_0000 | ((sh as u32) << 10) | ((ri as u32) << 16) | ((IP1 as u32) << 5) | IP1 as u32);
                (IP1, None, 0)
            }
            None => {
                let rb = self.use_i(base, S1);
                if off == 0 && (sh == 0 || scaled) {
                    return (rb, Some((ri, sh != 0)), 0);
                }
                self.a.e(0x8B00_0000 | ((sh as u32) << 10) | ((ri as u32) << 16) | ((rb as u32) << 5) | IP1 as u32);
                // ldst's large-offset form needs ip1 itself: fold big offsets in first
                if (off as i64).abs() >= 256 {
                    self.a.add_any(IP1, IP1, off as i64);
                    return (IP1, None, 0);
                }
                (IP1, None, off as i64)
            }
        }
    }
    /// One load/store at [rn + (rm << (scaled ? size : 0))] (register-offset form) or [rn + off].
    fn ldst_x(&mut self, op: u32, size: u32, t: u8, rn: u8, rm: Option<(u8, bool)>, off: i64) {
        match rm {
            // LDR/STR (register), option LSL (011), S = scaled
            Some((rm, scaled)) => self.a.e((op & !0x0100_0000) | 0x0020_6800 | if scaled { 0x1000 } else { 0 } | (rm as u32) << 16 | (rn as u32) << 5 | t as u32),
            None => self.a.ldst(op, size, t, rn, off),
        }
    }
    fn def_i(&self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => r,
            _ => scratch,
        }
    }
    fn fin_i(&mut self, v: VReg, r: u8) {
        if let Loc::Stack(o) = self.al.loc[v.0 as usize] {
            let off = self.spill_sp(o);
            self.a.ldst(STRX, 8, r, SP, off);
            self.last_spill = Some((off, r, self.a.pos()));
        }
    }
    fn use_f(&mut self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Flt(x) => x,
            Loc::Stack(o) if self.cls(v) == Cls::V128 => {
                let off = self.spill_sp(o);
                self.vldst(true, scratch, SP, off);
                scratch
            }
            Loc::Stack(o) => {
                let off = self.spill_sp(o);
                self.a.ldst(LDRD, 8, scratch, SP, off);
                scratch
            }
            _ => scratch,
        }
    }
    fn def_f(&self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Flt(x) => x,
            _ => scratch,
        }
    }
    fn fin_f(&mut self, v: VReg, x: u8) {
        if let Loc::Stack(o) = self.al.loc[v.0 as usize] {
            let off = self.spill_sp(o);
            if self.cls(v) == Cls::V128 {
                self.vldst(false, x, SP, off);
                return;
            }
            self.a.ldst(STRD, 8, x, SP, off);
        }
    }

    fn mloc(&self, v: VReg) -> MLoc {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => MLoc::R(r),
            Loc::Flt(x) => MLoc::F(x),
            Loc::Stack(o) => MLoc::M(self.spill_sp(o) as i32),
            Loc::None => MLoc::R(ZR),
        }
    }

    /// Emits a sequentialized parallel move. Memory locations are sp offsets of
    /// 8-byte cells; floats are moved as full 64-bit registers.
    fn moves(&mut self, mv: &[(MLoc, MLoc)]) {
        let seq = regalloc::seq_moves(mv, S0, FS0);
        for (d, s) in seq {
            match (d, s) {
                (MLoc::R(d), MLoc::R(s)) => self.a.mov(d, s),
                (MLoc::F(d), MLoc::F(s)) => self.a.fmov(d, s),
                (MLoc::R(d), MLoc::M(o)) => self.a.ldst(LDRX, 8, d, SP, o as i64),
                (MLoc::F(d), MLoc::M(o)) => self.a.ldst(LDRD, 8, d, SP, o as i64),
                (MLoc::M(o), MLoc::R(s)) => self.a.ldst(STRX, 8, s, SP, o as i64),
                (MLoc::M(o), MLoc::F(s)) => self.a.ldst(STRD, 8, s, SP, o as i64),
                (MLoc::M(o), MLoc::M(p)) => {
                    self.a.ldst(LDRX, 8, S1, SP, p as i64);
                    self.a.ldst(STRX, 8, S1, SP, o as i64);
                }
                (MLoc::R(d), MLoc::F(s)) => self.a.fmov_to_gp(true, d, s),
                (MLoc::F(d), MLoc::R(s)) => self.a.fmov_from_gp(true, d, s),
            }
        }
    }

    fn gen(&mut self, options: &relax_session::Options) {
        // frame layout
        let n_saved = self.al.used_callee_int.len() + self.al.used_callee_flt.len();
        self.saved = 8 * n_saved as i64;
        // only slots something addresses take space (SROA and inlining leave many unreferenced:
        // the Splash VM's O3 opcode() had 1749 slots, 515 in use); slots whose lifetimes cannot
        // overlap share it (opt::slot_layout)
        let (slot_fp, mut off) = crate::opt::slot_layout(self.f, self.saved + self.al.spill_bytes as i64, !crate::opt::opt_off(options, "-slotcolor"));
        // C ABI body: buffers that rebuild aggregate params from their registers, and the
        // buffer an aggregate result is written to before Ret loads it into registers
        let mut cbuf_fp: Vec<i64> = Vec::new();
        let mut cret_fp = 0i64;
        if let Some(sig) = &self.f.cabi {
            let plan = cabi::plan(sig, sig.conv.target());
            for (k, a) in plan.args.iter().enumerate() {
                match (a, &sig.args[k]) {
                    (ArgPlan::Parts(_), CArg::Agg(ag)) => {
                        off += ((ag.size as i64 + 7) & !7) + 8;
                        cbuf_fp.push(off);
                    }
                    _ => cbuf_fp.push(-1),
                }
            }
            if let (RetPlan::Parts(_), Some(ag)) = (&plan.ret, &sig.ret) {
                off += ((ag.size as i64 + 7) & !7) + 8;
                cret_fp = off;
            }
            self.cplan = Some(plan);
        }
        // outgoing stack-argument area at the bottom of the frame ([sp, sp + out)) and
        // staging cells for C calls (aggregate pieces, the result address)
        let mut out = 0i64;
        let mut cells = 0i64;
        for b in &self.f.blocks {
            for i in &b.insts {
                if let Inst::Call(c, a, _) = i {
                    match c {
                        Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig) => {
                            let plan = cabi::plan(sig, sig.conv.target());
                            out = out.max(plan.stack_bytes as i64);
                            let mut n = 0i64;
                            for ap in &plan.args {
                                if let ArgPlan::Parts(ps) = ap {
                                    n += ps.len() as i64;
                                }
                            }
                            if let RetPlan::Parts(ps) = &plan.ret {
                                n = n.max(ps.len() as i64);
                            }
                            cells = cells.max(1 + n);
                        }
                        _ => {
                            let (ni, nf) = count_classes(self.f, a);
                            let over = ni.saturating_sub(INT_ARGS) + nf.saturating_sub(FLT_ARGS);
                            out = out.max(8 * over as i64);
                        }
                    }
                }
            }
        }
        off += 8 * cells;
        let stage_fp = off;
        // the domain base, read once in the prologue when the function names a domain static
        let dom_fp = if uses_domain(self.f, self.sym_class) {
            off += 8;
            off
        } else {
            -1
        };
        let frame = (off + out + 15) / 16 * 16;
        self.frame = frame;
        if dom_fp >= 0 {
            self.dom_slot = frame - dom_fp;
        }
        for x in slot_fp {
            self.slot_off.push(frame - x);
        }
        for x in cbuf_fp {
            self.cbuf.push(if x < 0 { -1 } else { frame - x });
        }
        self.cret_buf = frame - cret_fp;
        self.stage = frame - stage_fp;

        // prologue
        self.a.stp_pre(FP, LR, SP, -16);
        self.a.mov_sp(FP, SP);
        self.stack_probe(frame);
        self.a.add_any(SP, SP, -frame);
        self.callee_saves(true);
        if self.dom_slot >= 0 {
            self.dom_base_prologue();
        }
        if self.cplan.is_some() {
            self.c_prologue();
        } else {
            self.params_prologue();
        }

        let nb = self.f.blocks.len();
        self.block_pos = vec![0; nb];
        let reach = regalloc::reachable(self.f);
        self.next_block = vec![u32::MAX; nb];
        let mut prev: Option<usize> = None;
        for bi in 0..nb {
            if reach[bi] {
                if let Some(p) = prev {
                    self.next_block[p] = bi as u32;
                }
                prev = Some(bi);
            }
        }
        for bi in 0..nb {
            self.block_pos[bi] = self.a.pos();
            if !reach[bi] {
                continue;
            }
            let b = &self.f.blocks[bi];
            let n = b.insts.len();
            // a compare feeding only the branch is fused into cmp + b.cond
            let mut fused = false;
            if let Term::Branch(c, _, _) = &b.term {
                if n > 0 && self.al.uses[c.0 as usize] == 1 && isel_flags(options) & A64_FUSE != 0 {
                    match &b.insts[n - 1] {
                        Inst::ICmp(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) | Inst::FCmp(_, _, d, _, _) if d == c => fused = true,
                        _ => {}
                    }
                }
            }
            // `t = x & K; c = t ==/!= 0` feeding the branch (t used once, K a bitmask immediate): tst x, #K
            self.test_fuse = None;
            if fused && n >= 2 {
                if let (Inst::IBinI(IOp::And, it, t, x, k), Inst::ICmpI(Cond::Eq | Cond::Ne, _, _, t2, 0)) = (&b.insts[n - 2], &b.insts[n - 1]) {
                    if t == t2 && *x != *t && self.al.uses[t.0 as usize] == 1 && (it.bits == 64 || (0..1i64 << 32).contains(k)) && bitmask_imm(*k as u64).is_some() {
                        self.test_fuse = Some((*x, *k));
                    }
                }
            }
            let mut skip_next = false;
            // a block start can be a jump target: no spill store before it carries over
            self.last_spill = None;
            for (ii, inst) in b.insts.iter().enumerate() {
                self.pc_map.push(((self.a.pos() * 4) as u32, b.pos[ii]));
                self.cur_pos = self.al.inst_pos(bi, ii);
                if let Some(ls) = self.win_start.get(&self.cur_pos).cloned() {
                    for (v, r) in ls {
                        if let Loc::Stack(o) = self.al.loc[v as usize] {
                            let off = self.spill_sp(o);
                            self.a.ldst(LDRX, 8, r, SP, off);
                        }
                    }
                }
                if fused && ii == n - 1 {
                    break;
                }
                if self.test_fuse.is_some() && ii == n - 2 {
                    continue;
                }
                if skip_next {
                    skip_next = false;
                    continue;
                }
                // a shift by a constant feeding only the next add/sub/and/or/xor: the shifted
                // register operand form (`eor d, a, b, lsl #13`)
                if ii + 1 < n && !(fused && ii + 1 == n - 1) {
                    if let Some(()) = self.shifted_operand(options, inst, &b.insts[ii + 1]) {
                        skip_next = true;
                        continue;
                    }
                }
                self.inst(options, inst);
            }
            self.pc_map.push(((self.a.pos() * 4) as u32, b.term_pos));
            let fuse_inst = if fused { Some(b.insts[n - 1].clone()) } else { None };
            self.term(&b.term, bi, fuse_inst);
        }
        if crate::rir::unwind_stats_on() {
            // bytes of blocks only reachable through unwind edges (cleanup pads)
            let nb = self.f.blocks.len();
            let mut normal = vec![false; nb];
            let mut st = vec![0u32];
            while let Some(b) = st.pop() {
                if normal[b as usize] {
                    continue;
                }
                normal[b as usize] = true;
                match self.f.blocks[b as usize].term {
                    Term::Invoke(n, _) => st.push(n),
                    ref t => st.extend(t.succs()),
                }
            }
            let end_pos = self.a.pos();
            let mut order: Vec<(usize, usize)> = (0..nb).filter(|b| reach[*b]).map(|b| (self.block_pos[b], b)).collect();
            order.sort();
            let mut pad = 0usize;
            for k in 0..order.len() {
                let next = if k + 1 < order.len() { order[k + 1].0 } else { end_pos };
                if !normal[order[k].1] {
                    pad += (next - order[k].0) * 4;
                }
            }
            crate::rir::UNWIND_STATS[0].fetch_add(pad as u64, std::sync::atomic::Ordering::Relaxed);
        }
        let stub0 = self.a.pos();
        // landing stubs of invoked calls: the unwinder resumes here with fp, sp and the
        // callee-saved registers of this frame restored; reload the caller-saved values the
        // call's normal path reloads, then the cleanup pad
        let mut unwind_tab: Vec<(u32, u32)> = Vec::new();
        for (ret, saves, pad) in std::mem::take(&mut self.pads) {
            // only the values the pad reads come back (live into the pad block); none: the
            // unwinder lands on the pad itself
            let at = self.al.block_start[pad as usize];
            let reload: Vec<(VReg, i32)> = saves.into_iter().filter(|(v, _)| self.al.start[v.0 as usize] <= at && self.al.end[v.0 as usize] >= at).collect();
            if reload.is_empty() {
                unwind_tab.push(((ret * 4) as u32, (self.block_pos[pad as usize] * 4) as u32));
                continue;
            }
            let stub = self.a.pos();
            for (v, slot) in &reload {
                self.save_restore(*v, *slot, false);
            }
            let p = self.a.b();
            self.fixups.push((p, pad));
            unwind_tab.push(((ret * 4) as u32, (stub * 4) as u32));
        }
        if crate::rir::unwind_stats_on() {
            crate::rir::UNWIND_STATS[1].fetch_add(((self.a.pos() - stub0) * 4) as u64, std::sync::atomic::Ordering::Relaxed);
            crate::rir::UNWIND_STATS[3].fetch_add(unwind_tab.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        for (at, t) in std::mem::take(&mut self.fixups) {
            let target = self.block_pos[t as usize];
            self.a.patch(at, target);
        }
        for (at, start, t) in std::mem::take(&mut self.jt_fix) {
            let rel = (self.block_pos[t as usize] as i64 - start as i64) * 4;
            self.a.w[at] = rel as i32 as u32;
        }
        self.text_words = self.a.pos();
        // float literal pool (8-byte aligned entries, deduplicated)
        if !self.fpool.is_empty() {
            if self.a.pos() % 2 != 0 {
                self.a.udf();
            }
            let mut placed: Vec<(u64, usize)> = Vec::new();
            for (at, bits) in std::mem::take(&mut self.fpool) {
                let mut addr = usize::MAX;
                for (b2, a2) in &placed {
                    if *b2 == bits {
                        addr = *a2;
                    }
                }
                if addr == usize::MAX {
                    addr = self.a.pos();
                    self.a.e(bits as u32);
                    self.a.e((bits >> 32) as u32);
                    placed.push((bits, addr));
                }
                self.a.patch(at, addr);
            }
        }
        // address pool (8-byte words, one per distinct relocated value; Compiled::relocs)
        if !self.apool.is_empty() {
            if self.a.pos() % 2 != 0 {
                self.a.udf();
            }
            let mut placed: Vec<(Reloc, usize)> = Vec::new();
            for (at, r) in std::mem::take(&mut self.apool) {
                let addr = match placed.iter().find(|(r2, _)| *r2 == r) {
                    Some((_, a2)) => *a2,
                    None => {
                        let a2 = self.a.pos();
                        self.a.e(0);
                        self.a.e(0);
                        placed.push((r, a2));
                        self.relocs.push(RelocAt { at: (a2 * 4) as u32, kind: RelocKind::Pool64, target: r });
                        a2
                    }
                };
                if self.long {
                    let delta = ((addr - at) * 4) as u32;
                    if delta >= 1 << 24 {
                        BRANCH_RANGE.with(|c| c.set(true));
                    }
                    self.a.w[at + 1] |= ((delta >> 12) & 0xFFF) << 10;
                    self.a.w[at + 2] |= (delta & 0xFFF) << 10;
                } else {
                    self.a.patch(at, addr);
                }
            }
        }
        // vector literal pool (16-byte aligned, deduplicated)
        if !self.vpool.is_empty() {
            while self.a.pos() % 4 != 0 {
                self.a.udf();
            }
            let mut placed: Vec<(u32, usize)> = Vec::new();
            for (at, bits) in std::mem::take(&mut self.vpool) {
                let addr = match placed.iter().find(|(b2, _)| *b2 == bits) {
                    Some((_, a2)) => *a2,
                    None => {
                        let a2 = self.a.pos();
                        for _ in 0..4 {
                            self.a.e(bits);
                        }
                        placed.push((bits, a2));
                        a2
                    }
                };
                self.a.patch(at, addr);
            }
        }
        // unwind info for the runtime's frame walk (crate::unwind_info)
        let mut saved: Vec<u32> = self.al.used_callee_int.iter().map(|r| *r as u32).collect();
        saved.extend(self.al.used_callee_flt.iter().map(|x| 0x100 | *x as u32));
        let words = crate::rir::unwind_info_words(&saved, &unwind_tab, self.frame as u32);
        if crate::rir::unwind_stats_on() {
            crate::rir::UNWIND_STATS[2].fetch_add((words.len() * 4) as u64, std::sync::atomic::Ordering::Relaxed);
        }
        for w in words {
            self.a.e(w);
        }
    }

    /// Relax ABI params: incoming registers -> allocated locations; overflow params
    /// from the caller's outgoing area, [fp + 16 + 8k].
    fn params_prologue(&mut self) {
        let mut mv = Vec::new();
        let mut ni = 0usize;
        let mut nf = 0usize;
        let mut stack_params: Vec<(VReg, i64)> = Vec::new();
        for p in &self.f.params {
            let src = match self.cls(*p) {
                Cls::I if ni < INT_ARGS => {
                    ni += 1;
                    MLoc::R(ni as u8 - 1)
                }
                Cls::F32 | Cls::F64 if nf < FLT_ARGS => {
                    nf += 1;
                    MLoc::F(nf as u8 - 1)
                }
                _ => {
                    stack_params.push((*p, 16 + 8 * stack_params.len() as i64));
                    continue;
                }
            };
            if self.al.loc[p.0 as usize] != Loc::None {
                mv.push((self.mloc(*p), src));
            }
        }
        self.moves(&mv);
        for (p, off) in stack_params {
            let m = if self.cls(p) == Cls::I { Mem::Int(8, false) } else { Mem::F64 };
            self.load_param(p, m, off);
        }
    }

    /// Loads a stack-passed parameter ([fp + off], `m` sized) into its location.
    fn load_param(&mut self, p: VReg, m: Mem, off: i64) {
        let (op, size) = mem_op(m, true);
        match self.al.loc[p.0 as usize] {
            Loc::Reg(r) => self.a.ldst(op, size, r, FP, off),
            Loc::Flt(x) => self.a.ldst(op, size, x, FP, off),
            Loc::Stack(o) => {
                let so = self.spill_sp(o);
                if matches!(m, Mem::Int(..)) {
                    self.a.ldst(op, size, S1, FP, off);
                    self.a.ldst(STRX, 8, S1, SP, so);
                } else {
                    self.a.ldst(op, size, FS0, FP, off);
                    self.a.ldst(STRD, 8, FS0, SP, so);
                }
            }
            Loc::None => {}
        }
    }

    /// Sets a parameter's location to `base + off` (an address).
    fn def_addr_param(&mut self, p: VReg, base: u8, off: i64) {
        if self.al.loc[p.0 as usize] == Loc::None {
            return;
        }
        let rd = self.def_i(p, S0);
        self.a.add_any(rd, base, off);
        self.fin_i(p, rd);
    }

    /// Prologue of an `extern "C"` body: aggregate params are rebuilt in frame buffers
    /// (their address is the param), the rest arrive like Relax params but with C's
    /// placement (Apple stack packing, x8 result address); narrow ints are re-extended.
    fn c_prologue(&mut self) {
        let plan = self.cplan.clone().unwrap();
        let sig = self.f.cabi.clone().unwrap();
        let params = self.f.params.clone();
        let has_ret = sig.ret.is_some();
        let pargs = if has_ret { &params[1..] } else { &params[..] };
        // 1. aggregate pieces out of the argument registers (before any move clobbers them)
        for (k, ap) in plan.args.iter().enumerate() {
            if let ArgPlan::Parts(parts) = ap {
                let buf = self.cbuf[k];
                for pt in parts {
                    let o = buf + pt.off as i64;
                    match pt.loc {
                        PLoc::Int(r) => {
                            if pt.size == 4 {
                                self.a.ldst(STRW, 4, r, SP, o);
                            } else {
                                self.a.ldst(STRX, 8, r, SP, o);
                            }
                        }
                        PLoc::Flt(x) => {
                            if pt.size == 4 {
                                self.a.ldst(STRS, 4, x, SP, o);
                            } else {
                                self.a.ldst(STRD, 8, x, SP, o);
                            }
                        }
                        PLoc::Stack(_) => {}
                    }
                }
            }
        }
        // 2. register params (scalars, by-reference aggregates, the x8 result address)
        let mut mv = Vec::new();
        if let (RetPlan::Indirect, true) = (&plan.ret, has_ret) {
            if self.al.loc[params[0].0 as usize] != Loc::None {
                mv.push((self.mloc(params[0]), MLoc::R(X8)));
            }
        }
        for (k, ap) in plan.args.iter().enumerate() {
            let p = pargs[k];
            if self.al.loc[p.0 as usize] == Loc::None {
                continue;
            }
            match ap {
                ArgPlan::Scalar(PLoc::Int(r), _) | ArgPlan::ByRef(PLoc::Int(r)) => mv.push((self.mloc(p), MLoc::R(*r))),
                ArgPlan::Scalar(PLoc::Flt(x), _) => mv.push((self.mloc(p), MLoc::F(*x))),
                _ => {}
            }
        }
        self.moves(&mv);
        // 3. stack params, aggregate addresses, re-extension of narrow C integers
        for (k, ap) in plan.args.iter().enumerate() {
            let p = pargs[k];
            let m = match &sig.args[k] {
                CArg::Scalar(m) => *m,
                CArg::Agg(_) => Mem::Int(8, false),
            };
            match ap {
                ArgPlan::Scalar(PLoc::Stack(o), _) => self.load_param(p, m, 16 + *o as i64),
                ArgPlan::ByRef(PLoc::Stack(o)) => self.load_param(p, Mem::Int(8, false), 16 + *o as i64),
                ArgPlan::Parts(_) => {
                    let b = self.cbuf[k];
                    self.def_addr_param(p, SP, b);
                }
                ArgPlan::Stack(o, _) => self.def_addr_param(p, FP, 16 + *o as i64),
                ArgPlan::Scalar(PLoc::Int(_), _) => {
                    if let Mem::Int(b, sg) = m {
                        if b < 8 && self.al.loc[p.0 as usize] != Loc::None {
                            let r = self.use_i(p, S0);
                            self.a.extend(r, r, b * 8, sg);
                            self.fin_i(p, r);
                        }
                    }
                }
                _ => {}
            }
        }
        if let (RetPlan::Parts(_), true) = (&plan.ret, has_ret) {
            let b = self.cret_buf;
            self.def_addr_param(params[0], SP, b);
        }
    }

    /// Ret of an `extern "C"` body with an aggregate result held in registers.
    fn c_ret_parts(&mut self) {
        if let Some(RetPlan::Parts(parts)) = self.cplan.as_ref().map(|p| p.ret.clone()) {
            let buf = self.cret_buf;
            for pt in &parts {
                let o = buf + pt.off as i64;
                match pt.loc {
                    PLoc::Int(r) => self.a.ldst(LDRX, 8, r, SP, o),
                    PLoc::Flt(x) => {
                        if pt.size == 4 {
                            self.a.ldst(LDRS, 4, x, SP, o);
                        } else {
                            self.a.ldst(LDRD, 8, x, SP, o);
                        }
                    }
                    PLoc::Stack(_) => {}
                }
            }
        }
    }

    /// Touches every page of a big new frame from the top down, so a frame larger than
    /// the guard page faults on the guard instead of skipping past it.
    fn stack_probe(&mut self, frame: i64) {
        const PAGE: i64 = 4096;
        if frame <= PAGE {
            return;
        }
        // x17 = sp; loop: x17 -= PAGE; str xzr, [x17]; until x17 <= sp - frame
        self.a.mov_sp(IP1, SP);
        let mut o = PAGE;
        if frame <= 16 * PAGE {
            while o < frame {
                self.a.sub_imm_lsl12(IP1, IP1, (PAGE >> 12) as u32);
                self.a.ldst(STRX, 8, ZR, IP1, 0);
                o += PAGE;
            }
            return;
        }
        self.a.mov_imm(IP0, frame / PAGE);
        let top = self.a.pos();
        self.a.sub_imm_lsl12(IP1, IP1, (PAGE >> 12) as u32);
        self.a.ldst(STRX, 8, ZR, IP1, 0);
        self.a.sub_imm(IP0, IP0, 1);
        let p = self.a.cbnz(IP0);
        self.a.patch(p, top);
    }

    /// Copies `size` bytes [src + so] -> [dst + do_] through S1 (bases may be sp).
    fn copy_bytes(&mut self, dst: u8, do_: i64, src: u8, so: i64, size: u32) {
        let mut o = 0i64;
        let size = size as i64;
        while o < size {
            let (lop, sop, n) = if size - o >= 8 {
                (LDRX, STRX, 8)
            } else if size - o >= 4 {
                (LDRW, STRW, 4)
            } else if size - o >= 2 {
                (LDRH, STRH, 2)
            } else {
                (LDRB, STRB, 1)
            };
            self.a.ldst(lop, n as u32, S1, src, so + o);
            self.a.ldst(sop, n as u32, S1, dst, do_ + o);
            o += n;
        }
    }

    /// A call under the C ABI (foreign function, `extern "C"` pointer or body).
    fn c_call(&mut self, c: &Callee, sig: &CSig, args: &[VReg], rets: &[VReg]) {
        let plan = cabi::plan(sig, sig.conv.target());
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            self.save_restore(*v, *slot, true);
        }
        let has_ret = sig.ret.is_some();
        let cargs = if has_ret { &args[1..] } else { args };
        let st = self.stage;
        // the result address survives the call in staging cell 0
        if has_ret {
            let r = self.use_i(args[0], S0);
            self.a.ldst(STRX, 8, r, SP, st);
        }
        if let Callee::CIndirect(v, _) = c {
            let r = self.use_i(*v, IP0);
            self.a.mov(IP0, r);
        }
        // stack arguments and aggregate pieces (staged in cells), then register moves
        let mut mv = Vec::new();
        let mut cell = 1i64;
        for (k, ap) in plan.args.iter().enumerate() {
            let v = cargs[k];
            match ap {
                ArgPlan::Scalar(PLoc::Int(r), _) | ArgPlan::ByRef(PLoc::Int(r)) => mv.push((MLoc::R(*r), self.mloc(v))),
                ArgPlan::Scalar(PLoc::Flt(x), _) => mv.push((MLoc::F(*x), self.mloc(v))),
                ArgPlan::Scalar(PLoc::Stack(o), size) => {
                    let o = *o as i64;
                    if self.cls(v) == Cls::I {
                        let r = self.use_i(v, S0);
                        let op = match size {
                            1 => STRB,
                            2 => STRH,
                            4 => STRW,
                            _ => STRX,
                        };
                        self.a.ldst(op, (*size).max(1), r, SP, o);
                    } else {
                        let x = self.use_f(v, FS0);
                        if *size == 4 && self.cls(v) == Cls::F32 {
                            self.a.ldst(STRS, 4, x, SP, o);
                        } else if self.cls(v) == Cls::F32 {
                            // variadic f32 would be promoted by C; store as f64
                            self.a.e(0x1E22_C000 | (x as u32) << 5 | FS1 as u32);
                            self.a.ldst(STRD, 8, FS1, SP, o);
                        } else {
                            self.a.ldst(STRD, 8, x, SP, o);
                        }
                    }
                }
                ArgPlan::ByRef(PLoc::Stack(o)) => {
                    let r = self.use_i(v, S0);
                    self.a.ldst(STRX, 8, r, SP, *o as i64);
                }
                ArgPlan::ByRef(PLoc::Flt(_)) => {}
                ArgPlan::Parts(parts) => {
                    let ra = self.use_i(v, S0);
                    for pt in parts {
                        let cell_off = st + 8 * cell;
                        self.copy_bytes(SP, cell_off, ra, pt.off as i64, pt.size);
                        match pt.loc {
                            PLoc::Int(r) => mv.push((MLoc::R(r), MLoc::M(cell_off as i32))),
                            PLoc::Flt(x) => mv.push((MLoc::F(x), MLoc::M(cell_off as i32))),
                            PLoc::Stack(_) => {}
                        }
                        cell += 1;
                    }
                }
                ArgPlan::Stack(o, size) => {
                    let ra = self.use_i(v, S0);
                    self.copy_bytes(SP, *o as i64, ra, 0, *size);
                }
            }
        }
        if let (RetPlan::Indirect, true) = (&plan.ret, has_ret) {
            mv.push((MLoc::R(X8), MLoc::M(st as i32)));
        }
        self.moves(&mv);
        match c {
            Callee::CFn(id, _) => {
                self.ldr_reloc(IP0, Reloc::Slot(*id));
                self.a.ldst(LDRX, 8, IP0, IP0, 0);
                self.a.blr(IP0);
            }
            Callee::CHost(sym, _) => {
                self.ldr_reloc(IP0, Reloc::Sym(*sym));
                self.a.blr(IP0);
            }
            _ => self.a.blr(IP0),
        }
        // a C-unwind call is invoked like a Relax one: its landing stub is keyed by this address
        self.last_call_ret = self.a.pos();
        // C leaves a narrow integer result's upper bits undefined
        if let Some(it) = sig.ret_narrow {
            self.a.extend(0, 0, it.bits, it.signed);
        }
        // scalar results
        let mut mv = Vec::new();
        let mut ri = 0u8;
        let mut rf = 0u8;
        for r in rets {
            if self.cls(*r) == Cls::I {
                if self.al.loc[r.0 as usize] != Loc::None {
                    mv.push((self.mloc(*r), MLoc::R(ri)));
                }
                ri += 1;
            } else {
                if self.al.loc[r.0 as usize] != Loc::None {
                    mv.push((self.mloc(*r), MLoc::F(rf)));
                }
                rf += 1;
            }
        }
        // aggregate result registers -> cells (x0/x1/v0-v3 are never allocated)
        if let RetPlan::Parts(parts) = &plan.ret {
            for (j, pt) in parts.iter().enumerate() {
                let o = st + 8 * (1 + j as i64);
                match pt.loc {
                    PLoc::Int(r) => self.a.ldst(STRX, 8, r, SP, o),
                    PLoc::Flt(x) => self.a.ldst(STRD, 8, x, SP, o),
                    PLoc::Stack(_) => {}
                }
            }
        }
        self.moves(&mv);
        for (v, slot) in &saves {
            self.save_restore(*v, *slot, false);
        }
        if let RetPlan::Parts(parts) = &plan.ret {
            self.a.ldst(LDRX, 8, S0, SP, st);
            for (j, pt) in parts.iter().enumerate() {
                let o = st + 8 * (1 + j as i64);
                self.copy_bytes(S0, pt.off as i64, SP, o, pt.size);
            }
        }
    }

    // ---- atomics (LSE: every Apple arm64 CPU has ARMv8.1 atomics)

    fn atomic(&mut self, i: &Inst) {
        fn size_bits(m: Mem) -> (u32, u8, bool) {
            match m {
                Mem::Int(1, s) => (0, 8, s),
                Mem::Int(2, s) => (1, 16, s),
                Mem::Int(4, s) => (2, 32, s),
                Mem::Int(_, s) => (3, 64, s),
                _ => (3, 64, false),
            }
        }
        fn acq(o: AtomOrd) -> bool {
            matches!(o, AtomOrd::Acquire | AtomOrd::AcqRel | AtomOrd::SeqCst)
        }
        fn rel(o: AtomOrd) -> bool {
            matches!(o, AtomOrd::Release | AtomOrd::AcqRel | AtomOrd::SeqCst)
        }
        match i {
            Inst::AtomicLoad(m, o, d, a) => {
                let (sz, bits, sg) = size_bits(*m);
                let ra = self.use_i(*a, S1);
                let rd = self.def_i(*d, S0);
                if acq(*o) {
                    // ldar{b,h,w,x}
                    self.a.e(0x08DF_FC00 | sz << 30 | (ra as u32) << 5 | rd as u32);
                    if sg && bits < 64 {
                        self.a.extend(rd, rd, bits, true);
                    }
                } else {
                    let (op, size) = mem_op(*m, true);
                    self.a.ldst(op, size, rd, ra, 0);
                }
                self.fin_i(*d, rd);
            }
            Inst::AtomicStore(m, o, a, v) => {
                let (sz, _, _) = size_bits(*m);
                let ra = self.use_i(*a, S1);
                let rv = self.use_i(*v, S0);
                if rel(*o) {
                    // stlr{b,h,w,x}
                    self.a.e(0x089F_FC00 | sz << 30 | (ra as u32) << 5 | rv as u32);
                } else {
                    let (op, size) = mem_op(*m, false);
                    self.a.ldst(op, size, rv, ra, 0);
                }
            }
            Inst::AtomicRmw(op, m, o, d, a, v) => {
                let (sz, bits, sg) = size_bits(*m);
                let ar = (acq(*o) as u32) << 23 | (rel(*o) as u32) << 22;
                let ra = self.use_i(*a, IP0);
                let rv = self.use_i(*v, S1);
                let rd0 = self.def_i(*d, S0);
                // the old value goes to a register no operand uses
                let rd = if rd0 == ra || rd0 == rv { S0 } else { rd0 };
                // LDADD family: size 111000 A R 1 Rs o3 opc 00 Rn Rt
                let lse = |opc: u32, o3: u32, rs: u8| 0x3820_0000 | sz << 30 | ar | (rs as u32) << 16 | o3 << 15 | opc << 12 | (ra as u32) << 5;
                match op {
                    RmwOp::Xchg => self.a.e(lse(0, 1, rv) | rd as u32),
                    RmwOp::Add => self.a.e(lse(0, 0, rv) | rd as u32),
                    RmwOp::Sub => {
                        self.a.rrr(0xCB00_0000, IP1, ZR, rv);
                        self.a.e(lse(0, 0, IP1) | rd as u32);
                    }
                    RmwOp::And => {
                        // ldclr with the complement
                        self.a.rrr(0xAA20_0000, IP1, ZR, rv);
                        self.a.e(lse(1, 0, IP1) | rd as u32);
                    }
                    RmwOp::Xor => self.a.e(lse(2, 0, rv) | rd as u32),
                    RmwOp::Or => self.a.e(lse(3, 0, rv) | rd as u32),
                    RmwOp::Max => self.a.e(lse(4, 0, rv) | rd as u32),
                    RmwOp::Min => self.a.e(lse(5, 0, rv) | rd as u32),
                    RmwOp::UMax => self.a.e(lse(6, 0, rv) | rd as u32),
                    RmwOp::UMin => self.a.e(lse(7, 0, rv) | rd as u32),
                    RmwOp::Nand => {
                        // old = [a]; loop { new = !(old & v); cas(old -> new) }
                        let (lop, lsz) = mem_op(Mem::Int(1 << sz, false), true);
                        self.a.ldst(lop, lsz, X8, ra, 0);
                        let top = self.a.pos();
                        self.a.rrr(0x8A00_0000, IP1, X8, rv);
                        self.a.rrr(0xAA20_0000, IP1, ZR, IP1);
                        self.a.mov(rd, X8);
                        // casal x8(expected -> observed), ip1(new), [ra]
                        self.a.e(0x08A0_7C00 | sz << 30 | 1 << 22 | 1 << 15 | (X8 as u32) << 16 | (ra as u32) << 5 | IP1 as u32);
                        self.a.cmp(X8, rd);
                        let p = self.a.bcond(NE);
                        self.a.patch(p, top);
                    }
                }
                if sg && bits < 64 {
                    self.a.extend(rd0, rd, bits, true);
                } else {
                    self.a.mov(rd0, rd);
                }
                self.fin_i(*d, rd0);
            }
            Inst::AtomicCas(m, ok, fail, d, a, e, n) => {
                let (sz, bits, sg) = size_bits(*m);
                let ra = self.use_i(*a, IP0);
                let re = self.use_i(*e, S0);
                let rn = self.use_i(*n, S1);
                let a_bit = (acq(*ok) || acq(*fail)) as u32;
                let r_bit = rel(*ok) as u32;
                // cas: Rs = expected in, observed out
                self.a.mov(IP1, re);
                self.a.e(0x08A0_7C00 | sz << 30 | a_bit << 22 | r_bit << 15 | (IP1 as u32) << 16 | (ra as u32) << 5 | rn as u32);
                let rd = self.def_i(*d, S0);
                self.a.mov(rd, IP1);
                if sg && bits < 64 {
                    self.a.extend(rd, rd, bits, true);
                }
                self.fin_i(*d, rd);
            }
            Inst::Fence(o, single) => {
                if !*single && *o != AtomOrd::Relaxed {
                    if *o == AtomOrd::Acquire {
                        self.a.e(0xD503_39BF); // dmb ishld
                    } else {
                        self.a.e(0xD503_3BBF); // dmb ish
                    }
                }
            }
            _ => {}
        }
    }

    /// x16 = this thread's thread-local block. Fast path: the block pointer sits in a word of
    /// the thread's slot array (macOS: pthread TSD slot `key` at tpidrro_el0; Android: bionic
    /// TLS_SLOT_APP at tpidr_el0, os/tls.rs); a null slot calls `tls_slow`, which keeps every
    /// register but x16/x17 (glue), so no saves here; lr is preserved by the frame record.
    fn tls_block_ip0(&mut self) {
        // mrs x16, <thread slot base>; and x16, x16, #~7; ldr x17, =key*8; ldr x16, [x16, x17]
        self.a.e(MRS_SLOT_BASE | IP0 as u32);
        self.a.e(0x9243_F000 | (IP0 as u32) << 5 | IP0 as u32);
        self.ldr_reloc(IP1, Reloc::TlsKey);
        self.a.e(0xF860_6800 | (IP1 as u32) << 16 | (IP0 as u32) << 5 | IP0 as u32);
        let p = self.a.cbnz(IP0);
        self.ldr_reloc(IP1, Reloc::TlsSlow);
        self.a.blr(IP1);
        let skip = self.a.pos();
        self.a.patch(p, skip);
    }

    /// d = this thread's thread-local block + off.
    fn tls_addr(&mut self, d: VReg, off: Sym) {
        self.tls_block_ip0();
        // the variable's offset in the block (a symbol: it is assigned per run)
        self.ldr_reloc(IP1, Reloc::Sym(off));
        let rd = self.def_i(d, S0);
        self.a.rrr(0x8B00_0000, rd, IP0, IP1);
        self.fin_i(d, rd);
    }

    /// Prologue of a function that names domain statics: the running thread's domain base
    /// (slot 0 of its thread-local block) into the frame's domain slot, read once per call.
    fn dom_base_prologue(&mut self) {
        // the shared TLS routine (fast path inside), not the inline sequence: one call per entry
        self.ldr_reloc(IP1, Reloc::TlsSlow);
        self.a.blr(IP1);
        // ldr x16, [x16]: the domain base
        self.a.e(0xF940_0000 | (IP0 as u32) << 5 | IP0 as u32);
        self.a.ldst(STRX, 8, IP0, SP, self.dom_slot);
    }

    /// rd = the running thread's domain static block + `off` (a domain symbol: the offset in
    /// the block, assigned per run): the base the prologue stored + the offset.
    fn dom_addr(&mut self, rd: u8, off: Sym) {
        self.a.ldst(LDRX, 8, IP0, SP, self.dom_slot);
        self.ldr_reloc(IP1, Reloc::Sym(off));
        self.a.rrr(0x8B00_0000, rd, IP0, IP1);
    }

    /// `ldr xd, =<relocated value>` from the address pool after the code.
    fn ldr_reloc(&mut self, rd: u8, r: Reloc) {
        let at = self.a.pos();
        if self.long {
            // past +-1 MB: adr rd, .; add rd, rd, #hi, lsl 12; add rd, rd, #lo; ldr rd, [rd]
            // (pc-relative within the code image, up to 16 MB ahead; the adds are patched when
            // the pool is placed)
            self.a.e(0x1000_0000 | rd as u32);
            self.a.e(0x9140_0000 | (rd as u32) << 5 | rd as u32);
            self.a.e(0x9100_0000 | (rd as u32) << 5 | rd as u32);
            self.a.e(0xF940_0000 | (rd as u32) << 5 | rd as u32);
        } else {
            self.a.e(0x5800_0000 | rd as u32);
        }
        self.apool.push((at, r));
    }

    // ---- 128-bit vectors (NEON, 4S lanes)

    /// q register t <-> [n + off] (any offset; x17 for far ones)
    /// Saves (or restores) a caller-saved register's value around a call: 16 bytes for a vector.
    fn save_restore(&mut self, v: VReg, slot: i32, save: bool) {
        let off = self.spill_sp(slot);
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => self.a.ldst(if save { STRX } else { LDRX }, 8, r, SP, off),
            Loc::Flt(x) if self.f.vregs[v.0 as usize] == Cls::V128 => self.vldst(!save, x, SP, off),
            Loc::Flt(x) => self.a.ldst(if save { STRD } else { LDRD }, 8, x, SP, off),
            _ => {}
        }
    }

    fn vldst(&mut self, load: bool, t: u8, n: u8, off: i64) {
        if off >= 0 && off % 16 == 0 && off / 16 < 4096 {
            self.a.e(if load { 0x3DC0_0000 } else { 0x3D80_0000 } | ((off / 16) as u32) << 10 | (n as u32) << 5 | t as u32);
        } else if (-256..256).contains(&off) {
            self.a.e(if load { 0x3CC0_0000 } else { 0x3C80_0000 } | ((off as u32) & 0x1ff) << 12 | (n as u32) << 5 | t as u32);
        } else {
            self.a.add_any(IP1, n, off);
            self.a.e(if load { 0x3DC0_0000 } else { 0x3D80_0000 } | (IP1 as u32) << 5 | t as u32);
        }
    }

    fn vmov(&mut self, d: u8, n: u8) {
        if d != n {
            // orr vd.16b, vn.16b, vn.16b
            self.a.e(0x4EA0_1C00 | (n as u32) << 16 | (n as u32) << 5 | d as u32);
        }
    }

    /// Where an in-place vector op builds `d`: d's own register unless one of `keep`
    /// (read after the build starts) lives there, else FS2 (spilled d stores from FS2).
    fn vtmp(&self, d: VReg, keep: &[VReg]) -> u8 {
        let xd = self.def_f(d, FS2);
        let clash = keep.iter().enumerate().any(|(k, &r)| self.def_f(r, if k == 0 { FS0 } else { FS1 }) == xd);
        if clash {
            FS2
        } else {
            xd
        }
    }

    // Repair the ARM arithmetic result to SSE's NaN selection, all in vectors.
    // a/b remain intact even if the allocated result aliases an operand.
    fn intel_nan(&mut self, result: u8, a: u8, b: u8) {
        const FCMEQ: u32 = 0x4E20_E400;
        const NOT: u32 = 0x6E20_5800;
        const BSL: u32 = 0x6E60_1C00;
        const ORR: u32 = 0x4EA0_1C00;
        self.const128(5, 0xffc00000ffc00000, 0xffc00000ffc00000);
        self.v3(FCMEQ, FS2, result, result);
        self.v2(NOT, FS2, FS2);
        self.v3(BSL, FS2, 5, result);
        self.vmov(result, FS2);
        self.const128(5, 0x0040000000400000, 0x0040000000400000);
        for input in [b, a] {
            self.v3(ORR, 6, input, 5);
            self.v3(FCMEQ, FS2, input, input);
            self.v2(NOT, FS2, FS2);
            self.v3(BSL, FS2, 6, result);
            self.vmov(result, FS2);
        }
    }

    fn v3(&mut self, op: u32, d: u8, n: u8, m: u8) {
        self.a.e(op | (m as u32) << 16 | (n as u32) << 5 | d as u32);
    }

    fn v2(&mut self, op: u32, d: u8, n: u8) {
        self.a.e(op | (n as u32) << 5 | d as u32);
    }


    /// Reverse four 32-bit words, without reversing bytes inside a word.
    fn sha_reverse(&mut self, d: u8, a: u8) {
        self.v2(0x4EA0_0800, d, a); // rev64 .4s
        self.v3(0x6E00_4000, d, d, d); // ext #8
    }

    fn vx3(&mut self, op: VX, d: VReg, a: VReg, b: VReg, c: VReg) {
        if op == VX::X86Sha256Rnds2 {
            // Intel's [H,G,D,C], [F,E,B,A] -> Arm's [A,B,C,D], [E,F,G,H].
            // After four Arm rounds, lanes 2/3 still contain the round-2/1 values.
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            let xc = self.use_f(c, FS2);
            self.v3(0x4EC0_7800, 4, xa, xb); // zip2 .2d
            self.sha_reverse(4, 4);
            self.v3(0x4EC0_3800, 5, xa, xb); // zip1 .2d
            self.sha_reverse(5, 5);
            self.vmov(6, 4);
            self.v3(0x5E00_4000, 6, 5, xc); // sha256h
            self.v3(0x5E00_5000, 5, 4, xc); // sha256h2, original ABCD
            self.v3(0x4EC0_7800, 4, 5, 6); // [E2,E1,A2,A1]
            self.v2(0x4EA0_0800, 4, 4); // [E1,E2,A1,A2]
            let xd = self.def_f(d, FS0);
            self.vmov(xd, 4);
            self.fin_f(d, xd);
            return;
        }
        let encoding = match op {
            VX::ArmSha1C => 0x5E00_0000,
            VX::ArmSha1P => 0x5E00_1000,
            VX::ArmSha1M => 0x5E00_2000,
            VX::ArmSha1Su0 => 0x5E00_3000,
            VX::ArmSha256H => 0x5E00_4000,
            VX::ArmSha256H2 => 0x5E00_5000,
            VX::ArmSha256Su1 => 0x5E00_6000,
            _ => unreachable!("arm64 ternary vx {:?}", op),
        };
        let xa = self.use_f(a, FS0);
        let xb = self.use_f(b, FS1);
        // v4 is unallocated; leave FS2 available for an aliased/spilled result.
        let xc = self.use_f(c, 4);
        let xd = self.def_f(d, FS2);
        let tmp = if xd == xb || xd == xc { FS2 } else { xd };
        self.vmov(tmp, xa);
        self.v3(encoding, tmp, xb, xc);
        self.vmov(xd, tmp);
        self.fin_f(d, xd);
    }

    /// Lane-typed vector ops (core::arch intrinsics), NEON. Scratch: v29-v31 (FS*) and
    /// v4-v7 (argument registers, unallocated inside an instruction).
    fn vx(&mut self, op: VX, d: VReg, a: VReg, b: VReg) {
        // core::arch exposes Intel semantics on Arm too (check_arch's cross-ISA
        // fixtures exercise them). Keep these in hardware, with lane conversion.
        if matches!(op, VX::X86Sha1Rnds4(_) | VX::X86Sha1NextE | VX::X86Sha1Msg1
            | VX::X86Sha1Msg2 | VX::X86Sha256Msg1 | VX::X86Sha256Msg2) {
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            match op {
                VX::X86Sha1Rnds4(k) => {
                    self.sha_reverse(4, xa);
                    self.sha_reverse(5, xb);
                    let kword = [0x5a82_7999u64, 0x6ed9_eba1, 0x8f1b_bcdc, 0xca62_c1d6][k as usize];
                    let pair = kword | kword << 32;
                    self.const128(6, pair, pair);
                    self.v3(0x4EA0_8400, 5, 5, 6); // add .4s, WK includes K on Arm
                    self.movi16b(6, 0); // E is already included in Intel's first WK
                    let enc = match k { 0 => 0x5E00_0000, 2 => 0x5E00_2000, _ => 0x5E00_1000 };
                    self.v3(enc, 4, 6, 5);
                    self.sha_reverse(4, 4);
                }
                VX::X86Sha1NextE => {
                    self.sha_reverse(4, xa);
                    self.v2(0x5E28_0800, 4, 4); // sha1h, zero upper lanes
                    self.movi16b(5, 0);
                    self.v3(0x6E00_2000, 4, 5, 4); // ext #4: move E to lane 3
                    self.v3(0x4EA0_8400, 4, xb, 4);
                }
                VX::X86Sha1Msg1 | VX::X86Sha1Msg2 => {
                    self.sha_reverse(4, xa);
                    self.sha_reverse(5, xb);
                    if op == VX::X86Sha1Msg1 {
                        self.movi16b(6, 0);
                        self.v3(0x5E00_3000, 4, 5, 6);
                    } else {
                        self.v2(0x5E28_1800, 4, 5);
                    }
                    self.sha_reverse(4, 4);
                }
                VX::X86Sha256Msg1 => {
                    self.vmov(4, xa);
                    self.v2(0x5E28_2800, 4, xb);
                }
                VX::X86Sha256Msg2 => {
                    self.vmov(4, xa);
                    self.movi16b(5, 0);
                    self.vmov(6, xb);
                    self.v2(0x4E04_1C00, 6, 31); // ins v6.s[0], wzr
                    self.v3(0x5E00_6000, 4, 5, 6); // no middle-word additions
                }
                _ => unreachable!(),
            }
            let xd = self.def_f(d, FS0);
            self.vmov(xd, 4);
            self.fin_f(d, xd);
            return;
        }
        if op == VX::ArmSha1H {
            let xa = self.use_f(a, FS0);
            let xd = self.def_f(d, FS2);
            self.v2(0x5E28_0800, xd, xa);
            self.fin_f(d, xd);
            return;
        }
        let sha = match op {
            VX::ArmSha1Su1 => Some(0x5E28_1800),
            VX::ArmSha256Su0 => Some(0x5E28_2800),
            _ => None,
        };
        if let Some(encoding) = sha {
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            let xd = self.def_f(d, FS2);
            let tmp = if xd == xb && xd != xa { FS2 } else { xd };
            self.vmov(tmp, xa);
            self.v2(encoding, tmp, xb);
            self.vmov(xd, tmp);
            self.fin_f(d, xd);
            return;
        }
        // three-same (Q=1): u, opcode
        let ts = |u: u32, w: VW, opc: u32| -> u32 { 0x4E20_0400 | u << 29 | (w as u32) << 22 | opc << 11 };
        const AND: u32 = 0x4E20_1C00;
        const ORR: u32 = 0x4EA0_1C00;
        const EOR: u32 = 0x6E20_1C00;
        const BIC: u32 = 0x4E60_1C00;
        const T0: u8 = 4;
        const T1: u8 = 5;
        const T2: u8 = 6;
        const T3: u8 = 7;
        let three = match op {
            VX::Add(w) => Some(ts(0, w, 0b10000)),
            VX::Sub(w) => Some(ts(1, w, 0b10000)),
            VX::MulLo(w) => Some(ts(0, w, 0b10011)),
            VX::MaxS(w) => Some(ts(0, w, 0b01100)),
            VX::MinS(w) => Some(ts(0, w, 0b01101)),
            VX::MaxU(w) => Some(ts(1, w, 0b01100)),
            VX::MinU(w) => Some(ts(1, w, 0b01101)),
            VX::CmpEq(w) => Some(ts(1, w, 0b10001)),
            VX::CmpGtS(w) => Some(ts(0, w, 0b00110)),
            VX::AddSatU(w) => Some(ts(1, w, 0b00001)),
            VX::AddSatS(w) => Some(ts(0, w, 0b00001)),
            VX::SubSatU(w) => Some(ts(1, w, 0b00101)),
            VX::SubSatS(w) => Some(ts(0, w, 0b00101)),
            VX::AvgU(w) => Some(ts(1, w, 0b00010)),
            VX::And => Some(AND),
            VX::Or => Some(ORR),
            VX::Xor => Some(EOR),
            VX::AndNot => Some(BIC),
            VX::ZipLo(w) => Some(0x4E00_3800 | (w as u32) << 22),
            VX::ZipHi(w) => Some(0x4E00_7800 | (w as u32) << 22),
            _ => None,
        };
        if let Some(opc) = three {
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            let xd = self.def_f(d, FS0);
            self.v3(opc, xd, xa, xb);
            self.fin_f(d, xd);
            return;
        }
        let esize = |w: VW| w.bits();
        match op {
            VX::MaddH => {
                // smull lo / smull2 hi (h -> s), then pairwise add
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v3(0x0E60_C000, T0, xa, xb);
                self.v3(0x4E60_C000, T1, xa, xb);
                let xd = self.def_f(d, FS0);
                self.v3(0x4EA0_BC00, xd, T0, T1); // addp .4s
                self.fin_f(d, xd);
            }
            VX::MaddUbs => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v2(0x2F08_A400, T0, xa); // ushll .8h, .8b, #0
                self.v2(0x6F08_A400, T1, xa); // ushll2
                self.v2(0x0F08_A400, T2, xb); // sshll
                self.v2(0x4F08_A400, T3, xb); // sshll2
                self.v3(ts(0, VW::H, 0b10011), T0, T0, T2); // mul .8h
                self.v3(ts(0, VW::H, 0b10011), T1, T1, T3);
                self.v2(0x4E60_2800, T0, T0); // saddlp .4s, .8h
                self.v2(0x4E60_2800, T1, T1);
                let xd = self.def_f(d, FS0);
                self.v2(0x0E61_4800, xd, T0); // sqxtn .4h, .4s
                self.v2(0x4E61_4800, xd, T1); // sqxtn2 .8h, .4s
                self.fin_f(d, xd);
            }
            VX::SadU8 => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v3(0x6E20_7400, T0, xa, xb); // uabd .16b
                self.v2(0x6E20_2800, T0, T0); // uaddlp .8h
                self.v2(0x6E60_2800, T0, T0); // uaddlp .4s
                let xd = self.def_f(d, FS0);
                self.v2(0x6EA0_2800, xd, T0); // uaddlp .2d
                self.fin_f(d, xd);
            }
            VX::PackUsH | VX::PackSsH | VX::PackSsS | VX::PackUsS => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                let (lo, hi) = match op {
                    VX::PackSsH => (0x0E21_4800, 0x4E21_4800), // sqxtn(2) .8b/.16b
                    VX::PackUsH => (0x2E21_2800, 0x6E21_2800), // sqxtun(2) .8b/.16b from .8h
                    VX::PackSsS => (0x0E61_4800, 0x4E61_4800), // sqxtn(2) .4h/.8h from .4s
                    _ => (0x2E61_2800, 0x6E61_2800),           // sqxtun(2) .4h/.8h from .4s
                };
                self.v2(lo, T0, xa);
                self.v2(hi, T0, xb);
                let xd = self.def_f(d, FS0);
                self.vmov(xd, T0);
                self.fin_f(d, xd);
            }
            VX::ByteShuffle => {
                // tbl gives 0 for indices >= 16: keep bit 7 and the low 4 bits
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.movi16b(T0, 0x8f);
                self.v3(AND, T0, xb, T0);
                let xd = self.def_f(d, FS0);
                self.v3(0x4E00_0000, xd, xa, T0); // tbl
                self.fin_f(d, xd);
            }
            VX::Abs(w) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                self.v2(0x4E20_B800 | (w as u32) << 22, xd, xa);
                self.fin_f(d, xd);
            }
            VX::ShlI(w, k) | VX::ShrUI(w, k) | VX::ShrSI(w, k) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                let es = esize(w);
                let k = k as u32;
                match op {
                    VX::ShlI(..) if k >= es => self.a.e(0x6F00_E400 | xd as u32), // movi 0
                    VX::ShlI(..) if k == 0 => self.vmov(xd, xa),
                    VX::ShlI(..) => self.v2(0x4F00_5400 | (es + k) << 16, xd, xa),
                    VX::ShrUI(..) if k >= es => self.a.e(0x6F00_E400 | xd as u32),
                    VX::ShrUI(..) if k == 0 => self.vmov(xd, xa),
                    VX::ShrUI(..) => self.v2(0x6F00_0400 | (2 * es - k) << 16, xd, xa),
                    _ if k == 0 => self.vmov(xd, xa),
                    _ => self.v2(0x4F00_0400 | (2 * es - k.min(es)) << 16, xd, xa),
                }
                self.fin_f(d, xd);
            }
            VX::ShufS(k) => {
                // tbl with the byte indices of the selected 32-bit lanes
                let xa = self.use_f(a, FS0);
                let mut lo = 0u64;
                let mut hi = 0u64;
                for lane in 0..4u64 {
                    let sel = ((k as u64) >> (2 * lane)) & 3;
                    for byte in 0..4u64 {
                        let idx = sel * 4 + byte;
                        let pos = lane * 4 + byte;
                        if pos < 8 {
                            lo |= idx << (8 * pos);
                        } else {
                            hi |= idx << (8 * (pos - 8));
                        }
                    }
                }
                self.const128(T0, lo, hi);
                let xd = self.def_f(d, FS0);
                self.v3(0x4E00_0000, xd, xa, T0);
                self.fin_f(d, xd);
            }
            VX::BytesShrI(k) | VX::BytesShlI(k) => {
                let xa = self.use_f(a, FS0);
                self.a.e(0x6F00_E400 | T0 as u32); // movi T0, #0
                let xd = self.def_f(d, FS0);
                let k = (k as u32).min(16);
                if k == 16 {
                    self.vmov(xd, T0);
                } else if let VX::BytesShrI(_) = op {
                    // ext d, a, zero, #k: bytes k.. of a, then zeros
                    self.v3(0x6E00_0000 | k << 11, xd, xa, T0);
                } else if k == 0 {
                    self.vmov(xd, xa);
                } else {
                    // ext d, zero, a, #(16-k)
                    self.v3(0x6E00_0000 | (16 - k) << 11, xd, T0, xa);
                }
                self.fin_f(d, xd);
            }
            VX::WidenLoU(w) | VX::WidenHiU(w) | VX::WidenLoS(w) | VX::WidenHiS(w) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                let immh = 1u32 << (w as u32); // B 0001, H 0010, S 0100
                let base = match op {
                    VX::WidenLoU(_) => 0x2F00_A400,
                    VX::WidenHiU(_) => 0x6F00_A400,
                    VX::WidenLoS(_) => 0x0F00_A400,
                    _ => 0x4F00_A400,
                };
                self.v2(base | immh << 19, xd, xa);
                self.fin_f(d, xd);
            }
            VX::PairAddLongU(w) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                self.v2(0x6E20_2800 | (w as u32) << 22, xd, xa);
                self.fin_f(d, xd);
            }
            VX::Splat(w) => {
                let ra = self.use_i(a, S0);
                let xd = self.def_f(d, FS0);
                self.v2(0x4E00_0C00 | (1u32 << (w as u32)) << 16, xd, ra);
                self.fin_f(d, xd);
            }
            VX::FromLo64 => {
                let ra = self.use_i(a, S0);
                let xd = self.def_f(d, FS0);
                self.v2(0x9E67_0000, xd, ra); // fmov d, x
                self.fin_f(d, xd);
            }
            VX::Extract(w, k) => {
                let xa = self.use_f(a, FS0);
                let rd = self.def_i(d, S0);
                let sz = w as u32;
                let imm5 = ((k as u32) << (sz + 1)) | (1 << sz);
                let q = if w == VW::D { 0x4000_0000 } else { 0 };
                self.v2(0x0E00_3C00 | q | imm5 << 16, rd, xa); // umov
                self.fin_i(d, rd);
            }
            VX::Insert(w, k) => {
                let xa = self.use_f(a, FS0);
                let rb = self.use_i(b, S0);
                let xd = self.def_f(d, FS0);
                self.vmov(xd, xa);
                let sz = w as u32;
                let imm5 = ((k as u32) << (sz + 1)) | (1 << sz);
                self.v2(0x4E00_1C00 | imm5 << 16, xd, rb); // ins
                self.fin_f(d, xd);
            }
            VX::MoveMaskB => {
                // top bit of each byte -> 0/1 bytes; gather each half with a multiply
                let xa = self.use_f(a, FS0);
                self.v2(0x6F09_0400, T0, xa); // ushr .16b, #7
                self.v2(0x4E08_3C00, IP0, T0); // umov x16, v.d[0]
                self.v2(0x4E18_3C00, IP1, T0); // umov x17, v.d[1]
                self.a.mov_imm(S1, 0x0102_0408_1020_4080u64 as i64);
                self.a.rrr(0x9B00_7C00, IP0, IP0, S1); // mul
                self.a.rrr(0x9B00_7C00, IP1, IP1, S1);
                let rd = self.def_i(d, S0);
                self.a.e(0xD378_FC00 | (IP0 as u32) << 5 | IP0 as u32); // lsr x16, x16, #56
                self.a.e(0xD378_FC00 | (IP1 as u32) << 5 | IP1 as u32);
                self.a.e(0xAA00_2000 | (IP1 as u32) << 16 | (IP0 as u32) << 5 | rd as u32); // orr rd, x16, x17, lsl #8
                self.fin_i(d, rd);
            }
            VX::AddAcross(w) | VX::MaxAcrossU(w) => {
                let xa = self.use_f(a, FS0);
                let rd = self.def_i(d, S0);
                if w == VW::D {
                    // addp d, v.2d (no u64 max across: the table never asks)
                    self.v2(0x5EF1_B800, T0, xa);
                    self.v2(0x9E66_0000, rd, T0); // fmov x, d
                } else {
                    let base = if let VX::AddAcross(_) = op { 0x4E31_B800 } else { 0x6E30_A800 };
                    self.v2(base | (w as u32) << 22, T0, xa);
                    let sz = w as u32;
                    self.v2(0x0E00_3C00 | (1u32 << sz) << 16, rd, T0); // umov lane 0
                }
                self.fin_i(d, rd);
            }
            _ => unreachable!("vx {:?}", op),
        }
    }

    /// movi v.16b, #imm8
    fn movi16b(&mut self, x: u8, imm: u8) {
        let i = imm as u32;
        self.a.e(0x4F00_E400 | (i >> 5) << 16 | (i & 31) << 5 | x as u32);
    }

    /// a 128-bit constant through the general scratch registers
    fn const128(&mut self, x: u8, lo: u64, hi: u64) {
        self.a.mov_imm(IP0, lo as i64);
        self.v2(0x4E08_1C00, x, IP0); // ins v.d[0], x16
        self.a.mov_imm(IP0, hi as i64);
        self.v2(0x4E18_1C00, x, IP0); // ins v.d[1], x16
    }

    /// 128-bit vector instructions (NEON, 4 x 32-bit lanes; FPCR as the scalar code).
    fn vinst(&mut self, i: &Inst) {
        const FADD: u32 = 0x4E20_D400;
        const FSUB: u32 = 0x4EA0_D400;
        const FMUL: u32 = 0x6E20_DC00;
        const FDIV: u32 = 0x6E20_FC00;
        const FCMEQ: u32 = 0x4E20_E400;
        const FCMGE: u32 = 0x6E20_E400;
        const FCMGT: u32 = 0x6EA0_E400;
        const ADD: u32 = 0x4EA0_8400;
        const SUB: u32 = 0x6EA0_8400;
        const MUL: u32 = 0x4EA0_9C00;
        const AND: u32 = 0x4E20_1C00;
        const ORR: u32 = 0x4EA0_1C00;
        const EOR: u32 = 0x6E20_1C00;
        const BIC: u32 = 0x4E60_1C00;
        const BSL: u32 = 0x6E60_1C00;
        const CMEQ: u32 = 0x6EA0_8C00;
        const CMGT: u32 = 0x4EA0_3400;
        const CMGE: u32 = 0x4EA0_3C00;
        const SSHL: u32 = 0x4EA0_4400;
        const USHL: u32 = 0x6EA0_4400;
        const NEG: u32 = 0x6EA0_B800;
        const NOT: u32 = 0x6E20_5800;
        const VFNEG: u32 = 0x6EA0_F800;
        match *i {
            Inst::VBin(op, d, a, b) => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                let xd = self.def_f(d, FS0);
                match op {
                    VOp::FAdd | VOp::FAddNeon => self.v3(FADD, xd, xa, xb),
                    VOp::FSub | VOp::FSubNeon => self.v3(FSUB, xd, xa, xb),
                    VOp::FMul | VOp::FMulNeon => self.v3(FMUL, xd, xa, xb),
                    VOp::FDiv | VOp::FDivNeon => self.v3(FDIV, xd, xa, xb),
                    VOp::FMinNeon => self.v3(0x4EA0_F400, xd, xa, xb),
                    VOp::FMaxNeon => self.v3(0x4E20_F400, xd, xa, xb),
                    VOp::FAddIntel | VOp::FSubIntel | VOp::FMulIntel | VOp::FDivIntel => {
                        let opc = match op {
                            VOp::FAddIntel => FADD,
                            VOp::FSubIntel => FSUB,
                            VOp::FMulIntel => FMUL,
                            _ => FDIV,
                        };
                        self.v3(opc, 4, xa, xb);
                        self.intel_nan(4, xa, xb);
                        self.vmov(xd, 4);
                    }
                    VOp::FMinLt | VOp::FMaxGt => {
                        // mask = b > a (min) / a > b (max), false for NaN lanes: pick a there
                        if op == VOp::FMinLt {
                            self.v3(FCMGT, FS2, xb, xa);
                        } else {
                            self.v3(FCMGT, FS2, xa, xb);
                        }
                        self.v3(BSL, FS2, xa, xb);
                        self.vmov(xd, FS2);
                    }
                    VOp::IAdd => self.v3(ADD, xd, xa, xb),
                    VOp::ISub => self.v3(SUB, xd, xa, xb),
                    VOp::IMul => self.v3(MUL, xd, xa, xb),
                    VOp::And => self.v3(AND, xd, xa, xb),
                    VOp::Or => self.v3(ORR, xd, xa, xb),
                    VOp::Xor => self.v3(EOR, xd, xa, xb),
                    VOp::AndNot => self.v3(BIC, xd, xa, xb),
                    VOp::UMin => self.v3(0x6EA0_6C00, xd, xa, xb), // umin v.4s
                    VOp::Shl => self.v3(USHL, xd, xa, xb),
                    VOp::ShrS | VOp::ShrU => {
                        self.v2(NEG, FS2, xb);
                        self.v3(if op == VOp::ShrS { SSHL } else { USHL }, xd, xa, FS2);
                    }
                    VOp::FCmp(c) => match c {
                        Cond::Lt => self.v3(FCMGT, xd, xb, xa),
                        Cond::Le => self.v3(FCMGE, xd, xb, xa),
                        Cond::Gt => self.v3(FCMGT, xd, xa, xb),
                        Cond::Ge => self.v3(FCMGE, xd, xa, xb),
                        Cond::Eq => self.v3(FCMEQ, xd, xa, xb),
                        Cond::Ne => {
                            self.v3(FCMEQ, xd, xa, xb);
                            self.v2(NOT, xd, xd);
                        }
                    },
                    VOp::ICmp(c) => match c {
                        Cond::Lt => self.v3(CMGT, xd, xb, xa),
                        Cond::Le => self.v3(CMGE, xd, xb, xa),
                        Cond::Gt => self.v3(CMGT, xd, xa, xb),
                        Cond::Ge => self.v3(CMGE, xd, xa, xb),
                        Cond::Eq => self.v3(CMEQ, xd, xa, xb),
                        Cond::Ne => {
                            self.v3(CMEQ, xd, xa, xb);
                            self.v2(NOT, xd, xd);
                        }
                    },
                }
                self.fin_f(d, xd);
            }
            Inst::VUn(op, d, a) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                if op == VUn::FSqrtIntel {
                    self.v2(0x6EA1_F800, 4, xa);
                    self.intel_nan(4, xa, xa);
                    self.vmov(xd, 4);
                    self.fin_f(d, xd);
                    return;
                }
                let opc = match op {
                    VUn::FNeg => VFNEG,
                    VUn::FAbs => 0x4EA0_F800,
                    VUn::FSqrt | VUn::FSqrtIntel | VUn::FSqrtNeon => 0x6EA1_F800,
                    VUn::FFloor => 0x4E21_9800,
                    VUn::FCeil => 0x4EA1_8800,
                    VUn::FTrunc => 0x4EA1_9800,
                    VUn::FRoundAway => 0x6E21_8800,
                    VUn::FToI => 0x4EA1_B800,
                    VUn::IToF => 0x4E21_D800,
                    VUn::INeg => NEG,
                };
                self.v2(opc, xd, xa);
                self.fin_f(d, xd);
            }
            Inst::VSel(d, m, a, b) => {
                // bsl works in place on the mask: build it in d's register when a and b
                // live elsewhere
                let t = self.vtmp(d, &[a, b]);
                let xm = self.use_f(m, t);
                self.vmov(t, xm);
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v3(BSL, t, xa, xb);
                let xd = self.def_f(d, FS2);
                self.vmov(xd, t);
                self.fin_f(d, xd);
            }
            Inst::VFma(np, na, d, a, b, c) => {
                // t = c (negated first for the - c forms: exact, and the fused op then
                // rounds once, with IEEE's sign of an exact zero); fmla / fmls t, a, b
                let t = self.vtmp(d, &[a, b]);
                let xc = self.use_f(c, t);
                if na {
                    self.v2(VFNEG, t, xc);
                } else {
                    self.vmov(t, xc);
                }
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v3(if np { 0x4EA0_CC00 } else { 0x4E20_CC00 }, t, xa, xb);
                let xd = self.def_f(d, FS2);
                self.vmov(xd, t);
                self.fin_f(d, xd);
            }
            Inst::VSplat(d, s) => {
                let xd = self.def_f(d, FS0);
                if self.cls(s) == Cls::I {
                    let rs = self.use_i(s, S0);
                    self.a.e(0x4E04_0C00 | (rs as u32) << 5 | xd as u32); // dup vd.4s, wn
                } else {
                    let xs = self.use_f(s, FS1);
                    self.a.e(0x4E04_0400 | (xs as u32) << 5 | xd as u32); // dup vd.4s, vn.s[0]
                }
                self.fin_f(d, xd);
            }
            Inst::VExtract(d, v, lane) => {
                let xv = self.use_f(v, FS0);
                let imm5 = ((lane as u32) << 3) | 4;
                if self.cls(d) == Cls::I {
                    let rd = self.def_i(d, S0);
                    self.a.e(0x0E00_3C00 | imm5 << 16 | (xv as u32) << 5 | rd as u32); // umov
                    self.a.extend(rd, rd, 32, true);
                    self.fin_i(d, rd);
                } else {
                    let xd = self.def_f(d, FS1);
                    self.a.e(0x5E00_0400 | imm5 << 16 | (xv as u32) << 5 | xd as u32); // dup sd, vn.s[l]
                    self.fin_f(d, xd);
                }
            }
            Inst::VInsert(d, v, lane, s) => {
                let t = if self.cls(s) == Cls::I { self.vtmp(d, &[]) } else { self.vtmp(d, &[s]) };
                let xv = self.use_f(v, t);
                self.vmov(t, xv);
                let imm5 = ((lane as u32) << 3) | 4;
                if self.cls(s) == Cls::I {
                    let rs = self.use_i(s, S0);
                    self.a.e(0x4E00_1C00 | imm5 << 16 | (rs as u32) << 5 | t as u32); // ins vd.s[l], wn
                } else {
                    let xs = self.use_f(s, FS1);
                    self.a.e(0x6E00_0400 | imm5 << 16 | (xs as u32) << 5 | t as u32); // ins vd.s[l], vn.s[0]
                }
                let xd = self.def_f(d, FS2);
                self.vmov(xd, t);
                self.fin_f(d, xd);
            }
            Inst::VLoad(d, base, off) => {
                let rb = self.use_i(base, S1);
                let xd = self.def_f(d, FS0);
                self.vldst(true, xd, rb, off as i64);
                self.fin_f(d, xd);
            }
            Inst::VStore(base, off, v) => {
                let rb = self.use_i(base, S1);
                let xv = self.use_f(v, FS0);
                self.vldst(false, xv, rb, off as i64);
            }
            Inst::VShift(op, d, a, k) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                let k = (k & 31) as u32;
                if k == 0 {
                    self.vmov(xd, xa);
                } else {
                    let w = match op {
                        VOp::Shl => 0x4F00_5400 | (32 + k) << 16,  // shl
                        VOp::ShrS => 0x4F00_0400 | (64 - k) << 16, // sshr
                        _ => 0x6F00_0400 | (64 - k) << 16,         // ushr
                    };
                    self.a.e(w | (xa as u32) << 5 | xd as u32);
                }
                self.fin_f(d, xd);
            }
            Inst::VConst(d, bits) => {
                let xd = self.def_f(d, FS0);
                if let Some(w) = vimm(bits) {
                    self.a.e(w | xd as u32);
                } else if self.long {
                    // dup vd.4s, w16
                    self.a.mov_imm(IP0, bits as i64);
                    self.a.e(0x4E04_0C00 | (IP0 as u32) << 5 | xd as u32);
                } else {
                    // ldr qd, =bits x4 (vector literal pool after the code)
                    let at = self.a.pos();
                    self.a.e(0x9C00_0000 | xd as u32);
                    self.vpool.push((at, bits));
                }
                self.fin_f(d, xd);
            }
            Inst::VGather(d, base, idx) => {
                // per lane: umov w17, vi.s[l]; ldr w16, [base, x17, lsl #2]; ins v.s[l], w16
                let rb = self.use_i(base, S1);
                let xi = self.use_f(idx, FS0);
                for l in 0..4u32 {
                    let imm5 = (l << 3) | 4;
                    self.a.e(0x0E00_3C00 | imm5 << 16 | (xi as u32) << 5 | IP1 as u32);
                    self.a.e(0xB860_7800 | (IP1 as u32) << 16 | (rb as u32) << 5 | IP0 as u32);
                    self.a.e(0x4E00_1C00 | imm5 << 16 | (IP0 as u32) << 5 | FS2 as u32);
                }
                let xd = self.def_f(d, FS0);
                self.vmov(xd, FS2);
                self.fin_f(d, xd);
            }
            Inst::VAny(d, v) => {
                let xv = self.use_f(v, FS0);
                self.a.e(0x6EB0_A800 | (xv as u32) << 5 | FS2 as u32); // umaxv s, v.4s
                let rd = self.def_i(d, S0);
                self.a.fmov_to_gp(false, rd, FS2);
                self.cmp_imm(rd, 0);
                self.a.cset(rd, NE);
                self.fin_i(d, rd);
            }
            _ => {}
        }
    }

    /// `t = a <<|>> k; d = x op t` (t used once) as one shifted-register instruction. None: no match.
    fn shifted_operand(&mut self, options: &relax_session::Options, sh: &Inst, next: &Inst) -> Option<()> {
        let (t, a, k, kind) = match sh {
            Inst::IBinI(IOp::Shl, it, t, a, k) if it.bits == 64 && (1..64).contains(k) => (*t, *a, *k as u32, 0u32),
            Inst::IBinI(IOp::Shr, it, t, a, k) if it.bits == 64 && (1..64).contains(k) => (*t, *a, *k as u32, if it.signed { 2 } else { 1 }),
            _ => return None,
        };
        if self.al.uses[t.0 as usize] != 1 || isel_flags(options) & A64_FUSE == 0 {
            return None;
        }
        let (op, d, x) = match next {
            Inst::IBin(op @ (IOp::Add | IOp::Sub | IOp::And | IOp::Or | IOp::Xor), it, d, x, y) if it.bits == 64 && *y == t && *x != t => (*op, *d, *x),
            Inst::IBin(op @ (IOp::Add | IOp::And | IOp::Or | IOp::Xor), it, d, x, y) if it.bits == 64 && *x == t && *y != t => (*op, *d, *y),
            _ => return None,
        };
        let base = match op {
            IOp::Add => 0x8B00_0000u32,
            IOp::Sub => 0xCB00_0000,
            IOp::And => 0x8A00_0000,
            IOp::Or => 0xAA00_0000,
            _ => 0xCA00_0000,
        };
        let rx = self.use_i(x, S0);
        let ra = self.use_i(a, S1);
        let rd = self.def_i(d, S0);
        self.a.e(base | kind << 22 | (ra as u32) << 16 | k << 10 | (rx as u32) << 5 | rd as u32);
        self.fin_i(d, rd);
        Some(())
    }

    /// Sets the flags for a fused compare and returns the condition for "true".
    fn cmp_flags(&mut self, i: &Inst) -> u8 {
        match i {
            Inst::ICmp(c, signed, _, a, b) => {
                let ra = self.use_i(*a, S0);
                let rb = self.use_i(*b, S1);
                self.a.cmp(ra, rb);
                icc(*c, *signed)
            }
            Inst::ICmpI(c, signed, _, a, v) => {
                let ra = self.use_i(*a, S0);
                self.cmp_imm(ra, *v);
                icc(*c, *signed)
            }
            Inst::FCmp(c, f64_, _, a, b) => {
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                self.a.fcmp(*f64_, xa, xb);
                fcc(*c)
            }
            _ => NE,
        }
    }

    fn cmp_imm(&mut self, ra: u8, v: i64) {
        if (0..4096).contains(&v) {
            // cmp xn, #imm (subs xzr)
            self.a.e(0xF100_001F | (v as u32) << 10 | (ra as u32) << 5);
        } else if (-4095..0).contains(&v) {
            // cmn xn, #-imm
            self.a.e(0xB100_001F | ((-v) as u32) << 10 | (ra as u32) << 5);
        } else {
            self.a.mov_imm(S1, v);
            self.a.cmp(ra, S1);
        }
    }

    /// A conditional branch to a block: short (imm19), or in long mode the inverted condition
    /// skipping a `b` to the block.
    fn cond_branch(&mut self, c: CondBr, target: u32) {
        if !self.long {
            let p = match c {
                CondBr::B(cc) => self.a.bcond(cc),
                CondBr::Z(r) => self.a.cbz(r),
                CondBr::Nz(r) => self.a.cbnz(r),
            };
            self.fixups.push((p, target));
            return;
        }
        let skip = match c {
            CondBr::B(cc) => self.a.bcond(cc ^ 1),
            CondBr::Z(r) => self.a.cbnz(r),
            CondBr::Nz(r) => self.a.cbz(r),
        };
        let p = self.a.b();
        self.fixups.push((p, target));
        let here = self.a.pos();
        self.a.patch(skip, here);
    }

    fn jump_to(&mut self, target: u32, bi: usize) {
        if self.next_block[bi] != target {
            let p = self.a.b();
            self.fixups.push((p, target));
        }
    }

    /// Saves (prologue) or restores the used callee-saved registers at [fp - 8k], k = 1.. (ints
    /// first, then floats: the unwinder's layout), two per stp / ldp where the classes match.
    fn callee_saves(&mut self, save: bool) {
        let mut regs: Vec<(bool, u8)> = self.al.used_callee_int.iter().map(|r| (false, *r)).collect();
        regs.extend(self.al.used_callee_flt.iter().map(|x| (true, *x)));
        let mut k = 0usize;
        while k < regs.len() {
            let (fp, r) = regs[k];
            if k + 1 < regs.len() && regs[k + 1].0 == fp {
                // [fp - 8(k+2)] holds regs[k+1], [fp - 8(k+1)] regs[k]
                let off = -8 * (k as i32 + 2);
                if save {
                    self.a.stp(fp, regs[k + 1].1, r, FP, off);
                } else {
                    self.a.ldp(fp, regs[k + 1].1, r, FP, off);
                }
                k += 2;
            } else {
                let op = match (fp, save) {
                    (false, true) => STRX,
                    (false, false) => LDRX,
                    (true, true) => STRD,
                    (true, false) => LDRD,
                };
                self.a.ldst(op, 8, r, FP, -8 * (k as i64 + 1));
                k += 1;
            }
        }
    }

    fn epilogue(&mut self) {
        self.callee_saves(false);
        self.a.mov_sp(SP, FP);
        self.a.ldp_post(FP, LR, SP, 16);
        self.a.ret();
    }

    fn term(&mut self, t: &Term, bi: usize, fused: Option<Inst>) {
        match t {
            Term::Jump(target) => self.jump_to(*target, bi),
            Term::Branch(c, tt, ff) => {
                let next = self.next_block[bi];
                if let Some((x, min, targets, default)) = self.jt_plan.get(&(bi as u32)).cloned() {
                    // a chain of compares of x with constants: one table dispatch
                    // t = x - min; if t > range { default }; br table[t]
                    let r = self.use_i(x, S0);
                    self.a.mov_imm(IP0, min);
                    self.a.rrr(0xCB00_0000, IP0, r, IP0); // sub x16, r, x16
                    self.a.mov_imm(IP1, targets.len() as i64 - 1);
                    self.a.cmp(IP0, IP1);
                    self.cond_branch(CondBr::B(HI), default);
                    self.a.e(0x1000_0000 | (4 << 5) | IP1 as u32); // adr x17, .+16
                    self.a.e(0xB8A0_7800 | (IP0 as u32) << 16 | (IP1 as u32) << 5 | IP0 as u32); // ldrsw x16, [x17, x16, lsl #2]
                    self.a.rrr(0x8B00_0000, IP1, IP1, IP0); // add x17, x17, x16
                    self.a.e(0xD61F_0000 | (IP1 as u32) << 5); // br x17
                    let start = self.a.pos();
                    for t in targets {
                        let at = self.a.pos();
                        self.a.e(0);
                        self.jt_fix.push((at, start, t));
                    }
                    return;
                }
                match fused {
                    // and with a bitmask immediate, compared with zero: tst + b.cond
                    Some(Inst::ICmpI(cnd @ (Cond::Eq | Cond::Ne), _, _, _, 0)) if self.test_fuse.is_some() => {
                        let (src, mask) = self.test_fuse.take().unwrap();
                        let rs = self.use_i(src, S0);
                        let (nn, immr, imms) = bitmask_imm(mask as u64).unwrap();
                        // ANDS xzr, xs, #mask
                        self.a.e(0xF200_001F | nn << 22 | immr << 16 | imms << 10 | (rs as u32) << 5);
                        let cc = if cnd == Cond::Eq { EQ } else { NE };
                        if *tt == next {
                            self.cond_branch(CondBr::B(cc ^ 1), *ff);
                        } else {
                            self.cond_branch(CondBr::B(cc), *tt);
                            self.jump_to(*ff, bi);
                        }
                    }
                    // compare with zero for (in)equality: cbz / cbnz, no flags
                    Some(Inst::ICmpI(cnd @ (Cond::Eq | Cond::Ne), _, _, a, 0)) => {
                        let r = self.use_i(a, S0);
                        // branch to tt when (r == 0) == (cnd == Eq)
                        let zero_to_tt = cnd == Cond::Eq;
                        if *tt == next {
                            self.cond_branch(if zero_to_tt { CondBr::Nz(r) } else { CondBr::Z(r) }, *ff);
                        } else {
                            self.cond_branch(if zero_to_tt { CondBr::Z(r) } else { CondBr::Nz(r) }, *tt);
                            self.jump_to(*ff, bi);
                        }
                    }
                    Some(i) => {
                        let cc = self.cmp_flags(&i);
                        if *tt == next {
                            // the condition codes come in true/false pairs: cc ^ 1 negates
                            self.cond_branch(CondBr::B(cc ^ 1), *ff);
                        } else {
                            self.cond_branch(CondBr::B(cc), *tt);
                            self.jump_to(*ff, bi);
                        }
                    }
                    None => {
                        let r = self.use_i(*c, S0);
                        if *tt == next {
                            self.cond_branch(CondBr::Z(r), *ff);
                        } else {
                            self.cond_branch(CondBr::Nz(r), *tt);
                            self.jump_to(*ff, bi);
                        }
                    }
                }
            }
            Term::Ret(vals) => {
                self.c_ret_parts();
                let mut mv = Vec::new();
                let mut ni = 0u8;
                let mut nf = 0u8;
                for v in vals {
                    if self.cls(*v) == Cls::I {
                        mv.push((MLoc::R(ni), self.mloc(*v)));
                        ni += 1;
                    } else {
                        mv.push((MLoc::F(nf), self.mloc(*v)));
                        nf += 1;
                    }
                }
                self.moves(&mv);
                self.epilogue();
            }
            Term::Unreachable => self.a.udf(),
            Term::Invoke(normal, unwind) => {
                // the call was the block's last instruction: unwinding returns to a landing
                // stub (see gen) that reloads what the call's normal path reloads
                let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
                self.pads.push((self.last_call_ret, saves, *unwind));
                self.jump_to(*normal, bi);
            }
            Term::Resume => {
                self.ldr_reloc(IP0, Reloc::Resume);
                self.a.blr(IP0);
                self.a.udf();
            }
        }
    }

    fn inst(&mut self, options: &relax_session::Options, i: &Inst) {
        match i {
            // recomputed at each use (Alloc::remat)
            Inst::Iconst(d, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::Mov(d, _) if self.al.remat[d.0 as usize].is_some() => {}
            Inst::Iconst(d, v) => {
                let r = self.def_i(*d, S0);
                self.a.mov_imm(r, *v);
                self.fin_i(*d, r);
            }
            Inst::Fconst(d, bits, f64_) => {
                let x = self.def_f(*d, FS0);
                if *bits == 0 {
                    self.a.fmov_from_gp(true, x, ZR);
                } else if isel_flags(options) & A64_FCONST == 0 {
                    self.a.mov_imm(IP0, *bits as i64);
                    self.a.fmov_from_gp(*f64_, x, IP0);
                } else if let Some(imm8) = fp_imm8(*bits, *f64_) {
                    // fmov dd/sd, #imm
                    self.a.e(0x1E20_1000 | if *f64_ { DBL } else { 0 } | imm8 << 13 | x as u32);
                } else if *f64_ && !self.long {
                    // ldr dd, =bits (literal pool after the code)
                    let at = self.a.pos();
                    self.a.e(0x5C00_0000 | x as u32);
                    self.fpool.push((at, *bits));
                } else {
                    self.a.mov_imm(IP0, *bits as i64);
                    self.a.fmov_from_gp(*f64_, x, IP0);
                }
                self.fin_f(*d, x);
            }
            // pieces of one split value sharing a slot
            Inst::Mov(d, s) if matches!(self.al.loc[d.0 as usize], Loc::Stack(_)) && self.al.loc[d.0 as usize] == self.al.loc[s.0 as usize] => {}
            Inst::Mov(d, s) => {
                if self.cls(*d) == Cls::I && matches!((self.al.loc[d.0 as usize], self.al.loc[s.0 as usize]), (Loc::Stack(_), Loc::Reg(_)) | (Loc::Reg(_), Loc::Stack(_))) {
                    // straight between a register and a spill slot
                    match (self.al.loc[d.0 as usize], self.al.loc[s.0 as usize]) {
                        (Loc::Stack(o), Loc::Reg(r)) => {
                            let off = self.spill_sp(o);
                            self.a.ldst(STRX, 8, r, SP, off);
                        }
                        (Loc::Reg(r), Loc::Stack(o)) => {
                            let off = self.spill_sp(o);
                            self.a.ldst(LDRX, 8, r, SP, off);
                        }
                        _ => {}
                    }
                } else if self.cls(*d) == Cls::I {
                    let rs = self.use_i(*s, S0);
                    let rd = self.def_i(*d, S0);
                    self.a.mov(rd, rs);
                    self.fin_i(*d, rd);
                } else {
                    let xs = self.use_f(*s, FS0);
                    let xd = self.def_f(*d, FS0);
                    if self.cls(*d) == Cls::V128 {
                        self.vmov(xd, xs);
                    } else {
                        self.a.fmov(xd, xs);
                    }
                    self.fin_f(*d, xd);
                }
            }
            Inst::IBin(op, it, d, a, b) => {
                let ra = self.use_i(*a, S0);
                let rb = self.use_i(*b, S1);
                let rd = self.def_i(*d, S0);
                self.ibin_rr(*op, *it, rd, ra, rb);
                self.fin_i(*d, rd);
            }
            Inst::IBinI(op, it, d, a, imm) => self.ibin_imm(*op, *it, *d, *a, *imm),
            Inst::IBit(o, d, a) => {
                let ra = self.use_i(*a, S0);
                let rd = self.def_i(*d, S0);
                match o {
                    BitOp::Clz => self.a.e(0xDAC0_1000 | (ra as u32) << 5 | rd as u32),
                    BitOp::Bswap => self.a.e(0xDAC0_0C00 | (ra as u32) << 5 | rd as u32),
                    BitOp::Rev => self.a.e(0xDAC0_0000 | (ra as u32) << 5 | rd as u32),
                    BitOp::Ctz => {
                        // rbit + clz
                        self.a.e(0xDAC0_0000 | (ra as u32) << 5 | rd as u32);
                        self.a.e(0xDAC0_1000 | (rd as u32) << 5 | rd as u32);
                    }
                    BitOp::Popcnt => {
                        // fmov d, x; cnt v.8b; addv b, v.8b; fmov w, s
                        self.a.fmov_from_gp(true, FS2, ra);
                        self.a.e(0x0E20_5800 | (FS2 as u32) << 5 | FS2 as u32);
                        self.a.e(0x0E31_B800 | (FS2 as u32) << 5 | FS2 as u32);
                        self.a.fmov_to_gp(false, rd, FS2);
                    }
                }
                self.fin_i(*d, rd);
            }
            Inst::INeg(it, d, a) | Inst::INot(it, d, a) => {
                let ra = self.use_i(*a, S0);
                let rd = self.def_i(*d, S0);
                if matches!(i, Inst::INeg(..)) {
                    self.a.rrr(0xCB00_0000, rd, ZR, ra);
                } else {
                    self.a.rrr(0xAA20_0000, rd, ZR, ra);
                }
                self.a.extend(rd, rd, it.bits, it.signed);
                self.fin_i(*d, rd);
            }
            Inst::ICmp(c, signed, d, a, b) => {
                let ra = self.use_i(*a, S0);
                let rb = self.use_i(*b, S1);
                self.a.cmp(ra, rb);
                let rd = self.def_i(*d, S0);
                self.a.cset(rd, icc(*c, *signed));
                self.fin_i(*d, rd);
            }
            Inst::ICmpI(c, signed, d, a, imm) => {
                let ra = self.use_i(*a, S0);
                self.cmp_imm(ra, *imm);
                let rd = self.def_i(*d, S0);
                self.a.cset(rd, icc(*c, *signed));
                self.fin_i(*d, rd);
            }
            Inst::FBin(op @ (FOp::MinLt | FOp::MaxGt), f64_, d, a, b) => {
                // a < b ? a : b  (fcmp; fcsel mi / gt: unordered picks b)
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                self.a.fcmp(*f64_, xa, xb);
                let xd = self.def_f(*d, FS0);
                let cond = if *op == FOp::MinLt { MI } else { GT };
                self.a.e(0x1E20_0C00 | if *f64_ { DBL } else { 0 } | (xb as u32) << 16 | (cond as u32) << 12 | (xa as u32) << 5 | xd as u32);
                self.fin_f(*d, xd);
            }
            Inst::FBin(op, f64_, d, a, b) => {
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                let xd = self.def_f(*d, FS0);
                let opc = match op {
                    FOp::Add => FADD,
                    FOp::Sub => FSUB,
                    FOp::Mul => FMUL,
                    FOp::Div => FDIV,
                    FOp::Min => FMINNM,
                    FOp::Max => FMAXNM,
                    FOp::MinLt | FOp::MaxGt => FMINNM,
                };
                self.a.fp3(opc, *f64_, xd, xa, xb);
                self.fin_f(*d, xd);
            }
            Inst::FUnary(op, f64_, d, a) => {
                let xa = self.use_f(*a, FS0);
                let xd = self.def_f(*d, FS0);
                let opc = match op {
                    FUn::Neg => FNEG,
                    FUn::Abs => FABS,
                    FUn::Sqrt => FSQRT,
                    FUn::Floor => FRINTM,
                    FUn::Ceil => FRINTP,
                    FUn::Trunc => FRINTZ,
                    FUn::RoundEven => FRINTN,
                    FUn::RoundAway => FRINTA,
                };
                self.a.fp2(opc, *f64_, xd, xa);
                self.fin_f(*d, xd);
            }
            Inst::FCmp(c, f64_, d, a, b) => {
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                self.a.fcmp(*f64_, xa, xb);
                let rd = self.def_i(*d, S0);
                self.a.cset(rd, fcc(*c));
                self.fin_i(*d, rd);
            }
            Inst::Conv(c, d, s) => self.conv(*c, *d, *s),
            Inst::LoadX(m, d, base, idx, sh, off) => {
                let (op, size) = mem_op(*m, true);
                let ri = self.use_i(*idx, S0);
                let (rn, rm, o) = self.index_addr(*base, ri, *sh, *off, size);
                match m {
                    Mem::Int(..) => {
                        let rd = self.def_i(*d, S0);
                        self.ldst_x(op, size, rd, rn, rm, o);
                        self.fin_i(*d, rd);
                    }
                    _ => {
                        let xd = self.def_f(*d, FS0);
                        self.ldst_x(op, size, xd, rn, rm, o);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::StoreX(m, base, idx, sh, off, src) => {
                // the stored value first: reloading a spilled one from a far stack slot uses
                // ip1 as its offset register, which then holds the address (frames > 32 KB:
                // metal draw_pass wrote paint_dirty through a stack offset)
                let (op, size) = mem_op(*m, false);
                let rs = match m {
                    Mem::Int(..) => self.use_i(*src, S0),
                    _ => self.use_f(*src, FS0),
                };
                let ri = self.use_i(*idx, IP0);
                let (rn, rm, o) = self.index_addr(*base, ri, *sh, *off, size);
                self.ldst_x(op, size, rs, rn, rm, o);
            }
            Inst::Load(m, d, base, off) => {
                let (rb, off) = match self.slot_base(*base) {
                    Some(so) => (SP, &((so + *off as i64) as i32)),
                    None => (self.use_i(*base, S1), off),
                };
                let (op, size) = mem_op(*m, true);
                match m {
                    Mem::Int(..) => {
                        let rd = self.def_i(*d, S0);
                        self.a.ldst(op, size, rd, rb, *off as i64);
                        self.fin_i(*d, rd);
                    }
                    _ => {
                        let xd = self.def_f(*d, FS0);
                        self.a.ldst(op, size, xd, rb, *off as i64);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::Store(m, base, off, src) => {
                let (rb, off) = match self.slot_base(*base) {
                    Some(so) => (SP, &((so + *off as i64) as i32)),
                    None => (self.use_i(*base, S1), off),
                };
                let (op, size) = mem_op(*m, false);
                match m {
                    Mem::Int(..) => {
                        let rs = self.use_i(*src, S0);
                        self.a.ldst(op, size, rs, rb, *off as i64);
                    }
                    _ => {
                        let xs = self.use_f(*src, FS0);
                        self.a.ldst(op, size, xs, rb, *off as i64);
                    }
                }
            }
            Inst::SlotAddr(d, s) => {
                let rd = self.def_i(*d, S0);
                let off = self.slot_off[*s as usize];
                self.a.add_any(rd, SP, off);
                self.fin_i(*d, rd);
            }
            Inst::Addr(d, sym) => {
                let rd = self.def_i(*d, S0);
                if SymClass::of(self.sym_class, sym.id) == SymClass::Domain {
                    self.dom_addr(rd, *sym);
                } else {
                    self.ldr_reloc(rd, Reloc::Sym(*sym));
                }
                self.fin_i(*d, rd);
            }
            Inst::FnAddr(d, id) => {
                let rd = self.def_i(*d, S0);
                self.ldr_reloc(rd, Reloc::Thunk(*id));
                self.fin_i(*d, rd);
            }
            Inst::Call(c, args, rets) => self.call(c, args, rets),
            Inst::Copy(dst, src, size) if *size > 256 => {
                // x8 = dst, x17 = src, x16 = 16-byte chunks: ldp/stp post-index loop
                let rd = self.use_i(*dst, S0);
                self.a.mov(X8, rd);
                let rs = self.use_i(*src, S1);
                self.a.mov(IP1, rs);
                let size = *size as i64;
                self.a.mov_imm(IP0, size / 16);
                let top = self.a.pos();
                self.a.ldp_post(S0, S1, IP1, 16);
                // stp x14, x15, [x8], #16
                self.a.e(0xA880_0000 | (2u32 << 15) | (S1 as u32) << 10 | (X8 as u32) << 5 | S0 as u32);
                // subs x16, x16, #1 ; b.ne top
                self.a.e(0xF100_0400 | (IP0 as u32) << 5 | IP0 as u32);
                let p = self.a.bcond(NE);
                self.a.patch(p, top);
                let mut o = 0i64;
                let rest = size % 16;
                while o + 8 <= rest {
                    self.a.ldst(LDRX, 8, S0, IP1, o);
                    self.a.ldst(STRX, 8, S0, X8, o);
                    o += 8;
                }
                while o < rest {
                    self.a.ldst(LDRB, 1, S0, IP1, o);
                    self.a.ldst(STRB, 1, S0, X8, o);
                    o += 1;
                }
            }
            Inst::Copy(dst, src, size) => {
                let rd = self.use_i(*dst, S0);
                let rs = self.use_i(*src, S1);
                let size = *size as i64;
                let mut o = 0i64;
                while o + 8 <= size {
                    self.a.ldst(LDRX, 8, IP0, rs, o);
                    self.a.ldst(STRX, 8, IP0, rd, o);
                    o += 8;
                }
                while o + 4 <= size {
                    self.a.ldst(LDRW, 4, IP0, rs, o);
                    self.a.ldst(STRW, 4, IP0, rd, o);
                    o += 4;
                }
                while o < size {
                    self.a.ldst(LDRB, 1, IP0, rs, o);
                    self.a.ldst(STRB, 1, IP0, rd, o);
                    o += 1;
                }
            }
            Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) => self.atomic(i),
            Inst::TlsAddr(d, off) => self.tls_addr(*d, *off),
            Inst::Cpuid(..) | Inst::Xgetbv(..) => unreachable!("x86 CPU query rejected before arm64 emission"),
            Inst::FpEnv(true, x) => {
                // msr fpcr, xt
                let rx = self.use_i(*x, IP0);
                self.a.e(0xD51B_4400 | rx as u32);
            }
            Inst::FpEnv(false, d) => {
                // mrs xt, fpcr
                let rd = self.def_i(*d, IP0);
                self.a.e(0xD53B_4400 | rd as u32);
                self.fin_i(*d, rd);
            }
            Inst::VBin(..) | Inst::VUn(..) | Inst::VSel(..) | Inst::VFma(..) | Inst::VSplat(..) | Inst::VExtract(..) | Inst::VInsert(..) | Inst::VLoad(..) | Inst::VStore(..) | Inst::VAny(..) | Inst::VGather(..) | Inst::VConst(..) | Inst::VShift(..) => self.vinst(i),
            Inst::VX(op, d, a, b) => self.vx(*op, *d, *a, *b),
            Inst::VX3(op, d, a, b, c) => self.vx3(*op, *d, *a, *b, *c),
            Inst::Sel(d, c, a, b) => {
                let rc = self.use_i(*c, S0);
                self.cmp_imm(rc, 0);
                if self.cls(*d) == Cls::I {
                    let ra = self.use_i(*a, S0);
                    let rb = self.use_i(*b, S1);
                    let rd = self.def_i(*d, S0);
                    self.a.csel(rd, ra, rb, NE);
                    self.fin_i(*d, rd);
                } else {
                    let w = self.cls(*d) == Cls::F64;
                    let xa = self.use_f(*a, FS0);
                    let xb = self.use_f(*b, FS1);
                    let xd = self.def_f(*d, FS0);
                    self.a.e(0x1E20_0C00 | if w { DBL } else { 0 } | (xb as u32) << 16 | (NE as u32) << 12 | (xa as u32) << 5 | xd as u32);
                    self.fin_f(*d, xd);
                }
            }
            Inst::FMulAdd(np, na, w, d, a, b, c) => {
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                let xc = self.use_f(*c, FS2);
                let xd = self.def_f(*d, FS0);
                // fmadd c + ab, fmsub c - ab, fnmsub ab - c, fnmadd -ab - c
                let base = match (np, na) {
                    (false, false) => 0x1F00_0000,
                    (true, false) => 0x1F00_8000,
                    (false, true) => 0x1F20_8000,
                    (true, true) => 0x1F20_0000,
                };
                self.a.e(base | if *w { DBL } else { 0 } | (xb as u32) << 16 | (xc as u32) << 10 | (xa as u32) << 5 | xd as u32);
                self.fin_f(*d, xd);
            }
        }
    }

    fn ibin_rr(&mut self, op: IOp, it: IntTy, rd: u8, ra: u8, rb: u8) {
        if op == IOp::Rotl {
            // rotate left by b = rorv by -b (rorv takes the count mod the width; W form for u32)
            self.a.e(0xCB00_03E0 | (rb as u32) << 16 | IP0 as u32);
            self.a.rrr(if it.bits == 32 { 0x1AC0_2C00 } else { 0x9AC0_2C00 }, rd, ra, IP0);
            return;
        }
        if it == U32 && !matches!(op, IOp::Div | IOp::Rem | IOp::MulHi) {
            // W forms zero-extend for free (shift amounts are taken mod 32)
            let w = match op {
                IOp::Add => 0x0B00_0000,
                IOp::Sub => 0x4B00_0000,
                IOp::Mul => 0x1B00_7C00,
                IOp::And => 0x0A00_0000,
                IOp::Or => 0x2A00_0000,
                IOp::Xor => 0x4A00_0000,
                IOp::Shl => 0x1AC0_2000,
                _ => 0x1AC0_2400, // lsrv
            };
            self.a.rrr(w, rd, ra, rb);
            return;
        }
        match op {
            IOp::Add => self.a.rrr(0x8B00_0000, rd, ra, rb),
            IOp::Sub => self.a.rrr(0xCB00_0000, rd, ra, rb),
            IOp::Mul => self.a.rrr(0x9B00_7C00, rd, ra, rb),
            IOp::And => self.a.rrr(0x8A00_0000, rd, ra, rb),
            IOp::Or => self.a.rrr(0xAA00_0000, rd, ra, rb),
            IOp::Xor => self.a.rrr(0xCA00_0000, rd, ra, rb),
            IOp::Shl => self.a.rrr(0x9AC0_2000, rd, ra, rb),
            IOp::Shr => self.a.rrr(if it.signed { 0x9AC0_2800 } else { 0x9AC0_2400 }, rd, ra, rb),
            IOp::Div => self.a.rrr(if it.signed { 0x9AC0_0C00 } else { 0x9AC0_0800 }, rd, ra, rb),
            // smulh / umulh
            IOp::MulHi => self.a.rrr(if it.signed { 0x9B40_7C00 } else { 0x9BC0_7C00 }, rd, ra, rb),
            // handled at the top
            IOp::Rotl => {}
            IOp::Rem => {
                // q = a / b; d = a - q * b  (rd may alias ra or rb: compute q in IP0)
                self.a.rrr(if it.signed { 0x9AC0_0C00 } else { 0x9AC0_0800 }, IP0, ra, rb);
                // msub rd, ip0, rb, ra
                self.a.e(0x9B00_8000 | (rb as u32) << 16 | (ra as u32) << 10 | (IP0 as u32) << 5 | rd as u32);
            }
        }
        if it.bits < 64 && !matches!(op, IOp::And | IOp::Or | IOp::Xor) {
            self.a.extend(rd, rd, it.bits, it.signed);
        }
    }

    fn ibin_imm(&mut self, op: IOp, it: IntTy, d: VReg, a: VReg, imm: i64) {
        let ra = self.use_i(a, S0);
        let rd = self.def_i(d, S0);
        if op == IOp::Rotl {
            // rotate left by k = ror by (w - k) = extr rd, ra, ra, #(w - k)
            let w = if it.bits == 32 { 32u32 } else { 64 };
            let r = (w - (imm as u32 % w)) % w;
            let base = if w == 32 { 0x1380_0000u32 } else { 0x93C0_0000 };
            self.a.e(base | (ra as u32) << 16 | r << 10 | (ra as u32) << 5 | rd as u32);
            self.fin_i(d, rd);
            return;
        }
        if it == U32 && matches!(op, IOp::Shl | IOp::Shr) {
            // lsl / lsr w by an immediate (ubfm w forms) zero-extend
            let s = (imm & 31) as u32;
            if op == IOp::Shl {
                self.a.e(0x5300_0000 | ((32 - s) & 31) << 16 | (31 - s) << 10 | (ra as u32) << 5 | rd as u32);
            } else {
                self.a.e(0x5300_0000 | s << 16 | 31 << 10 | (ra as u32) << 5 | rd as u32);
            }
            self.fin_i(d, rd);
            return;
        }
        if it == U32 && matches!(op, IOp::Add | IOp::Sub) && (0..4096).contains(&imm) {
            self.a.e(if op == IOp::Add { 0x1100_0000 } else { 0x5100_0000 } | (imm as u32) << 10 | (ra as u32) << 5 | rd as u32);
            self.fin_i(d, rd);
            return;
        }
        let mut done = true;
        match op {
            IOp::Add | IOp::Sub => {
                let v = if op == IOp::Sub { imm.wrapping_neg() } else { imm };
                if v.unsigned_abs() < (1 << 24) {
                    self.a.add_any(rd, ra, v);
                } else {
                    done = false;
                }
            }
            IOp::Mul if imm > 0 && (imm & (imm - 1)) == 0 => {
                let s = imm.trailing_zeros();
                self.a.ubfm(rd, ra, (64 - s) & 63, 63 - s);
            }
            IOp::Shl => {
                let s = (imm & 63) as u32;
                self.a.ubfm(rd, ra, (64 - s) & 63, 63 - s);
            }
            IOp::Shr => {
                let s = (imm & 63) as u32;
                if it.signed {
                    self.a.sbfm(rd, ra, s, 63);
                } else {
                    self.a.ubfm(rd, ra, s, 63);
                }
            }
            IOp::And if imm == 0xff || imm == 0xffff || imm == 0xffff_ffff => {
                let bits = match imm {
                    0xff => 8,
                    0xffff => 16,
                    _ => 32,
                };
                self.a.extend(rd, ra, bits, false);
            }
            // and / orr / eor with a bitmask immediate (64-bit values, or u32 ones without high bits)
            IOp::And | IOp::Or | IOp::Xor if (it.bits == 64 || (it == U32 && (0..1i64 << 32).contains(&imm))) && bitmask_imm(imm as u64).is_some() => {
                let (n, immr, imms) = bitmask_imm(imm as u64).unwrap();
                let base = match op {
                    IOp::And => 0x9200_0000u32,
                    IOp::Or => 0xB200_0000,
                    _ => 0xD200_0000,
                };
                self.a.e(base | n << 22 | immr << 16 | imms << 10 | (ra as u32) << 5 | rd as u32);
            }
            _ => done = false,
        }
        if !done {
            self.a.mov_imm(S1, imm);
            self.ibin_rr(op, it, rd, ra, S1);
            self.fin_i(d, rd);
            return;
        }
        if it.bits < 64 && !matches!(op, IOp::And | IOp::Or | IOp::Xor) {
            self.a.extend(rd, rd, it.bits, it.signed);
        }
        self.fin_i(d, rd);
    }

    fn conv(&mut self, c: Conv, d: VReg, s: VReg) {
        match c {
            Conv::IntToInt(it) => {
                let rs = self.use_i(s, S0);
                let rd = self.def_i(d, S0);
                self.a.extend(rd, rs, it.bits, it.signed);
                self.fin_i(d, rd);
            }
            Conv::IntToF(from, to64) => {
                let rs = self.use_i(s, S0);
                let xd = self.def_f(d, FS0);
                // registers hold values extended to 64 bits, so a 64-bit convert is exact;
                // u64 needs the unsigned form
                let op = match (from.bits == 64 && !from.signed, to64) {
                    (true, true) => 0x9E63_0000,
                    (true, false) => 0x9E23_0000,
                    (false, true) => 0x9E62_0000,
                    (false, false) => 0x9E22_0000,
                };
                self.a.e(op | (rs as u32) << 5 | xd as u32);
                self.fin_f(d, xd);
            }
            Conv::FToInt(from64, it) => {
                // fcvtz[su] saturates and maps NaN to 0: Rust `as` semantics for 32/64 bits
                let xs = self.use_f(s, FS0);
                let rd = self.def_i(d, S0);
                let src = if from64 { DBL } else { 0 };
                let uns = if it.signed { 0 } else { 0x0001_0000 };
                if it.bits == 64 {
                    self.a.e(0x9E38_0000 | src | uns | (xs as u32) << 5 | rd as u32);
                } else {
                    // 32-bit saturating convert (W form zero-extends into X)
                    self.a.e(0x1E38_0000 | src | uns | (xs as u32) << 5 | rd as u32);
                    if it.signed {
                        self.a.extend(rd, rd, 32, true);
                    }
                    if it.bits < 32 {
                        let (lo, hi): (i64, i64) = if it.signed {
                            (-(1i64 << (it.bits - 1)), (1i64 << (it.bits - 1)) - 1)
                        } else {
                            (0, (1i64 << it.bits) - 1)
                        };
                        self.a.mov_imm(IP0, hi);
                        self.a.cmp(rd, IP0);
                        self.a.csel(rd, IP0, rd, if it.signed { GT } else { HI });
                        if it.signed {
                            self.a.mov_imm(IP0, lo);
                            self.a.cmp(rd, IP0);
                            self.a.csel(rd, IP0, rd, LT);
                        }
                    }
                }
                self.fin_i(d, rd);
            }
            Conv::F32ToF64 => {
                let xs = self.use_f(s, FS0);
                let xd = self.def_f(d, FS0);
                self.a.e(0x1E22_C000 | (xs as u32) << 5 | xd as u32);
                self.fin_f(d, xd);
            }
            Conv::F64ToF32 => {
                let xs = self.use_f(s, FS0);
                let xd = self.def_f(d, FS0);
                self.a.e(0x1E62_4000 | (xs as u32) << 5 | xd as u32);
                self.fin_f(d, xd);
            }
            Conv::BitsToF(f64_) => {
                let rs = self.use_i(s, S0);
                let xd = self.def_f(d, FS0);
                self.a.fmov_from_gp(f64_, xd, rs);
                self.fin_f(d, xd);
            }
            Conv::FToBits(f64_) => {
                let xs = self.use_f(s, FS0);
                let rd = self.def_i(d, S0);
                // fmov wd, sn zero-extends: f32 bits as u32
                self.a.fmov_to_gp(f64_, rd, xs);
                self.fin_i(d, rd);
            }
        }
    }

    fn call(&mut self, c: &Callee, args: &[VReg], rets: &[VReg]) {
        if let Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig) = c {
            return self.c_call(c, sig, args, rets);
        }
        // save caller-saved values that live across this call
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            self.save_restore(*v, *slot, true);
        }
        // indirect target first: argument moves may overwrite its register
        if let Callee::Indirect(v) = c {
            let r = self.use_i(*v, IP0);
            self.a.mov(IP0, r);
        }
        let mut mv = Vec::new();
        let mut ni = 0usize;
        let mut nf = 0usize;
        let mut k = 0i64;
        for a in args {
            let is_int = self.cls(*a) == Cls::I;
            if is_int && ni < INT_ARGS {
                mv.push((MLoc::R(ni as u8), self.mloc(*a)));
                ni += 1;
            } else if !is_int && nf < FLT_ARGS {
                mv.push((MLoc::F(nf as u8), self.mloc(*a)));
                nf += 1;
            } else {
                // overflow argument: outgoing area [sp + 8k], stored before the register
                // moves overwrite argument registers
                match self.mloc(*a) {
                    MLoc::R(r) => self.a.ldst(STRX, 8, r, SP, 8 * k),
                    MLoc::F(x) => self.a.ldst(STRD, 8, x, SP, 8 * k),
                    MLoc::M(o) => {
                        self.a.ldst(LDRX, 8, S1, SP, o as i64);
                        self.a.ldst(STRX, 8, S1, SP, 8 * k);
                    }
                }
                k += 1;
            }
        }
        self.moves(&mv);
        match c {
            Callee::Fn(id) => {
                self.ldr_reloc(IP0, Reloc::Slot(*id));
                self.a.ldst(LDRX, 8, IP0, IP0, 0);
                self.a.blr(IP0);
            }
            Callee::Indirect(_) => self.a.blr(IP0),
            Callee::Host(sym) => {
                self.ldr_reloc(IP0, Reloc::Sym(*sym));
                self.a.blr(IP0);
            }
            Callee::CHost(..) | Callee::CIndirect(..) | Callee::CFn(..) => {}
        }
        self.last_call_ret = self.a.pos();
        let mut mv = Vec::new();
        let mut ri = 0u8;
        let mut rf = 0u8;
        for r in rets {
            if self.cls(*r) == Cls::I {
                if self.al.loc[r.0 as usize] != Loc::None {
                    mv.push((self.mloc(*r), MLoc::R(ri)));
                }
                ri += 1;
            } else {
                if self.al.loc[r.0 as usize] != Loc::None {
                    mv.push((self.mloc(*r), MLoc::F(rf)));
                }
                rf += 1;
            }
        }
        self.moves(&mv);
        for (v, slot) in &saves {
            self.save_restore(*v, *slot, false);
        }
    }
}

fn words_to_bytes(w: &[u32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(w.len() * 4);
    for x in w {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

fn count_classes(f: &Func, vs: &[VReg]) -> (usize, usize) {
    let mut ni = 0;
    let mut nf = 0;
    for v in vs {
        if f.vregs[v.0 as usize] == Cls::I {
            ni += 1;
        } else {
            nf += 1;
        }
    }
    (ni, nf)
}

pub fn compile(options: &relax_session::Options, f: &Func, env: &Env) -> Result<Compiled, String> {
    compile_freq(options, f, env, None)
}

/// research (ios-interp): MAKEPAD_RUN_INTERP_STATS=1 also sums the machine-code backend's time and
/// output for functions compiled natively, to compare with the interpreter's lowering
fn compile_freq_timed(options: &relax_session::Options, f: &Func, env: &Env, freq: Option<&[f32]>) -> Result<Compiled, String> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_INTERP_STATS").is_some()) {
        return compile_freq_inner(options, f, env, freq);
    }
    let t = std::time::Instant::now();
    let r = compile_freq_inner(options, f, env, freq);
    if let Ok(c) = &r {
        crate::interp::note_native(t.elapsed().as_nanos() as u64, c.code.len() as u64, f.blocks.iter().map(|b| b.insts.len() as u64 + 1).sum());
    }
    r
}

/// `compile` with measured block frequencies of `f` (0..=1 of its hottest block, per block), which
/// the register allocator weighs uses by instead of loop depth.
/// Chains of blocks testing one value against constants -- `c = x == k (or !=); branch c` with
/// nothing else in the block after the head, the frontend's form of a `match` on an integer --
/// with at least 6 distinct cases over a dense range: the head becomes one table dispatch to the
/// case targets (the chain's blocks stay for any other entries). Interpreter loops dispatch
/// through such matches (the Splash VM's opcode(): ~90 cases tested one by one).
pub(crate) fn jump_tables(options: &relax_session::Options, f: &Func, al: &Alloc) -> std::collections::HashMap<u32, (VReg, i64, Vec<u32>, u32)> {
    let mut out = std::collections::HashMap::new();
    if options.no_jump_tables {
        return out;
    }
    // (x, k, eq-target, else-target) of a block ending in a fused compare of x with k
    let shape = |bi: usize| -> Option<(VReg, i64, u32, u32)> {
        let b = &f.blocks[bi];
        let (c, t, e) = match b.term {
            Term::Branch(c, t, e) => (c, t, e),
            _ => return None,
        };
        match b.insts.last() {
            Some(Inst::ICmpI(cc @ (Cond::Eq | Cond::Ne), _, d, x, k)) if *d == c && al.uses[c.0 as usize] == 1 && *x != c => {
                if *cc == Cond::Eq {
                    Some((*x, *k, t, e))
                } else {
                    Some((*x, *k, e, t))
                }
            }
            _ => None,
        }
    };
    let nb = f.blocks.len();
    let shapes: Vec<Option<(VReg, i64, u32, u32)>> = (0..nb).map(shape).collect();
    // blocks that continue a chain (only the compare in them, reached as a previous link's else)
    let mut is_link = vec![false; nb];
    for bi in 0..nb {
        if let Some((x, _, _, e)) = shapes[bi] {
            let e = e as usize;
            if e != bi && f.blocks[e].insts.len() == 1 && matches!(shapes[e], Some((x2, ..)) if x2 == x) {
                is_link[e] = true;
            }
        }
    }
    for head in 0..nb {
        let (x, ..) = match shapes[head] {
            Some(s) if !is_link[head] => s,
            _ => continue,
        };
        let mut cases: Vec<(i64, u32)> = Vec::new();
        let mut cur = head;
        let mut seen = vec![head];
        let default;
        loop {
            let (_, k, t, e) = shapes[cur].unwrap();
            if !cases.iter().any(|c| c.0 == k) {
                cases.push((k, t));
            }
            let en = e as usize;
            if en != cur && !seen.contains(&en) && f.blocks[en].insts.len() == 1 && matches!(shapes[en], Some((x2, ..)) if x2 == x) {
                seen.push(en);
                cur = en;
            } else {
                default = e;
                break;
            }
        }
        if cases.len() < 6 {
            continue;
        }
        let min = cases.iter().map(|c| c.0).min().unwrap();
        let max = cases.iter().map(|c| c.0).max().unwrap();
        let range = (max as i128 - min as i128) as u128;
        if range > 1024 || range as usize + 1 > 4 * cases.len() {
            continue;
        }
        let mut targets = vec![default; range as usize + 1];
        for (k, t) in &cases {
            targets[(k - min) as usize] = *t;
        }
        out.insert(head as u32, (x, min, targets, default));
    }
    out
}

pub fn compile_freq(options: &relax_session::Options, f: &Func, env: &Env, freq: Option<&[f32]>) -> Result<Compiled, String> {
    // the function runs in the threaded interpreter (interp.rs, MAKEPAD_RUN_INTERP /
    // MAKEPAD_RUN_NO_CODEGEN): its "code" is the interpreter's relocatable form (interp/blob.rs)
    if let Some((code, relocs)) = crate::interp::maybe_compile(f, env.slots, env.syms, env.sym_class, env.tls_slow, env.resume) {
        let n = code.len() as u32;
        return Ok(Compiled { code, pc_map: Vec::new(), frame_size: 0, text_len: n, alloc_sig: 0, relocs, rir_map: Vec::new(), rir_emitted: String::new() });
    }
    compile_freq_timed(options, f, env, freq)
}

fn compile_freq_inner(options: &relax_session::Options, f: &Func, env: &Env, freq: Option<&[f32]>) -> Result<Compiled, String> {
    let _warm = crate::warm::Scope::backend(&f.name);
    let _cost_codegen = crate::fncost::phase(5);
    crate::fncost::size(false, f);
    for b in &f.blocks {
        for i in &b.insts {
            if matches!(i, Inst::Cpuid(..) | Inst::Xgetbv(..)) {
                return Err(format!("{}: CPUID/XGETBV require x86_64; unsupported on arm64", f.name));
            }
            if let Inst::Call(_, _, r) = i {
                let (ri, rf) = count_classes(f, r);
                if ri > 2 || rf > 2 {
                    return Err(format!("{}: call with more than 2 integer or 2 float results", f.name));
                }
            }
        }
    }
    let _cost_ra = crate::fncost::phase(9);
    let t_ra = crate::opt::pass_clock();
    // immediates arm64 cannot encode, inside loops: registers set up once
    let hoisted = regalloc::hoist_loop_consts(f, &imm_fits);
    let f = hoisted.as_ref().unwrap_or(f);
    // indexed accesses with displacements on one base and index: one address register
    let shared = regalloc::share_index_bases(f);
    let f = shared.as_ref().unwrap_or(f);
    let (split, mut al0) = regalloc::allocate_split(options, f, &RegConfig { sym_class: env.sym_class, ..reg_config() }, env.opt_regs || f.full, freq);
    let f = split.as_ref().unwrap_or(f);
    crate::opt::pass_time(11, t_ra);
    drop(_cost_ra);
    let alloc_sig = regalloc::alloc_sig(&al0);
    // a function past the branch ranges is emitted again in long mode: from the same
    // allocation (allocating twice doubled a 3.7 s allocation)
    let mut long = false;
    loop {
    let al = al0;
    let _cost_emit = crate::fncost::phase(10);
    let t_emit = crate::opt::pass_clock();
    let mut g = Gen {
        a: Asm { w: Vec::with_capacity(128) },
        test_fuse: None,
        jt_plan: jump_tables(options, f, &al),
        jt_fix: Vec::new(),
        last_spill: None,
        win: {
            let mut w: std::collections::HashMap<u32, Vec<(u32, u32, u8)>> = std::collections::HashMap::new();
            for &(v, a, z, r) in &al.windows {
                w.entry(v).or_default().push((a, z, r));
            }
            w
        },
        win_start: {
            let mut ws: std::collections::HashMap<u32, Vec<(u32, u8)>> = std::collections::HashMap::new();
            for &(v, a, _, r) in &al.windows {
                ws.entry(a).or_default().push((v, r));
            }
            ws
        },
        f,
        al,
        frame: 0,
        saved: 0,
        slot_off: Vec::new(),
        block_pos: Vec::new(),
        next_block: Vec::new(),
        fixups: Vec::new(),
        fpool: Vec::new(),
        vpool: Vec::new(),
        apool: Vec::new(),
        sym_class: env.sym_class,
        dom_slot: -1,
        relocs: Vec::new(),
        text_words: 0,
        pc_map: Vec::new(),
        cur_pos: 0,
        cplan: None,
        cbuf: Vec::new(),
        cret_buf: 0,
        stage: 0,
        last_call_ret: 0,
        pads: Vec::new(),
        long,
    };
    BRANCH_RANGE.with(|c| c.set(false));
    g.gen(options);
    if BRANCH_RANGE.with(|c| c.replace(false)) {
        if !long {
            // past +-1 MB: emit again with relaxed branches and no literal pools (emission
            // reads the allocation only)
            long = true;
            al0 = g.al;
            continue;
        }
        return Err(format!("{}: function too large for arm64 branch ranges ({} instructions)", f.name, g.a.w.len()));
    }
    let frame = g.frame as u32;
    let code = words_to_bytes(&g.a.w);
    let text_len = (g.text_words * 4) as u32;
    crate::opt::pass_time(12, t_emit);
    crate::fncost::code(code.len());
    return Ok(Compiled { code, pc_map: g.pc_map, frame_size: frame, text_len, alloc_sig, relocs: g.relocs, rir_map: Vec::new(), rir_emitted: String::new() });
    }
}

thread_local! {
    /// a branch or literal load of the function being emitted did not reach its target
    static BRANCH_RANGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// ------------------------------------------------------------ runtime glue

/// The lazy-compile entry shared by all stubs: saves the argument registers, calls
/// `compile_fn(slot) -> code address`, restores them and jumps to the code.
/// On entry x16 holds the slot index.
pub fn lazy_entry(host_compile: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.stp_pre(FP, LR, SP, -16);
    a.mov_sp(FP, SP);
    a.sub_imm(SP, SP, 144);
    for k in 0..4u8 {
        a.stp(false, 2 * k, 2 * k + 1, SP, 16 * k as i32);
        a.stp(true, 2 * k, 2 * k + 1, SP, 64 + 16 * k as i32);
    }
    // x8: the C ABI's indirect result address (extern "C" bodies reached through a stub)
    a.ldst(STRX, 8, X8, SP, 128);
    a.mov(0, IP0);
    a.mov_imm64_fixed(IP1, host_compile);
    a.blr(IP1);
    a.mov(IP0, 0);
    for k in 0..4u8 {
        a.ldp(false, 2 * k, 2 * k + 1, SP, 16 * k as i32);
        a.ldp(true, 2 * k, 2 * k + 1, SP, 64 + 16 * k as i32);
    }
    a.ldst(LDRX, 8, X8, SP, 128);
    a.mov_sp(SP, FP);
    a.ldp_post(FP, LR, SP, 16);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// Per-slot thunk: `ldr x16, [table + slot*8]; br x16` (the permanent address of a function).
pub fn thunk(slot_addr: u64, size: usize) -> Vec<u8> {
    let mut a = Asm { w: Vec::with_capacity(12) };
    a.mov_imm64_fixed(IP0, slot_addr);
    a.ldst(LDRX, 8, IP0, IP0, 0);
    a.br(IP0);
    while a.w.len() * 4 < size {
        a.udf();
    }
    words_to_bytes(&a.w)
}

/// Per-slot call counter (tier-up): while a function runs at O1 its table slot points
/// here. `*counter -= 1`; on reaching 0 jumps to `hot` (a lazy_entry-style glue: x16 = slot,
/// returns the address to continue at), else to the O1 code. Argument registers, sp
/// and lr are untouched; only x16/x17 (the AAPCS veneer registers) are used.
pub fn count_stub(counter: u64, slot: u32, code: u64, hot: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::with_capacity(12) };
    a.mov_imm64_fixed(IP0, counter);
    a.ldst(LDRX, 8, IP1, IP0, 0);
    // subs x17, x17, #1
    a.e(0xF100_0000 | 1 << 10 | (IP1 as u32) << 5 | IP1 as u32);
    a.ldst(STRX, 8, IP1, IP0, 0);
    // b.eq hot (6 words ahead: 4 + br + this)
    a.e(0x5400_0000 | 6 << 5);
    a.mov_imm64_fixed(IP0, code);
    a.br(IP0);
    a.e(0xD280_0000 | (slot & 0xffff) << 5 | IP0 as u32);
    a.e(0xF2A0_0000 | (slot >> 16) << 5 | IP0 as u32);
    a.mov_imm64_fixed(IP1, hot);
    a.br(IP1);
    words_to_bytes(&a.w)
}

/// Per-slot stub: `mov x16, slot; b lazy_entry` (absolute via x17).
pub fn stub(slot: u32, lazy: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::with_capacity(12) };
    a.e(0xD280_0000 | (slot & 0xffff) << 5 | IP0 as u32);
    a.e(0xF2A0_0000 | (slot >> 16) << 5 | IP0 as u32);
    a.mov_imm64_fixed(IP1, lazy);
    a.br(IP1);
    words_to_bytes(&a.w)
}

/// enter(fn, arg, ctx): saves the callee-saved registers and sp into *ctx, calls fn(arg).
/// Returns 0 normally. `leave(ctx, code)` (at the returned offset) restores them and
/// returns `code` from enter (used by panics, faults and hangs to get back to the host).
pub fn enter_leave() -> (Vec<u8>, usize) {
    let mut a = Asm { w: Vec::new() };
    // enter: x0 = fn, x1 = arg, x2 = ctx
    a.stp_pre(FP, LR, SP, -16);
    a.mov_sp(FP, SP);
    a.sub_imm(SP, SP, 160);
    for k in 0..5u8 {
        a.stp(false, 19 + 2 * k, 20 + 2 * k, SP, 16 * k as i32);
    }
    for k in 0..4u8 {
        a.stp(true, 8 + 2 * k, 9 + 2 * k, SP, 80 + 16 * k as i32);
    }
    // ctx[0] = sp
    a.mov_sp(IP0, SP);
    a.ldst(STRX, 8, IP0, 2, 0);
    a.mov(IP0, 0);
    a.mov(0, 1);
    a.blr(IP0);
    a.mov_imm(0, 0);
    let tail = a.pos();
    for k in 0..5u8 {
        a.ldp(false, 19 + 2 * k, 20 + 2 * k, SP, 16 * k as i32);
    }
    for k in 0..4u8 {
        a.ldp(true, 8 + 2 * k, 9 + 2 * k, SP, 80 + 16 * k as i32);
    }
    a.add_imm(SP, SP, 160);
    a.ldp_post(FP, LR, SP, 16);
    a.ret();
    // leave(ctx = x0, code = x1)
    let leave = a.pos();
    a.ldst(LDRX, 8, IP0, 0, 0);
    a.mov_sp(SP, IP0);
    a.mov(0, 1);
    let j = a.b();
    a.patch(j, tail);
    (words_to_bytes(&a.w), leave * 4)
}

/// JIT trampoline in front of a host panic function (or the unwinder's resume entry): stores
/// the JIT caller's register state as a block on the stack -- [fp, return address, x19..x28,
/// d8..d15] (20 words) -- passes its address as argument 6 (x6), then tail-jumps to `target`,
/// which never returns. The unwinder starts its frame walk from the block.
pub fn panic_tramp(target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.sub_imm(SP, SP, 160);
    a.stp(false, FP, LR, SP, 0);
    for k in 0..5u8 {
        a.stp(false, 19 + 2 * k, 20 + 2 * k, SP, 16 + 16 * k as i32);
    }
    for k in 0..4u8 {
        a.stp(true, 8 + 2 * k, 9 + 2 * k, SP, 96 + 16 * k as i32);
    }
    a.mov_sp(6, SP);
    a.mov_imm(7, 0);
    a.mov_imm64_fixed(IP0, target);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// land(regs, fp, sp, pc): continues at `pc` (a landing stub) with x19..x28 and d8..d15
/// from the register block `regs` (panic_tramp's layout), x29 = fp and sp = sp. Never returns.
pub fn land() -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    for k in 0..5u8 {
        a.ldp(false, 19 + 2 * k, 20 + 2 * k, 0, 16 + 16 * k as i32);
    }
    for k in 0..4u8 {
        a.ldp(true, 8 + 2 * k, 9 + 2 * k, 0, 96 + 16 * k as i32);
    }
    a.mov(FP, 1);
    a.mov_sp(SP, 2);
    a.br(3);
    words_to_bytes(&a.w)
}

/// `br target` through x16 (the AAPCS intra-procedure-call register a veneer may use):
/// every argument register, sp and lr arrive at `target` unchanged.
pub fn jump_to(target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.mov_imm64_fixed(IP0, target);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// `x0 = arg; br target` (tail jump keeping lr: the caller's frame stays the JIT one).
pub fn jump_with_arg0(arg: u64, target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.mov_imm64_fixed(0, arg);
    a.mov_imm64_fixed(IP0, target);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// This thread's thread-local block in x16, preserving every other register JIT code may hold
/// (x0-x15, d0-d7, d16-d29): the fast path `tls_addr` inlines (slot `key` of the thread's slot
/// array), and when the slot is still null `host() -> block`. `tls_addr` calls it after its own
/// fast path found null; a prologue that needs the domain base calls it directly.
pub fn tls_slow(host: u64, key: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    // mrs x16, <slot base>; and x16, x16, #~7; mov x17, #key*8; ldr x16, [x16, x17]
    a.e(MRS_SLOT_BASE | IP0 as u32);
    a.e(0x9243_F000 | (IP0 as u32) << 5 | IP0 as u32);
    a.mov_imm(IP1, (key * 8) as i64);
    a.e(0xF860_6800 | (IP1 as u32) << 16 | (IP0 as u32) << 5 | IP0 as u32);
    // cbz x16, +2 (past the ret): the block exists, return it
    a.e(0xB400_0000 | 2 << 5 | IP0 as u32);
    a.ret();
    a.stp_pre(FP, LR, SP, -16);
    a.mov_sp(FP, SP);
    a.sub_imm(SP, SP, 320);
    for k in 0..8u8 {
        a.stp(false, 2 * k, 2 * k + 1, SP, 16 * k as i32);
    }
    for k in 0..4u8 {
        a.stp(true, 2 * k, 2 * k + 1, SP, 128 + 16 * k as i32);
    }
    for k in 0..7u8 {
        a.stp(true, 16 + 2 * k, 17 + 2 * k, SP, 192 + 16 * k as i32);
    }
    a.mov_imm64_fixed(IP1, host);
    a.blr(IP1);
    a.mov(IP0, 0);
    for k in 0..8u8 {
        a.ldp(false, 2 * k, 2 * k + 1, SP, 16 * k as i32);
    }
    for k in 0..4u8 {
        a.ldp(true, 2 * k, 2 * k + 1, SP, 128 + 16 * k as i32);
    }
    for k in 0..7u8 {
        a.ldp(true, 16 + 2 * k, 17 + 2 * k, SP, 192 + 16 * k as i32);
    }
    a.mov_sp(SP, FP);
    a.ldp_post(FP, LR, SP, 16);
    a.ret();
    words_to_bytes(&a.w)
}

/// `read_tsd(key) -> value`: this thread's TSD slot through tpidrro_el0, exactly the
/// sequence `tls_addr` inlines (checked against pthread_getspecific at startup).
/// Whether `f` takes the address of a domain symbol (a static: its code reads the domain base).
fn uses_domain(f: &Func, sym_class: u64) -> bool {
    sym_class != 0 && f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Addr(_, s) if SymClass::of(sym_class, s.id) == SymClass::Domain)))
}

/// `mrs xN, <register holding the thread's slot array>` (N or-ed in). macOS: tpidrro_el0 (the
/// pthread TSD base; its low 3 bits carry the cpu number, masked off after). Android: tpidr_el0,
/// bionic's thread pointer = its TLS slot array (bionic libc/platform/bionic/tls_defines.h).
#[cfg(not(target_os = "android"))]
const MRS_SLOT_BASE: u32 = 0xD53B_D060;
#[cfg(target_os = "android")]
const MRS_SLOT_BASE: u32 = 0xD53B_D040;

pub fn tsd_reader() -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.e(MRS_SLOT_BASE | IP0 as u32);
    a.e(0x9243_F000 | (IP0 as u32) << 5 | IP0 as u32);
    // ldr x0, [x16, x0, lsl #3]
    a.e(0xF860_7800 | (IP0 as u32) << 5);
    a.ret();
    words_to_bytes(&a.w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R13: a conditional branch over more than 1 MB of code (imm19) is relaxed into an
    /// inverted short branch over a `b` instead of being truncated into other code (Amp's
    /// O3 script parser, 1.9 MB, branched into other functions).
    #[test]
    fn branch_out_of_range_is_relaxed() {
        let options = &relax_session::Options::default();
        let mut f = Func::new("big".to_string(), 0);
        let p = f.vreg(Cls::I);
        f.params.push(p);
        f.rets.push(Cls::I);
        let b0 = f.block();
        let b1 = f.block();
        let b2 = f.block();
        f.blocks[b0 as usize].term = Term::Branch(p, b1, b2);
        let mut acc = p;
        for k in 0..300_000 {
            let d = f.vreg(Cls::I);
            f.blocks[b1 as usize].insts.push(Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, d, acc, k + 1));
            f.blocks[b1 as usize].pos.push(0);
            acc = d;
        }
        f.blocks[b1 as usize].term = Term::Ret(vec![acc]);
        f.blocks[b2 as usize].term = Term::Ret(vec![p]);
        let env = Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
        let r = compile(options, &f, &env).expect("relaxed");
        let w: Vec<u32> = r.code[..r.text_len as usize].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        // the entry block's branch on p: cbz/cbnz skipping one word, then `b` beyond 1 MB
        let k = w.iter().position(|x| x & 0x7E00_0000 == 0x3400_0000).expect("cbz/cbnz");
        assert_eq!((w[k] >> 5) & 0x7FFFF, 2, "{:08x}", w[k]);
        assert_eq!(w[k + 1] & 0xFC00_0000, 0x1400_0000, "{:08x}", w[k + 1]);
        assert!((w[k + 1] & 0x03FF_FFFF) as usize > (1 << 18), "{:08x}", w[k + 1]);
        // long mode builds a pooled f64 constant in a register: as an f64 (fmov dN, x16)
        let fc = f.vreg(Cls::F64);
        let fb = f.vreg(Cls::I);
        let bits = 0.1f64.to_bits();
        f.blocks[b2 as usize].insts.push(Inst::Fconst(fc, bits, true));
        f.blocks[b2 as usize].pos.push(0);
        f.blocks[b2 as usize].insts.push(Inst::Conv(Conv::FToBits(true), fb, fc));
        f.blocks[b2 as usize].pos.push(0);
        f.blocks[b2 as usize].term = Term::Ret(vec![fb]);
        let r = compile(options, &f, &env).expect("relaxed with an f64 constant");
        let w: Vec<u32> = r.code[..r.text_len as usize].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        assert!(w.iter().any(|x| x & 0xFFFF_FFE0 == 0x9E67_0200), "no fmov dN, x16");
        assert!(!w.iter().any(|x| x & 0xFFFF_FFE0 == 0x1E27_0200), "f64 constant built as an f32");
        assert!(!w.iter().any(|x| x & 0xFF00_0000 == 0x5C00_0000), "literal pool load in long mode");
        f.blocks[b2 as usize].insts.clear();
        f.blocks[b2 as usize].pos.clear();
        f.blocks[b2 as usize].term = Term::Ret(vec![p]);
        // a small one still compiles
        f.blocks[b1 as usize].insts.truncate(1000);
        f.blocks[b1 as usize].pos.truncate(1000);
        let last = match f.blocks[b1 as usize].insts.last() {
            Some(Inst::IBinI(_, _, d, ..)) => *d,
            _ => p,
        };
        f.blocks[b1 as usize].term = Term::Ret(vec![last]);
        assert!(compile(options, &f, &env).is_ok());
    }

    /// The fused entry branches (`x == 0` as cbz/cbnz, `x & 8 == 0` as tst + b.cond) also relax
    /// past +-1 MB: the inverted short branch over a `b`.
    #[test]
    fn fused_branches_are_relaxed() {
        let options = &relax_session::Options::default();
        for tst in [false, true] {
            let mut f = Func::new("bigf".to_string(), 0);
            let p = f.vreg(Cls::I);
            f.params.push(p);
            f.rets.push(Cls::I);
            let (b0, b1, b2) = (f.block(), f.block(), f.block());
            let c = f.vreg(Cls::I);
            let it = IntTy { bits: 64, signed: false };
            let mut x = p;
            if tst {
                x = f.vreg(Cls::I);
                f.blocks[b0 as usize].insts.push(Inst::IBinI(IOp::And, it, x, p, 8));
                f.blocks[b0 as usize].pos.push(0);
            }
            f.blocks[b0 as usize].insts.push(Inst::ICmpI(Cond::Eq, false, c, x, 0));
            f.blocks[b0 as usize].pos.push(0);
            f.blocks[b0 as usize].term = Term::Branch(c, b1, b2);
            let mut acc = p;
            for k in 0..300_000 {
                let d = f.vreg(Cls::I);
                f.blocks[b1 as usize].insts.push(Inst::IBinI(IOp::Add, it, d, acc, k + 1));
                f.blocks[b1 as usize].pos.push(0);
                acc = d;
            }
            f.blocks[b1 as usize].term = Term::Ret(vec![acc]);
            f.blocks[b2 as usize].term = Term::Ret(vec![p]);
            let env = Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
            let r = compile(options, &f, &env).expect("relaxed");
            let w: Vec<u32> = r.code[..r.text_len as usize].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            // the first conditional branch skips one word; a `b` beyond 1 MB follows
            let k = w.iter().position(|x| x & 0x7E00_0000 == 0x3400_0000 || x & 0xFF00_0010 == 0x5400_0000).expect("conditional branch");
            assert_eq!((w[k] >> 5) & 0x7FFFF, 2, "{:08x}", w[k]);
            assert_eq!(w[k + 1] & 0xFC00_0000, 0x1400_0000, "{:08x}", w[k + 1]);
            assert!((w[k + 1] & 0x03FF_FFFF) as usize > (1 << 18), "{:08x}", w[k + 1]);
            if tst {
                // tst x, #8 (ANDS xzr) before it, no and
                assert_eq!(w[k - 1] & 0xFF80_001F, 0xF200_001F, "{:08x}", w[k - 1]);
            }
        }
    }
}
