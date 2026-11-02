//! Proto topology, part 2: creating, finding, combining and resolving proto vertices,
//! proto edges and their definitions, the imprint of vertex and edge definitions into
//! both B-reps, and the updates of the proto topology after an imprint split an edge
//! or a face.
//!
//! Upstream: `source/SMLib/src/SmProtoTopology.cpp`, `source/SMLib/inc/SmProtoTopology.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`ProtoBrep`] | the B-rep members these functions read through `SmEdge*` / `SmFace*` / `SmVertex*` (`GetInterval`, `GetCurve`, `GetTolerance`, `GetUVDomain`, `SetUVDomain`, `GetSurface`, `GetEdgeuseOfFace`, `IsConnectedToFace`), `SmCurve::Copy`'s allocation, and the B-rep edits of `SmPointClassification::MergeIntoObject` ([`ProtoBrep::merge_point_class_into_brep`]) |
//! | [`ProtoTopologyManager::imprint_vertex_definition`] | `SmVertexDefinition::Imprint` |
//! | [`ProtoTopologyManager::imprint_edge_definition`] | `SmEdgeDefinition::Imprint` |
//! | [`ProtoTopologyManager::resolve_proto_vertex`] | `SmProtoVertex::Resolve` |
//! | [`ProtoTopologyManager::add_vertex_definition_from_pcs`] | `SmProtoVertex::AddVertexDefinition(SmPointClassification&, SmPointClassification&, ..)` |
//! | [`ProtoTopologyManager::find_matching_edge_definition`] | `SmProtoEdge::FindMatchingEdgeDefinition` |
//! | [`ProtoTopologyManager::add_edge_definition`] | `SmProtoEdge::AddEdgeDefinition(SmEdgeDefinition*)` |
//! | [`ProtoTopologyManager::add_edge_definition_from_intervals`] | `SmProtoEdge::AddEdgeDefinition(const SmCurveInterval&, const SmCurveInterval&)` |
//! | [`ProtoTopologyManager::combine_proto_edge`] | `SmProtoEdge::CombineProtoEdge` |
//! | [`ProtoTopologyManager::resolve_proto_edge`] | `SmProtoEdge::Resolve` |
//! | [`ProtoTopologyManager::create_vertex_definition`] | `SmProtoTopologyManager::CreateVertexDefinition` |
//! | [`ProtoTopologyManager::create_proto_vertex`] | `SmProtoTopologyManager::CreateProtoVertex` |
//! | [`ProtoTopologyManager::find_or_create_proto_vertex`] | `SmProtoTopologyManager::FindOrCreateProtoVertex` |
//! | [`ProtoTopologyManager::find_proto_edge`] | `SmProtoTopologyManager::FindProtoEdge` |
//! | [`ProtoTopologyManager::create_proto_edge`] | `SmProtoTopologyManager::CreateProtoEdge` |
//! | [`ProtoTopologyManager::remove_edge_definition`] | `SmProtoTopologyManager::RemoveEdgeDefinition` |
//! | [`ProtoTopologyManager::update_edges`] | `SmProtoTopologyManager::UpdateEdges` |
//! | [`ProtoTopologyManager::update_faces`] | `SmProtoTopologyManager::UpdateFaces` |
//! | [`ProtoTopologyManager::can_combine_vertex_definitions`] | `SmProtoTopologyManager::CanCombineVertexDefinitions` |
//! | [`ProtoBrep::brep_make_edge_in_face`], [`make_edge_in_face_in_context`] | `SmBrep::MakeEdgeInFace` of `SmEdgeDefinition::Imprint` on `GetWhichBrep(i)` |
//!
//! Deviations:
//! - **B-rep access.** The topology objects are the caller's handles `O`, as in
//!   [`p1`](super::p1). The stored members of edges, faces and vertices these functions
//!   read or write, the two connectivity walks of `UpdateFaces`, and the allocation of
//!   curve copies go through [`ProtoBrep`], which the owner of the B-reps implements
//!   (upstream reaches them through the pointers). `SmTol::GetZoneTol3d` of a vertex,
//!   edge or face is [`ProtoBrep::topology_tolerance`]; of any other object
//!   [`object_zone_tol_3d`].
//! - **Imprinting into a face.** `SmBrep::MakeEdgeInFace` of `SmEdgeDefinition::Imprint`
//!   is [`ProtoBrep::brep_make_edge_in_face`]: the owner runs part 5's arena entry point
//!   on the B-rep behind its handles (most through [`make_edge_in_face_in_context`]),
//!   which checks that B-rep's own `m_bEditingEnabled`. The curves it consumes are
//!   passed by value (the definition's 3D curve is cloned out of the owner's store,
//!   where its handle stays valid for the later `Copy` for B-rep 2, as upstream's
//!   pointer does); `SM_CAST_PTR(SmBSplineCurve, m_pXSectUVCurve<i>)` passes the UV
//!   curve only when it is of a B-spline kind.
//! - **Deleted curves.** Functions that delete curves (through `Destruct` or the
//!   destructors) return the handles deleted, as in [`p1`](super::p1).
//! - **Undefined behaviour upstream** is [`SolidError::InvalidInput`]: dereferencing a
//!   null or deleted object (plan/upstream-issues.md UB-117, UB-118), and the reads of an
//!   uninitialised position, gap or tangent after an ignored failure (of
//!   `sm_FindPositionFromPCs` in `CreateVertexDefinition` and `CreateProtoVertex`, UB-115;
//!   of the mid tangents in `AddEdgeDefinition`, UB-116).
//! - **Leaks.** On the error returns of `CreateProtoEdge`, `AddEdgeDefinition` (from
//!   intervals) and `Imprint` upstream leaks the proto edge or curve copies it made; the
//!   checks are done before anything is made, so nothing is leaked.
//! - **Out-params** become return values: `AddVertexDefinition`'s definition and
//!   created flag, `FindOrCreateProtoVertex`'s created flag, `FindProtoEdge`'s
//!   identical flag, `CanCombineVertexDefinitions`' choice (its `SM_ERR_NULL_POINTER`
//!   return leaves the choice 0). `FindMatchingEdgeDefinition`'s `rbRev` is always
//!   `FALSE` upstream and dropped.
//! - **Curve intervals** are passed with the classification they belong to (upstream
//!   reaches it through the interval's back pointer; `SmCurveInterval::GetCurve` is the
//!   classification's curve).
//! - **Not upstream: no split on a vertex** (source issue #480).
//!   `SmProtoEdge::Resolve` splits a proto edge that runs from a proto vertex back to
//!   itself along an open curve longer than `2 * m_dTol` by adding a vertex definition
//!   that copies the first one's classifications. `CanCombineVertexDefinitions` has
//!   just merged definitions up to `5 * (tol1 + tol2)` apart into that proto vertex,
//!   keeping the one on a vertex. When the copied definition is on a vertex in either
//!   B-rep, both ends land on that one vertex there (moved twice, to their midpoint)
//!   while the other B-rep gets a short edge between two vertices; the merge then
//!   fails in `MakeFaceuse` (a union whose coplanar faces' rims cross 4e-5 to 2e-4 mm
//!   from a vertex, `SM_ERR`). With
//!   [`ProtoTopologyManager::set_split_self_loops_on_vertices`]`(false)`, which the
//!   Boolean's topology intersector sets only with the opt-in
//!   `Kernel::fixes.proto_edge_self_loop_on_vertex_degenerate`,
//!   [`ProtoTopologyManager::resolve_proto_edge`]
//!   treats such a proto edge as degenerate instead, as upstream does for one shorter
//!   than `2 * m_dTol`. The default keeps upstream's split, so this unit's difftest
//!   still compares the faithful port (7 generated cases reach the branch).
//!   Confirmed on the C++ build (plan/upstream-issues.md WR-214).
//! - **Opt-in fix (WR-306, #1058).** With `Kernel::fixes.degenerate_proto_edge_in_one_vertex`,
//!   [`ProtoTopologyManager::resolve_proto_edge`] treats a proto edge from a proto vertex
//!   back to itself whose curve ends find one vertex definition as degenerate below twice
//!   the proto vertex's containment radius (`ContainsPoint`'s), not twice its own
//!   tolerance, and marks the proto vertex as having absorbed it; the Boolean's
//!   `ImprintIntersections` then fits that vertex to its edges (the SMLib fork's fix).
//! - **Opt-in fixes (WR-452, WR-453, #435, #410).** With `Kernel::fixes.imprint_trims_edge_to_proto_vertex` and
//!   `Kernel::fixes.imprint_extends_edge_to_proto_vertex`, [`ProtoTopologyManager::imprint_edge_definition`]
//!   first fits each end of the definition's interval to its proto vertex (`imprint_ends`: the SMLib
//!   fork's thread-groove fixes, source fork b43876f, a8caccd).
//! - `UpdateEdges` trims a definition at the split point; its alternative of splitting
//!   the proto edge (`SmProtoEdge::Split`) sits behind `static constexpr SmBoolean
//!   cbiDoSplit = FALSE` and is not ported.
//! - The `SM_DEBUG_CODE` blocks (dumps, drawing) are not ported; release `SM_ASSERT`s
//!   are expression statements whose only effect is the dereference they make.
//! - **Edge use binormals.** `SmEdgeuse::EvaluateBinormal` of `UpdateFaces` is
//!   [`ProtoBrep::edgeuse_evaluate_binormal`] on the owner's B-rep. Its status is ignored
//!   as upstream: a failed evaluation leaves the binormal at its previous value
//!   (`SM_UNDEF_DOUBLE` components before the first success), and a missing splitting
//!   edge, which upstream dereferences, is [`SolidError::InvalidInput`].
//! - **Recursion.** None.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::Curve;
use crate::curve::curve::p2::BSPLINE_CURVE_TYPE;
use crate::error::EntityRef;
use crate::error::{Result, SolidError, SM_ERR_NULL_POINTER};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::{xsect_tol3d, ScaledZero};
use crate::intersect::curve_class::p6::MergeContext;
use crate::intersect::curve_class::{CurveClassification, CurveInterval};
use crate::intersect::point_class::{
    object_zone_tol_3d, ClassObject, ClassObjectKind, MergeOutputs, PointClassType,
    PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{max2, max3, max4, Point2, Point3, Vec3, EFF_ZERO};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::face::p2::proto_vertex_containment_radius;
use crate::topology::brep::p5::{make_edge_in_face_in, EdgeInFace};
use crate::topology::VertexId;

mod imprint_ends;

use super::p1::proto::{are_connected, can_imprint, find_position_from_pcs, topo_dimension};
use super::p1::{
    EdgeDefId, EdgeDefinition, ProtoEdgeId, ProtoGeometry, ProtoTopology, ProtoTopologyManager,
    ProtoVertex, ProtoVertexId, VertexDefId, VertexDefinition,
};

const OP: &str = "topology::proto_topology::p2";
const OP_IMPRINT_VD: &str =
    "topology::proto_topology::p2::ProtoTopologyManager::imprint_vertex_definition";
const OP_IMPRINT_ED: &str =
    "topology::proto_topology::p2::ProtoTopologyManager::imprint_edge_definition";
const OP_ADD_VD: &str =
    "topology::proto_topology::p2::ProtoTopologyManager::add_vertex_definition_from_pcs";
const OP_CREATE_PE: &str = "topology::proto_topology::p2::ProtoTopologyManager::create_proto_edge";
const OP_UPDATE_EDGES: &str = "topology::proto_topology::p2::ProtoTopologyManager::update_edges";
const OP_COMBINE_VD: &str =
    "topology::proto_topology::p2::ProtoTopologyManager::can_combine_vertex_definitions";

/// `sdTolFactor` of `CanCombineVertexDefinitions`: the distance allowed between two
/// combinable vertex definitions, in units of their summed tolerances.
const VD_TOL_FACTOR: f64 = 5.0;

/// The B-rep side of the proto topology: the members of the caller's edges, faces and
/// vertices these functions read or write, and the store new curves go to.
///
/// Every method answers for a live handle of the kind it names; `None` is an unknown
/// handle or one of another kind (a dereference of a bad pointer upstream).
pub trait ProtoBrep<O>: ProtoGeometry<O> + ProtoTopology<O> {
    /// `SmEdge::GetInterval`.
    fn edge_interval(&self, edge: O) -> Option<Extent1d>;
    /// `SmEdge::GetCurve`: the handle of the edge's curve (`Some(None)` for none).
    fn edge_curve(&self, edge: O) -> Option<Option<O>>;
    /// `GetTolerance` of a vertex, edge or face (the stored zone tolerance).
    fn topology_tolerance(&self, topo: O) -> Option<f64>;
    /// `SmFace::GetUVDomain`.
    fn face_uv_domain(&self, face: O) -> Option<Extent2d>;
    /// Trim-derived UV domain with native's ignored ordinary status. Retains the
    /// initialized or partial domain on failure.
    ///
    /// # Errors
    ///
    /// Invalid or stale owning-face handles and genuine unported callees.
    fn face_trim_uv_domain(&mut self, k: &Kernel, face: O) -> Result<Extent2d>;
    /// The actual (possibly partial) UV point classification with the native
    /// `PointClassify(uv, tolerance, TRUE, TRUE, pc)` flags and ignored ordinary status.
    ///
    /// # Errors
    ///
    /// Invalid or stale owning-face handles and genuine unported callees.
    fn face_point_classify_uv(
        &mut self,
        k: &Kernel,
        face: O,
        uv: Point2,
        source_tolerance: f64,
    ) -> Result<PointClassType>;
    /// `SmFace::SetUVDomain`.
    fn set_face_uv_domain(&mut self, face: O, domain: Extent2d) -> Option<()>;
    /// `SmFace::GetSurface`: the handle of the face's surface (`Some(None)` for none).
    fn face_surface(&self, face: O) -> Option<Option<O>>;
    /// `SmEdge::GetEdgeuseOfFace`: the edge's use bounding `face`, if any.
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk.
    fn edge_edgeuse_of_face(&self, edge: O, face: O) -> Result<Option<O>>;
    /// `SmEdge::IsConnectedToFace`.
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk.
    fn edge_is_connected_to_face(&self, edge: O, face: O) -> Result<bool>;
    /// Stores a new curve (an `SmCurve::Copy`) and returns its handle; `None` when the
    /// store is full.
    fn insert_curve(&mut self, curve: Curve) -> Option<O>;
    /// `SmVertex::GetCommonEdges` (read by `topology.proto_topology_p3`).
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk.
    fn vertex_common_edges(&self, v1: O, v2: O) -> Result<Vec<O>>;
    /// `SmBrep::SqueezeEdge(pEdge, pSurvivingVertex)` on the B-rep `brep` (called by
    /// `topology.proto_topology_p3` on `GetWhichBrep`): the owner of a real B-rep
    /// implements it with `topology::brep::p3::squeeze_edge`.
    ///
    /// # Errors
    ///
    /// Those of the edit; an owner with no B-rep behind its handles returns
    /// [`SolidError::Unported`] under a `harness:` label.
    fn brep_squeeze_edge(&mut self, k: &mut Kernel, brep: O, edge: O, survivor: O) -> Result<()>;
    /// `SmFace::CurveOnClassify(bDoPointClassify, cc)` with upstream's other arguments at
    /// their defaults (`FALSE`), read by `topology.proto_topology_p3`: the B-rep owner
    /// implements it with `query::face::p2::curve_on_classify`. Returns
    /// `pbOptModifiedCurve`.
    ///
    /// # Errors
    ///
    /// Those of `curve_on_classify`.
    fn face_curve_on_classify(
        &self,
        k: &Kernel,
        face: O,
        do_point_classify: bool,
        cc: &mut CurveClassification<O>,
    ) -> Result<bool>;
    /// Mutable owning-query successor. Legacy providers retain their read-only behavior;
    /// actual providers can retain lazy UV/gap updates on both success and failure.
    /// # Errors
    /// The original face classification error.
    fn face_curve_on_classify_mut(
        &mut self,
        k: &Kernel,
        face: O,
        do_point_classify: bool,
        cc: &mut CurveClassification<O>,
    ) -> Result<bool> {
        self.face_curve_on_classify(k, face, do_point_classify, cc)
    }

    /// `SmBrep::Point3DClassify(point, tolerance, bDoBoundaryIntersections, sPC)` of the
    /// B-rep `brep` names, read by `topology.proto_topology_p3`: the B-rep owner
    /// implements it with `query::brep::BrepQuery::point_3d_classify` on its B-rep and
    /// maps the classification's objects to its handles.
    ///
    /// # Errors
    ///
    /// Those of `point_3d_classify`; [`SolidError::InvalidInput`] for a handle that
    /// names no B-rep or an object without a handle.
    fn brep_point_3d_classify(
        &mut self,
        k: &Kernel,
        brep: O,
        point: Point3,
        tolerance: f64,
        boundary_intersections: bool,
    ) -> Result<PointClassification<O>>;
    /// `pBrep->MakeEdgeInFace(pFace, pStartVtx, pEndVtx, pCurve, pTempUV, ivl, eOrient,
    /// dTol, rpNewEdge, rpNewLoop, rpNewFace)` of `SmEdgeDefinition::Imprint` on the
    /// B-rep `brep` (`GetWhichBrep`): the owner of real B-reps runs
    /// [`make_edge_in_face_in`] on the B-rep behind the handles, whose own
    /// `m_bEditingEnabled` it checks (most owners through
    /// [`make_edge_in_face_in_context`]). `curve` and `uv_curve` are consumed. Returns
    /// the new edge, loop and face, each `None` when not made.
    ///
    /// # Errors
    ///
    /// Those of the edit; [`SolidError::InvalidInput`] for a face or vertex that is not
    /// in `brep`; an owner with no B-rep behind its handles returns
    /// [`SolidError::Unported`] under a `harness:` label.
    fn brep_make_edge_in_face(
        &mut self,
        k: &Kernel,
        brep: O,
        face: O,
        start: O,
        end: O,
        curve: Curve,
        uv_curve: Option<Curve>,
        interval: Extent1d,
        orientation: OrientType,
        tolerance: f64,
    ) -> Result<(Option<O>, Option<O>, Option<O>)>;
    /// `SmEdgeuse::EvaluateBinormal(dParam, FALSE, rPt, rBinormal)` of the edge use
    /// `edgeuse` (read by `UpdateFaces`): the owner of real B-reps runs
    /// [`evaluate_binormal`](crate::topology::edgeuse::p3::evaluate_binormal) on the
    /// B-rep behind the handle (which may build the edge use's UV trim curve, as
    /// upstream) and returns the binormal.
    ///
    /// # Errors
    ///
    /// Those of `evaluate_binormal`; [`SolidError::InvalidInput`] for a handle that
    /// names no edge use; an owner with no B-rep behind its handles returns
    /// [`SolidError::Unported`] under a `harness:` label.
    fn edgeuse_evaluate_binormal(&mut self, k: &Kernel, edgeuse: O, param: f64) -> Result<Vec3>;
    /// `rPC.MergeIntoObject(srcPoint, &newEdges, &newVertices)` of
    /// `SmVertexDefinition::Imprint`: the classification's point merged into the B-rep
    /// behind its object, which upstream edits through the object's pointer (a new
    /// shell vertex, vertex loop or edge split, or the existing vertex moved by
    /// `SmVertex::SetPoint`). The owner of mutable B-reps implements it with
    /// [`merge_into_object_mut`](crate::intersect::curve_class::p6::merge_into_object_mut);
    /// the default has no mutable owning context and returns
    /// [`PointClassification::merge_into_object`]'s result.
    ///
    /// # Errors
    ///
    /// Those of the merge.
    fn merge_point_class_into_brep(
        &mut self,
        k: &mut Kernel,
        pc: &mut PointClassification<O>,
        src_point: Point3,
    ) -> Result<MergeOutputs<O>>
    where
        O: ClassObject,
    {
        pc.merge_into_object(k, src_point)
    }
}

/// The handle the curve point solves name (only the solution count and values are
/// used).
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle(Extent1d);

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// A dereference of a null pointer or a deleted object upstream, or a read of an
/// uninitialised value (undefined behaviour).
const fn dangling(reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: OP,
        reason,
    }
}

