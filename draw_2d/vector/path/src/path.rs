use crate::PathCommand;
use makepad_geometry::{Point, Transform, Transformation};
use makepad_internal_iter::{
    ExtendFromInternalIterator, FromInternalIterator, InternalIterator, IntoInternalIterator,
};
use std::iter::Cloned;
use std::slice::Iter;

/// A sequence of commands that defines a set of contours, each of which consists of a sequence of
/// curve segments. Each contour is either open or closed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    verbs: Vec<Verb>,
    points: Vec<Point>,
}

impl Path {
    /// Creates a new empty path.
    pub fn new() -> Path {
        Path::default()
    }

    /// Returns a slice of the points that make up `self`.
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Returns an iterator over the commands that make up `self`.
    pub fn commands(&self) -> Commands {
        Commands {
            verbs: self.verbs.iter().cloned(),
            points: self.points.iter().cloned(),
        }
    }

    /// Returns a mutable slice of the points that make up `self`.
    pub fn points_mut(&mut self) -> &mut [Point] {
        &mut self.points
    }

    /// Adds a new contour, starting at the given point.
    pub fn move_to(&mut self, p: Point) {
        self.verbs.push(Verb::MoveTo);
        self.points.push(p);
    }

    /// Adds a line segment to the current contour, starting at the current point.
    pub fn line_to(&mut self, p: Point) {
        self.verbs.push(Verb::LineTo);
        self.points.push(p);
    }

    // Adds a quadratic Bezier curve segment to the current contour, starting at the current point.
    pub fn quadratic_to(&mut self, p1: Point, p: Point) {
        self.verbs.push(Verb::QuadraticTo);
        self.points.push(p1);
        self.points.push(p);
    }

    /// Closes the current contour.
    pub fn close(&mut self) {
        self.verbs.push(Verb::Close);
    }

    /// Clears `self`.
    pub fn clear(&mut self) {
        self.verbs.clear();
        self.points.clear();
    }
}

impl ExtendFromInternalIterator<PathCommand> for Path {
    fn extend_from_internal_iter<I>(&mut self, internal_iter: I)
    where
        I: IntoInternalIterator<Item = PathCommand>,
    {
        internal_iter.into_internal_iter().for_each(&mut |command| {
            match command {
                PathCommand::MoveTo(p) => self.move_to(p),
                PathCommand::LineTo(p) => self.line_to(p),
                PathCommand::QuadraticTo(p1, p) => self.quadratic_to(p1, p),
                PathCommand::Close => self.close(),
            }
            true
        });
    }
}

impl FromInternalIterator<PathCommand> for Path {
    fn from_internal_iter<I>(internal_iter: I) -> Self
    where
        I: IntoInternalIterator<Item = PathCommand>,
    {
        let mut path = Path::new();
        path.extend_from_internal_iter(internal_iter);
        path
    }
}

impl Transform for Path {
    fn transform<T>(mut self, t: &T) -> Path
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

/// An iterator over the commands that make up a path.
#[derive(Clone, Debug)]
pub struct Commands<'a> {
    verbs: Cloned<Iter<'a, Verb>>,
    points: Cloned<Iter<'a, Point>>,
}

impl<'a> Iterator for Commands<'a> {
    type Item = PathCommand;

    fn next(&mut self) -> Option<PathCommand> {
        self.verbs.next().map(|verb| match verb {
            Verb::MoveTo => PathCommand::MoveTo(self.points.next().unwrap()),
            Verb::LineTo => PathCommand::LineTo(self.points.next().unwrap()),
            Verb::QuadraticTo => {
                PathCommand::QuadraticTo(self.points.next().unwrap(), self.points.next().unwrap())
            }
            Verb::Close => PathCommand::Close,
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Verb {
    MoveTo,
    LineTo,
    QuadraticTo,
    Close,
}
