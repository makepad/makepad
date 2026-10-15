//! Volatile accesses (AtomicLoad / AtomicStore with AtomOrd::Volatile) through every optimiser pass
//! family: never removed (DCE, dead stores), merged (CSE), forwarded (slot store/load/copy forwarding,
//! SROA), hoisted or speculated (LICM, if-conversion, layout) or reordered with each other.
use relax_codegen::opt;
use relax_codegen::rir::*;

const U64: IntTy = IntTy { bits: 64, signed: false };

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// Every pass the JIT runs, in pipeline order, twice over (a later pass can undo an earlier one's care).
fn all_passes(f: &mut Func) {
    for _ in 0..2 {
        opt::simplify(&relax_session::Options::default(), f, None, true, true);
        opt::forward_slot_copies(f);
        opt::forward_slot_dests(f);
        opt::forward_slot_loads(f);
        opt::sroa(f);
        opt::simplify(&relax_session::Options::default(), f, None, true, true);
        opt::licm(f);
        opt::live_dce(f);
        opt::sink_consts(f);
        opt::relayout(f);
    }
    opt::addr_modes(f);
    opt::rotate_loops(f);
    f.verify().unwrap();
}

fn volatile_ops(f: &Func) -> Vec<(usize, Inst)> {
    let mut v = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            if matches!(i, Inst::AtomicLoad(_, AtomOrd::Volatile, ..) | Inst::AtomicStore(_, AtomOrd::Volatile, ..)) {
                v.push((bi, i.clone()));
            }
        }
    }
    v
}

fn func(name: &str, nparams: usize) -> Func {
    let mut f = Func::new(name.to_string(), 0);
    for _ in 0..nparams {
        let p = f.vreg(Cls::I);
        f.params.push(p);
    }
    f
}

#[test]
fn dce_keeps_unused_volatile_load() {
    let mut f = func("dce", 1);
    let p = f.params[0];
    let b = f.block();
    let d = f.vreg(Cls::I);
    push(&mut f, b, Inst::AtomicLoad(Mem::Int(4, false), AtomOrd::Volatile, d, p));
    f.blocks[b as usize].term = Term::Ret(vec![]);
    all_passes(&mut f);
    assert_eq!(volatile_ops(&f).len(), 1, "{}", f.dump());
}

#[test]
fn cse_keeps_both_volatile_loads_in_order() {
    let mut f = func("cse", 1);
    f.rets = vec![Cls::I];
    let p = f.params[0];
    let b = f.block();
    let (d1, d2, s) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b, Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Volatile, d1, p));
    push(&mut f, b, Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Volatile, d2, p));
    push(&mut f, b, Inst::IBin(IOp::Sub, U64, s, d1, d2));
    f.blocks[b as usize].term = Term::Ret(vec![s]);
    all_passes(&mut f);
    let v = volatile_ops(&f);
    assert_eq!(v.len(), 2, "{}", f.dump());
    // the first defines the subtraction's left operand: same order as written
    let sub = f.blocks.iter().flat_map(|b| b.insts.iter()).find_map(|i| if let Inst::IBin(IOp::Sub, _, _, a, b2) = i { Some((*a, *b2)) } else { None });
    if let (Inst::AtomicLoad(_, _, x, _), Inst::AtomicLoad(_, _, y, _), Some((a, b2))) = (&v[0].1, &v[1].1, sub) {
        assert_eq!((*x, *y), (a, b2), "{}", f.dump());
    }
}

