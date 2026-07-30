use crate::core::edge_data::{EdgeDataMerge, OverlayEdgeData};
use crate::segm::segment::Segment;
use crate::segm::winding::WindingCount;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;

pub(crate) trait ShapeSegmentsMerge<C, D>
where
    C: WindingCount,
    D: OverlayEdgeData<C>,
{
    #[cfg(test)]
    fn merge_if_needed(&mut self) -> bool;
    fn merge_if_needed_with_store(&mut self, store: &mut D::Store) -> bool;
}

impl<I: IntNumber, C: WindingCount, D: OverlayEdgeData<C>> ShapeSegmentsMerge<C, D>
    for Vec<Segment<C, I, D>>
{
    #[cfg(test)]
    fn merge_if_needed(&mut self) -> bool {
        let mut store = D::Store::default();
        self.merge_if_needed_with_store(&mut store)
    }

    fn merge_if_needed_with_store(&mut self, store: &mut D::Store) -> bool {
        if self.len() < 2 {
            return false;
        }

        let mut prev = &self[0].x_segment;
        for i in 1..self.len() {
            let this = &self[i].x_segment;
            if prev.eq(this) {
                let new_len = merge(self, i, store);
                self.truncate(new_len);
                return true;
            }
            prev = this;
        }

        false
    }
}

