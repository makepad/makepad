//! Topology intersector, part 2: registering existing surface/surface intersections,
//! the mate-map edits (squeezing re-mated vertices, removing relationships, deleting
//! faces, closing gaps), the `IntersectInsertRelate` driver with its topology
//! intersection and relationship stages, the classification-pair fix-ups, the
//! edge/edge and edge/face-coincidence internal-use methods and the co-ported
//! intersection-loop closing (`CloseIntersectionLoop` with its `IIREdgeBrep`).
//!
//! Upstream: `source/SMLib/src/SmTopologyIntersector.cpp`,
//! `source/SMLib/inc/SmTopologyIntersector.h`.
//!
//! | Rust                                                  | Upstream                      |
//! |-------------------------------------------------------|-------------------------------|
//! | [`TopologyIntersector::register_existing_ssi`]        | `RegisterExistingSSI`         |
//! | [`TopologyIntersector::check_for_squeeze_vertices`]   | `CheckForSqueezeVertices`     |
//! | [`TopologyIntersector::check_for_squeeze_vertices_into`] | the same, appending to the caller's lists |
//! | [`SqueezeVertices`]                                   | its three out-lists           |
//! | [`TopologyIntersector::delete_faces`]                 | `DeleteFaces`                 |
//! | [`TopologyIntersector::fix_gaps`]                     | `FixGaps`                     |
//! | [`FixGaps`]                                           | its out-params                |
//! | [`TopologyIntersector::remove_relationship`]          | `RemoveRelationship`          |
//! | [`TopologyIntersector::intersect_insert_relate`]      | `IntersectInsertRelate`       |
//! | [`TopologyIntersector::intersect_topology`]           | `IntersectTopology`           |
//! | [`TopologyIntersector::set_relationships`]            | `SetRelationships`            |
//! | [`TopologyIntersector::fixup_class_mates`]            | `FixupClassMates`             |
//! | [`TopologyIntersector::fixup_split_vertex_pair`]      | `FixupSplitVertexPair`        |
//! | [`TopologyIntersector::edge_edge_intersect`]          | `EdgeEdgeIntersect`           |
//! | [`EdgeEdgeIntersect`]                                 | its out-params                |
//! | [`TopologyIntersector::edge_face_coincidence`]        | `EdgeFaceCoincidence`         |
//! | [`EdgeFaceCoincidence`]                               | its out-params                |
//! | [`TopologyIntersector::close_intersection_loop`]      | `CloseIntersectionLoop` (co-ported) |
//! | [`TopologyIntersector::iir_edge_brep`]                | `IIREdgeBrep` (co-ported)     |
//! | [`DeletedTopology`]                                   | `IIREdgeBrep`'s deleted-topology outputs |
//! | [`IntersectorBrepP2`]                                 | the `SmVertex` walks these use |
//! | [`MapMates`]                                          | `GetOtherMate` / `RemoveRelationship` for `SmProtoTopologyManager::GlueCoincidentBrepVertices` |
//!
//! The two constructors are in [`p1`](super::p1) (co-ported there). `CloseIntersectionLoop`
//! (`SmTopologyIntersector.cpp:6108-6368`, called from `SmMerge.cpp:2961`) and its only
//! callee `IIREdgeBrep` (`:2836-2971`) are listed by no unit in `units.json`; they are
//! co-ported here.
//!
//! Deviations:
//! - **Deleted gap edges (issue #432).** As source fork be3fc10,
//!   `FixGaps` unmates each deleted edge even when only one B-rep deletes an edge.
//!   The pinned reference leaves that edge in the maps and later reads freed memory.
//! - **Handles and B-reps** as in [`p1`](super::p1): objects are handles `O`, the
//!   B-reps are read and edited through [`IntersectorBrep`] plus [`IntersectorBrepP2`]
//!   (`SmVertex::GetEdges` / `GetFaces`, `SmBrep::GetFaces` /
//!   `CombineCoincidentVertices`). `IntersectInsertRelate`
//!   takes the handle the manager stores for the intersector (`pTI`).
//! - **Out-params** become return values ([`SqueezeVertices`], [`FixGaps`],
//!   [`EdgeEdgeIntersect`], [`EdgeFaceCoincidence`], `bool` for
//!   `bChangedClassifications`); on an error status the partly filled out-lists are
//!   not returned, except by `check_for_squeeze_vertices_into`, which appends to the
//!   caller's lists as upstream does (a failure after the first conflict keeps its
//!   appends). The `NotUsed` `bMakingManifoldSolid` of `EdgeEdgeIntersect` is kept.
//! - **Opt-in (WR-527, #1526).** With `Kernel::fixes.boolean_skips_faces_apart`,
//!   `IntersectTopology` skips a face pair whose face boxes are disjoint by the
//!   intersection tolerance (the fork's source fork@60b1c6a). It then visits,
//!   for each face of the B-rep, only the faces of the other B-rep a box search
//!   (`SpatialIndex`) finds not disjoint from its box, in ascending order (the fork's
//!   source fork@84a7998): a superset of the pairs the box test keeps, so the
//!   pairs processed and their order are the same, and a join of two bodies of n faces
//!   no longer walks all n^2 pairs to skip them. With an intersection table registered
//!   every pair is still visited (its lookup copies curves before the box test).
//! - **Deleted curves.** `IntersectTopology` and `IntersectInsertRelate` return the
//!   curve handles deleted (by `SplitIntersectingProtoEdges`, the resolution, the
//!   imprint and the manager's destructor), as [`p1`](super::p1) does.
//! - **A failed imprint** (source fix, source fork af8db37, UB-242).
//!   Upstream's `IntersectInsertRelate` reports a failure of `ImprintIntersections`
//!   and then overwrites its status with that of `SetRelationships`, so the boolean
//!   goes on with both B-reps half imprinted (upstream: reads of freed vertex uses).
//!   The port deletes the manager and returns the imprint's error, unless every
//!   failing imprint stopped before editing a B-rep (an edge definition without a
//!   final vertex): the B-reps are then consistent and the port goes on as upstream.
//! - **Statuses upstream ignores** (`SmBrep` edits whose status is unchecked,
//!   `RemoveRelationship` in `DeleteFaces` / `FixGaps`, bounding boxes, `DropPoint`,
//!   `SimpleCoincidenceChecker`, `CalculateUVDomainFromUVTrimCurves`, `Relate` in
//!   `SetRelationships`, the shell-vertex and wire-edge intersections of
//!   `IntersectTopology`) are ignored through `ignore_soft`, except
//!   [`SolidError::Unported`] and [`SolidError::InvalidInput`]; the ignored call's
//!   outputs keep their initial values (a failed `DropPoint` is "no drop point", a
//!   failed coincidence check "not coincident").
//! - **Curve ownership.** `EdgeFaceCoincidence` resets the curve's owner to the
//!   original edge after `MakeEdgeInFace`; the port has no curve owners (the curve
//!   copy is a new handle), so there is nothing to reset.
//! - **Callees not ported yet** are private stubs returning [`SolidError::Unported`],
//!   each tagged `UNPORTED(<unit>)`:
//!   `SmFace::Point3DClassify` (`query.face_p1`) and the stub of [`p1`](super::p1)
//!   for `SmFace::FaceIntersect` (`topology.face_p3`). `SmFace::GetEdges` /
//!   `GetVertices` / `GetBrep` go through [`IntersectorBrep::face_edges`] and siblings. The code after a stub is ported in
//!   full. `SmBrep::DeleteEdge` / `DeleteVertex` (`topology.brep_p4`) go through
//!   [`IntersectorBrep::brep_delete_edge`] / [`IntersectorBrep::brep_delete_vertex`].
//!   `SmBrep::GlueEdgesGeneral` (`topology.brep_p7`) goes through
//!   [`IntersectorBrep::brep_glue_edges_general`] and `SmBrep::MakeEdgeInFace`
//!   (`topology.brep_p5`) through part 1's `brep_make_edge_in_face`, as in part 1;
//!   `SmBrep::CombineCoincidentVertices` (`topology.brep_p5`) goes through
//!   [`IntersectorBrepP2::brep_combine_coincident_vertices`].
//! - **Undefined behaviour upstream** (a null or deleted object dereferenced, an
//!   index past a list, a vertex mated to a non-vertex used as a vertex) is
//!   [`SolidError::InvalidInput`].
//! - **Bounds.** `EdgeEdgeIntersect` appends every split edge to its working lists;
//!   past [`MAX_EDGE_LIST`] edges it fails with `SM_ERR` (upstream is unbounded).
//!   `FixupClassMates` restarts its loop after each `FixupSplitVertexPair` change;
//!   each change removes an interval, so it restarts at most `size` times.
//! - `SM_USE_OLDTOL` is the reference build's setting: `GetGap3d()` and
//!   `GetDeviation()` are the same stored gap, so the swapped `#ifdef`s of
//!   `FixupSplitVertexPair` read the same value. The `sbRelateFaces` block of
//!   `SetRelationships` is compiled out (`FALSE`); `SM_DEBUG_CODE` / `SM_USE_TBB`
//!   blocks are not built upstream and not ported.
//! - **Recursion.** None.

use crate::curve::curve::p2::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{SolutionObject, SolutionType};
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::curve_class::p2::ResetOptions;
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, ClassObjectKind, PointClassType, PointClassification,
};
use crate::intersect::topology_solver::p1::{CurveRef, TopologySolver};
use crate::intersect::topology_solver::p2::TopologySolveOptions;
use crate::kernel::Kernel;
use crate::math::{max2, max3, min2, Point3, BIG_DOUBLE, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::p2::TsectCurveType;
use crate::topology::brep::validation::spatial::SpatialIndex;
use crate::topology::proto_topology::p3::Mates;
use crate::topology::proto_topology::{ProtoEdgeId, ProtoTopologyManager, ProtoVertexId};

use super::p1::{
    add_unique, brep_make_edge_in_face, can_imprint, cc_insert_topology_intersections, contains,
    eval_curve_point, face_face_intersect_curves, find_common_elements, ignore_soft,
    intersect_shell_vertices, intersect_wire_lamina_edges, new_curve_classification,
    set_edge_class, solve_with_face_caches, surface_coincidence_check,
    surface_solve_with_face_caches, FaceIntersectOutputs, IntersectionCurves, IntersectionResults,
    IntersectorBrep, TopologyIntersector,
};
use super::p3::IntersectorBrepP3;

const OP_REGISTER: &str = "boolean::topology_intersector::p2::register_existing_ssi";
const OP_SQUEEZE: &str = "boolean::topology_intersector::p2::check_for_squeeze_vertices";
const OP_FIX_GAPS: &str = "boolean::topology_intersector::p2::fix_gaps";
const OP_REMOVE: &str = "boolean::topology_intersector::p2::remove_relationship";
const OP_CLASS_MATES: &str = "boolean::topology_intersector::p2::fixup_class_mates";
const OP_SPLIT_PAIR: &str = "boolean::topology_intersector::p2::fixup_split_vertex_pair";
const OP_EDGE_EDGE: &str = "boolean::topology_intersector::p2::edge_edge_intersect";
const OP_EDGE_FACE_COIN: &str = "boolean::topology_intersector::p2::edge_face_coincidence";
const OP_DELETE_FACES: &str = "boolean::topology_intersector::p2::delete_faces";
const OP_INSERT_RELATE: &str = "boolean::topology_intersector::p2::intersect_insert_relate";
const OP_INTERSECT_TOPOLOGY: &str = "boolean::topology_intersector::p2::intersect_topology";
const OP_SET_RELS: &str = "boolean::topology_intersector::p2::set_relationships";
const OP_CLOSE_LOOP: &str = "boolean::topology_intersector::p2::close_intersection_loop";
const OP_IIR_EDGE: &str = "boolean::topology_intersector::p2::iir_edge_brep";
const OP: &str = "boolean::topology_intersector::p2";

/// The error for a handle or state upstream dereferences without checking (part 1's
/// `dangling`, with `op` naming the part 2 function).
const fn invalid(op: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: op,
        reason,
    }
}

/// Most edges the working lists of [`TopologyIntersector::edge_edge_intersect`] may
/// hold (the inputs plus every split edge appended again); a bound of the port.
pub const MAX_EDGE_LIST: usize = 1 << 16;

