//! LICM hoists loads of a stack slot no pointer to which is kept (its address only a load /
//! store base, offset by constants, or handed to calls that provably keep no copy) out of a loop
//! that neither writes it nor hands it to a call: a local struct's fields read in each step of
//! a walk (an inlined `SegGrid::nearest_with` over a local grid). The load stays when the loop
//! writes the slot (directly or through a derived address), hands it to a call, when the slot's
//! address is stored, handed to a call that may keep it, when the load reaches past the slot,
//! or with RELAX_OPT=-licmslot.
use relax_backend::passes;
use relax_backend::rir::*;

const U64: IntTy = IntTy { bits: 64, signed: false };

#[derive(Clone, Copy, PartialEq, Debug)]
enum Body {
    /// store through the pointer parameter
    OtherStore,
    /// a call handed only the pointer parameter
    CallWithoutSlot,
    /// store into the slot
    SlotStore,
    /// store through an address derived from the slot's
    DerivedStore,
    /// a call handed the slot's address
    CallWithSlot,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Before {
    Nothing,
    /// the slot's address is stored through `q`
    Stored,
    /// the slot's address is handed to call Fn(7)
    Call,
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// `fn(q: *mut u64, n: u64, v: u64) -> u64` with a 16-byte slot `s`:
/// `s.f8 = v; <before>; acc = 0; i = 0; while i < n { x = s.<off>; <body>; acc += x; i += 1 } acc`.
fn build(body: Body, before: Before, off: i32) -> Func {
    let mut f = Func::new("licm_slot".into(), 0);
    let (q, n, v) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![q, n, v];
    f.rets = vec![Cls::I];
    let sl = f.slot(16, 8);
    let b: Vec<u32> = (0..4).map(|_| f.block()).collect();
    let (s, i, acc, c, x, one) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b[0], Inst::SlotAddr(s, sl));
    push(&mut f, b[0], Inst::Store(Mem::Int(8, false), s, 8, v));
    match before {
        Before::Nothing => {}
        Before::Stored => push(&mut f, b[0], Inst::Store(Mem::Int(8, false), q, 0, s)),
        Before::Call => push(&mut f, b[0], Inst::Call(Callee::Fn(7), vec![s], vec![])),
    }
    push(&mut f, b[0], Inst::Iconst(i, 0));
    push(&mut f, b[0], Inst::Iconst(acc, 0));
    f.blocks[b[0] as usize].term = Term::Jump(b[1]);
    push(&mut f, b[1], Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b[1] as usize].term = Term::Branch(c, b[2], b[3]);
    push(&mut f, b[2], Inst::Load(Mem::Int(8, false), x, s, off));
    match body {
        Body::OtherStore => push(&mut f, b[2], Inst::Store(Mem::Int(8, false), q, 0, i)),
        Body::CallWithoutSlot => push(&mut f, b[2], Inst::Call(Callee::Fn(0), vec![q], vec![])),
        Body::SlotStore => push(&mut f, b[2], Inst::Store(Mem::Int(8, false), s, 0, i)),
        Body::DerivedStore => {
            let d = f.vreg(Cls::I);
            push(&mut f, b[2], Inst::IBinI(IOp::Add, U64, d, s, 8));
            push(&mut f, b[2], Inst::Store(Mem::Int(8, false), d, 0, i));
        }
        Body::CallWithSlot => push(&mut f, b[2], Inst::Call(Callee::Fn(0), vec![s], vec![])),
    }
    push(&mut f, b[2], Inst::IBin(IOp::Add, U64, acc, acc, x));
    push(&mut f, b[2], Inst::Iconst(one, 1));
    push(&mut f, b[2], Inst::IBin(IOp::Add, U64, i, i, one));
    f.blocks[b[2] as usize].term = Term::Jump(b[1]);
    f.blocks[b[3] as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    f
}

/// LICM, then the block holding the load.
fn run(f: &mut Func, options: &relax_session::Options) -> usize {
    let x = f.blocks.iter().flat_map(|b| &b.insts).find_map(|i| if let Inst::Load(_, d, ..) = i { Some(*d) } else { None }).unwrap();
    passes::licm(options, f);
    f.verify().unwrap();
    f.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::Load(_, d, ..) if *d == x))).expect("load vanished")
}

#[test]
fn slot_load_is_hoisted_out_of_a_loop_that_does_not_touch_the_slot() {
    for body in [Body::OtherStore, Body::CallWithoutSlot] {
        let mut f = build(body, Before::Nothing, 8);
        assert_eq!(run(&mut f, &relax_session::Options::default()), 0, "{body:?}: the load stays in the loop");
    }
}

#[test]
fn slot_load_stays_when_the_loop_writes_the_slot_or_hands_it_to_a_call() {
    for body in [Body::SlotStore, Body::DerivedStore, Body::CallWithSlot] {
        let mut f = build(body, Before::Nothing, 8);
        assert_eq!(run(&mut f, &relax_session::Options::default()), 2, "{body:?}");
    }
}

#[test]
fn slot_load_stays_when_the_slot_address_is_stored() {
    let mut f = build(Body::OtherStore, Before::Stored, 8);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
}

/// A call before the loop handed the slot's address may keep it (the loop's store through `q`
/// could then write the slot), unless the callee provably keeps no copy (O3's param_nocapture
/// oracle, LLVM's `nocapture`).
#[test]
fn slot_load_is_hoisted_past_a_call_before_the_loop_only_when_it_keeps_nothing() {
    for (kept_by_callee, hoisted) in [(false, true), (true, false)] {
        let mut f = build(Body::OtherStore, Before::Call, 8);
        passes::set_sroa_nocapture(Some(Box::new(move |c: &Callee, k: usize| matches!(c, Callee::Fn(7)) && k == 0 && !kept_by_callee)));
        let b = run(&mut f, &relax_session::Options::default());
        passes::set_sroa_nocapture(None);
        assert_eq!(b == 0, hoisted, "kept_by_callee {kept_by_callee}: load in b{b}");
    }
    // no oracle: nothing is known about the callee
    let mut f = build(Body::OtherStore, Before::Call, 8);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
    // the build's facts for a callee outside the inline snapshot (`Func::nocapture`): keeps
    // no copy; one that may return the pointer hands it back
    for (returned, hoisted) in [(false, true), (true, false)] {
        let mut f = build(Body::OtherStore, Before::Call, 8);
        f.nocapture = vec![(Callee::Fn(7), 0, returned, None)];
        let b = run(&mut f, &relax_session::Options::default());
        assert_eq!(b == 0, hoisted, "returned {returned}: load in b{b}");
    }
}

#[test]
fn slot_load_past_the_slot_stays() {
    let mut f = build(Body::OtherStore, Before::Nothing, 12);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
}

#[test]
fn licmslot_off_hoists_nothing() {
    let mut f = build(Body::OtherStore, Before::Nothing, 8);
    let mut o = relax_session::Options::default();
    o.opt = vec!["-licmslot".into()];
    assert_eq!(run(&mut f, &o), 2);
}
