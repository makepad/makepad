//! Arena access for the existing generic topology intersector used by merge p4.
//!
//! Upstream: `source/SMLib/src/SmTopologyIntersector.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `ArenaPair` (private) | operand access through the intersector's topology handles |
//!
//! Deviations: borrowed arenas replace pointers; generated curves have local handles.
//! This adapts the ported operations without copying their geometry algorithms.
//! The faces' trimmed surface caches that face intersections make are kept for later
//! face intersections, as upstream keeps them on the surfaces; upstream drops one when
//! its face is edited, the pair drops a B-rep's caches at any mutable lend of it but
//! a face intersection or a classification query.
//! `ProtoBrep::face_curve_on_classify_mut` classifies on the face's B-rep in the pair
//! (`curve_on_classify_mut`), keeping its lazy edits there; the `&self`
//! `face_curve_on_classify` still classifies on a private copy for its remaining callers.
//! No input-dependent recursion is introduced.

use super::notification_keys::NotificationKeys;
use crate::boolean::topology_intersector::p1 as ti;
use crate::boolean::topology_intersector::p1::IntersectorBrep;
use crate::boolean::topology_intersector::p2::IntersectorBrepP2;
use crate::boolean::topology_intersector::p3::IntersectorBrepP3;
use crate::curve::curve::Curve as SmCurve;
use crate::geom::a_object::BREP_TYPE;
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::context::Context;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::object::{ObjectRef, ShapeMember};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::{update_object_tolerance, TopologyToUpdate};
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::curve_class::p6::{MergeContext, MergeIntervalBrep};
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, ClassObjectKind, MergeOutputs, PointClassType, PointClassification,
};
use crate::intersect::topology_solver::p1::TopologyObjects;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::{BrepQuery, FaceCaches};
use crate::query::face::p2::{
    curve_on_classify, curve_on_classify_mut, point_classify, CurveOnClassifyBrep,
    CurveOnClassifyObjects, FaceBrep, PointClassifyOptions,
};
use crate::surface::surface::Surface;
use crate::surface::volume::Volume;
use crate::topology::brep::p1 as bp1;
use crate::topology::brep::p7;
use crate::topology::brep_cache::{arena_identity, BrepCache, CacheObject, CacheSource};
use crate::topology::edge as ed;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p3 as eu3;
use crate::topology::face::p2 as fp2;
use crate::topology::face::p3 as fp3;
use crate::topology::face::p3::{FaceIntersectOptions, IntersectObject};
use crate::topology::loopuse as lu;
use crate::topology::proto_topology::p1::ProtoTopology;
use crate::topology::proto_topology::{ProtoBrep, ProtoGeometry};
use crate::topology::vertex as vx;
use crate::topology::{
    Brep, BrepId, EdgeId, EdgeuseId, FaceId, FaceuseId, LoopId, LoopuseId, RegionId, VertexId,
    VertexuseId,
};
use crate::{EntityRef, SolidError};
use std::cell::Cell;
use std::rc::Rc;
const K_VERTEX: u8 = 0;
const K_EDGE: u8 = 1;
const K_FACE: u8 = 2;
const K_EDGE_CURVE: u8 = 3;
const K_FACE_SURFACE: u8 = 4;
const K_CURVE: u8 = 5;
const K_BREP: u8 = 6;
const K_EXTRA: u8 = 7;
const K_EDGEUSE: u8 = 8;
const K_VERTEXUSE: u8 = 9;
const K_LOOP: u8 = 10;
const K_TI: u8 = 11;
const K_TRIM_CURVE: u8 = 12;
const K_NULL: u8 = 13;
const K_REGION: u8 = 14;
const K_LOOPUSE: u8 = 15;
const K_FACEUSE: u8 = 16;
/// An object handle: the B-rep (`2` for the curve stores, `-1` for the intersector),
/// the kind and the index. Ordered by operand, class and arena index. A
/// curve handle also carries its curve's natural interval (what `SmSolutionArray`
/// reads through `SmCurve::GetNaturalInterval`); it takes no part in identity.
#[derive(Debug, Clone, Copy)]
pub(super) struct H {
    b: i8,
    kind: u8,
    i: u32,
    ivl: [f64; 2],
}

impl PartialEq for H {
    fn eq(&self, o: &Self) -> bool {
        (self.b, self.kind, self.i) == (o.b, o.kind, o.i)
    }
}

impl Eq for H {}

impl PartialOrd for H {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for H {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.b, self.kind, self.i).cmp(&(o.b, o.kind, o.i))
    }
}

pub(super) const fn hd(b: i8, kind: u8, i: u32) -> H {
    H {
        b,
        kind,
        i,
        ivl: [f64::NAN, f64::NAN],
    }
}

/// A curve handle carrying the curve's natural interval.
fn curve_h(b: i8, kind: u8, i: u32, c: &SmCurve) -> H {
    let ivl = c.natural_interval();
    H {
        b,
        kind,
        i,
        ivl: [ivl.min(), ivl.max()],
    }
}

impl ClassObject for H {
    fn class_kind(&self) -> ClassObjectKind {
        match self.kind {
            K_VERTEX => ClassObjectKind::Vertex,
            K_EDGE => ClassObjectKind::Edge,
            K_FACE => ClassObjectKind::Face,
            K_EDGE_CURVE | K_CURVE | K_EXTRA | K_TRIM_CURVE => ClassObjectKind::Curve,
            K_FACE_SURFACE => ClassObjectKind::Surface,
            K_EDGEUSE => ClassObjectKind::Edgeuse,
            K_VERTEXUSE => ClassObjectKind::Vertexuse,
            K_LOOP => ClassObjectKind::Loop,
            K_REGION => ClassObjectKind::Region,
            _ => ClassObjectKind::Other,
        }
    }
}

impl SolutionObject for H {
    fn kind(&self) -> ObjectKind {
        match self.kind {
            K_VERTEX => ObjectKind::Vertex,
            K_EDGE => ObjectKind::Edge,
            K_FACE => ObjectKind::Face,
            K_EDGE_CURVE | K_CURVE | K_EXTRA | K_TRIM_CURVE => ObjectKind::Curve,
            K_FACE_SURFACE => ObjectKind::Surface,
            _ => ObjectKind::Other,
        }
    }
    fn natural_interval(&self) -> Option<Extent1d> {
        match self.kind {
            K_EDGE_CURVE | K_CURVE | K_EXTRA if self.ivl.first()?.is_finite() => Some(
                Extent1d::from_min_max(*self.ivl.first()?, *self.ivl.get(1)?),
            ),
            _ => None,
        }
    }
}

pub(super) const TI_HANDLE: H = hd(-1, K_TI, 0);

pub(super) const fn brep_h(b: i8) -> H {
    hd(b, K_BREP, b as u32)
}

#[derive(Debug)]
pub(super) struct ArenaPair<'a> {
    breps: [Option<&'a mut Brep>; 2],
    curves: &'a mut Vec<SmCurve>,
    extra: &'a mut Vec<SmCurve>,
    records: &'a mut crate::topology::topology::NotifyRecords,
    attributes: &'a mut AttributeStore,
    primary_context: &'a mut Context,
    other_context: Option<&'a mut Context>,
    keys: NotificationKeys<'a>,
    identities: Identities<'a>,
    cache_kernels: [Kernel; 2],
    caches: [std::cell::OnceCell<BrepCache<H>>; 2],
    /// The trimmed surface caches of each B-rep's faces that face intersections made
    /// (upstream keeps them on the surfaces). Any mutable use of a B-rep other than
    /// a face intersection or a classification query
    /// ([`ClassGeometryMut::topology_entity_for_query`]) empties them.
    face_caches: [FaceCaches; 2],
}

/// The identity callbacks of each operand's edits that a [`MergeContext`] borrows for as
/// long as it borrows the pair (`NotificationKeys` builds its identities on the call).
/// A curve identity the caller's keys refuse cannot be returned through the context's
/// infallible callback: the first refusal is kept (with a placeholder identity returned)
/// and [`ArenaPair::take_identity_failure`] reports it, so the merge fails with it.
struct Identities<'a> {
    objects: [Box<dyn Fn(EntityRef) -> ObjectRef + 'a>; 2],
    curves: [Box<dyn Fn(EdgeId, i64) -> ObjectRef + 'a>; 2],
    failure: Rc<Cell<Option<SolidError>>>,
}

impl core::fmt::Debug for Identities<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Identities").finish_non_exhaustive()
    }
}

