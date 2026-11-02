//! Concrete arena adapter for the manifold Boolean's shared merge stages.
//!
//! Upstream: `source/SMLib/src/SmMerge.cpp`, `source/SMLib/src/SmTopologyIntersector.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`KernelManifoldIntersector`] | `SmMerge::m_vTI` and shared merge-stage access |
//! | [`KernelManifoldIntersector::delete_faces_in`] | `SmTopologyIntersector(ctx, brep, brep, ..).DeleteFaces` over one B-rep |
//!
//! Deviations: the existing typed `Relations` is the sole mate store. Object
//! keys either encode a caller-reserved namespace, operand, class and arena id,
//! or use caller identity callbacks and the caller's actual stores.
//! Cache evaluation preserves the actual operand kernel settings; owner checks
//! use the same identity callbacks as notifications. Retained merge state and
//! caller records return to their owners on errors and unwinding callbacks.
//! Source-list notification handles resolve back into that same attribute store.
//! Surface identity follows the canonical B-rep cache convention: its owner,
//! or its containing face when the owned surface has no explicit owner.
//! No geometry, relationship algorithm or input-dependent recursion is added.

use super::arena_pair::{self, ArenaPair, H};
use super::notification_keys::{ManifoldNotificationKeys, NotificationKeys};
use super::{
    manifold_boolean, BooleanKernels, FixedGaps, MadeFaceuse, ManifoldIntersector, ManifoldState,
    MergeEntity,
};
use crate::boolean::merge::p1::{self, MergeState};
use crate::boolean::merge::p2::{self, MergeContext, MergeOperand, TopologyCopy};
use crate::boolean::merge::p5;
use crate::boolean::topology_intersector::p1::TopologyIntersector;
use crate::boolean::topology_intersector::p1::{set_subset_objects, Relations};
use crate::error::{Result, SolidError};
use crate::geom::a_object::{BooleanOperation, REGION_TYPE};
use crate::geom::attribute::{ids, AttributeStore, ObjectKey};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::{Context, MarkLock, MarkType};
use crate::geom::object::ObjectRef;
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::topology::brep::p1 as bp;
use crate::topology::brep_cache::{arena_identity, CacheObject};
use crate::topology::shell::{RegionLookup, RegionRef};
#[cfg(test)]
use crate::topology::topology::Topology;
use crate::topology::{Brep, BrepId, EdgeId, FaceId, FaceuseId, RegionId, VertexId};
use crate::EntityRef;
use std::collections::{BTreeMap, BTreeSet};

const OP: &str = "boolean::merge::p4::kernel_adapter";

fn key(namespace: u32, other: bool, kind: u64, index: u32) -> ObjectKey {
    ObjectKey(
        (u64::from(namespace) << 37) | (u64::from(other) << 36) | (kind << 32) | u64::from(index),
    )
}

// Cache construction takes only &Kernel; preserve the caller's settings and
// RNG state for that read-only work rather than evaluating under defaults.
fn cache_kernel(k: &Kernel, context: &Context) -> Kernel {
    Kernel {
        tolerances: k.tolerances,
        rng: k.rng.clone(),
        caches: crate::kernel::Caches::default(),
        context: context.metadata_copy(),
        fillet_point_step: core::cell::Cell::new(k.fillet_point_step.get()),
        upstream_ub_passed: Vec::new(),
        memoize_curve_caches: k.memoize_curve_caches,
        fixes: k.fixes,
        notify_records: crate::topology::topology::NotifyRecords::new(),
    }
}

fn cache_kernels(k: &Kernel, other: Option<&Kernel>, context: &Context) -> [Kernel; 2] {
    [
        cache_kernel(k, context),
        other.map_or_else(|| cache_kernel(k, context), |k| cache_kernel(k, &k.context)),
    ]
}

// Every topology class has its own slot in the caller-reserved namespace.
pub(crate) fn notification_key(namespace: u32, other: bool, entity: EntityRef) -> ObjectRef {
    let (kind, type_code, index) = match entity {
        EntityRef::Region(v) => (1, 16012, v.index()),
        EntityRef::Face(v) => (2, 16002, v.index()),
        EntityRef::Edge(v) => (3, 16016, v.index()),
        EntityRef::Vertex(v) => (4, 16021, v.index()),
        EntityRef::Brep(v) => (0, 16011, v.index()),
        EntityRef::Shell(v) => (7, 16013, v.index()),
        EntityRef::Loop(v) => (8, 16003, v.index()),
        EntityRef::Faceuse(v) => (9, 16022, v.index()),
        EntityRef::Loopuse(v) => (10, 16004, v.index()),
        EntityRef::Edgeuse(v) => (11, 16005, v.index()),
        EntityRef::Vertexuse(v) => (12, 16006, v.index()),
    };
    ObjectRef {
        key: key(namespace, other, kind, index),
        type_code,
    }
}

/// The operand slot (`true` for the other B-rep) and entity that [`notification_key`]
/// names `key` in `namespace`, if any (its inverse on keys).
pub(crate) fn entity_of_notification_key(
    namespace: u32,
    key: ObjectKey,
) -> Option<(bool, EntityRef)> {
    if key.0 >> 37 != u64::from(namespace) {
        return None;
    }
    let other = (key.0 >> 36) & 1 == 1;
    let kind = (key.0 >> 32) & 0xf;
    let index = u32::try_from(key.0 & u64::from(u32::MAX)).ok()?;
    let entity = match kind {
        0 => EntityRef::Brep(BrepId::from_index(index)),
        1 => EntityRef::Region(RegionId::from_index(index)),
        2 => EntityRef::Face(FaceId::from_index(index)),
        3 => EntityRef::Edge(EdgeId::from_index(index)),
        4 => EntityRef::Vertex(VertexId::from_index(index)),
        7 => EntityRef::Shell(crate::topology::ShellId::from_index(index)),
        8 => EntityRef::Loop(crate::topology::LoopId::from_index(index)),
        9 => EntityRef::Faceuse(FaceuseId::from_index(index)),
        10 => EntityRef::Loopuse(crate::topology::LoopuseId::from_index(index)),
        11 => EntityRef::Edgeuse(crate::topology::EdgeuseId::from_index(index)),
        12 => EntityRef::Vertexuse(crate::topology::VertexuseId::from_index(index)),
        _ => return None,
    };
    Some((other, entity))
}

/// Hands the geometry a merge made under the adapter's namespace back to the B-rep's
/// own naming: a face's surface (an edge's curve) owned under that face's (edge's)
/// [`notification_key`] in `namespace` is the record's own geometry (upstream's owner
/// is the record), so it takes the record's name in `b`'s recorded scheme
/// ([`Brep::naming`](crate::topology::Brep::naming): notification or arena identity)
/// and the face's tolerance is re-recorded on it. An edge use's UV trim curve owned
/// under the edge use's key takes the edge use's notification identity (upstream's owner
/// is the edge use; `SmEdgeuse::AssertValid` test #6). Called when the namespace ends with
/// the merge (its records and attributes go with it); a B-rep of another or unknown
/// scheme is left as it is.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling record (none in practice).
pub(crate) fn adopt_namespace_owners(b: &mut Brep, namespace: u32, other: bool) -> Result<()> {
    use crate::topology::NamingScheme;
    let name = |e: EntityRef| -> Option<ObjectRef> {
        match b.naming() {
            NamingScheme::Notify => Some(crate::topology::loopuse::notify_ref(e)),
            NamingScheme::Arena => Some(match e {
                EntityRef::Face(f) => arena_identity(CacheObject::Face(f)),
                EntityRef::Edge(x) => arena_identity(CacheObject::Edge(x)),
                _ => return None,
            }),
            _ => None,
        }
    };
    let faces: Vec<(FaceId, ObjectRef)> = bp::faces(b, None)?
        .into_iter()
        .filter_map(|f| name(EntityRef::Face(f)).map(|n| (f, n)))
        .collect();
    let edges: Vec<(EdgeId, ObjectRef)> = bp::edges(b, None)?
        .into_iter()
        .filter_map(|e| name(EntityRef::Edge(e)).map(|n| (e, n)))
        .collect();
    for (f, own) in faces {
        let merged = notification_key(namespace, other, EntityRef::Face(f));
        let adopted = b.face_mut(f)?.surface.as_mut().is_some_and(|surface| {
            let matched = surface.owner() == Some(merged);
            if matched {
                surface.set_owner(Some(own));
            }
            matched
        });
        if adopted {
            b.set_geometry_owner_key(EntityRef::Face(f), Some(own));
            b.sync_face_surface_tolerance(f)?;
        }
    }
    for (e, own) in edges {
        let merged = notification_key(namespace, other, EntityRef::Edge(e));
        let adopted = b.edge_mut(e)?.curve.as_mut().is_some_and(|curve| {
            let matched = curve.owner().map(|o| o.key) == Some(merged.key)
                && curve.owner().map(|o| o.type_code) == Some(merged.type_code);
            if matched {
                curve.set_owner(Some(own), None);
            }
            matched
        });
        if adopted {
            b.set_geometry_owner_key(EntityRef::Edge(e), Some(own));
        }
    }
    // An edge use's UV trim curve is owned by the edge use (`SmEdgeuse` test #6); one
    // the merge set under the namespace takes the edge use's own name. Both schemes
    // name edge uses by their notification identity (the check's keys for an arena
    // B-rep fall back to it too).
    if matches!(b.naming(), NamingScheme::Notify | NamingScheme::Arena) {
        let uses: Vec<_> = b.edgeuses().iter().map(|(id, _)| id).collect();
        for eu in uses {
            let merged = notification_key(namespace, other, EntityRef::Edgeuse(eu));
            if let Some(trim) = b.edgeuse_mut(eu)?.uv_trim_curve.as_mut() {
                if trim.owner() == Some(merged) {
                    let own = crate::topology::loopuse::notify_ref(EntityRef::Edgeuse(eu));
                    trim.set_owner(Some(own), None);
                }
            }
        }
    }
    Ok(())
}

