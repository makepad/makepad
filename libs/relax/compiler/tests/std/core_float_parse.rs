// Float parsing syntax, round trips, decimal boundaries and exact midpoints.
// Differential file: `| ` lines must match under rustc --test and makepad test.

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

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn parse64(s: &str, d: &mut D) {
    match s.parse::<f64>() {
        Ok(v) => {
            d.bytes(&[1]);
            d.bytes(&v.to_bits().to_le_bytes());
        }
        Err(e) => {
            d.bytes(&[0]);
            d.text(&format!("{:?}", e));
            d.text(&e.to_string());
        }
    }
    d.1 += 1;
}

fn parse32(s: &str, d: &mut D) {
    match s.parse::<f32>() {
        Ok(v) => {
            d.bytes(&[1]);
            d.bytes(&v.to_bits().to_le_bytes());
        }
        Err(e) => {
            d.bytes(&[0]);
            d.text(&format!("{:?}", e));
            d.text(&e.to_string());
        }
    }
    d.1 += 1;
}

fn check(s: &str, d: &mut D) {
    parse64(s, d);
    parse32(s, d);
}

#[test]
fn a01_edges() {
    let mut d = D::new();
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
    for s in cases {
        check(s, &mut d);
    }
    for midpoint in ["1.000000059604644775390625", "1.00000000000000011102230246251565404236316680908203125"] {
        check(&format!("{}{}", midpoint, "0".repeat(900)), &mut d);
        check(&format!("{}{}1", midpoint, "0".repeat(900)), &mut d);
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
        if carry != 0 {
            digits.push(carry);
        }
    }
    let integer: String = digits.iter().rev().map(|d| (b'0' + d) as char).collect();
    check(&format!("{integer}e-1075"), &mut d);
    check(&format!("{integer}1e-1076"), &mut d);
    check(&format!("{}e-10000", "9".repeat(10000)), &mut d);
    check(&format!("0.{}1e10001", "0".repeat(10000)), &mut d);
    d.line("edges");
}

#[test]
fn a02_random_strings() {
    let mut d = D::new();
    let mut state = 0x81e9_4d72_c35a_b60fu64;
    for i in 0..2_000 {
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
                if bits & 1 != 0 {
                    s.push('-');
                }
                if bits & 2 != 0 {
                    s.push_str("000000000000000000000");
                }
                for j in 0..n {
                    if j == dot {
                        s.push('.');
                    }
                    s.push((b'0' + (next(&mut state) % 10) as u8) as char);
                }
                s.push_str(&format!("e{}", (next(&mut state) % 2400) as i32 - 1600));
                s
            }
        };
        check(&s, &mut d);
    }
    d.line("random_strings");
}

#[test]
fn a03_exponent_and_mantissa_boundaries() {
    let mut d = D::new();
    for e in -350..=315 {
        for m in [
            1u64, 5, 16777215, 16777217, 9007199254740991, 9007199254740993,
            999999999999999999, 1000000000000000000, 9999999999999999999,
        ] {
            check(&format!("{m}e{e}"), &mut d);
            check(&format!("{m}0000000000000000001e{}", e - 19), &mut d);
        }
    }
    d.line("exponent_and_mantissa_boundaries");
}

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
fn a04_parse_syntax() {
    let mut d = D::new();
    let cases = [
        "", "+", "-", ".", "e", "e5", "1e", "1e+", "1e-", "1.", ".5", "+.5", "-.5", "1.5e3", "1.5E-3", "1e+05",
        "0", "-0",
        "+0", "00", "0.0", "-0.0e10", "inf", "-inf", "+inf", "INF", "Inf", "infinity", "Infinity", "-INFINITY", "infin",
        "nan", "NaN", "-nan", "+NAN", "nan(1)", " 1", "1 ", "1_0", "0x10", "1e1.5", "1..2", "--1", "+-1", "1e400",
        "-1e400", "1e-400", "2.4703282292062327e-324", "2.4703282292062328e-324", "4.9406564584124654e-324",
        "1.7976931348623157e308", "1.7976931348623158e308", "1.7976931348623159e308",
        concat!(
            "179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017",
            "977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273",
            "854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704",
            "342711559699508093042880177904174497791.99999999999999999999999999999999999999999999999999",
            "99999999999999999999",
        ),
        "0.000000000000000000000000000000000000000000001", "123456789012345678901234567890", "9007199254740993",
        "9007199254740992.5", "1e23", "8.589973e9", "3.4028235e38", "3.4028236e38", "1.17549435e-38", "1e-45", "7e-46",
        "0e999999999", "1e-99999999", "1e99999999", "000000000000000000000000000000000001.5",
        "1.00000000000000000000000000000000000000000000000000000000000001",
    ];
    for s in cases {
        parse64(s, &mut d);
        parse32(s, &mut d);
    }
    d.line("parse_syntax");
}