impl<'a> Identities<'a> {
    fn new(keys: NotificationKeys<'a>) -> Self {
        let failure: Rc<Cell<Option<SolidError>>> = Rc::new(Cell::new(None));
        let object = move |other: bool| -> Box<dyn Fn(EntityRef) -> ObjectRef + 'a> {
            Box::new(move |e| keys.object(other, e))
        };
        let curve = |other: bool| -> Box<dyn Fn(EdgeId, i64) -> ObjectRef + 'a> {
            let failure = Rc::clone(&failure);
            Box::new(move |e, kind| {
                keys.curve(other, e, kind).unwrap_or_else(|err| {
                    let first = failure.take();
                    failure.set(first.or(Some(err)));
                    ObjectRef {
                        key: ObjectKey(u64::MAX),
                        type_code: kind,
                    }
                })
            })
        };
        Self {
            objects: [object(false), object(true)],
            curves: [curve(false), curve(true)],
            failure,
        }
    }
}

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input("boolean::merge::p4::arena_pair", reason)
}
const fn vid(h: H) -> VertexId {
    VertexId::from_index(h.i)
}
const fn eid(h: H) -> EdgeId {
    EdgeId::from_index(h.i)
}
const fn fid(h: H) -> FaceId {
    FaceId::from_index(h.i)
}
impl ArenaPair<'_> {
    pub(super) fn new<'a>(
        primary: &'a mut Brep,
        other: Option<&'a mut Brep>,
        curves: &'a mut Vec<SmCurve>,
        extra: &'a mut Vec<SmCurve>,
        records: &'a mut crate::topology::topology::NotifyRecords,
        attributes: &'a mut AttributeStore,
        cache_kernels: [Kernel; 2],
        primary_context: &'a mut Context,
        other_context: Option<&'a mut Context>,
        keys: NotificationKeys<'a>,
    ) -> ArenaPair<'a> {
        ArenaPair {
            breps: [Some(primary), other],
            curves,
            extra,
            records,
            attributes,
            primary_context,
            other_context,
            keys,
            identities: Identities::new(keys),
            cache_kernels,
            caches: [std::cell::OnceCell::new(), std::cell::OnceCell::new()],
            face_caches: [FaceCaches::new(), FaceCaches::new()],
        }
    }

    /// The first curve identity the caller's keys refused during a [`MergeContext`] edit
    /// (see [`Identities`]); `None` when there was none.
    pub(super) fn take_identity_failure(&self) -> Option<SolidError> {
        self.identities.failure.take()
    }
    fn brep(&self, h: H) -> Option<&Brep> {
        self.breps.get(usize::try_from(h.b).ok()?)?.as_deref()
    }

    fn brep_mut(&mut self, h: H) -> Option<&mut Brep> {
        let slot = usize::try_from(h.b).ok()?;
        self.clear_face_caches(slot);
        self.breps.get_mut(slot)?.as_deref_mut()
    }

    /// Empties the face caches of the B-rep in `slot`: a caller about to lend the
    /// B-rep mutably may change it, so they may no longer match.
    fn clear_face_caches(&mut self, slot: usize) {
        if let Some(caches) = self.face_caches.get_mut(slot) {
            caches.clear();
        }
    }

    fn brep_r(&self, h: H) -> crate::Result<&Brep> {
        self.brep(h)
            .ok_or_else(|| invalid("handle names an absent B-rep"))
    }

    fn brep_m(&mut self, h: H) -> crate::Result<&mut Brep> {
        self.brep_mut(h)
            .ok_or_else(|| invalid("handle names an absent B-rep"))
    }

    /// The edge's curve handle (`ProtoBrep::edge_curve`'s shape).
    #[allow(clippy::option_option)]
    fn edge_curve_h(&self, e: H) -> Option<Option<H>> {
        let b = self.brep(e)?;
        let edge = b.edge(eid(e)).ok()?;
        Some(
            edge.curve
                .as_ref()
                .map(|c| curve_h(e.b, K_EDGE_CURVE, e.i, c)),
        )
    }

    /// The face's surface handle (`ProtoBrep::face_surface`'s shape).
    #[allow(clippy::option_option)]
    fn face_surface_h(&self, f: H) -> Option<Option<H>> {
        let b = self.brep(f)?;
        let face = b.face(fid(f)).ok()?;
        Some(face.surface.as_ref().map(|_| hd(f.b, K_FACE_SURFACE, f.i)))
    }

    /// Gives a curve made by the port a handle in the curve store.
    fn register_curve(&mut self, curve: SmCurve) -> crate::Result<H> {
        self.insert_curve(curve)
            .ok_or_else(|| invalid("curve store is full"))
    }

    /// [`Two::register_curve`] for each curve present.
    fn register_curves(&mut self, curves: Vec<Option<SmCurve>>) -> crate::Result<Vec<Option<H>>> {
        curves
            .into_iter()
            .map(|c| c.map(|c| self.register_curve(c)).transpose())
            .collect()
    }

    fn entity_h(b: i8, e: EntityRef) -> Option<H> {
        match e {
            EntityRef::Vertex(v) => Some(hd(b, K_VERTEX, v.index())),
            EntityRef::Edge(x) => Some(hd(b, K_EDGE, x.index())),
            EntityRef::Face(f) => Some(hd(b, K_FACE, f.index())),
            EntityRef::Loop(l) => Some(hd(b, K_LOOP, l.index())),
            EntityRef::Edgeuse(eu) => Some(hd(b, K_EDGEUSE, eu.index())),
            EntityRef::Region(r) => Some(hd(b, K_REGION, r.index())),
            _ => None,
        }
    }

    /// Distinct `(operand_index, &Brep)` pairs currently held by this pair.
    fn distinct_breps(&self) -> Vec<(i8, &Brep)> {
        let mut out: Vec<(i8, &Brep)> = Vec::new();
        for (slot, entry) in self.breps.iter().enumerate() {
            let Some(b) = entry.as_deref() else {
                continue;
            };
            let Ok(bi) = i8::try_from(slot) else {
                continue;
            };
            if !out.iter().any(|&(_, prev)| std::ptr::eq(prev, b)) {
                out.push((bi, b));
            }
        }
        out
    }

    /// Operand-scoped canonical owner key of an `Edge` or `Face`, matching
    /// `ArenaPair::brep_cache`.
    fn entity_owner_ref(&self, bi: i8, b: &Brep, entity: EntityRef) -> Option<ObjectRef> {
        let expected = self.keys.object(bi == 1, entity);
        let (actual, arena_obj) = match entity {
            EntityRef::Edge(edge) => (
                b.edge(edge).ok()?.curve.as_ref().and_then(SmCurve::owner),
                CacheObject::Edge(edge),
            ),
            EntityRef::Face(face) => (
                b.face(face).ok()?.surface.as_ref().and_then(Surface::owner),
                CacheObject::Face(face),
            ),
            _ => return None,
        };
        if actual == Some(expected) {
            return Some(expected);
        }
        let proven = b.geometry_owner_key(entity);
        if let Some(proven) = proven {
            if actual == Some(proven) {
                return Some(proven);
            }
        }
        if let Some(serial) = b.notify_serial() {
            let canonical = crate::topology::brep::p3::topology_ref(serial, entity);
            if actual == Some(canonical) {
                return Some(canonical);
            }
        }
        Some(if matches!(self.keys, NotificationKeys::Caller(_)) {
            proven.unwrap_or(expected)
        } else {
            arena_identity(arena_obj)
        })
    }

    /// True when `owner` matches a globally disjoint identity of `entity` in `(bi, b)`.
    fn matches_disjoint_owner(
        &self,
        bi: i8,
        b: &Brep,
        entity: EntityRef,
        owner: ObjectRef,
    ) -> bool {
        self.keys.object(bi == 1, entity) == owner
            || b.geometry_owner_key(entity) == Some(owner)
            || b.notify_serial()
                .is_some_and(|s| crate::topology::brep::p3::topology_ref(s, entity) == owner)
    }
}
const OP_FACE_CACHE: &str = "boolean::merge::p4::arena_pair::face_cache_target";

/// The handle of an entity of operand `b` a face's trimmed surface cache names (its
/// faces, edges, vertices, loop uses, edge uses and vertex uses).
fn cache_entity_h(b: i8, e: EntityRef) -> crate::Result<H> {
    Ok(match e {
        EntityRef::Face(f) => hd(b, K_FACE, f.index()),
        EntityRef::Edge(x) => hd(b, K_EDGE, x.index()),
        EntityRef::Vertex(v) => hd(b, K_VERTEX, v.index()),
        EntityRef::Loopuse(lu) => hd(b, K_LOOPUSE, lu.index()),
        EntityRef::Edgeuse(eu) => hd(b, K_EDGEUSE, eu.index()),
        EntityRef::Vertexuse(vu) => hd(b, K_VERTEXUSE, vu.index()),
        _ => return Err(invalid("entity a surface cache does not name")),
    })
}

/// The entity of operand `b` a surface cache handle names (inverse of
/// [`cache_entity_h`]).
fn cache_h_entity(b: i8, h: H) -> crate::Result<EntityRef> {
    if h.b != b {
        return Err(invalid("surface cache handle of another operand"));
    }
    Ok(match h.kind {
        K_FACE => EntityRef::Face(fid(h)),
        K_EDGE => EntityRef::Edge(eid(h)),
        K_VERTEX => EntityRef::Vertex(vid(h)),
        K_LOOPUSE => EntityRef::Loopuse(LoopuseId::from_index(h.i)),
        K_EDGEUSE => EntityRef::Edgeuse(EdgeuseId::from_index(h.i)),
        K_VERTEXUSE => EntityRef::Vertexuse(VertexuseId::from_index(h.i)),
        _ => return Err(invalid("handle a surface cache does not name")),
    })
}

impl ClassGeometry<H> for ArenaPair<'_> {
    fn curve(&self, o: H) -> Option<&SmCurve> {
        match o.kind {
            K_EDGE_CURVE => self.brep(o)?.edge(eid(o)).ok()?.curve.as_ref(),
            K_CURVE => self.curves.get(o.i as usize),
            K_TRIM_CURVE => {
                eu1::uv_trim_curve(self.brep(o)?, EdgeuseId::from_index(o.i))
                    .ok()?
                    .0
            }
            K_EXTRA => self.extra.get(o.i as usize),
            _ => None,
        }
    }
    fn surface(&self, o: H) -> Option<&Surface> {
        match o.kind {
            K_FACE_SURFACE => self.brep(o)?.face(fid(o)).ok()?.surface.as_ref(),
            _ => None,
        }
    }
    fn volume(&self, _o: H) -> Option<&Volume> {
        None
    }
    fn point(&self, _o: H) -> Option<Point3> {
        None
    }
    // Classification reads the actual vertex points and stored tolerances.
    fn classification_vertex_point(&self, o: H) -> Option<Point3> {
        if o.kind != K_VERTEX {
            return None;
        }
        vx::point(self.brep(o)?, vid(o)).ok()
    }
    fn topology_zone_tol_3d(&self, o: H) -> Option<f64> {
        match o.kind {
            K_VERTEX | K_EDGE | K_FACE => self.topology_tolerance(o),
            _ => None,
        }
    }
    // Topology handles name entities of their operand's arena, so the classification
    // reaches their members (`SmEdge::GetVertex`, `SmEdge::GetCurve`, ...) as upstream's
    // pointers do.
    fn topology_entity(&self, o: H) -> Option<(&Brep, EntityRef)> {
        let t = match o.kind {
            K_VERTEX => EntityRef::Vertex(vid(o)),
            K_EDGE => EntityRef::Edge(eid(o)),
            K_FACE => EntityRef::Face(fid(o)),
            K_LOOP => EntityRef::Loop(LoopId::from_index(o.i)),
            K_EDGEUSE => EntityRef::Edgeuse(EdgeuseId::from_index(o.i)),
            K_VERTEXUSE => EntityRef::Vertexuse(VertexuseId::from_index(o.i)),
            K_REGION => EntityRef::Region(RegionId::from_index(o.i)),
            _ => return None,
        };
        Some((self.brep(o)?, t))
    }
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<H> {
        let b = self
            .breps
            .iter()
            .position(|x| x.as_deref().is_some_and(|x| std::ptr::eq(x, brep)))?;
        let b = i8::try_from(b).ok()?;
        match entity {
            EntityRef::Vertexuse(vu) => Some(hd(b, K_VERTEXUSE, vu.index())),
            _ => Self::entity_h(b, entity),
        }
    }
    fn brep_id(&self, brep: &Brep) -> Option<BrepId> {
        let b = self
            .breps
            .iter()
            .position(|x| x.as_deref().is_some_and(|x| std::ptr::eq(x, brep)))?;
        Some(BrepId::from_index(u32::try_from(b).ok()?))
    }
    fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
        let breps = self.distinct_breps();
        if owner.type_code == ed::TYPE {
            for &(bi, b) in &breps {
                for edge in b.edges().ids() {
                    let entity = EntityRef::Edge(edge);
                    if self.matches_disjoint_owner(bi, b, entity, owner) {
                        return Some((b, entity));
                    }
                }
            }
            let mut fallback = None;
            for &(bi, b) in &breps {
                for edge in b.edges().ids() {
                    let entity = EntityRef::Edge(edge);
                    let actual = b.edge(edge).ok().and_then(|e| e.curve.as_ref()?.owner());
                    if actual == Some(owner)
                        && (self.entity_owner_ref(bi, b, entity) == Some(owner)
                            || bp1::default_edge_owner(b, edge) == owner)
                    {
                        if fallback.is_some() {
                            return None;
                        }
                        fallback = Some((b, entity));
                    }
                }
            }
            return fallback;
        }
        if owner.type_code == fp2::TYPE {
            for &(bi, b) in &breps {
                for face in b.faces().ids() {
                    let entity = EntityRef::Face(face);
                    if self.matches_disjoint_owner(bi, b, entity, owner) {
                        return Some((b, entity));
                    }
                }
            }
            let mut fallback = None;
            for &(bi, b) in &breps {
                for face in b.faces().ids() {
                    let entity = EntityRef::Face(face);
                    let actual = b.face(face).ok().and_then(|f| f.surface.as_ref()?.owner());
                    if actual == Some(owner)
                        && (self.entity_owner_ref(bi, b, entity) == Some(owner)
                            || bp1::default_face_owner(b, face) == owner)
                    {
                        if fallback.is_some() {
                            return None;
                        }
                        fallback = Some((b, entity));
                    }
                }
            }
            return fallback;
        }
        None
    }
    fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        let breps = self.distinct_breps();
        if owner.type_code == eu1::TYPE {
            for &(bi, b) in &breps {
                for eu in b.edgeuses().ids() {
                    let entity = EntityRef::Edgeuse(eu);
                    if self.matches_disjoint_owner(bi, b, entity, owner) {
                        let edge = eu1::edge(b, eu).ok()??;
                        return self.entity_owner_ref(bi, b, EntityRef::Edge(edge));
                    }
                }
            }
            let mut fallback = None;
            for &(bi, b) in &breps {
                for eu in b.edgeuses().ids() {
                    let actual = b
                        .edgeuse(eu)
                        .ok()
                        .and_then(|r| r.uv_trim_curve.as_ref()?.owner());
                    if actual == Some(owner) {
                        let edge = eu1::edge(b, eu).ok()??;
                        let edge_owner = self.entity_owner_ref(bi, b, EntityRef::Edge(edge))?;
                        if fallback.is_some() {
                            return None;
                        }
                        fallback = Some(edge_owner);
                    }
                }
            }
            return fallback;
        }
        if crate::curve::curve::p2::is_curve_class(owner.type_code) {
            for &(bi, b) in &breps {
                for edge in b.edges().ids() {
                    let Some(c) = b.edge(edge).ok().and_then(|r| r.curve.as_ref()) else {
                        continue;
                    };
                    if self.keys.curve(bi == 1, edge, c.type_code()).ok() == Some(owner) {
                        return c.owner();
                    }
                }
            }
        }
        None
    }
}

