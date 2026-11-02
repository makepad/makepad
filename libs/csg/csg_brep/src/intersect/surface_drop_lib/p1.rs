//! `SmSurfaceDropLib`, part 1: the drop point record [`DropPt`] with its tests and
//! fix-ups (moving along a boundary, span accuracy, seam jumps, boundary tangents),
//! the span stack and control polygon helpers of `DropCurve`, and
//! [`Surface::drop_iso_curve`] / [`Surface::drop_and_trim_curve`].
//!
//! Upstream: `source/SMLib/src/SmSurfaceDropLib.cpp`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`DropPt`]                                   | `SmDropPt` (file-local class)                    |
//! | [`DropPt::new`]                              | `SmDropPt::SmDropPt(dT, bDegenCurve)`            |
//! | `Clone` / [`DropPt::assign`]                 | `SmDropPt::operator=`                            |
//! | [`DropPt::set_uninitialized`]                | `SmDropPt::SetUninitialized`                     |
//! | [`DropPt::is_good_tangent_drop`]             | `SmDropPt::IsGoodTangentDrop`                    |
//! | [`DropPt::is_moving_along_boundary`]         | `SmDropPt::IsMovingAlongBoundary`                |
//! | [`DropPt::test_span_accuracy`], [`SpanAccuracy`] | `SmDropPt::TestSpanAccuracy`                 |
//! | [`DropPt::check_seam_jump`]                  | `SmDropPt::CheckSeamJump`                        |
//! | [`DropPt::fix_boundary_tangents`]            | `SmDropPt::FixBoundaryTangents`                  |
//! | [`push_point`]                               | `sm_PushPoint` (static)                          |
//! | [`pop_stack`]                                | `sm_PopStack` (static)                           |
//! | [`stack_look`]                               | `sm_StackLook` (static)                          |
//! | [`fix_boundary_curves`]                      | `sm_FixBoundaryCurves` (static)                  |
//! | [`remove_duplicate_drop_point_solutions`]    | `sm_RemoveDuplicateDropPointSolutions` (static)  |
//! | [`Surface::drop_and_trim_curve`], [`Surface::drop_and_trim_curve_in`], [`DropAndTrim`], [`DropFailRecord`] | `SmSurface::DropAndTrimCurve` |
//! | [`Surface::drop_iso_curve`], [`IsoDrop`]     | `SmSurface::DropIsoCurve`                        |
//!
//! Deviations:
//! - **Tri-state flags.** `SmBoolean` members that start as `UNSURE` are
//!   `Option<bool>` (`None` = `UNSURE`); `m_eDropDerivRtn` is the raw status code,
//!   [`BIG_ULONG`] when unset. Where upstream passes such a member on as a
//!   boolean (`m_bFromLeftU` to `Evaluate1stDerivatives`), `UNSURE` is true, as
//!   upstream's truth test makes it.
//! - **Outputs.** Out-params become return values: [`SpanAccuracy`], [`IsoDrop`],
//!   [`DropAndTrim`], the `rbFixBTChange` / `rbFixBCChange` flags are returned
//!   (`sm_FixBoundaryCurves` only ever sets its flag, so the caller ORs the result
//!   into its own). On an error the partial outputs upstream may have written
//!   (`rbSuccessfulIsoDrop` before a failed `CreateLineSegment`) are not returned.
//! - **Statuses.** `CheckSeamJump`'s `SM_ERR_OUTSIDE_OF_DOMAIN` is
//!   [`SolidError::Upstream`] with code [`SM_ERR_OUTSIDE_OF_DOMAIN`];
//!   `sm_PopStack` / `sm_StackLook` on a short stack are `SM_ERR`.
//! - **Ignored statuses.** `IsMovingAlongBoundary` ignores the statuses of its
//!   surface evaluations and `TestSpanAccuracy` those of the two `EvaluatePoint`s of
//!   its domain clamp check; upstream then goes on with whatever the failed
//!   evaluator left in its outputs (the `SM_UNDEF_DOUBLE` member defaults, or values
//!   it wrote before failing). That failure path is not ported: here a failed
//!   evaluation propagates its error, and the difftest shim rejects those inputs. No
//!   generated or review case (poles, cone apex, collapsed B-spline edge) reaches it.
//!   `AngleBetween`'s status is ignored as upstream (angle `0`).
//! - **`sm_FixBoundaryCurves`.** A polygon of `3k` points (`k >= 2`) makes upstream
//!   read one point past the array (`SM_ASSERT(lNumPts == 1 + 3 * lNumSpans)` is
//!   compiled out); that is [`SolidError::InvalidInput`] here, before any change.
//! - **`DropAndTrimCurve`.** The record's `pUVCurvesArg` (the caller's output array)
//!   has no handle; it stays `None`. The surface and curve handles the record keeps
//!   come with the record ([`DropFailRecord`]).
//! - **`DropAndTrimCurve` UV curves.** `rUVCurves` (`SmBSplineCurve*`) holds each
//!   traced UV curve as the object `DoTrace` made (an `SmLine` stays a line, keeping
//!   its overrides), so [`DropAndTrim::uv_curves`] are [`Curve`]s of the B-spline
//!   kinds; a traced curve of another kind (a null `SM_CAST_PTR(SmBSplineCurve, ...)`
//!   upstream, added to `rUVCurves` as a null pointer) is `SM_ERR_NULL_POINTER`.
//!   The surface cache `DoTrace` checks out is the caller's when it passes one
//!   ([`Surface::drop_and_trim_curve_in`]), else built fresh
//!   ([`tracer_surface_cache`]).
//! - **Not ported here.** The `SM_DEBUG_CODE` blocks, `Dump`, `Draw`,
//!   `my_AssertAndDrawDropCurve` (graphics) and the `SM_ASSERT`s.

