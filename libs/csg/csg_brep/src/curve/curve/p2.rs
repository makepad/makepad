//! `SmCurve`, part 2: the curve object ([`Curve`]: a concrete curve kind plus the
//! owner pointer) with its type queries, equality and owner links, the virtual
//! interface of `SmCurve` dispatched over the curve kinds (evaluation, knots, ends,
//! degree and control point counts, parameterisation edits, trimming, copying,
//! analytic conversion, reading from a database stream), the base-class numerics
//! (sampled length, STEP evaluation, Frenet frames, continuity between two curves,
//! chord step-off, splitting, the surface gap sampler), the file-static helpers of
//! the Hermite approximation and tessellation drivers, and the box tests of the
//! curve global solvers.
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`Curve`], [`CurveKind`]                          | `SmCurve` (and its derived classes)              |
//! | `Drop` (implicit), `Clone`                        | `~SmCurve`, copy constructor (owner reset)       |
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`Curve::type_code`], [`Curve::type_string`], [`Curve::class_string`] | `GetType`, `GetClassType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`is_kind_of`], [`Curve::is_kind_of`]             | `SmCurve::IsKindOf`                              |
//! | [`Curve::approx_eq`]                              | `operator==`                                     |
//! | [`Curve::owner`], [`Curve::set_owner`], [`OwnerChange`] | `GetOwner`, `SetOwner`                     |
//! | [`Curve::notify_key`] | (none: the object's kernel-issued identity in notifications; CO-PORTED by `topology.brep_p3`) |
//! | [`Curve::edge`]                                   | `GetEdge`                                        |
//! | [`notify`]                                        | `SmCurve::Notify`                                |
//! | [`CurveEvaluate`]                                 | the evaluation virtuals the base numerics call   |
//! | [`Curve::calculate_continuities`]                 | `CalculateContinuities`                          |
//! | [`Curve::convert_to_2d`]                          | `ConvertTo2D`                                    |
//! | [`Curve::create_analytic_curve`]                  | `CreateAnalyticCurve`                            |
//! | [`Curve::copy`]                                   | `Copy`                                           |
//! | [`Curve::drop_point_fast`]                        | `DropPointFast`                                  |
//! | [`Curve::evaluate`], [`Curve::evaluate_point`]    | `Evaluate`, `EvaluatePoint`                      |
//! | [`Curve::degree`], [`Curve::dim`], [`Curve::natural_interval`] | `GetDegree`, `GetDim`, `GetNaturalInterval` |
//! | [`Curve::number_natural_knots`], [`Curve::number_control_points`] | `GetNumberNaturalKnots`, `GetNumberControlPoints` |
//! | [`Curve::is_analytic`], [`Curve::is_bounded`], [`Curve::is_rational`] | `IsAnalytic`, `IsBounded`, `IsRational` |
//! | [`Curve::has_repeated_end_control_points`]        | `HasRepeatedEndControlPoints`                    |
//! | [`Curve::reverse_parameterization`], [`Curve::edit_parameterization`] | `ReverseParameterization`, `EditParameterization` |
//! | [`Curve::step_inversion`]                         | `STEPInversion`                                  |
//! | [`Curve::transform`], [`Curve::trim`]             | `Transform`, `Trim`                              |
//! | [`Curve::knots`], [`Curve::ends`], [`base_ends`]  | `GetKnots`, `GetEnds`                            |
//! | [`Curve::intersect_with_line`], [`LineIntersection`] | `IntersectWithLine`                           |
//! | [`approximate_length`], [`Curve::approximate_length`] | `ApproximateLength`                          |
//! | [`Curve::convert_t_from_nurbs_to_step`], [`Curve::convert_t_from_step_to_nurbs`] | `ConvertTFromNURBSToSTEP`, `ConvertTFromSTEPToNURBS` |
//! | [`Curve::create_by_split_curve`]                  | `CreateBySplitCurve`                             |
//! | [`Curve::surface_max_distance_between`], [`SurfaceGap`] | `SurfaceMaxDistanceBetween`              |
//! | [`evaluate_step`]                                 | `EvaluateSTEP`                                   |
//! | [`euclidian_step_off`], [`StepOff`]               | `EuclidianStepOff`                               |
//! | [`evaluate_geometric`]                            | `EvaluateGeometric`                              |
//! | [`evaluate_continuity`], [`CurveContinuity`]      | `EvaluateContinuity`                             |
//! | [`Curve::read_from_db`]                           | `SmCurve::ReadFromDB`                            |
//! | [`convert_to_parameter_delta`]                    | `sm_ConvertToParameterDelta` (static)            |
//! | [`push_point`], [`pop_stack`], [`stack_look`]     | `sm_PushPoint`, `sm_PopStack`, `sm_StackLook` (static) |
//! | [`flush_crv`]                                     | `sm_FlushCrv` (static)                           |
//! | [`add_to_curve`]                                  | `sm_AddToCurve` (static)                         |
//! | [`snap_sol_to_specified_endpoint`]                | `sm_SnapSolToSpecifiedEndpoint` (static)         |
//! | [`unitize_tangent`]                               | `sm_UnitizeTangent` (static)                     |
//! | [`proj_signed_angle`]                             | `sm_GetProjSignedAngle` (static)                 |
//! | [`output`]                                        | `sm_Output` (static)                             |
//! | [`test_step`], [`TessStepResult`]                 | `sm_TestStep`, `SmTessStepTestResultType`        |
//! | [`find_test_params`]                              | `sm_FindTestParams` (static)                     |
//! | [`CcGlobalSolver`]                                | `SmCCGlobalSolver` (constructor, `BranchMayContainAnswers`; `LocalSolve` is unit `curve.curve_p9`) |
//! | [`CpGlobalSolver`]                                | `SmCPGlobalSolver` (constructor, `BranchMayContainAnswers`; `LocalSolve` is unit `curve.curve_p1`) |
//! | `GlobalSolve::identical_solutions` for [`CcGlobalSolver`] / [`CpGlobalSolver`], `TreeCurves` (private) | `SmGlobalSolver::IdenticalSolutions` (not overridden upstream: a solution's curve handle resolves to the curve its tree decomposes) |
//! | [`CurvePropertyGs`], [`CurvePropertyType`]        | `SmCurvePropertyGS` (constructor, `FindBestSubdivisionNode`, `BranchMayContainAnswers`), `SmCurvePropertyType` |
//! | [`CciGlobalSolver`]                               | `SmCCIGlobalSolver` (constructor; `LocalSolve` is unit `curve.curve_p7`) |
//! | [`LengthIntegrand`]                               | `SmLengthIFEO` (constructor, `Evaluate`)         |
//! | [`FindCurveDistanceExtrema`]                      | `SmFindCurveDistanceExtremaEFO` (constructor, destructor) |
//!
//! Deviations:
//! - Temporary evaluation copies retain owners and exact NURBS data throughout
//!   owned curve, surface and volume branches, rejecting nesting deeper than
//!   [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING)
//!   before any recursive copy.
//! - **Object model.** `SmCurve` is abstract; its derived classes become the
//!   [`CurveKind`] variants. The ported kinds are [`BSplineCurve`],
//!   [`HermiteCurve`], [`Conic`], [`IsoCurve`], [`ProjectedCurve`], [`OffsetCurve`],
//!   [`CrvInVolume`], [`CrvOnSurf`], [`Hyperbola`] and [`Parabola`] (each an
//!   `SmBSplineCurve`: the variant runs the B-spline methods on its base where it does
//!   not override them) and [`Line`] (`SmLine`, derived from
//!   `SmBSplineCurve`: the inherited B-spline queries run on its B-spline part, made
//!   with its NURB, see [`crate::curve::line`]) and [`CompositeCurve`]
//!   (`SmCompositeCurve`, see [`crate::curve::composite_curve`]); the other analytic and
//!   generated curves (`SmEllipse`,
//!   ...) join the enum in their own units. Each virtual method is
//!   one `match`: a kind that overrides it calls its own method, the others run the
//!   base-class body ported here (the `SmCurve.h` inline defaults: `SM_ERR`,
//!   `FALSE`, `TRUE`, `3`, `2`, `0`, an empty answer). The base numerics that the
//!   kinds call on themselves are generic over [`CurveEvaluate`] (the evaluation
//!   virtuals), so a B-spline, Hermite or conic curve can use them without being
//!   wrapped.
//! - **Owner.** `m_pOwner` is an [`ObjectRef`] (type code and key) instead of a
//!   pointer. `GetEdge` follows owners that are curves or edge uses through a
//!   caller-supplied lookup (their owner pointers live in other objects) and fails
//!   with `InvalidInput` on an owner cycle (upstream loops forever). `SetOwner`
//!   cannot reach the curve's context: it returns the `SM_NO_CHANGE_OWNER`
//!   notification ([`OwnerChange`]) for the caller to deliver, together with the
//!   edge's B-rep (`SM_NO_GET_BREP`), which the caller supplies. `Clone` resets the
//!   owner, as the copy constructor. The constructor and destructor notifications
//!   (`SM_NO_CONSTRUCTION`, `SM_NO_DESTRUCTION`) and the debug cache counter are
//!   not modelled; [`notify`] is the `Notify` override for callers that keep the
//!   curve's [`AObject`] state.
//! - **Cache memo.** A [`Curve`] carries a memo of its curve cache (`CurveMemo`, see
//!   `curve.curve_p5`), which upstream keeps in `SmCacheMgr`. `Clone` and the
//!   evaluation copies share it; every `&mut` edit of the geometry (`kind_mut` and the
//!   editing methods) starts an empty one.
//! - **Arguments.** Out-params become return values; `SmContext` arguments only
//!   choose where upstream allocates and are dropped, as are arguments upstream
//!   marks `NotUsed` (`EvaluateSTEP`'s zone tolerance, `sm_TestStep`'s current
//!   parameter, `sm_FindTestParams`' degree) and `Evaluate`'s `bFromLeft` for
//!   Hermite and line curves. Solutions of the line overrides (`DropPointFast`,
//!   `IntersectWithLine`) name no objects here (the curve has no object reference). `EditParameterization`'s
//!   and `Trim`'s notify flags become [`TrimMode`] (trim) or are dropped (the
//!   B-spline edit does not notify here, see its unit). `bOptCreateAnalytics` of
//!   [`flush_crv`] is honoured as upstream (`USE_ANALYTICS` is defined).
//! - **Statuses ignored upstream.** Where upstream ignores an evaluation status
//!   (`ApproximateLength`, `GetEnds`, the trims of `CreateBySplitCurve`) the port
//!   ignores the error too and keeps upstream's values (the previous sample, or
//!   [`Vec3::UNDEF`] for a never-written `SmVector3d`), except
//!   [`SolidError::Unported`], which is always propagated so a missing port is
//!   never hidden.
//! - **`EuclidianStepOff`** returns `SM_ERR` *with* its outputs when it leaves the
//!   interval or does not converge; here that is `Ok` with
//!   [`StepOff::converged`] `false`. A failing evaluation is an error (upstream's
//!   end parameter is then the start parameter).
//! - **Solvers.** The four curve solvers are structs holding a [`GlobalSolver`] and
//!   implementing [`GlobalSolve`]. Their `LocalSolve` overrides belong to units
//!   `curve.curve_p9` (`SmCCGlobalSolver`, ported there; the solver carries its two
//!   curves and the kernel, [`CcGlobalSolver::with_curves`]), `curve.curve_p1` (`SmCPGlobalSolver`,
//!   ported there; the solver carries its curve, [`CpGlobalSolver::curve`]),
//!   `curve.curve_p3` (`SmCurvePropertyGS`, ported there; the solver carries the
//!   curve it evaluates, [`CurvePropertyGs::curve`], since a tree's owner object is
//!   only the caller's handle) and `curve.curve_p7`
//!   (`SmCCIGlobalSolver`, ported there; the solver carries its two curves and the
//!   kernel, [`CciGlobalSolver::with_curves`]). Curve tree nodes
//!   hold `SmBezierSpan` / `SmBezierAux1d` data built by the curve cache
//!   ([`crate::curve::curve_cache`]), read from the tree
//!   ([`Tree::curve_aux`], [`Tree::curve_span`]); the span end points are those the
//!   cache stored when it built the tree (`GetEnds`); a node pointing at other data
//!   is [`SolidError::InvalidInput`] (upstream casts whatever `m_pData` holds). A node without data (upstream `NER` on a
//!   null `m_pData`) makes `BranchMayContainAnswers` return
//!   `SM_ERR_NULL_POINTER`, which callers read as `TRUE`: the port answers `true`;
//!   in `FindBestSubdivisionNode` a missing child is `SM_ERR_NULL_POINTER`
//!   ([`SolidError::Upstream`], code 1006). Optional vectors or values a property
//!   needs but the caller did not give are [`SolidError::InvalidInput`] (upstream
//!   reads through a null or short array). `SmCurvePropertyGS`'s corner loop keeps
//!   upstream's `dZV = 1.0 * j`.
//! - **Not ported here.** `HasIntervals` / `SetIntervals` of `SmCCIntersectENFO` and
//!   `SmBestLineFitENFO` and the destructors of the evaluator classes
//!   (`SmCurvePropertyEFO`, `SmCCTangentIntENFO`, `SmCCIntersectENFO`,
//!   `SmFindCCNormalENFO`, `SmFindPCExtremaEFO`, `SmCPDistAtENFO`) are trivial
//!   members of classes whose constructors and `Evaluate` belong to other units
//!   (`curve.curve_p3`, `_p4`, ...); they are written with those classes (the
//!   destructors are implicit `Drop`). `SmLengthIFEO` is a plain struct: the
//!   integrator interface it implements (`SmIntegFuncEvalObj`) is unit
//!   `mass.integrator`.
//! - **Unported callees.** `SurfaceMaxDistanceBetween` (over any
//!   [`crate::surface::surface::p2::SurfaceEvaluate`]) needs `SmSurface::DropPoint`
//!   (`surface.surface_p5`) and `LocalCurveSolve`'s evaluator (`SmCSNormalizeENFO`,
//!   `surface.surface_p1`), and
//!   `ReadFromDB` of the analytic and generated curve types needs their units;
//!   those calls return [`SolidError::Unported`].
//! - **Derivative order bound.** [`Curve::evaluate`] returns
//!   [`SolidError::InvalidInput`] for a Hermite or line kind (whose evaluators are
//!   infallible) when `num_derivs` exceeds [`crate::curve::MAX_EVAL_DERIVS`]; the other
//!   kinds' evaluators check the bound themselves or fail earlier (B-splines at
//!   `NL_MAXDER`).

use crate::curve::composite_curve::CompositeCurve;
use crate::curve::conic::Conic;
use crate::curve::crv_in_volume::CrvInVolume;
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p5::CurveMemo;
use crate::curve::ellipse::Ellipse;
use crate::curve::hermite_curve::HermiteCurve;
use crate::curve::hyperbola::Hyperbola;
use crate::curve::iso_curve::IsoCurve;
use crate::curve::line::Line;
use crate::curve::offset_curve::OffsetCurve;
use crate::curve::parabola::Parabola;
use crate::curve::projected_curve::ProjectedCurve;
use crate::curve::tangent_field::TangentFieldCurve;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::a_object::{self, AObject, CacheStore};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::{Context, IdentitySlot};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{
    line_line_closest_point, point_project_to_plane, segment_point_angle_deg_to_perp,
};
use crate::geom::geom_utility::p2::{
    evaluate_curve_continuity_with, frenet_from_derivatives, ContinuityOptions, ContinuityType,
    CurveEval,
};
use crate::geom::global_solver::{
    resolve_object_zone_tol_3d, Branch, GlobalSolve, GlobalSolver, ObjectZoneTol,
    PreferredSolution, SolutionPairType, SolutionRequested, SolverOperation,
};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::solution_array::{ParameterDomains, Solution, SolutionObject};
use crate::geom::tree::{AuxDataType, NodeData, Tree, TreeNode};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{acos, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{
    AnalyticCurve, BSplineCurve, BSplineCurveForm, CanonicalData, Ends, KnotType, NurbCircleParam,
    ProjectionType,
};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::surface::surface::p2::SurfaceEvaluate;
use crate::topology::database_io::DatabaseReader;

// ---- type codes ---------------------------------------------------------------------

/// `CURV_BASE_TYPE` (`SmCurveTypes.h`).
pub(crate) const CURV_BASE_TYPE: i64 = 14000;

/// `SmCurve_TYPE` (`CURV_BASE_TYPE + 1`).
pub const TYPE: i64 = CURV_BASE_TYPE + 1;
/// `SmLine_TYPE`.
pub const LINE_TYPE: i64 = CURV_BASE_TYPE + 10;
/// `SmConic_TYPE`.
pub const CONIC_TYPE: i64 = CURV_BASE_TYPE + 20;
/// `SmCircle_TYPE`.
pub const CIRCLE_TYPE: i64 = CURV_BASE_TYPE + 21;
/// `SmEllipse_TYPE`.
pub const ELLIPSE_TYPE: i64 = CURV_BASE_TYPE + 22;
/// `SmParabola_TYPE`.
pub const PARABOLA_TYPE: i64 = CURV_BASE_TYPE + 23;
/// `SmHyperbola_TYPE`.
pub const HYPERBOLA_TYPE: i64 = CURV_BASE_TYPE + 24;
/// `SmCompositeCurve_TYPE`.
pub const COMPOSITE_CURVE_TYPE: i64 = CURV_BASE_TYPE + 32;
/// `SmBSplineCurve_TYPE`.
pub const BSPLINE_CURVE_TYPE: i64 = CURV_BASE_TYPE + 33;
/// `SmHermiteCurve_TYPE`.
pub const HERMITE_CURVE_TYPE: i64 = CURV_BASE_TYPE + 34;
/// `SmOffsetCurve_TYPE`.
pub const OFFSET_CURVE_TYPE: i64 = CURV_BASE_TYPE + 36;
/// `SmProjectedCurve_TYPE`.
pub const PROJECTED_CURVE_TYPE: i64 = CURV_BASE_TYPE + 37;
/// `SmIsoCurve_TYPE`.
pub const ISO_CURVE_TYPE: i64 = CURV_BASE_TYPE + 45;
/// `SmCrvOnSurf_TYPE`.
pub const CRV_ON_SURF_TYPE: i64 = CURV_BASE_TYPE + 48;
/// `SmTangentField_TYPE`.
pub const TANGENT_FIELD_TYPE: i64 = CURV_BASE_TYPE + 50;
/// `SmCrvInVolume_TYPE`.
pub const CRV_IN_VOLUME_TYPE: i64 = CURV_BASE_TYPE + 53;

/// Every class derived from `SmCurve` (their `IsKindOf(SmCurve_TYPE)` is true).
const CURVE_CLASS_TYPES: [i64; 16] = [
    TYPE,
    LINE_TYPE,
    CONIC_TYPE,
    CIRCLE_TYPE,
    ELLIPSE_TYPE,
    PARABOLA_TYPE,
    HYPERBOLA_TYPE,
    COMPOSITE_CURVE_TYPE,
    BSPLINE_CURVE_TYPE,
    HERMITE_CURVE_TYPE,
    OFFSET_CURVE_TYPE,
    PROJECTED_CURVE_TYPE,
    ISO_CURVE_TYPE,
    CRV_ON_SURF_TYPE,
    TANGENT_FIELD_TYPE,
    CRV_IN_VOLUME_TYPE,
];

/// `SmEdge_TYPE` (`TOPO_BASE_TYPE + 16`).
const EDGE_TYPE: i64 = 16016;
/// `SmEdgeuse_TYPE` (`TOPO_BASE_TYPE + 5`).
const EDGEUSE_TYPE: i64 = 16005;

/// `SmCurve::GetTypeString`.
pub const TYPE_STRING: &str = "SmCurve_TYPE";
/// `SmCurve::GetClassString`.
pub const CLASS_STRING: &str = "SmCurve";

/// `SmCurve::IsKindOf`: `SmCurve_TYPE` or a base type of `SmCurve`.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || a_object::is_kind_of(t)
}

/// Whether `t` is the type of an `SmCurve` class (`IsKindOf(SmCurve_TYPE)` of an
/// object of that type).
pub fn is_curve_class(t: i64) -> bool {
    CURVE_CLASS_TYPES.contains(&t)
}

const OP_EDGE: &str = "curve::curve::p2::Curve::edge";
const OP_SPLIT: &str = "curve::curve::p2::Curve::create_by_split_curve";
const OP_BASE: &str = "curve::curve::p2::Curve";
const OP_STACK: &str = "curve::curve::p2::stack";
const OP_ADD: &str = "curve::curve::p2::add_to_curve";
const OP_FLUSH: &str = "curve::curve::p2::flush_crv";
const OP_GEOM: &str = "curve::curve::p2::evaluate_geometric";
const OP_READ: &str = "curve::curve::p2::Curve::read_from_db";
const OP_GS: &str = "curve::curve::p2::CurvePropertyGs";
const OP_EVAL: &str = "curve::curve::p2::Curve::evaluate";

/// `SM_ERR` of a base-class default.
const fn base_err() -> SolidError {
    SolidError::sm_err(OP_BASE)
}

// ---- the evaluation interface ---------------------------------------------------------

/// The evaluation virtuals of `SmCurve` that the base-class numerics call on
/// `this`: implemented by every curve kind and by [`Curve`] (which dispatches).
pub trait CurveEvaluate {
    /// Image-space dimension (`GetDim`).
    fn dim(&self) -> u64;

    /// The natural parameter interval (`GetNaturalInterval`).
    fn natural_interval(&self) -> Extent1d;

    /// Position and derivatives `0..=num_derivs` at `t` (`Evaluate`), from the left
    /// or right at a knot; with `non_zero_tangents` a zero first derivative is
    /// replaced by a tolerance-sized one along the limiting direction.
    fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>>;

    /// The point at `t` (`EvaluatePoint`).
    fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3>;

    /// The virtual `IsClosed(crInterval, dTol)`: the base class
    /// ([`crate::curve::curve::p4::is_closed_base`]) unless the kind overrides it.
    ///
    /// # Errors
    ///
    /// Those of the evaluations.
    fn is_closed_virtual(&self, k: &Kernel, interval: &Extent1d, tol: f64) -> Result<bool> {
        crate::curve::curve::p4::is_closed_base(k, self, interval, tol)
    }

    /// `SM_CAST_PTR(SmCrvOnSurf, this)`: the curve as a curve on a surface, if it is
    /// one.
    fn as_crv_on_surf(&self) -> Option<&CrvOnSurf> {
        None
    }
}

impl CurveEvaluate for BSplineCurve {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        _k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, t, num_derivs, from_left, non_zero_tangents)
    }

    fn evaluate_point(&self, _k: &Kernel, t: f64) -> Result<Point3> {
        Self::evaluate_point(self, t)
    }
}

impl CurveEvaluate for HermiteCurve {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        _k: &Kernel,
        t: f64,
        num_derivs: usize,
        _from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Ok(Self::evaluate(self, t, num_derivs, non_zero_tangents))
    }

    fn evaluate_point(&self, _k: &Kernel, t: f64) -> Result<Point3> {
        Ok(Self::evaluate_point(self, t))
    }
}

impl CurveEvaluate for Conic {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        _k: &Kernel,
        t: f64,
        num_derivs: usize,
        _from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, t, num_derivs, non_zero_tangents)
    }

    fn evaluate_point(&self, _k: &Kernel, t: f64) -> Result<Point3> {
        Ok(Self::evaluate_point(self, t))
    }
}

impl CurveEvaluate for IsoCurve {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, k, t, num_derivs, from_left, non_zero_tangents)
    }

    fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        Self::evaluate_point(self, k, t)
    }
}

impl CurveEvaluate for ProjectedCurve {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, k, t, num_derivs, from_left, non_zero_tangents)
    }

    fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        Self::evaluate_point(self, k, t)
    }
}

impl CurveEvaluate for OffsetCurve {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, k, t, num_derivs, from_left, non_zero_tangents)
    }

    fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        Self::evaluate_point(self, k, t)
    }
}

impl CurveEvaluate for Curve {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, k, t, num_derivs, from_left, non_zero_tangents)
    }

    fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        Self::evaluate_point(self, k, t)
    }

    fn is_closed_virtual(&self, k: &Kernel, interval: &Extent1d, tol: f64) -> Result<bool> {
        Self::is_closed(self, k, interval, tol)
    }

    fn as_crv_on_surf(&self) -> Option<&CrvOnSurf> {
        match &self.kind {
            CurveKind::CrvOnSurf(c) => Some(c),
            CurveKind::TangentField(c) => Some(c.base()),
            _ => None,
        }
    }
}

/// Entry `i` of an evaluation, or `UNDEF` (an unwritten `SmVector3d`).
pub(crate) fn at(v: &[Vec3], i: usize) -> Vec3 {
    v.get(i).copied().unwrap_or(Vec3::UNDEF)
}

// ---- the curve object ---------------------------------------------------------------

/// The concrete kind of a [`Curve`] (the classes derived from `SmCurve`).
#[derive(Debug, Clone)]
pub enum CurveKind {
    /// `SmBSplineCurve`.
    BSpline(BSplineCurve),
    /// `SmHermiteCurve`.
    Hermite(HermiteCurve),
    /// `SmHyperbola` (an `SmBSplineCurve`).
    Hyperbola(Hyperbola),
    /// `SmProjectedCurve`.
    Projected(ProjectedCurve),
    /// `SmIsoCurve`.
    Iso(IsoCurve),
    /// `SmOffsetCurve`.
    Offset(OffsetCurve),
    /// `SmConic`.
    Conic(Conic),
    /// `SmCrvInVolume`.
    CrvInVolume(Box<CrvInVolume>),
    /// `SmCrvOnSurf`.
    CrvOnSurf(Box<CrvOnSurf>),
    /// `SmTangentField` (an `SmCrvOnSurf`).
    TangentField(Box<TangentFieldCurve>),
    /// `SmLine`.
    Line(Line),
    /// `SmEllipse` and `SmCircle` (an `SmBSplineCurve`).
    Ellipse(Ellipse),
    /// `SmParabola` (an `SmBSplineCurve`).
    Parabola(Parabola),
    /// `SmCompositeCurve`.
    Composite(CompositeCurve),
}

/// A curve (`SmCurve`): its concrete kind and the object owning it (`m_pOwner`:
/// an edge, edge use, face or another curve).
#[derive(Debug)]
pub struct Curve {
    kind: CurveKind,
    owner: Option<ObjectRef>,
    /// The kernel-issued identity this curve object is notified under (upstream passes
    /// the object itself), issued by the context of the first notification that names
    /// it ([`IdentitySlot`]).
    notify_identity: IdentitySlot,
    /// Not upstream: the memo of the curve's cache ([`CurveMemo`]), shared by clones
    /// and emptied by every edit of the geometry.
    memo: CurveMemo,
    /// Not upstream: the tolerance of the edge whose shape this curve defines, with the
    /// owner it was recorded under ([`Curve::set_owner_zone_tol_3d`]).
    owner_zone_tol_3d: Option<(Option<ObjectRef>, f64)>,
}

/// The copy constructor: copies the geometry, resets the owner (and the identity: a
/// copy is another object). The copy shares the curve cache memo (same geometry).
impl Clone for Curve {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind.clone(),
            owner: None,
            notify_identity: IdentitySlot::new(),
            memo: self.memo.share(),
            owner_zone_tol_3d: None,
        }
    }
}

impl Curve {
    /// Exact recursive copy after the caller validates the complete geometry graph.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        let kind = match &self.kind {
            CurveKind::BSpline(c) => CurveKind::BSpline(c.clone_exact()),
            CurveKind::Hermite(c) => CurveKind::Hermite(*c),
            CurveKind::Hyperbola(c) => CurveKind::Hyperbola(Hyperbola {
                base: c.base.clone_exact(),
                position: c.position,
                anal_domain: c.anal_domain,
                semi_axis: c.semi_axis,
                semi_image_axis: c.semi_image_axis,
            }),
            CurveKind::Projected(c) => CurveKind::Projected(c.clone_exact_bounded()),
            CurveKind::Iso(c) => CurveKind::Iso(c.clone_exact_bounded()),
            CurveKind::Offset(c) => CurveKind::Offset(c.clone_exact_bounded()),
            CurveKind::Conic(c) => CurveKind::Conic(*c),
            CurveKind::CrvInVolume(c) => CurveKind::CrvInVolume(Box::new(c.clone_exact_bounded())),
            CurveKind::CrvOnSurf(c) => CurveKind::CrvOnSurf(Box::new(c.clone_exact_shared())),
            CurveKind::TangentField(c) => CurveKind::TangentField(Box::new(c.as_ref().clone())),
            CurveKind::Line(c) => CurveKind::Line(c.clone_exact_bounded()),
            CurveKind::Ellipse(c) => CurveKind::Ellipse(c.clone_exact_bounded()),
            CurveKind::Parabola(c) => CurveKind::Parabola(c.clone_exact_bounded()),
            CurveKind::Composite(c) => CurveKind::Composite(c.clone_exact_bounded()),
        };
        Self {
            kind,
            owner: self.owner,
            // an evaluation snapshot is never notified under the original's identity
            notify_identity: IdentitySlot::new(),
            memo: self.memo.share(),
            owner_zone_tol_3d: self.owner_zone_tol_3d,
        }
    }
}

impl From<BSplineCurve> for Curve {
    fn from(c: BSplineCurve) -> Self {
        Self::new(CurveKind::BSpline(c))
    }
}

impl From<HermiteCurve> for Curve {
    fn from(c: HermiteCurve) -> Self {
        Self::new(CurveKind::Hermite(c))
    }
}

impl From<Ellipse> for Curve {
    fn from(c: Ellipse) -> Self {
        Self::new(CurveKind::Ellipse(c))
    }
}

impl From<Line> for Curve {
    fn from(c: Line) -> Self {
        Self::new(CurveKind::Line(c))
    }
}

impl From<Parabola> for Curve {
    fn from(c: Parabola) -> Self {
        Self::new(CurveKind::Parabola(c))
    }
}

impl From<CompositeCurve> for Curve {
    fn from(c: CompositeCurve) -> Self {
        Self::new(CurveKind::Composite(c))
    }
}

impl From<Conic> for Curve {
    fn from(c: Conic) -> Self {
        Self::new(CurveKind::Conic(c))
    }
}

impl From<IsoCurve> for Curve {
    fn from(c: IsoCurve) -> Self {
        Self::new(CurveKind::Iso(c))
    }
}

impl From<CrvInVolume> for Curve {
    fn from(c: CrvInVolume) -> Self {
        Self::new(CurveKind::CrvInVolume(Box::new(c)))
    }
}

impl From<CrvOnSurf> for Curve {
    fn from(c: CrvOnSurf) -> Self {
        Self::new(CurveKind::CrvOnSurf(Box::new(c)))
    }
}

impl From<TangentFieldCurve> for Curve {
    fn from(c: TangentFieldCurve) -> Self {
        Self::new(CurveKind::TangentField(Box::new(c)))
    }
}

impl From<ProjectedCurve> for Curve {
    fn from(c: ProjectedCurve) -> Self {
        Self::new(CurveKind::Projected(c))
    }
}

impl From<OffsetCurve> for Curve {
    fn from(c: OffsetCurve) -> Self {
        Self::new(CurveKind::Offset(c))
    }
}

/// The `SM_NO_CHANGE_OWNER` notification [`Curve::set_owner`] asks the caller to
/// deliver (caller the curve; data the new owner, its B-rep, the old owner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerChange {
    /// `pData1`: the new owner (`None` = owner removed).
    pub new_owner: Option<ObjectRef>,
    /// `pData2`: the new owner edge's B-rep (`SM_NO_GET_BREP`), if any.
    pub new_owner_brep: Option<ObjectRef>,
    /// `pData3`: the previous owner.
    pub old_owner: Option<ObjectRef>,
}

/// Result of [`Curve::intersect_with_line`] (`IntersectWithLine`).
#[derive(Debug, Clone, PartialEq)]
pub struct LineIntersection {
    /// Intersections found (`[curve param, line param]`).
    pub solutions: Vec<Solution<ObjectRef>>,
    /// Whether the general curve/curve intersector must be called.
    pub needs_more_intersections: bool,
}

impl Curve {
    /// A temporary evaluation copy retaining owners, concrete kinds and exact
    /// NURBS data, without copy-constructor knot fix-up. Notification identity is
    /// not copied: the temporary is not another notification target.
    pub(crate) fn clone_for_evaluation(&self) -> Result<Self> {
        self.clone_for_evaluation_depth(0)
    }

    /// Copy every owned geometry branch within Surface's shared `MAX_GEOMETRY_NESTING` (32) bound.
    pub(crate) fn clone_for_evaluation_depth(&self, depth: usize) -> Result<Self> {
        crate::surface::surface::p2::Surface::check_curve_evaluation_copy_depth(self, depth)?;
        let kind = match &self.kind {
            CurveKind::BSpline(c) => CurveKind::BSpline(c.clone_exact()),
            CurveKind::Hermite(c) => CurveKind::Hermite(*c),
            CurveKind::Conic(c) => CurveKind::Conic(*c),
            CurveKind::Line(c) => {
                let mut copy = c.clone();
                *copy.base_mut() = c.base().clone_exact();
                CurveKind::Line(copy)
            }
            CurveKind::Ellipse(c) => {
                let mut copy = c.clone();
                *copy.base_mut() = c.base().clone_exact();
                CurveKind::Ellipse(copy)
            }
            CurveKind::Hyperbola(c) => {
                let mut copy = c.clone();
                *copy.base_mut() = c.base().clone_exact();
                CurveKind::Hyperbola(copy)
            }
            CurveKind::Parabola(c) => {
                let mut copy = c.clone();
                *copy.base_mut() = c.base().clone_exact();
                CurveKind::Parabola(copy)
            }
            CurveKind::Projected(c) => CurveKind::Projected(c.clone_for_evaluation(depth + 1)?),
            CurveKind::Iso(c) => CurveKind::Iso(c.clone_for_evaluation(depth + 1)?),
            CurveKind::Offset(c) => CurveKind::Offset(c.clone_for_evaluation(depth + 1)?),
            CurveKind::CrvInVolume(c) => {
                CurveKind::CrvInVolume(Box::new(c.clone_for_evaluation(depth + 1)?))
            }
            CurveKind::CrvOnSurf(c) => {
                CurveKind::CrvOnSurf(Box::new(c.clone_for_evaluation(depth + 1)?))
            }
            CurveKind::TangentField(c) => {
                CurveKind::TangentField(Box::new(c.clone_for_evaluation(depth + 1)?))
            }
            CurveKind::Composite(c) => CurveKind::Composite(c.clone_for_evaluation(depth + 1)?),
        };
        Ok(Self {
            kind,
            owner: self.owner,
            notify_identity: IdentitySlot::new(),
            memo: self.memo.share(),
            owner_zone_tol_3d: self.owner_zone_tol_3d,
        })
    }

    /// A curve of `kind` without owner (`SmCurve(ULONG)` plus the derived
    /// constructor).
    #[must_use]
    pub const fn new(kind: CurveKind) -> Self {
        Self {
            kind,
            owner: None,
            notify_identity: IdentitySlot::new(),
            memo: CurveMemo::new(),
            owner_zone_tol_3d: None,
        }
    }

    /// The curve a cache keeps of its own: an exact copy (as
    /// [`Curve::clone_for_evaluation`], without the copy constructor's knot fix-up,
    /// since upstream's `SmCurveCache::m_crCurve` references the curve itself) without
    /// owner and with an empty curve cache memo of its own, so a memo never holds
    /// itself.
    /// # Errors
    /// Those of [`Curve::clone_for_evaluation`] (geometry nested too deeply).
    pub(crate) fn clone_detached(&self) -> Result<Self> {
        let copy = self.clone_for_evaluation()?;
        Ok(Self {
            kind: copy.kind,
            owner: None,
            notify_identity: IdentitySlot::new(),
            memo: CurveMemo::new(),
            owner_zone_tol_3d: None,
        })
    }

    /// The curve cache memo (see [`CurveMemo`]).
    pub(crate) const fn memo(&self) -> &CurveMemo {
        &self.memo
    }

    /// The key this curve object is notified under, once a context has issued it one
    /// (the kernel issues it; there is no setter).
    #[must_use]
    pub fn notify_key(&self) -> Option<ObjectKey> {
        self.notify_identity.value().map(ObjectKey)
    }

    /// The slot of this curve object's notification identity, for the context that
    /// issues it.
    pub(crate) fn notify_identity_slot(&mut self) -> &mut IdentitySlot {
        &mut self.notify_identity
    }

    /// The concrete kind.
    #[must_use]
    pub const fn kind(&self) -> &CurveKind {
        &self.kind
    }

    /// The concrete kind, mutable.
    pub fn kind_mut(&mut self) -> &mut CurveKind {
        // an edit makes another curve value: start an empty memo
        self.memo = CurveMemo::new();
        &mut self.kind
    }

    /// The concrete kind, consuming the curve.
    #[must_use]
    pub fn into_kind(self) -> CurveKind {
        self.kind
    }

    // ---- type queries ---------------------------------------------------------------

    /// The `SM_TYPE` of the concrete class (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        match &self.kind {
            CurveKind::Composite(_) => COMPOSITE_CURVE_TYPE,
            CurveKind::BSpline(_) => BSplineCurve::TYPE,
            CurveKind::Hermite(_) => HermiteCurve::TYPE,
            CurveKind::Hyperbola(_) => Hyperbola::TYPE,
            CurveKind::Conic(_) => Conic::TYPE,
            CurveKind::Projected(_) => ProjectedCurve::TYPE,
            CurveKind::Iso(_) => IsoCurve::TYPE,
            CurveKind::Offset(_) => OffsetCurve::TYPE,
            CurveKind::CrvInVolume(_) => CrvInVolume::TYPE,
            CurveKind::CrvOnSurf(_) => CrvOnSurf::TYPE,
            CurveKind::TangentField(_) => TangentFieldCurve::TYPE,
            CurveKind::Line(_) => Line::TYPE,
            CurveKind::Ellipse(c) => c.type_code(),
            CurveKind::Parabola(_) => Parabola::TYPE,
        }
    }

    /// `GetTypeString` of the concrete class.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        match &self.kind {
            CurveKind::Composite(_) => crate::curve::composite_curve::p1::TYPE_STRING,
            CurveKind::BSpline(_) => "SmBSplineCurve_TYPE",
            CurveKind::Hermite(_) => "SmHermiteCurve_TYPE",
            CurveKind::Hyperbola(_) => "SmHyperbola_TYPE",
            CurveKind::Conic(_) => "SmConic_TYPE",
            CurveKind::Projected(_) => "SmProjectedCurve_TYPE",
            CurveKind::Iso(_) => "SmIsoCurve_TYPE",
            CurveKind::Offset(_) => "SmOffsetCurve_TYPE",
            CurveKind::CrvInVolume(_) => "SmCrvInVolume_TYPE",
            CurveKind::CrvOnSurf(_) => "SmCrvOnSurf_TYPE",
            CurveKind::TangentField(_) => crate::curve::tangent_field::TYPE_STRING,
            CurveKind::Line(_) => "SmLine_TYPE",
            CurveKind::Ellipse(c) => c.type_string(),
            CurveKind::Parabola(_) => "SmParabola_TYPE",
        }
    }

    /// `GetClassString` of the concrete class.
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        match &self.kind {
            CurveKind::Composite(_) => crate::curve::composite_curve::p1::CLASS_STRING,
            CurveKind::BSpline(_) => "SmBSplineCurve",
            CurveKind::Hermite(_) => "SmHermiteCurve",
            CurveKind::Hyperbola(_) => "SmHyperbola",
            CurveKind::Conic(_) => "SmConic",
            CurveKind::Projected(_) => "SmProjectedCurve",
            CurveKind::Iso(_) => "SmIsoCurve",
            CurveKind::Offset(_) => "SmOffsetCurve",
            CurveKind::CrvInVolume(_) => "SmCrvInVolume",
            CurveKind::CrvOnSurf(_) => "SmCrvOnSurf",
            CurveKind::TangentField(_) => crate::curve::tangent_field::CLASS_STRING,
            CurveKind::Line(_) => "SmLine",
            CurveKind::Ellipse(c) => c.class_string(),
            CurveKind::Parabola(_) => "SmParabola",
        }
    }

    /// `IsKindOf`: the concrete type or one of its base types.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        match &self.kind {
            CurveKind::Hyperbola(_) => Hyperbola::is_kind_of(t),
            CurveKind::Line(c) => c.is_kind_of(t),
            CurveKind::Ellipse(c) => c.kind_of(t),
            CurveKind::Parabola(c) => c.is_kind_of(t),
            CurveKind::TangentField(c) => c.is_kind_of(t),
            _ => t == self.type_code() || is_kind_of(t),
        }
    }

    /// `operator==`: same object, or same type and dimension and (per kind) the same
    /// data.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from [`CrvInVolume::approx_eq`] (the volume
    /// comparison).
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }
        match (&self.kind, &other.kind) {
            (CurveKind::BSpline(a), CurveKind::BSpline(b)) => Ok(a.approx_eq(b)),
            (CurveKind::Hermite(a), CurveKind::Hermite(b)) => Ok(a.approx_eq(b)),
            (CurveKind::Hyperbola(a), CurveKind::Hyperbola(b)) => Ok(a.approx_eq(b)),
            (CurveKind::Conic(a), CurveKind::Conic(b)) => Ok(a.approx_eq(b)),
            (CurveKind::Projected(a), CurveKind::Projected(b)) => a.approx_eq(k, b),
            (CurveKind::Iso(a), CurveKind::Iso(b)) => a.approx_eq(k, b),
            (CurveKind::Offset(a), CurveKind::Offset(b)) => a.approx_eq(k, b),
            (CurveKind::CrvInVolume(a), CurveKind::CrvInVolume(b)) => a.approx_eq(k, b),
            (CurveKind::CrvOnSurf(a), CurveKind::CrvOnSurf(b)) => a.approx_eq(k, b),
            (CurveKind::TangentField(a), CurveKind::TangentField(b)) => a.approx_eq(k, b),
            (CurveKind::Line(a), CurveKind::Line(b)) => Ok(a.approx_eq(b)),
            (CurveKind::Ellipse(a), CurveKind::Ellipse(b)) => {
                Ok(a.type_code() == b.type_code() && a.approx_eq(b))
            }
            (CurveKind::Parabola(a), CurveKind::Parabola(b)) => Ok(a.approx_eq(b)),
            (CurveKind::Composite(a), CurveKind::Composite(b)) => a.approx_eq(k, b),
            // GetType() differs
            _ => Ok(false),
        }
    }

    // ---- simple queries -------------------------------------------------------------

    /// Image-space dimension (`GetDim`, `m_lDim`).
    #[must_use]
    pub const fn dim(&self) -> u64 {
        match &self.kind {
            CurveKind::Composite(c) => c.dim(),
            CurveKind::BSpline(c) => c.dim(),
            CurveKind::Hermite(c) => c.dim(),
            CurveKind::Hyperbola(c) => c.base().dim(),
            CurveKind::Ellipse(c) => c.base().dim(),
            CurveKind::Conic(c) => c.dim(),
            CurveKind::Projected(c) => c.dim(),
            CurveKind::Iso(c) => c.dim(),
            CurveKind::Offset(c) => c.dim(),
            CurveKind::CrvInVolume(c) => c.dim(),
            CurveKind::CrvOnSurf(c) => c.dim(),
            CurveKind::TangentField(c) => c.base().dim(),
            CurveKind::Line(c) => c.dim(),
            CurveKind::Parabola(c) => c.dim(),
        }
    }

    /// The degree (`GetDegree`; `3` for a Hermite curve, the base default).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline without NURB.
    pub fn degree(&self) -> Result<usize> {
        match &self.kind {
            CurveKind::BSpline(c) => c.degree(),
            CurveKind::Hyperbola(c) => c.base().degree(),
            CurveKind::Ellipse(c) => Ok(c.degree()),
            CurveKind::Parabola(c) => c.base().degree(),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => Ok(3),
            CurveKind::Line(c) => Ok(c.degree()),
        }
    }

    /// Knots with repeats (`GetNumberNaturalKnots`; base default `2`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline without NURB.
    pub fn number_natural_knots(&self, k: &Kernel) -> Result<usize> {
        match &self.kind {
            CurveKind::BSpline(c) => c.number_natural_knots(),
            CurveKind::Hyperbola(c) => c.base().number_natural_knots(),
            CurveKind::Ellipse(c) => c.base().number_natural_knots(),
            CurveKind::Parabola(c) => c.base().number_natural_knots(),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Iso(_)
            | CurveKind::Composite(_) => Ok(2),
            CurveKind::Offset(c) => c.number_natural_knots(k),
            CurveKind::Projected(c) => c.number_natural_knots(k),
            CurveKind::CrvInVolume(c) => c.number_natural_knots_ref(k),
            CurveKind::CrvOnSurf(c) => c.number_natural_knots_ref(k),
            CurveKind::TangentField(c) => c.base().number_natural_knots_ref(k),
            CurveKind::Line(c) => Ok(c.number_natural_knots()),
        }
    }

    /// Control points (`GetNumberControlPoints`; base default `0`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline without NURB.
    pub fn number_control_points(&self) -> Result<usize> {
        match &self.kind {
            CurveKind::BSpline(c) => c.number_control_points(),
            CurveKind::Hyperbola(c) => c.base().number_control_points(),
            CurveKind::Ellipse(c) => c.base().number_control_points(),
            CurveKind::Parabola(c) => c.base().number_control_points(),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => Ok(0),
            CurveKind::Line(c) => c.base_with_nurb()?.number_control_points(),
        }
    }

    /// The natural interval (`GetNaturalInterval`).
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        match &self.kind {
            CurveKind::Composite(c) => c.natural_interval(),
            CurveKind::BSpline(c) => c.natural_interval(),
            CurveKind::Hermite(c) => c.natural_interval(),
            CurveKind::Hyperbola(c) => c.base().natural_interval(),
            CurveKind::Ellipse(c) => c.base().natural_interval(),
            CurveKind::Conic(c) => c.natural_interval(),
            CurveKind::Projected(c) => c.natural_interval(),
            CurveKind::Iso(c) => c.natural_interval(),
            CurveKind::Offset(c) => c.natural_interval(),
            CurveKind::CrvInVolume(c) => c.natural_interval(),
            CurveKind::CrvOnSurf(c) => c.natural_interval(),
            CurveKind::TangentField(c) => c.base().natural_interval(),
            CurveKind::Line(c) => c.natural_interval(),
            CurveKind::Parabola(c) => c.base().natural_interval(),
        }
    }

    /// `IsAnalytic` (base default `FALSE`; `TRUE` for a hyperbola, a parabola or a
    /// line).
    #[must_use]
    pub const fn is_analytic(&self) -> bool {
        match &self.kind {
            CurveKind::Hyperbola(c) => c.is_analytic(),
            CurveKind::Ellipse(c) => c.is_analytic(),
            CurveKind::Line(c) => c.is_analytic(),
            CurveKind::Parabola(c) => c.is_analytic(),
            CurveKind::BSpline(_)
            | CurveKind::Hermite(_)
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

    /// `IsBounded` (base default `TRUE`: B-splines and finite analytic curves; a
    /// hyperbola, a parabola or a line asks its analytic domain).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        match &self.kind {
            CurveKind::Hyperbola(c) => c.is_bounded(),
            CurveKind::Line(c) => c.is_bounded(),
            CurveKind::Parabola(c) => c.is_bounded(),
            CurveKind::BSpline(_)
            | CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Composite(_) => true,
            CurveKind::Projected(c) => c.is_bounded(),
            CurveKind::CrvInVolume(c) => c.is_bounded(),
            CurveKind::CrvOnSurf(c) => c.is_bounded(),
            CurveKind::TangentField(c) => c.base().is_bounded(),
        }
    }

    /// `IsRational` (base: an error message and `FALSE`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline without NURB.
    pub fn is_rational(&self) -> Result<bool> {
        match &self.kind {
            CurveKind::BSpline(c) => c.is_rational(),
            CurveKind::Hyperbola(c) => c.base().is_rational(),
            CurveKind::Ellipse(c) => c.base().is_rational(),
            CurveKind::Parabola(c) => c.base().is_rational(),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => Ok(false),
            CurveKind::Line(c) => c.base_with_nurb()?.is_rational(),
        }
    }

    /// `HasRepeatedEndControlPoints` (base default `FALSE`).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineCurve::has_repeated_end_control_points`].
    pub fn has_repeated_end_control_points(&self, k: &Kernel, tol: f64) -> Result<bool> {
        match &self.kind {
            CurveKind::BSpline(c) => c.has_repeated_end_control_points(k, tol),
            CurveKind::Hyperbola(c) => c.base().has_repeated_end_control_points(k, tol),
            CurveKind::Ellipse(c) => c.base().has_repeated_end_control_points(k, tol),
            CurveKind::Parabola(c) => c.base().has_repeated_end_control_points(k, tol),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => Ok(false),
            CurveKind::Line(c) => c.base_with_nurb()?.has_repeated_end_control_points(k, tol),
        }
    }

    // ---- owner ----------------------------------------------------------------------

    /// The owning object (`GetOwner`, `m_pOwner`).
    #[must_use]
    pub const fn owner(&self) -> Option<ObjectRef> {
        self.owner
    }

    /// The tolerance of the edge whose shape this curve defines
    /// (`SmTol::GetZoneTol3d(const SmObject*)` on a curve: `GetEdge()->GetTolerance()`),
    /// when a caller holding the edge recorded it on this curve value
    /// ([`Curve::set_owner_zone_tol_3d`]) and the owner has not changed since; `None`
    /// otherwise (the curve's caches then use the system tolerance).
    #[must_use]
    pub fn owner_zone_tol_3d(&self) -> Option<f64> {
        match self.owner_zone_tol_3d {
            Some((recorded, tol)) if recorded == self.owner => Some(tol),
            _ => None,
        }
    }

    /// Records the tolerance of the edge whose shape this curve defines (`None` clears
    /// it), for the caches built over this value (`SmCurve::CacheMakeOrValidate` reads
    /// it through the owner, `SmTol::GetApproxTol3d(&m_crCurve)`). Not upstream: the
    /// port's curves cannot reach their edge, so a caller that holds the edge records
    /// its tolerance on its evaluation copy of the edge's curve
    /// ([`Curve::clone_for_evaluation`] keeps it; a [`Clone`], a new curve without
    /// owner as upstream's copy, does not). A later owner change
    /// ([`Curve::set_owner`]) makes the record unreadable.
    pub fn set_owner_zone_tol_3d(&mut self, tol: Option<f64>) {
        self.owner_zone_tol_3d = tol.map(|t| (self.owner, t));
    }

    /// Sets the owner and returns the previous one (`SetOwner`), plus the
    /// `SM_NO_CHANGE_OWNER` notification to deliver when the new owner is `None`
    /// or an edge, differs from the old one, and the curve is 3D.
    /// `new_owner_brep` is the new owner edge's B-rep (`SM_NO_GET_BREP`).
    pub fn set_owner(
        &mut self,
        new_owner: Option<ObjectRef>,
        new_owner_brep: Option<ObjectRef>,
    ) -> (Option<ObjectRef>, Option<OwnerChange>) {
        // notify when pNewOwner is NULL or an Edge and curve is 3d - do not report UVTrimCurve changes
        let mut change = None;
        if new_owner.is_none_or(|o| o.type_code == EDGE_TYPE)
            && new_owner != self.owner
            && self.dim() == 3
        {
            change = Some(OwnerChange {
                new_owner,
                new_owner_brep: if new_owner.is_some() {
                    new_owner_brep
                } else {
                    None
                },
                old_owner: self.owner,
            });
        }
        let previous = self.owner;
        self.owner = new_owner;
        (previous, change)
    }

    /// The edge whose shape this curve defines, directly or through owning curves
    /// (generated curves) and edge uses (UV trim curves) (`GetEdge`). `owner_of`
    /// returns the owner of such an intermediate object.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an owner chain longer than 64 links (a cycle;
    /// upstream loops forever).
    pub fn edge(
        &self,
        mut owner_of: impl FnMut(ObjectRef) -> Option<ObjectRef>,
    ) -> Result<Option<ObjectRef>> {
        let mut this_owner = self.owner;
        let mut links = 0usize;

        // when the curve has an owner
        while let Some(o) = this_owner {
            // When curve defines a Edge's shape
            if o.type_code == EDGE_TYPE {
                return Ok(Some(o));
            }
            links += 1;
            if links > 64 {
                return Err(SolidError::InvalidInput {
                    operation: OP_EDGE,
                    reason: "cyclic owner chain",
                });
            }
            // set up for next iter: a GenCurve's owner is a curve, a UVTrimCurve's an edgeuse
            this_owner = if is_curve_class(o.type_code) || o.type_code == EDGEUSE_TYPE {
                owner_of(o)
            } else {
                None
            };
        }
        Ok(None)
    }

    // ---- evaluation -----------------------------------------------------------------

    /// Position and derivatives `0..=num_derivs` at `t` (`Evaluate`).
    ///
    /// # Errors
    ///
    /// Those of the kind's evaluator; [`SolidError::InvalidInput`] for a Hermite or
    /// line kind when `num_derivs` exceeds [`crate::curve::MAX_EVAL_DERIVS`].
    pub fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        crate::limits::probe();
        match &self.kind {
            CurveKind::Composite(_) => Err(base_err()),
            CurveKind::BSpline(c) => c.evaluate(t, num_derivs, from_left, non_zero_tangents),
            CurveKind::Hermite(c) => {
                // The Hermite evaluator is infallible: bound its output size here.
                crate::curve::check_eval_derivs(num_derivs, OP_EVAL)?;
                Ok(c.evaluate(t, num_derivs, non_zero_tangents))
            }
            CurveKind::Hyperbola(c) => {
                c.base()
                    .evaluate(t, num_derivs, from_left, non_zero_tangents)
            }
            CurveKind::Ellipse(c) => c
                .base()
                .evaluate(t, num_derivs, from_left, non_zero_tangents),
            CurveKind::Conic(c) => c.evaluate(t, num_derivs, non_zero_tangents),
            CurveKind::Parabola(c) => {
                c.base()
                    .evaluate(t, num_derivs, from_left, non_zero_tangents)
            }
            CurveKind::Projected(c) => c.evaluate(k, t, num_derivs, from_left, non_zero_tangents),
            CurveKind::Iso(c) => c.evaluate(k, t, num_derivs, from_left, non_zero_tangents),
            CurveKind::Offset(c) => c.evaluate(k, t, num_derivs, from_left, non_zero_tangents),
            CurveKind::CrvInVolume(c) => c.evaluate(k, t, num_derivs, from_left, non_zero_tangents),
            CurveKind::CrvOnSurf(c) => c.evaluate(k, t, num_derivs, from_left, non_zero_tangents),
            // SmTangentField::Evaluate (bFromLeft and bNonZeroTangents unused)
            CurveKind::TangentField(c) => c.evaluate(k, t, num_derivs),
            CurveKind::Line(c) => {
                // The line evaluator is infallible: bound its output size here.
                crate::curve::check_eval_derivs(num_derivs, OP_EVAL)?;
                Ok(c.evaluate(t, num_derivs))
            }
        }
    }

    /// [`Curve::evaluate`] into the caller's slice (`num_derivs = out.len() - 1`):
    /// the entries the evaluation returns are written, the others are set to
    /// [`Vec3::UNDEF`] (what [`at`] reads for a missing entry). The B-spline kinds
    /// evaluate without a heap allocation ([`BSplineCurve::evaluate_into`]).
    ///
    /// # Errors
    ///
    /// As [`Curve::evaluate`]; `SM_ERR` for an empty `out`.
    pub fn evaluate_into(
        &self,
        k: &Kernel,
        t: f64,
        from_left: bool,
        non_zero_tangents: bool,
        out: &mut [Vec3],
    ) -> Result<()> {
        let base = match &self.kind {
            CurveKind::BSpline(c) => Some(c),
            CurveKind::Hyperbola(c) => Some(c.base()),
            CurveKind::Ellipse(c) => Some(c.base()),
            CurveKind::Parabola(c) => Some(c.base()),
            CurveKind::Composite(_)
            | CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Line(_) => None,
        };
        if let Some(b) = base {
            return b.evaluate_into(t, from_left, non_zero_tangents, out);
        }
        let num_derivs = out
            .len()
            .checked_sub(1)
            .ok_or_else(|| SolidError::sm_err(OP_EVAL))?;
        let pv = self.evaluate(k, t, num_derivs, from_left, non_zero_tangents)?;
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = at(&pv, i);
        }
        Ok(())
    }

    /// The point at `t` (`EvaluatePoint`).
    ///
    /// # Errors
    ///
    /// Those of the kind's evaluator.
    pub fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        match &self.kind {
            CurveKind::Composite(_) => Err(base_err()),
            CurveKind::BSpline(c) => c.evaluate_point(t),
            CurveKind::Hermite(c) => Ok(c.evaluate_point(t)),
            CurveKind::Hyperbola(c) => c.base().evaluate_point(t),
            CurveKind::Ellipse(c) => c.base().evaluate_point(t),
            CurveKind::Conic(c) => Ok(c.evaluate_point(t)),
            CurveKind::Parabola(c) => c.base().evaluate_point(t),
            CurveKind::Projected(c) => c.evaluate_point(k, t),
            CurveKind::Iso(c) => c.evaluate_point(k, t),
            CurveKind::Offset(c) => c.evaluate_point(k, t),
            CurveKind::CrvInVolume(c) => c.evaluate_point(k, t),
            CurveKind::CrvOnSurf(c) => c.evaluate_point(k, t),
            CurveKind::TangentField(c) => c.evaluate_point(k, t),
            CurveKind::Line(c) => Ok(c.evaluate_point(t)),
        }
    }

    /// The minimum interior continuity and the continuity at every knot
    /// (`CalculateContinuities`).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineCurve::calculate_continuities`].
    pub fn calculate_continuities(
        &self,
        k: &Kernel,
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        match &self.kind {
            CurveKind::Composite(_) => Err(base_err()),
            CurveKind::BSpline(c) => c.calculate_continuities(k, angle_tol),
            CurveKind::Hyperbola(c) => c.base().calculate_continuities(k, angle_tol),
            CurveKind::Ellipse(c) => c.base().calculate_continuities(k, angle_tol),
            CurveKind::Hermite(c) => Ok(c.calculate_continuities()),
            CurveKind::Conic(c) => Ok(c.calculate_continuities()),
            CurveKind::Parabola(c) => c.base().calculate_continuities(k, angle_tol),
            CurveKind::Projected(c) => c.calculate_continuities(k, angle_tol),
            CurveKind::Iso(c) => c.calculate_continuities(k),
            CurveKind::Offset(c) => c.calculate_continuities(k, angle_tol),
            CurveKind::CrvInVolume(c) => c.calculate_continuities_ref(k, angle_tol),
            CurveKind::CrvOnSurf(c) => c.calculate_continuities_ref(k, angle_tol),
            // SmCrvOnSurf::CalculateContinuities on the derived object (its Evaluate)
            CurveKind::TangentField(c) => {
                c.base()
                    .calculate_continuities_as(k, angle_tol, Some(c.members()))
            }
            CurveKind::Line(c) => c.base_with_nurb()?.calculate_continuities(k, angle_tol),
        }
    }

    /// Unique knots and multiplicities, optionally limited to `interval`
    /// (`GetKnots`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline without NURB.
    pub fn knots_with_status(
        &self,
        k: &Kernel,
        interval: Option<&Extent1d>,
    ) -> Result<KnotsWithStatus> {
        match &self.kind {
            CurveKind::CrvInVolume(c) => c.knots_with_status_ref(k, interval),
            CurveKind::CrvOnSurf(c) => c.knots_with_status(k, interval),
            CurveKind::TangentField(c) => c.base().knots_with_status(k, interval),
            _ => {
                let (knots, mults) = self.knots(k, interval)?;
                Ok(KnotsWithStatus::success(knots, mults))
            }
        }
    }

    /// Unique knots and multiplicities, optionally limited to `interval`
    /// (`GetKnots`), with a failing status as the error.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline without NURB.
    pub fn knots(&self, k: &Kernel, interval: Option<&Extent1d>) -> Result<(Vec<f64>, Vec<usize>)> {
        match &self.kind {
            CurveKind::Composite(c) => c.knots(k, interval),
            CurveKind::BSpline(c) => c.knots(interval),
            CurveKind::Hyperbola(c) => c.base().knots(interval),
            CurveKind::Ellipse(c) => c.base().knots(interval),
            CurveKind::Parabola(c) => c.base().knots(interval),
            CurveKind::Hermite(c) => {
                let (k, m) = c.knots(interval);
                Ok((k, m.into_iter().map(|v| v as usize).collect()))
            }
            CurveKind::Conic(c) => {
                let (k, m) = c.knots(interval);
                Ok((k, m.into_iter().map(|v| v as usize).collect()))
            }
            CurveKind::Projected(c) => c.knots(k, interval),
            CurveKind::Iso(c) => c.knots(interval),
            CurveKind::Offset(c) => c.knots(k, interval),
            CurveKind::CrvInVolume(c) => c.knots_ref(k, interval),
            CurveKind::CrvOnSurf(c) => c.knots_ref(k, interval),
            CurveKind::TangentField(c) => c.base().knots_ref(k, interval),
            CurveKind::Line(c) => c.base_with_nurb()?.knots(interval),
        }
    }

    /// The end points and end weights (`GetEnds`; base: the natural interval end
    /// points and weights `1`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline without NURB.
    pub fn ends(&self, k: &Kernel) -> Result<Ends> {
        match &self.kind {
            CurveKind::Composite(c) => base_ends(k, c),
            CurveKind::BSpline(c) => c.ends(),
            CurveKind::Hyperbola(c) => c.base().ends(),
            CurveKind::Ellipse(c) => c.base().ends(),
            CurveKind::Hermite(c) => base_ends(k, c),
            CurveKind::Conic(c) => base_ends(k, c),
            CurveKind::Parabola(c) => c.base().ends(),
            CurveKind::Projected(c) => base_ends(k, c),
            CurveKind::Iso(c) => base_ends(k, c),
            CurveKind::Offset(c) => base_ends(k, c),
            CurveKind::CrvInVolume(c) => base_ends(k, c.as_ref()),
            CurveKind::CrvOnSurf(c) => base_ends(k, c.as_ref()),
            CurveKind::TangentField(c) => base_ends(k, c.as_ref()),
            CurveKind::Line(c) => c.ends_copy(),
        }
    }

    /// `ConvertTFromNURBSToSTEP` (base: the identity; a line maps its domains, an
    /// ellipse converts through its polar table).
    ///
    /// # Errors
    ///
    /// Those of [`Ellipse::convert_t_from_nurbs_to_step`].
    pub fn convert_t_from_nurbs_to_step(&self, k: &Kernel, nurbs_param: f64) -> Result<f64> {
        match &self.kind {
            CurveKind::Line(c) => Ok(c.convert_t_from_nurbs_to_step(nurbs_param)),
            CurveKind::Ellipse(c) => c.convert_t_from_nurbs_to_step(k, nurbs_param),
            _ => Ok(nurbs_param),
        }
    }

    /// `ConvertTFromSTEPToNURBS` (base: the identity; a line maps its domains, an
    /// ellipse converts through its polar table).
    ///
    /// # Errors
    ///
    /// Those of [`Ellipse::convert_t_from_step_to_nurbs`].
    pub fn convert_t_from_step_to_nurbs(&self, k: &Kernel, step_param: f64) -> Result<f64> {
        match &self.kind {
            CurveKind::Line(c) => Ok(c.convert_t_from_step_to_nurbs(step_param)),
            CurveKind::Ellipse(c) => c.convert_t_from_step_to_nurbs(k, step_param),
            _ => Ok(step_param),
        }
    }

    /// `ApproximateLength`: polygon length through `samples + 1` equally spaced
    /// parameters of `interval`, and for 2D curves with `want_turn_angle` the UV
    /// turning angle in degrees.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an evaluator.
    pub fn approximate_length(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        samples: usize,
        want_turn_angle: bool,
    ) -> Result<(f64, Option<f64>)> {
        match &self.kind {
            CurveKind::Line(c) => Ok(c.approximate_length(interval, want_turn_angle)),
            _ => approximate_length(k, self, interval, samples, want_turn_angle),
        }
    }

    /// `STEPInversion` (base: `SM_ERR`), without point classification.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without the inversion.
    pub fn step_inversion(&self, k: &Kernel, point: Point3) -> Result<f64> {
        match &self.kind {
            CurveKind::Hyperbola(c) => c.step_inversion(point, false).map(|(t, _)| t),
            CurveKind::Ellipse(c) => Ok(c.step_inversion(k, point, false, 0.0).0),
            CurveKind::Parabola(c) => Ok(c.step_inversion(point, false).0),
            CurveKind::Line(c) => Ok(c.step_inversion(point).0),
            CurveKind::BSpline(_)
            | CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => Err(base_err()),
        }
    }

    /// `DropPointFast` (base: `SM_ERR`; a line drops the point onto itself; the
    /// solution's object is `NULL` here, the curve has no object reference).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without the fast drop and for unsupported operations.
    pub fn drop_point_fast(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        operation: SolverOperation,
        point: Point3,
        in_pointing: Option<Vec3>,
        tol: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
    ) -> Result<Vec<Solution<ObjectRef>>> {
        match &self.kind {
            CurveKind::Line(c) => {
                c.drop_point_fast(interval, operation, point, tol, target_distance, None)
            }
            CurveKind::Ellipse(c) => c.drop_point_fast(
                k,
                interval,
                operation,
                point,
                in_pointing,
                tol,
                target_distance,
                requested,
                None,
            ),
            _ => Err(base_err()),
        }
    }

    /// `IntersectWithLine` of this curve over `interval` and `other` over
    /// `other_interval` (base: no solutions, ask the general intersector; a line
    /// intersects analytically, its solutions' objects are `NULL` here).
    ///
    /// # Errors
    ///
    /// Those of [`Line::intersect_with_line`].
    pub fn intersect_with_line(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Line,
        other_interval: &Extent1d,
        tol: f64,
    ) -> Result<LineIntersection> {
        match &self.kind {
            CurveKind::Ellipse(c) => {
                let r =
                    c.intersect_with_line(k, interval, other, other_interval, tol, None, None)?;
                Ok(LineIntersection {
                    solutions: r.solutions,
                    needs_more_intersections: r.needs_more_intersections,
                })
            }
            CurveKind::Line(c) => {
                let r = c.intersect_with_line(interval, other, other_interval, tol, None, None)?;
                Ok(LineIntersection {
                    solutions: r.solutions,
                    needs_more_intersections: r.needs_more_intersections,
                })
            }
            _ => Ok(LineIntersection {
                solutions: Vec::new(),
                needs_more_intersections: true,
            }),
        }
    }

    // ---- editing --------------------------------------------------------------------

    /// `ConvertTo2D` (base: `SM_ERR`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without the conversion; those of
    /// [`BSplineCurve::convert_to_2d`].
    pub fn convert_to_2d(&mut self, k: &Kernel) -> Result<()> {
        self.memo = CurveMemo::new();
        match &mut self.kind {
            CurveKind::BSpline(c) => c.convert_to_2d(),
            CurveKind::Hyperbola(c) => c.base.convert_to_2d(),
            CurveKind::Ellipse(c) => c.convert_to_2d(k),
            CurveKind::Parabola(c) => c.base_mut().convert_to_2d(),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => Err(base_err()),
            CurveKind::Line(c) => c.convert_to_2d_owned(self.owner),
        }
    }

    /// `EditParameterization` (base: `SM_ERR`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without the edit; those of
    /// [`BSplineCurve::edit_parameterization`].
    pub fn edit_parameterization(&mut self, k: &Kernel, new_param: &Extent1d) -> Result<()> {
        self.memo = CurveMemo::new();
        match &mut self.kind {
            CurveKind::BSpline(c) => c.edit_parameterization(new_param),
            CurveKind::Hyperbola(c) => c.base.edit_parameterization(new_param),
            CurveKind::Ellipse(c) => c.edit_parameterization(k, new_param, true, self.owner),
            CurveKind::Parabola(c) => c.base_mut().edit_parameterization(new_param),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::Composite(_) => Err(base_err()),
            CurveKind::Projected(c) => c.edit_parameterization(k, new_param),
            CurveKind::CrvInVolume(c) => c.edit_parameterization(k, new_param),
            CurveKind::CrvOnSurf(c) => c.edit_parameterization(k, new_param),
            CurveKind::TangentField(c) => c.base_mut().edit_parameterization(k, new_param),
            CurveKind::Line(c) => c.edit_parameterization_owned(self.owner, new_param),
        }
    }

    /// `ReverseParameterization` (base: `SM_ERR`): the new interval of `old_interval`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without the edit; those of
    /// [`BSplineCurve::reverse_parameterization`].
    pub fn reverse_parameterization(
        &mut self,
        k: &Kernel,
        old_interval: &Extent1d,
    ) -> Result<Extent1d> {
        let owner = self.owner;
        self.memo = CurveMemo::new();
        match &mut self.kind {
            CurveKind::BSpline(c) => c.reverse_parameterization(old_interval),
            CurveKind::Hyperbola(c) => c.reverse_parameterization(old_interval, owner),
            CurveKind::Ellipse(c) => c.reverse_parameterization(k, old_interval, owner),
            CurveKind::Parabola(c) => c.reverse_parameterization(old_interval, owner),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Iso(_)
            | CurveKind::Composite(_) => Err(base_err()),
            CurveKind::Projected(c) => c.reverse_parameterization(k, old_interval),
            CurveKind::Offset(c) => Ok(c.reverse_parameterization(old_interval)),
            CurveKind::CrvInVolume(c) => c.reverse_parameterization(k, old_interval),
            CurveKind::CrvOnSurf(c) => c.reverse_parameterization(k, old_interval),
            CurveKind::TangentField(c) => c.base_mut().reverse_parameterization(k, old_interval),
            CurveKind::Line(c) => Ok(c.reverse_parameterization(old_interval)),
        }
    }

    /// `Transform` (base: `SM_ERR`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without the edit; those of [`BSplineCurve::transform`].
    /// A volume append is refused if it would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_at_depth(k, rotate_n_move, scale, 0)
    }

    pub(crate) fn transform_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        self.memo = CurveMemo::new();
        match &mut self.kind {
            CurveKind::Composite(c) => c.transform_at_depth(k, rotate_n_move, scale, depth),
            CurveKind::BSpline(c) => c.transform(rotate_n_move, scale),
            CurveKind::Hyperbola(c) => c.transform(rotate_n_move, scale),
            CurveKind::Ellipse(c) => c.transform(k, rotate_n_move, scale),
            CurveKind::Parabola(c) => c.transform(rotate_n_move, scale),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_) => Err(base_err()),
            CurveKind::Projected(c) => c.transform_at_depth(k, rotate_n_move, scale, depth),
            CurveKind::CrvInVolume(c) => c.transform_at_depth(rotate_n_move, scale, depth),
            CurveKind::CrvOnSurf(c) => c.transform_at_depth(k, rotate_n_move, scale, depth),
            CurveKind::TangentField(c) => {
                c.base_mut()
                    .transform_at_depth(k, rotate_n_move, scale, depth)
            }
            CurveKind::Line(c) => c.transform(rotate_n_move, scale),
        }
    }

    /// `Trim` (base: `SM_ERR`); `trim_interval` may be snapped.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without the edit; those of [`BSplineCurve::trim`].
    pub fn trim(&mut self, k: &Kernel, trim_interval: &mut Extent1d, mode: TrimMode) -> Result<()> {
        self.trim_with_zone_tol(
            k,
            trim_interval,
            mode,
            crate::geom::tol::zone_tol_3d(k, crate::geom::tol::SmallTopoType::NormalSize),
        )
    }

    /// `Trim` with the caller-resolved owning object's 3D zone tolerance.
    pub(crate) fn trim_with_zone_tol(
        &mut self,
        k: &Kernel,
        trim_interval: &mut Extent1d,
        mode: TrimMode,
        zone_tol_3d: f64,
    ) -> Result<()> {
        self.memo = CurveMemo::new();
        match &mut self.kind {
            CurveKind::BSpline(c) => c.trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d),
            CurveKind::Hyperbola(c) => {
                c.base
                    .trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d)
            }
            CurveKind::Ellipse(c) => c.trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d),
            CurveKind::Parabola(c) => {
                c.base_mut()
                    .trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d)
            }
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Iso(_)
            | CurveKind::Composite(_) => Err(base_err()),
            CurveKind::Projected(c) => c.trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d),
            CurveKind::Offset(c) => c.trim(trim_interval, mode == TrimMode::IntervalOnly),
            CurveKind::CrvInVolume(c) => c.trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d),
            CurveKind::CrvOnSurf(c) => c.trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d),
            CurveKind::TangentField(c) => {
                c.base_mut()
                    .trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d)
            }
            CurveKind::Line(c) => c.trim_owned(self.owner, trim_interval, mode),
        }
    }

    /// `Copy` (base: `SM_ERR`): an exact copy without owner.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without `Copy` (Hermite curves).
    pub fn copy(&self, k: &Kernel) -> Result<Self> {
        match &self.kind {
            CurveKind::Composite(c) => c.copy(k).map(Self::from),
            CurveKind::BSpline(c) => Ok(Self::from(c.copy())),
            CurveKind::Ellipse(c) => c.copy(k).map(Self::from),
            CurveKind::Hyperbola(c) => Ok(Self::from(c.copy())),
            CurveKind::Parabola(c) => Ok(Self::from(c.copy())),
            CurveKind::Hermite(_) | CurveKind::Conic(_) | CurveKind::Iso(_) => Err(base_err()),
            CurveKind::Projected(c) => c.copy(k).map(Self::from),
            CurveKind::Offset(c) => c.copy().map(Self::from),
            CurveKind::CrvInVolume(c) => c.copy().map(Self::from),
            CurveKind::CrvOnSurf(c) => c.copy(k).map(Self::from),
            // SmCrvOnSurf::Copy (not overridden): `new SmCrvOnSurf(*this)`, the base only
            CurveKind::TangentField(c) => c.base().copy(k).map(Self::from),
            CurveKind::Line(c) => Ok(Self::from(c.copy())),
        }
    }

    /// `CreateMirrorCurve` (base: `SM_ERR`): a copy mirrored about the XY plane of
    /// `mirror_plane`.
    ///
    /// Every `SmBSplineCurve`-derived kind (line, ellipse and circle, parabola,
    /// hyperbola) inherits the B-spline override: the result is a plain B-spline curve
    /// over the NURB the kind makes first (on a copy here). A curve on a surface and a
    /// curve in a volume mirror what they wrap.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for kinds without the override (Hermite, projected, iso, offset,
    /// conic and composite curves); those of the NURB construction and of the override.
    pub fn create_mirror_curve(&self, k: &Kernel, mirror_plane: &Axis2Placement) -> Result<Self> {
        match &self.kind {
            CurveKind::BSpline(c) => c.create_mirror_curve(mirror_plane),
            CurveKind::Line(c) => {
                if c.base().gw_nurb().is_some() {
                    c.base().create_mirror_curve(mirror_plane)
                } else {
                    let mut n = c.copy();
                    n.make_nurb()?;
                    n.base().create_mirror_curve(mirror_plane)
                }
            }
            CurveKind::Ellipse(c) => {
                if c.base().gw_nurb().is_some() {
                    c.base().create_mirror_curve(mirror_plane)
                } else {
                    let mut n = c.copy(k)?;
                    n.make_nurb(k)?;
                    n.base().create_mirror_curve(mirror_plane)
                }
            }
            CurveKind::Parabola(c) => {
                if c.base().gw_nurb().is_some() {
                    c.base().create_mirror_curve(mirror_plane)
                } else {
                    let mut n = c.copy();
                    n.make_nurb(None)?;
                    n.base().create_mirror_curve(mirror_plane)
                }
            }
            CurveKind::Hyperbola(c) => {
                if c.base().gw_nurb().is_some() {
                    c.base().create_mirror_curve(mirror_plane)
                } else {
                    let mut n = c.copy();
                    n.make_nurb(None)?;
                    n.base().create_mirror_curve(mirror_plane)
                }
            }
            CurveKind::CrvInVolume(c) => {
                return c.create_mirror_curve(mirror_plane).map(Self::from)
            }
            CurveKind::CrvOnSurf(c) => {
                return c.create_mirror_curve(k, mirror_plane).map(Self::from);
            }
            // `SmTangentField` inherits `SmCrvOnSurf::CreateMirrorCurve`, which builds a plain
            // `SmCrvOnSurf` on the mirrored surface.
            CurveKind::TangentField(c) => {
                return c
                    .base()
                    .create_mirror_curve(k, mirror_plane)
                    .map(Self::from);
            }
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::Composite(_) => return Err(base_err()),
        }
        .map(Self::from)
    }

    /// `CreateAnalyticCurve` (base: no analytic curve): an analytic equivalent
    /// over `interval`, if any.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineCurve::create_analytic_curve`].
    pub fn create_analytic_curve(&self, k: &Kernel, interval: &Extent1d) -> Result<Option<Self>> {
        match &self.kind {
            CurveKind::BSpline(c) => match c.create_analytic_curve(k, interval)? {
                None => Ok(None),
                Some(AnalyticCurve::Line(l)) => Ok(Some(Self::from(l))),
                Some(AnalyticCurve::Circle(e)) => Ok(Some(Self::from(*e))),
            },
            CurveKind::Hyperbola(c) => match c.base().create_analytic_curve(k, interval)? {
                None => Ok(None),
                Some(AnalyticCurve::Line(l)) => Ok(Some(Self::from(l))),
                Some(AnalyticCurve::Circle(e)) => Ok(Some(Self::from(*e))),
            },
            CurveKind::Ellipse(c) => match c.base().create_analytic_curve(k, interval)? {
                None => Ok(None),
                Some(AnalyticCurve::Line(l)) => Ok(Some(Self::from(l))),
                Some(AnalyticCurve::Circle(e)) => Ok(Some(Self::from(*e))),
            },
            CurveKind::Parabola(c) => match c.base().create_analytic_curve(k, interval)? {
                None => Ok(None),
                Some(AnalyticCurve::Line(l)) => Ok(Some(Self::from(l))),
                Some(AnalyticCurve::Circle(e)) => Ok(Some(Self::from(*e))),
            },
            CurveKind::Line(c) => Ok(c.clone().create_analytic_curve(interval)?.map(Self::from)),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => Ok(None),
        }
    }

    /// `CreateBySplitCurve`: trims this curve to `[min, split]` and returns a copy
    /// trimmed to `[split, max]`. The trims' statuses are ignored, as upstream.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a split parameter outside the natural interval or within
    /// `100 * EFF_ZERO * (1 + |end|)` of an end, or a kind without `Copy`;
    /// [`SolidError::Unported`] from a trim.
    pub fn create_by_split_curve(&mut self, k: &Kernel, split_param: f64) -> Result<Self> {
        // make sure splitParam is within the parameter range
        let mut ivl = self.natural_interval();
        if !ivl.contains_value(split_param, 0.0) {
            return Err(SolidError::sm_err(OP_SPLIT));
        }

        // limit a valid splitParam so that it can't be on the end or very close to the end
        let min = ivl.min();
        let max = ivl.max();
        let tol_min = EFF_ZERO * 100.0 * (1.0 + min.abs());
        let tol_max = EFF_ZERO * 100.0 * (1.0 + max.abs());
        if split_param < min + tol_min {
            return Err(SolidError::sm_err(OP_SPLIT));
        }
        if split_param > max - tol_max {
            return Err(SolidError::sm_err(OP_SPLIT));
        }

        let mut copy = self.copy(k)?;

        let mut ivl2 = ivl;
        ivl2.set_min_max(split_param, ivl2.max())?;
        ivl.set_min_max(ivl.min(), split_param)?;
        ignore_status(self.trim(k, &mut ivl, TrimMode::Notify))?;
        ignore_status(copy.trim(k, &mut ivl2, TrimMode::Notify))?;
        Ok(copy)
    }

    /// `SurfaceMaxDistanceBetween`: the largest gap between this curve over
    /// `interval` and `surface` over `domain`, sampled at `num_samples` parameters
    /// (`0`: a dense sample refined by a local solve), stopping early once a gap
    /// exceeds `max_needed`.
    ///
    /// # Errors
    ///
    /// Those of the surface queries; [`SolidError::Unported`] from
    /// `SmSurface::DropPoint` and `LocalCurveSolve`.
    pub fn surface_max_distance_between<S: SurfaceEvaluate>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        surface: &S,
        domain: &Extent2d,
        num_samples: usize,
        max_needed: Option<f64>,
    ) -> Result<SurfaceGap> {
        let mut num_samples = num_samples;

        // special case line/plane check
        let mut do_precise = false;
        let is_plane = surface_is_plane(surface)?;
        if is_plane && self.degree()? == 1 && self.number_natural_knots(k)? == 4 {
            do_precise = false;
            num_samples = 2;
        } else if num_samples == 0 {
            // Do a very large sample and adjust it upward to compensate for
            // cases where there are a large number of knots.
            do_precise = true;
            let num_knots1 = self.number_natural_knots(k)?;
            let num_knots2 = surface_knot_count(k, surface, false)?;
            let num_knots3 = surface_knot_count(k, surface, true)?;
            num_samples = 22 + num_knots1 + num_knots2 + num_knots3;
        }

        let mut max_dist = 0.0;
        let mut guess_uv: Option<Point2> = None;

        // Init Max parameters to something.
        let mut max_param = interval.mid();
        let mut max_uv = domain.mid();

        // Sample this curve uniformly in parameter space.
        for ii in 0..num_samples {
            let per = ii as f64 / (num_samples as f64 - 1.0);
            let mut param = interval.evaluate(per);

            let crv_pt = ignored(self.evaluate_point(k, param))?.unwrap_or(Vec3::UNDEF);
            let mut drop = surface_drop_point(k, surface, crv_pt, domain, guess_uv)?;

            // If drop failed for first or last point, try moving a small distance from the end.
            if drop.is_none() && ii == 0 {
                param = interval.evaluate(0.001);
                let crv_pt = self.evaluate_point(k, param)?;
                drop = surface_drop_point(k, surface, crv_pt, domain, guess_uv)?;
            }
            if drop.is_none() && ii + 1 == num_samples {
                param = interval.evaluate(0.999);
                let crv_pt = self.evaluate_point(k, param)?;
                drop = surface_drop_point(k, surface, crv_pt, domain, guess_uv)?;
            }

            if let Some((uv, dist)) = drop {
                if dist > max_dist {
                    max_dist = dist;
                    max_param = param;
                    max_uv = uv;
                }

                // When given a MaxDistance limit - and the limit is exceeded
                if max_needed.is_some_and(|m| max_dist > m) {
                    return Ok(SurfaceGap {
                        max_distance: max_dist,
                        curve_param: max_param,
                        surface_param: max_uv,
                    });
                }

                // Set surface guess for next iteration.
                guess_uv = Some(uv);
            }
        }

        // Done sampling the curve. If we're doing precise calculation, call a solver
        // with our best guesses, unless the curve appears to be coincident with the surface.
        let crv_pt = self.evaluate_point(k, max_param)?;
        let scaled_zero = EFF_ZERO * (1.0 + crv_pt.max_dimension());
        if do_precise && max_dist > scaled_zero {
            // Find the gap maximum near the largest sampled gap point.
            if let Some((param, uv)) =
                surface.local_curve_maximize(k, self, domain, interval, max_uv, max_param)?
            {
                let crv_pt = self.evaluate_point(k, param)?;
                let srf_pt = surface.evaluate_point(k, uv)?;
                let dist = crv_pt.distance(srf_pt);

                // save largest gap seen
                if dist > max_dist {
                    max_dist = dist;
                    max_param = param;
                    max_uv = uv;
                }
            }
        }

        Ok(SurfaceGap {
            max_distance: max_dist,
            curve_param: max_param,
            surface_param: max_uv,
        })
    }

    // ---- persistence ----------------------------------------------------------------

    /// Reads a curve of type `curve_type` (image dimension `dim`, database version
    /// `version`) from `db` (`ReadFromDB`, which dispatches on the type).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a type that is not a curve; [`SolidError::Unported`] for the curve
    /// types not ported yet; those of the kind's reader.
    pub fn read_from_db(
        k: &Kernel,
        curve_type: i64,
        db: &mut DatabaseReader,
        dim: u64,
        version: u64,
    ) -> Result<Self> {
        // Bound nesting (a projected/offset/... curve reads its base curve through
        // here): upstream recurses without limit on crafted input.
        db.enter_nested(OP_READ)?;
        // pass the call along to the appropriate derived type
        let result = match curve_type {
            BSPLINE_CURVE_TYPE => BSplineCurve::read_from_db(db, dim, version).map(Self::from),
            ELLIPSE_TYPE => Ellipse::read_from_db(k, db, dim, version, false).map(Self::from),
            CIRCLE_TYPE => Ellipse::read_from_db(k, db, dim, version, true).map(Self::from),
            HYPERBOLA_TYPE => Hyperbola::read_from_db(db, dim, version).map(Self::from),
            LINE_TYPE => Line::read_from_db(db, dim, version).map(Self::from),
            PARABOLA_TYPE => Parabola::read_from_db(db, dim, version).map(Self::from),
            COMPOSITE_CURVE_TYPE => {
                CompositeCurve::read_from_db(k, db, dim, version).map(Self::from)
            }
            CONIC_TYPE => Conic::read_from_db(db).map(Self::from),
            CRV_ON_SURF_TYPE => CrvOnSurf::read_from_db(k, db, version).map(Self::from),
            CRV_IN_VOLUME_TYPE => CrvInVolume::read_from_db(k, db, version).map(Self::from),
            TANGENT_FIELD_TYPE => {
                TangentFieldCurve::read_from_db(k, db, dim, version).map(Self::from)
            }
            HERMITE_CURVE_TYPE => HermiteCurve::read_from_db(db).map(Self::from),
            ISO_CURVE_TYPE => IsoCurve::read_from_db(k, db, version).map(Self::from),
            OFFSET_CURVE_TYPE => OffsetCurve::read_from_db(k, db, version).map(Self::from),
            PROJECTED_CURVE_TYPE => ProjectedCurve::read_from_db(k, db, version).map(Self::from),
            _ => Err(SolidError::sm_err(OP_READ)),
        };
        db.leave_nested();
        result
    }
}

