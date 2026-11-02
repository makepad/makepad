//! Arena-based B-rep topology with typed ids.
//!
//! Upstream: `source/SMLib/inc/SmBrep.h`, `SmRegion.h`, `SmShell.h`, `SmFace.h`,
//! `SmLoop.h`, `SmEdge.h`, `SmVertex.h`, the use records `SmFaceuse.h`, `SmLoopuse.h`,
//! `SmEdgeuse.h`, `SmVertexuse.h`, and `SmTopology.h` / `SmOwningTopology.h`.
//!
//! | Rust                        | Upstream                                              |
//! |-----------------------------|-------------------------------------------------------|
//! | [`Brep`]                    | `SmBrep` (face/edge/vertex lists `m_p*ListHead`, `m_bValidateOnly`; its own members in [`brep::p1`]) |
//! | [`Region`] / [`RegionId`]   | `SmRegion` (minimal: list owner, shell list, void flag) |
//! | [`Shell`] / [`ShellId`]     | `SmShell` (faces, region, type and list head)         |
//! | [`Brep::new_with_infinite_region`] | `SmBrep::SmBrep()` (topology part, CO-PORTED) |
//! | [`Brep::add_attribute`], [`Brep::attach_attribute`], [`Brep::find_attribute`], [`Brep::entity_attributes`], [`Brep::remove_attribute`], [`Brep::clear_attributes`] | `SmAObject::AddAttribute`, `FindAttribute`, `GetAttributes`, `RemoveAttribute`, `ClearAttributes` on the B-rep's topology objects (their `m_pAttributes`, kept in one store per B-rep) |
//! | [`Brep::notify_serial`]     | (none: the B-rep's kernel-issued notification serial; CO-PORTED by `topology.brep_p3`) |
//! | [`Faceuse`] / [`FaceuseId`] | `SmFaceuse` (unit `topology.edgeuse_p1`)              |
//! | [`Face`] / [`FaceId`]       | `SmFace` (unit `topology.face_p2`)                    |
//! | [`Loopuse`] / [`LoopuseId`] | `SmLoopuse` (unit `topology.loopuse`)                 |
//! | [`Loop`] / [`LoopId`]       | `SmLoop`                                              |
//! | [`Edgeuse`] / [`EdgeuseId`] | `SmEdgeuse` (unit `topology.edgeuse_p1`)              |
//! | [`Edge`] / [`EdgeId`]       | `SmEdge`                                              |
//! | [`Vertexuse`] / [`VertexuseId`] | `SmVertexuse` (unit `topology.vertexuse`)         |
//! | [`Vertex`] / [`VertexId`]   | `SmVertex` (unit `topology.vertex`)                   |
//! | [`ObjectHandle`]            | an `SmObject*` a record keeps but never dereferences  |
//! | [`Arena`]                   | upstream's intrusive `SmListNode` linked lists        |
//!
//! Upstream links entities with raw pointers in intrusive lists; the port stores each
//! kind in a flat [`Arena`] owned by the [`Brep`] and links them with typed ids, so
//! iteration order is insertion order (deterministic) and there is no shared
//! ownership. This file is the **skeleton** shared by every unit: the record fields
//! below are the minimum the scaffolding needs. The topology units in
//! `plan/modules.toml` (regions, face/loop/edge/vertex uses, geometry binding,
//! tolerances, attributes) extend these records; keep the id/arena/accessor shape.
//!
//! The use records and the fields marked "CO-PORTED" were added by unit
//! `topology.edgeuse_p1` (the minimum its functions read). An owner's circular
//! `m_pList` / `m_pNext` / `m_pLast` list is a `Vec` of ids in list order from the
//! head (`m_pList`); an object's `m_pNext` / `m_pLast` are its neighbours in that
//! `Vec` (cyclically). The low-level `add_*_record` and `*_mut` accessors are the
//! friend writes of `SmBrepConstructor`: they link records without validation (a
//! B-rep graph is cyclic, so a record may name ids added later).

pub mod api_files;
pub mod api_projection;
pub mod brep;
pub mod brep_cache;
pub mod brep_data;
pub mod brep_data_write;
pub mod database_io;
pub mod edge;
pub mod edgeuse;
pub mod face;
pub mod geometry_write;
pub mod integrity;
pub mod loopuse;
pub mod part_file;
pub mod proto_topology;
pub mod shell;
#[allow(clippy::module_inception)]
pub mod topology;
pub mod topology_traverser;
pub mod trimming_tools;
pub mod vertex;
pub mod vertexuse;

use core::marker::PhantomData;
use std::sync::Arc;

use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::attribute::{Attribute, AttributeHandle, AttributeStore, ObjectKey};
use crate::geom::context::IdentitySlot;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::{USE_DEFAULT, ZONE_TOL_3D};
use crate::geom::vector2d::Point2;
use crate::heal::gap::{EdgeEdgeGap, EdgeFaceGap, VertexEdgeGap, VertexFaceGap};
use crate::math::{Point3, BIG_DOUBLE};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::poly::PolyVertexId;
use crate::surface::surface::p2::DropCurveFail;
use crate::surface::surface::Surface;

/// Implemented by every typed id: conversion to and from a dense arena index.
pub trait ArenaId: Copy + Eq + core::fmt::Debug {
    /// Builds an id from an arena index.
    fn from_index(index: u32) -> Self;
    /// The arena index of this id.
    fn index(self) -> u32;
    /// This id as an error-context [`EntityRef`].
    fn entity(self) -> EntityRef;
}

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident => $variant:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            /// Builds an id from a raw index. Ids are only meaningful for the B-rep
            /// (or collection) that issued them.
            #[must_use]
            pub const fn from_index(index: u32) -> Self {
                Self(index)
            }

            /// The raw index of this id.
            #[must_use]
            pub const fn index(self) -> u32 {
                self.0
            }
        }

        impl ArenaId for $name {
            fn from_index(index: u32) -> Self {
                Self(index)
            }
            fn index(self) -> u32 {
                self.0
            }
            fn entity(self) -> EntityRef {
                EntityRef::$variant(self)
            }
        }
    };
}

typed_id!(
    /// Identifies a [`Brep`] within a caller-owned collection of B-reps.
    BrepId => Brep
);
typed_id!(
    /// Identifies a [`Shell`] within its [`Brep`].
    ShellId => Shell
);
typed_id!(
    /// Identifies a [`Face`] within its [`Brep`].
    FaceId => Face
);
typed_id!(
    /// Identifies a [`Loop`] within its [`Brep`].
    LoopId => Loop
);
typed_id!(
    /// Identifies an [`Edge`] within its [`Brep`].
    EdgeId => Edge
);
typed_id!(
    /// Identifies a [`Vertex`] within its [`Brep`].
    VertexId => Vertex
);
typed_id!(
    /// Identifies a [`Region`] within its [`Brep`].
    RegionId => Region
);
typed_id!(
    /// Identifies a [`Faceuse`] within its [`Brep`].
    FaceuseId => Faceuse
);
typed_id!(
    /// Identifies a [`Loopuse`] within its [`Brep`].
    LoopuseId => Loopuse
);
typed_id!(
    /// Identifies an [`Edgeuse`] within its [`Brep`].
    EdgeuseId => Edgeuse
);
typed_id!(
    /// Identifies a [`Vertexuse`] within its [`Brep`].
    VertexuseId => Vertexuse
);

/// A copy-on-write handle to a big arena of a [`Brep`]: `Clone` is a deep copy (as for
/// a plain `Arena`), [`Shared::share`] is a refcount bump, and the first write through
/// `DerefMut` of a shared handle copies the arena, so every writer behaves as on an
/// unshared one. Used by [`Brep::clone_sharing_surface_caches`] for the arenas a query's
/// snapshot only reads.
#[derive(Debug)]
struct Shared<T>(Arc<T>);

impl<T> Shared<T> {
    fn share(&self) -> Self {
        Self(Arc::clone(&self.0))
    }

    fn copy(&self, share: bool) -> Self
    where
        T: Clone,
    {
        if share {
            self.share()
        } else {
            self.clone()
        }
    }
}

impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T: Clone> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(Arc::new((*self.0).clone()))
    }
}

impl<T> core::ops::Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> core::ops::DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

/// A dense store of `T` addressed by typed id `I`. Items are appended; an item can
/// be removed (upstream's `delete` of a topology object, unit `topology.brep_p4`),
/// which empties its slot for good: ids are never reused, so the ids of the other
/// items do not change, and iteration skips the empty slots.
#[derive(Debug, Clone, PartialEq)]
pub struct Arena<I, T> {
    items: Vec<Option<T>>,
    live: usize,
    _id: PhantomData<fn() -> I>,
}

impl<I, T> Default for Arena<I, T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            live: 0,
            _id: PhantomData,
        }
    }
}

impl<I: ArenaId, T> Arena<I, T> {
    /// Appends an item and returns its id.
    ///
    /// Fails with [`SolidError::InvalidInput`] if the arena already holds `u32::MAX`
    /// slots or cannot grow (allocation failure).
    pub fn insert(&mut self, item: T) -> Result<I> {
        let index = u32::try_from(self.items.len()).map_err(|_| SolidError::InvalidInput {
            operation: "topology::Arena::insert",
            reason: "arena is full",
        })?;
        self.items
            .try_reserve(1)
            .map_err(|_| SolidError::InvalidInput {
                operation: "topology::Arena::insert",
                reason: "arena cannot grow",
            })?;
        self.items.push(Some(item));
        self.live = self.live.saturating_add(1);
        Ok(I::from_index(index))
    }

    /// Removes the item with id `id` (upstream `delete`) and returns it; `None` if the
    /// id is out of range or already removed. The slot stays empty.
    pub fn remove(&mut self, id: I) -> Option<T> {
        let item = self.items.get_mut(id.index() as usize)?.take()?;
        self.live = self.live.saturating_sub(1);
        Some(item)
    }