/// A status upstream ignores, except the errors that cannot be ignored here: unported
/// callees and undefined behaviour upstream.
fn ignore_soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }),
        ) => Err(e),
        Err(_) => Ok(None),
    }
}

/// `SM_CAST_PTR(SmTopology, obj)`.
fn as_topology<O: ClassObject>(o: Option<O>) -> Option<O> {
    o.filter(|o| {
        matches!(
            o.class_kind(),
            ClassObjectKind::Vertex
                | ClassObjectKind::Edge
                | ClassObjectKind::Face
                | ClassObjectKind::Loop
                | ClassObjectKind::Region
                | ClassObjectKind::Vertexuse
                | ClassObjectKind::Edgeuse
                | ClassObjectKind::Faceuse
                | ClassObjectKind::Loopuse
        )
    })
}

/// `SM_CAST_PTR(<kind>, obj)` for a topology kind.
fn as_kind<O: ClassObject>(o: Option<O>, kind: ClassObjectKind) -> Option<O> {
    o.filter(|o| o.class_kind() == kind)
}

/// `EvaluatePoint` with its status ignored: the point, `SM_UNDEF` on failure.
fn eval_point(k: &Kernel, curve: &Curve, t: f64) -> Result<Point3> {
    Ok(ignore_soft(curve.evaluate_point(k, t))?.unwrap_or(Vec3::UNDEF))
}

/// `SmTol::GetZoneTol3d(const SmObject*)`: a vertex's, edge's or face's own tolerance,
/// otherwise [`object_zone_tol_3d`].
fn zone_tol_of<O: ClassObject>(
    k: &Kernel,
    brep: &impl ProtoBrep<O>,
    object: Option<O>,
) -> Result<f64> {
    match object {
        Some(o)
            if matches!(
                o.class_kind(),
                ClassObjectKind::Vertex | ClassObjectKind::Edge | ClassObjectKind::Face
            ) =>
        {
            brep.topology_tolerance(o)
                .ok_or(dangling("topology handle is unknown"))
        }
        _ => object_zone_tol_3d(k, brep, object),
    }
}

/// `SmTol::GetXSectTol3d(const SmObject*, const SmObject*)`: the sum of the two zone
/// tolerances.
fn xsect_tol_of<O: ClassObject>(
    k: &Kernel,
    brep: &impl ProtoBrep<O>,
    obj1: Option<O>,
    obj2: Option<O>,
) -> Result<f64> {
    let t1 = zone_tol_of(k, brep, obj1)?;
    let t2 = zone_tol_of(k, brep, obj2)?;
    Ok(xsect_tol3d(k, t1 + t2))
}

/// The curve behind `o` (a `NULL` dereference upstream for none).
fn curve_of<O: Copy>(brep: &impl ProtoBrep<O>, o: Option<O>) -> Result<&Curve> {
    o.and_then(|o| brep.curve(o))
        .ok_or(dangling("curve handle is null or unknown"))
}

/// `pEdge->GetCurve()` dereferenced.
fn edge_curve_of<O: Copy>(brep: &impl ProtoBrep<O>, edge: O) -> Result<O> {
    brep.edge_curve(edge)
        .ok_or(dangling("edge handle is unknown"))?
        .ok_or(dangling("edge has no curve"))
}

/// `SmCurve::Trim(rDomain)` with its status ignored (`domain` may be snapped).
fn trim_ignored<O: Copy>(
    k: &Kernel,
    brep: &mut impl ProtoBrep<O>,
    curve: Option<O>,
    domain: &mut Extent1d,
) -> Result<()> {
    if let Some(c) = curve {
        let crv = brep
            .curve_mut(c)
            .ok_or(dangling("curve handle is unknown"))?;
        ignore_soft(crv.trim(k, domain, TrimMode::Notify))?;
    }
    Ok(())
}

/// `SmTol::GetZoneTol3d_ForXSectResult(cpObj1, cpObj2)`: the larger zone tolerance.
fn zone_tol_for_xsect_result<O: ClassObject>(
    k: &Kernel,
    brep: &impl ProtoBrep<O>,
    obj1: O,
    obj2: O,
) -> Result<f64> {
    Ok(max2(
        zone_tol_of(k, brep, Some(obj1))?,
        zone_tol_of(k, brep, Some(obj2))?,
    ))
}

/// `GlobalPointSolve(crIvl, SM_SO_INTERSECT, crPt, dTol, &dTol, NULL, SM_SR_SINGLE,
/// rSols)` of the curve `curve`: the solution values, or `None` when the solve
/// failed.
fn edge_point_solve<O: ClassObject>(
    k: &Kernel,
    brep: &impl ProtoBrep<O>,
    curve: O,
    interval: &Extent1d,
    point: Point3,
    tol: f64,
) -> Result<Option<Vec<f64>>> {
    let crv = curve_of(brep, Some(curve))?;
    let opts = GlobalPointSolveOptions {
        target_distance: Some(tol),
        handle: Some(CurveHandle(crv.natural_interval())),
        ..GlobalPointSolveOptions::default()
    };
    let sols = ignore_soft(crv.global_point_solve(
        k,
        interval,
        SolverOperation::Intersect,
        point,
        tol,
        SolutionRequested::Single,
        &opts,
    ))?;
    Ok(sols.map(|s| {
        (0..s.len())
            .filter_map(|i| s.get(i).map(|x| x.start.solution_value))
            .collect()
    }))
}

// ---- callees in units not ported yet -------------------------------------------------------

/// `SM_CAST_PTR(SmBSplineCurve, pUVCurve)` of a definition's UV curve: a copy of the
/// curve when it is of a B-spline kind, `None` for none or another kind.
fn bspline_uv_curve<O: Copy>(brep: &impl ProtoBrep<O>, uv: Option<O>) -> Result<Option<Curve>> {
    match uv {
        None => Ok(None),
        Some(o) => {
            let c = brep
                .curve(o)
                .ok_or(dangling("UV curve handle is unknown"))?;
            Ok(c.is_kind_of(BSPLINE_CURVE_TYPE).then(|| c.clone()))
        }
    }
}

/// [`ProtoBrep::brep_make_edge_in_face`] for an owner that reaches its B-reps through a
/// [`MergeContext`] (`MergeIntervalBrep::merge_context` of the face handle): runs
/// [`make_edge_in_face_in`] on the context's B-rep, which checks that B-rep's own
/// `m_bEditingEnabled`, notifying through the context and naming the new edge's curve
/// with its `curve_ref`. `start` and `end` are the vertices in that B-rep.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a context whose target is not a face; those of
/// [`make_edge_in_face_in`].
pub fn make_edge_in_face_in_context<O>(
    k: &Kernel,
    ctx: MergeContext<'_, O>,
    start: VertexId,
    end: VertexId,
    curve: Curve,
    uv_curve: Option<Curve>,
    interval: Extent1d,
    orientation: OrientType,
    tolerance: f64,
) -> Result<(Option<O>, Option<O>, Option<O>)> {
    let mut ctx = ctx;
    let EntityRef::Face(face) = ctx.target else {
        return Err(SolidError::invalid_input(
            OP,
            "MakeEdgeInFace target is not a face",
        ));
    };
    let mut out = EdgeInFace::default();
    make_edge_in_face_in(
        k,
        ctx.brep,
        Some(face),
        Some(start),
        Some(end),
        curve,
        uv_curve,
        interval,
        orientation,
        tolerance,
        &mut out,
        &mut ctx.notifications,
        ctx.curve_ref,
    )?;
    Ok((
        out.edge.map(|e| (ctx.handle)(EntityRef::Edge(e))),
        out.loop_.map(|l| (ctx.handle)(EntityRef::Loop(l))),
        out.face.map(|f| (ctx.handle)(EntityRef::Face(f))),
    ))
}

/// `sm_ClassifyParamOnFaces`: first matching face interior, or zero.
fn classify_param_on_faces<O: ClassObject>(
    k: &Kernel,
    brep: &mut impl ProtoBrep<O>,
    uv: Point2,
    zone_tol_3d: f64,
    face1: O,
    domain1: &Extent2d,
    face2: O,
    domain2: &Extent2d,
) -> Result<i32> {
    let in_face1 = domain1.contains_point(uv, EFF_ZERO);
    let in_face2 = domain2.contains_point(uv, EFF_ZERO);
    if in_face1 && !in_face2 {
        return Ok(1);
    }
    if !in_face1 && in_face2 {
        return Ok(2);
    }
    let tolerance = if zone_tol_3d > EFF_ZERO {
        zone_tol_3d
    } else {
        zone_tol_for_xsect_result(k, brep, face1, face2)?
    };
    if brep.face_point_classify_uv(k, face1, uv, tolerance)? == PointClassType::Face {
        return Ok(1);
    }
    if brep.face_point_classify_uv(k, face2, uv, tolerance)? == PointClassType::Face {
        return Ok(2);
    }
    Ok(0)
}

/// `SmSurface::DropPoint(pt, GetNaturalUVDomain(), NULL, .., SM_SO_NORMALIZE)`: the
/// drop point when it succeeded.
fn surface_drop_point<O: Copy>(
    k: &Kernel,
    brep: &impl ProtoBrep<O>,
    surface: Option<O>,
    point: Point3,
) -> Result<Option<Point2>> {
    let srf = surface
        .and_then(|o| brep.surface(o))
        .ok_or(dangling("surface handle is null or unknown"))?;
    let domain = srf.natural_uv_domain_with_nurb(k)?;
    let opts = crate::surface::surface::p5::DropPointOptions {
        operation: SolverOperation::Normalize, // Don't want off-boundary drops.
        ..crate::surface::surface::p5::DropPointOptions::default()
    };
    let r = srf.drop_point::<crate::surface::surface::p4::NoObject>(k, point, &domain, &opts)?;
    Ok(r.success.then_some(r.result_uv))
}

/// `pEU->EvaluateBinormal(dParam, FALSE, sBinormPt, rBinormal)` with its status ignored
/// ([`ProtoBrep::edgeuse_evaluate_binormal`]): the binormal, or `previous` (what
/// `rBinormal` held, which upstream writes only on success) when the evaluation failed.
fn edgeuse_evaluate_binormal<O: Copy>(
    k: &Kernel,
    brep: &mut impl ProtoBrep<O>,
    edgeuse: O,
    param: f64,
    previous: Vec3,
) -> Result<Vec3> {
    Ok(ignore_soft(brep.edgeuse_evaluate_binormal(k, edgeuse, param))?.unwrap_or(previous))
}

impl<O: ClassObject> ProtoTopologyManager<O> {
    fn vd_ref(&self, vd: VertexDefId) -> Result<&VertexDefinition<O>> {
        self.vertex_def(vd)
            .ok_or(dangling("vertex definition is deleted"))
    }

    fn vd_mut(&mut self, vd: VertexDefId) -> Result<&mut VertexDefinition<O>> {
        self.vertex_def_mut(vd)
            .ok_or(dangling("vertex definition is deleted"))
    }

    fn ed_ref(&self, ed: EdgeDefId) -> Result<&EdgeDefinition<O>> {
        self.edge_def(ed)
            .ok_or(dangling("edge definition is deleted"))
    }

    fn ed_mut(&mut self, ed: EdgeDefId) -> Result<&mut EdgeDefinition<O>> {
        self.edge_def_mut(ed)
            .ok_or(dangling("edge definition is deleted"))
    }

    fn pv_live(&self, pv: Option<ProtoVertexId>) -> Result<ProtoVertexId> {
        pv.filter(|&pv| self.proto_vertex(pv).is_some())
            .ok_or(dangling("proto vertex is null or deleted"))
    }

    fn pe_live(&self, pe: Option<ProtoEdgeId>) -> Result<ProtoEdgeId> {
        pe.filter(|&pe| self.proto_edge(pe).is_some())
            .ok_or(dangling("proto edge is null or deleted"))
    }

