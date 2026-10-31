//! form_bzhi: `x & ((1 << n) - 1)` and `x & !(-1 << n)` on 64-bit values become IOp::Bzhi when
//! the shift and mask are read only there and n is the same value at both.
use relax_backend::rir::*;
use relax_codegen::opt::form_bzhi;

fn push(f: &mut Func, i: Inst) {
    f.blocks[0].insts.push(i);
    f.blocks[0].pos.push(1);
}

fn options() -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.cpu.bmi2 = true;
    o
}

/// ret x & ((1 << n) - 1) [`not`: x & !(-1 << n)]; `redef`: n reassigned between shift and mask
fn idiom(not: bool, redef: bool) -> (Func, VReg) {
    let mut f = Func::new("mask".into(), 0);
    let (x, n0) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![x, n0];
    f.rets = vec![Cls::I];
    f.block();
    let (n, one, s, m, d) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, Inst::Mov(n, n0));
    push(&mut f, Inst::Iconst(one, if not { -1 } else { 1 }));
    push(&mut f, Inst::IBin(IOp::Shl, U64, s, one, n));
    if not {
        push(&mut f, Inst::IBinI(IOp::Xor, U64, m, s, -1));
    } else {
        push(&mut f, Inst::IBinI(IOp::Sub, U64, m, s, 1));
    }
    if redef {
        push(&mut f, Inst::IBinI(IOp::Add, U64, n, n, 1));
    }
    push(&mut f, Inst::IBin(IOp::And, U64, d, m, x));
    f.blocks[0].term = Term::Ret(vec![d, ]);
    f.verify().unwrap();
    (f, d)
}

fn bzhi_of(f: &Func, d: VReg) -> bool {
    f.blocks[0].insts.iter().any(|i| matches!(i, Inst::IBin(IOp::Bzhi, _, dd, _, _) if *dd == d))
}

#[test]
fn the_mask_idiom_becomes_bzhi() {
    for not in [false, true] {
        let (mut f, d) = idiom(not, false);
        assert!(form_bzhi(&options(), &mut f));
        f.verify().unwrap();
        assert!(bzhi_of(&f, d), "{}", f.dump());
    }
}

#[test]
fn a_count_changed_in_between_is_left_alone() {
    let (mut f, d) = idiom(false, true);
    assert!(!form_bzhi(&options(), &mut f));
    assert!(!bzhi_of(&f, d));
}

#[test]
fn a_mask_read_twice_is_left_alone() {
    let (mut f, d) = idiom(false, false);
    // the mask is also returned
    let m = match f.blocks[0].insts[3] { Inst::IBinI(_, _, m, _, _) => m, _ => unreachable!() };
    f.rets.push(Cls::I);
    f.blocks[0].term = Term::Ret(vec![d, m]);
    assert!(!form_bzhi(&options(), &mut f));
}
