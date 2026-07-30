use core::cmp::Ordering;

pub trait KeyFn<T, K>: Fn(&T) -> K + Copy {}
impl<T, K, F: Fn(&T) -> K + Copy> KeyFn<T, K> for F {}

pub trait CmpFn<T>: Fn(&T, &T) -> Ordering + Copy {}
impl<T, F: Fn(&T, &T) -> Ordering + Copy> CmpFn<T> for F {}

pub trait SortKey: Copy + Ord {
    /// Returns the number of bits required to represent `self - other`.
    fn distance_bits(self, other: Self) -> usize;

    /// Returns `(self - other) >> shift`, saturated to the `usize` range.
    ///
    /// The distance must be shifted before it is converted to `usize`.
    fn shifted_distance(self, other: Self, shift: usize) -> usize;
}

macro_rules! impl_unsigned_sort_key {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl SortKey for $ty {
                #[inline(always)]
                fn distance_bits(self, other: Self) -> usize {
                    debug_assert!(self >= other, "distance_bits() requires self >= other");
                    let distance = self - other;
                    (<$ty>::BITS - distance.leading_zeros()) as usize
                }

                #[inline(always)]
                fn shifted_distance(self, other: Self, shift: usize) -> usize {
                    debug_assert!(self >= other, "shifted_distance() requires self >= other");
                    let distance = self - other;
                    let shifted = u32::try_from(shift)
                        .ok()
                        .and_then(|shift| distance.checked_shr(shift))
                        .unwrap_or(0);
                    usize::try_from(shifted).unwrap_or(usize::MAX)
                }
            }
        )+
    };
}

macro_rules! impl_signed_sort_key {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl SortKey for $ty {
                #[inline(always)]
                fn distance_bits(self, other: Self) -> usize {
                    debug_assert!(self >= other, "distance_bits() requires self >= other");
                    let distance = self.abs_diff(other);
                    (<$ty>::BITS - distance.leading_zeros()) as usize
                }

                #[inline(always)]
                fn shifted_distance(self, other: Self, shift: usize) -> usize {
                    debug_assert!(self >= other, "shifted_distance() requires self >= other");
                    let distance = self.abs_diff(other);
                    let shifted = u32::try_from(shift)
                        .ok()
                        .and_then(|shift| distance.checked_shr(shift))
                        .unwrap_or(0);
                    usize::try_from(shifted).unwrap_or(usize::MAX)
                }
            }
        )+
    };
}

impl_unsigned_sort_key!(u8, u16, u32, u64, usize);
impl_signed_sort_key!(i8, i16, i32, i64);