impl ProtoGeometry<H> for ArenaPair<'_> {
    fn curve_mut(&mut self, o: H) -> Option<&mut SmCurve> {
        match o.kind {
            K_EDGE_CURVE => self.brep_mut(o)?.edge_mut(eid(o)).ok()?.curve.as_mut(),
            K_CURVE => self.curves.get_mut(o.i as usize),
            K_EXTRA => self.extra.get_mut(o.i as usize),
            _ => None,
        }
    }
}

/// The face-side objects of `curve_on_classify` for one B-rep: `two` is the pair, shared
/// (`&ArenaPair`) or lent mutably (`&mut ArenaPair`, for [`curve_on_classify_mut`] on the
/// face's own B-rep).
struct Objs<P> {
    two: P,
    b: i8,
    /// [`FaceBrep::brep`] / [`CurveOnClassifyBrep::brep_mut`] for a face whose B-rep is
    /// absent: an empty B-rep, in which the face lookup fails as a dangling id.
    absent: Brep,
}

impl<'b, P: core::ops::Deref<Target = ArenaPair<'b>>> Objs<P> {
    fn new(two: P, b: i8) -> Self {
        Self {
            two,
            b,
            absent: Brep::new(),
        }
    }
    fn slot(&self) -> Option<usize> {
        usize::try_from(self.b).ok()
    }
}

/// The shared and the mutably lent form of [`Objs`] read the pair the same way.
macro_rules! objs_reads {
    ($t:ty) => {
        impl ClassGeometry<H> for $t {
            fn curve(&self, o: H) -> Option<&SmCurve> {
                ClassGeometry::curve(&*self.two, o)
            }
            fn surface(&self, o: H) -> Option<&Surface> {
                ClassGeometry::surface(&*self.two, o)
            }
            fn volume(&self, o: H) -> Option<&Volume> {
                ClassGeometry::volume(&*self.two, o)
            }
            fn point(&self, o: H) -> Option<Point3> {
                ClassGeometry::point(&*self.two, o)
            }
            // the topology handles are those of the pair, so the classification's topology
            // members (`SmVertex::GetTolerance`, `SmEdge::GetTolerance`, ...) reach the same
            // entities through either adapter
            fn classification_vertex_point(&self, o: H) -> Option<Point3> {
                self.two.classification_vertex_point(o)
            }
            fn topology_zone_tol_3d(&self, o: H) -> Option<f64> {
                self.two.topology_zone_tol_3d(o)
            }
            fn topology_entity(&self, o: H) -> Option<(&Brep, EntityRef)> {
                self.two.topology_entity(o)
            }
            fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<H> {
                self.two.topology_handle_in(brep, entity)
            }
            fn brep_id(&self, brep: &Brep) -> Option<BrepId> {
                self.two.brep_id(brep)
            }
            fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
                self.two.owner_entity(owner)
            }
            fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
                self.two.owner_of(owner)
            }
        }

        impl CurveOnClassifyObjects<H> for $t {
            fn face(&self, f: FaceId) -> H {
                hd(self.b, K_FACE, f.index())
            }
            fn vertex(&self, v: VertexId) -> H {
                hd(self.b, K_VERTEX, v.index())
            }
            fn edge(&self, e: EdgeId) -> H {
                hd(self.b, K_EDGE, e.index())
            }
            fn vertexuse(&self, vu: VertexuseId) -> H {
                hd(self.b, K_VERTEXUSE, vu.index())
            }
            fn edgeuse(&self, eu: EdgeuseId) -> H {
                hd(self.b, K_EDGEUSE, eu.index())
            }
            fn edge_curve(&self, e: EdgeId) -> H {
                hd(self.b, K_EDGE_CURVE, e.index())
            }
            fn trim_curve(&self, eu: EdgeuseId) -> H {
                hd(self.b, K_TRIM_CURVE, eu.index())
            }
            fn face_surface(&self, f: FaceId) -> Option<H> {
                self.two
                    .face_surface_h(hd(self.b, K_FACE, f.index()))
                    .flatten()
            }
            fn loop_id(&self, tgt_loop: H) -> Option<LoopId> {
                (tgt_loop.kind == K_LOOP && tgt_loop.b == self.b)
                    .then(|| LoopId::from_index(tgt_loop.i))
            }
        }
    };
}

objs_reads!(Objs<&ArenaPair<'_>>);
objs_reads!(Objs<&mut ArenaPair<'_>>);

/// The classification's lazy face queries edit the face's B-rep in the pair, as a query
/// (the faces' caches are kept, see `face_caches`).
impl ClassGeometryMut<H> for Objs<&mut ArenaPair<'_>> {
    fn curve_mut(&mut self, o: H) -> Option<&mut SmCurve> {
        ProtoGeometry::curve_mut(&mut *self.two, o)
    }
    fn topology_entity_mut(&mut self, o: H) -> Option<(&mut Brep, EntityRef)> {
        self.two.topology_entity_for_query(o)
    }
    fn topology_entity_for_query(&mut self, o: H) -> Option<(&mut Brep, EntityRef)> {
        self.two.topology_entity_for_query(o)
    }
}

impl FaceBrep for Objs<&mut ArenaPair<'_>> {
    fn brep(&self) -> &Brep {
        match self.slot().and_then(|s| self.two.breps.get(s)?.as_deref()) {
            Some(b) => b,
            None => &self.absent,
        }
    }
}

impl CurveOnClassifyBrep<H> for Objs<&mut ArenaPair<'_>> {
    fn brep_mut(&mut self) -> &mut Brep {
        // a query: the faces' caches are kept (see `face_caches`)
        let slot = usize::try_from(self.b).ok();
        match slot.and_then(|s| self.two.breps.get_mut(s)?.as_deref_mut()) {
            Some(b) => b,
            None => &mut self.absent,
        }
    }
}