use crate::curve::curve::{Curve, CurveKind};
use crate::curve::hermite_curve::HermiteCurve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_point_distance;
use crate::geom::point_set::PointContainment;
use crate::geom::solution_array::{ObjectKind, SolutionArray, SolutionObject};
use crate::geom::tol::{are_same, are_same_to_tol};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::surface_drop_curve::SurfaceDropCurve;
use crate::intersect::surface_drop_lib::p3::DropSolveContext;
use crate::intersect::surface_tracer::tracer_surface_cache;
use crate::kernel::Kernel;
use crate::math::{max2, rad_to_deg, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::{
    CreateUvTrimCurvePathType, DropCurveFail, DropCurveFailArgs, UNSURE,
};
use crate::surface::surface::p4::is_point_on_normal as is_point_on_normal_p4;
use crate::surface::surface::Surface;
use crate::surface::surface_cache::p1::SurfaceCache;

/// `SM_BIG_ULONG` (`SmMath.h`): the unset `m_eDropDerivRtn`.
pub const BIG_ULONG: i32 = 99_999_999;

/// `SM_SUCCESS` (`SmMessages.h`).
pub const SM_SUCCESS: i32 = 1000;

/// `SM_ERR_OUTSIDE_OF_DOMAIN` (`SmMessages.h`): `CheckSeamJump` cannot snap a seam
/// jump within tolerance.
pub const SM_ERR_OUTSIDE_OF_DOMAIN: i32 = 1020;

/// `IsMovingAlongBoundary`'s default angle limit (`dAngLimitDeg`, degrees).
pub const DEFAULT_ALONG_ANGLE_DEG: f64 = 5.0;

const OP_SEAM: &str = "intersect::surface_drop_lib::DropPt::check_seam_jump";
const OP_POP: &str = "intersect::surface_drop_lib::pop_stack";
const OP_LOOK: &str = "intersect::surface_drop_lib::stack_look";
const OP_FIX_CURVES: &str = "intersect::surface_drop_lib::fix_boundary_curves";
const OP_TRIM: &str = "intersect::surface_drop_lib::drop_and_trim_curve";

/// `SM_DEG2RAD(a)` = `a * (SM_PI / 180.0)`.
fn deg_to_rad(a: f64) -> f64 {
    a * (PI / 180.0)
}

/// The upstream `bUFromLeft` / `bVFromLeft` flag as a span side (`TRUE` is
/// [`Side::Left`]).
const fn side(from_left: bool) -> Side {
    if from_left {
        Side::Left
    } else {
        Side::Right
    }
}

/// A curve point dropped to a surface point, with everything `DropCurve` learns
/// about it (`SmDropPt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropPt {
    /// `m_dT`: the curve parameter of the dropped point.
    pub t: f64,
    /// `m_bGoodDrop`: the curve point lies within tolerance of the surface normal line
    /// at the drop point.
    pub good_drop: Option<bool>,
    /// `m_CrvPD`: the curve point and its first and second derivatives at `t`.
    pub crv_pd: [Vec3; 3],
    /// `m_UVPD`: the surface drop point and its first derivative.
    pub uv_pd: [Point2; 2],
    /// `m_3dPD`: the surface point and first derivative at `uv_pd[0]`.
    pub pd_3d: [Point3; 2],
    /// `m_SrfNormal`: the unit surface normal at the drop point.
    pub srf_normal: Vec3,
    /// `m_bFromLeftU`: evaluate from the left on a u boundary.
    pub from_left_u: Option<bool>,
    /// `m_bFromLeftV`: evaluate from the left on a v boundary.
    pub from_left_v: Option<bool>,
    /// `m_dDropToSurf`: distance from the curve point to the surface drop point.
    pub drop_to_surf: f64,
    /// `m_dApproxDev`: distance from the curve point to the surface normal line.
    pub approx_dev: f64,
    /// `m_dNormalToCrvAngDeg`: angle between the curve tangent and the surface
    /// normal (should be 90).
    pub normal_to_crv_ang_deg: f64,
    /// `m_dSrfToCrvAngDeg`: angle between the dropped tangent and the surface image
    /// of the UV tangent (should be 0).
    pub srf_to_crv_ang_deg: f64,
    /// `m_dSrfToCrvSpeedRatio`: their speed ratio (should be 1).
    pub srf_to_crv_speed_ratio: f64,
    /// `m_bOnBoundary`: the drop point is on the domain boundary.
    pub on_boundary: Option<bool>,
    /// `m_bOnSeam`: that boundary is a seam.
    pub on_seam: Option<bool>,
    /// `m_bOnPole`: that boundary is a pole.
    pub on_pole: Option<bool>,
    /// `m_bDegenCurve`: the dropped curve is degenerate (a point).
    pub degen_curve: Option<bool>,
    /// `m_bLeavingDomain`: the drop curve leaves the domain here.
    pub leaving_domain: Option<bool>,
    /// `m_bEnteringDomain`: the drop curve enters the domain here.
    pub entering_domain: Option<bool>,
    /// `m_eDropDerivRtn`: the status of the derivative drop ([`BIG_ULONG`] unset).
    pub drop_deriv_rtn: i32,
}

/// What [`DropPt::test_span_accuracy`] measured (`TestSpanAccuracy`'s outputs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanAccuracy {
    /// `rbSpanPassesTest`.
    pub passes: bool,
    /// `rdMaxDropToSurf`: the largest curve to surface distance seen.
    pub max_drop_to_surf: f64,
    /// `rdMaxDropToSurfParam`: the curve parameter of that distance.
    pub max_drop_to_surf_param: f64,
    /// `rdMaxApproxDev`: the largest deviation from a surface normal line seen.
    pub max_approx_dev: f64,
}

/// The `smsurf_IsPointOnNormal` outputs `TestSpanAccuracy` reads.
struct PointOnNormal {
    is_good: bool,
    drop_to_surf: f64,
    approx_dev: f64,
}

/// `smsurf_IsPointOnNormal(surface, &domain, uv, point, &tangent, ...)`.
fn is_point_on_normal(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    uv: Point2,
    point: Point3,
    tangent: Vec3,
    approx_tol_3d: f64,
) -> Result<PointOnNormal> {
    let r = is_point_on_normal_p4(
        k,
        surface,
        Some(domain),
        uv,
        point,
        Some(tangent),
        approx_tol_3d,
    )?;
    Ok(PointOnNormal {
        is_good: r.good_point,
        drop_to_surf: r.drop_to_surf,
        approx_dev: r.approx_dev,
    })
}

/// The sector of a counter-clockwise angle from a boundary tangent:
/// `(-PI + lim, -lim)` is `first`, `(lim, PI - lim)` the other side, anything else
/// along the boundary.
fn sector(ang: f64, lim: f64, first_inside: bool) -> PointContainment {
    let (first, second) = if first_inside {
        (PointContainment::Inside, PointContainment::Outside)
    } else {
        (PointContainment::Outside, PointContainment::Inside)
    };
    if ang > -PI + lim && ang < 0.0 - lim {
        first
    } else if ang > 0.0 + lim && ang < PI - lim {
        second
    } else {
        PointContainment::OnBoundary
    }
}

impl DropPt {
    /// A drop point at `t`, everything else unset (`SmDropPt(dT, bDegenCurve)`).
    #[must_use]
    pub fn new(t: f64, degen_curve: Option<bool>) -> Self {
        let mut s = Self {
            t: UNDEF_DOUBLE,
            good_drop: None,
            crv_pd: [Vec3::UNDEF; 3],
            uv_pd: [Point2::UNDEF; 2],
            pd_3d: [Point3::UNDEF; 2],
            srf_normal: Vec3::UNDEF,
            from_left_u: None,
            from_left_v: None,
            drop_to_surf: UNDEF_DOUBLE,
            approx_dev: UNDEF_DOUBLE,
            normal_to_crv_ang_deg: UNDEF_DOUBLE,
            srf_to_crv_ang_deg: UNDEF_DOUBLE,
            srf_to_crv_speed_ratio: UNDEF_DOUBLE,
            on_boundary: None,
            on_seam: None,
            on_pole: None,
            degen_curve: None,
            leaving_domain: None,
            entering_domain: None,
            drop_deriv_rtn: BIG_ULONG,
        };
        s.set_uninitialized();
        s.t = t;
        s.degen_curve = degen_curve;
        s
    }

    /// Copies every member of `other` (`operator=`; the same as `*self = *other`).
    pub fn assign(&mut self, other: &Self) {
        *self = *other;
    }

    /// Resets every member to unset (`SetUninitialized`).
    pub fn set_uninitialized(&mut self) {
        self.t = UNDEF_DOUBLE;
        self.good_drop = None;
        self.crv_pd[0].set_uninitialized();
        self.crv_pd[1].set_uninitialized();
        self.crv_pd[2].set_uninitialized();
        self.uv_pd[0].set_uninitialized();
        self.uv_pd[1].set_uninitialized();
        self.pd_3d[0].set_uninitialized();
        self.pd_3d[1].set_uninitialized();
        self.srf_normal.set_uninitialized();
        self.from_left_u = None;
        self.from_left_v = None;
        self.drop_to_surf = UNDEF_DOUBLE;
        self.approx_dev = UNDEF_DOUBLE;
        self.normal_to_crv_ang_deg = UNDEF_DOUBLE;
        self.srf_to_crv_ang_deg = UNDEF_DOUBLE;
        self.srf_to_crv_speed_ratio = UNDEF_DOUBLE;
        self.on_boundary = None;
        self.on_seam = None;
        self.on_pole = None;
        self.degen_curve = None;
        self.leaving_domain = None;
        self.entering_domain = None;
        self.drop_deriv_rtn = BIG_ULONG;
    }

    /// True when the curve tangent drops to the surface within 3 degrees and without
    /// a large speed change (15 degrees and a factor 4 at a pole; any `on_pole` but
    /// `Some(false)` counts as a pole, as upstream's `bOnPole == FALSE` test)
    /// (`IsGoodTangentDrop`).
    #[must_use]
    pub fn is_good_tangent_drop(&self, on_pole: Option<bool>) -> bool {
        if on_pole == Some(false) {
            self.srf_to_crv_ang_deg < 3.0
                && self.srf_to_crv_speed_ratio < 2.0
                && self.srf_to_crv_speed_ratio > 0.5
        } else {
            self.srf_to_crv_ang_deg < 15.0
                && self.srf_to_crv_speed_ratio < 4.0
                && self.srf_to_crv_speed_ratio > 0.25
        }
    }

