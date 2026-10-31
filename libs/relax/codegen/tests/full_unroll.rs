//! unroll_const_loops on the loop shape of a `for` over a small array with calls in its body
//! (sandbox reconcile's `for (shape, applied, wanted, solid) in [(..), (..)]`): the counter steps in
//! the block after the header test, a second test there leaves for the header's exit block (the
//! loaded Option's tag), the body has a `continue` (two latches) and an invoked call with an unwind
//! edge. The loop must be fully unrolled, and the RIR before and after must agree on an evaluator
//! that records every call (callee argument) in order. The body reads the array (a stack slot of
//! borrows: another slot's address is stored in it) at `slot + k * 8`: unrolled, those reads forward
//! and the borrowed slot stops escaping, the gain that lets the general rule unroll it. The same loop
//! over memory behind a pointer, or over a slot of plain values (box3d's scatter_bodies), stays rolled.
use relax_backend::regalloc::reachable;
use relax_backend::rir::*;

const U64: IntTy = IntTy { bits: 64, signed: false };

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(1);
}

/// Integer evaluator: a Host call appends its first argument to the trace. Returns (result, trace).
fn eval(f: &Func, args: &[u64]) -> (Vec<u64>, Vec<u64>) {
    let mut r = vec![0u64; f.vregs.len()];
    for (p, a) in f.params.iter().zip(args) {
        r[p.0 as usize] = *a;
    }
    let mut trace = Vec::new();
    let mut mem: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    let mut b = 0usize;
    for _ in 0..100_000 {
        for i in &f.blocks[b].insts {
            match *i {
                Inst::Iconst(d, k) => r[d.0 as usize] = k as u64,
                Inst::SlotAddr(d, s) => r[d.0 as usize] = 0x1000 + 0x100 * s as u64,
                Inst::Store(_, base, off, src) => {
                    mem.insert(r[base.0 as usize].wrapping_add(off as i64 as u64), r[src.0 as usize]);
                }
                Inst::Load(_, d, base, off) => r[d.0 as usize] = mem.get(&r[base.0 as usize].wrapping_add(off as i64 as u64)).copied().unwrap_or(0),
                Inst::Mov(d, s) => r[d.0 as usize] = r[s.0 as usize],
                Inst::IBin(IOp::Add, _, d, a, c) => r[d.0 as usize] = r[a.0 as usize].wrapping_add(r[c.0 as usize]),
                Inst::IBinI(IOp::Add, _, d, a, k) => r[d.0 as usize] = r[a.0 as usize].wrapping_add(k as u64),
                Inst::IBinI(IOp::And, _, d, a, k) => r[d.0 as usize] = r[a.0 as usize] & k as u64,
                Inst::IBinI(IOp::Mul, _, d, a, k) => r[d.0 as usize] = r[a.0 as usize].wrapping_mul(k as u64),
                Inst::ICmpI(Cond::Eq, _, d, a, k) => r[d.0 as usize] = (r[a.0 as usize] == k as u64) as u64,
                Inst::ICmpI(Cond::Ne, _, d, a, k) => r[d.0 as usize] = (r[a.0 as usize] != k as u64) as u64,
                Inst::ICmpI(Cond::Lt, false, d, a, k) => r[d.0 as usize] = (r[a.0 as usize] < k as u64) as u64,
                Inst::Call(Callee::Host(_), ref a, _) => trace.push(r[a[0].0 as usize]),
                ref other => panic!("evaluator: {:?}", other),
            }
        }
        match f.blocks[b].term {
            Term::Jump(t) => b = t as usize,
            Term::Branch(c, t, e) => b = if r[c.0 as usize] != 0 { t as usize } else { e as usize },
            Term::Invoke(t, _) => b = t as usize,
            Term::Ret(ref v) => return (v.iter().map(|x| r[x.0 as usize]).collect(), trace),
            ref t => panic!("evaluator: {:?}", t),
        }
    }
    panic!("runaway")
}

