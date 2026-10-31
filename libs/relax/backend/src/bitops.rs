//! Bit operations the target has no instruction for, expanded into shifts and masks; byte-mask vector constants.

use crate::regalloc::for_defs;
use crate::rir::*;

/// Rewrites `IBit` ops a target lacks (`keep(op)` false) into shift/mask sequences
/// (branch-free SWAR in 64-bit arithmetic; bit reverse keeps a byte swap when kept).
pub fn expand_bit_ops(f: &mut Func, keep: &dyn Fn(BitOp) -> bool) {
    let u = IntTy { bits: 64, signed: false };
    for bi in 0..f.blocks.len() {
        if !f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::IBit(o, _, _) if !keep(*o))) {
            continue;
        }
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let old_pos = std::mem::take(&mut f.blocks[bi].pos);
        let mut out: Vec<Inst> = Vec::with_capacity(old.len() + 32);
        let mut pos: Vec<u64> = Vec::with_capacity(old.len() + 32);
        for (k, i) in old.into_iter().enumerate() {
            let p = old_pos.get(k).copied().unwrap_or(0);
            let (o, d, x) = match i {
                Inst::IBit(o, d, x) if !keep(o) => (o, d, x),
                other => {
                    out.push(other);
                    pos.push(p);
                    continue;
                }
            };
            let mut e = BitEx { f: &mut *f, out: &mut out, u };
            let r = match o {
                BitOp::Popcnt => e.popcnt(x),
                BitOp::Clz => {
                    let mut y = x;
                    for k in [1, 2, 4, 8, 16, 32] {
                        let s = e.binc(IOp::Shr, y, k);
                        y = e.bin(IOp::Or, y, s);
                    }
                    let pc = e.popcnt(y);
                    let n = e.k(64);
                    e.bin(IOp::Sub, n, pc)
                }
                BitOp::Ctz => {
                    let xm1 = e.binc(IOp::Sub, x, 1);
                    let nx = e.binc(IOp::Xor, x, -1);
                    let y = e.bin(IOp::And, nx, xm1);
                    e.popcnt(y)
                }
                BitOp::Bswap => {
                    let y = e.swap(x, 8, 0x00ff_00ff_00ff_00ff);
                    let y = e.swap(y, 16, 0x0000_ffff_0000_ffff);
                    let a = e.binc(IOp::Shr, y, 32);
                    let b = e.binc(IOp::Shl, y, 32);
                    e.bin(IOp::Or, a, b)
                }
                BitOp::Rev => {
                    let mut y = x;
                    for (k, m) in [(1, 0x5555_5555_5555_5555u64), (2, 0x3333_3333_3333_3333), (4, 0x0f0f_0f0f_0f0f_0f0f)] {
                        y = e.swap(y, k, m);
                    }
                    if keep(BitOp::Bswap) {
                        let r = e.f.vreg(Cls::I);
                        e.out.push(Inst::IBit(BitOp::Bswap, r, y));
                        r
                    } else {
                        let y = e.swap(y, 8, 0x00ff_00ff_00ff_00ff);
                        let y = e.swap(y, 16, 0x0000_ffff_0000_ffff);
                        let a = e.binc(IOp::Shr, y, 32);
                        let b = e.binc(IOp::Shl, y, 32);
                        e.bin(IOp::Or, a, b)
                    }
                }
            };
            out.push(Inst::Mov(d, r));
            while pos.len() < out.len() {
                pos.push(p);
            }
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
}

pub struct BitEx<'a> {
    f: &'a mut Func,
    out: &'a mut Vec<Inst>,
    u: IntTy,
}

impl BitEx<'_> {
    fn k(&mut self, v: i64) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.out.push(Inst::Iconst(d, v));
        d
    }
    fn bin(&mut self, op: IOp, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.out.push(Inst::IBin(op, self.u, d, a, b));
        d
    }
    fn binc(&mut self, op: IOp, a: VReg, k: i64) -> VReg {
        let c = self.k(k);
        self.bin(op, a, c)
    }
    /// ((x >> k) & m) | ((x & m) << k)
    fn swap(&mut self, x: VReg, k: i64, m: u64) -> VReg {
        let a = self.binc(IOp::Shr, x, k);
        let a = self.binc(IOp::And, a, m as i64);
        let b = self.binc(IOp::And, x, m as i64);
        let b = self.binc(IOp::Shl, b, k);
        self.bin(IOp::Or, a, b)
    }
    fn popcnt(&mut self, x: VReg) -> VReg {
        let s1 = self.binc(IOp::Shr, x, 1);
        let s1 = self.binc(IOp::And, s1, 0x5555_5555_5555_5555);
        let a = self.bin(IOp::Sub, x, s1);
        let lo = self.binc(IOp::And, a, 0x3333_3333_3333_3333);
        let s2 = self.binc(IOp::Shr, a, 2);
        let hi = self.binc(IOp::And, s2, 0x3333_3333_3333_3333);
        let b = self.bin(IOp::Add, lo, hi);
        let s4 = self.binc(IOp::Shr, b, 4);
        let c = self.bin(IOp::Add, b, s4);
        let c = self.binc(IOp::And, c, 0x0f0f_0f0f_0f0f_0f0f);
        let c = self.binc(IOp::Mul, c, 0x0101_0101_0101_0101);
        self.binc(IOp::Shr, c, 56)
    }
}

pub fn ext(v: i64, it: IntTy) -> i64 {
    crate::rir::extend_const(v, it)
}

/// Values whose every byte is a canonical boolean mask (0x00 or 0xff).
pub fn vector_byte_masks(f: &Func) -> Vec<bool> {
    // A byte blend is equivalent to a bitwise select only when each byte is
    // entirely zero or one. Require this of every definition, including params.
    let mut masks = vec![true; f.vregs.len()];
    for p in &f.params {
        masks[p.0 as usize] = false;
    }
    for b in &f.blocks {
        for i in &b.insts {
            let mask = match i {
                Inst::VX(VX::CmpEq(_) | VX::CmpGtS(_) | VX::ICmp(..) | VX::FCmp(..), ..) => true,
                Inst::VConst(_, k) => k.to_le_bytes().iter().all(|b| *b == 0 || *b == 255),
                _ => false,
            };
            if !mask {
                for_defs(i, |d| masks[d.0 as usize] = false);
            }
        }
    }
    masks
}