    /// The item with id `id`, or `None` if the id is out of range or removed.
    #[must_use]
    pub fn get(&self, id: I) -> Option<&T> {
        self.items.get(id.index() as usize)?.as_ref()
    }

    /// Mutable access to the item with id `id`.
    #[must_use]
    pub fn get_mut(&mut self, id: I) -> Option<&mut T> {
        self.items.get_mut(id.index() as usize)?.as_mut()
    }

    /// Number of items (removed ones not counted).
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// Number of slots, removed items included: the index the next item takes.
    #[must_use]
    pub(crate) fn slot_count(&self) -> usize {
        self.items.len()
    }

    /// True when the arena holds no item.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Iterates `(id, item)` in insertion order, skipping removed items.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (I, &T)> + DoubleEndedIterator + '_ {
        ArenaIter {
            inner: self.items.iter().enumerate(),
            remaining: self.live,
            _id: PhantomData,
        }
    }

    /// Iterates ids in insertion order, skipping removed items.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = I> + DoubleEndedIterator + '_ {
        self.iter().map(|(id, _)| id)
    }
}

/// The iterator behind [`Arena::iter`]: `(id, item)` in insertion order, skipping
/// removed items.
#[derive(Debug, Clone)]
struct ArenaIter<'a, I, T> {
    inner: core::iter::Enumerate<core::slice::Iter<'a, Option<T>>>,
    remaining: usize,
    _id: PhantomData<fn() -> I>,
}

impl<'a, I: ArenaId, T> Iterator for ArenaIter<'a, I, T> {
    type Item = (I, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        // Arena length never exceeds u32::MAX (checked in `insert`), so the cast is
        // lossless.
        for (i, slot) in self.inner.by_ref() {
            if let Some(t) = slot {
                self.remaining = self.remaining.saturating_sub(1);
                return Some((I::from_index(i as u32), t));
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<I: ArenaId, T> DoubleEndedIterator for ArenaIter<'_, I, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        while let Some((i, slot)) = self.inner.next_back() {
            if let Some(t) = slot {
                self.remaining = self.remaining.saturating_sub(1);
                return Some((I::from_index(i as u32), t));
            }
        }
        None
    }
}

impl<I: ArenaId, T> ExactSizeIterator for ArenaIter<'_, I, T> {}

/// An `SmObject*` a topology record keeps but never dereferences (the surface,
/// curve and B-rep pointers of an `SmDropCurveFail`): the object's kind and the
/// identity it is attributed and notified under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectHandle {
    /// What the object is.
    pub kind: ObjectKind,
    /// Its identity.
    pub key: ObjectKey,
}

impl SolutionObject for ObjectHandle {
    fn kind(&self) -> ObjectKind {
        self.kind
    }
}

/// A vertex (`SmVertex`): a point where edges meet (the methods are in unit
/// `topology.vertex`, [`vertex`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Vertex {
    /// `m_vPoint`: position in model space.
    pub point: Point3,
    /// `m_sZoneTol3d` (`SM_USE_OLDTOL`): the vertex tolerance.
    pub tolerance: f64,
    /// `m_pList` / `m_lListSize`: the vertex's vertex uses in list order from the head
    /// (the primary vertex use, `GetPrimaryVertexuse`, is first).
    pub vertexuses: Vec<VertexuseId>,
    /// `m_pPolyVertex3D`: the temporary back pointer to the 3D poly vertex made from the
    /// vertex during tessellation.
    pub poly_vertex: Option<PolyVertexId>,
}

impl Default for Vertex {
    /// `SmVertex()`: at the origin, tolerance `0`, no vertex uses, no poly vertex.
    fn default() -> Self {
        Self {
            point: Point3::new(0.0, 0.0, 0.0),
            tolerance: 0.0,
            vertexuses: Vec::new(),
            poly_vertex: None,
        }
    }
}

/// An edge (`SmEdge`): a bounded curve between two vertices.
#[derive(Debug, Clone)]
pub struct Edge {
    /// Start and end vertex (equal for a closed edge), as set when the edge was made.
    /// Upstream has no such field: `GetVertices` / `GetStartVertex` read the ends through
    /// the edge and vertex uses ([`edge::vertices`], [`edge::start_end_vertices`]), and
    /// topology edits inside an operation (vertex merges, edge splits) rewire only those
    /// uses. Inside an operation read the ends through the uses; the public operations
    /// refresh this pair before they return ([`Brep::refresh_cached_topology`]).
    pub vertices: [VertexId; 2],
    /// `m_pList` (CO-PORTED): the edge's edge uses in radial list order; the first is
    /// the primary edge use (`GetPrimaryEdgeuse`).
    pub edgeuses: Vec<EdgeuseId>,
    /// `m_vInterval` (CO-PORTED): the used parameter interval of the curve.
    pub interval: Extent1d,
    /// `m_sZoneTol3d` (CO-PORTED, `SM_USE_OLDTOL`): the edge tolerance.
    pub tolerance: f64,
    /// `m_pCurve` (CO-PORTED): the edge's 3D curve.
    pub curve: Option<Curve>,
}

/// One oriented use of an edge in a loop (upstream `SmEdgeuse`, simplified).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopEdge {
    /// The edge used.
    pub edge: EdgeId,
    /// True when the loop traverses the edge from `vertices[0]` to `vertices[1]`.
    pub forward: bool,
}

/// A closed boundary loop of a face (`SmLoop`).
#[derive(Debug, Clone, PartialEq)]
pub struct Loop {
    /// Face this loop bounds. Port-only: upstream's `GetFace` goes through the loop use
    /// and its face use. Refreshed by [`Brep::refresh_cached_topology`].
    pub face: FaceId,
    /// Edges in traversal order. Port-only builder data (upstream walks the loop use's
    /// edge uses); after a refresh ([`Brep::refresh_cached_topology`]) the primary loop
    /// use's edge-use cycle.
    pub edges: Vec<LoopEdge>,
    /// `m_pLU` (CO-PORTED): the primary loop use (`GetLoopuse`).
    pub loopuse: Option<LoopuseId>,
}

/// A face (`SmFace`): a bounded region of a surface. The methods are in unit
/// `topology.face_p2` ([`face::p2`]).
#[derive(Debug, Clone)]
pub struct Face {
    /// Shell containing this face. Port-only: upstream's `GetShell` reads the primary
    /// face use's owner, and the native shell moves update only that; read it through
    /// the face use inside an operation. Refreshed by [`Brep::refresh_cached_topology`].
    pub shell: ShellId,
    /// Boundary loops; upstream keeps the outer loop first. Port-only: upstream reaches
    /// them through the primary face use's loop uses. Refreshed by
    /// [`Brep::refresh_cached_topology`] in upstream's iteration order, the primary face
    /// use's loop-use list order.
    pub loops: Vec<LoopId>,
    /// `m_pSurface`: the face's surface (`GetSurface`).
    pub surface: Option<Surface>,
    /// `m_pFU`: the primary (upward, [`OrientType::Same`]) face use
    /// (`GetUpwardFaceuse`).
    pub faceuse: Option<FaceuseId>,
    /// `m_vUVDomain`: the domain of the surface the face uses (`GetUVDomain`).
    pub uv_domain: Extent2d,
    /// `m_bRectangularTrim` (an `SmBoolean`: `1` the face has one rectangular outer
    /// loop of four iso trim curves spanning [`Face::uv_domain`], `0` it has not,
    /// [`face::p2::UNSURE`] not yet known).
    pub rectangular_trim: i32,
    /// `m_sZoneTol3d` (`SM_USE_OLDTOL`): the face tolerance (`GetTolerance`).
    pub tolerance: f64,
}

impl Face {
    /// `SmFace()` in `shell`: no loops, surface or face use, the "inside out" UV
    /// domain, rectangular trim [`face::p2::UNSURE`] and tolerance `0`.
    #[must_use]
    pub const fn new(shell: ShellId) -> Self {
        Self {
            shell,
            loops: Vec::new(),
            surface: None,
            faceuse: None,
            uv_domain: Extent2d::new(),
            rectangular_trim: face::p2::UNSURE,
            tolerance: 0.0,
        }
    }
}

/// A shell (`SmShell`): a connected set of faces.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shell {
    /// Faces of the shell, in insertion order. Port-only: upstream reaches them through
    /// the shell's face uses; after a refresh ([`Brep::refresh_cached_topology`]) in
    /// upstream's iteration order, the shell's face-use member order, each face once.
    pub faces: Vec<FaceId>,
    /// `m_pListOwner` (CO-PORTED): the region owning the shell (`GetRegion`).
    pub region: Option<RegionId>,
    /// Native `m_tShellType`, independent of the list head during construction.
    pub shell_type: shell::ShellType,
    /// Native list members in head-first circular traversal order.
    pub members: shell::ShellMembers,
}

/// A region (`SmRegion`, CO-PORTED minimal record).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Region {
    /// `m_pListOwner != NULL`: the region is on the B-rep's region list, so
    /// `GetBrep` reaches the B-rep.
    pub in_brep: bool,
    /// Native region shell list, in traversal order from its head.
    pub shells: Vec<ShellId>,
    /// Native `SmBoolean` `m_bIsVoidFlag` (0 at construction; 2 is `UNSURE`).
    pub is_void: i32,
}

/// One use of a face (`SmFaceuse`): the face seen from one side.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Faceuse {
    /// `m_pListOwner`: the shell owning the face use (`GetShell`).
    pub shell: Option<ShellId>,
    /// `m_pF`: the face used (`GetFace`).
    pub face: Option<FaceId>,
    /// `m_pFUMate`: the face's other face use (`GetMate`).
    pub mate: Option<FaceuseId>,
    /// `m_eOrientation`: [`OrientType::Same`] when the face use normal is the
    /// surface normal.
    pub orientation: OrientType,
    /// `m_pList`: the loop uses, outer loop use first.
    pub loopuses: Vec<LoopuseId>,
    /// Literal sibling links when present, even if the owner has no list head.
    /// `None` uses the owner's canonical face-use vector during construction.
    pub sibling_links: Option<shell::FaceuseSiblingLinks>,
}

