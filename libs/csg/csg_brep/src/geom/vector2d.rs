//! Two-dimensional vectors and points ([`Vec2`] / [`Point2`]), usually surface
//! parameters `(u, v)`: arithmetic, lengths, distances, unitizing, rotation and angles.
//!
//! Upstream: `source/SMLib/inc/SmVector2d.h`, `source/SMLib/src/SmVector2d.cpp`.
//!
//! | Rust                                   | Upstream                                         |
//! |----------------------------------------|--------------------------------------------------|
//! | [`Vec2`]                               | `SmVector2d`                                     |
//! | [`Point2`]                             | `SmPoint2d` (same class upstream)                |
//! | [`Vec2::new`]                          | `SmVector2d(double, double)`, `Set`              |
//! | [`Vec2::from_array`], `From<[f64; 2]>` | `SmVector2d(double *)`                           |
//! | `Copy` / assignment                    | copy constructor, `operator=(const SmVector2d&)`, `Copy` |
//! | [`Vec2::from_xy`]                      | `operator=(const SmVector3d&)`                   |
//! | [`Vec2::UNDEF`], [`Vec2::set_uninitialized`] | `SmVector2d()` (default ctor), `SetUninitialized` |
//! | [`Vec2::is_initialized`]               | `IsInitialized`                                  |
//! | [`Vec2::is_zero`]                      | `IsZero`                                         |
//! | [`Vec2::length_squared`]               | `LengthSquared`                                  |
//! | [`Vec2::length`]                       | `Length`                                         |
//! | [`Vec2::dot`]                          | `Dot`                                            |
//! | `Neg`, `Add`, `Sub`                    | `operator-(v)`, `operator+`, `operator-`         |
//! | `Mul<f64>`, `f64 * Vec2`               | `operator*(v, d)`, `operator*(d, v)`             |
//! | `Div<f64>`                             | `operator/(v, d)`                                |
//! | `AddAssign`, `SubAssign`, `MulAssign`  | `operator+=`, `operator-=`, `operator*=`         |
//! | [`Vec2::unitize`]                      | `Unitize`                                        |
//! | [`Vec2::distance`]                     | `DistanceBetween`                                |
//! | [`Vec2::distance_squared`]             | `DistanceBetweenSquared`                         |
//! | [`Vec2::closer_than`]                  | `CloserThan`                                     |
//! | [`Vec2::max_dimension`]                | `GetMaxDimension`                                |
//! | [`Vec2::swap_xy`]                      | `SwapXY`                                         |
//! | [`Vec2::rotate`]                       | `Rotate`                                         |
//! | [`Vec2::approx_eq`]                    | `operator==`                                     |
//! | [`Vec2::approx_ne`]                    | `operator!=`                                     |
//! | [`Vec2::component`]                    | `operator[](ULONG) const`                        |
//! | [`Vec2::component_mut`]                | `operator[](ULONG)`                              |
//! | [`Vec2::angle_between`]                | `AngleBetween`                                   |
//! | [`Vec2::ccw_angle_between`]            | `CCWAngleBetween`                                |
//!
//! Deviations:
//! - Out-params and `SmStatus` become return values: `unitize`, `angle_between` and
//!   `ccw_angle_between` return `Result`; `SM_ERR_INVALID_INPUT` is
//!   [`SolidError::InvalidInput`]. `unitize` returns the new vector instead of
//!   updating `self`; on error upstream leaves the vector unchanged.
//! - Upstream's tolerant `operator==` / `operator!=` are [`Vec2::approx_eq`] /
//!   [`Vec2::approx_ne`]; Rust `==` (`PartialEq`) is exact component equality.
//! - `Default` is the zero vector; upstream's default constructor gives
//!   [`Vec2::UNDEF`] (both components `SM_UNDEF_DOUBLE`).
//! - `operator[]` with an index other than 0 or 1 returns `0.0` (const) or
//!   dereferences `NULL` (non-const) after logging `SM_ERR`; Rust returns
//!   `SolidError::Upstream { code: 1001 }` (`SM_ERR`) for both.
//! - `SM_ASSERT_BREAK` checks are compiled out upstream (release build) and omitted.
//! - `sm_sqrt`, `sm_acos`, `sm_asin` (`SmMath.cpp`) come from [`crate::geom::vector3d`];
//!   `SM_IS_ZERO_SQUARED` = `SmTol::InTol(a, SmTol::GetScaledZeroSq())` calls
//!   [`crate::geom::tol`].

