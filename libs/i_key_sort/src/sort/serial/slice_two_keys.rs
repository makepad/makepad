use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::serial::slice_one_key::OneKeyBinSortSerial;
use alloc::vec::Vec;

pub(crate) trait TwoKeysBinSortSerial<T> {
    fn ser_sort_by_two_keys_and_buffer<K1, K2, F1, F2>(
        &mut self,
        buf: &mut [T],
        key1: F1,
        key2: F2,
        copy_to_src: bool,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>;

    fn ser_sort_by_two_keys_and_uninit_buffer<K1, K2, F1, F2>(
        &mut self,
        buf: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>;
}

impl<T: Copy> TwoKeysBinSortSerial<T> for [T] {
    #[inline]
    fn ser_sort_by_two_keys_and_buffer<K1, K2, F1, F2>(
        &mut self,
        buf: &mut [T],
        key1: F1,
        key2: F2,
        copy_to_src: bool,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
    {
        debug_assert_eq!(self.len(), buf.len());
        if let Some(layout) = BinLayout::with_keys(self, key1) {
            layout.sort_by_two_keys_and_buffer(self, buf, key1, key2, copy_to_src);
        } else {
            // one bin with single key1 for all elements
            self.ser_sort_by_one_key_and_buffer(buf, key2, copy_to_src);
        }
    }

    #[inline]
    fn ser_sort_by_two_keys_and_uninit_buffer<K1, K2, F1, F2>(
        &mut self,
        buf: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
    {
        if let Some(layout) = BinLayout::with_keys(self, key1) {
            layout.sort_by_two_keys_and_uninit_buffer(self, buf, key1, key2);
        } else {
            // one bin with single key1 for all elements
            self.ser_sort_by_one_key_and_uninit_buffer(buf, key2);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sort::serial::slice_two_keys::TwoKeysBinSortSerial;
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

    fn test(count: usize) {
        let mut org: Vec<_> = reversed_2d_array(count);
        let mut arr = org.clone();
        arr.ser_sort_by_two_keys_and_uninit_buffer(&mut Vec::new(), |a| a.0, |a| a.1);
        org.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        assert!(arr == org);
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
