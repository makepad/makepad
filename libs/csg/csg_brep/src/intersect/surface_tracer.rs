//! The surface tracer: the framework that marches curves across one surface in
//! parameter space (start points on the domain boundary and in the interior, step
//! size control, closed-loop and boundary detection, Bezier output), shared by the
//! drop-curve and silhouette tracers that supply the curve definition.
//!
//! Upstream: `source/SMLib/src/SmSurfaceTracer.cpp`, `source/SMLib/inc/SmSurfaceTracer.h`.
//!
//! | Rust                                                  | Upstream                                         |
//! |-------------------------------------------------------|--------------------------------------------------|
//! | [`TracePointType`]                                    | `SmTracePointType`                               |
//! | [`TracePoint`], [`TracePoint::new`], [`TracePoint::reset`] | `SmTracePnt`, `SmTracePnt()`, `~SmTracePnt`  |
//! | [`TracePointId`]                                      | `SmTracePnt*` (a block of `m_vTrcPntMgr`)        |
//! | [`SurfaceTracer`], [`SurfaceTracer::new`]             | `SmSurfaceTracer`, `SmSurfaceTracer(cpSurface, crUVDomain)` |
//! | `Drop` (implicit)                                     | `~SmSurfaceTracer`                               |
//! | [`SurfaceTracer::approx_tol_3d`], [`SurfaceTracer::ang_tol_rad`] | `GetThisApproxTol3d`, `GetThisAngTolRad` |
//! | [`SurfaceTrace`]                                      | the virtual interface of `SmSurfaceTracer`       |
//! | [`SurfaceTrace::do_trace`], [`do_trace_base`], [`TraceOutput`] | `SmSurfaceTracer::DoTrace`              |
//! | [`SurfaceTrace::is_point_on_curve`], [`is_point_on_curve_base`] | `SmSurfaceTracer::IsPointOnCurve`      |
//! | [`SurfaceTrace::find_boundary_start_points`], [`find_boundary_start_points_base`] | `SmSurfaceTracer::FindBoundaryStartPoints` |
//! | [`SurfaceTrace::compute_span_deviation`], [`compute_span_deviation_base`] | `SmSurfaceTracer::ComputeSpanDeviation` |
//! | [`SurfaceTrace::local_point_solve`]                   | `SmSurfaceTracer::LocalPointSolve`               |
//! | [`SurfaceTrace::trace_curve`], [`trace_curve_base`]   | `SmSurfaceTracer::TraceCurve`                    |
//! | [`flush_curve`]                                       | `SmSurfaceTracer::FlushCurve`                    |
//! | `flush_crv` (private)                                 | `sm_FlushCrv` (static)                           |
//! | [`SurfaceTrace::find_interior_curves`], [`find_interior_curves_base`] | `SmSurfaceTracer::FindInteriorCurves` |
//! | [`SurfaceTrace::compute_step_size`], [`SurfaceTracer::compute_step_size`] | `SmSurfaceTracer::ComputeStepSize` |
//! | [`SurfaceTrace::compute_point_values`]                | `SmSurfaceTracer::ComputePointValues`            |
//! | [`SurfaceTrace::compute_next_point`], [`compute_next_point_base`], [`NextPoint`] | `SmSurfaceTracer::ComputeNextPoint` |
//! | [`SurfaceTrace::test_iso_curve`], [`test_iso_curve_base`] | `SmSurfaceTracer::TestIsoCurve`              |
//! | [`SurfaceTrace::reverse_curve_direction`], [`SurfaceTracer::reverse_curve_direction`] | `SmSurfaceTracer::ReverseCurveDirection` |
//! | [`SurfaceTrace::find_solutions_on_boundary_curve`]    | `SmSurfaceTracer::FindSolutionsOnBoundaryCurve`  |
//! | [`SurfaceTrace::does_point_lie_on_curve`]             | `SmSurfaceTracer::DoesPointLieOnCurve`           |
//! | [`SurfaceTracer::add_point_to_curve`]                 | `SmSurfaceTracer::AddPointToCurve`               |
//! | [`SurfaceTracer::add_start_point`]                    | `SmSurfaceTracer::AddStartPoint`                 |
//! | [`SurfaceTrace::test_point`], [`test_point_base`]     | `SmSurfaceTracer::TestPoint`                     |
//! | [`SurfaceTrace::branch_may_contain_answers`], [`SurfaceTrace::local_solve`] | the `SmGlobalSolver` hooks a tracer overrides |
//! | [`TYPE`], [`TYPE_STRING`], [`is_kind_of`]             | `GetType`, `GetTypeString`, `IsKindOf` (`SM_COMMON`) |
//!
//! Deviations:
//! - **Virtual methods.** Derived tracers implement [`SurfaceTrace`] over their own
//!   state plus a [`SurfaceTracer`] (the base members), overriding its hooks. The
//!   base bodies of the overridable members are free functions (`*_base`) or
//!   inherent methods of [`SurfaceTracer`], so an override can still call
//!   "`SmSurfaceTracer::X`". The base `ComputePointValues`, `LocalPointSolve` and
//!   `FindSolutionsOnBoundaryCurve` fail with `SM_ERR`, as upstream;
//!   `DoesPointLieOnCurve` answers `false` (upstream signals `SM_ERR` and returns
//!   `FALSE`). [`SurfaceTracer`] itself implements [`SurfaceTrace`] with every
//!   default.
//! - **Trace points and lists.** The block manager `m_vTrcPntMgr` is an arena of
//!   [`TracePoint`]s addressed by [`TracePointId`] (never freed while the tracer
//!   lives, as upstream); `m_vCurvePoints` and `m_vStartPoints` are `Vec`s of ids
//!   in list order and `m_pStartPoint` an optional id, so the pointer identity tests
//!   are id comparisons. The circular `SmTList` is kept where it shows: the previous
//!   node of the first curve point is the last one. A point the trace may still
//!   reject is a local [`TracePoint`] and gets its arena slot when it is added to
//!   the curve (upstream takes the block first and leaks it on rejection).
//!   Points taken from the block manager start as [`TracePoint::zeroed`] (its
//!   blocks are zero-filled and never constructed); stack points as
//!   [`TracePoint::new`].
//! - **Surface cache.** `smsurf_GetSurfaceCache` ([`surface_cache`], not ported:
//!   [`SolidError::Unported`] for `surface.surface_p1`) cannot hand out a cache yet,
//!   so [`SurfaceTrace::do_trace`], [`SurfaceTrace::find_boundary_start_points`] and
//!   [`SurfaceTrace::find_interior_curves`] take the cache it returns for the
//!   surface as an argument. The constructor's `GetTree`
//!   ([`SurfaceCache::get_tree`], which builds the tree when needed) is made where the
//!   tree is used (the global solve of [`find_interior_curves_base`]).
//!   `SmCacheCheckOutIn` has no counterpart (the cache
//!   is borrowed for the whole trace), the `SmTemporaryChangeValue`s of
//!   `DoTrace` are restored on every exit.
//! - **Global solver.** The tracer's `SmGlobalSolver` base is made for the solve in
//!   [`find_interior_curves_base`] with the members the constructor sets (one tree,
//!   two variables, `SM_SO_INTERSECT`/`SM_SR_NODES` replaced by `SolveIt`'s
//!   arguments); [`SurfaceTrace::branch_may_contain_answers`] and
//!   [`SurfaceTrace::local_solve`] are the hooks derived tracers override.
//! - **Context.** `m_cpContext` (set by `DoTrace`, read when curves are made) has
//!   no counterpart: curves are values.
//! - **Out-params** become return values ([`NextPoint`], [`TraceOutput`],
//!   `Option<Point2>` for `LocalPointSolve`'s found flag and point). Output curve
//!   arrays are `Vec<Curve>`; `DoTrace`'s optional outputs are all returned.
//! - **Ignored statuses** stay ignored (`IsDegenerate`, `EvaluatePoint` and
//!   `DropPoint` in `IsPointOnCurve`, the surface evaluations of `FlushCurve`, the
//!   `TraceCurve`/`ReverseCurveDirection`/`FlushCurve` calls of
//!   `FindInteriorCurves`' reverse trace, `AddStartPoint`), except
//!   [`SolidError::Unported`], which is always propagated. `GetSurfaceContinuity`
//!   at most `SM_CT_C0` only signals upstream (`SE`), so the trace goes on.
//! - **Drop curve.** `SmSurface::DropCurve` is the plane override for planes and the
//!   base body ([`Surface::drop_curve_base`]) for the other kinds.
//! - **Undefined behaviour.** `ReverseCurveDirection` on an empty curve is
//!   `SM_ERR_NULL_POINTER` (upstream `NER`); a `TraceCurve` that ends with an empty
//!   curve and a `TestPoint` closure check on an empty curve (upstream dereferences
//!   the null last node) are [`SolidError::InvalidInput`], as is a boundary iso curve
//!   index past the four curves or a node solution without node.
//! - The `SM_DEBUG_CODE` drawing blocks, `SmTracePnt::Draw`, `SmTracePnt::Dump` and
//!   `SmSurfaceTracer::Dump` are not ported. No recursion depends on input here
//!   (the global solve bounds its own).

use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::p5::make_curve_cache;
use crate::curve::curve::Curve;
use crate::curve::hermite_curve::HermiteCurve;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{
    Branch, GlobalSolve, GlobalSolver, SolutionRequested, SolveRequest, SolverOperation,
};
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionObject, SolutionType,
};
use crate::geom::tol::is_zero;
use crate::geom::tree::{NodeData, SurfaceAux, Tree};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm, CanonicalData, KnotType};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::surface_cache;
use crate::surface::surface::Surface;
use crate::surface::surface_cache::SurfaceCache;

/// `SmSurfaceTracer_TYPE` (`SOLV_BASE_TYPE + 56`).
pub const TYPE: i64 = 19_056;
/// `SmGlobalSolver_TYPE` (`SOLV_BASE_TYPE + 4`).
pub const GLOBAL_SOLVER_TYPE: i64 = 19_004;
/// `"SmSurfaceTracer_TYPE"`.
pub const TYPE_STRING: &str = "SmSurfaceTracer_TYPE";

/// `SM_UNDEF_ULONG`.
const UNDEF_ULONG: u64 = 123_456_789;

/// `NUM_TEST_POINTS` of `ComputeSpanDeviation`.
const NUM_TEST_POINTS: usize = 5;
/// `NUM_ISO_TEST_POINTS` of `TestIsoCurve`.
const NUM_ISO_TEST_POINTS: usize = 10;

const OP_TRACE: &str = "intersect::surface_tracer::trace_curve";
const OP_REVERSE: &str = "intersect::surface_tracer::SurfaceTracer::reverse_curve_direction";
const OP_BOUNDARY: &str = "intersect::surface_tracer::find_boundary_start_points";
const OP_POINT: &str = "intersect::surface_tracer::SurfaceTracer::point";
const OP_HOOK: &str = "intersect::surface_tracer::SurfaceTrace";

