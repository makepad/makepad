//! LICM hoists loads of an uncaptured noalias param's dereferenceable memory (`&mut self`
//! fields) out of a loop that does not write that param's memory (rustc's `noalias`, the roots
//! local_cse uses), and leaves them when the loop stores through the param, hands it to a call,
//! stores through a pointer of unknown root, when the param is captured, withheld, or noalias is
//! turned off.
use relax_backend::passes;
use relax_backend::rir::*;

const U64: IntTy = IntTy { bits: 64, signed: false };

#[derive(Clone, Copy, PartialEq)]
enum Body {
    /// store through another pointer parameter
    OtherStore,
    /// store through the noalias param itself
    ParamStore,
    /// store through a pointer derived from the param by address arithmetic
    DerivedStore,
    /// call handed the param
    CallWithParam,
    /// call handed only the other pointer
    CallWithoutParam,
    /// store through a pointer that is redefined (unknown root)
    UnknownStore,
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// `fn(p: &mut S (16 bytes, noalias), q: *mut u64, n: u64) -> u64`:
/// `acc = 0; i = 0; while i < n { x = p.f8; <body>; acc += x; i += 1 } acc`.
/// Returns the function and the load's destination.
fn build(body: Body, capture: bool) -> (Func, VReg) {
    let mut f = Func::new("licm_noalias".into(), 0);
    let (p, q, n) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, q, n];
    f.rets = vec![Cls::I];
    f.noalias = vec![p];
    f.deref = vec![(p, 16)];
    let b: Vec<u32> = (0..4).map(|_| f.block()).collect();
    let (i, acc, c, x, one) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b[0], Inst::Iconst(i, 0));
    push(&mut f, b[0], Inst::Iconst(acc, 0));
    f.blocks[b[0] as usize].term = Term::Jump(b[1]);
    push(&mut f, b[1], Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b[1] as usize].term = Term::Branch(c, b[2], b[3]);
    push(&mut f, b[2], Inst::Load(Mem::Int(8, false), x, p, 8));
    match body {
        Body::OtherStore => push(&mut f, b[2], Inst::Store(Mem::Int(8, false), q, 0, i)),
        Body::ParamStore => push(&mut f, b[2], Inst::Store(Mem::Int(8, false), p, 0, i)),
        Body::DerivedStore => {
            let d = f.vreg(Cls::I);
            push(&mut f, b[2], Inst::IBinI(IOp::Add, U64, d, p, 8));
            push(&mut f, b[2], Inst::Store(Mem::Int(8, false), d, 0, i));
        }
        Body::CallWithParam => push(&mut f, b[2], Inst::Call(Callee::Fn(0), vec![p, q], vec![])),
        Body::CallWithoutParam => push(&mut f, b[2], Inst::Call(Callee::Fn(0), vec![q], vec![])),
        Body::UnknownStore => {
            // `r` has two defs (q, then q + 8): no single root
            let r = f.vreg(Cls::I);
            push(&mut f, b[0], Inst::Mov(r, q));
            push(&mut f, b[2], Inst::IBinI(IOp::Add, U64, r, r, 8));
            push(&mut f, b[2], Inst::Store(Mem::Int(8, false), r, 0, i));
        }
    }
    if capture {
        // the param's address leaves the function through memory
        push(&mut f, b[0], Inst::Store(Mem::Int(8, false), q, 0, p));
    }
    push(&mut f, b[2], Inst::IBin(IOp::Add, U64, acc, acc, x));
    push(&mut f, b[2], Inst::Iconst(one, 1));
    push(&mut f, b[2], Inst::IBin(IOp::Add, U64, i, i, one));
    f.blocks[b[2] as usize].term = Term::Jump(b[1]);
    f.blocks[b[3] as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    (f, x)
}

/// The block holding the instruction that defines `x`.
fn load_block(f: &Func, x: VReg) -> usize {
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            if let Inst::Load(_, d, ..) = i {
                if *d == x {
                    return bi;
                }
            }
        }
    }
    panic!("load vanished");
}

fn run(f: &mut Func, options: &relax_session::Options) -> usize {
    let x = f.blocks.iter().flat_map(|b| &b.insts).find_map(|i| if let Inst::Load(_, d, ..) = i { Some(*d) } else { None }).unwrap();
    passes::licm(options, f);
    f.verify().unwrap();
    load_block(f, x)
}

#[test]
fn load_of_param_field_is_hoisted_past_a_store_through_another_pointer() {
    let (mut f, _) = build(Body::OtherStore, false);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 0, "the load stays in the loop");
}

#[test]
fn load_is_hoisted_past_a_call_not_handed_the_param() {
    let (mut f, _) = build(Body::CallWithoutParam, false);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 0);
}

#[test]
fn load_stays_when_the_loop_stores_through_the_param() {
    for body in [Body::ParamStore, Body::DerivedStore] {
        let (mut f, _) = build(body, false);
        assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
    }
}

#[test]
fn load_stays_when_the_loop_passes_the_param_to_a_call() {
    let (mut f, _) = build(Body::CallWithParam, false);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
}

#[test]
fn load_stays_when_a_store_has_no_known_root() {
    let (mut f, _) = build(Body::UnknownStore, false);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
}

#[test]
fn load_stays_when_the_param_is_captured() {
    let (mut f, _) = build(Body::OtherStore, true);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
}

#[test]
fn load_stays_without_dereferenceable() {
    let (mut f, _) = build(Body::OtherStore, false);
    f.deref.clear();
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
    // and beyond the dereferenceable size
    let (mut f, _) = build(Body::OtherStore, false);
    f.deref = vec![(f.params[0], 12)];
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
}

#[test]
fn withheld_noalias_hoists_nothing() {
    // code that runs before its borrow-check verdict: noalias is withheld
    let (mut f, _) = build(Body::OtherStore, false);
    f.withheld_noalias = std::mem::take(&mut f.noalias);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
}

#[test]
fn no_noalias_option_hoists_nothing() {
    let (mut f, _) = build(Body::OtherStore, false);
    let mut o = relax_session::Options::default();
    o.no_noalias = true;
    assert_eq!(run(&mut f, &o), 2);
}

/// A call before the loop handed the param may keep it; the loop's store through `q` (another
/// pointer, which could be the kept one) may then write the param's memory.
#[test]
fn load_stays_when_a_call_before_the_loop_is_handed_the_param() {
    let (mut f, _) = build(Body::OtherStore, false);
    let at = f.blocks[0].insts.len();
    let p = p_of(&f);
    f.blocks[0].insts.insert(at, Inst::Call(Callee::Fn(0), vec![p], vec![]));
    f.blocks[0].pos.insert(at, 0);
    assert_eq!(run(&mut f, &relax_session::Options::default()), 2);
}

fn p_of(f: &Func) -> VReg {
    f.params[0]
}
