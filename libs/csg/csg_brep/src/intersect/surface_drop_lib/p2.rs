//! `SmSurfaceDropLib`, part 2: the drop point steps of `DropCurve`
//! ([`DropPt::set_properties`], [`DropPt::get_next_point`]) and the growing UV curve
//! of the dropped spans ([`add_to_curve`], [`flush_curve`]).
//!
//! Upstream: `source/SMLib/src/SmSurfaceDropLib.cpp`.
//!
//! | Rust                                  | Upstream                                  |
//! |---------------------------------------|-------------------------------------------|
//! | [`DropPt::set_properties`]            | `SmDropPt::SetProperties`                 |
//! | [`DropPt::get_next_point`]            | `SmDropPt::GetNextPoint`                  |
//! | [`flush_curve`]                       | `sm_FlushCrv` (static)                    |
//! | [`add_to_curve`]                      | `sm_AddToCurve` (static)                  |
//! | [`DropOffset`]                        | the `SmOffsetSurface` of `DropCurve` (`SmOffsetSurface(1.0, *this, FALSE, lSingularities)`) these steps take |
//! | [`SS_NONE`] .. [`SS_UNKNOWN`]         | `SM_SS_NONE` .. `SM_SS_UNKNOWN` (`SmTypes.h`) |
//!
//! Deviations:
//! - **Statuses.** The statuses these functions return by design are `Ok` codes
//!   ([`SM_SUCCESS`], [`SM_ERR_NOT_WITHIN_TOLERANCE`], [`SM_ERR_BAD_TANGENT_DROP`],
//!   [`SM_ERR_OUTSIDE_OF_DOMAIN`], [`SM_ERR_BAD_SURFACE_POINT`] and, for
//!   `GetNextPoint`'s last try, `SM_ERR`); `SetProperties`' two `SM_ERR` returns for a
//!   zero or normal-parallel curve tangent and every propagated callee failure (`SER`)
//!   are errors. `GetNextPoint` stores the status of `SetProperties` (and goes on with
//!   its second try): an error there becomes its code, the `code` of a
//!   [`SolidError::Upstream`], `SM_ERR_INVALID_INPUT` for [`SolidError::InvalidInput`]
//!   and `SM_ERR` for any other variant (Rust errors do not carry every upstream
//!   code); [`SolidError::Unported`] always propagates.
//! - **Offset surface.** `DropCurve`'s `SmOffsetSurface` always offsets the drop
//!   surface itself, so it is a [`DropOffset`] (its offset distance and the
//!   singularities it was built with) next to that surface. `DropVectors` and
//!   `EvaluateDirectionalDerivs` run the `SmSurface` bodies on the ported
//!   [`OffsetSurface`] over an exact copy of that surface (upstream shares it), built
//!   at the first of them and kept with the [`DropOffset`], as upstream builds it
//!   once per `DropCurve`; [`DropOffset::set_offset_distance`] sets its distance.
//! - **Ignored statuses.** `GetNextPoint` ignores the status of its start
//!   `Evaluate2ndDerivatives` (upstream then goes on with what the failed evaluator
//!   left in its outputs): that failure path is not ported, the error propagates
//!   (the difftest shim rejects those inputs). `smgu_VecLinCombTwoVectors`' failure
//!   leaves the zero vector, `FindDegenParamForDirection`'s failures leave the UV
//!   point, `AngleBetween`'s failures the angle `0`, as upstream.
//! - **Owner.** `GetNextPoint`'s global solve nulls the surface's owner for the call
//!   (`SmTemporaryChangeValue`); here it runs on a copy, which has no owner.
//! - **Unused arguments** of `GetNextPoint` (`dUVTolX`, `dUVTolY`), `sm_FlushCrv`
//!   (`crContext`, `crSurface`, `cr3dCurve`) and `sm_AddToCurve` (`cr3dCurve`) are
//!   dropped. `rbFixBCChange` stays an in/out flag (only ever set,
//!   and kept set when the curve then cannot be made); the curves go to a
//!   `Vec<BSplineCurve>`.
//! - **Kernel.** `SmSurface::IsSingularity` and `SmSurface::LocalPointSolve` take the
//!   caller's [`Kernel`]; `sm_AddToCurve` takes it (for `IsSingularity`) in place of
//!   its unused `crContext`. `EvaluateGeometric`'s status is ignored, as upstream: a
//!   failed evaluation leaves the principal curvatures at their initial `0.0`. So is
//!   `SetProperties`' offset `EvaluateDirectionalDerivs` (it fails at a pole): the
//!   derivatives then stay `SM_UNDEF_DOUBLE` and the tangent drop tests bad.
//! - **Unported callees.** The offset surface evaluations (`surface.offset_surface`)
//!   return [`SolidError::Unported`].
//! - **Not ported here.** The `SM_DEBUG_CODE` blocks, the `SM_ASSERT`s and the
//!   messages (`MSG` at 2000 control points).

use crate::curve::curve::Curve;
use crate::error::{ignored, Result, SolidError, SM_ERR};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{is_dog_leg, line_point_distance, vec_lin_comb_two_vectors};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::SolutionArray;
use crate::geom::tol::scaled_zero2;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, max3, rad_to_deg, EFF_ZERO, EFF_ZERO_SQ, PI};
use crate::nurbs::b_spline_curve::p1::{BSplineCurveForm, CanonicalData, KnotType};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::knots::Side;
use crate::surface::offset_surface::OffsetSurface;
use crate::surface::surface::p1::{map_to_2d_dir, zone_tol_3d};
use crate::surface::surface::p2::SurfaceKind;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::Surface;
use crate::topology::ObjectHandle;

use super::p1::{fix_boundary_curves, DropPt, SM_ERR_OUTSIDE_OF_DOMAIN, SM_SUCCESS};

/// `SM_ERR_INVALID_INPUT` (`SmMessages.h`).
pub const SM_ERR_INVALID_INPUT: i32 = 1007;

/// `SM_ERR_NOT_WITHIN_TOLERANCE` (`SmMessages.h`): the curve point is not on the
/// surface normal line at its drop point.
pub const SM_ERR_NOT_WITHIN_TOLERANCE: i32 = 1021;

/// `SM_ERR_BAD_SURFACE_POINT` (`SmMessages.h`): the span start is outside the domain.
pub const SM_ERR_BAD_SURFACE_POINT: i32 = 1030;