/// `SmSurfaceTracer::IsKindOf`: the tracer type or `SmGlobalSolver_TYPE`
/// (`SmGlobalSolver::IsKindOf`, `SM_COMMON_BASE`).
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || t == GLOBAL_SOLVER_TYPE
}

/// The kind of a trace point (`SmTracePointType`); discriminants are upstream's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum TracePointType {
    /// `SM_TP_UNDEF`.
    #[default]
    Undef = 0,
    /// `SM_TP_NORMAL`.
    Normal = 1,
    /// `SM_TP_SINGULARITY`: the trace meets a singularity here.
    Singularity = 2,
    /// `SM_TP_TANGENT_POINT`: an isolated tangent point.
    TangentPoint = 3,
    /// `SM_TP_TANGENT_LINE`.
    TangentLine = 4,
}

impl TracePointType {
    /// The enumerator of an upstream value, `None` outside the enum.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => Self::Undef,
            1 => Self::Normal,
            2 => Self::Singularity,
            3 => Self::TangentPoint,
            4 => Self::TangentLine,
            _ => return None,
        })
    }
}

/// One sample of a traced curve (`SmTracePnt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TracePoint {
    /// `m_dCurveParameter`.
    pub curve_parameter: f64,
    /// `m_v3DCurvePV`: 3D position, 3D tangent (not unitized), unused.
    pub curve_3d: [Vec3; 3],
    /// `m_vUVCurvePV`: UV position, UV tangent, unused.
    pub curve_uv: [Vec2; 3],
    /// `m_vSurfacePV`: `[0][0]` position, `[1][0]` `S_u`, `[0][1]` `S_v`, `[1][1]`
    /// `S_uv`, `[2][0]` `S_uu`, `[0][2]` `S_vv`, `[2][2]` the unit normal.
    pub surface: [[Vec3; 3]; 3],
    /// `m_dSurfDropDist`: curve point to surface drop point distance along the
    /// normal line.
    pub surf_drop_dist: f64,
    /// `m_dSurfNormLineDist`: curve point to surface normal line distance.
    pub surf_norm_line_dist: f64,
    /// `m_ePointType`.
    pub point_type: TracePointType,
    /// `m_lUserLong`.
    pub user_long: u64,
    /// `m_dUserDouble1`.
    pub user_double1: f64,
    /// `m_dUserDouble2`.
    pub user_double2: f64,
}

impl Default for TracePoint {
    fn default() -> Self {
        Self::new()
    }
}

impl TracePoint {
    /// `SmTracePnt()`: undefined scalars and type; zero vectors (upstream leaves
    /// them uninitialised).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            curve_parameter: UNDEF_DOUBLE,
            curve_3d: [Vec3::ZERO; 3],
            curve_uv: [Vec2::ZERO; 3],
            surface: [[Vec3::ZERO; 3]; 3],
            surf_drop_dist: UNDEF_DOUBLE,
            surf_norm_line_dist: UNDEF_DOUBLE,
            point_type: TracePointType::Undef,
            user_long: UNDEF_ULONG,
            user_double1: UNDEF_DOUBLE,
            user_double2: UNDEF_DOUBLE,
        }
    }

    /// A fresh block of `m_vTrcPntMgr` (`GetNewElement`: zero-filled memory, no
    /// constructor run): every field zero, type [`TracePointType::Undef`].
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            curve_parameter: 0.0,
            curve_3d: [Vec3::ZERO; 3],
            curve_uv: [Vec2::ZERO; 3],
            surface: [[Vec3::ZERO; 3]; 3],
            surf_drop_dist: 0.0,
            surf_norm_line_dist: 0.0,
            point_type: TracePointType::Undef,
            user_long: 0,
            user_double1: 0.0,
            user_double2: 0.0,
        }
    }

    /// `~SmTracePnt`: resets the scalars and the type (the vectors are kept).
    pub fn reset(&mut self) {
        self.curve_parameter = UNDEF_DOUBLE;
        self.surf_drop_dist = UNDEF_DOUBLE;
        self.surf_norm_line_dist = UNDEF_DOUBLE;
        self.point_type = TracePointType::Undef;
        self.user_long = UNDEF_ULONG;
        self.user_double1 = UNDEF_DOUBLE;
        self.user_double2 = UNDEF_DOUBLE;
    }

    /// `SmPoint3d(m_vUVCurvePV[0])`.
    fn uv_point_3d(&self) -> Point3 {
        Vec3::from(self.curve_uv[0])
    }
}

/// The object of the boundary solutions of
/// [`SurfaceTrace::find_solutions_on_boundary_curve`]: the boundary iso curve.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoundaryCurve {
    /// The curve's natural interval (`SmCurve::GetNaturalInterval`), when known.
    pub natural_interval: Option<Extent1d>,
}

impl SolutionObject for BoundaryCurve {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        self.natural_interval
    }
}

/// A trace point in the tracer's arena (`SmTracePnt*` into `m_vTrcPntMgr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TracePointId(pub usize);

/// The base members of a surface tracer (`SmSurfaceTracer`'s data).
#[derive(Debug, Clone)]
pub struct SurfaceTracer<'s> {
    /// `m_cpSurface`: the surface the curves are traced on.
    pub surface: &'s Surface,
    /// `m_vUVDomain`: the domain traced in.
    pub uv_domain: Extent2d,
    /// `m_dThisAngTolRad`: largest tangent angle change between two trace points.
    pub ang_tol_rad: f64,
    /// `m_dThisApproxTol3d`: largest curve deviation between two trace points.
    pub approx_tol_3d: f64,
    /// `m_vTrcPntMgr`: every trace point made.
    points: Vec<TracePoint>,
    /// `m_pStartPoint`: start of the interior trace in progress.
    pub start_point: Option<TracePointId>,
    /// `m_vCurvePoints`: the trace points of the curve being traced, in order.
    pub curve_points: Vec<TracePointId>,
    /// `m_dCurveTraceDirection`: `1` forward, `-1` backward.
    pub curve_trace_direction: f64,
    /// `m_bCurveIsClosed`.
    pub curve_is_closed: bool,
    /// `m_vStartPoints`: start points of the curves still to trace.
    pub start_points: Vec<TracePointId>,
}

impl<'s> SurfaceTracer<'s> {
    /// `SmSurfaceTracer(cpSurface, crUVDomain)`: the default approximation
    /// tolerance is a thousandth of the length of the polyline through the domain's
    /// corners and middle; the angle tolerance is 20 degrees.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the surface evaluations (whose other failures
    /// upstream only signals; the tolerance is then made from whatever points were
    /// evaluated, here from the origin).
    pub fn new(k: &Kernel, surface: &'s Surface, uv_domain: &Extent2d) -> Result<Self> {
        // Compute a reasonable default tolerance for intersection based on the sizes
        // of the surfaces.
        let p1 = ignored(surface.evaluate_point(k, uv_domain.min()))?.unwrap_or(Vec3::ZERO);
        let p_mid =
            ignored(surface.evaluate_point(k, uv_domain.evaluate(0.5, 0.5)))?.unwrap_or(Vec3::ZERO);
        let p2 = ignored(surface.evaluate_point(k, uv_domain.max()))?.unwrap_or(Vec3::ZERO);
        let size = p1.distance(p_mid) + p_mid.distance(p2);
        let approx_tol_3d = size / 1000.0; // accuracy is 1/1000th size of surface

        Ok(Self {
            surface,
            uv_domain: *uv_domain,
            ang_tol_rad: 20.0 * PI / 180.0,
            approx_tol_3d,
            points: Vec::new(),
            start_point: None,
            curve_points: Vec::new(),
            curve_trace_direction: 1.0,
            curve_is_closed: false,
            start_points: Vec::new(),
        })
    }

    /// `GetThisApproxTol3d`.
    #[must_use]
    pub const fn approx_tol_3d(&self) -> f64 {
        self.approx_tol_3d
    }

    /// `GetThisAngTolRad`.
    #[must_use]
    pub const fn ang_tol_rad(&self) -> f64 {
        self.ang_tol_rad
    }

    /// `m_vTrcPntMgr.GetNewElement()` filled with `point`.
    pub fn new_point(&mut self, point: TracePoint) -> TracePointId {
        self.points.push(point);
        TracePointId(self.points.len() - 1)
    }

    /// The trace point `id`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an id this tracer did not make.
    pub fn point(&self, id: TracePointId) -> Result<&TracePoint> {
        self.points
            .get(id.0)
            .ok_or(SolidError::invalid_input(OP_POINT, "unknown trace point"))
    }

    /// The trace point `id`, mutable.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an id this tracer did not make.
    pub fn point_mut(&mut self, id: TracePointId) -> Result<&mut TracePoint> {
        self.points
            .get_mut(id.0)
            .ok_or(SolidError::invalid_input(OP_POINT, "unknown trace point"))
    }

    /// The number of trace points made.
    #[must_use]
    pub fn point_count(&self) -> usize {
        self.points.len()
    }

    /// The node before `id` in the circular curve point list
    /// (`m_vCurvePoints.GetPrevNode`): the last node before the first.
    fn prev_curve_point(&self, id: TracePointId) -> Option<TracePointId> {
        let i = self.curve_points.iter().position(|&p| p == id)?;
        if i == 0 {
            self.curve_points.last().copied()
        } else {
            self.curve_points.get(i - 1).copied()
        }
    }

    /// `SmSurfaceTracer::ComputeStepSize`: the old step, or a hundredth of the
    /// smaller domain side when the old step is zero.
    #[must_use]
    pub fn compute_step_size(&self, old_step_size: f64) -> f64 {
        // Surface UVDomain of interest
        let uv_size = self.uv_domain.size();

        // default step size - expect derived classes to compute this differently
        if is_zero(old_step_size) {
            min2(uv_size.x, uv_size.y) * 1.0e-2
        } else {
            old_step_size
        }
    }

    /// `AddPointToCurve`: appends `id` to the curve points.
    pub fn add_point_to_curve(&mut self, id: TracePointId) {
        self.curve_points.push(id);
    }

    /// `AddStartPoint`: appends `id` to the start points unless a start point lies
    /// within `SM_EFF_ZERO` (squared UV distance) of it; two singular points match
    /// only when their 3D tangents are parallel and point the same way.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for unknown ids.
    pub fn add_start_point(&mut self, id: TracePointId) -> Result<()> {
        let start = *self.point(id)?;
        let mut found_match = false;

        // for every startpoint
        for &p in &self.start_points {
            // get dist to candidate point
            let tsp = self.point(p)?;
            let dist_uv1_sq = start.curve_uv[0].distance_squared(tsp.curve_uv[0]);

            // when candidate points are near one another
            if dist_uv1_sq < EFF_ZERO {
                // when both points mark a surface singularity
                if start.point_type == TracePointType::Singularity {
                    if tsp.point_type == TracePointType::Singularity {
                        // points are the same when tangents are the same
                        let cross = start.curve_3d[1].cross(tsp.curve_3d[1]);
                        if cross.length_squared() < EFF_ZERO_SQ
                            && start.curve_3d[1].dot(tsp.curve_3d[1]) > 0.0
                        {
                            found_match = true; // Yes the tangents match
                            break;
                        }
                    }
                } else {
                    // not singularity just distance check is fine
                    found_match = true;
                    break;
                }
            }
        }

        // when candidate point is unique
        if !found_match {
            self.start_points.push(id);
        }
        Ok(())
    }

