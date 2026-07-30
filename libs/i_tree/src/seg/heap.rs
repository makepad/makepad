use crate::seg::bit::BitOp;

pub(super) struct Heap32 {}

impl Heap32 {
    pub(super) const SUB_CAPACITY: u32 = 32 - 1;
    pub(super) const POWER: u32 = 32_u32.ilog2();

    #[inline]
    pub(super) fn range_to_intersect_mask(start: u32, end: u32) -> u64 {
        debug_assert!(start < 32);
        debug_assert!(end < 32);

        let mut w = Self::range_to_fill_mask(start, end);

        let mut shift = 32;
        for _ in 0..6 {
            let mut lt = shift - 1;
            shift >>= 1; // 16
            for _ in 0..shift {
                let rt = lt + 1;
                let pt = lt >> 1;

                let lt_bit = (w >> lt) & 1;
                let rt_bit = (w >> rt) & 1;
                let pt_bit = lt_bit | rt_bit;

                w |= pt_bit << pt;

                lt += 2;
            }
        }

        w
    }

    #[inline]
    pub(super) fn range_to_place_mask(start: u32, end: u32) -> u64 {
        debug_assert!(start < 32);
        debug_assert!(end < 32);

        if end - start == 31 {
            return 1;
        }

        let mut w = Self::range_to_fill_mask(start, end);

        let mut m: u64 = 0;
        let mut shift = 32;
        for _ in 0..6 {
            let mut lt = shift - 1;
            shift >>= 1; // 16

            for _ in 0..shift {
                let rt = lt + 1;
                let pt = lt >> 1;

                let lt_bit = (w >> lt) & 1;
                let rt_bit = (w >> rt) & 1;
                let pt_bit = lt_bit & rt_bit;

                w |= pt_bit << pt;
                m |= (lt_bit ^ pt_bit) << lt;
                m |= (rt_bit ^ pt_bit) << rt;

                lt += 2;
            }
        }

        m
    }

    #[inline]
    fn range_to_fill_mask(start: u32, end: u32) -> u64 {
        let i0 = Self::order_to_heap_index(start);
        let i1 = Self::order_to_heap_index(end);
        u64::fill(i0, i1)
    }

    #[inline]
    pub(super) fn order_to_heap_index(order: u32) -> u32 {
        order + Self::SUB_CAPACITY
    }
}

pub(super) struct BitIter {
    value: u64,
}

impl BitIter {
    #[inline]
    pub(super) fn new(value: u64) -> Self {
        Self { value }
    }
}

impl Iterator for BitIter {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.value == 0 {
            return None;
        }
        let pos = self.value.trailing_zeros() as usize;
        self.value &= self.value - 1;
        Some(pos)
    }
}

#[cfg(test)]
mod tests {
    use crate::seg::heap::{BitIter, Heap32};
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn test_00() {
        let m = Heap32::range_to_place_mask(0, 31);
        let indices: Vec<_> = BitIter::new(m).collect();
        assert_eq!(indices, vec![0]);
    }

    #[test]
    fn test_01() {
        let m = Heap32::range_to_place_mask(0, 30);
        let indices: Vec<_> = BitIter::new(m).collect();
        assert_eq!(indices, vec![1, 5, 13, 29, 61]);
    }

    #[test]
    fn test_02() {
        let m = Heap32::range_to_place_mask(30, 31);
        let indices: Vec<_> = BitIter::new(m).collect();
        assert_eq!(indices, vec![30]);
    }

    #[test]
    fn test_03() {
        let m = Heap32::range_to_place_mask(29, 31);
        let mut indices: Vec<_> = BitIter::new(m).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![30, 60]);
    }

    #[test]
    fn test_04() {
        let m = Heap32::range_to_place_mask(15, 16);
        let mut indices: Vec<_> = BitIter::new(m).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![46, 47]);
    }

    #[test]
    fn test_05() {
        let m = Heap32::range_to_place_mask(0, 12);
        let mut indices: Vec<_> = BitIter::new(m).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![3, 9, 43]);
    }

    #[test]
    fn test_06() {
        let m = Heap32::range_to_intersect_mask(0, 0);
        let mut indices: Vec<_> = BitIter::new(m).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![0, 1, 3, 7, 15, 31]);
    }

    #[test]
    fn test_07() {
        let m = Heap32::range_to_intersect_mask(1, 1);
        let mut indices: Vec<_> = BitIter::new(m).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![0, 1, 3, 7, 15, 32]);
    }

    #[test]
    fn test_08() {
        let m = Heap32::range_to_intersect_mask(0, 1);
        let mut indices: Vec<_> = BitIter::new(m).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![0, 1, 3, 7, 15, 31, 32]);
    }

    #[test]
    fn test_09() {
        let m = Heap32::range_to_intersect_mask(0, 31);
        let mut indices: Vec<_> = BitIter::new(m).collect();
        indices.sort_unstable();
        let template: Vec<_> = (0..63).collect();
        assert_eq!(indices, template);
    }
}
