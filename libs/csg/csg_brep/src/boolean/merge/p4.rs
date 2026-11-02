//! Manifold Boolean orchestration, face classification and region bookkeeping.
//!
//! Upstream: `source/SMLib/src/SmMerge.cpp` (2510–5099),
//! `source/SMLib/inc/SmMerge.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`manifold_boolean`] | `SmMerge::ManifoldBoolean` |
//! | [`ManifoldState`] | the intersection and extracted-component members of `SmMerge` |
//! | [`ManifoldIntersector`] | calls through `SmMerge::m_vTI` and shared merge stages |
//!
//! Deviations: operand-local arena ids replace pointers; nullable owned B-reps
//! express deletion. The kernel adapter reuses `MergeState<Relations>` and the
//! ported merge stages; it maintains no duplicate maps. Both operand kernels
//! are explicit; `None` for the second kernel means a shared context. Contexts are temporarily moved
//! together only for the existing paired mark-lock API. No input-driven recursion
//! is introduced; the two retry loops retain upstream's ten-attempt limits.
//! Global cache queue limits are absent (the port has no global caches).
//! The temporary origin shift goes through `merge_p1`, before any topology
//! mutation. Its inverse token spans the entire operation,
//! including error/imprint exits, extraction and deletion of either operand.
//! The complete subsequent stages remain here.
//! Component face copies use `brep::p7::copy_faces` with the native merge defaults.
//! Opt-in fixes (`Kernel::fixes`, off by default, so the default is SMLib's):
//! with `slice_coincident_faces_sheet_side`, `SM_BO_SLICE` gets its own coincident-face
//! rule in steps 7.3e and 7.B for a primary face bounding material (keep the pair on the
//! sheet's `SM_OT_SAME` side only) instead of upstream's difference rule, which keeps a
//! face lying in the sheet in both parts (issue #528, WR-254). With
//! `boolean_outer_shell_first`, after step 8 a Union, Difference or Intersection whose
//! result is a manifold solid with a nested shell runs
//! `SetRegionIsVoidFlagsForNestedSolids`, so every region's outer shell is first (and
//! its void flags follow the nesting): a union that seals a void left the void's shell
//! first and failed `SmBrep::AssertValid` test 12 (the fork's fix,
//! source fork `abastran/void-shell-check`; WR-265,
//! `plan/upstream-issues/WR-265--manifold-boolean-void-shell-first.md`).

use super::p1::{self, MergeOptions};
use crate::error::{Result, SolidError};
use crate::geom::a_object::BooleanOperation;
use crate::geom::attribute::{
    ids, Attribute, AttributeStore, Behavior, MergeRegionData, ObjectKey,
};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::{MarkLock, MarkType};
use crate::intersect::point_class::PointClassType;
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::BrepQuery;
use crate::topology::brep::p1 as bp;
use crate::topology::edgeuse::p1 as eu;
use crate::topology::face::p2 as face;
use crate::topology::topology_traverser::{self as traverser, Entity, Marks};
use crate::topology::{edge, vertex, Brep, EdgeId, FaceId, FaceuseId, RegionId, VertexId};
use crate::EntityRef;

mod arena_pair;
mod kernel_adapter;
mod notification_keys;
pub(crate) use kernel_adapter::{
    adopt_namespace_owners, entity_of_notification_key, notification_key,
};
pub use kernel_adapter::{KernelManifoldIntersector, ManifoldKernelAdapter};
pub use notification_keys::ManifoldNotificationKeys;

const OP: &str = "boolean::merge::p4::manifold_boolean";

/// State written by `ManifoldBoolean`, retained across calls as upstream members.
#[derive(Debug)]
pub struct ManifoldState {
    /// Primary edges directly involved in the intersection, before later copying.
    pub intersection_edges: Vec<EdgeId>,
    /// Primary vertices directly involved in the intersection.
    pub intersection_vertices: Vec<VertexId>,
    /// Primary common vertices, paired in order with `other_vertices`.
    pub vertices: Vec<VertexId>,
    /// Other common vertices.
    pub other_vertices: Vec<VertexId>,
    /// Primary common edges, paired in order with `other_edges`.
    pub edges: Vec<EdgeId>,
    /// Other common edges.
    pub other_edges: Vec<EdgeId>,
    /// Whether every common vertex has a common-edge count other than one.
    pub intersection_loop_closed: bool,
    /// One copied B-rep per connected primary face set for `ExtractSeparate`.
    pub primary_components: Vec<Brep>,
    /// One copied B-rep per connected other face set for `ExtractSeparate`.
    pub other_components: Vec<Brep>,
    /// Traversal marks of the primary operand.
    pub primary_marks: Marks,
    /// Traversal marks of the other operand.
    pub other_marks: Marks,
}

impl Default for ManifoldState {
    fn default() -> Self {
        Self {
            intersection_edges: Vec::new(),
            intersection_vertices: Vec::new(),
            vertices: Vec::new(),
            other_vertices: Vec::new(),
            edges: Vec::new(),
            other_edges: Vec::new(),
            intersection_loop_closed: true,
            primary_components: Vec::new(),
            other_components: Vec::new(),
            primary_marks: Marks::default(),
            other_marks: Marks::default(),
        }
    }
}

/// The two kernels of a Boolean, or one shared kernel when `other` is `None`.
pub struct BooleanKernels<'a> {
    /// Primary operand context, tolerances and caches.
    pub primary: &'a mut Kernel,
    /// Independently owned other context; `None` means the primary context.
    pub other: Option<&'a mut Kernel>,
}

impl BooleanKernels<'_> {
    fn other(&self) -> &Kernel {
        self.other.as_deref().unwrap_or(self.primary)
    }

    pub(crate) fn mark_action<T>(
        &mut self,
        action: impl FnOnce(&mut [crate::geom::context::Context]) -> Result<T>,
    ) -> Result<T> {
        if let Some(other) = self.other.as_deref_mut() {
            let mut contexts = [
                std::mem::take(&mut self.primary.context),
                std::mem::take(&mut other.context),
            ];
            let result = action(&mut contexts);
            let [first, second] = contexts;
            self.primary.context = first;
            other.context = second;
            result
        } else {
            action(std::slice::from_mut(&mut self.primary.context))
        }
    }

    fn doing_boolean(&mut self, value: bool) {
        self.primary.context.set_doing_boolean(value);
        if let Some(other) = self.other.as_deref_mut() {
            other.context.set_doing_boolean(value);
        }
    }
}

/// Outputs of `SmMerge::MakeFaceuse`; new faces remain ordered.
#[derive(Debug, Default)]
pub struct MadeFaceuse {
    /// Existing primary region receiving the new face.
    pub old_region: Option<RegionId>,
    /// New region, if inserting the face split a region.
    pub new_region: Option<RegionId>,
    /// Newly made primary faces.
    pub faces: Vec<FaceId>,
}

/// Outputs of `SmTopologyIntersector::FixGaps`.
#[derive(Debug, Default)]
pub struct FixedGaps {
    /// Whether the marking loop must restart.
    pub modified_topology: bool,
    /// Deleted primary vertices to remove from saved intersection lists.
    pub deleted_vertices: Vec<VertexId>,
    /// Deleted primary edges to remove from saved intersection lists.
    pub deleted_edges: Vec<EdgeId>,
}

/// Interface for calls into the canonical intersector and other merge units.
/// [`KernelManifoldIntersector`] implements the ported stages using one
/// `p1::MergeState<Relations>`, shared notification records and caller attributes.
/// Defaults replace only calls into genuinely unported dependencies.
pub trait ManifoldIntersector {
    /// `SmBrep::Transform` for the temporary scope's surviving operands. The
    /// adapter supplies notification records and selects the operand's live
    /// kernel from `other`; attributes are the same store used during the body.
    fn transform_brep(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        brep: &mut Brep,
        other: bool,
        attributes: &mut AttributeStore,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        scale_tolerance: bool,
    ) -> Result<()>;

    /// `CopyFaces` for extracted components, with native `use_merge=TRUE`.
    fn copy_component_faces(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        source: &mut Brep,
        other: bool,
        faces: &[FaceId],
        destination: &mut Brep,
        attributes: &mut AttributeStore,
    ) -> Result<()>;