/// `SM_ERR_BAD_TANGENT_DROP` (`SmMessages.h`): the curve tangent dropped to a surface
/// tangent that is not parallel to it.
pub const SM_ERR_BAD_TANGENT_DROP: i32 = 1032;

/// `SM_SS_NONE`: the surface has no singularities.
pub const SS_NONE: u32 = 0;
/// `SM_SS_UMIN`: singular on the `u = UMin` boundary.
pub const SS_UMIN: u32 = 1;
/// `SM_SS_VMIN`: singular on the `v = VMin` boundary.
pub const SS_VMIN: u32 = 2;
/// `SM_SS_UMAX`: singular on the `u = UMax` boundary.
pub const SS_UMAX: u32 = 4;
/// `SM_SS_VMAX`: singular on the `v = VMax` boundary.
pub const SS_VMAX: u32 = 8;
/// `SM_SS_UNKNOWN`: singularities not yet tested.
pub const SS_UNKNOWN: u32 = 16;

/// `sm_AddToCurve` fails once the control polygon reaches this many points.
pub const MAX_CTRL_POINTS: usize = 10_000;

const OP_SET: &str = "intersect::surface_drop_lib::DropPt::set_properties";
const OP_NEXT: &str = "intersect::surface_drop_lib::DropPt::get_next_point";
const OP_ADD: &str = "intersect::surface_drop_lib::add_to_curve";

/// The offset of the drop surface `DropCurve` hands to these steps (its
/// `SmOffsetSurface(1.0, *this, FALSE, lSingularities)`): the base is always the drop
/// surface itself, so the offset distance (which [`DropPt::set_properties`] sets), the
/// singularities it was built with and, once a step evaluates it, the offset surface
/// are kept. One `DropOffset` serves one drop surface, as upstream's serves its
/// `DropCurve`.
#[derive(Debug)]
pub struct DropOffset {
    /// `m_dOffsetDistance`.
    pub offset_distance: f64,
    /// The `lSingularities` the offset was built with.
    pub singularities: u32,
    /// The offset surface, built at its first evaluation (see [`DropOffset::offset_of`]).
    surface: Option<Surface>,
}

impl DropOffset {
    /// `SmOffsetSurface(dOffsetDistance, crSurface, FALSE, lSingularities)`.
    #[must_use]
    pub const fn new(offset_distance: f64, singularities: u32) -> Self {
        Self {
            offset_distance,
            singularities,
            surface: None,
        }
    }

    /// `SetOffsetDistance`.
    pub fn set_offset_distance(&mut self, offset_distance: f64) {
        // (the kept offset surface takes it at its next evaluation)
        self.offset_distance = offset_distance;
    }

    /// The `SmOffsetSurface` this stands for over `surface` (not owning it, with the
    /// stored singularities and offset distance), built at the first call and kept: its
    /// constructor's `IsPeriodic` (and, for [`SS_UNKNOWN`], `GetSingularities`) depend
    /// only on `surface`, which is the same drop surface at every call.
    ///
    /// Upstream's base is `surface` itself (`m_pSurface(&rSurface)`), so the stand-in is
    /// an exact evaluation copy that keeps its owner: [`Clone`] runs the copy
    /// constructor's knot fix-up, which moves near-coincident knots and changes the
    /// base's derivatives (bottle edge 29 at r=0.1: `DropVectors` at a 7e-10 first knot
    /// span).
    fn offset_of(&mut self, k: &Kernel, surface: &Surface) -> Result<&Surface> {
        let built = match self.surface.take() {
            Some(built) => built,
            None => Surface::from(OffsetSurface::new(
                k,
                self.offset_distance,
                surface.clone_for_evaluation()?,
                false,
                self.singularities,
            )?),
        };
        if let SurfaceKind::OffsetSurface(off) = built.kind() {
            off.set_offset_distance(self.offset_distance);
        }
        Ok(self.surface.insert(built))
    }

    /// `rOffset.DropVectors(uv, bFromLeftU, bFromLeftV, 1, &vector, &duv)` on the
    /// offset of `surface` (the `SmSurface` body: `SmOffsetSurface` does not override
    /// it).
    fn drop_vectors(
        offset: &mut Self,
        k: &Kernel,
        surface: &Surface,
        uv: Point2,
        from_left_u: bool,
        from_left_v: bool,
        vector: Vec3,
    ) -> Result<Vec2> {
        let off = offset.offset_of(k, surface)?;
        drop_vectors_at(k, off, uv, side(from_left_u), side(from_left_v), &[vector])?
            .first()
            .copied()
            .ok_or_else(|| SolidError::sm_err(OP_SET))
    }

    /// `rOffset.EvaluateDirectionalDerivs(uv, uv_dir, 1, pd, bFromLeftU, bFromLeftV)`
    /// on the offset of `surface`: the point and first directional derivative (the
    /// `SmSurface` body: `SmOffsetSurface` does not override it).
    fn evaluate_directional_derivs(
        offset: &mut Self,
        k: &Kernel,
        surface: &Surface,
        uv: Point2,
        uv_dir: Vec2,
        from_left_u: bool,
        from_left_v: bool,
    ) -> Result<[Point3; 2]> {
        let off = offset.offset_of(k, surface)?;
        let d = off.evaluate_directional_derivs(k, uv, uv_dir, 1, from_left_u, from_left_v)?;
        match d.as_slice() {
            [p, d1, ..] => Ok([*p, *d1]),
            _ => Err(SolidError::sm_err(OP_SET)),
        }
    }
}

/// `SM_DEG2RAD(a)` = `a * (SM_PI / 180.0)`.
fn deg_to_rad(a: f64) -> f64 {
    a * (PI / 180.0)
}

/// The upstream `bFromLeft` flag as a span side (`TRUE` is [`Side::Left`]).
const fn side(from_left: bool) -> Side {
    if from_left {
        Side::Left
    } else {
        Side::Right
    }
}

/// `SM_IS_BETWEEN(c, a, b)`: `c` strictly inside the open interval between `a` and
/// `b` shrunk by their scaled zero.
fn is_between(c: f64, a: f64, b: f64) -> bool {
    let z = scaled_zero2(&a, &b);
    ((a + z) < c && c < (b - z)) || ((b + z) < c && c < (a - z))
}

