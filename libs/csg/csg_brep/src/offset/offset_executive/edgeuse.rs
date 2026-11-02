//! Cross-arena geometry adapter for offset edge caps.
//!
//! Upstream: `source/SMLib/src/SmOffsetExecutive.cpp` (2848–3100, the
//! `BuildOffsetOfEdgeuses` call to `EdgeuseOffset`),
//! `source/SMLib/src/SmOffsetGeometryCreation.cpp` (`EdgeuseOffset` and its helpers).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `build` (private) | `m_rGeometryCreation.EdgeuseOffset(..)` in `BuildOffsetOfEdgeuses` |
//! | `EdgeBrep` (private) | the original and offset B-reps behind `SmEdgeuse*`/`SmFace*`/curve pointers |
//! | `Executive` (private) | what `EdgeuseOffset` reads of `SmOffsetExecutive` |
//!
//! Deviations: arena-tagged handles stand in for pointers into the two B-reps and
//! for the creator's new curves and surfaces; owned values replace the output
//! pointers. Edits of either B-rep notify through the caller's stores under that
//! arena's identities, with the context the caller selected. Edge-use marks are read
//! from the caller's mark source. The extension cache is keyed by the face's
//! identity (upstream: the face surface's pointer); a new extension is copied into
//! the adapter's store, where its handle resolves. The shared creator algorithm and
//! concrete curve kinds are unchanged. No new recursion.

use std::collections::{BTreeMap, BTreeSet};

use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::attribute::AttributeStore;
use crate::geom::context::{Context, MarkType};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::intersect::point_class::{ClassGeometry, ClassObject, ClassObjectKind};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::offset::offset_geometry_creation::{
    EdgeuseOffset, OffsetBrep, OffsetExecutive as CreatorExecutive, OffsetGeometryCreation,
};
use crate::query::edge::ClosestPoint;
use crate::surface::surface::Surface;
use crate::surface::surface_cache::face_context::{FaceQueries, PreparedFaceQueries};
use crate::surface::surface_cache::SurfaceCache;
use crate::surface::volume::Volume;
use crate::topology::edge::{self, NotifyEnv, SetCurveOptions};
use crate::topology::edgeuse::{p1 as eu, p2 as eu2, p3 as eu3};
use crate::topology::face::{p1 as face1, p2 as face2};
use crate::topology::proto_topology::ProtoGeometry;
use crate::topology::{vertex, Brep, EdgeId, EdgeuseId, FaceId, VertexId};

use super::p1::{OffsetExecutive, OffsetOperation};

const OP: &str = "offset::offset_executive::edgeuse";

/// Which B-rep a topology handle or face-owned geometry handle names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Arena {
    /// The executive's original B-rep.
    Original,
    /// The executive's offset (temporary) B-rep.
    Offset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Object {
    Topology(Arena, EntityRef),
    EdgeCurve(Arena, EdgeId),
    /// The UV trim curve stored on this edge use.
    UvCurve(Arena, EdgeuseId),
    FaceSurface(Arena, FaceId),
    NewCurve(usize),
    NewSurface(usize),
}

/// An object handle of the creator; curve handles carry their natural interval.
#[derive(Debug, Clone, Copy)]
pub(super) struct Handle {
    object: Object,
    interval: Option<[f64; 2]>,
}

impl PartialEq for Handle {
    fn eq(&self, other: &Self) -> bool {
        self.object == other.object
    }
}

impl Handle {
    const fn new(object: Object) -> Self {
        Self {
            object,
            interval: None,
        }
    }

    const fn topology(arena: Arena, entity: EntityRef) -> Self {
        Self::new(Object::Topology(arena, entity))
    }

    fn curve(object: Object, curve: &Curve) -> Self {
        let interval = curve.natural_interval();
        Self {
            object,
            interval: Some([interval.min(), interval.max()]),
        }
    }

    fn entity(self) -> Result<(Arena, EntityRef)> {
        match self.object {
            Object::Topology(arena, entity) => Ok((arena, entity)),
            _ => Err(invalid_handle()),
        }
    }

    fn vertex(self) -> Result<(Arena, VertexId)> {
        match self.entity()? {
            (arena, EntityRef::Vertex(id)) => Ok((arena, id)),
            _ => Err(invalid_handle()),
        }
    }

    fn edge(self) -> Result<(Arena, EdgeId)> {
        match self.entity()? {
            (arena, EntityRef::Edge(id)) => Ok((arena, id)),
            _ => Err(invalid_handle()),
        }
    }

    fn face(self) -> Result<(Arena, FaceId)> {
        match self.entity()? {
            (arena, EntityRef::Face(id)) => Ok((arena, id)),
            _ => Err(invalid_handle()),
        }
    }

    fn edgeuse(self) -> Result<(Arena, EdgeuseId)> {
        match self.entity()? {
            (arena, EntityRef::Edgeuse(id)) => Ok((arena, id)),
            _ => Err(invalid_handle()),
        }
    }

    fn faceuse(self) -> Result<(Arena, crate::topology::FaceuseId)> {
        match self.entity()? {
            (arena, EntityRef::Faceuse(id)) => Ok((arena, id)),
            _ => Err(invalid_handle()),
        }
    }
}

impl ClassObject for Handle {
    fn class_kind(&self) -> ClassObjectKind {
        match self.object {
            Object::Topology(_, entity) => match entity {
                EntityRef::Vertex(_) => ClassObjectKind::Vertex,
                EntityRef::Edge(_) => ClassObjectKind::Edge,
                EntityRef::Face(_) => ClassObjectKind::Face,
                EntityRef::Loop(_) => ClassObjectKind::Loop,
                EntityRef::Region(_) => ClassObjectKind::Region,
                EntityRef::Vertexuse(_) => ClassObjectKind::Vertexuse,
                EntityRef::Edgeuse(_) => ClassObjectKind::Edgeuse,
                EntityRef::Faceuse(_) => ClassObjectKind::Faceuse,
                EntityRef::Loopuse(_) => ClassObjectKind::Loopuse,
                EntityRef::Shell(_) | EntityRef::Brep(_) => ClassObjectKind::Other,
            },
            Object::EdgeCurve(..) | Object::UvCurve(..) | Object::NewCurve(_) => {
                ClassObjectKind::Curve
            }
            Object::FaceSurface(..) | Object::NewSurface(_) => ClassObjectKind::Surface,
        }
    }
}

