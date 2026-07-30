use crate::float::number::FloatNumber;
use crate::int::number::uint::UIntNumber;
use crate::int::number::wide_int::WideIntNumber;
use core::fmt::{Binary, Display};
use core::ops::{Add, AddAssign, Div, Mul, Neg, Shl, Shr, Sub};

pub trait IntNumber
where
    Self: Copy
        + Mul<Output = Self>
        + Add<Output = Self>
        + Sub<Output = Self>
        + Div<Output = Self>
        + Neg<Output = Self>
        + AddAssign
        + Shl<u32, Output = Self>
        + Shr<u32, Output = Self>
        + Binary
        + Display
        + Ord
        + Send
        + Sync
        + Default,
{
    type WideUInt: UIntNumber;
    type Wide: WideIntNumber<UInt = Self::WideUInt>;
    const BITS: u32;
    const MAX: Self;
    const MIN: Self;
    const ZERO: Self;
    const ONE: Self;
    const TWO: Self;
    const FOUR: Self;
    fn wide(self) -> Self::Wide;
    fn from_wide(value: Self::Wide) -> Self;
    fn from_uint(value: Self::WideUInt) -> Self;
    fn from_usize(value: usize) -> Self;
    fn from_float<F: FloatNumber>(value: F) -> Self;
    fn from_rounded_float<F: FloatNumber>(value: F) -> Self;
    fn wrapping_add(self, rhs: Self) -> Self;
    fn wrapping_sub(self, rhs: Self) -> Self;
    fn wrapping_mul(self, rhs: Self) -> Self;
    fn ilog2(self) -> u32;
    fn to_usize(self) -> usize;
    fn to_f32(self) -> f32;
    fn to_f64(self) -> f64;
    fn to_uint(self) -> Self::WideUInt;
}

impl IntNumber for i16 {
    type WideUInt = u32;
    type Wide = i32;
    const BITS: u32 = i16::BITS;
    const MAX: Self = Self::MAX;
    const MIN: Self = Self::MIN;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const TWO: Self = 2;
    const FOUR: Self = 4;
    #[inline(always)]
    fn wide(self) -> Self::Wide {
        self as Self::Wide
    }

    #[inline(always)]
    fn from_wide(value: Self::Wide) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_uint(value: Self::WideUInt) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_usize(value: usize) -> Self {
        value as Self
    }
    #[inline(always)]
    fn from_float<F: FloatNumber>(value: F) -> Self {
        value.to_i16()
    }

    #[inline(always)]
    fn from_rounded_float<F: FloatNumber>(value: F) -> Self {
        value.to_round_i16()
    }

    #[inline(always)]
    fn wrapping_add(self, rhs: Self) -> Self {
        self.wrapping_add(rhs)
    }

    #[inline(always)]
    fn wrapping_sub(self, rhs: Self) -> Self {
        self.wrapping_sub(rhs)
    }

    #[inline(always)]
    fn wrapping_mul(self, rhs: Self) -> Self {
        self.wrapping_mul(rhs)
    }
    #[inline(always)]
    fn ilog2(self) -> u32 {
        self.ilog2()
    }
    #[inline(always)]
    fn to_usize(self) -> usize {
        self as usize
    }
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }

    #[inline(always)]
    fn to_uint(self) -> Self::WideUInt {
        self as Self::WideUInt
    }
}

impl IntNumber for i32 {
    type WideUInt = u64;
    type Wide = i64;
    const BITS: u32 = i32::BITS;
    const MAX: Self = Self::MAX;
    const MIN: Self = Self::MIN;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const TWO: Self = 2;
    const FOUR: Self = 4;
    #[inline(always)]
    fn wide(self) -> Self::Wide {
        self as Self::Wide
    }

    #[inline(always)]
    fn from_wide(value: Self::Wide) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_uint(value: Self::WideUInt) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_usize(value: usize) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_float<F: FloatNumber>(value: F) -> Self {
        value.to_i32()
    }

    #[inline(always)]
    fn from_rounded_float<F: FloatNumber>(value: F) -> Self {
        value.to_round_i32()
    }

    #[inline(always)]
    fn wrapping_add(self, rhs: Self) -> Self {
        self.wrapping_add(rhs)
    }

    #[inline(always)]
    fn wrapping_sub(self, rhs: Self) -> Self {
        self.wrapping_sub(rhs)
    }

    #[inline(always)]
    fn wrapping_mul(self, rhs: Self) -> Self {
        self.wrapping_mul(rhs)
    }

    #[inline(always)]
    fn ilog2(self) -> u32 {
        self.ilog2()
    }

    #[inline(always)]
    fn to_usize(self) -> usize {
        self as usize
    }
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }

    #[inline(always)]
    fn to_uint(self) -> Self::WideUInt {
        self as Self::WideUInt
    }
}

impl IntNumber for i64 {
    type WideUInt = u128;
    type Wide = i128;
    const BITS: u32 = i64::BITS;
    const MAX: Self = Self::MAX;
    const MIN: Self = Self::MIN;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const TWO: Self = 2;
    const FOUR: Self = 4;
    #[inline(always)]
    fn wide(self) -> Self::Wide {
        self as Self::Wide
    }

    #[inline(always)]
    fn from_wide(value: Self::Wide) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_uint(value: Self::WideUInt) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_usize(value: usize) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_float<F: FloatNumber>(value: F) -> Self {
        value.to_i64()
    }

    #[inline(always)]
    fn from_rounded_float<F: FloatNumber>(value: F) -> Self {
        value.to_round_i64()
    }

    #[inline(always)]
    fn wrapping_add(self, rhs: Self) -> Self {
        self.wrapping_add(rhs)
    }

    #[inline(always)]
    fn wrapping_sub(self, rhs: Self) -> Self {
        self.wrapping_sub(rhs)
    }

    #[inline(always)]
    fn wrapping_mul(self, rhs: Self) -> Self {
        self.wrapping_mul(rhs)
    }

    #[inline(always)]
    fn ilog2(self) -> u32 {
        self.ilog2()
    }
    #[inline(always)]
    fn to_usize(self) -> usize {
        self as usize
    }
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }
    #[inline(always)]
    fn to_uint(self) -> Self::WideUInt {
        self as Self::WideUInt
    }
}
