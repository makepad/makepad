//! Topology intersector, part 1: the intersector object and its mate maps, the table
//! of pre-existing surface/surface intersections, the shell-vertex and wire/lamina
//! edge intersections, resolving and imprinting the proto topology, the
//! classification fix-ups and the six internal-use intersection methods.
//!
//! Upstream: `source/SMLib/src/SmTopologyIntersector.cpp`,
//! `source/SMLib/inc/SmTopologyIntersector.h`.
//!
//! | Rust                                                   | Upstream                                             |
//! |--------------------------------------------------------|------------------------------------------------------|
//! | [`IntersectionResults`]                                | `SmIntersectionResults`, `SmIntersectionResults()`   |
//! | [`IntersectionTable`]                                  | `SmIntersectionTable`, `SmIntersectionTable()`       |
//! | [`IntersectionTable::destroy`]                         | `~SmIntersectionTable`                               |
//! | [`TopologyIntersector`]                                | `SmTopologyIntersector`                              |
//! | [`TopologyIntersector::destroy`]                       | `~SmTopologyIntersector`                             |
//! | [`TopologyIntersector::clear_existing_ssi`]            | `ClearExistingSSI`                                   |
//! | [`TopologyIntersector::check_if_have_existing_ssi`]    | `CheckIfHaveExistingSSI`                             |
//! | [`TopologyIntersector::context`]                       | `GetContext`                                         |
//! | [`TopologyIntersector::other_mate`]                    | `GetOtherMate`                                       |
//! | [`TopologyIntersector::this_mate`]                     | `GetThisMate`                                        |
//! | [`TopologyIntersector::is_mated_pair`]                 | `IsMatedPair`                                        |
//! | [`TopologyIntersector::brep_mate`]                     | `GetBrepMate`                                        |
//! | [`TopologyIntersector::mapped_brep_objects`]           | `GetMappedBrepObjects`                               |
//! | [`TopologyIntersector::common_edges`]                  | `GetCommonEdges`                                     |
//! | [`TopologyIntersector::common_vertices`]               | `GetCommonVertices`                                  |
//! | `can_imprint` (private)                                | `sm_CanImprint`                                      |
//! | [`find_first_lowest_dimension_sol`]                    | `sm_FindFirstLowestDimensionSol`                     |
//! | [`intersect_shell_vertices`]                           | `sm_IntersectShellVertices`                          |
//! | [`intersect_wire_lamina_edges`]                        | `sm_IntersectWireLaminaEdges`                        |
//! | [`TopologyIntersector::resolve_intersections`]         | `ResolveIntersections`                               |
//! | [`TopologyIntersector::imprint_intersections`]         | `ImprintIntersections`                               |
//! | [`TopologyIntersector::imprint_intersections_reporting_edits`] | `ImprintIntersections`                     |
//! | [`set_edge_class`]                                     | `sm_SetEdgeClass`                                    |
//! | [`TopologyIntersector::relate`]                        | `Relate`                                             |
//! | [`TopologyIntersector::fixup_end_point_class_mates`]   | `FixupEndPointClassMates`                            |
//! | [`TopologyIntersector::fixup_mid_point_class_mates`]   | `FixupMidPointClassMates`                            |
//! | [`TopologyIntersector::clear_relationships`] | mate maps `RemoveAll` (inline call sites) |
//! | [`TopologyIntersector::set_subset`]                    | `SetSubset`                                          |
//! | [`check_interior_edge`]                                | `sm_CheckInteriorEdge`                               |
//! | [`test_and_split_two_edges`]                           | `sm_TestAndSplitTwoEdges`                            |
//! | [`test_and_imprint_face`]                              | `sm_TestAndImprintFace`                              |
//! | [`Reason`]                                             | `sReasonType`                                        |
//! | [`TopologyIntersector::vertex_face_intersect`]         | `VertexFaceIntersect`                                |
//! | [`TopologyIntersector::edge_face_intersect`]           | `EdgeFaceIntersect`                                  |
//! | [`TopologyIntersector::face_face_intersect`]           | `FaceFaceIntersect`                                  |
//! | [`TopologyIntersector::face_face_coincidence`]         | `FaceFaceCoincidence`                                |
//! | [`TopologyIntersector::primary_brep`], [`TopologyIntersector::other_brep`] | `GetPrimaryBrep`, `GetOtherBrep` |
//! | [`TopologyIntersector::this_approx_tol_3d`]            | `GetThisApproxTol3d`                                 |
//! | [`TopologyIntersector::set_primary_brep`], [`TopologyIntersector::set_other_brep`] | `SetPrimaryBrep`, `SetOtherBrep` |
//! | [`TopologyIntersector::set_this_approx_tol_3d`]        | `SetThisApproxTol3d`                                 |
//! | [`TopologyIntersector::new`], [`TopologyIntersector::with_context`] | the two constructors (CO-PORTED from `boolean.topology_intersector_p2`) |
//! | [`TopologyIntersector::subset`]                        | `GetSubset`                                          |
//! | [`TopologyIntersector::this_ang_tol_rad`], [`TopologyIntersector::set_this_ang_tol_rad`] | `GetThisAngTolRad`, `SetThisAngTolRad` |
//! | [`IntersectorBrep`]                                    | what the intersector reads from and writes to its two `SmBrep`s |
//! | [`MateMap`], [`Relations`]                             | `m_pBToO`, `m_pOToB` (face/edge/vertex subset over [`Brep`] ids; shared by `boolean.merge_p1/p4/p5`) |
//! | [`MateMap::other`], [`MateMap::primary`]               | `GetOtherMate`, `GetBrepMate` over [`Brep`] ids       |
//! | [`Relations::relate_vertices`]                         | `Relate` of two vertices over [`Brep`] ids            |
//! | [`FaceCacheTarget`], `solve_with_face_caches` (crate) | `smsurf_GetSurfaceCache` of a face's surface inside the solves |
//!
//! Deviations:
//! - **Opt-in fix (WR-306, #1058).** With `Kernel::fixes.degenerate_proto_edge_in_one_vertex`,
//!   `ImprintIntersections` fits each final vertex of a proto vertex that absorbed a
//!   degenerate proto edge to its edges (`IntersectorBrep::vertex_update_edge_intervals`)
//!   and raises the pair's tolerances to cover each other (the SMLib fork's fix).
//! - **Intersection trims (issue #489).** `FaceFaceIntersect` supplies a copy of
//!   each classification's current UV curve to `MakeEdgeInFace`. Upstream passes
//!   null here and projects the 3D curve again. The copy keeps the intersection's
//!   parameterisation and remains independent of the classification's cleanup.
//! - **Object handles.** Every `SmObject*` (vertices, edges, faces, curves, surfaces,
//!   the two B-reps and the intersector itself) is a caller-chosen handle `O`
//!   ([`ClassObject`] + [`SolutionObject`] + `Ord`), as in
//!   [`proto_topology`](crate::topology::proto_topology). The B-reps are read and
//!   edited through [`IntersectorBrep`], which the owner of the B-reps implements
//!   (its methods name the ported function each one stands for); geometry is reached
//!   through [`ClassGeometry`].
//! - **Face surface caches.** The topology solves and surface solves of the
//!   intersector read the owner's B-reps shared ([`IntersectorBrep::topology_objects`]).
//!   When a solve asks for the cache of a face-owned surface (`smsurf_GetSurfaceCache`,
//!   which builds the face's trimmed cache and may create its UV trim curves), it stops
//!   with a preparation request; the cache is built on the owner's B-rep
//!   ([`IntersectorBrep::face_cache_target`]) and the solve runs again from its start,
//!   once per face. A solve has no other effect, so the last attempt is the solve
//!   upstream runs with the caches made where it asks for them.
//! - **Pointer maps.** `m_pBToO` / `m_pOToB` (`SmMapPtrToPtr`, an
//!   `std::unordered_map` keyed by address) are `BTreeMap`s over the handles.
//!   `GetMappedBrepObjects` lists the map's values in upstream's *hash order*
//!   (plan hazard `ptr-hash`); the port lists them in the key order of the handles
//!   (see `plan/findings.md`). `m_pSubset` is a `BTreeSet`.
//! - **Context.** `m_crContext` is the [`Kernel`] passed to each method; `GetContext`
//!   has no counterpart. `m_bEditingEnabled` of a B-rep and the context's
//!   `GetDoingBooleanRef` are set through [`IntersectorBrep::set_editing_enabled`] and
//!   [`Kernel::context`] and restored afterwards (`SmTemporaryChangeValue`).
//! - **Out-params** become return values ([`ExistingSsi`], [`SplitTwoEdges`],
//!   [`ImprintFace`], [`VertexFaceIntersect`], [`EdgeFaceIntersect`],
//!   [`FaceFaceIntersect`], [`FaceFaceCoincidence`], the mate lists). The `NotUsed`
//!   parameters (`bMakingManifoldSolid`, `bIgnoreProblems` of `FaceFaceIntersect`,
//!   `bDoRegionNesting` of `FaceFaceCoincidence`, `cpClassificationCurve`) are kept.
//! - **Deleted curves.** Functions that delete curves upstream (`~SmIntersectionTable`,
//!   `ClearExistingSSI`, the resolution and imprint of proto topology) return the
//!   handles deleted, as `proto_topology` does.
//! - **The proto topology manager** (`m_pProtoTopoMgr`) is created by
//!   `IntersectInsertRelate` (unit `boolean.topology_intersector_p2`); until that is
//!   ported it is set with [`TopologyIntersector::set_proto_topology_manager`].
//! - **B-rep edits** go through the adapter: `SmBrep::DeleteFace` is
//!   [`IntersectorBrep::brep_delete_face`] (the owner of the B-reps calls
//!   `topology::brep::p3::delete_face`), so `face_face_coincidence` takes `&mut Kernel`.
//! - **`SmBrep::GlueEdgesGeneral`** (`topology.brep_p7`) goes through
//!   [`IntersectorBrep::brep_glue_edges_general`], which the B-rep owner implements with
//!   `topology::brep::p7::glue_edges_general` (so `face_face_intersect` takes the
//!   kernel mutably).
//! - **`SmBrep::GlueVertices`** (`topology.brep_p5`) goes through
//!   its supertrait `BrepGlue` (`topology::proto_topology::p3`), which the B-rep owner
//!   implements with `topology::brep::p5` (so `vertex_face_intersect` and the
//!   interval-end imprint take the kernel mutably).
//! - **`SmFace::GetBrep` / `GetVertices` / `GetEdges`** (`topology.face_p4`, with their
//!   default arguments) go through [`IntersectorBrep::face_brep`],
//!   [`IntersectorBrep::face_vertices`] and [`IntersectorBrep::face_edges`], which the B-rep
//!   owner implements with `topology::face::p4`.
//! - **`SmFace::FaceIntersect`** (`topology.face_p3`) goes through
//!   [`IntersectorBrep::face_intersect`] (`face_intersect_breps`, faces of one or both
//!   operands), with the UV curve and point outputs part 2's
//!   `face_face_intersect_curves` asks for ([`FaceIntersectWants`]).
//! - **Callees not ported yet** are private stubs returning [`SolidError::Unported`],
//!   each tagged `UNPORTED(<unit>)`: `SmBrep::MakeVertexLoop` / `MakeVertexSplitEdge`
//!   (`topology.brep_p9`),
//!   `SmFace::Point3DClassify` (`query.face_p1`; `SmFace::CalculateBoundingBox` goes
//!   through [`IntersectorBrep::face_calculate_bounding_box`] since the `query.face_p1`
//!   port). The
//!   code after a stub is ported in full. `SmSurface::GlobalCurveIntersect` /
//!   `CoincidenceCheck` (`surface.surface_p8`), `SmCurveClassification::Homogenize`
//!   (`intersect.curve_class_p6`), `InsertTopologyIntersections`
//!   (`intersect.curve_class_p5`), the `SmCurveClassification` constructor
//!   (`intersect.curve_class_p4`; it defers on `ReSet`, `intersect.curve_class_p2`) and `SmFace::CalculateUVDomainFromUVTrimCurves`
//!   (`topology.face_p1`, through [`IntersectorBrep`]; its `CreateUVTrimCurves` is
//!   `topology.face_p3`'s), `SmFace::FaceIntersect` and `SmFace::SetTolerance`
//!   (`topology.face_p3`, through [`IntersectorBrep`]) are the ported calls.
//! - **`SmBrep::MakeEdgeInFace`** (`topology.brep_p5`) runs its checks on the handles
//!   and its `SmFace::FindVertexSector` (`topology.face_p4`) and
//!   `SmBrep::MakeEdgeInFaceWithVU` (`topology.brep_p9`) on the arena the face's handle
//!   names, through the owner's `MergeIntervalBrep::merge_context_with_curve_identity`; the curves are
//!   copied out of the owner's stores (the edge takes the copy, as upstream it takes
//!   the pointer).
//! - **Undefined behaviour upstream** (a null object dereferenced, a solution without
//!   objects, the maps of an intersector made with the context-only constructor) is
//!   [`SolidError::InvalidInput`].
//! - **Statuses upstream ignores** (`WARN`, `SE_MSG`, the imprint loop's "record the
//!   first problem", the unchecked statuses of the B-rep edits, bounding boxes,
//!   surface queries and classifications) are ignored here too through
//!   `ignore_soft`, except [`SolidError::Unported`] and [`SolidError::InvalidInput`],
//!   which always propagate; the ignored call's outputs then stay at their initial
//!   values (an empty solution list, an initial extent, a null edge or vertex).
//! - **Bound.** `FaceFaceIntersect` extends its working face lists with every face
//!   split off; past [`MAX_FACE_LIST`] faces it fails with `SM_ERR` (upstream is
//!   unbounded).
//! - `sm_IntersectWireLaminaEdges` takes the intersector's approximation tolerance
//!   (`m_pTI->GetThisApproxTol3d()` in `FindOrCreateProtoEdge`) as a parameter.
//! - The `SM_DEBUG_CODE` / `CBI_CACHE` / `SM_VALIDATE_TOPOLOGY` blocks are compiled out
//!   upstream and not ported; `SM_ASSERT` / `SM_ASSERT_TOL` only report.
//! - **Merge-stage mates.** The merge stages (`boolean.merge_p1`, `_p5`) work on
//!   concrete [`Brep`] arenas, not handles `O`, and share their edge and vertex mates
//!   as [`Relations`] (co-ported by `boolean.merge_p1`, PR #163). Its
//!   [`Relations::relate_vertices`] is `Relate`'s vertex branch on those ids;
//!   [`MateMap::record`] stores an already-established pair after the same conflict
//!   checks.
//! - **Mates of `GlueCoincidentBrepVertices`.** `imprint_body` hands
//!   `glue_coincident_brep_vertices` a read-only snapshot of `m_pBToO`; upstream calls
//!   `m_pTI->RemoveRelationship` there (`SmProtoTopology.cpp` 6028/6030/6119/6121) and
//!   then `GetOtherMate` again, so wiring `RemoveRelationship` needs a mutable mate
//!   interface.
//! - **Recursion.** None.

use core::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

/// Most faces the working lists of [`TopologyIntersector::face_face_intersect`] may
/// hold (the inputs plus the faces split off); a bound of the port.
pub const MAX_FACE_LIST: usize = 1 << 16;

use crate::curve::curve::p2::{Curve, CurveKind};
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::ShapeMember;
use crate::geom::solution_array::{ObjectKind, SolutionArray, SolutionObject, SolutionType};
use crate::geom::tol::{xsect_tol_3d_from_zones, zone_tol3d};
use crate::intersect::curve_class::p1::{src_zone_tol_3d_of, CurveClassification};
use crate::intersect::curve_class::ClassificationInit;
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, ClassObjectKind, PointClassType, PointClassification,
};
use crate::intersect::topology_solver::p1::{CurveRef, TopologyObjects, TopologySolver};
use crate::intersect::topology_solver::p2::TopologySolveOptions;
use crate::kernel::Kernel;
use crate::math::{max2, min2, Point2, Point3, BIG_DOUBLE, EFF_ZERO, PI};
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::p1::SurfaceSolveContext;
use crate::surface::surface::p2::surface_cache;
use crate::surface::surface::p2::TsectCurveType;
use crate::surface::surface::p5::{DropPoint, DropPointOptions};
use crate::surface::surface::Surface;
use crate::surface::surface_cache::face_context::{
    prepare_face_surface_cache, prepare_face_surface_cache_state, FaceQueries, PreparedFaceQueries,
};
use crate::surface::surface_cache::SurfaceCache;
use crate::topology::proto_topology::{ProtoBrep, ProtoTopologyManager};
use crate::topology::{edge, vertex, Brep, EdgeId, FaceId, VertexId};
use crate::EntityRef;

const OP: &str = "boolean::topology_intersector::p1";
const OP_CHECK_SSI: &str = "boolean::topology_intersector::p1::check_if_have_existing_ssi";
const OP_RELATE: &str = "boolean::topology_intersector::p1::relate";
const OP_LOWEST: &str = "boolean::topology_intersector::p1::find_first_lowest_dimension_sol";
const OP_SET_EDGE_CLASS: &str = "boolean::topology_intersector::p1::set_edge_class";
const OP_INTERIOR: &str = "boolean::topology_intersector::p1::check_interior_edge";
const OP_SPLIT_TWO: &str = "boolean::topology_intersector::p1::test_and_split_two_edges";
const OP_IMPRINT_FACE: &str = "boolean::topology_intersector::p1::test_and_imprint_face";
const OP_FFC: &str = "boolean::topology_intersector::p1::face_face_coincidence";
const OP_FACE_FACE: &str = "boolean::topology_intersector::p1::face_face_intersect";

/// A dereference of a null pointer or a deleted object upstream (undefined
/// behaviour).
pub(super) const fn dangling(reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: OP,
        reason,
    }
}

/// A status upstream ignores, except the errors that cannot be ignored here: unported
/// callees and undefined behaviour upstream.
pub(super) fn ignore_soft<T>(r: Result<T>) -> Result<Option<T>> {
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

/// `SmTArray::FindElement`.
pub(super) fn contains<O: PartialEq>(list: &[O], item: &O) -> bool {
    list.iter().any(|x| x == item)
}

/// `SmTArray::AddUnique`.
pub(super) fn add_unique<O: PartialEq>(list: &mut Vec<O>, item: O) -> bool {
    if contains(list, &item) {
        false
    } else {
        list.push(item);
        true
    }
}

/// `SmTArray::FindCommonElements(crOther, rResult)`: the elements of `other` that
/// are in `this`, in `other`'s order.
pub(super) fn find_common_elements<O: Copy + PartialEq>(this: &[O], other: &[O]) -> Vec<O> {
    other
        .iter()
        .filter(|x| contains(this, x))
        .copied()
        .collect()
}

// ---- what the intersector needs from its B-reps -------------------------------------------

/// What the intersector reads from and writes to its two B-reps (the `SmBrep`,
/// `SmVertex`, `SmEdge` and `SmFace` members it calls), implemented by the owner of
/// the B-reps on top of [`ProtoBrep`] (whose [`ProtoTopology`](crate::topology::proto_topology::p1::proto::ProtoTopology) walks, `vertex_point`,
/// `edge_start_vertex` and `edge_other_vertex`, serve here too). Each method names the
/// upstream function and
/// the ported function that implements it. A method returns `Ok(None)` /
/// `Err(InvalidInput)` where upstream would dereference null.
pub trait IntersectorBrep<O>:
    ProtoBrep<O>
    + crate::topology::proto_topology::p3::BrepGlue<O>
    + crate::intersect::curve_class::p5::ClassGeometryMut<O>
    + crate::intersect::curve_class::p6::MergeIntervalBrep<O>
{
    /// `SmFace::Point3DClassify(rPnt, sSrcZoneTol3d, bDoBoundaryIntersections,
    /// rPointClass, pOptGuessUV)` with the remaining arguments at their defaults
    /// (`bTgtLoopsOnly = FALSE`, `pOptTgtLoops = NULL`, `bUVSpaceOkay = TRUE`; unit
    /// `query.face_p1`: `query::face::p1::point_3d_classify` on the B-rep behind the
    /// handle). Mutable: the classification may build missing UV trim curves.
    ///
    /// # Errors
    ///
    /// Those of `point_3d_classify`; [`SolidError::InvalidInput`] for a handle that is
    /// not a face of a known B-rep.
    #[allow(clippy::too_many_arguments)]
    fn face_point_3d_classify(
        &mut self,
        k: &Kernel,
        face: O,
        point: Point3,
        src_zone_tol_3d: f64,
        do_boundary_intersections: bool,
        pc: &mut PointClassification<O>,
        opt_guess_uv: Option<Point2>,
    ) -> Result<()>;
    /// `SmBrep::MakeVertexLoop` with its default opposite orientation, using the
    /// caller's mutable arenas and notification state (`topology::brep::p9`).
    fn make_vertex_loop(&mut self, brep: O, face: O, point: Point3) -> Result<(O, O)>;
    /// `SmBrep::MakeVertexSplitEdge` through `topology::brep::p9`. Ordinary
    /// refusals return their native status; these callers discard failure outputs.
    fn make_vertex_split_edge(
        &mut self,
        k: &mut Kernel,
        brep: O,
        edge: O,
        parameter: f64,
    ) -> Result<(O, O, O)>;

    /// `SmVertex::GetBrep` (`topology::vertex::brep`): the B-rep handle of a vertex.
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn vertex_brep(&self, vertex: O) -> Result<Option<O>>;
    /// The arena of the B-rep `brep` names (what a `SmBrep*` reaches: its tolerance
    /// and edges, for `SmProtoTopologyManager`'s tolerance fallback).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a handle that names no B-rep.
    fn brep_arena(&self, brep: O) -> Result<&Brep>;
    /// `SmEdge::GetBrep` (`topology::edge::brep`): the B-rep handle of an edge
    /// ([`ProtoTopology::edge_brep`](crate::topology::proto_topology::p1::proto::ProtoTopology::edge_brep) gives its key; the intersector edits the B-rep
    /// through its handle).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn edge_brep_handle(&self, edge: O) -> Result<Option<O>>;
    /// `SmVertex::SetPoint` (`topology::vertex::set_point`).
    ///
    /// # Errors
    ///
    /// Those of the setter.
    fn set_vertex_point(&mut self, k: &Kernel, vertex: O, point: Point3) -> Result<()>;
    /// `SmVertex::SetTolerance` / `SmEdge::SetTolerance` (`topology::vertex::set_tolerance`,
    /// `topology::edge::set_tolerance`) with their `bUpdateOnlyIfLarger` and
    /// `bCascadeToBndries` arguments.
    ///
    /// # Errors
    ///
    /// Those of the setters.
    fn set_topology_tolerance(
        &mut self,
        k: &Kernel,
        topo: O,
        tolerance: f64,
        update_only_if_larger: bool,
        cascade_to_boundaries: bool,
    ) -> Result<()>;
    /// `SmTol::UpdateObjectTolerance(pTopo, dThisGap, dThisTol, bCheckGaps)`
    /// (`geom::tol::update_object_tolerance`).
    ///
    /// # Errors
    ///
    /// Those of `update_object_tolerance`.
    fn update_object_tolerance(
        &mut self,
        k: &Kernel,
        topo: O,
        this_gap: f64,
        this_tol: f64,
        check_gaps: bool,
    ) -> Result<()>;
    /// `SmVertex::IsShellVertex` (`topology::vertex::is_shell_vertex`).
    ///
    /// # Errors
    ///
    /// Those of the query.
    fn vertex_is_shell_vertex(&self, vertex: O) -> Result<bool>;
    /// `SmEdge::IsWire` (`topology::edge::is_wire`).
    ///
    /// # Errors
    ///
    /// Those of the query.
    fn edge_is_wire(&self, edge: O) -> Result<bool>;
    /// `SmEdge::IsLamina` (`topology::edge::is_lamina`).
    ///
    /// # Errors
    ///
    /// Those of the query.
    fn edge_is_lamina(&self, edge: O) -> Result<bool>;
    /// `SmEdge::GetVertex` (`topology::edge::vertex`): the primary edge use's vertex.
    ///
    /// # Errors
    ///
    /// Those of the query.
    fn edge_vertex(&self, edge: O) -> Result<Option<O>>;
    /// `SmFace::GetBrep` (`topology::face::p4::brep`): the B-rep handle of a face, `None`
    /// for a face without face use or shell.
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn face_brep(&self, face: O) -> Result<Option<O>>;
    /// `SmFace::GetVertices(rVertices)` with its defaults (no attribute filter, every
    /// loop; `topology::face::p4::vertices`).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn face_vertices(&self, face: O) -> Result<Vec<O>>;
    /// `SmFace::GetEdges(rEdges)` with its defaults (no attribute filter, every loop;
    /// `topology::face::p4::edges`).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn face_edges(&self, face: O) -> Result<Vec<O>>;
    /// `SmEdge::GetPrimaryEdgeuse()->GetOrientation()`
    /// (`topology::edgeuse::p1::primary_edgeuse`, `orientation`): `None` without a
    /// primary edge use (upstream dereferences null).
    ///
    /// # Errors
    ///
    /// Those of the queries.
    fn edge_primary_edgeuse_orientation(&self, edge: O) -> Result<Option<OrientType>>;
    /// `SmEdge::CalculateBoundingBox(&box)` (`query::edge::calculate_bounding_box`
    /// asked for the normal box, not tight).
    ///
    /// # Errors
    ///
    /// Those of the computation.
    fn edge_bounding_box(&self, k: &Kernel, edge: O) -> Result<Extent3d>;
    /// `SmFace::CalculateUVDomainFromUVTrimCurves(rDomain)` with upstream's default
    /// arguments (`bLimitDomain = TRUE`, no loop gaps;
    /// `topology::face::p1::calculate_uv_domain_from_uv_trim_curves`): the new domain.
    ///
    /// # Errors
    ///
    /// Those of the computation (`CreateUVTrimCurves`, `topology.face_p3`, defers at
    /// `SmFace::GetUpwardLoopuses`, `topology.face_p4`).
    fn face_uv_domain_from_uv_trim_curves(&mut self, k: &Kernel, face: O) -> Result<Extent2d>;
    /// `SmFace::PointClassify(uv, tol, FALSE, TRUE, pc)` with upstream's other
    /// arguments at their defaults (`query::face::p2::point_classify`).
    ///
    /// # Errors
    ///
    /// Those of `point_classify`.
    fn face_point_classify(
        &mut self,
        k: &Kernel,
        face: O,
        uv: Point2,
        pc: &mut PointClassification<O>,
    ) -> Result<()>;
    /// `pFace1->FaceIntersect(ctx, pFace2, tol, angTolDeg, 3DCurves, pOptThisUVCurves,
    /// pOptOtherUVCurves, pOpt3DPoints, &CC1, &CC2, &CC3D, &CCUV1, &CCUV2,
    /// &bAreCoincident)` (`topology::face::p3::face_intersect_breps`) of two faces of
    /// one or two B-reps, with every curve and classification object given a handle;
    /// the first three optional outputs are given as `wants` says (all `NULL` by
    /// default). `opt_coincidence_flag` is `pbOptCoincidenceFlag`, an input ("known
    /// coincident or not") that makes `FaceIntersect` skip its own `CoincidenceCheck`.
    ///
    /// # Errors
    ///
    /// Those of `face_intersect_breps`; [`SolidError::InvalidInput`] for unknown
    /// handles.
    #[allow(clippy::too_many_arguments)] // upstream's FaceIntersect arguments
    fn face_intersect(
        &mut self,
        k: &Kernel,
        face1: O,
        face2: O,
        approx_tol_3d: f64,
        ang_tol_deg: f64,
        wants: FaceIntersectWants,
        opt_coincidence_flag: Option<bool>,
    ) -> Result<FaceIntersection<O>>;
    /// Sets a B-rep's `m_bEditingEnabled` and returns its previous value
    /// (`SmTemporaryChangeValue<SmBoolean>(pBrep->m_bEditingEnabled, ..)`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown B-rep.
    fn set_editing_enabled(&mut self, brep: O, enabled: bool) -> Result<bool>;
    /// Reads the owning B-rep's actual editing flag (`SmBrep::GetEditingEnabled`).
    fn editing_enabled(&self, brep: O) -> Result<bool>;
    /// Reads `SmBrep::GetTolerance` for face sector classification.
    fn brep_tolerance(&self, brep: O) -> Result<f64>;

    /// The adapter the topology solver reads the B-reps' cache trees and geometry
    /// through (`SmTopologySolver::BrepPointSolve` / `BrepCurveSolve`).
    fn topology_objects(&self) -> &dyn TopologyObjects<O>;
    /// The B-rep of `face` lent mutably for the trimmed cache of the face's surface
    /// (`smsurf_GetSurfaceCache` of `pFace->GetSurface()`, which builds the cache
    /// through `GetFace()`), with the inverse handle mappings of that B-rep's entities.
    ///
    /// # Errors
    ///
    /// Those of the implementer for a handle that names no face of a B-rep or a face
    /// without a surface.
    fn face_cache_target(&mut self, face: O) -> Result<FaceCacheTarget<'_, O>>;
    /// The handle of each entity of the B-rep of `face` (the forward mapping of
    /// [`IntersectorBrep::face_cache_target`]), for the classifications of a face
    /// whose surface cache was prepared.
    ///
    /// # Errors
    ///
    /// As [`IntersectorBrep::face_cache_target`].
    fn face_entity_handles(&self, face: O) -> Result<EntityHandles<O>>;
    /// `SmBrep::DeleteFace(pFace)` with upstream's default arguments (`FALSE, TRUE,
    /// NULL, NULL`) on the B-rep `brep`: the owner of a real B-rep implements it with
    /// `topology::brep::p3::delete_face`.
    ///
    /// # Errors
    ///
    /// Those of the edit; an owner with no B-rep behind its handles returns
    /// [`SolidError::Unported`] under a `harness:` label.
    fn brep_delete_face(&mut self, k: &mut Kernel, brep: O, face: O) -> Result<()>;
    /// `SmBrep::DeleteEdge(pEdge)` with its default (`pOptSurfaceToKeep = NULL`) on
    /// the B-rep `brep`: the owner of a real B-rep implements it with
    /// `topology::brep::p4::delete_edge`.
    ///
    /// # Errors
    ///
    /// Those of the edit; [`SolidError::InvalidInput`] for an edge of another B-rep.
    fn brep_delete_edge(&mut self, k: &mut Kernel, brep: O, edge: O) -> Result<()>;
    /// `SmBrep::DeleteVertex(pVertex)` on the B-rep `brep`: the owner of a real B-rep
    /// implements it with `topology::brep::p4::delete_vertex`.
    ///
    /// # Errors
    ///
    /// Those of the edit; [`SolidError::InvalidInput`] for a vertex of another B-rep.
    fn brep_delete_vertex(&mut self, k: &mut Kernel, brep: O, vertex: O) -> Result<()>;
    /// `SmBrep::GlueEdgesGeneral(pSurvivingEdge, eRelativeOrientation, dEdgeGap,
    /// bDoRegionNesting, pEdgeToDelete, pInsideFaceuses)` on B-rep `brep`
    /// (`topology::brep::p7::glue_edges_general`; `pInsideFaceuses` defaults to `NULL`).
    ///
    /// # Errors
    ///
    /// Those of the glue; [`SolidError::InvalidInput`] for an unknown B-rep or edge.
    fn brep_glue_edges_general(
        &mut self,
        k: &mut Kernel,
        brep: O,
        surviving: Option<O>,
        relative_orientation: OrientType,
        edge_gap: f64,
        do_region_nesting: bool,
        to_delete: Option<O>,
        inside_faceuses: Option<&[O]>,
    ) -> Result<()>;
    /// The edges of `brep` in its list order (`SmBrep::GetEdges(rEdges)`, unit
    /// `topology.brep_p1`: `topology::brep::p1::edges` on the B-rep behind the handle).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown B-rep.
    fn brep_edges(&self, brep: O) -> Result<Vec<O>>;
    /// The vertices of `brep` in its list order (`SmBrep::GetVertices(rVertices)`, unit
    /// `topology.brep_p1`: `topology::brep::p1::vertices`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown B-rep.
    fn brep_vertices(&self, brep: O) -> Result<Vec<O>>;
    /// The bounding box of `face` (`SmFace::CalculateBoundingBox(rBox, bTight)`, unit
    /// `query.face_p1`: `query::face::p1::calculate_bounding_box` on the B-rep behind the
    /// handle). The intersector passes `tight = false`, upstream's default. (Named after
    /// the upstream call in full: `face_bounding_box` is #229's `MergeBrep` method.)
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown face, and those of the port.
    fn face_calculate_bounding_box(&self, k: &Kernel, face: O, tight: bool) -> Result<Extent3d>;
    /// `SmBrep::UpdateVertexEdgeIntervals(pVertex)` on the vertex's B-rep
    /// (`topology::brep::p1::update_vertex_edge_intervals`), for the opt-in WR-306 fit in
    /// `ImprintIntersections`.
    ///
    /// # Errors
    ///
    /// Those of the update; [`SolidError::Unported`] (the default) for an adapter without it.
    fn vertex_update_edge_intervals(&mut self, k: &Kernel, vertex: O) -> Result<()> {
        let _ = (k, vertex);
        Err(SolidError::unported(
            "SmBrep::UpdateVertexEdgeIntervals",
            "topology.brep_p1",
        ))
    }
}

// ---- unported callees -----------------------------------------------------------------------

/// `SmBrep::MakeEdgeInFace(pFace, pV1, pV2, pCurve, pOptUVCurve, ivl, eOrient, tol,
/// rpNewEdge, rpNewLoop, rpNewFace)`: the new edge, loop and (when the face was
/// split) face, each null when not made. The face and vertices may be null
/// (`MakeEdgeInFace` checks them itself).
pub(super) fn brep_make_edge_in_face<O: Copy, B: IntersectorBrep<O>>(
    k: &Kernel,
    backend: &mut B,
    brep: O,
    face: Option<O>,
    v1: Option<O>,
    v2: Option<O>,
    curve: O,
    uv_curve: Option<O>,
    interval: Extent1d,
    orientation: OrientType,
    tol: f64,
) -> Result<(Option<O>, Option<O>, Option<O>)> {
    let editing = backend.editing_enabled(brep)?;
    let mut out = None;
    crate::topology::brep::p5::make_edge_with_backend(
        &mut HandleFaceEdge { k, backend, brep },
        editing,
        face,
        v1,
        v2,
        curve,
        uv_curve,
        interval,
        orientation,
        tol,
        &mut out,
    )?;
    out.map(|(edge, new_loop, new_face)| (Some(edge), new_loop, new_face))
        .ok_or_else(|| SolidError::null_pointer(OP))
}

/// `MakeEdgeInFace`'s face, vertex and curve handles: its `SmFace::FindVertexSector`
/// and `SmBrep::MakeEdgeInFaceWithVU` run on the arena the face's handle names, through
/// the owner's [`MergeIntervalBrep::merge_context_with_curve_identity`](crate::intersect::curve_class::p6::MergeIntervalBrep::merge_context_with_curve_identity)
/// (`topology::brep::p5`'s calls of `topology::face::p4::find_vertex_sector` and
/// `topology::brep::p9::make_edge_in_face_with_vu`).
struct HandleFaceEdge<'a, O, B> {
    k: &'a Kernel,
    backend: &'a mut B,
    brep: O,
}

