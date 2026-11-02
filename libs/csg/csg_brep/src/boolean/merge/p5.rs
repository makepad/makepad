//! Wire edges and isolated shell vertices in a boolean merge.
//!
//! Upstream: `source/SMLib/src/SmMerge.cpp`, `source/SMLib/inc/SmMerge.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`process_wires_and_shell_vertices`] | `SmMerge::ProcessWiresAndShellVertices` |
//! | [`process_wires_and_shell_vertices_with_kernels`] | the same stage with each operand context |
//! | [`Relations`] | shared bidirectional mate lookups from `m_vTI` |
//!
//! Deviations: operands, marks and mate maps are borrowed explicitly instead of
//! reading `SmMerge::m_vTI`; ids replace pointers. Output stays mutable because
//! upstream preserves its partial contents on failure. Classification goes through
//! `query.brep` (`p1::classify_point_editing_disabled`), each B-rep with its own kernel;
//! graph deletion is `topology.brep_p4`'s `DeleteEdge` / `DeleteVertex`, notified through
//! `copy`'s records, attributes and primary identities and the primary kernel's context
//! (hence `&mut Kernel`). Edge copying is [`p2::merge_edge`] of each edge's primary edge
//! use; vertex copying delegates to [`p1::merge_vertex`]. B-rep
//! caches are not attached to arena B-reps yet, so the optional cache resizing is
//! absent. The deletion retry loop is iterative; wire collection uses the bounded
//! explicit stack of `topology_traverser`. No input-dependent recursion is added.
//! Relations are shared mutably with `merge_p1` and `merge_p2`; notifications use
//! the caller-owned records, attribute store and identities of [`TopologyCopy`].
//! Newly created vertex relationships go through `Relate` ([`Relations::relate_vertices`]), which may raise the other operand's
//! vertex tolerance, so `other` is mutable.
//! Missing/dangling mates are rejected only when added to the keep list:
//! typed ids cannot represent the null pointer upstream can append there.

use crate::boolean::merge::p1::{self, VertexCopy};
use crate::boolean::merge::p2::{self, MergeContext, TopologyCopy};
use crate::boolean::topology_intersector::p1::Relations;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::a_object::BooleanOperation;
use crate::geom::context::{Context, MarkType};
use crate::intersect::point_class::PointClassType;
use crate::kernel::Kernel;
use crate::math::{Point3, EFF_ZERO};
use crate::query::brep::BrepQuery;
use crate::topology::brep::p4::{self, DeleteEnv};
use crate::topology::edgeuse::p1 as eu;
use crate::topology::topology_traverser::{collect_wire_edges, Entity, Marks};
use crate::topology::{
    edge, face, vertex, vertexuse, Brep, EdgeId, EdgeuseId, RegionId, ShellId, VertexId,
};

const OP: &str = "boolean::merge::p5::process_wires_and_shell_vertices";

/// The region a point classified to, and whether it is void (`pRegion->IsVoid()` as a
/// C++ condition: `UNSURE` is non-zero).
struct LocatedRegion {
    id: RegionId,
    is_void: bool,
}

/// `pBrep->Point3DClassify(point, SM_EFF_ZERO, boundary, sPC)` with editing disabled
/// ([`p1::classify_point_editing_disabled`]; the classification's construction
/// tolerance, the source object's, is not read) and the region it lies in: a region
/// class gives its region; with `accept_face_or_edge` (the primary-wire pass) a face
/// gives its upward face use's shell's region and an edge its primary edge use's
/// shell's region. `None` for any other class: the wire passes then signal `SE(SM_ERR)`
/// and go on, the shell-vertex passes return `SM_ERR`. `query` is `b`'s: the callers keep
/// one across classifications while `b` is not edited.
fn classify_region(
    k: &Kernel,
    query: &BrepQuery<'_>,
    b: &mut Brep,
    point: Point3,
    boundary_intersections: bool,
    accept_face_or_edge: bool,
) -> Result<Option<LocatedRegion>> {
    let pc =
        p1::classify_point_editing_disabled(k, query, b, point, EFF_ZERO, boundary_intersections)?;
    let shell_region = |shell: Option<ShellId>| -> Result<RegionId> {
        let shell = shell.ok_or_else(|| SolidError::null_pointer(OP))?;
        b.shell(shell)?
            .region
            .ok_or_else(|| SolidError::null_pointer(OP))
    };
    let region = match (pc.point_class(), pc.object()) {
        (PointClassType::Region, Some(EntityRef::Region(r))) => r,
        (PointClassType::Face, Some(EntityRef::Face(f))) if accept_face_or_edge => {
            let fu = face::p2::upward_faceuse(b, f)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            shell_region(b.faceuse(fu)?.shell)?
        }
        (PointClassType::Edge, Some(EntityRef::Edge(e))) if accept_face_or_edge => {
            let edgeuse = eu::primary_edgeuse(b, e)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            shell_region(eu::shell(b, edgeuse)?)?
        }
        (PointClassType::Region | PointClassType::Face | PointClassType::Edge, None) => {
            return Err(SolidError::null_pointer(OP))
        }
        _ => return Ok(None),
    };
    Ok(Some(LocatedRegion {
        id: region,
        is_void: b.region(region)?.is_void() != 0,
    }))
}

