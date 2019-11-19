use crate::OutlinePoint;
use geometry::{Point, Transform, Transformation};
use internal_iter::{
    ExtendFromInternalIterator, InternalIterator, IntoInternalIterator,
};
use path::PathCommand;
use std::iter::Cloned;
use std::slice::Iter;

/// The outline for a glyph.
///
/// An outline consists of one or more closed contours, each of which consists of one or more
/// quadratic b-spline curve segments, which are described by a sequence of outline points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    contour_ends: Vec<usize>,
    points: Vec<OutlinePoint>,
}

impl Outline {
    /// Creates a new empty outline.
    pub fn new() -> Outline {
        Outline::default()
    }

    /// Returns an iterator over the contours of `self`.
    pub fn contours(&self) -> Contours {
        Contours {
            contour_start: 0,
            contour_ends: self.contour_ends.iter().cloned(),
            points: &self.points,
        }
    }

    /// Returns an slice of the points of `self`.
    pub fn points(&self) -> &[OutlinePoint] {
        &self.points
    }

    /// Returns an iterator over the path commands that correspond to `self`.
    pub fn commands(&self) -> Commands {
        Commands {
            contours: self.contours(),
        }
    }

    /// Returns a mutable slice of the points of `self`.
    pub fn points_mut(&mut self) -> &mut [OutlinePoint] {
        &mut self.points
    }

    /// Returns a builder for a contour.
    pub fn begin_contour(&mut self) -> ContourBuilder {
        ContourBuilder {
            contour_ends: &mut self.contour_ends,
            points: &mut self.points,
        }
    }
}

impl<'a> ExtendFromInternalIterator<Contour<'a>> for Outline {
    fn extend_from_internal_iter<I>(&mut self, internal_iter: I)
    where
        I: IntoInternalIterator<Item = Contour<'a>>,
    {
        internal_iter
            .into_internal_iter()
            .for_each(&mut |other_contour| {
                let mut contour = self.begin_contour();
                contour.extend_from_internal_iter(other_contour.points().iter().cloned());
                contour.end();
                true
            });
    }
}

impl Transform for Outline {
    fn transform<T>(mut self, t: &T) -> Outline
    where
        T: Transformation,
    {
        self.transform_mut(t);
        self
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        for point in self.points_mut() {
            point.transform_mut(t);
        }
    }
}

/// An iterator over the path commands that correspond to an outline.
#[derive(Clone, Debug)]
pub struct Contours<'a> {
    contour_start: usize,
    contour_ends: Cloned<Iter<'a, usize>>,
    points: &'a [OutlinePoint],
}

impl<'a> Iterator for Contours<'a> {
    type Item = Contour<'a>;

    fn next(&mut self) -> Option<Contour<'a>> {
        self.contour_ends.next().map(|contour_end| {
            let contour_start = self.contour_start;
            self.contour_start = contour_end;
            Contour {
                points: &self.points[contour_start..contour_end],
            }
        })
    }
}

/// A contour in an outline.
#[derive(Clone, Copy, Debug)]
pub struct Contour<'a> {
    points: &'a [OutlinePoint],
}

impl<'a> Contour<'a> {
    pub fn points(&self) -> &'a [OutlinePoint] {
        &self.points
    }
}

/// Returns an iterator over the path commands that correspond to an outline.
#[derive(Clone, Debug)]
pub struct Commands<'a> {
    contours: Contours<'a>,
}

impl<'a> InternalIterator for Commands<'a> {
    type Item = PathCommand;

    fn for_each<F>(self, f: &mut F) -> bool
    where
        F: FnMut(PathCommand) -> bool,
    {
        // To convert a sequence of quadratic b-spline curve segments to a sequence of quadratic
        // Bezier curve segments, we need to insert a new endpoint at the midpoint of each pair
        // of adjacent off curve points.
        for contour in self.contours {
            // The off curve point we encountered before the first on curve point, if it exists.
            let mut first_off_curve_point: Option<Point> = None;
            // The first on curve point we encountered.
            let mut first_on_curve_point: Option<Point> = None;
            // The last off curve point we encountered.
            let mut last_off_curve_point: Option<Point> = None;
            for point in contour.points() {
                if first_on_curve_point.is_none() {
                    if point.is_on_curve {
                        if !f(PathCommand::MoveTo(point.point)) {
                            return false;
                        }
                        first_on_curve_point = Some(point.point);
                    } else {
                        if let Some(first_off_curve_point) = first_off_curve_point {
                            let midpoint = first_off_curve_point.lerp(point.point, 0.5);
                            if !f(PathCommand::MoveTo(midpoint)) {
                                return false;
                            }
                            first_on_curve_point = Some(midpoint);
                            last_off_curve_point = Some(point.point);
                        } else {
                            first_off_curve_point = Some(point.point);
                        }
                    }
                } else {
                    match (last_off_curve_point, point.is_on_curve) {
                        (None, false) => {
                            last_off_curve_point = Some(point.point);
                        }
                        (None, true) => {
                            if !f(PathCommand::LineTo(point.point)) {
                                return false;
                            }
                        }
                        (Some(last_point), false) => {
                            if !f(PathCommand::QuadraticTo(
                                last_point,
                                last_point.lerp(point.point, 0.5),
                            )) {
                                return false;
                            }
                            last_off_curve_point = Some(point.point);
                        }
                        (Some(last_point), true) => {
                            if !f(PathCommand::QuadraticTo(last_point, point.point)) {
                                return false;
                            }
                            last_off_curve_point = None;
                        }
                    }
                }
            }
            if let Some(first_on_curve_point) = first_on_curve_point {
                match (last_off_curve_point, first_off_curve_point) {
                    (None, None) => {
                        if !f(PathCommand::LineTo(first_on_curve_point)) {
                            return false;
                        }
                    }
                    (None, Some(first_off_curve_point)) => {
                        if !f(PathCommand::QuadraticTo(
                            first_off_curve_point,
                            first_on_curve_point,
                        )) {
                            return false;
                        }
                    }
                    (Some(last_point), None) => {
                        if !f(PathCommand::QuadraticTo(last_point, first_on_curve_point)) {
                            return false;
                        }
                    }
                    (Some(last_point), Some(first_off_curve_point)) => {
                        let midpoint = last_point.lerp(first_off_curve_point, 0.5);
                        if !f(PathCommand::QuadraticTo(last_point, midpoint)) {
                            return false;
                        }
                        if !f(PathCommand::QuadraticTo(
                            first_off_curve_point,
                            first_on_curve_point,
                        )) {
                            return false;
                        }
                    }
                }
                if !f(PathCommand::Close) {
                    return false;
                }
            }
        }
        true
    }
}

#[derive(Debug)]
pub struct ContourBuilder<'a> {
    contour_ends: &'a mut Vec<usize>,
    points: &'a mut Vec<OutlinePoint>,
}

impl<'a> ContourBuilder<'a> {
    pub fn end(self) {}

    pub fn push(&mut self, point: OutlinePoint) {
        self.points.push(point);
    }
}

impl<'a> Drop for ContourBuilder<'a> {
    fn drop(&mut self) {
        if self.points.len() != self.contour_ends.last().cloned().unwrap_or(0) {
            self.contour_ends.push(self.points.len());
        }
    }
}

impl<'a> ExtendFromInternalIterator<OutlinePoint> for ContourBuilder<'a> {
    fn extend_from_internal_iter<I>(&mut self, internal_iter: I)
    where
        I: IntoInternalIterator<Item = OutlinePoint>,
    {
        internal_iter.into_internal_iter().for_each(&mut |point| {
            self.push(point);
            true
        });
    }
}
