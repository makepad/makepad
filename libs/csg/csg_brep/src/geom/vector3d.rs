//! Three-dimensional vectors and points ([`Vec3`] / [`Point3`]): arithmetic, lengths,
//! angles, projections, rotation, orthonormal frames and derivatives of unit vectors,
//! plus the guarded scalar math helpers of `SmMath.cpp`.
//!
//! Upstream: `source/SMLib/inc/SmVector3d.h`, `source/SMLib/src/SmVector3d.cpp`,
//! `source/SMLib/src/SmMath.cpp`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`Vec3`]                                     | `SmVector3d`                                     |
//! | [`Point3`]                                   | `SmPoint3d` (same class upstream)                |
//! | [`Vec3::new`]                                | `SmVector3d(double, double, double)`, `Set`      |
//! | [`Vec3::from_array`], `From<[f64; 3]>`       | `SmVector3d(double *)`                           |
//! | [`Vec3::from_nl`]                            | `SmVector3d(double *, bool fromNL)`              |
//! | [`Vec3::between`]                            | `SmVector3d(const SmPoint3d&, const SmPoint3d&)` |
//! | `From<Vec2>`                                 | `SmVector3d(const SmVector2d&)`                  |
//! | `Copy` / assignment, `core::mem::swap`       | copy constructor, `operator=`, `Copy`, `Swap`    |
//! | [`Vec3::UNDEF`], [`Vec3::set_uninitialized`] | `SmVector3d()` (default ctor), `SetUninitialized` |
//! | [`Vec3::is_initialized`]                     | `IsInitialized`                                  |
//! | [`Vec3::is_undef`]                           | `IsUndef`                                        |
//! | [`Vec3::is_zero`]                            | `IsZero`                                         |
//! | [`Vec3::is_2d`]                              | `Is2d`                                           |
//! | [`Vec3::length_squared`], [`Vec3::length`]   | `LengthSquared`, `Length`                        |
//! | [`Vec3::dot`]                                | `Dot`                                            |
//! | [`Vec3::cross`]                              | `operator*(const SmVector3d&, const SmVector3d&)` |
//! | `Neg`, `Add`, `Sub`                          | `operator-(v)`, `operator+`, `operator-`         |
//! | `Mul<f64>`, `f64 * Vec3`, `Div<f64>`         | `operator*(v, d)`, `operator*(d, v)`, `operator/` |
//! | `AddAssign`, `SubAssign`, `MulAssign`, `DivAssign` | `operator+=`, `operator-=`, `operator*=`, `operator/=` |
//! | [`Vec3::unitize`], [`Vec3::unitize_or_z`]    | `Unitize`                                        |
//! | [`Vec3::triple_product`]                     | `TripleProduct`                                  |
//! | [`Vec3::normal_in_plane`]                    | `NormalInPlane`                                  |
//! | [`Vec3::max_dimension`], [`Vec3::min_dimension`] | `GetMaxDimension`, `GetMinDimension`         |
//! | [`Vec3::is_parallel_to`], [`Vec3::parallel_sense`] | `IsParallelTo`                             |
//! | [`Vec3::is_perpendicular_to`]                | `IsPerpendicularTo`                              |
//! | [`Vec3::project_to_plane`]                   | `ProjectToPlane`                                 |
//! | [`Vec3::project_point_to_plane`]             | `ProjectPointToPlane`                            |
//! | [`Vec3::rotate_project_to_plane`]            | `RotateProjectToPlane`                           |
//! | [`Vec3::rotate_project_point_to_plane`]      | `RotateProjectPointToPlane`                      |
//! | [`Vec3::perspective_project_to_plane`]       | `PerspectiveProjectToPlane`                      |
//! | [`Vec3::perspective_project_point_to_plane`] | `PerspectiveProjectPointToPlane`                 |
//! | [`Vec3::distance`], [`Vec3::distance_squared`] | `DistanceBetween`, `DistanceBetweenSquared`    |
//! | [`Vec3::closer_than`]                        | `CloserThan`                                     |
//! | [`Vec3::multiply`]                           | `Multiply`                                       |
//! | [`Vec3::derivative_of_cross_product`]        | `DerivativeOfCrossProduct`                       |
//! | [`Vec3::solve_two_linear_equations`]         | `SolveTwoLinearEquations`                        |
//! | [`Vec3::component`], [`Vec3::component_mut`] | `operator[] const`, `operator[]`                 |
//! | [`Vec3::approx_eq`]                          | `operator==`                                     |
//! | [`Vec3::is_inside_sector`]                   | `IsInsideSector`                                 |
//! | [`Vec3::make_unit_ortho_vectors`], [`Vec3::unit_ortho_vectors_on_failure`] | `MakeUnitOrthoVectors` |
//! | [`Vec3::rotate_vec_about_axis`]              | `RotateVecAboutAxis`                             |
//! | [`Vec3::unitized_derivative`]                | `UnitizedDerivative`                             |
//! | [`Vec3::unitized_derivative2`]               | `UnitizedDerivative2`                            |
//! | [`Vec3::unitized_derivative3`]               | `UnitizedDerivative3`                            |
//! | [`Vec3::angle_between`]                      | `AngleBetween`                                   |
//! | [`Vec3::ccw_angle_between`]                  | `CCWAngleBetween`                                |
//! | [`Vec3::is_colinear_with`]                   | `IsColinearWith`                                 |
//! | [`sqrt`], [`cbrt`], [`acos`], [`asin`]       | `sm_sqrt`, `sm_cbrt`, `sm_acos`, `sm_asin` (`smos_Sqrt`, `smos_CubeRoot`, `smos_ArcCosine`, `smos_ArcSine`) |
//!
//! Deviations:
//! - Out-params and `SmStatus` become return values. `SM_ERR_INVALID_INPUT` is
//!   [`SolidError::InvalidInput`]; `SM_ERR` is [`SolidError::Upstream`] with code 1001.
//! - `Unitize` updates the vector in place and, for a zero vector, stores `(0, 0, 1)`
//!   and returns an error. [`Vec3::unitize`] returns the unit vector or the error;
//!   [`Vec3::unitize_or_z`] returns upstream's in-place result whatever the status
//!   (what callers that ignore the status see).
//! - `UnitizedDerivative2`/`3` return `SM_ERR` *and* the input derivatives for a
//!   degenerate `self`; the Rust versions return `Err(inputs)` so callers that ignore
//!   the status (`.unwrap_or_else(|d| d)`) see the same values.
//! - `IsParallelTo`'s optional anti-parallel out-flag is [`Vec3::parallel_sense`].
//! - Upstream's tolerant `operator==` is [`Vec3::approx_eq`]; Rust `==`
//!   (`PartialEq`) is exact component equality.
//! - `Default` is the zero vector; upstream's default constructor gives
//!   [`Vec3::UNDEF`] (all components `SM_UNDEF_DOUBLE`).
//! - `operator[] const` with an index above 2 returns `0.0` (its `SE(SM_ERR)` is a no-op
//!   in the release build), as [`Vec3::component`] does. The non-const `operator[]`
//!   dereferences `NULL`; [`Vec3::component_mut`] returns `SM_ERR` instead.
//! - `operator/=` keeps the vector unchanged when `|d|` is negligible against every
//!   component; `operator/` always divides. `DivAssign` and `Div` differ the same way.
//! - Statuses ignored upstream through `SE(...)` (a no-op in the release build) are
//!   ignored here too; the one whose output would be uninitialised (`IsInsideSector`'s
//!   `CCWAngleBetween`) cannot fail, see the comment there.
//! - `SM_ASSERT_BREAK` checks and `sm_*`'s `ERR(SM_ERR)` logging are compiled out
//!   upstream (release build) and omitted.

