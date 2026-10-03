//! Float Display/Debug/LowerExp/UpperExp: HotRust's fmt vs real format!, on edge cases and
//! large randomized corpora (f64 and f32).

mod fmt_common;

use fmt_common::*;
use hotrust_std_check::fmt as hf;
use hotrust_std_check::fmt::rt;
use std::fmt as sf;

const P: u32 = rt::FLAG_PLUS;
const Z: u32 = rt::FLAG_ZERO_PAD;
const A: u32 = rt::FLAG_ALTERNATE;
const L: u8 = rt::ALIGN_LEFT;
const R: u8 = rt::ALIGN_RIGHT;
const C: u8 = rt::ALIGN_CENTER;
const U: u8 = rt::ALIGN_UNKNOWN;

#[derive(Clone, Copy)]
enum Tr {
    Display,
    Debug,
    LowerExp,
    UpperExp,
}

const NCASES: usize = 23;

/// (trait, spec) of case `i` for width `w` and precision `p`; real_case renders the same.
fn case(i: usize, w: usize, p: usize) -> (Tr, Spec) {
    match i {
        0 => (Tr::Display, DEF),
        1 => (Tr::Display, spec(' ', U, 0, None, Some(p))),
        2 => (Tr::Display, spec(' ', U, 0, Some(w), None)),
        3 => (Tr::Display, spec(' ', U, 0, Some(w), Some(p))),
        4 => (Tr::Display, spec(' ', L, 0, Some(w), None)),
        5 => (Tr::Display, spec(' ', C, 0, Some(w), Some(p))),
        6 => (Tr::Display, spec('*', R, 0, Some(w), None)),
        7 => (Tr::Display, spec(' ', U, P, None, None)),
        8 => (Tr::Display, spec(' ', U, P, None, Some(p))),
        9 => (Tr::Display, spec(' ', U, Z, Some(w), None)),
        10 => (Tr::Display, spec(' ', U, P | Z, Some(w), Some(p))),
        11 => (Tr::Debug, DEF),
        12 => (Tr::Debug, spec(' ', U, 0, None, Some(p))),
        13 => (Tr::Debug, spec(' ', U, A, None, None)),
        14 => (Tr::LowerExp, DEF),
        15 => (Tr::UpperExp, DEF),
        16 => (Tr::LowerExp, spec(' ', U, 0, None, Some(p))),
        17 => (Tr::UpperExp, spec(' ', U, P, Some(w), Some(p))),
        18 => (Tr::LowerExp, spec(' ', U, Z, Some(w), None)),
        19 => (Tr::Debug, spec(' ', U, P, None, None)),
        20 => (Tr::Debug, spec(' ', U, Z, Some(10), None)),
        21 => (Tr::Debug, spec('x', L, 0, Some(w), None)),
        _ => (Tr::Display, spec('_', C, Z, Some(w), None)),
    }
}

fn real_case<T: sf::Display + sf::Debug + sf::LowerExp + sf::UpperExp>(i: usize, v: T, w: usize, p: usize) -> String {
    match i {
        0 => format!("{}", v),
        1 => format!("{:.p$}", v, p = p),
        2 => format!("{:w$}", v, w = w),
        3 => format!("{:w$.p$}", v, w = w, p = p),
        4 => format!("{:<w$}", v, w = w),
        5 => format!("{:^w$.p$}", v, w = w, p = p),
        6 => format!("{:*>w$}", v, w = w),
        7 => format!("{:+}", v),
        8 => format!("{:+.p$}", v, p = p),
        9 => format!("{:0w$}", v, w = w),
        10 => format!("{:+0w$.p$}", v, w = w, p = p),
        11 => format!("{:?}", v),
        12 => format!("{:.p$?}", v, p = p),
        13 => format!("{:#?}", v),
        14 => format!("{:e}", v),
        15 => format!("{:E}", v),
        16 => format!("{:.p$e}", v, p = p),
        17 => format!("{:+w$.p$E}", v, w = w, p = p),
        18 => format!("{:0w$e}", v, w = w),
        19 => format!("{:+?}", v),
        20 => format!("{:010?}", v),
        21 => format!("{:x<w$?}", v, w = w),
        _ => format!("{:_^0w$}", v, w = w),
    }
}

