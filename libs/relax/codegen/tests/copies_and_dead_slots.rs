//! reuse_copied_variables (a temporary copied into a variable dies at the copy) and
//! dead_slot_stores (slot stores no later read sees).
use relax_backend::rir::*;
use relax_codegen::opt;

const U64: IntTy = IntTy { bits: 64, signed: false };
fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(1);
}
fn uses_of(f: &Func, v: VReg) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            relax_backend::regalloc::for_uses(i, |u| {
                if u == v {
                    out.push((bi, k));
                }
            });
        }
        let mut tu = Vec::new();
        relax_backend::regalloc::term_uses(&b.term, &mut tu);
        if tu.contains(&v) {
            out.push((bi, usize::MAX));
        }
    }
    out
}

/// loop { t = x >> n; x = t; if t & 1 { y = t + 1 } ; x = x + 3 }: the read of t in b2 sees x == t
/// (no definition of x between); b2 is reached only from b1.
fn bit_reader() -> (Func, VReg, VReg) {
    let mut f = Func::new("br".into(), 0);
    let (x, n, t, c, y, a) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.push(x);
    f.params.push(n);
    f.rets.push(Cls::I);
    for _ in 0..5 {
        f.block();
    }
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::IBin(IOp::Shr, U64, t, x, n));
    push(&mut f, 1, Inst::Mov(x, t));
    push(&mut f, 1, Inst::IBinI(IOp::And, U64, a, t, 1));
    push(&mut f, 1, Inst::ICmpI(Cond::Ne, false, c, a, 0));
    f.blocks[1].term = Term::Branch(c, 2, 3);
    push(&mut f, 2, Inst::IBinI(IOp::Add, U64, y, t, 1));
    f.blocks[2].term = Term::Ret(vec![y]);
    push(&mut f, 3, Inst::IBinI(IOp::Add, U64, x, x, 3));
    push(&mut f, 3, Inst::ICmpI(Cond::Eq, false, c, x, 0));
    f.blocks[3].term = Term::Branch(c, 4, 1);
    f.blocks[4].term = Term::Ret(vec![x]);
    (f, x, t)
}

#[test]
fn reads_of_a_copied_temporary_read_the_variable() {
    let (mut f, x, t) = bit_reader();
    assert!(opt::reuse_copied_variables(&relax_session::Options::default(), &mut f));
    f.verify().unwrap();
    // t is read only by the copy now: it dies there
    assert_eq!(uses_of(&f, t), vec![(1, 1)]);
    assert!(matches!(f.blocks[1].insts[2], Inst::IBinI(IOp::And, _, _, v, 1) if v == x));
    assert!(matches!(f.blocks[2].insts[0], Inst::IBinI(IOp::Add, _, _, v, 1) if v == x));
}

#[test]
fn a_read_after_the_variable_changed_keeps_the_temporary() {
    let (mut f, x, t) = bit_reader();
    // b3 redefines x before reading t: t stays live past the copy, so nothing is rewritten
    let y2 = f.vreg(Cls::I);
    f.blocks[3].insts.insert(1, Inst::IBinI(IOp::Add, U64, y2, t, 5));
    f.blocks[3].pos.insert(1, 1);
    f.blocks[3].insts[2] = Inst::ICmpI(Cond::Eq, false, VReg(3), y2, 0);
    let before = f.dump();
    assert!(!opt::reuse_copied_variables(&relax_session::Options::default(), &mut f));
    assert_eq!(before, f.dump());
    let _ = x;
}

