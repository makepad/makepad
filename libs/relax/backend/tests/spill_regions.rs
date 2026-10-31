//! Spilled loop-carried values in region windows (regalloc::region_windows_alloc): regions over
//! blocks that redefine the value, entry loads moved to the end of outside predecessors (edge
//! loads), single-block loops, and stores home nobody reads skipped (dead_spill_stores). More
//! integer values live across the loop than there are registers, so some spill; results must
//! equal the same computation in Rust with every feature on and with each one off.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

fn options(off: &[&str]) -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.opt = off.iter().map(|s| s.to_string()).collect();
    o
}

#[cfg(target_arch = "x86_64")]
fn compile(o: &relax_session::Options, f: &Func, freq: Option<&[f32]>) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile_freq(o, f, &env, freq).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(o: &relax_session::Options, f: &Func, freq: Option<&[f32]>) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile_freq(o, f, &env, freq).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

const W: IntTy = IntTy { bits: 64, signed: false };
const K: usize = 34;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(n, seed): acc[k] = seed + k; i = 0; while i < n { body } ; fold.
/// body (`diamond` false): acc[k] = acc[k] * 3 + (acc[(k + 1) % K] ^ i) for every k; i += step.
/// `diamond`: the odd accumulators update only when i & 1 (a second block), the rest always.
/// `live_after`: the fold also reads i after the loop (its stores home stay needed).
fn build(diamond: bool, live_after: bool, step: i64) -> Func {
    let mut f = Func::new("spill_regions".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    let b0 = f.block();
    let b1 = f.block();
    let b2 = f.block();
    let b3 = f.block();
    let (b4, b5) = if diamond { (f.block(), f.block()) } else { (0, 0) };
    let acc: Vec<VReg> = (0..K).map(|_| f.vreg(Cls::I)).collect();
    let i = f.vreg(Cls::I);
    for k in 0..K {
        push(&mut f, b0, Inst::IBinI(IOp::Add, W, acc[k], seed, k as i64));
    }
    push(&mut f, b0, Inst::Iconst(i, 0));
    f.blocks[b0 as usize].term = Term::Jump(b1);
    let c = f.vreg(Cls::I);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b1 as usize].term = Term::Branch(c, b2, b3);
    let upd = |f: &mut Func, b: u32, k: usize| {
        let (t, x) = (f.vreg(Cls::I), f.vreg(Cls::I));
        push(f, b, Inst::IBinI(IOp::Mul, W, t, acc[k], 3));
        push(f, b, Inst::IBin(IOp::Xor, W, x, acc[(k + 1) % K], i));
        push(f, b, Inst::IBin(IOp::Add, W, acc[k], t, x));
    };
    if diamond {
        for k in (0..K).step_by(2) {
            upd(&mut f, b2, k);
        }
        let (bit, z) = (f.vreg(Cls::I), f.vreg(Cls::I));
        push(&mut f, b2, Inst::IBinI(IOp::And, W, bit, i, 1));
        push(&mut f, b2, Inst::ICmpI(Cond::Ne, false, z, bit, 0));
        f.blocks[b2 as usize].term = Term::Branch(z, b4, b5);
        for k in (1..K).step_by(2) {
            upd(&mut f, b4, k);
        }
        f.blocks[b4 as usize].term = Term::Jump(b5);
        let t = f.vreg(Cls::I);
        push(&mut f, b5, Inst::IBinI(IOp::Add, W, t, i, step));
        push(&mut f, b5, Inst::Mov(i, t));
        f.blocks[b5 as usize].term = Term::Jump(b1);
    } else {
        for k in 0..K {
            upd(&mut f, b2, k);
        }
        push(&mut f, b2, Inst::IBinI(IOp::Add, W, i, i, step));
        f.blocks[b2 as usize].term = Term::Jump(b1);
    }
    let r = f.vreg(Cls::I);
    push(&mut f, b3, Inst::Iconst(r, 0));
    for k in 0..K {
        let t = f.vreg(Cls::I);
        push(&mut f, b3, Inst::IBinI(IOp::Rotl, W, t, r, 7));
        push(&mut f, b3, Inst::IBin(IOp::Xor, W, r, t, acc[k]));
    }
    if live_after {
        push(&mut f, b3, Inst::IBin(IOp::Add, W, r, r, i));
    }
    f.blocks[b3 as usize].term = Term::Ret(vec![r]);
    f.verify().unwrap();
    f
}

fn reference(n: u64, seed: u64, diamond: bool, live_after: bool, step: u64) -> u64 {
    let mut acc: Vec<u64> = (0..K as u64).map(|k| seed.wrapping_add(k)).collect();
    let mut i = 0u64;
    let upd = |acc: &mut Vec<u64>, k: usize, i: u64| {
        acc[k] = acc[k].wrapping_mul(3).wrapping_add(acc[(k + 1) % K] ^ i);
    };
    while i < n {
        if diamond {
            for k in (0..K).step_by(2) {
                upd(&mut acc, k, i);
            }
            if i & 1 != 0 {
                for k in (1..K).step_by(2) {
                    upd(&mut acc, k, i);
                }
            }
        } else {
            for k in 0..K {
                upd(&mut acc, k, i);
            }
        }
        i = i.wrapping_add(step);
    }
    let mut r = 0u64;
    for k in 0..K {
        r = r.rotate_left(7) ^ acc[k];
    }
    if live_after {
        r = r.wrapping_add(i);
    }
    r
}

