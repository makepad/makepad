//! `SmSurfaceIntersector`, part 4: tracing one intersection curve from a start point
//! ([`SurfaceIntersector::trace_curve`]), the search for closed interior curves
//! ([`SurfaceIntersector::find_interior_curves`]) and the two single-curve drivers,
//! through given points ([`SurfaceIntersector::do_point_intersection`]) and along a
//! fillet law ([`SurfaceIntersector::do_law_intersection`]).
//!
//! Upstream: `source/SMLib/src/SmSurfaceIntersector.cpp`.
//!
//! | Rust                                                  | Upstream                      |
//! |-------------------------------------------------------|-------------------------------|
//! | [`SurfaceIntersector::trace_curve`]                   | `TraceCurve`                  |
//! | [`SurfaceIntersector::find_interior_curves`], [`SurfaceIntersector::find_interior_curves_from_node_pairs`], [`InteriorNodePair`] | `FindInteriorCurves` |
//! | [`SurfaceIntersector::do_point_intersection`], [`PointIntersectionOptions`] | `DoPointIntersection` |
//! | [`SurfaceIntersector::do_law_intersection`], [`LawIntersectionOptions`] | `DoLawIntersection` |
//! | `SurfaceIntersector::span_on_found_curve`, `SurfaceIntersector::reaches_found_curve_end`, `stop_at_found_curve_end` (crate/private) | the source fork's `bSpanOnFoundCurve`, `bReachesFoundCurveEnd` lambdas of `TraceCurve` and its loop-top stop (opt-in, WR-76) |
//! | [`IntersectionCurve`]                                 | the curve, type and deviation out-params of both drivers |
//! | [`IntersectorVirtuals`], [`do_point_intersection_with`], [`trace_curve_with`], [`do_law_intersection_with`] | the virtual calls of `DoPointIntersection` / `TraceCurve` / `DoLawIntersection` (`this->ComputePointValues(..)`, ..) on a derived intersector |
//!
//! Deviations:
//! - **Points.** Points are arena ids ([`TsectId`]), as in parts 1 to 3: the start
//!   point of `TraceCurve`, the input points of the drivers (`crTsectPoints`) and the
//!   points upstream takes from `m_vTSPntMgr` (zero-filled, [`TsectPnt::zeroed`]) or
//!   builds on the stack (`sTempTSP`, `sTempEndTSP`: default points; the end point
//!   `sEndTSP` of the singular-point retry becomes an arena point). Linking a point
//!   that is already on a list (undefined upstream) is
//!   [`SolidError::InvalidInput`], as in part 1. `DoLawIntersection` passes the end
//!   points' own `UVPos` to `ComputeLawPointValues` as its guesses, by reference: they
//!   are re-read on every try ([`LawGuess::PointUvs`]), so a failed try that wrote them
//!   changes the next try's guesses as upstream.
//! - **Outputs.** Out-params become return values: the drivers return an
//!   [`IntersectionCurve`] whose members are `None` where upstream leaves the
//!   out-param `NULL` or unset (more or fewer than one curve flushed). On an error
//!   no curve is returned (upstream may already have handed the caller a curve).
//!   `FindInteriorCurves`' output arrays are a [`TsectCurves`] (part 2).
//! - **Virtual calls.** `DoPointIntersection`, `TraceCurve` (and part 3's `TestPoint`
//!   / `sm_UpdateTsectPoint`) are generic drivers ([`do_point_intersection_with`],
//!   [`trace_curve_with`]) over an [`IntersectorVirtuals`] implementor, which supplies
//!   the base part and the virtuals they call (`ComputePointValues`,
//!   `ComputeNextPoint`, `FlushCurve`); the [`SurfaceIntersector`] methods run them on
//!   the basic intersector, `blend::fillet_intersector::FilletDispatch` on the fillet
//!   intersector's overrides. `DoLawIntersection` is the driver
//!   [`do_law_intersection_with`], whose virtual calls (`EvaluateLawPoint`,
//!   `ComputePointValuesDelta`, `TestSpanAccuracy`, `FlushCurve`) go through the same
//!   trait. `FindInteriorCurves` calls the basic bodies.
//! - **Bounded loops.** `TraceCurve`'s step loops end on finite step sizes within a
//!   few thousand halvings or growths, and the trace on its 5000th point (`SM_ERR`
//!   upstream); a NaN step (from NaN tolerances) would loop forever upstream. Here
//!   each step loop stops after [`MAX_STEP_TRIES`] tries and the trace after
//!   [`MAX_TRACE_ITERATIONS`] steps (also counting the snapped points upstream does
//!   not count) with [`SolidError::InvalidInput`]. `DoLawIntersection`'s span
//!   subdivision stops at [`MAX_LAW_POINTS`] points the same way.
//! - **Recursion.** `DoPointIntersection` retries once from a singular end point
//!   through itself, guarded by the function-static `bInsideMe` (set for the nested
//!   call by an `SmTemporaryChangeValue`); here the flag is a parameter of the
//!   private body, so the depth is at most 1.
//! - **Unused arguments.** `DoPointIntersection`'s context (only stored in
//!   `m_cpContext`, not modelled) and `DoLawIntersection`'s context, uniform-steps
//!   flag and angle tolerance (unused upstream) are dropped. `DoLawIntersection`
//!   takes the fillet law as `&mut FilletLaw` (upstream's `const` law caches a
//!   B-spline law's extension through a `const` cast), as part 2 does.
//! - **Interior node pairs.** `FindInteriorCurves` walks the tree node pairs of an
//!   `SM_SR_NODES` global surface solve, reading each node's box and Bezier patch UV
//!   domain: [`Surface::global_surface_solve_with`] returns them with the solutions,
//!   and the walk over the pairs is
//!   [`SurfaceIntersector::find_interior_curves_from_node_pairs`].
//!   That walk retains each curve's lazily built cache for the call, as upstream's
//!   cache manager does. Existing output curves are immutable; flushing only
//!   appends curves, whose caches are built when first visited.
//! - **Offset surfaces.** `DoLawIntersection` needs both surfaces to be
//!   `SmOffsetSurface`s ([`SurfaceKind::OffsetSurface`], else `SM_ERR_NULL_POINTER`).
//! - **Unported callees.** `SmSurface::IsSingularity` of a surface that is not a plane
//!   (`surface.surface_p5`) and the callees of `SmSurface::LocalSurfaceIntersect` and of
//!   the global surface solve's leaf solve that are not ported yet (`surface.surface_p3`,
//!   `surface.surface_p8`) return [`SolidError::Unported`], which always propagates
//!   (also where upstream ignores the status).
//! - **Opt-in fix (WR-76, #392).** With
//!   [`UpstreamFixes::ssi_trace_tangent_curves_once`](crate::kernel::UpstreamFixes::ssi_trace_tangent_curves_once)
//!   `TraceCurve` runs its "already traced" test also on a first step that `TestPoint`
//!   ended (`SurfaceIntersector::span_on_found_curve`), and a near-tangent trace that
//!   passes a found curve's free end stops there, its last span shortened to that end
//!   (`SurfaceIntersector::reaches_found_curve_end`); the source fork's
//!   source fork@0ce09f2. Off by default (upstream behaviour).
//! - **Not ported here.** The `SM_DEBUG_CODE` blocks (graphics, dumps), the
//!   `SM_ASSERT`s and the point-count warnings (`MSG`).

use crate::blend::fillet_standard_solver::p1::FilletLaw;
use crate::curve::curve::p5::make_curve_cache;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::curve_cache::CurveCache;
use crate::curve::hermite_curve::HermiteCurve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::surface::surface::p2::TsectCurveType;
use crate::surface::surface::{Surface, SurfaceKind};

use super::p1::SpanAccuracy;
use super::p1::{
    surface_local_surface_intersect, IntersectionPointType, SrfIndex, SurfaceIntersector, TsectId,
    TsectPnt,
};
use super::p2::{compute_law_point_values_with, LawGuess, LawPointOptions, TsectCurves};
use super::p3::{split_span_with, test_point_with, NextPoint};

const OP_TRACE: &str = "intersect::surface_intersector::p4::SurfaceIntersector::trace_curve";
const OP_FIC: &str = "intersect::surface_intersector::p4::SurfaceIntersector::find_interior_curves";
const OP_DPI: &str =
    "intersect::surface_intersector::p4::SurfaceIntersector::do_point_intersection";
const OP_DLI: &str = "intersect::surface_intersector::p4::SurfaceIntersector::do_law_intersection";

/// The tries of one step-size loop of [`SurfaceIntersector::trace_curve`]: a finite
/// positive step leaves `[100 SM_EFF_ZERO, f64::MAX]` within about 2500 halvings or
/// growths by 1.5.
pub const MAX_STEP_TRIES: usize = 4096;

/// The steps of one [`SurfaceIntersector::trace_curve`] call (upstream fails on the
/// 5000th counted point).
pub const MAX_TRACE_ITERATIONS: usize = 100_000;

/// The points of a [`SurfaceIntersector::do_law_intersection`] subdivision.
pub const MAX_LAW_POINTS: usize = 100_000;

