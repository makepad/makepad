use crate::build::sweep::{FillHandler, SweepRunner};
use crate::core::fill_rule::FillRule;
use crate::core::overlay::ShapeType;
use crate::core::predicate::{
    InteriorsIntersectHandler, IntersectsHandler, PointIntersectsHandler, TouchesHandler, WithinHandler,
};
use crate::core::solver::Solver;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::build::BuildSegments;
use crate::segm::segment::Segment;
use crate::split::solver::SplitSolver;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_key_sort::sort::key::SortKey;
use i_shape::int::shape::{IntContour, IntShape};
use i_tree::{Expiration, LayoutNumber};

/// Overlay structure optimized for spatial predicate evaluation.
///
/// `PredicateOverlay` provides efficient spatial relationship testing between
/// two polygon sets without computing full boolean operation results. It is
/// designed for cases where you only need to know *whether* shapes intersect,
/// not *what* the intersection looks like.
///
/// # Example
///
/// ```
/// use i_overlay::core::relate::PredicateOverlay;
/// use i_overlay::core::overlay::ShapeType;
///
/// let mut overlay = PredicateOverlay::<i32>::new(16);
/// // Add subject and clip segments...
/// let intersects = overlay.intersects();
/// ```
///
/// For float coordinates, prefer using [`FloatPredicateOverlay`](crate::float::relate::FloatPredicateOverlay)
/// or the [`FloatRelate`](crate::float::relate::FloatRelate) trait.
pub struct PredicateOverlay<I: IntNumber + Expiration> {
    /// Solver configuration for segment operations.
    pub solver: Solver,
    /// Fill rule for determining polygon interiors.
    pub fill_rule: FillRule,
    pub(crate) segments: Vec<Segment<ShapeCountBoolean, I>>,
    pub(crate) split_solver: SplitSolver<I>,
    sweep_runner: SweepRunner<ShapeCountBoolean, I>,
}