use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

use crate::error::{Result, SolidError};
use crate::geom::tol;
use crate::geom::vector2d::{Point2, Vec2};
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, PI, UNDEF_DOUBLE};
use crate::nurbs::math::{NOW, NOZ};

/// Upstream `SM_ERR` (generic error status).
const SM_ERR: i32 = 1001;

// ---- SmMath.cpp ----------------------------------------------------------------------

/// `sm_sqrt` (`smos_Sqrt`): square root that maps every non-positive input to `0.0`
/// (upstream also flags an error below `-SM_EFF_ZERO_SQ`).
#[must_use]
pub fn sqrt(x: f64) -> f64 {
    if x <= 0.0 {
        0.0
    } else {
        x.sqrt()
    }
}

/// `sm_cbrt` (`smos_CubeRoot`): cube root.
#[must_use]
pub fn cbrt(x: f64) -> f64 {
    x.cbrt()
}

/// `sm_acos` (`smos_ArcCosine`): arc cosine clamped to `0` / `PI` for arguments
/// outside `(-1, 1)` (upstream also flags an error beyond `1 + SM_EFF_ZERO`).
#[must_use]
pub fn acos(x: f64) -> f64 {
    if x >= 1.0 {
        return 0.0;
    }
    if x <= -1.0 {
        return PI;
    }
    x.acos()
}

/// `sm_asin` (`smos_ArcSine`): arc sine clamped to `±PI/2` for arguments outside
/// `(-1, 1)` (upstream also flags an error beyond `1 + SM_EFF_ZERO`).
#[must_use]
pub fn asin(x: f64) -> f64 {
    if x >= 1.0 {
        return PI / 2.0;
    }
    if x <= -1.0 {
        return -PI / 2.0;
    }
    x.asin()
}

/// `SM_DEG2RAD(a)` = `a * (SM_PI / 180.0)`.
fn deg_to_rad(a: f64) -> f64 {
    a * (PI / 180.0)
}

/// `SM_IS_VALID_DOUBLE(a)`: not NaN, finite and within `±SM_BIG_DOUBLE / 100`.
pub(crate) fn is_valid_double(a: f64) -> bool {
    // `(a) == (a)` is `!a.is_nan()`.
    !a.is_nan()
        && (-f64::MAX..=f64::MAX).contains(&a)
        && ((-BIG_DOUBLE / 100.0)..=(BIG_DOUBLE / 100.0)).contains(&a)
}

fn sm_err(operation: &'static str) -> SolidError {
    SolidError::Upstream {
        operation,
        code: SM_ERR,
        entity: None,
    }
}

// ---- Vec3 ----------------------------------------------------------------------------

/// A 3D vector (`SmVector3d`). Also used for points, see [`Point3`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

/// A 3D point. Upstream uses one type for points and vectors
/// (`#define SmPoint3d SmVector3d`), and so does the port.
pub type Point3 = Vec3;

