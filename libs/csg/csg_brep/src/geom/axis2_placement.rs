//! Right-handed coordinate systems (origin, X axis, Y axis; Z = X × Y) used as
//! rigid rotate-and-translate transformations.
//!
//! Upstream: `source/SMLib/inc/SmAxis2Placement.h`, `source/SMLib/src/SmAxis2Placement.cpp`.
//!
//! | Rust                                          | Upstream                                              |
//! |-----------------------------------------------|-------------------------------------------------------|
//! | [`Axis2Placement`]                            | `SmAxis2Placement`                                    |
//! | [`Axis2Placement::IDENTITY`], `Default`, [`Axis2Placement::init`] | `SmAxis2Placement()`, `Init`      |
//! | `Copy` / `Clone`                              | copy constructor                                      |
//! | [`Axis2Placement::new`]                       | `SmAxis2Placement(Origin, XAxis, YAxis)` (orient)     |
//! | [`Axis2Placement::from_plane`]                | `SmAxis2Placement(const SmVector3d&, double)` (plane) |
//! | [`Axis2Placement::from_to`]                   | `SmAxis2Placement(FromO, FromX, FromY, ToO, ToX, ToY)` (takes the two built systems) |
//! | [`Axis2Placement::origin`]                    | `GetOrigin`, `GetOriginRef`                           |
//! | [`Axis2Placement::x_axis`]                    | `GetXAxis`, `GetXAxisRef`                             |
//! | [`Axis2Placement::y_axis`]                    | `GetYAxis`, `GetYAxisRef`                             |
//! | [`Axis2Placement::z_axis`]                    | `GetZAxis`                                            |
//! | [`Axis2Placement::canonical`]                 | `GetCanonical`                                        |
//! | [`Axis2Placement::set_canonical`]             | `SetCanonical`                                        |
//! | [`Axis2Placement::set_step_canonical`]        | `SetSTEPCanonical`                                    |
//! | [`Axis2Placement::matrix`]                    | `GetMatrix`                                           |
//! | [`Axis2Placement::mirror_matrix`]             | `GetMirrorMatrix`                                     |
//! | `mult_matrix` (private)                       | `sm_MultMatrix`                                       |
//! | [`Axis2Placement::mirror_point`]              | `MirrorPoint`                                         |
//! | [`Axis2Placement::mirror_vector`]             | `MirrorVector`                                        |
//! | (array copy)                                  | `sm_CopyMatrix`                                       |
//! | [`Axis2Placement::translate`]                 | `Translate`                                           |
//! | [`Axis2Placement::rotate_about_axis`]         | `RotateAboutAxis`                                     |
//! | [`Axis2Placement::rotate_about_axis_at_point`] | `RotateAboutAxisAtPoint`                             |
//! | [`Axis2Placement::transform_point`]           | `TransformPoint`                                      |
//! | [`Axis2Placement::transform_vector`]          | `TransformVector`                                     |
//! | [`Axis2Placement::transform_axis2_placement`] | `TransformAxis2Placement`                             |
//! | [`Axis2Placement::invert`]                    | `Invert`                                              |
//! | [`Axis2Placement::approx_eq`]                 | `operator==`                                          |
//! | [`Axis2Placement::is_identity`]               | `IsIdentity`                                          |
//! | [`Axis2Placement::are_coaxial`]               | `AreCoaxial`                                          |
//!
//! Deviations:
//! - The from/to constructor's six vectors become two placements built with
//!   [`Axis2Placement::new`] (what upstream builds internally).
//! - Out-params become return values (`GetCanonical`, `GetMatrix`, `GetMirrorMatrix`,
//!   `TransformPoint`, `TransformVector`, `TransformAxis2Placement`, `Invert`); the
//!   4x4 matrices are `[[f64; 4]; 4]` in upstream's `padMat[row][col]` layout.
//! - `SetCanonical` / `SetSTEPCanonical` return `Result<()>` and, like upstream, leave
//!   the partially updated state in `self` on error (the setters' `SER` returns early).
//!   Unitize failures are `SolidError::InvalidInput` (`SM_ERR_INVALID_INPUT`).
//! - The constructors ignore `SetCanonical`'s status, as upstream does. A zero plane
//!   normal leaves both axes at [`Vec3::UNDEF`] (upstream's default-constructed
//!   members, untouched when `MakeUnitOrthoVectors` fails); a normal whose squared
//!   length overflows gets the Y axis upstream writes before failing
//!   ([`Vec3::unit_ortho_vectors_on_failure`]).
//! - Upstream's tolerant `operator==` is [`Axis2Placement::approx_eq`]; Rust `==`
//!   (`PartialEq`) is exact component equality.
//! - `SM_ASSERT`, `SE`, `SE_MSG` and `ERR` are no-ops in the release build and omitted;
//!   the `SM_DEBUG_CODE` blocks are not compiled upstream either.
//! - The unused-upstream members (`MirrorPoint`, `MirrorVector`, `InvTransformPoint`,
//!   `InvTransformVector`, `DecomposeToAngles`, `ComposeFromAngles`, `Load4x4`,
//!   `SetFrom4x4`, the nine-double constructor, `Draw`, `Dump`, `AssertValid`) are not
//!   part of this unit.

