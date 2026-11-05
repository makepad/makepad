//! A spilled value defined on a hot loop's path and held in a register over that path (a region
//! window) but also read on a branch taken one iteration in four where no register is free (a hot
//! loop's segment fields, read again on its colder accept path) stored its slot at every
//! definition: one store per iteration for a read that runs every fourth. The store moves to that
//! block's start (regalloc::dead_spill_stores, Alloc::exit_stores): with sampled frequencies the
//! definition skips its store and the cold block stores the register. Where that block is also
//! entered from outside the region, the region's cold block before it stores before its branch;
//! a select reading the value in the region reads the register (no slot read). The allocation
//! checker runs on every compile, and results must equal Rust's.
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

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// m(n, seed): acc = seed; k = seed | 1; i = 0; while i < n { d = (acc ^ k) + i; i += 1; if d &
/// 3 == 0 { acc = burst(burst(burst(acc, 3), 5), 7) ^ d } else { acc = d.rotl(7) } }; acc. d is
/// live over the three bursts (no register there, and the bursts' temporaries outweigh it: it
/// spills) and read after them. Returns the function and its sampled block frequencies (the loop
/// 1, the bursts 1/4, the rest never sampled).
fn build() -> (Func, Vec<f32>) {
    let mut f = Func::new("region_exit_stores".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    let (b0, b1, b2, b3, b4, fin) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
    let (acc, k, i) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Mov(acc, seed));
    push(&mut f, b0, Inst::IBinI(IOp::Or, W, k, seed, 1));
    push(&mut f, b0, Inst::Iconst(i, 0));
    f.blocks[b0 as usize].term = Term::Jump(b1);
    let c = f.vreg(Cls::I);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b1 as usize].term = Term::Branch(c, b2, fin);
    let (x, d, bits, z) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b2, Inst::IBin(IOp::Xor, W, x, acc, k));
    push(&mut f, b2, Inst::IBin(IOp::Add, W, d, x, i));
    push(&mut f, b2, Inst::IBinI(IOp::Add, W, i, i, 1));
    push(&mut f, b2, Inst::IBinI(IOp::And, W, bits, d, 3));
    push(&mut f, b2, Inst::ICmpI(Cond::Eq, false, z, bits, 0));
    f.blocks[b2 as usize].term = Term::Branch(z, b4, b3);
    push(&mut f, b3, Inst::IBinI(IOp::Rotl, W, acc, d, 7));
    f.blocks[b3 as usize].term = Term::Jump(b1);
    let q = burst(&mut f, b4, acc, 3);
    let q = burst(&mut f, b4, q, 5);
    let q = burst(&mut f, b4, q, 7);
    push(&mut f, b4, Inst::IBin(IOp::Xor, W, acc, q, d));
    f.blocks[b4 as usize].term = Term::Jump(b1);
    f.blocks[fin as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    let mut freq = vec![0.0f32; f.blocks.len()];
    for b in [b1, b2] {
        freq[b as usize] = 1.0;
    }
    freq[b3 as usize] = 0.75;
    freq[b4 as usize] = 0.25;
    (f, freq)
}

/// t_m = y + (m + base) * 97 for m < T, all live, then folded in reverse, each read five times
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
        push(f, b, Inst::IBin(IOp::Add, W, q, q, t[m]));
        push(f, b, Inst::IBin(IOp::Mul, W, q, q, t[m]));
    }
    q
}

fn burst_ref(y: u64, base: u64) -> u64 {
    let mut q = 0u64;
    for m in (0..T as u64).rev() {
        let t = y.wrapping_add((m + base) * 97);
        q = (q.rotate_left(5) ^ t).wrapping_add(t >> 3).wrapping_sub(t).wrapping_add(t).wrapping_mul(t);
    }
    q
}

fn reference(n: u64, seed: u64) -> u64 {
    let (mut acc, k) = (seed, seed | 1);
    let mut i = 0u64;
    while i < n {
        let d = (acc ^ k).wrapping_add(i);
        i += 1;
        acc = if d & 3 == 0 { burst_ref(burst_ref(burst_ref(acc, 3), 5), 7) ^ d } else { d.rotate_left(7) };
    }
    acc
}