#[test]
fn a05_parse_round_trips() {
    let mut r = Rng(777);
    let mut d = D::new();
    for _ in 0..2_000 {
        let v = f64::from_bits(r.next());
        if !v.is_finite() {
            continue;
        }
        parse64(&format!("{}", v), &mut d);
        parse64(&format!("{:e}", v), &mut d);
        parse64(&format!("{:.3e}", v), &mut d);
        let v32 = f32::from_bits(r.next() as u32);
        if v32.is_finite() {
            parse32(&format!("{}", v32), &mut d);
            parse32(&format!("{:.2e}", v32), &mut d);
            parse64(&format!("{:?}", v32), &mut d);
        }
    }
    d.line("parse_round_trips");
}

#[test]
fn a06_parse_halfway_points() {
    let mut r = Rng(4242);
    let mut d = D::new();
    for k in 0..30 {
        let bits = if k % 3 == 0 { r.next() & 0x000f_ffff_ffff_ffff } else { r.next() & 0x7fef_ffff_ffff_ffff };
        let exp_field = ((bits >> 52) & 0x7ff) as i32;
        let (m, e) = if exp_field == 0 {
            (bits & 0xf_ffff_ffff_ffff, -1074)
        } else {
            ((bits & 0xf_ffff_ffff_ffff) | (1 << 52), exp_field - 1075)
        };
        let mid = midpoint_decimal(m, e);
        parse64(&mid, &mut d);
        // just above / below the midpoint
        let mut above = mid.clone();
        if !above.contains('.') {
            above.push('.');
        }
        above.push_str("000000000000000000001");
        parse64(&above, &mut d);
        let below = if mid.ends_with('5') {
            let mut b = mid[..mid.len() - 1].to_string();
            b.push_str("49999999999999999999");
            b
        } else {
            mid.clone()
        };
        parse64(&below, &mut d);
        // with an exponent
        parse64(&format!("{}e-3", mid.replace('.', "")), &mut d);
    }
    // f32 midpoints
    for _ in 0..30 {
        let bits = (r.next() as u32) & 0x7f7f_ffff;
        let exp_field = ((bits >> 23) & 0xff) as i32;
        let (m, e) = if exp_field == 0 {
            ((bits & 0x7f_ffff) as u64, -149)
        } else {
            (((bits & 0x7f_ffff) | (1 << 23)) as u64, exp_field - 150)
        };
        let mid = midpoint_decimal(m, e);
        parse32(&mid, &mut d);
        let mut above = mid.clone();
        if !above.contains('.') {
            above.push('.');
        }
        above.push_str("0000000001");
        parse32(&above, &mut d);
    }
    d.line("parse_halfway_points");
}

#[test]
fn a07_parse_random_digit_strings() {
    let mut r = Rng(99);
    let mut d = D::new();
    for _ in 0..1_000 {
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
        parse64(&s, &mut d);
        parse32(&s, &mut d);
    }
    d.line("parse_random_digit_strings");
}
