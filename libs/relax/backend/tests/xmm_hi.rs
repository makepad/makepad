//! x64 with AVX-512VL allocates xmm16..31 (reg_config): values there are read and written through
//! EVEX moves (or in place by the EVEX arithmetic forms). Many float, double and vector values live
//! at once force the allocator into the upper registers; every result must equal the Rust reference
//! bit for bit, and on such a host the code must contain EVEX instructions.
#![cfg(all(target_arch = "x86_64", unix))]
use relax_backend::{exec::Code, rir::*};

type Entry = extern "sysv64" fn(*const u8, *mut u8);

fn options() -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
    o
}

const N: usize = 40;

/// fn(src, dst): x[i] = load src[i]; d[i] = (x[i] * x[(i+1)%N] + x[(i+7)%N]) - x[(i+13)%N] / x[(i+3)%N],
/// every x live until the last d is computed; then dst[i] = d[i] + x[i].
fn build(cls: Cls, mem: Mem, sz: i32, vector: bool) -> (Code, bool) {
    let mut f = Func::new("xmm_hi".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let mut insts = Vec::new();
    let xs: Vec<VReg> = (0..N).map(|_| f.vreg(cls)).collect();
    for (i, x) in xs.iter().enumerate() {
        insts.push(if vector { Inst::VLoad(*x, ps, i as i32 * 16) } else { Inst::Load(mem, *x, ps, i as i32 * sz) });
    }
    let f64_ = cls == Cls::F64;
    let mut ds = Vec::new();
    for i in 0..N {
        let (m, a, q, d) = (f.vreg(cls), f.vreg(cls), f.vreg(cls), f.vreg(cls));
        if vector {
            insts.push(Inst::VX(VX::FMul(VW::S, NanBits::Host), m, xs[i], xs[(i + 1) % N]));
            insts.push(Inst::VX(VX::FAdd(VW::S, NanBits::Host), a, m, xs[(i + 7) % N]));
            insts.push(Inst::VX(VX::FDiv(VW::S, NanBits::Host), q, xs[(i + 13) % N], xs[(i + 3) % N]));
            insts.push(Inst::VX(VX::FSub(VW::S, NanBits::Host), d, a, q));
        } else {
            insts.push(Inst::FBin(FOp::Mul, f64_, m, xs[i], xs[(i + 1) % N]));
            insts.push(Inst::FBin(FOp::Add, f64_, a, m, xs[(i + 7) % N]));
            insts.push(Inst::FBin(FOp::Div, f64_, q, xs[(i + 13) % N], xs[(i + 3) % N]));
            insts.push(Inst::FBin(FOp::Sub, f64_, d, a, q));
        }
        ds.push(d);
    }
    for i in 0..N {
        let r = f.vreg(cls);
        if vector {
            insts.push(Inst::VX(VX::FAdd(VW::S, NanBits::Host), r, ds[i], xs[i]));
            insts.push(Inst::VStore(pd, i as i32 * 16, r));
        } else {
            insts.push(Inst::FBin(FOp::Add, f64_, r, ds[i], xs[i]));
            insts.push(Inst::Store(mem, pd, i as i32 * sz, r));
        }
    }
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&options(), &f, &env).unwrap();
    assert!(c.relocs.is_empty());
    let evex = c.code.windows(2).any(|w| w[0] == 0x62 && w[1] & 0x0c == 0);
    (Code::new(&c.code).unwrap(), evex)
}

fn reference<T: Copy + std::ops::Mul<Output = T> + std::ops::Add<Output = T> + std::ops::Sub<Output = T> + std::ops::Div<Output = T>>(x: &[T]) -> Vec<T> {
    let n = x.len();
    (0..n).map(|i| ((x[i] * x[(i + 1) % n] + x[(i + 7) % n]) - x[(i + 13) % n] / x[(i + 3) % n]) + x[i]).collect()
}

