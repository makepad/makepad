//! Fillet geometry, part 4: the UV trim curve of a fillet edge between two fillet
//! vertices on a surface (`sm_CreateUVCurve`), the fillet-surface intersection vertex
//! calculators (`sm_CalcNx2VertFilletXFillet`, `CalcFilletInt2Fillets`,
//! `CalcRailIntExtendedEdgeuse`), the rail index of a fillet vertex, the face of a
//! fillet geometry in the fillet B-rep (`MakeFaceBrep`), the insertion of the rails and
//! side curves into the target B-rep (`InsertIntersectionTopology`) and the blending
//! geometry (`CalcBlendingGeom`).
//!
//! Upstream: `source/SMLib/src/SmFilletGeom.cpp`, `source/SMLib/inc/SmFilletGeom.h`.
//!
//! | Rust                                              | Upstream                                     |
//! |---------------------------------------------------|----------------------------------------------|
//! | [`create_uv_curve`], [`UvCurves`]                 | `sm_CreateUVCurve`                           |
//! | [`FilletBrep::calc_nx2_vert_fillet_x_fillet`]     | `sm_CalcNx2VertFilletXFillet`                |
//! | [`FilletBrep::calc_fillet_int_2_fillets`], [`FilletBrep::calc_fillet_int_2_fillets_with_context`] | `SmFilletVertex::CalcFilletInt2Fillets` |
//! | [`FilletBrep::calc_rail_int_extended_edgeuse`]    | `SmFilletVertex::CalcRailIntExtendedEdgeuse` |
//! | [`FilletBrep::make_face_brep`]                    | `SmFilletGeom::MakeFaceBrep`                 |
//! | [`FilletBrep::rail_index_of_vertex`]              | `SmFilletGeom::GetRailIndexOfVertex`         |
//! | [`FilletBrep::insert_intersection_topology_with_context`] | `SmFilletGeom::InsertIntersectionTopology` |
//! | [`FilletBrep::calc_blending_geom`]                | `SmFilletGeom::CalcBlendingGeom`             |
//!
//! Deviations:
//! - **Object model.** As in `blend.fillet_geom_p1`, a fillet vertex, edge or geometry
//!   is an id of the [`FilletBrep`] (`self`). `GetFilletSolver()` is an opaque
//!   [`FilletSolverId`] resolved in the caller's `solvers` table (index `id.0`); an id
//!   not in the table is [`SolidError::InvalidInput`], a geometry without a solver
//!   `SM_ERR_NULL_POINTER` (upstream dereferences `NULL`). The target B-rep the
//!   classifications, original faces and solver edge uses refer to is `target`; the
//!   fillet B-rep `MakeFaceBrep` builds into (`SmFilletExecutive::m_pFilletBrep`) is
//!   `brep`. The `SmContext` of a corner or solver is the [`Kernel`]; a corner that
//!   upstream dereferences only for its context must still be present (`NULL` is
//!   `SM_ERR_NULL_POINTER`).
//! - **Fillet corners.** A vertex's `SmFilletCorner*` is a
//!   [`FilletCornerId`](super::p1::FilletCornerId) that the caller's [`CornerLookup`]
//!   resolves to the [`FilletCorner`](crate::blend::fillet_corner::p3::FilletCorner) (unit
//!   `blend.fillet_corner_p1`) whose corner type and filleted vertex the calculators
//!   read; the lookup also holds the executive's extended surfaces
//!   (`GetExtendedSurface`).
//! - **Cones.** `sm_CreateUVCurve`'s `IsKindOf(SmCone_TYPE)` test is a match on
//!   [`SurfaceKind::Cone`] (which also holds `SmCylinder`s).
//! - **Curves.** Upstream's curve arrays hold pointers into the edges and copies are
//!   made at the end of `MakeFaceBrep`; here the arrays hold clones, and the final
//!   copies are made the same way (`Copy` for the 3D curves, the copy constructor for
//!   the UV curves). The retained session transfers the original fillet surface to
//!   its managed face, recording early destruction and partial installation on
//!   failure. The legacy indexed API retains its copied-surface contract.
//!   `SetCurve` on a fillet edge applies the
//!   `SM_NO_CHANGE_GEOMETRY` resets directly (as `blend.fillet_geom_p1` does: the
//!   pseudo B-rep has no notification environment) and `SetOwner` of the new curve is
//!   implied by the edge holding it.
//! - **Statuses.** `SER`/`NER` failures are errors (`SM_ERR`, `SM_ERR_NULL_POINTER`);
//!   the failure statuses `sm_CreateUVCurve` swallows (its surface/plane intersections)
//!   are swallowed here too, except that [`SolidError::Unported`] always propagates.
//!   The double tolerances passed to `GlobalPointSolve` convert to `SmZoneTol3d`
//!   (`USE_DEFAULT` becomes the zone tolerance, then `fabs`).
//! - **Surface/surface intersections.** `sm_SrfSrfIntersection` is
//!   `blend.fillet_geom_p2`'s, called with the fillet vertices' points; its curves are
//!   retained as their actual B-spline-derived objects for virtual operations, and
//!   [`UvCurves`] returns them as such (an `SmEllipse` stays an ellipse).
//! - **Part 6.** `GetRailIndexOfVertexuse`, `JoinTwoFilletVertsByCurves`,
//!   `SmFilletEdge::CalcBlendingRail` and `RecordFilletError` are `blend.fillet_geom_p6`'s;
//!   `MakeFaceBrep`'s 3D curve list holds `Option<Curve>` as the join appends them.
//! - **Executive state.** `InsertIntersectionTopology` runs against the executive's
//!   topology intersector, attribute users, marker attributes (777/778) and topology
//!   bookkeeping through the caller's [`IntersectionTopologyContext`] (the live
//!   executive session).
//! - **B-spline casts.** `SM_CAST_PTR(SmBSplineCurve, ..)` succeeds for every kind
//!   derived from `SmBSplineCurve` upstream (`SmLine`, `SmEllipse`/`SmCircle`,
//!   `SmHyperbola`, `SmParabola`): `as_b_spline` returns their B-spline base.
//! - **Corner blend curves.** `CalcBlendingGeom` passes `pFE->GetCurve()` (an
//!   `SmCurve*`, uncast and unchecked) to `CreateCornerBlend`; the Rust
//!   `BSplineSurface::create_corner_blend` takes B-spline curves, so a fillet edge
//!   without a curve, or with a curve of a kind not derived from `SmBSplineCurve`, is
//!   `SM_ERR_NULL_POINTER` here.
//!   Its base surfaces are likewise `Option<&BSplineSurface>`: a base face or fillet
//!   surface whose kind has no B-spline net (offset, curve-bounded, STEP surfaces) is
//!   passed as `None`, where upstream passes the real `SmSurface*`.
//! - **Deleted edges.** `InsertIntersectionTopology` keeps one `aDeletedEdges` list for
//!   the whole routine, as upstream: each `MergeCurveOnSurfaces` appends to it and
//!   every `ClearFilletBrepEdges` clears against all edges deleted so far.
//! - **Missing mates and UV curves.** Upstream fetches `pPrimEU->GetMate()` and the
//!   UV curves of `sm_SrfSrfIntersection` early but dereferences them only where they
//!   are used; here they stay `Option`s until then, and a `NULL` one is
//!   `SM_ERR_NULL_POINTER` only at that use (upstream dereferences `NULL`).
//! - **Arguments.** Out-params become return values; `iDebugLevel` and debug drawing
//!   are dropped.

use crate::blend::fillet_solver::p1::{FilletSolver, FilletSurfaceGeneratorType};
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionArray, SolutionObject};
use crate::geom::tol::zone_tol3d;
use crate::geom::vector2d::{Point2, Vec2};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p2::{BlendNeighbour, OrientType};
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::query::brep::BrepFaceServices;
use crate::surface::plane::Plane;
use crate::surface::surface::p1::map_to_2d_dir;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::brep::p7::{
    make_face_with_curves, make_face_with_curves_transfer, FaceCurvesInput, NewFace,
};
use crate::topology::brep_cache::CacheObject;
use crate::topology::edge;
use crate::topology::edgeuse::p1::{self as eu1, primary_edgeuse};
use crate::topology::trimming_tools::{trim_surface_with_model_space_curves, TrimInput};
use crate::topology::vertex;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, VertexId};

use super::p1::{
    cleanup_noise, join_verts_by_capping, CornerLookup, CurveObjectHandle, FilStatus, FilletBrep,
    FilletCornerType, FilletEdgeType, FilletGeomId, FilletGeomType, FilletSolverId,
};
use super::p6::join_two_fillet_verts_by_curves_resolved;

/// The indexed value API retains its historical copied-surface behavior. The live
/// retained-solver API transfers the original surface into its actual managed face.
#[derive(Clone, Copy)]
enum FaceSurfaceBackend {
    IndexedValue,
    RetainedTransfer,
}

const OP_UV: &str = "blend::fillet_geom::p4::create_uv_curve";
const OP_NX2: &str = "blend::fillet_geom::p4::FilletBrep::calc_nx2_vert_fillet_x_fillet";
const OP_INT2: &str = "blend::fillet_geom::p4::FilletBrep::calc_fillet_int_2_fillets";
const OP_EXT: &str = "blend::fillet_geom::p4::FilletBrep::calc_rail_int_extended_edgeuse";
const OP_FACE: &str = "blend::fillet_geom::p4::FilletBrep::make_face_brep";
const OP_RAIL_IDX: &str = "blend::fillet_geom::p4::FilletBrep::rail_index_of_vertex";
const OP_INSERT: &str = "blend::fillet_geom::p4::FilletBrep::insert_intersection_topology";
const OP_BLEND: &str = "blend::fillet_geom::p4::FilletBrep::calc_blending_geom";

/// The solver of a fillet geometry in the caller's table (index `id.0`).
pub(crate) fn solver_of<'s>(
    solvers: &'s [FilletSolver],
    id: FilletSolverId,
    op: &'static str,
) -> Result<&'s FilletSolver> {
    usize::try_from(id.0)
        .ok()
        .and_then(|i| solvers.get(i))
        .ok_or_else(|| SolidError::invalid_input(op, "fillet solver id not in the solver table"))
}

/// `smos_Min`.
fn smin(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `SmVertex::SetPoint` on a fillet vertex: the `SM_NO_CHANGE_GEOMETRY` notification
/// resets the cached gaps of every vertex use of the vertex, then the point is stored
/// (as `blend.fillet_geom_p1` does).
pub(crate) fn set_vertex_point(brep: &mut Brep, v: VertexId, point: Point3) -> Result<()> {
    for vu in brep.vertex(v)?.vertexuses.clone() {
        let vu = brep.vertexuse_mut(vu)?;
        vu.vertex_edge_gap.reset();
        vu.vertex_face_gap.reset();
    }
    brep.vertex_mut(v)?.point = point;
    Ok(())
}

/// `SmEdge::SetCurve(pCurve, TRUE)` on a fillet edge: the `SM_NO_CHANGE_GEOMETRY`
/// notification clears each edge use's UV trim curve and drop-curve record and resets
/// its cached edge/face gap (its mate's too) and the cached gaps of the edge's vertex
/// uses; then the curve (possibly `NULL`) replaces the old one (as
/// `blend.fillet_geom_p1` does for a rail).
pub(crate) fn set_edge_curve(brep: &mut Brep, e: EdgeId, curve: Option<Curve>) -> Result<()> {
    for eu in edge::edgeuses(brep, e, None, None)? {
        let mate_eu = eu1::mate(brep, eu)?;
        let rec = brep.edgeuse_mut(eu)?;
        rec.uv_trim_curve = None;
        rec.drop_curve_fail = None;
        rec.max_edge_face_gap.reset();
        let vu = rec.vertexuse;
        if let Some(m) = mate_eu {
            brep.edgeuse_mut(m)?.max_edge_face_gap.reset();
        }
        if let Some(vu) = vu {
            let vu = brep.vertexuse_mut(vu)?;
            vu.vertex_edge_gap.reset();
            vu.vertex_face_gap.reset();
        }
    }
    brep.edge_mut(e)?.curve = curve;
    Ok(())
}

/// `SmPoint3d(SmPoint2d)`: `z = 0`.
const fn p3(uv: Point2) -> Point3 {
    Point3::new(uv.x, uv.y, 0.0)
}

/// The surface `sm_CreateUVCurve`'s point solves name (upstream `pSurface`, `this` of
/// `SmSurface::GlobalPointSolve`), with the natural UV domain
/// `SmSolution::GetParameterInterval` reads back from it:
/// `SmGlobalSolver::IdenticalSolutions` (SmGlobalSolver.cpp:1910-1931) merges two
/// solutions whose parameters differ by less than `1e-4` of that domain's sides.
#[derive(Debug, Clone, Copy, PartialEq)]
struct UvSurfaceHandle(Extent2d);

impl SolutionObject for UvSurfaceHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Surface
    }

    fn natural_uv_domain(&self) -> Option<Extent2d> {
        Some(self.0)
    }
}

/// The UV parameters (`m_vStart[0]`, `m_vStart[1]`) of solution `i`.
fn solution_uv<O: SolutionObject>(
    sols: &SolutionArray<O>,
    i: usize,
    op: &'static str,
) -> Result<Point2> {
    let s = sols
        .get(i)
        .ok_or_else(|| SolidError::invalid_input(op, "solution index out of range"))?;
    Ok(Point2::new(s.start.parameters[0], s.start.parameters[1]))
}

