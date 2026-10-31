//! Value ranges across an offset (ranges::decide_rel): in `for i in 1..out.len() { out[i] = out[i] + out[i - 1] }`
//! the loop's own test `i < n` decides the check of `i - 1` (i >= 1, so `i - 1` does not wrap); the
//! check of `i - 2` stays (i may be 1). The result must not change.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

const U64: IntTy = IntTy { bits: 64, signed: false };
const U32: IntTy = IntTy { bits: 32, signed: false };

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap();
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

/// f(p, n) over u64 words: i = 1; while i < n { t = i; i += 1; check t - back < n (else return
/// 1000 + t); p[t] += p[t - back] }; return 0. `back` 1 or 2.
fn build(back: i64) -> Func {
    let mut f = Func::new("offset_ranges".into(), 0);
    let (p, n0) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, n0];
    f.rets = vec![Cls::I];
    let v: Vec<VReg> = (0..16).map(|_| f.vreg(Cls::I)).collect();
    let (n, i, t, c, j, c2) = (v[0], v[1], v[2], v[3], v[4], v[5]);
    let b: Vec<u32> = (0..6).map(|_| f.block()).collect();
    push(&mut f, b[0], Inst::Conv(Conv::IntToInt(U32), n, n0));
    push(&mut f, b[0], Inst::Iconst(i, 1));
    f.blocks[b[0] as usize].term = Term::Jump(b[1]);
    // b1 (header): t = i; i = i + 1; t < n ?
    push(&mut f, b[1], Inst::Mov(t, i));
    push(&mut f, b[1], Inst::IBinI(IOp::Add, U32, v[6], i, 1));
    push(&mut f, b[1], Inst::Mov(i, v[6]));
    push(&mut f, b[1], Inst::ICmp(Cond::Lt, false, c, t, n));
    f.blocks[b[1] as usize].term = Term::Branch(c, b[2], b[5]);
    // b2: the check of t - back
    push(&mut f, b[2], Inst::IBinI(IOp::Sub, U32, j, t, back));
    push(&mut f, b[2], Inst::ICmp(Cond::Lt, false, c2, j, n));
    f.blocks[b[2] as usize].term = Term::Branch(c2, b[3], b[4]);
    // b3: p[t] += p[j]
    push(&mut f, b[3], Inst::LoadX(Mem::Int(8, false), v[7], p, t, 3, 0));
    push(&mut f, b[3], Inst::LoadX(Mem::Int(8, false), v[8], p, j, 3, 0));
    push(&mut f, b[3], Inst::IBin(IOp::Add, U64, v[9], v[7], v[8]));
    push(&mut f, b[3], Inst::StoreX(Mem::Int(8, false), p, t, 3, 0, v[9]));
    f.blocks[b[3] as usize].term = Term::Jump(b[1]);
    // b4: the failed check
    push(&mut f, b[4], Inst::IBinI(IOp::Add, U64, v[10], t, 1000));
    f.blocks[b[4] as usize].term = Term::Ret(vec![v[10]]);
    push(&mut f, b[5], Inst::Iconst(v[11], 0));
    f.blocks[b[5] as usize].term = Term::Ret(vec![v[11]]);
    f.verify().unwrap();
    f
}

/// Reachable branches on a compare of `t - back`.
fn checks(f: &Func) -> usize {
    let reach = relax_backend::regalloc::reachable(f);
    let mut n = 0;
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] && b.insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Sub, _, _, _, _))) && matches!(b.term, Term::Branch(..)) {
            n += 1;
        }
    }
    n
}

fn want(words: &mut [u64], back: usize) -> u64 {
    for t in 1..words.len() {
        if t < back {
            return 1000 + t as u64;
        }
        words[t] = words[t].wrapping_add(words[t - back]);
    }
    0
}