impl SolutionObject for Handle {
    fn kind(&self) -> ObjectKind {
        match self.object {
            Object::Topology(_, EntityRef::Vertex(_)) => ObjectKind::Vertex,
            Object::Topology(_, EntityRef::Edge(_)) => ObjectKind::Edge,
            Object::Topology(_, EntityRef::Face(_)) => ObjectKind::Face,
            Object::Topology(..) => ObjectKind::Other,
            Object::EdgeCurve(..) | Object::UvCurve(..) | Object::NewCurve(_) => ObjectKind::Curve,
            Object::FaceSurface(..) | Object::NewSurface(_) => ObjectKind::Surface,
        }
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        self.interval
            .map(|[min, max]| Extent1d::from_min_max(min, max))
    }
}

fn invalid_handle() -> SolidError {
    SolidError::invalid_input(
        OP,
        "edge creator handle does not resolve to the requested kind",
    )
}

/// Where `SmEdgeuse::IsMarked` reads its marks.
#[derive(Clone, Copy)]
pub(super) enum MarkSource<'a> {
    /// The caller's topology records (the context's current mark values).
    Records,
    /// Original edge uses the calling loop has marked.
    Set(&'a BTreeSet<EdgeuseId>),
}

/// The two B-reps, the creator's new geometry, and the notification state of edits.
struct EdgeBrep<'a> {
    /// The kernel of the creator's calls, for the one query the trait passes none.
    kernel: &'a Kernel,
    original: &'a mut Brep,
    offset: &'a mut Brep,
    curves: Vec<Curve>,
    surfaces: Vec<Option<Surface>>,
    records: &'a mut crate::topology::topology::NotifyRecords,
    attributes: &'a mut AttributeStore,
    context: Option<&'a mut Context>,
    original_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    offset_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    marks: MarkSource<'a>,
}

impl core::fmt::Debug for EdgeBrep<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EdgeBrep")
            .field("curves", &self.curves.len())
            .field("surfaces", &self.surfaces.len())
            .finish_non_exhaustive()
    }
}

