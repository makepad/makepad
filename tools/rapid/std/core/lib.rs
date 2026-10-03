//! Rapid's core: the part of Rust's core library makepad uses, written in the
//! Rapid subset. Compiled by Rapid only (never by rustc): it may define inherent
//! impls on primitive types and declare compiler intrinsics.

pub mod intrinsics {
    // Calls to these become single machine instructions.
    extern "rapid-intrinsic" {
        pub fn sqrt_f64(x: f64) -> f64;
        pub fn sqrt_f32(x: f32) -> f32;
        pub fn floor_f64(x: f64) -> f64;
        pub fn floor_f32(x: f32) -> f32;
        pub fn ceil_f64(x: f64) -> f64;
        pub fn ceil_f32(x: f32) -> f32;
        pub fn trunc_f64(x: f64) -> f64;
        pub fn trunc_f32(x: f32) -> f32;
        pub fn rint_f64(x: f64) -> f64;
        pub fn rint_f32(x: f32) -> f32;
        pub fn fabs_f64(x: f64) -> f64;
        pub fn fmin_f64(x: f64, y: f64) -> f64;
        pub fn fmax_f64(x: f64, y: f64) -> f64;
        pub fn fmin_f32(x: f32, y: f32) -> f32;
        pub fn fmax_f32(x: f32, y: f32) -> f32;
        pub fn fabs_f32(x: f32) -> f32;
        pub fn to_bits_f64(x: f64) -> u64;
        pub fn to_bits_f32(x: f32) -> u32;
        pub fn from_bits_f64(x: u64) -> f64;
        pub fn from_bits_f32(x: u32) -> f32;
    }
}

pub mod libm {
    // The platform math library (what Rust's std calls for transcendentals).
    extern "C" {
        pub fn sin(x: f64) -> f64;
        pub fn cos(x: f64) -> f64;
        pub fn tan(x: f64) -> f64;
        pub fn asin(x: f64) -> f64;
        pub fn acos(x: f64) -> f64;
        pub fn atan(x: f64) -> f64;
        pub fn atan2(y: f64, x: f64) -> f64;
        pub fn exp(x: f64) -> f64;
        pub fn log(x: f64) -> f64;
        pub fn pow(x: f64, y: f64) -> f64;
        pub fn fma(x: f64, y: f64, z: f64) -> f64;
        pub fn fmaf(x: f32, y: f32, z: f32) -> f32;
        pub fn sinf(x: f32) -> f32;
        pub fn cosf(x: f32) -> f32;
        pub fn tanf(x: f32) -> f32;
        pub fn asinf(x: f32) -> f32;
        pub fn acosf(x: f32) -> f32;
        pub fn atanf(x: f32) -> f32;
        pub fn atan2f(y: f32, x: f32) -> f32;
        pub fn expf(x: f32) -> f32;
        pub fn logf(x: f32) -> f32;
        pub fn powf(x: f32, y: f32) -> f32;
    }
}

pub mod mem;
pub mod option;
pub mod boxed;
pub mod vec;
pub mod string;
pub mod char_utf8;
pub mod iter;
pub mod fmt;
pub mod slice;

pub mod clone {
    pub trait Clone {
        fn clone(&self) -> Self;
    }
}

pub mod default {
    pub trait Default {
        fn default() -> Self;
    }
}

pub mod cmp {
    pub trait PartialEq<Rhs = Self> {
        fn eq(&self, other: &Rhs) -> bool;
        fn ne(&self, other: &Rhs) -> bool {
            !self.eq(other)
        }
    }
    pub trait Eq {}
}

pub mod marker {
    pub trait Copy {}
    pub trait Send {}
    pub trait Sync {}
    pub trait Sized {}
}

