//! Fillet construction, insertion, trimming and merge orchestration.
//!
//! Upstream: `source/SMLib/src/SmFilletExecutive.cpp`,
//! `source/SMLib/inc/SmFilletExecutive.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletExecutive::do_fillet`] | `SmFilletExecutive::DoFillet` |
//! | [`FilletExecutive::create_fillet_brep`] | `SmFilletExecutive::CreateFilletBrep` |
//! | [`FilletExecutive::preview_fillet_geometry`] | `SmFilletExecutive::GetPreviewFilletSurfaces` after its `CreateFilletCorners` |
//! | [`FilletExecutive::do_filleting`] | `SmFilletExecutive::DoFilleting` |
//! | [`FilletExecutive`], [`ErrorInfo`] | executive/error-info member data (co-ported from p1) |
//! | [`ExecutiveAccess`] | borrowed access to dependency-owned objects |
//! | [`FilletExecutive::finish_merged_fillet`], [`FilletFinishAccess`] | the `DoFilleting` tail after `MergeBrep` |
//!
//! Deviations: debug drawing/output is omitted. Handles follow the existing
//! topology-intersector adapter model: an adapter resolves topology to typed ids
//! in `Brep` arenas; it owns solver/corner objects, never this module. Views copy
//! only small metadata and handle lists, never B-reps. Error-info data is co-ported;
//! its assignment deliberately leaves the same fields unchanged as C++'s operator=.
//! Uninitialized C++ fillet-vertex pointers are represented by `None`. Constructor
//! and parameter setup belong to p1; callers supply the state. Missing dependency
//! calls return `Unported` without swallowing it in ignored-status branches. There
//! is no input-dependent recursion. The two temporary flags and temporary attribute
//! ownership are restored explicitly after fallible bodies, including on deferral.
//! The upstream `cbi_MAP_TOPO_EDGES=0` experiments remain disabled. Parent-linked
//! stage hooks borrow this live executive, never an adapter-owned duplicate.
//! Opt-in, not upstream: with `Kernel::fixes.fillet_refuse_non_manifold_result`
//! (WR-212, source issue #422) `do_fillet` ends with a closed-manifold
//! check of the target (`check_closed_manifold`). A target with an edge that is
//! not `SmEdge::IsManifold` is then refused with error code `SM_FILERR_BAD_MERGE`,
//! the edge in the error info and in the error, where upstream (the default)
//! returns success. `do_filleting` alone is unchanged.
//! Not upstream: [`ExecutiveAccess::shrink_face`] shrinks the new fillet faces with
//! knot snapping of rounding errors (source issue #472).

use std::collections::BTreeMap;

use crate::blend::fillet_executive::p1::ExecutiveObjects;
use crate::blend::fillet_geom::p1::{FilStatus, FilletCornerType, FilletEdgeType, FilletErrorType};
use crate::blend::fillet_solver::p1::FilletSurfaceGenerator;
use crate::boolean::topology_intersector::p1::TopologyIntersector;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::object::NotifyOperation;
use crate::geom::solution_array::SolutionObject;
use crate::geom::tol::zone_tol3d;
use crate::intersect::point_class::ClassObject;
use crate::kernel::Kernel;
use crate::math::Point3;
use crate::topology::proto_topology::p1::CopyBrepMap;

const OP: &str = "blend::fillet_executive::p3";

fn failure() -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code: 1001,
        entity: None,
    }
}

fn invalid(reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: OP,
        reason,
    }
}

fn ignore<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.is_unported() => Err(error),
        Err(_) => Ok(None),
    }
}

/// The initialized error-info members p3 reads or writes. Corner kinds use the
/// shared upstream enum; the other numeric enum values are `SmFilletErrorType`,
/// `SmFilStatus` and `SmFilletEdgeType`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorInfo<O> {
    /// `m_eErrorCode`.
    pub code: i32,
    /// `m_eFilletStatus`.
    pub status: i32,
    /// `m_eCornerType`.
    pub corner_type: FilletCornerType,
    /// `m_eEdgeType`.
    pub edge_type: i32,
    /// `m_pFilExec` (identity supplied by the executive owner).
    pub executive: Option<O>,
    /// `m_pFilletedVertex`.
    pub vertex: Option<O>,
    /// `m_pFilletedEdge`.
    pub edge: Option<O>,
    /// `m_pSideFace` (not copied by operator=).
    pub side_face: Option<O>,
    /// `m_pEndFace`.
    pub end_face: Option<O>,
    /// `m_pFilletVertex1/2` (not copied by operator=).
    pub fillet_vertices: [Option<O>; 2],
    /// `m_pFilletSurfaces`, including null entries.
    pub surfaces: Vec<Option<O>>,
    /// `m_pFilletEdges`, including null entries.
    pub edges: Vec<Option<O>>,
    /// `m_pFilletCurves` (not copied by operator=).
    pub curves: Vec<Option<O>>,
    /// `m_vLocation` (not copied by operator=).
    pub location: Option<Point3>,
    /// `m_cComment` (not copied by operator=).
    pub comment: String,
    /// `m_cEntityName` (not copied by operator=).
    pub entity_name: String,
}

impl<O> Default for ErrorInfo<O> {
    fn default() -> Self {
        Self {
            code: 0,
            status: 0,
            corner_type: FilletCornerType::Unknown,
            edge_type: FilletEdgeType::Unknown as i32,
            executive: None,
            vertex: None,
            edge: None,
            side_face: None,
            end_face: None,
            fillet_vertices: [None, None],
            surfaces: Vec::new(),
            edges: Vec::new(),
            curves: Vec::new(),
            location: None,
            comment: "<no comment>\n".into(),
            entity_name: "\n".into(),
        }
    }
}

impl<O: Copy> ErrorInfo<O> {
    pub(crate) fn assign(&mut self, source: &Self) {
        self.code = source.code;
        self.status = source.status;
        self.corner_type = source.corner_type;
        self.edge_type = source.edge_type;
        self.executive = source.executive;
        self.end_face = source.end_face;
        self.edge = source.edge;
        self.vertex = source.vertex;
        self.surfaces.clone_from(&source.surfaces);
        self.edges.clone_from(&source.edges);
    }
}

/// The executive's independent control switches; p1 setup determines their values.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // five independent upstream controls
pub struct ExecutionOptions {
    /// `m_bDoPiecewiseMerge`.
    pub piecewise_merge: bool,
    /// `m_bDoTopologyInsertion`.
    pub topology_insertion: bool,
    /// `m_bDoGlobalMerge`.
    pub global_merge: bool,
    /// `m_bDoTrimming`.
    pub trimming: bool,
    /// `m_bDoClassification`.
    pub classification: bool,
}

impl Default for ExecutionOptions {
    /// The five switch defaults of the upstream executive constructor.
    fn default() -> Self {
        Self {
            piecewise_merge: false,
            topology_insertion: true,
            global_merge: false,
            trimming: true,
            classification: true,
        }
    }
}

/// The native vertex map's pending solver edge uses or completed corner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VertexEntry<O> {
    /// Edge uses collected by `LoadFilletSolver`, in native insertion order.
    SolverEdgeuses(Vec<O>),
    /// The corner that replaced the pending list.
    Corner(O),
}

/// Actual face-selection capabilities, sharing the executive's ordered loop.
pub trait OriginalSurfaceTrimAccess<O: Copy + Ord, F> {
    /// Retained geometries of the actual solver, in order.
    fn original_trim_geometries(&self, solver: O) -> Result<Vec<O>>;
    /// Execute one geometry's selection with the caller's existing marks and outputs.
    fn original_trim_geometry(
        &mut self,
        k: &mut Kernel,
        geom: O,
        kept: &mut Vec<F>,
        deleted: &mut Vec<F>,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
}

struct IndexedOriginalTrim<'a, A> {
    access: &'a mut A,
    marks: &'a [ContextMark],
}

impl<O: ClassObject + SolutionObject + Ord, A: ExecutiveAccess<O>> OriginalSurfaceTrimAccess<O, O>
    for IndexedOriginalTrim<'_, A>
{
    fn original_trim_geometries(&self, solver: O) -> Result<Vec<O>> {
        self.access.solver_geoms(solver)
    }
    fn original_trim_geometry(
        &mut self,
        k: &mut Kernel,
        geom: O,
        kept: &mut Vec<O>,
        deleted: &mut Vec<O>,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        self.access
            .trim_original_surfaces(k, geom, kept, deleted, self.marks, executive)
    }
}

/// Attribute tagging and merge capabilities for the surviving intermediate body.
pub trait FilletTagMergeAccess<O: Copy + Ord> {
    /// Re-walk the intermediate body's current owning edges.
    ///
    /// # Errors
    /// Returns body lookup or traversal failures.
    fn tag_merge_edges(&self, body: O) -> Result<Vec<O>>;
    /// Read the recorded pseudo edge's native eligibility and mapped edge.
    ///
    /// # Errors
    /// Returns pseudo-edge lookup failures.
    fn tag_merge_edge(&self, edge: O) -> Result<TopoEdgeView<O>>;
    /// Allocate one `FILLET_EDGES` standalone-reference attribute.
    ///
    /// # Errors
    /// Returns allocation or store failures.
    fn tag_merge_attribute(&mut self, k: &Kernel) -> Result<O>;
    /// Attach the allocated attribute to the actual surviving edge.
    ///
    /// # Errors
    /// Returns attachment failures; the executive ignores ordinary native status.
    fn tag_merge_attach(&mut self, edge: O, attribute: O) -> Result<()>;
    /// Merge the intermediate body into the target, adding analytics.
    ///
    /// # Errors
    /// Returns the underlying merge failure without discarding partial edits.
    fn tag_merge_bodies(&mut self, k: &mut Kernel, target: O, fillet: O) -> Result<()>;
}

impl<O: ClassObject + SolutionObject + Ord, A: ExecutiveAccess<O>> FilletTagMergeAccess<O> for A {
    fn tag_merge_edges(&self, body: O) -> Result<Vec<O>> {
        self.edges(body)
    }
    fn tag_merge_edge(&self, edge: O) -> Result<TopoEdgeView<O>> {
        self.topo_edge(edge)
    }
    fn tag_merge_attribute(&mut self, k: &Kernel) -> Result<O> {
        self.new_keep_attribute(k)
    }
    fn tag_merge_attach(&mut self, edge: O, attribute: O) -> Result<()> {
        self.add_attribute(edge, Some(attribute))
    }
    fn tag_merge_bodies(&mut self, k: &mut Kernel, target: O, fillet: O) -> Result<()> {
        self.merge_brep(k, target, fillet, None, None, true)
    }
}

