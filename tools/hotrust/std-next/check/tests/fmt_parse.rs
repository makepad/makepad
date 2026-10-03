//! Float parsing: HotRust's dec2flt vs real str::parse, on syntax edge cases, round trips,
//! exact halfway points and long digit strings.

mod fmt_common;

use fmt_common::{render, Rng, DEF};
use hotrust_std_check::fmt as hf;
use hotrust_std_check::dec2flt;

fn same64(s: &str, fails: &mut usize) {
    let want = s.parse::<f64>();
    let got = dec2flt::parse_f64(s);
    let ok = match (&got, &want) {
        (Ok(g), Ok(w)) => g.to_bits() == w.to_bits() || (g.is_nan() && w.is_nan() && g.is_sign_negative() == w.is_sign_negative()),
        (Err(g), Err(w)) => render(g, <dec2flt::ParseFloatError as hf::Display>::fmt, &DEF) == w.to_string() && format!("{:?}", g) == format!("{:?}", w),
        _ => false,
    };
    if !ok {
        *fails += 1;
        if *fails < 30 {
            eprintln!("MISMATCH f64 {:?}: got {:?} want {:?}", s, got.map(|v| v.to_bits()), want.map(|v| v.to_bits()));
        }
    }
}

fn same32(s: &str, fails: &mut usize) {
    let want = s.parse::<f32>();
    let got = dec2flt::parse_f32(s);
    let ok = match (&got, &want) {
        (Ok(g), Ok(w)) => g.to_bits() == w.to_bits() || (g.is_nan() && w.is_nan() && g.is_sign_negative() == w.is_sign_negative()),
        (Err(g), Err(w)) => render(g, <dec2flt::ParseFloatError as hf::Display>::fmt, &DEF) == w.to_string(),
        _ => false,
    };
    if !ok {
        *fails += 1;
        if *fails < 30 {
            eprintln!("MISMATCH f32 {:?}: got {:?} want {:?}", s, got.map(|v| v.to_bits()), want.map(|v| v.to_bits()));
        }
    }
}

#[test]
fn parse_syntax() {
    let mut fails = 0;
    let cases = [
        "", "+", "-", ".", "e", "e5", "1e", "1e+", "1e-", "1.", ".5", "+.5", "-.5", "1.5e3", "1.5E-3", "1e+05", "0", "-0",
        "+0", "00", "0.0", "-0.0e10", "inf", "-inf", "+inf", "INF", "Inf", "infinity", "Infinity", "-INFINITY", "infin",
        "nan", "NaN", "-nan", "+NAN", "nan(1)", " 1", "1 ", "1_0", "0x10", "1e1.5", "1..2", "--1", "+-1", "1e400",
        "-1e400", "1e-400", "2.4703282292062327e-324", "2.4703282292062328e-324", "4.9406564584124654e-324",
        "1.7976931348623157e308", "1.7976931348623158e308", "1.7976931348623159e308", "179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791.9999999999999999999999999999999999999999999999999999999999999999999999",
        "0.000000000000000000000000000000000000000000001", "123456789012345678901234567890", "9007199254740993",
        "9007199254740992.5", "1e23", "8.589973e9", "3.4028235e38", "3.4028236e38", "1.17549435e-38", "1e-45", "7e-46",
        "0e999999999", "1e-99999999", "1e99999999", "000000000000000000000000000000000001.5",
        "1.00000000000000000000000000000000000000000000000000000000000001",
    ];
    for s in cases {
        same64(s, &mut fails);
        same32(s, &mut fails);
    }
    assert_eq!(fails, 0);
}

#[test]
fn parse_round_trips() {
    let mut r = Rng(777);
    let mut fails = 0;
    for _ in 0..200_000 {
        let v = f64::from_bits(r.next());
        if !v.is_finite() {
            continue;
        }
        same64(&format!("{}", v), &mut fails);
        same64(&format!("{:e}", v), &mut fails);
        same64(&format!("{:.3e}", v), &mut fails);
        let v32 = f32::from_bits(r.next() as u32);
        if v32.is_finite() {
            same32(&format!("{}", v32), &mut fails);
            same32(&format!("{:.2e}", v32), &mut fails);
            same64(&format!("{:?}", v32), &mut fails);
        }
    }
    assert_eq!(fails, 0);
}

