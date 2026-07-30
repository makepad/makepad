use crate::sort::buffer::{CopyFromNotOverlap, CopyNotOverlapValue};
use crate::sort::key::CmpFn;
use crate::sort::mapper::Mapper;

impl Mapper {
    #[inline]
    pub(crate) fn sort_chunks_by<T: Copy, F: CmpFn<T>>(
        &self,
        src: &mut [T],
        buf: &mut [T],
        compare: F,
        copy_to_src: bool,
    ) {

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
                _ => {
                    let sub_buf = unsafe { buf.get_unchecked_mut(range.clone()) };
                    sub_buf.sort_unstable_by(compare);
                    if copy_to_src {
                        src.copy_to_range_from_not_overlap(sub_buf, range);
                    }
                }
            }
        }
    }
}