/// The deletion environment over the caller-owned records, attributes and primary
/// identities of `copy`.
fn delete_env<'a>(copy: &'a mut VertexCopy<'_>) -> DeleteEnv<'a> {
    DeleteEnv {
        records: &mut *copy.records,
        attributes: &mut *copy.attributes,
        key_of: copy.destination_key,
        context: None,
    }
}

/// `SmBrep::DeleteEdge(edge)` (default `pOptSurfaceToKeep = NULL`).
fn delete_edge(
    k: &mut Kernel,
    brep: &mut Brep,
    copy: &mut VertexCopy<'_>,
    edge: EdgeId,
) -> Result<()> {
    p4::delete_edge(k, brep, &mut delete_env(copy), edge, None)
}

/// `SmBrep::DeleteVertex(vertex)`.
fn delete_vertex(
    k: &mut Kernel,
    brep: &mut Brep,
    copy: &mut VertexCopy<'_>,
    vertex: VertexId,
) -> Result<()> {
    p4::delete_vertex(k, brep, &mut delete_env(copy), vertex)
}

fn midpoint(k: &Kernel, b: &Brep, id: EdgeId) -> Result<Point3> {
    let edge = b.edge(id)?;
    let curve = edge
        .curve
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    curve.evaluate_point(k, edge.interval.evaluate(0.5))
}

fn removes_primary(operation: BooleanOperation) -> bool {
    matches!(
        operation,
        BooleanOperation::Union
            | BooleanOperation::Intersection
            | BooleanOperation::Difference
            | BooleanOperation::Slice
    )
}

fn remove_in_region(operation: BooleanOperation, is_void: bool) -> bool {
    (matches!(
        operation,
        BooleanOperation::Union | BooleanOperation::Difference | BooleanOperation::Slice
    ) && !is_void)
        || (operation == BooleanOperation::Intersection && is_void)
}

fn keep_other(operation: BooleanOperation, is_void: bool) -> bool {
    match operation {
        BooleanOperation::Union => is_void,
        BooleanOperation::Difference | BooleanOperation::Slice | BooleanOperation::Intersection => {
            !is_void
        }
        BooleanOperation::ExclusiveOr
        | BooleanOperation::Merge
        | BooleanOperation::PartialMerge => true,
        _ => false,
    }
}

/// Remove unwanted primary wires and shell vertices, collect mapped primary
/// edges that must survive face deletion, then copy needed other geometry.
/// The caller supplies the already-established intersection marks (not advanced).
/// `keep_as_wires` is cleared even when the first subsequent operation fails.
/// Missing callees return [`SolidError::Unported`] at the first call reached;
/// malformed topology or missing mates return an error instead of dereferencing
/// a null pointer. The mate maps must refer to the supplied operands. `other` is
/// mutable because relating a kept vertex (`SmTopologyIntersector::Relate`) raises the
/// smaller of the two vertex tolerances, in whichever operand holds it.
#[allow(clippy::too_many_arguments)]
pub fn process_wires_and_shell_vertices(
    k: &mut Kernel,
    brep: &mut Brep,
    other: &mut Brep,
    brep_marks: &mut Marks,
    other_marks: &mut Marks,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
    context: Option<&mut Context>,
    operation: BooleanOperation,
    brep_mark: MarkType,
    other_mark: MarkType,
    keep_as_wires: &mut Vec<EdgeId>,
) -> Result<()> {
    let mut stage = SharedStage { k, context, copy };
    let keep_vertices = collect_wires_and_shell_vertices(
        &mut stage,
        brep,
        other,
        brep_marks,
        other_marks,
        relations,
        operation,
        brep_mark,
        other_mark,
        keep_as_wires,
    )?;
    for (vertex, region) in keep_vertices {
        p1::merge_vertex(
            brep,
            other,
            Some(vertex),
            Some(region),
            relations,
            &mut stage.copy.vertices,
            stage.context.as_deref_mut(),
        )?;
    }
    Ok(())
}

