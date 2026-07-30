use crate::int::number::uint::UIntNumber;
use core::cmp::Ordering;

pub trait UIntProduct<U: UIntNumber>: Copy + PartialOrd {
    fn multiply(a: U, b: U) -> Self;

    /// Divides this extended-width value by `divisor` and rounds to the nearest integer.
    ///
    /// The caller must guarantee that `divisor` is non-zero and its highest bit is not set:
    /// `0 < divisor < U::LAST_BIT`. The rounded quotient must fit in `U`.
    fn divide_with_rounding(&self, divisor: U) -> U;
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UIntProduct64 {
    value: u64,
}

impl UIntProduct<u32> for UIntProduct64 {
    #[inline(always)]
    fn multiply(a: u32, b: u32) -> Self {
        let value = a as u64 * b as u64;
        Self { value }
    }

    #[inline(always)]
    fn divide_with_rounding(&self, divisor: u32) -> u32 {
        debug_assert!(divisor > 0);
        debug_assert!(divisor < <u32 as UIntNumber>::LAST_BIT);

        let divisor = divisor as u64;
        let result = self.value / divisor;
        let remainder = self.value - result * divisor;
        let half = (divisor >> 1) + (divisor & 1);
        let round_up = remainder >= half;
        debug_assert!(result < u32::MAX as u64 || (result == u32::MAX as u64 && !round_up));

        if round_up {
            (result + 1) as u32
        } else {
            result as u32
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CompositeUIntProduct<U: UIntNumber> {
    high: U,
    low: U,
}

impl<U: UIntNumber> CompositeUIntProduct<U> {
    #[inline(always)]
    fn new(high: U, low: U) -> Self {
        Self { high, low }
    }

    #[inline]
    fn sum(a: U, b: U, c: U) -> (U, U) {
        let (s0, overflow0) = a.overflowing_add(b);
        let mut high = if overflow0 { U::ONE } else { U::ZERO };
        let (s1, overflow1) = s0.overflowing_add(c);
        if overflow1 {
            high += U::ONE;
        }

        (s1, high)
    }
}
impl<U: UIntNumber> UIntProduct<U> for CompositeUIntProduct<U> {
    #[inline]
    fn multiply(a: U, b: U) -> Self {
        if a.leading_zeros() + b.leading_zeros() >= U::BITS {
            return Self::new(U::ZERO, a * b);
        }

        let a1 = a >> U::HALF_BITS;
        let a0 = a & U::HALF_MASK;
        let b1 = b >> U::HALF_BITS;
        let b0 = b & U::HALF_MASK;

        let ab00 = a0 * b0;
        let (m_partial, m_high) = Self::sum(a0 * b1, a1 * b0, ab00 >> U::HALF_BITS);
        let high = a1 * b1 + (m_partial >> U::HALF_BITS) + (m_high << U::HALF_BITS);

        let low = (m_partial << U::HALF_BITS) | (ab00 & U::HALF_MASK);

        Self::new(high, low)
    }

    fn divide_with_rounding(&self, divisor: U) -> U {
        debug_assert!(divisor > U::ZERO);
        debug_assert!(divisor < U::LAST_BIT);
        debug_assert!(self.high < divisor);

        if self.high == U::ZERO {
            let result = self.low / divisor;
            let remainder = self.low - result * divisor;
            let half = (divisor >> 1) + (divisor & U::ONE);
            let round_up = remainder >= half;
            debug_assert!(result < U::MAX || !round_up);

            return if round_up { result + U::ONE } else { result };
        }

        let dn = divisor.leading_zeros();
        let norm_divisor = divisor << dn;
        let mut norm_dividend_high = (self.high << dn) | (self.low >> (U::BITS - dn));
        let mut norm_dividend_low = self.low << dn;

        let mut quotient = U::ZERO;

        for _ in 0..U::BITS {
            let bit = (norm_dividend_high & U::LAST_BIT) != U::ZERO;
            norm_dividend_high = (norm_dividend_high << 1) | (norm_dividend_low >> U::LAST_BIT_INDEX);
            norm_dividend_low <<= U::ONE;
            quotient <<= U::ONE;
            if norm_dividend_high >= norm_divisor || bit {
                norm_dividend_high = norm_dividend_high.wrapping_sub(norm_divisor);
                quotient |= U::ONE;
            }
        }

        // Check remainder for rounding
        let remainder = norm_dividend_high >> dn;
        let half = (divisor >> 1) + (divisor & U::ONE);
        let round_up = remainder >= half;
        debug_assert!(quotient < U::MAX || !round_up);

        if round_up {
            quotient += U::ONE;
        }

        quotient
    }
}

impl<U: UIntNumber> PartialOrd for CompositeUIntProduct<U> {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<U: UIntNumber> Ord for CompositeUIntProduct<U> {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> Ordering {
        let cmp_high = self.high.cmp(&other.high);
        match cmp_high {
            Ordering::Equal => self.low.cmp(&other.low),
            _ => cmp_high,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::int::number::product_uint::{CompositeUIntProduct, UIntProduct, UIntProduct64};
    use crate::int::number::uint::UIntNumber;

    #[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct UIntProduct128 {
        value: u128,
    }

    impl UIntProduct<u64> for UIntProduct128 {
        #[inline(always)]
        fn multiply(a: u64, b: u64) -> Self {
            Self {
                value: a as u128 * b as u128,
            }
        }

        #[inline(always)]
        fn divide_with_rounding(&self, divisor: u64) -> u64 {
            debug_assert!(divisor > 0);
            debug_assert!(divisor < <u64 as UIntNumber>::LAST_BIT);

            let divisor = divisor as u128;
            let result = self.value / divisor;
            let remainder = self.value - result * divisor;
            if remainder >= ((divisor >> 1) + (divisor & 1)) {
                (result + 1) as u64
            } else {
                result as u64
            }
        }
    }

    fn next_random(state: &mut u64) -> u64 {
        *state ^= *state << 7;
        *state ^= *state >> 9;
        *state ^= *state << 8;
        *state
    }

    #[test]
    fn test_u64_rounding_down() {
        let value = UIntProduct64 { value: 7 };
        assert_eq!(value.divide_with_rounding(3), 2);
    }

    #[test]
    fn test_u64_rounding_up() {
        let value = UIntProduct64 { value: 5 };
        assert_eq!(value.divide_with_rounding(2), 3);
    }

    #[test]
    fn test_u64_rounding_max_allowed_divisor() {
        let divisor = <u32 as UIntNumber>::LAST_BIT - 1;
        let value = UIntProduct64 {
            value: (divisor as u64 + 1) >> 1,
        };
        assert_eq!(value.divide_with_rounding(divisor), 1);
    }

    #[test]
    fn test_composite_divide_rounding_up_with_high_part() {
        let a = 0x3d92_38da_8840_c619;
        let b = 0x8e9e_5a83_c12f_0e93;
        let divisor = 0x29a0_9376_2663_471b;

        let composite = CompositeUIntProduct::<u64>::multiply(a, b);
        let reference = UIntProduct128::multiply(a, b);

        assert_eq!(
            composite.divide_with_rounding(divisor),
            reference.divide_with_rounding(divisor)
        );
    }

    #[test]
    fn test_composite_divide_matches_u128_reference() {
        for i in 1..1024u64 {
            let a = i
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                .wrapping_add(0x0123_4567_89ab_cdef)
                & (<u64 as UIntNumber>::LAST_BIT - 1);
            let b = i
                .wrapping_mul(0xbf58_476d_1ce4_e5b9)
                .wrapping_add(0x0fed_cba9_8765_4321)
                & (<u64 as UIntNumber>::LAST_BIT - 1);

            let composite = CompositeUIntProduct::<u64>::multiply(a, b);
            let reference = UIntProduct128::multiply(a, b);
            let divisor = composite.high + (i & 0xffff) + 1;

            assert!(divisor > composite.high);
            assert!(divisor < <u64 as UIntNumber>::LAST_BIT);
            assert_eq!(
                composite.divide_with_rounding(divisor),
                reference.divide_with_rounding(divisor)
            );
        }
    }

    #[test]
    fn test_composite_divide_matches_u128_reference_random() {
        let mut state = 0x4d59_5df4_d0f3_3173;

        for _ in 0..10_000 {
            let a = next_random(&mut state) & (<u64 as UIntNumber>::LAST_BIT - 1);
            let b = next_random(&mut state) & (<u64 as UIntNumber>::LAST_BIT - 1);
            let composite = CompositeUIntProduct::<u64>::multiply(a, b);
            let reference = UIntProduct128::multiply(a, b);

            let offset = (next_random(&mut state) & 0xffff) + 1;
            let divisor = composite.high + offset;

            assert!(divisor > composite.high);
            assert!(divisor < <u64 as UIntNumber>::LAST_BIT);
            assert_eq!(
                composite.divide_with_rounding(divisor),
                reference.divide_with_rounding(divisor)
            );
        }
    }

    #[test]
    fn test_basic() {
        let result = CompositeUIntProduct::<u64>::multiply(2, 3);
        assert_eq!(result.high, 0);
        assert_eq!(result.low, 6);
    }

    #[test]
    fn test_overflow() {
        let result = CompositeUIntProduct::<u64>::multiply(0x1_0000_0000, 0x1_0000_0000);
        assert_eq!(result.high, 1);
        assert_eq!(result.low, 0);
    }

    #[test]
    fn test_max() {
        let result = CompositeUIntProduct::<u64>::multiply(0xFFFF_FFFF_FFFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF);
        assert_eq!(result.high, 0xFFFF_FFFF_FFFF_FFFE);
        assert_eq!(result.low, 1);
    }

    #[test]
    fn test_zero() {
        let result = CompositeUIntProduct::<u64>::multiply(0, 1234567890);
        assert_eq!(result.high, 0);
        assert_eq!(result.low, 0);
    }

    #[test]
    fn test_one() {
        let result = CompositeUIntProduct::<u64>::multiply(1, 1234567890);
        assert_eq!(result.high, 0);
        assert_eq!(result.low, 1234567890);
    }

    #[test]
    fn test_0() {
        let result = CompositeUIntProduct::<u64>::multiply(0xFFFF_0000_FFFF_FFFF, 0xFFFF_FFFF_0000_FFFF);
        assert_eq!(result.high, 0xFFFF_0000_0001_FFFC);
        assert_eq!(result.low, 0x1_FFFF_FFFF_0001);
    }

    #[test]
    fn test_1() {
        let result = CompositeUIntProduct::<u64>::multiply(0x825e0a1f447a9d0f, 0xbeae05eb50b368cd);
        assert_eq!(result.high, 0x611a6a71c1b2333b);
        assert_eq!(result.low, 0x967f7971277add03);
    }

    #[test]
    fn test_2() {
        let result = CompositeUIntProduct::<u64>::multiply(0xa40f0cc4738525b, 0xc4339113aff1fb8);
        assert_eq!(result.high, 0x7dbc91bf17af89);
        assert_eq!(result.low, 0x76583e40a9193668);
    }

    #[test]
    fn test_3() {
        let result = CompositeUIntProduct::<u64>::multiply(0x013d10e9cab6d9101, 0x0ac718b6798f0cc2b);
        assert_eq!(result.high, 0xd593fe33e37ff5f);
        assert_eq!(result.low, 0xc8adf423a3e4272b);
    }

    #[test]
    fn test_4() {
        let result = CompositeUIntProduct::<u64>::multiply(0xfb1d552bec078d70, 0xcf842b7995bb80d0);
        assert_eq!(result.high, 0xcb8e5da39f8b7104);
        assert_eq!(result.low, 0xcbf4a27a0daaeb00);
    }

    #[test]
    fn test_5() {
        let result = CompositeUIntProduct::<u64>::multiply(0x38f44d557e6d9bc0, 0xb5f343ebf6828e7f);
        assert_eq!(result.high, 0x287ad9af49e2acce);
        assert_eq!(result.low, 0x86a08a1e1c44c440);
    }

    #[test]
    fn test_6() {
        let result = CompositeUIntProduct::<u64>::multiply(0x5522e2b50ba73069, 0x13bdc5312abbf74);
        assert_eq!(result.high, 0x690b32901dff58);
        assert_eq!(result.low, 0xcab645dabd034694);
    }

    #[test]
    fn test_7() {
        let result = CompositeUIntProduct::<u64>::multiply(0xfa2d1b0f09047b2a, 0xb0d6f746db94b662);
        assert_eq!(result.high, 0xacd115f5b8cb70b0);
        assert_eq!(result.low, 0xf7144b9ac58f0214);
    }

    #[test]
    fn test_8() {
        let result = CompositeUIntProduct::<u64>::multiply(0x24346c19605512a6, 0x6ccd292ea2c6cb3);
        assert_eq!(result.high, 0xf63216842b6cea);
        assert_eq!(result.low, 0xaec56ab92fe21212);
    }

    #[test]
    fn test_9() {
        let result = CompositeUIntProduct::<u64>::multiply(0xe9ffe90c7f3adc66, 0xebc8a042d873cba1);
        assert_eq!(result.high, 0xd7854d59960a17c4);
        assert_eq!(result.low, 0xbc51dd72c29b7e26);
    }

    #[test]
    fn test_10() {
        let result = CompositeUIntProduct::<u64>::multiply(0x22e0c81bc93e3a9, 0xa036a6c01e0db3);
        assert_eq!(result.high, 0x15d3ef338213a);
        assert_eq!(result.low, 0x65039ef3cbc5c42b);
    }
}