/// What a loop use starts from (`m_tLoopuseType` and `m_pEUorVU`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopuseStart {
    /// `SmUnknown_TYPE` (a loop use not yet built).
    #[default]
    Unknown,
    /// `SmEdgeuse_TYPE`: the loop's start edge use.
    Edgeuse(Option<EdgeuseId>),
    /// `SmVertexuse_TYPE`: the single vertex use of a vertex loop.
    Vertexuse(Option<VertexuseId>),
}

/// One use of a loop (`SmLoopuse`; `Default` is `SmLoopuse()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Loopuse {
    /// `m_pListOwner` as an `SmFaceuse` (`GetFaceuse`).
    pub faceuse: Option<FaceuseId>,
    /// `m_pL`: the loop used (`GetLoop`).
    pub loop_: Option<LoopId>,
    /// `m_pLUMate`: the loop use on the face's other side (`GetOtherLoopuse`).
    pub mate: Option<LoopuseId>,
    /// `m_tLoopuseType` and `m_pEUorVU`.
    pub start: LoopuseStart,
    /// `m_eOrientation` (unit `topology.loopuse`): [`OrientType::Same`] for an outer
    /// loop use, [`OrientType::Opposite`] for an inner one.
    pub orientation: OrientType,
}

/// What an edge use connects to (`m_tEdgeuseType` and `m_pSorLU`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeuseOwner {
    /// `SmUnknown_TYPE` (the constructor's value).
    #[default]
    Unknown,
    /// `SmLoopuse_TYPE`: an edge in a face, connected through a loop use.
    Loopuse(Option<LoopuseId>),
    /// `SmShell_TYPE`: a wire edge, connected to a region through a shell.
    Shell(Option<ShellId>),
}

/// One use of an edge (`SmEdgeuse`): the edge seen from one face side or one wire
/// end.
#[derive(Debug, Clone)]
pub struct Edgeuse {
    /// `m_pListOwner`: the edge used (`GetEdge`).
    pub edge: Option<EdgeId>,
    /// `m_tEdgeuseType` and `m_pSorLU`.
    pub owner: EdgeuseOwner,
    /// `m_pCCW`: the next edge use counter-clockwise in the loop.
    pub ccw: Option<EdgeuseId>,
    /// `m_pCW`: the next edge use clockwise in the loop.
    pub cw: Option<EdgeuseId>,
    /// `m_pVU`: the vertex use at the edge use's start.
    pub vertexuse: Option<VertexuseId>,
    /// `m_eOrientation`: [`OrientType::Same`] when the edge use runs with its edge's
    /// curve.
    pub orientation: OrientType,
    /// `m_pUVTrimCurve`: the 2D trim curve, kept on the [`OrientType::Same`] member
    /// of a mated pair (the edge use is its owner). Upstream's `SmBSplineCurve*`: a
    /// B-spline curve or one of its derived kinds (`SmLine`, ...).
    pub uv_trim_curve: Option<Curve>,
    /// `m_sMaxEdgeFaceGap3d`: the cached largest edge/face gap.
    pub max_edge_face_gap: EdgeFaceGap,
    /// `m_sEdgeCCWEdgeGap3d`: the cached gap to the counter-clockwise edge use.
    pub edge_ccw_edge_gap: EdgeEdgeGap,
    /// `m_pDropCurveFail`: what the last `CreateUVTrimCurve` recorded.
    pub drop_curve_fail: Option<Box<DropCurveFail<ObjectHandle>>>,
}

impl Default for Edgeuse {
    /// `SmEdgeuse()`: unlinked, unknown type and orientation, gaps reset.
    fn default() -> Self {
        Self {
            edge: None,
            owner: EdgeuseOwner::Unknown,
            ccw: None,
            cw: None,
            vertexuse: None,
            orientation: OrientType::Unknown,
            uv_trim_curve: None,
            max_edge_face_gap: EdgeFaceGap::new(),
            edge_ccw_edge_gap: EdgeEdgeGap::new(),
            drop_curve_fail: None,
        }
    }
}

/// What a vertex use connects to (`m_tVertexuseType` and `m_pSorLUorEU`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VertexuseOwner {
    /// `SmUnknown_TYPE`.
    #[default]
    Unknown,
    /// `SmEdgeuse_TYPE`: the vertex at an edge use's start.
    Edgeuse(Option<EdgeuseId>),
    /// `SmLoopuse_TYPE`: the vertex of a vertex loop.
    Loopuse(Option<LoopuseId>),
    /// `SmShell_TYPE`: a vertex alone in a region.
    Shell(Option<ShellId>),
}

/// One use of a vertex (`SmVertexuse`; the methods are in unit `topology.vertexuse`,
/// [`vertexuse`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertexuse {
    /// `m_pListOwner`: the vertex used (`GetVertex`).
    pub vertex: Option<VertexId>,
    /// `m_tVertexuseType` and `m_pSorLUorEU`.
    pub owner: VertexuseOwner,
    /// `m_sVertexEdgeGap3d`: the cached vertex/edge gap.
    pub vertex_edge_gap: VertexEdgeGap,
    /// `m_sVertexFaceGap3d`: the cached vertex/face gap.
    pub vertex_face_gap: VertexFaceGap,
    /// `m_sUVPoint` (`GetUVPoint` / `SetUVPoint`; `(SM_BIG_DOUBLE, SM_BIG_DOUBLE)`
    /// until set).
    pub uv_point: Point2,
}

impl Default for Vertexuse {
    /// `SmVertexuse()`: unlinked, unknown type, gaps reset, UV point
    /// `(SM_BIG_DOUBLE, SM_BIG_DOUBLE)`.
    fn default() -> Self {
        Self {
            vertex: None,
            owner: VertexuseOwner::Unknown,
            vertex_edge_gap: VertexEdgeGap::new(),
            vertex_face_gap: VertexFaceGap::new(),
            uv_point: Point2::new(BIG_DOUBLE, BIG_DOUBLE),
        }
    }
}

/// How a B-rep names its topology entities as the owners of its geometry (upstream's
/// owner pointers): the port has no object pointers, so a geometry owner is an
/// [`ObjectRef`](crate::geom::object::ObjectRef) under one of these schemes. A routine
/// that attaches new geometry to topology (`SmEdgeuse::SetUVTrimCurve`'s `SetOwner`)
/// names its owner by the B-rep's scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NamingScheme {
    /// Not recorded: such routines leave the new geometry without owner (the caller
    /// names it with its own identities), except a UV trim curve, which
    /// `SetUVTrimCurve` always owns by its holder: under the edge use's identity of
    /// [`NamingScheme::Arena`] (its notification identity).
    #[default]
    Unknown,
    /// The Euler operators' notification identities
    /// (`topology::loopuse::notify_ref`).
    Notify,
    /// A caller's own identities (a `TopologyEnv` / `NotifyEnv` `key_of`), recorded per
    /// edge, face and edge use where the B-rep was built (`Brep::geometry_owner_keys`):
    /// such routines name the new geometry by the recorded key, and leave it without
    /// owner when none is recorded.
    Caller,
    /// The arena identities the crate's imported B-reps use
    /// ([`brep_cache::arena_identity`](crate::topology::brep_cache::arena_identity):
    /// the entity's type code and arena index).
    Arena,
}

/// A boundary representation (`SmBrep`): arenas of shells, faces, loops, edges and
/// vertices linked by typed ids.
#[derive(Debug)]
pub struct Brep {
    shells: Arena<ShellId, Shell>,
    faces: Arena<FaceId, Face>,
    loops: Arena<LoopId, Loop>,
    edges: Shared<Arena<EdgeId, Edge>>,
    vertices: Arena<VertexId, Vertex>,
    regions: Arena<RegionId, Region>,
    infinite_region: Option<RegionId>,
    faceuses: Arena<FaceuseId, Faceuse>,
    loopuses: Arena<LoopuseId, Loopuse>,
    edgeuses: Shared<Arena<EdgeuseId, Edgeuse>>,
    vertexuses: Arena<VertexuseId, Vertexuse>,
    /// `m_bValidateOnly`.
    validate_only: bool,
    /// `m_sThisZoneTol3d` (CO-PORTED by `topology.edgeuse_p2`): the B-rep's tolerance.
    tolerance: f64,
    /// The identity the B-rep is attributed and notified under (`SmBrep*`).
    key: ObjectKey,
    /// `SmOwningTopology::m_pList` of the B-rep (unit `topology.brep_p1`): the head of
    /// its circular region list, `None` for the first region record.
    region_list_head: Option<RegionId>,
    /// The circular region list's link order (unit `topology.brep_p1`), from
    /// [`Brep::region_list_head`] when it was last moved. Regions on the list
    /// (`Region::in_brep`) that are not in it follow in arena order: that is upstream's
    /// `PostInsert` (append at the tail, before the head). Empty while the list is still
    /// in arena order (the head never moved past an append).
    region_order: Vec<RegionId>,
    /// `m_pFaceListHead` (unit `topology.brep_p1`): the face list in its list order
    /// (every face record, once; an importer may reorder it). Upstream appends on
    /// `PostInsert` and unlinks on `Remove`: `remove_*_record` takes the id off this
    /// list and its siblings below, as `add_*_record` appends it.
    face_list: Vec<FaceId>,
    /// `m_pEdgeListHead` (unit `topology.brep_p1`): the edge list in its list order.
    edge_list: Vec<EdgeId>,
    /// `m_pVertexListHead` (unit `topology.brep_p1`): the vertex list in its list order.
    vertex_list: Vec<VertexId>,
    /// `m_bEditingEnabled` (unit `topology.brep_p1`): the B-rep is being edited, so
    /// trimmed-surface caches skip the trimmed face data.
    editing_enabled: bool,
    /// `m_dThisModelSizeEstimate` (unit `topology.brep_p1`).
    model_size_estimate: f64,
    /// `m_dThisLargeSmallSizeRatio` (unit `topology.brep_p1`).
    large_small_size_ratio: f64,
    /// `m_pDeletedSurfaces` (unit `topology.brep_p1`): surfaces removed from the B-rep
    /// that are kept until the operation ends because they may still be referenced.
    deleted_surfaces: Vec<Surface>,
    /// Faces whose surface's owner a running `SmFace::CreateUVTrimCurves` /
    /// `SmEdgeuse::CreateUVTrimCurve` has temporarily cleared (`SmRescueSurfaceOwner`,
    /// `SmTemporaryChangeValue` on `m_pOwner`), one entry per active clearing. While a
    /// face is listed its surface has no face upstream (`GetFace()` is `NULL`), which
    /// the port's surface owners (not set on every imported face surface) cannot
    /// express on their own.
    surface_owner_cleared: Vec<FaceId>,
    /// (not upstream) How the B-rep names its topology as an owner of geometry
    /// ([`NamingScheme`]).
    naming: NamingScheme,
    /// `m_pLockedBoundingBox` (unit `query.brep`): the box
    /// [`crate::query::brep::lock_bounding_box`] saved, which
    /// [`crate::query::brep::calculate_bounding_box`] returns while it is set. A clone
    /// keeps it (clones stand in for the same B-rep); upstream's copy constructor
    /// (`topology.brep_p3`) starts without one.
    locked_bounding_box: Option<Extent3d>,
    /// The B-rep's kernel-issued notification serial (CO-PORTED by `topology.brep_p3`):
    /// its objects' topology keys hold it (`topology::brep::p3::topology_ref`). A
    /// clone is a new B-rep without one.
    notify_identity: IdentitySlot,
    /// (not upstream) Names this B-rep value for the notify records' seeding progress
    /// (`topology::brep::p3`); a clone is another value without one.
    progress_tag: topology::ProgressTag,
    /// Edge/face/edge-use identities supplied when their geometry was bound. These
    /// record producer provenance; they are never inferred from an opaque geometry
    /// owner.
    geometry_owner_keys: std::collections::BTreeMap<EntityRef, crate::geom::object::ObjectRef>,
    /// `SmAObject::m_pAttributes` of the B-rep's topology objects: the caller's and the
    /// kernel's attributes on its faces, edges, ... (keyed by the B-rep-local
    /// [`loopuse::notify_ref`] identity of each entity). A clone copies them.
    attributes: AttributeStore,
}

