use crate::sort::bin_layout::{BinLayout, LayoutConstraints, MAX_BINS_COUNT};
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::parallel::pre_sort::{PreSortFragment, IdRange, FragmentationByCount};
use rayon::iter::ParallelIterator;
use rayon::iter::{IntoParallelRefIterator, IntoParallelRefMutIterator};
use core::ops::Range;
use core::ptr;

impl<K: SortKey + Send + Sync> BinLayout<K> {
    #[inline]
    pub(super) fn with_cpu_count<T, F>(cpu: usize, slice: &[T], key: F) -> Option<Self>
    where
        T: Copy + Send + Sync,
        F: KeyFn<T, K> + Send + Sync,
    {
        let (min_key, max_key) = slice.par_min_max(key);

        if min_key == max_key {
            // array is flat
            return None;
        }

        let max_bins_count = cpu.saturating_mul(4).min(MAX_BINS_COUNT);

        Some(BinLayout::with_constraints(min_key, max_key, LayoutConstraints { max_split_count: max_bins_count }))
    }
}

trait ParMinMax<T> {
    fn par_min_max<K, F>(&self, key: F) -> (K, K)
    where
        K: Copy + Ord + Send + Sync,
        T: Copy + Send + Sync,
        F: KeyFn<T, K> + Send + Sync;
}

impl<T> ParMinMax<T> for [T] {
    #[inline(always)]
    fn par_min_max<K, F>(&self, key: F) -> (K, K)
    where
        K: Copy + Ord + Send + Sync,
        T: Copy + Send + Sync,
        F: KeyFn<T, K> + Send + Sync,
    {
        debug_assert!(!self.is_empty());
        let first_val = self.first().unwrap();
        let k0 = key(first_val);

        let (min_key, max_key) = self
            .par_iter()
            .map(|v| {
                let k = key(v);
                (k, k)
            })
            .reduce(|| (k0, k0), |a, b| (a.0.min(b.0), a.1.max(b.1)));

        (min_key, max_key)
    }
}

impl<K: SortKey + Send + Sync> BinLayout<K> {
    pub(super) fn par_pre_sort<T, F>(
        &self,
        cpu: usize,
        src: &mut [T],
        buf: &mut Vec<T>,
        key: F,
    ) -> Vec<usize>
    where
        T: Copy + Send + Sync,
        F: KeyFn<T, K> + Send + Sync,
    {
        buf.clear();
        if buf.capacity() < src.len() {
            buf.reserve(src.len());
        }

        let mut fragments = src.fragment_by_count(buf, cpu);

        let groups = self.par_spread(&mut fragments, key);

        // at this time buffer is fully initialized
        #[allow(clippy::uninit_vec)]
        unsafe { buf.set_len(src.len()); }

        copy_groups(src, buf, groups)
    }

    #[inline]
    fn par_spread<T, F>(&self, fragments: &mut [PreSortFragment<T>], key: F) -> Vec<Vec<Range<usize>>>
    where
        T: Copy + Send + Sync,
        F: KeyFn<T, K> + Send + Sync,
    {
        let bins_count = self.count();
        let frags_count = fragments.len();

        let id_ranges = fragments
            .par_iter_mut()
            .map(|f| f.spread(self.clone(), key))
            .reduce(
                || Vec::<IdRange>::with_capacity(frags_count * bins_count),
                |mut a, mut b| {
                    a.append(&mut b);
                    a
                },
            );

        let mut counter = vec![0usize; bins_count];
        for range in id_ranges.iter() {
            counter[range.index] += 1;
        }

        let mut groups: Vec<Vec<Range<usize>>> = (0..bins_count).map(|_| Vec::new()).collect();
        for (group, &count) in groups.iter_mut().zip(counter.iter()) {
            if count > 0 {
                *group = Vec::with_capacity(count);
            }
        }

        for id_range in id_ranges {
            unsafe {
                groups
                    .get_unchecked_mut(id_range.index)
                    .push(id_range.range);
            }
        }

        groups.retain(|g| !g.is_empty());
        groups
    }
}

#[inline]
fn copy_groups<T>(mut dst: &mut [T], src: &[T], mut groups: Vec<Vec<Range<usize>>>) -> Vec<usize> {
    let mut marks = Vec::with_capacity(groups.len());
    let last_group = if let Some(last) = groups.pop() {
        last
    } else {
        return marks;
    };

    let mut base = 0;
    for ranges in groups.iter() {
        let length = copy_ranges(dst, src, ranges);
        (_, dst) = dst.split_at_mut(length);

        base += length;
        marks.push(base);
    }

    let _ = copy_ranges(dst, src, &last_group);

    marks
}

#[inline]
fn copy_ranges<T>(dst: &mut [T], src: &[T], ranges: &[Range<usize>]) -> usize {
    let mut offset = 0;
    let dst_base = dst.as_mut_ptr();
    let src_base = src.as_ptr();
    for range in ranges.iter() {
        unsafe {
            let dst_ptr = dst_base.add(offset);
            let src_ptr = src_base.add(range.start);
            ptr::copy_nonoverlapping(src_ptr, dst_ptr, range.len());
        }

        offset += range.len();
    }

    offset
}
