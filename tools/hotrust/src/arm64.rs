//! AArch64 backend (tier 0): instruction selection and encoding over the shared
//! register allocator (regalloc.rs), plus the runtime glue code the live image
//! needs (lazy-compile entry, stubs, thunks, enter/leave, panic trampolines).
//!
//! ABI (HotRust's own = AAPCS64 for host calls): int args x0-x7, float args v0-v7,
//! int returns x0 x1, float returns v0 v1. Callee-saved x19-x28 and the low 64 bits
//! of v8-v15. x18 is the platform register on Darwin and never touched. Frame
//! pointers always on: every function stores its frame record (x29, x30) at x29.
//! Scratch, never allocated: x14 x15 (operands), x16 (call targets, immediates),
//! x17 (address offsets), v30 v31.
//!
//! Frame (fp = x29 after the prologue):
//!   [fp + 8]  return address, [fp] caller's fp
//!   [fp - 8 * k]  callee-saved registers (ints then floats)
//!   below: spill area, stack slots, poll save area; sp = fp - frame (16-aligned)
//! The body addresses the frame through sp with positive offsets.
//!
//! Encodings follow the hand-written encoders of platform/script/compute/src/arm64.rs,
//! extended to 64-bit integer forms.

use crate::cabi::{self, ArgPlan, PLoc, Plan, RetPlan};
use crate::regalloc::{self, Alloc, Loc, MLoc, RegConfig};
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
/// x8: indirect result register of the C ABI; otherwise a scratch for atomics loops
const X8: u8 = 8;

const INT_ARGS: usize = 8;
const FLT_ARGS: usize = 8;

/// arm64 isel switches for measurements: `HOTRUST_A64=none` or `-fuse,-fconst`.
const A64_FUSE: u8 = 1;
const A64_FCONST: u8 = 2;

fn isel_flags() -> u8 {
    static FLAGS: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *FLAGS.get_or_init(|| {
        let mut f = A64_FUSE | A64_FCONST;
        if let Ok(v) = std::env::var("HOTRUST_A64") {
            for part in v.split(',') {
                match part {
                    "none" => f = 0,
                    "-fuse" => f &= !A64_FUSE,
                    "-fconst" => f &= !A64_FCONST,
                    _ => {}
                }
            }
        }
        f
    })
}

pub fn reg_config() -> RegConfig {
    RegConfig {
        int_caller: vec![9, 10, 11, 12, 13, 2, 3, 4, 5, 6, 7],
        int_callee: vec![19, 20, 21, 22, 23, 24, 25, 26, 27, 28],
        flt_caller: vec![16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29],
        flt_callee: vec![8, 9, 10, 11, 12, 13, 14, 15],
    }
}

pub struct Compiled {
    pub code: Vec<u8>,
    /// (code offset, source position) in ascending offset order
    pub pc_map: Vec<(u32, u64)>,
    pub frame_size: u32,
    /// bytes of instructions (the float literal pool follows)
    pub text_len: u32,
}

