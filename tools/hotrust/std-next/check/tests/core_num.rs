//! Generated integer method bodies (core/num/int_impls.rs, via its rustc-checkable copy) vs
//! real std, on edge values and random values; integer parsing vs real str::parse.

use hotrust_std_check::num_int_check::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

macro_rules! check_int {
    ($fname:ident, $t:ident, $h:ident, $u:ident, $extra:ident, signed = $signed:expr) => {
        #[test]
        fn $fname() {
            let mut rng = Rng(0x9e3779b97f4a7c15 ^ (stringify!($t).len() as u64));
            let mut vals: Vec<$t> = vec![0, 1, 2, 3, 7, 8, 9, 10, 100, $t::MAX, $t::MAX - 1, $t::MIN, $t::MIN + 1, $t::MAX / 2, $t::MAX / 2 + 1];
            if $signed {
                vals.push((0 as $t).wrapping_sub(1));
                vals.push((0 as $t).wrapping_sub(2));
                vals.push((0 as $t).wrapping_sub(10));
            }
            for _ in 0..400 {
                let r = ((rng.next() as u128) << 64 | rng.next() as u128) as $t;
                vals.push(r);
                vals.push(r >> (rng.next() % ($t::BITS as u64)));
            }
            assert_eq!($h::MIN, $t::MIN);
            assert_eq!($h::MAX, $t::MAX);
            assert_eq!($h::BITS, $t::BITS);
            for &a in &vals {
                let h = $h(a);
                assert_eq!(h.count_ones(), a.count_ones());
                assert_eq!(h.leading_zeros(), a.leading_zeros());
                assert_eq!(h.trailing_zeros(), a.trailing_zeros());
                assert_eq!(h.leading_ones(), a.leading_ones());
                assert_eq!(h.trailing_ones(), a.trailing_ones());
                assert_eq!(h.swap_bytes(), a.swap_bytes());
                assert_eq!(h.reverse_bits(), a.reverse_bits());
                assert_eq!(h.to_be(), a.to_be());
                assert_eq!(h.to_le_bytes(), a.to_le_bytes());
                assert_eq!(h.to_be_bytes(), a.to_be_bytes());
                assert_eq!($h::from_le_bytes(a.to_le_bytes()), a);
                assert_eq!($h::from_be_bytes(a.to_be_bytes()), a);
                for n in [0u32, 1, 3, 7, 8, 13, 31, 32, 33, 63, 64, 65, 127, 128, 200] {
                    assert_eq!(h.rotate_left(n), a.rotate_left(n));
                    assert_eq!(h.rotate_right(n), a.rotate_right(n));
                    assert_eq!(h.overflowing_shl(n), a.overflowing_shl(n));
                    assert_eq!(h.overflowing_shr(n), a.overflowing_shr(n));
                    assert_eq!(h.checked_shl(n), a.checked_shl(n));
                    assert_eq!(h.checked_shr(n), a.checked_shr(n));
                    assert_eq!(h.wrapping_shl(n), a.wrapping_shl(n));
                    assert_eq!(h.overflowing_pow(n), a.overflowing_pow(n), "{}^{}", a, n);
                    assert_eq!(h.checked_pow(n), a.checked_pow(n));
                    assert_eq!(h.saturating_pow(n), a.saturating_pow(n));
                    assert_eq!(h.wrapping_pow(n), a.wrapping_pow(n));
                }
                assert_eq!(h.overflowing_neg(), a.overflowing_neg());
                assert_eq!(h.checked_neg(), a.checked_neg());
                assert_eq!(h.wrapping_neg(), a.wrapping_neg());
                assert_eq!(h.checked_ilog2(), a.checked_ilog2());
                assert_eq!(h.checked_ilog10(), a.checked_ilog10());
                for base in [2 as $t, 3, 10, 16] {
                    assert_eq!(h.checked_ilog(base), a.checked_ilog(base));
                }
                for &b in vals.iter().step_by(7) {
                    assert_eq!(h.overflowing_add(b), a.overflowing_add(b), "{} + {}", a, b);
                    assert_eq!(h.overflowing_sub(b), a.overflowing_sub(b), "{} - {}", a, b);
                    assert_eq!(h.overflowing_mul(b), a.overflowing_mul(b), "{} * {}", a, b);
                    assert_eq!(h.checked_add(b), a.checked_add(b));
                    assert_eq!(h.checked_sub(b), a.checked_sub(b));
                    assert_eq!(h.checked_mul(b), a.checked_mul(b));
                    assert_eq!(h.saturating_add(b), a.saturating_add(b));
                    assert_eq!(h.saturating_sub(b), a.saturating_sub(b));
                    assert_eq!(h.saturating_mul(b), a.saturating_mul(b));
                    assert_eq!(h.wrapping_add(b), a.wrapping_add(b));
                    assert_eq!(h.wrapping_mul(b), a.wrapping_mul(b));
                    assert_eq!(h.abs_diff(b), a.abs_diff(b));
                    assert_eq!(h.checked_div(b), a.checked_div(b));
                    assert_eq!(h.checked_rem(b), a.checked_rem(b));
                    assert_eq!(h.checked_div_euclid(b), a.checked_div_euclid(b));
                    assert_eq!(h.checked_rem_euclid(b), a.checked_rem_euclid(b));
                    if b != 0 {
                        assert_eq!(h.overflowing_div(b), a.overflowing_div(b));
                        assert_eq!(h.overflowing_rem(b), a.overflowing_rem(b));
                        assert_eq!(h.wrapping_div(b), a.wrapping_div(b));
                        assert_eq!(h.wrapping_rem(b), a.wrapping_rem(b));
                        if a.checked_div_euclid(b).is_some() {
                            assert_eq!(h.div_euclid(b), a.div_euclid(b));
                            assert_eq!(h.rem_euclid(b), a.rem_euclid(b));
                        }
                    }
                }
                let s = format!("{}", a);
                assert_eq!($h::from_str_radix(&s, 10).map_err(|e| format!("{:?}", e.kind())), $t::from_str_radix(&s, 10).map_err(|e| format!("{:?}", e.kind())));
                let x = format!("{:x}", a);
                assert_eq!($h::from_str_radix(&x, 16).ok(), $t::from_str_radix(&x, 16).ok());
            }
            $extra(&vals);
        }
    };
}