/// The single intersection curve of [`SurfaceIntersector::do_point_intersection`] /
/// [`SurfaceIntersector::do_law_intersection`]; each member is `None` where upstream
/// leaves the out-param `NULL` or unset (unless exactly one curve was flushed).
#[derive(Debug, Clone, Default)]
pub struct IntersectionCurve {
    /// `rp3DCurve`: the 3D curve.
    pub curve_3d: Option<Curve>,
    /// `rpSurface1UVCurve`, `rpSurface2UVCurve`: the UV curves on each surface.
    pub uv_curves: [Option<Curve>; 2],
    /// `reCurveType`.
    pub curve_type: Option<TsectCurveType>,
    /// `rdDeviation`: the largest curve/surface distance.
    pub deviation: Option<f64>,
}

impl IntersectionCurve {
    /// The single curve of `out` (`if (size == 1) out = array[0]` for each array).
    fn from_single(out: TsectCurves) -> Self {
        fn single<T>(mut v: Vec<T>) -> Option<T> {
            if v.len() == 1 {
                v.pop()
            } else {
                None
            }
        }
        let TsectCurves {
            curves_3d,
            uv_curves: [uv0, uv1],
            curve_types,
            deviations,
        } = out;
        Self {
            curve_3d: single(curves_3d),
            uv_curves: [single(uv0), single(uv1)],
            curve_type: single(curve_types),
            deviation: single(deviations),
        }
    }
}

/// The options of [`SurfaceIntersector::do_point_intersection`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointIntersectionOptions {
    /// `bExtendBeforeStart`: also trace backwards from the first point.
    pub extend_before_start: bool,
    /// `bExtendAfterEnd`: the last point does not end the curve.
    pub extend_after_end: bool,
    /// `pOptStartDirection`: the direction to trace from the first point.
    pub start_direction: Option<Vec3>,
    /// `pOptEndDirection`: the curve direction at the last point.
    pub end_direction: Option<Vec3>,
    /// `pOptApproxTol3d`: replaces `this_approx_tol_3d`.
    pub approx_tol_3d: Option<f64>,
    /// `pdOptAngTolRad`: replaces `this_ang_tol_rad`.
    pub ang_tol_rad: Option<f64>,
}

/// The options of [`SurfaceIntersector::do_law_intersection`].
#[derive(Debug, Clone, Copy)]
pub struct LawIntersectionOptions<'a> {
    /// `bLawOrient`: the law runs from the interval end to its start.
    pub law_orient: bool,
    /// `crPointCurve`: the edge curve (arc length parameterised).
    pub point_curve: &'a Curve,
    /// `crPointCurveInterval`: the range of the fillet edge.
    pub point_curve_interval: Extent1d,
    /// `dSurfaceOrientations`: the offset directions of the two surfaces (`1` or `-1`).
    pub surface_orientations: [f64; 2],
    /// `bUseNormalPlaneAveraging`: interpolate the section plane normals between the
    /// end points' curve tangents.
    pub use_normal_plane_averaging: bool,
    /// `pOptApproxTol3d`: the tolerance (`1e-3` when `None`).
    pub approx_tol_3d: Option<f64>,
}

/// A tree node pair of the global surface solve in
/// [`SurfaceIntersector::find_interior_curves`]: the boxes of both nodes and the UV
/// domains of their Bezier patches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteriorNodePair {
    /// `pNode1->m_sBBox`, `pNode2->m_sBBox`.
    pub bboxes: [Extent3d; 2],
    /// `pBezPatch1->GetUVDomain()`, `pBezPatch2->GetUVDomain()`.
    pub uv_domains: [Extent2d; 2],
}

/// The two surfaces as objects of the global surface solve.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SolveSurface(u8);

impl SolutionObject for SolveSurface {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Surface
    }
}

/// The `SmSurfaceIntersector` virtuals that `DoPointIntersection` and
/// `DoLawIntersection` reach: their own `ComputePointValues`, `FlushCurve` and
/// `TestSpanAccuracy`, `TraceCurve`'s `ComputeNextPoint`, `TestPoint`'s
/// `ComputePointValues` (through `sm_UpdateTsectPoint`) and `ComputeLawPointValues`'
/// `EvaluateLawPoint` and `ComputePointValuesDelta`. The drivers
/// [`do_point_intersection_with`], [`trace_curve_with`],
/// [`do_law_intersection_with`] and (part 3) `test_point_with` /
/// `update_tsect_point_with` / `split_span_with` run on any implementor, calling
/// these methods where upstream makes a virtual call and the
/// [`SurfaceIntersector`] (the base class part) for everything else.
///
/// [`SurfaceIntersector`] implements it with the basic bodies (its inherent methods
/// call the drivers on itself); `blend::fillet_intersector::FilletDispatch`
/// implements it with the `SmFilletIntersector` overrides. The law methods default to
/// the basic bodies. The other virtual calls are not reached from a derived
/// intersector's overrides of these: `ComputeSingularityPoint` (and `TestSpanAccuracy`
/// from `ComputeNextPoint`) are only called by the basic `ComputeNextPoint` /
/// `ComputePointValues` bodies, so each implementor's overrides call their own.
pub trait IntersectorVirtuals<'s> {
    /// The `SmSurfaceIntersector` base part.
    fn intersector(&self) -> &SurfaceIntersector<'s>;

    /// The `SmSurfaceIntersector` base part, mutable.
    fn intersector_mut(&mut self) -> &mut SurfaceIntersector<'s>;

    /// `ComputePointValues(aUVValues, rTsectPnt, pOptPreviousPnt, pOptStepSize)`.
    ///
    /// # Errors
    ///
    /// The override's; `point` keeps what was computed before an error.
    fn compute_point_values(
        &mut self,
        k: &Kernel,
        uv_values: [Point2; 2],
        point: &mut TsectPnt,
        previous: Option<&TsectPnt>,
        step_size: Option<f64>,
    ) -> Result<()>;

    /// `ComputeNextPoint(rPrevTSP, dStepSize, rNextTSP, ..)`.
    ///
    /// # Errors
    ///
    /// The override's.
    fn compute_next_point(
        &mut self,
        k: &Kernel,
        prev: TsectId,
        step_size: f64,
        next: TsectId,
    ) -> Result<NextPoint>;

    /// `FlushCurve(r3DCurves, rSurface1UVCurves, rSurface2UVCurves, rCurveTypes,
    /// rDeviations)`.
    ///
    /// # Errors
    ///
    /// The override's.
    fn flush_curve(&mut self, k: &Kernel, out: &mut TsectCurves) -> Result<()>;

    /// `EvaluateLawPoint(dParameter, crUV0, crUV1, crLawCurve, bLawOrient,
    /// crPointCurveInterval, dSurfaceOrientations, rPlaneOrigin, rPlaneNormal,
    /// rdCurveParam, sUVs)`: the curve parameter (`None` where the override leaves it
    /// unset, as the basic body does) and the UV points of the surface / surface /
    /// plane intersection. The default is the basic body
    /// ([`SurfaceIntersector::evaluate_law_point`]).
    ///
    /// # Errors
    ///
    /// The override's.
    #[allow(clippy::too_many_arguments)]
    fn evaluate_law_point(
        &mut self,
        k: &Kernel,
        parameter: f64,
        uv0: Point2,
        uv1: Point2,
        law: &mut FilletLaw,
        law_orient: bool,
        point_curve_interval: &Extent1d,
        surface_orientations: [f64; 2],
        plane_origin: Point3,
        plane_normal: Vec3,
    ) -> Result<(Option<f64>, [Point2; 2])> {
        let uvs = self.intersector().evaluate_law_point(
            k,
            parameter,
            uv0,
            uv1,
            law,
            law_orient,
            point_curve_interval,
            surface_orientations,
            plane_origin,
            plane_normal,
        )?;
        Ok((None, uvs))
    }

    /// `ComputePointValuesDelta(aUVValues, aUVValuesDelta, rTsectPnt, rDeltaPnt)`. The
    /// default is the basic body ([`SurfaceIntersector::compute_point_values_delta`],
    /// always `SM_ERR`).
    ///
    /// # Errors
    ///
    /// The override's.
    fn compute_point_values_delta(
        &mut self,
        _k: &Kernel,
        _uv_values: [Point2; 2],
        _uv_values_delta: [Point2; 2],
        _point: &mut TsectPnt,
        _delta: &mut TsectPnt,
    ) -> Result<()> {
        self.intersector().compute_point_values_delta()
    }

    /// `TestSpanAccuracy(rTsectPnt, rNextTSP, pOptMidPnt, ..)`. The default is the
    /// basic body ([`SurfaceIntersector::test_span_accuracy`], which ignores `mid`).
    ///
    /// # Errors
    ///
    /// The override's.
    fn test_span_accuracy(
        &mut self,
        k: &Kernel,
        point: TsectId,
        next: TsectId,
        _mid: Option<TsectId>,
    ) -> Result<SpanAccuracy> {
        self.intersector_mut().test_span_accuracy(k, point, next)
    }
}

/// The basic `SmSurfaceIntersector` bodies.
impl<'s> IntersectorVirtuals<'s> for SurfaceIntersector<'s> {
    fn intersector(&self) -> &SurfaceIntersector<'s> {
        self
    }

    fn intersector_mut(&mut self) -> &mut SurfaceIntersector<'s> {
        self
    }

    fn compute_point_values(
        &mut self,
        k: &Kernel,
        uv_values: [Point2; 2],
        point: &mut TsectPnt,
        previous: Option<&TsectPnt>,
        step_size: Option<f64>,
    ) -> Result<()> {
        SurfaceIntersector::compute_point_values(self, k, uv_values, point, previous, step_size)
    }

    fn compute_next_point(
        &mut self,
        k: &Kernel,
        prev: TsectId,
        step_size: f64,
        next: TsectId,
    ) -> Result<NextPoint> {
        SurfaceIntersector::compute_next_point(self, k, prev, step_size, next)
    }

    fn flush_curve(&mut self, k: &Kernel, out: &mut TsectCurves) -> Result<()> {
        SurfaceIntersector::flush_curve(self, k, out)
    }
}

