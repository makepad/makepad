//! The B-spline volume `SmBSplineVolume`: a trivariate NURBS map from parameter space
//! to projection space, the [`VolumeKind::BSpline`] simple map of a
//! [`Volume`](crate::surface::volume::Volume).
//!
//! Upstream: `source/SMLib/src/SmBSplineVolume.cpp`, `source/SMLib/inc/SmBSplineVolume.h`.
//!
//! | Rust                                                  | Upstream                                         |
//! |-------------------------------------------------------|--------------------------------------------------|
//! | [`BSplineVolume`]                                     | `SmBSplineVolume` (members `m_pNurb`, `m_bOutOfBoundsEnabled`) |
//! | [`BSplineVolume::new`]                                | `SmBSplineVolume(const SmContext*, const gw_VOLUME*)`, `SmBSplineVolume(gw_VOLUME*, FALSE, ctx)` |
//! | [`BSplineVolume::copy`], `Volume::copy`               | `SmBSplineVolume(const SmBSplineVolume&, SmBoolean)`, `Copy` |
//! | `Drop` (implicit)                                     | `~SmBSplineVolume`                               |
//! | [`BSplineVolume::nurb`]                               | `GetGwNurbPointer`, `GetOrCreateGwNurbPointer`, `smGet_gw_VOLUME` |
//! | [`BSplineVolume::out_of_bounds_enabled`]              | `GetOutOfBoundsEnabled`                          |
//! | [`are_equal_nurb_volumes`]                            | `AreEqual_gw_VOLUME`                             |
//! | [`BSplineVolume::approx_eq`], `Volume::approx_eq`     | `operator==`                                     |
//! | [`load_nlib_transform`]                               | `sm_LoadNLibTransform`                           |
//! | [`BSplineVolume::degree`]                             | `GetDegree`                                      |
//! | [`BSplineVolume::natural_param_domain`]               | `GetNaturalParamDomain`                          |
//! | [`BSplineVolume::knots`]                              | `GetKnots`                                       |
//! | [`BSplineVolume::evaluate_simple`]                    | `EvaluateSimple`                                 |
//! | [`BSplineVolume::evaluate_bounding_box_simple`]       | `EvaluateBoundingBoxSimple`                      |
//! | [`BSplineVolume::evaluate_iso_parametric_curve_simple`] | `EvaluateIsoParametricCurveSimple`             |
//! | [`BSplineVolume::inv_evaluate_guess_point_simple`]    | `InvEvaluateGuessPointSimple`                    |
//! | [`BSplineVolume::is_bounded_simple`]                  | `IsBoundedSimple`                                |
//! | [`BSplineVolume::transform_simple`]                   | `TransformSimple`                                |
//! | [`BSplineVolume::read_from_db`]                       | `ReadFromDB`                                     |
//! | `Volume::type_code`, `type_string`, `class_string`, `is_kind_of` | `GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` (`SM_COMMON`) |
//! | [`VolumeParam`], [`VolumeParams`]                     | `SmVolumeParamType`, `SmVolumeParamsType`        |
//!
//! Deviations:
//! - **`USE_VOLUMES`.** Upstream builds the NLib volume code only with `USE_VOLUMES`,
//!   which the SMLib build does not define. The port follows the build: the
//!   `#else` branches of `AreEqual_gw_VOLUME` (`FALSE`), `sm_LoadNLibTransform`,
//!   `EvaluateSimple`, `EvaluateIsoParametricCurveSimple` and `ReadFromDB` (all
//!   `SM_ERR`) are what is ported. So two distinct B-spline volumes never compare
//!   equal, a B-spline volume cannot be evaluated, and `TransformSimple` builds its
//!   transform and then fails without touching the control points.
//! - **The NURBS record.** `m_pNurb` is a [`NurbVolume`] held by value; it is never
//!   `NULL` (the empty constructor, which leaves it `NULL`, is used only by
//!   `ReadFromDB`, which fails) and never borrowed, so `m_bNurbIsBorrowed` is
//!   dropped. The constructors taking a `gw_VOLUME` (by copy or by ownership) are
//!   [`BSplineVolume::new`]; like the copy constructor they run
//!   `sm_FixupKnotVector(.., SM_EFF_ZERO_PARAM)` on the three knot vectors.
//! - **Parameter enums.** `SmVolumeParamType` is [`VolumeParam`]; values other than
//!   `U`, `V`, `W` and `All` take upstream's fallback branch (`GetDegree`: the
//!   largest degree, after an error message; `GetKnots`: the `w` knots).
//! - **Out-params** become return values. `EvaluateBoundingBoxSimple` keeps its
//!   optional outputs as `Option<&mut _>` (a requested pseudo box is rebuilt, the
//!   axis-aligned box re-initialised); the parameter pseudo box only matters
//!   upstream when it aliases the output, which Rust excludes, so it is unused.
//! - **Inconsistent records.** Upstream indexes knots by control point index
//!   (`InvEvaluateGuessPointSimple`) without a check; a record whose knot vectors
//!   are shorter than control point count plus degree returns
//!   [`SolidError::InvalidInput`].
//! - **`ReadFromDB`** takes no stream (it reads nothing) and returns `SM_ERR`.
//!   Contexts and notifications are dropped (`sm_LoadNLibTransform` notifies only in
//!   the `USE_VOLUMES` branch).

