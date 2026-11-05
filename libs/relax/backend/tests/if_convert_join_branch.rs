//! if_convert and a branch after the join: a value choice (`m = if a < b { a } else { b }`, a
//! diamond of copies) stays a branch only when that later branch can be threaded to a known
//! outcome on each arm -- its condition computed from the choice and constants (an Option's
//! discriminant test). A branch on arithmetic of the choice with other values (`m - p < 0`, a
//! float compare of a distance) cannot be threaded: the choice becomes a select (painted slicing's
//! `a.y.min(b.y)` stayed a branch that mispredicts half the time).
use relax_backend::passes;
use relax_backend::rir::*;

const I64: IntTy = IntTy { bits: 64, signed: true };

/// b0: c = a < b; branch c ? b1 : b2; b1: m = a; b2: m = b; b3: `join` (instructions reading m,
/// the last one defining the branch's condition); branch ? b4 : b5 (each returns m)
fn diamond(join: impl Fn(&mut Func, VReg, VReg, VReg) -> (Vec<Inst>, VReg)) -> Func {
    let mut f = Func::new("diamond".into(), 0);
    let (a, b, p) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b, p];
    f.rets = vec![Cls::I];
    let (c, m) = (f.vreg(Cls::I), f.vreg(Cls::I));
    let (insts, jc) = join(&mut f, m, p, a);
    for _ in 0..6 {
        f.block();
    }
    let set = |f: &mut Func, k: usize, insts: Vec<Inst>, term: Term| {
        f.blocks[k].pos = vec![0; insts.len()];
        f.blocks[k].insts = insts;
        f.blocks[k].term = term;
    };
    set(&mut f, 0, vec![Inst::ICmp(Cond::Lt, true, c, a, b)], Term::Branch(c, 1, 2));
    set(&mut f, 1, vec![Inst::Mov(m, a)], Term::Jump(3));
    set(&mut f, 2, vec![Inst::Mov(m, b)], Term::Jump(3));
    set(&mut f, 3, insts, Term::Branch(jc, 4, 5));
    set(&mut f, 4, vec![], Term::Ret(vec![m]));
    set(&mut f, 5, vec![], Term::Ret(vec![m]));
    f.verify().unwrap();
    f
}

fn converted(mut f: Func) -> bool {
    let changed = passes::if_convert(&mut f, true);
    f.verify().unwrap();
    let sel = f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Sel(..)));
    assert_eq!(changed, sel && matches!(f.blocks[0].term, Term::Jump(3)), "a conversion leaves a select and a jump to the join");
    sel
}

#[test]
fn a_choice_feeding_unthreadable_arithmetic_becomes_a_select() {
    // t = m - p; branch t < 0 (p is no constant)
    let f = diamond(|f, m, p, _| {
        let (t, jc) = (f.vreg(Cls::I), f.vreg(Cls::I));
        (vec![Inst::IBin(IOp::Sub, I64, t, m, p), Inst::ICmpI(Cond::Lt, true, jc, t, 0)], jc)
    });
    assert!(converted(f), "m - p < 0 cannot be threaded on the arms: the min is a select");
    // the nearest-segment walk's form: (m - p) as f64, squared, compared with another value
    let f = diamond(|f, m, p, a| {
        let [t, x, x2, y, jc] = [f.vreg(Cls::I), f.vreg(Cls::F64), f.vreg(Cls::F64), f.vreg(Cls::F64), f.vreg(Cls::I)];
        let ins = vec![
            Inst::IBin(IOp::Sub, I64, t, m, p),
            Inst::Conv(Conv::IntToF(I64, true), x, t),
            Inst::FBin(FOp::Mul, true, x2, x, x),
            Inst::Conv(Conv::IntToF(I64, true), y, a),
            Inst::FCmp(Cond::Gt, true, jc, x2, y),
        ];
        (ins, jc)
    });
    assert!(converted(f), "a float compare of a distance cannot be threaded: the min is a select");
}

#[test]
fn a_choice_a_threadable_branch_tests_stays_a_branch() {
    // branch m == 7 (a compare with a constant), and (m + 1) & 3 == 2 through constant operands
    let f = diamond(|f, m, _, _| {
        let jc = f.vreg(Cls::I);
        (vec![Inst::ICmpI(Cond::Eq, true, jc, m, 7)], jc)
    });
    assert!(!converted(f), "m == 7 is threadable on each arm: the branch stays");
    let f = diamond(|f, m, _, _| {
        let [one, s, k, t, jc] = [(); 5].map(|_| f.vreg(Cls::I));
        let ins = vec![
            Inst::Iconst(one, 1),
            Inst::IBin(IOp::Add, I64, s, m, one),
            Inst::IBinI(IOp::And, I64, t, s, 3),
            Inst::Iconst(k, 2),
            Inst::ICmp(Cond::Eq, true, jc, t, k),
        ];
        (ins, jc)
    });
    assert!(!converted(f), "(m + 1) & 3 == 2 with constant operands is threadable: the branch stays");
}