fn mine64(v: f64, i: usize, w: usize, p: usize) -> String {
    let (tr, s) = case(i, w, p);
    match tr {
        Tr::Display => render(&v, <f64 as hf::Display>::fmt, &s),
        Tr::Debug => render(&v, <f64 as hf::Debug>::fmt, &s),
        Tr::LowerExp => render(&v, <f64 as hf::LowerExp>::fmt, &s),
        Tr::UpperExp => render(&v, <f64 as hf::UpperExp>::fmt, &s),
    }
}

fn mine32(v: f32, i: usize, w: usize, p: usize) -> String {
    let (tr, s) = case(i, w, p);
    match tr {
        Tr::Display => render(&v, <f32 as hf::Display>::fmt, &s),
        Tr::Debug => render(&v, <f32 as hf::Debug>::fmt, &s),
        Tr::LowerExp => render(&v, <f32 as hf::LowerExp>::fmt, &s),
        Tr::UpperExp => render(&v, <f32 as hf::UpperExp>::fmt, &s),
    }
}

struct Fails {
    n: usize,
    shown: usize,
}

impl Fails {
    fn check(&mut self, what: &str, got: String, want: String) {
        if got != want {
            self.n += 1;
            if self.shown < 25 {
                self.shown += 1;
                eprintln!("MISMATCH {}: got {:?} want {:?}", what, got, want);
            }
        }
    }
}

fn edge64() -> Vec<f64> {
    let mut v = vec![
        0.0, -0.0, 1.0, -1.0, 0.1, 0.2, 0.3, 0.5, 1.5, 2.5, -2.5, 0.25, 0.125, 0.05, 0.15, 0.35, 0.45, 9.5, 99.5,
        999.9999, 1e-4, 9.9999e-5, 1e-5, 1e16, 9.999999999999998e15, 1e15, 1e17, 1e21, 1e22, 1e23, 1e100, 1e300,
        1e308, 1.7976931348623157e308, 2.2250738585072014e-308, 2.225073858507201e-308, 5e-324, 1e-323, 4.9e-324,
        f64::EPSILON, f64::MIN_POSITIVE, f64::MAX, f64::MIN, f64::NAN, -f64::NAN, f64::INFINITY, f64::NEG_INFINITY,
        123456789.0, 0.1 + 0.2, 1.0 / 3.0, 2.0 / 3.0, 100.0, 1234.5, 0.000123, 12345678901234567890.0,
        9007199254740992.0, 9007199254740993.0, 4503599627370496.5, 0.30000000000000004, 3.141592653589793,
        2.718281828459045, 6.02214076e23, 1.602176634e-19, 299792458.0, 0.999999999999, 0.9999999999999999,
    ];
    let mut x = 1.0f64;
    for _ in 0..64 {
        v.push(x);
        v.push(1.0 / x);
        x *= 10.0;
    }
    let mut e = -1074i32;
    while e <= 1023 {
        v.push(2f64.powi(e));
        e += 7;
    }
    v
}

fn edge32() -> Vec<f32> {
    let mut v = vec![
        0.0, -0.0, 1.0, -1.0, 0.1, 0.2, 0.3, 0.5, 1.5, 2.5, 0.25, 0.05, 0.15, 9.5, 1e-4, 9.9999e-5, 1e16, 1e15,
        1e17, 1e38, 3.4028235e38, 1.1754944e-38, 1e-45, 1.4e-45, f32::EPSILON, f32::MIN_POSITIVE, f32::MAX, f32::MIN,
        f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 16777216.0, 16777217.0, 3.1415927, 0.33333334, 123456.79,
    ];
    let mut e = -149i32;
    while e <= 127 {
        v.push(2f32.powi(e));
        e += 3;
    }
    v
}

#[test]
fn float_edge_cases() {
    let mut fails = Fails { n: 0, shown: 0 };
    let mut total = 0;
    let wps = [(0usize, 0usize), (1, 1), (8, 3), (12, 0), (25, 17), (30, 25), (5, 60)];
    for &v in &edge64() {
        for i in 0..NCASES {
            for &(w, p) in &wps {
                total += 1;
                fails.check(&format!("f64 {:e} case {} w{} p{}", v, i, w, p), mine64(v, i, w, p), real_case(i, v, w, p));
            }
        }
    }
    for &v in &edge32() {
        for i in 0..NCASES {
            for &(w, p) in &wps {
                total += 1;
                fails.check(&format!("f32 {:e} case {} w{} p{}", v, i, w, p), mine32(v, i, w, p), real_case(i, v, w, p));
            }
        }
    }
    eprintln!("float_edge_cases: {} comparisons, {} mismatches", total, fails.n);
    assert_eq!(fails.n, 0);
}

