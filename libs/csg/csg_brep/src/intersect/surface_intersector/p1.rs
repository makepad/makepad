//! `SmSurfaceIntersector`, part 1: the intersection point [`TsectPnt`] (`SmTsectPnt`)
//! and the intersector object [`SurfaceIntersector`] with its construction, point
//! lists (start, through and curve points), step size, span accuracy test, point
//! refinement, curve reversal and the "is this point already found" predicates, plus
//! the file-static helpers `sm_ComputeCurveType`, `sm_IsPointOnCurve` and
//! `sm_FlushCrv`.
//!
//! Upstream: `source/SMLib/src/SmSurfaceIntersector.cpp`,
//! `source/SMLib/inc/SmSurfaceIntersector.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`IntersectionPointType`]                         | `SmIntersectionPointType`                        |
//! | [`TsectPnt`], [`TsectPnt::new`], `Clone`          | `SmTsectPnt`, its default and copy constructors  |
//! | `Drop` (implicit)                                 | `SmTsectPnt::~SmTsectPnt`                        |
//! | [`TsectPnt::assign`]                              | `SmTsectPnt::operator=`                          |
//! | [`TsectPnt::is_bounded`]                          | `SmTsectPnt::IsBounded`                          |
//! | [`TsectPnt::classify_point_params`]               | `SmTsectPnt::ClassifyPointParams`                |
//! | [`TsectPnt::point_type`], [`TsectPnt::u_param_type`], [`TsectPnt::v_param_type`], [`TsectPnt::uv_pos`], [`TsectPnt::uv_deriv`], [`TsectPnt::srf_pos`], [`TsectPnt::srf_du`], [`TsectPnt::srf_dv`], [`TsectPnt::srf_duu`], [`TsectPnt::srf_duv`], [`TsectPnt::srf_dvv`], [`TsectPnt::srf_norm`], [`TsectPnt::crv_pos`], [`TsectPnt::crv_deriv`], [`TsectPnt::srf_eval_matrix`] (and `_mut` setters) | `PointType`, `UParamType`, `VParamType`, `UVPos`, `UVDeriv`, `SrfPos`, `SrfDu`, `SrfDv`, `SrfDuu`, `SrfDuv`, `SrfDvv`, `SrfNorm`, `CrvPos`, `CrvDeriv`, `SrfEvalMatrix` |
//! | [`SurfaceIntersector`], [`SurfaceIntersector::new`] | `SmSurfaceIntersector`, its constructor        |
//! | `Drop` (implicit)                                 | `~SmSurfaceIntersector`                          |
//! | [`SurfaceIntersector::surface`], [`SurfaceIntersector::this_approx_tol_3d`], [`SurfaceIntersector::this_ang_tol_rad`] | `GetSurface`, `GetThisApproxTol3d`, `GetThisAngTolRad` |
//! | [`SurfaceIntersector::set_extension_distance`]    | `SetExtensionDistance`                           |
//! | [`SurfaceIntersector::set_use_surface_edges`]     | `SetUseSurfaceEdges`                             |
//! | [`SurfaceIntersector::compute_point_values_delta`] | `ComputePointValuesDelta` (inline `SM_ERR`)     |
//! | [`SurfaceIntersector::step_back_from_singularity`] | `StepBackFromSingularity`                       |
//! | [`SurfaceIntersector::compute_singularity_point`] | `ComputeSingularityPoint`                        |
//! | [`SurfaceIntersector::compute_step_size`]         | `ComputeStepSize`                                |
//! | [`SurfaceIntersector::refine_point`]              | `RefinePoint`                                    |
//! | [`SurfaceIntersector::test_span_accuracy`], [`SpanAccuracy`] | `TestSpanAccuracy`                    |
//! | [`SurfaceIntersector::add_point_to_curve`]        | `AddPointToCurve`                                |
//! | [`SurfaceIntersector::add_start_point`]           | `AddStartPoint`                                  |
//! | [`SurfaceIntersector::is_same_tangent_cluster`], [`IntersectionPointType::is_tangent_type`], `tangent_start_rank` (private) | the source fork's `IsSameTangentCluster`, `sm_IsTangentType`, `sm_TangentStartRank` (opt-in, WR-76) |
//! | [`SurfaceIntersector::add_through_point`]         | `AddThroughPoint`                                |
//! | [`SurfaceIntersector::reverse_curve_direction`]   | `ReverseCurveDirection`                          |
//! | [`SurfaceIntersector::is_point_on_curve`]         | `IsPointOnCurve`                                 |
//! | [`SurfaceIntersector::is_point_on_start_point`]   | `IsPointOnStartPoint`                            |
//! | [`SurfaceIntersector::evaluate_law_point`]        | `EvaluateLawPoint`                               |
//! | [`compute_curve_type`]                            | `sm_ComputeCurveType` (static)                   |
//! | [`is_point_on_tsect_curve`]                       | `sm_IsPointOnCurve` (static)                     |
//! | [`flush_crv`]                                     | `sm_FlushCrv` (static)                           |
//!
//! Deviations:
//! - **Point storage.** Upstream allocates `SmTsectPnt`s from the block manager
//!   `m_vTSPntMgr` and threads them through the intrusive circular lists
//!   `m_vCurvePoints`, `m_vStartPoints` and `m_vThroughPoints`. Here the intersector
//!   owns an arena of points ([`SurfaceIntersector::alloc_point`], addressed by
//!   [`TsectId`]) and each list is a `Vec` of ids in list order (`Append` pushes,
//!   `Prepend` inserts at the front). `m_pStartPoint` / `m_pEndPoint` are optional
//!   ids. Linking a point that is already on a list (undefined upstream: it corrupts
//!   both lists) is rejected with [`SolidError::InvalidInput`]. The list links of
//!   `SmListNode` are not part of [`TsectPnt`], so [`TsectPnt::assign`] does not
//!   clear them as `operator=` does.
//! - **Surface index.** The accessors and [`SurfaceIntersector::step_back_from_singularity`]
//!   take a [`SrfIndex`] instead of a `ULONG` (upstream indexes out of bounds for
//!   values above 1).
//! - **Unused members.** `m_apUserPointer` (not used by SMLib), `m_cpContext` (never
//!   set) and the `SmGlobalSolver` base part (the settings the constructor writes
//!   are used by none of these methods; the solver is set up by the unit that drives
//!   it) are not modelled.
//! - **Arguments.** Out-params become return values. `ComputeStepSize`'s unused point,
//!   `TestSpanAccuracy`'s unused mid point and `EvaluateLawPoint`'s unset curve
//!   parameter are dropped. [`SurfaceIntersector::evaluate_law_point`] takes the
//!   law as `&mut FilletLaw`: upstream's `const SmFilletLaw&` caches a B-spline law's
//!   extension through a `const` cast.
//!   An out-param upstream leaves unset is `None` ([`SpanAccuracy::deviation_found`]).
//! - **Boolean methods.** `IsPointOnCurve` and `sm_IsPointOnCurve` return a
//!   `SmBoolean` but use `SER`: a failing call returns its (non-zero) status, which
//!   the caller reads as `TRUE`. The port returns `Ok(true)` there.
//! - **`AddStartPoint`** keeps upstream's "replace a bounded start point by the exact
//!   one" branch, which only reassigns a local pointer: the exact candidate is
//!   dropped and the bounded point stays.
//! - **Opt-in fix (WR-76, #392).** With
//!   [`UpstreamFixes::ssi_trace_tangent_curves_once`](crate::kernel::UpstreamFixes::ssi_trace_tangent_curves_once)
//!   `AddStartPoint` keeps one start point per tangent cluster
//!   ([`SurfaceIntersector::is_same_tangent_cluster`]): a candidate in a cluster
//!   replaces the start point it clusters with when it ranks better and is dropped
//!   otherwise (the source fork's source fork@0ce09f2). Off by default
//!   (upstream behaviour).
//! - **Surface calls.** `SmSurface::IsClosed`, `SmSurface::EvaluateNormal` and
//!   `SmSurface::Evaluate1stDerivatives` go through `surface::surface::p7` (the
//!   last co-ported there from unit `surface.surface_p1`).
//! - **Offset distances.** `EvaluateLawPoint` sets the offset distances of the
//!   intersector's surfaces, which it shares with its owner as upstream does
//!   (`OffsetSurface::set_offset_distance` takes `&self`). Statuses upstream ignores
//!   are ignored, except `Unported`, which always propagates.
//! - **Not ported here.** `SmTsectPnt::Dump` / `Draw` (debug output) and the
//!   `SM_DEBUG_CODE` blocks (graphics and jump statistics).

use crate::blend::fillet_standard_solver::p1::FilletLaw;
use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::p6::DropPointCaches;
use crate::curve::curve::Curve;
use crate::curve::hermite_curve::HermiteCurve;
use crate::error::{Result, SolidError, SM_ERR_NULL_POINTER};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::tol::is_zero;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, max3, min2, EFF_ZERO, EFF_ZERO_RAD, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm, CanonicalData, KnotType};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::TsectCurveType;
use crate::surface::surface::{Surface, SurfaceKind};

const OP_POINT: &str = "intersect::surface_intersector::p1::SurfaceIntersector::point";
const OP_LIST: &str = "intersect::surface_intersector::p1::SurfaceIntersector::add_to_list";
const OP_DELTA: &str =
    "intersect::surface_intersector::p1::SurfaceIntersector::compute_point_values_delta";
const OP_SING: &str =
    "intersect::surface_intersector::p1::SurfaceIntersector::compute_singularity_point";
const OP_SPAN: &str = "intersect::surface_intersector::p1::SurfaceIntersector::test_span_accuracy";
const OP_REVERSE: &str =
    "intersect::surface_intersector::p1::SurfaceIntersector::reverse_curve_direction";
const OP_LAW: &str = "intersect::surface_intersector::p1::SurfaceIntersector::evaluate_law_point";

/// `SM_MAX_USER_DOUBLES`.
pub const MAX_USER_DOUBLES: usize = 4;

/// `NUM_TEST_POINTS` of `TestSpanAccuracy`.
const NUM_SPAN_TEST_POINTS: usize = 5;

// ---- intersection point ------------------------------------------------------------------

/// The type of an intersection point (`SmIntersectionPointType`); the order is
/// upstream's (some code compares ranks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum IntersectionPointType {
    /// `SM_IP_UNDEFINED`: cleared value of a deleted point.
    Undefined = 0,
    /// `SM_IP_UNKNOWN`.
    #[default]
    Unknown = 1,
    /// `SM_IP_CROSSING`: the surface normals are not parallel.
    Crossing = 2,
    /// `SM_IP_TANGENT_POINT`: the surfaces touch at the point.
    TangentPoint = 3,
    /// `SM_IP_TANGENT_CURVE`: the point is on a tangent curve.
    TangentCurve = 4,
    /// `SM_IP_SINGULARITY`: the nexus of 4 or 6 intersection curves.
    Singularity = 5,
    /// `SM_IP_COINCIDENCE`: the second fundamental forms match.
    Coincidence = 6,
    /// `SM_IP_TOUCHING`: the only point of intersection of two natural boundaries.
    Touching = 7,
}