use core::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

use crate::error::{Result, SolidError};
use crate::geom::tol;
use crate::geom::vector3d::{acos as sm_acos, asin as sm_asin, sqrt as sm_sqrt};
use crate::math::{Vec3, EFF_ZERO, EFF_ZERO_SQ, PI, UNDEF_DOUBLE};

/// Upstream `SM_ERR` (generic error status).
const SM_ERR: i32 = 1001;

/// A 2D vector (`SmVector2d`), typically a surface parameter `(u, v)`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    /// X (or `u`) component.
    pub x: f64,
    /// Y (or `v`) component.
    pub y: f64,
}

/// A 2D point (`SmPoint2d`, an alias of `SmVector2d` upstream).
pub type Point2 = Vec2;

impl Vec2 {
    /// The zero vector.
    pub const ZERO: Self = Self::new(0.0, 0.0);

    /// The uninitialized vector: both components `SM_UNDEF_DOUBLE` (upstream's
    /// default constructor).
    pub const UNDEF: Self = Self::new(UNDEF_DOUBLE, UNDEF_DOUBLE);

    /// Creates a vector from its components (`SmVector2d(dX, dY)`, `Set`).
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Creates a vector from `[x, y]` (`SmVector2d(double *)`).
    #[must_use]
    pub const fn from_array(a: [f64; 2]) -> Self {
        Self::new(a[0], a[1])
    }

    /// Components as an array `[x, y]`.
    #[must_use]
    pub const fn to_array(self) -> [f64; 2] {
        [self.x, self.y]
    }

    /// The `x` and `y` of a 3D vector, dropping `z`
    /// (`operator=(const SmVector3d&)`).
    #[must_use]
    pub const fn from_xy(v: Vec3) -> Self {
        Self::new(v.x, v.y)
    }

    /// Sets both components to `SM_UNDEF_DOUBLE` (`SetUninitialized`).
    pub fn set_uninitialized(&mut self) {
        *self = Self::UNDEF;
    }

    /// True when neither component is `SM_UNDEF_DOUBLE` (`IsInitialized`).
    #[must_use]
    pub fn is_initialized(self) -> bool {
        self.x != UNDEF_DOUBLE && self.y != UNDEF_DOUBLE
    }

    /// True when the length is below `tol` (`IsZero`; upstream default
    /// `tol` is [`EFF_ZERO`]).
    #[must_use]
    pub fn is_zero(self, tol: f64) -> bool {
        self.length_squared() < tol * tol
    }

    /// Squared Euclidean length, `(x*x) + (y*y)` (`LengthSquared`).
    #[must_use]
    pub fn length_squared(self) -> f64 {
        (self.x * self.x) + (self.y * self.y)
    }

    /// Euclidean length (`Length`).
    #[must_use]
    pub fn length(self) -> f64 {
        sm_sqrt((self.x * self.x) + (self.y * self.y))
    }

    /// Dot product, `(x*o.x) + (y*o.y)` (`Dot`).
    #[must_use]
    pub fn dot(self, o: Self) -> f64 {
        (self.x * o.x) + (self.y * o.y)
    }

    /// Unit vector in the same direction (`Unitize`).
    ///
    /// A vector whose squared length is within [`EFF_ZERO_SQ`] of 1 is returned
    /// unchanged; one with squared length below [`EFF_ZERO_SQ`] is an error.
    pub fn unitize(self) -> Result<Self> {
        let len_sq = self.length_squared();
        if len_sq < EFF_ZERO_SQ {
            return Err(SolidError::InvalidInput {
                operation: "geom::vector2d::Vec2::unitize",
                reason: "zero-length vector",
            });
        }
        if (len_sq - 1.0).abs() > EFF_ZERO_SQ {
            let len = sm_sqrt(len_sq);
            return Ok(Self::new(self.x / len, self.y / len));
        }
        Ok(self)
    }

    /// Distance between two points (`DistanceBetween`).
    #[must_use]
    pub fn distance(self, o: Self) -> f64 {
        (self - o).length()
    }