impl Vec3 {
    /// The zero vector.
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    /// Unit X axis.
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    /// Unit Y axis.
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    /// Unit Z axis.
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);
    /// The uninitialized vector: all components `SM_UNDEF_DOUBLE` (upstream's default
    /// constructor).
    pub const UNDEF: Self = Self::new(UNDEF_DOUBLE, UNDEF_DOUBLE, UNDEF_DOUBLE);

    /// Creates a vector from its components (`SmVector3d(dX, dY, dZ)`, `Set`).
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Creates a vector from `[x, y, z]` (`SmVector3d(double *)`).
    #[must_use]
    pub const fn from_array(a: [f64; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }

    /// Converts an NLib control point `[x, y, z, w]` (`SmVector3d(double *, bool)`):
    /// `z == NL_NOZ` becomes `0`, and unless `w == NL_NOW` the point is divided by `w`.
    #[must_use]
    pub fn from_nl(p: [f64; 4]) -> Self {
        let [px, py, pz, pw] = p;
        let mut v = Self::new(px, py, if pz == NOZ { 0.0 } else { pz });
        if NOW != pw {
            v.x /= pw;
            v.y /= pw;
            v.z /= pw;
        }
        v
    }

    /// The vector from `start` to `end`, `end - start`
    /// (`SmVector3d(const SmPoint3d&, const SmPoint3d&)`).
    #[must_use]
    pub fn between(start: Point3, end: Point3) -> Self {
        Self::new(end.x - start.x, end.y - start.y, end.z - start.z)
    }

    /// Components as an array `[x, y, z]`.
    #[must_use]
    pub const fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// Sets all components to `SM_UNDEF_DOUBLE` (`SetUninitialized`).
    pub fn set_uninitialized(&mut self) {
        *self = Self::UNDEF;
    }

    /// True when no component is `SM_UNDEF_DOUBLE` (`IsInitialized`).
    #[must_use]
    pub fn is_initialized(self) -> bool {
        self.x != UNDEF_DOUBLE && self.y != UNDEF_DOUBLE && self.z != UNDEF_DOUBLE
    }

    /// True when any component is `SM_UNDEF_DOUBLE`, NaN, infinite or beyond
    /// `±SM_BIG_DOUBLE / 100` (`IsUndef`).
    #[must_use]
    pub fn is_undef(self) -> bool {
        self.x == UNDEF_DOUBLE
            || !is_valid_double(self.x)
            || self.y == UNDEF_DOUBLE
            || !is_valid_double(self.y)
            || self.z == UNDEF_DOUBLE
            || !is_valid_double(self.z)
    }

    /// True when the length is below `tol` (`IsZero`; upstream default `tol` is
    /// [`EFF_ZERO`]).
    #[must_use]
    pub fn is_zero(self, tol: f64) -> bool {
        self.length_squared() < tol * tol
    }

    /// True when `z` is NLib's 2D marker `NL_NOZ` (`Is2d`).
    #[must_use]
    pub fn is_2d(self) -> bool {
        self.z == NOZ
    }

    /// Squared Euclidean length, `(x*x) + (y*y) + (z*z)` (`LengthSquared`).
    #[must_use]
    pub fn length_squared(self) -> f64 {
        (self.x * self.x) + (self.y * self.y) + (self.z * self.z)
    }

    /// Euclidean length (`Length`).
    #[must_use]
    pub fn length(self) -> f64 {
        sqrt(self.length_squared())
    }

    /// Dot product, `(x*o.x) + (y*o.y) + (z*o.z)` (`Dot`).
    #[must_use]
    pub fn dot(self, o: Self) -> f64 {
        (self.x * o.x) + (self.y * o.y) + (self.z * o.z)
    }

    /// Cross product `self × o` (upstream `operator*` on two vectors).
    #[must_use]
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            (self.y * o.z) - (self.z * o.y),
            (self.z * o.x) - (self.x * o.z),
            (self.x * o.y) - (self.y * o.x),
        )
    }

    /// Unit vector in the same direction (`Unitize`).
    ///
    /// A vector whose squared length is within [`EFF_ZERO_SQ`] of 1 is returned
    /// unchanged; one with squared length below [`EFF_ZERO_SQ`] is an error
    /// (upstream then stores `(0, 0, 1)`, see [`Vec3::unitize_or_z`]).
    pub fn unitize(self) -> Result<Self> {
        let len_sq = self.length_squared();
        if len_sq < EFF_ZERO_SQ {
            return Err(SolidError::InvalidInput {
                operation: "geom::vector3d::Vec3::unitize",
                reason: "zero-length vector",
            });
        }
        Ok(self.unitize_nonzero(len_sq))
    }

    /// Upstream's in-place `Unitize` result with the status ignored: the unit vector,
    /// or `(0, 0, 1)` for a zero-length vector.
    #[must_use]
    pub fn unitize_or_z(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq < EFF_ZERO_SQ {
            Self::Z
        } else {
            self.unitize_nonzero(len_sq)
        }
    }

    fn unitize_nonzero(self, len_sq: f64) -> Self {
        if (len_sq - 1.0).abs() > EFF_ZERO_SQ {
            let len = sqrt(len_sq);
            Self::new(self.x / len, self.y / len, self.z / len)
        } else {
            self
        }
    }

    /// Triple product `self · (b × c)` (`TripleProduct`).
    #[must_use]
    pub fn triple_product(self, b: Self, c: Self) -> f64 {
        self.x * ((b.y * c.z) - (b.z * c.y))
            + self.y * ((b.z * c.x) - (b.x * c.z))
            + self.z * ((b.x * c.y) - (b.y * c.x))
    }

    /// `(self × v) × self`: a vector normal to `self` in the plane of `self` and `v`
    /// (`NormalInPlane`).
    #[must_use]
    pub fn normal_in_plane(self, v: Self) -> Self {
        let (x, y, z) = (self.x, self.y, self.z);
        Self::new(
            y * ((v.x * y) - (v.y * x)) - z * ((v.z * x) - (v.x * z)),
            z * ((v.y * z) - (v.z * y)) - x * ((v.x * y) - (v.y * x)),
            x * ((v.z * x) - (v.x * z)) - y * ((v.y * z) - (v.z * y)),
        )
    }

    /// Largest absolute coordinate (`GetMaxDimension`).
    #[must_use]
    pub fn max_dimension(self) -> f64 {
        let mut d = if self.x.abs() > self.y.abs() {
            self.x.abs()
        } else {
            self.y.abs()
        };
        if self.z.abs() > d {
            d = self.z.abs();
        }
        d
    }

    /// Smallest absolute coordinate (`GetMinDimension`).
    #[must_use]
    pub fn min_dimension(self) -> f64 {
        let mut d = if self.x.abs() < self.y.abs() {
            self.x.abs()
        } else {
            self.y.abs()
        };
        if self.z.abs() < d {
            d = self.z.abs();
        }
        d
    }

    /// True when `self` and `o` are parallel or anti-parallel within `ang_tol_deg`
    /// degrees (`IsParallelTo`; upstream default [`crate::math::EFF_ZERO_DEG`]).
    /// Zero-length vectors are parallel to nothing.
    #[must_use]
    pub fn is_parallel_to(self, o: Self, ang_tol_deg: f64) -> bool {
        self.parallel_sense(o, ang_tol_deg).0
    }

    /// [`Vec3::is_parallel_to`] plus `IsParallelTo`'s anti-parallel out-flag: `Some(true)`
    /// when the angle exceeds `PI/2`, `Some(false)` otherwise, `None` (flag untouched
    /// upstream) when either vector has zero length.
    #[must_use]
    pub fn parallel_sense(self, o: Self, ang_tol_deg: f64) -> (bool, Option<bool>) {
        let mut parallel = false;
        let mut anti = None;
        if self.length_squared() > EFF_ZERO_SQ && o.length_squared() > EFF_ZERO_SQ {
            // SE(): a failed AngleBetween leaves 0.0.
            let mut ang = self.angle_between(o).unwrap_or(0.0);
            if ang > PI / 2.0 {
                ang = PI - ang;
                anti = Some(true);
            } else {
                anti = Some(false);
            }
            if ang < deg_to_rad(ang_tol_deg) {
                parallel = true;
            }
        }
        (parallel, anti)
    }

    /// True when `self` and `o` are perpendicular within `ang_tol_deg` degrees
    /// (`IsPerpendicularTo`). Zero-length vectors are perpendicular to nothing.
    #[must_use]
    pub fn is_perpendicular_to(self, o: Self, ang_tol_deg: f64) -> bool {
        if self.length_squared() > EFF_ZERO_SQ && o.length_squared() > EFF_ZERO_SQ {
            // SE(): a failed AngleBetween leaves 0.0.
            let ang = self.angle_between(o).unwrap_or(0.0);
            if (ang - PI / 2.0).abs() < deg_to_rad(ang_tol_deg) {
                return true;
            }
        }
        false
    }

    /// The vector projected along the unit `plane_normal` onto the plane
    /// (`ProjectToPlane`).
    #[must_use]
    pub fn project_to_plane(self, plane_normal: Self) -> Self {
        self - plane_normal * self.dot(plane_normal)
    }

    /// The point projected along the unit `plane_normal` onto the plane through
    /// `plane_point` (`ProjectPointToPlane`).
    #[must_use]
    pub fn project_point_to_plane(self, plane_point: Point3, plane_normal: Self) -> Point3 {
        let diff = self - plane_point;
        self - plane_normal * diff.dot(plane_normal)
    }

    /// The vector rotated about the unit `rotate_axis` into the plane spanned by
    /// `rotate_axis` and the unit `x_axis` (`RotateProjectToPlane`).
    #[must_use]
    pub fn rotate_project_to_plane(self, rotate_axis: Self, x_axis: Self) -> Self {
        rotate_axis * self.dot(rotate_axis) + x_axis * self.dot(x_axis)
    }

    /// The point rotated about the line (`rotate_point`, unit `rotate_axis`) into the
    /// plane through it spanned by `rotate_axis` and the unit `x_axis`
    /// (`RotateProjectPointToPlane`).
    #[must_use]
    pub fn rotate_project_point_to_plane(
        self,
        rotate_point: Point3,
        rotate_axis: Self,
        x_axis: Self,
    ) -> Point3 {
        let diff = self - rotate_point;
        rotate_point + rotate_axis * diff.dot(rotate_axis) + x_axis * diff.dot(x_axis)
    }

    /// The vector based at `base_point`, projected towards `eye_point` onto the
    /// viewing plane (`plane_point`, unit `plane_normal`)
    /// (`PerspectiveProjectToPlane`).
    #[must_use]
    pub fn perspective_project_to_plane(
        self,
        base_point: Point3,
        plane_point: Point3,
        plane_normal: Self,
        eye_point: Point3,
    ) -> Self {
        let eye_plane_diff = plane_point - eye_point;
        let eye_point_diff = base_point - eye_point;
        let eye_vec_diff = base_point + self - eye_point;
        eye_vec_diff * (eye_plane_diff.dot(plane_normal) / eye_vec_diff.dot(plane_normal))
            - eye_point_diff * (eye_plane_diff.dot(plane_normal) / eye_point_diff.dot(plane_normal))
    }

    /// The point projected towards `eye_point` onto the viewing plane (`plane_point`,
    /// unit `plane_normal`) (`PerspectiveProjectPointToPlane`).
    #[must_use]
    pub fn perspective_project_point_to_plane(
        self,
        plane_point: Point3,
        plane_normal: Self,
        eye_point: Point3,
    ) -> Point3 {
        let eye_plane_diff = plane_point - eye_point;
        let eye_point_diff = self - eye_point;
        eye_point
            + eye_point_diff * (eye_plane_diff.dot(plane_normal) / eye_point_diff.dot(plane_normal))
    }

    /// Distance between two points (`DistanceBetween`).
    #[must_use]
    pub fn distance(self, o: Self) -> f64 {
        sqrt(self.distance_squared(o))
    }

    /// Squared distance between two points (`DistanceBetweenSquared`).
    #[must_use]
    pub fn distance_squared(self, o: Self) -> f64 {
        (self.x - o.x) * (self.x - o.x)
            + (self.y - o.y) * (self.y - o.y)
            + (self.z - o.z) * (self.z - o.z)
    }

    /// True when the distance to `o` is at most `max` (`CloserThan`).
    ///
    /// Rejects early when any coordinate difference alone exceeds `max`.
    #[must_use]
    pub fn closer_than(self, max: f64, o: Self) -> bool {
        !((self.x - o.x).abs() > max
            || (self.y - o.y).abs() > max
            || (self.z - o.z).abs() > max
            || self.distance_squared(o) > max * max)
    }

    /// Component-wise product (`Multiply`).
    #[must_use]
    pub fn multiply(self, o: Self) -> Self {
        Self::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }

    /// Derivative of `self × other` given the derivatives of both vectors
    /// (`DerivativeOfCrossProduct`).
    #[must_use]
    pub fn derivative_of_cross_product(
        self,
        other: Self,
        self_prime: Self,
        other_prime: Self,
    ) -> Self {
        let (a, b, da, db) = (self, other, self_prime, other_prime);
        Self::new(
            da.y * b.z + a.y * db.z - (da.z * b.y + a.z * db.y),
            da.z * b.x + a.z * db.x - (da.x * b.z + a.x * db.z),
            da.x * b.y + a.x * db.y - (da.y * b.x + a.y * db.x),
        )
    }

    /// Solves `self.x*t + self.y*u = self.z`, `o.x*t + o.y*u = o.z` for `(t, u)`
    /// (`SolveTwoLinearEquations`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` ([`SolidError::Upstream`]) when the system is (numerically) singular.
    pub fn solve_two_linear_equations(self, o: Self) -> Result<Point2> {
        let (x, y, z) = (self.x, self.y, self.z);
        let denom = (o.y * x) - (o.x * y);
        if denom.abs() >= EFF_ZERO_SQ {
            let u = (o.z * x - o.x * z) / denom;
            if (x >= EFF_ZERO_SQ || x <= -EFF_ZERO_SQ) && x.abs() > o.x.abs() {
                return Ok(Point2::new((z - y * u) / x, u));
            } else if o.x.abs() >= EFF_ZERO_SQ {
                return Ok(Point2::new((o.z - o.y * u) / o.x, u));
            }
        }
        Err(sm_err("geom::vector3d::Vec3::solve_two_linear_equations"))
    }

    /// Component `0` (`x`), `1` (`y`) or `2` (`z`); `0.0` for any other index
    /// (`operator[] const`).
    #[must_use]
    pub fn component(self, index: usize) -> f64 {
        match index {
            0 => self.x,
            1 => self.y,
            2 => self.z,
            _ => 0.0,
        }
    }

    /// Mutable component `0` (`x`), `1` (`y`) or `2` (`z`) (`operator[]`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` ([`SolidError::Upstream`]) for any other index.
    pub fn component_mut(&mut self, index: usize) -> Result<&mut f64> {
        match index {
            0 => Ok(&mut self.x),
            1 => Ok(&mut self.y),
            2 => Ok(&mut self.z),
            _ => Err(sm_err("geom::vector3d::Vec3::component_mut")),
        }
    }

    /// Upstream's tolerant equality (`operator==`): two uninitialized vectors are
    /// equal, one uninitialized vector equals nothing, otherwise the L1 distance must be
    /// below `EFF_ZERO * (1 + self.max_dimension())`. Not symmetric in general.
    #[must_use]
    pub fn approx_eq(self, o: Self) -> bool {
        let init1 = self.is_initialized();
        let init2 = o.is_initialized();
        if !init1 && !init2 {
            return true;
        }
        if !init1 || !init2 {
            return false;
        }
        let scaled_zero = EFF_ZERO * (1.0 + self.max_dimension());
        let dist = (self.x - o.x).abs() + (self.y - o.y).abs() + (self.z - o.z).abs();
        dist < scaled_zero
    }

    /// True when `self` lies in the sector swept from `bound1` towards `binormal1` up
    /// to `bound2`, or on either bound (`IsInsideSector`). All vectors are assumed
    /// coplanar; three vectors in essentially the same direction count as inside.
    #[must_use]
    pub fn is_inside_sector(self, bound1: Self, binormal1: Self, bound2: Self) -> bool {
        // SE(sNorm.Unitize()): a zero normal becomes (0,0,1).
        let norm = bound1.cross(binormal1).unitize_or_z();
        // SE(...CCWAngleBetween(...)): it only fails through AngleBetween on
        // zero-length projections, which it rejects (as 0.0) before that call, so the
        // fallback is never used.
        let mut sector_angle = norm.ccw_angle_between(bound1, bound2).unwrap_or(0.0);
        let mut this_angle = norm.ccw_angle_between(bound1, self).unwrap_or(0.0);

        // CCWAngleBetween() returns [-PI,PI], we need [0,2PI]
        if sector_angle < 0.0 {
            sector_angle += 2.0 * PI;
        }
        if this_angle < -EFF_ZERO {
            this_angle += 2.0 * PI;
        }

        // Angles near 0 and near 2*PI are the same direction: call that inside.
        if (this_angle - sector_angle).abs() >= 2.0 * PI - EFF_ZERO {
            return true;
        }

        this_angle > -EFF_ZERO && this_angle < sector_angle + EFF_ZERO
    }

    /// An orthonormal frame `[x, y, z]` with `x` along `self` (`MakeUnitOrthoVectors`).
    ///
    /// `y` lies in the plane of `self` and `y_reference` (on its side) when that is
    /// given and not parallel to `self`; otherwise `y` is chosen arbitrarily
    /// (perpendicular to `x`, trying `(0,1,0)` then `(0,0,1)`). `z = x × y`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `self` has zero length; `SM_ERR`
    /// ([`SolidError::Upstream`]) if no perpendicular can be found.
    pub fn make_unit_ortho_vectors(self, y_reference: Option<Self>) -> Result<[Self; 3]> {
        let x_axis = self.unitize()?;

        if let Some(y_ref) = y_reference {
            let z_axis = x_axis.cross(y_ref);
            let len_sq = z_axis.length_squared();
            if len_sq > EFF_ZERO_SQ {
                let z_axis = z_axis.unitize_or_z();
                let y_axis = z_axis.cross(x_axis).unitize_or_z();
                return Ok([x_axis, y_axis, z_axis]);
            }
        }
        // If made it to here choose arbitrary axis
        let mut z_axis = x_axis.cross(Self::new(0.0, 1.0, 0.0));
        let len_sq = z_axis.length_squared();
        if len_sq < EFF_ZERO_SQ {
            z_axis = x_axis.cross(Self::new(0.0, 0.0, 1.0));
            if z_axis.length_squared() < EFF_ZERO_SQ {
                return Err(sm_err("geom::vector3d::Vec3::make_unit_ortho_vectors"));
            }
        }
        let z_axis = z_axis.unitize_or_z();
        let y_axis = z_axis.cross(x_axis).unitize_or_z();
        Ok([x_axis, y_axis, z_axis])
    }

    /// The out-params upstream's `MakeUnitOrthoVectors` has written when it fails
    /// (when [`Vec3::make_unit_ortho_vectors`] returns an error), as `[x, y, z]`
    /// with `None` for an out-param left unchanged: only `x` (`Unitize`'s
    /// `(0, 0, 1)`) for a zero-length `self`; otherwise the unit `x` and
    /// `z = x × (0, 0, 1)`, written before `SER(SM_ERR)` (reached when the unit
    /// vector underflows to zero because `|self|^2` overflows). Callers that pass
    /// their own members as out-params use this to keep upstream's partial state.
    #[must_use]
    pub fn unit_ortho_vectors_on_failure(self) -> [Option<Self>; 3] {
        match self.unitize() {
            Ok(x_axis) => [
                Some(x_axis),
                None,
                Some(x_axis.cross(Self::new(0.0, 0.0, 1.0))),
            ],
            Err(_) => [Some(self.unitize_or_z()), None, None],
        }
    }

    /// The vector rotated by `ang_rad` about `axis` (right-handed; `axis` need not be
    /// unit length) (`RotateVecAboutAxis`).
    #[must_use]
    pub fn rotate_vec_about_axis(self, axis: Self, ang_rad: f64) -> Self {
        let cos = ang_rad.cos();
        let sin = ang_rad.sin();

        // avoid errors - unitize the input (status ignored)
        let z = axis.unitize_or_z();

        // Get the component perp to the rotation axis
        let dot = self.dot(z);
        let mut x = self - dot * z;
        let x_len = x.length();
        if x_len < EFF_ZERO {
            return self;
        }
        x /= x_len;

        // get component perp to rotate and x axes
        let y = z.cross(x);

        x_len * cos * x + x_len * sin * y + dot * z
    }

    /// Derivative of the unit vector `self/|self|` given `d1 = self'`
    /// (`UnitizedDerivative`). For a zero-length `self`, returns `d1`.
    #[must_use]
    pub fn unitized_derivative(self, d1: Self) -> Self {
        let mag = self.length();
        if mag < EFF_ZERO {
            return d1;
        }
        let unit = self / mag;
        let mag_t = unit.dot(d1);
        let mut unit_t = d1 - mag_t * unit;
        unit_t /= mag;
        unit_t
    }

    /// First two derivatives of the unit vector `self/|self|` given `d1 = self'` and
    /// `d2 = self''` (`UnitizedDerivative2`).
    ///
    /// # Errors
    ///
    /// Upstream's `SM_ERR` for a zero-length `self`; the error carries `[d1, d2]`,
    /// the values upstream writes to its outputs in that case.
    pub fn unitized_derivative2(
        self,
        d1: Self,
        d2: Self,
    ) -> core::result::Result<[Self; 2], [Self; 2]> {
        let mag = self.length();
        if mag < EFF_ZERO {
            return Err([d1, d2]);
        }

        // 1st derivative
        let unit = self / mag;
        let mag_t = unit.dot(d1);
        let mut unit_t = d1 - mag_t * unit;
        unit_t /= mag;

        // 2nd derivative
        let mag_tt = unit_t.dot(d1) + unit.dot(d2);
        let mut unit_tt = d2 - 2.0 * mag_t * unit_t - mag_tt * unit;
        unit_tt /= mag;

        Ok([unit_t, unit_tt])
    }

    /// First three derivatives of the unit vector `self/|self|` given `d1 = self'`,
    /// `d2 = self''` and `d3 = self'''` (`UnitizedDerivative3`).
    ///
    /// # Errors
    ///
    /// Upstream's `SM_ERR` for a zero-length `self`; the error carries `[d1, d2, d3]`,
    /// the values upstream writes to its outputs in that case.
    pub fn unitized_derivative3(
        self,
        d1: Self,
        d2: Self,
        d3: Self,
    ) -> core::result::Result<[Self; 3], [Self; 3]> {
        let mag = self.length();
        if mag < EFF_ZERO {
            return Err([d1, d2, d3]);
        }

        let unit = self / mag;
        let mag_t = unit.dot(d1);
        let mut unit_t = d1 - mag_t * unit;
        unit_t /= mag;

        // 2nd deriv:
        let mag_tt = unit_t.dot(d1) + unit.dot(d2);
        let mut unit_tt = d2 - 2.0 * mag_t * unit_t - mag_tt * unit;
        unit_tt /= mag;

        // 3rd deriv:
        let mag_ttt = unit_tt.dot(d1) + 2.0 * unit_t.dot(d2) + unit.dot(d3);
        let mut unit_ttt = d3 - 3.0 * mag_t * unit_tt - 3.0 * mag_tt * unit_t - mag_ttt * unit;
        unit_ttt /= mag;

        Ok([unit_t, unit_tt, unit_ttt])
    }

    /// Angle between two vectors in radians, in `[0, PI]` (`AngleBetween`).
    ///
    /// Uses `acos` of the normalised dot product when its square is below `0.81`,
    /// else `asin` of the normalised cross product (more accurate near `0` and `PI`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when either squared length is within the scaled
    /// zero (`SM_IS_ZERO_SQUARED`); upstream then also reports an angle of `0`.
    pub fn angle_between(self, o: Self) -> Result<f64> {
        let v1_len_sq = self.length_squared();
        let v2_len_sq = o.length_squared();

        if tol::is_zero_squared(v1_len_sq) || tol::is_zero_squared(v2_len_sq) {
            return Err(SolidError::InvalidInput {
                operation: "geom::vector3d::Vec3::angle_between",
                reason: "zero-length vector",
            });
        }
        let dot_prep = self.dot(o);
        let dot_sign = if dot_prep < 0.0 { -1.0 } else { 1.0 };
        let dot_squared = (dot_prep * dot_prep) / v1_len_sq / v2_len_sq;
        let dot = dot_sign * sqrt(dot_squared);

        if dot_squared < 0.81 {
            Ok(acos(dot))
        } else {
            let crs = self.cross(o);
            let crs_norm = crs.length_squared();
            let mag = sqrt(crs_norm / v1_len_sq / v2_len_sq);
            Ok(if dot < 0.0 { PI - asin(mag) } else { asin(mag) })
        }
    }

    /// Counter-clockwise angle in radians, in `[-PI, PI]`, from `start` to `end`
    /// after projecting both into the plane normal to `self` (`CCWAngleBetween`).
    /// Returns `0` when either vector is parallel to `self`.
    ///
    /// # Errors
    ///
    /// Propagates [`Vec3::angle_between`] on the projected vectors.
    pub fn ccw_angle_between(self, start: Self, end: Self) -> Result<f64> {
        let start_proj = self.cross(start).cross(self);
        if start_proj.length_squared() < 100.0 * EFF_ZERO_SQ {
            return Ok(0.0);
        }
        let end_proj = self.cross(end).cross(self);
        if end_proj.length_squared() < 100.0 * EFF_ZERO_SQ {
            return Ok(0.0);
        }

        let mut proj_ang = start_proj.angle_between(end_proj)?;

        // negate angle's sign when Cross(start,end) opposes this vector
        if start_proj.cross(end_proj).dot(self) < 0.0 {
            proj_ang = -proj_ang;
        }
        Ok(proj_ang)
    }

    /// True when `self` and `end_vec` both lie along the segment `start_pnt`..`end_pnt`
    /// (either sense), or the segment is degenerate (`IsColinearWith`).
    #[must_use]
    pub fn is_colinear_with(self, end_vec: Self, start_pnt: Point3, end_pnt: Point3) -> bool {
        let vec = end_pnt - start_pnt;
        if vec.length_squared() < EFF_ZERO_SQ {
            return true;
        }
        let vec = vec.unitize_or_z();
        let dir1 = self.unitize_or_z();
        let dir2 = end_vec.unitize_or_z();

        1.0 - vec.dot(dir1).abs() < EFF_ZERO && 1.0 - vec.dot(dir2).abs() < EFF_ZERO
    }
}

