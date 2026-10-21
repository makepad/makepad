//! Loop-invariant reassociation (passes::reassociate_invariants, run by licm): `t1 = a + i;
//! t2 = t1 + c` in a loop with a and c invariant becomes `t1 = a + c` (hoisted) and `t2 = t1 + i`;
//! the result must not change, also for a narrow wrapping type.
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

/// f(n, a, c) = sum over i < n of ((a + i) + c) in `it` (accumulated in u64)
fn build(it: IntTy) -> Func {
    let w = IntTy { bits: 64, signed: false };
    let mut f = Func::new("reassoc".into(), 0);
    let (n, a, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, a, c];
    f.rets = vec![Cls::I];
    let v: Vec<VReg> = (0..8).map(|_| f.vreg(Cls::I)).collect();
    let (s, i, t1, t2) = (v[0], v[1], v[2], v[3]);
    let b: Vec<u32> = (0..4).map(|_| f.block()).collect();
    push(&mut f, b[0], Inst::Iconst(s, 0));
    push(&mut f, b[0], Inst::Iconst(i, 0));
    f.blocks[b[0] as usize].term = Term::Jump(b[1]);
    push(&mut f, b[1], Inst::ICmp(Cond::Lt, false, v[4], i, n));
    f.blocks[b[1] as usize].term = Term::Branch(v[4], b[2], b[3]);
    push(&mut f, b[2], Inst::IBin(IOp::Add, it, t1, a, i));
    push(&mut f, b[2], Inst::IBin(IOp::Add, it, t2, t1, c));
    push(&mut f, b[2], Inst::IBin(IOp::Add, w, s, s, t2));
    push(&mut f, b[2], Inst::IBinI(IOp::Add, w, i, i, 1));
    f.blocks[b[2] as usize].term = Term::Jump(b[1]);
    f.blocks[b[3] as usize].term = Term::Ret(vec![s]);
    f.verify().unwrap();
    f
}

#[test]
fn invariant_halves_hoist() {
    for it in [IntTy { bits: 64, signed: false }, IntTy { bits: 8, signed: false }] {
        let mut f = build(it);
        assert!(relax_backend::passes::licm(&relax_session::Options::default(), &mut f));
        f.verify().unwrap();
        // the loop body keeps one add of the index: t1 = a + c moved to the preheader
        let in_pre = f.blocks[0].insts.iter().any(|i| matches!(i, Inst::IBin(IOp::Add, _, _, _, _)));
        assert!(in_pre, "{}", f.dump());
        let code = Code::new(&compile(&f)).unwrap();
        let run: extern "C" fn(u64, u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
        for (n, a, c) in [(0u64, 5u64, 7u64), (10, 250, 3), (300, 0xffff_fff0, 0x40)] {
            let mut want = 0u64;
            for i in 0..n {
                let x = if it.bits == 8 { (a.wrapping_add(i).wrapping_add(c)) & 0xff } else { a.wrapping_add(i).wrapping_add(c) };
                want = want.wrapping_add(x);
            }
            assert_eq!(run(n, a, c), want, "{:?} n {} a {} c {}", it, n, a, c);
        }
    }
}
