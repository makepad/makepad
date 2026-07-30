//! This module contains functionality to construct and manage overlays, which are used to perform
//! boolean operations (union, intersection, etc.) on polygons. It provides structures and methods to
//! manage subject and clip polygons and convert them into graphs for further operations.

use crate::build::builder::GraphBuilder;
use crate::core::extract::BooleanExtractionBuffer;
use crate::core::fill_rule::FillRule;
use crate::core::overlay_rule::OverlayRule;
use crate::core::solver::Solver;
use crate::i_shape::flat::buffer::FlatContoursBuffer;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::build::BuildSegments;
use crate::segm::segment::Segment;
use crate::split::solver::SplitSolver;
use crate::vector::edge::{DataVectorEdge, VectorShape};
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::uint::UIntNumber;
use i_float::int::point::IntPoint;
use i_key_sort::sort::key::SortKey;
use i_shape::int::count::PointsCount;
use i_shape::int::shape::{IntContour, IntShape, IntShapes};
use i_tree::{Expiration, LayoutNumber};

use super::graph::{OverlayGraph, OverlayNode};

/// Configuration options for polygon Boolean operations using [`Overlay`].
///
/// These options control precision, simplification, and contour filtering
/// during the Boolean operation process. You can use this to adjust output
/// direction, eliminate small artifacts, or retain collinear points.
#[derive(Debug, Clone, Copy)]
pub struct IntOverlayOptions<U: UIntNumber> {
    /// Preserve collinear points in the input before Boolean operations.
    pub preserve_input_collinear: bool,

    /// Desired direction for output contours (default outer: CCW / hole: CW).
    pub output_direction: ContourDirection,

    /// Preserve collinear points in the output after Boolean operations.
    pub preserve_output_collinear: bool,

    /// Minimum area threshold to include a contour in the result.
    pub min_output_area: U,

    /// If true, extract OGC-valid shapes.
    pub ogc: bool,
}

/// Specifies the type of shape being processed, influencing how the shape participates in Boolean operations.
/// Note: All operations except for `Difference` are commutative, meaning the order of `Subject` and `Clip` shapes does not impact the outcome.
/// - `Subject`: The primary shape(s) for operations. Acts as the base layer in the operation.
/// - `Clip`: The modifying shape(s) that are applied to the `Subject`. Determines how the `Subject` is altered or intersected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeType {
    Subject,
    Clip,
}

/// Represents the winding direction of a contour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContourDirection {
    CounterClockwise,
    Clockwise,
}

/// This struct is essential for describing and uploading the geometry or shapes required to construct an `OverlayGraph`. It prepares the necessary data for boolean operations.
pub struct Overlay<I: IntNumber + Expiration> {
    pub solver: Solver,
    pub options: IntOverlayOptions<I::WideUInt>,
    pub boolean_buffer: Option<BooleanExtractionBuffer<I>>,
    pub(crate) segments: Vec<Segment<ShapeCountBoolean, I>>,
    pub(crate) split_solver: SplitSolver<I>,
    pub(crate) graph_builder: GraphBuilder<ShapeCountBoolean, OverlayNode, I>,
}