#[test]
fn float_large_precision() {
    let mut fails = Fails { n: 0, shown: 0 };
    let vals = [5e-324, 2.2250738585072014e-308, 1e-300, 0.1, 1.0 / 3.0, 1e300, f64::MAX, 123.456];
    for &v in &vals {
        for &p in &[50usize, 100, 340, 767, 800, 1100, 1500] {
            for i in [1usize, 12, 16] {
                fails.check(&format!("f64 {:e} case {} p{}", v, i, p), mine64(v, i, 0, p), real_case(i, v, 0, p));
            }
        }
    }
    assert_eq!(fails.n, 0);
}

#[test]
fn float_param_counts() {
    let mut fails = Fails { n: 0, shown: 0 };
    for &v in &[0.1f64, -3.75, 1e20, 5e-324] {
        for w in [0usize, 7, 30] {
            for p in [0usize, 2, 9] {
                let got = render_param(&v, <f64 as hf::Display>::fmt, w, p);
                let want = format!("<{:w$.p$}>", v, w = w, p = p);
                fails.check("param", got, want);
            }
        }
    }
    assert_eq!(fails.n, 0);
}

fn random_f64(r: &mut Rng) -> f64 {
    match r.below(6) {
        0 => f64::from_bits(r.next()),
        1 => f64::from_bits(r.next() & 0x000f_ffff_ffff_ffff), // subnormal
        2 => (r.below(2_000_000) as f64) / 10f64.powi(r.below(12) as i32), // short decimals
        3 => (r.below(1 << 53) as f64) * 2f64.powi(r.below(200) as i32 - 100),
        4 => {
            // near powers of ten
            let p = 10f64.powi(r.below(600) as i32 - 300);
            f64::from_bits(p.to_bits().wrapping_add(r.below(5)).wrapping_sub(2))
        }
        _ => (r.next() as f64) * if r.below(2) == 0 { 1.0 } else { -1e-9 },
    }
}

fn random_f32(r: &mut Rng) -> f32 {
    match r.below(4) {
        0 => f32::from_bits(r.next() as u32),
        1 => f32::from_bits((r.next() as u32) & 0x007f_ffff),
        2 => (r.below(200_000) as f32) / 10f32.powi(r.below(8) as i32),
        _ => (r.below(1 << 24) as f32) * 2f32.powi(r.below(80) as i32 - 40),
    }
}

#[test]
fn float_random_f64() {
    let mut r = Rng(0x9E3779B97F4A7C15);
    let mut fails = Fails { n: 0, shown: 0 };
    let mut total = 0;
    for _ in 0..300_000 {
        let v = random_f64(&mut r);
        let i = r.below(NCASES as u64) as usize;
        let w = r.below(40) as usize;
        let p = if r.below(10) == 0 { r.below(120) as usize } else { r.below(20) as usize };
        total += 1;
        fails.check(&format!("f64 bits {:#x} case {} w{} p{}", v.to_bits(), i, w, p), mine64(v, i, w, p), real_case(i, v, w, p));
    }
    // shortest Display/Debug/exp on every value of a second stream
    for _ in 0..300_000 {
        let v = random_f64(&mut r);
        for i in [0usize, 11, 14] {
            total += 1;
            fails.check(&format!("f64 bits {:#x} case {}", v.to_bits(), i), mine64(v, i, 0, 0), real_case(i, v, 0, 0));
        }
    }
    eprintln!("float_random_f64: {} comparisons, {} mismatches", total, fails.n);
    assert_eq!(fails.n, 0);
}

#[test]
fn float_random_f32() {
    let mut r = Rng(0xD1B54A32D192ED03);
    let mut fails = Fails { n: 0, shown: 0 };
    let mut total = 0;
    for _ in 0..300_000 {
        let v = random_f32(&mut r);
        let i = r.below(NCASES as u64) as usize;
        let w = r.below(40) as usize;
        let p = r.below(30) as usize;
        total += 1;
        fails.check(&format!("f32 bits {:#x} case {} w{} p{}", v.to_bits(), i, w, p), mine32(v, i, w, p), real_case(i, v, w, p));
        total += 1;
        fails.check(&format!("f32 bits {:#x} case 0", v.to_bits()), mine32(v, 0, 0, 0), real_case(0, v, 0, 0));
    }
    eprintln!("float_random_f32: {} comparisons, {} mismatches", total, fails.n);
    assert_eq!(fails.n, 0);
}
