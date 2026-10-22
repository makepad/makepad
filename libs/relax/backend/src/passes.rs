//! The target-independent RIR passes every optimising tier runs, Relax's baseline/O1 and Splash
//! kernels alike: constant folding and copy propagation ([`simplify`]), local CSE, dead code,
//! jump threading, scalar replacement of stack slots ([`sroa`]), loop-invariant code motion
//! ([`licm`]), constant sinking, block layout ([`relayout`]), address modes and loop rotation.
//! Relax's O3 passes (relax-codegen) build on them.

use crate::bitops::{ext, vector_byte_masks};
use crate::dom::{dom_numbers, idoms, relayout_order};
use crate::regalloc::{for_defs, for_uses, reachable, term_uses, uses_defs};
use crate::rir::*;
use crate::stats::{opt_off, pass_clock, pass_time};

/// MAKEPAD_RUN_OPT=-ifconv: no if-conversion (bisection).
pub fn if_conv_on(options: &relax_session::Options) -> bool {
    !options.opt.iter().any(|p| p == "-ifconv")
}

/// MAKEPAD_RUN_OPT=-thread: no jump threading over known values (bisection).
pub fn thread_on(options: &relax_session::Options) -> bool {
    !options.opt.iter().any(|p| p == "-thread")
}

pub fn map_inst(x: &Inst, m: &dyn Fn(VReg) -> VReg, slot_base: u32) -> Inst {
    match x {
        Inst::Iconst(d, v) => Inst::Iconst(m(*d), *v),
        Inst::Fconst(d, v, w) => Inst::Fconst(m(*d), *v, *w),
        Inst::Mov(d, a) => Inst::Mov(m(*d), m(*a)),
        Inst::IBin(o, t, d, a, b) => Inst::IBin(*o, *t, m(*d), m(*a), m(*b)),
        Inst::IBinI(o, t, d, a, k) => Inst::IBinI(*o, *t, m(*d), m(*a), *k),
        Inst::INeg(t, d, a) => Inst::INeg(*t, m(*d), m(*a)),
        Inst::INot(t, d, a) => Inst::INot(*t, m(*d), m(*a)),
        Inst::IBit(o, d, a) => Inst::IBit(*o, m(*d), m(*a)),
        Inst::ICmp(c, s, d, a, b) => Inst::ICmp(*c, *s, m(*d), m(*a), m(*b)),
        Inst::ICmpI(c, s, d, a, k) => Inst::ICmpI(*c, *s, m(*d), m(*a), *k),
        Inst::FBin(o, w, d, a, b) => Inst::FBin(*o, *w, m(*d), m(*a), m(*b)),
        Inst::FUnary(o, w, d, a) => Inst::FUnary(*o, *w, m(*d), m(*a)),
        Inst::FCmp(c, w, d, a, b) => Inst::FCmp(*c, *w, m(*d), m(*a), m(*b)),
        Inst::Conv(c, d, a) => Inst::Conv(*c, m(*d), m(*a)),
        Inst::Load(k, d, b, o) => Inst::Load(*k, m(*d), m(*b), *o),
        Inst::Store(k, b, o, s) => Inst::Store(*k, m(*b), *o, m(*s)),
        Inst::SlotAddr(d, s) => Inst::SlotAddr(m(*d), s + slot_base),
        Inst::Addr(d, a) => Inst::Addr(m(*d), *a),
        Inst::FnAddr(d, id) => Inst::FnAddr(m(*d), *id),
        Inst::Call(c, a, r) => {
            let c2 = match c {
                Callee::Indirect(v) => Callee::Indirect(m(*v)),
                Callee::Fn(i) => Callee::Fn(*i),
                Callee::Host(h) => Callee::Host(*h),
                Callee::CHost(h, sig) => Callee::CHost(*h, sig.clone()),
                Callee::CIndirect(v, sig) => Callee::CIndirect(m(*v), sig.clone()),
                Callee::CFn(i, sig) => Callee::CFn(*i, sig.clone()),
                Callee::Asm(b) => Callee::Asm(b.clone()),
            };
            let mut a2 = Vec::new();
            for v in a {
                a2.push(m(*v));
            }
            let mut r2 = Vec::new();
            for v in r {
                r2.push(m(*v));
            }
            Inst::Call(c2, a2, r2)
        }
        Inst::Copy(a, b, n) => Inst::Copy(m(*a), m(*b), *n),
        Inst::LoadX(k, d, b, x, sh, o) => Inst::LoadX(*k, m(*d), m(*b), m(*x), *sh, *o),
        Inst::StoreX(k, b, x, sh, o, v) => Inst::StoreX(*k, m(*b), m(*x), *sh, *o, m(*v)),
        Inst::AtomicLoad(k, o, d, a) => Inst::AtomicLoad(*k, *o, m(*d), m(*a)),
        Inst::AtomicStore(k, o, a, v) => Inst::AtomicStore(*k, *o, m(*a), m(*v)),
        Inst::AtomicRmw(op, k, o, d, a, v) => Inst::AtomicRmw(*op, *k, *o, m(*d), m(*a), m(*v)),
        Inst::AtomicCas(k, o1, o2, d, a, e, n) => Inst::AtomicCas(*k, *o1, *o2, m(*d), m(*a), m(*e), m(*n)),
        Inst::Fence(o, st) => Inst::Fence(*o, *st),
        Inst::FpEnv(s, v) => Inst::FpEnv(*s, m(*v)),
        Inst::Cpuid(ds, a, b) => Inst::Cpuid(ds.map(m), m(*a), m(*b)),
        Inst::Xgetbv(d, a) => Inst::Xgetbv(m(*d), m(*a)),
        Inst::TlsAddr(d, off) => Inst::TlsAddr(m(*d), *off),
        Inst::TlsStatic(d, off) => Inst::TlsStatic(m(*d), *off),
        Inst::Sel(d, c, a, b) => Inst::Sel(m(*d), m(*c), m(*a), m(*b)),
        Inst::VSel(d, k, a, b) => Inst::VSel(m(*d), m(*k), m(*a), m(*b)),
        Inst::VFma(np, na, d, a, b, c) => Inst::VFma(*np, *na, m(*d), m(*a), m(*b), m(*c)),
        Inst::VSplat(d, a) => Inst::VSplat(m(*d), m(*a)),
        Inst::VExtract(d, a, l) => Inst::VExtract(m(*d), m(*a), *l),
        Inst::VInsert(d, v, l, a) => Inst::VInsert(m(*d), m(*v), *l, m(*a)),
        Inst::VLoad(d, b, o) => Inst::VLoad(m(*d), m(*b), *o),
        Inst::VStore(b, o, v) => Inst::VStore(m(*b), *o, m(*v)),
        Inst::VLoadX(d, b, x, sh, o) => Inst::VLoadX(m(*d), m(*b), m(*x), *sh, *o),
        Inst::VStoreX(b, x, sh, o, v) => Inst::VStoreX(m(*b), m(*x), *sh, *o, m(*v)),
        Inst::VAny(d, a) => Inst::VAny(m(*d), m(*a)),
        Inst::VConst(d, k) => Inst::VConst(m(*d), *k),
        Inst::VGather(d, b, i) => Inst::VGather(m(*d), m(*b), m(*i)),
        Inst::VTbl(d, b, i, n) => Inst::VTbl(m(*d), m(*b), m(*i), *n),
        Inst::VShuffle(d, x, y, k) => Inst::VShuffle(m(*d), m(*x), m(*y), *k),
        Inst::VX(op, d, a, b) => Inst::VX(*op, m(*d), m(*a), m(*b)),
        Inst::VX3(op, d, a, b, c) => Inst::VX3(*op, m(*d), m(*a), m(*b), m(*c)),
        Inst::FMulAdd(np, na, w, d, a, b, c) => Inst::FMulAdd(*np, *na, *w, m(*d), m(*a), m(*b), m(*c)),
    }
}

// ------------------------------------------------------------ folding

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum K {
    I(i64),
    F(u64, bool),
    /// a symbol's value + offset (an address the embedder relocates)
    A(Sym),
}

/// What the embedder says about `n` bytes at a constant address in immutable data
/// (`RoReader`): their value, a pointer (the 8 bytes hold symbol + offset), or "do not fold"
/// (not immutable, or bytes whose meaning the embedder cannot vouch for across runs).
pub enum RoRead {
    /// the bytes, little-endian, zero-extended to 64 bits
    Bytes(u64),
    Ptr(Sym),
    /// For a zero-size query only: this many bytes starting at the symbol are immutable.
    /// Unlike a scalar read this does not require classifying pointer words or padding.
    Range(u64),
    No,
}

/// Loads at constant addresses fold through this (None: no load folding).
/// A query with n = 0 asks for an immutable extent (Range), without reading any bytes.
/// `ctx` names the embedder state the symbols belong to (the JIT unit the function is built
/// for): a process can hold more than one unit (the proc-macro world next to the program), and
/// a symbol id read against another unit's tables names another object.
#[derive(Clone, Copy, Debug)]
pub struct RoReader {
    pub read: fn(usize, Sym, u32) -> RoRead,
    pub ctx: usize,
}

impl RoReader {
    pub fn new(read: fn(usize, Sym, u32) -> RoRead, ctx: usize) -> RoReader {
        RoReader { read, ctx }
    }

    /// `n` bytes at symbol + offset (n = 0: the immutable extent).
    pub fn read(&self, s: Sym, n: u32) -> RoRead {
        (self.read)(self.ctx, s, n)
    }
}

/// `IBit` on a constant (the semantics every backend implements).
pub fn bit_op(o: BitOp, x: u64) -> u64 {
    match o {
        BitOp::Popcnt => x.count_ones() as u64,
        BitOp::Clz => x.leading_zeros() as u64,
        BitOp::Ctz => x.trailing_zeros() as u64,
        BitOp::Bswap => x.swap_bytes(),
        BitOp::Rev => x.reverse_bits(),
    }
}

pub fn fold_ibin(op: IOp, it: IntTy, a: i64, b: i64) -> Option<i64> {
    let r = match op {
        IOp::Add => a.wrapping_add(b),
        IOp::Sub => a.wrapping_sub(b),
        IOp::Mul => a.wrapping_mul(b),
        IOp::And => a & b,
        IOp::Or => a | b,
        IOp::Xor => a ^ b,
        IOp::Shl => a.wrapping_shl((b & 63) as u32),
        IOp::Shr => {
            if it.signed {
                a.wrapping_shr((b & 63) as u32)
            } else {
                ((a as u64) >> (b & 63)) as i64
            }
        }
        IOp::Rotl => {
            let k = (b as u32) % (it.bits as u32).max(1);
            match it.bits {
                32 => (a as u32).rotate_left(k) as i64,
                64 => (a as u64).rotate_left(k) as i64,
                _ => return None,
            }
        }
        IOp::Bzhi => {
            let n = (b & 63) as u32;
            if n == 0 {
                0
            } else {
                a & (u64::MAX >> (64 - n)) as i64
            }
        }
        IOp::MulHi => {
            if it.signed {
                ((a as i128 * b as i128) >> 64) as i64
            } else {
                ((a as u64 as u128 * b as u64 as u128) >> 64) as i64
            }
        }
        IOp::Div | IOp::Rem => {
            if b == 0 || (it.signed && b == -1) {
                return None;
            }
            if it.signed {
                if op == IOp::Div {
                    a / b
                } else {
                    a % b
                }
            } else if op == IOp::Div {
                ((a as u64) / (b as u64)) as i64
            } else {
                ((a as u64) % (b as u64)) as i64
            }
        }
    };
    Some(ext(r, it))
}

pub fn cmp_i(c: Cond, signed: bool, a: i64, b: i64) -> bool {
    if signed {
        match c {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    } else {
        let (a, b) = (a as u64, b as u64);
        match c {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    }
}

pub fn fval(bits: u64, w: bool) -> f64 {
    if w {
        f64::from_bits(bits)
    } else {
        f32::from_bits(bits as u32) as f64
    }
}

pub fn fold_fbin(op: FOp, w: bool, a: u64, b: u64) -> u64 {
    // select forms return an operand's exact bits
    if op == FOp::MinLt || op == FOp::MaxGt {
        let (x, y) = (fval(a, w), fval(b, w));
        let pick_a = if op == FOp::MinLt { x < y } else { x > y };
        return if pick_a { a } else { b };
    }
    if w {
        let (x, y) = (f64::from_bits(a), f64::from_bits(b));
        let r = match op {
            FOp::Add => x + y,
            FOp::Sub => x - y,
            FOp::Mul => x * y,
            FOp::Div => x / y,
            FOp::Min => x.min(y),
            FOp::Max => x.max(y),
            FOp::MinLt | FOp::MaxGt => 0.0,
        };
        r.to_bits()
    } else {
        let (x, y) = (f32::from_bits(a as u32), f32::from_bits(b as u32));
        let r = match op {
            FOp::Add => x + y,
            FOp::Sub => x - y,
            FOp::Mul => x * y,
            FOp::Div => x / y,
            FOp::Min => x.min(y),
            FOp::Max => x.max(y),
            FOp::MinLt | FOp::MaxGt => 0.0,
        };
        r.to_bits() as u64
    }
}

pub fn fold_fun(op: FUn, w: bool, a: u64) -> u64 {
    if w {
        let x = f64::from_bits(a);
        let r = match op {
            FUn::Neg => -x,
            FUn::Abs => x.abs(),
            FUn::Sqrt => x.sqrt(),
            FUn::Floor => x.floor(),
            FUn::Ceil => x.ceil(),
            FUn::Trunc => x.trunc(),
            FUn::RoundEven => x.round_ties_even(),
            FUn::RoundAway => x.round(),
        };
        r.to_bits()
    } else {
        let x = f32::from_bits(a as u32);
        let r = match op {
            FUn::Neg => -x,
            FUn::Abs => x.abs(),
            FUn::Sqrt => x.sqrt(),
            FUn::Floor => x.floor(),
            FUn::Ceil => x.ceil(),
            FUn::Trunc => x.trunc(),
            FUn::RoundEven => x.round_ties_even(),
            FUn::RoundAway => x.round(),
        };
        r.to_bits() as u64
    }
}

pub fn cmp_f(c: Cond, a: f64, b: f64) -> bool {
    match c {
        Cond::Eq => a == b,
        Cond::Ne => a != b,
        Cond::Lt => a < b,
        Cond::Le => a <= b,
        Cond::Gt => a > b,
        Cond::Ge => a >= b,
    }
}

pub fn f_to_int(x: f64, it: IntTy) -> i64 {
    let v = match (it.bits, it.signed) {
        (8, true) => x as i8 as i64,
        (8, false) => x as u8 as i64,
        (16, true) => x as i16 as i64,
        (16, false) => x as u16 as i64,
        (32, true) => x as i32 as i64,
        (32, false) => x as u32 as i64,
        (_, true) => x as i64,
        (_, false) => x as u64 as i64,
    };
    v
}

pub fn fold_conv(c: Conv, k: K) -> Option<K> {
    Some(match (c, k) {
        (Conv::IntToInt(it), K::I(v)) => K::I(ext(v, it)),
        (Conv::IntToInt(_), K::A(a)) => K::A(a),
        (Conv::IntToF(from, w), K::I(v)) => {
            let x = if from.bits == 64 && !from.signed { (v as u64) as f64 } else { v as f64 };
            if w {
                K::F(x.to_bits(), true)
            } else {
                // single rounding to f32
                let y = if from.bits == 64 && !from.signed { (v as u64) as f32 } else { v as f32 };
                K::F(y.to_bits() as u64, false)
            }
        }
        (Conv::FToInt(w, it), K::F(b, _)) => K::I(f_to_int(fval(b, w), it)),
        (Conv::F32ToF64, K::F(b, _)) => K::F((f32::from_bits(b as u32) as f64).to_bits(), true),
        (Conv::F64ToF32, K::F(b, _)) => K::F((f64::from_bits(b) as f32).to_bits() as u64, false),
        (Conv::BitsToF(w), K::I(v)) => K::F(if w { v as u64 } else { v as u32 as u64 }, w),
        (Conv::FToBits(w), K::F(b, _)) => K::I(if w { b as i64 } else { b as u32 as i64 }),
        _ => return None,
    })
}

pub fn swap_cond(c: Cond) -> Cond {
    match c {
        Cond::Lt => Cond::Gt,
        Cond::Le => Cond::Ge,
        Cond::Gt => Cond::Lt,
        Cond::Ge => Cond::Le,
        x => x,
    }
}

// VX and VX3 (including SHA) only define their result. No scalar constant folding
// or lane-wise algebra applies to the SHA operations.
pub fn is_pure(i: &Inst) -> bool {
    !is_mem_effect(i)
}

pub fn is_mem_effect(i: &Inst) -> bool {
    matches!(i, Inst::Call(..) | Inst::Store(..) | Inst::StoreX(..) | Inst::Copy(..) | Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) | Inst::FpEnv(..) | Inst::Cpuid(..) | Inst::Xgetbv(..) | Inst::VStore(..) | Inst::VStoreX(..))
}

pub fn const_inst(d: VReg, k: K) -> Inst {
    match k {
        K::I(v) => Inst::Iconst(d, v),
        K::F(b, w) => Inst::Fconst(d, b, w),
        K::A(a) => Inst::Addr(d, a),
    }
}

/// Folding, copy propagation, immediates, branch folding and DCE.
/// `ro` answers constant loads (symbol + offset) from immutable data: they fold to their value,
/// or to a symbolic address for a pointer word (`RoReader`).
/// Narrow integer results whose upper register bits nobody reads: per vreg, whether it has one
/// definition by a narrow (< 64-bit) Add / Sub / Mul / And / Or / Xor / Neg / Not / shift-left or
/// integer cast, and every use reads only its low `bits`: a store of at most that width, a narrow
/// wrapping operation (add, sub, mul, neg, not -- re-extended -- or an and / or / xor whose own
/// result reads only its low bits) of at most that width -- whose result's low bits depend only
/// on its operands' low bits --, a cast to at most that width, or (with at least 8
/// bits) the count of a 64-bit shift (the hardware reads the count's low 6 bits). The backends then skip the
/// result's re-extension (movzx / sxtw after every u8 / i32 operation): the bit reader's
/// `bits_left -= n` (u8, stored back as a byte) and its `entry as u8` (a shift count).
/// MAKEPAD_RUN_OPT=-lowbits: none (bisection).
pub fn low_bits_only(options: &relax_session::Options, f: &Func) -> Vec<bool> {
    let nv = f.vregs.len();
    if crate::stats::opt_off(options, "-lowbits") {
        return vec![false; nv];
    }
    let mut bits = vec![0u8; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    for b in &f.blocks {
        for i in &b.insts {
            crate::regalloc::for_defs(i, |d| ndef[d.0 as usize] += 1);
            match *i {
                Inst::IBin(IOp::Add | IOp::Sub | IOp::Mul | IOp::Shl | IOp::And | IOp::Or | IOp::Xor, it, d, _, _) | Inst::IBinI(IOp::Add | IOp::Sub | IOp::Mul | IOp::Shl | IOp::And | IOp::Or | IOp::Xor, it, d, _, _) | Inst::INeg(it, d, _) | Inst::INot(it, d, _) | Inst::Conv(Conv::IntToInt(it), d, _) if it.bits < 64 => bits[d.0 as usize] = it.bits,
                _ => {}
            }
        }
    }
    let mut ok: Vec<bool> = (0..nv).map(|v| bits[v] != 0 && ndef[v] == 1 && f.vregs[v] == Cls::I).collect();
    let narrow_wrap = |op: IOp| matches!(op, IOp::Add | IOp::Sub | IOp::Mul);
    // And / Or / Xor results are never re-extended (their operands' upper bits pass through to
    // the result: 64-bit forms on x64): an operand of one reads only its low bits when the result
    // does -- a fixed point, since clearing one result can clear its operands
    let bitwise = |op: IOp| matches!(op, IOp::And | IOp::Or | IOp::Xor);
    let mut tu = Vec::new();
    loop {
        let mut changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                // the uses that read only low bits: (vreg, how many)
                let mut low: [(u32, u8); 2] = [(u32::MAX, 0); 2];
                match *i {
                    Inst::Store(Mem::Int(n, _), base, _, v) if base != v => low[0] = (v.0, 8 * n),
                    Inst::StoreX(Mem::Int(n, _), base, idx, _, _, v) if base != v && idx != v => low[0] = (v.0, 8 * n),
                    Inst::IBin(op, it, d, x, y) if it.bits < 64 && (narrow_wrap(op) || bitwise(op) && ok[d.0 as usize]) => {
                        low[0] = (x.0, it.bits);
                        low[1] = (y.0, it.bits);
                    }
                    Inst::IBinI(op, it, d, x, _) if it.bits < 64 && (narrow_wrap(op) || bitwise(op) && ok[d.0 as usize]) => low[0] = (x.0, it.bits),
                    Inst::INeg(it, _, x) | Inst::INot(it, _, x) if it.bits < 64 => low[0] = (x.0, it.bits),
                    Inst::Conv(Conv::IntToInt(it), _, x) => low[0] = (x.0, it.bits),
                    // a 64-bit shift's count: the hardware reads its low 6 bits
                    Inst::IBin(IOp::Shl | IOp::Shr, IntTy { bits: 64, .. }, _, x, y) if x != y => low[0] = (y.0, 8),
                    _ => {}
                }
                crate::regalloc::for_uses(i, |u| {
                    let u = u.0 as usize;
                    if ok[u] && !low.iter().any(|&(v, n)| v as usize == u && n > 0 && n <= bits[u]) {
                        ok[u] = false;
                        changed = true;
                    }
                });
            }
            crate::regalloc::term_uses(&b.term, &mut tu);
            for u in &tu {
                ok[u.0 as usize] = false;
            }
        }
        if !changed {
            break;
        }
    }
    ok
}

/// A cast to `it` of a value whose register is zero above `z` bits (zero_above; 0 unknown) is the
/// value itself: a zero-extension from `it.bits` when z <= bits, a sign-extension only when z < bits
/// (bit bits - 1 is then zero too).
pub fn cast_is_move(z: u8, it: IntTy) -> bool {
    z > 0 && (z < it.bits || (!it.signed && z == it.bits))
}

/// Values whose register is known zero above some bit: per vreg (over all its definitions and copies), that bit count
/// (0: unknown): an unsigned narrow load (`ldrb` / movzx), an And with a non-negative mask, an
/// unsigned cast that keeps its extension (`low`: passes::low_bits_only, whose casts leave the
/// upper bits as they were). A cast of such a value to a wider type is a plain move: zero- and
/// sign-extension of a value whose upper bits are zero are the value itself (Paeth's
/// `i32::from(u8)` was ldrb + sxtw per pixel byte).
/// MAKEPAD_RUN_OPT=-zeroabove: none (bisection).
pub fn zero_above(options: &relax_session::Options, f: &Func, low: &[bool]) -> Vec<u8> {
    let nv = f.vregs.len();
    if crate::stats::opt_off(options, "-zeroabove") {
        return vec![0; nv];
    }
    // per vreg the widest of its definitions (0: some definition unknown, absorbing); copies take
    // their source's, to a fixed point (a loop variable copied around its loop: optimistic, as
    // every value enters the cycle from a definition outside it)
    const NONE: u8 = 255;
    let mut z = vec![NONE; nv];
    for p in &f.params {
        z[p.0 as usize] = 0;
    }
    let mut copies: Vec<(u32, u32)> = Vec::new();
    let merge = |z: &mut [u8], d: usize, k: u8| {
        z[d] = if z[d] == NONE { k } else if z[d] == 0 || k == 0 { 0 } else { z[d].max(k) };
    };
    for b in &f.blocks {
        for i in &b.insts {
            match *i {
                Inst::Load(Mem::Int(n, false), d, _, _) | Inst::LoadX(Mem::Int(n, false), d, _, _, _, _) if n < 8 => merge(&mut z, d.0 as usize, 8 * n),
                Inst::IBinI(IOp::And, _, d, _, m) if m >= 0 => merge(&mut z, d.0 as usize, (64 - (m as u64).leading_zeros()).max(1) as u8),
                Inst::Conv(Conv::IntToInt(IntTy { bits, signed: false }), d, _) if bits < 64 && !low[d.0 as usize] => merge(&mut z, d.0 as usize, bits),
                Inst::Iconst(d, k) if k >= 0 && f.vregs[d.0 as usize] == Cls::I => merge(&mut z, d.0 as usize, (64 - (k as u64).leading_zeros()).max(1) as u8),
                Inst::Mov(d, s) if f.vregs[d.0 as usize] == Cls::I => copies.push((d.0, s.0)),
                _ => crate::regalloc::for_defs(i, |d| z[d.0 as usize] = 0),
            }
        }
    }
    // a worklist over the copies by source: each change re-merges the copies of that value only
    // (linear-ish; a sweep to a fixed point was quadratic in long copy chains)
    let mut by_src: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for &(d, s) in &copies {
        by_src.entry(s).or_default().push(d);
    }
    let mut work: Vec<u32> = copies.iter().map(|c| c.1).collect();
    while let Some(s) = work.pop() {
        let k = z[s as usize];
        if k == NONE {
            continue;
        }
        if let Some(ds) = by_src.get(&s) {
            for &d in ds {
                let old = z[d as usize];
                merge(&mut z, d as usize, k);
                if z[d as usize] != old {
                    work.push(d);
                }
            }
        }
    }
    // MAKEPAD_RUN_OPT=-zeromulti: single-definition values only, no copies (bisection)
    let single = crate::stats::opt_off(options, "-zeromulti");
    let mut ndef = vec![0u32; if single { nv } else { 0 }];
    if single {
        for p in &f.params {
            ndef[p.0 as usize] += 2;
        }
        for b in &f.blocks {
            for i in &b.insts {
                crate::regalloc::for_defs(i, |d| ndef[d.0 as usize] += 1);
                if let Inst::Mov(d, _) | Inst::Iconst(d, _) = *i {
                    ndef[d.0 as usize] += 2;
                }
            }
        }
    }
    for v in 0..nv {
        if z[v] == NONE || z[v] >= 64 || f.vregs[v] != Cls::I || (single && ndef[v] != 1) {
            z[v] = 0;
        }
    }
    z
}

/// Renumbers the vregs a function still mentions densely (0..n): lowering allocates many that
/// later passes delete, and every pass sizes its per-vreg tables by `f.vregs.len()` -- at
/// twice the instruction count that table work dominated the optimiser.
#[inline(never)]
pub fn compact_vregs(f: &mut Func) {
    let nv = f.vregs.len();
    const NONE: u32 = u32::MAX;
    let mut map = vec![NONE; nv];
    let mut cls = Vec::new();
    let mut n = 0u32;
    let mut note = |v: VReg, map: &mut Vec<u32>, cls: &mut Vec<Cls>, vregs: &Vec<Cls>| {
        let i = v.0 as usize;
        if i < map.len() && map[i] == NONE {
            map[i] = n;
            cls.push(vregs[i]);
            n += 1;
        }
    };
    let mut uses = Vec::new();
    for p in &f.params {
        note(*p, &mut map, &mut cls, &f.vregs);
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |v| note(v, &mut map, &mut cls, &f.vregs));
            for_defs(i, |v| note(v, &mut map, &mut cls, &f.vregs));
        }
        term_uses(&b.term, &mut uses);
        for v in &uses {
            note(*v, &mut map, &mut cls, &f.vregs);
        }
    }
    for v in f.noalias.iter().chain(&f.withheld_noalias).chain(&f.owned) {
        note(*v, &mut map, &mut cls, &f.vregs);
    }
    for (v, _) in &f.deref {
        note(*v, &mut map, &mut cls, &f.vregs);
    }
    if (n as usize) == nv {
        return;
    }
    let m = |v: VReg| VReg(map[v.0 as usize]);
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            i.vregs_mut(|v| *v = VReg(map[v.0 as usize]));
        }
        b.term = match &b.term {
            Term::Branch(c, x, y) => Term::Branch(m(*c), *x, *y),
            Term::Ret(v) => Term::Ret(v.iter().map(|x| m(*x)).collect()),
            other => other.clone(),
        };
    }
    for p in f.params.iter_mut() {
        *p = m(*p);
    }
    if let Some(p) = f.sret {
        f.sret = Some(m(p));
    }
    for p in f.noalias.iter_mut().chain(f.withheld_noalias.iter_mut()).chain(f.owned.iter_mut()) {
        *p = m(*p);
    }
    for p in f.deref.iter_mut() {
        p.0 = m(p.0);
    }
    f.nonnull.retain(|v| (v.0 as usize) < map.len() && map[v.0 as usize] != NONE);
    for p in f.nonnull.iter_mut() {
        *p = m(*p);
    }
    f.ranged.retain(|r| (r.0 .0 as usize) < map.len() && map[r.0 .0 as usize] != NONE);
    for r in f.ranged.iter_mut() {
        r.0 = m(r.0);
    }
    // (the debug table follows; a vreg no instruction mentions is gone: its leaf is no longer held)
    f.dbg_map_vregs(&|v| map.get(v.0 as usize).filter(|x| **x != NONE).map(|x| VReg(*x)));
    f.vregs = cls;
}

// OnceLock cannot be reset through a shared static; see this crate's zone_take. The string is
// leaked in the kept heap: it outlives every compiler generation (a settle swap destroys the
// compiler phase's allocations).
pub static T: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();

pub fn trace_opt(f: &Func, tag: &str) {
    let t = T.get_or_init(|| {
        let _k = relax_session::phase::kept();
        std::env::var("MAKEPAD_RUN_TRACE_OPT").ok().map(|s| &*Box::leak(s.into_boxed_str()))
    });
    if let Some(t) = t {
        if f.name.contains(*t) {
            eprintln!("=== {} {}\n{}", tag, f.name, f.dump());
        }
    }
}

/// Lane-op peepholes for intrinsic chains: redundant mask expansion, widening,
/// and vectors round-tripped through scalar registers.
pub fn vx_peep(f: &mut Func, ndef: &[u32]) {
    let nv = f.vregs.len();
    let masks = vector_byte_masks(f);
    let mut zero = vec![false; nv];
    let mut def: Vec<Option<(VX, VReg)>> = vec![None; nv];
    // single-def scalar results (zero-extended lanes) and their lane width
    let mut lane_bits = vec![0u8; nv];
    // single-def integer constants
    let mut ik: Vec<Option<i64>> = vec![None; nv];
    // single-def bit patterns of a single-def f32 (`x.to_bits()`, zero-extended): the float
    let mut f32_bits: Vec<Option<VReg>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            match *i {
                Inst::Conv(Conv::FToBits(false), d, x) if ndef[d.0 as usize] == 1 && ndef[x.0 as usize] == 1 => f32_bits[d.0 as usize] = Some(x),
                Inst::Conv(Conv::IntToInt(t), d, x) if ndef[d.0 as usize] == 1 && !t.signed && t.bits >= 32 => f32_bits[d.0 as usize] = f32_bits[x.0 as usize],
                _ => {}
            }
            if let Inst::VConst(d, 0) = i {
                zero[d.0 as usize] = ndef[d.0 as usize] == 1;
            }
            if let Inst::Iconst(d, k) = i {
                if ndef[d.0 as usize] == 1 {
                    ik[d.0 as usize] = Some(*k);
                }
            }
            if let Inst::VX(op, d, a, _) = i {
                if ndef[d.0 as usize] == 1 && ndef[a.0 as usize] == 1 {
                    def[d.0 as usize] = Some((*op, *a));
                }
                if ndef[d.0 as usize] == 1 {
                    if let VX::Extract(w, _) | VX::AddAcross(w) | VX::MaxAcrossU(w) = op {
                        lane_bits[d.0 as usize] = w.bits() as u8;
                    }
                }
            }
        }
    }
    // vectors whose every use reads only their low 64 bits (lane extracts there), and the lane-wise
    // results only such extracts read: the operands of those may be any vector with the same low
    // half (NEON 8-byte group compares widened to 16-byte vectors: `FromLo64(Extract(D, 0, v))`)
    let mut uses = vec![0u32; nv];
    let mut low_uses = vec![0u32; nv];
    for b in &f.blocks {
        for i in &b.insts {
            crate::regalloc::for_uses(i, |u| uses[u.0 as usize] += 1);
            if let Inst::VX(VX::Extract(w, lane), _, a, _) = *i {
                if (lane as u32 + 1) * w.bits() <= 64 {
                    low_uses[a.0 as usize] += 1;
                }
            }
        }
        let mut t = Vec::new();
        crate::regalloc::term_uses(&b.term, &mut t);
        for u in t {
            uses[u.0 as usize] += 1;
        }
    }
    let low_only = |v: VReg| uses[v.0 as usize] > 0 && uses[v.0 as usize] == low_uses[v.0 as usize];
    // the vector whose low 64 bits `x` holds
    let low = |x: VReg| -> VReg {
        if let Some((VX::FromLo64, t)) = def[x.0 as usize] {
            if let Some((VX::Extract(VW::D, 0), y)) = def[t.0 as usize] {
                return y;
            }
        }
        x
    };
    for b in &mut f.blocks {
        for i in &mut b.insts {
            // the splat of a constant whose lanes repeat a 32-bit word: a pool constant (zero:
            // xorps / movi), not a GPR move and a shuffle (`_mm_setzero_ps`, `_mm_set1_epi8(k)`)
            if let Inst::VX(VX::Splat(w), d, a, _) = *i {
                if let Some(k) = ik[a.0 as usize] {
                    let k = k as u64;
                    let word = match w {
                        VW::B => Some((k & 0xff) as u32 * 0x0101_0101),
                        VW::H => Some((k & 0xffff) as u32 * 0x0001_0001),
                        VW::S => Some(k as u32),
                        VW::D => ((k >> 32) as u32 == k as u32).then_some(k as u32),
                    };
                    if let Some(word) = word {
                        *i = Inst::VConst(d, word);
                        continue;
                    }
                }
            }
            // a splat of a float's bits splats the float register itself (no round trip
            // through an integer register): `_mm_set1_ps(x)`, `vdupq_n_f32(x)`
            if let Inst::VX(VX::Splat(VW::S), d, a, _) = *i {
                if let Some(x) = f32_bits[a.0 as usize] {
                    *i = Inst::VSplat(d, x);
                    continue;
                }
            }
            if let Inst::VX(VX::CmpGtS(VW::B), d, a, b) = *i {
                // Expanding an already canonical comparison mask's sign bits
                // is an identity (the lowering of blendv_epi8 does this).
                if zero[a.0 as usize] && masks[b.0 as usize] {
                    *i = Inst::Mov(d, b);
                    continue;
                }
            }
            // the zero extension a lane extract already did
            if let Inst::Conv(Conv::IntToInt(t), d, x) = *i {
                let lb = lane_bits[x.0 as usize];
                if lb != 0 && !t.signed && lb <= t.bits {
                    *i = Inst::Mov(d, x);
                }
                continue;
            }
            if let Inst::VX(op, d, a, bb) = i {
                match *op {
                    VX::WidenLoU(w) | VX::WidenLoS(w) => {
                        let y = low(*a);
                        let signed = matches!(op, VX::WidenLoS(_));
                        if let Some((VX::BytesShrI(8), z)) = def[y.0 as usize] {
                            *op = if signed { VX::WidenHiS(w) } else { VX::WidenHiU(w) };
                            *a = z;
                        } else {
                            *a = y;
                        }
                        *bb = *a;
                    }
                    VX::ZipLo(VW::D) => {
                        *a = low(*a);
                        *bb = low(*bb);
                        // the low halves of two lane shuffles of one vector: one shuffle
                        // (`_mm_shuffle_ps(a, a, m)` = pshufd a, m; cross products, dot splats)
                        let src = |v: VReg| match def[v.0 as usize] {
                            Some((VX::ShufS(k), x)) => (x, k),
                            _ => (v, 0b11_10_01_00),
                        };
                        let ((x, k1), (y, k2)) = (src(*a), src(*bb));
                        if x == y {
                            *op = VX::ShufS((k1 & 0x0f) | ((k2 & 0x0f) << 4));
                            *a = x;
                            *bb = x;
                        }
                    }
                    VX::Extract(w, lane) if (lane as u32 + 1) * w.bits() <= 64 => {
                        *a = low(*a);
                        *bb = *a;
                        // lane 0 of [x, 0] is x
                        if let (VX::Extract(VW::D, 0), Some((VX::FromLo64, x))) = (*op, def[a.0 as usize]) {
                            let d = *d;
                            *i = Inst::Mov(d, x);
                            continue;
                        }
                    }
                    // lane-wise ops whose result is only read in its low half: so are their operands
                    VX::CmpEq(_) | VX::CmpGtS(_) | VX::And | VX::Or | VX::Xor | VX::AndNot | VX::Add(_) | VX::Sub(_) if low_only(*d) => {
                        *a = low(*a);
                        *bb = low(*bb);
                    }
                    _ => {}
                }
            }
        }
    }
}

// Deferred edits keep the definition tables and the current folding walk's indices valid.
pub struct FoldEdit {
    pub block: usize,
    pub at: usize,
    pub remove: usize,
    pub insts: Vec<Inst>,
    pub pos: u64,
}

pub fn fold_insert(f: &Func, edits: &mut Vec<FoldEdit>, block: usize, after: usize, inst: Inst) {
    edits.push(FoldEdit { block, at: after + 1, remove: 0, insts: vec![inst], pos: f.blocks[block].pos[after] });
}

/// The one compare equal to `k1 || k2` for two compares of the same operands (same signedness;
/// the second may name them swapped), when there is one (`Lt | Eq` = `Le`, `Lt | Gt` = `Ne`).
fn cond_union(k1: (Cond, bool, VReg, i64, bool), k2: (Cond, bool, VReg, i64, bool)) -> Option<Cond> {
    let (c1, s1, a1, b1, i1) = k1;
    let (c2, s2, a2, b2, i2) = k2;
    if s1 != s2 || i1 != i2 {
        return None;
    }
    // outcome sets over {less, equal, greater} of `a ? b`
    let set = |c: Cond| -> u8 {
        match c {
            Cond::Lt => 0b100,
            Cond::Eq => 0b010,
            Cond::Gt => 0b001,
            Cond::Le => 0b110,
            Cond::Ge => 0b011,
            Cond::Ne => 0b101,
        }
    };
    let swap = |m: u8| (m & 0b010) | ((m & 0b100) >> 2) | ((m & 0b001) << 2);
    let m2 = if a2 == a1 && b2 == b1 {
        set(c2)
    } else if !i1 && a2.0 as i64 == b1 && b2 == a1.0 as i64 {
        swap(set(c2))
    } else {
        return None;
    };
    match set(c1) | m2 {
        0b100 => Some(Cond::Lt),
        0b010 => Some(Cond::Eq),
        0b001 => Some(Cond::Gt),
        0b110 => Some(Cond::Le),
        0b011 => Some(Cond::Ge),
        0b101 => Some(Cond::Ne),
        _ => None,
    }
}

/// All definitions must be constants or copies of such values. Params and copy cycles
/// fail too; at most four copy edges are followed.
pub fn const_phi(f: &Func, def_at: &[Vec<(usize, usize)>], ndef: &[u32], v: VReg, depth: u32) -> bool {
    let Some(ds) = def_at.get(v.0 as usize) else { return false };
    if depth > 4 || ds.is_empty() || ds.len() != ndef[v.0 as usize] as usize {
        return false;
    }
    ds.iter().all(|&(bi, ii)| match f.blocks[bi].insts[ii] {
        Inst::Iconst(..) => true,
        Inst::Mov(_, x) => const_phi(f, def_at, ndef, x, depth + 1),
        Inst::Sel(_, _, a, b) => sel_const(f, def_at, ndef, a).is_some() && sel_const(f, def_at, ndef, b).is_some(),
        _ => false,
    })
}

/// The constant a select operand holds: a single-def vreg defined by an Iconst.
fn sel_const(f: &Func, def_at: &[Vec<(usize, usize)>], ndef: &[u32], v: VReg) -> Option<i64> {
    let ds = def_at.get(v.0 as usize)?;
    if ds.len() != 1 || ndef[v.0 as usize] != 1 {
        return None;
    }
    match f.blocks[ds[0].0].insts[ds[0].1] {
        Inst::Iconst(_, k) => Some(k),
        _ => None,
    }
}

// Targets of DFS back edges, as in loop_depths. These include natural loop headers
// and conservatively cover cycles in irreducible flow too; block order is irrelevant.
pub fn loop_headers(f: &Func, depth: &[u32]) -> Vec<bool> {
    let mut header = vec![false; f.blocks.len()];
    if depth.iter().all(|&d| d == 0) { return header; }
    let mut state = vec![0u8; f.blocks.len()];
    let mut work = vec![(0usize, false)];
    while let Some((b, done)) = work.pop() {
        if done {
            state[b] = 2;
            continue;
        }
        match state[b] {
            1 => header[b] = true,
            2 => {}
            _ => {
                state[b] = 1;
                work.push((b, true));
                let at = work.len();
                work.extend(f.blocks[b].term.succs().map(|s| (s as usize, false)));
                work[at..].reverse();
            }
        }
    }
    header
}

// In a loop, every definition must precede the read within this iteration: no
// path from a definition may reach a loop header without first passing the read.
// Header reads/defs and definitions after a same-block read are left alone.
pub fn defs_in_same_iteration(f: &Func, depth: &[u32], header: &[bool], defs: &[(usize, usize)], at: (usize, usize)) -> bool {
    if defs.iter().any(|&(b, _)| depth[b] != depth[at.0]) { return false; }
    if depth[at.0] == 0 { return true; }
    if header[at.0] || defs.iter().any(|&(b, i)| header[b] || (b == at.0 && i >= at.1)) { return false; }
    let mut seen = vec![false; f.blocks.len()];
    seen[at.0] = true;
    let mut work: Vec<usize> = defs.iter().map(|&(b, _)| b).collect();
    while let Some(b) = work.pop() {
        if seen[b] { continue; }
        if header[b] { return false; }
        seen[b] = true;
        work.extend(f.blocks[b].term.succs().map(|s| s as usize));
    }
    true
}

// const_phi has already bounded/validated the copy graph. Check each copied value
// at its read, since a copy can itself capture a value from the previous iteration.
pub fn phi_in_same_iteration(
    f: &Func, def_at: &[Vec<(usize, usize)>], kn: &[Option<K>], v: VReg,
    at: (usize, usize), depth: &[u32], header: &[bool],
) -> bool {
    let defs = &def_at[v.0 as usize];
    defs_in_same_iteration(f, depth, header, defs, at) && defs.iter().all(|&(b, i)| {
        match f.blocks[b].insts[i] {
            // A known constant needs no boolean shadow at its source definition.
            Inst::Mov(_, x) if !matches!(kn[x.0 as usize], Some(K::I(_))) => {
                phi_in_same_iteration(f, def_at, kn, x, (b, i), depth, header)
            }
            _ => true,
        }
    })
}

/// Shadow each assignment with its comparison result. A copy of another multi-def
/// value copies its boolean shadow at the same point, preserving the value it read.
pub fn phi_compare(
    f: &mut Func, def_at: &[Vec<(usize, usize)>], kn: &[Option<K>], v: VReg,
    cc: Cond, signed: bool, k: i64, narrow: Option<IntTy>,
    shadows: &mut Vec<(VReg, VReg)>, edits: &mut Vec<FoldEdit>,
) -> VReg {
    if let Some(&(_, b)) = shadows.iter().find(|(x, _)| *x == v) {
        return b;
    }
    let b = f.vreg(Cls::I);
    shadows.push((v, b));
    let cmp = |x| cmp_i(cc, signed, narrow.map_or(x, |t| ext(x, t)), k) as i64;
    for &(bi, ii) in &def_at[v.0 as usize] {
        let inst = match f.blocks[bi].insts[ii] {
            Inst::Iconst(_, x) => Inst::Iconst(b, cmp(x)),
            Inst::Mov(_, x) => match kn[x.0 as usize] {
                Some(K::I(kx)) => Inst::Iconst(b, cmp(kx)),
                _ => Inst::Mov(b, phi_compare(f, def_at, kn, x, cc, signed, k, narrow, shadows, edits)),
            },
            // c ? ka : kb compares as c ? cmp(ka) : cmp(kb) (read where the select read c)
            Inst::Sel(_, c, x, y) => {
                // const_phi checked both operands: single-def, defined by an Iconst
                let konst = |v: VReg| match def_at[v.0 as usize].first().map(|&(b2, i2)| &f.blocks[b2].insts[i2]) {
                    Some(Inst::Iconst(_, k)) => *k,
                    _ => unreachable!(),
                };
                let (kx, ky) = (konst(x), konst(y));
                let (tx, ty) = (f.vreg(Cls::I), f.vreg(Cls::I));
                let pos = f.blocks[bi].pos[ii];
                edits.push(FoldEdit { block: bi, at: ii + 1, remove: 0, insts: vec![Inst::Iconst(tx, cmp(kx)), Inst::Iconst(ty, cmp(ky)), Inst::Sel(b, c, tx, ty)], pos });
                continue;
            }
            _ => unreachable!(),
        };
        fold_insert(f, edits, bi, ii, inst);
    }
    b
}

pub fn unchanged_between(f: &Func, ndef: &[u32], v: VReg, from: (usize, usize), to: (usize, usize)) -> bool {
    if ndef[v.0 as usize] == 1 {
        return true;
    }
    if from.0 != to.0 || from.1 >= to.1 {
        return false;
    }
    !f.blocks[from.0].insts[from.1 + 1..to.1].iter().any(|i| {
        let mut writes = false;
        for_defs(i, |d| writes |= d == v);
        writes
    })
}

/// A vreg defined only by `SlotAddr`s of one slot, more than once (inlining and sinking
/// duplicate a slot address into each path), holds one value everywhere: it gets a single
/// definition at the entry instead. The slot passes then follow it (slot_scan reads a slot
/// address only through single-def vregs and takes a multi-def one as escaping: reconcile's
/// 152-byte `Option<MirrorEntry>` temporaries were never forwarded) and the allocator
/// rematerialises it. Not in baseline bodies, whose SlotAddr positions mark slot lifetimes
/// (Slot::born), nor for call arguments, which the allocator does not rematerialise (see
/// unify_slot_addrs).
pub fn merge_slot_addr_defs(f: &mut Func) -> bool {
    if f.baseline || f.blocks.is_empty() {
        return false;
    }
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut nsa = vec![0u32; nv];
    // the slot of a vreg's SlotAddr defs (u32::MAX: none yet, u32::MAX - 1: several slots)
    let mut slot = vec![u32::MAX; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Call(_, args, _) = i {
                for a in args {
                    nsa[a.0 as usize] = u32::MAX / 2;
                }
            }
            if let Inst::SlotAddr(d, s2) = i {
                let d = d.0 as usize;
                nsa[d] += 1;
                slot[d] = if slot[d] == u32::MAX || slot[d] == *s2 { *s2 } else { u32::MAX - 1 };
            }
        }
    }
    let one = |v: usize| ndef[v] > 1 && nsa[v] == ndef[v] && slot[v] < u32::MAX - 1;
    if !(0..nv).any(one) {
        return false;
    }
    let mut hoist: Vec<(VReg, u32)> = Vec::new();
    for b in f.blocks.iter_mut() {
        let mut k = 0;
        let keep: Vec<bool> = b.insts.iter().map(|i| !matches!(i, Inst::SlotAddr(d, _) if one(d.0 as usize))).collect();
        for i in &b.insts {
            if let Inst::SlotAddr(d, s2) = i {
                if one(d.0 as usize) && !hoist.iter().any(|h| h.0 == *d) {
                    hoist.push((*d, *s2));
                }
            }
        }
        b.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        let mut k2 = 0;
        b.pos.retain(|_| {
            k2 += 1;
            keep.get(k2 - 1).copied().unwrap_or(true)
        });
    }
    let p0 = f.blocks[0].pos.first().copied().unwrap_or(f.blocks[0].term_pos);
    let n = hoist.len();
    f.blocks[0].insts.splice(0..0, hoist.iter().map(|&(d, s2)| Inst::SlotAddr(d, s2)));
    f.blocks[0].pos.splice(0..0, std::iter::repeat(p0).take(n));
    true
}

pub fn simplify(options: &relax_session::Options, f: &mut Func, ro: Option<RoReader>, imm: bool, cse: bool) {
    let tc = pass_clock();
    compact_vregs(f);
    merge_slot_addr_defs(f);
    pass_time(14, tc);
    trace_opt(f, "simplify-start");
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let ic = !opt_off(options, "-ic");
    let (ic_conv, ic_phi, ic_sink, ic_nn) = (ic && !opt_off(options, "-icconv"), ic && !opt_off(options, "-icphi"), ic && !opt_off(options, "-icsink"), ic && !opt_off(options, "-icnn"));
    // two rounds: a third and fourth changed code size by < 0.1% on the counter (6,941 vs
    // 6,945 KB) at ~25% of the optimiser's time
    for _round in 0..2 {
        // passes in a round may add vregs (div_by_const)
        let nv = f.vregs.len();
        let tf = pass_clock();
        let reach = reachable(f);
        let mut has_vx = false;
        // definition counts (params count as a definition)
        let mut ndef = vec![0u32; nv];
        for p in &f.params {
            ndef[p.0 as usize] += 1;
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                for_defs(i, |d| {
                    ndef[d.0 as usize] += 1;
                });
                has_vx |= matches!(i, Inst::VX(..));
            }
        }
        if has_vx {
            vx_peep(f, &ndef);
        }
        let single = |v: VReg, ndef: &Vec<u32>| ndef[v.0 as usize] == 1;
        // known constants and copy aliases of single-def vregs
        // vregs every definition of which is the same integer constant (an Option's `None` tag,
        // built on several paths): known like a single definition
        let mut same_k: Vec<Option<(i64, u32)>> = vec![None; nv];
        let mut kn: Vec<Option<K>> = vec![None; nv];
        let mut alias: Vec<u32> = (0..nv as u32).collect();
        let mut def_inst: Vec<Option<(IOp, VReg, i64)>> = vec![None; nv];
        // single-def integer compares: (cond, signed, a, b-or-imm, is_imm); operands must be
        // single-def too so the copy reads the same values
        let mut cmp_def: Vec<Option<(Cond, bool, VReg, i64, bool)>> = vec![None; nv];
        // single-def selects of single-def operands: (cond, a, b)
        let mut sel_def: Vec<Option<(VReg, VReg, VReg)>> = vec![None; nv];
        // `d = a ^ 1` (single defs): a compare outcome's negation, seen through by the select-tree fold
        let mut xor1_def: Vec<Option<VReg>> = vec![None; nv];
        // IntToInt definitions also retain their source location: RIR operands can be
        // reassigned between the inner conversion and its use.
        let mut conv_def: Vec<Option<(IntTy, VReg, usize, usize)>> = vec![None; if ic { nv } else { 0 }];
        let mut bin_def: Vec<Option<(IOp, IntTy, VReg, i64, usize, usize)>> = vec![None; if ic { nv } else { 0 }];
        let mut phi_defs = vec![Vec::new(); if ic { nv } else { 0 }];
        // uses per vreg (profitability of the phi-compare fold: only when the phi dies)
        let mut phi_nuse = vec![0u32; if ic_phi { nv } else { 0 }];
        // Only build loop information when a constant-phi compare is otherwise eligible.
        let mut phi_loops = None;
        if ic_phi {
            let mut tu = Vec::new();
            for b in &f.blocks {
                for i in &b.insts {
                    for_uses(i, |u| phi_nuse[u.0 as usize] += 1);
                }
                term_uses(&b.term, &mut tu);
                for u in &tu {
                    phi_nuse[u.0 as usize] += 1;
                }
            }
        }
        // values known to fit in the low N bits (zero above), every definition counted (a loop
        // variable assigned only masked values stays narrow): 0 = no definition seen yet,
        // 64 = unknown; a fixpoint below
        let mut zw: Vec<u8> = vec![0; nv];
        for p in &f.params {
            zw[p.0 as usize] = 64;
        }
        // vregs read only by arithmetic and stores (not by copies, compares, selects or
        // terminators, where a select of constants feeds the Ordering / phi folds): a 0/1 select
        // of them becomes its condition
        let mut arith_read = vec![true; nv];
        {
            let mut tu = Vec::new();
            for b in &f.blocks {
                for i in &b.insts {
                    if matches!(i, Inst::Mov(..) | Inst::ICmp(..) | Inst::ICmpI(..) | Inst::Sel(..)) {
                        for_uses(i, |u| arith_read[u.0 as usize] = false);
                    }
                }
                term_uses(&b.term, &mut tu);
                for u in &tu {
                    arith_read[u.0 as usize] = false;
                }
            }
        }
        // single-def function addresses (devirtualised calls)
        let mut fn_def: Vec<u32> = vec![u32::MAX; nv];
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for (ii, i) in b.insts.iter().enumerate() {
                if let Inst::Iconst(d, v) = i {
                    same_k[d.0 as usize] = match same_k[d.0 as usize] {
                        None => Some((*v, 1)),
                        Some((w, n)) if w == *v && n != u32::MAX => Some((w, n + 1)),
                        // two different constants: never known
                        Some(_) => Some((0, u32::MAX)),
                    };
                }
                if ic {
                    match *i {
                        Inst::Iconst(d, _) | Inst::Mov(d, _) => phi_defs[d.0 as usize].push((bi, ii)),
                        // a choice between two constants (an `Ordering` from `a == b`): an
                        // assignment of the phi machinery too (const_phi / phi_compare)
                        Inst::Sel(d, ..) => phi_defs[d.0 as usize].push((bi, ii)),
                        Inst::Conv(Conv::IntToInt(t), d, x) if single(d, &ndef) => conv_def[d.0 as usize] = Some((t, x, bi, ii)),
                        Inst::IBinI(op @ (IOp::Add | IOp::Sub | IOp::And), t, d, x, k) if single(d, &ndef) => bin_def[d.0 as usize] = Some((op, t, x, k, bi, ii)),
                        _ => {}
                    }
                }
                if let Inst::FnAddr(d, id) = i {
                    if single(*d, &ndef) {
                        fn_def[d.0 as usize] = *id;
                    }
                }
                match i {
                    Inst::ICmpI(c, sg, d, a, k) if single(*d, &ndef) && single(*a, &ndef) => cmp_def[d.0 as usize] = Some((*c, *sg, *a, *k, true)),
                    Inst::ICmp(c, sg, d, a, b2) if single(*d, &ndef) && single(*a, &ndef) && single(*b2, &ndef) => cmp_def[d.0 as usize] = Some((*c, *sg, *a, b2.0 as i64, false)),
                    Inst::Sel(d, c, a, b2) if single(*d, &ndef) && single(*c, &ndef) && single(*a, &ndef) && single(*b2, &ndef) => sel_def[d.0 as usize] = Some((*c, *a, *b2)),
                    Inst::IBinI(IOp::Xor, _, d, a, 1) if single(*d, &ndef) && single(*a, &ndef) => xor1_def[d.0 as usize] = Some(*a),
                    _ => {}
                }
            }
        }
        // narrow widths: monotone, a few sweeps
        let mut settled = false;
        for _ in 0..8 {
            let mut grew = false;
            for (bi, b) in f.blocks.iter().enumerate() {
                if !reach[bi] {
                    continue;
                }
                for i in &b.insts {
                    let w = |v: &VReg, zw: &Vec<u8>| zw[v.0 as usize];
                    let mut set = |d: VReg, b: u8, zw: &mut Vec<u8>| {
                        if b > zw[d.0 as usize] {
                            zw[d.0 as usize] = b;
                            grew = true;
                        }
                    };
                    match i {
                        Inst::Load(Mem::Int(n, false), d, ..) | Inst::LoadX(Mem::Int(n, false), d, ..) => set(*d, (*n * 8).min(64), &mut zw),
                        Inst::Conv(Conv::IntToInt(t), d, x) if !t.signed => {
                            let b = t.bits.min(w(x, &zw).max(1));
                            set(*d, if w(x, &zw) == 0 { 0 } else { b }, &mut zw)
                        }
                        Inst::IBinI(IOp::And, _, d, x, m) if *m >= 0 => {
                            let b = ((64 - (*m as u64).leading_zeros()) as u8).min(w(x, &zw));
                            set(*d, b, &mut zw)
                        }
                        Inst::IBinI(IOp::Shr, it, d, x, k) if !it.signed && it.bits == 64 && (0..64).contains(k) => {
                            let b = (64 - *k as u8).min(w(x, &zw));
                            set(*d, b, &mut zw)
                        }
                        Inst::IBinI(IOp::Xor | IOp::Or, _, d, x, k) if *k >= 0 => {
                            let b = ((64 - (*k as u64).leading_zeros()) as u8).max(w(x, &zw));
                            set(*d, b, &mut zw)
                        }
                        Inst::Sel(d, _, a, b2) => {
                            let b = w(a, &zw).max(w(b2, &zw));
                            set(*d, b, &mut zw)
                        }
                        Inst::Iconst(d, v) if *v >= 0 => set(*d, (64 - (*v as u64).leading_zeros()) as u8, &mut zw),
                        Inst::ICmp(_, _, d, ..) | Inst::ICmpI(_, _, d, ..) | Inst::FCmp(_, _, d, ..) => set(*d, 1, &mut zw),
                        Inst::Mov(d, x) => {
                            let b = w(x, &zw);
                            set(*d, b, &mut zw)
                        }
                        _ => for_defs(i, |d| set(d, 64, &mut zw)),
                    }
                }
            }
            if !grew {
                settled = true;
                break;
            }
        }
        if !settled {
            // still growing: nothing is known
            zw.iter_mut().for_each(|x| *x = 64);
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                match i {
                    Inst::Iconst(d, v) if single(*d, &ndef) || matches!(same_k[d.0 as usize], Some((w, n)) if w == *v && n == ndef[d.0 as usize]) => kn[d.0 as usize] = Some(K::I(*v)),
                    Inst::Fconst(d, v, w) if single(*d, &ndef) => kn[d.0 as usize] = Some(K::F(*v, *w)),
                    Inst::Addr(d, a) if single(*d, &ndef) => kn[d.0 as usize] = Some(K::A(*a)),
                    Inst::Mov(d, s) if single(*d, &ndef) && single(*s, &ndef) => alias[d.0 as usize] = s.0,
                    Inst::IBinI(IOp::Add, _, d, a, k) if single(*d, &ndef) && single(*a, &ndef) => def_inst[d.0 as usize] = Some((IOp::Add, *a, *k)),
                    _ => {}
                }
            }
        }
        // resolve alias chains
        for v in 0..nv {
            let mut r = alias[v];
            let mut guard = 0;
            while alias[r as usize] != r && guard < 64 {
                r = alias[r as usize];
                guard += 1;
            }
            alias[v] = r;
        }
        for v in 0..nv {
            if alias[v] != v as u32 && kn[v].is_none() {
                kn[v] = kn[alias[v] as usize];
            }
        }
        if !f.dbg.is_empty() {
            // the debug table follows the renaming (and keeps folded constants)
            let named = f.dbg_vregs();
            let copies: Vec<(VReg, VReg)> = named.iter().filter(|&&v| (v as usize) < nv && alias[v as usize] != v).map(|&v| (VReg(v), VReg(alias[v as usize]))).collect();
            let consts: Vec<(VReg, i64)> = named.iter().filter(|&&v| (v as usize) < nv && single(VReg(v), &ndef)).filter_map(|&v| match kn[v as usize] {
                Some(K::I(k)) => Some((VReg(v), k)),
                Some(K::F(bits, _)) => Some((VReg(v), bits as i64)),
                _ => None,
            }).collect();
            f.dbg_salvage(&copies, &consts);
        }
        let mut changed = false;
        let mut edits = Vec::new();
        // memmoves of a known small size, expanded after the walk: (block, index, size)
        let mut moves: Vec<(usize, usize, u32)> = Vec::new();
        let a = |v: VReg| VReg(alias[v.0 as usize]);
        for bi in 0..f.blocks.len() {
            if !reach[bi] {
                continue;
            }
            for ii in 0..f.blocks[bi].insts.len() {
                // skip instructions with nothing to rename or fold (most of them)
                uses_defs(&f.blocks[bi].insts[ii], &mut uses, &mut defs);
                let mut interesting = false;
                for u in &uses {
                    if alias[u.0 as usize] != u.0 || kn[u.0 as usize].is_some() || def_inst[u.0 as usize].is_some() {
                        interesting = true;
                        break;
                    }
                }
                if !interesting && !matches!(f.blocks[bi].insts[ii], Inst::IBin(..) | Inst::ICmp(..) | Inst::IBinI(..) | Inst::ICmpI(..) | Inst::Conv(..) | Inst::Sel(..) | Inst::Call(Callee::Indirect(_), ..)) {
                    continue;
                }
                let old = f.blocks[bi].insts[ii].clone();
                let mut new = rename(&old, &a);
                let kv = |v: VReg, kn: &Vec<Option<K>>| kn[v.0 as usize];
                let folded: Option<(VReg, K)> = match &new {
                    Inst::Mov(d, s) => kv(*s, &kn).map(|k| (*d, k)),
                    Inst::IBin(op, it, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::I(p)), Some(K::I(q))) => fold_ibin(*op, *it, p, q).map(|r| (*d, K::I(r))),
                        (Some(K::A(p)), Some(K::I(q))) if *op == IOp::Add => Some((*d, K::A(p.plus(q)))),
                        (Some(K::I(p)), Some(K::A(q))) if *op == IOp::Add => Some((*d, K::A(q.plus(p)))),
                        _ => None,
                    },
                    Inst::IBinI(op, it, d, x, q) => match kv(*x, &kn) {
                        Some(K::I(p)) => fold_ibin(*op, *it, p, *q).map(|r| (*d, K::I(r))),
                        Some(K::A(p)) if *op == IOp::Add => Some((*d, K::A(p.plus(*q)))),
                        // x & 0, x * 0
                        _ if *q == 0 && matches!(op, IOp::And | IOp::Mul) => Some((*d, K::I(0))),
                        _ => None,
                    },
                    Inst::INeg(it, d, x) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(ext(p.wrapping_neg(), *it)))),
                        _ => None,
                    },
                    Inst::IBit(o, d, x) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(bit_op(*o, p as u64) as i64))),
                        _ => None,
                    },
                    Inst::INot(it, d, x) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(ext(!p, *it)))),
                        _ => None,
                    },
                    Inst::ICmp(c, s, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::I(p)), Some(K::I(q))) => Some((*d, K::I(cmp_i(*c, *s, p, q) as i64))),
                        _ => None,
                    },
                    Inst::ICmpI(c, s, d, x, q) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(cmp_i(*c, *s, p, *q) as i64))),
                        _ => None,
                    },
                    Inst::FBin(op, w, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::F(p, _)), Some(K::F(q, _))) => Some((*d, K::F(fold_fbin(*op, *w, p, q), *w))),
                        _ => None,
                    },
                    Inst::FUnary(op, w, d, x) => match kv(*x, &kn) {
                        Some(K::F(p, _)) => Some((*d, K::F(fold_fun(*op, *w, p), *w))),
                        _ => None,
                    },
                    Inst::Sel(d, c, x, y) => match kv(*c, &kn) {
                        Some(K::I(p)) => kv(if p != 0 { *x } else { *y }, &kn).map(|k| (*d, k)),
                        _ => None,
                    },
                    Inst::FCmp(c, w, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::F(p, _)), Some(K::F(q, _))) => Some((*d, K::I(cmp_f(*c, fval(p, *w), fval(q, *w)) as i64))),
                        _ => None,
                    },
                    Inst::Conv(c, d, x) => match kv(*x, &kn) {
                        Some(k) => fold_conv(*c, k).map(|r| (*d, r)),
                        None => None,
                    },
                    Inst::Load(m, d, b, off) => match kv(*b, &kn) {
                        Some(K::A(addr)) => {
                            let at = addr.plus(*off as i64);
                            let n = match m {
                                Mem::Int(n, _) => *n as u32,
                                Mem::F32 => 4,
                                Mem::F64 => 8,
                            };
                            match ro.map_or(RoRead::No, |rd| rd.read(at, n)) {
                                RoRead::Bytes(raw) => {
                                    let k = match m {
                                        Mem::Int(1, s) => K::I(if *s { raw as u8 as i8 as i64 } else { raw as u8 as i64 }),
                                        Mem::Int(2, s) => K::I(if *s { raw as u16 as i16 as i64 } else { raw as u16 as i64 }),
                                        Mem::Int(4, s) => K::I(if *s { raw as u32 as i32 as i64 } else { raw as u32 as i64 }),
                                        Mem::Int(_, _) => K::I(raw as i64),
                                        Mem::F32 => K::F(raw as u32 as u64, false),
                                        Mem::F64 => K::F(raw, true),
                                    };
                                    Some((*d, k))
                                }
                                RoRead::Ptr(p) if matches!(m, Mem::Int(8, _)) => Some((*d, K::A(p))),
                                _ => None,
                            }
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((d, k)) = folded {
                    new = const_inst(d, k);
                    if single(d, &ndef) && kn[d.0 as usize].is_none() {
                        kn[d.0 as usize] = Some(k);
                    }
                } else {
                    // address arithmetic into load/store offsets; immediates
                    new = match new {
                        Inst::Load(m, d, b, off) => match def_inst[b.0 as usize] {
                            Some((IOp::Add, x, k)) if (off as i64 + k).abs() < (1 << 30) => Inst::Load(m, d, x, off + k as i32),
                            _ => Inst::Load(m, d, b, off),
                        },
                        Inst::Store(m, b, off, s) => match def_inst[b.0 as usize] {
                            Some((IOp::Add, x, k)) if (off as i64 + k).abs() < (1 << 30) => Inst::Store(m, x, off + k as i32, s),
                            _ => Inst::Store(m, b, off, s),
                        },
                        // memcpy of a known small size: an inline copy (SROA and the backends'
                        // copy sequences see it; `ptr::copy_nonoverlapping(p, q, 1)` of a T)
                        Inst::Call(Callee::Host(a), ref args, ref rets)
                            if rets.is_empty() && args.len() == 3 && Some(a) == mem_fns().map(|m| m.0) && matches!(kn[args[2].0 as usize], Some(K::I(n)) if (0..=INLINE_COPY_MAX).contains(&n)) =>
                        {
                            match kn[args[2].0 as usize] {
                                Some(K::I(n)) => Inst::Copy(args[0], args[1], n as u32),
                                _ => unreachable!(),
                            }
                        }
                        Inst::Sel(d, c, x, y) if matches!(kn[c.0 as usize], Some(K::I(_))) => match kn[c.0 as usize] {
                            Some(K::I(p)) if p != 0 => Inst::Mov(d, x),
                            _ => Inst::Mov(d, y),
                        },
                        Inst::Sel(d, _, x, y) if x == y => Inst::Mov(d, x),
                        // c ? 1 : 0 of a 0/1 value (a compare): c itself, c ? 0 : 1 is c ^ 1
                        // (`count += is_less as usize` after an Ordering became a select)
                        Inst::Sel(d, c, x, y)
                            if zw[c.0 as usize] == 1 && arith_read[d.0 as usize] && f.vregs[d.0 as usize] == Cls::I && !opt_off(options, "-sel01") && matches!((kn[x.0 as usize], kn[y.0 as usize]), (Some(K::I(1)), Some(K::I(0))) | (Some(K::I(0)), Some(K::I(1)))) =>
                        {
                            if kn[x.0 as usize] == Some(K::I(1)) {
                                Inst::Mov(d, c)
                            } else {
                                Inst::IBinI(IOp::Xor, U64, d, c, 1)
                            }
                        }
                        Inst::IBin(op, it, d, x, y) if imm && op != IOp::Div && op != IOp::Rem && op != IOp::MulHi => {
                            let fits = |k: Option<K>| match k {
                                Some(K::I(v)) if v >= i32::MIN as i64 && v <= i32::MAX as i64 => Some(v),
                                _ => None,
                            };
                            let commutative = matches!(op, IOp::Add | IOp::Mul | IOp::And | IOp::Or | IOp::Xor);
                            if let Some(v) = fits(kn[y.0 as usize]) {
                                Inst::IBinI(op, it, d, x, v)
                            } else if commutative && fits(kn[x.0 as usize]).is_some() {
                                Inst::IBinI(op, it, d, y, fits(kn[x.0 as usize]).unwrap())
                            } else {
                                Inst::IBin(op, it, d, x, y)
                            }
                        }
                        Inst::ICmp(c, s, d, x, y) if imm => {
                            let fits = |k: Option<K>| match k {
                                Some(K::I(v)) if v >= i32::MIN as i64 && v <= i32::MAX as i64 => Some(v),
                                _ => None,
                            };
                            if let Some(v) = fits(kn[y.0 as usize]) {
                                Inst::ICmpI(c, s, d, x, v)
                            } else if let Some(v) = fits(kn[x.0 as usize]) {
                                Inst::ICmpI(swap_cond(c), s, d, y, v)
                            } else {
                                Inst::ICmp(c, s, d, x, y)
                            }
                        }
                        other => other,
                    };
                    // (x + k) - x -> k and x - (x + k) -> -k (wrapping 64-bit arithmetic, x one
                    // value; LLVM InstSimplify): `data[i..i + 8]` is 8 long, so `try_into::<[u8; 8]>`
                    // is always Ok -- the Err path, its tag store and reload and the default's fill
                    // stayed in every BitReader::peek
                    if ic && !opt_off(options, "-subk") {
                        if let Inst::IBin(IOp::Sub, it, d, a, b) = new {
                            let off = |s: VReg, x: VReg| match bin_def[s.0 as usize] {
                                Some((op @ (IOp::Add | IOp::Sub), t, y, k, _, _)) if y == x && t.bits == 64 && single(x, &ndef) => Some(if op == IOp::Add { k } else { k.wrapping_neg() }),
                                _ => None,
                            };
                            let k = if it.bits != 64 { None } else if let Some(k) = off(a, b) { Some(k) } else { off(b, a).map(|k| k.wrapping_neg()) };
                            if let Some(k) = k {
                                new = Inst::Iconst(d, k);
                                if single(d, &ndef) {
                                    kn[d.0 as usize] = Some(K::I(k));
                                }
                            }
                        }
                    }
                    // x * 1.0, 1.0 * x, x / 1.0, x - 0.0, x + -0.0, -0.0 + x: x itself (exact in
                    // IEEE arithmetic, a NaN's payload aside, which Rust leaves unspecified;
                    // LLVM's instsimplify does the same) -- `Vec4 { w: 1.0, .. }` through a matrix
                    if let Inst::FBin(op, w, d, x, y) = new {
                        let kf = |v: VReg| match kn[v.0 as usize] {
                            Some(K::F(b, _)) => Some(b),
                            _ => None,
                        };
                        let (one, nz) = if w { (1.0f64.to_bits(), (-0.0f64).to_bits()) } else { (1.0f32.to_bits() as u64, (-0.0f32).to_bits() as u64) };
                        let same = match op {
                            FOp::Mul | FOp::Div if kf(y) == Some(one) => Some(x),
                            FOp::Mul if kf(x) == Some(one) => Some(y),
                            FOp::Sub if kf(y) == Some(0) => Some(x),
                            FOp::Add if kf(y) == Some(nz) => Some(x),
                            FOp::Add if kf(x) == Some(nz) => Some(y),
                            _ => None,
                        };
                        if let Some(v) = same {
                            new = Inst::Mov(d, v);
                        }
                    }
                    // an unsigned compare of a narrow value with a constant beyond its range
                    if let Inst::ICmpI(c, false, d, x, k) = new {
                        let wx = zw[x.0 as usize];
                        if wx < 63 && k >= 0 {
                            let max = (1u64 << wx) - 1;
                            let r = match c {
                                Cond::Lt if max < k as u64 => Some(1),
                                Cond::Le if max <= k as u64 => Some(1),
                                Cond::Ge if max < k as u64 => Some(0),
                                Cond::Gt if max <= k as u64 => Some(0),
                                Cond::Eq if max < k as u64 => Some(0),
                                Cond::Ne if max < k as u64 => Some(1),
                                _ => None,
                            };
                            if let Some(v) = r {
                                new = Inst::Iconst(d, v);
                                if single(d, &ndef) {
                                    kn[d.0 as usize] = Some(K::I(v));
                                }
                            }
                        }
                    }
                    // a zero-extension or mask of a value already that narrow
                    if let Inst::Conv(Conv::IntToInt(t), d, x) = new {
                        if !t.signed && zw[x.0 as usize] <= t.bits {
                            new = Inst::Mov(d, x);
                        } else if ic_conv {
                            if let Some((t1, y, cb, ci)) = conv_def[x.0 as usize] {
                                if t == t1 {
                                    new = Inst::Mov(d, x);
                                } else if t.bits <= t1.bits && unchanged_between(f, &ndef, y, (cb, ci), (bi, ii)) {
                                    new = Inst::Conv(Conv::IntToInt(t), d, y);
                                }
                            }
                        }
                    }
                    if let Inst::IBinI(IOp::And, _, d, x, m) = new {
                        if m > 0 && (m as u64 + 1).is_power_of_two() && zw[x.0 as usize] as u32 <= 64 - (m as u64).leading_zeros() {
                            new = Inst::Mov(d, x);
                        }
                    }
                    // a call through a known function's address: a direct call (inlinable)
                    if let Inst::Call(Callee::Indirect(v), ref args, ref rets) = new {
                        if fn_def[v.0 as usize] != u32::MAX {
                            new = Inst::Call(Callee::Fn(fn_def[v.0 as usize]), args.clone(), rets.clone());
                        }
                    }
                    // x <=> x
                    if let Inst::ICmp(c, _, d, x, y) = new {
                        if x == y {
                            let v = matches!(c, Cond::Eq | Cond::Le | Cond::Ge) as i64;
                            new = Inst::Iconst(d, v);
                            if single(d, &ndef) {
                                kn[d.0 as usize] = Some(K::I(v));
                            }
                        }
                    }
                    // `(y ^ 1) ^ 1`: y
                    if let Inst::IBinI(IOp::Xor, _, d, x, 1) = new {
                        if let Some(y) = xor1_def[x.0 as usize] {
                            new = Inst::Mov(d, y);
                        }
                    }
                    // a compare result (0 / 1) masked with 1
                    if let Inst::IBinI(IOp::And, _, d, x, 1) = new {
                        if cmp_def[x.0 as usize].is_some() {
                            new = Inst::Mov(d, x);
                        }
                    }
                    // unsigned compares with 0 / 1 that are equality tests
                    if let Inst::ICmpI(c, false, d, x, k) = new {
                        new = match (c, k) {
                            (Cond::Le, 0) | (Cond::Lt, 1) => Inst::ICmpI(Cond::Eq, false, d, x, 0),
                            (Cond::Gt, 0) | (Cond::Ge, 1) => Inst::ICmpI(Cond::Ne, false, d, x, 0),
                            _ => new,
                        };
                    }
                    // a compare of a two-level select tree of constants and compare outcomes (an
                    // Ordering made branch-free: `c1 ? -1 : (c2 ? 0 : 1)` or `lt ? -1 : ne`, maybe
                    // narrowed to i8 by a conversion or a low mask), against a constant: the tree
                    // of the compare's outcomes, each a constant 0 / 1, an outcome or its negation
                    // (LLVM foldICmpSelect); the select folds below merge it into one compare
                    // (`ord == Greater` -> `!(a <= b)`), a tree of equal outcomes is a constant
                    // (`(lt ? -1 : ne) as u8 != 254`, Option<Ordering>'s niche test)
                    if let (Inst::ICmpI(cc, sg, d, x, k), false) = (&new, opt_off(options, "-selfold")) {
                        let (cc, sg, d, x, k) = (*cc, *sg, *d, *x, *k);
                        let low_mask = |m: i64| m > 0 && (m as u64 + 1).is_power_of_two();
                        // the narrowing steps between the select and the compare (innermost last):
                        // conversions below 64 bits and low masks (`ord as i8 as u8`)
                        let mut narrow: Vec<IntTy> = Vec::new();
                        let mut root = x;
                        while narrow.len() < 4 && single(root, &ndef) {
                            match (conv_def.get(root.0 as usize).copied().flatten(), bin_def.get(root.0 as usize).copied().flatten()) {
                                (Some((t, y, _, _)), _) if t.bits < 64 => {
                                    narrow.push(t);
                                    root = y;
                                }
                                (_, Some((IOp::And, _, y, m, _, _))) if low_mask(m) => {
                                    narrow.push(IntTy { bits: (64 - (m as u64).leading_zeros()) as u8, signed: false });
                                    root = y;
                                }
                                _ => break,
                            }
                        }
                        // a leaf's outcome: Ok(0 / 1) a constant, Err((v, negated)) the 0 / 1 value v
                        // (a compare result) or its negation
                        let cmp_k = |kv: i64| cmp_i(cc, sg, narrow.iter().rev().fold(kv, |v, t| crate::bitops::ext(v, *t)), k) as i64;
                        let leaf = |v: VReg| -> Option<Result<i64, (VReg, bool)>> {
                            match kn[v.0 as usize] {
                                Some(K::I(kv)) => Some(Ok(cmp_k(kv))),
                                _ if zw[v.0 as usize] == 1 && single(v, &ndef) => {
                                    // `y ^ 1` of an outcome y: y, negated
                                    let (v, neg) = match xor1_def[v.0 as usize] {
                                        Some(y) if zw[y.0 as usize] == 1 => (y, true),
                                        _ => (v, false),
                                    };
                                    match (cmp_k(0), cmp_k(1)) {
                                        (o0, o1) if o0 == o1 => Some(Ok(o0)),
                                        (0, _) => Some(Err((v, neg))),
                                        _ => Some(Err((v, !neg))),
                                    }
                                }
                                _ => None,
                            }
                        };
                        // the outcome tree, four levels at most (`partial_cmp` of floats is three);
                        // equal subtrees collapse
                        fn otree(v: VReg, depth: u32, sel_def: &[Option<(VReg, VReg, VReg)>], ndef: &[u32], leaf: &dyn Fn(VReg) -> Option<Result<i64, (VReg, bool)>>) -> Option<OTree> {
                            if let Some(l) = leaf(v) {
                                return Some(OTree::L(l));
                            }
                            if depth == 0 {
                                return None;
                            }
                            let (c, a, b) = sel_def[v.0 as usize].filter(|_| ndef[v.0 as usize] == 1)?;
                            let (ta, tb) = (otree(a, depth - 1, sel_def, ndef, leaf)?, otree(b, depth - 1, sel_def, ndef, leaf)?);
                            Some(if ta == tb { ta } else { OTree::N(c, Box::new(ta), Box::new(tb)) })
                        }
                        fn emit(f: &mut Func, insts: &mut Vec<Inst>, t: &OTree) -> VReg {
                            let d = f.vreg(Cls::I);
                            let i = match t {
                                OTree::L(Ok(kv)) => Inst::Iconst(d, *kv),
                                OTree::L(Err((v, false))) => Inst::Mov(d, *v),
                                OTree::L(Err((v, true))) => Inst::IBinI(IOp::Xor, U64, d, *v, 1),
                                OTree::N(c, a, b) => {
                                    let (ta, tb) = (emit(f, insts, a), emit(f, insts, b));
                                    Inst::Sel(d, *c, ta, tb)
                                }
                            };
                            insts.push(i);
                            d
                        }
                        if let Some((_, a1, b1)) = sel_def[root.0 as usize].filter(|_| single(root, &ndef)) {
                            let konst = |v: VReg| matches!(kn[v.0 as usize], Some(K::I(_)));
                            match otree(root, 4, &sel_def, &ndef, &leaf) {
                                // every outcome the same: a constant
                                Some(OTree::L(Ok(kv))) => {
                                    new = Inst::Iconst(d, kv);
                                    if single(d, &ndef) {
                                        kn[d.0 as usize] = Some(K::I(kv));
                                    }
                                }
                                // a select of two constants: the select-of-constants folds below (and
                                // the phi distribution) take it
                                Some(t) if !(konst(a1) && konst(b1)) => {
                                    let pos = f.blocks[bi].pos[ii];
                                    let mut insts = Vec::new();
                                    let top = emit(f, &mut insts, &t);
                                    edits.push(FoldEdit { block: bi, at: ii, remove: 0, insts, pos });
                                    new = Inst::Mov(d, top);
                                }
                                _ => {}
                            }
                        }
                    }
                    // a select on `c ? k1 : k2` (constants, one of them 0): a select on c, the
                    // arms swapped when k1 is the 0
                    if let (Inst::Sel(d, x, p, q), false) = (&new, opt_off(options, "-selfold")) {
                        let (d, x, p, q) = (*d, *x, *p, *q);
                        if let Some((c, a, b)) = sel_def[x.0 as usize] {
                            if let (Some(K::I(ka)), Some(K::I(kb)), Some(_)) = (kn[a.0 as usize], kn[b.0 as usize], cmp_def[c.0 as usize]) {
                                if ka == 0 && kb != 0 {
                                    new = Inst::Sel(d, c, q, p);
                                } else if kb == 0 && ka != 0 {
                                    new = Inst::Sel(d, c, p, q);
                                }
                            }
                        }
                    }
                    // `c1 ? v : (c2 ? v : w)` with c1, c2 compares of the same two operands:
                    // `(c1 | c2) ? v : w`, the union one compare (`a < b || a == b` is `a <= b`:
                    // an `Ordering` tested for one outcome, binary_search's step)
                    if let (Inst::Sel(d, c1, v1, x), false) = (&new, opt_off(options, "-selfold")) {
                        let (d, c1, v1, x) = (*d, *c1, *v1, *x);
                        if let Some((v2, w, k1, k2)) = sel_def[x.0 as usize].and_then(|(c2, v2, w)| Some((v2, w, cmp_def[c1.0 as usize]?, cmp_def[c2.0 as usize]?))) {
                            let same_v = v1 == v2 || matches!((kn[v1.0 as usize], kn[v2.0 as usize]), (Some(K::I(p)), Some(K::I(q))) if p == q);
                            if same_v && single(x, &ndef) {
                                if let Some(cu) = cond_union(k1, k2) {
                                    let cm = f.vreg(Cls::I);
                                    let (cc, sg, a, bk, imm) = k1;
                                    let _ = cc;
                                    let ci = if imm { Inst::ICmpI(cu, sg, cm, a, bk) } else { Inst::ICmp(cu, sg, cm, a, VReg(bk as u32)) };
                                    edits.push(FoldEdit { block: bi, at: ii, remove: 0, insts: vec![ci], pos: f.blocks[bi].pos[ii] });
                                    new = Inst::Sel(d, cm, v1, w);
                                }
                            }
                        }
                    }
                    // `(c ? k1 : k2) == k` with constants k1 != k2: c itself (or !c); c is a
                    // compare result (0 / 1)
                    if let Inst::ICmpI(cc @ (Cond::Eq | Cond::Ne), _, d, x, k) = new {
                        if let Some((c, a, b)) = sel_def[x.0 as usize] {
                            if let (Some(K::I(ka)), Some(K::I(kb)), Some(_)) = (kn[a.0 as usize], kn[b.0 as usize], cmp_def[c.0 as usize]) {
                                if ka != kb && (k == ka || k == kb) {
                                    let is_c = (k == ka) == (cc == Cond::Eq);
                                    new = if is_c { Inst::Mov(d, c) } else { Inst::IBinI(IOp::Xor, U64, d, c, 1) };
                                }
                            }
                        }
                    }
                    if ic_phi {
                        // A compare of an Ordering-like phi of constants, optionally
                        // narrowed first: distribute the compare over its assignments.
                        if let Inst::ICmpI(cc, signed, d, x, k) = new {
                            let (root, narrow, snapshot) = match conv_def[x.0 as usize] {
                                Some((t, y, cb, ci)) if t.bits < 64 => (y, Some(t), Some((cb, ci))),
                                _ => (x, None, None),
                            };
                            // only when the phi has no other use: otherwise its constants stay
                            // live next to the compare's (markdown +5%, splash_parse +2% at O1)
                            let dies = (root.0 as usize) < phi_nuse.len() && phi_nuse[root.0 as usize] == 1 && (narrow.is_none() || phi_nuse.get(x.0 as usize) == Some(&1));
                            if dies && ndef[root.0 as usize] > 1 && const_phi(f, &phi_defs, &ndef, root, 0) {
                                let (depth, header) = phi_loops.get_or_insert_with(|| {
                                    let depth = crate::regalloc::loop_depths(f, &reach);
                                    let header = loop_headers(f, &depth);
                                    (depth, header)
                                });
                                let at = snapshot.unwrap_or((bi, ii));
                                // Loop-carried shadows add assignments/register pressure in the
                                // hot loop (llm_host). Keep only per-iteration Ordering-like phis.
                                if (snapshot.is_none() || defs_in_same_iteration(f, depth, header, &[at], (bi, ii)))
                                    && phi_in_same_iteration(f, &phi_defs, &kn, root, at, depth, header)
                                {
                                    let mut b = phi_compare(f, &phi_defs, &kn, root, cc, signed, k, narrow, &mut Vec::new(), &mut edits);
                                    if let Some((cb, ci)) = snapshot {
                                        let saved = f.vreg(Cls::I);
                                        fold_insert(f, &mut edits, cb, ci, Inst::Mov(saved, b));
                                        b = saved;
                                    }
                                    new = Inst::Mov(d, b);
                                }
                            }
                        }
                        // ((x - c1) & (2^n - 1)) == 0 -> (x & (2^n - 1)) == (c1 & mask).
                        if let Inst::ICmpI(cc @ (Cond::Eq | Cond::Ne), signed, d, a, 0) = new {
                            if let Some((IOp::And, mt, s, m, _, _)) = bin_def[a.0 as usize] {
                                if m > 0 && (m as u64 + 1).is_power_of_two() {
                                    if let Some((op @ (IOp::Add | IOp::Sub), st, x, c, sb, si)) = bin_def[s.0 as usize] {
                                        let bits = (64 - (m as u64).leading_zeros()) as u8;
                                        if bits <= mt.bits && bits <= st.bits {
                                            let a2 = f.vreg(Cls::I);
                                            // Capture x where the subtraction read it; x itself may be mutable.
                                            fold_insert(f, &mut edits, sb, si, Inst::IBinI(IOp::And, U64, a2, x, m));
                                            let c1 = if op == IOp::Add { c.wrapping_neg() } else { c };
                                            new = Inst::ICmpI(cc, signed, d, a2, c1 & m);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Inst::IBinI(IOp::Xor, _, d, x, 1) = new {
                        // !cmp -> the inverted integer compare
                        if single(x, &ndef) {
                            if let Some((c, sg, a, kk, is_imm)) = cmp_def[x.0 as usize] {
                                let inv = match c {
                                    Cond::Eq => Cond::Ne,
                                    Cond::Ne => Cond::Eq,
                                    Cond::Lt => Cond::Ge,
                                    Cond::Ge => Cond::Lt,
                                    Cond::Le => Cond::Gt,
                                    Cond::Gt => Cond::Le,
                                };
                                new = if is_imm { Inst::ICmpI(inv, sg, d, a, kk) } else { Inst::ICmp(inv, sg, d, a, VReg(kk as u32)) };
                            }
                        }
                    }
                    if let Inst::IBinI(op, it, d, x, k) = new {
                        let ident = match op {
                            IOp::Add | IOp::Sub | IOp::Or | IOp::Xor | IOp::Shl | IOp::Shr => k == 0,
                            IOp::Mul | IOp::Div => k == 1,
                            _ => false,
                        };
                        // narrow types: the operand is already extended by its type
                        if k == 0 && matches!(op, IOp::And | IOp::Mul) {
                            new = Inst::Iconst(d, 0);
                            if single(d, &ndef) {
                                kn[d.0 as usize] = Some(K::I(0));
                            }
                        } else if ident {
                            new = Inst::Mov(d, x);
                        } else if op == IOp::And && k == -1 {
                            new = Inst::Mov(d, x);
                        } else if op == IOp::Mul && k > 1 && (k & (k - 1)) == 0 && k < (1 << 30) {
                            new = Inst::IBinI(IOp::Shl, it, d, x, k.trailing_zeros() as i64);
                        }
                    }
                    if let Inst::IBinI(IOp::Add, _, d, x, k) = new {
                        if single(d, &ndef) && single(x, &ndef) {
                            def_inst[d.0 as usize] = Some((IOp::Add, x, k));
                        }
                    }
                }
                // memmove of a known small size: loads, then stores (overlap-safe; not a
                // `Copy`, which is a memcpy to the passes and the backends)
                if let Inst::Call(Callee::Host(m), args, rets) = &new {
                    if rets.is_empty() && args.len() == 3 && Some(*m) == mem_fns().map(|m| m.1) {
                        if let Some(K::I(n)) = kn[args[2].0 as usize] {
                            if (0..=INLINE_COPY_MAX).contains(&n) {
                                moves.push((bi, ii, n as u32));
                            }
                        }
                    }
                }
                if !same_inst(&old, &new) {
                    changed = true;
                    f.blocks[bi].insts[ii] = new;
                }
            }
            // terminator (renamed in place)
            match &mut f.blocks[bi].term {
                Term::Branch(c, x, y) => {
                    let c2 = a(*c);
                    let (x, y) = (*x, *y);
                    f.blocks[bi].term = match kn[c2.0 as usize] {
                        Some(K::I(v)) => Term::Jump(if v != 0 { x } else { y }),
                        _ => Term::Branch(c2, x, y),
                    };
                }
                Term::Ret(v) => {
                    for x in v.iter_mut() {
                        *x = a(*x);
                    }
                }
                _ => {}
            }
        }
        changed |= !moves.is_empty();
        for &(bi, ii, n) in moves.iter().rev() {
            let Inst::Call(_, ref args, _) = f.blocks[bi].insts[ii] else { unreachable!() };
            let (d, s2) = (args[0], args[1]);
            // 8-byte words, the tail as one overlapping word; under 8 bytes, 4/2/1 pieces
            let mut parts: Vec<(u8, i32)> = Vec::new();
            if n >= 8 {
                parts.extend((0..n / 8).map(|j| (8u8, j as i32 * 8)));
                if n % 8 != 0 {
                    parts.push((8, n as i32 - 8));
                }
            } else {
                let mut o = 0;
                for w in [4u32, 2, 1] {
                    if (n - o) >= w {
                        parts.push((w as u8, o as i32));
                        o += w;
                    }
                }
            }
            let ts: Vec<VReg> = parts.iter().map(|_| f.vreg(Cls::I)).collect();
            let mut seq: Vec<Inst> = parts.iter().zip(&ts).map(|(&(w, o), &t)| Inst::Load(Mem::Int(w, false), t, s2, o)).collect();
            seq.extend(parts.iter().zip(&ts).map(|(&(w, o), &t)| Inst::Store(Mem::Int(w, false), d, o, t)));
            let p = f.blocks[bi].pos[ii];
            edits.push(FoldEdit { block: bi, at: ii, remove: 1, insts: seq, pos: p });
        }
        edits.sort_by_key(|e| (e.block, e.at));
        for e in edits.into_iter().rev() {
            let b = &mut f.blocks[e.block];
            let n = e.insts.len();
            b.insts.splice(e.at..e.at + e.remove, e.insts);
            b.pos.splice(e.at..e.at + e.remove, std::iter::repeat(e.pos).take(n));
        }
        // `x % c == 0` (unsigned, c constant): one multiply by c's inverse and a compare
        if !crate::stats::opt_off(options, "-remzero") {
            changed |= rem_eq_zero(f, &kn, nv);
        }
        // unsigned division / remainder by a constant: multiply-high and shifts
        changed |= div_by_const(f, &kn, nv);
        if !crate::stats::opt_off(options, "-divrem") {
            changed |= div_rem_pairs(f);
        }
        pass_time(0, tf);
        let t = pass_clock();
        trace_opt(f, "fold");
        changed |= local_copy_prop(f);
        changed |= forward_narrow_stores(f);
        trace_opt(f, "lcopy");
        pass_time(2, t);
        let t = pass_clock();
        // CSE runs in the first round of each call: later rounds fold what it exposed; a
        // second CSE pass found next to nothing (counter: same code size) for 15% of opt
        if cse && _round == 0 {
            changed |= local_cse(options, f);
            trace_opt(f, "cse");
        }
        if ic_sink {
            changed |= sink_to_successor(f);
        }
        pass_time(1, t);
        let t = pass_clock();
        changed |= dce(f);
        pass_time(3, t);
        let t = pass_clock();
        thread_jumps(f);
        trace_opt(f, "dce+jumps");
        changed |= ic_nn && fold_nonnull(f);
        if !opt_off(options, "-unfoldsel") {
            changed |= unfold_branch_selects(f);
        }
        let tt = pass_clock();
        let threaded = thread_on(options) && thread_known(f);
        pass_time(15, tt);
        trace_opt(f, "thread");
        changed |= threaded;
        if if_conv_on(options) {
            changed |= if_convert(f, !opt_off(options, "-ifspec"));
        }
        changed |= merge_blocks(f);
        trace_opt(f, "merge");
        pass_time(4, t);
        if threaded {
            let t = pass_clock();
            changed |= live_dce(f);
            pass_time(3, t);
        }
        if !changed {
            break;
        }
    }
    // byte loads assembled into a wider value (after the rounds: it adds vregs; the next simplify folds around it)
    if f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Load(Mem::Int(1, false), ..) | Inst::LoadX(Mem::Int(1, false), ..)))) {
        let mut nd = vec![0u32; f.vregs.len()];
        for p in &f.params {
            nd[p.0 as usize] += 1;
        }
        for b in &f.blocks {
            for i in &b.insts {
                for_defs(i, |d| nd[d.0 as usize] += 1);
            }
        }
        if combine_loads(f, &nd) {
            dce(f);
        }
    }
    // x << k | x >> (w - k): one rotate (rol / ror)
    if f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::IBin(IOp::Or, ..)))) && combine_rotates(f) {
        dce(f);
    }
    // x / 2^k: x * 2^-k (adds vregs, so after the rounds)
    if recip_pow2_div(f) {
        dce(f);
    }
    // branch on `c == 0` / `c != 0` / `c ^ 1` of a compare c: branch on c (swapping the targets)
    if branch_on_compare(f) {
        dce(f);
    }
    // a float load read only as its bits: an integer load (and the reverse)
    if !opt_off(options, "-loadbits") && f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Conv(Conv::FToBits(_) | Conv::BitsToF(_) | Conv::IntToInt(_), ..)))) {
        load_bits(f);
    }
    fuse_sincos(f);
}

/// Per width (f32, f64): libm's `sin` and `cos` symbol ids and the combined `sincos` routine. The
/// embedder registers them only for targets whose rustc build combines the pair: LLVM's
/// SelectionDAG turns an FSIN and an FCOS of the same value into one call of Darwin's
/// `__sincosf_stret` / `__sincos_stret` (a two-float HFA on arm64), and its results are not always
/// those of separate `sinf`/`cosf` calls (1 ulp in 127 of 4000 arguments), so a program's floats
/// only match a rustc build's if we combine them too.
pub static SINCOS: relax_session::sync::Mutex<[Option<(u32, u32, Sym)>; 2]> = relax_session::sync::Mutex::new([None, None]);

/// `sin(x)` and `cos(x)` of the same value in one block become one combined call, as LLVM's
/// legalizer does (LegalizeDAG: `useSinCos` / `ExpandSinCosLibCall`: an FSIN and an FCOS with the
/// same operand in one DAG, i.e. one basic block). Runs at the end of `simplify`, when inlining has
/// exposed the libm calls and the block splits of bounds checks and invokes are gone, as they are
/// by the time LLVM selects instructions. The call goes where the earlier of the two was; the
/// later's result is defined there too, so it must be neither read nor written in between (RIR
/// vregs are mutable).
pub fn fuse_sincos(f: &mut Func) -> bool {
    let tab = *SINCOS.lock().unwrap_or_else(|e| e.into_inner());
    if tab.iter().all(|t| t.is_none()) {
        return false;
    }
    let kind = |i: &Inst| -> Option<(usize, bool, VReg, VReg)> {
        let Inst::Call(Callee::CHost(s, _), args, rets) = i else { return None };
        if args.len() != 1 || rets.len() != 1 {
            return None;
        }
        (0..2).find_map(|w| {
            let (sn, cs, _) = tab[w]?;
            if s.id == sn { Some((w, true, args[0], rets[0])) } else if s.id == cs { Some((w, false, args[0], rets[0])) } else { None }
        })
    };
    // the value an operand holds at instruction `at`: followed back through `Mov` copies within
    // the block to (vreg, index of its defining instruction or none for a value live into the block)
    let ident = |insts: &[Inst], at: usize, v: VReg| -> (VReg, Option<usize>) {
        let (mut v, mut at) = (v, at);
        loop {
            let def = (0..at).rev().find(|&k| {
                let mut hit = false;
                for_defs(&insts[k], |d| hit |= d == v);
                hit
            });
            match def {
                Some(k) => match insts[k] {
                    Inst::Mov(_, y) => {
                        v = y;
                        at = k;
                    }
                    _ => return (v, Some(k)),
                },
                None => return (v, None),
            }
        }
    };
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let mut n = 0;
        while n < f.blocks[bi].insts.len() {
            let found = (|| {
                let insts = &f.blocks[bi].insts;
                let (w, is_sin, x, r1) = kind(&insts[n])?;
                let both = tab[w]?.2;
                let id1 = ident(insts, n, x);
                for m in n + 1..insts.len() {
                    if let Some((w2, s2, x2, r2)) = kind(&insts[m]) {
                        if w2 == w && s2 != is_sin && ident(insts, m, x2) == id1 {
                            let touched = |v: VReg| {
                                insts[n + 1..m].iter().any(|i| {
                                    let mut t = false;
                                    for_uses(i, |u| t |= u == v);
                                    for_defs(i, |d| t |= d == v);
                                    t
                                })
                            };
                            let (rs, rc) = if is_sin { (r1, r2) } else { (r2, r1) };
                            if rs == rc {
                                return None;
                            }
                            // at the earlier call (reading its operand) or at the later one
                            if !touched(r2) {
                                return Some((m, n, rs, rc, both, x));
                            }
                            if !touched(r1) {
                                return Some((m, m, rs, rc, both, x2));
                            }
                            return None;
                        }
                    }
                }
                None
            })();
            if let Some((m, at, rs, rc, both, x)) = found {
                let b = &mut f.blocks[bi];
                b.insts[at] = Inst::Call(Callee::Host(both), vec![x], vec![rs, rc]);
                let drop = if at == n { m } else { n };
                b.insts.remove(drop);
                b.pos.remove(drop);
                changed = true;
            }
            n += 1;
        }
    }
    changed
}

pub fn branch_on_compare(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut cmp_only = vec![true; nv];
    let mut uses = vec![0u32; nv];
    for p in &f.params {
        cmp_only[p.0 as usize] = false;
    }
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| {
                if !matches!(i, Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..)) {
                    cmp_only[d.0 as usize] = false;
                }
            });
            for_uses(i, |u| uses[u.0 as usize] += 1);
        }
        tu.clear();
        term_uses(&b.term, &mut tu);
        for u in &tu {
            uses[u.0 as usize] += 1;
        }
    }
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let Term::Branch(d, t, e) = f.blocks[bi].term else { continue };
        if t == e || uses[d.0 as usize] != 1 {
            continue;
        }
        let Some(last) = f.blocks[bi].insts.last() else { continue };
        let (c, invert) = match *last {
            Inst::ICmpI(Cond::Eq, _, dd, c, 0) if dd == d => (c, true),
            Inst::ICmpI(Cond::Ne, _, dd, c, 0) if dd == d => (c, false),
            Inst::IBinI(IOp::Xor, _, dd, c, 1) if dd == d => (c, true),
            _ => continue,
        };
        // c is not redefined by the removed instruction itself, and holds a 0 / 1 compare
        if c == d || !cmp_only[c.0 as usize] || uses[c.0 as usize] != 1 {
            continue;
        }
        let b = &mut f.blocks[bi];
        b.insts.pop();
        b.pos.pop();
        b.term = if invert { Term::Branch(c, e, t) } else { Term::Branch(c, t, e) };
        changed = true;
    }
    changed
}

/// The reciprocal of a power-of-two float constant (`bits` of an f32 / f64) when that is itself
/// a finite non-zero power of two, normal or subnormal: then `x / c` and `x * (1 / c)` are the
/// correctly rounded value of the same real number for every x (NaN, inf, zero and signs
/// included; an overflow or underflow of `x * r` rounds identically in both). Zero, inf, NaN and
/// non-powers of two have none; 1 / c rounds when it is not representable, which the power-of-two
/// check on the result rejects (an overflow gives inf, an underflow 0).
pub fn exact_recip(w: bool, bits: u64) -> Option<u64> {
    if w {
        let pow2 = |b: u64| {
            let m = b & ((1u64 << 52) - 1);
            let e = (b >> 52) & 0x7ff;
            e != 0x7ff && if e == 0 { m.count_ones() == 1 } else { m == 0 }
        };
        if !pow2(bits) {
            return None;
        }
        let r = (1.0 / f64::from_bits(bits)).to_bits();
        pow2(r).then_some(r)
    } else {
        let pow2 = |b: u32| {
            let m = b & ((1u32 << 23) - 1);
            let e = (b >> 23) & 0xff;
            e != 0xff && if e == 0 { m.count_ones() == 1 } else { m == 0 }
        };
        let bits = bits as u32;
        if !pow2(bits) {
            return None;
        }
        let r = (1.0 / f32::from_bits(bits)).to_bits();
        pow2(r).then_some(r as u64)
    }
}

/// `x / c` for a single-definition float constant c = +-2^k with an exact reciprocal becomes
/// `x * c^-1` (LLVM does the same for exact reciprocals; a multiply is far cheaper than a divide).
pub fn recip_pow2_div(f: &mut Func) -> bool {
    if !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::FBin(FOp::Div, ..)))) {
        return false;
    }
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut kf: Vec<Option<u64>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Fconst(d, bits, _) = i {
                kf[d.0 as usize] = Some(*bits);
            }
        }
    }
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let mut out: Vec<Inst> = Vec::new();
        let mut pos: Vec<_> = Vec::new();
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let old_pos = std::mem::take(&mut f.blocks[bi].pos);
        for (i, p) in old.into_iter().zip(old_pos) {
            if let Inst::FBin(FOp::Div, w, d, x, y) = i {
                if ndef[y.0 as usize] == 1 {
                    if let Some(r) = kf[y.0 as usize].and_then(|b| exact_recip(w, b)) {
                        let c = f.vreg(if w { Cls::F64 } else { Cls::F32 });
                        out.push(Inst::Fconst(c, r, w));
                        out.push(Inst::FBin(FOp::Mul, w, d, x, c));
                        pos.push(p);
                        pos.push(p);
                        changed = true;
                        continue;
                    }
                }
            }
            out.push(i);
            pos.push(p);
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
    changed
}

/// Rotates: `d = (x << k) | (x >> (w - k))` (either operand order) with both shifts by
/// constants of the same unsigned 32-bit or 64-bit type, x, the shifts and d single-def, becomes
/// `d = x rotl k` (the shifts stay for DCE when nothing else reads them). rustc's LLVM matches the
/// same funnel shift; `rotate_left` lowers to the shift pair.
pub fn combine_rotates(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut def_at: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
                def_at[d.0 as usize] = (bi as u32, k as u32);
            });
        }
    }
    let shift = |f: &Func, v: VReg| -> Option<(IOp, IntTy, VReg, i64)> {
        if ndef[v.0 as usize] != 1 {
            return None;
        }
        let (bi, k) = def_at[v.0 as usize];
        if bi == u32::MAX {
            return None;
        }
        match f.blocks[bi as usize].insts[k as usize] {
            Inst::IBinI(op @ (IOp::Shl | IOp::Shr), it, _, x, c) if ndef[x.0 as usize] == 1 => Some((op, it, x, c)),
            _ => None,
        }
    };
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        for k in 0..f.blocks[bi].insts.len() {
            let (it, d, p, q) = match f.blocks[bi].insts[k] {
                Inst::IBin(IOp::Or, it, d, p, q) => (it, d, p, q),
                _ => continue,
            };
            let w = it.bits as i64;
            if !(w == 64 || (w == 32 && !it.signed)) {
                continue;
            }
            let (Some(a), Some(b)) = (shift(f, p), shift(f, q)) else { continue };
            let (l, r) = if a.0 == IOp::Shl { (a, b) } else { (b, a) };
            if l.0 != IOp::Shl || r.0 != IOp::Shr || l.2 != r.2 || r.1.signed || l.1.bits as i64 != w || r.1.bits as i64 != w {
                continue;
            }
            if !(1..w).contains(&l.3) || l.3 + r.3 != w {
                continue;
            }
            f.blocks[bi].insts[k] = Inst::IBinI(IOp::Rotl, IntTy { bits: w as u8, signed: false }, d, l.2, l.3);
            changed = true;
        }
    }
    changed
}

/// Float loads read only as their bits (`f32::to_bits` of a loaded float: `total_cmp`, float
/// hashing and radix keys) load the integer directly: `Load(F32, x); FToBits(d, x)` ->
/// `Load(Int(4), t); Mov(d, t)` (F64: Int(8)), and integer loads read only through `BitsToF`
/// load the float. One definition and one use of x; the load stays where it was, so no store
/// can come between. Returns whether anything changed.
pub fn load_bits(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        term_uses(&b.term, &mut tu);
        for u in &tu {
            nuse[u.0 as usize] += 1;
        }
    }
    // x -> the memory kind its one reader wants
    let mut want: Vec<Option<Mem>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Conv(c, _, x) = *i {
                let m = match c {
                    Conv::FToBits(w) => Mem::Int(if w { 8 } else { 4 }, false),
                    Conv::BitsToF(w) => if w { Mem::F64 } else { Mem::F32 },
                    // a narrow integer load re-extended at its own width: the load extends
                    // (`to_bits() as i32` of a loaded f32: movsxd from memory)
                    Conv::IntToInt(t) if matches!(t.bits, 8 | 16 | 32) => Mem::Int(t.bits / 8, t.signed),
                    _ => continue,
                };
                if ndef[x.0 as usize] == 1 && nuse[x.0 as usize] == 1 {
                    want[x.0 as usize] = Some(m);
                }
            }
        }
    }
    // the loads that qualify: x -> its replacement t (of the reader's class)
    let mut repl: Vec<Option<VReg>> = vec![None; nv];
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        for ii in 0..f.blocks[bi].insts.len() {
            let Inst::Load(m, x, base, off) = f.blocks[bi].insts[ii] else { continue };
            let Some(w) = want[x.0 as usize] else { continue };
            let ok = matches!((m, w), (Mem::F32, Mem::Int(4, _)) | (Mem::F64, Mem::Int(8, _)) | (Mem::Int(4, _), Mem::F32) | (Mem::Int(8, _), Mem::F64))
                || matches!((m, w), (Mem::Int(a, _), Mem::Int(b, _)) if a == b && a < 8);
            if !ok {
                continue;
            }
            let cls = if matches!(w, Mem::Int(..)) { Cls::I } else if w == Mem::F32 { Cls::F32 } else { Cls::F64 };
            let t = f.vreg(cls);
            repl.resize(f.vregs.len(), None);
            repl[x.0 as usize] = Some(t);
            f.blocks[bi].insts[ii] = Inst::Load(w, t, base, off);
            changed = true;
        }
    }
    if !changed {
        return false;
    }
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            if let Inst::Conv(Conv::FToBits(_) | Conv::BitsToF(_) | Conv::IntToInt(_), d, x) = *i {
                if let Some(Some(t)) = repl.get(x.0 as usize) {
                    *i = Inst::Mov(d, *t);
                }
            }
        }
    }
    true
}

/// Removes pure instructions whose results are dead by liveness: the definitions `dce`'s use
/// counts cannot see because the vreg has several (SROA cells' entry zeros overwritten before
/// any read, instructions `thread_known` copied whose values are not needed on that path).
pub fn live_dce(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    if nb == 0 {
        return false;
    }
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    // only multi-def vregs take part (a single-def dead vreg is dce's, by use counts):
    // liveness runs over their dense indices
    let mut ndef = vec![0u8; nv];
    // vregs reaching a second definition (the candidates), numbered below in vreg order
    let mut multi: Vec<u32> = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| {
                let c = &mut ndef[d.0 as usize];
                if *c == 1 {
                    multi.push(d.0);
                }
                *c = c.saturating_add(1);
            });
        }
    }
    for p in &f.params {
        ndef[p.0 as usize] = 0;
    }
    multi.retain(|v| ndef[*v as usize] > 1);
    if multi.is_empty() {
        return false;
    }
    multi.sort_unstable();
    let mut idx = vec![u32::MAX; nv];
    for (k, v) in multi.iter().enumerate() {
        idx[*v as usize] = k as u32;
    }
    let m = multi.len() as u32;
    let words = (m as usize + 63) / 64;
    let reach = reachable(f);
    let mut gen = vec![0u64; nb * words];
    let mut kill = vec![0u64; nb * words];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let g = &mut gen[bi * words..(bi + 1) * words];
        let k = &mut kill[bi * words..(bi + 1) * words];
        for i in &b.insts {
            for_uses(i, |u| {
                let x = idx[u.0 as usize];
                if x != u32::MAX && k[x as usize / 64] & (1u64 << (x % 64)) == 0 {
                    g[x as usize / 64] |= 1u64 << (x % 64);
                }
            });
            for_defs(i, |d| {
                let x = idx[d.0 as usize];
                if x != u32::MAX {
                    k[x as usize / 64] |= 1u64 << (x % 64);
                }
            });
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            let x = idx[u.0 as usize];
            if x != u32::MAX && k[x as usize / 64] & (1u64 << (x % 64)) == 0 {
                g[x as usize / 64] |= 1u64 << (x % 64);
            }
        }
    }
    let (live_in, _) = crate::regalloc::solve_liveness(f, &reach, words, &gen, &kill, false);
    let mut any = false;
    let mut live = vec![0u64; words];
    for bi in 0..nb {
        if !reach[bi] {
            continue;
        }
        live.iter_mut().for_each(|w| *w = 0);
        for t in f.blocks[bi].term.succs() {
            for w in 0..words {
                live[w] |= live_in[t as usize * words + w];
            }
        }
        let b = &mut f.blocks[bi];
        term_uses(&b.term, &mut uses);
        for u in &uses {
            let x = idx[u.0 as usize];
            if x != u32::MAX {
                live[x as usize / 64] |= 1u64 << (x % 64);
            }
        }
        let mut keep = vec![true; b.insts.len()];
        let mut dropped = false;
        for k in (0..b.insts.len()).rev() {
            let i = &b.insts[k];
            uses_defs(i, &mut uses, &mut defs);
            let dead = !defs.is_empty()
                && defs.iter().all(|d| {
                    let x = idx[d.0 as usize];
                    x != u32::MAX && live[x as usize / 64] & (1u64 << (x % 64)) == 0
                })
                && is_pure(i)
                && !matches!(i, Inst::Call(..));
            if dead {
                keep[k] = false;
                dropped = true;
                continue;
            }
            for d in &defs {
                let x = idx[d.0 as usize];
                if x != u32::MAX {
                    live[x as usize / 64] &= !(1u64 << (x % 64));
                }
            }
            for u in &uses {
                let x = idx[u.0 as usize];
                if x != u32::MAX {
                    live[x as usize / 64] |= 1u64 << (x % 64);
                }
            }
        }
        if dropped {
            any = true;
            let mut k = 0;
            b.insts.retain(|_| {
                k += 1;
                keep[k - 1]
            });
            let mut k = 0;
            b.pos.retain(|_| {
                k += 1;
                keep[k - 1]
            });
        }
    }
    any
}

pub fn no_noalias(options: &relax_session::Options) -> bool {
    options.no_noalias
}

/// Largest constant-size memcpy turned into an inline `Copy`.
pub const INLINE_COPY_MAX: i64 = 64;

pub static MEM_FNS: std::sync::OnceLock<(Sym, Sym)> = std::sync::OnceLock::new();

pub fn mem_fns() -> Option<(Sym, Sym)> {
    MEM_FNS.get().copied()
}

/// Most instructions a branch block may have to be copied into a predecessor by `thread_known`.
pub const THREAD_MAX_INSTS: usize = 8;

/// Jump threading over values known on an edge: a predecessor P that ends in `Jump(B)` and
/// knows (from its own constants, or its single-predecessor chain's) the values B's branch
/// condition is computed from, jumps straight to the branch's target. This is the
/// construct-then-match of small enums after inlining (`Some(x)` built, then `is_some` /
/// `?` / `match` on it): the tag is a constant on each incoming edge. The path's
/// instructions are copied into P (RIR is not SSA: a vreg may have several definitions);
/// `live_dce` then removes the copies this path does not need.
#[inline(never)]
/// `p == 0` / `p != 0` of a pointer that cannot be null become constants: the Option<&T> /
/// Option<NonNull<T>> niche tests a slice iterator's next() leaves per element (rustc knows
/// the pointers are nonnull). Non-null: stack slot, symbol and function addresses, loads
/// lowering marked (Func::nonnull: a field whose layout niche excludes 0), dereferenceable
/// params, in-bounds pointer arithmetic from a non-null base, and
/// a vreg all of whose definitions are non-null (a loop's pointer cell: start, then +4).
pub fn fold_nonnull(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    // optimistic: every vreg some definition could make non-null starts as such; a definition
    // that is not removes it, to a fixpoint
    let mut nn = vec![true; nv];
    let mut has_def = vec![false; nv];
    let mut marked = vec![false; nv];
    for v in &f.nonnull {
        marked[v.0 as usize] = true;
    }
    for p in &f.params {
        has_def[p.0 as usize] = true;
        if !marked[p.0 as usize] && !f.deref.iter().any(|(d, _)| d == p) {
            nn[p.0 as usize] = false;
        }
    }
    let mut any_cmp = false;
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| has_def[d.0 as usize] = true);
            if let Inst::ICmpI(Cond::Eq | Cond::Ne, _, _, _, 0) = i {
                any_cmp = true;
            }
        }
    }
    if !any_cmp {
        return false;
    }
    for v in 0..nv {
        nn[v] &= has_def[v];
    }
    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                let ok = |nn: &[bool]| -> bool {
                    match i {
                        Inst::SlotAddr(..) | Inst::Addr(..) | Inst::FnAddr(..) => true,
                        Inst::Load(Mem::Int(8, _), d, ..) | Inst::LoadX(Mem::Int(8, _), d, ..) => marked[d.0 as usize],
                        Inst::Mov(_, x) => nn[x.0 as usize],
                        // Integer constants (even nonzero ones) never seed this rule:
                        // the base must trace back to one of the pointer leaves above.
                        Inst::IBinI(IOp::Add, it, _, x, _) | Inst::IBin(IOp::Add, it, _, x, _) => it.bits == 64 && nn[x.0 as usize],
                        Inst::Sel(_, _, x, y) => nn[x.0 as usize] && nn[y.0 as usize],
                        _ => false,
                    }
                };
                let good = ok(&nn);
                // `good` is computed before visiting any definition, including multi-def
                // instructions; updating here preserves the old simultaneous decision.
                for_defs(i, |d| {
                    if !good && nn[d.0 as usize] {
                        nn[d.0 as usize] = false;
                        changed = true;
                    }
                });
            }
        }
    }
    let mut folded = false;
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            if let Inst::ICmpI(c @ (Cond::Eq | Cond::Ne), _, d, x, 0) = *i {
                if has_def[x.0 as usize] && nn[x.0 as usize] {
                    *i = Inst::Iconst(d, (c == Cond::Ne) as i64);
                    folded = true;
                }
            }
        }
    }
    folded
}

/// Move a result used in just one successor onto that edge. Decisions use the original
/// use blocks, and arrivals are installed only at the end: one sweep, without cascading.
pub fn sink_to_successor(f: &mut Func) -> bool {
    const NONE: usize = usize::MAX;
    const MANY: usize = usize::MAX - 1;
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    let depth = crate::regalloc::loop_depths(f, &reachable(f));
    let header = loop_headers(f, &depth);
    let mut ndef = vec![0u32; nv];
    let mut use_block = vec![NONE; nv];
    let mut npred = vec![0u32; nb];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut uses = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut note_use = |v: VReg| {
            let at = &mut use_block[v.0 as usize];
            *at = if *at == NONE || *at == bi { bi } else { MANY };
        };
        for i in &b.insts {
            for_uses(i, &mut note_use);
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
        term_uses(&b.term, &mut uses);
        for &u in &uses {
            note_use(u);
        }
        for s in b.term.succs() {
            npred[s as usize] += 1;
        }
    }
    let mut arrivals: Vec<Vec<(Inst, u64)>> = vec![Vec::new(); nb];
    let mut written = vec![NONE; nv];
    let mut changed = false;
    let mut defs = Vec::new();
    for bi in 0..nb {
        // loops.rs runs later: counted loops need their header's body intact, and
        // byte-map widening needs loads/ALU ops to remain on its linear loop chain.
        if header[bi] { continue; }
        // Do not move work into an unwind pad or across an invoke's final call.
        if !matches!(f.blocks[bi].term, Term::Branch(..) | Term::Jump(..)) {
            continue;
        }
        let b = &f.blocks[bi];
        let mut remove = vec![false; b.insts.len()];
        let mut cand: Vec<(usize, usize)> = Vec::new();
        let mut memory_effect = false;
        let mut fp_env = false;
        for (ii, i) in b.insts.iter().enumerate().rev() {
            uses_defs(i, &mut uses, &mut defs);
            let load = matches!(i, Inst::Load(..));
            let movable = is_pure(i) && !matches!(i, Inst::LoadX(..) | Inst::VLoad(..) | Inst::VLoadX(..) | Inst::VGather(..) | Inst::VTbl(..) | Inst::TlsAddr(..) | Inst::TlsStatic(..));
            if movable && !fp_env && (!load || !memory_effect) && defs.len() == 1 && ndef[defs[0].0 as usize] == 1 {
                let s = use_block[defs[0].0 as usize];
                if s < nb && s != bi && s != 0 && npred[s] == 1 && b.term.succs().any(|t| t as usize == s)
                    && depth[bi] == depth[s]
                    && uses.iter().all(|v| written[v.0 as usize] != bi)
                {
                    cand.push((ii, s));
                    remove[ii] = true;
                }
            }
            for &d in &defs {
                written[d.0 as usize] = bi;
            }
            memory_effect |= is_mem_effect(i);
            fp_env |= matches!(i, Inst::FpEnv(..));
        }
        // A block left with only its branch compare is what jump threading duplicates into a
        // predecessor: an iterator `next` whose element copy left its guard block, once inlined
        // into a loop, became a header that threading copied into the latch (png's byte-add
        // loop lost loops.rs's "store; jump header" shape and its vectorization: 2.2x
        // instructions). Copies save nothing by moving, so they stay when the block would
        // otherwise be emptied down to its terminator's compare.
        if b.insts.len() - cand.len() <= 1 {
            for &(ii, _) in &cand {
                if matches!(b.insts[ii], Inst::Mov(..)) {
                    remove[ii] = false;
                }
            }
        }
        for &(ii, s) in &cand {
            if remove[ii] {
                arrivals[s].push((b.insts[ii].clone(), b.pos[ii]));
                changed = true;
            }
        }
        if remove.iter().any(|&r| r) {
            let b = &mut f.blocks[bi];
            let mut ii = 0;
            b.insts.retain(|_| { let keep = !remove[ii]; ii += 1; keep });
            ii = 0;
            b.pos.retain(|_| { let keep = !remove[ii]; ii += 1; keep });
        }
    }
    for (b, incoming) in f.blocks.iter_mut().zip(arrivals) {
        if !incoming.is_empty() {
            let (insts, pos): (Vec<_>, Vec<_>) = incoming.into_iter().rev().unzip();
            b.insts.splice(0..0, insts);
            b.pos.splice(0..0, pos);
        }
    }
    changed
}

pub fn thread_known(f: &mut Func) -> bool {
    let n = f.blocks.len();
    // a branch on a vreg whose last definition in its own block is a constant (multi-def drop
    // flags, `Iconst(c, 0); ...; Branch(c)`): a jump
    let mut folded = false;
    let (mut us, mut ds) = (Vec::new(), Vec::new());
    for b in f.blocks.iter_mut() {
        if let Term::Branch(c, t, e) = b.term {
            for i in b.insts.iter().rev() {
                if let Inst::Iconst(d, k) = i {
                    if *d == c {
                        b.term = Term::Jump(if *k != 0 { t } else { e });
                        folded = true;
                        break;
                    }
                }
                uses_defs(i, &mut us, &mut ds);
                if ds.contains(&c) {
                    break;
                }
            }
        }
    }
    // predecessors (count and the single one)
    let mut npred = vec![0u32; n];
    let mut pred1 = vec![u32::MAX; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut add = |t: u32| {
            npred[t as usize] += 1;
            pred1[t as usize] = bi as u32;
        };
        for t in b.term.succs() {
            add(t);
        }
    }
    // candidate branch blocks: few pure instructions, ending in Branch
    let cand: Vec<bool> = f
        .blocks
        .iter()
        .map(|b| matches!(b.term, Term::Branch(..)) && b.insts.len() <= THREAD_MAX_INSTS && b.insts.iter().all(|i| is_pure(i) && !matches!(i, Inst::Call(..))))
        .collect();
    if !cand.iter().any(|x| *x) {
        return folded;
    }
    // known integer values at a block's end (from its own constants and moves, entering with
    // its single predecessor's when it has one); computed on demand, a short chain
    fn eval(i: &Inst, kn: &mut std::collections::HashMap<u32, i64>, uses: &mut Vec<VReg>, defs: &mut Vec<VReg>) {
        let get = |kn: &std::collections::HashMap<u32, i64>, v: VReg| kn.get(&v.0).copied();
        let (d, val) = match i {
            Inst::Iconst(d, v) => (*d, Some(*v)),
            Inst::Mov(d, a) => (*d, get(kn, *a)),
            Inst::IBinI(op, it, d, a, k) => (*d, get(kn, *a).and_then(|x| fold_ibin(*op, *it, x, *k))),
            Inst::IBin(op, it, d, a, b) => (*d, match (get(kn, *a), get(kn, *b)) {
                (Some(x), Some(y)) => fold_ibin(*op, *it, x, y),
                _ => None,
            }),
            Inst::ICmpI(c, sg, d, a, k) => (*d, get(kn, *a).map(|x| cmp_i(*c, *sg, x, *k) as i64)),
            Inst::ICmp(c, sg, d, a, b) => (*d, match (get(kn, *a), get(kn, *b)) {
                (Some(x), Some(y)) => Some(cmp_i(*c, *sg, x, y) as i64),
                _ => None,
            }),
            Inst::Conv(c @ Conv::IntToInt(_), d, a) => (*d, get(kn, *a).and_then(|x| match fold_conv(*c, K::I(x)) {
                Some(K::I(v)) => Some(v),
                _ => None,
            })),
            _ => {
                if kn.is_empty() {
                    return;
                }
                uses_defs(i, uses, defs);
                for d in defs.iter() {
                    kn.remove(&d.0);
                }
                return;
            }
        };
        match val {
            Some(v) => {
                kn.insert(d.0, v);
            }
            None => {
                kn.remove(&d.0);
            }
        }
    }
    // loop headers (blocks with a predecessor they dominate): a path may not pass through
    // one, or P would enter its loop past the header (irreducible flow, which the dominator-
    // based passes -- CSE's join rules, LICM -- assume away)
    let mut header: Option<(Vec<Vec<u32>>, Vec<Vec<u32>>)> = None;
    let mut loop_body: std::collections::HashMap<usize, Vec<bool>> = std::collections::HashMap::new();
    let mut kn: std::collections::HashMap<u32, i64> = std::collections::HashMap::new();
    let (mut uses, mut defs) = (Vec::new(), Vec::new());
    let mut changed = false;
    for p in 0..n {
        // the path P jumps along: blocks of few pure instructions ending in Jump, then a branch
        let mut path: Vec<usize> = Vec::new();
        let mut ninst = 0;
        let mut x = match f.blocks[p].term {
            Term::Jump(b) => b as usize,
            _ => continue,
        };
        loop {
            if x == p || path.contains(&x) || path.len() >= 3 {
                break;
            }
            let blk = &f.blocks[x];
            ninst += blk.insts.len();
            if ninst > THREAD_MAX_INSTS || !blk.insts.iter().all(|i| is_pure(i) && !matches!(i, Inst::Call(..))) {
                break;
            }
            path.push(x);
            match blk.term {
                Term::Jump(y) => x = y as usize,
                _ => break,
            }
        }
        let b = match path.last() {
            Some(&b) if cand[b] => b,
            _ => continue,
        };
        // known values at P's end: walk up P's single-predecessor chain (bounded)
        let mut chain = vec![p];
        let mut q = p;
        while chain.len() < 4 && npred[q] == 1 && !chain.contains(&(pred1[q] as usize)) && matches!(f.blocks[pred1[q] as usize].term, Term::Jump(_)) {
            q = pred1[q] as usize;
            chain.push(q);
        }
        kn.clear();
        for &c in chain.iter().rev() {
            for i in &f.blocks[c].insts {
                eval(i, &mut kn, &mut uses, &mut defs);
            }
        }
        if kn.is_empty() {
            continue;
        }
        for &pb in &path {
            for i in &f.blocks[pb].insts {
                eval(i, &mut kn, &mut uses, &mut defs);
            }
        }
        let (c, t, e) = match f.blocks[b].term {
            Term::Branch(c, t, e) => (c, t, e),
            _ => continue,
        };
        let cv = match kn.get(&c.0) {
            Some(x) => *x,
            None => continue,
        };
        let target = if cv != 0 { t } else { e };
        if path.contains(&(target as usize)) {
            continue;
        }
        let hd = header.get_or_insert_with(|| {
            let idom = idoms(f);
            let (pre, post, _) = dom_numbers(&idom);
            let dominates = |a: usize, b: usize| pre[a] != u32::MAX && pre[b] != u32::MAX && pre[a] <= pre[b] && post[b] <= post[a];
            // per header its back-edge sources; per block its predecessors
            let mut h: Vec<Vec<u32>> = vec![Vec::new(); n];
            let mut pr: Vec<Vec<u32>> = vec![Vec::new(); n];
            for (bi, b) in f.blocks.iter().enumerate() {
                for t in b.term.succs() {
                    pr[t as usize].push(bi as u32);
                    if dominates(t as usize, bi) {
                        h[t as usize].push(bi as u32);
                    }
                }
            }
            (h, pr)
        });
        let (hd, pr) = (&hd.0, &hd.1);
        // a path through a loop header is fine when it leaves that loop (the target is outside the
        // header's natural loop: a latch whose flag says "done" exits straight away, as rotation
        // leaves it); anything else would enter the loop past its header
        let mut through_header = false;
        for &x in &path {
            if hd[x].is_empty() {
                continue;
            }
            if crate::stats::opt_env_off("-threadhdr") {
                through_header = true;
                break;
            }
            // the header's natural loop, once per header
            let inl = loop_body.entry(x).or_insert_with(|| {
                let mut inl = vec![false; n];
                inl[x] = true;
                let mut work: Vec<u32> = hd[x].clone();
                while let Some(y) = work.pop() {
                    if inl[y as usize] {
                        continue;
                    }
                    inl[y as usize] = true;
                    work.extend(pr[y as usize].iter().copied());
                }
                inl
            });
            if inl[target as usize] {
                through_header = true;
                break;
            }
        }
        if through_header {
            continue;
        }
        {
            // MAKEPAD_RUN_THREAD_MAX=n: only the first n threads of the process (bisection)
            static MAX: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let m = *MAX.get_or_init(|| std::env::var("MAKEPAD_RUN_THREAD_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(u64::MAX));
            if m != u64::MAX {
                let k = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if k >= m {
                    continue;
                }
                if k + 1 == m {
                    eprintln!("thread #{} in {}: b{} -> path {:?} -> b{}\n{}", k, f.name, p, path, target, f.dump());
                }
            }
        }
        let tp = f.blocks[b].term_pos;
        // all of the path's instructions (any of them may be read later, also by the same
        // block through a loop); live_dce drops the ones this path does not need
        for &pb in &path {
            let (ii, pp) = (f.blocks[pb].insts.clone(), f.blocks[pb].pos.clone());
            f.blocks[p].insts.extend(ii);
            f.blocks[p].pos.extend(pp);
        }
        f.blocks[p].term = Term::Jump(target);
        f.blocks[p].term_pos = tp;
        changed = true;
    }
    changed || folded
}

pub fn same_inst(a: &Inst, b: &Inst) -> bool {
    a == b
}

pub fn rename(i: &Inst, a: &dyn Fn(VReg) -> VReg) -> Inst {
    // operands only: definitions keep their vreg
    match i {
        Inst::Mov(d, s) => Inst::Mov(*d, a(*s)),
        Inst::IBin(o, t, d, x, y) => Inst::IBin(*o, *t, *d, a(*x), a(*y)),
        Inst::IBinI(o, t, d, x, k) => Inst::IBinI(*o, *t, *d, a(*x), *k),
        Inst::INeg(t, d, x) => Inst::INeg(*t, *d, a(*x)),
        Inst::INot(t, d, x) => Inst::INot(*t, *d, a(*x)),
        Inst::IBit(o, d, x) => Inst::IBit(*o, *d, a(*x)),
        Inst::ICmp(c, s, d, x, y) => Inst::ICmp(*c, *s, *d, a(*x), a(*y)),
        Inst::ICmpI(c, s, d, x, k) => Inst::ICmpI(*c, *s, *d, a(*x), *k),
        Inst::FBin(o, w, d, x, y) => Inst::FBin(*o, *w, *d, a(*x), a(*y)),
        Inst::FUnary(o, w, d, x) => Inst::FUnary(*o, *w, *d, a(*x)),
        Inst::FCmp(c, w, d, x, y) => Inst::FCmp(*c, *w, *d, a(*x), a(*y)),
        Inst::Conv(c, d, x) => Inst::Conv(*c, *d, a(*x)),
        Inst::Load(m, d, b, o) => Inst::Load(*m, *d, a(*b), *o),
        Inst::Store(m, b, o, s) => Inst::Store(*m, a(*b), *o, a(*s)),
        Inst::Call(c, args, r) => {
            let c2 = match c {
                Callee::Indirect(v) => Callee::Indirect(a(*v)),
                Callee::CIndirect(v, sig) => Callee::CIndirect(a(*v), sig.clone()),
                x => x.clone(),
            };
            let mut n = Vec::new();
            for v in args {
                n.push(a(*v));
            }
            Inst::Call(c2, n, r.clone())
        }
        Inst::Copy(x, y, n) => Inst::Copy(a(*x), a(*y), *n),
        Inst::AtomicLoad(k, o, d, x) => Inst::AtomicLoad(*k, *o, *d, a(*x)),
        Inst::AtomicStore(k, o, x, v) => Inst::AtomicStore(*k, *o, a(*x), a(*v)),
        Inst::AtomicRmw(op, k, o, d, x, v) => Inst::AtomicRmw(*op, *k, *o, *d, a(*x), a(*v)),
        Inst::AtomicCas(k, o1, o2, d, x, e, n) => Inst::AtomicCas(*k, *o1, *o2, *d, a(*x), a(*e), a(*n)),
        Inst::Sel(d, c, x, y) => Inst::Sel(*d, a(*c), a(*x), a(*y)),
        Inst::VSel(d, k, x, y) => Inst::VSel(*d, a(*k), a(*x), a(*y)),
        Inst::VFma(np, na, d, x, y, z) => Inst::VFma(*np, *na, *d, a(*x), a(*y), a(*z)),
        Inst::VSplat(d, x) => Inst::VSplat(*d, a(*x)),
        Inst::VExtract(d, x, l) => Inst::VExtract(*d, a(*x), *l),
        Inst::VInsert(d, v, l, x) => Inst::VInsert(*d, a(*v), *l, a(*x)),
        Inst::VLoad(d, b, o) => Inst::VLoad(*d, a(*b), *o),
        Inst::VStore(b, o, v) => Inst::VStore(a(*b), *o, a(*v)),
        Inst::VAny(d, x) => Inst::VAny(*d, a(*x)),
        Inst::VGather(d, b, i) => Inst::VGather(*d, a(*b), a(*i)),
        Inst::VTbl(d, b, i, n) => Inst::VTbl(*d, a(*b), a(*i), *n),
        Inst::VShuffle(d, x, y, k) => Inst::VShuffle(*d, a(*x), a(*y), *k),
        Inst::VX(op, d, x, y) => Inst::VX(*op, *d, a(*x), a(*y)),
        Inst::VX3(op, d, x, y, z) => Inst::VX3(*op, *d, a(*x), a(*y), a(*z)),
        Inst::FMulAdd(np, na, w, d, x, y, z) => Inst::FMulAdd(*np, *na, *w, *d, a(*x), a(*y), a(*z)),
        Inst::FpEnv(true, x) => Inst::FpEnv(true, a(*x)),
        Inst::Cpuid(ds, x, y) => Inst::Cpuid(*ds, a(*x), a(*y)),
        Inst::Xgetbv(d, x) => Inst::Xgetbv(*d, a(*x)),
        other => other.clone(),
    }
}

/// Removes pure instructions whose results are unused (and `Mov(x, x)`), with a
/// worklist: removing an instruction decrements its operands' use counts.
#[inline(never)]
pub fn dce(f: &mut Func) -> bool {
    let reach = reachable(f);
    let nv = f.vregs.len();
    let mut cnt = vec![0u32; nv];
    // defining instruction (block, index) of single-def vregs, for the cascade
    const MULTI: u32 = u32::MAX - 1;
    let mut def_at: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv];
    let mut uses = Vec::new();
    let mut total = 0usize;
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        total += b.insts.len();
        for (k, i) in b.insts.iter().enumerate() {
            for_uses(i, |u| cnt[u.0 as usize] += 1);
            for_defs(i, |d| {
                let e = &mut def_at[d.0 as usize];
                *e = if e.0 == u32::MAX { (bi as u32, k as u32) } else { (MULTI, 0) };
            });
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            cnt[u.0 as usize] += 1;
        }
    }
    if total == 0 {
        return false;
    }
    // an instruction is dead when it is a self-move or pure with every result unused; its
    // removal can make its operands' definitions dead (the set is the same fixpoint the
    // per-instruction worklist reached)
    let mut removed: Vec<Vec<bool>> = vec![Vec::new(); f.blocks.len()];
    let mut work: Vec<(u32, u32)> = Vec::new();
    let mut any = false;
    let is_dead = |i: &Inst, cnt: &[u32]| -> bool {
        match i {
            Inst::Mov(d, s) if d == s => true,
            i if is_pure(i) => {
                let mut nd = 0;
                let mut used = false;
                for_defs(i, |d| {
                    nd += 1;
                    used |= cnt[d.0 as usize] != 0;
                });
                nd > 0 && !used
            }
            _ => false,
        }
    };
    // last instruction first, cascades before the next one (the order the old all-in-one
    // worklist popped them: a multi-def operand is not revisited, so the order matters)
    for bi in (0..f.blocks.len()).rev() {
        if !reach[bi] {
            continue;
        }
        for k in (0..f.blocks[bi].insts.len()).rev() {
            work.push((bi as u32, k as u32));
            while let Some((wb, wk)) = work.pop() {
                let (wb, wk) = (wb as usize, wk as usize);
                if removed[wb].get(wk).copied().unwrap_or(false) {
                    continue;
                }
                let i = &f.blocks[wb].insts[wk];
                if !is_dead(i, &cnt) {
                    continue;
                }
                if removed[wb].is_empty() {
                    removed[wb] = vec![false; f.blocks[wb].insts.len()];
                }
                removed[wb][wk] = true;
                any = true;
                for_uses(i, |u| {
                    let c = &mut cnt[u.0 as usize];
                    *c -= 1;
                    if *c == 0 {
                        let d = def_at[u.0 as usize];
                        if d.0 < MULTI {
                            work.push(d);
                        }
                    }
                });
            }
        }
    }
    if any && !f.dbg.is_empty() {
        // removed copies / constants of single-definition values the debug table names: the
        // table keeps their value (side metadata)
        let named = f.dbg_vregs();
        let single = |v: VReg| match def_at[v.0 as usize] {
            (u32::MAX, _) => f.params.contains(&v),
            (b, _) => b < MULTI && !f.params.contains(&v),
        };
        let (mut copies, mut consts) = (Vec::new(), Vec::new());
        for (bi, rs) in removed.iter().enumerate() {
            for (k, r) in rs.iter().enumerate() {
                if !*r {
                    continue;
                }
                match &f.blocks[bi].insts[k] {
                    Inst::Mov(d, s) if d != s && named.binary_search(&d.0).is_ok() && single(*d) && single(*s) => copies.push((*d, *s)),
                    Inst::Iconst(d, v) if named.binary_search(&d.0).is_ok() && single(*d) => consts.push((*d, *v)),
                    Inst::Fconst(d, bits, _) if named.binary_search(&d.0).is_ok() && single(*d) => consts.push((*d, *bits as i64)),
                    _ => {}
                }
            }
        }
        f.dbg_salvage(&copies, &consts);
    }
    if any {
        for (bi, b) in f.blocks.iter_mut().enumerate() {
            let rs = &removed[bi];
            if rs.is_empty() {
                continue;
            }
            let mut k = 0;
            let mut j = 0;
            b.insts.retain(|_| {
                let keep = !rs[k];
                k += 1;
                keep
            });
            b.pos.retain(|_| {
                let keep = !rs[j];
                j += 1;
                keep
            });
        }
    }
    any
}

/// Redirects jumps through empty `Jump` blocks.
#[inline(never)]
pub fn thread_jumps(f: &mut Func) {
    let n = f.blocks.len();
    if n <= 1 {
        return;
    }
    let mut target: Vec<u32> = (0..n as u32).collect();
    for b in 0..n {
        let mut t = b as u32;
        let mut guard = 0;
        while guard < 16 {
            let blk = &f.blocks[t as usize];
            match blk.term {
                Term::Jump(x) if blk.insts.is_empty() && x != t => t = x,
                _ => break,
            }
            guard += 1;
        }
        target[b] = t;
    }
    for b in &mut f.blocks {
        b.term = match b.term.clone() {
            Term::Jump(x) => Term::Jump(target[x as usize]),
            Term::Branch(c, x, y) => {
                let (x, y) = (target[x as usize], target[y as usize]);
                if x == y {
                    Term::Jump(x)
                } else {
                    Term::Branch(c, x, y)
                }
            }
            t => t,
        };
    }
}

/// Reorders reachable blocks so a block's jump/false-branch target follows it, drops
/// unreachable blocks and puts panic paths (blocks ending in `Unreachable`) last.
pub fn relayout(f: &mut Func) {
    relayout_order(f);
}

/// Numbers the blocks in reverse postorder from the entry (unreachable ones after, in index
/// order). Inlining appends each body's blocks in the order sites were taken, so the same code
/// reached by a different inline order had different block numbers, and the passes and the
/// allocator that visit blocks by index (relayout's next seed, ties) made different code from it
/// (html_parse process_entity: identical RIR up to numbering, 2% more instructions).
pub fn renumber_rpo(f: &mut Func) {
    let order = crate::frame::block_rpo(f);
    if order.iter().enumerate().all(|(i, b)| i as u32 == *b) {
        return;
    }
    let mut map = vec![0u32; order.len()];
    for (i, b) in order.iter().enumerate() {
        map[*b as usize] = i as u32;
    }
    let mut old: Vec<Option<Block>> = std::mem::take(&mut f.blocks).into_iter().map(Some).collect();
    for b in &order {
        let mut blk = old[*b as usize].take().unwrap();
        blk.term.map_succs(|t| map[t as usize]);
        f.blocks.push(blk);
    }
}

/// Appends a block's single-predecessor jump target to it (straight-line code after
/// inlining and branch folding), so compares end up next to their branches.
#[inline(never)]
/// libdivide's unsigned 64-bit magic for d > 1 not a power of two: (magic, shift, add)
/// A remainder of operands a block also divides (`x / n` and `x % n`: an index split into row
/// and column) is `a - (a / b) * b` from that quotient: one division instead of two (LLVM's
/// DivRemPairs; x64 `div` and arm64 `udiv` are tens of cycles, the multiply and subtract a few).
/// The division moves up to the remainder when it comes later and nothing between needs it moved.
pub fn div_rem_pairs(f: &mut Func) -> bool {
    let mut changed = false;
    let (mut us, mut ds) = (Vec::new(), Vec::new());
    for bi in 0..f.blocks.len() {
        if !f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::IBin(IOp::Rem, ..))) {
            continue;
        }
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            let Inst::IBin(IOp::Rem, it, r, a, b) = f.blocks[bi].insts[k] else {
                k += 1;
                continue;
            };
            let insts = &f.blocks[bi].insts;
            // touches(i, v): i defines v; reads(i, v): i reads v
            let mut defines = |i: &Inst, v: VReg| {
                crate::regalloc::uses_defs(i, &mut us, &mut ds);
                ds.contains(&v)
            };
            // an earlier division of the same operands, its quotient still there
            let mut q = None;
            for j in (0..k).rev() {
                if let Inst::IBin(IOp::Div, it2, q2, a2, b2) = insts[j] {
                    if it2 == it && a2 == a && b2 == b && q2 != a && q2 != b {
                        q = Some((j, q2));
                        break;
                    }
                }
                if defines(&insts[j], a) || defines(&insts[j], b) {
                    break;
                }
            }
            let mut hoist = None;
            if let Some((j, q2)) = q {
                if insts[j + 1..k].iter().any(|i| {
                    crate::regalloc::uses_defs(i, &mut us, &mut ds);
                    ds.contains(&q2)
                }) {
                    q = None;
                }
            } else if r != a && r != b {
                // a later one: it moves up to just before the remainder when its quotient is
                // neither read nor written in between (nor is a or b written)
                for m in k + 1..insts.len() {
                    if let Inst::IBin(IOp::Div, it2, q2, a2, b2) = insts[m] {
                        if it2 == it && a2 == a && b2 == b && q2 != a && q2 != b && q2 != r {
                            let clear = insts[k + 1..m].iter().all(|i| {
                                crate::regalloc::uses_defs(i, &mut us, &mut ds);
                                !us.contains(&q2) && !ds.contains(&q2)
                            });
                            if clear {
                                hoist = Some((m, q2));
                            }
                            break;
                        }
                    }
                    crate::regalloc::uses_defs(&insts[m], &mut us, &mut ds);
                    if ds.contains(&a) || ds.contains(&b) {
                        break;
                    }
                }
            }
            let qv = match (q, hoist) {
                (Some((_, q2)), _) => q2,
                (None, Some((m, q2))) => {
                    let blk = &mut f.blocks[bi];
                    let div = blk.insts.remove(m);
                    blk.insts.insert(k, div);
                    // (positions are kept per instruction only when the block has them all)
                    if blk.pos.len() == blk.insts.len() {
                        let p = blk.pos.remove(m);
                        blk.pos.insert(k, p);
                    }
                    k += 1;
                    q2
                }
                _ => {
                    k += 1;
                    continue;
                }
            };
            let t = f.vreg(Cls::I);
            let blk = &mut f.blocks[bi];
            if blk.pos.len() == blk.insts.len() {
                let p = blk.pos[k];
                blk.pos.insert(k + 1, p);
            }
            blk.insts[k] = Inst::IBin(IOp::Mul, it, t, qv, b);
            blk.insts.insert(k + 1, Inst::IBin(IOp::Sub, it, r, a, t));
            changed = true;
            k += 2;
        }
    }
    changed
}

pub fn udiv_magic(d: u64) -> (u64, u32, bool) {
    let l = 63 - d.leading_zeros();
    let num: u128 = 1u128 << (64 + l);
    let mut m = (num / d as u128) as u64;
    let rem = (num % d as u128) as u64;
    let e = d - rem;
    if e < (1u64 << l) {
        (m.wrapping_add(1), l, false)
    } else {
        m = m.wrapping_add(m);
        let twice = rem.wrapping_add(rem);
        if twice >= d || twice < rem {
            m = m.wrapping_add(1);
        }
        (m.wrapping_add(1), l, true)
    }
}

/// Divisibility tests: `x % c == 0` / `!= 0` for an unsigned 32- or 64-bit x and a constant c (not a
/// power of two) whose remainder only these compares read. With c = c0 << s (c0 odd) and
/// inv = c0^-1 mod 2^w, `x % c == 0` exactly when `rotr(x * inv, s) <= (2^w - 1) / c` (Granlund and
/// Montgomery; LLVM's urem-seteq fold): one multiply (and a rotate) instead of a multiply-high
/// division sequence (Vec::retain(|x| x % 3 != 0)).
pub fn rem_eq_zero(f: &mut Func, kn: &[Option<K>], nv: usize) -> bool {
    let mut uses = vec![0u32; f.vregs.len()];
    let mut cmp_uses = vec![0u32; f.vregs.len()];
    let mut ndef = vec![0u32; f.vregs.len()];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| uses[u.0 as usize] += 1);
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::ICmpI(Cond::Eq | Cond::Ne, _, _, r, 0) = i {
                cmp_uses[r.0 as usize] += 1;
            }
        }
        term_uses(&b.term, &mut tu);
        for u in tu.drain(..) {
            uses[u.0 as usize] += 1;
        }
    }
    // candidates: r = x % c (unsigned 32 / 64 bits, c > 1 not a power of two), r single-def, read
    // only by `r == 0` / `r != 0`
    let mut plan: Vec<(VReg, u64, u8)> = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::IBin(IOp::Rem, it, r, _, y) = *i {
                if it.signed || !(it.bits == 32 || it.bits == 64) || (y.0 as usize) >= nv {
                    continue;
                }
                let Some(K::I(c)) = kn[y.0 as usize] else { continue };
                let c = c as u64;
                if c < 3 || c.is_power_of_two() || (it.bits == 32 && c >> 32 != 0) {
                    continue;
                }
                if ndef[r.0 as usize] == 1 && uses[r.0 as usize] > 0 && uses[r.0 as usize] == cmp_uses[r.0 as usize] {
                    plan.push((r, c, it.bits));
                }
            }
        }
    }
    if plan.is_empty() {
        return false;
    }
    for bi in 0..f.blocks.len() {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            match f.blocks[bi].insts[k] {
                Inst::IBin(IOp::Rem, it, r, x, _) if plan.iter().any(|p| p.0 == r) => {
                    let &(_, c, bits) = plan.iter().find(|p| p.0 == r).unwrap();
                    let s = c.trailing_zeros();
                    let c0 = c >> s;
                    // Newton's iteration for the inverse of an odd c0 mod 2^64 (5 steps: 3 -> 96 bits)
                    let mut inv: u64 = c0;
                    for _ in 0..5 {
                        inv = inv.wrapping_mul(2u64.wrapping_sub(c0.wrapping_mul(inv)));
                    }
                    let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
                    let inv = inv & mask;
                    let pos = f.blocks[bi].pos[k];
                    let kv = f.vreg(Cls::I);
                    let mut seq = vec![Inst::Iconst(kv, inv as i64)];
                    if s == 0 {
                        seq.push(Inst::IBin(IOp::Mul, it, r, x, kv));
                    } else {
                        let m = f.vreg(Cls::I);
                        seq.push(Inst::IBin(IOp::Mul, it, m, x, kv));
                        seq.push(Inst::IBinI(IOp::Rotl, it, r, m, (bits - s as u8) as i64));
                    }
                    let n = seq.len();
                    f.blocks[bi].insts.splice(k..k + 1, seq);
                    f.blocks[bi].pos.splice(k..k + 1, std::iter::repeat(pos).take(n));
                    k += n;
                }
                Inst::ICmpI(cc @ (Cond::Eq | Cond::Ne), _, d, r, 0) if plan.iter().any(|p| p.0 == r) => {
                    let &(_, c, bits) = plan.iter().find(|p| p.0 == r).unwrap();
                    let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
                    let lim = mask / c;
                    let pos = f.blocks[bi].pos[k];
                    let lv = f.vreg(Cls::I);
                    let seq = vec![Inst::Iconst(lv, lim as i64), Inst::ICmp(if cc == Cond::Eq { Cond::Le } else { Cond::Gt }, false, d, r, lv)];
                    f.blocks[bi].insts.splice(k..k + 1, seq);
                    f.blocks[bi].pos.splice(k..k + 1, std::iter::repeat(pos).take(2));
                    k += 2;
                }
                _ => k += 1,
            }
        }
    }
    true
}

/// `x / c` and `x % c` for an unsigned x and a constant c > 1: a power of two becomes a shift
/// / mask, anything else multiply-high and shifts (libdivide's u64 algorithm, exact for every
/// 64-bit x, so also for narrower zero-extended values). A divide takes 20-40 cycles.
pub fn div_by_const(f: &mut Func, kn: &[Option<K>], nv: usize) -> bool {
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            // by 1: x / 1 is x, x % 1 is 0 (any signedness; `len / size_of::<T>()` of byte
            // slices, `n / step` of a step of 1)
            if let Inst::IBin(op @ (IOp::Div | IOp::Rem), _, d, x, y) = f.blocks[bi].insts[k] {
                if (y.0 as usize) < nv && kn[y.0 as usize] == Some(K::I(1)) {
                    f.blocks[bi].insts[k] = if op == IOp::Div { Inst::Mov(d, x) } else { Inst::Iconst(d, 0) };
                    changed = true;
                    k += 1;
                    continue;
                }
            }
            // signed by a power of two 2^s (s < 31): bias = (x >>a 63) >>u (64 - s) is 2^s - 1 for
            // negative x, else 0; t = x + bias; quotient t >>a s, remainder x - (t & -2^s) (x is
            // sign-extended to 64 bits, so 64-bit steps are exact; the last step re-extends)
            if let Inst::IBin(op @ (IOp::Div | IOp::Rem), it, d, x, y) = f.blocks[bi].insts[k] {
                if it.signed && (y.0 as usize) < nv {
                    if let Some(K::I(c)) = kn[y.0 as usize] {
                        if c > 1 && (c as u64).is_power_of_two() && c.trailing_zeros() < 31 && c < (1i64 << (it.bits.min(64) - 1)) {
                            let sh = c.trailing_zeros() as i64;
                            let pos = f.blocks[bi].pos[k];
                            let (sg, bias, t) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
                            let mut seq = vec![
                                Inst::IBinI(IOp::Shr, I64, sg, x, 63),
                                Inst::IBinI(IOp::Shr, U64, bias, sg, 64 - sh),
                                Inst::IBin(IOp::Add, I64, t, x, bias),
                            ];
                            if op == IOp::Div {
                                seq.push(Inst::IBinI(IOp::Shr, it, d, t, sh));
                            } else {
                                let m = f.vreg(Cls::I);
                                seq.push(Inst::IBinI(IOp::And, I64, m, t, -c));
                                seq.push(Inst::IBin(IOp::Sub, it, d, x, m));
                            }
                            let n = seq.len();
                            f.blocks[bi].insts.splice(k..k + 1, seq);
                            f.blocks[bi].pos.splice(k..k + 1, std::iter::repeat(pos).take(n));
                            k += n;
                            changed = true;
                            continue;
                        }
                    }
                }
            }
            let (op, it, d, x, c) = match f.blocks[bi].insts[k] {
                Inst::IBin(op @ (IOp::Div | IOp::Rem), it, d, x, y) if !it.signed && (y.0 as usize) < nv => match kn[y.0 as usize] {
                    Some(K::I(c)) if c > 1 && (it.bits == 64 || (c as u64) < (1u64 << it.bits)) => (op, it, d, x, c as u64),
                    _ => {
                        k += 1;
                        continue;
                    }
                },
                _ => {
                    k += 1;
                    continue;
                }
            };
            let pos = f.blocks[bi].pos[k];
            let mut seq: Vec<Inst> = Vec::new();
            let q = if op == IOp::Div { d } else { f.vreg(Cls::I) };
            if c.is_power_of_two() {
                if op == IOp::Div {
                    seq.push(Inst::IBinI(IOp::Shr, U64, q, x, c.trailing_zeros() as i64));
                } else {
                    if c - 1 <= i32::MAX as u64 {
                        seq.push(Inst::IBinI(IOp::And, U64, d, x, (c - 1) as i64));
                    } else {
                        let mk = f.vreg(Cls::I);
                        seq.push(Inst::Iconst(mk, (c - 1) as i64));
                        seq.push(Inst::IBin(IOp::And, U64, d, x, mk));
                    }
                }
            } else {
                let (m, sh, add) = udiv_magic(c);
                let mk = f.vreg(Cls::I);
                seq.push(Inst::Iconst(mk, m as i64));
                let hi = f.vreg(Cls::I);
                seq.push(Inst::IBin(IOp::MulHi, U64, hi, x, mk));
                if add {
                    let t = f.vreg(Cls::I);
                    seq.push(Inst::IBin(IOp::Sub, U64, t, x, hi));
                    let t2 = f.vreg(Cls::I);
                    seq.push(Inst::IBinI(IOp::Shr, U64, t2, t, 1));
                    let t3 = f.vreg(Cls::I);
                    seq.push(Inst::IBin(IOp::Add, U64, t3, t2, hi));
                    seq.push(Inst::IBinI(IOp::Shr, U64, q, t3, sh as i64));
                } else {
                    seq.push(Inst::IBinI(IOp::Shr, U64, q, hi, sh as i64));
                }
                if op == IOp::Rem {
                    let p = f.vreg(Cls::I);
                    if c <= i32::MAX as u64 {
                        seq.push(Inst::IBinI(IOp::Mul, U64, p, q, c as i64));
                    } else {
                        let ck = f.vreg(Cls::I);
                        seq.push(Inst::Iconst(ck, c as i64));
                        seq.push(Inst::IBin(IOp::Mul, U64, p, q, ck));
                    }
                    seq.push(Inst::IBin(IOp::Sub, U64, d, x, p));
                }
            }
            if op == IOp::Rem && c.is_power_of_two() {
                // (the mask above is the whole sequence)
            }
            let _ = it;
            let n = seq.len();
            f.blocks[bi].insts.splice(k..k + 1, seq);
            f.blocks[bi].pos.splice(k..k + 1, std::iter::repeat(pos).take(n));
            k += n;
            changed = true;
        }
    }
    changed
}

/// Load combining: a value assembled from byte loads of consecutive addresses (`b0 | b1 << 8`,
/// four or eight bytes the same way; little- or big-endian order: `read_u16_le`,
/// `u16::from_be_bytes([p[0], p[1]])`) becomes one wide load (plus a byte swap for big
/// endian), as LLVM's load combining does. The wide load replaces the root `Or`, so the
/// bytes' loads must reach it with no memory write, call or block merge in between (the
/// root's block, then single-predecessor blocks back up to the loads), and the address
/// registers must have one definition.
pub fn combine_loads(f: &mut Func, ndef: &[u32]) -> bool {
    let nv = f.vregs.len();
    // single-def defining instruction per vreg: (block, index)
    let mut def_at: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv];
    let mut has_or = false;
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| {
                if ndef[d.0 as usize] == 1 {
                    def_at[d.0 as usize] = (bi as u32, k as u32);
                }
            });
            has_or |= matches!(i, Inst::IBin(IOp::Or, ..));
        }
    }
    if !has_or {
        return false;
    }
    let n = f.blocks.len();
    let mut npred = vec![0u32; n];
    let mut pred1 = vec![u32::MAX; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        for t in b.term.succs() {
            npred[t as usize] += 1;
            pred1[t as usize] = bi as u32;
        }
    }
    // (byte index within the value, bytes (a 1-, 2- or 4-byte load: little endian), address
    // key: the sorted registers summed into the address, its displacement, the load's location)
    type Lane = (u32, u32, Vec<u32>, i64, (u32, u32));
    // an address as a sum of registers plus a constant (`p + i + 1` == `(p + i) + 1`)
    fn canon(f: &Func, def_at: &[(u32, u32)], v: VReg, regs: &mut Vec<u32>, depth: u32) -> Option<i64> {
        if depth > 6 || regs.len() > 4 {
            return None;
        }
        let (bi, k) = def_at[v.0 as usize];
        if bi != u32::MAX {
            match f.blocks[bi as usize].insts[k as usize] {
                Inst::IBinI(IOp::Add, it, _, x, c) if it.bits == 64 => return Some(canon(f, def_at, x, regs, depth + 1)? + c),
                Inst::Mov(_, x) => return canon(f, def_at, x, regs, depth + 1),
                Inst::IBin(IOp::Add, it, _, a, b) if it.bits == 64 => {
                    let c1 = canon(f, def_at, a, regs, depth + 1)?;
                    return Some(c1 + canon(f, def_at, b, regs, depth + 1)?);
                }
                _ => {}
            }
        }
        regs.push(v.0);
        Some(0)
    }
    fn lanes(f: &Func, def_at: &[(u32, u32)], ndef: &[u32], v: VReg, shift: u32, out: &mut Vec<Lane>, depth: u32) -> bool {
        if depth > 12 || out.len() > 8 {
            return false;
        }
        let (bi, k) = def_at[v.0 as usize];
        if bi == u32::MAX {
            return false;
        }
        match f.blocks[bi as usize].insts[k as usize] {
            Inst::Load(Mem::Int(w @ (1 | 2 | 4), false), _, base, off) if ndef[base.0 as usize] == 1 => {
                let mut regs = Vec::new();
                match canon(f, def_at, base, &mut regs, 0) {
                    Some(c) if regs.iter().all(|r| ndef[*r as usize] == 1) => {
                        regs.sort_unstable();
                        out.push((shift, w as u32, regs, off as i64 + c, (bi, k)));
                        true
                    }
                    _ => false,
                }
            }
            Inst::LoadX(Mem::Int(w @ (1 | 2 | 4), false), _, base, idx, 0, off) if ndef[base.0 as usize] == 1 && ndef[idx.0 as usize] == 1 => {
                let mut regs = Vec::new();
                let c1 = canon(f, def_at, base, &mut regs, 0);
                let c2 = canon(f, def_at, idx, &mut regs, 0);
                match (c1, c2) {
                    (Some(a), Some(b)) if regs.iter().all(|r| ndef[*r as usize] == 1) => {
                        regs.sort_unstable();
                        out.push((shift, w as u32, regs, off as i64 + a + b, (bi, k)));
                        true
                    }
                    _ => false,
                }
            }
            // every lane's bytes must survive the node's width (a narrowing conversion or shift
            // drops the bytes above it); lane indices here are relative to `shift`
            Inst::Conv(Conv::IntToInt(t), _, x) if !t.signed && t.bits >= 8 => {
                let at = out.len();
                lanes(f, def_at, ndef, x, shift, out, depth + 1) && out[at..].iter().all(|l| l.0 - shift + l.1 <= t.bits as u32 / 8)
            }
            Inst::IBinI(IOp::Shl, it, _, x, sh) if sh % 8 == 0 && sh >= 0 && sh < 64 => {
                let at = out.len();
                lanes(f, def_at, ndef, x, shift + sh as u32 / 8, out, depth + 1) && out[at..].iter().all(|l| l.0 - shift + l.1 <= it.bits as u32 / 8)
            }
            Inst::Mov(_, x) => lanes(f, def_at, ndef, x, shift, out, depth + 1),
            Inst::IBin(IOp::Or, it, _, x, y) => {
                let at = out.len();
                lanes(f, def_at, ndef, x, shift, out, depth + 1) && lanes(f, def_at, ndef, y, shift, out, depth + 1) && out[at..].iter().all(|l| l.0 - shift + l.1 <= it.bits as u32 / 8)
            }
            _ => false,
        }
    }
    let mut changed = false;
    for bi in 0..n {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            let (it, d) = match f.blocks[bi].insts[k] {
                Inst::IBin(IOp::Or, it, d, _, _) => (it, d),
                _ => {
                    k += 1;
                    continue;
                }
            };
            let mut ls: Vec<Lane> = Vec::new();
            let root = match f.blocks[bi].insts[k] {
                Inst::IBin(IOp::Or, _, _, x, y) => (x, y),
                _ => unreachable!(),
            };
            let ok = lanes(f, &def_at, ndef, root.0, 0, &mut ls, 0) && lanes(f, &def_at, ndef, root.1, 0, &mut ls, 0);
            let nb: usize = ls.iter().map(|l| l.1 as usize).sum();
            if !ok || !(nb == 2 || nb == 4 || nb == 8) || (it.bits as usize) < nb * 8 {
                k += 1;
                continue;
            }
            let key = ls[0].2.clone();
            // every byte of the value from exactly one lane
            let mut lanes_ok = ls.iter().all(|l| l.2 == key) && {
                let mut seen = vec![false; nb];
                ls.iter().all(|l| (l.0 + l.1) as usize <= nb && (l.0..l.0 + l.1).all(|b| !std::mem::replace(&mut seen[b as usize], true)))
            };
            // a lane at byte i sits at disp0 + i (little endian; wider lanes are little-endian
            // loads themselves) or, single bytes only, at disp0 + (n-1-i) (big endian)
            let disp_of = |i: u32| ls.iter().find(|l| l.0 == i).map(|l| l.3);
            let mut big = false;
            let mut disp0 = 0;
            if lanes_ok {
                disp0 = disp_of(0).unwrap_or(i64::MIN);
                if disp_of(0).is_some() && ls.iter().all(|l| l.3 == disp0 + l.0 as i64) {
                } else if ls.iter().all(|l| l.1 == 1) && (0..nb as u32).all(|i| disp_of(i).is_some() && disp_of(i) == disp_of(0).map(|d| d - i as i64)) {
                    big = true;
                    disp0 = disp_of(nb as u32 - 1).unwrap();
                } else {
                    lanes_ok = false;
                }
            }
            if !lanes_ok {
                k += 1;
                continue;
            }
            // the byte loads reach the root without a memory effect in between: walk back from
            // the root through single-predecessor blocks
            let mut need: Vec<(u32, u32)> = ls.iter().map(|l| l.4).collect();
            let mut safe = true;
            let mut cb = bi as u32;
            let mut upto = k;
            let mut steps = 0;
            'walk: loop {
                for j in (0..upto).rev() {
                    if let Some(p) = need.iter().position(|x| *x == (cb, j as u32)) {
                        need.swap_remove(p);
                        if need.is_empty() {
                            break 'walk;
                        }
                    }
                    if is_mem_effect(&f.blocks[cb as usize].insts[j]) {
                        safe = false;
                        break 'walk;
                    }
                }
                steps += 1;
                if steps > 4 || npred[cb as usize] != 1 || cb == 0 {
                    safe = false;
                    break;
                }
                cb = pred1[cb as usize];
                upto = f.blocks[cb as usize].insts.len();
            }
            // the address registers are defined before the root (they reach the byte loads)
            if !safe {
                k += 1;
                continue;
            }
            // the wide load uses the lowest byte's own address operands
            let low = ls.iter().find(|l| l.3 == disp0).unwrap().4;
            let mem = Mem::Int(nb as u8, false);
            let t = f.vreg(Cls::I);
            let load = match f.blocks[low.0 as usize].insts[low.1 as usize] {
                Inst::Load(_, _, base, off) => Inst::Load(mem, if big { t } else { d }, base, off),
                Inst::LoadX(_, _, base, x, sh, off) => Inst::LoadX(mem, if big { t } else { d }, base, x, sh, off),
                _ => unreachable!(),
            };
            let pos = f.blocks[bi].pos[k];
            if big {
                let s2 = f.vreg(Cls::I);
                f.blocks[bi].insts[k] = Inst::IBinI(IOp::Shr, U64, d, s2, 64 - 8 * nb as i64);
                f.blocks[bi].insts.insert(k, Inst::IBit(BitOp::Bswap, s2, t));
                f.blocks[bi].pos.insert(k, pos);
                f.blocks[bi].insts.insert(k, load);
                f.blocks[bi].pos.insert(k, pos);
                k += 3;
            } else {
                f.blocks[bi].insts[k] = load;
                k += 1;
            }
            changed = true;
        }
    }
    changed
}

/// If-conversion of branches that only choose values: `if c { x = a } else { x = b }`
/// (a diamond whose arms are a few register copies, or a triangle with one such arm) becomes
/// `x = c ? a : b` (Sel: cmov / csel) and a jump to the join -- no branch to mispredict
/// (rustc / LLVM SimplifyCFG speculates such arms into selects). The arms must have the
/// branch as their only predecessor, copy into distinct registers and read none of them.
/// Where an if-converted arm's result comes from: a register, a constant, or a speculated compare /
/// select (computed into a fresh temporary before the select).
#[derive(Clone)]
enum ArmSrc {
    V(VReg),
    K(i64),
    I(Inst),
}

pub fn if_convert(f: &mut Func, spec_on: bool) -> bool {
    const MAX_MOVS: usize = 3;
    // cheap pure instructions an arm may compute into its own temporaries (speculated: they run on
    // both paths): an `Ordering` built from a compare (`if a < b { -1 } else { (a != b) as i8 }`)
    const MAX_SPEC: usize = 6;
    let n = f.blocks.len();
    if n < 3 {
        return false;
    }
    let reach = reachable(f);
    let mut npred = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for t in b.term.succs() {
                npred[t as usize] += 1;
            }
        }
    }
    let nv = f.vregs.len();
    let (mut ndef, mut nuse) = (vec![0u32; nv], vec![0u32; nv]);
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        term_uses(&b.term, &mut tu);
        for u in tu.drain(..) {
            nuse[u.0 as usize] += 1;
        }
    }
    // compares and selects only: the arms of a value choice (an Ordering), not general arithmetic
    // (speculating sums cost iterator loops more instructions than their branches: iter_chain +27%)
    // A float negation too: `if x < 0.0 { -x } else { x }` (box3d's scalar abs_v: -0.0 and NaN keep
    // their sign, so no fabs) is fcmp + fneg + fcsel, not a branch around one fneg per lane
    // (rustc's arm64 test_bounds_triangle_overlap has no such branches)
    let speculatable = |i: &Inst| matches!(i, Inst::ICmp(..) | Inst::ICmpI(..) | Inst::Sel(..) | Inst::FUnary(FUn::Neg, ..));
    // an arm: (speculated temporaries, copies (dest, source or constant), join) when block x is
    // cheap temporaries (single-def, read only in x) then copies / constants into its results and a
    // jump, entered only from the branch
    let arm = |f: &Func, npred: &[u32], x: u32| -> Option<(Vec<Inst>, Vec<(VReg, ArmSrc)>, u32)> {
        let b = &f.blocks[x as usize];
        if npred[x as usize] != 1 || b.insts.len() > MAX_MOVS + MAX_SPEC {
            return None;
        }
        // a block an earlier conversion rewrote holds its fresh temporaries (vregs made after the
        // def / use counts): no counts to judge it by, so it is no arm this time (ndef[d] was out
        // of bounds: O3 panic compiling wasm::Fx::expr under counted frequencies)
        let mut fresh = false;
        for i in &b.insts {
            for_defs(i, |d| fresh |= d.0 as usize >= nv);
            for_uses(i, |u| fresh |= u.0 as usize >= nv);
        }
        if fresh {
            return None;
        }
        let j = match b.term {
            Term::Jump(j) if j != x => j,
            _ => return None,
        };
        let mut spec = Vec::new();
        let mut mv = Vec::new();
        let mut local_uses: Vec<(VReg, u32)> = Vec::new();
        for i in &b.insts {
            for_uses(i, |u| match local_uses.iter_mut().find(|e| e.0 == u) {
                Some(e) => e.1 += 1,
                None => local_uses.push((u, 1)),
            });
        }
        for i in &b.insts {
            let mut defs = Vec::new();
            for_defs(i, |d| defs.push(d));
            let temp = defs.len() == 1 && {
                let d = defs[0];
                // temporaries this pass made for an earlier conversion (the block is now another
                // branch's arm) are past the counts: one def, one use in their block
                let (nd, nu) = (ndef.get(d.0 as usize).copied().unwrap_or(1), nuse.get(d.0 as usize).copied().unwrap_or(1));
                nd == 1 && nu > 0 && local_uses.iter().any(|e| e.0 == d && e.1 == nu)
            };
            match i {
                _ if temp && speculatable(i) && mv.is_empty() => spec.push(i.clone()),
                Inst::Iconst(..) if temp && mv.is_empty() => spec.push(i.clone()),
                Inst::Mov(d, s) if d != s && matches!(f.vregs[d.0 as usize], Cls::I | Cls::F32 | Cls::F64) => mv.push((*d, ArmSrc::V(*s))),
                Inst::Iconst(d, k) if f.vregs[d.0 as usize] == Cls::I => mv.push((*d, ArmSrc::K(*k))),
                // a result computed by a select / compare (the phi machinery's boolean shadow):
                // speculated into a fresh temporary, then copied (Spec)
                Inst::Sel(d, ..) | Inst::ICmp(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) if defs.len() == 1 && f.vregs[d.0 as usize] == Cls::I && { let mut clean = true; for_uses(i, |u| clean &= !mv.iter().any(|(md, _)| *md == u)); clean } => mv.push((*d, ArmSrc::I(i.clone()))),
                _ => return None,
            }
        }
        if spec.len() > MAX_SPEC || mv.len() > MAX_MOVS || (!spec_on && (!spec.is_empty() || mv.iter().any(|(_, s)| !matches!(s, ArmSrc::V(_))))) {
            return None;
        }
        Some((spec, mv, j))
    };
    let mut changed = false;
    for bi in 0..n {
        if !reach[bi] {
            continue;
        }
        let (c, t, e) = match f.blocks[bi].term {
            Term::Branch(c, t, e) if t != e && t as usize != bi && e as usize != bi => (c, t, e),
            _ => continue,
        };
        let (st, mt0, se, me0, j) = match (arm(f, &npred, t), arm(f, &npred, e)) {
            (Some((st, mt, jt)), Some((se, me, je))) if jt == je => (st, mt, se, me, jt),
            (Some((st, mt, jt)), _) if jt == e => (st, mt, Vec::new(), Vec::new(), e),
            (_, Some((se, me, je))) if je == t => (Vec::new(), Vec::new(), se, me, t),
            _ => continue,
        };
        if mt0.is_empty() && me0.is_empty() {
            continue;
        }
        let konst = |m: &Vec<(VReg, ArmSrc)>| m.iter().filter(|(_, s)| !matches!(s, ArmSrc::V(_))).count();
        // speculation only for a choice between constants and compare outcomes (an Ordering-like
        // value): an arm that also copies values keeps its branch (iterator loops: position / any /
        // count arms speculated cost iter_chain +26% instructions)
        let speculated = !st.is_empty() || !se.is_empty() || konst(&mt0) + konst(&me0) > 0;
        if speculated && mt0.iter().chain(me0.iter()).any(|(_, s)| matches!(s, ArmSrc::V(v) if !st.iter().chain(se.iter()).any(|i| { let mut d = false; for_defs(i, |x| d |= x == *v); d }))) {
            continue;
        }
        if st.len() + se.len() + konst(&mt0) + konst(&me0) > MAX_SPEC {
            continue;
        }
        let as_movs = |m: &Vec<(VReg, ArmSrc)>| -> Vec<(VReg, VReg)> { m.iter().map(|(d, s)| (*d, if let ArmSrc::V(v) = s { *v } else { *d })).collect() };
        let (mt, me) = (as_movs(&mt0), as_movs(&me0));
        // destinations, each once per arm; sources and the condition are not among them
        let mut dests: Vec<VReg> = Vec::new();
        let mut ok = true;
        for arm_movs in [&mt, &me] {
            for (k, (d, _)) in arm_movs.iter().enumerate() {
                if arm_movs[..k].iter().any(|x| x.0 == *d) {
                    ok = false;
                }
                if !dests.contains(d) {
                    dests.push(*d);
                }
            }
        }
        if !ok || dests.contains(&c) || mt0.iter().chain(me0.iter()).any(|(_, s)| match s { ArmSrc::V(v) => dests.contains(v), ArmSrc::I(i) => { let mut r = false; for_uses(i, |u| r |= dests.contains(&u)); r } ArmSrc::K(_) => false }) {
            continue;
        }
        // a choice the join then branches on stays a branch: the branch can be threaded to
        // its known outcome on each arm (an Option's Some / None test after a select would
        // turn a threadable branch into a select plus a test)
        {
            let mut taint = dests.clone();
            let mut seen = Vec::new();
            let mut at = j;
            let mut feeds_branch = false;
            loop {
                if seen.contains(&at) {
                    break;
                }
                seen.push(at);
                for i in &f.blocks[at as usize].insts {
                    let mut hit = false;
                    for_uses(i, |u| hit |= taint.contains(&u));
                    if hit {
                        for_defs(i, |d| taint.push(d));
                    }
                }
                match f.blocks[at as usize].term {
                    Term::Jump(next) => at = next,
                    Term::Branch(jc, ..) => {
                        feeds_branch = taint.contains(&jc);
                        break;
                    }
                    _ => break,
                }
            }
            if feeds_branch {
                continue;
            }
        }
        let pos = f.blocks[bi].term_pos;
        // the speculated temporaries run here, both arms' (they only define arm-local values);
        // an arm's constant goes through a fresh temporary
        let mut mt = mt;
        let mut me = me;
        for i in st.iter().chain(se.iter()) {
            f.blocks[bi].insts.push(i.clone());
            f.blocks[bi].pos.push(pos);
        }
        for (src, dst) in [(&mt0, &mut mt), (&me0, &mut me)] {
            for (k, (d, s2)) in src.iter().enumerate() {
                let hoisted = match s2 {
                    ArmSrc::V(_) => continue,
                    ArmSrc::K(kv) => {
                        let tmp = f.vreg(f.vregs[d.0 as usize]);
                        (Inst::Iconst(tmp, *kv), tmp)
                    }
                    ArmSrc::I(i) => {
                        let tmp = f.vreg(f.vregs[d.0 as usize]);
                        let i2 = match i.clone() {
                            Inst::Sel(_, c2, a2, b2) => Inst::Sel(tmp, c2, a2, b2),
                            Inst::ICmp(cc, sg, _, a2, b2) => Inst::ICmp(cc, sg, tmp, a2, b2),
                            Inst::ICmpI(cc, sg, _, a2, k2) => Inst::ICmpI(cc, sg, tmp, a2, k2),
                            other => other,
                        };
                        (i2, tmp)
                    }
                };
                f.blocks[bi].insts.push(hoisted.0);
                f.blocks[bi].pos.push(pos);
                dst[k] = (*d, hoisted.1);
            }
        }
        for d in dests {
            let a = mt.iter().find(|x| x.0 == d).map_or(d, |x| x.1);
            let b = me.iter().find(|x| x.0 == d).map_or(d, |x| x.1);
            // the same value on both paths: a plain copy (none if it is d's own)
            let i = if a == b { Inst::Mov(d, a) } else { Inst::Sel(d, c, a, b) };
            if a == d && b == d {
                continue;
            }
            f.blocks[bi].insts.push(i);
            f.blocks[bi].pos.push(pos);
        }
        f.blocks[bi].term = Term::Jump(j);
        // the arms are entered from here only: now unreachable
        npred[t as usize] = 0;
        npred[e as usize] = npred[e as usize].saturating_sub(1);
        changed = true;
    }
    changed
}

/// A compare's outcome over a select tree (simplify's select-tree fold): a leaf (Ok: constant 0 / 1,
/// Err: a 0 / 1 value or its negation) or a select on a condition.
#[derive(Clone, PartialEq)]
enum OTree {
    L(Result<i64, (VReg, bool)>),
    N(VReg, Box<OTree>, Box<OTree>),
}

/// A branch on a select of constants and compare outcomes (`c ? k : x`, what if-conversion of a
/// callee's `Ordering` / `Option<Ordering>` leaves once the caller tests it) branches on c, then
/// on the chosen arm (LLVM JumpThreading's select unfolding): the arms' compares become branches
/// edge facts and threading can decide (`a < b` re-tested on the other path). The select must be
/// read only by the branch; a constant arm goes straight to its target.
pub fn unfold_branch_selects(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let (mut ndef, mut nuse) = (vec![0u32; nv], vec![0u32; nv]);
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    // single definitions: 0 = other, 1 = compare outcome, 2 = constant (value in kv)
    let mut kind = vec![0u8; nv];
    let mut kv = vec![0i64; nv];
    let mut sel: Vec<Option<(usize, VReg, VReg, VReg)>> = vec![None; nv];
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            for_uses(i, |u| nuse[u.0 as usize] += 1);
            match *i {
                Inst::ICmp(_, _, d, ..) | Inst::ICmpI(_, _, d, ..) | Inst::FCmp(_, _, d, ..) => kind[d.0 as usize] = 1,
                Inst::Iconst(d, k) => {
                    kind[d.0 as usize] = 2;
                    kv[d.0 as usize] = k;
                }
                Inst::Sel(d, c, a, x) if f.vregs[d.0 as usize] == Cls::I => sel[d.0 as usize] = Some((bi, c, a, x)),
                _ => {}
            }
        }
        term_uses(&b.term, &mut tu);
        for u in tu.drain(..) {
            nuse[u.0 as usize] += 1;
        }
    }
    let reach = reachable(f);
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        if !reach.get(bi).copied().unwrap_or(false) {
            continue;
        }
        let (v, t, e) = match f.blocks[bi].term {
            Term::Branch(v, t, e) if t != e => (v, t, e),
            _ => continue,
        };
        let Some((sb, c, a, x)) = sel[v.0 as usize] else { continue };
        let ok = |w: VReg| ndef[w.0 as usize] == 1 && kind[w.0 as usize] != 0;
        if sb != bi || ndef[v.0 as usize] != 1 || nuse[v.0 as usize] != 1 || ndef[c.0 as usize] != 1 || !ok(a) || !ok(x) {
            continue;
        }
        let pos = f.blocks[bi].term_pos;
        let target = |f: &mut Func, w: VReg| -> u32 {
            if kind[w.0 as usize] == 2 {
                return if kv[w.0 as usize] != 0 { t } else { e };
            }
            let nb = f.block();
            f.blocks[nb as usize].term = Term::Branch(w, t, e);
            f.blocks[nb as usize].term_pos = pos;
            nb
        };
        let ta = target(f, a);
        let tx = target(f, x);
        f.blocks[bi].term = Term::Branch(c, ta, tx);
        changed = true;
    }
    changed
}

pub fn merge_blocks(f: &mut Func) -> bool {
    let n = f.blocks.len();
    if n <= 1 {
        return false;
    }
    let reach = reachable(f);
    let mut preds = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for t in b.term.succs() {
            preds[t as usize] += 1;
        }
    }
    preds[0] += 1; // entry
    let mut changed = false;
    for bi in 0..n {
        if !reach[bi] {
            continue;
        }
        loop {
            let t = match f.blocks[bi].term {
                Term::Jump(t) if t as usize != bi && preds[t as usize] == 1 => t as usize,
                _ => break,
            };
            let tb = std::mem::replace(&mut f.blocks[t], Block { insts: Vec::new(), pos: Vec::new(), term: Term::Unreachable, term_pos: 0, count: -1.0 });
            let b = &mut f.blocks[bi];
            b.insts.extend(tb.insts);
            b.pos.extend(tb.pos);
            b.term = tb.term;
            b.term_pos = tb.term_pos;
            preds[t] = 0;
            changed = true;
        }
    }
    changed
}

/// Moves single-use constant definitions next to their use in the same block, so
/// constants do not occupy registers across long expressions.
pub fn sink_consts(f: &mut Func) {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| {
                nuse[u.0 as usize] += 1;
            });
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
            });
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    // constants used only on a cold path (a block ending in Unreachable: panics) move there
    let nb = f.blocks.len();
    let mut use_block = vec![u32::MAX; nv]; // u32::MAX-1: several blocks
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                let ub = &mut use_block[u.0 as usize];
                if *ub == u32::MAX {
                    *ub = bi as u32;
                } else if *ub != bi as u32 {
                    *ub = u32::MAX - 1;
                }
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            let ub = &mut use_block[u.0 as usize];
            if *ub == u32::MAX {
                *ub = bi as u32;
            } else if *ub != bi as u32 {
                *ub = u32::MAX - 1;
            }
        }
    }
    for bi in 0..nb {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            let d = match &f.blocks[bi].insts[k] {
                Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) if ndef[d.0 as usize] == 1 => *d,
                _ => {
                    k += 1;
                    continue;
                }
            };
            let ub = use_block[d.0 as usize];
            if ub < nb as u32 && ub != bi as u32 && matches!(f.blocks[ub as usize].term, Term::Unreachable | Term::Resume) {
                let i = f.blocks[bi].insts.remove(k);
                let p = f.blocks[bi].pos.remove(k);
                f.blocks[ub as usize].insts.insert(0, i);
                f.blocks[ub as usize].pos.insert(0, p);
            } else {
                k += 1;
            }
        }
    }
    // a stack slot's address used in other blocks is computed again where it is used (a
    // frame-pointer add): one long-lived register per escaping slot from the entry block
    // (SROA's write-back addresses, lowering's slots) spilled and reloaded otherwise
    let mut slot_of: Vec<u32> = vec![u32::MAX; nv];
    let mut any_slot = false;
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, sl) = i {
                if ndef[d.0 as usize] == 1 {
                    slot_of[d.0 as usize] = *sl;
                    any_slot = true;
                }
            }
        }
    }
    if any_slot {
        // per block: the slot-address vregs it uses that are defined elsewhere
        let mut def_block = vec![u32::MAX; nv];
        for (bi, b) in f.blocks.iter().enumerate() {
            for i in &b.insts {
                if let Inst::SlotAddr(d, _) = i {
                    if slot_of[d.0 as usize] != u32::MAX {
                        def_block[d.0 as usize] = bi as u32;
                    }
                }
            }
        }
        let mut still_used = vec![false; nv];
        for bi in 0..nb {
            let mut map: Vec<(VReg, VReg)> = Vec::new();
            let mut k = 0;
            while k < f.blocks[bi].insts.len() {
                let mut need: Vec<VReg> = Vec::new();
                for_uses(&f.blocks[bi].insts[k], |u| {
                    if slot_of.get(u.0 as usize).map_or(false, |x| *x != u32::MAX) && def_block[u.0 as usize] != bi as u32 && !need.contains(&u) && !map.iter().any(|x| x.0 == u) {
                        need.push(u);
                    }
                });
                for u in need {
                    let nvr = f.vreg(Cls::I);
                    let p = f.blocks[bi].pos[k];
                    f.blocks[bi].insts.insert(k, Inst::SlotAddr(nvr, slot_of[u.0 as usize]));
                    f.blocks[bi].pos.insert(k, p);
                    k += 1;
                    map.push((u, nvr));
                }
                if !map.is_empty() {
                    f.blocks[bi].insts[k].vregs_mut(|v| {
                        if let Some(x) = map.iter().find(|x| x.0 == *v) {
                            *v = x.1;
                        }
                    });
                }
                for_uses(&f.blocks[bi].insts[k], |u| {
                    if slot_of.get(u.0 as usize).map_or(false, |x| *x != u32::MAX) && def_block[u.0 as usize] == bi as u32 {
                        still_used[u.0 as usize] = true;
                    }
                });
                k += 1;
            }
            term_uses(&f.blocks[bi].term, &mut uses);
            for u in uses.clone() {
                if !slot_of.get(u.0 as usize).map_or(false, |x| *x != u32::MAX) {
                    continue;
                }
                if def_block[u.0 as usize] == bi as u32 {
                    still_used[u.0 as usize] = true;
                    continue;
                }
                let nvr = match map.iter().find(|x| x.0 == u) {
                    Some(x) => x.1,
                    None => {
                        let nvr = f.vreg(Cls::I);
                        let at = if matches!(f.blocks[bi].term, Term::Invoke(..)) && !f.blocks[bi].insts.is_empty() { f.blocks[bi].insts.len() - 1 } else { f.blocks[bi].insts.len() };
                        let p = f.blocks[bi].term_pos;
                        f.blocks[bi].insts.insert(at, Inst::SlotAddr(nvr, slot_of[u.0 as usize]));
                        f.blocks[bi].pos.insert(at, p);
                        map.push((u, nvr));
                        nvr
                    }
                };
                match &mut f.blocks[bi].term {
                    Term::Branch(c, _, _) if *c == u => *c = nvr,
                    Term::Ret(vs) => vs.iter_mut().for_each(|v| {
                        if *v == u {
                            *v = nvr
                        }
                    }),
                    _ => {}
                }
            }
        }
        // definitions nothing in their own block reads any more
        for b in &mut f.blocks {
            let mut k = 0;
            while k < b.insts.len() {
                if let Inst::SlotAddr(d, _) = b.insts[k] {
                    if slot_of.get(d.0 as usize).map_or(false, |x| *x != u32::MAX) && !still_used[d.0 as usize] {
                        b.insts.remove(k);
                        b.pos.remove(k);
                        continue;
                    }
                }
                k += 1;
            }
        }
    }
    let nv = f.vregs.len();
    let mut nuse = vec![0u32; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| nuse[u.0 as usize] += 1);
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    for b in &mut f.blocks {
        let n = b.insts.len();
        let mut out: Vec<Inst> = Vec::with_capacity(n);
        let mut out_pos: Vec<u64> = Vec::with_capacity(n);
        // constants waiting for their use: (vreg, inst, pos)
        let mut held: Vec<(u32, Inst, u64)> = Vec::new();
        for k in 0..n {
            let i = b.insts[k].clone();
            let p = b.pos[k];
            let movable = match &i {
                Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) | Inst::SlotAddr(d, _) => ndef[d.0 as usize] == 1 && nuse[d.0 as usize] == 1,
                _ => false,
            };
            if movable {
                let d = match &i {
                    Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) | Inst::SlotAddr(d, _) => d.0,
                    _ => 0,
                };
                held.push((d, i, p));
                continue;
            }
            uses_defs(&i, &mut uses, &mut defs);
            for u in &uses {
                let mut j = 0;
                while j < held.len() {
                    if held[j].0 == u.0 {
                        let (_, hi, hp) = held.remove(j);
                        out.push(hi);
                        out_pos.push(hp);
                    } else {
                        j += 1;
                    }
                }
            }
            out.push(i);
            out_pos.push(p);
        }
        // the rest are used by the terminator or in other blocks (before an invoked call,
        // which stays last)
        let at = if matches!(b.term, Term::Invoke(..)) && !out.is_empty() { out.len() - 1 } else { out.len() };
        for (k, (_, hi, hp)) in held.into_iter().enumerate() {
            out.insert(at + k, hi);
            out_pos.insert(at + k, hp);
        }
        b.insts = out;
        b.pos = out_pos;
    }
}

/// Within a block, uses of `a` after `Mov(a, b)` read `b` directly until either is
/// redefined (handles copies of multi-definition vregs that global propagation skips).
#[inline(never)]
/// Forward little-endian words assembled by byte stores. Track the current values of
/// mutable RIR registers, and analyse all replacements before changing instruction indices.
pub fn forward_narrow_stores(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut defs = vec![None; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut preds = vec![Vec::new(); f.blocks.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        for t in b.term.succs() {
            preds[t as usize].push(bi);
        }
        for (ii, inst) in b.insts.iter().enumerate() {
            for_defs(inst, |d| {
                ndef[d.0 as usize] += 1;
                defs[d.0 as usize] = Some((bi, ii));
            });
        }
    }
    // Resolve simple address derivations. Unknown provenance must not justify moving
    // a load across stores: it might point back into the destination slot.
    fn root(f: &Func, defs: &[Option<(usize, usize)>], ndef: &[u32], v: VReg, depth: usize) -> Option<(bool, u32)> {
        if depth > 32 || ndef[v.0 as usize] != 1 {
            return None;
        }
        let Some((bi, ii)) = defs[v.0 as usize] else { return Some((false, v.0)) };
        match f.blocks[bi].insts[ii] {
            Inst::SlotAddr(_, sl) => Some((true, sl)),
            Inst::Mov(_, a) | Inst::IBinI(IOp::Add | IOp::Sub, IntTy { bits: 64, .. }, _, a, _) => root(f, defs, ndef, a, depth + 1),
            _ => None,
        }
    }
    let sc = slot_scan(f);
    // a base address: the slot for a SlotAddr (several SlotAddr vregs of one slot are one base),
    // else the vreg
    let canon = |v: VReg| -> (bool, u32) {
        if ndef[v.0 as usize] == 1 {
            if let Some((bi, ii)) = defs[v.0 as usize] {
                if let Inst::SlotAddr(_, sl) = f.blocks[bi].insts[ii] {
                    return (true, sl);
                }
            }
        }
        (false, v.0)
    };
    // (value, store index, value version)
    type Byte = (VReg, usize, u32);
    let mut edits = Vec::new();
    let mut versions = vec![0u32; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut base = None;
        let mut bytes = std::collections::HashMap::<i64, Byte>::new();
        for (li, inst) in b.insts.iter().enumerate() {
            if let Inst::Load(m, d, a, off) = *inst {
                let n = mem_size(m) as usize;
                if matches!(n, 2 | 4 | 8) && base == Some(canon(a)) {
                    let lanes: Option<Vec<Byte>> = (0..n).map(|i| bytes.get(&(off as i64 + i as i64)).copied()).collect();
                    if let Some(lanes) = lanes {
                        if lanes.iter().all(|&(v, _, ver)| versions[v.0 as usize] == ver) {
                            let direct = (|| {
                                let mut loads = Vec::new();
                                let mut source = None;
                                for (i, &(v, si, _)) in lanes.iter().enumerate() {
                                    if ndef[v.0 as usize] != 1 { return None; }
                                    let loc = defs[v.0 as usize]?;
                                    let Inst::Load(Mem::Int(1, false), _, p, k) = f.blocks[loc.0].insts[loc.1] else { return None };
                                    let key = (p, k as i64 - i as i64);
                                    if source.is_some_and(|s| s != key) || (loc.0 == bi && loc.1 >= si) { return None; }
                                    source = Some(key);
                                    loads.push(loc);
                                }
                                let (p, k) = source?;
                                // the word's source may be anything (a loop's pointer) when the
                                // destination is a stack slot whose address goes nowhere but load /
                                // store bases: nothing else can point into it
                                let br = root(f, &defs, &ndef, a, 0)?;
                                let pr = match root(f, &defs, &ndef, p, 0) {
                                    Some(r) => r,
                                    None if br.0 && !sc.esc[br.1 as usize] => (false, u32::MAX),
                                    None => return None,
                                };
                                if pr == br || !(pr.0 || br.0 || f.noalias.contains(&p) || f.noalias.contains(&a)) { return None; }
                                // Follow single-predecessor blocks (a Branch may have another
                                // successor, but the next block has just one entry).
                                let mut chain = vec![bi];
                                while !loads.iter().all(|l| chain.contains(&l.0)) {
                                    let cur = *chain.last()?;
                                    if cur == 0 || preds[cur].len() != 1 { return None; }
                                    let pred = preds[cur][0];
                                    if chain.contains(&pred) || !matches!(f.blocks[pred].term, Term::Jump(_) | Term::Branch(..)) { return None; }
                                    chain.push(pred);
                                }
                                chain.reverse();
                                let first = loads.iter().map(|&(lb, ii)| (chain.iter().position(|&c| c == lb).unwrap(), ii)).min()?;
                                for (ci, &cb) in chain.iter().enumerate().skip(first.0) {
                                    let start = if ci == first.0 { first.1 } else { 0 };
                                    let end = if cb == bi { li } else { f.blocks[cb].insts.len() };
                                    for ii in start..end {
                                        if is_mem_effect(&f.blocks[cb].insts[ii]) && !(cb == bi && lanes.iter().any(|l| l.1 == ii)) { return None; }
                                    }
                                }
                                Some(Inst::Load(m, d, p, i32::try_from(k).ok()?))
                            })();
                            edits.push((bi, li, m, d, lanes, direct));
                        }
                    }
                }
            }
            match *inst {
                Inst::Store(m, a, off, v) => {
                    if base != Some(canon(a)) {
                        bytes.clear();
                        base = Some(canon(a));
                    }
                    for i in 0..mem_size(m) {
                        bytes.remove(&(off as i64 + i as i64));
                    }
                    if matches!(m, Mem::Int(1, _)) {
                        bytes.insert(off as i64, (v, li, versions[v.0 as usize]));
                    }
                }
                _ if is_mem_effect(inst) => bytes.clear(),
                _ => {}
            }
            for_defs(inst, |d| {
                versions[d.0 as usize] += 1;
                if base == Some((false, d.0)) { bytes.clear(); }
            });
        }
    }
    let changed = !edits.is_empty();
    for (bi, li, m, d, lanes, direct) in edits.into_iter().rev() {
        let mut ins = Vec::new();
        if let Some(load) = direct {
            ins.push(load);
        } else {
            let ty = IntTy { bits: mem_size(m) as u8 * 8, signed: false };
            let mut acc = None;
            for (i, (v, _, _)) in lanes.into_iter().enumerate() {
                // A byte store discards all high bits, even for signed or wide inputs.
                let byte = f.vreg(Cls::I);
                ins.push(Inst::Conv(Conv::IntToInt(IntTy { bits: 8, signed: false }), byte, v));
                let shifted = if i == 0 { byte } else {
                    let t = f.vreg(Cls::I);
                    ins.push(Inst::IBinI(IOp::Shl, ty, t, byte, (8 * i) as i64));
                    t
                };
                acc = Some(if let Some(prev) = acc {
                    let t = f.vreg(Cls::I);
                    ins.push(Inst::IBin(IOp::Or, ty, t, prev, shifted));
                    t
                } else { shifted });
            }
            let conv = match m {
                Mem::Int(n, signed) => Conv::IntToInt(IntTy { bits: n * 8, signed }),
                Mem::F32 => Conv::BitsToF(false),
                Mem::F64 => Conv::BitsToF(true),
            };
            ins.push(Inst::Conv(conv, d, acc.unwrap()));
        }
        let b = &mut f.blocks[bi];
        let pos = b.pos[li];
        b.pos.splice(li..=li, std::iter::repeat(pos).take(ins.len()));
        b.insts.splice(li..=li, ins);
    }
    changed
}

pub fn local_copy_prop(f: &mut Func) -> bool {
    let mut changed = false;
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut map: Vec<(VReg, VReg)> = Vec::new();
    for b in &mut f.blocks {
        map.clear();
        for k in 0..b.insts.len() {
            if !map.is_empty() {
                let look = |v: VReg| -> VReg {
                    for (a, s) in map.iter() {
                        if *a == v {
                            return *s;
                        }
                    }
                    v
                };
                let n = rename(&b.insts[k], &look);
                if n != b.insts[k] {
                    b.insts[k] = n;
                    changed = true;
                }
            }
            uses_defs(&b.insts[k], &mut uses, &mut defs);
            for d in &defs {
                map.retain(|(a, s)| a != d && s != d);
            }
            if let Inst::Mov(d, s) = b.insts[k] {
                if d != s {
                    map.push((d, s));
                }
            }
        }
        if !map.is_empty() {
            let look = |v: VReg| -> VReg {
                for (a, s) in map.iter() {
                    if *a == v {
                        return *s;
                    }
                }
                v
            };
            let t = match &b.term {
                Term::Branch(c, x, y) => Term::Branch(look(*c), *x, *y),
                Term::Ret(v) => {
                    let mut n = Vec::new();
                    for x in v {
                        n.push(look(*x));
                    }
                    Term::Ret(n)
                }
                t => t.clone(),
            };
            b.term = t;
        }
    }
    changed
}

/// Path state of the CSE walk kept in per-vreg arrays (set and cleared with an undo log,
/// so entering and leaving a dominator subtree costs only what it changed).
pub struct Facts {
    /// branch facts: vreg has this value
    pub fact: Vec<Option<i64>>,
    /// unsigned upper bound: vreg <u bound (the tightest known)
    pub ub: Vec<Option<i64>>,
    /// unsigned lower bound: vreg >u bound (the tightest known)
    pub lb: Vec<Option<i64>>,
    /// (which array, vreg, previous value)
    pub undo: Vec<(u8, u32, Option<i64>)>,
    /// multi-definition vregs with a fact set on the current path (a join re-checks them)
    pub multi: Vec<u32>,
    /// undo length -> multi length marks
    pub multi_undo: Vec<(usize, usize)>,
}

impl Facts {
    fn arr(&mut self, k: u8) -> &mut Vec<Option<i64>> {
        match k {
            0 => &mut self.fact,
            1 => &mut self.ub,
            _ => &mut self.lb,
        }
    }
    fn set(&mut self, k: u8, v: VReg, x: Option<i64>, multi: bool) {
        let old = self.arr(k)[v.0 as usize];
        if old == x {
            return;
        }
        self.undo.push((k, v.0, old));
        self.arr(k)[v.0 as usize] = x;
        if multi && x.is_some() {
            self.multi_undo.push((self.undo.len(), self.multi.len()));
            self.multi.push(v.0);
        }
    }
    fn tighten_ub(&mut self, v: VReg, b: i64, multi: bool) {
        match self.ub[v.0 as usize] {
            Some(o) if (o as u64) <= (b as u64) => {}
            _ => self.set(1, v, Some(b), multi),
        }
    }
    fn tighten_lb(&mut self, v: VReg, b: i64, multi: bool) {
        match self.lb[v.0 as usize] {
            Some(o) if (o as u64) >= (b as u64) => {}
            _ => self.set(2, v, Some(b), multi),
        }
    }
    fn clear(&mut self, v: VReg) {
        for k in 0..3 {
            if self.arr(k)[v.0 as usize].is_some() {
                self.set(k, v, None, false);
            }
        }
    }
    fn rewind(&mut self, mark: usize) {
        while self.undo.len() > mark {
            let (k, v, old) = self.undo.pop().unwrap();
            self.arr(k)[v as usize] = old;
        }
        while let Some(&(u, m)) = self.multi_undo.last() {
            if u <= mark {
                break;
            }
            self.multi.truncate(m);
            self.multi_undo.pop();
        }
    }
}

#[derive(Clone, Default)]
pub struct CseState {
    /// (expression key, dest, operands, is load)
    pub avail: Vec<([u64; 3], VReg, [VReg; 2], bool)>,
    pub copies: Vec<(VReg, VReg)>,
}

pub enum CseWork {
    /// block, its inherited state, the edge it was entered by (Some((cond vreg, value,
    /// compare behind the cond)) for a direct child; None for a join child)
    Enter(u32, CseState, Option<Option<(VReg, i64, Option<(Cond, VReg, i64)>)>>, u32),
    Leave(usize),
}

/// CSE over the dominator tree. A block entered through its only predecessor keeps all
/// state of that predecessor plus the branch fact of the edge (`c` true/false, and for an
/// unsigned `x < k` compare the bound `x <u k`). A block reached from its immediate
/// dominator through a join keeps only state about single-definition vregs (their values
/// cannot change on the way). Repeated compares fold, so bounds checks implied by a loop
/// condition (also through copies of the loop variable) disappear.
/// Cost: linear in the function plus, per join, the multi-definition facts on the path.
/// Whether local_cse can change anything: it replaces an expression whose key an earlier
/// one (or a store, for a load) produced, renames uses of `Mov` copies, and folds with
/// facts that only branches create. Without a branch, a copy or a repeated key it is a no-op.
pub fn cse_can_change(f: &Func) -> bool {
    let mut keys: Vec<[u64; 3]> = Vec::new();
    for b in &f.blocks {
        if matches!(b.term, Term::Branch(..)) {
            return true;
        }
        for i in &b.insts {
            match i {
                Inst::Mov(..) => return true,
                Inst::Store(m, base, off, _) => {
                    if let Some(k) = cse_key(&Inst::Load(*m, VReg(0), *base, *off)) {
                        keys.push(k);
                    }
                }
                Inst::SlotAddr(..) => {}
                i if is_mem_effect(i) => {}
                _ => {
                    if let Some(k) = cse_key(i) {
                        keys.push(k);
                    }
                }
            }
        }
    }
    keys.sort_unstable();
    keys.windows(2).any(|w| w[0] == w[1])
}

/// The pointer roots local_cse and licm ask alias questions with. `ndef` is the def count per
/// vreg (params counted). Returns, per vreg, `1<<40 | p` for an address derived from noalias
/// param p by address arithmetic, LOADED_ROOT for a pointer not derived from any noalias param
/// (other params, loaded pointers, call results: it may point anywhere except into a noalias
/// param's memory -- rustc marks `&mut T` (T: Unpin) and frozen `&T` params LLVM `noalias`:
/// rustc_ty_utils/src/abi.rs, `no_alias`; LLVM's "based on" does not pass through memory),
/// 1+slot for a stack slot, 0 unknown; then the noalias params whose address this function
/// stores or returns (captured: left alone, cheap insurance for unsafe code in our own std);
/// then per vreg the noalias param whose address it may hold through any def, to a fixpoint
/// (u32::MAX: none; call arguments). Params in `f.noalias` only: withheld ones (code that runs
/// before its borrow-check verdict) get no roots, so nothing is concluded from them.
pub const LOADED_ROOT: u64 = 3u64 << 40;

pub fn noalias_roots(f: &Func, ndef: &[u32]) -> (Vec<u64>, Vec<u32>, Vec<u32>) {
    const LOADED: u64 = LOADED_ROOT;
    let nv = f.vregs.len();
    let single = |v: VReg| ndef[v.0 as usize] == 1;
    let mut root = vec![0u64; nv];
    for p in &f.noalias {
        if ndef[p.0 as usize] == 1 {
            root[p.0 as usize] = (1u64 << 40) | p.0 as u64;
        }
    }
    for p in &f.params {
        if ndef[p.0 as usize] == 1 && root[p.0 as usize] == 0 {
            root[p.0 as usize] = LOADED;
        }
    }
    // defs in order of appearance in reverse postorder approximated by block order
    for _ in 0..2 {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::SlotAddr(d, sl) if single(*d) => root[d.0 as usize] = 1 + *sl as u64,
                    Inst::Mov(d, x) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, x, _) if single(*d) => root[d.0 as usize] = root[x.0 as usize],
                    Inst::IBin(IOp::Add, _, d, x, y) if single(*d) => {
                        let (rx, ry) = (root[x.0 as usize], root[y.0 as usize]);
                        root[d.0 as usize] = if rx != 0 && ry == 0 { rx } else if ry != 0 && rx == 0 { ry } else { 0 };
                    }
                    Inst::Load(Mem::Int(8, _), d, _, _) if single(*d) => root[d.0 as usize] = LOADED,
                    Inst::Call(_, _, rets) => {
                        for r in rets {
                            if single(*r) {
                                root[r.0 as usize] = LOADED;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut captured: Vec<u32> = Vec::new();
    let mut taint: Vec<u32> = vec![u32::MAX; nv];
    if !f.noalias.is_empty() {
        for p in &f.noalias {
            taint[p.0 as usize] = p.0;
        }
        let mut more = true;
        let mut rounds = 0;
        while more && rounds < 8 {
            more = false;
            rounds += 1;
            for b in &f.blocks {
                for i in &b.insts {
                    let (d, srcs): (VReg, [VReg; 2]) = match i {
                        Inst::Mov(d, x) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, x, _) | Inst::Conv(_, d, x) => (*d, [*x, *x]),
                        Inst::IBin(IOp::Add | IOp::Sub, _, d, x, y) => (*d, [*x, *y]),
                        Inst::Sel(d, _, x, y) => (*d, [*x, *y]),
                        _ => continue,
                    };
                    for x in srcs {
                        let t = taint[x.0 as usize];
                        if t != u32::MAX && taint[d.0 as usize] == u32::MAX {
                            taint[d.0 as usize] = t;
                            more = true;
                        }
                    }
                }
            }
        }
        if more {
            // did not settle: treat every noalias param as captured
            captured.extend(f.noalias.iter().map(|p| p.0));
        }
        for b in &f.blocks {
            for i in &b.insts {
                let v = match i {
                    Inst::Store(_, _, _, v) | Inst::StoreX(_, _, _, _, _, v) | Inst::AtomicStore(_, _, _, v) | Inst::AtomicRmw(_, _, _, _, _, v) => *v,
                    Inst::AtomicCas(_, _, _, _, _, _, n) => *n,
                    _ => continue,
                };
                if taint[v.0 as usize] != u32::MAX {
                    captured.push(taint[v.0 as usize]);
                }
            }
            if let Term::Ret(vs) = &b.term {
                for v in vs {
                    if taint[v.0 as usize] != u32::MAX {
                        captured.push(taint[v.0 as usize]);
                    }
                }
            }
        }
    }
    (root, captured, taint)
}

/// Expressions local_cse keeps available along a dominator path (oldest forgotten first).
const CSE_AVAIL: usize = 48;

pub fn local_cse(options: &relax_session::Options, f: &mut Func) -> bool {
    if !cse_can_change(f) {
        return false;
    }
    // MAKEPAD_RUN_OPT=-cseevict: a full table takes nothing new (bisection)
    let keep_full = crate::stats::opt_off(options, "-cseevict");
    let n = f.blocks.len();
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    // store forwarding / load invalidation needs pointer roots only when both occur
    let (mut has_load, mut has_store) = (false, false);
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Load(..) => has_load = true,
                Inst::Store(..) => has_store = true,
                _ => {}
            }
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    let idom = idoms(f);
    let mut npred = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        for t in b.term.succs() {
            npred[t as usize] += 1;
        }
    }
    let (pre, post, children) = dom_numbers(&idom);
    // cleanup pads (unwind targets) and what they dominate run only while unwinding: no CSE
    // there (cold; pads are a tenth of the code with unwinding on)
    let mut pad = vec![false; n];
    for b in &f.blocks {
        if let Term::Invoke(_, u) = b.term {
            pad[u as usize] = true;
        }
    }
    let single = |v: VReg| ndef[v.0 as usize] == 1;
    // pointer roots (see noalias_roots); only store forwarding / load invalidation needs them
    const LOADED: u64 = LOADED_ROOT;
    let (root, captured, _) = if has_load && has_store {
        noalias_roots(f, &ndef)
    } else {
        (vec![0u64; nv], Vec::new(), Vec::new())
    };
    let may_alias = |a: VReg, b: VReg| -> bool {
        if a == b {
            return true;
        }
        let (ra, rb) = (root[a.0 as usize], root[b.0 as usize]);
        // an uncaptured noalias param's memory vs a pointer not derived from it
        let na_vs_loaded = |x: u64, y: u64| x >> 40 == 1 && y == LOADED && !captured.contains(&((x & 0xffff_ffff) as u32));
        if (na_vs_loaded(ra, rb) || na_vs_loaded(rb, ra)) && !no_noalias(options) {
            return false;
        }
        // otherwise LOADED is just unknown
        let (ra, rb) = (if ra == LOADED { 0 } else { ra }, if rb == LOADED { 0 } else { rb });
        !(ra != 0 && rb != 0 && ra != rb)
    };
    // definition blocks per multi-def vreg; unsigned `w = v - c` definitions per w
    // (multi-def vregs only, by midx[v] - 1)
    let mut midx = vec![0u32; nv];
    let mut def_blocks: Vec<Vec<u32>> = Vec::new();
    let mut def_sites: Vec<Vec<(u32, u32)>> = Vec::new();
    let mut sub_defs: Vec<(VReg, u32, VReg, i64)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            if let Inst::IBinI(IOp::Sub, it, wd, x, c) = i {
                if !it.signed && *c > 0 {
                    sub_defs.push((*wd, bi as u32, *x, *c));
                }
            }
            uses_defs(i, &mut uses, &mut defs);
            for d in &defs {
                if ndef[d.0 as usize] > 1 {
                    if midx[d.0 as usize] == 0 {
                        def_blocks.push(Vec::new());
                        def_sites.push(Vec::new());
                        midx[d.0 as usize] = def_blocks.len() as u32;
                    }
                    let m = midx[d.0 as usize] as usize - 1;
                    if def_blocks[m].last() != Some(&(bi as u32)) {
                        def_blocks[m].push(bi as u32);
                    }
                    def_sites[m].push((bi as u32, k as u32));
                }
            }
        }
    }
    let dominates = |a: u32, b: u32| -> bool {
        let (a, b) = (a as usize, b as usize);
        pre[a] != u32::MAX && pre[b] != u32::MAX && pre[a] <= pre[b] && post[b] <= post[a]
    };
    // loop headers: blocks with a predecessor they dominate
    let mut is_header = vec![false; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        for t in b.term.succs() {
            if dominates(t, bi as u32) {
                is_header[t as usize] = true;
            }
        }
    }
    // Loop header `h` entered from `d`: is `v` only ever decreased inside the loop (each
    // in-loop definition is `v = w` with `w = v - c`, c > 0, guarded by `v > c-1`)? Then an
    // upper bound known when entering the loop holds at the header on every iteration.
    let blocks_ptr: *const Vec<Block> = &f.blocks;
    let non_increasing = |v: VReg, d: u32, h: u32| -> bool {
        let blocks = unsafe { &*blocks_ptr };
        if midx[v.0 as usize] == 0 {
            return true;
        }
        for &(db, dk) in &def_sites[midx[v.0 as usize] as usize - 1] {
            if dominates(db, d) {
                continue;
            }
            if !dominates(h, db) {
                return false;
            }
            let w = match blocks[db as usize].insts[dk as usize] {
                Inst::Mov(_, w) => w,
                _ => return false,
            };
            // w's definition `w = v - c` with a guard on v dominating it
            let mut ok = false;
            if let Some(&(_, bi2, _, c)) = sub_defs.iter().find(|s| s.0 == w && s.2 == v) {
                // a block on the dominator chain of bi2 entered by the true edge of
                // `v > k` (k >= c-1) or `v != 0` (c == 1)
                let mut xb = bi2;
                let mut guard = 0;
                while xb != h && guard < 1000 {
                    let p = idom[xb as usize];
                    if p == u32::MAX || p == xb {
                        break;
                    }
                    if npred[xb as usize] == 1 {
                        if let Term::Branch(cv, tt, _) = blocks[p as usize].term {
                            if tt == xb {
                                for i3 in &blocks[p as usize].insts {
                                    match i3 {
                                        Inst::ICmpI(Cond::Gt, false, cd, cx, k) if *cd == cv && *cx == v && *k >= c - 1 => ok = true,
                                        Inst::ICmpI(Cond::Ne, _, cd, cx, 0) if *cd == cv && *cx == v && c == 1 => ok = true,
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    xb = p;
                    guard += 1;
                }
            }
            if !ok {
                return false;
            }
        }
        true
    };
    let mut fx = Facts { fact: vec![None; nv], ub: vec![None; nv], lb: vec![None; nv], undo: Vec::new(), multi: Vec::new(), multi_undo: Vec::new() };
    let mut changed = false;
    let mut work: Vec<CseWork> = vec![CseWork::Enter(0, CseState::default(), None, u32::MAX)];
    let mut ops: Vec<VReg> = Vec::new();
    // vregs ever entered into an avail or copies list (on any path): a def of any other
    // vreg has nothing to invalidate there
    let mut mentioned = vec![false; nv];
    while let Some(wk) = work.pop() {
        let (bi, mut st, edge, parent) = match wk {
            CseWork::Leave(mark) => {
                fx.rewind(mark);
                continue;
            }
            CseWork::Enter(b, s, e, p) => (b as usize, s, e, p),
        };
        work.push(CseWork::Leave(fx.undo.len()));
        match edge {
            Some(None) => {}
            Some(Some((c, val, cmp))) => {
                // the branch fact of the edge, and range facts of an unsigned compare
                fx.set(0, c, Some(val), !single(c));
                if let Some((cc, x, k)) = cmp {
                    let m = !single(x);
                    if (cc == Cond::Eq && val == 1) || (cc == Cond::Ne && val == 0) {
                        fx.set(0, x, Some(k), m);
                    }
                    // x <u k on the true edge of `x < k`, the false edge of `x >= k`
                    if (cc == Cond::Lt && val == 1) || (cc == Cond::Ge && val == 0) {
                        fx.tighten_ub(x, k, m);
                    }
                    if k != -1 && ((cc == Cond::Le && val == 1) || (cc == Cond::Gt && val == 0)) {
                        fx.tighten_ub(x, k.wrapping_add(1), m);
                    }
                    // x >u k on the true edge of `x > k`, the false edge of `x <= k`
                    if (cc == Cond::Gt && val == 1) || (cc == Cond::Le && val == 0) {
                        fx.tighten_lb(x, k, m);
                    }
                    if k != 0 && ((cc == Cond::Ge && val == 1) || (cc == Cond::Lt && val == 0)) {
                        fx.tighten_lb(x, k.wrapping_sub(1), m);
                    }
                    if k == 0 && ((cc == Cond::Ne && val == 1) || (cc == Cond::Eq && val == 0)) {
                        fx.tighten_lb(x, 0, m);
                    }
                }
            }
            None if parent != u32::MAX => {
                // a join: keep what cannot change on the way from the dominator `parent`
                let header = is_header[bi];
                // blocks reachable from a definition block (memoised): a definition that `bi` dominates
                // still reaches `bi` again when both sit in a loop, so it lies on a path from `parent`
                let reaches: std::cell::RefCell<std::collections::HashMap<u32, Vec<bool>>> = Default::default();
                let reaches_bi = |db: u32| -> bool {
                    let mut memo = reaches.borrow_mut();
                    let seen = memo.entry(db).or_insert_with(|| {
                        let blocks = unsafe { &*blocks_ptr };
                        let mut seen = vec![false; n];
                        let mut stack: Vec<u32> = blocks[db as usize].term.succs().collect();
                        while let Some(x) = stack.pop() {
                            if !seen[x as usize] {
                                seen[x as usize] = true;
                                stack.extend(blocks[x as usize].term.succs());
                            }
                        }
                        seen
                    });
                    seen[bi]
                };
                let stable = |v: VReg| -> bool {
                    if v.0 == u32::MAX || single(v) {
                        return true;
                    }
                    if header {
                        return false;
                    }
                    let blocks = unsafe { &*blocks_ptr };
                    if midx[v.0 as usize] == 0 {
                        return true;
                    }
                    for &db in &def_blocks[midx[v.0 as usize] as usize - 1] {
                        // a latch that only jumps back to the dominator redefines v before
                        // the next pass through there: the last pass to the join sees no def
                        let latch_to_here = matches!(blocks[db as usize].term, Term::Jump(t) if t == parent);
                        if !(dominates(db, parent) || (dominates(bi as u32, db) && !reaches_bi(db)) || latch_to_here) {
                            return false;
                        }
                    }
                    true
                };
                st.avail.retain(|e| !e.3 && stable(e.1) && stable(e.2[0]) && stable(e.2[1]));
                st.copies.retain(|x| stable(x.0) && stable(x.1));
                let multi: Vec<u32> = fx.multi.clone();
                for v in multi {
                    let v = VReg(v);
                    if stable(v) {
                        continue;
                    }
                    fx.set(0, v, None, false);
                    fx.set(2, v, None, false);
                    if !(header && fx.ub[v.0 as usize].is_some() && non_increasing(v, parent, bi as u32)) {
                        fx.set(1, v, None, false);
                    }
                }
            }
            None => {}
        }
        let b = &mut f.blocks[bi];
        for k in 0..b.insts.len() {
            uses_defs(&b.insts[k], &mut uses, &mut defs);
            if !st.copies.is_empty() {
                let hit_copy = uses.iter().any(|u| st.copies.iter().any(|(d, _)| d == u));
                if hit_copy {
                    let cp = &st.copies;
                    let look = |v: VReg| -> VReg {
                        for (d, s2) in cp.iter() {
                            if *d == v {
                                return *s2;
                            }
                        }
                        v
                    };
                    let n2 = rename(&b.insts[k], &look);
                    if n2 != b.insts[k] {
                        b.insts[k] = n2;
                        changed = true;
                        uses_defs(&b.insts[k], &mut uses, &mut defs);
                    }
                }
            }
            // A checked slice length is constant on the successful equality edge.
            // Use the path fact here: global folding cannot assign one constant to
            // the length vreg on both the success and the panic paths.
            if let Inst::Call(Callee::Host(a), args, rets) = &b.insts[k] {
                if rets.is_empty() && args.len() == 3 && Some(*a) == mem_fns().map(|m| m.0) {
                    if let Some(n) = fx.fact[args[2].0 as usize] {
                        if (0..=INLINE_COPY_MAX).contains(&n) {
                            b.insts[k] = Inst::Copy(args[0], args[1], n as u32);
                            changed = true;
                            uses_defs(&b.insts[k], &mut uses, &mut defs);
                        }
                    }
                }
            }
            // Both edges of a guard constrain the unsigned range. In particular,
            // the success edge of `len >= n` also proves a later `len < n` false.
            if let Inst::ICmpI(cc, false, d, y, k2) = b.insts[k] {
                let lo = fx.lb[y.0 as usize].and_then(|b| (b as u64).checked_add(1)).unwrap_or(0);
                let hi = fx.ub[y.0 as usize].and_then(|b| (b as u64).checked_sub(1)).unwrap_or(u64::MAX);
                let k2 = k2 as u64;
                let result = match cc {
                    Cond::Lt if hi < k2 => Some(true),
                    Cond::Lt if lo >= k2 => Some(false),
                    Cond::Le if hi <= k2 => Some(true),
                    Cond::Le if lo > k2 => Some(false),
                    Cond::Gt if lo > k2 => Some(true),
                    Cond::Gt if hi <= k2 => Some(false),
                    Cond::Ge if lo >= k2 => Some(true),
                    Cond::Ge if hi < k2 => Some(false),
                    Cond::Eq | Cond::Ne if k2 < lo || k2 > hi => Some(cc == Cond::Ne),
                    Cond::Eq | Cond::Ne if lo == hi && lo == k2 => Some(cc == Cond::Eq),
                    _ => None,
                };
                if let Some(result) = result {
                    b.insts[k] = Inst::Iconst(d, result as i64);
                    changed = true;
                    uses_defs(&b.insts[k], &mut uses, &mut defs);
                }
            }
            let is_load = matches!(b.insts[k], Inst::Load(..) | Inst::VLoad(..));
            ops.clear();
            ops.extend_from_slice(&uses);
            let cand = match &b.insts[k] {
                Inst::Mov(..) | Inst::SlotAddr(..) => None,
                i if is_mem_effect(i) => None,
                _ => {
                    if defs.len() == 1 && !uses.contains(&defs[0]) {
                        Some(defs[0])
                    } else {
                        None
                    }
                }
            };
            let key = match cand {
                Some(_) => cse_key(&b.insts[k]),
                None => None,
            };
            let mut replaced = false;
            if let (Some(d), Some(key)) = (cand, key) {
                // the same key with another result class is another value: a VExtract of one
                // lane as an integer (sign-extended) and as an f32
                if let Some(e) = st.avail.iter().find(|x| x.0 == key && f.vregs[x.1 .0 as usize] == f.vregs[d.0 as usize]).map(|x| x.1) {
                    if e != d {
                        b.insts[k] = match fx.fact[e.0 as usize] {
                            Some(x) => Inst::Iconst(d, x),
                            None => Inst::Mov(d, e),
                        };
                        changed = true;
                        replaced = true;
                    }
                }
            }
            match &b.insts[k] {
                Inst::Call(..) | Inst::Copy(..) | Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) | Inst::FpEnv(..) | Inst::Cpuid(..) | Inst::Xgetbv(..) | Inst::VStore(..) => st.avail.retain(|x| !x.3),
                Inst::Store(m, base, off, v) => {
                    let (base, off, v, m) = (*base, *off as i64, *v, *m);
                    let sz = mem_size(m) as i64;
                    st.avail.retain(|x| {
                        if !x.3 {
                            return true;
                        }
                        let lb = x.2[0];
                        if !may_alias(lb, base) {
                            return true;
                        }
                        if lb == base {
                            // same base: disjoint byte ranges survive
                            let lo = x.0[2] as i64;
                            let lsz = load_key_size(x.0[0]);
                            return lo + lsz <= off || off + sz <= lo;
                        }
                        false
                    });
                    // forward the stored value to later loads of the same cell (a full table
                    // forgets its oldest entry: the nearest expressions are the likely repeats)
                    if st.avail.len() < CSE_AVAIL || !keep_full {
                        if st.avail.len() >= CSE_AVAIL {
                            st.avail.remove(0);
                        }
                        let key = cse_key(&Inst::Load(m, VReg(0), base, off as i32)).unwrap();
                        st.avail.push((key, v, [base, v], true));
                        mentioned[v.0 as usize] = true;
                        mentioned[base.0 as usize] = true;
                    }
                }
                _ => {}
            }
            // (a replacement keeps the destination: `defs` still holds)
            for d in &defs {
                let d = *d;
                fx.clear(d);
                if !mentioned[d.0 as usize] {
                    continue;
                }
                if st.avail.iter().any(|(_, e, ops, _)| *e == d || ops[0] == d || ops[1] == d) {
                    st.avail.retain(|(_, e, ops, _)| *e != d && ops[0] != d && ops[1] != d);
                }
                if st.copies.iter().any(|(x, y)| *x == d || *y == d) {
                    st.copies.retain(|(x, y)| *x != d && *y != d);
                }
            }
            if let Inst::IBinI(IOp::Sub, it, d, x, c) = b.insts[k] {
                if !it.signed && c > 0 && fx.lb[x.0 as usize].is_some_and(|lb| lb >= c - 1) {
                    if let Some(bnd) = fx.ub[x.0 as usize] {
                        fx.tighten_ub(d, bnd, !single(d));
                    }
                }
            }
            if let Inst::Mov(d, s2) = b.insts[k] {
                if d != s2 {
                    if st.copies.len() < 32 {
                        st.copies.push((d, s2));
                        mentioned[d.0 as usize] = true;
                        mentioned[s2.0 as usize] = true;
                    }
                    // a copy inherits the source's bound (snapshot of the same value)
                    if let Some(bound) = fx.ub[s2.0 as usize] {
                        fx.tighten_ub(d, bound, !single(d));
                    }
                    if let Some(bound) = fx.lb[s2.0 as usize] {
                        fx.tighten_lb(d, bound, !single(d));
                    }
                }
            }
            if let (Some(d), false, Some(key)) = (cand, replaced, key) {
                if st.avail.len() < CSE_AVAIL || !keep_full {
                    if st.avail.len() >= CSE_AVAIL {
                        st.avail.remove(0);
                    }
                    let o0 = if !ops.is_empty() { ops[0] } else { VReg(u32::MAX) };
                    let o1 = if ops.len() > 1 { ops[1] } else { VReg(u32::MAX) };
                    st.avail.push((key, d, [o0, o1], is_load));
                    mentioned[d.0 as usize] = true;
                    for o in [o0, o1] {
                        if o.0 != u32::MAX {
                            mentioned[o.0 as usize] = true;
                        }
                    }
                }
            }
        }
        if !st.copies.is_empty() {
            let cp = &st.copies;
            let look = |v: VReg| -> VReg {
                for (d, s2) in cp.iter() {
                    if *d == v {
                        return *s2;
                    }
                }
                v
            };
            b.term = match &b.term {
                Term::Branch(c, x, y) => Term::Branch(look(*c), *x, *y),
                Term::Ret(v) => Term::Ret(v.iter().map(|x| look(*x)).collect()),
                t => t.clone(),
            };
        }
        if let Term::Branch(c, t, e) = b.term {
            if let Some(x) = fx.fact[c.0 as usize] {
                b.term = Term::Jump(if x != 0 { t } else { e });
                changed = true;
            }
        }
        // the compare behind the branch condition (unsigned compares give range facts)
        let mut cmp_of_cond: Option<(Cond, VReg, i64)> = None;
        if let Term::Branch(c, _, _) = b.term {
            for i in &b.insts {
                // RIR allows reassignment: the edge describes the operands at
                // the compare, not a later definition of either operand.
                for_defs(i, |d| {
                    if d == c || cmp_of_cond.map_or(false, |(_, x, _)| d == x) {
                        cmp_of_cond = None;
                    }
                });
                if let Inst::ICmpI(cc, false, d, x, kk) = i {
                    if *d == c && *x != c {
                        cmp_of_cond = Some((*cc, *x, *kk));
                    }
                }
            }
        }
        let term = b.term.clone();
        // children in the dominator tree (pushed in reverse: visited in order); the last
        // pushed takes the state by move
        let nch = children[bi].len();
        let mut st_opt = Some(st);
        for ci in (0..nch).rev() {
            let ch = children[bi][ci];
            if pad[ch as usize] {
                continue;
            }
            let cs = if ci == 0 { st_opt.take().unwrap() } else { st_opt.as_ref().unwrap().clone() };
            let edge = if npred[ch as usize] == 1 {
                match term {
                    Term::Branch(c, t, e) if t != e && (ch == t || ch == e) => Some(Some((c, if ch == t { 1 } else { 0 }, cmp_of_cond))),
                    _ => Some(None),
                }
            } else {
                None
            };
            work.push(CseWork::Enter(ch, cs, edge, bi as u32));
        }
    }
    changed
}

/// Compact CSE key of a pure single-result instruction (dest excluded).
/// Byte size of the load encoded in a `cse_key` tag word.
pub fn load_key_size(k0: u64) -> i64 {
    let mk = k0 >> 8;
    if mk & (1 << 16) != 0 {
        4
    } else if mk & (2 << 16) != 0 {
        8
    } else if mk & (4 << 16) != 0 {
        32
    } else {
        (mk & 0xff) as i64
    }
}

/// Value numbering of pure values over the dominator tree, without local_cse's bounded table
/// (48 values per path, sized for Rust functions): an instruction with a `cse_key` that is no load
/// and no constant, whose result and operands each have one definition, becomes a copy of the
/// result of an instruction with the same key and result class above it in the dominator tree
/// (simplify's copy propagation then removes the copy). Constants stay: they are rematerialised
/// and sunk to their uses. For generated code with long runs of repeated pure values: a Splash
/// kernel's lane extracts, which LICM gathers in the element loop's preheader (Stage's largest x4
/// kernel extracted each lane of one product 216 times, all live through its 14k blocks).
/// Vector ops (VX, VX3, VFma, VSel, VShuffle) are numbered too, their constant operands by value
/// (each use has its own constant: a table's byte indices `h * splat(0x04040404) + splat(..)` of
/// two lookups were made twice). Returns whether anything changed.
pub fn gvn_pure(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    // vector ops are numbered with their constant operands by value: each use has its own
    // (rematerialised) constant, so `x * splat(k)` made twice reads two equal constants
    let mut konst: Vec<Option<u64>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            match *i {
                Inst::VConst(d, k) if ndef[d.0 as usize] == 1 => konst[d.0 as usize] = Some(1 << 62 | k as u64),
                Inst::Iconst(d, k) if ndef[d.0 as usize] == 1 => konst[d.0 as usize] = Some(k as u64),
                _ => {}
            }
        }
    }
    let idom = idoms(f);
    let (_, _, kids) = dom_numbers(&idom);
    let (blocks, vregs) = (&mut f.blocks, &f.vregs);
    let mut table: std::collections::HashMap<(GvnKey, Cls), VReg> = std::collections::HashMap::new();
    let mut undo: Vec<(GvnKey, Cls)> = Vec::new();
    // a numbered value's equal: its uses (all below it in the dominator tree) read the leader, so
    // the values made from it number equal too (`h * k` then `h * k + c`, twice)
    let mut leader: Vec<u32> = (0..nv as u32).collect();
    let mut marks: Vec<usize> = Vec::new();
    let (mut uses, mut defs) = (Vec::new(), Vec::new());
    let mut changed = false;
    // (block, entering): a block's values are dropped when it is left, after its subtree
    let mut work: Vec<(u32, bool)> = vec![(0, true)];
    while let Some((bi, enter)) = work.pop() {
        if !enter {
            let m = marks.pop().unwrap_or(0);
            for k in undo.drain(m..) {
                table.remove(&k);
            }
            continue;
        }
        marks.push(undo.len());
        work.push((bi, false));
        let b = &mut blocks[bi as usize];
        for k in 0..b.insts.len() {
            let mut stale = false;
            for_uses(&b.insts[k], |u| stale |= leader[u.0 as usize] != u.0);
            if stale {
                b.insts[k] = rename(&b.insts[k], &|r| VReg(leader[r.0 as usize]));
                changed = true;
            }
            let i = &b.insts[k];
            if matches!(i, Inst::Load(..) | Inst::VLoad(..) | Inst::Iconst(..) | Inst::Fconst(..) | Inst::VConst(..) | Inst::Addr(..) | Inst::FnAddr(..)) {
                continue;
            }
            // an operand: its constant (Iconst / VConst operands differ by value: tagged) or its vreg
            let o = |r: VReg| match konst[r.0 as usize] {
                Some(k) if vregs[r.0 as usize] == Cls::V128 => (1u64 << 63) | k,
                _ => r.0 as u64,
            };
            let key = match *i {
                Inst::VX(op, _, a, b) => GvnKey::X(op, o(a), o(b), 0),
                Inst::VX3(op, _, a, b, c) => GvnKey::X(op, o(a), o(b), o(c)),
                Inst::VFma(np, na, _, a, b, c) => GvnKey::Fma(np, na, o(a), o(b), o(c)),
                Inst::VSel(_, m, a, b) => GvnKey::Sel(o(m), o(a), o(b)),
                Inst::VShuffle(_, a, b, k) => GvnKey::Shuf(o(a), o(b), k),
                _ => match cse_key(i) {
                    Some(k) => GvnKey::K(k),
                    None => continue,
                },
            };
            uses_defs(i, &mut uses, &mut defs);
            if defs.len() != 1 {
                continue;
            }
            let d = defs[0];
            if ndef[d.0 as usize] != 1 || uses.iter().any(|u| *u == d || ndef[u.0 as usize] != 1) {
                continue;
            }
            let cls = vregs[d.0 as usize];
            match table.get(&(key, cls)) {
                Some(&e) => {
                    b.insts[k] = Inst::Mov(d, e);
                    leader[d.0 as usize] = e.0;
                    changed = true;
                }
                None => {
                    table.insert((key, cls), d);
                    undo.push((key, cls));
                }
            }
        }
        for &c in kids[bi as usize].iter().rev() {
            work.push((c, true));
        }
    }
    changed
}

/// gvn_pure's value key: cse_key's, or a vector op with its operands (constants by value)
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum GvnKey {
    K([u64; 3]),
    X(VX, u64, u64, u64),
    Fma(bool, bool, u64, u64, u64),
    Sel(u64, u64, u64),
    Shuf(u64, u64, [u8; 16]),
}

pub fn cse_key(i: &Inst) -> Option<[u64; 3]> {
    fn it(t: &IntTy) -> u64 {
        t.bits as u64 | ((t.signed as u64) << 8)
    }
    fn cond(c: &Cond) -> u64 {
        *c as u64
    }
    let v = |r: &VReg| r.0 as u64;
    Some(match i {
        Inst::Iconst(_, x) => [1, *x as u64, 0],
        Inst::Fconst(_, x, w) => [2 | ((*w as u64) << 8), *x, 0],
        Inst::IBin(o, t, _, a, b) => [3 | ((*o as u64) << 8) | (it(t) << 16), v(a), v(b)],
        Inst::IBinI(o, t, _, a, k) => [4 | ((*o as u64) << 8) | (it(t) << 16), v(a), *k as u64],
        Inst::INeg(t, _, a) => [5 | (it(t) << 16), v(a), 0],
        Inst::INot(t, _, a) => [6 | (it(t) << 16), v(a), 0],
        Inst::IBit(o, _, a) => [19 | ((*o as u64) << 8), v(a), 0],
        Inst::ICmp(c, s2, _, a, b) => [7 | (cond(c) << 8) | ((*s2 as u64) << 16), v(a), v(b)],
        Inst::ICmpI(c, s2, _, a, k) => [8 | (cond(c) << 8) | ((*s2 as u64) << 16), v(a), *k as u64],
        Inst::FBin(o, w, _, a, b) => [9 | ((*o as u64) << 8) | ((*w as u64) << 16), v(a), v(b)],
        Inst::FUnary(o, w, _, a) => [10 | ((*o as u64) << 8) | ((*w as u64) << 16), v(a), 0],
        Inst::FCmp(c, w, _, a, b) => [11 | (cond(c) << 8) | ((*w as u64) << 16), v(a), v(b)],
        Inst::Conv(c, _, a) => {
            let ck = match c {
                Conv::IntToInt(t) => 1 | (it(t) << 8),
                Conv::IntToF(t, w) => 2 | (it(t) << 8) | ((*w as u64) << 24),
                Conv::FToInt(w, t) => 3 | (it(t) << 8) | ((*w as u64) << 24),
                Conv::F32ToF64 => 4,
                Conv::F64ToF32 => 5,
                Conv::BitsToF(w) => 6 | ((*w as u64) << 8),
                Conv::FToBits(w) => 7 | ((*w as u64) << 8),
            };
            [12 | (ck << 8), v(a), 0]
        }
        Inst::Load(m, _, b, o) => {
            let mk = match m {
                Mem::Int(n, s2) => *n as u64 | ((*s2 as u64) << 8),
                Mem::F32 => 1 << 16,
                Mem::F64 => 2 << 16,
            };
            [13 | (mk << 8), v(b), *o as i64 as u64]
        }
        // a vector load (16 or 32 bytes: load_key_size takes 32); the result class tells V128 and
        // V256 apart where local_cse matches keys
        Inst::VLoad(_, b, o) => [13 | ((4 << 16) << 8), v(b), *o as i64 as u64],
        Inst::Addr(_, x) => [14, x.id as u64, x.off as u64],
        Inst::FnAddr(_, x) => [15, *x as u64, 0],
        Inst::VExtract(_, a, l) => [16 | ((*l as u64) << 8), v(a), 0],
        Inst::VConst(_, x) => [17, *x as u64, 0],
        Inst::VSplat(_, a) => [18, v(a), 0],
        _ => return None,
    })
}

/// Scalar replacement of stack slots. A slot qualifies when every use of its address -- the
/// SlotAddr vreg or an address derived from it by adding a constant (a field, an enum
/// payload) -- is the base of a load or store, an operand of a `Copy`, or another such
/// derivation. Its accessed cells (offset, kind) become vregs. Slots joined by copies form a
/// group with one cell layout in the group's coordinates (a copy of slot B at offset 4 into
/// slot A at offset 1 puts B's offset x at A's x - 3), so the chains of small aggregates
/// lowering makes (an Option built in one slot, copied to a temporary, its payload copied
/// into a field of the result) become register moves. A copy between a group and other
/// memory (a call's result slot, an out-pointer, a slice's bytes) becomes per-cell loads or
/// stores. Overlapping accesses become one unsigned integer cell only when every access
/// has the same offset and size, with different kinds handled by bit reinterpretation
/// or integer extension. Overlaps with different offsets or sizes stay in memory.
/// Groups with both incoming and outgoing memory copies tile unnamed bytes of each copied
/// range (at most 64 bytes) with integer cells, preserving padding through the copies.
/// Copies must not cut cells; these groups get a 16-cell copy budget instead of eight.
/// Most cells a group joined by copies may have (scalar pairs, Option/Result of a few fields).
pub const SROA_COPY_CELLS: usize = 8;

pub const SROA_COPY_IN_OUT_CELLS: usize = 16;

/// A 128-bit vector cell (VLoad/VStore) in sroa's cell table (no integer access is 16 bytes)
pub const SROA_V128: Mem = Mem::Int(16, false);

#[derive(Clone, Copy)]
pub struct SroaCell {
    pub off: i64,
    pub mem: Mem,
    // Different kinds at the same offset and size share unsigned integer bits.
    pub widened: bool,
    // A 16-byte vector cell also accessed as its 32-bit lanes (f32 / 4-byte integer loads and
    // stores at lane offsets): those become VExtract / VInsert of the vector (LLVM SROA's vector
    // promotion; core::arch `_mm_setr_ps` / lane setters build a vector through memory).
    pub lanes: bool,
}

#[derive(Clone, Copy)]
pub enum SroaReject {
    Escape,
    Overlap,
    KindMix,
    CopyCut,
    CopyCells,
    CopyInOut,
    Other,
}

#[derive(Default)]
pub struct SroaStats {
    // Early rejects are singleton candidates; copies coalesce the remaining candidates.
    pub candidates: usize,
    pub promoted: usize,
    pub rejects: [usize; 7],
}

pub fn sroa_reject_slot(ok: &mut [bool], sl: usize, why: SroaReject, stats: &mut Option<&mut SroaStats>) {
    if ok[sl] {
        ok[sl] = false;
        if let Some(s) = stats {
            s.rejects[why as usize] += 1;
        }
    }
}

relax_session::gen_local! {
    /// sroa's answer to "does this call keep or return a pointer passed as argument k?":
    /// set by the O3 driver from the callees' RIR (param_nocapture), None elsewhere
    static SROA_NOCAP: std::cell::RefCell<Option<Box<dyn Fn(&Callee, usize) -> bool>>>;
}

pub fn sroa_nocapture(c: &Callee, k: usize) -> bool {
    SROA_NOCAP.try_with(|q| q.borrow().as_ref().map_or(false, |f| f(c, k))).unwrap_or(false)
}

pub fn sroa(f: &mut Func) -> bool {
    sroa_with(f, false)
}

pub fn sroa_with(f: &mut Func, tile: bool) -> bool {
    static STATS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let mut stats = STATS.get_or_init(|| std::env::var("MAKEPAD_RUN_SROA_STATS").map_or(false, |v| v == "1"))
        .then(|| SroaStats { candidates: f.slots.len(), ..SroaStats::default() });
    let changed = sroa_inner(f, stats.as_mut(), tile);
    if let Some(s) = stats {
        eprintln!("sroa {}: candidates={} promoted={} escape={} overlap={} kind-mix={} copy-cut={} copy-cells={} copy-in+out={} other={}",
            f.name, s.candidates, s.promoted, s.rejects[0], s.rejects[1], s.rejects[2], s.rejects[3], s.rejects[4], s.rejects[5], s.rejects[6]);
    }
    changed
}

pub fn sroa_inner(f: &mut Func, mut stats: Option<&mut SroaStats>, tile: bool) -> bool {
    let nv = f.vregs.len();
    let ns = f.slots.len();
    if ns == 0 {
        return false;
    }
    // the cells' zero definitions go at the top of the entry block: it must not be a loop head
    for b in &f.blocks {
        match b.term {
            Term::Jump(0) | Term::Branch(_, 0, _) | Term::Branch(_, _, 0) => {
                if let Some(s) = &mut stats {
                    s.rejects[SroaReject::Other as usize] = ns;
                }
                return false;
            }
            _ => {}
        }
    }
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    // vreg -> (slot, offset) of the address it holds (SlotAddr / SlotAddr + k; a vreg defined
    // several times only by SlotAddr of one slot -- copies made by jump threading -- counts)
    const NO: (u32, i64) = (u32::MAX, 0);
    let mut addr: Vec<(u32, i64)> = vec![NO; nv];
    let mut slot_defs: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv]; // (slot, SlotAddr def count)
    // one scan: the instructions the phases below look at (`rel`, in block order) and the
    // operands of all the others and of the terminators (an address used there escapes)
    let mut rel: Vec<(u32, u32)> = Vec::new();
    let mut other_uses: Vec<VReg> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            match i {
                Inst::SlotAddr(d, sl) => {
                    let e = &mut slot_defs[d.0 as usize];
                    if e.0 == u32::MAX || e.0 == *sl {
                        *e = (*sl, e.1 + 1);
                    } else {
                        *e = (u32::MAX - 1, e.1 + 1);
                    }
                    rel.push((bi as u32, k as u32));
                }
                Inst::IBinI(IOp::Add, ..) | Inst::Mov(..) | Inst::Load(..) | Inst::VLoad(..) | Inst::Store(..) | Inst::VStore(..) | Inst::Copy(..) | Inst::Call(..) => rel.push((bi as u32, k as u32)),
                _ => for_uses(i, |u| other_uses.push(u)),
            }
        }
        term_uses(&b.term, &mut uses);
        other_uses.extend_from_slice(&uses);
    }
    for _ in 0..3 {
        let mut more = false;
        for &(bi, k) in &rel {
            {
                let i = &f.blocks[bi as usize].insts[k as usize];
                match i {
                    Inst::SlotAddr(d, sl) if slot_defs[d.0 as usize] == (*sl, ndef[d.0 as usize]) && addr[d.0 as usize].0 == u32::MAX => {
                        addr[d.0 as usize] = (*sl, 0);
                        more = true;
                    }
                    Inst::IBinI(IOp::Add, it, d, a, k) if it.bits == 64 && ndef[d.0 as usize] == 1 && addr[a.0 as usize].0 != u32::MAX && addr[d.0 as usize].0 == u32::MAX => {
                        let (sl, o) = addr[a.0 as usize];
                        addr[d.0 as usize] = (sl, o + *k);
                        more = true;
                    }
                    Inst::Mov(d, a) if f.vregs[d.0 as usize] == Cls::I && ndef[d.0 as usize] == 1 && addr[a.0 as usize].0 != u32::MAX && addr[d.0 as usize].0 == u32::MAX => {
                        addr[d.0 as usize] = addr[a.0 as usize];
                        more = true;
                    }
                    _ => {}
                }
            }
        }
        if !more {
            break;
        }
    }
    let at = |v: VReg| addr[v.0 as usize];
    // a slot qualifies when its addresses are only bases, copy operands or derivations
    let mut ok = vec![true; ns];
    // slots whose address is passed to calls that are their last uses (an iterator handed
    // to its drop after the loop): the cells live in registers and are written back to the
    // slot right before such a call. Nothing touches the slot after it on any path, so no
    // pointer the callee may keep can see stale cells.
    let mut esc = vec![false; ns];
    let mut touched = false;
    for &(bi, k) in &rel {
        {
            let i = &f.blocks[bi as usize].insts[k as usize];
            match i {
                Inst::Load(_, _, base, off) | Inst::VLoad(_, base, off) if at(*base).0 != u32::MAX => {
                    touched = true;
                    // an access outside the slot (a branch that is dead for this instance:
                    // the second word of a thin pointer) keeps the slot in memory
                    let n = if let Inst::Load(m, ..) = i { mem_size(*m) } else { 16 };
                    let (sl, o) = at(*base);
                    if o + (*off as i64) < 0 || o + *off as i64 + n as i64 > f.slots[sl as usize].size as i64 {
                        sroa_reject_slot(&mut ok, sl as usize, SroaReject::Other, &mut stats);
                    }
                }
                Inst::Store(_, base, off, v) | Inst::VStore(base, off, v) => {
                    if at(*base).0 != u32::MAX {
                        touched = true;
                        let n = if let Inst::Store(m, ..) = i { mem_size(*m) } else { 16 };
                        let (sl, o) = at(*base);
                        if o + (*off as i64) < 0 || o + *off as i64 + n as i64 > f.slots[sl as usize].size as i64 {
                            sroa_reject_slot(&mut ok, sl as usize, SroaReject::Other, &mut stats);
                        }
                    }
                    // the stored value must not be an address itself
                    if at(*v).0 != u32::MAX {
                        sroa_reject_slot(&mut ok, at(*v).0 as usize, SroaReject::Escape, &mut stats);
                    }
                }
                Inst::Copy(d, s2, n) => {
                    for v in [*d, *s2] {
                        let (sl, o) = at(v);
                        if sl != u32::MAX {
                            touched = true;
                            if o < 0 || o + *n as i64 > f.slots[sl as usize].size as i64 {
                                sroa_reject_slot(&mut ok, sl as usize, SroaReject::Other, &mut stats);
                            }
                        }
                    }
                }
                Inst::SlotAddr(..) => {}
                Inst::IBinI(IOp::Add, it, d, a, _) if it.bits == 64 && at(*a).0 != u32::MAX && at(*d).0 != u32::MAX => {}
                Inst::Mov(d, a) if at(*a).0 != u32::MAX && at(*d).0 != u32::MAX => {}
                Inst::Call(c, args, _) => {
                    for u in args {
                        if at(*u).0 != u32::MAX {
                            esc[at(*u).0 as usize] = true;
                        }
                    }
                    if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                        if at(*v).0 != u32::MAX {
                            sroa_reject_slot(&mut ok, at(*v).0 as usize, SroaReject::Escape, &mut stats);
                        }
                    }
                }
                _ => {
                    uses_defs(i, &mut uses, &mut defs);
                    for u in &uses {
                        if at(*u).0 != u32::MAX {
                            sroa_reject_slot(&mut ok, at(*u).0 as usize, SroaReject::Escape, &mut stats);
                        }
                    }
                }
            }
        }
    }
    for u in &other_uses {
        if at(*u).0 != u32::MAX {
            sroa_reject_slot(&mut ok, at(*u).0 as usize, SroaReject::Escape, &mut stats);
        }
    }
    if esc.iter().any(|x| *x) {
        for &(bi, k) in &rel {
            {
                let i = &f.blocks[bi as usize].insts[k as usize];
                if let Inst::Copy(d, s2, _) = i {
                    for v in [*d, *s2] {
                        if at(v).0 != u32::MAX && esc[at(v).0 as usize] {
                            sroa_reject_slot(&mut ok, at(v).0 as usize, SroaReject::Escape, &mut stats);
                        }
                    }
                }
            }
        }
    }
    if !touched || !ok.iter().any(|x| *x) {
        if let Some(s) = &mut stats {
            s.rejects[SroaReject::Other as usize] += ok.iter().filter(|x| **x).count();
        }
        return false;
    }
    let mem_key = |m: Mem| -> u32 {
        match m {
            Mem::Int(n, sg) => n as u32 * 2 + sg as u32,
            Mem::F32 => 100,
            Mem::F64 => 101,
        }
    };
    // groups: union-find with offsets (slot offset x = root offset x + delta[slot])
    let mut root = vec![0u32; ns];
    let mut delta = vec![0i64; ns];
    let mut gcells: Vec<Vec<SroaCell>>;
    let mut first_groups = true;
    let depth = crate::regalloc::loop_depths(f, &crate::regalloc::reachable(f));
    loop {
        fn find(p: &mut [u32], dl: &mut [i64], x: u32) -> (u32, i64) {
            // (root, delta of x relative to root)
            let mut path = Vec::new();
            let mut r = x;
            while p[r as usize] != r {
                path.push(r);
                r = p[r as usize];
            }
            // compress: deltas accumulate from x up to the root
            let mut acc = 0i64;
            for &y in path.iter().rev() {
                acc += dl[y as usize];
                dl[y as usize] = acc;
                p[y as usize] = r;
            }
            (r, if x == r { 0 } else { dl[x as usize] })
        }
        let mut par: Vec<u32> = (0..ns as u32).collect();
        let mut dl = vec![0i64; ns];
        let mut bad = vec![None; ns];
        for &(bi, k) in &rel {
            let i = &f.blocks[bi as usize].insts[k as usize]; {
                if let Inst::Copy(d, s2, _) = i {
                    let ((a, ao), (c, co)) = (at(*d), at(*s2));
                    if a != u32::MAX && c != u32::MAX && ok[a as usize] && ok[c as usize] {
                        // byte ao + x of a is byte co + x of c: pos_a(y) = pos_c(y - ao + co)
                        let (ra, da) = find(&mut par, &mut dl, a);
                        let (rc, dc) = find(&mut par, &mut dl, c);
                        // root coords: a's y -> y + da; c's z -> z + dc; need ao + da == co + dc
                        if ra == rc {
                            if ao + da != co + dc {
                                bad[ra as usize] = Some(SroaReject::Other);
                            }
                        } else {
                            // hang ra under rc: ra's coords shift by k so that ao + da + k == co + dc
                            par[ra as usize] = rc;
                            dl[ra as usize] = co + dc - ao - da;
                            if first_groups {
                                if let Some(s) = &mut stats {
                                    s.candidates -= 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        for sl in 0..ns {
            let (r, d) = find(&mut par, &mut dl, sl as u32);
            root[sl] = r;
            delta[sl] = d;
        }
        for sl in 0..ns {
            if let Some(why) = bad[sl] {
                bad[root[sl] as usize] = Some(why);
            }
        }
        first_groups = false;
        // per group root: cells in root coordinates, copies in from / out to memory
        let mut cells: Vec<(u32, i64, Mem)> = Vec::new();
        let mut has_in = vec![false; ns];
        let mut has_out = vec![false; ns];
        let mut access_depth = vec![0u32; ns];
        // (group, lo, hi, loop depth, internal: both ends in the group, so promoted it is register moves)
        let mut copies: Vec<(u32, i64, i64, u32, bool)> = Vec::new();
        let okat = |v: VReg| -> Option<(u32, i64)> {
            let (sl, o) = at(v);
            if sl != u32::MAX && ok[sl as usize] {
                Some((sl, o))
            } else {
                None
            }
        };
        for &(bi, k) in &rel {
            let i = &f.blocks[bi as usize].insts[k as usize]; {
                match i {
                    Inst::Load(m, _, base, off) | Inst::Store(m, base, off, _) => {
                        if let Some((sl, o)) = okat(*base) {
                            cells.push((root[sl as usize], o + *off as i64 + delta[sl as usize], *m));
                            access_depth[root[sl as usize] as usize] = access_depth[root[sl as usize] as usize].max(depth[bi as usize]);
                        }
                    }
                    Inst::VLoad(_, base, off) | Inst::VStore(base, off, _) => {
                        if let Some((sl, o)) = okat(*base) {
                            cells.push((root[sl as usize], o + *off as i64 + delta[sl as usize], SROA_V128));
                            access_depth[root[sl as usize] as usize] = access_depth[root[sl as usize] as usize].max(depth[bi as usize]);
                        }
                    }
                    Inst::Copy(d, s2, n) => {
                        let (ga, gc) = (okat(*d), okat(*s2));
                        let internal = matches!((ga, gc), (Some((x, _)), Some((y, _))) if root[x as usize] == root[y as usize]);
                        if let Some((sl, o)) = ga {
                            let lo = o + delta[sl as usize];
                            copies.push((root[sl as usize], lo, lo + *n as i64, depth[bi as usize], internal));
                            if gc.is_none() {
                                has_in[root[sl as usize] as usize] = true;
                            }
                        }
                        if let Some((sl, o)) = gc {
                            let lo = o + delta[sl as usize];
                            copies.push((root[sl as usize], lo, lo + *n as i64, depth[bi as usize], internal));
                            if ga.is_none() {
                                has_out[root[sl as usize] as usize] = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        cells.sort_unstable_by_key(|c| (c.0, c.1, mem_key(c.2)));
        cells.dedup_by(|b, a| a.0 == b.0 && a.1 == b.1 && mem_key(a.2) == mem_key(b.2));
        // Maximal overlapping components, including different kinds at the same offset.
        // Adjacent (but disjoint) cells stay separate.
        gcells = vec![Vec::new(); ns];
        let mut start = 0;
        while start < cells.len() {
            let (g, lo, m) = cells[start];
            let mut hi = lo + mem_size(m) as i64;
            let mut end = start + 1;
            let mut kind_mix = false;
            while end < cells.len() && cells[end].0 == g && cells[end].1 < hi {
                kind_mix |= cells[end].1 == cells[end - 1].1;
                hi = hi.max(cells[end].1 + mem_size(cells[end].2) as i64);
                end += 1;
            }
            let widened = end > start + 1;
            let size = hi - lo;
            // pure: every access is at lo with the cell's size (only the kind differs: bitcasts)
            let pure = cells[start..end].iter().all(|c| c.1 == lo && mem_size(c.2) as i64 == size);
            // lanes: one 16-byte vector access at lo, every other access a 32-bit lane of it
            let lanes = !pure
                && size == 16
                && cells[start..end].iter().any(|c| c.1 == lo && c.2 == SROA_V128)
                && cells[start..end].iter().all(|c| (c.1 == lo && c.2 == SROA_V128) || ((c.1 - lo) % 4 == 0 && matches!(c.2, Mem::F32 | Mem::Int(4, _))));
            if lanes {
                gcells[g as usize].push(SroaCell { off: lo, mem: SROA_V128, widened: false, lanes: true });
            } else if !pure {
                bad[g as usize].get_or_insert(if kind_mix { SroaReject::KindMix } else { SroaReject::Overlap });
            } else {
                gcells[g as usize].push(SroaCell { off: lo, mem: if widened { Mem::Int(size as u8, false) } else { m }, widened, lanes: false });
            }
            start = end;
        }
        copies.sort_unstable();
        copies.dedup();
        // A group copied both ways must preserve even bytes that no scalar access names.
        // Split gaps at every copy endpoint so tiling itself never introduces a copy cut.
        let mut ci = 0;
        while ci < copies.len() {
            let g = copies[ci].0 as usize;
            let mut ce = ci + 1;
            while ce < copies.len() && copies[ce].0 as usize == g {
                ce += 1;
            }
            if has_in[g] && has_out[g] && bad[g].is_none() && !tile {
                bad[g] = Some(SroaReject::CopyInOut);
            } else if has_in[g] && has_out[g] && bad[g].is_none() {
                if copies[ci..ce].iter().any(|c| c.2 - c.1 > 64) {
                    bad[g] = Some(SroaReject::CopyInOut);
                } else {
                    let mut bounds: Vec<i64> = copies[ci..ce].iter().flat_map(|c| [c.1, c.2]).collect();
                    bounds.sort_unstable();
                    bounds.dedup();
                    let mut gaps = Vec::new();
                    let mut k = 0;
                    let mut range = ci;
                    let mut covered_hi = i64::MIN;
                    for w in bounds.windows(2) {
                        let (lo, hi) = (w[0], w[1]);
                        while range < ce && copies[range].1 <= lo {
                            covered_hi = covered_hi.max(copies[range].2);
                            range += 1;
                        }
                        if hi > covered_hi {
                            continue;
                        }
                        let mut off = lo;
                        while off < hi {
                            while k < gcells[g].len() && gcells[g][k].off + mem_size(gcells[g][k].mem) as i64 <= off {
                                k += 1;
                            }
                            if k < gcells[g].len() && gcells[g][k].off <= off {
                                off = hi.min(gcells[g][k].off + mem_size(gcells[g][k].mem) as i64);
                                continue;
                            }
                            let end = gcells[g].get(k).map_or(hi, |c| hi.min(c.off));
                            while off < end {
                                let mut n = 8u8;
                                while off % n as i64 != 0 || off + n as i64 > end {
                                    n /= 2;
                                }
                                gaps.push(SroaCell { off, mem: Mem::Int(n, false), widened: false, lanes: false });
                                off += n as i64;
                            }
                        }
                    }
                    gcells[g].extend(gaps);
                    gcells[g].sort_unstable_by_key(|c| c.off);
                    // Each copy of a tiled group becomes one load or store per cell: narrow
                    // cells (a 16-byte group read bytewise) turn a two-word copy into 16 byte
                    // moves per copy (brotli ProcessCommandsInternal: +28% instructions). Keep
                    // the group in memory unless every copied range has at most two cells per
                    // 8-byte word (plus two for an unaligned tail).
                    let dense = copies[ci..ce].iter().all(|&(_, lo, hi, _, _)| {
                        let n = gcells[g].iter().filter(|c| c.off >= lo && c.off < hi).count() as i64;
                        n <= (hi - lo + 7) / 8 * 2 + 2
                    });
                    if !dense {
                        bad[g] = Some(SroaReject::CopyCells);
                    }
                }
            }
            ci = ce;
        }
        // A copy may not cut a cell in two. Keep large copies intact unless
        // promotion removes accesses in a deeper loop: initialization of a
        // compound iterator should not keep its state in memory per element.
        for &(g, lo, hi, copy_depth, internal) in &copies {
            let g = g as usize;
            let mut cnt = 0;
            for c in &gcells[g] {
                let end = c.off + mem_size(c.mem) as i64;
                if (c.off < lo && end > lo) || (c.off < hi && end > hi) {
                    bad[g].get_or_insert(SroaReject::CopyCut);
                }
                if c.off >= lo && end <= hi {
                    cnt += 1;
                }
            }
            let limit = if has_in[g] && has_out[g] { SROA_COPY_IN_OUT_CELLS } else { SROA_COPY_CELLS };
            // a copy inside the group becomes one register move per cell (coalesced away mostly):
            // only copies to or from memory cost a load / store per cell
            if !internal && cnt > limit && copy_depth >= access_depth[g] {
                bad[g].get_or_insert(SroaReject::CopyCells);
            }
        }
        if let Some(s) = &mut stats {
            for g in 0..ns {
                if ok[g] && root[g] as usize == g {
                    if let Some(why) = bad[g] {
                        s.rejects[why as usize] += 1;
                    }
                }
            }
        }
        let mut changed = false;
        for sl in 0..ns {
            if ok[sl] && bad[root[sl] as usize].is_some() {
                ok[sl] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // escaped slots: every escaping call must be a last use (no access of the slot after it,
    // on any path, the call's own block included)
    let mut esc_slots: Vec<u32> = Vec::new();
    // (block, instruction, slot): no-capture calls after which the slot's cells are reloaded
    let mut reload_sites: Vec<(u32, u32, u32)> = Vec::new();
    // The deepest loop a load / store of each escaped slot sits in.
    let acc_depth = if (0..ns).any(|sl| ok[sl] && esc[sl]) {
        let mut ad = vec![0u32; ns];
        for (bi, b) in f.blocks.iter().enumerate() {
            for i in &b.insts {
                if let Inst::Load(_, _, base, _) | Inst::Store(_, base, _, _) | Inst::VLoad(_, base, _) | Inst::VStore(base, _, _) = i {
                    let sl = at(*base).0;
                    if sl != u32::MAX {
                        ad[sl as usize] = ad[sl as usize].max(depth[bi]);
                    }
                }
            }
        }
        ad
    } else {
        Vec::new()
    };
    if (0..ns).any(|sl| ok[sl] && esc[sl]) {
        let nb = f.blocks.len();
        let succ = |bi: usize| -> Vec<usize> { f.blocks[bi].term.succs().map(|t| t as usize).collect() };
        // per slot: blocks that access it (loads/stores/copies through its addresses)
        // an access: a load or store, or another call that gets the slot (a call in a loop
        // reaches itself: the iterator handed to a non-inlined next())
        let accesses = |i: &Inst, sl: u32| -> bool {
            match i {
                Inst::Load(_, _, b, _) | Inst::Store(_, b, _, _) | Inst::VLoad(_, b, _) | Inst::VStore(b, _, _) => at(*b).0 == sl,
                Inst::Call(_, args, _) => args.iter().any(|a| at(*a).0 == sl),
                _ => false,
            }
        };
        let mut seen = vec![false; nb];
        // escape call sites per candidate slot, in block / instruction order (one scan)
        let mut sites: Vec<Vec<(u32, u32)>> = vec![Vec::new(); ns];
        for &(bi, k) in &rel {
            let (i, bi, k) = (&f.blocks[bi as usize].insts[k as usize], bi as usize, k as usize); {
                if let Inst::Call(_, args, _) = i {
                    for a in args {
                        let sl = at(*a).0;
                        if sl != u32::MAX && ok[sl as usize] && esc[sl as usize] && sites[sl as usize].last() != Some(&(bi as u32, k as u32)) {
                            sites[sl as usize].push((bi as u32, k as u32));
                        }
                    }
                }
            }
        }
        for sl in 0..ns {
            if !(ok[sl] && esc[sl]) {
                continue;
            }
            // few escape sites only (each last-use site costs a reachability walk; no-capture
            // sites are cheap: a Vec's push loop has a few grow calls)
            if sites[sl].len() > 12 {
                sroa_reject_slot(&mut ok, sl, SroaReject::Escape, &mut stats);
                continue;
            }
            let mut good = true;
            'sites: for &(bi, k) in &sites[sl] {
                let (bi, k) = (bi as usize, k as usize);
                // a call that keeps no pointer to the slot (O3: proven from the callee's
                // RIR): the cells are written back before it and read again after it, so the
                // slot may be used afterwards (the iterator / Vec handed to a non-inlined
                // spec_extend ahead of the loop that then runs on registers)
                // ... and only when the call runs less often than the slot's accesses: a flush and
                // reload per call in the slot's own loop costs more than the memory traffic it
                // saves (bench_rt sort_ints / shape got slower with calls inside the hot loop)
                let nocap = depth[bi] < acc_depth[sl]
                    && match &f.blocks[bi].insts[k] {
                        Inst::Call(c, args, _) => args.iter().enumerate().all(|(j, a)| at(*a).0 != sl as u32 || (at(*a).1 == 0 && sroa_nocapture(c, j))),
                        _ => false,
                    };
                if nocap {
                    reload_sites.push((bi as u32, k as u32, sl as u32));
                    continue;
                }
                {
                    // later in the same block
                    if f.blocks[bi].insts[k + 1..].iter().any(|j| accesses(j, sl as u32)) {
                        good = false;
                        break 'sites;
                    }
                    // any block reachable from here
                    seen.iter_mut().for_each(|x| *x = false);
                    let mut stack = succ(bi);
                    while let Some(x) = stack.pop() {
                        if seen[x] {
                            continue;
                        }
                        seen[x] = true;
                        if f.blocks[x].insts.iter().any(|j| accesses(j, sl as u32)) {
                            good = false;
                            break 'sites;
                        }
                        stack.extend(succ(x));
                    }
                }
            }
            if good {
                esc_slots.push(sl as u32);
            } else {
                sroa_reject_slot(&mut ok, sl, SroaReject::Escape, &mut stats);
                reload_sites.retain(|r| r.2 != sl as u32);
            }
        }
    }
    if let Some(s) = &mut stats {
        s.promoted = (0..ns).filter(|&sl| ok[sl] && root[sl] as usize == sl).count();
    }
    if !ok.iter().any(|x| *x) {
        return false;
    }
    // one vreg per (slot, group cell inside the slot), zero at entry (a cell read before any
    // write of it reads padding or an unwritten variant field: any value will do)
    let mut slot_regs: Vec<Vec<u32>> = vec![Vec::new(); ns];
    let mut init: Vec<Inst> = Vec::new();
    for sl in 0..ns {
        if !ok[sl] {
            continue;
        }
        let g = root[sl] as usize;
        let lo = delta[sl];
        let hi = delta[sl] + f.slots[sl].size as i64;
        let mut regs = Vec::with_capacity(gcells[g].len());
        for k in 0..gcells[g].len() {
            let SroaCell { off, mem: m, .. } = gcells[g][k];
            if off < lo || off + mem_size(m) as i64 > hi {
                regs.push(u32::MAX);
                continue;
            }
            if m == SROA_V128 {
                // a constant (rematerialised at its uses: movi / pxor), not a register held
                // from the entry on
                let r = f.vreg(Cls::V128);
                init.push(Inst::VConst(r, 0));
                regs.push(r.0);
                continue;
            }
            let r = f.vreg(match m {
                Mem::F32 => Cls::F32,
                Mem::F64 => Cls::F64,
                Mem::Int(..) => Cls::I,
            });
            init.push(match m {
                Mem::F32 => Inst::Fconst(r, 0, false),
                Mem::F64 => Inst::Fconst(r, 0, true),
                Mem::Int(..) => Inst::Iconst(r, 0),
            });
            regs.push(r.0);
        }
        slot_regs[sl] = regs;
    }
    let esc_addr: Vec<(u32, VReg)> = esc_slots.iter().map(|&sl| (sl, f.vreg(Cls::I))).collect();
    for &(sl, v) in &esc_addr {
        init.push(Inst::SlotAddr(v, sl));
    }
    // Every promoted access names an entire cell, including mixed-kind scalar accesses.
    let cell_at = |sl: u32, roff: i64| -> (VReg, SroaCell) {
        let g = root[sl as usize] as usize;
        // a lane access names the vector cell that contains it
        let k = match gcells[g].binary_search_by_key(&roff, |c| c.off) {
            Ok(k) => k,
            Err(k) if k > 0 && gcells[g][k - 1].lanes && roff < gcells[g][k - 1].off + 16 => k - 1,
            Err(_) => panic!("sroa cell"),
        };
        let c = gcells[g][k];
        let r = slot_regs[sl as usize][k];
        assert!(r != u32::MAX, "sroa cell outside its slot");
        (VReg(r), c)
    };
    let cell = |sl, roff| cell_at(sl, roff).0;
    let okat = |v: VReg| -> Option<(u32, i64)> {
        let (sl, o) = at(v);
        if sl != u32::MAX && ok[sl as usize] {
            Some((sl, o + delta[sl as usize]))
        } else {
            None
        }
    };
    // only blocks with an access of a replaced slot are rebuilt
    let touches = |i: &Inst| -> bool {
        match i {
            Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::Store(_, base, _, _) | Inst::VStore(base, _, _) => okat(*base).is_some(),
            Inst::Copy(d, s2, _) => okat(*d).is_some() || okat(*s2).is_some(),
            Inst::Call(_, args, _) => !esc_addr.is_empty() && args.iter().any(|x| esc_addr.iter().any(|e| e.0 == at(*x).0)),
            _ => false,
        }
    };
    let mut dirty = vec![false; f.blocks.len()];
    for &(bi, k) in &rel {
        if !dirty[bi as usize] && touches(&f.blocks[bi as usize].insts[k as usize]) {
            dirty[bi as usize] = true;
        }
    }
    // a reload site's cells, read back from the slot after the call (as many as the flush wrote)
    let reload = |sl: u32, a: VReg, out: &mut Vec<Inst>| {
        let g = root[sl as usize] as usize;
        for (k, &SroaCell { off, mem: m, .. }) in gcells[g].iter().enumerate() {
            if slot_regs[sl as usize][k] != u32::MAX {
                let (o, r) = ((off - delta[sl as usize]) as i32, VReg(slot_regs[sl as usize][k]));
                out.push(if m == SROA_V128 { Inst::VLoad(r, a, o) } else { Inst::Load(m, r, a, o) });
            }
        }
    };
    // invoked reload sites: the reloads go on both edges (blocks made below)
    let mut edge_reloads: Vec<(usize, u32, VReg)> = Vec::new();
    for (bi, (b, _)) in f.blocks.iter_mut().zip(dirty.iter()).enumerate().filter(|x| *x.1 .1) {
        let mut out: Vec<Inst> = Vec::with_capacity(b.insts.len());
        let mut pos: Vec<u64> = Vec::with_capacity(b.pos.len());
        for (k, i) in b.insts.iter().enumerate() {
            let p = b.pos[k];
            match i {
                Inst::Load(m, d, base, off) if okat(*base).is_some() => {
                    let (sl, o) = okat(*base).unwrap();
                    let (r, c) = cell_at(sl, o + *off as i64);
                    if c.lanes {
                        let lane = ((o + *off as i64 - c.off) / 4) as u8;
                        out.push(Inst::VExtract(*d, r, lane));
                        if *m == Mem::Int(4, false) {
                            pos.push(p);
                            out.push(Inst::Conv(Conv::IntToInt(IntTy { bits: 32, signed: false }), *d, *d));
                        }
                    } else if c.widened {
                        out.push(match *m {
                            Mem::Int(n, signed) if n < 8 => Inst::Conv(Conv::IntToInt(IntTy { bits: n * 8, signed }), *d, r),
                            Mem::Int(..) => Inst::Mov(*d, r),
                            Mem::F32 | Mem::F64 => Inst::Conv(Conv::BitsToF(*m == Mem::F64), *d, r),
                        });
                    } else {
                        out.push(Inst::Mov(*d, r));
                    }
                    pos.push(p);
                }
                Inst::VLoad(d, base, off) if okat(*base).is_some() => {
                    let (sl, o) = okat(*base).unwrap();
                    out.push(Inst::Mov(*d, cell(sl, o + *off as i64)));
                    pos.push(p);
                }
                Inst::VStore(base, off, v) if okat(*base).is_some() => {
                    let (sl, o) = okat(*base).unwrap();
                    out.push(Inst::Mov(cell(sl, o + *off as i64), *v));
                    pos.push(p);
                }
                Inst::Store(m, base, off, v) if okat(*base).is_some() => {
                    let (sl, o) = okat(*base).unwrap();
                    let (r, c) = cell_at(sl, o + *off as i64);
                    if c.lanes {
                        out.push(Inst::VInsert(r, r, ((o + *off as i64 - c.off) / 4) as u8, *v));
                    } else if c.widened {
                        out.push(match m {
                            Mem::F32 | Mem::F64 => Inst::Conv(Conv::FToBits(*m == Mem::F64), r, *v),
                            Mem::Int(..) => Inst::Mov(r, *v),
                        });
                    } else {
                        out.push(match m {
                            Mem::Int(n, sg) if *n < 8 => Inst::Conv(Conv::IntToInt(IntTy { bits: *n * 8, signed: *sg }), r, *v),
                            _ => Inst::Mov(r, *v),
                        });
                    }
                    pos.push(p);
                }
                Inst::Copy(d, s2, n) if okat(*d).is_some() || okat(*s2).is_some() => {
                    let (da, sa) = (okat(*d), okat(*s2));
                    // the group's cells inside the copied range, in root coordinates
                    let (g, lo) = match (da, sa) {
                        (Some((sl, o)), _) => (root[sl as usize] as usize, o),
                        (None, Some((sl, o))) => (root[sl as usize] as usize, o),
                        _ => unreachable!(),
                    };
                    let src_lo = sa.map(|x| x.1).unwrap_or(0);
                    for &SroaCell { off, mem: m, .. } in &gcells[g] {
                        if off < lo || off + mem_size(m) as i64 > lo + *n as i64 {
                            continue;
                        }
                        let rel = off - lo; // byte offset inside the copied range
                        out.push(match (da, sa) {
                            (Some((dsl, _)), Some((ssl, _))) => Inst::Mov(cell(dsl, off), cell(ssl, src_lo + rel)),
                            (Some((dsl, _)), None) if m == SROA_V128 => Inst::VLoad(cell(dsl, off), *s2, rel as i32),
                            (None, Some((ssl, _))) if m == SROA_V128 => Inst::VStore(*d, rel as i32, cell(ssl, src_lo + rel)),
                            (Some((dsl, _)), None) => Inst::Load(m, cell(dsl, off), *s2, rel as i32),
                            (None, Some((ssl, _))) => Inst::Store(m, *d, rel as i32, cell(ssl, src_lo + rel)),
                            _ => unreachable!(),
                        });
                        pos.push(p);
                    }
                }
                Inst::Call(_, args, _) if !esc_addr.is_empty() => {
                    // an escaped slot's last use: the slot gets its cells' values first
                    for &(sl, a) in &esc_addr {
                        if !args.iter().any(|x| at(*x).0 == sl) {
                            continue;
                        }
                        let g = root[sl as usize] as usize;
                        for (k, &SroaCell { off, mem: m, .. }) in gcells[g].iter().enumerate() {
                            if slot_regs[sl as usize][k] != u32::MAX {
                                let (o, r) = ((off - delta[sl as usize]) as i32, VReg(slot_regs[sl as usize][k]));
                                out.push(if m == SROA_V128 { Inst::VStore(a, o, r) } else { Inst::Store(m, a, o, r) });
                                pos.push(p);
                            }
                        }
                    }
                    out.push(i.clone());
                    pos.push(p);
                    for &(sb, sk, sl) in &reload_sites {
                        if sb as usize != bi || sk as usize != k {
                            continue;
                        }
                        let a = esc_addr.iter().find(|e| e.0 == sl).map(|e| e.1).expect("reload site slot");
                        if k + 1 == b.insts.len() && matches!(b.term, Term::Invoke(..)) {
                            edge_reloads.push((bi, sl, a));
                        } else {
                            let n0 = out.len();
                            reload(sl, a, &mut out);
                            pos.extend(std::iter::repeat(p).take(out.len() - n0));
                        }
                    }
                }
                _ => {
                    out.push(i.clone());
                    pos.push(p);
                }
            }
        }
        b.insts = out;
        b.pos = pos;
    }
    // an invoked no-capture call: its normal and unwind successors each get a block that
    // reloads the cells first (the callee may have written the slot before unwinding)
    let mut by_block: Vec<(usize, Vec<(u32, VReg)>)> = Vec::new();
    for (bi, sl, a) in edge_reloads {
        match by_block.iter_mut().find(|x| x.0 == bi) {
            Some(x) => x.1.push((sl, a)),
            None => by_block.push((bi, vec![(sl, a)])),
        }
    }
    for (bi, sls) in by_block {
        let Term::Invoke(n, u) = f.blocks[bi].term else { continue };
        let p = f.blocks[bi].term_pos;
        let mut targets = [n, u];
        for t in targets.iter_mut() {
            let mut insts = Vec::new();
            for &(sl, a) in &sls {
                reload(sl, a, &mut insts);
            }
            let np = insts.len();
            let nb = f.blocks.len() as u32;
            f.blocks.push(Block { insts, pos: vec![p; np], term: Term::Jump(*t), term_pos: p, count: -1.0 });
            *t = nb;
        }
        f.blocks[bi].term = Term::Invoke(targets[0], targets[1]);
    }
    let p0 = f.blocks[0].pos.first().copied().unwrap_or(0);
    let n0 = init.len();
    let b0 = &mut f.blocks[0];
    b0.insts.splice(0..0, init);
    b0.pos.splice(0..0, std::iter::repeat(p0).take(n0));
    true
}

pub fn mem_size(m: Mem) -> i32 {
    match m {
        Mem::Int(n, _) => n as i32,
        Mem::F32 => 4,
        Mem::F64 => 8,
    }
}

/// Loop-invariant code motion: pure, non-trapping, single-definition instructions inside a
/// natural loop whose operands come from outside it (or from other hoisted instructions)
/// move to the end of the loop's preheader (its single outside predecessor ending in a
/// jump to the header). Loads stay (stores/calls in the loop), integer div/rem stay (trap).
/// No edge goes to the same or an earlier block: the CFG has no cycle, so no loop.
pub fn forward_only(f: &Func) -> bool {
    f.blocks.iter().enumerate().all(|(bi, b)| b.term.succs().all(|t| t as usize > bi))
}

/// Predecessors of the reachable blocks and the natural loops (header, member flags), inner
/// loops (fewer blocks) first.
pub fn natural_loops(f: &Func) -> (Vec<Vec<u32>>, Vec<(u32, Vec<bool>)>) {
    let n = f.blocks.len();
    let idom = idoms(f);
    let dominates = |a: u32, b: u32| -> bool {
        let mut x = b;
        let mut guard = 0;
        loop {
            if x == a {
                return true;
            }
            let p = idom[x as usize];
            if p == u32::MAX || p == x || guard > 10000 {
                return false;
            }
            x = p;
            guard += 1;
        }
    };
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        for t in b.term.succs() {
            preds[t as usize].push(bi as u32);
        }
    }
    // natural loops per header
    let mut loops: Vec<(u32, Vec<bool>)> = Vec::new();
    for h in 0..n {
        let mut latches = Vec::new();
        for &p in &preds[h] {
            if dominates(h as u32, p) {
                latches.push(p);
            }
        }
        if latches.is_empty() {
            continue;
        }
        let mut body = vec![false; n];
        body[h] = true;
        let mut stack = latches.clone();
        while let Some(x) = stack.pop() {
            if body[x as usize] {
                continue;
            }
            body[x as usize] = true;
            for &p in &preds[x as usize] {
                stack.push(p);
            }
        }
        loops.push((h as u32, body));
    }
    // inner loops first
    loops.sort_by_key(|l| l.1.iter().filter(|x| **x).count());
    (preds, loops)
}

/// Loop-invariant reassociation (C2's `reassociate_invariants`): in a loop, `t1 = a + b; t2 = t1 + c`
/// with a and c invariant, b not, and t1 read only by t2 becomes `t1 = a + c; t2 = t1 + b`, so the
/// invariant half hoists (a 2-D index `row + x + k`: `row + k` once per row). Integer adds wrap in
/// their type, so the regrouping is exact. Runs at the start of licm. Returns whether any changed.
/// MAKEPAD_RUN_OPT=-reassoc: off (bisection).
pub fn reassociate_invariants(options: &relax_session::Options, f: &mut Func) -> bool {
    if crate::stats::opt_off(options, "-reassoc") {
        return false;
    }
    let (_, loops) = natural_loops(f);
    if loops.is_empty() {
        return false;
    }
    let nv = f.vregs.len();
    let (mut us, mut ds) = (Vec::new(), Vec::new());
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            uses_defs(i, &mut us, &mut ds);
            for d in &ds {
                ndef[d.0 as usize] += 1;
            }
            for u in &us {
                nuse[u.0 as usize] += 1;
            }
        }
        crate::regalloc::term_uses(&b.term, &mut tu);
        for u in &tu {
            nuse[u.0 as usize] += 1;
        }
    }
    let mut changed = false;
    for (_, body) in &loops {
        // defined in the loop (not invariant)
        let mut inl = vec![false; nv];
        for (bi, b) in f.blocks.iter().enumerate() {
            if body[bi] {
                for i in &b.insts {
                    for_defs(i, |d| inl[d.0 as usize] = true);
                }
            }
        }
        for bi in 0..f.blocks.len() {
            if !body[bi] {
                continue;
            }
            for k in 0..f.blocks[bi].insts.len() {
                // t2 = t1 + c (c invariant or a constant)
                let (it2, t2, t1, c) = match f.blocks[bi].insts[k] {
                    Inst::IBin(IOp::Add, it, t2, x, y) if !inl[y.0 as usize] && inl[x.0 as usize] => (it, t2, x, Some(y)),
                    Inst::IBin(IOp::Add, it, t2, x, y) if !inl[x.0 as usize] && inl[y.0 as usize] => (it, t2, y, Some(x)),
                    Inst::IBinI(IOp::Add, it, t2, x, _) if inl[x.0 as usize] => (it, t2, x, None),
                    _ => continue,
                };
                if t1 == t2 || ndef[t1.0 as usize] != 1 || nuse[t1.0 as usize] != 1 {
                    continue;
                }
                // t1 = a + b in this block before, a invariant, b not; b unchanged up to t2
                let Some(j) = (0..k).rev().find(|&j| {
                    let mut d = false;
                    for_defs(&f.blocks[bi].insts[j], |x| d |= x == t1);
                    d
                }) else { continue };
                let Inst::IBin(IOp::Add, it1, _, a0, b0) = f.blocks[bi].insts[j] else { continue };
                if it1 != it2 {
                    continue;
                }
                let (a, b) = if !inl[a0.0 as usize] && inl[b0.0 as usize] {
                    (a0, b0)
                } else if !inl[b0.0 as usize] && inl[a0.0 as usize] {
                    (b0, a0)
                } else {
                    continue;
                };
                if b == t2 || f.blocks[bi].insts[j + 1..k].iter().any(|i| {
                    let mut d = false;
                    for_defs(i, |x| d |= x == b);
                    d
                }) {
                    continue;
                }
                let (new1, new2) = match (&f.blocks[bi].insts[k], c) {
                    (Inst::IBin(..), Some(c)) => (Inst::IBin(IOp::Add, it1, t1, a, c), Inst::IBin(IOp::Add, it2, t2, t1, b)),
                    (Inst::IBinI(_, _, _, _, kk), None) => (Inst::IBinI(IOp::Add, it1, t1, a, *kk), Inst::IBin(IOp::Add, it2, t2, t1, b)),
                    _ => continue,
                };
                f.blocks[bi].insts[j] = new1;
                f.blocks[bi].insts[k] = new2;
                // t1 is invariant now (licm hoists it); t2 still is not
                changed = true;
            }
        }
    }
    changed
}

pub fn licm(options: &relax_session::Options, f: &mut Func) -> bool {
    if forward_only(f) {
        return false;
    }
    let reassoc = reassociate_invariants(options, f);
    let n = f.blocks.len();
    let (preds, loops) = natural_loops(f);
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut def_block = vec![u32::MAX; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
                def_block[d.0 as usize] = bi as u32;
            });
        }
    }
    let mut changed = false;
    // loads of an uncaptured noalias param's memory (`&mut self` fields) are invariant in a loop
    // whose stores go through pointers not derived from that param and whose calls are not
    // handed it: rustc's `noalias`, with local_cse's roots. Params withheld until the borrow
    // check's verdict are not in `f.noalias`, so they hoist nothing.
    let mut na = if f.noalias.is_empty() || no_noalias(options) { None } else { Some(noalias_roots(f, &ndef)) };
    // a param handed to a call anywhere in the function may be kept by it (then reached later through a
    // loaded pointer, which the roots take as not derived from the param): captured for the hoist,
    // unless the callee provably keeps no pointer it gets there (O3's param_nocapture oracle, as
    // LLVM's `nocapture`: `f1.decode(&mut r, &self.setup.codebooks, y)` left every `self.spectra`
    // load in decode_spectra's loops)
    if let Some((_, cap, taint)) = na.as_mut() {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Call(c, args, _) = i {
                    let ind = match c {
                        Callee::Indirect(x) | Callee::CIndirect(x, _) => Some(*x),
                        _ => None,
                    };
                    for (k, a) in args.iter().copied().enumerate() {
                        let t = taint[a.0 as usize];
                        if t != u32::MAX && !cap.contains(&t) && !(sroa_nocapture(c, k) && !opt_off(options, "-licmnocap")) {
                            cap.push(t);
                        }
                    }
                    if let Some(x) = ind {
                        let t = taint[x.0 as usize];
                        if t != u32::MAX && !cap.contains(&t) {
                            cap.push(t);
                        }
                    }
                }
            }
        }
    }
    for (h, body) in &loops {
        let outside: Vec<u32> = preds[*h as usize].iter().copied().filter(|p| !body[*p as usize]).collect();
        if outside.len() != 1 {
            continue;
        }
        let pre = outside[0];
        if !matches!(f.blocks[pre as usize].term, Term::Jump(t) if t == *h) {
            continue;
        }
        let mut hoisted: Vec<(Inst, u64)> = Vec::new();
        // the noalias params whose memory the loop may write (None: any memory, or unknown)
        let na_clob: Option<Vec<u32>> = na.as_ref().and_then(|(root, _, taint)| {
            let mut cl: Vec<u32> = Vec::new();
            for bi in 0..n {
                if !body[bi] {
                    continue;
                }
                for i in &f.blocks[bi].insts {
                    match i {
                        Inst::Store(_, base, ..) | Inst::StoreX(_, base, ..) | Inst::VStore(base, ..) | Inst::Copy(base, ..) => {
                            let r = root[base.0 as usize];
                            if r >> 40 == 1 {
                                cl.push((r & 0xffff_ffff) as u32);
                            } else if r != LOADED_ROOT && !(r != 0 && r < (1u64 << 40)) {
                                // a store through a pointer of unknown root
                                return None;
                            }
                        }
                        Inst::Call(c, args, _) => {
                            let callee = match c {
                                Callee::Indirect(x) | Callee::CIndirect(x, _) => Some(*x),
                                _ => None,
                            };
                            for a in args.iter().chain(callee.iter()) {
                                if taint[a.0 as usize] != u32::MAX {
                                    cl.push(taint[a.0 as usize]);
                                }
                            }
                        }
                        // a read of memory: the param's memory is ours alone, nothing orders against it
                        Inst::AtomicLoad(..) => {}
                        // atomics, fences, FP env: ordering points and writes the roots do not model
                        i if is_mem_effect(i) => return None,
                        _ => {}
                    }
                }
            }
            Some(cl)
        });
        // a loop that writes no memory and calls nothing: loads from dereferenceable params
        // read the same value on every iteration and may run before it
        let read_only = (0..n).all(|bi| !body[bi] || (f.blocks[bi].insts.iter().all(|i| !is_mem_effect(i)) && matches!(f.blocks[bi].term, Term::Jump(_) | Term::Branch(..) | Term::Ret(_) | Term::Unreachable)));
        // vregs with a definition inside the loop (multi-def ones never become invariant
        // through hoisting: only single-def instructions move)
        let mut def_in_loop = vec![false; nv];
        for bi in 0..n {
            if body[bi] {
                for i2 in &f.blocks[bi].insts {
                    for_defs(i2, |d| def_in_loop[d.0 as usize] = true);
                }
            }
        }
        let mut progress = true;
        while progress {
            progress = false;
            // constants some invariant user needs: a pure, single-def user in the loop whose
            // only in-loop operand is the constant (computed once per sweep: it was a scan of
            // the whole loop per constant)
            let mut needed = vec![false; nv];
            for b2 in 0..n {
                if !body[b2] {
                    continue;
                }
                for i2 in &f.blocks[b2].insts {
                    if matches!(i2, Inst::Load(..) | Inst::Store(..) | Inst::Call(..) | Inst::Copy(..) | Inst::Mov(..)) {
                        continue;
                    }
                    uses_defs(i2, &mut uses, &mut defs);
                    if !(defs.len() == 1 && ndef[defs[0].0 as usize] == 1) {
                        continue;
                    }
                    let mut inloop: Option<VReg> = None;
                    let mut ok = true;
                    for u in &uses {
                        let db = def_block[u.0 as usize];
                        if db != u32::MAX && body[db as usize] {
                            match inloop {
                                None => inloop = Some(*u),
                                Some(x) if x == *u => {}
                                Some(_) => ok = false,
                            }
                        }
                    }
                    if let (true, Some(v)) = (ok, inloop) {
                        needed[v.0 as usize] = true;
                    }
                }
            }
            for bi in 0..n {
                if !body[bi] {
                    continue;
                }
                let mut k = 0;
                while k < f.blocks[bi].insts.len() {
                    let i = &f.blocks[bi].insts[k];
                    let pure = match i {
                        Inst::IBin(IOp::Div | IOp::Rem, ..) | Inst::IBinI(IOp::Div | IOp::Rem, ..) => false,
                        Inst::Load(m, _, base, off) if read_only && *off >= 0 && ndef[base.0 as usize] == 1 && f.deref.iter().any(|(p, sz)| p == base && (*off as i64 + mem_size(*m) as i64) <= *sz as i64) => true,
                        // a load of an uncaptured noalias param's own dereferenceable memory that
                        // the loop never writes: same value every iteration, and safe to run
                        // before the loop (the param is dereferenceable for the whole call)
                        Inst::Load(m, _, base, off)
                            if *off >= 0
                                && ndef[base.0 as usize] == 1
                                && f.noalias.contains(base)
                                && f.deref.iter().any(|(p, sz)| p == base && (*off as i64 + mem_size(*m) as i64) <= *sz as i64)
                                && na.as_ref().map_or(false, |(_, cap, _)| !cap.contains(&base.0))
                                && na_clob.as_ref().map_or(false, |cl| !cl.contains(&base.0)) =>
                        {
                            true
                        }
                        Inst::Load(..) | Inst::VLoad(..) | Inst::VGather(..) | Inst::VTbl(..) | Inst::Mov(..) => false,
                        i if is_mem_effect(i) => false,
                        _ => true,
                    };
                    if !pure {
                        k += 1;
                        continue;
                    }
                    uses_defs(i, &mut uses, &mut defs);
                    let mut inv = defs.len() == 1 && ndef[defs[0].0 as usize] == 1;
                    for u in &uses {
                        let db = def_block[u.0 as usize];
                        let in_loop = db != u32::MAX && body[db as usize];
                        // multi-def operands are invariant only if no definition is in the loop
                        if in_loop || (ndef[u.0 as usize] > 1 && def_in_loop[u.0 as usize]) {
                            inv = false;
                        }
                    }
                    // bare constants are not worth a register across the loop on their own:
                    // hoisted only when some invariant user needs it
                    let is_const = matches!(i, Inst::Iconst(..) | Inst::Fconst(..) | Inst::Addr(..) | Inst::FnAddr(..) | Inst::SlotAddr(..) | Inst::VConst(..));
                    if inv && is_const {
                        inv = needed[defs[0].0 as usize];
                    }
                    if inv {
                        let i = f.blocks[bi].insts.remove(k);
                        let p = f.blocks[bi].pos.remove(k);
                        def_block[defs[0].0 as usize] = pre;
                        hoisted.push((i, p));
                        progress = true;
                        changed = true;
                    } else {
                        k += 1;
                    }
                }
            }
        }
        for (i, p) in hoisted {
            f.blocks[pre as usize].insts.push(i);
            f.blocks[pre as usize].pos.push(p);
        }
    }
    changed || reassoc
}

/// 64-bit shifts by a register take the count mod 64 on both backends (x64 shl/shr/sar/shlx,
/// arm64 lslv/lsrv/asrv): `s = y & 63` feeding only such a shift, in the same block, goes.
pub fn fold_shift_masks(f: &mut Func) {
    let nv = f.vregs.len();
    let mut nuse = vec![0u32; nv];
    let mut uses = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    let mut defs = Vec::new();
    for b in &mut f.blocks {
        let n = b.insts.len();
        let mut dead = vec![false; n];
        for k in 0..n {
            let (s0, it) = match b.insts[k] {
                Inst::IBin(IOp::Shl | IOp::Shr, it, _, _, s0) if it.bits == 64 => (s0, it),
                _ => continue,
            };
            let _ = it;
            if nuse[s0.0 as usize] != 1 {
                continue;
            }
            // the mask: defined earlier in this block, its source not redefined since
            let mut found = None;
            for j in (0..k).rev() {
                let mut defines = false;
                for_defs(&b.insts[j], |d| defines |= d == s0);
                if defines {
                    if let Inst::IBinI(IOp::And, mt, _, y, 63) = b.insts[j] {
                        if mt.bits == 64 {
                            found = Some((j, y));
                        }
                    }
                    break;
                }
            }
            let (j, y) = match found {
                Some(x) => x,
                None => continue,
            };
            let mut ok = true;
            for m in j + 1..k {
                defs.clear();
                for_defs(&b.insts[m], |d| defs.push(d));
                if defs.contains(&y) {
                    ok = false;
                }
            }
            if !ok {
                continue;
            }
            if let Inst::IBin(_, _, _, _, ref mut s1) = b.insts[k] {
                *s1 = y;
            }
            dead[j] = true;
        }
        if dead.iter().any(|x| *x) {
            let mut k = 0;
            b.insts.retain(|_| {
                k += 1;
                !dead[k - 1]
            });
            let mut k2 = 0;
            b.pos.retain(|_| {
                k2 += 1;
                !dead[k2 - 1]
            });
        }
    }
}

/// One address per stack slot (backend preparation, before addr_modes): lowering and constant
/// sinking leave a `SlotAddr(v, s)` next to each access; they become one value defined at the
/// entry, which the allocator keeps in a register, folds into the access (x64 [rbp + ..], arm64
/// [sp + ..]) or recomputes where it has no register (Alloc::remat), instead of an add per access.
pub fn unify_slot_addrs(f: &mut Func) {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut n_slot_defs = 0usize;
    // a call argument cannot be recomputed at its use (Alloc::remat excludes call arguments): an
    // entry definition of one stays live through the whole function and, in a large function, is
    // computed and spilled on every entry (a large interpreter's opcode(): ~245 slot addresses, 1238
    // instructions per call); such a SlotAddr keeps its own definition next to the call
    let mut call_arg = vec![false; nv];
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if matches!(i, Inst::SlotAddr(..)) {
                n_slot_defs += 1;
            }
            if let Inst::Call(_, args, _) = i {
                for a in args {
                    call_arg[a.0 as usize] = true;
                }
            }
        }
    }
    if n_slot_defs < 2 {
        return;
    }
    let mut canon: std::collections::HashMap<u32, VReg> = std::collections::HashMap::new();
    let mut map: Vec<u32> = (0..nv as u32).collect();
    let mut order: Vec<u32> = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, s) = i {
                if ndef[d.0 as usize] == 1 && !call_arg[d.0 as usize] {
                    let c = *canon.entry(*s).or_insert_with(|| {
                        order.push(*s);
                        VReg(u32::MAX)
                    });
                    let _ = c;
                }
            }
        }
    }
    if order.is_empty() {
        return;
    }
    for &s in &order {
        let v = f.vreg(Cls::I);
        canon.insert(s, v);
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, s) = i {
                if ndef[d.0 as usize] == 1 && !call_arg[d.0 as usize] {
                    map[d.0 as usize] = canon[s].0;
                }
            }
        }
    }
    for b in &mut f.blocks {
        let mut k = 0;
        let mut keep = Vec::with_capacity(b.insts.len());
        for i in &b.insts {
            keep.push(!matches!(i, Inst::SlotAddr(d, _) if ndef[d.0 as usize] == 1 && !call_arg[d.0 as usize]));
        }
        b.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        let mut k2 = 0;
        b.pos.retain(|_| {
            k2 += 1;
            keep[k2 - 1]
        });
        for i in b.insts.iter_mut() {
            i.vregs_mut(|r| {
                if (r.0 as usize) < nv {
                    r.0 = map[r.0 as usize];
                }
            });
        }
        match &mut b.term {
            Term::Branch(c, _, _) => {
                if (c.0 as usize) < nv {
                    c.0 = map[c.0 as usize];
                }
            }
            Term::Ret(vs) => {
                for v in vs.iter_mut() {
                    if (v.0 as usize) < nv {
                        v.0 = map[v.0 as usize];
                    }
                }
            }
            _ => {}
        }
    }
    let p0 = f.blocks[0].pos.first().copied().unwrap_or(f.blocks[0].term_pos);
    let defs: Vec<Inst> = order.iter().map(|s| Inst::SlotAddr(canon[s], *s)).collect();
    let n = defs.len();
    f.blocks[0].insts.splice(0..0, defs);
    f.blocks[0].pos.splice(0..0, std::iter::repeat(p0).take(n));
}

/// Backend address modes (run on the codegen copy only, after every IR pass): a load or
/// store whose base is `p + (i << k)` (k <= 3) or `p + i`, computed in the same block for
/// this access only, becomes LoadX/StoreX and the address arithmetic disappears.
/// Copies of 1, 2, 4, 8 or 16 bytes become integer loads then stores, so address modes can fold
/// their addresses (a stable-partition step `copy(sel + count * 8, src, 8)` keeps `count << 3` and
/// the add as temporaries otherwise; as [base + index * 8] it is one store).
/// A small copy whose source was just loaded whole (same block, or the end of its one
/// predecessor, no write or call between): a store of that value. `retain`'s
/// `if keep(&v[i]) { v[j] = v[i] }` reads the element once.
fn copies_from_loaded(f: &mut Func) {
    let n = f.blocks.len();
    let mut npred = vec![0u32; n];
    let mut pred = vec![u32::MAX; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        for t in b.term.succs() {
            npred[t as usize] += 1;
            pred[t as usize] = bi as u32;
        }
    }
    // single-def `v = b + k`: accesses at [b + k] read the same bytes as at [v]
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    let mut add_def: Vec<Option<(VReg, i64)>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::IBinI(IOp::Add, it, d, x, c) = *i {
                if it.bits == 64 && ndef[d.0 as usize] == 1 && ndef[x.0 as usize] == 1 && c.abs() < 1 << 30 {
                    add_def[d.0 as usize] = Some((x, c));
                }
            }
        }
    }
    for bi in 0..n {
        for k in 0..f.blocks[bi].insts.len() {
            let Inst::Copy(d, src, sz @ (1 | 2 | 4 | 8)) = f.blocks[bi].insts[k] else { continue };
            let (sb, sk) = add_def[src.0 as usize].unwrap_or((src, 0));
            let at = |base: VReg, off: i32| -> Option<i64> {
                if base == src {
                    Some(sk + off as i64)
                } else if base == sb {
                    Some(off as i64)
                } else {
                    None
                }
            };
            let mut seen: Vec<VReg> = Vec::new();
            let mut found = None;
            let (mut b, mut end) = (bi, k);
            'scan: for hop in 0..2 {
                for j in (0..end).rev() {
                    let i = &f.blocks[b].insts[j];
                    if let Inst::Load(Mem::Int(w, _), v, base, off) = *i {
                        if at(base, off) == Some(sk) && w as u32 == sz && !seen.contains(&v) {
                            found = Some(v);
                            break 'scan;
                        }
                    }
                    // a store through the source's own base beside the copied bytes leaves them
                    let beside = matches!(*i, Inst::Store(m, base, off, _) if at(base, off).is_some_and(|o| o >= sk + sz as i64 || o + mem_size(m) as i64 <= sk));
                    if !beside && matches!(i, Inst::Store(..) | Inst::StoreX(..) | Inst::VStore(..) | Inst::VStoreX(..) | Inst::Copy(..) | Inst::Call(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..)) {
                        break 'scan;
                    }
                    let mut hit_src = false;
                    for_defs(i, |x| {
                        hit_src |= x == src || x == sb;
                        seen.push(x);
                    });
                    if hit_src {
                        break 'scan;
                    }
                }
                if hop == 1 || npred[b] != 1 || pred[b] == u32::MAX || pred[b] as usize == b || matches!(f.blocks[pred[b] as usize].term, Term::Invoke(..)) {
                    break;
                }
                b = pred[b] as usize;
                end = f.blocks[b].insts.len();
            }
            if let Some(v) = found {
                f.blocks[bi].insts[k] = Inst::Store(Mem::Int(sz as u8, false), d, 0, v);
            }
        }
    }
}

pub fn expand_small_copies(f: &mut Func) {
    copies_from_loaded(f);
    for bi in 0..f.blocks.len() {
        if !f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::Copy(_, _, 1 | 2 | 4 | 8 | 16))) {
            continue;
        }
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let oldp = std::mem::take(&mut f.blocks[bi].pos);
        let mut ins = Vec::with_capacity(old.len() + 4);
        let mut pos = Vec::with_capacity(old.len() + 4);
        for (i, p) in old.into_iter().zip(oldp) {
            match i {
                Inst::Copy(d, s2, n @ (1 | 2 | 4 | 8 | 16)) => {
                    let (w, k) = if n == 16 { (8u8, 2) } else { (n as u8, 1) };
                    let m = Mem::Int(w, false);
                    let ts: Vec<VReg> = (0..k).map(|_| f.vreg(Cls::I)).collect();
                    // every load before any store, as one memcpy reads its source
                    for (j, t) in ts.iter().enumerate() {
                        ins.push(Inst::Load(m, *t, s2, j as i32 * w as i32));
                        pos.push(p);
                    }
                    for (j, t) in ts.iter().enumerate() {
                        ins.push(Inst::Store(m, d, j as i32 * w as i32, *t));
                        pos.push(p);
                    }
                }
                other => {
                    ins.push(other);
                    pos.push(p);
                }
            }
        }
        f.blocks[bi].insts = ins;
        f.blocks[bi].pos = pos;
    }
}

/// Late: `c ? 1 : 0` / `c ? 0 : 1` of a compare result c (0 / 1) is c / c ^ 1 (setcc instead of
/// two constants and a cmov: `count += is_less as usize` after an Ordering became a select).
/// Runs after the last simplify, whose Ordering / phi folds read selects of constants.
pub fn bool_selects(f: &mut Func) {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut k01: Vec<Option<i64>> = vec![None; nv];
    let mut is_cmp = vec![false; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            match *i {
                Inst::Iconst(d, k @ (0 | 1)) => k01[d.0 as usize] = Some(k),
                Inst::ICmp(_, _, d, ..) | Inst::ICmpI(_, _, d, ..) | Inst::FCmp(_, _, d, ..) => is_cmp[d.0 as usize] = true,
                _ => {}
            }
        }
    }
    let one = |v: VReg, ndef: &[u32]| ndef[v.0 as usize] == 1;
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            if let Inst::Sel(d, c, x, y) = *i {
                if !one(c, &ndef) || !is_cmp[c.0 as usize] || !one(x, &ndef) || !one(y, &ndef) || f.vregs[d.0 as usize] != Cls::I {
                    continue;
                }
                match (k01[x.0 as usize], k01[y.0 as usize]) {
                    (Some(1), Some(0)) => *i = Inst::Mov(d, c),
                    (Some(0), Some(1)) => *i = Inst::IBinI(IOp::Xor, U64, d, c, 1),
                    _ => {}
                }
            }
        }
    }
}

pub fn addr_modes(f: &mut Func) {
    bool_selects(f);
    unify_slot_addrs(f);
    fold_shift_masks(f);
    expand_small_copies(f);
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut nuse = vec![0u32; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| {
                nuse[u.0 as usize] += 1;
            });
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
            });
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    for b in &mut f.blocks {
        let n = b.insts.len();
        // index of the defining inst in this block for single-def vregs
        let mut def_at: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        let mut dead = vec![false; n];
        // x64: uses of a vreg as an access's base in this block; an address every use of which is
        // such an access folds into each of them (SIB with any scale and displacement is free),
        // so the add and shift go (lomuto partition: one element loaded and stored through
        // [v + i*4])
        let mut blk_base: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        if cfg!(target_arch = "x86_64") {
            for i in &b.insts {
                match i {
                    Inst::Load(_, _, base, _) | Inst::Store(_, base, _, _) | Inst::VLoad(_, base, _) | Inst::VStore(base, _, _) => *blk_base.entry(base.0).or_insert(0) += 1,
                    _ => {}
                }
            }
            blk_base.retain(|v, c| *c == nuse[*v as usize]);
        }
        for k in 0..n {
            let (m, base, off) = match &b.insts[k] {
                Inst::Load(m, _, base, off) => (*m, *base, *off),
                Inst::Store(m, base, off, _) => (*m, *base, *off),
                // vectors too: movups takes any SIB form; arm64's ldr q an unscaled or x16-scaled
                // register (other shifts: one shifted add instead of a shift and an add)
                Inst::VLoad(_, base, off) | Inst::VStore(base, off, _) => (Mem::Int(8, false), *base, *off),
                i => {
                    for_defs(i, |d| {
                        if ndef[d.0 as usize] == 1 {
                            def_at.insert(d.0, k);
                        }
                    });
                    continue;
                }
            };
            // arm64: an address read by several accesses (a load and a store of one element) is
            // folded into each one that needs no displacement ([x, x{, lsl #size}] is free); the
            // add stays for the others and goes with its last use
            let ai = match def_at.get(&base.0) {
                Some(&ai) if (nuse[base.0 as usize] == 1 || cfg!(target_arch = "aarch64") || blk_base.contains_key(&base.0)) && !dead[ai] => ai,
                _ => continue,
            };
            let shared_add = nuse[base.0 as usize] > 1;
            let (p, q) = match b.insts[ai] {
                Inst::IBin(IOp::Add, it, _, p, q) if it.bits == 64 => (p, q),
                _ => continue,
            };
            // which operand is the scaled index
            // arm64: the access's own scale (q: 16, else its size) is free in the register-offset
            // form, so a shift shared by several accesses (a load and a store of one element) is
            // folded into each; it goes when its last use does
            let vector = matches!(b.insts[k], Inst::VLoad(..) | Inst::VStore(..));
            let natural = if vector {
                4
            } else {
                match m {
                    Mem::Int(n, _) => n.trailing_zeros() as i64,
                    Mem::F32 => 2,
                    Mem::F64 => 3,
                }
            };
            let mut found: Option<(VReg, VReg, u8, Option<usize>)> = None;
            for (pp, qq) in [(p, q), (q, p)] {
                if let Some(&si) = def_at.get(&qq.0) {
                    if let Inst::IBinI(IOp::Shl, it, _, idx, sh) = b.insts[si] {
                        let shared_ok = cfg!(target_arch = "aarch64") && sh == natural;
                        if (nuse[qq.0 as usize] == 1 || shared_ok) && !dead[si] {
                            // scaled index: x86-64 scales 1..8; arm64 vector accesses also 16
                            let max_sh = if cfg!(target_arch = "aarch64") && vector { 4 } else { 3 };
                            if it.bits == 64 && sh <= max_sh {
                                found = Some((pp, idx, sh as u8, Some(si)));
                                break;
                            }
                        }
                    }
                }
            }
            let (pp, idx, sh, si) = match found {
                Some(x) => x,
                None => (p, q, 0, None),
            };
            // `idx = x +- c` computed in this block for this access only: index by x, fold c into
            // the offset (the add goes). When idx is used elsewhere too it is computed anyway:
            // indexing by it is free and keeps arm64's register-offset form.
            let idx_orig = idx;
            let mut idx = idx;
            let mut extra: i64 = 0;
            let mut fold_at: Option<usize> = None;
            // a shift that stays for other uses keeps reading idx: no folding through it
            let shift_stays = shared_add || si.is_some_and(|si| matches!(b.insts[si], Inst::IBinI(_, _, sv, _, _) if nuse[sv.0 as usize] > 1));
            if shared_add && cfg!(target_arch = "aarch64") && (off != 0 || !(sh == 0 || sh as i64 == natural)) {
                continue;
            }
            if let Some(&ii) = def_at.get(&idx.0).filter(|_| !shift_stays) {
                // x64: the displacement is free in every form, so an index used elsewhere too is
                // still replaced by its operand here (`a[j - 1]` reads [b + j*4 - 4]; the subtract
                // stays for its other uses and can then sink to them); arm64 keeps the
                // register-offset form instead
                let shared_ok = cfg!(target_arch = "x86_64") && !dead[ii];
                if (nuse[idx.0 as usize] == 1 && !dead[ii]) || shared_ok {
                    let xc = match b.insts[ii] {
                        Inst::IBinI(IOp::Add, it, _, x, c) if it.bits == 64 && c.abs() < (1 << 20) => Some((x, c)),
                        Inst::IBinI(IOp::Sub, it, _, x, c) if it.bits == 64 && c.abs() < (1 << 20) => Some((x, -c)),
                        _ => None,
                    };
                    if let Some((x, c)) = xc {
                        let mut ok2 = true;
                        for j in ii + 1..k {
                            uses_defs(&b.insts[j], &mut uses, &mut defs);
                            if defs.contains(&x) {
                                ok2 = false;
                            }
                        }
                        if ok2 {
                            idx = x;
                            extra = c << sh;
                            if nuse[idx_orig.0 as usize] == 1 {
                                fold_at = Some(ii);
                            } else {
                                nuse[idx_orig.0 as usize] -= 1;
                                nuse[x.0 as usize] += 1;
                            }
                        }
                    }
                }
            }
            // operands must not be redefined between their use in the arithmetic and here: the base
            // after the add, the index after the shift (or the add, unscaled); a def before its use
            // (a SlotAddr / constant made right before the add) is fine
            let idx_from = si.unwrap_or(ai);
            let mut clobbered = false;
            for j in idx_from.min(ai) + 1..k {
                uses_defs(&b.insts[j], &mut uses, &mut defs);
                if (j > ai && defs.contains(&pp)) || (j > idx_from && defs.contains(&idx)) {
                    clobbered = true;
                }
            }
            if clobbered {
                continue;
            }
            b.insts[k] = match b.insts[k] {
                Inst::Load(m, d, _, off2) => Inst::LoadX(m, d, pp, idx, sh, off2 + extra as i32),
                Inst::Store(m, _, off2, v) => Inst::StoreX(m, pp, idx, sh, off2 + extra as i32, v),
                Inst::VLoad(d, _, off2) => Inst::VLoadX(d, pp, idx, sh, off2 + extra as i32),
                Inst::VStore(_, off2, v) => Inst::VStoreX(pp, idx, sh, off2 + extra as i32, v),
                ref other => other.clone(),
            };
            if shared_add {
                nuse[base.0 as usize] -= 1;
                if nuse[base.0 as usize] > 0 {
                    // the add stays (with its shift): the access reads its operands too
                    nuse[pp.0 as usize] += 1;
                    nuse[idx.0 as usize] += 1;
                    continue;
                }
            }
            dead[ai] = true;
            if let Some(ii) = fold_at {
                dead[ii] = true;
            }
            if let Some(si) = si {
                // a shared shift (arm64) stays for its other uses; the access reads its operand
                let sv = match b.insts[si] {
                    Inst::IBinI(_, _, sv, _, _) => sv,
                    _ => unreachable!(),
                };
                nuse[sv.0 as usize] -= 1;
                if nuse[sv.0 as usize] == 0 {
                    dead[si] = true;
                } else {
                    nuse[idx.0 as usize] += 1;
                }
            }
        }
        if dead.iter().any(|x| *x) {
            let mut k = 0;
            let mut j = 0;
            b.insts.retain(|_| {
                let keep = !dead[k];
                k += 1;
                keep
            });
            b.pos.retain(|_| {
                let keep = !dead[j];
                j += 1;
                keep
            });
        }
    }
}

/// Loop rotation on the codegen copy: a latch ending in `jump H` to a loop header whose
/// block is only a short compare-and-branch gets a copy of that test, so each iteration
/// runs one conditional branch at the bottom instead of a jump plus the header test.
/// A latch whose branch goes back to the header on its false edge (`if skip { continue }`)
/// gets the copy as a new block laid out after it: the latch falls into the test instead of
/// jumping to the header. Header values used only in the header get fresh vregs in the copy;
/// a header value the loop reads elsewhere (`let i = it.start` of a range's `next`, a loaded length) keeps its
/// vreg -- the copy is the very code the jump would have run, so it defines the same values.
pub fn rotate_loops(f: &mut Func) {
    if forward_only(f) {
        return;
    }
    let n = f.blocks.len();
    let idom = idoms(f);
    let dominates = |a: u32, b: u32| -> bool {
        let mut x = b;
        let mut guard = 0;
        loop {
            if x == a {
                return true;
            }
            let p = idom[x as usize];
            if p == u32::MAX || p == x || guard > 10000 {
                return false;
            }
            x = p;
            guard += 1;
        }
    };
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut nuse = vec![0u32; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    // latches ending in a branch whose false edge goes to the header get a new block with the
    // header's copy there (laid out after the latch below)
    let mut added = false;
    for l in 0..n {
        let (h, branch_latch) = match f.blocks[l].term {
            Term::Jump(h) if idom[l] != u32::MAX && dominates(h, l as u32) => (h as usize, false),
            Term::Branch(_, t, e) if t != e && e as usize != l && idom[l] != u32::MAX && dominates(e, l as u32) => (e as usize, true),
            _ => continue,
        };
        let hb = f.blocks[h].clone();
        if hb.insts.len() > 4 || !matches!(hb.term, Term::Branch(..)) {
            continue;
        }
        // header defs must be used only inside the header (they get fresh copies)
        let mut ok = true;
        let mut local_uses = vec![0u32; f.vregs.len()];
        for i in &hb.insts {
            match i {
                Inst::Iconst(..) | Inst::Fconst(..) | Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..) | Inst::IBinI(..) | Inst::IBin(..) | Inst::Mov(..) | Inst::Conv(..) | Inst::Load(..) => {}
                _ => ok = false,
            }
            for_uses(i, |u| local_uses[u.0 as usize] += 1);
        }
        term_uses(&hb.term, &mut uses);
        for u in &uses {
            local_uses[u.0 as usize] += 1;
        }
        // (header def, its copy's vreg): u32::MAX = a fresh one, the def itself when the loop
        // reads it outside the header
        let mut map: Vec<(VReg, VReg)> = Vec::new();
        for i in &hb.insts {
            uses_defs(i, &mut uses, &mut defs);
            if matches!(i, Inst::IBin(IOp::Div | IOp::Rem, ..)) {
                ok = false;
            }
            for d in &defs {
                let keep = nuse[d.0 as usize] != local_uses[d.0 as usize];
                map.push((*d, if keep { *d } else { VReg(u32::MAX) }));
            }
        }
        // a header def read in the header before it is (re)defined carries its value around
        // the loop (`n = n - 1`): the copy must update the real vreg, not a fresh one
        let mut seen_def: Vec<VReg> = Vec::new();
        for i in &hb.insts {
            uses_defs(i, &mut uses, &mut defs);
            if uses.iter().any(|u| map.iter().any(|(a, b)| a == u && a != b) && !seen_def.contains(u)) {
                ok = false;
            }
            seen_def.extend(defs.iter().cloned());
        }
        if !ok {
            continue;
        }
        for m in map.iter_mut() {
            if m.1 == VReg(u32::MAX) {
                let c = f.vregs[m.0 .0 as usize];
                m.1 = f.vreg(c);
            }
        }
        let look = |v: VReg| -> VReg {
            for (a, b) in &map {
                if *a == v {
                    return *b;
                }
            }
            v
        };
        let insts: Vec<Inst> = hb.insts.iter().map(|i| map_inst(i, &look, 0)).collect();
        let term = match hb.term {
            Term::Branch(c, t, e) => Term::Branch(look(c), t, e),
            t => t,
        };
        if branch_latch {
            let nb = f.blocks.len() as u32;
            f.blocks.push(Block { insts, pos: hb.pos.clone(), term, term_pos: hb.term_pos, count: hb.count });
            if let Term::Branch(_, _, ref mut e) = f.blocks[l].term {
                *e = nb;
            }
            added = true;
        } else {
            f.blocks[l].insts.extend(insts);
            f.blocks[l].pos.extend(hb.pos.iter().copied());
            f.blocks[l].term = term;
            f.blocks[l].term_pos = hb.term_pos;
        }
    }
    // the header copies follow their latches (a branch's false target is laid out next)
    if added {
        relayout(f);
    }
}

/// The stack slots' addresses and uses: `vs[v]` = (slot, offset) of a vreg holding a slot
/// address (single-definition SlotAddr, adds and moves of it; slot u32::MAX = none); a slot
/// escapes when its address goes anywhere but a load / store / copy base or a copy source.
pub struct SlotScan {
    pub vs: Vec<(u32, i64)>,
    pub esc: Vec<bool>,
    pub writes: Vec<u32>,
    pub reads: Vec<u32>,
    /// end of the furthest byte read
    pub rd_end: Vec<i64>,
    /// a copy into the slot's start from another slot's start: (block, inst, source slot)
    pub whole: Vec<Option<(usize, usize, u32)>>,
    /// A ret slot escaping only into one call's arguments: (block, inst). `esc` stays
    /// conservative for the other passes; forwarding counts this call as a write.
    pub ret_call: Vec<Option<(usize, usize)>>,
}

pub fn slot_scan(f: &Func) -> SlotScan {
    slot_scan_inner(f, false)
}

pub fn slot_scan_inner(f: &Func, terminal_calls: bool) -> SlotScan {
    let ns = f.slots.len();
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    // vreg -> (slot, Some(offset) when known); slot addresses and adds/moves of them
    const NONE: u32 = u32::MAX;
    let mut vs: Vec<(u32, i64)> = vec![(NONE, 0); nv];
    // a slot address in a vreg defined more than once is not followed: the slot escapes
    let mut multi = vec![false; ns];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, s) = i {
                if ndef[d.0 as usize] == 1 {
                    vs[d.0 as usize] = (*s, 0);
                } else {
                    multi[*s as usize] = true;
                }
            }
        }
    }
    let mut grew = true;
    while grew {
        grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, s, k) = match i {
                    Inst::Mov(d, s) => (*d, *s, 0),
                    Inst::IBinI(IOp::Add, _, d, s, k) => (*d, *s, *k),
                    _ => continue,
                };
                let (sl, o) = vs[s.0 as usize];
                if sl != NONE && ndef[d.0 as usize] == 1 && vs[d.0 as usize].0 == NONE {
                    vs[d.0 as usize] = (sl, o + k);
                    grew = true;
                }
            }
        }
    }
    // per slot: escaped, write count, the one whole copy (block, inst, source slot)
    // Poison a potential result-buffer call on any other escape or a second call.
    const BAD_CALL: (usize, usize) = (usize::MAX, usize::MAX);
    let mut ret_call = multi.iter().map(|&m| m.then_some(BAD_CALL)).collect::<Vec<_>>();
    let mut esc = multi;
    let mut writes = vec![0u32; ns];
    // end of the furthest byte read (a copy of fewer bytes than the slot holds -- its tail
    // padding -- still covers every read)
    let mut rd_end = vec![0i64; ns];
    let mut whole: Vec<Option<(usize, usize, u32)>> = vec![None; ns];
    let mut reads = vec![0u32; ns];
    let sl = |v: VReg| vs[v.0 as usize].0;
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            match i {
                Inst::Load(m, _, base, off) => {
                    if sl(*base) != NONE {
                        let e = &mut rd_end[sl(*base) as usize];
                        *e = (*e).max(vs[base.0 as usize].1 + *off as i64 + mem_size(*m) as i64);
                    }
                }
                Inst::VLoad(_, base, off) => {
                    if sl(*base) != NONE {
                        let e = &mut rd_end[sl(*base) as usize];
                        *e = (*e).max(vs[base.0 as usize].1 + *off as i64 + 16);
                    }
                }
                Inst::LoadX(_, _, base, x, _, _) | Inst::VLoadX(_, base, x, _, _) => {
                    if sl(*base) != NONE {
                        rd_end[sl(*base) as usize] = i64::MAX;
                    }
                    if sl(*x) != NONE {
                        esc[sl(*x) as usize] = true;
                        ret_call[sl(*x) as usize] = Some(BAD_CALL);
                    }
                }
                Inst::Store(_, base, _, src) | Inst::VStore(base, _, src) => {
                    if sl(*base) != NONE {
                        writes[sl(*base) as usize] += 1;
                    }
                    if sl(*src) != NONE {
                        esc[sl(*src) as usize] = true;
                        ret_call[sl(*src) as usize] = Some(BAD_CALL);
                    }
                }
                Inst::StoreX(_, base, x, _, _, src) | Inst::VStoreX(base, x, _, _, src) => {
                    if sl(*base) != NONE {
                        writes[sl(*base) as usize] += 1;
                    }
                    for v in [x, src] {
                        if sl(*v) != NONE {
                            esc[sl(*v) as usize] = true;
                            ret_call[sl(*v) as usize] = Some(BAD_CALL);
                        }
                    }
                }
                Inst::Copy(d, s, n) => {
                    if sl(*s) != NONE {
                        let e = &mut rd_end[sl(*s) as usize];
                        *e = (*e).max(vs[s.0 as usize].1 + *n as i64);
                    }
                    let ds = sl(*d);
                    if ds != NONE {
                        writes[ds as usize] += 1;
                        let ss = sl(*s);
                        if vs[d.0 as usize].1 == 0 && *n <= f.slots[ds as usize].size && ss != NONE && ss != ds && vs[s.0 as usize].1 == 0 && f.slots[ss as usize].size >= *n {
                            whole[ds as usize] = Some((bi, ii, ss));
                        }
                    }
                }
                // Vector promotion materializes the cells before entering a
                // terminal block; calls there may observe or retain the address.
                Inst::Call(..) if terminal_calls && matches!(b.term, Term::Unreachable) => {}
                Inst::Call(c, args, _) => {
                    for arg in args {
                        let s = sl(*arg);
                        if s != NONE {
                            esc[s as usize] = true;
                            let call = &mut ret_call[s as usize];
                            *call = Some(match *call {
                                None => (bi, ii),
                                Some(at) if at == (bi, ii) => at,
                                _ => BAD_CALL,
                            });
                        }
                    }
                    if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                        if sl(*v) != NONE {
                            esc[sl(*v) as usize] = true;
                            ret_call[sl(*v) as usize] = Some(BAD_CALL);
                        }
                    }
                }
                Inst::SlotAddr(..) => {}
                Inst::Mov(d, s) | Inst::IBinI(IOp::Add, _, d, s, _) if sl(*s) != NONE && sl(*d) == sl(*s) => {}
                _ => {
                    for_uses(i, |v| {
                        if sl(v) != NONE {
                            esc[sl(v) as usize] = true;
                            ret_call[sl(v) as usize] = Some(BAD_CALL);
                        }
                    });
                }
            }
        }
        term_uses(&b.term, &mut tu);
        for v in &tu {
            if sl(*v) != NONE {
                esc[sl(*v) as usize] = true;
                ret_call[sl(*v) as usize] = Some(BAD_CALL);
            }
        }
    }
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::LoadX(_, _, base, ..) | Inst::VLoadX(_, base, ..) | Inst::Copy(_, base, _) => {
                    if sl(*base) != NONE {
                        reads[sl(*base) as usize] += 1;
                    }
                }
                _ => {}
            }
        }
    }
    for (s, call) in ret_call.iter_mut().enumerate() {
        if !f.slots[s].ret || *call == Some(BAD_CALL) {
            *call = None;
        }
    }
    SlotScan { vs, esc, writes, reads, rd_end, whole, ret_call }
}
/// Installs (Some) or removes (None) sroa's no-capture oracle for this thread.
pub fn set_sroa_nocapture(q: Option<Box<dyn Fn(&Callee, usize) -> bool>>) {
    // (try_with: the JIT compiles on threads whose thread-locals are being destroyed)
    let _ = SROA_NOCAP.try_with(|c| *c.borrow_mut() = q);
}