/// A copy of the arenas whose curves and surfaces have no owner (the geometry's copy
/// constructor resets it). Operations that find a curve's edge or a surface's face
/// through its owner (the fillet merge, B-rep ray-fire caches, transforms of a body's
/// geometry) therefore do not see the clone's topology: `fillet_edges` of a clone fails
/// in its merge (#1144). To copy a body that is edited later (undo, history, preview),
/// use [`brep::p3::api_brep_copy`] (`SmApiBrepCopy`), whose geometry the copy owns.
/// [`Brep::clone_standing_for_self`] is a private stand-in for this B-rep (its geometry
/// keeps this B-rep's owners) and not an independent body.
impl Clone for Brep {
    fn clone(&self) -> Self {
        self.copy(false)
    }
}

impl Brep {
    /// The copy constructor; with `share` the edge and edge use arenas (the bulk of a
    /// clone: their curves) are shared copy-on-write instead of copied, so their curves
    /// keep their owners until a write copies the arena.
    fn copy(&self, share: bool) -> Self {
        Self {
            shells: self.shells.clone(),
            faces: self.faces.clone(),
            loops: self.loops.clone(),
            edges: self.edges.copy(share),
            vertices: self.vertices.clone(),
            regions: self.regions.clone(),
            infinite_region: self.infinite_region,
            faceuses: self.faceuses.clone(),
            loopuses: self.loopuses.clone(),
            edgeuses: self.edgeuses.copy(share),
            vertexuses: self.vertexuses.clone(),
            validate_only: self.validate_only,
            tolerance: self.tolerance,
            key: self.key,
            region_list_head: self.region_list_head,
            region_order: self.region_order.clone(),
            face_list: self.face_list.clone(),
            edge_list: self.edge_list.clone(),
            vertex_list: self.vertex_list.clone(),
            editing_enabled: self.editing_enabled,
            model_size_estimate: self.model_size_estimate,
            large_small_size_ratio: self.large_small_size_ratio,
            deleted_surfaces: self.deleted_surfaces.clone(),
            surface_owner_cleared: self.surface_owner_cleared.clone(),
            naming: self.naming,
            locked_bounding_box: self.locked_bounding_box,
            notify_identity: self.notify_identity.clone(),
            progress_tag: self.progress_tag.clone(),
            geometry_owner_keys: self.geometry_owner_keys.clone(),
            attributes: self.attributes.clone(),
        }
    }
}

impl Default for Brep {
    fn default() -> Self {
        Self {
            shells: Arena::default(),
            faces: Arena::default(),
            loops: Arena::default(),
            edges: Shared::default(),
            vertices: Arena::default(),
            regions: Arena::default(),
            infinite_region: None,
            faceuses: Arena::default(),
            loopuses: Arena::default(),
            edgeuses: Shared::default(),
            vertexuses: Arena::default(),
            validate_only: false,
            tolerance: ZONE_TOL_3D,
            key: ObjectKey(0),
            region_list_head: None,
            region_order: Vec::new(),
            face_list: Vec::new(),
            edge_list: Vec::new(),
            vertex_list: Vec::new(),
            editing_enabled: false,
            model_size_estimate: USE_DEFAULT,
            large_small_size_ratio: USE_DEFAULT,
            deleted_surfaces: Vec::new(),
            surface_owner_cleared: Vec::new(),
            naming: NamingScheme::Unknown,
            locked_bounding_box: None,
            notify_identity: IdentitySlot::default(),
            progress_tag: topology::ProgressTag::default(),
            geometry_owner_keys: std::collections::BTreeMap::new(),
            attributes: AttributeStore::default(),
        }
    }
}

fn not_found<I: ArenaId>(operation: &'static str, id: I) -> SolidError {
    SolidError::NotFound {
        operation,
        entity: id.entity(),
    }
}

impl Brep {
    /// An empty B-rep.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `SmBrep()` (CO-PORTED by `topology.brep_p10` from unit `topology.brep_p3`, the
    /// topology part only): an empty B-rep whose region list holds the infinite region,
    /// a void region (`SetIsVoid(TRUE)`); editing disabled, tolerance
    /// [`ZONE_TOL_3D`].
    ///
    /// # Errors
    ///
    /// None in practice (the region arena is empty).
    pub fn new_with_infinite_region() -> Result<Self> {
        let mut b = Self::default();
        // Create infinite region and put into brep's region list
        let r = b.regions.insert(Region {
            in_brep: true,
            shells: Vec::new(),
            is_void: 1,
        })?;
        b.infinite_region = Some(r);
        // PreInsert: the infinite region heads the region list
        b.region_list_head = Some(r);
        Ok(b)
    }

    /// `m_pInfiniteRegion` (`GetInfiniteRegion`).
    #[must_use]
    pub const fn infinite_region(&self) -> Option<RegionId> {
        self.infinite_region
    }

    /// Friend write of the B-rep's infinite-region identity. Like other record
    /// writes, this permits references to records added later.
    pub fn set_infinite_region_record(&mut self, region: Option<RegionId>) {
        self.infinite_region = region;
    }

    /// How this B-rep names its topology as the owner of geometry ([`NamingScheme`]).
    #[must_use]
    pub const fn naming(&self) -> NamingScheme {
        self.naming
    }

    /// Records how this B-rep names its topology as the owner of geometry: set where a
    /// B-rep is built or its geometry owners are relabelled.
    pub fn set_naming(&mut self, naming: NamingScheme) {
        self.naming = naming;
    }

    /// Relabels geometry owned by this B-rep's own topology records under
    /// [`NamingScheme::Arena`] ([`brep_cache::arena_identity`]), preserving shared or
    /// unowned geometry and syncing face surface tolerances. An edge use's own UV trim
    /// curve takes the edge use's notification identity, the arena scheme's name for an
    /// edge use (`edgeuse::p1::owner_ref`, `check_brep`).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling face, edge or edge use id (none in
    /// practice).
    pub fn use_arena_naming(&mut self) -> Result<()> {
        let edges: Vec<EdgeId> = self.edges.ids().collect();
        for edge in edges {
            let entity = EntityRef::Edge(edge);
            let arena = brep_cache::arena_identity(brep_cache::CacheObject::Edge(edge));
            let notify = loopuse::notify_ref(entity);
            let proven = self.geometry_owner_key(entity);
            let rebound = self.edge_mut(edge)?.curve.as_mut().is_some_and(|curve| {
                let matched = curve.owner().is_some_and(|o| {
                    o == arena
                        || (o.key, o.type_code) == (notify.key, notify.type_code)
                        || Some(o) == proven
                });
                if matched {
                    curve.set_owner(Some(arena), None);
                }
                matched
            });
            if rebound {
                self.set_geometry_owner_key(entity, Some(arena));
            }
        }
        let uses: Vec<EdgeuseId> = self.edgeuses.ids().collect();
        for eu in uses {
            let entity = EntityRef::Edgeuse(eu);
            let notify = loopuse::notify_ref(entity);
            let proven = self.geometry_owner_key(entity);
            let rebound = self
                .edgeuse_mut(eu)?
                .uv_trim_curve
                .as_mut()
                .is_some_and(|curve| {
                    let matched = curve
                        .owner()
                        .is_some_and(|o| o == notify || Some(o) == proven);
                    if matched {
                        curve.set_owner(Some(notify), None);
                    }
                    matched
                });
            if rebound {
                self.set_geometry_owner_key(entity, Some(notify));
            }
        }
        let faces: Vec<FaceId> = self.faces.ids().collect();
        for face in faces {
            let entity = EntityRef::Face(face);
            let arena = brep_cache::arena_identity(brep_cache::CacheObject::Face(face));
            let notify = loopuse::notify_ref(entity);
            let proven = self.geometry_owner_key(entity);
            let rebound = self
                .face_mut(face)?
                .surface
                .as_mut()
                .is_some_and(|surface| {
                    let matched = surface
                        .owner()
                        .is_some_and(|o| o == arena || o == notify || Some(o) == proven);
                    if matched {
                        surface.set_owner(Some(arena));
                    }
                    matched
                });
            if rebound {
                self.set_geometry_owner_key(entity, Some(arena));
                self.sync_face_surface_tolerance(face)?;
            }
        }
        self.naming = NamingScheme::Arena;
        Ok(())
    }

