use crate::build::sweep::FillHandler;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::{
    BOTH_BOTTOM, BOTH_TOP, CLIP_BOTH, CLIP_BOTTOM, CLIP_TOP, SUBJ_BOTH, SUBJ_BOTTOM, SUBJ_TOP, Segment,
    SegmentFill,
};
use alloc::vec::Vec;
use core::ops::ControlFlow;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_key_sort::sort::key::SortKey;
use i_key_sort::sort::two_keys::TwoKeysSort;

/// Collects segment endpoints and checks for coincidence between subject and clip.
///
/// Uses optimized algorithm: collect into separate Vecs, sort with `sort_by_two_keys`,
/// dedup, then binary search from shorter into longer array.
pub(crate) struct PointCoincidenceChecker<I: IntNumber> {
    subj_points: Vec<IntPoint<I>>,
    clip_points: Vec<IntPoint<I>>,
}

impl<I: IntNumber + SortKey> PointCoincidenceChecker<I> {
    /// Create a new checker with pre-allocated capacity.
    ///
    /// `capacity` is the number of segments; each segment contributes 2 endpoints.
    #[inline]
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            subj_points: Vec::with_capacity(capacity * 2),
            clip_points: Vec::with_capacity(capacity * 2),
        }
    }

    /// Add a segment's endpoints based on its count and fill.
    ///
    /// Uses fill to skip inner segments that can't contribute to boundary coincidence:
    /// - Segments entirely inside subject (SUBJ_BOTH, no clip contribution) with no
    ///   clip in the segment are skipped for clip collection
    /// - Similarly for clip-only interior segments
    #[inline]
    pub(crate) fn add_segment(&mut self, segment: &Segment<ShapeCountBoolean, I>, fill: SegmentFill) {
        // Skip inner segments optimization:
        // If segment is entirely inside one shape's interior (filled on both sides)
        // and has no contribution from the other shape, it's not on a boundary
        // where coincidence could occur.
        let subj_interior = (fill & SUBJ_BOTH) == SUBJ_BOTH;
        let clip_interior = (fill & CLIP_BOTH) == CLIP_BOTH;

        if subj_interior || clip_interior || fill == 0 {
            return;
        }

        let is_subj = fill & SUBJ_BOTH != 0;
        let is_clip = fill & CLIP_BOTH != 0;
        if is_subj && is_clip {
            // Segment belongs to both shapes (boundary contact) - this is a shared edge, not a point coincidence.
            return;
        }
        if is_subj {
            self.subj_points.push(segment.x_segment.a);
            self.subj_points.push(segment.x_segment.b);
        } else {
            debug_assert!(is_clip);
            self.clip_points.push(segment.x_segment.a);
            self.clip_points.push(segment.x_segment.b);
        }
    }

    /// Check if any subject point coincides with any clip point.
    ///
    /// Consumes self and returns true if coincidence found.
    ///
    /// Optimization: Only sort/dedup the shorter array, then iterate the longer
    /// array doing binary searches into the shorter. This minimizes total work:
    /// O(n log n) sort + O(m log n) searches, where n ≤ m.
    #[inline]
    pub(crate) fn has_coincidence(mut self) -> bool {
        if self.subj_points.is_empty() || self.clip_points.is_empty() {
            return false;
        }

        // Determine shorter/longer by pre-dedup size (good estimate of post-dedup)
        let (shorter, longer) = if self.subj_points.len() <= self.clip_points.len() {
            (&mut self.subj_points, &self.clip_points)
        } else {
            (&mut self.clip_points, &self.subj_points)
        };

        // Sort and dedup only the shorter array (binary search target)
        shorter.sort_by_two_keys(false, |p| p.x, |p| p.y);
        shorter.dedup();

        // Iterate longer (unsorted) and binary search into shorter
        longer.iter().any(|p| shorter.binary_search(p).is_ok())
    }
}

/// Handler that checks if subject and clip shapes intersect (share any point).
///
/// Returns `true` on the first segment where both shapes contribute fill,
/// indicating the geometries share at least one point (interior overlap or boundary contact).
/// This matches the DE-9IM definition of `intersects`.
///
/// This handler is designed for early-exit optimization - it breaks out of the sweep
/// loop as soon as an intersection is detected, avoiding processing of remaining segments.
///
/// Also collects endpoint information for point coincidence check in finalize.
pub(crate) struct IntersectsHandler<I: IntNumber> {
    point_checker: PointCoincidenceChecker<I>,
}