/// The outputs of `ComputeNextPoint` that `TraceCurve` keeps between calls.
#[derive(Debug, Clone, Copy)]
struct Step {
    found_good_point: bool,
    deviation: f64,
    angle_found: f64,
    clipped: bool,
    hit_boundary: bool,
}

fn too_many(op: &'static str) -> SolidError {
    SolidError::invalid_input(op, "the iteration does not terminate (non-finite values)")
}

impl SurfaceIntersector<'_> {
    // ---- tracing -----------------------------------------------------------------------

    /// `SER(SM_ERR)` unless the UV points of `next` are in both domains (enlarged by
    /// `uv_tol`).
    fn check_in_domains(&self, next: TsectId, uv_tol: [f64; 2]) -> Result<()> {
        let p = self.point(next)?;
        if !self.uv_domains[0].contains_point(p.uv_pos(SrfIndex::First), uv_tol[0]) {
            return Err(SolidError::sm_err(OP_TRACE));
        }
        if !self.uv_domains[1].contains_point(p.uv_pos(SrfIndex::Second), uv_tol[1]) {
            return Err(SolidError::sm_err(OP_TRACE));
        }
        Ok(())
    }

    /// Traces the intersection curve from `start` into the curve points
    /// (`TraceCurve`); `curves_3d` are the curves found so far, which the trace must
    /// not run along.
    ///
    /// With no curve points yet, a tangent or coincident start point becomes a
    /// single-point (closed) curve unless it is on one of `curves_3d`; else the start
    /// point (parameter and deviation zero) starts the curve, and when a tiny first
    /// step (`10 SM_EFF_ZERO_SQRT` along the UV tangents) leaves a domain the trace
    /// direction is reversed (or, for a bounded start point on one of `curves_3d`, the
    /// curve is dropped).
    ///
    /// Then steps from the last curve point
    /// ([`SurfaceIntersector::compute_next_point`]) with the step size shrunk by 1.5
    /// until a good point is found (giving up below `100 SM_EFF_ZERO`) and, for an
    /// unclipped first try with small deviation and angle, grown by 1.5 while the
    /// points stay good; each point is tested against the trace targets
    /// ([`SurfaceIntersector::test_point`]) and appended. The trace ends at a domain
    /// boundary, a target that ends it, a step that does not move, a second point on
    /// a curve of `curves_3d`, or a failing step.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a good next point outside a domain, on the 5000th point, or from
    /// the last regrowth step; `InvalidInput` when the start point is already on a
    /// list, an id is unknown, or a loop does not terminate; those of the curve
    /// reversal, the point tests and the curve predicates; [`SolidError::Unported`]
    /// from unported callees.
    pub fn trace_curve(&mut self, k: &Kernel, start: TsectId, curves_3d: &[Curve]) -> Result<()> {
        trace_curve_with(self, k, start, curves_3d)
    }

    /// True when the span from point `from` to point `to` already lies on a found
    /// curve: `to` and three points of the span (a Hermite with chord-length
    /// tangents) are all on `curves_3d` (the fork's `bSpanOnFoundCurve` of
    /// `TraceCurve`, opt-in fix WR-76, #392; source fork@0ce09f2). Used for
    /// the second point of a trace whose first step `TestPoint` ended (snapped onto
    /// another start point or curve end), which upstream adds without its "already
    /// traced" test.
    ///
    /// # Errors
    ///
    /// As [`SurfaceIntersector::is_point_on_curve`]; an unknown point id is
    /// [`SolidError::InvalidInput`].
    pub(crate) fn span_on_found_curve(
        &self,
        k: &Kernel,
        from: TsectId,
        to: TsectId,
        curves_3d: &[Curve],
    ) -> Result<bool> {
        let (from, to) = (self.point(from)?, self.point(to)?);
        if curves_3d.is_empty() || to.point_type == IntersectionPointType::Singularity {
            return Ok(false);
        }
        if !self.is_point_on_curve(k, to.crv_pos(), curves_3d, None)? {
            return Ok(false);
        }
        let chord = to.crv_pos().distance(from.crv_pos());
        let (mut t0, mut t1) = (from.crv_deriv(), to.crv_deriv());
        let (l0, l1) = (t0.length(), t1.length());
        if l0 > EFF_ZERO {
            t0 *= chord / l0;
        }
        if l1 > EFF_ZERO {
            t1 *= chord / l1;
        }
        let span = HermiteCurve::new(from.crv_pos(), t0, to.crv_pos(), t1, 3);
        for i in 1..=3 {
            let mid = span.evaluate_point(0.25 * f64::from(i));
            if !self.is_point_on_curve(k, mid, curves_3d, None)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The fraction `t` of the span `from -> to` of a near-tangent trace (normals
    /// within 1 degree at both ends) at which it passes within `max(10 tol,
    /// sqrt(tol (1 + size)))` of a free end of a non-degenerate found curve, an end
    /// not at the trace's `start` point and with `t` in `[0, 0.95]` (the fork's
    /// `bReachesFoundCurveEnd` of `TraceCurve`, opt-in fix WR-76, #392;
    /// source fork@0ce09f2).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the curve calls (other failed end evaluations
    /// skip that end); an unknown point id is [`SolidError::InvalidInput`].
    pub(crate) fn reaches_found_curve_end(
        &self,
        k: &Kernel,
        start: TsectId,
        from: TsectId,
        to: TsectId,
        curves_3d: &[Curve],
    ) -> Result<Option<f64>> {
        let (start, from, to) = (self.point(start)?, self.point(from)?, self.point(to)?);
        // tangential contact: near-parallel normals at both span ends
        let one_degree = 1.0 * (PI / 180.0);
        if to.tangent_plane_angle_rad > one_degree || from.tangent_plane_angle_rad > one_degree {
            return Ok(None);
        }
        let tol = self.this_approx_tol_3d;
        let reach = max2(10.0 * tol, sqrt(tol * (1.0 + to.crv_pos().max_dimension())));
        let seg = to.crv_pos() - from.crv_pos();
        let seg_len_sq = seg.length_squared();
        for curve in curves_3d {
            if curve.is_degenerate(k, EFF_ZERO, None)? {
                continue;
            }
            let ivl = curve.natural_interval();
            for param in [ivl.min(), ivl.max()] {
                let Some(end) = ignored(curve.evaluate_point(k, param))? else {
                    continue;
                };
                if end.distance(start.crv_pos()) < reach {
                    continue;
                }
                // only an end the span has gone past: an end ahead of the trace may
                // be a true junction (a corner) that the trace is about to reach
                let t = if seg_len_sq > 0.0 {
                    (end - from.crv_pos()).dot(seg) / seg_len_sq
                } else {
                    1.0
                };
                if !(0.0..=0.95).contains(&t) {
                    continue;
                }
                let near = from.crv_pos() + t * seg;
                if near.distance(end) < reach {
                    return Ok(Some(t));
                }
            }
        }
        Ok(None)
    }

    // ---- interior curves ---------------------------------------------------------------

    /// Finds the closed intersection curves inside both domains
    /// (`FindInteriorCurves`): the tree node pairs of a global surface/surface solve
    /// (`SM_SO_INTERSECT`, `SM_SR_NODES`) are traced as
    /// [`SurfaceIntersector::find_interior_curves_from_node_pairs`] does; the curves
    /// are appended to `out`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a self intersection; those of the solve;
    /// [`SolidError::Unported`]: the global surface solve.
    pub fn find_interior_curves(&mut self, k: &Kernel, out: &mut TsectCurves) -> Result<()> {
        // The basic intersector does not allow self intersection testing.
        if self.doing_self_intersection {
            return Err(SolidError::sm_err(OP_FIC));
        }

        // Find node pairs in the tree (SER: checked; surface::surface::p7, its leaf
        // solve SmSSGlobalSolver::LocalSolve in surface::surface::p11)
        let (_, nodes) = self
            .surface(SrfIndex::First)
            .global_surface_solve_with::<SolveSurface>(
                k,
                &self.uv_domains[0],
                self.surface(SrfIndex::Second),
                &self.uv_domains[1],
                SolverOperation::Intersect,
                self.this_approx_tol_3d,
                None,
                &[],
                SolutionRequested::Nodes,
                [Some(SolveSurface(0)), Some(SolveSurface(1))],
            )?;

        // each solution's two tree nodes: their boxes and Bezier patch UV domains
        let pairs: Vec<InteriorNodePair> = nodes
            .iter()
            .map(|n| InteriorNodePair {
                bboxes: n.bboxes,
                uv_domains: n.uv_domains,
            })
            .collect();
        self.find_interior_curves_from_node_pairs(k, &pairs, out)
    }

    /// The loop of `FindInteriorCurves` over the node pairs of the global solve: a
    /// pair whose boxes meet (the first expanded by `this_approx_tol_3d`) near the
    /// tree of a curve of `out` is skipped; else the local surface/surface
    /// intersection from the patches' mid UV points gives a start point which, unless
    /// it is on a curve of `out` or cannot be evaluated, is traced forwards and (for an
    /// open curve from an unbounded point) backwards, and the curve is flushed into
    /// `out`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a self intersection or disjoint boxes; `SM_ERR_NULL_POINTER` for a
    /// curve without a curve cache tree; those of the evaluations, traces, reversals
    /// and flushes; [`SolidError::Unported`]: `SmSurface::LocalSurfaceIntersect`.
    pub fn find_interior_curves_from_node_pairs(
        &mut self,
        k: &Kernel,
        pairs: &[InteriorNodePair],
        out: &mut TsectCurves,
    ) -> Result<()> {
        if self.doing_self_intersection {
            return Err(SolidError::sm_err(OP_FIC));
        }
        let mut curve_caches: Vec<Option<CurveCache<()>>> = Vec::new();
        for pair in pairs {
            // a fast test: is the intersection of these two nodes near a 3D curve
            let mut int_bbox = pair.bboxes[0];
            int_bbox.expand_absolute(self.this_approx_tol_3d);
            // (disjoint boxes: SM_ERR_INVALID_INPUT)
            let int_bbox = int_bbox.intersect(&pair.bboxes[1])?;

            let mut is_near_curve = false;
            // flush_curve only appends to out.curves_3d. Keep existing caches,
            // adding lazy slots for curves appended by an earlier pair.
            curve_caches.resize_with(out.curves_3d.len(), || None);
            for (curve, slot) in out.curves_3d.iter().zip(&mut curve_caches) {
                // NER: no cache (or no tree) is SM_ERR_NULL_POINTER
                let cache = match slot {
                    Some(cache) => cache,
                    None => slot.insert(match make_curve_cache::<()>(k, curve, None) {
                        Ok(c) => c,
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => return Err(SolidError::null_pointer(OP_FIC)),
                    }),
                };
                let tree = cache
                    .tree()
                    .ok_or_else(|| SolidError::null_pointer(OP_FIC))?;
                if tree.intersects_box(&int_bbox)? {
                    is_near_curve = true;
                    break;
                }
            }
            if is_near_curve {
                continue;
            }

            // an intersection curve start point
            let (found_answer, sol) = surface_local_surface_intersect(
                k,
                self.surface(SrfIndex::First),
                &self.uv_domains[0],
                self.surface(SrfIndex::Second),
                &self.uv_domains[1],
                self.this_approx_tol_3d,
                pair.uv_domains[0].evaluate(0.5, 0.5),
                pair.uv_domains[1].evaluate(0.5, 0.5),
                None,
                None,
                true,
            )?;
            if !found_answer {
                continue;
            }
            let [s0, s1, s2, s3] = sol;
            let uv = Point2::new(s0, s1);
            let surf_pnt = self.surface(SrfIndex::First).evaluate_point(k, uv)?;

            // a point on a curve already found
            if self.is_point_on_curve(k, surf_pnt, &out.curves_3d, None)? {
                continue;
            }

            // load the start point and trace it
            let uvs = [uv, Point2::new(s2, s3)];
            let mut fresh = TsectPnt::zeroed();
            fresh.point_type = IntersectionPointType::Unknown;
            let start = self.alloc_point(fresh);
            self.curve_trace_direction = 1.0;

            // a start point: no previous point
            let mut sp = self.point(start)?.clone();
            let r = self.compute_point_values(k, uvs, &mut sp, None, None);
            *self.point_mut(start)? = sp;
            if ignored(r)?.is_none() {
                continue;
            }

            self.start_point = Some(start);
            let input_trace_direction = self.curve_trace_direction;
            self.trace_curve(k, start, &out.curves_3d)?;

            // an open curve from an unbounded point, not reversed by the trace: trace
            // the other way too
            // (opt-in, WR-80: nothing traced, nothing to reverse)
            let traced = !k.fixes.ssi_no_reverse_of_empty_trace || !self.curve_points.is_empty();
            if traced
                && !self.curve_is_closed
                && !self.point(start)?.is_bounded()
                && input_trace_direction == self.curve_trace_direction
            {
                self.curve_trace_direction *= -1.0;
                self.reverse_curve_direction()?;
                self.trace_curve(k, start, &out.curves_3d)?;
                // (upstream multiplies by 1.0 here)
                self.curve_trace_direction *= 1.0;
                self.reverse_curve_direction()?;
            }

            // add the curve of the curve points (which are cleared)
            self.flush_curve(k, out)?;
            self.start_point = None;
        }
        Ok(())
    }

    // ---- single curve drivers ----------------------------------------------------------

    /// Intersects the surfaces in a single curve through `points`
    /// (`DoPointIntersection`). The first point starts the trace (traced backwards
    /// too with `extend_before_start`); with a start direction, the last point ends it
    /// unless `extend_after_end`; the points between are through points. Each point is
    /// evaluated at its UV points (the first with the start direction as previous
    /// tangent, the last with the end direction); the first must be on both surfaces
    /// within `this_approx_tol_3d`. When the trace stops short of the last point at a
    /// singular point, the curve from there to the last point is joined on.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without points, for a point that cannot be evaluated or a first point
    /// off the intersection; `SM_ERR_NULL_POINTER` when no start point was added;
    /// `InvalidInput` when a through point is already on a list or an id is unknown;
    /// those of the reversals, the flush, the singular-point retry and its joins;
    /// [`SolidError::Unported`] from unported callees.
    pub fn do_point_intersection(
        &mut self,
        k: &Kernel,
        points: &[TsectId],
        opts: &PointIntersectionOptions,
    ) -> Result<IntersectionCurve> {
        do_point_intersection_with(self, k, points, opts)
    }

    /// Traces a fillet law curve between the two points of `points`
    /// (`DoLawIntersection`): the intersection of the two offset surfaces (offset by
    /// the law's radius) with the section planes along the point curve. The end points
    /// are evaluated at the interval ends ([`SurfaceIntersector::compute_law_point_values`]);
    /// spans are split ([`SurfaceIntersector::split_span`]) until they pass
    /// [`SurfaceIntersector::test_span_accuracy`], then spans more than 2.7 times as
    /// long as a neighbour are split; the points are flushed into one curve.
    /// `this_approx_tol_3d` becomes the given tolerance (`1e-3` by default).
    ///
    /// # Errors
    ///
    /// `SM_ERR` unless there are two points; `SM_ERR_NULL_POINTER` unless both
    /// surfaces are offset surfaces; those of the law point evaluations, the splits,
    /// the span tests and the flush; `InvalidInput` for an unknown id or a subdivision
    /// that does not terminate; [`SolidError::Unported`] from unported callees.
    pub fn do_law_intersection(
        &mut self,
        k: &Kernel,
        points: &[TsectId],
        opts: &LawIntersectionOptions<'_>,
        law: &mut FilletLaw,
    ) -> Result<IntersectionCurve> {
        do_law_intersection_with(self, k, points, opts, law)
    }
}

/// [`SurfaceIntersector::do_law_intersection`] on the intersector of `v`, its virtual
/// calls (`EvaluateLawPoint` and `ComputePointValuesDelta` through
/// `ComputeLawPointValues`, `TestSpanAccuracy`, `FlushCurve`) dispatched to `v`.
///
/// # Errors
///
/// As [`SurfaceIntersector::do_law_intersection`], with the errors of `v`'s
/// overrides.
#[allow(clippy::too_many_lines)]
pub fn do_law_intersection_with<'s, V: IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    points: &[TsectId],
    opts: &LawIntersectionOptions<'_>,
    law_curve: &mut FilletLaw,
) -> Result<IntersectionCurve> {
    let [first, last] = points else {
        return Err(SolidError::sm_err(OP_DLI));
    };
    let (first, last) = (*first, *last);

    // the point curve tolerance
    v.intersector_mut().this_approx_tol_3d = opts.approx_tol_3d.unwrap_or(1.0e-3);

    let delta_step = 1.0e-8;

    // the offset surfaces
    if !is_offset_surface(v.intersector().surface(SrfIndex::First)) {
        return Err(SolidError::null_pointer(OP_DLI));
    }
    if !is_offset_surface(v.intersector().surface(SrfIndex::Second)) {
        return Err(SolidError::null_pointer(OP_DLI));
    }

    let normals = [
        v.intersector().point(first)?.crv_deriv(),
        v.intersector().point(last)?.crv_deriv(),
    ];
    let ivl = opts.point_curve_interval;
    {
        let p = v.intersector_mut().point_mut(first)?;
        p.user_doubles[1] = ivl.min();
        p.user_doubles[0] = p.user_doubles[1];
    }
    {
        let p = v.intersector_mut().point_mut(last)?;
        p.user_doubles[1] = ivl.max();
        p.user_doubles[0] = p.user_doubles[1];
    }

    let law = LawPointOptions {
        law_orient: opts.law_orient,
        point_curve: opts.point_curve,
        point_curve_interval: ivl,
        normals,
        average_normals: opts.use_normal_plane_averaging,
        surface_orientations: opts.surface_orientations,
    };

    for (id, delta) in [(first, delta_step), (last, -delta_step)] {
        let mut p = v.intersector().point(id)?.clone();
        let r = compute_law_point_values_with(
            v,
            k,
            p.user_doubles[1],
            // (references to the point's own UV points upstream)
            LawGuess::PointUvs,
            delta,
            &law,
            law_curve,
            false,
            &mut p,
        );
        *v.intersector_mut().point_mut(id)? = p;
        r?;
    }

    v.intersector_mut().point_mut(first)?.curve_parameter = 0.0;
    let first_pos = v.intersector().point(first)?.crv_pos();
    let d = first_pos.distance(v.intersector().point(last)?.crv_pos());
    v.intersector_mut().point_mut(last)?.curve_parameter = d;

    let mut stack = vec![last, first];
    let mut curve_pnts = vec![first];

    while stack.len() > 1 {
        if stack.len() + curve_pnts.len() > MAX_LAW_POINTS {
            return Err(too_many(OP_DLI));
        }
        let curr = stack
            .pop()
            .ok_or_else(|| SolidError::invalid_input(OP_DLI, "empty stack"))?;
        let next = *stack
            .last()
            .ok_or_else(|| SolidError::invalid_input(OP_DLI, "empty stack"))?;

        let (curr_param, curr_pos) = {
            let p = v.intersector().point(curr)?;
            (p.curve_parameter, p.crv_pos())
        };
        let next_pos = v.intersector().point(next)?.crv_pos();
        v.intersector_mut().point_mut(next)?.curve_parameter =
            curr_param + curr_pos.distance(next_pos);

        // the new mid point
        let new_tsp = split_span_with(v, k, curr, next, delta_step, &law, law_curve)?;

        let acc = v.test_span_accuracy(k, curr, next, Some(new_tsp))?;
        if acc.found_good_point {
            // accept the point
            curve_pnts.push(next);
        } else {
            // subdivide more
            stack.push(new_tsp);
            stack.push(curr);
        }
    }

    // split spans much longer than their neighbours
    let split_factor = 2.7;
    let mut done = false;
    while !done {
        done = true;
        let mut ip = 1;
        while ip < curve_pnts.len() {
            if curve_pnts.len() > MAX_LAW_POINTS {
                return Err(too_many(OP_DLI));
            }
            let at = |i: usize| {
                curve_pnts
                    .get(i)
                    .copied()
                    .ok_or_else(|| SolidError::invalid_input(OP_DLI, "point index"))
            };
            let curr = at(ip - 1)?;
            let next = at(ip)?;
            let curr_param = v.intersector().point(curr)?.curve_parameter;
            let next_param = v.intersector().point(next)?.curve_parameter;
            let curr_param_range = next_param - curr_param;
            let mut split = false;
            if ip > 1 {
                let prev = at(ip - 2)?;
                let prev_param_range = curr_param - v.intersector().point(prev)?.curve_parameter;
                if curr_param_range > prev_param_range * split_factor {
                    split = true;
                }
            }
            if ip + 1 < curve_pnts.len() {
                let next_next = at(ip + 1)?;
                let next_param_range =
                    v.intersector().point(next_next)?.curve_parameter - next_param;
                if curr_param_range > next_param_range * split_factor {
                    split = true;
                }
            }
            if split {
                let new_tsect = split_span_with(v, k, curr, next, delta_step, &law, law_curve)?;
                curve_pnts.insert(ip, new_tsect);
                done = false;
            }
            ip += 1;
        }
    }

    for &id in &curve_pnts {
        // (status ignored, but linking a linked point is undefined upstream)
        if let Err(
            e @ (SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::Unported { .. }),
        ) = v.intersector_mut().add_point_to_curve(id)
        {
            return Err(e);
        }
    }

    v.intersector_mut().through_points.clear();
    v.intersector_mut().start_points.clear();

    let mut out = TsectCurves::default();
    v.flush_curve(k, &mut out)?;
    Ok(IntersectionCurve::from_single(out))
}

/// One `ComputeNextPoint` call of `TraceCurve`: updates `step` (the deviation and
/// angle only when set); `Ok(false)` when the call failed (`Unported` propagates).
fn trace_step<'s, V: IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    prev: TsectId,
    step_size: f64,
    next: TsectId,
    step: &mut Step,
) -> Result<bool> {
    match v.compute_next_point(k, prev, step_size, next) {
        Ok(n) => {
            step.found_good_point = n.found_good_point;
            if let Some(d) = n.deviation_found {
                step.deviation = d;
            }
            if let Some(a) = n.angle_found_rad {
                step.angle_found = a;
            }
            step.clipped = n.clipped;
            step.hit_boundary = n.boundary_hit;
            Ok(true)
        }
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(false),
    }
}