/// The capabilities of the post-merge tail of `DoFilleting` (`SmFilletExecutive.cpp`
/// 3532-3644): intermediate destruction, final stitching, E-attribute remapping,
/// `RemoveTopoEdges`, `BooleanPostProcess`, the per-face `PreEdit` notification and
/// `OrientTrimmedSurfaces`. Each method is the [`ExecutiveAccess`] method of the same
/// name without its `finish_` prefix.
pub trait FilletFinishAccess<O: ClassObject + SolutionObject + Ord> {
    /// `delete m_pFilletBrep` after its intersector references have been cleared.
    ///
    /// # Errors
    /// Lookup or destruction failures.
    fn finish_delete_brep(&mut self, k: &mut Kernel, brep: O) -> Result<()>;
    /// `GetTolerance` of the target.
    ///
    /// # Errors
    /// Lookup failures.
    fn finish_tolerance(&self, k: &Kernel, brep: O) -> Result<f64>;
    /// `SmStitch::DoStitching` with the `DoFilleting` configuration.
    ///
    /// # Errors
    /// Only an unresolved handle; stitch failures are [`StitchAttempt::status`].
    fn finish_do_stitching(
        &mut self,
        k: &mut Kernel,
        brep: O,
        tolerance: f64,
        settings: StitchSettings,
    ) -> Result<StitchAttempt>;
    /// `SmAttribute::GetUsers`, preserving order.
    ///
    /// # Errors
    /// Lookup failures.
    fn finish_attribute_users(&self, attribute: O) -> Result<Vec<O>>;
    /// `SmFilletExecutive::RemoveTopoEdges`.
    ///
    /// # Errors
    /// Those of the removal.
    fn finish_remove_topo_edges(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
    /// `SmMerge::BooleanPostProcess(bCheckAllEdges)`.
    ///
    /// # Errors
    /// Those of the post-process.
    fn finish_boolean_post_process(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
        check_all_edges: bool,
    ) -> Result<()>;
    /// `SmBrep::GetFaces`, preserving list order.
    ///
    /// # Errors
    /// Lookup failures.
    fn finish_faces(&self, brep: O) -> Result<Vec<O>>;
    /// `SmFace::Notify`.
    ///
    /// # Errors
    /// Those of the notification.
    fn finish_notify_face(
        &mut self,
        k: &mut Kernel,
        face: O,
        operation: NotifyOperation,
        data1: Option<O>,
        data2: Option<O>,
        data3: Option<O>,
    ) -> Result<()>;
    /// `SmBrep::OrientTrimmedSurfaces`.
    ///
    /// # Errors
    /// Those of the orientation.
    fn finish_orient_trimmed_surfaces(
        &mut self,
        k: &mut Kernel,
        brep: O,
        outward: bool,
        maybe_not_solid: &mut bool,
        just_unify: bool,
    ) -> Result<()>;
}

impl<O: ClassObject + SolutionObject + Ord, A: ExecutiveAccess<O>> FilletFinishAccess<O> for A {
    fn finish_delete_brep(&mut self, _k: &mut Kernel, brep: O) -> Result<()> {
        self.delete_brep(brep)
    }
    fn finish_tolerance(&self, k: &Kernel, brep: O) -> Result<f64> {
        self.tolerance(k, brep, false)
    }
    fn finish_do_stitching(
        &mut self,
        k: &mut Kernel,
        brep: O,
        tolerance: f64,
        settings: StitchSettings,
    ) -> Result<StitchAttempt> {
        self.do_stitching(k, brep, tolerance, None, None, settings, None, None)
    }
    fn finish_attribute_users(&self, attribute: O) -> Result<Vec<O>> {
        self.attribute_users(attribute)
    }
    fn finish_remove_topo_edges(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        self.remove_topo_edges(k, executive)
    }
    fn finish_boolean_post_process(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
        check_all_edges: bool,
    ) -> Result<()> {
        self.boolean_post_process(k, executive, check_all_edges)
    }
    fn finish_faces(&self, brep: O) -> Result<Vec<O>> {
        self.faces(brep)
    }
    fn finish_notify_face(
        &mut self,
        k: &mut Kernel,
        face: O,
        operation: NotifyOperation,
        data1: Option<O>,
        data2: Option<O>,
        data3: Option<O>,
    ) -> Result<()> {
        self.notify_face(k, face, operation, data1, data2, data3)
    }
    fn finish_orient_trimmed_surfaces(
        &mut self,
        k: &mut Kernel,
        brep: O,
        outward: bool,
        maybe_not_solid: &mut bool,
        just_unify: bool,
    ) -> Result<()> {
        self.orient_trimmed_surfaces(k, brep, outward, maybe_not_solid, just_unify, None)
    }
}

/// Native face-list completion and deletion through the retained intersector.
pub trait OriginalSurfaceDeletionAccess<O: ClassObject + SolutionObject + Ord> {
    /// Current owning faces of the actual fillet body, in traversal order.
    fn original_managed_faces(&self, body: O) -> Result<Vec<O>>;
    /// Delete the qualified list through the actual intersector and body provider.
    fn original_delete_faces(
        &mut self,
        k: &mut Kernel,
        intersector: &mut TopologyIntersector<O>,
        faces: &[O],
    ) -> Result<()>;
}
impl<O: ClassObject + SolutionObject + Ord, A: ExecutiveAccess<O>> OriginalSurfaceDeletionAccess<O>
    for A
{
    fn original_managed_faces(&self, body: O) -> Result<Vec<O>> {
        self.faces(body)
    }
    fn original_delete_faces(
        &mut self,
        k: &mut Kernel,
        intersector: &mut TopologyIntersector<O>,
        faces: &[O],
    ) -> Result<()> {
        self.delete_faces(k, intersector, faces, None)
    }
}

/// Executive member state consumed by p3, co-ported from p1. This is not the
/// constructor: p1 must supply its constructor/setup defaults and intersector.
#[derive(Debug, Clone)]
pub struct FilletExecutive<O> {
    /// `m_vFilletedVertices`, in first-insertion order (a null vertex is retained).
    pub filleted_vertices: Vec<Option<O>>,
    /// `m_pTargetBrep`.
    pub target: Option<O>,
    /// `m_pFilletBrep`.
    pub fillet: Option<O>,
    /// `m_vFilletCorners` in upstream order.
    pub corners: Vec<O>,
    /// `m_vFilletSolvers` in upstream order.
    pub solvers: Vec<O>,
    /// `m_vTopoEdges` (fillet edges until replaced by actual B-rep edges).
    pub topo_edges: Vec<Option<O>>,
    /// `m_pAttributeE`.
    pub edge_attribute: Option<O>,
    /// `m_bIsInitialized`.
    pub initialized: bool,
    /// The five operation switches.
    pub options: ExecutionOptions,
    /// `m_sErrorInfo`.
    pub error: ErrorInfo<O>,
    /// The inherited `m_vTI`; this is the existing intersector type.
    pub intersector: TopologyIntersector<O>,
    /// Inherited `m_sIntersectionEdges` (null casts retained).
    pub intersection_edges: Vec<Option<O>>,
    /// Inherited `m_sIntersectionVertices` (null casts retained).
    pub intersection_vertices: Vec<Option<O>>,
    /// `m_sLawSequenceEdgeList`: the edge sequences `CreateSequenceOfLaws` set up, each
    /// in order (added by `blend.fillet_executive_p2`).
    pub law_sequence_edges: Vec<Vec<O>>,
    /// `m_pPseudoBrep`: the fillet B-rep of fillet edges and vertices (added by
    /// `blend.fillet_executive_p1`).
    pub pseudo_brep: Option<O>,
    /// `m_vFSGs`: the fillet surface generators the executive owns (added by
    /// `blend.fillet_executive_p1`).
    pub surface_generators: Vec<O>,
    /// `m_vVertexCornerMap`: one pending-list or completed-corner slot per vertex.
    /// A null vertex is retained as `None`, matching native loader ordering.
    pub vertex_corner_map: BTreeMap<Option<O>, VertexEntry<O>>,
    /// `m_vEdgeuseSolverMap`: the fillet solver of each filleted edge use (added by
    /// `blend.fillet_executive_p1`; `blend::fillet_corner::p3::FilletExecutiveData`
    /// holds the same map over typed ids).
    pub edgeuse_solver_map: BTreeMap<O, O>,
    /// `m_pSelfIntersectionHandler` (added by `blend.fillet_executive_p1`).
    pub self_intersection_handler: Option<O>,
    /// `m_bBlendCurveAtThirds` (the fork's, source fork `fd3b6a7`; `FALSE` from
    /// the constructor): `SetFilletParameters` builds its blend-curve generators with
    /// their sections at thirds of the way to the corner.
    pub blend_curve_at_thirds: bool,
    /// `m_bMarkCornerPatches` (the fork's, source fork `60af574`; `FALSE` from
    /// the constructor): `DoFilleting` tags each face of a set-back corner's patch with
    /// an `SM_AI_FILLET_CORNER_FACE` mark naming the corner before the merge.
    pub mark_corner_patches: bool,
}

/// Error-only corner metadata, requested only after a corner operation fails.
#[derive(Debug, Clone)]
pub struct CornerView<O> {
    /// `GetFilletedVertex()`; failure reporting accepts null.
    pub vertex: Option<O>,
    /// `GetCornerType()`.
    pub corner_type: FilletCornerType,
    /// `GetName(sBuff, SM_TBLOCK_SIZE)`.
    pub name: String,
}

/// Metadata of a recorded fillet edge, owned by fillet-geometry p2.
#[derive(Debug, Clone, Copy)]
pub struct TopoEdgeView<O> {
    /// `GetFilletEdgeType() == SM_FE_ON_EDGE`.
    pub on_edge: bool,
    /// Whether `GetCurve()` is non-null.
    pub has_curve: bool,
    /// `m_pFilletBrepEdge1`.
    pub brep_edge: Option<O>,
}

/// Stitch outputs read by this executive. Ordinary errors must still preserve the
/// counts already written by C++; `Unported` remains a `Result` error.
#[derive(Debug, Clone)]
pub struct StitchAttempt {
    /// `eStat` converted to a Rust error, if any.
    pub status: Option<SolidError>,
    /// `lNumStitched`.
    pub stitched: u64,
    /// `lNumLamina`.
    pub lamina: u64,
    /// `dMaxVGap`.
    pub max_vertex_gap: f64,
    /// `dMaxEGap`.
    pub max_edge_gap: f64,
}

impl StitchAttempt {
    /// The attempt a `SmBrep::StitchFaces` call ([`crate::topology::brep::p4::stitch_faces`])
    /// reports: its counts and gaps, or its failure as `status` with zero counts and
    /// gaps (the callers read none of them after a failure). [`SolidError::Unported`]
    /// propagates.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the stitch.
    pub fn from_stitch(r: Result<crate::topology::brep::p4::StitchOutcome>) -> Result<Self> {
        match r {
            Ok(o) => Ok(Self {
                status: None,
                stitched: u64::from(o.stitched_edges),
                lamina: u64::from(o.lamina_edges),
                max_vertex_gap: o.max_vertex_edge_gap,
                max_edge_gap: o.max_edge_face_trim_curve_gap,
            }),
            Err(e) if e.is_unported() => Err(e),
            Err(e) => Ok(Self {
                status: Some(e),
                stitched: 0,
                lamina: 0,
                max_vertex_gap: 0.0,
                max_edge_gap: 0.0,
            }),
        }
    }
}

/// Retained operations for the native ordered geometry/corner insertion component.
pub trait TopologyInsertionAccess<O: ClassObject + SolutionObject + Ord> {
    /// Copy the current solver's geometry list at its native traversal position.
    /// # Errors
    /// Missing actual solver context.
    fn insertion_solver_geoms(&self, solver: O) -> Result<Vec<O>>;
    /// Insert one actual geometry through the retained intersector.
    /// # Errors
    /// The original geometry insertion error.
    fn insertion_geom_topology(
        &mut self,
        k: &mut Kernel,
        geom: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
    /// Insert one actual corner after all solver geometries.
    /// # Errors
    /// The original corner insertion error.
    fn insertion_corner_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
}
impl<O, A> TopologyInsertionAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn insertion_solver_geoms(&self, solver: O) -> Result<Vec<O>> {
        ExecutiveAccess::solver_geoms(self, solver)
    }
    fn insertion_geom_topology(
        &mut self,
        k: &mut Kernel,
        geom: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::insert_geom_topology(self, k, geom, executive)
    }
    fn insertion_corner_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::insert_corner_topology(self, k, corner, executive)
    }
}

/// Dependency access. Handles must preserve object identity across B-reps and
/// resolve topology through typed `Brep` arenas. Required methods read dependency
/// records or adapt existing APIs; default methods are precisely the missing calls, with complete arguments.
/// Parent-linked corner/solver/geometry operations receive the live executive,
/// including its options, lists, error info and intersector. Adapters must not
/// retain a second executive or intersector; writes must be visible to the next phase.
/// No claimed solver/corner implementation or record is reproduced here.
pub trait ExecutiveAccess<O: ClassObject + SolutionObject + Ord> {
    /// The actual retained solver base, borrowed from its concrete solver object.
    fn solver_base(&self, solver: O) -> Result<&crate::blend::fillet_solver::p1::FilletSolver>;
    /// The same actual solver for a native member write.
    fn solver_base_mut(
        &mut self,
        solver: O,
    ) -> Result<&mut crate::blend::fillet_solver::p1::FilletSolver>;
    /// `GetSurface(rail)`, including the actual geometry-for-surfaces fallback.
    fn solver_has_surface(&self, solver: O, rail: usize) -> Result<bool>;
    /// G1 knots from the edge's actual knots and existing-or-created curve cache.
    fn edge_g1_knots(&mut self, k: &Kernel, edge: O) -> Result<Vec<f64>>;
    /// `edgeuse->GetVertexuse()->GetVertex()`; a null vertex remains null.
    fn edgeuse_vertex(&self, edgeuse: O) -> Result<Option<O>>;
    /// `GetName`, `GetCornerType`, `GetFilletedVertex` for error reporting only.
    fn corner_error_info(&self, corner: O, executive: &FilletExecutive<O>)
        -> Result<CornerView<O>>;
    /// `SmFilletCorner::IsBlendingCorner`.
    fn is_blending_corner(&self, corner: O) -> Result<bool>;
    /// `SmFilletCorner::IsTangentCorner`; called only for blending corners.
    fn is_tangent_corner(&self, corner: O) -> Result<bool>;
    /// `SmFilletCorner::GetFilletSolvers`; called only for tangent blending corners.
    /// This is owned logic, not a field accessor. Upstream repeats the lookup of
    /// `m_pSolverEUs[0]` for every entry, through the current executive's solver list
    /// (`SmFilletCorner.cpp:4067-4075`); adapters must preserve that quirk. Delegate to
    /// the already ported `blend::fillet_corner::p1::FilletCorner::fillet_solvers`
    /// (fillet corner p1), which does.
    fn corner_solvers(&self, corner: O, executive: &FilletExecutive<O>) -> Result<Vec<O>>;
    /// `SmFilletCorner::GetFilletedVertex`.
    fn corner_vertex(&self, corner: O) -> Result<Option<O>>;
    /// `SmFilletCorner::GetThisApproxTol3d`.
    fn corner_approx_tolerance(&self, corner: O) -> Result<f64>;
    /// `SmFilletSolver::m_vFilletGeoms`, copied only where the caller needs them.
    fn solver_geoms(&self, solver: O) -> Result<Vec<O>>;
    /// Live indexed access to `m_vFilletGeoms`, without copying its full array.
    fn solver_geom(&self, solver: O, index: usize) -> Result<Option<O>>;
    /// `SmFilletSolver::GetEdgeuse(0)->GetEdge()`.
    fn solver_edge(&self, solver: O) -> Result<O>;
    /// `SmFilletSolver::GetThisApproxTol3d`.
    fn solver_approx_tolerance(&self, solver: O) -> Result<f64>;
    /// `SmFilletGeom::m_pFilletSurface->GetOwner()`, cast to face.
    fn geom_face(&self, geom: O) -> Result<Option<O>>;
    /// Metadata of `m_vTopoEdges` entries.
    fn topo_edge(&self, edge: O) -> Result<TopoEdgeView<O>>;
    /// `SmVertex::GetPoint`.
    fn vertex_point(&self, vertex: O) -> Result<Point3>;
    /// `SmEdge::GetCurve()->GetEnds`.
    fn edge_curve_ends(&self, k: &Kernel, edge: O) -> Result<(Point3, Point3)>;
    /// `SetCalcAsConstRad`.
    fn set_calc_as_const_rad(&mut self, corner: O, value: bool) -> Result<()>;
    /// Allocate `SmBrep()` in the executive context.
    fn new_brep(&mut self, k: &Kernel) -> Result<O>;
    /// Delete a B-rep after its intersector references have been cleared.
    fn delete_brep(&mut self, brep: O) -> Result<()>;
    /// `GetTolerance` / `SmTol::GetApproxTol3d` with `approx=true`.
    fn tolerance(&self, k: &Kernel, brep: O, approx: bool) -> Result<f64>;
    /// `m_bEditingEnabled`.
    fn editing_enabled(&self, brep: O) -> Result<bool>;
    /// Store `m_bEditingEnabled`.
    fn set_editing_enabled(&mut self, brep: O, enabled: bool) -> Result<()>;
    /// `SmBrep::GetEdges`, preserving list order.
    fn edges(&self, brep: O) -> Result<Vec<O>>;
    /// `SmEdge::IsManifold` (`topology::edge::is_manifold`): four edge uses, the
    /// first a loop edge use. Used only by the closed-manifold result check.
    fn edge_is_manifold(&self, edge: O) -> Result<bool>;
    /// The typed id of `edge` for error context, when the handle has one.
    fn edge_entity(&self, _edge: O) -> Option<EntityRef> {
        None
    }
    /// `SmBrep::GetVertices`, preserving list order.
    fn vertices(&self, brep: O) -> Result<Vec<O>>;
    /// `SmBrep::GetFaces`, preserving list order.
    fn faces(&self, brep: O) -> Result<Vec<O>>;
    /// `SmAObject::AddAttribute` (null is passed through).
    fn add_attribute(&mut self, edge: O, attribute: Option<O>) -> Result<()>;
    /// `new SmAttribute(SM_AI_FILLET_EDGES, SM_AB_STANDALONE_REFERENCE)`.
    fn new_keep_attribute(&mut self, k: &Kernel) -> Result<O>;
    /// `SmAttribute::GetUsers`, preserving order.
    fn attribute_users(&self, attribute: O) -> Result<Vec<O>>;
    /// Destroy temporary attributes and unlink their users, including on errors.
    fn delete_attribute(&mut self, attribute: O) -> Result<()>;
    /// Mark common edges and vertices using one lock per distinct context, then
    /// invoke the body while those locks are held. Both mark types and context
    /// identities must be preserved (fillet context first). Each pair lists
    /// objects in mark-write order: other/target for vertices, target/other for
    /// edges; resolve each object's context, never infer it from tuple position.
    fn with_marks<T>(
        &mut self,
        k: &mut Kernel,
        target: O,
        fillet: O,
        mates: &[(O, O)],
        body: impl FnOnce(&mut Self, &mut Kernel, &[ContextMark]) -> Result<T>,
    ) -> Result<T>;
    /// `SmFace::ShrinkGeometry()` with its header defaults; status is ignored.
    /// Legacy indexed adapters (not upstream, source issue #472) run
    /// [`crate::topology::face::p1::shrink_geometry_with`] with
    /// [`crate::nurbs::b_spline_surface_n_lib::p2::TrimOptions::snap_roundoff_to_knots`],
    /// so that a fillet face whose domain lies a rounding error inside a knot of its
    /// round keeps no sliver span (upstream's `TrimWithDomain` leaves one of
    /// `10 * SM_EFF_ZERO_SQRT * (1 + |u|)`).
    fn shrink_face(&mut self, k: &mut Kernel, face: O) -> Result<()>;
    /// `SmFace::Notify`, adapting to `topology::face::p3::notify` with the
    /// caller's live notification records, attributes and surface trim cache.
    /// All notification arguments are retained, including null.
    fn notify_face(
        &mut self,
        k: &mut Kernel,
        face: O,
        operation: NotifyOperation,
        data1: Option<O>,
        data2: Option<O>,
        data3: Option<O>,
    ) -> Result<()>;
    /// `SmMerge::BooleanPostProcess`, using its actual intersector and recorded
    /// intersection lists (the implementation is already ported in merge p1).
    fn boolean_post_process(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
        check_all_edges: bool,
    ) -> Result<()>;