#[test]
fn cold_read_stores_on_the_way_out() {
    let (f, freq) = build();
    for fr in [None, Some(&freq[..])] {
        let code = Code::new(&compile(&f, fr)).unwrap();
        let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
        for (n, seed) in [(0u64, 1u64), (1, 7), (16, 0), (63, 0x1234), (64, 0x9e3779b97f4a7c15), (1001, 3)] {
            assert_eq!(run(n, seed), reference(n, seed), "freq {} n {} seed {}", fr.is_some(), n, seed);
        }
    }
    let (g, al) = allocate_split(&options(), &f, &reg_config(), true, Some(&freq));
    let h = g.as_ref().unwrap_or(&f);
    // the loop body (d & 3) and the bursts' block (the allocated function may add and reorder blocks)
    let body = h.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::And, _, _, _, 3)))).unwrap();
    let cold = h.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Rotl, _, _, _, 5)))).unwrap();
    let d = h.blocks[body].insts.iter().find_map(|i| if let Inst::IBinI(IOp::And, _, _, s, 3) = i { Some(*s) } else { None }).unwrap();
    let di = h.blocks[body].insts.iter().position(|i| matches!(i, Inst::IBin(IOp::Add, _, x, ..) if *x == d)).unwrap();
    assert!(matches!(al.loc[d.0 as usize], Loc::Stack(_)), "the fixture needs d spilled: {:?}", al.loc[d.0 as usize]);
    let p = al.block_start[body] + 2 * di as u32 + 1;
    let ex = al.exit_stores.get(&al.block_start[cold]).cloned().unwrap_or_default();
    assert!(al.store_skip.contains(&(p, d.0)), "d is stored at its definition on the hot path (body b{} cold b{}): exit stores {:?}, chained {:?}", body, cold, al.exit_stores, al.chained.iter().filter(|w| w.0 == d.0).collect::<Vec<_>>());
    assert!(ex.iter().any(|x| x.0 == d.0), "the burst block stores d at its start: {:?}", al.exit_stores);
}

/// As `build`, with the cold side split: `if d & 3 == 0 { if d & 4 != 0 { acc = burst(acc, 11)
/// } acc = burst(burst(burst(acc, 3), 5), 7) ^ d } else { acc = if i < 1000 { d } else { x } }`
/// (x = acc ^ k; the select reads d). The bursts' block is entered from the region's cold block
/// (d & 4) and from the extra burst, which no register survives: d goes home before the cold
/// block's branch. Frequencies: the loop 1, its latch 3/4, the cold block and the bursts 1/4, the
/// extra burst 1/8.
fn build_join() -> (Func, Vec<f32>) {
    let mut f = Func::new("region_exit_stores".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    let (b0, b1, b2, b3, b4, fin, b5, b6) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
    let (acc, k, i) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Mov(acc, seed));
    push(&mut f, b0, Inst::IBinI(IOp::Or, W, k, seed, 1));
    push(&mut f, b0, Inst::Iconst(i, 0));
    f.blocks[b0 as usize].term = Term::Jump(b1);
    let c = f.vreg(Cls::I);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b1 as usize].term = Term::Branch(c, b2, fin);
    let (x, d, bits, z) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b2, Inst::IBin(IOp::Xor, W, x, acc, k));
    push(&mut f, b2, Inst::IBin(IOp::Add, W, d, x, i));
    push(&mut f, b2, Inst::IBinI(IOp::Add, W, i, i, 1));
    push(&mut f, b2, Inst::IBinI(IOp::And, W, bits, d, 3));
    push(&mut f, b2, Inst::ICmpI(Cond::Eq, false, z, bits, 0));
    f.blocks[b2 as usize].term = Term::Branch(z, b6, b3);
    let c3 = f.vreg(Cls::I);
    push(&mut f, b3, Inst::ICmpI(Cond::Lt, false, c3, i, 1000));
    push(&mut f, b3, Inst::Sel(acc, c3, d, x));
    f.blocks[b3 as usize].term = Term::Jump(b1);
    let (e, ce) = (f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b6, Inst::IBinI(IOp::And, W, e, d, 4));
    push(&mut f, b6, Inst::ICmpI(Cond::Eq, false, ce, e, 0));
    f.blocks[b6 as usize].term = Term::Branch(ce, b4, b5);
    let q = burst(&mut f, b5, acc, 11);
    push(&mut f, b5, Inst::Mov(acc, q));
    f.blocks[b5 as usize].term = Term::Jump(b4);
    let q = burst(&mut f, b4, acc, 3);
    let q = burst(&mut f, b4, q, 5);
    let q = burst(&mut f, b4, q, 7);
    push(&mut f, b4, Inst::IBin(IOp::Xor, W, acc, q, d));
    f.blocks[b4 as usize].term = Term::Jump(b1);
    f.blocks[fin as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    let mut freq = vec![0.0f32; f.blocks.len()];
    for b in [b1, b2] {
        freq[b as usize] = 1.0;
    }
    freq[b3 as usize] = 0.75;
    freq[b6 as usize] = 0.25;
    freq[b4 as usize] = 0.25;
    freq[b5 as usize] = 0.125;
    (f, freq)
}