impl IntersectionPointType {
    /// A tangent point, tangent curve or coincidence point (the fork's
    /// `sm_IsTangentType`, opt-in fix WR-76, #392).
    #[must_use]
    pub const fn is_tangent_type(self) -> bool {
        matches!(
            self,
            Self::TangentPoint | Self::TangentCurve | Self::Coincidence
        )
    }

    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The type of an upstream enum value.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => Self::Undefined,
            1 => Self::Unknown,
            2 => Self::Crossing,
            3 => Self::TangentPoint,
            4 => Self::TangentCurve,
            5 => Self::Singularity,
            6 => Self::Coincidence,
            7 => Self::Touching,
            _ => return None,
        })
    }
}

/// Which of the two intersected surfaces (upstream `ULONG lSrf`, 0 or 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrfIndex {
    /// `m_cpSurface[0]`.
    First,
    /// `m_cpSurface[1]`.
    Second,
}

impl SrfIndex {
    /// The upstream index (0 or 1).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
        }
    }
}

const fn pick<T>(a: &[T; 2], s: SrfIndex) -> &T {
    match s {
        SrfIndex::First => &a[0],
        SrfIndex::Second => &a[1],
    }
}

fn pick_mut<T>(a: &mut [T; 2], s: SrfIndex) -> &mut T {
    match s {
        SrfIndex::First => &mut a[0],
        SrfIndex::Second => &mut a[1],
    }
}

/// An intersection point (`SmTsectPnt`): a point on both surfaces with its UV
/// positions and tangents, both surfaces' evaluation matrices and the 3D position and
/// tangent of the intersection curve.
#[derive(Debug, Clone, PartialEq)]
pub struct TsectPnt {
    /// `m_ePointType`.
    pub point_type: IntersectionPointType,
    /// `m_eUVParamType[srf][0 = u, 1 = v]`: the UV point against the target domain.
    pub uv_param_type: [[ExtentPointType; 2]; 2],
    /// `m_vUVCurvePV[srf][0 = uv position, 1 = uv tangent]`.
    pub uv_curve_pv: [[Vec2; 2]; 2],
    /// `m_vSurfacePV[srf][i][j]`: `[0][0]` position, `[1][0]` `Su`, `[0][1]` `Sv`,
    /// `[2][0]` `Suu`, `[1][1]` `Suv`, `[0][2]` `Svv`, `[2][2]` normal; `[1][2]` and
    /// `[2][1]` are unused.
    pub surface_pv: [[[Vec3; 3]; 3]; 2],
    /// `m_v3DCurvePV`: intersection curve position and tangent.
    pub curve_pv: [Vec3; 2],
    /// `m_dCurveParameter`.
    pub curve_parameter: f64,
    /// `m_dTangentPlaneAngleRad`: angle between the surface normals.
    pub tangent_plane_angle_rad: f64,
    /// `m_dDeviation`: largest distance to each surface.
    pub deviation: f64,
    /// `m_adUserDoubles`: per-algorithm scratch values.
    pub user_doubles: [f64; MAX_USER_DOUBLES],
    /// `m_apUserPointer[0..2]` (`None`: `NULL`). SMLib's only writer
    /// (`SmFilletGeom::MakeRailEdgeuse`) stores surfaces in them and its only reader
    /// (`SmFilletNx2Corner::AdjustTangentCorner`) reads them back as surfaces.
    pub user_pointer: [Option<UserSurface>; 2],
}

/// A surface held by an intersection point's user pointer (`(void*)pSrf`). Copies of
/// the point share it; two user pointers are equal when they hold the same surface
/// (pointer identity, as upstream compares `void*`).
#[derive(Debug, Clone)]
pub struct UserSurface(pub std::sync::Arc<Surface>);

impl PartialEq for UserSurface {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for TsectPnt {
    fn default() -> Self {
        Self::new()
    }
}

/// `m_eUVParamType[s][t]` is a natural boundary classification (start, end or both).
const fn is_boundary_type(t: ExtentPointType) -> bool {
    matches!(
        t,
        ExtentPointType::Start | ExtentPointType::End | ExtentPointType::Both
    )
}

impl TsectPnt {
    /// The default point (`SmTsectPnt()`): type unknown, parameters zero, vectors
    /// uninitialised ([`Vec3::UNDEF`], the `SmVector3d` member initialisers).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            point_type: IntersectionPointType::Unknown,
            uv_param_type: [[ExtentPointType::Unknown; 2]; 2],
            uv_curve_pv: [[Vec2::UNDEF; 2]; 2],
            surface_pv: [[[Vec3::UNDEF; 3]; 3]; 2],
            curve_pv: [Vec3::UNDEF; 2],
            curve_parameter: 0.0,
            tangent_plane_angle_rad: 0.0,
            deviation: 0.0,
            user_doubles: [0.0; MAX_USER_DOUBLES],
            user_pointer: [None, None],
        }
    }

    /// `operator=`: copies everything but the unused `[1][2]` and `[2][1]` entries of
    /// the evaluation matrices.
    pub fn assign(&mut self, other: &Self) {
        if core::ptr::eq(self, other) {
            return;
        }
        self.point_type = other.point_type;
        self.uv_param_type = other.uv_param_type;
        self.uv_curve_pv = other.uv_curve_pv;
        for (dst, src) in self.surface_pv.iter_mut().zip(other.surface_pv.iter()) {
            dst[0] = src[0];
            dst[1][0] = src[1][0];
            dst[1][1] = src[1][1];
            dst[2][0] = src[2][0];
            dst[2][2] = src[2][2];
        }
        self.curve_pv = other.curve_pv;
        self.curve_parameter = other.curve_parameter;
        self.tangent_plane_angle_rad = other.tangent_plane_angle_rad;
        self.deviation = other.deviation;
        self.user_doubles = other.user_doubles;
        self.user_pointer.clone_from(&other.user_pointer);
    }

    /// True when either UV point is classified on a natural domain boundary or
    /// outside it (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.uv_param_type
            .iter()
            .flatten()
            .any(|&t| t != ExtentPointType::Inside && t != ExtentPointType::Unknown)
    }

    /// Classifies the UV points `uvs` against the target domains `uv_domains`
    /// (`ClassifyPointParams`; upstream default `tol_3d = SM_EFF_ZERO`). The UV
    /// tolerance of a surface is `tol_3d / max(0.99, |S_u|)` (resp. `S_v`, from this
    /// point's evaluation matrix) capped at a tenth of the domain; without a surface
    /// it is that tenth. Only the presence of `surfaces` matters.
    pub fn classify_point_params(
        &mut self,
        uvs: [Point2; 2],
        surfaces: [Option<&Surface>; 2],
        uv_domains: &[Extent2d; 2],
        tol_3d: f64,
    ) {
        let [uv0, uv1] = uvs;
        let [s0, s1] = surfaces;
        let [d0, d1] = uv_domains;
        self.uv_param_type[0] = classify_one(
            uv0,
            s0.is_some(),
            d0,
            self.surface_pv[0][1][0],
            self.surface_pv[0][0][1],
            tol_3d,
        );
        self.uv_param_type[1] = classify_one(
            uv1,
            s1.is_some(),
            d1,
            self.surface_pv[1][1][0],
            self.surface_pv[1][0][1],
            tol_3d,
        );
    }

    /// `PointType()`.
    #[must_use]
    pub const fn point_type(&self) -> IntersectionPointType {
        self.point_type
    }

    /// `UParamType(srf)`.
    #[must_use]
    pub const fn u_param_type(&self, srf: SrfIndex) -> ExtentPointType {
        pick(&self.uv_param_type, srf)[0]
    }

    /// `VParamType(srf)`.
    #[must_use]
    pub const fn v_param_type(&self, srf: SrfIndex) -> ExtentPointType {
        pick(&self.uv_param_type, srf)[1]
    }

    /// `UVPos(srf)`.
    #[must_use]
    pub const fn uv_pos(&self, srf: SrfIndex) -> Point2 {
        pick(&self.uv_curve_pv, srf)[0]
    }

    /// `UVPos(srf)`, mutable.
    pub fn uv_pos_mut(&mut self, srf: SrfIndex) -> &mut Point2 {
        &mut pick_mut(&mut self.uv_curve_pv, srf)[0]
    }

    /// `UVDeriv(srf)`.
    #[must_use]
    pub const fn uv_deriv(&self, srf: SrfIndex) -> Vec2 {
        pick(&self.uv_curve_pv, srf)[1]
    }

    /// `UVDeriv(srf)`, mutable.
    pub fn uv_deriv_mut(&mut self, srf: SrfIndex) -> &mut Vec2 {
        &mut pick_mut(&mut self.uv_curve_pv, srf)[1]
    }

    /// `SrfPos(srf)`.
    #[must_use]
    pub const fn srf_pos(&self, srf: SrfIndex) -> Point3 {
        pick(&self.surface_pv, srf)[0][0]
    }

    /// `SrfPos(srf)`, mutable.
    pub fn srf_pos_mut(&mut self, srf: SrfIndex) -> &mut Point3 {
        &mut pick_mut(&mut self.surface_pv, srf)[0][0]
    }

    /// `SrfDu(srf)`.
    #[must_use]
    pub const fn srf_du(&self, srf: SrfIndex) -> Vec3 {
        pick(&self.surface_pv, srf)[1][0]
    }

    /// `SrfDu(srf)`, mutable.
    pub fn srf_du_mut(&mut self, srf: SrfIndex) -> &mut Vec3 {
        &mut pick_mut(&mut self.surface_pv, srf)[1][0]
    }

    /// `SrfDv(srf)`.
    #[must_use]
    pub const fn srf_dv(&self, srf: SrfIndex) -> Vec3 {
        pick(&self.surface_pv, srf)[0][1]
    }

    /// `SrfDv(srf)`, mutable.
    pub fn srf_dv_mut(&mut self, srf: SrfIndex) -> &mut Vec3 {
        &mut pick_mut(&mut self.surface_pv, srf)[0][1]
    }

    /// `SrfDuu(srf)`.
    #[must_use]
    pub const fn srf_duu(&self, srf: SrfIndex) -> Vec3 {
        pick(&self.surface_pv, srf)[2][0]
    }

    /// `SrfDuv(srf)`.
    #[must_use]
    pub const fn srf_duv(&self, srf: SrfIndex) -> Vec3 {
        pick(&self.surface_pv, srf)[1][1]
    }

    /// `SrfDvv(srf)`.
    #[must_use]
    pub const fn srf_dvv(&self, srf: SrfIndex) -> Vec3 {
        pick(&self.surface_pv, srf)[0][2]
    }

    /// `SrfNorm(srf)`.
    #[must_use]
    pub const fn srf_norm(&self, srf: SrfIndex) -> Vec3 {
        pick(&self.surface_pv, srf)[2][2]
    }

    /// `CrvPos()`.
    #[must_use]
    pub const fn crv_pos(&self) -> Point3 {
        self.curve_pv[0]
    }

    /// `CrvPos()`, mutable.
    pub fn crv_pos_mut(&mut self) -> &mut Point3 {
        &mut self.curve_pv[0]
    }

    /// `CrvDeriv()`.
    #[must_use]
    pub const fn crv_deriv(&self) -> Vec3 {
        self.curve_pv[1]
    }

    /// `CrvDeriv()`, mutable.
    pub fn crv_deriv_mut(&mut self) -> &mut Vec3 {
        &mut self.curve_pv[1]
    }

    /// `SrfEvalMatrix(srf)`: the evaluation matrix of a surface.
    #[must_use]
    pub const fn srf_eval_matrix(&self, srf: SrfIndex) -> &[[Vec3; 3]; 3] {
        pick(&self.surface_pv, srf)
    }

    /// `SrfEvalMatrix(srf)`, mutable.
    pub fn srf_eval_matrix_mut(&mut self, srf: SrfIndex) -> &mut [[Vec3; 3]; 3] {
        pick_mut(&mut self.surface_pv, srf)
    }

    /// Both bounded-intersection tests of `AddStartPoint`: the deviation exceeds
    /// `scaled_zero` and both UV points are on a natural boundary.
    fn is_bounded_start(&self, scaled_zero: f64) -> bool {
        let [[u0, v0], [u1, v1]] = self.uv_param_type;
        self.deviation > scaled_zero
            && (is_boundary_type(u0) || is_boundary_type(v0))
            && (is_boundary_type(u1) || is_boundary_type(v1))
    }
}

