// Float Display, Debug and exponent formatting, including shortest and exact decimals.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::fmt as sf;

struct D(u64, u32);
impl D {
    fn new() -> D {
        D(0xcbf29ce484222325, 0)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn text(&mut self, s: &str) {
        self.bytes(&(s.len() as u64).to_le_bytes());
        self.bytes(s.as_bytes());
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const NCASES: usize = 23;

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
fn a01_float_edge_cases() {
    let wps = [(0usize, 0usize), (1, 1), (8, 3), (12, 0), (25, 17), (30, 25), (5, 60)];
    for i in 0..NCASES {
        let mut d = D::new();
        for v in edge64() {
            for (w, p) in wps {
                d.text(&real_case(i, v, w, p));
                d.1 += 1;
            }
        }
        d.line(&format!("float edges f64 case {}", i));
        let mut d = D::new();
        for v in edge32() {
            for (w, p) in wps {
                d.text(&real_case(i, v, w, p));
                d.1 += 1;
            }
        }
        d.line(&format!("float edges f32 case {}", i));
    }
}

#[test]
fn a02_float_large_precision() {
    let mut d = D::new();
    let vals = [5e-324, 2.2250738585072014e-308, 1e-300, 0.1, 1.0 / 3.0, 1e300, f64::MAX, 123.456];
    for v in vals {
        for p in [50usize, 100, 340, 767, 800, 1100, 1500] {
            for i in [1usize, 12, 16] {
                d.text(&real_case(i, v, 0, p));
                d.1 += 1;
            }
        }
    }
    d.line("float large precision");
}

#[test]
fn a03_float_param_counts() {
    let mut d = D::new();
    for v in [0.1f64, -3.75, 1e20, 5e-324] {
        for w in [0usize, 7, 30] {
            for p in [0usize, 2, 9] {
                d.text(&format!("<{:w$.p$}>", v, w = w, p = p));
                d.1 += 1;
            }
        }
    }
    d.line("float parameter counts");
}

#[test]
fn a04_float_random_f64() {
    let mut r = Rng(0x9E3779B97F4A7C15);
    let mut d = D::new();
    for _ in 0..3_000 {
        let v = random_f64(&mut r);
        let i = r.below(NCASES as u64) as usize;
        let w = r.below(40) as usize;
        let p = if r.below(10) == 0 { r.below(120) as usize } else { r.below(20) as usize };
        d.text(&real_case(i, v, w, p));
        d.1 += 1;
    }
    d.line("float random f64");
    let mut d = D::new();
    for _ in 0..3_000 {
        let v = random_f64(&mut r);
        for i in [0usize, 11, 14] {
            d.text(&real_case(i, v, 0, 0));
            d.1 += 1;
        }
    }
    d.line("float shortest f64");
}

#[test]
fn a05_float_random_f32() {
    let mut r = Rng(0xD1B54A32D192ED03);
    let mut d = D::new();
    for _ in 0..3_000 {
        let v = random_f32(&mut r);
        let i = r.below(NCASES as u64) as usize;
        let w = r.below(40) as usize;
        let p = r.below(30) as usize;
        d.text(&real_case(i, v, w, p));
        d.text(&real_case(0, v, 0, 0));
        d.1 += 2;
    }
    d.line("float random f32");
}

fn shortest_exact<T: Copy + sf::Display + sf::Debug + sf::LowerExp + sf::UpperExp>(v: T, d: &mut D) {
    for i in [0, 11, 14, 15, 7, 3] {
        d.text(&real_case(i, v, 10, 3));
        d.1 += 1;
    }
    for p in (0..=17).chain(std::iter::once(40)) {
        d.text(&format!("{:.p$}", v));
        d.text(&format!("{:.p$e}", v));
        d.1 += 2;
    }
}

#[test]
fn a06_grisu_f64_corpus() {
    let mut d = D::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for _ in 0..3_000 {
        shortest_exact(f64::from_bits(rng.next() as _), &mut d);
    }
    for v in [
        0.0, -0.0, f64::from_bits(1), f64::from_bits((1 << 52) - 1),
        f64::MIN_POSITIVE, f64::MAX, f64::INFINITY, f64::NEG_INFINITY,
        f64::NAN, 0.1 + 0.2, 0.5, 1.5, 2.5, 9.5, 99.5,
    ] {
        shortest_exact(v, &mut d);
        shortest_exact(-v, &mut d);
    }
    for e in -323..=308 {
        let v = 10f64.powi(e);
        shortest_exact(v, &mut d);
        shortest_exact(f64::from_bits(v.to_bits().saturating_sub(1)), &mut d);
        shortest_exact(f64::from_bits(v.to_bits() + 1), &mut d);
    }
    d.line("grisu_f64_corpus");
}

#[test]
fn a07_grisu_f32_corpus() {
    let mut d = D::new();
    let mut rng = Rng(0xd1b54a32d192ed03);
    for _ in 0..3_000 {
        shortest_exact(f32::from_bits(rng.next() as _), &mut d);
    }
    for v in [
        0.0, -0.0, f32::from_bits(1), f32::from_bits((1 << 23) - 1),
        f32::MIN_POSITIVE, f32::MAX, f32::INFINITY, f32::NEG_INFINITY,
        f32::NAN, 0.1 + 0.2, 0.5, 1.5, 2.5, 9.5, 99.5,
    ] {
        shortest_exact(v, &mut d);
        shortest_exact(-v, &mut d);
    }
    for e in -45..=38 {
        let v = 10f32.powi(e);
        shortest_exact(v, &mut d);
        shortest_exact(f32::from_bits(v.to_bits().saturating_sub(1)), &mut d);
        shortest_exact(f32::from_bits(v.to_bits() + 1), &mut d);
    }
    d.line("grisu_f32_corpus");
}
