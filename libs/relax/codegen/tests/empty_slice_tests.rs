//! fold_empty_tests on a rotated loop over a cell's items, `for &i in &items[at[c] as usize..at[c
//! + 1] as usize]`: the entry test `begin == begin + (end - start) * 4` compares the two u32
//! indices instead, and the slice pointers are built on the non-empty edge only (a block of their
//! own in front of the loop). With indices of 64 bits the scaled difference can wrap to zero, a
//! difference or a shift narrower than the pointers is zero for other indices too, and an index
//! defined again after the subtraction is no longer the one subtracted: no fold. Results must agree
//! with the RIR before the pass; RELAX_OPT=-emptytest leaves it.
use relax_backend::rir::*;
use relax_codegen::opt;

const U64: IntTy = IntTy { bits: 64, signed: false };

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(1);
}

/// Integer evaluator over a 4-byte-cell memory.
fn eval(f: &Func, args: &[u64], mem: &std::collections::HashMap<u64, u64>) -> Vec<u64> {
    let mut r = vec![0u64; f.vregs.len()];
    for (p, a) in f.params.iter().zip(args) {
        r[p.0 as usize] = *a;
    }
    let rd = |a: u64| mem.get(&a).copied().unwrap_or(0);
    let mut b = 0usize;
    for _ in 0..100_000 {
        for i in &f.blocks[b].insts {
            match *i {
                Inst::Iconst(d, k) => r[d.0 as usize] = k as u64,
                Inst::Mov(d, s) => r[d.0 as usize] = r[s.0 as usize],
                Inst::Load(_, d, base, off) => r[d.0 as usize] = rd(r[base.0 as usize].wrapping_add(off as i64 as u64)),
                Inst::LoadX(_, d, base, x, sh, off) => r[d.0 as usize] = rd(r[base.0 as usize].wrapping_add(r[x.0 as usize] << sh).wrapping_add(off as i64 as u64)),
                Inst::IBin(IOp::Add, _, d, a, c) => r[d.0 as usize] = r[a.0 as usize].wrapping_add(r[c.0 as usize]),
                Inst::IBin(IOp::Sub, _, d, a, c) => r[d.0 as usize] = r[a.0 as usize].wrapping_sub(r[c.0 as usize]),
                Inst::IBinI(IOp::Add, _, d, a, k) => r[d.0 as usize] = r[a.0 as usize].wrapping_add(k as u64),
                Inst::IBinI(IOp::Shl, _, d, a, k) => r[d.0 as usize] = r[a.0 as usize] << (k & 63),
                Inst::ICmp(Cond::Eq, _, d, a, c) => r[d.0 as usize] = (r[a.0 as usize] == r[c.0 as usize]) as u64,
                Inst::ICmpI(Cond::Eq, _, d, a, k) => r[d.0 as usize] = (r[a.0 as usize] == k as u64) as u64,
                ref other => panic!("evaluator: {:?}", other),
            }
        }
        match f.blocks[b].term {
            Term::Jump(t) => b = t as usize,
            Term::Branch(c, t, e) => b = if r[c.0 as usize] != 0 { t as usize } else { e as usize },
            Term::Ret(ref v) => return v.iter().map(|x| r[x.0 as usize]).collect(),
            ref t => panic!("evaluator: {:?}", t),
        }
    }
    panic!("runaway")
}

