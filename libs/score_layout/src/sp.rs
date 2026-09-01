//! The staff-space unit and small geometry types.
//!
//! One staff space is the distance between two adjacent staff lines. All
//! layout happens in `f64` staff spaces so results are independent of font
//! size and zoom; conversion to device units is the renderer's problem.
//! The y axis grows downward.

use std::cmp::Ordering;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A length in staff spaces.
///
/// A transparent `f64` newtype so lengths cannot be silently mixed with
/// unitless scalars or device pixels. Multiplying by a bare `f64` scales;
/// dividing two `Sp` values yields a unitless ratio.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, PartialOrd, Default, Debug)]
pub struct Sp(pub f64);

impl Sp {
    /// Zero staff spaces.
    pub const ZERO: Sp = Sp(0.0);

    /// The smaller of two lengths.
    pub fn min(self, other: Sp) -> Sp {
        Sp(self.0.min(other.0))
    }

    /// The larger of two lengths.
    pub fn max(self, other: Sp) -> Sp {
        Sp(self.0.max(other.0))
    }

    /// Clamp into `[lo, hi]`.
    pub fn clamp(self, lo: Sp, hi: Sp) -> Sp {
        Sp(self.0.clamp(lo.0, hi.0))
    }

    /// Absolute value.
    pub fn abs(self) -> Sp {
        Sp(self.0.abs())
    }

    /// Deterministic total ordering (IEEE `total_cmp`), for sorts.
    pub fn total_cmp(&self, other: &Sp) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Add for Sp {
    type Output = Sp;
    fn add(self, rhs: Sp) -> Sp {
        Sp(self.0 + rhs.0)
    }
}

impl Sub for Sp {
    type Output = Sp;
    fn sub(self, rhs: Sp) -> Sp {
        Sp(self.0 - rhs.0)
    }
}

impl Neg for Sp {
    type Output = Sp;
    fn neg(self) -> Sp {
        Sp(-self.0)
    }
}

impl AddAssign for Sp {
    fn add_assign(&mut self, rhs: Sp) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Sp {
    fn sub_assign(&mut self, rhs: Sp) {
        self.0 -= rhs.0;
    }
}

impl Mul<f64> for Sp {
    type Output = Sp;
    fn mul(self, rhs: f64) -> Sp {
        Sp(self.0 * rhs)
    }
}

impl Mul<Sp> for f64 {
    type Output = Sp;
    fn mul(self, rhs: Sp) -> Sp {
        Sp(self * rhs.0)
    }
}

impl Div<f64> for Sp {
    type Output = Sp;
    fn div(self, rhs: f64) -> Sp {
        Sp(self.0 / rhs)
    }
}

impl Div<Sp> for Sp {
    type Output = f64;
    fn div(self, rhs: Sp) -> f64 {
        self.0 / rhs.0
    }
}

impl Sum for Sp {
    fn sum<I: Iterator<Item = Sp>>(iter: I) -> Sp {
        Sp(iter.map(|s| s.0).sum())
    }
}

/// A point in staff-space coordinates, y growing downward.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub struct SpPoint {
    /// Horizontal position.
    pub x: Sp,
    /// Vertical position, growing downward.
    pub y: Sp,
}

impl SpPoint {
    /// Construct from staff-space components.
    pub fn new(x: Sp, y: Sp) -> SpPoint {
        SpPoint { x, y }
    }

    /// Construct from raw `f64` staff spaces.
    pub fn xy(x: f64, y: f64) -> SpPoint {
        SpPoint { x: Sp(x), y: Sp(y) }
    }

    /// Component-wise addition.
    pub fn add(self, other: SpPoint) -> SpPoint {
        SpPoint::new(self.x + other.x, self.y + other.y)
    }

    /// Component-wise subtraction.
    pub fn sub(self, other: SpPoint) -> SpPoint {
        SpPoint::new(self.x - other.x, self.y - other.y)
    }

    /// Scale both components.
    pub fn scale(self, s: f64) -> SpPoint {
        SpPoint::new(self.x * s, self.y * s)
    }

    /// Linear interpolation: `self` at `t = 0`, `other` at `t = 1`.
    pub fn lerp(self, other: SpPoint, t: f64) -> SpPoint {
        self.scale(1.0 - t).add(other.scale(t))
    }

