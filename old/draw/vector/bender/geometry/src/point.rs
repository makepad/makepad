use crate::Vector;
use std::ops::{Add, Sub};

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Point {
    x: f32,
    y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn x(self) -> f32 {
        self.x
    }

    pub fn y(self) -> f32 {
        self.y
    }

    pub fn min(self, other: Self) -> Self {
        if self <= other {
            self
        } else {
            other
        }
    }

    pub fn max(self, other: Self) -> Self {
        if self <= other {
            other
        } else {
            self
        }
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            x: self.x * (1.0 - t) + other.x * t,
            y: self.y * (1.0 - t) + other.y * t,
        }
    }
}

impl Add<Vector> for Point {
    type Output = Self;

    fn add(self, v: Vector) -> Self::Output {
        Self {
            x: self.x + v.x(),
            y: self.y + v.y(),
        }
    }
}

impl Sub for Point {
    type Output = Vector;

    fn sub(self, other: Self) -> Self::Output {
        Vector::new(self.x() - other.x(), self.y() - other.y())
    }
}

impl Sub<Vector> for Point {
    type Output = Self;

    fn sub(self, v: Vector) -> Self::Output {
        Self {
            x: self.x - v.x(),
            y: self.y - v.y(),
        }
    }
}