// The original namespace adapter imports geometry with arena owner labels.
// Temporarily bind those owned objects to the same identities as its notifications
// while a native edit runs. Caller-key stores already use their actual identities.
struct GeometryOwnerScope<'a, 'keys> {
    body: &'a mut Brep,
    keys: NotificationKeys<'keys>,
    other: bool,
    edges: Vec<(EdgeId, ObjectRef)>,
    faces: Vec<(FaceId, ObjectRef)>,
}
impl<'a, 'keys> GeometryOwnerScope<'a, 'keys> {
    fn new(body: &'a mut Brep, keys: NotificationKeys<'keys>, other: bool) -> Result<Self> {
        use crate::topology::NamingScheme;
        let naming = body.naming();
        let mut scope = Self {
            body,
            keys,
            other,
            edges: Vec::new(),
            faces: Vec::new(),
        };
        if matches!(keys, NotificationKeys::Namespace(_)) {
            for edge in crate::topology::brep::p1::edges(scope.body, None)? {
                let expected = if naming == NamingScheme::Notify {
                    crate::topology::loopuse::notify_ref(EntityRef::Edge(edge))
                } else {
                    arena_identity(CacheObject::Edge(edge))
                };
                if let Some(curve) = scope.body.edge_mut(edge)?.curve.as_mut() {
                    if curve.owner() == Some(expected) {
                        scope.edges.push((edge, expected));
                        curve.set_owner(Some(keys.object(other, EntityRef::Edge(edge))), None);
                    }
                }
            }
            for face in crate::topology::brep::p1::faces(scope.body, None)? {
                let expected = if naming == NamingScheme::Notify {
                    crate::topology::loopuse::notify_ref(EntityRef::Face(face))
                } else {
                    arena_identity(CacheObject::Face(face))
                };
                let rebound = scope
                    .body
                    .face_mut(face)?
                    .surface
                    .as_mut()
                    .is_some_and(|surface| {
                        let matched = surface.owner() == Some(expected);
                        if matched {
                            surface.set_owner(Some(keys.object(other, EntityRef::Face(face))));
                        }
                        matched
                    });
                if rebound {
                    scope.faces.push((face, expected));
                    scope.body.sync_face_surface_tolerance(face)?;
                }
            }
        }
        Ok(scope)
    }
}
impl Drop for GeometryOwnerScope<'_, '_> {
    fn drop(&mut self) {
        for &(edge, expected) in &self.edges {
            if let Ok(record) = self.body.edge_mut(edge) {
                if let Some(curve) = record.curve.as_mut() {
                    if curve.owner() == Some(self.keys.object(self.other, EntityRef::Edge(edge))) {
                        curve.set_owner(Some(expected), None);
                    }
                }
            }
        }
        for &(face, expected) in &self.faces {
            let restored = self.body.face_mut(face).is_ok_and(|record| {
                record.surface.as_mut().is_some_and(|surface| {
                    let matched = surface.owner()
                        == Some(self.keys.object(self.other, EntityRef::Face(face)));
                    if matched {
                        surface.set_owner(Some(expected));
                    }
                    matched
                })
            });
            if restored {
                let _ = self.body.sync_face_surface_tolerance(face);
            }
        }
    }
}

/// The kernel adapter for the ported merge and topology-intersector stages.
/// The canonical merge state owns the single shared face/edge/vertex mate store;
/// notifications use `records` and the attributes supplied to each operation.
/// Missing dependency calls retain the trait's owner-tagged defaults.
#[derive(Debug)]
pub struct ManifoldKernelAdapter<'a> {
    /// Canonical options, tag and relationships, shared with the ported stages.
    pub merge: MergeState<Relations>,
    /// Topology notification state for both operands, under [`Self::object_key`].
    pub records: crate::topology::topology::NotifyRecords,
    /// `m_bTopologyDeleted`; dependency editing stages update this same flag.
    pub topology_deleted: bool,
    intersector: TopologyIntersector<H>,
    curves: Vec<crate::curve::curve::Curve>,
    extra_curves: Vec<crate::curve::curve::Curve>,
    keys: NotificationKeys<'a>,
    body_ids: (BrepId, BrepId),
    source_handles: BTreeMap<ObjectRef, (ObjectKey, bool)>,
    subset: Option<BTreeSet<ObjectRef>>,
}

/// Namespace-backed production adapter; its existing owned API is unchanged.
pub type KernelManifoldIntersector = ManifoldKernelAdapter<'static>;

struct NotificationRecordScope<'records, 'keys> {
    adapter: ManifoldKernelAdapter<'keys>,
    caller: &'records mut crate::topology::topology::NotifyRecords,
}

impl Drop for NotificationRecordScope<'_, '_> {
    fn drop(&mut self) {
        std::mem::swap(&mut self.adapter.records, self.caller);
    }
}

impl KernelManifoldIntersector {
    /// Create an adapter with default merge options and no relationships.
    /// The caller reserves `namespace` exclusively among all object keys in the
    /// supplied attribute/notification stores; its complete block is `2^37` keys. Keys
    /// stay below the kernel's reserved range (`geom::context::KERNEL_KEY_BASE`, bit
    /// 63), which `topology::brep::p3`'s edits refuse as caller keys, so the namespace
    /// has 26 bits.
    ///
    /// # Errors
    /// A namespace exceeding 26 bits.
    pub fn new(namespace: u32) -> Result<Self> {
        if namespace > 0x03ff_ffff {
            return Err(SolidError::invalid_input(
                OP,
                "object namespace exceeds 26 bits",
            ));
        }
        Ok(Self::from_keys(NotificationKeys::Namespace(namespace)))
    }

    /// The notification namespace of an adapter that names the operands' objects itself
    /// ([`Self::new`]); `None` for one using the caller's identities.
    #[must_use]
    pub(crate) const fn namespace(&self) -> Option<u32> {
        match self.keys {
            NotificationKeys::Namespace(n) => Some(n),
            NotificationKeys::Caller(_) => None,
        }
    }

    /// Borrow caller identities and move the actual records through one scoped
    /// operation. Records, including partial edits, are returned on both success
    /// and error, including unwinding caller callbacks; no alternate store is used
    /// during the operation. Attributes and live kernels are supplied to the usual
    /// adapter methods.
    pub fn with_notifications<T>(
        keys: ManifoldNotificationKeys<'_>,
        records: &mut crate::topology::topology::NotifyRecords,
        action: impl FnOnce(&mut ManifoldKernelAdapter<'_>) -> T,
    ) -> T {
        let mut scope = NotificationRecordScope {
            adapter: ManifoldKernelAdapter::from_keys(NotificationKeys::Caller(keys)),
            caller: records,
        };
        std::mem::swap(&mut scope.adapter.records, scope.caller);
        action(&mut scope.adapter)
    }
}

/// Restores the retained algorithm state and caller records even when a callback
/// unwinds. Borrowed key callbacks never escape the operation.
struct RetainedNotificationScope<'state, 'records, 'keys> {
    retained: &'state mut KernelManifoldIntersector,
    caller: &'records mut crate::topology::topology::NotifyRecords,
    adapter: ManifoldKernelAdapter<'keys>,
}
impl Drop for RetainedNotificationScope<'_, '_, '_> {
    fn drop(&mut self) {
        self.adapter.swap_state(self.retained);
        std::mem::swap(&mut self.adapter.records, self.caller);
    }
}
impl KernelManifoldIntersector {
    /// Bind caller identity callbacks and records to this retained merge engine.
    /// Canonical relationships, intersection lists and configuration survive the
    /// operation; key callbacks remain borrowed only for its duration.
    pub fn with_caller_notifications<T>(
        &mut self,
        keys: ManifoldNotificationKeys<'_>,
        body_ids: (BrepId, BrepId),
        records: &mut crate::topology::topology::NotifyRecords,
        action: impl FnOnce(&mut ManifoldKernelAdapter<'_>) -> T,
    ) -> T {
        let mut adapter = ManifoldKernelAdapter::from_keys(NotificationKeys::Caller(keys));
        adapter.swap_state(self);
        adapter.body_ids = body_ids;
        std::mem::swap(&mut adapter.records, records);
        let mut scope = RetainedNotificationScope {
            retained: self,
            caller: records,
            adapter,
        };
        action(&mut scope.adapter)
    }
}

impl<'keys> ManifoldKernelAdapter<'keys> {
    fn swap_state(&mut self, other: &mut ManifoldKernelAdapter<'_>) {
        std::mem::swap(&mut self.merge, &mut other.merge);
        std::mem::swap(&mut self.topology_deleted, &mut other.topology_deleted);
        std::mem::swap(&mut self.intersector, &mut other.intersector);
        std::mem::swap(&mut self.curves, &mut other.curves);
        std::mem::swap(&mut self.extra_curves, &mut other.extra_curves);
        std::mem::swap(&mut self.source_handles, &mut other.source_handles);
        std::mem::swap(&mut self.subset, &mut other.subset);
        std::mem::swap(&mut self.body_ids, &mut other.body_ids);
    }

    fn from_keys(keys: NotificationKeys<'keys>) -> Self {
        Self {
            merge: MergeState::new(Relations::default()),
            records: crate::topology::topology::NotifyRecords::new(),
            topology_deleted: false,
            keys,
            body_ids: match keys {
                NotificationKeys::Namespace(n) => {
                    (BrepId::from_index(n * 2), BrepId::from_index(n * 2 + 1))
                }
                NotificationKeys::Caller(_) => (BrepId::from_index(0), BrepId::from_index(1)),
            },
            source_handles: BTreeMap::new(),
            subset: None,
            intersector: TopologyIntersector::new(
                Some(arena_pair::brep_h(0)),
                Some(arena_pair::brep_h(1)),
                0.0,
                0.0,
            ),
            curves: Vec::new(),
            extra_curves: Vec::new(),
        }
    }

    /// Configure the intersector tolerances using the canonical merge constructor.
    pub fn configure_operands(
        &mut self,
        kernel: &Kernel,
        primary: &Brep,
        other: &Brep,
        distance: f64,
        angle_radians: f64,
    ) -> Result<()> {
        let configured = MergeState::<TopologyIntersector<MergeOperand>>::with_breps(
            kernel,
            (primary, MergeOperand::Primary),
            Some((other, MergeOperand::Other)),
            distance,
            angle_radians,
            None,
        )?;
        self.set_intersection_tolerances(
            configured.intersector.this_approx_tol_3d(),
            configured.intersector.this_ang_tol_rad(),
        );
        Ok(())
    }

    /// Clear the two canonical mate maps between piecewise stages
    /// (`m_pBToO->RemoveAll(); m_pOToB->RemoveAll()`).
    pub fn clear_relationships(&mut self) -> Result<()> {
        self.merge.intersector.with_intersector(
            &mut self.intersector,
            arena_pair::encode,
            arena_pair::decode,
            TopologyIntersector::clear_relationships,
        )
    }

    /// `m_vTI.m_bIntersectWireEdges`.
    pub fn set_intersect_wire_edges(&mut self, enabled: bool) {
        self.intersector.intersect_wire_edges = enabled;
    }

    /// `m_vTI.m_bIgnoreTangency`.
    pub fn set_ignore_tangency(&mut self, enabled: bool) {
        self.intersector.ignore_tangency = enabled;
    }

    /// `m_vTI.m_bDoingFillets`.
    pub fn set_doing_fillets(&mut self, enabled: bool) {
        self.intersector.doing_fillets = enabled;
    }

    /// The empty-array `RegisterExistingSSI` call used by `PiecewiseMerge`.
    pub fn register_empty_ssi(&mut self, primary: FaceId, other: FaceId) {
        self.intersector.register_empty_ssi(
            Some(arena_pair::surface_handle(false, primary)),
            Some(arena_pair::surface_handle(true, other)),
        );
    }

    /// Clear the piecewise stage's registered surface pairs.
    pub fn clear_existing_ssi(&mut self) {
        let _ = self.intersector.clear_existing_ssi();
    }

