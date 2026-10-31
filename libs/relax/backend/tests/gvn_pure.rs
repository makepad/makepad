//! `passes::gvn_pure`: a pure value repeated where an equal one dominates it becomes a copy of
//! it, without local CSE's bounded table; constants, loads, values of another result class,
//! multi-definition values and repeats in blocks the first one does not dominate stay.
use relax_backend::passes;
use relax_backend::rir::*;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

fn movs(f: &Func) -> Vec<(VReg, VReg)> {
    f.blocks.iter().flat_map(|b| b.insts.iter()).filter_map(|i| if let Inst::Mov(d, s) = i { Some((*d, *s)) } else { None }).collect()
}

#[test]
fn repeats_past_the_bounded_table_merge() {
    // more distinct values than local CSE keeps, then each of them again
    let mut f = Func::new("gvn_many".into(), 0);
    let src = f.vreg(Cls::I);
    f.params = vec![src];
    f.block();
    let v = f.vreg(Cls::V128);
    push(&mut f, 0, Inst::VLoad(v, src, 0));
    let mut first = Vec::new();
    for round in 0..2 {
        for k in 0..100i64 {
            let d = f.vreg(Cls::I);
            push(&mut f, 0, Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: true }, d, src, k));
            if round == 0 {
                first.push(d);
            }
        }
    }
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    assert!(passes::gvn_pure(&mut f));
    let m = movs(&f);
    assert_eq!(m.len(), 100);
    for (k, (_, s)) in m.iter().enumerate() {
        assert_eq!(*s, first[k]);
    }
}

#[test]
fn dominance_classes_constants_and_multi_defs() {
    let mut f = Func::new("gvn_shape".into(), 0);
    let (src, c) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![src, c];
    let b: Vec<u32> = (0..4).map(|_| f.block()).collect();
    let v = f.vreg(Cls::V128);
    let (e0, e_f, k0) = (f.vreg(Cls::I), f.vreg(Cls::F32), f.vreg(Cls::I));
    push(&mut f, b[0], Inst::VLoad(v, src, 0));
    push(&mut f, b[0], Inst::VExtract(e0, v, 1));
    push(&mut f, b[0], Inst::VExtract(e_f, v, 1)); // another class: stays
    push(&mut f, b[0], Inst::Iconst(k0, 7));
    f.blocks[0].term = Term::Branch(c, b[1], b[2]);
    // b1, b2: siblings, each repeats the extract (dominated by b0: merged) and the constant (stays)
    let (e1, k1, s1) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b[1], Inst::VExtract(e1, v, 1));
    push(&mut f, b[1], Inst::Iconst(k1, 7));
    push(&mut f, b[1], Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: true }, s1, src, 3));
    f.blocks[1].term = Term::Jump(b[3]);
    let (e2, s2) = (f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b[2], Inst::VExtract(e2, v, 1));
    // the sibling's value is not available here: stays
    push(&mut f, b[2], Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: true }, s2, src, 3));
    f.blocks[2].term = Term::Jump(b[3]);
    // b3: a multi-def variable's operation repeated: stays
    let (x, y1, y2) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b[3], Inst::Iconst(x, 1));
    push(&mut f, b[3], Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: true }, y1, x, 1));
    push(&mut f, b[3], Inst::Iconst(x, 2));
    push(&mut f, b[3], Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: true }, y2, x, 1));
    f.blocks[3].term = Term::Ret(vec![]);
    f.verify().unwrap();
    assert!(passes::gvn_pure(&mut f));
    let mut m: Vec<(u32, u32)> = movs(&f).iter().map(|(d, s)| (d.0, s.0)).collect();
    m.sort();
    assert_eq!(m, vec![(e1.0, e0.0), (e2.0, e0.0)]);
}