/// b0: c = 0, acc = 0 -> b1
/// b1 (header): done = c == 3 -> b7 : b2
/// b2: k = c; c = c + 1; tag = (seed & 1) == 0 || k != 2 (the "loaded Some"); -> b3 : b7
/// b3: skip = (k + seed) & 1 -> b1 (continue: latch 1) : b4
/// b4: call host(k * 10 + seed + base + k * 8) -> invoke b5, unwind b8 (base: the array slot's
///     address, or the parameter `p` for `Base::Pointer`)
/// b5: acc = acc + k -> b1 (latch 2)
/// b7: ret acc
/// b8: (cleanup) ret acc
#[derive(Clone, Copy, PartialEq)]
enum Base {
    /// a slot holding another slot's address (an array of borrows)
    Borrows,
    /// a slot holding a plain value
    Values,
    /// a pointer parameter
    Pointer,
}

fn array_loop(kind: Base) -> Func {
    let mut f = Func::new("arr".into(), 0);
    let seed = f.vreg(Cls::I);
    f.params.push(seed);
    let p = f.vreg(Cls::I);
    f.params.push(p);
    let base = f.vreg(Cls::I);
    let (sc, ad) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.rets.push(Cls::I);
    for _ in 0..9 {
        f.block();
    }
    let v: Vec<VReg> = (0..12).map(|_| f.vreg(Cls::I)).collect();
    let (c, acc, done, k, t0, t1, tag, s1, skip, arg, t2, t3) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10], v[11]);
    push(&mut f, 0, Inst::Iconst(c, 0));
    push(&mut f, 0, Inst::Iconst(acc, 0));
    if kind == Base::Pointer {
        push(&mut f, 0, Inst::Mov(base, p));
    } else {
        let s = f.slot(32, 8);
        push(&mut f, 0, Inst::SlotAddr(base, s));
        let x = f.vreg(Cls::I);
        if kind == Base::Borrows {
            let s2 = f.slot(8, 8);
            push(&mut f, 0, Inst::SlotAddr(x, s2));
        } else {
            push(&mut f, 0, Inst::Iconst(x, 5));
        }
        push(&mut f, 0, Inst::Store(Mem::Int(8, false), base, 0, x));
    }
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::ICmpI(Cond::Eq, false, done, c, 3));
    f.blocks[1].term = Term::Branch(done, 7, 2);
    push(&mut f, 2, Inst::Mov(k, c));
    push(&mut f, 2, Inst::IBinI(IOp::Add, U64, c, c, 1));
    push(&mut f, 2, Inst::IBinI(IOp::And, U64, t0, seed, 1));
    push(&mut f, 2, Inst::ICmpI(Cond::Eq, false, t1, t0, 0));
    push(&mut f, 2, Inst::ICmpI(Cond::Ne, false, t2, k, 2));
    push(&mut f, 2, Inst::IBin(IOp::Add, U64, t3, t1, t2));
    push(&mut f, 2, Inst::ICmpI(Cond::Ne, false, tag, t3, 0));
    f.blocks[2].term = Term::Branch(tag, 3, 7);
    push(&mut f, 3, Inst::IBin(IOp::Add, U64, s1, k, seed));
    push(&mut f, 3, Inst::IBinI(IOp::And, U64, skip, s1, 1));
    f.blocks[3].term = Term::Branch(skip, 1, 4);
    push(&mut f, 4, Inst::IBinI(IOp::Mul, U64, arg, k, 10));
    push(&mut f, 4, Inst::IBin(IOp::Add, U64, arg, arg, seed));
    push(&mut f, 4, Inst::IBinI(IOp::Mul, U64, sc, k, 8));
    push(&mut f, 4, Inst::IBin(IOp::Add, U64, ad, base, sc));
    push(&mut f, 4, Inst::IBin(IOp::Add, U64, arg, arg, ad));
    push(&mut f, 4, Inst::Call(Callee::Host(Sym::new(7)), vec![arg], vec![]));
    f.blocks[4].term = Term::Invoke(5, 8);
    push(&mut f, 5, Inst::IBin(IOp::Add, U64, acc, acc, k));
    f.blocks[5].term = Term::Jump(1);
    f.blocks[6].term = Term::Unreachable;
    f.blocks[7].term = Term::Ret(vec![acc]);
    f.blocks[8].term = Term::Ret(vec![acc]);
    for b in &mut f.blocks {
        b.pos = vec![1; b.insts.len()];
    }
    f
}