impl EdgeBrep<'_> {
    fn brep(&self, arena: Arena) -> &Brep {
        match arena {
            Arena::Original => self.original,
            Arena::Offset => self.offset,
        }
    }

    fn brep_mut(&mut self, arena: Arena) -> &mut Brep {
        match arena {
            Arena::Original => self.original,
            Arena::Offset => self.offset,
        }
    }

    fn key_of(&self, arena: Arena) -> &dyn Fn(EntityRef) -> ObjectRef {
        match arena {
            Arena::Original => self.original_key_of,
            Arena::Offset => self.offset_key_of,
        }
    }

    /// Runs an edit of `arena` with that arena's notification environment.
    fn with_env<T>(
        &mut self,
        arena: Arena,
        run: impl FnOnce(&mut Brep, &mut NotifyEnv<'_>) -> Result<T>,
    ) -> Result<T> {
        let key_of = match arena {
            Arena::Original => self.original_key_of,
            Arena::Offset => self.offset_key_of,
        };
        let brep = match arena {
            Arena::Original => &mut *self.original,
            Arena::Offset => &mut *self.offset,
        };
        let mut env = NotifyEnv {
            records: &mut *self.records,
            attributes: &mut *self.attributes,
            context: self.context.as_deref_mut(),
            key_of,
        };
        run(brep, &mut env)
    }

    fn resolve_curve(&self, object: Object) -> Option<&Curve> {
        match object {
            Object::EdgeCurve(arena, e) => self.brep(arena).edge(e).ok()?.curve.as_ref(),
            Object::UvCurve(arena, owner) => {
                self.brep(arena).edgeuse(owner).ok()?.uv_trim_curve.as_ref()
            }
            Object::NewCurve(index) => self.curves.get(index),
            _ => None,
        }
    }

    fn resolve_surface(&self, object: Object) -> Option<&Surface> {
        match object {
            Object::FaceSurface(arena, f) => self.brep(arena).face(f).ok()?.surface.as_ref(),
            Object::NewSurface(index) => self.surfaces.get(index)?.as_ref(),
            _ => None,
        }
    }

    fn cloned_curve(&self, handle: Handle) -> Result<Curve> {
        self.resolve_curve(handle.object)
            .cloned()
            .ok_or_else(invalid_handle)
    }

    /// The face of either arena whose surface names `owner` (an offset copy keeps
    /// its base face's owner, but its handle is not a face-surface handle).
    fn face_for_owner(&self, owner: ObjectRef) -> Result<Option<(Arena, FaceId)>> {
        let mut found = None;
        for arena in [Arena::Original, Arena::Offset] {
            let brep = self.brep(arena);
            for f in brep.faces().ids() {
                let Some(surface) = brep.face(f)?.surface.as_ref() else {
                    continue;
                };
                if surface.face()? != Some(owner) {
                    continue;
                }
                if found.is_some() {
                    return Err(SolidError::invalid_input(
                        OP,
                        "surface owner names more than one face",
                    ));
                }
                found = Some((arena, f));
            }
        }
        Ok(found)
    }

    fn arena_of(&self, brep: &Brep) -> Option<Arena> {
        if core::ptr::eq(brep, self.original) {
            Some(Arena::Original)
        } else if core::ptr::eq(brep, self.offset) {
            Some(Arena::Offset)
        } else {
            None
        }
    }

    fn entity_owner_ref(&self, arena: Arena, entity: EntityRef) -> Option<ObjectRef> {
        let b = self.brep(arena);
        let expected = (self.key_of(arena))(entity);
        match entity {
            EntityRef::Edge(edge) => {
                let actual = b.edge(edge).ok()?.curve.as_ref().and_then(Curve::owner);
                if actual == Some(expected) {
                    return Some(expected);
                }
                if let Some(proven) = b.geometry_owner_key(entity) {
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
                Some(b.geometry_owner_key(entity).unwrap_or(expected))
            }
            EntityRef::Face(face) => {
                let actual = b.face(face).ok()?.surface.as_ref().and_then(Surface::owner);
                if actual == Some(expected) {
                    return Some(expected);
                }
                if let Some(proven) = b.geometry_owner_key(entity) {
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
                Some(b.geometry_owner_key(entity).unwrap_or(expected))
            }
            _ => None,
        }
    }

    fn take_output(mut self, made: EdgeuseOffset<Handle>) -> Result<EdgeCap> {
        let surface = match made.surface {
            None => None,
            Some(Handle {
                object: Object::NewSurface(index),
                ..
            }) => Some(
                self.surfaces
                    .get_mut(index)
                    .and_then(Option::take)
                    .ok_or_else(invalid_handle)?,
            ),
            Some(_) => return Err(invalid_handle()),
        };
        let mut taken = BTreeSet::new();
        let mut trim_curves = Vec::new();
        for handle in made.trimming_curves {
            let Object::NewCurve(index) = handle.object else {
                return Err(invalid_handle());
            };
            if !taken.insert(index) {
                return Err(invalid_handle());
            }
            trim_curves.push(self.curves.get(index).cloned().ok_or_else(invalid_handle)?);
        }
        Ok(EdgeCap {
            surface,
            trim_curves,
            trim_orientations: made.trim_orientations,
            closing_wall: made.closing_wall,
        })
    }
}

/// The cap geometry `EdgeuseOffset` made for one synchronized pair.
pub(super) struct EdgeCap {
    /// `pNewSurface`.
    pub(super) surface: Option<Surface>,
    /// `s3DTrimCurves`.
    pub(super) trim_curves: Vec<Curve>,
    /// `sTrimOrientations`.
    pub(super) trim_orientations: Vec<OrientType>,
    /// Not upstream (`bClosingWall`, opt-in `Kernel::fixes.local_operation_step_walls`):
    /// the cap is a step wall, kept out of the face extensions.
    pub(super) closing_wall: bool,
}

impl ClassGeometry<Handle> for EdgeBrep<'_> {
    fn curve(&self, o: Handle) -> Option<&Curve> {
        self.resolve_curve(o.object)
    }
    fn surface(&self, o: Handle) -> Option<&Surface> {
        self.resolve_surface(o.object)
    }
    fn volume(&self, _o: Handle) -> Option<&Volume> {
        None
    }
    fn point(&self, _o: Handle) -> Option<Point3> {
        None
    }
    fn classification_vertex_point(&self, o: Handle) -> Option<Point3> {
        let (arena, v) = o.vertex().ok()?;
        vertex::point(self.brep(arena), v).ok()
    }
    fn topology_zone_tol_3d(&self, o: Handle) -> Option<f64> {
        let (arena, entity) = o.entity().ok()?;
        let b = self.brep(arena);
        match entity {
            EntityRef::Vertex(v) => vertex::tolerance(b, v).ok(),
            EntityRef::Edge(e) => edge::tolerance(b, e).ok(),
            EntityRef::Face(f) => face2::tolerance(b, f).ok(),
            EntityRef::Edgeuse(id) => eu::tolerance(b, id).ok(),
            _ => None,
        }
    }
    fn topology_entity(&self, o: Handle) -> Option<(&Brep, EntityRef)> {
        let (arena, entity) = o.entity().ok()?;
        Some((self.brep(arena), entity))
    }
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<Handle> {
        Some(Handle::topology(self.arena_of(brep)?, entity))
    }
    fn brep_id(&self, brep: &Brep) -> Option<crate::topology::BrepId> {
        let idx = match self.arena_of(brep)? {
            Arena::Original => 0,
            Arena::Offset => 1,
        };
        Some(crate::topology::BrepId::from_index(idx))
    }
    fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
        if owner.type_code == edge::TYPE {
            for arena in [Arena::Original, Arena::Offset] {
                let b = self.brep(arena);
                for e in b.edges().ids() {
                    let entity = EntityRef::Edge(e);
                    if self.entity_owner_ref(arena, entity) == Some(owner) {
                        return Some((b, entity));
                    }
                }
            }
            let mut fallback = None;
            for arena in [Arena::Original, Arena::Offset] {
                let b = self.brep(arena);
                for e in b.edges().ids() {
                    let actual = b.edge(e).ok().and_then(|r| r.curve.as_ref()?.owner());
                    if actual == Some(owner)
                        && crate::topology::brep::p1::default_edge_owner(b, e) == owner
                    {
                        if fallback.is_some() {
                            return None;
                        }
                        fallback = Some((b, EntityRef::Edge(e)));
                    }
                }
            }
            return fallback;
        }
        if owner.type_code == face2::TYPE {
            for arena in [Arena::Original, Arena::Offset] {
                let b = self.brep(arena);
                for f in b.faces().ids() {
                    let entity = EntityRef::Face(f);
                    if self.entity_owner_ref(arena, entity) == Some(owner) {
                        return Some((b, entity));
                    }
                }
            }
            let mut fallback = None;
            for arena in [Arena::Original, Arena::Offset] {
                let b = self.brep(arena);
                for f in b.faces().ids() {
                    let actual = b.face(f).ok().and_then(|r| r.surface.as_ref()?.owner());
                    if actual == Some(owner)
                        && crate::topology::brep::p1::default_face_owner(b, f) == owner
                    {
                        if fallback.is_some() {
                            return None;
                        }
                        fallback = Some((b, EntityRef::Face(f)));
                    }
                }
            }
            return fallback;
        }
        None
    }
    fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        if owner.type_code == eu::TYPE {
            for arena in [Arena::Original, Arena::Offset] {
                let b = self.brep(arena);
                for id in b.edgeuses().ids() {
                    let entity = EntityRef::Edgeuse(id);
                    let matches_key = (self.key_of(arena))(entity) == owner
                        || b.notify_serial().is_some_and(|s| {
                            crate::topology::brep::p3::topology_ref(s, entity) == owner
                        });
                    if matches_key {
                        let e = eu::edge(b, id).ok()??;
                        return self.entity_owner_ref(arena, EntityRef::Edge(e));
                    }
                }
            }
            let mut fallback = None;
            for arena in [Arena::Original, Arena::Offset] {
                let b = self.brep(arena);
                for id in b.edgeuses().ids() {
                    let actual = b
                        .edgeuse(id)
                        .ok()
                        .and_then(|r| r.uv_trim_curve.as_ref()?.owner());
                    if actual == Some(owner) {
                        let e = eu::edge(b, id).ok()??;
                        let edge_owner = self.entity_owner_ref(arena, EntityRef::Edge(e))?;
                        if fallback.is_some() {
                            return None;
                        }
                        fallback = Some(edge_owner);
                    }
                }
            }
            return fallback;
        }
        None
    }
}

