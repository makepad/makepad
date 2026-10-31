//! A recomputed base + k whose base is base + k itself (`v = w + 8`, `w = a + 1000`) folds down to
//! `a` at its uses, so `a` must hold its value there too -- not only `w`. The loop split parks `a`
//! (live through the loop, never read in it: only the chain reads it) in a copy around a loop that
//! needs the registers while `w` stays live past it; `v` is read inside the loop: the load must not
//! use `a`'s register, which the loop's values take while `a` is parked. (Sandbox race x64 segfault
//! at 0x2948 in Renderer::draw_scene_inner: `self` parked around a set_uniform loop, the loop counter
//! in its register.)
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

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

/// loop accumulators (more than the registers: spilled references in the loop) and uses of `a`
/// after it (a register in the first allocation, so the split parks it)
const K: usize = 14;
const H: usize = 200;

/// f(a, n) with a = &buf - 1008: w = a + 1000; v = w + 8 (= &buf); K accumulators over i < n:
/// acc_k += buf[i] * (k + 1); after the loop (w - a) ^ a*3 ^ .. (H uses of `a`: a register outside
/// the loop) ^ acc_0 ^ .. ^ acc_K-1.
fn build() -> Func {
    let u = IntTy { bits: 64, signed: false };
    let mut f = Func::new("remat_offset_chain".into(), 0);
    let (a, n) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, n];
    f.rets = vec![Cls::I];
    let (b0, b1, b2, b3) = (f.block(), f.block(), f.block(), f.block());
    let (w, v, i) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::IBinI(IOp::Add, u, w, a, 1000));
    push(&mut f, b0, Inst::IBinI(IOp::Add, u, v, w, 8));
    push(&mut f, b0, Inst::Iconst(i, 0));
    let accs: Vec<VReg> = (0..K).map(|_| f.vreg(Cls::I)).collect();
    for acc in &accs {
        push(&mut f, b0, Inst::Iconst(*acc, 0));
    }
    f.blocks[b0 as usize].term = Term::Jump(b1);
    let c = f.vreg(Cls::I);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b1 as usize].term = Term::Branch(c, b2, b3);
    let x = f.vreg(Cls::I);
    push(&mut f, b2, Inst::LoadX(Mem::Int(8, false), x, v, i, 3, 0));
    for (k, acc) in accs.iter().enumerate() {
        let m = f.vreg(Cls::I);
        push(&mut f, b2, Inst::IBinI(IOp::Mul, u, m, x, k as i64 + 1));
        push(&mut f, b2, Inst::IBin(IOp::Add, u, *acc, *acc, m));
    }
    push(&mut f, b2, Inst::IBinI(IOp::Add, u, i, i, 1));
    f.blocks[b2 as usize].term = Term::Jump(b1);
    let mut r = f.vreg(Cls::I);
    push(&mut f, b3, Inst::IBin(IOp::Sub, u, r, w, a));
    for j in 0..H {
        let (m, t) = (f.vreg(Cls::I), f.vreg(Cls::I));
        push(&mut f, b3, Inst::IBinI(IOp::Mul, u, m, a, j as i64 + 3));
        push(&mut f, b3, Inst::IBin(IOp::Xor, u, t, r, m));
        r = t;
    }
    for acc in &accs {
        let t = f.vreg(Cls::I);
        push(&mut f, b3, Inst::IBin(IOp::Xor, u, t, r, *acc));
        r = t;
    }
    f.blocks[b3 as usize].term = Term::Ret(vec![r]);
    f.verify().unwrap();
    f
}

#[test]
fn offset_chain_base_parked() {
    let code = Code::new(&compile(&build())).unwrap();
    let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    let buf: Vec<u64> = (0..64u64).map(|j| j.wrapping_mul(0x9e3779b97f4a7c15) ^ 0x42).collect();
    let a = (buf.as_ptr() as u64).wrapping_sub(1008);
    for n in [0usize, 1, 7, 64] {
        let mut want = 1000u64;
        for j in 0..H as u64 {
            want ^= a.wrapping_mul(j + 3);
        }
        for k in 0..K as u64 {
            want ^= buf[..n].iter().fold(0u64, |s, x| s.wrapping_add(x.wrapping_mul(k + 1)));
        }
        assert_eq!(run(a, n as u64), want, "n = {}", n);
    }
}