use crate::error::Result;
use crate::geom::geom_utility::p1::line_point_distance;
use crate::geom::tol::are_same;
use crate::geom::vector3d::{Point3, Vec3};
use crate::math::EFF_ZERO;

/// A right-handed coordinate system: an origin and unit, perpendicular X and Y axes
/// (Z is X × Y). As a transformation it maps local coordinates `[u v w]` to
/// `origin + u·X + v·Y + w·Z` (`SmAxis2Placement`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis2Placement {
    origin: Point3,
    x_axis: Vec3,
    y_axis: Vec3,
}

impl Default for Axis2Placement {
    /// The identity placement (`SmAxis2Placement()`).
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// `padMat[i][j] = sum_k a[i][k] * b[k][j]`, accumulated from `0.0` in `k` order
/// (`sm_MultMatrix`).
fn mult_matrix(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut r = [[0.0; 4]; 4];
    for (row_r, row_a) in r.iter_mut().zip(a) {
        for (j, cell) in row_r.iter_mut().enumerate() {
            let mut val = 0.0;
            for (aik, row_b) in row_a.iter().zip(b) {
                val += aik * row_b.get(j).copied().unwrap_or(0.0);
            }
            *cell = val;
        }
    }
    r
}

impl Axis2Placement {
    /// The identity placement: origin `(0,0,0)`, X `(1,0,0)`, Y `(0,1,0)`.
    pub const IDENTITY: Self = Self {
        origin: Vec3::ZERO,
        x_axis: Vec3::X,
        y_axis: Vec3::Y,
    };

    /// Reset to the identity placement (`Init`).
    pub fn init(&mut self) {
        *self = Self::IDENTITY;
    }

    /// Orient constructor: the placement with origin `origin` and axes from
    /// [`set_canonical`](Self::set_canonical) (whose status is ignored, as upstream).
    #[must_use]
    pub fn new(origin: Point3, x_axis: Vec3, y_axis: Vec3) -> Self {
        let mut p = Self::IDENTITY;
        // Upstream ignores SetCanonical's status in the constructor.
        let _ = p.set_canonical(origin, x_axis, y_axis);
        p
    }

    /// Plane constructor: the placement of the plane `normal · p + d = 0`, with origin
    /// the plane point closest to the global origin, Z along `normal` and X, Y from
    /// `MakeUnitOrthoVectors` (`SmAxis2Placement(const SmVector3d&, double)`).
    #[must_use]
    pub fn from_plane(normal: Vec3, d: f64) -> Self {
        let size = normal.length();
        let mut origin = normal;
        origin *= -d / size / size;
        // Upstream writes MakeUnitOrthoVectors' Y and Z outputs straight into the
        // members, which keep their default (undefined) value where it fails
        // before writing them.
        let (x_axis, y_axis) = if let Ok([_, y, z]) = normal.make_unit_ortho_vectors(None) {
            (y, z)
        } else {
            let [_, y, z] = normal.unit_ortho_vectors_on_failure();
            (y.unwrap_or(Vec3::UNDEF), z.unwrap_or(Vec3::UNDEF))
        };
        Self {
            origin,
            x_axis,
            y_axis,
        }
    }

    /// From/to constructor: the placement mapping coordinates of the `from` system
    /// to the `to` system, `inverse(from) * to`. Upstream builds both systems from
    /// origin/X/Y triples with the orient constructor; pass
    /// [`new`](Self::new)`(origin, x, y)` for each.
    #[must_use]
    pub fn from_to(from: &Self, to: &Self) -> Self {
        from.invert().transform_axis2_placement(to)
    }

