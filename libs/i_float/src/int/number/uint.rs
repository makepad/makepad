use crate::int::number::product_uint::{CompositeUIntProduct, UIntProduct, UIntProduct64};
use core::fmt::Display;
use core::ops::{Add, AddAssign, BitAnd, BitOr, BitOrAssign, Div, Mul, Shl, ShlAssign, Shr, Sub};

pub trait UIntNumber:
    Copy
    + Clone
    + Ord
    + Send
    + Sync
    + Display
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + BitAnd<Output = Self>
    + BitOr<Output = Self>
    + BitOrAssign
    + Shl<u32, Output = Self>
    + Shr<u32, Output = Self>
    + ShlAssign
    + AddAssign
{
    type Product: UIntProduct<Self>;
    const BITS: u32;
    const HALF_BITS: u32;
    const HALF_MASK: Self;
    const LAST_BIT_INDEX: u32;
    const ZERO: Self;
    const ONE: Self;
    const MAX: Self;
    const LAST_BIT: Self;

    fn overflowing_add(self, rhs: Self) -> (Self, bool);
    fn from_u64(value: u64) -> Self;
    fn wrapping_sub(self, rhs: Self) -> Self;
    fn leading_zeros(self) -> u32;
    fn ilog2(self) -> u32;
    fn to_f64(self) -> f64;
}

impl UIntNumber for u32 {
    type Product = UIntProduct64;
    const BITS: u32 = 32;
    const HALF_BITS: u32 = 16;
    const HALF_MASK: Self = 0xFFFF;
    const LAST_BIT_INDEX: u32 = 31;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const MAX: Self = Self::MAX;
    const LAST_BIT: Self = Self::ONE << Self::LAST_BIT_INDEX;

    #[inline(always)]
    fn overflowing_add(self, rhs: Self) -> (Self, bool) {
        self.overflowing_add(rhs)
    }
    #[inline(always)]
    fn from_u64(value: u64) -> Self {
        value as Self
    }

    #[inline(always)]
    fn wrapping_sub(self, rhs: Self) -> Self {
        self.wrapping_sub(rhs)
    }

    #[inline(always)]
    fn leading_zeros(self) -> u32 {
        self.leading_zeros()
    }

    #[inline(always)]
    fn ilog2(self) -> u32 {
        self.ilog2()
    }

    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }
}

impl UIntNumber for u64 {
    type Product = CompositeUIntProduct<Self>;
    const BITS: u32 = 64;
    const HALF_BITS: u32 = 32;
    const HALF_MASK: Self = 0xFFFF_FFFF;
    const LAST_BIT_INDEX: u32 = 63;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const MAX: Self = Self::MAX;
    const LAST_BIT: Self = Self::ONE << Self::LAST_BIT_INDEX;

    #[inline(always)]
    fn overflowing_add(self, rhs: Self) -> (Self, bool) {
        self.overflowing_add(rhs)
    }

    #[inline(always)]
    fn from_u64(value: u64) -> Self {
        value
    }

    #[inline(always)]
    fn wrapping_sub(self, rhs: Self) -> Self {
        self.wrapping_sub(rhs)
    }

    #[inline(always)]
    fn leading_zeros(self) -> u32 {
        self.leading_zeros()
    }

    #[inline(always)]
    fn ilog2(self) -> u32 {
        self.ilog2()
    }

    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }
}

impl UIntNumber for u128 {
    type Product = CompositeUIntProduct<Self>;
    const BITS: u32 = 128;
    const HALF_BITS: u32 = 64;
    const HALF_MASK: Self = 0xFFFF_FFFF_FFFF_FFFF;
    const LAST_BIT_INDEX: u32 = 127;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const MAX: Self = Self::MAX;
    const LAST_BIT: Self = Self::ONE << Self::LAST_BIT_INDEX;
    #[inline(always)]
    fn overflowing_add(self, rhs: Self) -> (Self, bool) {
        self.overflowing_add(rhs)
    }

    #[inline(always)]
    fn from_u64(value: u64) -> Self {
        value as Self
    }

    #[inline(always)]
    fn wrapping_sub(self, rhs: Self) -> Self {
        self.wrapping_sub(rhs)
    }

    #[inline(always)]
    fn leading_zeros(self) -> u32 {
        self.leading_zeros()
    }

    #[inline(always)]
    fn ilog2(self) -> u32 {
        self.ilog2()
    }

    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }
}