impl<I> Overlay<I>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    /// Constructs a new `Overlay` instance, initializing it with a capacity that should closely match the total count of edges from all shapes being processed.
    /// This pre-allocation helps in optimizing memory usage and performance.
    /// - `capacity`: The initial capacity for storing edge data. Ideally, this should be set to the sum of the edges of all shapes to be added to the overlay, ensuring efficient data management.
    pub fn new(capacity: usize) -> Self {
        Self {
            solver: Default::default(),
            options: Default::default(),
            boolean_buffer: Some(Default::default()),
            segments: Vec::with_capacity(capacity),
            split_solver: SplitSolver::new(),
            graph_builder: GraphBuilder::<ShapeCountBoolean, OverlayNode, I>::new(),
        }
    }

    /// Constructs a new `Overlay` instance, initializing it with a capacity that should closely match the total count of edges from all shapes being processed.
    /// This pre-allocation helps in optimizing memory usage and performance.
    /// - `capacity`: The initial capacity for storing edge data. Ideally, this should be set to the sum of the edges of all shapes to be added to the overlay, ensuring efficient data management.
    /// - `options`: Adjust custom behavior.
    /// - `solver`: Type of solver to use.
    pub fn new_custom(capacity: usize, options: IntOverlayOptions<I::WideUInt>, solver: Solver) -> Self {
        Self {
            solver,
            options,
            boolean_buffer: Some(Default::default()),
            segments: Vec::with_capacity(capacity),
            split_solver: SplitSolver::new(),
            graph_builder: GraphBuilder::<ShapeCountBoolean, OverlayNode, I>::new(),
        }
    }

    /// Creates a new `Overlay` instance and initializes it with subject and clip contours.
    /// - `subj`: An array of contours that together define the subject.
    /// - `clip`: An array of contours that together define the clip.
    pub fn with_contour(subj: &[IntPoint<I>], clip: &[IntPoint<I>]) -> Self {
        let mut overlay = Self::new(subj.len() + clip.len());
        overlay.add_contour(subj, ShapeType::Subject);
        overlay.add_contour(clip, ShapeType::Clip);
        overlay
    }

    /// Creates a new `Overlay` instance and initializes it with subject and clip contours.
    /// - `subj`: An array of contours that together define the subject.
    /// - `clip`: An array of contours that together define the clip.
    /// - `options`: Adjust custom behavior.
    /// - `solver`: Type of solver to use.
    pub fn with_contour_custom(
        subj: &[IntPoint<I>],
        clip: &[IntPoint<I>],
        options: IntOverlayOptions<I::WideUInt>,
        solver: Solver,
    ) -> Self {
        let mut overlay = Self::new_custom(subj.len() + clip.len(), options, solver);
        overlay.add_contour(subj, ShapeType::Subject);
        overlay.add_contour(clip, ShapeType::Clip);
        overlay
    }

    /// Creates a new `Overlay` instance and initializes it with subject and clip contours.
    /// - `subj`: An array of contours that together define the subject shape.
    /// - `clip`: An array of contours that together define the clip shape.
    pub fn with_contours(subj: &[IntContour<I>], clip: &[IntContour<I>]) -> Self {
        let mut overlay = Self::new(subj.points_count() + clip.points_count());
        overlay.add_contours(subj, ShapeType::Subject);
        overlay.add_contours(clip, ShapeType::Clip);
        overlay
    }

    /// Creates a new `Overlay` instance and initializes it with subject and clip contours.
    /// - `subj`: An array of contours that together define the subject shape.
    /// - `clip`: An array of contours that together define the clip shape.
    /// - `options`: Adjust custom behavior.
    /// - `solver`: Type of solver to use.
    pub fn with_contours_custom(
        subj: &[IntContour<I>],
        clip: &[IntContour<I>],
        options: IntOverlayOptions<I::WideUInt>,
        solver: Solver,
    ) -> Self {
        let mut overlay = Self::new_custom(subj.points_count() + clip.points_count(), options, solver);
        overlay.add_contours(subj, ShapeType::Subject);
        overlay.add_contours(clip, ShapeType::Clip);
        overlay
    }

    /// Creates a new `Overlay` instance and initializes it with subject and clip shapes.
    /// - `subj`: An array of shapes to be used as the subject in the overlay operation.
    /// - `clip`: An array of shapes to be used as the clip in the overlay operation.
    pub fn with_shapes(subj: &[IntShape<I>], clip: &[IntShape<I>]) -> Self {
        let mut overlay = Self::new(subj.points_count() + clip.points_count());
        overlay.add_shapes(subj, ShapeType::Subject);
        overlay.add_shapes(clip, ShapeType::Clip);
        overlay
    }

    /// Creates a new `Overlay` instance and initializes it with subject and clip shapes.
    /// - `subj`: An array of shapes to be used as the subject in the overlay operation.
    /// - `clip`: An array of shapes to be used as the clip in the overlay operation.
    /// - `options`: Adjust custom behavior.
    /// - `solver`: Type of solver to use.
    pub fn with_shapes_options(
        subj: &[IntShape<I>],
        clip: &[IntShape<I>],
        options: IntOverlayOptions<I::WideUInt>,
        solver: Solver,
    ) -> Self {
        let mut overlay = Self::new_custom(subj.points_count() + clip.points_count(), options, solver);
        overlay.add_shapes(subj, ShapeType::Subject);
        overlay.add_shapes(clip, ShapeType::Clip);
        overlay
    }

    /// Adds a path to the overlay using an iterator, allowing for more flexible path input.
    /// This function is particularly useful when working with dynamically generated paths or
    /// when paths are not directly stored in a collection.
    /// - `iter`: An iterator over references to `IntPoint` that defines the path.
    /// - `shape_type`: Specifies the role of the added path in the overlay operation, either as `Subject` or `Clip`.
    #[inline]
    pub fn add_path_iter<It: Iterator<Item = IntPoint<I>>>(&mut self, iter: It, shape_type: ShapeType) {
        self.segments
            .append_path_iter(iter, shape_type, self.options.preserve_input_collinear);
    }

    /// Adds a single path to the overlay as either subject or clip paths.
    /// - `contour`: An array of points that form a closed path.
    /// - `shape_type`: Specifies the role of the added path in the overlay operation, either as `Subject` or `Clip`.
    #[inline]
    pub fn add_contour(&mut self, contour: &[IntPoint<I>], shape_type: ShapeType) {
        self.segments.append_path_iter(
            contour.iter().copied(),
            shape_type,
            self.options.preserve_input_collinear,
        );
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

    /// Adds multiple flat-shape to the overlay as either subject or clip shapes.
    /// - `buffer`: A buffer of `IntShapes<I>` instances to be added to the overlay.
    /// - `shape_type`: Specifies the role of the added shapes in the overlay operation, either as `Subject` or `Clip`.
    #[inline]
    pub fn add_flat_buffer(&mut self, buffer: &FlatContoursBuffer<I>, shape_type: ShapeType) {
        for range in buffer.ranges.iter() {
            let contour = &buffer.points[range.clone()];
            self.add_contour(contour, shape_type);
        }
    }

    /// Convert into vector shapes from the added paths or shapes, applying the specified build and overlay rules. This method is particularly useful for development purposes and for creating visualizations in educational demos, where understanding the impact of different rules on the final geometry is crucial.
    /// - `fill_rule`: The build rule to use for the shapes.
    /// - `overlay_rule`: The overlay rule to apply.
    pub fn build_shape_vectors(
        &mut self,
        fill_rule: FillRule,
        overlay_rule: OverlayRule,
    ) -> Vec<VectorShape<I>> {
        self.split_solver.split_segments(&mut self.segments, &self.solver);
        if self.segments.is_empty() {
            return Vec::new();
        }

        let mut buffer = self.boolean_buffer.take().unwrap_or_default();

        let shapes = self
            .graph_builder
            .build_boolean_overlay(
                fill_rule,
                overlay_rule,
                self.options,
                &self.solver,
                &self.segments,
            )
            .extract_vector_shapes(overlay_rule, &mut buffer);

        self.boolean_buffer = Some(buffer);

        shapes
    }

    /// Convert into vectors from the added paths or shapes, applying the specified build rule. This method is particularly useful for development purposes and for creating visualizations in educational demos, where understanding the impact of different rules on the final geometry is crucial.
    /// - `fill_rule`: The build rule to use for the shapes.
    pub fn build_separate_vectors(&mut self, fill_rule: FillRule) -> Vec<DataVectorEdge<I>> {
        self.split_solver.split_segments(&mut self.segments, &self.solver);
        if self.segments.is_empty() {
            return Vec::new();
        }
        self.graph_builder
            .build_boolean_all(fill_rule, self.options, &self.solver, &self.segments)
            .extract_separate_vectors()
    }

    /// Convert into `OverlayGraph` from the added paths or shapes using the specified build rule. This graph is the foundation for executing boolean operations, allowing for the analysis and manipulation of the geometric data. The `OverlayGraph` created by this method represents a preprocessed state of the input shapes, optimized for the application of boolean operations based on the provided build rule.
    /// - `fill_rule`: Specifies the rule for determining filled areas within the shapes, influencing how the resulting graph represents intersections and unions.
    #[inline]
    pub fn build_graph_view(&mut self, fill_rule: FillRule) -> Option<OverlayGraph<'_, I>> {
        self.split_solver.split_segments(&mut self.segments, &self.solver);
        if self.segments.is_empty() {
            return None;
        }
        let graph =
            self.graph_builder
                .build_boolean_all(fill_rule, self.options, &self.solver, &self.segments);

        Some(graph)
    }

    /// Executes a single Boolean operation on the current geometry using the specified overlay and build rules.
    /// This method provides a streamlined approach for performing a Boolean operation without generating
    /// an entire `OverlayGraph`. Ideal for cases where only one Boolean operation is needed, `overlay`
    /// saves on computational resources by building only the necessary links, optimizing CPU usage by 0-20%
    /// compared to a full graph-based approach.
    ///
    /// ### Parameters:
    /// - `overlay_rule`: The boolean operation rule to apply, determining how shapes are combined or subtracted.
    /// - `fill_rule`: Specifies the rule for determining filled areas within the shapes, influencing how the resulting graph represents intersections and unions.
    /// - Returns: A vector of `IntShape<I>` that meet the specified area criteria, representing the cleaned-up geometric result.
    /// # Shape Representation
    /// The output is a `IntShapes<I>`, where:
    /// - The outer `Vec<IntShape<I>>` represents a set of shapes.
    /// - Each shape `Vec<IntContour<I>>` represents a collection of contours, where the first contour is the outer boundary, and all subsequent contours are holes in this boundary.
    /// - Each path `Vec<IntPoint>` is a sequence of points, forming a closed path.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    /// ### Usage:
    /// This function is suitable when a single, optimized Boolean operation is required on the provided
    /// geometry. For example:
    ///
    /// ```rust
    /// use i_float::int::point::IntPoint;
    /// use i_float::int_pnt;
    /// use i_overlay::core::fill_rule::FillRule;
    /// use i_overlay::core::overlay::Overlay;
    /// use i_overlay::core::overlay_rule::OverlayRule;
    ///
    /// let left_rect = [int_pnt!(0, 0), int_pnt!(0, 10), int_pnt!(10, 10), int_pnt!(10, 0)];
    /// let right_rect = [int_pnt!(10, 0), int_pnt!(10, 10), int_pnt!(20, 10), int_pnt!(20, 0)];
    /// let mut overlay = Overlay::with_contour(&left_rect, &right_rect);
    ///
    /// let result = overlay.overlay(OverlayRule::Union, FillRule::EvenOdd);
    /// ```
    ///
    /// This method is particularly useful in scenarios where the geometry only needs one overlay operation
    /// without subsequent modifications. By excluding unnecessary graph structures, it optimizes performance,
    /// particularly for complex or resource-intensive geometries.
    #[inline]
    pub fn overlay(&mut self, overlay_rule: OverlayRule, fill_rule: FillRule) -> IntShapes<I> {
        self.split_solver.split_segments(&mut self.segments, &self.solver);
        if self.segments.is_empty() {
            return Vec::new();
        }
        let mut buffer = self.boolean_buffer.take().unwrap_or_default();
        let shapes = self
            .graph_builder
            .build_boolean_overlay(
                fill_rule,
                overlay_rule,
                self.options,
                &self.solver,
                &self.segments,
            )
            .extract_shapes(overlay_rule, &mut buffer);
        self.boolean_buffer = Some(buffer);
        shapes
    }

    /// Executes a single Boolean operation and writes the result into a flat contour buffer.
    ///
    /// This is a lower-allocation alternative to [`Self::overlay`] when you want flat contour
    /// output (`points` + `ranges`) instead of nested `IntShapes<I>`.
    ///
    /// - `overlay_rule`: The Boolean operation to apply.
    /// - `fill_rule`: Fill rule used to determine interior regions.
    /// - `output`: Destination [`FlatContoursBuffer<I>`] that receives resulting contours.
    ///   Existing buffer contents are replaced.
    #[inline]
    pub fn overlay_into(
        &mut self,
        overlay_rule: OverlayRule,
        fill_rule: FillRule,
        output: &mut FlatContoursBuffer<I>,
    ) {
        self.split_solver.split_segments(&mut self.segments, &self.solver);
        if self.segments.is_empty() {
            return;
        }
        let mut buffer = self.boolean_buffer.take().unwrap_or_default();
        self.graph_builder
            .build_boolean_overlay(
                fill_rule,
                overlay_rule,
                self.options,
                &self.solver,
                &self.segments,
            )
            .extract_contours_into(overlay_rule, &mut buffer, output);
        self.boolean_buffer = Some(buffer);
    }
}