    /// `SmSurfaceTracer::ReverseCurveDirection`: reverses the curve points, their
    /// tangents and their parameters (the new first point has parameter `0`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for an empty curve.
    pub fn reverse_curve_direction(&mut self) -> Result<()> {
        if self.curve_points.is_empty() {
            return Err(SolidError::null_pointer(OP_REVERSE));
        }

        // Grab points in m_vCurvePoints list into an array, and empty the list.
        let pnts = core::mem::take(&mut self.curve_points);

        let mut curr_param = 0.0;
        for i in (1..=pnts.len()).rev() {
            let id = *pnts
                .get(i - 1)
                .ok_or(SolidError::invalid_input(OP_REVERSE, "index"))?;
            let prev_param = if i > 1 {
                let prev = *pnts
                    .get(i - 2)
                    .ok_or(SolidError::invalid_input(OP_REVERSE, "index"))?;
                Some(self.point(prev)?.curve_parameter)
            } else {
                None
            };
            let pnt = self.point_mut(id)?;
            pnt.curve_3d[1] = -pnt.curve_3d[1];
            pnt.curve_3d[2] = -pnt.curve_3d[2];
            pnt.curve_uv[1] = -pnt.curve_uv[1];
            pnt.curve_uv[2] = -pnt.curve_uv[2];
            let delta_param = match prev_param {
                Some(p) => pnt.curve_parameter - p,
                None => 0.0,
            };
            pnt.curve_parameter = curr_param;
            curr_param += delta_param;
            self.curve_points.push(id);
        }
        Ok(())
    }
}

/// `smos_Min`.
fn min2(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `smos_Fabs`.
fn fabs(a: f64) -> f64 {
    if a < 0.0 {
        -a
    } else {
        a
    }
}

/// What [`SurfaceTrace::compute_next_point`] found (`ComputeNextPoint`'s outputs).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NextPoint {
    /// `rbFoundGoodPoint`: the step satisfies the tolerances.
    pub found_good_point: bool,
    /// `rdDeviationFound`: the span deviation.
    pub deviation: f64,
    /// `rdAngleFoundRad`: the tangent angle change.
    pub angle_rad: f64,
    /// `rbClipped`: the step would cross the domain boundary and was clipped (or
    /// ended at a singularity).
    pub clipped: bool,
    /// `rbBoundaryHit`: the current point is on the boundary and the step leaves.
    pub boundary_hit: bool,
}

/// The curves [`SurfaceTrace::do_trace`] (and the helpers adding to its arrays)
/// produce: one entry per traced curve in each array.
#[derive(Debug, Clone, Default)]
pub struct TraceOutput {
    /// `p3DCurves`: the 3D curves.
    pub curves_3d: Vec<Curve>,
    /// `pSurfaceUVCurves`: the UV curves.
    pub uv_curves: Vec<Curve>,
    /// `pMaxDropDists`: the largest curve to surface drop distance of each curve.
    pub max_drop_dists: Vec<f64>,
    /// `pDeviations`: the largest curve to surface normal line distance of each
    /// curve.
    pub deviations: Vec<f64>,
}

/// The virtual interface of `SmSurfaceTracer`. Implementors hold a
/// [`SurfaceTracer`] (the base members) and override the hooks; the provided
/// methods are the base bodies.
pub trait SurfaceTrace<'s> {
    /// The base members.
    fn tracer(&self) -> &SurfaceTracer<'s>;

    /// The base members, mutable.
    fn tracer_mut(&mut self) -> &mut SurfaceTracer<'s>;

    /// `ComputePointValues`: fills `pnt` with the surface point at `uv` and the
    /// traced curve's position and tangents there (`prev`: the previous point on
    /// the curve, for singular cases; `step_size`: unused upstream).
    ///
    /// # Errors
    ///
    /// `SM_ERR` in the base (a pure virtual upstream).
    fn compute_point_values(
        &mut self,
        k: &Kernel,
        uv: Point2,
        pnt: &mut TracePoint,
        prev: Option<&TracePoint>,
        step_size: Option<f64>,
    ) -> Result<()> {
        let _ = (k, uv, pnt, prev, step_size);
        Err(SolidError::sm_err(OP_HOOK))
    }

    /// `LocalPointSolve`: refines `guess` to a point of the traced curve near
    /// `curr`; `None` when no answer was found. `next` is not used upstream.
    ///
    /// # Errors
    ///
    /// `SM_ERR` in the base (a pure virtual upstream).
    fn local_point_solve(
        &self,
        k: &Kernel,
        curr: &TracePoint,
        guess: Point2,
        next: &mut TracePoint,
    ) -> Result<Option<Point2>> {
        let _ = (k, curr, guess, next);
        Err(SolidError::sm_err(OP_HOOK))
    }

    /// `FindSolutionsOnBoundaryCurve`: approximate start points along the boundary
    /// iso curve `boundary` (over `interval`; `side` 0 low, 1 high; `param` its
    /// constant parameter), as solutions whose first start parameter is the curve
    /// parameter.
    ///
    /// # Errors
    ///
    /// `SM_ERR` in the base (a pure virtual upstream).
    fn find_solutions_on_boundary_curve(
        &self,
        k: &Kernel,
        boundary: &Curve,
        interval: &Extent1d,
        side: usize,
        param: SurfParamType,
    ) -> Result<SolutionArray<BoundaryCurve>> {
        let _ = (k, boundary, interval, side, param);
        Err(SolidError::sm_err(OP_HOOK))
    }

    /// `DoesPointLieOnCurve`: whether `uv` lies on the traced curve, refining it
    /// onto the curve when `refine`. The base answers `false` (upstream also
    /// signals `SM_ERR`).
    ///
    /// # Errors
    ///
    /// None in the base.
    fn does_point_lie_on_curve(&self, k: &Kernel, uv: &mut Point2, refine: bool) -> Result<bool> {
        let _ = (k, uv, refine);
        Ok(false)
    }

    /// `ComputeStepSize`: the UV step from `pnt` given the last one
    /// ([`SurfaceTracer::compute_step_size`]).
    ///
    /// # Errors
    ///
    /// None in the base.
    fn compute_step_size(&mut self, pnt: TracePointId, old_step_size: f64) -> Result<f64> {
        let _ = pnt;
        Ok(self.tracer().compute_step_size(old_step_size))
    }

    /// `IsPointOnCurve` ([`is_point_on_curve_base`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the curve queries.
    fn is_point_on_curve(&self, k: &Kernel, point: Point3, curves: &[Curve]) -> Result<bool> {
        is_point_on_curve_base(self.tracer(), k, point, curves)
    }

    /// `ComputeNextPoint` ([`compute_next_point_base`]).
    ///
    /// # Errors
    ///
    /// As [`compute_next_point_base`].
    fn compute_next_point(
        &mut self,
        k: &Kernel,
        curr: TracePointId,
        step_size: f64,
        next: &mut TracePoint,
    ) -> Result<NextPoint> {
        compute_next_point_base(self, k, curr, step_size, next)
    }

    /// `ComputeSpanDeviation` ([`compute_span_deviation_base`]).
    ///
    /// # Errors
    ///
    /// As [`compute_span_deviation_base`].
    fn compute_span_deviation(
        &mut self,
        k: &Kernel,
        curr: &TracePoint,
        next: &TracePoint,
        step_size: f64,
    ) -> Result<(Option<f64>, bool)> {
        compute_span_deviation_base(self, k, curr, next, step_size)
    }

    /// `TestPoint` ([`test_point_base`]).
    ///
    /// # Errors
    ///
    /// As [`test_point_base`].
    fn test_point(&mut self, k: &Kernel, next: &mut TracePoint) -> Result<bool> {
        test_point_base(self, k, next)
    }

    /// `TraceCurve` ([`trace_curve_base`]).
    ///
    /// # Errors
    ///
    /// As [`trace_curve_base`].
    fn trace_curve(&mut self, k: &Kernel, start: TracePointId, curves: &[Curve]) -> Result<()> {
        trace_curve_base(self, k, start, curves)
    }

    /// `ReverseCurveDirection` ([`SurfaceTracer::reverse_curve_direction`]).
    ///
    /// # Errors
    ///
    /// As [`SurfaceTracer::reverse_curve_direction`].
    fn reverse_curve_direction(&mut self) -> Result<()> {
        self.tracer_mut().reverse_curve_direction()
    }

    /// `TestIsoCurve` ([`test_iso_curve_base`]).
    ///
    /// # Errors
    ///
    /// As [`test_iso_curve_base`].
    fn test_iso_curve(&mut self, k: &Kernel, param: SurfParamType, value: f64) -> Result<bool> {
        test_iso_curve_base(self, k, param, value)
    }

    /// `FindBoundaryStartPoints` ([`find_boundary_start_points_base`]).
    ///
    /// # Errors
    ///
    /// As [`find_boundary_start_points_base`].
    fn find_boundary_start_points<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        out: &mut TraceOutput,
    ) -> Result<()> {
        find_boundary_start_points_base(self, k, cache, out)
    }

    /// `FindInteriorCurves` ([`find_interior_curves_base`]).
    ///
    /// # Errors
    ///
    /// As [`find_interior_curves_base`].
    fn find_interior_curves<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        out: &mut TraceOutput,
    ) -> Result<()> {
        find_interior_curves_base(self, k, cache, out)
    }

    /// `DoTrace` ([`do_trace_base`]).
    ///
    /// # Errors
    ///
    /// As [`do_trace_base`].
    fn do_trace<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<TraceOutput> {
        do_trace_base(self, k, cache, approx_tol_3d, ang_tol_rad)
    }

    /// `BranchMayContainAnswers` of the tracer's global solve (the base:
    /// `SmGlobalSolver::BranchMayContainAnswers`); `cache` owns the solved tree and
    /// the node data its nodes point at.
    ///
    /// # Errors
    ///
    /// Those of the global solver.
    fn branch_may_contain_answers<O: SolutionObject>(
        &mut self,
        solver: &GlobalSolver<'_, O>,
        cache: &SurfaceCache<O>,
        branch: &Branch,
    ) -> Result<bool> {
        let _ = cache;
        solver.branch_may_contain_answers(branch)
    }

    /// `LocalSolve` of the tracer's global solve (the base:
    /// `SmGlobalSolver::LocalSolve`).
    ///
    /// # Errors
    ///
    /// Those of the global solver.
    fn local_solve<O: SolutionObject>(
        &mut self,
        solver: &mut GlobalSolver<'_, O>,
        branch: &Branch,
    ) -> Result<bool> {
        solver.local_solve(branch)
    }
}