    /// The origin (`GetOrigin`, `GetOriginRef`).
    #[must_use]
    pub fn origin(&self) -> Point3 {
        self.origin
    }

    /// The X axis (`GetXAxis`, `GetXAxisRef`).
    #[must_use]
    pub fn x_axis(&self) -> Vec3 {
        self.x_axis
    }

    /// The Y axis (`GetYAxis`, `GetYAxisRef`).
    #[must_use]
    pub fn y_axis(&self) -> Vec3 {
        self.y_axis
    }

    /// The Z axis, `X × Y` unitized (status ignored, so `(0,0,1)` for parallel axes)
    /// (`GetZAxis`).
    #[must_use]
    pub fn z_axis(&self) -> Vec3 {
        self.x_axis.cross(self.y_axis).unitize_or_z()
    }

    /// `(origin, x_axis, y_axis)` (`GetCanonical`).
    #[must_use]
    pub fn canonical(&self) -> (Point3, Vec3, Vec3) {
        (self.origin, self.x_axis, self.y_axis)
    }

    /// Set origin and axes (`SetCanonical`). Axes that are not unit length are
    /// unitized; axes that are not perpendicular (`|X·Y| > SM_EFF_ZERO`) are replaced
    /// by the orthonormal frame of `MakeUnitOrthoVectors(X, &Y)`.
    ///
    /// # Errors
    ///
    /// `SolidError::InvalidInput` when an axis has zero length. As upstream, `self`
    /// then keeps the values stored so far (the failed axis is `(0,0,1)`).
    pub fn set_canonical(&mut self, origin: Point3, x_axis: Vec3, y_axis: Vec3) -> Result<()> {
        self.origin = origin;
        self.x_axis = x_axis;
        self.y_axis = y_axis;

        // make sure X Axis is a unit vector
        if (self.x_axis.dot(self.x_axis) - 1.0).abs() > EFF_ZERO / 100.0 {
            let unit = self.x_axis.unitize();
            self.x_axis = self.x_axis.unitize_or_z();
            unit?;
        }

        // make sure Y Axis is a unit vector
        if (self.y_axis.dot(self.y_axis) - 1.0).abs() > EFF_ZERO / 100.0 {
            let unit = self.y_axis.unitize();
            self.y_axis = self.y_axis.unitize_or_z();
            unit?;
        }

        // make sure vectors are perpendicular
        if self.x_axis.dot(self.y_axis).abs() > EFF_ZERO {
            let [x, y, _] = x_axis.make_unit_ortho_vectors(Some(y_axis))?;
            self.x_axis = x;
            self.y_axis = y;
        }
        Ok(())
    }

    /// Set origin, Z axis and the X reference direction, STEP style: X is the unit
    /// component of `x_ref_direction` perpendicular to Z (`SetSTEPCanonical`).
    ///
    /// # Errors
    ///
    /// `SolidError::InvalidInput` when `z_axis` has zero length; `self` is then
    /// unchanged.
    pub fn set_step_canonical(
        &mut self,
        origin: Point3,
        z_axis: Vec3,
        x_ref_direction: Vec3,
    ) -> Result<()> {
        let [_, x, y] = z_axis.make_unit_ortho_vectors(Some(x_ref_direction))?;
        self.origin = origin;
        self.x_axis = x;
        self.y_axis = y;
        Ok(())
    }

