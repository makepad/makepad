//! Switch lowering (`switch::lower_switches`, run by the back ends): random chains of compares of
//! one value against constants -- the shape a `match` on an integer, `char` or discriminant
//! lowers to: equality, inclusive ranges, or-patterns, signed and unsigned relations -- give the
//! same results compiled as the first-match-wins reference, whichever of jump table, bit tests or
//! binary search the lowering picks, with and without the thorough register allocation.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*, switch};

#[cfg(target_arch = "x86_64")]
fn compile(options: &relax_session::Options, f: &Func, regs: bool) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: regs, syms: 0, sym_class: 0 };
    let c = relax_backend::x64::compile(options, f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(options: &relax_session::Options, f: &Func, regs: bool) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: regs, syms: 0, sym_class: 0 };
    let c = relax_backend::arm64::compile(options, f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone, Copy)]
struct T {
    cond: Cond,
    signed: bool,
    k: i64,
}

fn holds(t: T, x: u64) -> bool {
    let (a, b) = (x as i64, t.k);
    let (ua, ub) = (x, t.k as u64);
    match (t.cond, t.signed) {
        (Cond::Eq, _) => a == b,
        (Cond::Ne, _) => a != b,
        (Cond::Lt, true) => a < b,
        (Cond::Le, true) => a <= b,
        (Cond::Gt, true) => a > b,
        (Cond::Ge, true) => a >= b,
        (Cond::Lt, false) => ua < ub,
        (Cond::Le, false) => ua <= ub,
        (Cond::Gt, false) => ua > ub,
        (Cond::Ge, false) => ua >= ub,
    }
}

/// arms[i] = alternatives (or-pattern); each alternative a conjunction of tests. Result of arm i is
/// 100 + i; no arm: 7.
fn reference(arms: &[Vec<Vec<T>>], x: u64) -> u64 {
    for (i, alts) in arms.iter().enumerate() {
        if alts.iter().any(|a| a.iter().all(|t| holds(*t, x))) {
            return 100 + i as u64;
        }
    }
    7
}

fn build(arms: &[Vec<Vec<T>>]) -> Func {
    let mut f = Func::new("sw".into(), 0);
    let x = f.vreg(Cls::I);
    f.params = vec![x];
    f.rets = vec![Cls::I];
    let entry = f.block();
    let ret_block = |f: &mut Func, v: i64| -> u32 {
        let b = f.block();
        let r = f.vreg(Cls::I);
        f.blocks[b as usize].insts.push(Inst::Iconst(r, v));
        f.blocks[b as usize].pos.push(0);
        f.blocks[b as usize].term = Term::Ret(vec![r]);
        b
    };
    let default = ret_block(&mut f, 7);
    let targets: Vec<u32> = (0..arms.len()).map(|i| ret_block(&mut f, 100 + i as i64)).collect();
    // tests in arm order; the first one is the entry block
    let mut flat: Vec<(T, usize, usize)> = Vec::new(); // (test, arm, alt)
    let mut firsts: Vec<Vec<usize>> = Vec::new();
    for (i, alts) in arms.iter().enumerate() {
        let mut fa = Vec::new();
        for (j, a) in alts.iter().enumerate() {
            fa.push(flat.len());
            for t in a {
                flat.push((*t, i, j));
            }
        }
        firsts.push(fa);
    }
    let blocks: Vec<u32> = (0..flat.len()).map(|i| if i == 0 { entry } else { f.block() }).collect();
    if flat.is_empty() {
        f.blocks[entry as usize].term = Term::Jump(default);
        return f;
    }
    // where a failing test of (arm, alt) goes: the arm's next alternative, else the next arm's first test
    let next_arm = |i: usize| -> u32 {
        let mut k = i + 1;
        while k < arms.len() {
            if let Some(&s) = firsts[k].first() {
                return blocks[s];
            }
            k += 1;
        }
        default
    };
    for (n, &(t, i, j)) in flat.iter().enumerate() {
        let last_of_alt = n + 1 == flat.len() || flat[n + 1].1 != i || flat[n + 1].2 != j;
        let on_true = if last_of_alt { targets[i] } else { blocks[n + 1] };
        let on_false = if j + 1 < arms[i].len() { blocks[firsts[i][j + 1]] } else { next_arm(i) };
        let d = f.vreg(Cls::I);
        let b = blocks[n] as usize;
        f.blocks[b].insts.push(Inst::ICmpI(t.cond, t.signed, d, x, t.k));
        f.blocks[b].pos.push(0);
        f.blocks[b].term = Term::Branch(d, on_true, on_false);
    }
    f.verify().unwrap();
    f
}

