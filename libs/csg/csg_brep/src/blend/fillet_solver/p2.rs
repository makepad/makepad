//! Fillet solver, part 2: analytic fillet geometry (cylinder, cone, torus), the
//! plane, rail/edge-use and rail/rail point solves, the cliff roll-over side
//! geometry, corner lookup and error recording.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp`, `source/SMLib/inc/SmFilletSolver.h`.
//!
//! | Rust                                                     | Upstream                                     |
//! |----------------------------------------------------------|----------------------------------------------|
//! | [`FilletSolver::fillet_corner`]                          | `SmFilletSolver::GetFilletCorner`            |
//! | [`FilletSolver::calc_cylinder_fillet_geom`], [`FilletSolver::cylinder_fillet_geom_parts`] | `SmFilletSolver::CalcCylinderFilletGeom` |
//! | [`FilletSolver::calc_cone_fillet_geom`], [`FilletSolver::cone_fillet_geom_parts`]         | `SmFilletSolver::CalcConeFilletGeom`     |
//! | [`FilletSolver::calc_torus_fillet_geom`], [`FilletSolver::torus_fillet_geom_parts`]       | `SmFilletSolver::CalcTorusFilletGeom`    |
//! | [`FilletSolver::point_on_plane_solve`]                   | `SmFilletSolver::PointOnPlaneSolve`          |
//! | [`FilletSolver::rail_edgeuse_intersect`]                 | `SmFilletSolver::RailEdgeuseIntersect`       |
//! | [`FilletSolver::rail_rail_intersect`]                    | `SmFilletSolver::RailRailIntersect`          |
//! | [`find_rollover_side_edge`]                              | `sm_FindRolloverSideEdge` (static)           |
//! | [`FilletSolver::create_cliff_roll_over_side_fg`]         | `SmFilletSolver::CreateCliffRollOverSideFG`  |
//! | [`FilletSolver::record_fillet_error`], [`FilletSolver::record_fillet_error_in`], [`SolverErrorObject`] | `SmFilletSolver::RecordFilletError` |
//!
//! Deviations:
//! - **Methods of p1's solver.** Everything here is a method of p1's [`FilletSolver`];
//!   methods that call a virtual take the solver as `&mut dyn FilletSolverOps` (as
//!   p1's `ComputeSurfaceValues`). The p1 function objects are wrapped privately into
//!   [`EvalNFunctions`] for [`LocalSolveNd`].
//! - **Fillet geometries.** The ported `SmFilletGeom`, `SmFilletEdge`, `SmFilletVertex`
//!   and the other pseudo-B-rep records (`blend.fillet_geom_p1`/`_p2`/`_p5`/`_p6`) live
//!   in the [`FilletBrep`] the executive owns (`SmFilletExecutive::GetPseudoBrep`): the
//!   methods that reach it take it as `pseudo`, and the solver's `SmFilletGeom*`s are
//!   [`FilletGeomId`]s of it (as in `blend.fillet_solver_p1`). `CreateCliffRollOverSideFG`
//!   calls [`FilletBrep::make_fillet_edge`] and [`FilletBrep::calc_edge_on_edge`], and
//!   links its new rail's edge uses directly onto the pseudo-B-rep arena record
//!   (`PreInsert`/`PostInsert`, as `blend.fillet_geom_p5` does). `RecordFilletError`
//!   records into the caller's [`ErrorInfo`] via [`FilletSolver::record_fillet_error_in`]
//!   ([`FilletSolver::record_fillet_error`] returns [`SolidError::Unported`] when the
//!   caller holds no `ErrorInfo`). The
//!   analytic `Calc*FilletGeom` methods build the fillet surface, rail curves and centre
//!   line first ([`FilletGeomParts`], `*_fillet_geom_parts`) and then store them into
//!   the fillet geometry (`m_pFilletSurface`, `SmFilletEdge::SetCurve`, `SetInterval`,
//!   `m_pCenterLineCurve`); upstream interleaves the two, which changes only the fillet
//!   geometry's state on a failure. The centre line is stored as the `SmBSplineCurve`
//!   part of the `SmLine` upstream builds.
//! - **Arguments.** Out-params become return values (`rbFoundSolution` /
//!   `rbFoundIntersection` the returned `bool` / `Option`); `SmContext` is the
//!   [`Kernel`]; `SmFilletErrorType` is [`FilletErrorType`] (or its raw value on
//!   [`FilletSolver::record_fillet_error`]); `SmFilletCorner*` is
//!   `blend.fillet_geom_p1`'s opaque [`FilletCornerId`].
//! - **Undefined inputs.** Where upstream dereferences `NULL` (missing edge use, edge,
//!   offset surface, surface generator, executive, UV trim curve), reads an
//!   uninitialised `lSurf2Index`, indexes past an array (`rTsectPnts` empty, a rail
//!   index above 1, `m_adUserDoubles` past `SM_MAX_USER_DOUBLES` in
//!   `PointOnPlaneSolve`, `pCC` past its end in `sm_FindRolloverSideEdge`) or wraps an
//!   unsigned size (`RailRailIntersect` with Jacobian sizes summing below 2), the port
//!   returns [`SolidError::InvalidInput`]. Writes of the solver intervals past the
//!   Jacobian size (upstream: `SM_ASSERT`, then into the fixed `SmExtentNd` storage)
//!   are dropped; the solve then rejects intervals not sized like the guess.
//! - **Callback sizes.** The Jacobian-size additions in the rail/edge-use and rail/rail
//!   intersections are checked and return [`SolidError::InvalidInput`] on overflow.
//! - **Iteration.** `sm_FindRolloverSideEdge`'s `while (TRUE)` advances its interval
//!   index on every pass that does not return, so it runs at most `pCC.len()` times;
//!   there is no recursion.

use crate::blend::fillet_custom_solver::OffsetSurface;
use crate::blend::fillet_executive::p3::ErrorInfo;
use crate::blend::fillet_geom::p1::{
    FilletBrep, FilletCornerId, FilletEdgeType, FilletErrorType, FilletGeom, FilletGeomId,
    FilletGeomType, FilletSolverId, FilletVertex, FilletVertexType, FilletVertexuse,
};
use crate::blend::fillet_geom::p2::{srf_srf_intersection, SrfSrfOptions};
use crate::blend::fillet_geom::p3::SplitCorners;
use crate::blend::fillet_geom::p4::{set_edge_curve, set_vertex_point};
use crate::blend::fillet_solver::p1::{
    check_edge_curve_domains, test_manifold_tangent_rollover, test_side_edge_convexity, FilStatus,
    FilletSolver, FilletSolverOps, FilletSphereSolveEnfo, FilletSurfaceGenerator,
    FilletSurfaceGeneratorType, RailRailIntersectEnfo, RailUvCurveIntersectEnfo,
};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::Curve;
use crate::curve::line::Line;
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::local_solve_nd::{BoundaryHandler, EvalNFunctions, LocalSolveNd};
use crate::geom::matrix::Matrix;
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::ANG_TOL_RAD;
use crate::geom::vector2d::Point2;
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::point_class::ClassObject;
use crate::intersect::point_class::PointClassType;
use crate::intersect::surface_intersector::p1::{
    IntersectionPointType, SrfIndex, TsectPnt, MAX_USER_DOUBLES,
};
use crate::kernel::Kernel;
use crate::math::{rad_to_deg, Point3, Vec3, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::p1::NurbCircleParam;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::plane::Plane;
use crate::surface::surf_of_revolution::SurfOfRevolution;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1 as eu;
use crate::topology::edgeuse::p3::get_or_create_uv_trim_curve;
use crate::topology::face::p2 as face_p2;
use crate::topology::{edge, vertexuse, Brep, EdgeId, EdgeuseId, FaceId, VertexId};
use crate::topology::{vertex, Vertexuse};

const OP: &str = "blend::fillet_solver::p2";

/// `dAnalyticTol` of the `Calc*FilletGeom` methods.
const ANALYTIC_TOL: f64 = 1.0e-8;

/// `pFilletGeom->GetRail(i)`, dereferenced (`NULL` is `SM_ERR_NULL_POINTER`).
fn geom_rail(pseudo: &FilletBrep, g: FilletGeomId, i: usize) -> Result<EdgeId> {
    pseudo
        .geom(g)?
        .rail(i)
        .ok_or_else(|| SolidError::null_pointer(OP))
}

/// An object referenced by [`FilletSolver::record_fillet_error_in`] in an executive
/// [`ErrorInfo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SolverErrorObject {
    /// The filleted target edge (`pFilletedEdge`).
    FilletedEdge(EdgeId),
    /// The fillet surface of a solver [`FilletGeom`] (`pThisGeom->GetFilletSurface()`).
    FilletSurface(FilletGeomId),
    /// The centre-line curve of a solver [`FilletGeom`]
    /// (`pThisGeom->GetCenterLineCurve()`).
    CenterLineCurve(FilletGeomId),
    /// A pseudo-B-rep rail edge (`pThisGeom->GetRail(0)`).
    FilletEdge(EdgeId),
}

fn fillet_error_type(code: i64) -> Result<FilletErrorType> {
    Ok(match code {
        0 => FilletErrorType::Ok,
        1 => FilletErrorType::ExecUninitialized,
        2 => FilletErrorType::InvalidInput,
        3 => FilletErrorType::VertexProblem,
        4 => FilletErrorType::EdgeProblem,
        5 => FilletErrorType::BadInt,
        6 => FilletErrorType::BadTrim,
        7 => FilletErrorType::BadMerge,
        _ => return Err(invalid("unknown fillet error type")),
    })
}

/// The curve and surface handles of `CreateCliffRollOverSideFG`'s
/// `GlobalCurveIntersect` (the cross-section curve and the side surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliffHandle {
    /// `*pCurve`.
    Curve,
    /// `pSideSurface`.
    Surface,
}

impl SolutionObject for CliffHandle {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Curve => ObjectKind::Curve,
            Self::Surface => ObjectKind::Surface,
        }
    }
}

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

fn sm_err() -> SolidError {
    SolidError::sm_err(OP)
}

/// The analytic fillet geometry a `Calc*FilletGeom` method builds before storing it into
/// the fillet geometry: the fillet surface (`m_pFilletSurface`), the rail curves
/// (`GetRail(i)->SetCurve`) with the interval each rail edge gets (`SetInterval`, `None`
/// where upstream sets none) and the centre line (`m_pCenterLineCurve`, `None` where
/// upstream sets none).
#[derive(Debug, Clone)]
pub struct FilletGeomParts {
    /// The fillet surface.
    pub surface: Surface,
    /// The rail curves.
    pub rails: [Curve; 2],
    /// The rail edge intervals.
    pub rail_intervals: [Option<Extent1d>; 2],
    /// The centre line.
    pub center_line: Option<Curve>,
}

/// The reference arguments of [`find_rollover_side_edge`] (`rlCurrIndx` and
/// `rpRolloverCornerEU` are read too; each path writes only some of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RolloverSideEdge {
    /// `rlCurrIndx`.
    pub curr_index: usize,
    /// `rpSideEdge`.
    pub side_edge: Option<EdgeId>,
    /// `rbIsTangentialSideEdge`.
    pub is_tangential_side_edge: bool,
    /// `rbTrimSideFaces`.
    pub trim_side_faces: bool,
    /// `rbOppositeNormals`.
    pub opposite_normals: bool,
    /// `rpRolloverCornerEU`.
    pub rollover_corner_eu: Option<EdgeuseId>,
    /// `rpRolloverCornerV`.
    pub rollover_corner_v: Option<VertexId>,
}

// ---- function-object adapters ---------------------------------------------------------

/// The function objects with the caller's kernel, which their `Evaluate` reads.
struct SphereEval<'a, 'k>(FilletSphereSolveEnfo<'a>, &'k Kernel);

impl EvalNFunctions for SphereEval<'_, '_> {
    fn evaluate(&mut self, x: &[f64], f: &mut [f64], j: Option<&mut Matrix>) -> Result<bool> {
        self.0.evaluate(self.1, x, f, j)
    }
}

struct RailUvEval<'a, 'k>(RailUvCurveIntersectEnfo<'a>, &'k Kernel);

impl EvalNFunctions for RailUvEval<'_, '_> {
    fn evaluate(&mut self, x: &[f64], f: &mut [f64], j: Option<&mut Matrix>) -> Result<bool> {
        self.0.evaluate(self.1, x, f, j)
    }
}

struct RailRailEval<'a, 'k>(RailRailIntersectEnfo<'a>, &'k Kernel);

impl EvalNFunctions for RailRailEval<'_, '_> {
    fn evaluate(&mut self, x: &[f64], f: &mut [f64], j: Option<&mut Matrix>) -> Result<bool> {
        self.0.evaluate(self.1, x, f, j)
    }
}

// ---- small helpers --------------------------------------------------------------------

/// `sIntervals[i] = e`; a write past the Jacobian size is dropped (module notes).
fn set_interval(intervals: &mut ExtentNd, i: usize, e: Extent1d) {
    if let Some(slot) = intervals.get_mut(i) {
        *slot = e;
    }
}

fn srf_of_rail(rail: usize) -> Result<SrfIndex> {
    match rail {
        0 => Ok(SrfIndex::First),
        1 => Ok(SrfIndex::Second),
        _ => Err(invalid("rail index out of range")),
    }
}

fn sol_at(x: &[f64], i: usize) -> Result<f64> {
    x.get(i)
        .copied()
        .ok_or_else(|| invalid("solution index out of range"))
}

/// A copy of the solver's offset surface of `rail` (`*GetSurface(rail)`), to hand to
/// `LoadInitialValues` alongside the solver; written back with [`put_surface`].
fn take_surface(s: &FilletSolver, rail: usize) -> Result<OffsetSurface> {
    s.surface(rail)?
        .map(|slot| slot.surface.clone())
        .ok_or_else(|| invalid("no offset surface"))
}

fn put_surface(s: &mut FilletSolver, rail: usize, surface: OffsetSurface) -> Result<()> {
    if let Some(slot) = s.surface_mut(rail)? {
        slot.surface = surface;
    }
    Ok(())
}

/// `GetSurface(rail)->EvaluatePoint(uv)` (keeping an extension it makes).
fn eval_offset(k: &Kernel, s: &mut FilletSolver, rail: usize, uv: Point2) -> Result<Point3> {
    s.surface_mut(rail)?
        .ok_or_else(|| invalid("no offset surface"))?
        .surface
        .surface
        .evaluate_point_extending(k, uv)
}

fn has_surface(s: &FilletSolver, rail: usize) -> Result<bool> {
    Ok(s.surface(rail)?.is_some())
}

/// `pCurve->Evaluate(t, 1, TRUE, sPV)`: point and first derivative.
fn curve_pv(k: &Kernel, c: &Curve, t: f64) -> Result<(Point3, Vec3)> {
    let pv = c.evaluate(k, t, 1, true, false)?;
    let p = pv
        .first()
        .copied()
        .ok_or_else(|| invalid("short curve evaluation"))?;
    let d = pv
        .get(1)
        .copied()
        .ok_or_else(|| invalid("short curve evaluation"))?;
    Ok((p, d))
}

/// The edge of the solver's first edge use (`GetEdgeuse(0)->GetEdge()`).
fn filleted_edge(s: &FilletSolver, b: &Brep) -> Result<EdgeId> {
    let eu0 = s.edgeuse(0)?.ok_or_else(|| invalid("no edge use"))?;
    eu::edge(b, eu0)?.ok_or_else(|| invalid("edge use without edge"))
}

/// The filleted edge's curve after `sm_CheckEdgeCurveDomains` (the trimmed copy, or the
/// edge's own curve).
fn checked_edge_curve(k: &Kernel, s: &FilletSolver, b: &Brep, e: EdgeId) -> Result<Curve> {
    if let Some(copy) = check_edge_curve_domains(k, b, e, s.extend_before, s.extend_after)? {
        return Ok(copy);
    }
    // NER(pCurve)
    edge::curve(b, e)?
        .cloned()
        .ok_or_else(|| SolidError::null_pointer(OP))
}

fn is_circular_fsg(s: &FilletSolver) -> Result<bool> {
    let fsg = s
        .fsg
        .as_ref()
        .ok_or_else(|| invalid("no fillet surface generator"))?;
    Ok(fsg.fillet_surface_generator_type() == FilletSurfaceGeneratorType::Circular)
}

/// The solver's surface generator type.
fn fsg_type(s: &FilletSolver) -> Result<FilletSurfaceGeneratorType> {
    s.fsg
        .as_ref()
        .map(FilletSurfaceGenerator::fillet_surface_generator_type)
        .ok_or_else(|| invalid("no fillet surface generator"))
}

/// `sm_TurnIntoHalfPlane` (fork addition): turns `point` about the axis (through `origin`,
/// unit direction `axis`) into the half-plane, bounded by the axis, through `target`. A
/// point on the axis, or a target on it (within `tol`), stays.
fn turn_into_half_plane(origin: Point3, axis: Vec3, target: Point3, tol: f64, point: &mut Point3) {
    let p = *point - origin;
    let t = target - origin;
    let along = p.dot(axis);
    let p_rad = p - axis * along;
    let t_rad = t - axis * t.dot(axis);
    let (p_len, t_len) = (p_rad.length(), t_rad.length());
    if p_len < tol || t_len < tol {
        return;
    }
    *point = origin + axis * along + t_rad * (p_len / t_len);
}