    /// Deletes the edge definition `ed` after `Destruct` (`pED->Destruct(); delete
    /// pED;`): the curves deleted.
    fn destruct_and_delete(&mut self, ed: EdgeDefId) -> Vec<O> {
        let mut deleted = self
            .edge_def_mut(ed)
            .map(EdgeDefinition::destruct)
            .unwrap_or_default();
        deleted.extend(self.delete_edge_definition(ed));
        deleted
    }

    // ---- SmVertexDefinition ----------------------------------------------------------

    /// Imprints the vertex definition `vd` into both B-reps (`Imprint`): each
    /// classification merges the position into its object (creating or reusing a
    /// vertex, which becomes the definition's final vertex), then the proto topology is
    /// updated for an edge that was split. The classification's object is restored
    /// afterwards (for B-rep 2 only when the update succeeded, as upstream).
    ///
    /// # Errors
    ///
    /// Those of [`ProtoBrep::merge_point_class_into_brep`]; `SM_ERR` when a merge
    /// made no single vertex and the classification is not on a vertex; those of
    /// [`ProtoTopologyManager::update_edges`]; [`SolidError::InvalidInput`] for a
    /// definition without a proto vertex (upstream reaches the manager through it).
    pub fn imprint_vertex_definition(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl ProtoBrep<O>,
        vd: VertexDefId,
    ) -> Result<()> {
        for which in [1, 2] {
            // If we're on an Edge, save that Edge for checking splits, after merge.
            let v = self.vd_ref(vd)?;
            let this_edge = v.brep_point_classif(which).edge_object();

            // MergeIntoObject() resets the PointClass' Object to the newly-created Vertex
            // (if it creates one). Save the original topology and restore it after.
            let pc_type = v.brep_point_classif(which).point_class();
            let pc_object = v.brep_point_classif(which).object();
            let position = v.position();

            let v = self.vd_mut(vd)?;
            let outputs =
                brep.merge_point_class_into_brep(k, v.brep_point_classif_mut(which), position)?;

            if outputs.new_vertices.len() == 1 {
                v.set_final_vertex(which, outputs.new_vertices.first().copied());
            } else if v.src_topo_type(which) == PointClassType::Vertex {
                let vertex = v.brep_point_classif(which).vertex_object();
                v.set_final_vertex(which, vertex);
            } else {
                return Err(SolidError::sm_err(OP_IMPRINT_VD));
            }

            if which == 1 {
                // Restore the PointClass' original object.
                v.brep_point_classif_mut(which)
                    .set_class_object(pc_type, pc_object);
            }

            // the manager is reached through the proto vertex
            self.pv_live(self.vd_ref(vd)?.proto_vertex())?;

            // imprint Vertex into the Brep - commonly splits edge
            self.update_edges(k, brep, which, this_edge, &outputs.new_edges, Some(vd))?;

            if which == 2 {
                // Restore the PointClass' original object.
                self.vd_mut(vd)?
                    .brep_point_classif_mut(which)
                    .set_class_object(pc_type, pc_object);
            }
        }
        Ok(())
    }

    // ---- SmEdgeDefinition ------------------------------------------------------------

    /// Imprints the edge definition `ed` into both B-reps (`Imprint`; the vertex
    /// definitions at both proto vertices must have been imprinted): on a face source a
    /// new edge is made in the face (splitting it, after which the proto topology on it
    /// is updated), on an edge source that edge is the final edge.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when a proto vertex has no final vertex for the
    /// definition; those of [`ProtoTopologyManager::proto_vertex_final_vertex`];
    /// those of [`ProtoBrep::brep_make_edge_in_face`] for a face source;
    /// [`SolidError::InvalidInput`] for a definition without a proto edge, or a proto
    /// edge without both proto vertices.
    pub fn imprint_edge_definition(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        ed: EdgeDefId,
    ) -> Result<()> {
        self.imprint_edge_definition_reporting_edits(k, brep, ed).0
    }

    /// [`ProtoTopologyManager::imprint_edge_definition`], also answering whether the
    /// imprint reached a B-rep edit (`MakeEdgeInFace`). A failure without one (no final
    /// vertex, or a check before the first `MakeEdgeInFace`) leaves both B-reps as
    /// they were.
    pub fn imprint_edge_definition_reporting_edits(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        ed: EdgeDefId,
    ) -> (Result<()>, bool) {
        let mut edited = false;
        let result = self.imprint_edge_definition_inner(k, brep, ed, &mut edited);
        (result, edited)
    }

    /// The body of [`ProtoTopologyManager::imprint_edge_definition`]; `edited` is set
    /// before the first B-rep edit.
    fn imprint_edge_definition_inner(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        ed: EdgeDefId,
        edited: &mut bool,
    ) -> Result<()> {
        // Make sure both SmVertices are in place before proceeding.
        let start_pv = self.edge_def_start_proto_vertex(ed)?;
        let end_pv = self.edge_def_end_proto_vertex(ed)?;
        let start_pv = self.pv_live(start_pv)?;
        let end_pv = self.pv_live(end_pv)?;

        let null = || SolidError::null_pointer(OP_IMPRINT_ED);
        let start_vtx1 = self
            .proto_vertex_final_vertex(k, brep, start_pv, 1, ed, true)?
            .ok_or_else(null)?;
        let end_vtx1 = self
            .proto_vertex_final_vertex(k, brep, end_pv, 1, ed, false)?
            .ok_or_else(null)?;
        let start_vtx2 = self
            .proto_vertex_final_vertex(k, brep, start_pv, 2, ed, true)?
            .ok_or_else(null)?;
        let end_vtx2 = self
            .proto_vertex_final_vertex(k, brep, end_pv, 2, ed, false)?
            .ok_or_else(null)?;
        let brep1 = self.which_brep(1);
        let brep2 = self.which_brep(2);

        // not upstream (WR-452, WR-453, #435, #410; opt-in): fit the ends to the proto vertices
        if k.fixes.imprint_trims_edge_to_proto_vertex
            || k.fixes.imprint_extends_edge_to_proto_vertex
        {
            self.fit_edge_def_ends_to_proto_vertices(
                k,
                brep,
                ed,
                start_pv,
                end_pv,
                [(start_vtx1, start_vtx2), (end_vtx1, end_vtx2)],
            )?;
        }

        let d = self.ed_ref(ed)?;
        let type1 = d.src_topo_type(1);
        let type2 = d.src_topo_type(2);

        // First do Brep 1.
        let mut new_e1 = None;
        let mut new_f1 = None;
        let mut old_f1 = None;

        if type1 == PointClassType::Face {
            old_f1 = as_kind(d.src_topo(1), ClassObjectKind::Face);
            let face = old_f1.ok_or_else(null)?;
            let face_tol = brep
                .topology_tolerance(face)
                .ok_or(dangling("face handle is unknown"))?;
            let this_tol = max2(face_tol, d.raw_tolerance());
            let temp_uv = bspline_uv_curve(brep, d.xsect_uv_curve(1))?;
            let curve = curve_of(brep, d.xsect_curve_3d())?.clone();
            let interval = d.xsect_interval();

            // Note, input curves are consumed:
            *edited = true;
            match brep.brep_make_edge_in_face(
                k,
                brep1,
                face,
                start_vtx1,
                end_vtx1,
                curve,
                temp_uv,
                interval,
                OrientType::Same,
                this_tol,
            ) {
                Ok((e, _l, f)) => {
                    new_e1 = e;
                    new_f1 = f;
                }
                Err(e) => {
                    if !e.is_unported() {
                        // Get rid of our curves, which were not consumed.
                        let _ = self.ed_mut(ed)?.destruct();
                    }
                    return Err(e);
                }
            }
            self.ed_mut(ed)?.set_final_edge(1, new_e1);
        } else if type1 == PointClassType::Edge {
            let e = as_kind(d.src_topo(1), ClassObjectKind::Edge);
            self.ed_mut(ed)?.set_final_edge(1, e);
        }

        // Now do Brep 2.
        let d = self.ed_ref(ed)?;
        if type2 == PointClassType::Face {
            // m_pXSectCurve3d went into Brep1. Make a copy for the other Brep (status
            // ignored).
            let copy = ignore_soft(curve_of(brep, d.xsect_curve_3d())?.copy(k))?;

            let old_f2 = as_kind(d.src_topo(2), ClassObjectKind::Face).ok_or_else(null)?;
            let face_tol = brep
                .topology_tolerance(old_f2)
                .ok_or(dangling("face handle is unknown"))?;
            let this_tol = max2(face_tol, d.raw_tolerance());

            let temp_uv = bspline_uv_curve(brep, d.xsect_uv_curve(2))?;
            let interval = d.xsect_interval();
            // a failed copy is a null curve, dereferenced in MakeEdgeInFace
            let copy = copy.ok_or(dangling("copy of the 3D curve failed"))?;
            *edited = true;
            let made = brep.brep_make_edge_in_face(
                k,
                brep2,
                old_f2,
                start_vtx2,
                end_vtx2,
                copy,
                temp_uv,
                interval,
                OrientType::Same,
                this_tol,
            );
            let (new_e2, new_f2) = match made {
                Ok((e, _l, f)) => (e, f),
                Err(e) => {
                    if !e.is_unported() {
                        // Brep1 keeps the curves it consumed; the copy is dropped.
                        let d = self.ed_mut(ed)?;
                        d.set_xsect_curve_3d(None);
                        d.set_xsect_curve_uv(1, None);
                    }
                    return Err(e);
                }
            };
            self.ed_mut(ed)?.set_final_edge(2, new_e2);

            // If a Face was split, move the proto topology now on the new Face.
            if let Some(new_f2) = new_f2 {
                let other_edge = as_kind(self.ed_ref(ed)?.src_topo(1), ClassObjectKind::Edge);
                self.update_faces(k, brep, 2, old_f2, new_f2, new_e2, other_edge, Some(ed))?;
            }
        } else if type2 == PointClassType::Edge {
            let e = as_kind(d.src_topo(2), ClassObjectKind::Edge);
            self.ed_mut(ed)?.set_final_edge(2, e);
        }

        // Deferred:
        if let (Some(new_f1), Some(old_f1)) = (new_f1, old_f1) {
            let other_edge = as_kind(self.ed_ref(ed)?.src_topo(2), ClassObjectKind::Edge);
            self.update_faces(k, brep, 1, old_f1, new_f1, new_e1, other_edge, Some(ed))?;
        }
        Ok(())
    }

    // ---- SmProtoVertex ---------------------------------------------------------------

    /// Resolves the proto vertex `pv` (`Resolve`): resolves each vertex definition,
    /// then deletes, of every pair that can be combined, the redundant one
    /// ([`ProtoTopologyManager::can_combine_vertex_definitions`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects; [`SolidError::Unported`] from
    /// the definitions' resolution and comparisons.
    pub fn resolve_proto_vertex(
        &mut self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
        pv: ProtoVertexId,
    ) -> Result<()> {
        let pv = self.pv_live(Some(pv))?;
        let mut vds: Vec<Option<VertexDefId>> = self
            .proto_vertex(pv)
            .map(|v| v.vertex_definitions().iter().copied().map(Some).collect())
            .unwrap_or_default();

        for slot in &mut vds {
            let Some(vd1) = *slot else { continue };
            let ret = ignore_soft(self.resolve_vertex_definition(k, brep, vd1))?.unwrap_or(0);
            if ret == 1 {
                // Don't use RemoveVertexDefinition(), that removes it from the array.
                self.delete_vertex_definition(vd1);
                *slot = None;
            }
        }
        vds.retain(Option::is_some);
        self.write_vertex_definitions(pv, &vds)?;
        let num_vds = vds.len();

        // No further checks for a single VertexDef.
        if num_vds < 2 {
            return Ok(());
        }

        // Pairwise:
        for ii in 0..num_vds - 1 {
            let Some(vd1) = vds.get(ii).copied().flatten() else {
                continue;
            };
            let mut jj = ii + 1;
            while jj < num_vds {
                let Some(vd2) = vds.get(jj).copied().flatten() else {
                    jj += 1;
                    continue;
                };
                let delete_which =
                    self.can_combine_vertex_definitions_ignored(k, brep, vd1, vd2)?;
                if delete_which == 1 {
                    self.delete_vertex_definition(vd1);
                    if let Some(s) = vds.get_mut(ii) {
                        *s = None;
                    }
                    // We're done with PV[ii].
                    jj = num_vds + 1;
                } else if delete_which == 2 {
                    self.delete_vertex_definition(vd2);
                    if let Some(s) = vds.get_mut(jj) {
                        *s = None;
                    }
                }
                jj += 1;
            }
        }

        vds.retain(Option::is_some);
        self.write_vertex_definitions(pv, &vds)
    }

    /// Sets the vertex definitions listed at `pv`.
    fn write_vertex_definitions(
        &mut self,
        pv: ProtoVertexId,
        vds: &[Option<VertexDefId>],
    ) -> Result<()> {
        let v = self
            .proto_vertex_mut(pv)
            .ok_or(dangling("proto vertex is deleted"))?;
        *v.vertex_definitions_mut() = vds.iter().flatten().copied().collect();
        Ok(())
    }

    /// [`ProtoTopologyManager::can_combine_vertex_definitions`] with its status
    /// ignored (the choice stays 0 on its null-pointer return).
    fn can_combine_vertex_definitions_ignored(
        &self,
        k: &Kernel,
        topology: &impl ProtoTopology<O>,
        vd1: VertexDefId,
        vd2: VertexDefId,
    ) -> Result<u64> {
        Ok(ignore_soft(self.can_combine_vertex_definitions(k, topology, vd1, vd2))?.unwrap_or(0))
    }

    /// Finds or creates a vertex definition of the classifications `pc1`, `pc2` at
    /// the proto vertex `pv` (`AddVertexDefinition(rPC1, rPC2, ..)`): an existing
    /// definition on the same two topology objects, or a new one
    /// ([`ProtoTopologyManager::create_vertex_definition`], which may compute the
    /// classifications' parameters) listed at `pv`, whose position becomes the average
    /// of its definitions' and whose tolerance grows to the objects' zone tolerances.
    /// Returns the definition and whether it was created.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when either classification is not on a topology object;
    /// those of [`ProtoTopologyManager::create_vertex_definition`].
    pub fn add_vertex_definition_from_pcs(
        &mut self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
        pv: ProtoVertexId,
        pc1: &mut PointClassification<O>,
        pc2: &mut PointClassification<O>,
    ) -> Result<(VertexDefId, bool)> {
        let topo1 = as_topology(pc1.object()).ok_or(SolidError::null_pointer(OP_ADD_VD))?;
        let topo2 = as_topology(pc2.object()).ok_or(SolidError::null_pointer(OP_ADD_VD))?;

        // no work - VertexDefintion with input Tgt SrcTopos already exists
        if let Some(vd) = self.find_matching_vertex_definition(pv, Some(topo1), Some(topo2))? {
            return Ok((vd, false));
        }

        // create Vertex definition for input PointClassifications
        let vd = self.create_vertex_definition(k, brep, pc1, pc2)?;

        // add VertexDef to ProtoVertex list
        self.vd_mut(vd)?.set_proto_vertex(Some(pv));
        let this = self
            .proto_vertex_mut(pv)
            .ok_or(dangling("proto vertex is deleted"))?;
        this.vertex_definitions_mut().push(vd);

        // ProtoVertex position = Avg(VertexDef positions)
        let vds = this.vertex_definitions().to_vec();
        let mut avg = Point3::new(0.0, 0.0, 0.0);
        for &x in &vds {
            avg += self.vd_ref(x)?.position();
        }
        let position = avg / vds.len() as f64;

        // ProtoVertex ZoneTol3d
        let tol1 = zone_tol_of(k, brep, pc1.object())?;
        let tol2 = zone_tol_of(k, brep, pc2.object())?;
        let this = self
            .proto_vertex_mut(pv)
            .ok_or(dangling("proto vertex is deleted"))?;
        this.set_position(position);
        let tol = max3(this.raw_tolerance(), tol1, tol2);
        this.set_raw_tolerance(tol);
        Ok((vd, true))
    }