    /// The 4x4 matrix `[[Xx Yx Zx Ox] [Xy Yy Zy Oy] [Xz Yz Zz Oz] [0 0 0 1]]`
    /// (`GetMatrix`, `padMat[row][col]`).
    #[must_use]
    pub fn matrix(&self) -> [[f64; 4]; 4] {
        let x = self.x_axis;
        let y = self.y_axis;
        let z = self.z_axis();
        let p = self.origin;
        [
            [x.x, y.x, z.x, p.x],
            [x.y, y.y, z.y, p.y],
            [x.z, y.z, z.z, p.z],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    /// The 4x4 matrix of the mirror about this placement's XY plane,
    /// `M * diag(1,1,-1,1) * M⁻¹` in [`matrix`](Self::matrix) layout
    /// (`GetMirrorMatrix`).
    #[must_use]
    pub fn mirror_matrix(&self) -> [[f64; 4]; 4] {
        let inv = self.invert();
        let mat2 = self.matrix();
        let mut mat1 = Self::IDENTITY.matrix();
        if let Some(c) = mat1.get_mut(2).and_then(|r| r.get_mut(2)) {
            *c = -1.0;
        }
        let mat3 = mult_matrix(&mat2, &mat1);
        let mat1 = inv.matrix();
        mult_matrix(&mat3, &mat1)
    }

    /// The mirror image of `point` about this placement's XY plane (`MirrorPoint`):
    /// twice the signed distance to the plane is removed along the unit normal.
    #[must_use]
    pub fn mirror_point(&self, point: Point3) -> Point3 {
        let n = self.z_axis();
        let o = self.origin;
        // a tad faster - double the distance from the point to the mirror plane
        let k = 2.0 * ((point.x - o.x) * n.x + (point.y - o.y) * n.y + (point.z - o.z) * n.z);
        Point3::new(point.x - k * n.x, point.y - k * n.y, point.z - k * n.z)
    }

    /// The mirror image of `vector` about this placement's XY plane (`MirrorVector`).
    #[must_use]
    pub fn mirror_vector(&self, vector: Vec3) -> Vec3 {
        let n = self.z_axis();
        // 2 times the to-mirror-plane vector component size
        let k = 2.0 * vector.dot(n);
        Vec3::new(vector.x - k * n.x, vector.y - k * n.y, vector.z - k * n.z)
    }

    /// Post-concatenate a translation: `origin += translation` (`Translate`).
    pub fn translate(&mut self, translation: Vec3) {
        self.origin.x += translation.x;
        self.origin.y += translation.y;
        self.origin.z += translation.z;
    }

    /// Post-concatenate a rotation by `angle_radians` (right-hand rule) about `axis`
    /// through the global origin (`RotateAboutAxis`). A zero-length axis is replaced by
    /// `(1,0,0)`.
    pub fn rotate_about_axis(&mut self, angle_radians: f64, axis: Vec3) {
        let a = axis.unitize().unwrap_or(Vec3::X);

        let s = angle_radians.sin();
        let c = angle_radians.cos();
        let t = 1.0 - c;

        // X axis [1,0,0] rotated about A.
        let x_axis = Vec3::new(
            t * a.x * a.x + c,
            t * a.x * a.y + s * a.z,
            t * a.x * a.z - s * a.y,
        );
        // Y axis [0,1,0] rotated about A.
        let y_axis = Vec3::new(
            t * a.y * a.x - s * a.z,
            t * a.y * a.y + c,
            t * a.y * a.z + s * a.x,
        );

        // reunitize
        let x_axis = x_axis.unitize_or_z();
        let y_axis = y_axis.unitize_or_z();

        // reorthogonalize to eliminate tolerance build up
        let y_axis = x_axis.cross(y_axis).cross(x_axis).unitize_or_z();

        let mut other = Self::IDENTITY;
        if other.set_canonical(Vec3::ZERO, x_axis, y_axis).is_err() {
            return;
        }
        *self = self.transform_axis2_placement(&other);
    }

    /// Post-concatenate a rotation by `angle_radians` about the axis through `center`
    /// directed along `axis` (`RotateAboutAxisAtPoint`).
    pub fn rotate_about_axis_at_point(&mut self, angle_radians: f64, center: Point3, axis: Vec3) {
        self.translate(-center);
        self.rotate_about_axis(angle_radians, axis);
        self.translate(center);
    }

    /// Map a local point to global coordinates, `origin + u·X + v·Y + w·Z`
    /// (`TransformPoint`).
    #[must_use]
    pub fn transform_point(&self, uvw: Point3) -> Point3 {
        let z = self.z_axis();
        let (o, x, y) = (self.origin, self.x_axis, self.y_axis);
        Vec3::new(
            o.x + uvw.x * x.x + uvw.y * y.x + uvw.z * z.x,
            o.y + uvw.x * x.y + uvw.y * y.y + uvw.z * z.y,
            o.z + uvw.x * x.z + uvw.y * y.z + uvw.z * z.z,
        )
    }

    /// Map a local vector to global coordinates (rotation only), `u·X + v·Y + w·Z`
    /// (`TransformVector`).
    #[must_use]
    pub fn transform_vector(&self, uvw: Vec3) -> Vec3 {
        let z = self.z_axis();
        let (x, y) = (self.x_axis, self.y_axis);
        Vec3::new(
            uvw.x * x.x + uvw.y * y.x + uvw.z * z.x,
            uvw.x * x.y + uvw.y * y.y + uvw.z * z.y,
            uvw.x * x.z + uvw.y * y.z + uvw.z * z.z,
        )
    }

    /// The concatenation `self * input`: applying the result equals applying `self`
    /// then `input` (`TransformAxis2Placement`).
    #[must_use]
    pub fn transform_axis2_placement(&self, input: &Self) -> Self {
        let iz = input.z_axis();
        let (ix, iy, io) = (input.x_axis, input.y_axis, input.origin);
        let (mx, my, mo) = (self.x_axis, self.y_axis, self.origin);

        let x_axis = Vec3::new(
            mx.x * ix.x + mx.y * iy.x + mx.z * iz.x,
            mx.x * ix.y + mx.y * iy.y + mx.z * iz.y,
            mx.x * ix.z + mx.y * iy.z + mx.z * iz.z,
        );
        let y_axis = Vec3::new(
            my.x * ix.x + my.y * iy.x + my.z * iz.x,
            my.x * ix.y + my.y * iy.y + my.z * iz.y,
            my.x * ix.z + my.y * iy.z + my.z * iz.z,
        );
        let origin = Vec3::new(
            mo.x * ix.x + mo.y * iy.x + mo.z * iz.x + io.x,
            mo.x * ix.y + mo.y * iy.y + mo.z * iz.y + io.y,
            mo.x * ix.z + mo.y * iy.z + mo.z * iz.z + io.z,
        );
        Self {
            origin,
            x_axis,
            y_axis,
        }
    }

    /// The inverse placement, mapping global coordinates to this placement's local
    /// ones (`Invert`).
    #[must_use]
    pub fn invert(&self) -> Self {
        let z = self.z_axis();
        let (x, y, o) = (self.x_axis, self.y_axis, self.origin);
        Self {
            origin: Vec3::new(-x.dot(o), -y.dot(o), -z.dot(o)),
            x_axis: Vec3::new(x.x, y.x, z.x),
            y_axis: Vec3::new(x.y, y.y, z.y),
        }
    }

    /// Tolerant equality: every origin and axis component pair passes `SM_ARE_SAME`
    /// (`|a-b| <= SM_EFF_ZERO * (1 + max(|a|,|b|))`); a placement always equals itself
    /// (`operator==`).
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // when objects are equivalent
        if core::ptr::eq(self, other) {
            return true;
        }
        let pairs = [
            (self.origin.x, other.origin.x),
            (self.origin.y, other.origin.y),
            (self.origin.z, other.origin.z),
            (self.x_axis.x, other.x_axis.x),
            (self.x_axis.y, other.x_axis.y),
            (self.x_axis.z, other.x_axis.z),
            (self.y_axis.x, other.y_axis.x),
            (self.y_axis.y, other.y_axis.y),
            (self.y_axis.z, other.y_axis.z),
        ];
        pairs.iter().all(|&(a, b)| are_same(a, b))
    }

    /// True when every origin and axis component is within `tol` of the identity
    /// placement's (`IsIdentity`; upstream default `tol` is `SM_EFF_ZERO`).
    #[must_use]
    pub fn is_identity(&self, tol: f64) -> bool {
        let (o, x, y) = (self.origin, self.x_axis, self.y_axis);
        // Same comparisons, same order (NaN components pass, as upstream).
        !(o.x > tol
            || o.y > tol
            || o.z > tol
            || x.x > 1.0 + tol
            || x.y > tol
            || x.z > tol
            || y.x > tol
            || y.y > 1.0 + tol
            || y.z > tol
            || o.x < -tol
            || o.y < -tol
            || o.z < -tol
            || x.x < 1.0 - tol
            || x.y < -tol
            || x.z < -tol
            || y.x < -tol
            || y.y < 1.0 - tol
            || y.z < -tol)
    }

    /// True when the Z axes of `self` and `other` are the same line within `tol3d`,
    /// checked at both origins and at `±size_scale` along each axis (`AreCoaxial`;
    /// upstream passes `tol3d` as an `SmApproxTol3d`, which stores `|tol3d|`; its
    /// `SM_USE_DEFAULT` substitution is not applied, there is no kernel here).
    #[must_use]
    pub fn are_coaxial(&self, other: &Self, tol3d: f64, size_scale: f64) -> bool {
        let tol = crate::geom::tol::tol3d(tol3d);
        let this_pt = self.origin;
        let this_vec = self.z_axis();
        let othr_pt = other.origin;
        let othr_vec = other.z_axis();

        let far = |line_pnt: Point3, line_vec: Vec3, pt: Point3| {
            line_point_distance(line_pnt, line_vec, pt).0 > tol
        };

        if far(this_pt, this_vec, othr_pt) || far(othr_pt, othr_vec, this_pt) {
            return false;
        }

        // Get a point on the axis that is dSizeScale away from origin.
        !(far(this_pt, this_vec, othr_pt + size_scale * othr_vec)
            || far(this_pt, this_vec, othr_pt - size_scale * othr_vec)
            || far(othr_pt, othr_vec, this_pt + size_scale * this_vec)
            || far(othr_pt, othr_vec, this_pt - size_scale * this_vec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SolidError;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-12
    }

    #[test]
    fn identity_and_init() {
        let mut p = Axis2Placement::new(Vec3::new(1.0, 2.0, 3.0), Vec3::Y, Vec3::Z);
        assert!(!p.is_identity(EFF_ZERO));
        p.init();
        assert!(p.is_identity(EFF_ZERO));
        assert_eq!(Axis2Placement::default(), Axis2Placement::IDENTITY);
        assert_eq!(p.z_axis(), Vec3::Z);
    }

    #[test]
    fn set_canonical_unitizes_and_orthogonalizes() {
        let mut p = Axis2Placement::IDENTITY;
        p.set_canonical(
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(1.0, 3.0, 0.0),
        )
        .unwrap();
        assert!(close(p.x_axis(), Vec3::X));
        assert!(close(p.y_axis(), Vec3::Y));
        assert!(close(p.z_axis(), Vec3::Z));
    }

    #[test]
    fn set_canonical_zero_axis_keeps_partial_state() {
        let mut p = Axis2Placement::IDENTITY;
        let o = Vec3::new(1.0, 1.0, 1.0);
        let r = p.set_canonical(o, Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0));
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        assert_eq!(p.canonical(), (o, Vec3::Z, Vec3::new(0.0, 2.0, 0.0)));
    }

    #[test]
    fn step_canonical() {
        let mut p = Axis2Placement::IDENTITY;
        p.set_step_canonical(Vec3::ZERO, Vec3::Z, Vec3::new(1.0, 1.0, 0.5))
            .unwrap();
        let s = core::f64::consts::FRAC_1_SQRT_2;
        assert!(close(p.x_axis(), Vec3::new(s, s, 0.0)));
        assert!(close(p.z_axis(), Vec3::Z));
        assert!(p.set_step_canonical(Vec3::X, Vec3::ZERO, Vec3::X).is_err());
        assert!(close(p.origin(), Vec3::ZERO));
    }

    #[test]
    fn plane_constructor() {
        let p = Axis2Placement::from_plane(Vec3::new(0.0, 0.0, 2.0), -4.0);
        assert!(close(p.origin(), Vec3::new(0.0, 0.0, 2.0)));
        assert!(close(p.z_axis(), Vec3::Z));
        let bad = Axis2Placement::from_plane(Vec3::ZERO, 1.0);
        assert!(bad.x_axis().is_undef());
    }

    #[test]
    fn rotate_and_transform() {
        let mut p = Axis2Placement::IDENTITY;
        p.rotate_about_axis(core::f64::consts::FRAC_PI_2, Vec3::Z);
        assert!(close(p.transform_point(Vec3::X), Vec3::Y));
        assert!(close(p.transform_vector(Vec3::Y), -Vec3::X));
        p.translate(Vec3::new(1.0, 0.0, 0.0));
        let q = p.transform_axis2_placement(&p.invert());
        assert!(q.is_identity(1e-12));
        let inv = p.invert();
        assert!(close(
            inv.transform_point(p.transform_point(Vec3::new(3.0, 4.0, 5.0))),
            Vec3::new(3.0, 4.0, 5.0)
        ));
    }

    #[test]
    fn rotate_at_point_keeps_center() {
        let mut p = Axis2Placement::IDENTITY;
        let c = Vec3::new(-2.0, 0.0, 0.0);
        p.rotate_about_axis_at_point(0.7, c, Vec3::new(1.0, 1.0, 1.0));
        assert!(close(p.transform_point(c), c));
        // A zero axis rotates about X.
        let mut q = Axis2Placement::IDENTITY;
        q.rotate_about_axis(0.3, Vec3::ZERO);
        let mut r = Axis2Placement::IDENTITY;
        r.rotate_about_axis(0.3, Vec3::X);
        assert_eq!(q, r);
    }

    #[test]
    fn from_to_maps_frames() {
        let p = Axis2Placement::from_to(
            &Axis2Placement::new(Vec3::new(1.0, 0.0, 0.0), Vec3::X, Vec3::Y),
            &Axis2Placement::new(Vec3::new(0.0, 5.0, 0.0), Vec3::Y, -Vec3::X),
        );
        assert!(close(
            p.transform_point(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(0.0, 5.0, 0.0)
        ));
        assert!(close(
            p.transform_point(Vec3::new(2.0, 0.0, 0.0)),
            Vec3::new(0.0, 6.0, 0.0)
        ));
    }

    #[test]
    fn matrices() {
        let mut p = Axis2Placement::new(Vec3::new(0.0, 0.0, 3.0), Vec3::X, Vec3::Y);
        let m = p.matrix();
        assert_eq!(m[2][3], 3.0);
        assert_eq!(m[3], [0.0, 0.0, 0.0, 1.0]);
        // Mirror about the plane z = 3 maps z -> 6 - z.
        let mm = p.mirror_matrix();
        assert!((mm[2][2] + 1.0).abs() < 1e-15);
        assert!((mm[2][3] - 6.0).abs() < 1e-15);
        p.rotate_about_axis(0.4, Vec3::new(1.0, 2.0, 3.0));
        let mm = p.mirror_matrix();
        let mm2 = mult_matrix(&mm, &mm);
        for (i, row) in mm2.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                let e = if i == j { 1.0 } else { 0.0 };
                assert!((v - e).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn approx_eq_and_coaxial() {
        let p = Axis2Placement::new(Vec3::new(1.0, 2.0, 3.0), Vec3::X, Vec3::Y);
        let mut q = p;
        assert!(p.approx_eq(&q));
        q.translate(Vec3::new(1e-14, 0.0, 0.0));
        assert!(p.approx_eq(&q));
        q.translate(Vec3::new(1e-6, 0.0, 0.0));
        assert!(!p.approx_eq(&q));
        let nan = Axis2Placement::new(Vec3::new(f64::NAN, 0.0, 0.0), Vec3::X, Vec3::Y);
        assert!(nan.approx_eq(&nan));
        let nan2 = nan;
        assert!(!nan.approx_eq(&nan2));
        assert!(!p.are_coaxial(&q, 1e-7, 1.0));
        assert!(p.are_coaxial(&q, 1e-5, 1.0));
        let r = Axis2Placement::new(Vec3::new(1.0, 2.0, 9.0), Vec3::Y, Vec3::X);
        assert!(p.are_coaxial(&r, 1e-9, 10.0));
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    #[test]
    fn mirror_point_and_vector_reflect_in_the_xy_plane() {
        // the plane z = 2
        let p = Axis2Placement::new(Vec3::new(0.0, 0.0, 2.0), Vec3::X, Vec3::Y);
        assert_eq!(
            p.mirror_point(Vec3::new(1.0, 5.0, 3.0)),
            Vec3::new(1.0, 5.0, 1.0)
        );
        assert_eq!(
            p.mirror_vector(Vec3::new(1.0, 5.0, 3.0)),
            Vec3::new(1.0, 5.0, -3.0)
        );
        // agrees with the mirror matrix
        let m = p.mirror_matrix();
        let q = Vec3::new(0.5, -1.5, 7.0);
        let by_matrix = Vec3::new(
            m[0][0] * q.x + m[0][1] * q.y + m[0][2] * q.z + m[0][3],
            m[1][0] * q.x + m[1][1] * q.y + m[1][2] * q.z + m[1][3],
            m[2][0] * q.x + m[2][1] * q.y + m[2][2] * q.z + m[2][3],
        );
        assert!((p.mirror_point(q) - by_matrix).length() < 1e-12);
    }
}