impl ProtoGeometry<Handle> for EdgeBrep<'_> {
    fn curve_mut(&mut self, o: Handle) -> Option<&mut Curve> {
        match o.object {
            Object::EdgeCurve(arena, e) => self.brep_mut(arena).edge_mut(e).ok()?.curve.as_mut(),
            Object::UvCurve(arena, owner) => self
                .brep_mut(arena)
                .edgeuse_mut(owner)
                .ok()?
                .uv_trim_curve
                .as_mut(),
            Object::NewCurve(index) => self.curves.get_mut(index),
            _ => None,
        }
    }
}

fn cache_handle(arena: Arena) -> impl Fn(EntityRef) -> Result<Handle> {
    move |entity| Ok(Handle::topology(arena, entity))
}

impl FaceQueries<Handle> for EdgeBrep<'_> {
    fn face_zone_tol_3d(&self, face: Handle) -> Result<f64> {
        let (arena, f) = face.face()?;
        face2::tolerance(self.brep(arena), f)
    }

    fn face_point_classify(
        &self,
        k: &Kernel,
        face: Handle,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<Handle>)> {
        let (arena, f) = face.face()?;
        let map = cache_handle(arena);
        PreparedFaceQueries::new(self.brep(arena), f, &map)?
            .face_point_classify(k, face, uv, src_tol)
    }

    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: Handle,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<Handle>)> {
        let (arena, f) = face.face()?;
        let map = cache_handle(arena);
        PreparedFaceQueries::new(self.brep(arena), f, &map)?
            .face_point_classify_fast_drop(k, face, uv, src_tol)
    }

    fn curve_edge_tolerance(&self, curve: Handle) -> Option<Result<f64>> {
        if let Object::EdgeCurve(arena, id) = curve.object {
            Some(edge::tolerance(self.brep(arena), id))
        } else {
            let c = self.resolve_curve(curve.object)?;
            let edge_ref = match c.edge(|r| self.owner_of(r)) {
                Ok(Some(r)) => r,
                Ok(None) => return None,
                Err(e) => return Some(Err(e)),
            };
            let (b, EntityRef::Edge(id)) = self.owner_entity(edge_ref)? else {
                return None;
            };
            Some(edge::tolerance(b, id))
        }
    }
}

impl EdgeBrep<'_> {
    /// `smsurf_GetSurfaceCache` of `surface` for a solve: with its `GetTree` when
    /// `get_tree` ([`OffsetBrep::prepare_surface_solve_cache`]), else as handed to a
    /// reader of the state bits only ([`OffsetBrep::prepare_surface_state_cache`]).
    fn surface_cache_for(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<Handle>,
        get_tree: bool,
    ) -> Result<SurfaceCache<Handle>> {
        use crate::query::face::p2::{solve_surface_cache_of_face, surface_cache_state_of_face};
        use crate::surface::surface::p1::{get_surface_cache, make_surface_cache};
        // the solve's GetTree: a reused cache whose tree is not built builds it now
        let with_tree = |mut cache: SurfaceCache<Handle>| {
            if get_tree && cache.tree().is_none() {
                cache.get_tree(k)?;
            }
            Ok(cache)
        };
        let Some(owner) = surface.face()? else {
            return with_tree(get_surface_cache(k, surface, handle)?);
        };
        let (arena, face) = if let Some(Object::FaceSurface(arena, face)) = handle.map(|h| h.object)
        {
            (arena, face)
        } else {
            let Some(found) = self.face_for_owner(owner)? else {
                return with_tree(get_surface_cache(k, surface, handle)?);
            };
            found
        };
        let b = self.brep_mut(arena);
        let actual_owner = face2::surface(b, face)?
            .ok_or_else(|| SolidError::invalid_input(OP, "cache face has no surface"))?
            .face()?;
        if actual_owner != Some(owner) {
            return Err(SolidError::invalid_input(
                OP,
                "surface owner is not the selected B-rep face",
            ));
        }
        // the surface's trimmed cache, kept with it as upstream keeps it (reused by
        // later solves on the same surface)
        let map = cache_handle(arena);
        let cache = if get_tree {
            solve_surface_cache_of_face(k, b, face, owner, surface, handle, map)?
        } else {
            surface_cache_state_of_face(k, b, face, owner, surface, handle, map)?
        };
        match cache {
            Some(cache) => Ok(cache),
            None => make_surface_cache(k, surface, handle),
        }
    }
}

