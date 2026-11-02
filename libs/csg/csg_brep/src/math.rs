//! Small `f64` linear-algebra types: [`Vec3`]/[`Point3`], [`Vec2`]/[`Point2`],
//! [`Transform`], and the numeric constants of `SmMath.h`.
//!
//! Upstream: `source/SMLib/inc/SmVector3d.h`, `SmVector2d.h`, `SmCoreTypes.h`,
//! `SmMath.h`, `SmTransform.h` / `src/SmTransform.cpp`, and `smgu_4x4Mult` in
//! `src/SmGeomUtility.cpp`.
//!
//! | Rust                         | Upstream                                       |
//! |------------------------------|------------------------------------------------|
//! | [`Vec3`] / [`Point3`]        | `SmVector3d` / `SmPoint3d` (re-exported from [`crate::geom::vector3d`]) |
//! | [`Vec2`] / [`Point2`]        | `SmVector2d` / `SmPoint2d` (re-exported from [`crate::geom::vector2d`]) |
//! | [`Transform`]                | `SmTransform` (`m_adT[4][4]`)                  |
//! | [`Transform::concat`]        | `SmTransform::ConcatTransform` / `smgu_4x4Mult`|
//! | [`Transform::transform_point`]  | `SmTransform::TransformPoint`               |
//! | [`Transform::transform_vector`] | `SmTransform::TransformVector`              |
//! | [`EFF_ZERO`] etc.            | `SM_EFF_ZERO` etc.                             |
//! | [`is_infinite_param`]        | `SM_IS_INFINITE`                               |
//! | [`max2`], [`min2`], [`max3`], [`min3`], [`max4`], [`min4`] | `smos_Max`, `smos_Min`, `smos_3Max`, `smos_3Min`, `smos_4Max`, `smos_4Min` |
//! | [`rad_to_deg`]               | `SM_RAD2DEG`                                   |
//!
//! ## Why not `glam`/`nalgebra`
//!
//! The correctness gate compares against the C++ to `1e-9` relative, and the port keeps
//! upstream operation order. Third-party vector crates may reassociate sums, use SIMD
//! lanes or fused multiply-add, and do not match upstream degenerate-case behaviour
//! (e.g. `Unitize` of a zero vector). These hand-written types spell every expression
//! in upstream order, so results are bit-identical to the C++ (built with
//! `-ffp-contract=off`).

pub use crate::geom::vector2d::{Point2, Vec2};
pub use crate::geom::vector3d::{Point3, Vec3};

/// `SM_PI` (the upstream literal rounds to exactly this `f64`).
pub const PI: f64 = core::f64::consts::PI;
/// `SM_HALFPI`.
pub const HALF_PI: f64 = core::f64::consts::FRAC_PI_2;
/// `SM_3HALFPI`.
pub const THREE_HALF_PI: f64 = 4.7123889803846897576939650749192543262957530990626;
/// `SM_2PI`.
pub const TWO_PI: f64 = core::f64::consts::TAU;
/// `SM_SQRT3`.
pub const SQRT3: f64 = 1.7320508075688772935274463415058723669428052538104;
/// `SM_EFF_ZERO`: numeric zero tolerance.
pub const EFF_ZERO: f64 = 1.0e-12;
/// `SM_EFF_ZERO_SQ`: `EFF_ZERO` squared.
pub const EFF_ZERO_SQ: f64 = 1.0e-24;
/// `SM_EFF_ZERO_SQRT`: square root of `EFF_ZERO`.
pub const EFF_ZERO_SQRT: f64 = 1.0e-6;
/// `SM_EFF_ZERO_PARAM`: numeric zero tolerance for knot/parameter values.
pub const EFF_ZERO_PARAM: f64 = 1.2e-8;
/// `SM_EFF_ZERO_DEG`: numeric zero tolerance for angles in degrees.
pub const EFF_ZERO_DEG: f64 = 1.0e-4;
/// `SM_EFF_ZERO_RAD`: numeric zero tolerance for angles in radians.
pub const EFF_ZERO_RAD: f64 = EFF_ZERO_DEG * PI / 180.0;
/// `SM_UNDEF_DOUBLE`: upstream's sentinel for an unset double.
pub const UNDEF_DOUBLE: f64 = 1.23456e-61;
/// `SM_BIG_DOUBLE`: a value larger than any coordinate (used for "inside out" extents).
pub const BIG_DOUBLE: f64 = 1.0e20;
/// `SM_INFINITE_PARAMETER`: the value that marks an unbounded interval end.
pub const INFINITE_PARAMETER: f64 = 1234567.0;
/// `SM_INFINITE_PARAMETER_SQRT`: unbounded marker for representations that square it.
pub const INFINITE_PARAMETER_SQRT: f64 = 1111.111;
/// `SM_BOUNDED_INFINITE_PARAM`: half size used to approximate an unbounded interval.
pub const BOUNDED_INFINITE_PARAM: f64 = 33.0;