fn rand_arms(r: &mut Rng, span: u64, signed: bool) -> Vec<Vec<Vec<T>>> {
    let n = 2 + r.below(14) as usize;
    // small values, negative ones, and ones around the 32-bit immediate limits of the back ends
    let base = match r.below(6) {
        0 => -(span as i64) / 2,
        1 => 0x7fff_ff00 + r.below(200) as i64 - 100,
        2 => 0xffff_ff00u32 as i64 + r.below(200) as i64 - 100,
        3 => -0x8000_0000 - r.below(100) as i64,
        4 => i64::MAX - r.below(200) as i64 - span as i64,
        _ => r.below(300) as i64,
    };
    let mut arms = Vec::new();
    for _ in 0..n {
        let nalt = 1 + r.below(3) as usize;
        let mut alts = Vec::new();
        for _ in 0..nalt {
            let k = base + r.below(span) as i64;
            let alt = match r.below(8) {
                0..=3 => vec![T { cond: Cond::Eq, signed, k }],
                4..=5 => {
                    let w = r.below(span / 3 + 1) as i64;
                    vec![T { cond: Cond::Ge, signed, k }, T { cond: Cond::Le, signed, k: k + w }]
                }
                6 => vec![T { cond: if r.below(2) == 0 { Cond::Lt } else { Cond::Gt }, signed, k }],
                _ => vec![T { cond: Cond::Ne, signed, k }, T { cond: Cond::Lt, signed, k: k + 3 }],
            };
            alts.push(alt);
        }
        arms.push(alts);
    }
    arms
}

#[test]
fn switch_lowering_matches_the_compare_chain() {
    let options = relax_session::Options::default();
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let mut switched = 0;
    let mut tables = 0;
    let total = 3000;
    for case in 0..total {
        let span = [12, 40, 70, 300, 3000][case % 5];
        let signed = case % 3 != 0;
        let arms = rand_arms(&mut r, span, signed);
        let f = build(&arms);
        if let Some((g, origin)) = switch::lower_switches(&options, &f) {
            assert_eq!(origin.len(), g.blocks.len());
            g.verify().unwrap();
            switched += 1;
            if g.blocks.iter().any(|b| matches!(b.term, Term::Switch(..))) {
                tables += 1;
            }
        }
        let mut xs: Vec<u64> = vec![0, 1, u64::MAX, u64::MAX - 1, i64::MAX as u64, i64::MIN as u64, (i64::MIN as u64) + 1, 1 << 32, (1 << 32) - 1];
        for v in -400i64..4200 {
            xs.push(v as u64);
        }
        for alts in &arms {
            for alt in alts {
                for t in alt {
                    for d in -2i64..=2 {
                        xs.push(t.k.wrapping_add(d) as u64);
                    }
                }
            }
        }
        for t in arms.iter().flatten().flatten() {
            for off in 0..40i64 {
                xs.push(t.k.wrapping_add(off) as u64);
                xs.push(t.k.wrapping_sub(off) as u64);
            }
        }
        for regs in [false, true] {
            let code = Code::new(&compile(&options, &f, regs)).unwrap();
            let run: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            for &x in &xs {
                assert_eq!(run(x), reference(&arms, x), "case {case} regs {regs} x {}", x as i64);
            }
        }
    }
    // the generator must exercise the lowering, jump tables included
    assert!(switched > total / 4, "only {switched} of {total} chains were lowered");
    assert!(tables > 10, "only {tables} jump tables");
}