impl From<[f64; 3]> for Vec3 {
    fn from(a: [f64; 3]) -> Self {
        Self::from_array(a)
    }
}

/// `SmVector3d(const SmVector2d&)`: `z = 0`.
impl From<Vec2> for Vec3 {
    fn from(v: Vec2) -> Self {
        Self::new(v.x, v.y, 0.0)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, d: f64) -> Self {
        Self::new(self.x * d, self.y * d, self.z * d)
    }
}

impl Mul<Vec3> for f64 {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        Vec3::new(v.x * self, v.y * self, v.z * self)
    }
}

/// `operator/(v, d)`: plain component division (no guard against tiny `d`).
impl Div<f64> for Vec3 {
    type Output = Self;
    fn div(self, d: f64) -> Self {
        Self::new(self.x / d, self.y / d, self.z / d)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, o: Self) {
        self.x += o.x;
        self.y += o.y;
        self.z += o.z;
    }
}

impl SubAssign for Vec3 {
    fn sub_assign(&mut self, o: Self) {
        self.x -= o.x;
        self.y -= o.y;
        self.z -= o.z;
    }
}

impl MulAssign<f64> for Vec3 {
    fn mul_assign(&mut self, d: f64) {
        self.x *= d;
        self.y *= d;
        self.z *= d;
    }
}