/// `SmCurve::Notify`: drops the curve's object cache before an edit and on
/// destruction, then passes the notification to `SmAObject::Notify`.
///
/// # Errors
///
/// Those of `SmCacheMgr::DeleteObjectsCache` and `SmAObject::Notify`.
pub fn notify<T>(
    caches: &mut CacheStore<T>,
    object: &mut AObject,
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
) -> Result<()> {
    match operation {
        NotifyOperation::PreEdit | NotifyOperation::Destruction => {
            // delete the object cache
            a_object::delete_objects_cache(caches, object)?;
        }
        // SM_NO_UNKNOWN only prints a message; every other event has nothing to do here
        _ => {}
    }

    // Propagate notification up hierarchy
    a_object::notify(attributes, context, operation, this, data1, data2, data3)
}

// ---- base-class numerics --------------------------------------------------------------

/// `SmCurve::GetEnds`: the points at the natural interval ends and weights `1`.
/// Evaluation statuses are ignored, as upstream (an unevaluated end is
/// [`Vec3::UNDEF`]).
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluator.
pub fn base_ends<C: CurveEvaluate + ?Sized>(k: &Kernel, curve: &C) -> Result<Ends> {
    let ivl = curve.natural_interval();
    let point = |t: f64| -> Result<Point3> {
        Ok(ignored(curve.evaluate_point(k, t))?.unwrap_or(Vec3::UNDEF))
    };
    Ok(Ends {
        start: point(ivl.min())?,
        end: point(ivl.max())?,
        start_weight: 1.0,
        end_weight: 1.0,
    })
}