use crate::curve::curve::p2::Curve;
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::transform::Transform;
use crate::geom::vector3d::{Point3, Vec3};
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQ};
use crate::nurbs::knots::KnotVector;
use crate::nurbs::math::CPoint;
use crate::nurbs::nurbs_crv::{fixup_knot_vector, get_knots};
use crate::nurbs::spiral::Volume as NurbVolume;
use crate::surface::volume::p1::EvalOptions;
#[cfg(doc)]
use crate::surface::volume::VolumeKind;

const OP_EVAL: &str = "surface::b_spline_volume::BSplineVolume::evaluate_simple";
const OP_BOX: &str = "surface::b_spline_volume::BSplineVolume::evaluate_bounding_box_simple";
const OP_ISO: &str =
    "surface::b_spline_volume::BSplineVolume::evaluate_iso_parametric_curve_simple";
const OP_GUESS: &str = "surface::b_spline_volume::BSplineVolume::inv_evaluate_guess_point_simple";
const OP_READ: &str = "surface::b_spline_volume::BSplineVolume::read_from_db";
const OP_LOAD: &str = "surface::b_spline_volume::load_nlib_transform";

/// A parameter direction of a volume (`SmVolumeParamType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeParam {
    /// `SM_VP_X_IN`.
    XIn,
    /// `SM_VP_Y_IN`.
    YIn,
    /// `SM_VP_Z_IN`.
    ZIn,
    /// `SM_VP_U`.
    U,
    /// `SM_VP_V`.
    V,
    /// `SM_VP_W`.
    W,
    /// `SM_VP_UNKNOWN`.
    Unknown,
    /// `SM_VP_ALL`.
    All,
    /// `SM_VP_NONE`.
    None,
}

impl VolumeParam {
    /// Every value, in upstream's enum order (`code` = index).
    pub const ALL_VALUES: [Self; 9] = [
        Self::XIn,
        Self::YIn,
        Self::ZIn,
        Self::U,
        Self::V,
        Self::W,
        Self::Unknown,
        Self::All,
        Self::None,
    ];

    /// The value with upstream enum code `code`.
    #[must_use]
    pub fn from_code(code: usize) -> Option<Self> {
        Self::ALL_VALUES.get(code).copied()
    }
}

