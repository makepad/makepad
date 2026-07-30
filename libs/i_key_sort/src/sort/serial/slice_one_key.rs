use crate::sort::bin_layout::BinLayout;
use crate::sort::buffer::CopyFromNotOverlap;
use crate::sort::key::{KeyFn, SortKey};
use alloc::vec::Vec;

pub(crate) trait OneKeyBinSortSerial<T> {
    fn ser_sort_by_one_key_and_buffer<K: SortKey, F: KeyFn<T, K>>(
        &mut self,
        buf: &mut [T],
        key: F,
        copy_to_src: bool,
    );

    fn ser_sort_by_one_key_and_uninit_buffer<K: SortKey, F: KeyFn<T, K>>(
        &mut self,
        buf: &mut Vec<T>,
        key: F,
    );
}

impl<T: Copy> OneKeyBinSortSerial<T> for [T] {
    #[inline]
    fn ser_sort_by_one_key_and_buffer<K: SortKey, F: KeyFn<T, K>>(
        &mut self,
        buf: &mut [T],
        key: F,
        copy_to_src: bool,
    ) {
        debug_assert_eq!(self.len(), buf.len());
        if let Some(layout) = BinLayout::with_keys(self, key) {
            layout.sort_by_one_key_and_buffer(self, buf, key, copy_to_src);
        } else if !copy_to_src {
            buf.copy_from_not_overlap(self);
        }
    }

    #[inline]
    fn ser_sort_by_one_key_and_uninit_buffer<K: SortKey, F: KeyFn<T, K>>(
        &mut self,
        buf: &mut Vec<T>,
        key: F,
    ) {
        if let Some(layout) = BinLayout::with_keys(self, key) {
            layout.sort_by_one_key_and_uninit_buffer(self, buf, key);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sort::serial::slice_one_key::OneKeyBinSortSerial;
    use alloc::vec::Vec;

    #[test]
    fn test_0() {
        test(10);
    }

    #[test]
    fn test_1x() {
        test(18);
    }

    #[test]
    fn test_1() {
        test(100);
    }

    #[test]
    fn test_2() {
        test(1_000);
    }

    #[test]
    fn test_3() {
        test(10_000);
    }

    #[test]
    fn test_4() {
        test(100_000);
    }

    #[test]
    fn test_5() {
        test(1_000_000);
    }

    fn test(count: usize) {
        let mut org: Vec<_> = (0..count).rev().collect();
        let mut arr = org.clone();
        arr.ser_sort_by_one_key_and_uninit_buffer(&mut Vec::new(), |&a| a);
        org.sort_unstable();
        assert!(arr == org);
    }
}