/// `crSurface.IsSingularity(uv, eSingularDirection, dTol3d)` (virtual: the plane
/// override, the base body otherwise; `bPtTestOnly` defaults to `FALSE`).
fn is_singularity(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    tol: f64,
) -> Result<(bool, SurfParamType)> {
    let r = surface.is_singularity(k, uv, tol, false)?;
    Ok((r.singular, r.direction))
}

/// `crSurface.IsSingularity(uv, lSingularities, pPolePoints, eSingularDirection,
/// dTol3d)` (virtual: the plane override, the base body otherwise).
fn is_singularity_with_poles(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    singularities: u32,
    pole_points: &[Point3],
    tol: f64,
) -> Result<(bool, SurfParamType)> {
    let r = surface.is_singularity_with_poles(
        k,
        uv,
        u64::from(singularities),
        Some(pole_points),
        tol,
        false,
    )?;
    Ok((r.singular, r.direction))
}

/// `crSurface.IsOnSeam(uv, &dTol3d)` (virtual: the plane override, never on a seam;
/// the base body otherwise).
fn is_on_seam(k: &Kernel, surface: &Surface, uv: Point2, tol: f64) -> Result<bool> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_on_seam().0),
        _ => Ok(surface.is_on_seam(k, uv, Some(tol))?.on_seam),
    }
}

/// `crSurface.LocalPointSolve(domain, SM_SO_MINIMIZE, point, guess, bFound, sol)`:
/// whether a solution was found and its UV point.
fn local_point_solve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    point: Point3,
    guess: Point2,
) -> Result<(bool, Point2)> {
    let r = surface.local_point_solve::<ObjectHandle>(
        k,
        uv_domain,
        SolverOperation::Minimize,
        point,
        guess,
    )?;
    let s = &r.solution.start;
    Ok((
        r.found,
        Point2::new(
            s.parameters.first().copied().unwrap_or(0.0),
            s.parameters.get(1).copied().unwrap_or(0.0),
        ),
    ))
}

/// `SmSurface::EvaluateGeometric(uv, bFromLeftU, bFromLeftV, ..)`: the two principal
/// curvatures. Upstream ignores the status; the caller's locals start at `0.0`, so a
/// failed evaluation gives `(0.0, 0.0)`.
fn evaluate_principal_curvatures(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    from_left_u: bool,
    from_left_v: bool,
) -> Result<(f64, f64)> {
    Ok(
        ignored(surface.evaluate_geometric(k, uv, side(from_left_u), side(from_left_v)))?
            .map_or((0.0, 0.0), |g| {
                (g.principle_curvature1, g.principle_curvature2)
            }),
    )
}

/// `crSurface.FindDegenParamForDirection(uv, dir, bDirFromPole, eDir, 0.0001)` with
/// its status ignored: the moved point, or `uv` when the call fails before moving it.
fn find_degen_param(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    dir: Vec3,
    dir_from_pole: bool,
    singular_dir: SurfParamType,
) -> Result<Point2> {
    match surface.find_degen_param_for_direction(k, uv, dir, dir_from_pole, singular_dir, 0.0001) {
        Ok(d) => Ok(d.uv),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(uv),
    }
}

/// The status upstream would store for a failed callee (see the module notes).
fn status_of(e: &SolidError) -> i32 {
    match e {
        SolidError::Upstream { code, .. } => *code,
        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. } => SM_ERR_INVALID_INPUT,
        _ => SM_ERR,
    }
}