/// A pair of constant parameters of an iso-parametric volume curve
/// (`SmVolumeParamsType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeParams {
    /// `SM_VPS_XY_IN`: constant `x`, `y` in-space; `z` varies.
    XyIn,
    /// `SM_VPS_XZ_IN`.
    XzIn,
    /// `SM_VPS_YZ_IN`.
    YzIn,
    /// `SM_VPS_UV`: constant `u`, `v`; `w` varies.
    Uv,
    /// `SM_VPS_UW`.
    Uw,
    /// `SM_VPS_VW`.
    Vw,
    /// `SM_VPS_UNKNOWN`.
    Unknown,
    /// `SM_VPS_ALL`.
    All,
    /// `SM_VPS_NONE`.
    None,
}

/// The B-spline simple map of a volume (`SmBSplineVolume`'s own members).
#[derive(Debug, Clone, PartialEq)]
pub struct BSplineVolume {
    /// `m_pNurb`: the NURBS record.
    nurb: NurbVolume,
    /// `m_bOutOfBoundsEnabled`.
    out_of_bounds_enabled: bool,
}

/// Runs `sm_FixupKnotVector(.., SM_EFF_ZERO_PARAM)` on the three knot vectors.
fn fixup_knots(nurb: &mut NurbVolume) {
    // Edit knot values: close knots (knotSpacing<ScaledZero) become same and
    //                   near knots (ScaledZero<KnotSpacing<sTol2d) are seperated by more than sTol2d.
    for dir in 0..3 {
        fixup_knot_vector(nurb.knots_mut(dir), EFF_ZERO_PARAM);
    }
}

/// Equivalence of two NURBS volume records (`AreEqual_gw_VOLUME`): always `false`
/// in upstream's build (`N_VolumesAreEqual` needs `USE_VOLUMES`).
#[must_use]
pub const fn are_equal_nurb_volumes(
    vol1: &NurbVolume,
    vol2: &NurbVolume,
    tol3d: f64,
    tol1d: f64,
) -> bool {
    let _ = (vol1, vol2, tol3d, tol1d);
    false
}

/// Applies `t` to the control points of `volume` (`sm_LoadNLibTransform`): `SM_ERR`
/// without touching them in upstream's build (`N_VolumeTransform` needs
/// `USE_VOLUMES`).
///
/// # Errors
///
/// Always `SM_ERR`.
pub fn load_nlib_transform(t: &Transform, volume: &mut BSplineVolume) -> Result<()> {
    let _ = (t, volume);
    Err(SolidError::sm_err(OP_LOAD))
}

impl BSplineVolume {
    /// A B-spline volume over a copy of `nurb` with its knot vectors fixed up
    /// (`SmBSplineVolume(const SmContext*, const gw_VOLUME*)`).
    #[must_use]
    pub fn new(nurb: NurbVolume) -> Self {
        let mut nurb = nurb;
        fixup_knots(&mut nurb);
        Self {
            nurb,
            out_of_bounds_enabled: false,
        }
    }

    /// The copy constructor's own part (`SmBSplineVolume(const SmBSplineVolume&,
    /// SmBoolean)`): a copy of the record with its knot vectors fixed up again, and
    /// the out-of-bounds flag.
    #[must_use]
    pub fn copy(&self) -> Self {
        // fetch the source NurbVolume, copy and store m_pNurb
        let mut nurb = self.nurb.clone();
        fixup_knots(&mut nurb);
        Self {
            nurb,
            out_of_bounds_enabled: self.out_of_bounds_enabled,
        }
    }

    /// The NURBS record (`GetGwNurbPointer`, `GetOrCreateGwNurbPointer`,
    /// `smGet_gw_VOLUME`).
    #[must_use]
    pub const fn nurb(&self) -> &NurbVolume {
        &self.nurb
    }

    /// `m_bOutOfBoundsEnabled` (`GetOutOfBoundsEnabled`).
    #[must_use]
    pub const fn out_of_bounds_enabled(&self) -> bool {
        self.out_of_bounds_enabled
    }

    /// Sets `m_bOutOfBoundsEnabled` (through `GetOutOfBoundsEnabled`'s reference).
    pub fn set_out_of_bounds_enabled(&mut self, enabled: bool) {
        self.out_of_bounds_enabled = enabled;
    }