fn merge<I: IntNumber, C: WindingCount, D: OverlayEdgeData<C>>(
    segments: &mut [Segment<C, I, D>],
    after: usize,
    store: &mut D::Store,
) -> usize {
    let mut i = after;
    let mut j = i - 1;
    let mut prev = segments[j];

    while i < segments.len() {
        if prev.x_segment.eq(&segments[i].x_segment) {
            let lhs_count = prev.count;
            let rhs_count = segments[i].count;
            let out_count = lhs_count.add(rhs_count);
            prev.data = D::merge(
                EdgeDataMerge {
                    lhs_data: prev.data,
                    lhs_count,
                    rhs_data: segments[i].data,
                    rhs_count,
                    out_count,
                },
                store,
            );
            prev.count = out_count;
        } else {
            if prev.count.is_not_empty() {
                segments[j] = prev;
                j += 1;
            }
            prev = segments[i];
        }
        i += 1;
    }

    if prev.count.is_not_empty() {
        segments[j] = prev;
        j += 1;
    }

    j
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segm::boolean::ShapeCountBoolean;
    use alloc::vec;
    use i_float::int::point::IntPoint;

    impl<C: WindingCount, I: IntNumber> Segment<C, I> {
        fn create_and_validate(a: IntPoint<I>, b: IntPoint<I>, count: C) -> Self {
            let mut store = ();
            Self::create_and_validate_with_data(a, b, count, (), &mut store)
        }
    }

    #[test]
    fn test_merge_if_needed_empty() {
        let mut segments: Vec<Segment<ShapeCountBoolean, i32>> = Vec::new();
        segments.merge_if_needed();
        assert!(
            segments.is_empty(),
            "Empty vector should remain empty after merge"
        );
    }

    #[test]
    fn test_merge_if_needed_single_element() {
        let a = IntPoint::new(1, 2);
        let b = IntPoint::new(3, 4);
        let count = ShapeCountBoolean::new(1, 1);
        let segment = Segment::create_and_validate(a, b, count);
        let mut segments = vec![segment];
        segments.merge_if_needed();
        assert_eq!(segments.len(), 1, "Single segment should remain unchanged");
        assert_eq!(segments[0], segment, "Segment should be unchanged after merge");
    }

    #[test]
    fn test_merge_if_needed_no_merge() {
        let a1 = IntPoint::new(1, 2);
        let b1 = IntPoint::new(3, 4);
        let count1 = ShapeCountBoolean::new(1, 0);
        let segment1 = Segment::create_and_validate(a1, b1, count1);

        let a2 = IntPoint::new(5, 6);
        let b2 = IntPoint::new(7, 8);
        let count2 = ShapeCountBoolean::new(0, 1);
        let segment2 = Segment::create_and_validate(a2, b2, count2);

        let mut segments = vec![segment1, segment2];
        segments.merge_if_needed();

        assert_eq!(
            segments.len(),
            2,
            "Segments with different x_segments should not be merged"
        );
        assert_eq!(segments[0], segment1, "First segment should remain unchanged");
        assert_eq!(segments[1], segment2, "Second segment should remain unchanged");
    }

    #[test]
    fn test_merge_if_needed_single_merge() {
        let a1 = IntPoint::new(1, 2);
        let b1 = IntPoint::new(3, 4);
        let count1 = ShapeCountBoolean::new(1, 0);
        let segment1 = Segment::create_and_validate(a1, b1, count1);

        let a2 = IntPoint::new(1, 2);
        let b2 = IntPoint::new(3, 4);
        let count2 = ShapeCountBoolean::new(0, 1);
        let segment2 = Segment::create_and_validate(a2, b2, count2);

        let mut segments = vec![segment1, segment2];
        segments.merge_if_needed();

        assert_eq!(segments.len(), 1, "Segments should be merged into one");
        let merged_count = ShapeCountBoolean::new(1, 1);
        let expected_segment = Segment::create_and_validate(a1, b1, merged_count);
        assert_eq!(
            segments[0], expected_segment,
            "Merged segment should have combined counts"
        );
    }

    #[test]
    fn test_merge_if_needed_multiple_merges() {
        let a = IntPoint::new(1, 2);
        let b = IntPoint::new(3, 4);

        let count1 = ShapeCountBoolean::new(1, 0);
        let count2 = ShapeCountBoolean::new(0, 1);
        let count3 = ShapeCountBoolean::new(2, 2);

        let segment1 = Segment::create_and_validate(a, b, count1);
        let segment2 = Segment::create_and_validate(a, b, count2);
        let segment3 = Segment::create_and_validate(a, b, count3);

        let mut segments = vec![segment1, segment2, segment3];
        segments.merge_if_needed();

        assert_eq!(segments.len(), 1, "All segments should be merged into one");
        let merged_count = ShapeCountBoolean::new(3, 3);
        let expected_segment = Segment::create_and_validate(a, b, merged_count);
        assert_eq!(
            segments[0], expected_segment,
            "Merged segment should have combined counts"
        );
    }

    #[test]
    fn test_merge_if_needed_segments_with_inverted_order() {
        let a1 = IntPoint::new(3, 4);
        let b1 = IntPoint::new(1, 2);
        let count1 = ShapeCountBoolean::new(1, 0);
        // create_and_validate should order the points
        let segment1 = Segment::create_and_validate(a1, b1, count1);

        let a2 = IntPoint::new(1, 2);
        let b2 = IntPoint::new(3, 4);
        let count2 = ShapeCountBoolean::new(0, 1);
        let segment2 = Segment::create_and_validate(a2, b2, count2);

        let mut segments = vec![segment1, segment2];
        segments.merge_if_needed();

        // Both segments should have the same ordered x_segment
        assert_eq!(
            segments.len(),
            1,
            "Segments with inverted points should be merged"
        );

        let merged_count = ShapeCountBoolean::new(1, 1);
        let expected_segment =
            Segment::create_and_validate(IntPoint::new(1, 2), IntPoint::new(3, 4), merged_count);
        assert_eq!(
            segments[0], expected_segment,
            "Merged segment should have combined counts and ordered points"
        );
    }

    #[test]
    fn test_merge_if_needed_no_merge_different_x_segments() {
        let a1 = IntPoint::new(1, 1);
        let b1 = IntPoint::new(2, 2);
        let count1 = ShapeCountBoolean::new(1, 1);
        let segment1 = Segment::create_and_validate(a1, b1, count1);

        let a2 = IntPoint::new(3, 3);
        let b2 = IntPoint::new(4, 4);
        let count2 = ShapeCountBoolean::new(2, 2);
        let segment2 = Segment::create_and_validate(a2, b2, count2);

        let a3 = IntPoint::new(5, 5);
        let b3 = IntPoint::new(6, 6);
        let count3 = ShapeCountBoolean::new(3, 3);
        let segment3 = Segment::create_and_validate(a3, b3, count3);

        let mut segments = vec![segment1, segment2, segment3];
        segments.merge_if_needed();

        assert_eq!(
            segments.len(),
            3,
            "Segments with different x_segments should not be merged"
        );
        assert_eq!(segments[0], segment1, "First segment should remain unchanged");
        assert_eq!(segments[1], segment2, "Second segment should remain unchanged");
        assert_eq!(segments[2], segment3, "Third segment should remain unchanged");
    }
}
