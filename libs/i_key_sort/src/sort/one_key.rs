use crate::sort::bin_layout::BIN_SORT_MIN;
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::serial::slice_one_key::OneKeyBinSortSerial;
use alloc::vec::Vec;

/// Sort a slice by a single integer‐like key function.
///
/// This is the simplest and fastest entry point: it bins by key,
/// then sorts within bins if needed.
///
/// - Accepts a `parallel: bool` flag.
///   *If the crate was built without the `allow_multithreading` feature,
///   this flag is ignored and sorting runs serially.*
/// - Accepts a key extractor `F: Fn(&T) -> K` where `K: SortKey`
///   (all integer types are supported).
///
/// Two variants:
/// - [`sort_by_one_key`] — allocates a buffer internally.
/// - [`sort_by_one_key_and_buffer`] — caller provides a reusable
///   `Vec<T>` to avoid allocations.
///
/// # Examples
/// ```
/// use i_key_sort::sort::one_key::OneKeySort;
///
/// let mut v = [5, 1, 4, 2];
/// v.sort_by_one_key(true, |&x| x);
/// assert_eq!(v, [1,2,4,5]);
/// ```
#[cfg(not(feature = "allow_multithreading"))]
pub trait OneKeySort<T> {
    fn sort_by_one_key<K, F>(&mut self, parallel: bool, key: F)
    where
        K: SortKey,
        F: KeyFn<T, K>;

    fn sort_by_one_key_and_buffer<K, F>(
        &mut self,
        parallel: bool,
        buffer: &mut Vec<T>,
        key: F,
    ) where
        K: SortKey,
        F: KeyFn<T, K>;
}

#[cfg(feature = "allow_multithreading")]
pub trait OneKeySort<T> {
    fn sort_by_one_key<K, F>(&mut self, parallel: bool, key: F)
    where
        K: SortKey + Send + Sync,
        F: KeyFn<T, K> + Send + Sync;

    fn sort_by_one_key_and_buffer<K, F>(
        &mut self,
        parallel: bool,
        reusable_buffer: &mut Vec<T>,
        key: F,
    ) where
        K: SortKey + Send + Sync,
        F: KeyFn<T, K> + Send + Sync;
}

#[cfg(not(feature = "allow_multithreading"))]
impl<T: Copy> OneKeySort<T> for [T] {
    #[inline]
    fn sort_by_one_key<K, F>(&mut self, _: bool, key: F)
    where
        K: SortKey,
        F: KeyFn<T, K>,
    {
        if self.len() < BIN_SORT_MIN {
            self.sort_unstable_by_key(key);
            return;
        }
        self.ser_sort_by_one_key_and_uninit_buffer(&mut Vec::new(), key);
    }

    #[inline]
    fn sort_by_one_key_and_buffer<K: SortKey, F: KeyFn<T, K>>(
        &mut self,
        _: bool,
        buffer: &mut Vec<T>,
        key: F,
    ) {
        if self.len() < BIN_SORT_MIN {
            self.sort_unstable_by_key(key);
            return;
        }
        self.ser_sort_by_one_key_and_uninit_buffer(buffer, key);
    }
}

#[cfg(feature = "allow_multithreading")]
impl<T> OneKeySort<T> for [T]
where
    T: Send + Sync + Copy,
{
    #[inline]
    fn sort_by_one_key<K, F>(&mut self, parallel: bool, key: F)
    where
        K: SortKey + Send + Sync,
        F: KeyFn<T, K> + Send + Sync,
    {
        use crate::sort::parallel::slice_one_key::OneKeyBinSortParallel;

        if self.len() < BIN_SORT_MIN {
            self.sort_unstable_by_key(key);
            return;
        }
        let mut buffer = Vec::new();
        if parallel {
            self.par_sort_by_one_key(&mut buffer, key);
        } else {
            self.ser_sort_by_one_key_and_uninit_buffer(&mut buffer, key);
        }
    }

    #[inline]
    fn sort_by_one_key_and_buffer<K, F>(
        &mut self,
        parallel: bool,
        buffer: &mut Vec<T>,
        key: F,
    ) where
        K: SortKey + Send + Sync,
        F: KeyFn<T, K> + Send + Sync,
    {
        use crate::sort::parallel::slice_one_key::OneKeyBinSortParallel;

        if self.len() < BIN_SORT_MIN {
            self.sort_unstable_by_key(key);
            return;
        }
        if parallel {
            self.par_sort_by_one_key(buffer, key);
        } else {
            self.ser_sort_by_one_key_and_uninit_buffer(buffer, key);
        }
    }
}


#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use crate::sort::one_key::OneKeySort;

    #[test]
    fn test_0() {
        test(10);
    }

    #[test]
    fn test_1() {
        test(34);
    }

    #[test]
    fn test_2() {
        test(1_000);
    }

    #[test]
    fn test_3() {
        test(5_000);
    }

    #[test]
    fn test_4() {
        test(100_000);
    }

    #[test]
    fn test_5() {
        test(1_000_000);
    }

    #[test]
    fn test_dynamic_0() {
        for i in 0..1_000 {
            test(i);
        }
    }

    fn test(count: usize) {
        let mut org: Vec<_> = (0..count).rev().collect();
        let mut arr1 = org.clone();
        let mut arr2 = org.clone();
        arr1.sort_by_one_key(true, |&a| a);
        arr2.sort_by_one_key(false, |&a| a);
        org.sort_unstable();
        assert!(arr1 == org);
        assert!(arr2 == org);
    }
}
