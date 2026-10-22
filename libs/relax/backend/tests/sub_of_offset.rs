//! simplify: `(x + k) - x` is k and `x - (x + k)` is -k (64-bit wrapping arithmetic): the length of
//! `data[i..i + 8]`, so `try_into::<[u8; 8]>` folds to Ok.
use relax_backend::passes;
use relax_backend::rir::*;

const U64: IntTy = IntTy { bits: 64, signed: false };
const U32: IntTy = IntTy { bits: 32, signed: false };

fn push(f: &mut Func, i: Inst) {
    f.blocks[0].insts.push(i);
    f.blocks[0].pos.push(0);
}

/// fn(x) { y = x + k (or x - k); ret <sub> } with `sub` built from (x, y)
fn build(add: IOp, k: i64, it: IntTy, sub: impl Fn(VReg, VReg) -> (VReg, VReg)) -> Func {
    let mut f = Func::new("subk".into(), 0);
    let x = f.vreg(Cls::I);
    f.params.push(x);
    f.rets.push(Cls::I);
    f.block();
    let (y, z) = (f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, Inst::IBinI(add, U64, y, x, k));
    let (a, b) = sub(x, y);
    push(&mut f, Inst::IBin(IOp::Sub, it, z, a, b));
    f.blocks[0].term = Term::Ret(vec![z]);
    f
}

fn ret_const(f: &Func) -> Option<i64> {
    let Term::Ret(ref v) = f.blocks[0].term else { return None };
    f.blocks[0].insts.iter().find_map(|i| match *i {
        Inst::Iconst(d, k) if d == v[0] => Some(k),
        _ => None,
    })
}

#[test]
fn a_sub_of_an_offset_of_the_same_value_is_the_offset() {
    let o = relax_session::Options::default();
    let cases: [(IOp, i64, bool, i64); 4] = [(IOp::Add, 8, false, 8), (IOp::Add, 8, true, -8), (IOp::Sub, 3, false, -3), (IOp::Sub, 3, true, 3)];
    for (op, k, swapped, want) in cases {
        let mut f = build(op, k, U64, |x, y| if swapped { (x, y) } else { (y, x) });
        passes::simplify(&o, &mut f, None, true, true);
        f.verify().unwrap();
        assert_eq!(ret_const(&f), Some(want), "{op:?} {k} swapped {swapped}: {}", f.dump());
    }
}

#[test]
fn a_narrow_sub_or_another_value_is_left() {
    let o = relax_session::Options::default();
    // 32-bit subtraction of 64-bit values: not folded here
    let mut f = build(IOp::Add, 8, U32, |x, y| (y, x));
    passes::simplify(&o, &mut f, None, true, true);
    assert_eq!(ret_const(&f), None, "{}", f.dump());
    // (x + 8) - (x + 8): not this rule (and x - x is another one)
    let mut f = build(IOp::Add, 8, U64, |_, y| (y, y));
    passes::simplify(&o, &mut f, None, true, true);
    assert_ne!(ret_const(&f), Some(8));
}
