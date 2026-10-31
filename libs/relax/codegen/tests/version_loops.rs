//! version_bounds_loops: a counted loop's bounds checks proved by one guard in front of a
//! check-free clone, the original loop the fallback. Each case runs the RIR before and after the
//! pass through a small evaluator over the same inputs, including lengths that make the guard
//! fail and inputs that panic, and requires identical memory, results and panic (index, len,
//! number of stores done before it).
use relax_backend::regalloc::reachable;
use relax_backend::rir::*;
use relax_codegen::version;

const U64: IntTy = IntTy { bits: 64, signed: false };

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(1);
}
fn term(f: &mut Func, b: u32, t: Term) {
    f.blocks[b as usize].term = t;
}

#[derive(Debug, PartialEq)]
enum Out {
    Ret(Vec<u64>, Vec<u8>, usize),
    Panic(u64, u64, Vec<u8>, usize),
}

// Blocks executed past the original function's 8 (axpy shapes): the clone ran.
thread_local! { static CLONE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Integer-only evaluator: memory is one byte vector, pointers are offsets into it. A Host call
/// is the bounds panic (index, len). Returns the result, the memory and the number of stores.
fn eval(f: &Func, args: &[u64], mem0: &[u8]) -> Out {
    let mut mem = mem0.to_vec();
    let mut r = vec![0u64; f.vregs.len()];
    for (p, a) in f.params.iter().zip(args) {
        r[p.0 as usize] = *a;
    }
    let mut stores = 0usize;
    let mut b = 0usize;
    let mut steps = 0u64;
    loop {
        steps += 1;
        assert!(steps < 10_000_000, "runaway");
        if b > 8 {
            // past axpy's 8 blocks and the guard: a block the versioning added (the clone) ran
            CLONE_STEPS.with(|c| c.set(c.get() + 1));
        }
        for i in &f.blocks[b].insts {
            match *i {
                Inst::Iconst(d, k) => r[d.0 as usize] = k as u64,
                Inst::Mov(d, s) => r[d.0 as usize] = r[s.0 as usize],
                Inst::Sel(d, c, a, b2) => r[d.0 as usize] = if r[c.0 as usize] != 0 { r[a.0 as usize] } else { r[b2.0 as usize] },
                Inst::Conv(Conv::IntToInt(t), d, s) => {
                    let v = r[s.0 as usize];
                    r[d.0 as usize] = if t.bits >= 64 { v } else if t.signed { (((v << (64 - t.bits)) as i64) >> (64 - t.bits)) as u64 } else { v & ((1u64 << t.bits) - 1) };
                }
                Inst::IBin(op, it, d, a, c) => r[d.0 as usize] = typed(it, op, r[a.0 as usize], r[c.0 as usize]),
                Inst::IBinI(op, it, d, a, k) => r[d.0 as usize] = typed(it, op, r[a.0 as usize], k as u64),
                Inst::ICmp(cc, s, d, a, c) => r[d.0 as usize] = cmp(cc, s, r[a.0 as usize], r[c.0 as usize]) as u64,
                Inst::ICmpI(cc, s, d, a, k) => r[d.0 as usize] = cmp(cc, s, r[a.0 as usize], k as u64) as u64,
                Inst::Load(Mem::Int(4, false), d, a, off) => {
                    let p = (r[a.0 as usize] as i64 + off as i64) as usize;
                    r[d.0 as usize] = u32::from_le_bytes(mem[p..p + 4].try_into().unwrap()) as u64;
                }
                Inst::Store(Mem::Int(4, _), a, off, s) => {
                    let p = (r[a.0 as usize] as i64 + off as i64) as usize;
                    mem[p..p + 4].copy_from_slice(&(r[s.0 as usize] as u32).to_le_bytes());
                    stores += 1;
                }
                Inst::Call(Callee::Host(_), ref a, _) => {
                    return Out::Panic(r[a[0].0 as usize], r[a[1].0 as usize], mem, stores);
                }
                ref other => panic!("eval: {other:?}"),
            }
        }
        match &f.blocks[b].term {
            Term::Jump(t) => b = *t as usize,
            Term::Branch(c, t, e) => b = if r[c.0 as usize] != 0 { *t as usize } else { *e as usize },
            Term::Ret(vs) => return Out::Ret(vs.iter().map(|v| r[v.0 as usize]).collect(), mem, stores),
            t => panic!("eval: {t:?}"),
        }
    }
}

/// an operation of a type: unsigned widths below 64 wrap (the result zero-extended); rotates and
/// remainders at the type's width
fn typed(it: IntTy, op: IOp, a: u64, b: u64) -> u64 {
    if it.bits >= 64 || it.signed {
        return bin(op, a, b);
    }
    let m = (1u64 << it.bits) - 1;
    match op {
        IOp::Rotl => {
            let (a, k) = (a & m, b % it.bits as u64);
            ((a << k) | (a >> ((it.bits as u64 - k) % it.bits as u64))) & m
        }
        _ => bin(op, a, b) & m,
    }
}

fn bin(op: IOp, a: u64, b: u64) -> u64 {
    match op {
        IOp::Rem => a % b,
        IOp::Div => a / b,
        IOp::Rotl => a.rotate_left(b as u32),
        IOp::Add => a.wrapping_add(b),
        IOp::Sub => a.wrapping_sub(b),
        IOp::Mul => a.wrapping_mul(b),
        IOp::And => a & b,
        IOp::Or => a | b,
        IOp::Xor => a ^ b,
        IOp::Shl => a << (b & 63),
        IOp::Shr => a >> (b & 63),
        _ => panic!("bin {op:?}"),
    }
}

fn cmp(c: Cond, s: bool, a: u64, b: u64) -> bool {
    if s {
        let (a, b) = (a as i64, b as i64);
        return match c {
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
            Cond::Eq => a == b,
            Cond::Ne => a != b,
        };
    }
    match c {
        Cond::Lt => a < b,
        Cond::Le => a <= b,
        Cond::Gt => a > b,
        Cond::Ge => a >= b,
        Cond::Eq => a == b,
        Cond::Ne => a != b,
    }
}

/// `for i in start..n { y[i] = y[i] + x[i + k] * 3 }` with both checks (y: i < ylen, x: i + k < xlen),
/// in the shape fold_ranges leaves (`t = i; i = i + 1` after the header's test).
/// params: y, ylen, x, xlen, n, start
fn axpy(k: i64, check_y: bool) -> Func {
    axpy_sym(k, check_y, false)
}

/// as axpy; `sym`: the x index is `(t + m) - w + k` with m, w two more params (a 2D stencil's
/// `y * w + x - w`)
fn axpy_sym(k: i64, check_y: bool, sym: bool) -> Func {
    let mut f = Func::new("axpy".into(), 0);
    let v: Vec<VReg> = (0..8).map(|_| f.vreg(Cls::I)).collect();
    let (y, ylen, x, xlen, n, start) = (v[0], v[1], v[2], v[3], v[4], v[5]);
    f.params.extend([y, ylen, x, xlen, n, start, v[6], v[7]]);
    for _ in 0..8 {
        f.block();
    }
    let (i, c, t, i2, cy, xi, cx, off, offx, yv, xv, m, s, ya, xa) =
        (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, 0, Inst::Mov(i, start));
    term(&mut f, 0, Term::Jump(1));
    // b1: header test
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c, i, n));
    term(&mut f, 1, Term::Branch(c, 2, 5));
    // b2: t = i; i = i + 1; y check
    push(&mut f, 2, Inst::Mov(t, i));
    push(&mut f, 2, Inst::IBinI(IOp::Add, U64, i2, i, 1));
    push(&mut f, 2, Inst::Mov(i, i2));
    if check_y {
        push(&mut f, 2, Inst::ICmp(Cond::Lt, false, cy, t, ylen));
        term(&mut f, 2, Term::Branch(cy, 3, 6));
    } else {
        term(&mut f, 2, Term::Jump(3));
    }
    // b3: x check of t + k
    let tk = if sym {
        let (a1, a2) = (f.vreg(Cls::I), f.vreg(Cls::I));
        push(&mut f, 3, Inst::IBin(IOp::Add, U64, a1, v[6], t));
        push(&mut f, 3, Inst::IBin(IOp::Sub, U64, a2, a1, v[7]));
        a2
    } else {
        t
    };
    if k >= 0 {
        push(&mut f, 3, Inst::IBinI(IOp::Add, U64, xi, tk, k));
    } else {
        push(&mut f, 3, Inst::IBinI(IOp::Sub, U64, xi, tk, -k));
    }
    push(&mut f, 3, Inst::ICmp(Cond::Lt, false, cx, xi, xlen));
    term(&mut f, 3, Term::Branch(cx, 4, 7));
    // b4: body
    push(&mut f, 4, Inst::IBinI(IOp::Shl, U64, off, t, 2));
    push(&mut f, 4, Inst::IBinI(IOp::Shl, U64, offx, xi, 2));
    push(&mut f, 4, Inst::IBin(IOp::Add, U64, ya, y, off));
    push(&mut f, 4, Inst::IBin(IOp::Add, U64, xa, x, offx));
    push(&mut f, 4, Inst::Load(Mem::Int(4, false), yv, ya, 0));
    push(&mut f, 4, Inst::Load(Mem::Int(4, false), xv, xa, 0));
    push(&mut f, 4, Inst::IBinI(IOp::Mul, U64, m, xv, 3));
    push(&mut f, 4, Inst::IBin(IOp::Add, U64, s, yv, m));
    push(&mut f, 4, Inst::Store(Mem::Int(4, false), ya, 0, s));
    term(&mut f, 4, Term::Jump(1));
    // b5: exit, returns the counter (a value read after the loop)
    f.rets.push(Cls::I);
    term(&mut f, 5, Term::Ret(vec![i]));
    for (b, (a0, a1)) in [(6u32, (t, ylen)), (7u32, (xi, xlen))] {
        let sym = f.vreg(Cls::I);
        push(&mut f, b, Inst::Iconst(sym, 0));
        push(&mut f, b, Inst::Call(Callee::Host(Sym { id: 5, off: 0 }), vec![a0, a1, sym], Vec::new()));
        term(&mut f, b, Term::Unreachable);
    }
    f
}