    /// Producer-supplied owner identity recorded when geometry was bound to `entity`.
    #[must_use]
    pub(crate) fn geometry_owner_key(
        &self,
        entity: EntityRef,
    ) -> Option<crate::geom::object::ObjectRef> {
        self.geometry_owner_keys.get(&entity).copied()
    }

    /// All producer-supplied geometry owner identities recorded on this B-rep.
    #[must_use]
    pub(crate) const fn geometry_owner_keys(
        &self,
    ) -> &std::collections::BTreeMap<EntityRef, crate::geom::object::ObjectRef> {
        &self.geometry_owner_keys
    }

    /// Updates or clears the producer-supplied geometry owner identity of `entity`.
    pub(crate) fn set_geometry_owner_key(
        &mut self,
        entity: EntityRef,
        owner: Option<crate::geom::object::ObjectRef>,
    ) {
        match owner {
            Some(owner) => {
                self.geometry_owner_keys.insert(entity, owner);
            }
            None => {
                self.geometry_owner_keys.remove(&entity);
            }
        }
    }

    /// `m_bEditingEnabled`.
    #[must_use]
    pub const fn editing_enabled(&self) -> bool {
        self.editing_enabled
    }

    /// Sets `m_bEditingEnabled`.
    pub fn set_editing_enabled(&mut self, enabled: bool) {
        self.editing_enabled = enabled;
    }

    /// Whether the owner of face `f`'s surface is temporarily cleared (see
    /// [`Brep::begin_surface_owner_cleared`]): upstream's `GetFace()` of that surface
    /// is then `NULL`.
    #[must_use]
    pub fn is_surface_owner_cleared(&self, f: FaceId) -> bool {
        self.surface_owner_cleared.contains(&f)
    }

    /// Records that the owner of face `f`'s surface is temporarily cleared (the start
    /// of an `SmTemporaryChangeValue<SmObject*>(pSurface->m_pOwner, NULL)` scope);
    /// [`Brep::end_surface_owner_cleared`] ends it.
    pub fn begin_surface_owner_cleared(&mut self, f: FaceId) {
        self.surface_owner_cleared.push(f);
        // the surface object (and its copies) learns it too
        if let Some(s) = self.faces.get(f).and_then(|face| face.surface.as_ref()) {
            s.begin_owner_cleared(f);
        }
    }

    /// A private copy of this B-rep that stands for the B-rep itself (a query's scratch
    /// copy where upstream works on the B-rep): its face surfaces share the cache slots
    /// of this B-rep's ([`Surface::share_cache_slot_of`]), so that the surface caches the
    /// query makes or reuses on the copy are the B-rep's surfaces' own, as upstream's
    /// are. They also keep their owners (and the face tolerance cached on them), so
    /// that `GetFace()` of the copy's surfaces finds the face as upstream's in-place
    /// queries do (`SmTol::GetZoneTol3d(pSurface)` reads the face tolerance). A plain
    /// `clone` (the copy constructor) starts every surface without cache or owner.
    #[must_use]
    pub fn clone_sharing_surface_caches(&self) -> Self {
        let mut copy = self.copy(true);
        for (id, face) in self.faces.iter() {
            if let (Some(original), Ok(face_copy)) = (face.surface.as_ref(), copy.face_mut(id)) {
                if let Some(surface) = face_copy.surface.as_mut() {
                    surface.share_cache_slot_of(original);
                    face::p2::RescueSurfaceOwner::new(original).restore(surface);
                }
            }
        }
        copy
    }

    /// A private copy of this B-rep that stands for the B-rep itself where upstream
    /// queries the B-rep in place (a `const` query that still makes missing UV trim
    /// curves on it): its edge curves, face surfaces and UV trim curves are exact copies
    /// that keep their owners (and the surfaces share their cache slots), so the copy's
    /// geometry still names its own edges and faces. A plain `clone` (the copy
    /// constructor) resets every owner, and a B-rep cache made from it then holds no
    /// curve or surface.
    ///
    /// # Errors
    ///
    /// Those of the geometry copies (a geometry nesting deeper than
    /// `MAX_GEOMETRY_NESTING`).
    pub fn clone_standing_for_self(&self) -> Result<Self> {
        let mut copy = self.clone();
        for (id, edge) in self.edges.iter() {
            if let Some(curve) = edge.curve.as_ref() {
                copy.edge_mut(id)?.curve = Some(curve.clone_for_evaluation()?);
            }
        }
        for (id, face) in self.faces.iter() {
            if let Some(surface) = face.surface.as_ref() {
                copy.face_mut(id)?.surface = Some(surface.clone_for_evaluation()?);
            }
        }
        for (id, edgeuse) in self.edgeuses.iter() {
            if let Some(curve) = edgeuse.uv_trim_curve.as_ref() {
                copy.edgeuse_mut(id)?.uv_trim_curve = Some(curve.clone_for_evaluation()?);
            }
        }
        Ok(copy)
    }

    /// A copy of this B-rep to put back with [`Brep::restore_from`] if an operation
    /// on it is refused (not upstream: an operation's rollback). The edge and edge use
    /// arenas are shared copy-on-write, so their curves keep their owners, and the face
    /// surfaces keep theirs; the surfaces start with empty cache slots (a cache the
    /// operation builds on the edited faces is not one of the copy's), and the copy's
    /// objects, being copies, carry no notification identities.
    #[must_use]
    pub fn clone_for_rollback(&self) -> Self {
        let mut copy = self.copy(true);
        for (id, face) in self.faces.iter() {
            if let (Some(original), Ok(face_copy)) = (face.surface.as_ref(), copy.face_mut(id)) {
                if let Some(surface) = face_copy.surface.as_mut() {
                    face::p2::RescueSurfaceOwner::new(original).restore(surface);
                }
            }
        }
        copy
    }

    /// Puts back the contents of `saved`, a [`Brep::clone_for_rollback`] of this
    /// B-rep taken earlier, keeping this value's own identity (its notification
    /// identity and progress tag, which a copy does not carry), so the B-rep is again
    /// the one it was when `saved` was taken (not upstream: an operation's rollback).
    pub fn restore_from(&mut self, mut saved: Self) {
        std::mem::swap(&mut saved.notify_identity, &mut self.notify_identity);
        std::mem::swap(&mut saved.progress_tag, &mut self.progress_tag);
        *self = saved;
    }

    /// Ends the innermost [`Brep::begin_surface_owner_cleared`] scope of face `f`.
    pub fn end_surface_owner_cleared(&mut self, f: FaceId) {
        if let Some(i) = self.surface_owner_cleared.iter().rposition(|&g| g == f) {
            self.surface_owner_cleared.remove(i);
        }
        if let Some(s) = self.faces.get(f).and_then(|face| face.surface.as_ref()) {
            s.end_owner_cleared(f);
        }
    }

    /// `m_pLockedBoundingBox` (unit `query.brep`): the locked bounding box, if any.
    #[must_use]
    pub const fn locked_bounding_box(&self) -> Option<Extent3d> {
        self.locked_bounding_box
    }

    /// Sets (`Some`) or clears (`None`) `m_pLockedBoundingBox`; see
    /// [`crate::query::brep::lock_bounding_box`] and
    /// [`crate::query::brep::unlock_bounding_box`].
    pub fn set_locked_bounding_box(&mut self, bbox: Option<Extent3d>) {
        self.locked_bounding_box = bbox;
    }

    /// All shells.
    #[must_use]
    pub fn shells(&self) -> &Arena<ShellId, Shell> {
        &self.shells
    }

    /// All faces.
    #[must_use]
    pub fn faces(&self) -> &Arena<FaceId, Face> {
        &self.faces
    }

    /// All loops.
    #[must_use]
    pub fn loops(&self) -> &Arena<LoopId, Loop> {
        &self.loops
    }

    /// All edges.
    #[must_use]
    pub fn edges(&self) -> &Arena<EdgeId, Edge> {
        &self.edges
    }

    /// All vertices.
    #[must_use]
    pub fn vertices(&self) -> &Arena<VertexId, Vertex> {
        &self.vertices
    }

    /// The shell `id`.
    pub fn shell(&self, id: ShellId) -> Result<&Shell> {
        self.shells
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::shell", id))
    }

    /// The face `id`.
    pub fn face(&self, id: FaceId) -> Result<&Face> {
        crate::limits::probe();
        self.faces
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::face", id))
    }

    /// The loop `id`.
    pub fn loop_(&self, id: LoopId) -> Result<&Loop> {
        self.loops
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::loop_", id))
    }

    /// The edge `id`.
    pub fn edge(&self, id: EdgeId) -> Result<&Edge> {
        crate::limits::probe();
        self.edges
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::edge", id))
    }

    /// The vertex `id`.
    pub fn vertex(&self, id: VertexId) -> Result<&Vertex> {
        self.vertices
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::vertex", id))
    }

    /// All regions.
    #[must_use]
    pub fn regions(&self) -> &Arena<RegionId, Region> {
        &self.regions
    }

    /// All face uses.
    #[must_use]
    pub fn faceuses(&self) -> &Arena<FaceuseId, Faceuse> {
        &self.faceuses
    }

    /// All loop uses.
    #[must_use]
    pub fn loopuses(&self) -> &Arena<LoopuseId, Loopuse> {
        &self.loopuses
    }

    /// All edge uses.
    #[must_use]
    pub fn edgeuses(&self) -> &Arena<EdgeuseId, Edgeuse> {
        &self.edgeuses
    }