/// `rTsectPnts[0]` and `rTsectPnts[GetSize() - 1]`.
fn first_last(tsects: &[TsectPnt]) -> Result<(&TsectPnt, &TsectPnt)> {
    let first = tsects.first().ok_or_else(|| invalid("no tsect points"))?;
    let last = tsects.last().ok_or_else(|| invalid("no tsect points"))?;
    Ok((first, last))
}

/// `SetupOffsetValues(rTsectPnt, &dRadius)`, returning the radius.
fn offset_radius(ops: &mut dyn FilletSolverOps, t: &TsectPnt) -> Result<f64> {
    let mut r = 0.0;
    ops.setup_offset_values(t, Some(&mut r))?;
    Ok(r)
}

/// Stores the parts into the fillet geometry (`pFilletGeom` or `m_vFilletGeoms[0]`):
/// `m_pFilletSurface = surface`, then for each rail `GetRail(i)->SetCurve(curve, TRUE)`
/// (the edge owns the curve: `SetOwner`), `SetInterval` where upstream sets one, and
/// `m_pCenterLineCurve`.
fn store_parts(
    s: &FilletSolver,
    pseudo: &mut FilletBrep,
    fillet_geom: Option<FilletGeomId>,
    parts: FilletGeomParts,
) -> Result<()> {
    let fg = match fillet_geom {
        Some(fg) => fg,
        None => s.first_fillet_geom()?,
    };
    let g = fg;
    let FilletGeomParts {
        surface,
        rails,
        rail_intervals,
        center_line,
    } = parts;
    pseudo.geom_mut(g)?.set_fillet_surface(Some(surface));
    for (i, (curve, ivl)) in rails.into_iter().zip(rail_intervals).enumerate() {
        let rail = geom_rail(pseudo, g, i)?;
        set_edge_curve(pseudo.brep_mut(), rail, Some(curve))?;
        if let Some(ivl) = ivl {
            edge::set_interval(pseudo.brep_mut(), rail, &ivl)?;
        }
    }
    if let Some(center_line) = center_line {
        // The native base pointer retains the actual derived curve object.
        bspline_part(&center_line)?;
        pseudo.geom_mut(g)?.center_line_curve = Some(center_line);
    }
    Ok(())
}

/// The `SmBSplineCurve` part of a curve (`SM_CAST_PTR(SmBSplineCurve, ..)`: the analytic
/// curves derive from it upstream).
fn bspline_part(curve: &Curve) -> Result<&BSplineCurve> {
    use crate::curve::curve::CurveKind;
    match curve.kind() {
        CurveKind::BSpline(c) => Ok(c),
        CurveKind::Line(c) => Ok(c.base()),
        CurveKind::Ellipse(c) => Ok(c.base()),
        CurveKind::Hyperbola(c) => Ok(c.base()),
        CurveKind::Parabola(c) => Ok(c.base()),
        _ => Err(SolidError::null_pointer(OP)),
    }
}

/// `GetOrCreateUVTrimCurve` hands back an `SmBSplineCurve*`: only the kinds deriving
/// from it (B-splines, lines, ellipses, hyperbolas, parabolas) can be a UV trim curve.
fn check_uv_bspline_kind(c: &Curve) -> Result<()> {
    use crate::curve::curve::CurveKind;
    match c.kind() {
        CurveKind::BSpline(_)
        | CurveKind::Line(_)
        | CurveKind::Ellipse(_)
        | CurveKind::Hyperbola(_)
        | CurveKind::Parabola(_) => Ok(()),
        _ => Err(invalid("UV trim curve is not an SmBSplineCurve")),
    }
}

/// `SmLine::CreateLineSegment(ctx, 3, a, b, pLine)` then `EditParameterization(ivl)`.
fn line_segment(a: Point3, b: Point3, ivl: &Extent1d) -> Result<Line> {
    let mut l = Line::create_line_segment(3, a, b, None)?;
    l.edit_parameterization(ivl)?;
    Ok(l)
}

impl FilletSolver {
    /// `GetFilletCorner(lIndex)`: the fillet corner at the start (0) or end (1) vertex
    /// of the filleted edge; `None` for another index.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::vertex`]; [`SolidError::InvalidInput`] without an
    /// executive (upstream dereferences `NULL`). `corners` is the executive's
    /// `GetFilletCornerOfVertex` (a vertex without corner, or none, is `NULL`).
    pub fn fillet_corner(
        &self,
        b: &Brep,
        index: usize,
        corners: &dyn SplitCorners,
    ) -> Result<Option<FilletCornerId>> {
        if index > 1 {
            return Ok(None);
        }
        let vtx = self.vertex(b, index)?;
        if self.executive.is_none() {
            return Err(invalid("no fillet executive"));
        }
        Ok(vtx.and_then(|v| corners.corner_of_vertex(v)))
    }

    /// `CalcCylinderFilletGeom(rTsectPnts, pFilletGeom)`: builds the cylinder of a
    /// constant-radius circular fillet along a straight edge
    /// ([`FilletSolver::cylinder_fillet_geom_parts`]) and stores it into `fillet_geom`
    /// (`m_vFilletGeoms[0]` for `None`) of the pseudo B-rep `pseudo`; `b` is the B-rep being
    /// filleted.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::cylinder_fillet_geom_parts`]; then
    /// [`SolidError::InvalidInput`] / `SM_ERR_NULL_POINTER` for a geometry or rail that
    /// is missing from `pseudo` (upstream dereferences `NULL`).
    pub fn calc_cylinder_fillet_geom(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &Brep,
        pseudo: &mut FilletBrep,
        tsects: &[TsectPnt],
        fillet_geom: Option<FilletGeomId>,
    ) -> Result<()> {
        let parts = Self::cylinder_fillet_geom_parts(ops, k, b, tsects)?;
        store_parts(ops.solver(), pseudo, fillet_geom, parts)
    }

    /// The geometry of [`FilletSolver::calc_cylinder_fillet_geom`]: the cylinder
    /// (`CreateConePatch` with equal radii, UV swapped, `CopyAndAddAnalytics`), the two
    /// rail lines and the centre line, each parameterised on the cylinder's u range.
    /// The ends are extended by the extension factor where the solver extends.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the generator is not circular, the edge curve is not a line, the
    /// radius changes, or the arc angle is degenerate or near `pi`; those of
    /// `SetupOffsetValues` (`SM_ERR` for the base solver), the evaluations and the
    /// constructors; [`SolidError::InvalidInput`] where upstream dereferences `NULL`
    /// or for no tsect points.
    pub fn cylinder_fillet_geom_parts(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &Brep,
        tsects: &[TsectPnt],
    ) -> Result<FilletGeomParts> {
        let s = ops.solver();
        let e = filleted_edge(s, b)?;

        // Check for Edge's curve domain being bigger than the Edge domain.
        let curve = checked_edge_curve(k, s, b, e)?;

        // no work - not a circular cross-section on a straight line
        if !is_circular_fsg(s)? || curve.is_line(k, 3, ANALYTIC_TOL)?.is_none() {
            return Err(sm_err());
        }

        // get start and end Offset Surface UV points
        let (t_first, t_last) = first_last(tsects)?;
        let start_uv0 = t_first.uv_pos(SrfIndex::First);
        let start_uv1 = t_first.uv_pos(SrfIndex::Second);
        let end_uv0 = t_last.uv_pos(SrfIndex::First);
        let end_uv1 = t_last.uv_pos(SrfIndex::Second);

        // set OffsetSurface offset = radius for begin point and get sCenter0
        let mut radius0 = offset_radius(ops, t_first)?;
        let mut center0 = eval_offset(k, ops.solver_mut(), 0, start_uv0)?;

        // set OffsetSurface offset = radius for end point and get sCenter1
        let mut radius1 = offset_radius(ops, t_last)?;
        let mut center1 = eval_offset(k, ops.solver_mut(), 0, end_uv0)?;

        // check state - cylinder case = constant radius fillets
        if (radius1 - radius0).abs() > EFF_ZERO {
            // Non-cylinder case
            return Err(sm_err());
        }

        // Evaluate points on base surfaces.  Use a zero-offset offset surface
        // instead of the base surface, in order to handle extended surfaces.
        let s = ops.solver_mut();
        s.set_surfaces_to_zero_offset()?;
        let mut start_pnt0 = eval_offset(k, s, 0, start_uv0)?;
        let mut start_pnt1 = eval_offset(k, s, 1, start_uv1)?;
        let mut end_pnt0 = eval_offset(k, s, 0, end_uv0)?;
        let mut end_pnt1 = eval_offset(k, s, 1, end_uv1)?;
        s.reload_surface_offsets()?;

        // get extension vectors along each rail curve and the center curve
        let f = s.surface_extension_factor;
        let ext0 = f * (end_pnt0 - start_pnt0);
        let ext1 = f * (end_pnt1 - start_pnt1);
        let center_ext = f * (center1 - center0);

        // when extending the start of the fillet
        if s.extend_before {
            start_pnt0 -= ext0;
            start_pnt1 -= ext1;
            center0 -= center_ext;
            radius0 = center0.distance(start_pnt0);
        }

        // when extending the end of the fillet
        if s.extend_after {
            end_pnt0 += ext0;
            end_pnt1 += ext1;
            center1 += center_ext;
            radius1 = center1.distance(end_pnt0);
        }
        let _ = radius1;

        // get the cylinder parameters
        let mut x_vec = start_pnt0 - center0;
        let mut y_vec = start_pnt1 - center0;
        let axis_of_revolution = center1 - center0;
        let height = axis_of_revolution.length();

        // Get angle of revolution
        let mut angle = axis_of_revolution.ccw_angle_between(x_vec, y_vec)?;

        // check state - avoid degenerate zero arc cylinders
        if angle.abs() < EFF_ZERO_SQRT {
            return Err(sm_err()); // Degenerate case here
        }

        // check state - avoid fillet arcs equal or greater than Pi
        if angle.abs() > PI - EFF_ZERO_SQRT {
            return Err(sm_err()); // Nearly 180 degree case here
        }

        // swap center to start vectors to force rotation angle to be positive
        if angle < 0.0 {
            std::mem::swap(&mut x_vec, &mut y_vec);
            angle = axis_of_revolution.ccw_angle_between(x_vec, y_vec)?;
        }

        // check state - avoid degenerate zero arc cylinders
        if angle < EFF_ZERO {
            return Err(sm_err());
        }

        // convert radians to degrees
        let angle = rad_to_deg(angle);

        // orthogonalize and normalize sXVec and sYVec
        let mut placement = Axis2Placement::default();
        y_vec = axis_of_revolution.cross(x_vec);
        let x_vec = x_vec.unitize()?;
        let y_vec = y_vec.unitize()?;

        // set up a coordinate system for cylinder center line
        placement.set_canonical(center0, x_vec, y_vec)?;

        // Create a temporary cylindrical cone patch
        let mut cone_patch = BSplineSurface::create_cone_patch(
            &placement,
            radius0,
            radius0,
            0.0,
            angle,
            height,
            NurbCircleParam::Quadratic,
        )?;

        // CreateConePatch() makes a STEP-parameterized cone,
        // we go the other way: u along the edge, v around the fillet surface.
        ignore_status(cone_patch.swap_uv())?;

        // Copy pConePatch, when possible as an analytic surface
        let cylinder = cone_patch.copy_and_add_analytics(k)?;

        // get cylinder u parameter range
        let uv_domain = cylinder.natural_uv_domain_with_nurb(k)?;
        let rail_crv_ivl = Extent1d::from_min_max(uv_domain.min.x, uv_domain.max.x);

        // Calc Rail0, Rail1 - set their extent = cylinder u parameter range
        let rail0 = line_segment(start_pnt0, end_pnt0, &rail_crv_ivl)?;
        let rail1 = line_segment(start_pnt1, end_pnt1, &rail_crv_ivl)?;

        // Calc and store centerline
        let center_line = line_segment(center0, center1, &rail_crv_ivl)?;

        Ok(FilletGeomParts {
            surface: cylinder,
            rails: [Curve::from(rail0), Curve::from(rail1)],
            rail_intervals: [None, None],
            center_line: Some(Curve::from(center_line)),
        })
    }

    /// `CalcConeFilletGeom(rTsectPnts, pFilletGeom)`: builds the cone of a circular
    /// fillet along a straight edge ([`FilletSolver::cone_fillet_geom_parts`]) and
    /// stores it into `fillet_geom` (`m_vFilletGeoms[0]` for `None`) of the pseudo
    /// B-rep `pseudo`.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::cone_fillet_geom_parts`] and of
    /// [`FilletSolver::calc_cylinder_fillet_geom`]'s store.
    pub fn calc_cone_fillet_geom(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &Brep,
        pseudo: &mut FilletBrep,
        tsects: &[TsectPnt],
        fillet_geom: Option<FilletGeomId>,
    ) -> Result<()> {
        let parts = Self::cone_fillet_geom_parts(ops, k, b, tsects)?;
        store_parts(ops.solver(), pseudo, fillet_geom, parts)
    }

