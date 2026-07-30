use crate::sort::bin_layout::BIN_SORT_MIN;
use crate::sort::buffer::{CopyFromNotOverlap, CopyNotOverlapValue, DoubleRangeSlices};
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use crate::sort::mapper::Mapper;
use crate::sort::serial::slice_two_keys_cmp::TwoKeysBinSortCmpSerial;
use crate::sort::two_keys_cmp::sort_unstable_by_two_keys_then_by;

impl Mapper {
    #[inline]
    pub(crate) fn sort_chunks_by_two_keys_then_by<K1, K2, T, F1, F2, F3>(
        &self,
        src: &mut [T],
        buf: &mut [T],
        key1: F1,
        key2: F2,
        compare: F3,
        copy_to_src: bool,
    ) where
        K1: SortKey,
        K2: SortKey,
        T: Copy,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
        F3: CmpFn<T>,
    {
        const TINY_SORT_MAX: usize = BIN_SORT_MIN;

        // if `copy_to_src` is true
        // must copy `src` to `buf`, since the result array is in the buffer

        for chunk in self.iter() {
            let range = chunk.as_range();
            match range.len() {
                0 => continue,
                1 => {
                    if copy_to_src {
                        src.copy_value_from(buf, range.start);
                    }
                }
                2..TINY_SORT_MAX => {
                    // SAFETY: mapper ranges never overlap; (src, buf) are distinct buffers.
                    let sub_buf = unsafe { buf.get_unchecked_mut(range.clone()) };
                    sort_unstable_by_two_keys_then_by(sub_buf, key1, key2, compare);

                    if copy_to_src {
                        src.copy_to_range_from_not_overlap(sub_buf, range);
                    }
                }
                _ => {
                    let (sub_src, sub_buf) = range.mut_slices(src, buf);
                    sub_buf.ser_sort_by_two_keys_then_by_and_buffer(
                        sub_src,
                        key1,
                        key2,
                        compare,
                        !copy_to_src,
                    );
                }
            }
        }
    }
}