impl OffsetBrep<Handle> for EdgeBrep<'_> {
    fn prepare_surface_solve_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<Handle>,
    ) -> Result<SurfaceCache<Handle>> {
        self.surface_cache_for(k, surface, handle, true)
    }

    fn prepare_surface_state_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<Handle>,
    ) -> Result<SurfaceCache<Handle>> {
        self.surface_cache_for(k, surface, handle, false)
    }

    fn prepared_face_queries(&self) -> Option<&dyn FaceQueries<Handle>> {
        Some(self)
    }

    fn insert_curve(&mut self, curve: Curve) -> Option<Handle> {
        let handle = Handle::curve(Object::NewCurve(self.curves.len()), &curve);
        self.curves.push(curve);
        Some(handle)
    }

    fn insert_surface(&mut self, surface: Surface) -> Option<Handle> {
        let handle = Handle::new(Object::NewSurface(self.surfaces.len()));
        self.surfaces.push(Some(surface));
        Some(handle)
    }

    fn surface_mut(&mut self, o: Handle) -> Option<&mut Surface> {
        match o.object {
            Object::FaceSurface(arena, f) => {
                self.brep_mut(arena).face_mut(f).ok()?.surface.as_mut()
            }
            Object::NewSurface(index) => self.surfaces.get_mut(index)?.as_mut(),
            _ => None,
        }
    }

    fn brep_tolerance(&self, topo: Handle) -> Result<f64> {
        let (arena, _) = topo.entity()?;
        Ok(self.brep(arena).tolerance())
    }

    fn faceuse_face(&self, fu: Handle) -> Result<Option<Handle>> {
        let (arena, fu) = fu.faceuse()?;
        Ok(eu::faceuse_face(self.brep(arena), fu)?
            .map(|f| Handle::topology(arena, EntityRef::Face(f))))
    }

    fn faceuse_orientation(&self, fu: Handle) -> Result<OrientType> {
        let (arena, fu) = fu.faceuse()?;
        eu::faceuse_orientation(self.brep(arena), fu)
    }

    fn face_surface(&self, face: Handle) -> Result<Option<Handle>> {
        let (arena, f) = face.face()?;
        Ok(self
            .brep(arena)
            .face(f)?
            .surface
            .as_ref()
            .map(|_| Handle::new(Object::FaceSurface(arena, f))))
    }

    fn face_uv_domain(&self, face: Handle) -> Result<Extent2d> {
        let (arena, f) = face.face()?;
        face2::uv_domain(self.brep(arena), f)
    }

    fn set_face_uv_domain(&mut self, face: Handle, domain: Extent2d) -> Result<()> {
        let (arena, f) = face.face()?;
        face2::set_uv_domain(self.brep_mut(arena), f, &domain)
    }

    fn face_uv_domain_from_uv_trim_curves(&mut self, k: &Kernel, face: Handle) -> Result<Extent2d> {
        let (arena, f) = face.face()?;
        Ok(face1::calculate_uv_domain_from_uv_trim_curves(
            k,
            self.brep_mut(arena),
            f,
            true,
            false,
            false,
        )?
        .domain)
    }

    fn edge_curve(&self, e: Handle) -> Result<Option<Handle>> {
        let (arena, e) = e.edge()?;
        Ok(self
            .brep(arena)
            .edge(e)?
            .curve
            .as_ref()
            .map(|c| Handle::curve(Object::EdgeCurve(arena, e), c)))
    }

    fn edge_interval(&self, e: Handle) -> Result<Extent1d> {
        let (arena, e) = e.edge()?;
        edge::interval(self.brep(arena), e)
    }

    fn set_edge_interval(&mut self, e: Handle, interval: Extent1d) -> Result<()> {
        let (arena, e) = e.edge()?;
        edge::set_interval(self.brep_mut(arena), e, &interval)
    }

    fn edge_tolerance(&self, e: Handle) -> Result<f64> {
        let (arena, e) = e.edge()?;
        edge::tolerance(self.brep(arena), e)
    }

    fn set_edge_tolerance(&mut self, _k: &Kernel, e: Handle, tolerance: f64) -> Result<()> {
        let (arena, e) = e.edge()?;
        edge::set_tolerance(self.brep_mut(arena), e, tolerance, true, true)
    }

    fn edge_set_curve(&mut self, _k: &Kernel, e: Handle, curve: Handle) -> Result<()> {
        let (arena, e) = e.edge()?;
        let curve = self.cloned_curve(curve)?;
        let options = SetCurveOptions {
            delete_old_curve: true,
            dbg_warn_leaks: true,
            call_notify: true,
        };
        self.with_env(arena, |b, env| {
            edge::set_curve(b, e, Some(curve), options, env, [None, None])?;
            let owner = (env.key_of)(EntityRef::Edge(e));
            if let Some(c) = b.edge_mut(e)?.curve.as_mut() {
                c.set_owner(Some(owner), None);
            }
            b.set_geometry_owner_key(EntityRef::Edge(e), Some(owner));
            Ok(())
        })
    }

    fn brep_mate_lamina_edge_to_surface(
        &mut self,
        k: &Kernel,
        e: Handle,
        surface: Handle,
        surface_extension_factor: f64,
        uv_curve_extension_factor: f64,
    ) -> Result<()> {
        let (arena, e) = e.edge()?;
        // a const reference upstream: the mating surface is only read
        let mating = self
            .resolve_surface(surface.object)
            .cloned()
            .ok_or_else(invalid_handle)?;
        self.with_env(arena, |b, env| {
            crate::topology::brep::p2::mate_lamina_edge_to_surface(
                k,
                b,
                env,
                e,
                &mating,
                surface_extension_factor,
                uv_curve_extension_factor,
            )
        })
    }

    fn brep_replace_surface(
        &mut self,
        k: &Kernel,
        face: Handle,
        surface: Handle,
        add_to_deleted_list: bool,
        create_uv_trim_curves: bool,
    ) -> Result<()> {
        use crate::topology::brep::p2::OldSurface;
        let (arena, f) = face.face()?;
        let new_surface = self
            .resolve_surface(surface.object)
            .cloned()
            .ok_or_else(invalid_handle)?;
        let old = if add_to_deleted_list {
            OldSurface::AddToDeletedList
        } else {
            OldSurface::Delete
        };
        self.with_env(arena, |b, env| {
            let face_ref = (env.key_of)(EntityRef::Face(f));
            crate::topology::brep::p2::replace_surface(
                k,
                b,
                env,
                f,
                new_surface,
                face_ref,
                old,
                create_uv_trim_curves,
            )
            .map(|_| ())
        })
    }

    fn edge_faces(&self, e: Handle) -> Result<Vec<Handle>> {
        let (arena, e) = e.edge()?;
        Ok(edge::faces(self.brep(arena), e, None)?
            .into_iter()
            .map(|f| Handle::topology(arena, EntityRef::Face(f)))
            .collect())
    }

    fn edge_edgeuses(&self, e: Handle) -> Result<Vec<Handle>> {
        let (arena, e) = e.edge()?;
        Ok(edge::edgeuses(self.brep(arena), e, None, None)?
            .into_iter()
            .map(|u| Handle::topology(arena, EntityRef::Edgeuse(u)))
            .collect())
    }

    fn edge_is_lamina(&self, e: Handle) -> Result<bool> {
        let (arena, e) = e.edge()?;
        edge::is_lamina(self.brep(arena), e)
    }

    fn edge_is_seam_of_face_surface(
        &mut self,
        k: &Kernel,
        e: Handle,
        face: Handle,
    ) -> Result<bool> {
        let (arena, e) = e.edge()?;
        let (face_arena, f) = face.face()?;
        if face_arena != arena {
            return Err(SolidError::invalid_input(OP, "face of the other B-rep"));
        }
        edge::is_seam_of_surface(k, self.brep_mut(arena), e, f)
    }

    fn edge_vertex(&self, e: Handle) -> Result<Option<Handle>> {
        let (arena, e) = e.edge()?;
        Ok(edge::vertex(self.brep(arena), e)?
            .map(|v| Handle::topology(arena, EntityRef::Vertex(v))))
    }

    fn edge_other_vertex(&self, e: Handle, v: Handle) -> Result<Option<Handle>> {
        let (arena, e) = e.edge()?;
        // a vertex of the other B-rep is no end of this edge
        let v = match v.vertex()? {
            (a, v) if a == arena => Some(v),
            _ => None,
        };
        Ok(edge::other_vertex(self.brep(arena), e, v)?
            .map(|v| Handle::topology(arena, EntityRef::Vertex(v))))
    }

    fn edge_closest_point(
        &self,
        k: &Kernel,
        e: Handle,
        point: Point3,
    ) -> Result<Option<ClosestPoint>> {
        let (arena, e) = e.edge()?;
        crate::query::edge::closest_point(k, self.brep(arena), e, point)
    }

    fn edge_create_trimmed_nurbs_curve(&mut self, k: &Kernel, e: Handle) -> Result<Option<Handle>> {
        let (arena, e) = e.edge()?;
        match edge::create_trimmed_nurbs_curve(k, self.brep(arena), e)? {
            Some(curve) => Ok(self.insert_curve(curve)),
            None => Ok(None),
        }
    }

    fn vertex_point(&self, v: Handle) -> Result<Point3> {
        let (arena, v) = v.vertex()?;
        vertex::point(self.brep(arena), v)
    }

    fn set_vertex_point(&mut self, _k: &Kernel, v: Handle, point: Point3) -> Result<()> {
        let (arena, v) = v.vertex()?;
        self.with_env(arena, |b, env| {
            let key_of = env.key_of;
            let this = key_of(EntityRef::Vertex(v));
            vertex::set_point(
                b,
                v,
                point,
                env.records,
                env.attributes,
                env.context.as_deref_mut(),
                this,
                &|vu| key_of(EntityRef::Vertexuse(vu)),
            )
        })
    }

    fn vertex_is_lamina(&self, v: Handle) -> Result<bool> {
        let (arena, v) = v.vertex()?;
        vertex::is_lamina_vertex(self.brep(arena), v)
    }

    fn vertex_are_edges_tangent(
        &self,
        v: Handle,
        e1: Handle,
        e2: Handle,
        ang_tol_deg: f64,
    ) -> Result<bool> {
        let (arena, v) = v.vertex()?;
        let (a1, e1) = e1.edge()?;
        let (a2, e2) = e2.edge()?;
        if a1 != arena || a2 != arena {
            return Err(SolidError::invalid_input(OP, "edges of the other B-rep"));
        }
        vertex::are_edges_tangent(self.kernel, self.brep(arena), v, e1, e2, ang_tol_deg)
    }

    fn edgeuse_edge(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        Ok(eu::edge(self.brep(arena), u)?.map(|e| Handle::topology(arena, EntityRef::Edge(e))))
    }

    fn edgeuse_mate(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        Ok(eu::mate(self.brep(arena), u)?.map(|m| Handle::topology(arena, EntityRef::Edgeuse(m))))
    }

    fn edgeuse_radial(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        Ok(
            eu::radial(self.brep(arena), u)?
                .map(|r| Handle::topology(arena, EntityRef::Edgeuse(r))),
        )
    }

    fn edgeuse_cw(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        Ok(eu::cw_edgeuse(self.brep(arena), u)?
            .map(|r| Handle::topology(arena, EntityRef::Edgeuse(r))))
    }

    fn edgeuse_ccw(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        Ok(eu::ccw_edgeuse(self.brep(arena), u)?
            .map(|r| Handle::topology(arena, EntityRef::Edgeuse(r))))
    }

    fn edgeuse_vertex(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        let b = self.brep(arena);
        let Some(vu) = eu::vertexuse(b, u)? else {
            return Ok(None);
        };
        Ok(b.vertexuse(vu)?
            .vertex
            .map(|v| Handle::topology(arena, EntityRef::Vertex(v))))
    }

    fn edgeuse_face(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        Ok(eu::face(self.brep(arena), u)?.map(|f| Handle::topology(arena, EntityRef::Face(f))))
    }

    fn edgeuse_orientation(&self, u: Handle) -> Result<OrientType> {
        let (arena, u) = u.edgeuse()?;
        eu::orientation(self.brep(arena), u)
    }

    fn edgeuse_faceuse(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        Ok(eu::faceuse(self.brep(arena), u)?
            .map(|f| Handle::topology(arena, EntityRef::Faceuse(f))))
    }

    fn edgeuse_uv_trim_curve(&self, u: Handle) -> Result<Option<Handle>> {
        let (arena, u) = u.edgeuse()?;
        Ok(match eu::uv_trim_curve(self.brep(arena), u)? {
            (Some(c), Some(owner)) => Some(Handle::curve(Object::UvCurve(arena, owner), c)),
            _ => None,
        })
    }

    fn edgeuse_set_uv_trim_curve(
        &mut self,
        _k: &Kernel,
        u: Handle,
        curve: Option<Handle>,
        max_dist: f64,
    ) -> Result<()> {
        let (arena, u) = u.edgeuse()?;
        let arg = match curve {
            Some(h) => eu::UvTrimCurveArg::New(Box::new(self.cloned_curve(h)?)),
            None => eu::UvTrimCurveArg::Null,
        };
        eu::set_uv_trim_curve(self.brep_mut(arena), u, arg, max_dist, true, None)?;
        Ok(())
    }

    fn edgeuse_is_tangent_sector(&mut self, k: &Kernel, u: Handle, tol_deg: f64) -> Result<bool> {
        let (arena, u) = u.edgeuse()?;
        eu2::is_tangent_sector(k, self.brep_mut(arena), u, tol_deg, false, true)
    }

    fn edgeuse_is_convex_radial_sector(
        &mut self,
        k: &Kernel,
        u: Handle,
        num_samples: u64,
    ) -> Result<bool> {
        let (arena, u) = u.edgeuse()?;
        eu2::is_convex_radial_sector(k, self.brep_mut(arena), u, num_samples, false, 0.0)
    }

    fn edgeuse_is_marked(&self, k: &Kernel, u: Handle, mark: MarkType) -> Result<bool> {
        let (arena, id) = u.edgeuse()?;
        self.brep(arena).edgeuse(id)?;
        match &self.marks {
            MarkSource::Set(set) => Ok(arena == Arena::Original && set.contains(&id)),
            MarkSource::Records => {
                let key = (self.key_of(arena))(EntityRef::Edgeuse(id)).key;
                Ok(self
                    .records
                    .get(&key)
                    .is_some_and(|record| record.is_marked(&k.context, mark)))
            }
        }
    }

    fn edgeuse_normalized_evaluate(
        &mut self,
        k: &Kernel,
        u: Handle,
        t: f64,
        parameter_space_eval: bool,
        want_first_deriv: bool,
        ok_to_make_uv_trim_curve: bool,
    ) -> Result<Point3> {
        let (arena, u) = u.edgeuse()?;
        Ok(eu3::normalized_evaluate(
            k,
            self.brep_mut(arena),
            u,
            t,
            parameter_space_eval,
            want_first_deriv,
            ok_to_make_uv_trim_curve,
        )?
        .point)
    }

    fn edgeuse_create_uv_trim_curve(
        &mut self,
        k: &Kernel,
        u: Handle,
        want_max_deviation: bool,
        want_drop_curve_fail: bool,
    ) -> Result<(f64, Curve)> {
        let (arena, u) = u.edgeuse()?;
        let mut fail = crate::surface::surface::p2::DropCurveFail::default();
        let made = eu3::create_uv_trim_curve(
            k,
            self.brep_mut(arena),
            u,
            want_max_deviation,
            want_drop_curve_fail.then_some(&mut fail),
        )?;
        Ok((made.max_dist_to_surf, made.curve))
    }
}