#[test]
fn a_join_where_one_path_redefined_the_variable_keeps_the_temporary() {
    // b1: t = x >> n; x = t; branch b2 / b3; b2: x = x + 1; b3 (from both): ret t
    let mut f = Func::new("join".into(), 0);
    let (x, n, t, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.push(x);
    f.params.push(n);
    f.rets.push(Cls::I);
    for _ in 0..3 {
        f.block();
    }
    push(&mut f, 0, Inst::IBin(IOp::Shr, U64, t, x, n));
    push(&mut f, 0, Inst::Mov(x, t));
    push(&mut f, 0, Inst::ICmpI(Cond::Ne, false, c, n, 0));
    f.blocks[0].term = Term::Branch(c, 1, 2);
    push(&mut f, 1, Inst::IBinI(IOp::Add, U64, x, x, 1));
    f.blocks[1].term = Term::Jump(2);
    f.blocks[2].term = Term::Ret(vec![t]);
    let before = f.dump();
    assert!(!opt::reuse_copied_variables(&relax_session::Options::default(), &mut f));
    assert_eq!(before, f.dump());
}

/// b0: tmp[0..16] = 0 (byte stores); branch b1 / b2 (b2: exit without reading tmp);
/// b1: tmp[0..8], tmp[8..16] = loads of src; b3: dst = tmp's two words
fn zero_then_copy() -> Func {
    let mut f = Func::new("zc".into(), 0);
    let (src, dst, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.push(src);
    f.params.push(dst);
    f.params.push(c);
    let s = f.slot(16, 8);
    let (p, z, a, b, ra, rb) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    for _ in 0..4 {
        f.block();
    }
    push(&mut f, 0, Inst::SlotAddr(p, s));
    push(&mut f, 0, Inst::Iconst(z, 0));
    for k in 0..16 {
        push(&mut f, 0, Inst::Store(Mem::Int(1, false), p, k, z));
    }
    f.blocks[0].term = Term::Branch(c, 1, 2);
    push(&mut f, 1, Inst::Load(Mem::Int(8, false), a, src, 0));
    push(&mut f, 1, Inst::Load(Mem::Int(8, false), b, src, 8));
    push(&mut f, 1, Inst::Store(Mem::Int(8, false), p, 0, a));
    push(&mut f, 1, Inst::Store(Mem::Int(8, false), p, 8, b));
    f.blocks[1].term = Term::Jump(3);
    f.blocks[2].term = Term::Ret(vec![]);
    push(&mut f, 3, Inst::Load(Mem::Int(8, false), ra, p, 0));
    push(&mut f, 3, Inst::Load(Mem::Int(8, false), rb, p, 8));
    push(&mut f, 3, Inst::Store(Mem::Int(8, false), dst, 0, ra));
    push(&mut f, 3, Inst::Store(Mem::Int(8, false), dst, 8, rb));
    f.blocks[3].term = Term::Ret(vec![]);
    f
}

#[test]
fn zero_fill_overwritten_before_any_read_goes() {
    let mut f = zero_then_copy();
    assert!(opt::dead_slot_stores(&mut f));
    f.verify().unwrap();
    assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Store(..))));
    assert_eq!(f.blocks[1].insts.len(), 4);
    assert_eq!(f.blocks[3].insts.len(), 4);
}

#[test]
fn zero_fill_read_on_some_path_stays() {
    // b2 reads byte 3 of tmp: that byte's zero store stays, the others go
    let mut f = zero_then_copy();
    let r = f.vreg(Cls::I);
    push(&mut f, 2, Inst::Load(Mem::Int(1, false), r, VReg(3), 3));
    f.blocks[2].term = Term::Ret(vec![r]);
    f.rets.push(Cls::I);
    assert!(opt::dead_slot_stores(&mut f));
    let st: Vec<i32> = f.blocks[0].insts.iter().filter_map(|i| if let Inst::Store(_, _, o, _) = i { Some(*o) } else { None }).collect();
    assert_eq!(st, vec![3]);
}

#[test]
fn a_partial_overwrite_keeps_the_rest() {
    // b1 writes only tmp[0..8]: the zeros at 8..16 are read in b3
    let mut f = zero_then_copy();
    f.blocks[1].insts.remove(3);
    f.blocks[1].pos.remove(3);
    assert!(opt::dead_slot_stores(&mut f));
    let st: Vec<i32> = f.blocks[0].insts.iter().filter_map(|i| if let Inst::Store(_, _, o, _) = i { Some(*o) } else { None }).collect();
    assert_eq!(st, (8..16).collect::<Vec<i32>>());
}

#[test]
fn an_escaping_slot_keeps_its_stores() {
    let mut f = zero_then_copy();
    push(&mut f, 2, Inst::Call(Callee::Fn(9), vec![VReg(3)], vec![]));
    assert!(!opt::dead_slot_stores(&mut f));
}

#[test]
fn stores_in_a_loop_read_by_the_next_iteration_stay() {
    // b0: p = slot; jump b1. b1: r = load p; store p <- r + 1; branch b1 / b2. b2: ret
    let mut f = Func::new("lp".into(), 0);
    let c = f.vreg(Cls::I);
    f.params.push(c);
    let s = f.slot(8, 8);
    let (p, r, r1) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    for _ in 0..3 {
        f.block();
    }
    push(&mut f, 0, Inst::SlotAddr(p, s));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), p, 0, c));
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::Load(Mem::Int(8, false), r, p, 0));
    push(&mut f, 1, Inst::IBinI(IOp::Add, U64, r1, r, 1));
    push(&mut f, 1, Inst::Store(Mem::Int(8, false), p, 0, r1));
    f.blocks[1].term = Term::Branch(r1, 1, 2);
    f.blocks[2].term = Term::Ret(vec![]);
    // the entry store and the loop store are both read by the loop's load
    assert!(!opt::dead_slot_stores(&mut f));
    assert_eq!(f.blocks[0].insts.len(), 2);
    assert_eq!(f.blocks[1].insts.len(), 3);
}