/// Evaluation whose failure upstream ignores: `None` keeps the caller's previous
/// values; [`SolidError::Unported`] is propagated.
fn eval_ignored<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    t: f64,
    num_derivs: usize,
    non_zero_tangents: bool,
) -> Result<Option<Vec<Vec3>>> {
    ignored(curve.evaluate(k, t, num_derivs, true, non_zero_tangents))
}

/// `SmCurve::ApproximateLength`: the length of the polygon through the curve
/// points at `samples + 1` equally spaced parameters of `interval` and, for a 2D
/// curve with `want_turn_angle`, the turning angle of its tangent in degrees
/// (`pOptUVTurnAngDeg`; `None` otherwise). Evaluation statuses are ignored, as
/// upstream.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluator.
pub fn approximate_length<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    interval: &Extent1d,
    samples: usize,
    want_turn_angle: bool,
) -> Result<(f64, Option<f64>)> {
    // init output
    let mut ret = 0.0;

    // switch for turning angles
    if curve.dim() == 2 && want_turn_angle {
        let z = Vec3::new(0.0, 0.0, 1.0);
        let mut last = [Vec3::UNDEF; 2];
        let mut curr = [Vec3::UNDEF; 2];
        let mut turn_ang_rad = 0.0;

        // Get Start and end Points
        if let Some(v) = eval_ignored(k, curve, interval.min(), 1, true)? {
            last = [at(&v, 0), at(&v, 1)];
        }
        if let Some(v) = eval_ignored(k, curve, interval.max(), 1, true)? {
            curr = [at(&v, 0), at(&v, 1)];
        }

        // Get base of ActTurnAngRad = BaseTurnAngRad +- N*2PI
        let base_turn_ang_rad = z.ccw_angle_between(last[1], curr[1])?;

        // for every sample point - assume enough sample points that every turn is less than 180 degrees
        for ii in 1..=samples {
            // normalized parameter
            let param = if ii < samples {
                interval.evaluate(ii as f64 / samples as f64)
            } else {
                interval.max()
            };

            // Evaluate and accumulate distance
            if let Some(v) = eval_ignored(k, curve, param, 1, true)? {
                curr = [at(&v, 0), at(&v, 1)];
            }
            ret += curr[0].distance(last[0]);

            // accumulate turn angle
            let this_turn_ang_rad = z.ccw_angle_between(last[1], curr[1])?;
            turn_ang_rad += this_turn_ang_rad;

            // for next iter
            last = curr;
        }

        // Get N of ActTurnAngRad = BaseTurnAngRad +- N*2PI
        let n = ((turn_ang_rad - base_turn_ang_rad) / (2.0 * PI)).round();

        // set TurnAngDeg output
        let turn_deg = (base_turn_ang_rad + n * 2.0 * PI) * (180.0 / PI);
        Ok((ret, Some(turn_deg)))
    } else {
        let point = |t: f64, prev: Point3| -> Result<Point3> {
            Ok(ignored(curve.evaluate_point(k, t))?.unwrap_or(prev))
        };

        // Get Start Point
        let mut last = point(interval.min(), Vec3::UNDEF)?;
        let mut curr = Vec3::UNDEF;

        // for every sample point
        for ii in 1..=samples {
            // normalized parameter
            let param = if ii < samples {
                interval.evaluate(ii as f64 / samples as f64)
            } else {
                interval.max()
            };

            // evaluate and accumulate distance
            curr = point(param, curr)?;
            ret += curr.distance(last);

            // next iter
            last = curr;
        }
        Ok((ret, None))
    }
}

