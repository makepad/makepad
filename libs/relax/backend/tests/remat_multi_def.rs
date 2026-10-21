//! A constant built on several paths into one vreg (every definition the same `Iconst`) is
//! rematerialised at its uses like a single-definition constant instead of living in a spill slot;
//! the function must still compute the same values with many other values live around it.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(a, b, c): 24 values a+1..a+24 stay live across a branch on c that sets k = 3 on both paths;
/// then ((a << k) ^ (b << k)) ^ all the sums.
fn build() -> Func {
    let mut f = Func::new("remat_multi".into(), 0);
    let (a, b, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b, c];
    f.rets = vec![Cls::I];
    let bl: Vec<u32> = (0..4).map(|_| f.block()).collect();
    let ts: Vec<VReg> = (1..=24).map(|_| f.vreg(Cls::I)).collect();
    for (i, t) in ts.iter().enumerate() {
        push(&mut f, bl[0], Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, *t, a, i as i64 + 1));
    }
    f.blocks[bl[0] as usize].term = Term::Branch(c, bl[1], bl[2]);
    let k = f.vreg(Cls::I);
    push(&mut f, bl[1], Inst::Iconst(k, 3));
    f.blocks[bl[1] as usize].term = Term::Jump(bl[3]);
    push(&mut f, bl[2], Inst::Iconst(k, 3));
    f.blocks[bl[2] as usize].term = Term::Jump(bl[3]);
    let (sa, sb, mut acc) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let u = IntTy { bits: 64, signed: false };
    push(&mut f, bl[3], Inst::IBin(IOp::Shl, u, sa, a, k));
    push(&mut f, bl[3], Inst::IBin(IOp::Shl, u, sb, b, k));
    push(&mut f, bl[3], Inst::IBin(IOp::Xor, u, acc, sa, sb));
    for t in &ts {
        let n = f.vreg(Cls::I);
        push(&mut f, bl[3], Inst::IBin(IOp::Xor, u, n, acc, *t));
        acc = n;
    }
    f.blocks[bl[3] as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    f
}

#[test]
fn multi_def_constant_under_pressure() {
    let code = Code::new(&compile(&build())).unwrap();
    let run: extern "C" fn(u64, u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    let mut s = 0x9e3779b97f4a7c15u64;
    for _ in 0..2000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let (a, b, c) = (s, s.rotate_left(17) ^ 0x1234, s & 1);
        let mut want = (a << 3) ^ (b << 3);
        for i in 1..=24u64 {
            want ^= a.wrapping_add(i);
        }
        assert_eq!(run(a, b, c), want);
    }
}
