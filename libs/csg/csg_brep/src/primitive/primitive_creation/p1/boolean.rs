//! Planar boolean classification, copying and cleanup after intersection insertion.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp` (`Boolean2D`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `finish` (private) | `SmPrimitiveCreation::Boolean2D`, from `IntersectInsertRelate` on |
//!
//! Deviations: marks are caller-owned records, separately for each arena. Both
//! operands use the passed Kernel. On success the result owns the primary arena;
//! the two consumed input slots become empty arenas. The local `SmTopologyIntersector`
//! is merge p4's [`KernelManifoldIntersector`] over the two arenas, notifying under
//! the caller's `key_of` for both operands (the identities are unique within one
//! B-rep, as the arena notification scheme's are), a split edge's new curve under its
//! edge's identity with the curve's type code. No recursion.

use super::primitives::{detailed_face, stitch_faces, Boolean2d};
use crate::boolean::merge::p4::{
    BooleanKernels, KernelManifoldIntersector, ManifoldIntersector, ManifoldKernelAdapter,
    ManifoldNotificationKeys,
};
use crate::error::{ignore_status, EntityRef, Result, SolidError};
use crate::geom::attribute::AttributeStore;
use crate::geom::context::{Context, MarkLock};
use crate::geom::object::ObjectRef;
use crate::kernel::Kernel;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::edge;
use crate::topology::edgeuse::p1 as eu;
use crate::topology::topology_traverser::{Entity, Marks};
use crate::topology::{Brep, EdgeId, FaceId};

fn remove_topology(
    k: &mut Kernel,
    a: &mut Brep,
    notifications: &mut edge::NotifyEnv<'_>,
) -> Result<()> {
    crate::topology::brep::p5::remove_topological_with_notifications(
        k,
        a,
        None,
        None,
        Some(notifications),
    )
}

pub(super) fn finish(
    k: &mut Kernel,
    a: &mut Brep,
    b: &mut Brep,
    first: FaceId,
    operation: Boolean2d,
    result: &mut Option<Brep>,
    marks: (&mut Marks, &mut Marks),
    notifications: &mut edge::NotifyEnv<'_>,
) -> Result<()> {
    // SmTopologyIntersector sTI(*pContext, pBrepA, pBrepB,
    // SM_ZONE_TO_APPROXTOL3D(dZoneTol3d), 20.0*SM_PI/180.0)
    let tolerance = crate::math::max2(a.tolerance(), b.tolerance());
    let key_of = notifications.key_of;
    let curve = |e: EdgeId, kind: i64| {
        Ok(ObjectRef {
            type_code: kind,
            ..key_of(EntityRef::Edge(e))
        })
    };
    let no_sources = |_| {
        Err(SolidError::invalid_input(
            OP,
            "Boolean2D propagates no region sources",
        ))
    };
    let keys = ManifoldNotificationKeys {
        primary: key_of,
        other: key_of,
        primary_curve: &curve,
        other_curve: &curve,
        source_lists: &no_sources,
        actual_curve_identities: false,
    };
    let attributes = &mut *notifications.attributes;
    let context = &mut notifications.context;
    KernelManifoldIntersector::with_notifications(keys, notifications.records, |ti| {
        ti.set_intersection_tolerances(tolerance / 2.0, 20.0 * crate::math::PI / 180.0);
        let session = Session {
            ti,
            attributes,
            context,
            key_of,
        };
        finish_with(k, a, b, first, operation, result, marks, session)
    })
}

const OP: &str = "primitive::boolean_2d";

/// The local `SmTopologyIntersector` and the caller's notification state.
struct Session<'s, 'k, 'c> {
    ti: &'s mut ManifoldKernelAdapter<'k>,
    attributes: &'s mut AttributeStore,
    context: &'s mut Option<&'c mut Context>,
    key_of: &'s dyn Fn(EntityRef) -> ObjectRef,
}