/// `operator/=(d)`: leaves the vector unchanged when `|d| <= EFF_ZERO * |c|` for
/// every component `c` (so also for `0 / 0`), else divides.
impl DivAssign<f64> for Vec3 {
    fn div_assign(&mut self, d: f64) {
        let denom_chk = d.abs();
        if denom_chk <= EFF_ZERO * self.x.abs()
            && denom_chk <= EFF_ZERO * self.y.abs()
            && denom_chk <= EFF_ZERO * self.z.abs()
        {
            return;
        }
        self.x /= d;
        self.y /= d;
        self.z /= d;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fuzz regression: when `|v|^2` overflows, the unit vector underflows to
    /// zero and upstream fails after writing the x and z out-params.
    #[test]
    fn ortho_vectors_on_overflow() {
        let v = Vec3::new(1.0, -1.0, -1e300);
        assert!(v.make_unit_ortho_vectors(None).is_err());
        let [x, y, z] = v.unit_ortho_vectors_on_failure();
        let x = x.unwrap();
        assert_eq!(x.length_squared(), 0.0);
        assert!(y.is_none());
        assert_eq!(z, Some(x.cross(Vec3::new(0.0, 0.0, 1.0))));
        assert_eq!(
            Vec3::ZERO.unit_ortho_vectors_on_failure(),
            [Some(Vec3::Z), None, None]
        );
    }
    use crate::math::EFF_ZERO_DEG;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= EFF_ZERO
    }