fn run_cases(orig: &Func, new: &Func) -> usize {
    run_cases_mw(orig, new, &[(0, 0)])
}

fn run_cases_mw(orig: &Func, new: &Func, mws: &[(u64, u64)]) -> usize {
    // memory: y at 0 (64 u32), x at 256 (64 u32)
    let mut mem = vec![0u8; 512];
    for j in 0..128u32 {
        mem[j as usize * 4..j as usize * 4 + 4].copy_from_slice(&(j * 7 + 1).to_le_bytes());
    }
    let mut cases = 0;
    let mut panics = 0;
    for ylen in [0u64, 1, 5, 20, 40, 64] {
        for xlen in [0u64, 3, 19, 20, 21, 40, 64] {
            for n in [0u64, 1, 4, 19, 20, 21, 40, 64] {
                for start in [0u64, 1, 2, 3, 30] {
                    for &(m, w) in mws {
                        let args = [0, ylen, 256, xlen, n, start, m, w];
                        let a = eval(orig, &args, &mem);
                        let b = eval(new, &args, &mem);
                        assert_eq!(a, b, "ylen {ylen} xlen {xlen} n {n} start {start} m {m} w {w}");
                        if matches!(a, Out::Panic(..)) {
                            panics += 1;
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    assert!(panics > 0 && panics < cases, "the cases cover both panicking and clean runs");
    cases
}

fn clone_has_no_checks(f: &Func) -> bool {
    // some reachable loop runs without any branch to an Unreachable block
    let reach = reachable(f);
    let cold = |b: u32| matches!(f.blocks[b as usize].term, Term::Unreachable);
    let reach_loop_blocks: Vec<usize> = (0..f.blocks.len()).filter(|&b| reach[b]).collect();
    reach_loop_blocks.iter().any(|&b| matches!(f.blocks[b].term, Term::Jump(t) if !cold(t)))
        && f.blocks.len() > 8
}

fn versioned(mut f: Func) -> (Func, Func) {
    let orig = f.clone();
    let changed = version::version_bounds_loops(&relax_session::Options::default(), &mut f);
    assert!(changed, "versioned");
    (orig, f)
}

#[test]
fn axpy_two_checks_offset_zero() {
    let (orig, new) = versioned(axpy(0, true));
    assert!(clone_has_no_checks(&new));
    // the clone's two checks are gone: count Branch terms to cold blocks in appended blocks
    let cold = |b: u32| matches!(new.blocks[b as usize].term, Term::Unreachable);
    let clone_checks = new.blocks[8..].iter().filter(|b| matches!(b.term, Term::Branch(_, t, e) if cold(t) || cold(e))).count();
    assert_eq!(clone_checks, 0);
    run_cases(&orig, &new);
}

#[test]
fn stencil_positive_offset() {
    let (orig, new) = versioned(axpy(2, true));
    run_cases(&orig, &new);
}

#[test]
fn negative_offset_needs_start() {
    let (orig, new) = versioned(axpy(-1, false));
    run_cases(&orig, &new);
}

/// The clone renames the counter copy t although the original's panic block reads it (the clone
/// cannot reach that block): later single-def passes and the vectoriser see a loop-local t.
#[test]
fn clone_renames_values_only_cold_blocks_read() {
    let (_, new) = versioned(axpy(0, true));
    let t = match new.blocks[2].insts[0] {
        Inst::Mov(d, _) => d,
        _ => unreachable!(),
    };
    let clone_defines_t = new.blocks[8..].iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Mov(d, _) if *d == t)));
    assert!(!clone_defines_t);
}

/// The index copy also defined after the loop (a storage-dead zero, as lowering emits): the check
/// is still proved, and the clone still renames it.
#[test]
fn outside_def_of_index_copy() {
    let mut f = axpy(0, true);
    let t = match f.blocks[2].insts[0] {
        Inst::Mov(d, _) => d,
        _ => unreachable!(),
    };
    f.blocks[5].insts.insert(0, Inst::Iconst(t, 0));
    f.blocks[5].pos.insert(0, 1);
    let (orig, new) = versioned(f);
    let cold = |b: u32| matches!(new.blocks[b as usize].term, Term::Unreachable);
    let clone_checks = new.blocks[8..].iter().filter(|b| matches!(b.term, Term::Branch(_, t, e) if cold(t) || cold(e))).count();
    assert_eq!(clone_checks, 0);
    run_cases(&orig, &new);
}

/// Offsets with loop-invariant vregs (2D stencil rows): positive, negative and wrapping offsets.
#[test]
fn symbolic_offsets() {
    for k in [0i64, 1, -1, 3] {
        let (orig, new) = versioned(axpy_sym(k, true, true));
        let mws: Vec<(u64, u64)> = vec![(0, 0), (5, 0), (0, 5), (5, 5), (8, 3), (3, 8), (0, 1), (1, 0), (40, 2), (u64::MAX, 0), (0, u64::MAX), (u64::MAX - 3, 0)];
        run_cases_mw(&orig, &new, &mws);
    }
}

/// A counter stepping by 2 (or 3): negative constant and symbolic offsets get the end-based guard
/// (`i0 + o <= n + o <= len`, `n <= MAX - step`), the results equal the checked loop's.
#[test]
fn stepped_counter_offsets() {
    for step in [2i64, 3] {
        for (k, sym) in [(-1i64, false), (-2, false), (0, true), (-1, true), (2, true)] {
            let mut f = axpy_sym(k, true, sym);
            for i in f.blocks[2].insts.iter_mut() {
                if let Inst::IBinI(IOp::Add, _, _, _, s) = i {
                    *s = step;
                }
            }
            let (orig, new) = versioned(f);
            let mws: Vec<(u64, u64)> = if sym { vec![(0, 0), (5, 0), (0, 5), (5, 5), (8, 3), (3, 8), (u64::MAX, 0), (0, u64::MAX)] } else { vec![(0, 0)] };
            let before = CLONE_STEPS.with(|c| c.get());
            run_cases_mw(&orig, &new, &mws);
            assert!(CLONE_STEPS.with(|c| c.get()) > before, "the guard never chose the clone (step {step} k {k} sym {sym})");
        }
    }
}

#[test]
fn not_counted_left_alone() {
    // the bound redefined in the loop: no counted loop, nothing versioned
    let mut f = axpy(0, true);
    let n = f.params[4];
    push(&mut f, 4, Inst::IBinI(IOp::Add, U64, n, n, 0));
    let before = f.blocks.len();
    assert!(!version::version_bounds_loops(&relax_session::Options::default(), &mut f));
    assert_eq!(f.blocks.len(), before);
}

#[test]
fn stale_read_after_increment_not_proved() {
    // the x index reads i after the increment (i + 0 is the next value): it may equal n, so the
    // check stays in the clone; the y check (fresh t) is still versioned
    let mut f = axpy(0, true);
    let i = match f.blocks[0].insts[0] {
        Inst::Mov(d, _) => d,
        _ => unreachable!(),
    };
    let xi = match f.blocks[3].insts[0] {
        Inst::IBinI(_, _, d, _, _) => d,
        _ => unreachable!(),
    };
    f.blocks[3].insts[0] = Inst::IBinI(IOp::Add, U64, xi, i, 0);
    let (orig, new) = versioned(f);
    let cold = |b: u32| matches!(new.blocks[b as usize].term, Term::Unreachable);
    let clone_checks = new.blocks[8..].iter().filter(|b| matches!(b.term, Term::Branch(_, t, e) if cold(t) || cold(e))).count();
    assert_eq!(clone_checks, 1);
    // memory comparison: the x read at i reaches index n, inside the 128-word buffer
    run_cases(&orig, &new);
}

/// The clone of `y[i] += k * x[i]` after rotation (one block): the stream-map vectoriser must take it.
#[test]
fn clone_shape_vectorises() {
    let mut f = Func::new("axpy_clone".into(), 0);
    let p: Vec<VReg> = (0..4).map(|_| f.vreg(Cls::I)).collect();
    let k = f.vreg(Cls::F32);
    let (y, ylen, x, _xlen) = (p[0], p[1], p[2], p[3]);
    f.params.extend([p[0], p[1], p[2], p[3], k]);
    for _ in 0..4 {
        f.block();
    }
    let i = f.vreg(Cls::I);
    let (c0, t, i2, off, xa, xv, m, ya, yv, s, c1) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::F32), f.vreg(Cls::F32), f.vreg(Cls::I), f.vreg(Cls::F32), f.vreg(Cls::F32), f.vreg(Cls::I));
    push(&mut f, 0, Inst::Iconst(i, 0));
    push(&mut f, 0, Inst::ICmp(Cond::Lt, false, c0, i, ylen));
    term(&mut f, 0, Term::Branch(c0, 1, 2));
    push(&mut f, 1, Inst::Mov(t, i));
    push(&mut f, 1, Inst::IBinI(IOp::Add, U64, i2, i, 1));
    push(&mut f, 1, Inst::Mov(i, i2));
    push(&mut f, 1, Inst::IBinI(IOp::Shl, U64, off, t, 2));
    push(&mut f, 1, Inst::IBin(IOp::Add, U64, xa, x, off));
    push(&mut f, 1, Inst::Load(Mem::F32, xv, xa, 0));
    push(&mut f, 1, Inst::FBin(FOp::Mul, false, m, k, xv));
    push(&mut f, 1, Inst::IBin(IOp::Add, U64, ya, y, off));
    push(&mut f, 1, Inst::Load(Mem::F32, yv, ya, 0));
    push(&mut f, 1, Inst::FBin(FOp::Add, false, s, yv, m));
    push(&mut f, 1, Inst::Store(Mem::F32, ya, 0, s));
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c1, i, ylen));
    term(&mut f, 1, Term::Branch(c1, 1, 2));
    term(&mut f, 2, Term::Ret(vec![]));
    let mut g = f.clone();
    let n0 = f.blocks.len();
    let ch = relax_codegen::loops::vectorize_reductions(&relax_session::Options::default(), &mut f);
    assert!(ch && f.blocks.len() > n0, "vectorised");
    // t read in an unreachable block outside the loop (an original loop's panic block)
    let b = g.block();
    push(&mut g, b, Inst::Call(Callee::Host(Sym { id: 5, off: 0 }), vec![t, ylen], Vec::new()));
    term(&mut g, b, Term::Unreachable);
    let n0 = g.blocks.len();
    let ch = relax_codegen::loops::vectorize_reductions(&relax_session::Options::default(), &mut g);
    eprintln!("with outside read of t: vectorised {}", ch && g.blocks.len() > n0);
}

