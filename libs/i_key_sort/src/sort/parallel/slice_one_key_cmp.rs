use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{CmpFn, KeyFn, SortKey};
use crate::sort::parallel::cpu_count::CPUCount;
use crate::sort::serial::slice_one_key_cmp::OneKeyBinSortCmpSerial;
use rayon::prelude::*;
use crate::sort::parallel::sub_sort::{FragmentationByMarks, SubSortFragment};

pub(crate) trait OneKeyBinSortCmpParallel<T> {
    fn par_sort_by_one_key_then_by<K, F1, F2>(
        &mut self,
        reusable_buffer: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        K: SortKey + Send + Sync,
        F1: KeyFn<T, K> + Send + Sync,
        F2: CmpFn<T> + Send + Sync;
}

impl<T: Copy + Send + Sync> OneKeyBinSortCmpParallel<T> for [T] {
    #[inline]
    fn par_sort_by_one_key_then_by<K, F1, F2>(
        &mut self,
        reusable_buffer: &mut Vec<T>,
        key: F1,
        compare: F2,
    ) where
        K: SortKey + Send + Sync,
        F1: KeyFn<T, K> + Send + Sync,
        F2: CmpFn<T> + Send + Sync,
    {
        if self.is_empty() {
            return;
        }

        let cpu = if let Some(count) = CPUCount::should_parallel(self.len()) {
            count
        } else {
            self.ser_sort_by_one_key_then_by_and_uninit_buffer(reusable_buffer, key, compare);
            return;
        };

        let layout = if let Some(layout) = BinLayout::with_cpu_count(cpu, self, key) {
            layout
        } else {
            // array is flat by key
            self.par_sort_unstable_by(compare);
            return;
        };

        let marks = layout.par_pre_sort(cpu, self, reusable_buffer, key);

        self.fragment_by_marks(reusable_buffer, &marks)
            .par_iter_mut()
            .for_each(|f| f.sort_by_one_key_then_by(key, compare));
    }
}

impl<T> SubSortFragment<'_, T>
where
    T: Send + Copy,
{
    #[inline]
    fn sort_by_one_key_then_by<K, F1, F2>(&mut self, key: F1, compare: F2)
    where
        K: SortKey,
        F1: KeyFn<T, K>,
        F2: CmpFn<T>,
    {
        self.src
            .ser_sort_by_one_key_then_by_and_buffer(self.buf, key, compare, true);
    }
}

#[cfg(test)]
mod tests {
    use crate::sort::parallel::slice_one_key_cmp::OneKeyBinSortCmpParallel;

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
        test(30);
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
        arr.par_sort_by_one_key_then_by(&mut Vec::new(), |a| a.0, |a, b| a.1.cmp(&b.1));
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