// ---- what part 2 needs from the B-reps beyond part 1 -----------------------------------

/// The `SmVertex` walks of this part, on top of [`IntersectorBrep`]
/// (`SmEdge::GetFaces` is [`ProtoTopology::edge_faces`](crate::topology::proto_topology::p1::ProtoTopology::edge_faces)).
pub trait IntersectorBrepP2<O>: IntersectorBrep<O> {
    /// `SmVertex::GetEdges(rEdges)` (`topology::vertex::edges` with no face filter).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn vertex_edges(&self, vertex: O) -> Result<Vec<O>>;
    /// `SmVertex::GetFaces(rFaces)` (`topology::vertex::faces`).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn vertex_faces(&self, vertex: O) -> Result<Vec<O>>;
    /// The faces of `brep` in its list order (`SmBrep::GetFaces(rFaces)` with no
    /// attribute filter, unit `topology.brep_p1`: `topology::brep::p1::faces` on the
    /// B-rep behind the handle). Upstream's `pOptAttributeId` is dropped: both
    /// callers in this file pass the default; add it if a caller needs the filtered
    /// form.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown B-rep.
    fn brep_faces(&self, brep: O) -> Result<Vec<O>>;
    /// `SmBrep::CombineCoincidentVertices(pSurvivingVertex, pVertexToDelete,
    /// rpDeletedEdge)` on B-rep `brep` (unit `topology.brep_p5`:
    /// `topology::brep::p5::combine_with_notifications` on the B-rep behind the
    /// handle): the deleted edge, if any (`rpDeletedEdge`, null on failure as well).
    ///
    /// # Errors
    ///
    /// Those of the combination; [`SolidError::InvalidInput`] for an unknown B-rep or
    /// a vertex of another B-rep.
    fn brep_combine_coincident_vertices(
        &mut self,
        k: &mut Kernel,
        brep: O,
        surviving: O,
        to_delete: O,
    ) -> Result<Option<O>>;
    /// `pEdge->GetPrimaryEdgeuse()->NormalizedEvaluate(t, FALSE, rPoint)`: the 3D point
    /// at normalized parameter `t` of the edge's primary edge use
    /// (`topology::edgeuse::p3::normalized_evaluate`, unit `topology.edgeuse_p3`).
    ///
    /// # Errors
    ///
    /// Those of the evaluation; [`SolidError::InvalidInput`] for an unknown edge or
    /// one without primary edge use.
    fn edge_normalized_evaluate(&mut self, k: &Kernel, edge: O, t: f64) -> Result<Point3>;
}

/// `GetOtherMate` and `RemoveRelationship` over an intersector's mate maps, for
/// [`ProtoTopologyManager::glue_coincident_brep_vertices`] (the manager's `m_pTI`).
/// The maps are moved out of the intersector while the manager runs (the manager is
/// itself a member of the intersector) and moved back afterwards.
#[derive(Debug)]
pub struct MapMates<O> {
    /// `m_pBToO`.
    pub(super) b_to_o: Option<std::collections::BTreeMap<O, Option<O>>>,
    /// `m_pOToB`.
    pub(super) o_to_b: Option<std::collections::BTreeMap<Option<O>, O>>,
}

impl<O: Ord + Copy> MapMates<O> {
    /// `GetOtherMate`.
    fn other(&self, brep_entity: Option<O>) -> Option<O> {
        self.b_to_o.as_ref()?.get(&brep_entity?).copied().flatten()
    }

    /// `RemoveRelationship`: `SM_ERR` unless the two are a mated pair.
    fn remove(&mut self, brep_entity: Option<O>, other_entity: Option<O>) -> Result<()> {
        let (Some(b), Some(o)) = (brep_entity, other_entity) else {
            return Err(SolidError::sm_err(OP_REMOVE));
        };
        if self.other(Some(b)) != Some(o) {
            return Err(SolidError::sm_err(OP_REMOVE));
        }
        if let Some(m) = self.b_to_o.as_mut() {
            m.remove(&b);
        }
        if let Some(m) = self.o_to_b.as_mut() {
            m.remove(&Some(o));
        }
        Ok(())
    }
}

impl<O: Ord + Copy> Mates<O> for MapMates<O> {
    fn other_mate(&self, brep_entity: O) -> Option<O> {
        self.other(Some(brep_entity))
    }

    fn remove_relationship(&mut self, brep_entity: O, other_entity: O) -> Result<()> {
        self.remove(Some(brep_entity), Some(other_entity))
    }
}

// ---- unported callees ----------------------------------------------------------------------

// ---- results ---------------------------------------------------------------------------------

/// What [`TopologyIntersector::check_for_squeeze_vertices`] appends to its out-lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqueezeVertices<O> {
    /// `rDeletedV`.
    pub deleted_vertices: Vec<O>,
    /// `rSurvivingV`.
    pub surviving_vertices: Vec<O>,
    /// `rDeletedE`.
    pub deleted_edges: Vec<O>,
}

/// What [`TopologyIntersector::iir_edge_brep`] returns: what its `MergeCurveClasses`
/// deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedTopology<O> {
    /// `bDeletedTopology` (`rbTopologyWasDeleted` of `MergeCurveClasses`).
    pub topology_deleted: bool,
    /// `paDeletedEdges`.
    pub deleted_edges: Vec<O>,
    /// `paDeletedVerts`.
    pub deleted_vertices: Vec<O>,
}

/// What [`TopologyIntersector::fix_gaps`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixGaps<O> {
    /// `rbModifiedTopology`.
    pub modified_topology: bool,
    /// `rDeletedVertices`.
    pub deleted_vertices: Vec<O>,
    /// `rDeletedEdges` (a null entry when a combination deleted no edge, as upstream).
    pub deleted_edges: Vec<Option<O>>,
}

impl<O> Default for SqueezeVertices<O> {
    fn default() -> Self {
        Self {
            deleted_vertices: Vec::new(),
            surviving_vertices: Vec::new(),
            deleted_edges: Vec::new(),
        }
    }
}

impl<O> Default for FixGaps<O> {
    fn default() -> Self {
        Self {
            modified_topology: false,
            deleted_vertices: Vec::new(),
            deleted_edges: Vec::new(),
        }
    }
}

/// What [`TopologyIntersector::edge_edge_intersect`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeEdgeIntersect<O> {
    /// `rSplitEdges1`.
    pub split_edges1: Vec<O>,
    /// `rNewEdges1` (a null entry where `MakeVertexSplitEdge` returned no edge, as
    /// upstream's list).
    pub new_edges1: Vec<Option<O>>,
    /// `rSplitEdges2`.
    pub split_edges2: Vec<O>,
    /// `rNewEdges2` (null entries as in `new_edges1`).
    pub new_edges2: Vec<Option<O>>,
    /// `rNewVerts`.
    pub new_verts: Vec<O>,
    /// `rdMaxEdgeGap`.
    pub max_edge_gap: f64,
    /// `rdMinUnstitchedEdgeGap`.
    pub min_unstitched_edge_gap: f64,
}

/// What [`TopologyIntersector::edge_face_coincidence`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeFaceCoincidence<O> {
    /// `rImprintedEdges`.
    pub imprinted_edges: Vec<O>,
    /// `rProcessedFaces`.
    pub processed_faces: Vec<O>,
    /// `rNewFaces`.
    pub new_faces: Vec<O>,
    /// `rSplitFaces` (in step with `new_faces`).
    pub split_faces: Vec<O>,
    /// `dMaxEdgeGap` (never set upstream: 0).
    pub max_edge_gap: f64,
    /// `dThisMinUnstitched` (never set upstream: `SM_BIG_DOUBLE`).
    pub this_min_unstitched: f64,
}

/// The tolerance of a vertex, edge or face handle (`GetTolerance`).
fn tol_of<O: Copy>(brep: &impl IntersectorBrep<O>, topo: O) -> Result<f64> {
    brep.topology_tolerance(topo)
        .ok_or(invalid(OP, "topology handle is unknown"))
}

/// The curve behind a handle (a null dereference upstream for none).
fn curve_of<O: Copy>(geometry: &impl ClassGeometry<O>, curve: Option<O>) -> Result<&Curve> {
    curve
        .and_then(|c| geometry.curve(c))
        .ok_or(invalid(OP, "curve handle cannot be resolved"))
}

impl<O: ClassObject + SolutionObject + Ord> TopologyIntersector<O> {
    // ---- the intersection table ------------------------------------------------------

    /// Register a known empty surface intersection without a geometry backend.
    /// This is the empty-array branch of [`Self::register_existing_ssi`].
    pub fn register_empty_ssi(&mut self, brep_surface: Option<O>, other_surface: Option<O>) {
        let table = self.intersection_table.get_or_insert_with(Default::default);
        table.brep_surfaces.push(brep_surface);
        table.other_surfaces.push(other_surface);
        table.results.push(IntersectionResults::default());
    }

    /// `RegisterExistingSSI(pBrepSurface, pOtherSurface, cr3DCurves, crUVCurves1,
    /// crUVCurves2, crCurveTypes, crDeviations)`: adds a table entry (making the table
    /// when there is none) with copies of the curves (each 3D curve must be a B-spline
    /// curve; UV curves that are not are kept as given). A null surface turns off
    /// every non-registered intersection of the other one.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a 3D curve that is not a B-spline curve (or null): the surfaces are
    /// already added and the entry's results are not, as upstream (the table's lists
    /// then differ in length); [`SolidError::InvalidInput`] for a UV list shorter than
    /// the 3D list (upstream reads past it) or an unresolvable handle.
    #[allow(clippy::too_many_arguments)]
    pub fn register_existing_ssi(
        &mut self,
        geometry: &mut impl IntersectorBrep<O>,
        brep_surface: Option<O>,
        other_surface: Option<O>,
        curves_3d: &[Option<O>],
        uv_curves1: &[Option<O>],
        uv_curves2: &[Option<O>],
        curve_types: &[TsectCurveType],
        deviations: &[f64],
    ) -> Result<()> {
        if curves_3d.is_empty() {
            self.register_empty_ssi(brep_surface, other_surface);
            return Ok(());
        }
        let table = self.intersection_table.get_or_insert_with(Default::default);
        table.brep_surfaces.push(brep_surface);
        table.other_surfaces.push(other_surface);
        let mut result = IntersectionResults::default();
        if !curves_3d.is_empty() {
            let mut c = IntersectionCurves {
                curves_3d: curves_3d.to_vec(),
                uv_curves1: uv_curves1.to_vec(),
                uv_curves2: uv_curves2.to_vec(),
                curve_types: curve_types.to_vec(),
                deviations: deviations.to_vec(),
            };

            for ii in 0..c.curves_3d.len() {
                let c3 = c.curves_3d.get(ii).copied().flatten();
                let copy =
                    Self::b_spline_copy(geometry, c3)?.ok_or(SolidError::sm_err(OP_REGISTER))?;
                if let Some(slot) = c.curves_3d.get_mut(ii) {
                    *slot = Some(copy);
                }
                for uv in [&mut c.uv_curves1, &mut c.uv_curves2] {
                    let slot = uv.get_mut(ii).ok_or(invalid(
                        OP_REGISTER,
                        "UV curve list shorter than the 3D list",
                    ))?;
                    if let Some(h) = Self::b_spline_copy(geometry, *slot)? {
                        *slot = Some(h);
                    }
                }
            }
            result.curves = Some(c);
        }

        if let Some(table) = self.intersection_table.as_mut() {
            table.results.push(result);
        }
        Ok(())
    }

    // ---- the mate maps -------------------------------------------------------------------

    /// `RemoveRelationship(pBrepEntity, pOtherEntity)`: unmates the two objects.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when they are not a mated pair (also when either is null or the
    /// intersector has no maps).
    pub fn remove_relationship(
        &mut self,
        brep_entity: Option<O>,
        other_entity: Option<O>,
    ) -> Result<()> {
        if !self.is_mated_pair(brep_entity, other_entity) {
            return Err(SolidError::sm_err(OP_REMOVE));
        }
        if let (Some(m), Some(b)) = (self.b_to_o.as_mut(), brep_entity) {
            m.remove(&b);
        }
        if let Some(m) = self.o_to_b.as_mut() {
            m.remove(&other_entity);
        }
        Ok(())
    }