#[test]
fn spilled_loop_values_in_regions() {
    let offs: [&[&str]; 5] = [&[], &["-storeskip"], &["-edgeload"], &["-defregion"], &["-defregion", "-edgeload", "-storeskip"]];
    for diamond in [false, true] {
        for live_after in [false, true] {
            for step in [1i64, 2, 3] {
                let f = build(diamond, live_after, step);
                let mut codes = Vec::new();
                for off in offs {
                    let bytes = compile(&options(off), &f, None);
                    let code = Code::new(&bytes).unwrap();
                    let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                    for (n, seed) in [(0u64, 1u64), (1, 7), (5, 0x1234), (64, 0x9e3779b97f4a7c15), (1001, 3)] {
                        assert_eq!(run(n, seed), reference(n, seed, diamond, live_after, step as u64), "diamond {} live_after {} step {} off {:?} n {}", diamond, live_after, step, off, n);
                    }
                    codes.push(bytes);
                }
                // every register is busy in this loop: correctness only
                let _ = codes;
            }
        }
    }
}

/// g(n, seed): acc = seed; i = 0; while i < n { x = acc ^ i; t_k = x + (k + 3) * 97 for k < T, all live
/// at once (folded back in reverse); acc = fold; i += step }; ret acc (+ i when `live_after`).
/// The temporaries need every register in the body block only: i and n are free in a register in
/// the loop header and latch, the body block holds them in memory.
const T: usize = 40;
fn build_burst(live_after: bool, step: i64) -> Func {
    let mut f = Func::new("spill_burst".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    let (b0, b1, b2, b3, b4, b5) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
    let (acc, i, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Mov(acc, seed));
    push(&mut f, b0, Inst::Iconst(i, 0));
    f.blocks[b0 as usize].term = Term::Jump(b1);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b1 as usize].term = Term::Branch(c, b5, b4);
    // the burst runs every fourth iteration
    let (bits, z) = (f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b5, Inst::IBinI(IOp::And, W, bits, i, 3));
    push(&mut f, b5, Inst::ICmpI(Cond::Eq, false, z, bits, 0));
    f.blocks[b5 as usize].term = Term::Branch(z, b2, b3);
    let x = f.vreg(Cls::I);
    push(&mut f, b2, Inst::IBin(IOp::Xor, W, x, acc, i));
    let t: Vec<VReg> = (0..T).map(|_| f.vreg(Cls::I)).collect();
    for k in 0..T {
        push(&mut f, b2, Inst::IBinI(IOp::Add, W, t[k], x, (k as i64 + 3) * 97));
    }
    let r = f.vreg(Cls::I);
    push(&mut f, b2, Inst::Iconst(r, 0));
    for k in (0..T).rev() {
        let u = f.vreg(Cls::I);
        push(&mut f, b2, Inst::IBinI(IOp::Rotl, W, u, r, 5));
        push(&mut f, b2, Inst::IBin(IOp::Xor, W, r, u, t[k]));
    }
    push(&mut f, b2, Inst::Mov(acc, r));
    f.blocks[b2 as usize].term = Term::Jump(b3);
    // the latch
    push(&mut f, b3, Inst::IBinI(IOp::Add, W, i, i, step));
    f.blocks[b3 as usize].term = Term::Jump(b1);
    if live_after {
        push(&mut f, b4, Inst::IBin(IOp::Add, W, acc, acc, i));
    }
    f.blocks[b4 as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    f
}

fn reference_burst(n: u64, seed: u64, live_after: bool, step: u64) -> u64 {
    let (mut acc, mut i) = (seed, 0u64);
    while i < n {
        if i & 3 == 0 {
            let x = acc ^ i;
            let mut r = 0u64;
            for k in (0..T as u64).rev() {
                r = r.rotate_left(5) ^ x.wrapping_add((k + 3) * 97);
            }
            acc = r;
        }
        i = i.wrapping_add(step);
    }
    if live_after {
        acc = acc.wrapping_add(i);
    }
    acc
}

#[test]
fn values_free_outside_a_burst_block() {
    let offs: [&[&str]; 5] = [&[], &["-storeskip"], &["-edgeload"], &["-defregion"], &["-defregion", "-edgeload", "-storeskip"]];
    for live_after in [false, true] {
        for step in [1i64, 2, 3] {
            let f = build_burst(live_after, step);
            // sampled frequencies: the burst block runs on a quarter of the iterations
            let freq = [0.0f32, 1.0, 0.25, 1.0, 0.0, 1.0];
            for fr in [None, Some(&freq[..])] {
                for off in offs {
                    let bytes = compile(&options(off), &f, fr);
                    let code = Code::new(&bytes).unwrap();
                    let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                    for (n, seed) in [(0u64, 1u64), (1, 7), (5, 0x1234), (64, 0x9e3779b97f4a7c15), (1001, 3)] {
                        assert_eq!(run(n, seed), reference_burst(n, seed, live_after, step as u64), "live_after {} step {} off {:?} freq {} n {}", live_after, step, off, fr.is_some(), n);
                    }
                }
            }
        }
    }
}