    /// How the curve tangent `crv_pd[1]` moves at the drop point `uv_pd[0]` when that
    /// is on a boundary of `uv_domain` (`IsMovingAlongBoundary`): into the surface
    /// ([`PointContainment::Inside`]), out of it, along the boundary (within
    /// `ang_limit_deg`, upstream default [`DEFAULT_ALONG_ANGLE_DEG`]), or
    /// [`PointContainment::Unknown`] off the boundary (or, on a u boundary, for a
    /// zero curve or boundary tangent). `test_bdry` selects the u boundaries
    /// ([`SurfParamType::U`]), the v boundaries ([`SurfParamType::V`]) or both (any
    /// other value; upstream default [`SurfParamType::Both`]).
    ///
    /// # Errors
    ///
    /// Those of the surface evaluations (ignored upstream, see the module notes).
    pub fn is_moving_along_boundary(
        &self,
        k: &Kernel,
        surface: &Surface,
        uv_domain: &Extent2d,
        uv_tol_x: f64,
        uv_tol_y: f64,
        ang_limit_deg: f64,
        test_bdry: SurfParamType,
    ) -> Result<PointContainment> {
        let u_range = uv_domain.u_interval();
        let v_range = uv_domain.v_interval();
        let test = if test_bdry != SurfParamType::U && test_bdry != SurfParamType::V {
            SurfParamType::Both
        } else {
            test_bdry
        };

        // Angle limit for 'along': default:[5 degrees]
        let ang_limit_rad = deg_to_rad(ang_limit_deg);
        let uv0 = self.uv_pd[0];
        let crv_tan = self.crv_pd[1];

        // when UVPt is on UMin/UMax boundary
        if (test == SurfParamType::U || test == SurfParamType::Both)
            && u_range.is_value_on_boundary(uv0.x, uv_tol_x).0
        {
            // which surface bdry, Sv 1stDeriv, and m_SrfNorm
            let low = uv0.x < u_range.mid();
            let (_, _, sv) = surface.evaluate_1st_derivatives(k, uv0, low, false)?;
            let srf_norm = surface.evaluate_normal(k, uv0, side(low), side(false))?;

            // when either Crv or SrfBdry is zero length - then not moving along boundary
            if sv.length_squared() < EFF_ZERO_SQ || crv_tan.length_squared() < EFF_ZERO_SQ {
                return Ok(PointContainment::Unknown);
            }

            // curveTangent to Surface Sv angle - [-Pi to Pi]
            let ang = srf_norm.ccw_angle_between(sv, crv_tan)?;
            let mut poc = sector(ang, ang_limit_rad, low);

            // watch out for raindrops (Sv on both sides of the seam classify the
            // CurveTangent as Inside)
            if poc == PointContainment::Inside {
                let other_uv = Point2::new(if low { u_range.max() } else { u_range.min() }, uv0.y);
                let (_, _, other_sv) =
                    surface.evaluate_1st_derivatives(k, other_uv, !low, false)?;
                let other_norm = surface.evaluate_normal(k, other_uv, side(!low), side(false))?;
                let other_ang = other_norm.ccw_angle_between(other_sv, crv_tan)?;
                if sector(other_ang, ang_limit_rad, !low) == PointContainment::Inside {
                    // upstream ignores AngleBetween's status (angle 0 on failure)
                    let other_normal_to_crv = other_norm.angle_between(crv_tan).unwrap_or(0.0);
                    poc = if (self.normal_to_crv_ang_deg - 90.0).abs()
                        < (rad_to_deg(other_normal_to_crv) - 90.0).abs()
                    {
                        PointContainment::Inside
                    } else {
                        PointContainment::Outside
                    };
                }
            }
            return Ok(poc);
        }

        // when UVPt is on VMin/VMax boundary
        if (test == SurfParamType::V || test == SurfParamType::Both)
            && v_range.is_value_on_boundary(uv0.y, uv_tol_y).0
        {
            // which surface bdry, Su 1stDeriv, and m_SrfNorm
            let low = uv0.y < v_range.mid();
            let (_, su, _) = surface.evaluate_1st_derivatives(k, uv0, false, low)?;
            let srf_norm = surface.evaluate_normal(k, uv0, side(false), side(low))?;

            // curveTangent to Surface Su angle - [-Pi to Pi]
            let ang = srf_norm.ccw_angle_between(su, crv_tan)?;
            let mut poc = sector(ang, ang_limit_rad, !low);

            if poc == PointContainment::Inside {
                // (upstream evaluates the other side with the u flag set to !low)
                let other_uv = Point2::new(uv0.x, if low { v_range.max() } else { v_range.min() });
                let (_, other_su, _) =
                    surface.evaluate_1st_derivatives(k, other_uv, !low, false)?;
                let other_norm = surface.evaluate_normal(k, other_uv, side(!low), side(false))?;
                let other_ang = other_norm.ccw_angle_between(other_su, crv_tan)?;
                if sector(other_ang, ang_limit_rad, low) == PointContainment::Inside {
                    let other_normal_to_crv = other_norm.angle_between(crv_tan).unwrap_or(0.0);
                    poc = if (self.normal_to_crv_ang_deg - 90.0).abs()
                        < (rad_to_deg(other_normal_to_crv) - 90.0).abs()
                    {
                        PointContainment::Inside
                    } else {
                        PointContainment::Outside
                    };
                }
            }
            return Ok(poc);
        }

        // arrive here when UV point is not on a surface domain boundary
        Ok(PointContainment::Unknown)
    }

    /// Tests whether the surface image of the cubic Hermite UV span from this drop
    /// point to `end` stays within `approx_tol_3d` of `curve` (`TestSpanAccuracy`):
    /// three interior samples must lie on surface normal lines through the curve
    /// points; spans shorter than a thousandth of their own parameter interval
    /// always pass. `uv_tol_x`, `uv_tol_y` and `do_refinement` are not used
    /// (upstream default `do_refinement = TRUE`).
    ///
    /// # Errors
    ///
    /// Those of the curve and surface evaluations.
    pub fn test_span_accuracy(
        &self,
        k: &Kernel,
        surface: &Surface,
        uv_domain: &Extent2d,
        curve: &Curve,
        end: &Self,
        approx_tol_3d: f64,
        _uv_tol_x: f64,
        _uv_tol_y: f64,
        _do_refinement: bool,
    ) -> Result<SpanAccuracy> {
        // init output
        let mut out = SpanAccuracy {
            passes: true,
            max_drop_to_surf: 0.0,
            max_approx_dev: 0.0,
            max_drop_to_surf_param: self.t,
        };

        let start = self;
        let ivl = Extent1d::from_min_max(start.t, end.t);

        // current parameter step size
        let delta_t = end.t - start.t;

        // Don't let the parameter step get too small
        if delta_t < ivl.length() / 1000.0 {
            out.passes = true;
            return Ok(out);
        }

        // Cubic Hermite approximation to UVCurve
        let hermite = HermiteCurve::from_2d(
            start.uv_pd[0],
            start.uv_pd[1] * delta_t,
            end.uv_pd[0],
            end.uv_pd[1] * delta_t,
            2,
        );

        let num_test_points: u32 = 3;
        for ii in 0..num_test_points {
            // Eval HermiteUVSpanCurve to get next sample UV point
            let normalized_t = (f64::from(ii) + 1.0) / (f64::from(num_test_points) + 1.0);
            let herm_pnt = hermite.evaluate_point(normalized_t);
            let mut uv = Point2::new(herm_pnt.x, herm_pnt.y);

            // If the UVPoint is not in the domain - check 3DDist moved when UVPoint is clamped
            if !uv_domain.contains_point(uv, EFF_ZERO) {
                let clamp_uv = uv_domain.clamp_point(uv);
                let pt_from_uv = surface.evaluate_point(k, uv)?;
                let pt_from_clamp_uv = surface.evaluate_point(k, clamp_uv)?;

                if pt_from_uv.distance(pt_from_clamp_uv) > approx_tol_3d {
                    out.passes = false;
                    return Ok(out);
                }

                // else clamp dist is small - use the clamp sUV value
                uv = clamp_uv;
            }

            // get SpanSmp[CrvPt]
            let crv = curve.evaluate(k, start.t + normalized_t * delta_t, 1, true, true)?;
            let (crv_pnt, crv_tan) = match (crv.first(), crv.get(1)) {
                (Some(p), Some(d)) => (*p, *d),
                _ => {
                    return Err(SolidError::sm_err(
                        "intersect::surface_drop_lib::DropPt::test_span_accuracy",
                    ))
                }
            };

            // test SpanSmp[CrvPnt/SurfaceNormal dist] against sApproxTol3d
            let (is_good, drop_to_surf, approx_dev) = match is_point_on_normal(
                k,
                surface,
                uv_domain,
                uv,
                crv_pnt,
                crv_tan,
                approx_tol_3d,
            ) {
                Ok(r) => (r.is_good, r.drop_to_surf, r.approx_dev),
                Err(e) if e.is_unported() => return Err(e),
                // when smsurf_IsPointOnNormal fails - it's not a good point
                Err(_) => (false, 0.0, 0.0),
            };

            // When any point is bad - whole span fails check
            if !is_good {
                out.passes = false;
                break;
            }
            if drop_to_surf > out.max_drop_to_surf {
                out.max_drop_to_surf = drop_to_surf;
                out.max_drop_to_surf_param = start.t + normalized_t * delta_t;
            }
            if approx_dev > out.max_approx_dev {
                out.max_approx_dev = approx_dev;
            }
        }

        Ok(out)
    }

