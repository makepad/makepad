use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use alloc::vec::Vec;

impl<K: SortKey> BinLayout<K> {
    #[inline]
    pub(super) fn sort_by_one_key_then_by_and_uninit_buffer<T, F1, F2>(
        &self,
        src: &mut [T],
        buf: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        T: Copy,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>,
    {
        let mapper = self.spread_with_uninit_buffer(src, buf, key);

        if self.bin_width_is_one() {
            // all elements inside bins have the same key1
            // continue sort elements by compare
            mapper.sort_chunks_by(src, buf, compare, true);
        } else {
            mapper.sort_chunks_by_one_key_then_by(src, buf, key, compare, true);
        }
    }

    #[inline]
    pub(super) fn sort_by_one_key_then_by_and_buffer<T, F1, F2>(
        &self,
        src: &mut [T],
        buf: &mut [T],
        key: F1,
        compare: F2,
        copy_to_src: bool,
    ) where
        T: Copy,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>,
    {
        debug_assert_eq!(src.len(), buf.len());

        let mapper = self.spread_with_buffer(src, buf, key);

        if self.bin_width_is_one() {
            // all elements inside bins have the same key1
            // continue sort elements by compare
            mapper.sort_chunks_by(src, buf, compare, copy_to_src);
        } else {
            mapper.sort_chunks_by_one_key_then_by(src, buf, key, compare, copy_to_src);
        }
    }
}
