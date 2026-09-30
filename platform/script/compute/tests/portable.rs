//! `math: portable` and f64: kernel math bit-compared with
//! `makepad_csg_math::portable` (the Rust generators' math), natively and
//! interpreted, on special values, edge cases and random bit patterns.

use makepad_csg_math::portable as p;
use makepad_script_compute::kernel::{compile_with, CANONICAL_NAN};
use makepad_script_compute::Backend;

/// A tiny deterministic generator.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn f32_inputs() -> Vec<f32> {
    let mut v = vec![
        0.0, -0.0, 1.0, -1.0, 0.5, -0.5, 2.0, 1e-30, -1e-30, 1e-45, f32::MIN_POSITIVE, 3.0e38, -3.0e38, f32::INFINITY, f32::NEG_INFINITY,
        f32::NAN, 0.785_398_2, 1.570_796_4, 3.141_592_7, 4.712_389, 6.283_185_5, 100.0, 1e6, 12345.678, 0.4375, 0.6875, 1.1875, 2.4375, 88.0,
        -87.0, 700.0, 64.0, 63.0, 0.999_999_9, 1.000_000_1, 0.25, 7.0, -7.0, 1.5, -2.5,
    ];
    let mut r = Rng(0x1234_5678_9ABC_DEF1);
    for _ in 0..3000 {
        let bits = r.next() as u32;
        v.push(f32::from_bits(bits));
        // Modelling-sized values too.
        v.push(((r.next() % 2_000_000) as f32 - 1_000_000.0) * 0.001);
    }
    v
}

/// NaN in any form counts as the canonical NaN the kernel stores.
fn canon(x: f32) -> u32 {
    if x.is_nan() {
        CANONICAL_NAN
    } else {
        x.to_bits()
    }
}