/// `TestPoint(next, bDone)`: `done` keeps its value when upstream leaves it.
fn trace_test<'s, V: IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    next: TsectId,
    done: &mut bool,
) -> Result<()> {
    if let Some(d) = test_point_with(v, k, next)? {
        *done = d;
    }
    Ok(())
}

/// [`SurfaceIntersector::trace_curve`] on the intersector of `v`, with
/// `ComputeNextPoint` and `TestPoint`'s `ComputePointValues` dispatched to `v`.
///
/// # Errors
///
/// As [`SurfaceIntersector::trace_curve`].
#[allow(clippy::too_many_lines)]
pub fn trace_curve_with<'s, V: IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    start: TsectId,
    curves_3d: &[Curve],
) -> Result<()> {
    // scale tolerances by UV domain size
    let uv_tol = [
        EFF_ZERO * (1.0 + v.intersector().uv_domains[0].max_dimension()),
        EFF_ZERO * (1.0 + v.intersector().uv_domains[1].max_dimension()),
    ];

    // when there are no points on the curve: pick a walking direction from the
    // start point
    if v.intersector().curve_points.is_empty() {
        let sp = v.intersector().point(start)?.clone();

        // 1. a tangent or coincidence start point: a single point curve
        if matches!(
            sp.point_type,
            IntersectionPointType::TangentPoint | IntersectionPointType::Coincidence
        ) {
            if !v
                .intersector()
                .is_point_on_curve(k, sp.crv_pos(), curves_3d, None)?
            {
                let p = v.intersector_mut().point_mut(start)?;
                p.curve_parameter = 0.0; // curves start at 0.0
                p.deviation = 0.0;
                v.intersector_mut().add_point_to_curve(start)?;
                v.intersector_mut().curve_is_closed = true; // Closed curve finishes the job
            }
            return Ok(());
        }

        // 2. place the start point on the curve
        {
            let p = v.intersector_mut().point_mut(start)?;
            p.curve_parameter = 0.0;
            p.deviation = 0.0;
        }
        v.intersector_mut().add_point_to_curve(start)?;

        // 3. when a tiny first step is out of bounds: reverse the direction
        let delta_0 =
            sp.uv_pos(SrfIndex::First) + (10.0 * EFF_ZERO_SQRT) * sp.uv_deriv(SrfIndex::First);
        let delta_1 =
            sp.uv_pos(SrfIndex::Second) + (10.0 * EFF_ZERO_SQRT) * sp.uv_deriv(SrfIndex::Second);
        if !v.intersector().uv_domains[0].contains_point(delta_0, uv_tol[0])
            || !v.intersector().uv_domains[1].contains_point(delta_1, uv_tol[1])
        {
            if sp.is_bounded()
                && v.intersector()
                    .is_point_on_curve(k, sp.crv_pos(), curves_3d, None)?
            {
                // no curve to add here
                v.intersector_mut().curve_points.clear();
                return Ok(());
            }
            v.intersector_mut().curve_trace_direction *= -1.0; // Reverse curve tracing direction
            v.intersector_mut().reverse_curve_direction()?;
        }
    }

    // Curve is not closed prior to tracing
    v.intersector_mut().curve_is_closed = false;

    let mut point_count: u64 = 0;
    let mut step_size = 0.0;
    let mut done = false;
    // (dAngleFound is uninitialised upstream; it is only read after a good point,
    // whose span test set it)
    let mut st = Step {
        found_good_point: false,
        deviation: 0.0,
        angle_found: 0.0,
        clipped: false,
        hit_boundary: false,
    };

    // 4. iterate the trace, adding points to the curve
    let mut iterations = 0usize;
    while !done {
        iterations += 1;
        if iterations > MAX_TRACE_ITERATIONS {
            return Err(too_many(OP_TRACE));
        }

        // 4a. the curve's last point
        let tsp = *v
            .intersector()
            .curve_points
            .last()
            .ok_or_else(|| SolidError::invalid_input(OP_TRACE, "no curve point"))?;

        // opt-in fix (WR-76, #392): a tangent curve traced from its other end that
        // has run onto the free end of an already traced curve is the same tangent
        // curve, whose earlier trace stopped part way; stop here so the two traces
        // meet instead of the second retracing the first
        if k.fixes.ssi_trace_tangent_curves_once
            && stop_at_found_curve_end(v, k, start, tsp, curves_3d, uv_tol)?
        {
            break;
        }

        // 4b. last step size if given, else 100 * this_approx_tol_3d
        step_size = v.intersector().compute_step_size(step_size);

        // 4c. a new point
        let mut fresh = TsectPnt::zeroed();
        fresh.point_type = IntersectionPointType::Unknown;
        let next = v.intersector_mut().alloc_point(fresh);

        // 4d. the next point for the current step size
        if !trace_step(v, k, tsp, step_size, next, &mut st)? {
            // an error return indicates that it can never succeed
            break;
        }

        // when the next guess steps out of bounds (with a modest step) - stop
        if st.hit_boundary && step_size < v.intersector().this_approx_tol_3d * 100.0 {
            break;
        }

        // a good next point must be in both domains
        if st.found_good_point {
            v.intersector().check_in_domains(next, uv_tol)?;
        }

        let mut decreased_step_size = false;

        // try smaller and smaller steps until the point is good
        let mut tries = 0usize;
        while !st.found_good_point {
            tries += 1;
            if tries > MAX_STEP_TRIES {
                return Err(too_many(OP_TRACE));
            }
            step_size /= 1.5;
            if step_size < EFF_ZERO * 100.0 {
                break;
            }
            decreased_step_size = true;
            if !trace_step(v, k, tsp, step_size, next, &mut st)? {
                st.found_good_point = false;
                break;
            }
            if st.found_good_point {
                v.intersector().check_in_domains(next, uv_tol)?;
            }
        }

        // no good point even with smaller steps - quit
        if !st.found_good_point {
            break;
        }

        // snap the good point to a through, start or end point it passed
        let mut last_curve_param = v.intersector().point(next)?.curve_parameter;
        trace_test(v, k, next, &mut done)?;
        if done || v.intersector().point(next)?.curve_parameter != last_curve_param {
            // opt-in fix (WR-76, #392): don't retrace an already found curve
            if k.fixes.ssi_trace_tangent_curves_once
                && point_count == 0
                && v.intersector()
                    .span_on_found_curve(k, tsp, next, curves_3d)?
            {
                break;
            }
            v.intersector_mut().add_point_to_curve(next)?;
            if done {
                break;
            }
            continue;
        }

        // try bigger steps when this one was easy
        if !st.clipped
            && !decreased_step_size
            && st.deviation < v.intersector().this_approx_tol_3d / 8.0
            && st.angle_found < v.intersector().this_ang_tol_rad / 4.0
        {
            last_curve_param = v.intersector().point(next)?.curve_parameter;

            let mut tries = 0usize;
            while st.found_good_point && !st.clipped {
                tries += 1;
                if tries > MAX_STEP_TRIES {
                    return Err(too_many(OP_TRACE));
                }
                step_size *= 1.5;
                if !trace_step(v, k, tsp, step_size, next, &mut st)? {
                    st.found_good_point = false;
                    break;
                }

                // quit increasing after a point that is no good
                if !st.found_good_point {
                    continue;
                }

                v.intersector().check_in_domains(next, uv_tol)?;

                // a refined step of zero: probably on a boundary
                let param = v.intersector().point(next)?.curve_parameter;
                if param <= last_curve_param + EFF_ZERO {
                    st.found_good_point = false;
                    break;
                }
                last_curve_param = param;

                // passed a through or end point, or a singularity
                trace_test(v, k, next, &mut done)?;
                if done || v.intersector().point(next)?.curve_parameter != last_curve_param {
                    if done {
                        // opt-in fix (WR-76, #392): don't retrace an already found
                        // curve
                        if k.fixes.ssi_trace_tangent_curves_once
                            && point_count == 0
                            && v.intersector()
                                .span_on_found_curve(k, tsp, next, curves_3d)?
                        {
                            return Ok(());
                        }
                        // an end condition: done tracing
                        v.intersector_mut().add_point_to_curve(next)?;
                        return Ok(());
                    }
                    break;
                }
            }

            // the last increase gave a bad point: step back to the last good one
            if !st.found_good_point {
                step_size /= 1.5;
                let n = v.compute_next_point(k, tsp, step_size, next)?;
                st.found_good_point = n.found_good_point;
                if let Some(d) = n.deviation_found {
                    st.deviation = d;
                }
                if let Some(a) = n.angle_found_rad {
                    st.angle_found = a;
                }
                st.clipped = n.clipped;
                st.hit_boundary = n.boundary_hit;
                if st.found_good_point {
                    v.intersector().check_in_domains(next, uv_tol)?;
                }
            }
        }

        v.intersector_mut().clipped_by_through_point = false;

        // 4g. a step size this small: stop near the problem
        if step_size < EFF_ZERO * 100.0 {
            // (bDone = TRUE: done tracing)
            break;
        }

        let (tsp_pos, tsp_deriv) = {
            let p = v.intersector().point(tsp)?;
            (p.crv_pos(), p.crv_deriv())
        };
        let (next_pos, next_deriv, next_type) = {
            let p = v.intersector().point(next)?;
            (p.crv_pos(), p.crv_deriv(), p.point_type)
        };
        let dist_moved = next_pos.distance(tsp_pos);
        let tol = EFF_ZERO * 100.0 * (1.0 + tsp_pos.max_dimension());
        if dist_moved < tol {
            // (bDone = TRUE: done tracing)
            break;
        }

        // the second point of the trace on a curve already found
        if point_count == 0
            && (next_type != IntersectionPointType::Singularity
                && v.intersector()
                    .is_point_on_curve(k, next_pos, curves_3d, None)?)
        {
            // unclipped points have to be on curves already traced
            if !st.clipped {
                // (bDone = TRUE: done tracing)
                break;
            }

            // test the span's mid point
            let herm = HermiteCurve::new(
                tsp_pos,
                tsp_deriv * step_size,
                next_pos,
                next_deriv * step_size,
                3,
            );
            let mid = herm.evaluate_point(0.5);
            if v.intersector().is_point_on_curve(k, mid, curves_3d, None)? {
                // (bDone = TRUE: done tracing)
                break;
            }
        }

        // snap to a through or end point passed, then add the point
        if !done {
            trace_test(v, k, next, &mut done)?;
            v.intersector_mut().point_mut(next)?.deviation = st.deviation;
            v.intersector_mut().add_point_to_curve(next)?;
        }

        // 4i. count the points
        point_count += 1;
        if point_count == 5000 {
            // the tolerance is too small or a surface is poorly defined
            return Err(SolidError::sm_err(OP_TRACE));
        }
    }
    Ok(())
}

