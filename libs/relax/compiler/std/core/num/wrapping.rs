//! Integer wrappers for modular and saturating arithmetic.

use crate::fmt;
use crate::ops::*;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default, Hash)]
#[repr(transparent)]
pub struct Wrapping<T>(pub T);

impl<T: fmt::Debug> fmt::Debug for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::Display> fmt::Display for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::LowerHex> fmt::LowerHex for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::UpperHex> fmt::UpperHex for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::Binary> fmt::Binary for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::Octal> fmt::Octal for Wrapping<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

macro_rules! forward_ref_unop {
    (impl $imp:ident, $method:ident for $t:ty) => {
        impl $imp for &$t {
            type Output = <$t as $imp>::Output;

            #[inline]
            fn $method(self) -> <$t as $imp>::Output {
                $imp::$method(*self)
            }
        }
    }
}

macro_rules! forward_ref_binop {
    (impl $imp:ident, $method:ident for $t:ty, $u:ty) => {
        impl $imp<$u> for &$t {
            type Output = <$t as $imp<$u>>::Output;

            #[inline]
            #[track_caller]
            fn $method(self, other: $u) -> <$t as $imp<$u>>::Output {
                $imp::$method(*self, other)
            }
        }

        impl $imp<&$u> for $t {
            type Output = <$t as $imp<$u>>::Output;

            #[inline]
            #[track_caller]
            fn $method(self, other: &$u) -> <$t as $imp<$u>>::Output {
                $imp::$method(self, *other)
            }
        }

        impl $imp<&$u> for &$t {
            type Output = <$t as $imp<$u>>::Output;

            #[inline]
            #[track_caller]
            fn $method(self, other: &$u) -> <$t as $imp<$u>>::Output {
                $imp::$method(*self, *other)
            }
        }
    }
}

macro_rules! forward_ref_op_assign {
    (impl $imp:ident, $method:ident for $t:ty, $u:ty) => {
        impl $imp<&$u> for $t {
            #[inline]
            #[track_caller]
            fn $method(&mut self, other: &$u) {
                $imp::$method(self, *other);
            }
        }
    }
}

#[allow(unused_macros)]
macro_rules! sh_impl_signed {
    ($t:ident, $f:ident) => {
        impl Shl<$f> for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn shl(self, other: $f) -> Wrapping<$t> {
                if other < 0 {
                    Wrapping(self.0.wrapping_shr(-other as u32))
                } else {
                    Wrapping(self.0.wrapping_shl(other as u32))
                }
            }
        }
        forward_ref_binop! { impl Shl, shl for Wrapping<$t>, $f }

        impl ShlAssign<$f> for Wrapping<$t> {
            #[inline]
            fn shl_assign(&mut self, other: $f) {
                *self = *self << other;
            }
        }
        forward_ref_op_assign! { impl ShlAssign, shl_assign for Wrapping<$t>, $f }

        impl Shr<$f> for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn shr(self, other: $f) -> Wrapping<$t> {
                if other < 0 {
                    Wrapping(self.0.wrapping_shl(-other as u32))
                } else {
                    Wrapping(self.0.wrapping_shr(other as u32))
                }
            }
        }
        forward_ref_binop! { impl Shr, shr for Wrapping<$t>, $f }

        impl ShrAssign<$f> for Wrapping<$t> {
            #[inline]
            fn shr_assign(&mut self, other: $f) {
                *self = *self >> other;
            }
        }
        forward_ref_op_assign! { impl ShrAssign, shr_assign for Wrapping<$t>, $f }
    };
}

macro_rules! sh_impl_unsigned {
    ($t:ident, $f:ident) => {
        impl Shl<$f> for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn shl(self, other: $f) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_shl(other as u32))
            }
        }
        forward_ref_binop! { impl Shl, shl for Wrapping<$t>, $f }

        impl ShlAssign<$f> for Wrapping<$t> {
            #[inline]
            fn shl_assign(&mut self, other: $f) {
                *self = *self << other;
            }
        }
        forward_ref_op_assign! { impl ShlAssign, shl_assign for Wrapping<$t>, $f }

        impl Shr<$f> for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn shr(self, other: $f) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_shr(other as u32))
            }
        }
        forward_ref_binop! { impl Shr, shr for Wrapping<$t>, $f }

        impl ShrAssign<$f> for Wrapping<$t> {
            #[inline]
            fn shr_assign(&mut self, other: $f) {
                *self = *self >> other;
            }
        }
        forward_ref_op_assign! { impl ShrAssign, shr_assign for Wrapping<$t>, $f }
    };
}