    fn close3(v: Vec3, x: f64, y: f64, z: f64) -> bool {
        close(v.x, x) && close(v.y, y) && close(v.z, z)
    }

    /// `my_test_vector3d` (`tests/prog_test/src/smCurveTest.cpp:1907`), owned parts.
    #[test]
    fn upstream_my_test_vector3d() {
        let mut v2 = Vec3::new(0.5, 1.2, 2.2);
        assert!(v2.is_initialized());
        v2.set_uninitialized();
        assert!(!v2.is_initialized());

        let mut v3 = Vec3::from([1.0, 2.0, 2.0]);
        assert!(close(v3.length_squared(), 9.0));
        let mut v4 = Vec3::UNDEF;
        assert!(!v4.is_initialized());
        v4 = Vec3::new(3.0, 4.0, 0.0);
        assert!(close(v4.length(), 5.0));
        let mut v5 = v4;
        v4 = -v4;
        let mut v6 = v5;
        v6 += v4;
        assert!(close3(v6, 0.0, 0.0, 0.0));
        v6 -= v4;
        assert!(close3(v6, 3.0, 4.0, 0.0));

        v3 = v3.unitize().unwrap();
        assert!(close(v3.length(), 1.0));
        assert!(close(v3.dot(v3), 1.0));

        let v8 = Vec3::new(5.0, 1.0, 2.0);
        let v9 = Vec3::new(1.0, 3.0, 6.0);
        assert!(close(v8.distance(v9), 6.0));
        assert!(close(v8.angle_between(v9).unwrap(), 1.0022805307621478));
        v5 = Vec3::new(0.0, 0.0, 1.0);
        assert!(close(
            v5.ccw_angle_between(v8, v9).unwrap(),
            1.0516502125483738
        ));
        assert!(close(v8.distance_squared(v4), 93.0));
        assert!(!v8.closer_than(1.0, v9));
        assert!(close(v8.min_dimension(), 1.0));
        assert!(close(v8.max_dimension(), 5.0));

        let v10 = Vec3::new(-5.0, -1.0, -2.0);
        assert_eq!(v10 + v8, Vec3::ZERO);
        assert_eq!(v8 - v8, Vec3::ZERO);
        let mut v11 = v8 * 2.0;
        assert!(close3(v11, 10.0, 2.0, 4.0));
        v11 *= 3.0;
        assert!(close3(v11, 30.0, 6.0, 12.0));
        assert!(close3(v8 / 2.0, 2.5, 0.5, 1.0));
        v11 /= 2.0;
        assert!(close3(v11, 15.0, 3.0, 6.0));
        assert!(close3(v8.multiply(v9), 5.0, 3.0, 12.0));
        assert_eq!(v8.component(0), 5.0);
        *v11.component_mut(1).unwrap() = 7.0;
        assert_eq!(v11.y, 7.0);

        let mut tmp0 = Vec3::new(0.0, 0.0, 1.0);
        let mut tmp1 = Vec3::new(0.0, 1.0, 0.0);
        let mut v21 = Vec3::between(tmp0, tmp1);
        assert!(close3(v21, 0.0, 1.0, -1.0));
        let mut v22 = Vec3::from(Vec2::new(1.0, 0.0));
        assert!(close3(v22, 1.0, 0.0, 0.0));
        assert!(!v21.is_undef());

        tmp0 = Vec3::new(1.0, 1.0, 1.0);
        tmp1 = Vec3::new(3.0, 3.0, 3.0);
        assert!(tmp0.is_colinear_with(tmp1, Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)));
        assert!(tmp0.is_parallel_to(tmp1, 0.000001));
        tmp1 = Vec3::new(-3.0, 2.0, 1.0);
        assert!(tmp0.is_perpendicular_to(tmp1, 0.000001));
        v21 = Vec3::new(0.0, 2.0, 1.0);
        assert!(tmp0.is_inside_sector(tmp1, v21, v22));
        assert!(close(tmp0.triple_product(v21, v22), -1.0));