fn run1(src: &str, input: &[f32], backend: Backend, interp: bool) -> Vec<u32> {
    let k = compile_with(src, &[], backend).unwrap_or_else(|e| panic!("{:?}\n{}", e, src));
    let mut out = vec![0.0f32; input.len()];
    let mut c = k.call();
    c.input("x", input).unwrap();
    c.output("o", &mut out).unwrap();
    if interp { c.run_interp(input.len()).unwrap() } else { c.run(input.len()).unwrap() };
    out.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn portable_f32_unary_math_is_bit_exact_with_csg_math() {
    use p::PortableFloat;
    let xs = f32_inputs();
    let cases: &[(&str, fn(f32) -> f32)] = &[
        ("sin", |x| x.psin()),
        ("cos", |x| x.pcos()),
        ("tan", |x| x.ptan()),
        ("atan", |x| x.patan()),
        ("asin", |x| x.pasin()),
        ("acos", |x| x.pacos()),
        ("exp", |x| x.pexp()),
        ("ln", |x| x.pln()),
        ("cbrt", |x| x.pcbrt()),
    ];
    for (name, f) in cases {
        let src = format!("let math = portable\nlet x = input(f32)\nlet o = output(f32)\nfn element(i) {{ o[i] = {}(x[i]) }}", name);
        let want: Vec<u32> = xs.iter().map(|x| canon(f(*x))).collect();
        for (backend, interp) in [(Backend::Native, false), (Backend::Interp, true)] {
            let got = run1(&src, &xs, backend, interp);
            for (k, (g, w)) in got.iter().zip(&want).enumerate() {
                assert_eq!(g, w, "{}({:e} = {:#x}) {:?}: kernel {:#x}, csg_math {:#x}", name, xs[k], xs[k].to_bits(), backend, g, w);
            }
        }
    }
}

#[test]
fn portable_f32_binary_math_is_bit_exact_with_csg_math() {
    use p::PortableFloat;
    let xs = f32_inputs();
    let mut r = Rng(0xDEAD_BEEF_0BAD_F00D);
    // Pairs: every input against a shuffled partner and a few exponents.
    let ys: Vec<f32> = xs
        .iter()
        .enumerate()
        .map(|(k, _)| match k % 5 {
            0 => xs[(r.next() as usize) % xs.len()],
            1 => ((r.next() % 129) as f32) - 64.0,
            2 => 0.5,
            3 => -((r.next() % 10) as f32),
            _ => f32::from_bits(r.next() as u32),
        })
        .collect();
    let mut both = xs.clone();
    both.extend_from_slice(&ys);
    let n = xs.len();
    let cases: &[(&str, fn(f32, f32) -> f32)] = &[("pow", |x, y| x.ppowf(y)), ("atan2", |y, x| y.patan2(x)), ("hypot", |x, y| x.phypot(y))];
    for (name, f) in cases {
        let src = format!("let math = portable\nlet x = input(f32)\nlet o = output(f32)\nfn element(i) {{ o[i] = {}(x[i], x[i + {}]) }}", name, n);
        let want: Vec<u32> = (0..n).map(|k| canon(f(xs[k], ys[k]))).collect();
        for (backend, interp) in [(Backend::Native, false), (Backend::Interp, true)] {
            let k = compile_with(&src, &[], backend).unwrap_or_else(|e| panic!("{:?}", e));
            let mut out = vec![0.0f32; n];
            let mut c = k.call();
            c.input("x", &both).unwrap();
            c.output("o", &mut out).unwrap();
            if interp { c.run_interp(n).unwrap() } else { c.run(n).unwrap() };
            for i in 0..n {
                assert_eq!(out[i].to_bits(), want[i], "{}({:e}, {:e}) {:?}", name, xs[i], ys[i], backend);
            }
        }
    }
}

#[test]
fn f64_kernels_match_csg_math_f64_and_store_two_words() {
    let mut r = Rng(0x0F0F_1234_5555_AAAA);
    let mut xs: Vec<f64> = vec![0.0, -0.0, 1.0, -1.0, 0.5, 1e-300, 5e-324, 1e300, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 709.0, -745.0, 1e6, 3.0];
    for _ in 0..2000 {
        xs.push(f64::from_bits(r.next()));
        xs.push(((r.next() % 20_000_000) as f64 - 10_000_000.0) * 1e-4);
    }
    let words: Vec<u32> = xs.iter().flat_map(|x| [x.to_bits() as u32, (x.to_bits() >> 32) as u32]).collect();
    let cases: &[(&str, fn(f64) -> f64)] = &[
        ("sin", p::sin),
        ("cos", p::cos),
        ("tan", p::tan),
        ("atan", p::atan),
        ("asin", p::asin),
        ("acos", p::acos),
        ("exp", p::exp),
        ("ln", p::ln),
        ("cbrt", p::cbrt),
        ("sqrt", f64::sqrt),
        ("floor", f64::floor),
        ("round", f64::round),
    ];
    for (name, f) in cases {
        // Plain f64 math (no pragma): f64 arguments always take the
        // portable kernels.
        let src = format!("let x = input(f64)\nlet o = output(f64)\nfn element(i) {{ o[i] = {}(x[i]) }}", name);
        for backend in [Backend::Native, Backend::Interp] {
            let k = compile_with(&src, &[], backend).unwrap_or_else(|e| panic!("{:?}\n{}", e, src));
            let mut out = vec![0u32; words.len()];
            let mut c = k.call();
            c.input_u32("x", &words).unwrap();
            c.output_u32("o", &mut out).unwrap();
            if backend == Backend::Interp { c.run_interp(xs.len()).unwrap() } else { c.run(xs.len()).unwrap() };
            for (k, x) in xs.iter().enumerate() {
                let got = f64::from_bits(out[2 * k] as u64 | (out[2 * k + 1] as u64) << 32);
                let want = f(*x);
                let same = got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan());
                assert!(same, "{}({:e}) {:?}: kernel {:e} ({:#x}), csg_math {:e} ({:#x})", name, x, backend, got, got.to_bits(), want, want.to_bits());
            }
        }
    }
    // Arithmetic, conversions and mixed types: large-world coordinates.
    let src = "let x = input(f64)\nlet o = output(f64)\nlet s = output(f32)\nfn element(i) { let a = x[i]\n let b = a * 3.0 + f64(0.1) - float(i)\n o[i] = b % 7.0\n s[i] = a - floor(a) }";
    let k = compile_with(src, &[], Backend::Native).unwrap();
    let ki = compile_with(src, &[], Backend::Interp).unwrap();
    let mut outs = Vec::new();
    for k in [&k, &ki] {
        let mut o = vec![0u32; words.len()];
        let mut s = vec![0.0f32; xs.len()];
        let mut c = k.call();
        c.input_u32("x", &words).unwrap();
        c.output_u32("o", &mut o).unwrap();
        c.output("s", &mut s).unwrap();
        c.run(xs.len()).unwrap();
        outs.push((o, s.iter().map(|x| x.to_bits()).collect::<Vec<_>>()));
    }
    assert_eq!(outs[0], outs[1], "native and interpreter differ on f64 arithmetic");
    for (k, x) in xs.iter().enumerate().take(200) {
        let b = x * 3.0 + 0.1 - k as f32 as f64;
        let want = b - 7.0 * (b / 7.0).trunc();
        let got = f64::from_bits(outs[0].0[2 * k] as u64 | (outs[0].0[2 * k + 1] as u64) << 32);
        assert!(got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()), "{}: {} vs {}", x, got, want);
    }
}

#[test]
fn portable_mode_stores_one_nan() {
    let src = "let math = portable\nlet x = input(f32)\nlet o = output(f32)\nfn element(i) { o[i] = x[i] * 1.0 }";
    let xs = [f32::from_bits(0x7FC0_1234), f32::from_bits(0xFFC0_0001), f32::from_bits(0x7F80_0001), 1.0];
    for backend in [Backend::Native, Backend::Interp] {
        let got = run1(src, &xs, backend, backend == Backend::Interp);
        assert_eq!(got, vec![CANONICAL_NAN, CANONICAL_NAN, CANONICAL_NAN, 1.0f32.to_bits()]);
    }
    // Fast mode keeps the hardware's NaN (bit-identical across backends).
    let fast = "let x = input(f32)\nlet o = output(f32)\nfn element(i) { o[i] = x[i] * 1.0 }";
    let a = run1(fast, &xs, Backend::Native, false);
    let b = run1(fast, &xs, Backend::Interp, true);
    assert_eq!(a, b);
}