    /// `GetMappedBrepObjects(SmEdge_TYPE)` before later topology copies.
    fn mapped_edges(&self) -> Result<Vec<EdgeId>>;
    /// `GetMappedBrepObjects(SmVertex_TYPE)` before later topology copies.
    fn mapped_vertices(&self) -> Result<Vec<VertexId>>;
    /// `GetCommonEdges`, in primary B-rep list order, paired with other mates.
    fn common_edges(&self, primary: &Brep, other: &Brep) -> Result<(Vec<EdgeId>, Vec<EdgeId>)>;
    /// `GetCommonVertices`, in primary B-rep list order.
    fn common_vertices(
        &self,
        primary: &Brep,
        other: &Brep,
    ) -> Result<(Vec<VertexId>, Vec<VertexId>)>;
    /// `GetOtherMate` for a primary edge.
    fn edge_mate(&self, edge: EdgeId) -> Option<EdgeId>;
    /// `GetOtherMate` for a primary face.
    fn face_mate(&self, face: FaceId) -> Option<FaceId>;
    /// `Relate` for coincident faces (the `topology_intersector_p1` port).
    fn relate_faces(
        &mut self,
        primary: &mut Brep,
        face: FaceId,
        other: &mut Brep,
        other_face: FaceId,
    ) -> Result<()>;
    /// `FixFoundFaceCoincidence`: adapter target is `merge_p1` with the live edge maps;
    /// a mismatch runs `MergeCoincidentSurfaces` on the operands with these kernels and
    /// the attributes its edits notify.
    fn fix_face_coincidence(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
        face: FaceId,
        other_face: FaceId,
    ) -> Result<(Vec<FaceId>, Vec<FaceId>, bool)>;
    /// `ProcessWiresAndShellVertices`: adapter target is `merge_p5` using the same mates.
    /// Build `VertexCopy` with these caller attributes and the adapter's records;
    /// no separate attribute store may be retained by the adapter.
    fn process_wires(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        state: &mut ManifoldState,
        attributes: &mut AttributeStore,
        operation: BooleanOperation,
        primary_mark: MarkType,
        other_mark: MarkType,
        keep: &mut Vec<EdgeId>,
    ) -> Result<()>;
    /// `sm_FindBrepRegionOfOtherFace`: adapter target is `merge_p1` with live edge maps.
    fn find_region_of_other_face(
        &mut self,
        k: &Kernel,
        primary: &mut Brep,
        other: &mut Brep,
        faceuse: FaceuseId,
    ) -> Result<Option<RegionId>>;
    /// `PropagateOtherSrcRegionAttribs`: adapter target is `merge_p1` with source lookups
    /// and the contexts' notification handlers. Its ordinary status is ignored.
    fn propagate_regions(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &Brep,
        other: &Brep,
        attributes: &mut AttributeStore,
        operation: BooleanOperation,
    ) -> Result<()>;
    /// `BooleanPostProcess(FALSE)`: adapter target is `merge_p1` with intersection lists
    /// and live mates rather than all topology objects.
    fn post_process(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        attributes: &mut AttributeStore,
        state: &ManifoldState,
        check_all_edges: bool,
    ) -> Result<()>;
    /// A globally distinct attribute/notification identity for an arena entity.
    /// `other` distinguishes equal ids in independently owned arenas.
    fn object_key(&self, other: bool, entity: MergeEntity) -> ObjectKey;
    /// Whether a partial-merge subset exists and excludes this other face's
    /// surface. Uses surface identity, not face identity.
    fn excluded_surface(&self, other: &Brep, face: FaceId) -> Result<bool>;

    /// `IntersectInsertRelate()`; all subsequent orchestration remains after it.
    fn intersect_insert_relate(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
    ) -> Result<()>;
    /// `RemoveRelationship(pFace,pOtherFace)`.
    fn remove_relationship(&mut self, face: FaceId, other_face: FaceId) -> Result<()>;
    /// Reset `m_bTopologyDeleted` before attempting to close a single vertex.
    fn clear_topology_deleted(&mut self);
    /// Current `m_bTopologyDeleted`, including changes during a failed close.
    fn topology_deleted(&self) -> bool;
    /// `CloseIntersectionLoop(pVertex,bMadeNewTopology,sDelVerts,sDelEdges)`. The
    /// callgraph omits this live callee; it is co-ported into
    /// `boolean.topology_intersector_p2`
    /// (`TopologyIntersector::close_intersection_loop`). `made_new_topology` is
    /// written only on success. `deleted` receives the primary vertices and edges its
    /// intersections deleted, also when it fails. Topology it inserts notifies through
    /// `attributes`. The retry/cleanup tail stays at the call site.
    fn close_intersection_loop(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
        vertex: VertexId,
        made_new_topology: &mut bool,
        deleted: &mut (Vec<VertexId>, Vec<EdgeId>),
    ) -> Result<()>;
    /// `FixGaps(sSingleVertices,bModifiedTopology,sDelVerts,sDelEdges)`.
    fn fix_gaps(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
        single_vertices: &[VertexId],
    ) -> Result<FixedGaps>;
    /// `MakeFaceuse(pFU1,pOldRegion,pNewRegion,pOrigRegion,&NewFs)`
    /// (`boolean::merge::p2::make_faceuse` with the same mates).
    /// Its copy notifications use this same caller store, including when a face
    /// already has a mate. Build `TopologyCopy` with these attributes.
    fn make_faceuse(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
        faceuse: FaceuseId,
        original_region: Option<RegionId>,
    ) -> Result<MadeFaceuse>;
    /// `DeleteFaces(sDelFaces,&sKeepAsWires)`; the wire list is non-null even empty.
    fn delete_faces(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        attributes: &mut AttributeStore,
        faces: &[FaceId],
        keep_wires: Option<&[EdgeId]>,
    ) -> Result<()>;
}

/// Typed entities used for attribute identities; ids stay operand-local.
#[derive(Debug, Clone, Copy)]
pub enum MergeEntity {
    /// A region.
    Region(RegionId),
    /// A face.
    Face(FaceId),
    /// An edge.
    Edge(EdgeId),
    /// A vertex.
    Vertex(VertexId),
}

fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| SolidError::null_pointer(OP))
}

fn ignored<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.is_unported() => Err(error),
        Err(_) => Ok(None),
    }
}

fn pairs<A>(a: &[A], b: &[A]) -> Result<()> {
    if a.len() != b.len() {
        return Err(SolidError::invalid_input(
            OP,
            "unpaired common/coincident objects",
        ));
    }
    Ok(())
}

#[derive(Default)]
struct Faces {
    keep: Vec<FaceId>,
    other_keep: Vec<FaceId>,
    delete: Vec<FaceId>,
    other_delete: Vec<FaceId>,
    primary_sets: Vec<Vec<FaceId>>,
    other_sets: Vec<Vec<FaceId>>,
}

// `slice_same` (`Slice` with a primary face bounding material only, and only with the
// opt-in `Kernel::fixes.slice_coincident_faces_sheet_side`): that material lies on the
// side the sheet's `SM_OT_SAME` faceuse bounds. Upstream (and the default) gives
// `SM_BO_SLICE` the difference rule, which never deletes a pair against a sheet (void on
// both sides), so a face lying in the slicing sheet is kept in both parts (issue #528,
// WR-254). The fix keeps such a pair on the sheet's same side only, the rule step 7.4d
// uses for radial sectors; other primary faces keep the difference rule.
fn coincident_keep(
    operation: BooleanOperation,
    same_side: bool,
    non_solid: bool,
    slice_same: Option<bool>,
) -> Option<bool> {
    match operation {
        BooleanOperation::Union | BooleanOperation::Intersection => Some(same_side || non_solid),
        BooleanOperation::Difference => Some(!same_side || non_solid),
        BooleanOperation::Slice => Some(slice_same.unwrap_or(!same_side || non_solid)),
        BooleanOperation::ExclusiveOr => Some(false),
        BooleanOperation::Merge | BooleanOperation::PartialMerge => Some(true),
        _ => None,
    }
}

fn region_keep(
    operation: BooleanOperation,
    other: bool,
    is_void: bool,
    slice_same: Option<bool>,
) -> Option<bool> {
    match operation {
        BooleanOperation::Union => Some(is_void),
        BooleanOperation::Difference => Some(if other { !is_void } else { is_void }),
        BooleanOperation::Slice => Some(if other {
            !is_void
        } else {
            slice_same.unwrap_or(is_void)
        }),
        BooleanOperation::Intersection => Some(!is_void),
        BooleanOperation::ExclusiveOr
        | BooleanOperation::Merge
        | BooleanOperation::PartialMerge => Some(true),
        _ => None,
    }
}