        tmp0 = Vec3::new(0.0, 1.0, 0.0);
        tmp1 = Vec3::new(0.0, 2.0, 1.0);
        let [x, y, z] = tmp0.make_unit_ortho_vectors(Some(tmp1)).unwrap();
        assert!(close3(x, 0.0, 1.0, 0.0));
        assert!(close3(y, 0.0, 0.0, 1.0));
        assert!(close3(z, 1.0, 0.0, 0.0));
        v22 = y;

        tmp1 = tmp1.unitize_or_z();
        assert!(close3(tmp0.project_to_plane(tmp1), 0.0, 0.2, -0.4));
        assert!(close3(
            tmp0.project_point_to_plane(v22, tmp1),
            0.0,
            0.6,
            -0.2
        ));

        let half_pi = core::f64::consts::FRAC_PI_2;
        tmp1 = Vec3::new(0.0, 0.0, 1.0);
        assert!(close3(
            tmp0.rotate_vec_about_axis(tmp1, half_pi),
            -1.0,
            0.0,
            0.0
        ));
        assert!(close3(tmp0.normal_in_plane(tmp1), 0.0, 0.0, 1.0));
        assert!(close3(tmp0.unitized_derivative(tmp1), 0.0, 0.0, 1.0));

        let tmp2 = Vec3::ZERO;
        let [d1, d2] = tmp0.unitized_derivative2(tmp1, tmp2).unwrap();
        assert!(close3(d1, 0.0, 0.0, 1.0));
        assert!(close3(d2, 0.0, -1.0, 0.0));
        let [d1, d2, d3] = tmp0.unitized_derivative3(tmp1, tmp2, Vec3::ZERO).unwrap();
        assert!(close3(d1, 0.0, 0.0, 1.0));
        assert!(close3(d2, 0.0, -1.0, 0.0));
        assert!(close3(d3, 0.0, 0.0, -3.0));