fn reference_join(n: u64, seed: u64) -> u64 {
    let (mut acc, k) = (seed, seed | 1);
    let mut i = 0u64;
    while i < n {
        let x = acc ^ k;
        let d = x.wrapping_add(i);
        i += 1;
        if d & 3 == 0 {
            if d & 4 != 0 {
                acc = burst_ref(acc, 11);
            }
            acc = burst_ref(burst_ref(burst_ref(acc, 3), 5), 7) ^ d;
        } else {
            acc = if i < 1000 { d } else { x };
        }
    }
    acc
}

#[test]
fn join_stores_before_the_branch() {
    let (f, freq) = build_join();
    for fr in [None, Some(&freq[..])] {
        let code = Code::new(&compile(&f, fr)).unwrap();
        let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
        for (n, seed) in [(0u64, 1u64), (1, 7), (16, 0), (63, 0x1234), (64, 0x9e3779b97f4a7c15), (1001, 3)] {
            assert_eq!(run(n, seed), reference_join(n, seed), "freq {} n {} seed {}", fr.is_some(), n, seed);
        }
    }
    let (g, al) = allocate_split(&options(), &f, &reg_config(), true, Some(&freq));
    let h = g.as_ref().unwrap_or(&f);
    // the loop body (d & 3) and the region's cold block (d & 4)
    let body = h.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::And, _, _, _, 3)))).unwrap();
    let cold = h.blocks.iter().position(|b| b.insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::And, _, _, _, 4)))).unwrap();
    let d = h.blocks[body].insts.iter().find_map(|i| if let Inst::IBinI(IOp::And, _, _, s, 3) = i { Some(*s) } else { None }).unwrap();
    let di = h.blocks[body].insts.iter().position(|i| matches!(i, Inst::IBin(IOp::Add, _, x, ..) if *x == d)).unwrap();
    assert!(matches!(al.loc[d.0 as usize], Loc::Stack(_)), "the fixture needs d spilled: {:?}", al.loc[d.0 as usize]);
    let p = al.block_start[body] + 2 * di as u32 + 1;
    let ex = al.exit_stores.get(&(al.block_end[cold] - 1)).cloned().unwrap_or_default();
    assert!(al.store_skip.contains(&(p, d.0)), "d is stored at its definition on the hot path (body b{} cold b{}): exit stores {:?}, chained {:?}", body, cold, al.exit_stores, al.chained.iter().filter(|w| w.0 == d.0).collect::<Vec<_>>());
    assert!(ex.iter().any(|x| x.0 == d.0), "the cold block stores d before its branch: {:?}", al.exit_stores);
}