impl<I: IntNumber + SortKey> IntersectsHandler<I> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            point_checker: PointCoincidenceChecker::new(capacity),
        }
    }
}

impl<I: IntNumber + SortKey> FillHandler<ShapeCountBoolean, I> for IntersectsHandler<I> {
    type Output = bool;

    #[inline(always)]
    fn handle(
        &mut self,
        _index: usize,
        segment: &Segment<ShapeCountBoolean, I>,
        fill: SegmentFill,
    ) -> ControlFlow<bool> {
        // Shapes intersect if both contribute to any segment (interior overlap or boundary contact)
        let has_subj = (fill & SUBJ_BOTH) != 0;
        let has_clip = (fill & CLIP_BOTH) != 0;
        if has_subj && has_clip {
            ControlFlow::Break(true)
        } else {
            self.point_checker.add_segment(segment, fill);
            ControlFlow::Continue(())
        }
    }

    #[inline(always)]
    fn finalize(self) -> bool {
        self.point_checker.has_coincidence()
    }
}

/// Handler that checks if the interiors of subject and clip shapes overlap.
///
/// Returns `true` when both shapes have fill on the same side of a segment,
/// indicating their interiors share area. This is stricter than `intersects`
/// which also returns true for boundary-only contact.
///
/// Early-exits `true` on first interior overlap.
pub(crate) struct InteriorsIntersectHandler;

impl<I: IntNumber> FillHandler<ShapeCountBoolean, I> for InteriorsIntersectHandler {
    type Output = bool;

    #[inline(always)]
    fn handle(
        &mut self,
        _index: usize,
        _segment: &Segment<ShapeCountBoolean, I>,
        fill: SegmentFill,
    ) -> ControlFlow<bool> {
        // Interiors intersect if both shapes fill the same side
        if (fill & BOTH_TOP) == BOTH_TOP || (fill & BOTH_BOTTOM) == BOTH_BOTTOM {
            ControlFlow::Break(true)
        } else {
            ControlFlow::Continue(())
        }
    }

    #[inline(always)]
    fn finalize(self) -> bool {
        false
    }
}

/// Handler that checks if subject and clip shapes touch (boundaries intersect but interiors don't).
///
/// Returns `true` if boundaries contact without interior overlap.
/// Early-exits with `false` on first interior overlap since that definitively means
/// the shapes don't just touch.
///
/// Also collects endpoint information for point coincidence check in finalize.
pub(crate) struct TouchesHandler<I: IntNumber> {
    has_boundary_contact: bool,
    point_checker: PointCoincidenceChecker<I>,
}

impl<I: IntNumber + SortKey> TouchesHandler<I> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            has_boundary_contact: false,
            point_checker: PointCoincidenceChecker::new(capacity),
        }
    }
}

impl<I: IntNumber + SortKey> FillHandler<ShapeCountBoolean, I> for TouchesHandler<I> {
    type Output = bool;

    #[inline(always)]
    fn handle(
        &mut self,
        _index: usize,
        segment: &Segment<ShapeCountBoolean, I>,
        fill: SegmentFill,
    ) -> ControlFlow<bool> {
        // Interior overlap = not a touch (early exit false)
        if (fill & BOTH_TOP) == BOTH_TOP || (fill & BOTH_BOTTOM) == BOTH_BOTTOM {
            return ControlFlow::Break(false);
        }
        // Track boundary contact
        if (fill & SUBJ_BOTH) != 0 && (fill & CLIP_BOTH) != 0 {
            self.has_boundary_contact = true;
        }
        self.point_checker.add_segment(segment, fill);
        ControlFlow::Continue(())
    }

    #[inline(always)]
    fn finalize(self) -> bool {
        self.has_boundary_contact || self.point_checker.has_coincidence()
    }
}

/// Handler that checks if subject and clip shapes intersect by point coincidence only.
///
/// Returns `true` if shapes share boundary vertices but NOT edges.
/// - Returns `false` if there's interior overlap (early exit)
/// - Returns `false` if there's edge/boundary contact (shared segments, early exit)
/// - Returns `true` ONLY if shapes touch by point coincidence without any edge overlap
pub(crate) struct PointIntersectsHandler<I: IntNumber> {
    point_checker: PointCoincidenceChecker<I>,
}