    /// The derived part of `operator==`, run after `SmVolume::operator==` passed on
    /// two distinct volumes: the records are distinct and never `NULL`, so the
    /// result is [`are_equal_nurb_volumes`] (`false`).
    #[must_use]
    pub const fn approx_eq(&self, other: &Self) -> bool {
        are_equal_nurb_volumes(&self.nurb, &other.nurb, EFF_ZERO, EFF_ZERO)
    }

    /// The degree in direction `param`; the largest degree for
    /// [`VolumeParam::All`] and (after upstream's error message) any other
    /// direction (`GetDegree`).
    #[must_use]
    pub fn degree(&self, param: VolumeParam) -> usize {
        let [p, q, r] = self.nurb.degrees();
        match param {
            VolumeParam::U => p,
            VolumeParam::V => q,
            VolumeParam::W => r,
            _ => p.max(q).max(r),
        }
    }

    /// The parameter box spanned by the first and last knots
    /// (`GetNaturalParamDomain`).
    #[must_use]
    pub fn natural_param_domain(&self) -> Extent3d {
        let ends = |dir: usize| {
            let k = self.nurb.knots(dir);
            (
                k.first().copied().unwrap_or(0.0),
                k.last().copied().unwrap_or(0.0),
            )
        };
        let (u0, u1) = ends(0);
        let (v0, v1) = ends(1);
        let (w0, w1) = ends(2);
        // return bounded volume min/max knot values
        Extent3d::from_points(Vec3::new(u0, v0, w0), Vec3::new(u1, v1, w1))
    }

    /// The distinct knots of direction `param` (`U`, `V`, else `W`) and their
    /// multiplicities, limited to `ivl` when given (`GetKnots`, through
    /// `sm_GetKnots`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an empty knot vector.
    pub fn knots(
        &self,
        param: VolumeParam,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        let dir = match param {
            VolumeParam::U => 0,
            VolumeParam::V => 1,
            _ => 2,
        };
        // pass the call along
        let kv = KnotVector::from_slice(self.nurb.knots(dir))?;
        Ok(get_knots(&kv, ivl))
    }

    /// Position and partial derivatives of the parameter point (`EvaluateSimple`):
    /// `SM_ERR` in upstream's build.
    ///
    /// # Errors
    ///
    /// Always `SM_ERR`.
    pub fn evaluate_simple(
        &self,
        param_point: Point3,
        n: usize,
        opts: &EvalOptions,
    ) -> Result<Vec<Vec3>> {
        let _ = (param_point, n, opts);
        Err(SolidError::sm_err(OP_EVAL))
    }

