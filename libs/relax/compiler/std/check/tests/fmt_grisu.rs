//! String-exact Grisu coverage and direct fallback accounting.
#![allow(dead_code)]
mod fmt_common;
use fmt_common::*;
use relax_std_check::fmt as hf;
use relax_std_check::fmt::rt;
pub use std::cmp;
#[path = "../../core/fmt/float/grisu.rs"]
mod grisu;
#[path = "../../core/fmt/float/grisu_table.rs"]
mod grisu_table;
#[path = "../../core/fmt/float/dragon.rs"]
mod dragon;
#[path = "../../core/fmt/float/bignum.rs"]
mod bignum;

pub struct Decoded {
    pub mant: u64, pub minus: u64, pub plus: u64, pub exp: i16, pub inclusive: bool,
}

// IEEE decoder for directly testing the digit strategies, independently of fmt dispatch.
fn decoded(bits: u64, fraction: u32, bias: i16, max_exp: u64) -> Option<Decoded> {
    let frac = bits & ((1u64 << fraction) - 1);
    let ef = (bits >> fraction) & max_exp;
    if ef == max_exp || (ef == 0 && frac == 0) { return None; }
    let mant = if ef == 0 { frac << 1 } else { frac | (1 << fraction) };
    let exp = if ef == 0 { -bias } else { ef as i16 - bias };
    let inclusive = mant & 1 == 0;
    if ef == 0 {
        Some(Decoded { mant, minus: 1, plus: 1, exp, inclusive })
    } else if mant == 1 << fraction {
        Some(Decoded { mant: mant << 2, minus: 1, plus: 2, exp: exp - 2, inclusive })
    } else {
        Some(Decoded { mant: mant << 1, minus: 1, plus: 1, exp: exp - 1, inclusive })
    }
}

#[derive(Default)]
struct Counts { finite: usize, shortest: usize, exact: usize, requests: usize }
fn strategies(d: Option<Decoded>, counts: &mut Counts) {
    if let Some(d) = d {
        counts.finite += 1;
        let mut a = [0; 1024];
        let mut b = [0; 1024];
        if grisu::format_shortest_opt(&d, &mut a[..17]).is_none() { counts.shortest += 1; }
        let x = grisu::format_shortest(&d, &mut a[..17]);
        let y = dragon::format_shortest(&d, &mut b[..17]);
        assert_eq!(x, y);
        assert_eq!(&a[..x.0], &b[..y.0]);
        for p in (0..=17).chain(std::iter::once(40)) {
            counts.requests += 1;
            let n = dragon::estimate_max_buf_len(d.exp);
            if grisu::format_exact_opt(&d, &mut a[..n], -p).is_none() { counts.exact += 1; }
            let x = grisu::format_exact(&d, &mut a[..n], -p);
            let y = dragon::format_exact(&d, &mut b[..n], -p);
            assert_eq!(x, y);
            assert_eq!(&a[..x.0], &b[..y.0]);
        }
    }
}

fn check_f64(v: f64, counts: &mut Counts) {
    assert_eq!(render(&v, <f64 as hf::Display>::fmt, &DEF), format!("{}", v));
    assert_eq!(render(&v, <f64 as hf::Debug>::fmt, &DEF), format!("{:?}", v));
    assert_eq!(render(&v, <f64 as hf::LowerExp>::fmt, &DEF), format!("{:e}", v));
    assert_eq!(render(&v, <f64 as hf::UpperExp>::fmt, &DEF), format!("{:E}", v));
    assert_eq!(render(&v, <f64 as hf::Display>::fmt, &spec(' ', rt::ALIGN_UNKNOWN, rt::FLAG_PLUS, None, None)), format!("{:+}", v));
    assert_eq!(render(&v, <f64 as hf::Display>::fmt, &spec(' ', rt::ALIGN_UNKNOWN, 0, Some(10), Some(3))), format!("{:10.3}", v));
    for p in (0..=17).chain(std::iter::once(40)) {
        let sp = spec(' ', rt::ALIGN_UNKNOWN, 0, None, Some(p));
        assert_eq!(render(&v, <f64 as hf::Display>::fmt, &sp), format!("{:.p$}", v), "bits {:x}, precision {}", v.to_bits(), p);
        assert_eq!(render(&v, <f64 as hf::LowerExp>::fmt, &sp), format!("{:.p$e}", v));
    }
    strategies(decoded(v.to_bits() as u64, 52, 1075, 0x7ff), counts);
}

#[test]
fn grisu_f64_corpus() {
    let mut counts = Counts::default();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for _ in 0..300_000 { check_f64(f64::from_bits(rng.next() as _), &mut counts); }
    for v in [0.0, -0.0, f64::from_bits(1), f64::from_bits((1 << 52) - 1), f64::MIN_POSITIVE, f64::MAX, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 0.1 + 0.2, 0.5, 1.5, 2.5, 9.5, 99.5] {
        check_f64(v, &mut counts);
        check_f64(-v, &mut counts);
    }
    for e in -323..=308 {
        let v = 10f64.powi(e);
        check_f64(v, &mut counts);
        check_f64(f64::from_bits(v.to_bits().saturating_sub(1)), &mut counts);
        check_f64(f64::from_bits(v.to_bits() + 1), &mut counts);
    }
    eprintln!("f64: finite={}, shortest fallbacks={}, exact fallbacks={}/{}", counts.finite, counts.shortest, counts.exact, counts.requests);
    assert!(counts.shortest > 0);
    assert!(counts.exact > 0);
}