/// Exact decimal string of (2m+1) * 2^(e-1): the midpoint above m * 2^e.
fn midpoint_decimal(m: u64, e: i32) -> String {
    // big integer in base 1e9, little endian
    let mut big: Vec<u64> = vec![];
    let mut x = 2 * (m as u128) + 1;
    while x > 0 {
        big.push((x % 1_000_000_000) as u64);
        x /= 1_000_000_000;
    }
    let mul = |big: &mut Vec<u64>, k: u64| {
        let mut carry = 0u64;
        for d in big.iter_mut() {
            let v = *d * k + carry;
            *d = v % 1_000_000_000;
            carry = v / 1_000_000_000;
        }
        while carry > 0 {
            big.push(carry % 1_000_000_000);
            carry /= 1_000_000_000;
        }
    };
    let sh = e - 1;
    let mut frac_digits = 0usize;
    if sh >= 0 {
        for _ in 0..sh {
            mul(&mut big, 2);
        }
    } else {
        for _ in 0..(-sh) {
            mul(&mut big, 5);
        }
        frac_digits = (-sh) as usize;
    }
    let mut s = String::new();
    for (i, d) in big.iter().rev().enumerate() {
        if i == 0 {
            s.push_str(&d.to_string());
        } else {
            s.push_str(&format!("{:09}", d));
        }
    }
    if frac_digits > 0 {
        while s.len() <= frac_digits {
            s.insert(0, '0');
        }
        let p = s.len() - frac_digits;
        s.insert(p, '.');
    }
    s
}

#[test]
fn parse_halfway_points() {
    let mut r = Rng(4242);
    let mut fails = 0;
    for k in 0..3000 {
        let bits = if k % 3 == 0 { r.next() & 0x000f_ffff_ffff_ffff } else { r.next() & 0x7fef_ffff_ffff_ffff };
        let v = f64::from_bits(bits);
        let exp_field = ((bits >> 52) & 0x7ff) as i32;
        let (m, e) = if exp_field == 0 {
            (bits & 0xf_ffff_ffff_ffff, -1074)
        } else {
            ((bits & 0xf_ffff_ffff_ffff) | (1 << 52), exp_field - 1075)
        };
        let _ = v;
        let mid = midpoint_decimal(m, e);
        same64(&mid, &mut fails);
        // just above / below the midpoint
        let mut above = mid.clone();
        if !above.contains('.') {
            above.push('.');
        }
        above.push_str("000000000000000000001");
        same64(&above, &mut fails);
        let below = if mid.ends_with('5') {
            let mut b = mid[..mid.len() - 1].to_string();
            b.push_str("49999999999999999999");
            b
        } else {
            mid.clone()
        };
        same64(&below, &mut fails);
        // with an exponent
        same64(&format!("{}e-3", mid.replace('.', "")), &mut fails);
    }
    // f32 midpoints
    for _ in 0..3000 {
        let bits = (r.next() as u32) & 0x7f7f_ffff;
        let exp_field = ((bits >> 23) & 0xff) as i32;
        let (m, e) = if exp_field == 0 { ((bits & 0x7f_ffff) as u64, -149) } else { (((bits & 0x7f_ffff) | (1 << 23)) as u64, exp_field - 150) };
        let mid = midpoint_decimal(m, e);
        same32(&mid, &mut fails);
        let mut above = mid.clone();
        if !above.contains('.') {
            above.push('.');
        }
        above.push_str("0000000001");
        same32(&above, &mut fails);
    }
    assert_eq!(fails, 0);
}

#[test]
fn parse_random_digit_strings() {
    let mut r = Rng(99);
    let mut fails = 0;
    for _ in 0..100_000 {
        let mut s = String::new();
        if r.below(4) == 0 {
            s.push('-');
        }
        let n_int = r.below(30) as usize;
        for _ in 0..n_int {
            s.push((b'0' + r.below(10) as u8) as char);
        }
        if r.below(2) == 0 {
            s.push('.');
            let n_frac = if r.below(20) == 0 { r.below(900) as usize } else { r.below(25) as usize };
            for _ in 0..n_frac {
                s.push((b'0' + r.below(10) as u8) as char);
            }
        }
        if r.below(2) == 0 {
            s.push(if r.below(2) == 0 { 'e' } else { 'E' });
            let ev = r.below(700) as i64 - 350;
            s.push_str(&ev.to_string());
        }
        same64(&s, &mut fails);
        same32(&s, &mut fails);
    }
    assert_eq!(fails, 0);
}