    /// Corrects a seam-side choice of `end.uv_pd[0]` when the step from this drop point
    /// jumps more than half the domain across a seam of a closed surface and the
    /// snapped point stays within `approx_tol_3d` in 3D (`CheckSeamJump`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Upstream`] with [`SM_ERR_OUTSIDE_OF_DOMAIN`] for a seam jump that
    /// cannot be snapped within tolerance (`end` unchanged); those of the surface
    /// evaluations.
    pub fn check_seam_jump(
        &self,
        k: &Kernel,
        surface: &Surface,
        srf_closure: SurfParamType,
        approx_tol_3d: f64,
        end: &mut Self,
    ) -> Result<()> {
        // no work - not a closed surface
        if srf_closure == SurfParamType::Neither {
            return Ok(());
        }

        let start = self;
        let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
        let outside = || SolidError::Upstream {
            operation: OP_SEAM,
            code: SM_ERR_OUTSIDE_OF_DOMAIN,
            entity: None,
        };

        // Check u-direction when srf is closed in U and span length > 1/2 DomainLength
        let u_domain = uv_domain.u_interval();
        if (srf_closure == SurfParamType::U || srf_closure == SurfParamType::Both)
            && (end.uv_pd[0].x - start.uv_pd[0].x).abs() > u_domain.length() / 2.0
        {
            let (start_3d, start_su, start_sv) = surface.evaluate_1st_derivatives(
                k,
                start.uv_pd[0],
                start.from_left_u != Some(false),
                start.from_left_v != Some(false),
            )?;
            let end_3d = surface.evaluate_point(k, end.uv_pd[0])?;
            let step_3d = end_3d - start_3d;

            // Compare the steps perpendicular to the U seam
            let across_step_3d = step_3d.cross(start_sv);
            let delta_u = end.uv_pd[0].x - start.uv_pd[0].x;
            let across_expected_step_3d = delta_u * start_su.cross(start_sv);
            let numer = across_step_3d.dot(across_expected_step_3d);
            let denom = across_step_3d.dot(across_step_3d);

            // when span jumps seam - snap back to domain
            if numer < denom * -2.0 {
                let mut snapped_uv = end.uv_pd[0];
                snapped_uv.x = if end.uv_pd[0].x > u_domain.mid() {
                    u_domain.min()
                } else {
                    u_domain.max()
                };
                let snapped_point = surface.evaluate_point(k, snapped_uv)?;
                if snapped_point.distance(end_3d) <= approx_tol_3d {
                    end.uv_pd[0] = snapped_uv;
                } else {
                    return Err(outside());
                }
            }
        }

        // Check v-direction when srf is closed in V and span length > 1/2 DomainLength
        let v_domain = uv_domain.v_interval();
        if (srf_closure == SurfParamType::V || srf_closure == SurfParamType::Both)
            && (end.uv_pd[0].y - start.uv_pd[0].y).abs() > v_domain.length() / 2.0
        {
            let (start_3d, start_su, start_sv) = surface.evaluate_1st_derivatives(
                k,
                start.uv_pd[0],
                start.from_left_u != Some(false),
                start.from_left_v != Some(false),
            )?;
            let end_3d = surface.evaluate_point(k, end.uv_pd[0])?;
            let step_3d = end_3d - start_3d;

            // Compare only motion across the V seam, removing the Su direction.
            let across_step_3d = step_3d.cross(start_su);
            let delta_v = end.uv_pd[0].y - start.uv_pd[0].y;
            let across_expected_step_3d = delta_v * start_sv.cross(start_su);
            let numer = across_step_3d.dot(across_expected_step_3d);
            let denom = across_step_3d.dot(across_step_3d);

            if numer < denom * -2.0 {
                let mut snapped_uv = end.uv_pd[0];
                snapped_uv.y = if end.uv_pd[0].y > v_domain.mid() {
                    v_domain.min()
                } else {
                    v_domain.max()
                };
                let snapped_point = surface.evaluate_point(k, snapped_uv)?;
                if snapped_point.distance(end_3d) <= approx_tol_3d {
                    end.uv_pd[0] = snapped_uv;
                } else {
                    return Err(outside());
                }
            }
        }

        Ok(())
    }

    /// When this drop point and `end` both lie on the same domain boundary (within
    /// `uv_tol_x` / `uv_tol_y`) and the step runs mostly along it, zeroes the UV
    /// tangent component across that boundary in both (`FixBoundaryTangents`).
    /// Returns whether anything changed (`rbFixBTChange`). `approx_tol_3d` is not
    /// used.
    pub fn fix_boundary_tangents(
        &mut self,
        uv_domain: &Extent2d,
        end: &mut Self,
        uv_tol_x: f64,
        uv_tol_y: f64,
        _approx_tol_3d: f64,
    ) -> bool {
        let mut changed = false;
        let start = self;
        let u_step = (end.uv_pd[0].x - start.uv_pd[0].x).abs();
        let v_step = (end.uv_pd[0].y - start.uv_pd[0].y).abs();
        let min = uv_domain.min();
        let max = uv_domain.max();

        // SpanStart and SpanEnd DropUVPts on UMin bdry - moving in V dir
        if are_same_to_tol(start.uv_pd[0].x, min.x, uv_tol_x)
            && are_same_to_tol(end.uv_pd[0].x, min.x, uv_tol_x)
            && v_step > u_step
        {
            start.uv_pd[1].x = 0.0;
            end.uv_pd[1].x = 0.0;
            changed = true;
        }

        // SpanStart and SpanEnd DropUVPts on UMax bdry - moving in V dir
        if are_same_to_tol(start.uv_pd[0].x, max.x, uv_tol_x)
            && are_same_to_tol(end.uv_pd[0].x, max.x, uv_tol_x)
            && v_step > u_step
        {
            start.uv_pd[1].x = 0.0;
            end.uv_pd[1].x = 0.0;
            changed = true;
        }

        // SpanStart and SpanEnd DropUVPts on VMin bdry - moving in U dir
        if are_same_to_tol(start.uv_pd[0].y, min.y, uv_tol_y)
            && are_same_to_tol(end.uv_pd[0].y, min.y, uv_tol_y)
            && u_step > v_step
        {
            start.uv_pd[1].y = 0.0;
            end.uv_pd[1].y = 0.0;
            changed = true;
        }

        // SpanStart and SpanEnd DropUVPts on VMax bdry - moving in U dir
        if are_same_to_tol(start.uv_pd[0].y, max.y, uv_tol_y)
            && are_same_to_tol(end.uv_pd[0].y, max.y, uv_tol_y)
            && u_step > v_step
        {
            start.uv_pd[1].y = 0.0;
            end.uv_pd[1].y = 0.0;
            changed = true;
        }

        changed
    }
}

