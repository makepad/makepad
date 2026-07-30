use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use crate::sort::serial::slice_one_key_cmp::OneKeyBinSortCmpSerial;
use alloc::vec::Vec;

pub(crate) trait TwoKeysBinSortCmpSerial<T> {
    fn ser_sort_by_two_keys_then_by_and_buffer<K1, K2, F1, F2, F3>(
        &mut self,
        buf: &mut [T],
        key1: F1,
        key2: F2,
        compare: F3,
        copy_to_src: bool,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
        F3: CmpFn<T>;

    fn ser_sort_by_two_keys_then_by_and_uninit_buffer<K1, K2, F1, F2, F3>(
        &mut self,
        buf: &mut Vec<T>,
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

impl<T: Copy> TwoKeysBinSortCmpSerial<T> for [T] {
    #[inline]
    fn ser_sort_by_two_keys_then_by_and_buffer<K1, K2, F1, F2, F3>(
        &mut self,
        buf: &mut [T],
        key1: F1,
        key2: F2,
        compare: F3,
        copy_to_src: bool,
    ) where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
        F3: CmpFn<T>,
    {
        debug_assert_eq!(self.len(), buf.len());
        if let Some(layout) = BinLayout::with_keys(self, key1) {
            layout.sort_by_two_keys_then_by_and_buffer(self, buf, key1, key2, compare, copy_to_src);
        } else {
            // already sorted by key1
            self.ser_sort_by_one_key_then_by_and_buffer(buf, key2, compare, copy_to_src);
        }
    }

    #[inline]
    fn ser_sort_by_two_keys_then_by_and_uninit_buffer<K1, K2, F1, F2, F3>(
        &mut self,
        buf: &mut Vec<T>,
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
        if let Some(layout) = BinLayout::with_keys(self, key1) {
            layout.sort_by_two_keys_then_by_and_uninit_buffer(self, buf, key1, key2, compare);
        } else {
            // all bins already sorted by key1
            self.ser_sort_by_one_key_then_by_and_uninit_buffer(buf, key2, compare);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sort::serial::slice_two_keys_cmp::TwoKeysBinSortCmpSerial;
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

    fn test(count: usize) {
        let mut org: Vec<_> = reversed_2d_array(count);
        let mut arr = org.clone();
        arr.ser_sort_by_two_keys_then_by_and_uninit_buffer(
            &mut Vec::new(),
            |a| a.0,
            |a| a.1,
            |a, b| a.2.cmp(&b.2),
        );

        org.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        assert!(arr == org);
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