    /// `SmFilletExecutive::CreateFilletCorners`: the implementor runs
    /// [`FilletExecutive::create_fillet_corners`] with its `SmFilletCorner::Create`
    /// factory (the live session creates its corners itself).
    fn create_corners(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
        error: Option<&mut ErrorInfo<O>>,
    ) -> Result<()>;

    /// `SmFilletExecutive::ProcessSmoothEdgeSequences` (`blend.fillet_executive_p2`).
    fn process_smooth_edge_sequences(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        executive.process_smooth_edge_sequences(k, self)
    }

    /// Pure virtual `SmFilletCorner::MakeCornerTopology` dispatch. Concrete
    /// overrides are not assigned to units.json; adapters must use the corner
    /// owner's implementation rather than inventing an UNPORTED owner here.
    fn make_corner_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        _executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmFilletCorner::CalcCornerVertGeom`: delegate to the already ported
    /// `blend::fillet_corner::p1::FilletCorner::calc_corner_vert_geom` (fillet corner
    /// p1) on the corner `corner` resolves to, with its `CornerEnv`.
    fn calc_corner_vertices(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmFilletCorner::CheckAndFixDegeneracy`: delegate to the already ported
    /// `blend::fillet_corner::p1::FilletCorner::check_and_fix_degeneracy` (fillet corner
    /// p1) on the corner `corner` resolves to, with its `CornerEnv`.
    fn check_degeneracy(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<bool>;

    /// Virtual `AdjustTopology` dispatch: delegate to the ported
    /// `blend::fillet_corner::p3::FilletCorner::adjust_topology`, which runs
    /// `SmFilletNx2Corner::AdjustTopology`, `SmFilletConvexNxNCorner::AdjustTopology`
    /// (`blend.fillet_corner_p2`) and the base, 4x3 and `NxN` versions
    /// (`blend.fillet_corner_p1`: `return SM_SUCCESS`).
    fn adjust_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// Virtual `AdjustTangentCorner` dispatch: delegate to the ported
    /// `blend::fillet_corner::p3::FilletCorner::adjust_tangent_corner`, which runs
    /// `SmFilletNx2Corner::AdjustTangentCorner`,
    /// `SmFilletNx1ClosedCorner::AdjustTangentCorner` (`blend.fillet_corner_p2`) and the
    /// base (`blend.fillet_corner_p1`: `return SM_SUCCESS`).
    fn adjust_tangent_corner(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmFilletCorner::CalcCornerEdgeGeom`: delegate to the already ported
    /// `blend::fillet_corner::p1::FilletCorner::calc_corner_edge_geom` (fillet corner
    /// p1) on the corner `corner` resolves to, with its `CornerEnv`.
    fn calc_corner_edges(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmFilletCorner::ReInitializeCornerEdges`: delegate to the already ported
    /// `blend::fillet_corner::p1::FilletCorner::re_initialize_corner_edges` (fillet
    /// corner p1) on the corner `corner` resolves to.
    fn reinitialize_corner_edges(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmFilletCorner::CalcCornerGeom()` (`SmFilletExecutive.cpp:2448`): delegate to
    /// the already ported `blend::fillet_corner::p3::FilletCorner::calc_corner_geom`
    /// (fillet corner p3) on the corner `corner` resolves to, with its `CornerEnv`
    /// (the target and fillet B-reps and the executive's solvers and edge-use map).
    /// Its status is returned unchanged; the caller handles it as upstream does.
    fn calc_corner_geom(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// Virtual `CalcFilletGeom` dispatch. The base is
    /// `blend::fillet_solver::p4::FilletSolver::calc_fillet_geom` (it takes the solver
    /// as its `FilletSolverOps`, the target and pseudo B-reps and the solver table);
    /// the `SmSurfaceSurfaceFS` override is `blend.fillet_standard_solver_p2`'s.
    /// Adapters dispatch by concrete solver type.
    fn calc_fillet_geom(
        &mut self,
        k: &mut Kernel,
        solver: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmFilletSolver::TrimRails()` (`SmFilletExecutive.cpp:2350`): delegate to the
    /// already ported `blend::fillet_solver::p1::FilletSolver::trim_rails` (fillet
    /// solver p1) on the solver `solver` resolves to. Its status is returned
    /// unchanged; the caller handles it as upstream does.
    fn trim_rails(
        &mut self,
        k: &mut Kernel,
        solver: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmFilletSolver::TestRollOver(sAdjustedCorners)` (`SmFilletExecutive.cpp:2384`):
    /// delegate to the already ported `FilletSolver::test_roll_over` in
    /// `blend::fillet_solver::p5` on the solver `solver` resolves to. Its status is
    /// returned unchanged; the caller handles it as upstream does.
    fn test_rollover(
        &mut self,
        k: &mut Kernel,
        solver: O,
        adjusted_corners: &mut Vec<O>,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmBrep::SetTolerance(sZoneTol3d, bUpdateOnlyIfLarger, bCascadeToBndries)`:
    /// delegate to the already ported `topology::brep::p2::set_tolerance` (brep p2) on
    /// the B-rep `brep` resolves to, passing every argument. Upstream's `SetTolerance`
    /// is `void`, so the callers ignore any non-`Unported` error
    /// (`SmFilletExecutive.cpp:2439`, `:2509`).
    fn set_brep_tolerance(
        &mut self,
        k: &mut Kernel,
        brep: O,
        tolerance: f64,
        update_only_if_larger: bool,
        cascade_to_boundaries: bool,
    ) -> Result<()>;

    /// `SmFilletGeom::MakeFaceBrep(m_pFilletBrep)`: delegate to the already ported
    /// `blend::fillet_geom::p4::FilletBrep::make_face_brep` (fillet geom p4) on the
    /// fillet geometry `geom` resolves to, with the executive's solver table, the
    /// fillet B-rep `brep` resolves to (the face is built there) and that B-rep's notify
    /// environment. Its status is returned unchanged; the caller ignores a failure as
    /// upstream does (`SmFilletExecutive.cpp:2519`).
    fn make_face_brep(
        &mut self,
        k: &mut Kernel,
        geom: O,
        brep: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmBrep::StitchFaces(tol, ..., pOptFaces, bAllowSpineEdges)`: delegate to the
    /// already ported `topology::brep::p4::stitch_faces` (brep p4) on the B-rep `brep`
    /// resolves to, passing every argument, and report it as
    /// [`StitchAttempt::from_stitch`] does.
    fn stitch_faces(
        &mut self,
        k: &mut Kernel,
        brep: O,
        tolerance: f64,
        faces: Option<&[O]>,
        allow_spine_edges: bool,
    ) -> Result<StitchAttempt>;

    /// `SmFilletExecutive::DoPiecewiseMerge` on the implementor's bodies (the live
    /// session's port is `FilletSession`'s piecewise merge).
    fn piecewise_merge(&mut self, k: &mut Kernel, executive: &mut FilletExecutive<O>)
        -> Result<()>;

    /// `SmFilletExecutive::SetFilletBrepEdgeMap` (`blend.fillet_executive_p2`).
    fn set_fillet_brep_edge_map(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        executive.set_fillet_brep_edge_map(k, self)
    }

    /// `SmFilletGeom::InsertIntersectionTopology()`: delegate to the already ported
    /// `blend::fillet_geom::p4::FilletBrep::insert_intersection_topology` (fillet geom
    /// p4) on the fillet geometry `geom` resolves to, with the executive's solver table,
    /// the target B-rep (`executive.target`) and the fillet B-rep
    /// (`executive.fillet`). Its status propagates (`SER`, `SmFilletExecutive.cpp:3113`).
    fn insert_geom_topology(
        &mut self,
        k: &mut Kernel,
        geom: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `SmFilletCorner::InsertIntersectionTopology()`: delegate to the already ported
    /// `blend::fillet_corner::p1::FilletCorner::insert_intersection_topology_with_context` (fillet
    /// corner p1) on the corner `corner` resolves to, with the executive's insertion
    /// context (its topology intersector, the target and fillet B-reps). Its status propagates (`SER`, `SmFilletExecutive.cpp:2758`,
    /// `:3148`).
    fn insert_corner_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// Delegate to the live intersector's already ported
    /// `TopologyIntersector::intersect_insert_relate` (boolean intersector p2),
    /// using this adapter's `IntersectorBrep` view and stable intersector handle.
    /// Preserve its mutations on error; p3 does not consume its deleted list.
    fn intersect_insert_relate(
        &mut self,
        k: &mut Kernel,
        intersector: &mut TopologyIntersector<O>,
    ) -> Result<()>;

    /// `SmFilletGeom::TrimOriginalSurfaces(rKept, rDeleted, rMarkLocks)`: delegate to
    /// the already ported `blend::fillet_geom::p6::FilletBrep::trim_original_surfaces`
    /// (fillet geom p6) on the fillet geometry `geom` resolves to, with the fillet
    /// B-rep (`executive.fillet`), the target B-rep (`executive.target`), the
    /// executive's topology-intersector mates and the solver table; the faces it
    /// collects resolve back to handles in `kept_faces` / `deleted_faces`.
    fn trim_original_surfaces(
        &mut self,
        k: &mut Kernel,
        geom: O,
        kept_faces: &mut Vec<O>,
        deleted_faces: &mut Vec<O>,
        marks: &[ContextMark],
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// Delegate to the live intersector's already ported
    /// `TopologyIntersector::delete_faces` (boolean intersector p2), using this
    /// adapter's `IntersectorBrep` view and preserving every argument.
    fn delete_faces(
        &mut self,
        k: &mut Kernel,
        intersector: &mut TopologyIntersector<O>,
        faces: &[O],
        keep_as_wires: Option<&[O]>,
    ) -> Result<()>;

    /// `target->MergeBrep(other, pOptCopyBrepMap, pOptReverseMap, bAddAnalytics)`: the
    /// owner of the B-reps implements it with `topology::brep::p3::merge_brep` (a
    /// stateless owner through `topology::brep::p3::with_kernel_callbacks`, `other`
    /// enrolled in the same identity domain), as `IntersectorBrep::brep_delete_face`.
    ///
    /// # Errors
    ///
    /// Those of the merge; an owner with no B-rep behind a handle returns its own error.
    fn merge_brep(
        &mut self,
        k: &mut Kernel,
        target: O,
        other: O,
        copy_map: Option<&mut CopyBrepMap<O>>,
        reverse_map: Option<&mut CopyBrepMap<O>>,
        add_analytics: bool,
    ) -> Result<()>;

    /// `SmStitch::DoStitching` (`SmFilletExecutive.cpp:3559-3576`). Delegate to the
    /// already ported `heal::stitch::p2::Stitch::do_stitching` (heal stitch p2) on the
    /// B-rep `brep` resolves to:
    /// - build the `Stitch` with `DefaultStitchCallback` (upstream's base
    ///   `SmStitchCallback`) and `tolerance` as its stitch tolerance, and copy every
    ///   [`StitchSettings`] field onto the `Stitch` field of the same name;
    /// - pass the candidate lists through (`DoFilleting` passes `None` for both);
    /// - pass a `MinGapRequest` with `vertex` and `edge` set by whether
    ///   `min_vertex_gap` and `min_edge_gap` are `Some` (both `false` from
    ///   `DoFilleting`, whose call omits the min-gap out-params), writing the
    ///   reported gaps back when requested;
    /// - map `Stitching.{stitched_edges, lamina_edges, max_vertex_gap, max_edge_gap}`
    ///   to [`StitchAttempt`]`.{stitched, lamina, max_vertex_gap, max_edge_gap}` with
    ///   `status: None`; an error from `do_stitching` becomes `status: Some(error)`
    ///   with zero counts and gaps (the outputs `DoStitching` initialises), so the
    ///   retry loop sees it as upstream's `eStat`. Return `Err` only when the handle
    ///   cannot be resolved.
    ///
    /// `Stitch` keeps no state between calls besides its options and tolerance, so
    /// one `Stitch` per call matches upstream's single `sStitch` whose tolerance
    /// `SetStitchTol3d` doubles between tries.
    ///
    /// Once this is wired to a live `Brep`, UB-197 (`DoStitching` keeps using an edge
    /// that a failed glue deleted) applies: `DoFilleting` sets
    /// `m_bIgnoreProblems = TRUE`, so a failed glue does not stop the stitch.
    fn do_stitching(
        &mut self,
        k: &mut Kernel,
        brep: O,
        tolerance: f64,
        candidate_vertices: Option<&[O]>,
        candidate_edges: Option<&[O]>,
        settings: StitchSettings,
        min_vertex_gap: Option<&mut f64>,
        min_edge_gap: Option<&mut f64>,
    ) -> Result<StitchAttempt>;

    /// `SmFilletExecutive::RemoveTopoEdges`: the owner of the B-reps implements it with
    /// [`crate::blend::fillet_executive::p1::remove_topo_edges`] on the target B-rep and
    /// the edges of `executive.topo_edges`. That function notifies through local
    /// environments only, so an owner tracking attribute users or identities of the
    /// deleted edges, vertices and surfaces (the topo edges are the fillet-edges
    /// attribute's users) updates them itself.
    fn remove_topo_edges(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;

    /// `brep->OrientTrimmedSurfaces(bNormalsOutward, rbMaybeNotSolid, bJustUnifyNormals,
    /// pOptFacesToSwapUV)`: the owner of the B-reps implements it with
    /// `topology::brep::p3::orient_trimmed_surfaces`, writing its
    /// `maybe_not_closed_solid` to `maybe_not_solid`.
    ///
    /// # Errors
    ///
    /// Those of the orientation; an owner with no B-rep behind a handle returns its own
    /// error.
    fn orient_trimmed_surfaces(
        &mut self,
        k: &mut Kernel,
        brep: O,
        outward: bool,
        maybe_not_solid: &mut bool,
        just_unify: bool,
        faces_to_swap_uv: Option<&mut Vec<O>>,
    ) -> Result<()>;

    // ---- added by blend.fillet_executive_p2 (PR #267) --------------------------------

    /// `SmFilletGeom::GetFilletSurface() != NULL`.
    fn geom_has_surface(&self, geom: O) -> Result<bool>;
    /// The rails of a fillet geometry in `SetFilletBrepEdgeMap`'s order:
    /// `m_vRails[0]`, `m_vRails[1]`, then `m_vOtherRails1` and `m_vOtherRails2`
    /// (`None` for a null entry).
    fn geom_rails(&self, geom: O) -> Result<Vec<Option<O>>>;
    /// `SmFilletGeom::m_vSideEUs`, in order.
    fn geom_side_edgeuses(&self, geom: O) -> Result<Vec<O>>;
    /// `SmEdgeuse::GetEdge()` / the edge use's owning edge (`None` for null).
    fn edgeuse_edge(&self, edgeuse: O) -> Result<Option<O>>;
    /// `SmEdge::GetEdgeuses()` (used by `blend.fillet_executive_p1`).
    fn edge_edgeuses(&self, edge: O) -> Result<Vec<O>>;
    /// `SmFilletEdge::GetName`, `GetFilletEdgeType()` and `GetStatus()` of a fillet
    /// edge; the name is `None` for the types `GetName` writes nothing for (used by
    /// `blend.fillet_executive_p1`).
    fn fillet_edge_note(
        &self,
        fillet_edge: O,
    ) -> Result<(Option<&'static str>, FilletEdgeType, FilStatus)>;
    /// The objects `SmFilletExecutive(crContext, pTargetBrep)` allocates: its identity,
    /// a new pseudo `SmFilletBrep` with editing enabled and a new
    /// `SmAttribute(SM_AI_ALL_FILLET_EDGES, SM_AB_STANDALONE_REFERENCE)` (used by
    /// `blend.fillet_executive_p1`).
    fn new_executive_objects(&mut self, k: &mut Kernel) -> Result<ExecutiveObjects<O>>;
    /// `SmFilletEdge::GetOriginalFace()`.
    fn fillet_edge_original_face(&self, fillet_edge: O) -> Result<Option<O>>;
    /// `SmFilletEdge::GetOriginalEdge()`.
    fn fillet_edge_original_edge(&self, fillet_edge: O) -> Result<Option<O>>;
    /// `SmFilletEdge::FindFilletBrepEdge(crEdges, bFindMultipleEdges)` (the
    /// `blend.fillet_geom_p1` port, `FilletBrep::find_fillet_brep_edge`).
    fn find_fillet_brep_edge(
        &mut self,
        k: &mut Kernel,
        fillet_edge: O,
        edges: &[O],
        find_multiple_edges: bool,
    ) -> Result<()>;
    /// `SmFilletCorner::m_vEdges` (`None` for a null entry).
    fn corner_edges(&self, corner: O) -> Result<Vec<Option<O>>>;
    /// `SmEdge::GetBlendEdgeuse` (`crate::topology::edge::blend_edgeuse`).
    fn blend_edgeuse(&mut self, k: &mut Kernel, edge: O) -> Result<Option<O>>;
    /// `SmEdge::GetTolerance`.
    fn edge_tolerance(&self, edge: O) -> Result<f64>;
    /// `SmEdge::GetStartVertex()` and `GetOtherVertex(start)`.
    fn edge_vertices(&self, edge: O) -> Result<(Option<O>, Option<O>)>;
    /// `SM_CAST_PTR(SmVariableRadiusFS, solver)`: the solver's variable-radius data,
    /// `None` for any other solver class.
    fn variable_radius_solver(
        &self,
        solver: O,
    ) -> Result<Option<&crate::blend::fillet_standard_solver::p1::VariableRadiusFs>>;
    /// The same cast, for writing the law (`SmTemporaryRadiusChange`).
    fn variable_radius_solver_mut(
        &mut self,
        solver: O,
    ) -> Result<Option<&mut crate::blend::fillet_standard_solver::p1::VariableRadiusFs>>;

    /// `SmFilletSolver::GetEdgeuse(lIndex)`: delegate to the solver's
    /// `FilletSolver::edgeuse` (`blend.fillet_solver_p1`).
    fn solver_edgeuse(&self, solver: O, index: usize) -> Result<Option<O>>;

    /// `SmFilletSolver::GetFilletCorner(lIndex)`: delegate to the solver's
    /// `FilletSolver::fillet_corner` (`blend.fillet_solver_p2`; its corner lookup,
    /// `SmFilletExecutive::GetFilletCornerOfVertex`, is unit
    /// `blend.fillet_executive_p1`'s). A vertex without a corner
    /// (`SmFilletCorner::Create` returned `NULL`) yields `None`; upstream returns its
    /// raw `m_vVertexCornerMap` edge-use array cast to a corner (UB-209).
    fn solver_corner(&self, solver: O, index: usize) -> Result<Option<O>>;

    /// `SmFilletSolver::GetOtherFilletCorner(pCorner)`: delegate to the solver's
    /// `FilletSolver::other_fillet_corner` (`blend.fillet_solver_p3`), which reads the
    /// same corners as [`ExecutiveAccess::solver_corner`] (`None` when `corner` is not
    /// one of the solver's).
    fn solver_other_corner(&self, solver: O, corner: O) -> Result<Option<O>>;

    /// `SmFilletCorner::GetTangentFilletSolver(pFS)` (virtual): delegate to the already
    /// ported `blend::fillet_corner::p1::FilletCorner::tangent_fillet_solver` (fillet
    /// corner p1: the base, `SmFillet1x1Corner`, `SmFillet2x2Corner`; it dispatches the
    /// `SmFilletNx1ClosedCorner` / `SmFilletNx2Corner` overrides to fillet corner p2).
    fn corner_tangent_solver(&self, corner: O, solver: O) -> Result<Option<O>>;

    /// Context-aware `SmFilletSolver::GetFilletCorner(lIndex)` for adapters that look up
    /// corners in `executive`'s vertex-corner map.
    fn solver_corner_in(
        &self,
        _executive: &FilletExecutive<O>,
        solver: O,
        index: usize,
    ) -> Result<Option<O>> {
        self.solver_corner(solver, index)
    }

    /// Context-aware `SmFilletSolver::GetOtherFilletCorner(pCorner)`.
    fn solver_other_corner_in(
        &self,
        _executive: &FilletExecutive<O>,
        solver: O,
        corner: O,
    ) -> Result<Option<O>> {
        self.solver_other_corner(solver, corner)
    }

    /// Context-aware `SmFilletCorner::GetTangentFilletSolver(pFS)` for adapters that
    /// evaluate G1 continuity with `k` and resolve solvers in `executive`.
    fn corner_tangent_solver_in(
        &self,
        _k: &Kernel,
        _executive: &FilletExecutive<O>,
        corner: O,
        solver: O,
    ) -> Result<Option<O>> {
        self.corner_tangent_solver(corner, solver)
    }

    /// Target `EdgeId` of `pFS->GetEdgeuse(0)->GetEdge()` for `CreateSmoothEdgeSequence`.
    fn smooth_solver_edge0_id(&self, _solver: O) -> Result<crate::topology::EdgeId> {
        Err(SolidError::invalid_input(
            "ExecutiveAccess::smooth_solver_edge0_id",
            "adapter does not host target B-rep edges",
        ))
    }

    /// Borrow the target `Brep` for `CreateSmoothEdgeSequence`.
    fn smooth_target_brep(&self) -> Result<&crate::topology::Brep> {
        Err(SolidError::invalid_input(
            "ExecutiveAccess::smooth_target_brep",
            "adapter does not host a target B-rep",
        ))
    }

    /// Mutate the target `Brep` during `CreateSmoothEdgeSequence`.
    fn smooth_with_target_mut<T>(
        &mut self,
        _edges: &[crate::topology::EdgeId],
        _f: impl FnOnce(&mut crate::topology::Brep) -> Result<T>,
    ) -> Result<T> {
        Err(SolidError::invalid_input(
            "ExecutiveAccess::smooth_with_target_mut",
            "adapter does not host a target B-rep",
        ))
    }

    /// Set a joined curve on `edge` (`pE->SetCurve(pNewCurve, TRUE)`).
    fn smooth_set_edge_curve(
        &mut self,
        _k: &mut Kernel,
        _edge: crate::topology::EdgeId,
        _curve: crate::curve::curve::Curve,
    ) -> Result<()> {
        Err(SolidError::invalid_input(
            "ExecutiveAccess::smooth_set_edge_curve",
            "adapter does not host a target B-rep",
        ))
    }

    /// `SM_CAST_PTR(SmCurveBasedFS, solver)->GetOriginalCurve()`.
    fn smooth_solver_original_curve(
        &self,
        _solver: O,
    ) -> Result<Option<&crate::curve::curve::Curve>> {
        Ok(None)
    }

    /// `SM_CAST_PTR(SmCurveBasedFS, solver)->SetOriginalCurve(curve)`.
    fn smooth_set_solver_original_curve(
        &mut self,
        _solver: O,
        _curve: crate::curve::curve::Curve,
    ) -> Result<()> {
        Ok(())
    }

    /// `SM_CAST_PTR(SmVariableRadiusFS, solver)->SetFilletLawInterval(interval)`.
    fn smooth_set_solver_law_interval(
        &mut self,
        _solver: O,
        _interval: crate::geom::extent1d::Extent1d,
    ) -> Result<()> {
        Ok(())
    }

    /// `SmFilletCorner::GetSetBackDist()`: the inline read of `m_dSetBackDist`
    /// (`FilletCorner::set_back_dist`).
    fn corner_setback_distance(&self, corner: O) -> Result<f64>;

    /// `new SmLinearCrossSectionFSG` / `SmCircularCrossSectionFSG(bApproximateArcs,
    /// dTolPercentRadius, bMirror, bComplement)` / `SmBlendCurveCrossSectionFSG(
    /// dBlendScale, lContinuity)` in the executive context: store `generator` (the
    /// `blend.fillet_solver_p1` record) and return its handle.
    fn new_surface_generator(
        &mut self,
        k: &mut Kernel,
        generator: FilletSurfaceGenerator,
    ) -> Result<O>;

    /// `new SmConstantRadiusFS(ctx, dApproxTol, dAngTolRad, dTanTolRad, dRadius, pEU)`:
    /// delegate to `fillet_standard_solver::p1::constant_radius_from_edgeuse` on the
    /// B-rep of `edgeuse` (no second radius), with the executive's pseudo B-rep
    /// (`m_pPseudoBrep`, which receives the solver's fillet geometry) and the new
    /// solver's identity, and return the new solver's handle.
    fn new_constant_radius_solver(
        &mut self,
        k: &mut Kernel,
        approx_tol: f64,
        ang_tol_rad: f64,
        tan_tol_rad: f64,
        radius: f64,
        edgeuse: O,
    ) -> Result<O>;

    /// The fillet solver constructors `SetFilletParameters` and `SurfaceSurfaceFillet`
    /// call (`new SmConstantRadiusFS` / `SmConstantDistanceFS` / `SmVariableRadiusFS` /
    /// `SmSurfaceSurfaceFS`), dispatched on the spec, adding the new solver to the
    /// adapter's solver store and returning its handle.
    ///
    /// # Errors
    /// The constructor's.
    fn new_fillet_solver(
        &mut self,
        k: &mut Kernel,
        spec: &super::p2::SolverSpec<'_, O>,
    ) -> Result<O>;

    /// `SmFilletSolver::SetFilletSurfaceGenerator(pFSG)`: delegate to
    /// `FilletSolver::set_fillet_surface_generator` (`blend.fillet_solver_p1`) with
    /// the generator's record.
    fn set_surface_generator(&mut self, solver: O, generator: O) -> Result<()>;
}

impl<O, T> super::p1::SmoothSequenceAccess<O> for T
where
    O: ClassObject + SolutionObject + Ord,
    T: ExecutiveAccess<O> + ?Sized,
{
    fn smooth_edge_edgeuses(&self, edge: O) -> Result<Vec<O>> {
        self.edge_edgeuses(edge)
    }

    fn smooth_solver_edge(&self, solver: O) -> Result<O> {
        self.solver_edge(solver)
    }

    fn smooth_solver_corner(
        &self,
        executive: &FilletExecutive<O>,
        solver: O,
        index: usize,
    ) -> Result<Option<O>> {
        self.solver_corner_in(executive, solver, index)
    }

    fn smooth_solver_other_corner(
        &self,
        executive: &FilletExecutive<O>,
        solver: O,
        corner: O,
    ) -> Result<Option<O>> {
        self.solver_other_corner_in(executive, solver, corner)
    }

    fn smooth_corner_tangent_solver(
        &self,
        k: &Kernel,
        executive: &FilletExecutive<O>,
        corner: O,
        solver: O,
    ) -> Result<Option<O>> {
        self.corner_tangent_solver_in(k, executive, corner, solver)
    }

    fn smooth_solver_edge0_id(&self, solver: O) -> Result<crate::topology::EdgeId> {
        ExecutiveAccess::smooth_solver_edge0_id(self, solver)
    }

    fn smooth_target_brep(&self) -> Result<&crate::topology::Brep> {
        ExecutiveAccess::smooth_target_brep(self)
    }

    fn smooth_with_target_mut<R>(
        &mut self,
        edges: &[crate::topology::EdgeId],
        f: impl FnOnce(&mut crate::topology::Brep) -> Result<R>,
    ) -> Result<R> {
        ExecutiveAccess::smooth_with_target_mut(self, edges, f)
    }

    fn smooth_set_edge_curve(
        &mut self,
        k: &mut Kernel,
        edge: crate::topology::EdgeId,
        curve: crate::curve::curve::Curve,
    ) -> Result<()> {
        ExecutiveAccess::smooth_set_edge_curve(self, k, edge, curve)
    }

    fn smooth_solver_original_curve(
        &self,
        solver: O,
    ) -> Result<Option<&crate::curve::curve::Curve>> {
        ExecutiveAccess::smooth_solver_original_curve(self, solver)
    }

    fn smooth_set_solver_original_curve(
        &mut self,
        solver: O,
        curve: crate::curve::curve::Curve,
    ) -> Result<()> {
        ExecutiveAccess::smooth_set_solver_original_curve(self, solver, curve)
    }

    fn smooth_set_solver_law_interval(
        &mut self,
        solver: O,
        interval: crate::geom::extent1d::Extent1d,
    ) -> Result<()> {
        ExecutiveAccess::smooth_set_solver_law_interval(self, solver, interval)
    }
}

/// A mark lock's context identity and mark type; the adapter keeps its real
/// `MarkLock` alive around the callback passed to `with_marks`.
#[derive(Debug, Clone, Copy)]
pub struct ContextMark {
    /// Adapter context identity (not an address).
    pub context: usize,
    /// Locked mark type, to select the corresponding current mark.
    pub mark_type: crate::geom::context::MarkType,
}

/// Live operations consumed by the executive's ordered corner-vertex stage.
/// Full executive adapters delegate these operations without changing behavior.
pub trait CornerVertexAccess<O: ClassObject + SolutionObject + Ord> {
    /// Construct actual corner topology.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_make_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
    /// Actual corner metadata for failure reporting.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_error_info(
        &self,
        corner: O,
        executive: &FilletExecutive<O>,
    ) -> Result<CornerView<O>>;
    /// Actual blending predicate.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_is_blending(&self, corner: O) -> Result<bool>;
    /// Actual tangent predicate.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_is_tangent(&self, corner: O) -> Result<bool>;
    /// Ordered actual corner solvers, preserving native lookup behavior.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_solvers(&self, corner: O, executive: &FilletExecutive<O>) -> Result<Vec<O>>;
    /// Actual edge of the solver.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_solver_edge(&self, solver: O) -> Result<O>;
    /// Actual edge curve endpoints.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_curve_ends(&self, k: &Kernel, edge: O) -> Result<(Point3, Point3)>;
    /// Actual filleted vertex, including native null.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_corner_vertex(&self, corner: O) -> Result<Option<O>>;
    /// Actual target vertex point.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_point(&self, vertex: O) -> Result<Point3>;
    /// Actual corner approximation tolerance.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_approx_tolerance(&self, corner: O) -> Result<f64>;
    /// Write the actual constant-radius calculation flag.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_set_constant(&mut self, corner: O, value: bool) -> Result<()>;
    /// Calculate actual corner vertex geometry.
    ///
    /// # Errors
    /// Actual object resolution or operation failures.
    fn vertices_calculate(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
}

impl<O, A> CornerVertexAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn vertices_make_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::make_corner_topology(self, k, corner, executive)
    }
    fn vertices_error_info(
        &self,
        corner: O,
        executive: &FilletExecutive<O>,
    ) -> Result<CornerView<O>> {
        ExecutiveAccess::corner_error_info(self, corner, executive)
    }
    fn vertices_is_blending(&self, corner: O) -> Result<bool> {
        ExecutiveAccess::is_blending_corner(self, corner)
    }
    fn vertices_is_tangent(&self, corner: O) -> Result<bool> {
        ExecutiveAccess::is_tangent_corner(self, corner)
    }
    fn vertices_solvers(&self, corner: O, executive: &FilletExecutive<O>) -> Result<Vec<O>> {
        ExecutiveAccess::corner_solvers(self, corner, executive)
    }
    fn vertices_solver_edge(&self, solver: O) -> Result<O> {
        ExecutiveAccess::solver_edge(self, solver)
    }
    fn vertices_curve_ends(&self, k: &Kernel, edge: O) -> Result<(Point3, Point3)> {
        ExecutiveAccess::edge_curve_ends(self, k, edge)
    }
    fn vertices_corner_vertex(&self, corner: O) -> Result<Option<O>> {
        ExecutiveAccess::corner_vertex(self, corner)
    }
    fn vertices_point(&self, vertex: O) -> Result<Point3> {
        ExecutiveAccess::vertex_point(self, vertex)
    }
    fn vertices_approx_tolerance(&self, corner: O) -> Result<f64> {
        ExecutiveAccess::corner_approx_tolerance(self, corner)
    }
    fn vertices_set_constant(&mut self, corner: O, value: bool) -> Result<()> {
        ExecutiveAccess::set_calc_as_const_rad(self, corner, value)
    }
    fn vertices_calculate(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::calc_corner_vertices(self, k, corner, executive)
    }
}

/// Live operations for the post-vertex corner topology-adjustment stage.
pub trait CornerAdjustmentAccess<O: ClassObject + SolutionObject + Ord>:
    CornerVertexAccess<O>
{
    /// Check and fix the actual corner's degeneracy.
    ///
    /// # Errors
    /// Actual lookups or concrete degeneracy-mutation failures.
    fn adjustment_check_degeneracy(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<bool>;
    /// Invoke the corner's actual topology-adjustment virtual operation.
    ///
    /// # Errors
    /// Actual context or concrete adjustment failures.
    fn adjustment_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
}

impl<O, A> CornerAdjustmentAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn adjustment_check_degeneracy(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<bool> {
        ExecutiveAccess::check_degeneracy(self, k, corner, executive)
    }
    fn adjustment_topology(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::adjust_topology(self, k, corner, executive)
    }
}

/// Live operations for each corner's post-surface geometry stage.
pub trait CornerEdgeAccess<O: ClassObject + SolutionObject + Ord>: CornerVertexAccess<O> {
    /// Invoke actual tangent topology adjustment.
    ///
    /// # Errors
    /// Concrete context or tangent-adjustment failures.
    fn edges_adjust_tangent(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
    /// Calculate the actual corner's boundary edges.
    ///
    /// # Errors
    /// Native curve calculation or concrete context failures.
    fn edges_calculate(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
}

impl<O, A> CornerEdgeAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn edges_adjust_tangent(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::adjust_tangent_corner(self, k, corner, executive)
    }
    fn edges_calculate(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::calc_corner_edges(self, k, corner, executive)
    }
}

/// Actual solver operations for the executive's ordered rail-trimming stage.
pub trait RailTrimAccess<O: ClassObject + SolutionObject + Ord>: SolverGeometryAccess<O> {
    /// Trim every retained geometry's real rails through the actual solver.
    ///
    /// # Errors
    /// Native trim failures or actual context-resolution errors.
    fn rails_trim(
        &mut self,
        k: &mut Kernel,
        solver: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
}

impl<O, A> RailTrimAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn rails_trim(
        &mut self,
        k: &mut Kernel,
        solver: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::trim_rails(self, k, solver, executive)
    }
}

/// Actual operations for enabled rail classification and adjusted-corner repair.
pub trait RailClassificationAccess<O: ClassObject + SolutionObject + Ord>:
    SolverGeometryAccess<O> + CornerEdgeAccess<O>
{
    /// Classify the actual solver's rails and append its adjusted corners.
    ///
    /// # Errors
    /// Classification, recovery, or actual object-resolution failures.
    fn rails_classify(
        &mut self,
        k: &mut Kernel,
        solver: O,
        adjusted: &mut Vec<O>,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
    /// Reinitialize the actual adjusted corner's retained edges.
    ///
    /// # Errors
    /// Actual corner resolution or reinitialization failures.
    fn rails_reinitialize_corner(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
}

impl<O, A> RailClassificationAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn rails_classify(
        &mut self,
        k: &mut Kernel,
        solver: O,
        adjusted: &mut Vec<O>,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::test_rollover(self, k, solver, adjusted, executive)
    }
    fn rails_reinitialize_corner(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::reinitialize_corner_edges(self, k, corner, executive)
    }
}

/// Actual capabilities used by the complete intermediate-body constructor.
/// Every stage delegates its existing algorithm and the caller's real object stores.
pub trait BrepCreationAccess<O: ClassObject + SolutionObject + Ord>:
    FaceStitchAccess<O>
    + CornerAdjustmentAccess<O>
    + CornerEdgeAccess<O>
    + RailTrimAccess<O>
    + RailClassificationAccess<O>
{
    /// Execute the actual smooth-sequence prelude on this executive.
    ///
    /// # Errors
    /// Real smoothing or missing live tangent-collection context.
    fn creation_smooth(&mut self, k: &mut Kernel, executive: &mut FilletExecutive<O>)
        -> Result<()>;

    /// `SmFilletSolver::BallFitsItsFaces` (source change, WR-462, #1172) on this
    /// solver: whether its rolling ball fits the faces it rolls on. Only a live session
    /// has solvers to judge.
    ///
    /// # Errors
    /// The judging's own errors; the adapter over [`ExecutiveAccess`] has no solver
    /// geometry to judge.
    fn creation_ball_fits(&mut self, k: &mut Kernel, solver: O) -> Result<bool>;
}
impl<O, A> BrepCreationAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn creation_smooth(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::process_smooth_edge_sequences(self, k, executive)
    }

    fn creation_ball_fits(&mut self, _k: &mut Kernel, _solver: O) -> Result<bool> {
        Err(invalid("the ball-fit rule needs a live fillet session"))
    }
}

/// Actual surface-owner resolution and face shrinking after fillet construction.
pub trait FaceShrinkAccess<O: ClassObject + SolutionObject + Ord>:
    FaceConstructionAccess<O>
{
    /// Resolve the actual face owning this geometry's surface.
    /// # Errors
    /// Missing actual geometry or owning-body context.
    fn shrink_geometry_face(&self, geom: O) -> Result<Option<O>>;
    /// Shrink the actual face. The live Session uses native default options;
    /// the blanket adapter retains the legacy [`ExecutiveAccess::shrink_face`] options.
    /// # Errors
    /// Actual shrink-operation or notification-context errors.
    fn shrink_actual_face(&mut self, k: &mut Kernel, face: O) -> Result<()>;
}
impl<O, A> FaceShrinkAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn shrink_geometry_face(&self, geom: O) -> Result<Option<O>> {
        ExecutiveAccess::geom_face(self, geom)
    }
    fn shrink_actual_face(&mut self, k: &mut Kernel, face: O) -> Result<()> {
        ExecutiveAccess::shrink_face(self, k, face)
    }
}

/// Actual attribute attachment and stitching of the intermediate fillet body.
pub trait FaceStitchAccess<O: ClassObject + SolutionObject + Ord>:
    FaceConstructionAccess<O>
{
    /// Enumerate actual owning edges.
    /// # Errors
    /// Actual body resolution or traversal errors.
    fn stitch_edges(&self, body: O) -> Result<Vec<O>>;
    /// Attach the retained executive attribute to the actual edge.
    /// # Errors
    /// Actual edge or attribute resolution errors.
    fn stitch_attribute(&mut self, edge: O, attribute: Option<O>) -> Result<()>;
    /// Stitch using the actual caller's attributes and notification context.
    /// # Errors
    /// Genuine missing context or stitch-operation failures.
    fn stitch_body(&mut self, k: &mut Kernel, body: O, tolerance: f64) -> Result<StitchAttempt>;
    /// [`FaceStitchAccess::stitch_body`] keeping a fillet's own edge where a corner's
    /// patch meets it: the fork's stitch with `SmFilletPatchStitchCallback`
    /// (source fork@1072d1b, WR-546), whose patch faces are those
    /// `MakeFaceBrep` built on `corners`' surfaces.
    /// # Errors
    /// [`SolidError::Unported`] by default; as [`FaceStitchAccess::stitch_body`].
    fn stitch_body_keeping_fillet_edges(
        &mut self,
        k: &mut Kernel,
        body: O,
        tolerance: f64,
        corners: &[O],
    ) -> Result<StitchAttempt> {
        let _ = (k, body, tolerance, corners);
        Err(SolidError::Unported {
            upstream: "SmFilletPatchStitchCallback",
            unit: "blend.fillet_executive_p3",
        })
    }
}
impl<O, A> FaceStitchAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn stitch_edges(&self, body: O) -> Result<Vec<O>> {
        ExecutiveAccess::edges(self, body)
    }
    fn stitch_attribute(&mut self, edge: O, attribute: Option<O>) -> Result<()> {
        ExecutiveAccess::add_attribute(self, edge, attribute)
    }
    fn stitch_body(&mut self, k: &mut Kernel, body: O, tolerance: f64) -> Result<StitchAttempt> {
        ExecutiveAccess::stitch_faces(self, k, body, tolerance, None, false)
    }
}

/// Actual construction operations for the intermediate fillet faces.
pub trait FaceConstructionAccess<O: ClassObject + SolutionObject + Ord>:
    SolverGeometryAccess<O> + CornerVertexAccess<O>
{
    /// Allocate and retain the actual intermediate B-rep.
    ///
    /// # Errors
    /// Native construction or identity-allocation errors.
    fn faces_new_brep(&mut self, k: &mut Kernel) -> Result<O>;
    /// Read the actual body's zone tolerance.
    ///
    /// # Errors
    /// Actual body resolution errors.
    fn faces_tolerance(&self, k: &Kernel, body: O) -> Result<f64>;
    /// Apply native `SetTolerance` defaults to the actual intermediate body.
    ///
    /// # Errors
    /// Actual body resolution or tolerance-operation errors.
    fn faces_set_tolerance(&mut self, k: &mut Kernel, body: O, tolerance: f64) -> Result<()>;
    /// Enable editing on the actual intermediate B-rep.
    ///
    /// # Errors
    /// Actual body resolution errors.
    fn faces_enable_editing(&mut self, body: O) -> Result<()>;
    /// Calculate the actual corner patch when its native predicate requires one.
    ///
    /// # Errors
    /// Actual corner resolution or concrete patch-operation failures.
    fn faces_corner_geometry(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
    /// Read the actual retained solver's approximation tolerance.
    ///
    /// # Errors
    /// Actual solver resolution errors.
    fn faces_solver_tolerance(&self, solver: O) -> Result<f64>;
    /// Resolve the indexed entry in the actual solver's retained geometry list.
    ///
    /// # Errors
    /// Actual solver resolution errors.
    fn faces_solver_geom(&self, solver: O, index: usize) -> Result<Option<O>>;
    /// Build the actual geometry's face in the retained intermediate B-rep.
    ///
    /// # Errors
    /// Actual object resolution or face-construction errors.
    fn faces_make_face(
        &mut self,
        k: &mut Kernel,
        geom: O,
        body: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
}
impl<O, A> FaceConstructionAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn faces_new_brep(&mut self, k: &mut Kernel) -> Result<O> {
        ExecutiveAccess::new_brep(self, k)
    }
    fn faces_tolerance(&self, k: &Kernel, body: O) -> Result<f64> {
        ExecutiveAccess::tolerance(self, k, body, false)
    }
    fn faces_set_tolerance(&mut self, k: &mut Kernel, body: O, tolerance: f64) -> Result<()> {
        ExecutiveAccess::set_brep_tolerance(self, k, body, tolerance, true, true)
    }
    fn faces_enable_editing(&mut self, body: O) -> Result<()> {
        ExecutiveAccess::set_editing_enabled(self, body, true)
    }
    fn faces_corner_geometry(
        &mut self,
        k: &mut Kernel,
        corner: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::calc_corner_geom(self, k, corner, executive)
    }
    fn faces_solver_tolerance(&self, solver: O) -> Result<f64> {
        ExecutiveAccess::solver_approx_tolerance(self, solver)
    }
    fn faces_solver_geom(&self, solver: O, index: usize) -> Result<Option<O>> {
        ExecutiveAccess::solver_geom(self, solver, index)
    }
    fn faces_make_face(
        &mut self,
        k: &mut Kernel,
        geom: O,
        body: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::make_face_brep(self, k, geom, body, executive)
    }
}

/// Actual operations for the executive's ordered solver-geometry stage.
pub trait SolverGeometryAccess<O: ClassObject + SolutionObject + Ord> {
    /// Run the actual retained solver's geometry calculation.
    ///
    /// # Errors
    /// Native calculation or actual context-resolution failures.
    fn geometry_calculate(
        &mut self,
        k: &mut Kernel,
        solver: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()>;
    /// Resolve the actual original edge for native solver-failure metadata.
    ///
    /// # Errors
    /// Missing actual solver topology.
    fn geometry_edge(&self, solver: O) -> Result<O>;
    /// The fork's `SmFilletSolver::RailsFitTheirFaces` for `solver` (not upstream; opt-in
    /// `fixes.fillet_refuse_rails_leaving_faces`, WR-302): whether every rail of its
    /// untrimmed fillet stays on the face it rolls on. Only an accessor with a live
    /// pseudo-B-rep can tell.
    ///
    /// # Errors
    /// [`SolidError::Unported`] without live context; the classification's errors.
    fn geometry_rails_fit(
        &mut self,
        k: &mut Kernel,
        solver: O,
        executive: &FilletExecutive<O>,
    ) -> Result<bool> {
        let _ = (k, solver, executive);
        Err(SolidError::unported(
            "SmFilletSolver::RailsFitTheirFaces (fork)",
            "blend.fillet_executive_p3",
        ))
    }
    /// The fork's `SmFilletExecutive::FacesOutlastTheirBlends` over `solvers` (not
    /// upstream; opt-in `fixes.fillet_refuse_face_consuming_blends`, WR-320): `false`
    /// when the blends take the whole of a face they roll on, some at an angle to it.
    ///
    /// # Errors
    /// [`SolidError::Unported`] without live context; missing geometry.
    fn geometry_faces_outlast(&mut self, k: &mut Kernel, solvers: &[O]) -> Result<bool> {
        let _ = (k, solvers);
        Err(SolidError::unported(
            "SmFilletExecutive::FacesOutlastTheirBlends (fork)",
            "blend.fillet_executive_p3",
        ))
    }
    /// The fork's `SmFilletExecutive::BandsClearOtherRails` over `solvers` (not upstream;
    /// opt-in `fixes.fillet_refuse_overlapping_bands`, WR-542): `false` when a blend's
    /// rail runs inside the band another blend takes off a face they share, away from a
    /// corner that trims them.
    ///
    /// # Errors
    /// [`SolidError::Unported`] without live context; missing geometry.
    fn geometry_bands_clear_other_rails(&mut self, k: &mut Kernel, solvers: &[O]) -> Result<bool> {
        let _ = (k, solvers);
        Err(SolidError::unported(
            "SmFilletExecutive::BandsClearOtherRails (fork)",
            "blend.fillet_executive_p3",
        ))
    }
}

impl<O, A> SolverGeometryAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn geometry_calculate(
        &mut self,
        k: &mut Kernel,
        solver: O,
        executive: &mut FilletExecutive<O>,
    ) -> Result<()> {
        ExecutiveAccess::calc_fillet_geom(self, k, solver, executive)
    }
    fn geometry_edge(&self, solver: O) -> Result<O> {
        ExecutiveAccess::solver_edge(self, solver)
    }
}

/// The `SmStitch` switches passed to [`ExecutiveAccess::do_stitching`]. `Default` is
/// the configuration `DoFilleting` builds.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // upstream independent switches
pub struct StitchSettings {
    /// `m_bDoRegionNesting`.
    pub do_region_nesting: bool,
    /// `m_bValidateResult`.
    pub validate_result: bool,
    /// `m_bIgnoreProblems`.
    pub ignore_problems: bool,
    /// `m_bFastEdgeCompare`.
    pub fast_edge_compare: bool,
    /// `m_bSqueezeSmallEdges`.
    pub squeeze_small_edges: bool,
    /// `m_bSplitEdgesWithVertices`.
    pub split_edges_with_vertices: bool,
    /// `m_bMakingManifoldSolid`.
    pub making_manifold_solid: bool,
    /// `m_bRemoveLaminarSlivers`.
    pub remove_laminar_slivers: bool,
}

impl Default for StitchSettings {
    fn default() -> Self {
        Self {
            do_region_nesting: true,
            validate_result: true,
            ignore_problems: true,
            fast_edge_compare: false,
            squeeze_small_edges: false,
            split_edges_with_vertices: true,
            making_manifold_solid: true,
            remove_laminar_slivers: false,
        }
    }
}

impl<O: ClassObject + SolutionObject + Ord> FilletExecutive<O> {
    /// Execute edge filleting after parameter setup, with the upstream initialized
    /// check and default null error-info argument to `CreateFilletCorners`.
    pub fn do_fillet(
        &mut self,
        k: &mut Kernel,
        access: &mut impl ExecutiveAccess<O>,
    ) -> Result<()> {
        if !self.initialized {
            // The generic NoteError overload changes exactly these two members.
            self.error.code = 1; // SM_FILERR_EXEC_UNINITIALIZED
            self.error.comment = "ERROR: fillet parameters not set up.".into();
            return Err(invalid("fillet parameters not set up"));
        }
        access.create_corners(k, self, None)?;
        self.do_filleting(k, access, None)?;
        if !k.fixes.fillet_refuse_non_manifold_result {
            return Ok(());
        }
        self.check_closed_manifold(access)
    }

    /// Refuse a filleted target that is not a closed manifold solid (not upstream;
    /// opt-in `fixes.fillet_refuse_non_manifold_result`, WR-212,
    /// source issue #422). Upstream ignores the `bMaybeNotSolid` that
    /// `OrientTrimmedSurfaces` reports and returns success, so a round whose
    /// trimming breaks the shell came back as a valid body. The first edge of the
    /// target, in `GetEdges` order, that is not `SmEdge::IsManifold` is recorded in
    /// the error info and named in the error. The target keeps the merged
    /// topology, as on upstream's other merge failures.
    fn check_closed_manifold(&mut self, access: &impl ExecutiveAccess<O>) -> Result<()> {
        let target = self.target.ok_or(invalid("missing target B-rep"))?;
        for edge in access.edges(target)? {
            if access.edge_is_manifold(edge)? {
                continue;
            }
            self.error.code = 7; // SM_FILERR_BAD_MERGE
            self.error.status = 2; // SM_FIL_FAILURE
            self.error.edge = Some(edge);
            self.error.comment =
                "Fillet Merge Failure: Result Is Not A Closed Manifold Solid".into();
            return Err(SolidError::Upstream {
                operation: OP,
                code: 1001,
                entity: access.edge_entity(edge),
            });
        }
        Ok(())
    }

    fn record_corner_failure(&mut self, view: CornerView<O>, comment: &str) -> Result<()> {
        self.error.code = 3; // SM_FILERR_VERTEX_PROBLEM
        self.error.status = 2; // SM_FIL_FAILURE
        self.error.corner_type = view.corner_type;
        self.error.vertex = view.vertex;
        if !self.error.surfaces.contains(&None) {
            self.error.surfaces.push(None);
        }
        self.error.fillet_vertices = [None, None];
        self.error.comment = comment.into();
        self.error.entity_name = view.name;
        Err(failure())
    }

    fn record_solver_failure(&mut self, edge: O, status: i32, comment: &str) -> Result<()> {
        self.error.edge = Some(edge);
        self.error.code = 4; // SM_FILERR_EDGE_PROBLEM
        self.error.status = status;
        self.error.comment = comment.into();
        Err(failure())
    }

    /// Build topology and then vertex geometry for each corner in native order.
    ///
    /// This is the corner-vertex component of `CreateFilletBrep`; its smooth-sequence
    /// prelude and subsequent topology adjustment are separate operations.
    ///
    /// # Errors
    /// Actual lookup/operation failures, or the native corner failure with retained
    /// error metadata. Genuine missing dependencies propagate.
    pub fn create_corner_vertices(
        &mut self,
        k: &mut Kernel,
        access: &mut impl CornerVertexAccess<O>,
    ) -> Result<()> {
        let mut index = 0;
        while let Some(corner) = self.corners.get(index).copied() {
            if ignore(access.vertices_make_topology(k, corner, self))?.is_none() {
                return self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Unable to Make Topology",
                );
            }
            let mut as_constant = access.vertices_is_blending(corner)?;
            if as_constant && access.vertices_is_tangent(corner)? {
                let solvers = access.vertices_solvers(corner, self)?;
                if solvers.len() == 2 {
                    let solver = solvers
                        .first()
                        .copied()
                        .ok_or(invalid("missing first solver"))?;
                    let edge = access.vertices_solver_edge(solver)?;
                    let (start, end) = access.vertices_curve_ends(k, edge)?;
                    let vertex = access
                        .vertices_corner_vertex(corner)?
                        .ok_or(invalid("null tangent corner vertex"))?;
                    let point = access.vertices_point(vertex)?;
                    let distance1 = start.distance(point);
                    let distance2 = end.distance(point);
                    let tolerance = 2.0 * access.vertices_approx_tolerance(corner)?;
                    if distance1 > tolerance && distance2 > tolerance {
                        as_constant = false;
                    }
                }
            }
            access.vertices_set_constant(corner, as_constant)?;
            if ignore(access.vertices_calculate(k, corner, self))?.is_none() {
                return self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Unable to Compute Geometry",
                );
            }
            index += 1;
        }
        Ok(())
    }

    fn corner_vertex_failure(
        &mut self,
        access: &impl CornerVertexAccess<O>,
        corner: O,
        comment: &str,
    ) -> Result<()> {
        let view = access.vertices_error_info(corner, self)?;
        self.record_corner_failure(view, comment)
    }

    /// Adjust every corner's topology after all corner vertices are calculated.
    ///
    /// # Errors
    /// Actual context/degeneracy errors or native adjustment failure metadata.
    pub fn adjust_corner_topology(
        &mut self,
        k: &mut Kernel,
        access: &mut impl CornerAdjustmentAccess<O>,
    ) -> Result<()> {
        let mut index = 0;
        while let Some(corner) = self.corners.get(index).copied() {
            if !access.adjustment_check_degeneracy(k, corner, self)?
                && ignore(access.adjustment_topology(k, corner, self))?.is_none()
            {
                return self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Topology Adjustment Failure",
                );
            }
            index += 1;
        }
        Ok(())
    }

    /// Calculate every retained solver's geometry in native executive order.
    ///
    /// # Errors
    /// Genuine dependency errors or native surface-tracing failure metadata.
    pub fn calculate_solver_geometry(
        &mut self,
        k: &mut Kernel,
        access: &mut impl SolverGeometryAccess<O>,
    ) -> Result<()> {
        let mut index = 0;
        while let Some(solver) = self.solvers.get(index).copied() {
            if ignore(access.geometry_calculate(k, solver, self))?.is_none() {
                return self.record_solver_failure(
                    access.geometry_edge(solver)?,
                    10,
                    "Fillet Surface Computation Failure",
                );
            }
            index += 1;
        }
        Ok(())
    }

    /// Refuse blends whose rails leave the faces they roll on, after every solver's fillet
    /// geometry exists (not upstream; the fork's check in `CreateFilletBrep`, opt-in
    /// `fixes.fillet_refuse_rails_leaving_faces`, WR-302, source issue #841 and
    /// #1024): a solver whose rail leaves the face it rolls on between its ends
    /// (`SmFilletSolver::RailsFitTheirFaces`). Fails with `FilletErrorType::NoRoom` (8, not
    /// an upstream code).
    ///
    /// # Errors
    /// The refusal as `SM_ERR`, noted in the error info; those of the check.
    pub fn refuse_blends_larger_than_faces(
        &mut self,
        k: &mut Kernel,
        access: &mut impl SolverGeometryAccess<O>,
    ) -> Result<()> {
        if k.fixes.fillet_refuse_rails_leaving_faces {
            let mut index = 0;
            while let Some(solver) = self.solvers.get(index).copied() {
                if !access.geometry_rails_fit(k, solver, self)? {
                    return self.record_no_room(
                        "Fillet Error: a rail leaves the face it rolls on (the blend is larger than the faces allow)",
                    );
                }
                index += 1;
            }
        }
        Ok(())
    }

    /// Refuse blends that take the whole of a face they roll on, some meeting it at an angle
    /// (not upstream; opt-in `fixes.fillet_refuse_face_consuming_blends`, WR-320, #879:
    /// `SmFilletExecutive::FacesOutlastTheirBlends`). The fork calls it in `CreateFilletBrep`
    /// after the fillet faces are built and before they are stitched (the diff in #879),
    /// on the trimmed rails; here it runs once the geometry exists, which is earlier. The
    /// cylinder top case of the tests already fails in the port at the corner edges
    /// (`SM_FILERR_VERTEX_PROBLEM`) before the fork's call site, so a later call would
    /// lose the `NoRoom` status there (to be compared against C++). Fails with
    /// `FilletErrorType::NoRoom`.
    ///
    /// # Errors
    /// The refusal as `SM_ERR`, noted in the error info; those of the check.
    pub fn refuse_face_consuming_blends(
        &mut self,
        k: &mut Kernel,
        access: &mut impl SolverGeometryAccess<O>,
    ) -> Result<()> {
        if k.fixes.fillet_refuse_face_consuming_blends
            && !access.geometry_faces_outlast(k, &self.solvers)?
        {
            return self.record_no_room(
                "Fillet Error: the blends take the whole of a face they roll on, some at an angle to it (the blend is larger than the faces allow)",
            );
        }
        Ok(())
    }

    /// Refuse blends whose bands on a face they share overlap, away from any corner that
    /// trims them (not upstream; opt-in `fixes.fillet_refuse_overlapping_bands`, WR-542:
    /// the fork's source fork@c0d74ee, `SmFilletExecutive::BandsClearOtherRails`,
    /// called right after `FacesOutlastTheirBlends`). Fails with `FilletErrorType::NoRoom`.
    ///
    /// # Errors
    /// The refusal as `SM_ERR`, noted in the error info; those of the check.
    pub fn refuse_overlapping_bands(
        &mut self,
        k: &mut Kernel,
        access: &mut impl SolverGeometryAccess<O>,
    ) -> Result<()> {
        if k.fixes.fillet_refuse_overlapping_bands
            && !access.geometry_bands_clear_other_rails(k, &self.solvers)?
        {
            return self.record_no_room(
                "Fillet Error: a blend's rail runs inside the band another blend takes off a face they share (the blends are larger than the face allows)",
            );
        }
        Ok(())
    }

    /// `NoteError(NoRoom, comment)` (`NoRoom` is not an upstream code), then `SM_ERR`.
    fn record_no_room(&mut self, comment: &str) -> Result<()> {
        // The generic NoteError overload changes exactly these two members.
        self.error.code = FilletErrorType::NoRoom as i32;
        self.error.comment = comment.into();
        Err(failure())
    }

    /// Complete tangent adjustment, repeated vertex geometry and boundary edges per corner.
    ///
    /// # Errors
    /// Actual corner failures with native error metadata, or missing dependencies.
    pub fn calculate_corner_edges(
        &mut self,
        k: &mut Kernel,
        access: &mut impl CornerEdgeAccess<O>,
    ) -> Result<()> {
        let mut index = 0;
        while let Some(corner) = self.corners.get(index).copied() {
            if ignore(access.edges_adjust_tangent(k, corner, self))?.is_none() {
                return self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Topology Adjustment Failure",
                );
            }
            if ignore(access.vertices_calculate(k, corner, self))?.is_none() {
                return self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Vertex Computation Failure",
                );
            }
            if ignore(access.edges_calculate(k, corner, self))?.is_none() {
                return self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Corner Edge Computation Failure",
                );
            }
            index += 1;
        }
        Ok(())
    }