pub mod ops {
    pub trait AddAssign<Rhs = Self> {
        fn add_assign(&mut self, rhs: Rhs);
    }
    pub trait SubAssign<Rhs = Self> {
        fn sub_assign(&mut self, rhs: Rhs);
    }
    pub trait MulAssign<Rhs = Self> {
        fn mul_assign(&mut self, rhs: Rhs);
    }
    pub trait DivAssign<Rhs = Self> {
        fn div_assign(&mut self, rhs: Rhs);
    }
    pub trait RemAssign<Rhs = Self> {
        fn rem_assign(&mut self, rhs: Rhs);
    }
    pub trait BitAndAssign<Rhs = Self> {
        fn bitand_assign(&mut self, rhs: Rhs);
    }
    pub trait BitOrAssign<Rhs = Self> {
        fn bitor_assign(&mut self, rhs: Rhs);
    }
    pub trait BitXorAssign<Rhs = Self> {
        fn bitxor_assign(&mut self, rhs: Rhs);
    }
    pub trait ShlAssign<Rhs = Self> {
        fn shl_assign(&mut self, rhs: Rhs);
    }
    pub trait ShrAssign<Rhs = Self> {
        fn shr_assign(&mut self, rhs: Rhs);
    }
    pub trait FnOnce<Args> {
        type Output;
        fn call_once(self, args: Args) -> Self::Output;
    }
    pub trait FnMut<Args>: FnOnce<Args> {
        fn call_mut(&mut self, args: Args) -> Self::Output;
    }
    pub trait Fn<Args>: FnMut<Args> {
        fn call(&self, args: Args) -> Self::Output;
    }
    pub trait Drop {
        fn drop(&mut self);
    }
    pub trait Deref {
        type Target;
        fn deref(&self) -> &Self::Target;
    }
    pub trait DerefMut: Deref {
        fn deref_mut(&mut self) -> &mut Self::Target;
    }
    pub trait Index<Idx> {
        type Output;
        fn index(&self, index: Idx) -> &Self::Output;
    }
    pub trait IndexMut<Idx>: Index<Idx> {
        fn index_mut(&mut self, index: Idx) -> &mut Self::Output;
    }
    pub trait Add<Rhs = Self> {
        type Output;
        fn add(self, rhs: Rhs) -> Self::Output;
    }
    pub trait Sub<Rhs = Self> {
        type Output;
        fn sub(self, rhs: Rhs) -> Self::Output;
    }
    pub trait Mul<Rhs = Self> {
        type Output;
        fn mul(self, rhs: Rhs) -> Self::Output;
    }
    pub trait Div<Rhs = Self> {
        type Output;
        fn div(self, rhs: Rhs) -> Self::Output;
    }
    pub trait Rem<Rhs = Self> {
        type Output;
        fn rem(self, rhs: Rhs) -> Self::Output;
    }
    pub trait Neg {
        type Output;
        fn neg(self) -> Self::Output;
    }
    pub trait BitAnd<Rhs = Self> {
        type Output;
        fn bitand(self, rhs: Rhs) -> Self::Output;
    }
    pub trait BitOr<Rhs = Self> {
        type Output;
        fn bitor(self, rhs: Rhs) -> Self::Output;
    }
    pub trait BitXor<Rhs = Self> {
        type Output;
        fn bitxor(self, rhs: Rhs) -> Self::Output;
    }
    pub trait Shl<Rhs = Self> {
        type Output;
        fn shl(self, rhs: Rhs) -> Self::Output;
    }
    pub trait Shr<Rhs = Self> {
        type Output;
        fn shr(self, rhs: Rhs) -> Self::Output;
    }

    pub struct Range<Idx> {
        pub start: Idx,
        pub end: Idx,
    }
    pub struct RangeInclusive<Idx> {
        pub start: Idx,
        pub end: Idx,
        pub exhausted: bool,
    }
    pub struct RangeFrom<Idx> {
        pub start: Idx,
    }
    pub struct RangeTo<Idx> {
        pub end: Idx,
    }
    pub struct RangeToInclusive<Idx> {
        pub end: Idx,
    }
    pub struct RangeFull;

    impl<Idx> Range<Idx> {
        pub fn contains(&self, item: &Idx) -> bool {
            *item >= self.start && *item < self.end
        }
    }
    impl<Idx> RangeInclusive<Idx> {
        pub fn contains(&self, item: &Idx) -> bool {
            *item >= self.start && *item <= self.end
        }
    }
}

pub mod f64 {
    pub mod consts {
        pub const PI: f64 = 3.14159265358979323846264338327950288_f64;
        pub const TAU: f64 = 6.28318530717958647692528676655900577_f64;
        pub const FRAC_PI_2: f64 = 1.57079632679489661923132169163975144_f64;
        pub const FRAC_PI_4: f64 = 0.785398163397448309615660845819875721_f64;
        pub const SQRT_2: f64 = 1.41421356237309504880168872420969808_f64;
        pub const LN_2: f64 = 0.693147180559945309417232121458176568_f64;
        pub const E: f64 = 2.71828182845904523536028747135266250_f64;
    }
}