    /// Squared distance between two points (`DistanceBetweenSquared`).
    #[must_use]
    pub fn distance_squared(self, o: Self) -> f64 {
        (self - o).length_squared()
    }

    /// True when the distance to `o` is at most `max` (`CloserThan`).
    ///
    /// Rejects early when either coordinate difference alone exceeds `max`.
    #[must_use]
    pub fn closer_than(self, max: f64, o: Self) -> bool {
        !((self.x - o.x).abs() > max
            || (self.y - o.y).abs() > max
            || ((self.x - o.x) * (self.x - o.x) + (self.y - o.y) * (self.y - o.y)) > max * max)
    }

    /// Largest absolute coordinate (`GetMaxDimension`).
    #[must_use]
    pub fn max_dimension(self) -> f64 {
        if self.x.abs() > self.y.abs() {
            self.x.abs()
        } else {
            self.y.abs()
        }
    }

    /// The vector with `x` and `y` exchanged (`SwapXY`).
    #[must_use]
    pub const fn swap_xy(self) -> Self {
        Self::new(self.y, self.x)
    }

    /// The vector rotated counter-clockwise by `ang_rad` about the Z axis (`Rotate`).
    #[must_use]
    pub fn rotate(self, ang_rad: f64) -> Self {
        let cos = ang_rad.cos();
        let sin = ang_rad.sin();
        Self::new(cos * self.x - sin * self.y, sin * self.x + cos * self.y)
    }

    /// Upstream's tolerant equality (`operator==`): the L1 distance is below
    /// `EFF_ZERO * (1 + self.max_dimension())`. Not symmetric in general.
    #[must_use]
    pub fn approx_eq(self, o: Self) -> bool {
        let scale = 1.0 + self.max_dimension();
        let dist = (self.x - o.x).abs() + (self.y - o.y).abs();
        dist < EFF_ZERO * scale
    }

    /// Upstream's tolerant inequality (`operator!=`): the L1 distance is at least
    /// `EFF_ZERO * (1 + self.max_dimension())`. Both this and [`Vec2::approx_eq`]
    /// are false when the distance is NaN.
    #[must_use]
    pub fn approx_ne(self, o: Self) -> bool {
        let scale = 1.0 + self.max_dimension();
        let dist = (self.x - o.x).abs() + (self.y - o.y).abs();
        dist >= EFF_ZERO * scale
    }

    /// Component `0` (`x`) or `1` (`y`) (`operator[] const`).
    pub fn component(self, index: usize) -> Result<f64> {
        match index {
            0 => Ok(self.x),
            1 => Ok(self.y),
            _ => Err(Self::bad_index()),
        }
    }

    /// Mutable component `0` (`x`) or `1` (`y`) (`operator[]`).
    pub fn component_mut(&mut self, index: usize) -> Result<&mut f64> {
        match index {
            0 => Ok(&mut self.x),
            1 => Ok(&mut self.y),
            _ => Err(Self::bad_index()),
        }
    }

    fn bad_index() -> SolidError {
        SolidError::Upstream {
            operation: "geom::vector2d::Vec2::component",
            code: SM_ERR,
            entity: None,
        }
    }

    /// Angle between two vectors in radians, in `[0, PI]` (`AngleBetween`).
    ///
    /// Uses `acos` of the normalised dot product when its square is below `0.81`,
    /// else `asin` of the normalised cross product (more accurate near `0` and `PI`).
    /// Fails when either vector has squared length within `EFF_ZERO_SQ` of zero.
    pub fn angle_between(self, o: Self) -> Result<f64> {
        let v1_len_sq = self.length_squared();
        let v2_len_sq = o.length_squared();

        if tol::is_zero_squared(v1_len_sq) || tol::is_zero_squared(v2_len_sq) {
            return Err(SolidError::InvalidInput {
                operation: "geom::vector2d::Vec2::angle_between",
                reason: "zero-length vector",
            });
        }
        let dot_prep = self.dot(o);
        let dot_sign = if dot_prep < 0.0 { -1.0 } else { 1.0 };
        let dot_squared = (dot_prep * dot_prep) / v1_len_sq / v2_len_sq;
        let dot = dot_sign * sm_sqrt(dot_squared);

        if dot_squared < 0.81 {
            Ok(sm_acos(dot))
        } else {
            let cross_prep = self.x * o.y - self.y * o.x;
            let cross = sm_sqrt((cross_prep * cross_prep) / v1_len_sq / v2_len_sq);
            Ok(if dot < 0.0 {
                PI - sm_asin(cross)
            } else {
                sm_asin(cross)
            })
        }
    }