#[test]
fn slot_forwarding_and_sroa_keep_volatile_accesses() {
    // a slot written plainly then read volatile, written volatile then read plainly, and a
    // volatile store nobody reads: every volatile access stays, none becomes a register value
    let mut f = func("slots", 0);
    f.rets = vec![Cls::I];
    let s = f.slot(8, 8);
    let b = f.block();
    let (a, c5, c9, d1, d2, sum) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b, Inst::SlotAddr(a, s));
    push(&mut f, b, Inst::Iconst(c5, 5));
    push(&mut f, b, Inst::Store(Mem::Int(8, false), a, 0, c5));
    push(&mut f, b, Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Volatile, d1, a));
    push(&mut f, b, Inst::Iconst(c9, 9));
    push(&mut f, b, Inst::AtomicStore(Mem::Int(8, false), AtomOrd::Volatile, a, c9));
    push(&mut f, b, Inst::Load(Mem::Int(8, false), d2, a, 0));
    push(&mut f, b, Inst::IBin(IOp::Add, U64, sum, d1, d2));
    push(&mut f, b, Inst::AtomicStore(Mem::Int(8, false), AtomOrd::Volatile, a, c5));
    f.blocks[b as usize].term = Term::Ret(vec![sum]);
    all_passes(&mut f);
    let v = volatile_ops(&f);
    assert_eq!(v.len(), 3, "{}", f.dump());
    assert!(matches!(v[0].1, Inst::AtomicLoad(..)) && matches!(v[1].1, Inst::AtomicStore(..)) && matches!(v[2].1, Inst::AtomicStore(..)), "{}", f.dump());
}

#[test]
fn licm_keeps_volatile_load_in_loop() {
    // loop: i = 0; do { x += *p (volatile); i += 1 } while i < 10
    let mut f = func("licm", 1);
    f.rets = vec![Cls::I];
    let p = f.params[0];
    let (b0, b1, b2) = (f.block(), f.block(), f.block());
    let (i, x, d, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Iconst(i, 0));
    push(&mut f, b0, Inst::Iconst(x, 0));
    f.blocks[b0 as usize].term = Term::Jump(b1);
    push(&mut f, b1, Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Volatile, d, p));
    push(&mut f, b1, Inst::IBin(IOp::Add, U64, x, x, d));
    push(&mut f, b1, Inst::IBinI(IOp::Add, U64, i, i, 1));
    push(&mut f, b1, Inst::ICmpI(Cond::Lt, false, c, i, 10));
    f.blocks[b1 as usize].term = Term::Branch(c, b1, b2);
    f.blocks[b2 as usize].term = Term::Ret(vec![x]);
    all_passes(&mut f);
    let v = volatile_ops(&f);
    assert_eq!(v.len(), 1, "{}", f.dump());
    // its block is still in the loop: it branches back to itself or reaches itself again
    let lb = v[0].0 as u32;
    let mut seen = vec![false; f.blocks.len()];
    let mut work: Vec<u32> = f.blocks[lb as usize].term.succs().collect();
    let mut cyc = false;
    while let Some(x) = work.pop() {
        if x == lb {
            cyc = true;
            break;
        }
        if !std::mem::replace(&mut seen[x as usize], true) {
            work.extend(f.blocks[x as usize].term.succs());
        }
    }
    assert!(cyc, "volatile load left the loop:\n{}", f.dump());
}

#[test]
fn if_conversion_does_not_speculate_volatile_load() {
    // r = if c { *p (volatile) } else { 0 }: the load stays on its own path (it may fault)
    let mut f = func("ifconv", 2);
    f.rets = vec![Cls::I];
    let (p, cnd) = (f.params[0], f.params[1]);
    let (b0, b1, b2, b3) = (f.block(), f.block(), f.block(), f.block());
    let (r, d, z) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.blocks[b0 as usize].term = Term::Branch(cnd, b1, b2);
    push(&mut f, b1, Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Volatile, d, p));
    push(&mut f, b1, Inst::Mov(r, d));
    f.blocks[b1 as usize].term = Term::Jump(b3);
    push(&mut f, b2, Inst::Iconst(z, 0));
    push(&mut f, b2, Inst::Mov(r, z));
    f.blocks[b2 as usize].term = Term::Jump(b3);
    f.blocks[b3 as usize].term = Term::Ret(vec![r]);
    all_passes(&mut f);
    let v = volatile_ops(&f);
    assert_eq!(v.len(), 1, "{}", f.dump());
    assert_ne!(v[0].0, 0, "volatile load speculated into the entry:\n{}", f.dump());
    assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::AtomicLoad(..))), "{}", f.dump());
}