/// The loop-top test of the opt-in fix WR-76 (#392) in `TraceCurve` (the source
/// fork's source fork@0ce09f2): when the curve's last span `prev -> last` of a
/// near-tangent trace has gone past a free end of a found curve
/// ([`SurfaceIntersector::reaches_found_curve_end`]), the last span is shortened to end
/// there (the last point dropped when the previous point is already there, or replaced
/// by a point a shorter step from the previous one) and the trace stops (`true`).
fn stop_at_found_curve_end<'s, V: IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    start: TsectId,
    last: TsectId,
    curves_3d: &[Curve],
    uv_tol: [f64; 2],
) -> Result<bool> {
    let points = &v.intersector().curve_points;
    let Some(&prev) = points.len().checked_sub(2).and_then(|i| points.get(i)) else {
        return Ok(false);
    };
    if prev == last {
        return Ok(false);
    }
    let first = points.first().copied();
    let Some(end_t) = v
        .intersector()
        .reaches_found_curve_end(k, start, prev, last, curves_3d)?
    else {
        return Ok(false);
    };

    // shorten the last span so it ends at that curve end rather than overlap it
    let (prev_uv0, prev_uv_deriv_len) = {
        let p = v.intersector().point(prev)?;
        (
            p.uv_pos(SrfIndex::First),
            p.uv_deriv(SrfIndex::First).length(),
        )
    };
    if end_t <= 0.05 && first != Some(prev) {
        // the previous point is already at that curve end
        v.intersector_mut().curve_points.pop();
    } else if end_t < 0.95 && prev_uv_deriv_len > EFF_ZERO {
        let last_uv0 = v.intersector().point(last)?.uv_pos(SrfIndex::First);
        let short_step = end_t * last_uv0.distance(prev_uv0) / prev_uv_deriv_len;
        let mut fresh = TsectPnt::zeroed();
        fresh.point_type = IntersectionPointType::Unknown;
        let end = v.intersector_mut().alloc_point(fresh);
        let n = match v.compute_next_point(k, prev, short_step, end) {
            Ok(n) => Some(n),
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => None,
        };
        if let Some(n) = n {
            let (end_uv0, end_uv1) = {
                let p = v.intersector().point(end)?;
                (p.uv_pos(SrfIndex::First), p.uv_pos(SrfIndex::Second))
            };
            if n.found_good_point
                && v.intersector().uv_domains[0].contains_point(end_uv0, uv_tol[0])
                && v.intersector().uv_domains[1].contains_point(end_uv1, uv_tol[1])
            {
                v.intersector_mut().curve_points.pop();
                v.intersector_mut().point_mut(end)?.deviation = n.deviation_found.unwrap_or(0.0);
                v.intersector_mut().add_point_to_curve(end)?;
            }
        }
    }
    Ok(true)
}