/// The shared wire stage with live, possibly distinct operand kernels.
/// Marks are read from their owning contexts; copied other edges and vertices notify
/// through the other context (or the shared primary context), and curves are copied
/// with the primary kernel. Caller records and attributes remain in `copy`; no
/// replacement context or attribute store is constructed.
#[allow(clippy::too_many_arguments)]
pub fn process_wires_and_shell_vertices_with_kernels(
    primary_kernel: &mut Kernel,
    other_kernel: Option<&mut Kernel>,
    brep: &mut Brep,
    other: &mut Brep,
    brep_marks: &mut Marks,
    other_marks: &mut Marks,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
    operation: BooleanOperation,
    brep_mark: MarkType,
    other_mark: MarkType,
    keep_as_wires: &mut Vec<EdgeId>,
) -> Result<()> {
    let mut stage = SplitStage {
        primary: primary_kernel,
        other: other_kernel,
        copy,
    };
    let keep_vertices = collect_wires_and_shell_vertices(
        &mut stage,
        brep,
        other,
        brep_marks,
        other_marks,
        relations,
        operation,
        brep_mark,
        other_mark,
        keep_as_wires,
    )?;
    let source_context = &mut stage.other.unwrap_or(stage.primary).context;
    for (vertex, region) in keep_vertices {
        p1::merge_vertex(
            brep,
            other,
            Some(vertex),
            Some(region),
            relations,
            &mut stage.copy.vertices,
            Some(&mut *source_context),
        )?;
    }
    Ok(())
}

/// The kernels of the wire stage: the primary kernel reads the primary marks, the
/// other kernel the other marks, and an other-operand edge is merged (`MergeEdge`)
/// with the primary kernel and notified through the source context.
trait StageKernels {
    fn primary(&self) -> &Kernel;
    fn other(&self) -> &Kernel;
    /// `SmBrep::DeleteEdge(edge)` on the primary B-rep, notified through the
    /// primary kernel's context and the copy's records, attributes and identities.
    fn delete_edge(&mut self, brep: &mut Brep, edge: EdgeId) -> Result<()>;
    /// `SmBrep::DeleteVertex(vertex)` on the primary B-rep (as `delete_edge`).
    fn delete_vertex(&mut self, brep: &mut Brep, vertex: VertexId) -> Result<()>;
    fn merge_edge(
        &mut self,
        brep: &mut Brep,
        other: &mut Brep,
        relations: &mut Relations,
        edgeuse: Option<EdgeuseId>,
        region: RegionId,
    ) -> Result<()>;
}

/// One kernel for both operands and a separate notification context.
struct SharedStage<'a, 'c> {
    k: &'a mut Kernel,
    context: Option<&'a mut Context>,
    copy: &'a mut TopologyCopy<'c>,
}

impl StageKernels for SharedStage<'_, '_> {
    fn primary(&self) -> &Kernel {
        self.k
    }
    fn other(&self) -> &Kernel {
        self.k
    }
    fn delete_edge(&mut self, brep: &mut Brep, edge: EdgeId) -> Result<()> {
        delete_edge(self.k, brep, &mut self.copy.vertices, edge)
    }
    fn delete_vertex(&mut self, brep: &mut Brep, vertex: VertexId) -> Result<()> {
        delete_vertex(self.k, brep, &mut self.copy.vertices, vertex)
    }
    fn merge_edge(
        &mut self,
        brep: &mut Brep,
        other: &mut Brep,
        relations: &mut Relations,
        edgeuse: Option<EdgeuseId>,
        region: RegionId,
    ) -> Result<()> {
        p2::merge_edge(
            self.k,
            brep,
            other,
            edgeuse,
            Some(region),
            relations,
            self.copy,
            self.context.as_deref_mut(),
        )
        .map(|_| ())
    }
}

/// Each operand's own kernel (the other one may be the primary's).
struct SplitStage<'a, 'c> {
    primary: &'a mut Kernel,
    other: Option<&'a mut Kernel>,
    copy: &'a mut TopologyCopy<'c>,
}