/// `for i in 0..n { y[i] += if flag { 1 } else { x[i] } }` with flag a param: two loops, chosen once.
#[test]
fn unswitch_invariant_branch() {
    let mut f = Func::new("unsw".into(), 0);
    let p: Vec<VReg> = (0..5).map(|_| f.vreg(Cls::I)).collect();
    let (y, x, n, flag, _) = (p[0], p[1], p[2], p[3], p[4]);
    f.params.extend([p[0], p[1], p[2], p[3]]);
    for _ in 0..6 {
        f.block();
    }
    let (i, c, off, ya, yv, xa, xv, s, one, z) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, 0, Inst::Iconst(i, 0));
    term(&mut f, 0, Term::Jump(1));
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c, i, n));
    term(&mut f, 1, Term::Branch(c, 2, 5));
    push(&mut f, 2, Inst::IBinI(IOp::Shl, U64, off, i, 2));
    push(&mut f, 2, Inst::IBin(IOp::Add, U64, ya, y, off));
    push(&mut f, 2, Inst::Load(Mem::Int(4, false), yv, ya, 0));
    term(&mut f, 2, Term::Branch(flag, 3, 4));
    push(&mut f, 3, Inst::Iconst(one, 1));
    push(&mut f, 3, Inst::IBin(IOp::Add, U64, s, yv, one));
    push(&mut f, 3, Inst::Store(Mem::Int(4, false), ya, 0, s));
    push(&mut f, 3, Inst::IBinI(IOp::Add, U64, i, i, 1));
    term(&mut f, 3, Term::Jump(1));
    push(&mut f, 4, Inst::IBin(IOp::Add, U64, xa, x, off));
    push(&mut f, 4, Inst::Load(Mem::Int(4, false), xv, xa, 0));
    push(&mut f, 4, Inst::IBin(IOp::Add, U64, z, yv, xv));
    push(&mut f, 4, Inst::Store(Mem::Int(4, false), ya, 0, z));
    push(&mut f, 4, Inst::IBinI(IOp::Add, U64, i, i, 1));
    term(&mut f, 4, Term::Jump(1));
    f.rets.push(Cls::I);
    term(&mut f, 5, Term::Ret(vec![i]));
    let orig = f.clone();
    assert!(version::unswitch_loops(&relax_session::Options::default(), &mut f));
    // neither copy branches on flag any more
    let reach = reachable(&f);
    assert!(!f.blocks.iter().enumerate().any(|(b, bl)| reach[b] && matches!(bl.term, Term::Branch(c, _, _) if c == flag)));
    let mut mem = vec![0u8; 512];
    for j in 0..128u32 {
        mem[j as usize * 4..j as usize * 4 + 4].copy_from_slice(&(j * 3 + 2).to_le_bytes());
    }
    for n in [0u64, 1, 7, 64] {
        for fl in [0u64, 1, 2] {
            let args = [0, 256, n, fl];
            assert_eq!(eval(&orig, &args, &mem), eval(&f, &args, &mem), "n {n} flag {fl}");
        }
    }
}