/// A B-spline 3D or UV curve held as a [`Curve`] (`SM_CAST_PTR(SmBSplineCurve, ..)`):
/// the B-spline (base) of the kinds derived from `SmBSplineCurve` (`SmLine`,
/// `SmEllipse`/`SmCircle`, `SmHyperbola`, `SmParabola`), `None` for the others.
const fn as_b_spline(c: &Curve) -> Option<&BSplineCurve> {
    match c.kind() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => Some(l.base()),
        CurveKind::Ellipse(e) => Some(e.base()),
        CurveKind::Hyperbola(h) => Some(h.base()),
        CurveKind::Parabola(p) => Some(p.base()),
        CurveKind::Hermite(_)
        | CurveKind::Conic(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::CrvOnSurf(_)
        | CurveKind::TangentField(_)
        | CurveKind::Composite(_) => None,
    }
}

/// The B-spline net of a surface that is an `SmBSplineSurface` (a B-spline surface, a
/// plane, a surface of revolution, a cone, a sphere or a torus):
/// `SM_CAST_PTR(SmBSplineSurface, ..)`.
fn b_spline_net(s: &Surface) -> Option<&BSplineSurface> {
    match s.kind() {
        SurfaceKind::BSpline(b) => Some(b),
        SurfaceKind::Plane(p) => Some(p.base()),
        SurfaceKind::SurfOfRevolution(r) => Some(r.base()),
        SurfaceKind::SurfOfExtrusion(r) => Some(r.base()),
        SurfaceKind::Cone(c) => Some(c.rev().base()),
        SurfaceKind::Sphere(s) => Some(s.base().base()),
        SurfaceKind::Torus(t) => Some(t.base().base()),
        _ => None,
    }
}

// ---- unported callees ---------------------------------------------------------------

/// The curves `sm_SrfSrfIntersection` returns: the 3D intersection curve (`NULL` on a
/// success without a curve) and its UV curves on both surfaces.
/// Also used by `blend.fillet_geom_p5`.
pub(super) struct SrfSrfCurves {
    pub(super) curve_3d: Option<Curve>,
    pub(super) uv_curve1: Option<Curve>,
    pub(super) uv_curve2: Option<Curve>,
}

/// `sm_SrfSrfIntersection` ([`super::p2::srf_srf_intersection`]) with fillet vertices
/// (their points) for its `SmVertex` arguments. The native B-spline casts retain
/// the actual analytic curve objects. Also used by `blend.fillet_geom_p5`.
#[allow(clippy::too_many_arguments)]
pub(super) fn srf_srf_intersection(
    k: &Kernel,
    fb: &FilletBrep,
    start_fv: Option<VertexId>,
    end_fv: Option<VertexId>,
    dir: Option<&mut Vec3>,
    end_dir: Option<Vec3>,
    surface1: &Surface,
    surface2: &Surface,
    approx_tol: f64,
    ang_tol: f64,
    skip_two_pnts_intersection: bool,
    ref_point: Option<Point3>,
) -> Result<SrfSrfCurves> {
    let start = start_fv.map(|v| fb.vertex_point(v)).transpose()?;
    let end = end_fv.map(|v| fb.vertex_point(v)).transpose()?;
    let r = super::p2::srf_srf_intersection(
        k,
        start,
        end,
        dir,
        end_dir,
        surface1,
        surface2,
        approx_tol,
        ang_tol,
        super::p2::SrfSrfOptions {
            skip_two_pnts_intersection,
            ref_point,
        },
    )?;
    let part = |c: Option<Curve>| c.filter(|curve| as_b_spline(curve).is_some());
    Ok(SrfSrfCurves {
        curve_3d: part(r.curve_3d),
        uv_curve1: part(r.uv_curve1),
        uv_curve2: part(r.uv_curve2),
    })
}

/// The curve lists `MakeFaceBrep` grows: 3D curves (`NULL` entries where a side edge
/// has no curve, as `JoinTwoFilletVertsByCurves` appends them), UV curves (`NULL`
/// entries for a cliff side patch) and orientations.
#[derive(Debug, Default)]
struct CurveLists {
    curves_3d: Vec<Option<Curve>>,
    uv_curves: Vec<Option<Curve>>,
    orients: Vec<OrientType>,
}

/// Native lifetime of a face marker during geometry insertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntersectionFaceRole {
    /// Rail merge: temporarily enable editing and retain marker 777.
    Rail,
    /// Side merge: retain marker 778 without changing editing.
    Side,
}

/// Retained objects and scoped edits used by `InsertIntersectionTopology`.
/// Surface bindings must identify the original object; resolving a replacement
/// surface as though it were that object is an error.
pub trait IntersectionTopologyContext {
    /// The actual face marker owned for one native scope.
    type FaceMarker;
    /// Authority for the original fillet surface, not a copied surface.
    type SurfaceBinding;

    /// The caller's kernel.
    fn kernel(&self) -> &Kernel;
    /// Resolve the executive's intersector at the native getter position.
    /// # Errors
    /// Missing live intersector context.
    fn begin_intersection(&mut self, solver: &FilletSolver) -> Result<()>;
    /// Capture the original surface at the native getter position.
    /// # Errors
    /// Unresolvable geometry or surface authority.
    fn surface_binding(
        &mut self,
        pseudo: &FilletBrep,
        geometry: FilletGeomId,
    ) -> Result<Option<Self::SurfaceBinding>>;
    /// Read the current users of the executive's actual edge attribute.
    /// # Errors
    /// Missing attribute or unresolvable users.
    fn attribute_edges(&self) -> Result<Vec<EdgeId>>;
    /// Attach the marker, run the body, and clean up on every return. Rail scopes
    /// also restore the previous editing flag after the body.
    /// # Errors
    /// Marker setup or the body fails.
    fn with_face_scope<T>(
        &mut self,
        target: &mut Brep,
        face: FaceId,
        role: IntersectionFaceRole,
        body: impl FnOnce(&mut Self, &mut Brep, &Self::FaceMarker) -> Result<T>,
    ) -> Result<T>;
    /// Run `body` on the B-rep of the original face of solver `solver`'s rail side
    /// `side` (`pBaseFace->GetBrep()`): `target`, unless the context holds another
    /// base B-rep for that rail (a surface/surface fillet's second surface).
    /// # Errors
    /// Those of `body`, or a base B-rep the context cannot lend.
    fn with_rail_base<T>(
        &mut self,
        target: &mut Brep,
        solver: FilletSolverId,
        side: usize,
        body: impl FnOnce(&mut Self, &mut Brep) -> Result<T>,
    ) -> Result<T> {
        let _ = (solver, side);
        body(self, target)
    }
    /// Read the current tolerance through a short borrow of the managed edge.
    /// # Errors
    /// The managed edge is no longer live or belongs to another body.
    fn managed_edge_tolerance(&self, edge: EdgeId) -> Result<f64>;
    /// Merge the actual curve into its target face and associated managed edge.
    /// # Errors
    /// The native merge fails or lacks a required capability. Deleted edges are
    /// appended even on ordinary failure; the existing list is never cleared.
    #[allow(clippy::too_many_arguments)]
    fn merge_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        face: FaceId,
        tolerance: f64,
        pseudo_edge: EdgeId,
        managed_edge: EdgeId,
        deleted_edges: &mut Vec<EdgeId>,
    ) -> Result<()>;
    /// Process users of the still-live marker after a merge.
    /// # Errors
    /// Topology bookkeeping fails.
    fn update_topology(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        marker: &Self::FaceMarker,
        face: FaceId,
    ) -> Result<()>;
    /// Register the native empty intersection against the original surface.
    /// # Errors
    /// Registration fails, or the original surface is no longer resolvable.
    fn register_intersection(
        &mut self,
        pseudo: &FilletBrep,
        target: &mut Brep,
        face: FaceId,
        surface: &Option<Self::SurfaceBinding>,
    ) -> Result<()>;
    /// Record the ordinary rail-merge failure at its native position.
    /// # Errors
    /// Error recording fails or lacks actual context.
    #[allow(clippy::too_many_arguments)]
    fn record_bad_merge(
        &mut self,
        pseudo: &FilletBrep,
        geometry: FilletGeomId,
        filleted_edge: Option<EdgeId>,
        target: &Brep,
        face: FaceId,
        rail: EdgeId,
        surface: &Option<Self::SurfaceBinding>,
    ) -> Result<()>;
}

// ---- sm_CreateUVCurve -----------------------------------------------------------------

/// The curves [`create_uv_curve`] builds: `rp3DCurve` (only when the UV curve is not
/// an iso-parametric line), `rpUVCurve1` and `rpUVCurve2` (the second seam curve).
#[derive(Debug, Clone, Default)]
pub struct UvCurves {
    /// `rp3DCurve`: the actual `SmBSplineCurve`-derived object (an `SmEllipse` from a
    /// plane cut of a cylinder, for one).
    pub curve_3d: Option<Curve>,
    /// `rpUVCurve1`: the actual `SmBSplineCurve`-derived object.
    pub uv_curve1: Option<Curve>,
    /// `rpUVCurve2`.
    pub uv_curve2: Option<BSplineCurve>,
}