/// `SM_IS_INFINITE(a)`: true when `a` is `±INFINITE_PARAMETER` or
/// `±INFINITE_PARAMETER_SQRT` (exact comparison, as upstream).
#[must_use]
pub fn is_infinite_param(a: f64) -> bool {
    a == INFINITE_PARAMETER
        || a == -INFINITE_PARAMETER
        || a == INFINITE_PARAMETER_SQRT
        || a == -INFINITE_PARAMETER_SQRT
}

/// `smos_Max(a, b)`: `a > b ? a : b`.
///
/// Unlike [`f64::max`], the result for a NaN operand follows the comparison
/// (`b` when either is NaN), as upstream's macro does.
#[must_use]
pub fn max2(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `smos_Min(a, b)`: `a < b ? a : b` (NaN handling as [`max2`]).
#[must_use]
pub fn min2(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `smos_3Max(a, b, c)`, with upstream's comparison order.
#[must_use]
pub fn max3(a: f64, b: f64, c: f64) -> f64 {
    if a > b {
        if a > c {
            a
        } else {
            c
        }
    } else if b > c {
        b
    } else {
        c
    }
}

/// `smos_3Min(a, b, c)`, with upstream's comparison order.
#[must_use]
pub fn min3(a: f64, b: f64, c: f64) -> f64 {
    if a < b {
        if a < c {
            a
        } else {
            c
        }
    } else if b < c {
        b
    } else {
        c
    }
}

/// `smos_4Max(a, b, c, d)` = `smos_Max(smos_Max(a, b), smos_Max(c, d))`.
#[must_use]
pub fn max4(a: f64, b: f64, c: f64, d: f64) -> f64 {
    max2(max2(a, b), max2(c, d))
}

/// `smos_4Min(a, b, c, d)` = `smos_Min(smos_Min(a, b), smos_Min(c, d))`.
#[must_use]
pub fn min4(a: f64, b: f64, c: f64, d: f64) -> f64 {
    min2(min2(a, b), min2(c, d))
}

/// `SM_RAD2DEG(a)` = `a * (180.0 / SM_PI)`: radians to degrees.
#[must_use]
pub fn rad_to_deg(a: f64) -> f64 {
    a * (180.0 / PI)
}

/// An affine 3D transform stored as upstream's row-vector 4×4 matrix
/// (`SmTransform::m_adT`): `out = [p.x p.y p.z 1] * m`.
///
/// Rows 0..3 are the images of the X, Y, Z axes; row 3 is the translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// The 4×4 matrix, `m[row][col]`.
    pub m: [[f64; 4]; 4],
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// The identity transform.
    pub const IDENTITY: Self = Self {
        m: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    /// A pure translation by `d`.
    #[must_use]
    pub const fn translation(d: Vec3) -> Self {
        let mut t = Self::IDENTITY;
        t.m[3] = [d.x, d.y, d.z, 1.0];
        t
    }

    /// Builds a transform from the images of the axes and the origin
    /// (`SetToXAxis`/`SetToYAxis`/`SetToZAxis`/`SetToDisp`).
    #[must_use]
    pub const fn from_axes(x: Vec3, y: Vec3, z: Vec3, origin: Point3) -> Self {
        Self {
            m: [
                [x.x, x.y, x.z, 0.0],
                [y.x, y.y, y.z, 0.0],
                [z.x, z.y, z.z, 0.0],
                [origin.x, origin.y, origin.z, 1.0],
            ],
        }
    }

    /// Image of the X axis (`GetToXAxis`).
    #[must_use]
    pub const fn x_axis(&self) -> Vec3 {
        Vec3::new(self.m[0][0], self.m[0][1], self.m[0][2])
    }

    /// Image of the Y axis (`GetToYAxis`).
    #[must_use]
    pub const fn y_axis(&self) -> Vec3 {
        Vec3::new(self.m[1][0], self.m[1][1], self.m[1][2])
    }

    /// Image of the Z axis (`GetToZAxis`).
    #[must_use]
    pub const fn z_axis(&self) -> Vec3 {
        Vec3::new(self.m[2][0], self.m[2][1], self.m[2][2])
    }

    /// Translation part (`GetToDisp` / `GetToOrigin`).
    #[must_use]
    pub const fn translation_part(&self) -> Vec3 {
        Vec3::new(self.m[3][0], self.m[3][1], self.m[3][2])
    }

    /// `self * next`: apply `self`, then `next` (`ConcatTransform`, `smgu_4x4Mult`,
    /// same summation order).
    #[must_use]
    pub fn concat(&self, next: &Self) -> Self {
        let mut r = [[0.0; 4]; 4];
        // Loop order is kk-outer instead of upstream's jj-outer, but each r[ii][jj] still
        // accumulates 0.0 + a[ii][0]*b[0][jj] + ... + a[ii][3]*b[3][jj] in kk order,
        // so the result is bit-identical.
        for (r_row, a_row) in r.iter_mut().zip(self.m.iter()) {
            for (a_val, b_row) in a_row.iter().zip(next.m.iter()) {
                for (r_val, b_val) in r_row.iter_mut().zip(b_row.iter()) {
                    *r_val += a_val * b_val;
                }
            }
        }
        Self { m: r }
    }

    /// Maps a point (`SmTransform::TransformPoint`).
    #[must_use]
    pub fn transform_point(&self, p: Point3) -> Point3 {
        let (tx, ty, tz, td) = (
            self.x_axis(),
            self.y_axis(),
            self.z_axis(),
            self.translation_part(),
        );
        Vec3::new(
            td.x + p.x * tx.x + p.y * ty.x + p.z * tz.x,
            td.y + p.x * tx.y + p.y * ty.y + p.z * tz.y,
            td.z + p.x * tx.z + p.y * ty.z + p.z * tz.z,
        )
    }

    /// Maps a vector, ignoring translation (`SmTransform::TransformVector`).
    #[must_use]
    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        let (tx, ty, tz) = (self.x_axis(), self.y_axis(), self.z_axis());
        Vec3::new(
            v.x * tx.x + v.y * ty.x + v.z * tz.x,
            v.x * tx.y + v.y * ty.y + v.z * tz.y,
            v.x * tx.z + v.y * ty.z + v.z * tz.z,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::approx_constant)]
    fn constants_match_upstream_literals() {
        // Upstream SmMath.h literals round to the std constants bit for bit.
        assert_eq!(PI, 3.1415926535897932384626433832795028841971693993751);
        assert_eq!(HALF_PI, 1.5707963267948965192313216916397514420985846996875);
        assert_eq!(TWO_PI, 6.2831853071795864769252867665590057683943387987502);
    }

    #[test]
    fn dot_cross_length() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(-4.0, 5.0, 0.5);
        assert_eq!(a.dot(b), -4.0 + 10.0 + 1.5);
        assert_eq!(
            a.cross(b),
            Vec3::new(2.0 * 0.5 - 3.0 * 5.0, 3.0 * -4.0 - 0.5, 5.0 + 8.0)
        );
        assert_eq!(Vec3::X.cross(Vec3::Y), Vec3::Z);
        assert_eq!(Vec3::new(3.0, 4.0, 12.0).length(), 13.0);
        assert_eq!(a.cross(b).dot(a), 0.0);
    }

    #[test]
    fn transform_point_vector_concat() {
        let rot_z = Transform::from_axes(Vec3::Y, -Vec3::X, Vec3::Z, Vec3::ZERO);
        let tr = Transform::translation(Vec3::new(1.0, 2.0, 3.0));
        let p = Vec3::new(1.0, 0.0, 0.0);
        assert_eq!(rot_z.transform_point(p), Vec3::new(0.0, 1.0, 0.0));
        // concat applies self first, then the argument.
        let both = rot_z.concat(&tr);
        assert_eq!(both.transform_point(p), Vec3::new(1.0, 3.0, 3.0));
        assert_eq!(both.transform_vector(p), Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(Transform::IDENTITY.concat(&both), both);
        assert_eq!(Transform::default(), Transform::IDENTITY);
    }

    #[test]
    fn vec2_basics() {
        let a = Vec2::new(3.0, 4.0);
        assert_eq!(a.length(), 5.0);
        assert_eq!((a - a + a * 2.0).dot(-a), -50.0);
    }
}
