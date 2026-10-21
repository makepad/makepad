//! Vector legalisation: a lane-typed vector op the target has no instruction sequence for
//! (`native` says which it has) becomes per-lane scalar code -- each lane extracted, the scalar
//! op of the same meaning, the lane inserted back. Runs before register allocation; ops the
//! target encodes are untouched, so code without such ops is unchanged.

use crate::rir::*;

/// Whether `op` is an op this pass can expand (the portable lane-wise and lane-moving ops;
/// target-specific ones such as the SHA rounds are never expanded).
pub fn expandable(op: VX) -> bool {
    match op {
        VX::Add(_) | VX::Sub(_) | VX::MulLo(_) | VX::MinS(_) | VX::MaxS(_) | VX::MinU(_) | VX::MaxU(_) => true,
        VX::AddSatU(_) | VX::SubSatU(_) | VX::AddSatS(_) | VX::SubSatS(_) => true,
        VX::ICmp(..) | VX::UCmp(..) | VX::CmpEq(_) | VX::CmpGtS(_) => true,
        VX::ShlV(_) | VX::ShrSV(_) | VX::ShrUV(_) | VX::ShlI(..) | VX::ShrUI(..) | VX::ShrSI(..) => true,
        VX::Neg(_) | VX::Abs(_) | VX::And | VX::Or | VX::Xor | VX::AndNot => true,
        VX::FAdd(_, NanBits::Host) | VX::FSub(_, NanBits::Host) | VX::FMul(_, NanBits::Host) | VX::FDiv(_, NanBits::Host) => true,
        VX::FMinLt(_) | VX::FMaxGt(_) | VX::FMinNum(_) | VX::FMaxNum(_) | VX::FCmp(..) => true,
        VX::FNeg(_) | VX::FAbs(_) | VX::FSqrt(_, NanBits::Host) | VX::FRound(..) | VX::FToI(_) | VX::IToF(_) => true,
        VX::Narrow(_) | VX::FCvtLo | VX::FCvtNarrow | VX::MoveMask(_) => true,
        _ => false,
    }
}

/// Whether `legalize` would change `f`.
pub fn needed(f: &Func, native: &dyn Fn(VX) -> bool, native_shuffle: bool) -> bool {
    f.blocks.iter().any(|b| {
        b.insts.iter().any(|i| match i {
            Inst::VX(op, ..) => expandable(*op) && !native(*op),
            Inst::VShuffle(..) => !native_shuffle,
            _ => false,
        })
    })
}

