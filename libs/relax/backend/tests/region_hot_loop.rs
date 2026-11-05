//! A spilled value read on a loop's hot path stays in a register over that path (region windows,
//! regalloc::region_windows_alloc) when the hot path's latch block only moves (`acc = y`, what a
//! loop's phi copies leave): such a block was never part of a region, which broke the region at
//! the loop header and reloaded the value on every iteration. Also with the loop nested past the
//! static weights' depth cap and entered through a block made by the loop split. Results must
//! equal Rust's.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::regalloc::{allocate_split, Loc};
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func, freq: Option<&[f32]>) -> Vec<u8> {
    let o = relax_session::Options::default();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile_freq(&o, f, &env, freq).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func, freq: Option<&[f32]>) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile_freq(&relax_session::Options::default(), f, &env, freq).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

const W: IntTy = IntTy { bits: 64, signed: false };
/// burst temporaries, all live at once: more than either target has registers
const T: usize = 40;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// m(n, seed): acc = seed; k = seed | 1; r = seed * 7; `outer` enclosing loops of one trip each,
/// the innermost entering through a block that runs a burst too (acc ^= burst(acc): no register
/// there, the hot loop's entry load goes to the end of the entry block the loop split makes), around
/// the hot loop `i = 0; while i < n { y = (acc ^ k) + i; i += 1; if i & 3 == 0 { acc =
/// burst(y) } else { acc = y } }`; then r folded into acc six times. The burst's temporaries,
/// each read three times, need every register and outweigh k (k spills); the hot path's latch is
/// `acc = y` alone. Returns the function and its sampled block frequencies (the hot path 1, the
/// burst 1/4, the rest never sampled).
fn build(outer: usize) -> (Func, Vec<f32>) {
    let mut f = Func::new("region_hot_loop".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    let b0 = f.block();
    let hs: Vec<u32> = (0..outer).map(|_| f.block()).collect();
    let ls: Vec<u32> = (0..outer).map(|_| f.block()).collect();
    let (b1, b2, b3, b4, fin, e) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
    let (acc, k, r, i) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let js: Vec<VReg> = (0..outer).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, b0, Inst::Mov(acc, seed));
    push(&mut f, b0, Inst::IBinI(IOp::Or, W, k, seed, 1));
    push(&mut f, b0, Inst::IBinI(IOp::Mul, W, r, seed, 7));
    for &j in &js {
        push(&mut f, b0, Inst::Iconst(j, 0));
    }
    push(&mut f, b0, Inst::Iconst(i, 0));
    let first = if outer > 0 { hs[0] } else { b1 };
    f.blocks[b0 as usize].term = Term::Jump(first);
    // outer loop d: header hs[d] (j_d < 1: in, else to the latch of loop d - 1 or the end),
    // latch ls[d] (j_d += 1, the next inner counter reset)
    for d in 0..outer {
        let c = f.vreg(Cls::I);
        push(&mut f, hs[d], Inst::ICmpI(Cond::Lt, false, c, js[d], 1));
        let inner = if d + 1 < outer { hs[d + 1] } else { e };
        let out = if d > 0 { ls[d - 1] } else { fin };
        f.blocks[hs[d] as usize].term = Term::Branch(c, inner, out);
        push(&mut f, ls[d], Inst::IBinI(IOp::Add, W, js[d], js[d], 1));
        if d + 1 < outer {
            push(&mut f, ls[d], Inst::Iconst(js[d + 1], 0));
        } else {
            push(&mut f, ls[d], Inst::Iconst(i, 0));
        }
        f.blocks[ls[d] as usize].term = Term::Jump(hs[d]);
    }
    if outer > 0 {
        let q = burst(&mut f, e, acc, 5);
        push(&mut f, e, Inst::IBin(IOp::Xor, W, acc, acc, q));
        let c = f.vreg(Cls::I);
        push(&mut f, e, Inst::ICmp(Cond::Lt, false, c, i, n));
        f.blocks[e as usize].term = Term::Branch(c, b1, ls[outer - 1]);
    } else {
        f.blocks[e as usize].term = Term::Unreachable;
    }
    let c = f.vreg(Cls::I);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b1 as usize].term = Term::Branch(c, b2, if outer > 0 { ls[outer - 1] } else { fin });
    let (x, y, bits, z) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b2, Inst::IBin(IOp::Xor, W, x, acc, k));
    push(&mut f, b2, Inst::IBin(IOp::Add, W, y, x, i));
    push(&mut f, b2, Inst::IBinI(IOp::Add, W, i, i, 1));
    push(&mut f, b2, Inst::IBinI(IOp::And, W, bits, i, 3));
    push(&mut f, b2, Inst::ICmpI(Cond::Eq, false, z, bits, 0));
    f.blocks[b2 as usize].term = Term::Branch(z, b4, b3);
    push(&mut f, b3, Inst::Mov(acc, y));
    f.blocks[b3 as usize].term = Term::Jump(b1);
    let q = burst(&mut f, b4, y, 3);
    push(&mut f, b4, Inst::Mov(acc, q));
    f.blocks[b4 as usize].term = Term::Jump(b1);
    for _ in 0..6 {
        let u = f.vreg(Cls::I);
        push(&mut f, fin, Inst::IBinI(IOp::Rotl, W, u, acc, 3));
        push(&mut f, fin, Inst::IBin(IOp::Xor, W, acc, u, r));
    }
    f.blocks[fin as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    let mut freq = vec![0.0f32; f.blocks.len()];
    for b in [b1, b2, b3] {
        freq[b as usize] = 1.0;
    }
    freq[b3 as usize] = 0.75;
    freq[b4 as usize] = 0.25;
    (f, freq)
}