impl ProtoBrep<H> for ArenaPair<'_> {
    /// `MergeIntoObject` on the operand the classification's object names, through
    /// [`MergeIntervalBrep::merge_context`].
    fn merge_point_class_into_brep(
        &mut self,
        k: &mut Kernel,
        pc: &mut PointClassification<H>,
        src_point: Point3,
    ) -> crate::Result<MergeOutputs<H>> {
        crate::intersect::curve_class::p6::merge_into_object_mut(k, self, pc, src_point)
    }
    fn edge_interval(&self, edge: H) -> Option<Extent1d> {
        ed::interval(self.brep(edge)?, eid(edge)).ok()
    }
    fn edge_curve(&self, edge: H) -> Option<Option<H>> {
        self.edge_curve_h(edge)
    }
    fn topology_tolerance(&self, topo: H) -> Option<f64> {
        let b = self.brep(topo)?;
        match topo.kind {
            K_VERTEX => vx::tolerance(b, vid(topo)).ok(),
            K_EDGE => ed::tolerance(b, eid(topo)).ok(),
            K_FACE => fp2::tolerance(b, fid(topo)).ok(),
            _ => None,
        }
    }
    fn face_uv_domain(&self, face: H) -> Option<Extent2d> {
        fp2::uv_domain(self.brep(face)?, fid(face)).ok()
    }
    fn face_trim_uv_domain(&mut self, k: &Kernel, face: H) -> crate::Result<Extent2d> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        let b = self.brep_m(face)?;
        b.face(fid(face))
            .map_err(|_| invalid("unknown face handle"))?;
        let mut output = crate::topology::face::p1::UvDomainFromTrim {
            domain: Extent2d::new(),
            max_uv_loop_gap: 0.0,
            second_max_uv_loop_gap: 0.0,
        };
        let result = crate::topology::face::p1::calculate_uv_domain_from_uv_trim_curves_into(
            k,
            b,
            fid(face),
            true,
            false,
            false,
            &mut output,
        );
        if let Err(error) = result {
            if error.is_unported() {
                return Err(error);
            }
        }
        Ok(output.domain)
    }
    fn face_point_classify_uv(
        &mut self,
        k: &Kernel,
        face: H,
        uv: Point2,
        source_tolerance: f64,
    ) -> crate::Result<PointClassType> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        let b = self.brep_m(face)?;
        b.face(fid(face))
            .map_err(|_| invalid("unknown face handle"))?;
        let mut output = PointClassification::new(source_tolerance);
        let result = point_classify(
            k,
            b,
            fid(face),
            uv,
            &mut output,
            PointClassifyOptions {
                src_zone_tol_3d: source_tolerance,
                do_boundary_intersections: true,
                use_uv_classification: true,
                ..PointClassifyOptions::default()
            },
        );
        if let Err(error) = result {
            if error.is_unported() {
                return Err(error);
            }
        }
        Ok(output.point_class())
    }
    fn set_face_uv_domain(&mut self, face: H, domain: Extent2d) -> Option<()> {
        fp2::set_uv_domain(self.brep_mut(face)?, fid(face), &domain).ok()
    }
    fn face_surface(&self, face: H) -> Option<Option<H>> {
        self.face_surface_h(face)
    }
    fn edge_edgeuse_of_face(&self, edge: H, face: H) -> crate::Result<Option<H>> {
        let b = self.brep_r(edge)?;
        let face = (face.b == edge.b && face.kind == K_FACE).then(|| fid(face));
        Ok(ed::edgeuse_of_face(b, eid(edge), face)?.map(|eu| hd(edge.b, K_EDGEUSE, eu.index())))
    }
    fn edge_is_connected_to_face(&self, edge: H, face: H) -> crate::Result<bool> {
        let b = self.brep_r(edge)?;
        let face = (face.b == edge.b && face.kind == K_FACE).then(|| fid(face));
        ed::is_connected_to_face(b, eid(edge), face)
    }
    fn insert_curve(&mut self, curve: SmCurve) -> Option<H> {
        let i = u32::try_from(self.extra.len()).ok()?;
        let hh = curve_h(2, K_EXTRA, i, &curve);
        self.extra.push(curve);
        Some(hh)
    }
    fn vertex_common_edges(&self, v1: H, v2: H) -> crate::Result<Vec<H>> {
        let b = self.brep_r(v1)?;
        let other = (v2.b == v1.b && v2.kind == K_VERTEX).then(|| vid(v2));
        Ok(vx::common_edges(b, vid(v1), other, None)?
            .into_iter()
            .map(|e| hd(v1.b, K_EDGE, e.index()))
            .collect())
    }
    fn brep_point_3d_classify(
        &mut self,
        k: &Kernel,
        brep: H,
        point: Point3,
        tolerance: f64,
        boundary_intersections: bool,
    ) -> crate::Result<PointClassification<H>> {
        let operand = brep.b;
        let id = BrepId::from_index(u32::try_from(operand).map_err(|_| invalid("not an operand"))?);
        let b = self.brep_m(brep)?;
        let query = BrepQuery::of_brep_with_id(id, b);
        let pc = query.point_3d_classify(k, b, point, tolerance, boundary_intersections)?;
        pc.map_objects(&mut |e| {
            Self::entity_h(operand, e)
                .ok_or_else(|| invalid("classification object without a handle"))
        })
    }

    fn face_curve_on_classify(
        &self,
        k: &Kernel,
        face: H,
        do_point_classify: bool,
        cc: &mut CurveClassification<H>,
    ) -> crate::Result<bool> {
        let b = self.brep_r(face)?;
        let objs = Objs::new(self, face.b);
        curve_on_classify(
            k,
            b,
            fid(face),
            &objs,
            do_point_classify,
            cc,
            false,
            false,
            true,
        )
    }

    /// `SmFace::CurveOnClassify(bDoPointClassify, cc)` on the face's own B-rep in the
    /// pair: its lazy edits (UV trim curves, gap caches, tolerance growth) stay on it, as
    /// upstream's on the face's B-rep.
    fn face_curve_on_classify_mut(
        &mut self,
        k: &Kernel,
        face: H,
        do_point_classify: bool,
        cc: &mut CurveClassification<H>,
    ) -> crate::Result<bool> {
        self.brep_r(face)?;
        let mut objs = Objs::new(self, face.b);
        curve_on_classify_mut(
            k,
            fid(face),
            &mut objs,
            do_point_classify,
            cc,
            false,
            false,
            true,
        )
    }

    /// `SmEdgeuse::EvaluateBinormal(t, FALSE, ..)` on the operand's B-rep
    /// ([`crate::topology::edgeuse::p3::evaluate_binormal`]).
    fn edgeuse_evaluate_binormal(
        &mut self,
        k: &Kernel,
        edgeuse: H,
        param: f64,
    ) -> crate::Result<crate::math::Vec3> {
        if edgeuse.kind != K_EDGEUSE {
            return Err(invalid("not an edge use handle"));
        }
        let b = self.brep_m(edgeuse)?;
        Ok(crate::topology::edgeuse::p3::evaluate_binormal(
            k,
            b,
            EdgeuseId::from_index(edgeuse.i),
            param,
            false,
            &crate::topology::edgeuse::p3::BinormalOptions::default(),
        )?
        .binormal)
    }
    /// `SmBrep::MakeEdgeInFace` of `brep`'s B-rep, which checks that operand's own
    /// `m_bEditingEnabled`, through [`MergeIntervalBrep::merge_context`] of the face
    /// ([`crate::topology::proto_topology::p2::make_edge_in_face_in_context`]).
    fn brep_make_edge_in_face(
        &mut self,
        k: &Kernel,
        brep: H,
        face: H,
        start: H,
        end: H,
        curve: SmCurve,
        uv_curve: Option<SmCurve>,
        interval: Extent1d,
        orientation: OrientType,
        tolerance: f64,
    ) -> crate::Result<(Option<H>, Option<H>, Option<H>)> {
        let in_brep = |h: H, kind: u8| brep.kind == K_BREP && h.b == brep.b && h.kind == kind;
        if !in_brep(face, K_FACE) || !in_brep(start, K_VERTEX) || !in_brep(end, K_VERTEX) {
            return Err(invalid("face or vertex is not in the B-rep"));
        }
        let ctx = self
            .merge_context(face)
            .ok_or_else(|| invalid("handle names an absent B-rep"))?;
        crate::topology::proto_topology::p2::make_edge_in_face_in_context(
            k,
            ctx,
            vid(start),
            vid(end),
            curve,
            uv_curve,
            interval,
            orientation,
            tolerance,
        )
    }

    /// `SmBrep::SqueezeEdge(edge, survivor)` of `brep`'s B-rep
    /// ([`crate::topology::brep::p3::squeeze_edge`]), notifying as the glue does.
    fn brep_squeeze_edge(
        &mut self,
        k: &mut Kernel,
        brep: H,
        edge: H,
        survivor: H,
    ) -> crate::Result<()> {
        if brep.kind != K_BREP || edge.b != brep.b || survivor.b != brep.b {
            return Err(invalid("edge or vertex is not in the B-rep"));
        }
        self.with_edit_env(brep, |b, env| {
            crate::topology::brep::p3::squeeze_edge(k, b, env, eid(edge), vid(survivor))
        })
    }
}

/// The B-rep walks over one operand's own entities (as `Brep`'s own impl, through the
/// handles); an unknown handle is an invalid-input error, as a null dereference upstream.
impl ProtoTopology<H> for ArenaPair<'_> {
    fn vertex_use_faces(&self, vertex: H) -> crate::Result<Vec<Option<H>>> {
        let b = self.brep_r(vertex)?;
        let mut out = Vec::new();
        for vu in vx::vertexuses(b, vid(vertex))? {
            let face = match crate::topology::vertexuse::faceuse(b, vu)? {
                Some(fu) => eu1::faceuse_face(b, fu)?,
                None => None,
            };
            out.push(face.map(|f| hd(vertex.b, K_FACE, f.index())));
        }
        Ok(out)
    }
    fn edge_use_faces(&self, edge: H) -> crate::Result<Vec<Option<H>>> {
        let b = self.brep_r(edge)?;
        let mut out = Vec::new();
        for eu in ed::edgeuses(b, eid(edge), None, None)? {
            out.push(eu1::face(b, eu)?.map(|f| hd(edge.b, K_FACE, f.index())));
        }
        Ok(out)
    }
    fn edge_start_vertex(&self, edge: H) -> crate::Result<Option<H>> {
        Ok(ed::start_vertex(self.brep_r(edge)?, eid(edge))?
            .map(|v| hd(edge.b, K_VERTEX, v.index())))
    }
    fn edge_other_vertex(&self, edge: H, vertex: Option<H>) -> crate::Result<Option<H>> {
        let b = self.brep_r(edge)?;
        let v = vertex
            .filter(|v| v.b == edge.b && v.kind == K_VERTEX)
            .map(vid);
        Ok(ed::other_vertex(b, eid(edge), v)?.map(|v| hd(edge.b, K_VERTEX, v.index())))
    }
    fn vertex_point(&self, vertex: H) -> crate::Result<Point3> {
        vx::point(self.brep_r(vertex)?, vid(vertex))
    }
    fn edge_brep(&self, edge: H) -> crate::Result<Option<ObjectKey>> {
        Ok(eu1::edge_brep(self.brep_r(edge)?, eid(edge))?.map(Brep::key))
    }
    fn edge_faces(&self, edge: H) -> crate::Result<Vec<H>> {
        let b = self.brep_r(edge)?;
        Ok(ed::faces(b, eid(edge), None)?
            .into_iter()
            .map(|f| hd(edge.b, K_FACE, f.index()))
            .collect())
    }
}