/// `sm_CreateUVCurve`: the UV trim curve on `surface` running from the point of
/// `start_fv` to that of `end_fv`.
///
/// With `create_two_boundary_curves` the points are on the seam of a surface closed in
/// u: the two u-boundary iso-lines (trimmed at the cut corner when one v corner of the
/// surface is cut off) are returned. Otherwise both points are dropped onto the surface
/// (intersecting with growing tolerances, then minimising); multiple solutions on a
/// seam or a degenerate edge are sorted out, and the result is an iso line, a second
/// seam line when both points are on the seam, or, for a curve that is not an iso
/// line, the intersection of the surface with a plane through the end point.
///
/// # Errors
///
/// `SM_ERR` when the seam surface is not closed, a cut corner is not found, a point
/// does not drop or multiple solutions cannot be sorted out; `SM_ERR_NULL_POINTER` for
/// a surface that is not an `SmBSplineSurface` in the plane branch; those of the
/// evaluations, solves and line creations; [`SolidError::Unported`] from the plane
/// branch's `sm_SrfSrfIntersection` ([`super::p2::srf_srf_intersection`]).
#[allow(clippy::too_many_lines)] // one upstream routine
pub fn create_uv_curve(
    k: &Kernel,
    fb: &FilletBrep,
    start_fv: VertexId,
    end_fv: VertexId,
    create_two_boundary_curves: bool,
    surface: &Surface,
    approx_tol: f64,
) -> Result<UvCurves> {
    // init output
    let mut out = UvCurves::default();

    // locals
    let start_pnt = fb.vertex_point(start_fv)?;
    let end_pnt = fb.vertex_point(end_fv)?;
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;

    // when asked - create 2 UVTrimCurves running along surface seam
    if create_two_boundary_curves {
        // First double-check that the surface is indeed closed.
        let mut uv00 = uv_domain.min();
        let mut uv11 = uv_domain.max();
        let mut uv01 = Point2::new(uv00.x, uv11.y);
        let mut uv10 = Point2::new(uv11.x, uv00.y);

        // evaluate Vmin corners
        let srf_pt00 = surface.evaluate_point(k, uv00)?;
        let srf_pt10 = surface.evaluate_point(k, uv10)?;
        if srf_pt00.distance(srf_pt10) > approx_tol {
            return Err(SolidError::sm_err(OP_UV));
        }

        // evaluate Vmax corners
        let srf_pt01 = surface.evaluate_point(k, uv01)?;
        let srf_pt11 = surface.evaluate_point(k, uv11)?;
        if srf_pt01.distance(srf_pt11) > approx_tol {
            return Err(SolidError::sm_err(OP_UV));
        }

        // Now check which v-direction is indicated by Start & End verts
        let mut which_corner_cut: i32 = 0; // -1 for vMin, +1 for vMax
        let mut direction: i32 = if start_pnt.distance(srf_pt00) < approx_tol {
            1
        } else if end_pnt.distance(srf_pt00) < approx_tol {
            -1
        } else {
            0
        };

        if direction == 0 {
            // Perhaps the SMin corner has been cut off.  Check SMax corner.
            direction = if end_pnt.distance(srf_pt01) < approx_tol {
                1
            } else if start_pnt.distance(srf_pt01) < approx_tol {
                -1
            } else {
                0
            };

            // Not currently handling it if both corners are cut off.
            if direction == 0 {
                return Err(SolidError::sm_err(OP_UV));
            }

            which_corner_cut = -1;
        } else {
            // the low-v corner is not cut off: is the high-v corner ok?
            which_corner_cut = if (direction > 0 && end_pnt.distance(srf_pt01) > approx_tol)
                || (direction < 0 && start_pnt.distance(srf_pt01) > approx_tol)
            {
                1
            } else {
                which_corner_cut
            };
        }

        // the rib is circular and less than 180 degrees: the midpoint converges
        let guess_param = 0.5;

        // when a corner has been cut - find the param value that trims the isoCurve to
        // the cut corner
        if which_corner_cut != 0 {
            let pt_to_drop = if which_corner_cut * direction > 0 {
                end_pnt
            } else {
                start_pnt
            };

            // Extract the full-length constant start-u isoParameter rib curve.
            let rib_iso_curve = surface
                .create_iso_parametric_curve(k, SurfParamType::U, uv00.x, approx_tol, None, false)?
                .curve;

            // Drop sPtToDrop to full curve to find trim param value for cut corner
            let dropped = rib_iso_curve.drop_point(
                k,
                &rib_iso_curve.natural_interval(),
                pt_to_drop,
                None,
                approx_tol,
                Some(guess_param),
                SolverOperation::Minimize,
            )?;
            if !dropped.success {
                return Err(SolidError::sm_err(OP_UV));
            }
            let drop_param = dropped.parameter;

            // trim the UVTrimCurve on the seam side of the surface which has been cut
            if which_corner_cut < 0 {
                uv00 = Point2::new(uv00.x, drop_param);
                uv10 = Point2::new(uv10.x, drop_param);
            } else {
                uv01 = Point2::new(uv01.x, drop_param);
                uv11 = Point2::new(uv11.x, drop_param);
            }
        }

        // set up the desired IsoParameter intervals
        let (start_uv0, end_uv0, start_uv1, end_uv1) = if direction > 0 {
            // run constant u IsoCurve from Surface v Min to Max
            (uv00, uv01, uv10, uv11)
        } else {
            // run constant u IsoCurve from Surface v Max to Min
            (uv01, uv00, uv11, uv10)
        };

        // make Surface constant uMin isoParam curve
        out.uv_curve1 = Some(Curve::from(BSplineCurve::create_line_segment(
            2,
            p3(start_uv0),
            p3(end_uv0),
        )?));

        // make Surface constant uMax isoParam curve
        out.uv_curve2 = Some(BSplineCurve::create_line_segment(
            2,
            p3(start_uv1),
            p3(end_uv1),
        )?);

        // all done
        return Ok(out);
    }

    // arrive here when desired UVTrimCurve is not known to sit on a Seam

    // Compute dropping tolerance - start with a tight tolerance and relax that as needed
    let bbox = surface
        .calculate_bounding_box(k, &uv_domain, [true, false, false], true)?
        .normal_box
        .ok_or_else(|| SolidError::null_pointer(OP_UV))?;
    let mut dropping_tol = 1.0e-9 * bbox.size().max_dimension();

    // Param tolerance
    let param_tol = (uv_domain.size().max_dimension() + 1.0) * EFF_ZERO;

    // the solutions name pSurface (SmFilletGeom.cpp:4461-4504), see UvSurfaceHandle
    let handle = UvSurfaceHandle(surface.natural_uv_domain_with_nurb(k)?);
    let solve = |pnt: Point3, op: SolverOperation, tol: f64, req: SolutionRequested| {
        surface.global_point_solve(
            k,
            Some(handle),
            &uv_domain,
            op,
            pnt,
            zone_tol3d(k, tol),
            None,
            req,
        )
    };

    // try increasing tolerance sized drops of Start/End Pts to surface
    let mut solutions1: SolutionArray<UvSurfaceHandle> = SolutionArray::new();
    let mut solutions2: SolutionArray<UvSurfaceHandle> = SolutionArray::new();
    let mut num_solutions1 = 0usize;
    let mut num_solutions2 = 0usize;
    for _ in 0..7 {
        // Drop the start point
        if num_solutions1 == 0 {
            solutions1 = solve(
                start_pnt,
                SolverOperation::Intersect,
                dropping_tol,
                SolutionRequested::All,
            )?;
        }

        // Drop the end point
        if num_solutions2 == 0 {
            solutions2 = solve(
                end_pnt,
                SolverOperation::Intersect,
                dropping_tol,
                SolutionRequested::All,
            )?;
        }

        // increase the drop tolerance
        dropping_tol *= 10.0;

        // quit iterating once both points drop to surface
        num_solutions1 = solutions1.len();
        num_solutions2 = solutions2.len();
        if num_solutions1 > 0 && num_solutions2 > 0 {
            break;
        }
    }

    // when DropPoint Intersection failed - try DropPoint Minimize
    if num_solutions1 == 0 {
        solutions1 = solve(
            start_pnt,
            SolverOperation::Minimize,
            approx_tol,
            SolutionRequested::Single,
        )?;
        num_solutions1 = solutions1.len();
    }
    if num_solutions2 == 0 {
        solutions2 = solve(
            end_pnt,
            SolverOperation::Minimize,
            approx_tol,
            SolutionRequested::Single,
        )?;
        num_solutions2 = solutions2.len();
    }

    // failure case: start or end pt failed to drop to surface
    if num_solutions1 == 0 || num_solutions2 == 0 {
        return Err(SolidError::sm_err(OP_UV));
    }

    // Discard solutions within dParamTol of the first one
    let dedupe = |sols: &mut SolutionArray<UvSurfaceHandle>, n: usize| -> Result<usize> {
        if n > 1 {
            let uv1 = solution_uv(sols, 0, OP_UV)?;
            for ii in (1..n).rev() {
                let uv2 = solution_uv(sols, ii, OP_UV)?;
                let param_dist = uv1.distance(uv2);
                if param_dist < param_tol {
                    sols.remove_at(ii, 1);
                }
            }
            return Ok(sols.len());
        }
        Ok(n)
    };
    num_solutions1 = dedupe(&mut solutions1, num_solutions1)?;
    num_solutions2 = dedupe(&mut solutions2, num_solutions2)?;

    // Sort out dual(or multi-) solution cases
    let mut start_uv = solution_uv(&solutions1, 0, OP_UV)?;
    let mut end_uv = solution_uv(&solutions2, 0, OP_UV)?;
    let mut start_uv1 = Point2::UNDEF;
    let mut end_uv1 = Point2::UNDEF;

    // when start point still dropped to surface multiple times
    if num_solutions1 > 1 {
        start_uv1 = solution_uv(&solutions1, 1, OP_UV)?;
        if num_solutions1 == 2 && num_solutions2 == 1 {
            // Start point is on seam but end point is not: take the solution closest
            // to sEndUV
            let dist_to_end = start_uv.distance(end_uv);
            let dist_to_end1 = start_uv1.distance(end_uv);
            if dist_to_end1 < dist_to_end {
                start_uv = start_uv1;
            }
            num_solutions1 = 1;
        }
    }

    // when end point still dropped to surface multiple times
    if num_solutions2 > 1 {
        end_uv1 = solution_uv(&solutions2, 1, OP_UV)?;
        if num_solutions2 == 2 && num_solutions1 == 1 {
            // Could be seam or degenerate cases: take the solution closest to sStartUV
            let dist_to_start = end_uv.distance(start_uv);
            let dist_to_start1 = end_uv1.distance(start_uv);
            if dist_to_start1 < dist_to_start {
                end_uv = end_uv1;
            }
            num_solutions2 = 1;
        }
    }

    // remember when Start and End Pts both dropped to a seam
    let make_two_curves = num_solutions1 >= 2 && num_solutions2 >= 2;

    // when start point still dropped to surface multiple times
    if num_solutions1 > 2 {
        if num_solutions2 > 2 {
            return Err(SolidError::sm_err(OP_UV)); // Should not happen!!
        }

        // Start point lies on degenerate edge
        if (start_uv.x - start_uv1.x).abs() < param_tol {
            start_uv.y = end_uv.y;
            if num_solutions2 == 2 {
                start_uv1.y = end_uv1.y;
            }
        } else if (start_uv.y - start_uv1.y).abs() < param_tol {
            start_uv.x = end_uv.x;
            if num_solutions2 == 2 {
                start_uv1.x = end_uv1.x;
            }
        } else {
            return Err(SolidError::sm_err(OP_UV));
        }
    }

    // when end point still dropped to surface multiple times
    if num_solutions2 > 2 {
        // End point lies on degenerate edge
        if (end_uv.x - end_uv1.x).abs() < param_tol {
            end_uv.y = start_uv.y;
            if num_solutions1 == 2 {
                end_uv1.y = start_uv1.y;
            }
        } else if (end_uv.y - end_uv1.y).abs() < param_tol {
            end_uv.x = start_uv.x;
            if num_solutions1 == 2 {
                end_uv1.x = start_uv1.x;
            }
        } else {
            return Err(SolidError::sm_err(OP_UV));
        }
    }

    // when needed - Make the second UVcurve
    if make_two_curves {
        let start_uv1 = cleanup_noise(start_uv1);
        let end_uv1 = cleanup_noise(end_uv1);
        out.uv_curve2 = Some(BSplineCurve::create_line_segment(
            2,
            p3(start_uv1),
            p3(end_uv1),
        )?);
    }

    // Now, make the first curve
    let start_uv = cleanup_noise(start_uv);
    let end_uv = cleanup_noise(end_uv);

    // when curve is not an iso-curve, intersect the surface with a plane
    let domain_size = uv_domain.size();
    if (start_uv.x - end_uv.x).abs() > 0.01 * domain_size.x
        && (start_uv.y - end_uv.y).abs() > 0.01 * domain_size.y
    {
        // SM_CAST_PTR(SmBSplineSurface, pSurface): upstream dereferences the result
        if b_spline_net(surface).is_none() {
            return Err(SolidError::null_pointer(OP_UV));
        }

        // StartPt to EndPt 3d unit-vector
        let mut dir = (end_pnt - start_pnt).unitize()?;

        // pick a likely ZAxis: the cone's axis, else a fillet vertex use's curve
        // derivative
        let mut z_axis = Vec3::new(0.0, 0.0, 0.0);
        if let SurfaceKind::Cone(cone) = surface.kind() {
            // SmCone_TYPE (an SmCylinder too): the cone's axis
            z_axis = cone.rev().position().z_axis();
        } else {
            // pick a likely ZAxis from the Start/EndFV->GetTsectPnt().CrvDeriv() values
            // find a fillet vertexuse to target
            let mut vu = None;
            let geoms = fb.vertex_fillet_geoms(start_fv)?;
            if geoms.len() == 1 {
                vu = fb.vu_at_rail_end(start_fv, geoms.first().copied(), None, None)?;
            } else {
                let geoms = fb.vertex_fillet_geoms(end_fv)?;
                if geoms.len() == 1 {
                    vu = fb.vu_at_rail_end(end_fv, geoms.first().copied(), None, None)?;
                }
            }
            // when a vertexuse was found
            if let Some(vu) = vu {
                z_axis = fb.fillet_vertexuse(vu)?.tsect_pnt().crv_deriv();
            }
        }

        // when ZAxis was set
        if z_axis.length() > EFF_ZERO_SQRT {
            // build plane with planeNormal perp to sDir in [sZAxis sDir] plane
            let view_vec = z_axis.cross(dir);
            let plane_normal = view_vec.cross(dir).unitize()?;
            let mut plane = Plane::from_normal(end_pnt, plane_normal);
            plane.make_nurb()?;
            let plane = Surface::from(plane);

            // surface intersection initial tol
            let mut ssi_tol = crate::geom::tol::approx_tol3d(k, approx_tol / 100.0);

            // iter a sequence of SrfSrfIntersections with increasing tolerance
            for _ in 0..4 {
                match srf_srf_intersection(
                    k,
                    fb,
                    Some(start_fv),
                    Some(end_fv),
                    Some(&mut dir),
                    None,
                    surface,
                    &plane,
                    ssi_tol,
                    30.0 * PI / 180.0,
                    false,
                    None,
                ) {
                    Ok(r) => {
                        // when Srf/Srf worked - don't need 2nd UVcurve
                        // (the out-params are `SmBSplineCurve *&`: the derived objects
                        // themselves, not copies of their B-spline bases)
                        out.curve_3d = r.curve_3d.filter(|c| as_b_spline(c).is_some());
                        out.uv_curve1 = r.uv_curve1.filter(|c| as_b_spline(c).is_some());
                        return Ok(out);
                    }
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => {}
                }

                // try Srf/Srf xsect again with larger tolerance
                ssi_tol = crate::geom::tol::tol_mul(ssi_tol, 10.0);
            }
        }
    }

    // Otherwise, make an iso-curve
    out.uv_curve1 = Some(Curve::from(BSplineCurve::create_line_segment(
        2,
        p3(start_uv),
        p3(end_uv),
    )?));

    // all done
    Ok(out)
}

// ---- vertex calculators -----------------------------------------------------------------

/// Which end of the 3D intersection curve `curve` the reference point is nearer to, and
/// the end points (`EvaluatePoint` at the natural interval's ends).
fn curve_ends(k: &Kernel, curve: &Curve) -> Result<(Extent1d, Point3, Point3)> {
    let ivl = curve.natural_interval();
    let pnt1 = curve.evaluate_point(k, ivl.min())?;
    let pnt2 = curve.evaluate_point(k, ivl.max())?;
    Ok((ivl, pnt1, pnt2))
}

