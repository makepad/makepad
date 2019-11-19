use crate::LinePathCommand;
use geometry::{Point, Transform, Transformation};
use internal_iter::{
    ExtendFromInternalIterator, FromInternalIterator, InternalIterator, IntoInternalIterator,
};
use std::iter::Cloned;
use std::slice::Iter;

/// A sequence of commands that defines a set of contours, each of which consists of a sequence of
/// line segments. Each contour is either open or closed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinePath {
    verbs: Vec<Verb>,
    points: Vec<Point>,
}

impl LinePath {
    /// Creates a new empty line path.
    pub fn new() -> LinePath {
        LinePath::default()
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

impl ExtendFromInternalIterator<LinePathCommand> for LinePath {
    fn extend_from_internal_iter<I>(&mut self, internal_iter: I)
    where
        I: IntoInternalIterator<Item = LinePathCommand>,
    {
        internal_iter.into_internal_iter().for_each(&mut |command| {
            match command {
                LinePathCommand::MoveTo(p) => self.move_to(p),
                LinePathCommand::LineTo(p) => self.line_to(p),
                LinePathCommand::Close => self.close(),
            }
            true
        });
    }
}

impl FromInternalIterator<LinePathCommand> for LinePath {
    fn from_internal_iter<I>(internal_iter: I) -> Self
    where
        I: IntoInternalIterator<Item = LinePathCommand>,
    {
        let mut path = LinePath::new();
        path.extend_from_internal_iter(internal_iter);
        path
    }
}

impl Transform for LinePath {
    fn transform<T>(mut self, t: &T) -> LinePath
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

/// An iterator over the commands that make up a line path.
#[derive(Clone, Debug)]
pub struct Commands<'a> {
    verbs: Cloned<Iter<'a, Verb>>,
    points: Cloned<Iter<'a, Point>>,
}

impl<'a> Iterator for Commands<'a> {
    type Item = LinePathCommand;

    fn next(&mut self) -> Option<LinePathCommand> {
        self.verbs.next().map(|verb| match verb {
            Verb::MoveTo => LinePathCommand::MoveTo(self.points.next().unwrap()),
            Verb::LineTo => LinePathCommand::LineTo(self.points.next().unwrap()),
            Verb::Close => LinePathCommand::Close,
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Verb {
    MoveTo,
    LineTo,
    Close,
}
