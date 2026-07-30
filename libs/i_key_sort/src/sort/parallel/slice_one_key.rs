use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::parallel::cpu_count::CPUCount;
use crate::sort::parallel::sub_sort::{FragmentationByMarks, SubSortFragment};
use crate::sort::serial::slice_one_key::OneKeyBinSortSerial;
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefMutIterator;

pub(crate) trait OneKeyBinSortParallel<T> {
    fn par_sort_by_one_key<K, F>(&mut self, reusable_buffer: &mut Vec<T>, key: F)
    where
        K: SortKey + Send + Sync,
        F: KeyFn<T, K> + Send + Sync;
}

impl<T: Copy + Send + Sync> OneKeyBinSortParallel<T> for [T] {
    #[inline]
    fn par_sort_by_one_key<K, F>(&mut self, reusable_buffer: &mut Vec<T>, key: F)
    where
        K: SortKey + Send + Sync,
        F: KeyFn<T, K> + Send + Sync,
    {
        if self.is_empty() {
            return;
        }

        let cpu = if let Some(count) = CPUCount::should_parallel(self.len()) {
            count
        } else {
            self.ser_sort_by_one_key_and_uninit_buffer(reusable_buffer, key);
            return;
        };

        let layout = if let Some(layout) = BinLayout::with_cpu_count(cpu, self, key) {
            layout
        } else {
            // array is flat
            return;
        };

        let marks = layout.par_pre_sort(cpu, self, reusable_buffer, key);

        let mut frags = self.fragment_by_marks(reusable_buffer, &marks);

        frags.par_iter_mut().for_each(|f| f.sort_by_one_key(key));
    }
}

impl<T> SubSortFragment<'_, T>
where
    T: Send + Copy,
{
    #[inline]
    fn sort_by_one_key<K, F>(&mut self, key: F)
    where
        K: SortKey,
        F: KeyFn<T, K>,
    {
        self.src.ser_sort_by_one_key_and_buffer(self.buf, key, true);
    }
}

#[cfg(test)]
mod tests {
    use crate::sort::parallel::slice_one_key::OneKeyBinSortParallel;

    #[test]
    fn test_0() {
        test(10);
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
        arr.par_sort_by_one_key(&mut Vec::new(), |&a| a);
        org.sort_unstable();
        assert!(arr == org);
    }
}