/// t_m = y + (m + base) * 97 for m < T, all live, then folded in reverse, each read three times
fn burst(f: &mut Func, b: u32, y: VReg, base: i64) -> VReg {
    let t: Vec<VReg> = (0..T).map(|_| f.vreg(Cls::I)).collect();
    for m in 0..T {
        push(f, b, Inst::IBinI(IOp::Add, W, t[m], y, (m as i64 + base) * 97));
    }
    let q = f.vreg(Cls::I);
    push(f, b, Inst::Iconst(q, 0));
    for m in (0..T).rev() {
        let (u, s, w) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        push(f, b, Inst::IBinI(IOp::Rotl, W, u, q, 5));
        push(f, b, Inst::IBin(IOp::Xor, W, q, u, t[m]));
        push(f, b, Inst::IBinI(IOp::Shr, W, s, t[m], 3));
        push(f, b, Inst::IBin(IOp::Add, W, w, q, s));
        push(f, b, Inst::IBin(IOp::Sub, W, q, w, t[m]));
    }
    q
}

fn burst_ref(y: u64, base: u64) -> u64 {
    let mut q = 0u64;
    for m in (0..T as u64).rev() {
        let t = y.wrapping_add((m + base) * 97);
        q = (q.rotate_left(5) ^ t).wrapping_add(t >> 3).wrapping_sub(t);
    }
    q
}

fn reference(n: u64, seed: u64, outer: usize) -> u64 {
    let (mut acc, k, r) = (seed, seed | 1, seed.wrapping_mul(7));
    if outer > 0 {
        acc ^= burst_ref(acc, 5);
    }
    let mut i = 0u64;
    while i < n {
        let y = (acc ^ k).wrapping_add(i);
        i += 1;
        if i & 3 == 0 {
            acc = burst_ref(y, 3);
        } else {
            acc = y;
        }
    }
    for _ in 0..6 {
        acc = acc.rotate_left(3) ^ r;
    }
    acc
}

#[cfg(target_arch = "x86_64")]
fn reg_config() -> relax_backend::regalloc::RegConfig {
    relax_backend::x64::reg_config(&relax_session::Options::default())
}

#[cfg(target_arch = "aarch64")]
fn reg_config() -> relax_backend::regalloc::RegConfig {
    relax_backend::arm64::reg_config()
}

#[test]
fn hot_path_holds_the_spilled_value() {
    for outer in [0usize, 6] {
        let (f, freq) = build(outer);
        for fr in [None, Some(&freq[..])] {
            let code = Code::new(&compile(&f, fr)).unwrap();
            let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            for (n, seed) in [(0u64, 1u64), (1, 7), (63, 0x1234), (64, 0x9e3779b97f4a7c15), (1001, 3)] {
                assert_eq!(run(n, seed), reference(n, seed, outer), "outer {} freq {} n {}", outer, fr.is_some(), n);
            }
        }
        let (g, al) = allocate_split(&relax_session::Options::default(), &f, &reg_config(), true, Some(&freq));
        let h = g.as_ref().unwrap_or(&f);
        // the hot path in the allocated function (the loop split may add and reorder blocks): the
        // body (i & 3), its predecessor (the header) and its move-only successor (the latch)
        let body = h.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::And, _, _, _, 3)))).unwrap();
        let head = (0..h.blocks.len()).find(|&b| h.blocks[b].term.succs().any(|s| s as usize == body)).unwrap();
        let latch = h.blocks[body].term.succs().map(|s| s as usize).find(|&s| matches!(h.blocks[s].term, Term::Jump(t) if t as usize == head) && h.blocks[s].insts.iter().all(|i| matches!(i, Inst::Mov(..)))).unwrap();
        let (xi, kk) = h.blocks[body].insts.iter().enumerate().find_map(|(ii, i)| if let Inst::IBin(IOp::Xor, _, _, _, b) = i { Some((ii, *b)) } else { None }).unwrap();
        assert!(matches!(al.loc[kk.0 as usize], Loc::Stack(_)), "outer {}: the fixture needs k spilled: {:?}", outer, al.loc[kk.0 as usize]);
        if outer > 0 {
            assert!(g.is_some(), "outer {}: the fixture needs the loop split's made blocks", outer);
        }
        let in_block = |p: u32, b: usize| al.block_start[b] <= p && p <= al.block_end[b];
        for b in [head, body, latch] {
            let loads: Vec<_> = al.windows.iter().filter(|w| w.0 == kk.0 && in_block(w.1, b)).collect();
            assert!(loads.is_empty(), "outer {}: k loaded on the hot path in b{} (head b{} body b{} latch b{}): {:?}", outer, b, head, body, latch, loads);
        }
        let p = al.block_start[body] + 2 * xi as u32 + 1;
        assert!(al.chained.iter().any(|w| w.0 == kk.0 && w.1 <= p && p <= w.2), "outer {}: k is read from its slot in the body", outer);
    }
}
