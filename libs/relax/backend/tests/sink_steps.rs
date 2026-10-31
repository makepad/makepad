//! regalloc::sink_increments (a counter's step moves down to the next read of the counter, past
//! bounds-check side exits where the counter is dead) and regalloc::sink_branch_compares (a
//! branch's one-use compare moves to the end of its block): where they move things, where they
//! must not, and that the host backend's code computes the same sums.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, regalloc, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&o, f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(n) = sum of 3 * i + (i < 1000 ? 0 : the side exit) for i in 0..n, `Range::next` style:
///   b1: i < n ?            b2: old = i; t = i + 1; i = t; s += 3 * old; old < 1000 ? b5 : b3
///   b3: side exit          b5: latch (jump b1)           b4: return
/// `side_reads_i`: the side exit returns i (live there: the step must not pass it); else it is a
/// panic (Unreachable).
fn build(side_reads_i: bool) -> Func {
    let mut f = Func::new("steps".into(), 0);
    let n = f.vreg(Cls::I);
    f.params = vec![n];
    f.rets = vec![Cls::I];
    for _ in 0..6 {
        f.block();
    }
    let (i, s, c, old, t, x, s2, c2) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, 0, Inst::Iconst(i, 0));
    push(&mut f, 0, Inst::Iconst(s, 0));
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[1].term = Term::Branch(c, 2, 4);
    push(&mut f, 2, Inst::Mov(old, i));
    push(&mut f, 2, Inst::IBinI(IOp::Add, U64, t, i, 1));
    push(&mut f, 2, Inst::Mov(i, t));
    push(&mut f, 2, Inst::IBinI(IOp::Mul, U64, x, old, 3));
    push(&mut f, 2, Inst::IBin(IOp::Add, U64, s2, s, x));
    push(&mut f, 2, Inst::Mov(s, s2));
    push(&mut f, 2, Inst::ICmpI(Cond::Lt, false, c2, old, 1000));
    f.blocks[2].term = Term::Branch(c2, 5, 3);
    f.blocks[3].term = if side_reads_i { Term::Ret(vec![i]) } else { Term::Unreachable };
    f.blocks[4].term = Term::Ret(vec![s]);
    f.blocks[5].term = Term::Jump(1);
    f.verify().unwrap();
    f
}

fn step_block(f: &Func) -> usize {
    (0..f.blocks.len()).find(|&b| f.blocks[b].insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Add, _, _, _, 1)))).unwrap()
}

#[test]
fn step_moves_past_dead_side_exits_to_the_latch() {
    let o = relax_session::Options::default();
    let f = build(false);
    let g = regalloc::sink_increments(&o, &f).expect("the step moves");
    g.verify().unwrap();
    assert_eq!(step_block(&g), 5, "the step lands in the latch");
    // the copy `old = i` stays where it was (the allocator joins it)
    assert!(matches!(g.blocks[2].insts[0], Inst::Mov(..)));
}

#[test]
fn step_stays_when_a_side_exit_reads_the_counter() {
    let o = relax_session::Options::default();
    let f = build(true);
    let g = regalloc::sink_increments(&o, &f);
    // it may move inside b2, never past the exit that returns i
    assert_eq!(step_block(g.as_ref().unwrap_or(&f)), 2);
}

#[test]
fn moved_steps_compute_the_same_sums() {
    type Entry = extern "C" fn(u64) -> u64;
    let code = Code::new(&compile(&build(false))).unwrap();
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    for n in [0u64, 1, 2, 7, 100, 999] {
        assert_eq!(run(n), (0..n).map(|i| 3 * i).sum::<u64>(), "n {n}");
    }
    let code = Code::new(&compile(&build(true))).unwrap();
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    assert_eq!(run(1005), 1000 + 1, "side exit returns the stepped counter");
}

#[test]
fn branch_compare_moves_to_the_block_end() {
    let o = relax_session::Options::default();
    let mut f = Func::new("brc".into(), 0);
    let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b];
    f.rets = vec![Cls::I];
    for _ in 0..3 {
        f.block();
    }
    let (m, c, k, r) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, 0, Inst::IBinI(IOp::And, U64, m, a, 0xff));
    push(&mut f, 0, Inst::ICmpI(Cond::Eq, false, c, m, 0));
    push(&mut f, 0, Inst::Iconst(k, 7));
    push(&mut f, 0, Inst::IBin(IOp::Add, U64, r, b, k));
    f.blocks[0].term = Term::Branch(c, 1, 2);
    f.blocks[1].term = Term::Ret(vec![r]);
    f.blocks[2].term = Term::Ret(vec![a]);
    f.verify().unwrap();
    let g = regalloc::sink_branch_compares(&o, &f).expect("moved");
    g.verify().unwrap();
    let ins = &g.blocks[0].insts;
    assert!(matches!(ins[ins.len() - 1], Inst::ICmpI(..)) && matches!(ins[ins.len() - 2], Inst::IBinI(IOp::And, ..)), "{:?}", ins);
    type Entry = extern "C" fn(u64, u64) -> u64;
    let code = Code::new(&compile(&f)).unwrap();
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    assert_eq!(run(0x100, 5), 12);
    assert_eq!(run(0x101, 5), 0x101);
}

/// Two counters stepped next to each other before one instruction that reads both: each step
/// moves once (they used to hop over each other forever, hanging the O3 worker).
#[test]
fn two_steps_before_a_common_reader_terminate() {
    let o = relax_session::Options::default();
    let mut f = Func::new("two".into(), 0);
    let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b];
    f.rets = vec![Cls::I];
    f.block();
    let (x, y, k, r) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, 0, Inst::Mov(x, a));
    push(&mut f, 0, Inst::Mov(x, a));
    push(&mut f, 0, Inst::Mov(y, b));
    push(&mut f, 0, Inst::Mov(y, b));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, x, x, 1));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, y, y, 2));
    push(&mut f, 0, Inst::Iconst(k, 5));
    push(&mut f, 0, Inst::IBin(IOp::Add, U64, r, x, y));
    f.blocks[0].term = Term::Ret(vec![r]);
    f.verify().unwrap();
    let g = regalloc::sink_increments(&o, &f).expect("both move past the constant");
    let ins = &g.blocks[0].insts;
    assert!(matches!(ins[4], Inst::Iconst(..)), "{:?}", ins);
}