// ---- span stack ---------------------------------------------------------------------------

/// Pushes `[t, u, v, du, dv]` (a curve parameter, a UV point and a UV tangent) onto
/// `stack` (`sm_PushPoint`).
pub fn push_point(t: f64, uv: [Point2; 2], stack: &mut Vec<f64>) {
    let [p, d] = uv;
    stack.push(t); // param
    stack.push(p.x); // U position value
    stack.push(p.y); // V position value
    stack.push(d.x); // dU tangent coordinate value
    stack.push(d.y); // dV tangent coordinate value
}

/// The top `[t, u, v, du, dv]` tuple of `stack`.
fn top_of(stack: &[f64], op: &'static str) -> Result<(f64, [Point2; 2])> {
    match stack.len().checked_sub(5).and_then(|i| stack.get(i..)) {
        Some([t, u, v, du, dv]) => Ok((*t, [Point2::new(*u, *v), Vec2::new(*du, *dv)])),
        _ => Err(SolidError::sm_err(op)),
    }
}

/// Pops the top `[t, u, v, du, dv]` tuple of `stack` (`sm_PopStack`).
///
/// # Errors
///
/// `SM_ERR` (stack unchanged) when it holds fewer than five values.
pub fn pop_stack(stack: &mut Vec<f64>) -> Result<(f64, [Point2; 2])> {
    let r = top_of(stack, OP_POP)?;
    stack.truncate(stack.len() - 5);
    Ok(r)
}

/// The top `[t, u, v, du, dv]` tuple of `stack`, left in place (`sm_StackLook`).
///
/// # Errors
///
/// `SM_ERR` when it holds fewer than five values.
pub fn stack_look(stack: &[f64]) -> Result<(f64, [Point2; 2])> {
    top_of(stack, OP_LOOK)
}

// ---- control polygon --------------------------------------------------------------------

/// Keeps the piecewise cubic Bezier UV curve of `ctrl_poly` (knot points at every
/// third point) inside `uv_domain` (`sm_FixBoundaryCurves`): knot points outside are
/// snapped to the boundary; the neighbours of an interior knot point on a boundary
/// (within `uv_tol_x` / `uv_tol_y`) move onto it; the inner neighbour of an end
/// point on a boundary is clamped into the domain. Returns whether a boundary
/// adjustment was made (`rbFixBCChange` is only ever set); fewer than four points
/// change nothing.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for `3k` points (`k >= 2`), which upstream reads past
/// (see the module notes).
pub fn fix_boundary_curves(
    ctrl_poly: &mut [Point3],
    uv_domain: &Extent2d,
    uv_tol_x: f64,
    uv_tol_y: f64,
) -> Result<bool> {
    let num_pts = ctrl_poly.len();

    // curve is expected to have at least one piecewise cubic segment
    if num_pts < 4 {
        return Ok(false);
    }

    // span count
    let num_spans = num_pts / 3;
    if 3 * num_spans >= num_pts {
        return Err(SolidError::invalid_input(
            OP_FIX_CURVES,
            "control polygon of 3k points reads past its end",
        ));
    }

    let u0 = uv_domain.min().x;
    let v0 = uv_domain.min().y;
    let u1 = uv_domain.max().x;
    let v1 = uv_domain.max().y;
    let u_ivl = uv_domain.u_interval();
    let v_ivl = uv_domain.v_interval();

    let bad = || SolidError::invalid_input(OP_FIX_CURVES, "control point index");
    let mut changed = false;

    // for every curve knot
    for ii in 0..=num_spans {
        let this_pt = 3 * ii;
        let mut knot = *ctrl_poly.get(this_pt).ok_or_else(bad)?;

        // snap KnotPts outside of domain to domain boundary
        if knot.x < u0 {
            knot.x = u0;
        }
        if knot.x > u1 {
            knot.x = u1;
        }
        if knot.y < v0 {
            knot.y = v0;
        }
        if knot.y > v1 {
            knot.y = v1;
        }
        *ctrl_poly.get_mut(this_pt).ok_or_else(bad)? = knot;

        // when interior KnotPoint is on boundary - move adjacent points to the bdry
        if ii > 0 && ii < num_spans {
            let (prev, next) = (this_pt - 1, this_pt + 1);
            if knot.x < u0 + uv_tol_x || knot.x > u1 - uv_tol_x {
                ctrl_poly.get_mut(prev).ok_or_else(bad)?.x = knot.x;
                ctrl_poly.get_mut(next).ok_or_else(bad)?.x = knot.x;
                changed = true;
            }
            if knot.y < v0 + uv_tol_y || knot.y > v1 - uv_tol_y {
                ctrl_poly.get_mut(prev).ok_or_else(bad)?.y = knot.y;
                ctrl_poly.get_mut(next).ok_or_else(bad)?.y = knot.y;
                changed = true;
            }
        }

        if ii == 0 {
            // make sure its neighbour is in/on the boundary so the start 1stDeriv
            // does not go out of the domain
            let next = ctrl_poly.get_mut(this_pt + 1).ok_or_else(bad)?;
            if knot.x < u0 + uv_tol_x || knot.x > u1 - uv_tol_x {
                next.x = u_ivl.clamp_value(next.x);
                changed = true;
            }
            if knot.y < v0 + uv_tol_y || knot.y > v1 - uv_tol_y {
                next.y = v_ivl.clamp_value(next.y);
                changed = true;
            }
        } else if ii == num_spans {
            // make sure its neighbour is in/on the boundary so the end -1stDeriv
            // does not go out of the domain
            let prev = ctrl_poly.get_mut(this_pt - 1).ok_or_else(bad)?;
            if knot.x < u0 + uv_tol_x || knot.x > u1 - uv_tol_x {
                prev.x = u_ivl.clamp_value(prev.x);
                changed = true;
            }
            if knot.y < v0 + uv_tol_y || knot.y > v1 - uv_tol_y {
                prev.y = v_ivl.clamp_value(prev.y);
                changed = true;
            }
        }
    }

    Ok(changed)
}

/// Removes the solutions whose start UV (`start.parameters[0..2]`) is within
/// (`<`) `uv_tol_x` / `uv_tol_y` of an earlier one, keeping the earlier
/// (`sm_RemoveDuplicateDropPointSolutions`).
pub fn remove_duplicate_drop_point_solutions<O: SolutionObject>(
    solutions: &mut SolutionArray<O>,
    uv_tol_x: f64,
    uv_tol_y: f64,
) {
    let uv_of = |s: &SolutionArray<O>, i: usize| {
        s.get(i)
            .map(|x| Point2::new(x.start.parameters[0], x.start.parameters[1]))
    };
    if solutions.len() > 1 {
        let mut ii = 1;
        while ii < solutions.len() {
            let Some(uv) = uv_of(solutions, ii) else {
                break;
            };
            let mut removed = false;
            for jj in (0..ii).rev() {
                let Some(uv2) = uv_of(solutions, jj) else {
                    continue;
                };
                if (uv.x - uv2.x).abs() < uv_tol_x && (uv.y - uv2.y).abs() < uv_tol_y {
                    // remove sol last from ordered solution array
                    solutions.remove_at(ii, 1);
                    removed = true;
                    break;
                }
            }
            if !removed {
                ii += 1;
            }
        }
    }
}

// ---- DropAndTrimCurve / DropIsoCurve ------------------------------------------------------

