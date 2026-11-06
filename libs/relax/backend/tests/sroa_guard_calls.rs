//! A live session's guarded inline copy (`Func::patch_flag`, `guard_calls`): the copy runs while
//! its callee's patch flag reads 0, else the site calls the slot. That slow call is handed the
//! call's result buffer and argument copy, as the call was before it was inlined. It runs only after
//! a patch, so sroa keeps both slots in registers on the copy's path, in a loop too, and flushes
//! and reloads them around the slow call alone. Without that, every value an inlined `sub(a, p)`
//! returned stayed in stack memory, and live sessions ran a mesh's winding number 5x slower than
//! `makepad test`. The same call as a real call in the loop keeps the slots in memory (a flush
//! and reload per iteration costs more than it saves).
use relax_backend::passes;
use relax_backend::rir::*;

const U64: IntTy = IntTy { bits: 64, signed: false };
const SLOW_POS: u64 = 5;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// `fn(n) -> u64`: `acc = 0; i = 0; while i < n { a = i; r = step(a); acc += r; i += 1 } acc`,
/// `step` (Fn(7), result buffer first) inlined as `r = a + 1` behind its patch flag, its call in
/// the slow block. `guarded`: that call is the copy's slow call (`guard_calls`), else a plain call.
fn build(guarded: bool) -> (Func, u32, u32) {
    let mut f = Func::new("sroa_guard_calls".into(), 0);
    let n = f.vreg(Cls::I);
    f.params = vec![n];
    f.rets = vec![Cls::I];
    let (r_slot, a_slot) = (f.slot(8, 8), f.slot(8, 8));
    let b: Vec<u32> = (0..7).map(|_| f.block()).collect();
    let v: Vec<VReg> = (0..12).map(|_| f.vreg(Cls::I)).collect();
    let (r, a, i, acc, c, flag, w, fast, x, y, rv, one) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10], v[11]);
    push(&mut f, b[0], Inst::SlotAddr(r, r_slot));
    push(&mut f, b[0], Inst::SlotAddr(a, a_slot));
    push(&mut f, b[0], Inst::Iconst(i, 0));
    push(&mut f, b[0], Inst::Iconst(acc, 0));
    push(&mut f, b[0], Inst::Iconst(one, 1));
    f.blocks[b[0] as usize].term = Term::Jump(b[1]);
    push(&mut f, b[1], Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b[1] as usize].term = Term::Branch(c, b[2], b[5]);
    // the call's argument copy, then the guard
    push(&mut f, b[2], Inst::Store(Mem::Int(8, false), a, 0, i));
    push(&mut f, b[2], Inst::Addr(flag, Sym { id: 9, off: 16 }));
    push(&mut f, b[2], Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, w, flag));
    push(&mut f, b[2], Inst::ICmpI(Cond::Eq, false, fast, w, 0));
    f.blocks[b[2] as usize].term = Term::Branch(fast, b[3], b[4]);
    // the inlined copy: reads its argument, writes its result
    push(&mut f, b[3], Inst::Load(Mem::Int(8, false), x, a, 0));
    push(&mut f, b[3], Inst::IBin(IOp::Add, U64, y, x, one));
    push(&mut f, b[3], Inst::Store(Mem::Int(8, false), r, 0, y));
    f.blocks[b[3] as usize].term = Term::Jump(b[6]);
    // the slow call
    f.blocks[b[4] as usize].insts.push(Inst::Call(Callee::Fn(7), vec![r, a], vec![]));
    f.blocks[b[4] as usize].pos.push(SLOW_POS);
    f.blocks[b[4] as usize].term = Term::Jump(b[6]);
    // the continuation reads the result
    push(&mut f, b[6], Inst::Load(Mem::Int(8, false), rv, r, 0));
    push(&mut f, b[6], Inst::IBin(IOp::Add, U64, acc, acc, rv));
    push(&mut f, b[6], Inst::IBin(IOp::Add, U64, i, i, one));
    f.blocks[b[6] as usize].term = Term::Jump(b[1]);
    f.blocks[b[5] as usize].term = Term::Ret(vec![acc]);
    f.sret_callees = vec![7];
    if guarded {
        f.guard_calls = vec![(SLOW_POS, 7)];
    }
    f.verify().unwrap();
    (f, b[4], b[6])
}

/// sroa as O3 runs it: the callee's RIR proves it keeps no copy of its argument (param_nocapture).
fn sroa(f: &mut Func) {
    passes::set_sroa_nocapture(Some(Box::new(|c: &Callee, k: usize| matches!(c, Callee::Fn(7)) && k == 1)));
    passes::sroa(f);
    passes::set_sroa_nocapture(None);
    f.verify().unwrap();
}

/// The blocks that load, store or copy through a slot address.
fn slot_access_blocks(f: &Func) -> Vec<usize> {
    let mut slot_addr = vec![false; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, _) = i {
                slot_addr[d.0 as usize] = true;
            }
        }
    }
    let on_slot = |v: &VReg| slot_addr[v.0 as usize];
    let mut out = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let touches = b.insts.iter().any(|i| match i {
            Inst::Load(_, _, base, _) | Inst::Store(_, base, _, _) => on_slot(base),
            Inst::Copy(d, s, _) => on_slot(d) || on_slot(s),
            _ => false,
        });
        if touches {
            out.push(bi);
        }
    }
    out
}

#[test]
fn a_guarded_copys_slots_stay_in_registers_and_meet_memory_only_at_its_slow_call() {
    let (mut f, slow, cont) = build(true);
    sroa(&mut f);
    assert_eq!(slot_access_blocks(&f), vec![slow as usize], "only the slow block touches the slots:\n{}", f.dump());
    let s = &f.blocks[slow as usize].insts;
    let call = s.iter().position(|i| matches!(i, Inst::Call(Callee::Fn(7), ..))).expect("the slow call stays");
    assert!(s[..call].iter().any(|i| matches!(i, Inst::Store(..))), "the argument goes to memory before the slow call:\n{}", f.dump());
    assert!(s[call + 1..].iter().any(|i| matches!(i, Inst::Load(..))), "the result is read back after it:\n{}", f.dump());
    assert!(!f.blocks[cont as usize].insts.iter().any(|i| matches!(i, Inst::Load(..))), "the continuation reads the result from a register:\n{}", f.dump());
}

#[test]
fn the_same_call_in_the_loop_keeps_its_slots_in_memory() {
    let (mut f, _, cont) = build(false);
    sroa(&mut f);
    assert!(slot_access_blocks(&f).contains(&(cont as usize)), "a real call in the loop: the slots stay in memory:\n{}", f.dump());
}