impl IntersectorBrep<H> for ArenaPair<'_> {
    fn face_point_3d_classify(
        &mut self,
        k: &Kernel,
        face: H,
        point: Point3,
        src_zone_tol_3d: f64,
        do_boundary_intersections: bool,
        pc: &mut PointClassification<H>,
        opt_guess_uv: Option<Point2>,
    ) -> crate::Result<()> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        // the caller's classification in the face's B-rep entities (its other members
        // are kept unless written, as upstream's out-param)
        let mut pe = pc.map_objects(&mut |h: H| {
            if h.b != face.b {
                return Err(invalid("classification object of the other B-rep"));
            }
            self.topology_entity(h)
                .map(|(_, e)| e)
                .ok_or_else(|| invalid("classification object without a B-rep entity"))
        })?;
        let r = crate::query::face::p1::point_3d_classify(
            k,
            self.brep_m(face)?,
            fid(face),
            point,
            src_zone_tol_3d,
            do_boundary_intersections,
            &mut pe,
            crate::query::face::p1::Point3dClassifyOptions {
                guess_uv: opt_guess_uv,
                ..Default::default()
            },
        );
        // copied back whatever the status
        *pc = pe.map_objects(&mut |e: EntityRef| {
            Self::entity_h(face.b, e).ok_or_else(|| invalid("entity without a handle"))
        })?;
        r
    }

    fn make_vertex_loop(&mut self, brep: H, face: H, point: Point3) -> crate::Result<(H, H)> {
        if brep.kind != K_BREP || face.kind != K_FACE || brep.b != face.b {
            return Err(invalid("vertex loop owner mismatch"));
        }
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        self.clear_face_caches(slot);
        let b = self
            .breps
            .get_mut(slot)
            .and_then(Option::as_deref_mut)
            .ok_or_else(|| invalid("absent B-rep"))?;

        let mut env = crate::topology::edge::NotifyEnv {
            records: self.records,
            attributes: self.attributes,
            context: Some(if brep.b == 1 {
                self.other_context
                    .as_deref_mut()
                    .unwrap_or(self.primary_context)
            } else {
                self.primary_context
            }),
            key_of: &|e| self.keys.object(brep.b == 1, e),
        };
        let (loop_, vertex) = crate::topology::brep::p9::make_vertex_loop(
            b,
            fid(face),
            point,
            OrientType::Opposite,
            &mut env,
        )?;
        Ok((
            hd(brep.b, K_LOOP, loop_.index()),
            hd(brep.b, K_VERTEX, vertex.index()),
        ))
    }
    fn make_vertex_split_edge(
        &mut self,
        k: &mut Kernel,
        brep: H,
        edge: H,
        parameter: f64,
    ) -> crate::Result<(H, H, H)> {
        if brep.kind != K_BREP || edge.kind != K_EDGE || brep.b != edge.b {
            return Err(invalid("split edge owner mismatch"));
        }
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        self.clear_face_caches(slot);
        let b = self
            .breps
            .get_mut(slot)
            .and_then(Option::as_deref_mut)
            .ok_or_else(|| invalid("absent B-rep"))?;

        let mut env = crate::topology::edge::NotifyEnv {
            records: self.records,
            attributes: self.attributes,
            context: Some(if brep.b == 1 {
                self.other_context
                    .as_deref_mut()
                    .unwrap_or(self.primary_context)
            } else {
                self.primary_context
            }),
            key_of: &|e| self.keys.object(brep.b == 1, e),
        };
        let checked = |e, kind| self.keys.curve(brep.b == 1, e, kind);
        let identity = if self.keys.actual_curve_identities() {
            crate::topology::brep::p9::CurveIdentity::Actual(
                &crate::topology::brep::p9::issue_curve_identity,
            )
        } else {
            crate::topology::brep::p9::CurveIdentity::Checked(&checked)
        };
        let out = crate::topology::brep::p9::make_vertex_split_edge_with_curve_identity_state(
            k,
            b,
            eid(edge),
            parameter,
            &mut crate::topology::edge::NotifyState::from_env(&mut env),
            identity,
        )?;
        if !out.split {
            return Err(SolidError::sm_err("boolean::merge::p4::arena_pair::split"));
        }
        let [a, z] = out
            .edges
            .ok_or_else(|| invalid("successful split without edges"))?;
        let v = out
            .vertex
            .ok_or_else(|| invalid("successful split without vertex"))?;
        Ok((
            hd(brep.b, K_EDGE, a.index()),
            hd(brep.b, K_EDGE, z.index()),
            hd(brep.b, K_VERTEX, v.index()),
        ))
    }

    fn brep_edges(&self, brep: H) -> crate::Result<Vec<H>> {
        let b = self.brep_r(brep)?;
        Ok(bp1::edges(b, None)?
            .into_iter()
            .map(|e| hd(brep.b, K_EDGE, e.index()))
            .collect())
    }
    fn brep_vertices(&self, brep: H) -> crate::Result<Vec<H>> {
        let b = self.brep_r(brep)?;
        Ok(bp1::vertices(b, None)?
            .into_iter()
            .map(|v| hd(brep.b, K_VERTEX, v.index()))
            .collect())
    }
    fn face_calculate_bounding_box(
        &self,
        k: &Kernel,
        face: H,
        tight: bool,
    ) -> crate::Result<Extent3d> {
        let b = self.brep_r(face)?;
        crate::query::face::p1::calculate_bounding_box(k, b, fid(face), tight)
    }
    fn vertex_update_edge_intervals(&mut self, k: &Kernel, vertex: H) -> crate::Result<()> {
        if vertex.kind != K_VERTEX {
            return Err(invalid("not a vertex handle"));
        }
        self.with_edit_env(brep_h(vertex.b), |b, env| {
            crate::topology::brep::p1::update_vertex_edge_intervals(k, b, env, vid(vertex))
        })
    }
    fn vertex_brep(&self, vertex: H) -> crate::Result<Option<H>> {
        let b = self.brep_r(vertex)?;
        Ok(vx::brep(b, vid(vertex))?.map(|_| brep_h(vertex.b)))
    }
    fn edge_brep_handle(&self, edge: H) -> crate::Result<Option<H>> {
        let b = self.brep_r(edge)?;
        Ok(ed::brep(b, eid(edge))?.map(|_| brep_h(edge.b)))
    }
    fn set_vertex_point(&mut self, _k: &Kernel, vertex: H, point: Point3) -> crate::Result<()> {
        let slot = usize::try_from(vertex.b).map_err(|_| invalid("bad B-rep"))?;
        self.clear_face_caches(slot);
        let b = self
            .breps
            .get_mut(slot)
            .and_then(Option::as_deref_mut)
            .ok_or_else(|| invalid("absent B-rep"))?;
        let keys = self.keys;
        let this = keys.object(vertex.b == 1, EntityRef::Vertex(vid(vertex)));
        vx::set_point(
            b,
            vid(vertex),
            point,
            self.records,
            self.attributes,
            Some(if vertex.b == 1 {
                self.other_context
                    .as_deref_mut()
                    .unwrap_or(self.primary_context)
            } else {
                self.primary_context
            }),
            this,
            &|vu| keys.object(vertex.b == 1, EntityRef::Vertexuse(vu)),
        )
    }
    fn set_topology_tolerance(
        &mut self,
        _k: &Kernel,
        topo: H,
        tolerance: f64,
        update_only_if_larger: bool,
        cascade_to_boundaries: bool,
    ) -> crate::Result<()> {
        let b = self.brep_m(topo)?;
        match topo.kind {
            K_VERTEX => vx::set_tolerance(
                b,
                vid(topo),
                tolerance,
                update_only_if_larger,
                cascade_to_boundaries,
            ),
            K_EDGE => ed::set_tolerance(
                b,
                eid(topo),
                tolerance,
                update_only_if_larger,
                cascade_to_boundaries,
            ),
            _ => Err(invalid("SetTolerance of a non-vertex, non-edge")),
        }
    }
    fn update_object_tolerance(
        &mut self,
        k: &Kernel,
        topo: H,
        this_gap: f64,
        this_tol: f64,
        check_gaps: bool,
    ) -> crate::Result<()> {
        let target = match topo.kind {
            K_VERTEX => TopologyToUpdate::Vertex(vid(topo)),
            K_EDGE => TopologyToUpdate::Edge(eid(topo)),
            K_FACE => TopologyToUpdate::Face(fid(topo)),
            _ => return Err(invalid("UpdateObjectTolerance of a non-topology object")),
        };
        let b = self.brep_m(topo)?;
        update_object_tolerance(k, b, Some(target), this_gap, this_tol, check_gaps)
    }
    fn vertex_is_shell_vertex(&self, vertex: H) -> crate::Result<bool> {
        vx::is_shell_vertex(self.brep_r(vertex)?, vid(vertex))
    }
    fn edge_is_wire(&self, edge: H) -> crate::Result<bool> {
        ed::is_wire(self.brep_r(edge)?, eid(edge))
    }
    fn edge_is_lamina(&self, edge: H) -> crate::Result<bool> {
        ed::is_lamina(self.brep_r(edge)?, eid(edge))
    }
    fn edge_vertex(&self, edge: H) -> crate::Result<Option<H>> {
        Ok(ed::vertex(self.brep_r(edge)?, eid(edge))?.map(|v| hd(edge.b, K_VERTEX, v.index())))
    }
    fn face_brep(&self, face: H) -> crate::Result<Option<H>> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        let b = self.brep_r(face)?;
        Ok(crate::topology::face::p4::brep(b, fid(face))?.map(|_| brep_h(face.b)))
    }
    fn face_vertices(&self, face: H) -> crate::Result<Vec<H>> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        let vs =
            crate::topology::face::p4::vertices(self.brep_r(face)?, fid(face), None, false, None)?;
        Ok(vs
            .into_iter()
            .map(|v| hd(face.b, K_VERTEX, v.index()))
            .collect())
    }
    fn face_edges(&self, face: H) -> crate::Result<Vec<H>> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        let es =
            crate::topology::face::p4::edges(self.brep_r(face)?, fid(face), None, false, None)?;
        Ok(es
            .into_iter()
            .map(|e| hd(face.b, K_EDGE, e.index()))
            .collect())
    }
    fn edge_primary_edgeuse_orientation(&self, edge: H) -> crate::Result<Option<OrientType>> {
        let b = self.brep_r(edge)?;
        match eu1::primary_edgeuse(b, eid(edge))? {
            Some(eu) => Ok(Some(eu1::orientation(b, eu)?)),
            None => Ok(None),
        }
    }
    fn edge_bounding_box(&self, k: &Kernel, edge: H) -> crate::Result<Extent3d> {
        let b = self.brep_r(edge)?;
        let boxes = crate::query::edge::calculate_bounding_box(
            k,
            b,
            eid(edge),
            BoundingBoxRequest {
                normal: true,
                ..BoundingBoxRequest::default()
            },
            false,
        )?;
        boxes.normal.ok_or_else(|| invalid("no normal box"))
    }
    fn face_uv_domain_from_uv_trim_curves(
        &mut self,
        k: &Kernel,
        face: H,
    ) -> crate::Result<Extent2d> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        let b = self.brep_m(face)?;
        Ok(
            crate::topology::face::p1::calculate_uv_domain_from_uv_trim_curves(
                k,
                b,
                fid(face),
                true,
                false,
                false,
            )?
            .domain,
        )
    }
    fn face_point_classify(
        &mut self,
        k: &Kernel,
        face: H,
        uv: Point2,
        pc: &mut PointClassification<H>,
    ) -> crate::Result<()> {
        // (mutable: the classification may create UV trim curves, as upstream)
        let b = self.brep_m(face)?;
        let mut pe: PointClassification<EntityRef> = PointClassification::new(pc.tolerance());
        let r = point_classify(
            k,
            b,
            fid(face),
            uv,
            &mut pe,
            PointClassifyOptions::default(),
        );
        // copy the classification back whatever the status (upstream's out-param)
        pc.set_class_object(
            pe.point_class(),
            pe.object().and_then(|e| Self::entity_h(face.b, e)),
        );
        pc.set_face(pe.face().and_then(|e| Self::entity_h(face.b, e)));
        pc.set_gap_3d(pe.deviation());
        pc.set_pre_snap_param(pe.pre_snap_param());
        if pe.are_parameters_set() {
            pc.set_t_param(pe.t_param(), false);
            pc.set_uv_param(pe.uv_param());
        } else {
            pc.unset_parameters();
        }
        r
    }
    fn face_intersect(
        &mut self,
        k: &Kernel,
        face1: H,
        face2: H,
        approx_tol_3d: f64,
        ang_tol_deg: f64,
        wants: ti::FaceIntersectWants,
        opt_coincidence_flag: Option<bool>,
    ) -> crate::Result<ti::FaceIntersection<H>> {
        if face1.kind != K_FACE || face2.kind != K_FACE {
            return Err(invalid("FaceIntersect needs two faces"));
        }
        let options = FaceIntersectOptions {
            want_this_classes: true,
            want_other_classes: true,
            want_cc_curves: true,
            want_this_uv_curves: wants.this_uv_curves,
            want_other_uv_curves: wants.other_uv_curves,
            want_points: wants.points_3d,
            coincidence: opt_coincidence_flag,
        };
        // the faces may be of the two operands (`face_intersect_breps`)
        // the faces' caches are made on their B-rep(s) (smsurf_GetSurfaceCache)
        // (the faces' caches are kept for the next face intersection on the same
        // B-reps: the intersection only adds the faces' UV trim curves)
        let (i, j) = (
            usize::try_from(face1.b).map_err(|_| invalid("handle names an absent B-rep"))?,
            usize::try_from(face2.b).map_err(|_| invalid("handle names an absent B-rep"))?,
        );
        let [first, second] = &mut self.breps;
        let [first_caches, second_caches] = &mut self.face_caches;
        let (breps, face_caches) = match (i, j) {
            (0, 0) => (
                first.as_deref_mut().map(fp3::FaceBreps::One),
                [Some(first_caches), None],
            ),
            (1, 1) => (
                second.as_deref_mut().map(fp3::FaceBreps::One),
                [Some(second_caches), None],
            ),
            (0, 1) => (
                first
                    .as_deref_mut()
                    .zip(second.as_deref_mut())
                    .map(|(t, o)| fp3::FaceBreps::Two(t, o)),
                [Some(first_caches), Some(second_caches)],
            ),
            (1, 0) => (
                second
                    .as_deref_mut()
                    .zip(first.as_deref_mut())
                    .map(|(t, o)| fp3::FaceBreps::Two(t, o)),
                [Some(second_caches), Some(first_caches)],
            ),
            _ => (None, [None, None]),
        };
        let breps = breps.ok_or_else(|| invalid("handle names an absent B-rep"))?;
        let out = fp3::face_intersect_breps_cached(
            k,
            breps,
            fid(face1),
            fid(face2),
            approx_tol_3d,
            ang_tol_deg,
            &options,
            face_caches,
        )?;
        // the B-rep of a classification object of `side` (see `IntersectObject`)
        let brep_of = |side: bool| if side { face2.b } else { face1.b };

        // every curve the call made gets a handle in the curve store
        let mut curves_3d = Vec::new();
        for c in out.curves_3d {
            curves_3d.push(self.register_curve(c)?);
        }
        let cc3d: Vec<Option<H>> = self.register_curves(
            out.cc_curves_3d
                .map(|v| v.into_iter().map(Some).collect())
                .unwrap_or_default(),
        )?;
        let uv1 = self.register_curves(out.cc_this_uv_curves.unwrap_or_default())?;
        let uv2 = self.register_curves(out.cc_other_uv_curves.unwrap_or_default())?;
        let this_uv_curves = self.register_curves(out.this_uv_curves.unwrap_or_default())?;
        let other_uv_curves = self.register_curves(out.other_uv_curves.unwrap_or_default())?;

        // the classifications name their objects by handle
        let mut map = |o: IntersectObject| -> crate::Result<H> {
            let bad = || invalid("classification object without a handle");
            match o {
                IntersectObject::Curve(i) => cc3d.get(i).copied().flatten().ok_or_else(bad),
                IntersectObject::UvCurve(i, false) => uv1.get(i).copied().flatten().ok_or_else(bad),
                IntersectObject::UvCurve(i, true) => uv2.get(i).copied().flatten().ok_or_else(bad),
                IntersectObject::Surface(f, side) => {
                    Ok(hd(brep_of(side), K_FACE_SURFACE, f.index()))
                }
                IntersectObject::EdgeCurve(e, side) => self
                    .edge_curve_h(hd(brep_of(side), K_EDGE, e.index()))
                    .flatten()
                    .ok_or_else(bad),
                IntersectObject::Trim(eu, side) => Ok(hd(brep_of(side), K_TRIM_CURVE, eu.index())),
                IntersectObject::Topology(e, side) => match e {
                    EntityRef::Vertexuse(vu) => Ok(hd(brep_of(side), K_VERTEXUSE, vu.index())),
                    other => Self::entity_h(brep_of(side), other).ok_or_else(bad),
                },
                // face intersections name no free surface
                IntersectObject::FreeSurface(_) => Err(bad()),
            }
        };
        let mut cc1 = Vec::new();
        for c in out.this_classes.unwrap_or_default() {
            cc1.push(c.map_objects(&mut map)?);
        }
        let mut cc2 = Vec::new();
        for c in out.other_classes.unwrap_or_default() {
            cc2.push(c.map_objects(&mut map)?);
        }
        Ok(ti::FaceIntersection {
            curves_3d,
            cc1,
            cc2,
            cc_curves: [
                cc3d.into_iter().flatten().collect(),
                uv1.into_iter().flatten().collect(),
                uv2.into_iter().flatten().collect(),
            ],
            this_uv_curves: this_uv_curves.into_iter().flatten().collect(),
            other_uv_curves: other_uv_curves.into_iter().flatten().collect(),
            points_3d: out.points_3d.unwrap_or_default(),
        })
    }
    fn editing_enabled(&self, brep: H) -> crate::Result<bool> {
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        if brep.kind != K_BREP {
            return Err(invalid("not a B-rep handle"));
        }
        self.breps
            .get(slot)
            .and_then(Option::as_ref)
            .map(|b| b.editing_enabled())
            .ok_or_else(|| invalid("not a B-rep handle"))
    }
    fn brep_tolerance(&self, brep: H) -> crate::Result<f64> {
        Ok(self.brep_r(brep)?.tolerance())
    }
    fn set_editing_enabled(&mut self, brep: H, enabled: bool) -> crate::Result<bool> {
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        if brep.kind != K_BREP {
            return Err(invalid("not a B-rep handle"));
        }
        self.clear_face_caches(slot);
        let b = self
            .breps
            .get_mut(slot)
            .and_then(Option::as_deref_mut)
            .ok_or_else(|| invalid("not a B-rep handle"))?;
        let old = b.editing_enabled();
        b.set_editing_enabled(enabled);
        Ok(old)
    }
    fn topology_objects(&self) -> &dyn TopologyObjects<H> {
        self
    }
    fn face_cache_target(&mut self, face: H) -> crate::Result<ti::FaceCacheTarget<'_, H>> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        let b = face.b;
        let surface = self
            .face_surface_h(face)
            .flatten()
            .ok_or(SolidError::null_pointer(OP_FACE_CACHE))?;
        Ok(ti::FaceCacheTarget {
            brep: self.brep_m(face)?,
            face: fid(face),
            surface,
            handles: Box::new(move |e| cache_entity_h(b, e)),
            entities: Box::new(move |h| cache_h_entity(b, h)),
        })
    }
    fn face_entity_handles(&self, face: H) -> crate::Result<ti::EntityHandles<H>> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        let b = face.b;
        Ok(Box::new(move |e| cache_entity_h(b, e)))
    }
    fn brep_arena(&self, brep: H) -> crate::Result<&Brep> {
        if brep.kind != K_BREP {
            return Err(invalid("not a B-rep handle"));
        }
        self.brep_r(brep)
    }
    fn brep_glue_edges_general(
        &mut self,
        k: &mut Kernel,
        brep: H,
        surviving: Option<H>,
        relative_orientation: OrientType,
        edge_gap: f64,
        do_region_nesting: bool,
        to_delete: Option<H>,
        inside_faceuses: Option<&[H]>,
    ) -> crate::Result<()> {
        if brep.kind != K_BREP {
            return Err(invalid("not a B-rep handle"));
        }
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        let edge = |h: Option<H>| -> crate::Result<Option<EdgeId>> {
            match h {
                None => Ok(None),
                Some(h) if h.kind == K_EDGE && h.b == brep.b => Ok(Some(eid(h))),
                Some(_) => Err(invalid("not an edge of this B-rep")),
            }
        };
        let (surviving, to_delete) = (edge(surviving)?, edge(to_delete)?);
        // (face-use handles are not accepted here: the only caller passes NULL)
        let inside: Option<&[FaceuseId]> = match inside_faceuses {
            None => None,
            Some([]) => Some(&[]),
            Some(_) => return Err(invalid("face-use handles are not modelled")),
        };
        self.clear_face_caches(slot);
        let breps = &mut self.breps;
        let b = breps
            .get_mut(slot)
            .and_then(Option::as_deref_mut)
            .ok_or_else(|| invalid("handle names an absent B-rep"))?;

        let mut notify = ed::NotifyEnv {
            records: self.records,
            attributes: self.attributes,
            context: Some(if brep.b == 1 {
                self.other_context
                    .as_deref_mut()
                    .unwrap_or(self.primary_context)
            } else {
                self.primary_context
            }),
            key_of: &|e| self.keys.object(brep.b == 1, e),
        };
        p7::glue_edges_general(
            k,
            b,
            surviving,
            relative_orientation,
            edge_gap,
            do_region_nesting,
            to_delete,
            inside,
            &mut notify,
        )
    }

    /// `SmBrep::DeleteFace(pFace)` with its defaults (`FALSE, TRUE, NULL, NULL`):
    /// [`crate::topology::brep::p3::delete_face`], notifying as the glue does.
    fn brep_delete_face(&mut self, k: &mut Kernel, brep: H, face: H) -> crate::Result<()> {
        if brep.kind != K_BREP || face.kind != K_FACE || face.b != brep.b {
            return Err(invalid("face is not in the B-rep"));
        }
        self.with_edit_env(brep, |b, env| {
            let mut marks = crate::topology::topology_traverser::Marks::default();
            crate::topology::brep::p3::delete_face(
                k,
                b,
                env,
                &mut marks,
                fid(face),
                crate::topology::brep::p3::DeleteFaceOptions::default(),
            )
            .map(drop)
        })
    }

    /// `SmBrep::DeleteEdge(pEdge)` (`pOptSurfaceToKeep = NULL`):
    /// [`crate::topology::brep::p4::delete_edge`], notifying as the face deletion does.
    fn brep_delete_edge(&mut self, k: &mut Kernel, brep: H, edge: H) -> crate::Result<()> {
        if brep.kind != K_BREP || edge.kind != K_EDGE || edge.b != brep.b {
            return Err(invalid("edge is not in the B-rep"));
        }
        self.with_edit_env(brep, |b, env| {
            let mut delete = crate::topology::brep::p4::DeleteEnv::from_notify(env);
            crate::topology::brep::p4::delete_edge(k, b, &mut delete, eid(edge), None)
        })
    }

    /// `SmBrep::DeleteVertex(pVertex)`: [`crate::topology::brep::p4::delete_vertex`],
    /// notifying as the face deletion does.
    fn brep_delete_vertex(&mut self, k: &mut Kernel, brep: H, vertex: H) -> crate::Result<()> {
        if brep.kind != K_BREP || vertex.kind != K_VERTEX || vertex.b != brep.b {
            return Err(invalid("vertex is not in the B-rep"));
        }
        self.with_edit_env(brep, |b, env| {
            let mut delete = crate::topology::brep::p4::DeleteEnv::from_notify(env);
            crate::topology::brep::p4::delete_vertex(k, b, &mut delete, vid(vertex))
        })
    }
}