/// [`SurfaceIntersector::do_point_intersection`] on the intersector of `v`, its
/// virtual calls (`ComputePointValues`, `FlushCurve`, and through `TraceCurve`
/// `ComputeNextPoint`) dispatched to `v`: the base driver as a derived intersector
/// runs it (`SmFilletIntersector` through `sFI.DoPointIntersection`).
///
/// # Errors
///
/// As [`SurfaceIntersector::do_point_intersection`].
pub fn do_point_intersection_with<'s, V: IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    points: &[TsectId],
    opts: &PointIntersectionOptions,
) -> Result<IntersectionCurve> {
    do_point_intersection_body(v, k, points, opts, false)
}

#[allow(clippy::too_many_lines)]
fn do_point_intersection_body<'s, V: IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    points: &[TsectId],
    opts: &PointIntersectionOptions,
    inside_me: bool,
) -> Result<IntersectionCurve> {
    // init output
    v.intersector_mut().curve_trace_direction = 1.0;

    // no work - no through points specified
    if points.is_empty() {
        return Err(SolidError::sm_err(OP_DPI));
    }

    // Use supplied tolerances when given
    if let Some(t) = opts.approx_tol_3d {
        v.intersector_mut().this_approx_tol_3d = t;
    }
    if let Some(a) = opts.ang_tol_rad {
        v.intersector_mut().this_ang_tol_rad = a;
    }

    let mut out = TsectCurves::default();

    // init the through and start point lists
    v.intersector_mut().through_points.clear();
    v.intersector_mut().start_points.clear();

    // an optional start direction as the 3D tangent of a previous point
    let temp_tsp = opts.start_direction.map(|d| {
        let mut t = TsectPnt::new();
        t.point_type = IntersectionPointType::Unknown;
        t.curve_pv[1] = d;
        t
    });
    // an optional end direction likewise
    let temp_end_tsp = opts.end_direction.map(|d| {
        let mut t = TsectPnt::new();
        t.curve_pv[1] = d;
        t
    });
    let mut prev_tsp: Option<&TsectPnt> = temp_tsp.as_ref();

    let n = points.len();
    for (ii, &pid) in points.iter().enumerate() {
        // initialize its values to unknown
        let mut tsp = {
            let p = v.intersector_mut().point_mut(pid)?;
            p.point_type = IntersectionPointType::Unknown;
            p.curve_parameter = -1.0;
            p.clone()
        };
        let uvs = [tsp.uv_pos(SrfIndex::First), tsp.uv_pos(SrfIndex::Second)];

        // the start direction for the first point, the end direction for the last
        if ii > 0 {
            prev_tsp = None;
        }
        if ii == n - 1 {
            prev_tsp = temp_end_tsp.as_ref();
        }

        let r = v.compute_point_values(k, uvs, &mut tsp, prev_tsp, None);
        *v.intersector_mut().point_mut(pid)? = tsp.clone();
        match r {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::sm_err(OP_DPI)),
        }

        // the first points must be on both surfaces
        if ii == 0
            && !tsp.srf_pos(SrfIndex::First).closer_than(
                v.intersector().this_approx_tol_3d,
                tsp.srf_pos(SrfIndex::Second),
            )
        {
            return Err(SolidError::sm_err(OP_DPI));
        }

        if ii == 0 {
            // a copy of the first point is the start point
            let mut start = TsectPnt::zeroed();
            start.assign(&tsp);
            let start_id = v.intersector_mut().alloc_point(start);
            // (status ignored)
            ignored(v.intersector_mut().add_start_point(k, start_id))?;
            v.intersector_mut().point_mut(pid)?.curve_parameter = 0.0;
            if !(opts.start_direction.is_some() && n > 1) {
                v.intersector_mut().start_point = Some(start_id);
            }
        } else if ii == n - 1 && opts.start_direction.is_some() && !opts.extend_after_end {
            // the last point ends the curve
            v.intersector_mut().end_point = Some(pid);
        } else {
            // the points between are through points (status ignored, but linking
            // a linked point is undefined upstream)
            if let Err(
                e @ (SolidError::InvalidInput { .. }
                | SolidError::PortInvariant { .. }
                | SolidError::Unported { .. }),
            ) = v.intersector_mut().add_through_point(pid)
            {
                return Err(e);
            }
        }
    }

    // trace from the last start point
    let tsp_id = v
        .intersector_mut()
        .start_points
        .pop()
        .ok_or_else(|| SolidError::null_pointer(OP_DPI))?;
    v.intersector_mut().curve_trace_direction = 1.0;

    // tracing beyond the end: clip the extension
    if v.intersector().extension_distance > 0.0
        && opts.extend_after_end
        && v.intersector().end_point.is_none()
    {
        v.intersector_mut().doing_extension_clipping = true;
    }

    // Reverse direction of tracing if indicated by the start direction
    if let Some(d) = opts.start_direction {
        let p = v.intersector_mut().point_mut(tsp_id)?;
        if d.dot(p.crv_deriv()) < -EFF_ZERO {
            p.curve_pv[1] = -p.curve_pv[1];
            p.uv_curve_pv[0][1] = -p.uv_curve_pv[0][1];
            p.uv_curve_pv[1][1] = -p.uv_curve_pv[1][1];
            v.intersector_mut().curve_trace_direction = -1.0;
        }
    }

    // the last curve point of the trace
    let mut final_tsp = None;

    let input_trace_direction = v.intersector().curve_trace_direction;
    let traced = match trace_curve_with(v, k, tsp_id, &out.curves_3d) {
        Ok(()) => true,
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => false,
    };
    if traced {
        // trace backwards from the first point too
        if opts.extend_before_start
            && !v.intersector().curve_is_closed
            && !v.intersector().curve_points.is_empty()
            && input_trace_direction == v.intersector().curve_trace_direction
        {
            v.intersector_mut().reverse_curve_direction()?;
            v.intersector_mut().curve_trace_direction *= -1.0;
            trace_curve_with(v, k, tsp_id, &out.curves_3d)?;
            v.intersector_mut().reverse_curve_direction()?;
            v.intersector_mut().curve_trace_direction *= -1.0;
        }

        final_tsp = v.intersector().curve_points.last().copied();

        // the curve points become curves
        v.flush_curve(k, &mut out)?;
    }

    // a trace that stopped at a singular point before the last point
    let last = *points
        .last()
        .ok_or_else(|| SolidError::invalid_input(OP_DPI, "no points"))?;
    let mut is_singular = false;
    if out.curves_3d.len() == 1 && v.intersector().point(last)?.curve_parameter < 0.0 && !inside_me
    {
        if let Some(f) = final_tsp {
            let (uv0, uv1) = {
                let p = v.intersector().point(f)?;
                (p.uv_pos(SrfIndex::First), p.uv_pos(SrfIndex::Second))
            };
            is_singular = surface_is_singularity(k, v.intersector().surface(SrfIndex::First), uv0)?;
            is_singular |=
                surface_is_singularity(k, v.intersector().surface(SrfIndex::Second), uv1)?;
        }
    }

    let mut res = IntersectionCurve::from_single(out);
    if is_singular {
        join_from_singular_point(v, k, last, opts, &mut res)?;
    }
    Ok(res)
}

