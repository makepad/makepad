//! The surface drop-curve tracer: drops a 3D curve lying on or near a surface onto
//! that surface (curve break points at surface boundaries and discontinuities,
//! iso-line drops, per-interval `DropCurve` with the surface tracer as fallback).
//!
//! Upstream: `source/SMLib/src/SmSurfaceDropCurve.cpp`, `source/SMLib/inc/SmSurfaceDropCurve.h`.
//!
//! | Rust                                                        | Upstream                                         |
//! |-------------------------------------------------------------|--------------------------------------------------|
//! | [`SurfaceDropCurve`], [`SurfaceDropCurve::new`]             | `SmSurfaceDropCurve`, `SmSurfaceDropCurve(cpSurface, crUVDomain, crCurveToDrop, crInterval)` |
//! | `Drop` (implicit)                                           | `~SmSurfaceDropCurve`                            |
//! | [`SurfaceDropCurve::curve`], [`SurfaceDropCurve::interval`], [`SurfaceDropCurve::max_dist_to_surface`], [`SurfaceDropCurve::curve_is_on_surface`] | `GetCurve`, `GetInterval`, `GetMaxDistToSurface`, `GetCurveIsOnSurface` |
//! | [`SurfaceDropCurve::set_curve_is_on_surface`]               | `SetCurveIsOnSurface`                            |
//! | [`SurfaceDropCurve::compute_point_values_with_prev`] ([`SurfaceTrace::compute_point_values`]) | `SmSurfaceDropCurve::ComputePointValues` |
//! | [`SurfaceDropCurve::step_size`] ([`SurfaceTrace::compute_step_size`]) | `SmSurfaceDropCurve::ComputeStepSize`  |
//! | [`SurfaceDropCurve::interior_curves`] ([`SurfaceTrace::find_interior_curves`]) | `SmSurfaceDropCurve::FindInteriorCurves` |
//! | [`test_point`], [`PointTest`]                               | `sm_TestPoint` (static)                          |
//! | [`SurfaceDropCurve::drop_local_point_solve`] ([`SurfaceTrace::local_point_solve`]) | `SmSurfaceDropCurve::LocalPointSolve` |
//! | [`add_break`]                                               | `sm_AddBreak`                                    |
//! | [`is_grazing_intersection`]                                 | `sm_IsGrazingIntersection` (static)              |
//! | [`SurfaceDropCurve::boundary_start_points`] ([`SurfaceTrace::find_boundary_start_points`]) | `SmSurfaceDropCurve::FindBoundaryStartPoints` |
//! | [`SurfaceDropCurve::drop_trace`] ([`SurfaceTrace::do_trace`]) | `SmSurfaceDropCurve::DoTrace`                  |
//! | [`SurfaceDropCurve::trace_curve_2`]                         | `SmSurfaceDropCurve::TraceCurve2`                |
//! | [`SurfaceTrace::is_point_on_curve`] for [`SurfaceDropCurve`] | `SmSurfaceDropCurve::IsPointOnCurve`            |
//! | [`SurfaceDropCurve::next_point`] ([`SurfaceTrace::compute_next_point`]) | `SmSurfaceDropCurve::ComputeNextPoint` |
//! | [`SurfaceTrace::test_point`] for [`SurfaceDropCurve`]       | `SmSurfaceDropCurve::TestPoint`                  |
//! | [`SurfaceDropCurve::span_deviation`] ([`SurfaceTrace::compute_span_deviation`]) | `SmSurfaceDropCurve::ComputeSpanDeviation` |
//! | [`TYPE`], [`TYPE_STRING`], [`is_kind_of`]                   | `GetType`, `GetTypeString`, `IsKindOf` (`SM_COMMON`) |
//!
//! Deviations:
//! - **Virtual methods.** The drop curve owns a [`SurfaceTracer`] (the base members,
//!   [`SurfaceDropCurve::base`]) and implements [`SurfaceTrace`], overriding the
//!   hooks upstream overrides; the bodies are inherent methods so they can be called
//!   directly. `TraceCurve2` calls the base `SmSurfaceTracer::TraceCurve`
//!   ([`trace_curve_base`]), whose hooks dispatch back here, as upstream.
//! - **Private state.** `m_vWorkingInterval`, `m_vWorkingUVDomain`,
//!   `m_pCurveKnotVector` and `m_pCurveBreaks` are public fields (`None` for the null
//!   arrays); the working interval and domain start "inside out" as
//!   `SmExtent1d()`/`SmExtent2d()` leave them. `ComputeStepSize` without a knot
//!   vector and `TraceCurve2` without breaks (null dereferences upstream) are
//!   `SM_ERR_NULL_POINTER`; `ComputeStepSize` with an empty knot vector (an index
//!   underflow upstream) is `InvalidInput`.
//! - **Previous point.** `ComputePointValues` writes the snapped UV tangent back into
//!   `pPrevPnt`: [`SurfaceDropCurve::compute_point_values_with_prev`] takes it
//!   mutably; the trait hook (whose base callers never pass a previous point) works
//!   on a copy. [`SurfaceDropCurve::next_point`] writes its changes of the current
//!   point back into the tracer's arena, as upstream changes `rTracePnt` in place.
//! - **Out-params** become return values: `LocalPointSolve`'s found flag and point
//!   are an `Option<Point2>`, `sm_TestPoint`'s outputs a [`PointTest`],
//!   `ComputeNextPoint`'s a [`NextPoint`] (deviation and angle stay `0` when not
//!   computed), `ComputeSpanDeviation`'s deviation is `None` when the span fails
//!   (the output is then left alone upstream). Output arrays are a [`TraceOutput`].
//! - **Context and cache.** `m_cpContext` has no counterpart (curves are values);
//!   `smsurf_GetSurfaceCache` is the cache passed to
//!   [`SurfaceTrace::find_boundary_start_points`] and [`SurfaceTrace::do_trace`] (see
//!   [`crate::intersect::surface_tracer`]); `SmCacheCheckOutIn` has no counterpart,
//!   the cache flags `DoTrace` changes are restored on every exit. The curve cache
//!   of `GetOrCreateObjectCache(SM_OC_CURVE)` is built fresh
//!   ([`make_curve_cache`]); a failed build is `SM_ERR_NULL_POINTER` (`NER`).
//! - **Ignored statuses** stay ignored: the curve `GetKnots` into the knot vector
//!   (an empty vector on failure), the iso curve start evaluation, the grazing test
//!   evaluations and the UV curve end evaluations (upstream then reads
//!   uninitialised vectors; the port uses zero vectors), `DropIsoCurve`'s first call
//!   (`SE`: the candidate is skipped), `DropCurve` in `TraceCurve2` (the trace falls
//!   back to the tracer) and the base trace there. `IsClosed` and `IsDegenerate`
//!   answer `false` on failure. [`SolidError::Unported`] is always propagated.
//! - **Solutions.** The surface point solves give the start parameters of their
//!   solutions as `Point2`s. The curve solves of `FindBoundaryStartPoints` keep
//!   no objects (upstream nulls the extinct `SmCrvOnSurf` object of each solution).
//! - **Upstream quirk kept.** `FindBoundaryStartPoints` measures the L1 distance
//!   from a UV point to the x and y of the last 3D sample point when picking the
//!   seam-side solution, as upstream does.
//! - **Unported callees** return [`SolidError::Unported`] (see the `UNPORTED`
//!   markers). `SmSurface::DropCurve` is the plane override for planes and the base
//!   body ([`Surface::drop_curve_base`]) otherwise. The surface point solves defer
//!   inside their own units
//!   (`LocalPointSolve_1`, `GlobalPointSolve_1`).
//! - The `SM_DEBUG_CODE` drawing blocks and `Dump` are not ported. No recursion
//!   depends on input; the seam search loop is bounded (19 steps) as upstream.

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::knots_ignoring_status;
use crate::curve::curve::p5::make_curve_cache;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::p9::CurveSolveOptions;
use crate::curve::curve::Curve;
use crate::curve::hermite_curve::HermiteCurve;
use crate::curve::line::Line;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject, SolutionType};
use crate::geom::tol::is_zero;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::surface_drop_lib::p3::DropSolveContext;
use crate::intersect::surface_tracer::{
    flush_curve, trace_curve_base, NextPoint, SurfaceTrace, SurfaceTracer, TraceOutput, TracePoint,
    TracePointId, TracePointType,
};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_SQ, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_surface::{IsoCurveOptions, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::drop_vectors;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::surface_cache::SurfaceCache;

/// `SmSurfaceDropCurve_TYPE` (`SOLV_BASE_TYPE + 60`).
pub const TYPE: i64 = 19_060;
/// `"SmSurfaceDropCurve_TYPE"`.
pub const TYPE_STRING: &str = "SmSurfaceDropCurve_TYPE";

/// `NUM_TEST_POINTS` of `ComputeSpanDeviation`.
const NUM_TEST_POINTS: usize = 5;

const OP_CPV: &str = "intersect::surface_drop_curve::SurfaceDropCurve::compute_point_values";
const OP_STEP: &str = "intersect::surface_drop_curve::SurfaceDropCurve::step_size";
const OP_BOUNDARY: &str = "intersect::surface_drop_curve::SurfaceDropCurve::boundary_start_points";
const OP_TRACE2: &str = "intersect::surface_drop_curve::SurfaceDropCurve::trace_curve_2";

/// `SmSurfaceDropCurve::IsKindOf`: the drop curve type or a base type
/// (`SmSurfaceTracer::IsKindOf`).
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || crate::intersect::surface_tracer::is_kind_of(t)
}

/// `smos_Fabs`.
fn fabs(a: f64) -> f64 {
    if a < 0.0 {
        -a
    } else {
        a
    }
}

/// `smos_Max`: `a > b ? a : b`.
fn max2(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `smos_Min`: `a < b ? a : b`.
fn min2(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `SmBoolean bFromLeft` as a span side.
const fn side(from_left: bool) -> Side {
    if from_left {
        Side::Left
    } else {
        Side::Right
    }
}

/// The objects of the curve solves (no solution keeps one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NoObject;

impl SolutionObject for NoObject {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
}

// ---- callees that dispatch on the surface kind ------------------------------------------

/// `IsSingularity(uv, eSingDir)` (virtual, default tolerances): the plane override
/// (none); other kinds run `SmSurface::IsSingularity`.
fn is_singularity(k: &Kernel, surface: &Surface, uv: Point2) -> Result<(bool, SurfParamType)> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_singularity()),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            let s = surface.is_singularity(k, uv, EFF_ZERO, false)?;
            Ok((s.singular, s.direction))
        }
    }
}

