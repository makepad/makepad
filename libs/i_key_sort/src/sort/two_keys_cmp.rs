use crate::sort::bin_layout::BIN_SORT_MIN;
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use crate::sort::serial::slice_two_keys_cmp::TwoKeysBinSortCmpSerial;
use alloc::vec::Vec;

/// Sort a slice lexicographically by two integer‐like keys, then by a comparator.
///
/// This is the most general API: primary key, secondary key,
/// and a tiebreaking comparator.
///
/// - Accepts `parallel: bool`. Ignored without `allow_multithreading`.
/// - Two variants: allocate internally or reuse a buffer.
///
/// # Examples
/// ```
/// use i_key_sort::sort::two_keys_cmp::TwoKeysAndCmpSort;
///
/// let mut v = vec![(1u32,0i32,9i32), (1,0,3), (1,1,1)];
/// v.sort_by_two_keys_then_by(true, |x| x.0, |x| x.1, |a,b| a.2.cmp(&b.2));
/// assert_eq!(v, [(1,0,3), (1,0,9), (1,1,1)]);
/// ```
#[cfg(not(feature = "allow_multithreading"))]
pub trait TwoKeysAndCmpSort<T> {
    fn sort_by_two_keys_then_by<K1, K2, F1, F2, F3>(
        &mut self,
        parallel: bool,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
        F3: CmpFn<T>;

    fn sort_by_two_keys_then_by_and_buffer<K1, K2, F1, F2, F3>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
        F3: CmpFn<T>;
}

#[cfg(feature = "allow_multithreading")]
pub trait TwoKeysAndCmpSort<T> {
    fn sort_by_two_keys_then_by<K1, K2, F1, F2, F3>(
        &mut self,
        parallel: bool,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
        F3: CmpFn<T> + Send + Sync;

    fn sort_by_two_keys_then_by_and_buffer<K1, K2, F1, F2, F3>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
        F3: CmpFn<T> + Send + Sync;
}

#[cfg(not(feature = "allow_multithreading"))]
impl<T: Copy> TwoKeysAndCmpSort<T> for [T] {
    #[inline]
    fn sort_by_two_keys_then_by<K1, K2, F1, F2, F3>(
        &mut self,
        _: bool,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
        F3: CmpFn<T>,
    {
        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_two_keys_then_by(self, key1, key2, compare);
            return;
        }
        self.ser_sort_by_two_keys_then_by_and_uninit_buffer(&mut Vec::new(), key1, key2, compare);
    }

    #[inline]
    fn sort_by_two_keys_then_by_and_buffer<K1, K2, F1, F2, F3>(
        &mut self,
        _: bool,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
        F3: CmpFn<T>,
    {
        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_two_keys_then_by(self, key1, key2, compare);
            return;
        }
        self.ser_sort_by_two_keys_then_by_and_uninit_buffer(reusable_buffer, key1, key2, compare);
    }
}

#[cfg(feature = "allow_multithreading")]
impl<T> TwoKeysAndCmpSort<T> for [T]
where
    T: Send + Sync + Copy,
{
    #[inline]
    fn sort_by_two_keys_then_by<K1, K2, F1, F2, F3>(
        &mut self,
        parallel: bool,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
        F3: CmpFn<T> + Send + Sync,
    {
        use crate::sort::parallel::slice_two_keys_cmp::TwoKeysBinSortCmpParallel;

        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_two_keys_then_by(self, key1, key2, compare);
            return;
        }
        let mut reusable_buffer = Vec::new();
        if parallel {
            self.par_sort_by_two_keys_then_by(&mut reusable_buffer, key1, key2, compare);
        } else {
            self.ser_sort_by_two_keys_then_by_and_uninit_buffer(
                &mut reusable_buffer,
                key1,
                key2,
                compare,
            );
        }
    }

    #[inline]
    fn sort_by_two_keys_then_by_and_buffer<K1, K2, F1, F2, F3>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
        F3: CmpFn<T> + Send + Sync,
    {
        use crate::sort::parallel::slice_two_keys_cmp::TwoKeysBinSortCmpParallel;

        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_two_keys_then_by(self, key1, key2, compare);
            return;
        }
        if parallel {
            self.par_sort_by_two_keys_then_by(reusable_buffer, key1, key2, compare);
        } else {
            self.ser_sort_by_two_keys_then_by_and_uninit_buffer(
                reusable_buffer,
                key1,
                key2,
                compare,
            );
        }
    }
}

#[inline]
pub(crate) fn sort_unstable_by_two_keys_then_by<T, K1, K2, F1, F2, F3>(
    slice: &mut [T],
    key1: F1,
    key2: F2,
    compare: F3,
) where
    K1: SortKey,
    K2: SortKey,
    F1: KeyFn<T, K1>,
    F2: KeyFn<T, K2>,
    F3: CmpFn<T>,
{
    slice.sort_unstable_by(|a, b| {
        key1(a)
            .cmp(&key1(b))
            .then(key2(a).cmp(&key2(b)))
            .then(compare(a, b))
    });
}

#[cfg(test)]
mod tests {
    use crate::sort::two_keys_cmp::TwoKeysAndCmpSort;
    use alloc::vec::Vec;

    #[test]
    fn test_0() {
        test(2);
    }

    #[test]
    fn test_1() {
        test(5);
    }

    #[test]
    fn test_2() {
        test(10);
    }

    #[test]
    fn test_3() {
        test(20);
    }

    #[test]
    fn test_4() {
        test(40);
    }

    #[test]
    fn test_5() {
        test(100);
    }

    #[test]
    fn test_dynamic_0() {
        for i in 0..30 {
            test(i);
        }
    }

    fn test(count: usize) {
        let mut org: Vec<_> = reversed_2d_array(count);
        let mut arr1 = org.clone();
        let mut arr2 = org.clone();
        arr1.sort_by_two_keys_then_by(true, |a| a.0, |a| a.1, |a, b| a.2.cmp(&b.2));
        arr2.sort_by_two_keys_then_by(false, |a| a.0, |a| a.1, |a, b| a.2.cmp(&b.2));

        org.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        assert!(arr1 == org);
        assert!(arr2 == org);
    }

    fn reversed_2d_array(count: usize) -> Vec<(u32, i32, i32)> {
        let mut arr = Vec::with_capacity(count * count * count);
        for i in (0..count as u32).rev() {
            for x in (0..count as i32).rev() {
                for y in (0..count as i32).rev() {
                    arr.push((i, x, y))
                }
            }
        }

        arr
    }
}