impl<O: Copy, B: IntersectorBrep<O>> HandleFaceEdge<'_, O, B> {
    /// The B-rep entity behind topology handle `o`.
    fn entity(&self, o: O) -> Result<EntityRef> {
        ClassGeometry::topology_entity(&*self.backend, o)
            .map(|(_, e)| e)
            .ok_or_else(|| {
                SolidError::invalid_input(OP, "topology handle the owner cannot resolve")
            })
    }
    /// A copy of the curve behind handle `o` (`MakeEdgeInFaceWithVU` consumes it).
    fn curve(&self, o: O) -> Result<Curve> {
        ClassGeometry::curve(&*self.backend, o)
            .cloned()
            .ok_or_else(|| SolidError::invalid_input(OP, "curve handle the owner cannot resolve"))
    }
    /// The vertex use behind handle `o`.
    fn vertexuse(&self, o: O) -> Result<crate::topology::VertexuseId> {
        match self.entity(o)? {
            EntityRef::Vertexuse(vu) => Ok(vu),
            _ => Err(SolidError::invalid_input(OP, "not a vertex use handle")),
        }
    }
}
impl<O: Copy, B: IntersectorBrep<O>> crate::topology::brep::p5::FaceEdgeBackend
    for HandleFaceEdge<'_, O, B>
{
    type Face = O;
    type Vertex = O;
    type Curve = O;
    type UvCurve = O;
    type Vertexuse = O;
    type Output = Option<(O, Option<O>, Option<O>)>;
    fn validate_face(&self, _face: O) -> Result<()> {
        // Upstream only checks the face pointer (`NER(pFace)`), never its surface;
        // a handle is never null, and the caller has already rejected a missing face.
        Ok(())
    }
    fn tolerance(&self) -> Result<f64> {
        self.backend.brep_tolerance(self.brep)
    }
    fn sector(
        &mut self,
        face: O,
        vertex: Option<O>,
        curve: &O,
        uv: Option<&O>,
        start: f64,
        end: f64,
        orientation: OrientType,
        tol: f64,
        all: bool,
    ) -> Result<Option<O>> {
        // NER(cpVertex) before anything is resolved
        let vertex = vertex.ok_or_else(|| SolidError::null_pointer(OP))?;
        let EntityRef::Vertex(vertex) = self.entity(vertex)? else {
            return Err(SolidError::invalid_input(OP, "not a vertex handle"));
        };
        let curve = self.curve(*curve)?;
        let uv = uv.map(|o| self.curve(*o)).transpose()?;
        let k = self.k;
        let ctx = self
            .backend
            .merge_context_with_curve_identity(face)
            .ok_or_else(|| SolidError::invalid_input(OP, "face handle the owner cannot resolve"))?;
        let EntityRef::Face(f) = ctx.target else {
            return Err(SolidError::invalid_input(OP, "not a face handle"));
        };
        let vu = crate::topology::brep::p5::find_vertex_sector(
            k,
            ctx.brep,
            f,
            Some(vertex),
            &curve,
            uv.as_ref(),
            start,
            end,
            orientation,
            tol,
            all,
        )?;
        Ok(vu.map(|vu| (ctx.handle)(EntityRef::Vertexuse(vu))))
    }
    fn insert(
        &mut self,
        face: O,
        first: O,
        last: O,
        curve: O,
        uv: Option<O>,
        interval: Extent1d,
        orientation: OrientType,
        gap: f64,
        output: &mut Self::Output,
    ) -> Result<()> {
        let first = self.vertexuse(first)?;
        let last = self.vertexuse(last)?;
        let curve = self.curve(curve)?;
        let uv = uv.map(|o| self.curve(o)).transpose()?;
        let k = self.k;
        let mut ctx = self
            .backend
            .merge_context_with_curve_identity(face)
            .ok_or_else(|| SolidError::invalid_input(OP, "face handle the owner cannot resolve"))?;
        let EntityRef::Face(f) = ctx.target else {
            return Err(SolidError::invalid_input(OP, "not a face handle"));
        };
        let made = crate::topology::brep::p9::make_edge_in_face_with_vu_with_curve_identity(
            k,
            ctx.brep,
            crate::topology::brep::p9::EdgeInFaceInput {
                face: Some(f),
                vertices: [Some(first), Some(last)],
                curve: Some(curve),
                uv_trim: uv,
                drop_fail: None,
                interval,
                orientation,
                curve_zone_tol_3d: zone_tol3d(k, gap),
            },
            &mut ctx.notifications,
            ctx.curve_identity,
        )?;
        *output = Some((
            (ctx.handle)(EntityRef::Edge(made.edge)),
            made.loop_.map(|l| (ctx.handle)(EntityRef::Loop(l))),
            made.face.map(|f| (ctx.handle)(EntityRef::Face(f))),
        ));
        Ok(())
    }
}

/// `eStat = pFace->CalculateUVDomainFromUVTrimCurves(sTempDomain)` of a caller that takes
/// the domain only on `SM_SUCCESS`: `None` for another status. That includes
/// `SmExtent2d::Intersect`'s real `SM_ERR_INVALID_INPUT` for a trim box disjoint from the
/// surface domain (`SER` there), which the port reports as `InvalidInput` from
/// `Extent2d::intersect`; every other `InvalidInput` is a null dereference upstream and
/// propagates, as in [`ignore_soft`].
fn face_uv_domain_from_uv_trim_curves<O>(
    k: &Kernel,
    brep: &mut impl IntersectorBrep<O>,
    face: O,
) -> Result<Option<Extent2d>> {
    match brep.face_uv_domain_from_uv_trim_curves(k, face) {
        Err(SolidError::InvalidInput {
            operation: "geom::extent2d::Extent2d::intersect",
            ..
        }) => Ok(None),
        r => ignore_soft(r),
    }
}

/// The curves and classifications `SmFace::FaceIntersect` returns.
#[derive(Debug, Clone)]
pub struct FaceIntersection<O> {
    /// `s3DCurves`.
    pub curves_3d: Vec<O>,
    /// `siiCC`.
    pub cc1: Vec<CurveClassification<O>>,
    /// `sjjCC`.
    pub cc2: Vec<CurveClassification<O>>,
    /// `sCC3DCrvs`, `sCCUVCrvs1`, `sCCUVCrvs2`.
    pub cc_curves: [Vec<O>; 3],
    /// `pOptThisUVCurves`: the UV curves of `curves_3d` on the first face (empty unless
    /// asked for in [`FaceIntersectWants`]).
    pub this_uv_curves: Vec<O>,
    /// `pOptOtherUVCurves`: their UV curves on the second face (when asked).
    pub other_uv_curves: Vec<O>,
    /// `pOpt3DPoints`: the single-point intersections (when asked).
    pub points_3d: Vec<Point3>,
}

/// Which of the first three optional outputs of `SmFace::FaceIntersect` are given
/// (`pOptThisUVCurves`, `pOptOtherUVCurves`, `pOpt3DPoints`; `NULL` by default).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FaceIntersectWants {
    /// `pOptThisUVCurves`.
    pub this_uv_curves: bool,
    /// `pOptOtherUVCurves`.
    pub other_uv_curves: bool,
    /// `pOpt3DPoints`.
    pub points_3d: bool,
}

/// Which of the curve-classification out-pointers of `SmFace::FaceIntersect` are given
/// (non-`NULL`); each is `NULL` by default. The arrays given are returned in
/// [`FaceIntersection`].
#[allow(clippy::struct_excessive_bools)] // upstream's optional out-pointers
pub(super) struct FaceIntersectOutputs {
    /// `pOptThisCrvClasses`.
    pub(super) this_crv_classes: bool,
    /// `pOptOtherCrvClasses`.
    pub(super) other_crv_classes: bool,
    /// `pOptCC3DCrvs`.
    pub(super) cc_3d_crvs: bool,
    /// `pOptThisCCUVCrvs`.
    pub(super) this_cc_uv_crvs: bool,
    /// `pOptOtherCCUVCrvs`.
    pub(super) other_cc_uv_crvs: bool,
}

/// `SmFace::FaceIntersect(crContext, pFace2, tol, angTolDeg, r3DCurves,
/// pOptThisUVCurves, pOptOtherUVCurves, pOpt3DPoints, pOptThisCrvClasses,
/// pOptOtherCrvClasses, pOptCC3DCrvs, pOptThisCCUVCrvs, pOptOtherCCUVCrvs,
/// pbOptCoincidenceFlag)` through [`IntersectorBrep::face_intersect`]; the first three
/// optional outputs are `opt_this_uv_curves`, `opt_other_uv_curves` and `opt_3d_points`
/// (appended to), the other five are chosen by `outputs` (the arrays not given come
/// back empty). `opt_coincidence_flag` is `pbOptCoincidenceFlag`, an input ("known
/// coincident or not") that makes `FaceIntersect` skip its own `CoincidenceCheck`.
///
/// # Errors
///
/// Those of [`IntersectorBrep::face_intersect`].
#[allow(clippy::too_many_arguments)] // upstream's FaceIntersect arguments
pub(super) fn face_face_intersect_curves<O>(
    k: &Kernel,
    brep: &mut impl IntersectorBrep<O>,
    face1: O,
    face2: O,
    approx_tol_3d: f64,
    ang_tol_deg: f64,
    opt_this_uv_curves: Option<&mut Vec<O>>,
    opt_other_uv_curves: Option<&mut Vec<O>>,
    opt_3d_points: Option<&mut Vec<Point3>>,
    outputs: &FaceIntersectOutputs,
    opt_coincidence_flag: Option<bool>,
) -> Result<FaceIntersection<O>> {
    let wants = FaceIntersectWants {
        this_uv_curves: opt_this_uv_curves.is_some(),
        other_uv_curves: opt_other_uv_curves.is_some(),
        points_3d: opt_3d_points.is_some(),
    };
    let mut fi = brep.face_intersect(
        k,
        face1,
        face2,
        approx_tol_3d,
        ang_tol_deg,
        wants,
        opt_coincidence_flag,
    )?;
    if let Some(v) = opt_this_uv_curves {
        v.append(&mut fi.this_uv_curves);
    }
    if let Some(v) = opt_other_uv_curves {
        v.append(&mut fi.other_uv_curves);
    }
    if let Some(v) = opt_3d_points {
        v.append(&mut fi.points_3d);
    }
    // the classification arrays and curves of a NULL out-pointer are not returned
    if !outputs.this_crv_classes {
        fi.cc1.clear();
    }
    if !outputs.other_crv_classes {
        fi.cc2.clear();
    }
    let [cc_3d, cc_uv1, cc_uv2] = &mut fi.cc_curves;
    for (given, list) in [
        (outputs.cc_3d_crvs, cc_3d),
        (outputs.this_cc_uv_crvs, cc_uv1),
        (outputs.other_cc_uv_crvs, cc_uv2),
    ] {
        if !given {
            list.clear();
        }
    }
    Ok(fi)
}

/// `pSurf->GlobalCurveIntersect(domain, *pCurve, interval, tol, solutions)` of the
/// surface and curve behind the handles (unit `surface.surface_p8`).
/// The surface reaches its face's cache through the face (`smsurf_GetSurfaceCache`,
/// [`surface_solve_with_face_caches`]).
fn surface_global_curve_intersect<O: ClassObject + SolutionObject + Ord>(
    k: &Kernel,
    geometry: &mut impl IntersectorBrep<O>,
    surface: O,
    domain: &Extent2d,
    curve: O,
    interval: &Extent1d,
    tol: f64,
) -> Result<SolutionArray<O>> {
    let s = geometry
        .surface(surface)
        .ok_or(dangling("surface handle cannot be resolved"))?
        .clone_for_evaluation()?;
    let c = geometry
        .curve(curve)
        .ok_or(dangling("curve handle cannot be resolved"))?
        .clone_for_evaluation()?;
    surface_solve_with_face_caches(k, geometry, |context| {
        s.global_curve_intersect_with_context(
            k,
            [Some(curve), Some(surface)],
            None,
            context,
            domain,
            &c,
            interval,
            tol,
        )
    })
}

/// `pSurf1->CoincidenceCheck(*pSurf2, tol, bAreCoincident, dMaxDist, FALSE, pSCover)`
/// of the surfaces behind the handles (unit `surface.surface_p8`): whether they are
/// coincident and the largest distance seen (`pSCover` is `NotUsed` upstream).
pub(super) fn surface_coincidence_check<O: ClassObject + SolutionObject>(
    k: &Kernel,
    geometry: &impl IntersectorBrep<O>,
    surface: O,
    other: O,
    tol_3d: f64,
    allow_extensions: bool,
) -> Result<(bool, f64)> {
    let s = geometry
        .surface(surface)
        .ok_or(dangling("surface handle cannot be resolved"))?;
    let o = geometry
        .surface(other)
        .ok_or(dangling("surface handle cannot be resolved"))?;
    let c = s.coincidence_check(k, o, tol_3d, allow_extensions)?;
    Ok((c.are_coincident, c.max_distance))
}

/// `SmCurveClassification(pCurve, interval, NULL, tol)` (`cpUVCurve = NULL`; the remaining
/// arguments default: no interval storage, the curve not owned and current, no surfaces),
/// unit `intersect.curve_class_p4`.
pub(super) fn new_curve_classification<O: ClassObject>(
    k: &Kernel,
    curve: O,
    interval: &Extent1d,
    uv_curve: Option<O>,
    tol: f64,
) -> Result<CurveClassification<O>> {
    CurveClassification::with_curve(
        k,
        &ClassificationInit {
            curve: Some(curve),
            interval: *interval,
            uv_curve,
            src_zone_tol_3d: zone_tol3d(k, tol),
            opt_data_size: 0,
            is_curve_owned: false,
            is_curve_current: true,
            surface1: None,
            surface2: None,
            uv_curve2: None,
            ti: None,
        },
    )
}

/// `SmCurveClassification::InsertTopologyIntersections(solutions, 0)` (unit
/// `intersect.curve_class_p5`).
pub(super) fn cc_insert_topology_intersections<O: ClassObject + SolutionObject>(
    k: &Kernel,
    geometry: &impl IntersectorBrep<O>,
    cc: &mut CurveClassification<O>,
    solutions: &SolutionArray<O>,
) -> Result<()> {
    cc.insert_topology_intersections(k, geometry, solutions.as_slice(), 0)
}

/// `SmCurveClassification::Homogenize(other)` (unit `intersect.curve_class_p6`).
fn cc_homogenize<O: ClassObject + SolutionObject>(
    k: &Kernel,
    geometry: &impl IntersectorBrep<O>,
    cc: &mut CurveClassification<O>,
    other: &mut CurveClassification<O>,
) -> Result<()> {
    cc.homogenize(k, geometry, other)
}

// ---- the intersection table ----------------------------------------------------------------

/// The curves of one registered surface/surface intersection
/// (`SmIntersectionResults`): all five arrays exist together (`m_p3DCurves != NULL`)
/// or none does. `SmIntersectionResults()` is [`Default`] (no arrays).
#[derive(Debug, Clone, PartialEq)]
pub struct IntersectionResults<O> {
    /// `m_p3DCurves`, `m_pUVCurves1`, `m_pUVCurves2`, `m_pCurveTypes`, `m_pDeviations`.
    pub curves: Option<IntersectionCurves<O>>,
}

impl<O> Default for IntersectionResults<O> {
    fn default() -> Self {
        Self { curves: None }
    }
}

/// The five parallel arrays of an [`IntersectionResults`].
#[derive(Debug, Clone, PartialEq)]
pub struct IntersectionCurves<O> {
    /// `*m_p3DCurves`: the 3D intersection curves (null entries are an error when
    /// looked up).
    pub curves_3d: Vec<Option<O>>,
    /// `*m_pUVCurves1`: the UV trim curves on the B-rep surface (null allowed).
    pub uv_curves1: Vec<Option<O>>,
    /// `*m_pUVCurves2`: the UV trim curves on the other surface (null allowed).
    pub uv_curves2: Vec<Option<O>>,
    /// `*m_pCurveTypes`.
    pub curve_types: Vec<TsectCurveType>,
    /// `*m_pDeviations`.
    pub deviations: Vec<f64>,
}

impl<O> Default for IntersectionCurves<O> {
    fn default() -> Self {
        Self {
            curves_3d: Vec::new(),
            uv_curves1: Vec::new(),
            uv_curves2: Vec::new(),
            curve_types: Vec::new(),
            deviations: Vec::new(),
        }
    }
}

/// The table of pre-existing surface/surface intersections (`SmIntersectionTable`):
/// parallel lists of a B-rep surface, an other surface (null: "no intersections of
/// the B-rep surface other than the registered ones") and the results.
/// `SmIntersectionTable()` is [`Default`].
#[derive(Debug, Clone, PartialEq)]
pub struct IntersectionTable<O> {
    /// `m_vBrepSurfaces`.
    pub brep_surfaces: Vec<Option<O>>,
    /// `m_vOtherSurfaces`.
    pub other_surfaces: Vec<Option<O>>,
    /// `m_vResults`.
    pub results: Vec<IntersectionResults<O>>,
}

impl<O> Default for IntersectionTable<O> {
    fn default() -> Self {
        Self {
            brep_surfaces: Vec::new(),
            other_surfaces: Vec::new(),
            results: Vec::new(),
        }
    }
}

impl<O: Copy> IntersectionTable<O> {
    /// `~SmIntersectionTable`: deletes every curve of every result that has arrays
    /// (the 3D curve and both UV curves of each entry, when not null). Returns the
    /// curve handles deleted, in that order.
    #[must_use]
    pub fn destroy(mut self) -> Vec<O> {
        let mut deleted = Vec::new();
        for result in &mut self.results {
            if let Some(c) = result.curves.take() {
                for j in 0..c.curves_3d.len() {
                    // upstream indexes the UV arrays with the 3D array's index
                    let c3 = c.curves_3d.get(j).copied().flatten();
                    let u1 = c.uv_curves1.get(j).copied().flatten();
                    let u2 = c.uv_curves2.get(j).copied().flatten();
                    deleted.extend(c3);
                    deleted.extend(u1);
                    deleted.extend(u2);
                }
            }
        }
        deleted
    }
}