/// One surface's half of `ClassifyPointParams`.
fn classify_one(
    uv: Point2,
    has_surface: bool,
    domain: &Extent2d,
    du: Vec3,
    dv: Vec3,
    tol_3d: f64,
) -> [ExtentPointType; 2] {
    let u_ivl = domain.u_interval();
    let v_ivl = domain.v_interval();
    let u_band = u_ivl.length() / 10.0;
    let v_band = v_ivl.length() / 10.0;

    let (mut tol_u, mut tol_v);
    if has_surface {
        let mut u_size = du.length();
        if u_size < EFF_ZERO {
            u_size = EFF_ZERO;
        }
        let mut v_size = dv.length();
        if v_size < EFF_ZERO {
            v_size = EFF_ZERO;
        }
        tol_u = min2(u_band, tol_3d / max2(0.99, u_size));
        tol_v = min2(v_band, tol_3d / max2(0.99, v_size));
    } else {
        tol_u = u_band;
        tol_v = v_band;
    }
    if tol_u < EFF_ZERO {
        tol_u = EFF_ZERO;
    }
    if tol_v < EFF_ZERO {
        tol_v = EFF_ZERO;
    }
    [
        u_ivl.classify_point(uv.x, tol_u),
        v_ivl.classify_point(uv.y, tol_v),
    ]
}

// ---- the intersector -----------------------------------------------------------------------

/// The index of a [`TsectPnt`] in a [`SurfaceIntersector`]'s point arena (upstream
/// an `SmTsectPnt*` from `m_vTSPntMgr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TsectId(pub usize);

impl TsectId {
    /// The arena index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}

/// The result of [`SurfaceIntersector::test_span_accuracy`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanAccuracy {
    /// `rbFoundGoodPoint`: the span passed the angle and distance tests.
    pub found_good_point: bool,
    /// `rdDeviationFound`: the largest curve/surface distance of the test points;
    /// `None` when upstream leaves it unset (the span failed the angle test).
    pub deviation_found: Option<f64>,
    /// `rdAngleFoundRad`: the angle between the two curve tangents.
    pub angle_found_rad: f64,
}

/// The surface/surface intersector (`SmSurfaceIntersector`): two surfaces with
/// their target domains, the tracing tolerances and state, and the intersection
/// points it collects.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members
pub struct SurfaceIntersector<'s> {
    /// `m_cpSurface`.
    surfaces: [&'s Surface; 2],
    /// `m_vUVDomain`: the domain of each surface.
    pub uv_domains: [Extent2d; 2],
    /// `m_bClosedU`.
    pub closed_u: [bool; 2],
    /// `m_bClosedV`.
    pub closed_v: [bool; 2],
    /// `m_bUseSurfaceEdges`: look for start points on the surface boundaries.
    pub use_surface_edges: [bool; 2],
    /// `m_dThisAngTolRad`: largest tangent change between curve samples.
    pub this_ang_tol_rad: f64,
    /// `m_dThisApproxTol3d`: largest distance between the curve and the surfaces.
    pub this_approx_tol_3d: f64,
    /// `m_dSteppingParallelTolRadians`.
    pub stepping_parallel_tol_radians: f64,
    /// `m_dStartParallelTolRadians`.
    pub start_parallel_tol_radians: f64,
    /// `m_dCurveTraceDirection`: `+1` along `N0 x N1`, `-1` against it.
    pub curve_trace_direction: f64,
    /// `m_dLastThroughParmeter`.
    pub last_through_parameter: f64,
    /// `m_dExtensionDistance`.
    pub extension_distance: f64,
    /// `m_bDoingExtensionClipping`.
    pub doing_extension_clipping: bool,
    /// `m_bCurveIsClosed`.
    pub curve_is_closed: bool,
    /// `m_bCheckForInteriorCurves` (no longer used upstream).
    pub check_for_interior_curves: bool,
    /// `m_bDoingSelfIntersection`: both surfaces are the same object.
    pub doing_self_intersection: bool,
    /// `m_bClippedByThroughPoint`.
    pub clipped_by_through_point: bool,
    /// `m_bDoBoundaryPointOnCurveTest`.
    pub do_boundary_point_on_curve_test: bool,
    /// `m_vTSPntMgr`: the point arena.
    pub(crate) points: Vec<TsectPnt>,
    /// `m_pStartPoint`.
    pub start_point: Option<TsectId>,
    /// `m_pEndPoint`.
    pub end_point: Option<TsectId>,
    /// `m_vCurvePoints`.
    pub(crate) curve_points: Vec<TsectId>,
    /// `m_vStartPoints`.
    pub(crate) start_points: Vec<TsectId>,
    /// `m_vThroughPoints`.
    pub(crate) through_points: Vec<TsectId>,
    /// `Kernel::fixes.ssi_singular_start_inside_direction` as it was when this
    /// intersector was built (see [`SurfaceIntersector::new`]): a point computed later
    /// uses the value from construction, not a later change to the kernel.
    pub(crate) singular_start_inside_direction: bool,
    /// The object is an `SmAdvSurfaceIntersector`: the virtual
    /// `ComputeSingularityPoint` that `ComputePointValues` calls is that class's
    /// override, not the basic `SM_ERR` one.
    pub(crate) advanced: bool,
}

fn bad_id() -> SolidError {
    SolidError::invalid_input(OP_POINT, "no such intersection point")
}

/// A status upstream ignores: the value, else `fallback` (`Unported` propagates).
fn or_keep<T>(r: Result<T>, fallback: T) -> Result<T> {
    match r {
        Ok(v) => Ok(v),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(fallback),
    }
}

/// `SER` inside a method returning `SmBoolean`: a failing status reads as `TRUE`.
fn or_true(r: Result<bool>) -> Result<bool> {
    or_keep(r, true)
}

impl<'s> SurfaceIntersector<'s> {
    /// The intersector of `surface1` over `uv_domain1` and `surface2` over
    /// `uv_domain2` (`SmSurfaceIntersector(...)`; upstream default
    /// `from_fillet_intersector = FALSE`).
    ///
    /// Unless `from_fillet_intersector`, a domain not contained in its surface's
    /// natural domain is cut to the overlap (reset to the inside-out extent when they
    /// are disjoint). The closedness flags and the default tolerance
    /// `this_approx_tol_3d = (|P(min) P(mid)| + |P(mid) P(max)|) / 1000` are taken
    /// on the given domains.
    pub fn new(
        k: &Kernel,
        surface1: &'s Surface,
        uv_domain1: &Extent2d,
        surface2: &'s Surface,
        uv_domain2: &Extent2d,
        from_fillet_intersector: bool,
    ) -> Result<Self> {
        let mut surf1_uv_domain = surface1.natural_uv_domain_with_nurb(k)?;
        let mut surf2_uv_domain = surface2.natural_uv_domain_with_nurb(k)?;

        // check input - given Surf XSect domains fit within Surface NaturalUVDomains
        //               unless bFromFilletIntersector == TRUE
        if from_fillet_intersector || uv_domain1.is_contained_by(&surf1_uv_domain, EFF_ZERO) {
            surf1_uv_domain = *uv_domain1;
        } else {
            // change input domain to intersection of input and natural domains
            match surf1_uv_domain.intersect(uv_domain1) {
                Ok(d) => surf1_uv_domain = d,
                Err(_) => surf1_uv_domain.init(),
            }
        }
        if from_fillet_intersector || uv_domain2.is_contained_by(&surf2_uv_domain, EFF_ZERO) {
            surf2_uv_domain = *uv_domain2;
        } else {
            match surf2_uv_domain.intersect(uv_domain2) {
                Ok(d) => surf2_uv_domain = d,
                Err(_) => surf2_uv_domain.init(),
            }
        }

        let closed_u0 = surface_is_closed(k, surface1, uv_domain1, SurfParamType::U)?;
        let closed_v0 = surface_is_closed(k, surface1, uv_domain1, SurfParamType::V)?;
        let closed_u1 = surface_is_closed(k, surface2, uv_domain2, SurfParamType::U)?;
        let closed_v1 = surface_is_closed(k, surface2, uv_domain2, SurfParamType::V)?;

        // Compute a reasonable default tolerance for intersection based on the sizes
        // of the surfaces (statuses are only reported upstream).
        let p1 = or_keep(surface1.evaluate_point(k, uv_domain1.min()), Vec3::UNDEF)?;
        let p_mid = or_keep(
            surface1.evaluate_point(k, uv_domain1.evaluate(0.5, 0.5)),
            Vec3::UNDEF,
        )?;
        let p2 = or_keep(surface1.evaluate_point(k, uv_domain1.max()), Vec3::UNDEF)?;
        let size = p1.distance(p_mid) + p_mid.distance(p2);
        let this_approx_tol_3d = size / 1000.0; // accuracy is 1/1000th size of surface

        Ok(Self {
            surfaces: [surface1, surface2],
            uv_domains: [surf1_uv_domain, surf2_uv_domain],
            closed_u: [closed_u0, closed_u1],
            closed_v: [closed_v0, closed_v1],
            use_surface_edges: [true, true],
            this_ang_tol_rad: 20.0 * (PI / 180.0),
            this_approx_tol_3d,
            stepping_parallel_tol_radians: EFF_ZERO,
            start_parallel_tol_radians: EFF_ZERO_RAD,
            curve_trace_direction: 1.0,
            last_through_parameter: 0.0,
            extension_distance: 0.0,
            doing_extension_clipping: false,
            curve_is_closed: false,
            check_for_interior_curves: true,
            doing_self_intersection: core::ptr::eq(surface1, surface2),
            clipped_by_through_point: false,
            do_boundary_point_on_curve_test: true,
            points: Vec::new(),
            start_point: None,
            end_point: None,
            curve_points: Vec::new(),
            start_points: Vec::new(),
            through_points: Vec::new(),
            singular_start_inside_direction: k.fixes.ssi_singular_start_inside_direction,
            advanced: false,
        })
    }

