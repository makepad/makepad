//! This module provides methods to simplify paths and shapes by reducing complexity
//! (e.g., removing small artifacts or shapes below a certain area threshold) based on a build rule.

use crate::core::fill_rule::FillRule;
use crate::core::overlay::ContourDirection;
use crate::core::overlay::ContourDirection::Clockwise;
use crate::core::overlay::{IntOverlayOptions, Overlay, ShapeType};
use crate::core::overlay_rule::OverlayRule;
use crate::i_float::int::number::int::IntNumber;
use crate::i_float::int::point::IntPoint;
use alloc::vec;
use i_key_sort::sort::key::SortKey;
use i_shape::flat::buffer::FlatContoursBuffer;

use crate::segm::build::BuildSegments;
use i_shape::int::count::PointsCount;
use i_shape::int::path::ContourExtension;
use i_shape::int::shape::{IntContour, IntShape, IntShapes};
use i_tree::{Expiration, LayoutNumber};

/// Trait `Simplify` provides a method to simplify geometric shapes by reducing the number of points in contours or shapes
/// while preserving overall shape and topology. The method applies a minimum area threshold and a build rule to
/// determine which areas should be retained or excluded.
pub trait Simplify<I: IntNumber> {
    /// Simplifies the shape or collection of points, contours, or shapes, based on a specified minimum area threshold.
    ///
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - `options`: Adjust custom behavior.
    /// # Shape Representation
    /// The output is a `IntShapes<I>`, where:
    /// - The outer `Vec<IntShape<I>>` represents a set of shapes.
    /// - Each shape `Vec<IntContour<I>>` represents a collection of contours, where the first contour is the outer boundary, and all subsequent contours are holes in this boundary.
    /// - Each path `Vec<IntPoint>` is a sequence of points, forming a closed path.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn simplify(&self, fill_rule: FillRule, options: IntOverlayOptions<I::WideUInt>) -> IntShapes<I>;
}

impl<I> Simplify<I> for [IntPoint<I>]
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    fn simplify(&self, fill_rule: FillRule, options: IntOverlayOptions<I::WideUInt>) -> IntShapes<I> {
        match Overlay::new_custom(self.len(), options, Default::default()).simplify_contour(self, fill_rule) {
            Some(shapes) => shapes,
            None => vec![vec![self.to_vec()]],
        }
    }
}

impl<I> Simplify<I> for [IntContour<I>]
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    fn simplify(&self, fill_rule: FillRule, options: IntOverlayOptions<I::WideUInt>) -> IntShapes<I> {
        match Overlay::new_custom(self.len(), options, Default::default()).simplify_shape(self, fill_rule) {
            Some(shapes) => shapes,
            None => vec![self.to_vec()],
        }
    }
}

impl<I> Simplify<I> for [IntShape<I>]
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    fn simplify(&self, fill_rule: FillRule, options: IntOverlayOptions<I::WideUInt>) -> IntShapes<I> {
        Overlay::new_custom(self.points_count(), options, Default::default()).simplify_shapes(self, fill_rule)
    }
}

enum ContourFillDirection {
    Reverse,
    Correct,
    Empty,
}