    /// The geometry of [`FilletSolver::calc_cone_fillet_geom`]: the first rail's line
    /// revolved about the centre line (`SmSurfOfRevolution::CreateCanonical`, its sweep
    /// limited to the fillet angle, UV swapped), its generator copy and its v-max iso
    /// curve as rails, and the centre line extended at both ends.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the generator is not circular, the edge curve is not a line, or the
    /// arc angle is degenerate or near `pi`; those of `SetupOffsetValues`, the
    /// evaluations and the constructors; `SM_ERR_NULL_POINTER` for a missing generator;
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL`.
    pub fn cone_fillet_geom_parts(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &Brep,
        tsects: &[TsectPnt],
    ) -> Result<FilletGeomParts> {
        let s = ops.solver();
        let e = filleted_edge(s, b)?;

        // Check for Edge's curve domain being bigger than the Edge domain.
        let curve = checked_edge_curve(k, s, b, e)?;

        // no work - not a circular cross-section on a line
        if !is_circular_fsg(s)? || curve.is_line(k, 3, ANALYTIC_TOL)?.is_none() {
            return Err(sm_err());
        }

        // Get some parameters for surf-of-revo creation
        let (t_first, t_last) = first_last(tsects)?;
        let start_uv0 = t_first.uv_pos(SrfIndex::First);
        let start_uv1 = t_first.uv_pos(SrfIndex::Second);
        let end_uv0 = t_last.uv_pos(SrfIndex::First);
        let end_uv1 = t_last.uv_pos(SrfIndex::Second);
        let _radius0 = offset_radius(ops, t_first)?;
        let mut center0 = eval_offset(k, ops.solver_mut(), 0, start_uv0)?;
        let _radius1 = offset_radius(ops, t_last)?;
        let mut center1 = eval_offset(k, ops.solver_mut(), 0, end_uv0)?;

        // Evaluate points on base surfaces.  Use a zero-offset offset surface
        // instead of the base surface, in order to handle extended surfaces.
        let s = ops.solver_mut();
        s.set_surfaces_to_zero_offset()?;
        let mut start_pnt0 = eval_offset(k, s, 0, start_uv0)?;
        let start_pnt1 = eval_offset(k, s, 1, start_uv1)?;
        let mut end_pnt0 = eval_offset(k, s, 0, end_uv0)?;
        let _end_pnt1 = eval_offset(k, s, 1, end_uv1)?;
        s.reload_surface_offsets()?;

        // Get origin & axis of revolution
        let v0 = start_pnt0 - center0;
        let v1 = start_pnt1 - center0;
        let mut axis_of_revolution = (center1 - center0).unitize()?;
        let angle = axis_of_revolution.ccw_angle_between(v0, v1)?;
        if angle.abs() < EFF_ZERO_SQRT {
            return Err(sm_err()); // Degenerate case here
        }
        if angle.abs() > PI - EFF_ZERO_SQRT {
            return Err(sm_err()); // Nearly 180 degree case here
        }
        if angle < 0.0 {
            axis_of_revolution = -axis_of_revolution;
        }
        let angle = axis_of_revolution.ccw_angle_between(v0, v1)?;
        if angle < EFF_ZERO {
            return Err(sm_err());
        }
        let angle = rad_to_deg(angle);

        let v0 = v0.unitize()?;
        let mut placement = Axis2Placement::default();
        let x_vec = v0;
        let y_vec = axis_of_revolution.cross(x_vec);
        let x_vec = x_vec.unitize()?;
        let y_vec = y_vec.unitize()?;
        placement.set_canonical(center0, x_vec, y_vec)?;
        let _ = placement;

        // Calc generator curve (Rail0)
        let f = s.surface_extension_factor;
        let dir = end_pnt0 - start_pnt0;
        let ext = dir * f;
        if s.extend_before {
            start_pnt0 -= ext;
        }
        if s.extend_after {
            end_pnt0 += ext;
        }
        let gen_curve = BSplineCurve::create_line_segment(3, start_pnt0, end_pnt0)?;

        // Now, create surface of revolution
        let mut cone = SurfOfRevolution::create_canonical(
            k,
            Curve::from(gen_curve),
            center0,
            axis_of_revolution,
            false,
        )?;
        let mut step_domain = cone.step_uv_domain();
        let mut uv_max = step_domain.max;
        uv_max.x = angle;
        step_domain = Extent2d::from_points(step_domain.min, uv_max);
        cone.adjust_step_uv_domain(k, &step_domain)?;
        let mut cone = Surface::from(cone);
        ignore_status(cone.swap_uv(k))?;

        let uv_domain = cone.natural_uv_domain_with_nurb(k)?;
        let ivl = Extent1d::from_min_max(uv_domain.min.x, uv_domain.max.x);

        // Calc rail0
        let gen = match cone.kind() {
            crate::surface::surface::SurfaceKind::SurfOfRevolution(r) => r.gen_curve(),
            _ => None,
        }
        .ok_or_else(|| invalid("surface of revolution without generator"))?;
        let mut rail_crv0 = gen.copy(k)?;
        rail_crv0.edit_parameterization(k, &ivl)?;
        let crv_ivl0 = rail_crv0.natural_interval();

        // Calc Rail1
        let rail_crv1 = cone
            .create_iso_parametric_curve(k, SurfParamType::V, uv_domain.max.y, 0.0, None, false)?
            .curve;
        let crv_ivl1 = rail_crv1.natural_interval();

        // Calc centerline
        let dir = center1 - center0;
        let ext = dir * f;
        center0 -= ext;
        center1 += ext;
        let mut center_line = BSplineCurve::create_line_segment(3, center0, center1)?;
        center_line.edit_parameterization(&ivl)?;

        Ok(FilletGeomParts {
            surface: cone,
            rails: [rail_crv0, rail_crv1],
            rail_intervals: [Some(crv_ivl0), Some(crv_ivl1)],
            center_line: Some(Curve::from(center_line)),
        })
    }

    /// `CalcTorusFilletGeom(rTsectPnts, pFilletGeom)`: builds the torus of a
    /// constant-radius circular fillet along a circular edge
    /// ([`FilletSolver::torus_fillet_geom_parts`]) and stores it into `fillet_geom`
    /// (`m_vFilletGeoms[0]` for `None`) of the pseudo B-rep `pseudo`.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::torus_fillet_geom_parts`] and of
    /// [`FilletSolver::calc_cylinder_fillet_geom`]'s store.
    pub fn calc_torus_fillet_geom(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &Brep,
        pseudo: &mut FilletBrep,
        tsects: &[TsectPnt],
        fillet_geom: Option<FilletGeomId>,
    ) -> Result<()> {
        let parts = Self::torus_fillet_geom_parts(ops, k, b, pseudo, tsects, fillet_geom)?;
        store_parts(ops.solver(), pseudo, fillet_geom, parts)
    }

    /// The geometry of [`FilletSolver::calc_torus_fillet_geom`]: the cross-section arc
    /// through the rail points revolved about the edge circle's axis
    /// (`SmSurfOfRevolution::CreateCanonical`, the sweep extended where the solver
    /// extends, at most 360 degrees, the seam moved by `pi` for a full-circle torus of an
    /// open edge, rotated back by the extension angle when extending before), and its
    /// v-min and v-max iso curves as rails (no centre line).
    ///
    /// # Errors
    ///
    /// With the opt-in `Kernel::fixes.fillet_chamfer_cone_on_arc` (not upstream, WR-421) a
    /// linear generator is revolved too, into a cone.
    /// With `Kernel::fixes.fillet_torus_center_on_axis` (WR-422), a section centred on the
    /// arc's axis has its rail points turned into the half-planes through the edge's ends and
    /// takes the edge's start for the nearest point.
    ///
    /// `SM_ERR` when the generator is not circular, the edge curve is not an arc, the
    /// radius is tiny or changes, or the nearest point on the edge is not unique; those
    /// of `SetupOffsetValues`, the evaluations and the constructors;
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (also a rail
    /// missing from `pseudo` in the rail end checks of an open edge that does not extend
    /// after).
    ///
    /// The rail end checks may set the solver's extend-after flag
    /// (`m_bExtendAfter = TRUE`), which stays set.
    pub fn torus_fillet_geom_parts(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &Brep,
        pseudo: &FilletBrep,
        tsects: &[TsectPnt],
        fillet_geom: Option<FilletGeomId>,
    ) -> Result<FilletGeomParts> {
        let s = ops.solver();
        let e = filleted_edge(s, b)?;

        // Check for Edge's curve domain being bigger than the Edge domain.
        let curve = checked_edge_curve(k, s, b, e)?;

        // no work - not a circular cross-section on a circular arc (with
        // `fixes.fillet_chamfer_cone_on_arc`, or a linear one: its revolution is a cone)
        // Side effect: get circular arc RefFrame, Radius, StartAng, and EndAng.
        let section = fsg_type(s)?;
        let linear_section =
            k.fixes.fillet_chamfer_cone_on_arc && section == FilletSurfaceGeneratorType::Linear;
        if section != FilletSurfaceGeneratorType::Circular && !linear_section {
            return Err(sm_err());
        }
        let Some((ref_frame, _arc_radius, start_ang, _end_ang)) =
            curve.is_arc(k, 11, ANALYTIC_TOL)?
        else {
            return Err(sm_err());
        };

        // select new fillet surface storage spot
        let fillet_geom = match fillet_geom {
            Some(fg) => fg,
            None => s.first_fillet_geom()?,
        };
        let fillet_geom_id = fillet_geom;
        let null_ptr = || SolidError::null_pointer(OP);

        // Get rotation point and axis for surf-of-revolution creation
        let mut axis_of_revolution = ref_frame.z_axis();
        let mut origin = ref_frame.origin();

        // Locate the generating curve(arc)
        let (t_first, t_last) = first_last(tsects)?;
        let start_uv0 = t_first.uv_pos(SrfIndex::First);
        let start_uv1 = t_first.uv_pos(SrfIndex::Second);

        // get centerPoint = offsetSurface0(uvPnt0)
        let arc_center = eval_offset(k, ops.solver_mut(), 0, start_uv0)?;

        // get cross-section radius size at begin and end fillet points
        let radius = offset_radius(ops, t_first)?;
        let radius1 = offset_radius(ops, t_last)?;
        if radius < EFF_ZERO || (radius1 - radius).abs() > EFF_ZERO {
            return Err(sm_err());
        }

        // Evaluate points on base surfaces.  Use a zero-offset offset surface
        // instead of the base surface, in order to handle extended surfaces.
        let s = ops.solver_mut();
        s.set_surfaces_to_zero_offset()?;
        let mut p0 = eval_offset(k, s, 0, start_uv0)?; // sP0 = rail1 point
        let mut p1 = eval_offset(k, s, 1, start_uv1)?; // sP1 = rail2 point
        s.reload_surface_offsets()?;

        // `fixes.fillet_torus_center_on_axis` (WR-422): when the section's centre lies on
        // the arc's axis the rail points carry no angle, so turn them into the half-planes
        // through the edge's ends.
        let ivl = edge::interval(b, e)?;
        let unit_axis = ref_frame.z_axis().unitize()?;
        let axis_pnt = ref_frame.origin();
        let center_tol = ops.solver().approx_tol_3d;
        let center_on_axis = k.fixes.fillet_torus_center_on_axis && {
            let off = arc_center - axis_pnt;
            (off - unit_axis * off.dot(unit_axis)).length() < center_tol
        };
        let edge_ends = if center_on_axis {
            let (start, _) = curve_pv(k, &curve, ivl.min())?;
            let (end, _) = curve_pv(k, &curve, ivl.max())?;
            turn_into_half_plane(axis_pnt, unit_axis, start, center_tol, &mut p0);
            turn_into_half_plane(axis_pnt, unit_axis, start, center_tol, &mut p1);
            Some((start, end))
        } else {
            None
        };

        // allocate the arc from the points [centerPoint, sP0, sP1] (a linear section: the
        // line from rail to rail)
        let mut arc = if linear_section {
            BSplineCurve::create_line_segment(3, p0, p1)?
        } else {
            BSplineCurve::create_arc_from_points(arc_center, p0, p1, NurbCircleParam::Quadratic)?
        };

        // Determine if we need to reverse the axis of revolution
        let mut vec = p0 - origin;
        if vec.length() < ANALYTIC_TOL {
            vec = p1 - origin;
        }
        let dir = axis_of_revolution.cross(vec); // Start direction of rail curves

        // get circular to-be-filleted edge->curve point (pos,tang) nearest to arcCenter Point
        // (with the centre on the axis every point is as near: the edge's start, whose
        // half-plane the section was turned into)
        let near_tan = if center_on_axis {
            curve_pv(k, &curve, ivl.min())?.1
        } else {
            let sols = curve.global_point_solve(
                k,
                &ivl,
                SolverOperation::Minimize,
                arc_center,
                ANALYTIC_TOL,
                SolutionRequested::Single,
                &GlobalPointSolveOptions::<crate::topology::ObjectHandle>::default(),
            )?;
            if sols.len() != 1 {
                return Err(sm_err());
            }
            let t_near = sols.get(0).ok_or_else(sm_err)?.start.param(0)?;
            curve_pv(k, &curve, t_near)?.1
        };

        // when to-be-filleted edge tangent nearest sweep curve opposes initial sweep direction
        if near_tan.dot(dir) < 0.0 {
            axis_of_revolution = -axis_of_revolution;
        }

        // Recalculate the origin of torus (see upstream notes).
        let tmp_vec = arc_center - origin;
        origin += axis_of_revolution * tmp_vec.dot(axis_of_revolution);

        // Evaluate points at the ends of the rail curves embedded within the base surfaces.
        let end_uv0 = t_last.uv_pos(SrfIndex::First);
        let end_uv1 = t_last.uv_pos(SrfIndex::Second);
        let s = ops.solver_mut();
        s.set_surfaces_to_zero_offset()?;
        let mut end_p0 = eval_offset(k, s, 0, end_uv0)?; // sEndP0 = rail1 point
        let mut end_p1 = eval_offset(k, s, 1, end_uv1)?; // sEndP1 = rail2 point
        s.reload_surface_offsets()?;
        if let Some((_, end)) = edge_ends {
            turn_into_half_plane(axis_pnt, unit_axis, end, center_tol, &mut end_p0);
            turn_into_half_plane(axis_pnt, unit_axis, end, center_tol, &mut end_p1);
        }

        let mut end_vec = end_p0 - origin;

        // If sEndVec is zero or parallel to sAxisOfRev, use sEndP1.
        let temp_cross = axis_of_revolution.cross(end_vec);
        if temp_cross.length() < ANALYTIC_TOL {
            end_vec = end_p1 - origin;
        }

        // CCWAngleBetween status ignored upstream: dSpanAng stays 0 on a failure
        let mut span_ang = match axis_of_revolution.ccw_angle_between(vec, end_vec) {
            Ok(a) => a,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => 0.0,
        };
        if span_ang < ANG_TOL_RAD {
            span_ang += 2.0 * PI;
        }

        let end_ang = rad_to_deg(span_ang);
        // Not upstream (WR-568, the fork's source fork@985faae): the span is the
        // turn from the start section to the end one; the start angle `IsArc` gives is
        // measured from the arc's own frame (an `SmCircle`'s analytic domain, which need
        // not start at 0), not from the start section, so it is no part of that turn
        let mut angle_span = if k.fixes.fillet_torus_span_from_start_section {
            end_ang
        } else {
            end_ang - start_ang
        };

        // Determine if we need to extend the span even when 'm_bExtendBefore' is FALSE
        if !curve.is_closed(k, &ivl, 0.0)? && !s.extend_after {
            // for both filletSector->edgeuses
            for i in 0..2 {
                let eu_i = s.edgeuse(i)?.ok_or_else(|| invalid("no edge use"))?;
                let vu = eu::vertexuse(b, eu_i)?
                    .ok_or_else(|| invalid("edge use without vertex use"))?;
                let v = vertexuse::vertex(b, vu)?
                    .ok_or_else(|| invalid("vertex use without vertex"))?;
                let orient = eu::orientation(b, eu_i)?;

                if orient == OrientType::Same && !s.extend_before {
                    // for both rail ends
                    for jj in 0..2 {
                        let rail = geom_rail(pseudo, fillet_geom_id, jj)?;
                        let rail_end_v = edge::vertex(pseudo.brep(), rail)?.ok_or_else(null_ptr)?;
                        // skip redundant ends
                        let rail_end = pseudo.fillet_vertex(rail_end_v)?;
                        if rail_end.fillet_vertex_type() != FilletVertexType::RailXEdgeuse
                            || !rail_end.is_processed()
                        {
                            continue;
                        }

                        // when rail endPoint is before start of edge - set m_bExtendAfter
                        let pt = pseudo.vertex_point(rail_end_v)?;
                        let vector = pt - vertex::point(b, v)?;
                        let (_, tangent) = curve_pv(k, &curve, ivl.min())?;
                        let tangent = tangent.unitize()?;
                        let vector = vector.unitize()?;
                        if vector.dot(tangent) < -EFF_ZERO {
                            s.extend_after = true;
                        }
                    }
                }

                // when orientation is opposite
                if orient == OrientType::Opposite {
                    // for both rail endPoints
                    for jj in 0..2 {
                        let rail = geom_rail(pseudo, fillet_geom_id, jj)?;
                        let rail_start_v = edge::vertex(pseudo.brep(), rail)?;
                        let rail_end_v = edge::other_vertex(pseudo.brep(), rail, rail_start_v)?
                            .ok_or_else(null_ptr)?;
                        // skip redundant endPoints
                        let rail_end = pseudo.fillet_vertex(rail_end_v)?;
                        if rail_end.fillet_vertex_type() != FilletVertexType::RailXEdgeuse
                            || !rail_end.is_processed()
                        {
                            continue;
                        }

                        // when rail EndPoint is beyond edge end - set m_bExtendAfter = TRUE
                        let pt = pseudo.vertex_point(rail_end_v)?;
                        let vector = pt - vertex::point(b, v)?;
                        let (_, tangent) = curve_pv(k, &curve, ivl.max())?;
                        let tangent = tangent.unitize()?;
                        let vector = vector.unitize()?;
                        if vector.dot(tangent) > EFF_ZERO {
                            s.extend_after = true;
                        }
                    }
                }
            }
        }

        // extend angle span when requested - limit extent to 360 degrees
        let mut extend_flag = false;
        let extension_angle = (360.0 - angle_span) / 4.0;
        if angle_span < 350.0 {
            if s.extend_after {
                angle_span += extension_angle;
            }
            if s.extend_before {
                angle_span += extension_angle;
                extend_flag = true;
            }
        }
        if angle_span > 360.0 {
            angle_span = 360.0;
        }

        // Move the seam of a full-circle torus of an open edge out of the way [B466].
        let eu0 = s.edgeuse(0)?.ok_or_else(|| invalid("no edge use"))?;
        let e0 = eu::edge(b, eu0)?.ok_or_else(|| invalid("edge use without edge"))?;
        let e0_curve = edge::curve(b, e0)?.ok_or_else(|| invalid("edge without curve"))?;
        let move_seam = if e0_curve.is_closed(k, &edge::interval(b, e0)?, 0.0)? {
            false
        } else {
            angle_span > 360.0 - EFF_ZERO
        };

        if move_seam {
            // Revolve pArc around to the other side of the torus.  [B466]
            let mut cs = Axis2Placement::default();
            cs.init();
            cs.translate(-ref_frame.origin());
            cs.rotate_about_axis(PI, ref_frame.z_axis());
            cs.translate(ref_frame.origin());
            ignore_status(arc.transform(&cs, None))?;
        }

        // create surface of revolution
        let mut torus = SurfOfRevolution::create_canonical(
            k,
            Curve::from(arc),
            origin,
            axis_of_revolution,
            false,
        )?;

        // extend the torus uvDomain as needed
        let mut step_domain = torus.step_uv_domain();
        let mut uv_max = step_domain.max;
        uv_max.x = angle_span;
        step_domain = Extent2d::from_points(step_domain.min, uv_max);
        torus.adjust_step_uv_domain(k, &step_domain)?;

        // Z-Rotate the torus by dAngleExtendBefore
        if extend_flag {
            let mut trans = Axis2Placement::default();
            trans.translate(-origin);
            trans.rotate_about_axis((-extension_angle) * (PI / 180.0), axis_of_revolution);
            trans.translate(origin);
            torus.transform(k, &trans, None)?;
        }
        let torus = Surface::from(torus);

        // Calc rails: the min and max SM_SP_V isoParameter curves
        let uv_domain = torus.natural_uv_domain_with_nurb(k)?;
        let rail_crv0 = torus
            .create_iso_parametric_curve(k, SurfParamType::V, uv_domain.min.y, 0.0, None, false)?
            .curve;
        let crv_ivl0 = rail_crv0.natural_interval();
        let rail_crv1 = torus
            .create_iso_parametric_curve(k, SurfParamType::V, uv_domain.max.y, 0.0, None, false)?
            .curve;
        let crv_ivl1 = rail_crv1.natural_interval();

        Ok(FilletGeomParts {
            surface: torus,
            rails: [rail_crv0, rail_crv1],
            rail_intervals: [Some(crv_ivl0), Some(crv_ivl1)],
            center_line: None,
        })
    }

    /// `CalcSphereFilletGeom(rTsectPnts, pFilletGeom)` (a fork addition, not in the pinned
    /// upstream; opt-in `Kernel::fixes.fillet_corner_ball_patch`, WR-420): builds the
    /// corner ball's patch ([`FilletSolver::sphere_fillet_geom_parts`]) and stores it
    /// into `fillet_geom` (`m_vFilletGeoms[0]` for `None`) of the pseudo B-rep `pseudo`.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::sphere_fillet_geom_parts`] and of
    /// [`FilletSolver::calc_cylinder_fillet_geom`]'s store.
    pub fn calc_sphere_fillet_geom(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &Brep,
        pseudo: &mut FilletBrep,
        tsects: &[TsectPnt],
        fillet_geom: Option<FilletGeomId>,
    ) -> Result<()> {
        let parts = Self::sphere_fillet_geom_parts(ops, k, b, tsects)?;
        store_parts(ops.solver(), pseudo, fillet_geom, parts)
    }

    /// The geometry of [`FilletSolver::calc_sphere_fillet_geom`]: where the fillet's
    /// centre curve collapses to a point (a circular round of an edge between two
    /// cylinders as wide as the round, whose axes meet at `P`), each cylinder's offset
    /// by the radius is its axis, so every ball touching both is centred at `P` and
    /// touches each cylinder along the circle about `P` square to its axis. The patch is
    /// the arc of the first contact circle from the edge's start to its end, turned about
    /// the line through `P` square to both axes onto the second circle (v from rail 0 to
    /// rail 1), and its v-min and v-max iso curves are the rails.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the generator is not circular, a base surface is not a cylinder,
    /// the radius is tiny, changes or differs from a cylinder's, the points' centres are
    /// not one point on both axes, the axes are parallel, the edge ends lie on an axis or
    /// the turn between the contact circles is not within (0, pi); those of
    /// `SetupOffsetValues`, the evaluations and the constructors.
    pub fn sphere_fillet_geom_parts(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &Brep,
        tsects: &[TsectPnt],
    ) -> Result<FilletGeomParts> {
        use crate::surface::surface::SurfaceKind;

        let s = ops.solver();
        if !is_circular_fsg(s)? || tsects.is_empty() {
            return Err(sm_err());
        }

        // both base surfaces cylinders: (origin, z axis, radius)
        let mut cylinders = [(Point3::ZERO, Vec3::ZERO, 0.0); 2];
        for (rail, slot) in cylinders.iter_mut().enumerate() {
            let offset = s
                .surface(rail)?
                .ok_or_else(|| invalid("no offset surface"))?;
            let SurfaceKind::Cone(cone) = offset.surface.base_surface().kind() else {
                return Err(sm_err());
            };
            if !cone.is_cylinder() {
                return Err(sm_err());
            }
            let frame = cone.canonical().origin;
            *slot = (frame.origin(), frame.z_axis(), cone.bot_radius());
        }

        // constant radius, as wide as both cylinders
        let tol = s.approx_tol_3d;
        let (t_first, t_last) = first_last(tsects)?;
        let radius = offset_radius(ops, t_first)?.abs();
        let radius1 = offset_radius(ops, t_last)?.abs();
        if radius < EFF_ZERO
            || (radius1.abs() - radius).abs() > EFF_ZERO
            || cylinders.iter().any(|c| (c.2 - radius).abs() > tol)
        {
            return Err(sm_err());
        }

        let (center, axes) = if k.fixes.fillet_corner_ball_center_from_axes {
            // Not upstream (WR-541, the fork's source fork@bc6ddf9): the ball's
            // centre is where the axes meet. Not the trace's points: at the edge's end where
            // the contact circles meet both offset cylinders collapse onto their axes, so a
            // point of their intersection fixes neither base parameter round its axis, and
            // its solve stops a step short of the axes' crossing.
            let axes = [cylinders[0].1.unitize()?, cylinders[1].1.unitize()?];
            let center = axes_meet(cylinders[0].0, axes[0], cylinders[1].0, axes[1], tol)
                .ok_or_else(sm_err)?;
            (center, axes)
        } else {
            // the ball's centre: every TsectPnt's, where the axes meet
            let center = eval_offset(k, ops.solver_mut(), 0, t_first.uv_pos(SrfIndex::First))?;
            for t in tsects {
                let c0 = eval_offset(k, ops.solver_mut(), 0, t.uv_pos(SrfIndex::First))?;
                let c1 = eval_offset(k, ops.solver_mut(), 1, t.uv_pos(SrfIndex::Second))?;
                if c0.distance(center) > tol || c1.distance(center) > tol {
                    return Err(sm_err());
                }
            }
            let axes = [cylinders[0].1.unitize()?, cylinders[1].1.unitize()?];
            for (c, axis) in cylinders.iter().zip(axes) {
                let off = center - c.0;
                if (off - axis * off.dot(axis)).length() > tol {
                    return Err(sm_err());
                }
            }
            (center, axes)
        };
        let mut turn_axis = axes[0].cross(axes[1]);
        if !k.fixes.fillet_corner_ball_center_from_axes && turn_axis.length() < EFF_ZERO_SQRT {
            return Err(sm_err()); // parallel cylinders: the centres fill a line, not this case
        }
        turn_axis = turn_axis.unitize()?;

        // the edge's ends, and where the ball touches each cylinder in their cross-sections
        let e = filleted_edge(ops.solver(), b)?;
        let curve = edge::curve(b, e)?.ok_or_else(|| SolidError::null_pointer(OP))?;
        let ivl = edge::interval(b, e)?;
        let ends = [
            curve_pv(k, curve, ivl.min())?.0,
            curve_pv(k, curve, ivl.max())?.0,
        ];
        let touch = |axis: Vec3, end: Point3| -> Result<Point3> {
            let off = end - center;
            let rad = off - axis * off.dot(axis);
            if rad.length() < tol {
                return Err(sm_err());
            }
            Ok(center + rad.unitize()? * radius)
        };
        let contact = [
            [touch(axes[0], ends[0])?, touch(axes[0], ends[1])?],
            [touch(axes[1], ends[0])?, touch(axes[1], ends[1])?],
        ];

        // the turn from the first circle to the second, less than half a turn, measured at
        // the edge's end where the contacts lie farther apart (at the other they may meet on
        // the turn's axis, where the edge runs into the corner ball's pole)
        let [[from0, from1], [to0, to1]] = contact;
        let (from, to) = if to1.distance(from1) > to0.distance(from0) {
            (from1 - center, to1 - center)
        } else {
            (from0 - center, to0 - center)
        };
        let mut turn = turn_axis.ccw_angle_between(from, to)?;
        if turn < 0.0 {
            turn_axis = -turn_axis;
            turn = -turn;
        }
        if turn < ANG_TOL_RAD || turn > PI - ANG_TOL_RAD {
            return Err(sm_err());
        }

        // the first circle's arc from the edge's start to its end, turned onto the second
        let arc =
            BSplineCurve::create_arc_from_points(center, from0, from1, NurbCircleParam::Quadratic)?;
        let mut sphere =
            SurfOfRevolution::create_canonical(k, Curve::from(arc), center, turn_axis, true)?;
        let mut step_domain = sphere.step_uv_domain();
        let mut uv_max = step_domain.max;
        uv_max.x = rad_to_deg(turn);
        step_domain = Extent2d::from_points(step_domain.min, uv_max);
        sphere.adjust_step_uv_domain(k, &step_domain)?;
        let sphere = Surface::from(sphere);

        // rails: the patch's v = min and v = max isoparameter curves
        let uv_domain = sphere.natural_uv_domain_with_nurb(k)?;
        let rail_crv0 = sphere
            .create_iso_parametric_curve(k, SurfParamType::V, uv_domain.min.y, 0.0, None, false)?
            .curve;
        let crv_ivl0 = rail_crv0.natural_interval();
        let rail_crv1 = sphere
            .create_iso_parametric_curve(k, SurfParamType::V, uv_domain.max.y, 0.0, None, false)?
            .curve;
        let crv_ivl1 = rail_crv1.natural_interval();

        Ok(FilletGeomParts {
            surface: sphere,
            rails: [rail_crv0, rail_crv1],
            rail_intervals: [Some(crv_ivl0), Some(crv_ivl1)],
            center_line: None,
        })
    }

    /// `PointOnPlaneSolve(crPlaneOrig, crPlaneNormal, crUVDomain1, crUVDomain2, rUV1,
    /// rUV2, rbFoundSolution, rTsectPnt)`: the fillet point on the plane through
    /// `plane_orig` normal to `plane_normal`, solved with `ops`'s equations
    /// ([`FilletSphereSolveEnfo`]; a second try lets the plane adjust). Returns
    /// `rbFoundSolution`; when found, `tsect` gets both UV points and the further
    /// solution values as user doubles.
    ///
    /// # Errors
    ///
    /// Those of `GetJacobianSize`, `LoadInitialValues` (`SM_ERR` for the base solver)
    /// and the solves; [`SolidError::InvalidInput`] without a first offset surface or
    /// for more than [`MAX_USER_DOUBLES`] further values (upstream writes past
    /// `m_adUserDoubles`).
    pub fn point_on_plane_solve(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        brep: Option<&mut Brep>,
        plane_orig: Point3,
        plane_normal: Vec3,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        uv1: Point2,
        uv2: Point2,
        tsect: &mut TsectPnt,
    ) -> Result<bool> {
        // Now Set up Solver
        let jacobian_size = ops.jacobian_size()?;
        let mut intervals = ExtentNd::new(jacobian_size);
        set_interval(
            &mut intervals,
            0,
            Extent1d::from_min_max(uv_domain1.min.x, uv_domain1.max.x),
        );
        set_interval(
            &mut intervals,
            1,
            Extent1d::from_min_max(uv_domain1.min.y, uv_domain1.max.y),
        );
        set_interval(
            &mut intervals,
            2,
            Extent1d::from_min_max(uv_domain2.min.x, uv_domain2.max.x),
        );
        set_interval(
            &mut intervals,
            3,
            Extent1d::from_min_max(uv_domain2.min.y, uv_domain2.max.y),
        );

        let mut periodicities = vec![false, false, false, false];
        let mut guess_t = vec![uv1.x, uv1.y, uv2.x, uv2.y];

        let do_surf2_calcs = false; // Don't do surface2 calculations
        let surf1_index = 0; // UV values of Surface1
        let surf2_index = 2; // UV values of Surface2
        let rail_index = 0; // rail index for pSurface1

        // given current inputs - place a good guess for upcoming solve in sGuessT
        let mut surface1 = take_surface(ops.solver(), 0)?;
        let loaded = ops.load_initial_values(
            k,
            brep,
            do_surf2_calcs,
            rail_index,
            &mut surface1,
            surf1_index,
            &mut intervals,
            &mut periodicities,
            &mut guess_t,
        );
        put_surface(ops.solver_mut(), 0, surface1)?;
        // lSurf2Index is an in/out argument: a solver may move it
        let surf2_index = loaded?.map_or(surf2_index, |l| l.surface2_index);

        // get plane equation for given PlanePt and PlaneNormal
        let plane_d = -plane_orig.dot(plane_normal);
        let approx_tol = ops.solver().approx_tol_3d;

        // allocate NewtonRaphson evaluator - contains equation definitions being solved
        let mut eval = SphereEval(FilletSphereSolveEnfo::new(plane_normal, plane_d, ops), k);
        let mut ls = LocalSolveNd::new(&mut eval, Some(&intervals), Some(&periodicities));

        // set solve parameters
        ls.set_desired_accuracy(EFF_ZERO_SQ);
        ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 8);
        ls.set_maximum_iterations(30);

        // Solve equations with NewtonRaphson iteration
        let mut sol = ls.solve_it(&guess_t, approx_tol)?;

        // Try it again and allow the plane to adjust as convergance happens
        if !sol.found {
            ls.evaluator().0.set_plane_adjusts(true);
            sol = ls.solve_it(&guess_t, approx_tol)?;
        }

        // when a solution point was found
        if sol.found {
            let x = &sol.x;
            *tsect.uv_pos_mut(SrfIndex::First) =
                Point2::new(sol_at(x, surf1_index)?, sol_at(x, surf1_index + 1)?);
            *tsect.uv_pos_mut(SrfIndex::Second) =
                Point2::new(sol_at(x, surf2_index)?, sol_at(x, surf2_index + 1)?);
            for i in 4..x.len() {
                // upstream has no SM_MAX_USER_DOUBLES check here (UB-192)
                let slot = tsect
                    .user_doubles
                    .get_mut(i - 4)
                    .ok_or_else(|| invalid("more solution values than user doubles"))?;
                *slot = sol_at(x, i)?;
            }
        }
        Ok(sol.found)
    }

    /// `RailEdgeuseIntersect(lRailIndex, pSideEU, dGuessEdgeParameter,
    /// rbFoundIntersection, rTsectPnt, rdEdgeuseParameter)`: the fillet point where
    /// rail `rail_index` meets the side edge use's UV trim curve (built when missing),
    /// solved from the guess parameter on that curve with `ops`'s equations
    /// ([`RailUvCurveIntersectEnfo`]). Returns the edge use parameter
    /// (`rdEdgeuseParameter`) when found (`rbFoundIntersection`); `tsect` then gets both
    /// UV points, the parameter (`m_dCurveParameter`), a crossing type when the type was
    /// undefined or unknown, and the further solution values.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without both offset surfaces or for more than [`MAX_USER_DOUBLES`]
    /// further values; those of `GetOrCreateUVTrimCurve`, the evaluations,
    /// `GetJacobianSize`, `LoadInitialValues` and the solve;
    /// [`SolidError::InvalidInput`] for a Jacobian size that overflows when the edge-use
    /// parameter is added, where upstream dereferences `NULL` (no UV trim
    /// curve, no side edge), for a UV trim curve that is not a B-spline, a rail index
    /// above 1, or no second surface index from `LoadInitialValues`.
    pub fn rail_edgeuse_intersect(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        b: &mut Brep,
        rail_index: usize,
        side_eu: EdgeuseId,
        guess_edge_parameter: f64,
        tsect: &mut TsectPnt,
    ) -> Result<Option<f64>> {
        // check state - need both offset surfaces
        if ops.solver_surface(0)?.is_none() || ops.solver_surface(1)?.is_none() {
            return Err(sm_err());
        }
        let rail_srf = srf_of_rail(rail_index)?;
        let other_srf = srf_of_rail(1 - rail_index)?;

        // get original Brep sideEdge->Curve to intersect
        let side_edge = eu::edge(b, side_eu)?;

        // let edgeuse->Surface UV guess = edgeuse->UVTrimCurve(GuessEdgeParameter)
        get_or_create_uv_trim_curve(k, b, side_eu, false, false, true)?.status?;
        let uv_trim = eu::uv_trim_curve(b, side_eu)?
            .0
            .cloned()
            .ok_or_else(|| invalid("no UV trim curve"))?;
        check_uv_bspline_kind(&uv_trim)?;
        let p2d = uv_trim.evaluate_point(k, guess_edge_parameter)?;
        let uv_guess = Point2::new(p2d.x, p2d.y);

        // Now Set up Solver
        let jacobian_size = ops
            .jacobian_size()?
            .checked_add(1)
            .ok_or_else(|| invalid("Jacobian size overflows with edge-use parameter"))?;
        let mut intervals = ExtentNd::new(jacobian_size);
        set_interval(&mut intervals, 0, uv_trim.natural_interval());

        let base = ops
            .solver_surface(rail_index)?
            .map(|surface| surface.base_surface().clone())
            .ok_or_else(|| invalid("no offset surface"))?;
        let mut domain = base.natural_uv_domain_with_nurb(k)?;
        // virtual SetupOffsetExtension: dispatched on the solver's kind
        ops.setup_offset_extension(k, &base, &mut domain)?;
        set_interval(
            &mut intervals,
            1,
            Extent1d::from_min_max(domain.min.x, domain.max.x),
        );
        set_interval(
            &mut intervals,
            2,
            Extent1d::from_min_max(domain.min.y, domain.max.y),
        );

        let mut periodicities = vec![false, false, false];
        let mut guess_t = vec![guess_edge_parameter, uv_guess.x, uv_guess.y];

        let do_surf2_calcs = true; // Do surface2 calculations.
        let surf1_index = 1; // UV values of Surface1

        // given current inputs - place a good guess for upcoming solve in sGuessT
        let mut surface1 = ops
            .solver_surface(rail_index)?
            .cloned()
            .ok_or_else(|| invalid("no offset surface"))?;
        let loaded = ops.load_initial_values(
            k,
            Some(b),
            do_surf2_calcs,
            rail_index,
            &mut surface1,
            surf1_index,
            &mut intervals,
            &mut periodicities,
            &mut guess_t,
        );
        if let Some(selected) = ops.solver_surface_mut(rail_index)? {
            *selected = surface1;
        }
        let loaded = loaded?;

        let side_tol = edge::tolerance(
            b,
            side_edge.ok_or_else(|| invalid("edge use without edge"))?,
        )?;
        let tol = ops.solver().approx_tol_3d + side_tol;

        // specify the rail/curve intersection evaluate function for the NewtonRaphson Solver
        let ivl = uv_trim.natural_interval();
        let mut eval = RailUvEval(
            RailUvCurveIntersectEnfo::new(&uv_trim, ivl, rail_index, ops),
            k,
        );
        let mut ls = LocalSolveNd::new(&mut eval, Some(&intervals), Some(&periodicities));
        ls.set_desired_accuracy(EFF_ZERO_SQ);
        ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 8);
        ls.set_maximum_iterations(30);

        // Solve it
        let sol = ls.solve_it(&guess_t, tol)?;
        if !sol.found {
            return Ok(None);
        }

        // lSurf2Index is set by LoadInitialValues (uninitialised otherwise)
        let surf2_index = loaded
            .ok_or_else(|| invalid("LoadInitialValues set no second surface index"))?
            .surface2_index;
        let x = &sol.x;
        let edgeuse_parameter = sol_at(x, 0)?;

        // set intersection point
        *tsect.uv_pos_mut(rail_srf) =
            Point2::new(sol_at(x, surf1_index)?, sol_at(x, surf1_index + 1)?);
        *tsect.uv_pos_mut(other_srf) =
            Point2::new(sol_at(x, surf2_index)?, sol_at(x, surf2_index + 1)?);
        tsect.curve_parameter = edgeuse_parameter; // Just throw this into here for now.

        // Set this here, otherwise it will be ignored in LoadInitialValues().
        if matches!(
            tsect.point_type,
            IntersectionPointType::Undefined | IntersectionPointType::Unknown
        ) {
            tsect.point_type = IntersectionPointType::Crossing;
        }

        // Load other fillet surface variables being solved for into the fillet point.
        for (cnt, i) in ((surf2_index + 2)..x.len()).enumerate() {
            if cnt == MAX_USER_DOUBLES {
                return Err(sm_err());
            }
            if let Some(slot) = tsect.user_doubles.get_mut(cnt) {
                *slot = sol_at(x, i)?;
            }
        }
        Ok(Some(edgeuse_parameter))
    }

    /// `RailRailIntersect(crUVGuess, lRailIndex, pOtherFillet, lOtherRailIndex,
    /// rbFoundIntersection, rTsectPnt, rOtherTsectPnt)`: where rail `rail_index` of
    /// `ops` meets rail `other_rail_index` of `other` on their common surface, solved
    /// from `uv_guess` with both solvers' equations ([`RailRailIntersectEnfo`]).
    /// Returns `rbFoundIntersection`; when found, both tsect points get their UV points
    /// and further solution values.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a solver lacks an offset surface or for more than
    /// [`MAX_USER_DOUBLES`] further values; those of `GetJacobianSize`,
    /// `SetupOffsetExtension`, `LoadInitialValues` and the solve;
    /// [`SolidError::Unported`] for the temporary radius change
    /// (`SmTemporaryRadiusChange`, unit `blend.fillet_executive_p2`);
    /// [`SolidError::InvalidInput`] when Jacobian sizes overflow while being summed,
    /// sum below 2, the rail index is above 1, or `LoadInitialValues` sets no second
    /// surface index.
    pub fn rail_rail_intersect(
        ops: &mut dyn FilletSolverOps,
        other: &mut dyn FilletSolverOps,
        k: &Kernel,
        mut brep: Option<&mut Brep>,
        uv_guess: Point2,
        rail_index: usize,
        other_rail_index: usize,
        tsect: &mut TsectPnt,
        other_tsect: &mut TsectPnt,
    ) -> Result<bool> {
        // check inputs
        if !has_surface(ops.solver(), 0)?
            || !has_surface(ops.solver(), 1)?
            || !has_surface(other.solver(), 0)?
            || !has_surface(other.solver(), 1)?
        {
            return Err(sm_err());
        }

        // Set up Solver Size
        let jacobian_size = ops.jacobian_size()?;
        let other_jacobian_size = other.jacobian_size()?;
        let jacobian_size = jacobian_size
            .checked_add(other_jacobian_size)
            .ok_or_else(|| invalid("Jacobian sizes sum overflows"))?
            .checked_sub(2)
            .ok_or_else(|| invalid("Jacobian sizes sum below 2"))?;

        // compute and save extended domain size for TargetRail->Surface
        let base = ops
            .solver()
            .surface(rail_index)?
            .map(|slot| slot.surface.base_surface().clone())
            .ok_or_else(|| invalid("no offset surface"))?;
        let mut domain = base.natural_uv_domain_with_nurb(k)?;
        // virtual SetupOffsetExtension: dispatched on the solver's kind
        ops.setup_offset_extension(k, &base, &mut domain)?;
        let mut intervals = ExtentNd::new(jacobian_size);
        set_interval(
            &mut intervals,
            0,
            Extent1d::from_min_max(domain.min.x, domain.max.x),
        );
        set_interval(
            &mut intervals,
            1,
            Extent1d::from_min_max(domain.min.y, domain.max.y),
        );

        // mark periodicities for TargetRail->Surface
        let mut periodicities = vec![false, false];

        // Set Solver GuessPoint
        let mut guess_t = vec![uv_guess.x, uv_guess.y];

        // Temporarily replace the variable-radius functions with a constant. [Reg Iter:306]
        // `SmTemporaryRadiusChange::SetUp` does nothing unless the solver is an
        // `SmVariableRadiusFS` (`SM_CAST_PTR`), first for this solver, then the other.
        let other_side_eu = other.solver().edgeuse(other_rail_index)?;
        let this_side_eu = ops.solver().edgeuse(rail_index)?;
        let temp_this = match (ops.variable_radius_mut(), other_side_eu) {
            (Some(vrfs), Some(side)) => {
                let target = brep.as_deref().ok_or_else(|| {
                    invalid("variable-radius RailRailIntersect requires target B-rep")
                })?;
                crate::blend::fillet_executive::p2::DirectTemporaryRadiusChange::new(
                    Some(vrfs),
                    target,
                    rail_index,
                    side,
                )?
            }
            _ => crate::blend::fillet_executive::p2::DirectTemporaryRadiusChange::default(),
        };
        let temp_other = match (other.variable_radius_mut(), this_side_eu) {
            (Some(vrfs), Some(side)) => {
                let Some(target) = brep.as_deref() else {
                    temp_this.revert(ops.variable_radius_mut());
                    return Err(invalid(
                        "variable-radius RailRailIntersect requires target B-rep",
                    ));
                };
                match crate::blend::fillet_executive::p2::DirectTemporaryRadiusChange::new(
                    Some(vrfs),
                    target,
                    other_rail_index,
                    side,
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        temp_this.revert(ops.variable_radius_mut());
                        return Err(e);
                    }
                }
            }
            _ => crate::blend::fillet_executive::p2::DirectTemporaryRadiusChange::default(),
        };

        let result = (|| -> Result<bool> {
            // Append a good lRailIndex->OtherSurface UVguessPoint into sGuessT
            let do_surf2_calcs = true;
            let surf1_index = 0;
            let mut surface1 = take_surface(ops.solver(), rail_index)?;
            let loaded = ops.load_initial_values(
                k,
                brep.as_deref_mut(),
                do_surf2_calcs,
                rail_index,
                &mut surface1,
                surf1_index,
                &mut intervals,
                &mut periodicities,
                &mut guess_t,
            );
            put_surface(ops.solver_mut(), rail_index, surface1)?;
            let loaded = loaded?;

            // Append a good lOtherRailIndex->OtherSurface UVguessPoint into sGuessT
            let mut other_surface1 = take_surface(other.solver(), other_rail_index)?;
            let other_loaded = other.load_initial_values(
                k,
                brep.as_deref_mut(),
                do_surf2_calcs,
                other_rail_index,
                &mut other_surface1,
                surf1_index,
                &mut intervals,
                &mut periodicities,
                &mut guess_t,
            );
            put_surface(other.solver_mut(), other_rail_index, other_surface1)?;
            let other_loaded = other_loaded?;

            let tol = ops.solver().approx_tol_3d + other.solver().approx_tol_3d;

            // Set Solver Manager
            let mut eval = RailRailEval(
                RailRailIntersectEnfo::new(rail_index, ops, other_rail_index, other),
                k,
            );
            let mut ls = LocalSolveNd::new(&mut eval, Some(&intervals), Some(&periodicities));
            ls.set_desired_accuracy(EFF_ZERO_SQ);
            ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 8);
            ls.set_maximum_iterations(30);

            // Solve it
            let sol = ls.solve_it(&guess_t, tol)?;
            if !sol.found {
                return Ok(false);
            }

            let surf2_index = loaded
                .ok_or_else(|| invalid("LoadInitialValues set no second surface index"))?
                .surface2_index;
            let other_surf2_index = other_loaded
                .ok_or_else(|| invalid("LoadInitialValues set no second surface index"))?
                .surface2_index;
            let x = &sol.x;
            let rail_srf = srf_of_rail(rail_index)?;
            let rail_other_srf = srf_of_rail(1 - rail_index)?;
            *tsect.uv_pos_mut(rail_srf) =
                Point2::new(sol_at(x, surf1_index)?, sol_at(x, surf1_index + 1)?);
            *tsect.uv_pos_mut(rail_other_srf) =
                Point2::new(sol_at(x, surf2_index)?, sol_at(x, surf2_index + 1)?);

            // Load other fillet surface variables being solved for into the fillet point.
            for (cnt, i) in ((surf2_index + 2)..other_surf2_index).enumerate() {
                if cnt == MAX_USER_DOUBLES {
                    return Err(sm_err());
                }
                if let Some(slot) = tsect.user_doubles.get_mut(cnt) {
                    *slot = sol_at(x, i)?;
                }
            }

            let o_srf = srf_of_rail(other_rail_index)?;
            let o_other_srf = srf_of_rail(1 - other_rail_index)?;
            *other_tsect.uv_pos_mut(o_srf) =
                Point2::new(sol_at(x, surf1_index)?, sol_at(x, surf1_index + 1)?);
            *other_tsect.uv_pos_mut(o_other_srf) = Point2::new(
                sol_at(x, other_surf2_index)?,
                sol_at(x, other_surf2_index + 1)?,
            );

            // Load values into rOtherTsectPnt.
            for (cnt, i) in ((other_surf2_index + 2)..x.len()).enumerate() {
                if cnt == MAX_USER_DOUBLES {
                    return Err(sm_err());
                }
                if let Some(slot) = other_tsect.user_doubles.get_mut(cnt) {
                    *slot = sol_at(x, i)?;
                }
            }
            Ok(true)
        })();

        temp_other.revert(other.variable_radius_mut());
        temp_this.revert(ops.variable_radius_mut());
        result
    }

    /// `CreateCliffRollOverSideFG(pRailOnCliff, pSideEdge, pSideFace, bTrimSideFaces,
    /// dExtensionDist, rpSideFG)`: the side fillet geometry of a cliff roll-over. When
    /// trimming, the rail's original face becomes the side face; otherwise the side
    /// face's surface is extended by `extension_dist` (copied as an analytic surface
    /// where possible) and the rail loses its original face. At a fillet corner the
    /// corner vertex moves to where the cross-section edge meets that surface and the
    /// cross section is trimmed to it (and, without trimming and without a split-face
    /// edge, a split-face edge is made from the corner to the side edge). Without
    /// trimming a new cliff-side-patch geometry is made, with the rail and a new rail
    /// on the side edge; it is returned (`rpSideFG`), `None` when trimming.
    ///
    /// `b` is the B-rep being filleted (the side edge and face); `pseudo` is the
    /// executive's pseudo B-rep (`GetPseudoBrep()`) holding `rail_on_cliff`, the fillet
    /// vertices and the fillet geometries; `solver_id` is `this` in the caller's solver
    /// table (`SmFilletGeom::GetFilletSolver`). The new geometry is added to `pseudo`
    /// and to this solver's geometries (as the [`FilletGeomId`] of the same index).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without an executive or where upstream dereferences
    /// `NULL`; `SM_ERR_NULL_POINTER` (`NER`) for a side face without surface (when
    /// extending), no cross-section edge or curve, or a side edge without curve;
    /// `SM_ERR` when the cross section misses the side surface or the corner point does
    /// not drop onto the side edge; those of the extension, copy, intersections,
    /// evaluations, drops, [`FilletBrep::make_fillet_edge`],
    /// [`FilletBrep::new_fillet_edgeuse`] and [`FilletBrep::calc_edge_on_edge`].
    pub fn create_cliff_roll_over_side_fg(
        &mut self,
        k: &Kernel,
        b: &Brep,
        pseudo: &mut FilletBrep,
        solver_id: FilletSolverId,
        rail_on_cliff: EdgeId,
        side_edge: EdgeId,
        side_face: FaceId,
        trim_side_faces: bool,
        extension_dist: f64,
    ) -> Result<Option<FilletGeomId>> {
        const OP_C: &str = "blend::fillet_solver::p2::create_cliff_roll_over_side_fg";
        let nul = || SolidError::null_pointer(OP_C);

        // locals
        let rail_start_v = edge::vertex(pseudo.brep(), rail_on_cliff)?;
        let rail_end_v = edge::other_vertex(pseudo.brep(), rail_on_cliff, rail_start_v)?;
        // this->GetFilletExecutive()->GetPseudoBrep() is `pseudo`
        if self.executive.is_none() {
            return Err(invalid("no fillet executive"));
        }
        let start_v = rail_start_v.ok_or_else(|| invalid("rail without start vertex"))?;
        let adj_corner_vert = if pseudo.fillet_vertex(start_v)?.fillet_corner().is_some() {
            Some(start_v)
        } else {
            let end_v = rail_end_v.ok_or_else(|| invalid("rail without end vertex"))?;
            pseudo.fillet_vertex(end_v)?.fillet_corner().map(|_| end_v)
        };

        // Pick the side surface to extend: the side face's surface or its copy
        // (SM_FILLETSURF_TYPE is SmSurface: the cast keeps any surface)
        let adj_surface = face_p2::surface(b, side_face)?.cloned();
        let side_surface = if trim_side_faces {
            pseudo
                .fillet_edge_mut(rail_on_cliff)?
                .set_original_face(Some(side_face));
            adj_surface
        } else {
            let adj = adj_surface.ok_or_else(nul)?;
            let new_surface = adj.create_extended_surface(k, extension_dist, ContinuityType::G1)?;
            // Copy pNewSurface, when possible as an analytic surface
            let copy = new_surface.copy_and_add_analytics(k)?;
            pseudo
                .fillet_edge_mut(rail_on_cliff)?
                .set_original_face(None);
            Some(copy)
        };

        // At a corner: move the corner vertex to the cross section / side surface
        // intersection and trim the cross section to run from there.
        if let Some(corner_v) = adj_corner_vert {
            let corner_edges = vertex::edges(pseudo.brep(), corner_v, None)?;

            // the last CROSS_SECTION and SPLIT_FACE fillet edges of the list
            let mut split_edge: Option<EdgeId> = None;
            let mut cross_section: Option<EdgeId> = None;
            for fe in &corner_edges {
                match pseudo.fillet_edge(*fe)?.fillet_edge_type() {
                    FilletEdgeType::CrossSection => cross_section = Some(*fe),
                    FilletEdgeType::SplitFace => split_edge = Some(*fe),
                    _ => {}
                }
            }
            let cross_section = cross_section.ok_or_else(nul)?;

            // CrossSection curve locals
            let mut cs_curve = edge::curve(pseudo.brep(), cross_section)?
                .cloned()
                .ok_or_else(nul)?;
            let mut trim_ivl = cs_curve.natural_interval();

            // Intersect the cross section with the extended side surface
            let side = side_surface
                .as_ref()
                .ok_or_else(|| invalid("no side surface"))?;
            let solutions = side.global_curve_intersect(
                k,
                [CliffHandle::Curve, CliffHandle::Surface],
                &side.natural_uv_domain_with_nurb(k)?,
                &cs_curve,
                &trim_ivl,
                self.approx_tol_3d,
            )?;
            let first = solutions.get(0).ok_or_else(sm_err)?;
            let param1 = first.start.param(0)?;
            let corner_pnt = cs_curve.evaluate_point(k, param1)?;

            // Update the corner vertex to the cross section / side face point
            // (SmVertex::SetPoint: the SM_NO_CHANGE_GEOMETRY resets, as geom p1/p4)
            set_vertex_point(pseudo.brep_mut(), corner_v, corner_pnt)?;

            // Drop the cross section's other vertex onto its curve
            let other_v = edge::other_vertex(pseudo.brep(), cross_section, Some(corner_v))?
                .ok_or_else(|| invalid("cross section without other vertex"))?;
            let dropped = cs_curve.drop_point(
                k,
                &trim_ivl,
                vertex::point(pseudo.brep(), other_v)?,
                None,
                self.approx_tol_3d,
                None,
                SolverOperation::Minimize,
            )?;
            if dropped.success {
                let param2 = dropped.parameter;
                // order the desired cross section interval
                trim_ivl = if param1 > param2 {
                    Extent1d::from_min_max(param2, param1)
                } else {
                    Extent1d::from_min_max(param1, param2)
                };

                // The cross section's curves belong to its edge: their trims read the
                // edge's tolerance as zone tolerance (as `sm_TrimRail`, p1 notes).
                let tk = edge_zone_kernel(k, edge::tolerance(pseudo.brep(), cross_section)?);
                // Set the cross section curve interval (status ignored upstream)
                ignore_status(cs_curve.trim(&tk, &mut trim_ivl, TrimMode::Notify))?;
                if let Some(c) = pseudo.brep_mut().edge_mut(cross_section)?.curve.as_mut() {
                    *c = cs_curve.clone();
                }

                // Set the cross section's UV trim curve intervals
                let prim = eu::primary_edgeuse(pseudo.brep(), cross_section)?
                    .ok_or_else(|| invalid("cross section without edge use"))?;
                let mate = eu::mate(pseudo.brep(), prim)?
                    .ok_or_else(|| invalid("edge use without mate"))?;
                for e_u in [prim, mate] {
                    if let Some(uv) = pseudo.brep_mut().edgeuse_mut(e_u)?.uv_trim_curve.as_mut() {
                        ignore_status(uv.trim(&tk, &mut trim_ivl, TrimMode::Notify))?;
                    }
                }
            }
            // else: SM_ASSERT_MSG only (the cross section interval stays)

            // Without a split-face edge and not trimming: make one
            if !trim_side_faces && split_edge.is_none() {
                // Create a split fillet edge between the corner point and its
                // projection onto the side curve
                // new (pPseudoBrep) SmFilletVertex(): SmVertex() default point (0, 0, 0),
                // no corner (`SmFilletVertex::SmFilletVertex` is `FilletVertex::new`)
                let end_vert = pseudo.brep_mut().add_vertex(Point3::ZERO)?;
                pseudo.insert_fillet_vertex(end_vert, FilletVertex::new(k, None));
                let new_split = pseudo.make_fillet_edge(Some(end_vert), Some(corner_v), None)?;
                pseudo
                    .fillet_edge_mut(new_split)?
                    .set_fillet_edge_type(FilletEdgeType::SplitFace);

                // drop the corner point to the side curve
                let side_curve = edge::curve(b, side_edge)?.ok_or_else(nul)?;
                let sols = side_curve.global_point_solve(
                    k,
                    &edge::interval(b, side_edge)?,
                    SolverOperation::Minimize,
                    corner_pnt,
                    EFF_ZERO_SQRT,
                    SolutionRequested::Single,
                    &GlobalPointSolveOptions::<crate::topology::ObjectHandle>::default(),
                )?;
                let sol = sols.get(0).ok_or_else(sm_err)?;
                let (end_pnt, end_tan) = curve_pv(k, side_curve, sol.start.param(0)?)?;
                set_vertex_point(pseudo.brep_mut(), end_vert, end_pnt)?;
                pseudo
                    .fillet_vertex_mut(end_vert)?
                    .set_status(FilStatus::Processed);

                // the plane through the end point perpendicular to the line
                // [corner, end] in the side curve's tangent plane
                let dir = corner_pnt - end_pnt;
                let plane_normal = end_tan.cross(dir).cross(dir).unitize()?;
                let mut plane = Plane::from_normal(end_pnt, plane_normal);
                plane.make_nurb()?;

                // the side surface / plane intersection curve (its UV curves are
                // deleted unused); the vertices' points are sEndPnt and the corner
                // point they were just set to
                let mut start_dir = dir;
                let plane_surface = Surface::from(plane);
                let curves = srf_srf_intersection(
                    k,
                    Some(end_pnt),
                    Some(corner_pnt),
                    Some(&mut start_dir),
                    None,
                    side,
                    &plane_surface,
                    self.approx_tol_3d,
                    30.0 * PI / 180.0,
                    SrfSrfOptions::default(),
                )?;
                // p3DCurve->SetOwner dereferences the curve
                let curve_3d = curves.curve_3d.ok_or_else(nul)?;
                // SetCurve(p3DCurve, TRUE) (the edge owns the curve: SetOwner(pSplitEdge));
                // the UV curves are deleted unused
                set_edge_curve(pseudo.brep_mut(), new_split, Some(curve_3d))?;
            }
        }

        // Trimming: done after setting the original face and the corner
        if trim_side_faces {
            return Ok(None);
        }

        // Create new fillet geom added to FilletSolver->m_vFilletGeoms array
        // (`SmFilletGeom::SmFilletGeom` is `FilletGeom::new`)
        let side_geom =
            FilletGeom::new(k, Some((solver_id, &*self)), FilletGeomType::CliffSidePatch)?;
        let side_fg = pseudo.add_geom(side_geom);
        self.fillet_geoms.push(side_fg);

        let mut rail_eu = eu::primary_edgeuse(pseudo.brep(), rail_on_cliff)?
            .ok_or_else(|| invalid("rail without edge use"))?;
        if eu_vertex(pseudo.brep(), rail_eu)? != rail_start_v {
            rail_eu = eu::mate(pseudo.brep(), rail_eu)?
                .ok_or_else(|| invalid("edge use without mate"))?;
        }
        pseudo.set_edgeuse_fillet_geom(rail_eu, Some(side_fg))?;

        // Attach the surface
        pseudo.geom_mut(side_fg)?.set_fillet_surface(side_surface);

        // Make some new topology objects (edge, edge uses, vertex uses). The new rail
        // runs between the vertices its vertex uses will go into: the rail ends, or the
        // other ends of their 'support' edges (found here, attached below).
        pseudo.geom_mut(side_fg)?.set_rail(0, Some(rail_on_cliff)); // Use existing rail as rail #1
        let rail_start_v = rail_start_v.ok_or_else(|| invalid("rail without vertex"))?;
        let rail_end_v = rail_end_v.ok_or_else(|| invalid("rail without vertex"))?;
        let start_support = support_edge_at(pseudo, rail_start_v)?;
        let end_support = support_edge_at(pseudo, rail_end_v)?;
        let new_rail_start_v = start_support.map_or(rail_start_v, |(_, other)| other);
        let new_rail_end_v = end_support.map_or(rail_end_v, |(_, other)| other);
        // new (pPseudoBrep) SmFilletEdge(pPseudoBrep)
        let new_rail = pseudo.new_fillet_edge(new_rail_start_v, new_rail_end_v, None)?;
        pseudo.geom_mut(side_fg)?.set_rail(1, Some(new_rail));
        pseudo
            .fillet_edge_mut(new_rail)?
            .set_fillet_edge_type(FilletEdgeType::Rail);

        // new (pPseudoBrep) SmFilletEdgeuse(pPseudoBrep), twice
        let new_eu1 = pseudo.new_fillet_edgeuse()?;
        let new_eu2 = pseudo.new_fillet_edgeuse()?;
        {
            // SmEdge::PreInsert(pNewEU1) / PostInsert(pNewEU2) on the arena record
            // (as blend.fillet_geom_p5's edge_insert)
            let brep = pseudo.brep_mut();
            brep.edge(new_rail)?;
            brep.edgeuse_mut(new_eu1)?.edge = Some(new_rail);
            brep.edge_mut(new_rail)?.edgeuses.insert(0, new_eu1);
            brep.edgeuse_mut(new_eu2)?.edge = Some(new_rail);
            brep.edge_mut(new_rail)?.edgeuses.push(new_eu2);
        }
        // new (pPseudoBrep) SmFilletVertexuse(), twice
        let mut new_vus = [None, None];
        for slot in &mut new_vus {
            let vu = pseudo
                .brep_mut()
                .add_vertexuse_record(Vertexuse::default())?;
            pseudo.insert_fillet_vertexuse(vu, FilletVertexuse::new());
            *slot = Some(vu);
        }
        let [Some(new_vu1), Some(new_vu2)] = new_vus else {
            return Err(invalid("new vertex uses missing"));
        };
        for (vu, e_u) in [(new_vu1, new_eu1), (new_vu2, new_eu2)] {
            pseudo.set_vertexuse_property(vu, e_u)?;
        }
        for (e_u, vu) in [(new_eu1, new_vu1), (new_eu2, new_vu2)] {
            pseudo.set_edgeuse_fillet_vertexuse(e_u, Some(vu))?;
        }

        // Put each new vertex use into its rail end vertex, or into the other end of
        // a 'support' edge at that vertex (attaching the new geometry to it)
        for (support, new_rail_v, new_vu) in [
            (start_support, new_rail_start_v, new_vu1),
            (end_support, new_rail_end_v, new_vu2),
        ] {
            if let Some((fe, _)) = support {
                // Attach the new fillet geometry to the support edge
                let mut feu = eu::primary_edgeuse(pseudo.brep(), fe)?
                    .ok_or_else(|| invalid("support edge without edge use"))?;
                if pseudo.fillet_edgeuse(feu)?.fillet_geom().is_some() {
                    feu = eu::mate(pseudo.brep(), feu)?
                        .ok_or_else(|| invalid("edge use without mate"))?;
                }
                pseudo.geom_mut(side_fg)?.add_side_fillet_edgeuse(feu);
                pseudo.set_edgeuse_fillet_geom(feu, Some(side_fg))?;
            }
            // SmVertex::PostInsert(pNewVU): the vertex use joins the vertex's list
            let brep = pseudo.brep_mut();
            brep.vertexuse_mut(new_vu)?.vertex = Some(new_rail_v);
            brep.vertex_mut(new_rail_v)?.vertexuses.push(new_vu);
        }

        // The new rail runs on the side edge
        let new_rail_edge = pseudo.fillet_edge_mut(new_rail)?;
        new_rail_edge.set_original_edge(Some(side_edge));
        new_rail_edge.set_original_face(Some(side_face));
        pseudo.calc_edge_on_edge(k, new_rail, b)?;
        Ok(Some(side_fg))
    }

    /// `RecordFilletError(eType, rapTSPs, cComment)`: passes the error, with the
    /// filleted edge, the fillet surfaces, centre lines and rails built so far and the
    /// first tsect point's curve position, to the executive (`NoteFilletError`); a
    /// caller holding no executive [`ErrorInfo`] notes into a scratch record.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown `error_type` or where upstream
    /// dereferences `NULL` (no edge use, geometry or rail, or no executive).
    pub fn record_fillet_error(
        &self,
        b: &Brep,
        pseudo: &FilletBrep,
        error_type: i64,
        tsects: &[TsectPnt],
        comment: &str,
    ) -> Result<()> {
        let code = fillet_error_type(error_type)?;
        self.record_fillet_error_in(b, pseudo, code, tsects, Some(comment), None)
    }

    /// `RecordFilletError(eType, rapTSPs, cComment)` into the executive's
    /// [`ErrorInfo`] (`None` when the caller has no executive error record: the note
    /// goes to a scratch record).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (no edge use,
    /// geometry or rail, or no executive).
    pub fn record_fillet_error_in(
        &self,
        b: &Brep,
        pseudo: &FilletBrep,
        error_type: FilletErrorType,
        tsects: &[TsectPnt],
        comment: Option<&str>,
        error_info: Option<&mut ErrorInfo<SolverErrorObject>>,
    ) -> Result<()> {
        let filleted = filleted_edge_opt(self, b)?;

        // Collect all existing fillet surfaces and rail curves (upstream adds rail 0
        // where it tests rail 1, WR-190)
        let mut fil_surfs: Vec<FilletGeomId> = Vec::new();
        let mut fil_curvs: Vec<FilletGeomId> = Vec::new();
        let mut fil_edges: Vec<EdgeId> = Vec::new();
        for g in self.fillet_geoms() {
            let this_geom = pseudo.geom(g)?;
            if this_geom.has_fillet_surface() && !fil_surfs.contains(&g) {
                fil_surfs.push(g);
            }
            if this_geom.center_line_curve().is_some() && !fil_curvs.contains(&g) {
                fil_curvs.push(g);
            }
            for rail_index in 0..2 {
                let rail = geom_rail(pseudo, g, rail_index)?;
                if edge::curve(pseudo.brep(), rail)?.is_some() {
                    // WR-190: rail 1's test adds rail 0
                    let added = geom_rail(pseudo, g, 0)?;
                    if !fil_edges.contains(&added) {
                        fil_edges.push(added);
                    }
                }
            }
        }

        // Just look at one TsectPnt position for now
        let mut tsp_pos = Vec3::UNDEF;
        if let Some(t) = tsects.first() {
            if t.crv_pos().is_initialized() {
                tsp_pos = t.crv_pos();
            }
        }

        // pass the call along to FilletExecutive
        if self.executive.is_none() {
            return Err(invalid("no fillet executive"));
        }
        // m_pExecutive->NoteFilletError: a caller without an executive record (a
        // standalone entry point) notes into a scratch record it does not keep.
        let mut scratch = ErrorInfo::default();
        let error_info = match error_info {
            Some(error_info) => error_info,
            None => &mut scratch,
        };
        let surfaces: Vec<Option<SolverErrorObject>> = fil_surfs
            .into_iter()
            .map(|g| Some(SolverErrorObject::FilletSurface(g)))
            .collect();
        let curves: Vec<Option<SolverErrorObject>> = fil_curvs
            .into_iter()
            .map(|g| Some(SolverErrorObject::CenterLineCurve(g)))
            .collect();
        let edges: Vec<Option<SolverErrorObject>> = fil_edges
            .into_iter()
            .map(|e| Some(SolverErrorObject::FilletEdge(e)))
            .collect();
        error_info.note_intersection_error(
            error_type,
            self.status,
            filleted.map(SolverErrorObject::FilletedEdge),
            &surfaces,
            &curves,
            &edges,
            tsp_pos,
            comment,
        );
        Ok(())
    }
}