/// What `EdgeuseOffset` reads of the executive, over the adapter's handles.
struct Executive<'a> {
    inner: &'a mut OffsetExecutive,
    /// The face identities that key the extension cache (as the extended offset's
    /// face pass keys it), per arena.
    original_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    offset_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// The handles of extensions copied into the adapter's store, by cache key.
    made: BTreeMap<u64, Handle>,
}

impl CreatorExecutive<Handle> for Executive<'_> {
    fn inset(&self) -> bool {
        self.inner.inset()
    }

    fn extend_convex_edges(&self) -> bool {
        self.inner.extend_convex_edges()
    }

    fn shell_faces(&self) -> Vec<Handle> {
        self.inner
            .shell_faces()
            .iter()
            .map(|&f| Handle::topology(Arena::Original, EntityRef::Face(f)))
            .collect()
    }

    fn is_local_operation(&self) -> bool {
        self.inner.operation() == OffsetOperation::LocalOperation
    }

    fn has_collapsed_faces(&self) -> bool {
        !self.inner.collapsed_faces.is_empty()
    }

    fn is_collapsed_face(&self, face: Handle) -> bool {
        match face.object {
            Object::Topology(Arena::Original, EntityRef::Face(f)) => {
                self.inner.is_collapsed_face(f)
            }
            _ => false,
        }
    }

    fn offset_edgeuse(&self, original: Handle) -> Option<Handle> {
        match original.object {
            Object::Topology(Arena::Original, EntityRef::Edgeuse(u)) => self
                .inner
                .offset_edgeuse(u)
                .map(|o| Handle::topology(Arena::Offset, EntityRef::Edgeuse(o))),
            _ => None,
        }
    }

    fn get_create_extended_surface<B: OffsetBrep<Handle>>(
        &mut self,
        k: &Kernel,
        brep: &mut B,
        base: Handle,
    ) -> Result<(Handle, Extent2d)> {
        let Object::FaceSurface(arena, f) = base.object else {
            return Err(SolidError::invalid_input(
                OP,
                "GetCreateExtendedSurface of a surface no face owns",
            ));
        };
        let surface = ClassGeometry::surface(brep, base)
            .ok_or_else(|| {
                SolidError::invalid_input(OP, "GetCreateExtendedSurface of a null surface")
            })?
            .clone();
        let key_of = match arena {
            Arena::Original => self.original_key_of,
            Arena::Offset => self.offset_key_of,
        };
        let key = key_of(EntityRef::Face(f)).key.0;
        let extension = self
            .inner
            .get_create_extended_surface(k, key, &surface)?
            .clone();
        let domain = extension.natural_uv_domain_with_nurb(k)?;
        if let Some(&handle) = self.made.get(&key) {
            return Ok((handle, domain));
        }
        let handle = brep
            .insert_surface(extension)
            .ok_or_else(|| SolidError::invalid_input(OP, "surface store is full"))?;
        self.made.insert(key, handle);
        Ok((handle, domain))
    }
}