impl<I> Overlay<I>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    /// Fast-path simplification for a single contour.
    ///
    /// Skips full overlay if the contour is already simple (no splits, no loops, no collinear issues).
    /// Ensures correct winding order based on `fill_rule` and `options.output_direction`.
    ///
    /// Returns `None` if the contour is valid and needs no changes, or `Some(IntShapes<I>)` with the simplified result.
    #[inline]
    pub fn simplify_contour(&mut self, contour: &[IntPoint<I>], fill_rule: FillRule) -> Option<IntShapes<I>> {
        self.clear();

        let is_perfect = self.find_intersections(contour);

        if is_perfect {
            // the path is already perfect
            // need to check fill rule direction
            let fill_direction = Self::contour_direction(self.options.output_direction, fill_rule, contour);

            return match fill_direction {
                ContourFillDirection::Reverse => {
                    let mut rev_contour = contour.to_vec();
                    rev_contour.reverse();
                    Some(vec![vec![rev_contour]])
                }
                ContourFillDirection::Correct => None,
                ContourFillDirection::Empty => Some(vec![]),
            };
        }

        let mut boolean_buffer = self.boolean_buffer.take().unwrap_or_default();

        let result = self
            .graph_builder
            .build_boolean_overlay(
                fill_rule,
                OverlayRule::Subject,
                self.options,
                &self.solver,
                &self.segments,
            )
            .extract_shapes(OverlayRule::Subject, &mut boolean_buffer);

        self.boolean_buffer = Some(boolean_buffer);

        Some(result)
    }

    #[inline]
    fn contour_direction(
        output_direction: ContourDirection,
        fill_rule: FillRule,
        contour: &[IntPoint<I>],
    ) -> ContourFillDirection {
        let contour_clockwise = contour.is_clockwise_ordered();
        let output_clockwise = output_direction == Clockwise;

        match fill_rule {
            FillRule::EvenOdd | FillRule::NonZero => {
                if contour_clockwise != output_clockwise {
                    ContourFillDirection::Reverse
                } else {
                    ContourFillDirection::Correct
                }
            }
            FillRule::Positive => {
                if contour_clockwise == output_clockwise {
                    ContourFillDirection::Correct
                } else {
                    ContourFillDirection::Empty
                }
            }
            FillRule::Negative => {
                if contour_clockwise != output_clockwise {
                    ContourFillDirection::Correct
                } else {
                    ContourFillDirection::Empty
                }
            }
        }
    }

    #[inline]
    pub fn simplify_shape(&mut self, shape: &[IntContour<I>], fill_rule: FillRule) -> Option<IntShapes<I>> {
        if shape.len() == 1 {
            return self.simplify_contour(&shape[0], fill_rule);
        }
        self.clear();
        self.add_contours(shape, ShapeType::Subject);
        Some(self.overlay(OverlayRule::Subject, fill_rule))
    }

    #[inline]
    pub fn simplify_shapes(&mut self, shapes: &[IntShape<I>], fill_rule: FillRule) -> IntShapes<I> {
        self.clear();
        self.add_shapes(shapes, ShapeType::Subject);
        self.overlay(OverlayRule::Subject, fill_rule)
    }

    #[inline]
    pub fn simplify_flat_buffer(&mut self, flat_buffer: &mut FlatContoursBuffer<I>, fill_rule: FillRule) {
        self.clear();

        if flat_buffer.is_single_contour() {
            let first_contour = flat_buffer.as_first_contour();
            let is_perfect = self.find_intersections(first_contour);

            if is_perfect {
                // the path is already perfect
                // need to check fill rule direction
                let fill_direction =
                    Self::contour_direction(self.options.output_direction, fill_rule, first_contour);

                match fill_direction {
                    ContourFillDirection::Reverse => {
                        flat_buffer.as_first_contour_mut().reverse();
                    }
                    ContourFillDirection::Correct => {}
                    ContourFillDirection::Empty => flat_buffer.clear_and_reserve(0, 0),
                }

                return;
            }
        } else {
            self.add_flat_buffer(flat_buffer, ShapeType::Subject);
            self.split_solver.split_segments(&mut self.segments, &self.solver);
            if self.segments.is_empty() {
                flat_buffer.clear_and_reserve(0, 0);
                return;
            }
        }

        let mut boolean_buffer = self.boolean_buffer.take().unwrap_or_default();

        self.graph_builder
            .build_boolean_overlay(
                fill_rule,
                OverlayRule::Subject,
                self.options,
                &self.solver,
                &self.segments,
            )
            .extract_contours_into(OverlayRule::Subject, &mut boolean_buffer, flat_buffer);

        self.boolean_buffer = Some(boolean_buffer);
    }

    fn find_intersections(&mut self, contour: &[IntPoint<I>]) -> bool {
        let append_modified = self.segments.append_path_iter(
            contour.iter().copied(),
            ShapeType::Subject,
            self.options.preserve_input_collinear,
        );

        let split_modified = self.split_solver.split_segments(&mut self.segments, &self.solver);

        if split_modified || append_modified || self.segments.is_empty() {
            return false;
        }

        let mut buffer = self.boolean_buffer.take().unwrap_or_default();
        let has_loops = self
            .graph_builder
            .test_contour_for_loops(contour, &mut buffer.points);
        self.boolean_buffer = Some(buffer);

        !has_loops
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::core::overlay::IntOverlayOptions;
    use crate::core::simplify::Simplify;
    use crate::core::simplify::vec;
    use i_float::int::point::IntPoint;

    #[test]
    fn test_0() {
        let contour = vec![
            IntPoint::new(0, 0),
            IntPoint::new(10, 0),
            IntPoint::new(10, 10),
            IntPoint::new(0, 10),
        ];

        let mut rev_contour = contour.clone();
        rev_contour.reverse();

        let c0 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(c0[0][0].len(), 4);

        let c1 = rev_contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(c1[0][0].len(), 4);
    }

    #[test]
    fn test_1() {
        let contour = vec![
            IntPoint::new(0, 0),
            IntPoint::new(10, 10),
            IntPoint::new(10, 0),
            IntPoint::new(0, 10),
        ];

        let mut rev_contour = contour.clone();
        rev_contour.reverse();

        let r0 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r0.len(), 2);

        let r1 = rev_contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r1.len(), 2);
    }

    #[test]
    fn test_2() {
        // 2 outer contours, not intersections but share point
        let contour = vec![
            IntPoint::new(-2, -1),
            IntPoint::new(0, 0),
            IntPoint::new(2, 1),
            IntPoint::new(2, -1),
            IntPoint::new(0, 0),
            IntPoint::new(-2, 1),
        ];

        let mut rev_contour = contour.clone();
        rev_contour.reverse();

        let r0 = contour.simplify(FillRule::NonZero, IntOverlayOptions::keep_all_points());
        assert_eq!(r0.len(), 2);

        let r1 = rev_contour.simplify(FillRule::NonZero, IntOverlayOptions::keep_all_points());
        assert_eq!(r1.len(), 2);
    }

    #[test]
    fn test_3() {
        // outer and inner contours, not intersections but share point
        let contour = vec![
            IntPoint::new(0, 0),
            IntPoint::new(-3, 2),
            IntPoint::new(-3, -2),
            IntPoint::new(0, 0),
            IntPoint::new(-2, -1),
            IntPoint::new(-2, 1),
        ];

        let mut rev_contour = contour.clone();
        rev_contour.reverse();

        let r0 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r0.len(), 1);

        let r1 = rev_contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r1.len(), 1);
    }

    #[test]
    fn test_4() {
        // 2 inner contours (one inside other), not intersections but share point
        let contour = vec![
            IntPoint::new(0, 0),
            IntPoint::new(-3, 2),
            IntPoint::new(-3, -2),
            IntPoint::new(0, 0),
            IntPoint::new(-2, 1),
            IntPoint::new(-2, -1),
        ];

        let mut rev_contour = contour.clone();
        rev_contour.reverse();

        let r0 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r0.len(), 1);
        assert_eq!(r0[0].len(), 1);
        assert_eq!(r0[0][0].len(), 3);

        let r1 = rev_contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r1.len(), 1);
        assert_eq!(r1[0][0].len(), 3);
    }

    #[test]
    fn test_without_points() {
        let contour: &[IntPoint] = &[];

        let mut rev_contour = contour.to_vec();
        rev_contour.reverse();

        let r0 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r0.len(), 0);

        let r1 = rev_contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r1.len(), 0);
    }

    #[test]
    fn test_with_single_point() {
        let contour = vec![IntPoint::new(0, 0)];

        let mut rev_contour = contour.clone();
        rev_contour.reverse();

        let r0 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r0.len(), 0);

        let r1 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r1.len(), 0);
    }

    #[test]
    fn test_with_pair_of_points() {
        let contour = vec![IntPoint::new(0, 0), IntPoint::new(1, 1)];

        let mut rev_contour = contour.clone();
        rev_contour.reverse();

        let r0 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r0.len(), 0);

        let r1 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r1.len(), 0);
    }

    #[test]
    fn test_near_collinear_paths() {
        let contour = vec![
            IntPoint::new(-100, -100),
            IntPoint::new(0, 0),
            IntPoint::new(101, 100),
        ];

        let mut rev_contour = contour.clone();
        rev_contour.reverse();

        let r0 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r0.len(), 1);

        let r1 = contour.simplify(FillRule::NonZero, Default::default());
        assert_eq!(r1.len(), 1);
    }
}
