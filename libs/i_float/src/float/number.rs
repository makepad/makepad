use crate::int::number::int::IntNumber;
use crate::int::number::wide_int::WideIntNumber;
use core::fmt::Display;
use core::ops::{Add, Div, Mul, Neg, Sub};

pub trait FloatNumber
where
    Self: Copy
        + Mul<Output = Self>
        + Add<Output = Self>
        + Sub<Output = Self>
        + Div<Output = Self>
        + Neg<Output = Self>
        + Display
        + PartialOrd,
{
    const MAX: Self;
    const MIN: Self;
    const BITS: u32;

    // Construction.
    fn from_usize(value: usize) -> Self;
    fn from_int<I: IntNumber>(value: I) -> Self;
    fn from_wide_int<I: WideIntNumber>(value: I) -> Self;
    fn from_float<F: FloatNumber>(value: F) -> Self;

    // Math.
    fn abs(self) -> Self;
    fn sqrt(self) -> Self;
    fn max(self, other: Self) -> Self;
    fn min(self, other: Self) -> Self;
    fn log2(self) -> Self;
    fn cos(self) -> Self;
    fn sin(self) -> Self;
    fn tan(self) -> Self;
    fn sin_cos(self) -> (Self, Self);
    fn acos(self) -> Self;
    fn asin(self) -> Self;

    // Truncating casts.
    fn to_i16(self) -> i16;
    fn to_i32(self) -> i32;
    fn to_i64(self) -> i64;
    fn to_usize(self) -> usize;
    fn to_f32(self) -> f32;
    fn to_f64(self) -> f64;

    // Rounding casts.
    fn to_round_i16(self) -> i16;
    fn to_round_i32(self) -> i32;
    fn to_round_i64(self) -> i64;
    fn to_round_i128(self) -> i128;
    fn to_round_usize(self) -> usize;
}

impl FloatNumber for f32 {
    const MAX: Self = f32::MAX;
    const MIN: Self = f32::MIN;
    const BITS: u32 = 32;

    // Construction.
    #[inline(always)]
    fn from_usize(value: usize) -> Self {
        value as f32
    }

    #[inline(always)]
    fn from_int<I: IntNumber>(value: I) -> Self {
        value.to_f64() as f32
    }

    #[inline(always)]
    fn from_wide_int<I: WideIntNumber>(value: I) -> Self {
        value.to_f32()
    }

    #[inline(always)]
    fn from_float<F: FloatNumber>(value: F) -> Self {
        value.to_f32()
    }

    // Math.
    #[inline(always)]
    fn abs(self) -> Self {
        self.abs()
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        self.sqrt()
    }

    #[inline(always)]
    fn max(self, other: Self) -> Self {
        self.max(other)
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        self.min(other)
    }

    #[inline(always)]
    fn log2(self) -> Self {
        self.log2()
    }

    #[inline(always)]
    fn cos(self) -> Self {
        self.cos()
    }

    #[inline(always)]
    fn sin(self) -> Self {
        self.sin()
    }

    #[inline(always)]
    fn tan(self) -> Self {
        self.tan()
    }

    #[inline(always)]
    fn sin_cos(self) -> (Self, Self) {
        self.sin_cos()
    }

    #[inline(always)]
    fn acos(self) -> Self {
        self.acos()
    }

    #[inline(always)]
    fn asin(self) -> Self {
        self.asin()
    }

    // Truncating casts.
    #[inline(always)]
    fn to_i16(self) -> i16 {
        self as i16
    }

    #[inline(always)]
    fn to_i32(self) -> i32 {
        self as i32
    }

    #[inline(always)]
    fn to_i64(self) -> i64 {
        self as i64
    }

    #[inline(always)]
    fn to_usize(self) -> usize {
        self as usize
    }

    #[inline(always)]
    fn to_f32(self) -> f32 {
        self
    }

    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }

    // Rounding casts.
    #[inline(always)]
    fn to_round_i16(self) -> i16 {
        (self + Self::from_float(0.5).copysign(self)) as i16
    }

    #[inline(always)]
    fn to_round_i32(self) -> i32 {
        (self + Self::from_float(0.5).copysign(self)) as i32
    }

    #[inline(always)]
    fn to_round_i64(self) -> i64 {
        (self + Self::from_float(0.5).copysign(self)) as i64
    }

    #[inline(always)]
    fn to_round_i128(self) -> i128 {
        (self + Self::from_float(0.5).copysign(self)) as i128
    }

    #[inline(always)]
    fn to_round_usize(self) -> usize {
        (self + 0.5) as usize
    }
}

impl FloatNumber for f64 {
    const MAX: Self = f64::MAX;
    const MIN: Self = f64::MIN;
    const BITS: u32 = 64;

    // Construction.
    #[inline(always)]
    fn from_usize(value: usize) -> Self {
        value as f64
    }

    #[inline(always)]
    fn from_int<I: IntNumber>(value: I) -> Self {
        value.to_f64()
    }

    #[inline(always)]
    fn from_wide_int<I: WideIntNumber>(value: I) -> Self {
        value.to_f64()
    }

    #[inline(always)]
    fn from_float<F: FloatNumber>(value: F) -> Self {
        value.to_f64()
    }

    // Math.
    #[inline(always)]
    fn abs(self) -> Self {
        self.abs()
    }

    #[inline(always)]
    fn sqrt(self) -> Self {
        self.sqrt()
    }

    #[inline(always)]
    fn max(self, other: Self) -> Self {
        self.max(other)
    }

    #[inline(always)]
    fn min(self, other: Self) -> Self {
        self.min(other)
    }

    #[inline(always)]
    fn log2(self) -> Self {
        self.log2()
    }

    #[inline(always)]
    fn cos(self) -> Self {
        self.cos()
    }

    #[inline(always)]
    fn sin(self) -> Self {
        self.sin()
    }

    #[inline(always)]
    fn tan(self) -> Self {
        self.tan()
    }

    #[inline(always)]
    fn sin_cos(self) -> (Self, Self) {
        self.sin_cos()
    }

    #[inline(always)]
    fn acos(self) -> Self {
        self.acos()
    }

    #[inline(always)]
    fn asin(self) -> Self {
        self.asin()
    }

    // Truncating casts.
    #[inline(always)]
    fn to_i16(self) -> i16 {
        self as i16
    }

    #[inline(always)]
    fn to_i32(self) -> i32 {
        self as i32
    }

    #[inline(always)]
    fn to_i64(self) -> i64 {
        self as i64
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
        self
    }

    // Rounding casts.
    #[inline(always)]
    fn to_round_i16(self) -> i16 {
        (self + Self::from_float(0.5).copysign(self)) as i16
    }

    #[inline(always)]
    fn to_round_i32(self) -> i32 {
        (self + Self::from_float(0.5).copysign(self)) as i32
    }

    #[inline(always)]
    fn to_round_i64(self) -> i64 {
        (self + Self::from_float(0.5).copysign(self)) as i64
    }

    #[inline(always)]
    fn to_round_i128(self) -> i128 {
        (self + Self::from_float(0.5).copysign(self)) as i128
    }

    #[inline(always)]
    fn to_round_usize(self) -> usize {
        (self + 0.5) as usize
    }
}