        let dc = tmp0.derivative_of_cross_product(tmp1, Vec3::new(-1.0, 0.0, 0.0), Vec3::ZERO);
        assert!(close3(dc, 0.0, 1.0, 0.0));

        let sol = Vec3::new(0.0, 3.0, 6.0)
            .solve_two_linear_equations(Vec3::new(5.0, 2.0, 9.0))
            .unwrap();
        assert!(close(sol.x, 1.0) && close(sol.y, 2.0));

        let mut a = Vec3::new(2.0, 3.0, 1.0);
        let mut b = Vec3::new(4.0, 1.0, 5.0);
        core::mem::swap(&mut a, &mut b);
        assert!(close3(a.cross(b), -14.0, 6.0, 10.0));

        assert!(Vec3::new(3.0, 6.0, 5.0).approx_eq(Vec3::new(3.0, 6.0, 5.0)));
        assert!(!Vec3::new(3.0, 6.0, 5.0).approx_eq(Vec3::new(3.0, 7.0, 5.0)));
    }

    #[test]
    fn unitize_edge_cases() {
        assert!(matches!(
            Vec3::ZERO.unitize(),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(Vec3::new(1e-13, 0.0, 0.0).unitize_or_z(), Vec3::Z);
        let n = Vec3::new(0.6, 0.0, 0.8);
        assert_eq!(n.unitize().unwrap(), n);
        assert_eq!(
            Vec3::new(3.0, 0.0, 4.0).unitize().unwrap(),
            Vec3::new(0.6, 0.0, 0.8)
        );
    }

    #[test]
    fn degenerate_paths() {
        // Zero-length self: derivatives pass through.
        let d = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(Vec3::ZERO.unitized_derivative(d), d);
        assert_eq!(Vec3::ZERO.unitized_derivative2(d, -d), Err([d, -d]));
        assert_eq!(Vec3::ZERO.unitized_derivative3(d, d, d), Err([d, d, d]));
        // Zero vectors are neither parallel nor perpendicular.
        assert_eq!(Vec3::ZERO.parallel_sense(Vec3::X, 1.0), (false, None));
        assert!(!Vec3::X.is_perpendicular_to(Vec3::ZERO, 1.0));
        assert_eq!(
            Vec3::X.parallel_sense(-Vec3::X, EFF_ZERO_DEG),
            (true, Some(true))
        );
        assert!(Vec3::X.angle_between(Vec3::ZERO).is_err());
        // Parallel to the projection axis gives 0.
        assert_eq!(Vec3::Z.ccw_angle_between(Vec3::Z, Vec3::X).unwrap(), 0.0);
        assert!(close(
            Vec3::Z.ccw_angle_between(Vec3::Y, Vec3::X).unwrap(),
            -PI / 2.0
        ));
        assert!(Vec3::ZERO.make_unit_ortho_vectors(None).is_err());
        let [x, y, z] = Vec3::new(0.0, 5.0, 0.0)
            .make_unit_ortho_vectors(None)
            .unwrap();
        assert_eq!((x, y, z), (Vec3::Y, Vec3::Z, Vec3::X));
        assert!(Vec3::new(1.0, 2.0, 3.0)
            .solve_two_linear_equations(Vec3::new(2.0, 4.0, 6.0))
            .is_err());
        // Rotation about the vector itself is the identity.
        assert_eq!(Vec3::Z.rotate_vec_about_axis(Vec3::Z, 1.0), Vec3::Z);
        assert!(Vec3::X.is_colinear_with(Vec3::Y, Vec3::X, Vec3::X));
    }

    #[test]
    fn operators_and_predicates() {
        let mut v = Vec3::new(1.0, 2.0, 3.0);
        v /= 1e-13;
        assert_eq!(v, Vec3::new(1.0, 2.0, 3.0));
        let mut z = Vec3::ZERO;
        z /= 0.0;
        assert_eq!(z, Vec3::ZERO);
        assert!((Vec3::ZERO / 0.0).x.is_nan());
        assert_eq!(v.component(3), 0.0);
        assert!(v.component_mut(3).is_err());
        assert!(Vec3::UNDEF.approx_eq(Vec3::UNDEF));
        assert!(!Vec3::UNDEF.approx_eq(v));
        assert!(Vec3::new(f64::NAN, 0.0, 0.0).is_undef());
        assert!(Vec3::new(1e19, 0.0, 0.0).is_undef());
        assert!(!Vec3::new(1e17, 0.0, 0.0).is_undef());
        assert!(Vec3::new(0.0, 0.0, NOZ).is_2d());
        assert_eq!(
            Vec3::from_nl([2.0, 4.0, NOZ, 2.0]),
            Vec3::new(1.0, 2.0, 0.0)
        );
        assert_eq!(
            Vec3::from_nl([2.0, 4.0, 6.0, NOW]),
            Vec3::new(2.0, 4.0, 6.0)
        );
        assert_eq!(sqrt(-1.0), 0.0);
        assert_eq!(cbrt(27.0), 3.0);
        assert_eq!(acos(1.5), 0.0);
        assert_eq!(acos(-1.5), PI);
        assert_eq!(asin(-1.5), -PI / 2.0);
    }
}
