#![no_std]
extern crate alloc;

pub mod key;
pub mod seg;
pub mod set;

pub const EMPTY_REF: u32 = u32::MAX;

pub trait ExpiredKey<E: Expiration>: Copy + Ord {
    fn expiration(&self) -> E;
}

pub trait ExpiredVal<E: Expiration>: Copy {
    fn expiration(&self) -> E;
}

pub trait Expiration: Copy + Ord {
    fn max_expiration() -> Self;
}

pub trait LayoutUInt: Copy + Ord + core::ops::Shr<u32, Output = Self> {
    const HEAP_MIN_SPAN: Self;

    fn ilog2(self) -> u32;
    fn to_u32(self) -> u32;
}

pub trait LayoutNumber: Copy + Ord {
    type UInt: LayoutUInt;

    fn range_span(start: Self, end: Self) -> Option<Self::UInt>;
    fn offset_from(self, min: Self) -> Self::UInt;
}

macro_rules! impl_layout_uint {
    ($($t:ty),*) => {
        $(
            impl LayoutUInt for $t {
                const HEAP_MIN_SPAN: Self = 31;

                #[inline]
                fn ilog2(self) -> u32 {
                    self.ilog2()
                }

                #[inline]
                fn to_u32(self) -> u32 {
                    self as u32
                }
            }
        )*
    };
}

macro_rules! impl_signed_layout_number {
    ($($t:ty => ($u:ty, $w:ty)),*) => {
        $(
            impl LayoutNumber for $t {
                type UInt = $u;

                #[inline]
                fn range_span(start: Self, end: Self) -> Option<Self::UInt> {
                    if end < start {
                        return None;
                    }
                    Some((end as $w - start as $w) as Self::UInt)
                }

                #[inline]
                fn offset_from(self, min: Self) -> Self::UInt {
                    (self as $w - min as $w) as Self::UInt
                }
            }
        )*
    };
}

macro_rules! impl_unsigned_layout_number {
    ($($t:ty),*) => {
        $(
            impl LayoutNumber for $t {
                type UInt = $t;

                #[inline]
                fn range_span(start: Self, end: Self) -> Option<Self::UInt> {
                    if end < start {
                        return None;
                    }
                    Some(end - start)
                }

                #[inline]
                fn offset_from(self, min: Self) -> Self::UInt {
                    self - min
                }
            }
        )*
    };
}

impl_layout_uint!(u8, u16, u32, u64, usize);
impl_signed_layout_number!(
    i8 => (u8, i16),
    i16 => (u16, i32),
    i32 => (u32, i64),
    i64 => (u64, i128),
    isize => (usize, i128)
);
impl_unsigned_layout_number!(u8, u16, u32, u64, usize);

impl Expiration for u8 {
    #[inline]
    fn max_expiration() -> Self {
        u8::MAX
    }
}

impl Expiration for i8 {
    #[inline]
    fn max_expiration() -> Self {
        i8::MAX
    }
}

impl Expiration for u16 {
    #[inline]
    fn max_expiration() -> Self {
        u16::MAX
    }
}

impl Expiration for i16 {
    #[inline]
    fn max_expiration() -> Self {
        i16::MAX
    }
}

impl Expiration for u32 {
    #[inline]
    fn max_expiration() -> Self {
        u32::MAX
    }
}

impl Expiration for i32 {
    #[inline]
    fn max_expiration() -> Self {
        i32::MAX
    }
}

impl Expiration for u64 {
    #[inline]
    fn max_expiration() -> Self {
        u64::MAX
    }
}

impl Expiration for i64 {
    #[inline]
    fn max_expiration() -> Self {
        i64::MAX
    }
}

impl Expiration for usize {
    #[inline]
    fn max_expiration() -> Self {
        usize::MAX
    }
}
