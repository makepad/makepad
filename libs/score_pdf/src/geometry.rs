//! Deterministic PDF-point geometry used at the file boundary.

use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Self) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Rect {
    pub const fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub fn from_points(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
    }

    pub fn width(self) -> f64 {
        (self.max_x - self.min_x).max(0.0)
    }

    pub fn height(self) -> f64 {
        (self.max_y - self.min_y).max(0.0)
    }

    pub fn area(self) -> f64 {
        self.width() * self.height()
    }

    pub fn center(self) -> Point {
        Point::new(
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
        )
    }

    pub fn contains(self, point: Point) -> bool {
        point.x >= self.min_x
            && point.x <= self.max_x
            && point.y >= self.min_y
            && point.y <= self.max_y
    }

    pub fn intersects(self, other: Self) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }

    pub fn overlap_x(self, other: Self) -> f64 {
        (self.max_x.min(other.max_x) - self.min_x.max(other.min_x)).max(0.0)
    }

    pub fn include_point(self, point: Point) -> Self {
        Self::new(
            self.min_x.min(point.x),
            self.min_y.min(point.y),
            self.max_x.max(point.x),
            self.max_y.max(point.y),
        )
    }

    pub fn union(self, other: Self) -> Self {
        Self::new(
            self.min_x.min(other.min_x),
            self.min_y.min(other.min_y),
            self.max_x.max(other.max_x),
            self.max_y.max(other.max_y),
        )
    }

    pub fn expand(self, amount: f64) -> Self {
        Self::new(
            self.min_x - amount,
            self.min_y - amount,
            self.max_x + amount,
            self.max_y + amount,
        )
    }

    pub fn finite(self) -> bool {
        self.min_x.is_finite()
            && self.min_y.is_finite()
            && self.max_x.is_finite()
            && self.max_y.is_finite()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f64; 6]);

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub const fn new(values: [f64; 6]) -> Self {
        Self(values)
    }

    pub fn transform_point(self, point: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        Point::new(
            a * point.x + c * point.y + e,
            b * point.x + d * point.y + f,
        )
    }

    /// Matrix product `self * rhs`, using PDF's six-value affine layout.
    pub fn then(self, rhs: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [g, h, i, j, k, l] = rhs.0;
        Self([
            a * g + c * h,
            b * g + d * h,
            a * i + c * j,
            b * i + d * j,
            a * k + c * l + e,
            b * k + d * l + f,
        ])
    }

    pub fn scale_magnitude(self) -> f64 {
        let [a, b, c, d, _, _] = self.0;
        let x = (a * a + b * b).sqrt();
        let y = (c * c + d * d).sqrt();
        (x + y) * 0.5
    }

    pub fn unit_square_bounds(self) -> Rect {
        let points = [
            self.transform_point(Point::new(0.0, 0.0)),
            self.transform_point(Point::new(1.0, 0.0)),
            self.transform_point(Point::new(0.0, 1.0)),
            self.transform_point(Point::new(1.0, 1.0)),
        ];
        bounds(&points).unwrap_or_default()
    }
}

pub fn bounds(points: &[Point]) -> Option<Rect> {
    let first = *points.first()?;
    Some(points.iter().skip(1).fold(
        Rect::new(first.x, first.y, first.x, first.y),
        |rect, point| rect.include_point(*point),
    ))
}

pub(crate) fn median(values: &mut [f64]) -> Option<f64> {
    let mut finite: Vec<_> = values.iter().copied().filter(|value| value.is_finite()).collect();
    if finite.is_empty() {
        return None;
    }
    finite.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let middle = finite.len() / 2;
    if finite.len() % 2 == 0 {
        Some((finite[middle - 1] + finite[middle]) * 0.5)
    } else {
        Some(finite[middle])
    }
}

pub(crate) fn approximately(left: f64, right: f64, tolerance: f64) -> bool {
    (left - right).abs() <= tolerance
}