/// The failure record `DropAndTrimCurve` fills (`pOptDropCurveFail`) and the handles
/// it records for the surface and the curve (`this`, `&cr3dCurve`).
#[derive(Debug)]
pub struct DropFailRecord<'a, O> {
    /// The record.
    pub fail: &'a mut DropCurveFail<O>,
    /// The surface's handle.
    pub surface: Option<O>,
    /// The curve's handle.
    pub curve: Option<O>,
}

/// The outputs of [`Surface::drop_and_trim_curve`].
#[derive(Debug)]
pub struct DropAndTrim {
    /// `rdMaxDropToSurf`: the largest drop distance of the kept curves.
    pub max_drop_to_surf: f64,
    /// `rdMaxApproxDev`: the largest deviation from a surface normal line.
    pub max_approx_dev: f64,
    /// `rUVCurves`: one UV trim curve per piece of the curve over the surface, each
    /// of a B-spline kind (`SmBSplineCurve` or a subclass such as `SmLine`).
    pub uv_curves: Vec<Curve>,
}

/// The object handle of the surface cache `DoTrace` runs on (its solves keep no
/// objects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TraceCacheObj;

impl SolutionObject for TraceCacheObj {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Surface
    }
}

/// The outputs of `SmSurfaceDropCurve::DoTrace` used by `DropAndTrimCurve`.
struct Trace {
    uv_curves: Vec<Curve>,
    max_drop_dists: Vec<f64>,
    deviations: Vec<f64>,
}

/// `SmSurfaceDropCurve sDrop(this, domain, curve, interval);
/// sDrop.SetCurveIsOnSurface(on_surface); sDrop.DoTrace(ctx, tol, NULL, NULL, ...)`
/// on the surface's cache (`smsurf_GetSurfaceCache` inside `DoTrace`).
///
/// `cache` is the surface's cache when the caller holds it (upstream's
/// `smsurf_GetSurfaceCache` hands every caller the one cache of the surface); `None`
/// builds one.
#[allow(clippy::too_many_arguments)]
fn drop_curve_trace<C: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    cache: Option<&mut SurfaceCache<C>>,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    curve_is_on_surface: bool,
    approx_tol_3d: Option<f64>,
    context: Option<DropSolveContext<'_, C>>,
) -> Result<Trace> {
    // prepare the SurfaceDropCurve operation object
    let mut drop = SurfaceDropCurve::new(k, surface, uv_domain, curve, interval)?;
    drop.set_curve_is_on_surface(curve_is_on_surface);

    // get and checkout surface cache (NER)
    let out = if let Some(cache) = cache {
        drop.drop_trace(k, cache, approx_tol_3d, None, context)?
    } else {
        let mut cache = tracer_surface_cache::<TraceCacheObj>(k, &drop.base)?;
        drop.drop_trace(k, &mut cache, approx_tol_3d, None, None)?
    };
    Ok(Trace {
        uv_curves: out.uv_curves,
        max_drop_dists: out.max_drop_dists,
        deviations: out.deviations,
    })
}