/// The caller's notification data for an edge cap's edits.
pub(super) struct CapNotifications<'a> {
    pub(super) records: &'a mut crate::topology::topology::NotifyRecords,
    pub(super) attributes: &'a mut AttributeStore,
    pub(super) context: Option<&'a mut Context>,
    pub(super) original_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    pub(super) offset_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
}

/// `EdgeuseOffset(ctx, this, pEdgeuse, pOtherEdgeuse, pOffsetEU, pOtherOffEU, ..,
/// eMarkType)` over the executive's original and offset B-reps.
#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    kernel: &Kernel,
    creator: &OffsetGeometryCreation,
    executive: &mut OffsetExecutive,
    original: &mut Brep,
    offset: &mut Brep,
    edgeuse: EdgeuseId,
    other: EdgeuseId,
    offset_edgeuse: Option<EdgeuseId>,
    other_offset_edgeuse: Option<EdgeuseId>,
    mark_type: MarkType,
    marks: MarkSource<'_>,
    notifications: CapNotifications<'_>,
) -> Result<EdgeCap> {
    let mut backend = EdgeBrep {
        kernel,
        original,
        offset,
        curves: Vec::new(),
        surfaces: Vec::new(),
        records: notifications.records,
        attributes: notifications.attributes,
        context: notifications.context,
        original_key_of: notifications.original_key_of,
        offset_key_of: notifications.offset_key_of,
        marks,
    };
    let mut exec = Executive {
        inner: executive,
        original_key_of: notifications.original_key_of,
        offset_key_of: notifications.offset_key_of,
        made: BTreeMap::new(),
    };
    let original_use = |u| Handle::topology(Arena::Original, EntityRef::Edgeuse(u));
    let offset_use = |u| Handle::topology(Arena::Offset, EntityRef::Edgeuse(u));
    let made = creator.edgeuse_offset(
        kernel,
        &mut backend,
        &mut exec,
        original_use(edgeuse),
        Some(original_use(other)),
        offset_edgeuse.map(offset_use),
        other_offset_edgeuse.map(offset_use),
        mark_type,
    )?;
    backend.take_output(made)
}