impl FilletBrep {
    /// The fillet surface and solver tolerances of the fillet geometry of edge use
    /// `eu`.
    fn eu_geom_solver_with<'s>(
        &self,
        eu: EdgeuseId,
        context: &'s impl super::p5::FilletEdgeContext,
        op: &'static str,
    ) -> Result<(FilletGeomId, &'s FilletSolver)> {
        let g = self
            .fillet_edgeuse(eu)?
            .fillet_geom()
            .ok_or_else(|| SolidError::null_pointer(op))?;
        let sid = self
            .geom(g)?
            .fillet_solver()
            .ok_or_else(|| SolidError::null_pointer(op))?;
        Ok((g, context.solver(sid)?))
    }

    /// The primary edge use of `e` and its mate.
    fn prim_and_mate(&self, e: EdgeId, op: &'static str) -> Result<(EdgeuseId, EdgeuseId)> {
        let (prim, mate) = self.prim_and_opt_mate(e, op)?;
        Ok((prim, mate.ok_or_else(|| SolidError::null_pointer(op))?))
    }

    /// The primary edge use of `e` and its mate, if any (`pPrimEU->GetMate()`, which
    /// upstream fetches early but dereferences only where it is used).
    fn prim_and_opt_mate(
        &self,
        e: EdgeId,
        op: &'static str,
    ) -> Result<(EdgeuseId, Option<EdgeuseId>)> {
        let prim = primary_edgeuse(self.brep(), e)?.ok_or_else(|| SolidError::null_pointer(op))?;
        Ok((prim, eu1::mate(self.brep(), prim)?))
    }

    /// Stores a new intersection curve on fillet edge `e` (`SetCurve(curve, TRUE)`,
    /// `SetOwner`, `SetInterval(ivl)`) and its UV curves on the primary edge use and
    /// its mate (`SetUVCurve`).
    #[allow(clippy::too_many_arguments)]
    fn set_edge_curves(
        &mut self,
        e: EdgeId,
        prim: EdgeuseId,
        mate: EdgeuseId,
        curve: Curve,
        ivl: &Extent1d,
        uv_curve1: Option<Curve>,
        uv_curve2: Option<Curve>,
    ) -> Result<()> {
        set_edge_curve(self.brep_mut(), e, Some(curve))?;
        edge::set_interval(self.brep_mut(), e, ivl)?;
        self.set_edgeuse_uv_curve(prim, uv_curve1, true, true)?;
        self.set_edgeuse_uv_curve(mate, uv_curve2, true, true)?;
        Ok(())
    }

    /// `sm_CalcNx2VertFilletXFillet`: the vertex at the end of the intersection curve
    /// of the two fillets of an Nx2 concave corner whose cross-section edges meet at
    /// `fv`, the end nearer the processed vertex across the fillet/side-face edge.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when that other vertex is not processed, or when the intersection
    /// fails (also for a missing fillet surface, which `sm_SrfSrfIntersection` rejects;
    /// the vertex is then `SM_FIL_SURF_INT_FAILURE`); `SM_ERR_NULL_POINTER` for a
    /// cross-section edge without fillet geometry or solver, or a vertex without
    /// corner; [`SolidError::Unported`] from the intersection (`sm_SrfSrfIntersection`).
    pub fn calc_nx2_vert_fillet_x_fillet(
        &mut self,
        k: &Kernel,
        fv: VertexId,
        solvers: &[FilletSolver],
    ) -> Result<()> {
        self.calc_nx2_vert_fillet_x_fillet_with_context(
            k,
            fv,
            &super::p5::IndexedEdgeContext {
                solvers,
                corners: CornerLookup::default(),
                op: OP_NX2,
            },
        )
    }

    /// [`Self::calc_nx2_vert_fillet_x_fillet`] resolving the solvers through `context`.
    ///
    /// # Errors
    /// As [`Self::calc_nx2_vert_fillet_x_fillet`], with `context`'s resolution failures.
    pub fn calc_nx2_vert_fillet_x_fillet_with_context(
        &mut self,
        k: &Kernel,
        fv: VertexId,
        context: &impl super::p5::FilletEdgeContext,
    ) -> Result<()> {
        let mut ref_pnt = Point3::UNDEF;
        let mut approx_tol = BIG_DOUBLE;
        let mut ang_tol = BIG_DOUBLE;
        let mut fillet_surface1: Option<FilletGeomId> = None;
        let mut fillet_surface2: Option<FilletGeomId> = None;

        for e in vertex::edges(self.brep(), fv, None)? {
            match self.fillet_edge(e)?.fillet_edge_type() {
                FilletEdgeType::CrossSection => {
                    let prim = primary_edgeuse(self.brep(), e)?
                        .ok_or_else(|| SolidError::null_pointer(OP_NX2))?;
                    let (g, fs) = self.eu_geom_solver_with(prim, context, OP_NX2)?;
                    approx_tol = smin(approx_tol, fs.approx_tol_3d);
                    ang_tol = smin(ang_tol, fs.ang_tol_rad);
                    if fillet_surface1.is_none() {
                        fillet_surface1 = Some(g);
                    } else {
                        fillet_surface2 = Some(g);
                    }
                }
                FilletEdgeType::FilletXSideFace => {
                    let other_fv = edge::other_vertex(self.brep(), e, Some(fv))?
                        .ok_or_else(|| SolidError::null_pointer(OP_NX2))?;
                    if !self.fillet_vertex(other_fv)?.is_processed() {
                        return Err(SolidError::sm_err(OP_NX2));
                    }
                    ref_pnt = self.vertex_point(other_fv)?;
                }
                _ => {}
            }
        }

        // Do surface/surface intersection
        let ref_point = self.vertex_point(fv)?;
        // pFV->GetFilletCorner()->GetContext(): the kernel; the corner is dereferenced
        if self.fillet_vertex(fv)?.fillet_corner().is_none() {
            return Err(SolidError::null_pointer(OP_NX2));
        }
        // GetFilletSurface() of each geometry; NULL when no cross-section edge was
        // seen or the geometry has no fillet surface yet
        let surface_of = |g: Option<FilletGeomId>| -> Result<Option<&Surface>> {
            Ok(match g {
                Some(g) => self.geom(g)?.fillet_surface(None)?,
                None => None,
            })
        };
        let result = match (surface_of(fillet_surface1)?, surface_of(fillet_surface2)?) {
            (Some(s1), Some(s2)) => srf_srf_intersection(
                k,
                self,
                None,
                None,
                None,
                None,
                s1,
                s2,
                approx_tol,
                ang_tol,
                false,
                Some(ref_point),
            ),
            // sm_SrfSrfIntersection starts with NER(pSurf1); NER(pSurf2): a failed
            // intersection for the caller
            _ => Err(SolidError::null_pointer(OP_NX2)),
        };
        let curve_3d = match result {
            Ok(SrfSrfCurves {
                curve_3d: Some(c), ..
            }) => c,
            Err(e) if e.is_unported() => return Err(e),
            _ => {
                self.fillet_vertex_mut(fv)?
                    .set_status(FilStatus::SurfIntFailure);
                return Err(SolidError::sm_err(OP_NX2));
            }
        };

        let (_, pnt1, pnt2) = curve_ends(k, &curve_3d)?;
        let mut vert_geom = pnt1;
        if ref_pnt.distance(pnt1) > ref_pnt.distance(pnt2) {
            vert_geom = pnt2;
        }

        set_vertex_point(self.brep_mut(), fv, vert_geom)?;
        self.fillet_vertex_mut(fv)?.set_status(FilStatus::Processed);
        Ok(())
    }

    /// `SmFilletVertex::CalcFilletInt2Fillets`: the vertex where three fillets meet at
    /// a corner, as the common point of the intersection curves of the fillet pairs
    /// across its fillet/fillet edges (which get those curves).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without corner, fillet geometry or solver; `SM_ERR` when
    /// a neighbour vertex is not processed or the curves do not meet once;
    /// those of [`CornerLookup::corner`] (`corners` resolves the vertex's corner);
    /// [`SolidError::Unported`] from the intersection (`sm_SrfSrfIntersection`).
    pub fn calc_fillet_int_2_fillets(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<()> {
        self.calc_fillet_int_2_fillets_with_context(
            k,
            v,
            corners,
            &super::p5::IndexedEdgeContext {
                solvers,
                corners: CornerLookup::default(),
                op: OP_INT2,
            },
            target,
        )
    }

    /// [`Self::calc_fillet_int_2_fillets`] resolving the geometries' solvers through
    /// `context` (a live executive's retained solvers).
    ///
    /// # Errors
    /// As [`Self::calc_fillet_int_2_fillets`], with `context`'s resolution failures.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn calc_fillet_int_2_fillets_with_context(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        context: &impl super::p5::FilletEdgeContext,
        target: &Brep,
    ) -> Result<()> {
        let corner = self
            .fillet_vertex(v)?
            .fillet_corner()
            .ok_or_else(|| SolidError::null_pointer(OP_INT2))?;
        let filleted_vertex = corners.corner(corner)?.filleted_vertex();

        // Intersection curves of fillet #1 & #2 and of #2 & #3 (their edges)
        let mut curve1: Option<EdgeId> = None;
        let mut curve2: Option<EdgeId> = None;
        let mut min_tol = BIG_DOUBLE;

        for curr_edge in vertex::edges(self.brep(), v, None)? {
            if self.fillet_edge(curr_edge)?.fillet_edge_type() != FilletEdgeType::FilletXFillet {
                continue;
            }

            let other_fv = edge::other_vertex(self.brep(), curr_edge, Some(v))?
                .ok_or_else(|| SolidError::null_pointer(OP_INT2))?;
            if !self.fillet_vertex(other_fv)?.is_processed() {
                return Err(SolidError::sm_err(OP_INT2));
            }
            let other_pnt = self.vertex_point(other_fv)?;

            // Do surface/surface intersection
            let (prim, mate) = self.prim_and_mate(curr_edge, OP_INT2)?;
            let g1 = self
                .fillet_edgeuse(prim)?
                .fillet_geom()
                .ok_or_else(|| SolidError::null_pointer(OP_INT2))?;
            let g2 = self
                .fillet_edgeuse(mate)?
                .fillet_geom()
                .ok_or_else(|| SolidError::null_pointer(OP_INT2))?;
            let (Some(fillet_surface1), Some(fillet_surface2)) = (
                self.geom(g1)?.fillet_surface(None)?,
                self.geom(g2)?.fillet_surface(None)?,
            ) else {
                return Ok(()); // Fillet surfaces have not yet been computed
            };
            // The solvers are dereferenced only past the surface test (upstream
            // fetches the pointers before it but does not use them)
            let (_, fs1) = self.eu_geom_solver_with(prim, context, OP_INT2)?;
            let (_, fs2) = self.eu_geom_solver_with(mate, context, OP_INT2)?;

            let approx_tol = smin(fs1.approx_tol_3d, fs2.approx_tol_3d);
            let ang_tol = smin(fs1.ang_tol_rad, fs2.ang_tol_rad);
            if approx_tol < min_tol {
                min_tol = approx_tol;
            }
            let filleted_pnt = target
                .vertex(filleted_vertex.ok_or_else(|| SolidError::null_pointer(OP_INT2))?)?
                .point;
            let mut dir = filleted_pnt - other_pnt;
            let skip_two_pnts_intersection = true;

            let result = srf_srf_intersection(
                k,
                self,
                Some(other_fv),
                None,
                Some(&mut dir),
                None,
                fillet_surface1,
                fillet_surface2,
                approx_tol,
                ang_tol,
                skip_two_pnts_intersection,
                None,
            );
            let (mut new_3d_curve, uv_curve1, uv_curve2) = match result {
                Ok(SrfSrfCurves {
                    curve_3d: Some(c),
                    uv_curve1,
                    uv_curve2,
                }) => (c, uv_curve1, uv_curve2),
                Err(e) if e.is_unported() => return Err(e),
                _ => {
                    self.fillet_vertex_mut(v)?
                        .set_status(FilStatus::SurfIntFailure);
                    self.fillet_edge_mut(curr_edge)?
                        .set_status(FilStatus::SurfIntFailure);
                    return Err(SolidError::sm_err(OP_INT2));
                }
            };
            let (mut uv_curve1, mut uv_curve2) = (uv_curve1, uv_curve2);

            // Determine whether the curves need to be reversed (before they are stored:
            // upstream reverses the stored objects in place)
            let crv_ivl = new_3d_curve.natural_interval();
            let (mut ivl, pnt1, pnt2) = curve_ends(k, &new_3d_curve)?;
            let near_start = other_pnt.distance(pnt1) < other_pnt.distance(pnt2);
            let reverse_curves = if edge::vertex(self.brep(), curr_edge)? == Some(v) {
                near_start
            } else {
                !near_start
            };
            if reverse_curves {
                // the UV curves are dereferenced only here upstream
                ivl = new_3d_curve.reverse_parameterization(k, &ivl)?;
                ivl = uv_curve1
                    .as_mut()
                    .ok_or_else(|| SolidError::null_pointer(OP_INT2))?
                    .reverse_parameterization(k, &ivl)?;
                let _ = uv_curve2
                    .as_mut()
                    .ok_or_else(|| SolidError::null_pointer(OP_INT2))?
                    .reverse_parameterization(k, &ivl)?;
            }
            self.set_edge_curves(
                curr_edge,
                prim,
                mate,
                new_3d_curve,
                &crv_ivl,
                uv_curve1,
                uv_curve2,
            )?;

            if curve1.is_none() {
                curve1 = Some(curr_edge);
            } else if curve2.is_none() {
                curve2 = Some(curr_edge);
            }
        }

        // Now find the common intersection of all the curves.
        let curve_of = |e: Option<EdgeId>| -> Result<&Curve> {
            let e = e.ok_or_else(|| SolidError::null_pointer(OP_INT2))?;
            self.brep()
                .edge(e)?
                .curve
                .as_ref()
                .ok_or_else(|| SolidError::null_pointer(OP_INT2))
        };
        let c1 = curve_of(curve1)?;
        let c2 = curve_of(curve2)?;
        // the solutions name the two curves (`this` and `rCurve2`) and their analytic
        // replacements, as upstream's do: `IdenticalSolutions` merges near-duplicates
        // within `1e-4` of the curve's interval before the `!= 1` count below
        let (ivl1, ivl2) = (c1.natural_interval(), c2.natural_interval());
        let handle = |id: u8, ivl| Some(CurveObjectHandle { id, ivl });
        let solutions = c1.global_curve_intersect(
            k,
            &ivl1,
            c2,
            &ivl2,
            min_tol,
            &crate::curve::curve::p8::CurveIntersectOptions {
                skip_global_coincidence_check: false,
                handles: [handle(0, ivl1), handle(1, ivl2)],
                analytic_handles: [handle(2, ivl1), handle(3, ivl2)],
            },
        )?;
        if solutions.len() != 1 {
            return Err(SolidError::sm_err(OP_INT2));
        }
        let sol = solutions
            .get(0)
            .ok_or_else(|| SolidError::sm_err(OP_INT2))?;
        let vert_geom = c1.evaluate_point(k, sol.start.parameters[0])?;
        set_vertex_point(self.brep_mut(), v, vert_geom)?;
        self.fillet_vertex_mut(v)?.set_status(FilStatus::Processed);
        Ok(())
    }

    /// `SmFilletVertex::CalcRailIntExtendedEdgeuse`: the vertex where a fillet surface
    /// meets the (extended) surface of the side face across the vertex's
    /// fillet/side-face edge (which gets the intersection curve, oriented from its
    /// start vertex), or for a 3x2 mixed corner the end of that intersection lying on
    /// the face the vertex is on.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a missing edge of the wanted type, fillet geometry,
    /// side face, solver, corner or extended B-spline surface; `SM_ERR` when the
    /// neighbour vertex is not processed or no end lies on the face;
    /// those of [`CornerLookup::corner`] and [`CornerLookup::extended_surface`]
    /// (`corners` resolves the vertex's corner and holds the executive's extended
    /// surfaces); [`SolidError::Unported`] from the intersection
    /// (`sm_SrfSrfIntersection`). For a 3x2 mixed corner, those of the on-face's
    /// fast-drop trim test (`SmFace::PointClassify` on `target`, which may make the
    /// face's trimmed surface cache).
    pub fn calc_rail_int_extended_edgeuse(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &mut Brep,
    ) -> Result<()> {
        let context = super::p5::IndexedEdgeContext {
            solvers,
            corners,
            op: OP_EXT,
        };
        self.calc_rail_int_extended_edgeuse_with_context(k, v, corners, &context, target)
    }

    /// [`Self::calc_rail_int_extended_edgeuse`] resolving the geometry's solver and
    /// the side face's extended surface through the caller's live objects.
    ///
    /// # Errors
    /// As [`Self::calc_rail_int_extended_edgeuse`]; those of `context`.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn calc_rail_int_extended_edgeuse_with_context(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        context: &impl super::p5::FilletEdgeContext,
        target: &mut Brep,
    ) -> Result<()> {
        let corner = self.fillet_vertex(v)?.fillet_corner();
        let b3x2_mixed_corner_case = match corner {
            Some(c) => corners.corner(c)?.corner_type() == FilletCornerType::ThreeByTwoMixed,
            None => false,
        };

        // Find adjacent fillet edge which was classified
        let fe_type = if b3x2_mixed_corner_case {
            FilletEdgeType::CrossSection
        } else {
            FilletEdgeType::FilletXSideFace
        };
        let mut curr_edge = None;
        for e in vertex::edges(self.brep(), v, None)? {
            if self.fillet_edge(e)?.fillet_edge_type() == fe_type {
                curr_edge = Some(e);
                break;
            }
        }
        let curr_edge = curr_edge.ok_or_else(|| SolidError::null_pointer(OP_EXT))?;

        // get FilletSurface stored in filletEdge->PrimaryEU->FilletGeom->FilletSurface
        // pMateEU is dereferenced only by the final SetUVCurve
        let (prim, mate) = self.prim_and_opt_mate(curr_edge, OP_EXT)?;
        let fillet_geom = self
            .fillet_edgeuse(prim)?
            .fillet_geom()
            .ok_or_else(|| SolidError::null_pointer(OP_EXT))?;
        if !self.geom(fillet_geom)?.has_fillet_surface() {
            // Need fillet surface, will process it later
            return Ok(());
        }
        // pFilletGeom->GetFilletSolver(), dereferenced where upstream does
        let sid = self.geom(fillet_geom)?.fillet_solver();
        let fil_solver = || -> Result<&FilletSolver> {
            context.solver(sid.ok_or_else(|| SolidError::null_pointer(OP_EXT))?)
        };

        // get OtherFaceSurface
        let mut other_fv = None;
        let side_face;
        let mut on_face = None;
        if b3x2_mixed_corner_case {
            // Will intersect fillet with a side-face of the original brep
            let side_eu = match self.fillet_vertex(v)?.point_class_object() {
                Some(EntityRef::Edgeuse(eu)) => eu,
                None => return Err(SolidError::null_pointer(OP_EXT)),
                Some(_) => {
                    return Err(SolidError::invalid_input(
                        OP_EXT,
                        "classification object is not an edge use",
                    ))
                }
            };
            let mut sf = eu1::face(target, side_eu)?;
            let radial =
                eu1::radial(target, side_eu)?.ok_or_else(|| SolidError::null_pointer(OP_EXT))?;
            let mut of = eu1::face(target, radial)?;
            for kk in 0..2 {
                let eu = fil_solver()?
                    .edgeuse(kk)?
                    .ok_or_else(|| SolidError::null_pointer(OP_EXT))?;
                if eu1::face(target, eu)? == sf {
                    core::mem::swap(&mut sf, &mut of);
                    break;
                }
            }
            side_face = sf;
            on_face = of;
        } else {
            let ofv = edge::other_vertex(self.brep(), curr_edge, Some(v))?
                .ok_or_else(|| SolidError::null_pointer(OP_EXT))?;

            // check state - the other filletVertex is already processed
            if !self.fillet_vertex(ofv)?.is_processed() {
                return Err(SolidError::sm_err(OP_EXT));
            }
            other_fv = Some(ofv);

            // get face upon which this filletEdge was originated
            side_face = self.fillet_edge(curr_edge)?.original_face();
        }
        let side_face = side_face.ok_or_else(|| SolidError::null_pointer(OP_EXT))?;

        // get face->extendedSurface
        let corner = corner.ok_or_else(|| SolidError::null_pointer(OP_EXT))?;
        let surface2 = context
            .extended_surface(corners.corner(corner)?, side_face)?
            .filter(|s| b_spline_net(s).is_some())
            .ok_or_else(|| SolidError::null_pointer(OP_EXT))?;

        // locals surface/surface intersection
        let approx_tol = fil_solver()?.approx_tol_3d;
        let ang_tol = fil_solver()?.ang_tol_rad;
        let fillet_surface = self
            .geom(fillet_geom)?
            .fillet_surface(None)?
            .ok_or_else(|| SolidError::null_pointer(OP_EXT))?;

        let result = srf_srf_intersection(
            k,
            self,
            other_fv,
            None,
            None,
            None,
            fillet_surface,
            surface2,
            approx_tol,
            ang_tol,
            false,
            None,
        );
        let (mut new_3d_curve, uv_curve1, uv_curve2) = match result {
            Ok(SrfSrfCurves {
                curve_3d: Some(c),
                uv_curve1,
                uv_curve2,
            }) => (c, uv_curve1, uv_curve2),
            Err(e) if e.is_unported() => return Err(e),
            _ => {
                self.fillet_vertex_mut(v)?
                    .set_status(FilStatus::SurfIntFailure);
                self.fillet_edge_mut(curr_edge)?
                    .set_status(FilStatus::SurfIntFailure);
                return Err(SolidError::sm_err(OP_EXT));
            }
        };

        // get surface/surface xSectCurve->endPoints
        let (mut ivl, pnt1, pnt2) = curve_ends(k, &new_3d_curve)?;

        if b3x2_mixed_corner_case {
            // Find which end point lies on the pOnFace (the curves are dropped)
            let on_face = on_face.ok_or_else(|| SolidError::null_pointer(OP_EXT))?;
            // pOnFace->GetSurface(): the face owns it, so the fast drop's trim test
            // classifies on the face (`pFace->PointClassify`); the solve reads a copy
            // sharing the surface's cache slot
            let surf1 = target
                .face(on_face)?
                .surface
                .as_ref()
                .ok_or_else(|| SolidError::null_pointer(OP_EXT))?
                .clone_for_evaluation()?;
            let tol = fil_solver()?.approx_tol_3d;
            let domain = surf1.natural_uv_domain_with_nurb(k)?;
            for pnt in [pnt1, pnt2] {
                let solutions =
                    crate::intersect::point_class::live_owned_surface_global_point_solve(
                        k,
                        &surf1,
                        target,
                        on_face,
                        &domain,
                        SolverOperation::Intersect,
                        pnt,
                        zone_tol3d(k, 10.0 * tol),
                        SolutionRequested::All,
                    )?;
                if !solutions.is_empty() {
                    set_vertex_point(self.brep_mut(), v, pnt)?;
                    self.fillet_vertex_mut(v)?.set_status(FilStatus::Processed);
                    return Ok(());
                }
            }
            return Err(SolidError::sm_err(OP_EXT));
        }

        // arrive here for SM_FE_FILLET_X_SIDE_FACE cases
        let mut vert_geom = pnt1;
        let mut reverse_curves = false;
        let other_pnt =
            self.vertex_point(other_fv.ok_or_else(|| SolidError::null_pointer(OP_EXT))?)?;

        // make sure curve is parameterized from edge->StartVert to edge->EndVert
        let near_start = other_pnt.distance(pnt1) < other_pnt.distance(pnt2);
        if edge::vertex(self.brep(), curr_edge)? == Some(v) {
            if near_start {
                vert_geom = pnt2;
                reverse_curves = true;
            }
        } else if near_start {
            vert_geom = pnt2;
        } else {
            reverse_curves = true;
        }

        let (mut uv_curve1, mut uv_curve2) = (uv_curve1, uv_curve2);

        // reverse FilletEdge parameterization when needed (the UV curves are
        // dereferenced only here upstream)
        if reverse_curves {
            ivl = new_3d_curve.reverse_parameterization(k, &ivl)?;
            ivl = uv_curve1
                .as_mut()
                .ok_or_else(|| SolidError::null_pointer(OP_EXT))?
                .reverse_parameterization(k, &ivl)?;
            ivl = uv_curve2
                .as_mut()
                .ok_or_else(|| SolidError::null_pointer(OP_EXT))?
                .reverse_parameterization(k, &ivl)?;
        }

        // Set edge geometry (pMateEU->SetUVCurve dereferences the mate)
        let mate = mate.ok_or_else(|| SolidError::null_pointer(OP_EXT))?;
        self.set_edge_curves(
            curr_edge,
            prim,
            mate,
            new_3d_curve,
            &ivl,
            uv_curve1,
            uv_curve2,
        )?;

        // Set vertex geometry
        set_vertex_point(self.brep_mut(), v, vert_geom)?;
        self.fillet_vertex_mut(v)?.set_status(FilStatus::Processed);
        Ok(())
    }

    // ---- SmFilletGeom ----------------------------------------------------------------

    /// `SmFilletGeom::GetRailIndexOfVertex`: the index of the rail of geometry `g`
    /// with a rail vertex use on `fvtx` (via `GetRailIndexOfVertexuse`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a `None` vertex or a missing rail vertex use (upstream
    /// dereferences it); `SM_ERR` from `GetRailIndexOfVertexuse` when no rail end
    /// matches.
    pub fn rail_index_of_vertex(&self, g: FilletGeomId, fvtx: Option<VertexId>) -> Result<usize> {
        let fvtx = fvtx.ok_or_else(|| SolidError::null_pointer(OP_RAIL_IDX))?;

        // Find the FilletVertexuse of this FilletVertex
        let mut fvu_to_use = None;
        'outer: for i in 0..2 {
            for j in 0..2 {
                let fvu_tmp = self
                    .rail_vertexuse(g, i, j)?
                    .ok_or_else(|| SolidError::null_pointer(OP_RAIL_IDX))?;
                if self.brep().vertexuse(fvu_tmp)?.vertex == Some(fvtx) {
                    fvu_to_use = Some(fvu_tmp);
                    break 'outer;
                }
            }
        }

        self.rail_index_of_vertexuse(g, fvu_to_use)
    }

    /// `SmFilletGeom::MakeFaceBrep`: makes the face of geometry `g` in `brep` (the
    /// fillet B-rep) on its fillet surface, bounded by the min-v rail(s), the side
    /// curves to the max-v rail(s), those reversed and the side curves back (gaps
    /// capped when the side curves do not connect); with UV curves
    /// (`MakeFaceWithCurves`) unless the geometry is a cliff side patch
    /// (`TrimSurfaceWithModelSpaceCurves`). Sets the geometry's capped flags. `env`
    /// carries the notifications the face makers send to `brep`'s entities. This
    /// indexed value API retains its historical surface-copy backend; shared surface
    /// binding requires [`Self::make_face_brep_with_solver`].
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without fillet surface, solver, rails, edge uses or UV
    /// curves; those of the evaluations, `JoinTwoFilletVertsByCurves`, the capping and
    /// the face makers.
    pub fn make_face_brep(
        &mut self,
        k: &mut Kernel,
        g: FilletGeomId,
        solvers: &[FilletSolver],
        brep: &mut Brep,
        env: &mut edge::NotifyEnv<'_>,
    ) -> Result<()> {
        self.make_face_brep_resolved(
            k,
            g,
            &|id, op| solver_of(solvers, id, op),
            brep,
            env,
            FaceSurfaceBackend::IndexedValue,
        )
    }

    /// Build a face using the actual retained solver without a copied base table.
    ///
    /// # Errors
    /// As [`Self::make_face_brep`], or a geometry naming a different solver.
    pub fn make_face_brep_with_solver(
        &mut self,
        k: &mut Kernel,
        g: FilletGeomId,
        solver_id: FilletSolverId,
        solver: &FilletSolver,
        brep: &mut Brep,
        env: &mut edge::NotifyEnv<'_>,
    ) -> Result<()> {
        self.make_face_brep_resolved(
            k,
            g,
            &|id, op| {
                if id == solver_id {
                    Ok(solver)
                } else {
                    Err(SolidError::invalid_input(op, "unresolved geometry solver"))
                }
            },
            brep,
            env,
            FaceSurfaceBackend::RetainedTransfer,
        )
    }

    #[allow(clippy::too_many_lines)] // one upstream routine
    fn make_face_brep_resolved<'s>(
        &mut self,
        k: &mut Kernel,
        g: FilletGeomId,
        resolve: &impl Fn(FilletSolverId, &'static str) -> Result<&'s FilletSolver>,
        brep: &mut Brep,
        env: &mut edge::NotifyEnv<'_>,
        surface_backend: FaceSurfaceBackend,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_FACE);

        // check state - must have a FilletSurface
        let uv_domain = self
            .geom(g)?
            .fillet_surface(None)?
            .ok_or_else(nul)?
            .natural_uv_domain_with_nurb(k)?;
        let (geom_type, solver_id, rails, other_rails1, other_rails2, side_eus) = {
            let geom = self.geom(g)?;
            (
                geom.fillet_geom_type(),
                geom.fillet_solver(),
                geom.rails,
                geom.other_rails1.clone(),
                geom.other_rails2.clone(),
                geom.side_eus.clone(),
            )
        };

        // make 1st loop edge the MinV rail curve
        let mut start_rail_indx = 0usize;
        let mut use_uv_curves = false;

        if geom_type != FilletGeomType::CliffSidePatch {
            // UVTrimCurves are available - work in cheaper UVSpace
            use_uv_curves = true;

            // See which rail is at low-v.  Check the midpoint of each.
            let mut test_uv = [Point3::UNDEF; 2];
            for (ii, slot) in test_uv.iter_mut().enumerate() {
                let this_rail = rails.get(ii).copied().flatten().ok_or_else(nul)?;
                let prim = primary_edgeuse(self.brep(), this_rail)?.ok_or_else(nul)?;
                let uv_curve = eu1::uv_trim_curve(self.brep(), prim)?.0.ok_or_else(nul)?;
                let t = self.brep().edge(this_rail)?.interval.evaluate(0.5);
                *slot = uv_curve.evaluate_point(k, t)?;
            }
            // (sTestUV[0].y < sTestUV[1].y) ? 0 : 1
            start_rail_indx = usize::from(test_uv[0].y >= test_uv[1].y);
        }

        // pick 2d or 3d tolerance depending on bUseUVCurve
        let sid = solver_id.ok_or_else(nul)?;
        // SmApproxTol3d sApproxTol3d = GetThisApproxTol3d(): USE_DEFAULT, then fabs
        let approx_tol_3d = crate::geom::tol::approx_tol3d(k, resolve(sid, OP_FACE)?.approx_tol_3d);
        let d3d_tol = approx_tol_3d;
        let mut uv_tol = d3d_tol;
        if use_uv_curves {
            let uv = uv_domain.mid();
            let uv_dir = Vec2::new(1.0, 0.0);
            uv_tol = map_to_2d_dir(
                k,
                approx_tol_3d,
                uv,
                uv_dir,
                self.geom(g)?.fillet_surface(None)?.ok_or_else(nul)?,
            )?;
        }

        let rail_curve = |fb: &Self, e: EdgeId| -> Result<Curve> {
            fb.brep().edge(e)?.curve.clone().ok_or_else(nul)
        };
        let eu_uv_curve = |fb: &Self, eu: EdgeuseId| -> Result<Option<Curve>> {
            Ok(eu1::uv_trim_curve(fb.brep(), eu)?.0.cloned())
        };

        let mut lists1 = CurveLists::default();
        let mut lists2 = CurveLists::default();

        // First the start rail:
        let rail0 = rails
            .get(start_rail_indx)
            .copied()
            .flatten()
            .ok_or_else(nul)?;
        let mut prim = primary_edgeuse(self.brep(), rail0)?.ok_or_else(nul)?;
        let uv_curve0 = if use_uv_curves {
            eu_uv_curve(self, prim)?
        } else {
            None
        };
        let other_rails = if start_rail_indx == 1 {
            &other_rails2
        } else {
            &other_rails1
        };

        lists1.curves_3d.push(Some(rail_curve(self, rail0)?));
        lists1.uv_curves.push(uv_curve0);
        lists1.orients.push(OrientType::Same);

        // Get current start & end verts (in the correct order).
        let (start_fv0, mut end_fv0);
        if self.brep().edgeuse(prim)?.orientation == OrientType::Opposite {
            end_fv0 = edge::vertex(self.brep(), rail0)?;
            // (upstream passes the still-NULL start vertex)
            start_fv0 = edge::other_vertex(self.brep(), rail0, None)?;
        } else {
            start_fv0 = edge::vertex(self.brep(), rail0)?;
            end_fv0 = edge::other_vertex(self.brep(), rail0, start_fv0)?;
        }

        // Next Add MinV OtherRails
        let total = other_rails.len();
        for (ii, &other_rail) in other_rails.iter().enumerate() {
            prim = primary_edgeuse(self.brep(), other_rail)?.ok_or_else(nul)?;
            lists1.curves_3d.push(Some(rail_curve(self, other_rail)?));
            lists1.uv_curves.push(eu_uv_curve(self, prim)?);
            lists1.orients.push(OrientType::Same);

            // for the last MinV Rail edge - update the pEndFV0 value
            if ii + 1 == total {
                let first = edge::vertex(self.brep(), other_rail)?;
                end_fv0 = edge::other_vertex(self.brep(), other_rail, first)?;
            }
        }

        // Load MaxV rail(s) into OrderedEdges List2
        let rail1 = rails
            .get(1 - start_rail_indx)
            .copied()
            .flatten()
            .ok_or_else(nul)?;
        let mut start_fv1 = edge::vertex(self.brep(), rail1)?;
        let mut end_fv1 = edge::other_vertex(self.brep(), rail1, start_fv1)?;
        let other_rails1 = if start_rail_indx == 1 {
            &other_rails1
        } else {
            &other_rails2
        };

        // For this side go backwards
        for (ii, &other_rail) in other_rails1.iter().enumerate().rev() {
            prim = primary_edgeuse(self.brep(), other_rail)?.ok_or_else(nul)?;
            lists2.curves_3d.push(Some(rail_curve(self, other_rail)?));
            lists2.uv_curves.push(eu_uv_curve(self, prim)?);
            lists2.orients.push(OrientType::Opposite);

            // for the first edge - update the EndVertex value
            if ii == 0 {
                let first = edge::vertex(self.brep(), other_rail)?;
                end_fv1 = edge::other_vertex(self.brep(), other_rail, first)?;
            }
        }

        // Finally load the MaxV rail into OrderedEdge List2.
        prim = primary_edgeuse(self.brep(), rail1)?.ok_or_else(nul)?;
        lists2.curves_3d.push(Some(rail_curve(self, rail1)?));
        lists2.uv_curves.push(eu_uv_curve(self, prim)?);
        lists2.orients.push(OrientType::Opposite);

        // order the MaxV Rail vertices
        if self.brep().edgeuse(prim)?.orientation == OrientType::Opposite {
            core::mem::swap(&mut start_fv1, &mut end_fv1);
        }

        // Add a sequence of m_vSideEUs->Curves to traverse from pEndFV0 to pEndFV1
        let made_connection = join_two_fillet_verts_by_curves_resolved(
            k,
            self,
            g,
            end_fv0,
            end_fv1,
            &side_eus,
            uv_tol,
            &mut lists1.curves_3d,
            &mut lists1.uv_curves,
            &mut lists1.orients,
            resolve,
        )?;

        // when railCurveEnd/FilletEdgeStart has too big a gap: cap it
        self.geom_mut(g)?.capped[1] = false;
        if use_uv_curves && !made_connection {
            self.geom_mut(g)?.capped[1] = true;
            let last = lists1.uv_curves.last().cloned();
            let first = lists2.uv_curves.first().cloned();
            let mut caps = CurveLists::default();
            Self::cap_gap(
                k,
                approx_tol_3d,
                self.geom(g)?.fillet_surface(None)?.ok_or_else(nul)?,
                [
                    (last.as_ref(), OrientType::Same),
                    (first.as_ref(), OrientType::Same),
                ],
                &mut caps,
            )?;
            lists1.curves_3d.append(&mut caps.curves_3d);
            lists1.uv_curves.append(&mut caps.uv_curves);
            lists1.orients.append(&mut caps.orients);
        }

        // Add MaxV rail sequence
        lists1.curves_3d.append(&mut lists2.curves_3d);
        lists1.uv_curves.append(&mut lists2.uv_curves);
        lists1.orients.append(&mut lists2.orients);

        // Add in m_vSideEUs Edge sequence to traverse from pStartFV1 to pStartFV0
        let made_connection = join_two_fillet_verts_by_curves_resolved(
            k,
            self,
            g,
            start_fv1,
            start_fv0,
            &side_eus,
            uv_tol,
            &mut lists1.curves_3d,
            &mut lists1.uv_curves,
            &mut lists1.orients,
            resolve,
        )?;

        self.geom_mut(g)?.capped[0] = false;
        if use_uv_curves && !made_connection {
            self.geom_mut(g)?.capped[0] = true;
            let last = lists1.uv_curves.last().cloned();
            let first = lists1.uv_curves.first().cloned();
            let mut caps = CurveLists::default();
            Self::cap_gap(
                k,
                approx_tol_3d,
                self.geom(g)?.fillet_surface(None)?.ok_or_else(nul)?,
                [
                    (last.as_ref(), OrientType::Opposite),
                    (first.as_ref(), OrientType::Opposite),
                ],
                &mut caps,
            )?;
            lists1.curves_3d.append(&mut caps.curves_3d);
            lists1.uv_curves.append(&mut caps.uv_curves);
            lists1.orients.append(&mut caps.orients);
        }

        // remove any degenerate curves added to the curve sequence
        for m in (0..lists1.curves_3d.len()).rev() {
            let degenerate = lists1
                .curves_3d
                .get(m)
                .and_then(Option::as_ref)
                .ok_or_else(nul)?
                .is_degenerate(k, EFF_ZERO, None)?;
            if degenerate {
                lists1.curves_3d.remove(m);
                lists1.uv_curves.remove(m);
                lists1.orients.remove(m);
            }
        }

        // set the number of curves forming this filletSurfaces boundary loop
        let curve_loops = vec![lists1.curves_3d.len()];

        // turn off UseUVCurves if any UVCurve array member is set to NULL
        if lists1.uv_curves.iter().any(Option::is_none) {
            use_uv_curves = false;
        }

        // Make a copy of each 3D & UV curve
        let mut curves_3d = Vec::with_capacity(lists1.curves_3d.len());
        let mut uv_curves = Vec::with_capacity(lists1.uv_curves.len());
        for (c, uv) in lists1.curves_3d.iter().zip(&lists1.uv_curves) {
            let c = c.as_ref().ok_or_else(nul)?;
            as_b_spline(c).ok_or_else(nul)?;
            let copy = c.copy(k)?;
            as_b_spline(&copy).ok_or_else(nul)?;
            curves_3d.push(copy);

            // skip copying uvTrimCurves when not using them
            if use_uv_curves {
                uv_curves.push(uv.clone().ok_or_else(nul)?);
            }
        }

        // Make Face: transfer the original surface only at the native call boundary.
        let region = brep.infinite_region();
        if matches!(surface_backend, FaceSurfaceBackend::IndexedValue) {
            // Preserve the existing indexed value contract, including its cliff path.
            // This API does not provide shared source-surface/managed-face binding.
            let surface = self.geom(g)?.fillet_surface(None)?.ok_or_else(nul)?.clone();
            if use_uv_curves {
                let input = FaceCurvesInput {
                    loops: curve_loops,
                    curves_3d: Some(curves_3d.into_iter().map(Some).collect()),
                    uv_curves: Some(uv_curves),
                    orientations: lists1.orients,
                    loop_points: Vec::new(),
                };
                let key_of = env.key_of;
                make_face_with_curves(
                    k,
                    brep,
                    region,
                    input,
                    surface,
                    &uv_domain,
                    OrientType::Same,
                    None,
                    &|face| key_of(EntityRef::Face(face)),
                    env,
                )?;
            } else {
                let input = TrimInput {
                    loop_counts: curve_loops,
                    curves: curves_3d,
                    orientations: lists1.orients,
                    loop_points: Vec::new(),
                    surface,
                    uv_domain,
                    surface_orientation: OrientType::Same,
                };
                trim_surface_with_model_space_curves(
                    k, brep, region, d3d_tol, input, true, false, false, env,
                )?;
            }
            return Ok(());
        }
        if use_uv_curves {
            let input = FaceCurvesInput {
                loops: curve_loops,
                curves_3d: Some(curves_3d.into_iter().map(Some).collect()),
                uv_curves: Some(uv_curves),
                orientations: lists1.orients,
                loop_points: Vec::new(),
            };
            let key_of = env.key_of;
            let face_ref = |f: FaceId| key_of(EntityRef::Face(f));
            let owned = self.geom_mut(g)?.fillet_surface.take().ok_or_else(nul)?;
            let super::p1::FilletSurfaceStorage::Owned(surface) = owned else {
                // Earlier checked reads already reject a transferred surface.
                self.geom_mut(g)?.fillet_surface = Some(owned);
                return Err(SolidError::invalid_input(
                    OP_FACE,
                    "surface already transferred",
                ));
            };
            let mut transfer = crate::topology::brep::p6::FaceSurfaceTransfer::new(*surface);
            let status = make_face_with_curves_transfer(
                k,
                brep,
                region,
                input,
                &mut transfer,
                &uv_domain,
                OrientType::Same,
                None,
                &face_ref,
                env,
                &mut NewFace::default(),
            );
            let installed = transfer.installed_face();
            let location = match transfer.into_location()? {
                crate::topology::brep::p6::FaceSurfaceLocation::Owned(surface) => {
                    super::p1::FilletSurfaceStorage::Owned(surface)
                }
                crate::topology::brep::p6::FaceSurfaceLocation::Installed(face) => {
                    debug_assert_eq!(installed, Some(face));
                    super::p1::FilletSurfaceStorage::ManagedFace {
                        body: key_of(EntityRef::Brep(crate::topology::BrepId::from_index(0))),
                        face,
                    }
                }
            };
            // Preserve the actual storage location even when construction failed.
            self.geom_mut(g)?.fillet_surface = Some(location);
            status?;
        } else {
            use crate::topology::trimming_tools::{
                trim_surface_with_model_space_curves_transfer, TrimCurvesInput,
                TrimSurfaceLocation, TrimSurfaceTransfer,
            };

            let input = TrimCurvesInput {
                loop_counts: curve_loops,
                curves: curves_3d,
                orientations: lists1.orients,
                loop_points: Vec::new(),
                uv_domain,
                surface_orientation: OrientType::Same,
            };
            let owned = self.geom_mut(g)?.fillet_surface.take().ok_or_else(nul)?;
            let super::p1::FilletSurfaceStorage::Owned(surface) = owned else {
                self.geom_mut(g)?.fillet_surface = Some(owned);
                return Err(SolidError::invalid_input(
                    OP_FACE,
                    "surface already transferred",
                ));
            };
            let mut transfer = TrimSurfaceTransfer::new(*surface);
            let status = trim_surface_with_model_space_curves_transfer(
                k,
                brep,
                region,
                d3d_tol,
                input,
                &mut transfer,
                env,
            );
            self.geom_mut(g)?.fillet_surface = match transfer.into_location()? {
                TrimSurfaceLocation::Owned(surface) => {
                    Some(super::p1::FilletSurfaceStorage::Owned(surface))
                }
                TrimSurfaceLocation::Installed(face) => {
                    Some(super::p1::FilletSurfaceStorage::ManagedFace {
                        body: (env.key_of)(EntityRef::Brep(crate::topology::BrepId::from_index(0))),
                        face,
                    })
                }
                TrimSurfaceLocation::Destroyed => None,
            };
            status?;
        }

        Ok(())
    }

    /// `JoinVertsByCapping` between two UV curves of the loop being built (upstream
    /// dereferences a `NULL` curve), appending the caps to `caps`.
    fn cap_gap(
        k: &Kernel,
        approx_tol_3d: f64,
        fillet_surface: &Surface,
        ends: [(Option<&Option<Curve>>, OrientType); 2],
        caps: &mut CurveLists,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_FACE);
        let [(c1, o1), (c2, o2)] = ends;
        let c1 = c1.and_then(Option::as_ref).ok_or_else(nul)?;
        let c2 = c2.and_then(Option::as_ref).ok_or_else(nul)?;
        let mut curves_3d = Vec::new();
        let mut uv_curves = Vec::new();
        let mut orients = Vec::new();
        join_verts_by_capping(
            k,
            approx_tol_3d,
            fillet_surface,
            c1,
            o1,
            c2,
            o2,
            &mut curves_3d,
            &mut uv_curves,
            &mut orients,
        )?;
        caps.curves_3d.extend(curves_3d.into_iter().map(Some));
        caps.uv_curves.extend(uv_curves.into_iter().map(Some));
        caps.orients.append(&mut orients);
        Ok(())
    }

    /// `SmFilletGeom::InsertIntersectionTopology`: merges each rail and each side curve
    /// of geometry `g` into its original face of the target B-rep through the fillet
    /// executive's topology intersector (`context`, the live executive's merge state),
    /// and registers the (empty) intersections of the fillet surface with those faces
    /// and with the faces around the filleted edge's uncapped vertices.
    ///
    /// `solver` is the geometry's actual retained solver; the context preserves
    /// surface identity and scoped marker lifetimes.
    ///
    /// # Errors
    /// Native insertion failures and missing live capabilities, with preceding
    /// topology and marker/error mutations retained.
    #[allow(clippy::too_many_lines)] // native ordered insertion routine
    pub fn insert_intersection_topology_with_context(
        &mut self,
        g: FilletGeomId,
        solver: &FilletSolver,
        target: &mut Brep,
        context: &mut impl IntersectionTopologyContext,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_INSERT);
        let Some(solver_id) = self.geom(g)?.fillet_solver() else {
            return Err(nul());
        };
        if solver.executive.is_none() {
            return Err(nul());
        }
        context.begin_intersection(solver)?;
        let fillet_surface = context.surface_binding(self, g)?;
        let mut deleted_edges = Vec::new();

        for ii in 0..2 {
            let geom = self.geom(g)?;
            let mut rails = vec![geom.rail(ii).ok_or_else(nul)?];
            rails.extend_from_slice(if ii == 0 {
                &geom.other_rails1
            } else {
                &geom.other_rails2
            });
            for rail in rails {
                let face = self.fillet_edge(rail)?.original_face();
                let managed_edge = self.fillet_edge(rail)?.fillet_brep_edge();
                let users = context.attribute_edges()?;
                let curve = self.brep().edge(rail)?.curve.as_ref().ok_or_else(nul)?;
                // Native evaluates degeneracy before the nullable face/edge checks.
                if curve.is_degenerate(context.kernel(), EFF_ZERO, None)? {
                    continue;
                }
                let (Some(face), Some(managed_edge)) = (face, managed_edge) else {
                    continue;
                };
                if !users.contains(&managed_edge) {
                    continue;
                }
                context.with_rail_base(target, solver_id, ii, |context, target| {
                    context.with_face_scope(
                        target,
                        face,
                        IntersectionFaceRole::Rail,
                        |context, target, marker| {
                            let tolerance = context.managed_edge_tolerance(managed_edge)?;
                            let merged = context.merge_curve(
                                self,
                                target,
                                face,
                                tolerance,
                                rail,
                                managed_edge,
                                &mut deleted_edges,
                            );
                            self.fillet_edge_mut(rail)?
                                .clear_fillet_brep_edges(&deleted_edges);
                            context.update_topology(self, target, marker, face)?;
                            if let Err(error) = merged {
                                if error.is_unported() {
                                    return Err(error);
                                }
                                if let Some(edgeuse) = solver.edgeuse(0)? {
                                    context.record_bad_merge(
                                        self,
                                        g,
                                        eu1::edge(target, edgeuse)?,
                                        target,
                                        face,
                                        rail,
                                        &fillet_surface,
                                    )?;
                                }
                                return Err(error);
                            }
                            context.register_intersection(self, target, face, &fillet_surface)
                        },
                    )
                })?;
            }
        }
        let side_eus = self.geom(g)?.side_eus.clone();
        for side_eu in side_eus {
            let edge = eu1::edge(self.brep(), side_eu)?.ok_or_else(nul)?;
            let face = self.fillet_edge(edge)?.original_face();
            let managed_edge = self.fillet_edge(edge)?.fillet_brep_edge();
            let users = context.attribute_edges()?;
            let Some(face) = face else {
                continue;
            };
            let Some(managed_edge) = managed_edge.filter(|edge| users.contains(edge)) else {
                continue;
            };
            context.with_face_scope(
                target,
                face,
                IntersectionFaceRole::Side,
                |context, target, marker| {
                    let tolerance = context.managed_edge_tolerance(managed_edge)?;
                    context.merge_curve(
                        self,
                        target,
                        face,
                        tolerance,
                        edge,
                        managed_edge,
                        &mut deleted_edges,
                    )?;
                    self.fillet_edge_mut(edge)?
                        .clear_fillet_brep_edges(&deleted_edges);
                    context.update_topology(self, target, marker, face)?;
                    context.register_intersection(self, target, face, &fillet_surface)
                },
            )?;
        }
        let mut vertex = None;
        for ii in 0..2 {
            let previous = vertex;
            vertex = solver.vertex(target, ii)?;
            if vertex == previous {
                continue;
            }
            if let Some(vertex) = vertex {
                if !self.geom(g)?.capped.get(ii).copied().unwrap_or(false) {
                    for face in vertex::faces(target, vertex)? {
                        context.register_intersection(self, target, face, &fillet_surface)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `SmFilletGeom::CalcBlendingGeom`: the surface of a blending geometry: both
    /// blending rails are recomputed, resampled at ten arc-length steps in their UV
    /// curves and lifted again; then a ruled surface between the rails for a linear
    /// cross-section, else a 4-sided corner blend over the start cross-section, rail
    /// 0, the end cross-section and rail 1.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a rail that is not a blending rail; `SM_ERR_NULL_POINTER` for a
    /// missing solver, rail, curve, UV curve, face, surface or cross-section edge;
    /// those of the length, arc-length, interpolation, lift and surface creations and
    /// of `SmFilletEdge::CalcBlendingRail`.
    pub fn calc_blending_geom(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        solvers: &[FilletSolver],
        target: &mut Brep,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_BLEND);
        let sid = self.geom(g)?.fillet_solver().ok_or_else(nul)?;
        let fil_solver = solver_of(solvers, sid, OP_BLEND)?;
        self.calc_blending_geom_of_solver(k, g, fil_solver, target)
    }

    /// [`FilletBrep::calc_blending_geom`] with the geometry's solver
    /// (`GetFilletSolver()`) given directly: for a caller that is that solver (the
    /// fillet solver's `CalcFilletGeom`, unit `blend.fillet_solver_p4`, whose
    /// geometries all name it). A geometry without a solver is still
    /// `SM_ERR_NULL_POINTER`.
    ///
    /// # Errors
    ///
    /// As [`FilletBrep::calc_blending_geom`].
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn calc_blending_geom_of_solver(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        fil_solver: &FilletSolver,
        target: &mut Brep,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_BLEND);
        if self.geom(g)?.fillet_solver().is_none() {
            return Err(nul());
        }
        let approx_tol_3d = fil_solver.approx_tol_3d;
        let tangency_tol_radians = fil_solver.tangency_tolerance;

        // Recreate each rail curve by sampling points evenly spaced in arc length
        // (m_vRails[i] is read as the loop reaches it: a failure on rail 0 comes first)
        for i in 0..2 {
            let rail = self.geom(g)?.rail(i).ok_or_else(nul)?;
            if self.fillet_edge(rail)?.fillet_edge_type() != FilletEdgeType::BlendingRail {
                return Err(SolidError::sm_err(OP_BLEND));
            }
            self.calc_blending_rail(k, rail, target)?;
            let rail_curve = self.brep().edge(rail)?.curve.clone().ok_or_else(nul)?;
            as_b_spline(&rail_curve).ok_or_else(nul)?;
            let (_, mate) = self.prim_and_mate(rail, OP_BLEND)?;
            let uv_curve = eu1::uv_trim_curve(self.brep(), mate)?
                .0
                .cloned()
                .ok_or_else(nul)?;
            let face = self.fillet_edge(rail)?.original_face().ok_or_else(nul)?;
            // the face's own surface (evaluated as a copy: the lift's trimmed surface
            // cache is made on `target` through the face, as upstream's GetFace())
            let base_surf = &target
                .face(face)?
                .surface
                .as_ref()
                .ok_or_else(nul)?
                .clone_for_evaluation()?;
            let ivl = rail_curve.natural_interval();
            let accuracy = approx_tol_3d / 10.0;
            let crv_len = rail_curve.length(k, &ivl, accuracy)?;

            // Step along this rail curve, at points evenly spaced in arc length.
            let total_samples = 10u32;
            let step = crv_len / f64::from(total_samples);
            let mut start_param = ivl.min();
            let mut uv_points = vec![uv_curve.evaluate_point(k, start_param)?];
            for _ in 1..total_samples {
                // Find param on curve at arc length dStep from previous point.
                let end_param = rail_curve
                    .find_parameter_at_arc_length(k, start_param, step)?
                    .end_param;
                start_param = end_param;

                // Find uv from the corresponding uv curve.
                uv_points.push(uv_curve.evaluate_point(k, start_param)?);
            }

            // Get the final point.
            uv_points.push(uv_curve.evaluate_point(k, ivl.max())?);

            // pRail->SetCurve(NULL, TRUE): deletes the curve and the UV trim curves
            set_edge_curve(self.brep_mut(), rail, None)?;
            let new_uv_curve = BSplineCurve::create_interpolating_curve(
                ParamMethod::ChordLength,
                2,
                2,
                &uv_points,
                &[],
                None,
                true,
            )?;

            // Lift it to 3D
            let uv_domain = base_surf.natural_uv_domain_with_nurb(k)?;
            let face_tol = crate::topology::face::p2::tolerance(target, face)?;
            let lifted = base_surf.lift_curve_with_context(
                k,
                &uv_domain,
                &Curve::from(new_uv_curve.clone()),
                &ivl,
                crate::geom::tol::approx_tol3d(k, face_tol),
                false,
                &mut BrepFaceServices::new(target),
                Some(CacheObject::Surface(face)),
            )?;
            let crv_ivl = lifted.curve.natural_interval();
            set_edge_curve(self.brep_mut(), rail, Some(lifted.curve))?;
            edge::set_interval(self.brep_mut(), rail, &crv_ivl)?;
            self.set_edgeuse_uv_curve(mate, Some(Curve::from(new_uv_curve)), true, true)?;
        }

        // pRail1, pRail2 (both processed above)
        let rail1 = self.geom(g)?.rail(0).ok_or_else(nul)?;
        let rail2 = self.geom(g)?.rail(1).ok_or_else(nul)?;

        // Create ruled surface if we have linear cross sections
        let fsg = fil_solver.fsg.ok_or_else(nul)?;
        if fsg.fillet_surface_generator_type() == FilletSurfaceGeneratorType::Linear {
            let c1 = self.brep().edge(rail1)?.curve.as_ref().ok_or_else(nul)?;
            let c2 = self.brep().edge(rail2)?.curve.as_ref().ok_or_else(nul)?;
            let ruled = BSplineSurface::create_ruled_surface(
                as_b_spline(c1).ok_or_else(nul)?,
                as_b_spline(c2).ok_or_else(nul)?,
                SurfParamType::V,
            )?;
            self.geom_mut(g)?
                .set_fillet_surface(Some(Surface::from(ruled)));
            return Ok(());
        }

        // Find 2 side edges (with type SM_FE_CROSS_SECTION)
        let cross_section_at = |fb: &Self, v: Option<VertexId>| -> Result<Option<EdgeId>> {
            let v = v.ok_or_else(nul)?;
            for e in vertex::edges(fb.brep(), v, None)? {
                if fb.fillet_edge(e)?.fillet_edge_type() == FilletEdgeType::CrossSection {
                    return Ok(Some(e));
                }
            }
            Ok(None)
        };
        let start_vert = edge::vertex(self.brep(), rail1)?;
        let start_edge = cross_section_at(self, start_vert)?;
        let end_vert = edge::other_vertex(self.brep(), rail1, start_vert)?;
        let end_edge = cross_section_at(self, end_vert)?;
        let start_edge = start_edge.ok_or_else(nul)?;
        let end_edge = end_edge.ok_or_else(nul)?;

        let fil_edges = [start_edge, rail1, end_edge, rail2];
        let orients = [
            OrientType::Opposite,
            OrientType::Same,
            OrientType::Same,
            OrientType::Opposite,
        ];
        let mut ordered_uv: Vec<Option<BSplineCurve>> = Vec::new();
        let mut ordered_3d: Vec<BSplineCurve> = Vec::new();
        let mut ordered_surfs: Vec<Option<&Surface>> = Vec::new();
        for &fe in &fil_edges {
            // pMateEU is dereferenced only for the rails
            let (prim, mate) = self.prim_and_opt_mate(fe, OP_BLEND)?;
            let (ps_curve, base_surface) = match self.fillet_edge(fe)?.fillet_edge_type() {
                FilletEdgeType::BlendingRail
                | FilletEdgeType::RailRailInterpolation
                | FilletEdgeType::SetbackRail => {
                    let mate = mate.ok_or_else(nul)?;
                    let face = self.fillet_edge(fe)?.original_face().ok_or_else(nul)?;
                    (
                        eu1::uv_trim_curve(self.brep(), mate)?.0,
                        target.face(face)?.surface.as_ref(),
                    )
                }
                _ => {
                    let fg = self.fillet_edgeuse(prim)?.fillet_geom().ok_or_else(nul)?;
                    (
                        eu1::uv_trim_curve(self.brep(), prim)?.0,
                        self.geom(fg)?.fillet_surface(None)?,
                    )
                }
            };
            ordered_uv.push(ps_curve.and_then(as_b_spline).cloned());
            let c = self.brep().edge(fe)?.curve.as_ref().ok_or_else(nul)?;
            ordered_3d.push(as_b_spline(c).ok_or_else(nul)?.clone());
            ordered_surfs.push(base_surface);
        }
        let nets: Vec<Option<BlendNeighbour<'_>>> = ordered_surfs
            .iter()
            .map(|s| s.map(|surface| BlendNeighbour { surface }))
            .collect();
        let surfaces = BSplineSurface::create_corner_blend(
            k,
            &ordered_3d,
            &orients,
            &ordered_uv,
            &nets,
            approx_tol_3d,
            tangency_tol_radians,
            false,
        )?;
        let first = surfaces.into_iter().next().ok_or_else(nul)?;
        self.geom_mut(g)?
            .set_fillet_surface(Some(Surface::from(first)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_geom::p1::{FilletGeom, FilletVertex};

    fn kernel() -> Kernel {
        Kernel::default()
    }

    /// A bilinear plane patch over `[0, 1]^2` scaled by `s`.
    fn plane_patch(s: f64) -> Surface {
        let mut p = Plane::from_normal(Point3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        p.make_nurb().unwrap();
        let _ = s;
        Surface::from(p)
    }

    fn fb_with_points(k: &Kernel, pts: &[Point3]) -> (FilletBrep, Vec<VertexId>) {
        let mut fb = FilletBrep::new();
        let mut ids = Vec::new();
        for &p in pts {
            let v = fb.brep_mut().add_vertex(p).unwrap();
            fb.insert_fillet_vertex(v, FilletVertex::new(k, None));
            ids.push(v);
        }
        (fb, ids)
    }

    #[test]
    fn uv_curve_on_plane_is_a_line() {
        let k = kernel();
        let srf = plane_patch(1.0);
        let dom = srf.natural_uv_domain_if_built();
        let a = srf.evaluate_point(&k, dom.evaluate(0.2, 0.3)).unwrap();
        let b = srf.evaluate_point(&k, dom.evaluate(0.2, 0.9)).unwrap();
        let (fb, v) = fb_with_points(&k, &[a, b]);
        let r = create_uv_curve(&k, &fb, v[0], v[1], false, &srf, 1e-6).unwrap();
        assert!(r.curve_3d.is_none());
        assert!(r.uv_curve2.is_none());
        let c = r.uv_curve1.unwrap();
        let ivl = c.natural_interval();
        let s = c.evaluate_point(&k, ivl.min()).unwrap();
        let e = c.evaluate_point(&k, ivl.max()).unwrap();
        let ua = dom.evaluate(0.2, 0.3);
        let ub = dom.evaluate(0.2, 0.9);
        assert!((s.x - ua.x).abs() < 1e-9 && (s.y - ua.y).abs() < 1e-9);
        assert!((e.x - ub.x).abs() < 1e-9 && (e.y - ub.y).abs() < 1e-9);
    }

    #[test]
    fn uv_curve_diagonal_without_vertexuse_falls_back_to_line() {
        let k = kernel();
        let srf = plane_patch(1.0);
        let dom = srf.natural_uv_domain_if_built();
        let a = srf.evaluate_point(&k, dom.evaluate(0.1, 0.1)).unwrap();
        let b = srf.evaluate_point(&k, dom.evaluate(0.8, 0.7)).unwrap();
        let (fb, v) = fb_with_points(&k, &[a, b]);
        let r = create_uv_curve(&k, &fb, v[0], v[1], false, &srf, 1e-6).unwrap();
        assert!(r.uv_curve1.is_some() && r.curve_3d.is_none());
    }

    #[test]
    fn seam_on_open_surface_fails() {
        let k = kernel();
        let srf = plane_patch(1.0);
        let (fb, v) = fb_with_points(
            &k,
            &[Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
        );
        let e = create_uv_curve(&k, &fb, v[0], v[1], true, &srf, 1e-6).unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }));
    }

    #[test]
    fn b_spline_cast_accepts_b_spline_derived_kinds() {
        // SM_CAST_PTR(SmBSplineCurve, ..) succeeds for an SmLine (derived from
        // SmBSplineCurve upstream)
        let line = Curve::from(crate::curve::line::Line::from_points(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 3.0),
            3,
        ));
        assert!(as_b_spline(&line).is_some());
        let seg = BSplineCurve::create_line_segment(
            3,
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        assert!(as_b_spline(&Curve::from(seg)).is_some());
    }

    #[test]
    fn rail_index_of_missing_vertex_is_null() {
        let k = kernel();
        let mut fb = FilletBrep::new();
        let g = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Default).unwrap());
        assert!(fb.rail_index_of_vertex(g, None).is_err());
        let v = fb
            .brep_mut()
            .add_vertex(Point3::new(0.0, 0.0, 0.0))
            .unwrap();
        // no rails: upstream dereferences the missing rail vertex use
        assert!(fb.rail_index_of_vertex(g, Some(v)).is_err());
    }

    #[test]
    fn nx2_without_fillet_surfaces_is_surf_int_failure() {
        // No cross-section edge: both fillet surfaces stay NULL, sm_SrfSrfIntersection's
        // NER fails, so upstream marks the vertex SM_FIL_SURF_INT_FAILURE and returns
        // SM_ERR (not SM_ERR_NULL_POINTER).
        let k = kernel();
        let (mut fb, v) = fb_with_points(&k, &[Point3::new(0.0, 0.0, 0.0)]);
        fb.fillet_vertex_mut(v[0])
            .unwrap()
            .set_fillet_corner(Some(super::super::p1::FilletCornerId(0)));
        let e = fb.calc_nx2_vert_fillet_x_fillet(&k, v[0], &[]).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Upstream {
                code: crate::error::SM_ERR,
                ..
            }
        ));
        assert_eq!(
            fb.fillet_vertex(v[0]).unwrap().status(),
            FilStatus::SurfIntFailure
        );
    }

    #[test]
    fn int_2_fillets_without_corner_is_null() {
        let k = kernel();
        let (mut fb, v) = fb_with_points(&k, &[Point3::new(0.0, 0.0, 0.0)]);
        let e = fb
            .calc_fillet_int_2_fillets(&k, v[0], CornerLookup::default(), &[], &Brep::new())
            .unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }));
        fb.fillet_vertex_mut(v[0])
            .unwrap()
            .set_fillet_corner(Some(super::super::p1::FilletCornerId(0)));
        let e = fb
            .calc_fillet_int_2_fillets(&k, v[0], CornerLookup::default(), &[], &Brep::new())
            .unwrap_err();
        assert!(matches!(
            e,
            SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            }
        ));
    }

    #[test]
    fn rail_int_extended_without_edge_is_null() {
        let k = kernel();
        let (mut fb, v) = fb_with_points(&k, &[Point3::new(0.0, 0.0, 0.0)]);
        let e = fb
            .calc_rail_int_extended_edgeuse(
                &k,
                v[0],
                CornerLookup::default(),
                &[],
                &mut Brep::new(),
            )
            .unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }));
    }

    #[test]
    fn nx2_without_corner_is_null() {
        let k = kernel();
        let (mut fb, v) = fb_with_points(&k, &[Point3::new(0.0, 0.0, 0.0)]);
        let e = fb.calc_nx2_vert_fillet_x_fillet(&k, v[0], &[]).unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }));
    }

    #[test]
    fn make_face_without_surface_is_null() {
        let mut k = kernel();
        let mut fb = FilletBrep::new();
        let g = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Default).unwrap());
        let mut b = Brep::new();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let key_of = |_e: EntityRef| crate::geom::object::ObjectRef {
            type_code: 0,
            key: crate::geom::attribute::ObjectKey(0),
        };
        let mut env = edge::NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        assert!(fb.make_face_brep(&mut k, g, &[], &mut b, &mut env).is_err());
    }

    #[test]
    fn blending_without_solver_is_null() {
        let k = kernel();
        let mut fb = FilletBrep::new();
        let g = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Blends).unwrap());
        assert!(fb.calc_blending_geom(&k, g, &[], &mut Brep::new()).is_err());
    }
}