/// b0: s = at[c]; e = at[c + 1] (u32, or u64 with `wide`); begin = items + s * 4;
///     end = begin + (e - s) * 4; cur = begin; acc = 0 -> b1
/// b1 (the rotated loop's entry test): cur == end -> b3 : b2
/// b2: cur = cur + 4; acc = acc + cur[-4]; cur == end -> b3 : b2
/// b3: ret acc
fn cell_sum(wide: bool) -> Func {
    let mut f = Func::new("cell_sum".into(), 0);
    let (at, items, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.extend([at, items, c]);
    f.rets = vec![Cls::I];
    for _ in 0..4 {
        f.block();
    }
    let v: Vec<VReg> = (0..14).map(|_| f.vreg(Cls::I)).collect();
    let (c1, s, e, len, sh, begin, lsh, end, cur, acc, eq, cur2, x, eq2) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10], v[11], v[12], v[13]);
    let m = Mem::Int(if wide { 8 } else { 4 }, false);
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, c1, c, 1));
    push(&mut f, 0, Inst::LoadX(m, s, at, c, 2, 0));
    push(&mut f, 0, Inst::LoadX(m, e, at, c1, 2, 0));
    push(&mut f, 0, Inst::IBin(IOp::Sub, U64, len, e, s));
    push(&mut f, 0, Inst::IBinI(IOp::Shl, U64, sh, s, 2));
    push(&mut f, 0, Inst::IBin(IOp::Add, U64, begin, items, sh));
    push(&mut f, 0, Inst::IBinI(IOp::Shl, U64, lsh, len, 2));
    push(&mut f, 0, Inst::IBin(IOp::Add, U64, end, begin, lsh));
    push(&mut f, 0, Inst::Mov(cur, begin));
    push(&mut f, 0, Inst::Iconst(acc, 0));
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::ICmp(Cond::Eq, false, eq, cur, end));
    f.blocks[1].term = Term::Branch(eq, 3, 2);
    push(&mut f, 2, Inst::IBinI(IOp::Add, U64, cur2, cur, 4));
    push(&mut f, 2, Inst::Mov(cur, cur2));
    push(&mut f, 2, Inst::Load(Mem::Int(4, false), x, cur2, -4));
    push(&mut f, 2, Inst::IBin(IOp::Add, U64, acc, acc, x));
    push(&mut f, 2, Inst::ICmp(Cond::Eq, false, eq2, cur, end));
    f.blocks[2].term = Term::Branch(eq2, 3, 2);
    f.blocks[3].term = Term::Ret(vec![acc]);
    f
}

/// at = [0, 3, 3, 5, 5] (cells 1 and 3 empty) at 0x1000, items 10, 20, .. at 0x2000
fn memory() -> std::collections::HashMap<u64, u64> {
    let mut m = std::collections::HashMap::new();
    for (k, a) in [0u64, 3, 3, 5, 5].iter().enumerate() {
        m.insert(0x1000 + 4 * k as u64, *a);
    }
    for k in 0..5u64 {
        m.insert(0x2000 + 4 * k, 10 * (k + 1));
    }
    m
}

#[test]
fn the_entry_test_compares_the_indices_and_the_pointers_move_onto_the_loop_edge() {
    let f = cell_sum(false);
    f.verify().unwrap();
    let mut off = relax_session::Options::default();
    off.opt = vec!["-emptytest".to_string()];
    let mut h = f.clone();
    assert!(!opt::fold_empty_tests(&off, &mut h));
    let mut g = f.clone();
    assert!(opt::fold_empty_tests(&relax_session::Options::default(), &mut g), "not folded:\n{}", f.dump());
    g.verify().unwrap();
    // the test reads the two loaded indices
    let (s, e) = (VReg(f.vregs.len() as u32 - 13), VReg(f.vregs.len() as u32 - 12));
    let guard = g.blocks.iter().position(|b| matches!(b.insts[..], [Inst::ICmp(Cond::Eq, false, _, a, b)] if a == e && b == s)).unwrap_or_else(|| panic!("{}", g.dump()));
    // the entry block keeps the loads (and the counter's start), the pointer arithmetic is on
    // the edge into the loop
    assert!(!g.blocks[0].insts.iter().any(|i| matches!(i, Inst::IBin(..) | Inst::IBinI(IOp::Shl, ..))), "{}", g.dump());
    let Term::Branch(_, empty, edge) = g.blocks[guard].term else { panic!() };
    assert!(matches!(g.blocks[empty as usize].term, Term::Ret(_)), "{}", g.dump());
    let Term::Jump(body) = g.blocks[edge as usize].term else { panic!("{}", g.dump()) };
    assert!(g.blocks[body as usize].insts.iter().any(|i| matches!(i, Inst::Load(..))), "{}", g.dump());
    assert_eq!(g.blocks[edge as usize].insts.iter().filter(|i| matches!(i, Inst::IBin(..) | Inst::IBinI(IOp::Shl, ..))).count(), 5, "{}", g.dump());
    let mem = memory();
    for c in 0..4u64 {
        let want = eval(&f, &[0x1000, 0x2000, c], &mem);
        assert_eq!(eval(&g, &[0x1000, 0x2000, c], &mem), want, "cell {}", c);
    }
    assert_eq!(eval(&g, &[0x1000, 0x2000, 0], &mem), vec![60]);
    assert_eq!(eval(&g, &[0x1000, 0x2000, 1], &mem), vec![0]);
}