    /// Projection-space boxes of the volume (`EvaluateBoundingBoxSimple`): the
    /// axis-aligned box `proj_box` and the pseudo box `proj_pseudo_box` (based on
    /// the corner control points' u and v directions, else the coordinate axes) of
    /// all control points. `param_box` must lie in the natural domain (within
    /// `SM_EFF_ZERO`) but does not shrink the boxes; `param_pseudo_box` is unused.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] (upstream `SM_ERR_INVALID_INPUT`, outputs
    /// untouched) when `param_box` is not in the natural domain.
    pub fn evaluate_bounding_box_simple(
        &self,
        param_box: &Extent3d,
        param_pseudo_box: Option<&PseudoBox>,
        mut proj_box: Option<&mut Extent3d>,
        mut proj_pseudo_box: Option<&mut PseudoBox>,
    ) -> Result<()> {
        let _ = param_pseudo_box;
        // no work - no output
        if proj_box.is_none() && proj_pseudo_box.is_none() {
            return Ok(());
        }

        let natural_uvw_domain = self.natural_param_domain();
        let [m, n, o] = self.nurb.cp_hi();

        // First make sure interval sent in is valid
        if !param_box.is_contained_by(&natural_uvw_domain, EFF_ZERO) {
            return Err(SolidError::InvalidInput {
                operation: OP_BOX,
                reason: "parameter box outside the natural domain",
            });
        }

        // initialize pProjBox
        if let Some(b) = proj_box.as_deref_mut() {
            b.init();
        }

        // Compute the basis vectors of the PseudoBox using the control polygon
        if let Some(pb) = proj_pseudo_box.as_deref_mut() {
            let cp = |i: usize, j: usize| {
                self.nurb
                    .cpt(i, j, 0)
                    .map(to_euclid)
                    .ok_or(SolidError::InvalidInput {
                        operation: OP_BOX,
                        reason: "control point outside the mesh",
                    })
            };
            let p000 = cp(0, 0)?;
            let p100 = cp(m, 0)?;
            let p010 = cp(0, n)?;
            let p110 = cp(m, n)?;

            let v1 = (p000 - p100) + (p010 - p110);
            let v2 = (p000 - p010) + (p100 - p110);
            let v1_ls = v1.length_squared();
            if v1_ls > EFF_ZERO_SQ {
                // cannot fail: v1 is not zero, so one of the fallback axes works
                let [b1, b2, b3] = v1.make_unit_ortho_vectors(Some(v2))?;
                // Initialize it; upstream ignores the SetBasis status (an
                // orthonormal basis passes)
                *pb = PseudoBox::new();
                let _ = pb.set_basis(b1, b2, b3);
            } else {
                // initialize it
                pb.init_basis(); // set basis vectors to unit orthogaonal
                pb.init(); // init intervals to [SM_BIG_DOUBLE -SM_BIG_DOUBLE]
            }
        }

        // iter over the BSpline VolumeMesh ControlPoints
        for i in 0..=m {
            for j in 0..=n {
                for k in 0..=o {
                    let Some(c) = self.nurb.cpt(i, j, k) else {
                        continue;
                    };
                    let point = to_euclid(c);
                    if let Some(b) = proj_box.as_deref_mut() {
                        b.add_point(point);
                    }
                    if let Some(pb) = proj_pseudo_box.as_deref_mut() {
                        pb.add_point(point);
                    }
                }
            }
        }
        Ok(())
    }

    /// The projection-space iso-parametric curve with the two parameters of
    /// `constant_params` fixed (`EvaluateIsoParametricCurveSimple`): `SM_ERR` in
    /// upstream's build.
    ///
    /// # Errors
    ///
    /// Always `SM_ERR`.
    pub fn evaluate_iso_parametric_curve_simple(
        &self,
        constant_params: VolumeParams,
        iso_param1: f64,
        iso_param2: f64,
        tol3d: f64,
        param_domain: Option<&Extent3d>,
    ) -> Result<Curve> {
        let _ = (constant_params, iso_param1, iso_param2, tol3d, param_domain);
        Err(SolidError::sm_err(OP_ISO))
    }

    /// A Newton start point for inverting `proj_point` (`InvEvaluateGuessPointSimple`):
    /// the Greville-style average of the knots of the control point closest to
    /// `proj_point` (the first one on ties).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a knot vector is too short for the control
    /// mesh (upstream reads past its end).
    pub fn inv_evaluate_guess_point_simple(&self, proj_point: Point3) -> Result<Vec<Point3>> {
        let [m, n, o] = self.nurb.cp_hi();
        let (mut i_min, mut j_min, mut k_min) = (0usize, 0usize, 0usize);
        let mut min_dist2 = BIG_DOUBLE;

        // find closest ControlPoint to crProjPoint
        for i in 0..=m {
            for j in 0..=n {
                for k in 0..=o {
                    let Some(c) = self.nurb.cpt(i, j, k) else {
                        continue;
                    };
                    let point = c.to_euclid();
                    // get CPoint/TargetPoint distance
                    let this_dist2 = proj_point.distance_squared(point);
                    // save closest point
                    if this_dist2 < min_dist2 {
                        min_dist2 = this_dist2;
                        i_min = i;
                        j_min = j;
                        k_min = k;
                    }
                }
            }
        }

        // get u, v, w guesses
        let [p, q, r] = self.nurb.degrees();
        let avg = |dir: usize, start: usize, deg: usize| -> Result<f64> {
            let knots = self.nurb.knots(dir);
            let mut sum = 0.0;
            for i in 0..=deg {
                sum += start
                    .checked_add(i)
                    .and_then(|x| knots.get(x))
                    .copied()
                    .ok_or(SolidError::InvalidInput {
                        operation: OP_GUESS,
                        reason: "knot vector shorter than the control mesh",
                    })?;
            }
            Ok(sum / (deg as f64 + 1.0))
        };
        let u = avg(0, i_min, p)?;
        let v = avg(1, j_min, q)?;
        let w = avg(2, k_min, r)?;
        Ok(vec![Vec3::new(u, v, w)])
    }