impl DropPt {
    /// Sets every drop point property from `uv_pd[0]` (the surface drop point) and
    /// `crv_pd` (the curve point and derivatives) (`SetProperties`): the from-left
    /// flags, the surface point, normal and drop deviation, the boundary, seam and
    /// pole state, the UV tangent (the curve tangent dropped to the offset through
    /// the curve point, `offset`'s distance is set to that) and its quality. With
    /// `adjust_pole_pt`, a drop point on a pole first slides along the pole to match
    /// the curve tangent.
    ///
    /// Returns [`SM_SUCCESS`], [`SM_ERR_NOT_WITHIN_TOLERANCE`] (the curve point is not
    /// on the normal line within the tolerance) or [`SM_ERR_BAD_TANGENT_DROP`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a zero curve tangent or one within 3 degrees of the normal
    /// (`good_drop` is cleared when it was set); those of the surface evaluations, the
    /// zone tolerance and `IsOnSeam`; [`SolidError::Unported`] (see the module notes).
    pub fn set_properties(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        offset: &mut DropOffset,
        singularities: u32,
        approx_tol_3d: f64,
        uv_domain: &Extent2d,
        adjust_pole_pt: bool,
    ) -> Result<i32> {
        // param is on a pole - change it's UV pos so that the Crv1stDeriv lines up with
        // the sole SrfTangent
        let mut singular_direction = SurfParamType::U;
        let on_pole = singularities != SS_NONE && {
            let (is, dir) = is_singularity(k, surface, self.uv_pd[0], approx_tol_3d)?;
            singular_direction = dir;
            is
        };
        self.on_pole = Some(on_pole);
        if on_pole && self.degen_curve != Some(true) && adjust_pole_pt {
            // find degenParam value at a singularity that matches the dropCurve UV
            // direction
            self.uv_pd[0] = find_degen_param(
                k,
                surface,
                self.uv_pd[0],
                self.crv_pd[1],
                true,
                singular_direction,
            )?;
        }

        // Surface pos and Srf U and V Derivs
        let (mut p3d, mut srf_du, mut srf_dv) =
            surface.evaluate_1st_derivatives(k, self.uv_pd[0], true, true)?;

        // set m_bFromLeftU and m_bFromLeftV
        let from_left_u = srf_du.dot(self.crv_pd[1]) >= 0.0;
        let from_left_v = srf_dv.dot(self.crv_pd[1]) >= 0.0;
        self.from_left_u = Some(from_left_u);
        self.from_left_v = Some(from_left_v);
        self.pd_3d[0] = p3d;

        // reevaluate when on knot boundaries as needed
        if !from_left_u || !from_left_v {
            (p3d, srf_du, srf_dv) =
                surface.evaluate_1st_derivatives(k, self.uv_pd[0], from_left_u, from_left_v)?;
            self.pd_3d[0] = p3d;
        }

        // SurfaceNormal at DropPoint
        self.srf_normal =
            surface.evaluate_normal(k, self.uv_pd[0], side(from_left_u), side(from_left_v))?;
        let start_gap = self.crv_pd[0] - self.pd_3d[0];

        // UDir tolerance
        let interval_u = uv_domain.u_interval();
        let interval_v = uv_domain.v_interval();
        let zone_tol = zone_tol_3d(k, surface)?;
        let zone_tol_2d_u =
            map_to_2d_dir(k, zone_tol, self.uv_pd[0], Vec2::new(1.0, 0.0), surface)?;
        let zone_tol_2d_v =
            map_to_2d_dir(k, zone_tol, self.uv_pd[0], Vec2::new(0.0, 1.0), surface)?;
        let limit_rad = deg_to_rad(30.0);

        // Classify DropPoint against UVDomain
        self.on_boundary = Some(false);
        self.leaving_domain = Some(false);
        self.entering_domain = Some(false);
        self.on_seam = Some(false);

        // When on U Boundary - set DropPoint Boundary State
        if self.uv_pd[0].x < interval_u.min() + zone_tol_2d_u
            || self.uv_pd[0].x > interval_u.max() - zone_tol_2d_u
        {
            // remember on boundary
            self.on_boundary = Some(true);
            let seam = is_on_seam(k, surface, self.uv_pd[0], approx_tol_3d)?;
            self.on_seam = Some(self.on_seam == Some(true) || seam);

            // Crv and Srf 1st derivative lengths
            let srf_len_sq = srf_du.length_squared();
            let crv_len_sq = self.crv_pd[1].length_squared();

            // when 1st derivs are nonZero
            if !(on_pole && (singularities & (SS_VMIN | SS_VMAX)) != 0)
                && (srf_len_sq * crv_len_sq > EFF_ZERO_SQ)
            {
                let mut ccw_rad = self.srf_normal.ccw_angle_between(srf_dv, self.crv_pd[1])?;
                if ccw_rad < 0.0 {
                    // This returns [-Pi, Pi]. [B260]
                    ccw_rad += 2.0 * PI;
                }

                // when on min boundary (dU points into surf)
                if self.uv_pd[0].x < interval_u.min() + zone_tol_2d_u {
                    self.leaving_domain = Some(is_between(ccw_rad, limit_rad, PI - limit_rad));
                    self.entering_domain =
                        Some(is_between(ccw_rad, PI + limit_rad, 2.0 * PI - limit_rad));
                } else {
                    // on max boundary (dU points out of surf)
                    self.leaving_domain =
                        Some(is_between(ccw_rad, PI + limit_rad, 2.0 * PI - limit_rad));
                    self.entering_domain = Some(is_between(ccw_rad, limit_rad, PI - limit_rad));
                }
            }
        }

        // When On V Boundary - set DropPoint Boundary State
        if (self.uv_pd[0].y < interval_v.min() + zone_tol_2d_v)
            || (self.uv_pd[0].y > interval_v.max() - zone_tol_2d_v)
        {
            // remember on boundary
            self.on_boundary = Some(true);
            let seam = is_on_seam(k, surface, self.uv_pd[0], approx_tol_3d)?;
            self.on_seam = Some(self.on_seam == Some(true) || seam);

            // Crv and Srf 1st derivative lengths
            let srf_len_sq = srf_dv.length_squared();
            let crv_len_sq = self.crv_pd[1].length_squared();

            if !(on_pole && (singularities & (SS_UMIN | SS_UMAX)) != 0)
                && (srf_len_sq * crv_len_sq > EFF_ZERO_SQ)
            {
                let mut ccw_rad = self.srf_normal.ccw_angle_between(srf_du, self.crv_pd[1])?;
                if ccw_rad < 0.0 {
                    // This returns [-Pi, Pi]. [B260]
                    ccw_rad += 2.0 * PI;
                }

                // when on min boundary (dV points into surf)
                if self.uv_pd[0].y < interval_v.min() + zone_tol_2d_v {
                    self.leaving_domain =
                        Some(is_between(ccw_rad, PI + limit_rad, 2.0 * PI - limit_rad));
                    self.entering_domain = Some(is_between(ccw_rad, limit_rad, PI - limit_rad));
                } else {
                    // on max boundary (dV points out of surf)
                    self.leaving_domain = Some(is_between(ccw_rad, limit_rad, PI - limit_rad));
                    self.entering_domain =
                        Some(is_between(ccw_rad, PI + limit_rad, 2.0 * PI - limit_rad));
                }
            }
        }
        let on_boundary = self.on_boundary == Some(true);

        // save the curve/Surface drop distance and deviation (distance from
        // SurfNormalLine to CurvePoint)
        let (approx_dev, drop_param) =
            line_point_distance(self.pd_3d[0], self.srf_normal, self.crv_pd[0]);
        self.approx_dev = approx_dev;
        self.drop_to_surf = drop_param; // was:   sStartGap.Length()   [B488]

        // A drop is good if the original point being dropped is within a tolerant shape
        // of the SrfNormalLine
        let scaled_tol = (if self.drop_to_surf < 1.0 {
            1.0
        } else {
            self.drop_to_surf
        }) * approx_tol_3d;
        let mut good_drop = self.approx_dev < scaled_tol;

        // when the Drop is not good and not on the boundary
        if !on_boundary && !good_drop {
            // when drop deviation is huge - don't bother with this check
            if self.approx_dev < 1000.0 * scaled_tol {
                // get the surface principle curvatures (status ignored upstream)
                let (curv1, curv2) = evaluate_principal_curvatures(
                    k,
                    surface,
                    self.uv_pd[0],
                    from_left_u,
                    from_left_v,
                )?;

                // center of curvature for both principle curvatures
                let center1 = self.pd_3d[0] + self.srf_normal * curv1;
                let center2 = self.pd_3d[0] + self.srf_normal * curv2;
                let pt_to_center1 = (center1 - self.crv_pd[0]).length();
                let pt_to_center2 = (center2 - self.crv_pd[0]).length();

                // get max scaling due to surface curvature
                let curv_scaling = max3(
                    curv1.abs() * pt_to_center1,
                    curv2.abs() * pt_to_center2,
                    5.0,
                );

                // reevaluate the quality of the drop
                good_drop = self.approx_dev < curv_scaling * approx_tol_3d;
            }
        }
        self.good_drop = Some(good_drop);

        // can't drop zero length Crv1stDeriv vectors when m_bDegenCurve is FALSE
        let crv_speed = self.crv_pd[1].length();
        if crv_speed < EFF_ZERO && self.degen_curve == Some(false) {
            // inform the public (when the drop was good) either way it is SM_ERR
            self.good_drop = Some(false);
            return Err(SolidError::sm_err(OP_SET));
        }

        // when m_bDegenCurve != TRUE
        if self.degen_curve != Some(true) {
            // can't drop Crv1stDeriv vectors parallel to m_SrfNormal (degenerate case)
            let srf_norm_crv_tan_ang_rad =
                self.srf_normal.angle_between(self.crv_pd[1]).unwrap_or(0.0);
            if rad_to_deg(srf_norm_crv_tan_ang_rad) < 3.0 {
                self.good_drop = Some(false);
                return Err(SolidError::sm_err(OP_SET));
            }
        }

        // Set rOffset.OffsetDist = crv/srf startSpan gap size in m_SrfNormal direction
        let offset_dist = start_gap.dot(self.srf_normal);
        offset.set_offset_distance(offset_dist);

        // Get SrfSpanStart 1stDeriv - drop CrvSpanStart 1stDeriv to OffsetSurface
        // TangentPlane at m_UVPD (For NonDegenCurve)
        if self.degen_curve == Some(false) {
            self.uv_pd[0] = uv_domain.clamp_point(self.uv_pd[0]); // note: perhaps no longer needed. [061001]
            self.uv_pd[1] = DropOffset::drop_vectors(
                offset,
                k,
                surface,
                self.uv_pd[0],
                from_left_u,
                from_left_v,
                self.crv_pd[1],
            )?;
        }

        // when DropPoint is on a pole - set DropPoint[UV 1stDeriv] Degen Param to 0
        if on_pole {
            // Just set undef param to 0: no 'sideways' movement at singularity.
            if singular_direction == SurfParamType::U {
                self.uv_pd[1].x = 0.0;
            } else {
                self.uv_pd[1].y = 0.0;
            }
        }

        // report the quality of the drop - compare the surface directions tangent with
        // the curve tangent
        if self.degen_curve != Some(true) {
            if !self.uv_pd[1].is_zero(EFF_ZERO) {
                // (status ignored upstream: a failed evaluation, e.g. the offset at a
                // pole, leaves `sOffsetPD` at its default `SM_UNDEF_DOUBLE` values)
                let [_, mut offset_d1] = ignored(DropOffset::evaluate_directional_derivs(
                    offset,
                    k,
                    surface,
                    self.uv_pd[0],
                    self.uv_pd[1],
                    from_left_u,
                    from_left_v,
                ))?
                .unwrap_or([Point3::UNDEF; 2]);
                let uv_speed = self.uv_pd[1].length();
                offset_d1 = uv_speed * offset_d1;

                // project Crv1stDir to plane perp to SurfaceNormal vector
                let crv_proj_1st_deriv = self.crv_pd[1].project_to_plane(self.srf_normal);

                // angle between Crv and SrfNormal
                let normal_to_crv_ang_rad =
                    self.crv_pd[1].angle_between(self.srf_normal).unwrap_or(0.0);
                self.normal_to_crv_ang_deg = rad_to_deg(normal_to_crv_ang_rad);

                // angle between Crv and Srf 1stDerivs
                let offset_to_crv_ang_rad =
                    crv_proj_1st_deriv.angle_between(offset_d1).unwrap_or(0.0);
                self.srf_to_crv_ang_deg = rad_to_deg(offset_to_crv_ang_rad);

                // size ratios
                let srf_speed = offset_d1.length();
                let proj_crv_speed = crv_proj_1st_deriv.length();
                self.srf_to_crv_speed_ratio = if proj_crv_speed > EFF_ZERO {
                    srf_speed / proj_crv_speed
                } else if srf_speed < EFF_ZERO {
                    1.0
                } else {
                    10000.0
                };

                // now project the UV vector to 3d through the surface
                self.pd_3d[1] = srf_du * self.uv_pd[1].x + srf_dv * self.uv_pd[1].y;
            }

            self.drop_deriv_rtn = if self.is_good_tangent_drop(Some(on_pole)) {
                SM_SUCCESS
            } else {
                SM_ERR_BAD_TANGENT_DROP
            };
        }

        // all done
        Ok(if good_drop {
            self.drop_deriv_rtn
        } else {
            SM_ERR_NOT_WITHIN_TOLERANCE
        })
    }

