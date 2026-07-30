use crate::sort::bin_layout::BIN_SORT_MIN;
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::serial::slice_two_keys::TwoKeysBinSortSerial;
use alloc::vec::Vec;

/// Sort a slice lexicographically by two integer‐like keys.
///
/// - First bins by `key1`, then within bins by `key2`.
/// - Accepts `parallel: bool`. Ignored without `allow_multithreading`.
/// - Has two variants: allocate internally or reuse a buffer.
///
/// # Examples
/// ```
/// use i_key_sort::sort::two_keys::TwoKeysSort;
///
/// let mut v = vec![(2,1), (1,2), (1,0)];
/// v.sort_by_two_keys(true, |x| x.0, |x| x.1);
/// assert_eq!(v, [(1,0), (1,2), (2,1)]);
/// ```
#[cfg(not(feature = "allow_multithreading"))]
pub trait TwoKeysSort<T> {
    fn sort_by_two_keys<K1, K2, F1, F2>(&mut self, parallel: bool, key1: F1, key2: F2)
    where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>;

    fn sort_by_two_keys_and_buffer<K1, K2, F1, F2>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>;
}

#[cfg(feature = "allow_multithreading")]
pub trait TwoKeysSort<T> {
    fn sort_by_two_keys<K1, K2, F1, F2>(&mut self, parallel: bool, key1: F1, key2: F2)
    where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync;

    fn sort_by_two_keys_and_buffer<K1, K2, F1, F2>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync;
}

#[cfg(not(feature = "allow_multithreading"))]
impl<T: Copy> TwoKeysSort<T> for [T] {
    #[inline]
    fn sort_by_two_keys<K1, K2, F1, F2>(&mut self, _: bool, key1: F1, key2: F2)
    where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
    {
        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_two_keys(self, key1, key2);
            return;
        }
        self.ser_sort_by_two_keys_and_uninit_buffer(&mut Vec::new(), key1, key2);
    }

    #[inline]
    fn sort_by_two_keys_and_buffer<K1, K2, F1, F2>(
        &mut self,
        _: bool,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
    {
        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_two_keys(self, key1, key2);
            return;
        }
        self.ser_sort_by_two_keys_and_uninit_buffer(reusable_buffer, key1, key2);
    }
}

#[cfg(feature = "allow_multithreading")]
impl<T> TwoKeysSort<T> for [T]
where
    T: Send + Sync + Copy,
{
    #[inline]
    fn sort_by_two_keys<K1, K2, F1, F2>(&mut self, parallel: bool, key1: F1, key2: F2)
    where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
    {
        use crate::sort::parallel::slice_two_keys::TwoKeysBinSortParallel;

        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_two_keys(self, key1, key2);
            return;
        }
        let mut reusable_buffer = Vec::new();
        if parallel {
            self.par_sort_by_two_keys(&mut reusable_buffer, key1, key2);
        } else {
            self.ser_sort_by_two_keys_and_uninit_buffer(&mut reusable_buffer, key1, key2);
        }
    }

    #[inline]
    fn sort_by_two_keys_and_buffer<K1, K2, F1, F2>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
    {
        use crate::sort::parallel::slice_two_keys::TwoKeysBinSortParallel;

        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_two_keys(self, key1, key2);
            return;
        }
        if parallel {
            self.par_sort_by_two_keys(reusable_buffer, key1, key2);
        } else {
            self.ser_sort_by_two_keys_and_uninit_buffer(reusable_buffer, key1, key2);
        }
    }
}

#[inline]
pub(crate) fn sort_unstable_by_two_keys<T, K1, K2, F1, F2>(slice: &mut [T], key1: F1, key2: F2)
where
    K1: SortKey,
    K2: SortKey,
    F1: KeyFn<T, K1>,
    F2: KeyFn<T, K2>,
{
    slice.sort_unstable_by(|a, b| key1(a).cmp(&key1(b)).then(key2(a).cmp(&key2(b))))
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use crate::sort::two_keys::TwoKeysSort;

    #[test]
    fn test_0() {
        test(2);
    }

    #[test]
    fn test_1() {
        test(10);
    }

    #[test]
    fn test_2() {
        test(30);
    }

    #[test]
    fn test_3() {
        test(100);
    }

    #[test]
    fn test_4() {
        test(300);
    }

    #[test]
    fn test_5() {
        test(1000);
    }

    #[test]
    fn test_dynamic_0() {
        for i in 0..100 {
            test(i);
        }
    }

    fn test(count: usize) {
        let mut org: Vec<_> = reversed_2d_array(count);
        let mut arr1 = org.clone();
        let mut arr2 = org.clone();
        arr1.sort_by_two_keys(true, |a| a.0, |a| a.1);
        arr2.sort_by_two_keys(false, |a| a.0, |a| a.1);
        org.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        assert!(arr1 == org);
        assert!(arr2 == org);
    }

    fn reversed_2d_array(count: usize) -> Vec<(i32, i32)> {
        let mut arr = Vec::with_capacity(count * count);
        for x in (0..count as i32).rev() {
            for y in (0..count as i32).rev() {
                arr.push((x, y))
            }
        }

        arr
    }
}