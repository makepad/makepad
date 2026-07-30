use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{KeyFn, SortKey};
use alloc::vec::Vec;

impl<K1: SortKey> BinLayout<K1> {
    #[inline]
    pub(super) fn sort_by_two_keys_and_uninit_buffer<T, K2, F1, F2>(
        &self,
        src: &mut [T],
        buf: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        T: Copy,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
    {
        let mapper = self.spread_with_uninit_buffer(src, buf, key1);

        if self.bin_width_is_one() {
            // all elements inside bins have the same key1
            // continue sort elements inside bins by key2
            mapper.sort_chunks_by_one_key(src, buf, key2, true);
        } else {
            mapper.sort_chunks_by_two_keys(src, buf, key1, key2, true);
        }
    }

    #[inline]
    pub(super) fn sort_by_two_keys_and_buffer<T, K2, F1, F2>(
        &self,
        src: &mut [T],
        buf: &mut [T],
        key1: F1,
        key2: F2,
        copy_to_src: bool,
    ) where
        T: Copy,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
    {
        debug_assert_eq!(src.len(), buf.len());

        let mapper = self.spread_with_buffer(src, buf, key1);

        if self.bin_width_is_one() {
            // all elements inside bins have the same key1
            // continue sort elements inside bins by key2
            mapper.sort_chunks_by_one_key(src, buf, key2, copy_to_src);
        } else {
            mapper.sort_chunks_by_two_keys(src, buf, key1, key2, copy_to_src);
        }
    }
}
