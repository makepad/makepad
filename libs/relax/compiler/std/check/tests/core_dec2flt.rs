//! Bit-exact differential coverage for the Clinger/Eisel-Lemire/decimal pipeline.
mod fmt_common;
use fmt_common::{render, DEF};
use relax_std_check::{dec2flt, fmt};

fn check(s: &str) {
    let want = s.parse::<f64>();
    let got = dec2flt::parse_f64(s);
    match (got, want) {
        (Ok(g), Ok(w)) => assert_eq!(g.to_bits(), w.to_bits(), "f64 {s:?}"),
        (Err(g), Err(w)) => {
            assert_eq!(format!("{g:?}"), format!("{w:?}"), "{s:?}");
            assert_eq!(render(&g, <dec2flt::ParseFloatError as fmt::Display>::fmt, &DEF), w.to_string());
        }
        (g, w) => panic!("f64 {s:?}: {g:?} != {w:?}"),
    }
    let want = s.parse::<f32>();
    let got = dec2flt::parse_f32(s);
    match (got, want) {
        (Ok(g), Ok(w)) => assert_eq!(g.to_bits(), w.to_bits(), "f32 {s:?}"),
        (Err(g), Err(w)) => {
            assert_eq!(format!("{g:?}"), format!("{w:?}"), "{s:?}");
            assert_eq!(render(&g, <dec2flt::ParseFloatError as fmt::Display>::fmt, &DEF), w.to_string());
        }
        (g, w) => panic!("f32 {s:?}: {g:?} != {w:?}"),
    }
}

#[test]
fn edges() {
    let cases = [
        "", "+", "-", ".", "1e", "1e+", "1e-", "e1", "1e2e3", "1..2", " 1", "1 ",
        "1_0", "0x1", "--1", "+-1", "nan(1)", "infinite", "∞", "１２", "1\0", "inf\0\0\0\0\0",
        "inf", "INF", "iNf", "+infinity", "-INFINITY", "nan", "NaN", "-NaN", "+NAN",
        "-0", "-0.0", "-0e99999999999", "0e-99999999999", "1e400", "-1e400", "1e-400",
        "1e999999999999999999999999", "1e-999999999999999999999999", ".5", "1.", "+.5E+1",
        "1e23", "1e37", "16777216e10", "16777217e10", "9007199254740992e22",
        "9007199254740993", "9007199254740995", "18446744073709551615", "18446744073709551616",
        "9999999999999999999", "99999999999999999999", "000000000000000000001.25",
        "0.00000000000000000000125", "12345678901234567890123456789e-20",
        "2.4703282292062327e-324", "2.4703282292062328e-324", "4.9406564584124654e-324",
        "2.2250738585072011e-308", "2.2250738585072012e-308", "2.2250738585072014e-308",
        "1.7976931348623157e308", "1.7976931348623158e308", "1.7976931348623159e308",
        "7.006492321624085e-46", "7.006492321624086e-46", "1.401298464324817e-45",
        "1.1754942807573643e-38", "1.1754943508222875e-38", "3.4028234663852886e38",
        "3.4028235677973366e38", "3.4028235677973367e38", "16777217", "16777219",
        "1.00000005960464477539062499999999999", "1.000000059604644775390625",
        "1.00000005960464477539062500000000001",
        "1.00000000000000011102230246251565404236316680908203125",
        "1.00000000000000033306690738754696212708950042724609375",
    ];
    for s in cases { check(s); }
    for midpoint in ["1.000000059604644775390625", "1.00000000000000011102230246251565404236316680908203125"] {
        check(&format!("{}{}", midpoint, "0".repeat(900)));
        check(&format!("{}{}1", midpoint, "0".repeat(900)));
    }
    // Exact half of the least subnormal: 2^-1075 = 5^1075 * 10^-1075.
    let mut digits = vec![1u8];
    for _ in 0..1075 {
        let mut carry = 0;
        for d in &mut digits {
            let value = *d * 5 + carry;
            *d = value % 10;
            carry = value / 10;
        }
        if carry != 0 { digits.push(carry); }
    }
    let integer: String = digits.iter().rev().map(|d| (b'0' + d) as char).collect();
    check(&format!("{integer}e-1075"));
    check(&format!("{integer}1e-1076"));
    check(&format!("{}e-10000", "9".repeat(10000)));
    check(&format!("0.{}1e10001", "0".repeat(10000)));
}

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn random_200k_strings() {
    let mut state = 0x81e9_4d72_c35a_b60fu64;
    for i in 0..200_000 {
        let bits = next(&mut state);
        let s = match i % 5 {
            0 => format!("{}", f64::from_bits(bits)),
            1 => format!("{:e}", f64::from_bits(bits)),
            2 => format!("{}", f32::from_bits(bits as u32)),
            3 => format!("{:e}", f32::from_bits(bits as u32)),
            _ => {
                let n = 1 + (next(&mut state) % 1100) as usize;
                let dot = (next(&mut state) % (n as u64 + 1)) as usize;
                let mut s = String::new();
                if bits & 1 != 0 { s.push('-'); }
                if bits & 2 != 0 { s.push_str("000000000000000000000"); }
                for j in 0..n {
                    if j == dot { s.push('.'); }
                    s.push((b'0' + (next(&mut state) % 10) as u8) as char);
                }
                s.push_str(&format!("e{}", (next(&mut state) % 2400) as i32 - 1600));
                s
            }
        };
        check(&s);
    }
}

#[test]
fn exponent_and_mantissa_boundaries() {
    for e in -350..=315 {
        for m in [1u64, 5, 16777215, 16777217, 9007199254740991, 9007199254740993, 999999999999999999, 1000000000000000000, 9999999999999999999] {
            check(&format!("{m}e{e}"));
            check(&format!("{m}0000000000000000001e{}", e - 19));
        }
    }
}

// Run explicitly with --ignored --nocapture; does not run the shared std bench.
#[test]
#[ignore]
fn quick_timing() {
    use std::hint::black_box;
    use std::time::Instant;
    let mut state = 0x97ac_7210_fed3_648bu64;
    let mut cases64 = Vec::new();
    let mut cases32 = Vec::new();
    while cases64.len() < 1024 {
        let value = f64::from_bits(next(&mut state));
        if value.is_finite() { cases64.push(format!("{value:e}")); }
    }
    while cases32.len() < 1024 {
        let value = f32::from_bits(next(&mut state) as u32);
        if value.is_finite() { cases32.push(format!("{value:e}")); }
    }
    let count = 512 * 1024;
    for width in [32, 64] {
        let cases = if width == 64 { &cases64 } else { &cases32 };
        let mut real = u128::MAX;
        let mut port = u128::MAX;
        for _ in 0..5 {
            let start = Instant::now();
            for i in 0..count {
                let s = black_box(cases[i % cases.len()].as_str());
                if width == 64 { black_box(s.parse::<f64>().unwrap()); }
                else { black_box(s.parse::<f32>().unwrap()); }
            }
            real = real.min(start.elapsed().as_nanos());
            let start = Instant::now();
            for i in 0..count {
                let s = black_box(cases[i % cases.len()].as_str());
                if width == 64 { black_box(dec2flt::parse_f64(s).unwrap()); }
                else { black_box(dec2flt::parse_f32(s).unwrap()); }
            }
            port = port.min(start.elapsed().as_nanos());
        }
        println!("f{width}: real {:.2} ns/parse, port {:.2} ns/parse, ratio {:.2} (best of 5, {count} parses)", real as f64 / count as f64, port as f64 / count as f64, port as f64 / real as f64);
    }
}