    /// A mate that upstream casts to `SmVertex*`: [`SolidError::InvalidInput`] when
    /// it is not a vertex (upstream calls vertex members on it).
    fn as_vertex(mate: Option<O>) -> Result<Option<O>> {
        match mate {
            Some(m) if m.class_kind() != ClassObjectKind::Vertex => {
                Err(invalid(OP, "vertex mated to a non-vertex"))
            }
            m => Ok(m),
        }
    }

    /// `CheckForSqueezeVertices(pBrepVertex, pOtherVertex, ..)`: when either vertex is
    /// already mated to a different vertex, that relationship is removed and the
    /// differing pair is combined in its B-rep (the other B-rep for the other vertex's
    /// old mate, the B-rep for the B-rep vertex's), unless the existing relationship is
    /// closer. Returns what upstream appends to its out-lists.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when an existing relationship is better (closer by more than
    /// `SM_EFF_ZERO_SQRT`); those of `RemoveRelationship` and
    /// [`IntersectorBrepP2::brep_combine_coincident_vertices`];
    /// [`SolidError::InvalidInput`] for an unknown vertex, a mate that is not a vertex
    /// or a missing B-rep.
    pub fn check_for_squeeze_vertices(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP2<O>,
        brep_vertex: O,
        other_vertex: O,
    ) -> Result<SqueezeVertices<O>> {
        let mut out = SqueezeVertices::default();
        self.check_for_squeeze_vertices_into(k, brep, brep_vertex, other_vertex, &mut out)?;
        Ok(out)
    }

    /// [`TopologyIntersector::check_for_squeeze_vertices`] appending to the caller's
    /// lists, as upstream's out-params: what was appended before a failure stays
    /// appended (the second conflict check can fail after the first one appended;
    /// `MergeCurveClasses` passes the same lists on every call and keeps going).
    ///
    /// # Errors
    ///
    /// As [`TopologyIntersector::check_for_squeeze_vertices`].
    pub fn check_for_squeeze_vertices_into(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP2<O>,
        brep_vertex: O,
        other_vertex: O,
        out: &mut SqueezeVertices<O>,
    ) -> Result<()> {
        // (the cbi breakpoints read both tolerances; SER(SM_SUCCESS) does nothing)
        tol_of(brep, brep_vertex)?;
        tol_of(brep, other_vertex)?;

        // Get any already-existing vertex relationships.
        let other_v = Self::as_vertex(self.other_mate(Some(brep_vertex)))?;
        // (read now, cast where the second block dereferences it: the first block can
        // fail before)
        let brep_v = self.brep_mate(Some(other_vertex));

        // Check for a pre-existing, conflicting relationship.
        if let Some(other_v) = other_v.filter(|&v| v != other_vertex) {
            tol_of(brep, other_v)?;

            // If the existing relationship is better than this would be, don't do it.
            let p = brep.vertex_point(brep_vertex)?;
            let dist_old = p.distance(brep.vertex_point(other_v)?);
            let dist_new = p.distance(brep.vertex_point(other_vertex)?);
            if dist_new > dist_old + EFF_ZERO_SQRT {
                return Err(SolidError::sm_err(OP_SQUEEZE));
            }

            self.remove_relationship(Some(brep_vertex), Some(other_v))?;

            // Combine two vertices which are coincident or nearly so
            let other = self.other.ok_or(invalid(OP_SQUEEZE, "no other B-rep"))?;
            let deleted_other_edge =
                brep.brep_combine_coincident_vertices(k, other, other_vertex, other_v)?;
            if let Some(e) = deleted_other_edge {
                out.deleted_edges.push(e);
                if let Some(b) = self.brep_mate(Some(e)) {
                    self.remove_relationship(Some(b), Some(e))?;
                }
            }

            // remember which vertices are deleted and which survive
            out.surviving_vertices.push(other_vertex);
            out.deleted_vertices.push(other_v);
        }

        // Check for a pre-existing, conflicting relationship.
        if let Some(brep_v) = brep_v.filter(|&v| v != brep_vertex) {
            let brep_v =
                Self::as_vertex(Some(brep_v))?.ok_or(invalid(OP_SQUEEZE, "no B-rep mate"))?;
            tol_of(brep, brep_v)?;

            let p = brep.vertex_point(other_vertex)?;
            let dist_old = p.distance(brep.vertex_point(brep_v)?);
            let dist_new = p.distance(brep.vertex_point(brep_vertex)?);
            if dist_new > dist_old + EFF_ZERO_SQRT {
                return Err(SolidError::sm_err(OP_SQUEEZE));
            }

            self.remove_relationship(Some(brep_v), Some(other_vertex))?;

            let this = self.brep.ok_or(invalid(OP_SQUEEZE, "no B-rep"))?;
            let deleted_brep_edge =
                brep.brep_combine_coincident_vertices(k, this, brep_vertex, brep_v)?;
            if let Some(e) = deleted_brep_edge {
                out.deleted_edges.push(e);
                if let Some(o) = self.other_mate(Some(e)) {
                    self.remove_relationship(Some(e), Some(o))?;
                }
            }

            // remember which vertex survives and which is deleted
            out.surviving_vertices.push(brep_vertex);
            out.deleted_vertices.push(brep_v);
        }

        Ok(())
    }

    /// `DeleteFaces(crFaces, pOptKeepAsWires)`: deletes the faces from their B-rep
    /// (unmating them first), then every edge of theirs that became a wire (unless
    /// it is in `opt_keep_as_wires`) and every vertex that became a shell vertex,
    /// unmating each first. When the infinite region merges it is kept.
    ///
    /// # Errors
    ///
    /// Those of `SmBrep::DeleteFace` / `DeleteEdge` / `DeleteVertex`;
    /// [`SolidError::InvalidInput`] for an unknown handle or an object without B-rep.
    pub fn delete_faces(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
        faces: &[O],
        opt_keep_as_wires: Option<&[O]>,
    ) -> Result<()> {
        let mut faces_vertices: Vec<O> = Vec::new();
        let mut faces_edges: Vec<O> = Vec::new();
        let mut face_brep_h: Option<O> = None;

        // iter every face saving all face vertices and edges and calling DeleteFace()
        for &f in faces {
            // Accumulate DeletedFace unique EdgeList for later
            for e in brep.face_edges(f)? {
                add_unique(&mut faces_edges, e);
            }

            // Accumulate DeletedFace unique VertexList for later
            for v in brep.face_vertices(f)? {
                add_unique(&mut faces_vertices, v);
            }

            // remove TargetFace<->OtherFace relationship if it exists (status unchecked)
            if let Some(o_face) = self.other_mate(Some(f)) {
                ignore_soft(self.remove_relationship(Some(f), Some(o_face)))?;
            }

            // TargetFace->Brep - to edit
            face_brep_h = brep.face_brep(f)?;
            let b = face_brep_h.ok_or(invalid(OP_DELETE_FACES, "face without B-rep"))?;

            // delete TargetFace from its Brep (FALSE = don't delete dangling edges and
            // vertices, we do that below in accordance with pOptKeepAsWires; the rest
            // are DeleteFace's defaults: IntersectorBrep::brep_delete_face)
            brep.brep_delete_face(k, b, f)?;
        }

        if face_brep_h.is_none() {
            return Ok(());
        }

        // iter every edge of every deleted face looking for wire edges to delete
        for &e in &faces_edges {
            if !brep.edge_is_wire(e)? {
                continue;
            }
            // save wires when asked
            if opt_keep_as_wires.is_some_and(|keep| contains(keep, &e)) {
                continue;
            }

            // remove any TargetEdge<->OtherEdge relationships
            if let Some(o_e) = self.other_mate(Some(e)) {
                ignore_soft(self.remove_relationship(Some(e), Some(o_e)))?;
            }

            // delete the Edge from the Brep
            let b = brep
                .edge_brep_handle(e)?
                .ok_or(invalid(OP_DELETE_FACES, "edge without B-rep"))?;
            brep.brep_delete_edge(k, b, e)?;
        }

        // iter every vertex of every deleted face looking for ShellVertices to delete
        for &v in &faces_vertices {
            if !brep.vertex_is_shell_vertex(v)? {
                continue;
            }
            if let Some(o_v) = self.other_mate(Some(v)) {
                ignore_soft(self.remove_relationship(Some(v), Some(o_v)))?;
            }
            let b = brep
                .vertex_brep(v)?
                .ok_or(invalid(OP_DELETE_FACES, "vertex without B-rep"))?;
            brep.brep_delete_vertex(k, b, v)?;
        }

        Ok(())
    }

    /// `FixGaps(crSingleVertices, ..)`: for each pair of the (B-rep) vertices closer
    /// than 1.01 times the sum of their tolerances and without a common edge longer
    /// than that, combines them in the B-rep and their mates in the other B-rep and
    /// drops the first vertex's (and the deleted edges') relationships.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when a candidate vertex has no mate; those of
    /// [`IntersectorBrepP2::brep_combine_coincident_vertices`] and of the edge length;
    /// [`SolidError::InvalidInput`] for an unknown handle, an edge without curve, a mate
    /// that is not a vertex or a missing B-rep.
    #[allow(clippy::neg_cmp_op_on_partial_ord)] // upstream's NaN-safe test
    pub fn fix_gaps(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP2<O>,
        single_vertices: &[O],
    ) -> Result<FixGaps<O>> {
        let mut out = FixGaps::default();
        let num_single_verts = single_vertices.len();

        // Look for gaps bigger than the sum of the two vertices' tolerances.
        for ii in 0..num_single_verts {
            let v1 = *single_vertices
                .get(ii)
                .ok_or(invalid(OP_FIX_GAPS, "vertex list"))?;
            for jj in ii + 1..num_single_verts {
                let v2 = *single_vertices
                    .get(jj)
                    .ok_or(invalid(OP_FIX_GAPS, "vertex list"))?;

                // Increase tol just a bit.  Was 10.0.  [B498]
                let tol_factor = 1.01;
                let tol = tol_factor * (tol_of(brep, v1)? + tol_of(brep, v2)?);

                // look for a close enough pairing (a NaN distance or tolerance is none)
                if !(brep.vertex_point(v1)?.distance(brep.vertex_point(v2)?) < tol) {
                    continue;
                }

                // Make sure the vertices do not have a common edge greater than
                // tolerance length
                let edges1 = brep.vertex_edges(v1)?;
                let edges2 = brep.vertex_edges(v2)?;
                let mut found_common_edge = false;
                for &test in &edges1 {
                    if contains(&edges2, &test) {
                        let curve = brep
                            .edge_curve(test)
                            .ok_or(invalid(OP_FIX_GAPS, "edge handle is unknown"))?;
                        let ivl = brep
                            .edge_interval(test)
                            .ok_or(invalid(OP_FIX_GAPS, "edge handle is unknown"))?;
                        let edge_leng = curve_of(brep, curve)?
                            .approximate_length(k, &ivl, 10, false)?
                            .0;
                        if edge_leng > tol {
                            found_common_edge = true;
                        }
                    }
                }

                // when no largerThanTol common edge is found between V1 and V2
                if !found_common_edge {
                    // Have a good candidate for squeezing
                    let other_v1 = self
                        .other_mate(Some(v1))
                        .ok_or(SolidError::null_pointer(OP_FIX_GAPS))?;
                    let other_v2 = self
                        .other_mate(Some(v2))
                        .ok_or(SolidError::null_pointer(OP_FIX_GAPS))?;
                    // (the casts to SmVertex* are used by CombineCoincidentVertices)
                    Self::as_vertex(Some(other_v1))?;
                    Self::as_vertex(Some(other_v2))?;

                    // combine coincident vertices
                    let this = self.brep.ok_or(invalid(OP_FIX_GAPS, "no B-rep"))?;
                    let deleted_brep_edge =
                        brep.brep_combine_coincident_vertices(k, this, v2, v1)?;
                    out.deleted_vertices.push(v1);
                    out.deleted_edges.push(deleted_brep_edge);

                    let other = self.other.ok_or(invalid(OP_FIX_GAPS, "no other B-rep"))?;
                    let deleted_other_edge =
                        brep.brep_combine_coincident_vertices(k, other, other_v2, other_v1)?;
                    ignore_soft(self.remove_relationship(Some(v1), Some(other_v1)))?;
                    self.forget_deleted_edge_relationships(deleted_brep_edge, deleted_other_edge)?;
                    out.modified_topology = true;
                    break; // V1 has gone away
                }
            }
        }

        Ok(out)
    }