impl StageKernels for SplitStage<'_, '_> {
    fn primary(&self) -> &Kernel {
        self.primary
    }
    fn other(&self) -> &Kernel {
        self.other.as_deref().unwrap_or(self.primary)
    }
    fn delete_edge(&mut self, brep: &mut Brep, edge: EdgeId) -> Result<()> {
        delete_edge(self.primary, brep, &mut self.copy.vertices, edge)
    }
    fn delete_vertex(&mut self, brep: &mut Brep, vertex: VertexId) -> Result<()> {
        delete_vertex(self.primary, brep, &mut self.copy.vertices, vertex)
    }
    fn merge_edge(
        &mut self,
        brep: &mut Brep,
        other: &mut Brep,
        relations: &mut Relations,
        edgeuse: Option<EdgeuseId>,
        region: RegionId,
    ) -> Result<()> {
        let ctx = match self.other.as_deref_mut() {
            Some(o) => MergeContext::Split(self.primary, Some(&mut o.context)),
            None => MergeContext::Shared(self.primary),
        };
        p2::merge_edge_with(
            ctx,
            brep,
            other,
            edgeuse,
            Some(region),
            relations,
            self.copy,
        )
        .map(|_| ())
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn collect_wires_and_shell_vertices(
    kernels: &mut impl StageKernels,
    brep: &mut Brep,
    other: &mut Brep,
    brep_marks: &mut Marks,
    other_marks: &mut Marks,
    relations: &mut Relations,
    operation: BooleanOperation,
    brep_mark: MarkType,
    other_mark: MarkType,
    keep_as_wires: &mut Vec<EdgeId>,
) -> Result<Vec<(VertexId, RegionId)>> {
    keep_as_wires.clear();
    let mut collected = Vec::new();
    // A pass keeps one query for the B-rep it classifies against, which it does not
    // edit; the other-wire pass makes a fresh one per classification (MergeEdge edits
    // `brep` in between).
    let other_query = BrepQuery::of_brep(other);
    let mut done = !removes_primary(operation);
    while !done {
        done = true;
        let mut edges: Vec<_> = brep.edges().ids().map(Some).collect();
        for i in 0..edges.len() {
            let Some(id) = edges.get_mut(i).and_then(Option::take) else {
                continue;
            };
            if !vertexuse::edge_is_wire(brep, id)?
                || brep_marks.is_marked(kernels.primary(), Entity::Edge(id), brep_mark)
            {
                continue;
            }
            let point = midpoint(kernels.primary(), brep, id)?;
            let region = classify_region(kernels.other(), &other_query, other, point, true, true)?;
            let Some(region) = region else { continue };
            collect_wire_edges(
                kernels.primary(),
                brep,
                brep_marks,
                Some(id),
                brep_mark,
                &mut collected,
                None,
            )?;
            if remove_in_region(operation, region.is_void) {
                for &edge_id in &collected {
                    done = false;
                    let v1 =
                        edge::vertex(brep, edge_id)?.ok_or_else(|| SolidError::null_pointer(OP))?;
                    let v2 = edge::other_vertex(brep, edge_id, Some(v1))?
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    if let Some(slot) = edges.iter_mut().find(|slot| **slot == Some(edge_id)) {
                        *slot = None;
                    }
                    kernels.delete_edge(brep, edge_id)?;
                    for v in [Some(v1), (v1 != v2).then_some(v2)].into_iter().flatten() {
                        if vertex::is_shell_vertex(brep, v)?
                            && relations.vertices.other(v).is_none()
                        {
                            kernels.delete_vertex(brep, v)?;
                        }
                    }
                }
            }
        }
    }

    // m_vTI.m_pOther->GetEdges(sEdges): the list is taken before any edge is merged.
    let other_edges: Vec<EdgeId> = other.edges().ids().collect();
    for id in other_edges {
        let is_wire = vertexuse::edge_is_wire(other, id)?;
        let marked = other_marks.is_marked(kernels.other(), Entity::Edge(id), other_mark);
        if is_wire && marked {
            match operation {
                BooleanOperation::Union
                | BooleanOperation::Intersection
                | BooleanOperation::Merge
                | BooleanOperation::PartialMerge => {
                    let mate = relations
                        .edges
                        .primary(id)
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    brep.edge(mate)?;
                    keep_as_wires.push(mate);
                }
                // Upstream SE(SM_ERR) records the error but does not return.
                _ => {}
            }
            continue;
        } else if marked || !is_wire {
            continue;
        }
        let point = midpoint(kernels.other(), other, id)?;
        let region = classify_region(
            kernels.primary(),
            &BrepQuery::of_brep(brep),
            brep,
            point,
            true,
            false,
        )?;
        let Some(region) = region else { continue };
        collect_wire_edges(
            kernels.other(),
            other,
            other_marks,
            Some(id),
            other_mark,
            &mut collected,
            None,
        )?;
        if keep_other(operation, region.is_void) {
            for &edge in &collected {
                // merge the edge into the Brep model
                let edgeuse = eu::primary_edgeuse(other, edge)?;
                kernels.merge_edge(brep, other, relations, edgeuse, region.id)?;
            }
        }
    }

    if removes_primary(operation) {
        let other_query = BrepQuery::of_brep(other);
        let vertices: Vec<_> = brep.vertices().ids().collect();
        for v in vertices {
            if brep_marks.is_marked(kernels.primary(), Entity::Vertex(v), brep_mark)
                || !vertex::is_shell_vertex(brep, v)?
            {
                continue;
            }
            let point = brep.vertex(v)?.point;
            let region =
                classify_region(kernels.other(), &other_query, other, point, false, false)?
                    .ok_or_else(|| SolidError::sm_err(OP))?;
            if remove_in_region(operation, region.is_void) {
                kernels.delete_vertex(brep, v)?;
            }
        }
    }

    let brep_query = BrepQuery::of_brep(brep);
    let mut keep_vertices = Vec::new();
    for v in other.vertices().ids() {
        if other_marks.is_marked(kernels.other(), Entity::Vertex(v), other_mark)
            || !vertex::is_shell_vertex(other, v)?
        {
            continue;
        }
        let point = other.vertex(v)?.point;
        let region = classify_region(kernels.primary(), &brep_query, brep, point, false, false)?
            .ok_or_else(|| SolidError::sm_err(OP))?;
        if keep_other(operation, region.is_void) {
            keep_vertices.push((v, region.id));
        }
    }
    Ok(keep_vertices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::{AttributeStore, ObjectKey};
    use crate::geom::object::ObjectRef;

    use std::collections::BTreeMap;

    use crate::curve::line::Line;
    use crate::geom::context::MarkLock;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::topology::{Edge, Edgeuse, EdgeuseOwner, Vertexuse, VertexuseOwner};

    fn copy<'a>(
        records: &'a mut crate::topology::topology::NotifyRecords,
        attributes: &'a mut AttributeStore,
    ) -> TopologyCopy<'a> {
        TopologyCopy {
            vertices: VertexCopy {
                records,
                attributes,
                source_key: &|v| ObjectRef {
                    key: ObjectKey(1_000_000 + u64::from(v.index())),
                    type_code: 16021,
                },
                destination_key: &|e| match e {
                    EntityRef::Vertex(v) => ObjectRef {
                        key: ObjectKey(u64::from(v.index())),
                        type_code: 16021,
                    },
                    EntityRef::Vertexuse(v) => ObjectRef {
                        key: ObjectKey(2_000_000 + u64::from(v.index())),
                        type_code: 16006,
                    },
                    _ => ObjectRef {
                        key: ObjectKey(3_000_000),
                        type_code: 16001,
                    },
                },
            },
            edge_source_key: &|e| ObjectRef {
                key: ObjectKey(5_000_000 + u64::from(e.index())),
                type_code: 16016,
            },
            edge_destination_key: &|e| ObjectRef {
                key: ObjectKey(4_000_000 + u64::from(e.index())),
                type_code: 16016,
            },
            face_source_key: &|f| ObjectRef {
                key: ObjectKey(7_000_000 + u64::from(f.index())),
                type_code: 16002,
            },
            face_destination_key: &|f| ObjectRef {
                key: ObjectKey(6_000_000 + u64::from(f.index())),
                type_code: 16002,
            },
        }
    }

    fn wire(b: &mut Brep) -> EdgeId {
        let shell = b.add_shell().unwrap();
        let p = Point3::new(0.0, 0.0, 0.0);
        let q = Point3::new(1.0, 0.0, 0.0);
        let v = b.add_vertex(p).unwrap();
        let w = b.add_vertex(q).unwrap();
        let e = b.add_edge_record(Edge::new([v, w])).unwrap();
        for (vertex, orientation) in [(v, OrientType::Same), (w, OrientType::Opposite)] {
            let eu = b
                .add_edgeuse_record(Edgeuse {
                    edge: Some(e),
                    owner: EdgeuseOwner::Shell(Some(shell)),
                    orientation,
                    ..Edgeuse::default()
                })
                .unwrap();
            let vu = b
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(vertex),
                    owner: VertexuseOwner::Edgeuse(Some(eu)),
                    ..Vertexuse::default()
                })
                .unwrap();
            b.edgeuse_mut(eu).unwrap().vertexuse = Some(vu);
            b.edge_mut(e).unwrap().edgeuses.push(eu);
            b.vertex_mut(vertex).unwrap().vertexuses.push(vu);
        }
        let edge = b.edge_mut(e).unwrap();
        edge.curve = Some(Line::from_points(p, q, 3).into());
        edge.interval = crate::geom::extent1d::Extent1d::from_raw(0.0, 1.0);
        e
    }

    #[test]
    fn output_resets_and_preserves_mates_before_a_deferred_call() {
        let mut k = Kernel::default();
        let lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, 1).unwrap();
        let mt = lock.mark_type();
        let mut a = Brep::new();
        let ae = wire(&mut a);
        let mut b = Brep::new();
        let be = wire(&mut b);
        wire(&mut b);
        let mut am = Marks::default();
        let mut bm = Marks::default();
        am.mark(&k, Entity::Edge(ae), mt);
        bm.mark(&k, Entity::Edge(be), mt);
        let mut relations = Relations::default();
        relations.edges.record(ae, be).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::new();
        let mut keep = vec![EdgeId::from_index(u32::MAX)];
        let result = process_wires_and_shell_vertices(
            &mut k,
            &mut a,
            &mut b,
            &mut am,
            &mut bm,
            &mut relations,
            &mut copy(&mut records, &mut attrs),
            None,
            BooleanOperation::Union,
            mt,
            mt,
            &mut keep,
        );
        // the second wire's classification fails (its ray fire finds no region:
        // `SM_ERR` from `Point3DClassify`)
        assert!(
            matches!(result, Err(SolidError::Upstream { code: 1001, .. })),
            "{result:?}"
        );
        assert_eq!(keep, [ae]);
    }

    #[test]
    fn missing_and_extreme_mates_return_errors_without_panicking() {
        let mut k = Kernel::default();
        let lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, 1).unwrap();
        let mt = lock.mark_type();
        let mut a = Brep::new();
        let ae = wire(&mut a);
        let mut b = Brep::new();
        let be = wire(&mut b);
        let mut am = Marks::default();
        let mut bm = Marks::default();
        am.mark(&k, Entity::Edge(ae), mt);
        bm.mark(&k, Entity::Edge(be), mt);
        for edges in [
            BTreeMap::new(),
            BTreeMap::from([(be, EdgeId::from_index(u32::MAX))]),
        ] {
            let mut relations = Relations::default();
            for (&other, &primary) in &edges {
                relations.edges.record(primary, other).unwrap();
            }
            let mut records = crate::topology::topology::NotifyRecords::new();
            let mut attrs = AttributeStore::new();
            for operation in [
                BooleanOperation::Union,
                BooleanOperation::Intersection,
                BooleanOperation::Merge,
                BooleanOperation::PartialMerge,
            ] {
                let mut keep = vec![ae];
                assert!(process_wires_and_shell_vertices(
                    &mut k,
                    &mut a,
                    &mut b,
                    &mut am,
                    &mut bm,
                    &mut relations,
                    &mut copy(&mut records, &mut attrs),
                    None,
                    operation,
                    mt,
                    mt,
                    &mut keep,
                )
                .is_err());
                assert!(keep.is_empty());
            }
            for operation in [
                BooleanOperation::Unknown,
                BooleanOperation::Difference,
                BooleanOperation::ExclusiveOr,
                BooleanOperation::Imprint,
                BooleanOperation::ImprintClassify,
                BooleanOperation::ExtractSeparate,
                BooleanOperation::Slice,
            ] {
                let mut keep = vec![ae];
                process_wires_and_shell_vertices(
                    &mut k,
                    &mut a,
                    &mut b,
                    &mut am,
                    &mut bm,
                    &mut relations,
                    &mut copy(&mut records, &mut attrs),
                    None,
                    operation,
                    mt,
                    mt,
                    &mut keep,
                )
                .unwrap();
                assert!(keep.is_empty(), "{operation:?}");
            }
        }
    }
}
