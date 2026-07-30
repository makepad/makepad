use crate::sort::bin_layout::BinLayout;
use crate::sort::buffer::CopyFromNotOverlap;
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use alloc::vec::Vec;

pub(crate) trait OneKeyBinSortCmpSerial<T> {

    fn ser_sort_by_one_key_then_by_and_buffer<K, F1, F2>(
        &mut self,
        buf: &mut [T],
        key: F1,
        compare: F2,
        copy_to_src: bool,
    ) where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>;

    fn ser_sort_by_one_key_then_by_and_uninit_buffer<K, F1, F2>(
        &mut self,
        buf: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>;
}

impl<T: Copy> OneKeyBinSortCmpSerial<T> for [T] {
    #[inline]
    fn ser_sort_by_one_key_then_by_and_buffer<K, F1, F2>(
        &mut self,
        buf: &mut [T],
        key: F1,
        compare: F2,
        copy_to_src: bool,
    ) where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>,
    {
        debug_assert_eq!(self.len(), buf.len());
        if let Some(layout) = BinLayout::with_keys(self, key) {
            layout.sort_by_one_key_then_by_and_buffer(self, buf, key, compare, copy_to_src);
        } else {
            // one bin with single key for all elements
            self.sort_unstable_by(compare);
            if !copy_to_src {
                buf.copy_from_not_overlap(self);
            }
        }
    }

    #[inline]
    fn ser_sort_by_one_key_then_by_and_uninit_buffer<K, F1, F2>(
        &mut self,
        buf: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>,
    {
        if let Some(layout) = BinLayout::with_keys(self, key) {
            layout.sort_by_one_key_then_by_and_uninit_buffer(self, buf, key, compare);
        } else {
            // one bin with single key for all elements
            self.sort_unstable_by(compare);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sort::serial::slice_one_key_cmp::OneKeyBinSortCmpSerial;
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
        arr.ser_sort_by_one_key_then_by_and_uninit_buffer(&mut Vec::new(), |a| a.0, |a, b| a.1.cmp(&b.1));
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