    // ---- SmProtoEdge -----------------------------------------------------------------

    /// The definition of the proto edge `pe` identical to the proposed one of the
    /// interval pair `ci1`, `ci2` (`FindMatchingEdgeDefinition`): the same shape
    /// between the proto edge's ends and the same source objects as the intervals'
    /// midpoints. `cc1` is the classification `ci1` belongs to (its curve is the
    /// intersection curve).
    ///
    /// # Errors
    ///
    /// Those of [`ProtoTopologyManager::is_same_edge_definition`];
    /// [`SolidError::InvalidInput`] for a deleted proto edge.
    pub fn find_matching_edge_definition(
        &self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
        pe: ProtoEdgeId,
        cc1: &CurveClassification<O>,
        ci1: &CurveInterval<O>,
        ci2: &CurveInterval<O>,
    ) -> Result<Option<EdgeDefId>> {
        let e = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?;
        let xsect_crv = cc1.curve();
        let ivl = ci1.interval;
        let (start, end) = (e.start_proto_vertex(), e.end_proto_vertex());

        for &this_ed in e.edge_definitions() {
            let (same, identical) = self.is_same_edge_definition(
                k, brep, this_ed, start, end, &ci1.mid, &ci2.mid, xsect_crv, &ivl,
            )?;
            if same && identical {
                return Ok(Some(this_ed));
            }
        }
        Ok(None)
    }