/// `out[i] = a[i].wrapping_mul(3) ^ b[i] >> 1` (u32) after versioning and rotation: one block, a
/// constant shift among the lane operations; the stream-map vectoriser takes it.
#[test]
fn u32_map_with_shift_vectorises() {
    let mut f = Func::new("mix_clone".into(), 0);
    let p: Vec<VReg> = (0..4).map(|_| f.vreg(Cls::I)).collect();
    let (out, a, b, n) = (p[0], p[1], p[2], p[3]);
    f.params.extend([out, a, b, n]);
    for _ in 0..3 {
        f.block();
    }
    let i = f.vreg(Cls::I);
    let (c0, t, i2, off, aa, av, m, ba, bv, sh, x, oa, c1) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let u32t = IntTy { bits: 32, signed: false };
    push(&mut f, 0, Inst::Iconst(i, 0));
    push(&mut f, 0, Inst::ICmp(Cond::Lt, false, c0, i, n));
    term(&mut f, 0, Term::Branch(c0, 1, 2));
    push(&mut f, 1, Inst::Mov(t, i));
    push(&mut f, 1, Inst::IBinI(IOp::Add, U64, i2, i, 1));
    push(&mut f, 1, Inst::Mov(i, i2));
    push(&mut f, 1, Inst::IBinI(IOp::Shl, U64, off, t, 2));
    push(&mut f, 1, Inst::IBin(IOp::Add, U64, aa, a, off));
    push(&mut f, 1, Inst::Load(Mem::Int(4, false), av, aa, 0));
    push(&mut f, 1, Inst::IBinI(IOp::Mul, u32t, m, av, 3));
    push(&mut f, 1, Inst::IBin(IOp::Add, U64, ba, b, off));
    push(&mut f, 1, Inst::Load(Mem::Int(4, false), bv, ba, 0));
    push(&mut f, 1, Inst::IBinI(IOp::Shr, u32t, sh, bv, 1));
    push(&mut f, 1, Inst::IBin(IOp::Xor, u32t, x, m, sh));
    push(&mut f, 1, Inst::IBin(IOp::Add, U64, oa, out, off));
    push(&mut f, 1, Inst::Store(Mem::Int(4, false), oa, 0, x));
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c1, i, n));
    term(&mut f, 1, Term::Branch(c1, 1, 2));
    term(&mut f, 2, Term::Ret(vec![]));
    let n0 = f.blocks.len();
    let ch = relax_codegen::loops::vectorize_reductions(&relax_session::Options::default(), &mut f);
    assert!(ch && f.blocks.len() > n0, "vectorised");
    assert!(f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::VX(VX::ShrUI(_, 1), ..)))));
}