/// What [`TopologyIntersector::check_if_have_existing_ssi`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct ExistingSsi<O> {
    /// The status when the function failed after filling the outputs so far (`SER(SM_ERR)`
    /// for a registered 3D curve that is not a B-spline curve; the caller upstream
    /// ignores the status and reads `rbHaveExistingSSI`); `None` on success.
    pub status: Option<SolidError>,
    /// `rbHaveExistingSSI`.
    pub have_existing_ssi: bool,
    /// `r3DCurves`: copies of the registered 3D curves.
    pub curves_3d: Vec<Option<O>>,
    /// `rUVCurves1`: copies of the registered UV curves (the registered handle itself
    /// when it is not a B-spline curve).
    pub uv_curves1: Vec<Option<O>>,
    /// `rUVCurves2`.
    pub uv_curves2: Vec<Option<O>>,
    /// `rCurveTypes`.
    pub curve_types: Vec<TsectCurveType>,
    /// `rDeviations`.
    pub deviations: Vec<f64>,
}

impl<O> Default for ExistingSsi<O> {
    fn default() -> Self {
        Self {
            status: None,
            have_existing_ssi: false,
            curves_3d: Vec::new(),
            uv_curves1: Vec::new(),
            uv_curves2: Vec::new(),
            curve_types: Vec::new(),
            deviations: Vec::new(),
        }
    }
}

// ---- the intersector -------------------------------------------------------------------------

/// The topology intersector (`SmTopologyIntersector`): the two B-reps, the mate maps
/// between their topology, the algorithm controls and the proto topology of the
/// intersections.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean controls
pub struct TopologyIntersector<O> {
    /// `m_bSkipSSI`: don't intersect surfaces (default `false`).
    pub skip_ssi: bool,
    /// `m_bSkipManifoldEdges`: don't intersect manifold edges (default `true`).
    pub skip_manifold_edges: bool,
    /// `m_bIntersectTangentEdges` (default `true`).
    pub intersect_tangent_edges: bool,
    /// `m_lIntersectLaminaEdges`: 0 none, 1 both, 2 only the B-rep's, 3 only the
    /// other's (default 0).
    pub intersect_lamina_edges: u64,
    /// `m_bIntersectWireEdges` (default `true`).
    pub intersect_wire_edges: bool,
    /// `m_bIntersectShellVertices` (default `true`).
    pub intersect_shell_vertices: bool,
    /// `m_bIgnoreTangency` (default `false`).
    pub ignore_tangency: bool,
    /// `m_bDoGluing` (default `false`, not implemented upstream).
    pub do_gluing: bool,
    /// `m_bDoingFillets` (default `false`).
    pub doing_fillets: bool,
    /// `m_bTopologyDeleted`: some topology was deleted during subsequent operations.
    pub topology_deleted: bool,
    /// `m_pBrep`: the B-rep which receives the merge.
    pub(super) brep: Option<O>,
    /// `m_pOther`: the B-rep to be merged into the result.
    pub(super) other: Option<O>,
    /// `m_pBToO`: B-rep object to its other mate (`None`: the map was never made).
    /// A null other object is a stored `None` value (`Relate(pBrepTopo, NULL)`).
    pub(super) b_to_o: Option<BTreeMap<O, Option<O>>>,
    /// `m_pOToB`: other object (a null one is the `None` key) to its B-rep mate.
    pub(super) o_to_b: Option<BTreeMap<Option<O>, O>>,
    /// `m_bSwappedOrder`.
    swapped_order: bool,
    /// `m_dThisApproxTol3d`.
    pub(super) this_approx_tol_3d: f64,
    /// `m_dThisAngTolRad`.
    pub(super) this_ang_tol_rad: f64,
    /// `m_pIntersectionTable`.
    pub(super) intersection_table: Option<IntersectionTable<O>>,
    /// `m_pSubset`: the geometry objects of the other B-rep to intersect.
    subset: Option<BTreeSet<O>>,
    /// `m_pProtoTopoMgr`.
    pub(super) proto_topo_mgr: Option<ProtoTopologyManager<O>>,
}

/// What [`test_and_split_two_edges`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitTwoEdges<O> {
    /// `rdDistToEdge` (always set).
    pub dist_to_edge: f64,
    /// `rdMinUnstitched` (always set).
    pub min_unstitched: f64,
    /// `rNewVerts`: the surviving glued vertices.
    pub new_verts: Vec<O>,
    /// `rNewEdges1`: the edges split off the first edge (a null entry where
    /// `MakeVertexSplitEdge` returned no edge, as upstream's list).
    pub new_edges1: Vec<Option<O>>,
    /// `rNewEdges2`: the edges split off the second edge (null entries as in
    /// `new_edges1`).
    pub new_edges2: Vec<Option<O>>,
}

/// Why [`test_and_imprint_face`] did not imprint (`sReasonType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reason {
    /// `NO_REASON`.
    #[default]
    NoReason,
    /// `FAR_FROM_GEOM`.
    FarFromGeom,
    /// `CLOSE_TO_BOUNDARY`.
    CloseToBoundary,
}

/// What [`test_and_imprint_face`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct ImprintFace<O> {
    /// `reReason`.
    pub reason: Reason,
    /// `rdDistToFace` (always set).
    pub dist_to_face: f64,
    /// `rpNewVertex`.
    pub new_vertex: Option<O>,
}

/// What [`TopologyIntersector::imprint_intersections_reporting_edits`] returns.
#[derive(Debug)]
pub struct ImprintOutcome<O> {
    /// The curve handles deleted.
    pub deleted: Vec<O>,
    /// `eRetStat`: the first failing imprint or glue status.
    pub status: Result<()>,
    /// Whether a failing imprint or glue may have edited a B-rep. False when every
    /// failure was an edge definition that stopped before its first `MakeEdgeInFace`
    /// (e.g. no final vertex), which leaves the B-reps as they were.
    pub failure_edited: bool,
}

/// What [`TopologyIntersector::vertex_face_intersect`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct VertexFaceIntersect<O> {
    /// `rProcessedVerts`: the glued vertices set to null in the input list.
    pub processed_verts: Vec<O>,
    /// `rProcessedFaces`.
    pub processed_faces: Vec<O>,
    /// `rdMaxVertexGap`.
    pub max_vertex_gap: f64,
    /// `rdMinUnstitchedVertGap`.
    pub min_unstitched_vert_gap: f64,
}

/// What [`TopologyIntersector::edge_face_intersect`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeFaceIntersect<O> {
    /// `rNewVerts` (a null entry where `MakeVertexSplitEdge` returned no vertex, as
    /// upstream's list).
    pub new_verts: Vec<Option<O>>,
    /// `rSplitEdges`: input edges that were split.
    pub split_edges: Vec<O>,
    /// `rNewEdges` (a null entry, added once, where `MakeVertexSplitEdge` returned no
    /// edge, as upstream's list).
    pub new_edges: Vec<Option<O>>,
    /// `rProcessedFaces`.
    pub processed_faces: Vec<O>,
    /// `rdMaxVertexGap`.
    pub max_vertex_gap: f64,
    /// `rdMinUnstitchedVertGap`.
    pub min_unstitched_vert_gap: f64,
}

/// What [`TopologyIntersector::face_face_intersect`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceFaceIntersect<O> {
    /// `rProcessedFaces`.
    pub processed_faces: Vec<O>,
    /// `rNewFaces`.
    pub new_faces: Vec<O>,
    /// `rNewEdges` (a null entry where `MakeEdgeInFace` failed, as upstream's list).
    pub new_edges: Vec<Option<O>>,
    /// `rdMaxGap` (`NotUsed`: always 0).
    pub max_gap: f64,
    /// `rdMinUnstitched` (`NotUsed`: always `SM_BIG_DOUBLE`).
    pub min_unstitched: f64,
}

/// What [`TopologyIntersector::face_face_coincidence`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceFaceCoincidence<O> {
    /// `rProcessedFaces`: the surviving coincident faces.
    pub processed_faces: Vec<O>,
    /// `rDeletedFaces`.
    pub deleted_faces: Vec<O>,
    /// `rdMaxGap`.
    pub max_gap: f64,
    /// `rdMinUnstitched`.
    pub min_unstitched: f64,
}

impl<O: ClassObject + SolutionObject + Ord> TopologyIntersector<O> {
    /// `SmTopologyIntersector(crContext)` (CO-PORTED from unit
    /// `boolean.topology_intersector_p2`): the defaults, no B-reps, no mate maps.
    #[must_use]
    pub fn with_context() -> Self {
        Self {
            skip_ssi: false,
            skip_manifold_edges: true,
            intersect_tangent_edges: true,
            intersect_lamina_edges: 0,
            intersect_wire_edges: true,
            intersect_shell_vertices: true,
            ignore_tangency: false,
            do_gluing: false,
            doing_fillets: false,
            topology_deleted: false,
            brep: None,
            other: None,
            b_to_o: None,
            o_to_b: None,
            swapped_order: false,
            this_approx_tol_3d: 0.0,
            this_ang_tol_rad: 0.0,
            intersection_table: None,
            subset: None,
            proto_topo_mgr: None,
        }
    }

    /// `SmTopologyIntersector(crContext, pBrep, pOther, dThisApproxTol3d,
    /// dThisAngTolRad)` (CO-PORTED from unit `boolean.topology_intersector_p2`): the
    /// defaults, the two B-reps, the tolerances and empty mate maps.
    #[must_use]
    pub fn new(
        brep: Option<O>,
        other: Option<O>,
        this_approx_tol_3d: f64,
        this_ang_tol_rad: f64,
    ) -> Self {
        Self {
            brep,
            other,
            b_to_o: Some(BTreeMap::new()),
            o_to_b: Some(BTreeMap::new()),
            this_approx_tol_3d,
            this_ang_tol_rad,
            ..Self::with_context()
        }
    }

    /// Populate valid paired mate-map fixtures without exposing maps in production.
    #[cfg(test)]
    pub(crate) fn seed_mates_for_test(&mut self, pairs: &[(O, O)]) {
        for &(primary, other) in pairs {
            self.b_to_o
                .get_or_insert_with(BTreeMap::new)
                .insert(primary, Some(other));
            self.o_to_b
                .get_or_insert_with(BTreeMap::new)
                .insert(Some(other), primary);
        }
    }

    /// `~SmTopologyIntersector`: deletes the intersection table (and its curves), the
    /// mate maps and the subset. Returns the curve handles deleted. The proto topology
    /// manager is not deleted by upstream's destructor (it is owned by
    /// `IntersectInsertRelate`); it is dropped here with the intersector.
    #[must_use]
    pub fn destroy(mut self) -> Vec<O> {
        let deleted = self
            .intersection_table
            .take()
            .map(IntersectionTable::destroy)
            .unwrap_or_default();
        self.b_to_o = None;
        self.o_to_b = None;
        self.subset = None;
        deleted
    }

    // ---- the intersection table ------------------------------------------------------

    /// `ClearExistingSSI`: deletes the intersection table and its curves. Returns the
    /// curve handles deleted.
    pub fn clear_existing_ssi(&mut self) -> Vec<O> {
        self.intersection_table
            .take()
            .map(IntersectionTable::destroy)
            .unwrap_or_default()
    }

    /// The intersection table (`m_pIntersectionTable`), for the registration of
    /// existing intersections (`RegisterExistingSSI`, unit
    /// `boolean.topology_intersector_p2`).
    #[must_use]
    pub const fn intersection_table(&self) -> Option<&IntersectionTable<O>> {
        self.intersection_table.as_ref()
    }

    /// Installs (or removes) the intersection table.
    pub fn set_intersection_table(&mut self, table: Option<IntersectionTable<O>>) {
        self.intersection_table = table;
    }

    /// `CheckIfHaveExistingSSI`: whether an intersection of `brep_surface` with
    /// `other_surface` was registered, with copies of every registered curve of every
    /// matching entry (the UV curves are copied when they are B-spline curves, else
    /// returned as they are). Each match re-copies the whole accumulated 3D and UV
    /// curve lists, as upstream (earlier copies are then replaced by copies of
    /// themselves). An entry of `brep_surface` with a null other surface also counts
    /// as "have existing SSI".
    ///
    /// # Errors
    ///
    /// (`SM_ERR` for a registered 3D curve that is not a B-spline curve is reported in
    /// [`ExistingSsi::status`] with the outputs filled so far, as upstream leaves them);
    /// [`SolidError::InvalidInput`] when a handle cannot be resolved or a store is
    /// full.
    pub fn check_if_have_existing_ssi(
        &self,
        geometry: &mut impl IntersectorBrep<O>,
        brep_surface: Option<O>,
        other_surface: Option<O>,
    ) -> Result<ExistingSsi<O>> {
        let mut out = ExistingSsi::default();

        let Some(table) = &self.intersection_table else {
            return Ok(out);
        };

        for ii in 0..table.brep_surfaces.len() {
            if brep_surface != table.brep_surfaces.get(ii).copied().flatten() {
                continue;
            }
            if other_surface != table.other_surfaces.get(ii).copied().flatten() {
                continue;
            }
            // Here we have found a match load answers and continue looking
            out.have_existing_ssi = true;
            let results = table
                .results
                .get(ii)
                .ok_or(dangling("intersection table lists differ in length"))?;
            if let Some(c) = &results.curves {
                out.curves_3d.extend_from_slice(&c.curves_3d);
                out.uv_curves1.extend_from_slice(&c.uv_curves1);
                out.uv_curves2.extend_from_slice(&c.uv_curves2);
                out.curve_types.extend_from_slice(&c.curve_types);
                out.deviations.extend_from_slice(&c.deviations);
                for jj in 0..out.curves_3d.len() {
                    let copy =
                        Self::b_spline_copy(geometry, out.curves_3d.get(jj).copied().flatten())?;
                    let Some(h) = copy else {
                        out.status = Some(SolidError::sm_err(OP_CHECK_SSI));
                        return Ok(out);
                    };
                    if let Some(slot) = out.curves_3d.get_mut(jj) {
                        *slot = Some(h);
                    }
                    if let Some(h) =
                        Self::b_spline_copy(geometry, out.uv_curves1.get(jj).copied().flatten())?
                    {
                        if let Some(slot) = out.uv_curves1.get_mut(jj) {
                            *slot = Some(h);
                        }
                    }
                    if let Some(h) =
                        Self::b_spline_copy(geometry, out.uv_curves2.get(jj).copied().flatten())?
                    {
                        if let Some(slot) = out.uv_curves2.get_mut(jj) {
                            *slot = Some(h);
                        }
                    }
                }
            }
        }

        // Now do a quick check to see if we have an opposing NULL which
        // means that we have an intersection but it is nothing.
        for ii in 0..table.brep_surfaces.len() {
            if brep_surface == table.brep_surfaces.get(ii).copied().flatten()
                && table.other_surfaces.get(ii).copied().flatten().is_none()
            {
                out.have_existing_ssi = true;
                return Ok(out);
            }
        }

        Ok(out)
    }

    /// `SM_CAST_PTR(SmBSplineCurve, curve)` followed by `new SmBSplineCurve(*pBSC)`:
    /// the handle of a copy when `curve` is a B-spline curve, `None` otherwise (a null
    /// handle or a curve of another kind).
    pub(super) fn b_spline_copy(
        geometry: &mut impl IntersectorBrep<O>,
        curve: Option<O>,
    ) -> Result<Option<O>> {
        let Some(h) = curve else {
            return Ok(None);
        };
        // `new SmBSplineCurve(*pBSC)`: the copy is the curve's B-spline part (a line
        // or conic is sliced to a plain B-spline curve)
        let copy: Curve = match geometry.curve(h).map(Curve::kind) {
            Some(CurveKind::BSpline(c)) => Curve::from(c.clone()),
            Some(CurveKind::Line(c)) => Curve::from(c.base.clone()),
            Some(CurveKind::Ellipse(c)) => Curve::from(c.base().clone()),
            Some(CurveKind::Hyperbola(c)) => Curve::from(c.base().clone()),
            Some(CurveKind::Parabola(c)) => Curve::from(c.base().clone()),
            Some(_) => return Ok(None),
            None => return Err(dangling("curve handle cannot be resolved")),
        };
        geometry
            .insert_curve(copy)
            .map(Some)
            .ok_or(dangling("curve store is full"))
    }

    // ---- the mate maps -------------------------------------------------------------------