fn region_pair(b: &Brep, fu: FaceuseId) -> Result<(RegionId, RegionId)> {
    Ok((
        required(eu::faceuse_region(b, fu)?)?,
        required(eu::faceuse_region(b, required(eu::faceuse_mate(b, fu)?)?)?)?,
    ))
}

fn void(b: &Brep, r: RegionId) -> Result<i32> {
    Ok(b.region(r)?.is_void())
}

fn common_count(
    intersector: &impl ManifoldIntersector,
    b: &Brep,
    v: VertexId,
) -> Result<(usize, bool)> {
    let mut count = 0;
    let mut lamina = false;
    for e in vertex::edges(b, v, None)? {
        lamina |= edge::is_lamina(b, e)?;
        if intersector.edge_mate(e).is_some() {
            count += 1;
        }
        if edge::is_closed(b, e)? {
            count += 1;
        }
    }
    Ok((count, lamina))
}

fn refresh_common(
    intersector: &impl ManifoldIntersector,
    primary: &Brep,
    other: &Brep,
    state: &mut ManifoldState,
) -> Result<()> {
    (state.edges, state.other_edges) = intersector.common_edges(primary, other)?;
    (state.vertices, state.other_vertices) = intersector.common_vertices(primary, other)?;
    pairs(&state.edges, &state.other_edges)?;
    pairs(&state.vertices, &state.other_vertices)
}

fn mark_common(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Brep,
    other: &mut Brep,
    state: &mut ManifoldState,
    intersector: &mut impl ManifoldIntersector,
    lock: &MarkLock,
    manifold: bool,
    attributes: &mut AttributeStore,
) -> Result<()> {
    let mark = lock.mark_type1();
    let other_mark = lock.mark_type2();
    for _ in 0..10 {
        kernels.mark_action(|contexts| lock.new_mark(contexts))?;
        refresh_common(intersector, primary, other, state)?;
        for (&e, &oe) in state.edges.iter().zip(&state.other_edges) {
            state
                .primary_marks
                .mark(kernels.primary, Entity::Edge(e), mark);
            state
                .other_marks
                .mark(kernels.other(), Entity::Edge(oe), other_mark);
        }
        let mut single = Vec::new();
        let mut done = true;
        for (&v, &ov) in state.vertices.iter().zip(&state.other_vertices) {
            state
                .primary_marks
                .mark(kernels.primary, Entity::Vertex(v), mark);
            state
                .other_marks
                .mark(kernels.other(), Entity::Vertex(ov), other_mark);
            let (count, lamina) = common_count(intersector, primary, v)?;
            if count != 1 || lamina {
                continue;
            }
            let mut made = false;
            intersector.clear_topology_deleted();
            if manifold {
                // This could delete entities in our stored pointers:
                let mut deleted = (Vec::new(), Vec::new());
                let status = ignored(intersector.close_intersection_loop(
                    kernels,
                    primary,
                    other,
                    attributes,
                    v,
                    &mut made,
                    &mut deleted,
                ));
                state
                    .intersection_vertices
                    .retain(|x| !deleted.0.contains(x));
                state.intersection_edges.retain(|x| !deleted.1.contains(x));
                if status?.is_none() {
                    made = false;
                }
            }
            if made || intersector.topology_deleted() {
                done = false;
                break;
            }
            single.push(v);
        }
        if done && single.len() > 1 && manifold {
            let fixed = intersector.fix_gaps(kernels, primary, other, attributes, &single)?;
            state
                .intersection_vertices
                .retain(|v| !fixed.deleted_vertices.contains(v));
            state
                .intersection_edges
                .retain(|e| !fixed.deleted_edges.contains(e));
            if fixed.modified_topology {
                done = false;
            }
        }
        if done {
            break;
        }
    }
    (state.vertices, state.other_vertices) = intersector.common_vertices(primary, other)?;
    pairs(&state.vertices, &state.other_vertices)?;
    state.intersection_loop_closed = true;
    for &v in &state.vertices {
        if common_count(intersector, primary, v)?.0 == 1 {
            state.intersection_loop_closed = false;
            break;
        }
    }
    Ok(())
}

// One direction of the common-edge face classification. Coincidence is always
// recorded primary-to-other, including when reached from an other edge use.
fn classify_edge_faces(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Brep,
    other: &mut Brep,
    attributes: &mut AttributeStore,
    state: &mut ManifoldState,
    intersector: &mut impl ManifoldIntersector,
    operation: BooleanOperation,
    marks: (MarkType, MarkType),
    edge_id: EdgeId,
    other_edge: EdgeId,
    reverse: bool,
    faces: &mut Faces,
) -> Result<bool> {
    let edgeuses = if reverse {
        edge::edgeuses(other, other_edge, None, None)?
    } else {
        edge::edgeuses(primary, edge_id, None, None)?
    };
    for &edgeuse in edgeuses.iter().step_by(2) {
        let (source, source_marks, source_k, mark) = if reverse {
            (&*other, &state.other_marks, kernels.other(), marks.1)
        } else {
            (&*primary, &state.primary_marks, &*kernels.primary, marks.0)
        };
        if eu::is_shell_edgeuse(source, edgeuse)? {
            continue;
        }
        let source_face = required(eu::face(source, edgeuse)?)?;
        if source_marks.is_marked(source_k, Entity::Face(source_face), mark) {
            continue;
        }
        let sector = if reverse {
            ignored(edge::find_radial_sector_between(
                kernels.primary,
                primary,
                edge_id,
                Some(&mut *other),
                edgeuse,
                None,
                OrientType::Same,
                None,
            ))?
        } else {
            ignored(edge::find_radial_sector_between(
                kernels.other(),
                other,
                other_edge,
                Some(&mut *primary),
                edgeuse,
                None,
                OrientType::Same,
                None,
            ))?
        };
        let Some(sector) = sector else {
            continue;
        };
        if let Some(found) = sector.coincident_faceuse {
            let source_fu = required(eu::faceuse(if reverse { other } else { primary }, edgeuse)?)?;
            let (pf, of, pf_regions, of_regions) = if reverse {
                (
                    required(eu::faceuse_face(primary, found)?)?,
                    source_face,
                    region_pair(primary, found)?,
                    region_pair(other, source_fu)?,
                )
            } else {
                (
                    source_face,
                    required(eu::faceuse_face(other, found)?)?,
                    region_pair(primary, source_fu)?,
                    region_pair(other, found)?,
                )
            };
            let first_equal = void(primary, pf_regions.0)? == void(other, of_regions.0)?;
            let second_equal = void(primary, pf_regions.1)? == void(other, of_regions.1)?;
            let same_side = first_equal && second_equal;
            let non_solid = first_equal && !second_equal;
            // the coincident faceuses point the same way (FindRadialSector): when the
            // primary face bounds material, its faceuse faces it if its region is solid
            let slice_same = if operation == BooleanOperation::Slice
                && kernels.primary.fixes.slice_coincident_faces_sheet_side
            {
                let solid = void(primary, pf_regions.0)? == 0;
                if solid == (void(primary, pf_regions.1)? == 0) {
                    None
                } else {
                    let other_fu = if reverse { source_fu } else { found };
                    Some(solid == (eu::faceuse_orientation(other, other_fu)? == OrientType::Same))
                }
            } else {
                None
            };
            if if reverse {
                state
                    .primary_marks
                    .is_marked(kernels.primary, Entity::Face(pf), marks.0)
            } else {
                state
                    .other_marks
                    .is_marked(kernels.other(), Entity::Face(of), marks.1)
            } {
                continue;
            }
            let (compatible, other_compatible, modified) =
                intersector.fix_face_coincidence(kernels, primary, other, attributes, pf, of)?;
            if modified {
                return Ok(false);
            }
            pairs(&compatible, &other_compatible)?;
            if operation == BooleanOperation::ExtractSeparate {
                faces.primary_sets.push(compatible.clone());
            }
            for (&pf, &of) in compatible.iter().zip(&other_compatible) {
                intersector.relate_faces(primary, pf, other, of)?;
                state
                    .primary_marks
                    .mark(kernels.primary, Entity::Face(pf), marks.0);
                state
                    .other_marks
                    .mark(kernels.other(), Entity::Face(of), marks.1);
                if let Some(keep) = coincident_keep(operation, same_side, non_solid, slice_same) {
                    if keep {
                        faces.keep.push(pf);
                        faces.other_keep.push(of);
                    } else {
                        faces.delete.push(pf);
                        faces.other_delete.push(of);
                    }
                }
            }
        } else if let Some(found) = sector.radial_edgeuse {
            let mut collected = Vec::new();
            let (source, source_marks, source_k, mark) = if reverse {
                (&*other, &mut state.other_marks, kernels.other(), marks.1)
            } else {
                (
                    &*primary,
                    &mut state.primary_marks,
                    &*kernels.primary,
                    marks.0,
                )
            };
            traverser::collect_faces(
                source_k,
                source,
                source_marks,
                Some(source_face),
                mark,
                &mut collected,
                None,
                None,
            )?;
            if operation == BooleanOperation::ExtractSeparate {
                if reverse {
                    faces.other_sets.push(collected.clone());
                } else {
                    faces.primary_sets.push(collected.clone());
                }
            }
            let opposite = if reverse { &*primary } else { &*other };
            let r = required(eu::shell_region(
                opposite,
                required(eu::shell(opposite, found)?)?,
            )?)?;
            let slice_same = if !reverse && operation == BooleanOperation::Slice {
                Some(
                    eu::faceuse_orientation(opposite, required(eu::faceuse(opposite, found)?)?)?
                        == OrientType::Same,
                )
            } else {
                None
            };
            if let Some(keep) = region_keep(operation, reverse, void(opposite, r)? != 0, slice_same)
            {
                match (reverse, keep) {
                    (false, true) => faces.keep.extend(collected),
                    (false, false) => faces.delete.extend(collected),
                    (true, true) => faces.other_keep.extend(collected),
                    (true, false) => faces.other_delete.extend(collected),
                }
            }
        } else {
            return Err(SolidError::sm_err(OP));
        }
    }
    Ok(true)
}

