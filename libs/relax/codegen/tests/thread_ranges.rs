//! Jump threading by edge facts (ranges::thread_decided): an `Option<u32>`-like niche test after a
//! join whose one input is the None constant and whose other is a zero-extended byte is decided on
//! each incoming edge, so each predecessor jumps straight to its successor; a second test of the
//! same condition right after the first (the loop's own match) is decided by the first's edge.
//! The result must not change.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

const U64: IntTy = IntTy { bits: 64, signed: false };
const U32: IntTy = IntTy { bits: 32, signed: false };
const NONE: i64 = 0xffff_ffff;

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

/// f(p, n): acc = 0; i = 0; loop { v = if i < n { i += 1; p[i - 1] as u32 } else { NONE };
/// if v != NONE { w = v } else { w = NONE }; if w == NONE { return acc }; acc = acc * 31 + w }
fn build() -> Func {
    let mut f = Func::new("thread_ranges".into(), 0);
    let (p, n) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, n];
    f.rets = vec![Cls::I];
    let v: Vec<VReg> = (0..16).map(|_| f.vreg(Cls::I)).collect();
    let (acc, i, val, w) = (v[0], v[1], v[2], v[3]);
    let b: Vec<u32> = (0..9).map(|_| f.block()).collect();
    push(&mut f, b[0], Inst::Iconst(acc, 0));
    push(&mut f, b[0], Inst::Iconst(i, 0));
    f.blocks[b[0] as usize].term = Term::Jump(b[1]);
    // b1 (loop header): i < n ?
    push(&mut f, b[1], Inst::ICmp(Cond::Lt, false, v[4], i, n));
    f.blocks[b[1] as usize].term = Term::Branch(v[4], b[3], b[2]);
    // b2: the end: None
    push(&mut f, b[2], Inst::Iconst(val, NONE));
    f.blocks[b[2] as usize].term = Term::Jump(b[4]);
    // b3: a byte, zero-extended
    push(&mut f, b[3], Inst::LoadX(Mem::Int(1, false), v[5], p, i, 0, 0));
    push(&mut f, b[3], Inst::IBinI(IOp::Add, U64, i, i, 1));
    push(&mut f, b[3], Inst::Conv(Conv::IntToInt(U32), val, v[5]));
    f.blocks[b[3] as usize].term = Term::Jump(b[4]);
    // b4: the decoder's own test
    push(&mut f, b[4], Inst::ICmpI(Cond::Ne, false, v[6], val, NONE));
    f.blocks[b[4] as usize].term = Term::Branch(v[6], b[5], b[6]);
    push(&mut f, b[5], Inst::Mov(w, val));
    f.blocks[b[5] as usize].term = Term::Jump(b[7]);
    push(&mut f, b[6], Inst::Iconst(w, NONE));
    f.blocks[b[6] as usize].term = Term::Jump(b[7]);
    // b7: the loop's match on the same value
    push(&mut f, b[7], Inst::ICmpI(Cond::Eq, false, v[7], w, NONE));
    f.blocks[b[7] as usize].term = Term::Branch(v[7], b[8], b[1]);
    // the latch is the false edge of b7: acc = acc * 31 + w happens in a block of its own below
    f.blocks[b[7] as usize].term = Term::Branch(v[7], b[8], b.len() as u32);
    let lb = f.block();
    push(&mut f, lb, Inst::IBinI(IOp::Mul, U64, v[8], acc, 31));
    push(&mut f, lb, Inst::IBin(IOp::Add, U64, acc, v[8], w));
    f.blocks[lb as usize].term = Term::Jump(b[1]);
    f.blocks[b[8] as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    f
}

/// Branches on a compare against NONE still reachable.
fn niche_tests(f: &Func) -> usize {
    let reach = relax_backend::regalloc::reachable(f);
    let mut n = 0;
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        if let Term::Branch(c, _, _) = b.term {
            if b.insts.iter().any(|i| matches!(*i, Inst::ICmpI(_, _, d, _, k) if d == c && k == NONE)) {
                n += 1;
            }
        }
    }
    n
}

fn want(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |a, &x| a.wrapping_mul(31).wrapping_add(x as u64))
}

#[test]
fn niche_tests_thread_on_their_edges() {
    let o = relax_session::Options::default();
    let mut f = build();
    assert_eq!(niche_tests(&f), 2);
    relax_codegen::ranges::fold_ranges(&o, &mut f);
    relax_backend::passes::simplify(&o, &mut f, None, true, true);
    f.verify().unwrap();
    // every remaining test is decided on its edge: none survives on the reachable paths
    assert_eq!(niche_tests(&f), 0, "{}", f.dump());
    for g in [build(), f] {
        let code = Code::new(&compile(&g)).unwrap();
        let run: extern "C" fn(*const u8, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
        for n in [0usize, 1, 2, 7, 64] {
            let bytes: Vec<u8> = (0..n).map(|k| (k * 37 + 11) as u8).collect();
            assert_eq!(run(bytes.as_ptr(), n as u64), want(&bytes), "n {}", n);
        }
    }
}
