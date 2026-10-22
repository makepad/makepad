//! x64 scalar FMA operand forms: the result in the accumulator's register (vfmadd231), in b's
//! (213 with a and b swapped) or in a's / elsewhere (213), for every negation form, over chains
//! that keep many values live (xmm16..31 on AVX-512VL hosts). Every result must equal Rust's
//! `mul_add` bit for bit (no NaN inputs).
#![cfg(all(target_arch = "x86_64", unix))]
use relax_backend::{exec::Code, rir::*};

type Entry = extern "sysv64" fn(*const u8, *mut u8);

const N: usize = 48;
const FORMS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

fn options() -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
    o
}

/// fn(src: *const [x; N], dst: *mut [d; 2N]): acc_i = fma_i(x_i, x_{i+1}, acc_{i-1}) (c dies: the
/// accumulator form), e_i = fma_i(x_{i+5}, x_i, acc_i) (b's last use: the swapped form) -- every
/// x live until the end (many registers), dst = acc_0.. and e_0..
fn build(w: bool) -> Code {
    let (cls, mem, sz) = if w { (Cls::F64, Mem::F64, 8) } else { (Cls::F32, Mem::F32, 4) };
    let mut f = Func::new("fma_forms".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let mut insts = Vec::new();
    let xs: Vec<VReg> = (0..N).map(|_| f.vreg(cls)).collect();
    for (i, x) in xs.iter().enumerate() {
        insts.push(Inst::Load(mem, *x, ps, i as i32 * sz));
    }
    let mut acc = xs[N - 1];
    let mut outs = Vec::new();
    for i in 0..N - 6 {
        let (np, na) = FORMS[i % 4];
        let (a, e) = (f.vreg(cls), f.vreg(cls));
        insts.push(Inst::FMulAdd(np, na, w, a, xs[i], xs[i + 1], acc));
        let (np2, na2) = FORMS[(i + 1) % 4];
        insts.push(Inst::FMulAdd(np2, na2, w, e, xs[i + 5], xs[i], a));
        outs.push(a);
        outs.push(e);
        acc = a;
    }
    for (k, v) in outs.iter().enumerate() {
        insts.push(Inst::Store(mem, pd, k as i32 * sz, *v));
    }
    // keep every x live to the end
    for (k, x) in xs.iter().enumerate() {
        insts.push(Inst::Store(mem, pd, (outs.len() + k) as i32 * sz, *x));
    }
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&options(), &f, &env).unwrap();
    Code::new(&c.code).unwrap()
}

fn fma<T: Copy + std::ops::Neg<Output = T>>(np: bool, na: bool, a: T, b: T, c: T, m: fn(T, T, T) -> T) -> T {
    m(if np { -a } else { a }, b, if na { -c } else { c })
}