macro_rules! sh_impl_all {
    ($($t:ident)*) => ($(
        //sh_impl_unsigned! { $t, u8 }
        //sh_impl_unsigned! { $t, u16 }
        //sh_impl_unsigned! { $t, u32 }
        //sh_impl_unsigned! { $t, u64 }
        //sh_impl_unsigned! { $t, u128 }
        sh_impl_unsigned! { $t, usize }

        //sh_impl_signed! { $t, i8 }
        //sh_impl_signed! { $t, i16 }
        //sh_impl_signed! { $t, i32 }
        //sh_impl_signed! { $t, i64 }
        //sh_impl_signed! { $t, i128 }
        //sh_impl_signed! { $t, isize }
    )*)
}

sh_impl_all! { u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize }

macro_rules! wrapping_impl {
    ($($t:ty)*) => ($(
        impl Add for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn add(self, other: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_add(other.0))
            }
        }
        forward_ref_binop! { impl Add, add for Wrapping<$t>, Wrapping<$t> }

        impl AddAssign for Wrapping<$t> {
            #[inline]
            fn add_assign(&mut self, other: Wrapping<$t>) {
                *self = *self + other;
            }
        }
        forward_ref_op_assign! { impl AddAssign, add_assign for Wrapping<$t>, Wrapping<$t> }

        impl AddAssign<$t> for Wrapping<$t> {
            #[inline]
            fn add_assign(&mut self, other: $t) {
                *self = *self + Wrapping(other);
            }
        }
        forward_ref_op_assign! { impl AddAssign, add_assign for Wrapping<$t>, $t }

        impl Sub for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn sub(self, other: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_sub(other.0))
            }
        }
        forward_ref_binop! { impl Sub, sub for Wrapping<$t>, Wrapping<$t> }

        impl SubAssign for Wrapping<$t> {
            #[inline]
            fn sub_assign(&mut self, other: Wrapping<$t>) {
                *self = *self - other;
            }
        }
        forward_ref_op_assign! { impl SubAssign, sub_assign for Wrapping<$t>, Wrapping<$t> }

        impl SubAssign<$t> for Wrapping<$t> {
            #[inline]
            fn sub_assign(&mut self, other: $t) {
                *self = *self - Wrapping(other);
            }
        }
        forward_ref_op_assign! { impl SubAssign, sub_assign for Wrapping<$t>, $t }

        impl Mul for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn mul(self, other: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_mul(other.0))
            }
        }
        forward_ref_binop! { impl Mul, mul for Wrapping<$t>, Wrapping<$t> }

        impl MulAssign for Wrapping<$t> {
            #[inline]
            fn mul_assign(&mut self, other: Wrapping<$t>) {
                *self = *self * other;
            }
        }
        forward_ref_op_assign! { impl MulAssign, mul_assign for Wrapping<$t>, Wrapping<$t> }

        impl MulAssign<$t> for Wrapping<$t> {
            #[inline]
            fn mul_assign(&mut self, other: $t) {
                *self = *self * Wrapping(other);
            }
        }
        forward_ref_op_assign! { impl MulAssign, mul_assign for Wrapping<$t>, $t }

        impl Div for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn div(self, other: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_div(other.0))
            }
        }
        forward_ref_binop! { impl Div, div for Wrapping<$t>, Wrapping<$t> }

        impl DivAssign for Wrapping<$t> {
            #[inline]
            fn div_assign(&mut self, other: Wrapping<$t>) {
                *self = *self / other;
            }
        }
        forward_ref_op_assign! { impl DivAssign, div_assign for Wrapping<$t>, Wrapping<$t> }

        impl DivAssign<$t> for Wrapping<$t> {
            #[inline]
            fn div_assign(&mut self, other: $t) {
                *self = *self / Wrapping(other);
            }
        }
        forward_ref_op_assign! { impl DivAssign, div_assign for Wrapping<$t>, $t }

        impl Rem for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn rem(self, other: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_rem(other.0))
            }
        }
        forward_ref_binop! { impl Rem, rem for Wrapping<$t>, Wrapping<$t> }

        impl RemAssign for Wrapping<$t> {
            #[inline]
            fn rem_assign(&mut self, other: Wrapping<$t>) {
                *self = *self % other;
            }
        }
        forward_ref_op_assign! { impl RemAssign, rem_assign for Wrapping<$t>, Wrapping<$t> }

        impl RemAssign<$t> for Wrapping<$t> {
            #[inline]
            fn rem_assign(&mut self, other: $t) {
                *self = *self % Wrapping(other);
            }
        }
        forward_ref_op_assign! { impl RemAssign, rem_assign for Wrapping<$t>, $t }

        impl Not for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn not(self) -> Wrapping<$t> {
                Wrapping(!self.0)
            }
        }
        forward_ref_unop! { impl Not, not for Wrapping<$t> }

        impl BitXor for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn bitxor(self, other: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0 ^ other.0)
            }
        }
        forward_ref_binop! { impl BitXor, bitxor for Wrapping<$t>, Wrapping<$t> }

        impl BitXorAssign for Wrapping<$t> {
            #[inline]
            fn bitxor_assign(&mut self, other: Wrapping<$t>) {
                *self = *self ^ other;
            }
        }
        forward_ref_op_assign! { impl BitXorAssign, bitxor_assign for Wrapping<$t>, Wrapping<$t> }

        impl BitXorAssign<$t> for Wrapping<$t> {
            #[inline]
            fn bitxor_assign(&mut self, other: $t) {
                *self = *self ^ Wrapping(other);
            }
        }
        forward_ref_op_assign! { impl BitXorAssign, bitxor_assign for Wrapping<$t>, $t }

        impl BitOr for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn bitor(self, other: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0 | other.0)
            }
        }
        forward_ref_binop! { impl BitOr, bitor for Wrapping<$t>, Wrapping<$t> }

        impl BitOrAssign for Wrapping<$t> {
            #[inline]
            fn bitor_assign(&mut self, other: Wrapping<$t>) {
                *self = *self | other;
            }
        }
        forward_ref_op_assign! { impl BitOrAssign, bitor_assign for Wrapping<$t>, Wrapping<$t> }

        impl BitOrAssign<$t> for Wrapping<$t> {
            #[inline]
            fn bitor_assign(&mut self, other: $t) {
                *self = *self | Wrapping(other);
            }
        }
        forward_ref_op_assign! { impl BitOrAssign, bitor_assign for Wrapping<$t>, $t }

        impl BitAnd for Wrapping<$t> {
            type Output = Wrapping<$t>;

            #[inline]
            fn bitand(self, other: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0 & other.0)
            }
        }
        forward_ref_binop! { impl BitAnd, bitand for Wrapping<$t>, Wrapping<$t> }

        impl BitAndAssign for Wrapping<$t> {
            #[inline]
            fn bitand_assign(&mut self, other: Wrapping<$t>) {
                *self = *self & other;
            }
        }
        forward_ref_op_assign! { impl BitAndAssign, bitand_assign for Wrapping<$t>, Wrapping<$t> }

        impl BitAndAssign<$t> for Wrapping<$t> {
            #[inline]
            fn bitand_assign(&mut self, other: $t) {
                *self = *self & Wrapping(other);
            }
        }
        forward_ref_op_assign! { impl BitAndAssign, bitand_assign for Wrapping<$t>, $t }

        impl Neg for Wrapping<$t> {
            type Output = Self;
            #[inline]
            fn neg(self) -> Self {
                Wrapping(0) - self
            }
        }
        forward_ref_unop! { impl Neg, neg for Wrapping<$t> }

    )*)
}