fn classify_remaining(
    kernels: &BooleanKernels<'_>,
    primary: &mut Brep,
    other: &mut Brep,
    state: &mut ManifoldState,
    operation: BooleanOperation,
    marks: (MarkType, MarkType),
    faces: &mut Faces,
) -> Result<()> {
    for reverse in [false, true] {
        let (source, opposite, source_marks, source_k, opposite_k, mark) = if reverse {
            (
                &*other,
                &mut *primary,
                &mut state.other_marks,
                kernels.other(),
                &*kernels.primary,
                marks.1,
            )
        } else {
            (
                &*primary,
                &mut *other,
                &mut state.primary_marks,
                &*kernels.primary,
                kernels.other(),
                marks.0,
            )
        };
        // The pass only reads `opposite`, so one query (and its cache) serves every face;
        // its geometry owners named by `opposite`'s naming scheme and proven owners.
        let query = BrepQuery::of_brep(opposite);
        for f in bp::faces(source, None)? {
            if source_marks.is_marked(source_k, Entity::Face(f), mark) {
                continue;
            }
            if !reverse
                && matches!(
                    operation,
                    BooleanOperation::ExclusiveOr
                        | BooleanOperation::Merge
                        | BooleanOperation::PartialMerge
                )
            {
                faces.keep.push(f);
                continue;
            }
            let mut collected = Vec::new();
            if !reverse {
                traverser::collect_faces(
                    source_k,
                    source,
                    source_marks,
                    Some(f),
                    mark,
                    &mut collected,
                    None,
                    None,
                )?;
                if operation == BooleanOperation::ExtractSeparate {
                    faces.primary_sets.push(collected);
                    continue;
                }
            }
            let edges = traverser::face_edges(source, f)?;
            let mut region = None;
            for &edge_id in &edges {
                // Upstream deliberately/repeatedly uses sFaceEdges[0] in the
                // primary pass; the other pass uses j and a different fraction.
                let selected = if reverse {
                    edge_id
                } else {
                    *edges.first().ok_or_else(|| SolidError::null_pointer(OP))?
                };
                let e = source.edge(selected)?;
                let curve = required(e.curve.as_ref())?;
                let point = curve.evaluate_point(
                    source_k,
                    e.interval
                        .evaluate(if reverse { 0.456789 } else { 0.45678 }),
                )?;
                // Upstream ignores a failed `Point3DClassify` and tries the next edge.
                let located = ignored(p1::classify_point_editing_disabled(
                    opposite_k,
                    &query,
                    opposite,
                    point,
                    crate::math::EFF_ZERO,
                    false,
                ))?;
                let Some(classification) = located else {
                    continue;
                };
                // The other pass checks PC_REGION; the primary pass only casts
                // GetObject(). These are caller decisions, not query behaviour.
                if reverse && classification.point_class() != PointClassType::Region {
                    continue;
                }
                if let Some(EntityRef::Region(r)) = classification.object() {
                    region = Some(r);
                    break;
                }
            }
            let Some(region) = region else {
                continue;
            };
            if reverse {
                traverser::collect_faces(
                    source_k,
                    source,
                    source_marks,
                    Some(f),
                    mark,
                    &mut collected,
                    None,
                    None,
                )?;
                if operation == BooleanOperation::ExtractSeparate {
                    faces.other_sets.push(collected.clone());
                }
            }
            if let Some(keep) = region_keep(operation, reverse, void(opposite, region)? != 0, None)
            {
                match (reverse, keep) {
                    (false, true) => faces.keep.extend(collected),
                    (false, false) => faces.delete.extend(collected),
                    (true, true) => faces.other_keep.extend(collected),
                    (true, false) => faces.other_delete.extend(collected),
                }
            }
        }
    }
    Ok(())
}

fn unique<T: Copy + PartialEq>(values: &mut Vec<T>) {
    // SmTArray::RemoveDuplicates retains the LAST occurrence of each entry.
    // Reverse/retain/reverse preserves precisely that order without pointer sort.
    let mut seen = Vec::new();
    values.reverse();
    values.retain(|v| {
        if seen.contains(v) {
            false
        } else {
            seen.push(*v);
            true
        }
    });
    values.reverse();
}

fn repair_lists(
    primary: &Brep,
    other: &Brep,
    operation: BooleanOperation,
    faces: &mut Faces,
) -> Result<()> {
    for list in [
        &mut faces.keep,
        &mut faces.delete,
        &mut faces.other_keep,
        &mut faces.other_delete,
    ] {
        unique(list);
    }
    let actual = bp::faces(primary, None)?;
    if actual.len() as i128 - faces.keep.len() as i128 - faces.delete.len() as i128 != 0 {
        faces.keep.retain(|f| actual.contains(f));
        faces.delete.retain(|f| actual.contains(f));
    }
    let actual = bp::faces(other, None)?;
    if operation != BooleanOperation::PartialMerge
        && actual.len() as i128 - faces.other_keep.len() as i128 - faces.other_delete.len() as i128
            != 0
    {
        faces.other_keep.retain(|f| actual.contains(f));
        faces.other_delete.retain(|f| actual.contains(f));
    }
    // Confused/duplicate lists only emit SE_MSG upstream; they do not fail.
    Ok(())
}

fn label(attributes: &mut AttributeStore, keys: &[ObjectKey], attribute_id: u64) -> Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let attribute = attributes.insert(Attribute::long(attribute_id, 1, Behavior::Reference))?;
    for &key in keys {
        if attributes.find_attribute(key, attribute_id).is_none() {
            attributes.add_attribute(key, attribute)?;
        }
    }
    Ok(())
}

fn label_classification(
    state: &ManifoldState,
    intersector: &impl ManifoldIntersector,
    faces: &Faces,
    attributes: &mut AttributeStore,
) -> Result<()> {
    let mut intersection: Vec<_> = state
        .intersection_edges
        .iter()
        .map(|&e| intersector.object_key(false, MergeEntity::Edge(e)))
        .collect();
    intersection.extend(
        state
            .intersection_vertices
            .iter()
            .map(|&v| intersector.object_key(false, MergeEntity::Vertex(v))),
    );
    label(attributes, &intersection, ids::BOOLEAN_INTERSECT)?;
    for (other, list, id) in [
        (false, &faces.delete, ids::BOOLEAN_DELETE),
        (false, &faces.keep, ids::BOOLEAN_SAVE),
        (true, &faces.other_delete, ids::BOOLEAN_DELETE),
        (true, &faces.other_keep, ids::BOOLEAN_SAVE),
    ] {
        let keys: Vec<_> = list
            .iter()
            .map(|&f| intersector.object_key(other, MergeEntity::Face(f)))
            .collect();
        label(attributes, &keys, id)?;
    }
    Ok(())
}

