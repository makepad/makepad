//! A live session's guarded inline copy (`Func::patch_flag`, `guard_calls`): the copy runs while
//! its callee's patch flag reads 0, else the site calls the slot. That slow call is handed the
//! call's result buffer and argument copy, as the call was before it was inlined. It runs only after
//! a patch, so sroa keeps both slots in registers on the copy's path, in a loop too, and flushes
//! and reloads them around the slow call alone. Without that, every value an inlined `sub(a, p)`
//! returned stayed in stack memory, and live sessions ran a mesh's winding number 5x slower than
//! `makepad test`. The result copied on (the next copy's argument copy) stays in registers too
//! once the copies are tiled. The result buffer lowering made for that call (`Slot::ret`) is not
//! flushed before it: the call writes all of it, and a flush keeps the cells live around the loop,
//! which the copy's path paid for (a BVH walk's popped Option stored and copied each iteration).
//! The same call as a real call in the loop keeps the slots in memory (a flush and reload per
//! iteration costs more than it saves). The cells read zero until written, but no zero goes at
//! the entry: the cells only the slow call's read-back writes (an error variant's fields, which
//! the copy never writes) take theirs where the copy's path meets the call's, so they are not live
//! through the function, and a cell no write reaches reads a zero of its own.
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
/// `copied`: the continuation reads the result from a copy of it (`q = r; acc += q`). `ret`: the
/// result buffer is the one lowering made for the call (`Slot::ret`).
fn build(guarded: bool, copied: bool, ret: bool) -> (Func, u32, u32) {
    let mut f = Func::new("sroa_guard_calls".into(), 0);
    let n = f.vreg(Cls::I);
    f.params = vec![n];
    f.rets = vec![Cls::I];
    let (r_slot, a_slot, q_slot) = (f.slot(8, 8), f.slot(8, 8), f.slot(8, 8));
    f.slots[r_slot as usize].ret = ret;
    let b: Vec<u32> = (0..7).map(|_| f.block()).collect();
    let v: Vec<VReg> = (0..13).map(|_| f.vreg(Cls::I)).collect();
    let (r, a, i, acc, c, flag, w, fast, x, y, rv, one, q) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10], v[11], v[12]);
    push(&mut f, b[0], Inst::SlotAddr(r, r_slot));
    push(&mut f, b[0], Inst::SlotAddr(a, a_slot));
    push(&mut f, b[0], Inst::SlotAddr(q, q_slot));
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
    if copied {
        push(&mut f, b[6], Inst::Copy(q, r, 8));
        push(&mut f, b[6], Inst::Load(Mem::Int(8, false), rv, q, 0));
    } else {
        push(&mut f, b[6], Inst::Load(Mem::Int(8, false), rv, r, 0));
    }
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

/// sroa as O3 runs it: the callee's RIR proves it keeps no copy of its argument (param_nocapture);
/// `tile`: after the last inlining round (opt::sroa_post_inline).
fn sroa(f: &mut Func, tile: bool) {
    passes::set_sroa_nocapture(Some(Box::new(|c: &Callee, k: usize| matches!(c, Callee::Fn(7)) && k == 1)));
    passes::sroa_with(f, tile);
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
    let (mut f, slow, cont) = build(true, false, false);
    sroa(&mut f, false);
    assert_eq!(slot_access_blocks(&f), vec![slow as usize], "only the slow block touches the slots:\n{}", f.dump());
    let s = &f.blocks[slow as usize].insts;
    let call = s.iter().position(|i| matches!(i, Inst::Call(Callee::Fn(7), ..))).expect("the slow call stays");
    assert!(s[..call].iter().any(|i| matches!(i, Inst::Store(..))), "the argument goes to memory before the slow call:\n{}", f.dump());
    assert!(s[call + 1..].iter().any(|i| matches!(i, Inst::Load(..))), "the result is read back after it:\n{}", f.dump());
    assert!(!f.blocks[cont as usize].insts.iter().any(|i| matches!(i, Inst::Load(..))), "the continuation reads the result from a register:\n{}", f.dump());
}

#[test]
fn a_guarded_copys_result_copied_on_stays_in_registers() {
    let (mut f, slow, cont) = build(true, true, false);
    sroa(&mut f, true);
    assert_eq!(slot_access_blocks(&f), vec![slow as usize], "only the slow block touches the slots:\n{}", f.dump());
    let s = &f.blocks[slow as usize].insts;
    let call = s.iter().position(|i| matches!(i, Inst::Call(Callee::Fn(7), ..))).expect("the slow call stays");
    assert!(s[call + 1..].iter().any(|i| matches!(i, Inst::Load(..))), "the result is read back after it:\n{}", f.dump());
    assert!(!f.blocks[cont as usize].insts.iter().any(|i| matches!(i, Inst::Load(..) | Inst::Copy(..))), "the copy is a register move:\n{}", f.dump());
}