    /// `GetContext`: the kernel is passed to each method; this returns `k` for callers
    /// written against upstream's shape.
    #[must_use]
    pub fn context<'k>(&self, k: &'k Kernel) -> &'k Kernel {
        k
    }

    /// `GetOtherMate`: the other B-rep entity mated to `brep_entity`, `None` without a
    /// relationship (or without maps).
    #[must_use]
    pub fn other_mate(&self, brep_entity: Option<O>) -> Option<O> {
        let map = self.b_to_o.as_ref()?;
        map.get(&brep_entity?).copied().flatten()
    }

    /// `GetThisMate`: the B-rep entity mated to `other_entity`, `None` without a
    /// relationship (or without maps).
    #[must_use]
    pub fn this_mate(&self, other_entity: Option<O>) -> Option<O> {
        let map = self.o_to_b.as_ref()?;
        map.get(&other_entity).copied()
    }

    /// `IsMatedPair`: whether the two objects are mated (`false` when either is null).
    #[must_use]
    pub fn is_mated_pair(&self, brep_entity: Option<O>, other_entity: Option<O>) -> bool {
        let (Some(b), Some(o)) = (brep_entity, other_entity) else {
            return false;
        };
        // SM_ASSERT(pBrepEntity == GetThisMate(pOtherEntity)) only reports
        Some(o) == self.other_mate(Some(b))
    }

    /// `GetBrepMate`: the B-rep entity mated to `other_entity`, `None` without a
    /// relationship (or without maps).
    #[must_use]
    pub fn brep_mate(&self, other_entity: Option<O>) -> Option<O> {
        if self.b_to_o.is_some() {
            // when Brep to OtherBrep Map exists - OtherBrep to Brep map also exists
            self.this_mate(other_entity)
        } else {
            None
        }
    }

    /// `GetMappedBrepObjects(eDesiredType, rList)`: every mated B-rep object of
    /// `kind` (the values of `m_pOToB`). Upstream lists them in the map's hash order;
    /// the port lists them in the key order of the other handles they are mated to.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an intersector without maps (upstream
    /// dereferences null).
    pub fn mapped_brep_objects(&self, kind: ClassObjectKind) -> Result<Vec<O>> {
        let map = self.o_to_b.as_ref().ok_or(dangling("no mate maps"))?;
        Ok(map
            .values()
            .filter(|v| v.class_kind() == kind)
            .copied()
            .collect())
    }

    /// `GetCommonEdges(rEdges, rOtherEdges)`: every B-rep edge with an other mate and
    /// that mate, in the B-rep's edge order.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a B-rep; those of
    /// [`IntersectorBrep::brep_edges`].
    pub fn common_edges(&self, geometry: &impl IntersectorBrep<O>) -> Result<(Vec<O>, Vec<O>)> {
        let brep = self.brep.ok_or(dangling("no B-rep"))?;
        let edges = geometry.brep_edges(brep)?;
        Ok(common_mates(edges, |edge| self.other_mate(Some(edge))))
    }

    /// `GetCommonVertices(rVertices, rOtherVertices)`: every B-rep vertex with an other
    /// mate and that mate, in the B-rep's vertex order.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a B-rep; those of
    /// [`IntersectorBrep::brep_vertices`].
    pub fn common_vertices(&self, geometry: &impl IntersectorBrep<O>) -> Result<(Vec<O>, Vec<O>)> {
        let brep = self.brep.ok_or(dangling("no B-rep"))?;
        let vertices = geometry.brep_vertices(brep)?;
        Ok(common_mates(vertices, |vertex| {
            self.other_mate(Some(vertex))
        }))
    }

    /// `Relate(pBrepTopo, pOtherTopo)`: mates the two objects. Vertices and edges
    /// first get the larger of the two tolerances (`SetTolerance` with
    /// `bUpdateOnlyIfLarger`, upstream's default).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either object already has a different mate;
    /// `SM_ERR_NULL_POINTER` when a vertex (edge) is related to a non-vertex
    /// (non-edge); [`SolidError::InvalidInput`] for a null B-rep object or an
    /// intersector without maps (upstream dereferences null); those of the tolerance
    /// setters. A null other object of another kind is stored (`GetBrepMate(NULL)`
    /// then finds the B-rep object, as upstream).
    pub fn relate(
        &mut self,
        k: &Kernel,
        brep: &mut impl IntersectorBrep<O>,
        brep_topo: Option<O>,
        other_topo: Option<O>,
    ) -> Result<()> {
        // check for conflicting existing mate relationships
        let oth = self.other_mate(brep_topo);
        let brp = self.brep_mate(other_topo);

        if oth.is_some() && oth != other_topo {
            return Err(SolidError::sm_err(OP_RELATE));
        }
        if brp.is_some() && brp != brep_topo {
            return Err(SolidError::sm_err(OP_RELATE));
        }

        let brep_topo = brep_topo.ok_or(dangling("null B-rep object"))?;

        // when objects are vertices - set object tolerances to max(object1 tol, object2 tol)
        if brep_topo.class_kind() == ClassObjectKind::Vertex {
            let v2 = other_topo
                .filter(|o| o.class_kind() == ClassObjectKind::Vertex)
                .ok_or(SolidError::null_pointer(OP_RELATE))?;
            Self::max_tolerances(k, brep, brep_topo, v2)?;
        }

        // when objects are edges - set object tolerances to max(object1 tol, object2 tol)
        if brep_topo.class_kind() == ClassObjectKind::Edge {
            let e2 = other_topo
                .filter(|o| o.class_kind() == ClassObjectKind::Edge)
                .ok_or(SolidError::null_pointer(OP_RELATE))?;
            Self::max_tolerances(k, brep, brep_topo, e2)?;
        }

        // add the pair to the Ptr to Ptr map (a null other object is stored as such)
        let b_to_o = self.b_to_o.as_mut().ok_or(dangling("no mate maps"))?;
        b_to_o.insert(brep_topo, other_topo);
        let o_to_b = self.o_to_b.as_mut().ok_or(dangling("no mate maps"))?;
        o_to_b.insert(other_topo, brep_topo);
        Ok(())
    }

    /// The two `SetTolerance` lines of `Relate`: each object gets the other's
    /// tolerance when that is larger.
    fn max_tolerances(k: &Kernel, brep: &mut impl IntersectorBrep<O>, t1: O, t2: O) -> Result<()> {
        let tol1 = brep
            .topology_tolerance(t1)
            .ok_or(dangling("topology handle is unknown"))?;
        let tol2 = brep
            .topology_tolerance(t2)
            .ok_or(dangling("topology handle is unknown"))?;
        if tol1 < tol2 {
            ignore_soft(brep.set_topology_tolerance(k, t1, tol2, true, true))?;
        }
        let tol1 = brep
            .topology_tolerance(t1)
            .ok_or(dangling("topology handle is unknown"))?;
        if tol2 < tol1 {
            ignore_soft(brep.set_topology_tolerance(k, t2, tol1, true, true))?;
        }
        Ok(())
    }

    /// `FixupEndPointClassMates(lPointDim, rPCBrep, rPCOther, cpClassificationCurve)`:
    /// checks two end point classifications that both classify to vertices against the
    /// mate maps. The violation only warns upstream (nothing is changed); the point
    /// dimension and curve are debugging aids. Always `SM_SUCCESS`.
    pub fn fixup_end_point_class_mates(
        &self,
        point_dim: u64,
        pc_brep: &mut PointClassification<O>,
        pc_other: &mut PointClassification<O>,
        classification_curve: Option<O>,
    ) {
        let _ = (point_dim, classification_curve);
        // Point classification types
        let brep_class = pc_brep.point_class();
        let other_class = pc_other.point_class();

        if brep_class != PointClassType::Vertex || other_class != PointClassType::Vertex {
            return;
        }

        // At this point, both classify to vertices.

        // get the ThisBrep Object and its OtherBrep mate
        let brep_object = pc_brep.object();
        let brep_mate = brep_object.and_then(|o| self.other_mate(Some(o)));

        // get the OtherBrep Object and its ThisBrep mate
        let other_object = pc_other.object();
        let other_mate = other_object.and_then(|o| self.brep_mate(Some(o)));

        // when either point classifies to a mated vertex
        // When both Points don't map to one another's mates: an error condition
        // reported with SM_DBG_WARN upstream; nothing is changed (release build).
        let _ = (brep_mate, other_mate);
    }

    /// `FixupMidPointClassMates(lPointDim, rPCBrep, rPCOther, cpClassificationCurve)`:
    /// checks two mid point classifications that classify to mated edges or vertices
    /// against the mate maps; when both classify to edges whose mates are not each
    /// other, both are reset to `SM_PC_UNKNOWN`. The point dimension and curve are
    /// debugging aids. Always `SM_SUCCESS`.
    pub fn fixup_mid_point_class_mates(
        &self,
        point_dim: u64,
        pc_brep: &mut PointClassification<O>,
        pc_other: &mut PointClassification<O>,
        classification_curve: Option<O>,
    ) {
        let _ = (point_dim, classification_curve);
        // Point classification types
        let brep_class = pc_brep.point_class();
        let other_class = pc_other.point_class();

        // get the ThisBrep Object and its OtherBrep mate
        let brep_object = pc_brep.object();
        let brep_mate = brep_object.and_then(|o| self.other_mate(Some(o)));

        // get the OtherBrep Object and its ThisBrep mate
        let other_object = pc_other.object();
        let other_mate = other_object.and_then(|o| self.brep_mate(Some(o)));

        let is_edge_or_vertex =
            |c: PointClassType| c == PointClassType::Vertex || c == PointClassType::Edge;

        // when either point classifies to a mated vertex or edge
        if (is_edge_or_vertex(brep_class) && brep_mate.is_some())
            || (is_edge_or_vertex(other_class) && other_mate.is_some())
        {
            // When both Points don't map to one another's mates.
            // Note, not a problem when the topologies are different,
            // e.g., one classifies to an Edge and the other to a Face. [B279]
            if (other_mate != brep_object || brep_mate != other_object) && brep_class == other_class
            {
                // This is an error condition (SM_DBG_WARN upstream).
                // When both points classify to edges, reset point classifications back to unknown.
                if brep_class == PointClassType::Edge && other_class == PointClassType::Edge {
                    pc_brep.set_class_object(PointClassType::Unknown, None);
                    pc_other.set_class_object(PointClassType::Unknown, None);
                }
            }
        }
    }

    /// `SetSubset(crSubsetGeometry)`: the subset of the other B-rep's geometry objects
    /// to intersect (a surface per face, a curve per edge, a vertex per vertex),
    /// replacing any previous subset; an empty list clears it. Always `SM_SUCCESS`.
    pub fn set_subset(&mut self, subset_geometry: &[O]) {
        set_subset_objects(&mut self.subset, subset_geometry);
    }

    /// Clears both canonical mate maps (`m_pBToO->RemoveAll`,
    /// `m_pOToB->RemoveAll`) without replacing the intersector or its controls.
    /// The operand constructor must have created both maps first.
    pub fn clear_relationships(&mut self) -> Result<()> {
        self.b_to_o
            .as_mut()
            .ok_or(dangling("no mate maps"))?
            .clear();
        self.o_to_b
            .as_mut()
            .ok_or(dangling("no mate maps"))?
            .clear();
        Ok(())
    }

    /// `GetSubset`: the subset, if one was set.
    #[must_use]
    pub const fn subset(&self) -> Option<&BTreeSet<O>> {
        self.subset.as_ref()
    }

    // ---- simple member access --------------------------------------------------------

    /// `GetPrimaryBrep` (`m_pBrep`).
    #[must_use]
    pub const fn primary_brep(&self) -> Option<O> {
        self.brep
    }

    /// `GetOtherBrep` (`m_pOther`).
    #[must_use]
    pub const fn other_brep(&self) -> Option<O> {
        self.other
    }

    /// `GetThisApproxTol3d` (`SM_ASSERT_TOL` only reports).
    #[must_use]
    pub const fn this_approx_tol_3d(&self) -> f64 {
        self.this_approx_tol_3d
    }

    /// `GetThisAngTolRad` (`SM_ASSERT_TOL` only reports).
    #[must_use]
    pub const fn this_ang_tol_rad(&self) -> f64 {
        self.this_ang_tol_rad
    }

    /// `SetPrimaryBrep`.
    pub fn set_primary_brep(&mut self, brep: Option<O>) {
        self.brep = brep;
    }

    /// `SetOtherBrep`.
    pub fn set_other_brep(&mut self, other: Option<O>) {
        self.other = other;
    }

    /// `SetThisApproxTol3d`.
    pub fn set_this_approx_tol_3d(&mut self, this_approx_tol_3d: f64) {
        self.this_approx_tol_3d = this_approx_tol_3d;
    }

    /// `SetThisAngTolRad`.
    pub fn set_this_ang_tol_rad(&mut self, this_ang_tol_rad: f64) {
        self.this_ang_tol_rad = this_ang_tol_rad;
    }

    /// `m_bSwappedOrder`.
    #[must_use]
    pub const fn swapped_order(&self) -> bool {
        self.swapped_order
    }

    /// The proto topology manager (`m_pProtoTopoMgr`), if one is installed.
    #[must_use]
    pub const fn proto_topology_manager(&self) -> Option<&ProtoTopologyManager<O>> {
        self.proto_topo_mgr.as_ref()
    }

    /// Mutable access to the proto topology manager.
    pub fn proto_topology_manager_mut(&mut self) -> Option<&mut ProtoTopologyManager<O>> {
        self.proto_topo_mgr.as_mut()
    }

    /// Installs (or removes) the proto topology manager (made by
    /// `IntersectInsertRelate`, unit `boolean.topology_intersector_p2`).
    pub fn set_proto_topology_manager(&mut self, mgr: Option<ProtoTopologyManager<O>>) {
        self.proto_topo_mgr = mgr;
    }

    // ---- resolving and imprinting the proto topology ----------------------------------

    /// `ResolveIntersections`: resolves every proto vertex, then every proto edge, then
    /// the overlapping proto edges of the manager. Problems in each resolution only
    /// warn upstream. Returns the curve handles deleted. Always `SM_SUCCESS`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a manager (upstream dereferences null);
    /// [`SolidError::Unported`] / [`SolidError::InvalidInput`] from the resolutions.
    pub fn resolve_intersections(
        &mut self,
        k: &Kernel,
        brep: &mut impl IntersectorBrep<O>,
    ) -> Result<Vec<O>> {
        let approx_tol = self.this_approx_tol_3d;
        let mgr = self
            .proto_topo_mgr
            .as_mut()
            .ok_or(dangling("no proto topology manager"))?;
        let mut deleted = Vec::new();

        // Resolve every ProtoVertex
        let pvs = mgr.proto_vertices().to_vec();
        for pv in pvs {
            // resolve ProtoVertex; a problem only warns
            ignore_soft(mgr.resolve_proto_vertex(k, brep, pv))?;
        }

        // Resolve every ProtoEdge
        let pes = mgr.proto_edges().to_vec();
        for pe in pes {
            if let Some(d) = ignore_soft(mgr.resolve_proto_edge(k, brep, pe))? {
                deleted.extend(d);
            }
        }

        // Resolve OverlappingPEs (SE_MSG: the status only warns)
        if let Some(d) = ignore_soft(mgr.resolve_overlapping_proto_edges(k, brep, approx_tol))? {
            deleted.extend(d);
        }

        // all done
        Ok(deleted)
    }

    /// `ImprintIntersections`: imprints every vertex definition of every proto vertex,
    /// then every edge definition of every proto edge (an edge definition that fails to
    /// imprint is removed from its proto edge and deleted), then glues the coincident
    /// B-rep vertices; both B-reps are editable and the context is "doing a Boolean"
    /// meanwhile. Returns the first failing status (`eRetStat`) as the error, after
    /// everything ran, or the curve handles deleted.
    ///
    /// # Errors
    ///
    /// The first failing imprint or glue status; [`SolidError::InvalidInput`] without
    /// a manager or B-reps (upstream dereferences null); [`SolidError::Unported`] /
    /// [`SolidError::InvalidInput`] from the imprints.
    pub fn imprint_intersections(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
    ) -> Result<Vec<O>> {
        let outcome = self.imprint_intersections_reporting_edits(k, brep)?;
        outcome.status.map(|()| outcome.deleted)
    }

    /// [`TopologyIntersector::imprint_intersections`], answering its status
    /// (`eRetStat`) beside the curve handles deleted and whether any failing imprint
    /// may have edited a B-rep ([`ImprintOutcome`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a manager or B-reps;
    /// [`SolidError::Unported`] / [`SolidError::InvalidInput`] from the imprints.
    pub fn imprint_intersections_reporting_edits(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
    ) -> Result<ImprintOutcome<O>> {
        // Don't SER on each imprint: if one fails, keep pushing on.
        let mut ret: Result<()> = Ok(());
        let mut deleted = Vec::new();
        let mut failure_edited = false;

        let this_brep = self.brep.ok_or(dangling("no B-rep"))?;
        let other_brep = self.other.ok_or(dangling("no other B-rep"))?;
        let old_edit1 = brep.set_editing_enabled(this_brep, true)?;
        let old_edit2 = brep.set_editing_enabled(other_brep, true)?;
        let old_doing_boolean = k.context.doing_boolean();
        k.context.set_doing_boolean(true);

        let result = self.imprint_body(k, brep, &mut ret, &mut deleted, &mut failure_edited);

        // SmTemporaryChangeValue destructors
        k.context.set_doing_boolean(old_doing_boolean);
        brep.set_editing_enabled(other_brep, old_edit2)?;
        brep.set_editing_enabled(this_brep, old_edit1)?;

        result?;
        Ok(ImprintOutcome {
            deleted,
            status: ret,
            failure_edited,
        })
    }

    /// The body of [`TopologyIntersector::imprint_intersections`] between the temporary
    /// changes.
    fn imprint_body(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
        ret: &mut Result<()>,
        deleted: &mut Vec<O>,
        failure_edited: &mut bool,
    ) -> Result<()> {
        let mgr = self
            .proto_topo_mgr
            .as_mut()
            .ok_or(dangling("no proto topology manager"))?;

        // record the first problem; propagate what cannot be ignored
        let record = |r: Result<()>, ret: &mut Result<()>| -> Result<bool> {
            match r {
                Ok(()) => Ok(true),
                Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => Err(e),
                Err(e) => {
                    if ret.is_ok() {
                        *ret = Err(e);
                    }
                    Ok(false)
                }
            }
        };

        let pvs = mgr.proto_vertices().to_vec();
        for pv in pvs {
            // Q: if a PV has more than one VD, do we connect them with a new Edge?
            // A: No: if they're meant to be connected, then there will be a ProtoEdge
            //    indicating that.
            let vds = mgr
                .proto_vertex(pv)
                .ok_or(dangling("proto vertex is deleted"))?
                .vertex_definitions()
                .to_vec();
            for vd in vds {
                // (a vertex definition's first step is a B-rep merge)
                if !record(mgr.imprint_vertex_definition(k, brep, vd), ret)? {
                    *failure_edited = true;
                }
            }
        }

        let pes = mgr.proto_edges().to_vec();
        for pe in pes {
            let mut jj = 0usize;
            while let Some(&ed) = mgr
                .proto_edge(pe)
                .ok_or(dangling("proto edge is deleted"))?
                .edge_definitions()
                .get(jj)
            {
                let (result, edited) = mgr.imprint_edge_definition_reporting_edits(k, brep, ed);
                if record(result, ret)? {
                    jj += 1;
                } else {
                    *failure_edited |= edited;
                    // Delete its curves, which would have been consumed if Imprint() had worked.
                    ignore_soft(
                        mgr.proto_edge_mut(pe)
                            .ok_or(dangling("proto edge is deleted"))?
                            .remove_edge_definition(ed),
                    )?;
                    deleted.extend(mgr.delete_edge_definition(ed));
                    // jj--: the next definition took this slot
                }
            }
        }

        // WR-306 (opt-in, the SMLib fork's fix): a proto vertex that absorbed a degenerate
        // proto edge (`resolve_proto_edge`) is one vertex for a curve that ran a little way
        // through it, so the edges ending there end on that curve, not on the vertex's
        // point. Fit each such vertex to its edges, as CombineCoincidentVertices does when
        // it squeezes a short edge away. Its two vertices, one in each B-rep, will be
        // related as one, so each tolerance also covers the other's.
        if k.fixes.degenerate_proto_edge_in_one_vertex {
            let pvs = mgr.proto_vertices().to_vec();
            for pv in pvs {
                let proto_vertex = mgr
                    .proto_vertex(pv)
                    .ok_or(dangling("proto vertex is deleted"))?;
                if !proto_vertex.absorbed_proto_edge() {
                    continue;
                }
                let vds = proto_vertex.vertex_definitions().to_vec();
                for vd in vds {
                    let def = mgr
                        .vertex_def(vd)
                        .ok_or(dangling("vertex definition is deleted"))?;
                    let vtx = [def.final_vertex(1), def.final_vertex(2)];
                    for v in vtx.into_iter().flatten() {
                        if brep.vertex_brep(v)?.is_none() {
                            continue;
                        }
                        // Record the first problem.
                        record(brep.vertex_update_edge_intervals(k, v), ret)?;
                    }
                    let [Some(v0), Some(v1)] = vtx else {
                        continue;
                    };
                    let p0 = ClassGeometry::classification_vertex_point(&*brep, v0)
                        .ok_or(dangling("vertex has no point"))?;
                    let p1 = ClassGeometry::classification_vertex_point(&*brep, v1)
                        .ok_or(dangling("vertex has no point"))?;
                    let apart = p0.distance(p1);
                    let tol0 = ProtoBrep::topology_tolerance(&*brep, v0)
                        .ok_or(dangling("vertex has no tolerance"))?;
                    let tol1 = ProtoBrep::topology_tolerance(&*brep, v1)
                        .ok_or(dangling("vertex has no tolerance"))?;
                    // Will set it only if larger.
                    brep.set_topology_tolerance(k, v0, tol1 + 2.0 * apart, true, true)?;
                    brep.set_topology_tolerance(k, v1, tol0 + 2.0 * apart, true, true)?;
                }
            }
        }

        // At this point, there might be coincident Vertices and Edges in the Breps.
        // The rest of ManifoldBoolean() expects coincident Vertices to be glued
        // (although not coincident Edges).  So do that.  [Offset demo 2]
        // (m_pTI's GetOtherMate / RemoveRelationship: the maps are moved out while the
        // intersector's own manager runs)
        let mut mates = super::p2::MapMates {
            b_to_o: self.b_to_o.take(),
            o_to_b: self.o_to_b.take(),
        };
        let glued = mgr.glue_coincident_brep_vertices(k, brep, &mut mates);
        self.b_to_o = mates.b_to_o;
        self.o_to_b = mates.o_to_b;
        match glued {
            Ok(d) => deleted.extend(d),
            Err(e) => {
                record(Err(e), ret)?;
                *failure_edited = true;
            }
        }

        Ok(())
    }

    // ---- the six internal-use methods -----------------------------------------------------

    /// `VertexFaceIntersect(rVerts, rFaces, ..)`: for each vertex of `verts`, imprints
    /// it into each face of `faces` it lies on (a new vertex loop in the face, glued
    /// to the vertex, whose position becomes the midpoint); such vertices are set to
    /// null in `verts` and listed as processed. `making_manifold_solid` is `NotUsed`.
    ///
    /// # Errors
    ///
    /// Unless `ignore_problems`, a failing imprint or a vertex without B-rep (`SER`);
    /// [`SolidError::Unported`] for the imprints (`SmBrep::MakeVertexLoop`,
    /// `GlueVertices`); [`SolidError::InvalidInput`] for null handles upstream
    /// dereferences.
    pub fn vertex_face_intersect(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
        verts: &mut [Option<O>],
        faces: &[Option<O>],
        making_manifold_solid: bool,
        ignore_problems: bool,
    ) -> Result<VertexFaceIntersect<O>> {
        let _ = making_manifold_solid;
        // Init outputs.
        let mut out = VertexFaceIntersect {
            processed_verts: Vec::new(),
            processed_faces: Vec::new(),
            max_vertex_gap: 0.0,
            min_unstitched_vert_gap: BIG_DOUBLE,
        };

        // Locals.
        let num_verts = verts.len();
        if num_verts < 1 {
            return Ok(out);
        }
        let num_faces = faces.len();
        if num_faces < 1 {
            return Ok(out);
        }

        // For each vertex in the list, look for close faces.
        for ii in 0..num_verts {
            let Some(v) = verts.get(ii).copied().flatten() else {
                continue;
            };

            let pv_point = brep.vertex_point(v)?;

            // Find all faces close to this vertex (the bounding box is computed and
            // unused upstream).
            let v_tol = brep
                .topology_tolerance(v)
                .ok_or(dangling("vertex handle is unknown"))?;
            let mut v_bbox = Extent3d::from_points(pv_point, pv_point);
            v_bbox.expand_absolute(v_tol);
            for &face in faces {
                let Some(face) = face else {
                    continue;
                };

                // (the vertex tolerance is re-read: UpdateObjectTolerance below may have
                // raised it for an earlier face)
                let v_tol_now = brep
                    .topology_tolerance(v)
                    .ok_or(dangling("vertex handle is unknown"))?;
                let this_tol = max2(v_tol_now, self.this_approx_tol_3d);

                let imprint = match test_and_imprint_face(k, brep, face, pv_point, this_tol) {
                    Ok(r) => r,
                    Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => {
                        return Err(e)
                    }
                    Err(e) => {
                        if ignore_problems {
                            continue;
                        }
                        return Err(e);
                    }
                };
                let dist_to_face = imprint.dist_to_face;

                // when imprint created a new vertex
                if let Some(new_vertex) = imprint.new_vertex {
                    // Glue the new vertex to pV, and switch lists.
                    // (a vertex without B-rep: skipped with bIgnoreProblems, dereferenced
                    // otherwise)
                    let Some(v_brep) = brep.vertex_brep(v)? else {
                        if ignore_problems {
                            continue;
                        }
                        return Err(dangling("vertex without B-rep"));
                    };

                    // First set the position of the surviving vertex to be
                    // the midpoint of the two ... this should help to keep tolerances
                    // from growing excessively.
                    let new_point = brep.vertex_point(new_vertex)?;
                    let v_point = brep.vertex_point(v)?;
                    let mid = 0.5 * (v_point + new_point);
                    brep.set_vertex_point(k, v, mid)?;

                    ignore_soft(brep.glue_vertices(k, v_brep, v, new_vertex, None))?;
                    add_unique(&mut out.processed_verts, v);
                    if let Some(slot) = verts.get_mut(ii) {
                        *slot = None;
                    }

                    add_unique(&mut out.processed_faces, face);

                    // Set max distance.
                    out.max_vertex_gap = max2(out.max_vertex_gap, dist_to_face);

                    ignore_soft(brep.update_object_tolerance(k, v, dist_to_face, 0.0, false))?;
                    ignore_soft(brep.update_object_tolerance(k, face, dist_to_face, 0.0, false))?;
                } else {
                    // Set min distance, if it didn't split due to being
                    // too far away (as opposed to being at a vertex).
                    if imprint.reason == Reason::FarFromGeom {
                        out.min_unstitched_vert_gap =
                            min2(out.min_unstitched_vert_gap, dist_to_face);
                    }
                }
            } // end for all close faces to pV
        } // end loop on unprocessed vertices

        Ok(out)
    }

    /// `EdgeFaceIntersect(rEdges, rFaces, ..)`: intersects each edge of `edges` with
    /// each face of `faces` it is not connected to (bounding boxes permitting); every
    /// single-point intersection interior to both is collected first, then imprinted:
    /// a vertex loop in the face and a split of the edge (or its existing vertex when
    /// the parameter is at an end), glued together at their midpoint.
    /// `making_manifold_solid` is `NotUsed`.
    ///
    /// # Errors
    ///
    /// Unless `ignore_problems`, a failing vertex loop or edge split (`SER`); those of
    /// the geometry queries; [`SolidError::Unported`] from the adapter's face bounding box
    /// and the stubs after it (`gen0011` defers at `SmBrep::CalculateBoundingBox`,
    /// `query.brep`; six cases at `SmFace::GetUpwardLoopuses`, `topology.face_p4`),
    /// `SmFace::CalculateUVDomainFromUVTrimCurves`'s
    /// `CreateUVTrimCurves`, `SmSurface::GlobalCurveIntersect` and the B-rep edits;
    /// [`SolidError::InvalidInput`] for null handles upstream dereferences.
    #[allow(clippy::too_many_lines)]
    pub fn edge_face_intersect(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
        edges: &[Option<O>],
        faces: &[Option<O>],
        making_manifold_solid: bool,
        ignore_problems: bool,
    ) -> Result<EdgeFaceIntersect<O>> {
        let _ = making_manifold_solid;
        // Init outputs.
        let mut out = EdgeFaceIntersect {
            new_verts: Vec::new(),
            split_edges: Vec::new(),
            new_edges: Vec::new(),
            processed_faces: Vec::new(),
            max_vertex_gap: 0.0,
            min_unstitched_vert_gap: BIG_DOUBLE,
        };

        // Locals.
        if edges.is_empty() {
            return Ok(out);
        }
        if faces.is_empty() {
            return Ok(out);
        }
        let mut reason = Reason::NoReason;

        // A complication: if more than one intersection, then after the
        // first split, the next intersection could be in either of the
        // split edges.  So we have to look at all of them.
        let mut edges_split_from_this: Vec<O> = Vec::new();

        // Collect all intersections first, then process them afterwards, in these
        // arrays, which will all be kept in sync:
        let mut s_edges: Vec<O> = Vec::new();
        let mut s_faces: Vec<O> = Vec::new();
        let mut s_edge_params: Vec<f64> = Vec::new();
        let mut s_face_params: Vec<Point2> = Vec::new();

        // For each edge in the list, look for nearby faces.
        for &edge in edges {
            // SmTol::GetZoneTol3d(pEdge) is read before the null check upstream
            let Some(edge) = edge else {
                continue;
            };
            let edge_zone_tol_3d = brep
                .topology_tolerance(edge)
                .ok_or(dangling("edge handle is unknown"))?;
            let edge_curve = brep
                .edge_curve(edge)
                .ok_or(dangling("edge handle is unknown"))?;
            let edge_domain = brep
                .edge_interval(edge)
                .ok_or(dangling("edge handle is unknown"))?;

            // We will skip faces and edges that are already connected.
            let this_edges_faces = brep.edge_faces(edge)?;

            // Find all input faces close to this edge.
            let mut e_bbox =
                ignore_soft(brep.edge_bounding_box(k, edge))?.unwrap_or_else(Extent3d::new);
            e_bbox.expand_absolute(edge_zone_tol_3d);
            // (sFBBox is declared with sEBBox: a failed CalculateBoundingBox keeps the
            // previous face's box)
            let mut f_bbox = Extent3d::new();
            for &face in faces {
                let Some(face) = face else {
                    continue;
                };
                let face_zone_tol_3d = brep
                    .topology_tolerance(face)
                    .ok_or(dangling("face handle is unknown"))?;

                // If face and edge are already connected, skip it.
                if contains(&this_edges_faces, &face) {
                    continue;
                }

                // BBox check.
                if let Some(b) = ignore_soft(brep.face_calculate_bounding_box(k, face, false))? {
                    f_bbox = b;
                }
                if e_bbox.are_disjoint(&f_bbox, face_zone_tol_3d) {
                    continue;
                }

                let this_tol = max2(edge_zone_tol_3d, self.this_approx_tol_3d);

                let surf = brep
                    .face_surface(face)
                    .ok_or(dangling("face handle is unknown"))?
                    .ok_or(dangling("face without surface"))?;
                let mut face_domain = brep
                    .face_uv_domain(face)
                    .ok_or(dangling("face handle is unknown"))?;

                // If possible, reduce the surface domain.
                // This can save a lot of work; e.g., two halves of a cylinder.
                if let Some(d) = face_uv_domain_from_uv_trim_curves(k, brep, face)? {
                    face_domain = d;
                }

                // (the edge's curve is dereferenced here, after the connected-face and box
                // filters)
                let curve = edge_curve.ok_or(dangling("edge without curve"))?;
                let solutions = ignore_soft(surface_global_curve_intersect(
                    k,
                    brep,
                    surf,
                    &face_domain,
                    curve,
                    &edge_domain,
                    this_tol,
                ))?
                .unwrap_or_else(SolutionArray::new);

                let num_sols = solutions.len();

                // If the edge is coincident with the face, it will return several solutions.
                // That is not what we're looking for.
                if num_sols > 3 {
                    continue;
                }

                for sol in solutions.as_slice() {
                    // Don't deal with range solutions.
                    if sol.solution_type != SolutionType::SingleValue {
                        continue;
                    }

                    // Check interior to both Edge and Face.
                    let t = sol.start.param(0)?;

                    // Check interior of Edge (the status is not checked upstream; bInterior
                    // is FALSE on trouble)
                    let mut interior =
                        ignore_soft(check_interior_edge(k, brep, edge, t, this_tol))?
                            .unwrap_or(false);

                    if !interior {
                        reason = Reason::CloseToBoundary;
                    }

                    // Check the face (if this intersection is interior to the edge).
                    // Note, it will presumably not be close to a vertex or an edge of the face.
                    let uv = Point2::new(sol.start.param(1)?, sol.start.param(2)?);

                    if interior {
                        // this tol is unlikely to be correct - check it
                        let mut point_class = PointClassification::new(face_zone_tol_3d);
                        ignore_soft(brep.face_point_classify(k, face, uv, &mut point_class))?;

                        match point_class.point_class() {
                            PointClassType::Unknown => {
                                interior = false;
                                reason = Reason::FarFromGeom;
                            }
                            PointClassType::Edge | PointClassType::Vertex => {
                                interior = false;
                                reason = Reason::CloseToBoundary;
                            }
                            PointClassType::Face => {}
                            _ => {
                                // SM_ASSERT(FALSE)
                                interior = false;
                                reason = Reason::CloseToBoundary;
                            }
                        }
                    }

                    if interior {
                        // Intersection is interior to both edge and face.
                        // Save the intersection info.
                        s_edges.push(edge);
                        s_faces.push(face);
                        s_edge_params.push(t);
                        s_face_params.push(uv);
                    }
                } // end for all intersection solutions
            } // end for all nearby faces to pEdge
        } // end loop on input edges

        // Now process each intersection found.
        let num_ints = s_edges.len();

        for ii in 0..num_ints {
            let mut edge = *s_edges
                .get(ii)
                .ok_or(dangling("intersection lists differ"))?;
            let face = *s_faces
                .get(ii)
                .ok_or(dangling("intersection lists differ"))?;
            let param = *s_edge_params
                .get(ii)
                .ok_or(dangling("intersection lists differ"))?;
            let uv = *s_face_params
                .get(ii)
                .ok_or(dangling("intersection lists differ"))?;

            let curve = brep
                .edge_curve(edge)
                .ok_or(dangling("edge handle is unknown"))?
                .ok_or(dangling("edge without curve"))?;
            let surf = brep
                .face_surface(face)
                .ok_or(dangling("face handle is unknown"))?
                .ok_or(dangling("face without surface"))?;
            let f_brep = brep
                .face_brep(face)?
                .ok_or(dangling("face without B-rep"))?;

            // Find the point -- make sure they're the same. Can err if pEdge was edited by prior loop iteration
            let mut edge_pt = eval_curve_point(k, brep, curve, param)?;
            // (the status is ignored upstream: UNDEF on failure)
            let face_pt = ignore_soft(
                brep.surface(surf)
                    .ok_or(dangling("surface handle cannot be resolved"))?
                    .evaluate_point(k, uv),
            )?
            .unwrap_or(Point3::UNDEF);

            let face_tol = brep
                .topology_tolerance(face)
                .ok_or(dangling("face handle is unknown"))?;
            let edge_tol = brep
                .topology_tolerance(edge)
                .ok_or(dangling("edge handle is unknown"))?;
            let mut this_tol = max2(face_tol, self.this_approx_tol_3d);
            this_tol = max2(edge_tol, this_tol);
            let mut this_deviation = edge_pt.distance(face_pt);

            if this_deviation > this_tol {
                // Try to find the edge that had the intersection in rNewEdges
                for &this_edge in &out.new_edges {
                    let this_edge = this_edge.ok_or(dangling("null edge in rNewEdges"))?;
                    let c = brep
                        .edge_curve(this_edge)
                        .ok_or(dangling("edge handle is unknown"))?
                        .ok_or(dangling("edge without curve"))?;
                    edge_pt = eval_curve_point(k, brep, c, param)?;

                    let new_deviation = edge_pt.distance(face_pt);

                    if new_deviation < this_deviation {
                        this_deviation = new_deviation;
                        edge = this_edge;
                    }
                }
            }

            // Now do all the topology:
            // 1: create a new vtx in the Face
            // 2: chk each edge for param being interior
            // 3: if interior, split edge, add new edge to list -- get new vertex
            //    else get vertex
            // 4: glue verts.

            edges_split_from_this.clear();
            edges_split_from_this.push(edge);

            // Create a new Vertex in the Face.
            let new_face_vtx = match brep
                .make_vertex_loop(f_brep, face, face_pt)
                .map(|(l, v)| (l, Some(v)))
            {
                Ok((_loop, v)) => v,
                Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => {
                    return Err(e)
                }
                Err(e) => {
                    // SM_ASSERT(FALSE)
                    if ignore_problems {
                        continue;
                    }
                    return Err(e);
                }
            };

            add_unique(&mut out.processed_faces, face);

            let check_gaps = this_deviation > this_tol / 4.0;
            // (UpdateObjectTolerance of a null topology returns SM_SUCCESS)
            if let Some(fv) = new_face_vtx {
                ignore_soft(brep.update_object_tolerance(k, fv, this_deviation, 0.0, check_gaps))?;
            }
            ignore_soft(brep.update_object_tolerance(k, face, this_deviation, 0.0, false))?;

            // Check interior of Edge -- all edges in sEdgesSplitFromThis.
            let mut interior = false;
            let mut this_split_edge = edge;
            for &split_edge in &edges_split_from_this {
                this_split_edge = split_edge;
                interior = ignore_soft(check_interior_edge(
                    k,
                    brep,
                    this_split_edge,
                    param,
                    this_tol,
                ))?
                .unwrap_or(false);
                if interior {
                    break;
                }
            }

            let new_edge_vtx: Option<O> = if interior {
                // Intersection is interior to pThisSplitEdge.
                // Split the edge with a new vertex.
                let e_brep = brep
                    .edge_brep_handle(this_split_edge)?
                    .ok_or(dangling("edge without B-rep"))?;
                let (e1, e2, v) = match brep
                    .make_vertex_split_edge(k, e_brep, this_split_edge, param)
                    .map(|(e1, e2, v)| (Some(e1), Some(e2), Some(v)))
                {
                    Ok(r) => r,
                    Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => {
                        return Err(e)
                    }
                    Err(e) => {
                        // SM_ASSERT(FALSE)
                        if ignore_problems {
                            continue;
                        }
                        return Err(e);
                    }
                };
                out.new_verts.push(v); // We'll keep this vertex.

                let new_edge = if Some(this_split_edge) == e1 { e2 } else { e1 };

                // Add to the output lists.
                add_unique(&mut out.split_edges, edge); // pEdge: only input edges on this list.
                add_unique(&mut out.new_edges, new_edge);

                // Also add it to our working edge list.
                // (a null pNewEdge is never read from it: the list is only read in the
                // else branch below and is reset for the next intersection)
                if let Some(new_edge) = new_edge {
                    edges_split_from_this.push(new_edge);
                }

                // (pThisSplitEdge->GetTolerance() is read for each call: pE1 is usually the
                // split edge itself, which the first call may have raised; a null topology
                // makes UpdateObjectTolerance return SM_SUCCESS)
                let split_tol = brep
                    .topology_tolerance(this_split_edge)
                    .ok_or(dangling("edge handle is unknown"))?;
                if let Some(e1) = e1 {
                    ignore_soft(brep.update_object_tolerance(
                        k,
                        e1,
                        this_deviation,
                        split_tol,
                        false,
                    ))?;
                }
                let split_tol = brep
                    .topology_tolerance(this_split_edge)
                    .ok_or(dangling("edge handle is unknown"))?;
                if let Some(e2) = e2 {
                    ignore_soft(brep.update_object_tolerance(
                        k,
                        e2,
                        this_deviation,
                        split_tol,
                        false,
                    ))?;
                }
                if let Some(v) = v {
                    ignore_soft(brep.update_object_tolerance(
                        k,
                        v,
                        this_deviation,
                        0.0,
                        check_gaps,
                    ))?;
                }
                v
            } else {
                // Hit an existing vertex.  (Probably coincident Faces.)
                // Glue that vertex to the Face Vertex we just created.
                // Find that vertex: search params of boundaries of Edges in sEdgesSplitFromThis list.
                let mut found: Option<O> = None;

                // (pNewFaceVtx is dereferenced in the first pass of the loop, which always
                // runs: the list holds pEdge)
                let new_face_pt =
                    brep.vertex_point(new_face_vtx.ok_or(dangling("null pNewFaceVtx"))?)?;
                let num_split = edges_split_from_this.len();
                for (jj, &this_e) in edges_split_from_this.iter().enumerate() {
                    let vtx = brep
                        .edge_start_vertex(this_e)?
                        .ok_or(dangling("edge without start vertex"))?;
                    let vtx_pt = brep.vertex_point(vtx)?;
                    if vtx_pt.closer_than(this_tol, new_face_pt) {
                        found = Some(vtx);
                        break;
                    }

                    // Check top of domain for last piece.
                    if jj + 1 == num_split {
                        let vtx = brep
                            .edge_other_vertex(this_e, Some(vtx))?
                            .ok_or(dangling("edge without end vertex"))?;
                        let vtx_pt = brep.vertex_point(vtx)?;
                        if vtx_pt.closer_than(this_tol, new_face_pt) {
                            found = Some(vtx);
                            break;
                        }
                    }
                } // end for each edge split from pEdge
                found
            }; // end if-else edge param was interior

            // State: we have pNewFaceVtx and pNewEdgeVtx.
            // Glue them together (if indeed they both exist).
            if let (Some(ev), Some(fv)) = (new_edge_vtx, new_face_vtx) {
                // First set the position of the surviving vertex to be
                // the midpoint of the two ... this should help to keep tolerances
                // from growing excessively.
                let ev_pt = brep.vertex_point(ev)?;
                let fv_pt = brep.vertex_point(fv)?;
                let mid = 0.5 * (ev_pt + fv_pt);
                brep.set_vertex_point(k, ev, mid)?;

                ignore_soft(brep.glue_vertices(k, f_brep, ev, fv, None))?;

                // Set max distance.
                out.max_vertex_gap = max2(out.max_vertex_gap, this_deviation);

                ignore_soft(brep.update_object_tolerance(k, ev, this_deviation, 0.0, check_gaps))?;
            } else {
                // Set min distance, if it didn't split due to being
                // too far away (as opposed to being at a vertex).
                if reason == Reason::FarFromGeom {
                    out.min_unstitched_vert_gap = min2(out.min_unstitched_vert_gap, this_deviation);
                }
            }
        } // end for all intersection found

        Ok(out)
    }

    /// `FaceFaceIntersect(rInputVerts, rFaceList1, rFaceList2, ..)`: intersects each
    /// face of `face_list1` with each face of `face_list2` that shares two of
    /// `input_verts` with it or when either face is closed (coincident faces, found
    /// through common edges, are skipped), and imprints each intersection interval
    /// that classifies to both faces as a new edge in each face (the faces are split as
    /// needed and the two edges glued). The working copies of `face_list1` and
    /// `face_list2` grow with the faces split off (the inputs are declared in/out
    /// upstream but never written); past [`MAX_FACE_LIST`] faces the function fails
    /// (`SM_ERR`; a bound of the port). `ignore_problems` is `NotUsed`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for `SmFace::GetVertices` / `GetEdges`
    /// (`topology.face_p4`), `SmSurface::CoincidenceCheck`, `SmFace::FaceIntersect`,
    /// `Point3DClassify` and the B-rep edits; [`SolidError::InvalidInput`] for null
    /// handles upstream dereferences; those of the surface queries.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub fn face_face_intersect(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
        input_verts: &[O],
        face_list1: &[O],
        face_list2: &[O],
        ignore_problems: bool,
        do_region_nesting: bool,
    ) -> Result<FaceFaceIntersect<O>> {
        let _ = ignore_problems;
        // Init outputs
        let mut out = FaceFaceIntersect {
            processed_faces: Vec::new(),
            new_faces: Vec::new(),
            new_edges: Vec::new(),
            max_gap: 0.0,
            min_unstitched: BIG_DOUBLE,
        };

        // Locals (sFaces1 / sFaces2 are copies of the input lists; the inputs are
        // declared in/out but never written upstream)
        let mut faces1: Vec<O> = face_list1.to_vec();
        let mut faces2: Vec<O> = face_list2.to_vec();
        let this_brep = self.brep.ok_or(dangling("no B-rep"))?;

        // Compare each face in rFaceList1 with each face in rFaceList2
        // Filtering out those that don't share multiple vertices
        let mut ii = 0usize;
        while ii < faces1.len() {
            // Loop locals
            let face1 = *faces1.get(ii).ok_or(dangling("face list changed"))?;
            let surf1 = brep
                .face_surface(face1)
                .ok_or(dangling("face handle is unknown"))?
                .ok_or(dangling("face without surface"))?;
            let uv1 = brep
                .face_uv_domain(face1)
                .ok_or(dangling("face handle is unknown"))?;
            let v1s = brep.face_vertices(face1)?;
            let intersect_f1 = Self::surface_is_closed(k, brep, surf1, &uv1)?;

            let mut jj = 0usize;
            while jj < faces2.len() {
                // Loop locals
                let face2 = *faces2.get(jj).ok_or(dangling("face list changed"))?;
                let surf2 = brep
                    .face_surface(face2)
                    .ok_or(dangling("face handle is unknown"))?
                    .ok_or(dangling("face without surface"))?;
                let uv2 = brep
                    .face_uv_domain(face2)
                    .ok_or(dangling("face handle is unknown"))?;
                let v2s = brep.face_vertices(face2)?;
                let intersect_f2 = Self::surface_is_closed(k, brep, surf2, &uv2)?;

                // Do these 2 faces share 2 or more common vertices?
                let common_vs = find_common_elements(&v1s, &v2s);
                let common_vs = find_common_elements(&common_vs, input_verts);
                let share_input_vs = common_vs.len() > 1;

                // Only intersect faces that share 2 rInputVerts or have only 1 vertex
                if !intersect_f1 && !intersect_f2 && !share_input_vs {
                    jj += 1;
                    continue;
                }

                // We do not handle coincident edges here
                let face1_edges = brep.face_edges(face1)?;
                let face2_edges = brep.face_edges(face2)?;
                let common_edges = find_common_elements(&face1_edges, &face2_edges);

                // In the SmMerge::MergeAddedTopology routine, by now all coincident edges should have common edges
                let mut are_coincident = false;
                if !common_edges.is_empty() {
                    // (upstream passes the angle tolerance as the 3D tolerance; the status is
                    // ignored, bAreCoincident stays FALSE)
                    let (coincident, _max_dist) = ignore_soft(surface_coincidence_check(
                        k,
                        brep,
                        surf1,
                        surf2,
                        self.this_ang_tol_rad,
                        false,
                    ))?
                    .unwrap_or((false, 0.0));
                    are_coincident = coincident;
                    if are_coincident {
                        jj += 1;
                        continue;
                    }
                }

                // If we arrive here, we are ready to intersect the faces
                let ang_tol_deg = 180.0 / PI * self.this_ang_tol_rad;
                // (the status is ignored: on failure the output arrays stay empty)
                let mut fi = ignore_soft(brep.face_intersect(
                    k,
                    face1,
                    face2,
                    self.this_approx_tol_3d,
                    ang_tol_deg,
                    FaceIntersectWants::default(),
                    Some(are_coincident),
                ))?
                .unwrap_or_else(|| FaceIntersection {
                    curves_3d: Vec::new(),
                    cc1: Vec::new(),
                    cc2: Vec::new(),
                    cc_curves: [Vec::new(), Vec::new(), Vec::new()],
                    this_uv_curves: Vec::new(),
                    other_uv_curves: Vec::new(),
                    points_3d: Vec::new(),
                });
                let _ = (&fi.curves_3d, &fi.cc_curves); // deleted by SmObjsDelete

                // In this loop we imprint each intersection as an edge (upstream indexes
                // sjjCC by the size of siiCC: a shorter sjjCC is read out of bounds)
                if fi.cc2.len() < fi.cc1.len() {
                    return Err(dangling(
                        "second classification list shorter than the first",
                    ));
                }
                let num_cc = fi.cc1.len();
                for kk in 0..num_cc {
                    {
                        let cc1 = fi
                            .cc1
                            .get_mut(kk)
                            .ok_or(dangling("classification lists differ"))?;
                        ignore_soft(cc1.set_point_class_parameters_mut(k, brep, true))?;
                        let cc2 = fi
                            .cc2
                            .get_mut(kk)
                            .ok_or(dangling("classification lists differ"))?;
                        ignore_soft(cc2.set_point_class_parameters_mut(k, brep, true))?;
                    }

                    // For each interval in the classification
                    let num_ivls = fi.cc1.get(kk).map_or(0, CurveClassification::size);
                    for ll in 0..num_ivls {
                        let (ci1, ci2) = {
                            let cc1 = fi
                                .cc1
                                .get(kk)
                                .ok_or(dangling("classification lists differ"))?;
                            let cc2 = fi
                                .cc2
                                .get(kk)
                                .ok_or(dangling("classification lists differ"))?;
                            (*cc1.at(ll)?, *cc2.at(ll)?)
                        };
                        let mut ci1 = ci1;
                        let mut ci2 = ci2;

                        // Have to imprint edges on faces
                        if ci1.mid.point_class() != PointClassType::Face
                            || ci2.mid.point_class() != PointClassType::Face
                        {
                            continue;
                        }

                        // the start of the interval
                        if ci1.start.point_class() == PointClassType::Face
                            && ci2.start.point_class() == PointClassType::Face
                        {
                            self.imprint_interval_end(
                                k,
                                brep,
                                this_brep,
                                &mut ci1.start,
                                &mut ci2.start,
                                &ci1.mid,
                                &ci2.mid,
                            )?;
                        } else if ci1.start.point_class() != PointClassType::Vertex
                            || ci2.start.point_class() != PointClassType::Vertex
                        {
                            // SE_MSG(SM_ERR, "Missing vertices should have been imprinted already")
                            Self::store_interval(&mut fi, kk, ll, &ci1, &ci2)?;
                            continue;
                        }

                        // the end of the interval
                        if ci1.end.point_class() == PointClassType::Face
                            && ci2.end.point_class() == PointClassType::Face
                        {
                            self.imprint_interval_end(
                                k,
                                brep,
                                this_brep,
                                &mut ci1.end,
                                &mut ci2.end,
                                &ci1.mid,
                                &ci2.mid,
                            )?;
                        } else if ci1.end.point_class() != PointClassType::Vertex
                            || ci2.end.point_class() != PointClassType::Vertex
                        {
                            // SE_MSG(SM_ERR, "Missing vertices should have been imprinted already")
                            Self::store_interval(&mut fi, kk, ll, &ci1, &ci2)?;
                            continue;
                        }

                        // (upstream classifies the array elements in place: write the
                        // imprinted ends back before the split updates re-classify them)
                        Self::store_interval(&mut fi, kk, ll, &ci1, &ci2)?;

                        // Make an edge in the first face
                        let new_edge1 = {
                            let cc1 = fi
                                .cc1
                                .get(kk)
                                .ok_or(dangling("classification lists differ"))?;
                            let (new_edge, new_face) =
                                self.make_edge_for_interval(k, brep, this_brep, cc1, &ci1)?;

                            // If the face was split, update classifications and add new face to intersection list
                            if let Some(new_face) = new_face {
                                Self::update_classifications_after_split(
                                    k,
                                    brep,
                                    &mut fi.cc1,
                                    kk,
                                    new_face,
                                    self.this_approx_tol_3d,
                                )?;

                                // Record new face for output and to include in future intersections
                                faces1.push(new_face); // some redundancy, will intersect all sFaces2, but only needs from jj+1
                                if faces1.len() + faces2.len() > MAX_FACE_LIST {
                                    return Err(SolidError::sm_err(OP_FACE_FACE));
                                }
                                out.new_faces.push(new_face);
                            }
                            new_edge
                        };

                        // Make an edge in the second face
                        let new_edge2 = {
                            let cc2 = fi
                                .cc2
                                .get(kk)
                                .ok_or(dangling("classification lists differ"))?;
                            let (new_edge, new_face) =
                                self.make_edge_for_interval(k, brep, this_brep, cc2, &ci2)?;

                            // If the face was split, update classifications and add new face to intersection list
                            if let Some(new_face) = new_face {
                                Self::update_classifications_after_split(
                                    k,
                                    brep,
                                    &mut fi.cc2,
                                    kk,
                                    new_face,
                                    self.this_approx_tol_3d,
                                )?;

                                // Record new face for output and to intersect both the old and the new face
                                out.new_faces.push(new_face);
                                if faces1.len() + faces2.len() + 1 > MAX_FACE_LIST {
                                    return Err(SolidError::sm_err(OP_FACE_FACE));
                                }

                                // Intersect both the old and the new face
                                faces2.insert((jj + 1).min(faces2.len()), new_face);
                                jj += 1;
                            }
                            new_edge
                        };

                        // Keep track of which faces have new edges
                        add_unique(&mut out.processed_faces, face1);
                        add_unique(&mut out.processed_faces, face2);

                        // We want only one edge shared between the faces (the status is
                        // ignored; GlueEdgesGeneral returns SM_SUCCESS at once for a null
                        // edge, which MakeEdgeInFace leaves when it makes none)
                        if let (Some(e1), Some(e2)) = (new_edge1, new_edge2) {
                            // m_pBrep->GlueEdgesGeneral(pNewEdge1, SM_OT_SAME, 0.0,
                            // bDoRegionNesting, pNewEdge2) (pInsideFaceuses NULL)
                            ignore_soft(brep.brep_glue_edges_general(
                                k,
                                this_brep,
                                Some(e1),
                                OrientType::Same,
                                0.0,
                                do_region_nesting,
                                Some(e2),
                                None,
                            ))?;
                        }
                        out.new_edges.push(new_edge1);
                    } // end for each SmCurveInterval
                } // end for each SmCurveClassification
                jj += 1;
            } // end for each face in sFaces2
            ii += 1;
        } // end for each face in sFaces1

        Ok(out)
    }

    /// Writes the edited copies of an interval pair back into the classifications.
    fn store_interval(
        fi: &mut FaceIntersection<O>,
        kk: usize,
        ll: usize,
        ci1: &crate::intersect::curve_class::p1::CurveInterval<O>,
        ci2: &crate::intersect::curve_class::p1::CurveInterval<O>,
    ) -> Result<()> {
        let cc1 = fi
            .cc1
            .get_mut(kk)
            .ok_or(dangling("classification lists differ"))?;
        *cc1.at_mut(ll)? = *ci1;
        let cc2 = fi
            .cc2
            .get_mut(kk)
            .ok_or(dangling("classification lists differ"))?;
        *cc2.at_mut(ll)? = *ci2;
        Ok(())
    }

    /// `pSurf->IsClosed(sUV, SM_SP_U) || pSurf->IsClosed(sUV, SM_SP_V)`.
    fn surface_is_closed(
        k: &Kernel,
        brep: &impl IntersectorBrep<O>,
        surf: O,
        uv: &Extent2d,
    ) -> Result<bool> {
        let s = brep
            .surface(surf)
            .ok_or(dangling("surface handle cannot be resolved"))?;
        let closed_u = s.is_closed(k, uv, SurfParamType::U, None, false)?.0;
        if closed_u {
            return Ok(true);
        }
        Ok(s.is_closed(k, uv, SurfParamType::V, None, false)?.0)
    }

    /// The imprint of an interval end that classifies to both faces (the `m_vStart`
    /// and `m_vEnd` blocks of `FaceFaceIntersect`): the classifications are refreshed
    /// (against the mid faces when they come out unknown), and when the point is still
    /// on the first face a vertex loop is made in each face and the two vertices glued.
    #[allow(clippy::too_many_arguments)]
    fn imprint_interval_end(
        &self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
        this_brep: O,
        pc1: &mut PointClassification<O>,
        pc2: &mut PointClassification<O>,
        mid1: &PointClassification<O>,
        mid2: &PointClassification<O>,
    ) -> Result<()> {
        let tol = self.this_approx_tol_3d;
        // Locals
        let mut f1 = pc1
            .face_object()
            .ok_or(dangling("classification without face"))?;
        let mut f2 = pc2
            .face_object()
            .ok_or(dangling("classification without face"))?;
        // (FindObjPoint3d's status is ignored: the point stays UNDEF on failure)
        let mut pt = ignore_soft(pc1.find_obj_point_3d(k, brep))?.unwrap_or(Point3::UNDEF);

        // This classification may be outdated. Check and update. Will either be face or vertex
        let mut uv1 = pc1.uv_param();
        let mut uv2 = pc2.uv_param();
        // (every status below is ignored upstream)
        ignore_soft(brep.face_point_3d_classify(k, f1, pt, tol, false, pc1, Some(uv1)))?;
        ignore_soft(brep.face_point_3d_classify(k, f2, pt, tol, false, pc2, Some(uv2)))?;

        // The CC may be unknow when a new face was created prior. m_vMid was updated, so check that face
        if pc1.point_class() == PointClassType::Unknown {
            f1 = mid1
                .face_object()
                .ok_or(dangling("classification without face"))?;
            ignore_soft(brep.face_point_3d_classify(k, f1, pt, tol, false, pc1, Some(uv1)))?;
        }
        if pc2.point_class() == PointClassType::Unknown {
            f2 = mid2
                .face_object()
                .ok_or(dangling("classification without face"))?;
            ignore_soft(brep.face_point_3d_classify(k, f2, pt, tol, false, pc2, Some(uv2)))?;
        }

        // We have intersecting 3dcurves. Imprint the point as a vertex on each face.
        // Glue the vertices, then keep going
        if pc1.point_class() == PointClassType::Face {
            let v1 = ignore_soft(brep.make_vertex_loop(this_brep, f1, pt))?.map(|(_loop1, v)| v);

            pt = ignore_soft(pc2.find_obj_point_3d(k, brep))?.unwrap_or(Point3::UNDEF);
            let v2 = ignore_soft(brep.make_vertex_loop(this_brep, f2, pt))?.map(|(_loop2, v)| v);

            // (two null vertices are equal: GlueVertices returns SM_SUCCESS at once; one
            // null vertex is dereferenced by GlueVertices)
            match (v1, v2) {
                (Some(v1), Some(v2)) => {
                    ignore_soft(brep.glue_vertices(k, this_brep, v1, v2, None))?;
                }
                (None, None) => {}
                _ => return Err(dangling("GlueVertices of one null vertex")),
            }

            // Update classifications
            uv1 = pc1.uv_param();
            uv2 = pc2.uv_param();
            ignore_soft(brep.face_point_3d_classify(k, f1, pt, tol, false, pc1, Some(uv1)))?;
            ignore_soft(brep.face_point_3d_classify(k, f2, pt, tol, false, pc2, Some(uv2)))?;
        }
        Ok(())
    }

    /// The "Make an edge in the first/second face" block of `FaceFaceIntersect`: a copy
    /// of the classification's curve and its current UV curve become a new edge
    /// between the interval's vertices in the mid face. Returns the new edge and
    /// the face split off, if any.
    fn make_edge_for_interval(
        &self,
        k: &Kernel,
        brep: &mut impl IntersectorBrep<O>,
        this_brep: O,
        cc: &CurveClassification<O>,
        ci: &crate::intersect::curve_class::p1::CurveInterval<O>,
    ) -> Result<(Option<O>, Option<O>)> {
        let v1 = ci.start.vertex_object();
        let v2 = ci.end.vertex_object();
        let (copy, uv_copy) = copy_intersection_edge_curves(k, brep, cc)?;
        let copy_c = brep
            .insert_curve(copy)
            .ok_or(dangling("curve store is full"))?;
        let uv_copy_c = uv_copy
            .map(|curve| {
                brep.insert_curve(curve)
                    .ok_or(dangling("curve store is full"))
            })
            .transpose()?;
        let ivl = ci.interval;

        let old_face = ci.mid.face_object();

        // (the face and vertices are passed as they are, null or not: MakeEdgeInFace
        // checks them itself. The status is ignored: a failed MakeEdgeInFace leaves the
        // edge and face null, and the curve copy stays in the store as upstream leaks it)
        let made = ignore_soft(brep_make_edge_in_face(
            k,
            brep,
            this_brep,
            old_face,
            v1,
            v2,
            copy_c,
            uv_copy_c,
            ivl,
            OrientType::Same,
            self.this_approx_tol_3d,
        ))?;
        Ok(match made {
            Some((new_edge, _new_loop, new_face)) => (new_edge, new_face),
            None => (None, None),
        })
    }

    /// The "Classification update" loops of `FaceFaceIntersect`: after a face split,
    /// every mid classification of the classifications from `from` on is refreshed
    /// against its face, and against the new face when it no longer classifies to a
    /// face.
    fn update_classifications_after_split(
        k: &Kernel,
        brep: &mut impl IntersectorBrep<O>,
        ccs: &mut [CurveClassification<O>],
        from: usize,
        new_face: O,
        approx_tol_3d: f64,
    ) -> Result<()> {
        for cc in ccs.iter_mut().skip(from) {
            let this_curve = cc.curve();
            for nn in 0..cc.size() {
                let ci = cc.at_mut(nn)?;
                let this_face = ci.mid.face_object();
                let this_curve = this_curve.ok_or(dangling("classification without curve"))?;
                let pt = eval_curve_point(k, brep, this_curve, ci.interval.mid())?;

                if let Some(this_face) = this_face {
                    let uv = ci.mid.uv_param();
                    ignore_soft(brep.face_point_3d_classify(
                        k,
                        this_face,
                        pt,
                        approx_tol_3d,
                        false,
                        &mut ci.mid,
                        Some(uv),
                    ))?;
                }
                if ci.mid.point_class() != PointClassType::Face && this_face.is_some() {
                    let uv = ci.mid.uv_param();
                    ignore_soft(brep.face_point_3d_classify(
                        k,
                        new_face,
                        pt,
                        approx_tol_3d,
                        false,
                        &mut ci.mid,
                        Some(uv),
                    ))?;
                }
            }
        }
        Ok(())
    }

    /// `FaceFaceCoincidence(rInputEdges, rFacesToDelete, ..)`: for each input edge,
    /// every pair of its faces that share the same set of edges is checked for
    /// coincidence (the same surface, or the centre of the first face's trimmed domain
    /// dropping onto the second within the approximation tolerance); of a coincident
    /// pair the face in `faces_to_delete` (else the first) is deleted and the survivor
    /// listed. `do_region_nesting` is `NotUsed`.
    ///
    /// # Errors
    ///
    /// Unless `ignore_problems`, `SM_ERR` for a face to delete without B-rep;
    /// [`SolidError::Unported`] for `SmFace::GetEdges` / `GetBrep`
    /// (`topology.face_p4`), `SmFace::CalculateUVDomainFromUVTrimCurves`'s
    /// `CreateUVTrimCurves`; those of
    /// [`IntersectorBrep::brep_delete_face`]; [`SolidError::InvalidInput`] for null handles upstream
    /// dereferences; those of the surface evaluations.
    #[allow(clippy::too_many_lines)]
    pub fn face_face_coincidence(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
        input_edges: &[Option<O>],
        faces_to_delete: &[O],
        ignore_problems: bool,
        do_region_nesting: bool,
    ) -> Result<FaceFaceCoincidence<O>> {
        let _ = do_region_nesting;
        // Init outputs
        let mut out = FaceFaceCoincidence {
            processed_faces: Vec::new(),
            deleted_faces: Vec::new(),
            max_gap: 0.0,
            min_unstitched: BIG_DOUBLE,
        };

        // Locals
        let mut gap = 0.0;

        for &edge in input_edges {
            let Some(edge) = edge else {
                continue;
            };
            let mut faces_of_edge: Vec<Option<O>> =
                brep.edge_faces(edge)?.into_iter().map(Some).collect();
            let num_faces = faces_of_edge.len();
            let mut jj = 0usize;
            while jj + 1 < num_faces {
                let Some(face1) = faces_of_edge.get(jj).copied().flatten() else {
                    jj += 1;
                    continue;
                };

                let edges1 = brep.face_edges(face1)?;
                let num_edges1 = edges1.len();
                let mut kk = jj + 1;
                let mut break_outer = false;
                while kk < num_faces {
                    let Some(face2) = faces_of_edge.get(kk).copied().flatten() else {
                        kk += 1;
                        continue;
                    };

                    let edges2 = brep.face_edges(face2)?;
                    let num_edges2 = edges2.len();

                    // See whether they're the same sets of edges.
                    if num_edges1 != num_edges2 {
                        kk += 1;
                        continue;
                    }

                    let common_edges = find_common_elements(&edges1, &edges2);
                    if common_edges.len() != num_edges1 {
                        kk += 1;
                        continue;
                    }

                    // Have two faces sharing the same set of edges.
                    // Check for coincidence.
                    let mut coin_faces = false;
                    let surf1 = brep
                        .face_surface(face1)
                        .ok_or(dangling("face handle is unknown"))?;
                    let surf2 = brep
                        .face_surface(face2)
                        .ok_or(dangling("face handle is unknown"))?;
                    if surf1 == surf2 {
                        coin_faces = true;
                        out.max_gap = 0.0;
                    } else {
                        // Check an interior point.
                        let mut domain1 = brep
                            .face_uv_domain(face1)
                            .ok_or(dangling("face handle is unknown"))?;
                        let mut domain2 = brep
                            .face_uv_domain(face2)
                            .ok_or(dangling("face handle is unknown"))?;

                        // Note, that's not good enough, Face domains may not be at all tight, so:
                        if let Some(d) = face_uv_domain_from_uv_trim_curves(k, brep, face1)? {
                            domain1 = d;
                        }
                        if let Some(d) = face_uv_domain_from_uv_trim_curves(k, brep, face2)? {
                            domain2 = d;
                        }

                        let guess_uv1 = domain1.evaluate(0.5, 0.5);
                        let guess_uv2 = domain2.evaluate(0.5, 0.5);

                        let s1 = surf1.ok_or(dangling("face without surface"))?;
                        let s2 = surf2.ok_or(dangling("face without surface"))?;
                        // (the statuses are ignored: a failed drop has bOk == FALSE and
                        // dGap == SM_BIG_DOUBLE)
                        let pt_3d1 = ignore_soft(
                            brep.surface(s1)
                                .ok_or(dangling("surface handle cannot be resolved"))?
                                .evaluate_point(k, guess_uv1),
                        )?
                        .unwrap_or(Point3::UNDEF);
                        let dropped: Option<DropPoint<O>> = ignore_soft(
                            brep.surface(s2)
                                .ok_or(dangling("surface handle cannot be resolved"))?
                                .drop_point(
                                    k,
                                    pt_3d1,
                                    &domain2,
                                    &DropPointOptions {
                                        guess_uv: Some(guess_uv2),
                                        ..DropPointOptions::default()
                                    },
                                ),
                        )?;
                        let (ok, dropped_gap) = match dropped {
                            Some(d) => (d.success, d.gap),
                            None => (false, BIG_DOUBLE),
                        };
                        gap = dropped_gap;
                        if ok && gap <= self.this_approx_tol_3d {
                            coin_faces = true;

                            out.max_gap = max2(out.max_gap, gap);
                        } else {
                            out.min_unstitched = min2(out.min_unstitched, gap);
                        }
                    }

                    if coin_faces {
                        // Delete one face, and add pointers to lists.
                        let mut del_face = face1;
                        let mut sav_face = face2;
                        if contains(faces_to_delete, &face2) {
                            del_face = face2;
                            sav_face = face1;
                        }

                        let Some(f_brep) = brep.face_brep(del_face)? else {
                            if ignore_problems {
                                kk += 1;
                                continue;
                            }
                            return Err(SolidError::sm_err(OP_FFC));
                        };
                        let old_tol = brep
                            .topology_tolerance(del_face)
                            .ok_or(dangling("face handle is unknown"))?;
                        ignore_soft(brep.brep_delete_face(k, f_brep, del_face))?;

                        // Add to the output lists.
                        // In the case of three or more faces all coincident (or
                        // certain error conditions), pDelFace could be contained in
                        // rProcessedFaces.  In that case, set that entry to pSavFace.
                        if let Some(pos) = out.processed_faces.iter().position(|f| *f == del_face) {
                            if let Some(slot) = out.processed_faces.get_mut(pos) {
                                *slot = sav_face;
                            }
                        }

                        out.processed_faces.push(sav_face);
                        out.deleted_faces.push(del_face);

                        ignore_soft(
                            brep.update_object_tolerance(k, sav_face, gap, old_tol, false),
                        )?;

                        if del_face == face1 {
                            if let Some(slot) = faces_of_edge.get_mut(jj) {
                                *slot = None;
                            }
                            // In this case, we're done with FacesOfEdge[jj].
                            break_outer = true;
                            break;
                        }
                        if let Some(slot) = faces_of_edge.get_mut(kk) {
                            *slot = None;
                        }
                    }
                    kk += 1;
                } // end of inner loop on faces of edge (kk)
                let _ = break_outer;
                jj += 1;
            } // end of outer loop on faces of edge (jj)
        } // end loop on all edges (ii)

        Ok(out)
    }
}