#[test]
fn a_mask_whose_result_is_read_only_in_its_bits_is_a_copy() {
    // loop variable n (u32): n = n - (e & 0xff); read only as `n as u8` and `(n >> 3) & 7`
    let mut f = Func::new("bd".into(), 0);
    let (n, p) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.push(n);
    f.params.push(p);
    f.rets.push(Cls::I);
    let (e, m, t, a, b, c, q) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    const U32: IntTy = IntTy { bits: 32, signed: false };
    const U8: IntTy = IntTy { bits: 8, signed: false };
    for _ in 0..3 {
        f.block();
    }
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::Load(Mem::Int(4, false), e, p, 0));
    push(&mut f, 1, Inst::IBinI(IOp::And, U32, m, e, 255));
    push(&mut f, 1, Inst::IBin(IOp::Sub, U32, t, n, m));
    push(&mut f, 1, Inst::Mov(n, t));
    push(&mut f, 1, Inst::IBinI(IOp::Shr, U32, a, n, 3));
    push(&mut f, 1, Inst::IBinI(IOp::And, U32, b, a, 7));
    push(&mut f, 1, Inst::Conv(Conv::IntToInt(U8), c, n));
    push(&mut f, 1, Inst::Store(Mem::Int(1, false), p, 8, c));
    push(&mut f, 1, Inst::ICmpI(Cond::Eq, false, q, b, 0));
    f.blocks[1].term = Term::Branch(q, 2, 1);
    f.blocks[2].term = Term::Ret(vec![b]);
    assert!(opt::drop_undemanded_masks(&relax_session::Options::default(), &mut f));
    assert!(matches!(f.blocks[1].insts[1], Inst::Mov(d, s) if d == m && s == e));
    // the byte cast feeds only a byte store: a copy too
    assert!(matches!(f.blocks[1].insts[6], Inst::Mov(d, s) if d == c && s == n));
    // b is returned (all bits): its mask stays
    assert!(matches!(f.blocks[1].insts[5], Inst::IBinI(IOp::And, ..)));
    // a full read of n (returned) keeps the mask
    let mut g = f.clone();
    g.blocks[1].insts[1] = Inst::IBinI(IOp::And, U32, m, e, 255);
    g.blocks[2].term = Term::Ret(vec![n]);
    g.blocks[1].insts[6] = Inst::Conv(Conv::IntToInt(U8), c, n);
    opt::drop_undemanded_masks(&relax_session::Options::default(), &mut g);
    assert!(matches!(g.blocks[1].insts[1], Inst::IBinI(IOp::And, ..)));
}

#[test]
fn an_access_through_a_constant_offset_reads_the_base() {
    // p1 = p + 1; a = [p1 - 1]; p = p + 4 (after): a reads [p + 0]
    let mut f = Func::new("ob".into(), 0);
    let p = f.vreg(Cls::I);
    f.params.push(p);
    f.rets.push(Cls::I);
    let (p1, a) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.block();
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, p1, p, 1));
    push(&mut f, 0, Inst::Load(Mem::Int(1, false), a, p1, -1));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, p, p, 4));
    f.blocks[0].term = Term::Ret(vec![a]);
    assert!(opt::fold_offset_bases(&relax_session::Options::default(), &mut f));
    assert!(matches!(f.blocks[0].insts[1], Inst::Load(_, _, b, 0) if b == p));
    // the base redefined between: unchanged
    let mut g = Func::new("ob2".into(), 0);
    for v in &f.vregs {
        g.vreg(*v);
    }
    g.params = f.params.clone();
    g.rets = f.rets.clone();
    g.block();
    push(&mut g, 0, Inst::IBinI(IOp::Add, U64, p1, p, 1));
    push(&mut g, 0, Inst::IBinI(IOp::Add, U64, p, p, 4));
    push(&mut g, 0, Inst::Load(Mem::Int(1, false), a, p1, -1));
    g.blocks[0].term = Term::Ret(vec![a]);
    assert!(!opt::fold_offset_bases(&relax_session::Options::default(), &mut g));
}