    fn with_topology_copy<T>(
        &mut self,
        attributes: &mut AttributeStore,
        action: impl FnOnce(&mut Relations, &mut TopologyCopy<'_>) -> Result<T>,
    ) -> Result<T> {
        let keys = self.keys;
        let mut copy = TopologyCopy {
            vertices: p1::VertexCopy {
                records: &mut self.records,
                attributes,
                source_key: &|v| keys.object(true, EntityRef::Vertex(v)),
                destination_key: &|e| keys.object(false, e),
            },
            edge_source_key: &|e| keys.object(true, EntityRef::Edge(e)),
            edge_destination_key: &|e| keys.object(false, EntityRef::Edge(e)),
            face_source_key: &|f| keys.object(true, EntityRef::Face(f)),
            face_destination_key: &|f| keys.object(false, EntityRef::Face(f)),
        };
        action(&mut self.merge.intersector, &mut copy)
    }

    /// Copy through the retained merge rather than constructing a disposable mate store.
    pub(crate) fn copy_faces(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        faces: &[FaceId],
        attributes: &mut AttributeStore,
    ) -> Result<()> {
        self.with_topology_copy(attributes, |relations, copy| {
            let context = match kernels.other.as_deref_mut() {
                Some(other) => MergeContext::Split(kernels.primary, Some(&mut other.context)),
                None => MergeContext::Shared(kernels.primary),
            };
            crate::topology::brep::p7::copy_faces_with_merge(
                context, other, faces, primary, relations, copy,
            )
            .map(|_| ())
        })
    }

    /// The distance and angular tolerances held by the production intersector.
    pub fn intersection_tolerances(&self) -> (f64, f64) {
        (
            self.intersector.this_approx_tol_3d(),
            self.intersector.this_ang_tol_rad(),
        )
    }

    /// `SmTopologyIntersector(ctx, pBrep, pBrep, SM_EFF_ZERO, SM_EFF_ZERO).DeleteFaces(faces)`
    /// over one kernel B-rep, as the offset executive's lamina clean-up uses it
    /// (`SmOffsetExecutive.cpp:3749-3752`): a fresh intersector whose two operands
    /// are the same B-rep deletes `faces` through this crate's arena access.
    /// Notifications of the deleted topology go to `records` and `attributes` under
    /// `namespace` (26 bits, distinct from any other pair sharing the store).
    ///
    /// # Errors
    ///
    /// Those of [`TopologyIntersector::delete_faces`] and an over-wide namespace (26 bits,
    /// as [`KernelManifoldIntersector::new`]'s); `InvalidInput` when the kernel
    /// context's callbacks are already lent (`Context::with_notification_context`).
    pub fn delete_faces_in(
        kernel: &mut Kernel,
        brep: &mut Brep,
        faces: &[FaceId],
        records: &mut crate::topology::topology::NotifyRecords,
        attributes: &mut AttributeStore,
        namespace: u32,
    ) -> Result<()> {
        if namespace > 0x03ff_ffff {
            return Err(SolidError::invalid_input(
                OP,
                "object namespace exceeds 26 bits",
            ));
        }
        Self::delete_faces_for_keys(
            kernel,
            brep,
            faces,
            records,
            attributes,
            NotificationKeys::Namespace(namespace),
        )
    }

    /// Delete faces using the caller's actual topology identities and stores.
    /// This operation does not construct or split curves or propagate regions.
    pub fn delete_faces_with_keys(
        kernel: &mut Kernel,
        brep: &mut Brep,
        faces: &[FaceId],
        records: &mut crate::topology::topology::NotifyRecords,
        attributes: &mut AttributeStore,
        key_of: &dyn Fn(EntityRef) -> ObjectRef,
    ) -> Result<()> {
        let no_curve = |_, _| {
            Err(SolidError::invalid_input(
                OP,
                "face deletion cannot allocate curves",
            ))
        };
        let no_sources = |_| {
            Err(SolidError::invalid_input(
                OP,
                "face deletion cannot propagate regions",
            ))
        };
        Self::delete_faces_for_keys(
            kernel,
            brep,
            faces,
            records,
            attributes,
            NotificationKeys::Caller(ManifoldNotificationKeys {
                primary: key_of,
                other: key_of,
                primary_curve: &no_curve,
                other_curve: &no_curve,
                source_lists: &no_sources,
                actual_curve_identities: false,
            }),
        )
    }

    fn delete_faces_for_keys(
        kernel: &mut Kernel,
        brep: &mut Brep,
        faces: &[FaceId],
        records: &mut crate::topology::topology::NotifyRecords,
        attributes: &mut AttributeStore,
        keys: NotificationKeys<'_>,
    ) -> Result<()> {
        let mut intersector: TopologyIntersector<H> = TopologyIntersector::new(
            Some(arena_pair::brep_h(0)),
            Some(arena_pair::brep_h(0)),
            crate::math::EFF_ZERO,
            crate::math::EFF_ZERO,
        );
        let handles: Vec<H> = faces
            .iter()
            .map(|&face| arena_pair::encode(EntityRef::Face(face), false))
            .collect();
        let mut curves = Vec::new();
        let mut extra_curves = Vec::new();
        Context::with_notification_context(kernel, |k, context| {
            let mut pair = ArenaPair::new(
                brep,
                None,
                &mut curves,
                &mut extra_curves,
                records,
                attributes,
                cache_kernels(k, None, context),
                context,
                None,
                keys,
            );
            intersector.delete_faces(k, &mut pair, &handles, None)
        })?
    }

    /// Set the same explicit intersection tolerances supplied to `SmMerge`.
    /// Leaving them at zero retains the context-only intersector defaults;
    /// automatic tolerance selection belongs to the merge constructor's owner.
    pub fn set_intersection_tolerances(&mut self, distance: f64, angle_radians: f64) {
        self.intersector.set_this_approx_tol_3d(distance);
        self.intersector.set_this_ang_tol_rad(angle_radians);
    }

    /// Set `m_vTI.m_lIntersectLaminaEdges` (0 none, 1 both, 2 only the B-rep's,
    /// 3 only the other's), as `NonManifoldBoolean` and `PiecewiseMerge` do.
    pub fn set_intersect_lamina_edges(&mut self, mode: u64) {
        self.intersector.intersect_lamina_edges = mode;
    }

    /// `m_vTI.m_lIntersectLaminaEdges`.
    #[must_use]
    pub fn intersect_lamina_edges(&self) -> u64 {
        self.intersector.intersect_lamina_edges
    }

    /// `m_vTI.m_bIntersectWireEdges`.
    #[must_use]
    pub fn intersect_wire_edges(&self) -> bool {
        self.intersector.intersect_wire_edges
    }

    /// The partial-merge surface subset ([`Self::set_surface_subset`]), if any.
    #[must_use]
    pub fn surface_subset(&self) -> Option<&BTreeSet<ObjectRef>> {
        self.subset.as_ref()
    }

    /// Run the orchestration with the options held in the canonical `MergeState`.
    pub fn manifold_boolean(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Option<Brep>,
        other: &mut Option<Brep>,
        state: &mut ManifoldState,
        attributes: &mut AttributeStore,
        manifold: bool,
        operation: BooleanOperation,
        lock: Option<&mut MarkLock>,
    ) -> Result<bool> {
        if let Some(body) = primary.as_ref() {
            crate::topology::brep::p3::seed_records(&mut self.records, body, &|e| {
                self.keys.object(false, e)
            });
        }
        if let Some(body) = other.as_ref() {
            crate::topology::brep::p3::seed_records(&mut self.records, body, &|e| {
                self.keys.object(true, e)
            });
        }
        let options = self.merge.options;
        manifold_boolean(
            kernels, primary, other, self, state, attributes, options, manifold, operation, lock,
        )
    }

    /// The geometry identity used by the canonical B-rep cache, including shared
    /// owner identities. This deliberately does not compare surface values.
    pub fn surface_identity(other: &Brep, face: FaceId) -> Result<ObjectRef> {
        let surface = other
            .face(face)?
            .surface
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        Ok(surface
            .owner()
            .unwrap_or_else(|| arena_identity(CacheObject::Face(face))))
    }

    /// Replace the partial-merge surface subset; an empty input clears it, as
    /// `SetSubset`. Obtain handles with [`Self::surface_identity`].
    pub fn set_surface_subset(&mut self, surfaces: &[ObjectRef]) {
        set_subset_objects(&mut self.subset, surfaces);
    }

    /// Resolve a propagation callback's source-list handle to the current merge
    /// attribute's actual list. No list contents or region flags are duplicated.
    pub fn source_regions<'a>(
        &self,
        attributes: &'a AttributeStore,
        handle: ObjectRef,
    ) -> Result<&'a [ObjectKey]> {
        let (owner, other_list) = if let Some(&source) = self.source_handles.get(&handle) {
            source
        } else if let NotificationKeys::Namespace(namespace) = self.keys {
            let kind = (handle.key.0 >> 32) & 15;
            if handle.type_code != 0
                || handle.key.0 >> 37 != u64::from(namespace)
                || (handle.key.0 >> 36) & 1 != 0
                || !(kind == 5 || kind == 6)
            {
                return Err(SolidError::invalid_input(OP, "not a source-list handle"));
            }
            let region = RegionId::from_index(handle.key.0 as u32);
            (
                self.object_key(false, MergeEntity::Region(region)),
                kind == 6,
            )
        } else {
            return Err(SolidError::invalid_input(OP, "not a source-list handle"));
        };
        let data = attributes
            .find_attribute(owner, ids::OTHER_REGION_ID)
            .and_then(|h| attributes.get(h))
            .and_then(|a| a.merge_region_data())
            .ok_or_else(|| {
                SolidError::invalid_input(OP, "source-list attribute no longer exists")
            })?;
        Ok(if other_list {
            &data.other_regions
        } else {
            &data.this_regions
        })
    }
}

struct Regions<'a> {
    body_ids: (BrepId, BrepId),
    keys: NotificationKeys<'a>,
    primary: &'a Brep,
    other: &'a Brep,
}

impl RegionLookup for Regions<'_> {
    fn resolve_region(&self, object: ObjectKey) -> Result<RegionRef<'_>> {
        if let NotificationKeys::Caller(keys) = self.keys {
            for (brep, key_of, ordinal) in
                [(self.primary, keys.primary, 0), (self.other, keys.other, 1)]
            {
                for (region_id, _) in brep.regions().iter() {
                    if key_of(EntityRef::Region(region_id)).key == object {
                        return Ok(RegionRef {
                            brep,
                            brep_id: if ordinal == 0 {
                                self.body_ids.0
                            } else {
                                self.body_ids.1
                            },
                            region_id,
                        });
                    }
                }
            }
            return Err(SolidError::invalid_input(
                OP,
                "not an operand region identity",
            ));
        }
        let NotificationKeys::Namespace(namespace) = self.keys else {
            return Err(SolidError::invalid_input(
                OP,
                "not an operand region identity",
            ));
        };
        if object.0 >> 37 != u64::from(namespace) || (object.0 >> 32) & 15 != 1 {
            return Err(SolidError::invalid_input(
                OP,
                "not an operand region identity",
            ));
        }
        let other = (object.0 >> 36) & 1 != 0;
        let brep = if other { self.other } else { self.primary };
        let region_id = RegionId::from_index(object.0 as u32);
        brep.region(region_id)?;
        Ok(RegionRef {
            brep,
            brep_id: BrepId::from_index(namespace * 2 + u32::from(other)),
            region_id,
        })
    }
}