impl<'s> SurfaceTrace<'s> for SurfaceTracer<'s> {
    fn tracer(&self) -> &SurfaceTracer<'s> {
        self
    }

    fn tracer_mut(&mut self) -> &mut SurfaceTracer<'s> {
        self
    }
}

/// `SmSurfaceTracer::IsPointOnCurve`: whether `point` lies within
/// `4 * approx_tol_3d` of the start of a degenerate curve of `curves`, or drops to
/// another curve within `10 * approx_tol_3d`. Failing curve queries skip the curve.
///
/// # Errors
///
/// [`SolidError::Unported`] from the curve queries.
pub fn is_point_on_curve_base(
    t: &SurfaceTracer<'_>,
    k: &Kernel,
    point: Point3,
    curves: &[Curve],
) -> Result<bool> {
    let mut point_is_on_curve = false;

    // for every previously trace curve
    for curve in curves {
        // When curve is degenerate - check Dist(CurvePnt, PointToTest)
        // (IsDegenerate's defaults: SM_EFF_ZERO, no interval)
        if ignored(curve.is_degenerate(k, EFF_ZERO, None))? == Some(true) {
            // Get CurvePnt3d
            let ivl = curve.natural_interval();
            let Some(crv_pnt) = ignored(curve.evaluate_point(k, ivl.min()))? else {
                continue;
            };

            // check Dist3d(PointToTest, CrvPnt) to m_dThisApproxTol3d*4.0
            if crv_pnt.distance(point) < t.approx_tol_3d * 4.0 {
                point_is_on_curve = true;
                break;
            }
        } else {
            // current curve is not degenerate - check Dist3d with max of m_dThisApproxTol3d*10.0
            let Some(dropped) = ignored(curve.drop_point(
                k,
                &curve.natural_interval(),
                point,
                None,
                t.approx_tol_3d * 10.0,
                None,
                SolverOperation::Intersect,
            ))?
            else {
                continue;
            };

            if dropped.success {
                point_is_on_curve = true;
                break;
            }
        }
    }
    Ok(point_is_on_curve)
}

/// `SmSurfaceTracer::ComputeNextPoint`: steps `step_size` along the UV tangent of
/// `curr` (clipped to the domain), refines the guess onto the curve
/// ([`SurfaceTrace::local_point_solve`]), evaluates it into `next`
/// ([`SurfaceTrace::compute_point_values`]) and judges the step: too large a
/// tangent turn, a step backwards or too large a span deviation
/// ([`SurfaceTrace::compute_span_deviation`]) is not a good point. A singular
/// `curr` other than the first curve point ends the trace (clipped and on the
/// boundary).
///
/// # Errors
///
/// `InvalidInput` for an unknown `curr`; those of the hooks and of
/// `AngleBetween`.
pub fn compute_next_point_base<'s, T: SurfaceTrace<'s> + ?Sized>(
    t: &mut T,
    k: &Kernel,
    curr: TracePointId,
    step_size: f64,
    next: &mut TracePoint,
) -> Result<NextPoint> {
    // init outputs
    let mut out = NextPoint::default();
    let curr_pnt = *t.tracer().point(curr)?;
    let mut step_size = step_size;

    // If we hit a singularity last time then we need to stop
    if curr_pnt.point_type == TracePointType::Singularity
        && t.tracer().curve_points.first() != Some(&curr)
    {
        out.clipped = true;
        out.boundary_hit = true;
        out.found_good_point = false;
        return Ok(out);
    }

    // init output
    next.point_type = TracePointType::Normal;

    // Check whether the proposed step will cross a domain boundary.
    let old_step_size = step_size;
    let domain = t.tracer().uv_domain;
    step_size = domain.clip_line(curr_pnt.curve_uv[0], curr_pnt.curve_uv[1], step_size);

    if step_size < old_step_size - EFF_ZERO * old_step_size {
        out.clipped = true;
    }

    if is_zero(step_size) {
        out.found_good_point = false;
        out.boundary_hit = true;
        return Ok(out);
    }

    // Take the step in uv and clamp to domain.
    let mut guess_uv = curr_pnt.curve_uv[0] + step_size * curr_pnt.curve_uv[1];
    guess_uv = domain.clamp_point(guess_uv);

    // Sometimes we need the next curve parameter to compute point values
    next.curve_parameter = curr_pnt.curve_parameter + step_size;

    // Now that we have guess for the next point - do local iteration
    let Some(refined_uv) = t.local_point_solve(k, &curr_pnt, guess_uv, next)? else {
        out.found_good_point = false;
        return Ok(out);
    };

    // Put RefinedUV into rNextPnt, and evaluate it there.
    t.compute_point_values(k, refined_uv, next, Some(&curr_pnt), None)?;

    // Adjust step size because it may have been clipped.
    let mut diff = next.curve_uv[0] - curr_pnt.curve_uv[0];
    let step_actual = diff.length();
    diff = guess_uv - curr_pnt.curve_uv[0];
    let step_predicted = diff.length();
    if step_actual < EFF_ZERO || step_predicted < EFF_ZERO {
        out.found_good_point = false;
        out.boundary_hit = true;
        return Ok(out);
    }
    step_size *= step_actual / step_predicted;

    next.curve_parameter = curr_pnt.curve_parameter + step_size;

    // First do a quick angular test to see if we can reject this span
    out.angle_rad = curr_pnt.curve_3d[1].angle_between(next.curve_3d[1])?;
    if out.angle_rad > t.tracer().ang_tol_rad && step_size > EFF_ZERO * 100.0 {
        out.found_good_point = false;
        return Ok(out);
    }
    // Also make sure that the point is going in the same direction.
    let line_param = line_closest_point(
        Vec3::from(curr_pnt.curve_uv[0]),
        Vec3::from(curr_pnt.curve_uv[1]),
        Vec3::from(next.curve_uv[0]),
    );
    if line_param < 0.0 {
        out.found_good_point = false;
        return Ok(out);
    }

    // Compute the deviation from the actual curve.
    let (deviation, satisfies) = t.compute_span_deviation(k, &curr_pnt, next, step_size)?;
    if let Some(d) = deviation {
        out.deviation = d;
    }
    out.found_good_point = satisfies;

    // If we have a singularity and it satisfies tolerance then we should stop here.
    if next.point_type == TracePointType::Singularity {
        out.clipped = true;
    }
    Ok(out)
}

/// `SmSurfaceTracer::ComputeSpanDeviation`: the largest distance, at five inner
/// parameters, between the 3D Hermite span of `curr`..`next` and the surface at
/// the refined point of the UV Hermite span. Returns the deviation (`None` when
/// the span fails, the deviation output is then left alone as upstream) and
/// whether the span satisfies the approximation tolerance.
///
/// # Errors
///
/// Those of the surface evaluations and of
/// [`SurfaceTrace::local_point_solve`].
pub fn compute_span_deviation_base<'s, T: SurfaceTrace<'s> + ?Sized>(
    t: &mut T,
    k: &Kernel,
    curr: &TracePoint,
    next: &TracePoint,
    step_size: f64,
) -> Result<(Option<f64>, bool)> {
    // Create two Hermite curves (parameter space and 3d) and test some points
    // on the curves as projected into 3d by their corresponding surfaces.
    let hermite_uv = HermiteCurve::from_2d(
        curr.curve_uv[0],
        curr.curve_uv[1] * step_size,
        next.curve_uv[0],
        next.curve_uv[1] * step_size,
        3,
    );
    let hermite_3d = HermiteCurve::new(
        curr.curve_3d[0],
        curr.curve_3d[1] * step_size,
        next.curve_3d[0],
        next.curve_3d[1] * step_size,
        3,
    );

    let mut max_dist = 0.0;
    let approx_tol = t.tracer().approx_tol_3d;
    let tol_sq = approx_tol * approx_tol;
    for i in 0..NUM_TEST_POINTS {
        let param = (i as f64 + 1.0) / (NUM_TEST_POINTS as f64 + 1.0);
        let crv_pnt = hermite_uv.evaluate_point(param);
        let mut uv = Vec2::new(crv_pnt.x, crv_pnt.y);
        uv = t.tracer().uv_domain.clamp_point(uv);
        t.tracer().surface.evaluate_point(k, uv)?;

        let mut dummy = TracePoint::new();
        let Some(uv_found) = t.local_point_solve(k, curr, uv, &mut dummy)? else {
            return Ok((None, false));
        };
        let crv_pnt2 = t.tracer().surface.evaluate_point(k, uv_found)?;
        let crv_pnt3 = hermite_3d.evaluate_point(param);
        let dist_sq = crv_pnt3.distance_squared(crv_pnt2);
        if dist_sq > max_dist {
            max_dist = dist_sq;
            if max_dist > tol_sq {
                return Ok((None, false));
            }
        }
    }

    Ok((Some(max_dist.sqrt()), true)) // passed test
}