/// `base = if a.cmp(&b) == Greater { base } else { mid }` (binary_search's step): the Ordering is a
/// diamond (`-1` on one arm, `a == b ? 0 : 1` on the other) narrowed to i8 and compared with 1.
/// simplify must leave no branch (the arms are speculated, the compare distributed over the
/// Ordering's assignments) and compute the same results.
#[test]
fn ordering_diamond_becomes_selects() {
    let mut f = Func::new("ord".into(), 0);
    let p: Vec<VReg> = (0..4).map(|_| f.vreg(Cls::I)).collect();
    f.params.extend(p.iter().copied());
    let (a, b, base, mid) = (p[0], p[1], p[2], p[3]);
    for _ in 0..4 {
        f.block();
    }
    let (c, o, e, z, one, t, o8, g, r) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, 0, Inst::Iconst(z, 0));
    push(&mut f, 0, Inst::Iconst(one, 1));
    push(&mut f, 0, Inst::ICmp(Cond::Lt, false, c, a, b));
    term(&mut f, 0, Term::Branch(c, 1, 2));
    push(&mut f, 1, Inst::Iconst(o, -1));
    term(&mut f, 1, Term::Jump(3));
    push(&mut f, 2, Inst::ICmp(Cond::Eq, false, e, a, b));
    push(&mut f, 2, Inst::Sel(t, e, z, one));
    push(&mut f, 2, Inst::Mov(o, t));
    term(&mut f, 2, Term::Jump(3));
    push(&mut f, 3, Inst::Conv(Conv::IntToInt(IntTy { bits: 8, signed: true }), o8, o));
    push(&mut f, 3, Inst::ICmpI(Cond::Eq, true, g, o8, 1));
    push(&mut f, 3, Inst::Sel(r, g, base, mid));
    f.rets.push(Cls::I);
    term(&mut f, 3, Term::Ret(vec![r]));
    let orig = f.clone();
    relax_backend::passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
    relax_backend::passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
    let reach = reachable(&f);
    let branches = f.blocks.iter().enumerate().filter(|(bi, bl)| reach[*bi] && matches!(bl.term, Term::Branch(..))).count();
    for x in [0u64, 1, 5, 9, u64::MAX] {
        for y in [0u64, 1, 5, 9, u64::MAX] {
            let args = [x, y, 100, 200];
            assert_eq!(eval(&orig, &args, &[]), eval(&f, &args, &[]), "a {x} b {y}");
        }
    }
    // one compare (`a <= b`) and one select remain
    let live: usize = f.blocks.iter().enumerate().filter(|(bi, _)| reach[*bi]).map(|(_, b)| b.insts.len()).sum();
    assert!(live <= 2, "left: {:?}", f.blocks.iter().map(|b| &b.insts).collect::<Vec<_>>());
    assert_eq!(branches, 0, "branch left:\n{:?}", f.blocks.iter().map(|b| (&b.insts, &b.term)).collect::<Vec<_>>());
}