    /// Euclidean distance to another point.
    pub fn distance(self, other: SpPoint) -> Sp {
        let dx = self.x.0 - other.x.0;
        let dy = self.y.0 - other.y.0;
        Sp((dx * dx + dy * dy).sqrt())
    }
}

/// An axis-aligned rectangle in staff spaces, y growing downward.
///
/// `y0` is the top edge (smaller y), `y1` the bottom edge (larger y).
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub struct SpRect {
    /// Left edge.
    pub x0: Sp,
    /// Top edge (smaller y).
    pub y0: Sp,
    /// Right edge.
    pub x1: Sp,
    /// Bottom edge (larger y).
    pub y1: Sp,
}

impl SpRect {
    /// Construct, normalizing so `x0 <= x1` and `y0 <= y1`.
    pub fn new(x0: Sp, y0: Sp, x1: Sp, y1: Sp) -> SpRect {
        SpRect {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }

    /// Construct from raw `f64` staff spaces, normalizing.
    pub fn xywh(x: f64, y: f64, w: f64, h: f64) -> SpRect {
        SpRect::new(Sp(x), Sp(y), Sp(x + w), Sp(y + h))
    }

    /// Width (always non-negative).
    pub fn width(&self) -> Sp {
        self.x1 - self.x0
    }

    /// Height (always non-negative).
    pub fn height(&self) -> Sp {
        self.y1 - self.y0
    }

    /// Signed distance from a point to the rectangle boundary:
    /// negative inside, positive outside, zero on the edge.
    pub fn signed_distance(&self, p: SpPoint) -> Sp {
        let dx = (self.x0.0 - p.x.0).max(p.x.0 - self.x1.0);
        let dy = (self.y0.0 - p.y.0).max(p.y.0 - self.y1.0);
        if dx <= 0.0 && dy <= 0.0 {
            // Inside: distance to nearest edge, negated.
            Sp(dx.max(dy))
        } else {
            let ox = dx.max(0.0);
            let oy = dy.max(0.0);
            Sp((ox * ox + oy * oy).sqrt())
        }
    }

    /// Grow the rectangle by `pad` on every side.
    pub fn inflate(&self, pad: Sp) -> SpRect {
        SpRect {
            x0: self.x0 - pad,
            y0: self.y0 - pad,
            x1: self.x1 + pad,
            y1: self.y1 + pad,
        }
    }
}

#[cfg(test)]
mod sp_tests {
    use super::*;

    #[test]
    fn arithmetic() {
        let a = Sp(2.0);
        let b = Sp(0.5);
        assert_eq!(a + b, Sp(2.5));
        assert_eq!(a - b, Sp(1.5));
        assert_eq!(a * 2.0, Sp(4.0));
        assert_eq!(2.0 * a, Sp(4.0));
        assert_eq!(a / 2.0, Sp(1.0));
        assert_eq!(a / b, 4.0);
        assert_eq!(-a, Sp(-2.0));
        assert_eq!([a, b].into_iter().sum::<Sp>(), Sp(2.5));
        assert_eq!(a.min(b), b);
        assert_eq!(a.max(b), a);
        assert_eq!(Sp(-1.5).abs(), Sp(1.5));
        assert_eq!(Sp(9.0).clamp(Sp(0.0), Sp(3.0)), Sp(3.0));
    }

    #[test]
    fn rect_signed_distance() {
        let r = SpRect::xywh(0.0, 0.0, 2.0, 1.0);
        // Center: 0.5 from top/bottom edges, 1.0 from sides -> -0.5.
        assert_eq!(r.signed_distance(SpPoint::xy(1.0, 0.5)), Sp(-0.5));
        // On an edge.
        assert_eq!(r.signed_distance(SpPoint::xy(0.0, 0.5)), Sp(0.0));
        // Outside along one axis.
        assert_eq!(r.signed_distance(SpPoint::xy(3.0, 0.5)), Sp(1.0));
        // Outside along both axes: corner distance.
        let d = r.signed_distance(SpPoint::xy(3.0, -1.0)).0;
        assert!((d - 2.0f64.sqrt()).abs() < 1e-12);
    }
}