/// `SmSurfaceTracer::TestPoint`: whether the trace ends at `next`: at a tangent or
/// singular point; when it comes back within `3 * approx_tol_3d` of the interior
/// start point (`next` is then moved to the start point's UV and the curve is
/// closed); or on the domain boundary, where the start points within
/// `SM_EFF_ZERO` (squared UV distance) of `next` are removed.
///
/// # Errors
///
/// `InvalidInput` for unknown ids; those of the Hermite box and point solve and of
/// [`SurfaceTrace::compute_point_values`].
pub fn test_point_base<'s, T: SurfaceTrace<'s> + ?Sized>(
    t: &mut T,
    k: &Kernel,
    next: &mut TracePoint,
) -> Result<bool> {
    // If point is at a tangent point then quit right here.
    if next.point_type == TracePointType::TangentPoint {
        return Ok(true);
    }

    // If point is at a singularity point then quit right here.
    if next.point_type == TracePointType::Singularity {
        return Ok(true);
    }

    // If we have a start point check for a closed curve situation
    if let Some(start_id) = t.tracer().start_point {
        // Don't check if we are only starting the curve
        let prev1 = t.tracer().curve_points.last().copied();
        if prev1 != Some(start_id) {
            let prev2 = prev1.and_then(|p| t.tracer().prev_curve_point(p));
            if prev2 != Some(start_id) {
                // upstream dereferences the last node
                let prev1 = prev1.ok_or(SolidError::invalid_input(OP_TRACE, "empty curve"))?;
                let p1 = *t.tracer().point(prev1)?;
                let start = *t.tracer().point(start_id)?;
                let approx_tol = t.tracer().approx_tol_3d;

                // make the hermite curve for the last trace segment
                let delta_t = next.curve_parameter - p1.curve_parameter;
                let herm = HermiteCurve::new(
                    p1.curve_3d[0],
                    p1.curve_3d[1] * delta_t,
                    next.curve_3d[0],
                    next.curve_3d[1] * delta_t,
                    3,
                );

                // get hermite curve's BBox
                let mut box_3d = Extent3d::new();
                herm.calculate_bounding_box(Some(&mut box_3d), None, None)?;

                // Build BBox centered on StartPoint sized 2*m_dThisApproxTol3d
                let mut pnt_box = Extent3d::from_point(start.curve_3d[0]);
                pnt_box.expand_absolute(approx_tol * 2.0);

                // when HermiteCurve BBox and StartPoint BBox intersect
                if !box_3d.are_disjoint(&pnt_box, 0.0) {
                    // locals - HermiteCurve's midPoint Param
                    let ivl = herm.natural_interval();
                    let guess = ivl.evaluate(0.5);
                    let herm_curve = Curve::from(herm);

                    // find closest HermiteCurve pt to StartPoint
                    let (success, sol) = herm_curve.local_point_solve::<()>(
                        k,
                        &ivl,
                        SolverOperation::Minimize,
                        start.curve_3d[0],
                        &PointSolveOptions::default(),
                        guess,
                    )?;

                    // when Dist(StartPoint,HermiteCurve) < m_dThisApproxTol3d*3.0
                    if success && sol.start.solution_value < approx_tol * 3.0 {
                        // Have found loop around - snap rNextPnt to StartPoint's UV position
                        let uv = start.curve_uv[0];
                        t.compute_point_values(k, uv, next, None, None)?;
                        t.tracer_mut().curve_is_closed = true;
                        return Ok(true);
                    }
                }
            }
        }
    }

    // is NextPnt.UVPnt on m_vUVDomain boundary (tol = SM_EFF_ZERO)
    let (on_boundary, _) = t
        .tracer()
        .uv_domain
        .is_point_on_boundary(next.curve_uv[0], EFF_ZERO);

    // exit case - TracePnt is in the middle of the surface - not the end of this trace
    if !on_boundary {
        return Ok(false);
    }

    // arrive here when rNextPnt is on a Surface boundary.
    // for every StartPnt - remove StartPoints coincident with rNextPnt from m_vStartPoints array
    let start_tsp = t.tracer().start_points.clone();
    for id in start_tsp {
        let dist_uv = t.tracer().point(id)?.curve_uv[0].distance_squared(next.curve_uv[0]);
        if dist_uv <= EFF_ZERO {
            t.tracer_mut().start_points.retain(|&p| p != id);
        }
    }
    Ok(true)
}

/// `SmSurfaceTracer::TraceCurve`: traces the curve from `start` until it leaves the
/// domain, closes, reaches a tangent or singular point, cannot step on, runs into
/// one of `curves` or takes 1000 steps, appending the points to the curve points.
/// A curve not started yet (no curve points) begins with `start` evaluated (a
/// failing evaluation ends the trace quietly); an isolated tangent start point not
/// on `curves` is the whole curve; a first step out of the domain reverses the
/// trace direction. The step size is halved until a step is good and doubled while
/// steps stay good and well within tolerance. A trace ending at a singularity
/// after more than one step adds a copy of that point to the start points.
///
/// # Errors
///
/// `InvalidInput` for an unknown `start` or a trace that ends with no curve
/// point; those of the hooks.
pub fn trace_curve_base<'s, T: SurfaceTrace<'s> + ?Sized>(
    t: &mut T,
    k: &Kernel,
    start: TracePointId,
    curves: &[Curve],
) -> Result<()> {
    // init output
    t.tracer_mut().curve_is_closed = false;

    // locals
    let mut count: u64 = 0;
    let uv_pt = t.tracer().point(start)?.curve_uv[0];

    // Note that if we are tracing backward the start point is already on the list.
    if t.tracer().curve_points.is_empty() {
        let mut sp = *t.tracer().point(start)?;
        let r = t.compute_point_values(k, uv_pt, &mut sp, None, None);
        *t.tracer_mut().point_mut(start)? = sp;
        match r {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(()), // Bad start point just go to next one
        }

        // when StartPoint is a TangentPoint
        if sp.point_type == TracePointType::TangentPoint {
            // when StartPoint is not on an already traced curve
            if !t.is_point_on_curve(k, sp.curve_3d[0], curves)? {
                // place one point in m_vCurvePoints and quit
                let p = t.tracer_mut().point_mut(start)?;
                p.curve_parameter = 0.0; // curves start at 0.0
                p.surf_drop_dist = 0.0;
                p.surf_norm_line_dist = 0.0;
                t.tracer_mut().curve_points.push(start);
            }
            // else: Point is already found, skip tracing this point
            return Ok(());
        }

        // Set StartPoint Parameter and deviation to zero, Make StartPoint 1st m_vCurvePoints element
        let p = t.tracer_mut().point_mut(start)?;
        p.curve_parameter = 0.0; // curves start at 0.0
        p.surf_drop_dist = 0.0;
        p.surf_norm_line_dist = 0.0;
        t.tracer_mut().curve_points.push(start);

        // Check to see if first step is out of bounds -- If it is try reversing the direction.
        let delta = sp.curve_uv[0] + EFF_ZERO_SQRT * sp.curve_uv[1];
        if !t
            .tracer()
            .uv_domain
            .contains_point(delta, EFF_ZERO_SQRT / 100_000.0)
        {
            t.tracer_mut().curve_trace_direction = -1.0; // Reverse curve tracing direction
            t.reverse_curve_direction()?;
        }
    }

    // Compute the values in the start point and put it into the list.
    let mut step_size = 0.0;
    let mut done = false;
    let mut small_step_count: u64 = 0;
    let mut point_count: u64 = 0;
    let approx_tol = t.tracer().approx_tol_3d;
    let ang_tol = t.tracer().ang_tol_rad;

    // iterate until sequence of UVStepped TracePnts terminates.
    while !done {
        // count iterations
        point_count += 1;

        // From the last SmTracePnt in m_vCurvePoints
        let tsp = *t
            .tracer()
            .curve_points
            .last()
            .ok_or(SolidError::invalid_input(OP_TRACE, "empty curve"))?;

        // compute the next UVstep size
        step_size = t.compute_step_size(tsp, step_size)?;

        // locals for NextPnt computation
        let mut next_pnt = TracePoint::zeroed();
        next_pnt.point_type = TracePointType::Normal;

        // Given current TracePnt and UVStepSize, compute and classify NextPoint
        let mut np = t.compute_next_point(k, tsp, step_size, &mut next_pnt)?;

        // TraceCurve exit case: TraceCurve hit a surface boundary
        if np.boundary_hit {
            break;
        }

        // When NextTracePnt is bad, iterate until a good NextTracePnt is found.
        let mut increased_step_size = false;
        while !np.found_good_point {
            // cut UVStepSize in half
            step_size /= 2.0;

            // Seek Good NextPnt exit case
            if step_size < EFF_ZERO_SQRT {
                done = true;
                break;
            }

            // remember that UVStepSize was changed
            increased_step_size = true;

            // Given current TracePnt and new UVStepSize, compute and classify NextPoint
            match t.compute_next_point(k, tsp, step_size, &mut next_pnt) {
                Ok(r) => np = r,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {
                    // the outputs initialised before the failure stay set
                    np = NextPoint {
                        found_good_point: false,
                        ..np
                    };
                }
            }
        }

        // TraceCurve exit case: couldn't find a good NextTracePnt
        if done {
            break;
        }

        // If we didn't go through loop to decrease step size then we should try
        // to increase the step size.
        if !np.clipped
            && !increased_step_size
            && np.deviation < approx_tol / 8.0
            && np.angle_rad < ang_tol / 2.0
        {
            // locals
            let mut last_curve_param = next_pnt.curve_parameter;
            let mut last = TracePoint::new();

            // While good, unclipped NextPoints are being found for larger UVStepSizes
            while np.found_good_point && !np.clipped {
                // double UVStepSize
                step_size *= 2.0;

                // remember the last NextPnt that was classified as a good TracePnt
                last = next_pnt;

                // compute next UVStepSize from recently doubled UVStepSize
                step_size = t.compute_step_size(tsp, step_size)?;

                // compute and classify NextTracePnt
                np = t.compute_next_point(k, tsp, step_size, &mut next_pnt)?;

                // largerUVStepSize exit case
                if np.found_good_point && next_pnt.curve_parameter <= last_curve_param + EFF_ZERO {
                    np.found_good_point = false;
                    break;
                }

                // update the NextPnt CurveParam
                last_curve_param = next_pnt.curve_parameter;
            }

            // when while doubling UVStepSize ended by finding a bad NextPoint
            if !np.found_good_point {
                step_size /= 2.0;
                next_pnt = last;
            }
        }

        // If the point is the first point after a singularity or tangency
        // point then we need to test the second point to see if it is on
        // an existing curve.
        if point_count == 1 {
            let mut pt_to_test = next_pnt.curve_3d[0];

            // [B74]
            if np.clipped {
                let half_step = step_size / 2.0;
                let mut mid = TracePoint::new();
                let r = ignored(t.compute_next_point(k, tsp, half_step, &mut mid))?;
                if r.is_some_and(|r| r.found_good_point) {
                    pt_to_test = mid.curve_3d[0];
                }
            }

            // See if TracePnt to test is on any of the already traced curves
            if t.is_point_on_curve(k, pt_to_test, curves)? {
                break;
            }
        }

        // If step size gets too small - quit curve tracing
        if step_size < EFF_ZERO_SQRT {
            small_step_count += 1;
            done = true;
        } else {
            small_step_count = 0; // Reset small step count
        }

        // When StepSize is small - quit without adding point to m_vCurvePoints
        if small_step_count > 10 || done {
            done = true;
        } else {
            // TestPoint looking for end Tracing conditions
            done = t.test_point(k, &mut next_pnt)?;

            // add NextPnt to m_vCurvePoints array
            let id = t.tracer_mut().new_point(next_pnt);
            t.tracer_mut().add_point_to_curve(id);
        }

        // count the number of TraceSteps
        count += 1;

        // When TraceStep count gets too large - stop TraceCurve
        if count > 1000 {
            done = true;
        }
    }

    // when last TracePnt on TraceCurve is on a singularity - Add TracePnt to
    // m_vStartPoints array so the Curve on the other side can also be traced.
    let last = *t
        .tracer()
        .curve_points
        .last()
        .ok_or(SolidError::invalid_input(OP_TRACE, "empty curve"))?;
    let last_pnt = *t.tracer().point(last)?;
    if last_pnt.point_type == TracePointType::Singularity && count > 1 {
        let id2 = t.tracer_mut().new_point(last_pnt);
        t.tracer_mut().add_start_point(id2)?;
    }
    Ok(())
}