#[test]
fn the_same_call_in_the_loop_keeps_its_slots_in_memory() {
    let (mut f, _, cont) = build(false, false, true);
    sroa(&mut f, false);
    assert!(slot_access_blocks(&f).contains(&(cont as usize)), "a real call in the loop: the slots stay in memory:\n{}", f.dump());
}

/// The slot each `SlotAddr` vreg names.
fn slot_addrs(f: &Func) -> Vec<Option<u32>> {
    let mut out = vec![None; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, sl) = i {
                out[d.0 as usize] = Some(*sl);
            }
        }
    }
    out
}

#[test]
fn a_guarded_copys_result_buffer_is_read_back_after_its_slow_call_but_not_flushed_before_it() {
    let (mut f, slow, cont) = build(true, false, true);
    sroa(&mut f, false);
    assert_eq!(slot_access_blocks(&f), vec![slow as usize], "only the slow block touches the slots:\n{}", f.dump());
    let addrs = slot_addrs(&f);
    let s = &f.blocks[slow as usize].insts;
    let call = s.iter().position(|i| matches!(i, Inst::Call(Callee::Fn(7), ..))).expect("the slow call stays");
    let stored: Vec<Option<u32>> = s[..call].iter().filter_map(|i| if let Inst::Store(_, base, _, _) = i { Some(addrs[base.0 as usize]) } else { None }).collect();
    // slot 1: the argument copy, slot 0: the result buffer
    assert_eq!(stored, vec![Some(1)], "only the argument goes to memory before the slow call:\n{}", f.dump());
    let loaded: Vec<Option<u32>> = s[call + 1..].iter().filter_map(|i| if let Inst::Load(_, _, base, _) = i { Some(addrs[base.0 as usize]) } else { None }).collect();
    assert!(loaded.contains(&Some(0)), "the result is read back after it:\n{}", f.dump());
    assert!(!f.blocks[cont as usize].insts.iter().any(|i| matches!(i, Inst::Load(..))), "the continuation reads the result from a register:\n{}", f.dump());
}

/// `fn(n, out) -> u64`: `rd(n)?`, as `KnotVector::find_span` reads its knots. `rd` (Fn(7), its
/// 16-byte result buffer first: a tag byte, 0 = Ok, the Ok value at 8, the error's code byte at 1
/// and its index at 4) is inlined behind its patch flag as `Ok(n)`: the copy never writes the
/// error's fields. The error return writes the tag, code and index to `out`, and byte 2 of a slot
/// no one writes. Returns the function, the copy's block and the slow call's.
fn build_result() -> (Func, u32, u32) {
    let mut f = Func::new("sroa_guard_result".into(), 0);
    let (n, out) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, out];
    f.rets = vec![Cls::I];
    let r_slot = f.slot(16, 8);
    f.slots[r_slot as usize].ret = true;
    let s_slot = f.slot(8, 8);
    let b: Vec<u32> = (0..6).map(|_| f.block()).collect();
    let v: Vec<VReg> = (0..13).map(|_| f.vreg(Cls::I)).collect();
    let (r, s, zero, flag, w, fast, tag, isok, ok, x, code, idx, junk) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10], v[11], v[12]);
    push(&mut f, b[0], Inst::SlotAddr(r, r_slot));
    push(&mut f, b[0], Inst::SlotAddr(s, s_slot));
    push(&mut f, b[0], Inst::Iconst(zero, 0));
    push(&mut f, b[0], Inst::Addr(flag, Sym { id: 9, off: 16 }));
    push(&mut f, b[0], Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, w, flag));
    push(&mut f, b[0], Inst::ICmpI(Cond::Eq, false, fast, w, 0));
    f.blocks[b[0] as usize].term = Term::Branch(fast, b[1], b[2]);
    // the inlined copy: Ok(n)
    push(&mut f, b[1], Inst::Store(Mem::Int(1, false), r, 0, zero));
    push(&mut f, b[1], Inst::Store(Mem::Int(8, false), r, 8, n));
    f.blocks[b[1] as usize].term = Term::Jump(b[3]);
    // the slow call
    f.blocks[b[2] as usize].insts.push(Inst::Call(Callee::Fn(7), vec![r, n], vec![]));
    f.blocks[b[2] as usize].pos.push(SLOW_POS);
    f.blocks[b[2] as usize].term = Term::Jump(b[3]);
    // `?`: Ok returns its value, Err returns the error through `out`
    push(&mut f, b[3], Inst::Load(Mem::Int(1, false), tag, r, 0));
    push(&mut f, b[3], Inst::ICmpI(Cond::Eq, false, isok, tag, 0));
    f.blocks[b[3] as usize].term = Term::Branch(isok, b[4], b[5]);
    push(&mut f, b[4], Inst::Load(Mem::Int(8, false), ok, r, 8));
    f.blocks[b[4] as usize].term = Term::Ret(vec![ok]);
    let err = b[5];
    push(&mut f, err, Inst::Load(Mem::Int(1, false), x, r, 0));
    push(&mut f, err, Inst::Load(Mem::Int(1, false), code, r, 1));
    push(&mut f, err, Inst::Load(Mem::Int(4, false), idx, r, 4));
    push(&mut f, err, Inst::Store(Mem::Int(1, false), out, 0, x));
    push(&mut f, err, Inst::Store(Mem::Int(1, false), out, 1, code));
    push(&mut f, err, Inst::Store(Mem::Int(4, false), out, 4, idx));
    push(&mut f, err, Inst::Load(Mem::Int(1, false), junk, s, 2));
    push(&mut f, err, Inst::Store(Mem::Int(1, false), out, 2, junk));
    f.blocks[err as usize].term = Term::Ret(vec![zero]);
    f.sret_callees = vec![7];
    f.guard_calls = vec![(SLOW_POS, 7)];
    f.verify().unwrap();
    (f, b[1], b[2])
}

