// Every integer method on edge and random values, as real core: one `| ` line per type and
// method, the number of results and a digest of them (FNV-1a over their bytes), equal under
// rustc --test and makepad test. Also the integer parsing of the former std/check core_num.rs:
// its edge strings, the radix panic and its random strings over every radix for its seven types
// (core_parse_int.rs parses others). Tests are a01.. so rustc's order is the file's.

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
}

trait Obs {
    fn obs(&self, d: &mut D);
}
macro_rules! obs_int {
    ($($t:ty),*) => {$(
        impl Obs for $t {
            fn obs(&self, d: &mut D) {
                d.bytes(&self.to_le_bytes());
            }
        }
    )*};
}
obs_int!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);
impl Obs for bool {
    fn obs(&self, d: &mut D) {
        d.bytes(&[*self as u8]);
    }
}
impl<T: Obs> Obs for Option<T> {
    fn obs(&self, d: &mut D) {
        match self {
            None => d.bytes(&[0]),
            Some(v) => {
                d.bytes(&[1]);
                v.obs(d);
            }
        }
    }
}
impl<A: Obs, B: Obs> Obs for (A, B) {
    fn obs(&self, d: &mut D) {
        self.0.obs(d);
        self.1.obs(d);
    }
}
impl<const N: usize> Obs for [u8; N] {
    fn obs(&self, d: &mut D) {
        d.bytes(self);
    }
}