fn merge_data(attributes: &AttributeStore, key: ObjectKey) -> Option<&MergeRegionData> {
    attributes
        .find_attribute(key, ids::OTHER_REGION_ID)
        .and_then(|h| attributes.get(h))
        .and_then(Attribute::merge_region_data)
}

fn source_type(b: &Brep, r: RegionId, infinite: bool) -> Result<i64> {
    Ok(if infinite {
        2
    } else {
        i64::from(void(b, r)? != 0)
    })
}

fn remember_split(
    primary: &Brep,
    other: &Brep,
    intersector: &impl ManifoldIntersector,
    attributes: &mut AttributeStore,
    original: RegionId,
    new_face: FaceId,
    other_face: FaceId,
) -> Result<()> {
    let (r1, r2) = region_pair(primary, required(face::upward_faceuse(primary, new_face)?)?)?;
    let (o1, o2) = region_pair(other, required(face::upward_faceuse(other, other_face)?)?)?;
    let key = |r| intersector.object_key(false, MergeEntity::Region(r));
    // Capture both handles before either AddAttribute, even when r1 == r2.
    let handle1 = attributes.find_attribute(key(r1), ids::OTHER_REGION_ID);
    let handle2 = attributes.find_attribute(key(r2), ids::OTHER_REGION_ID);
    let old_handle = if r1 == original { handle1 } else { handle2 };
    let old_infinite = old_handle
        .and_then(|h| attributes.get(h))
        .and_then(Attribute::merge_region_data)
        .is_some_and(MergeRegionData::has_this_infinite_region)
        || Some(original) == primary.infinite_region();
    let this_infinite = merge_data(attributes, key(r1))
        .is_some_and(MergeRegionData::has_this_infinite_region)
        || merge_data(attributes, key(r2)).is_some_and(MergeRegionData::has_this_infinite_region)
        || Some(r1) == primary.infinite_region()
        || Some(r2) == primary.infinite_region();
    let old_type = source_type(primary, original, old_infinite)?;
    for (r, o, handle) in [(r1, o1, handle1), (r2, o2, handle2)] {
        let other_key = intersector.object_key(true, MergeEntity::Region(o));
        let this_type = source_type(primary, r, this_infinite)?;
        let other_type = source_type(other, o, Some(o) == other.infinite_region())?;
        if let Some(handle) = handle {
            let data = required(
                attributes
                    .get_mut(handle)
                    .and_then(Attribute::merge_region_data_mut),
            )?;
            let first = required(data.this_region(0))?;
            ignored(data.set_regions(0, first, this_type, other_key, other_type))?;
        } else {
            let h = attributes.insert(Attribute::merge_region(
                key(original),
                old_type,
                other_key,
                other_type,
            ))?;
            attributes.add_attribute(key(r), h)?;
        }
    }
    Ok(())
}

fn make_other_faces(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Brep,
    other: &mut Brep,
    intersector: &mut impl ManifoldIntersector,
    attributes: &mut AttributeStore,
    operation: BooleanOperation,
    faces: &Faces,
) -> Result<()> {
    for &f in &faces.other_keep {
        let (fu1, fu2) = face::faceuses(other, f)?;
        let fu1 = required(fu1)?;
        let fu2 = required(fu2)?;
        let original =
            intersector.find_region_of_other_face(kernels.primary, primary, other, fu1)?;
        let original_void = if let Some(r) = original {
            void(primary, r)?
        } else {
            0
        };
        let made = if matches!(
            operation,
            BooleanOperation::Union
                | BooleanOperation::Intersection
                | BooleanOperation::Difference
                | BooleanOperation::PartialMerge
                | BooleanOperation::Merge
                | BooleanOperation::ExclusiveOr
                | BooleanOperation::Slice
        ) {
            intersector.make_faceuse(kernels, primary, other, attributes, fu1, original)?
        } else {
            MadeFaceuse::default()
        };
        if let (Some(new_region), Some(_)) = (made.new_region, original) {
            let old = required(made.old_region)?;
            if void(primary, new_region)? != original_void || void(primary, old)? != original_void {
                let changed = if void(primary, old)? == original_void {
                    new_region
                } else {
                    old
                };
                let other_void1 = void(other, required(eu::faceuse_region(other, fu1)?)?)?;
                let other_void2 = void(other, required(eu::faceuse_region(other, fu2)?)?)?;
                if other_void1 != other_void2 || (other_void1 == 1 && other_void2 == 1) {
                    primary.region_mut(changed)?.set_is_void(original_void);
                }
            }
        }
        if made.new_region.is_some() {
            if let Some(&new_face) = made.faces.first() {
                remember_split(
                    primary,
                    other,
                    intersector,
                    attributes,
                    required(made.old_region)?,
                    new_face,
                    f,
                )?;
            }
        }
    }
    Ok(())
}

fn update_infinite(
    primary: &mut Brep,
    intersector: &impl ManifoldIntersector,
    attributes: &AttributeStore,
) -> Result<bool> {
    let infinite = required(primary.infinite_region())?;
    if merge_data(
        attributes,
        intersector.object_key(false, MergeEntity::Region(infinite)),
    )
    .is_none()
    {
        return Ok(true);
    }
    let mut count = 0;
    for r in bp::regions(primary, None)? {
        if merge_data(
            attributes,
            intersector.object_key(false, MergeEntity::Region(r)),
        )
        .is_some_and(|data| data.has_this_infinite_region() && data.has_other_infinite_region())
        {
            primary.set_infinite_region(Some(r));
            count += 1;
        }
    }
    Ok(count <= 1)
}

fn result_solid(operation: BooleanOperation, data: &MergeRegionData, was_solid: bool) -> bool {
    match operation {
        BooleanOperation::Union | BooleanOperation::Merge | BooleanOperation::PartialMerge => {
            data.has_any_solid(3)
        }
        BooleanOperation::Difference | BooleanOperation::Slice => {
            data.has_any_solid(1) && !data.has_any_solid(2)
        }
        BooleanOperation::Intersection => data.has_any_solid(1) && data.has_any_solid(2),
        BooleanOperation::ExclusiveOr => data.has_any_solid(1) != data.has_any_solid(2),
        _ => was_solid,
    }
}

/// Every region's enclosing shell first, the region's outer shell that
/// `SmBrep::AssertValid` test 12 takes it to be (the fork's fix, source fork
/// `abastran/void-shell-check`), only with the opt-in
/// `Kernel::fixes.boolean_outer_shell_first` (WR-265); off (the default), upstream's
/// order is left. Step 6 merges regions and appends the merged region's shells after
/// the survivor's own, so when a union seals a void the void's shell can come first;
/// step 8 only swaps void flags. [`bp::set_region_is_void_flags_for_nested_solids`]
/// walks the nesting from the infinite region and makes each nested region's outer
/// shell its list head. It also sets every region's void flag from that nesting, which
/// differs from step 7's flags where those disagree with the nesting.
fn put_outer_shells_first(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Brep,
    operation: BooleanOperation,
) -> Result<()> {
    if !kernels.primary.fixes.boolean_outer_shell_first
        || !matches!(
            operation,
            BooleanOperation::Union | BooleanOperation::Difference | BooleanOperation::Intersection
        )
    {
        return Ok(());
    }
    let regions = bp::regions(primary, None)?;
    if regions.len() > 2
        && regions.first() == primary.infinite_region().as_ref()
        && crate::topology::brep::p2::is_manifold_solid(primary)?
    {
        bp::set_region_is_void_flags_for_nested_solids(kernels.primary, primary)?;
    }
    Ok(())
}

fn find_and_set_infinite(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Brep,
    shell: Option<crate::topology::ShellId>,
    void_setting: bp::SetRegionVoidType,
    bounding_box: Option<&crate::geom::extent3d::Extent3d>,
) -> Result<()> {
    // (the query's geometry owners named by `primary`'s naming scheme and proven owners)
    let query = BrepQuery::of_brep(primary);
    crate::topology::brep::p5::find_and_set_infinite_region(
        kernels.primary,
        primary,
        &query,
        shell,
        void_setting,
        bounding_box,
    )
}

