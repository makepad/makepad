use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::uint::UIntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;
use i_shape::int::path::ContourExtension;
use i_shape::int::shape::IntContour;
use i_shape::util::reserve::Reserve;

pub(super) trait Split<I: IntNumber> {
    fn split_loops(
        self,
        min_area: I::WideUInt,
        contour_buffer: &mut IntContour<I>,
        bin_store: &mut BinStore<I>,
    ) -> Vec<Self>
    where
        Self: Sized;
}

impl<I: IntNumber> Split<I> for IntContour<I> {
    fn split_loops(
        self,
        min_area: I::WideUInt,
        contour_buffer: &mut IntContour<I>,
        bin_store: &mut BinStore<I>,
    ) -> Vec<Self> {
        if self.is_empty() {
            return Vec::new();
        }
        contour_buffer.reserve_capacity(self.len());
        contour_buffer.clear();

        bin_store.init(&self);

        let mut result: Vec<IntContour<I>> = Vec::with_capacity(16);

        for point in self {
            let next_pos = contour_buffer.len() + 1;
            let pos = bin_store.insert_if_not_exist(point, next_pos);
            if pos < contour_buffer.len() {
                // found a loop
                let tail_len = contour_buffer.len() - pos;
                if tail_len < 2 {
                    // tail is too small
                    contour_buffer.truncate(pos);
                } else {
                    let mut tail = contour_buffer.split_off(pos);
                    tail.push(point);
                    if tail.validate_area(min_area) {
                        result.push(tail);
                    }
                }
            } else {
                contour_buffer.push(point);
            }
        }

        if contour_buffer.len() > 2 {
            result.push(contour_buffer.as_slice().to_vec());
        }

        result
    }
}

#[derive(Clone)]
struct PointItem<I: IntNumber> {
    point: IntPoint<I>,
    pos: usize,
}

#[derive(Clone)]
struct Bin {
    offset: usize,
    data: usize,
}

pub(super) struct BinStore<I: IntNumber> {
    mask: u32,
    bins: Vec<Bin>,
    items: Vec<PointItem<I>>,
}

impl<I: IntNumber> BinStore<I> {
    pub(super) fn new() -> Self {
        Self {
            mask: 0,
            bins: Vec::new(),
            items: Vec::new(),
        }
    }

    fn init(&mut self, contour: &IntContour<I>) {
        let log = contour.len().ilog2().saturating_sub(4).clamp(1, 30);
        let bins_count = (1 << log) as usize;

        self.bins.clear();
        self.bins.resize(bins_count, Bin { offset: 0, data: 0 });

        self.items.clear();
        self.items.resize(
            contour.len(),
            PointItem {
                point: IntPoint::EMPTY,
                pos: 0,
            },
        );

        self.mask = bins_count.wrapping_sub(1) as u32;

        for &p in contour.iter() {
            let index = self.bin_index(p);
            unsafe {
                // SAFETY: bin_index comes from bin_index(point); mask == bins_count - 1 with bins sized to bins_count, so this lookup is always in-bounds.
                self.bins.get_unchecked_mut(index).data += 1
            };
        }

        let mut offset = 0;
        for bin in self.bins.iter_mut() {
            let next_offset = offset + bin.data;
            *bin = Bin { offset, data: offset };
            offset = next_offset;
        }
    }

    #[inline]
    fn insert_if_not_exist(&mut self, point: IntPoint<I>, pos: usize) -> usize {
        let index = self.bin_index(point);
        let bin = unsafe {
            // SAFETY: insert_if_not_exist only touches bins within the pre-sized array; bin_index shares the same invariant.
            self.bins.get_unchecked_mut(index)
        };
        let start = bin.offset;
        let end = bin.data;
        for i in start..end {
            let item = unsafe {
                // SAFETY: items is pre-resized to contour.len(); start..end stays inside that range while we probe, so the mutable borrow is valid.
                self.items.get_unchecked_mut(i)
            };
            if item.point == point {
                return item.pos;
            }
        }
        bin.data = end + 1;
        unsafe {
            // SAFETY: end < items.len() because we grow data one slot at a time inside the reserved window.
            *self.items.get_unchecked_mut(end) = PointItem { point, pos }
        }

        usize::MAX
    }

    #[inline]
    fn bin_index(&self, p: IntPoint<I>) -> usize {
        let x = p.x.wide().wrapping_mul(I::Wide::from_usize(31));
        let y = p.y.wide().wrapping_mul(I::Wide::from_usize(17));
        let hash = x.wrapping_add(y);
        (hash & I::Wide::from_usize(self.mask as usize)).to_usize()
    }
}

trait ValidateArea<I: IntNumber> {
    fn validate_area(&self, min_area: I::WideUInt) -> bool;
}