#[test]
fn every_fma_form_matches_mul_add() {
    if !options().cpu.fma {
        return;
    }
    let x64: Vec<f64> = (0..N).map(|i| 0.75 + i as f64 * 0.311 - (i % 7) as f64 * 0.5).collect();
    let x32: Vec<f32> = x64.iter().map(|&v| v as f32).collect();
    for w in [true, false] {
        let code = build(w);
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        if w {
            let mut out = vec![0f64; 2 * (N - 6) + N];
            run(x64.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
            let mut acc = x64[N - 1];
            for i in 0..N - 6 {
                let (np, na) = FORMS[i % 4];
                let a = fma(np, na, x64[i], x64[i + 1], acc, f64::mul_add);
                let (np2, na2) = FORMS[(i + 1) % 4];
                let e = fma(np2, na2, x64[i + 5], x64[i], a, f64::mul_add);
                assert_eq!(out[2 * i].to_bits(), a.to_bits(), "f64 acc {}", i);
                assert_eq!(out[2 * i + 1].to_bits(), e.to_bits(), "f64 e {}", i);
                acc = a;
            }
        } else {
            let mut out = vec![0f32; 2 * (N - 6) + N];
            run(x32.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
            let mut acc = x32[N - 1];
            for i in 0..N - 6 {
                let (np, na) = FORMS[i % 4];
                let a = fma(np, na, x32[i], x32[i + 1], acc, f32::mul_add);
                let (np2, na2) = FORMS[(i + 1) % 4];
                let e = fma(np2, na2, x32[i + 5], x32[i], a, f32::mul_add);
                assert_eq!(out[2 * i].to_bits(), a.to_bits(), "f32 acc {}", i);
                assert_eq!(out[2 * i + 1].to_bits(), e.to_bits(), "f32 e {}", i);
                acc = a;
            }
        }
    }
}

/// A float Load used once by the next instruction becomes its memory operand: every FBin op
/// with the load as either operand, and an FMA with the load as a, b or c, with the other
/// operands live (results in any register, incl. xmm16..31).
fn build_loads(w: bool) -> Code {
    let (cls, mem, sz) = if w { (Cls::F64, Mem::F64, 8) } else { (Cls::F32, Mem::F32, 4) };
    let mut f = Func::new("fold_loads".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let mut insts = Vec::new();
    let xs: Vec<VReg> = (0..N).map(|_| f.vreg(cls)).collect();
    for (i, x) in xs.iter().enumerate() {
        insts.push(Inst::Load(mem, *x, ps, i as i32 * sz));
    }
    let mut outs = Vec::new();
    for i in 0..N {
        let t = f.vreg(cls);
        let d = f.vreg(cls);
        insts.push(Inst::Load(mem, t, ps, ((i * 7 + 3) % N) as i32 * sz));
        let x = xs[(i + 1) % N];
        insts.push(match i % 7 {
            0 => Inst::FBin(FOp::Add, w, d, x, t),
            1 => Inst::FBin(FOp::Sub, w, d, x, t),
            2 => Inst::FBin(FOp::Mul, w, d, t, x),
            3 => Inst::FBin(FOp::Div, w, d, x, t),
            4 => Inst::FMulAdd(false, false, w, d, x, xs[(i + 2) % N], t),
            5 => Inst::FMulAdd(false, false, w, d, x, t, xs[(i + 3) % N]),
            _ => Inst::FMulAdd(false, false, w, d, t, x, xs[(i + 4) % N]),
        });
        outs.push(d);
    }
    for (k, v) in outs.iter().chain(xs.iter()).enumerate() {
        insts.push(Inst::Store(mem, pd, k as i32 * sz, *v));
    }
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&options(), &f, &env).unwrap();
    Code::new(&c.code).unwrap()
}

fn loads_ref<T: Copy + std::ops::Add<Output = T> + std::ops::Sub<Output = T> + std::ops::Mul<Output = T> + std::ops::Div<Output = T>>(x: &[T], m: fn(T, T, T) -> T) -> Vec<T> {
    (0..N)
        .map(|i| {
            let t = x[(i * 7 + 3) % N];
            let y = x[(i + 1) % N];
            match i % 7 {
                0 => y + t,
                1 => y - t,
                2 => t * y,
                3 => y / t,
                4 => m(y, x[(i + 2) % N], t),
                5 => m(y, t, x[(i + 3) % N]),
                _ => m(t, y, x[(i + 4) % N]),
            }
        })
        .collect()
}

#[test]
fn folded_float_loads_compute_the_same_bits() {
    if !options().cpu.fma {
        return;
    }
    let x64: Vec<f64> = (0..N).map(|i| 0.75 + i as f64 * 0.311 - (i % 7) as f64 * 0.5).collect();
    let x32: Vec<f32> = x64.iter().map(|&v| v as f32).collect();
    let code = build_loads(true);
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    let mut out = vec![0f64; 2 * N];
    run(x64.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
    for (i, want) in loads_ref(&x64, f64::mul_add).iter().enumerate() {
        assert_eq!(out[i].to_bits(), want.to_bits(), "f64 {}", i);
    }
    let code = build_loads(false);
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    let mut out = vec![0f32; 2 * N];
    run(x32.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
    for (i, want) in loads_ref(&x32, f32::mul_add).iter().enumerate() {
        assert_eq!(out[i].to_bits(), want.to_bits(), "f32 {}", i);
    }
}
