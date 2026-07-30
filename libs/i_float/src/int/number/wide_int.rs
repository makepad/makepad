use crate::float::number::FloatNumber;
use crate::int::number::uint::UIntNumber;
use core::fmt::Display;
use core::ops::{Add, BitAnd, Div, Mul, Neg, Shl, Shr, Sub};

pub trait WideIntNumber:
    Copy
    + Ord
    + Send
    + Sync
    + Display
    + Add<Output = Self>
    + BitAnd<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
    + Shl<u32, Output = Self>
    + Shr<u32, Output = Self>
{
    type UInt: UIntNumber;
    const ZERO: Self;
    const ONE: Self;
    const TWO: Self;
    const FOUR: Self;
    fn from_usize(value: usize) -> Self;
    fn from_rounded_float<F: FloatNumber>(value: F) -> Self;
    fn from_uint(value: Self::UInt) -> Self;
    fn to_uint(self) -> Self::UInt;
    fn wrapping_add(self, rhs: Self) -> Self;
    fn wrapping_sub(self, rhs: Self) -> Self;
    fn wrapping_mul(self, rhs: Self) -> Self;
    fn unsigned_abs(self) -> Self::UInt;
    fn signum(self) -> Self;
    fn ilog2(self) -> u32;
    fn to_usize(self) -> usize;
    fn to_f32(self) -> f32;
    fn to_f64(self) -> f64;
}

impl WideIntNumber for i32 {
    type UInt = u32;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const TWO: Self = 2;
    const FOUR: Self = 4;

    #[inline(always)]
    fn from_usize(value: usize) -> Self {
        value as Self
    }
    #[inline(always)]
    fn from_rounded_float<F: FloatNumber>(value: F) -> Self {
        value.to_round_i32()
    }

    #[inline(always)]
    fn from_uint(value: Self::UInt) -> Self {
        value as Self
    }

    #[inline(always)]
    fn to_uint(self) -> Self::UInt {
        self as Self::UInt
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
    fn unsigned_abs(self) -> Self::UInt {
        self.unsigned_abs()
    }

    #[inline(always)]
    fn signum(self) -> Self {
        self.signum()
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
}
impl WideIntNumber for i64 {
    type UInt = u64;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const TWO: Self = 2;
    const FOUR: Self = 4;

    #[inline(always)]
    fn from_usize(value: usize) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_rounded_float<F: FloatNumber>(value: F) -> Self {
        value.to_round_i64()
    }

    #[inline(always)]
    fn from_uint(value: Self::UInt) -> Self {
        value as Self
    }

    #[inline(always)]
    fn to_uint(self) -> Self::UInt {
        self as Self::UInt
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
    fn unsigned_abs(self) -> Self::UInt {
        self.unsigned_abs()
    }
    #[inline(always)]
    fn signum(self) -> Self {
        self.signum()
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
}
impl WideIntNumber for i128 {
    type UInt = u128;
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const TWO: Self = 2;
    const FOUR: Self = 4;

    #[inline(always)]
    fn from_usize(value: usize) -> Self {
        value as Self
    }

    #[inline(always)]
    fn from_rounded_float<F: FloatNumber>(value: F) -> Self {
        value.to_round_i128()
    }

    #[inline(always)]
    fn from_uint(value: Self::UInt) -> Self {
        value as Self
    }

    #[inline(always)]
    fn to_uint(self) -> Self::UInt {
        self as Self::UInt
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
    fn unsigned_abs(self) -> Self::UInt {
        self.unsigned_abs()
    }
    #[inline(always)]
    fn signum(self) -> Self {
        self.signum()
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
}