/// The same chains inside a loop with many values live around it (spills, spill windows, loop
/// splitting in the thorough allocation): sum over an array of what the chain selects.
fn build_loop(arms: &[Vec<Vec<T>>], live: usize) -> Func {
    let mut f = Func::new("swloop".into(), 0);
    let (p, n) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, n];
    f.rets = vec![Cls::I];
    let entry = f.block();
    let head = f.block();
    let body = f.block();
    let latch = f.block();
    let exit = f.block();
    let push = |f: &mut Func, b: u32, i: Inst| {
        f.blocks[b as usize].insts.push(i);
        f.blocks[b as usize].pos.push(0);
    };
    let i = f.vreg(Cls::I);
    let acc = f.vreg(Cls::I);
    let ws: Vec<VReg> = (0..live).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, entry, Inst::Iconst(i, 0));
    push(&mut f, entry, Inst::Iconst(acc, 0));
    for (k, w) in ws.iter().enumerate() {
        push(&mut f, entry, Inst::Iconst(*w, k as i64 + 1));
    }
    f.blocks[entry as usize].term = Term::Jump(head);
    let c = f.vreg(Cls::I);
    push(&mut f, head, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[head as usize].term = Term::Branch(c, body, exit);
    let off = f.vreg(Cls::I);
    let addr = f.vreg(Cls::I);
    let x = f.vreg(Cls::I);
    push(&mut f, body, Inst::IBinI(IOp::Shl, U64, off, i, 3));
    push(&mut f, body, Inst::IBin(IOp::Add, U64, addr, p, off));
    push(&mut f, body, Inst::Load(Mem::Int(8, false), x, addr, 0));
    // the chain, in blocks after `body`
    let mut flat: Vec<(T, usize, usize)> = Vec::new();
    let mut firsts: Vec<Vec<usize>> = Vec::new();
    for (ai, alts) in arms.iter().enumerate() {
        let mut fa = Vec::new();
        for (j, a) in alts.iter().enumerate() {
            fa.push(flat.len());
            for t in a {
                flat.push((*t, ai, j));
            }
        }
        firsts.push(fa);
    }
    // one leaf block per arm adds 100 + arm to acc; the default adds 7
    let leaf = |f: &mut Func, v: i64| -> u32 {
        let b = f.block();
        let k = f.vreg(Cls::I);
        push(f, b, Inst::Iconst(k, v));
        push(f, b, Inst::IBin(IOp::Add, U64, acc, acc, k));
        f.blocks[b as usize].term = Term::Jump(latch);
        b
    };
    let default = leaf(&mut f, 7);
    let targets: Vec<u32> = (0..arms.len()).map(|ai| leaf(&mut f, 100 + ai as i64)).collect();
    let blocks: Vec<u32> = (0..flat.len()).map(|_| f.block()).collect();
    if flat.is_empty() {
        f.blocks[body as usize].term = Term::Jump(default);
    } else {
        f.blocks[body as usize].term = Term::Jump(blocks[0]);
        let next_arm = |ai: usize| -> u32 {
            let mut k = ai + 1;
            while k < arms.len() {
                if let Some(&s) = firsts[k].first() {
                    return blocks[s];
                }
                k += 1;
            }
            default
        };
        for (nn, &(t, ai, j)) in flat.iter().enumerate() {
            let last_of_alt = nn + 1 == flat.len() || flat[nn + 1].1 != ai || flat[nn + 1].2 != j;
            let on_true = if last_of_alt { targets[ai] } else { blocks[nn + 1] };
            let on_false = if j + 1 < arms[ai].len() { blocks[firsts[ai][j + 1]] } else { next_arm(ai) };
            let d = f.vreg(Cls::I);
            let b = blocks[nn] as usize;
            f.blocks[b].insts.push(Inst::ICmpI(t.cond, t.signed, d, x, t.k));
            f.blocks[b].pos.push(0);
            f.blocks[b].term = Term::Branch(d, on_true, on_false);
        }
    }
    // the latch: the live values change with x so they stay live and distinct
    for (k, w) in ws.iter().enumerate() {
        let t = f.vreg(Cls::I);
        push(&mut f, latch, Inst::IBinI(IOp::Mul, U64, t, *w, 3 + k as i64));
        push(&mut f, latch, Inst::IBin(IOp::Add, U64, *w, t, x));
    }
    push(&mut f, latch, Inst::IBinI(IOp::Add, U64, i, i, 1));
    f.blocks[latch as usize].term = Term::Jump(head);
    // exit: acc plus all live values
    let mut sum = acc;
    for w in &ws {
        let t = f.vreg(Cls::I);
        push(&mut f, exit, Inst::IBin(IOp::Xor, U64, t, sum, *w));
        sum = t;
    }
    f.blocks[exit as usize].term = Term::Ret(vec![sum]);
    f.verify().unwrap();
    f
}

#[test]
fn switch_lowering_in_loops_under_register_pressure() {
    let options = relax_session::Options::default();
    let mut r = Rng(0x1234_5678_9abc_def1);
    for case in 0..300 {
        let span = [12, 40, 70, 300][case % 4];
        let signed = case % 3 != 0;
        let arms = rand_arms(&mut r, span, signed);
        let live = [3, 14, 40][case % 3];
        let f = build_loop(&arms, live);
        let xs: Vec<u64> = (0..200).map(|k| if k % 17 == 0 { (r.next() as i64 >> 40) as u64 } else { (r.below(span * 2) as i64 - span as i64 / 2) as u64 }).collect();
        // reference
        let mut acc = 0u64;
        let mut ws: Vec<u64> = (0..live as u64).map(|k| k + 1).collect();
        for &x in &xs {
            acc = acc.wrapping_add(reference(&arms, x));
            for (k, w) in ws.iter_mut().enumerate() {
                *w = w.wrapping_mul(3 + k as u64).wrapping_add(x);
            }
        }
        let mut sum = acc;
        for w in &ws {
            sum ^= *w;
        }
        for regs in [false, true] {
            let code = Code::new(&compile(&options, &f, regs)).unwrap();
            let run: extern "C" fn(*const u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            assert_eq!(run(xs.as_ptr(), xs.len() as u64), sum, "case {case} regs {regs} live {live}");
        }
    }
}

/// A multi-way terminator's targets all have the switch block as their immediate dominator.
#[test]
fn dominators_see_every_switch_target() {
    let mut f = Func::new("dom".into(), 0);
    let x = f.vreg(Cls::I);
    f.params = vec![x];
    f.rets = vec![Cls::I];
    let head = f.block();
    let leaves: Vec<u32> = (0..5).map(|_| f.block()).collect();
    for &l in &leaves {
        f.blocks[l as usize].term = Term::Ret(vec![x]);
    }
    f.blocks[head as usize].term = Term::Switch(x, std::sync::Arc::new(SwitchTable::new(0, leaves[..4].to_vec(), leaves[4])));
    f.verify().unwrap();
    let idom = relax_backend::dom::idoms(&f);
    for &l in &leaves {
        assert_eq!(idom[l as usize], head, "block {l}");
    }
}