/// `SmCurve::EvaluateSTEP`: the NURBS evaluation with exact tangents.
///
/// # Errors
///
/// Those of the evaluator.
pub fn evaluate_step<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    step_param: f64,
    num_derivs: usize,
    from_left: bool,
) -> Result<Vec<Vec3>> {
    // pass the call along
    curve.evaluate(k, step_param, num_derivs, from_left, false)
}

/// `SmCurve::EvaluateGeometric`: `[position, first derivative, curvature vector,
/// torsion vector]` up to `num_vectors` (at most 3).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for `num_vectors > 3` (an upstream assert); those of
/// the evaluator and of [`frenet_from_derivatives`].
pub fn evaluate_geometric<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    param: f64,
    num_vectors: usize,
    from_left: bool,
) -> Result<Vec<Vec3>> {
    if num_vectors > 3 {
        return Err(SolidError::InvalidInput {
            operation: OP_GEOM,
            reason: "at most 3 geometric vectors",
        });
    }
    let pvvv = curve.evaluate(k, param, num_vectors, from_left, false)?;
    // pass the call along
    frenet_from_derivatives(&pvvv, num_vectors)
}

/// Result of [`evaluate_continuity`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveContinuity {
    /// The continuity found.
    pub continuity: ContinuityType,
    /// `pOptThisEval`: position, first and second derivative of this curve.
    pub this_eval: [Vec3; 3],
    /// `pOptOtherEval`: position, first and second derivative of the other curve.
    pub other_eval: [Vec3; 3],
}