impl<U: UIntNumber> Default for IntOverlayOptions<U> {
    fn default() -> Self {
        Self {
            preserve_input_collinear: false,
            output_direction: ContourDirection::CounterClockwise,
            preserve_output_collinear: false,
            min_output_area: U::ZERO,
            ogc: false,
        }
    }
}

impl<U: UIntNumber> IntOverlayOptions<U> {
    pub fn keep_all_points() -> Self {
        Self {
            preserve_input_collinear: true,
            output_direction: ContourDirection::CounterClockwise,
            preserve_output_collinear: true,
            min_output_area: U::ZERO,
            ogc: false,
        }
    }
    pub fn keep_output_points() -> Self {
        Self {
            preserve_input_collinear: false,
            output_direction: ContourDirection::CounterClockwise,
            preserve_output_collinear: true,
            min_output_area: U::ZERO,
            ogc: false,
        }
    }
    pub fn ogc() -> Self {
        Self {
            preserve_input_collinear: false,
            output_direction: ContourDirection::CounterClockwise,
            preserve_output_collinear: false,
            min_output_area: U::ZERO,
            ogc: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::core::overlay::Overlay;
    use crate::core::overlay_rule::OverlayRule;
    use alloc::vec;
    use i_float::int::point::IntPoint;
    use i_shape::int::area::Area;
    use i_shape::int::shape::IntContour;

    #[test]
    fn test_0() {
        let subj = [vec![
            IntPoint::new(0, 0),
            IntPoint::new(10, 0),
            IntPoint::new(10, 10),
            IntPoint::new(0, 10),
        ]];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::EvenOdd);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].len(), 4);
        assert_eq!(shape[0].area(), 100i64);
    }

    #[test]
    fn test_1() {
        let subj = [
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(5, 0),
                IntPoint::new(5, 10),
                IntPoint::new(0, 10),
            ],
            vec![
                IntPoint::new(5, 0),
                IntPoint::new(10, 0),
                IntPoint::new(10, 10),
                IntPoint::new(5, 10),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::EvenOdd);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].len(), 4);
        assert_eq!(shape[0].area(), 100i64);
    }

    #[test]
    fn test_2() {
        let subj = [
            vec![
                IntPoint::new(1, 1),
                IntPoint::new(1, 3),
                IntPoint::new(3, 3),
                IntPoint::new(3, 1),
            ],
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(4, 0),
                IntPoint::new(4, 4),
                IntPoint::new(0, 4),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::EvenOdd);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 2);
        assert_eq!(shape[0].len(), 4);
        assert_eq!(shape[0].area(), 16i64);
        assert_eq!(shape[1].len(), 4);
        assert_eq!(shape[1].area(), -4i64);
    }

    #[test]
    fn test_3() {
        let subj = [
            vec![
                IntPoint::new(0, 5),
                IntPoint::new(3, 5),
                IntPoint::new(3, 7),
                IntPoint::new(0, 7),
            ],
            vec![
                IntPoint::new(0, 1),
                IntPoint::new(1, 1),
                IntPoint::new(1, 6),
                IntPoint::new(0, 6),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].area(), 10i64);
    }

    #[test]
    fn test_4() {
        let subj = [
            vec![
                IntPoint::new(5, 5),
                IntPoint::new(6, 5),
                IntPoint::new(6, 7),
                IntPoint::new(5, 7),
            ],
            vec![
                IntPoint::new(3, 1),
                IntPoint::new(6, 1),
                IntPoint::new(6, 6),
                IntPoint::new(3, 6),
            ],
            vec![
                IntPoint::new(6, 6),
                IntPoint::new(7, 6),
                IntPoint::new(7, 7),
                IntPoint::new(6, 7),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].area(), 17i64);
    }

    #[test]
    fn test_5() {
        let subj = [
            vec![
                IntPoint::new(0, 1),
                IntPoint::new(4, 1),
                IntPoint::new(4, 3),
                IntPoint::new(0, 3),
            ],
            vec![
                IntPoint::new(4, 0),
                IntPoint::new(6, 0),
                IntPoint::new(6, 4),
                IntPoint::new(4, 4),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].area(), 16i64);
    }

    #[test]
    fn test_6() {
        let subj = [
            vec![
                IntPoint::new(6, 0),
                IntPoint::new(7, 0),
                IntPoint::new(7, 6),
                IntPoint::new(6, 6),
            ],
            vec![
                IntPoint::new(3, 2),
                IntPoint::new(6, 2),
                IntPoint::new(6, 4),
                IntPoint::new(3, 4),
            ],
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(6, 0),
                IntPoint::new(6, 3),
                IntPoint::new(0, 3),
            ],
            vec![
                IntPoint::new(1, 0),
                IntPoint::new(3, 0),
                IntPoint::new(3, 2),
                IntPoint::new(1, 2),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].area(), 27i64);
    }

    #[test]
    fn test_7() {
        let subj = [
            vec![
                IntPoint::new(5, 1),
                IntPoint::new(7, 1),
                IntPoint::new(7, 7),
                IntPoint::new(5, 7),
            ],
            vec![
                IntPoint::new(5, 3),
                IntPoint::new(7, 3),
                IntPoint::new(7, 5),
                IntPoint::new(5, 5),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].area(), 12i64);
    }

    #[test]
    fn test_8() {
        let subj = [
            vec![
                IntPoint::new(6, 4),
                IntPoint::new(7, 4),
                IntPoint::new(7, 5),
                IntPoint::new(6, 5),
            ],
            vec![
                IntPoint::new(3, 4),
                IntPoint::new(7, 4),
                IntPoint::new(7, 5),
                IntPoint::new(3, 5),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        let shape = &result[0];
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].area(), 4i64);
    }

    #[test]
    fn test_9() {
        let subj = [
            vec![
                IntPoint::new(3, 0),
                IntPoint::new(6, 0),
                IntPoint::new(6, 1),
                IntPoint::new(3, 1),
            ],
            vec![
                IntPoint::new(6, 6),
                IntPoint::new(7, 6),
                IntPoint::new(7, 7),
                IntPoint::new(6, 7),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result[1].len(), 1);
        assert_eq!(result.area(), 4i64);
    }

    #[test]
    fn test_10() {
        let subj = [
            vec![
                IntPoint::new(6, 0),
                IntPoint::new(7, 0),
                IntPoint::new(7, 6),
                IntPoint::new(6, 6),
            ],
            vec![
                IntPoint::new(6, 3),
                IntPoint::new(7, 3),
                IntPoint::new(7, 5),
                IntPoint::new(6, 5),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result.area(), 6i64);
    }

    #[test]
    fn test_11() {
        let subj = [
            vec![
                IntPoint::new(3, 4),
                IntPoint::new(6, 4),
                IntPoint::new(6, 5),
                IntPoint::new(3, 5),
            ],
            vec![
                IntPoint::new(0, 2),
                IntPoint::new(4, 2),
                IntPoint::new(4, 5),
                IntPoint::new(0, 5),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result.area(), 14i64);
    }

    #[test]
    fn test_12() {
        let subj = [
            vec![
                IntPoint::new(2, 0),
                IntPoint::new(5, 0),
                IntPoint::new(5, 5),
                IntPoint::new(2, 5),
            ],
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(5, 0),
                IntPoint::new(5, 5),
                IntPoint::new(0, 5),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result.area(), 25i64);
    }

    #[test]
    fn test_13() {
        let subj = [
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(5, 0),
                IntPoint::new(5, 5),
                IntPoint::new(0, 5),
            ],
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(5, 0),
                IntPoint::new(5, 5),
                IntPoint::new(0, 5),
            ],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result.area(), 25i64);
    }

    #[test]
    fn test_14() {
        let subj = [
            vec![IntPoint::new(0, 0), IntPoint::new(0, 2), IntPoint::new(2, 0)],
            vec![IntPoint::new(0, 0), IntPoint::new(2, 0), IntPoint::new(0, -2)],
            vec![IntPoint::new(0, 0), IntPoint::new(0, -2), IntPoint::new(-2, 0)],
            vec![IntPoint::new(0, 0), IntPoint::new(-2, 0), IntPoint::new(0, 2)],
        ];

        let mut overlay = Overlay::with_contours(&subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result.area(), 8i64);
    }

    #[test]
    fn test_empty_input() {
        let subj: &[IntContour<i32>] = &[];

        let mut overlay = Overlay::with_contours(subj, &[]);
        let result = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);

        assert_eq!(result.len(), 0);
    }
}