    /// All vertex uses.
    #[must_use]
    pub fn vertexuses(&self) -> &Arena<VertexuseId, Vertexuse> {
        &self.vertexuses
    }

    /// The region `id`.
    pub fn region(&self, id: RegionId) -> Result<&Region> {
        self.regions
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::region", id))
    }

    /// The face use `id`.
    pub fn faceuse(&self, id: FaceuseId) -> Result<&Faceuse> {
        crate::limits::probe();
        self.faceuses
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::faceuse", id))
    }

    /// The loop use `id`.
    pub fn loopuse(&self, id: LoopuseId) -> Result<&Loopuse> {
        crate::limits::probe();
        self.loopuses
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::loopuse", id))
    }

    /// The edge use `id`.
    pub fn edgeuse(&self, id: EdgeuseId) -> Result<&Edgeuse> {
        crate::limits::probe();
        self.edgeuses
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::edgeuse", id))
    }

    /// The vertex use `id`.
    pub fn vertexuse(&self, id: VertexuseId) -> Result<&Vertexuse> {
        crate::limits::probe();
        self.vertexuses
            .get(id)
            .ok_or_else(|| not_found("topology::Brep::vertexuse", id))
    }

    /// Mutable access to the shell `id`.
    pub fn shell_mut(&mut self, id: ShellId) -> Result<&mut Shell> {
        self.shells
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::shell_mut", id))
    }

    /// Mutable access to the face `id`.
    pub fn face_mut(&mut self, id: FaceId) -> Result<&mut Face> {
        self.faces
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::face_mut", id))
    }

    /// Mutable access to the loop `id`.
    pub fn loop_mut(&mut self, id: LoopId) -> Result<&mut Loop> {
        self.loops
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::loop_mut", id))
    }

    /// Mutable access to the edge `id`.
    pub fn edge_mut(&mut self, id: EdgeId) -> Result<&mut Edge> {
        self.edges
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::edge_mut", id))
    }

    /// Mutable access to the vertex `id`.
    pub fn vertex_mut(&mut self, id: VertexId) -> Result<&mut Vertex> {
        self.vertices
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::vertex_mut", id))
    }

    /// Mutable access to the region `id`.
    pub fn region_mut(&mut self, id: RegionId) -> Result<&mut Region> {
        self.regions
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::region_mut", id))
    }

    /// Mutable access to the face use `id`.
    pub fn faceuse_mut(&mut self, id: FaceuseId) -> Result<&mut Faceuse> {
        self.faceuses
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::faceuse_mut", id))
    }

    /// Mutable access to the loop use `id`.
    pub fn loopuse_mut(&mut self, id: LoopuseId) -> Result<&mut Loopuse> {
        self.loopuses
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::loopuse_mut", id))
    }

    /// Mutable access to the edge use `id`.
    pub fn edgeuse_mut(&mut self, id: EdgeuseId) -> Result<&mut Edgeuse> {
        self.edgeuses
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::edgeuse_mut", id))
    }

    /// Mutable access to the vertex use `id`.
    pub fn vertexuse_mut(&mut self, id: VertexuseId) -> Result<&mut Vertexuse> {
        self.vertexuses
            .get_mut(id)
            .ok_or_else(|| not_found("topology::Brep::vertexuse_mut", id))
    }

    /// `m_bValidateOnly`: while set, tolerances must not change.
    #[must_use]
    pub const fn validate_only(&self) -> bool {
        self.validate_only
    }

    /// Sets `m_bValidateOnly`.
    pub fn set_validate_only(&mut self, validate_only: bool) {
        self.validate_only = validate_only;
    }

    /// `SmBrep::GetTolerance` (`SM_USE_OLDTOL`, CO-PORTED by `topology.edgeuse_p2`):
    /// `m_sThisZoneTol3d`, [`ZONE_TOL_3D`] for a new B-rep (`SmTol::GetZoneTol3d` of
    /// the default context, as `SmBrep()`).
    #[must_use]
    pub const fn tolerance(&self) -> f64 {
        self.tolerance
    }

    /// Sets `m_sThisZoneTol3d` (the raw member write; `SmBrep::SetTolerance`, which
    /// also updates the entities, is unit `topology.brep_*`).
    pub fn set_tolerance(&mut self, tolerance: f64) {
        self.tolerance = tolerance;
    }

    /// The identity the B-rep is attributed and notified under (an `SmBrep*`).
    #[must_use]
    pub const fn key(&self) -> ObjectKey {
        self.key
    }

    /// Sets the identity the B-rep is attributed and notified under.
    pub fn set_key(&mut self, key: ObjectKey) {
        self.key = key;
    }

    /// Whether `e` names a live topology object of this B-rep that can carry
    /// attributes (every kind but the B-rep itself).
    #[must_use]
    pub fn is_live_entity(&self, e: EntityRef) -> bool {
        match e {
            EntityRef::Brep(_) => false,
            EntityRef::Shell(x) => self.shells.get(x).is_some(),
            EntityRef::Face(x) => self.faces.get(x).is_some(),
            EntityRef::Loop(x) => self.loops.get(x).is_some(),
            EntityRef::Edge(x) => self.edges.get(x).is_some(),
            EntityRef::Vertex(x) => self.vertices.get(x).is_some(),
            EntityRef::Region(x) => self.regions.get(x).is_some(),
            EntityRef::Faceuse(x) => self.faceuses.get(x).is_some(),
            EntityRef::Loopuse(x) => self.loopuses.get(x).is_some(),
            EntityRef::Edgeuse(x) => self.edgeuses.get(x).is_some(),
            EntityRef::Vertexuse(x) => self.vertexuses.get(x).is_some(),
        }
    }

    /// The faces, edges and vertices on the B-rep's lists (an edit can unlink one and
    /// leave its record in the arena: upstream's `delete` of it), for
    /// [`Brep::still_has`].
    pub(crate) fn listed_entities(&self) -> std::collections::BTreeSet<EntityRef> {
        self.face_list()
            .iter()
            .map(|&f| EntityRef::Face(f))
            .chain(self.edge_list().iter().map(|&e| EntityRef::Edge(e)))
            .chain(self.vertex_list().iter().map(|&v| EntityRef::Vertex(v)))
            .collect()
    }

    /// Whether `e` is an object the B-rep still has, as an operation that renamed its
    /// objects hands their attributes back ([`Brep::is_live_entity`], and for a face,
    /// edge or vertex on its list, `listed` from [`Brep::listed_entities`]): an object
    /// upstream deleted has cleared its attributes.
    pub(crate) fn still_has(
        &self,
        listed: &std::collections::BTreeSet<EntityRef>,
        e: EntityRef,
    ) -> bool {
        self.is_live_entity(e)
            && match e {
                EntityRef::Face(_) | EntityRef::Edge(_) | EntityRef::Vertex(_) => {
                    listed.contains(&e)
                }
                _ => true,
            }
    }

    fn attributed_key(&self, e: EntityRef, operation: &'static str) -> Result<ObjectKey> {
        if self.is_live_entity(e) {
            Ok(loopuse::notify_ref(e).key)
        } else {
            Err(SolidError::NotFound {
                operation,
                entity: e,
            })
        }
    }

    /// `SmAObject::AddAttribute(new attribute)`: gives `e` a new attribute and returns
    /// its handle. As upstream, a second attribute with the same id is added too
    /// ([`Brep::find_attribute`] finds the first). Operations carry it to the objects
    /// they make from `e` as its [`Behavior`](crate::geom::attribute::Behavior) says
    /// (SMLib's default handlers: split, copy and merge pass it on to an object that
    /// has no attribute of that id; temporary attributes are not passed on).
    /// Application attribute ids should be above 10000, as upstream asks.
    ///
    /// # Errors
    /// [`SolidError::NotFound`] when `e` is not a live object of this B-rep (or is the
    /// B-rep itself).
    pub fn add_attribute(&mut self, e: EntityRef, attribute: Attribute) -> Result<AttributeHandle> {
        let key = self.attributed_key(e, "topology::Brep::add_attribute")?;
        let h = self.attributes.insert(attribute)?;
        self.attributes.add_attribute(key, h)?;
        Ok(h)
    }

    /// `SmAObject::AddAttribute(existing attribute)`: puts an attribute this B-rep
    /// already holds on `e` as well (a reference attribute shared by several objects).
    /// Adding the attribute `e` already finds first under its id does nothing.
    ///
    /// # Errors
    /// [`SolidError::NotFound`] for a dead entity; [`SolidError::InvalidInput`] for a
    /// handle that is not live in this B-rep.
    pub fn attach_attribute(&mut self, e: EntityRef, h: AttributeHandle) -> Result<()> {
        let key = self.attributed_key(e, "topology::Brep::attach_attribute")?;
        self.attributes.add_attribute(key, h)
    }

    /// `SmAObject::FindAttribute(lAttributeID)`: the first attribute of `e` with that
    /// id (`None` for a dead entity).
    #[must_use]
    pub fn find_attribute(&self, e: EntityRef, attribute_id: u64) -> Option<&Attribute> {
        let key = self
            .attributed_key(e, "topology::Brep::find_attribute")
            .ok()?;
        self.attributes
            .find_attribute(key, attribute_id)
            .and_then(|h| self.attributes.get(h))
    }

    /// `SmAObject::GetAttributes`: every attribute of `e` with its handle, in the order
    /// they were added (empty for a dead entity).
    #[must_use]
    pub fn entity_attributes(&self, e: EntityRef) -> Vec<(AttributeHandle, &Attribute)> {
        let Ok(key) = self.attributed_key(e, "topology::Brep::entity_attributes") else {
            return Vec::new();
        };
        self.attributes
            .attributes(key)
            .into_iter()
            .filter_map(|h| self.attributes.get(h).map(|a| (h, a)))
            .collect()
    }