fn check_f32(v: f32, counts: &mut Counts) {
    assert_eq!(render(&v, <f32 as hf::Display>::fmt, &DEF), format!("{}", v));
    assert_eq!(render(&v, <f32 as hf::Debug>::fmt, &DEF), format!("{:?}", v));
    assert_eq!(render(&v, <f32 as hf::LowerExp>::fmt, &DEF), format!("{:e}", v));
    assert_eq!(render(&v, <f32 as hf::UpperExp>::fmt, &DEF), format!("{:E}", v));
    assert_eq!(render(&v, <f32 as hf::Display>::fmt, &spec(' ', rt::ALIGN_UNKNOWN, rt::FLAG_PLUS, None, None)), format!("{:+}", v));
    assert_eq!(render(&v, <f32 as hf::Display>::fmt, &spec(' ', rt::ALIGN_UNKNOWN, 0, Some(10), Some(3))), format!("{:10.3}", v));
    for p in (0..=17).chain(std::iter::once(40)) {
        let sp = spec(' ', rt::ALIGN_UNKNOWN, 0, None, Some(p));
        assert_eq!(render(&v, <f32 as hf::Display>::fmt, &sp), format!("{:.p$}", v), "bits {:x}, precision {}", v.to_bits(), p);
        assert_eq!(render(&v, <f32 as hf::LowerExp>::fmt, &sp), format!("{:.p$e}", v));
    }
    strategies(decoded(v.to_bits() as u64, 23, 150, 0xff), counts);
}

#[test]
fn grisu_f32_corpus() {
    let mut counts = Counts::default();
    let mut rng = Rng(0xd1b54a32d192ed03);
    for _ in 0..300_000 { check_f32(f32::from_bits(rng.next() as _), &mut counts); }
    for v in [0.0, -0.0, f32::from_bits(1), f32::from_bits((1 << 23) - 1), f32::MIN_POSITIVE, f32::MAX, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 0.1 + 0.2, 0.5, 1.5, 2.5, 9.5, 99.5] {
        check_f32(v, &mut counts);
        check_f32(-v, &mut counts);
    }
    for e in -45..=38 {
        let v = 10f32.powi(e);
        check_f32(v, &mut counts);
        check_f32(f32::from_bits(v.to_bits().saturating_sub(1)), &mut counts);
        check_f32(f32::from_bits(v.to_bits() + 1), &mut counts);
    }
    eprintln!("f32: finite={}, shortest fallbacks={}, exact fallbacks={}/{}", counts.finite, counts.shortest, counts.exact, counts.requests);
    assert!(counts.shortest > 0);
    assert!(counts.exact > 0);
}

/// Optional local timing, kept separate from correctness runs and the lane benchmark.
#[test]
#[ignore]
fn grisu_quick_timing() {
    use std::hint::black_box;
    use std::time::Instant;
    let mut rng = Rng(0x123456789abcdef);
    let mut values = Vec::new();
    for _ in 0..100_000 {
        values.push((rng.next() % 1_000_000_000) as f64 / 12345.0);
    }
    let mut real = u128::MAX;
    let mut port = u128::MAX;
    let precision = spec(' ', rt::ALIGN_UNKNOWN, 0, None, Some(6));
    for _ in 0..5 {
        let start = Instant::now();
        for &v in &values {
            black_box(format!("{}", black_box(v)));
            black_box(format!("{:.6}", black_box(v)));
            black_box(format!("{:e}", black_box(v)));
            let v = v as f32;
            black_box(format!("{}", black_box(v)));
            black_box(format!("{:.6}", black_box(v)));
            black_box(format!("{:e}", black_box(v)));
        }
        real = real.min(start.elapsed().as_nanos());
        let start = Instant::now();
        for &v in &values {
            black_box(render(black_box(&v), <f64 as hf::Display>::fmt, &DEF));
            black_box(render(black_box(&v), <f64 as hf::Display>::fmt, &precision));
            black_box(render(black_box(&v), <f64 as hf::LowerExp>::fmt, &DEF));
            let v = v as f32;
            black_box(render(black_box(&v), <f32 as hf::Display>::fmt, &DEF));
            black_box(render(black_box(&v), <f32 as hf::Display>::fmt, &precision));
            black_box(render(black_box(&v), <f32 as hf::LowerExp>::fmt, &DEF));
        }
        port = port.min(start.elapsed().as_nanos());
    }
    eprintln!("600k formats, best of 5: real={}ms port={}ms ratio={:.3}", real as f64 / 1e6, port as f64 / 1e6, port as f64 / real as f64);
}