fn next(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

// one line: `| <type> <method> <results> <digest>`
macro_rules! line {
    ($t:ident, $name:expr, $d:expr) => {
        println!("| {} {} {} {:016x}", stringify!($t), $name, $d.1, $d.0)
    };
}
// a method of one value, over every value
macro_rules! m1 {
    ($t:ident, $vals:expr, $name:ident, |$a:ident| $e:expr) => {{
        let mut d = D::new();
        for &$a in $vals.iter() {
            $e.obs(&mut d);
            d.1 += 1;
        }
        line!($t, stringify!($name), d);
    }};
}
// a method of a value and a shift or exponent
macro_rules! mn {
    ($t:ident, $vals:expr, $name:ident, |$a:ident, $n:ident| $e:expr) => {{
        let mut d = D::new();
        for &$a in $vals.iter() {
            for $n in [0u32, 1, 3, 7, 8, 13, 31, 32, 33, 63, 64, 65, 127, 128, 200] {
                $e.obs(&mut d);
                d.1 += 1;
            }
        }
        line!($t, stringify!($name), d);
    }};
}
// a method of two values: every value with every `step`th one
macro_rules! m2 {
    ($t:ident, $vals:expr, $step:expr, $name:ident, |$a:ident, $b:ident| $e:expr) => {{
        let mut d = D::new();
        for &$a in $vals.iter() {
            for &$b in $vals.iter().step_by($step) {
                $e.obs(&mut d);
                d.1 += 1;
            }
        }
        line!($t, stringify!($name), d);
    }};
}

macro_rules! values {
    ($t:ident, $signed:expr) => {{
        let mut x = 0x9e3779b97f4a7c15u64 ^ (stringify!($t).len() as u64);
        let mut vals: Vec<$t> = vec![0, 1, 2, 3, 7, 8, 9, 10, 100, $t::MAX, $t::MAX - 1, $t::MIN, $t::MIN + 1, $t::MAX / 2, $t::MAX / 2 + 1];
        if $signed {
            vals.push((0 as $t).wrapping_sub(1));
            vals.push((0 as $t).wrapping_sub(2));
            vals.push((0 as $t).wrapping_sub(10));
        }
        for _ in 0..100 {
            let r = ((next(&mut x) as u128) << 64 | next(&mut x) as u128) as $t;
            vals.push(r);
            vals.push(r >> (next(&mut x) % ($t::BITS as u64)));
        }
        vals
    }};
}

macro_rules! common {
    ($t:ident, $v:expr) => {{
        let v = $v;
        println!("| {} MIN {} MAX {} BITS {}", stringify!($t), $t::MIN, $t::MAX, $t::BITS);
        m1!($t, v, count_ones, |a| a.count_ones());
        m1!($t, v, leading_zeros, |a| a.leading_zeros());
        m1!($t, v, trailing_zeros, |a| a.trailing_zeros());
        m1!($t, v, leading_ones, |a| a.leading_ones());
        m1!($t, v, trailing_ones, |a| a.trailing_ones());
        m1!($t, v, swap_bytes, |a| a.swap_bytes());
        m1!($t, v, reverse_bits, |a| a.reverse_bits());
        m1!($t, v, to_be, |a| a.to_be());
        m1!($t, v, to_le_bytes, |a| a.to_le_bytes());
        m1!($t, v, to_be_bytes, |a| a.to_be_bytes());
        m1!($t, v, from_le_bytes, |a| $t::from_le_bytes(a.to_le_bytes()));
        m1!($t, v, from_be_bytes, |a| $t::from_be_bytes(a.to_be_bytes()));
        mn!($t, v, rotate_left, |a, n| a.rotate_left(n));
        mn!($t, v, rotate_right, |a, n| a.rotate_right(n));
        mn!($t, v, overflowing_shl, |a, n| a.overflowing_shl(n));
        mn!($t, v, overflowing_shr, |a, n| a.overflowing_shr(n));
        mn!($t, v, checked_shl, |a, n| a.checked_shl(n));
        mn!($t, v, checked_shr, |a, n| a.checked_shr(n));
        mn!($t, v, wrapping_shl, |a, n| a.wrapping_shl(n));
        mn!($t, v, overflowing_pow, |a, n| a.overflowing_pow(n));
        mn!($t, v, checked_pow, |a, n| a.checked_pow(n));
        mn!($t, v, saturating_pow, |a, n| a.saturating_pow(n));
        mn!($t, v, wrapping_pow, |a, n| a.wrapping_pow(n));
        m1!($t, v, overflowing_neg, |a| a.overflowing_neg());
        m1!($t, v, checked_neg, |a| a.checked_neg());
        m1!($t, v, wrapping_neg, |a| a.wrapping_neg());
        m1!($t, v, isolate_highest_one, |a| a.isolate_highest_one());
        m1!($t, v, isolate_lowest_one, |a| a.isolate_lowest_one());
        m1!($t, v, highest_one, |a| a.highest_one());
        m1!($t, v, lowest_one, |a| a.lowest_one());
        m1!($t, v, checked_ilog2, |a| a.checked_ilog2());
        m1!($t, v, checked_ilog10, |a| a.checked_ilog10());
        {
            let mut d = D::new();
            for &a in v.iter() {
                for base in [2 as $t, 3, 10, 16] {
                    a.checked_ilog(base).obs(&mut d);
                    d.1 += 1;
                }
            }
            line!($t, "checked_ilog", d);
        }
        m2!($t, v, 7, overflowing_add, |a, b| a.overflowing_add(b));
        m2!($t, v, 7, overflowing_sub, |a, b| a.overflowing_sub(b));
        m2!($t, v, 7, overflowing_mul, |a, b| a.overflowing_mul(b));
        m2!($t, v, 7, checked_add, |a, b| a.checked_add(b));
        m2!($t, v, 7, checked_sub, |a, b| a.checked_sub(b));
        m2!($t, v, 7, checked_mul, |a, b| a.checked_mul(b));
        m2!($t, v, 7, saturating_add, |a, b| a.saturating_add(b));
        m2!($t, v, 7, saturating_sub, |a, b| a.saturating_sub(b));
        m2!($t, v, 7, saturating_mul, |a, b| a.saturating_mul(b));
        m2!($t, v, 7, wrapping_add, |a, b| a.wrapping_add(b));
        m2!($t, v, 7, wrapping_mul, |a, b| a.wrapping_mul(b));
        m2!($t, v, 7, abs_diff, |a, b| a.abs_diff(b));
        m2!($t, v, 7, checked_div, |a, b| a.checked_div(b));
        m2!($t, v, 7, checked_rem, |a, b| a.checked_rem(b));
        m2!($t, v, 7, checked_div_euclid, |a, b| a.checked_div_euclid(b));
        m2!($t, v, 7, checked_rem_euclid, |a, b| a.checked_rem_euclid(b));
        m2!($t, v, 7, overflowing_div, |a, b| if b != 0 { Some(a.overflowing_div(b)) } else { None });
        m2!($t, v, 7, overflowing_rem, |a, b| if b != 0 { Some(a.overflowing_rem(b)) } else { None });
        m2!($t, v, 7, wrapping_div, |a, b| if b != 0 { Some(a.wrapping_div(b)) } else { None });
        m2!($t, v, 7, wrapping_rem, |a, b| if b != 0 { Some(a.wrapping_rem(b)) } else { None });
        m2!($t, v, 7, div_euclid, |a, b| a.checked_div_euclid(b).map(|_| (a.div_euclid(b), a.rem_euclid(b))));
        {
            let mut d = D::new();
            for &a in v.iter() {
                let s = format!("{}", a);
                let r = $t::from_str_radix(&s, 10).ok();
                r.obs(&mut d);
                let x = format!("{:x}", a);
                $t::from_str_radix(&x, 16).ok().obs(&mut d);
                d.1 += 1;
            }
            line!($t, "from_str_radix_round_trip", d);
        }
        v
    }};
}

macro_rules! unsigned {
    ($fname:ident, $t:ident, $s:ident) => {
        #[test]
        fn $fname() {
            let v = common!($t, values!($t, false));
            m1!($t, v, is_power_of_two, |a| a.is_power_of_two());
            // next_power_of_two wraps to 0 on overflow in release, panics in debug: the checked form
            m1!($t, v, checked_next_power_of_two, |a| a.checked_next_power_of_two());
            m1!($t, v, next_power_of_two, |a| if a <= (1 as $t) << ($t::BITS - 1) { Some(a.next_power_of_two()) } else { None });
            m1!($t, v, isqrt, |a| a.isqrt());
            m2!($t, v, 11, div_ceil, |a, b| if b != 0 { Some(a.div_ceil(b)) } else { None });
            m2!($t, v, 11, checked_next_multiple_of, |a, b| if b != 0 { a.checked_next_multiple_of(b) } else { None });
            m2!($t, v, 11, checked_add_signed, |a, b| a.checked_add_signed(b as $s));
            m2!($t, v, 11, saturating_add_signed, |a, b| a.saturating_add_signed(b as $s));
            m2!($t, v, 11, wrapping_add_signed, |a, b| a.wrapping_add_signed(b as $s));
            m2!($t, v, 11, overflowing_add_signed, |a, b| a.overflowing_add_signed(b as $s));
            m2!($t, v, 11, checked_sub_signed, |a, b| a.checked_sub_signed(b as $s));
            m2!($t, v, 11, saturating_sub_signed, |a, b| a.saturating_sub_signed(b as $s));
            m2!($t, v, 11, wrapping_sub_signed, |a, b| a.wrapping_sub_signed(b as $s));
            m2!($t, v, 11, overflowing_sub_signed, |a, b| a.overflowing_sub_signed(b as $s));
        }
    };
}

macro_rules! signed {
    ($fname:ident, $t:ident, $u:ident) => {
        #[test]
        fn $fname() {
            let v = common!($t, values!($t, true));
            m1!($t, v, wrapping_abs, |a| a.wrapping_abs());
            m1!($t, v, unsigned_abs, |a| a.unsigned_abs());
            m1!($t, v, signum, |a| a.signum());
            m1!($t, v, checked_abs, |a| a.checked_abs());
            m1!($t, v, saturating_abs, |a| a.saturating_abs());
            m1!($t, v, saturating_neg, |a| a.saturating_neg());
            m1!($t, v, overflowing_abs, |a| a.overflowing_abs());
            m2!($t, v, 11, checked_add_unsigned, |a, b| a.checked_add_unsigned(b as $u));
            m2!($t, v, 11, saturating_add_unsigned, |a, b| a.saturating_add_unsigned(b as $u));
            m2!($t, v, 11, wrapping_add_unsigned, |a, b| a.wrapping_add_unsigned(b as $u));
            m2!($t, v, 11, overflowing_add_unsigned, |a, b| a.overflowing_add_unsigned(b as $u));
            m2!($t, v, 11, checked_sub_unsigned, |a, b| a.checked_sub_unsigned(b as $u));
            m2!($t, v, 11, saturating_sub_unsigned, |a, b| a.saturating_sub_unsigned(b as $u));
            m2!($t, v, 11, wrapping_sub_unsigned, |a, b| a.wrapping_sub_unsigned(b as $u));
            m2!($t, v, 11, overflowing_sub_unsigned, |a, b| a.overflowing_sub_unsigned(b as $u));
            m2!($t, v, 11, saturating_div, |a, b| if b != 0 { Some(a.saturating_div(b)) } else { None });
        }
    };
}

unsigned!(a01_u8, u8, i8);
unsigned!(a02_u16, u16, i16);
unsigned!(a03_u32, u32, i32);
unsigned!(a04_u64, u64, i64);
unsigned!(a05_u128, u128, i128);
unsigned!(a06_usize, usize, isize);
signed!(a07_i8, i8, u8);
signed!(a08_i16, i16, u16);
signed!(a09_i32, i32, u32);
signed!(a10_i64, i64, u64);
signed!(a11_i128, i128, u128);
signed!(a12_isize, isize, usize);

#[test]
fn a13_parse_edges() {
    let cases = ["", "+", "-", "0", "-0", "+5", "-5", "300x", "x300", "255", "256", "-128", "-129", "127", "128", "00012", "1_0", " 1", "1 ", "ff", "FF",
        "-9223372036854775808", "-9223372036854775809", "18446744073709551615", "18446744073709551616",
        "340282366920938463463374607431768211455", "340282366920938463463374607431768211456",
        "-170141183460469231731687303715884105728", "-170141183460469231731687303715884105729"];
    fn kind<T: std::fmt::Display>(r: Result<T, std::num::ParseIntError>) -> String {
        match r {
            Ok(v) => format!("{}", v),
            Err(e) => format!("{:?}", e.kind()),
        }
    }
    for c in cases {
        println!("| {:?} u8 {} i8 {} u64 {} i64 {} u128 {} i128 {} u32x {} i32r36 {}", c,
            kind(u8::from_str_radix(c, 10)), kind(i8::from_str_radix(c, 10)), kind(u64::from_str_radix(c, 10)), kind(i64::from_str_radix(c, 10)),
            kind(u128::from_str_radix(c, 10)), kind(i128::from_str_radix(c, 10)), kind(u32::from_str_radix(c, 16)), kind(i32::from_str_radix(c, 36)));
    }
}

#[test]
fn a14_parse_bad_radix() {
    let r = std::panic::catch_unwind(|| u8::from_str_radix("1", 37));
    let msg = match r {
        Ok(_) => "no panic".to_string(),
        Err(e) => e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default(),
    };
    println!("| radix 37: {}", msg);
}

#[test]
fn a15_parse_random_all_radixes() {
    let alphabet = b"0123456789000111999azAZfFgGz+-x ";
    let mut x = 0x9E3779B97F4A7C15u64;
    let mut d = D::new();
    for _ in 0..3000 {
        let r = next(&mut x);
        let len = (r % 26) as usize;
        let radix = 2 + ((r >> 8) % 35) as u32;
        let mut s = String::new();
        let mut y = r;
        for _ in 0..len {
            y = y.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            s.push(alphabet[((y >> 33) % alphabet.len() as u64) as usize] as char);
        }
        macro_rules! parse {
            ($($t:ty),*) => {$(
                match <$t>::from_str_radix(&s, radix) {
                    Ok(v) => d.bytes(v.to_string().as_bytes()),
                    Err(e) => d.bytes(format!("{:?}", e.kind()).as_bytes()),
                }
                d.1 += 1;
            )*};
        }
        parse!(u8, i8, u16, i32, u64, i64, u128);
    }
    println!("| parse random {} {:016x}", d.1, d.0);
}