impl<I: IntNumber + SortKey> PointIntersectsHandler<I> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            point_checker: PointCoincidenceChecker::new(capacity),
        }
    }
}

impl<I: IntNumber + SortKey> FillHandler<ShapeCountBoolean, I> for PointIntersectsHandler<I> {
    type Output = bool;

    #[inline(always)]
    fn handle(
        &mut self,
        _index: usize,
        segment: &Segment<ShapeCountBoolean, I>,
        fill: SegmentFill,
    ) -> ControlFlow<bool> {
        // Interior overlap = not a point-only intersection (early exit false)
        if (fill & BOTH_TOP) == BOTH_TOP || (fill & BOTH_BOTTOM) == BOTH_BOTTOM {
            return ControlFlow::Break(false);
        }
        // Boundary contact (edge sharing) = not point-only (early exit false)
        if (fill & SUBJ_BOTH) != 0 && (fill & CLIP_BOTH) != 0 {
            return ControlFlow::Break(false);
        }
        self.point_checker.add_segment(segment, fill);
        ControlFlow::Continue(())
    }

    #[inline(always)]
    fn finalize(self) -> bool {
        self.point_checker.has_coincidence()
    }
}

/// Handler that checks if subject is completely within clip.
///
/// Returns `true` if everywhere the subject has fill, the clip also has fill
/// on the same side. Early-exits `false` on first violation.
pub(crate) struct WithinHandler {
    subj_present: bool,
}

impl WithinHandler {
    pub(crate) fn new() -> Self {
        Self { subj_present: false }
    }
}

impl<I: IntNumber> FillHandler<ShapeCountBoolean, I> for WithinHandler {
    type Output = bool;

    #[inline(always)]
    fn handle(
        &mut self,
        _index: usize,
        _segment: &Segment<ShapeCountBoolean, I>,
        fill: SegmentFill,
    ) -> ControlFlow<bool> {
        let subj_top = (fill & SUBJ_TOP) != 0;
        let subj_bot = (fill & SUBJ_BOTTOM) != 0;
        let clip_top = (fill & CLIP_TOP) != 0;
        let clip_bot = (fill & CLIP_BOTTOM) != 0;

        if subj_top || subj_bot {
            self.subj_present = true;
        }

        // Subject filled where clip isn't = not within
        if (subj_top && !clip_top) || (subj_bot && !clip_bot) {
            ControlFlow::Break(false)
        } else {
            ControlFlow::Continue(())
        }
    }