    /// Removes deleted edges from both mate maps without unpairing unrelated edges.
    fn forget_deleted_edge_relationships(
        &mut self,
        deleted_brep_edge: Option<O>,
        deleted_other_edge: Option<O>,
    ) -> Result<()> {
        if let (Some(db), Some(dother)) = (deleted_brep_edge, deleted_other_edge) {
            if self.is_mated_pair(Some(db), Some(dother)) {
                ignore_soft(self.remove_relationship(Some(db), Some(dother)))?;
                return Ok(());
            }
        }
        // source change (issue #432): either combination may delete an edge
        // independently. Only remove actual mates, keeping both maps in sync [B460].
        if let Some(db) = deleted_brep_edge {
            if let Some(mate) = self.other_mate(Some(db)) {
                ignore_soft(self.remove_relationship(Some(db), Some(mate)))?;
            }
        }
        if let Some(dother) = deleted_other_edge {
            if let Some(mate) = self.brep_mate(Some(dother)) {
                ignore_soft(self.remove_relationship(Some(mate), Some(dother)))?;
            }
        }
        Ok(())
    }

    // ---- closing intersection loops (co-ported) ----------------------------------------

    /// `IIREdgeBrep(pEdge, pBrep, bSwapOrder, bForceFaceIntersections, paDeletedEdges,
    /// paDeletedVerts)`: classifies `edge` to itself and to `target` (the B-rep the
    /// edge's curve is intersected with, its editing disabled meanwhile), then merges
    /// the two classifications into their B-reps (`MergeCurveClasses` with
    /// `bMergeSingularities`, the B-rep classification first unless `swap_order`).
    /// `force_face_intersections` intersects a wire edge with the faces too (every
    /// upstream call passes `TRUE`). Sets `topology_deleted` when the merge deleted
    /// topology. Returns the deleted edges and vertices (`paDeletedEdges` /
    /// `paDeletedVerts`, filled by the merge). Unowned in `units.json`; co-ported here
    /// for [`TopologyIntersector::close_intersection_loop`], its only caller.
    ///
    /// # Errors
    ///
    /// Those of `sm_SetEdgeClass`, `BrepCurveSolve` and `InsertTopologyIntersections`;
    /// Those of `MergeCurveClasses` ([`TopologyIntersector::merge_curve_classes`]);
    /// [`SolidError::InvalidInput`] for an unknown edge or one without curve.
    pub fn iir_edge_brep(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        edge: O,
        target: O,
        swap_order: bool,
        force_face_intersections: bool,
    ) -> Result<DeletedTopology<O>> {
        let curve = brep
            .edge_curve(edge)
            .ok_or(invalid(OP_IIR_EDGE, "edge handle is unknown"))?
            .ok_or(invalid(OP_IIR_EDGE, "edge without curve"))?;
        let interval = brep
            .edge_interval(edge)
            .ok_or(invalid(OP_IIR_EDGE, "edge handle is unknown"))?;
        let tolerance = tol_of(brep, edge)?;

        // construct 1st CurveClassification to map curve to pEdge
        let mut crv_class1 = new_curve_classification(k, curve, &interval, None, tolerance)?;

        // init CurveClassification to classify to pEdge
        // 1st Interval = [m_vStart = SM_PC_VERTEX, m_vMid = SM_PC_EDGE, m_vEnd = SM_PC_VERTEX]
        set_edge_class(k, &*brep, &mut crv_class1, edge)?;

        // select SolverType - handle wires differently unless bForceFaceIntersections == TRUE
        let solver_op = if force_face_intersections || !brep.edge_is_wire(edge)? {
            SolverOperation::Intersect
        } else {
            SolverOperation::IntersectWireframe
        };

        // construct 2nd CurveClassification object to map curve to Brep
        let mut crv_class2 = new_curve_classification(k, curve, &interval, None, tolerance)?;

        // (upstream iterates once: InsertTopologyIntersections no longer reshapes the
        // curve)
        crv_class2.reset(k, &ResetOptions::default())?;

        // temporarily disable EditingEnabled bit; Intersect Edge->Curve with Brep
        let old_edit = brep.set_editing_enabled(target, false)?;
        let solved = solve_with_face_caches(k, brep, |objects| {
            let curve_ref = CurveRef {
                curve: objects.curve(curve)?,
                handle: curve,
            };
            TopologySolver::brep_curve_solve(
                k,
                objects,
                Some(target),
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
        brep.set_editing_enabled(target, old_edit)?;
        let solutions = solved?;

        // add curve/Brep intersections to CrvClass2
        cc_insert_topology_intersections(k, &*brep, &mut crv_class2, &solutions)?;

        // merge curveClassifications into Breps
        let merge = if swap_order {
            self.merge_curve_classes(k, brep, &mut crv_class2, &mut crv_class1, true)?
        } else {
            self.merge_curve_classes(k, brep, &mut crv_class1, &mut crv_class2, true)?
        };
        let merged = DeletedTopology {
            topology_deleted: merge.topology_was_deleted,
            deleted_edges: merge.deleted_edges.unwrap_or_default(),
            deleted_vertices: merge.deleted_vertices.unwrap_or_default(),
        };

        // set output values and return
        if merged.topology_deleted {
            self.topology_deleted = true;
        }
        Ok(merged)
    }

    /// `CloseIntersectionLoop(pBrepVertex, rbMadeNewTopology, rDeletedVertices,
    /// rDeletedEdges)`: tries to close a gap in
    /// the intersection loop at a B-rep vertex mated to an other-B-rep vertex. For each
    /// unmated edge of the vertex whose far vertex is mated: when the two mated
    /// vertices of the other B-rep share exactly one edge and the edge's midpoint drops
    /// onto it (within twice the larger mated-vertex distance), the two edges are
    /// related; else, when the edge's curve is coincident with the other B-rep
    /// (`BrepCurveSolve` gives a range of values), it is intersected, inserted and
    /// related there ([`TopologyIntersector::iir_edge_brep`]). Without new topology in
    /// the other B-rep, the same is tried the other way round for the other vertex's
    /// unmated edges. Returns `rbMadeNewTopology`; `deleted_vertices` and
    /// `deleted_edges` are reset once the vertex and its mate are found, then receive
    /// the vertices and edges (of either B-rep) each completed
    /// [`TopologyIntersector::iir_edge_brep`] deleted by squeezing, so they hold the
    /// completed calls' deletions also when a later call fails. Unowned in `units.json`
    /// (called from `SmMerge.cpp:2961`); co-ported into this part.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the vertex is not mated to a vertex; those of
    /// `NormalizedEvaluate`, `DropPoint`, `BrepCurveSolve` and
    /// [`TopologyIntersector::iir_edge_brep`]; [`SolidError::InvalidInput`] for an
    /// unknown handle, an edge without curve or a missing B-rep.
    pub fn close_intersection_loop(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        brep_vertex: Option<O>,
        deleted_vertices: &mut Vec<O>,
        deleted_edges: &mut Vec<O>,
    ) -> Result<bool> {
        // check input
        let brep_vertex = brep_vertex.ok_or(SolidError::null_pointer(OP_CLOSE_LOOP))?;

        // find mating vertex in m_pOther brep
        let other_vertex = Self::cast(self.other_mate(Some(brep_vertex)), ClassObjectKind::Vertex)
            .ok_or(SolidError::null_pointer(OP_CLOSE_LOOP))?;

        // init output
        let mut made_new_topology = false;
        deleted_vertices.clear();
        deleted_edges.clear();

        // get all edges connected to target vertex
        let brep_edges = brep.vertex_edges(brep_vertex)?;

        // for every edge connected to target vertex
        for be in brep_edges {
            // target edge and any mated edge from m_pOther brep
            let oe = Self::cast(self.other_mate(Some(be)), ClassObjectKind::Edge);

            // when there is no mated edge
            if oe.is_some() {
                continue;
            }
            // get target edge's other vertex and see if that vertex is mated
            let b_vertex2 = brep.edge_other_vertex(be, Some(brep_vertex))?;
            let o_vertex2 = Self::cast(self.other_mate(b_vertex2), ClassObjectKind::Vertex);

            // skip closed curves
            if b_vertex2 == Some(brep_vertex) {
                continue;
            }

            // quit if other edge vertex is not mated
            let Some(o_vertex2) = o_vertex2 else {
                continue;
            };
            let b_vertex2 = b_vertex2.ok_or(invalid(OP_CLOSE_LOOP, "edge without vertex"))?;

            // get all edges connected to edge's othervertex mated partner
            let o_common_edges = brep.vertex_common_edges(other_vertex, o_vertex2)?;

            // When only one edge connects to the vertex
            if let [o_edge] = o_common_edges[..] {
                // We might be able to solve the problem right here.

                // distances between mated pairs of vertices
                let dist1 = brep
                    .vertex_point(brep_vertex)?
                    .distance(brep.vertex_point(other_vertex)?);
                let dist2 = brep
                    .vertex_point(b_vertex2)?
                    .distance(brep.vertex_point(o_vertex2)?);
                let max_dist = max2(dist1, dist2);

                // midPoint of this m_pBrep target edge
                let mid_pnt = brep.edge_normalized_evaluate(k, be, 0.5)?;

                // drop midPoint to sole other m_pOther Brep edge connected to mated vertex
                let ivl = brep
                    .edge_interval(o_edge)
                    .ok_or(invalid(OP_CLOSE_LOOP, "edge handle is unknown"))?;
                let curve = brep
                    .edge_curve(o_edge)
                    .ok_or(invalid(OP_CLOSE_LOOP, "edge handle is unknown"))?;
                let dropped = curve_of(&*brep, curve)?.drop_point(
                    k,
                    &ivl,
                    mid_pnt,
                    None,
                    max_dist * 2.0,
                    None,
                    SolverOperation::Minimize,
                )?;

                // when point dropped to curve within tolerance
                if dropped.success {
                    // add the edge pair to the list of mated object
                    // thus filling a potential intersection loop gap
                    // (status unchecked)
                    ignore_soft(self.relate(k, brep, Some(be), Some(o_edge)))?;
                    return Ok(true);
                }
            }

            // intersect the m_pOTher Brep with the target edge
            let other = self.other.ok_or(invalid(OP_CLOSE_LOOP, "no other B-rep"))?;
            if Self::edge_coincident_with_brep(k, brep, be, other)? {
                let old_edges = brep.brep_edges(other)?;

                // intersect, insert, and relate target Edge with m_pOther brep
                let deleted = self.iir_edge_brep(k, brep, be, other, false, true)?;
                deleted_vertices.extend(deleted.deleted_vertices);
                deleted_edges.extend(deleted.deleted_edges);

                // remember if  new edges were added to m_pOther
                let new_edges = brep.brep_edges(other)?;
                if new_edges.len() > old_edges.len() {
                    made_new_topology = true;
                }
                break;
            }
        }

        // when no topology was added to m_pOther - see if other edges can be added to this
        // m_pBrep
        if !made_new_topology {
            // get all edges connected to other vertex
            let other_edges = brep.vertex_edges(other_vertex)?;

            // for every edge connected to otherVertex
            for oe in other_edges {
                // get target edge and any mated edge within m_pBrep
                let be = Self::cast(self.brep_mate(Some(oe)), ClassObjectKind::Edge);

                // when target edge is not mated to a m_pBrep edge
                if be.is_some() {
                    continue;
                }
                // get targetEdge->OtherVertex
                let o_vertex2 = brep.edge_other_vertex(oe, Some(other_vertex))?;

                // skip closed curves
                if o_vertex2 == Some(other_vertex) {
                    continue;
                }

                // skip cases where OtherVertex is not mated to a m_pBrep Vertex
                if Self::cast(self.brep_mate(o_vertex2), ClassObjectKind::Vertex).is_none() {
                    continue;
                }

                // intersect target edge with m_pBrep
                let this = self.brep.ok_or(invalid(OP_CLOSE_LOOP, "no B-rep"))?;
                if Self::edge_coincident_with_brep(k, brep, oe, this)? {
                    let old_edges = brep.brep_edges(this)?;

                    // intersect, insert, and relate target edge into this  m_pBrep
                    let deleted = self.iir_edge_brep(k, brep, oe, this, true, true)?;
                    deleted_vertices.extend(deleted.deleted_vertices);
                    deleted_edges.extend(deleted.deleted_edges);

                    // remember if new topology was added to m_pBrep
                    let new_edges = brep.brep_edges(this)?;
                    if new_edges.len() > old_edges.len() {
                        made_new_topology = true;
                    }
                    break;
                }
            }
        }

        // all done
        Ok(made_new_topology)
    }

    /// `SM_CAST_PTR`: the mate when it is of `kind`, else null.
    fn cast(mate: Option<O>, kind: ClassObjectKind) -> Option<O> {
        mate.filter(|m| m.class_kind() == kind)
    }

    /// `CloseIntersectionLoop`'s coincidence test: `BrepCurveSolve(pBrep, *pEdge->GetCurve(),
    /// pEdge->GetInterval(), SM_SO_INTERSECT, SM_SR_ALL, pEdge->GetTolerance(),
    /// SM_BIG_DOUBLE, NULL, ..)` with `brep`'s editing disabled, and whether a solution is a
    /// range of values.
    fn edge_coincident_with_brep(
        k: &Kernel,
        geometry: &mut impl IntersectorBrepP2<O>,
        edge: O,
        brep: O,
    ) -> Result<bool> {
        let curve = geometry
            .edge_curve(edge)
            .ok_or(invalid(OP_CLOSE_LOOP, "edge handle is unknown"))?
            .ok_or(invalid(OP_CLOSE_LOOP, "edge without curve"))?;
        let interval = geometry
            .edge_interval(edge)
            .ok_or(invalid(OP_CLOSE_LOOP, "edge handle is unknown"))?;
        let tolerance = tol_of(geometry, edge)?;
        let old_edit = geometry.set_editing_enabled(brep, false)?;
        let solved = solve_with_face_caches(k, geometry, |objects| {
            let curve_ref = CurveRef {
                curve: objects.curve(curve)?,
                handle: curve,
            };
            TopologySolver::brep_curve_solve(
                k,
                objects,
                Some(brep),
                curve_ref,
                &interval,
                TopologySolveOptions {
                    operation: SolverOperation::Intersect,
                    requested: SolutionRequested::All,
                    tolerance_3d: tolerance,
                    best_answer_so_far_sq: BIG_DOUBLE,
                    opt_vectors: &[],
                },
            )
        });
        geometry.set_editing_enabled(brep, old_edit)?;

        // check every solution - for a coincident solution
        Ok(solved?
            .as_slice()
            .iter()
            .any(|sol| sol.solution_type == SolutionType::RangeOfValues))
    }

    // ---- intersect, insert, relate ----------------------------------------------------

    /// `IntersectInsertRelate()`: makes the proto topology manager (capture distance
    /// 0.01, at least 2000 times the approximation tolerance), intersects the
    /// topology, resolves and imprints the intersections, sets the relationships and
    /// deletes the manager. `ti` is the handle the manager stores for this
    /// intersector. Returns the curve handles deleted.
    ///
    /// # Errors
    ///
    /// Those of the manager's construction, `IntersectTopology` and
    /// `ResolveIntersections` (the manager then stays installed, as upstream leaks
    /// it); every error of the imprint that may have edited a B-rep, after deleting
    /// the manager (source fix af8db37: upstream goes on with the half-imprinted
    /// B-reps, UB-242, which [`Kernel::note_upstream_ub`] records); an imprint failure
    /// that edited nothing only warns, as upstream;
    /// [`SolidError::InvalidInput`] without both B-reps. Where the manager's
    /// destructor would read a freed proto edge upstream (UB-175) the port's defined
    /// teardown goes on and [`Kernel::note_upstream_ub`] records `"UB-175"`, also when
    /// `SetRelationships` or an editing imprint failed (upstream still runs that
    /// teardown); not after a failed `IntersectTopology` or `ResolveIntersections`,
    /// where upstream returns without deleting the manager.
    pub fn intersect_insert_relate(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP2<O>,
        ti: O,
    ) -> Result<Vec<O>> {
        // Capture Distance (see upstream's [B669] note)
        let mut capture_distance = 0.01;

        // Make sure it's not smaller than our tolerance value.
        let topo_int_tol = self.this_approx_tol_3d();
        if capture_distance < topo_int_tol * 2000.0 {
            capture_distance = topo_int_tol * 2000.0;
        }

        let this = self.brep.ok_or(invalid(OP_INSERT_RELATE, "no B-rep"))?;
        let other = self
            .other
            .ok_or(invalid(OP_INSERT_RELATE, "no other B-rep"))?;
        let mut mgr = ProtoTopologyManager::new(
            k,
            (this, brep.brep_arena(this)?),
            (other, brep.brep_arena(other)?),
            ti,
            self.this_approx_tol_3d,
            capture_distance,
            0.0,
        )?;
        // Not upstream unless opted in (`Kernel::fixes.proto_edge_self_loop_on_vertex_degenerate`,
        // WR-214, source issue #480): a self-loop proto edge on a vertex is
        // dropped, not split onto that vertex. Off (the default), upstream's split.
        mgr.set_split_self_loops_on_vertices(!k.fixes.proto_edge_self_loop_on_vertex_degenerate);
        self.proto_topo_mgr = Some(mgr);

        // Intersect Topology; if that didn't work, quit before we mess up the Breps.
        let mut deleted = self.intersect_topology(k, brep)?;

        // Resolve Intersections; if that didn't work, quit before we mess up the Breps.
        deleted.extend(self.resolve_intersections(k, brep)?);

        // Imprint Intersections, now we'll modify the Breps. If that didn't work and a
        // failing imprint edited a Brep, the Breps are partly imprinted and no longer
        // consistent: stop here, so that no caller goes on to relate, classify or
        // measure them (source change, source fork af8db37; upstream only
        // warns and overwrites the status). A failure that edited nothing (an edge
        // definition without a final vertex) leaves the Breps consistent, and upstream
        // goes on.
        let outcome = self
            .imprint_intersections_reporting_edits(k, brep)
            .and_then(|o| {
                if o.failure_edited {
                    o.status.map(|()| o.deleted)
                } else {
                    // If that didn't work, inform the public (SE_MSG: the status is
                    // overwritten below).
                    Ok(o.deleted)
                }
            });
        match outcome {
            Ok(d) => deleted.extend(d),
            Err(e) => {
                // upstream goes on here with the half-imprinted B-reps (UB-242) to
                // `SetRelationships` (which only reads the proto topology) and the same
                // manager teardown, so UB-175 is decided here as below
                if let Some(mut mgr) = self.proto_topo_mgr.take() {
                    if teardown_reads_freed(&mgr) {
                        k.note_upstream_ub("UB-175");
                    }
                    let _ = mgr.destroy();
                }
                k.note_upstream_ub("UB-242");
                return Err(e);
            }
        }

        // Set relationship maps.
        let status = self.set_relationships(k, brep);

        // all done - clean up and exit
        let mut reads_freed = false;
        if let Some(mut mgr) = self.proto_topo_mgr.take() {
            reads_freed = teardown_reads_freed(&mgr);
            deleted.extend(mgr.destroy());
        }
        if reads_freed {
            // Upstream reads a freed proto edge here (UB-175), whatever `SetRelationships`
            // returned (the delete precedes the return); the arena teardown above is
            // defined, so the port goes on with its result and records the condition.
            k.note_upstream_ub("UB-175");
        }
        status?;
        Ok(deleted)
    }

    /// `IntersectTopology()`: empties the manager, then intersects the shell
    /// vertices and wire (and, per `intersect_lamina_edges`, lamina) edges of each
    /// B-rep with the other, then every face pair not registered in the intersection
    /// table: coincident surfaces collect their proto edges into a proto face; each
    /// interval pair of the face intersection's classifications becomes a proto edge
    /// and each single-point intersection that classifies to imprintable topology of
    /// both faces a proto vertex. Finally intersecting proto edges are split and
    /// edge-less proto vertices lying on a proto edge are removed. Returns the curve
    /// handles deleted.
    ///
    /// # Errors
    ///
    /// Those of `CoincidenceCheck`, `SmFace::FaceIntersect` (`SER`) and
    /// [`IntersectorBrepP2::brep_faces`]; [`SolidError::Unported`] for
    /// `FaceIntersect` (`topology.face_p3`), `Point3DClassify` (`query.face_p1`) and
    /// the callees of [`intersect_shell_vertices`] / [`intersect_wire_lamina_edges`];
    /// [`SolidError::InvalidInput`] without a manager or B-reps, or for a face without
    /// surface.
    #[allow(clippy::too_many_lines)]
    pub fn intersect_topology(
        &mut self,
        k: &Kernel,
        brep: &mut impl IntersectorBrepP2<O>,
    ) -> Result<Vec<O>> {
        let approx_tol = self.this_approx_tol_3d;
        let lamina = self.intersect_lamina_edges;
        let this = self
            .brep
            .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "no B-rep"))?;
        let other = self
            .other
            .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "no other B-rep"))?;
        let mut deleted: Vec<O> = Vec::new();