/// A kernel for trimming curves that belong to the pseudo-B-rep edge of tolerance `tol`:
/// `SmBSplineCurve::Trim` reads `SmTol::GetZoneTol3d(this)`, the edge's tolerance for a
/// curve an edge owns (as `sm_TrimRail`, `blend.fillet_solver_p1`), where the owner-less
/// `Curve::trim` reads the kernel's. The caller's random state carries over; its
/// `context` (boxed trait objects, not clonable) is not read on the trim path.
fn edge_zone_kernel(k: &Kernel, tol: f64) -> Kernel {
    let mut tk = Kernel::new(&crate::kernel::KernelOptions {
        tolerances: crate::kernel::Tolerances {
            zone_tol_3d: tol,
            ..k.tolerances
        },
        ..crate::kernel::KernelOptions::default()
    });
    tk.rng = k.rng.clone();
    tk.fixes.bspline_trim_preserves_uv_spans = k.fixes.bspline_trim_preserves_uv_spans;
    tk
}

/// The first 'support' edge (`SM_FE_ON_EXTEND_EDGE` or `SM_FE_SPLIT_FACE`) at fillet
/// vertex `v` in `GetEdges` order and its other vertex.
fn support_edge_at(pseudo: &FilletBrep, v: VertexId) -> Result<Option<(EdgeId, VertexId)>> {
    for fe in vertex::edges(pseudo.brep(), v, None)? {
        let t = pseudo.fillet_edge(fe)?.fillet_edge_type();
        if t == FilletEdgeType::OnExtendEdge || t == FilletEdgeType::SplitFace {
            let other = edge::other_vertex(pseudo.brep(), fe, Some(v))?
                .ok_or_else(|| invalid("support edge without other vertex"))?;
            return Ok(Some((fe, other)));
        }
    }
    Ok(None)
}

