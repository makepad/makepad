// Copyright 2019 the Kurbo Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A generic trait for shapes.

use crate::{segments, BezPath, Circle, Line, PathEl, Point, Rect, RoundedRect, Segments};

/// A generic trait for open and closed shapes.
pub trait Shape {
    /// The iterator returned by the [`path_elements`] method.
    type PathElementsIter<'iter>: Iterator<Item = PathEl> + 'iter
    where
        Self: 'iter;

    /// Returns an iterator over this shape expressed as [`PathEl`]s.
    fn path_elements(&self, tolerance: f64) -> Self::PathElementsIter<'_>;

    /// Convert to a Bézier path.
    fn to_path(&self, tolerance: f64) -> BezPath {
        self.path_elements(tolerance).collect()
    }

    /// Convert into a Bézier path.
    fn into_path(self, tolerance: f64) -> BezPath
    where
        Self: Sized,
    {
        self.to_path(tolerance)
    }

    /// Returns an iterator over this shape expressed as Bézier path segments.
    fn path_segments(&self, tolerance: f64) -> Segments<Self::PathElementsIter<'_>> {
        segments(self.path_elements(tolerance))
    }

    /// Signed area.
    fn area(&self) -> f64;

    /// Total length of perimeter.
    fn perimeter(&self, accuracy: f64) -> f64;

    /// The winding number of a point.
    fn winding(&self, pt: Point) -> i32;

    /// Returns `true` if the [`Point`] is inside this shape.
    fn contains(&self, pt: Point) -> bool {
        self.winding(pt) != 0
    }

    /// The smallest rectangle that encloses the shape.
    fn bounding_box(&self) -> Rect;

    /// If the shape is a line, make it available.
    fn as_line(&self) -> Option<Line> {
        None
    }

    /// If the shape is a rectangle, make it available.
    fn as_rect(&self) -> Option<Rect> {
        None
    }

    /// If the shape is a rounded rectangle, make it available.
    fn as_rounded_rect(&self) -> Option<RoundedRect> {
        None
    }

    /// If the shape is a circle, make it available.
    fn as_circle(&self) -> Option<Circle> {
        None
    }

    /// If the shape is stored as a slice of path elements, make that available.
    fn as_path_slice(&self) -> Option<&[PathEl]> {
        None
    }
}

/// Blanket implementation so `impl Shape` will accept owned or reference.
impl<'a, T: Shape> Shape for &'a T {
    type PathElementsIter<'iter>
        = T::PathElementsIter<'iter>
    where
        T: 'iter,
        'a: 'iter;

    fn path_elements(&self, tolerance: f64) -> Self::PathElementsIter<'_> {
        (*self).path_elements(tolerance)
    }

    fn to_path(&self, tolerance: f64) -> BezPath {
        (*self).to_path(tolerance)
    }

    fn path_segments(&self, tolerance: f64) -> Segments<Self::PathElementsIter<'_>> {
        (*self).path_segments(tolerance)
    }

    fn area(&self) -> f64 {
        (*self).area()
    }

    fn perimeter(&self, accuracy: f64) -> f64 {
        (*self).perimeter(accuracy)
    }

    fn winding(&self, pt: Point) -> i32 {
        (*self).winding(pt)
    }

    fn bounding_box(&self) -> Rect {
        (*self).bounding_box()
    }

    fn as_line(&self) -> Option<Line> {
        (*self).as_line()
    }

    fn as_rect(&self) -> Option<Rect> {
        (*self).as_rect()
    }

    fn as_rounded_rect(&self) -> Option<RoundedRect> {
        (*self).as_rounded_rect()
    }

    fn as_circle(&self) -> Option<Circle> {
        (*self).as_circle()
    }

    fn as_path_slice(&self) -> Option<&[PathEl]> {
        (*self).as_path_slice()
    }
}