pub mod f32 {
    pub mod consts {
        pub const PI: f32 = 3.14159265358979323846264338327950288_f32;
        pub const TAU: f32 = 6.28318530717958647692528676655900577_f32;
        pub const FRAC_PI_2: f32 = 1.57079632679489661923132169163975144_f32;
        pub const FRAC_PI_4: f32 = 0.785398163397448309615660845819875721_f32;
        pub const SQRT_2: f32 = 1.41421356237309504880168872420969808_f32;
        pub const LN_2: f32 = 0.693147180559945309417232121458176568_f32;
        pub const E: f32 = 2.71828182845904523536028747135266250_f32;
    }
}

mod num_impls {
    use crate::intrinsics;
    use crate::libm;

    impl f64 {
        pub const NAN: f64 = f64::from_bits(0x7ff8_0000_0000_0000);
        pub const INFINITY: f64 = f64::from_bits(0x7ff0_0000_0000_0000);
        pub const NEG_INFINITY: f64 = f64::from_bits(0xfff0_0000_0000_0000);
        pub const EPSILON: f64 = 2.2204460492503131e-16_f64;
        pub const MAX: f64 = 1.7976931348623157e308_f64;
        pub const MIN: f64 = -1.7976931348623157e308_f64;

        pub fn to_bits(self) -> u64 {
            unsafe { intrinsics::to_bits_f64(self) }
        }
        pub fn from_bits(v: u64) -> f64 {
            unsafe { intrinsics::from_bits_f64(v) }
        }
        pub fn is_nan(self) -> bool {
            self != self
        }
        pub fn is_infinite(self) -> bool {
            self == f64::INFINITY || self == f64::NEG_INFINITY
        }
        pub fn is_finite(self) -> bool {
            (self.to_bits() & 0x7ff0_0000_0000_0000) != 0x7ff0_0000_0000_0000
        }
        pub fn is_sign_negative(self) -> bool {
            (self.to_bits() >> 63) != 0
        }
        pub fn is_sign_positive(self) -> bool {
            (self.to_bits() >> 63) == 0
        }
        pub fn abs(self) -> f64 {
            unsafe { intrinsics::fabs_f64(self) }
        }
        pub fn sqrt(self) -> f64 {
            unsafe { intrinsics::sqrt_f64(self) }
        }
        pub fn floor(self) -> f64 {
            unsafe { intrinsics::floor_f64(self) }
        }
        pub fn ceil(self) -> f64 {
            unsafe { intrinsics::ceil_f64(self) }
        }
        pub fn trunc(self) -> f64 {
            unsafe { intrinsics::trunc_f64(self) }
        }
        pub fn round(self) -> f64 {
            // half away from zero
            let t = self.trunc();
            let d = self - t;
            if d >= 0.5 {
                t + 1.0
            } else if d <= -0.5 {
                t - 1.0
            } else {
                t
            }
        }
        pub fn fract(self) -> f64 {
            self - self.trunc()
        }
        pub fn min(self, other: f64) -> f64 {
            unsafe { intrinsics::fmin_f64(self, other) }
        }
        pub fn max(self, other: f64) -> f64 {
            unsafe { intrinsics::fmax_f64(self, other) }
        }
        pub fn mul_add(self, a: f64, b: f64) -> f64 {
            unsafe { libm::fma(self, a, b) }
        }
        pub fn signum(self) -> f64 {
            if self.is_nan() {
                f64::NAN
            } else if self.is_sign_negative() {
                -1.0
            } else {
                1.0
            }
        }
        pub fn sin(self) -> f64 {
            unsafe { libm::sin(self) }
        }
        pub fn cos(self) -> f64 {
            unsafe { libm::cos(self) }
        }
        pub fn tan(self) -> f64 {
            unsafe { libm::tan(self) }
        }
        pub fn asin(self) -> f64 {
            unsafe { libm::asin(self) }
        }
        pub fn acos(self) -> f64 {
            unsafe { libm::acos(self) }
        }
        pub fn atan(self) -> f64 {
            unsafe { libm::atan(self) }
        }
        pub fn atan2(self, x: f64) -> f64 {
            unsafe { libm::atan2(self, x) }
        }
        pub fn exp(self) -> f64 {
            unsafe { libm::exp(self) }
        }
        pub fn ln(self) -> f64 {
            unsafe { libm::log(self) }
        }
        pub fn powf(self, y: f64) -> f64 {
            unsafe { libm::pow(self, y) }
        }
    }

