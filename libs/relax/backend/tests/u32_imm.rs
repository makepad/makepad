//! u32 operations with immediates above i32::MAX (masks with bit 31 set) take x64's 32-bit forms
//! with the constant's bit pattern, and a branch on `x & K` for such a mask is `test r32, imm32`;
//! results must equal Rust's u32 arithmetic for values with and without the high bits set.
#![cfg(all(target_arch = "x86_64", unix))]
use relax_backend::{exec::Code, rir::*, x64};

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(x) = (((x & 0x8000_0001) ^ 0xFFFF_0000) | 0x8000_0000) + 0x9000_0000 - 0xA000_0000) * 0x8000_0003,
/// plus 1 << 40 when x & 0x8000_0000 != 0 (a branch on the masked value)
fn build() -> Func {
    let u = U32;
    let w = IntTy { bits: 64, signed: false };
    let mut f = Func::new("u32_imm".into(), 0);
    let x = f.vreg(Cls::I);
    f.params = vec![x];
    f.rets = vec![Cls::I];
    let bl: Vec<u32> = (0..4).map(|_| f.block()).collect();
    let vs: Vec<VReg> = (0..8).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, bl[0], Inst::IBinI(IOp::And, u, vs[0], x, 0x8000_0001));
    push(&mut f, bl[0], Inst::IBinI(IOp::Xor, u, vs[1], vs[0], 0xFFFF_0000));
    push(&mut f, bl[0], Inst::IBinI(IOp::Or, u, vs[2], vs[1], 0x8000_0000));
    push(&mut f, bl[0], Inst::IBinI(IOp::Add, u, vs[3], vs[2], 0x9000_0000));
    push(&mut f, bl[0], Inst::IBinI(IOp::Sub, u, vs[4], vs[3], 0xA000_0000));
    push(&mut f, bl[0], Inst::IBinI(IOp::Mul, u, vs[5], vs[4], 0x8000_0003));
    let (t, c) = (f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, bl[0], Inst::IBinI(IOp::And, u, t, x, 0x8000_0000));
    push(&mut f, bl[0], Inst::ICmpI(Cond::Ne, false, c, t, 0));
    f.blocks[bl[0] as usize].term = Term::Branch(c, bl[1], bl[2]);
    // 1 << 40 is no imm32: a constant operand (the optimiser forms IBinI only for imm32 values;
    // without opt_regs nothing legalises a wider one)
    let (r, big) = (f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, bl[1], Inst::Iconst(big, 1 << 40));
    push(&mut f, bl[1], Inst::IBin(IOp::Add, w, r, vs[5], big));
    f.blocks[bl[1] as usize].term = Term::Jump(bl[3]);
    push(&mut f, bl[2], Inst::Mov(r, vs[5]));
    f.blocks[bl[2] as usize].term = Term::Jump(bl[3]);
    f.blocks[bl[3] as usize].term = Term::Ret(vec![r]);
    f.verify().unwrap();
    f
}

fn want(x: u32) -> u64 {
    let v = (((x & 0x8000_0001) ^ 0xFFFF_0000) | 0x8000_0000).wrapping_add(0x9000_0000).wrapping_sub(0xA000_0000).wrapping_mul(0x8000_0003);
    v as u64 + if x & 0x8000_0000 != 0 { 1 << 40 } else { 0 }
}

#[test]
fn u32_immediates_above_i32_max() {
    for opt in [false, true] {
        let env = x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: opt, syms: 0, sym_class: 0, near: false };
        let f = build();
        let c = x64::compile(&relax_session::Options::default(), &f, &env).unwrap();
        let code = Code::new(&c.code).unwrap();
        let run: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
        let mut s = 0x9e3779b97f4a7c15u64;
        for k in 0..4000u32 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let x = if k < 8 { [0, 1, 0x7FFF_FFFF, 0x8000_0000, 0x8000_0001, 0xFFFF_FFFF, 0xFFFF_0000, 0x1234_5678][k as usize] } else { s as u32 };
            assert_eq!(run(x as u64), want(x), "x = {:#x} opt_regs {}", x, opt);
        }
    }
}