/// `SmCurve::EvaluateContinuity`: the continuity between `this` at `this_param`
/// and `other` at `other_param` (each from the left or right), with
/// `continuity_angle_tol` degrees for G1 (upstream default `SM_CONTINUITY_ANGLE`,
/// 0.5).
///
/// # Errors
///
/// Those of the evaluators and of [`evaluate_curve_continuity_with`].
pub fn evaluate_continuity<A, B>(
    k: &Kernel,
    this: &A,
    this_param: f64,
    this_from_left: bool,
    other: &B,
    other_param: f64,
    other_from_left: bool,
    continuity_angle_tol: f64,
) -> Result<CurveContinuity>
where
    A: CurveEvaluate + ?Sized,
    B: CurveEvaluate + ?Sized,
{
    // evaluate the curves (exact tangent values)
    let left = this.evaluate(k, this_param, 3, this_from_left, false)?;
    let right = other.evaluate(k, other_param, 3, other_from_left, false)?;

    let eval_left = CurveEval {
        position: at(&left, 0),
        first: at(&left, 1),
        second: at(&left, 2),
        third: Some(at(&left, 3)),
    };
    let eval_right = CurveEval {
        position: at(&right, 0),
        first: at(&right, 1),
        second: at(&right, 2),
        third: Some(at(&right, 3)),
    };
    let continuity = evaluate_curve_continuity_with(
        &eval_left,
        &eval_right,
        continuity_angle_tol,
        &ContinuityOptions {
            geometric_g2: k.fixes.geometric_g2_continuity,
        },
    )?;

    // set outputs
    Ok(CurveContinuity {
        continuity,
        this_eval: [at(&left, 0), at(&left, 1), at(&left, 2)],
        other_eval: [at(&right, 0), at(&right, 1), at(&right, 2)],
    })
}

/// Result of [`euclidian_step_off`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepOff {
    /// `SM_SUCCESS` (`true`) or the `SM_ERR` of leaving the interval or not
    /// converging (`false`).
    pub converged: bool,
    /// `bClampped`: stopped because the search left the interval.
    pub clamped: bool,
    /// `rdEndParam`: the parameter found (the best one so far on failure).
    pub end_param: f64,
}

/// `SmCurve::EuclidianStepOff`: the parameter whose point is `step_off_distance`
/// (chord length) from the point at `start_param`, searching forward
/// (`forward`, `SM_OT_SAME`) or backward within `interval`, to relative accuracy
/// `tol_3d`.
///
/// # Errors
///
/// Those of the evaluator.
pub fn euclidian_step_off<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    interval: &Extent1d,
    start_param: f64,
    forward: bool,
    step_off_distance: f64,
    tol_3d: f64,
) -> Result<StepOff> {
    // init outputs
    let mut out = StepOff {
        converged: true,
        clamped: false,
        end_param: start_param,
    };

    // no work - spacing equal to zero
    if step_off_distance.abs() < EFF_ZERO {
        return Ok(out);
    }

    // get start evaluation point
    let pv = curve.evaluate(k, start_param, 1, true, true)?;
    let sign = if forward { 1.0 } else { -1.0 };

    // guess param value expected to be dStepOffDistance from initial point
    let mut end_param = start_param + sign * step_off_distance / at(&pv, 1).length();

    let mut count = 0u32;
    let mut num_outside = 0u32;
    while count < 30 {
        count += 1;

        // clamp guesses outside of the interval range
        if !interval.contains_value(end_param, 0.0) {
            // count number of outside guesses - quit if count exceeds 3
            num_outside += 1;
            end_param = interval.clamp_value(end_param);
            if num_outside > 3 {
                out.converged = false;
                out.clamped = true;
                out.end_param = end_param;
                return Ok(out);
            }
        }

        // check the guessed distance
        let pv2 = curve.evaluate(k, end_param, 1, true, true)?;
        let dist = at(&pv2, 0).distance(at(&pv, 0));

        // success stop: distance is less than tolerance
        if (dist / step_off_distance - 1.0).abs() < tol_3d {
            break;
        }

        // get a new guess
        let delta = (step_off_distance - dist) / at(&pv2, 1).length();
        end_param += sign * delta;

        // failure stop: failed to converge within 30 reps
        if count == 30 {
            out.converged = false;
            out.end_param = end_param;
            return Ok(out);
        }
    }

    out.end_param = end_param;
    Ok(out)
}

/// Result of [`Curve::surface_max_distance_between`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGap {
    /// `rdMaxDistanceFound`: the largest gap seen.
    pub max_distance: f64,
    /// `pOptCurveT`: curve parameter of that gap.
    pub curve_param: f64,
    /// `pOptSurfParam`: surface parameters of that gap.
    pub surface_param: Point2,
}

/// `SM_CAST_NONNULL_PTR(SmPlane, &crSurface) != NULL`.
fn surface_is_plane<S: SurfaceEvaluate>(surface: &S) -> Result<bool> {
    surface.is_kind_of(crate::surface::surface::p2::PLANE_TYPE)
}

/// The number of unique `v` (or `u`) knots of `surface` (`SmSurface::GetKnots`).
fn surface_knot_count<S: SurfaceEvaluate>(k: &Kernel, surface: &S, v: bool) -> Result<usize> {
    let param = if v {
        SurfParamType::V
    } else {
        SurfParamType::U
    };
    Ok(surface.knots(k, param, None)?.0.len())
}

/// `SmSurface::DropPoint`: the surface parameters and distance of the drop, `None`
/// when it failed.
fn surface_drop_point<S: SurfaceEvaluate>(
    k: &Kernel,
    surface: &S,
    point: Point3,
    domain: &Extent2d,
    guess: Option<Point2>,
) -> Result<Option<(Point2, f64)>> {
    surface.drop_point_minimize(k, point, domain, guess)
}

// ---- file-static helpers --------------------------------------------------------------

/// `sm_ConvertToParameterDelta`: the parameter step along `tangent` whose
/// projection matches `delta` (`0` for a zero tangent).
#[must_use]
pub fn convert_to_parameter_delta(tangent: Vec3, delta: Vec3) -> f64 {
    let tan_len = tangent.length();
    if tan_len < EFF_ZERO {
        return 0.0;
    }
    let unit_tan = tangent / tan_len;
    let dot = unit_tan.dot(delta);
    dot / tan_len
}

/// `sm_PushPoint`: appends `[t, pos.x, pos.y, pos.z, tan.x, tan.y, tan.z]` to
/// `stack`.
pub fn push_point(t: f64, pv: [Vec3; 2], stack: &mut Vec<f64>) {
    let [pos, tan] = pv;
    stack.extend_from_slice(&[t, pos.x, pos.y, pos.z, tan.x, tan.y, tan.z]);
}

/// `sm_PopStack`: removes the last `[t, pos, tan]` entry of `stack`.
///
/// # Errors
///
/// `SM_ERR` when the stack holds fewer than 7 values.
pub fn pop_stack(stack: &mut Vec<f64>) -> Result<(f64, [Vec3; 2])> {
    let entry = stack_look(stack)?;
    stack.truncate(stack.len() - 7);
    Ok(entry)
}

/// `sm_StackLook`: the last `[t, pos, tan]` entry of `stack`, left in place.
///
/// # Errors
///
/// `SM_ERR` when the stack holds fewer than 7 values.
pub fn stack_look(stack: &[f64]) -> Result<(f64, [Vec3; 2])> {
    let Some(&[t, x0, y0, z0, x1, y1, z1]) = stack.last_chunk() else {
        return Err(SolidError::sm_err(OP_STACK));
    };
    Ok((t, [Vec3::new(x0, y0, z0), Vec3::new(x1, y1, z1)]))
}

/// `sm_AddToCurve`: appends the cubic Bezier span of the Hermite data
/// `last_pv`/`curr_pv` over `[last_t, curr_t]` to the piecewise Bezier control
/// polygon `cntrl_poly` and knot vector `knots`.
///
/// # Errors
///
/// `SM_ERR` (arrays unchanged) when the span does not continue the existing curve
/// in parameter or position.
pub fn add_to_curve(
    last_pv: [Vec3; 2],
    curr_pv: [Vec3; 2],
    last_t: f64,
    curr_t: f64,
    cntrl_poly: &mut Vec<Point3>,
    knots: &mut Vec<f64>,
) -> Result<()> {
    // locals
    let scale = curr_t - last_t;
    let p1 = last_pv[0];
    let p4 = curr_pv[0];
    let d1 = last_pv[1] * scale;
    let d2 = curr_pv[1] * scale;

    // convert Hermite params into a Bezier cubic polygon
    let p2 = p1 + d1 / 3.0;
    let p3 = p4 - d2 / 3.0;

    // when adding intervals to an existing Curve start - check for continuity
    if let (Some(&last_knot), Some(&last_pt)) = (knots.last(), cntrl_poly.last()) {
        // if curve is discontinuous in either param or XYZ space
        let knot_scale = 100.0 * (1.0 + last_t.abs());
        let pnt_scale = 100.0 * (1.0 + p1.max_dimension());
        if (last_knot - last_t).abs() > knot_scale * EFF_ZERO
            || p1.distance(last_pt) > pnt_scale * EFF_ZERO
        {
            // signal an error for now
            return Err(SolidError::sm_err(OP_ADD));
        }
    } else if !knots.is_empty() {
        // knots without control points: upstream reads past the polygon
        return Err(SolidError::InvalidInput {
            operation: OP_ADD,
            reason: "knots without control points",
        });
    }

    // when starting the curve
    if knots.is_empty() {
        // add the 1st control polygon point and start param
        knots.push(last_t);
        cntrl_poly.push(p1);
    }

    // add end param value
    knots.push(curr_t);

    // add middle and end control polygon points
    cntrl_poly.push(p2);
    cntrl_poly.push(p3);
    cntrl_poly.push(p4);
    Ok(())
}

/// `sm_FlushCrv`: builds the curve of a piecewise cubic Bezier control polygon and
/// its knots (one per span end). With `create_analytics`, a polygon of more than 5
/// points whose spans lie on one circle (to `10000 * EFF_ZERO` scaled) becomes an
/// exact quadratic arc when that arc matches the polygon's span ends within `tol`
/// (the arrays are then left as they are); otherwise the cubic B-spline is built
/// and both arrays are cleared. `None` (nothing built) for empty knots.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when the polygon does not have `3 * (knots - 1) + 1`
/// points (upstream reads past it); those of the vector, frame, arc and canonical
/// constructions (`SER`).
pub fn flush_crv(
    tol: f64,
    cntrl_poly: &mut Vec<Point3>,
    knots: &mut Vec<f64>,
    create_analytics: bool,
) -> Result<Option<BSplineCurve>> {
    let (Some(&first_knot), Some(&last_knot)) = (knots.first(), knots.last()) else {
        return Ok(None);
    };
    if cntrl_poly.len() != 3 * (knots.len() - 1) + 1 {
        return Err(SolidError::InvalidInput {
            operation: OP_FLUSH,
            reason: "control polygon size does not match the knots",
        });
    }
    let poly = |i: usize| cntrl_poly.get(i).copied().unwrap_or(Vec3::UNDEF);

    // Test to see if points are on an arc.
    if cntrl_poly.len() > 5 && create_analytics {
        let p1 = poly(0);
        let v1 = poly(1) - p1;
        let p2 = poly(3);
        let v2 = p2 - poly(2);
        let mut norm = v1.cross(v2);
        if norm.length_squared() > EFF_ZERO_SQ {
            norm = norm.unitize()?;
            let line_vec1 = norm.cross(v1);
            let line_vec2 = norm.cross(v2);
            let mut is_circle = true;
            let mut total_angle = 0.0;
            if let Ok((param1, _param2)) = line_line_closest_point(p1, line_vec1, p2, line_vec2) {
                let cent = p1 + param1 * line_vec1;
                let radius = cent.distance(p1);
                let last = poly(cntrl_poly.len() - 1);
                let mut size = p1.distance(p2) + p2.distance(last);
                size = max2(size, EFF_ZERO * (1.0 + p2.max_dimension()));
                let analytic_tol =
                    10000.0 * EFF_ZERO * (1.0 + p1.max_dimension() + p2.max_dimension());
                let mut ii = 3;
                while ii < cntrl_poly.len() {
                    let test_radius = cent.distance(poly(ii));
                    if (test_radius - radius).abs() > analytic_tol || radius > 100.0 * size {
                        // Avoid Making Really big circles.
                        is_circle = false;
                        break;
                    }
                    let vec1 = poly(ii - 3) - cent;
                    let vec2 = poly(ii) - cent;
                    if vec1.length_squared() > EFF_ZERO_SQ && vec2.length_squared() > EFF_ZERO_SQ {
                        total_angle += vec1.angle_between(vec2)?;
                    }
                    ii += 3;
                }

                if is_circle {
                    let x_axis = (poly(0) - cent).unitize()?;
                    let reference = poly(3) - cent;
                    let cross = x_axis.cross(reference);
                    let mut y_axis = cross.cross(x_axis);
                    if y_axis.length_squared() > EFF_ZERO_SQ {
                        y_axis = y_axis.unitize()?;
                        let mut ref_frame = Axis2Placement::IDENTITY;
                        ref_frame.set_canonical(cent, x_axis, y_axis)?;
                        if total_angle > PI * 2.0 {
                            total_angle = PI * 2.0;
                        }
                        total_angle *= 180.0 / PI;
                        let mut arc = BSplineCurve::create_circle_segment(
                            3,
                            &ref_frame,
                            radius,
                            0.0,
                            total_angle,
                            NurbCircleParam::Quadratic,
                        )?;
                        let ivl = Extent1d::from_min_max(first_knot, last_knot);
                        // status ignored upstream
                        ignore_status(arc.edit_parameterization(&ivl))?;
                        // Check whether the approximation is accurate.
                        let mut good_approximation = true;
                        let tol_sq = tol * tol;
                        for (jj, &param) in knots.iter().enumerate() {
                            let pnt = poly(jj * 3);
                            let test_pnt = arc.evaluate_point(param)?;
                            let dist_sq = test_pnt.distance_squared(pnt);
                            if dist_sq > tol_sq {
                                good_approximation = false;
                            }
                        }
                        if good_approximation {
                            return Ok(Some(arc));
                        }
                    }
                }
            }
        }
    }

    let k = knots.len();
    let mut knot_mult = vec![3usize; k];
    if let Some(m) = knot_mult.first_mut() {
        *m = 4;
    }
    if let Some(m) = knot_mult.last_mut() {
        *m = 4;
    }
    let new_bsp = BSplineCurve::create_canonical(&CanonicalData {
        dim: 3,
        degree: 3,
        control_points: cntrl_poly,
        form: BSplineCurveForm::Unspecified,
        knot_multiplicities: &knot_mult,
        knots,
        knot_type: KnotType::Unspecified,
        weights: None,
        trim: None,
    })?;

    knots.clear();
    cntrl_poly.clear();
    Ok(Some(new_bsp))
}

/// `sm_SnapSolToSpecifiedEndpoint`: with `vec_to_inside` (the curve tangent
/// pointing from the wanted end into the curve), a solution within `snap_tol` of
/// an interval end is snapped to that end when the end's inward tangent agrees
/// with `vec_to_inside`, and marked bad when it opposes it. Returns the solution
/// and whether it is good.
///
/// # Errors
///
/// Those of the evaluator.
pub fn snap_sol_to_specified_endpoint<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    interval: &Extent1d,
    vec_to_inside: Option<Vec3>,
    solution: f64,
    snap_tol: f64,
) -> Result<(f64, bool)> {
    // init output
    let mut good = true;
    let mut solution = solution;
    let mut min_bound = false;

    // All solutions are good when cpVecToCrvInside == NULL
    let Some(vec_to_inside) = vec_to_inside else {
        return Ok((solution, good));
    };

    // The given point on the curve:
    let sol_pnt = curve.evaluate_point(k, solution)?;

    // Check the start point
    let pv = curve.evaluate(k, interval.min(), 1, true, true)?;
    let min_gap = at(&pv, 0).distance(sol_pnt);

    // when MinBoundary/Solution gap is less than tolerance
    if min_gap < snap_tol {
        min_bound = true;

        // snap Solution to start boundary
        if vec_to_inside.dot(at(&pv, 1)) < 0.0 {
            good = false;
        } else {
            good = true;
            solution = interval.min();
        }
    }

    // get MaxBoundaryPoint and gap
    let pv = curve.evaluate(k, interval.max(), 1, true, true)?;
    let max_gap = at(&pv, 0).distance(sol_pnt);

    // when MaxBoundary/Solution gap is less than tolerance and we haven't snapped to the minBoundary
    if max_gap < snap_tol && (!min_bound || !good) {
        // snap Solution to end boundary
        let tan = -at(&pv, 1);
        if vec_to_inside.dot(tan) < 0.0 {
            good = false;
        } else {
            good = true;
            solution = interval.max();
        }
    }
    Ok((solution, good))
}

/// `sm_UnitizeTangent`: the unit first derivative of `pvv` (position, first,
/// second derivative); for a zero first derivative the unit second derivative
/// when it points along `gap_vec` (towards the other curve), else zero. Also
/// returns the length of the first derivative.
#[must_use]
pub fn unitize_tangent(pvv: [Vec3; 3], gap_vec: Vec3) -> (Vec3, f64) {
    let mut len = pvv[1].length();
    let first_len = len;

    if len > EFF_ZERO {
        return (pvv[1] / len, first_len);
    }

    // a zero first derivative: use the 2nd derivative when it heads along the gap
    let dot = pvv[2].dot(gap_vec);
    if dot > EFF_ZERO {
        len = pvv[2].length();
        if len > EFF_ZERO {
            return (pvv[2] / len, first_len);
        }
        len = gap_vec.length();
        if len > EFF_ZERO {
            return (gap_vec / len, first_len);
        }
    }
    (Vec3::ZERO, first_len)
}

/// `sm_GetProjSignedAngle`: the counter-clockwise angle about `plane_normal` from
/// `pt1 - center` to `pt2 - center` (`-PI..=PI`), or [`BIG_DOUBLE`] when either
/// vector is zero or the second points against `side_direction`.
///
/// # Errors
///
/// Those of [`Vec3::ccw_angle_between`] (whose status upstream ignores; it cannot
/// fail for these non-zero vectors).
pub fn proj_signed_angle(
    pt1: Point3,
    pt2: Point3,
    center: Point3,
    plane_normal: Vec3,
    side_direction: Vec3,
) -> Result<f64> {
    // get and check Vec1 - nonZero
    let vec1 = pt1 - center;
    if vec1.length_squared() < EFF_ZERO_SQ {
        return Ok(BIG_DOUBLE);
    }

    // get and check Vec2 - nonZero and pointing in direction of sSideDirection
    let vec2 = pt2 - center;
    if vec2.length_squared() < EFF_ZERO_SQ || vec2.dot(side_direction) < 0.0 {
        return Ok(BIG_DOUBLE);
    }

    // get and return CCW angle in radians
    plane_normal.ccw_angle_between(vec1, vec2)
}