/// `m_pOther->IsSheet()` (`topology.brep_p2`), in the other B-rep's context as
/// [`orient_sheet_surfaces`].
fn is_sheet(kernels: &mut BooleanKernels<'_>, other: &Brep) -> Result<bool> {
    let k = kernels.other.as_deref_mut().unwrap_or(kernels.primary);
    crate::topology::brep::p2::is_sheet(k, other)
}

/// `m_pOther->IsOrientedSheet()` (`topology.brep_p2`), in the other B-rep's context.
fn is_oriented_sheet(kernels: &mut BooleanKernels<'_>, other: &Brep) -> Result<bool> {
    let k = kernels.other.as_deref_mut().unwrap_or(kernels.primary);
    crate::topology::brep::p2::is_oriented_sheet(k, other)
}

fn orient_sheet_surfaces(
    kernels: &mut BooleanKernels<'_>,
    other: &mut Brep,
    face: FaceId,
    same: bool,
) -> Result<()> {
    let k = kernels.other.as_deref_mut().unwrap_or(kernels.primary);
    crate::topology::brep::p9::orient_sheet_surfaces(k, other, Some(face), same)
}

/// `new SmBrep(dSize, dRatio, pOptContextForStack)` for a component: the B-rep and its
/// infinite void region ([`Brep::new_with_infinite_region`], the topology part of
/// `topology.brep_p3`'s constructor). The kernel is held shared here, so the rest of
/// `SmBrep()` (the context's zone tolerance and the given sizes, and the
/// `SM_NO_CONSTRUCTION` notification) is not applied, the same deviation as
/// `blend::fillet_geom::p1::FilletBrep::new`'s and `primitive::api_primitives`'.
fn new_component_brep(
    _kernel: &Kernel,
    _model_size_estimate: f64,
    _large_small_size_ratio: f64,
    _stack_context: Option<&crate::geom::context::Context>,
) -> Result<Brep> {
    Brep::new_with_infinite_region()
}

/// `m_vTI.m_pBrep->DeleteStaleCaches()` (`topology.brep_p2`): the faces' trimmed-surface
/// caches are held by their Rust callers, not by the B-rep, and the merge holds none, so
/// none is stale.
fn delete_stale_caches(primary: &mut Brep) -> Result<()> {
    crate::topology::brep::p2::delete_stale_caches::<()>(
        primary,
        &mut std::collections::BTreeMap::new(),
    )
}

// true means imprint-only or imprint/classify: preserve normal topology and exit.
fn editing_stages(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Brep,
    other: &mut Brep,
    intersector: &mut impl ManifoldIntersector,
    state: &mut ManifoldState,
    attributes: &mut AttributeStore,
    options: MergeOptions,
    manifold: bool,
    operation: BooleanOperation,
    lock: &MarkLock,
    faces: &mut Faces,
) -> Result<bool> {
    let marks = (lock.mark_type1(), lock.mark_type2());
    for _ in 0..10 {
        for &f in faces.keep.iter().chain(&faces.delete) {
            if let Some(of) = intersector.face_mate(f) {
                ignored(intersector.remove_relationship(f, of))?;
            }
        }
        faces.keep.clear();
        faces.other_keep.clear();
        faces.delete.clear();
        faces.other_delete.clear();
        mark_common(
            kernels,
            primary,
            other,
            state,
            intersector,
            lock,
            manifold,
            attributes,
        )?;
        if operation == BooleanOperation::Imprint || options.imprinting {
            return Ok(true);
        }
        if operation == BooleanOperation::PartialMerge {
            for f in bp::faces(other, None)? {
                if intersector.excluded_surface(other, f)? {
                    state
                        .other_marks
                        .mark(kernels.other(), Entity::Face(f), marks.1);
                }
            }
        }
        refresh_common(intersector, primary, other, state)?;
        let mut done = true;
        // A snapshot is required: coincidence repair can invalidate the current
        // common-edge lists and forces a new outer iteration before continuing.
        let edge_pairs: Vec<_> = state
            .edges
            .iter()
            .copied()
            .zip(state.other_edges.iter().copied())
            .collect();
        for (e, oe) in edge_pairs {
            if !classify_edge_faces(
                kernels,
                primary,
                other,
                attributes,
                state,
                intersector,
                operation,
                marks,
                e,
                oe,
                false,
                faces,
            )? || !classify_edge_faces(
                kernels,
                primary,
                other,
                attributes,
                state,
                intersector,
                operation,
                marks,
                e,
                oe,
                true,
                faces,
            )? {
                done = false;
                break;
            }
        }
        if done {
            break;
        }
    }
    classify_remaining(kernels, primary, other, state, operation, marks, faces)?;
    if operation != BooleanOperation::ExtractSeparate {
        repair_lists(primary, other, operation, faces)?;
    }
    if operation == BooleanOperation::ImprintClassify || options.imprint_and_classify_faces {
        label_classification(state, intersector, faces, attributes)?;
        return Ok(true);
    }
    let mut keep_wires = Vec::new();
    if !options.cookie_cutter && operation != BooleanOperation::PartialMerge {
        intersector.process_wires(
            kernels,
            primary,
            other,
            state,
            attributes,
            operation,
            marks.0,
            marks.1,
            &mut keep_wires,
        )?;
    }
    if !options.cookie_cutter && operation != BooleanOperation::ExtractSeparate {
        make_other_faces(
            kernels,
            primary,
            other,
            intersector,
            attributes,
            operation,
            faces,
        )?;
    }
    let infinite_current = update_infinite(primary, intersector, attributes)?;
    if operation == BooleanOperation::Intersection {
        let (edges, _) = intersector.common_edges(primary, other)?;
        for e in edges {
            if p1::edge_borders_solid(primary, e)?.is_none() && !keep_wires.contains(&e) {
                keep_wires.push(e);
            }
        }
    }
    intersector.delete_faces(
        kernels,
        primary,
        attributes,
        &faces.delete,
        Some(&keep_wires),
    )?;
    for r in bp::regions(primary, None)? {
        if let Some(data) = merge_data(
            attributes,
            intersector.object_key(false, MergeEntity::Region(r)),
        ) {
            let solid = result_solid(operation, data, void(primary, r)? == 0);
            primary.region_mut(r)?.set_is_void(i32::from(!solid));
        }
    }
    if !infinite_current || void(primary, required(primary.infinite_region())?)? == 0 {
        ignored(find_and_set_infinite(
            kernels,
            primary,
            None,
            bp::SetRegionVoidType::InfiniteRegionSwap,
            None,
        ))?;
    }
    put_outer_shells_first(kernels, primary, operation)?;
    ignored(intersector.propagate_regions(kernels, primary, other, attributes, operation))?;
    if options.boolean_post_process
        && matches!(
            operation,
            BooleanOperation::Union
                | BooleanOperation::Intersection
                | BooleanOperation::Difference
                | BooleanOperation::ExclusiveOr
                | BooleanOperation::Slice
        )
    {
        intersector.post_process(kernels, primary, attributes, state, false)?;
    }
    Ok(false)
}

/// Perform the manifold Boolean using the canonical intersector adapter. The
/// optional caller lock is bound to both operand contexts and remains locked on
/// return; an internally created lock is always released. `manifold` is the
/// existing `m_bManifoldBoolean` flag (this upstream function does not set it).
/// Returns whether `rpResult` was assigned the primary B-rep; `ExtractSeparate`
/// leaves it unassigned and writes the component arrays instead. A supplied lock
/// uses the same context-index convention as these kernels (primary 0, other 1
/// or 0 when shared); it may be initially unbound.
///
/// Ordinary early failures preserve upstream's doing-Boolean flags; temporary
/// editing changes and internally owned mark locks are restored on every return.
/// Imprint exits deliberately leave the other operand editing-enabled, as
/// `SmTemporaryChangeValue::Clear` does in the pinned source.
/// Operand slots are authoritative on **all** exits: a late deferred cache call
/// or inverse-transform error can follow deletion of `other`. `Err` does not
/// promise retained ownership or rollback of topology, records or attributes.
pub fn manifold_boolean(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Option<Brep>,
    other: &mut Option<Brep>,
    intersector: &mut impl ManifoldIntersector,
    state: &mut ManifoldState,
    attributes: &mut AttributeStore,
    options: MergeOptions,
    manifold: bool,
    operation: BooleanOperation,
    caller_lock: Option<&mut MarkLock>,
) -> Result<bool> {
    manifold_boolean_transformed(
        kernels,
        primary,
        other,
        intersector,
        state,
        attributes,
        options,
        manifold,
        operation,
        caller_lock,
    )
}