/// `GetEdgeuse(0)->GetEdge()` (a missing edge use is a `NULL` dereference upstream).
fn filleted_edge_opt(s: &FilletSolver, b: &Brep) -> Result<Option<EdgeId>> {
    let eu0 = s.edgeuse(0)?.ok_or_else(|| invalid("no edge use"))?;
    eu::edge(b, eu0)
}

/// `(*pCC)[i]` (an index past the classification is out of bounds upstream).
fn interval_at<O: ClassObject>(
    cc: &CurveClassification<O>,
    i: usize,
) -> Result<&crate::intersect::curve_class::p1::CurveInterval<O>> {
    cc.intervals()
        .get(i)
        .ok_or_else(|| invalid("classification index out of range"))
}

/// `(SmEdge*)rPC.GetObject()` through the caller's handle map.
fn edge_object<O: ClassObject>(
    pc: &crate::intersect::point_class::PointClassification<O>,
    edge_of: &dyn Fn(O) -> Option<EdgeId>,
) -> Result<EdgeId> {
    pc.object()
        .and_then(edge_of)
        .ok_or_else(|| invalid("classification object is not an edge"))
}

/// `pEU->GetVertexuse()->GetVertex()`.
fn eu_vertex(b: &Brep, e: EdgeuseId) -> Result<Option<VertexId>> {
    let vu = eu::vertexuse(b, e)?.ok_or_else(|| invalid("edge use without vertex use"))?;
    vertexuse::vertex(b, vu)
}