    #[inline(always)]
    fn finalize(self) -> bool {
        // Empty subject is not within anything
        self.subj_present
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::x_segment::XSegment;

    fn make_segment(
        ax: i32,
        ay: i32,
        bx: i32,
        by: i32,
        subj: i32,
        clip: i32,
    ) -> Segment<ShapeCountBoolean, i32> {
        Segment {
            x_segment: XSegment {
                a: IntPoint::new(ax, ay),
                b: IntPoint::new(bx, by),
            },
            count: ShapeCountBoolean { subj, clip },
            data: (),
        }
    }

    fn finalize_i32<H>(handler: H) -> H::Output
    where
        H: crate::build::sweep::FillHandler<ShapeCountBoolean, i32>,
    {
        handler.finalize()
    }

    #[test]
    fn test_point_coincidence_no_points() {
        let checker = PointCoincidenceChecker::<i32>::new(10);
        assert!(!checker.has_coincidence());
    }

    #[test]
    fn test_point_coincidence_subj_only() {
        let mut checker = PointCoincidenceChecker::<i32>::new(10);
        checker.add_segment(&make_segment(0, 0, 10, 0, 1, 0), SUBJ_TOP);
        assert!(!checker.has_coincidence());
    }

    #[test]
    fn test_point_coincidence_coincident_point() {
        let mut checker = PointCoincidenceChecker::<i32>::new(10);
        // Subject segment with endpoint at (10, 10)
        checker.add_segment(&make_segment(0, 0, 10, 10, 1, 0), SUBJ_TOP);
        // Clip segment with endpoint at (10, 10)
        checker.add_segment(&make_segment(10, 10, 20, 20, 0, 1), CLIP_TOP);
        assert!(checker.has_coincidence());
    }

    #[test]
    fn test_point_coincidence_no_coincidence() {
        let mut checker = PointCoincidenceChecker::<i32>::new(10);
        checker.add_segment(&make_segment(0, 0, 5, 5, 1, 0), SUBJ_TOP);
        checker.add_segment(&make_segment(10, 10, 20, 20, 0, 1), CLIP_TOP);
        assert!(!checker.has_coincidence());
    }

    #[test]
    fn test_point_coincidence_shared_segment_is_line_not_point() {
        let mut checker = PointCoincidenceChecker::<i32>::new(10);
        // Segment with both SUBJ and CLIP fill is a shared edge (line intersection),
        // not a point coincidence. Only one array gets populated, so no coincidence.
        checker.add_segment(&make_segment(0, 0, 10, 10, 1, 1), SUBJ_TOP | CLIP_BOTTOM);
        assert!(!checker.has_coincidence());
    }

    #[test]
    fn test_point_coincidence_dedup_works() {
        let mut checker = PointCoincidenceChecker::<i32>::new(10);
        // Two subject segments sharing endpoint (5, 5)
        checker.add_segment(&make_segment(0, 0, 5, 5, 1, 0), SUBJ_TOP);
        checker.add_segment(&make_segment(5, 5, 10, 10, 1, 0), SUBJ_TOP);
        // Clip at (5, 5)
        checker.add_segment(&make_segment(5, 5, 15, 15, 0, 1), CLIP_TOP);
        assert!(checker.has_coincidence());
    }

    #[test]
    fn test_intersects_handler_both_top() {
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = IntersectsHandler::<i32>::new(10);
        let fill = SUBJ_TOP | CLIP_TOP;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Break(true)));
    }

    #[test]
    fn test_intersects_handler_both_bottom() {
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = IntersectsHandler::<i32>::new(10);
        let fill = SUBJ_BOTTOM | CLIP_BOTTOM;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Break(true)));
    }

    #[test]
    fn test_intersects_handler_boundary_contact() {
        // Boundary contact (edge sharing) is still an intersection per DE-9IM
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = IntersectsHandler::<i32>::new(10);
        let fill = SUBJ_TOP | CLIP_BOTTOM;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Break(true)));
    }

    #[test]
    fn test_intersects_handler_no_intersection() {
        // Only subject contributes - no intersection
        let seg = make_segment(0, 0, 10, 0, 1, 0);
        let mut handler = IntersectsHandler::<i32>::new(10);
        let fill = SUBJ_TOP;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Continue(())));

        // Only clip contributes - no intersection
        let seg = make_segment(0, 0, 10, 0, 0, 1);
        let fill = CLIP_BOTTOM;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Continue(())));
    }

    #[test]
    fn test_intersects_handler_finalize_with_coincidence() {
        let mut handler = IntersectsHandler::<i32>::new(10);
        // Add segments that don't trigger early exit but have point coincidence
        let seg1 = make_segment(0, 0, 10, 10, 1, 0);
        let seg2 = make_segment(10, 10, 20, 20, 0, 1);
        let _ = handler.handle(0, &seg1, SUBJ_TOP);
        let _ = handler.handle(1, &seg2, CLIP_TOP);
        assert!(finalize_i32(handler));
    }

    #[test]
    fn test_interiors_intersect_handler_both_top() {
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = InteriorsIntersectHandler;
        let fill = SUBJ_TOP | CLIP_TOP;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Break(true)));
    }

    #[test]
    fn test_interiors_intersect_handler_both_bottom() {
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = InteriorsIntersectHandler;
        let fill = SUBJ_BOTTOM | CLIP_BOTTOM;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Break(true)));
    }

    #[test]
    fn test_interiors_intersect_handler_boundary_only() {
        // Boundary contact without interior overlap
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = InteriorsIntersectHandler;
        let fill = SUBJ_TOP | CLIP_BOTTOM;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Continue(())));
        assert!(!finalize_i32(handler));
    }

    #[test]
    fn test_touches_handler_boundary_only() {
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = TouchesHandler::<i32>::new(10);
        let fill = SUBJ_TOP | CLIP_BOTTOM;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Continue(())));
        assert!(handler.finalize()); // boundary contact, no interior overlap
    }

    #[test]
    fn test_touches_handler_interior_overlap() {
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = TouchesHandler::<i32>::new(10);
        let fill = SUBJ_TOP | CLIP_TOP;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Break(false))); // early exit on interior overlap
    }

    #[test]
    fn test_touches_handler_no_contact() {
        let seg = make_segment(0, 0, 10, 0, 1, 0);
        let mut handler = TouchesHandler::<i32>::new(10);
        let fill = SUBJ_TOP;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Continue(())));
        assert!(!handler.finalize()); // no boundary contact, no interior overlap
    }

    #[test]
    fn test_touches_handler_point_coincidence() {
        let mut handler = TouchesHandler::<i32>::new(10);
        // Add segments that don't touch via fill but have point coincidence
        let seg1 = make_segment(0, 0, 10, 10, 1, 0);
        let seg2 = make_segment(10, 10, 20, 20, 0, 1);
        let _ = handler.handle(0, &seg1, SUBJ_TOP);
        let _ = handler.handle(1, &seg2, CLIP_TOP);
        assert!(finalize_i32(handler));
    }

    #[test]
    fn test_within_handler_subject_inside_clip() {
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = WithinHandler::new();
        // Subject has top fill, clip also has top fill - subject is within
        let fill = SUBJ_TOP | CLIP_TOP;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Continue(())));
        assert!(finalize_i32(handler));
    }

    #[test]
    fn test_within_handler_subject_outside_clip() {
        let seg = make_segment(0, 0, 10, 0, 1, 0);
        let mut handler = WithinHandler::new();
        // Subject has top fill but clip doesn't - subject is outside
        let fill = SUBJ_TOP;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Break(false)));
    }

    #[test]
    fn test_within_handler_empty_subject() {
        let handler = WithinHandler::new();
        // Empty subject is not within anything
        assert!(!finalize_i32(handler));
    }

    #[test]
    fn test_within_handler_clip_only() {
        let seg = make_segment(0, 0, 10, 0, 0, 1);
        let mut handler = WithinHandler::new();
        // Only clip contributes - ok, but need subject present
        let fill = CLIP_TOP;
        let result = handler.handle(0, &seg, fill);
        assert!(matches!(result, ControlFlow::Continue(())));
        assert!(!finalize_i32(handler));
    }

    #[test]
    fn test_point_intersects_handler_point_only() {
        let mut handler = PointIntersectsHandler::<i32>::new(10);
        // Subject segment ending at (10, 10)
        let seg1 = make_segment(0, 0, 10, 10, 1, 0);
        // Clip segment starting at (10, 10)
        let seg2 = make_segment(10, 10, 20, 20, 0, 1);
        let _ = handler.handle(0, &seg1, SUBJ_TOP);
        let _ = handler.handle(1, &seg2, CLIP_TOP);
        // Point coincidence without edge contact → true
        assert!(handler.finalize());
    }

    #[test]
    fn test_point_intersects_handler_edge_contact() {
        // Segment belongs to both subject and clip (shared edge)
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = PointIntersectsHandler::<i32>::new(10);
        // Both shapes have fill on opposite sides (boundary contact)
        let fill = SUBJ_TOP | CLIP_BOTTOM;
        let result = handler.handle(0, &seg, fill);
        // Early exit false on boundary contact (edge sharing)
        assert!(matches!(result, ControlFlow::Break(false)));
    }

    #[test]
    fn test_point_intersects_handler_interior_overlap() {
        let seg = make_segment(0, 0, 10, 0, 1, 1);
        let mut handler = PointIntersectsHandler::<i32>::new(10);
        // Interior overlap (both shapes fill the same side)
        let fill = SUBJ_TOP | CLIP_TOP;
        let result = handler.handle(0, &seg, fill);
        // Early exit false on interior overlap
        assert!(matches!(result, ControlFlow::Break(false)));
    }

    #[test]
    fn test_point_intersects_handler_no_contact() {
        let seg1 = make_segment(0, 0, 5, 5, 1, 0);
        let seg2 = make_segment(10, 10, 20, 20, 0, 1);
        let mut handler = PointIntersectsHandler::<i32>::new(10);
        let _ = handler.handle(0, &seg1, SUBJ_TOP);
        let _ = handler.handle(1, &seg2, CLIP_TOP);
        // No contact at all → false
        assert!(!handler.finalize());
    }
}