/// `sm_FlushCrv`: a cubic of `dim` through the Bezier control points `cntrl_poly`
/// with the unique knots `knots` (full multiplicity), appended to `curves`;
/// nothing for fewer than two knots.
fn flush_crv(
    dim: u64,
    cntrl_poly: &[Point3],
    knots: &[f64],
    curves: &mut Vec<Curve>,
) -> Result<()> {
    // no work - not enough points
    if knots.len() < 2 {
        return Ok(());
    }

    // Set up knot vector: cubic, full multiplicities.
    let num_mults = knots.len();
    let mut knot_mult = vec![3_usize; num_mults];
    if let Some(m) = knot_mult.first_mut() {
        *m = 4;
    }
    if let Some(m) = knot_mult.last_mut() {
        *m = 4;
    }

    // Create the curve.
    let new_bsp = BSplineCurve::create_canonical(&CanonicalData {
        dim,
        degree: 3,
        control_points: cntrl_poly,
        form: BSplineCurveForm::Unspecified,
        knot_multiplicities: &knot_mult,
        knots,
        knot_type: KnotType::Unspecified,
        weights: None,
        trim: None,
    })?;

    // set output
    curves.push(Curve::from(new_bsp));
    Ok(())
}

/// `SmSurface::EvaluateNormal(uv, FALSE, FALSE, ..)` (virtual).
fn evaluate_normal(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Vec3> {
    surface.evaluate_normal(k, uv, Side::Right, Side::Right)
}

/// `SmSurfaceTracer::FlushCurve`: empties the curve points and appends the curve
/// they describe to `out`: a degenerate line pair for a single tangent point;
/// nothing for fewer than two points or two points closer than `approx_tol_3d`;
/// else the piecewise cubic Bezier interpolation of the points (3D and UV) with
/// the largest drop distance and deviation of the points. With `opt_curve` (the
/// curve the trace follows) the drop distance also covers the middle and end of
/// every span.
///
/// # Errors
///
/// `InvalidInput` for unknown ids; those of the curve constructions;
/// [`SolidError::Unported`] from the surface evaluations of `opt_curve`'s spans.
pub fn flush_curve(
    k: &Kernel,
    t: &mut SurfaceTracer<'_>,
    out: &mut TraceOutput,
    opt_curve: Option<&Curve>,
) -> Result<()> {
    // Grab the m_vCurvePoints list into an array and clear out the list.
    let ids = core::mem::take(&mut t.curve_points);
    let pnts: Vec<TracePoint> = ids
        .iter()
        .map(|&id| t.point(id).copied())
        .collect::<Result<_>>()?;

    // For single tangent point - create a degenerate curve.
    let num_pts = pnts.len();
    if let [curr] = pnts.as_slice() {
        if curr.point_type == TracePointType::TangentPoint {
            // let control polygon = two coincident points, Param domain = [0 1], knot vector = [2 2]
            let knots = [0.0, 1.0];
            let knot_mult = [2_usize, 2];
            let degenerate = |dim: u64, p: Point3| {
                BSplineCurve::create_canonical(&CanonicalData {
                    dim,
                    degree: 1,
                    control_points: &[p, p],
                    form: BSplineCurveForm::Unspecified,
                    knot_multiplicities: &knot_mult,
                    knots: &knots,
                    knot_type: KnotType::Unspecified,
                    weights: None,
                    trim: None,
                })
            };

            // construct the degenerate BSplineCurve
            let bsp_3d = degenerate(3, curr.curve_3d[0])?;
            out.curves_3d.push(Curve::from(bsp_3d));

            // Now build UVTrimCurve - update Control polygon - reuse same parameters and knots
            let bsp_uv = degenerate(2, curr.uv_point_3d())?;

            // set outputs
            out.uv_curves.push(Curve::from(bsp_uv));
            out.max_drop_dists.push(curr.surf_drop_dist);
            out.deviations.push(curr.surf_norm_line_dist);
            return Ok(());
        }
    }

    // exit case - less than 2 m_vCurvePoints - can't build a curve
    let (Some(first), true) = (pnts.first(), num_pts >= 2) else {
        return Ok(());
    };

    // exit case - 2 m_vCurvePoints closer than m_dThisApproxTol3d apart
    if let [a, b] = pnts.as_slice() {
        if a.curve_3d[0].distance(b.curve_3d[0]) < t.approx_tol_3d {
            return Ok(());
        }
    }

    // locals
    let mut knots: Vec<f64> = Vec::new();
    let mut ctrl_3d: Vec<Point3> = Vec::new();
    let mut ctrl_uv: Vec<Point3> = Vec::new();
    let mut max_drop_dist = first.surf_drop_dist;
    let mut max_deviation = first.surf_norm_line_dist;

    // init 3d BSplineCurve and 2d UVTrimCurve with 1st Knot and Control point
    knots.push(first.curve_parameter);
    ctrl_3d.push(first.curve_3d[0]);
    ctrl_uv.push(first.uv_point_3d());

    // upstream's function-scope locals: a failed (ignored) evaluation keeps the
    // previous value
    let mut crv_pt = Vec3::ZERO;
    let mut srf_pt = Vec3::ZERO;
    let mut norm = Vec3::ZERO;

    // for every remaining m_vCurvePoint - build piecewise cubic bezier interpolation
    for pair in pnts.windows(2) {
        let [last, curr] = pair else { continue };
        let scale = curr.curve_parameter - last.curve_parameter;

        // save max deviation
        if curr.surf_norm_line_dist > max_deviation {
            max_deviation = curr.surf_norm_line_dist;
        }

        // Add the next knot
        knots.push(curr.curve_parameter);

        {
            // 3d cubic bezier control points
            let p1 = last.curve_3d[0];
            let p4 = curr.curve_3d[0];
            let d1 = last.curve_3d[1] * scale;
            let d2 = curr.curve_3d[1] * scale;
            let p2 = p1 + d1 / 3.0;
            let p3 = p4 - d2 / 3.0;
            ctrl_3d.push(p2);
            ctrl_3d.push(p3);
            ctrl_3d.push(p4);
        }

        {
            // 2d UVTrimCurve Control points
            let p1 = last.curve_uv[0];
            let p4 = curr.curve_uv[0];
            let d1 = last.curve_uv[1] * scale;
            let d2 = curr.curve_uv[1] * scale;
            let p2 = p1 + d1 / 3.0;
            let p3 = p4 - d2 / 3.0;
            ctrl_uv.push(Vec3::from(p2));
            ctrl_uv.push(Vec3::from(p3));
            ctrl_uv.push(Vec3::from(p4));

            // Calc drop dists at Middle and End of each new Bezier segment.
            if let Some(opt) = opt_curve {
                // Mid Segment CrvPoint (status ignored upstream: the point stays)
                let mid_t = (last.curve_parameter + curr.curve_parameter) / 2.0;
                if let Some(p) = ignored(opt.evaluate_point(k, mid_t))? {
                    crv_pt = p;
                }

                // Mid Segment SrfPoint (SurfUV point is the midPoint of the Bezier UVTrimCurve)
                let uv = (p1 + 3.0 * p2 + 3.0 * p3 + p4) / 8.0;
                if let Some(p) = ignored(t.surface.evaluate_point(k, uv))? {
                    srf_pt = p;
                }

                // When Dist3d(CrvPnt, SrfPnt) is greater than MaxDropDist
                let dist = crv_pt.distance(srf_pt);
                if dist > max_drop_dist {
                    // the component of (CrvPnt - SrfPnt) in the surfNorm direction
                    if let Some(n) = ignored(evaluate_normal(k, t.surface, uv))? {
                        norm = n;
                    }
                    let dist = fabs((crv_pt - srf_pt).dot(norm));
                    if dist > max_drop_dist {
                        max_drop_dist = dist;
                    }
                }

                // End Segment CrvPnt
                if let Some(p) = ignored(opt.evaluate_point(k, curr.curve_parameter))? {
                    crv_pt = p;
                }

                // End Segment Drop distance
                srf_pt = curr.surface[0][0];
                norm = curr.surface[2][2];
                let dist = fabs((crv_pt - srf_pt).dot(norm));
                if dist > max_drop_dist {
                    max_drop_dist = dist;
                }
            }
        }
    }

    // set output
    flush_crv(3, &ctrl_3d, &knots, &mut out.curves_3d)?;
    flush_crv(2, &ctrl_uv, &knots, &mut out.uv_curves)?;
    out.max_drop_dists.push(max_drop_dist);
    out.deviations.push(max_deviation);
    Ok(())
}

/// `SmSurface::DropCurve(ctx, domain, curve, interval, tol, .., TRUE, NULL, FALSE)`:
/// the plane override; other kinds run `SmSurface::DropCurve`. Returns the largest
/// drop distance, the deviation and the UV curves.
fn surface_drop_curve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    approx_tol: f64,
) -> Result<(f64, f64, Vec<Curve>)> {
    let r = surface.drop_curve::<crate::surface::surface::p4::NoObject>(
        k, uv_domain, curve, interval, approx_tol, true, None, false,
    )?;
    Ok((r.max_drop_to_surf, r.max_approx_dev, r.uv_curves))
}

