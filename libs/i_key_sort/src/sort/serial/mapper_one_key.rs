use crate::sort::bin_layout::BIN_SORT_MIN;
use crate::sort::buffer::{CopyFromNotOverlap, CopyNotOverlapValue, DoubleRangeSlices};
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::mapper::Mapper;
use crate::sort::serial::slice_one_key::OneKeyBinSortSerial;

impl Mapper {
    #[inline]
    pub(crate) fn sort_chunks_by_one_key<K: SortKey, T: Copy, F: KeyFn<T, K>>(
        &self,
        src: &mut [T],
        buf: &mut [T],
        key: F,
        copy_to_src: bool,
    ) {
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
                    sub_buf.sort_unstable_by_key(key);

                    if copy_to_src {
                        src.copy_to_range_from_not_overlap(sub_buf, range);
                    }
                }
                _ => {
                    let (sub_src, sub_buf) = range.mut_slices(src, buf);
                    sub_buf.ser_sort_by_one_key_and_buffer(sub_src, key, !copy_to_src);
                }
            }
        }
    }
}