#[test]
fn cells_take_their_zeros_where_an_unwritten_path_meets_a_write_not_at_the_entry() {
    let (mut f, copy, slow) = build_result();
    let consts = |f: &Func| f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Iconst(..) | Inst::Fconst(..) | Inst::VConst(..))).count();
    let before = consts(&f);
    sroa(&mut f, false);
    assert_eq!(consts(&f), before, "the entry defines no cell:\n{}", f.dump());
    let s = &f.blocks[slow as usize].insts;
    let call = s.iter().position(|i| matches!(i, Inst::Call(Callee::Fn(7), ..))).expect("the slow call stays");
    assert!(!s[..call].iter().any(|i| matches!(i, Inst::Store(..))), "the result buffer is not flushed before the slow call:\n{}", f.dump());
    assert!(s[call + 1..].iter().filter(|i| matches!(i, Inst::Load(..))).count() >= 3, "the call's result is read back, the error's fields too:\n{}", f.dump());
    // the stray byte: no write reaches it, so its read takes a zero defined right before it
    let err = f.blocks.iter().position(|b| matches!(b.term, Term::Ret(_)) && b.insts.iter().any(|i| matches!(i, Inst::Store(_, _, 2, _)))).expect("the error return");
    let e = &f.blocks[err].insts;
    let st = e.iter().position(|i| matches!(i, Inst::Store(_, _, 2, _))).unwrap();
    let Inst::Store(_, _, _, mut val) = e[st] else { unreachable!() };
    let defs_of = |v: VReg| -> Vec<&Inst> { f.blocks.iter().flat_map(|b| b.insts.iter()).filter(|i| { let mut d = false; relax_backend::regalloc::for_defs(i, |x| d |= x == v); d }).collect() };
    // (the load became a move of the cell)
    if let [Inst::Mov(_, src)] = defs_of(val).as_slice() {
        val = *src;
    }
    assert!(matches!(defs_of(val).as_slice(), [Inst::Iconst(_, 0)]), "the unwritten byte reads one zero of its own:\n{}", f.dump());
    assert!(e[..st].iter().any(|i| matches!(i, Inst::Iconst(d, 0) if *d == val)), "defined in the error return, before its read:\n{}", f.dump());
    // the error's code and index: written by the slow call's read-back, zero on the copy's path
    for off in [1, 4] {
        let Some(Inst::Store(_, _, _, v)) = e.iter().find(|i| matches!(i, Inst::Store(_, _, o, _) if *o == off)) else { panic!("the error's field at {off} is stored") };
        let mut cell = None;
        if let [d] = defs_of(*v).as_slice() {
            relax_backend::regalloc::for_uses(d, |u| cell = Some(u));
        }
        let cell = cell.expect("the field is read from its cell");
        let defines = |b: u32| f.blocks[b as usize].insts.iter().any(|i| { let mut d = false; relax_backend::regalloc::for_defs(i, |x| d |= x == cell); d });
        assert!(!defines(0), "the field at {off} gets no zero at the entry:\n{}", f.dump());
        assert!(f.blocks[copy as usize].insts.iter().any(|i| matches!(i, Inst::Iconst(d, 0) if *d == cell)), "the field at {off} reads zero on the copy's path:\n{}", f.dump());
        assert!(defines(slow), "the field at {off} is read back after the slow call:\n{}", f.dump());
    }
}
