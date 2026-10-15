use relax_codegen::{opt, rir::*};

const U64: IntTy = IntTy { bits: 64, signed: false };
fn ro(_: usize, s: Sym, n: u32) -> opt::RoRead {
    assert_eq!(n, 0);
    if s.id == 7 && s.off >= 0 && s.off <= 128 {
        opt::RoRead::Range(128 - s.off as u64)
    } else {
        opt::RoRead::No
    }
}
fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(123);
}
// Entry initializes a table; a different block indexes it with a runtime parameter.
fn table() -> Func {
    let mut f = Func::new("table".into(), 0);
    for _ in 0..6 {
        f.vreg(Cls::I);
    }
    f.params.push(VReg(0));
    f.rets.push(Cls::I);
    f.slot(128, 8);
    f.block();
    f.block();
    push(&mut f, 0, Inst::SlotAddr(VReg(1), 0));
    push(&mut f, 0, Inst::Addr(VReg(2), Sym::new(7)));
    push(&mut f, 0, Inst::Copy(VReg(1), VReg(2), 128));
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::IBin(IOp::Add, U64, VReg(3), VReg(1), VReg(0)));
    push(&mut f, 1, Inst::Load(Mem::Int(8, false), VReg(4), VReg(3), 0));
    f.blocks[1].term = Term::Ret(vec![VReg(4)]);
    f
}
fn reject(mut f: Func) {
    let before = f.dump();
    assert!(!opt::forward_const_slots(&mut f, Some(opt::RoReader::new(ro, 0))));
    assert_eq!(before, f.dump());
}
#[test]
fn indexed_reads_use_immutable_symbol() {
    let mut f = table();
    // An unrelated call cannot modify the immutable source or the private destination.
    push(&mut f, 0, Inst::Call(Callee::Fn(42), vec![], vec![]));
    assert!(opt::forward_const_slots(&mut f, Some(opt::RoReader::new(ro, 0))));
    assert!(matches!(f.blocks[0].insts[0], Inst::Addr(VReg(1), Sym { id: 7, off: 0 })));
    assert!(!f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Copy(..))));
    f.verify().unwrap();
}
#[test]
fn reject_mutable_source_partial_copy_and_source_overrun() {
    let mut f = table();
    f.blocks[0].insts[1] = Inst::Addr(VReg(2), Sym::new(8));
    reject(f);
    let mut f = table();
    f.blocks[0].insts[2] = Inst::Copy(VReg(1), VReg(2), 64);
    reject(f);
    let mut f = table();
    f.blocks[0].insts[1] = Inst::Addr(VReg(2), Sym::new(7).plus(8));
    reject(f);
}
#[test]
fn reject_writes_and_escapes_through_indexed_pointer() {
    for i in [
        Inst::Store(Mem::Int(8, false), VReg(3), 0, VReg(0)),
        Inst::Copy(VReg(3), VReg(2), 8),
        Inst::Call(Callee::Fn(42), vec![VReg(3)], vec![]),
        Inst::Store(Mem::Int(8, false), VReg(0), 0, VReg(3)),
        Inst::ICmp(Cond::Eq, false, VReg(5), VReg(3), VReg(0)),
        Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Volatile, VReg(5), VReg(3)),
    ] {
        let mut f = table();
        push(&mut f, 1, i);
        reject(f);
    }
    let mut f = table();
    f.blocks[1].term = Term::Ret(vec![VReg(3)]);
    reject(f);
}
#[test]
fn reject_multiple_writes_and_pointer_redefinitions() {
    let mut f = table();
    push(&mut f, 1, Inst::Copy(VReg(1), VReg(2), 128));
    reject(f);
    let mut f = table();
    push(&mut f, 1, Inst::Mov(VReg(3), VReg(0)));
    reject(f);
    let mut f = table();
    push(&mut f, 1, Inst::SlotAddr(VReg(1), 0));
    reject(f);
}
#[test]
fn initializer_must_dominate_reads() {
    let mut f = table();
    // Move the copy after an early read (source remains an Addr, defined before the copy).
    f.blocks[0].insts = vec![
        Inst::SlotAddr(VReg(1), 0),
        Inst::Addr(VReg(2), Sym::new(7)),
        Inst::Load(Mem::Int(8, false), VReg(5), VReg(1), 0),
        Inst::Copy(VReg(1), VReg(2), 128),
    ];
    f.blocks[0].pos = vec![123; 4];
    reject(f);
    let mut f = table();
    f.block();
    let copy = f.blocks[0].insts.pop().unwrap();
    f.blocks[0].pos.pop();
    push(&mut f, 2, copy);
    f.blocks[0].term = Term::Branch(VReg(0), 1, 2);
    f.blocks[2].term = Term::Jump(1);
    reject(f);
}
#[test]
fn loop_reads_and_copy_out_are_safe() {
    let mut f = table();
    f.blocks[1].term = Term::Branch(VReg(0), 1, 2);
    f.block();
    f.blocks[2].term = Term::Ret(vec![VReg(4)]);
    push(&mut f, 1, Inst::Copy(VReg(0), VReg(3), 8));
    assert!(opt::forward_const_slots(&mut f, Some(opt::RoReader::new(ro, 0))));
    f.verify().unwrap();
}
