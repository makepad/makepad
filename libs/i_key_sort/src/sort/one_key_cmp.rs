use crate::sort::bin_layout::BIN_SORT_MIN;
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use crate::sort::serial::slice_one_key_cmp::OneKeyBinSortCmpSerial;
use alloc::vec::Vec;

/// Sort a slice by a primary integer‐like key, then by a comparator.
///
/// Useful when you want “group by key, then custom order inside groups”.
///
/// - Accepts `parallel: bool`. Ignored without the `allow_multithreading` feature.
/// - Accepts a key extractor and a comparator.
/// - Provides two variants: allocates internally, or reuse a buffer.
///
/// # Examples
/// ```
/// use i_key_sort::sort::one_key_cmp::OneKeyAndCmpSort;
///
/// let mut v = vec![("b", 2), ("a", 3), ("a", 1)];
/// v.sort_by_one_key_then_by(true, |x| x.0.as_bytes()[0], |a,b| a.1.cmp(&b.1));
/// assert_eq!(v, [("a",1), ("a",3), ("b",2)]);
/// ```
#[cfg(not(feature = "allow_multithreading"))]
pub trait OneKeyAndCmpSort<T> {
    fn sort_by_one_key_then_by<K, F1, F2>(&mut self, parallel: bool, key: F1, compare: F2)
    where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>;

    fn sort_by_one_key_then_by_and_buffer<K, F1, F2>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>;
}

#[cfg(feature = "allow_multithreading")]
pub trait OneKeyAndCmpSort<T> {
    fn sort_by_one_key_then_by<K, F1, F2>(&mut self, parallel: bool, key: F1, compare: F2)
    where
        K: SortKey + Send + Sync,
        F1: KeyFn<T, K> + Send + Sync,
        F2: CmpFn<T> + Send + Sync;

    fn sort_by_one_key_then_by_and_buffer<K, F1, F2>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        K: SortKey + Send + Sync,
        F1: KeyFn<T, K> + Send + Sync,
        F2: CmpFn<T> + Send + Sync;
}

#[cfg(not(feature = "allow_multithreading"))]
impl<T: Copy> OneKeyAndCmpSort<T> for [T] {
    #[inline]
    fn sort_by_one_key_then_by<K, F1, F2>(&mut self, _: bool, key: F1, compare: F2)
    where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>,
    {
        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_one_key_then_by(self, key, compare);
            return;
        }
        self.ser_sort_by_one_key_then_by_and_uninit_buffer(&mut Vec::new(), key, compare);
    }

    #[inline]
    fn sort_by_one_key_then_by_and_buffer<K, F1, F2>(
        &mut self,
        _: bool,
        reusable_buffer: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>,
    {
        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_one_key_then_by(self, key, compare);
            return;
        }
        self.ser_sort_by_one_key_then_by_and_uninit_buffer(reusable_buffer, key, compare);
    }
}

#[cfg(feature = "allow_multithreading")]
impl<T> OneKeyAndCmpSort<T> for [T]
where
    T: Send + Sync + Copy,
{
    #[inline]
    fn sort_by_one_key_then_by<K, F1, F2>(&mut self, parallel: bool, key: F1, compare: F2)
    where
        K: SortKey + Send + Sync,
        F1: KeyFn<T, K> + Send + Sync,
        F2: CmpFn<T> + Send + Sync,
    {
        use crate::sort::parallel::slice_one_key_cmp::OneKeyBinSortCmpParallel;

        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_one_key_then_by(self, key, compare);
            return;
        }
        let mut reusable_buffer = Vec::new();
        if parallel {
            self.par_sort_by_one_key_then_by(&mut reusable_buffer, key, compare);
        } else {
            self.ser_sort_by_one_key_then_by_and_uninit_buffer(&mut reusable_buffer, key, compare);
        }
    }

    #[inline]
    fn sort_by_one_key_then_by_and_buffer<K, F1, F2>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        K: SortKey + Send + Sync,
        F1: KeyFn<T, K> + Send + Sync,
        F2: CmpFn<T> + Send + Sync,
    {
        use crate::sort::parallel::slice_one_key_cmp::OneKeyBinSortCmpParallel;

        if self.len() < BIN_SORT_MIN {
            sort_unstable_by_one_key_then_by(self, key, compare);
            return;
        }

        if parallel {
            self.par_sort_by_one_key_then_by(reusable_buffer, key, compare);
        } else {
            self.ser_sort_by_one_key_then_by_and_uninit_buffer(reusable_buffer, key, compare);
        }
    }
}

#[inline]
pub(crate) fn sort_unstable_by_one_key_then_by<T, K, F1, F2>(slice: &mut [T], key: F1, compare: F2)
where
    K: SortKey,
    F1: KeyFn<T, K>,
    F2: CmpFn<T>,
{
    slice.sort_unstable_by(|a, b| key(a).cmp(&key(b)).then(compare(a, b)));
}

#[cfg(test)]
mod tests {
    use crate::sort::one_key_cmp::OneKeyAndCmpSort;
    use alloc::vec::Vec;

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
        arr1.sort_by_one_key_then_by(true, |a| a.0, |a, b| a.1.cmp(&b.1));
        arr2.sort_by_one_key_then_by(false, |a| a.0, |a, b| a.1.cmp(&b.1));
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