/// `LocalPointSolve(domain, SM_SO_NORMALIZE, point, guess, found, solution)`: the
/// found flag and the solution's start parameters.
fn surface_local_point_solve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    point: Point3,
    guess: Point2,
) -> Result<(bool, Point2)> {
    let r = surface.local_point_solve::<NoObject>(
        k,
        uv_domain,
        SolverOperation::Normalize,
        point,
        guess,
    )?;
    Ok((r.found, start_uv(&r.solution)))
}

/// The UV point of a surface solution's start parameters.
fn start_uv(s: &Solution<NoObject>) -> Point2 {
    Point2::new(
        s.start.parameters.first().copied().unwrap_or(0.0),
        s.start.parameters.get(1).copied().unwrap_or(0.0),
    )
}

/// `GlobalPointSolve(domain, op, point, tol, pdMaxDropDist, requested, sols)`: the
/// start parameters of the solutions in order. `DoTrace` runs with the surface
/// cache's point testing off (`SmSurfaceDropCurve.cpp:1629`), so the solve must not
/// build (or classify against) a face-owned surface's trimmed cache.
#[allow(clippy::too_many_arguments)] // upstream's argument list
fn surface_global_point_solve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    op: SolverOperation,
    point: Point3,
    distance_tolerance: f64,
    max_drop_dist: Option<f64>,
    requested: SolutionRequested,
) -> Result<Vec<Point2>> {
    let sols = surface.global_point_solve_point_test_off::<NoObject>(
        k,
        Some(NoObject),
        uv_domain,
        op,
        point,
        distance_tolerance,
        max_drop_dist,
        requested,
    )?;
    Ok(sols.as_slice().iter().map(start_uv).collect())
}

/// `IsOnSeam(uv, NULL, &eSeamDir, &newUVs)`: the points across the seam when `uv` is
/// on it (a plane has none; the seam direction output is not read afterwards).
fn is_on_seam(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Option<Vec<Point2>>> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(if p.is_on_seam().0 {
            Some(Vec::new())
        } else {
            None
        }),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            let info = surface.is_on_seam(k, uv, None)?;
            Ok(info.on_seam.then_some(info.cross_seam_uvs))
        }
    }
}

/// `CreateIsoParametricCurve(ctx, param, value, tol, curve)` (virtual, no domain):
/// the B-spline, plane and surface of revolution overrides (none uses the tolerance);
/// other kinds run `SmSurface::CreateIsoParametricCurve`.
fn create_iso_parametric_curve(
    k: &Kernel,
    surface: &Surface,
    param: SurfParamType,
    value: f64,
    approx_tol: f64,
) -> Result<Curve> {
    match surface.kind_with_nurb(k)? {
        SurfaceKind::BSpline(b) => Ok(Curve::from(
            b.create_iso_parametric_curve(k, param, value, IsoCurveOptions::default())?
                .curve,
        )),
        // (SmSurfOfExtrusion inherits the B-spline CreateIsoParametricCurve)
        SurfaceKind::SurfOfExtrusion(s) => Ok(Curve::from(
            s.base()
                .create_iso_parametric_curve(k, param, value, IsoCurveOptions::default())?
                .curve,
        )),
        SurfaceKind::Plane(p) => Ok(Curve::from(
            p.create_iso_parametric_curve(param, value, None, false)?
                .curve,
        )),
        SurfaceKind::Cone(s) => Ok(s
            .create_iso_parametric_curve(k, param, value, None, false)?
            .curve),
        SurfaceKind::Sphere(s) => Ok(s
            .create_iso_parametric_curve(k, param, value, None, false)?
            .curve),
        SurfaceKind::SurfOfRevolution(s) => {
            Ok(s.create_iso_parametric_curve(k, param, value, None)?.curve)
        }
        SurfaceKind::Torus(s) => Ok(s
            .create_iso_parametric_curve(k, param, value, None, false)?
            .curve),
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            // the SmSurface body (SmOffsetSurface does not override it)
            Ok(surface
                .create_iso_parametric_curve(k, param, value, approx_tol, None, false)?
                .curve)
        }
    }
}

/// What `DropIsoCurve` found.
#[derive(Debug, Clone)]
struct IsoDrop {
    /// `rbSuccessfulIsoDrop`.
    success: bool,
    /// `rdMaxDropToSurf`.
    max_dist: f64,
    /// `rpIsoUVCurve`.
    iso_curve: Option<Curve>,
    /// `pdMaxApproxDev`.
    deviation: f64,
}

/// `DropIsoCurve(ctx, domain, curve, interval, start_uv, tol, ..)`.
fn drop_iso_curve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    start_uv: Point2,
    approx_tol: f64,
) -> Result<IsoDrop> {
    let r = surface.drop_iso_curve(k, uv_domain, curve, interval, start_uv, approx_tol)?;
    Ok(IsoDrop {
        success: r.successful,
        max_dist: r.max_drop_to_surf,
        iso_curve: r.iso_uv_curve.map(Curve::from),
        deviation: r.max_approx_dev,
    })
}

/// A shorter-lived copy of the face solve context, for one drop.
fn reborrow<'b, O: SolutionObject>(c: &'b mut DropSolveContext<'_, O>) -> DropSolveContext<'b, O> {
    DropSolveContext {
        solver: &mut *c.solver,
        handle: c.handle,
    }
}

/// `DropCurve(ctx, domain, curve, interval, tol, maxDist, maxDev, uvCurves,
/// bKeepAllDropCurves, NULL, bCreatingUVTrimCurve)` (virtual): the plane override;
/// other kinds run `SmSurface::DropCurve`. Returns the largest drop distance, the
/// deviation and the UV curves. With the caller's face solve `context` the drop runs
/// under it; without one, as for [`surface_global_point_solve`] (`DoTrace` runs with
/// the surface cache's point testing off), the start point drop's `GlobalPointSolve`
/// keeps its fast-drop solutions unclassified (no face test).
#[allow(clippy::too_many_arguments)] // upstream's argument list
fn surface_drop_curve<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    approx_tol: f64,
    keep_all: bool,
    creating_uv_trim_curve: bool,
    context: Option<DropSolveContext<'_, O>>,
) -> Result<(f64, f64, Vec<Curve>)> {
    // `smsurf_GetSurfaceCache` inside the drop hands back the face's cache of a
    // face-owned surface: the caller's solve context builds it
    let r = match context {
        Some(context) => surface.drop_curve_with_context::<O>(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol,
            keep_all,
            None,
            creating_uv_trim_curve,
            context,
        )?,
        // (without one, the start drop keeps its fast-drop solutions unclassified:
        // `DoTrace` leaves the cache's point testing off)
        None => surface.drop_curve_face_tested::<O>(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol,
            keep_all,
            None,
            creating_uv_trim_curve,
            None,
        )?,
    };
    Ok((r.max_drop_to_surf, r.max_approx_dev, r.uv_curves))
}

/// `IsClosed(domain, param)` (default arguments); a failure answers `false`.
fn is_closed(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    param: SurfParamType,
) -> Result<bool> {
    Ok(ignored(surface.is_closed(k, uv_domain, param, None, false))?.is_some_and(|r| r.0))
}

// ---- static helpers -----------------------------------------------------------------------

/// What [`test_point`] found (`sm_TestPoint`'s outputs).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PointTest {
    /// `rbGoodPoint`: the 3D point is within the tolerance of the surface normal line
    /// at the surface point.
    pub good_point: bool,
    /// `rdSurfDropDist`: the distance from the 3D point to the surface along the
    /// normal line (`0` when not good).
    pub surf_drop_dist: f64,
    /// `rdSurfNormLineDist`: the distance from the 3D point to the normal line (`0`
    /// when not good).
    pub surf_norm_line_dist: f64,
}

/// `sm_TestPoint`: whether `point` lies within `approx_tol` of the surface normal
/// line at `uv` (clamped to `uv_domain`), with the drop and normal line distances.
///
/// # Errors
///
/// Those of the surface evaluations.
pub fn test_point(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    uv: Point2,
    point: Point3,
    approx_tol: f64,
) -> Result<PointTest> {
    // get Srf[pos normal]
    let clamped = uv_domain.clamp_point(uv);
    let (surf_pnt, du, dv) = surface.evaluate_1st_derivatives(k, clamped, true, true)?;
    let mut norm = du.cross(dv);
    let norm_length_sq = norm.length_squared();
    if norm_length_sq < EFF_ZERO_SQ {
        norm = surface.evaluate_normal(k, clamped, Side::Left, Side::Left)?;
    } else {
        norm /= norm_length_sq.sqrt();
    }

    // get distance between 3dPoint and the SurfaceNormal line originating at the SurfacePoint
    let line_t = line_closest_point(surf_pnt, norm, point); // sNorm is unit - dLineT = DropDist
    let line_pnt = surf_pnt + line_t * norm;
    let line_off = line_pnt - point;
    let surf_norm_line_dist = line_off.length();

    // when 3dPt is within tol of the SurfaceNormal line
    Ok(if surf_norm_line_dist < approx_tol {
        PointTest {
            good_point: true,
            surf_drop_dist: line_t,
            surf_norm_line_dist,
        }
    } else {
        PointTest::default()
    })
}

/// `sm_AddBreak`: inserts `new_break` into the sorted `breaks` unless a value within
/// `100 * SM_EFF_ZERO * (1 + |new_break|)` is already there.
pub fn add_break(breaks: &mut Vec<f64>, new_break: f64) {
    let scaled_zero = EFF_ZERO * 100.0 * (1.0 + fabs(new_break));

    // find newBreak values position in the rBreaks list
    let mut insertion = breaks.len();
    for (ii, &b) in breaks.iter().enumerate() {
        // skip duplicate values
        if fabs(new_break - b) < scaled_zero {
            return;
        }

        // mark target index
        if new_break < b {
            insertion = ii;
            break;
        }
    }

    // insert new value
    breaks.insert(insertion, new_break);
}

/// `sm_IsGrazingIntersection`: whether the tangents of `curve1` at `param1` and
/// `curve2` at `param2` (evaluated from the right) are parallel within
/// `SM_EFF_ZERO_DEG` (a failed evaluation gives a zero tangent, parallel to nothing).
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluations.
pub fn is_grazing_intersection(
    k: &Kernel,
    curve1: &Curve,
    param1: f64,
    curve2: &Curve,
    param2: f64,
) -> Result<bool> {
    // Method: check tangents.  If parallel, then it's a graze.
    let tangent = |c: &Curve, t: f64| -> Result<Vec3> {
        Ok(ignored(c.evaluate(k, t, 1, false, true))?
            .and_then(|pv| pv.get(1).copied())
            .unwrap_or(Vec3::ZERO))
    };
    let t1 = tangent(curve1, param1)?;
    let t2 = tangent(curve2, param2)?;
    Ok(t1.is_parallel_to(t2, EFF_ZERO_DEG))
}

// ---- the drop curve -----------------------------------------------------------------------