#[test]
fn a_loop_with_calls_two_latches_and_a_split_exit_test_unrolls() {
    let f = array_loop(Base::Borrows);
    f.verify().unwrap();
    let o = relax_session::Options::default();
    let mut g = f.clone();
    assert!(relax_codegen::loops::unroll_const_loops(&o, &mut g), "not unrolled");
    g.verify().unwrap();
    for seed in 0..8u64 {
        assert_eq!(eval(&f, &[seed, 0x5000]), eval(&g, &[seed, 0x5000]), "seed {}", seed);
    }
    // folded, the original loop is unreachable: every call left is in a copy, one per trip
    relax_backend::passes::simplify(&o, &mut g, None, true, true);
    g.verify().unwrap();
    let reach = reachable(&g);
    let preds = relax_backend::regalloc::preds_of(&g, &reach);
    assert!(relax_backend::regalloc::natural_loops(&g, &reach, &preds, false).is_empty(), "a loop is left:\n{}", g.dump());
    let calls = g.blocks.iter().enumerate().filter(|(bi, b)| reach[*bi] && b.insts.iter().any(|i| matches!(i, Inst::Call(..)))).count();
    assert_eq!(calls, 3, "one call per trip:\n{}", g.dump());
    for seed in 0..8u64 {
        assert_eq!(eval(&f, &[seed, 0x5000]), eval(&g, &[seed, 0x5000]), "seed {} after simplify", seed);
    }
}

#[test]
fn the_same_loop_over_a_pointer_or_a_slot_of_values_stays_rolled() {
    for kind in [Base::Pointer, Base::Values] {
        let f = array_loop(kind);
        f.verify().unwrap();
        let mut g = f.clone();
        assert!(!relax_codegen::loops::unroll_const_loops(&relax_session::Options::default(), &mut g), "unrolled:\n{}", g.dump());
    }
}

/// A small counted loop in front of the array loop: unrolled first, its copies add vregs the
/// array loop's gain check then scans (once an out-of-range index that dropped the O3 build). The
/// result must agree (the array loop's own copies stay unentered: the pass keeps the entries of its
/// start).
#[test]
fn the_gain_check_after_an_earlier_unrolled_loop_in_the_same_pass() {
    let mut f = array_loop(Base::Borrows);
    let (j, t, s) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let pre = f.block();
    push(&mut f, 0, Inst::Iconst(j, 0));
    push(&mut f, 0, Inst::Iconst(s, 0));
    f.blocks[0].term = Term::Jump(pre);
    push(&mut f, pre, Inst::IBin(IOp::Add, U64, s, s, j));
    push(&mut f, pre, Inst::IBinI(IOp::Add, U64, j, j, 1));
    push(&mut f, pre, Inst::ICmpI(Cond::Lt, false, t, j, 2));
    f.blocks[pre as usize].term = Term::Branch(t, pre, 1);
    f.verify().unwrap();
    let o = relax_session::Options::default();
    let mut g = f.clone();
    assert!(relax_codegen::loops::unroll_const_loops(&o, &mut g), "not unrolled");
    g.verify().unwrap();
    for seed in 0..8u64 {
        assert_eq!(eval(&f, &[seed, 0x5000]), eval(&g, &[seed, 0x5000]), "seed {}", seed);
    }
}