/// The body of [`manifold_boolean`].
fn manifold_boolean_transformed(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Option<Brep>,
    other: &mut Option<Brep>,
    intersector: &mut impl ManifoldIntersector,
    state: &mut ManifoldState,
    attributes: &mut AttributeStore,
    options: MergeOptions,
    manifold: bool,
    operation: BooleanOperation,
    caller_lock: Option<&mut MarkLock>,
) -> Result<bool> {
    let a = required(primary.as_mut())?;
    let b = required(other.as_mut())?;
    let transform = p1::temporary_brep_transform(kernels.primary, kernels.other(), a, b)?.apply(
        a,
        b,
        |brep, is_other, xfm, scale, scale_tol| {
            intersector.transform_brep(kernels, brep, is_other, attributes, xfm, scale, scale_tol)
        },
    )?;
    // This outer scope matches sTempXfm's lifetime: every Result exit from the
    // complete operation (including imprint, extraction and deletion) first
    // leaves its mark/edit scopes, then restores only surviving operands.
    let mut environment = (kernels, intersector, attributes);
    transform.run(
        primary,
        other,
        &mut environment,
        |(kernels, intersector, attributes), primary, other| {
            manifold_boolean_translated(
                kernels,
                primary,
                other,
                *intersector,
                state,
                attributes,
                options,
                manifold,
                operation,
                caller_lock,
            )
        },
        |(kernels, intersector, attributes), brep, is_other, xfm, scale, scale_tol| {
            intersector.transform_brep(kernels, brep, is_other, attributes, xfm, scale, scale_tol)
        },
    )
}

fn manifold_boolean_translated(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Option<Brep>,
    other: &mut Option<Brep>,
    intersector: &mut impl ManifoldIntersector,
    state: &mut ManifoldState,
    attributes: &mut AttributeStore,
    options: MergeOptions,
    manifold: bool,
    operation: BooleanOperation,
    caller_lock: Option<&mut MarkLock>,
) -> Result<bool> {
    let a = required(primary.as_mut())?;
    let b = required(other.as_mut())?;
    if operation == BooleanOperation::Slice {
        if !is_sheet(kernels, b)? {
            return Err(SolidError::sm_err(OP));
        }
        if !is_oriented_sheet(kernels, b)? {
            let first = required(bp::faces(b, None)?.first().copied())?;
            ignored(orient_sheet_surfaces(kernels, b, first, true))?;
        }
    }
    kernels.doing_boolean(true);
    let edit_a = a.editing_enabled();
    let edit_b = b.editing_enabled();
    a.set_editing_enabled(true);
    b.set_editing_enabled(true);
    let mut local_lock = MarkLock::unbound();
    let caller_owned = caller_lock.is_some();
    let lock = caller_lock.unwrap_or(&mut local_lock);
    let second = usize::from(kernels.other.is_some());
    // Intersections precede binding the classification mark lock upstream.
    let intersect = intersector
        .intersect_insert_relate(kernels, a, b, attributes)
        .and_then(|()| {
            state.intersection_edges.extend(intersector.mapped_edges()?);
            state
                .intersection_vertices
                .extend(intersector.mapped_vertices()?);
            Ok(())
        });
    if let Err(error) = intersect {
        a.set_editing_enabled(edit_a);
        b.set_editing_enabled(edit_b);
        return Err(error);
    }
    let bind = kernels.mark_action(|contexts| lock.set_context_pair(contexts, 0, second));
    if let Err(error) = bind {
        a.set_editing_enabled(edit_a);
        b.set_editing_enabled(edit_b);
        return Err(error);
    }
    let mut faces = Faces::default();
    let result = editing_stages(
        kernels,
        a,
        b,
        intersector,
        state,
        attributes,
        options,
        manifold,
        operation,
        lock,
        &mut faces,
    );
    if matches!(result, Ok(true)) {
        // Clear() disables restoring the other's editing flag. Deletion and
        // context resets happen while the primary is still editing-enabled,
        // before the native stack guards leave scope.
        if (operation == BooleanOperation::Imprint || options.imprinting)
            && !options.keep_other_brep
        {
            *other = None;
        }
        kernels.doing_boolean(false);
        let released = if caller_owned {
            Ok(())
        } else {
            kernels.mark_action(|contexts| lock.release(contexts))
        };
        required(primary.as_mut())?.set_editing_enabled(edit_a);
        released?;
        return Ok(true);
    }
    // The local mark lock belongs to the editing scope. It is destroyed
    // before the earlier-declared editing guards; extraction follows both.
    let released = if caller_owned {
        Ok(())
    } else {
        kernels.mark_action(|contexts| lock.release(contexts))
    };
    a.set_editing_enabled(edit_a);
    b.set_editing_enabled(edit_b);
    released?;
    result?;
    if operation == BooleanOperation::ExtractSeparate {
        for (source, reverse, sets, components) in [
            (
                required(primary.as_mut())?,
                false,
                &faces.primary_sets,
                &mut state.primary_components,
            ),
            (
                required(other.as_mut())?,
                true,
                &faces.other_sets,
                &mut state.other_components,
            ),
        ] {
            for set in sets {
                let mut component = new_component_brep(
                    if reverse {
                        kernels.other()
                    } else {
                        kernels.primary
                    },
                    crate::geom::tol::USE_DEFAULT,
                    crate::geom::tol::USE_DEFAULT,
                    None,
                )?;
                // Native defaults: new_faces=NULL, use_merge=TRUE, merge_object=NULL.
                ignored(intersector.copy_component_faces(
                    kernels,
                    source,
                    reverse,
                    set,
                    &mut component,
                    attributes,
                ))?;
                components.push(component);
            }
        }
        if !options.keep_other_brep {
            *primary = None;
        }
    }
    // Not upstream (source issue #746; opt-in,
    // `Kernel::fixes.remove_slit_loops_where_solids_touch`): a volumetric Boolean's result
    // loses the zero-area inner loops a contact curve inside a face left, and the shells
    // joined only through them are split (the fork's `RemoveSlitLoopsWhereSolidsTouch`
    // before `rpResult = m_vTI.m_pBrep`). The destroyed edge uses notify on call-local
    // records under their type code and arena index.
    if kernels.primary.fixes.remove_slit_loops_where_solids_touch
        && matches!(
            operation,
            BooleanOperation::Union | BooleanOperation::Intersection | BooleanOperation::Difference
        )
    {
        if let Some(a) = primary.as_mut() {
            let editing = a.editing_enabled();
            a.set_editing_enabled(true);
            let mut records = crate::topology::topology::NotifyRecords::new();
            let mut env = edge::NotifyEnv {
                records: &mut records,
                attributes: &mut *attributes,
                context: None,
                key_of: &crate::topology::loopuse::notify_ref,
            };
            let removed = crate::topology::brep::p3::remove_slit_loops_where_solids_touch(
                &mut *kernels.primary,
                a,
                &mut env,
            );
            a.set_editing_enabled(editing);
            removed?;
        }
    }
    finish_boolean(
        kernels,
        primary,
        other,
        options,
        operation,
        delete_stale_caches,
    )
}

