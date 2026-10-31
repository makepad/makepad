//! stores_for_copies: a copy out of a temporary slot that the same block just stored becomes
//! stores of those values at the destination; anything that makes the bytes uncertain keeps it.
use relax_backend::rir::*;
use relax_codegen::opt;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(1);
}

/// b0: t = slot 0 (48 bytes); dst = param; 3 f32 lanes + a vector + two i32 at t; copy dst <- t, 48
fn vertex() -> (Func, VReg, VReg, VReg, VReg) {
    let mut f = Func::new("vx".into(), 0);
    let dst = f.vreg(Cls::I);
    let x = f.vreg(Cls::F32);
    let v = f.vreg(Cls::V128);
    let k = f.vreg(Cls::I);
    f.params.push(dst);
    f.params.push(x);
    f.params.push(v);
    f.params.push(k);
    let s = f.slot(48, 16);
    let t = f.vreg(Cls::I);
    f.block();
    push(&mut f, 0, Inst::SlotAddr(t, s));
    push(&mut f, 0, Inst::Store(Mem::F32, t, 0, x));
    push(&mut f, 0, Inst::Store(Mem::F32, t, 4, x));
    push(&mut f, 0, Inst::Store(Mem::F32, t, 8, x));
    push(&mut f, 0, Inst::Store(Mem::F32, t, 12, x));
    push(&mut f, 0, Inst::VStore(t, 16, v));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), t, 32, k));
    push(&mut f, 0, Inst::Store(Mem::Int(4, true), t, 40, k));
    push(&mut f, 0, Inst::Store(Mem::Int(4, true), t, 44, k));
    push(&mut f, 0, Inst::Copy(dst, t, 48));
    f.blocks[0].term = Term::Ret(vec![]);
    (f, t, dst, x, k)
}

fn dst_stores(f: &Func, dst: VReg) -> Vec<i32> {
    f.blocks[0].insts.iter().filter_map(|i| match i {
        Inst::Store(_, b, o, _) | Inst::VStore(b, o, _) if *b == dst => Some(*o),
        _ => None,
    }).collect()
}

#[test]
fn a_built_temporary_is_stored_at_the_destination() {
    let (mut f, _, dst, _, _) = vertex();
    assert!(opt::stores_for_copies(&mut f));
    f.verify().unwrap();
    assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Copy(..))));
    assert_eq!(dst_stores(&f, dst), vec![0, 4, 8, 12, 16, 32, 40, 44]);
    assert_eq!(f.blocks[0].insts.len(), f.blocks[0].pos.len());
    // the temporary's stores are dead now
    assert!(opt::dead_slot_stores(&mut f));
    assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Store(_, b, ..) | Inst::VStore(b, ..) if *b != dst)));
}

#[test]
fn a_later_store_shadows_an_earlier_one() {
    let (mut f, t, dst, x, _) = vertex();
    let y = f.vreg(Cls::F32);
    f.params.push(y);
    // [t + 4] = y after the first round of stores: the copy takes y there
    let at = f.blocks[0].insts.len() - 1;
    f.blocks[0].insts.insert(at, Inst::Store(Mem::F32, t, 4, y));
    f.blocks[0].pos.insert(at, 1);
    assert!(opt::stores_for_copies(&mut f));
    let v4: Vec<VReg> = f.blocks[0].insts.iter().filter_map(|i| match i {
        Inst::Store(_, b, 4, v) if *b == dst => Some(*v),
        _ => None,
    }).collect();
    assert_eq!(v4, vec![y]);
    assert!(f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Store(_, b, 0, v) if *b == dst && *v == x)));
}

#[test]
fn a_byte_not_stored_in_the_block_keeps_the_copy() {
    let (mut f, ..) = vertex();
    // drop the store of bytes 40..44
    f.blocks[0].insts.remove(7);
    f.blocks[0].pos.remove(7);
    assert!(!opt::stores_for_copies(&mut f));
}

#[test]
fn a_partly_overwriting_store_keeps_the_copy() {
    // a 4-byte store into the upper half of the 8-byte one at 32: bytes 32..36 would need half
    // of the earlier store
    let (mut f, t, _, _, k) = vertex();
    let at = f.blocks[0].insts.len() - 1;
    f.blocks[0].insts.insert(at, Inst::Store(Mem::Int(4, false), t, 36, k));
    f.blocks[0].pos.insert(at, 1);
    assert!(!opt::stores_for_copies(&mut f));
}

#[test]
fn a_value_redefined_before_the_copy_keeps_it() {
    let (mut f, _, _, _, k) = vertex();
    let at = f.blocks[0].insts.len() - 1;
    f.blocks[0].insts.insert(at, Inst::Iconst(k, 5));
    f.blocks[0].pos.insert(at, 1);
    assert!(!opt::stores_for_copies(&mut f));
}

#[test]
fn an_escaping_or_indexed_temporary_keeps_the_copy() {
    let (mut f, t, ..) = vertex();
    push(&mut f, 0, Inst::Call(Callee::Fn(9), vec![t], vec![]));
    assert!(!opt::stores_for_copies(&mut f));
    let (mut g, t, _, _, k) = vertex();
    let at = g.blocks[0].insts.len() - 1;
    g.blocks[0].insts.insert(at, Inst::StoreX(Mem::Int(4, false), t, k, 2, 0, k));
    g.blocks[0].pos.insert(at, 1);
    assert!(!opt::stores_for_copies(&mut g));
}

#[test]
fn a_copy_of_only_part_of_a_store_keeps_it() {
    // copy dst <- t + 4, 8 bytes: the vector store at 16 is outside, the 8-byte store at 32 too,
    // bytes 4..12 come from two f32 stores
    let (mut f, t, dst, ..) = vertex();
    let t4 = f.vreg(Cls::I);
    let n = f.blocks[0].insts.len();
    f.blocks[0].insts[n - 1] = Inst::Copy(dst, t4, 8);
    f.blocks[0].insts.insert(n - 1, Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, t4, t, 4));
    f.blocks[0].pos.insert(n - 1, 1);
    assert!(opt::stores_for_copies(&mut f));
    assert_eq!(dst_stores(&f, dst), vec![0, 4]);
    // 8 bytes from t + 12: [12, 20) cuts the vector store at 16..32: kept
    let (mut g, t, dst, ..) = vertex();
    let t12 = g.vreg(Cls::I);
    let n = g.blocks[0].insts.len();
    g.blocks[0].insts[n - 1] = Inst::Copy(dst, t12, 8);
    g.blocks[0].insts.insert(n - 1, Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, t12, t, 12));
    g.blocks[0].pos.insert(n - 1, 1);
    assert!(!opt::stores_for_copies(&mut g));
}