/// `SM_CAST_PTR(SmBSplineCurve, pCurve) != NULL`: the curve is of a B-spline kind
/// (`SmBSplineCurve` or a subclass: line, ellipse, hyperbola, parabola).
const fn is_bspline_kind(curve: &Curve) -> bool {
    match curve.kind() {
        CurveKind::BSpline(_)
        | CurveKind::Line(_)
        | CurveKind::Ellipse(_)
        | CurveKind::Hyperbola(_)
        | CurveKind::Parabola(_) => true,
        CurveKind::Hermite(_)
        | CurveKind::Conic(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::CrvOnSurf(_)
        | CurveKind::TangentField(_)
        | CurveKind::Composite(_) => false,
    }
}

/// The outputs of [`Surface::drop_iso_curve`].
#[derive(Debug)]
pub struct IsoDrop {
    /// `rbSuccessfulIsoDrop`: the curve drops to an iso-parameter line.
    pub successful: bool,
    /// `rdMaxDropToSurf`: the largest sampled drop distance.
    pub max_drop_to_surf: f64,
    /// `rpIsoUVCurve`: the UV line, reparameterised to the curve interval.
    pub iso_uv_curve: Option<BSplineCurve>,
    /// `*pdMaxApproxDev`: the largest sampled deviation from a surface normal line.
    pub max_approx_dev: f64,
}

impl IsoDrop {
    const fn none() -> Self {
        Self {
            successful: false,
            max_drop_to_surf: 0.0,
            iso_uv_curve: None,
            max_approx_dev: 0.0,
        }
    }
}

impl Surface {
    /// Drops `curve` over `interval` to UV trim curves, one per piece over the surface
    /// (`DropAndTrimCurve`). With `keep_all_drop_curves` false, pieces farther than
    /// `approx_tol_3d` are dropped (upstream defaults: keep all, curve not on the
    /// surface). On success the optional `record` is set to
    /// [`CreateUvTrimCurvePathType::DropAndTrimCurveOkay`] unless a path was recorded.
    ///
    /// # Errors
    ///
    /// Those of the drop curve trace (`SmSurfaceDropCurve::DoTrace`, which defers
    /// non-plane `SmSurface::DropCurve` to `intersect.surface_drop_lib_p3`);
    /// `SM_ERR_NULL_POINTER` for a traced UV curve that is not a B-spline kind.
    pub fn drop_and_trim_curve<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        curve_is_on_surface: bool,
        record: Option<DropFailRecord<'_, O>>,
    ) -> Result<DropAndTrim> {
        self.drop_and_trim_curve_in::<O, TraceCacheObj>(
            k,
            None,
            uv_domain,
            curve,
            interval,
            approx_tol_3d,
            keep_all_drop_curves,
            curve_is_on_surface,
            record,
        )
    }

    /// [`Surface::drop_and_trim_curve`] with the trace on `cache`, this surface's
    /// cache when the caller holds it (upstream's `smsurf_GetSurfaceCache` inside
    /// `DoTrace` hands every caller the one cache of the surface, with whatever tree
    /// its holder set); `None` builds one.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::drop_and_trim_curve`].
    #[allow(clippy::too_many_arguments)]
    pub fn drop_and_trim_curve_in<O: SolutionObject, C: SolutionObject>(
        &self,
        k: &Kernel,
        cache: Option<&mut SurfaceCache<C>>,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        curve_is_on_surface: bool,
        record: Option<DropFailRecord<'_, O>>,
    ) -> Result<DropAndTrim> {
        self.drop_and_trim_curve_in_context::<O, C>(
            k,
            cache,
            uv_domain,
            curve,
            interval,
            approx_tol_3d,
            keep_all_drop_curves,
            curve_is_on_surface,
            record,
            None,
        )
    }

    /// [`Surface::drop_and_trim_curve_in`] whose trace drops under the caller's face
    /// solve `context` (`smsurf_GetSurfaceCache` inside each `DropCurve` of the trace
    /// hands back the face's cache of a face-owned surface); used with a held `cache`.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::drop_and_trim_curve_in`] and the context.
    #[allow(clippy::too_many_arguments)]
    pub fn drop_and_trim_curve_in_context<O: SolutionObject, C: SolutionObject>(
        &self,
        k: &Kernel,
        cache: Option<&mut SurfaceCache<C>>,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        curve_is_on_surface: bool,
        mut record: Option<DropFailRecord<'_, O>>,
        context: Option<DropSolveContext<'_, C>>,
    ) -> Result<DropAndTrim> {
        // init outputs
        let mut out = DropAndTrim {
            max_drop_to_surf: 0.0,
            max_approx_dev: 0.0,
            uv_curves: Vec::new(),
        };
        if let Some(r) = record.as_mut() {
            r.fail.un_init();
        }

        // trace out a UVTrimCurve by projecting cr3dCurve points onto Surface
        let trace = drop_curve_trace(
            k,
            self,
            cache,
            uv_domain,
            curve,
            interval,
            curve_is_on_surface,
            if approx_tol_3d == 0.0 {
                None
            } else {
                Some(approx_tol_3d)
            },
            context,
        )?;

        for ((uv, &max_drop), &dev) in trace
            .uv_curves
            .into_iter()
            .zip(&trace.max_drop_dists)
            .zip(&trace.deviations)
        {
            // skip curves too far from Surface when bKeepAllDropCurves is FALSE
            if !keep_all_drop_curves && max_drop > approx_tol_3d {
                continue;
            }
            // SM_CAST_PTR(SmBSplineCurve, sUVCurves[ii]): a null cast is not usable
            if !is_bspline_kind(&uv) {
                return Err(SolidError::null_pointer(OP_TRIM));
            }
            out.uv_curves.push(uv);
            out.max_drop_to_surf = max2(out.max_drop_to_surf, max_drop);
            out.max_approx_dev = max2(out.max_approx_dev, dev);
        }

        // capture the output for debug and heal purposes
        if let Some(r) = record {
            let path = r.fail.create_path_type;
            r.fail.set(&DropCurveFailArgs {
                fail_status: SM_SUCCESS,
                from_call: 2,
                create_path_type: path,
                has_context: true,
                uv_domain: self.natural_uv_domain_with_nurb(k)?,
                surface: r.surface,
                curve: r.curve,
                interval: *interval,
                approx_tol_3d,
                max_drop_to_surf_arg: out.max_drop_to_surf,
                max_approx_dev: out.max_approx_dev,
                uv_curves_arg: None,
                keep_all_drop_curves: i32::from(keep_all_drop_curves),
                degen_curve: UNSURE,
                degen_surf_u: UNSURE,
                degen_surf_v: UNSURE,
                drop_domain_is_contained: UNSURE,
                max_drop_to_surf: out.max_drop_to_surf,
                closed_u: UNSURE,
                closed_v: UNSURE,
                srf_closure: SurfParamType::Unknown,
                ..DropCurveFailArgs::default()
            });
            if r.fail.create_path_type == CreateUvTrimCurvePathType::Unknown {
                r.fail.create_path_type = CreateUvTrimCurvePathType::DropAndTrimCurveOkay;
            }
        }

        Ok(out)
    }

    /// When `curve` over `interval` drops to an iso-parameter line of this surface
    /// starting at `start_uv` with the same parameterisation, the UV line segment
    /// (`DropIsoCurve`): the start tangent must be parallel (within a degree) to a
    /// first derivative, and the curve points at evenly spaced parameters must lie
    /// within `approx_tol_3d` of the surface normal lines along that line.
    ///
    /// # Errors
    ///
    /// Those of the curve and surface evaluations, `AngleBetween` (a curve tangent
    /// normal to the surface) and `CreateLineSegment`.
    pub fn drop_iso_curve(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        start_uv: Point2,
        approx_tol_3d: f64,
    ) -> Result<IsoDrop> {
        let one_deg = 1.0 * PI / 180.0;

        // Check Curve Tangent == one of Surface 1st parametric derivatives
        let left_t = interval.t_left_eval(interval.min());
        let mut pv = curve.evaluate(k, interval.min(), 1, left_t, true)?;
        let tan = |pv: &[Vec3]| pv.get(1).copied().unwrap_or(Vec3::ZERO);

        // when curve StartTangent == 0.0 look for nonDegenerate tangent near StartPoint
        if tan(&pv).length_squared() < EFF_ZERO_SQ {
            pv = curve.evaluate(k, interval.evaluate(EFF_ZERO_SQRT), 1, left_t, true)?;
            if tan(&pv).length_squared() < EFF_ZERO_SQ {
                pv = curve.evaluate(k, interval.evaluate(1.0e-3), 1, left_t, true)?;
                if tan(&pv).length_squared() < EFF_ZERO_SQ {
                    // could not find a nonZero tangent near startPoint - quit
                    return Ok(IsoDrop::none());
                }
            }
        }
        let crv_tan = tan(&pv);

        let mut du = Vec3::ZERO;
        let mut dv = Vec3::ZERO;
        let mut dir_vec = Vec2::new(0.0, 0.0);
        let mut found = false;
        let mut scale = 1.0;

        // for 4 possible surface directions
        for kk in 0..4 {
            let left_u = !(kk == 2 || kk == 3);
            let left_v = !(kk == 1 || kk == 2);

            // evaluate the Surface 1st derivatives
            let (_, su, sv) = self.evaluate_1st_derivatives(k, start_uv, left_u, left_v)?;
            du = su;
            dv = sv;

            // Project crv tan vectors to srf plane before checking angles.
            let mut norm = du.cross(dv);
            let norm_len = norm.length();
            if norm_len > EFF_ZERO {
                norm /= norm_len;
            } else {
                norm = self.evaluate_normal(k, start_uv, side(left_u), side(left_v))?;
            }
            let crv_tan_proj = crv_tan.project_to_plane(norm);

            // get Curve/Surface tangent angles
            let mut ang_u = PI / 2.0;
            let mut ang_v = PI / 2.0;
            if du.length_squared() > EFF_ZERO_SQ {
                ang_u = du.angle_between(crv_tan_proj)?;
            }
            if dv.length_squared() > EFF_ZERO_SQ {
                ang_v = dv.angle_between(crv_tan_proj)?;
            }

            // Note following effectively checks for derivatives being in the same direction.
            if ang_u < one_deg {
                (_, du, dv) = self.evaluate_1st_derivatives(k, start_uv, true, true)?;
                scale = du.length() / crv_tan.length();
                dir_vec = Vec2::new(1.0, 0.0);
                found = true;
                break;
            } else if ang_u > PI - one_deg {
                (_, du, dv) = self.evaluate_1st_derivatives(k, start_uv, false, false)?;
                scale = du.length() / crv_tan.length();
                dir_vec = Vec2::new(-1.0, 0.0);
                found = true;
                break;
            } else if ang_v < one_deg {
                (_, du, dv) = self.evaluate_1st_derivatives(k, start_uv, true, true)?;
                scale = dv.length() / crv_tan.length();
                dir_vec = Vec2::new(0.0, 1.0);
                found = true;
                break;
            } else if ang_v > PI - one_deg {
                (_, du, dv) = self.evaluate_1st_derivatives(k, start_uv, false, false)?;
                scale = dv.length() / crv_tan.length();
                dir_vec = Vec2::new(0.0, -1.0);
                found = true;
                break;
            }
        }

        // when curveTangent is not parallel to a surface 1st Derivative - quit
        if !found {
            return Ok(IsoDrop::none());
        }

        // get Curve knots - trim them to the input interval range
        let (mut knots, _) = curve.knots(k, None)?;
        while knots
            .first()
            .is_some_and(|&k| k < interval.min() - EFF_ZERO_SQRT)
        {
            knots.remove(0);
        }
        while knots
            .last()
            .is_some_and(|&k| k > interval.max() + EFF_ZERO_SQRT)
        {
            knots.pop();
        }

        // make sure to have a knot at the crInterval Min and Max boundaries
        if knots.first().is_none_or(|&k| !are_same(k, interval.min())) {
            knots.insert(0, interval.min());
        }
        if knots.len() == 1 || knots.last().is_some_and(|&k| !are_same(k, interval.max())) {
            knots.push(interval.max());
        }

        // for every Curve span between knots
        let ivl_size = interval.max() - interval.min();
        let mut max_deviation = 0.0;
        let mut max_drop = 0.0;
        let num_knots = knots.len();
        for i in 1..=num_knots {
            // get Surface UVPoint at even sized param steps along surface isoParameter Curve
            let step = if i < num_knots {
                (i as f64 / num_knots as f64) * ivl_size
            } else {
                ivl_size
            };
            let uv_test = start_uv + dir_vec * step / scale;
            let uv_test = uv_domain.clamp_point(uv_test);

            // Clamp dParam to CurveInterval to account for tolerances
            let param = interval.clamp_value(interval.min() + step);

            // get curve/surfaceNormal gap for parameter points
            let c_pnt = curve.evaluate_point(k, param)?;
            let s_pnt = self.evaluate_point(k, uv_test)?;
            let s_nrm = self.evaluate_normal(k, uv_test, Side::Left, Side::Left)?;
            let (drop_line_dist, mut line_param) = line_point_distance(s_pnt, s_nrm, c_pnt);

            // when gap is too large - quit
            if drop_line_dist > approx_tol_3d {
                return Ok(IsoDrop::none());
            }

            // save the largest gap seen
            if line_param < 0.0 {
                line_param = -line_param;
            }
            if max_deviation < drop_line_dist {
                max_deviation = drop_line_dist;
            }
            if max_drop < line_param {
                max_drop = line_param;
            }
        }

        // get the surface parameter start/end points
        let uv_start = Point3::from(start_uv);
        let uv_end = Point3::from(uv_domain.clamp_point(start_uv + dir_vec * ivl_size / scale));

        // skip degenerate parameter space curves
        let uv_size = uv_start.distance(uv_end);
        if uv_size < EFF_ZERO_SQRT
            && (du.length_squared() <= EFF_ZERO || dv.length_squared() <= EFF_ZERO)
        {
            return Ok(IsoDrop::none());
        }

        // Passed all tests - make and output a parameter space line.
        let mut line = BSplineCurve::create_line_segment(2, uv_start, uv_end)?;
        line.edit_parameterization(interval)?;
        Ok(IsoDrop {
            successful: true,
            max_drop_to_surf: max_drop,
            iso_uv_curve: Some(line),
            max_approx_dev: max_deviation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn new_is_uninitialised() {
        let p = DropPt::new(0.5, Some(false));
        assert_eq!(p.t, 0.5);
        assert_eq!(p.degen_curve, Some(false));
        assert_eq!(p.good_drop, None);
        assert!(p.crv_pd[2].is_undef());
        assert_eq!(p.drop_deriv_rtn, BIG_ULONG);
        let mut q = DropPt::new(1.0, None);
        q.assign(&p);
        assert_eq!(q, p);
    }

    #[test]
    fn good_tangent_drop_limits() {
        let mut p = DropPt::new(0.0, None);
        p.srf_to_crv_ang_deg = 5.0;
        p.srf_to_crv_speed_ratio = 1.0;
        assert!(!p.is_good_tangent_drop(Some(false)));
        assert!(p.is_good_tangent_drop(Some(true)));
        assert!(p.is_good_tangent_drop(None));
    }

    #[test]
    fn stack_round_trip() {
        let mut s = Vec::new();
        assert!(pop_stack(&mut s).is_err());
        push_point(1.0, [Point2::new(2.0, 3.0), Vec2::new(4.0, 5.0)], &mut s);
        push_point(6.0, [Point2::new(7.0, 8.0), Vec2::new(9.0, 10.0)], &mut s);
        let (t, uv) = stack_look(&s).expect("look");
        assert_eq!((t, uv[0].x, uv[1].y), (6.0, 7.0, 10.0));
        assert_eq!(s.len(), 10);
        let (t, _) = pop_stack(&mut s).expect("pop");
        assert_eq!(t, 6.0);
        let (t, _) = pop_stack(&mut s).expect("pop");
        assert_eq!(t, 1.0);
        assert!(s.is_empty());
    }

    #[test]
    fn fix_boundary_curves_snaps() {
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut poly = vec![
            Point3::new(-0.1, 0.5, 0.0),
            Point3::new(-0.2, 0.6, 0.0),
            Point3::new(0.3, 0.7, 0.0),
            Point3::new(0.5, 0.5, 0.0),
        ];
        assert!(fix_boundary_curves(&mut poly, &d, 1e-9, 1e-9).expect("fix"));
        assert_eq!(poly[0].x, 0.0);
        assert_eq!(poly[1].x, 0.0);
        let mut bad = vec![Point3::ZERO; 6];
        assert!(fix_boundary_curves(&mut bad, &d, 1e-9, 1e-9).is_err());
        let mut short = vec![Point3::ZERO; 3];
        assert!(!fix_boundary_curves(&mut short, &d, 1e-9, 1e-9).expect("short"));
    }

    #[test]
    fn fix_boundary_tangents_on_umin() {
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut a = DropPt::new(0.0, None);
        let mut b = DropPt::new(1.0, None);
        a.uv_pd = [Point2::new(0.0, 0.1), Vec2::new(0.2, 1.0)];
        b.uv_pd = [Point2::new(0.0, 0.9), Vec2::new(0.3, 1.0)];
        assert!(a.fix_boundary_tangents(&d, &mut b, 1e-9, 1e-9, 1e-6));
        assert_eq!(a.uv_pd[1].x, 0.0);
        assert_eq!(b.uv_pd[1].x, 0.0);
    }

    #[test]
    fn moving_along_plane_boundary() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut p = DropPt::new(0.0, None);
        p.uv_pd[0] = Point2::new(0.0, 0.5);
        p.crv_pd[1] = Vec3::new(0.0, 1.0, 0.0);
        p.normal_to_crv_ang_deg = 90.0;
        let r = p
            .is_moving_along_boundary(k, &s, &d, 1e-9, 1e-9, 5.0, SurfParamType::Both)
            .expect("along");
        assert_eq!(r, PointContainment::OnBoundary);
        p.uv_pd[0] = Point2::new(0.5, 0.5);
        let r = p
            .is_moving_along_boundary(k, &s, &d, 1e-9, 1e-9, 5.0, SurfParamType::Both)
            .expect("interior");
        assert_eq!(r, PointContainment::Unknown);
    }

    #[test]
    fn iso_drop_on_plane() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let c = Curve::from(crate::curve::line::Line::from_points(
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::new(1.0, 0.5, 0.0),
            3,
        ));
        let r = s
            .drop_iso_curve(
                k,
                &d,
                &c,
                &Extent1d::from_min_max(0.0, 1.0),
                Point2::new(0.0, 0.5),
                1e-6,
            )
            .expect("iso");
        assert!(r.successful);
        assert!(r.iso_uv_curve.is_some());
    }

    #[test]
    fn drop_and_trim_iso_line() {
        // the flat bilinear patch P(u, v) = (u, v, 0) and the u-iso line v = 0.3
        let w =
            |x: f64, y: f64| crate::nurbs::math::CPoint::new(x, y, 0.0, crate::nurbs::math::NOW);
        let net = vec![
            vec![w(0.0, 0.0), w(0.0, 1.0)],
            vec![w(1.0, 0.0), w(1.0, 1.0)],
        ];
        let kn = vec![0.0, 0.0, 1.0, 1.0];
        let n = crate::nurbs::srf_basic::Surface::new(net, 1, 1, kn.clone(), kn).unwrap();
        let s = Surface::from(crate::nurbs::b_spline_surface::BSplineSurface::from_nurb(
            n, false,
        ));
        let c = Curve::from(crate::curve::line::Line::from_points(
            Vec3::new(0.0, 0.3, 0.0),
            Vec3::new(1.0, 0.3, 0.0),
            3,
        ));
        let r = s.drop_and_trim_curve::<crate::topology::ObjectHandle>(
            &Kernel::default(),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            &c,
            &Extent1d::from_min_max(0.0, 1.0),
            1e-6,
            true,
            true,
            None,
        );
        let r = r.expect("drop and trim");
        assert_eq!(r.uv_curves.len(), 1);
        assert!(r.max_drop_to_surf < 1e-9);
        assert!(r.max_approx_dev < 1e-9);
        let uv = &r.uv_curves[0];
        let ivl = uv.natural_interval();
        let a = uv
            .evaluate_point(&Kernel::default(), ivl.min())
            .expect("start");
        let b = uv
            .evaluate_point(&Kernel::default(), ivl.max())
            .expect("end");
        assert!(
            (a.x - 0.0).abs() < 1e-9 && (a.y - 0.3).abs() < 1e-9,
            "{a:?}"
        );
        assert!(
            (b.x - 1.0).abs() < 1e-9 && (b.y - 0.3).abs() < 1e-9,
            "{b:?}"
        );
    }
}