/// Addresses the backend needs from the runtime.
pub struct Env {
    pub table_base: u64,
    pub thunk_base: u64,
    pub thunk_size: u64,
    pub poll_flag: u64,
    pub rt_hang: u64,
    /// pthread TSD slot holding this thread's thread-local block (0 = none yet)
    pub tls_key: u64,
    /// glue that allocates the calling thread's block: returns it in x16, preserves all else
    pub tls_slow: u64,
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

struct Gen<'a> {
    a: Asm,
    f: &'a Func,
    al: Alloc,
    env: &'a Env,
    /// total frame size below fp (sp = fp - frame)
    frame: i64,
    /// bytes of callee-saved registers below fp
    saved: i64,
    slot_off: Vec<i64>,
    /// sp offset of the poll save area and the caller-saved registers it covers
    poll_area: i64,
    poll_regs: Vec<Loc>,
    block_pos: Vec<usize>,
    next_block: Vec<u32>,
    fixups: Vec<(usize, u32)>,
    /// float literal pool: (ldr word index, bits)
    fpool: Vec<(usize, u64)>,
    text_words: usize,
    pc_map: Vec<(u32, u64)>,
    cur_pos: u32,
    /// C ABI body: its plan, per-argument buffer (sp offset, -1 = none), result buffer
    cplan: Option<Plan>,
    cbuf: Vec<i64>,
    cret_buf: i64,
    /// sp offset of the C call staging cells (8 bytes each)
    stage: i64,
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
                let off = self.spill_sp(o);
                self.a.ldst(LDRX, 8, scratch, SP, off);
                scratch
            }
            _ => ZR,
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
        }
    }
    fn use_f(&mut self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Flt(x) => x,
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

    fn gen(&mut self) {
        // frame layout
        let n_saved = self.al.used_callee_int.len() + self.al.used_callee_flt.len();
        self.saved = 8 * n_saved as i64;
        let mut off = self.saved + self.al.spill_bytes as i64;
        let mut slot_fp = Vec::new();
        for s in &self.f.slots {
            let al = s.align.max(1) as i64;
            off += s.size as i64;
            off = (off + al - 1) / al * al;
            slot_fp.push(off);
        }
        // poll save area: caller-saved registers in use, if the function polls
        let mut polls = false;
        for b in &self.f.blocks {
            for i in &b.insts {
                if let Inst::Poll = i {
                    polls = true;
                }
            }
        }
        if polls {
            let cfg = reg_config();
            let mut seen: Vec<Loc> = Vec::new();
            for l in &self.al.loc {
                let caller = match l {
                    Loc::Reg(r) => cfg.int_caller.contains(r),
                    Loc::Flt(x) => cfg.flt_caller.contains(x),
                    _ => false,
                };
                if caller && !seen.contains(l) {
                    seen.push(*l);
                }
            }
            off += 8 * seen.len() as i64;
            self.poll_regs = seen;
        }
        let poll_fp = off;
        // C ABI body: buffers that rebuild aggregate params from their registers, and the
        // buffer an aggregate result is written to before Ret loads it into registers
        let mut cbuf_fp: Vec<i64> = Vec::new();
        let mut cret_fp = 0i64;
        if let Some(sig) = &self.f.cabi {
            let plan = cabi::plan(sig, cabi::Target::Darwin64);
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
                            let plan = cabi::plan(sig, cabi::Target::Darwin64);
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
        let frame = (off + out + 15) / 16 * 16;
        self.frame = frame;
        for x in slot_fp {
            self.slot_off.push(frame - x);
        }
        self.poll_area = frame - poll_fp;
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
        let mut k = 0i64;
        for r in self.al.used_callee_int.clone() {
            k += 1;
            self.a.ldst(STRX, 8, r, FP, -8 * k);
        }
        for x in self.al.used_callee_flt.clone() {
            k += 1;
            self.a.ldst(STRD, 8, x, FP, -8 * k);
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
                if n > 0 && self.al.uses[c.0 as usize] == 1 && isel_flags() & A64_FUSE != 0 {
                    match &b.insts[n - 1] {
                        Inst::ICmp(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) | Inst::FCmp(_, _, d, _, _) if d == c => fused = true,
                        _ => {}
                    }
                }
            }
            for (ii, inst) in b.insts.iter().enumerate() {
                self.pc_map.push(((self.a.pos() * 4) as u32, b.pos[ii]));
                self.cur_pos = self.al.inst_pos(bi, ii);
                if fused && ii == n - 1 {
                    break;
                }
                self.inst(inst);
            }
            self.pc_map.push(((self.a.pos() * 4) as u32, b.term_pos));
            let fuse_inst = if fused { Some(b.insts[n - 1].clone()) } else { None };
            self.term(&b.term, bi, fuse_inst);
        }
        for (at, t) in std::mem::take(&mut self.fixups) {
            let target = self.block_pos[t as usize];
            self.a.patch(at, target);
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
    }

    /// HotRust ABI params: incoming registers -> allocated locations; overflow params
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
    /// (their address is the param), the rest arrive like HotRust params but with C's
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
        let plan = cabi::plan(sig, cabi::Target::Darwin64);
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            let off = self.spill_sp(*slot);
            match self.al.loc[v.0 as usize] {
                Loc::Reg(r) => self.a.ldst(STRX, 8, r, SP, off),
                Loc::Flt(x) => self.a.ldst(STRD, 8, x, SP, off),
                _ => {}
            }
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
                let slot = self.env.table_base + *id as u64 * 8;
                self.a.mov_imm(IP0, slot as i64);
                self.a.ldst(LDRX, 8, IP0, IP0, 0);
                self.a.blr(IP0);
            }
            Callee::CHost(addr, _) => {
                self.a.mov_imm(IP0, *addr as i64);
                self.a.blr(IP0);
            }
            _ => self.a.blr(IP0),
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
            let off = self.spill_sp(*slot);
            match self.al.loc[v.0 as usize] {
                Loc::Reg(r) => self.a.ldst(LDRX, 8, r, SP, off),
                Loc::Flt(x) => self.a.ldst(LDRD, 8, x, SP, off),
                _ => {}
            }
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

    /// d = this thread's thread-local block + off. Fast path: the block pointer sits in
    /// the thread's pthread TSD slot (tpidrro_el0 base); a null slot calls `tls_slow`.
    fn tls_addr(&mut self, d: VReg, off: u32) {
        // mrs x16, tpidrro_el0; and x16, x16, #~7; ldr x16, [x16, #key*8]
        self.a.e(0xD53B_D060 | IP0 as u32);
        self.a.e(0x9243_F000 | (IP0 as u32) << 5 | IP0 as u32);
        self.a.ldst(LDRX, 8, IP0, IP0, self.env.tls_key as i64 * 8);
        let p = self.a.cbnz(IP0);
        // slow path keeps every register but x16/x17 (glue), so no saves here; lr is
        // preserved by the frame record
        self.a.mov_imm(IP1, self.env.tls_slow as i64);
        self.a.blr(IP1);
        let skip = self.a.pos();
        self.a.patch(p, skip);
        let rd = self.def_i(d, S0);
        self.a.add_any(rd, IP0, off as i64);
        self.fin_i(d, rd);
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

    fn jump_to(&mut self, target: u32, bi: usize) {
        if self.next_block[bi] != target {
            let p = self.a.b();
            self.fixups.push((p, target));
        }
    }

    fn epilogue(&mut self) {
        let mut k = 0i64;
        for r in self.al.used_callee_int.clone() {
            k += 1;
            self.a.ldst(LDRX, 8, r, FP, -8 * k);
        }
        for x in self.al.used_callee_flt.clone() {
            k += 1;
            self.a.ldst(LDRD, 8, x, FP, -8 * k);
        }
        self.a.mov_sp(SP, FP);
        self.a.ldp_post(FP, LR, SP, 16);
        self.a.ret();
    }

    fn term(&mut self, t: &Term, bi: usize, fused: Option<Inst>) {
        match t {
            Term::Jump(target) => self.jump_to(*target, bi),
            Term::Branch(c, tt, ff) => {
                let next = self.next_block[bi];
                match fused {
                    Some(i) => {
                        let cc = self.cmp_flags(&i);
                        if *tt == next {
                            // the condition codes come in true/false pairs: cc ^ 1 negates
                            let p = self.a.bcond(cc ^ 1);
                            self.fixups.push((p, *ff));
                        } else {
                            let p = self.a.bcond(cc);
                            self.fixups.push((p, *tt));
                            self.jump_to(*ff, bi);
                        }
                    }
                    None => {
                        let r = self.use_i(*c, S0);
                        if *tt == next {
                            let p = self.a.cbz(r);
                            self.fixups.push((p, *ff));
                        } else {
                            let p = self.a.cbnz(r);
                            self.fixups.push((p, *tt));
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
        }
    }

    fn inst(&mut self, i: &Inst) {
        match i {
            Inst::Iconst(d, v) => {
                let r = self.def_i(*d, S0);
                self.a.mov_imm(r, *v);
                self.fin_i(*d, r);
            }
            Inst::Fconst(d, bits, f64_) => {
                let x = self.def_f(*d, FS0);
                if *bits == 0 {
                    self.a.fmov_from_gp(true, x, ZR);
                } else if isel_flags() & A64_FCONST == 0 {
                    self.a.mov_imm(IP0, *bits as i64);
                    self.a.fmov_from_gp(*f64_, x, IP0);
                } else if let Some(imm8) = fp_imm8(*bits, *f64_) {
                    // fmov dd/sd, #imm
                    self.a.e(0x1E20_1000 | if *f64_ { DBL } else { 0 } | imm8 << 13 | x as u32);
                } else if *f64_ {
                    // ldr dd, =bits (literal pool after the code)
                    let at = self.a.pos();
                    self.a.e(0x5C00_0000 | x as u32);
                    self.fpool.push((at, *bits));
                } else {
                    self.a.mov_imm(IP0, *bits as i64);
                    self.a.fmov_from_gp(false, x, IP0);
                }
                self.fin_f(*d, x);
            }
            Inst::Mov(d, s) => {
                if self.cls(*d) == Cls::I {
                    let rs = self.use_i(*s, S0);
                    let rd = self.def_i(*d, S0);
                    self.a.mov(rd, rs);
                    self.fin_i(*d, rd);
                } else {
                    let xs = self.use_f(*s, FS0);
                    let xd = self.def_f(*d, FS0);
                    self.a.fmov(xd, xs);
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
                // ip1 = base + (idx << sh), then the plain load
                let ri = self.use_i(*idx, S0);
                let rb = self.use_i(*base, S1);
                self.a.e(0x8B00_0000 | ((*sh as u32) << 10) | ((ri as u32) << 16) | ((rb as u32) << 5) | IP1 as u32);
                // ldst's large-offset form needs ip1 itself: fold big offsets in first
                let off = &if (*off as i64).abs() >= 256 {
                    self.a.add_any(IP1, IP1, *off as i64);
                    0
                } else {
                    *off
                };
                let (op, size) = mem_op(*m, true);
                match m {
                    Mem::Int(..) => {
                        let rd = self.def_i(*d, S0);
                        self.a.ldst(op, size, rd, IP1, *off as i64);
                        self.fin_i(*d, rd);
                    }
                    _ => {
                        let xd = self.def_f(*d, FS0);
                        self.a.ldst(op, size, xd, IP1, *off as i64);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::StoreX(m, base, idx, sh, off, src) => {
                let ri = self.use_i(*idx, S0);
                let rb = self.use_i(*base, S1);
                self.a.e(0x8B00_0000 | ((*sh as u32) << 10) | ((ri as u32) << 16) | ((rb as u32) << 5) | IP1 as u32);
                let off = &if (*off as i64).abs() >= 256 {
                    self.a.add_any(IP1, IP1, *off as i64);
                    0
                } else {
                    *off
                };
                let (op, size) = mem_op(*m, false);
                match m {
                    Mem::Int(..) => {
                        let rs = self.use_i(*src, S0);
                        self.a.ldst(op, size, rs, IP1, *off as i64);
                    }
                    _ => {
                        let xs = self.use_f(*src, FS0);
                        self.a.ldst(op, size, xs, IP1, *off as i64);
                    }
                }
            }
            Inst::Load(m, d, base, off) => {
                let rb = self.use_i(*base, S1);
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
                let rb = self.use_i(*base, S1);
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
            Inst::Addr(d, v) => {
                let rd = self.def_i(*d, S0);
                self.a.mov_imm(rd, *v as i64);
                self.fin_i(*d, rd);
            }
            Inst::FnAddr(d, id) => {
                let rd = self.def_i(*d, S0);
                let a = self.env.thunk_base + *id as u64 * self.env.thunk_size;
                self.a.mov_imm(rd, a as i64);
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
            Inst::Poll => self.poll(),
            Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) => self.atomic(i),
            Inst::TlsAddr(d, off) => self.tls_addr(*d, *off),
        }
    }

    /// Hang-watchdog poll: `ldrb w16, [flag]; cbz w16, skip; <save>; bl rt_hang; <restore>`.
    /// rt_hang does not return when the flag is set; the save covers the race where it does.
    fn poll(&mut self) {
        self.a.mov_imm(IP0, self.env.poll_flag as i64);
        self.a.ldst(LDRB, 1, IP0, IP0, 0);
        let p = self.a.cbz(IP0);
        let regs = self.poll_regs.clone();
        let base = self.poll_area;
        for (k, l) in regs.iter().enumerate() {
            match l {
                Loc::Reg(r) => self.a.ldst(STRX, 8, *r, SP, base + 8 * k as i64),
                Loc::Flt(x) => self.a.ldst(STRD, 8, *x, SP, base + 8 * k as i64),
                _ => {}
            }
        }
        self.a.mov_imm(IP0, self.env.rt_hang as i64);
        self.a.blr(IP0);
        for (k, l) in regs.iter().enumerate() {
            match l {
                Loc::Reg(r) => self.a.ldst(LDRX, 8, *r, SP, base + 8 * k as i64),
                Loc::Flt(x) => self.a.ldst(LDRD, 8, *x, SP, base + 8 * k as i64),
                _ => {}
            }
        }
        let skip = self.a.pos();
        self.a.patch(p, skip);
    }

    fn ibin_rr(&mut self, op: IOp, it: IntTy, rd: u8, ra: u8, rb: u8) {
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
            let off = self.spill_sp(*slot);
            match self.al.loc[v.0 as usize] {
                Loc::Reg(r) => self.a.ldst(STRX, 8, r, SP, off),
                Loc::Flt(x) => self.a.ldst(STRD, 8, x, SP, off),
                _ => {}
            }
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
                let slot = self.env.table_base + *id as u64 * 8;
                self.a.mov_imm(IP0, slot as i64);
                self.a.ldst(LDRX, 8, IP0, IP0, 0);
                self.a.blr(IP0);
            }
            Callee::Indirect(_) => self.a.blr(IP0),
            // HostVariadic: compiler lane added the variant; Darwin stack placement of the
            // variadic args is the arm64 lane's A3 (treated as a plain host call for now)
            Callee::Host(addr) | Callee::HostVariadic(addr, _) => {
                self.a.mov_imm(IP0, *addr as i64);
                self.a.blr(IP0);
            }
            Callee::CHost(..) | Callee::CIndirect(..) | Callee::CFn(..) => {}
        }
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
            let off = self.spill_sp(*slot);
            match self.al.loc[v.0 as usize] {
                Loc::Reg(r) => self.a.ldst(LDRX, 8, r, SP, off),
                Loc::Flt(x) => self.a.ldst(LDRD, 8, x, SP, off),
                _ => {}
            }
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

pub fn compile(f: &Func, env: &Env) -> Result<Compiled, String> {
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Call(_, _, r) = i {
                let (ri, rf) = count_classes(f, r);
                if ri > 2 || rf > 2 {
                    return Err(format!("{}: call with more than 2 integer or 2 float results", f.name));
                }
            }
        }
    }
    let al = regalloc::allocate(f, &reg_config());
    let mut g = Gen {
        a: Asm { w: Vec::with_capacity(128) },
        f,
        al,
        env,
        frame: 0,
        saved: 0,
        slot_off: Vec::new(),
        poll_area: 0,
        poll_regs: Vec::new(),
        block_pos: Vec::new(),
        next_block: Vec::new(),
        fixups: Vec::new(),
        fpool: Vec::new(),
        text_words: 0,
        pc_map: Vec::new(),
        cur_pos: 0,
        cplan: None,
        cbuf: Vec::new(),
        cret_buf: 0,
        stage: 0,
    };
    g.gen();
    let frame = g.frame as u32;
    let code = words_to_bytes(&g.a.w);
    let text_len = (g.text_words * 4) as u32;
    Ok(Compiled { code, pc_map: g.pc_map, frame_size: frame, text_len })
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
    let mut a = Asm { w: Vec::new() };
    a.mov_imm64_fixed(IP0, slot_addr);
    a.ldst(LDRX, 8, IP0, IP0, 0);
    a.br(IP0);
    while a.w.len() * 4 < size {
        a.udf();
    }
    words_to_bytes(&a.w)
}

/// Per-slot stub: `mov x16, slot; b lazy_entry` (absolute via x17).
pub fn stub(slot: u32, lazy: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
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

/// JIT trampoline in front of a host panic function: passes the caller's frame
/// (fp, return address) as arguments 4 and 5 (x4, x5), then tail-jumps to `target`.
pub fn panic_tramp(target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.mov(4, FP);
    a.mov(5, LR);
    a.mov_imm64_fixed(IP0, target);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// Slow path of `TlsAddr`: calls `host() -> block` and returns the block in x16,
/// preserving every other register JIT code may hold (x0-x15, d0-d7, d16-d29).
pub fn tls_slow(host: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
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
pub fn tsd_reader() -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.e(0xD53B_D060 | IP0 as u32);
    a.e(0x9243_F000 | (IP0 as u32) << 5 | IP0 as u32);
    // ldr x0, [x16, x0, lsl #3]
    a.e(0xF860_7800 | (IP0 as u32) << 5);
    a.ret();
    words_to_bytes(&a.w)
}