/// `sm_Output`: appends a tessellation point (parameter, position, tangent) to the
/// requested outputs.
pub fn output(
    pv: [Vec3; 2],
    param: f64,
    params: Option<&mut Vec<f64>>,
    points: Option<&mut Vec<Point3>>,
    tangents: Option<&mut Vec<Vec3>>,
) {
    if let Some(p) = params {
        p.push(param);
    }
    if let Some(p) = points {
        p.push(pv[0]);
    }
    if let Some(t) = tangents {
        t.push(pv[1]);
    }
}

/// Verdict of [`test_step`] on a tessellation step (`SmTessStepTestResultType`);
/// discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TessStepResult {
    /// `SM_ST_NOT_TESTED`.
    NotTested = 0,
    /// `SM_ST_ACCEPT_STEP`.
    Accept = 1,
    /// `SM_ST_CUT_STEP`.
    Cut = 2,
    /// `SM_ST_INCREASE_STEP`.
    Increase = 3,
}

/// `sm_TestStep`: tests the tessellation step of parameter length `step` from
/// `cur` to `next` (position, first derivative) against the maximum squared
/// distance, the chord height (of the Hermite span) and the tangent angle in
/// degrees; each test is skipped when its tolerance is at most `EFF_ZERO`.
#[must_use]
pub fn test_step(
    step: f64,
    cur: [Vec3; 2],
    next: [Vec3; 2],
    chord_height_tol: f64,
    angle_tol_deg: f64,
    max_dist_sq: f64,
) -> TessStepResult {
    let mut result = TessStepResult::Increase;

    // First do the distance checks because they are the easiest.
    if max_dist_sq > EFF_ZERO {
        let dist_sq = cur[0].distance_squared(next[0]);
        if dist_sq > max_dist_sq {
            return TessStepResult::Cut;
        }
        result = if dist_sq < max_dist_sq / 4.0 {
            TessStepResult::Increase
        } else {
            TessStepResult::Accept
        };
    }

    // Use a temporary hermite approximation to this segment.
    if chord_height_tol > EFF_ZERO {
        let herm = HermiteCurve::new(cur[0], cur[1] * step, next[0], next[1] * step, 3);
        let chord_height = herm.chord_height();
        if chord_height > chord_height_tol {
            return TessStepResult::Cut;
        }
        result = if chord_height < chord_height_tol / 2.0 && result != TessStepResult::Accept {
            TessStepResult::Increase
        } else {
            TessStepResult::Accept
        };
    }

    if angle_tol_deg > EFF_ZERO {
        if cur[1].length_squared() < EFF_ZERO_SQ || next[1].length_squared() < EFF_ZERO_SQ {
            return TessStepResult::Accept;
        }
        let Ok(angle_rad) = cur[1].angle_between(next[1]) else {
            return TessStepResult::Accept;
        };
        let angle_span = angle_rad * 180.0 / PI;
        if angle_span > angle_tol_deg {
            return TessStepResult::Cut;
        }
        result = if angle_span < angle_tol_deg / 2.0 && result != TessStepResult::Accept {
            TessStepResult::Increase
        } else {
            TessStepResult::Accept
        };
    }
    result
}

/// `sm_FindTestParams`: parameters to test in `[start, end]`: three per knot span
/// (at quarters), three in the partial spans at the ends when longer than
/// `EFF_ZERO_SQRT` (else the end itself), or the three quarter points when the
/// range lies in one span; `[start, end]` without knots.
#[must_use]
pub fn find_test_params(start: f64, end: f64, knots: &[f64]) -> Vec<f64> {
    let mut params = Vec::new();
    let n = knots.len();
    if n == 0 {
        params.push(start);
        params.push(end);
        return params;
    }
    let knot = |i: usize| knots.get(i).copied().unwrap_or(0.0);
    let mut l_start = 0;
    while knot(l_start) < start - EFF_ZERO && l_start < n - 1 {
        l_start += 1;
    }
    let mut l_end = n - 1;
    while l_end > 0 && knot(l_end) - EFF_ZERO > end {
        l_end -= 1;
    }

    // If same interval test 3 middle points of interval and return
    if l_start >= l_end {
        params.push(start + 0.25 * (end - start));
        params.push(start + 0.50 * (end - start));
        params.push(start + 0.75 * (end - start));
        return params;
    }

    // Add 3 start points first, if there's a gap between dParamStart and first knot.
    let mut delta = knot(l_start) - start;
    if delta > EFF_ZERO_SQRT {
        params.push(start + 0.25 * delta);
        params.push(start + 0.50 * delta);
        params.push(start + 0.75 * delta);
    } else {
        params.push(start);
    }

    // Add 3 points between each knot pair.
    for k in l_start + 1..=l_end {
        delta = knot(k) - knot(k - 1);
        params.push(knot(k - 1) + 0.25 * delta);
        params.push(knot(k - 1) + 0.50 * delta);
        params.push(knot(k - 1) + 0.75 * delta);
    }

    // Add 3 points at end if there's a gap.
    delta = end - knot(l_end);
    if delta > EFF_ZERO_SQRT {
        params.push(knot(l_end) + 0.25 * delta);
        params.push(knot(l_end) + 0.50 * delta);
        params.push(knot(l_end) + 0.75 * delta);
    } else {
        params.push(end);
    }
    params
}

// ---- evaluator objects ----------------------------------------------------------------

/// The arc-length integrand of a B-spline (`SmLengthIFEO`): the length of the
/// first derivative.
#[derive(Debug, Clone, Copy)]
pub struct LengthIntegrand<'a> {
    curve: &'a BSplineCurve,
}

impl<'a> LengthIntegrand<'a> {
    /// The integrand of `curve` (`SmLengthIFEO(const SmBSplineCurve&)`).
    #[must_use]
    pub const fn new(curve: &'a BSplineCurve) -> Self {
        Self { curve }
    }

    /// `|C'(t)|` (`Evaluate`, exact tangent values).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineCurve::evaluate`].
    pub fn evaluate(&self, t: f64) -> Result<f64> {
        let pv = self.curve.evaluate(t, 1, true, false)?;
        // The function being integrated is the first derivative of the curve
        Ok(at(&pv, 1).length())
    }
}

/// The min/max separation function of two equally parameterised curves
/// (`SmFindCurveDistanceExtremaEFO`; its `Evaluate` is unit `curve.curve_p4`).
#[derive(Debug, Clone, Copy)]
pub struct FindCurveDistanceExtrema<'a> {
    /// Caller context for virtual evaluations.
    pub kernel: &'a Kernel,

    /// `m_pCurve1`.
    pub curve1: &'a Curve,
    /// `m_pCurve2`.
    pub curve2: &'a Curve,
    /// `m_sDomain`: domain of interest on the curves.
    pub domain: Extent1d,
    /// `m_dTol`: stopping tolerance.
    pub tol: f64,
}

impl<'a> FindCurveDistanceExtrema<'a> {
    /// `SmFindCurveDistanceExtremaEFO(pCurve1, pCurve2, rsDomain, dTol)`.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        curve1: &'a Curve,
        curve2: &'a Curve,
        domain: Extent1d,
        tol: f64,
    ) -> Self {
        Self {
            kernel: k,
            curve1,
            curve2,
            domain,
            tol,
        }
    }
}

// ---- global solvers -------------------------------------------------------------------

/// The curve property a [`CurvePropertyGs`] looks for (`SmCurvePropertyType`);
/// discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurvePropertyType {
    /// `SM_CP_PERPENDICULAR_TO_VECTOR`.
    PerpendicularToVector = 0,
    /// `SM_CP_INFLECTION_POINTS`.
    InflectionPoints = 1,
    /// `SM_CP_RADIUS_OF_CURVATURE`.
    RadiusOfCurvature = 2,
    /// `SM_CP_FIRST_DERIVATIVE_LENGTH`.
    FirstDerivativeLength = 3,
    /// `SM_CP_PARALLEL_TO_VECTOR`.
    ParallelToVector = 4,
    /// `SM_CP_SILHOUETTE_VECTOR`.
    SilhouetteVector = 5,
    /// `SM_CP_SILHOUETTE_POINT`.
    SilhouettePoint = 6,
    /// `SM_CP_X_NORMAL`.
    XNormal = 7,
    /// `SM_CP_Y_NORMAL`.
    YNormal = 8,
    /// `SM_CP_Z_NORMAL`.
    ZNormal = 9,
    /// `SM_CP_XYZ_NORMAL`.
    XyzNormal = 10,
    /// `SM_CP_MINIMIZE_FIRST_DERIVATIVE`.
    MinimizeFirstDerivative = 11,
    /// `SM_CP_MAXIMIZE_FIRST_DERIVATIVE`.
    MaximizeFirstDerivative = 12,
    /// `SM_CP_MINIMIZE_RADIUS_OF_CURVATURE`.
    MinimizeRadiusOfCurvature = 13,
    /// `SM_CP_MAXIMIZE_RADIUS_OF_CURVATURE`.
    MaximizeRadiusOfCurvature = 14,
    /// `SM_CP_PLANE_CLASSIFY`.
    PlaneClassify = 15,
    /// `SM_CP_PLANE_INTERSECTION`.
    PlaneIntersection = 16,
    /// `SM_CP_MINIMIZE_ANGLE_TO_PLANE`.
    MinimizeAngleToPlane = 17,
    /// `SM_CP_MINIMIZE_DIRECTED_ANGLE`.
    MinimizeDirectedAngle = 18,
    /// `SM_CP_MINIMIZE_CCW_ANGLE`.
    MinimizeCcwAngle = 19,
    /// `SM_CP_PROJECTED_POINT_MINIMIZE`.
    ProjectedPointMinimize = 20,
    /// `SM_CP_PROJECTED_POINT_MAXIMIZE`.
    ProjectedPointMaximize = 21,
    /// `SM_CP_CYLINDER_INTERSECTION`.
    CylinderIntersection = 22,
    /// `SM_CP_DISTANCE_TO_POINT`.
    DistanceToPoint = 23,
    /// `SM_CP_TANGENT_TO_CIRCLE`.
    TangentToCircle = 24,
    /// `SM_CP_POINT_INVERSION`.
    PointInversion = 25,
    /// `SM_CP_MAXIMA_TO_LINE`.
    MaximaToLine = 26,
    /// `SM_CP_MAXIMA_TO_PLANE`.
    MaximaToPlane = 27,
    /// `SM_CP_PROJECTED_TANGENT_THROUGH_POINT`.
    ProjectedTangentThroughPoint = 28,
}

impl CurvePropertyType {
    /// Every property, in upstream order.
    pub const ALL: [Self; 29] = [
        Self::PerpendicularToVector,
        Self::InflectionPoints,
        Self::RadiusOfCurvature,
        Self::FirstDerivativeLength,
        Self::ParallelToVector,
        Self::SilhouetteVector,
        Self::SilhouettePoint,
        Self::XNormal,
        Self::YNormal,
        Self::ZNormal,
        Self::XyzNormal,
        Self::MinimizeFirstDerivative,
        Self::MaximizeFirstDerivative,
        Self::MinimizeRadiusOfCurvature,
        Self::MaximizeRadiusOfCurvature,
        Self::PlaneClassify,
        Self::PlaneIntersection,
        Self::MinimizeAngleToPlane,
        Self::MinimizeDirectedAngle,
        Self::MinimizeCcwAngle,
        Self::ProjectedPointMinimize,
        Self::ProjectedPointMaximize,
        Self::CylinderIntersection,
        Self::DistanceToPoint,
        Self::TangentToCircle,
        Self::PointInversion,
        Self::MaximaToLine,
        Self::MaximaToPlane,
        Self::ProjectedTangentThroughPoint,
    ];

    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The property with upstream value `code`, if any.
    #[must_use]
    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.code() == code)
    }
}

/// Whether `node` is part of a curve decomposition tree (`SM_AD_BEZIER_CURVE` or
/// `SM_AD_CURVE_DATA`).
const fn is_curve_aux<O>(node: &TreeNode<O>) -> bool {
    matches!(
        node.aux_type,
        AuxDataType::BezierCurve | AuxDataType::CurveData
    )
}

/// Whether `node` has data (`m_pData != NULL`).
const fn has_data<O>(node: &TreeNode<O>) -> bool {
    !matches!(node.data, NodeData::None)
}

/// `SmBezierAux1d::m_dMaxTurningAngleDeg` of a curve tree node.
fn span_max_turning_angle_deg<O>(tree: &Tree<O>, node: &TreeNode<O>) -> Result<f64> {
    Ok(tree.curve_aux(node)?.max_turning_angle_deg)
}

/// `SmBezierAux1d::GetEnds` of a curve tree node (start and end point, as stored
/// by the curve cache when it built the tree).
fn span_ends<O>(tree: &Tree<O>, node: &TreeNode<O>) -> Result<(Point3, Point3)> {
    let [start, end] = tree.curve_aux(node)?.ends;
    Ok((start, end))
}

/// The corners of `SmBezierSpan::GetPseudoBox` of a curve tree node.
fn span_pseudo_box_corners<O>(tree: &Tree<O>, node: &TreeNode<O>) -> Result<[Point3; 8]> {
    Ok(tree.curve_span(node)?.pseudo_box.corners())
}

/// Curve/curve global solver (`SmCCGlobalSolver`): one parameter per tree.
#[derive(Debug, Clone)]
pub struct CcGlobalSolver<'t, O> {
    /// The `SmGlobalSolver` state.
    pub base: GlobalSolver<'t, O>,
    /// `m_crInterval1`: interval of the first curve.
    pub interval1: Extent1d,
    /// `m_crInterval2`: interval of the second curve.
    pub interval2: Extent1d,
    /// The curves the two trees decompose (upstream reads them back from the trees'
    /// curve caches, `GetOwner()->GetCurve()`); `None` until set with
    /// [`CcGlobalSolver::with_curves`].
    pub curves: Option<[&'t Curve; 2]>,
    /// The kernel the local solves run in (upstream reaches it through the curves'
    /// context); `None` until set with [`CcGlobalSolver::with_curves`].
    pub kernel: Option<&'t Kernel>,
}

impl<'t, O: SolutionObject + 't> CcGlobalSolver<'t, O> {
    /// `SmCCGlobalSolver(pPointTree, pCurveTree, crInterval1, crInterval2)`.
    #[must_use]
    pub fn new(
        tree1: &'t Tree<O>,
        tree2: &'t Tree<O>,
        interval1: Extent1d,
        interval2: Extent1d,
    ) -> Self {
        Self {
            base: GlobalSolver::with_trees(tree1, tree2, 1),
            interval1,
            interval2,
            curves: None,
            kernel: None,
        }
    }

    /// The solver with `curve1` and `curve2`, the curves its trees were built for,
    /// and the kernel `k`, which [`GlobalSolve::local_solve`] needs.
    #[must_use]
    pub const fn with_curves(
        mut self,
        curve1: &'t Curve,
        curve2: &'t Curve,
        k: &'t Kernel,
    ) -> Self {
        self.curves = Some([curve1, curve2]);
        self.kernel = Some(k);
        self
    }

    /// `SmCCGlobalSolver::BranchMayContainAnswers`: the base test, except for
    /// `SM_SO_NORMALIZE` and `SM_SO_PROJECTED_TANGENCY`, which cull span pairs whose
    /// chords and turning angles rule out common normals or tangents.
    /// `SM_SO_SURFACE_PROJECTED_INTERSECT` runs the base test with the second curve
    /// as the second tree's owner object.
    ///
    /// # Errors
    ///
    /// Those of the base test and of the vector and tree queries;
    /// [`SolidError::Unported`] once the curve span data is needed.
    pub fn may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        let op = self.base.operation;
        if op == SolverOperation::SurfaceProjectedIntersect {
            // The base test, with the second tree's owner curve (upstream
            // `apBranch[1]->GetOwnerObject()`) read from the solver's curves.
            if let (Some([_, curve2]), Some(k)) = (self.curves, self.kernel) {
                return self.base.surface_projected_may_contain_answers(
                    branch,
                    Some((k, curve2.as_crv_on_surf())),
                );
            }
        }
        if op != SolverOperation::Normalize && op != SolverOperation::ProjectedTangency {
            return self.base.branch_may_contain_answers(branch);
        }

        let node1 = self.base.branch_node(branch, 0)?;
        let node2 = self.base.branch_node(branch, 1)?;

        // Make sure nodes are SmBezierSpan types
        if !is_curve_aux(node1) || !is_curve_aux(node2) {
            return Ok(true);
        }

        // NER on the span data: SM_ERR_NULL_POINTER, read as TRUE by the callers
        if !has_data(node1) || !has_data(node2) {
            return Ok(true);
        }
        let tree1 = self.base.tree(0)?;
        let tree2 = self.base.tree(1)?;
        let turn1 = span_max_turning_angle_deg(tree1, node1)?;
        let turn2 = span_max_turning_angle_deg(tree2, node2)?;

        // Can't check for Normals when accumulated angle turning in both spans exceeds 90
        if turn1 + turn2 > 90.0 {
            return Ok(true);
        }

        let (start1, mut end1) = span_ends(tree1, node1)?;
        let (mut start2, mut end2) = span_ends(tree2, node2)?;

        // for PROJECTED_TANGENCY - project endPoints to common plane
        if op == SolverOperation::ProjectedTangency {
            let normal = self.base.opt_vector(0)?;
            end1 = point_project_to_plane(end1, start1, normal);
            start2 = point_project_to_plane(start2, start1, normal);
            end2 = point_project_to_plane(end2, start1, normal);
        }

        // get 1st and 2nd curve endPoint chord lines
        let line_vec1 = end1 - start1;
        let line_vec2 = end2 - start2;

        // If we don't have good lines quit here with a TRUE
        if line_vec1.length_squared() < EFF_ZERO_SQ || line_vec2.length_squared() < EFF_ZERO_SQ {
            return Ok(true);
        }

        // get closest line between 1st and 2nd curve endPoint chordLines
        if let Ok((t1, t2)) = line_line_closest_point(start1, line_vec1, start2, line_vec2) {
            let p1 = start1 + t1 * line_vec1;
            let p2 = start2 + t2 * line_vec2;
            let vec = p1 - p2;

            // when gap vector is zero
            if vec.length_squared() < EFF_ZERO_SQ {
                // Coplanarity - get angle between chords lines
                let ang = line_vec1.angle_between(line_vec2)?;
                let mut ang_deg = ang * 180.0 / PI;
                if ang_deg > 90.0 {
                    ang_deg = 180.0 - ang_deg;
                }

                // when angle is larger than the combined turning angle
                let combined = turn1 + turn2;
                if ang_deg * 0.99 - EFF_ZERO > combined {
                    // no normalized solution possible
                    return Ok(false);
                }
            }
        }

        // For now this is as far as we go with the projected tangency
        if op == SolverOperation::ProjectedTangency {
            return Ok(true);
        }

        // First Test to see if segment 2 is outside of valid angle of segment1
        if turn1 < 90.0 {
            let (ang1, t1) = segment_point_angle_deg_to_perp(start1, end1, start2);
            let (ang2, t2) = segment_point_angle_deg_to_perp(start1, end1, end2);
            if min2(ang1, ang2) > turn1 && (t1 - t2).abs() < EFF_ZERO {
                return Ok(false);
            }
        }

        // Now test to see if segment 1 is outside of valid angle of segment 2
        if turn2 < 90.0 {
            let (ang1, t1) = segment_point_angle_deg_to_perp(start2, end2, start1);
            let (ang2, t2) = segment_point_angle_deg_to_perp(start2, end2, end1);
            if min2(ang1, ang2) > turn2 && (t1 - t2).abs() < EFF_ZERO {
                return Ok(false);
            }
        }

        // all done - there may be an answer here
        Ok(true)
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for CcGlobalSolver<'t, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.base
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.base
    }

    fn branch_may_contain_answers(&mut self, branch: &Branch) -> Result<bool> {
        self.may_contain_answers(branch)
    }

    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        self.local_solve_branch(branch)
    }

    /// `SmGlobalSolver::IdenticalSolutions` (not overridden upstream), with each
    /// solution curve's natural interval read from the curve its tree decomposes
    /// (see `TreeCurves`).
    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let trees = [self.base.trees[0], self.base.trees[1]];
        let curves = self
            .curves
            .map_or([None, None], |[c1, c2]| [Some(c1), Some(c2)]);
        let domains = TreeCurves { trees, curves };
        self.base.identical_solutions_in(sol1, sol2, &domains)
    }
}

/// [`ParameterDomains`] of the curve solvers: upstream's solution object is the
/// `SmCurve*` a curve tree's cache owns (`SmTree::GetOwnerObject`), whose
/// `GetNaturalInterval` `IdenticalSolutions` reads. A handle the tree names as its
/// owner object resolves to the curve the solver decomposes with that tree; any
/// other handle answers for itself.
struct TreeCurves<'a, O> {
    trees: [Option<&'a Tree<O>>; 2],
    curves: [Option<&'a Curve>; 2],
}

impl<O: SolutionObject> ParameterDomains<O> for TreeCurves<'_, O> {
    fn natural_interval(&self, curve: &O) -> Option<Extent1d> {
        curve.natural_interval().or_else(|| {
            self.trees
                .iter()
                .zip(&self.curves)
                .find_map(|(tree, c)| {
                    let owner = tree.as_ref()?.owner_object().ok()??;
                    (owner == *curve).then_some(*c).flatten()
                })
                .map(Curve::natural_interval)
        })
    }
}

/// Curve/point global solver (`SmCPGlobalSolver`): a point tree and a curve tree,
/// one parameter.
#[derive(Debug, Clone)]
pub struct CpGlobalSolver<'t, O> {
    /// Caller context for virtual evaluations.
    pub kernel: &'t Kernel,

    /// The `SmGlobalSolver` state.
    pub base: GlobalSolver<'t, O>,
    /// `m_crInterval`: the curve interval to query.
    pub interval: Extent1d,
    /// The curve the curve tree decomposes (upstream reads it back from the tree's
    /// curve cache, `GetOwner()->GetCurve()`); `None` until set with
    /// [`CpGlobalSolver::with_curve`].
    pub curve: Option<&'t Curve>,
    /// The caller's `SmTol::GetZoneTol3d(const SmObject*)` of the solution handles
    /// ([`GlobalSolve::object_zone_tol_3d`]); `None` defers.
    pub zone_tol_3d: Option<ObjectZoneTol<'t, O>>,
}

