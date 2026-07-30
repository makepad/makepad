use crate::seg::heap::Heap32;
use crate::{LayoutNumber, LayoutUInt};

pub(super) struct Layout<R> {
    min: R,
    max: R,
    scale: u32,
}

impl<R: LayoutNumber> Layout<R> {
    #[inline]
    pub(super) fn new(start: R, end: R) -> Option<Self> {
        let min = start;
        let max = end;
        let span = R::range_span(min, max)?;
        if span < R::UInt::HEAP_MIN_SPAN {
            return None;
        }
        let p = span.ilog2() + 1;
        if p < Heap32::POWER {
            return None;
        }
        let scale = p - Heap32::POWER;

        Some(Self { min, max, scale })
    }

    #[inline]
    pub(super) fn index(&self, value: R) -> u32 {
        (value.offset_from(self.min) >> self.scale).to_u32()
    }

    #[inline]
    pub(super) fn count(&self) -> usize {
        let order = self.index(self.max);
        Heap32::order_to_heap_index(order) as usize + 1
    }

    #[inline]
    pub(super) fn insert_mask(&self, min: R, max: R) -> u64 {
        let start = self.index(min);
        let end = self.index(max);

        Heap32::range_to_place_mask(start, end)
    }

    #[inline]
    pub(super) fn intersect_mask(&self, min: R, max: R) -> u64 {
        let start = self.index(min);
        let end = self.index(max);

        Heap32::range_to_intersect_mask(start, end)
    }
}

#[cfg(test)]
mod tests {
    use crate::seg::layout::Layout;

    #[test]
    fn test_00() {
        let layout = Layout::new(0, 31).unwrap();
        for i in 0..31 {
            assert_eq!(layout.index(i), i as u32);
        }
    }

    #[test]
    fn test_01() {
        let layout = Layout::new(0, 63).unwrap();
        for i in 0..63 {
            assert_eq!(layout.index(i), (i / 2) as u32);
        }
    }

    #[test]
    fn test_02() {
        let layout = Layout::new(-63, 0).unwrap();
        for i in -63..0 {
            assert_eq!(layout.index(i), ((i + 63) / 2) as u32);
        }
    }

    #[test]
    fn test_03() {
        let layout = Layout::new(-10240, 15360).unwrap();
        let m0 = layout.insert_mask(-10240, 10240);
        let m1 = layout.intersect_mask(-10240, -10240);
        let inter = m0 & m1;

        assert_ne!(inter, 0);
    }

    #[test]
    fn test_full_i32_range() {
        let layout = Layout::new(i32::MIN, i32::MAX).unwrap();
        assert_eq!(layout.index(i32::MIN), 0);
        assert_eq!(layout.index(i32::MAX), 31);
    }

    #[test]
    fn test_full_i64_range() {
        let layout = Layout::new(i64::MIN, i64::MAX).unwrap();
        assert_eq!(layout.index(i64::MIN), 0);
        assert_eq!(layout.index(i64::MAX), 31);
    }
}