    impl f32 {
        pub const NAN: f32 = f32::from_bits(0x7fc0_0000);
        pub const INFINITY: f32 = f32::from_bits(0x7f80_0000);
        pub const NEG_INFINITY: f32 = f32::from_bits(0xff80_0000);
        pub const EPSILON: f32 = 1.19209290e-07_f32;
        pub const MAX: f32 = 3.40282347e+38_f32;
        pub const MIN: f32 = -3.40282347e+38_f32;

        pub fn to_bits(self) -> u32 {
            unsafe { intrinsics::to_bits_f32(self) }
        }
        pub fn from_bits(v: u32) -> f32 {
            unsafe { intrinsics::from_bits_f32(v) }
        }
        pub fn is_nan(self) -> bool {
            self != self
        }
        pub fn is_infinite(self) -> bool {
            self == f32::INFINITY || self == f32::NEG_INFINITY
        }
        pub fn is_finite(self) -> bool {
            (self.to_bits() & 0x7f80_0000) != 0x7f80_0000
        }
        pub fn is_sign_negative(self) -> bool {
            (self.to_bits() >> 31) != 0
        }
        pub fn is_sign_positive(self) -> bool {
            (self.to_bits() >> 31) == 0
        }
        pub fn abs(self) -> f32 {
            unsafe { intrinsics::fabs_f32(self) }
        }
        pub fn sqrt(self) -> f32 {
            unsafe { intrinsics::sqrt_f32(self) }
        }
        pub fn floor(self) -> f32 {
            unsafe { intrinsics::floor_f32(self) }
        }
        pub fn ceil(self) -> f32 {
            unsafe { intrinsics::ceil_f32(self) }
        }
        pub fn trunc(self) -> f32 {
            unsafe { intrinsics::trunc_f32(self) }
        }
        pub fn round(self) -> f32 {
            let t = self.trunc();
            let d = self - t;
            if d >= 0.5 {
                t + 1.0
            } else if d <= -0.5 {
                t - 1.0
            } else {
                t
            }
        }
        pub fn fract(self) -> f32 {
            self - self.trunc()
        }
        pub fn min(self, other: f32) -> f32 {
            unsafe { intrinsics::fmin_f32(self, other) }
        }
        pub fn max(self, other: f32) -> f32 {
            unsafe { intrinsics::fmax_f32(self, other) }
        }
        pub fn mul_add(self, a: f32, b: f32) -> f32 {
            unsafe { libm::fmaf(self, a, b) }
        }
        pub fn signum(self) -> f32 {
            if self.is_nan() {
                f32::NAN
            } else if self.is_sign_negative() {
                -1.0
            } else {
                1.0
            }
        }
        pub fn sin(self) -> f32 {
            unsafe { libm::sinf(self) }
        }
        pub fn cos(self) -> f32 {
            unsafe { libm::cosf(self) }
        }
        pub fn tan(self) -> f32 {
            unsafe { libm::tanf(self) }
        }
        pub fn asin(self) -> f32 {
            unsafe { libm::asinf(self) }
        }
        pub fn acos(self) -> f32 {
            unsafe { libm::acosf(self) }
        }
        pub fn atan(self) -> f32 {
            unsafe { libm::atanf(self) }
        }
        pub fn atan2(self, x: f32) -> f32 {
            unsafe { libm::atan2f(self, x) }
        }
        pub fn exp(self) -> f32 {
            unsafe { libm::expf(self) }
        }
        pub fn ln(self) -> f32 {
            unsafe { libm::logf(self) }
        }
        pub fn powf(self, y: f32) -> f32 {
            unsafe { libm::powf(self, y) }
        }
    }