// SmMerge.cpp:5059–5097 deletes other BEFORE the ignored cache status. An
// Unported cache call (or later inverse-transform failure) can therefore follow
// consumption. Keep that boundary explicit: callers must inspect the operand
// slots on every exit, not infer ownership from Result::is_ok().
fn finish_boolean(
    kernels: &mut BooleanKernels<'_>,
    primary: &mut Option<Brep>,
    other: &mut Option<Brep>,
    options: MergeOptions,
    operation: BooleanOperation,
    delete_caches: impl FnOnce(&mut Brep) -> Result<()>,
) -> Result<bool> {
    if operation != BooleanOperation::PartialMerge && !options.keep_other_brep {
        *other = None;
    }
    if let Some(a) = primary.as_mut() {
        ignored(delete_caches(a))?;
    }
    kernels.doing_boolean(false);
    Ok(operation != BooleanOperation::ExtractSeparate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_mark_contexts_survive_a_failing_action() {
        let mut first = Kernel::default();
        let mut second = Kernel::default();
        first.context.set_doing_boolean(true);
        let mut kernels = BooleanKernels {
            primary: &mut first,
            other: Some(&mut second),
        };
        let mut lock = kernels
            .mark_action(|contexts| MarkLock::new_pair(contexts, 0, 1))
            .unwrap();
        let marks = (lock.mark_type1(), lock.mark_type2());
        let before = (
            kernels.primary.context.current_mark(marks.0),
            kernels.other().context.current_mark(marks.1),
        );
        let result: Result<()> = kernels.mark_action(|contexts| {
            lock.new_mark(contexts)?;
            Err(SolidError::sm_err("test"))
        });
        assert!(result.is_err());
        assert!(kernels.primary.context.doing_boolean());
        assert!(!kernels.other().context.doing_boolean());
        assert_ne!(kernels.primary.context.current_mark(marks.0), before.0);
        assert_ne!(kernels.other().context.current_mark(marks.1), before.1);
        kernels
            .mark_action(|contexts| lock.release(contexts))
            .unwrap();
        assert!(!kernels.primary.context.is_mark_locked(marks.0));
        assert!(!kernels.other().context.is_mark_locked(marks.1));
    }

    #[test]
    fn completion_reports_actual_consumption_before_late_cache_failure() {
        for operation in [BooleanOperation::Difference, BooleanOperation::PartialMerge] {
            for keep in [false, true] {
                for late in [0, 1, 2] {
                    let mut kernel = Kernel::default();
                    kernel.context.set_doing_boolean(true);
                    let mut kernels = BooleanKernels {
                        primary: &mut kernel,
                        other: None,
                    };
                    let mut primary = Some(Brep::new());
                    let mut other = Some(Brep::new());
                    let options = MergeOptions {
                        keep_other_brep: keep,
                        ..MergeOptions::default()
                    };
                    let mut called = false;
                    let result = finish_boolean(
                        &mut kernels,
                        &mut primary,
                        &mut other,
                        options,
                        operation,
                        |_| {
                            called = true;
                            match late {
                                0 => Ok(()),
                                1 => Err(SolidError::sm_err("test::cache")),
                                _ => Err(SolidError::unported(
                                    "SmBrep::DeleteStaleCaches",
                                    "topology.brep_p2",
                                )),
                            }
                        },
                    );
                    assert!(called);
                    assert!(primary.is_some());
                    assert_eq!(
                        other.is_none(),
                        !keep && operation != BooleanOperation::PartialMerge
                    );
                    assert_eq!(result.is_err(), late == 2);
                    assert_eq!(kernels.primary.context.doing_boolean(), late == 2);
                }
            }
        }
    }

    #[test]
    fn slice_keeps_a_coincident_pair_on_the_sheet_same_side_only() {
        // issue #528: against a sheet (void on both sides) a coincident pair is either
        // non-solid or on opposite sides, which the difference rule always keeps
        for (same_side, non_solid) in [(false, false), (false, true), (true, false)] {
            let difference = Some(!same_side || non_solid);
            assert_eq!(
                coincident_keep(BooleanOperation::Difference, same_side, non_solid, None),
                difference
            );
            // a primary face not bounding material keeps the difference rule
            assert_eq!(
                coincident_keep(BooleanOperation::Slice, same_side, non_solid, None),
                difference
            );
            for slice_same in [Some(false), Some(true)] {
                assert_eq!(
                    coincident_keep(BooleanOperation::Slice, same_side, non_solid, slice_same),
                    slice_same
                );
                for operation in [
                    BooleanOperation::Union,
                    BooleanOperation::Intersection,
                    BooleanOperation::Difference,
                    BooleanOperation::ExclusiveOr,
                    BooleanOperation::Merge,
                    BooleanOperation::PartialMerge,
                    BooleanOperation::ExtractSeparate,
                ] {
                    assert_eq!(
                        coincident_keep(operation, same_side, non_solid, slice_same),
                        coincident_keep(operation, same_side, non_solid, None)
                    );
                }
            }
        }
    }

    #[test]
    fn classification_attributes_share_reference_identity_and_preserve_existing_tags() {
        let mut attributes = AttributeStore::new();
        let a = ObjectKey(11);
        let b = ObjectKey(12);
        let original = attributes
            .insert(Attribute::long(ids::BOOLEAN_SAVE, 7, Behavior::Reference))
            .unwrap();
        attributes.add_attribute(a, original).unwrap();
        label(&mut attributes, &[a, b], ids::BOOLEAN_SAVE).unwrap();
        assert_eq!(
            attributes.find_attribute(a, ids::BOOLEAN_SAVE),
            Some(original)
        );
        let added = attributes.find_attribute(b, ids::BOOLEAN_SAVE).unwrap();
        label(
            &mut attributes,
            &[ObjectKey(13), ObjectKey(14)],
            ids::BOOLEAN_DELETE,
        )
        .unwrap();
        assert_ne!(added, original);
        assert_eq!(
            attributes.find_attribute(ObjectKey(13), ids::BOOLEAN_DELETE),
            attributes.find_attribute(ObjectKey(14), ids::BOOLEAN_DELETE)
        );
    }

    /// The cup and lid of WR-265 (#388): a cube 100 hollowed to an open cup (walls and
    /// floor 10) joined with a lid over its top 10 closes the cavity 80 x 80 x 80. For
    /// either operand order, whether each shell of the one solid region is mated to the
    /// infinite region (the outer shell).
    fn cup_and_lid_outer_shells(fix: bool) -> Vec<Vec<bool>> {
        use crate::boolean::api_brep::boolean;
        use crate::math::Point3;
        use crate::primitive::api_primitives::create_box;
        let mut out = Vec::new();
        for lid_first in [false, true] {
            let mut k = Kernel::default();
            k.fixes.boolean_outer_shell_first = fix;
            let cube = create_box(&mut k, Point3::new(0.0, 0.0, 0.0), 100.0, 100.0, 100.0).unwrap();
            let hollow =
                create_box(&mut k, Point3::new(10.0, 10.0, 10.0), 80.0, 80.0, 100.0).unwrap();
            let lid = create_box(&mut k, Point3::new(0.0, 0.0, 90.0), 100.0, 100.0, 10.0).unwrap();
            let cup = boolean(&mut k, cube, hollow, BooleanOperation::Difference)
                .unwrap()
                .result;
            let (a, b) = if lid_first { (lid, cup) } else { (cup, lid) };
            let r = boolean(&mut k, a, b, BooleanOperation::Union)
                .unwrap()
                .result;
            let infinite = r.infinite_region().unwrap();
            let solids: Vec<RegionId> = bp::regions(&r, None)
                .unwrap()
                .into_iter()
                .filter(|&g| r.region(g).unwrap().is_void() == 0)
                .collect();
            assert_eq!(solids.len(), 1, "lid first: {lid_first}");
            let shells = r.region(solids[0]).unwrap().shells.clone();
            out.push(
                shells
                    .iter()
                    .map(|&shell| {
                        let mated = r.shell(shell).unwrap().mated_shells(&mut k, &r).unwrap();
                        mated
                            .iter()
                            .all(|&m| r.shell(m).unwrap().region == Some(infinite))
                    })
                    .collect(),
            );
        }
        out
    }

    /// WR-265 with `Kernel::fixes.boolean_outer_shell_first`: the outer shell comes
    /// first, as `SmBrep::AssertValid` test 12 wants.
    #[test]
    fn issue_388_union_sealing_a_void_puts_the_outer_shell_first() {
        assert_eq!(
            cup_and_lid_outer_shells(true),
            [vec![true, false], vec![true, false]]
        );
    }

    /// WR-265 by default (SMLib): step 6 leaves the cavity's shell first, as the C++
    /// build does (`boolean.api_brep` cases `cup_lid_*`).
    #[test]
    fn union_sealing_a_void_leaves_the_cavity_shell_first_by_default() {
        assert_eq!(
            cup_and_lid_outer_shells(false),
            [vec![false, true], vec![false, true]]
        );
    }
}