    /// Whether the parameter space is bounded (`IsBoundedSimple`): always.
    #[must_use]
    pub const fn is_bounded_simple(&self) -> bool {
        true
    }

    /// Builds the out-space motion and scale into the control points
    /// (`TransformSimple`): the transform `InvOrient(Transform(Orient()))` is built
    /// from the volume's orient maps and handed to [`load_nlib_transform`], which
    /// fails in upstream's build.
    ///
    /// # Errors
    ///
    /// `SM_ERR` from [`load_nlib_transform`].
    pub fn transform_simple(
        &mut self,
        orient_map: Option<&Transform>,
        inv_orient_map: Option<&Transform>,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        // build InvOrient(Transform(Orient())) transform SMLib style
        let mut t = Transform::IDENTITY; // identity mapping with no OrientMap
                                         // Orient
        if let Some(o) = orient_map {
            t.concat_transform(o);
        }
        // Transform
        t.transform_simple(rotate_n_move, scale);
        // InvOrient
        if let Some(inv) = inv_orient_map {
            t.concat_transform(inv);
        }
        // apply the transformation to the volume
        load_nlib_transform(&t, self)
    }

    /// Builds the out-space mirror about the plane through `plane_pt` with normal
    /// `plane_normal` into the control points (`MirrorSimple`): the transform
    /// `InvOrient(Mirror(Orient()))` is built from the volume's orient maps and handed
    /// to [`load_nlib_transform`], which fails in upstream's build.
    ///
    /// # Errors
    ///
    /// `SM_ERR` from [`load_nlib_transform`].
    pub fn mirror_simple(
        &mut self,
        orient_map: Option<&Transform>,
        inv_orient_map: Option<&Transform>,
        plane_pt: Point3,
        plane_normal: Vec3,
    ) -> Result<()> {
        // build InvOrient(Mirror(Orient())) transform SMLib style
        let mut t = Transform::IDENTITY; // identity mapping with no OrientMap
                                         // Orient
        if let Some(o) = orient_map {
            t.concat_transform(o);
        }
        // Mirror (its status is ignored upstream)
        ignore_status(t.mirror_simple(plane_pt, plane_normal))?;
        // InvOrient
        if let Some(inv) = inv_orient_map {
            t.concat_transform(inv);
        }
        // apply the transformation to the volume
        load_nlib_transform(&t, self)
    }

    /// Reads a B-spline volume from a database stream (`ReadFromDB`): `SM_ERR` in
    /// upstream's build, before anything is read.
    ///
    /// # Errors
    ///
    /// Always `SM_ERR`.
    pub fn read_from_db(version: u64) -> Result<Self> {
        let _ = version;
        Err(SolidError::sm_err(OP_READ))
    }
}

