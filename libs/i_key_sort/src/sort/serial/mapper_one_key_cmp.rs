use crate::sort::bin_layout::BIN_SORT_MIN;
use crate::sort::buffer::{CopyFromNotOverlap, CopyNotOverlapValue, DoubleRangeSlices};
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use crate::sort::mapper::Mapper;
use crate::sort::one_key_cmp::sort_unstable_by_one_key_then_by;
use crate::sort::serial::slice_one_key_cmp::OneKeyBinSortCmpSerial;

impl Mapper {
    #[inline]
    pub(crate) fn sort_chunks_by_one_key_then_by<K, T, F1, F2>(
        &self,
        src: &mut [T],
        buf: &mut [T],
        key: F1,
        compare: F2,
        copy_to_src: bool,
    ) where
        K: SortKey,
        T: Copy,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>,
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
                    sort_unstable_by_one_key_then_by(sub_buf, key, compare);
                
                    if copy_to_src {
                        src.copy_to_range_from_not_overlap(sub_buf, range);
                    }
                }
                _ => {
                    let (sub_src, sub_buf) = range.mut_slices(src, buf);
                    sub_buf.ser_sort_by_one_key_then_by_and_buffer(
                        sub_src,
                        key,
                        compare,
                        !copy_to_src,
                    );
                }
            }
        }
    }
}