/// `sm_FindRolloverSideEdge(pCC, rlCurrIndx, pRail, pOrigRailCurve, pOrigRailUVCurve,
/// pOrigFillet, bConvexFillet, rpSideEdge, rbIsTangentialSideEdge, rbTrimSideFaces,
/// rbOppositeNormals, rpRolloverCornerEU, rpRolloverCornerV, dTangencyTolRad,
/// iDebugLevel)`: walks the classified rail intervals from `io.curr_index` to the side
/// edge the rail runs into, telling a crash (trim the side faces) from a cliff (roll
/// over, possibly at a corner). `io` holds the reference arguments: upstream writes
/// only some of them on each path, the rest keep the caller's values. `pseudo` is the
/// pseudo B-rep holding the rail (its original face) and its fillet vertices (their
/// status); `b` is the B-rep being filleted (the side edges, their edge uses and
/// faces; the tangency test may build UV trim curves on it); `edge_of` maps a
/// classification object to its edge (`(SmEdge*)GetObject()`). `pOrigRailCurve` is
/// unused upstream.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` (`NER`) without original face; `SM_ERR` for an end class that
/// is not an edge before the last interval, or a start class that is not an edge on
/// it; those of the evaluations, the angle and the tangency and convexity tests;
/// [`SolidError::NotFound`] for a rail or vertex that is not a fillet edge or vertex of
/// `pseudo`; [`SolidError::InvalidInput`] where upstream dereferences `NULL` or indexes
/// past the classification.
pub fn find_rollover_side_edge<O: ClassObject>(
    k: &Kernel,
    pseudo: &FilletBrep,
    b: &mut Brep,
    cc: &CurveClassification<O>,
    rail: EdgeId,
    orig_rail_uv_curve: &Curve,
    orig_fillet: &Surface,
    convex_fillet: bool,
    tangency_tol_rad: f64,
    edge_of: &dyn Fn(O) -> Option<EdgeId>,
    io: &mut RolloverSideEdge,
) -> Result<()> {
    const OP_R: &str = "blend::fillet_solver::p2::find_rollover_side_edge";
    let total_intervals = cc.intervals().len();
    let start_v = edge::vertex(pseudo.brep(), rail)?;
    let end_v = edge::other_vertex(pseudo.brep(), rail, start_v)?;
    let orig_face = pseudo
        .fillet_edge(rail)?
        .original_face()
        .ok_or_else(|| SolidError::null_pointer(OP_R))?;
    // pStartV->IsProcessed(), pEndV->IsProcessed() (a NULL vertex is dereferenced)
    let is_processed = |v: Option<VertexId>| -> Result<bool> {
        let v = v.ok_or_else(|| invalid("rail without vertex"))?;
        Ok(pseudo.fillet_vertex(v)?.is_processed())
    };

    io.trim_side_faces = false;
    // Every pass that does not return advances `curr_index`, so the walk ends within
    // the classification's length (module notes).
    loop {
        let ivl = interval_at(cc, io.curr_index)?;

        if io.curr_index == 0
            && !is_processed(start_v)?
            && ivl.start.point_class() == PointClassType::Unknown
        {
            io.curr_index += 1; // Skip this interval, move on to next.
            continue;
        }

        let start_pc = &ivl.start;
        let end_pc = &ivl.end;
        if io.curr_index + 1 < total_intervals {
            if ivl.end.point_class() != PointClassType::Edge {
                return Err(sm_err()); // Unknown case
            }
            let side_edge = edge_object(&ivl.end, edge_of)?;

            // Check whether the side edge we hit is a tangent edge, and whether it's
            // convex.
            let (tangent, opposite) =
                test_manifold_tangent_rollover(k, b, side_edge, tangency_tol_rad)?;
            io.is_tangential_side_edge = tangent;
            io.opposite_normals = opposite;
            let convex_side_edge =
                test_side_edge_convexity(k, b, side_edge, io.is_tangential_side_edge)?;

            // Same convexity: the fillet crashes into the adjoining face (trim it);
            // otherwise it hangs over a cliff.
            if convex_fillet == convex_side_edge {
                // Trimming process
                io.trim_side_faces = true;
                io.curr_index += 1;
                continue;
            }

            if start_pc.point_class() == PointClassType::Edge
                && end_pc.point_class() == PointClassType::Edge
                && ivl.mid.point_class() == PointClassType::Unknown
            {
                let e1 = edge_object(start_pc, edge_of)?;
                let e2 = edge_object(end_pc, edge_of)?;
                let mut eu_on_orig_face =
                    eu::primary_edgeuse(b, e1)?.ok_or_else(|| invalid("edge without edge use"))?;
                if eu::face(b, eu_on_orig_face)? != Some(orig_face) {
                    eu_on_orig_face = eu::radial(b, eu_on_orig_face)?
                        .ok_or_else(|| invalid("edge use without radial"))?;
                }

                let start_param = start_pc.t_param();
                let end_param = end_pc.t_param();
                let e1_curve = edge::curve(b, e1)?.ok_or_else(|| invalid("edge without curve"))?;
                let (_, tangent_vec) = curve_pv(k, e1_curve, start_param)?;
                // Compute 'binormal' of rail curve
                let rail_param = ivl.interval.min();
                let pnt = orig_rail_uv_curve.evaluate_point(k, rail_param)?;
                let uv = Point2::new(pnt.x, pnt.y);
                let flags = crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags {
                    only_upper_half: true,
                    ..Default::default()
                };
                let pvv = orig_fillet.evaluate(k, uv, 1, 1, flags)?;
                let mut bin_vec = pvv
                    .first()
                    .and_then(|row| row.get(1))
                    .copied()
                    .ok_or_else(|| invalid("short surface evaluation"))?;
                let center_uv = orig_fillet
                    .natural_uv_domain_with_nurb(k)?
                    .evaluate(0.5, 0.5);
                if uv.y > center_uv.y {
                    bin_vec = -bin_vec;
                }

                let angle = bin_vec.angle_between(tangent_vec)?;
                let eu_orient = if angle < PI / 2.0 {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                };

                if let Some(corner_eu) = io.rollover_corner_eu {
                    let radial_eu = eu::radial(b, corner_eu)?
                        .ok_or_else(|| invalid("edge use without radial"))?;
                    let cw = eu::cw_edgeuse(b, radial_eu)?
                        .ok_or_else(|| invalid("edge use without CW edge use"))?;
                    let cw_edge = eu::edge(b, cw)?;
                    if io.is_tangential_side_edge {
                        let cw_edge = cw_edge.ok_or_else(|| invalid("edge use without edge"))?;
                        // rbOppositeNormals only; the tangency result is dropped
                        let (_, opposite) =
                            test_manifold_tangent_rollover(k, b, cw_edge, tangency_tol_rad)?;
                        io.opposite_normals = opposite;
                    }
                    if cw_edge != Some(e2) {
                        io.rollover_corner_v = eu_vertex(b, cw)?;
                        io.rollover_corner_eu = eu::cw_edgeuse(b, cw)?;
                        io.side_edge = cw_edge;
                        return Ok(());
                    }
                    io.rollover_corner_eu = None;
                    io.rollover_corner_v = None;
                } else {
                    if eu::orientation(b, eu_on_orig_face)? != eu_orient {
                        eu_on_orig_face = eu::mate(b, eu_on_orig_face)?
                            .ok_or_else(|| invalid("edge use without mate"))?;
                    }
                    let eu_r = eu::radial(b, eu_on_orig_face)?
                        .ok_or_else(|| invalid("edge use without radial"))?;
                    let mut rollover_corner = e1 != e2;
                    if e1 == e2
                        && edge::is_closed(b, e1)?
                        && ((start_param < end_param && eu_orient == OrientType::Opposite)
                            || (start_param > end_param && eu_orient == OrientType::Same))
                    {
                        rollover_corner = true;
                    }
                    if rollover_corner {
                        let eu_r_edge =
                            eu::edge(b, eu_r)?.ok_or_else(|| invalid("edge use without edge"))?;
                        let (tangent, opposite) =
                            test_manifold_tangent_rollover(k, b, eu_r_edge, tangency_tol_rad)?;
                        io.is_tangential_side_edge = tangent;
                        io.opposite_normals = opposite;
                        io.rollover_corner_v = eu_vertex(b, eu_r)?;
                        io.rollover_corner_eu = eu::cw_edgeuse(b, eu_r)?;
                        io.side_edge = if io.is_tangential_side_edge {
                            let corner_eu = io
                                .rollover_corner_eu
                                .ok_or_else(|| invalid("edge use without CW edge use"))?;
                            eu::edge(b, corner_eu)?
                        } else {
                            Some(eu_r_edge)
                        };
                        return Ok(());
                    }
                }
            }

            // If this is the one before the very last segment, do the following check
            if io.curr_index + 2 == total_intervals {
                let next_ivl = interval_at(cc, io.curr_index + 1)?;
                if next_ivl.end.point_class() == PointClassType::Unknown && !is_processed(end_v)? {
                    io.curr_index += 1;
                    return Ok(());
                }
            }
            io.rollover_corner_eu = None;
            io.side_edge = Some(side_edge);
            return Ok(());
        }

        // Process the very last segment
        if ivl.start.point_class() != PointClassType::Edge {
            return Err(sm_err()); // Unknown case
        }
        if ivl.end.point_class() == PointClassType::Unknown && !is_processed(end_v)? {
            return Ok(());
        }
        let side_edge = edge_object(&ivl.start, edge_of)?;

        // Check whether the side edge we hit is tangent, or concave/convex.
        let (tangent, opposite) =
            test_manifold_tangent_rollover(k, b, side_edge, tangency_tol_rad)?;
        io.is_tangential_side_edge = tangent;
        io.opposite_normals = opposite;
        let convex_side_edge =
            test_side_edge_convexity(k, b, side_edge, io.is_tangential_side_edge)?;

        // Cliff or crash (comment above).
        if convex_fillet == convex_side_edge {
            // Trimming process
            io.trim_side_faces = true;
            return Ok(());
        }
        io.side_edge = Some(side_edge);
        return Ok(());
    }
}