#[test]
fn many_live_floats_use_the_upper_registers() {
    let avx512 = options().cpu.avx512;
    for (cls, mem, sz) in [(Cls::F32, Mem::F32, 4), (Cls::F64, Mem::F64, 8)] {
        let (code, evex) = build(cls, mem, sz, false);
        assert!(!avx512 || evex, "AVX-512VL host: no EVEX code ({:?})", cls);
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        if cls == Cls::F32 {
            let x: Vec<f32> = (0..N).map(|i| 1.0 + i as f32 * 0.37 - (i % 5) as f32).collect();
            let mut out = vec![0f32; N];
            run(x.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
            let want = reference(&x);
            for i in 0..N {
                assert_eq!(out[i].to_bits(), want[i].to_bits(), "f32 lane {}", i);
            }
        } else {
            let x: Vec<f64> = (0..N).map(|i| 1.0 + i as f64 * 0.37 - (i % 5) as f64).collect();
            let mut out = vec![0f64; N];
            run(x.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
            let want = reference(&x);
            for i in 0..N {
                assert_eq!(out[i].to_bits(), want[i].to_bits(), "f64 {}", i);
            }
        }
    }
}

#[test]
fn many_live_vectors_use_the_upper_registers() {
    let avx512 = options().cpu.avx512;
    let (code, evex) = build(Cls::V128, Mem::F32, 16, true);
    assert!(!avx512 || evex, "AVX-512VL host: no EVEX code");
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    let x: Vec<f32> = (0..N * 4).map(|i| 1.0 + i as f32 * 0.13 - (i % 7) as f32).collect();
    let mut out = vec![0f32; N * 4];
    run(x.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
    for lane in 0..4 {
        let xl: Vec<f32> = (0..N).map(|i| x[i * 4 + lane]).collect();
        let want = reference(&xl);
        for i in 0..N {
            assert_eq!(out[i * 4 + lane].to_bits(), want[i].to_bits(), "vector {} lane {}", i, lane);
        }
    }
}

/// fn(src, dst): x[i] = load src[i]; s[i] = shuffle(x[i], K[i % 4]) + x[(i + 5) % N], every x and s
/// live until the end; dst[i] = s[i] - x[i]: the lane shuffles (vpshufd) read and write xmm16..31 in
/// place on an AVX-512VL host.
#[test]
fn many_live_shuffles() {
    const K: [u8; 4] = [0b11_00_10_01, 0b11_01_00_10, 0b00_01_10_11, 0b01_01_01_01];
    let mut f = Func::new("xmm_hi_shuf".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let mut insts = Vec::new();
    let xs: Vec<VReg> = (0..N).map(|_| f.vreg(Cls::V128)).collect();
    for (i, x) in xs.iter().enumerate() {
        insts.push(Inst::VLoad(*x, ps, i as i32 * 16));
    }
    let mut ss = Vec::new();
    for i in 0..N {
        let (t, s) = (f.vreg(Cls::V128), f.vreg(Cls::V128));
        insts.push(Inst::VX(VX::ShufS(K[i % 4]), t, xs[i], xs[i]));
        insts.push(Inst::VX(VX::FAdd(VW::S, NanBits::Host), s, t, xs[(i + 5) % N]));
        ss.push(s);
    }
    for i in 0..N {
        let r = f.vreg(Cls::V128);
        insts.push(Inst::VX(VX::FSub(VW::S, NanBits::Host), r, ss[i], xs[i]));
        insts.push(Inst::VStore(pd, i as i32 * 16, r));
    }
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    for o in [options(), { let mut o = relax_session::Options::default(); o.no_avx = true; o.cpu = relax_backend::x64::detect_cpu(&o); o }] {
        let c = relax_backend::x64::compile(&o, &f, &env).unwrap();
        let code = Code::new(&c.code).unwrap();
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        let x: Vec<f32> = (0..N * 4).map(|i| 1.0 + i as f32 * 0.13 - (i % 7) as f32).collect();
        let mut out = vec![0f32; N * 4];
        run(x.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
        for i in 0..N {
            for lane in 0..4 {
                let src = (K[i % 4] as usize >> (2 * lane)) & 3;
                let want = (x[i * 4 + src] + x[((i + 5) % N) * 4 + lane]) - x[i * 4 + lane];
                assert_eq!(out[i * 4 + lane].to_bits(), want.to_bits(), "no_avx {} vector {} lane {}", o.no_avx, i, lane);
            }
        }
    }
}

/// fn(src, dst) for F32 / F64: dst[i] = (fma(-x[i], x[i+1], x[i+7]) + 2.5) + x[i], every x live until
/// the end (FUnary, FMulAdd and Fconst in the upper registers too).
fn build_fma(f64_: bool) -> Code {
    let (cls, mem, sz) = if f64_ { (Cls::F64, Mem::F64, 8) } else { (Cls::F32, Mem::F32, 4) };
    let mut f = Func::new("xmm_hi_fma".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let mut insts = Vec::new();
    let xs: Vec<VReg> = (0..N).map(|_| f.vreg(cls)).collect();
    for (i, x) in xs.iter().enumerate() {
        insts.push(Inst::Load(mem, *x, ps, i as i32 * sz));
    }
    let k = f.vreg(cls);
    insts.push(Inst::Fconst(k, if f64_ { 2.5f64.to_bits() } else { 2.5f32.to_bits() as u64 }, f64_));
    let mut ds = Vec::new();
    for i in 0..N {
        let (n, m, a) = (f.vreg(cls), f.vreg(cls), f.vreg(cls));
        insts.push(Inst::FUnary(FUn::Neg, f64_, n, xs[i]));
        insts.push(Inst::FMulAdd(false, false, f64_, m, n, xs[(i + 1) % N], xs[(i + 7) % N]));
        insts.push(Inst::FBin(FOp::Add, f64_, a, m, k));
        ds.push(a);
    }
    for i in 0..N {
        let r = f.vreg(cls);
        insts.push(Inst::FBin(FOp::Add, f64_, r, ds[i], xs[i]));
        insts.push(Inst::Store(mem, pd, i as i32 * sz, r));
    }
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&options(), &f, &env).unwrap();
    Code::new(&c.code).unwrap()
}

#[test]
fn many_live_fma_operands() {
    if !options().cpu.fma {
        return;
    }
    for f64_ in [false, true] {
        let code = build_fma(f64_);
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        if f64_ {
            let x: Vec<f64> = (0..N).map(|i| 1.0 + i as f64 * 0.37 - (i % 5) as f64).collect();
            let mut out = vec![0f64; N];
            run(x.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
            for i in 0..N {
                let want = ((-x[i]).mul_add(x[(i + 1) % N], x[(i + 7) % N]) + 2.5) + x[i];
                assert_eq!(out[i].to_bits(), want.to_bits(), "f64 {}", i);
            }
        } else {
            let x: Vec<f32> = (0..N).map(|i| 1.0 + i as f32 * 0.37 - (i % 5) as f32).collect();
            let mut out = vec![0f32; N];
            run(x.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8);
            for i in 0..N {
                let want = ((-x[i]).mul_add(x[(i + 1) % N], x[(i + 7) % N]) + 2.5) + x[i];
                assert_eq!(out[i].to_bits(), want.to_bits(), "f32 {}", i);
            }
        }
    }
}

#[test]
fn block_copies_of_every_size() {
    // Copy of 32..=512 bytes (16-byte xmm pieces, then 8/4/2/1-byte gpr pieces)
    for n in [32u32, 33, 47, 48, 63, 64, 100, 200, 255, 256, 300, 511, 512] {
        let mut f = Func::new("copy".into(), 0);
        let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params = vec![ps, pd];
        f.block();
        let insts = vec![Inst::Copy(pd, ps, n)];
        f.blocks[0].pos = vec![0; insts.len()];
        f.blocks[0].insts = insts;
        f.blocks[0].term = Term::Ret(vec![]);
        f.verify().unwrap();
        let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        let c = relax_backend::x64::compile(&options(), &f, &env).unwrap();
        let code = Code::new(&c.code).unwrap();
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        let src: Vec<u8> = (0..n + 16).map(|i| (i * 7 + 3) as u8).collect();
        let mut dst = vec![0xeeu8; n as usize + 16];
        run(src.as_ptr(), dst.as_mut_ptr());
        assert_eq!(&dst[..n as usize], &src[..n as usize], "copy {}", n);
        assert!(dst[n as usize..].iter().all(|b| *b == 0xee), "copy {} wrote past its end", n);
    }
}