impl<I: IntNumber> ValidateArea<I> for IntContour<I> {
    #[inline]
    fn validate_area(&self, min_area: I::WideUInt) -> bool {
        if min_area == I::WideUInt::ZERO {
            return true;
        }
        let abs_area = self.unsafe_area().unsigned_abs() >> 1;
        abs_area < min_area
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i_shape::int::path::IntPath;
    use alloc::vec;

    #[test]
    fn test_empty_path() {
        let path: IntPath<i32> = vec![];
        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.split_loops(0, &mut contour, &mut bin_store);
        assert_eq!(result, vec![] as Vec<IntPath<i32>>);
    }

    #[test]
    fn test_single_point() {
        let path = vec![IntPoint::new(0, 0)];
        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.split_loops(0, &mut contour, &mut bin_store);
        assert!(result.is_empty());
    }

    #[test]
    fn test_two_points() {
        let path = vec![IntPoint::new(0, 0), IntPoint::new(1, 1)];
        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.split_loops(0, &mut contour, &mut bin_store);
        assert!(result.is_empty());
    }

    #[test]
    fn test_no_repeated_points() {
        let path = vec![
            IntPoint::new(0, 0),
            IntPoint::new(0, 1),
            IntPoint::new(1, 1),
            IntPoint::new(1, 0),
        ];

        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.clone().split_loops(0, &mut contour, &mut bin_store);
        assert_eq!(result, vec![path]);
    }

    #[test]
    fn test_2_loops_0() {
        let path = vec![
            IntPoint::new(0, 0),
            IntPoint::new(1, 1),
            IntPoint::new(2, 0),
            IntPoint::new(3, 1),
            IntPoint::new(4, 0),
            IntPoint::new(3, -1),
            IntPoint::new(2, 0), // same point
            IntPoint::new(1, -1),
        ];

        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.split_loops(0, &mut contour, &mut bin_store);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            [
                IntPoint::new(3, 1),
                IntPoint::new(4, 0),
                IntPoint::new(3, -1),
                IntPoint::new(2, 0),
            ]
            .to_vec()
        );
        assert_eq!(
            result[1],
            [
                IntPoint::new(0, 0),
                IntPoint::new(1, 1),
                IntPoint::new(2, 0),
                IntPoint::new(1, -1),
            ]
            .to_vec()
        );
    }

    #[test]
    fn test_2_loops_1() {
        let path = vec![
            IntPoint::new(0, 0),
            IntPoint::new(1, 1),
            IntPoint::new(2, 0),
            IntPoint::new(3, 1),
            IntPoint::new(3, -1),
            IntPoint::new(2, 0), // same point
            IntPoint::new(1, -1),
        ];

        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.split_loops(0, &mut contour, &mut bin_store);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            [IntPoint::new(3, 1), IntPoint::new(3, -1), IntPoint::new(2, 0),].to_vec()
        );
        assert_eq!(
            result[1],
            [
                IntPoint::new(0, 0),
                IntPoint::new(1, 1),
                IntPoint::new(2, 0),
                IntPoint::new(1, -1),
            ]
            .to_vec()
        );
    }

    #[test]
    fn test_2_loops_with_tails() {
        let path = vec![
            IntPoint::new(-1, 0),
            IntPoint::new(0, 0),
            IntPoint::new(1, 1),
            IntPoint::new(2, 0),
            IntPoint::new(3, 1),
            IntPoint::new(4, 0),
            IntPoint::new(5, 0),
            IntPoint::new(4, 0),
            IntPoint::new(3, -1),
            IntPoint::new(2, 0), // same point
            IntPoint::new(1, -1),
            IntPoint::new(0, 0),
        ];

        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.split_loops(0, &mut contour, &mut bin_store);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            [
                IntPoint::new(3, 1),
                IntPoint::new(4, 0),
                IntPoint::new(3, -1),
                IntPoint::new(2, 0),
            ]
            .to_vec()
        );
        assert_eq!(
            result[1],
            [
                IntPoint::new(1, 1),
                IntPoint::new(2, 0),
                IntPoint::new(1, -1),
                IntPoint::new(0, 0),
            ]
            .to_vec()
        );
    }

    #[test]
    fn test_single_loop() {
        let path = vec![
            IntPoint::new(0, 0),
            IntPoint::new(1, 1),
            IntPoint::new(2, 0),
            IntPoint::new(0, 0), // same point, forms a loop
        ];

        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.split_loops(0, &mut contour, &mut bin_store);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            [IntPoint::new(1, 1), IntPoint::new(2, 0), IntPoint::new(0, 0),].to_vec()
        );
    }

    #[test]
    fn test_cross() {
        let path = vec![
            IntPoint::new(-2, -1),
            IntPoint::new(-2, 1),
            IntPoint::new(0, 0),
            IntPoint::new(-1, 2),
            IntPoint::new(1, 2),
            IntPoint::new(0, 0), // same point, forms a loop
            IntPoint::new(2, 1),
            IntPoint::new(2, -1),
            IntPoint::new(0, 0), // same point, forms a loop
            IntPoint::new(1, -2),
            IntPoint::new(-1, -2),
            IntPoint::new(0, 0), // same point, forms a loop
        ];

        let mut contour: IntContour<i32> = Vec::new();
        let mut bin_store = BinStore::new();
        let result = path.split_loops(0, &mut contour, &mut bin_store);
        assert_eq!(result.len(), 4);
        assert_eq!(
            result[0],
            [IntPoint::new(-1, 2), IntPoint::new(1, 2), IntPoint::new(0, 0),].to_vec()
        );
        assert_eq!(
            result[1],
            [IntPoint::new(2, 1), IntPoint::new(2, -1), IntPoint::new(0, 0),].to_vec()
        );
        assert_eq!(
            result[2],
            [IntPoint::new(1, -2), IntPoint::new(-1, -2), IntPoint::new(0, 0),].to_vec()
        );
        assert_eq!(
            result[3],
            [IntPoint::new(-2, -1), IntPoint::new(-2, 1), IntPoint::new(0, 0),].to_vec()
        );
    }
}