macro_rules! unsigned_extra {
    ($fname:ident, $t:ident, $h:ident, $s:ident) => {
        fn $fname(vals: &[$t]) {
            for &a in vals {
                let h = $h(a);
                assert_eq!(h.is_power_of_two(), a.is_power_of_two());
                assert_eq!(h.next_power_of_two(), a.wrapping_next_power_of_two_shim());
                assert_eq!(h.checked_next_power_of_two(), a.checked_next_power_of_two());
                assert_eq!(h.isqrt(), a.isqrt(), "isqrt {}", a);
                for &b in vals.iter().step_by(11) {
                    if b != 0 {
                        assert_eq!(h.div_ceil(b), a.div_ceil(b));
                        assert_eq!(h.checked_next_multiple_of(b), a.checked_next_multiple_of(b));
                    }
                    let sb = b as $s;
                    assert_eq!(h.checked_add_signed(sb), a.checked_add_signed(sb), "{} + {}", a, sb);
                    assert_eq!(h.saturating_add_signed(sb), a.saturating_add_signed(sb), "{} +sat {}", a, sb);
                    assert_eq!(h.wrapping_add_signed(sb), a.wrapping_add_signed(sb));
                }
            }
        }
    };
}

macro_rules! signed_extra {
    ($fname:ident, $t:ident, $h:ident, $u:ident) => {
        fn $fname(vals: &[$t]) {
            for &a in vals {
                let h = $h(a);
                assert_eq!(h.abs(), a.wrapping_abs());
                assert_eq!(h.unsigned_abs(), a.unsigned_abs());
                assert_eq!(h.signum(), a.signum());
                assert_eq!(h.checked_abs(), a.checked_abs());
                assert_eq!(h.saturating_abs(), a.saturating_abs());
                assert_eq!(h.saturating_neg(), a.saturating_neg());
                assert_eq!(h.overflowing_abs(), a.overflowing_abs());
                for &b in vals.iter().step_by(11) {
                    let ub = b as $u;
                    assert_eq!(h.checked_add_unsigned(ub), a.checked_add_unsigned(ub), "{} + {}", a, ub);
                    assert_eq!(h.saturating_add_unsigned(ub), a.saturating_add_unsigned(ub));
                    if b != 0 {
                        assert_eq!(h.saturating_div(b), a.saturating_div(b));
                    }
                }
            }
        }
    };
}