    impl u64 {
        pub const MAX: u64 = 0xffff_ffff_ffff_ffff;
        pub const MIN: u64 = 0;
        pub fn wrapping_mul(self, rhs: u64) -> u64 {
            self * rhs
        }
        pub fn wrapping_add(self, rhs: u64) -> u64 {
            self + rhs
        }
        pub fn wrapping_sub(self, rhs: u64) -> u64 {
            self - rhs
        }
        pub fn overflowing_add(self, rhs: u64) -> (u64, bool) {
            let r = self + rhs;
            (r, r < self)
        }
        pub fn overflowing_mul(self, rhs: u64) -> (u64, bool) {
            let r = self * rhs;
            (r, self != 0 && r / self != rhs)
        }
        pub fn to_be_bytes(self) -> [u8; 8] {
            [
                (self >> 56) as u8,
                (self >> 48) as u8,
                (self >> 40) as u8,
                (self >> 32) as u8,
                (self >> 24) as u8,
                (self >> 16) as u8,
                (self >> 8) as u8,
                self as u8,
            ]
        }
        pub fn rotate_left(self, n: u32) -> u64 {
            let n = n & 63;
            if n == 0 {
                self
            } else {
                (self << n) | (self >> (64 - n))
            }
        }
    }

    impl u32 {
        pub const MAX: u32 = 0xffff_ffff;
        pub const MIN: u32 = 0;
        pub fn wrapping_mul(self, rhs: u32) -> u32 {
            self * rhs
        }
        pub fn wrapping_add(self, rhs: u32) -> u32 {
            self + rhs
        }
        pub fn wrapping_sub(self, rhs: u32) -> u32 {
            self - rhs
        }
    }

    impl i64 {
        pub const MAX: i64 = 0x7fff_ffff_ffff_ffff;
        pub const MIN: i64 = -0x7fff_ffff_ffff_ffff - 1;
        pub fn wrapping_mul(self, rhs: i64) -> i64 {
            self * rhs
        }
        pub fn wrapping_add(self, rhs: i64) -> i64 {
            self + rhs
        }
        pub fn abs(self) -> i64 {
            if self < 0 {
                -self
            } else {
                self
            }
        }
    }

    impl i32 {
        pub const MAX: i32 = 0x7fff_ffff;
        pub const MIN: i32 = -0x7fff_ffff - 1;
        pub fn wrapping_mul(self, rhs: i32) -> i32 {
            self * rhs
        }
        pub fn wrapping_add(self, rhs: i32) -> i32 {
            self + rhs
        }
        pub fn abs(self) -> i32 {
            if self < 0 {
                -self
            } else {
                self
            }
        }
    }

    impl usize {
        pub const MAX: usize = 0xffff_ffff_ffff_ffff;
        pub fn wrapping_add(self, rhs: usize) -> usize {
            self + rhs
        }
        pub fn wrapping_sub(self, rhs: usize) -> usize {
            self - rhs
        }
    }

    impl str {
        pub fn len(&self) -> usize {
            self.as_bytes().len()
        }
        pub fn as_bytes(&self) -> &[u8] {
            unsafe { crate::intrinsics_str::str_as_bytes(self) }
        }
    }

    impl<T> [T] {
        pub fn len(&self) -> usize {
            unsafe { crate::intrinsics_str::slice_len(self) }
        }
        pub fn iter(&self) -> crate::slice::Iter<'_, T> {
            crate::slice::iter(self)
        }
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }
}

pub mod intrinsics_str {
    extern "rapid-intrinsic" {
        pub fn str_as_bytes(s: &str) -> &[u8];
        pub fn slice_len<T>(s: &[T]) -> usize;
    }
}

pub mod prelude {
    pub use crate::boxed::Box;
    pub use crate::clone::Clone;
    pub use crate::cmp::PartialEq;
    pub use crate::default::Default;
    pub use crate::mem::drop;
    pub use crate::ops::Drop;
    pub use crate::ops::{Fn, FnMut, FnOnce};
    pub use crate::option::Option;
    pub use crate::option::Option::None;
    pub use crate::option::Option::Some;
    pub use crate::option::Result;
    pub use crate::option::Result::Err;
    pub use crate::option::Result::Ok;
    pub use crate::string::String;
    pub use crate::string::ToString;
    pub use crate::vec::Vec;
    pub use crate::iter::Iterator;
    pub use crate::iter::IntoIterator;
}