// ---- the file-static helpers ------------------------------------------------------------------

/// `sm_CanImprint`: whether a point classification type can be imprinted (region,
/// face, edge or vertex).
#[must_use]
pub fn can_imprint(t: PointClassType) -> bool {
    matches!(
        t,
        PointClassType::Region
            | PointClassType::Face
            | PointClassType::Edge
            | PointClassType::Vertex
    )
}

/// `sm_FindFirstLowestDimensionSol`: the index of the first solution whose first
/// object is of the lowest dimension present (a vertex, else an edge, else a face);
/// `solutions.len()` when there is no usable solution (none, or none of those kinds
/// among several).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a solution without a first object (upstream
/// dereferences null).
pub fn find_first_lowest_dimension_sol<O: SolutionObject>(
    solutions: &SolutionArray<O>,
) -> Result<usize> {
    let mut which_sol = 0usize; // Index of solution to use.

    let num_sols = solutions.len();
    if num_sols < 1 {
        return Ok(num_sols); // Indicates no usable results.
    }

    if num_sols > 1 {
        which_sol = num_sols; // Would indicate nothing found.

        // Choose the lowest-dimension result: Vertex, then Edge, then Face.
        let mut which_v = num_sols;
        let mut which_e = num_sols;
        let mut which_f = num_sols;

        // Use the first one found (of each type).
        for (ii, sol) in solutions.as_slice().iter().enumerate() {
            let kind = sol
                .objects
                .first()
                .copied()
                .flatten()
                .ok_or(SolidError::InvalidInput {
                    operation: OP_LOWEST,
                    reason: "solution without objects",
                })?
                .kind();

            if kind == ObjectKind::Vertex {
                which_v = ii;
                break; // Done: use first.
            } else if kind == ObjectKind::Edge {
                if which_e == num_sols {
                    which_e = ii;
                }
            } else if kind == ObjectKind::Face && which_f == num_sols {
                which_f = ii;
            }
        } // end iter every solution

        // lWhichSol defaults to lNumSols if there are none of these,
        // that should not happen (SM_ASSERT; SE(SM_ERR) only reports).
        if which_v < num_sols {
            which_sol = which_v;
        } else if which_e < num_sols {
            which_sol = which_e;
        } else if which_f < num_sols {
            which_sol = which_f;
        }
    } // end more than 1 solution check

    Ok(which_sol)
}