impl<'t, O: SolutionObject + 't> CpGlobalSolver<'t, O> {
    /// `SmCPGlobalSolver(rContext, pPointTree, pCurveTree, crInterval)`.
    #[must_use]
    pub fn new(
        k: &'t Kernel,
        point_tree: &'t Tree<O>,
        curve_tree: &'t Tree<O>,
        interval: Extent1d,
    ) -> Self {
        Self {
            kernel: k,
            base: GlobalSolver::with_trees(point_tree, curve_tree, 1),
            interval,
            curve: None,
            zone_tol_3d: None,
        }
    }

    /// The solver with the curve its curve tree decomposes (needed by
    /// `LocalSolve`).
    #[must_use]
    pub const fn with_curve(mut self, curve: &'t Curve) -> Self {
        self.curve = Some(curve);
        self
    }

    /// `SmCPGlobalSolver::BranchMayContainAnswers`: the base test, except for
    /// `SM_SO_NORMALIZE`, which culls spans no perpendicular from the point can
    /// reach given their turning angle.
    ///
    /// # Errors
    ///
    /// Those of the base test and of the tree queries; [`SolidError::Unported`] once
    /// the curve span data is needed.
    pub fn may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        // Use general BranchMayContainAnswers() check for all cases but NORMALIZE
        if self.base.operation != SolverOperation::Normalize {
            return self.base.branch_may_contain_answers(branch);
        }

        let point_node = self.base.branch_node(branch, 0)?;
        let curve_node = self.base.branch_node(branch, 1)?;
        // NER on the span data: SM_ERR_NULL_POINTER, read as TRUE by the callers
        if !has_data(curve_node) {
            return Ok(true);
        }
        let test_pnt = point_node.bbox.min();

        // no culling - When Curve node is not part of a curve decomposition tree
        if !is_curve_aux(curve_node) {
            return Ok(true);
        }

        // no culling until BezSpan's turning angle gets down below 90 degrees
        let curve_tree = self.base.tree(1)?;
        let turn = span_max_turning_angle_deg(curve_tree, curve_node)?;
        if turn > 90.0 {
            return Ok(true);
        }

        // First check whether the given point is within tolerance of either end point.
        let (pt0, pt1) = span_ends(curve_tree, curve_node)?;
        let tol = self.base.tolerance_3d;
        if test_pnt.closer_than(tol, pt0) {
            return Ok(true);
        }
        if test_pnt.closer_than(tol, pt1) {
            return Ok(true);
        }

        // compute the angle from 90 degrees for the testPoint to the span chord segment
        let (ang_deg_from_perp, _t) = segment_point_angle_deg_to_perp(pt0, pt1, test_pnt);

        // cull cases where no perp drop point is possible
        if ang_deg_from_perp * 0.99 - EFF_ZERO > turn {
            return Ok(false);
        }
        Ok(true)
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for CpGlobalSolver<'t, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.base
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.base
    }

    fn object_zone_tol_3d(&self, object: O) -> Result<f64> {
        resolve_object_zone_tol_3d(self.zone_tol_3d, object)
    }

    fn branch_may_contain_answers(&mut self, branch: &Branch) -> Result<bool> {
        self.may_contain_answers(branch)
    }

    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        self.local_solve_branch(k, branch)
    }

    /// `SmGlobalSolver::IdenticalSolutions` (not overridden upstream), with the
    /// solution curve's natural interval read from the curve the curve tree
    /// decomposes (see `TreeCurves`).
    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let domains = TreeCurves {
            trees: [None, self.base.trees[1]],
            curves: [None, self.curve],
        };
        self.base.identical_solutions_in(sol1, sol2, &domains)
    }
}

/// Curve property global solver (`SmCurvePropertyGS`): one curve tree, one
/// parameter.
#[derive(Debug, Clone)]
pub struct CurvePropertyGs<'t, O> {
    /// Caller context for virtual evaluations.
    pub kernel: &'t Kernel,

    /// The `SmGlobalSolver` state.
    pub base: GlobalSolver<'t, O>,
    /// `m_crInterval`: the curve interval to examine.
    pub interval: Extent1d,
    /// `m_eCurveProperty`: the property to find.
    pub property: CurvePropertyType,
    /// `m_cpdPropertyValue` (`None` = null).
    pub property_value: Option<f64>,
    /// `m_cpOptVectors` (this class's own member, shadowing the base one).
    pub opt_vectors: Vec<Vec3>,
    /// The curve the tree decomposes (upstream reads it back as the tree node's
    /// owner object, `pCurveNode->GetOwnerObject()`); `None` until set with
    /// [`CurvePropertyGs::with_curve`].
    pub curve: Option<&'t Curve>,
}

/// Angle measure of the `MINIMIZE_*ANGLE*` properties for the unit projected
/// vector `vec` (`dCosTheta` upstream).
pub(crate) fn property_angle(
    prop: CurvePropertyType,
    o1: Vec3,
    o2: Vec3,
    vec: Vec3,
) -> Result<f64> {
    if prop == CurvePropertyType::MinimizeAngleToPlane {
        Ok(PI / 2.0 - acos(vec.dot(o2).abs()))
    } else {
        // SM_CP_MINIMIZE_DIRECTED_ANGLE
        let mut cos_theta = o1.ccw_angle_between(o2, vec)?;
        if cos_theta > PI {
            cos_theta = 2.0 * PI - cos_theta; // make range 0 to -180
        }
        if prop == CurvePropertyType::MinimizeCcwAngle && cos_theta < 0.0 {
            cos_theta += 2.0 * PI;
        }
        Ok(cos_theta)
    }
}

impl<'t, O: SolutionObject + 't> CurvePropertyGs<'t, O> {
    /// `SmCurvePropertyGS(pCurveTree, crInterval, eCurveProperty, cpdPropertyValue,
    /// cpOptVectors)`.
    #[must_use]
    pub fn new(
        k: &'t Kernel,
        curve_tree: &'t Tree<O>,
        interval: Extent1d,
        property: CurvePropertyType,
        property_value: Option<f64>,
        opt_vectors: Vec<Vec3>,
    ) -> Self {
        Self {
            kernel: k,
            base: GlobalSolver::with_tree(curve_tree, 1),
            interval,
            property,
            property_value,
            opt_vectors,
            curve: None,
        }
    }

    /// The solver with `curve`, the curve its tree was built for (the owner object
    /// of the tree's nodes upstream), which [`GlobalSolve::local_solve`] evaluates.
    #[must_use]
    pub const fn with_curve(mut self, curve: &'t Curve) -> Self {
        self.curve = Some(curve);
        self
    }

    /// Optional vector `i` (`m_cpOptVectors[i]`).
    pub(crate) fn opt(&self, i: usize) -> Result<Vec3> {
        self.opt_vectors
            .get(i)
            .copied()
            .ok_or(SolidError::InvalidInput {
                operation: OP_GS,
                reason: "missing optional vector",
            })
    }

    /// `*m_cpdPropertyValue`.
    pub(crate) fn value(&self) -> Result<f64> {
        self.property_value.ok_or(SolidError::InvalidInput {
            operation: OP_GS,
            reason: "missing property value",
        })
    }

    /// `SmCurvePropertyGS::FindBestSubdivisionNode`: for the angle and projected
    /// point properties, always tree 0, visiting first the child whose box centre is
    /// angularly closer to the plane (or nearer / farther from the projected
    /// point); the base choice otherwise.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` ([`SolidError::Upstream`], code 1006) for a node without
    /// both children; those of the base choice and of the vector queries.
    pub fn best_subdivision_node(&self, branch: &Branch) -> Result<(usize, bool)> {
        use CurvePropertyType as P;
        let prop = self.property;
        if !matches!(
            prop,
            P::MinimizeAngleToPlane
                | P::MinimizeDirectedAngle
                | P::MinimizeCcwAngle
                | P::ProjectedPointMinimize
                | P::ProjectedPointMaximize
        ) {
            return self.base.find_best_subdivision_node(branch);
        }

        // take the branch whose bounding box centre is angularly closer to the plane
        let mut swap = false;
        let curve_node = self.base.branch_node(branch, 0)?;
        let tree = self.base.tree(0)?;
        let null = || SolidError::null_pointer(OP_GS);
        let node1 = tree.node(curve_node.child1.ok_or_else(null)?)?;
        let node2 = tree.node(curve_node.child2.ok_or_else(null)?)?;
        let sph1 = node1.bbox.sphere_bound();
        let sph2 = node2.bbox.sphere_bound();
        let mut cent1 = sph1.center;
        let mut cent2 = sph2.center;
        if matches!(
            prop,
            P::MinimizeAngleToPlane | P::MinimizeDirectedAngle | P::MinimizeCcwAngle
        ) {
            let (o0, o1, o2) = (self.opt(0)?, self.opt(1)?, self.opt(2)?);
            // Project sVec1 to the projection plane
            let mut vec1 = o1.cross(cent1 - o0).cross(o1);
            if vec1.length_squared() < EFF_ZERO {
                return Ok((0, swap));
            }
            vec1 = vec1.unitize()?;
            let cos_theta1 = property_angle(prop, o1, o2, vec1)?;

            let mut vec2 = o1.cross(cent2 - o0).cross(o1); // Project vector to plane
            if vec2.length_squared() < EFF_ZERO {
                return Ok((0, swap));
            }
            vec2 = vec2.unitize()?;
            let cos_theta2 = property_angle(prop, o1, o2, vec2)?;

            if cos_theta2 < cos_theta1 {
                swap = true;
            }
        } else {
            let (o0, o1) = (self.opt(0)?, self.opt(1)?);
            // Project points to the plane and see which is better branch to persue
            cent1 = point_project_to_plane(cent1, o0, o1);
            cent2 = point_project_to_plane(cent2, o0, o1);
            if prop == P::ProjectedPointMinimize {
                if cent1.distance_squared(o0) > cent2.distance_squared(o0) {
                    swap = true;
                }
            } else if cent1.distance_squared(o0) < cent2.distance_squared(o0) {
                // Maximize
                swap = true;
            }
        }
        Ok((0, swap))
    }

    /// `SmCurvePropertyGS::BranchMayContainAnswers`: false when the node's box (or
    /// span) cannot hold the property (see the per-property tests upstream).
    ///
    /// # Errors
    ///
    /// Those of the vector and tree queries; [`SolidError::InvalidInput`] for a
    /// missing optional vector or value; [`SolidError::Unported`] once the curve span
    /// data is needed.
    pub fn may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        use CurvePropertyType as P;
        let curve_node = self.base.branch_node(branch, 0)?;
        // NER on the span data: SM_ERR_NULL_POINTER, read as TRUE by the callers
        if !has_data(curve_node) {
            return Ok(true);
        }
        let bbox = curve_node.bbox;
        let tol = self.base.tolerance_3d;
        let best_sq = self.base.best_answer_so_far_sq;
        let prop = self.property;

        match prop {
            // false when the target point is further from the box than the best answer
            P::ProjectedPointMinimize => {
                let (o0, o1) = (self.opt(0)?, self.opt(1)?);
                let sph = bbox.sphere_bound();
                let center = point_project_to_plane(sph.center, o0, o1);
                let mut dist = o0.distance(center) - sph.radius - tol;
                if dist < 0.0 {
                    dist = 0.0;
                }
                if dist * dist > best_sq {
                    return Ok(false);
                }
                return Ok(true);
            }
            // false when the target point is closer to the box than the best answer
            P::ProjectedPointMaximize => {
                let (o0, o1) = (self.opt(0)?, self.opt(1)?);
                let sph = bbox.sphere_bound();
                let center = point_project_to_plane(sph.center, o0, o1);
                let dist = o0.distance(center) + sph.radius + tol;
                if dist * dist < best_sq {
                    return Ok(false);
                }
                return Ok(true);
            }
            // false when min/max distances to bbox don't bracket the target distance
            P::DistanceToPoint => {
                let point_box = Extent3d::from_point(self.opt(0)?);
                let v = self.value()?;
                let rad_squared = v * v;

                let dist_sq = point_box.max_distance_squared(&bbox);
                if dist_sq < rad_squared - EFF_ZERO {
                    return Ok(false);
                }
                let dist_sq = point_box.min_distance_squared(&bbox);
                if dist_sq > rad_squared + EFF_ZERO {
                    return Ok(false);
                }

                // Also, if curve box contains point:
                if dist_sq < EFF_ZERO_SQ {
                    let size_sq = bbox.size().length_squared();
                    if size_sq < rad_squared - EFF_ZERO {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            // false when no bbox corner angle is less than the current best answer
            P::MinimizeAngleToPlane | P::MinimizeDirectedAngle | P::MinimizeCcwAngle => {
                let (o0, o1, o2) = (self.opt(0)?, self.opt(1)?, self.opt(2)?);
                for i in 0..2u32 {
                    let xv = f64::from(i);
                    for j in 0..2u32 {
                        let yv = f64::from(j);
                        for _k in 0..2u32 {
                            let zv = f64::from(j);
                            let test_pnt = bbox.evaluate(xv, yv, zv);
                            let mut vec = o1.cross(test_pnt - o0).cross(o1);
                            if vec.length_squared() < EFF_ZERO {
                                return Ok(true);
                            }
                            vec = vec.unitize()?;
                            let cos_theta = property_angle(prop, o1, o2, vec)?;
                            if (cos_theta * cos_theta.abs()) + EFF_ZERO_SQRT < best_sq {
                                return Ok(true);
                            }
                        }
                    }
                }
                // If all of points pass the test then we are done with this branch
                return Ok(false);
            }
            // false when bbox does not intersect the cylinder radius
            P::CylinderIntersection => {
                let cyl_radius = self.value()?;
                let sph = bbox.sphere_bound();
                let (o0, o1) = (self.opt(0)?, self.opt(1)?);
                let vec = o1.cross(sph.center - o0).cross(o1);
                if (vec.length() - cyl_radius).abs() - tol > sph.radius {
                    return Ok(false);
                }
                return Ok(true);
            }
            // false when bbox and pseudo bbox does not intersect plane
            P::PlaneIntersection => {
                let abc = self.opt(0)?;
                let d = self.value()?;
                let sph = bbox.sphere_bound();
                let dist = sph.center.dot(abc) + d;

                if dist.abs() > sph.radius + tol {
                    return Ok(false);
                }
                let mut ret = dist.abs() < tol;

                // Here we'll test corners then other points to get a tighter test
                let crosses = |p: Point3| {
                    let d2 = abc.dot(p) + d;
                    dist * d2 < 0.0 || d2.abs() < tol
                };
                if crosses(bbox.max()) {
                    ret = true;
                }
                for corner in [
                    None,
                    Some((1.0, 0.0, 0.0)),
                    Some((1.0, 1.0, 0.0)),
                    Some((1.0, 0.0, 1.0)),
                    Some((0.0, 1.0, 0.0)),
                    Some((0.0, 1.0, 1.0)),
                    Some((0.0, 0.0, 1.0)),
                ] {
                    if ret {
                        break;
                    }
                    let p = match corner {
                        None => bbox.min(),
                        Some((x, y, z)) => bbox.evaluate(x, y, z),
                    };
                    if crosses(p) {
                        ret = true;
                    }
                }

                // bounding box completely on one side of plane
                if !ret {
                    return Ok(ret);
                }

                // Let's try the tighter pseudo box test
                if curve_node.aux_type == AuxDataType::BezierCurve {
                    let corners = span_pseudo_box_corners(self.base.tree(0)?, curve_node)?;
                    let dist = abc.dot(corners[0]) + d;
                    if dist.abs() < tol {
                        return Ok(true);
                    }
                    for c in corners.iter().skip(1) {
                        let d2 = abc.dot(*c) + d;
                        if dist * d2 < 0.0 || d2.abs() < tol {
                            return Ok(true);
                        }
                    }
                    return Ok(false);
                }
                return Ok(true);
            }
            _ => {}
        }

        // Make sure that the nodes have data in them
        if !is_curve_aux(curve_node) {
            return Ok(true);
        }

        // Don't try to make the call too soon - wait for angle to get down below 90 degrees
        let tree = self.base.tree(0)?;
        let turn = span_max_turning_angle_deg(tree, curve_node)?;
        if turn > 90.0 {
            return Ok(true);
        }

        // return false when turning angle is not near 90 degrees
        if prop == P::PerpendicularToVector || prop == P::MaximaToPlane {
            let (line_pt, end) = span_ends(tree, curve_node)?;
            let line_vec = end - line_pt;

            // no solutions for degenerate spans
            if line_vec.length_squared() < EFF_ZERO_SQ {
                return Ok(false);
            }

            let reference = if prop == P::PerpendicularToVector {
                self.opt(0)?
            } else {
                self.opt(1)?
            };
            if reference.length_squared() < EFF_ZERO_SQ {
                return Ok(false);
            }
            let angle = line_vec.angle_between(reference)?;

            // If the angle is not near 90 degrees then we can return a FALSE
            if angle + turn < PI / 2.0 || angle - turn > PI / 2.0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for CurvePropertyGs<'t, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.base
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.base
    }

    fn branch_may_contain_answers(&mut self, branch: &Branch) -> Result<bool> {
        self.may_contain_answers(branch)
    }

    fn find_best_subdivision_node(&self, branch: &Branch) -> Result<(usize, bool)> {
        self.best_subdivision_node(branch)
    }

    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        self.local_solve_branch(k, branch)
    }

    /// `SmGlobalSolver::IdenticalSolutions` (not overridden upstream), with the
    /// solution curve's natural interval read from the curve the tree decomposes (see
    /// `TreeCurves`), as for `CcGlobalSolver`.
    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let domains = TreeCurves {
            trees: [self.base.trees[0], None],
            curves: [self.curve, None],
        };
        self.base.identical_solutions_in(sol1, sol2, &domains)
    }
}

/// Curve/curve intersection global solver (`SmCCIGlobalSolver`): two curve trees,
/// two parameters.
#[derive(Debug, Clone)]
pub struct CciGlobalSolver<'t, O> {
    /// The `SmGlobalSolver` state.
    pub base: GlobalSolver<'t, O>,
    /// `m_crInterval1`.
    pub interval1: Extent1d,
    /// `m_crInterval2`.
    pub interval2: Extent1d,
    /// `m_bDoProjection`: projected, rotated-projected or perspective intersection.
    pub do_projection: bool,
    /// `m_eProjectionType`, used with [`CciGlobalSolver::do_projection`].
    pub projection_type: ProjectionType,
    /// `m_bSelfIntersection`: both trees are the same tree.
    pub self_intersection: bool,
    /// The curves the two trees decompose (upstream reads them back as the tree
    /// nodes' owner objects, `pCurveNode->GetOwnerObject()`); `None` until set with
    /// [`CciGlobalSolver::with_curves`].
    pub curves: Option<[&'t Curve; 2]>,
    /// The kernel the local solves run in (upstream reaches it through the curves'
    /// context); `None` until set with [`CciGlobalSolver::with_curves`].
    pub kernel: Option<&'t Kernel>,
}

impl<'t, O: SolutionObject + 't> CciGlobalSolver<'t, O> {
    /// `SmCCIGlobalSolver(pCurve1Tree, pCurve2Tree, crInterval1, crInterval2,
    /// bDoProjection, eProjectionType)` (upstream defaults `FALSE`,
    /// `SM_PT_PARALLEL`).
    #[must_use]
    pub fn new(
        tree1: &'t Tree<O>,
        tree2: &'t Tree<O>,
        interval1: Extent1d,
        interval2: Extent1d,
        do_projection: bool,
        projection_type: ProjectionType,
    ) -> Self {
        Self {
            base: GlobalSolver::with_trees(tree1, tree2, 2),
            interval1,
            interval2,
            do_projection,
            projection_type,
            self_intersection: core::ptr::eq(tree1, tree2),
            curves: None,
            kernel: None,
        }
    }

    /// The solver with `curve1` and `curve2`, the curves its trees were built for
    /// (the owner objects of the trees' nodes upstream), and the kernel `k`, which
    /// [`GlobalSolve::local_solve`] needs.
    #[must_use]
    pub const fn with_curves(
        mut self,
        curve1: &'t Curve,
        curve2: &'t Curve,
        k: &'t Kernel,
    ) -> Self {
        self.curves = Some([curve1, curve2]);
        self.kernel = Some(k);
        self
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for CciGlobalSolver<'t, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.base
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.base
    }

    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        self.local_solve_branch(branch)
    }

    /// `SmGlobalSolver::IdenticalSolutions` (not overridden upstream), with each
    /// solution curve's natural interval read from the curve its tree decomposes (see
    /// `TreeCurves`), as for `CcGlobalSolver`.
    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let trees = [self.base.trees[0], self.base.trees[1]];
        let curves = self
            .curves
            .map_or([None, None], |[c1, c2]| [Some(c1), Some(c2)]);
        let domains = TreeCurves { trees, curves };
        self.base.identical_solutions_in(sol1, sol2, &domains)
    }
}

/// `GetKnots`' out-parameters with its status. The classifying overrides
/// (`SmCrvOnSurf`, `SmCrvInVolume`) write the base curve's knots and multiplicities
/// before their classification can fail (`SER`), and their own callers ignore the
/// status and read what was written.
#[derive(Debug, Clone, PartialEq)]
pub struct KnotsWithStatus {
    /// `rKnots`.
    pub knots: Vec<f64>,
    /// `pOptMults`.
    pub mults: Vec<usize>,
    /// The returned status: `Err` (never [`SolidError::Unported`]) for a failing one.
    pub status: Result<()>,
}

impl KnotsWithStatus {
    /// A successful `GetKnots`.
    #[must_use]
    pub const fn success(knots: Vec<f64>, mults: Vec<usize>) -> Self {
        Self {
            knots,
            mults,
            status: Ok(()),
        }
    }

    /// The lists, or the failing status.
    ///
    /// # Errors
    ///
    /// The failing status.
    pub fn into_result(self) -> Result<(Vec<f64>, Vec<usize>)> {
        self.status?;
        Ok((self.knots, self.mults))
    }
}