/// The same step once the Ordering is already a select tree (`a < b ? -1 : (a == b ? 0 : 1)`,
/// narrowed to i8, compared with 1): one compare and one select remain.
#[test]
fn ordering_select_tree_folds() {
    let mut f = Func::new("ord2".into(), 0);
    let p: Vec<VReg> = (0..4).map(|_| f.vreg(Cls::I)).collect();
    f.params.extend(p.iter().copied());
    let (a, b, base, mid) = (p[0], p[1], p[2], p[3]);
    f.block();
    let v: Vec<VReg> = (0..10).map(|_| f.vreg(Cls::I)).collect();
    let (m1, z, one, lt, eq, t, o, o8, g, r) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9]);
    push(&mut f, 0, Inst::Iconst(m1, -1));
    push(&mut f, 0, Inst::Iconst(z, 0));
    push(&mut f, 0, Inst::Iconst(one, 1));
    push(&mut f, 0, Inst::ICmp(Cond::Lt, false, lt, a, b));
    push(&mut f, 0, Inst::ICmp(Cond::Eq, false, eq, a, b));
    push(&mut f, 0, Inst::Sel(t, eq, z, one));
    push(&mut f, 0, Inst::Sel(o, lt, m1, t));
    push(&mut f, 0, Inst::Conv(Conv::IntToInt(IntTy { bits: 8, signed: true }), o8, o));
    push(&mut f, 0, Inst::ICmpI(Cond::Eq, true, g, o8, 1));
    push(&mut f, 0, Inst::Sel(r, g, base, mid));
    f.rets.push(Cls::I);
    term(&mut f, 0, Term::Ret(vec![r]));
    let orig = f.clone();
    for _ in 0..2 {
        relax_backend::passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
    }
    for x in [0u64, 1, 5, 9, u64::MAX] {
        for y in [0u64, 1, 5, 9, u64::MAX] {
            let args = [x, y, 100, 200];
            assert_eq!(eval(&orig, &args, &[]), eval(&f, &args, &[]), "a {x} b {y}");
        }
    }
    let live: usize = f.blocks.iter().map(|b| b.insts.len()).sum();
    assert!(live <= 2, "left: {:?}", f.blocks.iter().map(|b| &b.insts).collect::<Vec<_>>());
}