    /// Counter-clockwise angle from `self` to `o` in radians, in `[0, 2*PI)`
    /// (`CCWAngleBetween`).
    ///
    /// Pairs whose cross product is in `[-2 * scaled_zero, 0)` snap to `0` or `PI`;
    /// tangent vectors give `0`, never `2*PI`.
    pub fn ccw_angle_between(self, o: Self) -> Result<f64> {
        let mut ang = self.angle_between(o)?;
        let a = o.max_dimension();
        let b = self.max_dimension();
        let scaled_zero = EFF_ZERO * (1.0 + if a > b { a } else { b });
        let cross = self.x * o.y - self.y * o.x;
        if cross < -2.0 * scaled_zero {
            ang = 2.0 * PI - ang;
        } else if cross < 0.0 {
            ang = if ang.abs() < 0.1 { 0.0 } else { PI };
        }
        Ok(ang)
    }
}

impl From<[f64; 2]> for Vec2 {
    fn from(a: [f64; 2]) -> Self {
        Self::from_array(a)
    }
}

impl Neg for Vec2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;
    fn mul(self, d: f64) -> Self {
        Self::new(self.x * d, self.y * d)
    }
}

impl Mul<Vec2> for f64 {
    type Output = Vec2;
    fn mul(self, v: Vec2) -> Vec2 {
        Vec2::new(v.x * self, v.y * self)
    }
}

/// `operator/(v, d)`: returns `v` unchanged when `|d|` is negligible relative to
/// either component (`|d| < |c| * EFF_ZERO`), instead of dividing.
impl Div<f64> for Vec2 {
    type Output = Self;
    fn div(self, d: f64) -> Self {
        if d.abs() < self.x.abs() * EFF_ZERO {
            return self;
        }
        if d.abs() < self.y.abs() * EFF_ZERO {
            return self;
        }
        Self::new(self.x / d, self.y / d)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, o: Self) {
        self.x += o.x;
        self.y += o.y;
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, o: Self) {
        self.x -= o.x;
        self.y -= o.y;
    }
}

impl MulAssign<f64> for Vec2 {
    fn mul_assign(&mut self, d: f64) {
        self.x *= d;
        self.y *= d;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= EFF_ZERO
    }

    /// `my_test_vector2d` (`tests/prog_test/src/smCurveTest.cpp:1758`), owned parts.
    #[test]
    fn upstream_my_test_vector2d() {
        let mut v2 = Vec2::new(0.5, 1.2);
        assert!(v2.is_initialized());
        v2.set_uninitialized();
        assert!(!v2.is_initialized());

        let mut v3 = Vec2::from([1.0, 2.0]);
        assert!(close(v3.length_squared(), 5.0));
        let mut v4 = Vec2::UNDEF;
        assert!(!v4.is_initialized());
        v4 = Vec2::new(3.0, 4.0);
        assert!(close(v4.length(), 5.0));
        let v5 = v4;
        v4 = -v4;
        let mut v6 = v5;
        v6 += v4;
        assert_eq!(v6, Vec2::ZERO);

        v3 = v3.unitize().unwrap();
        assert!(close(v3.length(), 1.0));
        assert!(close(v3.dot(v3), 1.0));

        let v8 = Vec2::new(5.0, 1.0);
        let v9 = Vec2::new(1.0, 4.0);
        assert!(close(v8.distance(v9), 5.0));
        assert!(close(v8.angle_between(v9).unwrap(), 1.1284221038181517));
        assert!(close(v9.ccw_angle_between(v8).unwrap(), 5.1547632033614343));
        assert!(close(v8.distance_squared(v4), 89.0));
        assert!(!v8.closer_than(1.0, v9));
        assert!(close(v8.max_dimension(), 5.0));

        let v10 = -v8;
        assert_eq!(v10 + v8, Vec2::ZERO);
        assert_eq!(v8 - v8, Vec2::ZERO);
        let mut v11 = v8 * 2.0;
        assert_eq!(v11, Vec2::new(10.0, 2.0));
        v11 *= 3.0;
        assert_eq!(v11, Vec2::new(30.0, 6.0));
        assert_eq!(v8 / 2.0, Vec2::new(2.5, 0.5));
        assert_eq!(2.0 * v8, v8 * 2.0);
        assert_eq!(v8.component(0).unwrap(), 5.0);
        *v11.component_mut(1).unwrap() = 6.0;
        assert_eq!(v11.y, 6.0);

        let r = v5.rotate(PI / 2.0);
        assert!(close(r.x, -4.0) && close(r.y, 3.0));
    }