impl ManifoldIntersector for ManifoldKernelAdapter<'_> {
    fn transform_brep(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        brep: &mut Brep,
        other: bool,
        attributes: &mut AttributeStore,
        placement: &Axis2Placement,
        scale: Option<Vec3>,
        scale_tolerance: bool,
    ) -> Result<()> {
        let keys = self.keys;
        let key_of = |e| keys.object(other, e);
        let k = if other {
            kernels.other.as_deref_mut().unwrap_or(kernels.primary)
        } else {
            &mut *kernels.primary
        };
        let scope = GeometryOwnerScope::new(brep, keys, other)?;
        Context::with_notification_context(k, |k, context| {
            let mut env = crate::topology::edge::NotifyEnv {
                records: &mut self.records,
                attributes,
                context: Some(context),
                key_of: &key_of,
            };
            crate::topology::brep::p5::transform_with_notifications(
                k,
                scope.body,
                placement,
                scale,
                scale_tolerance,
                Some(&mut env),
            )
        })?
    }

    fn intersect_insert_relate(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
    ) -> Result<()> {
        let mut subset = Vec::new();
        if let Some(selected) = &self.subset {
            for f in bp::faces(other, None)? {
                if selected.contains(&Self::surface_identity(other, f)?) {
                    subset.push(arena_pair::surface_handle(true, f));
                }
            }
            if subset.is_empty() {
                subset.push(arena_pair::unmatched_surface());
            }
        }
        self.intersector.set_subset(&subset);
        self.intersector.topology_deleted = self.topology_deleted;
        Context::with_notification_context(kernels.primary, |k, context| {
            let mut pair = ArenaPair::new(
                primary,
                Some(other),
                &mut self.curves,
                &mut self.extra_curves,
                &mut self.records,
                attributes,
                cache_kernels(k, kernels.other.as_deref(), context),
                context,
                kernels.other.as_deref_mut().map(|k| &mut k.context),
                self.keys,
            );
            let result = self.merge.intersector.with_intersector(
                &mut self.intersector,
                arena_pair::encode,
                arena_pair::decode,
                |ti| ti.intersect_insert_relate(k, &mut pair, arena_pair::TI_HANDLE),
            );
            self.topology_deleted = self.intersector.topology_deleted;
            // Deleted curve handles no longer appear in manager/table state. Storage
            // remains stable so surviving handles keep their identities.
            result.map(|_| ())
        })?
    }

    fn remove_relationship(&mut self, face: FaceId, other_face: FaceId) -> Result<()> {
        self.merge.intersector.with_intersector(
            &mut self.intersector,
            arena_pair::encode,
            arena_pair::decode,
            |ti| {
                ti.remove_relationship(
                    Some(arena_pair::encode(EntityRef::Face(face), false)),
                    Some(arena_pair::encode(EntityRef::Face(other_face), true)),
                )
            },
        )
    }

    fn close_intersection_loop(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
        vertex: VertexId,
        made_new_topology: &mut bool,
        deleted: &mut (Vec<VertexId>, Vec<EdgeId>),
    ) -> Result<()> {
        let vertex = arena_pair::encode(EntityRef::Vertex(vertex), false);
        // IIREdgeBrep's MergeCurveClasses sets m_bTopologyDeleted on this same flag.
        self.intersector.topology_deleted = self.topology_deleted;
        let (mut gone_vertices, mut gone_edges) = (Vec::new(), Vec::new());
        let result = Context::with_notification_context(kernels.primary, |k, context| {
            let mut pair = ArenaPair::new(
                primary,
                Some(other),
                &mut self.curves,
                &mut self.extra_curves,
                &mut self.records,
                attributes,
                cache_kernels(k, kernels.other.as_deref(), context),
                context,
                kernels.other.as_deref_mut().map(|k| &mut k.context),
                self.keys,
            );
            let result = self.merge.intersector.with_intersector(
                &mut self.intersector,
                arena_pair::encode,
                arena_pair::decode,
                |ti| {
                    ti.close_intersection_loop(
                        k,
                        &mut pair,
                        Some(vertex),
                        &mut gone_vertices,
                        &mut gone_edges,
                    )
                },
            );
            self.topology_deleted = self.intersector.topology_deleted;
            *made_new_topology = result?;
            Ok(())
        });
        // SmTArray::RemoveElements compares pointers: the other B-rep's entities match
        // nothing in the primary's lists, so only primary ones are reported.
        deleted.0.extend(
            gone_vertices
                .into_iter()
                .filter_map(|h| arena_pair::deleted_vertex(h).ok()),
        );
        deleted.1.extend(
            gone_edges
                .into_iter()
                .filter_map(|h| arena_pair::deleted_edge(h).ok()),
        );
        result?
    }

    fn fix_gaps(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
        single_vertices: &[VertexId],
    ) -> Result<FixedGaps> {
        let vertices: Vec<_> = single_vertices
            .iter()
            .map(|&v| arena_pair::encode(EntityRef::Vertex(v), false))
            .collect();
        Context::with_notification_context(kernels.primary, |k, context| {
            let mut pair = ArenaPair::new(
                primary,
                Some(other),
                &mut self.curves,
                &mut self.extra_curves,
                &mut self.records,
                attributes,
                cache_kernels(k, kernels.other.as_deref(), context),
                context,
                kernels.other.as_deref_mut().map(|k| &mut k.context),
                self.keys,
            );
            self.merge
                .intersector
                .with_intersector(
                    &mut self.intersector,
                    arena_pair::encode,
                    arena_pair::decode,
                    |ti| ti.fix_gaps(k, &mut pair, &vertices),
                )
                .and_then(|out| {
                    Ok(FixedGaps {
                        modified_topology: out.modified_topology,
                        deleted_vertices: out
                            .deleted_vertices
                            .into_iter()
                            .map(arena_pair::deleted_vertex)
                            .collect::<Result<_>>()?,
                        deleted_edges: out
                            .deleted_edges
                            .into_iter()
                            .flatten()
                            .map(arena_pair::deleted_edge)
                            .collect::<Result<_>>()?,
                    })
                })
        })?
    }

    fn delete_faces(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        attributes: &mut AttributeStore,
        faces: &[FaceId],
        keep_wires: Option<&[EdgeId]>,
    ) -> Result<()> {
        let faces: Vec<_> = faces
            .iter()
            .map(|&f| arena_pair::encode(EntityRef::Face(f), false))
            .collect();
        let keep = keep_wires.map(|edges| {
            edges
                .iter()
                .map(|&e| arena_pair::encode(EntityRef::Edge(e), false))
                .collect::<Vec<_>>()
        });
        Context::with_notification_context(kernels.primary, |k, context| {
            let mut pair = ArenaPair::new(
                primary,
                None,
                &mut self.curves,
                &mut self.extra_curves,
                &mut self.records,
                attributes,
                cache_kernels(k, kernels.other.as_deref(), context),
                context,
                None,
                self.keys,
            );
            self.merge.intersector.with_intersector(
                &mut self.intersector,
                arena_pair::encode,
                arena_pair::decode,
                |ti| ti.delete_faces(k, &mut pair, &faces, keep.as_deref()),
            )
        })?
    }

    fn copy_component_faces(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        source: &mut Brep,
        other: bool,
        faces: &[FaceId],
        destination: &mut Brep,
        attributes: &mut AttributeStore,
    ) -> Result<()> {
        let keys = self.keys;
        let k = if other {
            kernels.other.as_deref_mut().unwrap_or(kernels.primary)
        } else {
            &mut *kernels.primary
        };
        Context::with_notification_context(k, |k, context| {
            // A newly extracted component is a distinct native B-rep, including
            // when all its arena indices coincide with the source's indices.
            let serial = crate::topology::brep::p3::notify_serial(context, destination)?;
            let destination_key = |entity| crate::topology::brep::p3::topology_ref(serial, entity);
            let source_key = |entity| keys.object(other, entity);
            destination.set_key(destination_key(EntityRef::Brep(BrepId::from_index(0))).key);
            crate::topology::brep::p3::seed_records(&mut self.records, source, &source_key);
            crate::topology::brep::p3::seed_records(
                &mut self.records,
                destination,
                &destination_key,
            );
            let mut copy = TopologyCopy {
                vertices: p1::VertexCopy {
                    records: &mut self.records,
                    attributes,
                    source_key: &|v| source_key(EntityRef::Vertex(v)),
                    destination_key: &destination_key,
                },
                edge_source_key: &|e| source_key(EntityRef::Edge(e)),
                edge_destination_key: &|e| destination_key(EntityRef::Edge(e)),
                face_source_key: &|f| source_key(EntityRef::Face(f)),
                face_destination_key: &|f| destination_key(EntityRef::Face(f)),
            };
            crate::topology::brep::p7::copy_faces_with_merge(
                MergeContext::Lent(k, context),
                source,
                faces,
                destination,
                &mut Relations::default(),
                &mut copy,
            )
            .map(|_| ())
        })?
    }

    fn mapped_edges(&self) -> Result<Vec<EdgeId>> {
        Ok(self.merge.intersector.edges.mapped_primary())
    }

    fn mapped_vertices(&self) -> Result<Vec<VertexId>> {
        Ok(self.merge.intersector.vertices.mapped_primary())
    }

    fn common_edges(&self, primary: &Brep, _other: &Brep) -> Result<(Vec<EdgeId>, Vec<EdgeId>)> {
        Ok(self
            .merge
            .intersector
            .edges
            .common(bp::edges(primary, None)?))
    }

    fn common_vertices(
        &self,
        primary: &Brep,
        _other: &Brep,
    ) -> Result<(Vec<VertexId>, Vec<VertexId>)> {
        Ok(self
            .merge
            .intersector
            .vertices
            .common(bp::vertices(primary, None)?))
    }

    fn edge_mate(&self, edge: EdgeId) -> Option<EdgeId> {
        self.merge.intersector.edges.other(edge)
    }

    fn face_mate(&self, face: FaceId) -> Option<FaceId> {
        self.merge.intersector.faces.other(face)
    }

    fn relate_faces(
        &mut self,
        _primary: &mut Brep,
        face: FaceId,
        _other: &mut Brep,
        other_face: FaceId,
    ) -> Result<()> {
        // The face branch of Relate only records the pair; tolerance updates are
        // restricted to vertices/edges. MateMap::record performs the same checks.
        self.merge.intersector.faces.record(face, other_face)
    }

    fn fix_face_coincidence(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
        face: FaceId,
        other_face: FaceId,
    ) -> Result<(Vec<FaceId>, Vec<FaceId>, bool)> {
        let mates = &self.merge.intersector.edges;
        let check = p1::check_face_coincidence(
            primary,
            other,
            face,
            other_face,
            mates.primary_to_other(),
            mates.other_to_primary(),
        )?;
        let p1::FaceCoincidenceCheck::NeedsMerge { edge_counts } = check else {
            return Ok((vec![face], vec![other_face], false));
        };
        // `m_vTI.MergeCoincidentSurfaces(pBrepFace->GetSurface(), pOtherFace->GetSurface(),
        // rBrepFaces, rOtherFaces)` (a failing status is returned as is)
        let (surface, other_surface) = (
            arena_pair::surface_handle(false, face),
            arena_pair::surface_handle(true, other_face),
        );
        self.intersector.topology_deleted = self.topology_deleted;
        let merged = Context::with_notification_context(kernels.primary, |k, context| {
            let mut pair = ArenaPair::new(
                primary,
                Some(other),
                &mut self.curves,
                &mut self.extra_curves,
                &mut self.records,
                attributes,
                cache_kernels(k, kernels.other.as_deref(), context),
                context,
                kernels.other.as_deref_mut().map(|k| &mut k.context),
                self.keys,
            );
            let result = self.merge.intersector.with_intersector(
                &mut self.intersector,
                arena_pair::encode,
                arena_pair::decode,
                |ti| {
                    ti.merge_coincident_surfaces(
                        k,
                        &mut pair,
                        surface,
                        other_surface,
                        Some(arena_pair::TI_HANDLE),
                    )
                },
            );
            self.topology_deleted = self.intersector.topology_deleted;
            // a curve identity the caller's keys refused during an edit (no rollback)
            match pair.take_identity_failure() {
                Some(refused) => Err(refused),
                None => result,
            }
        })??;
        // (the temporary curves stay in the adapter's stores: handles are stable)
        let faces = merged
            .faces
            .into_iter()
            .map(|h| arena_pair::merged_face(h, false))
            .collect::<Result<Vec<_>>>()?;
        let other_faces = merged
            .other_faces
            .into_iter()
            .map(|h| arena_pair::merged_face(h, true))
            .collect::<Result<Vec<_>>>()?;
        let changed = p1::finish_face_coincidence(primary, other, edge_counts)?;
        Ok((faces, other_faces, changed))
    }

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
    ) -> Result<()> {
        let keys = self.keys;
        let source_key = |id| keys.object(true, EntityRef::Vertex(id));
        let destination_key = |entity| keys.object(false, entity);
        let edge_source = |edge| keys.object(true, EntityRef::Edge(edge));
        let edge_destination = |edge| keys.object(false, EntityRef::Edge(edge));
        let face_source = |face| keys.object(true, EntityRef::Face(face));
        let face_destination = |face| keys.object(false, EntityRef::Face(face));
        let mut copy = TopologyCopy {
            vertices: p1::VertexCopy {
                records: &mut self.records,
                attributes,
                source_key: &source_key,
                destination_key: &destination_key,
            },
            edge_source_key: &edge_source,
            edge_destination_key: &edge_destination,
            face_source_key: &face_source,
            face_destination_key: &face_destination,
        };
        p5::process_wires_and_shell_vertices_with_kernels(
            kernels.primary,
            kernels.other.as_deref_mut(),
            primary,
            other,
            &mut state.primary_marks,
            &mut state.other_marks,
            &mut self.merge.intersector,
            &mut copy,
            operation,
            primary_mark,
            other_mark,
            keep,
        )
    }

    fn make_faceuse(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        other: &mut Brep,
        attributes: &mut AttributeStore,
        faceuse: FaceuseId,
        original_region: Option<RegionId>,
    ) -> Result<MadeFaceuse> {
        let keys = self.keys;
        let vertex_source = |id| keys.object(true, EntityRef::Vertex(id));
        let destination = |entity| keys.object(false, entity);
        let edge_source = |edge| keys.object(true, EntityRef::Edge(edge));
        let edge_destination = |edge| keys.object(false, EntityRef::Edge(edge));
        let face_source = |face| keys.object(true, EntityRef::Face(face));
        let face_destination = |face| keys.object(false, EntityRef::Face(face));
        let mut copy = TopologyCopy {
            vertices: p1::VertexCopy {
                records: &mut self.records,
                attributes,
                source_key: &vertex_source,
                destination_key: &destination,
            },
            edge_source_key: &edge_source,
            edge_destination_key: &edge_destination,
            face_source_key: &face_source,
            face_destination_key: &face_destination,
        };
        // New topology is made with the primary context; copies notify through the
        // other operand's (or the shared) context.
        let ctx = match kernels.other.as_deref_mut() {
            Some(o) => MergeContext::Split(kernels.primary, Some(&mut o.context)),
            None => MergeContext::Shared(kernels.primary),
        };
        let mut faces = Vec::new();
        let made = p2::make_faceuse_with(
            ctx,
            primary,
            other,
            faceuse,
            original_region,
            &mut self.merge.intersector,
            &mut copy,
            Some(&mut faces),
        )?;
        Ok(MadeFaceuse {
            old_region: made.old_region,
            new_region: made.new_region,
            faces,
        })
    }

    fn find_region_of_other_face(
        &mut self,
        k: &Kernel,
        primary: &mut Brep,
        other: &mut Brep,
        faceuse: FaceuseId,
    ) -> Result<Option<RegionId>> {
        p1::find_brep_region_of_other_face(
            k,
            primary,
            other,
            faceuse,
            self.merge.intersector.edges.other_to_primary(),
        )
    }

    fn propagate_regions(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &Brep,
        other: &Brep,
        attributes: &mut AttributeStore,
        operation: BooleanOperation,
    ) -> Result<()> {
        let mut regions = Vec::new();
        for id in bp::regions(primary, None)? {
            let source_lists = match self.keys {
                NotificationKeys::Namespace(namespace) => [5, 6].map(|kind| ObjectRef {
                    key: key(namespace, false, kind, id.index()),
                    type_code: 0,
                }),
                NotificationKeys::Caller(keys) => (keys.source_lists)(id)?,
            };
            let owner = self.object_key(false, MergeEntity::Region(id));
            for (handle, other_list) in source_lists.into_iter().zip([false, true]) {
                if handle.type_code != 0 || self.records.contains_key(&handle.key) {
                    return Err(SolidError::invalid_input(
                        OP,
                        "source-list handle aliases an object",
                    ));
                }
                if self
                    .source_handles
                    .get(&handle)
                    .is_some_and(|value| *value != (owner, other_list))
                {
                    return Err(SolidError::invalid_input(
                        OP,
                        "source-list handles are not distinct",
                    ));
                }
                self.source_handles.insert(handle, (owner, other_list));
            }
            regions.push(p1::RegionPropagation {
                region: ObjectRef {
                    key: self.object_key(false, MergeEntity::Region(id)),
                    type_code: REGION_TYPE,
                },
                is_void: primary.region(id)?.is_void() == 1,
                source_lists,
            });
        }
        let lookup = Regions {
            body_ids: self.body_ids,
            keys: self.keys,
            primary,
            other,
        };
        p1::propagate_other_src_region_attribs(
            &regions,
            attributes,
            Some(&mut kernels.primary.context),
            operation,
            Some(&lookup),
        )
    }

    fn post_process(
        &mut self,
        kernels: &mut BooleanKernels<'_>,
        primary: &mut Brep,
        attributes: &mut AttributeStore,
        state: &ManifoldState,
        check_all_edges: bool,
    ) -> Result<()> {
        // Move, rather than clone, the canonical relationships through the existing
        // cleanup API, restoring them on both success and failure.
        let cleanup = p1::CleanupRelations {
            edges: state.intersection_edges.clone(),
            vertices: state.intersection_vertices.clone(),
            mates: std::mem::take(&mut self.merge.intersector),
            ..p1::CleanupRelations::default()
        };
        // The cleanup's deletions notify on the merge's records, attributes and names, as
        // every other edit of the merge (upstream: the objects' own attributes).
        let keys = self.keys;
        let key_of = move |e: EntityRef| keys.object(false, e);
        let records = &mut self.records;
        let result = Context::with_notification_context(kernels.primary, |k, context| {
            let mut env = crate::topology::edge::NotifyEnv {
                records,
                attributes,
                context: Some(context),
                key_of: &key_of,
            };
            p1::boolean_post_process_with_notifications(
                k,
                primary,
                &cleanup,
                check_all_edges,
                Some(&mut env),
            )
        })
        .and_then(|r| r);
        self.merge.intersector = cleanup.mates;
        result
    }

    fn object_key(&self, other: bool, entity: MergeEntity) -> ObjectKey {
        let entity = match entity {
            MergeEntity::Region(id) => EntityRef::Region(id),
            MergeEntity::Face(id) => EntityRef::Face(id),
            MergeEntity::Edge(id) => EntityRef::Edge(id),
            MergeEntity::Vertex(id) => EntityRef::Vertex(id),
        };
        self.keys.object(other, entity).key
    }

    fn excluded_surface(&self, other: &Brep, face: FaceId) -> Result<bool> {
        match &self.subset {
            Some(subset) => Ok(!subset.contains(&Self::surface_identity(other, face)?)),
            None => Ok(false),
        }
    }

    fn clear_topology_deleted(&mut self) {
        self.topology_deleted = false;
        self.intersector.topology_deleted = false;
    }

    fn topology_deleted(&self) -> bool {
        self.topology_deleted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::{Attribute, Behavior};
    use crate::geom::context::NotifyCallback;
    use crate::geom::object::NotifyOperation;
    use crate::math::Vec3;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::topology::topology_traverser::Entity;
    use crate::topology::{
        Edge, Edgeuse, EdgeuseId, EdgeuseOwner, Faceuse, Loopuse, LoopuseStart, Region, Vertexuse,
        VertexuseId, VertexuseOwner,
    };
    use std::sync::{Arc, Mutex};

    fn face(b: &mut Brep) -> FaceId {
        let shell = b.add_shell().unwrap();
        b.add_face(shell).unwrap()
    }

    #[test]
    fn close_intersection_loop_runs_the_co_ported_intersector() {
        let mut adapter = KernelManifoldIntersector::new(0).unwrap();
        let mut k = Kernel::default();
        let mut kernels = BooleanKernels {
            primary: &mut k,
            other: None,
        };
        let mut a = Brep::new();
        let v = a
            .add_vertex_record(crate::topology::Vertex::default())
            .unwrap();
        let mut b = Brep::new();
        let mut made = true;
        let mut deleted = (Vec::new(), Vec::new());
        // An unmated vertex is p2's NER early return; the flag is left alone and
        // nothing is deleted.
        let error = adapter
            .close_intersection_loop(
                &mut kernels,
                &mut a,
                &mut b,
                &mut AttributeStore::new(),
                v,
                &mut made,
                &mut deleted,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            }
        ));
        assert!(made);
        assert!(deleted.0.is_empty() && deleted.1.is_empty());
        assert!(!adapter.topology_deleted());
    }

    #[test]
    fn canonical_relationships_drive_lists_and_face_helpers() {
        let mut adapter = KernelManifoldIntersector::new(7).unwrap();
        let mut a = Brep::new();
        let mut b = Brep::new();
        let v0 = a.add_vertex(Vec3::X).unwrap();
        let v1 = a.add_vertex(Vec3::Y).unwrap();
        let e0 = a.add_edge(v0, v1).unwrap();
        let e1 = a.add_edge(v1, v0).unwrap();
        adapter
            .merge
            .intersector
            .vertices
            .record(v1, VertexId::from_index(0))
            .unwrap();
        adapter
            .merge
            .intersector
            .vertices
            .record(v0, VertexId::from_index(1))
            .unwrap();
        adapter
            .merge
            .intersector
            .edges
            .record(e1, EdgeId::from_index(0))
            .unwrap();
        adapter
            .merge
            .intersector
            .edges
            .record(e0, EdgeId::from_index(1))
            .unwrap();
        assert_eq!(adapter.mapped_edges().unwrap(), [e1, e0]);
        assert_eq!(adapter.mapped_vertices().unwrap(), [v1, v0]);
        assert_eq!(
            adapter.common_edges(&a, &b).unwrap(),
            (
                vec![e0, e1],
                vec![EdgeId::from_index(1), EdgeId::from_index(0)]
            )
        );
        assert_eq!(adapter.common_vertices(&a, &b).unwrap().0, [v0, v1]);
        assert_eq!(adapter.edge_mate(e0), Some(EdgeId::from_index(1)));
        let f = face(&mut a);
        let of = face(&mut b);
        adapter.relate_faces(&mut a, f, &mut b, of).unwrap();
        assert_eq!(adapter.face_mate(f), Some(of));
        assert!(adapter
            .relate_faces(&mut a, f, &mut b, FaceId::from_index(8))
            .is_err());
        assert_eq!(adapter.face_mate(f), Some(of));
        assert_eq!(
            adapter
                .fix_face_coincidence(
                    &mut BooleanKernels {
                        primary: &mut Kernel::default(),
                        other: None
                    },
                    &mut a,
                    &mut b,
                    &mut AttributeStore::new(),
                    f,
                    of
                )
                .unwrap(),
            (vec![f], vec![of], false)
        );
        let r = a
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        assert_eq!(
            adapter
                .find_region_of_other_face(
                    &Kernel::default(),
                    &mut a,
                    &mut b,
                    FaceuseId::from_index(u32::MAX)
                )
                .unwrap(),
            Some(r)
        );
    }

    // Use both ends of an edge as a closed boundary, so the face helper really
    // visits a non-empty loop. The degenerate face needs no surface evaluation.
    fn face_on_wire(b: &mut Brep, edge: EdgeId) -> (FaceId, FaceuseId) {
        let f = face(b);
        let fu = b
            .add_faceuse_record(Faceuse {
                face: Some(f),
                shell: Some(b.face(f).unwrap().shell),
                ..Faceuse::default()
            })
            .unwrap();
        b.face_mut(f).unwrap().faceuse = Some(fu);
        let lu = b
            .add_loopuse_record(Loopuse {
                faceuse: Some(fu),
                ..Loopuse::default()
            })
            .unwrap();
        b.faceuse_mut(fu).unwrap().loopuses.push(lu);
        let uses = b.edge(edge).unwrap().edgeuses.clone();
        for (&eu, &next) in uses.iter().zip(uses.iter().cycle().skip(1)) {
            let data = b.edgeuse_mut(eu).unwrap();
            data.owner = EdgeuseOwner::Loopuse(Some(lu));
            data.ccw = Some(next);
            data.cw = Some(next);
        }
        b.loopuse_mut(lu).unwrap().start = LoopuseStart::Edgeuse(uses.first().copied());
        (f, fu)
    }

    #[test]
    fn coincidence_uses_each_operand_edge_map_direction() {
        let mut adapter = KernelManifoldIntersector::new(0).unwrap();
        let mut a = Brep::new();
        let mut b = Brep::new();
        let edge = wire(&mut a);
        wire(&mut b); // Offset the other arena's boundary id: 0 -> 1.
        let other_edge = wire(&mut b);
        assert_ne!(edge, other_edge);
        adapter
            .merge
            .intersector
            .edges
            .record(edge, other_edge)
            .unwrap();
        let (f, _) = face_on_wire(&mut a, edge);
        let (of, _) = face_on_wire(&mut b, other_edge);
        assert_eq!(
            adapter
                .fix_face_coincidence(
                    &mut BooleanKernels {
                        primary: &mut Kernel::default(),
                        other: None
                    },
                    &mut a,
                    &mut b,
                    &mut AttributeStore::new(),
                    f,
                    of
                )
                .unwrap(),
            (vec![f], vec![of], false)
        );
    }

    /// Faces whose edges are not mated reach `MergeCoincidentSurfaces` itself (it was
    /// an `UNPORTED(boolean.topology_intersector_p3)` stub): these faces have no
    /// surfaces, which that merge refuses as upstream's unchecked dereference.
    #[test]
    fn unmated_faces_reach_the_coincident_surface_merge() {
        let mut adapter = KernelManifoldIntersector::new(0).unwrap();
        let mut a = Brep::new();
        let mut b = Brep::new();
        let edge = wire(&mut a);
        let other_edge = wire(&mut b);
        let (f, _) = face_on_wire(&mut a, edge);
        let (of, _) = face_on_wire(&mut b, other_edge);
        let err = adapter
            .fix_face_coincidence(
                &mut BooleanKernels {
                    primary: &mut Kernel::default(),
                    other: None,
                },
                &mut a,
                &mut b,
                &mut AttributeStore::new(),
                f,
                of,
            )
            .unwrap_err();
        assert!(!err.is_unported(), "{err:?}");
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn region_lookup_uses_other_to_primary_edge_mates() {
        let mut adapter = KernelManifoldIntersector::new(0).unwrap();
        let mut a = Brep::new();
        let mut b = Brep::new();
        let r = a
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        a.add_region_record(Region {
            in_brep: true,
            ..Region::default()
        })
        .unwrap();
        let edge = wire(&mut a);
        let eu = *a.edge(edge).unwrap().edgeuses.first().unwrap();
        let shell = crate::topology::edgeuse::p1::shell(&a, eu)
            .unwrap()
            .unwrap();
        a.shell_mut(shell).unwrap().region = Some(r);
        wire(&mut b); // Disjoint lookup keys between the two map directions.
        let other_edge = wire(&mut b);
        let (_, other_fu) = face_on_wire(&mut b, other_edge);
        adapter
            .merge
            .intersector
            .edges
            .record(edge, other_edge)
            .unwrap();
        assert_eq!(
            adapter
                .find_region_of_other_face(&Kernel::default(), &mut a, &mut b, other_fu)
                .unwrap(),
            Some(r)
        );
    }

    #[test]
    fn cleanup_restores_the_same_mates_on_both_result_exits() {
        let mut adapter = KernelManifoldIntersector::new(0).unwrap();
        let v = VertexId::from_index(u32::MAX);
        adapter.merge.intersector.vertices.record(v, v).unwrap();
        adapter
            .merge
            .intersector
            .faces
            .record(FaceId::from_index(0), FaceId::from_index(2))
            .unwrap();
        let mut primary = Brep::new();
        let mut k = Kernel::default();
        let mut kernels = BooleanKernels {
            primary: &mut k,
            other: None,
        };
        let mut state = ManifoldState::default();
        adapter
            .post_process(
                &mut kernels,
                &mut primary,
                &mut AttributeStore::default(),
                &state,
                false,
            )
            .unwrap();
        state.intersection_vertices.push(v);
        assert!(adapter
            .post_process(
                &mut kernels,
                &mut primary,
                &mut AttributeStore::default(),
                &state,
                false,
            )
            .is_err());
        assert_eq!(adapter.merge.intersector.vertices.other(v), Some(v));
        assert_eq!(
            adapter.face_mate(FaceId::from_index(0)),
            Some(FaceId::from_index(2))
        );
    }

    type Notice = (NotifyOperation, ObjectRef, [Option<ObjectRef>; 3]);
    #[derive(Debug)]
    struct Notices(Arc<Mutex<Vec<Notice>>>);
    impl NotifyCallback for Notices {
        fn execute(
            &mut self,
            op: NotifyOperation,
            caller: ObjectRef,
            d1: Option<ObjectRef>,
            d2: Option<ObjectRef>,
            d3: Option<ObjectRef>,
        ) -> Result<()> {
            self.0.lock().unwrap().push((op, caller, [d1, d2, d3]));
            Ok(())
        }
    }

    #[test]
    fn region_propagation_uses_live_regions_attributes_and_primary_callback() {
        let mut adapter = KernelManifoldIntersector::new(10).unwrap();
        let mut a = Brep::new();
        let mut b = Brep::new();
        let r = a
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        let o = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        a.region_mut(r).unwrap().set_is_void(0);
        b.region_mut(o).unwrap().set_is_void(0);
        let rk = adapter.object_key(false, MergeEntity::Region(r));
        let ok = adapter.object_key(true, MergeEntity::Region(o));
        let lookup = Regions {
            body_ids: (BrepId::from_index(20), BrepId::from_index(21)),
            keys: NotificationKeys::Namespace(10),
            primary: &a,
            other: &b,
        };
        assert_ne!(
            lookup.resolve_region(rk).unwrap().brep_id,
            lookup.resolve_region(ok).unwrap().brep_id
        );
        assert!(lookup.resolve_region(ObjectKey(0)).is_err());
        let mut attrs = AttributeStore::new();
        let h = attrs.insert(Attribute::merge_region(rk, 0, ok, 0)).unwrap();
        attrs.add_attribute(rk, h).unwrap();
        let color_id = 81;
        let color = attrs
            .insert(Attribute::color(color_id, Vec3::Y, Behavior::Copy))
            .unwrap();
        attrs.add_attribute(ok, color).unwrap();
        for (kind, expected) in [(5, rk), (6, ok)] {
            assert_eq!(
                adapter
                    .source_regions(
                        &attrs,
                        ObjectRef {
                            key: key(10, false, kind, r.index()),
                            type_code: 0
                        }
                    )
                    .unwrap(),
                [expected]
            );
        }
        let mut k = Kernel::default();
        let mut other_k = Kernel::default();
        let primary_notices = Arc::new(Mutex::new(Vec::new()));
        let other_notices = Arc::new(Mutex::new(Vec::new()));
        k.context
            .set_user_notify_callback(Some(Box::new(Notices(primary_notices.clone()))));
        other_k
            .context
            .set_user_notify_callback(Some(Box::new(Notices(other_notices.clone()))));
        let mut kernels = BooleanKernels {
            primary: &mut k,
            other: Some(&mut other_k),
        };
        adapter
            .propagate_regions(&mut kernels, &a, &b, &mut attrs, BooleanOperation::Merge)
            .unwrap();
        assert!(attrs.find_attribute(rk, ids::OTHER_REGION_ID).is_none());
        assert!(attrs.find_attribute(rk, color_id).is_some());
        assert!(attrs.find_attribute(ok, color_id).is_some());
        let notices = primary_notices.lock().unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].0, NotifyOperation::RegPropagation);
        assert_eq!(notices[0].1.key, rk);
        assert_eq!(notices[0].2[0].unwrap().key, key(10, false, 5, r.index()));
        assert_eq!(notices[0].2[1].unwrap().key, key(10, false, 6, r.index()));
        assert_eq!(notices[0].2[2].unwrap().key, rk);
        assert!(other_notices.lock().unwrap().is_empty());
        drop(notices);
        // Upstream compares IsVoid() to TRUE, rather than treating every nonzero
        // state as void. Unknown must still propagate and issue its notification.
        a.region_mut(r).unwrap().set_is_void(-1);
        let merge = attrs.insert(Attribute::merge_region(rk, 0, ok, 0)).unwrap();
        attrs.add_attribute(rk, merge).unwrap();
        adapter
            .propagate_regions(&mut kernels, &a, &b, &mut attrs, BooleanOperation::Merge)
            .unwrap();
        assert!(attrs.find_attribute(rk, color_id).is_some());
        assert_eq!(primary_notices.lock().unwrap().len(), 2);
    }

    #[test]
    fn newly_partial_intersector_stages_restore_the_live_mates_on_error() {
        let mut adapter = KernelManifoldIntersector::new(3).unwrap();
        adapter.set_intersection_tolerances(1e-9, 0.3);
        let mut a = Brep::default();
        let mut b = Brep::default();
        let f = face(&mut a);
        let of = face(&mut b);
        let v = a.add_vertex(Vec3::ZERO).unwrap();
        let ov = b.add_vertex(Vec3::X).unwrap();
        adapter.relate_faces(&mut a, f, &mut b, of).unwrap();
        adapter.merge.intersector.vertices.record(v, ov).unwrap();
        assert!(adapter
            .remove_relationship(f, FaceId::from_index(of.index() + 1))
            .is_err());
        assert_eq!(adapter.face_mate(f), Some(of));
        adapter.remove_relationship(f, of).unwrap();
        assert_eq!(adapter.face_mate(f), None);
        assert_eq!(adapter.merge.intersector.faces.primary(of), None);
        assert_eq!(adapter.merge.intersector.vertices.primary(ov), Some(v));
        let mut k = Kernel::default();
        let mut attrs = AttributeStore::default();
        let mut kernels = BooleanKernels {
            primary: &mut k,
            other: None,
        };
        let out = adapter
            .fix_gaps(&mut kernels, &mut a, &mut b, &mut attrs, &[v])
            .unwrap();
        assert!(!out.modified_topology);
        adapter
            .delete_faces(&mut kernels, &mut a, &mut attrs, &[], Some(&[]))
            .unwrap();
        // (SmBrep::GetFaces is wired since #274, so the walk now reaches the test's
        // surface-less faces, which upstream dereferences)
        let err = adapter
            .intersect_insert_relate(&mut kernels, &mut a, &mut b, &mut attrs)
            .unwrap_err();
        assert!(
            matches!(
                err,
                SolidError::InvalidInput {
                    operation: "boolean::topology_intersector::p2::intersect_topology",
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(adapter.intersector.proto_topology_manager().is_some());
        assert_eq!(adapter.merge.intersector.vertices.other(v), Some(ov));
        assert_eq!(adapter.merge.intersector.vertices.primary(ov), Some(v));
        assert_eq!(adapter.mapped_vertices().unwrap(), [v]);
    }

    #[test]
    fn notification_scope_preserves_marks_and_both_operand_callbacks_on_error() {
        use crate::boolean::topology_intersector::p1::IntersectorBrep;
        let mut k = Kernel::default();
        let mut other_k = Kernel::default();
        k.context.set_doing_boolean(true);
        let mut lock = MarkLock::new(
            std::slice::from_mut(&mut k.context),
            0,
            crate::geom::context::ALL_MARKS,
        )
        .unwrap();
        let mark = lock.mark_type();
        let primary_events = Arc::new(Mutex::new(Vec::new()));
        let other_events = Arc::new(Mutex::new(Vec::new()));
        k.context
            .set_user_notify_callback(Some(Box::new(Notices(primary_events.clone()))));
        other_k
            .context
            .set_user_notify_callback(Some(Box::new(Notices(other_events.clone()))));
        let mut a = Brep::default();
        let mut b = Brep::default();
        let v = a.add_vertex(Vec3::ZERO).unwrap();
        let ov = b.add_vertex(Vec3::X).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        records.insert(
            notification_key(4, false, EntityRef::Vertex(v)).key,
            Topology::new(),
        );
        records.insert(
            notification_key(4, true, EntityRef::Vertex(ov)).key,
            Topology::new(),
        );
        let mut attrs = AttributeStore::default();
        let mut curves = Vec::new();
        let mut extra = Vec::new();
        let current_mark = k.context.current_mark(mark);
        let error: Result<()> = Context::with_notification_context(&mut k, |k, context| {
            assert!(k.context.is_mark_locked(mark));
            assert_eq!(k.context.current_mark(mark), current_mark);
            assert!(context.doing_boolean());
            let settings = cache_kernels(k, Some(&other_k), context);
            let mut pair = ArenaPair::new(
                &mut a,
                Some(&mut b),
                &mut curves,
                &mut extra,
                &mut records,
                &mut attrs,
                settings,
                context,
                Some(&mut other_k.context),
                NotificationKeys::Namespace(4),
            );
            pair.set_vertex_point(k, arena_pair::encode(EntityRef::Vertex(v), false), Vec3::Y)?;
            pair.set_vertex_point(k, arena_pair::encode(EntityRef::Vertex(ov), true), Vec3::Z)?;
            Err(SolidError::sm_err(OP))
        })
        .unwrap();
        assert!(error.is_err());
        assert!(k.context.user_notify_callback().is_some());
        assert!(other_k.context.user_notify_callback().is_some());
        assert!(k.context.is_mark_locked(mark));
        assert_eq!(a.vertex(v).unwrap().point, Vec3::Y);
        assert_eq!(b.vertex(ov).unwrap().point, Vec3::Z);
        assert_eq!(primary_events.lock().unwrap().len(), 1);
        assert_eq!(other_events.lock().unwrap().len(), 1);
        assert_eq!(
            primary_events.lock().unwrap()[0].1.key,
            notification_key(4, false, EntityRef::Vertex(v)).key
        );
        assert_eq!(
            other_events.lock().unwrap()[0].1.key,
            notification_key(4, true, EntityRef::Vertex(ov)).key
        );
        lock.release(std::slice::from_mut(&mut k.context)).unwrap();
    }

    #[test]
    fn caller_records_keys_curves_and_both_live_callbacks_survive_success_and_error() {
        use crate::boolean::topology_intersector::p1::IntersectorBrep;
        let identity = |other, entity| {
            let object = notification_key(23, other, entity);
            ObjectRef {
                key: ObjectKey(u64::MAX - object.key.0),
                ..object
            }
        };
        let primary_key = |entity| identity(false, entity);
        let other_key = |entity| identity(true, entity);
        let primary_curve = |edge: EdgeId, type_code| ObjectRef {
            key: ObjectKey(800_000 + u64::from(edge.index())),
            type_code,
        };
        let other_curve = |edge: EdgeId, type_code| ObjectRef {
            key: ObjectKey(900_000 + u64::from(edge.index())),
            type_code,
        };
        let source_lists = |region: RegionId| {
            [0, 1].map(|list| ObjectRef {
                key: ObjectKey(1_000_000 + u64::from(region.index()) * 2 + list),
                type_code: 0,
            })
        };
        let keys = ManifoldNotificationKeys {
            primary: &primary_key,
            other: &other_key,
            primary_curve: &|edge, kind| Ok(primary_curve(edge, kind)),
            other_curve: &|edge, kind| Ok(other_curve(edge, kind)),
            source_lists: &|region| Ok(source_lists(region)),
            actual_curve_identities: false,
        };
        for fail in [false, true] {
            let mut a = Brep::new();
            let mut b = Brep::new();
            let av = a.add_vertex(Vec3::ZERO).unwrap();
            let bv = b.add_vertex(Vec3::X).unwrap();
            let ak = primary_key(EntityRef::Vertex(av)).key;
            let bk = other_key(EntityRef::Vertex(bv)).key;
            let mut records = crate::topology::topology::NotifyRecords::from(BTreeMap::from([
                (ak, Topology::new()),
                (bk, Topology::new()),
            ]));
            let mut attributes = AttributeStore::new();
            let attr = attributes
                .insert(Attribute::long(78, 99, Behavior::Reference))
                .unwrap();
            attributes.add_attribute(ak, attr).unwrap();
            let mut k = Kernel::default();
            let mut other_k = Kernel::default();
            let notices = Arc::new(Mutex::new(Vec::new()));
            let other_notices = Arc::new(Mutex::new(Vec::new()));
            k.context
                .set_user_notify_callback(Some(Box::new(Notices(notices.clone()))));
            other_k
                .context
                .set_user_notify_callback(Some(Box::new(Notices(other_notices.clone()))));
            let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, 1).unwrap();
            let mark = lock.mark_type();
            records.get_mut(&ak).unwrap().mark(Some(&k.context), mark);
            let before = k.context.current_mark(mark);
            let result: Result<()> =
                KernelManifoldIntersector::with_notifications(keys, &mut records, |adapter| {
                    assert!(adapter
                        .records
                        .get(&ak)
                        .unwrap()
                        .is_marked(&k.context, mark));
                    assert_eq!(adapter.object_key(false, MergeEntity::Vertex(av)), ak);
                    assert_eq!(adapter.object_key(true, MergeEntity::Vertex(bv)), bk);
                    let edge = EdgeId::from_index(0);
                    assert_eq!(
                        adapter.keys.curve(false, edge, 314).unwrap(),
                        primary_curve(edge, 314)
                    );
                    assert_eq!(
                        adapter.keys.curve(true, edge, 315).unwrap(),
                        other_curve(edge, 315)
                    );
                    Context::with_notification_context(&mut k, |k, context| {
                        let settings = cache_kernels(k, Some(&other_k), context);
                        let mut pair = ArenaPair::new(
                            &mut a,
                            Some(&mut b),
                            &mut adapter.curves,
                            &mut adapter.extra_curves,
                            &mut adapter.records,
                            &mut attributes,
                            settings,
                            context,
                            Some(&mut other_k.context),
                            adapter.keys,
                        );
                        let cache =
                            crate::intersect::topology_solver::p1::TopologyObjects::brep_cache(
                                &pair,
                                arena_pair::brep_h(0),
                            );
                        let mut cache = cache.unwrap().clone();
                        assert!(cache.vertex_tree_mut().is_some());
                        pair.set_vertex_point(
                            k,
                            arena_pair::encode(EntityRef::Vertex(av), false),
                            Vec3::Y,
                        )?;
                        pair.set_vertex_point(
                            k,
                            arena_pair::encode(EntityRef::Vertex(bv), true),
                            Vec3::Z,
                        )?;
                        if fail {
                            Err(SolidError::sm_err("test::after-notifications"))
                        } else {
                            Ok(())
                        }
                    })
                    .and_then(|r| r)
                });
            assert_eq!(result.is_err(), fail);
            assert_eq!(a.vertex(av).unwrap().point, Vec3::Y);
            assert_eq!(b.vertex(bv).unwrap().point, Vec3::Z);
            assert!(records.contains_key(&ak));
            assert!(records.contains_key(&bk));
            assert_eq!(attributes.find_attribute(ak, 78), Some(attr));
            assert_eq!(notices.lock().unwrap()[0].1.key, ak);
            assert_eq!(other_notices.lock().unwrap()[0].1.key, bk);
            assert!(k.context.is_mark_locked(mark));
            assert_eq!(k.context.current_mark(mark), before);
            lock.release(std::slice::from_mut(&mut k.context)).unwrap();
        }
    }

    #[test]
    fn caller_records_return_with_partial_edits_when_the_action_unwinds() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let primary = |entity| notification_key(23, false, entity);
        let other = |entity| notification_key(23, true, entity);
        let curve = |edge: EdgeId, type_code| ObjectRef {
            key: ObjectKey(800_000 + u64::from(edge.index())),
            type_code,
        };
        let source_lists = |region: RegionId| {
            [0, 1].map(|list| ObjectRef {
                key: ObjectKey(1_000_000 + u64::from(region.index()) * 2 + list),
                type_code: 0,
            })
        };
        let keys = ManifoldNotificationKeys {
            primary: &primary,
            other: &other,
            primary_curve: &|edge, kind| Ok(curve(edge, kind)),
            other_curve: &|edge, kind| Ok(curve(edge, kind)),
            source_lists: &|region| Ok(source_lists(region)),
            actual_curve_identities: false,
        };
        let old = primary(EntityRef::Vertex(VertexId::from_index(0))).key;
        let added = primary(EntityRef::Vertex(VertexId::from_index(1))).key;
        let mut records = crate::topology::topology::NotifyRecords::from(BTreeMap::from([(
            old,
            Topology::new(),
        )]));
        let kernel = Kernel::default();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            KernelManifoldIntersector::with_notifications(keys, &mut records, |adapter| {
                adapter
                    .records
                    .get_mut(&old)
                    .unwrap()
                    .mark(Some(&kernel.context), MarkType::Mark);
                adapter.records.insert(added, Topology::new());
                panic!("injected caller action panic");
            });
        }));
        assert!(outcome.is_err());
        assert_eq!(records.len(), 2);
        assert!(records
            .get(&old)
            .unwrap()
            .is_marked(&kernel.context, MarkType::Mark));
        assert!(records.contains_key(&added));
    }

    #[test]
    fn transform_delegates_identity_and_reports_the_actual_boundary() {
        let mut adapter = KernelManifoldIntersector::new(5).unwrap();
        let mut k = Kernel::default();
        let mut b = Brep::default();
        let mut attrs = AttributeStore::default();
        let mut kernels = BooleanKernels {
            primary: &mut k,
            other: None,
        };
        adapter
            .transform_brep(
                &mut kernels,
                &mut b,
                false,
                &mut attrs,
                &Axis2Placement::default(),
                None,
                true,
            )
            .unwrap();
        let mut placement = Axis2Placement::default();
        placement.translate(Vec3::X);
        adapter
            .transform_brep(
                &mut kernels,
                &mut b,
                false,
                &mut attrs,
                &placement,
                None,
                true,
            )
            .unwrap();
        let id = 17;
        let entities = [
            EntityRef::Region(RegionId::from_index(id)),
            EntityRef::Face(FaceId::from_index(id)),
            EntityRef::Edge(EdgeId::from_index(id)),
            EntityRef::Vertex(VertexId::from_index(id)),
            EntityRef::Edgeuse(EdgeuseId::from_index(id)),
            EntityRef::Vertexuse(VertexuseId::from_index(id)),
        ];
        let keys: BTreeSet<_> = entities
            .into_iter()
            .flat_map(|e| {
                [
                    notification_key(5, false, e).key,
                    notification_key(5, true, e).key,
                ]
            })
            .collect();
        assert_eq!(keys.len(), 12);
    }

    fn wire(b: &mut Brep) -> EdgeId {
        let shell = b.add_shell().unwrap();
        let v = b.add_vertex(Vec3::ZERO).unwrap();
        let w = b.add_vertex(Vec3::X).unwrap();
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
        let data = b.edge_mut(e).unwrap();
        data.curve = Some(crate::curve::line::Line::from_points(Vec3::ZERO, Vec3::X, 3).into());
        data.interval = crate::geom::extent1d::Extent1d::from_raw(0.0, 1.0);
        e
    }

    #[test]
    fn wire_stage_uses_other_context_marks_and_resets_output() {
        let mut adapter = KernelManifoldIntersector::new(0).unwrap();
        let mut a = Brep::new();
        let mut b = Brep::new();
        let ae = wire(&mut a);
        let be = wire(&mut b);
        adapter.merge.intersector.edges.record(ae, be).unwrap();
        let mut k = Kernel::default();
        let mut other_k = Kernel::default();
        let lock = MarkLock::new(std::slice::from_mut(&mut other_k.context), 0, 1).unwrap();
        let mark = lock.mark_type();
        assert_ne!(
            k.context.current_mark(mark),
            other_k.context.current_mark(mark)
        );
        let mut state = ManifoldState::default();
        state.other_marks.mark(&other_k, Entity::Edge(be), mark);
        for v in b.vertices().ids() {
            state.other_marks.mark(&other_k, Entity::Vertex(v), mark);
        }
        let mut kernels = BooleanKernels {
            primary: &mut k,
            other: Some(&mut other_k),
        };
        let mut keep = vec![EdgeId::from_index(123)];
        adapter
            .process_wires(
                &mut kernels,
                &mut a,
                &mut b,
                &mut state,
                &mut AttributeStore::new(),
                BooleanOperation::Merge,
                mark,
                mark,
                &mut keep,
            )
            .unwrap();
        assert_eq!(keep, [ae]);
    }

    #[test]
    fn surface_subset_uses_shared_owner_identity_and_can_be_cleared() {
        let mut adapter = KernelManifoldIntersector::new(0).unwrap();
        let mut b = Brep::new();
        let f0 = face(&mut b);
        let f1 = face(&mut b);
        let f2 = face(&mut b);
        let mut surface: crate::surface::surface::Surface =
            crate::surface::plane::p1::Plane::from_normal(Vec3::ZERO, Vec3::Z).into();
        *surface.owner_mut() = Some(arena_identity(CacheObject::Face(f0)));
        b.face_mut(f0).unwrap().surface = Some(surface.clone());
        b.face_mut(f1).unwrap().surface = Some(surface);
        b.face_mut(f2).unwrap().surface =
            Some(crate::surface::plane::p1::Plane::from_normal(Vec3::ZERO, Vec3::Z).into());
        adapter.set_surface_subset(&[KernelManifoldIntersector::surface_identity(&b, f0).unwrap()]);
        assert!(!adapter.excluded_surface(&b, f0).unwrap());
        assert!(!adapter.excluded_surface(&b, f1).unwrap());
        assert!(adapter.excluded_surface(&b, f2).unwrap());
        adapter.set_surface_subset(&[]);
        assert!(!adapter.excluded_surface(&b, f2).unwrap());
        adapter.topology_deleted = true;
        assert!(adapter.topology_deleted());
        adapter.clear_topology_deleted();
        assert!(!adapter.topology_deleted());
    }

    #[test]
    fn tolerances_match_the_canonical_merge_constructor() {
        use crate::math::EFF_ZERO;
        let mut a = Brep::new();
        let mut b = Brep::new();
        wire(&mut a);
        wire(&mut b);
        a.set_tolerance(EFF_ZERO * 0.8);
        b.set_tolerance(EFF_ZERO * 0.4);

        let mut adapter = KernelManifoldIntersector::new(9).unwrap();
        adapter.merge.options = p1::MergeOptions::new(true, true, false, true, true);
        adapter.merge.tag_id = 42;
        adapter.merge.manifold_boolean = Some(false);
        adapter.merge.intersection_loop_closed = Some(false);
        let options = adapter.merge.options;
        let kernel = Kernel::default();
        for requested in [0.0, -0.0, -1.0, EFF_ZERO * 0.5, EFF_ZERO, 0.123] {
            for angle in [0.0, 0.345] {
                let expected = MergeState::<TopologyIntersector<MergeOperand>>::with_breps(
                    &kernel,
                    (&a, MergeOperand::Primary),
                    Some((&b, MergeOperand::Other)),
                    requested,
                    angle,
                    None,
                )
                .unwrap();
                adapter
                    .configure_operands(&kernel, &a, &b, requested, angle)
                    .unwrap();
                assert_eq!(adapter.merge.options, options);
                assert_eq!(adapter.merge.tag_id, 42);
                assert_eq!(adapter.merge.manifold_boolean, Some(false));
                assert_eq!(adapter.merge.intersection_loop_closed, Some(false));
                assert_eq!(
                    adapter.intersection_tolerances(),
                    (
                        expected.intersector.this_approx_tol_3d(),
                        expected.intersector.this_ang_tol_rad()
                    )
                );
            }
        }
    }

    #[test]
    fn notification_key_blocks_never_alias_across_operands_or_classes() {
        let a = KernelManifoldIntersector::new(0).unwrap();
        let b = KernelManifoldIntersector::new(0x03ff_ffff).unwrap();
        assert!(KernelManifoldIntersector::new(0x0400_0000).is_err());
        // the largest namespace's keys stay below the kernel's reserved range
        assert!(!crate::geom::context::is_reserved_key(b.object_key(
            true,
            MergeEntity::Vertex(VertexId::from_index(u32::MAX))
        )));
        let mut keys = BTreeSet::new();
        for adapter in [&a, &b] {
            for other in [false, true] {
                for i in [0, u32::MAX] {
                    for entity in [
                        MergeEntity::Vertex(VertexId::from_index(i)),
                        MergeEntity::Edge(EdgeId::from_index(i)),
                        MergeEntity::Face(FaceId::from_index(i)),
                        MergeEntity::Region(RegionId::from_index(i)),
                    ] {
                        assert!(keys.insert(adapter.object_key(other, entity)));
                    }
                }
            }
        }
    }
}