    /// Lists the edge definition `ed` at the proto edge `pe` (`AddEdgeDefinition`):
    /// the first definition sets the proto edge's interval; a later one is
    /// reparameterised to it (reversed when its mid tangent opposes the proto edge's)
    /// when needed. A definition owned by another proto edge is unlisted there.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects or a failing mid evaluation
    /// (upstream then reads an uninitialised tangent); [`SolidError::Unported`] from
    /// the evaluations and the reparameterisation.
    pub fn add_edge_definition(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        pe: ProtoEdgeId,
        ed: EdgeDefId,
    ) -> Result<()> {
        let pe = self.pe_live(Some(pe))?;
        let d = self.ed_ref(ed)?;
        let ivl = d.xsect_interval();
        let owning_pe = d.proto_edge();
        let e = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?;

        if e.num_edge_definitions() == 0 {
            // set ProtoEdge->Interval = EdgeDefinition->Interval
            self.proto_edge_mut(pe)
                .ok_or(dangling("proto edge is deleted"))?
                .set_xsect_interval(ivl);
        } else {
            let pe_ivl = e.xsect_interval();
            let uninit = || dangling("mid tangent is uninitialised (evaluation failed)");
            let pe_tan = ignore_soft(self.proto_edge_evaluate_mid(k, brep, pe, false, true))?
                .and_then(|(_, t)| t)
                .ok_or_else(uninit)?;
            let ed_tan = ignore_soft(self.ed_ref(ed)?.evaluate_mid(k, brep, false, true))?
                .and_then(|(_, t)| t)
                .ok_or_else(uninit)?;

            // remember when ProtoEdge->Tan is opposite to EdgeDef->Tan
            let reverse = pe_tan.dot(ed_tan) < 0.0;

            // when EdgeDef parameter opposes ProtoEdges or Ivls don't match - reparameterize
            if reverse || !ivl.are_same(&pe_ivl) {
                ignore_soft(self.ed_mut(ed)?.reparameterize(k, brep, &pe_ivl, reverse))?;
            }
        }

        // EdgeDef->Owner is not ProtoEdge - remove EdgeDef from its owner.
        if let Some(owner) = owning_pe {
            if owner != pe {
                let o = self
                    .proto_edge_mut(owner)
                    .ok_or(dangling("owning proto edge is deleted"))?;
                let _ = o.remove_edge_definition(ed);
            }
        }

        // add EdgeDef to this ProtoEdge
        self.proto_edge_mut(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .edge_definitions_mut()
            .push(ed);
        self.ed_mut(ed)?.set_proto_edge(Some(pe));
        Ok(())
    }

    /// Adds the definition of the interval pair `ci1`, `ci2` to the proto edge `pe`
    /// unless an identical one exists (`AddEdgeDefinition(crCI1, crCI2)`): a trimmed
    /// copy of the intersection curve over `ci1`'s interval, on the intervals' midpoint
    /// objects, with the larger of the classifications' source tolerances. `cc1`,
    /// `cc2` are the classifications the intervals belong to.
    ///
    /// # Errors
    ///
    /// Those of [`ProtoTopologyManager::find_matching_edge_definition`] and of the
    /// curve copy (`SM_ERR_NULL_POINTER` without a copy);
    /// [`SolidError::InvalidInput`] for a midpoint object that is not topology (its
    /// type is read) or a missing curve.
    pub fn add_edge_definition_from_intervals(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        pe: ProtoEdgeId,
        cc1: &CurveClassification<O>,
        ci1: &CurveInterval<O>,
        cc2: &CurveClassification<O>,
        ci2: &CurveInterval<O>,
    ) -> Result<()> {
        // no work - identical EdgeDefinition already exists
        if self
            .find_matching_edge_definition(k, brep, pe, cc1, ci1, ci2)?
            .is_some()
        {
            return Ok(());
        }

        let mut ivl = ci1.interval;
        let topo1 = as_topology(ci1.mid.object());
        let topo2 = as_topology(ci2.mid.object());
        let tol = max2(cc1.src_zone_tol_3d(), cc2.src_zone_tol_3d());

        // copy and trim the XSectCurve3d to place in new EdgeDefinition
        let mut copy = curve_of(brep, cc1.curve())?.copy(k)?;
        ignore_soft(copy.trim(k, &mut ivl, TrimMode::Notify))?;

        // the new definition reads both objects' types
        let topo1 = topo1.ok_or(dangling("source object is not topology"))?;
        let topo2 = topo2.ok_or(dangling("source object is not topology"))?;
        let c3 = brep
            .insert_curve(copy)
            .ok_or(dangling("curve store is full"))?;
        let new_ed = self.insert_edge_definition(EdgeDefinition::new(
            Some(c3),
            ivl,
            topo1,
            topo2,
            tol,
            None,
            None,
            Some(pe),
        ))?;
        // status ignored
        let _ = ignore_soft(self.add_edge_definition(k, brep, pe, new_ed))?;
        Ok(())
    }

    /// Merges the proto edge `other` into `pe` (`CombineProtoEdge`): the larger
    /// tolerance, `other`'s overlapping proto edges, `other`'s ends moved to `pe`'s
    /// (listing `pe` at `pe`'s ends), `other`'s edge definitions added to `pe` (all
    /// of them, from a copy of the list), and `other`'s coincident proto faces moved
    /// to `pe`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted or null objects that are dereferenced;
    /// those of [`ProtoTopologyManager::add_edge_definition`] that cannot be ignored.
    pub fn combine_proto_edge(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        pe: ProtoEdgeId,
        other: ProtoEdgeId,
    ) -> Result<()> {
        // If they're the same, do nothing.
        if other == pe {
            return Ok(());
        }
        let pe = self.pe_live(Some(pe))?;
        let other = self.pe_live(Some(other))?;
        let o = self
            .proto_edge(other)
            .ok_or(dangling("proto edge is deleted"))?
            .clone();
        let this = self
            .proto_edge_mut(pe)
            .ok_or(dangling("proto edge is deleted"))?;
        this.set_raw_tolerance(max2(this.raw_tolerance(), o.raw_tolerance()));
        this.append_overlapping_pes(o.overlapping_pes());

        let this_start = this.start_proto_vertex();
        let other_start = o.start_proto_vertex();
        if other_start != this_start {
            self.proto_edge_mut(other)
                .ok_or(dangling("proto edge is deleted"))?
                .set_start_proto_vertex(this_start);
            let os = self.pv_live(other_start)?;
            let _ = self
                .proto_vertex_mut(os)
                .ok_or(dangling("proto vertex is deleted"))?
                .remove_proto_edge(other);
            let ts = self.pv_live(this_start)?;
            self.proto_vertex_mut(ts)
                .ok_or(dangling("proto vertex is deleted"))?
                .add_proto_edge(pe);
        }

        let this_end = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .end_proto_vertex();
        let other_end = o.end_proto_vertex();
        if other_end != this_end {
            self.proto_edge_mut(other)
                .ok_or(dangling("proto edge is deleted"))?
                .set_end_proto_vertex(this_end);
            let oe = self.pv_live(other_end)?;
            let _ = self
                .proto_vertex_mut(oe)
                .ok_or(dangling("proto vertex is deleted"))?
                .remove_proto_edge(other);
            let te = self.pv_live(this_end)?;
            self.proto_vertex_mut(te)
                .ok_or(dangling("proto vertex is deleted"))?
                .add_proto_edge(pe);
        }

        // Add in other PE's EdgeDefinitions, and remove them from pOtherPE.
        // AddEdgeDefinition already removes the definition from its previous owner, so
        // walk a copy of the list.
        let other_eds = self
            .proto_edge(other)
            .ok_or(dangling("proto edge is deleted"))?
            .edge_definitions()
            .to_vec();
        for ed in other_eds {
            ignore_soft(self.add_edge_definition(k, brep, pe, ed))?;
            let _ = self
                .proto_edge_mut(other)
                .ok_or(dangling("proto edge is deleted"))?
                .remove_edge_definition(ed);
        }

        // Take over pOtherPE's coincident ProtoFaces. The caller deletes pOtherPE right
        // after this call, and RemoveProtoEdge does not update ProtoFaces, so a
        // ProtoFace would otherwise keep pOtherPE in its coincident proto edges.
        let other_pfs = self
            .proto_edge(other)
            .ok_or(dangling("proto edge is deleted"))?
            .coinc_proto_faces()
            .to_vec();
        for pf in other_pfs {
            let pf_edges = self
                .proto_face_mut(pf)
                .ok_or(dangling("proto face is deleted"))?
                .coinc_proto_edges_mut();
            if let Some(idx) = pf_edges.iter().position(|&x| x == other) {
                pf_edges.remove(idx);
            }
            self.relate_coincident_pf(pe, pf)?;
        }
        self.proto_edge_mut(other)
            .ok_or(dangling("proto edge is deleted"))?
            .coinc_proto_faces_mut()
            .clear();
        Ok(())
    }

    /// Resolves the proto edge `pe` (`Resolve`). A proto edge from a proto vertex to
    /// itself loses its definitions when its first curve is shorter than twice its
    /// tolerance; when it is longer with ends farther apart than that, the start proto
    /// vertex gets a second vertex definition at the curve's end (unless the ends are
    /// already closest to different definitions). Not upstream: unless
    /// [`ProtoTopologyManager::split_self_loops_on_vertices`] (the default), when the
    /// definition to split is on a vertex in either B-rep, the proto edge loses its
    /// definitions instead (source issue #480; see the module notes). Then, of every pair
    /// of definitions that can be combined, the redundant one is deleted. Returns the
    /// curves deleted.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted or null objects that are dereferenced;
    /// those of [`ProtoTopologyManager::create_vertex_definition`] and of the curve
    /// length and ends; [`SolidError::Unported`] from the comparisons.
    pub fn resolve_proto_edge(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        pe: ProtoEdgeId,
    ) -> Result<Vec<O>> {
        let pe = self.pe_live(Some(pe))?;
        let mut deleted = Vec::new();
        let e = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?;
        let eds = e.edge_definitions().to_vec();
        if eds.is_empty() {
            return Ok(deleted);
        }

        // Check for degenerate ProtoEdge: Start and End on same ProtoVertex, and very
        // short.
        let (start, end) = (e.start_proto_vertex(), e.end_proto_vertex());
        let pe_ivl = e.xsect_interval();
        let pe_tol = e.raw_tolerance();
        if start == end {
            let first = *eds.first().ok_or(dangling("no edge definition"))?;
            let int_curve = curve_of(brep, self.ed_ref(first)?.xsect_curve_3d())?;
            let (len, _) = int_curve.approximate_length(k, &pe_ivl, 5, false)?;

            // WR-306 (opt-in): ends that find one vertex definition become one vertex in
            // each B-rep, so a curve shorter than the proto vertex's containment ball is
            // wide is that vertex, not a closed edge on it. Ends on two vertex definitions
            // keep upstream's test against our own tolerance.
            let absorb = k.fixes.degenerate_proto_edge_in_one_vertex;
            let degenerate_len = if absorb {
                let ends = int_curve.ends(k)?;
                let start = self.pv_live(start)?;
                let vd0 = self.closest_vertex_definition(start, ends.start)?;
                if vd0 == self.closest_vertex_definition(start, ends.end)? {
                    let pv = self
                        .proto_vertex(start)
                        .ok_or(dangling("proto vertex is deleted"))?;
                    2.0 * proto_vertex_containment_radius(self, pv)
                } else {
                    2.0 * pe_tol
                }
            } else {
                2.0 * pe_tol
            };

            if len < degenerate_len {
                if absorb {
                    // The edges that end on our proto vertex end on our curve, up to its
                    // length away from the vertex: ImprintIntersections fits the vertex
                    // to them.
                    let start = self.pv_live(start)?;
                    self.proto_vertex_mut(start)
                        .ok_or(dangling("proto vertex is deleted"))?
                        .set_absorbed_proto_edge();
                }
                // remove all of our EdgeDefinitions
                for &ed in &eds {
                    deleted.extend(self.destruct_and_delete(ed));
                }
                self.proto_edge_mut(pe)
                    .ok_or(dangling("proto edge is deleted"))?
                    .clear_edge_definitions();
            } else {
                // Our IntCurve is longer than tolerance. Check start & end points.
                let ends = int_curve.ends(k)?;
                let (pt0, pt1) = (ends.start, ends.end);
                let dist = pt0.distance(pt1);
                if dist > 2.0 * pe_tol {
                    // Check whether there are two VertexDefs, one for each end.
                    let start = self.pv_live(start)?;
                    let mut add_vtx_def = false;
                    let vd0 = self.closest_vertex_definition(start, pt0)?;
                    let num_vds = self
                        .proto_vertex(start)
                        .map_or(0, ProtoVertex::num_vertex_definitions);
                    if num_vds < 2 {
                        add_vtx_def = true;
                    } else {
                        let vd1 = self.closest_vertex_definition(start, pt1)?;
                        if vd0 == vd1 {
                            add_vtx_def = true;
                        }
                    }

                    // Not upstream (source issue #480): the new definition
                    // copies vd0's classifications, so when vd0 is on a vertex in
                    // either B-rep both ends land on that one vertex there while the
                    // other B-rep gets an edge between two vertices. Treat the proto
                    // edge as degenerate instead, as the short-curve branch above does,
                    // unless the caller keeps upstream's split.
                    let on_vertex = match vd0 {
                        Some(vd0) if !self.split_self_loops_on_vertices() => {
                            let v0 = self.vd_ref(vd0)?;
                            v0.src_topo_type(1) == PointClassType::Vertex
                                || v0.src_topo_type(2) == PointClassType::Vertex
                        }
                        _ => false,
                    };
                    if add_vtx_def && on_vertex {
                        for &ed in &eds {
                            deleted.extend(self.destruct_and_delete(ed));
                        }
                        self.proto_edge_mut(pe)
                            .ok_or(dangling("proto edge is deleted"))?
                            .clear_edge_definitions();
                    } else if add_vtx_def {
                        // Create a new VtxDef, and set pVD0 to sPt0, and NewVD to sPt1.
                        let vd0 = vd0.ok_or(dangling("proto vertex has no vertex definition"))?;
                        let v0 = self.vd_ref(vd0)?;
                        let mut pc1 = *v0.brep_point_classif(1);
                        let mut pc2 = *v0.brep_point_classif(2);
                        let created = self.create_vertex_definition(k, brep, &mut pc1, &mut pc2);
                        // the classifications are vd0's own (passed by reference)
                        let v0 = self.vd_mut(vd0)?;
                        *v0.brep_point_classif_mut(1) = pc1;
                        *v0.brep_point_classif_mut(2) = pc2;
                        let new_vd = created?;

                        let v0 = self.vd_mut(vd0)?;
                        v0.set_position(pt0);
                        v0.set_point_parameters(k, brep)?;
                        let nv = self.vd_mut(new_vd)?;
                        nv.set_position(pt1);
                        nv.set_point_parameters(k, brep)?;

                        // status ignored
                        ignore_soft(self.add_vertex_definition(start, new_vd))?;
                    }
                }
            }
        }

        // Now check EdgeDefs.
        let num = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .num_edge_definitions();
        if num < 2 {
            return Ok(deleted);
        }

        // Pairwise (upstream steps the unsigned indices back after a removal; the
        // first index wraps around and back to 0):
        let mut ii: usize = 0;
        loop {
            let size = self.pe_eds_len(pe)?;
            if ii >= size.wrapping_sub(1) {
                break;
            }
            let Some(ed1) = self.pe_ed_at(pe, ii)? else {
                ii = ii.wrapping_add(1);
                continue;
            };
            let mut jj = ii + 1;
            while jj < self.pe_eds_len(pe)? {
                let Some(ed2) = self.pe_ed_at(pe, jj)? else {
                    jj += 1;
                    continue;
                };
                let delete_which =
                    ignore_soft(self.can_combine_edge_definitions(k, brep, ed1, ed2))?.unwrap_or(0);
                if delete_which == 1 {
                    let _ = self
                        .proto_edge_mut(pe)
                        .ok_or(dangling("proto edge is deleted"))?
                        .remove_edge_definition(ed1);
                    deleted.extend(self.destruct_and_delete(ed1));
                    ii = ii.wrapping_sub(1);
                    break;
                } else if delete_which == 2 {
                    let _ = self
                        .proto_edge_mut(pe)
                        .ok_or(dangling("proto edge is deleted"))?
                        .remove_edge_definition(ed2);
                    deleted.extend(self.destruct_and_delete(ed2));
                    jj -= 1;
                }
                jj += 1;
            }
            ii = ii.wrapping_add(1);
        }
        Ok(deleted)
    }

    fn pe_eds_len(&self, pe: ProtoEdgeId) -> Result<usize> {
        Ok(self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .num_edge_definitions())
    }

    fn pe_ed_at(&self, pe: ProtoEdgeId, i: usize) -> Result<Option<EdgeDefId>> {
        Ok(self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .edge_definitions()
            .get(i)
            .copied())
    }

    // ---- SmProtoTopologyManager ------------------------------------------------------

    /// Makes a vertex definition of the classifications `pc1`, `pc2`, neither listed
    /// nor owned (`CreateVertexDefinition`): at the position they agree on
    /// ([`find_position_from_pcs`], which may compute their parameters), with the
    /// largest of their tolerances and the manager's, and a gap of at least the
    /// classifications' and their deviation.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the position cannot be found (upstream then
    /// reads an uninitialised position or gap); those of [`VertexDefinition::new`] and
    /// [`PointClassification::gap_3d`].
    pub fn create_vertex_definition(
        &mut self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
        pc1: &mut PointClassification<O>,
        pc2: &mut PointClassification<O>,
    ) -> Result<VertexDefId> {
        let found = ignore_soft(find_position_from_pcs(k, brep, pc1, pc2))?.ok_or(dangling(
            "position and gap are uninitialised (no position found)",
        ))?;
        let pt = found.position;
        let mut gap = found.deviation;

        let mut tol = max2(pc1.tolerance(), pc2.tolerance());
        tol = max2(tol, self.raw_tolerance());

        gap = max2(gap, pc1.gap_3d(k, brep, None)?);
        gap = max2(gap, pc2.gap_3d(k, brep, None)?);

        let vd = VertexDefinition::new(k, brep, pc1, pc2, pt, tol, gap, None)?;
        self.insert_vertex_definition(vd)
    }

    /// Makes and lists a proto vertex of the classifications `pc1`, `pc2` when both
    /// can be imprinted (`CreateProtoVertex`; `None` otherwise): at the position they
    /// agree on, with a tolerance of at least their deviation, the manager's
    /// tolerance and both objects' zone tolerances.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the position cannot be found (upstream then
    /// reads an uninitialised position or deviation); those of the zone tolerances
    /// and of [`ProtoTopologyManager::new_proto_vertex_with_pcs`].
    pub fn create_proto_vertex(
        &mut self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
        pc1: &mut PointClassification<O>,
        pc2: &mut PointClassification<O>,
    ) -> Result<Option<ProtoVertexId>> {
        // Check inputs
        if !can_imprint(pc1.point_class()) || !can_imprint(pc2.point_class()) {
            return Ok(None);
        }

        let found = ignore_soft(find_position_from_pcs(k, brep, pc1, pc2))?.ok_or(dangling(
            "position and deviation are uninitialised (no position found)",
        ))?;

        let tol1 = zone_tol_of(k, brep, pc1.object())?;
        let tol2 = zone_tol_of(k, brep, pc2.object())?;
        let tol = max4(found.deviation, self.raw_tolerance(), tol1, tol2);

        let pv = self.new_proto_vertex_with_pcs(k, brep, pc1, pc2, found.position, tol)?;
        self.add_proto_vertex(pv);
        Ok(Some(pv))
    }

    /// The listed proto vertex containing `position`, or a new listed one there with
    /// the manager's tolerance (`FindOrCreateProtoVertex`); and whether it was
    /// created.
    ///
    /// # Errors
    ///
    /// Those of [`ProtoTopologyManager::find_proto_vertex`].
    pub fn find_or_create_proto_vertex(
        &mut self,
        position: Point3,
    ) -> Result<(ProtoVertexId, bool)> {
        let (found, _) = self.find_proto_vertex(position)?;
        if let Some(pv) = found {
            return Ok((pv, false));
        }
        let pv = self.new_proto_vertex(position, self.raw_tolerance())?;
        self.add_proto_vertex(pv);
        Ok((pv, true))
    }

    /// The first proto edge between `start` and `end` with a definition of the shape
    /// of the intersection interval `tgt_curve` over `tgt_domain` (`FindProtoEdge`),
    /// and whether that definition also has the midpoints' source objects. The
    /// candidates are `end`'s proto edges that `start` also lists, in `end`'s order.
    ///
    /// # Errors
    ///
    /// Those of [`ProtoTopologyManager::is_same_proto_edge`];
    /// [`SolidError::InvalidInput`] for deleted proto vertices.
    pub fn find_proto_edge(
        &self,
        k: &Kernel,
        brep: &impl ProtoBrep<O>,
        start: Option<ProtoVertexId>,
        end: Option<ProtoVertexId>,
        mid_pc1: &PointClassification<O>,
        mid_pc2: &PointClassification<O>,
        tgt_curve: Option<O>,
        tgt_domain: &Extent1d,
    ) -> Result<(Option<ProtoEdgeId>, bool)> {
        // no work - missing a start or end ProtoVertex
        let (Some(s), Some(e)) = (start, end) else {
            return Ok((None, false));
        };
        let pv1_edges = self
            .proto_vertex(s)
            .ok_or(dangling("proto vertex is deleted"))?
            .proto_edges();
        let pv2_edges = self
            .proto_vertex(e)
            .ok_or(dangling("proto vertex is deleted"))?
            .proto_edges();

        // find all existing PEs between the start and end PVs.
        let common: Vec<ProtoEdgeId> = pv2_edges
            .iter()
            .copied()
            .filter(|x| pv1_edges.contains(x))
            .collect();

        let mut identical = false;
        for pe in common {
            let (same, ident) = self.is_same_proto_edge(
                k, brep, pe, start, end, mid_pc1, mid_pc2, tgt_curve, tgt_domain,
            )?;
            identical = ident;
            if same {
                return Ok((Some(pe), identical));
            }
        }
        Ok((None, identical))
    }

    /// Makes and lists a proto edge from `start` to `end` with one definition of the
    /// interval pair `ci1`, `ci2` (`CreateProtoEdge`): copies of the intersection curve
    /// and of both classifications' first UV curves trimmed to `ci1`'s interval, on the
    /// intervals' midpoint objects, with the manager's tolerance. The proto edge is
    /// listed at both ends. `cc1`, `cc2` are the classifications the intervals belong
    /// to.
    ///
    /// # Errors
    ///
    /// Those of the curve copy and trim (`SM_ERR_NULL_POINTER` without a copy);
    /// `SM_ERR_NULL_POINTER` for a midpoint object that is not topology;
    /// [`SolidError::InvalidInput`] for a missing curve or end proto vertex.
    pub fn create_proto_edge(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        start: Option<ProtoVertexId>,
        end: Option<ProtoVertexId>,
        cc1: &CurveClassification<O>,
        ci1: &CurveInterval<O>,
        cc2: &CurveClassification<O>,
        ci2: &CurveInterval<O>,
    ) -> Result<ProtoEdgeId> {
        // Collect data for a new ProtoEdge.
        let p3d_curve = cc1.curve();
        let _ = cc2.curve() == p3d_curve;

        // Create a copy of the input curve and trim it to this CurveInterval.
        let mut copy = curve_of(brep, p3d_curve)?.copy(k)?;
        let mut domain = ci1.interval;
        copy.trim(k, &mut domain, TrimMode::Notify)?;

        // See about uv curves (copy statuses ignored).
        let mut uv1 = None;
        if let Some(c) = cc1.uv_curve1() {
            uv1 = ignore_soft(curve_of(brep, Some(c))?.copy(k))?;
            if let Some(u) = uv1.as_mut() {
                ignore_soft(u.trim(k, &mut domain, TrimMode::Notify))?;
            }
        }
        let mut uv2 = None;
        if let Some(c) = cc2.uv_curve1() {
            uv2 = ignore_soft(curve_of(brep, Some(c))?.copy(k))?;
            if let Some(u) = uv2.as_mut() {
                ignore_soft(u.trim(k, &mut domain, TrimMode::Notify))?;
            }
        }

        // Make an EdgeDefinition for the new ProtoEdge.
        let topo1 = as_topology(ci1.mid.object()).ok_or(SolidError::null_pointer(OP_CREATE_PE))?;
        let topo2 = as_topology(ci2.mid.object()).ok_or(SolidError::null_pointer(OP_CREATE_PE))?;
        let start_pv = self.pv_live(start)?;
        let end_pv = self.pv_live(end)?;

        let full = || dangling("curve store is full");
        let c3 = brep.insert_curve(copy).ok_or_else(full)?;
        let u1 = match uv1 {
            Some(c) => Some(brep.insert_curve(c).ok_or_else(full)?),
            None => None,
        };
        let u2 = match uv2 {
            Some(c) => Some(brep.insert_curve(c).ok_or_else(full)?),
            None => None,
        };

        // Create it.
        let new_pe = self.new_proto_edge(k, start, end, domain)?;
        let tol = self.raw_tolerance();
        let new_ed = self.insert_edge_definition(EdgeDefinition::new(
            Some(c3),
            domain,
            topo1,
            topo2,
            tol,
            u1,
            u2,
            Some(new_pe),
        ))?;

        // Hook up pointers (status ignored).
        ignore_soft(self.add_edge_definition(k, brep, new_pe, new_ed))?;
        self.add_proto_edge(new_pe);
        for pv in [start_pv, end_pv] {
            self.proto_vertex_mut(pv)
                .ok_or(dangling("proto vertex is deleted"))?
                .add_proto_edge(new_pe);
        }
        Ok(new_pe)
    }

    /// Unlists the edge definition `ed` from its proto edge (not deleted), and with
    /// `remove_owner_if_empty` unlists and deletes that proto edge when it has no
    /// definitions left (`RemoveEdgeDefinition`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects or no proto edge.
    pub fn remove_edge_definition(
        &mut self,
        ed: EdgeDefId,
        remove_owner_if_empty: bool,
    ) -> Result<()> {
        let owner = self.pe_live(self.ed_ref(ed)?.proto_edge())?;

        // status ignored upstream
        let o = self
            .proto_edge_mut(owner)
            .ok_or(dangling("proto edge is deleted"))?;
        let _ = o.remove_edge_definition(ed);

        if remove_owner_if_empty && o.num_edge_definitions() < 1 {
            ignore_soft(self.remove_proto_edge(owner))?;
            // source fix 93efd6d (UB-239): drop the references to it first
            self.forget_proto_edge(owner);
            let _ = self.delete_proto_edge_object(owner);
        }
        Ok(())
    }

    /// Updates the proto topology after the edge `old_edge` of B-rep `which` was split
    /// into `new_edges` (`UpdateEdges`; nothing without an old edge or unless there are
    /// two new edges, one of them `old_edge`). Vertex definitions on the old edge whose
    /// parameter is only on the new edge move to it; edge definitions starting or
    /// ending at the proto vertex of `calling_vd` (the definition being imprinted) that
    /// contain the split point are trimmed there; and edge definitions on the old edge
    /// move to the new edge when their midpoint drops onto the new edge only, or closer
    /// to it.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when neither new edge is `old_edge`;
    /// [`SolidError::InvalidInput`] for deleted or null objects that are dereferenced
    /// (`calling_vd` is, when the edge was split); [`SolidError::Unported`] from the
    /// point solves and drops.
    pub fn update_edges(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        which: i32,
        old_edge: Option<O>,
        new_edges: &[O],
        calling_vd: Option<VertexDefId>,
    ) -> Result<()> {
        // no work - no split edge
        let Some(old_edge) = old_edge else {
            return Ok(());
        };

        // no work - not 2 NewEdges
        let &[e0, e1] = new_edges else {
            return Ok(());
        };
        let new_edge = if e0 == old_edge {
            e1
        } else if e1 == old_edge {
            e0
        } else {
            return Err(SolidError::Upstream {
                operation: OP_UPDATE_EDGES,
                code: SM_ERR_NULL_POINTER,
                entity: None,
            });
        };
        let unknown = || dangling("edge handle is unknown");
        let old_ivl = brep.edge_interval(old_edge).ok_or_else(unknown)?;
        let new_ivl = brep.edge_interval(new_edge).ok_or_else(unknown)?;

        // move VertexDefinitions between split Child1 (pOldEdge) and Child2 (pNewEdge)
        for vd in self.all_vertex_definitions()? {
            // no work - VD is CallingVD
            if Some(vd) == calling_vd {
                continue;
            }
            let v = self.vd_ref(vd)?;

            // no Work pVD SrcTopo is not an Edge
            if v.src_topo_type(which) != PointClassType::Edge {
                continue;
            }
            let edge = as_kind(v.src_topo(which), ClassObjectKind::Edge);

            // no work - pVD->Edge is not Tgt SplitEdge
            if edge != Some(old_edge) {
                continue;
            }

            let t = v.brep_t_param(which);
            let on_old_edge = old_ivl.contains_value(t, old_ivl.scaled_zero());
            let on_new_edge = new_ivl.contains_value(t, new_ivl.scaled_zero());

            // when pVD is only on the new Edge - change its srcTopo to the new Edge
            if on_new_edge && !on_old_edge {
                ignore_soft(self.vd_mut(vd)?.set_src_topo(
                    which,
                    PointClassType::Edge,
                    Some(new_edge),
                ))?;
            }
        }

        // next: move EdgeDefinitions between split Child1 (pOldEdge) and Child2 (pNewEdge)
        let eds = self.all_edge_definitions()?;

        // Get the split point: It's one end of pOldEdge.
        let end_param = if old_ivl.mid() < new_ivl.mid() {
            old_ivl.max()
        } else {
            old_ivl.min()
        };
        let old_curve = edge_curve_of(brep, old_edge)?;
        let split_pt = eval_point(k, curve_of(brep, Some(old_curve))?, end_param)?;

        let calling_vd = calling_vd.ok_or(dangling("calling vertex definition is null"))?;
        let calling_pv = self.vd_ref(calling_vd)?.proto_vertex();
        let tol = self.raw_tolerance();

        // (with WR-533 the upper parts of definitions split below are classified in their
        // turn, appended to the list)
        let mut eds = eds;
        let mut next = 0;
        while let Some(&ed) = eds.get(next) {
            next += 1;
            let d = self.ed_ref(ed)?;
            let this_pe = self.pe_live(d.proto_edge())?;
            let pe = self
                .proto_edge(this_pe)
                .ok_or(dangling("proto edge is deleted"))?;
            let (pe_start, pe_end) = (pe.start_proto_vertex(), pe.end_proto_vertex());

            for at_start in [true, false] {
                // CallingPV is EdgeDef Start (End)
                let end_pv = if at_start { pe_start } else { pe_end };
                if end_pv != calling_pv {
                    continue;
                }
                let split_vtx = self.vd_ref(calling_vd)?.final_vertex(which);
                let tol_3d = xsect_tol_of(k, brep, Some(old_edge), split_vtx)?;

                // Split pt is inside the pED - trim pED
                let d = self.ed_ref(ed)?;
                let (mut do_trim, split_param) =
                    d.is_interior_point(k, brep, split_pt, tol_3d, at_start)?;
                if !do_trim {
                    continue;
                }
                // if we start and end on the same ProtoVertex, make sure we're
                // interior w.r.t. the other end as well.
                if pe_start == pe_end {
                    do_trim = d.is_interior_point(k, brep, split_pt, tol_3d, !at_start)?.0;
                }
                if !do_trim {
                    continue;
                }

                // trim instead of split
                let domain = d.xsect_interval();
                let (min, max) = if at_start {
                    (split_param, domain.max())
                } else {
                    (domain.min(), split_param)
                };
                self.ed_mut(ed)?
                    .set_xsect_interval_min_max(k, brep, min, max)?;

                // Also have to trim the curves: we want these to be in sync.
                let d = self.ed_ref(ed)?;
                let mut domain = d.xsect_interval();
                let (c3, u1, u2) = (d.xsect_curve_3d(), d.xsect_uv_curve(1), d.xsect_uv_curve(2));
                curve_of(brep, c3)?;
                trim_ignored(k, brep, c3, &mut domain)?;
                trim_ignored(k, brep, u1, &mut domain)?;
                trim_ignored(k, brep, u2, &mut domain)?;
            }

            // Now check whether this EdgeDefintion is defined on the split SmEdge.
            let d = self.ed_ref(ed)?;
            if d.src_topo_type(which) != PointClassType::Edge {
                continue;
            }
            let edge = as_kind(d.src_topo(which), ClassObjectKind::Edge);
            if edge != Some(old_edge) {
                continue;
            }

            // Not upstream (WR-533), opt-in `boolean_splits_definition_across_split` (the
            // fork's source fork@f11d0ac): a coincident EdgeDefinition that runs
            // across the split point (an edge along the whole of an arc that another
            // edge's ends split twice in its middle) is split there too, with its
            // ProtoEdge (which finds the calling ProtoVertex at that point), so each part
            // lies on one of the two pieces of the edge. Re-pointed whole to the piece
            // under its midpoint, its ends were not that piece's vertices, and Imprint
            // found no vertex for them. The upper part, a new EdgeDefinition still on the
            // old edge, is classified in its turn.
            if k.fixes.boolean_splits_definition_across_split {
                let split_vtx = self.vd_ref(calling_vd)?.final_vertex(which);
                let tol_3d = xsect_tol_of(k, brep, Some(old_edge), split_vtx)?;
                let ed_ivl = d.xsect_interval();
                let ed_start = d.end_position(k, &*brep, true)?;
                let ed_end = d.end_position(k, &*brep, false)?;
                if split_pt.distance(ed_start) > tol_3d && split_pt.distance(ed_end) > tol_3d {
                    let dropped = curve_of(brep, d.xsect_curve_3d())?.drop_point(
                        k,
                        &ed_ivl,
                        split_pt,
                        None,
                        tol_3d,
                        Some(ed_ivl.mid()),
                        SolverOperation::Minimize,
                    );
                    let at = match dropped {
                        Ok(drop)
                            if drop.success
                                && drop.distance <= tol_3d
                                && ed_ivl.contains_value(drop.parameter, -EFF_ZERO) =>
                        {
                            Some(drop.parameter)
                        }
                        Err(e) if e.is_unported() => return Err(e),
                        _ => None,
                    };
                    if let Some(at) = at {
                        let (_, new_pe) = self.proto_edge_split(k, brep, this_pe, at)?;
                        if let Some(new_pe) = new_pe {
                            let upper = self
                                .proto_edge(new_pe)
                                .ok_or(dangling("proto edge is deleted"))?
                                .edge_definitions()
                                .to_vec();
                            for part in upper {
                                if !eds.contains(&part) {
                                    eds.push(part);
                                }
                            }
                        }
                    }
                }
            }
            let d = self.ed_ref(ed)?;

            // Found a ProtoEdge on the edge that was split: drop the midpoint of the
            // definition to both edges' curves.
            let mid_pt = match d.evaluate_mid(k, brep, true, false) {
                Ok((Some(p), _)) => p,
                Err(e)
                    if e.is_unported()
                        || matches!(
                            e,
                            SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. }
                        ) =>
                {
                    return Err(e)
                }
                _ => Vec3::UNDEF,
            };

            let old_tol = brep.topology_tolerance(old_edge).ok_or_else(unknown)?;
            let this_tol = max2(10.0 * tol, 4.0 * old_tol);
            let sols_old = edge_point_solve(k, brep, old_curve, &old_ivl, mid_pt, this_tol)?;

            let new_tol = brep.topology_tolerance(new_edge).ok_or_else(unknown)?;
            let this_tol = max2(10.0 * tol, 4.0 * new_tol);
            let new_curve = edge_curve_of(brep, new_edge)?;
            let sols_new = edge_point_solve(k, brep, new_curve, &new_ivl, mid_pt, this_tol)?;

            let old_value = sols_old.as_ref().and_then(|s| s.first().copied());
            let new_value = sols_new.as_ref().and_then(|s| s.first().copied());
            if old_value.is_some() || new_value.is_some() {
                match (old_value, new_value) {
                    // Drop to Old failed (and to New succeeded).
                    (None, _) => {
                        self.ed_mut(ed)?
                            .set_src_topo(which, PointClassType::Edge, new_edge);
                    }
                    // Drop to New failed (and to Old succeeded).
                    (_, None) => {}
                    // both drops worked: pick the closer one.
                    (Some(o), Some(n)) => {
                        if n < o {
                            self.ed_mut(ed)?
                                .set_src_topo(which, PointClassType::Edge, new_edge);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Updates the proto topology after the face `old_face` of B-rep `which` was split
    /// by `splitting_edge` into itself and `new_face` (`UpdateFaces`): the faces' UV
    /// domains shrink to their trim curves; vertex and edge definitions on the old face
    /// move to the new face when their UV point is in the new face only; and each proto
    /// face on the old face is assigned to the child face coincident with its other
    /// face (by binormals across `other_edge`, the splitting edge's counterpart in the
    /// other B-rep, or by its coincident proto edges' midpoints), its proto edges on
    /// the other child moving to a new proto face.
    ///
    /// # Errors
    ///
    /// Those of the owning-face domain and classification callbacks and of
    /// [`ProtoBrep::edgeuse_evaluate_binormal`] (other than its ignored statuses);
    /// [`SolidError::InvalidInput`] for deleted objects, unknown handles or a missing
    /// splitting edge.
    pub fn update_faces(
        &mut self,
        k: &Kernel,
        brep: &mut impl ProtoBrep<O>,
        which: i32,
        old_face: O,
        new_face: O,
        splitting_edge: Option<O>,
        other_edge: Option<O>,
        calling_ed: Option<EdgeDefId>,
    ) -> Result<()> {
        let unknown = || dangling("face handle is unknown");

        // Face UVDomains = Min(current FaceUVDomain, UVTrimCurve domain) (statuses
        // ignored)
        let old_domain = brep.face_trim_uv_domain(k, old_face)?;
        let new_domain = brep.face_trim_uv_domain(k, new_face)?;

        if !brep
            .face_uv_domain(old_face)
            .ok_or_else(unknown)?
            .is_contained_by(&old_domain, 0.0)
        {
            brep.set_face_uv_domain(old_face, old_domain)
                .ok_or_else(unknown)?;
        }
        if !brep
            .face_uv_domain(new_face)
            .ok_or_else(unknown)?
            .is_contained_by(&new_domain, 0.0)
        {
            brep.set_face_uv_domain(new_face, new_domain)
                .ok_or_else(unknown)?;
        }

        // move VertexDefinitions between split Child1 (pOldFace) and Child2 (pNewFace)
        for vd in self.all_vertex_definitions()? {
            let v = self.vd_ref(vd)?;
            if v.src_topo_type(which) != PointClassType::Face {
                continue;
            }
            let face = as_kind(v.src_topo(which), ClassObjectKind::Face);
            if face != Some(old_face) {
                continue;
            }

            let uv = v.brep_uv_param(which);
            let zone_tol = zone_tol_for_xsect_result(k, brep, old_face, new_face)?;
            let which_face = classify_param_on_faces(
                k,
                brep,
                uv,
                zone_tol,
                old_face,
                &old_domain,
                new_face,
                &new_domain,
            )?;
            if which_face == 2 {
                // move the VertexDefinition
                ignore_soft(self.vd_mut(vd)?.set_src_topo(
                    which,
                    PointClassType::Face,
                    Some(new_face),
                ))?;
            }
        }

        // move EdgeDefinitions between split Child1 (pOldFace) and Child2 (pNewFace)
        for ed in self.all_edge_definitions()? {
            if Some(ed) == calling_ed {
                continue;
            }
            let d = self.ed_ref(ed)?;
            let in_face = as_kind(d.src_topo(which), ClassObjectKind::Face);
            if in_face != Some(old_face) {
                continue;
            }

            let mut face_uv = None;
            let uv_curve = d.xsect_uv_curve(which);
            let mid_param = d.xsect_interval().mid();
            if uv_curve.is_some() {
                if let Some(p) =
                    ignore_soft(curve_of(brep, uv_curve)?.evaluate_point(k, mid_param))?
                {
                    face_uv = Some(Point2::new(p.x, p.y));
                }
            }
            if face_uv.is_none() {
                // drop a 3d MidPoint to the surface.
                let surface = brep.face_surface(old_face).ok_or_else(unknown)?;
                let mid_pt = match d.evaluate_mid(k, brep, true, false) {
                    Ok((Some(p), _)) => p,
                    Err(e) if e.is_unported() => return Err(e),
                    _ => Vec3::UNDEF,
                };
                face_uv = ignore_soft(surface_drop_point(k, brep, surface, mid_pt))?.flatten();
            }
            let Some(face_uv) = face_uv else { continue };

            let zone_tol = zone_tol_for_xsect_result(k, brep, old_face, new_face)?;
            let which_face = classify_param_on_faces(
                k,
                brep,
                face_uv,
                zone_tol,
                old_face,
                &old_domain,
                new_face,
                &new_domain,
            )?;
            if which_face == 2 && in_face == Some(old_face) {
                self.ed_mut(ed)?
                    .set_src_topo(which, PointClassType::Face, new_face);
            }
        }

        // See if ProtoFaces on the split face need to be split as well (the faces
        // this loop creates are not processed).
        let mut binorm_old = Vec3::UNDEF;
        let mut binorm_new = Vec3::UNDEF;
        let mut binorm_other = Vec3::UNDEF;
        let mut got_binormals = false;
        let pfs = self.proto_faces().to_vec();
        for pf in pfs {
            let f = self
                .proto_face(pf)
                .ok_or(dangling("proto face is deleted"))?;
            let Some(face) = as_kind(f.src_topo(which), ClassObjectKind::Face) else {
                continue;
            };
            if face != old_face {
                continue;
            }

            let other_face = as_kind(f.src_topo(3 - which), ClassObjectKind::Face);
            let eu = match (other_face, other_edge) {
                (Some(of), Some(oe)) => brep.edge_edgeuse_of_face(oe, of)?,
                _ => None,
            };
            let Some(other_face) = other_face else {
                continue;
            };
            let mut did_reclassify = false;
            let mut which_face = 0;

            if let Some(eu) = eu {
                if !got_binormals {
                    let se = splitting_edge.ok_or(dangling("splitting edge is null"))?;
                    if let Some(this_eu) = brep.edge_edgeuse_of_face(se, old_face)? {
                        let mid = brep
                            .edge_interval(se)
                            .ok_or(dangling("edge handle is unknown"))?
                            .mid();
                        binorm_old = edgeuse_evaluate_binormal(k, brep, this_eu, mid, binorm_old)?;
                        if let Some(this_eu) = brep.edge_edgeuse_of_face(se, new_face)? {
                            binorm_new =
                                edgeuse_evaluate_binormal(k, brep, this_eu, mid, binorm_new)?;
                            got_binormals = true;
                        }
                    }
                }
                let oe = other_edge.ok_or(dangling("other edge is null"))?;
                let mid = brep
                    .edge_interval(oe)
                    .ok_or(dangling("edge handle is unknown"))?
                    .mid();
                binorm_other = edgeuse_evaluate_binormal(k, brep, eu, mid, binorm_other)?;
                let dot_old = binorm_old.dot(binorm_other);
                let dot_new = binorm_new.dot(binorm_other);
                which_face = if dot_old > 0.0 && dot_new < 0.0 {
                    1
                } else if dot_old < 0.0 && dot_new > 0.0 {
                    2
                } else {
                    0
                };
                if which_face == 1 || which_face == 2 {
                    did_reclassify = true;
                }
            }

            let mut coinc_pes: Vec<Option<ProtoEdgeId>> = self
                .proto_face(pf)
                .ok_or(dangling("proto face is deleted"))?
                .coinc_proto_edges()
                .iter()
                .copied()
                .map(Some)
                .collect();

            if !did_reclassify {
                for &coinc_pe in coinc_pes.iter().flatten() {
                    let mid_pt = match self.proto_edge_evaluate_mid(k, brep, coinc_pe, true, false)
                    {
                        Ok((Some(p), _)) => p,
                        Err(e) if e.is_unported() => return Err(e),
                        _ => Vec3::UNDEF,
                    };
                    let surface = brep.face_surface(old_face).ok_or_else(unknown)?;
                    let Some(face_uv) =
                        ignore_soft(surface_drop_point(k, brep, surface, mid_pt))?.flatten()
                    else {
                        continue;
                    };
                    let zone_tol = zone_tol_for_xsect_result(k, brep, old_face, new_face)?;
                    which_face = classify_param_on_faces(
                        k,
                        brep,
                        face_uv,
                        zone_tol,
                        old_face,
                        &old_domain,
                        new_face,
                        &new_domain,
                    )?;
                    if which_face == 1 || which_face == 2 {
                        did_reclassify = true;
                        break;
                    }
                }
            }
            if !did_reclassify {
                continue;
            }

            let this_face = if which_face == 1 { old_face } else { new_face };
            let that_face = if which_face == 1 { new_face } else { old_face };
            self.proto_face_mut(pf)
                .ok_or(dangling("proto face is deleted"))?
                .set_src_topo(which, PointClassType::Face, Some(this_face));

            // sort the coincident proto edges into those that stay and those to move
            let mut to_move = Vec::new();
            for slot in &mut coinc_pes {
                let Some(pe) = *slot else { continue };
                let mut in_this_face = true;
                let e = self
                    .proto_edge(pe)
                    .ok_or(dangling("proto edge is deleted"))?;
                for &ed in e.edge_definitions() {
                    let d = self.ed_ref(ed)?;
                    match d.src_topo_type(which) {
                        PointClassType::Face => {
                            if let Some(f) = as_kind(d.src_topo(which), ClassObjectKind::Face) {
                                if f != this_face {
                                    in_this_face = false;
                                }
                            }
                            break;
                        }
                        PointClassType::Edge => {
                            if let Some(e) = as_kind(d.src_topo(which), ClassObjectKind::Edge) {
                                if !brep.edge_is_connected_to_face(e, this_face)? {
                                    in_this_face = false;
                                }
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                if !in_this_face {
                    to_move.push(pe);
                    *slot = None;
                }
            }
            let staying: Vec<ProtoEdgeId> = coinc_pes.into_iter().flatten().collect();

            if !to_move.is_empty() {
                if staying.is_empty() {
                    // All PEs were moved: just update this ProtoFace.
                    self.proto_face_mut(pf)
                        .ok_or(dangling("proto face is deleted"))?
                        .set_src_topo(which, PointClassType::Face, Some(new_face));
                } else if which == 1 {
                    // Make a new ProtoFace
                    self.create_proto_face(Some(that_face), Some(other_face), &to_move)?;
                } else {
                    self.create_proto_face(Some(other_face), Some(that_face), &to_move)?;
                }
            }
        }
        Ok(())
    }

    /// Whether two vertex definitions can be combined, and which to delete
    /// (`CanCombineVertexDefinitions`; 0 neither, 1 the first, 2 the second): for
    /// connected source pairs within five times their tolerances of each other, the
    /// one on higher-dimensional topology; otherwise, when both are on the same
    /// vertex in either B-rep or on the same objects in both, the one with the larger
    /// gap.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a definition without a topology source object; those
    /// of the `topology` walks of [`are_connected`]; [`SolidError::InvalidInput`] for
    /// deleted definitions.
    pub fn can_combine_vertex_definitions(
        &self,
        k: &Kernel,
        topology: &impl ProtoTopology<O>,
        vd1: VertexDefId,
        vd2: VertexDefId,
    ) -> Result<u64> {
        let v1 = self.vd_ref(vd1)?;
        let v2 = self.vd_ref(vd2)?;
        let null = || SolidError::null_pointer(OP_COMBINE_VD);
        let t11 = v1.src_topo(1).ok_or_else(null)?;
        let t12 = v1.src_topo(2).ok_or_else(null)?;
        let t21 = v2.src_topo(1).ok_or_else(null)?;
        let t22 = v2.src_topo(2).ok_or_else(null)?;

        // If both VDs are on the same Vertex in either Brep, then definitely combine.
        let mut force_combine = false;
        let is_vertex = |t: O| t.class_kind() == ClassObjectKind::Vertex;
        if is_vertex(t11) && t11 == t21 {
            force_combine = true;
        }
        if is_vertex(t12) && t12 == t22 {
            force_combine = true;
        }

        // Now, if they're sitting on the same topology in both Breps, combine.
        if t11 == t21 && t12 == t22 {
            force_combine = true;
        }

        // If either pair is not connected in their Brep, can't combine.
        if !are_connected(topology, Some(t11), Some(t21))? {
            return Ok(0);
        }
        if !are_connected(topology, Some(t12), Some(t22))? {
            return Ok(0);
        }

        // Check distance: if they're too far apart, we can't combine them.
        let dist = v1.position().distance(v2.position());
        let mut tol = v1.tolerance(k) + v2.tolerance(k);
        tol *= VD_TOL_FACTOR;
        if dist > tol {
            return Ok(0);
        }

        // get rid of the ones on the higher-dimensional Brep objects.
        let dim11 = topo_dimension(t11);
        let dim12 = topo_dimension(t12);
        let dim21 = topo_dimension(t21);
        let dim22 = topo_dimension(t22);

        let mut can_delete = 0;
        if dim11 < dim21 && dim12 <= dim22 {
            can_delete = 2;
        }
        if dim21 < dim11 && dim22 <= dim12 {
            can_delete = 1;
        }
        if dim12 < dim22 && dim11 <= dim21 {
            can_delete = 2;
        }
        if dim22 < dim12 && dim21 <= dim11 {
            can_delete = 1;
        }

        // Keep the one with the tighter gap.
        if force_combine && can_delete == 0 {
            can_delete = if v1.gap() > v2.gap() { 1 } else { 2 };
        }
        Ok(can_delete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::intersect::point_class::{ClassGeometry, ClassObjectKind};
    use crate::surface::surface::Surface;
    use crate::surface::volume::Volume;
    use crate::topology::Brep;

    /// Pool handles: an index and a kind.
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(usize, ClassObjectKind);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            self.1
        }
    }

    #[derive(Default)]
    struct Geo {
        curves: Vec<Curve>,
        edge_ivl: Vec<(usize, Extent1d, f64, usize)>,
        points: Vec<Point3>,
    }

    impl ClassGeometry<H> for Geo {
        fn curve(&self, o: H) -> Option<&Curve> {
            (o.1 == ClassObjectKind::Curve)
                .then(|| self.curves.get(o.0))
                .flatten()
        }
        fn surface(&self, _o: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _o: H) -> Option<&Volume> {
            None
        }
        fn point(&self, o: H) -> Option<Point3> {
            (o.1 == ClassObjectKind::Point)
                .then(|| self.points.get(o.0))
                .flatten()
                .copied()
        }
        fn classification_vertex_point(&self, vertex: H) -> Option<Point3> {
            (vertex.1 == ClassObjectKind::Vertex)
                .then(|| self.points.get(vertex.0))
                .flatten()
                .copied()
        }
    }

    impl ProtoGeometry<H> for Geo {
        fn curve_mut(&mut self, o: H) -> Option<&mut Curve> {
            (o.1 == ClassObjectKind::Curve)
                .then(|| self.curves.get_mut(o.0))
                .flatten()
        }
    }

    /// The bare stand-ins: no uses, no B-rep; an edge's start vertex dereferences its
    /// missing primary edge use.
    impl ProtoTopology<H> for Geo {
        fn vertex_use_faces(&self, _vertex: H) -> Result<Vec<Option<H>>> {
            Ok(Vec::new())
        }
        fn edge_use_faces(&self, _edge: H) -> Result<Vec<Option<H>>> {
            Ok(Vec::new())
        }
        fn edge_start_vertex(&self, _edge: H) -> Result<Option<H>> {
            Err(SolidError::invalid_input("test", "no primary edge use"))
        }
        fn edge_other_vertex(&self, _edge: H, _vertex: Option<H>) -> Result<Option<H>> {
            Ok(None)
        }
        fn vertex_point(&self, _vertex: H) -> Result<Point3> {
            Ok(Point3::new(0.0, 0.0, 0.0))
        }
        fn edge_brep(&self, _edge: H) -> Result<Option<crate::geom::attribute::ObjectKey>> {
            Ok(None)
        }
        fn edge_faces(&self, _edge: H) -> Result<Vec<H>> {
            Ok(Vec::new())
        }
    }

    impl ProtoBrep<H> for Geo {
        fn brep_make_edge_in_face(
            &mut self,
            _k: &Kernel,
            _b: H,
            _f: H,
            _v1: H,
            _v2: H,
            _c: Curve,
            _uv: Option<Curve>,
            _ivl: Extent1d,
            _o: OrientType,
            _tol: f64,
        ) -> Result<(Option<H>, Option<H>, Option<H>)> {
            Err(SolidError::unported(
                "SmBrep::MakeEdgeInFace (no B-rep behind the test handles)",
                "harness:proto-test-brep-edit",
            ))
        }
        fn edgeuse_evaluate_binormal(&mut self, _k: &Kernel, _eu: H, _t: f64) -> Result<Vec3> {
            Err(SolidError::unported(
                "SmEdgeuse::EvaluateBinormal (no B-rep behind the test handles)",
                "harness:proto-test-brep-edit",
            ))
        }
        fn brep_squeeze_edge(&mut self, _k: &mut Kernel, _b: H, _e: H, _v: H) -> Result<()> {
            Err(SolidError::unported(
                "SmBrep::SqueezeEdge (no B-rep behind the test handles)",
                "harness:proto-test-brep-edit",
            ))
        }
        fn edge_interval(&self, edge: H) -> Option<Extent1d> {
            self.edge_ivl.iter().find(|e| e.0 == edge.0).map(|e| e.1)
        }
        fn edge_curve(&self, edge: H) -> Option<Option<H>> {
            self.edge_ivl
                .iter()
                .find(|e| e.0 == edge.0)
                .map(|e| Some(H(e.3, ClassObjectKind::Curve)))
        }
        fn topology_tolerance(&self, topo: H) -> Option<f64> {
            Some(
                self.edge_ivl
                    .iter()
                    .find(|e| e.0 == topo.0)
                    .map_or(0.0, |e| e.2),
            )
        }
        fn face_uv_domain(&self, _face: H) -> Option<Extent2d> {
            None
        }
        fn face_trim_uv_domain(&mut self, _k: &Kernel, _face: H) -> Result<Extent2d> {
            Err(dangling("test handle has no owning face"))
        }
        fn face_point_classify_uv(
            &mut self,
            _k: &Kernel,
            _face: H,
            _uv: Point2,
            _tolerance: f64,
        ) -> Result<PointClassType> {
            Err(dangling("test handle has no owning face"))
        }
        fn set_face_uv_domain(&mut self, _face: H, _domain: Extent2d) -> Option<()> {
            None
        }
        fn face_surface(&self, _face: H) -> Option<Option<H>> {
            None
        }
        fn edge_edgeuse_of_face(&self, _edge: H, _face: H) -> Result<Option<H>> {
            Ok(None)
        }
        fn edge_is_connected_to_face(&self, _edge: H, _face: H) -> Result<bool> {
            Ok(false)
        }
        fn insert_curve(&mut self, curve: Curve) -> Option<H> {
            self.curves.push(curve);
            Some(H(self.curves.len() - 1, ClassObjectKind::Curve))
        }
        fn vertex_common_edges(&self, _v1: H, _v2: H) -> Result<Vec<H>> {
            Ok(Vec::new())
        }
        fn face_curve_on_classify(
            &self,
            _k: &Kernel,
            _face: H,
            _do_point_classify: bool,
            _cc: &mut CurveClassification<H>,
        ) -> Result<bool> {
            Ok(false)
        }
        fn brep_point_3d_classify(
            &mut self,
            _k: &Kernel,
            _brep: H,
            point: Point3,
            tolerance: f64,
            _boundary_intersections: bool,
        ) -> Result<PointClassification<H>> {
            let idx = self.points.len();
            self.points.push(point);
            let v = H(idx, ClassObjectKind::Vertex);
            let mut pc = PointClassification::new(tolerance);
            pc.set_class_object(PointClassType::Vertex, Some(v));
            Ok(pc)
        }
    }

    fn line(p: [f64; 3], q: [f64; 3]) -> Curve {
        Curve::from(
            Line::create_line_segment(3, Vec3::from_array(p), Vec3::from_array(q), None).unwrap(),
        )
    }

    const R0: H = H(10, ClassObjectKind::Region);
    const R1: H = H(11, ClassObjectKind::Region);
    const L0: H = H(12, ClassObjectKind::Loop);

    fn manager() -> ProtoTopologyManager<H> {
        let b = H(100, ClassObjectKind::Other);
        let e = Brep::new();
        ProtoTopologyManager::new(&Kernel::default(), (b, &e), (b, &e), b, 1e-5, 1e-3, -1.0)
            .unwrap()
    }

    fn region_pc(o: H) -> PointClassification<H> {
        let mut pc = PointClassification::new(1e-5);
        pc.set_class_object(PointClassType::Region, Some(o));
        pc
    }

    #[test]
    fn combine_vertex_definitions_by_topology() {
        let k = Kernel::default();
        let geo = Geo::default();
        let mut m = manager();
        let p = Point3::new(1.0, 2.0, 3.0);
        let mk = |m: &mut ProtoTopologyManager<H>, a: H, b: H, gap: f64| {
            let vd =
                VertexDefinition::new(&k, &geo, &region_pc(a), &region_pc(b), p, 1e-5, gap, None)
                    .unwrap();
            m.insert_vertex_definition(vd).unwrap()
        };
        let a = mk(&mut m, R0, R1, 2e-6);
        let b = mk(&mut m, R0, R1, 1e-6);
        let c = mk(&mut m, R0, L0, 1e-6);
        // same topology in both: the larger gap goes
        assert_eq!(m.can_combine_vertex_definitions(&k, &geo, a, b).unwrap(), 1);
        assert_eq!(m.can_combine_vertex_definitions(&k, &geo, b, a).unwrap(), 2);
        // a region and a loop are not connected
        assert_eq!(m.can_combine_vertex_definitions(&k, &geo, a, c).unwrap(), 0);

        // resolving a proto vertex with both keeps one
        let pv = m.new_proto_vertex(p, 1e-5).unwrap();
        m.add_proto_vertex(pv);
        m.add_vertex_definition(pv, a).unwrap();
        m.add_vertex_definition(pv, b).unwrap();
        m.add_vertex_definition(pv, c).unwrap();
        m.resolve_proto_vertex(&k, &geo, pv).unwrap();
        assert_eq!(m.proto_vertex(pv).unwrap().vertex_definitions(), &[b, c]);
        assert!(m.vertex_def(a).is_none());
    }

    /// A proto edge from a proto vertex back to itself along an open curve 5e-5 long
    /// (longer than twice the 1e-5 tolerance, ends as far apart), the proto vertex
    /// holding one vertex definition with `pc1` and `pc2`, resolved with
    /// `split_self_loops_on_vertices` set to `split`. Returns the manager, the proto
    /// vertex and edge, and what `resolve_proto_edge` returned.
    fn resolve_self_loop(
        pc1: &PointClassification<H>,
        pc2: &PointClassification<H>,
        split: bool,
    ) -> (
        ProtoTopologyManager<H>,
        ProtoVertexId,
        ProtoEdgeId,
        Result<Vec<H>>,
    ) {
        let k = Kernel::default();
        let mut geo = Geo::default();
        geo.curves.push(line([0.0, 0.0, 0.0], [5e-5, 0.0, 0.0]));
        let c0 = H(0, ClassObjectKind::Curve);
        let mut m = manager();
        assert!(
            m.split_self_loops_on_vertices(),
            "upstream's split is the default"
        );
        m.set_split_self_loops_on_vertices(split);
        let p = Point3::new(0.0, 0.0, 0.0);
        let pv = m.new_proto_vertex(p, 1e-5).unwrap();
        m.add_proto_vertex(pv);
        let vd = VertexDefinition::new(&k, &geo, pc1, pc2, p, 1e-5, 1e-9, None).unwrap();
        let vd = m.insert_vertex_definition(vd).unwrap();
        m.add_vertex_definition(pv, vd).unwrap();

        let mut cc = CurveClassification::new(&k);
        cc.set_curve(Some(c0), false);
        let ivl = geo.curves[0].natural_interval();
        let mut ci1 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
        ci1.mid = region_pc(R0);
        let mut ci2 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
        ci2.mid = region_pc(R1);
        let pe = m
            .create_proto_edge(&k, &mut geo, Some(pv), Some(pv), &cc, &ci1, &cc, &ci2)
            .unwrap();
        assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 1);
        let deleted = m.resolve_proto_edge(&k, &mut geo, pe);
        (m, pv, pe, deleted)
    }

    fn vertex_pc() -> PointClassification<H> {
        let mut pc = PointClassification::new(1e-5);
        pc.set_class_object(PointClassType::Vertex, Some(H(20, ClassObjectKind::Vertex)));
        pc
    }

    /// upstream: the proto vertex gets a second definition for the curve's end.
    /// These pool stand-ins have no surfaces or vertex records, so
    /// `CreateVertexDefinition` stops (no position found, UB-115; or the unported
    /// `SmVertex::GetPoint`): reaching it shows the split branch ran, edge kept.
    fn assert_split_attempted(
        pc1: &PointClassification<H>,
        pc2: &PointClassification<H>,
        split: bool,
    ) {
        let (m, pv, pe, result) = resolve_self_loop(pc1, pc2, split);
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no position found") || msg.contains("SmVertex::GetPoint"),
            "{msg}"
        );
        assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 1);
        assert_eq!(m.proto_vertex(pv).unwrap().vertex_definitions().len(), 1);
    }

    #[test]
    fn resolve_self_loop_off_a_vertex_still_splits() {
        for split in [true, false] {
            assert_split_attempted(&region_pc(R0), &region_pc(R1), split);
        }
    }

    #[test]
    fn resolve_self_loop_on_a_vertex_splits_by_default() {
        // upstream's behaviour, which this unit's difftest compares
        assert_split_attempted(&vertex_pc(), &region_pc(R1), true);
        assert_split_attempted(&region_pc(R0), &vertex_pc(), true);
    }

    #[test]
    fn resolve_self_loop_on_a_vertex_is_degenerate() {
        // source issue #480: a definition on a vertex in either B-rep is not
        // split (both ends would be that vertex there); the proto edge is dropped
        for (pc1, pc2) in [(vertex_pc(), region_pc(R1)), (region_pc(R0), vertex_pc())] {
            let (m, pv, pe, deleted) = resolve_self_loop(&pc1, &pc2, false);
            assert_eq!(
                deleted.unwrap().len(),
                1,
                "the definition's curve copy is deleted"
            );
            assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 0);
            assert_eq!(m.proto_vertex(pv).unwrap().vertex_definitions().len(), 1);
        }
    }

    #[test]
    fn resolve_self_loop_inside_one_vertex_is_absorbed() {
        // source issue #1058 (WR-306): both ends of the 5e-5 curve find the one
        // vertex definition, and the 0.01 capture distance makes the proto vertex's
        // containment ball far wider than the curve: with the opt-in the proto edge is
        // that vertex (dropped, the proto vertex marked); without it, upstream's split.
        for absorb in [false, true] {
            let mut k = Kernel::default();
            k.fixes.degenerate_proto_edge_in_one_vertex = absorb;
            let mut geo = Geo::default();
            geo.curves.push(line([0.0, 0.0, 0.0], [5e-5, 0.0, 0.0]));
            let c0 = H(0, ClassObjectKind::Curve);
            let mut m = manager();
            m.set_capture_distance(0.01);
            let p = Point3::new(0.0, 0.0, 0.0);
            let pv = m.new_proto_vertex(p, 1e-5).unwrap();
            m.add_proto_vertex(pv);
            let (pc1, pc2) = (region_pc(R0), region_pc(R1));
            let vd = VertexDefinition::new(&k, &geo, &pc1, &pc2, p, 1e-5, 1e-9, None).unwrap();
            let vd = m.insert_vertex_definition(vd).unwrap();
            m.add_vertex_definition(pv, vd).unwrap();

            let mut cc = CurveClassification::new(&k);
            cc.set_curve(Some(c0), false);
            let ivl = geo.curves[0].natural_interval();
            let mut ci1 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
            ci1.mid = region_pc(R0);
            let mut ci2 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
            ci2.mid = region_pc(R1);
            let pe = m
                .create_proto_edge(&k, &mut geo, Some(pv), Some(pv), &cc, &ci1, &cc, &ci2)
                .unwrap();
            let result = m.resolve_proto_edge(&k, &mut geo, pe);
            if absorb {
                assert_eq!(
                    result.unwrap().len(),
                    1,
                    "the definition's curve copy is deleted"
                );
                assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 0);
                assert!(m.proto_vertex(pv).unwrap().absorbed_proto_edge());
            } else {
                // the split branch ran (see `assert_split_attempted`)
                assert!(result.is_err());
                assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 1);
                assert!(!m.proto_vertex(pv).unwrap().absorbed_proto_edge());
            }
        }
    }

    #[test]
    fn proto_edges_create_add_combine_resolve() {
        let k = Kernel::default();
        let mut geo = Geo::default();
        geo.curves.push(line([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]));
        let c0 = H(0, ClassObjectKind::Curve);
        let mut m = manager();
        let v0 = m
            .new_proto_vertex(Point3::new(0.0, 0.0, 0.0), 1e-5)
            .unwrap();
        let v1 = m
            .new_proto_vertex(Point3::new(2.0, 0.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(v0);
        m.add_proto_vertex(v1);

        let mut cc = CurveClassification::new(&k);
        cc.set_curve(Some(c0), false);
        let ivl = geo.curves[0].natural_interval();
        let mut ci1 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
        ci1.mid = region_pc(R0);
        let mut ci2 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
        ci2.mid = region_pc(R1);

        let pe = m
            .create_proto_edge(&k, &mut geo, Some(v0), Some(v1), &cc, &ci1, &cc, &ci2)
            .unwrap();
        assert_eq!(m.proto_edges(), &[pe]);
        assert_eq!(m.proto_vertex(v0).unwrap().proto_edges(), &[pe]);
        assert_eq!(geo.curves.len(), 2);
        let eds = m.proto_edge(pe).unwrap().edge_definitions().to_vec();
        assert_eq!(eds.len(), 1);

        // the same interval pair matches; adding it again changes nothing
        assert_eq!(
            m.find_matching_edge_definition(&k, &geo, pe, &cc, &ci1, &ci2)
                .unwrap(),
            Some(eds[0])
        );
        m.add_edge_definition_from_intervals(&k, &mut geo, pe, &cc, &ci1, &cc, &ci2)
            .unwrap();
        assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 1);

        // another source pair adds a definition; resolving keeps both (not connected)
        let mut ci3 = ci2;
        ci3.mid = region_pc(L0);
        m.add_edge_definition_from_intervals(&k, &mut geo, pe, &cc, &ci1, &cc, &ci3)
            .unwrap();
        assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 2);
        assert!(m.resolve_proto_edge(&k, &mut geo, pe).unwrap().is_empty());

        // a second proto edge between the same vertices is found and combined
        let pe2 = m
            .create_proto_edge(&k, &mut geo, Some(v1), Some(v0), &cc, &ci1, &cc, &ci2)
            .unwrap();
        let (found, identical) = m
            .find_proto_edge(
                &k,
                &geo,
                Some(v0),
                Some(v1),
                &ci1.mid,
                &ci2.mid,
                Some(c0),
                &ivl,
            )
            .unwrap();
        assert_eq!(found, Some(pe));
        assert!(identical);
        // the other proto edge's coincident proto face moves to pe
        let pf = m.new_proto_face(None, None, &[]).unwrap();
        m.add_proto_face(pf);
        m.relate_coincident_pf(pe2, pf).unwrap();
        m.combine_proto_edge(&k, &mut geo, pe, pe2).unwrap();
        assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 3);
        assert_eq!(m.proto_face(pf).unwrap().coinc_proto_edges(), &[pe]);
        assert_eq!(m.proto_edge(pe).unwrap().coinc_proto_faces(), &[pf]);
        assert!(m.proto_edge(pe2).unwrap().coinc_proto_faces().is_empty());

        // removing the only definition of an edge deletes it
        let e2 = m.proto_edge(pe2).unwrap().edge_definitions().to_vec();
        assert!(e2.is_empty());
        let pe3 = m.new_proto_edge(&k, Some(v0), Some(v1), ivl).unwrap();
        m.add_proto_edge(pe3);
        let last = *m.proto_edge(pe).unwrap().edge_definitions().last().unwrap();
        m.add_edge_definition(&k, &mut geo, pe3, last).unwrap();
        assert_eq!(m.edge_def(last).unwrap().proto_edge(), Some(pe3));
        m.remove_edge_definition(last, true).unwrap();
        assert!(m.proto_edge(pe3).is_none());
    }

    #[test]
    fn find_or_create_and_create_proto_vertex() {
        let k = Kernel::default();
        let geo = Geo::default();
        let mut m = manager();
        let (pv, created) = m
            .find_or_create_proto_vertex(Point3::new(1.0, 0.0, 0.0))
            .unwrap();
        assert!(created);
        assert_eq!(m.proto_vertices(), &[pv]);
        // the same position finds the existing proto vertex
        assert_eq!(
            m.find_or_create_proto_vertex(Point3::new(1.0, 0.0, 0.0))
                .unwrap(),
            (pv, false)
        );
        assert_eq!(m.proto_vertices(), &[pv]);

        // curves cannot be imprinted
        let mut pc1 = PointClassification::new(1e-5);
        pc1.set_class_object(PointClassType::Curve, Some(H(0, ClassObjectKind::Curve)));
        let mut pc2 = region_pc(R0);
        assert_eq!(
            m.create_proto_vertex(&k, &geo, &mut pc1, &mut pc2).unwrap(),
            None
        );
        // regions have no object point: undefined upstream
        let mut pc1 = region_pc(R1);
        assert!(matches!(
            m.create_proto_vertex(&k, &geo, &mut pc1, &mut pc2),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn update_edges_moves_definitions_to_the_new_edge() {
        let k = Kernel::default();
        let mut geo = Geo::default();
        geo.curves.push(line([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]));
        let ivl = geo.curves[0].natural_interval();
        let (a, b) = (ivl.min(), ivl.max());
        let mid = ivl.mid();
        let e_old = H(20, ClassObjectKind::Edge);
        let e_new = H(21, ClassObjectKind::Edge);
        geo.edge_ivl
            .push((20, Extent1d::from_min_max(a, mid), 1e-6, 0));
        geo.edge_ivl
            .push((21, Extent1d::from_min_max(mid, b), 1e-6, 0));

        let mut m = manager();
        let mut pc = PointClassification::new(1e-5);
        pc.set_class_object(PointClassType::Edge, Some(e_old));
        pc.set_t_param(ivl.evaluate(0.75), false);
        let calling = m
            .insert_vertex_definition(
                VertexDefinition::new(
                    &k,
                    &geo,
                    &region_pc(R0),
                    &region_pc(R1),
                    Point3::new(1.0, 0.0, 0.0),
                    1e-5,
                    1e-6,
                    None,
                )
                .unwrap(),
            )
            .unwrap();
        let moved = m
            .insert_vertex_definition(
                VertexDefinition::new(
                    &k,
                    &geo,
                    &pc,
                    &region_pc(R1),
                    Point3::new(1.5, 0.0, 0.0),
                    1e-5,
                    1e-6,
                    None,
                )
                .unwrap(),
            )
            .unwrap();
        let pv = m
            .new_proto_vertex(Point3::new(1.0, 0.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(pv);
        m.add_vertex_definition(pv, calling).unwrap();
        m.add_vertex_definition(pv, moved).unwrap();

        // nothing to do without a split
        m.update_edges(&k, &mut geo, 1, Some(e_old), &[e_old], Some(calling))
            .unwrap();
        assert!(matches!(
            m.update_edges(&k, &mut geo, 1, Some(e_old), &[e_new, e_new], Some(calling)),
            Err(SolidError::Upstream {
                code: SM_ERR_NULL_POINTER,
                ..
            })
        ));
        m.update_edges(&k, &mut geo, 1, Some(e_old), &[e_old, e_new], Some(calling))
            .unwrap();
        assert_eq!(m.vertex_def(moved).unwrap().src_topo(1), Some(e_new));
    }

    #[test]
    fn edge_definition_split_starts_unowned() {
        // source issue #518: SmEdgeDefinition::Split's new EdgeDefinition
        // starts unowned, so adding it to the upper proto edge does not fail attempting
        // to remove it from the original proto edge.
        let k = Kernel::default();
        let mut geo = Geo::default();
        geo.curves.push(line([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]));
        let c0 = H(0, ClassObjectKind::Curve);
        let mut m = manager();
        let v0 = m
            .new_proto_vertex(Point3::new(0.0, 0.0, 0.0), 1e-5)
            .unwrap();
        let v1 = m
            .new_proto_vertex(Point3::new(2.0, 0.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(v0);
        m.add_proto_vertex(v1);

        let mut cc = CurveClassification::new(&k);
        cc.set_curve(Some(c0), false);
        let ivl = geo.curves[0].natural_interval();
        let mut ci1 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
        ci1.mid = region_pc(R0);
        let mut ci2 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
        ci2.mid = region_pc(R1);

        let pe = m
            .create_proto_edge(&k, &mut geo, Some(v0), Some(v1), &cc, &ci1, &cc, &ci2)
            .unwrap();
        assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 1);

        // Split the proto edge at its midpoint parameter
        let (new_pv, new_pe) = m.proto_edge_split(&k, &mut geo, pe, ivl.mid()).unwrap();
        let new_pe = new_pe.expect("proto edge was split");
        assert!(new_pv.is_some());

        // The lower proto edge retains its edge definition, and the upper proto edge
        // receives the new split edge definition without error.
        assert_eq!(m.proto_edge(pe).unwrap().num_edge_definitions(), 1);
        assert_eq!(m.proto_edge(new_pe).unwrap().num_edge_definitions(), 1);
    }

    /// WR-533 (the fork's source fork@f11d0ac): an edge definition along the
    /// whole of an edge that is split in its middle is split there too with the opt-in,
    /// each part on its own piece; without it the whole definition stays on the old
    /// edge, its midpoint at the split point.
    #[test]
    fn update_edges_splits_a_definition_across_the_split_point() {
        for fixed in [false, true] {
            let mut k = Kernel::default();
            k.fixes.boolean_splits_definition_across_split = fixed;
            let mut geo = Geo::default();
            geo.curves.push(line([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]));
            let c0 = H(0, ClassObjectKind::Curve);
            let ivl = geo.curves[0].natural_interval();
            let (a, b, mid) = (ivl.min(), ivl.max(), ivl.mid());
            let e_old = H(20, ClassObjectKind::Edge);
            let e_new = H(21, ClassObjectKind::Edge);
            geo.edge_ivl
                .push((20, Extent1d::from_min_max(a, mid), 1e-6, 0));
            geo.edge_ivl
                .push((21, Extent1d::from_min_max(mid, b), 1e-6, 0));

            let mut m = manager();
            let v0 = m
                .new_proto_vertex(Point3::new(0.0, 0.0, 0.0), 1e-5)
                .unwrap();
            let v1 = m
                .new_proto_vertex(Point3::new(2.0, 0.0, 0.0), 1e-5)
                .unwrap();
            m.add_proto_vertex(v0);
            m.add_proto_vertex(v1);

            // the definition along the whole curve, on the old edge in B-rep 1
            let mut cc = CurveClassification::new(&k);
            cc.set_curve(Some(c0), false);
            let mut on_edge = PointClassification::new(1e-5);
            on_edge.set_class_object(PointClassType::Edge, Some(e_old));
            on_edge.set_t_param(mid, false);
            let mut ci1 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
            ci1.mid = on_edge;
            let mut ci2 = CurveInterval::with_interval(&k, &ivl, Some(&cc));
            ci2.mid = region_pc(R1);
            let pe = m
                .create_proto_edge(&k, &mut geo, Some(v0), Some(v1), &cc, &ci1, &cc, &ci2)
                .unwrap();
            let ed = m.proto_edge(pe).unwrap().edge_definitions()[0];
            assert_eq!(m.edge_def(ed).unwrap().src_topo(1), Some(e_old));

            // the vertex definition that split the edge at (1, 0, 0)
            let calling = m
                .insert_vertex_definition(
                    VertexDefinition::new(
                        &k,
                        &geo,
                        &region_pc(R0),
                        &region_pc(R1),
                        Point3::new(1.0, 0.0, 0.0),
                        1e-5,
                        1e-6,
                        None,
                    )
                    .unwrap(),
                )
                .unwrap();
            let pv = m
                .new_proto_vertex(Point3::new(1.0, 0.0, 0.0), 1e-5)
                .unwrap();
            m.add_proto_vertex(pv);
            m.add_vertex_definition(pv, calling).unwrap();

            m.update_edges(&k, &mut geo, 1, Some(e_old), &[e_old, e_new], Some(calling))
                .unwrap();
            let mut on: Vec<Option<H>> = m
                .all_edge_definitions()
                .unwrap()
                .into_iter()
                .map(|d| m.edge_def(d).unwrap().src_topo(1))
                .collect();
            on.sort_by_key(|h| h.map(|h| h.0));
            if fixed {
                assert_eq!(on, [Some(e_old), Some(e_new)]);
            } else {
                assert_eq!(on.len(), 1);
            }
        }
    }
}