    #[test]
    fn unitize_edge_cases() {
        assert!(matches!(
            Vec2::ZERO.unitize(),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(Vec2::new(1e-13, 0.0).unitize().is_err());
        // Nearly-unit vectors are returned bit for bit.
        let n = Vec2::new(0.6, 0.8);
        assert_eq!(n.unitize().unwrap(), n);
        assert_eq!(
            Vec2::new(0.0, -7.0).unitize().unwrap(),
            Vec2::new(0.0, -1.0)
        );
    }

    #[test]
    fn angles() {
        let x = Vec2::new(1.0, 0.0);
        let y = Vec2::new(0.0, 2.0);
        assert_eq!(x.angle_between(x).unwrap(), 0.0);
        assert_eq!(x.angle_between(-x).unwrap(), PI);
        assert!(close(x.angle_between(y).unwrap(), PI / 2.0));
        assert!(close(x.ccw_angle_between(y).unwrap(), PI / 2.0));
        assert!(close(y.ccw_angle_between(x).unwrap(), 1.5 * PI));
        assert_eq!(x.ccw_angle_between(x).unwrap(), 0.0);
        // Tiny negative cross product snaps to 0 or PI.
        assert_eq!(x.ccw_angle_between(Vec2::new(1.0, -1e-13)).unwrap(), 0.0);
        assert_eq!(x.ccw_angle_between(Vec2::new(-1.0, -1e-13)).unwrap(), PI);
        assert!(x.angle_between(Vec2::ZERO).is_err());
        assert!(Vec2::ZERO.ccw_angle_between(x).is_err());
    }

    #[test]
    fn division_guard_and_comparisons() {
        let v = Vec2::new(1.0, 2.0);
        assert_eq!(v / 1e-13, v);
        assert_eq!(v / 2.0, Vec2::new(0.5, 1.0));
        assert!((Vec2::ZERO / 0.0).x.is_nan());
        assert!(v.approx_eq(Vec2::new(1.0 + 1e-13, 2.0)));
        assert!(!v.approx_ne(Vec2::new(1.0 + 1e-13, 2.0)));
        assert!(v.approx_ne(Vec2::new(1.0 + 1e-10, 2.0)));
        let nan = Vec2::new(f64::NAN, 0.0);
        assert!(!v.approx_eq(nan) && !v.approx_ne(nan));
        assert!(v.closer_than(0.0, v));
        assert!(!v.closer_than(-1.0, v));
    }

    #[test]
    fn misc() {
        let mut v = Vec2::new(1.0, 2.0);
        assert_eq!(v.swap_xy(), Vec2::new(2.0, 1.0));
        assert!(v.component(2).is_err());
        assert!(v.component_mut(5).is_err());
        v -= Vec2::new(1.0, 1.0);
        assert_eq!(v, Vec2::new(0.0, 1.0));
        assert!(v.is_zero(1.5));
        assert!(!v.is_zero(EFF_ZERO));
        assert_eq!(Vec2::from_xy(Vec3::new(1.0, 2.0, 3.0)), Vec2::new(1.0, 2.0));
        assert_eq!(Vec2::new(-3.0, 3.0).max_dimension(), 3.0);
        assert_eq!(sm_sqrt(-1.0), 0.0);
        assert_eq!(sm_sqrt(-0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(sm_acos(1.5), 0.0);
        assert_eq!(sm_asin(-1.5), -PI / 2.0);
        assert!(Vec2::new(f64::NAN, 0.0).length().is_nan());
    }
}
