//! Saturating float -> int casts (`Conv::FToInt`, Rust `as`): NaN -> 0, out of range clamps, the
//! integer kept extended to 64 bits by its type. The x64 targets whose bounds the float type holds
//! exactly (f32 -> 8/16 bits, f64 -> 8/16 bits and u32) clamp with maxss / minss and convert once;
//! every target must give the bits of Rust's cast for NaNs, infinities, zeros, the bounds and their
//! neighbours, in the AVX and the SSE forms.
#![cfg(target_arch = "x86_64")]
use relax_backend::{exec::Code, rir::*, x64};

fn build(from64: bool, it: IntTy, no_avx: bool, opt_regs: bool) -> Code {
    let cls = if from64 { Cls::F64 } else { Cls::F32 };
    let mut f = Func::new("ftoint".into(), 0);
    let (a, d) = (f.vreg(cls), f.vreg(Cls::I));
    f.params = vec![a];
    f.rets = vec![Cls::I];
    f.block();
    f.blocks[0].insts.push(Inst::Conv(Conv::FToInt(from64, it), d, a));
    f.blocks[0].pos.push(0);
    f.blocks[0].term = Term::Ret(vec![d]);
    f.verify().unwrap();
    let mut options = relax_session::Options::default();
    options.no_avx = no_avx;
    options.cpu.avx = !no_avx && std::arch::is_x86_feature_detected!("avx");
    let env = x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs, syms: 0, sym_class: 0, near: false };
    let c = x64::compile(&options, &f, &env).unwrap();
    Code::new(&c.code).unwrap()
}

fn reference(x: f64, it: IntTy) -> u64 {
    match (it.signed, it.bits) {
        (false, 8) => (x as u8) as u64,
        (true, 8) => (x as i8) as i64 as u64,
        (false, 16) => (x as u16) as u64,
        (true, 16) => (x as i16) as i64 as u64,
        (false, 32) => (x as u32) as u64,
        (true, 32) => (x as i32) as i64 as u64,
        (false, 64) => x as u64,
        _ => (x as i64) as u64,
    }
}

#[test]
fn casts_match_rust_as() {
    let mut xs: Vec<f64> = vec![0.0, -0.0, 0.4, 0.5, 0.99, 1.0, -0.5, -0.99, -1.0, -1.5, 2.5, 1e30, -1e30, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN, f64::MAX, f64::MIN_POSITIVE];
    for b in [8, 16, 32, 64] {
        for e in [(2f64).powi(b - 1), (2f64).powi(b)] {
            for d in [-1.5, -1.0, -0.5, 0.0, 0.5, 1.0] {
                xs.push(e + d);
                xs.push(-e + d);
            }
        }
    }
    for (from64, no_avx, opt_regs) in [(false, false, true), (true, false, true), (false, true, true), (true, true, true), (false, false, false), (true, false, false)] {
        for bits in [8u8, 16, 32, 64] {
            for signed in [false, true] {
                let it = IntTy { bits, signed };
                let code = build(from64, it, no_avx, opt_regs);
                for &x in &xs {
                    let (got, want) = if from64 {
                        let g: extern "sysv64" fn(f64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                        (g(x), reference(x, it))
                    } else {
                        let g: extern "sysv64" fn(f32) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                        let x = x as f32;
                        (g(x), reference(x as f64, it))
                    };
                    assert_eq!(got, want, "{} -> {}{} (no_avx {}): x = {:e}", if from64 { "f64" } else { "f32" }, if signed { "i" } else { "u" }, bits, no_avx, x);
                }
            }
        }
    }
}