/// `TO_EUCLID` of a control point (divides by `w`; a missing `z` becomes `0`).
fn to_euclid(c: CPoint) -> Point3 {
    crate::nurbs::nurbs_crv::to_euclid(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::spiral::allocate_nurb_volume;

    /// A 2x3x2 control mesh, degrees (1, 2, 1), clamped knots.
    fn sample() -> NurbVolume {
        let mut v = allocate_nurb_volume([1, 2, 1], [1, 2, 1], [3, 5, 3]).unwrap();
        v.knots_mut(0).copy_from_slice(&[0., 0., 1., 1.]);
        v.knots_mut(1).copy_from_slice(&[0., 0., 0., 2., 2., 2.]);
        v.knots_mut(2).copy_from_slice(&[-1., -1., 3., 3.]);
        for i in 0..=1 {
            for j in 0..=2 {
                for k in 0..=1 {
                    *v.cpt_mut(i, j, k).unwrap() =
                        CPoint::new(i as f64, j as f64 * 0.5, k as f64 + 0.25 * j as f64, 1.0);
                }
            }
        }
        v
    }

    #[test]
    fn accessors() {
        let b = BSplineVolume::new(sample());
        assert_eq!(b.degree(VolumeParam::U), 1);
        assert_eq!(b.degree(VolumeParam::V), 2);
        assert_eq!(b.degree(VolumeParam::W), 1);
        assert_eq!(b.degree(VolumeParam::All), 2);
        assert_eq!(b.degree(VolumeParam::XIn), 2);
        let d = b.natural_param_domain();
        assert_eq!(d.u_min(), 0.0);
        assert_eq!(d.v_max(), 2.0);
        assert_eq!(d.w_min(), -1.0);
        let (k, m) = b.knots(VolumeParam::V, None).unwrap();
        assert_eq!(k, vec![0., 2.]);
        assert_eq!(m, vec![3, 3]);
        let (k, _) = b.knots(VolumeParam::None, None).unwrap();
        assert_eq!(k, vec![-1., 3.]);
        assert!(b.is_bounded_simple());
        assert!(!b.approx_eq(&b.copy()));
    }

    #[test]
    fn failing_upstream_paths() {
        let mut b = BSplineVolume::new(sample());
        assert!(b
            .evaluate_simple(Vec3::new(0., 0., 0.), 0, &EvalOptions::default())
            .is_err());
        assert!(b
            .evaluate_iso_parametric_curve_simple(VolumeParams::Uv, 0., 0., 1e-3, None)
            .is_err());
        assert!(BSplineVolume::read_from_db(1).is_err());
        let before = b.clone();
        let a = Axis2Placement::default();
        assert!(b.transform_simple(None, None, &a, None).is_err());
        assert_eq!(b, before);
    }

    #[test]
    fn guess_and_boxes() {
        let b = BSplineVolume::new(sample());
        let g = b
            .inv_evaluate_guess_point_simple(Vec3::new(1., 1., 1.5))
            .unwrap();
        // closest control point (1, 2, 1): u knots 0+1 / 2, v 0+2+2 / 3, w -1+3 / 2...
        assert_eq!(g.len(), 1);
        let mut e = Extent3d::new();
        let mut pb = PseudoBox::new();
        b.evaluate_bounding_box_simple(
            &b.natural_param_domain(),
            None,
            Some(&mut e),
            Some(&mut pb),
        )
        .unwrap();
        assert_eq!(e.u_max(), 1.0);
        assert_eq!(e.w_max(), 1.5);
        let outside = Extent3d::from_points(Vec3::new(-1., 0., 0.), Vec3::new(0., 0., 0.));
        assert!(b
            .evaluate_bounding_box_simple(&outside, None, Some(&mut e), None)
            .is_err());
        assert!(b
            .evaluate_bounding_box_simple(&outside, None, None, None)
            .is_ok());
    }

    #[test]
    fn copy_fixes_knots() {
        let mut v = sample();
        v.knots_mut(0).copy_from_slice(&[0., 1e-13, 1., 1.]);
        let b = BSplineVolume::new(v);
        assert_eq!(b.nurb().knots(0)[1], 0.0);
        let c = b.copy();
        assert_eq!(c, b);
        assert_eq!(VolumeParam::from_code(3), Some(VolumeParam::U));
        assert_eq!(VolumeParam::from_code(9), None);
    }
}