// ---- face caches of the topology solves (smsurf_GetSurfaceCache through GetFace) --------

/// The handle of each entity of one B-rep (see [`IntersectorBrep::face_entity_handles`]).
pub type EntityHandles<O> = Box<dyn Fn(EntityRef) -> Result<O>>;

/// A face lent mutably for its surface's trimmed cache
/// ([`IntersectorBrep::face_cache_target`]).
pub struct FaceCacheTarget<'a, O> {
    /// The B-rep holding the face.
    pub brep: &'a mut Brep,
    /// The face.
    pub face: FaceId,
    /// The handle of the face's surface.
    pub surface: O,
    /// The handle of each entity of `brep`.
    pub handles: EntityHandles<O>,
    /// The entity each handle of `brep` names (the inverse of `handles`).
    pub entities: Box<dyn Fn(O) -> Result<EntityRef>>,
}

impl<O> core::fmt::Debug for FaceCacheTarget<'_, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FaceCacheTarget")
            .field("face", &self.face)
            .finish_non_exhaustive()
    }
}

const OP_FACE_CACHES: &str = "boolean::topology_intersector::p1::solve_with_face_caches";

/// The face surface caches a topology solve has asked for, by face handle: each
/// built once on the face's B-rep by [`solve_with_face_caches`] (or the failure of
/// that build), and the face the solve is waiting for (with whether it asked for the
/// tree, a solve's `GetTree`, or only the cache's state bits).
struct FaceCaches<O> {
    pending: Cell<Option<(O, bool)>>,
    built: BTreeMap<O, Result<SurfaceCache<O>>>,
    /// The caches as `smsurf_GetSurfaceCache` hands them to `GlobalPointSolve`'s
    /// fast-drop face test before any `GetTree` ([`prepare_face_surface_cache_state`]).
    states: BTreeMap<O, Result<SurfaceCache<O>>>,
    /// The faces whose cache the attempt has asked for with its tree so far: from then
    /// on upstream's cache, kept with the surface, has its tree.
    tree_requested: RefCell<BTreeSet<O>>,
    /// UV trim curves the face classifications of the solve asked for
    /// (`SmEdgeuse::GetOrCreateUVTrimCurve`), made on the B-rep between attempts.
    trims: crate::query::brep::RayPreparation,
    /// The faces the classifications of the attempt resolved to B-rep entities, from
    /// which the face owning a trim request's edge use is found.
    classified_faces: RefCell<Vec<O>>,
}

/// The intersector's topology objects with the face caches of [`FaceCaches`]: the
/// owner's objects, whose surface solves get each face's cache through the face.
struct FaceCacheObjects<'a, O, G: ?Sized> {
    geometry: &'a G,
    caches: &'a FaceCaches<O>,
}

impl<O: ClassObject + SolutionObject + Ord, G: IntersectorBrep<O> + ?Sized>
    FaceCacheObjects<'_, O, G>
{
    fn inner(&self) -> &dyn TopologyObjects<O> {
        self.geometry.topology_objects()
    }
}

/// The surface solve context of [`FaceCacheObjects`].
struct FaceCacheContext<'b, 'a, O, G: ?Sized>(&'b FaceCacheObjects<'a, O, G>);

impl<O, G: ?Sized> core::fmt::Debug for FaceCacheContext<'_, '_, O, G> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FaceCacheContext").finish_non_exhaustive()
    }
}

impl<O: ClassObject + SolutionObject + Ord, G: IntersectorBrep<O> + ?Sized> SurfaceSolveContext<O>
    for FaceCacheContext<'_, '_, O, G>
{
    fn build_surface_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<O>,
    ) -> Result<SurfaceCache<O>> {
        // a surface outside the B-reps, or one no face owns: its own cache, as a solve
        // without context builds it
        let Some(handle) = handle else {
            return surface_cache(k, surface, None);
        };
        if surface.face()?.is_none() {
            return surface_cache(k, surface, Some(handle));
        }
        let face = self
            .0
            .inner()
            .surface_face(handle)?
            .ok_or(SolidError::null_pointer(OP_FACE_CACHES))?;
        match self.0.caches.built.get(&face) {
            Some(Ok(cache)) => {
                self.0.caches.tree_requested.borrow_mut().insert(face);
                cache.map_handles(Ok)
            }
            Some(Err(e)) => Err(e.clone()),
            None => {
                self.0.caches.pending.set(Some((face, true)));
                Err(SolidError::PreparationRequired {
                    operation: OP_FACE_CACHES,
                })
            }
        }
    }

    fn surface_cache_state(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<O>,
    ) -> Result<SurfaceCache<O>> {
        let Some(handle) = handle else {
            return self.build_surface_cache(k, surface, None);
        };
        if surface.face()?.is_none() {
            return self.build_surface_cache(k, surface, Some(handle));
        }
        let face = self
            .0
            .inner()
            .surface_face(handle)?
            .ok_or(SolidError::null_pointer(OP_FACE_CACHES))?;
        // after the attempt's first GetTree of the face's cache, upstream's cache has
        // its tree
        if self.0.caches.tree_requested.borrow().contains(&face) {
            return self.build_surface_cache(k, surface, Some(handle));
        }
        match self.0.caches.states.get(&face) {
            Some(Ok(cache)) => cache.map_handles(Ok),
            Some(Err(e)) => Err(e.clone()),
            None => {
                self.0.caches.pending.set(Some((face, false)));
                Err(SolidError::PreparationRequired {
                    operation: OP_FACE_CACHES,
                })
            }
        }
    }

    fn face_queries(&self) -> Option<&dyn FaceQueries<O>> {
        Some(self)
    }
}

impl<O: ClassObject + SolutionObject + Ord, G: IntersectorBrep<O> + ?Sized>
    FaceCacheContext<'_, '_, O, G>
{
    /// `f` on the read-only queries of `face` (its B-rep holds the UV trim curves its
    /// cache build created).
    fn with_queries<R>(
        &self,
        face: O,
        f: impl FnOnce(&PreparedFaceQueries<'_, O>) -> Result<R>,
    ) -> Result<R> {
        let Some((b, id)) = self.0.inner().face_entity(face) else {
            // DISPATCH GAP: the owner's objects cannot resolve the face handle to its
            // `Brep` and `FaceId`
            return Err(SolidError::unported(
                "SmFace::PointClassify",
                "query.face_p2",
            ));
        };
        let handles = self.0.geometry.face_entity_handles(face)?;
        f(&PreparedFaceQueries::new(b, id, &*handles)?)
    }

    /// `pFace->PointClassify(uv, tol, TRUE, FALSE, ..)` of `GlobalPointSolve`'s
    /// fast-drop face test, as [`PreparedFaceQueries`] classifies it, except that a UV
    /// trim curve the classification needs and the face lacks is requested
    /// (`SmEdgeuse::GetOrCreateUVTrimCurve`, which upstream's classification makes on
    /// the face): the solve stops, [`solve_with_face_caches`] makes the trim on the
    /// face's B-rep and runs the solve again. (The face's cache made during a Boolean
    /// operation has no tree, so its build has not made the trims.)
    fn classify_fast_drop_requesting_trims(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<O>)> {
        // (through the attempt's objects: the face is recorded for the trim request)
        let Some((b, id)) = self.0.face_entity(face) else {
            // DISPATCH GAP: the owner's objects cannot resolve the face handle to its
            // `Brep` and `FaceId`
            return Err(SolidError::unported(
                "SmFace::PointClassify",
                "query.face_p2",
            ));
        };
        let handles = self.0.geometry.face_entity_handles(face)?;
        let mut pc = PointClassification::new(src_tol);
        crate::query::face::p2::point_classify_requested(
            k,
            b,
            id,
            uv,
            &mut pc,
            crate::query::face::p2::PointClassifyOptions {
                src_zone_tol_3d: src_tol,
                do_boundary_intersections: true,
                use_uv_classification: false,
                ..Default::default()
            },
            &self.0.caches.trims,
        )?;
        Ok((
            pc.point_class().code(),
            pc.object().map(&*handles).transpose()?,
        ))
    }
}

impl<O: ClassObject + SolutionObject + Ord, G: IntersectorBrep<O> + ?Sized> FaceQueries<O>
    for FaceCacheContext<'_, '_, O, G>
{
    fn face_zone_tol_3d(&self, face: O) -> Result<f64> {
        self.with_queries(face, |q| q.face_zone_tol_3d(face))
    }

    fn face_point_classify(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<O>)> {
        self.with_queries(face, |q| q.face_point_classify(k, face, uv, src_tol))
    }

    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<O>)> {
        self.classify_fast_drop_requesting_trims(k, face, uv, src_tol)
    }

    fn curve_edge_tolerance(&self, curve: O) -> Option<Result<f64>> {
        // `SmEdge::GetTolerance` of the curve's edge (`SmCurve::GetEdge`)
        let inner = self.0.inner();
        Some(
            inner
                .curve_edge(curve)
                .and_then(|edge| edge.ok_or(SolidError::null_pointer(OP_FACE_CACHES)))
                .and_then(|edge| inner.edge_tolerance(edge)),
        )
    }
}

impl<O: ClassObject + SolutionObject + Ord, G: IntersectorBrep<O> + ?Sized> TopologyObjects<O>
    for FaceCacheObjects<'_, O, G>
{
    fn surface_solve_context(&self) -> Option<Box<dyn SurfaceSolveContext<O> + '_>>
    where
        O: Copy + PartialEq,
    {
        self.inner()
            .surface_solve_context()
            .or_else(|| Some(Box::new(FaceCacheContext(self))))
    }
    fn classify_face_point(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        tolerance: f64,
        boundaries: bool,
        use_uv: bool,
    ) -> Option<Result<i32>> {
        self.inner()
            .classify_face_point(k, face, uv, tolerance, boundaries, use_uv)
    }
    fn classifies_on_face(
        &self,
        k: &Kernel,
        face: O,
        point: Point3,
        tolerance: f64,
    ) -> Option<Result<bool>> {
        self.inner().classifies_on_face(k, face, point, tolerance)
    }
    fn brep_cache(&self, brep: O) -> Result<&crate::topology::brep_cache::BrepCache<O>> {
        self.inner().brep_cache(brep)
    }
    fn brep_trees(
        &self,
        brep: O,
    ) -> Result<crate::intersect::topology_solver::p2::TopologyTrees<'_, O>> {
        self.inner().brep_trees(brep)
    }
    fn shape_member_object(&self, member: ShapeMember) -> Result<O> {
        self.inner().shape_member_object(member)
    }
    fn shape_geometry_object(&self, member: ShapeMember) -> Result<O> {
        self.inner().shape_geometry_object(member)
    }
    fn curve(&self, object: O) -> Result<&Curve> {
        self.inner().curve(object)
    }
    fn analytic_curve_handle(&self, curve: O) -> Option<O> {
        self.inner().analytic_curve_handle(curve)
    }
    fn vertex_point(&self, vertex: O) -> Result<Point3> {
        self.inner().vertex_point(vertex)
    }
    fn vertex_tolerance(&self, vertex: O) -> Result<f64> {
        self.inner().vertex_tolerance(vertex)
    }
    fn curve_edge(&self, curve: O) -> Result<Option<O>> {
        self.inner().curve_edge(curve)
    }
    fn edge_curve(&self, edge: O) -> Result<Option<O>> {
        self.inner().edge_curve(edge)
    }
    fn edge_interval(&self, edge: O) -> Result<Extent1d> {
        self.inner().edge_interval(edge)
    }
    fn edge_tolerance(&self, edge: O) -> Result<f64> {
        self.inner().edge_tolerance(edge)
    }
    fn surface_face(&self, surface: O) -> Result<Option<O>> {
        self.inner().surface_face(surface)
    }
    fn face_surface(&self, face: O) -> Result<O> {
        self.inner().face_surface(face)
    }
    fn surface(&self, surface: O) -> Result<&Surface> {
        self.inner().surface(surface)
    }
    fn face_uv_domain(&self, face: O) -> Result<Extent2d> {
        self.inner().face_uv_domain(face)
    }
    fn face_tolerance(&self, face: O) -> Result<f64> {
        self.inner().face_tolerance(face)
    }
    fn face_entity(&self, face: O) -> Option<(&Brep, FaceId)> {
        let entity = self.inner().face_entity(face);
        if entity.is_some() {
            let mut faces = self.caches.classified_faces.borrow_mut();
            if !faces.contains(&face) {
                faces.push(face);
            }
        }
        entity
    }
    fn uv_trim_preparation(&self) -> Option<&crate::query::brep::RayPreparation> {
        self.inner()
            .uv_trim_preparation()
            .or(Some(&self.caches.trims))
    }
    fn surface_owner(&self, surface: O) -> Result<Option<O>> {
        self.inner().surface_owner(surface)
    }
    fn shape_member(&self, object: O) -> ShapeMember {
        self.inner().shape_member(object)
    }
    fn object_zone_tol_3d(&self, k: &Kernel, object: O) -> Result<f64> {
        self.inner().object_zone_tol_3d(k, object)
    }
}

/// Runs the topology solve `solve` on the owner's objects, giving the surface solves
/// of face-owned surfaces their face's cache (`smsurf_GetSurfaceCache`, which builds
/// a face surface's trimmed cache through `GetFace()` and may create the face's UV
/// trim curves). The solve reads the B-reps shared, so a cache it asks for is built
/// on the owner's B-rep between attempts and the solve runs again from its start; a
/// solve has no other effect, so each attempt repeats the last one up to the request.
/// Each face's cache is built once per call (the failure of a build is what the
/// solve's later requests for it return, as upstream's solve sees the failing
/// `smsurf_GetSurfaceCache` there).
///
/// # Errors
///
/// Those of `solve`; [`SolidError::InvalidInput`] when a solve asks again for a face
/// whose cache it was given.
pub(crate) fn solve_with_face_caches<O, G, R>(
    k: &Kernel,
    geometry: &mut G,
    mut solve: impl FnMut(&dyn TopologyObjects<O>) -> Result<R>,
) -> Result<R>
where
    O: ClassObject + SolutionObject + Ord,
    G: IntersectorBrep<O> + ?Sized,
{
    with_face_caches(k, geometry, |objects| solve(objects))
}

/// [`solve_with_face_caches`] for a surface solve outside the topology solver: `solve`
/// gets the face-cache context its surface passes to the solve (the surface reaches
/// its face's cache, `smsurf_GetSurfaceCache`).
///
/// # Errors
///
/// As [`solve_with_face_caches`].
pub(crate) fn surface_solve_with_face_caches<O, G, R>(
    k: &Kernel,
    geometry: &mut G,
    mut solve: impl FnMut(&mut dyn SurfaceSolveContext<O>) -> Result<R>,
) -> Result<R>
where
    O: ClassObject + SolutionObject + Ord,
    G: IntersectorBrep<O> + ?Sized,
{
    with_face_caches(k, geometry, |objects| solve(&mut FaceCacheContext(objects)))
}

/// The face owning the edge use of a UV trim request: among the faces the attempt's
/// classifications resolved on the requesting B-rep, the one whose loop use holds the
/// edge use (`SmEdgeuse::GetLoopuse`, `GetFaceuse`, `GetFace`).
fn trim_request_face<O, G>(
    geometry: &G,
    caches: &FaceCaches<O>,
    request: crate::query::brep::PreparationRequest,
) -> Result<O>
where
    O: ClassObject + SolutionObject + Ord,
    G: IntersectorBrep<O> + ?Sized,
{
    let crate::query::brep::PreparationRequest::UvTrim(eu) = request else {
        return Err(SolidError::invalid_input(
            OP_FACE_CACHES,
            "unexpected preparation request",
        ));
    };
    let faces = caches.classified_faces.borrow();
    let candidates: Vec<(O, &Brep, FaceId)> = faces
        .iter()
        .filter_map(|&face| {
            let (b, f) = geometry.topology_objects().face_entity(face)?;
            Some((face, b, f))
        })
        .collect();
    trim_owner_face(&candidates, |b| caches.trims.trim_requested_on(b), eu)
        .ok_or(SolidError::null_pointer(OP_FACE_CACHES))
}

/// The candidate face whose B-rep `requested_on` accepts and whose loop uses hold `eu`
/// (`SmEdgeuse::GetLoopuse`, `SmLoopuse::GetFaceuse`, `GetFace`).
fn trim_owner_face<O: Copy>(
    candidates: &[(O, &Brep, FaceId)],
    requested_on: impl Fn(&Brep) -> bool,
    eu: crate::topology::EdgeuseId,
) -> Option<O> {
    for &(face, b, f) in candidates {
        if !requested_on(b) {
            continue;
        }
        let owner = match crate::topology::edgeuse::p1::loopuse(b, eu) {
            Ok(Some(lu)) => crate::topology::loopuse::faceuse(b, lu).and_then(|fu| {
                fu.map_or(Ok(None), |fu| {
                    crate::topology::edgeuse::p1::faceuse_face(b, fu)
                })
            }),
            Ok(None) | Err(_) => Ok(None),
        };
        if owner.ok().flatten() == Some(f) {
            return Some(face);
        }
    }
    None
}