impl<I> PredicateOverlay<I>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    pub fn new(capacity: usize) -> Self {
        Self {
            solver: Default::default(),
            fill_rule: FillRule::EvenOdd,
            segments: Vec::with_capacity(capacity),
            split_solver: SplitSolver::new(),
            sweep_runner: SweepRunner::new(),
        }
    }

    fn evaluate<T: Default, H: FillHandler<ShapeCountBoolean, I, Output = T>>(&mut self, handler: H) -> T {
        if self.segments.is_empty() {
            return T::default();
        }
        self.split_solver.split_segments(&mut self.segments, &self.solver);
        if self.segments.is_empty() {
            return T::default();
        }
        self.sweep_runner
            .run_with_fill_rule(self.fill_rule, &self.solver, &self.segments, handler)
    }

    /// Returns `true` if the subject and clip shapes intersect (share any point).
    ///
    /// This includes both interior overlap and boundary contact (including single-point touches).
    #[inline]
    pub fn intersects(&mut self) -> bool {
        let capacity = self.segments.len();
        self.evaluate(IntersectsHandler::<I>::new(capacity))
    }

    /// Returns `true` if the interiors of subject and clip shapes overlap.
    ///
    /// Unlike `intersects()`, this returns `false` for shapes that only share
    /// boundary points (edges or vertices) without interior overlap.
    #[inline]
    pub fn interiors_intersect(&mut self) -> bool {
        self.evaluate(InteriorsIntersectHandler)
    }

    /// Returns `true` if subject and clip shapes touch (boundaries intersect but interiors don't).
    ///
    /// This returns `true` when shapes share boundary points (edges or vertices)
    /// but their interiors don't overlap. This includes single-point vertex touches.
    #[inline]
    pub fn touches(&mut self) -> bool {
        let capacity = self.segments.len();
        self.evaluate(TouchesHandler::<I>::new(capacity))
    }

    /// Returns `true` if subject and clip shapes intersect by point coincidence only.
    ///
    /// This returns `true` when shapes share boundary vertices but NOT edges.
    /// Unlike `touches()`, this returns `false` for shapes that share edges.
    #[inline]
    pub fn point_intersects(&mut self) -> bool {
        let capacity = self.segments.len();
        self.evaluate(PointIntersectsHandler::<I>::new(capacity))
    }

    /// Returns `true` if subject is completely within clip.
    ///
    /// Subject is within clip if everywhere the subject has fill, the clip
    /// also has fill on the same side.
    #[inline]
    pub fn within(&mut self) -> bool {
        self.evaluate(WithinHandler::new())
    }

    /// Adds a path to the overlay using an iterator, allowing for more flexible path input.
    /// This function is particularly useful when working with dynamically generated paths or
    /// when paths are not directly stored in a collection.
    /// - `iter`: An iterator over references to `IntPoint` that defines the path.
    /// - `shape_type`: Specifies the role of the added path in the overlay operation, either as `Subject` or `Clip`.
    #[inline]
    pub fn add_path_iter<It: Iterator<Item = IntPoint<I>>>(&mut self, iter: It, shape_type: ShapeType) {
        self.segments.append_path_iter(iter, shape_type, false);
    }

    /// Adds a single path to the overlay as either subject or clip paths.
    /// - `contour`: An array of points that form a closed path.
    /// - `shape_type`: Specifies the role of the added path in the overlay operation, either as `Subject` or `Clip`.
    #[inline]
    pub fn add_contour(&mut self, contour: &[IntPoint<I>], shape_type: ShapeType) {
        self.segments
            .append_path_iter(contour.iter().copied(), shape_type, false);
    }

    /// Adds multiple paths to the overlay as either subject or clip paths.
    /// - `contours`: An array of `IntContour<I>` instances to be added to the overlay.
    /// - `shape_type`: Specifies the role of the added paths in the overlay operation, either as `Subject` or `Clip`.
    #[inline]
    pub fn add_contours(&mut self, contours: &[IntContour<I>], shape_type: ShapeType) {
        for contour in contours.iter() {
            self.add_contour(contour, shape_type);
        }
    }

    /// Adds a single shape to the overlay as either a subject or clip shape.
    /// - `shape`: A reference to a `IntShape<I>` instance to be added.
    /// - `shape_type`: Specifies the role of the added shape in the overlay operation, either as `Subject` or `Clip`.
    #[inline]
    pub fn add_shape(&mut self, shape: &IntShape<I>, shape_type: ShapeType) {
        self.add_contours(shape, shape_type);
    }

    /// Adds multiple shapes to the overlay as either subject or clip shapes.
    /// - `shapes`: An array of `IntShape<I>` instances to be added to the overlay.
    /// - `shape_type`: Specifies the role of the added shapes in the overlay operation, either as `Subject` or `Clip`.
    #[inline]
    pub fn add_shapes(&mut self, shapes: &[IntShape<I>], shape_type: ShapeType) {
        for shape in shapes.iter() {
            self.add_contours(shape, shape_type);
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.segments.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn square(x: i32, y: i32, size: i32) -> Vec<IntPoint> {
        vec![
            IntPoint::new(x, y),
            IntPoint::new(x, y + size),
            IntPoint::new(x + size, y + size),
            IntPoint::new(x + size, y),
        ]
    }

    #[test]
    fn test_add_contour_intersects() {
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(5, 5, 10), ShapeType::Clip);
        assert!(overlay.intersects());
    }

    #[test]
    fn test_add_contour_intersects_i64() {
        let mut overlay = PredicateOverlay::<i64>::new(16);
        overlay.add_contour(
            &[
                IntPoint::new(0, 0),
                IntPoint::new(0, 10),
                IntPoint::new(10, 10),
                IntPoint::new(10, 0),
            ],
            ShapeType::Subject,
        );
        overlay.add_contour(
            &[
                IntPoint::new(5, 5),
                IntPoint::new(5, 15),
                IntPoint::new(15, 15),
                IntPoint::new(15, 5),
            ],
            ShapeType::Clip,
        );
        assert!(overlay.intersects());
    }

    #[test]
    fn test_add_contour_disjoint() {
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(20, 20, 10), ShapeType::Clip);
        assert!(!overlay.intersects());
    }

    #[test]
    fn test_add_contour_touches() {
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(10, 0, 10), ShapeType::Clip);
        assert!(overlay.touches());

        overlay.clear();
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(10, 0, 10), ShapeType::Clip);
        assert!(!overlay.interiors_intersect());
    }

    #[test]
    fn test_add_contour_within() {
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(5, 5, 10), ShapeType::Subject);
        overlay.add_contour(&square(0, 0, 20), ShapeType::Clip);
        assert!(overlay.within());
    }

    #[test]
    fn test_add_contours() {
        let mut overlay = PredicateOverlay::new(16);
        let contours = vec![square(0, 0, 5), square(10, 10, 5)];
        overlay.add_contours(&contours, ShapeType::Subject);
        overlay.add_contour(&square(2, 2, 3), ShapeType::Clip);
        assert!(overlay.intersects());
    }

    #[test]
    fn test_add_shape() {
        let mut overlay = PredicateOverlay::new(16);
        let shape = vec![square(0, 0, 10)];
        overlay.add_shape(&shape, ShapeType::Subject);
        overlay.add_contour(&square(5, 5, 10), ShapeType::Clip);
        assert!(overlay.intersects());
    }

    #[test]
    fn test_add_shapes() {
        let mut overlay = PredicateOverlay::new(16);
        let shapes = vec![vec![square(0, 0, 5)], vec![square(20, 20, 5)]];
        overlay.add_shapes(&shapes, ShapeType::Subject);
        overlay.add_contour(&square(2, 2, 3), ShapeType::Clip);
        assert!(overlay.intersects());
    }

    #[test]
    fn test_add_path_iter() {
        let mut overlay = PredicateOverlay::new(16);
        let points = square(0, 0, 10);
        overlay.add_path_iter(points.into_iter(), ShapeType::Subject);
        overlay.add_contour(&square(5, 5, 10), ShapeType::Clip);
        assert!(overlay.intersects());
    }

    #[test]
    fn test_point_touch_intersects() {
        // Two squares touching at a single corner point (10, 10)
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(10, 10, 10), ShapeType::Clip);
        assert!(overlay.intersects(), "point-to-point should intersect");
    }

    #[test]
    fn test_point_touch_touches() {
        // Two squares touching at a single corner point (10, 10)
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(10, 10, 10), ShapeType::Clip);
        assert!(overlay.touches(), "point-to-point should touch");
    }

    #[test]
    fn test_point_touch_no_interior_intersect() {
        // Two squares touching at a single corner point (10, 10)
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(10, 10, 10), ShapeType::Clip);
        assert!(
            !overlay.interiors_intersect(),
            "point touch has no interior intersection"
        );
    }

    /// Test that intersects() detects when an edge passes through another polygon's interior.
    ///
    /// Shape A is a quadrilateral: (0,1), (0,0), (3,0), (3,8)
    /// Shape B is a box: (0,3) to (1,4)
    ///
    /// The edge from (0,1) to (3,8) passes through (1, 10/3 ≈ 3.33), which is inside B.
    /// Therefore intersects() should return true.
    #[test]
    fn test_intersects_edge_through_interior() {
        // Shape A: quadrilateral with edge passing through B's interior
        let shape_a = vec![
            IntPoint::new(0, 1),
            IntPoint::new(0, 0),
            IntPoint::new(3, 0),
            IntPoint::new(3, 8),
        ];

        // Shape B: box from (0,3) to (1,4)
        let shape_b = vec![
            IntPoint::new(0, 3),
            IntPoint::new(1, 3),
            IntPoint::new(1, 4),
            IntPoint::new(0, 4),
        ];

        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&shape_a, ShapeType::Subject);
        overlay.add_contour(&shape_b, ShapeType::Clip);

        // The edge from (0,1) to (3,8) has parametric form:
        // P(t) = (0,1) + t*(3,7) = (3t, 1+7t) for t in [0,1]
        // At x=1: t=1/3, y = 1 + 7/3 = 10/3 ≈ 3.333
        // Point (1, 3.333) is strictly inside B (x in [0,1], y in [3,4])
        assert!(
            overlay.intersects(),
            "Edge (0,1)->(3,8) passes through box interior at (1, 3.33); should intersect"
        );
    }

    #[test]
    fn test_segment_end_to_start_touch() {
        // Triangle where subject's segment endpoint touches clip's segment startpoint
        // Subject: triangle at (0,0), (10,0), (5,10)
        // Clip: triangle at (10,0), (20,0), (15,10)
        // They touch at exactly one point: (10,0)
        let subj = vec![IntPoint::new(0, 0), IntPoint::new(10, 0), IntPoint::new(5, 10)];
        let clip = vec![IntPoint::new(10, 0), IntPoint::new(20, 0), IntPoint::new(15, 10)];

        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&subj, ShapeType::Subject);
        overlay.add_contour(&clip, ShapeType::Clip);
        assert!(
            overlay.intersects(),
            "segment b touching segment a should intersect"
        );

        overlay.clear();
        overlay.add_contour(&subj, ShapeType::Subject);
        overlay.add_contour(&clip, ShapeType::Clip);
        assert!(overlay.touches(), "segment b touching segment a should touch");

        overlay.clear();
        overlay.add_contour(&subj, ShapeType::Subject);
        overlay.add_contour(&clip, ShapeType::Clip);
        assert!(
            !overlay.interiors_intersect(),
            "segment b touching segment a should not have interior intersection"
        );
    }

    /// Creates a square with a hole (doughnut shape).
    /// Outer: counter-clockwise, Inner hole: clockwise
    fn doughnut(
        outer_x: i32,
        outer_y: i32,
        outer_size: i32,
        hole_x: i32,
        hole_y: i32,
        hole_size: i32,
    ) -> Vec<Vec<IntPoint>> {
        vec![
            // Outer boundary (counter-clockwise)
            vec![
                IntPoint::new(outer_x, outer_y),
                IntPoint::new(outer_x, outer_y + outer_size),
                IntPoint::new(outer_x + outer_size, outer_y + outer_size),
                IntPoint::new(outer_x + outer_size, outer_y),
            ],
            // Inner hole (clockwise)
            vec![
                IntPoint::new(hole_x, hole_y),
                IntPoint::new(hole_x + hole_size, hole_y),
                IntPoint::new(hole_x + hole_size, hole_y + hole_size),
                IntPoint::new(hole_x, hole_y + hole_size),
            ],
        ]
    }

    /// Creates a diamond (rotated square) with corners at midpoints of a bounding box.
    fn diamond(cx: i32, cy: i32, radius: i32) -> Vec<IntPoint> {
        vec![
            IntPoint::new(cx, cy - radius), // top
            IntPoint::new(cx + radius, cy), // right
            IntPoint::new(cx, cy + radius), // bottom
            IntPoint::new(cx - radius, cy), // left
        ]
    }

    #[test]
    fn test_doughnut_with_diamond_touching_hole_intersects() {
        // Subject: square doughnut with outer (0,0)-(30,30) and hole (10,10)-(20,20)
        // Clip: diamond inside the hole with corners touching the hole boundary
        //       Diamond centered at (15,15) with corners at (15,10), (20,15), (15,20), (10,15)
        //
        // This tests that inner segments of the hole (which have SUBJ_BOTH fill)
        // are still correctly tracked for point coincidence detection.
        let mut overlay = PredicateOverlay::new(32);
        let doughnut_shape = doughnut(0, 0, 30, 10, 10, 10);
        overlay.add_shape(&doughnut_shape, ShapeType::Subject);
        overlay.add_contour(&diamond(15, 15, 5), ShapeType::Clip);
        assert!(
            overlay.intersects(),
            "diamond touching hole boundary should intersect"
        );
    }

    #[test]
    fn test_doughnut_with_diamond_touching_hole_touches() {
        // Same setup: diamond corners touch the hole boundary but don't overlap
        let mut overlay = PredicateOverlay::new(32);
        let doughnut_shape = doughnut(0, 0, 30, 10, 10, 10);
        overlay.add_shape(&doughnut_shape, ShapeType::Subject);
        overlay.add_contour(&diamond(15, 15, 5), ShapeType::Clip);
        assert!(overlay.touches(), "diamond touching hole boundary should touch");
    }

    #[test]
    fn test_doughnut_with_diamond_touching_hole_no_interior_intersect() {
        // Same setup: diamond only touches at boundary points, interiors don't overlap
        let mut overlay = PredicateOverlay::new(32);
        let doughnut_shape = doughnut(0, 0, 30, 10, 10, 10);
        overlay.add_shape(&doughnut_shape, ShapeType::Subject);
        overlay.add_contour(&diamond(15, 15, 5), ShapeType::Clip);
        assert!(
            !overlay.interiors_intersect(),
            "diamond touching hole boundary should not have interior intersection"
        );
    }

    #[test]
    fn test_doughnut_with_diamond_inside_hole_disjoint() {
        // Diamond fully inside the hole, not touching any boundary
        // Diamond centered at (15,15) with radius 2 (corners at 13,15,17,15 etc)
        let mut overlay = PredicateOverlay::new(32);
        let doughnut_shape = doughnut(0, 0, 30, 10, 10, 10);
        overlay.add_shape(&doughnut_shape, ShapeType::Subject);
        overlay.add_contour(&diamond(15, 15, 2), ShapeType::Clip);
        assert!(!overlay.intersects(), "diamond inside hole should not intersect");
        assert!(!overlay.touches(), "diamond inside hole should not touch");
    }

    #[test]
    fn test_doughnut_with_diamond_touching_single_corner() {
        // Diamond inside the hole, touching only one corner: (10, 10)
        // The hole is at (10,10)-(20,20), so a diamond inside it touching the
        // bottom-left corner (10,10) needs all other points inside the hole.
        // Diamond: (10,10), (12,10), (12,12), (10,12) - this is a small square
        // in the corner of the hole, with one corner touching the hole corner.
        let diamond_touching_corner = vec![
            IntPoint::new(10, 10), // touches hole corner (also doughnut boundary)
            IntPoint::new(12, 10), // on hole bottom edge
            IntPoint::new(12, 12), // inside hole
            IntPoint::new(10, 12), // on hole left edge
        ];

        let mut overlay = PredicateOverlay::new(32);
        overlay.add_shape(&doughnut(0, 0, 30, 10, 10, 10), ShapeType::Subject);
        overlay.add_contour(&diamond_touching_corner, ShapeType::Clip);

        assert!(
            overlay.intersects(),
            "diamond touching hole corner should intersect"
        );
        assert!(overlay.touches(), "diamond touching hole corner should touch");
        assert!(
            !overlay.interiors_intersect(),
            "diamond touching hole corner should not have interior intersection"
        );
    }

    #[test]
    fn test_outer_diamond_touching_doughnut_corner() {
        // Diamond outside the doughnut, touching the outer corner at (0, 0)
        // This tests point coincidence for outer boundary segments.
        let diamond_outside = vec![
            IntPoint::new(0, 0),   // touches doughnut outer corner
            IntPoint::new(-5, 3),  // outside
            IntPoint::new(-5, -3), // outside
        ];

        let mut overlay = PredicateOverlay::new(32);
        overlay.add_shape(&doughnut(0, 0, 30, 10, 10, 10), ShapeType::Subject);
        overlay.add_contour(&diamond_outside, ShapeType::Clip);

        assert!(
            overlay.intersects(),
            "triangle touching outer corner should intersect"
        );
        assert!(overlay.touches(), "triangle touching outer corner should touch");
        assert!(
            !overlay.interiors_intersect(),
            "triangle touching outer corner should not have interior intersection"
        );
    }

    #[test]
    fn test_point_intersects_corner_to_corner() {
        // Two squares touching at a single corner point (10, 10)
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(10, 10, 10), ShapeType::Clip);
        assert!(
            overlay.point_intersects(),
            "corner-to-corner should be point-only intersection"
        );
    }

    #[test]
    fn test_point_intersects_edge_sharing() {
        // Two squares sharing an edge (not point-only)
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(10, 0, 10), ShapeType::Clip);
        // touches() is true for edge sharing
        assert!(overlay.touches());

        overlay.clear();
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(10, 0, 10), ShapeType::Clip);
        // point_intersects() is false for edge sharing
        assert!(
            !overlay.point_intersects(),
            "edge sharing is not point-only intersection"
        );
    }

    #[test]
    fn test_point_intersects_overlapping() {
        // Overlapping squares (not point-only)
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(5, 5, 10), ShapeType::Clip);
        assert!(
            !overlay.point_intersects(),
            "overlapping shapes are not point-only intersection"
        );
    }

    #[test]
    fn test_point_intersects_disjoint() {
        // Disjoint squares (no contact at all)
        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&square(0, 0, 10), ShapeType::Subject);
        overlay.add_contour(&square(20, 20, 10), ShapeType::Clip);
        assert!(
            !overlay.point_intersects(),
            "disjoint shapes have no point intersection"
        );
    }

    #[test]
    fn test_point_intersects_triangle_vertex() {
        // Two triangles touching at a single vertex
        let tri1 = vec![IntPoint::new(0, 0), IntPoint::new(10, 0), IntPoint::new(5, 10)];
        let tri2 = vec![IntPoint::new(10, 0), IntPoint::new(20, 0), IntPoint::new(15, 10)];

        let mut overlay = PredicateOverlay::new(16);
        overlay.add_contour(&tri1, ShapeType::Subject);
        overlay.add_contour(&tri2, ShapeType::Clip);
        assert!(
            overlay.point_intersects(),
            "triangles touching at vertex should be point-only intersection"
        );
    }
}