    /// `SmAObject::RemoveAttribute(pAttribute)`: takes the attribute off `e`; it is
    /// deleted when that was its last user, unless it is standalone. Returns whether it
    /// was deleted (an attribute `e` does not carry is left alone).
    ///
    /// # Errors
    /// [`SolidError::NotFound`] for a dead entity.
    pub fn remove_attribute(&mut self, e: EntityRef, h: AttributeHandle) -> Result<bool> {
        let key = self.attributed_key(e, "topology::Brep::remove_attribute")?;
        self.attributes.remove_attribute(key, h, false)
    }

    /// `SmAObject::ClearAttributes`: takes every attribute off `e`, deleting each
    /// dependent one it was the last user of.
    ///
    /// # Errors
    /// [`SolidError::NotFound`] for a dead entity.
    pub fn clear_attributes(&mut self, e: EntityRef) -> Result<()> {
        let key = self.attributed_key(e, "topology::Brep::clear_attributes")?;
        self.attributes.clear_attributes(key)
    }

    /// The attribute `h` of this B-rep, if live (also a standalone attribute that no
    /// object carries any more).
    #[must_use]
    pub fn attribute(&self, h: AttributeHandle) -> Option<&Attribute> {
        self.attributes.get(h)
    }

    /// Every live entity carrying at least one attribute, in identity order.
    #[must_use]
    pub fn attributed_entities(&self) -> Vec<EntityRef> {
        self.attributes
            .attributed_objects()
            .into_iter()
            .filter_map(loopuse::entity_of_notify_key)
            .filter(|&e| self.is_live_entity(e))
            .collect()
    }

    /// The B-rep's attribute store, read-only (keyed by each entity's B-rep-local
    /// notification identity).
    #[must_use]
    pub fn attribute_store(&self) -> &AttributeStore {
        &self.attributes
    }

    /// Takes the attribute store out, leaving an empty one (for an operation that
    /// renames the objects while it runs).
    pub(crate) fn take_attributes(&mut self) -> AttributeStore {
        core::mem::take(&mut self.attributes)
    }

    /// Replaces the attribute store (keyed by [`loopuse::notify_ref`]).
    pub(crate) fn set_attributes(&mut self, attributes: AttributeStore) {
        self.attributes = attributes;
    }

    /// The serial a context issued this B-rep for its notifications, once one has
    /// (the kernel issues it; there is no setter).
    #[must_use]
    pub fn notify_serial(&self) -> Option<u64> {
        self.notify_identity.value()
    }

    /// The slot of the B-rep's notification serial, for the context that issues it.
    pub(crate) fn notify_identity_slot(&mut self) -> &mut IdentitySlot {
        &mut self.notify_identity
    }

    /// The slot of the B-rep's notification serial, read-only.
    pub(crate) const fn notify_identity(&self) -> &IdentitySlot {
        &self.notify_identity
    }

    /// The tag naming this B-rep value for the notify records' seeding progress.
    pub(crate) fn progress_tag(&mut self) -> Arc<()> {
        self.progress_tag.get()
    }

    /// Appends a region record as is (no validation).
    pub fn add_region_record(&mut self, region: Region) -> Result<RegionId> {
        self.regions.insert(region)
    }

    /// Appends a shell record as is (no validation).
    pub fn add_shell_record(&mut self, shell: Shell) -> Result<ShellId> {
        self.shells.insert(shell)
    }

    /// Appends a face record as is (no validation; the shell's face list is not
    /// touched); the face joins the end of the B-rep's face list, and its surface
    /// records the face's tolerance ([`Brep::sync_face_surface_tolerance`]).
    pub fn add_face_record(&mut self, face: Face) -> Result<FaceId> {
        let id = self.faces.insert(face)?;
        self.face_list.push(id);
        self.sync_face_surface_tolerance(id)?;
        Ok(id)
    }

    /// Records the face's tolerance on its surface
    /// ([`Surface::set_owner_zone_tol_3d`](crate::surface::surface::Surface::set_owner_zone_tol_3d)),
    /// so the surface's `GetFace()->GetTolerance()` reads it: the B-rep invariant that a
    /// face record's surface carries that record's `tolerance`. A surface reaching no
    /// face records nothing. Called wherever a face's surface or tolerance changes.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling id.
    pub fn sync_face_surface_tolerance(&mut self, f: FaceId) -> Result<()> {
        let face = self.face_mut(f)?;
        let tol = face.tolerance;
        if let Some(surface) = face.surface.as_mut() {
            surface.set_owner_zone_tol_3d(Some(tol));
        }
        Ok(())
    }

    /// Appends a loop record as is (no validation; the face's loop list is not
    /// touched).
    pub fn add_loop_record(&mut self, loop_: Loop) -> Result<LoopId> {
        self.loops.insert(loop_)
    }

    /// Appends an edge record as is (no validation); the edge joins the end of the
    /// B-rep's edge list.
    pub fn add_edge_record(&mut self, edge: Edge) -> Result<EdgeId> {
        let id = self.edges.insert(edge)?;
        self.edge_list.push(id);
        Ok(id)
    }

    /// Appends a vertex record as is; the vertex joins the end of the B-rep's vertex
    /// list.
    pub fn add_vertex_record(&mut self, vertex: Vertex) -> Result<VertexId> {
        let id = self.vertices.insert(vertex)?;
        self.vertex_list.push(id);
        Ok(id)
    }

    /// Allocates a vertex retained by another topology object, outside the B-rep's
    /// owning vertex list.
    pub(crate) fn add_unlinked_vertex_record(&mut self, vertex: Vertex) -> Result<VertexId> {
        self.vertices.insert(vertex)
    }

    /// Appends a face use record as is (no validation).
    pub fn add_faceuse_record(&mut self, faceuse: Faceuse) -> Result<FaceuseId> {
        self.faceuses.insert(faceuse)
    }

    /// Appends a loop use record as is (no validation).
    pub fn add_loopuse_record(&mut self, loopuse: Loopuse) -> Result<LoopuseId> {
        self.loopuses.insert(loopuse)
    }

    /// Appends an edge use record as is (no validation).
    pub fn add_edgeuse_record(&mut self, edgeuse: Edgeuse) -> Result<EdgeuseId> {
        self.edgeuses.insert(edgeuse)
    }

    /// Appends a vertex use record as is (no validation).
    pub fn add_vertexuse_record(&mut self, vertexuse: Vertexuse) -> Result<VertexuseId> {
        self.vertexuses.insert(vertexuse)
    }

    /// Deletes a shell record (upstream `delete`; no unlinking: the caller has
    /// removed it from its lists first, as the friend code does upstream).
    pub fn remove_shell_record(&mut self, id: ShellId) -> Result<Shell> {
        self.shells
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_shell_record", id))
    }

    /// Deletes a region record (upstream `delete`; no unlinking: the caller resets the
    /// region list head and the infinite region when they named it).
    pub fn remove_region_record(&mut self, id: RegionId) -> Result<Region> {
        let region = self
            .regions
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_region_record", id))?;
        self.region_order.retain(|&x| x != id);
        Ok(region)
    }

    /// Deletes a face record (upstream `m_pFaceListHead->Remove` and `delete`): the
    /// face leaves the B-rep's face list; no other unlinking.
    pub fn remove_face_record(&mut self, id: FaceId) -> Result<Face> {
        let face = self
            .faces
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_face_record", id))?;
        self.face_list.retain(|&x| x != id);
        self.geometry_owner_keys.remove(&EntityRef::Face(id));
        Ok(face)
    }

    /// Deletes a loop record (upstream `delete`; no unlinking).
    pub fn remove_loop_record(&mut self, id: LoopId) -> Result<Loop> {
        self.loops
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_loop_record", id))
    }

    /// Deletes an edge record (upstream `m_pEdgeListHead->Remove` and `delete`): the
    /// edge leaves the B-rep's edge list; no other unlinking.
    pub fn remove_edge_record(&mut self, id: EdgeId) -> Result<Edge> {
        let edge = self
            .edges
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_edge_record", id))?;
        self.edge_list.retain(|&x| x != id);
        self.geometry_owner_keys.remove(&EntityRef::Edge(id));
        Ok(edge)
    }

    /// Deletes a vertex record (upstream `m_pVertexListHead->Remove` and `delete`):
    /// the vertex leaves the B-rep's vertex list; no other unlinking.
    pub fn remove_vertex_record(&mut self, id: VertexId) -> Result<Vertex> {
        let vertex = self
            .vertices
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_vertex_record", id))?;
        self.vertex_list.retain(|&x| x != id);
        Ok(vertex)
    }

    /// Deletes a face use record (upstream `delete`; no unlinking).
    pub fn remove_faceuse_record(&mut self, id: FaceuseId) -> Result<Faceuse> {
        self.faceuses
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_faceuse_record", id))
    }

    /// Deletes a loop use record (upstream `delete`; no unlinking).
    pub fn remove_loopuse_record(&mut self, id: LoopuseId) -> Result<Loopuse> {
        self.loopuses
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_loopuse_record", id))
    }

    /// Deletes an edge use record (upstream `delete`; no unlinking).
    pub fn remove_edgeuse_record(&mut self, id: EdgeuseId) -> Result<Edgeuse> {
        let record = self
            .edgeuses
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_edgeuse_record", id))?;
        self.geometry_owner_keys.remove(&EntityRef::Edgeuse(id));
        Ok(record)
    }

    /// Deletes a vertex use record (upstream `delete`; no unlinking).
    pub fn remove_vertexuse_record(&mut self, id: VertexuseId) -> Result<Vertexuse> {
        self.vertexuses
            .remove(id)
            .ok_or_else(|| not_found("topology::Brep::remove_vertexuse_record", id))
    }

    /// Adds an empty shell.
    pub fn add_shell(&mut self) -> Result<ShellId> {
        self.shells.insert(Shell::default())
    }

    /// Adds a vertex at `point`.
    pub fn add_vertex(&mut self, point: Point3) -> Result<VertexId> {
        self.add_vertex_record(Vertex {
            point,
            tolerance: ZONE_TOL_3D,
            ..Vertex::default()
        })
    }