#[test]
fn indices_of_64_bits_keep_the_pointer_test() {
    let f = cell_sum(true);
    let mut g = f.clone();
    assert!(!opt::fold_empty_tests(&relax_session::Options::default(), &mut g), "{}", g.dump());
}

#[derive(Clone, Copy, PartialEq)]
enum Diff {
    /// end - start, 64 bits
    Sub64,
    /// end - start, 32 bits: zero for indices 1 << 32 apart
    Sub32,
    /// end - start, then end defined again before the test
    SubRedefined,
    /// the u32 at start << 4, 64 bits
    Shl64,
    /// the u32 at start << 4, 32 bits: zero for 1 << 28
    Shl32,
}

/// b0: d = (the difference); end_p = begin + d; begin == end_p -> b1 : b2; b1: ret 0; b2: ret end_p
fn pointer_test(diff: Diff) -> Func {
    let mut f = Func::new("pointer_test".into(), 0);
    let (begin, start, end) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.extend([begin, start, end]);
    f.rets = vec![Cls::I];
    for _ in 0..3 {
        f.block();
    }
    let v: Vec<VReg> = (0..5).map(|_| f.vreg(Cls::I)).collect();
    let (d, t, end_p, eq, zero) = (v[0], v[1], v[2], v[3], v[4]);
    const U32: IntTy = IntTy { bits: 32, signed: false };
    match diff {
        Diff::Sub64 => push(&mut f, 0, Inst::IBin(IOp::Sub, U64, d, end, start)),
        Diff::Sub32 => push(&mut f, 0, Inst::IBin(IOp::Sub, U32, d, end, start)),
        Diff::SubRedefined => {
            push(&mut f, 0, Inst::IBin(IOp::Sub, U64, d, end, start));
            push(&mut f, 0, Inst::IBinI(IOp::Add, U64, end, end, 1));
        }
        Diff::Shl64 | Diff::Shl32 => {
            push(&mut f, 0, Inst::Load(Mem::Int(4, false), t, start, 0));
            push(&mut f, 0, Inst::IBinI(IOp::Shl, if diff == Diff::Shl64 { U64 } else { U32 }, d, t, 4));
        }
    }
    push(&mut f, 0, Inst::IBin(IOp::Add, U64, end_p, begin, d));
    push(&mut f, 0, Inst::ICmp(Cond::Eq, false, eq, begin, end_p));
    f.blocks[0].term = Term::Branch(eq, 1, 2);
    push(&mut f, 1, Inst::Iconst(zero, 0));
    f.blocks[1].term = Term::Ret(vec![zero]);
    f.blocks[2].term = Term::Ret(vec![end_p]);
    f
}

#[test]
fn a_narrower_difference_or_a_redefined_index_keeps_the_pointer_test() {
    let on = relax_session::Options::default();
    let mut g = pointer_test(Diff::Sub64);
    g.verify().unwrap();
    let (start, end) = (g.params[1], g.params[2]);
    assert!(opt::fold_empty_tests(&on, &mut g), "{}", g.dump());
    assert!(matches!(g.blocks[0].insts.last(), Some(&Inst::ICmp(Cond::Eq, false, _, a, b)) if a == end && b == start), "{}", g.dump());
    let mut g = pointer_test(Diff::Shl64);
    g.verify().unwrap();
    let t = g.blocks[0].insts.iter().find_map(|i| if let Inst::Load(_, d, ..) = *i { Some(d) } else { None }).unwrap();
    assert!(opt::fold_empty_tests(&on, &mut g), "{}", g.dump());
    assert!(matches!(g.blocks[0].insts.last(), Some(&Inst::ICmpI(Cond::Eq, false, _, a, 0)) if a == t), "{}", g.dump());
    for diff in [Diff::Sub32, Diff::SubRedefined, Diff::Shl32] {
        let f = pointer_test(diff);
        f.verify().unwrap();
        let mut g = f.clone();
        assert!(!opt::fold_empty_tests(&on, &mut g), "folded:\n{}", g.dump());
        assert_eq!(g.dump(), f.dump());
    }
}