trait NextPow {
    fn wrapping_next_power_of_two_shim(self) -> Self;
}
macro_rules! next_pow {
    ($($t:ident),*) => {$(
        impl NextPow for $t {
            // core's next_power_of_two wraps to 0 on overflow in release
            fn wrapping_next_power_of_two_shim(self) -> $t { self.checked_next_power_of_two().unwrap_or(0) }
        }
    )*};
}
next_pow!(u8, u16, u32, u64, u128, usize);

unsigned_extra!(check_extra_u8, u8, H_u8, i8);
unsigned_extra!(check_extra_u16, u16, H_u16, i16);
unsigned_extra!(check_extra_u32, u32, H_u32, i32);
unsigned_extra!(check_extra_u64, u64, H_u64, i64);
unsigned_extra!(check_extra_u128, u128, H_u128, i128);
unsigned_extra!(check_extra_usize, usize, H_usize, isize);
signed_extra!(check_extra_i8, i8, H_i8, u8);
signed_extra!(check_extra_i16, i16, H_i16, u16);
signed_extra!(check_extra_i32, i32, H_i32, u32);
signed_extra!(check_extra_i64, i64, H_i64, u64);
signed_extra!(check_extra_i128, i128, H_i128, u128);
signed_extra!(check_extra_isize, isize, H_isize, usize);

check_int!(int_u8, u8, H_u8, u8, check_extra_u8, signed = false);
check_int!(int_u16, u16, H_u16, u16, check_extra_u16, signed = false);
check_int!(int_u32, u32, H_u32, u32, check_extra_u32, signed = false);
check_int!(int_u64, u64, H_u64, u64, check_extra_u64, signed = false);
check_int!(int_u128, u128, H_u128, u128, check_extra_u128, signed = false);
check_int!(int_usize, usize, H_usize, usize, check_extra_usize, signed = false);
check_int!(int_i8, i8, H_i8, u8, check_extra_i8, signed = true);
check_int!(int_i16, i16, H_i16, u16, check_extra_i16, signed = true);
check_int!(int_i32, i32, H_i32, u32, check_extra_i32, signed = true);
check_int!(int_i64, i64, H_i64, u64, check_extra_i64, signed = true);
check_int!(int_i128, i128, H_i128, u128, check_extra_i128, signed = true);
check_int!(int_isize, isize, H_isize, usize, check_extra_isize, signed = true);

#[test]
fn parse_errors_match() {
    let cases = ["", "+", "-", "0", "-0", "+5", "-5", "300x", "x300", "255", "256", "-128", "-129", "127", "128", "00012", "1_0", " 1", "1 ", "ff", "FF", "-9223372036854775808", "-9223372036854775809", "18446744073709551615", "18446744073709551616", "340282366920938463463374607431768211455", "340282366920938463463374607431768211456", "-170141183460469231731687303715884105728", "-170141183460469231731687303715884105729"];
    for c in cases {
        assert_eq!(H_u8::from_str_radix(c, 10).map_err(|e| e.kind().clone()).map_err(|k| format!("{:?}", k)), u8::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), "u8 {:?}", c);
        assert_eq!(H_i8::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), i8::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), "i8 {:?}", c);
        assert_eq!(H_u64::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), u64::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), "u64 {:?}", c);
        assert_eq!(H_i64::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), i64::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), "i64 {:?}", c);
        assert_eq!(H_u128::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), u128::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), "u128 {:?}", c);
        assert_eq!(H_i128::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), i128::from_str_radix(c, 10).map_err(|e| format!("{:?}", e.kind())), "i128 {:?}", c);
        assert_eq!(H_u32::from_str_radix(c, 16).map_err(|e| format!("{:?}", e.kind())), u32::from_str_radix(c, 16).map_err(|e| format!("{:?}", e.kind())), "u32 hex {:?}", c);
        assert_eq!(H_i32::from_str_radix(c, 36).map_err(|e| format!("{:?}", e.kind())), i32::from_str_radix(c, 36).map_err(|e| format!("{:?}", e.kind())), "i32 r36 {:?}", c);
    }
}

#[test]
#[should_panic(expected = "from_ascii_radix: radix must lie in the range `[2, 36]` - found 37")]
fn parse_bad_radix() {
    let _ = H_u8::from_str_radix("1", 37);
}