    /// Rewrites the port's cached topology fields from the links upstream reads them
    /// through (not an upstream port: upstream stores none of them). [`Edge::vertices`]
    /// from the edge uses ([`edge::start_end_vertices`], `GetStartVertex` /
    /// `GetEndVertex`); [`Face::shell`] from the primary face use (`GetShell`);
    /// [`Face::loops`] from the primary face use's loop uses; [`Loop::face`] through the
    /// primary loop use and its face use (`GetFace`); [`Loop::edges`] from the primary
    /// loop use's edge-use cycle; [`Shell::faces`] from the shell's face uses. The public
    /// operations call it before they return, so the fields are true for their callers;
    /// topology edits inside an operation maintain only the use links. Only these
    /// fields change, and never a result: a field whose links are missing or cannot be
    /// read keeps its value.
    pub fn refresh_cached_topology(&mut self) {
        use crate::topology::integrity as derive;
        let ids: Vec<EdgeId> = self.edges.ids().collect();
        for e in ids {
            let has_uses = self.edges.get(e).is_some_and(|r| !r.edgeuses.is_empty());
            if !has_uses {
                continue;
            }
            if let Ok((Some(start), Some(end))) = edge::start_end_vertices(self, e) {
                if let Some(r) = self.edges.get_mut(e) {
                    r.vertices = [start, end];
                }
            }
        }
        let ids: Vec<FaceId> = self.faces.ids().collect();
        for f in ids {
            let shell = derive::derived_face_shell(self, f);
            let loops = derive::derived_face_loops(self, f);
            if let Some(r) = self.faces.get_mut(f) {
                if let Some(shell) = shell {
                    r.shell = shell;
                }
                if let Some(loops) = loops {
                    r.loops = loops;
                }
            }
        }
        let ids: Vec<LoopId> = self.loops.ids().collect();
        for l in ids {
            let face = derive::derived_loop_face(self, l);
            let edges = derive::derived_loop_edges(self, l);
            if let Some(r) = self.loops.get_mut(l) {
                if let Some(face) = face {
                    r.face = face;
                }
                if let Some(edges) = edges {
                    r.edges = edges;
                }
            }
        }
        let ids: Vec<ShellId> = self.shells.ids().collect();
        for s in ids {
            if let Some(faces) = derive::derived_shell_faces(self, s) {
                if let Some(r) = self.shells.get_mut(s) {
                    r.faces = faces;
                }
            }
        }
    }

    /// Adds an edge from `start` to `end`; both vertices must exist.
    pub fn add_edge(&mut self, start: VertexId, end: VertexId) -> Result<EdgeId> {
        const OP: &str = "topology::Brep::add_edge";
        for v in [start, end] {
            if self.vertices.get(v).is_none() {
                return Err(not_found(OP, v));
            }
        }
        self.add_edge_record(Edge {
            vertices: [start, end],
            edgeuses: Vec::new(),
            interval: Extent1d::new(),
            tolerance: ZONE_TOL_3D,
            curve: None,
        })
    }

    /// Adds a face to `shell` with no loops yet.
    pub fn add_face(&mut self, shell: ShellId) -> Result<FaceId> {
        const OP: &str = "topology::Brep::add_face";
        if self.shells.get(shell).is_none() {
            return Err(not_found(OP, shell));
        }
        let face = self.add_face_record(Face::new(shell))?;
        self.shells
            .get_mut(shell)
            .ok_or_else(|| not_found(OP, shell))?
            .faces
            .push(face);
        Ok(face)
    }

    /// Adds a loop to `face` from edges in traversal order. Every edge must exist and
    /// consecutive uses must share a vertex, closing back to the first. An edge's ends
    /// are read through its edge and vertex uses (`GetVertices`) when those name both
    /// ends, and from its creation-time [`Edge::vertices`] otherwise (a builder edge from
    /// [`Brep::add_edge`], before its uses are linked), so a loop built mid-operation
    /// never reads a stale pair.
    pub fn add_loop(&mut self, face: FaceId, edges: Vec<LoopEdge>) -> Result<LoopId> {
        const OP: &str = "topology::Brep::add_loop";
        if self.faces.get(face).is_none() {
            return Err(not_found(OP, face));
        }
        let mut ends = Vec::with_capacity(edges.len());
        for use_ in &edges {
            let e = self
                .edges
                .get(use_.edge)
                .ok_or_else(|| not_found(OP, use_.edge))?;
            // through the uses once they name both ends; a builder edge (no edge uses,
            // or edge uses whose vertex uses are not linked yet) has only its pair
            let [a, b] = match edge::start_end_vertices(self, use_.edge) {
                Ok((Some(a), Some(b))) => [a, b],
                _ => e.vertices,
            };
            ends.push(if use_.forward { (a, b) } else { (b, a) });
        }
        let closes = ends
            .iter()
            .zip(ends.iter().cycle().skip(1))
            .all(|(cur, next)| cur.1 == next.0);
        if ends.is_empty() || !closes {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "loop edges do not close",
            });
        }
        let id = self.loops.insert(Loop {
            face,
            edges,
            loopuse: None,
        })?;
        self.faces
            .get_mut(face)
            .ok_or_else(|| not_found(OP, face))?
            .loops
            .push(id);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec3;

    #[test]
    fn surface_owner_cleared_scopes_nest() {
        let mut b = Brep::new();
        let (f, g) = (FaceId::from_index(0), FaceId::from_index(1));
        assert!(!b.is_surface_owner_cleared(f));
        b.begin_surface_owner_cleared(f);
        b.begin_surface_owner_cleared(f);
        b.end_surface_owner_cleared(f);
        assert!(b.is_surface_owner_cleared(f));
        assert!(!b.is_surface_owner_cleared(g));
        b.end_surface_owner_cleared(f);
        assert!(!b.is_surface_owner_cleared(f));
        // an unmatched end is a no-op
        b.end_surface_owner_cleared(g);
        assert!(!b.is_surface_owner_cleared(g));
    }

    #[test]
    fn copy_standing_for_the_brep_keeps_geometry_owners() {
        use crate::curve::line::Line;
        use crate::geom::object::ObjectRef;
        use crate::surface::plane::Plane;
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let (v0, v1) = (
            b.add_vertex(Vec3::ZERO).unwrap(),
            b.add_vertex(Vec3::X).unwrap(),
        );
        let e = b.add_edge(v0, v1).unwrap();
        let f = b.add_face(s).unwrap();
        let edge_owner = ObjectRef {
            type_code: edge::TYPE,
            key: ObjectKey(u64::from(e.index())),
        };
        let face_owner = ObjectRef {
            type_code: face::p2::TYPE,
            key: ObjectKey(u64::from(f.index())),
        };
        let mut curve =
            Curve::from(Line::create_line_segment(3, Vec3::ZERO, Vec3::X, None).unwrap());
        let _ = curve.set_owner(Some(edge_owner), None);
        b.edge_mut(e).unwrap().curve = Some(curve);
        let mut surface = Surface::from(Plane::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            crate::math::Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ));
        surface.set_owner(Some(face_owner));
        b.face_mut(f).unwrap().surface = Some(surface);

        // the copy constructor resets the owners
        let plain = b.clone();
        assert_eq!(plain.edge(e).unwrap().curve.as_ref().unwrap().owner(), None);
        assert_eq!(
            plain.face(f).unwrap().surface.as_ref().unwrap().owner(),
            None
        );
        // the copy standing for the B-rep keeps them
        let copy = b.clone_standing_for_self().unwrap();
        assert_eq!(
            copy.edge(e).unwrap().curve.as_ref().unwrap().owner(),
            Some(edge_owner)
        );
        assert_eq!(
            copy.face(f).unwrap().surface.as_ref().unwrap().owner(),
            Some(face_owner)
        );
    }

    #[test]
    fn build_single_triangle_face() {
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let v: Vec<VertexId> = [Vec3::ZERO, Vec3::X, Vec3::Y]
            .into_iter()
            .map(|p| b.add_vertex(p).unwrap())
            .collect();
        let e0 = b.add_edge(v[0], v[1]).unwrap();
        let e1 = b.add_edge(v[1], v[2]).unwrap();
        let e2 = b.add_edge(v[0], v[2]).unwrap();
        let f = b.add_face(s).unwrap();
        let uses = vec![
            LoopEdge {
                edge: e0,
                forward: true,
            },
            LoopEdge {
                edge: e1,
                forward: true,
            },
            LoopEdge {
                edge: e2,
                forward: false,
            },
        ];
        let l = b.add_loop(f, uses).unwrap();

        assert_eq!(b.vertices().len(), 3);
        assert_eq!(b.edges().ids().collect::<Vec<_>>(), vec![e0, e1, e2]);
        assert_eq!(b.face(f).unwrap().loops, vec![l]);
        assert_eq!(b.shell(s).unwrap().faces, vec![f]);
        assert_eq!(b.loop_(l).unwrap().face, f);
        assert_eq!(b.vertex(v[1]).unwrap().point, Vec3::X);
        assert_eq!(b.edge(e2).unwrap().vertices, [v[0], v[2]]);
        let (first_id, first) = b.vertices().iter().next().unwrap();
        assert_eq!((first_id, first.point), (v[0], Vec3::ZERO));
    }

    #[test]
    fn errors_name_operation_and_entity() {
        let mut b = Brep::new();
        let missing = VertexId::from_index(3);
        let err = b.add_edge(missing, missing).unwrap_err();
        assert_eq!(err.operation(), "topology::Brep::add_edge");
        assert_eq!(err.entity(), Some(EntityRef::Vertex(missing)));
        assert!(b.face(FaceId::from_index(0)).is_err());

        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        let v0 = b.add_vertex(Vec3::ZERO).unwrap();
        let v1 = b.add_vertex(Vec3::X).unwrap();
        let e = b.add_edge(v0, v1).unwrap();
        let open = vec![LoopEdge {
            edge: e,
            forward: true,
        }];
        assert!(matches!(
            b.add_loop(f, open),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(b.add_loop(f, Vec::new()).is_err());
        assert!(b.loops().is_empty());
    }
}