/// `SmSurfaceTracer::FindBoundaryStartPoints`: for each of the four boundary iso
/// curves of the domain (low u, low v, high u, high v; degenerate ones skipped),
/// a whole boundary curve that is a solution ([`SurfaceTrace::test_iso_curve`])
/// and not on a curve of `out` yet is added to `out` with its drop onto the
/// surface; then the boundary solutions
/// ([`SurfaceTrace::find_solutions_on_boundary_curve`], both ends of ranges) that
/// lie on the traced curve become start points.
///
/// # Errors
///
/// Those of the iso curves, the hooks, the drop (`SM_ERR` when it gives no UV
/// curve) and the curve evaluations; `InvalidInput` for a missing iso curve.
pub fn find_boundary_start_points_base<'s, T, O>(
    t: &mut T,
    k: &Kernel,
    cache: &mut SurfaceCache<O>,
    out: &mut TraceOutput,
) -> Result<()>
where
    T: SurfaceTrace<'s> + ?Sized,
    O: SolutionObject,
{
    let domain = t.tracer().uv_domain;
    let approx_tol = t.tracer().approx_tol_3d;

    // Get the four natural boundary curves of the surface.
    let iso = cache.iso_boundary_curves(k, &domain, approx_tol)?;

    // Loop over four Surface Natural Bounding edges: lSide = low/high, lCoord = u/v
    for side in 0..=1_usize {
        let norm_param = side as f64;

        // for U and V directions
        for coord in 0..=1_usize {
            let surf_param = if coord == 0 {
                SurfParamType::U
            } else {
                SurfParamType::V
            };
            let new_bsc = iso
                .curves
                .get(side * 2 + coord)
                .ok_or(SolidError::invalid_input(OP_BOUNDARY, "iso curve index"))?;
            let bsc_ivl = Extent1d::from_min_max(
                domain.min().component(1 - coord)?,
                domain.max().component(1 - coord)?,
            );

            // skip degen IsoCurves (IsDegenerate's defaults; a failure is not degenerate)
            if ignored(new_bsc.is_degenerate(k, EFF_ZERO, None))? == Some(true) {
                continue;
            }

            // Check whether the entire boundary curve is a solution.
            let param_val = domain.evaluate(norm_param, norm_param).component(coord)?;
            let curve_is_silhouette = t.test_iso_curve(k, surf_param, param_val)?;

            // when entire BoundaryCurve is a solution
            if curve_is_silhouette {
                // get near mid-Pnt
                let test_param = bsc_ivl.evaluate(0.45678);
                let test_pnt = new_bsc.evaluate_point(k, test_param)?;

                // If we already have this curve then skip it.
                if t.is_point_on_curve(k, test_pnt, &out.curves_3d)? {
                    continue;
                }

                // Add a copy of this solution curve to r3DCurves.
                // (`new SmBSplineCurve(*pNewBSC)`: a plain B-spline copy)
                let result = crate::surface::surface_cache::p1::copy_as_bspline(new_bsc)?;
                let result_ivl = result.natural_interval();
                out.curves_3d.push(result.clone());

                // we know this is a boundary iso-curve:
                let (dist_to_surf, deviation, mut uv_curves) = surface_drop_curve(
                    k,
                    t.tracer().surface,
                    &domain,
                    &result,
                    &result_ivl,
                    approx_tol,
                )?;

                // When Curve dropped along a seam - save the current target side solution
                if uv_curves.len() == 2 {
                    out.uv_curves.push(uv_curves.swap_remove(side));
                } else if uv_curves.len() == 1 {
                    out.uv_curves.extend(uv_curves);
                } else {
                    return Err(SolidError::sm_err(OP_BOUNDARY));
                }

                // set other outputs
                out.max_drop_dists.push(dist_to_surf);
                out.deviations.push(deviation);
            }

            // Note, we have to do the next check even if the whole curve
            // is a silhouette: picture a torus edge-on.  [B100]

            // Find curve start points along the curve.
            let solutions =
                t.find_solutions_on_boundary_curve(k, new_bsc, &bsc_ivl, side, surf_param)?;

            // for every solution
            for sol in solutions.as_slice() {
                let mut uv = Vec2::ZERO;
                *uv.component_mut(coord)? =
                    domain.evaluate(norm_param, norm_param).component(coord)?;
                *uv.component_mut(1 - coord)? = sol.start.param(0)?;

                // when solution lies on solution curve - save it
                if t.does_point_lie_on_curve(k, &mut uv, true)? {
                    add_boundary_start_point(t.tracer_mut(), uv)?;
                }

                // for range solutions
                if sol.solution_type == SolutionType::RangeOfValues {
                    *uv.component_mut(coord)? =
                        domain.evaluate(norm_param, norm_param).component(coord)?;
                    *uv.component_mut(1 - coord)? = sol.end.param(0)?;

                    // when solution lies on solution curve - save it
                    if t.does_point_lie_on_curve(k, &mut uv, true)? {
                        add_boundary_start_point(t.tracer_mut(), uv)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// A new normal trace point at `uv` offered to the start points.
fn add_boundary_start_point(t: &mut SurfaceTracer<'_>, uv: Point2) -> Result<()> {
    let mut p = TracePoint::zeroed();
    p.point_type = TracePointType::Normal;
    p.curve_uv[0] = uv;
    let id = t.new_point(p);
    t.add_start_point(id)
}

/// `SmSurfaceTracer::TestIsoCurve`: whether the iso curve `param = value` of the
/// domain is a solution curve: ten evenly spaced points along it all lie on the
/// curve ([`SurfaceTrace::does_point_lie_on_curve`], unrefined).
///
/// # Errors
///
/// Those of [`SurfaceTrace::does_point_lie_on_curve`].
pub fn test_iso_curve_base<'s, T: SurfaceTrace<'s> + ?Sized>(
    t: &mut T,
    k: &Kernel,
    param: SurfParamType,
    value: f64,
) -> Result<bool> {
    // get IsoParamCurve min and max UVPnts
    let domain = t.tracer().uv_domain;
    let mut uv1 = domain.min();
    let mut uv2 = domain.max();

    if param == SurfParamType::U {
        uv1 = Vec2::new(value, uv1.y);
        uv2 = Vec2::new(value, uv2.y);
    } else {
        uv1 = Vec2::new(uv1.x, value);
        uv2 = Vec2::new(uv2.x, value);
    }
    let uv_iso = Extent2d::from_points(uv1, uv2);

    // for every sample point
    for i in 0..NUM_ISO_TEST_POINTS {
        let mut dt = i as f64 / (NUM_ISO_TEST_POINTS as f64 - 1.0);
        if dt > 1.0 {
            dt = 1.0;
        }
        let mut uv = uv_iso.evaluate(dt, dt);

        // Check whether this uv point lies on the solution curve being traced.
        if !t.does_point_lie_on_curve(k, &mut uv, false)? {
            return Ok(false);
        }
    }

    // Each sample point passed, so it must be a solution curve.
    Ok(true)
}

/// The tracer as the global solver of [`find_interior_curves_base`]: the solver
/// state plus the tracer's hooks.
struct InteriorSolver<'a, 't, T: ?Sized, O> {
    solver: GlobalSolver<'t, O>,
    cache: &'t SurfaceCache<O>,
    tracer: &'a mut T,
}

impl<'s, 't, T, O> GlobalSolve<'t, O> for InteriorSolver<'_, 't, T, O>
where
    T: SurfaceTrace<'s> + ?Sized,
    O: SolutionObject + 't,
{
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.solver
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.solver
    }

    fn branch_may_contain_answers(&mut self, branch: &Branch) -> Result<bool> {
        self.tracer
            .branch_may_contain_answers(&self.solver, self.cache, branch)
    }

    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        self.tracer.local_solve(&mut self.solver, branch)
    }
}

/// `SmSurfaceTracer::FindInteriorCurves`: finds the leaves of the surface tree
/// that may hold the curve (`SolveIt(SM_SO_FIND, SM_SR_NODES)` with the tracer's
/// hooks) and traces, from the refined middle of every leaf meeting the domain
/// that is not near a curve of `out` yet, the curve forward and (unless it closed
/// or starts at a singularity) backward, adding it to `out`.
///
/// # Errors
///
/// Those of building the tree (`SmSurfaceCache::GetTree`, [`SurfaceCache::get_tree`]),
/// of the global solve, the curve caches and the curve queries. Failures of a single start
/// point skip it, as upstream.
pub fn find_interior_curves_base<'s, T, O>(
    t: &mut T,
    k: &Kernel,
    cache: &mut SurfaceCache<O>,
    out: &mut TraceOutput,
) -> Result<()>
where
    T: SurfaceTrace<'s> + ?Sized,
    O: SolutionObject,
{
    // the constructor's m_pTree = pCache->GetTree() (built if needed)
    cache.get_tree(k)?;
    let cache: &SurfaceCache<O> = cache;
    let tree: &Tree<O> = cache.tree().ok_or(SolidError::invalid_input(
        "intersect::surface_tracer::find_interior_curves",
        "surface cache without tree",
    ))?;

    // Find pairs of tree nodes containing possible solutions from which StartPoints can be found
    let mut solutions = SolutionArray::new();
    {
        let mut solver = GlobalSolver::with_tree(tree, 2);
        solver.set_solver_operation(SolverOperation::Intersect);
        solver.set_solution_requested(SolutionRequested::Nodes);
        solver.set_best_answer_so_far_sq(0.0);
        solver.set_at_distance(0.0);
        solver.set_3d_tolerance(0.0);
        let mut gs = InteriorSolver {
            solver,
            cache,
            tracer: t,
        };
        gs.solve_it(
            &SolveRequest {
                operation: SolverOperation::Find,
                solution_requested: SolutionRequested::Nodes,
                tolerance_3d: 0.0,
                best_answer_so_far_sq: 0.0,
                opt_vectors: &[],
            },
            &mut solutions,
        )?;
    }

    let domain = t.tracer().uv_domain;
    let approx_tol = t.tracer().approx_tol_3d;

    // Now for each node pair do a test to see if produces a unique start point
    for sol in solutions.as_slice() {
        let sol: &Solution<O> = sol;

        // Surface TreeNode locals for this solution
        let Some(node_id) = sol.nodes[0] else {
            return Err(SolidError::invalid_input(
                "intersect::surface_tracer::find_interior_curves",
                "solution without node",
            ));
        };
        let node = tree.node(node_id)?;

        // skip unexpected cases in which TreeNode has no BezPatch
        if !matches!(
            node.data,
            NodeData::Surface(SurfaceAux { slot: Some(_), .. })
        ) {
            continue;
        }
        let uv_domain1 = *cache.patch(node_id)?.aux.uv_domain();

        // skip TreeNodes disjoint from trace surface's specified UVSubDomain
        if uv_domain1.are_disjoint(&domain, 0.0) {
            continue;
        }

        // First let's do a fast test to see if the intersection of
        // these two nodes is close to any of the new 3D curves that
        // we have found.
        let mut is_near_curve = false;
        let mut int_bbox = node.bbox;
        int_bbox.expand_absolute(approx_tol);

        // for every 3D curve
        for curve in &out.curves_3d {
            // get curve->Cache (skip unexpected curves without curve caches)
            let cc = match make_curve_cache::<()>(k, curve, None) {
                Ok(cc) => cc,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => continue,
            };
            let Some(ctree) = cc.tree() else { continue };

            // remember when curve->Cache BBox intersects Surface->Node BBox
            if ctree.intersects_box(&int_bbox)? {
                is_near_curve = true;
                break;
            }
        }

        // skip cases where curve->Cache BBox is near an existing Curve
        if is_near_curve {
            continue;
        }

        let middle = uv_domain1.evaluate(0.5, 0.5);

        // Refine sMiddle to see if we can find a point on the curve
        let mut pnt1 = TracePoint::new();
        let mut pnt2 = TracePoint::new();
        pnt1.curve_3d[1] = Vec3::ZERO;
        let Some(Some(found_uv)) = ignored(t.local_point_solve(k, &pnt1, middle, &mut pnt2))?
        else {
            continue; // No point found continue to next node
        };

        // Now test start point to see if it drops to any of the existing 3D curves.
        let surf_pnt = match t.tracer().surface.evaluate_point(k, found_uv) {
            Ok(p) => p,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => continue,
        };
        if t.is_point_on_curve(k, surf_pnt, &out.curves_3d)? {
            continue;
        }

        // If we made it this far then we have a point which should serve as
        // a good start point.  Load it up and trace it.
        let mut start = TracePoint::zeroed();
        start.point_type = TracePointType::Normal;
        match t.compute_point_values(k, found_uv, &mut start, None, None) {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => continue,
        }
        let start_id = t.tracer_mut().new_point(start);

        t.tracer_mut().start_point = Some(start_id);
        t.tracer_mut().curve_trace_direction = 1.0; // Forward
        match t.trace_curve(k, start_id, &out.curves_3d) {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => continue,
        }

        // Trace the other direction if appropriate.
        let start_type = t.tracer().point(start_id)?.point_type;
        if !t.tracer().curve_is_closed && start_type != TracePointType::Singularity {
            match t.reverse_curve_direction() {
                Ok(()) => {}
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {
                    // Note, if this failed, we want to call FlushCurve()
                    ignore_status(flush_curve(k, t.tracer_mut(), out, None))?;
                    continue;
                }
            }

            t.tracer_mut().curve_trace_direction = -1.0;
            let back_start = *t
                .tracer()
                .curve_points
                .last()
                .ok_or(SolidError::null_pointer(OP_REVERSE))?;
            ignore_status(t.trace_curve(k, back_start, &out.curves_3d))?;
            ignore_status(t.reverse_curve_direction())?;
            t.tracer_mut().curve_trace_direction = 1.0;
        }

        ignore_status(flush_curve(k, t.tracer_mut(), out, None))?;
        t.tracer_mut().start_point = None;
    }
    Ok(())
}

/// `SmSurfaceTracer::DoTrace`: stores the optional tolerances, then (with the
/// cache's point test off and boundary curve processing on) traces from every
/// boundary start point ([`SurfaceTrace::find_boundary_start_points`],
/// [`SurfaceTrace::trace_curve`], reversed when traced backwards, [`flush_curve`])
/// and then the interior curves ([`SurfaceTrace::find_interior_curves`]).
/// `cache` is the surface's cache (`smsurf_GetSurfaceCache`).
///
/// # Errors
///
/// Those of the start point search, the reversal, the flush and the interior
/// search (a failing trace from one start point is skipped, as upstream).
pub fn do_trace_base<'s, T, O>(
    t: &mut T,
    k: &Kernel,
    cache: &mut SurfaceCache<O>,
    approx_tol_3d: Option<f64>,
    ang_tol_rad: Option<f64>,
) -> Result<TraceOutput>
where
    T: SurfaceTrace<'s> + ?Sized,
    O: SolutionObject,
{
    // store optional Tol values
    if let Some(tol) = approx_tol_3d {
        t.tracer_mut().approx_tol_3d = tol;
    }
    if let Some(ang) = ang_tol_rad {
        t.tracer_mut().ang_tol_rad = ang;
    }

    // Check to make sure internal surface continuities are better then C0
    // (upstream only signals SM_ERR_INVALID_INPUT)
    let domain = t.tracer().uv_domain;
    let _below_g1 = cache.surface_continuity(&domain)? as i32 <= ContinuityType::C0 as i32;

    // Turn off point testing and turn on boundary curve processing.
    let saved_point_test = cache.point_test_enabled;
    let saved_boundary = cache.process_boundary_curves;
    cache.point_test_enabled = false;
    cache.process_boundary_curves = true;
    let r = do_trace_body(t, k, cache);
    cache.point_test_enabled = saved_point_test;
    cache.process_boundary_curves = saved_boundary;
    r
}

/// The part of [`do_trace_base`] run with the cache flags changed.
fn do_trace_body<'s, T, O>(
    t: &mut T,
    k: &Kernel,
    cache: &mut SurfaceCache<O>,
) -> Result<TraceOutput>
where
    T: SurfaceTrace<'s> + ?Sized,
    O: SolutionObject,
{
    let mut out = TraceOutput::default();

    // The first step in tracing is to find some start points on the boundary
    t.find_boundary_start_points(k, cache, &mut out)?;

    // While m_vStartPoints has Pnts - trace 1 curve from each StartPnt
    while let Some(tsp) = t.tracer_mut().start_points.pop() {
        // Init trace in tangent dir
        t.tracer_mut().curve_trace_direction = 1.0;

        // trace curve: start in pos dir, switch to neg dir if needed
        match t.trace_curve(k, tsp, &out.curves_3d) {
            Ok(()) => {
                // reverse curves traced in neg direction
                if t.tracer().curve_trace_direction == -1.0 {
                    t.reverse_curve_direction()?;
                }

                // output curve
                flush_curve(k, t.tracer_mut(), &mut out, None)?;
            }
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => {}
        }
    }

    // Now that we have found all boundary intersections, look for non boundary intersections.
    t.find_interior_curves(k, cache, &mut out)?;
    Ok(out)
}

/// `smsurf_GetSurfaceCache` for the tracer's surface ([`surface_cache`], built fresh)
/// and the tracer constructor's `pSC->GetTree()` (whose failure the constructor
/// ignores): the cache callers pass to [`SurfaceTrace::do_trace`]. The tree is built
/// before the boundary search reads the cache's continuity arrays, also during a
/// Boolean operation.
///
/// # Errors
///
/// Those of the cache builds (the continuities of the tree build:
/// `SmSurface::CalculateContinuities`); [`SolidError::Unported`] from the tree build.
pub fn tracer_surface_cache<O: Copy + PartialEq>(
    k: &Kernel,
    t: &SurfaceTracer<'_>,
) -> Result<SurfaceCache<O>> {
    let mut cache = surface_cache(k, t.surface, None)?;
    match cache.get_tree(k) {
        Err(e) if e.is_unported() => Err(e),
        _ => Ok(cache),
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
            &Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0),
        ))
    }

    #[test]
    fn constructor_tolerances() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let t = SurfaceTracer::new(k, &s, &d).unwrap();
        let diag = (200.0_f64).sqrt();
        assert!((t.approx_tol_3d() - diag / 1000.0).abs() < 1e-15);
        assert!((t.ang_tol_rad() - 20.0 * PI / 180.0).abs() < 1e-15);
        assert!(t.curve_points.is_empty() && t.start_points.is_empty());
        assert_eq!(t.compute_step_size(0.0), 0.1);
        assert_eq!(t.compute_step_size(0.25), 0.25);
    }

    #[test]
    fn start_points_are_unique() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let mut t = SurfaceTracer::new(k, &s, &d).unwrap();
        let mut p = TracePoint::new();
        p.point_type = TracePointType::Normal;
        p.curve_uv[0] = Vec2::new(1.0, 2.0);
        let a = t.new_point(p);
        let b = t.new_point(p);
        t.add_start_point(a).unwrap();
        t.add_start_point(b).unwrap();
        assert_eq!(t.start_points, vec![a]);

        // singular points with opposite tangents are different
        let mut q = p;
        q.point_type = TracePointType::Singularity;
        q.curve_3d[1] = Vec3::new(1.0, 0.0, 0.0);
        let c = t.new_point(q);
        q.curve_3d[1] = Vec3::new(-1.0, 0.0, 0.0);
        let e = t.new_point(q);
        t.add_start_point(c).unwrap();
        t.add_start_point(e).unwrap();
        assert_eq!(t.start_points, vec![a, c, e]);
    }

    #[test]
    fn reverse_reparameterises() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let mut t = SurfaceTracer::new(k, &s, &d).unwrap();
        assert!(t.reverse_curve_direction().is_err());
        for (i, param) in [0.0, 1.0, 3.0].into_iter().enumerate() {
            let mut p = TracePoint::new();
            p.curve_parameter = param;
            p.curve_uv[1] = Vec2::new(1.0, 0.0);
            p.user_long = i as u64;
            let id = t.new_point(p);
            t.add_point_to_curve(id);
        }
        t.reverse_curve_direction().unwrap();
        let params: Vec<f64> = t
            .curve_points
            .iter()
            .map(|&id| t.point(id).unwrap().curve_parameter)
            .collect();
        assert_eq!(params, vec![0.0, 2.0, 3.0]);
        let first = t.point(t.curve_points[0]).unwrap();
        assert_eq!(first.user_long, 2);
        assert_eq!(first.curve_uv[1], Vec2::new(-1.0, 0.0));
    }

    #[test]
    fn flush_single_tangent_point_is_degenerate() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let mut t = SurfaceTracer::new(k, &s, &d).unwrap();
        let mut p = TracePoint::new();
        p.point_type = TracePointType::TangentPoint;
        p.curve_3d[0] = Vec3::new(1.0, 2.0, 0.0);
        p.curve_uv[0] = Vec2::new(1.0, 2.0);
        p.surf_drop_dist = 0.5;
        p.surf_norm_line_dist = 0.25;
        let id = t.new_point(p);
        t.add_point_to_curve(id);
        let mut out = TraceOutput::default();
        flush_curve(k, &mut t, &mut out, None).unwrap();
        assert!(t.curve_points.is_empty());
        assert_eq!(out.curves_3d.len(), 1);
        assert_eq!(out.uv_curves.len(), 1);
        assert_eq!(out.max_drop_dists, vec![0.5]);
        assert_eq!(out.deviations, vec![0.25]);
    }

    #[test]
    fn flush_two_close_points_makes_nothing() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let mut t = SurfaceTracer::new(k, &s, &d).unwrap();
        for x in [0.0, 1e-6] {
            let mut p = TracePoint::new();
            p.curve_3d[0] = Vec3::new(x, 0.0, 0.0);
            let id = t.new_point(p);
            t.add_point_to_curve(id);
        }
        let mut out = TraceOutput::default();
        flush_curve(k, &mut t, &mut out, None).unwrap();
        assert!(out.curves_3d.is_empty() && out.max_drop_dists.is_empty());
    }

    #[test]
    fn base_hooks_fail() {
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let k = Kernel::default();
        let mut t = SurfaceTracer::new(&k, &s, &d).unwrap();
        let mut p = TracePoint::new();
        assert!(t
            .compute_point_values(&k, Vec2::ZERO, &mut p, None, None)
            .is_err());
        let mut uv = Vec2::ZERO;
        assert!(!t.does_point_lie_on_curve(&k, &mut uv, true).unwrap());
        assert!(!t.test_iso_curve(&k, SurfParamType::U, 0.0).unwrap());
        // (the cache builds: its continuities are surface::surface::p6)
        assert!(tracer_surface_cache::<()>(&k, &t).is_ok());
    }

    #[test]
    fn test_point_on_boundary_removes_start_points() {
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let k = Kernel::default();
        let mut t = SurfaceTracer::new(&k, &s, &d).unwrap();
        let mut p = TracePoint::new();
        p.curve_uv[0] = Vec2::new(10.0, 5.0);
        let a = t.new_point(p);
        t.add_start_point(a).unwrap();
        let mut q = p;
        assert!(t.test_point(&k, &mut q).unwrap());
        assert!(t.start_points.is_empty());
        q.curve_uv[0] = Vec2::new(5.0, 5.0);
        assert!(!t.test_point(&k, &mut q).unwrap());
        q.point_type = TracePointType::TangentPoint;
        assert!(t.test_point(&k, &mut q).unwrap());
    }
}