    /// Trim every solver's rails after all corner boundary geometry is complete.
    ///
    /// # Errors
    /// Native trim failure metadata or genuine missing dependencies.
    pub fn trim_solver_rails(
        &mut self,
        k: &mut Kernel,
        access: &mut impl RailTrimAccess<O>,
    ) -> Result<()> {
        let mut index = 0;
        while let Some(solver) = self.solvers.get(index).copied() {
            if ignore(access.rails_trim(k, solver, self))?.is_none() {
                return self.record_solver_failure(
                    access.geometry_edge(solver)?,
                    7,
                    "Fillet Surface Computation Failure: Unable to Trim Rail Curve",
                );
            }
            index += 1;
        }
        Ok(())
    }

    /// `GetPreviewFilletSurfaces` after its `CreateFilletCorners` (which the caller runs,
    /// as [`Self::do_fillet`]'s): every corner's topology and vertex geometry (without
    /// `CreateFilletBrep`'s smooth sequences and constant-radius choice), the corner
    /// topology adjustments, every solver's untrimmed geometry and the trim of its rails.
    /// The preview surfaces are then those of each retained solver's fillet geometries in
    /// order (`GetFilletGeoms`), which the caller reads.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` without a target; the stage failures of
    /// [`Self::create_fillet_brep`], with the same error info; `SM_ERR` without solvers.
    pub fn preview_fillet_geometry<A>(&mut self, k: &mut Kernel, access: &mut A) -> Result<()>
    where
        A: CornerAdjustmentAccess<O> + RailTrimAccess<O>,
    {
        // NER: upstream returns SM_ERR_NULL_POINTER, not invalid input.
        self.target.ok_or(SolidError::Upstream {
            operation: OP,
            code: 1006,
            entity: None,
        })?;
        // Process corners by calculating the vertices first.
        let mut index = 0;
        while let Some(corner) = self.corners.get(index).copied() {
            if ignore(access.vertices_make_topology(k, corner, self))?.is_none() {
                return self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Unable to Make Topology",
                );
            }
            if ignore(access.vertices_calculate(k, corner, self))?.is_none() {
                return self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Unable to Compute Geometry",
                );
            }
            index += 1;
        }
        self.adjust_corner_topology(k, access)?;
        self.calculate_solver_geometry(k, access)?;
        // bTrimRails: always
        self.trim_solver_rails(k, access)?;
        if self.solvers.is_empty() {
            return Err(SolidError::sm_err(OP));
        }
        Ok(())
    }

    /// Classify all retained solver rails, then repair adjusted corners in order.
    /// Disabled classification performs no solver or corner operations.
    ///
    /// # Errors
    /// Genuine missing capabilities propagate; ordinary operation failures retain
    /// the native solver/corner failure metadata and every completed mutation.
    pub fn classify_solver_rails(
        &mut self,
        k: &mut Kernel,
        access: &mut impl RailClassificationAccess<O>,
    ) -> Result<()> {
        self.classify_solver_rails_with_adjusted(k, access)
            .map(|_| ())
    }

    /// Run the same classification stage and return its actual ordered adjusted corners.
    /// The returned IDs name retained corners; their repairs have already completed.
    ///
    /// # Errors
    /// The same failures and retained mutation prefixes as [`Self::classify_solver_rails`].
    pub fn classify_solver_rails_with_adjusted(
        &mut self,
        k: &mut Kernel,
        access: &mut impl RailClassificationAccess<O>,
    ) -> Result<Vec<O>> {
        let mut adjusted = Vec::new();
        if self.options.classification {
            let mut index = 0;
            while let Some(solver) = self.solvers.get(index).copied() {
                if ignore(access.rails_classify(k, solver, &mut adjusted, self))?.is_none() {
                    self.record_solver_failure(
                        access.geometry_edge(solver)?,
                        7,
                        "Fillet Solver Error: Unable to Process Rollover",
                    )?;
                }
                index += 1;
            }
            for corner in adjusted.iter().copied() {
                ignore(access.rails_reinitialize_corner(k, corner, self))?;
                if ignore(access.edges_calculate(k, corner, self))?.is_none() {
                    self.corner_vertex_failure(
                        access,
                        corner,
                        "Fillet Corner Error: Edge Computation Failure After Rollover Adjustment",
                    )?;
                }
            }
        }
        Ok(adjusted)
    }

    /// Allocate the intermediate B-rep and build every corner and solver face in native order.
    /// Classification and stitching are separate executive stages.
    ///
    /// # Errors
    /// Native operation failures with retained metadata, or genuine missing capabilities.
    pub fn build_fillet_faces(
        &mut self,
        k: &mut Kernel,
        access: &mut impl FaceConstructionAccess<O>,
    ) -> Result<O> {
        let target = self.target.ok_or(SolidError::Upstream {
            operation: OP,
            code: 1006,
            entity: None,
        })?;
        let fillet = access.faces_new_brep(k)?;
        self.fillet = Some(fillet);
        let tolerance = access.faces_tolerance(k, target)?;
        ignore(access.faces_set_tolerance(k, fillet, tolerance))?;
        access.faces_enable_editing(fillet)?;
        let mut index = 0;
        while let Some(corner) = self.corners.get(index).copied() {
            if ignore(access.faces_corner_geometry(k, corner, self))?.is_none() {
                self.corner_vertex_failure(
                    access,
                    corner,
                    "Fillet Corner Error: Corner Geometry Computation Failure",
                )?;
            }
            index += 1;
        }
        let mut index = 0;
        while let Some(solver) = self.solvers.get(index).copied() {
            let tolerance = access.faces_solver_tolerance(solver)?;
            if access.faces_tolerance(k, fillet)? < tolerance
                && access.faces_tolerance(k, fillet)? < tolerance
            {
                // SM_USE_OLDTOL: repeated comparison and full SetTolerance defaults.
                // SmBrep.h:700 implicitly constructs SmZoneTol3d at this call.
                let tolerance = zone_tol3d(k, tolerance);
                ignore(access.faces_set_tolerance(k, fillet, tolerance))?;
            }
            let mut geom_index = 0;
            while let Some(geom) = access.faces_solver_geom(solver, geom_index)? {
                if ignore(access.faces_make_face(k, geom, fillet, self))?.is_none() {
                    self.record_solver_failure(
                        access.geometry_edge(solver)?,
                        2,
                        "Fillet Surface Computation Failure: Fillet Face Creation Error",
                    )?;
                }
                geom_index += 1;
            }
            index += 1;
        }
        Ok(fillet)
    }

    /// Attach the actual executive edge attribute, then stitch the intermediate body.
    /// Counts and gaps are returned only after a successful stitch.
    ///
    /// # Errors
    /// Native stitching failure with its original executive error mutation, or a genuine
    /// missing dependency. Attribute and topology mutations preceding failure remain.
    pub fn stitch_fillet_faces(
        &mut self,
        k: &mut Kernel,
        access: &mut impl FaceStitchAccess<O>,
        fillet: O,
    ) -> Result<StitchAttempt> {
        for edge in access.stitch_edges(fillet)? {
            ignore(access.stitch_attribute(edge, self.edge_attribute))?;
        }
        let tolerance = access.faces_tolerance(k, fillet)?;
        // Opt-in, not upstream (`fixes.fillet_patch_keeps_fillet_edges`, WR-546, #1377;
        // source fork@1072d1b): where a corner's patch meets a fillet, the
        // fillet's edge is kept (`SmFilletPatchStitchCallback`).
        let mut attempt = if k.fixes.fillet_patch_keeps_fillet_edges {
            access.stitch_body_keeping_fillet_edges(k, fillet, tolerance, &self.corners)?
        } else {
            access.stitch_body(k, fillet, tolerance)?
        };
        if let Some(error) = attempt.status.take() {
            if error.is_unported() {
                return Err(error);
            }
            self.error.code = 7; // SM_FILERR_BAD_MERGE
            self.error.comment =
                "Fillet Stitching Failure: Error Stitching Fillets Together".into();
            return Err(failure());
        }
        Ok(attempt)
    }

    /// Construct and stitch the fillet shell, preserving every stage after each
    /// missing dependency call. `error_out` is assigned only on success.
    pub fn create_fillet_brep(
        &mut self,
        k: &mut Kernel,
        access: &mut impl BrepCreationAccess<O>,
        error_out: Option<&mut ErrorInfo<O>>,
    ) -> Result<()> {
        // NER: upstream returns SM_ERR_NULL_POINTER, not invalid input.
        self.target.ok_or(SolidError::Upstream {
            operation: OP,
            code: 1006,
            entity: None,
        })?;
        ignore(access.creation_smooth(k, self))?;

        // Opt-in, not upstream (`fixes.fillet_refuse_ball_not_fitting`, WR-462, #1172):
        // a blend whose rolling ball does not fit a face it rolls on has a rail turned
        // back on itself; a corner where it ends looks for it where it cannot be.
        if k.fixes.fillet_refuse_ball_not_fitting {
            let mut index = 0;
            while let Some(solver) = self.solvers.get(index).copied() {
                if !access.creation_ball_fits(k, solver)? {
                    self.error.code = FilletErrorType::NoRoom as i32;
                    self.error.comment = "Fillet Error: a rail turns back on itself: the blend is wider than the face's curve allows (its rolling ball does not fit)".into();
                    return Err(failure());
                }
                index += 1;
            }
        }

        self.create_corner_vertices(k, access)?;
        self.adjust_corner_topology(k, access)?;
        self.calculate_solver_geometry(k, access)?;
        self.refuse_blends_larger_than_faces(k, access)?;
        self.refuse_face_consuming_blends(k, access)?;
        self.refuse_overlapping_bands(k, access)?;
        self.calculate_corner_edges(k, access)?;
        self.trim_solver_rails(k, access)?;
        self.classify_solver_rails(k, access)?;
        let fillet = self.build_fillet_faces(k, access)?;
        self.stitch_fillet_faces(k, access, fillet)?;
        if let Some(output) = error_out {
            output.assign(&self.error);
        }
        Ok(())
    }

    /// Shrink the surface-owner face of the first geometry in each actual solver.
    /// Ordinary native errors are ignored; genuine missing dependencies propagate.
    ///
    /// # Errors
    /// Actual object-resolution errors or genuine missing shrink context.
    pub fn shrink_fillet_faces(
        &mut self,
        k: &mut Kernel,
        access: &mut impl FaceShrinkAccess<O>,
    ) -> Result<()> {
        let mut index = 0;
        while let Some(solver) = self.solvers.get(index).copied() {
            if let Some(geom) = access.faces_solver_geom(solver, 0)? {
                if let Some(face) = access.shrink_geometry_face(geom)? {
                    ignore(access.shrink_actual_face(k, face))?;
                }
            }
            index += 1;
        }
        Ok(())
    }

    /// Build the fillet B-rep and merge it using the selected strategy. The context
    /// Boolean flag and target editing flag are restored on every return.
    pub fn do_filleting(
        &mut self,
        k: &mut Kernel,
        access: &mut impl ExecutiveAccess<O>,
        mut error_out: Option<&mut ErrorInfo<O>>,
    ) -> Result<()> {
        let doing_boolean = k.context.doing_boolean();
        k.context.set_doing_boolean(true);
        self.intersector.doing_fillets = true;
        let result = (|| {
            if ignore(self.create_fillet_brep(k, access, error_out.as_deref_mut()))?.is_none() {
                return Err(failure());
            }
            let fillet = self
                .fillet
                .ok_or(invalid("missing constructed fillet B-rep"))?;
            if access.faces(fillet)?.is_empty() {
                return Err(failure());
            }
            self.shrink_fillet_faces(k, access)?;
            let target = self.target.ok_or(invalid("missing target B-rep"))?;
            let editing = access.editing_enabled(target)?;
            access.set_editing_enabled(target, true)?;
            let result = self.merge_fillet(k, access, target, fillet, error_out);
            let restore = access.set_editing_enabled(target, editing);
            result.and(restore)
        })();
        k.context.set_doing_boolean(doing_boolean);
        result
    }

    /// Insert solver geometries, then corners, in the native retained-list order.
    /// The caller establishes the actual intersector bodies and edge map first.
    /// # Errors
    /// The first geometry/corner insertion error, retaining all earlier mutations.
    pub fn insert_intersection_topology(
        &mut self,
        k: &mut Kernel,
        access: &mut impl TopologyInsertionAccess<O>,
    ) -> Result<()> {
        if self.options.topology_insertion {
            let mut index = 0;
            while let Some(solver) = self.solvers.get(index).copied() {
                // GetFilletGeoms copies the array before its inner loop.
                for geom in access.insertion_solver_geoms(solver)? {
                    access.insertion_geom_topology(k, geom, self)?;
                }
                index += 1;
            }
            let mut index = 0;
            while let Some(corner) = self.corners.get(index).copied() {
                access.insertion_corner_topology(k, corner, self)?;
                index += 1;
            }
        }
        Ok(())
    }

    fn merge_fillet(
        &mut self,
        k: &mut Kernel,
        access: &mut impl ExecutiveAccess<O>,
        target: O,
        fillet: O,
        error_out: Option<&mut ErrorInfo<O>>,
    ) -> Result<()> {
        self.intersector.set_primary_brep(Some(target));
        self.intersector.set_other_brep(Some(fillet));
        if self.options.piecewise_merge {
            access.piecewise_merge(k, self)?;
            if let Some(output) = error_out {
                output.assign(&self.error);
            }
            return Ok(());
        }
        access.set_fillet_brep_edge_map(k, self)?;
        self.insert_intersection_topology(k, access)?;
        // Upstream cbi_MAP_TOPO_EDGES=0: do not collect mapped intersections.
        if self.options.global_merge {
            self.intersector
                .set_this_approx_tol_3d(access.tolerance(k, target, true)?);
            access.intersect_insert_relate(k, &mut self.intersector)?;
        }
        if self.options.trimming {
            let mut mates = Vec::new();
            for vertex in access.vertices(target)? {
                if let Some(other) = self.intersector.other_mate(Some(vertex)) {
                    mates.push((other, vertex)); // fillet mark first, target second
                }
            }
            for edge in access.edges(target)? {
                if let Some(other) = self.intersector.other_mate(Some(edge)) {
                    mates.push((edge, other)); // original first for edges (C++:3311)
                }
            }
            access.with_marks(k, target, fillet, &mates, |access, k, marks| {
                self.trim_and_merge(k, access, target, fillet, marks)
            })?;
        }
        // The final spine-edge scan only emits SM_DBG_WARN and cannot change state
        // or status in this release build, so there is no geometry dependency here.
        if let Some(output) = error_out {
            output.assign(&self.error);
        }
        Ok(())
    }

    /// Select faces in native solver and geometry order under the caller's live mark scope.
    ///
    /// # Errors
    /// The first geometry failure; previously accumulated faces and marks remain.
    pub fn select_original_surfaces<F>(
        &mut self,
        k: &mut Kernel,
        access: &mut impl OriginalSurfaceTrimAccess<O, F>,
        kept: &mut Vec<F>,
        deleted: &mut Vec<F>,
    ) -> Result<()> {
        let mut index = 0;
        while let Some(solver) = self.solvers.get(index).copied() {
            for geom in access.original_trim_geometries(solver)? {
                access.original_trim_geometry(k, geom, kept, deleted, self)?;
            }
            index += 1;
        }
        Ok(())
    }

    /// Append unkept fillet faces and delete the combined qualified list.
    /// The caller keeps its original mark scope alive across this operation.
    ///
    /// # Errors
    /// The original face traversal or deletion error; list and topology prefixes remain.
    pub fn delete_original_surface_faces(
        &mut self,
        k: &mut Kernel,
        access: &mut impl OriginalSurfaceDeletionAccess<O>,
        fillet: O,
        kept: &[O],
        deleted: &mut Vec<O>,
    ) -> Result<()> {
        for face in access.original_managed_faces(fillet)? {
            if !kept.contains(&face) && !deleted.contains(&face) {
                deleted.push(face);
            }
        }
        access.original_delete_faces(k, &mut self.intersector, deleted)
    }

    /// Tag eligible surviving `OnEdge` edges, then merge the intermediate body.
    ///
    /// `attributes` owns every allocated marker even if attachment or merge fails.
    /// The caller retains them through subsequent stitching and orientation, then
    /// releases them on every exit. This component does not delete either body.
    ///
    /// # Errors
    /// Returns traversal, allocation, genuine missing-capability, or merge errors;
    /// already appended attributes and partial body edits remain available.
    pub fn tag_and_merge_fillet_faces(
        &mut self,
        k: &mut Kernel,
        access: &mut impl FilletTagMergeAccess<O>,
        target: O,
        fillet: O,
        attributes: &mut Vec<O>,
    ) -> Result<()> {
        let edges = access.tag_merge_edges(fillet)?;
        for edge in self.topo_edges.iter().flatten().copied() {
            let view = access.tag_merge_edge(edge)?;
            if !view.on_edge || !view.has_curve {
                continue;
            }
            if let Some(edge) = view.brep_edge.filter(|edge| edges.contains(edge)) {
                let attribute = access.tag_merge_attribute(k)?;
                attributes.push(attribute);
                ignore(access.tag_merge_attach(edge, attribute))?;
            }
        }
        // Upstream cbi_MAP_TOPO_EDGES=0: no attributes on extra mated edges.
        access.tag_merge_bodies(k, target, fillet)
    }

    /// The tail of `DoFilleting` after `MergeBrep` (`SmFilletExecutive.cpp` 3532-3644):
    /// clear the intersector's fillet references, delete the fillet B-rep, stitch
    /// up to five times (doubling the tolerance), remap the surviving tagged edges
    /// into `topo_edges`, `RemoveTopoEdges`, `BooleanPostProcess(TRUE)`, notify every
    /// target face `PreEdit` and orient the target. `attributes` are the temporary
    /// tags of [`Self::tag_and_merge_fillet_faces`]; the caller destroys them after
    /// this returns, on every result (upstream `SmObjsDelete`).
    ///
    /// # Errors
    /// Deletion, stitching (after the last try), removal and post-process failures;
    /// ordinary notification and orientation failures are ignored as upstream.
    pub fn finish_merged_fillet(
        &mut self,
        k: &mut Kernel,
        access: &mut impl FilletFinishAccess<O>,
        target: O,
        fillet: O,
        attributes: &[O],
    ) -> Result<()> {
        if self.intersector.other_brep() == Some(fillet) {
            self.intersector.set_other_brep(None);
        }
        if self.intersector.primary_brep() == Some(fillet) {
            self.intersector.set_primary_brep(None);
        }
        access.finish_delete_brep(k, fillet)?;
        self.fillet = None;
        let mut tolerance = access.finish_tolerance(k, target)?;
        for attempt in 0..5 {
            let stitch =
                access.finish_do_stitching(k, target, tolerance, StitchSettings::default())?;
            if stitch.status.is_none() && stitch.lamina == 0 {
                break;
            }
            if let Some(error) = &stitch.status {
                if error.is_unported() {
                    return Err(error.clone());
                }
                if attempt == 4 {
                    self.error.code = 7; // SM_FILERR_BAD_MERGE
                    self.error.comment =
                        "Fillet Stitching Failure: Error Stitching Fillets To Original Brep".into();
                    return Err(failure());
                }
            }
            tolerance *= 2.0;
        }
        self.intersector.set_primary_brep(Some(target));
        self.topo_edges.clear();
        for attribute in attributes {
            if let Some(user) = access.finish_attribute_users(*attribute)?.first().copied() {
                self.topo_edges.push(Some(user));
            }
        }
        // Upstream cbi_MAP_TOPO_EDGES=0: do not collect mapped intersections.
        access.finish_remove_topo_edges(k, self)?;
        access.finish_boolean_post_process(k, self, true)?;
        for face in access.finish_faces(target)? {
            // SmTypes.h:659: SM_NO_GET_BREP(face) expands to a null object.
            ignore(access.finish_notify_face(
                k,
                face,
                NotifyOperation::PreEdit,
                Some(face),
                None,
                None,
            ))?;
        }
        let mut maybe_not_solid = false;
        ignore(access.finish_orient_trimmed_surfaces(
            k,
            target,
            true,
            &mut maybe_not_solid,
            false,
        ))?;
        Ok(())
    }

    fn trim_and_merge(
        &mut self,
        k: &mut Kernel,
        access: &mut impl ExecutiveAccess<O>,
        target: O,
        fillet: O,
        marks: &[ContextMark],
    ) -> Result<()> {
        let mut deleted = Vec::new();
        let mut kept = Vec::new();
        self.select_original_surfaces(
            k,
            &mut IndexedOriginalTrim { access, marks },
            &mut kept,
            &mut deleted,
        )?;
        self.delete_original_surface_faces(k, access, fillet, &kept, &mut deleted)?;
        let mut attributes = Vec::new();
        let result = (|| {
            self.tag_and_merge_fillet_faces(k, access, target, fillet, &mut attributes)?;
            self.finish_merged_fillet(k, access, target, fillet, &attributes)
        })();
        // SmObjsDelete lives through orientation; also clean up on errors/deferrals.
        let mut cleanup = Ok(());
        for attribute in attributes {
            let deleted = access.delete_attribute(attribute);
            if cleanup.is_ok() {
                cleanup = deleted;
            }
        }
        result.and(cleanup)
    }
}

#[cfg(test)]
#[path = "p3_tests.rs"]
mod tests;
