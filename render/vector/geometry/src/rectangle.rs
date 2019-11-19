use crate::{Point, Transform, Transformation};

/// An axis-aligned rectangle in 2-dimensional Euclidian space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Rectangle {
    pub p_min: Point,
    pub p_max: Point,
}

impl Rectangle {
    /// Creates a new rectangle with the given minimum and maximum point.
    pub fn new(p_min: Point, p_max: Point) -> Rectangle {
        Rectangle { p_min, p_max }
    }
}

impl Transform for Rectangle {
    fn transform<T>(self, t: &T) -> Rectangle
    where
        T: Transformation,
    {
        Rectangle::new(self.p_min.transform(t), self.p_max.transform(t))
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        *self = self.transform(t);
    }
}