/// Expands every `Inst::VX` that is expandable and not `native`, and every `Inst::VShuffle`
/// when `native_shuffle` is false. Returns whether anything changed.
pub fn legalize(f: &mut Func, native: &dyn Fn(VX) -> bool, native_shuffle: bool) -> bool {
    let wants = |i: &Inst| match i {
        Inst::VX(op, ..) => expandable(*op) && !native(*op),
        Inst::VShuffle(..) => !native_shuffle,
        _ => false,
    };
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        if !f.blocks[bi].insts.iter().any(|i| wants(i)) {
            continue;
        }
        changed = true;
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let old_pos = std::mem::take(&mut f.blocks[bi].pos);
        let mut out = Vec::with_capacity(old.len() + 64);
        let mut pos = Vec::with_capacity(old.len() + 64);
        for (k, i) in old.into_iter().enumerate() {
            let p = old_pos.get(k).copied().unwrap_or(0);
            if !wants(&i) {
                out.push(i);
                pos.push(p);
                continue;
            }
            let start = out.len();
            let mut e = Ex { f: &mut *f, out: &mut out };
            match i {
                Inst::VX(op, d, a, b) => e.vx(op, d, a, b),
                Inst::VShuffle(d, a, b, idx) => e.shuffle(d, a, b, idx),
                _ => unreachable!(),
            }
            pos.resize(pos.len() + (out.len() - start), p);
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
    changed
}

/// Fused multiply-adds with a negated operand as a negation then the plain fused op: on x86-64
/// the vfnmadd/vfmsub forms negate the product or addend after the multiply, so a NaN operand
/// keeps its sign, while RIR's `neg_prod`/`neg_add` negate `a`/`c` themselves (a NaN's sign
/// flips, as `(-a).mul_add(b, c)` and NEON's fmsub/fnmadd do). Returns whether anything changed.
pub fn expand_fma_negations(f: &mut Func) -> bool {
    let wants = |i: &Inst| matches!(i, Inst::FMulAdd(np, na, ..) | Inst::VFma(np, na, ..) if *np || *na);
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        if !f.blocks[bi].insts.iter().any(|i| wants(i)) {
            continue;
        }
        changed = true;
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let old_pos = std::mem::take(&mut f.blocks[bi].pos);
        let mut out = Vec::with_capacity(old.len() + 8);
        let mut pos = Vec::with_capacity(old.len() + 8);
        for (k, i) in old.into_iter().enumerate() {
            let p = old_pos.get(k).copied().unwrap_or(0);
            match i {
                Inst::FMulAdd(np, na, w, d, mut a, b, mut c) if np || na => {
                    let cls = if w { Cls::F64 } else { Cls::F32 };
                    if np {
                        let t = f.vreg(cls);
                        out.push(Inst::FUnary(FUn::Neg, w, t, a));
                        pos.push(p);
                        a = t;
                    }
                    if na {
                        let t = f.vreg(cls);
                        out.push(Inst::FUnary(FUn::Neg, w, t, c));
                        pos.push(p);
                        c = t;
                    }
                    out.push(Inst::FMulAdd(false, false, w, d, a, b, c));
                }
                Inst::VFma(np, na, d, mut a, b, mut c) if np || na => {
                    let cls = f.vregs[d.0 as usize];
                    if np {
                        let t = f.vreg(cls);
                        out.push(Inst::VX(VX::FNeg(VW::S), t, a, a));
                        pos.push(p);
                        a = t;
                    }
                    if na {
                        let t = f.vreg(cls);
                        out.push(Inst::VX(VX::FNeg(VW::S), t, c, c));
                        pos.push(p);
                        c = t;
                    }
                    out.push(Inst::VFma(false, false, d, a, b, c));
                }
                i => out.push(i),
            }
            pos.push(p);
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
    changed
}

struct Ex<'a> {
    f: &'a mut Func,
    out: &'a mut Vec<Inst>,
}

fn lanes(w: VW) -> u8 {
    (128 / w.bits()) as u8
}

fn half(w: VW) -> VW {
    match w {
        VW::H => VW::B,
        VW::S => VW::H,
        VW::D => VW::S,
        VW::B => panic!("no narrower lane than a byte"),
    }
}

fn ity(w: VW, signed: bool) -> IntTy {
    IntTy { bits: w.bits() as u8, signed }
}

impl Ex<'_> {
    fn i(&mut self) -> VReg {
        self.f.vreg(Cls::I)
    }
    fn k(&mut self, v: i64) -> VReg {
        let d = self.i();
        self.out.push(Inst::Iconst(d, v));
        d
    }
    /// lane `k` of `v`, zero-extended
    fn get(&mut self, w: VW, v: VReg, k: u8) -> VReg {
        let d = self.i();
        self.out.push(Inst::VX(VX::Extract(w, k), d, v, v));
        d
    }
    /// lane `k` of `v`, sign-extended
    fn get_s(&mut self, w: VW, v: VReg, k: u8) -> VReg {
        let x = self.get(w, v, k);
        if w == VW::D {
            return x;
        }
        let d = self.i();
        self.out.push(Inst::Conv(Conv::IntToInt(ity(w, true)), d, x));
        d
    }
    /// `acc` with lane `k` = the low bits of `s`; returns the new vector
    fn put(&mut self, w: VW, acc: VReg, k: u8, s: VReg) -> VReg {
        let d = self.f.vreg(Cls::V128);
        self.out.push(Inst::VX(VX::Insert(w, k), d, acc, s));
        d
    }
    fn bin(&mut self, op: IOp, t: IntTy, a: VReg, b: VReg) -> VReg {
        let d = self.i();
        self.out.push(Inst::IBin(op, t, d, a, b));
        d
    }
    fn cmp(&mut self, c: Cond, signed: bool, a: VReg, b: VReg) -> VReg {
        let d = self.i();
        self.out.push(Inst::ICmp(c, signed, d, a, b));
        d
    }
    fn sel(&mut self, c: VReg, a: VReg, b: VReg) -> VReg {
        let d = self.i();
        self.out.push(Inst::Sel(d, c, a, b));
        d
    }
    /// 0/1 -> all ones / zero
    fn mask(&mut self, c: VReg) -> VReg {
        let d = self.i();
        self.out.push(Inst::INeg(I64, d, c));
        d
    }
    fn to_f(&mut self, w: VW, x: VReg) -> VReg {
        let d = self.f.vreg(if w == VW::D { Cls::F64 } else { Cls::F32 });
        self.out.push(Inst::Conv(Conv::BitsToF(w == VW::D), d, x));
        d
    }
    fn to_bits(&mut self, w: VW, x: VReg) -> VReg {
        let d = self.i();
        self.out.push(Inst::Conv(Conv::FToBits(w == VW::D), d, x));
        d
    }
    fn fresh_f(&mut self, w: VW) -> VReg {
        self.f.vreg(if w == VW::D { Cls::F64 } else { Cls::F32 })
    }

    /// The scalar result of lane-wise `op` on lanes `x`, `y` of width `w` (zero-extended
    /// lane bits in, lane bits in the low bits out).
    fn lane(&mut self, op: VX, w: VW, x: VReg, y: VReg, k: u8, a: VReg, b: VReg) -> VReg {
        let u = ity(w, false);
        let s = ity(w, true);
        let bits = w.bits() as i64;
        match op {
            VX::Add(_) => self.bin(IOp::Add, u, x, y),
            VX::Sub(_) => self.bin(IOp::Sub, u, x, y),
            VX::MulLo(_) => self.bin(IOp::Mul, u, x, y),
            VX::And => self.bin(IOp::And, U64, x, y),
            VX::Or => self.bin(IOp::Or, U64, x, y),
            VX::Xor => self.bin(IOp::Xor, U64, x, y),
            VX::AndNot => {
                let n = self.i();
                self.out.push(Inst::INot(U64, n, y));
                self.bin(IOp::And, U64, x, n)
            }
            VX::MinU(_) | VX::MaxU(_) => {
                let c = self.cmp(Cond::Lt, false, x, y);
                if matches!(op, VX::MinU(_)) { self.sel(c, x, y) } else { self.sel(c, y, x) }
            }
            VX::MinS(_) | VX::MaxS(_) => {
                let (xs, ys) = (self.get_s(w, a, k), self.get_s(w, b, k));
                let c = self.cmp(Cond::Lt, true, xs, ys);
                if matches!(op, VX::MinS(_)) { self.sel(c, x, y) } else { self.sel(c, y, x) }
            }
            VX::AddSatU(_) => {
                let r = self.bin(IOp::Add, u, x, y);
                let c = self.cmp(Cond::Lt, false, r, x);
                let m = self.k(-1);
                self.sel(c, m, r)
            }
            VX::SubSatU(_) => {
                let r = self.bin(IOp::Sub, u, x, y);
                let c = self.cmp(Cond::Lt, false, x, y);
                let z = self.k(0);
                self.sel(c, z, r)
            }
            VX::AddSatS(_) | VX::SubSatS(_) => {
                // overflow iff the operands' signs make it possible and the result's sign
                // differs from x's: add: x, y same sign; sub: x, y different signs
                let (xs, ys) = (self.get_s(w, a, k), self.get_s(w, b, k));
                let add = matches!(op, VX::AddSatS(_));
                let r = self.bin(if add { IOp::Add } else { IOp::Sub }, s, xs, ys);
                let t = self.bin(IOp::Xor, I64, xs, r);
                let v = self.bin(IOp::Xor, I64, xs, ys);
                let ov = if add {
                    let nv = self.i();
                    self.out.push(Inst::INot(I64, nv, v));
                    self.bin(IOp::And, I64, t, nv)
                } else {
                    self.bin(IOp::And, I64, t, v)
                };
                let z = self.k(0);
                let c = self.cmp(Cond::Lt, true, ov, z);
                // saturate toward x's sign: x < 0 -> MIN else MAX
                let neg = self.cmp(Cond::Lt, true, xs, z);
                let min = self.k(i64::MIN >> (64 - bits));
                let max = self.k(i64::MAX >> (64 - bits));
                let sat = self.sel(neg, min, max);
                self.sel(c, sat, r)
            }
            VX::ICmp(..) | VX::UCmp(..) | VX::CmpEq(_) | VX::CmpGtS(_) => {
                let (c, signed) = match op {
                    VX::ICmp(c, _) => (c, true),
                    VX::UCmp(c, _) => (c, false),
                    VX::CmpEq(_) => (Cond::Eq, false),
                    _ => (Cond::Gt, true),
                };
                let r = if signed {
                    let (xs, ys) = (self.get_s(w, a, k), self.get_s(w, b, k));
                    self.cmp(c, true, xs, ys)
                } else {
                    self.cmp(c, false, x, y)
                };
                self.mask(r)
            }
            VX::ShlV(_) => self.bin(IOp::Shl, u, x, y),
            VX::ShrUV(_) => self.bin(IOp::Shr, u, x, y),
            VX::ShrSV(_) => {
                let xs = self.get_s(w, a, k);
                self.bin(IOp::Shr, s, xs, y)
            }
            VX::ShlI(_, n) | VX::ShrUI(_, n) | VX::ShrSI(_, n) => {
                // immediate shifts of the whole width give 0 (arithmetic right: the sign)
                let n = n as i64;
                match op {
                    VX::ShlI(..) | VX::ShrUI(..) if n >= bits => self.k(0),
                    VX::ShlI(..) => { let c = self.k(n); self.bin(IOp::Shl, u, x, c) }
                    VX::ShrUI(..) => { let c = self.k(n); self.bin(IOp::Shr, u, x, c) }
                    _ => {
                        let xs = self.get_s(w, a, k);
                        let c = self.k(n.min(bits - 1));
                        self.bin(IOp::Shr, s, xs, c)
                    }
                }
            }
            VX::Neg(_) => {
                let d = self.i();
                self.out.push(Inst::INeg(u, d, x));
                d
            }
            VX::Abs(_) => {
                let xs = self.get_s(w, a, k);
                let z = self.k(0);
                let c = self.cmp(Cond::Lt, true, xs, z);
                let n = self.i();
                self.out.push(Inst::INeg(u, n, x));
                self.sel(c, n, x)
            }
            VX::FAdd(..) | VX::FSub(..) | VX::FMul(..) | VX::FDiv(..) | VX::FMinLt(_) | VX::FMaxGt(_) | VX::FMinNum(_) | VX::FMaxNum(_) => {
                let fop = match op {
                    VX::FAdd(..) => FOp::Add,
                    VX::FSub(..) => FOp::Sub,
                    VX::FMul(..) => FOp::Mul,
                    VX::FDiv(..) => FOp::Div,
                    VX::FMinLt(_) => FOp::MinLt,
                    VX::FMaxGt(_) => FOp::MaxGt,
                    VX::FMinNum(_) => FOp::Min,
                    _ => FOp::Max,
                };
                let (fx, fy) = (self.to_f(w, x), self.to_f(w, y));
                let r = self.fresh_f(w);
                self.out.push(Inst::FBin(fop, w == VW::D, r, fx, fy));
                self.to_bits(w, r)
            }
            VX::FCmp(c, _) => {
                let (fx, fy) = (self.to_f(w, x), self.to_f(w, y));
                let r = self.i();
                self.out.push(Inst::FCmp(c, w == VW::D, r, fx, fy));
                self.mask(r)
            }
            VX::FNeg(_) | VX::FAbs(_) | VX::FSqrt(..) | VX::FRound(..) => {
                let un = match op {
                    VX::FNeg(_) => FUn::Neg,
                    VX::FAbs(_) => FUn::Abs,
                    VX::FSqrt(..) => FUn::Sqrt,
                    VX::FRound(_, RoundTo::Floor) => FUn::Floor,
                    VX::FRound(_, RoundTo::Ceil) => FUn::Ceil,
                    VX::FRound(_, RoundTo::Trunc) => FUn::Trunc,
                    _ => FUn::RoundAway,
                };
                let fx = self.to_f(w, x);
                let r = self.fresh_f(w);
                self.out.push(Inst::FUnary(un, w == VW::D, r, fx));
                self.to_bits(w, r)
            }
            VX::FToI(_) => {
                let fx = self.to_f(w, x);
                let r = self.i();
                self.out.push(Inst::Conv(Conv::FToInt(w == VW::D, s), r, fx));
                r
            }
            VX::IToF(_) => {
                let xs = self.get_s(w, a, k);
                let r = self.fresh_f(w);
                self.out.push(Inst::Conv(Conv::IntToF(s, w == VW::D), r, xs));
                self.to_bits(w, r)
            }
            _ => unreachable!("not lane-wise: {:?}", op),
        }
    }

    fn vx(&mut self, op: VX, d: VReg, a: VReg, b: VReg) {
        let w = match op {
            VX::And | VX::Or | VX::Xor | VX::AndNot => VW::D,
            VX::Add(w) | VX::Sub(w) | VX::MulLo(w) | VX::MinS(w) | VX::MaxS(w) | VX::MinU(w) | VX::MaxU(w) => w,
            VX::AddSatU(w) | VX::SubSatU(w) | VX::AddSatS(w) | VX::SubSatS(w) => w,
            VX::ICmp(_, w) | VX::UCmp(_, w) | VX::CmpEq(w) | VX::CmpGtS(w) => w,
            VX::ShlV(w) | VX::ShrSV(w) | VX::ShrUV(w) | VX::ShlI(w, _) | VX::ShrUI(w, _) | VX::ShrSI(w, _) => w,
            VX::Neg(w) | VX::Abs(w) => w,
            VX::FAdd(w, _) | VX::FSub(w, _) | VX::FMul(w, _) | VX::FDiv(w, _) => w,
            VX::FMinLt(w) | VX::FMaxGt(w) | VX::FMinNum(w) | VX::FMaxNum(w) | VX::FCmp(_, w) => w,
            VX::FNeg(w) | VX::FAbs(w) | VX::FSqrt(w, _) | VX::FRound(w, _) | VX::FToI(w) | VX::IToF(w) => w,
            VX::Narrow(w) => {
                let h = half(w);
                let n = lanes(w);
                let mut acc = a;
                for j in 0..2 * n {
                    let src = if j < n { a } else { b };
                    let x = self.get(w, src, j % n);
                    acc = self.put(h, acc, j, x);
                }
                self.out.push(Inst::Mov(d, acc));
                return;
            }
            VX::FCvtLo => {
                let mut acc = a;
                for j in 0..2 {
                    let x = self.get(VW::S, a, j);
                    let f = self.to_f(VW::S, x);
                    let g = self.f.vreg(Cls::F64);
                    self.out.push(Inst::Conv(Conv::F32ToF64, g, f));
                    let bits = self.to_bits(VW::D, g);
                    acc = self.put(VW::D, acc, j, bits);
                }
                self.out.push(Inst::Mov(d, acc));
                return;
            }
            VX::FCvtNarrow => {
                let mut acc = a;
                for j in 0..4 {
                    let x = self.get(VW::D, if j < 2 { a } else { b }, j % 2);
                    let f = self.to_f(VW::D, x);
                    let g = self.f.vreg(Cls::F32);
                    self.out.push(Inst::Conv(Conv::F64ToF32, g, f));
                    let bits = self.to_bits(VW::S, g);
                    acc = self.put(VW::S, acc, j, bits);
                }
                self.out.push(Inst::Mov(d, acc));
                return;
            }
            VX::MoveMask(w) => {
                let mut r = self.k(0);
                for j in 0..lanes(w) {
                    let x = self.get(w, a, j);
                    let sh = self.k(w.bits() as i64 - 1);
                    let t = self.bin(IOp::Shr, U64, x, sh);
                    let jj = self.k(j as i64);
                    let t = self.bin(IOp::Shl, U64, t, jj);
                    r = self.bin(IOp::Or, U64, r, t);
                }
                self.out.push(Inst::Mov(d, r));
                return;
            }
            _ => unreachable!("not expandable: {:?}", op),
        };
        let unary = op.arity() == 1;
        let mut acc = a;
        for k in 0..lanes(w) {
            let x = self.get(w, a, k);
            let y = if unary { x } else { self.get(w, b, k) };
            let r = self.lane(op, w, x, y, k, a, if unary { a } else { b });
            acc = self.put(w, acc, k, r);
        }
        self.out.push(Inst::Mov(d, acc));
    }

    fn shuffle(&mut self, d: VReg, a: VReg, b: VReg, idx: [u8; 16]) {
        let mut acc = a;
        let mut vals = Vec::with_capacity(16);
        for &i in &idx {
            vals.push(match i {
                0..=15 => self.get(VW::B, a, i),
                16..=31 => self.get(VW::B, b, i - 16),
                _ => self.k(0),
            });
        }
        for (j, v) in vals.into_iter().enumerate() {
            acc = self.put(VW::B, acc, j as u8, v);
        }
        self.out.push(Inst::Mov(d, acc));
    }
}
