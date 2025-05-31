#[cfg(all(feature = "libm", not(feature = "std")))]
use crate::nostd_float::FloatExt;

/// An (x, y) coordinate.
///
/// # Example
/// ```
/// use ab_glyph_rasterizer::{point, Point};
/// let p: Point = point(0.1, 23.2);
/// ```
#[derive(Clone, Copy, Default, PartialEq, PartialOrd)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl core::fmt::Debug for Point {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "point({:?}, {:?})", self.x, self.y)
    }
}

impl Point {
    #[inline]
    pub(crate) fn distance_to(self, other: Point) -> f32 {
        let d = other - self;
        (d.x * d.x + d.y * d.y).sqrt()
    }
}

/// [`Point`] constructor.
///
/// # Example
/// ```
/// # use ab_glyph_rasterizer::{point, Point};
/// let p = point(0.1, 23.2);
/// ```
#[inline]
pub fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

/// Linear interpolation between points.
#[inline]
pub(crate) fn lerp(t: f32, p0: Point, p1: Point) -> Point {
    point(p0.x + t * (p1.x - p0.x), p0.y + t * (p1.y - p0.y))
}

impl core::ops::Sub for Point {
    type Output = Point;
    /// Subtract rhs.x from x, rhs.y from y.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// let p1 = point(1.0, 2.0) - point(2.0, 1.5);
    ///
    /// assert!((p1.x - -1.0).abs() <= f32::EPSILON);
    /// assert!((p1.y - 0.5).abs() <= f32::EPSILON);
    /// ```
    #[inline]
    fn sub(self, rhs: Point) -> Point {
        point(self.x - rhs.x, self.y - rhs.y)
    }
}

impl core::ops::Add for Point {
    type Output = Point;
    /// Add rhs.x to x, rhs.y to y.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// let p1 = point(1.0, 2.0) + point(2.0, 1.5);
    ///
    /// assert!((p1.x - 3.0).abs() <= f32::EPSILON);
    /// assert!((p1.y - 3.5).abs() <= f32::EPSILON);
    /// ```
    #[inline]
    fn add(self, rhs: Point) -> Point {
        point(self.x + rhs.x, self.y + rhs.y)
    }
}

impl core::ops::AddAssign for Point {
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// let mut p1 = point(1.0, 2.0);
    /// p1 += point(2.0, 1.5);
    ///
    /// assert!((p1.x - 3.0).abs() <= f32::EPSILON);
    /// assert!((p1.y - 3.5).abs() <= f32::EPSILON);
    /// ```
    #[inline]
    fn add_assign(&mut self, other: Self) {
        self.x += other.x;
        self.y += other.y;
    }
}

impl core::ops::SubAssign for Point {
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// let mut p1 = point(1.0, 2.0);
    /// p1 -= point(2.0, 1.5);
    ///
    /// assert!((p1.x - -1.0).abs() <= f32::EPSILON);
    /// assert!((p1.y - 0.5).abs() <= f32::EPSILON);
    /// ```
    #[inline]
    fn sub_assign(&mut self, other: Self) {
        self.x -= other.x;
        self.y -= other.y;
    }
}

impl<F: Into<f32>> From<(F, F)> for Point {
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// let p: Point = (23_f32, 34.5_f32).into();
    /// let p2: Point = (5u8, 44u8).into();
    /// ```
    #[inline]
    fn from((x, y): (F, F)) -> Self {
        point(x.into(), y.into())
    }
}

impl<F: Into<f32>> From<[F; 2]> for Point {
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// let p: Point = [23_f32, 34.5].into();
    /// let p2: Point = [5u8, 44].into();
    /// ```
    #[inline]
    fn from([x, y]: [F; 2]) -> Self {
        point(x.into(), y.into())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn distance_to() {
        let distance = point(0.0, 0.0).distance_to(point(3.0, 4.0));
        assert!((distance - 5.0).abs() <= f32::EPSILON);
    }
}