    /// `GetSurface(index)`.
    #[must_use]
    pub const fn surface(&self, srf: SrfIndex) -> &'s Surface {
        match srf {
            SrfIndex::First => self.surfaces[0],
            SrfIndex::Second => self.surfaces[1],
        }
    }

    /// `GetThisApproxTol3d()`.
    #[must_use]
    pub const fn this_approx_tol_3d(&self) -> f64 {
        self.this_approx_tol_3d
    }

    /// `GetThisAngTolRad()`.
    #[must_use]
    pub const fn this_ang_tol_rad(&self) -> f64 {
        self.this_ang_tol_rad
    }

    /// `SetExtensionDistance`.
    pub fn set_extension_distance(&mut self, extension_distance: f64) {
        self.extension_distance = extension_distance;
    }

    /// `SetUseSurfaceEdges`.
    pub fn set_use_surface_edges(&mut self, use_surface1_edges: bool, use_surface2_edges: bool) {
        self.use_surface_edges = [use_surface1_edges, use_surface2_edges];
    }

    // ---- point arena and lists ------------------------------------------------------------

    /// Stores a new point in the arena (upstream: allocated from `m_vTSPntMgr`).
    pub fn alloc_point(&mut self, point: TsectPnt) -> TsectId {
        self.points.push(point);
        TsectId(self.points.len() - 1)
    }

    /// The point `id`.
    pub fn point(&self, id: TsectId) -> Result<&TsectPnt> {
        self.points.get(id.0).ok_or_else(bad_id)
    }

    /// The point `id`, mutable.
    pub fn point_mut(&mut self, id: TsectId) -> Result<&mut TsectPnt> {
        self.points.get_mut(id.0).ok_or_else(bad_id)
    }

    /// Every point of the arena, by id.
    #[must_use]
    pub fn points(&self) -> &[TsectPnt] {
        &self.points
    }

    /// `m_vCurvePoints`, in list order.
    #[must_use]
    pub fn curve_points(&self) -> &[TsectId] {
        &self.curve_points
    }

    /// `m_vStartPoints`, in list order.
    #[must_use]
    pub fn start_points(&self) -> &[TsectId] {
        &self.start_points
    }

    /// `m_vThroughPoints`, in list order.
    #[must_use]
    pub fn through_points(&self) -> &[TsectId] {
        &self.through_points
    }

    /// True when `id` is linked on one of the three lists.
    #[must_use]
    pub fn is_linked(&self, id: TsectId) -> bool {
        self.curve_points.contains(&id)
            || self.start_points.contains(&id)
            || self.through_points.contains(&id)
    }

    /// Checks that `id` exists and is on no list (linking it again is undefined
    /// upstream).
    fn check_linkable(&self, id: TsectId) -> Result<()> {
        self.point(id)?;
        if self.is_linked(id) {
            return Err(SolidError::invalid_input(
                OP_LIST,
                "intersection point is already on a list",
            ));
        }
        Ok(())
    }

    // ---- simple virtuals -------------------------------------------------------------------

    /// `ComputePointValuesDelta`: not implemented by the basic intersector (`SM_ERR`).
    pub fn compute_point_values_delta(&self) -> Result<()> {
        Err(SolidError::sm_err(OP_DELTA))
    }

    /// `ComputeSingularityPoint`: the basic intersector does not handle singular or
    /// tangent points; it reports and returns `SM_ERR`.
    pub fn compute_singularity_point(
        &self,
        _uv_values: [Point2; 2],
        _point: TsectId,
        _previous: Option<TsectId>,
    ) -> Result<()> {
        Err(SolidError::sm_err(OP_SING))
    }

    /// `ComputeStepSize`: `100 * this_approx_tol_3d` when `old_step_size` is zero
    /// (`SM_IS_ZERO`), else `old_step_size`.
    #[must_use]
    pub fn compute_step_size(&self, old_step_size: f64) -> f64 {
        if is_zero(old_step_size) {
            100.0 * self.this_approx_tol_3d // default if nothing else goes right
        } else {
            old_step_size
        }
    }

    // ---- singularities -------------------------------------------------------------------------

    /// Steps back from a surface pole along the curve and recomputes good first
    /// derivatives of surface `srf` (`StepBackFromSingularity`): copies the curve
    /// tangent of one point to the other (not with
    /// `Kernel::fixes.ssi_pole_point_keeps_its_tangent`, WR-550), drops the point at `t = 0.01` (from
    /// `previous`) or `t = 0.99` (from `current`, when `step_off_current`) of the
    /// Hermite span onto the surface, and stores the derivatives and the singular
    /// parameter found there in that point.
    pub fn step_back_from_singularity(
        &mut self,
        k: &Kernel,
        sing_dir: SurfParamType,
        srf: SrfIndex,
        step_off_current: bool,
        previous: TsectId,
        current: TsectId,
        step_size: f64,
    ) -> Result<()> {
        let mut prev = self.point(previous)?.clone();
        let mut curr = self.point(current)?.clone();
        let r = self.step_back_from_singularity_points(
            k,
            sing_dir,
            srf,
            step_off_current,
            &mut prev,
            &mut curr,
            step_size,
        );
        // write the point stepped off last (they may be the same point)
        if step_off_current {
            *self.point_mut(previous)? = prev;
            *self.point_mut(current)? = curr;
        } else {
            *self.point_mut(current)? = curr;
            *self.point_mut(previous)? = prev;
        }
        r
    }

    /// [`SurfaceIntersector::step_back_from_singularity`] on points outside the arena
    /// (upstream takes the points by reference).
    pub(crate) fn step_back_from_singularity_points(
        &self,
        k: &Kernel,
        sing_dir: SurfParamType,
        srf: SrfIndex,
        step_off_current: bool,
        prev: &mut TsectPnt,
        curr: &mut TsectPnt,
        step_size: f64,
    ) -> Result<()> {
        // copy the curve tangent of the point stepped off to the other one
        // Not upstream (WR-550, opt-in `fixes.ssi_pole_point_keeps_its_tangent`; the
        // fork's source fork@8d95600): the points keep their own curve tangents.
        // The tangent at the singular point comes from the surface's limit normal there
        // (`ComputePointValues`), so it is exact; copying the other point's tangent over
        // it bent the fitted span into the pole. Only the surface derivatives are taken
        // from the stepped-back point.
        if !k.fixes.ssi_pole_point_keeps_its_tangent {
            if step_off_current {
                curr.curve_pv[1] = prev.crv_deriv();
            } else {
                prev.curve_pv[1] = curr.crv_deriv();
            }
        }

        let herm = HermiteCurve::new(
            prev.crv_pos(),
            prev.crv_deriv() * step_size,
            curr.crv_pos(),
            curr.crv_deriv() * step_size,
            3,
        );
        let ivl = herm.natural_interval();
        let t_val = if step_off_current { 0.99 } else { 0.01 };
        let pnt = herm.evaluate_point(ivl.evaluate(t_val));

        let surface = self.surface(srf);
        let domain = *pick(&self.uv_domains, srf);
        let (mut found_answer, mut solution) =
            surface_local_point_solve(k, surface, &domain, pnt, prev.uv_pos(srf))?;
        if !found_answer {
            let solutions =
                surface_global_point_solve(k, surface, &domain, pnt, self.this_approx_tol_3d)?;
            if let Some(first) = solutions.first() {
                solution = *first;
                found_answer = true;
            }
        }

        if found_answer {
            let uv = Point2::new(solution[0], solution[1]);
            let (_, du, dv) = evaluate_1st_derivatives(k, surface, uv)?;
            let p = if step_off_current { curr } else { prev };
            *p.srf_du_mut(srf) = du;
            *p.srf_dv_mut(srf) = dv;
            if sing_dir == SurfParamType::U {
                p.uv_pos_mut(srf).x = uv.x;
            } else {
                p.uv_pos_mut(srf).y = uv.y;
            }
        }
        Ok(())
    }

    // ---- refinement and span tests ------------------------------------------------------------------

    /// Refines a guess near a surface/surface intersection to an intersection
    /// (`RefinePoint`): crossing points are refined with `this_approx_tol_3d`, tangent
    /// curve and singular points with ten times that, a tangent point is not refined
    /// (the result is the default solution, `(0, 0)`, clamped); other types fail
    /// (`None`). Returns the refined UV points clamped to the domains, `None` when the
    /// refinement fails.
    pub fn refine_point(
        &self,
        k: &Kernel,
        uv0: Point2,
        uv1: Point2,
        ip_type: IntersectionPointType,
        plane_normal: Option<Vec3>,
        previous_uv: Option<&[Point2]>,
        do_boundary_testing: bool,
    ) -> Result<Option<[Point2; 2]>> {
        let (xsect, tol) = match ip_type {
            IntersectionPointType::TangentCurve | IntersectionPointType::Singularity => {
                (true, self.this_approx_tol_3d * 10.00)
            }
            IntersectionPointType::Crossing => (true, self.this_approx_tol_3d),
            IntersectionPointType::TangentPoint => (false, 0.0),
            _ => return Ok(None),
        };

        let mut sol = [0.0; 4];
        if xsect {
            // do local surface/surface intersection from nextTSP to find next point
            let (found_answer, s) = surface_local_surface_intersect(
                k,
                self.surfaces[0],
                &self.uv_domains[0],
                self.surfaces[1],
                &self.uv_domains[1],
                tol,
                uv0,
                uv1,
                plane_normal,
                previous_uv,
                do_boundary_testing,
            )?;
            if !found_answer {
                return Ok(None);
            }
            sol = s;
        }

        // clamp the refined point to the surface boundaries
        let [s0, s1, s2, s3] = sol;
        Ok(Some([
            self.uv_domains[0].clamp_point(Point2::new(s0, s1)),
            self.uv_domains[1].clamp_point(Point2::new(s2, s3)),
        ]))
    }

    /// Tests the span from point `point` to point `next` (`TestSpanAccuracy`): the
    /// next curve tangent must not be shorter than `stepping_parallel_tol_radians`
    /// (`SM_ERR`), the tangent turn must not exceed `this_ang_tol_rad`, and at five
    /// interior parameters the Hermite span and the two Hermite UV spans mapped onto
    /// their surfaces must agree within `this_approx_tol_3d` (both tests only for a
    /// step longer than `100 * SM_EFF_ZERO`). A rejected span turns a singular `next`
    /// into a crossing point.
    pub fn test_span_accuracy(
        &mut self,
        k: &Kernel,
        point: TsectId,
        next: TsectId,
    ) -> Result<SpanAccuracy> {
        let tsect = self.point(point)?.clone();
        let next_tsp = self.point(next)?.clone();

        // check 3D space tangent vector length
        if next_tsp.crv_deriv().length() < self.stepping_parallel_tol_radians {
            return Err(SolidError::sm_err(OP_SPAN));
        }

        // get xsect curve parameter StepSize
        let step_size = next_tsp.curve_parameter - tsect.curve_parameter;

        // angle between TsectPnt.3DTangent and NextTSP.3DTangent
        let angle_found_rad = tsect.crv_deriv().angle_between(next_tsp.crv_deriv())?;

        // First do a quick angular test to see if we can reject this span
        if angle_found_rad > self.this_ang_tol_rad && step_size > EFF_ZERO * 100.0 {
            self.demote_singularity(next)?;
            return Ok(SpanAccuracy {
                found_good_point: false,
                deviation_found: None,
                angle_found_rad,
            });
        }

        // Create two parameter space Hermite curves and a 3D Hermite curve.
        let uv_crv0 = HermiteCurve::from_2d(
            tsect.uv_pos(SrfIndex::First),
            tsect.uv_deriv(SrfIndex::First) * step_size,
            next_tsp.uv_pos(SrfIndex::First),
            next_tsp.uv_deriv(SrfIndex::First) * step_size,
            2,
        );
        let uv_crv1 = HermiteCurve::from_2d(
            tsect.uv_pos(SrfIndex::Second),
            tsect.uv_deriv(SrfIndex::Second) * step_size,
            next_tsp.uv_pos(SrfIndex::Second),
            next_tsp.uv_deriv(SrfIndex::Second) * step_size,
            2,
        );
        let crv_3d = HermiteCurve::new(
            tsect.crv_pos(),
            tsect.crv_deriv() * step_size,
            next_tsp.crv_pos(),
            next_tsp.crv_deriv() * step_size,
            3,
        );

        let adjusted_tolerance = self.this_approx_tol_3d;
        let tol_sq = adjusted_tolerance * adjusted_tolerance;
        let mut max_dist_sq = 0.0;

        // for every test point - do a distance check
        for i in 0..NUM_SPAN_TEST_POINTS {
            let param = (i as f64 + 1.0) / (NUM_SPAN_TEST_POINTS as f64 + 1.0);

            // get UV points from UV curves
            let uv3d0 = uv_crv0.evaluate_point(param);
            let uv3d1 = uv_crv1.evaluate_point(param);
            let uv0 = Point2::new(uv3d0.x, uv3d0.y);
            let uv1 = Point2::new(uv3d1.x, uv3d1.y);

            // clamp UV points to surface domains
            let uv0 = self.uv_domains[0].clamp_point(uv0);
            let uv1 = self.uv_domains[1].clamp_point(uv1);

            // get surface points from UVpoints
            let pnt0 = self.surfaces[0].evaluate_point(k, uv0)?;
            let pnt1 = self.surfaces[1].evaluate_point(k, uv1)?;

            // get 3D xyz point from 3D curve
            let crv_pnt = crv_3d.evaluate_point(param);

            // get distances between 3D points
            let dist_sq = pnt0.distance_squared(pnt1);
            let dist_sq_c0 = pnt0.distance_squared(crv_pnt);
            let dist_sq_c1 = pnt1.distance_squared(crv_pnt);

            // get max curve/surface distance
            let dist_sq = max3(dist_sq, dist_sq_c0, dist_sq_c1);

            if dist_sq > max_dist_sq {
                max_dist_sq = dist_sq;
            }

            // when max curve/surface dist is bad
            if dist_sq > tol_sq && step_size > EFF_ZERO * 100.0 {
                self.demote_singularity(next)?;
                return Ok(SpanAccuracy {
                    found_good_point: false,
                    deviation_found: Some(sqrt(max_dist_sq)),
                    angle_found_rad,
                });
            }

            // save max curve/surface distance
            if dist_sq > max_dist_sq {
                max_dist_sq = dist_sq;
            }
        }

        Ok(SpanAccuracy {
            found_good_point: true,
            deviation_found: Some(sqrt(max_dist_sq)),
            angle_found_rad,
        })
    }

    /// A rejected span: a singular point becomes a crossing point.
    fn demote_singularity(&mut self, id: TsectId) -> Result<()> {
        let p = self.point_mut(id)?;
        if p.point_type == IntersectionPointType::Singularity {
            p.point_type = IntersectionPointType::Crossing;
        }
        Ok(())
    }

    // ---- point lists ----------------------------------------------------------------------------

    /// Appends point `id` to the curve points (`AddPointToCurve`).
    pub fn add_point_to_curve(&mut self, id: TsectId) -> Result<()> {
        self.check_linkable(id)?;
        self.curve_points.push(id);
        Ok(())
    }

    /// Adds point `id` to the start points unless it duplicates one
    /// (`AddStartPoint`): same UV points on both surfaces (squared distances below
    /// `SM_EFF_ZERO_SQ`); a bounded/exact pair (one bounded, one not, within
    /// `2 * this_approx_tol_3d` in 3D, a tenth of the natural domains in UV, and with
    /// curve tangents closer than a curvature-scaled angle), where the bounded one is
    /// dropped; or UV points within `SM_EFF_ZERO` (squared) whose surface points are
    /// within `this_approx_tol_3d`. Crossing points are appended, others prepended.
    pub fn add_start_point(&mut self, k: &Kernel, id: TsectId) -> Result<()> {
        self.check_linkable(id)?;
        let start = self.point(id)?.clone();
        let mut found_match = false;

        // Is rStartTSP a bounded intersection
        let scaled_zero = EFF_ZERO * 100.0 * (1.0 + start.srf_pos(SrfIndex::First).max_dimension());
        let bounded_start = start.is_bounded_start(scaled_zero);

        // opt-in fix (WR-76, #392): keep one start point per tangent cluster, the
        // tangent-curve point with the smallest deviation
        if k.fixes.ssi_trace_tangent_curves_once && start.point_type.is_tangent_type() {
            for i in 0..self.start_points.len() {
                let Some(&tid) = self.start_points.get(i) else {
                    break;
                };
                let target = self.point(tid)?.clone();
                if !self.is_same_tangent_cluster(k, &start, &target, false)? {
                    continue;
                }
                if tangent_start_rank(&start) < tangent_start_rank(&target) {
                    // the new point is the better representative: it takes the
                    // target's place
                    if let Some(slot) = self.start_points.get_mut(i) {
                        *slot = id;
                    }
                }
                return Ok(());
            }
        }

        // check every StartPoint for a match to input point
        for &tid in &self.start_points {
            let tsp = self.point(tid)?;
            let bounded_target = tsp.is_bounded_start(scaled_zero);

            // get UVDist to TargetPoint on both surfaces
            let dist_uv1_sq = start
                .uv_pos(SrfIndex::First)
                .distance_squared(tsp.uv_pos(SrfIndex::First));
            let dist_uv2_sq = start
                .uv_pos(SrfIndex::Second)
                .distance_squared(tsp.uv_pos(SrfIndex::Second));

            // when UVDists for both Surfs are zero
            if dist_uv1_sq < EFF_ZERO_SQ && dist_uv2_sq < EFF_ZERO_SQ {
                found_match = true;
                break;
            }

            // Look for Exact/bounded intersection start point pairs
            if bounded_start != bounded_target
                && start
                    .srf_pos(SrfIndex::First)
                    .distance(tsp.srf_pos(SrfIndex::First))
                    < 2.0 * self.this_approx_tol_3d
                && start
                    .srf_pos(SrfIndex::Second)
                    .distance(tsp.srf_pos(SrfIndex::Second))
                    < 2.0 * self.this_approx_tol_3d
            {
                // get UV distances
                let uv_tol1 = self.surfaces[0]
                    .natural_uv_domain_with_nurb(k)?
                    .max_dimension()
                    / 10.0;
                let uv_tol2 = self.surfaces[1]
                    .natural_uv_domain_with_nurb(k)?
                    .max_dimension()
                    / 10.0;

                // UV Points must be close enough
                if start
                    .uv_pos(SrfIndex::First)
                    .distance(tsp.uv_pos(SrfIndex::First))
                    < uv_tol1
                    && start
                        .uv_pos(SrfIndex::Second)
                        .distance(tsp.uv_pos(SrfIndex::Second))
                        < uv_tol2
                {
                    // get XSectCurve tangent directions and a tangent angle tolerance value
                    let curvature0 = or_keep(
                        evaluate_normal_section(
                            &start.surface_pv[0],
                            Vec3::from(start.uv_deriv(SrfIndex::First)),
                        ),
                        0.0,
                    )?;
                    let curvature1 = or_keep(
                        evaluate_normal_section(
                            &start.surface_pv[1],
                            Vec3::from(start.uv_deriv(SrfIndex::Second)),
                        ),
                        0.0,
                    )?;
                    let curvature = max2(curvature0.abs(), curvature1.abs());
                    let c_radius = if is_zero(curvature) {
                        100.0 * self.this_approx_tol_3d
                    } else {
                        min2(1.0 / curvature, 100.0 * self.this_approx_tol_3d)
                    };
                    let ang_tol_rad = 0.01 + self.this_approx_tol_3d.atan2(c_radius);

                    // status ignored: a zero tangent gives an angle of 0
                    let ang_between_rad = start
                        .crv_deriv()
                        .angle_between(tsp.crv_deriv())
                        .unwrap_or(0.0);

                    // when tangent angles are the same - these are an exact/bounded
                    // startPoint pair: keep the exact one. (Upstream "replaces" the
                    // bounded target by assigning a local pointer only, so either way
                    // the candidate is not added.)
                    if ang_between_rad < ang_tol_rad {
                        found_match = true;
                        break;
                    }
                }
            }

            // when UVDists for both Surfs are small
            if dist_uv1_sq < EFF_ZERO && dist_uv2_sq < EFF_ZERO {
                // get TargetPoint 3d position
                let p1 = self.surfaces[0].evaluate_point(k, tsp.uv_pos(SrfIndex::First))?;
                let p2 = self.surfaces[1].evaluate_point(k, tsp.uv_pos(SrfIndex::Second))?;

                // CurrentPoint surf/surf distance is less than tolerance
                if p1.distance(p2) < self.this_approx_tol_3d {
                    found_match = true;
                    break;
                }
            }
        }

        // add Point to m_vStartPoints list
        if !found_match {
            // put crossing points in front of tangent, coincident, and singular points
            if start.point_type == IntersectionPointType::Crossing {
                self.start_points.push(id);
            } else {
                self.start_points.insert(0, id);
            }
        }
        Ok(())
    }

    /// True when two tangent-type boundary start points are two samples of one
    /// tangential contact crossing a boundary, not two intersection branches
    /// (`IsSameTangentCluster`, opt-in fix WR-76, #392; the source fork's
    /// source fork@0ce09f2): they lie on a common boundary side of both
    /// surfaces, within `10 sqrt(this_approx_tol_3d (1 + size))` of each other, and
    /// the surfaces stay within `this_approx_tol_3d` of each other at three points of
    /// the boundary between them. `any_type_a` lets `a` be of any type (a traced
    /// curve's end).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the surface evaluations; other failures read as
    /// `false`, as the fork ignores their statuses.
    pub fn is_same_tangent_cluster(
        &self,
        k: &Kernel,
        a: &TsectPnt,
        b: &TsectPnt,
        any_type_a: bool,
    ) -> Result<bool> {
        if (!a.point_type.is_tangent_type() && !any_type_a) || !b.point_type.is_tangent_type() {
            return Ok(false);
        }
        // both on a common boundary side of each surface
        for srf in [SrfIndex::First, SrfIndex::Second] {
            let same_u = a.u_param_type(srf) != ExtentPointType::Inside
                && a.u_param_type(srf) == b.u_param_type(srf)
                && (a.uv_pos(srf).x - b.uv_pos(srf).x).abs() < EFF_ZERO_SQRT;
            let same_v = a.v_param_type(srf) != ExtentPointType::Inside
                && a.v_param_type(srf) == b.v_param_type(srf)
                && (a.uv_pos(srf).y - b.uv_pos(srf).y).abs() < EFF_ZERO_SQRT;
            if !same_u && !same_v {
                return Ok(false);
            }
        }
        // near each other: a contact of order n smears the boundary solution over
        // ~ tol^(1/n); stay within a few sqrt(tol) (order 2) of each other
        let size = 1.0 + a.srf_pos(SrfIndex::First).max_dimension();
        let radius = 10.0 * sqrt(self.this_approx_tol_3d * size);
        if a.srf_pos(SrfIndex::First)
            .distance(b.srf_pos(SrfIndex::First))
            > radius
        {
            return Ok(false);
        }
        // the surfaces stay within tolerance along the boundary in between
        let dom1 = self.uv_domains[1];
        for i in 1..4 {
            let t = 0.25 * f64::from(i);
            let uv0 = a.uv_pos(SrfIndex::First)
                + t * (b.uv_pos(SrfIndex::First) - a.uv_pos(SrfIndex::First));
            let uv1 = a.uv_pos(SrfIndex::Second)
                + t * (b.uv_pos(SrfIndex::Second) - a.uv_pos(SrfIndex::Second));
            let Some(p0) = crate::error::ignored(self.surfaces[0].evaluate_point(k, uv0))? else {
                return Ok(false);
            };
            let opts = crate::surface::surface::p5::DropPointOptions {
                guess_uv: Some(uv1),
                ..Default::default()
            };
            let dropped = crate::error::ignored(
                self.surfaces[1]
                    .drop_point::<crate::surface::surface::p4::NoObject>(k, p0, &dom1, &opts),
            )?;
            match dropped {
                Some(d) if d.success && d.gap <= self.this_approx_tol_3d => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    /// Removes the start points in a tangent cluster
    /// ([`SurfaceIntersector::is_same_tangent_cluster`], the end of any type) with the
    /// first or the last curve point, when those differ: the trace reaches the
    /// boundary within tolerance but not necessarily at the cluster's representative
    /// start point, which would otherwise be traced again from that end (the
    /// source fork's `DoIntersection` block before `FlushCurve`, opt-in fix
    /// WR-76, #392; source fork@0ce09f2).
    ///
    /// # Errors
    ///
    /// As [`SurfaceIntersector::is_same_tangent_cluster`].
    pub(crate) fn remove_end_tangent_clusters(&mut self, k: &Kernel) -> Result<()> {
        let (Some(&first), Some(&last)) = (self.curve_points.first(), self.curve_points.last())
        else {
            return Ok(());
        };
        if first == last {
            return Ok(());
        }
        for end in [first, last] {
            let end = self.point(end)?.clone();
            let left = self.start_points.clone();
            for q in left {
                let candidate = self.point(q)?.clone();
                if self.is_same_tangent_cluster(k, &end, &candidate, true)? {
                    self.start_points.retain(|&id| id != q);
                }
            }
        }
        Ok(())
    }

    /// Appends point `id` to the through points unless its UV points match one's
    /// (squared distances below `SM_EFF_ZERO_SQ`) (`AddThroughPoint`).
    pub fn add_through_point(&mut self, id: TsectId) -> Result<()> {
        self.check_linkable(id)?;
        let through = self.point(id)?;
        let mut found_match = false;
        for &tid in &self.through_points {
            let tsp = self.point(tid)?;
            let dist_uv1_sq = through
                .uv_pos(SrfIndex::First)
                .distance_squared(tsp.uv_pos(SrfIndex::First));
            let dist_uv2_sq = through
                .uv_pos(SrfIndex::Second)
                .distance_squared(tsp.uv_pos(SrfIndex::Second));
            if dist_uv1_sq < EFF_ZERO_SQ && dist_uv2_sq < EFF_ZERO_SQ {
                found_match = true;
                break;
            }
        }
        if !found_match {
            self.through_points.push(id);
        }
        Ok(())
    }

    /// Reverses the traced curve (`ReverseCurveDirection`): negates every point's
    /// curve and UV tangents, re-parameterises so the old end is `0` (same range),
    /// and reverses the curve point order. An empty curve is `SM_ERR_NULL_POINTER`.
    pub fn reverse_curve_direction(&mut self) -> Result<()> {
        if self.curve_points.is_empty() {
            return Err(SolidError::Upstream {
                operation: OP_REVERSE,
                code: SM_ERR_NULL_POINTER,
                entity: None,
            });
        }
        let pnts = core::mem::take(&mut self.curve_points);

        let mut curr_param = 0.0;
        for i in (1..=pnts.len()).rev() {
            let id = *pnts.get(i - 1).ok_or_else(bad_id)?;
            let before = if i > 1 {
                let bid = *pnts.get(i - 2).ok_or_else(bad_id)?;
                Some(self.point(bid)?.curve_parameter)
            } else {
                None
            };
            let p = self.point_mut(id)?;

            // negate the tangent directions
            p.curve_pv[1] = -p.curve_pv[1];
            p.uv_curve_pv[0][1] = -p.uv_curve_pv[0][1];
            p.uv_curve_pv[1][1] = -p.uv_curve_pv[1][1];

            // change parameters so that old end is parameter = 0
            let delta_param = before.map_or(0.0, |b| p.curve_parameter - b);
            p.curve_parameter = curr_param;
            curr_param += delta_param;
            self.curve_points.push(id);
        }
        Ok(())
    }

    // ---- point predicates ---------------------------------------------------------------------

    /// True when `point` lies on one of `curves_3d` (skipping index `skip`)
    /// (`IsPointOnCurve`): a degenerate curve (`IsDegenerate()`, tolerance
    /// `SM_EFF_ZERO`) within `4 * this_approx_tol_3d` of its start; another within
    /// `this_approx_tol_3d` of its drop point (`DropPoint`, `SM_SO_INTERSECT` with
    /// `10 * this_approx_tol_3d`), or within ten times that when the surfaces do not
    /// diverge between the point and the curve point.
    pub fn is_point_on_curve(
        &self,
        k: &Kernel,
        point: Point3,
        curves_3d: &[Curve],
        skip: Option<usize>,
    ) -> Result<bool> {
        self.is_point_on_curve_cached(k, point, curves_3d, skip, &mut DropPointCaches::default())
    }

    /// [`SurfaceIntersector::is_point_on_curve`] keeping the caches of the curves'
    /// drops in `caches` across calls: slot `i` belongs to `curves_3d[i]`, which must
    /// not change while the slots are used (curves may be appended).
    ///
    /// # Errors
    ///
    /// As [`SurfaceIntersector::is_point_on_curve`].
    pub(crate) fn is_point_on_curve_cached(
        &self,
        k: &Kernel,
        point: Point3,
        curves_3d: &[Curve],
        skip: Option<usize>,
        caches: &mut DropPointCaches,
    ) -> Result<bool> {
        for (jj, curve) in curves_3d.iter().enumerate() {
            // when given an optional skip index - skip it
            if skip == Some(jj) {
                continue;
            }

            // if the curve is degenerate (can be treated as a single point)
            if curve.is_degenerate(k, EFF_ZERO, None)? {
                let ivl = curve.natural_interval();
                let crv_pnt = match curve.evaluate_point(k, ivl.min()) {
                    Ok(p) => p,
                    Err(e) => return or_true(Err(e)),
                };
                if crv_pnt.distance(point) < self.this_approx_tol_3d * 4.0 {
                    return Ok(true);
                }
            } else {
                let ivl = curve.natural_interval();

                // Find nearest point on curve to test point within 10*tolerance.
                let drop = match curve.drop_point_in(
                    k,
                    &ivl,
                    point,
                    None,
                    self.this_approx_tol_3d * 10.0,
                    None,
                    SolverOperation::Intersect,
                    caches,
                    jj,
                ) {
                    Ok(d) => d,
                    Err(e) => return or_true(Err(e)),
                };
                let (success, crv_param, dist) = (drop.success, drop.parameter, drop.distance);

                // If the point is within tolerance, it's on the curve.
                if success && dist < self.this_approx_tol_3d {
                    return Ok(true);
                }

                // If within 10x tol, then check whether the surfaces diverge in
                // between (two separate intersection curves).
                if success && dist < self.this_approx_tol_3d * 10.0 {
                    let srf1 = self.surfaces[0];
                    let srf2 = self.surfaces[1];
                    let dom1 = srf1.natural_uv_domain_with_nurb(k)?;
                    let dom2 = srf2.natural_uv_domain_with_nurb(k)?;

                    let (int_pt_uv1, int_pt_gap1) =
                        surface_drop_point(k, srf1, point, &dom1, None)?;
                    let (int_pt_uv2, int_pt_gap2) =
                        surface_drop_point(k, srf2, point, &dom2, None)?;

                    // status ignored upstream: an uninitialised point on failure
                    let crv_pt = or_keep(curve.evaluate_point(k, crv_param), Vec3::UNDEF)?;

                    let (crv_pt_uv1, crv_pt_gap1) =
                        surface_drop_point(k, srf1, crv_pt, &dom1, Some(int_pt_uv1))?;
                    let (crv_pt_uv2, crv_pt_gap2) =
                        surface_drop_point(k, srf2, crv_pt, &dom2, Some(int_pt_uv2))?;

                    let mid_pt_uv1 = (int_pt_uv1 + crv_pt_uv1) / 2.0;
                    let mid_pt_uv2 = (int_pt_uv2 + crv_pt_uv2) / 2.0;
                    let mid_srf1 = or_keep(srf1.evaluate_point(k, mid_pt_uv1), Vec3::UNDEF)?;
                    let mid_srf2 = or_keep(srf2.evaluate_point(k, mid_pt_uv2), Vec3::UNDEF)?;

                    let gap_mid = mid_srf1.distance(mid_srf2);

                    if gap_mid < (int_pt_gap1 + int_pt_gap2) + (crv_pt_gap1 + crv_pt_gap2) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// True when both surface points of `point` are within `this_approx_tol_3d` of a
    /// start point in `start_points` (skipping position `skip`) — upstream compares
    /// the squared distances with the tolerance (`IsPointOnStartPoint`).
    pub fn is_point_on_start_point(
        &self,
        point: &TsectPnt,
        start_points: &[TsectId],
        skip: Option<usize>,
    ) -> Result<bool> {
        let test_pt0 = point.srf_pos(SrfIndex::First);
        let test_pt1 = point.srf_pos(SrfIndex::Second);
        for (jj, &id) in start_points.iter().enumerate() {
            if skip == Some(jj) {
                continue;
            }
            let sp = self.point(id)?;
            if sp.srf_pos(SrfIndex::First).distance_squared(test_pt0) < self.this_approx_tol_3d
                && sp.srf_pos(SrfIndex::Second).distance_squared(test_pt1) < self.this_approx_tol_3d
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // ---- fillet laws ---------------------------------------------------------------------------

    /// The UV points of the offset surface / offset surface / plane intersection near
    /// `uv0`, `uv1` for the fillet radius of `law` at `parameter`
    /// (`EvaluateLawPoint`). Both surfaces must be offset surfaces; their offsets are
    /// set to the radius times `surface_orientations`.
    ///
    /// `law` is `const` upstream, but its evaluation caches a B-spline law's extension
    /// through a `const` cast; here it is `&mut`.
    ///
    /// # Errors
    ///
    /// Those of [`FilletLaw::evaluate`] (`SM_ERR` for the base `SmFilletLaw`);
    /// `SM_ERR_NULL_POINTER` for a surface that is not an offset surface (a `NULL`
    /// dereference upstream); those of `LocalPlaneSurfaceIntersect`; `SM_ERR` when it
    /// finds no answer.
    pub fn evaluate_law_point(
        &self,
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
    ) -> Result<[Point2; 2]> {
        // get fillet radius and its derivatives at a point along a fillet edge
        let offsets = law.evaluate(parameter, point_curve_interval, law_orient)?;

        // set Offset Surface Offset signed distances
        let [o0, o1] = surface_orientations;
        set_offset_distance(self.surfaces[0], offsets[0] * o0)?;
        set_offset_distance(self.surfaces[1], offsets[0] * o1)?;

        // do surf/surf/plane intersection
        let (found_answer, sol) = surface_local_plane_surface_intersect(
            k,
            self.surfaces[0],
            &self.uv_domains[0],
            self.surfaces[1],
            &self.uv_domains[1],
            self.this_approx_tol_3d,
            uv0,
            uv1,
            plane_origin,
            plane_normal,
        )?;
        if !found_answer {
            return Err(SolidError::sm_err(OP_LAW));
        }

        let [s0, s1, s2, s3] = sol;
        Ok([
            self.uv_domains[0].clamp_point(Point2::new(s0, s1)),
            self.uv_domains[1].clamp_point(Point2::new(s2, s3)),
        ])
    }
}

// ---- file-static helpers ---------------------------------------------------------------------

/// The kind of the intersection curve with UV curves `uv_curve1` on `surface1` and
/// `uv_curve2` on `surface2`, judged at their mid parameters (`sm_ComputeCurveType`):
/// crossing unless the normals are parallel within `1e-4`, then tangent, or
/// coincident when the principal curvatures (signed by the normals' sense) agree.
pub fn compute_curve_type(
    k: &Kernel,
    surface1: &Surface,
    surface2: &Surface,
    uv_curve1: &Curve,
    uv_curve2: &Curve,
) -> Result<TsectCurveType> {
    let mut ret = TsectCurveType::Crossing;
    let ivl1 = uv_curve1.natural_interval();
    let ivl2 = uv_curve2.natural_interval();
    let p1 = or_keep(uv_curve1.evaluate_point(k, ivl1.evaluate(0.5)), Vec3::UNDEF)?;
    let p2 = or_keep(uv_curve2.evaluate_point(k, ivl2.evaluate(0.5)), Vec3::UNDEF)?;
    let uv1 = Point2::new(p1.x, p1.y);
    let uv2 = Point2::new(p2.x, p2.y);
    let norm1 = or_keep(surface_evaluate_normal(k, surface1, uv1), Vec3::UNDEF)?;
    let norm2 = or_keep(surface_evaluate_normal(k, surface2, uv2), Vec3::UNDEF)?;
    let mut sign = 1.0;
    let dot = norm1.dot(norm2);
    if dot < 0.0 {
        sign = -1.0;
    }

    let limit = 0.0001; // was SM_EFF_ZERO_SQRT [B162]

    // Test to see if normals coincide
    if dot.abs() > 1.0 - limit {
        // We're at least TAN.
        ret = TsectCurveType::Tangent;
        let g1 = surface_evaluate_geometric(k, surface1, uv1)?;
        let g2 = surface_evaluate_geometric(k, surface2, uv2)?;
        let (prin_cur11, prin_cur12, prin_kv11) = (g1.k1, g1.k2, g1.kv1);
        let (prin_cur21, prin_cur22, prin_kv21, prin_kv22) = (g2.k1, g2.k2, g2.kv1, g2.kv2);

        // First check to see if we have coincident umbilical points
        let scale = 1.0 + prin_cur11.abs();
        if (prin_cur11 - prin_cur12).abs() < limit * scale {
            // Surf1 is umbilical. If Surf2 is not umbilical, return TAN.
            if (prin_cur21 - prin_cur22).abs() > limit * scale {
                return Ok(ret);
            }
            // Surf2 is also umbilical.  If radii of curvatures are equal, then COINC.
            if (prin_cur11 - sign * prin_cur21).abs() > limit * scale {
                return Ok(ret);
            }
            return Ok(TsectCurveType::Coincident);
        }
        // Here, Srf1 is not umbilical. If Srf2 is umbilical, just quit now.
        if (prin_cur21 - prin_cur22).abs() < limit * scale {
            return Ok(ret);
        }

        // Now check to see if we have curvature vectors and magnitudes which are
        // equivalent.
        if prin_kv11.dot(prin_kv21).abs() > 1.0 - limit {
            // First prin crv vectors line up.  Compare the appropriate curvatures.
            if (prin_cur11 - sign * prin_cur21).abs() > limit * scale {
                return Ok(ret);
            }
            let scale2 = 1.0 + prin_cur12.abs();
            if (prin_cur12 - sign * prin_cur22).abs() > limit * scale2 {
                return Ok(ret);
            }
            ret = TsectCurveType::Coincident;
        }
        if prin_kv11.dot(prin_kv22).abs() > 1.0 - limit {
            // First prin crv vector of Surf1 lines up with 2nd prin crv vector
            // of Surf2.  Compare the appropriate curvatures.
            if (prin_cur11 - sign * prin_cur22).abs() > limit * scale {
                return Ok(ret);
            }
            let scale2 = 1.0 + prin_cur12.abs();
            if (prin_cur12 - sign * prin_cur21).abs() > limit * scale2 {
                return Ok(ret);
            }
            ret = TsectCurveType::Coincident;
        }
    }
    Ok(ret)
}

/// True when `point_to_test` lies on the traced curve span from `prev` to `next`
/// (`sm_IsPointOnCurve`): the tolerance is widened by `1 / tan(angle / 2)` of the
/// larger surface angle (at most 1000 times); the point must pass box tests in 3D
/// and in both UV spaces, then drop onto the Hermite span within the tolerance with
/// a tangent pointing back along it, or onto its interior within half the chord's
/// largest coordinate.
pub fn is_point_on_tsect_curve(
    k: &Kernel,
    point_to_test: &TsectPnt,
    prev: &TsectPnt,
    next: &TsectPnt,
    approx_tol_3d: f64,
) -> Result<bool> {
    let mut tol = approx_tol_3d;

    // get max surfNorm/surfNorm angle at XSectCurve Segment endPoints
    let ang = max2(prev.tangent_plane_angle_rad, next.tangent_plane_angle_rad);

    // when the angle is not exactly zero (tangent xSectCurve Segment)
    if ang > 0.0 {
        let mut tan = (ang / 2.0).tan();
        if tan < 0.001 {
            tan = 0.001;
        }
        tol /= tan;
    }

    // Get xSect Segment domain interval.
    let delta_t = next.curve_parameter - prev.curve_parameter;

    // Build 3d hermite curve from PrevTSP to NextTSP.
    let p1 = prev.crv_pos();
    let p2 = next.crv_pos();
    let herm_3d = HermiteCurve::new(
        p1,
        prev.crv_deriv() * delta_t,
        p2,
        next.crv_deriv() * delta_t,
        3,
    );

    // Get rough distance between PrevTSP and NextTSP.
    let length = (p2 - p1).max_dimension();

    // let s3DBox = bounding box for hermite curve
    let mut box_3d = Extent3d::new();
    let hermite_domain = herm_3d.natural_interval();
    herm_3d.calculate_bounding_box(Some(&mut box_3d), None, None)?;

    // let sPntBox = HalfSize+Tol Sized box centered on the crPointToTest
    let mut pnt_box = Extent3d::from_point(point_to_test.crv_pos());
    pnt_box.expand_absolute(tol * 2.0 + length / 4.0);

    // Not on curve if the bounding box tests fails.
    if box_3d.are_disjoint(&pnt_box, 0.0) {
        return Ok(false);
    }

    // Do bounding-box checks in the uv-space of both surfaces.
    for srf in [SrfIndex::First, SrfIndex::Second] {
        let herm_uv = HermiteCurve::from_2d(
            prev.uv_pos(srf),
            prev.uv_deriv(srf) * delta_t,
            next.uv_pos(srf),
            next.uv_deriv(srf) * delta_t,
            2,
        );

        let mut uv_box = Extent3d::new();
        herm_uv.calculate_bounding_box(Some(&mut uv_box), None, None)?;

        // Expand u and v separately (zero-length domains by a hundredth of the
        // diagonal).
        let diag = uv_box.size().length();

        let mut temp = uv_box.u_interval();
        let mut len = temp.length();
        if len < diag / 100.0 {
            len = diag / 100.0;
        }
        temp.expand_absolute(len);
        uv_box.set_u_interval(&temp);

        let mut temp = uv_box.v_interval();
        let mut len = temp.length();
        if len < diag / 100.0 {
            len = diag / 100.0;
        }
        temp.expand_absolute(len);
        uv_box.set_v_interval(&temp);

        // If point is not contained by the expanded UVBounding Box - return FALSE.
        let uv = Vec3::from(point_to_test.uv_pos(srf));
        if !uv_box.contains_point(uv, 0.0) {
            return Ok(false);
        }
    }

    // Find closest point on the Hermite Curve to the Point, from the midpoint.
    let guess = hermite_domain.evaluate(0.5);
    let herm_curve = Curve::from(herm_3d);
    let (success, sol) = match herm_curve.local_point_solve::<()>(
        k,
        &hermite_domain,
        SolverOperation::Minimize,
        point_to_test.crv_pos(),
        &PointSolveOptions::default(),
        guess,
    ) {
        Ok(r) => r,
        Err(e) => return or_true(Err(e)),
    };

    // If the test point dropped to the interior of the Hermite curve,
    // then we've moved past it.  [B391]
    if success {
        // First, if distance is within tolerance, it's good.
        let dist = sol.start.solution_value;
        let t = sol.start.parameters[0];
        if dist <= tol {
            // crPointToTest has a direction: check that too.  [B401]
            let pv = herm_3d.evaluate(t, 1, true);
            let tangent = pv.get(1).copied().unwrap_or(Vec3::ZERO);
            if point_to_test.crv_deriv().dot(tangent) < 0.0 {
                return Ok(true);
            }
        }

        // Check value on interior.  (Pass negative tol: just excludes end points.)
        if hermite_domain.contains_value(t, -EFF_ZERO) {
            let cbi_dist_limit = length / 2.0;
            return Ok(dist <= cbi_dist_limit);
        }
    }

    // The point is not on the curve.
    Ok(false)
}

/// Appends the piecewise cubic, C0 B-spline of `cntrl_poly` over the unique `knots`
/// (end multiplicity 4, interior 3) to `curves` (`sm_FlushCrv`); nothing when there
/// are no knots.
pub fn flush_crv(
    dimension: u64,
    cntrl_poly: &[Point3],
    knots: &[f64],
    curves: &mut Vec<Curve>,
) -> Result<()> {
    // no work - no knots
    if knots.is_empty() {
        return Ok(());
    }

    // make a piecewise C0, degree 3, knot multiplicity array
    // 4 knots on each end, 3 knots for each internal knot
    let mut knot_mult = vec![3usize; knots.len()];
    if let Some(first) = knot_mult.first_mut() {
        *first = 4;
    }
    if let Some(last) = knot_mult.last_mut() {
        *last = 4;
    }

    // make the degree 3 SmBsplineCurve
    let new_bsp = BSplineCurve::create_canonical(&CanonicalData {
        dim: dimension,
        degree: 3,
        control_points: cntrl_poly,
        form: BSplineCurveForm::Unspecified,
        knot_multiplicities: &knot_mult,
        knots,
        knot_type: KnotType::Unspecified,
        weights: None,
        trim: None,
    })?;

    curves.push(Curve::from(new_bsp));
    Ok(())
}

// ---- co-ported callees ----------------------------------------------------------------------

/// `crSurface.IsClosed(crUVDomain, eSurfParam)` (default tolerance, no continuity):
/// [`Surface::is_closed`] (`surface::surface::p7`, with the `SmPlane` override).
fn surface_is_closed(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    surf_param: SurfParamType,
) -> Result<bool> {
    Ok(surface.is_closed(k, domain, surf_param, None, false)?.0)
}

/// `SmSurface::Evaluate1stDerivatives(uv, TRUE, TRUE, ..)`
/// (`surface::surface::p7::evaluate_1st_derivatives`).
fn evaluate_1st_derivatives(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
) -> Result<(Point3, Vec3, Vec3)> {
    crate::surface::surface::p1::evaluate_1st_derivatives(k, surface, uv, Side::Left, Side::Left)
}

// ---- unported callees ---------------------------------------------------------------------------

/// `crSurface.EvaluateNormal(uv, TRUE, TRUE, ..)`: the plane override; other kinds
/// run `SmSurface::EvaluateNormal`.
pub(crate) fn surface_evaluate_normal(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Vec3> {
    surface.evaluate_normal(k, uv, Side::Left, Side::Left)
}

/// The principal curvatures and directions of `SmSurface::EvaluateGeometric`.
#[derive(Debug, Clone, Copy)]
struct Principal {
    k1: f64,
    k2: f64,
    kv1: Vec3,
    kv2: Vec3,
}

/// `crSurface.EvaluateGeometric(uv, TRUE, TRUE, ..)`.
fn surface_evaluate_geometric(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Principal> {
    let g = surface.evaluate_geometric(k, uv, Side::Left, Side::Left)?;
    Ok(Principal {
        k1: g.principle_curvature1,
        k2: g.principle_curvature2,
        kv1: g.principle_curvature_vector1,
        kv2: g.principle_curvature_vector2,
    })
}

/// `smsurf_EvaluateNormalSection(eval, direction, ..)`: the curvature in `direction`.
fn evaluate_normal_section(eval: &[[Vec3; 3]; 3], direction: Vec3) -> Result<f64> {
    // (upstream may rebuild a vanishing first derivative of the caller's matrix in
    // place; the caller only reads the curvature, so a copy is evaluated)
    let mut eval = *eval;
    Ok(crate::surface::surface::p1::evaluate_normal_section(&mut eval, direction)?.1)
}

/// `crSurface.LocalPointSolve(domain, SM_SO_MINIMIZE, point, guess, ..)`: found flag
/// and the solution's start parameters.
fn surface_local_point_solve(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    point: Point3,
    guess: Point2,
) -> Result<(bool, [f64; 2])> {
    let r = surface.local_point_solve::<crate::surface::surface::p4::NoObject>(
        k,
        domain,
        SolverOperation::Minimize,
        point,
        guess,
    )?;
    Ok((
        r.found,
        [r.solution.start.param(0)?, r.solution.start.param(1)?],
    ))
}

/// `crSurface.GlobalPointSolve(domain, SM_SO_MINIMIZE, point, tol, NULL, SM_SR_ALL,
/// ..)`: the solutions' start parameters.
fn surface_global_point_solve(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    point: Point3,
    tol: f64,
) -> Result<Vec<[f64; 2]>> {
    let starts = crate::surface::surface::p4::global_point_solve_starts(
        k,
        surface,
        domain,
        SolverOperation::Minimize,
        point,
        tol,
        SolutionRequested::All,
    )?;
    starts
        .iter()
        .map(|s| Ok([s.param(0)?, s.param(1)?]))
        .collect()
}

/// The rank of a tangent start point as a cluster's representative, lower is better:
/// a traceable tangent-curve point first, then the smallest deviation (the fork's
/// `sm_TangentStartRank`, opt-in fix WR-76, #392).
fn tangent_start_rank(tsp: &TsectPnt) -> f64 {
    (if tsp.point_type == IntersectionPointType::TangentCurve {
        0.0
    } else {
        1.0
    }) + min2(tsp.deviation, 0.5)
}

/// `crSurface.DropPoint(point, domain, guess, ..)`: the UV point and its gap.
fn surface_drop_point(
    k: &Kernel,
    surface: &Surface,
    point: Point3,
    domain: &Extent2d,
    guess: Option<Point2>,
) -> Result<(Point2, f64)> {
    let opts = crate::surface::surface::p5::DropPointOptions {
        guess_uv: guess,
        ..Default::default()
    };
    let r = surface.drop_point::<crate::surface::surface::p4::NoObject>(k, point, domain, &opts)?;
    Ok((r.result_uv, r.gap))
}

/// `m_cpSurface[0]->LocalSurfaceIntersect(..)`: found flag and the solution's start
/// parameters `[u0, v0, u1, v1]` (`surface::surface::p11`; `cpOptPreviousUV` is the
/// first point of `previous_uv`, upstream's `&rPrevTSP.UVPos(0)`). The status is
/// checked (`SER`) by every caller; `LocalSurfaceIntersect` is not virtual and
/// evaluates each surface through its own kind.
pub(crate) fn surface_local_surface_intersect(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    other: &Surface,
    other_domain: &Extent2d,
    tol: f64,
    uv0: Point2,
    uv1: Point2,
    plane_normal: Option<Vec3>,
    previous_uv: Option<&[Point2]>,
    do_boundary_testing: bool,
) -> Result<(bool, [f64; 4])> {
    let opts = crate::surface::surface::p11::LocalSurfaceIntersectOptions {
        plane_normal,
        previous_uv: previous_uv.and_then(|p| p.first().copied()),
        do_boundary_testing,
    };
    // (the solution is only written when found; upstream callers read it then only)
    Ok(
        match surface.local_surface_intersect(
            k,
            domain,
            other,
            other_domain,
            tol,
            uv0,
            uv1,
            &opts,
        )? {
            Some(r) => (true, [r.uv.x, r.uv.y, r.other_uv.x, r.other_uv.y]),
            None => (false, [0.0; 4]),
        },
    )
}

/// `pOffSet1->LocalPlaneSurfaceIntersect(..)`: found flag and the solution's start
/// parameters `[u0, v0, u1, v1]`.
fn surface_local_plane_surface_intersect(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    other: &Surface,
    other_domain: &Extent2d,
    tol: f64,
    uv0: Point2,
    uv1: Point2,
    plane_origin: Point3,
    plane_normal: Vec3,
) -> Result<(bool, [f64; 4])> {
    Ok(
        match surface.local_plane_surface_intersect(
            k,
            domain,
            other,
            other_domain,
            tol,
            uv0,
            uv1,
            plane_origin,
            plane_normal,
        )? {
            Some(s) => (true, [s.uv.x, s.uv.y, s.other_uv.x, s.other_uv.y]),
            None => (false, [0.0; 4]),
        },
    )
}

/// `SM_CAST_PTR(SmOffsetSurface, surface)->SetOffsetDistance(distance)` (`NULL`
/// dereferenced upstream for a surface that is not an offset). The intersector shares
/// the surface with its owner, as upstream (the offset distance is a `Cell`).
fn set_offset_distance(surface: &Surface, distance: f64) -> Result<()> {
    match surface.kind() {
        SurfaceKind::OffsetSurface(o) => {
            o.set_offset_distance(distance);
            Ok(())
        }
        _ => Err(SolidError::null_pointer(
            "intersect::surface_intersector::p1::set_offset_distance",
        )),
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
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    /// `m_apUserPointer` follows copies and assignment, and compares by identity.
    #[test]
    fn user_pointers_follow_copies() {
        let held = UserSurface(std::sync::Arc::new(plane(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        )));
        let mut a = TsectPnt::new();
        assert_eq!(a.user_pointer, [None, None]);
        a.user_pointer = [Some(held.clone()), None];
        let copied = a.clone();
        assert_eq!(copied.user_pointer, a.user_pointer);
        let mut assigned = TsectPnt::zeroed();
        assigned.assign(&a);
        assert_eq!(assigned.user_pointer, a.user_pointer);
        let other = UserSurface(std::sync::Arc::new((*held.0).clone()));
        assert_ne!(Some(other), a.user_pointer[0]);
    }

    #[test]
    fn default_point() {
        let p = TsectPnt::new();
        assert_eq!(p.point_type, IntersectionPointType::Unknown);
        assert!(!p.is_bounded());
        assert_eq!(p.crv_pos(), Vec3::UNDEF);
    }

    #[test]
    fn assign_skips_unused_entries() {
        let mut a = TsectPnt::new();
        let mut b = TsectPnt::new();
        b.surface_pv[0][1][2] = Vec3::new(1.0, 2.0, 3.0);
        b.surface_pv[0][2][2] = Vec3::new(0.0, 0.0, 1.0);
        a.assign(&b);
        assert_eq!(a.surface_pv[0][1][2], Vec3::UNDEF);
        assert_eq!(a.surface_pv[0][2][2], Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn construct_two_planes() {
        let k = &Kernel::default();
        let s1 = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let s2 = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let si = SurfaceIntersector::new(k, &s1, &d, &s2, &d, false).unwrap();
        assert!(!si.doing_self_intersection);
        assert!(!si.closed_u[0] && !si.closed_v[1]);
        let expect = (2.0f64.sqrt() / 2.0 * 2.0) / 1000.0;
        assert!((si.this_approx_tol_3d - expect).abs() < 1e-15);
        assert_eq!(si.compute_step_size(0.0), 100.0 * si.this_approx_tol_3d);
        assert_eq!(si.compute_step_size(0.5), 0.5);
        let same = SurfaceIntersector::new(k, &s1, &d, &s1, &d, false).unwrap();
        assert!(same.doing_self_intersection);
    }

    #[test]
    fn lists_and_reverse() {
        let k = &Kernel::default();
        let s1 = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut si = SurfaceIntersector::new(k, &s1, &d, &s1, &d, false).unwrap();
        let mut ids = Vec::new();
        for i in 0..3 {
            let mut p = TsectPnt::new();
            p.curve_parameter = f64::from(i) * 2.0;
            p.curve_pv[1] = Vec3::new(1.0, 0.0, 0.0);
            p.uv_curve_pv = [[Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)]; 2];
            ids.push(si.alloc_point(p));
        }
        for &id in &ids {
            si.add_point_to_curve(id).unwrap();
        }
        assert!(si.add_point_to_curve(ids[0]).is_err());
        si.reverse_curve_direction().unwrap();
        assert_eq!(si.curve_points(), &[ids[2], ids[1], ids[0]]);
        assert_eq!(si.point(ids[2]).unwrap().curve_parameter, 0.0);
        assert_eq!(si.point(ids[0]).unwrap().curve_parameter, 4.0);
        assert_eq!(
            si.point(ids[0]).unwrap().crv_deriv(),
            Vec3::new(-1.0, 0.0, 0.0)
        );
    }

    #[test]
    fn empty_reverse_is_null_pointer() {
        let k = &Kernel::default();
        let s1 = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut si = SurfaceIntersector::new(k, &s1, &d, &s1, &d, false).unwrap();
        assert!(matches!(
            si.reverse_curve_direction(),
            Err(SolidError::Upstream { code: 1006, .. })
        ));
    }

    #[test]
    fn flush_needs_knots() {
        let mut curves = Vec::new();
        flush_crv(3, &[], &[], &mut curves).unwrap();
        assert!(curves.is_empty());
        let pts = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(3.0, 1.0, 0.0),
        ];
        flush_crv(3, &pts, &[0.0, 1.0], &mut curves).unwrap();
        assert_eq!(curves.len(), 1);
    }
}