impl ArenaPair<'_> {
    /// Runs the edit `f` of `brep`'s B-rep with the adapter's records and attributes,
    /// notifying through that B-rep's context under its notification keys (as
    /// `brep_glue_edges_general`).
    fn with_edit_env<R>(
        &mut self,
        brep: H,
        f: impl FnOnce(&mut Brep, &mut ed::NotifyEnv<'_>) -> crate::Result<R>,
    ) -> crate::Result<R> {
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        self.clear_face_caches(slot);
        let b = self
            .breps
            .get_mut(slot)
            .and_then(Option::as_deref_mut)
            .ok_or_else(|| invalid("handle names an absent B-rep"))?;
        let mut env = ed::NotifyEnv {
            records: self.records,
            attributes: self.attributes,
            context: Some(if brep.b == 1 {
                self.other_context
                    .as_deref_mut()
                    .unwrap_or(self.primary_context)
            } else {
                self.primary_context
            }),
            key_of: &|e| self.keys.object(brep.b == 1, e),
        };
        f(b, &mut env)
    }
}

impl TopologyObjects<H> for ArenaPair<'_> {
    /// Build the cache using the operand's actual kernel settings and geometry
    /// owner identities. Cache handles remain local to this pair and operation.
    fn brep_cache(&self, brep: H) -> crate::Result<&BrepCache<H>> {
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        let cell = self.caches.get(slot).ok_or_else(|| invalid("bad B-rep"))?;
        if let Some(cache) = cell.get() {
            return Ok(cache);
        }
        let b = self
            .breps
            .get(slot)
            .and_then(Option::as_deref)
            .ok_or_else(|| invalid("handle names an absent B-rep"))?;
        let bi = brep.b;
        let mut cache = BrepCache::new(BREP_TYPE)?;
        cache.build_trees(
            self.cache_kernels
                .get(slot)
                .ok_or_else(|| invalid("bad B-rep cache context"))?,
            CacheSource::Brep(b),
            |o| match o {
                CacheObject::Vertex(v) => hd(bi, K_VERTEX, v.index()),
                CacheObject::Edge(e) => hd(bi, K_EDGE, e.index()),
                CacheObject::Face(f) => hd(bi, K_FACE, f.index()),
                CacheObject::Curve(e) => b
                    .edge(e)
                    .ok()
                    .and_then(|x| x.curve.as_ref())
                    .map_or(hd(bi, K_EDGE_CURVE, e.index()), |c| {
                        curve_h(bi, K_EDGE_CURVE, e.index(), c)
                    }),
                CacheObject::Surface(f) => hd(bi, K_FACE_SURFACE, f.index()),
                CacheObject::Brep => brep_h(bi),
                _ => hd(bi, K_NULL, 0),
            },
            &|owner| {
                let candidate = match owner {
                    CacheObject::Edge(edge) => Some((
                        EntityRef::Edge(edge),
                        self.keys.object(bi == 1, EntityRef::Edge(edge)),
                        b.edge(edge)
                            .ok()
                            .and_then(|e| e.curve.as_ref())
                            .and_then(SmCurve::owner),
                    )),
                    CacheObject::Face(face) => Some((
                        EntityRef::Face(face),
                        self.keys.object(bi == 1, EntityRef::Face(face)),
                        b.face(face)
                            .ok()
                            .and_then(|f| f.surface.as_ref())
                            .and_then(Surface::owner),
                    )),
                    _ => None,
                };
                match candidate {
                    Some((_, expected, actual)) if actual == Some(expected) => expected,
                    Some((entity, expected, actual)) => {
                        let proven = b.geometry_owner_key(entity);
                        if let Some(proven) = proven {
                            if actual == Some(proven) {
                                return proven;
                            }
                        }
                        if matches!(self.keys, NotificationKeys::Caller(_)) {
                            proven.unwrap_or(expected)
                        } else {
                            arena_identity(owner)
                        }
                    }
                    None => arena_identity(owner),
                }
            },
        )?;
        let _ = cell.set(cache);
        cell.get().ok_or_else(|| invalid("B-rep cache not stored"))
    }
    fn curve(&self, object: H) -> crate::Result<&SmCurve> {
        ClassGeometry::curve(self, object).ok_or_else(|| invalid("not a curve handle"))
    }
    fn analytic_curve_handle(&self, _curve: H) -> Option<H> {
        None
    }
    fn vertex_point(&self, vertex: H) -> crate::Result<Point3> {
        vx::point(self.brep_r(vertex)?, vid(vertex))
    }
    fn vertex_tolerance(&self, vertex: H) -> crate::Result<f64> {
        vx::tolerance(self.brep_r(vertex)?, vid(vertex))
    }
    fn curve_edge(&self, curve: H) -> crate::Result<Option<H>> {
        Ok((curve.kind == K_EDGE_CURVE).then_some(hd(curve.b, K_EDGE, curve.i)))
    }
    fn edge_curve(&self, edge: H) -> crate::Result<Option<H>> {
        self.edge_curve_h(edge)
            .ok_or_else(|| invalid("not an edge handle"))
    }
    fn edge_interval(&self, edge: H) -> crate::Result<Extent1d> {
        ed::interval(self.brep_r(edge)?, eid(edge))
    }
    fn edge_tolerance(&self, edge: H) -> crate::Result<f64> {
        ed::tolerance(self.brep_r(edge)?, eid(edge))
    }
    fn surface_face(&self, surface: H) -> crate::Result<Option<H>> {
        Ok((surface.kind == K_FACE_SURFACE).then_some(hd(surface.b, K_FACE, surface.i)))
    }
    fn face_surface(&self, face: H) -> crate::Result<H> {
        self.face_surface_h(face)
            .flatten()
            .ok_or_else(|| SolidError::null_pointer("boolean::merge::p4::arena_pair::face_surface"))
    }
    fn surface(&self, surface: H) -> crate::Result<&Surface> {
        ClassGeometry::surface(self, surface).ok_or_else(|| invalid("not a surface handle"))
    }
    fn face_uv_domain(&self, face: H) -> crate::Result<Extent2d> {
        fp2::uv_domain(self.brep_r(face)?, fid(face))
    }
    fn face_tolerance(&self, face: H) -> crate::Result<f64> {
        fp2::tolerance(self.brep_r(face)?, fid(face))
    }
    fn face_entity(&self, face: H) -> Option<(&Brep, FaceId)> {
        if face.kind != K_FACE {
            return None;
        }
        Some((self.brep_r(face).ok()?, fid(face)))
    }
    fn surface_owner(&self, surface: H) -> crate::Result<Option<H>> {
        TopologyObjects::surface_face(self, surface)
    }
    fn shape_member(&self, _object: H) -> ShapeMember {
        ShapeMember::Other
    }
}