    /// Finds the span end drop point `end` at curve parameter `end_t` from this span
    /// start drop point while respecting seams and poles (`GetNextPoint`): a quadratic
    /// Taylor guess, a local drop of the curve point (then a global one when the first
    /// fails its tests), a seam-side correction and [`DropPt::set_properties`] with the
    /// curve tangent reversed (so the from-left flags look into the span). `end` is
    /// reset first; `pole_points` are the surface's pole points when known.
    ///
    /// Returns [`SM_SUCCESS`], [`SM_ERR_BAD_SURFACE_POINT`] (the start is outside
    /// `uv_domain`), [`SM_ERR_OUTSIDE_OF_DOMAIN`] (the start is on the boundary and
    /// leaving, or a seam correction is out of tolerance),
    /// [`SM_ERR_NOT_WITHIN_TOLERANCE`], [`SM_ERR_BAD_TANGENT_DROP`], or the status of
    /// the last [`DropPt::set_properties`] (see the module notes).
    ///
    /// # Errors
    ///
    /// Those of the curve evaluation and the point solves (`SM_ERR` when the local
    /// solve finds nothing), the start surface evaluation (ignored upstream, see the
    /// module notes) and the seam check's evaluations; [`SolidError::Unported`].
    pub fn get_next_point(
        &self,
        k: &Kernel,
        surface: &Surface,
        offset: &mut DropOffset,
        singularities: u32,
        pole_points: Option<&[Point3]>,
        uv_domain: &Extent2d,
        srf_closure: SurfParamType,
        curve: &Curve,
        approx_tol_3d: f64,
        end_t: f64,
        end: &mut Self,
    ) -> Result<i32> {
        let start = self;

        // init return values
        let mut ret = SM_ERR;
        end.set_uninitialized();
        end.t = end_t;
        end.degen_curve = start.degen_curve;

        // UDir tolerance
        let interval_u = uv_domain.u_interval();
        let interval_v = uv_domain.v_interval();
        let scaled_zero_u = EFF_ZERO * (1.0 + interval_u.length());
        let scaled_zero_v = EFF_ZERO * (1.0 + interval_v.length());

        // quit when rStart.m_UVPD[0] is not in UVDomain
        if !uv_domain.contains_point(start.uv_pd[0], max2(scaled_zero_u, scaled_zero_v)) {
            return Ok(SM_ERR_BAD_SURFACE_POINT);
        }

        // quit when rStart is on boundary and DropCurve is walking out of domain
        if start.on_boundary != Some(false) && start.leaving_domain != Some(false) {
            return Ok(SM_ERR_OUTSIDE_OF_DOMAIN);
        }

        // get StartPoint Surf DU, DV, DUU, DVV, DUV values (status ignored upstream)
        let [_, srf_u, srf_v, srf_uv, srf_uu, srf_vv] = surface.evaluate_2nd_derivatives(
            k,
            start.uv_pd[0],
            start.from_left_u != Some(false),
            start.from_left_v != Some(false),
        )?;

        // Guess SpanEnd[UV] with quadratic Taylor step
        let ut = start.uv_pd[1].x; // just renaming...
        let vt = start.uv_pd[1].y;

        // solve for UVtt from: Su*UVtt.x + Sv*UVtt.y = Ctt - [ Suu*Ut^2 + 2*Suv*Ut*Vt + Svv*Vt^2 ]
        let second_deriv_quantity = srf_uu * ut * ut + 2.0 * srf_uv * ut * vt + srf_vv * vt * vt;
        let rhs = start.crv_pd[2] - second_deriv_quantity;
        let uv_tt = vec_lin_comb_two_vectors(srf_u, srf_v, rhs).unwrap_or(Vec2::new(0.0, 0.0));

        // calc GuessSpanEnd[UV] with Taylor step from SpanStart[UV UV1stDeriv UV2ndDeriv]
        let delta_t = end.t - start.t;
        end.uv_pd[0] = start.uv_pd[0] + delta_t * start.uv_pd[1]; // first-order Taylor
        end.uv_pd[0] += uv_tt * (delta_t * delta_t / 2.0); // add second-order Taylor

        // Avoid bad SpanEnd[UV] guesses (common near poles with rapidly changing Surf
        // Deriv values).
        let delta_uv = end.uv_pd[0] - start.uv_pd[0];
        let uv_size = uv_domain.size();
        if delta_uv.x.abs() > 1.5 * uv_size.x || delta_uv.y.abs() > 1.5 * uv_size.y {
            end.uv_pd[0] = start.uv_pd[0];
        }

        // clamp GuessSpanEnd[UV]
        end.uv_pd[0] = uv_domain.clamp_point(end.uv_pd[0]);

        // get SpanEnd[CrvPt Crv1stDeriv]
        let crv = curve.evaluate(k, end.t, 2, false, true)?;
        for (dst, src) in end.crv_pd.iter_mut().zip(crv.iter()) {
            *dst = *src;
        }

        // find SpanEnd[UV] by dropping SpanEnd[CrvPt] to Surface starting iter at
        // GuessSpanEnd[UV].
        let (found_answer, local_uv) =
            local_point_solve(k, surface, uv_domain, end.crv_pd[0], end.uv_pd[0])?;

        // MINIMIZE should always find some solution.
        if !found_answer {
            return Err(SolidError::sm_err(OP_NEXT));
        }

        // load SpanEnd[UV] from DropPoint(SpandEnd[CrvPt]) solution
        let mut solution = local_uv;
        end.uv_pd[0] = solution;

        let mut ii = 0;
        while ii < 2 && ret != SM_SUCCESS {
            // if iter 2: try again with GlobalPointSolve DropPoint
            if ii == 1 {
                // temporarily set owner to null to prevent solver from seeing Face edges.
                // (upstream nulls the owner of the surface itself: an exact evaluation
                // copy, since `Clone` runs the copy constructor's knot fix-up)
                let mut unowned = surface.clone_for_evaluation()?;
                let _ = unowned.set_owner(None);

                // drop sCrvSpanEndPt to surface with GlobalPointSolve
                let solutions: SolutionArray<NoObject> = unowned.global_point_solve(
                    k,
                    Some(NoObject),
                    uv_domain,
                    SolverOperation::Minimize,
                    end.crv_pd[0],
                    EFF_ZERO,
                    None,
                    SolutionRequested::All,
                )?;

                // save best solution (closest to orig parameterSpace estimate)
                let mut small_diff = 0.0;
                for (jj, tmp_sol) in solutions.as_slice().iter().enumerate() {
                    let su = tmp_sol.start.parameters.first().copied().unwrap_or(0.0);
                    let sv = tmp_sol.start.parameters.get(1).copied().unwrap_or(0.0);

                    // distance between this solution and orig parameter estimate
                    let diff = (su - end.uv_pd[0].x).abs() + (sv - end.uv_pd[0].y).abs();

                    // save best solution
                    if jj == 0
                        || (diff < small_diff && tmp_sol.start.solution_value < approx_tol_3d)
                    {
                        small_diff = diff;
                        solution = Point2::new(su, sv);
                    }
                }

                // quit when Global Drop was no different than local drop
                if end.uv_pd[0].closer_than(EFF_ZERO, solution) {
                    break;
                }
            }

            // set SpanEnd[UV] = drop SpanEnd[CrvPt] to Surface
            end.uv_pd[0] = solution;

            // When SpanEnd[UV] is on a pole - adjust it's singular param value [061001]
            let (singular, singular_direction) = match pole_points {
                Some(poles) => is_singularity_with_poles(
                    k,
                    surface,
                    end.uv_pd[0],
                    singularities,
                    poles,
                    approx_tol_3d,
                )?,
                None => is_singularity(k, surface, end.uv_pd[0], approx_tol_3d)?,
            };
            if singular {
                end.uv_pd[0] = find_degen_param(
                    k,
                    surface,
                    end.uv_pd[0],
                    end.crv_pd[1],
                    false,
                    singular_direction,
                )?;
            }

            // Correct an ambiguous seam-side choice only within the 3D tolerance.
            // A larger correction rejects this candidate without moving the endpoint.
            match start.check_seam_jump(k, surface, srf_closure, approx_tol_3d, end) {
                Ok(()) => {}
                Err(SolidError::Upstream {
                    code: SM_ERR_OUTSIDE_OF_DOMAIN,
                    ..
                }) => return Ok(SM_ERR_OUTSIDE_OF_DOMAIN),
                Err(e) => return Err(e),
            }

            // get SpanEnd[OtherValues] from SpanEnd[CrvPD and UV[0]]
            end.crv_pd[1] = -end.crv_pd[1];
            ret = match end.set_properties(
                k,
                surface,
                offset,
                singularities,
                approx_tol_3d,
                uv_domain,
                false,
            ) {
                Ok(code) => code,
                Err(e) if e.is_unported() => return Err(e),
                Err(e) => status_of(&e),
            };

            // when Properties are set - negate the tangent direction a second time
            if ret != SM_ERR {
                end.crv_pd[1] = -end.crv_pd[1];
                end.uv_pd[1] = -end.uv_pd[1];
                // gwc: why not rEnd.m_3dPD[1] as well?
                if end.on_boundary != Some(false) {
                    // negate entering/leaving when leaving or entering domain - leave
                    // alone when running along domain
                    if end.leaving_domain != Some(false) {
                        end.leaving_domain = Some(false);
                        end.entering_domain = Some(true);
                    } else if end.entering_domain != Some(false) {
                        end.leaving_domain = Some(true);
                        end.entering_domain = Some(false);
                    }
                }
            }
            ii += 1;
        }

        // all done
        Ok(ret)
    }
}

