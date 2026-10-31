//! Block-local eviction (regalloc Alloc::evicts / prestores) and dead stores of def-window values.
//! A loop's long-lived values are read many times in one block of the loop and not at all in
//! another, where short temporaries (a definition and three reads each) outnumber the free
//! registers: the temporaries rank lower and spill, and their def windows find a register only when
//! a long value is put aside over that block. Long values defined once before the loop are stored
//! once after their definition (prestore), the ones the latch redefines at each eviction, a
//! constant held in a register is recomputed. Results must equal the same computation in Rust with
//! every feature on and each one off, and eviction must change the code.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

fn options(off: &[&str]) -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.opt = off.iter().map(|s| s.to_string()).collect();
    o
}

#[cfg(target_arch = "x86_64")]
fn compile(o: &relax_session::Options, f: &Func) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile_freq(o, f, &env, None).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(o: &relax_session::Options, f: &Func) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile_freq(o, f, &env, None).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

const W: IntTy = IntTy { bits: 64, signed: false };
const CST: i64 = 0x5555_1234_9876;
const REPS: usize = 6;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// long values l[k] (L of them), temporaries t[k] (T of them); the latch redefines every
/// `redef`-th long value (0: none).
/// `branchy`: the burst block ends in a branch on a long value (l[0] & 1), both ways to the long
/// values' block (one through an extra block): a long value read right after the burst.
fn build(l_n: usize, t_n: usize, redef: usize, branchy: bool) -> Func {
    let mut f = Func::new("evict_windows".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    let (b0, b1, b2, b3, b4, b5) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
    let l: Vec<VReg> = (0..l_n).map(|_| f.vreg(Cls::I)).collect();
    let (acc, i, cst, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    for k in 0..l_n {
        push(&mut f, b0, Inst::IBinI(IOp::Add, W, l[k], seed, (k as i64 + 1) * 0x9e37));
    }
    push(&mut f, b0, Inst::Iconst(cst, CST));
    push(&mut f, b0, Inst::Mov(acc, seed));
    push(&mut f, b0, Inst::Iconst(i, 0));
    f.blocks[b0 as usize].term = Term::Jump(b1);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b1 as usize].term = Term::Branch(c, b2, b3);
    // the burst: t[k] = x * (2k + 3), then three passes r = rotl(r, 5) ^ t[k]
    let x = f.vreg(Cls::I);
    push(&mut f, b2, Inst::IBin(IOp::Xor, W, x, acc, i));
    let t: Vec<VReg> = (0..t_n).map(|_| f.vreg(Cls::I)).collect();
    for k in 0..t_n {
        push(&mut f, b2, Inst::IBinI(IOp::Mul, W, t[k], x, 2 * k as i64 + 3));
    }
    let r = f.vreg(Cls::I);
    push(&mut f, b2, Inst::Iconst(r, 0));
    for _ in 0..3 {
        for k in 0..t_n {
            let u = f.vreg(Cls::I);
            push(&mut f, b2, Inst::IBinI(IOp::Rotl, W, u, r, 5));
            push(&mut f, b2, Inst::IBin(IOp::Xor, W, r, u, t[k]));
        }
    }
    push(&mut f, b2, Inst::Mov(acc, r));
    if branchy {
        let b6 = f.block();
        let (bit, z) = (f.vreg(Cls::I), f.vreg(Cls::I));
        push(&mut f, b2, Inst::IBinI(IOp::And, W, bit, l[0], 1));
        push(&mut f, b2, Inst::ICmpI(Cond::Ne, false, z, bit, 0));
        f.blocks[b2 as usize].term = Term::Branch(z, b6, b4);
        push(&mut f, b6, Inst::IBinI(IOp::Add, W, acc, acc, 1));
        f.blocks[b6 as usize].term = Term::Jump(b4);
    } else {
        f.blocks[b2 as usize].term = Term::Jump(b4);
    }
    // the long values' block: acc += l[k] ^ l[k + 1], acc ^= cst per pass
    for _ in 0..REPS {
        for k in 0..l_n {
            let u = f.vreg(Cls::I);
            push(&mut f, b4, Inst::IBin(IOp::Xor, W, u, l[k], l[(k + 1) % l_n]));
            push(&mut f, b4, Inst::IBin(IOp::Add, W, acc, acc, u));
        }
        push(&mut f, b4, Inst::IBin(IOp::Xor, W, acc, acc, cst));
    }
    f.blocks[b4 as usize].term = Term::Jump(b5);
    if redef > 0 {
        for k in (0..l_n).step_by(redef) {
            push(&mut f, b5, Inst::IBin(IOp::Add, W, l[k], l[k], i));
        }
    }
    push(&mut f, b5, Inst::IBinI(IOp::Add, W, i, i, 1));
    f.blocks[b5 as usize].term = Term::Jump(b1);
    for k in 0..l_n {
        let u = f.vreg(Cls::I);
        push(&mut f, b3, Inst::IBinI(IOp::Rotl, W, u, acc, 7));
        push(&mut f, b3, Inst::IBin(IOp::Xor, W, acc, u, l[k]));
    }
    f.blocks[b3 as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    f
}

fn reference(n: u64, seed: u64, l_n: usize, t_n: usize, redef: usize, branchy: bool) -> u64 {
    let mut l: Vec<u64> = (0..l_n as u64).map(|k| seed.wrapping_add((k + 1) * 0x9e37)).collect();
    let mut acc = seed;
    let mut i = 0u64;
    while i < n {
        let x = acc ^ i;
        let t: Vec<u64> = (0..t_n as u64).map(|k| x.wrapping_mul(2 * k + 3)).collect();
        let mut r = 0u64;
        for _ in 0..3 {
            for k in 0..t_n {
                r = r.rotate_left(5) ^ t[k];
            }
        }
        acc = r;
        if branchy && l[0] & 1 != 0 {
            acc = acc.wrapping_add(1);
        }
        for _ in 0..REPS {
            for k in 0..l_n {
                acc = acc.wrapping_add(l[k] ^ l[(k + 1) % l_n]);
            }
            acc ^= CST as u64;
        }
        if redef > 0 {
            for k in (0..l_n).step_by(redef) {
                l[k] = l[k].wrapping_add(i);
            }
        }
        i += 1;
    }
    for k in 0..l_n {
        acc = acc.rotate_left(7) ^ l[k];
    }
    acc
}

#[test]
fn evicted_registers_serve_windows() {
    // second chance runs only on request (+evict)
    let offs: [&[&str]; 4] = [&["+evict"], &[], &["+evict", "-defstore"], &["-defstore"]];
    let shapes: &[(usize, usize)] = if cfg!(target_arch = "x86_64") { &[(9, 14), (10, 20)] } else { &[(22, 30), (26, 40)] };
    let mut changed = false;
    for &(l_n, t_n) in shapes {
        for (redef, branchy) in [(0usize, false), (1, false), (3, false), (0, true), (3, true)] {
            let f = build(l_n, t_n, redef, branchy);
            let mut codes = Vec::new();
            for off in offs {
                let bytes = compile(&options(off), &f);
                let code = Code::new(&bytes).unwrap();
                let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                for (n, seed) in [(0u64, 1u64), (1, 7), (5, 0x1234), (64, 0x9e3779b97f4a7c15), (301, 3)] {
                    assert_eq!(run(n, seed), reference(n, seed, l_n, t_n, redef, branchy), "L {} T {} redef {} branchy {} off {:?} n {}", l_n, t_n, redef, branchy, off, n);
                }
                codes.push(bytes);
            }
            changed |= codes[0] != codes[1];
        }
    }
    assert!(changed, "no eviction in any shape: the test no longer covers it");
}