/// `x % c == 0` and `x % c != 0` (u32 and u64) become a multiply by c's inverse (and a rotate for even
/// c) and an unsigned compare; exact for every tested x.
#[test]
fn divisibility_tests_fold() {
    for bits in [32u8, 64] {
        for c in [3u64, 5, 6, 7, 10, 12, 24, 100, 641, 1000, 65535, 0x7fff_ffff] {
            for eq in [true, false] {
                let it = IntTy { bits, signed: false };
                let mut f = Func::new("rem".into(), 0);
                let x = f.vreg(Cls::I);
                f.params.push(x);
                f.block();
                let (kc, r, d) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
                push(&mut f, 0, Inst::Iconst(kc, c as i64));
                push(&mut f, 0, Inst::IBin(IOp::Rem, it, r, x, kc));
                push(&mut f, 0, Inst::ICmpI(if eq { Cond::Eq } else { Cond::Ne }, false, d, r, 0));
                f.rets.push(Cls::I);
                term(&mut f, 0, Term::Ret(vec![d]));
                let orig = f.clone();
                relax_backend::passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
                assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::IBin(IOp::Rem | IOp::MulHi, ..))), "c {c} bits {bits}: {:?}", f.blocks[0].insts);
                let m = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
                let mut xs: Vec<u64> = vec![0, 1, 2, c - 1, c, c + 1, 2 * c, 3 * c - 1, m, m - 1, m / c * c, m / c * c + 1];
                let mut s = 0x9E37_79B9_7F4A_7C15u64;
                for _ in 0..2000 {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    xs.push(s & m);
                    xs.push((s % 1000) * c & m);
                }
                for xv in xs.iter().map(|v| v & m) {
                    assert_eq!(eval(&orig, &[xv], &[]), eval(&f, &[xv], &[]), "x {xv} c {c} bits {bits} eq {eq}");
                }
            }
        }
    }
}

/// `c ? 1 : 0` and `c ? 0 : 1` of a compare: c and c ^ 1 (no select left); x / 1 is x and x % 1 is 0.
#[test]
fn bool_selects_and_unit_divisors_fold() {
    for (k1, k0) in [(1i64, 0i64), (0, 1)] {
        let mut f = Func::new("sel01".into(), 0);
        let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([a, b]);
        f.block();
        let v: Vec<VReg> = (0..8).map(|_| f.vreg(Cls::I)).collect();
        push(&mut f, 0, Inst::Iconst(v[0], k1));
        push(&mut f, 0, Inst::Iconst(v[1], k0));
        push(&mut f, 0, Inst::ICmp(Cond::Lt, false, v[2], a, b));
        push(&mut f, 0, Inst::Sel(v[3], v[2], v[0], v[1]));
        push(&mut f, 0, Inst::Iconst(v[4], 1));
        push(&mut f, 0, Inst::IBin(IOp::Div, U64, v[5], a, v[4]));
        push(&mut f, 0, Inst::IBin(IOp::Rem, IntTy { bits: 32, signed: true }, v[6], b, v[4]));
        push(&mut f, 0, Inst::IBin(IOp::Add, U64, v[7], v[3], v[5]));
        f.rets.extend([Cls::I, Cls::I]);
        term(&mut f, 0, Term::Ret(vec![v[7], v[6]]));
        let orig = f.clone();
        relax_backend::passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
        assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Sel(..) | Inst::IBin(IOp::Div | IOp::Rem, ..))), "{:?}", f.blocks[0].insts);
        for x in [0u64, 1, 5, 9, u64::MAX] {
            for y in [0u64, 1, 5, 9, (-3i64) as u64] {
                assert_eq!(eval(&orig, &[x, y], &[]), eval(&f, &[x, y], &[]), "a {x} b {y} k1 {k1}");
            }
        }
    }
}