/// camera_path_limit's slab test: `for a in 0..3` over a local array of plain values (`let d =
/// [dir.x, dir.y, dir.z]`), read at `slot + a * 8` and nowhere else but fixed-offset stores, whose two
/// latches also leave the loop (`continue 'entities` from the test that ends each iteration):
/// b0: s = slot; s[0], s[8], s[16] = seed, seed + 1, seed * 3; c = 0; acc = 0 -> b1
/// b1 (header): c < 3 -> b2 : b5
/// b2: k = c; c = c + 1; x = s[k * 8]; odd = x & 1 -> b6 : b3
/// b3: acc = acc + x; stop = acc == 7 -> b4 (leaves: latch 1) : b1
/// b6: acc = acc + 1; stop2 = acc == 5 -> b4 (latch 2) : b1
/// b4: ret acc + 1000;  b5: ret acc
/// Unrolled (the counter indexes the array: the copies read it at constant offsets), and the result
/// agrees; with MAKEPAD_RUN_OPT=-unrollearly it stays rolled.
fn slab_loop() -> Func {
    let mut f = Func::new("slab".into(), 0);
    let seed = f.vreg(Cls::I);
    f.params.push(seed);
    f.rets.push(Cls::I);
    for _ in 0..7 {
        f.block();
    }
    let v: Vec<VReg> = (0..16).map(|_| f.vreg(Cls::I)).collect();
    let (s, x1, x3, c, acc, lt, k, sh, ad, x, odd, stop, stop2, r4) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10], v[11], v[12], v[13]);
    let sl = f.slot(24, 8);
    push(&mut f, 0, Inst::SlotAddr(s, sl));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), s, 0, seed));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, x1, seed, 1));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), s, 8, x1));
    push(&mut f, 0, Inst::IBinI(IOp::Mul, U64, x3, seed, 3));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), s, 16, x3));
    push(&mut f, 0, Inst::Iconst(c, 0));
    push(&mut f, 0, Inst::Iconst(acc, 0));
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::ICmpI(Cond::Lt, false, lt, c, 3));
    f.blocks[1].term = Term::Branch(lt, 2, 5);
    push(&mut f, 2, Inst::Mov(k, c));
    push(&mut f, 2, Inst::IBinI(IOp::Add, U64, c, c, 1));
    push(&mut f, 2, Inst::IBinI(IOp::Mul, U64, sh, k, 8));
    push(&mut f, 2, Inst::IBin(IOp::Add, U64, ad, s, sh));
    push(&mut f, 2, Inst::Load(Mem::Int(8, false), x, ad, 0));
    push(&mut f, 2, Inst::IBinI(IOp::And, U64, odd, x, 1));
    f.blocks[2].term = Term::Branch(odd, 6, 3);
    push(&mut f, 3, Inst::IBin(IOp::Add, U64, acc, acc, x));
    push(&mut f, 3, Inst::ICmpI(Cond::Eq, false, stop, acc, 7));
    f.blocks[3].term = Term::Branch(stop, 4, 1);
    push(&mut f, 6, Inst::IBinI(IOp::Add, U64, acc, acc, 1));
    push(&mut f, 6, Inst::ICmpI(Cond::Eq, false, stop2, acc, 5));
    f.blocks[6].term = Term::Branch(stop2, 4, 1);
    push(&mut f, 4, Inst::IBinI(IOp::Add, U64, r4, acc, 1000));
    f.blocks[4].term = Term::Ret(vec![r4]);
    f.blocks[5].term = Term::Ret(vec![acc]);
    for b in &mut f.blocks {
        b.pos = vec![1; b.insts.len()];
    }
    f
}

#[test]
fn a_short_loop_over_a_local_array_of_values_whose_latches_leave_unrolls() {
    let f = slab_loop();
    f.verify().unwrap();
    let o = relax_session::Options::default();
    let mut g = f.clone();
    assert!(relax_codegen::loops::unroll_const_loops(&o, &mut g), "not unrolled:\n{}", f.dump());
    g.verify().unwrap();
    for seed in 0..16u64 {
        assert_eq!(eval(&f, &[seed]), eval(&g, &[seed]), "seed {}", seed);
    }
    relax_backend::passes::simplify(&o, &mut g, None, true, true);
    g.verify().unwrap();
    let reach = reachable(&g);
    let preds = relax_backend::regalloc::preds_of(&g, &reach);
    assert!(relax_backend::regalloc::natural_loops(&g, &reach, &preds, false).is_empty(), "a loop is left:\n{}", g.dump());
    for seed in 0..16u64 {
        assert_eq!(eval(&f, &[seed]), eval(&g, &[seed]), "seed {} after simplify", seed);
    }
    let mut off = relax_session::Options::default();
    off.opt = vec!["-unrollearly".to_string()];
    let mut h = f.clone();
    assert!(!relax_codegen::loops::unroll_const_loops(&off, &mut h), "unrolled with -unrollearly");
}