#[test]
fn loop_test_decides_the_previous_index() {
    let o = relax_session::Options::default();
    for back in [1i64, 2] {
        let mut f = build(back);
        assert_eq!(checks(&f), 1);
        relax_codegen::ranges::fold_ranges(&o, &mut f);
        relax_backend::passes::simplify(&o, &mut f, None, true, true);
        f.verify().unwrap();
        assert_eq!(checks(&f), if back == 1 { 0 } else { 1 }, "back {}\n{}", back, f.dump());
        for g in [build(back), f] {
            let code = Code::new(&compile(&g)).unwrap();
            let run: extern "C" fn(*mut u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            for n in [0usize, 1, 2, 3, 9] {
                let mut a: Vec<u64> = (0..n).map(|k| k as u64 * 7 + 3).collect();
                let mut b = a.clone();
                let got = run(a.as_mut_ptr(), n as u64);
                assert_eq!(got, want(&mut b, back as usize), "back {} n {}", back, n);
                assert_eq!(a, b, "back {} n {}", back, n);
            }
        }
    }
}

/// A rotated counting loop whose body indexes with the copy of the counter taken before its step
/// (`t = j; j = j + 1; j < 576` at the latch, `t < 576` checked in the body): the copy is a loop
/// variable of the counter's range, so the body's check folds. p[t] = t for t in 0..576 (n words).
fn build_rotated(n: i64) -> Func {
    let mut f = Func::new("rotated_copy".into(), 0);
    let p = f.vreg(Cls::I);
    f.params = vec![p];
    f.rets = vec![Cls::I];
    let v: Vec<VReg> = (0..10).map(|_| f.vreg(Cls::I)).collect();
    let (t, j) = (v[0], v[1]);
    let b: Vec<u32> = (0..5).map(|_| f.block()).collect();
    push(&mut f, b[0], Inst::Iconst(t, 0));
    push(&mut f, b[0], Inst::Iconst(j, 1));
    push(&mut f, b[0], Inst::ICmpI(Cond::Lt, false, v[2], j, n));
    f.blocks[b[0] as usize].term = Term::Branch(v[2], b[1], b[3]);
    push(&mut f, b[1], Inst::ICmpI(Cond::Lt, false, v[3], t, n));
    f.blocks[b[1] as usize].term = Term::Branch(v[3], b[2], b[4]);
    push(&mut f, b[2], Inst::StoreX(Mem::Int(8, false), p, t, 3, 0, t));
    push(&mut f, b[2], Inst::Mov(t, j));
    push(&mut f, b[2], Inst::IBinI(IOp::Add, U32, j, j, 1));
    push(&mut f, b[2], Inst::ICmpI(Cond::Lt, false, v[4], j, n));
    f.blocks[b[2] as usize].term = Term::Branch(v[4], b[1], b[3]);
    push(&mut f, b[3], Inst::Iconst(v[5], 0));
    f.blocks[b[3] as usize].term = Term::Ret(vec![v[5]]);
    push(&mut f, b[4], Inst::Iconst(v[6], 999));
    f.blocks[b[4] as usize].term = Term::Ret(vec![v[6]]);
    f.verify().unwrap();
    f
}

#[test]
fn rotated_loop_copy_of_the_counter_is_in_range() {
    let o = relax_session::Options::default();
    let mut f = build_rotated(576);
    relax_codegen::ranges::fold_ranges(&o, &mut f);
    relax_backend::passes::simplify(&o, &mut f, None, true, true);
    f.verify().unwrap();
    let reach = relax_backend::regalloc::reachable(&f);
    // the body's check of t (VReg 1) is gone: only the counter's own tests remain
    assert!(!f.blocks.iter().enumerate().any(|(bi, b)| reach[bi] && b.insts.iter().any(|i| matches!(i, Inst::ICmpI(_, _, _, x, 576) if *x == VReg(1)))), "{}", f.dump());
    for g in [build_rotated(576), f] {
        let code = Code::new(&compile(&g)).unwrap();
        let run: extern "C" fn(*mut u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
        let mut a = vec![7u64; 576];
        assert_eq!(run(a.as_mut_ptr()), 0);
        // t runs 0..575 (the step that makes j 576 leaves the loop)
        assert!(a.iter().enumerate().all(|(k, x)| if k < 575 { *x == k as u64 } else { *x == 7 }), "{:?}", &a[570..]);
    }
}
