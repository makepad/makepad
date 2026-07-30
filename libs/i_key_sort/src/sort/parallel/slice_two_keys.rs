use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::parallel::cpu_count::CPUCount;
use crate::sort::parallel::slice_one_key::OneKeyBinSortParallel;
use crate::sort::serial::slice_two_keys::TwoKeysBinSortSerial;
use rayon::prelude::*;
use crate::sort::parallel::sub_sort::{FragmentationByMarks, SubSortFragment};

pub(crate) trait TwoKeysBinSortParallel<T> {
    fn par_sort_by_two_keys<K1, K2, F1, F2>(
        &mut self,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync;
}

impl<T: Copy + Send + Sync> TwoKeysBinSortParallel<T> for [T] {
    #[inline]
    fn par_sort_by_two_keys<K1, K2, F1, F2>(
        &mut self,
        reusable_buffer: &mut Vec<T>,
        key1: F1,
        key2: F2,
    ) where
        K1: SortKey + Send + Sync,
        K2: SortKey + Send + Sync,
        F1: KeyFn<T, K1> + Send + Sync,
        F2: KeyFn<T, K2> + Send + Sync,
    {
        if self.is_empty() {
            return;
        }

        let cpu = if let Some(count) = CPUCount::should_parallel(self.len()) {
            count
        } else {
            self.ser_sort_by_two_keys_and_uninit_buffer(reusable_buffer, key1, key2);
            return;
        };

        let layout = if let Some(layout) = BinLayout::with_cpu_count(cpu, self, key1) {
            layout
        } else {
            // array is flat by key1
            // sort it by key2
            self.par_sort_by_one_key(reusable_buffer, key2);
            return;
        };

        let marks = layout.par_pre_sort(cpu, self, reusable_buffer, key1);

        let mut frags = self.fragment_by_marks(reusable_buffer, &marks);

        frags
            .par_iter_mut()
            .for_each(|f| f.sort_by_two_keys(key1, key2));
    }
}

impl<T> SubSortFragment<'_, T>
where
    T: Send + Copy,
{
    #[inline]
    fn sort_by_two_keys<K1, K2, F1, F2>(&mut self, key1: F1, key2: F2)
    where
        K1: SortKey,
        K2: SortKey,
        F1: KeyFn<T, K1>,
        F2: KeyFn<T, K2>,
    {
        self.src
            .ser_sort_by_two_keys_and_buffer(self.buf, key1, key2, true);
    }
}

#[cfg(test)]
mod tests {
    use crate::sort::parallel::slice_two_keys::TwoKeysBinSortParallel;

    #[test]
    fn test_0() {
        test(2);
    }

    #[test]
    fn test_1() {
        test(10);
    }

    #[test]
    fn test_2() {
        test(25);
    }

    #[test]
    fn test_3() {
        test(100);
    }

    #[test]
    fn test_4() {
        test(300);
    }

    #[test]
    fn test_5() {
        test(1000);
    }

    fn test(count: usize) {
        let mut org: Vec<_> = reversed_2d_array(count);
        let mut arr = org.clone();
        arr.par_sort_by_two_keys(&mut Vec::new(), |a| a.0, |a| a.1);
        org.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        assert!(arr == org);
    }

    fn reversed_2d_array(count: usize) -> Vec<(i32, i32)> {
        let mut arr = Vec::with_capacity(count * count);
        for x in (0..count as i32).rev() {
            for y in (0..count as i32).rev() {
                arr.push((x, y))
            }
        }

        arr
    }
}