/// The singular-point retry of `DoPointIntersection`: the curve from the end of
/// `res` to the point `last` is joined onto `res` when its start is within
/// `this_approx_tol_3d` of that end.
fn join_from_singular_point<'s, V: IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    last: TsectId,
    opts: &PointIntersectionOptions,
    res: &mut IntersectionCurve,
) -> Result<()> {
    let no_curve = || SolidError::invalid_input(OP_DPI, "no flushed curve");
    let ivl1 = res
        .curve_3d
        .as_ref()
        .ok_or_else(no_curve)?
        .natural_interval();

    // the end point of the current curve
    let mut end_tsp = TsectPnt::new();
    let uv_curve0 = res.uv_curves[0].as_ref().ok_or_else(no_curve)?;
    let pnt = uv_curve0.evaluate_point(k, ivl1.max())?;
    *end_tsp.uv_pos_mut(SrfIndex::First) = Point2::new(pnt.x, pnt.y);
    let uv_curve1 = res.uv_curves[1].as_ref().ok_or_else(no_curve)?;
    let pnt = uv_curve1.evaluate_point(k, ivl1.max())?;
    *end_tsp.uv_pos_mut(SrfIndex::Second) = Point2::new(pnt.x, pnt.y);
    let end_id = v.intersector_mut().alloc_point(end_tsp);

    // init the lists for the nested call
    v.intersector_mut().curve_points.clear();
    v.intersector_mut().start_points.clear();
    v.intersector_mut().through_points.clear();

    // the intersection from the other end to here
    let second = do_point_intersection_body(v, k, &[end_id, last], opts, true)?;

    if let Some(mut c2) = second.curve_3d {
        let c1 = res.curve_3d.as_mut().ok_or_else(no_curve)?;
        let crv1_pnt = c1.evaluate_point(k, ivl1.max())?;
        let ivl2 = c2.natural_interval();
        let crv2_pnt = c2.evaluate_point(k, ivl2.min())?;

        // curve end points close enough: join the curves
        if crv1_pnt.closer_than(v.intersector().this_approx_tol_3d, crv2_pnt) {
            join_curves(k, c1, &mut c2)?;
            let [u0, u1] = second.uv_curves;
            let mut u0 = u0.ok_or_else(no_curve)?;
            let mut u1 = u1.ok_or_else(no_curve)?;
            join_curves(k, res.uv_curves[0].as_mut().ok_or_else(no_curve)?, &mut u0)?;
            join_curves(k, res.uv_curves[1].as_mut().ok_or_else(no_curve)?, &mut u1)?;
        }
    }
    Ok(())
}

// ---- callees ----------------------------------------------------------------------------------

/// `rpCurve->JoinWith(1, pOther, 0)` of two flushed (B-spline) curves.
fn join_curves(k: &Kernel, this: &mut Curve, other: &mut Curve) -> Result<()> {
    let CurveKind::BSpline(other) = other.kind_mut() else {
        return Err(SolidError::invalid_input(
            OP_DPI,
            "flushed curve is not a B-spline",
        ));
    };
    let CurveKind::BSpline(this) = this.kind_mut() else {
        return Err(SolidError::invalid_input(
            OP_DPI,
            "flushed curve is not a B-spline",
        ));
    };
    this.join_with(k, true, other, false, None)
}

/// `SM_CAST_PTR(SmOffsetSurface, surface) != NULL`.
fn is_offset_surface(surface: &Surface) -> bool {
    matches!(surface.kind(), SurfaceKind::OffsetSurface(_))
}