/// The attempts of [`solve_with_face_caches`] and [`surface_solve_with_face_caches`].
fn with_face_caches<O, G, R>(
    k: &Kernel,
    geometry: &mut G,
    mut solve: impl FnMut(&FaceCacheObjects<'_, O, G>) -> Result<R>,
) -> Result<R>
where
    O: ClassObject + SolutionObject + Ord,
    G: IntersectorBrep<O> + ?Sized,
{
    let mut caches = FaceCaches {
        pending: Cell::new(None),
        built: BTreeMap::new(),
        states: BTreeMap::new(),
        tree_requested: RefCell::new(BTreeSet::new()),
        trims: crate::query::brep::RayPreparation::default(),
        classified_faces: RefCell::new(Vec::new()),
    };
    loop {
        caches.classified_faces.borrow_mut().clear();
        caches.tree_requested.borrow_mut().clear();
        let result = solve(&FaceCacheObjects {
            geometry: &*geometry,
            caches: &caches,
        });
        // (a request is answered even when the solve did not stop at it: upstream's
        // solve has the cache there)
        // a UV trim the classification needed is made on the face's B-rep, and the solve
        // runs again from its start (upstream creates it lazily inside the walk)
        if let (Err(SolidError::PreparationRequired { .. }), Some(request)) =
            (&result, caches.trims.take_request())
        {
            let face = trim_request_face(geometry, &caches, request)?;
            let target = geometry.face_cache_target(face)?;
            caches.trims.prepare(k, target.brep, request)?;
            continue;
        }
        let Some((face, get_tree)) = caches.pending.take() else {
            return result;
        };
        let made = if get_tree {
            &mut caches.built
        } else {
            &mut caches.states
        };
        if made.contains_key(&face) {
            return Err(SolidError::invalid_input(
                OP_FACE_CACHES,
                "face cache preparation made no progress",
            ));
        }
        let cache = match geometry.face_cache_target(face) {
            Ok(t) if get_tree => {
                prepare_face_surface_cache(k, t.brep, t.face, t.surface, &*t.handles, &*t.entities)
            }
            Ok(t) => prepare_face_surface_cache_state(
                k,
                t.brep,
                t.face,
                t.surface,
                &*t.handles,
                &*t.entities,
            ),
            Err(e) => Err(e),
        };
        made.insert(face, cache);
    }
}

/// `sm_IntersectShellVertices(pBrep1, pBrep2, bSwapOrder, pProtoTopoMgr)`: for every
/// shell vertex of `brep1`, the lowest-dimensional topology of `brep2` it intersects
/// (vertex, else edge, else face) becomes a proto vertex of the manager, from the
/// vertex's classification and the intersection's (in the manager's B-rep order, or
/// swapped). Always `SM_SUCCESS`.
///
/// # Errors
///
/// [`SolidError::Unported`] for the callees of the solver and manager; `SER` of a failing point solve or
/// classification load; [`SolidError::InvalidInput`] for null handles upstream
/// dereferences.
pub fn intersect_shell_vertices<O: ClassObject + SolutionObject + Ord>(
    k: &Kernel,
    geometry: &mut impl IntersectorBrep<O>,
    brep1: O,
    brep2: O,
    swap_order: bool,
    mgr: &mut ProtoTopologyManager<O>,
) -> Result<()> {
    let vertex_list = geometry.brep_vertices(brep1)?;

    for vertex in vertex_list {
        // Only shell vertices:
        if !geometry.vertex_is_shell_vertex(vertex)? {
            continue;
        }

        // Tol: BrepPointSolve() adds this to the Topology's tolerance.
        let vtx_zone_tol = geometry
            .topology_tolerance(vertex)
            .ok_or(dangling("vertex handle is unknown"))?;
        let point = geometry.vertex_point(vertex)?;

        // intersect pVertex with Brep (temporarily disable EditingEnabled)
        let old_edit = geometry.set_editing_enabled(brep2, false)?;
        let solved = solve_with_face_caches(k, geometry, |objects| {
            TopologySolver::brep_point_solve(
                k,
                objects,
                Some(brep2),
                point,
                TopologySolveOptions {
                    operation: SolverOperation::Intersect,
                    requested: SolutionRequested::All,
                    tolerance_3d: vtx_zone_tol,
                    best_answer_so_far_sq: 0.0,
                    opt_vectors: &[],
                },
            )
        });
        geometry.set_editing_enabled(brep2, old_edit)?;
        let solutions = solved?;

        // no work - no intersections
        let num_sols = solutions.len();
        if num_sols < 1 {
            continue;
        }

        // Find the lowest-dimensional result: Vertex first, then Edge, then Face.
        let which_sol = find_first_lowest_dimension_sol(&solutions)?;

        if which_sol >= num_sols {
            continue; // No usable sols.
        }

        // Create a PointClassification for each Brep, and a ProtoVertex from those.
        // The PointClass objects get copied into the new PV.
        let mut pc1: PointClassification<O> = PointClassification::new(vtx_zone_tol);
        pc1.set_class_object(PointClassType::Vertex, Some(vertex));

        let sol = solutions.get(which_sol).ok_or(dangling("solution index"))?;
        let mut pc2: PointClassification<O> = PointClassification::new(vtx_zone_tol);
        let object = sol
            .objects
            .first()
            .copied()
            .flatten()
            .ok_or(dangling("solution without objects"))?;
        pc2.load_from_intersection(vtx_zone_tol, object, &sol.start, 0, BIG_DOUBLE)?;

        // the status of FindOrCreateProtoVertex is not checked
        if swap_order {
            ignore_soft(mgr.find_or_create_proto_vertex_pcs(k, geometry, &mut pc2, &mut pc1))?;
        } else {
            ignore_soft(mgr.find_or_create_proto_vertex_pcs(k, geometry, &mut pc1, &mut pc2))?;
        }
    } // end for each m_Brep Vertex

    Ok(())
}

/// `sm_IntersectWireLaminaEdges(pBrep1, pBrep2, bDoLamina, bSwapOrder,
/// pProtoTopoMgr)`: for every wire (and, with `do_lamina`, lamina) edge of `brep1`,
/// its curve is intersected with `brep2`; the edge's own classification (vertex, edge,
/// vertex) is homogenised with the one holding the intersections, and each interval
/// pair becomes a proto edge of the manager (in the manager's B-rep order, or
/// swapped). Always `SM_SUCCESS`, except a failing `Homogenize` (`SM_ERR`).
///
/// # Errors
///
/// `SM_ERR` when a `Homogenize` fails; [`SolidError::Unported`] for the deferrals of
/// the `SmCurveClassification` constructor, `InsertTopologyIntersections`
/// (`intersect.curve_class_p5`), `Homogenize` (`intersect.curve_class_p6`) and the
/// callees of the solver and manager; [`SolidError::InvalidInput`] for null handles
/// upstream dereferences.
pub fn intersect_wire_lamina_edges<O: ClassObject + SolutionObject + Ord>(
    k: &Kernel,
    geometry: &mut impl IntersectorBrep<O>,
    brep1: O,
    brep2: O,
    do_lamina: bool,
    swap_order: bool,
    ti_approx_tol_3d: f64,
    mgr: &mut ProtoTopologyManager<O>,
) -> Result<()> {
    let edge_list = geometry.brep_edges(brep1)?;

    for edge in edge_list {
        let mut process_this = geometry.edge_is_wire(edge)?;
        process_this |= do_lamina && geometry.edge_is_lamina(edge)?;
        if !process_this {
            continue;
        }

        // select SolverType - handle wires differently unless bForceFaceIntersections == TRUE
        // (always TRUE here)
        let solver_op = SolverOperation::Intersect;

        let curve = geometry
            .edge_curve(edge)
            .ok_or(dangling("edge handle is unknown"))?
            .ok_or(dangling("edge without curve"))?;
        let interval = geometry
            .edge_interval(edge)
            .ok_or(dangling("edge handle is unknown"))?;
        let tolerance = geometry
            .topology_tolerance(edge)
            .ok_or(dangling("edge handle is unknown"))?;

        // temporarily disable EditingEnabled bit; Intersect Edge->Curve with Brep
        let old_edit = geometry.set_editing_enabled(brep2, false)?;
        let solved = solve_with_face_caches(k, geometry, |objects| {
            let curve_ref = CurveRef {
                curve: objects.curve(curve)?,
                handle: curve,
            };
            TopologySolver::brep_curve_solve(
                k,
                objects,
                Some(brep2),
                curve_ref,
                &interval,
                TopologySolveOptions {
                    operation: solver_op,
                    requested: SolutionRequested::All,
                    tolerance_3d: tolerance,
                    best_answer_so_far_sq: BIG_DOUBLE,
                    opt_vectors: &[],
                },
            )
        });
        geometry.set_editing_enabled(brep2, old_edit)?;

        let solutions = match solved {
            Ok(s) => s,
            Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => {
                return Err(e)
            }
            Err(_) => continue, // No ints for this Edge.
        };
        if solutions.is_empty() {
            continue; // No ints for this Edge.
        }

        // Construct first CurveClassification for all of pEdge.
        let mut crv_class1 = new_curve_classification(k, curve, &interval, None, tolerance)?;

        let vertex = geometry.edge_start_vertex(edge)?;
        {
            let ci = crv_class1.at_mut(0)?;
            ci.start.set_class_object(PointClassType::Vertex, vertex);
            ci.mid.set_class_object(PointClassType::Edge, Some(edge));
        }
        // (GetOtherVertex of a null vertex is null, with SE(SM_ERR))
        let vertex = geometry.edge_other_vertex(edge, vertex)?;
        crv_class1
            .at_mut(0)?
            .end
            .set_class_object(PointClassType::Vertex, vertex);

        // Construct second CurveClassification for pBrep2 intersections.
        let mut crv_class2 = new_curve_classification(k, curve, &interval, None, tolerance)?;

        // Add curve/Brep intersections to CrvClass2.
        if ignore_soft(cc_insert_topology_intersections(
            k,
            geometry,
            &mut crv_class2,
            &solutions,
        ))?
        .is_none()
        {
            continue;
        }

        // Syncronize the two curve classifications.
        if ignore_soft(cc_homogenize(k, geometry, &mut crv_class1, &mut crv_class2))?.is_none() {
            return Err(SolidError::sm_err(OP));
        }

        // Create ProtoEdges from each segment.
        for jj in 0..crv_class2.size() {
            let mut ci1 = *crv_class1.at(jj)?;
            let mut ci2 = *crv_class2.at(jj)?;
            let approx_tol = ti_approx_tol_3d; // m_pTI->GetThisApproxTol3d() in FindOrCreateProtoEdge
            if swap_order {
                ignore_soft(mgr.find_or_create_proto_edge(
                    k,
                    geometry,
                    approx_tol,
                    &crv_class2,
                    &mut ci2,
                    &crv_class1,
                    &mut ci1,
                ))?;
            } else {
                ignore_soft(mgr.find_or_create_proto_edge(
                    k,
                    geometry,
                    approx_tol,
                    &crv_class1,
                    &mut ci1,
                    &crv_class2,
                    &mut ci2,
                ))?;
            }
            *crv_class1.at_mut(jj)? = ci1;
            *crv_class2.at_mut(jj)? = ci2;
        }
    } // end for each pBrep1 edge

    Ok(())
}

/// `EvaluatePoint` of the curve behind `curve` at `t`; `SM_UNDEF` when the
/// evaluation fails (the status is not checked upstream).
pub(super) fn eval_curve_point<O: ClassObject>(
    k: &Kernel,
    geometry: &impl IntersectorBrep<O>,
    curve: O,
    t: f64,
) -> Result<Point3> {
    let c = geometry
        .curve(curve)
        .ok_or(dangling("curve handle cannot be resolved"))?;
    Ok(ignore_soft(c.evaluate_point(k, t))?.unwrap_or(Point3::UNDEF))
}

/// `sm_SetEdgeClass(rCurveClass, pEdge)`: sets the first interval of a classification
/// of the edge's curve to the edge and its end vertices (swapped when the primary edge
/// use is opposite), then replaces an end that lies farther from its vertex than the
/// vertex tolerance plus the classification's source zone tolerance (a trimmed
/// classification interval) by the drop of the interval end onto the edge.
///
/// # Errors
///
/// `SER` of a failing point evaluation or drop; [`SolidError::InvalidInput`] for an
/// edge without vertices, primary edge use or curve, or a classification without
/// curve or intervals (upstream dereferences null).
pub fn set_edge_class<O: ClassObject + SolutionObject + Ord>(
    k: &Kernel,
    geometry: &impl IntersectorBrep<O>,
    curve_class: &mut CurveClassification<O>,
    edge: O,
) -> Result<()> {
    // edge locals
    let mut v1 = geometry
        .edge_vertex(edge)?
        .ok_or(dangling("edge without vertex"))?;
    let mut v2 = geometry
        .edge_other_vertex(edge, Some(v1))?
        .ok_or(dangling("edge without other vertex"))?;
    let prim_eu_orientation = geometry
        .edge_primary_edgeuse_orientation(edge)?
        .ok_or(dangling("edge without primary edge use"))?;

    if prim_eu_orientation == OrientType::Opposite {
        core::mem::swap(&mut v1, &mut v2);
    }

    // CurveClassification locals
    let class_interval = curve_class.interval();
    let class_curve = curve_class
        .curve()
        .ok_or(dangling("classification without curve"))?;
    let src_zone_tol_3d = src_zone_tol_3d_of(k, Some(&*curve_class));

    // set 1st CurveClassification interval to classify to edge and its end vertices
    {
        let ci = curve_class.at_mut(0)?;
        ci.start.set_class_object(PointClassType::Vertex, Some(v1));
        ci.start.set_pre_snap_param(class_interval.min());
        ci.start.set_deviation(0.0);

        ci.mid.set_class_object(PointClassType::Edge, Some(edge));
        ci.mid.set_pre_snap_param(BIG_DOUBLE);
        ci.mid.set_deviation(0.0);

        ci.end.set_class_object(PointClassType::Vertex, Some(v2));
        ci.end.set_pre_snap_param(class_interval.max());
        ci.end.set_deviation(0.0);
    }

    // get curve endPoints
    let c = geometry
        .curve(class_curve)
        .ok_or(dangling("curve handle cannot be resolved"))?;
    let start = c.evaluate_point(k, class_interval.min())?;
    let end = c.evaluate_point(k, class_interval.max())?;

    // Note that the curve on the curve class may be trimmed - take
    // care of that here by replacing the vertex by edge classifications
    // for the ends of the interval.

    // define a tolerance
    let v1_tol = geometry
        .topology_tolerance(v1)
        .ok_or(dangling("vertex handle is unknown"))?;
    let start_tol = v1_tol + src_zone_tol_3d;
    let edge_curve_h = geometry
        .edge_curve(edge)
        .ok_or(dangling("edge handle is unknown"))?
        .ok_or(dangling("edge without curve"))?;
    let edge_curve = geometry
        .curve(edge_curve_h)
        .ok_or(dangling("curve handle cannot be resolved"))?;
    let edge_interval = geometry
        .edge_interval(edge)
        .ok_or(dangling("edge handle is unknown"))?;

    // when curveInterval start is trimmed
    let v1_point = geometry.vertex_point(v1)?;
    if start.distance(v1_point) > start_tol {
        // change CurveClassification m_vStart from Vertex to Point on Edge classification
        let dropped = edge_curve.drop_point(
            k,
            &edge_interval,
            start,
            None,
            start_tol,
            None,
            SolverOperation::Minimize,
        )?;
        let ci = curve_class.at_mut(0)?;
        ci.start.set_class_object(PointClassType::Edge, Some(edge));
        ci.start.set_pre_snap_param(dropped.parameter);
        ci.start.set_deviation(dropped.distance);
        ci.start.set_t_param(dropped.parameter, false);
    } // end is curveInterval start trimmed check

    // when curveInterval end is trimmed
    let v2_tol = geometry
        .topology_tolerance(v2)
        .ok_or(dangling("vertex handle is unknown"))?;
    let end_tol = v2_tol + src_zone_tol_3d;
    let v2_point = geometry.vertex_point(v2)?;
    if end.distance(v2_point) > end_tol {
        // change CurveClassification m_vEnd from Vertex to Point on Edge classification
        let dropped = edge_curve.drop_point(
            k,
            &edge_interval,
            end,
            None,
            end_tol,
            None,
            SolverOperation::Minimize,
        )?;
        let ci = curve_class.at_mut(0)?;
        ci.end.set_class_object(PointClassType::Edge, Some(edge));
        ci.end.set_pre_snap_param(dropped.parameter);
        ci.end.set_deviation(dropped.distance);
        ci.end.set_t_param(dropped.parameter, false);
    } // end is curveInterval end trimmed check

    let _ = OP_SET_EDGE_CLASS;
    Ok(())
}

/// `sm_CheckInteriorEdge(pEdge, dT, dTol, bInterior)`: whether the parameter `t` is in
/// the interior of the edge, to tolerance: inside its interval (excluding the ends by
/// `SM_EFF_ZERO`), then, when the tangent is long enough, inside by `20 * tol` in
/// parameter space; otherwise farther than `tol` from both vertices in 3D.
///
/// # Errors
///
/// `SER` of a failing evaluation (upstream then reports not interior through the
/// out-param; here the error); [`SolidError::InvalidInput`] for an edge without
/// curve or vertices (upstream dereferences null).
pub fn check_interior_edge<O: ClassObject>(
    k: &Kernel,
    geometry: &impl IntersectorBrep<O>,
    edge: O,
    t: f64,
    tol: f64,
) -> Result<bool> {
    // Check common simple case first.
    // Note, negative tol excludes the ends.
    let edge_domain = geometry
        .edge_interval(edge)
        .ok_or(dangling("edge handle is unknown"))?;
    if !edge_domain.contains_value(t, -EFF_ZERO) {
        return Ok(false);
    }

    let curve_h = geometry
        .edge_curve(edge)
        .ok_or(dangling("edge handle is unknown"))?
        .ok_or(dangling("edge without curve"))?;
    let curve = geometry
        .curve(curve_h)
        .ok_or(dangling("curve handle cannot be resolved"))?;
    let crv_pt = curve.evaluate(k, t, 1, false, true)?;
    let param_len = crv_pt.get(1).map_or(0.0, |d| d.length());
    if param_len > 0.1 {
        let mut param_tol = tol / param_len;

        // Because the param length can vary, use this test
        // only as a rough guide: if it's anywhere close,
        // go on to the 3d test.  So use a bigger tolerance.
        param_tol *= 20.0;
        if edge_domain.contains_value(t, -param_tol) {
            return Ok(true);
        }
    }
    // Work in 3d.
    // This is also more reliable in case of vertex/edge tolerance.
    let position = crv_pt.first().copied().ok_or(SolidError::InvalidInput {
        operation: OP_INTERIOR,
        reason: "evaluation without position",
    })?;

    let v = geometry
        .edge_vertex(edge)?
        .ok_or(dangling("edge without vertex"))?;
    let v_point = geometry.vertex_point(v)?;
    let dist = position.distance(v_point);
    if dist <= tol {
        return Ok(false);
    }

    let v = geometry
        .edge_other_vertex(edge, Some(v))?
        .ok_or(dangling("edge without other vertex"))?;
    let v_point = geometry.vertex_point(v)?;
    let dist = position.distance(v_point);
    if dist <= tol {
        return Ok(false);
    }

    Ok(true)
}

/// `sm_TestAndSplitTwoEdges(pEdge1, pEdge2, dTol, ..)`: intersects the two edges'
/// curves (at ten times the tolerance, skipping the global coincidence check); each
/// single-point intersection within the tolerance that is interior to both edges
/// splits both with a new vertex, and the two new vertices are glued at their
/// midpoint. Solutions farther than the tolerance lower the minimum unstitched gap.
/// Bounding boxes are not checked; the caller should do that first.
///
/// # Errors
///
/// `SER` of a failing curve intersection; `SM_ERR_NULL_POINTER` for an edge without
/// curve, without B-rep, or a split without new vertex; [`SolidError::Unported`] for
/// `SmBrep::MakeVertexSplitEdge` / `GlueVertices`; [`SolidError::InvalidInput`] for
/// null handles upstream dereferences.
#[allow(clippy::too_many_lines)]
pub fn test_and_split_two_edges<O: ClassObject + SolutionObject + Ord>(
    k: &mut Kernel,
    geometry: &mut impl IntersectorBrep<O>,
    edge1: O,
    edge2: O,
    tol: f64,
) -> Result<SplitTwoEdges<O>> {
    // Init outputs
    let mut out = SplitTwoEdges {
        dist_to_edge: 0.0,
        min_unstitched: BIG_DOUBLE,
        new_verts: Vec::new(),
        new_edges1: Vec::new(),
        new_edges2: Vec::new(),
    };

    let curve1 = geometry
        .edge_curve(edge1)
        .ok_or(dangling("edge handle is unknown"))?
        .ok_or(SolidError::null_pointer(OP_SPLIT_TWO))?;
    let curve2 = geometry
        .edge_curve(edge2)
        .ok_or(dangling("edge handle is unknown"))?
        .ok_or(SolidError::null_pointer(OP_SPLIT_TWO))?;

    let skip_global_coincidence_check = true; //cbi ?

    let e1_domain = geometry
        .edge_interval(edge1)
        .ok_or(dangling("edge handle is unknown"))?;
    let e2_domain = geometry
        .edge_interval(edge2)
        .ok_or(dangling("edge handle is unknown"))?;

    // Use a larger tol, to catch close ones for rdMinUnstitched.
    let solutions: SolutionArray<O> = {
        let c1 = geometry
            .curve(curve1)
            .ok_or(dangling("curve handle cannot be resolved"))?;
        let c2 = geometry
            .curve(curve2)
            .ok_or(dangling("curve handle cannot be resolved"))?;
        c1.global_curve_intersect(
            k,
            &e1_domain,
            c2,
            &e2_domain,
            10.0 * tol,
            &CurveIntersectOptions {
                skip_global_coincidence_check,
                handles: [Some(curve1), Some(curve2)],
                analytic_handles: [None, None],
            },
        )?
    };

    let num_sols = solutions.len();
    if num_sols < 1 {
        return Ok(out);
    }

    for sol in solutions.as_slice() {
        // We should be able to skip range solutions.
        if sol.solution_type != SolutionType::SingleValue {
            continue;
        }

        if sol.start.solution_value > tol {
            out.min_unstitched = min2(out.min_unstitched, sol.start.solution_value);
            continue;
        }

        // Check for internal to both edges.
        let t1 = sol.start.param(0)?;
        let interior =
            ignore_soft(check_interior_edge(k, geometry, edge1, t1, tol))?.unwrap_or(false);
        if !interior {
            continue;
        }

        let t2 = sol.start.param(1)?;
        let interior =
            ignore_soft(check_interior_edge(k, geometry, edge2, t2, tol))?.unwrap_or(false);
        if !interior {
            continue;
        }

        // The intersection is interior to both Edges.
        // Split them both, and then glue the vertices.
        let brep = geometry
            .edge_brep_handle(edge1)?
            .ok_or(SolidError::null_pointer(OP_SPLIT_TWO))?;

        let (e1, e2, new_v1) = match geometry
            .make_vertex_split_edge(k, brep, edge1, t1)
            .map(|(e1, e2, v)| (Some(e1), Some(e2), Some(v)))
        {
            Ok(r) => r,
            Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => {
                return Err(e)
            }
            Err(_) => {
                // SM_ASSERT(FALSE): Maybe because of tolerance.  Check that.
                let pt1 = eval_curve_point(k, geometry, curve1, t1)?;
                let pt2 = eval_curve_point(k, geometry, curve2, t2)?;
                if pt1.closer_than(out.min_unstitched, pt2) {
                    out.min_unstitched = pt1.distance(pt2);
                }
                continue;
            }
        };
        let new_v1 = new_v1.ok_or(SolidError::null_pointer(OP_SPLIT_TWO))?;
        let new_edge = if e2 == Some(edge1) { e1 } else { e2 };
        out.new_edges1.push(new_edge);

        let brep = geometry
            .edge_brep_handle(edge2)?
            .ok_or(dangling("edge without B-rep"))?; // Presumably the same... (dereferenced upstream)
        let (e1, e2, new_v2) = match geometry
            .make_vertex_split_edge(k, brep, edge2, t2)
            .map(|(e1, e2, v)| (Some(e1), Some(e2), Some(v)))
        {
            Ok(r) => r,
            Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => {
                return Err(e)
            }
            Err(_) => continue, // SM_ASSERT(FALSE)
        };
        // pNewV2 will be deleted, so don't add to output list.
        let new_v2 = new_v2.ok_or(SolidError::null_pointer(OP_SPLIT_TWO))?;
        let new_edge = if e2 == Some(edge2) { e1 } else { e2 };
        out.new_edges2.push(new_edge);

        let p1 = geometry.vertex_point(new_v1)?;
        let p2 = geometry.vertex_point(new_v2)?;
        out.dist_to_edge = p1.distance(p2);

        // And glue the vertices.

        // First set the position of the surviving vertex to be
        // the midpoint of the two ... this should help to keep tolerances
        // from growing excessively.
        let mid = 0.5 * (p1 + p2);
        geometry.set_vertex_point(k, new_v1, mid)?;

        ignore_soft(geometry.glue_vertices(k, brep, new_v1, new_v2, None))?;

        out.new_verts.push(new_v1); // This one will survive.
    }

    Ok(out)
}

/// `sm_TestAndImprintFace(pFace, crPoint, sSrcZoneTol3d, ..)`: drops the point onto
/// the face's surface over the face's UV domain; when it lands within the
/// intersection tolerance of the source and face zone tolerances and classifies to the
/// interior of the face, a new vertex loop is made there in the face. Otherwise the
/// reason (far from the geometry, or close to the boundary) is reported.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a face without surface or a vertex loop without vertex;
/// `SER` of a failing drop; [`SolidError::Unported`] for `SmFace::GetBrep`
/// (`topology.face_p4`) and `SmBrep::MakeVertexLoop` (`topology.brep_p9`);
/// [`SolidError::InvalidInput`] for null handles upstream dereferences.
pub fn test_and_imprint_face<O: ClassObject + SolutionObject + Ord>(
    k: &Kernel,
    geometry: &mut impl IntersectorBrep<O>,
    face: O,
    point: Point3,
    src_zone_tol_3d: f64,
) -> Result<ImprintFace<O>> {
    // Init outputs
    let mut out = ImprintFace {
        reason: Reason::NoReason,
        dist_to_face: BIG_DOUBLE,
        new_vertex: None,
    };

    let surface = geometry
        .face_surface(face)
        .ok_or(dangling("face handle is unknown"))?
        .ok_or(SolidError::null_pointer(OP_IMPRINT_FACE))?;
    let uv_domain = geometry
        .face_uv_domain(face)
        .ok_or(dangling("face handle is unknown"))?;
    let dropped: DropPoint<O> = geometry
        .surface(surface)
        .ok_or(dangling("surface handle cannot be resolved"))?
        .drop_point(k, point, &uv_domain, &DropPointOptions::default())?;
    out.dist_to_face = dropped.gap;

    // Check far from surface.
    let face_zone_tol_3d = geometry
        .topology_tolerance(face)
        .ok_or(dangling("face handle is unknown"))?;
    let xsect_tol_3d = xsect_tol_3d_from_zones(k, src_zone_tol_3d, face_zone_tol_3d);
    if !dropped.success || out.dist_to_face > xsect_tol_3d {
        out.reason = Reason::FarFromGeom;
        return Ok(out);
    }

    // Check interior.
    // Note, it will presumably not be close to a vertex or an edge of the face.
    let mut point_class: PointClassification<O> = PointClassification::new(src_zone_tol_3d);

    // the status is not checked upstream
    ignore_soft(geometry.face_point_classify(k, face, dropped.result_uv, &mut point_class))?;

    match point_class.point_class() {
        PointClassType::Unknown => {
            out.reason = Reason::FarFromGeom;
            return Ok(out);
        }
        PointClassType::Edge | PointClassType::Vertex => {
            out.reason = Reason::CloseToBoundary;
            return Ok(out);
        }
        PointClassType::Face => {}
        _ => {
            // SM_ASSERT(FALSE)
            out.reason = Reason::CloseToBoundary; // most probable reason; don't really care too much.
            return Ok(out);
        }
    }

    // Point is on the surface, within the face.
    let f_brep = geometry
        .face_brep(face)?
        .ok_or(dangling("face without B-rep"))?;
    match geometry
        .make_vertex_loop(f_brep, face, point)
        .map(|(l, v)| (l, Some(v)))
    {
        Ok((_new_loop, new_v)) => {
            let new_v = new_v.ok_or(SolidError::null_pointer(OP_IMPRINT_FACE))?;
            out.new_vertex = Some(new_v);
        }
        Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => return Err(e),
        Err(_) => {
            out.reason = Reason::CloseToBoundary; // most probable reason; don't really care too much.
            return Ok(out);
        }
    }

    Ok(out)
}

/// Copies the paired geometry before insertion consumes it. Keep the original
/// parameterisation: the edge's interval selects the same piece of both curves.
/// A missing or stale UV curve leaves projection to `MakeEdgeInFace`.
fn copy_intersection_edge_curves<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &CurveClassification<O>,
) -> Result<(Curve, Option<Curve>)> {
    let curve_3d = cc.curve().ok_or(dangling("classification without curve"))?;
    let copy = geometry
        .curve(curve_3d)
        .ok_or(dangling("curve handle cannot be resolved"))?
        .copy(k)?;
    let uv_copy = cc
        .uv_curve1()
        .map(|uv| {
            geometry
                .curve(uv)
                .ok_or(dangling("UV curve handle cannot be resolved"))?
                .copy(k)
        })
        .transpose()?;
    Ok((copy, uv_copy))
}