impl Session<'_, '_, '_> {
    fn env(&mut self) -> edge::NotifyEnv<'_> {
        edge::NotifyEnv {
            records: &mut self.ti.records,
            attributes: &mut *self.attributes,
            context: self.context.as_deref_mut(),
            key_of: self.key_of,
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn finish_with(
    k: &mut Kernel,
    a: &mut Brep,
    b: &mut Brep,
    first: FaceId,
    operation: Boolean2d,
    result: &mut Option<Brep>,
    marks: (&mut Marks, &mut Marks),
    mut session: Session<'_, '_, '_>,
) -> Result<()> {
    // intersect, merge, and relate the intersection geometry between the two topology graphs
    session.ti.intersect_insert_relate(
        &mut BooleanKernels {
            primary: k,
            other: None,
        },
        a,
        b,
        session.attributes,
    )?;
    let mut lock = MarkLock::new_pair(core::slice::from_mut(&mut k.context), 0, 0)?;
    let mark_a = lock.mark_type1();
    let mark_b = lock.mark_type2();
    let (marks_a, marks_b) = marks;
    let outcome = (|| {
        // get coincident edges, then coincident vertices
        let (edges, other_edges) = session.ti.common_edges(a, b)?;
        for (primary, other) in edges.iter().zip(&other_edges) {
            marks_a.mark(k, Entity::Edge(*primary), mark_a);
            marks_b.mark(k, Entity::Edge(*other), mark_b);
        }
        let (vertices, other_vertices) = session.ti.common_vertices(a, b)?;
        for (primary, other) in vertices.iter().zip(&other_vertices) {
            marks_a.mark(k, Entity::Vertex(*primary), mark_a);
            marks_b.mark(k, Entity::Vertex(*other), mark_b);
        }
        // The primary side propagates a failed radial-sector status; the other
        // side skips it. Mark each face BEFORE the radial query, as upstream.
        for (&primary, &other) in edges.iter().zip(&other_edges) {
            for edgeuse in edge::edgeuses(a, primary, None, None)?
                .into_iter()
                .step_by(2)
            {
                let face = eu::face(a, edgeuse)?.ok_or_else(|| SolidError::null_pointer(OP))?;
                if marks_a.is_marked(k, Entity::Face(face), mark_a) {
                    continue;
                }
                marks_a.mark(k, Entity::Face(face), mark_a);
                let sector = edge::find_radial_sector_between(
                    k,
                    b,
                    other,
                    Some(a),
                    edgeuse,
                    None,
                    OrientType::Same,
                    None,
                )?;
                if let Some(faceuse) = sector.coincident_faceuse {
                    let mate = eu::faceuse_face(b, faceuse)?
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    marks_b.mark(k, Entity::Face(mate), mark_b);
                    // sTI.Relate(pF,pOFace), its status unchecked
                    ignore_status(session.ti.relate_faces(a, face, b, mate))?;
                }
            }
            for edgeuse in edge::edgeuses(b, other, None, None)?.into_iter().step_by(2) {
                let face = eu::face(b, edgeuse)?.ok_or_else(|| SolidError::null_pointer(OP))?;
                if marks_b.is_marked(k, Entity::Face(face), mark_b) {
                    continue;
                }
                marks_b.mark(k, Entity::Face(face), mark_b);
                let sector = match edge::find_radial_sector_between(
                    k,
                    a,
                    primary,
                    Some(b),
                    edgeuse,
                    None,
                    OrientType::Same,
                    None,
                ) {
                    Ok(value) => value,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue,
                };
                if let Some(faceuse) = sector.coincident_faceuse {
                    let mate = eu::faceuse_face(a, faceuse)?
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    marks_a.mark(k, Entity::Face(mate), mark_a);
                    session.ti.relate_faces(a, mate, b, face)?;
                }
            }
        }
        let mut delete = Vec::new();
        for face in crate::topology::brep::p1::faces(a, None)? {
            // sTI.GetOtherMate(pF)
            let common = session.ti.face_mate(face).is_some();
            if matches!(operation, Boolean2d::ExclusiveOr | Boolean2d::Difference) && common
                || operation == Boolean2d::Intersection && !common
            {
                delete.push(face);
            }
        }
        // Keep the original A faces until every B classification/copy is done.
        let mut covering = first;
        for face in crate::topology::brep::p1::faces(b, None)? {
            // sTI.GetBrepMate(pOFace)
            let common = session.ti.merge.intersector.faces.primary(face).is_some();
            if matches!(
                operation,
                Boolean2d::ExclusiveOr | Boolean2d::Union | Boolean2d::Merge
            ) && !common
            {
                let boundary = crate::topology::face::p2::create_curves_from_face(
                    k,
                    b,
                    face,
                    OrientType::Same,
                    true,
                    false,
                )?;
                let original = a
                    .face(covering)?
                    .surface
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                let surface = if original.owner().is_some() {
                    original.copy(k)?
                } else {
                    original.clone_for_evaluation()?
                };
                let domain = surface.natural_uv_domain_with_nurb(k)?;
                covering = detailed_face(
                    k,
                    a,
                    a.infinite_region(),
                    boundary,
                    surface,
                    domain,
                    &mut session.env(),
                )?;
            }
        }
        // sTI.DeleteFaces(sDelFaces), its status unchecked
        ignore_status(session.ti.delete_faces(
            &mut BooleanKernels {
                primary: k,
                other: None,
            },
            a,
            session.attributes,
            &delete,
            None,
        ))?;
        if operation != Boolean2d::Merge {
            stitch_faces(k, a, a.tolerance())?;
            remove_topology(k, a, &mut session.env())?;
        }
        *result = Some(core::mem::take(a));
        *b = Brep::new();
        Ok(())
    })();
    lock.release(core::slice::from_mut(&mut k.context))?;
    outcome
}