/// Not upstream (WR-541, source fork@bc6ddf9): where the axes through `o0` along
/// `a0` and through `o1` along `a1` (unit vectors) meet: the middle of their nearest
/// points; `None` for (nearly) parallel axes (`1 - (a0 . a1)^2 < SM_EFF_ZERO_SQRT`: the
/// centres fill a line) or nearest points farther apart than `tol` (the axes miss each
/// other).
fn axes_meet(o0: Point3, a0: Vec3, o1: Point3, a1: Vec3, tol: f64) -> Option<Point3> {
    let w = o0 - o1;
    let bdot = a0.dot(a1);
    let den = 1.0 - bdot * bdot;
    if den < EFF_ZERO_SQRT {
        return None;
    }
    let d = a0.dot(w);
    let e = a1.dot(w);
    let p0 = o0 + a0 * ((bdot * e - d) / den);
    let p1 = o1 + a1 * ((e - bdot * d) / den);
    if p0.distance(p1) > tol {
        return None;
    }
    Some(p0 + (p1 - p0) * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_custom_solver::SurfaceId;
    use crate::blend::fillet_geom::p1::{FilletEdgeuse, FilletVertexuse};
    use crate::blend::fillet_solver::p1::{FilletExecutiveRef, OffsetSurfaceSlot};
    use crate::intersect::point_class::ClassObjectKind;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::surface::offset_surface::SS_UNKNOWN;
    use crate::topology::Edgeuse;
    use crate::topology::Vertexuse;
    use crate::topology::VertexuseOwner;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(u32);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Edge
        }
    }

    struct JacobianSizeOps {
        solver: FilletSolver,
        size: usize,
        extension_calls: std::cell::Cell<usize>,
        loaded_dimension: std::cell::Cell<Option<usize>>,
    }

    impl FilletSolverOps for JacobianSizeOps {
        fn solver(&self) -> &FilletSolver {
            &self.solver
        }

        fn solver_mut(&mut self) -> &mut FilletSolver {
            &mut self.solver
        }

        fn jacobian_size(&self) -> Result<usize> {
            Ok(self.size)
        }

        fn setup_offset_extension(
            &self,
            _k: &Kernel,
            _surface: &Surface,
            _domain: &mut Extent2d,
        ) -> Result<()> {
            self.extension_calls.set(self.extension_calls.get() + 1);
            Ok(())
        }

        fn load_initial_values(
            &mut self,
            _k: &Kernel,
            _brep: Option<&mut Brep>,
            _do_surf2_calcs: bool,
            _rail_index: usize,
            _surface1: &mut OffsetSurface,
            _surface1_index: usize,
            intervals: &mut ExtentNd,
            _periodicities: &mut Vec<bool>,
            _guess_t: &mut Vec<f64>,
        ) -> Result<Option<crate::blend::fillet_custom_solver::InitialValues>> {
            self.loaded_dimension.set(Some(intervals.dimension()));
            Err(sm_err())
        }
    }

    fn jacobian_size_ops(k: &Kernel, size: usize) -> JacobianSizeOps {
        let net = vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(1.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 1.0, 0.0, NOW),
            ],
        ];
        let base = Surface::from(BSplineSurface::from_nurb(
            crate::nurbs::srf_basic::Surface::new(
                net,
                1,
                1,
                vec![0.0, 0.0, 1.0, 1.0],
                vec![0.0, 0.0, 1.0, 1.0],
            )
            .unwrap(),
            false,
        ));
        let mut solver = FilletSolver::new(1e-6, 0.1, 0.01);
        for rail in 0..2 {
            let offset = OffsetSurface::new(
                k,
                0.1,
                base.clone(),
                SurfaceId::Free(rail as u32),
                SS_UNKNOWN,
            )
            .unwrap();
            solver
                .set_surface(
                    rail,
                    Some(OffsetSurfaceSlot {
                        key: rail as u64,
                        surface: offset,
                    }),
                )
                .unwrap();
        }
        JacobianSizeOps {
            solver,
            size,
            extension_calls: std::cell::Cell::new(0),
            loaded_dimension: std::cell::Cell::new(None),
        }
    }

    fn edgeuse_with_uv_trim() -> (Brep, EdgeuseId) {
        let mut b = Brep::new();
        let v0 = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let v1 = b.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let edge = b.add_edge(v0, v1).unwrap();
        b.edge_mut(edge).unwrap().interval = Extent1d::from_min_max(0.0, 1.0);
        let uv = Curve::from(BSplineCurve::from_nurb(
            crate::nurbs::crv_basic::Curve::new(
                vec![
                    CPoint::new(0.0, 0.0, 0.0, NOW),
                    CPoint::new(1.0, 0.0, 0.0, NOW),
                ],
                1,
                vec![0.0, 0.0, 1.0, 1.0],
            )
            .unwrap(),
            2,
            false,
        ));
        let edgeuse = b
            .add_edgeuse_record(Edgeuse {
                edge: Some(edge),
                orientation: OrientType::Same,
                uv_trim_curve: Some(uv),
                ..Edgeuse::default()
            })
            .unwrap();
        b.edge_mut(edge).unwrap().edgeuses.push(edgeuse);
        (b, edgeuse)
    }

    #[test]
    fn rail_edgeuse_jacobian_size_overflow_returns_invalid_input_before_outputs_change() {
        let k = Kernel::default();
        let mut ops = jacobian_size_ops(&k, usize::MAX);
        let (mut b, side_eu) = edgeuse_with_uv_trim();
        let mut tsect = TsectPnt {
            curve_parameter: 0.375,
            ..TsectPnt::default()
        };
        let before = tsect.clone();
        let result =
            FilletSolver::rail_edgeuse_intersect(&mut ops, &k, &mut b, 0, side_eu, 0.5, &mut tsect);
        assert_eq!(
            result,
            Err(invalid("Jacobian size overflows with edge-use parameter"))
        );
        assert_eq!(tsect, before);
        assert_eq!(ops.extension_calls.get(), 0);
        assert_eq!(ops.loaded_dimension.get(), None);
    }

    #[test]
    fn rail_edgeuse_normal_jacobian_size_reaches_load_initial_values_with_added_dimension() {
        let k = Kernel::default();
        let mut ops = jacobian_size_ops(&k, 4);
        let (mut b, side_eu) = edgeuse_with_uv_trim();
        let result = FilletSolver::rail_edgeuse_intersect(
            &mut ops,
            &k,
            &mut b,
            0,
            side_eu,
            0.5,
            &mut TsectPnt::default(),
        );
        assert_eq!(result, Err(sm_err()));
        assert_eq!(ops.extension_calls.get(), 1);
        assert_eq!(ops.loaded_dimension.get(), Some(5));
    }

    #[test]
    fn rail_rail_jacobian_sum_overflow_and_below_two_are_invalid_input_before_callbacks() {
        let k = Kernel::default();
        for (left_size, right_size, reason) in [
            (usize::MAX, 1, "Jacobian sizes sum overflows"),
            (usize::MAX, 2, "Jacobian sizes sum overflows"),
            (usize::MAX, 3, "Jacobian sizes sum overflows"),
            (1, usize::MAX, "Jacobian sizes sum overflows"),
            (2, usize::MAX, "Jacobian sizes sum overflows"),
            (3, usize::MAX, "Jacobian sizes sum overflows"),
            (0, 0, "Jacobian sizes sum below 2"),
            (0, 1, "Jacobian sizes sum below 2"),
            (1, 0, "Jacobian sizes sum below 2"),
        ] {
            let mut ops = jacobian_size_ops(&k, left_size);
            let mut other = jacobian_size_ops(&k, right_size);
            let mut tsect = TsectPnt {
                curve_parameter: 0.25,
                ..TsectPnt::default()
            };
            let mut other_tsect = TsectPnt {
                curve_parameter: 0.75,
                ..TsectPnt::default()
            };
            let tsect_before = tsect.clone();
            let other_tsect_before = other_tsect.clone();
            let result = FilletSolver::rail_rail_intersect(
                &mut ops,
                &mut other,
                &k,
                None,
                Point2::new(0.5, 0.5),
                0,
                0,
                &mut tsect,
                &mut other_tsect,
            );
            assert_eq!(result, Err(invalid(reason)));
            assert_eq!(ops.extension_calls.get(), 0);
            assert_eq!(other.extension_calls.get(), 0);
            assert_eq!(ops.loaded_dimension.get(), None);
            assert_eq!(other.loaded_dimension.get(), None);
            assert_eq!(tsect, tsect_before);
            assert_eq!(other_tsect, other_tsect_before);
        }
    }

    #[test]
    fn rail_rail_normal_jacobian_sizes_reach_load_initial_values_with_combined_dimension() {
        let k = Kernel::default();
        let mut ops = jacobian_size_ops(&k, 4);
        let mut other = jacobian_size_ops(&k, 5);
        let mut tsect = TsectPnt::default();
        let mut other_tsect = TsectPnt::default();
        let result = FilletSolver::rail_rail_intersect(
            &mut ops,
            &mut other,
            &k,
            None,
            Point2::new(0.5, 0.5),
            0,
            0,
            &mut tsect,
            &mut other_tsect,
        );
        assert_eq!(result, Err(sm_err()));
        assert_eq!(ops.extension_calls.get(), 1);
        assert_eq!(other.extension_calls.get(), 0);
        assert_eq!(ops.loaded_dimension.get(), Some(7));
    }

    /// A pseudo B-rep with one rail edge (two fillet vertices with edge and vertex uses)
    /// and, when given, an original face for the rail.
    fn pseudo_with_rail(
        k: &Kernel,
        start_processed: bool,
        end_processed: bool,
        with_face: bool,
    ) -> (FilletBrep, EdgeId) {
        let mut pseudo = FilletBrep::new();
        let mut v = Vec::new();
        for (i, processed) in [start_processed, end_processed].into_iter().enumerate() {
            let id = pseudo
                .brep_mut()
                .add_vertex(Point3::new(i as f64, 0.0, 0.0))
                .unwrap();
            let mut fv = FilletVertex::new(k, None);
            if processed {
                fv.set_status(FilStatus::Processed);
            }
            pseudo.insert_fillet_vertex(id, fv);
            v.push(id);
        }
        let rail = pseudo.new_fillet_edge(v[0], v[1], None).unwrap();
        pseudo
            .fillet_edge_mut(rail)
            .unwrap()
            .set_fillet_edge_type(FilletEdgeType::Rail);
        let eu1 = pseudo
            .brep_mut()
            .add_edgeuse_record(Edgeuse::default())
            .unwrap();
        let eu2 = pseudo
            .brep_mut()
            .add_edgeuse_record(Edgeuse::default())
            .unwrap();
        for (eu, vert, orient) in [
            (eu1, v[0], OrientType::Same),
            (eu2, v[1], OrientType::Opposite),
        ] {
            let vu = pseudo
                .brep_mut()
                .add_vertexuse_record(Vertexuse::default())
                .unwrap();
            let r = pseudo.brep_mut().edgeuse_mut(eu).unwrap();
            r.edge = Some(rail);
            r.orientation = orient;
            r.vertexuse = Some(vu);
            let u = pseudo.brep_mut().vertexuse_mut(vu).unwrap();
            u.vertex = Some(vert);
            u.owner = VertexuseOwner::Edgeuse(Some(eu));
            pseudo
                .brep_mut()
                .vertex_mut(vert)
                .unwrap()
                .vertexuses
                .push(vu);
            pseudo.insert_fillet_edgeuse(eu, FilletEdgeuse::default());
            pseudo.insert_fillet_vertexuse(vu, FilletVertexuse::new());
        }
        pseudo.brep_mut().edge_mut(rail).unwrap().edgeuses = vec![eu1, eu2];
        if with_face {
            let shell = pseudo.brep_mut().add_shell().unwrap();
            let face = pseudo.brep_mut().add_face(shell).unwrap();
            pseudo
                .fillet_edge_mut(rail)
                .unwrap()
                .set_original_face(Some(face));
        }
        (pseudo, rail)
    }

    /// A classification of `n` intervals whose ends take the given classes (an edge
    /// handle for `PointClassType::Edge`).
    fn classification(
        k: &Kernel,
        ends: &[(PointClassType, PointClassType)],
    ) -> CurveClassification<H> {
        let mut cc = CurveClassification::<H>::new(k);
        cc.set_size(k, ends.len()).unwrap();
        for (i, (s, e)) in ends.iter().enumerate() {
            let ivl = cc.at_mut(i).unwrap();
            ivl.interval = Extent1d::from_min_max(i as f64, i as f64 + 1.0);
            let obj = |c: &PointClassType| (*c == PointClassType::Edge).then_some(H(i as u32));
            ivl.start.set_class_object(*s, obj(s));
            ivl.end.set_class_object(*e, obj(e));
        }
        cc
    }

    fn run(
        k: &Kernel,
        pseudo: &FilletBrep,
        rail: EdgeId,
        cc: &CurveClassification<H>,
        io: &mut RolloverSideEdge,
    ) -> Result<()> {
        let uv = Curve::from(
            BSplineCurve::create_line_segment(
                3,
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        );
        let surface = Surface::from(Plane::from_normal(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ));
        let mut target = Brep::new();
        let edge_of = |h: H| Some(EdgeId::from_index(h.0));
        find_rollover_side_edge(
            k,
            pseudo,
            &mut target,
            cc,
            rail,
            &uv,
            &surface,
            true,
            1.0e-3,
            &edge_of,
            io,
        )
    }

    #[test]
    fn rollover_needs_the_rails_original_face() {
        let k = Kernel::default();
        let (pseudo, rail) = pseudo_with_rail(&k, true, true, false);
        let cc = classification(&k, &[(PointClassType::Edge, PointClassType::Unknown)]);
        let r = run(&k, &pseudo, rail, &cc, &mut RolloverSideEdge::default());
        // GetOriginalFace is NER'd: a null pointer, not an unported stub
        assert!(matches!(r, Err(SolidError::Upstream { code: 1006, .. })));
    }

    #[test]
    fn rollover_skips_the_unclassified_first_interval_of_an_unprocessed_start() {
        let k = Kernel::default();
        let (pseudo, rail) = pseudo_with_rail(&k, false, true, true);
        // interval 0 starts unclassified (skipped: IsProcessed false); the last one
        // does not start on an edge: SM_ERR, with the index advanced past the skip
        let cc = classification(
            &k,
            &[
                (PointClassType::Unknown, PointClassType::Unknown),
                (PointClassType::Unknown, PointClassType::Unknown),
            ],
        );
        let mut io = RolloverSideEdge::default();
        let r = run(&k, &pseudo, rail, &cc, &mut io);
        assert!(matches!(r, Err(SolidError::Upstream { code: 1001, .. })));
        assert_eq!(io.curr_index, 1);
        assert!(!io.trim_side_faces);

        // a processed start is not skipped: the first (last) interval is then judged
        let (pseudo, rail) = pseudo_with_rail(&k, true, true, true);
        let mut io = RolloverSideEdge::default();
        assert!(run(&k, &pseudo, rail, &cc, &mut io).is_err());
        assert_eq!(io.curr_index, 0);
    }

    #[test]
    fn rollover_ending_on_an_unprocessed_end_vertex_returns_without_a_side_edge() {
        let k = Kernel::default();
        let (pseudo, rail) = pseudo_with_rail(&k, true, false, true);
        let cc = classification(&k, &[(PointClassType::Edge, PointClassType::Unknown)]);
        let mut io = RolloverSideEdge::default();
        run(&k, &pseudo, rail, &cc, &mut io).unwrap();
        assert!(io.side_edge.is_none() && !io.trim_side_faces);

        // a processed end vertex goes on to the side edge's tangency test, which needs
        // the side edge in the target B-rep (here: a dangling id)
        let (pseudo, rail) = pseudo_with_rail(&k, true, true, true);
        let r = run(&k, &pseudo, rail, &cc, &mut RolloverSideEdge::default());
        assert!(r.is_err() && !r.unwrap_err().is_unported());
    }

    #[test]
    fn rollover_before_the_last_interval_needs_an_edge_end() {
        let k = Kernel::default();
        let (pseudo, rail) = pseudo_with_rail(&k, true, true, true);
        let cc = classification(
            &k,
            &[
                (PointClassType::Edge, PointClassType::Unknown),
                (PointClassType::Edge, PointClassType::Edge),
            ],
        );
        let r = run(&k, &pseudo, rail, &cc, &mut RolloverSideEdge::default());
        assert!(matches!(r, Err(SolidError::Upstream { code: 1001, .. })));
    }

    #[test]
    fn cliff_side_geometry_runs_until_its_unported_callees() {
        let k = Kernel::default();
        let (mut pseudo, rail) = pseudo_with_rail(&k, true, true, true);
        let mut s = FilletSolver::new(1e-6, 0.1, 0.01);
        // without an executive upstream dereferences NULL
        let target = Brep::new();
        let r = s.create_cliff_roll_over_side_fg(
            &k,
            &target,
            &mut pseudo,
            FilletSolverId(0),
            rail,
            EdgeId::from_index(0),
            FaceId::from_index(0),
            true,
            0.0,
        );
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        // trimming with an executive: the original face becomes the side face, which the
        // target B-rep lacks (a dangling id), before any stub is reached
        s.executive = Some(FilletExecutiveRef);
        let r = s.create_cliff_roll_over_side_fg(
            &k,
            &target,
            &mut pseudo,
            FilletSolverId(0),
            rail,
            EdgeId::from_index(0),
            FaceId::from_index(0),
            true,
            0.0,
        );
        assert!(r.is_err() && !r.unwrap_err().is_unported());
    }

    #[test]
    fn cliff_side_geometry_builds_patch_and_new_rail_on_side_edge() {
        use crate::topology::Face;

        let k = Kernel::default();
        let (mut pseudo, rail) = pseudo_with_rail(&k, true, true, true);
        let fe1 = pseudo
            .brep()
            .edge(rail)
            .unwrap()
            .edgeuses
            .first()
            .copied()
            .unwrap();
        pseudo.insert_fillet_edgeuse(fe1, FilletEdgeuse::default());
        let fe2 = pseudo
            .brep()
            .edge(rail)
            .unwrap()
            .edgeuses
            .get(1)
            .copied()
            .unwrap();
        pseudo.insert_fillet_edgeuse(fe2, FilletEdgeuse::default());

        let mut target = Brep::new();
        let tv0 = target.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let tv1 = target.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let side_edge = target
            .add_edge_record(crate::topology::Edge {
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 1e-6,
                curve: Some(Curve::from(
                    BSplineCurve::create_line_segment(
                        3,
                        Point3::new(0.0, 0.0, 0.0),
                        Point3::new(1.0, 0.0, 0.0),
                    )
                    .unwrap(),
                )),
                ..crate::topology::Edge::new([tv0, tv1])
            })
            .unwrap();
        let shell = target.add_shell().unwrap();
        let c0 = BSplineCurve::create_line_segment(
            3,
            Point3::new(-1.0, -1.0, 0.0),
            Point3::new(2.0, -1.0, 0.0),
        )
        .unwrap();
        let c1 = BSplineCurve::create_line_segment(
            3,
            Point3::new(-1.0, 2.0, 0.0),
            Point3::new(2.0, 2.0, 0.0),
        )
        .unwrap();
        let patch = BSplineSurface::create_ruled_surface(&c0, &c1, SurfParamType::V).unwrap();
        let side_face = target
            .add_face_record(Face {
                surface: Some(Surface::from(patch)),
                ..Face::new(shell)
            })
            .unwrap();

        let mut s = FilletSolver::new(1e-6, 0.1, 0.01);
        s.executive = Some(FilletExecutiveRef);
        let side_fg = s
            .create_cliff_roll_over_side_fg(
                &k,
                &target,
                &mut pseudo,
                FilletSolverId(3),
                rail,
                side_edge,
                side_face,
                false,
                0.5,
            )
            .unwrap()
            .unwrap();
        assert_eq!(s.fillet_geoms(), &[side_fg]);
        let g = pseudo.geom(side_fg).unwrap();
        assert_eq!(g.fillet_geom_type(), FilletGeomType::CliffSidePatch);
        assert_eq!(g.rail(0), Some(rail));
        let new_rail = g.rail(1).unwrap();
        assert!(edge::curve(pseudo.brep(), new_rail).unwrap().is_some());
        assert_eq!(pseudo.brep().edge(new_rail).unwrap().edgeuses.len(), 2);
    }

    #[test]
    fn record_fillet_error_in_populates_error_info_and_honours_wr_190() {
        let k = Kernel::default();
        let mut target = Brep::new();
        let v0 = target.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let v1 = target.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let te = target
            .add_edge_record(crate::topology::Edge::new([v0, v1]))
            .unwrap();
        let teu = target
            .add_edgeuse_record(Edgeuse {
                edge: Some(te),
                ..Edgeuse::default()
            })
            .unwrap();

        let mut pseudo = FilletBrep::new();
        let geom = FilletGeom::new(&k, None, FilletGeomType::Default).unwrap();
        let g = pseudo.add_geom(geom);
        let r0 = pseudo.make_rail_edge(&k, g, 0).unwrap();
        let r1 = pseudo.make_rail_edge(&k, g, 1).unwrap();
        let mut plane = Plane::from_normal(Point3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        plane.make_nurb().unwrap();
        pseudo
            .geom_mut(g)
            .unwrap()
            .set_fillet_surface(Some(Surface::from(plane)));
        let cl =
            BSplineCurve::create_line_segment(3, Point3::ZERO, Point3::new(1.0, 0.0, 0.0)).unwrap();
        pseudo.geom_mut(g).unwrap().center_line_curve = Some(Curve::from(cl));
        // Only rail 1 has a curve: WR-190 adds rail 0 to the error edges.
        let r1_crv = Curve::from(
            BSplineCurve::create_line_segment(3, Point3::ZERO, Point3::new(0.0, 1.0, 0.0)).unwrap(),
        );
        set_edge_curve(pseudo.brep_mut(), r1, Some(r1_crv)).unwrap();

        let mut s = FilletSolver::new(1e-6, 0.1, 0.01);
        s.edgeuses = [Some(teu), None];
        s.fillet_geoms = vec![g];
        s.status = FilStatus::SurfTracingFailure;

        let mut t = TsectPnt::default();
        *t.crv_pos_mut() = Point3::new(1.25, -2.5, 0.5);

        // Missing executive is rejected before looking at ErrorInfo.
        let mut info = ErrorInfo::default();
        let err = s
            .record_fillet_error_in(
                &target,
                &pseudo,
                FilletErrorType::BadInt,
                &[t.clone()],
                Some("bad int"),
                Some(&mut info),
            )
            .unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }));

        s.executive = Some(FilletExecutiveRef);
        // Without ErrorInfo, record_fillet_error notes into a scratch record.
        s.record_fillet_error(&target, &pseudo, 5, &[t.clone()], "bad int")
            .unwrap();

        s.record_fillet_error_in(
            &target,
            &pseudo,
            FilletErrorType::BadInt,
            &[t],
            Some("bad int"),
            Some(&mut info),
        )
        .unwrap();
        assert_eq!(info.code, FilletErrorType::BadInt as i32);
        assert_eq!(info.status, FilStatus::SurfTracingFailure as i32);
        assert_eq!(info.edge_type, FilletEdgeType::Unknown as i32);
        assert_eq!(info.edge, Some(SolverErrorObject::FilletedEdge(te)));
        assert_eq!(
            info.surfaces,
            vec![Some(SolverErrorObject::FilletSurface(g))]
        );
        assert_eq!(
            info.curves,
            vec![Some(SolverErrorObject::CenterLineCurve(g))]
        );
        assert_eq!(info.edges, vec![Some(SolverErrorObject::FilletEdge(r0))]);
        assert_eq!(info.location, Some(Point3::new(1.25, -2.5, 0.5)));
        assert_eq!(info.comment, "bad int");
    }

    /// WR-541 (source fork@bc6ddf9): the corner ball's centre is where the
    /// cylinders' axes meet, also when they are a rounding apart; parallel axes or axes
    /// that miss each other give none.
    #[test]
    fn corner_ball_centre_is_where_the_axes_meet() {
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        // axes along x through (0, 2, 2) and along y through (2, 0, 2 + 1e-9)
        let c = axes_meet(
            Point3::new(0.0, 2.0, 2.0),
            x,
            Point3::new(2.0, 0.0, 2.0 + 1e-9),
            y,
            1e-6,
        )
        .unwrap();
        assert!(
            c.distance(Point3::new(2.0, 2.0, 2.0 + 0.5e-9)) < 1e-15,
            "{c:?}"
        );
        assert!(axes_meet(
            Point3::new(0.0, 0.0, 0.0),
            x,
            Point3::new(0.0, 1.0, 0.0),
            x,
            1e-6
        )
        .is_none());
        assert!(axes_meet(
            Point3::new(0.0, 2.0, 2.0),
            x,
            Point3::new(2.0, 0.0, 3.0),
            y,
            1e-6
        )
        .is_none());
    }
}