/// Not upstream (WR-346): `MateEdges(ctx, this, pEdgeuse, pAcross, pOffsetEU,
/// pOffAcross, eMarkType)` over the executive's original and offset B-reps, mating the
/// offsets of the faces on either side of a collapsed face.
#[allow(clippy::too_many_arguments)]
pub(super) fn mate(
    kernel: &Kernel,
    creator: &OffsetGeometryCreation,
    executive: &mut OffsetExecutive,
    original: &mut Brep,
    offset: &mut Brep,
    edgeuse: EdgeuseId,
    other: EdgeuseId,
    offset_edgeuse: EdgeuseId,
    other_offset_edgeuse: EdgeuseId,
    mark_type: MarkType,
    marks: MarkSource<'_>,
    notifications: CapNotifications<'_>,
) -> Result<()> {
    let mut backend = EdgeBrep {
        kernel,
        original,
        offset,
        curves: Vec::new(),
        surfaces: Vec::new(),
        records: notifications.records,
        attributes: notifications.attributes,
        context: notifications.context,
        original_key_of: notifications.original_key_of,
        offset_key_of: notifications.offset_key_of,
        marks,
    };
    let mut exec = Executive {
        inner: executive,
        original_key_of: notifications.original_key_of,
        offset_key_of: notifications.offset_key_of,
        made: BTreeMap::new(),
    };
    let original_use = |u| Handle::topology(Arena::Original, EntityRef::Edgeuse(u));
    let offset_use = |u| Handle::topology(Arena::Offset, EntityRef::Edgeuse(u));
    creator.mate_edges(
        kernel,
        &mut backend,
        &mut exec,
        Some(original_use(edgeuse)),
        Some(original_use(other)),
        Some(offset_use(offset_edgeuse)),
        Some(offset_use(other_offset_edgeuse)),
        mark_type,
    )
}