impl IntersectorBrepP2<H> for ArenaPair<'_> {
    fn brep_faces(&self, brep: H) -> crate::Result<Vec<H>> {
        let b = self.brep_r(brep)?;
        Ok(bp1::faces(b, None)?
            .into_iter()
            .map(|f| hd(brep.b, K_FACE, f.index()))
            .collect())
    }
    fn brep_combine_coincident_vertices(
        &mut self,
        k: &mut Kernel,
        brep: H,
        surviving: H,
        to_delete: H,
    ) -> crate::Result<Option<H>> {
        if brep.kind != K_BREP {
            return Err(invalid("not a B-rep handle"));
        }
        let vertex = |h: H| -> crate::Result<VertexId> {
            if h.kind == K_VERTEX && h.b == brep.b {
                Ok(vid(h))
            } else {
                Err(invalid("not a vertex of this B-rep"))
            }
        };
        let (surviving, to_delete) = (vertex(surviving)?, vertex(to_delete)?);
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        self.clear_face_caches(slot);
        let breps = &mut self.breps;
        let b = breps
            .get_mut(slot)
            .and_then(Option::as_deref_mut)
            .ok_or_else(|| invalid("handle names an absent B-rep"))?;
        let mut notify = ed::NotifyEnv {
            records: self.records,
            attributes: self.attributes,
            context: Some(if brep.b == 1 {
                self.other_context
                    .as_deref_mut()
                    .unwrap_or(self.primary_context)
            } else {
                self.primary_context
            }),
            key_of: &|e| self.keys.object(brep.b == 1, e),
        };
        let mut deleted_edge = None;
        crate::topology::brep::p5::combine_with_notifications(
            k,
            b,
            surviving,
            to_delete,
            &mut deleted_edge,
            Some(&mut notify),
        )?;
        Ok(deleted_edge.map(|e| hd(brep.b, K_EDGE, e.index())))
    }
    fn edge_normalized_evaluate(&mut self, k: &Kernel, edge: H, t: f64) -> crate::Result<Point3> {
        if edge.kind != K_EDGE {
            return Err(invalid("not an edge handle"));
        }
        let b = self.brep_m(edge)?;
        let eu = eu1::primary_edgeuse(b, eid(edge))?
            .ok_or_else(|| invalid("edge without primary edge use"))?;
        // (pOptFirstDeriv NULL, bOKToMakeUVTrimCurve TRUE by default)
        Ok(eu3::normalized_evaluate(k, b, eu, t, false, false, true)?.point)
    }
    fn vertex_edges(&self, vertex: H) -> crate::Result<Vec<H>> {
        let b = self.brep_r(vertex)?;
        Ok(vx::edges(b, vid(vertex), None)?
            .into_iter()
            .map(|e| hd(vertex.b, K_EDGE, e.index()))
            .collect())
    }
    fn vertex_faces(&self, vertex: H) -> crate::Result<Vec<H>> {
        let b = self.brep_r(vertex)?;
        Ok(vx::faces(b, vid(vertex))?
            .into_iter()
            .map(|f| hd(vertex.b, K_FACE, f.index()))
            .collect())
    }
}

/// The mutable access the classifications' imprinting (`MergeInterval`,
/// `MergePointClass`) edits the B-reps through: the curve or the B-rep and entity behind a
/// handle.
impl ClassGeometryMut<H> for ArenaPair<'_> {
    fn curve_mut(&mut self, o: H) -> Option<&mut SmCurve> {
        ProtoGeometry::curve_mut(self, o)
    }
    fn topology_entity_mut(&mut self, o: H) -> Option<(&mut Brep, EntityRef)> {
        let (_, entity) = ClassGeometry::topology_entity(self, o)?;
        Some((self.brep_mut(o)?, entity))
    }
    fn topology_entity_for_query(&mut self, o: H) -> Option<(&mut Brep, EntityRef)> {
        // a query keeps the faces' caches (see `face_caches`)
        let (_, entity) = ClassGeometry::topology_entity(self, o)?;
        let slot = usize::try_from(o.b).ok()?;
        Some((self.breps.get_mut(slot)?.as_deref_mut()?, entity))
    }
}