/// `surface.IsSingularity(uv, eWhichDir)` (default tolerance): the plane override
/// (never singular); other kinds run `SmSurface::IsSingularity`.
fn surface_is_singularity(k: &Kernel, surface: &Surface, uv: Point2) -> Result<bool> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_singularity().0),
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
            Ok(surface.is_singularity(k, uv, EFF_ZERO, false)?.singular)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::plane::Plane;

    fn plane(o: [f64; 3], x: [f64; 3], y: [f64; 3]) -> Surface {
        Surface::from(Plane::new(
            Vec3::from_array(o),
            Vec3::from_array(x),
            Vec3::from_array(y),
            Point2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn unit() -> Extent2d {
        Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0)
    }

    #[test]
    fn tangent_start_point_is_a_single_point_curve() {
        let k = &Kernel::default();
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.5, -0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let mut si = SurfaceIntersector::new(k, &a, &unit(), &b, &unit(), false).unwrap();
        let mut p = TsectPnt::new();
        p.point_type = IntersectionPointType::TangentPoint;
        p.curve_pv[0] = Vec3::new(0.5, 0.5, 0.0);
        p.curve_parameter = 3.0;
        let id = si.alloc_point(p);
        si.trace_curve(&Kernel::default(), id, &[]).unwrap();
        assert_eq!(si.curve_points(), &[id]);
        assert!(si.curve_is_closed);
        assert_eq!(si.point(id).unwrap().curve_parameter, 0.0);
    }

    #[test]
    fn unknown_start_point_traces_nothing_more() {
        let k = &Kernel::default();
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.5, -0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let mut si = SurfaceIntersector::new(k, &a, &unit(), &b, &unit(), false).unwrap();
        let mut p = TsectPnt::zeroed();
        p.point_type = IntersectionPointType::Unknown;
        *p.uv_pos_mut(SrfIndex::First) = Point2::new(0.5, 0.5);
        *p.uv_pos_mut(SrfIndex::Second) = Point2::new(0.5, 0.5);
        *p.uv_deriv_mut(SrfIndex::First) = Point2::new(1.0, 0.0);
        *p.uv_deriv_mut(SrfIndex::Second) = Point2::new(1.0, 0.0);
        let id = si.alloc_point(p);
        // RefinePoint fails for unknown points: no next point is ever good
        si.trace_curve(&Kernel::default(), id, &[]).unwrap();
        assert_eq!(si.curve_points(), &[id]);
        assert!(!si.curve_is_closed);
    }

    #[test]
    fn linked_start_point_is_rejected() {
        let k = &Kernel::default();
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.5, -0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let mut si = SurfaceIntersector::new(k, &a, &unit(), &b, &unit(), false).unwrap();
        let id = si.alloc_point(TsectPnt::zeroed());
        si.add_start_point(k, id).unwrap();
        assert!(matches!(
            si.trace_curve(&Kernel::default(), id, &[]),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn point_intersection_needs_points_on_both_surfaces() {
        let k = &Kernel::default();
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.5, -0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let mut si = SurfaceIntersector::new(k, &a, &unit(), &b, &unit(), false).unwrap();
        let k = Kernel::default();
        let opts = PointIntersectionOptions::default();
        assert!(si.do_point_intersection(&k, &[], &opts).is_err());
        let mut p = TsectPnt::new();
        *p.uv_pos_mut(SrfIndex::First) = Point2::new(0.5, 0.1);
        *p.uv_pos_mut(SrfIndex::Second) = Point2::new(0.5, 0.5);
        let id = si.alloc_point(p);
        let r = si.do_point_intersection(&k, &[id], &opts);
        assert!(matches!(r, Err(SolidError::Upstream { .. })));
        assert_eq!(si.point(id).unwrap().curve_parameter, -1.0);
    }

    #[test]
    fn law_intersection_needs_two_offset_surfaces() {
        let k = &Kernel::default();
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.5, -0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let mut si = SurfaceIntersector::new(k, &a, &unit(), &b, &unit(), false).unwrap();
        let curve = Curve::from(
            crate::curve::line::Line::create_line_segment(
                3,
                Vec3::ZERO,
                Vec3::new(1.0, 0.0, 0.0),
                None,
            )
            .unwrap(),
        );
        let opts = LawIntersectionOptions {
            law_orient: false,
            point_curve: &curve,
            point_curve_interval: Extent1d::from_min_max(0.0, 1.0),
            surface_orientations: [1.0, 1.0],
            use_normal_plane_averaging: false,
            approx_tol_3d: Some(0.25),
        };
        let p = si.alloc_point(TsectPnt::new());
        let q = si.alloc_point(TsectPnt::new());
        let mut law = FilletLaw::new(crate::blend::fillet_standard_solver::p1::FilletLawKind::Base);
        assert!(matches!(
            si.do_law_intersection(k, &[p], &opts, &mut law),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR,
                ..
            })
        ));
        assert!(matches!(
            si.do_law_intersection(k, &[p, q], &opts, &mut law),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
        assert_eq!(si.this_approx_tol_3d, 0.25);
    }

    #[test]
    fn interior_curves_reject_self_intersection_and_skip_near_pairs() {
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let k = Kernel::default();
        let mut si = SurfaceIntersector::new(&k, &a, &unit(), &a, &unit(), false).unwrap();
        let mut out = TsectCurves::default();
        assert!(si.find_interior_curves(&k, &mut out).is_err());

        let b = plane([0.0, 0.5, -0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let mut si = SurfaceIntersector::new(&k, &a, &unit(), &b, &unit(), false).unwrap();
        // (completes since the generic IsoCurve holds a plane's iso boundaries, #76;
        // the curves are compared by difftest)
        let mut found = TsectCurves::default();
        si.find_interior_curves(&k, &mut found).unwrap();
        // a pair next to a curve already found is skipped
        out.curves_3d.push(Curve::from(
            crate::curve::line::Line::create_line_segment(
                3,
                Vec3::new(0.0, 0.5, 0.0),
                Vec3::new(1.0, 0.5, 0.0),
                None,
            )
            .unwrap(),
        ));
        let bx = Extent3d::from_bounds(0.4, 0.4, -0.1, 0.6, 0.6, 0.1);
        let pair = InteriorNodePair {
            bboxes: [bx, bx],
            uv_domains: [unit(), unit()],
        };
        si.find_interior_curves_from_node_pairs(&k, &[pair], &mut out)
            .unwrap();
        assert_eq!(out.curves_3d.len(), 1);
        // a far pair reaches the local intersection (surface.surface_p11, ported): it
        // succeeds, or stops at an unported boundary solve (surface.surface_p8)
        let far = Extent3d::from_bounds(5.0, 5.0, 5.0, 6.0, 6.0, 6.0);
        let pair = InteriorNodePair {
            bboxes: [far, far],
            uv_domains: [unit(), unit()],
        };
        if let Err(e) = si.find_interior_curves_from_node_pairs(&k, &[pair], &mut out) {
            assert!(e.is_unported(), "{e}");
        }
    }

    #[test]
    fn interior_pair_caches_preserve_lazy_errors_and_call_isolation() {
        let k = Kernel::default();
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.5, -0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let mut si = SurfaceIntersector::new(&k, &a, &unit(), &b, &unit(), false).unwrap();
        let line = Curve::from(
            crate::curve::line::Line::create_line_segment(
                3,
                Vec3::new(0.0, 0.5, 0.0),
                Vec3::new(1.0, 0.5, 0.0),
                None,
            )
            .unwrap(),
        );
        let invalid = Curve::from(crate::nurbs::b_spline_curve::BSplineCurve::default());
        let near = Extent3d::from_bounds(0.4, 0.4, -0.1, 0.6, 0.6, 0.1);
        let far = Extent3d::from_bounds(5.0, 5.0, 5.0, 6.0, 6.0, 6.0);
        let pair = InteriorNodePair {
            bboxes: [near, near],
            uv_domains: [unit(), unit()],
        };
        let mut out = TsectCurves::default();
        out.curves_3d.extend([line, invalid]);

        // The near first curve hides the malformed second curve, even when the
        // first tree is reused by subsequent pairs.
        si.find_interior_curves_from_node_pairs(&k, &[pair; 8], &mut out)
            .unwrap();
        assert_eq!(out.curves_3d.len(), 2);
        assert_eq!(
            out.curves_3d[0].evaluate_point(&k, 0.5).unwrap(),
            Vec3::new(0.5, 0.5, 0.0)
        );
        let far_pair = InteriorNodePair {
            bboxes: [far, far],
            ..pair
        };
        assert_eq!(
            si.find_interior_curves_from_node_pairs(&k, &[pair, far_pair], &mut out),
            Err(SolidError::null_pointer(OP_FIC))
        );

        // Pair validation still precedes even the first curve-cache build.
        let disjoint = InteriorNodePair {
            bboxes: [near, far],
            ..pair
        };
        out.curves_3d.remove(0);
        let mut expanded = near;
        expanded.expand_absolute(si.this_approx_tol_3d);
        assert_eq!(
            si.find_interior_curves_from_node_pairs(&k, &[disjoint], &mut out),
            expanded.intersect(&far).map(|_| ())
        );
        si.find_interior_curves_from_node_pairs(&k, &[], &mut out)
            .unwrap();
        // No cache from the earlier call may survive replacing its output curve.
        assert_eq!(
            si.find_interior_curves_from_node_pairs(&k, &[pair], &mut out),
            Err(SolidError::null_pointer(OP_FIC))
        );
    }
}