/// Makes a degree 3 piecewise Bezier UV curve (knot multiplicities `[4 3 .. 3 4]`) of
/// the control polygon `ctrl_poly` and the breakpoints `knots`, adds it to `curves`
/// and empties both arrays (`sm_FlushCrv`); does nothing without knots. The polygon is
/// first fixed up against `uv_domain` ([`fix_boundary_curves`]); when that changes it,
/// `fix_bc_change` is set (`rbFixBCChange`, only ever set, also when the curve then
/// cannot be made).
///
/// # Errors
///
/// Those of [`fix_boundary_curves`] and [`BSplineCurve::create_canonical`] (the arrays
/// are then kept).
pub fn flush_curve(
    uv_domain: &Extent2d,
    ctrl_poly: &mut Vec<Point3>,
    knots: &mut Vec<f64>,
    uv_tol_x: f64,
    uv_tol_y: f64,
    curves: &mut Vec<BSplineCurve>,
    fix_bc_change: &mut bool,
) -> Result<()> {
    // no work: no knots
    let num_knots = knots.len();
    if num_knots == 0 {
        return Ok(());
    }

    // postprocess the curve
    if fix_boundary_curves(ctrl_poly, uv_domain, uv_tol_x, uv_tol_y)? {
        *fix_bc_change = true;
    }

    // for every knot - set knot multiplicy = [ 4 3 3 ... 4]
    let mut knot_mult = vec![3usize; num_knots];
    if let Some(m) = knot_mult.first_mut() {
        *m = 4;
    }
    if let Some(m) = knot_mult.last_mut() {
        *m = 4;
    }

    // create degree 3 piecewise C1 BSpline
    let new_bsp = BSplineCurve::create_canonical(&CanonicalData {
        dim: 2,
        degree: 3,
        control_points: ctrl_poly,
        form: BSplineCurveForm::Unspecified,
        knot_multiplicities: &knot_mult,
        knots,
        knot_type: KnotType::Unspecified,
        weights: None,
        trim: None,
    })?;

    // set output
    curves.push(new_bsp);
    knots.clear();
    ctrl_poly.clear();

    // all done
    Ok(())
}

