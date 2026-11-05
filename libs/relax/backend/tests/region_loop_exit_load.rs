//! A spilled value held in a register over an outer loop's blocks (a region window) is loaded
//! again where an inner loop exits into the region. That load went to the end of the inner loop's
//! latch (an edge load) when the latch weighed less than the exit block: past the static weights'
//! depth cap the two tie, and an estimated latch frequency below its exit's then put the load in
//! every inner iteration. A block of a loop the exit block is not in never takes its loads.
//! Results must equal Rust's.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::regalloc::{allocate_split, Loc};
use relax_backend::{exec::Code, rir::*};

fn options() -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.opt.push("+checkra".into());
    o
}

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func, freq: Option<&[f32]>) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile_freq(&options(), f, &env, freq).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func, freq: Option<&[f32]>) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile_freq(&options(), f, &env, freq).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "x86_64")]
fn reg_config() -> relax_backend::regalloc::RegConfig {
    relax_backend::x64::reg_config(&relax_session::Options::default())
}

#[cfg(target_arch = "aarch64")]
fn reg_config() -> relax_backend::regalloc::RegConfig {
    relax_backend::arm64::reg_config()
}

const W: IntTy = IntTy { bits: 64, signed: false };
/// burst temporaries, all live at once: more than either target has registers
const T: usize = 40;
/// trips of the middle loop
const M: i64 = 3;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// m(n, seed): acc = seed; o = seed * 5; `outer` enclosing loops of one trip each around the
/// middle loop `for j in 0..M { acc ^= o; i = 0; do { acc = burst(acc) + i; i += 1 } while i < n;
/// acc += o }`. The inner loop is its burst block and a latch (`i += 1; i < n`). o is live over
/// the bursts (no register there: it spills) and read in the middle loop's blocks before and after
/// the inner loop. Returns the function and its block frequencies: the middle loop and the burst 1,
/// the latch never sampled (an estimate below its exit's), the rest 0.
fn build(outer: usize) -> (Func, Vec<f32>) {
    let mut f = Func::new("region_loop_exit_load".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    let b0 = f.block();
    let hs: Vec<u32> = (0..outer).map(|_| f.block()).collect();
    let ls: Vec<u32> = (0..outer).map(|_| f.block()).collect();
    let (mh, mb, ib, il, mx, fin) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
    let (acc, o, i, j) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let js: Vec<VReg> = (0..outer).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, b0, Inst::Mov(acc, seed));
    push(&mut f, b0, Inst::IBinI(IOp::Mul, W, o, seed, 5));
    for &x in &js {
        push(&mut f, b0, Inst::Iconst(x, 0));
    }
    push(&mut f, b0, Inst::Iconst(j, 0));
    f.blocks[b0 as usize].term = Term::Jump(if outer > 0 { hs[0] } else { mh });
    // outer loop d: header hs[d] (j_d < 1: in, else to the latch of loop d - 1 or the end),
    // latch ls[d] (j_d += 1, the next inner counter reset)
    for d in 0..outer {
        let c = f.vreg(Cls::I);
        push(&mut f, hs[d], Inst::ICmpI(Cond::Lt, false, c, js[d], 1));
        let inner = if d + 1 < outer { hs[d + 1] } else { mh };
        let out = if d > 0 { ls[d - 1] } else { fin };
        f.blocks[hs[d] as usize].term = Term::Branch(c, inner, out);
        push(&mut f, ls[d], Inst::IBinI(IOp::Add, W, js[d], js[d], 1));
        push(&mut f, ls[d], Inst::Iconst(if d + 1 < outer { js[d + 1] } else { j }, 0));
        f.blocks[ls[d] as usize].term = Term::Jump(hs[d]);
    }
    let c = f.vreg(Cls::I);
    push(&mut f, mh, Inst::ICmpI(Cond::Lt, false, c, j, M));
    f.blocks[mh as usize].term = Term::Branch(c, mb, if outer > 0 { ls[outer - 1] } else { fin });
    push(&mut f, mb, Inst::IBin(IOp::Xor, W, acc, acc, o));
    push(&mut f, mb, Inst::Iconst(i, 0));
    f.blocks[mb as usize].term = Term::Jump(ib);
    let q = burst(&mut f, ib, acc, 3);
    push(&mut f, ib, Inst::IBin(IOp::Add, W, acc, q, i));
    f.blocks[ib as usize].term = Term::Jump(il);
    let ci = f.vreg(Cls::I);
    push(&mut f, il, Inst::IBinI(IOp::Add, W, i, i, 1));
    push(&mut f, il, Inst::ICmp(Cond::Lt, false, ci, i, n));
    f.blocks[il as usize].term = Term::Branch(ci, ib, mx);
    push(&mut f, mx, Inst::IBin(IOp::Add, W, acc, acc, o));
    push(&mut f, mx, Inst::IBinI(IOp::Add, W, j, j, 1));
    f.blocks[mx as usize].term = Term::Jump(mh);
    f.blocks[fin as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    let mut freq = vec![0.0f32; f.blocks.len()];
    for b in [mh, mb, ib, mx] {
        freq[b as usize] = 1.0;
    }
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

fn reference(n: u64, seed: u64) -> u64 {
    let (mut acc, o) = (seed, seed.wrapping_mul(5));
    for _ in 0..M {
        acc ^= o;
        let mut i = 0u64;
        loop {
            acc = burst_ref(acc, 3).wrapping_add(i);
            i += 1;
            if i >= n {
                break;
            }
        }
        acc = acc.wrapping_add(o);
    }
    acc
}

#[test]
fn inner_latch_never_loads_for_the_exit() {
    for outer in [0usize, 6] {
        let (f, freq) = build(outer);
        for fr in [None, Some(&freq[..])] {
            let code = Code::new(&compile(&f, fr)).unwrap();
            let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            for (n, seed) in [(0u64, 1u64), (1, 7), (5, 0x1234), (17, 0x9e3779b97f4a7c15)] {
                assert_eq!(run(n, seed), reference(n, seed), "outer {} freq {} n {} seed {}", outer, fr.is_some(), n, seed);
            }
        }
        let (g, al) = allocate_split(&options(), &f, &reg_config(), true, Some(&freq));
        let h = g.as_ref().unwrap_or(&f);
        // by their code (the allocated function may add and reorder blocks): o, the inner latch
        // (i < n), the burst block and the inner loop's exit (j += 1)
        let o = h.blocks.iter().flat_map(|b| b.insts.iter()).find_map(|i| if let Inst::IBinI(IOp::Mul, _, d, _, 5) = i { Some(*d) } else { None }).unwrap();
        let latch = h.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::ICmp(Cond::Lt, ..)))).unwrap();
        let burst_b = h.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Rotl, _, _, _, 5)))).unwrap();
        let exit = h.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Add, _, _, _, 1))) && b.insts.iter().any(|i| matches!(i, Inst::IBin(IOp::Add, _, _, _, x) if *x == o))).unwrap();
        assert!(matches!(al.loc[o.0 as usize], Loc::Stack(_)), "outer {}: the fixture needs o spilled: {:?}", outer, al.loc[o.0 as usize]);
        let in_block = |p: u32, b: usize| al.block_start[b] <= p && p <= al.block_end[b];
        assert!(al.windows.iter().chain(al.chained.iter()).any(|w| w.0 == o.0 && in_block(w.1, exit)), "outer {}: o is in a register in the inner loop's exit b{}", outer, exit);
        for b in [latch, burst_b] {
            let loads: Vec<_> = al.windows.iter().filter(|w| w.0 == o.0 && in_block(w.1, b)).collect();
            assert!(loads.is_empty(), "outer {}: o loaded in the inner loop's b{} (latch b{} exit b{}): {:?}", outer, b, latch, exit, loads);
        }
    }
}
