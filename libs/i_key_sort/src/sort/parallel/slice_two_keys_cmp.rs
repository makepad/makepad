use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use crate::sort::parallel::cpu_count::CPUCount;
use crate::sort::parallel::slice_one_key_cmp::OneKeyBinSortCmpParallel;
use crate::sort::parallel::sub_sort::{FragmentationByMarks, SubSortFragment};
use crate::sort::serial::slice_two_keys_cmp::TwoKeysBinSortCmpSerial;
use rayon::prelude::*;

pub(crate) trait TwoKeysBinSortCmpParallel<T> {
    fn par_sort_by_two_keys_then_by<K1, K2, F1, F2, F3>(
        &mut self,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
        F3: CmpFn<T> + Send + Sync;
}

impl<T: Copy + Send + Sync> TwoKeysBinSortCmpParallel<T> for [T] {
    #[inline]
    fn par_sort_by_two_keys_then_by<K1, K2, F1, F2, F3>(
        &mut self,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
        compare: F3,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
        F3: CmpFn<T> + Send + Sync,
    {
        if self.is_empty() {
            return;
        }

        let cpu = if let Some(count) = CPUCount::should_parallel(self.len()) {
            count
        } else {
            self.ser_sort_by_two_keys_then_by_and_uninit_buffer(
                reusable_buffer,
                key1,
                key2,
                compare,
            );
            return;
        };

        let layout = if let Some(layout) = BinLayout::with_cpu_count(cpu, self, key1) {
            layout
        } else {
            // array is flat by key1
            // sort it by key2 and cmp
            self.par_sort_by_one_key_then_by(reusable_buffer, key2, compare);
            return;
        };

        let marks = layout.par_pre_sort(cpu, self, reusable_buffer, key1);

        self.fragment_by_marks(reusable_buffer, &marks)
            .par_iter_mut()
            .for_each(|f| f.sort_by_two_keys_then_by(key1, key2, compare));
    }
}

impl<T> SubSortFragment<'_, T>
where
    T: Send + Copy,
{
    #[inline]
    fn sort_by_two_keys_then_by<K1, K2, F1, F2, F3>(&mut self, key1: F1, key2: F2, compare: F3)
    where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
        F3: CmpFn<T>,
    {
        self.src
            .ser_sort_by_two_keys_then_by_and_buffer(self.buf, key1, key2, compare, true);
    }
}
#[cfg(test)]
mod tests {
    use crate::sort::parallel::slice_two_keys_cmp::TwoKeysBinSortCmpParallel;

    #[test]
    fn test_0() {
        test(2);
    }

    #[test]
    fn test_1() {
        test(5);
    }

    #[test]
    fn test_2() {
        test(10);
    }

    #[test]
    fn test_3() {
        test(20);
    }

    #[test]
    fn test_4() {
        test(40);
    }

    #[test]
    fn test_5() {
        test(100);
    }

    fn test(count: usize) {
        let mut org: Vec<_> = reversed_2d_array(count);
        let mut arr = org.clone();
        arr.par_sort_by_two_keys_then_by(&mut Vec::new(), |a| a.0, |a| a.1, |a, b| a.2.cmp(&b.2));
        org.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        assert!(arr == org);
    }

    fn reversed_2d_array(count: usize) -> Vec<(u32, i32, i32)> {
        let mut arr = Vec::with_capacity(count * count * count);
        for i in (0..count as u32).rev() {
            for x in (0..count as i32).rev() {
                for y in (0..count as i32).rev() {
                    arr.push((i, x, y))
                }
            }
        }

        arr
    }
}