/// The surface drop-curve tracer (`SmSurfaceDropCurve`).
#[derive(Debug, Clone)]
pub struct SurfaceDropCurve<'s> {
    /// The base tracer members.
    pub base: SurfaceTracer<'s>,
    /// `m_crCurveToDrop`: the curve dropped.
    curve: &'s Curve,
    /// `m_vInterval`: the curve interval dropped.
    interval: Extent1d,
    /// `m_vWorkingInterval`: scratch curve sub-interval.
    pub working_interval: Extent1d,
    /// `m_vWorkingUVDomain`: scratch surface sub-domain.
    pub working_uv_domain: Extent2d,
    /// `m_dMaxDistanceToSurface`.
    pub max_distance_to_surface: f64,
    /// `m_bCurveIsOnSurface`: the curve lies on the surface (tighter drop
    /// tolerances), else only near it.
    pub curve_is_on_surface: bool,
    /// `m_pCurveKnotVector`: the curve's unique knots (`None`: not made yet).
    pub curve_knot_vector: Option<Vec<f64>>,
    /// `m_pCurveBreaks`: the sorted curve break parameters (`None`: not made yet).
    pub curve_breaks: Option<Vec<f64>>,
}

impl<'s> SurfaceDropCurve<'s> {
    /// `SmSurfaceDropCurve(cpSurface, crUVDomain, crCurveToDrop, crInterval)`.
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceTracer::new`].
    pub fn new(
        k: &Kernel,
        surface: &'s Surface,
        uv_domain: &Extent2d,
        curve: &'s Curve,
        interval: &Extent1d,
    ) -> Result<Self> {
        Ok(Self {
            base: SurfaceTracer::new(k, surface, uv_domain)?,
            curve,
            interval: *interval,
            // currently leaving the following uninitialized (SmExtent1d()/SmExtent2d())
            working_interval: Extent1d::new(),
            working_uv_domain: Extent2d::new(),
            max_distance_to_surface: 0.0,
            curve_is_on_surface: false,
            curve_knot_vector: None,
            curve_breaks: None,
        })
    }

    /// `GetCurve`.
    #[must_use]
    pub const fn curve(&self) -> &'s Curve {
        self.curve
    }

    /// `GetInterval`.
    #[must_use]
    pub const fn interval(&self) -> Extent1d {
        self.interval
    }

    /// `GetMaxDistToSurface`.
    #[must_use]
    pub const fn max_dist_to_surface(&self) -> f64 {
        self.max_distance_to_surface
    }

    /// `GetCurveIsOnSurface`.
    #[must_use]
    pub const fn curve_is_on_surface(&self) -> bool {
        self.curve_is_on_surface
    }

    /// `SetCurveIsOnSurface`.
    pub fn set_curve_is_on_surface(&mut self, on_surface: bool) {
        self.curve_is_on_surface = on_surface;
    }

    /// `SmSurfaceDropCurve::ComputePointValues`: fills `pnt` (whose curve parameter is
    /// read) with the surface point, derivatives and unit normal at `uv`, the drop
    /// curve position and tangent (the curve tangent projected into the tangent
    /// plane), the UV point and tangent, and the drop and normal line distances. At a
    /// singular surface point the UV tangent is `prev`'s, or an iso direction
    /// pointing into the surface; with `prev`, UV tangent components small in both
    /// points are snapped to zero in both.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when no surface normal is found, when the dropped tangent vanishes
    /// without `prev`, or for a singular point off the natural domain boundary;
    /// those of the evaluations, the vector drop and the point solve.
    pub fn compute_point_values_with_prev(
        &self,
        k: &Kernel,
        uv: Point2,
        pnt: &mut TracePoint,
        prev: Option<&mut TracePoint>,
    ) -> Result<()> {
        let surface = self.base.surface;

        // get Curve(crCurrPnt.CurveParam) position and tangent
        let left = self.working_interval.t_left_eval(pnt.curve_parameter);
        let mut pv = self.curve.evaluate(k, pnt.curve_parameter, 1, left, true)?;

        // when curveToDrop tangent is degenerate
        if at(&pv, 1).length_squared() < EFF_ZERO_SQ {
            // move param a small step into the interval
            let param = if left {
                pnt.curve_parameter + EFF_ZERO_SQRT
            } else {
                pnt.curve_parameter - EFF_ZERO_SQRT
            };

            // get Curve position and tangent for the moved param value
            pv = self.curve.evaluate(k, param, 1, left, true)?;
        }
        let (pv0, pv1) = (at(&pv, 0), at(&pv, 1));

        // Evaluate SurfacePoint
        let flags = EvaluateFlags {
            u_side: side(self.working_uv_domain.u_left_eval(uv.x)),
            v_side: side(self.working_uv_domain.v_left_eval(uv.y)),
            only_upper_half: true,
            non_zero_tangents: true,
        };
        let eval = surface.evaluate(k, uv, 1, 1, flags)?;
        let ev = |i: usize, j: usize| {
            eval.get(i)
                .and_then(|r| r.get(j))
                .copied()
                .unwrap_or(Vec3::ZERO)
        };

        // output SurfacePoint position and tangent
        pnt.surface[0][0] = ev(0, 0);
        pnt.surface[1][0] = ev(1, 0);
        pnt.surface[0][1] = ev(0, 1);

        // get surfaceNormal
        let mut norm = pnt.surface[1][0].cross(pnt.surface[0][1]);
        if norm.length_squared() < EFF_ZERO_SQ {
            norm = surface.evaluate_normal(k, uv, Side::Left, Side::Left)?;

            // signal error when unable to get a surfaceNormal
            if norm.length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP_CPV));
            }
        }

        // output unitized surface normal in [2][2]
        let norm = norm.unitize_or_z();
        pnt.surface[2][2] = norm;

        // output DropCurve Position and Tangent (projected to surfaceTangentPlane)
        pnt.curve_3d[0] = pnt.surface[0][0];
        pnt.curve_3d[1] = pv1.project_to_plane(norm);

        // when DropCurveTangent is degenerate
        if pnt.curve_3d[1].length_squared() < EFF_ZERO_SQ {
            // copy PreviousPoint Tangent Value or signal error
            match prev.as_deref() {
                Some(p) => pnt.curve_3d[1] = p.curve_3d[1],
                None => return Err(SolidError::sm_err(OP_CPV)),
            }
        }

        // output DropUVCurve position
        pnt.curve_uv[0] = uv;

        // output DropUVCurve tangent as projection of curve3DTangent to SurfaceTangentPlane
        //   when surface point is singular - return appropriate isoParam direction
        let (singular, sing_dir) = is_singularity(k, surface, uv)?;
        if !singular {
            // when surfPoint is not singular - project curve3dTangent to get curveUVTangent
            let uvs = drop_vectors(pnt.surface[1][0], pnt.surface[0][1], &[pnt.curve_3d[1]])?;
            pnt.curve_uv[1] = uvs.first().copied().unwrap_or(Vec2::ZERO);
        } else if let Some(p) = prev.as_deref() {
            // when given a PrevPnt - use its tangent value
            pnt.curve_uv[1] = p.curve_uv[1];
        } else {
            // get isoParam vector pointing into surf from singular boundary
            let domain = surface.natural_uv_domain_with_nurb(k)?;
            let (on_boundary, uv_tang) = domain.is_point_on_boundary(uv, EFF_ZERO_SQRT);
            if !on_boundary {
                // error - singular surface point not on boundary
                return Err(SolidError::sm_err(OP_CPV));
            }

            // drop a point a small step in the 3d tangent direction onto the surf
            // to set the curveUVPoint appropriately for a isoparam 1st step.
            let size = pnt.curve_3d[1].length();
            let step_point = pnt.curve_3d[0] + 0.0001 / size * pnt.curve_3d[1];
            let sols = surface_global_point_solve(
                k,
                surface,
                &domain,
                SolverOperation::Minimize,
                step_point,
                0.001,
                None,
                SolutionRequested::Single,
            )?;
            // when drop point was not found - just use current crUVPoint value
            let drop_uv = sols.first().copied().unwrap_or(uv);

            // set UVPoint so that UVTangent will be an isoParameter direction
            if sing_dir == SurfParamType::U {
                pnt.curve_uv[0].x = drop_uv.x;
            } else {
                pnt.curve_uv[0].y = drop_uv.y;
            }

            // set UVTangent for no PrevPnt case
            pnt.curve_uv[1] = uv_tang;
        }

        // when given a previous point snap UVDropCurves to isoParameter directions
        if let Some(p) = prev {
            if fabs(p.curve_uv[1].x) < EFF_ZERO_SQRT && fabs(pnt.curve_uv[1].x) < EFF_ZERO_SQRT {
                pnt.curve_uv[1].x = 0.0;
                p.curve_uv[1].x = 0.0;
            }
            if fabs(p.curve_uv[1].y) < EFF_ZERO_SQRT && fabs(pnt.curve_uv[1].y) < EFF_ZERO_SQRT {
                pnt.curve_uv[1].y = 0.0;
                p.curve_uv[1].y = 0.0;
            }
        }

        // calc Surf DropDist and NormLineDist
        pnt.surf_drop_dist = line_closest_point(pnt.surface[0][0], pnt.surface[2][2], pv0);
        let line_pt = pnt.surface[0][0] + pnt.surf_drop_dist * pnt.surface[2][2];
        pnt.surf_norm_line_dist = (line_pt - pv0).length();
        Ok(())
    }

    /// `SmSurfaceDropCurve::ComputeStepSize`: the next curve parameter step from a
    /// point at `last_t`: `0` at the end of the working interval or of the last knot
    /// span, the next span's length at the end of a span, the step to the span end
    /// when `old_step_size` would pass it, else `old_step_size` (or the rest of the
    /// span when it is zero).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without a knot vector; `InvalidInput` for an empty one.
    pub fn step_size(&self, last_t: f64, old_step_size: f64) -> Result<f64> {
        // when LastT is on the 3dCurve's interval boundary - set StepSize to zero and quit
        if fabs(last_t - self.working_interval.max()) < EFF_ZERO_SQRT {
            return Ok(0.0);
        }

        // locals - knot indices of CurveToDrop knot interval containing CurrPnt.parameter
        let knots = self
            .curve_knot_vector
            .as_ref()
            .ok_or(SolidError::null_pointer(OP_STEP))?;
        let knot = |i: usize| -> Result<f64> {
            knots
                .get(i)
                .copied()
                .ok_or(SolidError::invalid_input(OP_STEP, "knot index"))
        };
        let last = knots
            .len()
            .checked_sub(1)
            .ok_or(SolidError::invalid_input(OP_STEP, "empty knot vector"))?;
        let mut l_min = 0usize;
        let mut l_max = last;

        // Find the CurveToDrop Knot interval containing CurrPnt.parameter.
        while l_max > l_min + 1 {
            let l_mid = (l_min + l_max) / 2;
            if last_t >= knot(l_mid)? {
                l_min = l_mid;
            } else {
                l_max = l_mid;
            }
        }

        // get CurveToDrop params for KnotIvl containing CurrPnt.param - clip to m_vWorkingInterval
        let d_min = knot(l_min)?;
        let d_max = if knot(l_max)? > self.working_interval.max() {
            self.working_interval.max()
        } else {
            knot(l_max)?
        };

        // If we hit the end of the span
        if fabs(d_max - last_t) < EFF_ZERO_SQRT {
            // if there are more CurveToDrop spans to explore - set NewStepSize to the end of the next span
            if l_max < last {
                return Ok(knot(l_max + 1)? - d_max);
            }

            // else there are no more CurveTo Drop spans to explore - zero the step size and quit
            return Ok(0.0);
        }

        // If at the beginning of a span upstream sets the step to the whole span, a
        // side effect every path below overwrites.
        let _ = d_min;

        // If next step goes beyond the current span - truncate it
        if last_t + old_step_size > d_max {
            return Ok(d_max - last_t);
        }

        // If we made it to here then the last point was in the middle of
        // a span and the step size does not take it past the end of the
        // span.
        Ok(if old_step_size > EFF_ZERO {
            old_step_size
        } else {
            d_max - last_t
        })
    }

    /// `SmSurfaceDropCurve::FindInteriorCurves`: when no UV curve was found yet,
    /// drops the whole curve interval onto the domain (`DropCurve`, keeping all
    /// drops) and adds each UV curve with the drop's distance and deviation.
    ///
    /// # Errors
    ///
    /// Those of the drop.
    pub fn interior_curves(&mut self, k: &Kernel, out: &mut TraceOutput) -> Result<()> {
        self.interior_curves_in::<NoObject>(k, out, None)
    }

    /// [`SurfaceDropCurve::interior_curves`] whose drop runs under the caller's face
    /// solve context.
    fn interior_curves_in<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        out: &mut TraceOutput,
        context: Option<DropSolveContext<'_, O>>,
    ) -> Result<()> {
        if !out.uv_curves.is_empty() {
            return Ok(());
        }

        out.max_drop_dists.clear();
        out.deviations.clear();

        let (max_drop_dist, max_deviation, uv_curves) = surface_drop_curve(
            k,
            self.base.surface,
            &self.base.uv_domain,
            self.curve,
            &self.interval,
            self.base.approx_tol_3d,
            true,
            false,
            context,
        )?;

        for c in uv_curves {
            out.uv_curves.push(c);
            out.max_drop_dists.push(max_drop_dist);
            out.deviations.push(max_deviation);
        }
        Ok(())
    }

    /// `SmSurfaceDropCurve::LocalPointSolve`: drops the curve point at `next`'s
    /// parameter onto the working domain from `guess` (a local solve, else the global
    /// solution closest to `guess` in L1), stores the found UV in `next`, and accepts
    /// it unless it jumped across a seam (more than a quarter of the domain in a
    /// closed direction) or lies farther than the approximation tolerance from the
    /// surface normal line ([`test_point`], whose distances are stored in `next`).
    ///
    /// # Errors
    ///
    /// Those of the evaluations and the point solves.
    pub fn drop_local_point_solve(
        &self,
        k: &Kernel,
        guess: Point2,
        next: &mut TracePoint,
    ) -> Result<Option<Point2>> {
        let surface = self.base.surface;
        let domain = self.working_uv_domain;

        // 3d Pnt to drop = CurveToDrop(NextPnt.CurveParam)
        let curr_pnt = self.curve.evaluate_point(k, next.curve_parameter)?;

        // Pass the Drop 3dPnt to Surface call along
        let (found_answer, mut solution) =
            surface_local_point_solve(k, surface, &domain, curr_pnt, guess)?;

        // when Local DropPoint to Surface fails - try global DropPoint to Surface
        if !found_answer {
            let sols = surface_global_point_solve(
                k,
                surface,
                &domain,
                SolverOperation::Minimize,
                curr_pnt,
                EFF_ZERO,
                None,
                SolutionRequested::All,
            )?;

            // Take closest answer in parameter space to our original estimate
            let mut small_diff = 0.0;
            for (jj, s) in sols.iter().enumerate() {
                let diff = fabs(s.x - guess.x) + fabs(s.y - guess.y);

                // save solution closest to guessUV value
                if jj == 0 || diff < small_diff {
                    small_diff = diff;
                    solution = *s;
                }
            }
        }

        // solution locals
        let found_pt = solution;
        let diff = found_pt - guess;
        next.curve_uv[0] = found_pt;

        // if SolPoint skipped across surface seam in U dir - report no good sol found
        if fabs(diff.x) > domain.x_length() / 4.0
            && is_closed(k, surface, &domain, SurfParamType::U)?
        {
            return Ok(None);
        }

        // if SolPoint skipped across surface seam in V dir - report no good sol found
        if fabs(diff.y) > domain.y_length() / 4.0
            && is_closed(k, surface, &domain, SurfParamType::V)?
        {
            return Ok(None);
        }

        // Found Point is good when dist between test pt and SurfNormal line starting at SurfPnt is less than ApproxTol
        let t = test_point(
            k,
            surface,
            &domain,
            found_pt,
            curr_pnt,
            self.base.approx_tol_3d,
        )?;
        next.surf_drop_dist = t.surf_drop_dist;
        next.surf_norm_line_dist = t.surf_norm_line_dist;

        // when NextTracePnt is bad - report no good sol found
        if !t.good_point {
            return Ok(None);
        }
        Ok(Some(found_pt))
    }

    /// `SmSurfaceDropCurve::FindBoundaryStartPoints`: makes the knot vector and the
    /// curve breaks (interval ends, intersections with the domain boundary and the
    /// surface's C0 iso lines (except grazing ones), the curve's C0 knots), then for
    /// each break interval whose middle drops onto the surface drops its start:
    /// intervals that drop onto an iso line (extended over as many breaks as possible)
    /// go to `out`, the others whose start runs into the surface become start points.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the curve cache cannot be built; `SM_ERR` when a
    /// start at a singularity cannot be recovered; those of the curve and surface
    /// queries, intersections, drops and point solves.
    #[allow(clippy::too_many_lines)] // one upstream function
    pub fn boundary_start_points<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        out: &mut TraceOutput,
    ) -> Result<()> {
        let surface = self.base.surface;
        let curve = self.curve;
        let uv_domain = self.base.uv_domain;
        let interval = self.interval;
        let approx_tol = self.base.approx_tol_3d;

        // For dropping points with GlobalPointSolve(): If the curve is on the surface,
        // then we put a limit on how far to allow a good drop, otherwise not.
        let max_drop_dist = self.curve_is_on_surface.then_some(approx_tol);

        // Copy TargetCurve knots into m_pCurveKnotVector
        self.curve_knot_vector = Some(knots_ignoring_status(curve.knots_with_status(k, None))?.0);

        // Place targetCurve interval start/end points into m_pCurveBreaks
        let mut breaks = Vec::new();
        add_break(&mut breaks, interval.min());
        add_break(&mut breaks, interval.max());
        self.curve_breaks = Some(breaks.clone());

        // get Curve and Surface Caches
        let curve_cache = match make_curve_cache::<()>(k, curve, None) {
            Ok(c) => c,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::null_pointer(OP_BOUNDARY)),
        };

        // select tolerance - from curve length or as given when Curve is on Surface
        let size = curve.approximate_length(k, &interval, 5, false)?.0;
        let cci_tol = if self.curve_is_on_surface {
            approx_tol
        } else {
            size / 100.0
        };

        // Add TargetSurface natural boundary curves to sIsoCurves array
        let iso = cache.iso_boundary_curves(k, &uv_domain, cci_tol / 3.0)?;
        let mut iso_curves: Vec<Curve> = iso.curves.to_vec();

        // add every Surface U internal continuity boundary to sIsoCurves array
        let u_conts: Vec<ContinuityType> = cache.u_continuities().to_vec();
        let (u_knots, _) = surface.knots_with_nurb(k, SurfParamType::U, None)?;
        for ii in 1..u_conts.len().saturating_sub(1) {
            if is_c0(&u_conts, ii)? {
                let value = index(&u_knots, ii)?;
                iso_curves.push(create_iso_parametric_curve(
                    k,
                    surface,
                    SurfParamType::U,
                    value,
                    cci_tol / 3.0,
                )?);
            }
        }

        // add every surface V internal continuity boundary to sIsoCurves array
        let (v_knots, _) = surface.knots_with_nurb(k, SurfParamType::V, None)?;
        let v_conts: Vec<ContinuityType> = cache.v_continuities().to_vec();
        for ii in 1..v_conts.len().saturating_sub(1) {
            if is_c0(&v_conts, ii)? {
                let value = index(&v_knots, ii)?;
                iso_curves.push(create_iso_parametric_curve(
                    k,
                    surface,
                    SurfParamType::V,
                    value,
                    cci_tol / 3.0,
                )?);
            }
        }

        // Add every SurfaceIsoBoundary/TargetCurve intersection into m_pCurveBreaks array
        for (ii, iso_curve) in iso_curves.iter().enumerate() {
            // when boundary curve is degenerate - intersect with point
            if ignored(iso_curve.is_degenerate(k, EFF_ZERO, None))? == Some(true) {
                let pole =
                    ignored(iso_curve.evaluate_point(k, iso_curve.natural_interval().min()))?
                        .unwrap_or(Vec3::ZERO);

                // DropPoint with SM_SO_INTERSECT avoids an actual drop from off the curve.
                let dropped = curve.drop_point(
                    k,
                    &interval,
                    pole,
                    None,
                    cci_tol,
                    None,
                    SolverOperation::Intersect,
                )?;
                if dropped.success {
                    add_break(&mut breaks, dropped.parameter);
                }
                continue;
            }

            // Call GlobalCurveIntersect() if the curve lies in the surface,
            // otherwise call GlobalCurveSolve() to do a projected intersection.
            let solutions = if self.curve_is_on_surface {
                // get surfaceBoundary/Curve intersections but no GlobalCoincidenceCheck
                curve.global_curve_intersect::<NoObject>(
                    k,
                    &interval,
                    iso_curve,
                    &iso_curve.natural_interval(),
                    cci_tol,
                    &CurveIntersectOptions::default(),
                )?
            } else {
                // Create SmCrvOnSurface.
                let uv_min = uv_domain.min();
                let uv_max = uv_domain.max();
                let ends = match ii {
                    0 => Some((
                        Point2::new(uv_min.x, uv_min.y),
                        Point2::new(uv_min.x, uv_max.y),
                    )),
                    1 => Some((
                        Point2::new(uv_min.x, uv_min.y),
                        Point2::new(uv_max.x, uv_min.y),
                    )),
                    2 => Some((
                        Point2::new(uv_max.x, uv_max.y),
                        Point2::new(uv_max.x, uv_min.y),
                    )),
                    3 => Some((
                        Point2::new(uv_max.x, uv_max.y),
                        Point2::new(uv_min.x, uv_max.y),
                    )),
                    _ => None,
                };
                let Some((pt0, pt1)) = ends else {
                    continue;
                };
                let uv_line = match Line::create_line_segment(
                    2,
                    Point3::new(pt0.x, pt0.y, 0.0),
                    Point3::new(pt1.x, pt1.y, 0.0),
                    None,
                ) {
                    Ok(l) => l,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue,
                };

                let srf_crv = Curve::from(CrvOnSurf::new(
                    k,
                    Curve::from(uv_line),
                    surface.clone_for_evaluation()?,
                    Some(uv_domain),
                    0,
                )?);

                // (no solution keeps the extinct SmCrvOnSurf as its object)
                curve.global_curve_solve::<NoObject>(
                    k,
                    &interval,
                    &srf_crv,
                    &srf_crv.natural_interval(),
                    SolverOperation::SurfaceProjectedIntersect,
                    cci_tol,
                    &CurveSolveOptions::default(),
                    SolutionRequested::All,
                )?
            };

            // add every intersection point to the m_pCurveBreaks array
            for sol in solutions.as_slice() {
                let mut skip = false;

                // look for coincident regions not connected to the CurveInterval endPoints
                if sol.solution_type == SolutionType::RangeOfValues {
                    // skip loose coincident solutions
                    if sol.start.solution_value > cci_tol / 100.00 {
                        skip = true;
                    } else {
                        // when coincident solution is not connected to interval endPoint
                        let end0 = sol.end.param(0)?;
                        if fabs(end0 - interval.min()) > EFF_ZERO_SQRT / 100.0
                            && fabs(end0 - interval.max()) > EFF_ZERO_SQRT / 100.0
                        {
                            // GWC: always Add Range Solution endPoints
                            add_break(&mut breaks, end0);
                        }
                    }
                }

                // skip intersections which start at interval endPoint
                let start0 = sol.start.param(0)?;
                if fabs(start0 - interval.min()) < EFF_ZERO_SQRT / 100.0 {
                    skip = true;
                }
                if fabs(start0 - interval.max()) < EFF_ZERO_SQRT / 100.0 {
                    skip = true;
                }

                // Skip grazing intersections.  [Reg_091216_S]
                if !skip
                    && is_grazing_intersection(k, curve, start0, iso_curve, sol.start.param(1)?)?
                {
                    skip = true;
                }

                // when not skipping this solution
                if !skip {
                    // GWC: always Add Solution Start Points
                    add_break(&mut breaks, start0);
                }
            }
        }

        // Add every curve discontinuity point to breakpoints array
        let (c_knots, _) = curve.knots(k, None)?;
        let conts = curve_cache.continuities();
        for jj in 1..conts.len().saturating_sub(1) {
            if is_c0(conts, jj)? {
                let knot = index(&c_knots, jj)?;
                if interval.contains_value(knot, 0.0) {
                    add_break(&mut breaks, knot);
                }
            }
        }
        self.curve_breaks = Some(breaks.clone());

        // for every CurveBreak interval (bounded by [ii-1,ii] BreakPoint pair)
        let mut ii = 1usize;
        while ii < breaks.len() {
            // snap this and last breakpoint parameters to nearby curve knot values
            let t0 = curve.snap_to_knots(k, index(&breaks, ii - 1)?, EFF_ZERO_SQRT / 100.0)?;
            let t1 = curve.snap_to_knots(k, index(&breaks, ii)?, EFF_ZERO_SQRT / 100.0)?;

            // Copy the snapped values back into the array:
            set_index(&mut breaks, ii - 1, t0)?;
            set_index(&mut breaks, ii, t1)?;
            self.curve_breaks = Some(breaks.clone());

            // skip intervals that have become zero length - no need for tolerance here
            #[allow(clippy::float_cmp)] // upstream's exact test
            if t0 == t1 {
                ii += 1;
                continue;
            }

            // this interval
            let ivl = Extent1d::from_min_max(t0, t1);

            // Find interval midPoint projection to Surface
            let mid = curve.evaluate_point(k, ivl.evaluate(0.5))?;
            let mid_sols = surface_global_point_solve(
                k,
                surface,
                &uv_domain,
                SolverOperation::Normalize,
                mid,
                approx_tol,
                max_drop_dist,
                SolutionRequested::All,
            )?;

            // skip intervals whose midpoints don't drop to the surface
            if mid_sols.is_empty() {
                ii += 1;
                continue;
            }

            // drop Interval StartPoint to Surface along surface normals
            self.working_interval = ivl;
            let mut start = curve.evaluate_point(k, ivl.min())?;
            let mut solutions = surface_global_point_solve(
                k,
                surface,
                &uv_domain,
                SolverOperation::Intersect,
                start,
                approx_tol,
                max_drop_dist,
                SolutionRequested::All,
            )?;

            let mut at_seam = false;
            let mut surf_u_closed = false;
            let mut surf_v_closed = false;

            // when StartPoint drops to exactly two surface points it may be on a seam
            if solutions.len() == 2 {
                surf_u_closed = is_closed(k, surface, &uv_domain, SurfParamType::U)?;
                surf_v_closed = is_closed(k, surface, &uv_domain, SurfParamType::V)?;
                if surf_u_closed || surf_v_closed {
                    at_seam = true;
                }
            }

            // when StartPoint dropped to more than 1 surface points and not a seam - we are probably on a surface singularity
            if solutions.len() > 1 && !at_seam {
                start = curve.evaluate_point(k, ivl.evaluate(0.001))?;
                solutions = surface_global_point_solve(
                    k,
                    surface,
                    &uv_domain,
                    SolverOperation::Intersect,
                    start,
                    approx_tol,
                    max_drop_dist,
                    SolutionRequested::All,
                )?;

                // snap solutions within 1% of interval boundaries to those boundaries
                let domain_u = uv_domain.u_interval();
                let domain_v = uv_domain.v_interval();
                for sol in &mut solutions {
                    let ut = (sol.x - domain_u.min()) / domain_u.length();
                    let vt = (sol.y - domain_v.min()) / domain_v.length();

                    if ut < 0.01 {
                        sol.x = domain_u.min();
                    } else if ut > 0.99 {
                        sol.x = domain_u.max();
                    }
                    if vt < 0.01 {
                        sol.y = domain_v.min();
                    } else if vt > 0.99 {
                        sol.y = domain_v.max();
                    }
                }
            }

            // process all StartPoint drop points - treat multiple drop points on a seam as two curves
            let mut uv_last = Vec2::ZERO;
            let mut jj = 0usize;
            while jj < solutions.len() {
                let mut uv = index2(&solutions, jj)?;
                if jj == 0 {
                    uv_last = uv;
                } else if uv_last.distance(uv) < EFF_ZERO_SQRT * 100.0 {
                    // skip break points too close to last break point
                    jj += 1;
                    continue;
                }

                // If we start out at a singularity point then we need to step
                // off a little and try again.
                let (singular, singular_dir) = is_singularity(k, surface, uv)?;
                if singular {
                    start = curve.evaluate_point(k, ivl.evaluate(0.001))?;
                    let tmp = surface_global_point_solve(
                        k,
                        surface,
                        &uv_domain,
                        SolverOperation::Minimize,
                        start,
                        EFF_ZERO,
                        max_drop_dist,
                        SolutionRequested::All,
                    )?;
                    let pick = if tmp.len() > jj {
                        tmp.get(jj).copied()
                    } else {
                        tmp.first().copied()
                    };
                    let Some(p) = pick else {
                        // we failed to resolve singularity problem
                        return Err(SolidError::sm_err(OP_BOUNDARY));
                    };
                    if singular_dir == SurfParamType::U {
                        uv.x = p.x;
                    }
                    if singular_dir == SurfParamType::V {
                        uv.y = p.y;
                    }
                }

                // see if working curve interval drops to Surface isoParameterCurve with the same parameterization
                uv = surface.snap_to_knots(k, uv, EFF_ZERO_SQRT / 100.0)?;
                let drop = match drop_iso_curve(k, surface, &uv_domain, curve, &ivl, uv, approx_tol)
                {
                    Ok(d) => d,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => {
                        jj += 1;
                        continue;
                    }
                };

                // when interval drops to Surface isoParameterCurve
                if drop.success {
                    let mut iso_curve = drop.iso_curve;
                    let mut max_dist = drop.max_dist;
                    let mut deviation = drop.deviation;
                    let mut kk = ii + 1;

                    // extend isoLine through as many breakPoints as possible
                    while kk < breaks.len() {
                        // extend interval to next breakPoint
                        let ivl2 = Extent1d::from_min_max(ivl.min(), index(&breaks, kk)?);
                        kk += 1;

                        // see if extended interval drops to Surface isoParameterCurve
                        let drop2 =
                            drop_iso_curve(k, surface, &uv_domain, curve, &ivl2, uv, approx_tol)?;
                        // if it does replace old line with new and continue
                        if drop2.success {
                            iso_curve = drop2.iso_curve;
                            max_dist = drop2.max_dist;
                            deviation = drop2.deviation;

                            // gwc:change need to increase breakPoint counter
                            ii += 1;
                        } else {
                            // use old line and break
                            break;
                        }
                    }

                    let iso_curve = iso_curve.ok_or(SolidError::null_pointer(OP_BOUNDARY))?;

                    // If there are more solutions to check, see if they're contained in this iso curve
                    if solutions.len() > jj + 1 {
                        let iso_ivl = iso_curve.natural_interval();
                        let curve_start = ignored(iso_curve.evaluate_point(k, iso_ivl.min()))?
                            .unwrap_or(Vec3::ZERO);
                        let curve_end = ignored(iso_curve.evaluate_point(k, iso_ivl.max()))?
                            .unwrap_or(Vec3::ZERO);

                        let iso_u_ivl = Extent1d::from_min_max(
                            min2(curve_start.x, curve_end.x),
                            max2(curve_start.x, curve_end.x),
                        );
                        let iso_v_ivl = Extent1d::from_min_max(
                            min2(curve_start.y, curve_end.y),
                            max2(curve_start.y, curve_end.y),
                        );

                        // Remove other solutions that are on this curve
                        let mut kk = jj + 1;
                        while kk < solutions.len() {
                            let next = index2(&solutions, kk)?;

                            // If the point drops to the curve, remove it from solutions to check
                            if iso_u_ivl.contains_value(next.x, 0.0)
                                && iso_v_ivl.contains_value(next.y, 0.0)
                            {
                                solutions.remove(kk);
                            } else {
                                kk += 1;
                            }
                        }
                    }

                    // add IsoLine drop Curve to output
                    out.uv_curves.push(iso_curve);
                    out.max_drop_dists.push(max_dist);
                    out.deviations.push(deviation);

                    self.max_distance_to_surface = max2(self.max_distance_to_surface, max_dist);
                    jj += 1;
                    continue;
                }

                // arrive here when interval StartPoint may be the start of a
                // legitimate dropCurve segment.  Include point in m_vStartPoint
                // array when interval is determined to be running into the
                // surface - skip points when the curve is running out of the surface.

                // drop point nearby interval startPoint to Surface
                let mut near_start = curve.evaluate_point(k, ivl.evaluate(0.001))?;
                let near_start_solutions = surface_global_point_solve(
                    k,
                    surface,
                    &uv_domain,
                    SolverOperation::Normalize,
                    near_start,
                    EFF_ZERO,
                    max_drop_dist,
                    SolutionRequested::All,
                )?;

                // when NearStartPoint dropped to surface
                // build domain surfaceDomain containing StartPoint and NearStartPoint drop points
                if let Some(&first_near) = near_start_solutions.first() {
                    let mut uv_near_start = first_near;
                    if at_seam {
                        let domain_size = uv_domain.size();

                        // Jump UV point across the seam instead of skipping [B651]
                        if near_start_solutions.len() > 1 {
                            let mut check_u_seam = false;
                            let mut check_v_seam = false;

                            // Determine which seams we might want to jump
                            for kk in 1..near_start_solutions.len() {
                                let a = index2(&near_start_solutions, kk)?;
                                let b = index2(&near_start_solutions, kk - 1)?;
                                if fabs(a.x - b.x) > EFF_ZERO {
                                    check_u_seam = true;
                                }
                                if fabs(a.y - b.y) > EFF_ZERO {
                                    check_v_seam = true;
                                }
                            }

                            // Determine which seams this curve actually jumps
                            let mut next_start_solutions = Vec::new();
                            let mut have_1_solution = false;
                            let mut iters = 1.0;
                            while iters < 20.0 {
                                // Big jumps should be fine, just trying to find which side of the seam this interval runs on
                                near_start = curve.evaluate_point(k, ivl.evaluate(0.05 * iters))?;
                                next_start_solutions = surface_global_point_solve(
                                    k,
                                    surface,
                                    &uv_domain,
                                    SolverOperation::Normalize,
                                    near_start,
                                    EFF_ZERO,
                                    max_drop_dist,
                                    SolutionRequested::All,
                                )?;

                                if next_start_solutions.len() == 1 {
                                    have_1_solution = true;
                                    break;
                                }
                                if next_start_solutions.is_empty() {
                                    // (SM_DBG_WARN: couldn't determine which way to cross seam)
                                    break;
                                }

                                iters += 1.0;
                            }

                            if !have_1_solution {
                                jj += 1;
                                continue;
                            }

                            // Jump the UV point across the seams as necessary
                            let uv_next_near_start = index2(&next_start_solutions, 0)?;
                            if check_u_seam
                                && surf_u_closed
                                && fabs(uv.x - uv_next_near_start.x) > 0.5 * domain_size.x
                            {
                                if let Some(new_uvs) = is_on_seam(k, surface, uv)? {
                                    uv = index2(&new_uvs, 0)?;
                                }
                            }

                            if check_v_seam
                                && surf_v_closed
                                && fabs(uv.y - uv_next_near_start.y) > 0.5 * domain_size.y
                            {
                                if let Some(new_uvs) = is_on_seam(k, surface, uv)? {
                                    uv = index2(&new_uvs, 0)?;
                                }
                            }

                            // Update sUVNearStart to the correct solution, as determined by smallest l1 distance
                            // (upstream measures against the 3D sample point sNearStart)
                            let mut dist = fabs(uv.x - near_start.x) + fabs(uv.y - near_start.y);
                            for kk in 1..near_start_solutions.len() {
                                let test = index2(&near_start_solutions, kk)?;
                                let test_dist = fabs(uv.x - test.x) + fabs(uv.y - test.y);
                                if dist > test_dist {
                                    dist = test_dist;
                                    uv_near_start = test;
                                }
                            }
                        } else {
                            if surf_u_closed && fabs(uv.x - uv_near_start.x) > 0.5 * domain_size.x {
                                if let Some(new_uvs) = is_on_seam(k, surface, uv)? {
                                    uv = index2(&new_uvs, 0)?;
                                }
                            }

                            if surf_v_closed && fabs(uv.y - uv_near_start.y) > 0.5 * domain_size.y {
                                if let Some(new_uvs) = is_on_seam(k, surface, uv)? {
                                    uv = index2(&new_uvs, 0)?;
                                }
                            }
                        }
                    }
                    let mut dom = Extent2d::from_point(uv_near_start); // Interval NearStartPoint drop to surface loc
                    dom.add_point(uv); // Interval StartPoint drop to surface loc
                    self.working_uv_domain = dom;
                }

                // skip segments whose NearStartPoint don't drop into the surface or
                // whose StartPoint and NearStartPoint drop to the same point.
                if near_start_solutions.is_empty()
                    || (self.working_uv_domain.x_length() < EFF_ZERO
                        && self.working_uv_domain.y_length() < EFF_ZERO)
                {
                    jj += 1;
                    continue;
                }

                // make intervalStartPoint a TracePoint
                let mut tsp = TracePoint::zeroed();
                tsp.curve_parameter = ivl.min();
                self.compute_point_values_with_prev(k, uv, &mut tsp, None)?;
                tsp.point_type = TracePointType::Normal;

                // add unique pTSP points to m_vStartPoints array
                let id = self.base.new_point(tsp);
                self.base.add_start_point(id)?;
                jj += 1;
            }
            ii += 1;
        }
        self.curve_breaks = Some(breaks);
        Ok(())
    }

    /// `SmSurfaceDropCurve::DoTrace`: stores the optional tolerances, then (with the
    /// cache's point test off and boundary curve processing on) finds the break
    /// intervals ([`SurfaceDropCurve::boundary_start_points`]), drops each from its
    /// start point ([`SurfaceDropCurve::trace_curve_2`]) and finally the whole curve
    /// when nothing dropped yet ([`SurfaceDropCurve::interior_curves`]).
    ///
    /// # Errors
    ///
    /// Those of the steps.
    pub fn drop_trace<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
        context: Option<DropSolveContext<'_, O>>,
    ) -> Result<TraceOutput> {
        // set object values from input
        if let Some(tol) = approx_tol_3d {
            self.base.approx_tol_3d = tol;
        }
        if let Some(ang) = ang_tol_rad {
            self.base.ang_tol_rad = ang;
        }

        // temporarily turn off point testing and turn on boundary curve processing
        let saved_point_test = cache.point_test_enabled;
        let saved_boundary = cache.process_boundary_curves;
        cache.point_test_enabled = false;
        cache.process_boundary_curves = true;
        let r = self.drop_trace_body(k, cache, context);
        cache.point_test_enabled = saved_point_test;
        cache.process_boundary_curves = saved_boundary;
        r
    }

    /// The part of [`SurfaceDropCurve::drop_trace`] run with the cache flags changed.
    fn drop_trace_body<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        mut context: Option<DropSolveContext<'_, O>>,
    ) -> Result<TraceOutput> {
        let mut out = TraceOutput::default();

        // 1. Place startPoints into m_vStartPoints.
        // 2. Place Curve Segments that drop to Surface IsoParameter Curves into sSurfaceUVCurves.
        self.boundary_start_points(k, cache, &mut out)?;

        // Now trace a dropCurve starting at each point listed in m_vStartPoints
        let active = self.base.start_points.clone();
        for tsp in active {
            self.base.curve_trace_direction = 1.0;
            self.trace_curve_2_in(
                k,
                tsp,
                &mut out,
                Some(cache),
                context.as_mut().map(reborrow),
            )?;
        }

        // trace dropCurve segments that don't start on a BreakPoint
        self.interior_curves_in(k, &mut out, context)?;
        Ok(out)
    }

    /// `SmSurfaceDropCurve::TraceCurve2`: appends `start` to the curve points and drops
    /// the curve from its parameter to the next break onto the G1 sub-domain around
    /// it (`DropCurve`); one UV curve (of a seam pair, the one starting nearer
    /// `start`) is added to `out`, otherwise the base tracer
    /// ([`trace_curve_base`]) traces and flushes the curve.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without breaks; `InvalidInput` for an unknown `start`;
    /// those of the domain intersection and the flush.
    pub fn trace_curve_2(
        &mut self,
        k: &Kernel,
        start: TracePointId,
        out: &mut TraceOutput,
    ) -> Result<()> {
        self.trace_curve_2_in::<NoObject>(k, start, out, None, None)
    }

    /// [`SurfaceDropCurve::trace_curve_2`] with the cache `DoTrace` checked out
    /// (`TraceCurve2` reads `GetDomainWithContinuity` through
    /// `smsurf_GetSurfaceCache`, the same cache the drop holds); `None` builds one.
    fn trace_curve_2_in<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        start: TracePointId,
        out: &mut TraceOutput,
        cache: Option<&SurfaceCache<O>>,
        mut context: Option<DropSolveContext<'_, O>>,
    ) -> Result<()> {
        self.base.curve_points.push(start);
        let breaks = self
            .curve_breaks
            .clone()
            .ok_or(SolidError::null_pointer(OP_TRACE2))?;
        let surface = self.base.surface;

        // If there are curve breaks - replace dMaxT with first break after dMinT.
        for &max_t in &breaks {
            let start_pnt = *self.base.point(start)?;
            let min_t = start_pnt.curve_parameter;

            // skip breaks at the start dMinT param value
            if max_t < min_t + EFF_ZERO {
                continue;
            }

            // let WorkingDomain = Surface G1 continuous SubDomain that contains the StartPoint
            self.working_uv_domain = match cache {
                Some(cache) => surface.domain_with_continuity_from_cache(
                    k,
                    ContinuityType::G1,
                    start_pnt.curve_uv[0],
                    start_pnt.curve_uv[1],
                    cache,
                )?,
                None => surface.domain_with_continuity(
                    k,
                    ContinuityType::G1,
                    start_pnt.curve_uv[0],
                    start_pnt.curve_uv[1],
                )?,
            };

            // exit case - working domain does not overlap the domain of interest
            if self
                .base
                .uv_domain
                .are_disjoint(&self.working_uv_domain, 0.0)
            {
                return Ok(());
            }

            // limit the working domain to the domain of interest
            self.working_uv_domain = self.base.uv_domain.intersect(&self.working_uv_domain)?;

            // curve working interval
            self.working_interval = Extent1d::from_min_max(min_t, max_t);

            // Drop Curve WorkingInterval to Surface WorkingUVDomain to AppoxTol
            let dropped = match surface_drop_curve(
                k,
                surface,
                &self.working_uv_domain,
                self.curve,
                &self.working_interval,
                self.base.approx_tol_3d,
                !self.curve_is_on_surface, // Maybe reject off-surface curves.
                true,
                context.as_mut().map(reborrow),
            ) {
                Ok(d) => Some(d),
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => None,
            };

            let mut single = None;
            if let Some((max_dist, deviation, mut uv_curves)) = dropped {
                // Look for seam case: remove one of the two seam curves.
                if uv_curves.len() == 2 {
                    // Seam case let's find the one that starts closest to rStartPoint.
                    let start_of = |c: &Curve| -> Result<Point3> {
                        Ok(ignored(c.evaluate_point(k, c.natural_interval().min()))?
                            .unwrap_or(Vec3::ZERO))
                    };
                    let pnt1 = start_of(index_curve(&uv_curves, 0)?)?;
                    let pnt2 = start_of(index_curve(&uv_curves, 1)?)?;
                    let start_uv = Vec3::from(start_pnt.curve_uv[0]);

                    // when 1st solution is closer to StartPoint - remove last solution
                    if pnt1.distance(start_uv) < pnt2.distance(start_uv) {
                        uv_curves.pop();
                    } else {
                        // last solution is closer to StartPoint - remove first solution
                        uv_curves.remove(0);
                    }
                }
                if uv_curves.len() == 1 {
                    single = uv_curves.pop().map(|c| (c, max_dist, deviation));
                }
            }

            // when there was just one solution or we are now down to one solution - put it in output and exit
            if let Some((c, max_dist, deviation)) = single {
                out.uv_curves.push(c);
                out.max_drop_dists.push(max_dist);
                out.deviations.push(deviation);
            } else {
                // more than one solution - probably walking in/out of a boundary:
                // the SmSurfaceTracer algorithm
                //
                // First suspect for a later disagreement (WR-182, plan/findings.md "The
                // start list after `DoTrace` shares nodes with the curve list"): upstream
                // TraceCurve2 appends `start` to the intrusive curve ring without taking it
                // off m_vStartPoints, and this base `TraceCurve` walks the start list too
                // (end test, AddStartPoint). The port's id `Vec`s do not model that
                // aliasing; the harness reports such a start list as -3 on both sides.
                match trace_curve_base(self, k, start, &out.curves_3d) {
                    Ok(()) => flush_curve(k, &mut self.base, out, Some(self.curve))?,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => {}
                }
            }

            // all done
            return Ok(());
        }
        Ok(())
    }

    /// `SmSurfaceDropCurve::ComputeNextPoint`: steps the curve parameter of `curr` by
    /// `step_size` (clipped where the UV step leaves the domain, with a tangent nearly
    /// along the boundary straightened onto it), drops the curve point from the UV
    /// guess ([`SurfaceDropCurve::drop_local_point_solve`]), evaluates it into `next`
    /// and judges the step: no UV progress, too large a tangent turn, a zero UV
    /// tangent, a step backwards or too large a span deviation
    /// ([`SurfaceDropCurve::span_deviation`]) is not a good point. A singular `curr`
    /// other than the first curve point ends the trace.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an unknown `curr`; those of the point solve, the evaluation,
    /// `AngleBetween` and the span deviation.
    pub fn next_point(
        &mut self,
        k: &Kernel,
        curr: TracePointId,
        step_size: f64,
        next: &mut TracePoint,
    ) -> Result<NextPoint> {
        let mut out = NextPoint::default();
        let mut cp = *self.base.point(curr)?;

        // exit case: if end of a trace sequence is on a singularity - then stop
        if cp.point_type == TracePointType::Singularity
            && self.base.curve_points.first() != Some(&curr)
        {
            out.clipped = true;
            out.boundary_hit = true;
            out.found_good_point = false;
            return Ok(out);
        }

        // Init output
        out.clipped = false;
        out.boundary_hit = false;
        next.point_type = TracePointType::Normal;

        // locals
        let mut hit_end = false;
        let mut tol = EFF_ZERO * (1.0 + self.interval.length());
        let mut old_step_size = step_size;
        let mut step_size = step_size;
        let domain = self.base.uv_domain;

        // remember when dStepSize steps to or beyond 3dCurve m_vWorkingInterval.GetMax()
        if fabs(cp.curve_parameter + step_size - self.working_interval.max()) < tol {
            hit_end = true;
            out.clipped = true;
        }

        // when StepSize does not hit the end of the 3dCurve m_vWorkingInterval - see if StepSize hits Surface m_vUVDomain boundary
        if !hit_end {
            // 1st pass: use first derivative of param space curves to get start point.
            step_size = domain.clip_line(cp.curve_uv[0], cp.curve_uv[1], step_size);

            // Try to straighten the vectors out if we are right on the
            // boundary and get a clip.
            tol = max2(EFF_ZERO_SQRT, step_size / 100.0);

            // When StepSize was clipped by m_vUVDomain and TracePnt is on the boundary
            if step_size < old_step_size && domain.is_point_on_boundary(cp.curve_uv[0], tol).0 {
                let vec = cp.curve_uv[1];

                // when TracePnt tangent is mostly in the v direction
                if fabs(vec.x) * 100.0 < fabs(vec.y) {
                    // Let Tangent = v dir and reClip to m_vUVDomain
                    cp.curve_uv[1].x = 0.0;
                    *self.base.point_mut(curr)? = cp;
                    step_size = domain.clip_line(cp.curve_uv[0], cp.curve_uv[1], old_step_size);
                } else if fabs(vec.y) * 100.0 < fabs(vec.x) {
                    // Let Tangent = u dir and reClip to m_vUVDomain
                    cp.curve_uv[1].y = 0.0;
                    *self.base.point_mut(curr)? = cp;
                    step_size = domain.clip_line(cp.curve_uv[0], cp.curve_uv[1], old_step_size);
                }
            }

            // init output
            out.clipped = false;
            out.boundary_hit = false;

            // exit case - StepSize clipped to zero length - last point is on boundary - no good next point found
            if is_zero(step_size) {
                out.found_good_point = false;
                out.boundary_hit = true;
                return Ok(out);
            }

            // when StepSize is clipped by more than tolerance
            if step_size < old_step_size - EFF_ZERO * old_step_size {
                // and TracePnt is on the boundary of m_vUVDomain
                if domain.is_point_on_boundary(cp.curve_uv[0], EFF_ZERO).0 {
                    // restore StepSize back to input StepSize - next step clamps next UVPnt to UVDomain
                    step_size = old_step_size;
                } else {
                    // remember that StepSize was clipped
                    out.clipped = true;
                }
            }
        }

        // Clamp sUV0 = TracePnt + StepSize * TracePnt.UVTangent to m_vUVDomain
        let mut uv0 = cp.curve_uv[0] + step_size * cp.curve_uv[1];
        uv0 = domain.clamp_point(uv0);

        // if dStepSize shortened a lot - reset input StepSize - computes next TracePnt param value
        if old_step_size > step_size * 1.2 {
            old_step_size = step_size;
        }

        // Set NextTracePnt.CurveParam = TracePnt.CurveParam + OldStepSize
        next.curve_parameter = cp.curve_parameter + old_step_size;

        // Now that we have guess for the next point - do local iteration
        let Some(uv) = self.drop_local_point_solve(k, uv0, next)? else {
            out.found_good_point = false;
            return Ok(out);
        };

        // fill out the rest of the rNextPnt member values
        let r = self.compute_point_values_with_prev(k, uv, next, Some(&mut cp));
        *self.base.point_mut(curr)? = cp;
        r?;

        // bad case: when UV Step to NextPnt goes to zero - report no good sol found
        let diff0 = next.curve_uv[0] - cp.curve_uv[0];
        let diff1 = cp.curve_uv[0] - uv;
        let leng_diff = diff0.length();
        let leng_tan = diff1.length();
        if leng_diff < EFF_ZERO || leng_tan < EFF_ZERO {
            out.found_good_point = false;
            out.boundary_hit = true;
            return Ok(out);
        }

        // bad case: when NextStep turning angle gets too large - report no good sol found
        out.angle_rad = cp.curve_3d[1].angle_between(next.curve_3d[1])?;
        if out.angle_rad > self.base.ang_tol_rad && step_size > EFF_ZERO * 100.0 {
            out.found_good_point = false;
            return Ok(out);
        }

        // bad case: when TracePnt UVCurveTangent has zero length - report no good sol found
        if cp.curve_uv[1].length_squared() < EFF_ZERO_SQ {
            out.found_good_point = false;
            return Ok(out);
        }

        // bad case: NextPnt.UVPnt doubles back on line(TracePnt.UVPnt, TracePnt.UVTan)
        let line_param = line_closest_point(
            Vec3::from(cp.curve_uv[0]),
            Vec3::from(cp.curve_uv[1]),
            Vec3::from(next.curve_uv[0]),
        );
        if line_param < 0.0 {
            out.found_good_point = false;
            return Ok(out);
        }

        // compute the span's deviation from the actual curve
        let (deviation, satisfies) = self.span_deviation(k, &cp, next, old_step_size)?;
        if let Some(d) = deviation {
            out.deviation = d;
        }
        out.found_good_point = satisfies;

        // exit case: If we have a singularity and it satisfies tolerance than we should stop here
        if next.point_type == TracePointType::Singularity {
            out.clipped = true;
        }
        Ok(out)
    }

    /// `SmSurfaceDropCurve::ComputeSpanDeviation`: the largest distance, at five inner
    /// parameters of the UV Hermite span of `curr`..`next` (clamped to the domain),
    /// between the surface point there and the surface point of the curve point
    /// dropped from it. Returns the deviation (`None` when the span fails) and
    /// whether the span satisfies the approximation tolerance.
    ///
    /// # Errors
    ///
    /// Those of the surface evaluations and of the point solve.
    pub fn span_deviation(
        &self,
        k: &Kernel,
        curr: &TracePoint,
        next: &TracePoint,
        step_size: f64,
    ) -> Result<(Option<f64>, bool)> {
        // Create two parameter space Hermite curves and test some points
        // on the curves as projected into 3d by their corresponding surfaces.
        let hermite_uv = HermiteCurve::from_2d(
            curr.curve_uv[0],
            curr.curve_uv[1] * step_size,
            next.curve_uv[0],
            next.curve_uv[1] * step_size,
            3,
        );

        let surface = self.base.surface;
        let mut max_dist = 0.0;
        let approx_tol = self.base.approx_tol_3d;
        let tol_sq = approx_tol * approx_tol;
        for i in 0..NUM_TEST_POINTS {
            let param = (i as f64 + 1.0) / (NUM_TEST_POINTS as f64 + 1.0);
            let herm_pnt = hermite_uv.evaluate_point(param);
            let mut uv = Vec2::new(herm_pnt.x, herm_pnt.y);
            uv = self.base.uv_domain.clamp_point(uv);
            let crv_pnt = surface.evaluate_point(k, uv)?;

            let mut dummy = TracePoint::new();
            dummy.curve_parameter = curr.curve_parameter + param * step_size;
            let Some(uv_found) = self.drop_local_point_solve(k, uv, &mut dummy)? else {
                return Ok((None, false));
            };

            let crv_pnt2 = surface.evaluate_point(k, uv_found)?;
            let crv_pnt3 = crv_pnt2; // Temp cludge
            let mut dist_sq = crv_pnt3.distance_squared(crv_pnt2);
            dist_sq = max2(dist_sq, crv_pnt.distance_squared(crv_pnt2));
            if dist_sq > max_dist {
                max_dist = dist_sq;
                if max_dist > tol_sq {
                    return Ok((None, false));
                }
            }
        }

        Ok((Some(max_dist.sqrt()), true)) // passed test
    }
}

/// `pv[i]` of an evaluation (zero when missing).
fn at(pv: &[Vec3], i: usize) -> Vec3 {
    pv.get(i).copied().unwrap_or(Vec3::ZERO)
}

/// `a[i]`, `InvalidInput` out of range.
fn index(a: &[f64], i: usize) -> Result<f64> {
    a.get(i)
        .copied()
        .ok_or(SolidError::invalid_input(OP_BOUNDARY, "index out of range"))
}

/// `a[i] = v`, `InvalidInput` out of range.
fn set_index(a: &mut [f64], i: usize, v: f64) -> Result<()> {
    let slot = a
        .get_mut(i)
        .ok_or(SolidError::invalid_input(OP_BOUNDARY, "index out of range"))?;
    *slot = v;
    Ok(())
}

/// `a[i]` of UV points, `InvalidInput` out of range.
fn index2(a: &[Point2], i: usize) -> Result<Point2> {
    a.get(i)
        .copied()
        .ok_or(SolidError::invalid_input(OP_BOUNDARY, "index out of range"))
}

/// `a[i]` of curves, `InvalidInput` out of range.
fn index_curve(a: &[Curve], i: usize) -> Result<&Curve> {
    a.get(i)
        .ok_or(SolidError::invalid_input(OP_TRACE2, "index out of range"))
}

/// `conts[i] <= SM_CT_C0`.
fn is_c0(conts: &[ContinuityType], i: usize) -> Result<bool> {
    let c = conts
        .get(i)
        .copied()
        .ok_or(SolidError::invalid_input(OP_BOUNDARY, "index out of range"))?;
    Ok(c as i32 <= ContinuityType::C0 as i32)
}

impl<'s> SurfaceTrace<'s> for SurfaceDropCurve<'s> {
    fn tracer(&self) -> &SurfaceTracer<'s> {
        &self.base
    }

    fn tracer_mut(&mut self) -> &mut SurfaceTracer<'s> {
        &mut self.base
    }

    fn compute_point_values(
        &mut self,
        k: &Kernel,
        uv: Point2,
        pnt: &mut TracePoint,
        prev: Option<&TracePoint>,
        _step_size: Option<f64>,
    ) -> Result<()> {
        // (the snap written into pPrevPnt goes to a copy: the only caller passing one
        // is the base SmSurfaceTracer::ComputeNextPoint (&rCurrPnt), which
        // SmSurfaceDropCurve overrides, so that path is unreachable for a drop curve)
        let mut prev_copy = prev.copied();
        self.compute_point_values_with_prev(k, uv, pnt, prev_copy.as_mut())
    }

    fn local_point_solve(
        &self,
        k: &Kernel,
        _curr: &TracePoint,
        guess: Point2,
        next: &mut TracePoint,
    ) -> Result<Option<Point2>> {
        self.drop_local_point_solve(k, guess, next)
    }

    fn compute_step_size(&mut self, pnt: TracePointId, old_step_size: f64) -> Result<f64> {
        let last_t = self.base.point(pnt)?.curve_parameter;
        self.step_size(last_t, old_step_size)
    }

    /// `SmSurfaceDropCurve::IsPointOnCurve`: always `false`.
    fn is_point_on_curve(&self, _k: &Kernel, _point: Point3, _curves: &[Curve]) -> Result<bool> {
        Ok(false)
    }

    fn compute_next_point(
        &mut self,
        k: &Kernel,
        curr: TracePointId,
        step_size: f64,
        next: &mut TracePoint,
    ) -> Result<NextPoint> {
        self.next_point(k, curr, step_size, next)
    }

    fn compute_span_deviation(
        &mut self,
        k: &Kernel,
        curr: &TracePoint,
        next: &TracePoint,
        step_size: f64,
    ) -> Result<(Option<f64>, bool)> {
        self.span_deviation(k, curr, next, step_size)
    }

    /// `SmSurfaceDropCurve::TestPoint`: never done.
    fn test_point(&mut self, _k: &Kernel, _next: &mut TracePoint) -> Result<bool> {
        Ok(false)
    }

    fn find_boundary_start_points<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        out: &mut TraceOutput,
    ) -> Result<()> {
        self.boundary_start_points(k, cache, out)
    }

    fn find_interior_curves<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        _cache: &mut SurfaceCache<O>,
        out: &mut TraceOutput,
    ) -> Result<()> {
        self.interior_curves(k, out)
    }

    fn do_trace<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<TraceOutput> {
        self.drop_trace(k, cache, approx_tol_3d, ang_tol_rad, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::surface::plane::Plane;

    fn plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0),
        ))
    }

    fn segment(a: Point3, b: Point3) -> Curve {
        Curve::from(Line::create_line_segment(3, a, b, None).unwrap())
    }

    #[test]
    fn kind() {
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(crate::intersect::surface_tracer::TYPE));
        assert!(!is_kind_of(1));
        assert_eq!(TYPE_STRING, "SmSurfaceDropCurve_TYPE");
    }

    #[test]
    fn add_break_sorts_and_dedups() {
        let mut b = Vec::new();
        add_break(&mut b, 1.0);
        add_break(&mut b, 0.0);
        add_break(&mut b, 0.5);
        add_break(&mut b, 0.5 + 1e-12);
        add_break(&mut b, 2.0);
        assert_eq!(b, vec![0.0, 0.5, 1.0, 2.0]);
    }

    #[test]
    fn grazing() {
        let k = &Kernel::default();
        let a = segment(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let b = segment(Vec3::new(0.0, 1.0, 0.0), Vec3::new(2.0, 1.0, 0.0));
        let c = segment(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        assert!(is_grazing_intersection(k, &a, 0.5, &b, 0.5).unwrap());
        assert!(!is_grazing_intersection(k, &a, 0.5, &c, 0.5).unwrap());
    }

    #[test]
    fn test_point_on_plane() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let t = test_point(
            k,
            &s,
            &d,
            Vec2::new(1.0, 2.0),
            Vec3::new(1.0, 2.0, 3.0),
            0.1,
        )
        .unwrap();
        assert!(t.good_point);
        assert!((t.surf_drop_dist - 3.0).abs() < 1e-12);
        let t = test_point(
            k,
            &s,
            &d,
            Vec2::new(1.0, 2.0),
            Vec3::new(2.0, 2.0, 3.0),
            0.1,
        )
        .unwrap();
        assert!(!t.good_point);
        assert_eq!(t.surf_drop_dist, 0.0);
    }

    #[test]
    fn point_values_on_plane() {
        let k = &Kernel::default();
        let s = plane();
        let c = segment(Vec3::new(0.0, 0.0, 1.0), Vec3::new(4.0, 3.0, 1.0));
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let mut t =
            SurfaceDropCurve::new(k, &s, &d, &c, &Extent1d::from_min_max(0.0, 1.0)).unwrap();
        t.working_interval = Extent1d::from_min_max(0.0, 1.0);
        t.working_uv_domain = d;
        let mut p = TracePoint::new();
        p.curve_parameter = 0.5;
        t.compute_point_values_with_prev(&Kernel::default(), Vec2::new(2.0, 1.5), &mut p, None)
            .unwrap();
        assert!((p.surf_drop_dist - 1.0).abs() < 1e-12);
        assert!(p.surf_norm_line_dist < 1e-12);
        assert!((p.curve_uv[1].x - 4.0).abs() < 1e-12);
        assert!((p.curve_uv[1].y - 3.0).abs() < 1e-12);
    }

    #[test]
    fn step_size_spans() {
        let k = &Kernel::default();
        let s = plane();
        let c = Curve::from(
            BSplineCurve::create_line_segment(
                3,
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        );
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let mut t =
            SurfaceDropCurve::new(k, &s, &d, &c, &Extent1d::from_min_max(0.0, 1.0)).unwrap();
        assert!(t.step_size(0.5, 0.1).is_err());
        t.working_interval = Extent1d::from_min_max(0.0, 1.0);
        t.curve_knot_vector = Some(vec![0.0, 0.5, 1.0]);
        assert_eq!(t.step_size(1.0, 0.1).unwrap(), 0.0);
        assert_eq!(t.step_size(0.5, 0.0).unwrap(), 0.5);
        assert_eq!(t.step_size(0.1, 0.2).unwrap(), 0.2);
        assert!((t.step_size(0.4, 0.2).unwrap() - 0.1).abs() < 1e-15);
        t.curve_knot_vector = Some(Vec::new());
        assert!(t.step_size(0.5, 0.1).is_err());
    }

    #[test]
    fn hooks() {
        let k = &Kernel::default();
        let s = plane();
        let c = segment(Vec3::new(0.0, 0.0, 1.0), Vec3::new(4.0, 3.0, 1.0));
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let mut t =
            SurfaceDropCurve::new(k, &s, &d, &c, &Extent1d::from_min_max(0.0, 1.0)).unwrap();
        let k = Kernel::default();
        assert!(!t.is_point_on_curve(&k, Vec3::ZERO, &[]).unwrap());
        assert!(!SurfaceTrace::test_point(&mut t, &k, &mut TracePoint::new()).unwrap());
        // LocalPointSolve_1 is ported (surface.surface_point_solve_p3)
        assert!(t
            .drop_local_point_solve(&k, Vec2::ZERO, &mut TracePoint::new())
            .is_ok());
        // no breaks yet: a null array upstream
        let id = t.base.new_point(TracePoint::zeroed());
        let mut out = TraceOutput::default();
        assert!(t.trace_curve_2(&k, id, &mut out).is_err());
    }

    #[test]
    fn interior_drop_on_plane() {
        let k = &Kernel::default();
        let s = plane();
        let c = segment(Vec3::new(1.0, 1.0, 0.5), Vec3::new(4.0, 3.0, 0.5));
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let mut t =
            SurfaceDropCurve::new(k, &s, &d, &c, &Extent1d::from_min_max(0.0, 1.0)).unwrap();
        let mut out = TraceOutput::default();
        t.interior_curves(&Kernel::default(), &mut out).unwrap();
        assert_eq!(out.uv_curves.len(), 1);
        assert_eq!(out.max_drop_dists.len(), 1);
    }
}
