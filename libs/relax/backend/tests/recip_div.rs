//! `x / c` for a constant c = +-2^k whose reciprocal is exactly representable (normal or
//! subnormal) is `x * (1 / c)`: bit-equal for every x (NaN, inf, zeros, subnormal and
//! overflowing / underflowing results), and no divide is left. Other constants keep the divide.
#![cfg(target_arch = "x86_64")]
use relax_backend::{exec::Code, rir::*, x64};
use relax_backend::passes;

fn build(w: bool, c: u64) -> (Func, bool) {
    let cls = if w { Cls::F64 } else { Cls::F32 };
    let mut f = Func::new("recip_div".into(), 0);
    let x = f.vreg(cls);
    f.params = vec![x];
    f.rets = vec![cls];
    f.block();
    let k = f.vreg(cls);
    let d = f.vreg(cls);
    for i in [Inst::Fconst(k, c, w), Inst::FBin(FOp::Div, w, d, x, k)] {
        f.blocks[0].insts.push(i);
        f.blocks[0].pos.push(0);
    }
    f.blocks[0].term = Term::Ret(vec![d]);
    f.verify().unwrap();
    let options = relax_session::Options::default();
    passes::simplify(&options, &mut f, None, true, true);
    let divides = f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::FBin(FOp::Div, ..))));
    (f, divides)
}

fn run(f: &Func) -> Code {
    let env = x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = x64::compile(&relax_session::Options::default(), f, &env).unwrap();
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

#[test]
fn f32_power_of_two_divisors() {
    // (constant, folds): 2^-149 has no representable reciprocal; 2^127 -> 2^-127 is subnormal
    let cases: [(f32, bool); 20] = [
        (2.0, true),
        (-2.0, true),
        (0.5, true),
        (-0.5, true),
        (1.0, true),
        (4.0, true),
        (1024.0, true),
        (f32::from_bits(0x7f000000), true),  // 2^127
        (f32::from_bits(0x00800000), true),  // 2^-126
        (f32::from_bits(0x00400000), true),  // 2^-127 (subnormal): reciprocal 2^127
        (f32::from_bits(0x00200000), false), // 2^-128: reciprocal 2^128 overflows
        (f32::from_bits(0x00000001), false), // 2^-149
        (f32::from_bits(0x80000001), false),
        (3.0, false),
        (0.1, false),
        (0.0, false),
        (-0.0, false),
        (f32::INFINITY, false),
        (f32::NAN, false),
        (f32::from_bits(0x00600000), false), // 1.5 * 2^-126: not a power of two
    ];
    let mut s = 0x9e3779b97f4a7c15u64;
    for (c, folds) in cases {
        let (f, divides) = build(false, c.to_bits() as u64);
        assert_eq!(!divides, folds, "f32 {c:e}");
        let code = run(&f);
        let go: extern "sysv64" fn(f32) -> f32 = unsafe { std::mem::transmute(code.ptr()) };
        let mut xs: Vec<f32> = vec![0.0, -0.0, 1.0, -1.0, 3.0, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN, f32::MIN_POSITIVE, f32::from_bits(1), f32::from_bits(3), f32::from_bits(0x007fffff), f32::from_bits(0x00800001), f32::from_bits(0x7f7fffff)];
        for _ in 0..4000 {
            xs.push(f32::from_bits(next(&mut s) as u32));
        }
        for x in xs {
            let want = x / c;
            let got = go(x);
            assert!(got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()), "f32 {x:e} / {c:e}: {got:e} vs {want:e}");
        }
    }
}

#[test]
fn f64_power_of_two_divisors() {
    let cases: [(f64, bool); 16] = [
        (2.0, true),
        (-2.0, true),
        (0.5, true),
        (8.0, true),
        (f64::from_bits(0x7fe0000000000000), true),  // 2^1023 -> 2^-1023 (subnormal)
        (f64::from_bits(0x0010000000000000), true),  // 2^-1022
        (f64::from_bits(0x0008000000000000), true),  // 2^-1023 (subnormal): reciprocal 2^1023
        (f64::from_bits(0x0004000000000000), false), // 2^-1024: reciprocal overflows
        (f64::from_bits(1), false),
        (f64::from_bits(0x8000000000000001), false),
        (3.0, false),
        (0.1, false),
        (0.0, false),
        (f64::INFINITY, false),
        (f64::NAN, false),
        (f64::from_bits(0x000c000000000000), false),
    ];
    let mut s = 0x2545f4914f6cdd1du64;
    for (c, folds) in cases {
        let (f, divides) = build(true, c.to_bits());
        assert_eq!(!divides, folds, "f64 {c:e}");
        let code = run(&f);
        let go: extern "sysv64" fn(f64) -> f64 = unsafe { std::mem::transmute(code.ptr()) };
        let mut xs: Vec<f64> = vec![0.0, -0.0, 1.0, -1.0, 3.0, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, f64::MIN, f64::MIN_POSITIVE, f64::from_bits(1), f64::from_bits(3), f64::from_bits(0x000fffffffffffff), f64::from_bits(0x0010000000000001)];
        for _ in 0..4000 {
            xs.push(f64::from_bits(next(&mut s)));
        }
        for x in xs {
            let want = x / c;
            let got = go(x);
            assert!(got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()), "f64 {x:e} / {c:e}: {got:e} vs {want:e}");
        }
    }
}