wrapping_impl! { usize u8 u16 u32 u64 u128 isize i8 i16 i32 i64 i128 }

/// Saturating<T>: the operator impls of core::num::saturating (binops with a `Saturating` rhs, the
/// `op=` impls with either rhs, Not), through the integer's saturating methods.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default, Hash)]
#[repr(transparent)]
pub struct Saturating<T>(pub T);

impl<T: fmt::Debug> fmt::Debug for Saturating<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl<T: fmt::Display> fmt::Display for Saturating<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

macro_rules! saturating_impl {
    ($($t:ty)*) => ($(
        impl Add for Saturating<$t> {
            type Output = Saturating<$t>;
            fn add(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0.saturating_add(rhs.0))
            }
        }
        impl AddAssign for Saturating<$t> {
            fn add_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0.saturating_add(rhs.0);
            }
        }
        impl AddAssign<$t> for Saturating<$t> {
            fn add_assign(&mut self, rhs: $t) {
                self.0 = self.0.saturating_add(rhs);
            }
        }
        impl Sub for Saturating<$t> {
            type Output = Saturating<$t>;
            fn sub(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0.saturating_sub(rhs.0))
            }
        }
        impl SubAssign for Saturating<$t> {
            fn sub_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0.saturating_sub(rhs.0);
            }
        }
        impl SubAssign<$t> for Saturating<$t> {
            fn sub_assign(&mut self, rhs: $t) {
                self.0 = self.0.saturating_sub(rhs);
            }
        }
        impl Mul for Saturating<$t> {
            type Output = Saturating<$t>;
            fn mul(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0.saturating_mul(rhs.0))
            }
        }
        impl MulAssign for Saturating<$t> {
            fn mul_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0.saturating_mul(rhs.0);
            }
        }
        impl MulAssign<$t> for Saturating<$t> {
            fn mul_assign(&mut self, rhs: $t) {
                self.0 = self.0.saturating_mul(rhs);
            }
        }
        impl Div for Saturating<$t> {
            type Output = Saturating<$t>;
            fn div(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0.saturating_div(rhs.0))
            }
        }
        impl DivAssign for Saturating<$t> {
            fn div_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0.saturating_div(rhs.0);
            }
        }
        impl DivAssign<$t> for Saturating<$t> {
            fn div_assign(&mut self, rhs: $t) {
                self.0 = self.0.saturating_div(rhs);
            }
        }
        impl Rem for Saturating<$t> {
            type Output = Saturating<$t>;
            fn rem(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0 % rhs.0)
            }
        }
        impl RemAssign for Saturating<$t> {
            fn rem_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0 % rhs.0;
            }
        }
        impl RemAssign<$t> for Saturating<$t> {
            fn rem_assign(&mut self, rhs: $t) {
                self.0 = self.0 % rhs;
            }
        }
        impl BitXor for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitxor(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0 ^ rhs.0)
            }
        }
        impl BitXorAssign for Saturating<$t> {
            fn bitxor_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0 ^ rhs.0;
            }
        }
        impl BitXorAssign<$t> for Saturating<$t> {
            fn bitxor_assign(&mut self, rhs: $t) {
                self.0 = self.0 ^ rhs;
            }
        }
        impl BitOr for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitor(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0 | rhs.0)
            }
        }
        impl BitOrAssign for Saturating<$t> {
            fn bitor_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0 | rhs.0;
            }
        }
        impl BitOrAssign<$t> for Saturating<$t> {
            fn bitor_assign(&mut self, rhs: $t) {
                self.0 = self.0 | rhs;
            }
        }
        impl BitAnd for Saturating<$t> {
            type Output = Saturating<$t>;
            fn bitand(self, rhs: Saturating<$t>) -> Saturating<$t> {
                Saturating(self.0 & rhs.0)
            }
        }
        impl BitAndAssign for Saturating<$t> {
            fn bitand_assign(&mut self, rhs: Saturating<$t>) {
                self.0 = self.0 & rhs.0;
            }
        }
        impl BitAndAssign<$t> for Saturating<$t> {
            fn bitand_assign(&mut self, rhs: $t) {
                self.0 = self.0 & rhs;
            }
        }
        impl Not for Saturating<$t> {
            type Output = Saturating<$t>;
            fn not(self) -> Saturating<$t> {
                Saturating(!self.0)
            }
        }
    )*)
}

saturating_impl! { usize u8 u16 u32 u64 u128 isize i8 i16 i32 i64 i128 }

// rustc iter/traits/accum.rs integer_sum_product!(Wrapping<$a>) and
// saturating_integer_sum_product!(Saturating<unsigned>)
macro_rules! wrapping_sum_product {
    ($w:ident, $($t:ty)*) => ($(
        impl crate::iter::Sum for $w<$t> {
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self { iter.fold($w(0), |a, b| a + b) }
        }
        impl crate::iter::Product for $w<$t> {
            fn product<I: Iterator<Item = Self>>(iter: I) -> Self { iter.fold($w(1), |a, b| a * b) }
        }
        impl<'a> crate::iter::Sum<&'a $w<$t>> for $w<$t> {
            fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self { iter.fold($w(0), |a, b| a + *b) }
        }
        impl<'a> crate::iter::Product<&'a $w<$t>> for $w<$t> {
            fn product<I: Iterator<Item = &'a Self>>(iter: I) -> Self { iter.fold($w(1), |a, b| a * *b) }
        }
    )*)
}
wrapping_sum_product! { Wrapping, i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize }
wrapping_sum_product! { Saturating, u8 u16 u32 u64 u128 usize }
