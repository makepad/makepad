//! fold_small_sets: compares and selects decided by the few values their operands can hold
//! (tuple `lt` through `Ordering` as an i8, tested against ControlFlow's niche 254).
use relax_backend::rir::*;
use relax_codegen::opt::fold_small_sets;

fn push(f: &mut Func, i: Inst) {
    f.blocks[0].insts.push(i);
    f.blocks[0].pos.push(1);
}

/// f(a, b): o = a < b ? -1 : ((a == b) ^ 1); t = (o as i8) & 255; returns (t != k, t <= 1, o)
fn ordering(k: i64) -> (Func, [VReg; 3]) {
    let mut f = Func::new("ordering".into(), 0);
    let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b];
    f.rets = vec![Cls::I, Cls::I, Cls::I];
    f.block();
    let v: Vec<VReg> = (0..9).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, Inst::ICmp(Cond::Lt, false, v[0], a, b));
    push(&mut f, Inst::ICmp(Cond::Eq, false, v[1], a, b));
    push(&mut f, Inst::IBinI(IOp::Xor, U64, v[2], v[1], 1));
    push(&mut f, Inst::Iconst(v[3], -1));
    push(&mut f, Inst::Sel(v[4], v[0], v[3], v[2]));
    push(&mut f, Inst::Conv(Conv::IntToInt(IntTy { bits: 8, signed: true }), v[5], v[4]));
    push(&mut f, Inst::IBinI(IOp::And, U64, v[6], v[5], 255));
    push(&mut f, Inst::ICmpI(Cond::Ne, false, v[7], v[6], k));
    push(&mut f, Inst::ICmpI(Cond::Le, false, v[8], v[6], 1));
    f.blocks[0].term = Term::Ret(vec![v[7], v[8], v[4]]);
    f.verify().unwrap();
    (f, [v[7], v[8], v[4]])
}

fn def(f: &Func, v: VReg) -> Inst {
    f.blocks[0].insts.iter().find(|i| {
        let mut d = false;
        relax_backend::regalloc::for_defs(i, |x| d |= x == v);
        d
    }).cloned().unwrap()
}

#[test]
fn a_niche_no_ordering_value_has_is_never_equal() {
    let (mut f, [ne, le, _]) = ordering(254);
    assert!(fold_small_sets(&mut f));
    f.verify().unwrap();
    assert!(matches!(def(&f, ne), Inst::Iconst(_, 1)), "{}", f.dump());
    // t is 255, 0 or 1: `t <= 1` is not decided
    assert!(matches!(def(&f, le), Inst::ICmpI(..)));
}

#[test]
fn a_value_in_the_set_is_left_alone() {
    // 255 is Less: the compare depends on the operands
    let (mut f, [ne, ..]) = ordering(255);
    fold_small_sets(&mut f);
    assert!(matches!(def(&f, ne), Inst::ICmpI(..)));
}

#[test]
fn a_select_on_a_decided_condition_is_a_copy() {
    let (mut f, [ne, _, o]) = ordering(254);
    // s = (t != 254) ? a : b
    let (a, b) = (f.params[0], f.params[1]);
    let s = f.vreg(Cls::I);
    push(&mut f, Inst::Sel(s, ne, a, b));
    f.blocks[0].term = Term::Ret(vec![s, o, o]);
    assert!(fold_small_sets(&mut f));
    assert!(matches!(def(&f, s), Inst::Mov(_, x) if x == a), "{}", f.dump());
}

#[test]
fn a_constant_defined_on_two_paths_is_one_value() {
    // the -1 arm is a vreg assigned -1 twice (one constant per path after jump threading)
    let (mut f, [ne, ..]) = ordering(254);
    let k = f.blocks[0].insts.iter().position(|i| matches!(i, Inst::Iconst(_, -1))).unwrap();
    let Inst::Iconst(m1, _) = f.blocks[0].insts[k] else { unreachable!() };
    f.blocks[0].insts.insert(k, Inst::Iconst(m1, -1));
    f.blocks[0].pos.insert(k, 1);
    assert!(fold_small_sets(&mut f));
    assert!(matches!(def(&f, ne), Inst::Iconst(_, 1)), "{}", f.dump());
}