/// A float load read only by FToBits becomes an integer load (f32 and f64), and an integer
/// load read only by BitsToF a float load.
#[test]
fn float_loads_read_as_bits_load_integers() {
    for (fm, w, im) in [(Mem::F32, false, Mem::Int(4, false)), (Mem::F64, true, Mem::Int(8, false))] {
        let mut f = Func::new("lb".into(), 0);
        let p = f.vreg(Cls::I);
        f.params.push(p);
        f.block();
        let x = f.vreg(if w { Cls::F64 } else { Cls::F32 });
        let (d, y) = (f.vreg(Cls::I), f.vreg(Cls::I));
        let z = f.vreg(if w { Cls::F64 } else { Cls::F32 });
        push(&mut f, 0, Inst::Load(fm, x, p, 8));
        push(&mut f, 0, Inst::Conv(Conv::FToBits(w), d, x));
        push(&mut f, 0, Inst::Load(im, y, p, 16));
        push(&mut f, 0, Inst::Conv(Conv::BitsToF(w), z, y));
        f.rets.extend([Cls::I, if w { Cls::F64 } else { Cls::F32 }]);
        term(&mut f, 0, Term::Ret(vec![d, z]));
        relax_backend::passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
        let insts = &f.blocks[0].insts;
        assert!(!insts.iter().any(|i| matches!(i, Inst::Conv(..))), "{insts:?}");
        assert!(insts.iter().any(|i| matches!(i, Inst::Load(m, _, _, 8) if *m == im)), "{insts:?}");
        assert!(insts.iter().any(|i| matches!(i, Inst::Load(m, _, _, 16) if *m == fm)), "{insts:?}");
    }
}

/// Lomuto's cyclic step: `*gap = *left; *left = *right; gap = right; right += 4` with gap entering
/// as right - 4: the gap's reads become `right - 4`, the memory and result do not change, and the
/// store through the gap shares right's base.
#[test]
fn previous_iv_copy_reads_the_stepped_pointer() {
    let mut f = Func::new("gap".into(), 0);
    let (base, end) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.extend([base, end]);
    for _ in 0..4 {
        f.block();
    }
    let v: Vec<VReg> = (0..8).map(|_| f.vreg(Cls::I)).collect();
    let (g, r, c, x, y, left) = (v[0], v[1], v[2], v[3], v[4], v[5]);
    // entry: gap = base, right = base + 4, left = base + 40
    push(&mut f, 0, Inst::Mov(g, base));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, r, base, 4));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, left, base, 40));
    term(&mut f, 0, Term::Jump(1));
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c, r, end));
    term(&mut f, 1, Term::Branch(c, 2, 3));
    push(&mut f, 2, Inst::Load(Mem::Int(4, false), x, left, 0));
    push(&mut f, 2, Inst::Store(Mem::Int(4, false), g, 0, x));
    push(&mut f, 2, Inst::Load(Mem::Int(4, false), y, r, 0));
    push(&mut f, 2, Inst::Store(Mem::Int(4, false), left, 0, y));
    push(&mut f, 2, Inst::Mov(g, r));
    push(&mut f, 2, Inst::IBinI(IOp::Add, U64, r, r, 4));
    term(&mut f, 2, Term::Jump(1));
    f.rets.push(Cls::I);
    term(&mut f, 3, Term::Ret(vec![g]));
    let orig = f.clone();
    assert!(version::prev_iv_copies(&relax_session::Options::default(), &mut f));
    assert!(f.blocks[2].insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Add, _, _, rr, -4) if *rr == r)), "{:?}", f.blocks[2].insts);
    assert!(!version::prev_iv_copies(&relax_session::Options::default(), &mut f), "a second round finds nothing");
    let mem0: Vec<u8> = (0..64u8).collect();
    for n in [0u64, 1, 2, 5, 8] {
        let args = [0u64, 4 + 4 * n];
        assert_eq!(eval(&orig, &args, &mem0), eval(&f, &args, &mem0), "n {n}");
    }
    // the step twice per iteration (lomuto's source unrolling): both copies' reads rewritten
    let mut u = orig.clone();
    let body = u.blocks[2].insts.clone();
    u.blocks[2].insts.extend(body);
    u.blocks[2].pos = vec![1; u.blocks[2].insts.len()];
    let u0 = u.clone();
    assert!(version::prev_iv_copies(&relax_session::Options::default(), &mut u));
    let n_rewrites = u.blocks[2].insts.iter().filter(|i| matches!(i, Inst::IBinI(IOp::Add, _, _, rr, -4) if *rr == r)).count();
    assert_eq!(n_rewrites, 2, "{:?}", u.blocks[2].insts);
    for n in [0u64, 2, 4, 6] {
        let args = [0u64, 4 + 4 * n];
        assert_eq!(eval(&u0, &args, &mem0), eval(&u, &args, &mem0), "unrolled n {n}");
    }
    // entry offset that does not match the step: left alone
    let mut h = orig.clone();
    h.blocks[0].insts[1] = Inst::IBinI(IOp::Add, U64, r, base, 8);
    assert!(!version::prev_iv_copies(&relax_session::Options::default(), &mut h));
}