/// Adds the cubic Bezier UV span from `start` to `end` (the Hermite span of their UV
/// points and tangents scaled to the parameter step) to the growing control polygon
/// `ctrl_poly` and breakpoints `knots` (`sm_AddToCurve`). A span that is not
/// continuous with the growing curve (in parameter or in UV) or of zero length first
/// flushes it ([`flush_curve`]); a zero length span is then skipped. At a pole, a
/// dog-legged span is straightened along the singular boundary. A flush that changes
/// the polygon sets `fix_bc_change` (`rbFixBCChange`, only ever set).
///
/// # Errors
///
/// `SM_ERR` once the polygon has [`MAX_CTRL_POINTS`] points (the span is added);
/// those of [`flush_curve`]; [`SolidError::Unported`] for the singular direction of a
/// non-planar pole.
pub fn add_to_curve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    start: &DropPt,
    end: &DropPt,
    ctrl_poly: &mut Vec<Point3>,
    knots: &mut Vec<f64>,
    approx_tol_3d: f64,
    uv_tol_x: f64,
    uv_tol_y: f64,
    curves: &mut Vec<BSplineCurve>,
    fix_bc_change: &mut bool,
) -> Result<()> {
    // locals for hermite curve segment
    let scale = end.t - start.t;
    let mut p1 = start.uv_pd[0];
    let d1 = start.uv_pd[1] * scale;
    let mut p4 = end.uv_pd[0];
    let d2 = end.uv_pd[1] * scale;

    // Bezier segement interior control points are end points +/- 1st derivs divided by
    // degree
    let mut p2 = p1 + d1 / 3.0;
    let mut p3 = p4 - d2 / 3.0;

    // when beg/end pt is on a pole - straighten out dog-leg shapes
    let start_on_pole = start.on_pole != Some(false);
    let dog_leg = if start_on_pole || end.on_pole != Some(false) {
        is_dog_leg(p1, p2, p3, p4, 35.0, 45.0)
    } else {
        false
    };
    if dog_leg {
        // fetch the singularity direction
        let (_, singular_direction) = if start_on_pole {
            is_singularity(k, surface, p1, approx_tol_3d)?
        } else {
            is_singularity(k, surface, p4, approx_tol_3d)?
        };

        // straighten out pole by moving Pt on Singularity in line with End of segment
        // not on singularity
        if singular_direction == SurfParamType::U {
            // U varies, V const along singularity
            if start_on_pole {
                p1.x = p4.x;
                p2.x = p3.x;
            } else {
                p4.x = p1.x;
                p3.x = p2.x;
            }
        } else if start_on_pole {
            // V varies, U const along singularity
            p1.y = p4.y;
            p2.y = p3.y;
        } else {
            p4.y = p1.y;
            p3.y = p2.y;
        }
    }

    // when span is zero length or discontinuous with the growing curve arrays,
    // output current controlPoint and knot lists as a curve.
    if let (Some(&last_knot), Some(&last_pt)) = (knots.last(), ctrl_poly.last()) {
        let p1_3d = Vec3::from(p1);
        if (scale < EFF_ZERO) // zero length span
            || ((last_knot - start.t).abs() > EFF_ZERO // next segment discontinuous in param space
                || p1_3d.distance(last_pt) > EFF_ZERO)
        // next segment discontinuous in image space
        {
            // span is notC0 or zeroLength - create current curve, restart lists, and
            // carry on.
            flush_curve(
                uv_domain,
                ctrl_poly,
                knots,
                uv_tol_x,
                uv_tol_y,
                curves,
                fix_bc_change,
            )?;
        }
    } else if !knots.is_empty() {
        // knots without a polygon (upstream would read past the empty polygon)
        return Err(SolidError::invalid_input(
            OP_ADD,
            "knots without control points",
        ));
    }

    // skip zero length spans
    if scale < EFF_ZERO {
        return Ok(());
    }

    // Clamp points into parameter space
    p1 = uv_domain.clamp_point(p1);
    p4 = uv_domain.clamp_point(p4);

    // when adding 1st segment - add start CtrlPoint and start knot value
    if knots.is_empty() {
        knots.push(start.t);
        ctrl_poly.push(Vec3::from(p1));
    }

    // for all segments - add mid and end CtrlPts and end knot value
    knots.push(end.t);
    ctrl_poly.push(Vec3::from(p2));
    ctrl_poly.push(Vec3::from(p3));
    ctrl_poly.push(Vec3::from(p4));

    // return error for huge drop point counts
    if ctrl_poly.len() >= MAX_CTRL_POINTS {
        return Err(SolidError::sm_err(OP_ADD));
    }

    // all done
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent2d::Extent2d;
    use crate::surface::plane::Plane;

    fn plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn pt(t: f64, uv: Point2, duv: Vec2) -> DropPt {
        let mut p = DropPt::new(t, Some(false));
        p.uv_pd = [uv, duv];
        p.on_pole = Some(false);
        p
    }

    #[test]
    fn add_to_curve_builds_and_flushes() {
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut poly = Vec::new();
        let mut knots = Vec::new();
        let mut curves = Vec::new();
        let mut ch = false;
        let a = pt(0.0, Point2::new(0.1, 0.1), Vec2::new(0.5, 0.0));
        let b = pt(1.0, Point2::new(0.6, 0.1), Vec2::new(0.5, 0.0));
        let c = pt(2.0, Point2::new(0.6, 0.6), Vec2::new(0.0, 0.5));
        let mut add = |p: &DropPt, q: &DropPt, poly: &mut Vec<Point3>, knots: &mut Vec<f64>| {
            add_to_curve(
                &Kernel::default(),
                &s,
                &d,
                p,
                q,
                poly,
                knots,
                1e-6,
                1e-9,
                1e-9,
                &mut curves,
                &mut ch,
            )
        };
        add(&a, &b, &mut poly, &mut knots).unwrap();
        assert_eq!(poly.len(), 4);
        assert_eq!(knots, vec![0.0, 1.0]);
        add(&b, &c, &mut poly, &mut knots).unwrap();
        assert_eq!(poly.len(), 7);
        // a discontinuous span flushes the curve
        add(&a, &b, &mut poly, &mut knots).unwrap();
        assert_eq!(poly.len(), 4);
        // a zero length span flushes and is skipped
        add(&b, &b, &mut poly, &mut knots).unwrap();
        assert!(poly.is_empty() && knots.is_empty());
        assert_eq!(curves.len(), 2);
        assert!(!ch);
    }

    #[test]
    fn flush_without_knots_is_a_no_op() {
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut curves = Vec::new();
        let mut ch = false;
        flush_curve(
            &d,
            &mut Vec::new(),
            &mut Vec::new(),
            0.0,
            0.0,
            &mut curves,
            &mut ch,
        )
        .unwrap();
        assert!(curves.is_empty() && !ch);
    }

    #[test]
    fn next_point_early_returns() {
        let k = Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let c = Curve::from(crate::curve::line::Line::from_points(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            3,
        ));
        let mut off = DropOffset::new(1.0, SS_NONE);
        let mut end = DropPt::new(0.0, None);
        let outside = pt(0.0, Point2::new(2.0, 0.5), Vec2::new(1.0, 0.0));
        let r = outside
            .get_next_point(
                &k,
                &s,
                &mut off,
                SS_NONE,
                None,
                &d,
                SurfParamType::Neither,
                &c,
                1e-6,
                0.5,
                &mut end,
            )
            .unwrap();
        assert_eq!(r, SM_ERR_BAD_SURFACE_POINT);
        assert_eq!(end.t, 0.5);
        assert_eq!(end.degen_curve, Some(false));
        let mut leaving = pt(0.0, Point2::new(0.0, 0.5), Vec2::new(1.0, 0.0));
        leaving.on_boundary = Some(true);
        leaving.leaving_domain = Some(true);
        let r = leaving
            .get_next_point(
                &k,
                &s,
                &mut off,
                SS_NONE,
                None,
                &d,
                SurfParamType::Neither,
                &c,
                1e-6,
                0.5,
                &mut end,
            )
            .unwrap();
        assert_eq!(r, SM_ERR_OUTSIDE_OF_DOMAIN);
    }

    #[test]
    fn set_properties_degenerate_curve() {
        let k = Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut p = DropPt::new(0.0, Some(true));
        p.uv_pd = [Point2::new(0.5, 0.5), Vec2::new(0.0, 0.0)];
        p.crv_pd = [
            Vec3::new(0.5, 0.5, 2.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        ];
        let mut off = DropOffset::new(1.0, SS_NONE);
        let r = p
            .set_properties(&k, &s, &mut off, SS_NONE, 1e-6, &d, true)
            .unwrap();
        assert_eq!(r, p.drop_deriv_rtn);
        assert_eq!(off.offset_distance, 2.0);
        assert_eq!(p.good_drop, Some(true));
        assert_eq!(p.on_boundary, Some(false));
        assert_eq!(p.drop_to_surf, 2.0);
    }

    #[test]
    fn set_properties_tangent_along_normal_is_sm_err() {
        let k = Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut p = DropPt::new(0.0, Some(false));
        p.uv_pd = [Point2::new(0.5, 0.5), Vec2::new(0.0, 0.0)];
        p.crv_pd = [
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
        ];
        let mut off = DropOffset::new(1.0, SS_NONE);
        assert!(p
            .set_properties(&k, &s, &mut off, SS_NONE, 1e-6, &d, true)
            .is_err());
        assert_eq!(p.good_drop, Some(false));
    }
}
