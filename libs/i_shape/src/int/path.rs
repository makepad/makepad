use crate::int::shape::IntContour;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;

pub type IntPath<I> = Vec<IntPoint<I>>;
pub type IntPaths<I> = Vec<IntPath<I>>;

pub trait ContourExtension<I: IntNumber> {
    fn unsafe_area(&self) -> I::Wide;
    fn is_convex(&self) -> bool;
    fn is_clockwise_ordered(&self) -> bool;
    fn contains(&self, point: IntPoint<I>) -> bool;
    fn to_reversed(&self) -> IntContour<I>;
}

impl<I: IntNumber> ContourExtension<I> for [IntPoint<I>] {
    /// The area of the `Path`.
    /// - Returns: A positive double area if path is counter-clockwise and negative double area otherwise.
    fn unsafe_area(&self) -> I::Wide {
        let n = self.len();
        let mut p0 = self[n - 1];
        let mut area = I::Wide::ZERO;

        for &p1 in self.iter() {
            let a = p0.x.wide().wrapping_mul(p1.y.wide());
            let b = p0.y.wide().wrapping_mul(p1.x.wide());
            area = area.wrapping_add(a).wrapping_sub(b);
            p0 = p1;
        }

        area
    }

    /// Determines if the `Path` is convex.
    ///
    /// A convex polygon is a simple polygon (not self-intersecting) in which
    /// the line segment between any two points along the boundary never
    /// goes outside the polygon. This method assumes that the points in `Path`
    /// are ordered (either clockwise or counter-clockwise) and the path is not
    /// self-intersecting.
    ///
    /// - Returns: A Boolean value indicating whether the path is convex.
    ///   - Returns `true` if the path is convex.
    ///   - Returns `false` otherwise.
    fn is_convex(&self) -> bool {
        let n = self.len();
        if n <= 2 {
            return true;
        }

        let p0 = self[n - 2];
        let mut p1 = self[n - 1];
        let mut e0 = p1 - p0;

        let mut sign = I::Wide::ZERO;
        for &p2 in self.iter() {
            let e1 = p2 - p1;
            let cross = e1.cross_product(e0).signum();
            if cross == I::Wide::ZERO {
                let dot = e1.dot_product(e0);
                if dot < I::Wide::ZERO {
                    return false;
                }
            } else if sign == I::Wide::ZERO {
                sign = cross
            } else if sign != cross {
                return false;
            }

            e0 = e1;
            p1 = p2;
        }

        true
    }

    /// The wind direction of the `Path`.
    /// - Returns: A Boolean value indicating whether the path is clockwise ordered.
    ///  - Returns `true` if the path is clockwise ordered.
    ///  - Returns `false` otherwise.
    #[inline(always)]
    fn is_clockwise_ordered(&self) -> bool {
        self.unsafe_area() <= I::Wide::ZERO
    }

    /// Checks if a point is contained within the `Path`.
    /// - Parameter p: The `IntPoint` point to check.
    /// - Returns: A boolean value indicating whether the point is within the path.
    fn contains(&self, point: IntPoint<I>) -> bool {
        let n = self.len();
        let mut is_contain = false;
        let mut b = self[n - 1];
        for &a in self.iter() {
            let is_in_range = (a.y > point.y) != (b.y > point.y);
            if is_in_range {
                let dx = b.x - a.x;
                let dy = b.y - a.y;
                let sx = (point.y - a.y) * dx / dy + a.x;
                if point.x < sx {
                    is_contain = !is_contain;
                }
            }
            b = a;
        }

        is_contain
    }

    #[inline]
    fn to_reversed(&self) -> IntContour<I> {
        let mut contour = self.to_vec();
        contour.reverse();
        contour
    }
}

#[cfg(test)]
mod tests {
    use crate::int::path::ContourExtension;
    use crate::int_path;

    #[test]
    fn test_0() {
        let contour = int_path![
            [-314572800, 209715200],
            [-314572800, -209715200],
            [-209715200, -314572800],
            [209715200, -314572800],
            [314572800, -209715200],
            [314572800, 209715200],
            [209715200, 314572800],
            [-209715200, 314572800],
        ];

        let area: i64 = contour.unsafe_area();
        let abs_area = area.unsigned_abs() as usize >> 1;
        assert!(area > 0);
        assert!(abs_area > 1);
    }
}