/// The mutable context of a topology handle's B-rep: its arena, the entity, the handles
/// of the entities an edit makes, and the pair's notification records, attributes and
/// identities. An edit deletes the B-rep's cache (`SmBrep::Notify` `PRE_EDIT`); the next
/// use rebuilds it.
impl MergeIntervalBrep<H> for ArenaPair<'_> {
    fn merge_context(&mut self, o: H) -> Option<MergeContext<'_, H>> {
        let target = match o.kind {
            K_VERTEX => EntityRef::Vertex(vid(o)),
            K_EDGE => EntityRef::Edge(eid(o)),
            K_FACE => EntityRef::Face(fid(o)),
            K_REGION => EntityRef::Region(RegionId::from_index(o.i)),
            _ => return None,
        };
        let slot = usize::try_from(o.b).ok()?;
        self.caches.get_mut(slot)?.take();
        self.clear_face_caches(slot);
        let brep = self.breps.get_mut(slot)?.as_deref_mut()?;
        let key_of: &dyn Fn(EntityRef) -> ObjectRef = self.identities.objects.get(slot)?.as_ref();
        let curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef =
            self.identities.curves.get(slot)?.as_ref();
        let context: &mut Context = match self.other_context.as_deref_mut() {
            Some(other) if o.b == 1 => other,
            _ => &mut *self.primary_context,
        };
        let b = o.b;
        Some(MergeContext {
            brep,
            target,
            handle: Box::new(move |e| match e {
                EntityRef::Vertexuse(vu) => hd(b, K_VERTEXUSE, vu.index()),
                e => ArenaPair::entity_h(b, e).unwrap_or(hd(b, K_NULL, 0)),
            }),
            notifications: ed::NotifyEnv {
                records: self.records,
                attributes: self.attributes,
                context: Some(context),
                key_of,
            },
            curve_ref,
        })
    }
}

/// The B-rep walks of `MergeCoincidentSurfaces` (`boolean.topology_intersector_p3`):
/// each hook runs the unit's function on the operand the handle names.
impl IntersectorBrepP3<H> for ArenaPair<'_> {
    fn surface_face(&self, surface: H) -> crate::Result<Option<H>> {
        TopologyObjects::surface_face(self, surface)
    }

    fn edge_is_manifold(&self, edge: H) -> crate::Result<bool> {
        if edge.kind != K_EDGE {
            return Err(invalid("not an edge handle"));
        }
        ed::is_manifold(self.brep_r(edge)?, eid(edge))
    }

    fn loopuse_is_vertex_loopuse(&self, loopuse: H) -> crate::Result<bool> {
        if loopuse.kind != K_LOOPUSE {
            return Err(invalid("not a loop use handle"));
        }
        lu::is_vertex_loopuse(self.brep_r(loopuse)?, LoopuseId::from_index(loopuse.i))
    }

    fn loopuse_edgeuses(&self, loopuse: H) -> crate::Result<Vec<H>> {
        if loopuse.kind != K_LOOPUSE {
            return Err(invalid("not a loop use handle"));
        }
        Ok(lu::edgeuses(
            self.brep_r(loopuse)?,
            LoopuseId::from_index(loopuse.i),
            None,
            None,
        )?
        .into_iter()
        .map(|eu| hd(loopuse.b, K_EDGEUSE, eu.index()))
        .collect())
    }

    fn edgeuse_edge(&self, edgeuse: H) -> crate::Result<Option<H>> {
        if edgeuse.kind != K_EDGEUSE {
            return Err(invalid("not an edge use handle"));
        }
        let b = self.brep_r(edgeuse)?;
        Ok(b.edgeuse(EdgeuseId::from_index(edgeuse.i))?
            .edge
            .map(|e| hd(edgeuse.b, K_EDGE, e.index())))
    }

    fn edgeuse_face(&self, edgeuse: H) -> crate::Result<Option<H>> {
        if edgeuse.kind != K_EDGEUSE {
            return Err(invalid("not an edge use handle"));
        }
        let b = self.brep_r(edgeuse)?;
        Ok(eu1::face(b, EdgeuseId::from_index(edgeuse.i))?
            .map(|f| hd(edgeuse.b, K_FACE, f.index())))
    }

    fn faceuse_face(&self, faceuse: H) -> crate::Result<Option<H>> {
        if faceuse.kind != K_FACEUSE {
            return Err(invalid("not a face use handle"));
        }
        let b = self.brep_r(faceuse)?;
        Ok(b.faceuse(FaceuseId::from_index(faceuse.i))?
            .face
            .map(|f| hd(faceuse.b, K_FACE, f.index())))
    }

    fn edge_find_radial_sector(
        &mut self,
        k: &Kernel,
        edge: H,
        classify_edgeuse: H,
        required_orientation: OrientType,
    ) -> crate::Result<(Option<H>, Option<H>)> {
        if edge.kind != K_EDGE || classify_edgeuse.kind != K_EDGEUSE {
            return Err(invalid("not an edge and an edge use"));
        }
        let eu = EdgeuseId::from_index(classify_edgeuse.i);
        let sector = if edge.b == classify_edgeuse.b {
            ed::find_radial_sector_between(
                k,
                self.brep_m(edge)?,
                eid(edge),
                None,
                eu,
                None,
                required_orientation,
                None,
            )?
        } else {
            // the edge and the classified edge use are in the two different B-reps
            self.clear_face_caches(0);
            self.clear_face_caches(1);
            let [first, second] = &mut self.breps;
            let (b, other) = match (edge.b, classify_edgeuse.b) {
                (0, 1) => (first, second),
                (1, 0) => (second, first),
                _ => return Err(invalid("edge and edge use are not in the two operands")),
            };
            let b = b
                .as_deref_mut()
                .ok_or_else(|| invalid("handle names an absent B-rep"))?;
            let other = other
                .as_deref_mut()
                .ok_or_else(|| invalid("handle names an absent B-rep"))?;
            ed::find_radial_sector_between(
                k,
                b,
                eid(edge),
                Some(other),
                eu,
                None,
                required_orientation,
                None,
            )?
        };
        Ok((
            sector
                .radial_edgeuse
                .map(|eu| hd(edge.b, K_EDGEUSE, eu.index())),
            sector
                .coincident_faceuse
                .map(|fu| hd(edge.b, K_FACEUSE, fu.index())),
        ))
    }

    fn face_bounding_box(&self, k: &Kernel, face: H) -> crate::Result<Extent3d> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        crate::query::face::p1::calculate_bounding_box(k, self.brep_r(face)?, fid(face), false)
    }

    fn face_upward_loopuses(&self, face: H) -> crate::Result<Vec<H>> {
        if face.kind != K_FACE {
            return Err(invalid("not a face handle"));
        }
        Ok(
            crate::topology::face::p4::upward_loopuses(self.brep_r(face)?, fid(face), false, None)?
                .into_iter()
                .map(|l| hd(face.b, K_LOOPUSE, l.index()))
                .collect(),
        )
    }
}

impl crate::topology::proto_topology::p3::BrepGlue<H> for ArenaPair<'_> {
    /// `SmBrep::GlueVertices` of `brep` (`topology::brep::p5::glue_with_notifications`),
    /// notifying through the adapter's records and attributes as
    /// [`IntersectorBrep::brep_glue_edges_general`].
    fn glue_vertices(
        &mut self,
        k: &mut Kernel,
        brep: H,
        v1: H,
        v2: H,
        opt_wire_edges_in_brep: Option<&[H]>,
    ) -> crate::Result<()> {
        if brep.kind != K_BREP
            || v1.kind != K_VERTEX
            || v2.kind != K_VERTEX
            || v1.b != brep.b
            || v2.b != brep.b
        {
            return Err(invalid("glue vertex owner mismatch"));
        }
        let wires = opt_wire_edges_in_brep
            .map(|edges| {
                edges
                    .iter()
                    .map(|&e| {
                        if e.kind == K_EDGE && e.b == brep.b {
                            Ok(eid(e))
                        } else {
                            Err(invalid("glue wire edge owner mismatch"))
                        }
                    })
                    .collect::<crate::Result<Vec<_>>>()
            })
            .transpose()?;
        let slot = usize::try_from(brep.b).map_err(|_| invalid("bad B-rep"))?;
        self.clear_face_caches(slot);
        let b = self
            .breps
            .get_mut(slot)
            .and_then(Option::as_deref_mut)
            .ok_or_else(|| invalid("handle names an absent B-rep"))?;
        let mut notify = ed::NotifyEnv {
            records: self.records,
            attributes: self.attributes,
            context: Some(if brep.b == 1 {
                self.other_context
                    .as_deref_mut()
                    .unwrap_or(self.primary_context)
            } else {
                self.primary_context
            }),
            key_of: &|e| self.keys.object(brep.b == 1, e),
        };
        crate::topology::brep::p5::glue_with_notifications(
            k,
            b,
            Some(vid(v1)),
            Some(vid(v2)),
            wires.as_deref(),
            Some(&mut notify),
        )
    }

    /// `SmBrep::DeleteEdge` of `brep`, as [`IntersectorBrep::brep_delete_edge`].
    fn delete_edge(&mut self, k: &mut Kernel, brep: H, edge: H) -> crate::Result<()> {
        IntersectorBrep::brep_delete_edge(self, k, brep, edge)
    }
}

pub(super) fn encode(entity: EntityRef, other: bool) -> H {
    let (kind, i) = match entity {
        EntityRef::Face(f) => (K_FACE, f.index()),
        EntityRef::Edge(e) => (K_EDGE, e.index()),
        EntityRef::Vertex(v) => (K_VERTEX, v.index()),
        _ => (K_NULL, 0),
    };
    hd(i8::from(other), kind, i)
}
pub(super) fn decode(h: H) -> EntityRef {
    match h.kind {
        K_FACE => EntityRef::Face(fid(h)),
        K_EDGE => EntityRef::Edge(eid(h)),
        K_VERTEX => EntityRef::Vertex(vid(h)),
        _ => EntityRef::Brep(BrepId::from_index(h.i)),
    }
}

pub(super) fn surface_handle(other: bool, face: FaceId) -> H {
    hd(i8::from(other), K_FACE_SURFACE, face.index())
}

// A nonempty subset whose external identities match no surface must still
// exclude every actual surface. Operand 2 has no B-rep surfaces in this adapter.
pub(super) fn unmatched_surface() -> H {
    hd(2, K_FACE_SURFACE, 0)
}

/// A face of `MergeCoincidentSurfaces`' output lists: the primary's (`other` false) or
/// the other operand's.
pub(super) fn merged_face(h: H, other: bool) -> crate::Result<FaceId> {
    if h.kind == K_FACE && h.b == i8::from(other) {
        Ok(fid(h))
    } else {
        Err(invalid("merged face is not a face of its operand"))
    }
}

pub(super) fn deleted_vertex(h: H) -> crate::Result<VertexId> {
    if h.b == 0 && h.kind == K_VERTEX {
        Ok(vid(h))
    } else {
        Err(invalid("deleted vertex is not a primary vertex"))
    }
}
pub(super) fn deleted_edge(h: H) -> crate::Result<EdgeId> {
    if h.b == 0 && h.kind == K_EDGE {
        Ok(eid(h))
    } else {
        Err(invalid("deleted edge is not a primary edge"))
    }
}