        // Init output
        self.proto_topo_mgr
            .as_mut()
            .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "no proto topology manager"))?
            .reset();

        let faces1 = brep.brep_faces(this)?;
        let faces2 = brep.brep_faces(other)?;

        {
            let mgr = self
                .proto_topo_mgr
                .as_mut()
                .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "no proto topology manager"))?;

            // Process ShellVertex Intersections (statuses unchecked).
            ignore_soft(intersect_shell_vertices(k, brep, this, other, false, mgr))?;
            ignore_soft(intersect_shell_vertices(k, brep, other, this, true, mgr))?;

            // Now do Wire Edges.
            let do_lamina = lamina == 1 || lamina == 2; // 1: both Breps, 2: m_pBrep
            ignore_soft(intersect_wire_lamina_edges(
                k, brep, this, other, do_lamina, false, approx_tol, mgr,
            ))?;
            let do_lamina = lamina == 1 || lamina == 3; // 3: m_pOther
            ignore_soft(intersect_wire_lamina_edges(
                k, brep, other, this, do_lamina, true, approx_tol, mgr,
            ))?;
        }

        // Opt-in (WR-527, #1526/#1527): each face's bounding box, for skipping the pairs
        // below that lie apart. A box that cannot be computed, or holds nothing, rules
        // nothing out.
        let face_boxes = |faces: &[O]| -> Vec<Extent3d> {
            faces
                .iter()
                .map(|&f| match brep.face_calculate_bounding_box(k, f, false) {
                    Ok(b) if !b.has_negative_volume() => b,
                    _ => {
                        let mut b = Extent3d::new();
                        b.set_unbounded();
                        b
                    }
                })
                .collect()
        };
        let skip_apart = k.fixes.boolean_skips_faces_apart;
        let (boxes1, boxes2) = if skip_apart {
            (face_boxes(&faces1), face_boxes(&faces2))
        } else {
            (Vec::new(), Vec::new())
        };
        // (the fork's source fork@84a7998) the other B-rep's faces whose box may
        // lie within the tolerance of a face's box: a superset of the pairs the box test
        // below keeps, in ascending order. The pairs it leaves out reach nothing before
        // that test but lookups without effect (no intersection table is registered).
        let pair_search = (skip_apart && self.intersection_table().is_none())
            .then(|| SpatialIndex::new(boxes2.iter().copied().map(Some)));

        // Finally, the Face-Face intersections.
        for (ii, &f1) in faces1.iter().enumerate() {
            let pairs: Vec<usize> = match (&pair_search, boxes1.get(ii)) {
                (Some(search), Some(box1)) => search.boxes_in_box(box1, self.this_approx_tol_3d()),
                _ => (0..faces2.len()).collect(),
            };
            for jj in pairs {
                let Some(&f2) = faces2.get(jj) else {
                    continue;
                };
                let s1 = brep
                    .face_surface(f1)
                    .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "face handle is unknown"))?;
                let s2 = brep
                    .face_surface(f2)
                    .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "face handle is unknown"))?;

                // See if this surface/surface xsect has been saved (status unchecked;
                // the copies it makes are not deleted upstream).
                let existing = self.check_if_have_existing_ssi(brep, s1, s2)?;
                if existing.have_existing_ssi {
                    continue;
                }

                // No existing SSI, check coincidence or intersection.
                let tol = self.this_approx_tol_3d();

                // Opt-in (WR-527): no work - faces farther apart than the intersection
                // tolerance cannot meet
                if let (Some(box1), Some(box2)) = (boxes1.get(ii), boxes2.get(jj)) {
                    if box1.are_disjoint(box2, tol) {
                        continue;
                    }
                }

                // Check for coincident surfaces -- partial coincidence is ok.
                let surf1 = s1.ok_or(invalid(OP_INTERSECT_TOPOLOGY, "face without surface"))?;
                let surf2 = s2.ok_or(invalid(OP_INTERSECT_TOPOLOGY, "face without surface"))?;
                let (are_coincident, _coincident_distance) =
                    surface_coincidence_check(k, brep, surf1, surf2, tol, false)?;
                let mut coinc_pes = Vec::new();

                let ang_tol_deg = 20.0;
                let mut uv_curves1 = Vec::new();
                let mut uv_curves2 = Vec::new();
                let mut int_pts: Vec<Point3> = Vec::new();
                let mut fi = face_face_intersect_curves(
                    k,
                    brep,
                    f1,
                    f2,
                    tol,
                    ang_tol_deg,
                    Some(&mut uv_curves1),
                    Some(&mut uv_curves2),
                    Some(&mut int_pts),
                    &FaceIntersectOutputs {
                        this_crv_classes: true,
                        other_crv_classes: true,
                        cc_3d_crvs: true,
                        this_cc_uv_crvs: true,
                        other_cc_uv_crvs: true,
                    },
                    Some(are_coincident),
                )?;

                // All of the curves created in FaceIntersect() are copied into new
                // EdgeDefinition objects and deleted here (SmObjsDelete).
                deleted.extend(fi.curves_3d.iter().copied());
                deleted.extend(uv_curves1.iter().copied());
                deleted.extend(uv_curves2.iter().copied());
                for list in &fi.cc_curves {
                    deleted.extend(list.iter().copied());
                }

                // low work - no curve or point XSects to process.
                if fi.curves_3d.is_empty() && int_pts.is_empty() {
                    continue;
                }

                let mgr = self
                    .proto_topo_mgr
                    .as_mut()
                    .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "no proto topology manager"))?;

                // for each CurveClassification pair returned from intersector
                // (SM_ASSERT(sCCs2.GetSize() == lNumCCs) only reports; a shorter
                // second list is read past its end)
                let num_ccs = fi.cc1.len();
                for kk in 0..num_ccs {
                    let (Some(cc1), Some(cc2)) = (fi.cc1.get_mut(kk), fi.cc2.get_mut(kk)) else {
                        return Err(invalid(
                            OP_INTERSECT_TOPOLOGY,
                            "classification lists differ",
                        ));
                    };
                    let num_ivls = cc1.size();

                    // Make sure that all PointClasses can evaluate their positions.
                    ignore_soft(cc1.set_point_class_parameters_mut(k, brep, false))?;
                    ignore_soft(cc2.set_point_class_parameters_mut(k, brep, false))?;

                    for ll in 0..num_ivls {
                        let mut ci1 = *cc1.at(ll)?;
                        let mut ci2 = *cc2.at(ll)?;
                        // (status unchecked)
                        let found = ignore_soft(mgr.find_or_create_proto_edge(
                            k, brep, approx_tol, cc1, &mut ci1, cc2, &mut ci2,
                        ))?;
                        *cc1.at_mut(ll)? = ci1;
                        *cc2.at_mut(ll)? = ci2;

                        // remember pNewPE is part of the trim boundary of this
                        // coincident face pair
                        if let Some((Some(pe), _was_created)) = found {
                            if are_coincident {
                                add_unique(&mut coinc_pes, pe);
                            }
                        }
                    }
                }

                // Process single-point intersections.
                let f1_zone_tol = tol_of(brep, f1)?;
                let f2_zone_tol = tol_of(brep, f2)?;
                for &pt in &int_pts {
                    let mut pc1 = PointClassification::new(f1_zone_tol);
                    let mut pc2 = PointClassification::new(f2_zone_tol);

                    // classify the point against both faces; skip what doesn't classify
                    let st1 =
                        brep.face_point_3d_classify(k, f1, pt, f1_zone_tol, true, &mut pc1, None);
                    let st2 =
                        brep.face_point_3d_classify(k, f2, pt, f2_zone_tol, true, &mut pc2, None);
                    if ignore_soft(st1)?.is_none() {
                        continue;
                    }
                    if ignore_soft(st2)?.is_none() {
                        continue;
                    }

                    // skip what doesn't classify to imprintable topology in both
                    if !can_imprint(pc1.point_class()) {
                        continue;
                    }
                    if !can_imprint(pc2.point_class()) {
                        continue;
                    }

                    // (status unchecked)
                    ignore_soft(mgr.find_or_create_proto_vertex_pcs(k, brep, &mut pc1, &mut pc2))?;
                }

                // If the Faces are coincident, and there is some overlap - create a
                // ProtoFace. (status unchecked)
                if are_coincident && !coinc_pes.is_empty() {
                    if let Some(pf) =
                        ignore_soft(mgr.create_proto_face(Some(f1), Some(f2), &coinc_pes))?
                    {
                        // Relate New ProtoFace to every pNewPF->CoincPE
                        for &pe in &coinc_pes {
                            ignore_soft(mgr.relate_coincident_pf(pe, pf))?;
                        }
                    }
                }
            }
        }

        let mgr = self
            .proto_topo_mgr
            .as_mut()
            .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "no proto topology manager"))?;

        // 1. for every ProtoEdge pair - split ProtoEdges at ProtoEdge/ProtoEdge XSects
        // (the list is re-read after each split: splitting appends proto edges)
        let mut ii = 0usize;
        while ii < mgr.proto_edges().len() {
            let pe1 = *mgr
                .proto_edges()
                .get(ii)
                .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "proto edge list"))?;
            let mut jj = ii + 1;
            while jj < mgr.proto_edges().len() {
                let pe2 = *mgr
                    .proto_edges()
                    .get(jj)
                    .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "proto edge list"))?;
                // (status unchecked)
                if let Some(d) = ignore_soft(mgr.split_intersecting_proto_edges(k, brep, pe1, pe2))?
                {
                    deleted.extend(d);
                }
                jj += 1;
            }
            ii += 1;
        }

        // 2. for every ProtoEdge/ProtoVertex pair - remove edge-less proto vertices
        // coincident with a proto edge (count backward: they are removed from the list)
        let mut ii = mgr.proto_vertices().len();
        while ii > 0 {
            let pv = *mgr
                .proto_vertices()
                .get(ii - 1)
                .ok_or(invalid(OP_INTERSECT_TOPOLOGY, "proto vertex list"))?;
            let pes = mgr.proto_edges().to_vec();
            for pe in pes {
                // a failure is FALSE (the SmBoolean function's error return)
                let removed =
                    ignore_soft(mgr.remove_coincident_no_edge_proto_vertex(k, brep, pe, pv))?
                        .unwrap_or(false);
                if removed {
                    break;
                }
            }
            ii -= 1;
        }

        Ok(deleted)
    }

    /// `SetRelationships()`: relates the final vertices of every vertex definition and
    /// the final edges of every edge definition of the manager (a definition without
    /// both is skipped with a warning; `Relate`'s status is unchecked). Coincident
    /// faces are not related (`sbRelateFaces` is `FALSE`). Always `SM_SUCCESS`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a manager or for deleted definitions;
    /// [`SolidError::Unported`] / [`SolidError::InvalidInput`] from `Relate`.
    pub fn set_relationships(
        &mut self,
        k: &Kernel,
        brep: &mut impl IntersectorBrep<O>,
    ) -> Result<()> {
        let mgr = self
            .proto_topo_mgr
            .as_ref()
            .ok_or(invalid(OP_SET_RELS, "no proto topology manager"))?;

        // (the manager is not changed by Relate: collect the pairs first)
        let mut pairs: Vec<(Option<O>, Option<O>)> = Vec::new();
        for vd in mgr.all_vertex_definitions()? {
            let d = mgr
                .vertex_def(vd)
                .ok_or(invalid(OP_SET_RELS, "vertex definition is deleted"))?;
            pairs.push((d.final_vertex(1), d.final_vertex(2)));
        }
        for ed in mgr.all_edge_definitions()? {
            let d = mgr
                .edge_def(ed)
                .ok_or(invalid(OP_SET_RELS, "edge definition is deleted"))?;
            pairs.push((d.final_edge(1), d.final_edge(2)));
        }

        for (t1, t2) in pairs {
            if t1.is_none() || t2.is_none() {
                // WARN: Brep Topology not set.
                continue;
            }
            ignore_soft(self.relate(k, brep, t1, t2))?;
        }
        Ok(())
    }

    // ---- the classification-pair fix-ups ------------------------------------------------

    /// `FixupClassMates(rCurveClassBrep, rCurveClassOther)`: for every interval of a
    /// pair of homogenised classifications of the same curve: a short open interval
    /// that splits a mated vertex pair is removed (`FixupSplitVertexPair`, and the
    /// loop starts over), then the end and mid point classifications are checked
    /// against the mates (`FixupEndPointClassMates`, `FixupMidPointClassMates`).
    /// Nothing is done when the classifications are of different curves.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for classifications of different sizes (not homogenised); those of
    /// `FixupSplitVertexPair` and `IsClosed`; [`SolidError::InvalidInput`] for a
    /// classification without curve.
    pub fn fixup_class_mates(
        &self,
        k: &Kernel,
        geometry: &impl IntersectorBrep<O>,
        cc_brep: &mut CurveClassification<O>,
        cc_other: &mut CurveClassification<O>,
    ) -> Result<()> {
        let curve_h = cc_brep.curve();
        let curve = curve_of(geometry, curve_h)?;
        let curve_dim = curve.dim();

        // no work - classifications are not for the same curve
        if curve_h != cc_other.curve() {
            return Ok(());
        }

        // error condition - curveClassifications not homogenized
        if cc_brep.size() != cc_other.size() {
            return Err(SolidError::sm_err(OP_CLASS_MATES));
        }

        // for every interval (the index restarts at 0 after a change)
        let mut ii = 0usize;
        while ii < cc_brep.size() {
            let ivl_b = *cc_brep.at(ii)?;
            let ivl_o = *cc_other.at(ii)?;

            // check for short intervals that split matched vertices; skip closed
            // intervals - both ends will often map to mated vertex pairs
            if !curve.is_closed(k, &ivl_b.interval, 0.0)?
                && ((ivl_b.start.point_class() == PointClassType::Vertex
                    && ivl_o.end.point_class() == PointClassType::Vertex
                    && self.is_mated_pair(ivl_b.start.object(), ivl_o.end.object()))
                    || (ivl_b.end.point_class() == PointClassType::Vertex
                        && ivl_o.start.point_class() == PointClassType::Vertex
                        && self.is_mated_pair(ivl_b.end.object(), ivl_o.start.object())))
            {
                let changed = self.fixup_split_vertex_pair(k, geometry, ii, cc_brep, cc_other)?;
                if changed {
                    // Start the iteration over
                    ii = 0;
                    continue;
                }
            }

            let ci_b = cc_brep.at_mut(ii)?;
            let ci_o = cc_other.at_mut(ii)?;

            // check all start points that classify to something
            if ci_b.start.point_class() != PointClassType::Unknown
                && ci_o.start.point_class() != PointClassType::Unknown
            {
                self.fixup_end_point_class_mates(
                    curve_dim,
                    &mut ci_b.start,
                    &mut ci_o.start,
                    curve_h,
                );
            }

            // check all mid points that classify to something
            if ci_b.mid.point_class() != PointClassType::Unknown
                && ci_o.mid.point_class() != PointClassType::Unknown
            {
                self.fixup_mid_point_class_mates(curve_dim, &mut ci_b.mid, &mut ci_o.mid, curve_h);
            }

            // check last end points that classify to something
            let last = cc_brep.size() - 1;
            let ci_b = cc_brep.at_mut(ii)?;
            let ci_o = cc_other.at_mut(ii)?;
            if ii == last
                && ci_b.end.point_class() != PointClassType::Unknown
                && ci_o.end.point_class() != PointClassType::Unknown
            {
                self.fixup_end_point_class_mates(curve_dim, &mut ci_b.end, &mut ci_o.end, curve_h);
            }
            ii += 1;
        }

        Ok(())
    }

    /// `FixupSplitVertexPair(lIntervalIndex, rCurveClassBrep, rCurveClassOther, ..)`:
    /// when the open interval `index` of the two classifications starts at a vertex of
    /// one that is mated to the vertex it ends at in the other, both mated vertices are
    /// dropped onto the curve, the pair's classifications get a common parameter (the
    /// B-rep one when it is five times closer, else the mean) and replace the
    /// neighbours' ends, and the interval is removed from both. Returns whether the
    /// classifications changed.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when neither vertex drops onto the curve; those of `IsClosed`;
    /// [`SolidError::InvalidInput`] for an index past the classifications, a
    /// classification without curve or a vertex object that is not a vertex.
    pub fn fixup_split_vertex_pair(
        &self,
        k: &Kernel,
        geometry: &impl IntersectorBrep<O>,
        index: usize,
        cc_brep: &mut CurveClassification<O>,
        cc_other: &mut CurveClassification<O>,
    ) -> Result<bool> {
        // check state - homogenized classifications (SM_ASSERT only reports)
        let size = cc_brep.size();
        let ivl_b = *cc_brep.at(index)?;
        let ivl_o = *cc_other.at(index)?;
        // (the two source zone tolerances only size classifications that are then
        // overwritten by copies)
        let curve = curve_of(geometry, cc_brep.curve())?;
        let ivl = cc_brep.interval().union(&cc_other.interval());

        // no work - don't fix closed intervals
        if curve.is_closed(k, &ivl_b.interval, 0.0)? {
            return Ok(false);
        }

        // find the pointClassifications with matched vertices
        let (mut pc_b, mut pc_o) = if ivl_b.start.point_class() == PointClassType::Vertex
            && ivl_o.end.point_class() == PointClassType::Vertex
            && self.is_mated_pair(ivl_b.start.object(), ivl_o.end.object())
        {
            // save the startPoint of This and the EndPoint of other
            (ivl_b.start, ivl_o.end)
        } else if ivl_b.end.point_class() == PointClassType::Vertex
            && ivl_o.start.point_class() == PointClassType::Vertex
            && self.is_mated_pair(ivl_b.end.object(), ivl_o.start.object())
        {
            // save the EndPoint of This and the StartPoint of other
            (ivl_b.end, ivl_o.start)
        } else {
            // nothing to clean up
            return Ok(false);
        };

        // drop the mated vertices to the curve to find their best projection points
        let drop = |pc: &PointClassification<O>| -> Result<(bool, f64, f64)> {
            let v = pc.vertex_object().ok_or(invalid(
                OP_SPLIT_PAIR,
                "vertex classification without object",
            ))?;
            if v.class_kind() != ClassObjectKind::Vertex {
                return Err(invalid(
                    OP_SPLIT_PAIR,
                    "vertex classification of a non-vertex",
                ));
            }
            let point = geometry.vertex_point(v)?;
            let guess = pc.t_param();
            let d = ignore_soft(curve.drop_point(
                k,
                &ivl,
                point,
                None,
                10.0 * pc.deviation(),
                Some(guess),
                SolverOperation::Minimize,
            ))?;
            Ok(d.map_or((false, 0.0, 0.0), |d| (d.success, d.parameter, d.distance)))
        };
        let (success_b, drop_param_b, dist_to_curve_b) = drop(&pc_b)?;
        let (success_o, drop_param_o, dist_to_curve_o) = drop(&pc_o)?;

        if !success_b && !success_o {
            return Err(SolidError::sm_err(OP_SPLIT_PAIR));
        }

        // pick a common curve parameter for the saved PointClasses: favor the ThisBrep
        // parameter value when its deviation is small
        let param = if success_b && success_o && dist_to_curve_b < dist_to_curve_o / 5.0 {
            drop_param_b
        } else if success_b && success_o {
            (drop_param_b + drop_param_o) / 2.0
        } else if success_b {
            drop_param_b
        } else {
            drop_param_o
        };

        // update saved point data
        pc_b.set_t_param(param, false);
        let dev_b = if success_b {
            dist_to_curve_b
        } else {
            pc_b.deviation()
        };
        pc_b.set_deviation(dev_b);
        pc_o.set_t_param(param, false);
        let dev_o = if success_o {
            dist_to_curve_o
        } else {
            pc_o.deviation()
        };
        pc_o.set_deviation(dev_o);

        // propagate the persistent PtClassification data to the neighbor boundaries
        if index >= 1 {
            cc_brep.at_mut(index - 1)?.end = pc_b;
            cc_other.at_mut(index - 1)?.end = pc_o;
        }
        if size > 1 && index + 2 <= size {
            cc_brep.at_mut(index + 1)?.start = pc_b;
            // (upstream bounds both by the B-rep classification's size: past the end
            // of a shorter other one the write lands in SmTArray's spare slots, which
            // nothing reads after the RemoveAt below)
            if index + 1 < cc_other.size() {
                cc_other.at_mut(index + 1)?.start = pc_o;
            }
        }

        // remove the specified interval
        cc_brep.remove_at(index)?;
        cc_other.remove_at(index)?;

        Ok(true)
    }

    // ---- the internal-use methods -------------------------------------------------------

    /// `EdgeEdgeIntersect(rInputEdges1, rInputEdges2, ..)`: intersects each edge of
    /// `edges1` with each of `edges2` whose box is within tolerance and that shares no
    /// vertex with it (`sm_TestAndSplitTwoEdges` at the largest of the approximation
    /// tolerance and the two edge tolerances); both edges of a split are appended to
    /// the working lists (the split edge and the new ones) and processed too, and the
    /// tolerances of the edges and new vertices are updated. `making_manifold_solid`
    /// is `NotUsed`.
    ///
    /// # Errors
    ///
    /// Unless `ignore_problems`, a failing split (`SER`); `SM_ERR` past
    /// [`MAX_EDGE_LIST`] edges; those of the splits (`SmBrep::MakeVertexSplitEdge`
    /// through [`IntersectorBrep`], `GlueVertices` of `topology::brep::p5`);
    /// [`SolidError::InvalidInput`] for null handles upstream dereferences.
    #[allow(clippy::too_many_lines)]
    pub fn edge_edge_intersect(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrep<O>,
        input_edges1: &[Option<O>],
        input_edges2: &[Option<O>],
        making_manifold_solid: bool,
        ignore_problems: bool,
    ) -> Result<EdgeEdgeIntersect<O>> {
        let _ = making_manifold_solid;
        // Init outputs.
        let mut out = EdgeEdgeIntersect {
            split_edges1: Vec::new(),
            new_edges1: Vec::new(),
            split_edges2: Vec::new(),
            new_edges2: Vec::new(),
            new_verts: Vec::new(),
            max_edge_gap: 0.0,
            min_unstitched_edge_gap: BIG_DOUBLE,
        };

        if input_edges1.is_empty() || input_edges2.is_empty() {
            return Ok(out);
        }

        // Work with copies of the input lists: split pieces are appended to them.
        let mut edges1: Vec<Option<O>> = input_edges1.to_vec();
        let mut edges2: Vec<Option<O>> = input_edges2.to_vec();

        let mut ii = 0usize;
        while ii < edges1.len() {
            let Some(e1) = edges1.get(ii).copied().flatten() else {
                ii += 1;
                continue;
            };
            // (status unchecked: an initial box on failure)
            let e1_box = ignore_soft(brep.edge_bounding_box(k, e1))?.unwrap_or_default();

            let mut jj = 0usize;
            while jj < edges2.len() {
                let Some(e2) = edges2.get(jj).copied().flatten() else {
                    jj += 1;
                    continue;
                };
                jj += 1;
                let e2_box: Extent3d =
                    ignore_soft(brep.edge_bounding_box(k, e2))?.unwrap_or_default();

                let this_tol = max3(
                    self.this_approx_tol_3d,
                    tol_of(brep, e1)?,
                    tol_of(brep, e2)?,
                );
                if e1_box.are_disjoint(&e2_box, this_tol) {
                    continue;
                }

                // We can also skip edges that share a vertex.
                let v11 = brep.edge_vertex(e1)?;
                let v12 = brep.edge_other_vertex(e1, v11)?;
                let v21 = brep.edge_vertex(e2)?;
                if v21 == v11 || v21 == v12 {
                    continue;
                }
                let v22 = brep.edge_other_vertex(e2, v21)?;
                if v22 == v11 || v22 == v12 {
                    continue;
                }

                // Do the intersection.
                let split = match super::p1::test_and_split_two_edges(k, brep, e1, e2, this_tol) {
                    Ok(s) => s,
                    Err(
                        e @ (SolidError::Unported { .. }
                        | SolidError::InvalidInput { .. }
                        | SolidError::PortInvariant { .. }),
                    ) => return Err(e),
                    Err(e) => {
                        if ignore_problems {
                            continue;
                        }
                        return Err(e);
                    }
                };
                let num_new_v = split.new_verts.len();
                if num_new_v < 1 {
                    continue;
                }
                let dist_to_edge = split.dist_to_edge;

                // We did a split.
                out.min_unstitched_edge_gap =
                    min2(out.min_unstitched_edge_gap, split.min_unstitched);
                out.max_edge_gap = max2(out.max_edge_gap, dist_to_edge);

                // Add the vertices, split edge and the new edge to the output lists.
                out.new_verts.extend_from_slice(&split.new_verts);
                out.new_edges1.extend_from_slice(&split.new_edges1);
                out.new_edges2.extend_from_slice(&split.new_edges2);
                add_unique(&mut out.split_edges1, e1);
                add_unique(&mut out.split_edges2, e2);

                // We also have to add new edges to the lists we're working on.
                edges1.push(Some(e1));
                edges1.extend(split.new_edges1.iter().copied());
                edges2.push(Some(e2));
                edges2.extend(split.new_edges2.iter().copied());
                if edges1.len() > MAX_EDGE_LIST || edges2.len() > MAX_EDGE_LIST {
                    return Err(SolidError::sm_err(OP_EDGE_EDGE));
                }

                // Update tolerances (statuses unchecked).
                ignore_soft(brep.update_object_tolerance(k, e1, dist_to_edge, 0.0, false))?;
                ignore_soft(brep.update_object_tolerance(k, e2, dist_to_edge, 0.0, false))?;
                let check_gaps = dist_to_edge > self.this_approx_tol_3d / 4.0;
                for &v in &split.new_verts {
                    ignore_soft(brep.update_object_tolerance(k, v, dist_to_edge, 0.0, check_gaps))?;
                }
                // (UpdateObjectTolerance of a null topology returns SM_SUCCESS)
                for &e in split.new_edges1.iter().flatten() {
                    let t = tol_of(brep, e1)?;
                    ignore_soft(brep.update_object_tolerance(k, e, dist_to_edge, t, false))?;
                }
                for &e in split.new_edges2.iter().flatten() {
                    let t = tol_of(brep, e2)?;
                    ignore_soft(brep.update_object_tolerance(k, e, dist_to_edge, t, false))?;
                }
            }
            ii += 1;
        }

        Ok(out)
    }

    /// `EdgeFaceCoincidence(rGivenVerts, rExcludedEdges, ..)`: every edge between two
    /// of the given vertices (not excluded) that lies in a face containing both
    /// vertices but not the edge (coincident with the face's surface within the
    /// approximation tolerance, and two of its points classifying onto the face) is
    /// imprinted into the face (`MakeEdgeInFace` with a copy of its curve) and glued
    /// to the new edge.
    ///
    /// # Errors
    ///
    /// Unless `ignore_problems`, a failing glue (`SM_ERR`); those of the topology
    /// walks and `IsClosed`; [`SolidError::Unported`] for `Point3DClassify`
    /// (`query.face_p1`), `MakeEdgeInFace` (`topology.brep_p5`) and
    /// `CalculateUVDomainFromUVTrimCurves`' `CreateUVTrimCurves`
    /// (`topology.face_p3`); [`SolidError::InvalidInput`] for null handles upstream
    /// dereferences; those of [`IntersectorBrep::brep_glue_edges_general`].
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub fn edge_face_coincidence(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP2<O>,
        given_verts: &[Option<O>],
        excluded_edges: &[Option<O>],
        ignore_problems: bool,
        do_region_nesting: bool,
    ) -> Result<EdgeFaceCoincidence<O>> {
        // init outputs
        let mut out = EdgeFaceCoincidence {
            imprinted_edges: Vec::new(),
            processed_faces: Vec::new(),
            new_faces: Vec::new(),
            split_faces: Vec::new(),
            max_edge_gap: 0.0,
            this_min_unstitched: BIG_DOUBLE,
        };
        let approx_tol = self.this_approx_tol_3d;

        for &v in given_verts {
            let Some(v) = v else {
                continue;
            };
            let edges_of_vtx = brep.vertex_edges(v)?;
            for e in edges_of_vtx {
                // Can skip edges that have been glued.
                if contains(excluded_edges, &Some(e)) {
                    continue;
                }
                let other_v = brep.edge_other_vertex(e, Some(v))?;
                if !contains(given_verts, &other_v) {
                    continue;
                }
                let other_v =
                    other_v.ok_or(invalid(OP_EDGE_FACE_COIN, "edge without other vertex"))?;

                // Find all faces that contain both vertices.
                let faces1 = brep.vertex_faces(v)?;
                let faces2 = brep.vertex_faces(other_v)?;
                let common_faces = find_common_elements(&faces1, &faces2);
                let edge_faces = brep.edge_faces(e)?;

                for f in common_faces {
                    if contains(&edge_faces, &f) {
                        continue;
                    }

                    // Check for coincidence of pE and pF.
                    let curve_h = brep
                        .edge_curve(e)
                        .ok_or(invalid(OP_EDGE_FACE_COIN, "edge handle is unknown"))?
                        .ok_or(invalid(OP_EDGE_FACE_COIN, "edge without curve"))?;
                    let surf_h = brep
                        .face_surface(f)
                        .ok_or(invalid(OP_EDGE_FACE_COIN, "face handle is unknown"))?
                        .ok_or(invalid(OP_EDGE_FACE_COIN, "face without surface"))?;
                    let edge_dom = brep
                        .edge_interval(e)
                        .ok_or(invalid(OP_EDGE_FACE_COIN, "edge handle is unknown"))?;
                    let mut face_dom = brep
                        .face_uv_domain(f)
                        .ok_or(invalid(OP_EDGE_FACE_COIN, "face handle is unknown"))?;
                    let edge_zone_tol = tol_of(brep, e)?;

                    // (status unchecked: the domain stays GetUVDomain's on failure)
                    if let Some(d) = ignore_soft(brep.face_uv_domain_from_uv_trim_curves(k, f))? {
                        face_dom = d;
                    }

                    // (the surface reaches its face's cache through the face,
                    // `smsurf_GetSurfaceCache`)
                    let surface = brep
                        .surface(surf_h)
                        .ok_or(invalid(
                            OP_EDGE_FACE_COIN,
                            "surface handle cannot be resolved",
                        ))?
                        .clone_for_evaluation()?;
                    let curve = curve_of(brep, Some(curve_h))?.clone_for_evaluation()?;
                    // (status unchecked: not coincident on failure)
                    let sol = ignore_soft(surface_solve_with_face_caches(k, brep, |context| {
                        surface.simple_coincidence_checker_with_context(
                            k,
                            &face_dom,
                            &curve,
                            &edge_dom,
                            approx_tol,
                            false,
                            [Some(curve_h), Some(surf_h)],
                            None,
                            context,
                        )
                    }))?
                    .flatten();
                    let Some(sol) = sol else {
                        continue;
                    };
                    if sol.start.solution_value > approx_tol {
                        continue;
                    }

                    // The curve may lie in the surface but not in the Face: test two
                    // points.
                    let mut pt_class = PointClassification::new(edge_zone_tol);
                    let crv_pt = eval_curve_point(k, brep, curve_h, edge_dom.evaluate(0.38))?;
                    // Allow any part of the Face: including Edges and Vertices
                    // (status unchecked).
                    ignore_soft(brep.face_point_3d_classify(
                        k,
                        f,
                        crv_pt,
                        edge_zone_tol,
                        false,
                        &mut pt_class,
                        None,
                    ))?;
                    if !matches!(
                        pt_class.point_class(),
                        PointClassType::Face | PointClassType::Edge | PointClassType::Vertex
                    ) {
                        continue;
                    }
                    if pt_class.deviation() > approx_tol {
                        continue;
                    }

                    let crv_pt = eval_curve_point(k, brep, curve_h, edge_dom.evaluate(0.71))?;
                    ignore_soft(brep.face_point_3d_classify(
                        k,
                        f,
                        crv_pt,
                        approx_tol,
                        false,
                        &mut pt_class,
                        None,
                    ))?;
                    if !matches!(
                        pt_class.point_class(),
                        PointClassType::Face | PointClassType::Edge | PointClassType::Vertex
                    ) {
                        continue;
                    }
                    if pt_class.deviation() > approx_tol {
                        continue;
                    }

                    // Merge this edge into this face: make it go from pV to pOtherV.
                    let vtx_pt = brep.vertex_point(v)?;
                    let pt = eval_curve_point(k, brep, curve_h, edge_dom.min())?;
                    let dist1 = vtx_pt.distance(pt);
                    let pt = eval_curve_point(k, brep, curve_h, edge_dom.max())?;
                    let dist2 = vtx_pt.distance(pt);
                    let orient = if dist1 < dist2 {
                        OrientType::Same
                    } else {
                        OrientType::Opposite
                    };

                    let edge_tol = tol_of(brep, e)?;
                    let face_b = brep
                        .face_brep(f)?
                        .ok_or(invalid(OP_EDGE_FACE_COIN, "face without B-rep"))?;

                    // We have to make a copy of the curve: what's passed in gets
                    // installed in the new edge.
                    let copy = curve_of(brep, Some(curve_h))?.clone();
                    let curve_copy = brep
                        .insert_curve(copy)
                        .ok_or(invalid(OP_EDGE_FACE_COIN, "curve store is full"))?;
                    // (status unchecked: no new edge, loop or face on failure)
                    let made = ignore_soft(brep_make_edge_in_face(
                        k,
                        brep,
                        face_b,
                        Some(f),
                        Some(v),
                        Some(other_v),
                        curve_copy,
                        None,
                        edge_dom,
                        orient,
                        edge_tol,
                    ))?;
                    let (new_edge, _new_loop, new_face) = made.unwrap_or((None, None, None));

                    add_unique(&mut out.processed_faces, f);
                    let dev = sol.start.solution_value;
                    if let Some(nf) = new_face {
                        out.new_faces.push(nf);
                        out.processed_faces.push(nf);
                        out.split_faces.push(f);
                        let t = tol_of(brep, f)?;
                        ignore_soft(brep.update_object_tolerance(k, nf, dev, t, false))?;
                    }

                    // A new edge would be glued to the edge it was made from.
                    if let Some(ne) = new_edge {
                        ignore_soft(brep.update_object_tolerance(k, ne, dev, edge_tol, false))?;
                        ignore_soft(brep.update_object_tolerance(k, f, dev, edge_tol, false))?;

                        // (the curve owner reset of upstream has no counterpart here)
                        // glue pE/pNewE edge pair - keep the original edge, pE
                        let rel_orient = OrientType::Same; // same curve.
                        let dist_to_curve = 0.0; // same curve.
                                                 // (pInsideFaceuses defaults to NULL)
                        match brep.brep_glue_edges_general(
                            k,
                            face_b,
                            Some(e),
                            rel_orient,
                            dist_to_curve,
                            do_region_nesting,
                            Some(ne),
                            None,
                        ) {
                            Ok(()) => out.imprinted_edges.push(e),
                            Err(
                                err @ (SolidError::Unported { .. }
                                | SolidError::InvalidInput { .. }
                                | SolidError::PortInvariant { .. }),
                            ) => return Err(err),
                            Err(_) => {
                                if !ignore_problems {
                                    return Err(SolidError::sm_err(OP_EDGE_FACE_COIN));
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(out)
    }
}

/// Whether `~SmProtoTopologyManager` reads freed memory (UB-175): it deletes the listed
/// proto edges before the listed proto vertices, each `~SmProtoEdge` unlists itself
/// once from its start vertex and once from a different end vertex, and each
/// `~SmProtoVertex` then dereferences every edge it still lists. An edge whose ends
/// coincide is listed twice there (`AddProtoEdge`, DD1) and a combined vertex keeps
/// entries of edges that no longer end at it, so such an entry is a listed edge
/// already freed. Replays the unlisting on copies of the vertex lists.
///
/// Only the edge-phase unlisting is modelled: a proto vertex freed before the
/// manager's destructor would be a different use-after-free and is not flagged (no
/// such path exists in source for these operations).
fn teardown_reads_freed<O: ClassObject>(mgr: &ProtoTopologyManager<O>) -> bool {
    let mut lists: Vec<(ProtoVertexId, Vec<ProtoEdgeId>)> = Vec::new();
    for &pv in mgr.proto_vertices() {
        if lists.iter().all(|(v, _)| *v != pv) {
            if let Some(v) = mgr.proto_vertex(pv) {
                lists.push((pv, v.proto_edges().to_vec()));
            }
        }
    }
    let mut unlist = |pv: Option<ProtoVertexId>, pe: ProtoEdgeId| {
        if let Some((_, l)) = lists.iter_mut().find(|(v, _)| Some(*v) == pv) {
            if let Some(i) = l.iter().position(|&e| e == pe) {
                l.remove(i);
            }
        }
    };
    let mut freed: Vec<ProtoEdgeId> = Vec::new();
    for &pe in mgr.proto_edges() {
        // (a second listing is a second delete of the same edge, skipped as `destroy`)
        if freed.contains(&pe) {
            continue;
        }
        let Some(edge) = mgr.proto_edge(pe) else {
            continue;
        };
        freed.push(pe);
        let (start, end) = (edge.start_proto_vertex(), edge.end_proto_vertex());
        unlist(start, pe);
        if end != start {
            unlist(end, pe);
        }
    }
    lists
        .iter()
        .any(|(_, l)| l.iter().any(|e| freed.contains(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::solution_array::ObjectKind;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct Edge(u8);

    impl ClassObject for Edge {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Edge
        }
    }

    impl SolutionObject for Edge {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Edge
        }
    }

    #[test]
    fn squeezed_edges_leave_both_mate_maps_even_when_only_one_side_deletes() {
        let pairs = [(Edge(0), Edge(1)), (Edge(2), Edge(3)), (Edge(4), Edge(5))];
        for (deleted_primary, deleted_other, remaining) in [
            (Some(Edge(0)), None, vec![pairs[1], pairs[2]]),
            (None, Some(Edge(1)), vec![pairs[1], pairs[2]]),
            (Some(Edge(0)), Some(Edge(1)), vec![pairs[1], pairs[2]]),
            (Some(Edge(0)), Some(Edge(3)), vec![pairs[2]]),
            (None, None, pairs.to_vec()),
            (Some(Edge(6)), Some(Edge(7)), pairs.to_vec()),
        ] {
            let mut ti = TopologyIntersector::new(None, None, 1e-5, 0.1);
            ti.seed_mates_for_test(&pairs);
            ti.forget_deleted_edge_relationships(deleted_primary, deleted_other)
                .unwrap();
            assert_eq!(
                ti.mapped_brep_objects(ClassObjectKind::Edge).unwrap(),
                remaining.iter().map(|&(a, _)| a).collect::<Vec<_>>(),
                "deleted {deleted_primary:?} / {deleted_other:?}"
            );
            for (primary, other) in pairs {
                let kept = remaining.contains(&(primary, other));
                assert_eq!(ti.other_mate(Some(primary)), kept.then_some(other));
                assert_eq!(ti.brep_mate(Some(other)), kept.then_some(primary));
            }
            // Repeating cleanup is harmless; a deleted edge has no remaining mate.
            ti.forget_deleted_edge_relationships(deleted_primary, deleted_other)
                .unwrap();
        }
    }
}
