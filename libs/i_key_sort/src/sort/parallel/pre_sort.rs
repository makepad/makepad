use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::mapper::Mapper;
use core::ops::Range;
use std::mem::MaybeUninit;

pub(super) struct PreSortFragment<'a, T> {
    pub(super) base: usize,
    pub(super) src: &'a mut [T],
    pub(super) buf: &'a mut [MaybeUninit<T>],
}

pub(super) struct IdRange {
    pub(super) index: usize,
    pub(super) range: Range<usize>,
}

impl<T> PreSortFragment<'_, T>
where
    T: Send + Copy,
{
    #[inline]
    pub(super) fn spread<K, F>(&mut self, layout: BinLayout<K>, key: F) -> Vec<IdRange>
    where
        K: SortKey,
        F: KeyFn<T, K>,
    {
        let mut mapper = Mapper::new(layout.count());
        for a in self.src.iter() {
            mapper.inc_bin_count(layout.index(key(a)));
        }

        mapper.init_indices();

        for val in self.src.iter() {
            let index = mapper.next_index(layout.index(key(val)));
            unsafe {
                let _ = *self.buf.get_unchecked_mut(index).write(*val);
            }
        }

        let mut ranges = Vec::with_capacity(mapper.count);
        for (index, chunk) in mapper.iter().enumerate() {
            let range = chunk.as_range();
            if !range.is_empty() {
                let global_range = range.start + self.base..range.end + self.base;
                ranges.push(IdRange {
                    index,
                    range: global_range,
                });
            }
        }

        ranges
    }
}

pub(super) trait FragmentationByCount<T> {
    fn fragment_by_count<'a>(
        &'a mut self,
        buffer: &'a mut Vec<T>,
        count: usize,
    ) -> Vec<PreSortFragment<'a, T>>;
}

impl<T> FragmentationByCount<T> for [T] {
    #[inline]
    fn fragment_by_count<'a>(
        &'a mut self,
        buffer: &'a mut Vec<T>,
        count: usize,
    ) -> Vec<PreSortFragment<'a, T>> {
        let (capacity, step_len) = if self.len() < count {
            (self.len(), 1)
        } else {
            let step_len = self.len().div_ceil(count);
            let count = self.len().div_ceil(step_len);
            (count, step_len)
        };

        let mut scratch: &mut [MaybeUninit<T>] = &mut buffer.spare_capacity_mut()[..self.len()];

        let mut frags = Vec::with_capacity(capacity);
        let mut base = 0;

        let mut src = self;

        for _ in 0..capacity.saturating_sub(1) {
            let (left_src, right_src) = src.split_at_mut(step_len);
            let (left_buf, right_buf) = scratch.split_at_mut(step_len);

            frags.push(PreSortFragment {
                base,
                src: left_src,
                buf: left_buf,
            });

            src = right_src;
            scratch = right_buf;

            base += step_len;
        }

        frags.push(PreSortFragment {
            base,
            src,
            buf: scratch,
        });

        frags
    }
}