// ---- mate storage of the merge stages (co-ported by boolean.merge_p1, PR #163) -----------

// The same SetSubset operation for generic handles and the concrete merge adapter.
pub(crate) fn set_subset_objects<O: Copy + Ord>(
    subset: &mut Option<BTreeSet<O>>,
    subset_geometry: &[O],
) {
    // clear out any old Subset SmMapPtrToPtr object
    *subset = None;

    // size the new array
    if !subset_geometry.is_empty() {
        *subset = Some(BTreeSet::new());
    }

    // for every Object in the subset make a subset entry
    if let Some(set) = subset {
        for &obj in subset_geometry {
            set.insert(obj);
        }
    }
}

fn common_mates<I: Copy>(
    order: impl IntoIterator<Item = I>,
    mate: impl Fn(I) -> Option<I>,
) -> (Vec<I>, Vec<I>) {
    let mut out = (Vec::new(), Vec::new());
    for id in order {
        if let Some(other) = mate(id) {
            out.0.push(id);
            out.1.push(other);
        }
    }
    out
}

/// The same mate relationship viewed in both operand directions: the `m_pBToO` /
/// `m_pOToB` pair over one kind of operand-local [`Brep`] arena id, as the merge
/// stages hold it.
#[derive(Debug, Clone)]
pub struct MateMap<I> {
    to_other: BTreeMap<I, I>,
    to_primary: BTreeMap<I, I>,
}

impl<I> Default for MateMap<I> {
    fn default() -> Self {
        Self {
            to_other: BTreeMap::new(),
            to_primary: BTreeMap::new(),
        }
    }
}

impl<I: Ord + Copy> MateMap<I> {
    /// `GetOtherMate`: the mate of a primary operand id.
    pub fn other(&self, primary: I) -> Option<I> {
        self.to_other.get(&primary).copied()
    }

    /// `GetBrepMate`: the mate of an other operand id.
    pub fn primary(&self, other: I) -> Option<I> {
        self.to_primary.get(&other).copied()
    }

    /// Primary-to-other projection, borrowed by the shared merge helpers.
    #[must_use]
    pub fn primary_to_other(&self) -> &BTreeMap<I, I> {
        &self.to_other
    }

    /// Other-to-primary projection, borrowed by the shared merge helpers.
    #[must_use]
    pub fn other_to_primary(&self) -> &BTreeMap<I, I> {
        &self.to_primary
    }

    /// `GetMappedBrepObjects` for this typed class, in other-handle order.
    #[must_use]
    pub fn mapped_primary(&self) -> Vec<I> {
        self.to_primary.values().copied().collect()
    }

    /// `GetCommonEdges` / `GetCommonVertices`, preserving the supplied B-rep list order.
    #[must_use]
    pub fn common(&self, order: impl IntoIterator<Item = I>) -> (Vec<I>, Vec<I>) {
        common_mates(order, |id| self.other(id))
    }

    /// Record an established pair. Conflicting pairs fail before either map changes.
    /// New geometry is related with [`Relations::relate_vertices`] (`Relate`, with its
    /// tolerance updates) instead.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either id already has a different mate.
    pub fn record(&mut self, primary: I, other: I) -> Result<()> {
        if self.other(primary).is_some_and(|v| v != other)
            || self.primary(other).is_some_and(|v| v != primary)
        {
            return Err(SolidError::sm_err(
                "boolean::topology_intersector::p1::record",
            ));
        }
        self.to_other.insert(primary, other);
        self.to_primary.insert(other, primary);
        Ok(())
    }
}

/// Face, edge and vertex mates shared by the merge stages; every id is operand-local.
/// Curves and surfaces are owned by their edge and face in the arena model, so they
/// have no mates of their own (see `boolean.merge_p2`).
#[derive(Debug, Default, Clone)]
pub struct Relations {
    /// Face mates in both directions; face Relate has no tolerance update.
    pub faces: MateMap<FaceId>,
    /// Edge mates in both directions.
    pub edges: MateMap<EdgeId>,
    /// Vertex mates in both directions.
    pub vertices: MateMap<VertexId>,
}

impl Relations {
    /// `Relate(pBrepTopo, pOtherTopo)` for two edges ([`TopologyIntersector::relate`]'s
    /// edge branch on arena ids; CO-PORTED by `boolean.merge_p2`): the conflict checks,
    /// then (`SM_USE_OLDTOL`) each edge gets the other's tolerance when that is larger
    /// (`SmEdge::SetTolerance` with its defaults `TRUE, TRUE`), then the pair is stored.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either edge already has a different mate (nothing changes);
    /// [`SolidError::NotFound`] for a dangling id; those of `SetTolerance`.
    pub fn relate_edges(
        &mut self,
        brep: &mut Brep,
        brep_edge: EdgeId,
        other: &mut Brep,
        other_edge: EdgeId,
    ) -> Result<()> {
        // check for conflicting existing mate relationships
        if self.edges.other(brep_edge).is_some_and(|e| e != other_edge)
            || self
                .edges
                .primary(other_edge)
                .is_some_and(|e| e != brep_edge)
        {
            return Err(SolidError::sm_err(OP_RELATE));
        }

        // when objects are edges - set object tolerances to max(object1 tol, object2 tol)
        let tol1 = brep.edge(brep_edge)?.tolerance;
        let tol2 = other.edge(other_edge)?.tolerance;
        if tol1 < tol2 {
            edge::set_tolerance(brep, brep_edge, tol2, true, true)?;
        }
        let tol1 = brep.edge(brep_edge)?.tolerance;
        if tol2 < tol1 {
            edge::set_tolerance(other, other_edge, tol1, true, true)?;
        }

        // add the pair to the Ptr to Ptr map
        self.edges.record(brep_edge, other_edge)
    }

    /// `Relate(pBrepTopo, pOtherTopo)` for two faces (CO-PORTED by
    /// `boolean.merge_p2`): the conflict checks, then the pair is stored. Faces have no
    /// tolerance branch upstream.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either face already has a different mate (nothing changes);
    /// [`SolidError::NotFound`] for a dangling id.
    pub fn relate_faces(
        &mut self,
        brep: &Brep,
        brep_face: FaceId,
        other: &Brep,
        other_face: FaceId,
    ) -> Result<()> {
        brep.face(brep_face)?;
        other.face(other_face)?;
        self.faces.record(brep_face, other_face)
    }

    /// Move the typed mate store through a generic intersector operation, then
    /// restore it even when that operation fails. Geometry and proto state stay
    /// in the same intersector; no second live relationship store is retained.
    pub(crate) fn with_intersector<O, T>(
        &mut self,
        intersector: &mut TopologyIntersector<O>,
        encode: impl Fn(EntityRef, bool) -> O,
        decode: impl Fn(O) -> EntityRef,
        action: impl FnOnce(&mut TopologyIntersector<O>) -> Result<T>,
    ) -> Result<T>
    where
        O: ClassObject + SolutionObject + Ord,
    {
        let mut forward = BTreeMap::new();
        let mut reverse = BTreeMap::new();
        macro_rules! transfer {
            ($field:ident, $variant:ident) => {
                for (b, o) in std::mem::take(&mut self.$field.to_other) {
                    let b = encode(EntityRef::$variant(b), false);
                    let o = encode(EntityRef::$variant(o), true);
                    forward.insert(b, Some(o));
                }
                for (o, b) in std::mem::take(&mut self.$field.to_primary) {
                    let b = encode(EntityRef::$variant(b), false);
                    let o = encode(EntityRef::$variant(o), true);
                    reverse.insert(Some(o), b);
                }
            };
        }
        transfer!(faces, Face);
        transfer!(edges, Edge);
        transfer!(vertices, Vertex);
        intersector.b_to_o = Some(forward);
        intersector.o_to_b = Some(reverse);
        let result = action(intersector);
        // Preserve both actual projections, including changes made before an
        // error. All handles admitted here represent the same typed entities.
        let mut restore_error = None;
        if let Some(forward) = intersector.b_to_o.take() {
            for (b, o) in forward {
                if let Some(o) = o {
                    match (decode(b), decode(o)) {
                        (EntityRef::Face(b), EntityRef::Face(o)) => {
                            self.faces.to_other.insert(b, o);
                        }
                        (EntityRef::Edge(b), EntityRef::Edge(o)) => {
                            self.edges.to_other.insert(b, o);
                        }
                        (EntityRef::Vertex(b), EntityRef::Vertex(o)) => {
                            self.vertices.to_other.insert(b, o);
                        }
                        _ => {
                            restore_error = Some(SolidError::invalid_input(
                                OP_RELATE,
                                "mixed typed mate handles",
                            ));
                        }
                    }
                }
            }
        }
        if let Some(reverse) = intersector.o_to_b.take() {
            for (o, b) in reverse {
                if let Some(o) = o {
                    match (decode(o), decode(b)) {
                        (EntityRef::Face(o), EntityRef::Face(b)) => {
                            self.faces.to_primary.insert(o, b);
                        }
                        (EntityRef::Edge(o), EntityRef::Edge(b)) => {
                            self.edges.to_primary.insert(o, b);
                        }
                        (EntityRef::Vertex(o), EntityRef::Vertex(b)) => {
                            self.vertices.to_primary.insert(o, b);
                        }
                        _ => {
                            restore_error = Some(SolidError::invalid_input(
                                OP_RELATE,
                                "mixed typed mate handles",
                            ));
                        }
                    }
                }
            }
        }
        restore_error.map_or(result, Err)
    }

    /// `Relate(pBrepTopo, pOtherTopo)` for two vertices of the concrete [`Brep`]
    /// operands of the merge stages: [`TopologyIntersector::relate`]'s vertex branch
    /// on arena ids. The conflict checks come first; then (`SM_USE_OLDTOL`) each vertex
    /// gets the other's tolerance when that is larger (`SetTolerance` with its
    /// defaults `bUpdateOnlyIfLarger = TRUE`, `bCascadeToBndries = TRUE`); then the
    /// pair is stored in both directions.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either vertex already has a different mate (nothing changes);
    /// [`SolidError::NotFound`] for a dangling id (upstream's `NER` of a null vertex).
    pub fn relate_vertices(
        &mut self,
        brep: &mut Brep,
        brep_vertex: VertexId,
        other: &mut Brep,
        other_vertex: VertexId,
    ) -> Result<()> {
        // check for conflicting existing mate relationships
        let oth = self.vertices.other(brep_vertex);
        let brp = self.vertices.primary(other_vertex);
        if oth.is_some_and(|v| v != other_vertex) {
            return Err(SolidError::sm_err(OP_RELATE));
        }
        if brp.is_some_and(|v| v != brep_vertex) {
            return Err(SolidError::sm_err(OP_RELATE));
        }

        // when objects are vertices - set object tolerances to max(object1 tol, object2 tol)
        let tol1 = vertex::tolerance(brep, brep_vertex)?;
        let tol2 = vertex::tolerance(other, other_vertex)?;
        if tol1 < tol2 {
            vertex::set_tolerance(brep, brep_vertex, tol2, true, true)?;
        }
        let tol1 = vertex::tolerance(brep, brep_vertex)?;
        if tol2 < tol1 {
            vertex::set_tolerance(other, other_vertex, tol1, true, true)?;
        }

        // add the pair to the Ptr to Ptr map
        self.vertices.record(brep_vertex, other_vertex)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn trim_owner_is_found_through_the_edge_use_not_the_last_face() {
        let mut k = Kernel::default();
        let b = crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 1.0, 1.0, 1.0)
            .unwrap();
        let other = b.clone();
        let faces: Vec<FaceId> = b.faces().ids().collect();
        let (first, last) = (faces[0], faces[faces.len() - 1]);
        // an edge use of `first`'s loop
        let eu = b
            .edgeuses()
            .ids()
            .find(|&e| {
                crate::topology::edgeuse::p1::loopuse(&b, e)
                    .ok()
                    .flatten()
                    .and_then(|lu| crate::topology::loopuse::faceuse(&b, lu).ok().flatten())
                    .and_then(|fu| {
                        crate::topology::edgeuse::p1::faceuse_face(&b, fu)
                            .ok()
                            .flatten()
                    })
                    == Some(first)
            })
            .unwrap();
        // both faces resolved before the request, the owner first; the same face index
        // of an identical copy is resolved too and must be refused by identity
        let candidates = [(1u8, &b, first), (2, &b, last), (3, &other, first)];
        let on_b = |x: &Brep| core::ptr::eq(x, core::ptr::from_ref(&b));
        assert_eq!(trim_owner_face(&candidates, on_b, eu), Some(1));
        let reversed = [(3u8, &other, first), (2, &b, last), (1, &b, first)];
        assert_eq!(trim_owner_face(&reversed, on_b, eu), Some(1));
        assert_eq!(
            trim_owner_face(
                &candidates,
                |x| core::ptr::eq(x, core::ptr::from_ref(&other)),
                eu
            ),
            Some(3)
        );
        assert_eq!(trim_owner_face(&candidates[1..2], on_b, eu), None);
    }
    use super::*;
    use crate::geom::extent1d::Extent1d;
    use crate::geom::solution_array::{Solution, SolutionArray};
    use crate::math::Vec3;

    /// A handle: a kind and an index.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct H(u8, u32);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            match self.0 {
                0 => ClassObjectKind::Vertex,
                1 => ClassObjectKind::Edge,
                2 => ClassObjectKind::Face,
                _ => ClassObjectKind::Other,
            }
        }
    }

    impl SolutionObject for H {
        fn kind(&self) -> ObjectKind {
            match self.0 {
                0 => ObjectKind::Vertex,
                1 => ObjectKind::Edge,
                2 => ObjectKind::Face,
                _ => ObjectKind::Other,
            }
        }
    }

    const V0: H = H(0, 0);
    const V1: H = H(0, 1);
    const E0: H = H(1, 0);
    const E1: H = H(1, 1);
    const F0: H = H(2, 0);
    const F1: H = H(2, 1);

    struct Curves(Vec<Curve>);

    impl ClassGeometry<H> for Curves {
        fn curve(&self, o: H) -> Option<&Curve> {
            self.0.get(o.1 as usize)
        }
        fn surface(&self, _o: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _o: H) -> Option<&crate::surface::volume::Volume> {
            None
        }
        fn point(&self, _o: H) -> Option<Point3> {
            None
        }
    }

    #[test]
    fn intersection_edge_copies_each_faces_uv_with_its_parameterisation() {
        use crate::nurbs::b_spline_curve::BSplineCurve;
        use crate::nurbs::math::{CPoint, NOW, NOZ};

        let k = Kernel::default();
        let domain = Extent1d::from_min_max(-3.0, 7.0);
        let mut curves = Vec::new();
        // One curved 3D intersection and two different surface parameterisations.
        for (dim, y, z) in [(3, 2.0, 4.0), (2, 0.25, NOZ), (2, -2.0, NOZ)] {
            curves.push(Curve::from(
                BSplineCurve::from_data(
                    dim,
                    2,
                    5,
                    2,
                    &[
                        CPoint::new(0.0, y, z, NOW),
                        CPoint::new(0.5, y + 0.5, z, NOW),
                        CPoint::new(1.0, y, z, NOW),
                    ],
                    &[-3.0, -3.0, -3.0, 7.0, 7.0, 7.0],
                )
                .unwrap(),
            ));
        }
        let geometry = Curves(curves);
        // Two subintervals can consume independent copies of the same trace.
        for (face_uv, interval) in [
            (H(4, 1), Extent1d::from_min_max(-2.0, 1.0)),
            (H(4, 2), Extent1d::from_min_max(1.0, 6.0)),
        ] {
            let cc = new_curve_classification(&k, H(4, 0), &domain, Some(face_uv), 1e-6).unwrap();
            let (mut xyz, uv) = copy_intersection_edge_curves(&k, &geometry, &cc).unwrap();
            let mut uv = uv.unwrap();
            assert_eq!(xyz.natural_interval(), domain);
            assert_eq!(uv.natural_interval(), domain);
            for t in [interval.min(), interval.mid(), interval.max()] {
                assert_eq!(
                    xyz.evaluate_point(&k, t).unwrap(),
                    geometry.0[0].evaluate_point(&k, t).unwrap()
                );
                assert_eq!(
                    uv.evaluate_point(&k, t).unwrap(),
                    geometry
                        .curve(face_uv)
                        .unwrap()
                        .evaluate_point(&k, t)
                        .unwrap()
                );
            }
            let mut ivl = interval;
            xyz.trim(
                &k,
                &mut ivl,
                crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
            )
            .unwrap();
            uv.trim(
                &k,
                &mut ivl,
                crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
            )
            .unwrap();
            assert_eq!(geometry.0[0].natural_interval(), domain);
            assert_eq!(geometry.curve(face_uv).unwrap().natural_interval(), domain);
        }
    }

    #[test]
    fn intersection_edge_projects_only_when_current_uv_is_unavailable() {
        let k = Kernel::default();
        let curve = Curve::from(crate::curve::line::Line::from_points(
            Vec3::ZERO,
            Vec3::new(1.0, 1.0, 0.0),
            3,
        ));
        let domain = curve.natural_interval();
        let geometry = Curves(vec![curve]);
        let mut cc = new_curve_classification(&k, H(4, 0), &domain, None, 1e-6).unwrap();
        assert!(copy_intersection_edge_curves(&k, &geometry, &cc)
            .unwrap()
            .1
            .is_none());
        cc.set_uv_curve1(Some(H(4, 99)));
        assert!(matches!(
            copy_intersection_edge_curves(&k, &geometry, &cc),
            Err(SolidError::InvalidInput { .. })
        ));
        cc.set_uv_curve1(Some(H(4, 0)));
        cc.set_intersect_data(None, None, None, false, None);
        assert_eq!(cc.any_uv_curve1(), Some(H(4, 0)));
        assert_eq!(cc.uv_curve1(), None);
        assert!(copy_intersection_edge_curves(&k, &geometry, &cc)
            .unwrap()
            .1
            .is_none());
    }

    #[test]
    fn intersection_uv_copy_preserves_analytic_kind() {
        let k = Kernel::default();
        let xyz = Curve::from(crate::curve::line::Line::from_points(
            Vec3::ZERO,
            Vec3::new(1.0, 1.0, 0.0),
            3,
        ));
        let uv = Curve::from(crate::curve::line::Line::from_points(
            Vec3::new(2.0, 3.0, 0.0),
            Vec3::new(4.0, 5.0, 0.0),
            2,
        ));
        let domain = xyz.natural_interval();
        let geometry = Curves(vec![xyz, uv]);
        let cc = new_curve_classification(&k, H(4, 0), &domain, Some(H(4, 1)), 1e-6).unwrap();
        let (_, copy) = copy_intersection_edge_curves(&k, &geometry, &cc).unwrap();
        let copy = copy.unwrap();
        assert!(matches!(copy.kind(), CurveKind::Line(_)));
        for t in [domain.min(), domain.mid(), domain.max()] {
            assert_eq!(
                copy.evaluate_point(&k, t).unwrap(),
                geometry.0[1].evaluate_point(&k, t).unwrap()
            );
        }
    }

    #[test]
    fn mates_follow_the_maps() {
        let mut ti: TopologyIntersector<H> =
            TopologyIntersector::new(Some(H(9, 0)), Some(H(9, 1)), 1e-5, 0.1);
        assert!(ti.other_mate(Some(F0)).is_none());
        assert!(!ti.is_mated_pair(Some(F0), Some(F1)));
        // faces have no tolerance handling: no B-rep access needed
        let b_to_o = ti.b_to_o.as_mut().unwrap();
        b_to_o.insert(F0, Some(F1));
        ti.o_to_b.as_mut().unwrap().insert(Some(F1), F0);
        assert_eq!(ti.other_mate(Some(F0)), Some(F1));
        assert_eq!(ti.this_mate(Some(F1)), Some(F0));
        assert_eq!(ti.brep_mate(Some(F1)), Some(F0));
        assert!(ti.is_mated_pair(Some(F0), Some(F1)));
        assert!(!ti.is_mated_pair(None, Some(F1)));
        assert_eq!(
            ti.mapped_brep_objects(ClassObjectKind::Face).unwrap(),
            vec![F0]
        );
        assert!(ti
            .mapped_brep_objects(ClassObjectKind::Edge)
            .unwrap()
            .is_empty());
        ti.set_subset(&[F0]);
        ti.clear_relationships().unwrap();
        assert_eq!(ti.other_mate(Some(F0)), None);
        assert_eq!(ti.this_mate(Some(F1)), None);
        assert_eq!(ti.primary_brep(), Some(H(9, 0)));
        assert!(ti.subset().unwrap().contains(&F0));
    }

    #[test]
    fn context_only_intersector_has_no_maps() {
        let ti: TopologyIntersector<H> = TopologyIntersector::with_context();
        assert!(ti.other_mate(Some(F0)).is_none());
        assert!(ti.brep_mate(Some(F0)).is_none());
        assert!(matches!(
            ti.mapped_brep_objects(ClassObjectKind::Face),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(ti.skip_manifold_edges && ti.intersect_wire_edges && !ti.skip_ssi);
    }

    #[test]
    fn subset_is_a_set() {
        let mut ti: TopologyIntersector<H> = TopologyIntersector::with_context();
        assert!(ti.subset().is_none());
        ti.set_subset(&[E1, E0, E1]);
        let s = ti.subset().unwrap();
        assert_eq!(s.iter().copied().collect::<Vec<_>>(), vec![E0, E1]);
        ti.set_subset(&[]);
        assert!(ti.subset().is_none());
    }

    #[test]
    fn mid_point_fixup_resets_mismatched_edges() {
        let mut ti: TopologyIntersector<H> = TopologyIntersector::new(None, None, 1e-5, 0.1);
        ti.b_to_o.as_mut().unwrap().insert(E0, Some(E1));
        ti.o_to_b.as_mut().unwrap().insert(Some(E1), E0);
        let mut pc_b = PointClassification::new(1e-5);
        pc_b.set_class_object(PointClassType::Edge, Some(E0));
        let mut pc_o = PointClassification::new(1e-5);
        pc_o.set_class_object(PointClassType::Edge, Some(H(1, 7)));
        ti.fixup_mid_point_class_mates(3, &mut pc_b, &mut pc_o, None);
        assert_eq!(pc_b.point_class(), PointClassType::Unknown);
        assert_eq!(pc_o.point_class(), PointClassType::Unknown);

        // matching mates are left alone
        let mut pc_b = PointClassification::new(1e-5);
        pc_b.set_class_object(PointClassType::Edge, Some(E0));
        let mut pc_o = PointClassification::new(1e-5);
        pc_o.set_class_object(PointClassType::Edge, Some(E1));
        ti.fixup_mid_point_class_mates(3, &mut pc_b, &mut pc_o, None);
        assert_eq!(pc_b.point_class(), PointClassType::Edge);

        // vertices never reset
        let mut pc_b = PointClassification::new(1e-5);
        pc_b.set_class_object(PointClassType::Vertex, Some(V0));
        ti.b_to_o.as_mut().unwrap().insert(V0, Some(V1));
        let mut pc_o = PointClassification::new(1e-5);
        pc_o.set_class_object(PointClassType::Vertex, Some(H(0, 5)));
        ti.fixup_mid_point_class_mates(3, &mut pc_b, &mut pc_o, None);
        assert_eq!(pc_b.point_class(), PointClassType::Vertex);
        ti.fixup_end_point_class_mates(3, &mut pc_b, &mut pc_o, None);
        assert_eq!(pc_b.point_class(), PointClassType::Vertex);
    }

    #[test]
    fn lowest_dimension_solution() {
        let mut sols: SolutionArray<H> = SolutionArray::with_capacity(4);
        let mut s = Solution::default();
        s.objects[0] = Some(F0);
        sols.add(&s);
        s.objects[0] = Some(E0);
        sols.add(&s);
        s.objects[0] = Some(V0);
        sols.add(&s);
        s.objects[0] = Some(E1);
        sols.add(&s);
        assert_eq!(find_first_lowest_dimension_sol(&sols).unwrap(), 2);
        sols.remove_at(2, 1);
        assert_eq!(find_first_lowest_dimension_sol(&sols).unwrap(), 1);
        sols.remove_at(1, 2);
        // a single solution is used whatever it is
        assert_eq!(find_first_lowest_dimension_sol(&sols).unwrap(), 0);
        let empty: SolutionArray<H> = SolutionArray::with_capacity(0);
        assert_eq!(find_first_lowest_dimension_sol(&empty).unwrap(), 0);
        let _ = Extent1d::new();
        assert!(can_imprint(PointClassType::Region) && !can_imprint(PointClassType::Curve));
    }

    #[test]
    fn table_destroy_lists_curves() {
        let mut table: IntersectionTable<H> = IntersectionTable::default();
        table.brep_surfaces.push(Some(H(3, 0)));
        table.other_surfaces.push(None);
        table.results.push(IntersectionResults::default());
        table.brep_surfaces.push(Some(H(3, 0)));
        table.other_surfaces.push(Some(H(3, 1)));
        table.results.push(IntersectionResults {
            curves: Some(IntersectionCurves {
                curves_3d: vec![Some(H(4, 0)), Some(H(4, 1))],
                uv_curves1: vec![None, Some(H(4, 2))],
                uv_curves2: vec![Some(H(4, 3)), None],
                curve_types: vec![TsectCurveType::Crossing; 2],
                deviations: vec![0.0; 2],
            }),
        });
        let mut ti: TopologyIntersector<H> = TopologyIntersector::with_context();
        ti.set_intersection_table(Some(table));
        let deleted = ti.clear_existing_ssi();
        assert_eq!(deleted, vec![H(4, 0), H(4, 3), H(4, 1), H(4, 2)]);
        assert!(ti.intersection_table().is_none());
        assert!(ti.clear_existing_ssi().is_empty());
    }

    #[test]
    fn conflicts_leave_both_directions_unchanged() {
        let mut m = MateMap::default();
        m.record(1, 8).unwrap();
        m.record(1, 8).unwrap();
        assert!(m.record(1, 9).is_err());
        assert!(m.record(2, 8).is_err());
        assert_eq!(m.other(1), Some(8));
        assert_eq!(m.primary(8), Some(1));
        assert_eq!(m.other(2), None);
        assert_eq!(m.primary(9), None);
    }
}
