//! Wrapping<T> and Saturating<T>. Integer `+ - * << >>` and unary `-` already wrap in Relax
//! (release semantics, as rustc --release), so Wrapping's operators forward to them.

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

impl<T: Add<Output = T>> Add for Wrapping<T> {
    type Output = Wrapping<T>;
    fn add(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 + rhs.0)
    }
}
impl<T: Sub<Output = T>> Sub for Wrapping<T> {
    type Output = Wrapping<T>;
    fn sub(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 - rhs.0)
    }
}
impl<T: Mul<Output = T>> Mul for Wrapping<T> {
    type Output = Wrapping<T>;
    fn mul(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 * rhs.0)
    }
}
impl<T: BitXor<Output = T>> BitXor for Wrapping<T> {
    type Output = Wrapping<T>;
    fn bitxor(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 ^ rhs.0)
    }
}
impl<T: BitOr<Output = T>> BitOr for Wrapping<T> {
    type Output = Wrapping<T>;
    fn bitor(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 | rhs.0)
    }
}
impl<T: BitAnd<Output = T>> BitAnd for Wrapping<T> {
    type Output = Wrapping<T>;
    fn bitand(self, rhs: Wrapping<T>) -> Wrapping<T> {
        Wrapping(self.0 & rhs.0)
    }
}
impl<T: Shl<usize, Output = T>> Shl<usize> for Wrapping<T> {
    type Output = Wrapping<T>;
    fn shl(self, rhs: usize) -> Wrapping<T> {
        Wrapping(self.0 << rhs)
    }
}
impl<T: Shr<usize, Output = T>> Shr<usize> for Wrapping<T> {
    type Output = Wrapping<T>;
    fn shr(self, rhs: usize) -> Wrapping<T> {
        Wrapping(self.0 >> rhs)
    }
}
impl<T: Not<Output = T>> Not for Wrapping<T> {
    type Output = Wrapping<T>;
    fn not(self) -> Wrapping<T> {
        Wrapping(!self.0)
    }
}
impl<T: Add<Output = T> + Copy> AddAssign for Wrapping<T> {
    fn add_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 + rhs.0;
    }
}
impl<T: Sub<Output = T> + Copy> SubAssign for Wrapping<T> {
    fn sub_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 - rhs.0;
    }
}
impl<T: Mul<Output = T> + Copy> MulAssign for Wrapping<T> {
    fn mul_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 * rhs.0;
    }
}
impl<T: BitXor<Output = T> + Copy> BitXorAssign for Wrapping<T> {
    fn bitxor_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 ^ rhs.0;
    }
}
impl<T: BitOr<Output = T> + Copy> BitOrAssign for Wrapping<T> {
    fn bitor_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 | rhs.0;
    }
}
impl<T: BitAnd<Output = T> + Copy> BitAndAssign for Wrapping<T> {
    fn bitand_assign(&mut self, rhs: Wrapping<T>) {
        self.0 = self.0 & rhs.0;
    }
}
// The by-reference forms real core gets from forward_ref_binop!, forward_ref_op_assign! and
// forward_ref_unop!: `a + &b`, `&a + b`, `&a + &b`, `a += &b`, `-&a`, `!&a`.
macro_rules! wrapping_ref_ops {
    ($($Op:ident $op:ident $OpAssign:ident $op_assign:ident;)*) => {$(
        impl<'a, T: $Op<Output = T> + Copy> $Op<&'a Wrapping<T>> for Wrapping<T> {
            type Output = Wrapping<T>;
            fn $op(self, rhs: &'a Wrapping<T>) -> Wrapping<T> {
                Wrapping($Op::$op(self.0, rhs.0))
            }
        }
        impl<'a, T: $Op<Output = T> + Copy> $Op<Wrapping<T>> for &'a Wrapping<T> {
            type Output = Wrapping<T>;
            fn $op(self, rhs: Wrapping<T>) -> Wrapping<T> {
                Wrapping($Op::$op(self.0, rhs.0))
            }
        }
        impl<'a, 'b, T: $Op<Output = T> + Copy> $Op<&'b Wrapping<T>> for &'a Wrapping<T> {
            type Output = Wrapping<T>;
            fn $op(self, rhs: &'b Wrapping<T>) -> Wrapping<T> {
                Wrapping($Op::$op(self.0, rhs.0))
            }
        }
        impl<'a, T: $Op<Output = T> + Copy> $OpAssign<&'a Wrapping<T>> for Wrapping<T> {
            fn $op_assign(&mut self, rhs: &'a Wrapping<T>) {
                self.0 = $Op::$op(self.0, rhs.0);
            }
        }
    )*};
}
wrapping_ref_ops! {
    Add add AddAssign add_assign; Sub sub SubAssign sub_assign; Mul mul MulAssign mul_assign;
    BitXor bitxor BitXorAssign bitxor_assign;
    BitOr bitor BitOrAssign bitor_assign; BitAnd bitand BitAndAssign bitand_assign;
}
// Division and remainder wrap as well: `MIN / -1` is `MIN` and `MIN % -1` is 0 where the plain
// operators overflow, so these go through wrapping_div and wrapping_rem per integer type, in
// every by-value and by-reference form.
macro_rules! wrapping_div_rem {
    ($($t:ty)*) => {$(
        wrapping_div_rem! { @ops $t; Div div DivAssign div_assign wrapping_div; Rem rem RemAssign rem_assign wrapping_rem; }
    )*};
    (@ops $t:ty; $($Op:ident $op:ident $OpAssign:ident $op_assign:ident $wrapping:ident;)*) => {$(
        impl $Op for Wrapping<$t> {
            type Output = Wrapping<$t>;
            fn $op(self, rhs: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.$wrapping(rhs.0))
            }
        }
        impl<'a> $Op<&'a Wrapping<$t>> for Wrapping<$t> {
            type Output = Wrapping<$t>;
            fn $op(self, rhs: &'a Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.$wrapping(rhs.0))
            }
        }
        impl<'a> $Op<Wrapping<$t>> for &'a Wrapping<$t> {
            type Output = Wrapping<$t>;
            fn $op(self, rhs: Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.$wrapping(rhs.0))
            }
        }
        impl<'a, 'b> $Op<&'b Wrapping<$t>> for &'a Wrapping<$t> {
            type Output = Wrapping<$t>;
            fn $op(self, rhs: &'b Wrapping<$t>) -> Wrapping<$t> {
                Wrapping(self.0.$wrapping(rhs.0))
            }
        }
        impl $OpAssign for Wrapping<$t> {
            fn $op_assign(&mut self, rhs: Wrapping<$t>) {
                self.0 = self.0.$wrapping(rhs.0);
            }
        }
        impl<'a> $OpAssign<&'a Wrapping<$t>> for Wrapping<$t> {
            fn $op_assign(&mut self, rhs: &'a Wrapping<$t>) {
                self.0 = self.0.$wrapping(rhs.0);
            }
        }
    )*};
}
wrapping_div_rem! { usize u8 u16 u32 u64 u128 isize i8 i16 i32 i64 i128 }
// `x op= int` and `x op= &int` with the plain integer on the right, which real core has for
// every integer type (wrapping_int_assign_impl!, stable since 1.60).
macro_rules! wrapping_int_assign {
    ($($t:ty)*) => {$(
        wrapping_int_assign! { @ops $t;
            AddAssign add_assign; SubAssign sub_assign; MulAssign mul_assign; DivAssign div_assign;
            RemAssign rem_assign; BitXorAssign bitxor_assign; BitOrAssign bitor_assign;
            BitAndAssign bitand_assign;
        }
    )*};
    (@ops $t:ty; $($OpAssign:ident $op_assign:ident;)*) => {$(
        impl $OpAssign<$t> for Wrapping<$t> {
            fn $op_assign(&mut self, rhs: $t) {
                <Wrapping<$t> as $OpAssign<Wrapping<$t>>>::$op_assign(self, Wrapping(rhs))
            }
        }
        impl<'a> $OpAssign<&'a $t> for Wrapping<$t> {
            fn $op_assign(&mut self, rhs: &'a $t) {
                <Wrapping<$t> as $OpAssign<Wrapping<$t>>>::$op_assign(self, Wrapping(*rhs))
            }
        }
    )*};
}
wrapping_int_assign! { usize u8 u16 u32 u64 u128 isize i8 i16 i32 i64 i128 }
// Unary minus is `0 - x` for every integer type, the unsigned ones too, as in real core.
macro_rules! wrapping_neg {
    ($($t:ty)*) => {$(
        impl Neg for Wrapping<$t> {
            type Output = Wrapping<$t>;
            fn neg(self) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_neg())
            }
        }
        impl<'a> Neg for &'a Wrapping<$t> {
            type Output = Wrapping<$t>;
            fn neg(self) -> Wrapping<$t> {
                Wrapping(self.0.wrapping_neg())
            }
        }
    )*};
}
wrapping_neg! { usize u8 u16 u32 u64 u128 isize i8 i16 i32 i64 i128 }
impl<'a, T: Not<Output = T> + Copy> Not for &'a Wrapping<T> {
    type Output = Wrapping<T>;
    fn not(self) -> Wrapping<T> {
        Wrapping(!self.0)
    }
}
impl<T: Shl<usize, Output = T> + Copy> ShlAssign<usize> for Wrapping<T> {
    fn shl_assign(&mut self, rhs: usize) {
        self.0 = self.0 << rhs;
    }
}
impl<T: Shr<usize, Output = T> + Copy> ShrAssign<usize> for Wrapping<T> {
    fn shr_assign(&mut self, rhs: usize) {
        self.0 = self.0 >> rhs;
    }
}
// The shifts with the value or the count by reference (forward_ref_binop! and
// forward_ref_op_assign! in real core): `a << &n`, `&a << n`, `&a << &n`, `a <<= &n`.
macro_rules! wrapping_ref_shifts {
    ($($Op:ident $op:ident $OpAssign:ident $op_assign:ident;)*) => {$(
        impl<'a, T: $Op<usize, Output = T>> $Op<&'a usize> for Wrapping<T> {
            type Output = Wrapping<T>;
            fn $op(self, rhs: &'a usize) -> Wrapping<T> {
                <Wrapping<T> as $Op<usize>>::$op(self, *rhs)
            }
        }
        impl<'a, T: $Op<usize, Output = T> + Copy> $Op<usize> for &'a Wrapping<T> {
            type Output = Wrapping<T>;
            fn $op(self, rhs: usize) -> Wrapping<T> {
                <Wrapping<T> as $Op<usize>>::$op(*self, rhs)
            }
        }
        impl<'a, 'b, T: $Op<usize, Output = T> + Copy> $Op<&'b usize> for &'a Wrapping<T> {
            type Output = Wrapping<T>;
            fn $op(self, rhs: &'b usize) -> Wrapping<T> {
                <Wrapping<T> as $Op<usize>>::$op(*self, *rhs)
            }
        }
        impl<'a, T: $Op<usize, Output = T> + Copy> $OpAssign<&'a usize> for Wrapping<T> {
            fn $op_assign(&mut self, rhs: &'a usize) {
                <Wrapping<T> as $OpAssign<usize>>::$op_assign(self, *rhs)
            }
        }
    )*};
}
wrapping_ref_shifts! { Shl shl ShlAssign shl_assign; Shr shr ShrAssign shr_assign; }

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