/// The lists a caller that ignores `GetKnots`' status reads: what was written, also on a
/// failing status; empty when nothing was (an error before the out-parameters, as
/// `ignored`).
///
/// # Errors
///
/// [`SolidError::Unported`].
pub fn knots_ignoring_status(r: Result<KnotsWithStatus>) -> Result<(Vec<f64>, Vec<usize>)> {
    Ok(ignored(r)?.map_or_else(Default::default, |r| (r.knots, r.mults)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::ObjectKey;

    fn hermite() -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(3.0, 1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            3,
        ))
    }

    fn cubic() -> Curve {
        let pts = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, -1.0, 0.0),
            Vec3::new(3.0, 0.0, 1.0),
        ];
        let c = BSplineCurve::create_canonical(&CanonicalData {
            dim: 3,
            degree: 3,
            control_points: &pts,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &[4, 4],
            knots: &[0.0, 1.0],
            knot_type: KnotType::Unspecified,
            weights: None,
            trim: None,
        })
        .unwrap();
        Curve::from(c)
    }

    fn evaluation_owner(key: u64) -> ObjectRef {
        ObjectRef {
            type_code: EDGE_TYPE,
            key: ObjectKey(key),
        }
    }

    fn noncanonical_evaluation_curve(dim: u64) -> Curve {
        let mut b = BSplineCurve::create_canonical(&CanonicalData {
            dim,
            degree: 1,
            control_points: &[Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y],
            form: BSplineCurveForm::PolylineForm,
            knot_multiplicities: &[2, 1, 1, 2],
            knots: &[0.0, 0.25, 0.75, 1.0],
            knot_type: KnotType::UniformKnots,
            weights: None,
            trim: None,
        })
        .unwrap();
        b.get_or_create_gw_nurb()
            .unwrap()
            .knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.5, 0.5 + 1e-14, 1.0, 1.0]);
        b.set_out_of_bounds_enabled(true);
        // This fixture must expose the ordinary copy constructor's knot fix-up.
        assert_ne!(
            b.gw_nurb().unwrap().knots(),
            b.clone().gw_nurb().unwrap().knots()
        );
        let mut c = Curve::from(b);
        let _ = c.set_owner(Some(evaluation_owner(1)), None);
        c
    }

    fn evaluation_bspline(c: &Curve) -> &BSplineCurve {
        let CurveKind::BSpline(b) = c.kind() else {
            panic!("expected B-spline");
        };
        b
    }

    #[test]
    fn evaluation_copy_keeps_nested_knots_flags_and_owners() {
        use crate::curve::projected_curve::OwnsCurve;
        let mut offset = OffsetCurve::new(
            3,
            noncanonical_evaluation_curve(3),
            Extent1d::from_raw(0.0, 1.0),
            Vec3::Z,
            0.125,
            true,
        )
        .unwrap();
        let _ = offset.reverse_parameterization(&Extent1d::from_raw(0.0, 1.0));
        let mut offset = Curve::from(offset);
        let _ = offset.set_owner(Some(evaluation_owner(2)), None);
        let mut source = Curve::from(
            ProjectedCurve::new(
                offset,
                Some(Vec3::ZERO),
                Some(Vec3::Z),
                Some(Vec3::X),
                ProjectionType::Parallel,
                OwnsCurve::Unsure,
            )
            .unwrap(),
        );
        let _ = source.set_owner(Some(evaluation_owner(3)), None);
        let copy = source.clone_for_evaluation().unwrap();
        assert_eq!(copy.owner(), source.owner());
        assert_eq!(copy.type_code(), PROJECTED_CURVE_TYPE);
        let CurveKind::Projected(projected) = copy.kind() else {
            panic!("expected projection");
        };
        assert_eq!(projected.owns_curve(), OwnsCurve::Unsure);
        assert_eq!(projected.curve().owner(), Some(evaluation_owner(2)));
        let CurveKind::Offset(offset) = projected.curve().kind() else {
            panic!("expected offset");
        };
        assert_eq!(offset.reversed_flag(), 1);
        assert_eq!(offset.owns_curve(), 1);
        let base = offset.base_curve().unwrap();
        assert_eq!(base.owner(), Some(evaluation_owner(1)));
        let b = evaluation_bspline(base);
        assert_eq!(
            b.gw_nurb().unwrap().knots(),
            &[0.0, 0.0, 0.5, 0.5 + 1e-14, 1.0, 1.0]
        );
        assert!(b.is_out_of_bounds_enabled());
        assert_eq!(b.knot_type(), KnotType::UniformKnots);
        assert_eq!(b.form(), BSplineCurveForm::PolylineForm);
        let k = Kernel::default();
        for t in [0.125, 0.25, 0.75, 0.875] {
            assert_eq!(
                source.evaluate(&k, t, 1, true, false).unwrap(),
                copy.evaluate(&k, t, 1, true, false).unwrap()
            );
        }
        // The public copy-constructor semantics remain unchanged.
        assert_eq!(source.clone().owner(), None);
    }

    #[test]
    fn evaluation_copy_preserves_composite_segment_state() {
        let mut composite = CompositeCurve::with_curves(
            3,
            vec![
                noncanonical_evaluation_curve(3),
                Curve::from(OffsetCurve::empty()),
            ],
            true,
            Some(&[false, true]),
            Some(&[0.125, 0.25]),
        )
        .unwrap();
        // (set_segment takes a whole segment; a plain Clone would reset the curve's
        // owner and fix up its knots, so copy the curve exactly)
        let mut first = composite.segment(0).unwrap().clone();
        first.curve = composite
            .segment(0)
            .unwrap()
            .curve
            .clone_for_evaluation()
            .unwrap();
        first.transition = ContinuityType::C1;
        first.owns_curve = true;
        composite.set_segment(0, first).unwrap();
        let source = Curve::from(composite);
        let copy = source.clone_for_evaluation().unwrap();
        let CurveKind::Composite(composite) = copy.kind() else {
            panic!("expected composite");
        };
        assert!(composite.is_closed());
        let segment = &composite.segments()[0];
        assert_eq!(segment.transition, ContinuityType::C1);
        assert!(!segment.same_sense);
        assert!(segment.owns_curve);
        assert_eq!((segment.gap_start, segment.gap_end), (0.25, 0.125));
        assert_eq!(segment.curve.owner(), Some(evaluation_owner(1)));
        assert_eq!(
            evaluation_bspline(&segment.curve)
                .gw_nurb()
                .unwrap()
                .knots()[3],
            0.5 + 1e-14
        );
        let CurveKind::Offset(empty) = composite.segments()[1].curve.kind() else {
            panic!("expected empty offset");
        };
        assert!(empty.base_curve().is_none());
        assert_eq!((empty.owns_curve(), empty.reversed_flag()), (2, 2));
    }

    #[test]
    fn evaluation_copy_keeps_iso_and_curve_on_surface_data() {
        use crate::nurbs::b_spline_surface::BSplineSurface;
        use crate::surface::surface::{Surface, SurfaceKind};
        let mut patch = BSplineSurface::create_bilinear_surface(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec3::new(1.0, 1.0, 1.0),
        )
        .unwrap();
        patch.get_or_create_gw_nurb().unwrap().u_knots_mut()[1] = 1e-14;
        let mut surface = Surface::from(patch.clone_exact());
        let face = ObjectRef {
            type_code: crate::geom::assert_array::labels::FACE_TYPE,
            key: ObjectKey(4),
        };
        surface.set_owner(Some(face));
        surface.set_owner_zone_tol_3d(Some(0.125));
        let source = Curve::from(
            IsoCurve::from_surface(
                &Kernel::default(),
                surface,
                SurfParamType::U,
                0.25,
                false,
                false,
            )
            .unwrap(),
        );
        let copy = source.clone_for_evaluation().unwrap();
        let CurveKind::Iso(iso) = copy.kind() else {
            panic!("expected iso curve");
        };
        assert!(!iso.from_left());
        assert_eq!(iso.iso_parameter(), 0.25);
        assert_eq!(iso.surface().owner(), Some(face));
        assert_eq!(iso.surface().owner_zone_tol_3d(), Some(0.125));
        let SurfaceKind::BSpline(b) = iso.surface().kind() else {
            panic!("expected B-spline surface");
        };
        assert_eq!(b.gw_nurb().unwrap().u_knots()[1], 1e-14);
        let mut on_surface = CrvOnSurf::new(
            &Kernel::default(),
            noncanonical_evaluation_curve(2),
            Surface::from(patch),
            None,
            3,
        )
        .unwrap();
        on_surface.set_breaks_cache(true, vec![0.25, 0.75]);
        let copy = Curve::from(on_surface).clone_for_evaluation().unwrap();
        let CurveKind::CrvOnSurf(c) = copy.kind() else {
            panic!("expected curve on surface");
        };
        assert_eq!(c.owner_flag(), 3);
        assert!(c.need_breaks());
        assert_eq!(c.breaks(), &[0.25, 0.75]);
        assert_eq!(c.uv_curve().owner(), Some(evaluation_owner(1)));
        assert_eq!(
            evaluation_bspline(c.uv_curve()).gw_nurb().unwrap().knots()[3],
            0.5 + 1e-14
        );
        let SurfaceKind::BSpline(base) = c.surface().kind() else {
            panic!("expected B-spline surface");
        };
        assert_eq!(base.gw_nurb().unwrap().u_knots()[1], 1e-14);
    }

    #[test]
    fn evaluation_copy_keeps_and_bounds_volume_chains() {
        use crate::surface::volume::{Volume, VolumeKind};
        let mut volume = Volume::new(VolumeKind::Base);
        let _ = volume.set_owner(Some(evaluation_owner(5)));
        // the longest chain a curve in a volume may hold (the volume sits at depth 1)
        for _ in 0..crate::surface::surface::p2::MAX_GEOMETRY_NESTING - 1 {
            volume = Volume::with_maps(VolumeKind::Base, None, Some(volume)).unwrap();
        }
        let mut c = CrvInVolume::new(noncanonical_evaluation_curve(3), true, volume, 0, 7).unwrap();
        c.set_breaks_cache(true, vec![0.25], vec![ContinuityType::C1]);
        let source = Curve::from(c);
        let copy = source.clone_for_evaluation().unwrap();
        let CurveKind::CrvInVolume(c) = copy.kind() else {
            panic!("expected curve in volume");
        };
        assert_eq!(c.owner_flag(), 7);
        assert!(c.need_breaks());
        assert_eq!(c.breaks(), &[0.25]);
        assert_eq!(c.conts(), &[ContinuityType::C1]);
        assert_eq!(c.curve().owner(), Some(evaluation_owner(1)));
        assert_eq!(
            evaluation_bspline(c.curve()).gw_nurb().unwrap().knots()[3],
            0.5 + 1e-14
        );
        let mut last = c.volume();
        while let Some(next) = last.next_map() {
            last = next;
        }
        assert_eq!(last.owner(), Some(evaluation_owner(5)));
        // One more wrapper would nest past the bound: construction refuses it, so a
        // too-deep chain never reaches the evaluation copy (or its drop).
        assert!(matches!(
            OffsetCurve::new(3, source, Extent1d::from_raw(0.0, 1.0), Vec3::Z, 0.0, true),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn evaluation_copy_depth_bound_spans_curve_and_surface_branches() {
        use crate::surface::surf_of_extrusion::SurfOfExtrusion;
        use crate::surface::surface::p2::MAX_GEOMETRY_NESTING;
        use crate::surface::surface::Surface;
        // Alternate curve and surface wrappers (two nesting levels per iso curve over
        // an extrusion) until the iso constructor refuses, then top up with offsets
        // until those refuse: construction bounds the mixed chain at the shared limit.
        let mut curve = hermite();
        let mut levels = 0;
        loop {
            let mut s = SurfOfExtrusion::empty();
            s.gen_curve = Some(curve.clone());
            let k = Kernel::default();
            match IsoCurve::from_surface(&k, Surface::from(s), SurfParamType::U, 0.0, true, false) {
                Ok(iso) => curve = Curve::from(iso),
                Err(e) => {
                    assert!(matches!(e, SolidError::InvalidInput { .. }));
                    break;
                }
            }
            levels += 2;
            assert!(levels <= MAX_GEOMETRY_NESTING);
        }
        loop {
            match OffsetCurve::new(
                3,
                curve.clone(),
                Extent1d::from_raw(0.0, 1.0),
                Vec3::Z,
                0.0,
                true,
            ) {
                Ok(off) => curve = Curve::from(off),
                Err(e) => {
                    assert!(matches!(e, SolidError::InvalidInput { .. }));
                    break;
                }
            }
            levels += 1;
            assert!(levels <= MAX_GEOMETRY_NESTING);
        }
        // the deepest buildable chain still copies; one level more is refused above
        assert!(curve.clone_for_evaluation().is_ok());
        // Exercise extreme initial counters without overflowing depth + 1.
        assert!(hermite().clone_for_evaluation_depth(usize::MAX).is_err());
    }

    #[test]
    fn dispatcher_rejects_huge_derivative_orders() {
        // fuzz (curve_p2, offset_curve, projected_curve): a Hermite kind evaluated with
        // `n = 2^31` / `usize::MAX` sized the output allocation
        let h = hermite();
        for n in [crate::curve::MAX_EVAL_DERIVS + 1, 1 << 31, usize::MAX] {
            assert!(matches!(
                h.evaluate(&Kernel::default(), 0.5, n, true, true),
                Err(SolidError::InvalidInput { .. })
            ));
        }
        assert_eq!(
            h.evaluate(&Kernel::default(), 0.5, 6, true, true)
                .unwrap()
                .len(),
            7
        );
    }

    #[test]
    fn type_queries() {
        let k = &Kernel::default();
        let h = hermite();
        assert_eq!(h.type_code(), HERMITE_CURVE_TYPE);
        assert!(h.is_kind_of(TYPE) && h.is_kind_of(HERMITE_CURVE_TYPE));
        assert!(!h.is_kind_of(BSPLINE_CURVE_TYPE));
        assert!(is_kind_of(a_object::TYPE));
        assert_eq!(h.degree().unwrap(), 3);
        assert_eq!(h.number_control_points().unwrap(), 0);
        assert_eq!(h.number_natural_knots(k).unwrap(), 2);
        assert!(!h.is_rational().unwrap());
        let b = cubic();
        assert!(b.approx_eq(k, &b.clone()).unwrap() && !b.approx_eq(k, &h).unwrap());
        let k = Kernel::default();
        assert!(h.copy(&k).is_err() && b.copy(&k).is_ok());
    }

    #[test]
    fn owner_links() {
        let mut c = cubic();
        let edge = ObjectRef {
            type_code: EDGE_TYPE,
            key: ObjectKey(7),
        };
        let (prev, change) = c.set_owner(Some(edge), None);
        assert!(prev.is_none() && change.is_some());
        assert_eq!(c.edge(|_| None).unwrap(), Some(edge));
        assert!(c.clone().owner().is_none());

        // a generated curve owned by a curve owned by the edge
        let owner_curve = ObjectRef {
            type_code: BSPLINE_CURVE_TYPE,
            key: ObjectKey(3),
        };
        let mut g = hermite();
        let _ = g.set_owner(Some(owner_curve), None);
        assert_eq!(
            g.edge(|o| (o == owner_curve).then_some(edge)).unwrap(),
            Some(edge)
        );
        // a cycle is an error
        assert!(g.edge(|_| Some(owner_curve)).is_err());
    }

    #[test]
    fn length_and_step_off() {
        let k = &Kernel::default();
        let b = cubic();
        let ivl = b.natural_interval();
        let (len, turn) = b.approximate_length(k, &ivl, 64, false).unwrap();
        assert!(len > 3.0 && turn.is_none());
        let s = euclidian_step_off(k, &b, &ivl, 0.0, true, 1.0, 1e-10).unwrap();
        assert!(s.converged);
        let p0 = b.evaluate_point(k, 0.0).unwrap();
        let p1 = b.evaluate_point(k, s.end_param).unwrap();
        assert!((p0.distance(p1) - 1.0).abs() < 1e-8);
    }

    #[test]
    fn continuity_of_a_curve_with_itself() {
        let k = &Kernel::default();
        let b = cubic();
        let c = evaluate_continuity(k, &b, 0.5, true, &b, 0.5, false, 0.5).unwrap();
        assert!(c.continuity as i32 >= ContinuityType::C1 as i32);
    }

    #[test]
    fn stacks_and_bezier_assembly() {
        let mut stack = Vec::new();
        push_point(
            1.0,
            [Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)],
            &mut stack,
        );
        push_point(2.0, [Vec3::ZERO, Vec3::ZERO], &mut stack);
        assert_eq!(stack_look(&stack).unwrap().0, 2.0);
        assert_eq!(pop_stack(&mut stack).unwrap().0, 2.0);
        let (t, pv) = pop_stack(&mut stack).unwrap();
        assert_eq!((t, pv[1].z), (1.0, 6.0));
        assert!(pop_stack(&mut stack).is_err());

        let mut poly = Vec::new();
        let mut knots = Vec::new();
        let a = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)];
        let b = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)];
        let c = [Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)];
        add_to_curve(a, b, 0.0, 1.0, &mut poly, &mut knots).unwrap();
        add_to_curve(b, c, 1.0, 2.0, &mut poly, &mut knots).unwrap();
        assert!(add_to_curve(a, b, 5.0, 6.0, &mut poly, &mut knots).is_err());
        assert_eq!((poly.len(), knots.len()), (7, 3));
        let crv = flush_crv(1e-6, &mut poly, &mut knots, true)
            .unwrap()
            .unwrap();
        assert!(poly.is_empty() && knots.is_empty());
        assert!((crv.evaluate_point(1.5).unwrap().x - 1.5).abs() < 1e-12);
    }

    #[test]
    fn test_params_and_steps() {
        assert_eq!(find_test_params(0.0, 1.0, &[]), vec![0.0, 1.0]);
        let p = find_test_params(0.0, 2.0, &[0.0, 1.0, 2.0]);
        assert_eq!(p.len(), 8);
        let r = test_step(
            1.0,
            [Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0)],
            [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)],
            0.0,
            10.0,
            0.0,
        );
        assert_eq!(r, TessStepResult::Cut);
        let (u, l) = unitize_tangent(
            [Vec3::ZERO, Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0)],
            Vec3::new(0.0, 1.0, 0.0),
        );
        assert_eq!((u, l), (Vec3::new(0.0, 1.0, 0.0), 0.0));
        assert_eq!(
            proj_signed_angle(
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::ZERO
            )
            .unwrap(),
            BIG_DOUBLE
        );
    }

    #[test]
    fn split_and_surface_distance() {
        let k = Kernel::default();
        let mut b = cubic();
        let tail = b.create_by_split_curve(&k, 0.5).unwrap();
        assert_eq!(b.natural_interval().max(), 0.5);
        assert_eq!(tail.natural_interval().min(), 0.5);
        let mut h = hermite();
        assert!(h.create_by_split_curve(&k, 0.5).is_err());
        assert!(h.create_by_split_curve(&k, 1.0).is_err());
        let srf = crate::nurbs::b_spline_surface::BSplineSurface::create_bilinear_surface(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        )
        .unwrap();
        let line = Curve::from(
            Line::create_line_segment(3, Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0), None)
                .unwrap(),
        );
        let gap = line
            .surface_max_distance_between(
                &Kernel::default(),
                &Extent1d::from_min_max(0.0, 1.0),
                &srf,
                &srf.natural_uv_domain(),
                2,
                None,
            )
            .unwrap();
        assert_eq!(gap.max_distance, 1.0);
    }

    /// Curve handles that cannot report their curve's natural interval (as
    /// `FilletObject` and `LoopObject` cannot) still get `IdenticalSolutions`' interval
    /// from the curve their tree decomposes, as upstream reads it from the `SmCurve`:
    /// a curve/curve intersection naming them merges as one naming interval-carrying
    /// handles.
    #[test]
    fn curve_intersection_reads_bare_handles_intervals_from_the_trees() {
        use crate::curve::curve::p8::CurveIntersectOptions;
        use crate::geom::solution_array::ObjectKind;

        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Bare {
            A,
            B,
        }
        impl SolutionObject for Bare {
            fn kind(&self) -> ObjectKind {
                ObjectKind::Curve
            }
        }
        #[derive(Debug, Clone, Copy, PartialEq)]
        struct Named(u8, Extent1d);
        impl SolutionObject for Named {
            fn kind(&self) -> ObjectKind {
                ObjectKind::Curve
            }
            fn natural_interval(&self) -> Option<Extent1d> {
                Some(self.1)
            }
        }

        fn params<O>(sols: &[Solution<O>]) -> Vec<(u64, u64, u64)> {
            sols.iter()
                .map(|s| {
                    (
                        s.start.parameters[0].to_bits(),
                        s.start.parameters[1].to_bits(),
                        s.start.solution_value.to_bits(),
                    )
                })
                .collect()
        }

        // two planar cubic waves, y = +-c * 6t(1-t)(1-2t) over x = 3t: they cross at
        // t = 0, 1/2 and 1 (several solutions naming the same pair of curves)
        let wave = |c: f64| {
            let pts = [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 2.0 * c, 0.0),
                Vec3::new(2.0, -2.0 * c, 0.0),
                Vec3::new(3.0, 0.0, 0.0),
            ];
            Curve::from(
                BSplineCurve::create_canonical(&CanonicalData {
                    dim: 3,
                    degree: 3,
                    control_points: &pts,
                    form: BSplineCurveForm::Unspecified,
                    knot_multiplicities: &[4, 4],
                    knots: &[0.0, 1.0],
                    knot_type: KnotType::Unspecified,
                    weights: None,
                    trim: None,
                })
                .unwrap(),
            )
        };
        let (a, b) = (wave(1.0), wave(-0.5));
        let k = Kernel::default();
        let (ia, ib) = (a.natural_interval(), b.natural_interval());
        let named = a
            .global_curve_intersect(
                &k,
                &ia,
                &b,
                &ib,
                1e-6,
                &CurveIntersectOptions {
                    handles: [Some(Named(0, ia)), Some(Named(1, ib))],
                    ..CurveIntersectOptions::default()
                },
            )
            .unwrap();
        let bare = a
            .global_curve_intersect(
                &k,
                &ia,
                &b,
                &ib,
                1e-6,
                &CurveIntersectOptions {
                    handles: [Some(Bare::A), Some(Bare::B)],
                    ..CurveIntersectOptions::default()
                },
            )
            .unwrap();
        assert!(named.len() >= 2, "{named:?}");
        assert_eq!(params(bare.as_slice()), params(named.as_slice()));
    }
}
