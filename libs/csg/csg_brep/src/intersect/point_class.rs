//! Point classification: the relation between a point and the object (vertex, edge,
//! face, region, curve, surface, volume or point) it lies on or near, with the
//! object's parameters at the point, the gap, and the surf/surf/surf point used by the
//! Boolean operation.
//!
//! Upstream: `source/SMLib/inc/SmPointClass.h`, `source/SMLib/src/SmPointClass.cpp`;
//! the type queries of `source/SMLib/inc/SmSpaceBend.h`, `SmSpaceUnbend.h` (bodies in
//! `SmSpaceBend.cpp`, `SmSpaceUnbend.cpp`); the destructor of
//! `source/SMLib/src/SmLineSegClass.cpp`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`PointClassification`]                           | `SmPointClassification`                          |
//! | [`PointClassType`]                                | `SmPointClassificationType`                      |
//! | [`ClassObject`], [`ClassObjectKind`]              | the `SmObject*` stored in `m_pObject` (`GetType`, `IsKindOf`) |
//! | [`ClassGeometry`]                                 | the casts of `m_pObject` to `SmCurve*`, `SmSurface*`, `SmVolume*`, `SmPoint3d*` |
//! | [`PointClassification::new`]                      | `SmPointClassification(SmZoneTol3d, const SmContext*)` |
//! | `Drop` (implicit)                                 | `~SmPointClassification`                         |
//! | [`PointClassification::reset`]                    | `ReSet`                                          |
//! | `Clone` / `Copy` (derived)                        | `operator=`                                      |
//! | [`PointClassification::map_objects`]             | complete state copy with translated Rust handles (data-model bridge) |
//! | [`PointClassification::copy_classification_only`] | `CopyClassificationOnly`                         |
//! | [`PointClassification::copy_sss_point`]           | `CopySSSPoint`                                   |
//! | [`PointClassification::brep`]                     | `GetBrep`                                        |
//! | [`PointClassification::face`]                     | `GetFace`                                        |
//! | [`PointClassification::vertex_object`]            | `GetVertexObject`                                |
//! | [`PointClassification::edge_object`]              | `GetEdgeObject`                                  |
//! | [`PointClassification::face_object`]              | `GetFaceObject`                                  |
//! | [`PointClassification::set_face`]                 | `SetFace`                                        |
//! | [`PointClassification::merge_into_object`], [`PointClassification::merge_into_brep`] | `MergeIntoObject` |
//! | [`PointClassification::point_class`]              | `GetPointClass`                                  |
//! | [`PointClassification::object`]                   | `GetObject`                                      |
//! | [`PointClassification::curve_object`]             | `GetCurveObject`                                 |
//! | [`PointClassification::gap_3d`]                   | `GetGap3d`                                       |
//! | [`PointClassification::deviation`]                | `GetDeviation`                                   |
//! | [`PointClassification::t_param`]                  | `GetTParam`                                      |
//! | [`PointClassification::uv_param`]                 | `GetUVParam`                                     |
//! | [`PointClassification::uvw_param`]                | `GetUVWParam`                                    |
//! | [`PointClassification::are_parameters_set`]       | `AreParametersSet`                               |
//! | [`PointClassification::pre_snap_param`]           | `GetPreSnapParam`                                |
//! | [`PointClassification::sss_point_flag`]           | `GetSSSPointFlag`                                |
//! | [`PointClassification::sss_point`]                | `GetSSSPoint`                                    |
//! | [`PointClassification::is_connected_to`]          | `IsConnectedTo(const SmTopology*)`               |
//! | [`PointClassification::is_connected_to_any`]      | `IsConnectedTo(const SmTArray<SmTopology*>&, ULONG&)` |
//! | [`PointClassification::set_class_object`]         | `SetClassObject`                                 |
//! | [`PointClassification::set_gap_3d`]               | `SetGap3d`                                       |
//! | [`PointClassification::set_deviation`]            | `SetDeviation`                                   |
//! | [`PointClassification::set_t_param`]              | `SetTParam`                                      |
//! | [`PointClassification::set_uv_param`]             | `SetUVParam`                                     |
//! | [`PointClassification::set_uvw_param`]            | `SetUVWParam`                                    |
//! | [`PointClassification::unset_parameters`]         | `UnSetParameters`                                |
//! | [`PointClassification::set_pre_snap_param`]       | `SetPreSnapParam`                                |
//! | [`PointClassification::set_sss_point`]            | `SetSSSPoint`                                    |
//! | [`PointClassification::tolerance`]                | `GetTolerance`                                   |
//! | [`PointClassification::obj_zone_tol_3d`]          | `GetObjZoneTol3d`, `SmTol::GetObjZoneTol3d(const SmPointClassification*)` |
//! | [`PointClassification::src_zone_tol_3d`]          | `GetSrcZoneTol3d`, `SmTol::GetSrcZoneTol3d(const SmPointClassification*)` |
//! | [`object_zone_tol_3d`]                            | `SmTol::GetZoneTol3d(const SmObject*)`           |
//! | [`PointClassification::xsect_tol_3d`]             | `SmTol::GetXSectTol3d(const SmPointClassification*)` |
//! | [`PointClassification::set_src_zone_tol_3d`]      | `SetSrcZoneTol3d`                                |
//! | [`PointClassification::combine`], [`PointClassification::combine_in_brep`], [`PointClassification::combine_setting_vertex_point`], [`Precedence`] | `Combine` |
//! | [`CombineBrep`]                                   | the arena behind the classified object           |
//! | [`PointClassification::compute_point_parameters`], [`PointClassification::compute_point_parameters_mut`]| `ComputePointParameters`                         |
//! | [`PointClassification::find_obj_point_3d`]        | `FindObjPoint3d`                                 |
//! | [`PointClassification::load_from_intersection`]   | `LoadFromIntersection`                           |
//! | [`MergeOutputs`]                                  | `MergeIntoObject`'s `pOptNewEdges`, `pOptNewVertices` |
//! | `merge_vertex_neighborhood` (private)             | the position/tolerance tail of `MergeIntoObject` |
//! | [`SPACE_BEND_TYPE_STRING`], [`space_bend_is_kind_of`] | `SmSpaceBend::GetType`, `GetTypeString`, `IsKindOf` |
//! | [`SPACE_UNBEND_TYPE_STRING`], [`space_unbend_is_kind_of`] | `SmSpaceUnbend::GetType`, `GetTypeString`, `IsKindOf` |
//! | `Drop` (implicit)                                 | `~SmSpaceBend`, `~SmSpaceUnbend`, `~SmLineSegClassification` |
//!
//! Deviations:
//! - **Mutable topology.** [`BrepMerge`] supplies an arena, resolved target,
//!   handle mapping and notification environment for [`PointClassification::merge_into_brep`],
//!   and [`CombineBrep`] the arena and notifications
//!   [`PointClassification::combine_in_brep`] writes a propagated vertex point to;
//!   [`PointClassification::combine_setting_vertex_point`] takes the caller's
//!   `SmVertex::SetPoint` instead (an adapter resolving the vertex on demand).
//!   The older opaque-handle entrypoints cannot resolve their object's arena and return
//!   an `Unported(topology.brep_p1)` / `Unported(topology.vertex)` deferral until the
//!   caller supplies that context. Region/face owner queries retain their own blockers.
//! - **Object handles.** `m_pObject` and `m_cpFace` are caller-chosen handles `O`
//!   ([`ClassObject`]): equal handles are the same object, and
//!   [`ClassObject::class_kind`] answers `GetType` / `IsKindOf`. The geometry behind a
//!   curve, surface, volume or point handle is reached through a [`ClassGeometry`];
//!   a topology handle's B-rep entity through its defaulted hooks
//!   [`ClassGeometry::topology_entity`] / [`ClassGeometry::topology_handle`]
//!   (CO-PORTED by `intersect.curve_class_p4`, which calls the topology members
//!   through them). `SmEdge::GetCurve` and `SmVertex::GetPoint` go through them (the
//!   vertex point first through [`ClassGeometry::classification_vertex_point`]); a
//!   handle the adapter cannot resolve to a B-rep stays an `Unported` deferral. `SM_PC_POINT` stores an `SmPoint3d*` cast to `SmObject*`; here it is a handle of
//!   kind [`ClassObjectKind::Point`]. Upstream calls virtual `SmObject` methods on that
//!   pointer in `ComputePointParameters` (`SmTol::GetZoneTol3d`), `IsConnectedTo` and
//!   `MergeIntoObject` (`GetType`), which is undefined behaviour; the port treats a
//!   point like any other non-topology object there (system zone tolerance, not
//!   connected, not mergeable).
//! - **Context.** `m_cpContext` is not stored: the kernel is passed explicitly, so
//!   `SetContext` has no counterpart and the context argument of the constructor is
//!   dropped.
//! - **Out-params.** `FindObjPoint3d` returns the point; `IsConnectedTo`'s found index is
//!   the `Option<usize>` result (`None` = `SM_UNDEF_ULONG`); `MergeIntoObject` returns
//!   its new edges and vertices ([`MergeOutputs`]); `Combine` returns which operand
//!   takes precedence ([`Precedence`]) instead of a reference to it.
//! - **Unused parameter.** `ComputePointParameters`' `dOptClassifyCrvParam` is
//!   documented `NotUsed` and dropped.
//! - **Release build.** The `SM_DEBUG_CODE` checks of `SetClassObject`, `SetTParam`
//!   (including its `bSnapToIvl` snapping) and `SetUVParam`, the debug dump and
//!   drawing of `MergeIntoObject` and its `SM_AI_SSSPOINT` attribute are compiled out
//!   upstream and not ported; `set_t_param` keeps the `snap_to_ivl` argument, unused,
//!   as upstream. `SM_ASSERT_TOL` only reports.
//! - **Geometry tolerances.** `SmTol::GetZoneTol3d(const SmObject*)` gives a curve or
//!   surface the tolerance of its edge or face (`GetEdge` / `GetFace`), and every other
//!   non-topology object the system zone tolerance. A curve's edge is found with
//!   [`Curve::edge`] (intermediate owners through [`ClassGeometry::owner_of`]); a
//!   surface's face is [`Surface::face`](crate::surface::surface::Surface::face),
//!   whose tolerance is the one its B-rep cached on the surface
//!   ([`Surface::owner_zone_tol_3d`](crate::surface::surface::Surface::owner_zone_tol_3d));
//!   an edge's or face's tolerance is that of the edge or face. Topology
//!   kinds without a `GetTolerance` override (region, faceuse, loop, loopuse,
//!   vertexuse) get `SmTopology::GetTolerance`, `SM_ZONE_TOL_3D`, and a faceuse
//!   `SmTopology::IsConnectedTo`, `FALSE`.
//! - **Fast-drop trim test.** `GlobalPointSolve` of a face's surface (in
//!   `ComputePointParameters` and `GetGap3d`) tests its fast-drop solutions against the
//!   face (`smsurf_GetSurfaceCache`, then `PointClassify(uv, tol, TRUE, FALSE)`): the
//!   read-only forms reuse the trimmed cache the face's surface keeps and classify
//!   without changing the B-rep, deferring ([`SolidError::Unported`]) when a new cache
//!   would have to be made; [`PointClassification::compute_point_parameters_mut`] makes
//!   it on the B-rep the adapter resolves the face to mutably
//!   ([`ClassGeometryMut::topology_entity_mut`]). The cache is looked up when the first
//!   solution is tested; upstream looks it up once the fast drop found solutions, also
//!   when the domain filter then removed them all (that lookup's side effects are not
//!   made here then).
//! - **`GetGap3d` failure.** With a surface and no single drop point, upstream returns
//!   `SM_ERR` *as the gap value*; so does [`PointClassification::gap_3d`].
//! - **Topology members.** `GetCurveObject`, `IsConnectedTo`, `ComputePointParameters`,
//!   `FindObjPoint3d` and `SmTol::GetZoneTol3d` of a vertex, edge, edge use, face, loop,
//!   vertex use or loop use call the ported topology members (`SmEdge::GetCurve` /
//!   `GetInterval` / `GetTolerance`, `SmEdgeuse::GetEdge`, `SmVertex::GetPoint`, `SmFace::
//!   GetSurface` / `GetUVDomain`, the `IsConnectedTo` overrides) on the B-rep entity
//!   [`ClassGeometry::topology_entity`] resolves; a handle whose adapter lacks that hook keeps
//!   the earlier [`SolidError::Unported`] deferral, and an `IsConnectedTo` target in another
//!   B-rep is connected to nothing. `SmFace::IsConnectedTo` (`topology.face_p4`) and the
//!   B-rep of `GetBrep` (a [`BrepId`] the handle hooks cannot name) are still unported.
//! - **Opt-in (WR-537).** With `Kernel::fixes.seam_point_solutions_are_one`,
//!   `ComputePointParameters` keeps the nearest of a face or surface solve's solutions
//!   when they all evaluate to one point within the source zone tolerance (a point on a
//!   closed face's seam; the fork's source fork@577f7be).
//! - **Not in this unit.** `GetLoop`, `GetSurfaceObject`, `GetVolumeObject`,
//!   `operator==`, `AssertValid`, `Dump`, `Draw` and the RTTI of
//!   `SmPointClassification` are unreachable from the in-scope entry points
//!   (`harness/callgraph`) and not ported.

use crate::curve::curve::p2::Curve;
use crate::error::{ignored, EntityRef, Result, SolidError, SM_ERR};
use crate::geom::assert_array::labels::{FACE_TYPE, SPACE_BEND_TYPE, SPACE_UNBEND_TYPE};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{self, ObjectRef};
use crate::geom::solution_array::{ObjectKind, SolutionEnd, SolutionObject};
use crate::geom::tol::{
    xsect_tol3d, xsect_tol_3d_from_zones, zone_tol3d, zone_tol_3d, SmallTopoType, UNINIT_TOL,
    ZONE_TOL_3D,
};
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::kernel::Kernel;
use crate::math::{max2, Point2, Point3, Vec3, BIG_DOUBLE};
use crate::nurbs::knots::Side;
use crate::surface::surface::p4::{global_point_solve_starts, NoObject};
use crate::surface::surface::{Surface, TrimCacheSlot};
use crate::surface::volume::Volume;
use crate::topology::{
    edge, edgeuse, face, loopuse, vertex, vertexuse, Brep, BrepId, EdgeId, FaceId,
};

/// Mutable topology and handle mapping for [`PointClassification::merge_into_brep`].
/// The target belongs to `brep`; `handle` maps its arena ids back into the caller's
/// classification handle space, including newly allocated vertices and edges.
/// This avoids a global resolver or a second topology implementation.
pub struct BrepMerge<'a, 'env, O> {
    /// Arena graph containing the classified object.
    pub brep: &'a mut Brep,
    /// Resolved target in that graph (region, face, edge or vertex).
    pub target: EntityRef,
    /// Classification handle corresponding to a topology id in this B-rep.
    pub handle: &'a dyn Fn(EntityRef) -> O,
    /// Notification storage/identities shared with the caller's other mutations.
    pub notifications: &'a mut edge::NotifyEnv<'env>,
    /// Identity of each edge's curve from its id and actual native type code, including a new split edge.
    pub curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
}

/// Actual merge context with checked identity access to newly constructed curves.
pub struct BrepMergeWithIdentity<'a, 'env, O> {
    /// Actual arena containing the classified object.
    pub brep: &'a mut Brep,
    /// Target in that arena.
    pub target: EntityRef,
    /// Caller handle for each arena entity.
    pub handle: &'a dyn Fn(EntityRef) -> O,
    /// Retained records, attributes and notification context.
    pub notifications: &'a mut edge::NotifyEnv<'env>,
    /// Legacy or actual-curve identity authority at construction.
    pub curve_identity: crate::topology::brep::p9::CurveIdentity<'a>,
}
impl<'a, 'env, O> From<BrepMerge<'a, 'env, O>> for BrepMergeWithIdentity<'a, 'env, O> {
    fn from(context: BrepMerge<'a, 'env, O>) -> Self {
        Self {
            brep: context.brep,
            target: context.target,
            handle: context.handle,
            notifications: context.notifications,
            curve_identity: crate::topology::brep::p9::CurveIdentity::Legacy(context.curve_ref),
        }
    }
}

/// The arena [`PointClassification::combine_in_brep`] writes a propagated surf/surf/surf
/// point to.
pub struct CombineBrep<'a, 'env, O> {
    /// Arena graph containing the classified object.
    pub brep: &'a mut Brep,
    /// Resolved target in that graph (a vertex for an `SM_PC_VERTEX` classification).
    pub target: EntityRef,
    /// Classification handle corresponding to a topology id in this B-rep.
    pub handle: &'a dyn Fn(EntityRef) -> O,
    /// Notification storage/identities shared with the caller's other mutations.
    pub notifications: &'a mut edge::NotifyEnv<'env>,
}

const OP_GAP: &str = "intersect::point_class::PointClassification::gap_3d";
const OP_COMPUTE: &str = "intersect::point_class::PointClassification::compute_point_parameters";
const OP_FIND: &str = "intersect::point_class::PointClassification::find_obj_point_3d";
const OP_MERGE: &str = "intersect::point_class::PointClassification::merge_into_object";
const OP_COMBINE: &str = "intersect::point_class::PointClassification::combine";
const OP_LOAD: &str = "intersect::point_class::PointClassification::load_from_intersection";
const OP_GEOMETRY: &str = "intersect::point_class::ClassGeometry";

/// The kind of object a point is classified to (`SmPointClassificationType`). The
/// discriminants are the upstream values; `SmCurveClassification` depends on their
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(i32)]
pub enum PointClassType {
    /// `SM_PC_UNKNOWN`: not classified yet.
    #[default]
    Unknown = 100,
    /// `SM_PC_REGION`: in a topological region (`SmRegion`).
    Region = 101,
    /// `SM_PC_FACE`: on or near a face (`SmFace`).
    Face = 102,
    /// `SM_PC_EDGE`: on or near an edge (`SmEdge`).
    Edge = 103,
    /// `SM_PC_VERTEX`: on or near a vertex (`SmVertex`).
    Vertex = 104,
    /// `SM_PC_POINT`: on or near a geometric point (`SmPoint3d`).
    Point = 105,
    /// `SM_PC_CURVE`: on or near a curve (`SmCurve`).
    Curve = 106,
    /// `SM_PC_SURFACE`: on or near a surface (`SmSurface`).
    Surface = 107,
    /// `SM_PC_VOLUME`: on, in or near a volume (`SmVolume`).
    Volume = 108,
    /// `SM_PC_EDGEUSE`: on or near an edgeuse (`SmEdgeuse`).
    Edgeuse = 109,
    /// `SM_PC_NOTHING`: known not to map to any object.
    Nothing = 110,
}

impl PointClassType {
    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The type with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            100 => Self::Unknown,
            101 => Self::Region,
            102 => Self::Face,
            103 => Self::Edge,
            104 => Self::Vertex,
            105 => Self::Point,
            106 => Self::Curve,
            107 => Self::Surface,
            108 => Self::Volume,
            109 => Self::Edgeuse,
            110 => Self::Nothing,
            _ => return None,
        })
    }
}

/// What a classification object is: the `GetType` / `IsKindOf` families this unit
/// tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClassObjectKind {
    /// An `SmCurve` (any curve type).
    Curve,
    /// An `SmSurface` (any surface type).
    Surface,
    /// An `SmVolume`.
    Volume,
    /// An `SmPoint3d` (stored by `SM_PC_POINT` classifications).
    Point,
    /// An `SmVertex`.
    Vertex,
    /// An `SmEdge`.
    Edge,
    /// An `SmFace`.
    Face,
    /// An `SmLoop`.
    Loop,
    /// An `SmRegion`.
    Region,
    /// An `SmVertexuse`.
    Vertexuse,
    /// An `SmEdgeuse`.
    Edgeuse,
    /// An `SmFaceuse`.
    Faceuse,
    /// An `SmLoopuse`.
    Loopuse,
    /// Any other object.
    Other,
}

/// A handle to a classification object (upstream `SmObject*`). Two handles are the
/// same object when they compare equal.
pub trait ClassObject: Copy + PartialEq {
    /// The object's kind.
    fn class_kind(&self) -> ClassObjectKind;

    /// The handle naming the `index`-th temporary object of `kind` (a boundary
    /// curve or plane) that a classification builds and records as an interval end
    /// object (upstream stores the temporary's pointer). Handles for distinct
    /// indices must differ from each other and from the caller's objects. `None`
    /// (the default): the handle type cannot name temporaries, and an insertion
    /// that needs one returns an invalid-input error.
    fn temporary(index: usize, kind: ClassObjectKind) -> Option<Self> {
        let _ = (index, kind);
        None
    }
}

/// The geometry behind classification handles: what upstream reaches by casting
/// `m_pObject`. Each method returns `None` for a handle of another kind.
pub trait ClassGeometry<O> {
    /// The curve of a [`ClassObjectKind::Curve`] handle.
    fn curve(&self, o: O) -> Option<&Curve>;
    /// The surface of a [`ClassObjectKind::Surface`] handle.
    fn surface(&self, o: O) -> Option<&Surface>;
    /// The volume of a [`ClassObjectKind::Volume`] handle.
    fn volume(&self, o: O) -> Option<&Volume>;
    /// The point of a [`ClassObjectKind::Point`] handle.
    fn point(&self, o: O) -> Option<Point3>;
    /// The point of a vertex handle resolved through its owning B-rep. `None`
    /// reports unresolved caller data; part 5 returns an invalid-input error.
    fn classification_vertex_point(&self, vertex: O) -> Option<Point3> {
        let _ = vertex;
        None
    }
    /// Stored topology zone tolerance, for a resolved vertex/edge/face/edgeuse.
    /// `None` preserves the existing opaque-handle deferral.
    fn topology_zone_tol_3d(&self, object: O) -> Option<f64> {
        let _ = object;
        None
    }
    /// Vertex-to-face gap length for the face of `surface`, preferring the explicit
    /// classification `face`. `Some(0.0)` denotes a known absent gap; `None` means
    /// that the vertexuse/face relationship could not be resolved and part 5
    /// returns an invalid-input error when a face requires this lookup.
    fn vertex_face_gap(&self, vertex: O, surface: &Surface, face: Option<O>) -> Option<f64> {
        let _ = (vertex, surface, face);
        None
    }
    /// The B-rep and topology entity behind a topology handle (a loop, face, face use,
    /// edge, edge use, vertex or vertex use), through which part 4 of the curve
    /// classification calls the topology members (`SmLoop::GetFace`,
    /// `SmFace::GetUpwardFaceuse`, `SmEdge::GetEdgeuses`, ...). `None` (the default) is a
    /// handle the adapter cannot resolve: a gap in the caller's adapter, reported as an
    /// invalid-input error.
    fn topology_entity(&self, object: O) -> Option<(&Brep, EntityRef)> {
        let _ = object;
        None
    }
    /// The handle of entity `entity` of the B-rep that [`ClassGeometry::topology_entity`]
    /// resolved to; `None` (the default) when the adapter has none (an invalid-input
    /// error for the caller).
    fn topology_handle(&self, entity: EntityRef) -> Option<O> {
        let _ = entity;
        None
    }
    /// The handle of entity `entity` of `brep`, a B-rep that
    /// [`ClassGeometry::topology_entity`] returned. An adapter over several B-reps
    /// implements this to tell them apart; the default is
    /// [`ClassGeometry::topology_handle`].
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<O> {
        let _ = brep;
        self.topology_handle(entity)
    }
    /// The Boolean mate of topology handle `entity` through the topology intersector
    /// behind handle `ti` (`ti->GetOtherMate(entity) ? .. : ti->GetBrepMate(entity)`):
    /// `Some(None)` for no mate. `None` (the default) is an adapter that cannot reach
    /// the intersector; callers keep that lookup as an `Unported` deferral.
    fn topology_intersector_mate(&self, ti: O, entity: O) -> Option<Option<O>> {
        let _ = (ti, entity);
        None
    }
    /// The B-rep id a caller's collection gives `brep`, a B-rep that
    /// [`ClassGeometry::topology_entity`] returned (upstream's `SmBrep*` of `GetBrep`).
    /// `None` (the default) is an adapter without such ids; `GetBrep` of a topology
    /// object then stays an `Unported` deferral.
    fn brep_id(&self, brep: &Brep) -> Option<BrepId> {
        let _ = brep;
        None
    }
    /// The B-rep entity behind the owner `owner` of a curve or surface (the edge of
    /// `SmCurve::GetEdge`, the face of `SmSurface::GetFace`), when the adapter can
    /// resolve object references; `None` (the default) when it cannot.
    fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
        let _ = owner;
        None
    }
    /// The owner of an intermediate owner on a curve's owner chain (`GetOwner` of a
    /// generating curve or an edgeuse), for `SmCurve::GetEdge`
    /// ([`Curve::edge`]); `None` (the default) ends the chain.
    fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        let _ = owner;
        None
    }
}

/// Which operand of [`PointClassification::combine`] takes precedence (upstream
/// returns a reference to it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precedence {
    /// `*this`: the classification `combine` was called on.
    This,
    /// `crPointClass`: the argument.
    Other,
}

/// The topology created by [`PointClassification::merge_into_object`]
/// (`pOptNewEdges`, `pOptNewVertices`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutputs<O> {
    /// The two edges from splitting an edge (one reused, one new).
    pub new_edges: Vec<O>,
    /// The new vertex, if any.
    pub new_vertices: Vec<O>,
}

/// A point classification (`SmPointClassification`): the object a point lies on or
/// near, its parameters there, the gap, the zone tolerance of what was classified and
/// the optional surf/surf/surf point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointClassification<O> {
    /// `m_sSrcZoneTol3d`.
    src_zone_tol_3d: f64,
    /// `m_ePointClass`.
    point_class: PointClassType,
    /// `m_pObject`.
    object: Option<O>,
    /// `m_cpFace`.
    face: Option<O>,
    /// `m_dGap3d`.
    gap_3d: f64,
    /// `m_dTParam`.
    t_param: f64,
    /// `m_vUVParam`.
    uv_param: Point2,
    /// `m_vUVWParam`.
    uvw_param: Point3,
    /// `m_bParamSet`.
    param_set: bool,
    /// `m_dPreSnapParam`.
    pre_snap_param: f64,
    /// `m_bSSSPointFlag`.
    sss_point_flag: bool,
    /// `m_vSSSPoint`.
    sss_point: Point3,
}

impl<O: ClassObject> PointClassification<O> {
    /// The same classification with every object handle (`m_pObject`, `m_cpFace`)
    /// replaced by `f` of it: a change of handle type, no new state.
    ///
    /// # Errors
    ///
    /// Those of `f`.
    pub fn map_objects<P: ClassObject>(
        &self,
        f: &mut dyn FnMut(O) -> Result<P>,
    ) -> Result<PointClassification<P>> {
        Ok(PointClassification {
            src_zone_tol_3d: self.src_zone_tol_3d,
            point_class: self.point_class,
            object: self.object.map(&mut *f).transpose()?,
            face: self.face.map(&mut *f).transpose()?,
            gap_3d: self.gap_3d,
            t_param: self.t_param,
            uv_param: self.uv_param,
            uvw_param: self.uvw_param,
            param_set: self.param_set,
            pre_snap_param: self.pre_snap_param,
            sss_point_flag: self.sss_point_flag,
            sss_point: self.sss_point,
        })
    }

    /// A cleared classification of something with zone tolerance `src_zone_tol_3d`
    /// (`SmPointClassification(SmZoneTol3d, const SmContext*)`). The argument is
    /// already an `SmZoneTol3d` and is stored as is; upstream's default,
    /// `SmZoneTol3d(SM_USE_DEFAULT)`, is `zone_tol3d(k, USE_DEFAULT)` (the system zone
    /// tolerance).
    #[must_use]
    pub fn new(src_zone_tol_3d: f64) -> Self {
        Self {
            src_zone_tol_3d,
            point_class: PointClassType::Unknown,
            object: None,
            face: None,
            gap_3d: 0.0,
            t_param: 0.0,
            uv_param: Point2::new(0.0, 0.0),
            uvw_param: Point3::new(0.0, 0.0, 0.0),
            param_set: false,
            pre_snap_param: BIG_DOUBLE,
            sss_point_flag: false,
            sss_point: Point3::new(0.0, 0.0, 0.0),
        }
    }

    /// Resets every member to its initial value (`ReSet`); the source zone tolerance
    /// becomes `opt_zone_tol_3d`, or `SM_UNINIT_TOL` for `None`.
    pub fn reset(&mut self, k: &Kernel, opt_zone_tol_3d: Option<f64>) {
        self.point_class = PointClassType::Unknown;
        self.face = None;
        self.object = None;
        self.src_zone_tol_3d = match opt_zone_tol_3d {
            Some(t) => t,
            None => zone_tol3d(k, UNINIT_TOL),
        };

        self.gap_3d = 0.0;
        self.t_param = 0.0;
        self.uv_param = Point2::new(0.0, 0.0);
        self.uvw_param = Point3::new(0.0, 0.0, 0.0);
        self.param_set = false;

        self.pre_snap_param = BIG_DOUBLE;
        self.sss_point_flag = false;
        self.sss_point = Point3::new(0.0, 0.0, 0.0);
    }

    /// Copies everything from `other` except the surf/surf/surf point
    /// (`CopyClassificationOnly`).
    pub fn copy_classification_only(&mut self, other: &Self) {
        self.point_class = other.point_class;
        self.face = other.face;

        self.object = other.object;
        self.src_zone_tol_3d = other.src_zone_tol_3d;

        self.gap_3d = other.gap_3d;
        self.t_param = other.t_param;
        self.uv_param = other.uv_param;
        self.uvw_param = other.uvw_param;
        self.param_set = other.param_set;

        self.pre_snap_param = other.pre_snap_param;
    }

    /// Copies only the surf/surf/surf point and its flag from `other`
    /// (`CopySSSPoint`).
    pub fn copy_sss_point(&mut self, other: &Self) {
        self.sss_point_flag = other.sss_point_flag;
        self.sss_point = other.sss_point;
    }

    /// The B-rep of the classification object, unless it is `skip` (`GetBrep`):
    /// `None` for geometry that no edge or face owns and for points, volumes and
    /// unclassified points.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for topology objects and owned geometry (the
    /// topology units); [`SolidError::InvalidInput`] when `geometry` cannot resolve a
    /// curve or surface handle.
    pub fn brep(
        &self,
        geometry: &impl ClassGeometry<O>,
        skip: Option<BrepId>,
    ) -> Result<Option<BrepId>> {
        let brep: Option<BrepId> = match self.point_class {
            PointClassType::Vertex => {
                let o = self.object.ok_or_else(|| null_object(OP_GEOMETRY))?;
                match topology_of(geometry, o, "SmVertex::GetBrep", "topology.vertex")? {
                    (b, EntityRef::Vertex(v)) => brep_id_of(geometry, vertex::brep(b, v)?)?,
                    _ => return Err(wrong_entity(OP_GEOMETRY)),
                }
            }
            PointClassType::Edge => {
                let o = self.object.ok_or_else(|| null_object(OP_GEOMETRY))?;
                match topology_of(geometry, o, "SmEdge::GetBrep", "topology.edge")? {
                    (b, EntityRef::Edge(e)) => brep_id_of(geometry, edgeuse::p1::edge_brep(b, e)?)?,
                    _ => return Err(wrong_entity(OP_GEOMETRY)),
                }
            }
            PointClassType::Face => {
                let o = self.object.ok_or_else(|| null_object(OP_GEOMETRY))?;
                match topology_of(geometry, o, "SmFace::GetBrep", "topology.face_p4")? {
                    (b, EntityRef::Face(f)) => brep_id_of(geometry, face::p4::brep(b, f)?)?,
                    _ => return Err(wrong_entity(OP_GEOMETRY)),
                }
            }
            PointClassType::Edgeuse => {
                let o = self.object.ok_or_else(|| null_object(OP_GEOMETRY))?;
                match topology_of(geometry, o, "SmEdgeuse::GetBrep", "topology.edgeuse_p1")? {
                    (b, EntityRef::Edgeuse(eu)) => brep_id_of(geometry, edgeuse::p1::brep(b, eu)?)?,
                    _ => return Err(wrong_entity(OP_GEOMETRY)),
                }
            }
            PointClassType::Curve => {
                let curve = self.curve_of(geometry, OP_GEOMETRY)?;
                // SmCurve::GetEdge, then SmEdge::GetBrep
                match curve.edge(|r| geometry.owner_of(r))? {
                    None => None,
                    Some(edge_ref) => match geometry.owner_entity(edge_ref) {
                        Some((b, EntityRef::Edge(e))) => {
                            brep_id_of(geometry, edgeuse::p1::edge_brep(b, e)?)?
                        }
                        Some(_) => return Err(wrong_entity(OP_GEOMETRY)),
                        // UNPORTED(topology.edge): SmEdge::GetBrep (of SmCurve::GetEdge; an
                        // adapter that cannot resolve the owning edge)
                        // Proof (unreachable): fires only for an adapter whose owner_entity
                        // fails on this owner; the library adapters resolve it; 0 hits in
                        // the green difftest sweep.
                        None => {
                            return Err(SolidError::unported("SmEdge::GetBrep", "topology.edge"))
                        }
                    },
                }
            }
            PointClassType::Surface => {
                let surface = self.surface_of(geometry, OP_GEOMETRY)?;
                // SmSurface::GetFace, then SmFace::GetBrep
                match surface.face()? {
                    None => None,
                    Some(face_ref) => match geometry.owner_entity(face_ref) {
                        Some((b, EntityRef::Face(f))) => {
                            brep_id_of(geometry, face::p4::brep(b, f)?)?
                        }
                        Some(_) => return Err(wrong_entity(OP_GEOMETRY)),
                        // UNPORTED(topology.face_p4): SmFace::GetBrep (of SmSurface::GetFace;
                        // an adapter that cannot resolve the owning face)
                        // Proof (unreachable): fires only for an adapter whose owner_entity
                        // fails on this owner; the library adapters resolve it; 0 hits in
                        // the green difftest sweep.
                        None => {
                            return Err(SolidError::unported("SmFace::GetBrep", "topology.face_p4"))
                        }
                    },
                }
            }
            PointClassType::Volume
            | PointClassType::Point
            | PointClassType::Nothing
            | PointClassType::Unknown
            | PointClassType::Region => None,
        };

        // skip specified breps
        Ok(match (skip, brep) {
            (Some(s), Some(b)) if s == b => None,
            _ => brep,
        })
    }

    /// The face the point was classified against, if any (`GetFace`).
    #[must_use]
    pub const fn face(&self) -> Option<O> {
        self.face
    }

    /// The object when the point is classified to a vertex (`GetVertexObject`).
    #[must_use]
    pub fn vertex_object(&self) -> Option<O> {
        if self.point_class == PointClassType::Vertex {
            self.object
        } else {
            None
        }
    }

    /// The object when the point is classified to an edge (`GetEdgeObject`).
    #[must_use]
    pub fn edge_object(&self) -> Option<O> {
        if self.point_class == PointClassType::Edge {
            self.object
        } else {
            None
        }
    }

    /// The object when the point is classified to a face (`GetFaceObject`).
    #[must_use]
    pub fn face_object(&self) -> Option<O> {
        if self.point_class == PointClassType::Face {
            self.object
        } else {
            None
        }
    }

    /// Sets the face the point was classified against (`SetFace`).
    pub fn set_face(&mut self, face: Option<O>) {
        self.face = face;
    }

    /// The classification type (`GetPointClass`).
    #[must_use]
    pub const fn point_class(&self) -> PointClassType {
        self.point_class
    }

    /// The classification object (`GetObject`).
    #[must_use]
    pub const fn object(&self) -> Option<O> {
        self.object
    }

    /// The curve of the classification object (`GetCurveObject`): the curve itself,
    /// or the curve of an edge or edgeuse; `None` otherwise.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for edgeuses (`SmEdgeuse::GetEdge`) and edge handles
    /// `geometry` cannot resolve to its B-rep; [`SolidError::InvalidInput`] when
    /// `geometry` cannot resolve a curve handle.
    pub fn curve_object<'g>(
        &self,
        geometry: &'g impl ClassGeometry<O>,
    ) -> Result<Option<&'g Curve>> {
        match self.point_class {
            PointClassType::Edge | PointClassType::Edgeuse => match self.object {
                Some(o) => {
                    let (b, e) = topology_edge(geometry, o, OP_GEOMETRY)?;
                    edge::curve(b, e)
                }
                None => Ok(None),
            },
            PointClassType::Curve => match self.object {
                Some(o) => geometry
                    .curve(o)
                    .map(Some)
                    .ok_or_else(|| unresolved(OP_GEOMETRY)),
                None => Ok(None),
            },
            _ => Ok(None),
        }
    }

    /// The gap between the classified point and the object (`GetGap3d`): the stored
    /// gap for `opt_surface = None`, else the length of the gap component tangent to
    /// `opt_surface` at the object point's drop point on it. Upstream returns
    /// `SM_ERR` (as a value) when the object point has no single drop point on the
    /// surface, even at a hundred times the object's zone tolerance; so does this.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for a surface: the surface point solve (unit
    /// `surface.surface_p4`), and topology objects.
    pub fn gap_3d(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        opt_surface: Option<&Surface>,
    ) -> Result<f64> {
        // low work - full 3d gaps - no gap vector projection requested
        let Some(surface) = opt_surface else {
            return Ok(self.gap_3d);
        };

        // else - in surface gaps
        // Get Object Point
        let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
        let obj_pt_3d = ignored(self.find_obj_point_3d(k, geometry))?.unwrap_or(Vec3::UNDEF);
        let obj_zone_tol_3d = self.obj_zone_tol_3d(k, geometry)?;

        // find SurfaceDropPt for ObjPt
        let owner = owner_face(geometry, surface);
        let mut solutions = ignored(surface_global_point_solve(
            k,
            surface,
            owner,
            &uv_domain,
            obj_pt_3d,
            obj_zone_tol_3d,
        ))?
        .unwrap_or_default();

        // when a distinct solution was not found
        if solutions.len() != 1 {
            // find point/face->surface intersection point to 100 * face->Tolerance
            solutions = ignored(surface_global_point_solve(
                k,
                surface,
                owner,
                &uv_domain,
                obj_pt_3d,
                zone_tol3d(k, obj_zone_tol_3d * 100.0),
            ))?
            .unwrap_or_default();

            // still no solutions - return an error
            if solutions.len() != 1 {
                return Ok(f64::from(SM_ERR));
            }
        }

        let first = solutions.first().ok_or_else(|| unresolved(OP_GAP))?;
        let uv = Point2::new(first.param(0)?, first.param(1)?);
        let surf_pt_3d = ignored(surface.evaluate_point(k, uv))?.unwrap_or(Vec3::UNDEF);
        let surf_norm = ignored(surface_evaluate_normal(k, surface, uv))?.unwrap_or(Vec3::UNDEF);

        let gap_3d = obj_pt_3d - surf_pt_3d;
        let in_surf_gap_3d = gap_3d - gap_3d.dot(surf_norm) * surf_norm;

        Ok(in_surf_gap_3d.length())
    }

    /// The stored gap (`GetDeviation`, the obsolete name of `GetGap3d()`).
    #[must_use]
    pub const fn deviation(&self) -> f64 {
        self.gap_3d
    }

    /// The curve or edge parameter (`GetTParam`).
    #[must_use]
    pub const fn t_param(&self) -> f64 {
        self.t_param
    }

    /// The surface or face parameters (`GetUVParam`).
    #[must_use]
    pub const fn uv_param(&self) -> Point2 {
        self.uv_param
    }

    /// The volume parameters (`GetUVWParam`).
    #[must_use]
    pub const fn uvw_param(&self) -> Point3 {
        self.uvw_param
    }

    /// Whether the parameters for the object kind are set (`AreParametersSet`).
    #[must_use]
    pub const fn are_parameters_set(&self) -> bool {
        self.param_set
    }

    /// The parameter on the curve being classified (`GetPreSnapParam`).
    #[must_use]
    pub const fn pre_snap_param(&self) -> f64 {
        self.pre_snap_param
    }

    /// Whether the surf/surf/surf point is set (`GetSSSPointFlag`).
    #[must_use]
    pub const fn sss_point_flag(&self) -> bool {
        self.sss_point_flag
    }

    /// The surf/surf/surf point (`GetSSSPoint`).
    #[must_use]
    pub const fn sss_point(&self) -> Point3 {
        self.sss_point
    }

    /// Whether the classification object connects to `target` through the topology
    /// graph (`IsConnectedTo(const SmTopology*)`): false for objects that are not
    /// topology and for no object.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for topology objects (the topology units).
    pub fn is_connected_to(&self, geometry: &impl ClassGeometry<O>, target: O) -> Result<bool> {
        let Some(o) = self.object else {
            return Ok(false);
        };
        let (upstream, unit) = match o.class_kind() {
            ClassObjectKind::Vertex => ("SmVertex::IsConnectedTo", "topology.vertex"),
            ClassObjectKind::Edge => ("SmEdge::IsConnectedTo", "topology.edge"),
            ClassObjectKind::Face => ("SmFace::IsConnectedTo", "topology.face_p4"),
            ClassObjectKind::Loop => ("SmLoop::IsConnectedTo", "topology.loopuse"),
            ClassObjectKind::Vertexuse => ("SmVertexuse::IsConnectedTo", "topology.vertexuse"),
            ClassObjectKind::Edgeuse => ("SmEdgeuse::IsConnectedTo", "topology.edgeuse_p1"),
            ClassObjectKind::Loopuse => ("SmLoopuse::IsConnectedTo", "topology.loopuse"),
            // SmFaceuse does not override it: SmTopology::IsConnectedTo reports and
            // returns FALSE (unit topology.topology); the other kinds are not tested
            ClassObjectKind::Faceuse
            | ClassObjectKind::Curve
            | ClassObjectKind::Surface
            | ClassObjectKind::Volume
            | ClassObjectKind::Point
            | ClassObjectKind::Region
            | ClassObjectKind::Other => return Ok(false),
        };
        let (b, entity) = topology_of(geometry, o, upstream, unit)?;
        let (tb, tgt) = topology_of(geometry, target, upstream, unit)?;

        // a target in another B-rep is none of this object's neighbours
        if !std::ptr::eq(b, tb) {
            return Ok(false);
        }
        let tgt = Some(tgt);
        match entity {
            EntityRef::Vertex(v) => vertex::is_connected_to(b, v, tgt),
            EntityRef::Edge(e) => edge::is_connected_to(b, e, tgt),
            EntityRef::Face(f) => face::p4::is_connected_to(b, f, tgt),
            EntityRef::Loop(l) => loopuse::loop_is_connected_to(b, l, tgt),
            EntityRef::Vertexuse(vu) => vertexuse::is_connected_to(b, vu, tgt),
            EntityRef::Edgeuse(eu) => edgeuse::p1::is_connected_to(b, eu, tgt),
            EntityRef::Loopuse(lu) => loopuse::is_connected_to(b, lu, tgt),
            _ => Err(wrong_entity(OP_GEOMETRY)),
        }
    }

    /// The index of the first of `targets` the classification object connects to
    /// (`IsConnectedTo(const SmTArray<SmTopology*>&, ULONG&)`), `None` if none.
    ///
    /// # Errors
    ///
    /// Those of [`PointClassification::is_connected_to`].
    pub fn is_connected_to_any(
        &self,
        geometry: &impl ClassGeometry<O>,
        targets: &[O],
    ) -> Result<Option<usize>> {
        for (ii, &t) in targets.iter().enumerate() {
            if self.is_connected_to(geometry, t)? {
                return Ok(Some(ii));
            }
        }
        Ok(None)
    }

    /// Sets the classification type and object (`SetClassObject`): no object makes
    /// the type `Unknown`; the parameters count as set for points and vertices only.
    pub fn set_class_object(&mut self, point_class: PointClassType, object: Option<O>) {
        self.point_class = point_class;
        self.object = object;

        if object.is_none() {
            self.point_class = PointClassType::Unknown; // Should be safer this way.
        }

        self.param_set = matches!(point_class, PointClassType::Point | PointClassType::Vertex);
    }

    /// Sets the gap (`SetGap3d`).
    pub fn set_gap_3d(&mut self, gap_3d: f64) {
        self.gap_3d = gap_3d;
    }

    /// Sets the gap (`SetDeviation`, the obsolete name of `SetGap3d`).
    pub fn set_deviation(&mut self, gap_3d: f64) {
        self.set_gap_3d(gap_3d);
    }

    /// Sets the curve or edge parameter and marks the parameters set (`SetTParam`).
    /// `snap_to_ivl` only matters to upstream's debug build.
    pub fn set_t_param(&mut self, t_param: f64, snap_to_ivl: bool) {
        let _ = snap_to_ivl;
        self.t_param = t_param;
        self.param_set = true;
    }

    /// Sets the surface or face parameters and marks the parameters set
    /// (`SetUVParam`).
    pub fn set_uv_param(&mut self, uv_param: Point2) {
        self.uv_param = uv_param;
        self.param_set = true;
    }

    /// Sets the volume parameters and marks the parameters set (`SetUVWParam`).
    pub fn set_uvw_param(&mut self, uvw_param: Point3) {
        self.uvw_param = uvw_param;
        self.param_set = true;
    }

    /// Clears the parameters-set flag (`UnSetParameters`).
    pub fn unset_parameters(&mut self) {
        self.param_set = false;
    }

    /// Sets the parameter on the curve being classified (`SetPreSnapParam`).
    pub fn set_pre_snap_param(&mut self, pre_snap_param: f64) {
        self.pre_snap_param = pre_snap_param;
    }

    /// Sets the surf/surf/surf point and its flag (`SetSSSPoint`).
    pub fn set_sss_point(&mut self, sss_point: Point3) {
        self.sss_point_flag = true;
        self.sss_point = sss_point;
    }

    /// The zone tolerance of what was classified (`GetTolerance`, the obsolete name of
    /// `GetSrcZoneTol3d`).
    #[must_use]
    pub const fn tolerance(&self) -> f64 {
        self.src_zone_tol_3d
    }

    /// The zone tolerance of the classification object (`GetObjZoneTol3d`,
    /// `SmTol::GetZoneTol3d(m_pObject)`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for topology objects and owned geometry (the
    /// topology units); [`SolidError::InvalidInput`] when `geometry` cannot resolve a
    /// curve or surface handle.
    pub fn obj_zone_tol_3d(&self, k: &Kernel, geometry: &impl ClassGeometry<O>) -> Result<f64> {
        object_zone_tol_3d(k, geometry, self.object)
    }

    /// The zone tolerance of what was classified (`GetSrcZoneTol3d`; its
    /// `SM_ASSERT_TOL` only reports).
    #[must_use]
    pub const fn src_zone_tol_3d(&self) -> f64 {
        self.src_zone_tol_3d
    }

    /// The intersection tolerance of the classified thing and the object
    /// (`SmTol::GetXSectTol3d(const SmPointClassification*)`).
    ///
    /// # Errors
    ///
    /// Those of [`PointClassification::obj_zone_tol_3d`].
    pub fn xsect_tol_3d(&self, k: &Kernel, geometry: &impl ClassGeometry<O>) -> Result<f64> {
        Ok(xsect_tol_3d_from_zones(
            k,
            self.src_zone_tol_3d(),
            self.obj_zone_tol_3d(k, geometry)?,
        ))
    }

    /// Sets the zone tolerance of what was classified (`SetSrcZoneTol3d`).
    pub fn set_src_zone_tol_3d(&mut self, src_zone_tol_3d: f64) {
        self.src_zone_tol_3d = src_zone_tol_3d;
    }

    /// Which of `self` and `other` takes precedence (`Combine`): the vertex with the
    /// smallest gap, then the lowest-dimension topology object; the same for points,
    /// curves, surfaces and volumes. When `self` wins without a surf/surf/surf point
    /// and `other` has one, the point is copied to `self` (and to its vertex).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] when the surf/surf/surf point is copied to a vertex
    /// classification and no arena is available ([`Self::combine_in_brep`] writes it);
    /// `self` has the point by then.
    pub fn combine(&mut self, other: &Self) -> Result<Precedence> {
        self.combine_with(other, None)
    }

    /// `Combine(other)` with the arena `self`'s topology handle names: a surf/surf/surf
    /// point propagated to a vertex classification is written to that vertex
    /// (`SmVertex::SetPoint`).
    ///
    /// # Errors
    ///
    /// Those of [`Self::combine`]; [`SolidError::InvalidInput`] when `target` is not a
    /// vertex; those of `SmVertex::SetPoint`.
    pub fn combine_in_brep(
        &mut self,
        other: &Self,
        context: CombineBrep<'_, '_, O>,
    ) -> Result<Precedence> {
        let CombineBrep {
            brep,
            target,
            handle,
            notifications,
        } = context;
        self.combine_with(
            other,
            Some(&mut |object: Option<O>, point: Point3| {
                if object.is_some_and(|object| handle(target) != object) {
                    return Err(SolidError::invalid_input(
                        OP_COMBINE,
                        "target does not resolve classification object",
                    ));
                }
                let EntityRef::Vertex(v) = target else {
                    return Err(SolidError::invalid_input(
                        OP_COMBINE,
                        "combine target is not a vertex",
                    ));
                };
                let key_of = notifications.key_of;
                let this = key_of(EntityRef::Vertex(v));
                vertex::set_point(
                    brep,
                    v,
                    point,
                    notifications.records,
                    notifications.attributes,
                    notifications.context.as_deref_mut(),
                    this,
                    &|vu| key_of(EntityRef::Vertexuse(vu)),
                )
            }),
        )
    }

    /// `Combine(other)` with the caller's `SmVertex::SetPoint`: a surf/surf/surf point
    /// propagated to a vertex classification is written by
    /// `set_vertex_point(object, point)`, with this classification's object (the
    /// vertex upstream casts `m_pObject` to) and the point, at the moment upstream
    /// calls `pVertex->SetPoint(m_vSSSPoint)`.
    ///
    /// # Errors
    ///
    /// Those of [`Self::combine`], without the deferral; those of `set_vertex_point`.
    pub fn combine_setting_vertex_point(
        &mut self,
        other: &Self,
        set_vertex_point: &mut dyn FnMut(Option<O>, Point3) -> Result<()>,
    ) -> Result<Precedence> {
        self.combine_with(other, Some(set_vertex_point))
    }

    /// [`Self::combine`] with an optional `SmVertex::SetPoint` for the propagated vertex
    /// point.
    fn combine_with(
        &mut self,
        other: &Self,
        set_vertex_point: Option<&mut dyn FnMut(Option<O>, Point3) -> Result<()>>,
    ) -> Result<Precedence> {
        use PointClassType as Pc;
        let this_class = self.point_class;
        let other_class = other.point_class;

        // When Point is classified to a Topology object - see if NewPoint values should
        // be saved: 1. save the vertex with the smallest gap, 2. save the lowest dim
        // topology object seen, 3. ignore equal dim topology objects except vertices
        let mut new_point_flag = (this_class == Pc::Vertex
            && (other_class == Pc::Vertex
                && other.object != self.object
                && other.gap_3d < self.gap_3d))
            || (this_class == Pc::Edgeuse && other_class == Pc::Vertex)
            || (this_class == Pc::Edge && other_class == Pc::Vertex)
            || (this_class == Pc::Face
                && (other_class == Pc::Vertex
                    || other_class == Pc::Edge
                    || other_class == Pc::Edgeuse))
            || (this_class == Pc::Region
                && (other_class == Pc::Vertex
                    || other_class == Pc::Edge
                    || other_class == Pc::Edgeuse
                    || other_class == Pc::Face));

        // when PointClass is classified to a geometry object - see if new point values
        // should be saved
        new_point_flag |= this_class == Pc::Unknown
            || (this_class == Pc::Point
                && (other_class == Pc::Point
                    && other.object != self.object
                    && other.gap_3d < self.gap_3d))
            || (this_class == Pc::Curve && other_class == Pc::Point)
            || (this_class == Pc::Surface
                && (other_class == Pc::Point || other_class == Pc::Curve))
            || (this_class == Pc::Volume
                && (other_class == Pc::Point
                    || other_class == Pc::Curve
                    || other_class == Pc::Surface));

        // see if SSSPointData should be propagated
        if !new_point_flag && other.sss_point_flag && !self.sss_point_flag {
            // copy the SSSPointData
            self.sss_point_flag = true;
            self.sss_point = other.sss_point;

            // set vertex position when appropriate
            if self.point_class == Pc::Vertex {
                let Some(set_vertex_point) = set_vertex_point else {
                    // UNPORTED(topology.vertex): SmVertex::SetPoint
                    // Proof (unreachable): needs a None vertex setter with an SSS point on
                    // a vertex class; 0 hits in the green difftest sweep.
                    return Err(SolidError::unported(
                        "SmVertex::SetPoint",
                        "topology.vertex",
                    ));
                };
                set_vertex_point(self.object, self.sss_point)?;
            }
        }

        Ok(if new_point_flag {
            Precedence::Other
        } else {
            Precedence::This
        })
    }

    /// Projects `point` onto the classification object and stores the gap and the
    /// object's parameters there (`ComputePointParameters`): a curve, edge or edgeuse
    /// gets its `t` (the closest point, `SM_SO_MINIMIZE`; a hundred times the
    /// intersection tolerance is only `DropPoint`'s end-snapping distance), a surface or
    /// face its `uv`, a volume its `uvw`; points and vertices only get their gap and
    /// parameters zeroed.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the point does not drop onto the curve or has no single drop
    /// point on the surface (also at a hundred times the source zone tolerance) or
    /// volume; the errors of [`Curve::drop_point`] and [`Volume::global_point_solve`];
    /// [`SolidError::Unported`] for surfaces (their point solve, unit
    /// `surface.surface_p4`) and topology objects.
    pub fn compute_point_parameters(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        point: Point3,
    ) -> Result<()> {
        self.compute_point_parameters_in(k, &mut ReadOnlyDrop(geometry), point)
    }

    /// [`PointClassification::compute_point_parameters`] over topology it may change:
    /// the point solve of a face's surface makes the face's trimmed surface cache that
    /// its fast-drop trim test reads (`smsurf_GetSurfaceCache`, which may create the
    /// face's UV trim curves) and classifies on the B-rep that
    /// [`ClassGeometryMut::topology_entity_mut`] resolves the face to, as upstream. A
    /// face the adapter cannot resolve mutably is solved as by
    /// [`PointClassification::compute_point_parameters`].
    ///
    /// # Errors
    ///
    /// Those of [`PointClassification::compute_point_parameters`].
    pub fn compute_point_parameters_mut(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ClassGeometryMut<O>,
        point: Point3,
    ) -> Result<()> {
        self.compute_point_parameters_in(k, &mut MutableDrop(geometry), point)
    }

    fn compute_point_parameters_in(
        &mut self,
        k: &Kernel,
        access: &mut impl SurfaceDrop<O>,
        point: Point3,
    ) -> Result<()> {
        let geometry = access.geometry();
        // init side effect values
        self.gap_3d = 0.0;
        self.t_param = 0.0;
        self.uv_param = Point2::new(0.0, 0.0);
        self.uvw_param = Point3::new(0.0, 0.0, 0.0);

        // locals
        let xsect_tol_3d = xsect_tol_3d_from_zones(
            k,
            object_zone_tol_3d(k, geometry, self.object)?,
            self.src_zone_tol_3d,
        );
        let xsect_tol_3d_100 = xsect_tol3d(k, xsect_tol_3d * 100.0);

        // branch on m_ePointClass to get geometry object and domain for upcoming point
        // drop
        let mut obj_curve: Option<(&Curve, Extent1d)> = None;
        // (the surface or face is named by its handle: its point solve may reach the
        // face's B-rep mutably)
        let mut obj_surface: Option<(DropTarget<O>, Extent2d)> = None;
        let mut obj_volume: Option<&Volume> = None;
        match self.point_class {
            PointClassType::Curve => {
                let c = self.curve_of(geometry, OP_COMPUTE)?;
                obj_curve = Some((c, c.natural_interval()));
            }
            PointClassType::Surface => {
                let s = self.surface_of(geometry, OP_COMPUTE)?;
                let o = self.object.ok_or_else(|| null_object(OP_COMPUTE))?;
                obj_surface = Some((DropTarget::Surface(o), s.natural_uv_domain_with_nurb(k)?));
            }
            PointClassType::Volume => {
                obj_volume = Some(
                    self.object
                        .and_then(|o| geometry.volume(o))
                        .ok_or_else(|| unresolved(OP_COMPUTE))?,
                );
            }
            PointClassType::Edgeuse | PointClassType::Edge => {
                let o = self.object.ok_or_else(|| null_object(OP_COMPUTE))?;
                let (b, e) = topology_edge(geometry, o, OP_COMPUTE)?;
                // a NULL curve is dereferenced by DropPoint upstream
                let c = edge::curve(b, e)?.ok_or_else(|| null_object(OP_COMPUTE))?;
                obj_curve = Some((c, edge::interval(b, e)?));
            }
            PointClassType::Face => {
                let o = self.object.ok_or_else(|| null_object(OP_COMPUTE))?;
                let (b, f) = topology_face(geometry, o, OP_COMPUTE)?;
                // a NULL surface is dereferenced by the point solve upstream
                face::p2::surface(b, f)?.ok_or_else(|| null_object(OP_COMPUTE))?;
                obj_surface = Some((DropTarget::Face(o), face::p2::uv_domain(b, f)?));
            }
            PointClassType::Point
            | PointClassType::Vertex
            | PointClassType::Unknown
            | PointClassType::Region
            | PointClassType::Nothing => {}
        }

        // when point classifies to an edge or curve branch
        if let Some((curve, ivl)) = obj_curve {
            // project point to edge->Curve to get dGap3d, dParam, and dDist
            let dropped = curve.drop_point(
                k,
                &ivl,
                point,
                None,
                xsect_tol_3d_100,
                None,
                SolverOperation::Minimize,
            )?;

            // If we fail then for now we'll error out
            if !dropped.success {
                return Err(SolidError::sm_err(OP_COMPUTE));
            }

            // save param and deviation
            self.gap_3d = dropped.distance;
            self.t_param = dropped.parameter;
            self.param_set = true;
        }
        // when point classifies to a face or surface branch
        else if let Some((target, uv_domain)) = obj_surface {
            // find point/face->surface intersection point to face->Tolerance
            let mut solutions =
                access.global_point_solve(k, target, &uv_domain, point, self.src_zone_tol_3d)?;
            // not upstream (WR-537, opt-in): solutions that are one point are one
            if k.fixes.seam_point_solutions_are_one {
                one_point(
                    k,
                    access.geometry(),
                    target,
                    &mut solutions,
                    self.src_zone_tol_3d,
                )?;
            }

            // when a distinct solution was not found
            if solutions.len() != 1 {
                // find point/face->surface intersection point to 100 * face->Tolerance
                solutions = access.global_point_solve(
                    k,
                    target,
                    &uv_domain,
                    point,
                    zone_tol3d(k, self.src_zone_tol_3d * 100.0),
                )?;
                if k.fixes.seam_point_solutions_are_one {
                    one_point(
                        k,
                        access.geometry(),
                        target,
                        &mut solutions,
                        self.src_zone_tol_3d,
                    )?;
                }

                // still no solutions - return an error
                if solutions.len() != 1 {
                    return Err(SolidError::sm_err(OP_COMPUTE));
                }
            }

            // save param and deviation
            let first = solutions.first().ok_or_else(|| unresolved(OP_COMPUTE))?;
            self.gap_3d = first.solution_value;
            self.uv_param = Point2::new(first.param(0)?, first.param(1)?);
            self.param_set = true;
        }
        // when point classifies to a volume branch
        else if let Some(volume) = obj_volume {
            let param_domain = volume.natural_param_domain()?;
            let Some(solution) = volume_global_point_solve(k, volume, point, &param_domain)? else {
                return Err(SolidError::sm_err(OP_COMPUTE));
            };

            // save param and deviation
            self.gap_3d = solution.solution_value;
            self.uvw_param =
                Point3::new(solution.param(0)?, solution.param(1)?, solution.param(2)?);
            self.param_set = true;
        }

        Ok(())
    }

    /// The object point at the stored parameters (`FindObjPoint3d`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the parameters are not set or the classification has no point
    /// (unknown, nothing, region); the errors of the evaluations;
    /// [`SolidError::Unported`] for edge uses, faces and vertex or edge handles
    /// `geometry` cannot resolve to its B-rep.
    pub fn find_obj_point_3d(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
    ) -> Result<Point3> {
        if !self.are_parameters_set() {
            return Err(SolidError::sm_err(OP_FIND));
        }

        // switch on classification type (`SM_CAST_PTR` is NULL for an object of
        // another kind, and `NER` returns SM_ERR_NULL_POINTER)
        match self.point_class {
            PointClassType::Point => {
                let o = self.object.ok_or_else(|| null_object(OP_FIND))?;
                geometry.point(o).ok_or_else(|| unresolved(OP_FIND))
            }
            PointClassType::Curve => {
                let o = self.cast(ClassObjectKind::Curve)?;
                let curve = geometry.curve(o).ok_or_else(|| unresolved(OP_FIND))?;
                curve.evaluate_point(k, self.t_param)
            }
            PointClassType::Surface => {
                let o = self.cast(ClassObjectKind::Surface)?;
                let surface = geometry.surface(o).ok_or_else(|| unresolved(OP_FIND))?;
                surface.evaluate_point(k, self.uv_param)
            }
            PointClassType::Volume => {
                let o = self.cast(ClassObjectKind::Volume)?;
                let volume = geometry.volume(o).ok_or_else(|| unresolved(OP_FIND))?;
                volume_evaluate_point(volume, self.uvw_param)
            }
            PointClassType::Vertex => {
                let o = self.cast(ClassObjectKind::Vertex)?;
                if let Some(p) = geometry.classification_vertex_point(o) {
                    return Ok(p);
                }
                match topology_of(geometry, o, "SmVertex::GetPoint", "topology.vertex")? {
                    (b, EntityRef::Vertex(v)) => vertex::point(b, v),
                    _ => Err(wrong_entity(OP_FIND)),
                }
            }
            PointClassType::Edge | PointClassType::Edgeuse => {
                let o = self.cast(if self.point_class == PointClassType::Edge {
                    ClassObjectKind::Edge
                } else {
                    ClassObjectKind::Edgeuse
                })?;
                let (b, e) = topology_edge(geometry, o, OP_FIND)?;
                let curve = edge::curve(b, e)?.ok_or_else(|| null_object(OP_FIND))?;
                curve.evaluate_point(k, self.t_param)
            }
            PointClassType::Face => {
                let o = self.cast(ClassObjectKind::Face)?;
                let (b, f) = topology_face(geometry, o, OP_FIND)?;
                let surface = face::p2::surface(b, f)?.ok_or_else(|| null_object(OP_FIND))?;
                surface.evaluate_point(k, self.uv_param)
            }
            PointClassType::Unknown | PointClassType::Nothing | PointClassType::Region => {
                Err(SolidError::sm_err(OP_FIND))
            }
        }
    }

    /// `SM_CAST_PTR(<kind>, m_pObject)` followed by `NER`: the object when it is of
    /// `kind`, else `SM_ERR_NULL_POINTER`.
    fn cast(&self, kind: ClassObjectKind) -> Result<O> {
        self.object
            .filter(|o| o.class_kind() == kind)
            .ok_or_else(|| null_object(OP_FIND))
    }

    /// Inserts the classified point into its object (`MergeIntoObject`): a shell
    /// vertex in a region, a vertex loop in a face, a vertex splitting an edge, or the
    /// existing vertex; the vertex then moves to the surf/surf/surf point if there is
    /// one, and its position is chosen so that its neighbourhood covers both its own
    /// and `src_point`'s. The classification becomes that vertex.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for no object (upstream's `NER`); `SM_ERR` for an object
    /// that is not a region, face, edge or vertex. A region, face or edge is edited in
    /// its owning B-rep, which [`Self::merge_into_brep`] takes mutably: through an
    /// opaque handle the edit returns `Unported` here.
    pub fn merge_into_object(&mut self, k: &Kernel, src_point: Point3) -> Result<MergeOutputs<O>> {
        let _ = k;
        let object = self.object.ok_or_else(|| null_object(OP_MERGE))?;
        let src_zone_tol_3d = self.src_zone_tol_3d();
        let outputs = MergeOutputs {
            new_edges: Vec::new(),
            new_vertices: Vec::new(),
        };

        // the vertex the point merges into
        let new_v: O = match object.class_kind() {
            ClassObjectKind::Region | ClassObjectKind::Face | ClassObjectKind::Edge => {
                // `GetBrep()->MakeShellVertex / MakeVertexLoop / MakeVertexSplitEdge`
                // edit the B-rep that owns the object, which an opaque handle cannot
                // reach mutably (the face's `GetBrep` itself is ported): the edit runs
                // through merge_into_brep, which callers with the B-rep use.
                // DISPATCH GAP: SmBrep mutable owning context (opaque handle without a Brep)
                return Err(SolidError::unported(
                    "SmBrep mutable owning context",
                    "topology.brep_p1",
                ));
            }
            // Almost no work to do. There is already a vertex here.
            ClassObjectKind::Vertex => object,
            // else error - point is not classified to Vertex, Edge, Face, or Region
            ClassObjectKind::Curve
            | ClassObjectKind::Surface
            | ClassObjectKind::Volume
            | ClassObjectKind::Point
            | ClassObjectKind::Loop
            | ClassObjectKind::Vertexuse
            | ClassObjectKind::Edgeuse
            | ClassObjectKind::Faceuse
            | ClassObjectKind::Loopuse
            | ClassObjectKind::Other => return Err(SolidError::sm_err(OP_MERGE)),
        };

        // arrive here when new_v = new or found Vertex that classifies to SrcPoint
        let _init_v_point = vertex_point(new_v)?;

        // update this pointClassification
        self.set_class_object(PointClassType::Vertex, Some(new_v));

        // when the PointClassification has a SSSPoint
        if self.sss_point_flag {
            // set the vertex to the SSSPoint location
            set_vertex_point(new_v, self.sss_point)?;
        }

        // adjust vertex tolerance as needed
        let new_v_point = vertex_point(new_v)?;
        let scaled_zero = vertex_scaled_zero(new_v)?;
        let new_v_zone_tol_3d = vertex_tolerance(new_v)?;
        let merged = merge_vertex_neighborhood(
            src_point,
            src_zone_tol_3d,
            new_v_point,
            new_v_zone_tol_3d,
            scaled_zero,
        );

        // remember when vertex is representing two different exact points
        set_vertex_exact(new_v, merged.exact)?;

        // adjust the Vertex position and Tolerance (the reference build is
        // SM_USE_OLDTOL, SmConfig.h:227, so SetTolerance is called)
        set_vertex_point(new_v, merged.position)?;
        set_vertex_tolerance(new_v, merged.zone_tol_3d)?;

        Ok(outputs)
    }

    /// `MergeIntoObject` with an explicit mutable B-rep and handle mapping.
    ///
    /// A refused edge split with a nearby endpoint changes only the classification
    /// and returns immediately, before SSS relocation and tolerance adjustment.
    /// This matches the native status/output lifecycle.
    ///
    /// # Errors
    /// Mismatched target/handle, missing target records, native operation errors,
    /// notification failures and owned dependency deferrals.
    pub fn merge_into_brep(
        &mut self,
        k: &mut Kernel,
        src_point: Point3,
        context: BrepMerge<'_, '_, O>,
    ) -> Result<MergeOutputs<O>> {
        self.merge_into_brep_with_curve_identity(k, src_point, context.into())
    }

    /// The same merge with checked identity issuance for actual copied curves.
    /// # Errors
    /// Original merge errors and identity failures, retaining prior edits.
    pub fn merge_into_brep_with_curve_identity(
        &mut self,
        k: &mut Kernel,
        src_point: Point3,
        context: BrepMergeWithIdentity<'_, '_, O>,
    ) -> Result<MergeOutputs<O>> {
        use crate::error::EntityRef;
        use crate::geom::tol::ScaledZero;
        use crate::topology::{brep::p9, vertex};
        let BrepMergeWithIdentity {
            brep: b,
            target,
            handle,
            notifications: env,
            curve_identity,
        } = context;
        let object = self.object.ok_or_else(|| null_object(OP_MERGE))?;
        let kind = match target {
            EntityRef::Region(_) => ClassObjectKind::Region,
            EntityRef::Face(_) => ClassObjectKind::Face,
            EntityRef::Edge(_) => ClassObjectKind::Edge,
            EntityRef::Vertex(_) => ClassObjectKind::Vertex,
            _ => return Err(SolidError::sm_err(OP_MERGE)),
        };
        if object.class_kind() != kind || handle(target) != object {
            return Err(SolidError::invalid_input(
                OP_MERGE,
                "target does not resolve classification object",
            ));
        }
        let mut outputs = MergeOutputs {
            new_edges: Vec::new(),
            new_vertices: Vec::new(),
        };
        let vertex_id = match target {
            EntityRef::Region(r) => {
                let (_, v) = p9::make_shell_vertex(b, Some(r), src_point, env)?;
                outputs.new_vertices.push(handle(EntityRef::Vertex(v)));
                v
            }
            EntityRef::Face(f) => {
                let (_, v) = p9::make_vertex_loop(
                    b,
                    f,
                    src_point,
                    crate::nurbs::b_spline_surface_n_lib::p2::OrientType::Opposite,
                    env,
                )?;
                outputs.new_vertices.push(handle(EntityRef::Vertex(v)));
                v
            }
            EntityRef::Edge(e) => {
                let split = p9::make_vertex_split_edge_with_curve_identity(
                    k,
                    b,
                    e,
                    self.t_param(),
                    env,
                    curve_identity,
                )?;
                let v = split.vertex.ok_or_else(|| SolidError::sm_err(OP_MERGE))?;
                if !split.split {
                    // Native assigns these two fields directly; in particular an
                    // unset parameter flag remains unset on this early return.
                    self.point_class = PointClassType::Vertex;
                    self.object = Some(handle(EntityRef::Vertex(v)));
                    return Ok(outputs);
                }
                let edges = split.edges.ok_or_else(|| {
                    SolidError::invalid_input(OP_MERGE, "successful split has no edges")
                })?;
                for edge in edges {
                    outputs.new_edges.push(handle(EntityRef::Edge(edge)));
                }
                outputs.new_vertices.push(handle(EntityRef::Vertex(v)));
                v
            }
            EntityRef::Vertex(v) => v,
            _ => return Err(SolidError::sm_err(OP_MERGE)),
        };
        let _initial_point = b.vertex(vertex_id)?.point;
        self.set_class_object(
            PointClassType::Vertex,
            Some(handle(EntityRef::Vertex(vertex_id))),
        );
        let set_point = |b: &mut Brep, env: &mut edge::NotifyEnv<'_>, point| {
            let this = (env.key_of)(EntityRef::Vertex(vertex_id));
            vertex::set_point(
                b,
                vertex_id,
                point,
                env.records,
                env.attributes,
                env.context.as_deref_mut(),
                this,
                &|vu| (env.key_of)(EntityRef::Vertexuse(vu)),
            )
        };
        if self.sss_point_flag {
            set_point(b, env, self.sss_point)?;
        }
        let point = b.vertex(vertex_id)?.point;
        let merged = merge_vertex_neighborhood(
            src_point,
            self.src_zone_tol_3d(),
            point,
            vertex::tolerance(b, vertex_id)?,
            point.scaled_zero(),
        );
        vertex::set_exact(b, vertex_id, merged.exact)?;
        set_point(b, env, merged.position)?;
        vertex::set_tolerance(b, vertex_id, merged.zone_tol_3d, true, true)?;
        Ok(outputs)
    }

    /// Loads the classification from an intersection of something with zone tolerance
    /// `src_zone_tol_3d` and `object` (`LoadFromIntersection`): the class from the
    /// object's kind, the gap from the solution value and the object's parameters from
    /// `end` starting at `offset`. Upstream's default `pre_snap_param` is
    /// `SM_BIG_DOUBLE`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an object that is not a curve, surface, volume, vertex, edge or
    /// face (the source tolerance is set by then); [`SolidError::InvalidInput`] for a
    /// parameter index past the solution's parameters.
    pub fn load_from_intersection(
        &mut self,
        src_zone_tol_3d: f64,
        object: O,
        end: &SolutionEnd,
        offset: usize,
        pre_snap_param: f64,
    ) -> Result<()> {
        // tolerance copy
        self.set_src_zone_tol_3d(src_zone_tol_3d);

        match object.class_kind() {
            ClassObjectKind::Curve => {
                self.set_class_object(PointClassType::Curve, Some(object));
                self.set_gap_3d(end.solution_value);
                self.set_t_param(end.param(offset)?, false);
            }
            ClassObjectKind::Surface => {
                self.set_class_object(PointClassType::Surface, Some(object));
                self.set_gap_3d(end.solution_value);
                self.set_uv_param(Point2::new(
                    end.param(offset)?,
                    end.param(param_index(offset, 1)?)?,
                ));
            }
            ClassObjectKind::Volume => {
                self.set_class_object(PointClassType::Volume, Some(object));
                self.set_gap_3d(end.solution_value);
                self.set_uvw_param(Point3::new(
                    end.param(offset)?,
                    end.param(param_index(offset, 1)?)?,
                    end.param(param_index(offset, 2)?)?,
                ));
            }
            ClassObjectKind::Vertex => {
                self.set_class_object(PointClassType::Vertex, Some(object));
                self.set_gap_3d(end.solution_value);
            }
            ClassObjectKind::Edge => {
                self.set_class_object(PointClassType::Edge, Some(object));
                self.set_gap_3d(end.solution_value);
                self.set_t_param(end.param(offset)?, false);
            }
            ClassObjectKind::Face => {
                self.set_class_object(PointClassType::Face, Some(object));
                self.set_gap_3d(end.solution_value);
                self.set_uv_param(Point2::new(
                    end.param(offset)?,
                    end.param(param_index(offset, 1)?)?,
                ));
            }
            ClassObjectKind::Point
            | ClassObjectKind::Loop
            | ClassObjectKind::Region
            | ClassObjectKind::Vertexuse
            | ClassObjectKind::Edgeuse
            | ClassObjectKind::Faceuse
            | ClassObjectKind::Loopuse
            | ClassObjectKind::Other => return Err(SolidError::sm_err(OP_LOAD)),
        }

        // save the preSnapParam value when Point solution comes from a curve/object
        // solution
        self.set_pre_snap_param(pre_snap_param);

        Ok(())
    }

    /// The curve of a curve classification.
    fn curve_of<'g>(
        &self,
        geometry: &'g impl ClassGeometry<O>,
        op: &'static str,
    ) -> Result<&'g Curve> {
        self.object
            .and_then(|o| geometry.curve(o))
            .ok_or_else(|| unresolved(op))
    }

    /// The surface of a surface classification.
    fn surface_of<'g>(
        &self,
        geometry: &'g impl ClassGeometry<O>,
        op: &'static str,
    ) -> Result<&'g Surface> {
        self.object
            .and_then(|o| geometry.surface(o))
            .ok_or_else(|| unresolved(op))
    }
}

/// `SmTol::GetZoneTol3d(const SmObject*)` (with `SM_USE_OLDTOL`): the system zone
/// tolerance for no object, a curve or surface that no edge or face owns, and every
/// non-topology object; the topology object's own tolerance otherwise.
pub fn object_zone_tol_3d<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    object: Option<O>,
) -> Result<f64> {
    let system = zone_tol_3d(k, SmallTopoType::NormalSize);
    let Some(o) = object else {
        return Ok(system);
    };
    if matches!(
        o.class_kind(),
        ClassObjectKind::Vertex
            | ClassObjectKind::Edge
            | ClassObjectKind::Face
            | ClassObjectKind::Edgeuse
    ) {
        if let Some(tolerance) = geometry.topology_zone_tol_3d(o) {
            return Ok(tolerance);
        }
    }
    match o.class_kind() {
        ClassObjectKind::Vertex => {
            match topology_of(geometry, o, "SmVertex::GetTolerance", "topology.vertex")? {
                (b, EntityRef::Vertex(v)) => vertex::tolerance(b, v),
                _ => Err(wrong_entity(OP_GEOMETRY)),
            }
        }
        ClassObjectKind::Edge => {
            match topology_of(geometry, o, "SmEdge::GetTolerance", "topology.edge")? {
                (b, EntityRef::Edge(e)) => edge::tolerance(b, e),
                _ => Err(wrong_entity(OP_GEOMETRY)),
            }
        }
        ClassObjectKind::Face => {
            match topology_of(geometry, o, "SmFace::GetTolerance", "topology.face_p2")? {
                (b, EntityRef::Face(f)) => face::p2::tolerance(b, f),
                _ => Err(wrong_entity(OP_GEOMETRY)),
            }
        }
        ClassObjectKind::Edgeuse => {
            match topology_of(
                geometry,
                o,
                "SmEdgeuse::GetTolerance",
                "topology.edgeuse_p1",
            )? {
                (b, EntityRef::Edgeuse(eu)) => edgeuse::p1::tolerance(b, eu),
                _ => Err(wrong_entity(OP_GEOMETRY)),
            }
        }
        // no GetTolerance override: SmTopology::GetTolerance, SM_ZONE_TOL_3D (unit
        // topology.topology, SM_USE_OLDTOL)
        ClassObjectKind::Faceuse
        | ClassObjectKind::Loop
        | ClassObjectKind::Loopuse
        | ClassObjectKind::Vertexuse
        | ClassObjectKind::Region => Ok(ZONE_TOL_3D),
        ClassObjectKind::Surface => {
            let surface = geometry.surface(o).ok_or_else(|| unresolved(OP_GEOMETRY))?;
            // GetFace() ? its tolerance (cached on the surface by its B-rep, else read
            // from the face the adapter resolves the owner to)
            match surface.face()? {
                None => Ok(system),
                Some(face_ref) => match surface.owner_zone_tol_3d() {
                    Some(tol) => Ok(tol),
                    None => match geometry.owner_entity(face_ref) {
                        Some((b, EntityRef::Face(f))) => face::p2::tolerance(b, f),
                        Some(_) => Err(wrong_entity(OP_GEOMETRY)),
                        // UNPORTED(topology.face_p2): SmFace::GetTolerance (a bare face
                        // owner the adapter cannot resolve)
                        // Proof (unreachable): a B-rep face's surface carries the face
                        // tolerance (Brep::sync_face_surface_tolerance invariant, see
                        // Surface::face_zone_tol_3d); 0 hits in the green difftest sweep.
                        None => Err(SolidError::unported(
                            "SmFace::GetTolerance",
                            "topology.face_p2",
                        )),
                    },
                },
            }
        }
        ClassObjectKind::Curve => {
            let curve = geometry.curve(o).ok_or_else(|| unresolved(OP_GEOMETRY))?;
            // SmCurve::GetEdge (Curve::edge) walks the owner chain
            match curve.edge(|r| geometry.owner_of(r))? {
                None => Ok(system),
                Some(edge_ref) => match geometry.owner_entity(edge_ref) {
                    Some((b, EntityRef::Edge(e))) => edge::tolerance(b, e),
                    Some(_) => Err(wrong_entity(OP_GEOMETRY)),
                    // UNPORTED(topology.edge): SmEdge::GetTolerance (an adapter that
                    // cannot resolve the curve's owning edge)
                    // Proof (unreachable): fires only when the adapter cannot resolve the
                    // curve's owning edge; the library adapters resolve it; 0 hits in the
                    // green difftest sweep.
                    None => Err(SolidError::unported(
                        "SmEdge::GetTolerance",
                        "topology.edge",
                    )),
                },
            }
        }
        // SmVolume and other objects: the "unsupported object" branch (it warns)
        ClassObjectKind::Volume | ClassObjectKind::Point | ClassObjectKind::Other => Ok(system),
    }
}

/// The position, tolerance and exactness of a merged vertex.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MergedVertex {
    position: Point3,
    zone_tol_3d: f64,
    exact: bool,
}

/// The tail of `MergeIntoObject`: the vertex position (and tolerance) whose local
/// neighbourhood includes both the vertex's (`v_point`, `v_zone_tol_3d`) and the
/// source point's (`src_point`, `src_zone_tol_3d`), with the smallest tolerance.
fn merge_vertex_neighborhood(
    src_point: Point3,
    src_zone_tol_3d: f64,
    v_point: Point3,
    v_zone_tol_3d: f64,
    scaled_zero: f64,
) -> MergedVertex {
    let mut gap_3d = v_point - src_point;
    let gap_size = gap_3d.length();

    // TRUE = vertex is representing just one point location
    let exact = gap_size < scaled_zero;

    let (position, zone_tol_3d) = if gap_size < scaled_zero {
        // use VertexPt and max ZoneTol3d
        (v_point, max2(src_zone_tol_3d, v_zone_tol_3d))
    } else if src_zone_tol_3d >= v_zone_tol_3d + gap_size {
        // use SrcPt location and ZoneTol3d
        (src_point, src_zone_tol_3d)
    } else if v_zone_tol_3d >= src_zone_tol_3d + gap_size {
        // use VertexPt location and ZoneTol3d
        (v_point, v_zone_tol_3d)
    } else {
        // place final vertex location to minimize final ZoneTol3d size while making
        // final local neighborhood include both input local neighborhoods
        gap_3d /= gap_size;
        (
            0.5 * (src_point - src_zone_tol_3d * gap_3d + v_point + v_zone_tol_3d * gap_3d),
            0.5 * (src_zone_tol_3d + gap_size + v_zone_tol_3d),
        )
    };
    MergedVertex {
        position,
        zone_tol_3d,
        exact,
    }
}

/// `SmSurface::EvaluateNormal(uv, TRUE, TRUE, normal)` (virtual).
fn surface_evaluate_normal(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Vec3> {
    surface.evaluate_normal(k, uv, Side::Left, Side::Left)
}

// ---- callees in units not ported yet --------------------------------------------

/// `pObjSurface->GlobalPointSolve(domain, SM_SO_INTERSECT, point, tol, NULL,
/// SM_SR_SINGLE, sols)`: the solutions' start ends. For a surface whose owning face
/// the adapter resolves (`owner`), the fast drop's trim test reads the trimmed
/// surface cache the face's surface keeps and classifies on the B-rep read-only.
///
/// # Errors
///
/// Those of the point solve; [`SolidError::Unported`] when the trim test would make
/// a new trimmed cache ([`face_point_test_on`]) or the read-only classification
/// would create a UV trim curve.
fn surface_global_point_solve(
    k: &Kernel,
    surface: &Surface,
    owner: Option<(&Brep, FaceId)>,
    uv_domain: &Extent2d,
    point: Point3,
    zone_tol_3d: f64,
) -> Result<Vec<SolutionEnd>> {
    let Some((b, f)) = owner else {
        return global_point_solve_starts(
            k,
            surface,
            uv_domain,
            SolverOperation::Intersect,
            point,
            zone_tol_3d,
            SolutionRequested::Single,
        );
    };
    // The fast drop's trim test: `smsurf_GetSurfaceCache(this)` is looked up when the
    // first solution is tested (upstream: once its solutions are in the domain), then
    // each solution is `pFace->PointClassify(uv, tol, TRUE, FALSE, sPtClass)`.
    let mut test_on: Option<bool> = None;
    let mut classify = |k: &Kernel, uv: Point2, src_tol: f64| -> Result<bool> {
        let on = if let Some(on) = test_on {
            on
        } else {
            let on = face_point_test_on(b, f, surface)?;
            test_on = Some(on);
            on
        };
        if !on {
            return Ok(true);
        }
        let mut pc = PointClassification::new(src_tol);
        let status = crate::query::face::p2::point_classify_prepared(
            k,
            b,
            f,
            uv,
            &mut pc,
            fast_drop_classify_options(src_tol),
        );
        fast_drop_good_point(status, &pc)
    };
    face_tested_point_solve(k, surface, uv_domain, point, zone_tol_3d, &mut classify)
}

/// The fast drop's trim test of face `f`'s surface (`pFace->PointClassify(uv, tol,
/// TRUE, FALSE, ..)` in `SmSurface::GlobalPointSolve`) with the face's B-rep at hand:
/// `true` keeps the solution. Whether the test runs at all (the face's surface cache is
/// trimmed, point testing is on and its face was not modified; `GetOwner()` is not
/// cleared) is decided on the first call and kept in `test_on`. The face's trimmed
/// surface cache is made when it has none
/// ([`crate::query::face::p2::face_surface_cache`]).
///
/// # Errors
///
/// Those of the cache lookup and the classification's [`SolidError::Unported`].
pub(crate) fn live_face_fast_drop_test(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    test_on: &mut Option<bool>,
    uv: Point2,
    src_tol: f64,
) -> Result<bool> {
    let on = if let Some(on) = *test_on {
        on
    } else {
        // `GetOwner()` is NULL while `CreateUVTrimCurve(s)` clears it: no face
        let on = !b.is_surface_owner_cleared(f) && {
            let lease = crate::query::face::p2::face_surface_cache(k, b, f)?;
            let base = lease.cache.base();
            let on = lease.trimmed && base.point_test_enabled && !base.face_was_modified;
            lease.release();
            on
        };
        *test_on = Some(on);
        on
    };
    if !on {
        return Ok(true);
    }
    let mut pc = PointClassification::new(src_tol);
    let status = crate::query::face::p2::point_classify(
        k,
        b,
        f,
        uv,
        &mut pc,
        fast_drop_classify_options(src_tol),
    );
    fast_drop_good_point(status, &pc)
}

/// [`surface_global_point_solve`] of face `f`'s surface with the face's B-rep at hand:
/// the trim test makes the face's trimmed surface cache when it has none
/// ([`crate::query::face::p2::face_surface_cache`], kept on the surface as upstream's
/// cache manager keeps it) and classifies on the B-rep, which both may change (UV trim
/// curves). The solve reads a copy of the face's surface that shares its cache slot.
///
/// # Errors
///
/// Those of the point solve, the cache lookup and the classification's
/// [`SolidError::Unported`].
fn live_surface_global_point_solve(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    uv_domain: &Extent2d,
    point: Point3,
    zone_tol_3d: f64,
) -> Result<Vec<SolutionEnd>> {
    let surface = face::p2::surface(b, f)?
        .ok_or_else(|| null_object(OP_COMPUTE))?
        .clone_for_evaluation()?;
    let mut test_on: Option<bool> = None;
    let mut classify = |k: &Kernel, uv: Point2, src_tol: f64| -> Result<bool> {
        live_face_fast_drop_test(k, b, f, &mut test_on, uv, src_tol)
    };
    face_tested_point_solve(k, &surface, uv_domain, point, zone_tol_3d, &mut classify)
}

/// `GlobalPointSolve(domain, op, point, tol, NULL, requested, sols)` of `surface`, the
/// surface of face `f` of `b` (or a copy standing for it), with the fast drop's trim
/// test on the B-rep ([`live_face_fast_drop_test`]: the face's trimmed surface cache
/// is made when it has none): the solutions' start ends.
///
/// # Errors
///
/// Those of the point solve, the cache lookup and the classification.
#[allow(clippy::too_many_arguments)]
pub(crate) fn live_owned_surface_global_point_solve(
    k: &Kernel,
    surface: &Surface,
    b: &mut Brep,
    f: FaceId,
    uv_domain: &Extent2d,
    op: SolverOperation,
    point: Point3,
    zone_tol_3d: f64,
    requested: SolutionRequested,
) -> Result<Vec<SolutionEnd>> {
    let mut test_on: Option<bool> = None;
    let mut classify = |k: &Kernel, uv: Point2, src_tol: f64| -> Result<bool> {
        live_face_fast_drop_test(k, b, f, &mut test_on, uv, src_tol)
    };
    let solutions = surface.global_point_solve_face_tested::<NoObject>(
        k,
        Some(NoObject),
        uv_domain,
        op,
        point,
        zone_tol_3d,
        None,
        requested,
        Some(&mut classify),
    )?;
    Ok(solutions.as_slice().iter().map(|s| s.start).collect())
}

/// `GlobalPointSolve(domain, SM_SO_INTERSECT, point, tol, NULL, SM_SR_SINGLE, sols)`
/// of a face's surface whose fast-drop trim test is `classify`: the solutions' start
/// ends.
fn face_tested_point_solve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    point: Point3,
    zone_tol_3d: f64,
    classify: &mut dyn FnMut(&Kernel, Point2, f64) -> Result<bool>,
) -> Result<Vec<SolutionEnd>> {
    let solutions = surface.global_point_solve_face_tested::<NoObject>(
        k,
        Some(NoObject),
        uv_domain,
        SolverOperation::Intersect,
        point,
        zone_tol_3d,
        None,
        SolutionRequested::Single,
        Some(classify),
    )?;
    Ok(solutions.as_slice().iter().map(|s| s.start).collect())
}

/// `pFace->PointClassify(uv, sSrcZoneTol3d, TRUE, FALSE, sPtClass)`'s arguments in the
/// fast drop's trim test.
fn fast_drop_classify_options(
    src_tol: f64,
) -> crate::query::face::p2::PointClassifyOptions<'static> {
    crate::query::face::p2::PointClassifyOptions {
        src_zone_tol_3d: src_tol,
        do_boundary_intersections: true,
        use_uv_classification: false,
        ..crate::query::face::p2::PointClassifyOptions::default()
    }
}

/// `bGoodPoint` of the fast drop's trim test: `SM_SUCCESS` and a face, edge or vertex
/// class. [`SolidError::Unported`] propagates.
fn fast_drop_good_point(status: Result<()>, pc: &PointClassification<EntityRef>) -> Result<bool> {
    match status {
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(false),
        Ok(()) => Ok(matches!(
            pc.point_class(),
            PointClassType::Face | PointClassType::Edge | PointClassType::Vertex
        )),
    }
}

/// The surface `ComputePointParameters` drops its point on: the classification's
/// surface (`SM_PC_SURFACE`) or the surface of its face (`SM_PC_FACE`), by handle.
#[derive(Debug, Clone, Copy)]
enum DropTarget<O> {
    Surface(O),
    Face(O),
}

/// Not upstream (WR-537, opt-in `Kernel::fixes.seam_point_solutions_are_one`; the fork's
/// source fork@577f7be): a point on the seam of a face that closes on itself (a
/// whole cylinder's wall) has its parameters at both ends of the period, several
/// solutions that are one point. When every solution evaluates within `tol` of the
/// first's point, only the nearest (the first of the smallest solution values) is
/// kept; a failed evaluation keeps them all.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluations; [`SolidError::InvalidInput`] for an
/// object that does not resolve to a surface or face with a surface.
fn one_point<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    target: DropTarget<O>,
    solutions: &mut Vec<SolutionEnd>,
    tol: f64,
) -> Result<()> {
    if solutions.len() < 2 {
        return Ok(());
    }
    let surface = match target {
        DropTarget::Surface(o) => geometry.surface(o).ok_or_else(|| unresolved(OP_COMPUTE))?,
        DropTarget::Face(o) => {
            let (b, f) = topology_face(geometry, o, OP_COMPUTE)?;
            face::p2::surface(b, f)?.ok_or_else(|| null_object(OP_COMPUTE))?
        }
    };
    let evaluate = |sol: &SolutionEnd| -> Result<Option<Point3>> {
        let uv = Point2::new(sol.param(0)?, sol.param(1)?);
        match surface.evaluate_point(k, uv) {
            Ok(p) => Ok(Some(p)),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Ok(None),
        }
    };
    let Some(first) = solutions.first() else {
        return Ok(());
    };
    let Some(first_point) = evaluate(first)? else {
        return Ok(());
    };
    let mut best = *first;
    for sol in solutions.iter().skip(1) {
        match evaluate(sol)? {
            Some(p) if p.distance(first_point) > tol => return Ok(()),
            Some(_) => {}
            None => return Ok(()),
        }
        if sol.solution_value < best.solution_value {
            best = *sol;
        }
    }
    solutions.clear();
    solutions.push(best);
    Ok(())
}

/// How `ComputePointParameters` reaches its objects: the geometry, and the point solve
/// of its surface or face, read-only or with the face's B-rep mutable.
trait SurfaceDrop<O> {
    type Geometry: ClassGeometry<O>;
    fn geometry(&self) -> &Self::Geometry;
    fn global_point_solve(
        &mut self,
        k: &Kernel,
        target: DropTarget<O>,
        uv_domain: &Extent2d,
        point: Point3,
        zone_tol_3d: f64,
    ) -> Result<Vec<SolutionEnd>>;
}

/// Read-only access ([`PointClassification::compute_point_parameters`]).
struct ReadOnlyDrop<'g, G>(&'g G);

impl<O: ClassObject, G: ClassGeometry<O>> SurfaceDrop<O> for ReadOnlyDrop<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn global_point_solve(
        &mut self,
        k: &Kernel,
        target: DropTarget<O>,
        uv_domain: &Extent2d,
        point: Point3,
        zone_tol_3d: f64,
    ) -> Result<Vec<SolutionEnd>> {
        let geometry = self.0;
        let (surface, owner) = match target {
            DropTarget::Surface(o) => {
                let s = geometry.surface(o).ok_or_else(|| unresolved(OP_COMPUTE))?;
                (s, owner_face(geometry, s))
            }
            DropTarget::Face(o) => {
                let (b, f) = topology_face(geometry, o, OP_COMPUTE)?;
                let s = face::p2::surface(b, f)?.ok_or_else(|| null_object(OP_COMPUTE))?;
                (s, Some((b, f)))
            }
        };
        surface_global_point_solve(k, surface, owner, uv_domain, point, zone_tol_3d)
    }
}

/// Mutable access ([`PointClassification::compute_point_parameters_mut`]).
struct MutableDrop<'g, G>(&'g mut G);

impl<O: ClassObject, G: ClassGeometryMut<O>> SurfaceDrop<O> for MutableDrop<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn global_point_solve(
        &mut self,
        k: &Kernel,
        target: DropTarget<O>,
        uv_domain: &Extent2d,
        point: Point3,
        zone_tol_3d: f64,
    ) -> Result<Vec<SolutionEnd>> {
        let face = match target {
            DropTarget::Face(o) => Some(o),
            DropTarget::Surface(o) => {
                let geometry = &*self.0;
                geometry
                    .surface(o)
                    .and_then(|s| owner_face(geometry, s))
                    .and_then(|(b, f)| geometry.topology_handle_in(b, EntityRef::Face(f)))
            }
        };
        if let Some((b, EntityRef::Face(f))) =
            face.and_then(|o| self.0.topology_entity_for_query(o))
        {
            return live_surface_global_point_solve(k, b, f, uv_domain, point, zone_tol_3d);
        }
        ReadOnlyDrop(&*self.0).global_point_solve(k, target, uv_domain, point, zone_tol_3d)
    }
}

/// The face owning `surface` (`SM_CAST_PTR(SmFace, GetOwner())`), when the adapter
/// resolves the owner ([`ClassGeometry::owner_entity`]).
fn owner_face<'g, O>(
    geometry: &'g impl ClassGeometry<O>,
    surface: &Surface,
) -> Option<(&'g Brep, FaceId)> {
    let owner = surface.owner().filter(|o| o.type_code == FACE_TYPE)?;
    match geometry.owner_entity(owner) {
        Some((b, EntityRef::Face(f))) => Some((b, f)),
        _ => None,
    }
}

/// Whether `smsurf_GetSurfaceCache(pFace->GetSurface())` is a trimmed surface cache
/// with point testing on (`m_bPointTestEnabled`) whose face was not modified
/// (`m_bFaceWasModified`): the condition of `GlobalPointSolve`'s fast-drop trim test.
/// A face whose surface owner is cleared (inside `CreateUVTrimCurve(s)`) has no face
/// upstream, so no test. The lookup reuses the trimmed cache the surface keeps, as
/// [`crate::query::face::p2::face_surface_cache`] does.
///
/// # Errors
///
/// [`SolidError::Unported`] when the lookup would make a new cache (none kept, one in
/// use, or a kept one whose face was modified outside a B-rep edit): that build
/// creates the face's UV trim curves, which read-only topology cannot.
fn face_point_test_on(b: &Brep, f: FaceId, surface: &Surface) -> Result<bool> {
    if b.is_surface_owner_cleared(f) {
        return Ok(false);
    }
    let kept = match surface.take_trim_cache() {
        TrimCacheSlot::Stored(cache) => {
            let base = cache.base();
            // a modified face's cache is made again unless its B-rep is being edited
            let reused = !base.face_was_modified || b.editing_enabled();
            let on = base.point_test_enabled && !base.face_was_modified;
            Surface::store_trim_cache(*cache);
            reused.then_some(on)
        }
        TrimCacheSlot::Empty | TrimCacheSlot::CheckedOut => None,
    };
    // DISPATCH GAP: smsurf_GetSurfaceCache is ported (`query::face::p2::
    // face_surface_cache`), but making the cache needs the face's B-rep mutably.
    kept.ok_or_else(|| {
        SolidError::unported(
            "smsurf_GetSurfaceCache (a face's new trimmed cache from read-only topology)",
            "query.face_p2",
        )
    })
}

/// The solution handle of the volume point solve, whose solutions never leave
/// [`volume_global_point_solve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VolumeHandle;

impl SolutionObject for VolumeHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Volume
    }
}

/// `SmVolume::GlobalPointSolve(point, bFoundAnswer, solutions, &domain)` (compounding,
/// the default): the start of the first solution when one was found. A found answer
/// without a solution (upstream then reads past the empty array) is `SM_ERR`.
fn volume_global_point_solve(
    k: &Kernel,
    volume: &Volume,
    point: Point3,
    param_domain: &Extent3d,
) -> Result<Option<SolutionEnd>> {
    let (found, solutions) =
        volume.global_point_solve(k, point, Some(param_domain), true, VolumeHandle)?;
    if !found {
        return Ok(None);
    }
    solutions
        .get(0)
        .map(|s| Some(s.start))
        .ok_or(SolidError::sm_err(OP_COMPUTE))
}

/// `SmVolume::EvaluatePoint(uvw, point)` (compounding, the default).
fn volume_evaluate_point(volume: &Volume, uvw: Point3) -> Result<Point3> {
    volume.evaluate_point(uvw, true)
}

/// `SmVertex::GetPoint`.
fn vertex_point<O>(_vertex: O) -> Result<Point3> {
    // UNPORTED(topology.vertex): SmVertex::GetPoint
    // Proof (unreachable): adapter-less free fn used only by
    // PointClassification::merge_into_object (Vertex arm); library callers use the
    // B-rep-carrying variants (merge_into_brep, Mutable); 0 hits in the green difftest
    // sweep.
    Err(SolidError::unported(
        "SmVertex::GetPoint",
        "topology.vertex",
    ))
}

/// `SmVertex::SetPoint`.
fn set_vertex_point<O>(_vertex: O, _point: Point3) -> Result<()> {
    // UNPORTED(topology.vertex): SmVertex::SetPoint
    // Proof (unreachable): adapter-less free fn used only by
    // PointClassification::merge_into_object (Vertex arm); library callers use the
    // B-rep-carrying variants (merge_into_brep, Mutable); 0 hits in the green difftest
    // sweep.
    Err(SolidError::unported(
        "SmVertex::SetPoint",
        "topology.vertex",
    ))
}

/// `SmVertex::SetTolerance(zone_tol_3d)` (defaults `bUpdateOnlyIfLarger = TRUE`, ...).
fn set_vertex_tolerance<O>(_vertex: O, _zone_tol_3d: f64) -> Result<()> {
    // UNPORTED(topology.vertex): SmVertex::SetTolerance
    // Proof (unreachable): adapter-less free fn used only by
    // PointClassification::merge_into_object (Vertex arm); library callers use the
    // B-rep-carrying variants (merge_into_brep, Mutable); 0 hits in the green difftest
    // sweep.
    Err(SolidError::unported(
        "SmVertex::SetTolerance",
        "topology.vertex",
    ))
}

/// `SmVertex::SetExact`.
fn set_vertex_exact<O>(_vertex: O, _exact: bool) -> Result<()> {
    // UNPORTED(topology.vertex): SmVertex::SetExact
    // Proof (unreachable): adapter-less free fn used only by
    // PointClassification::merge_into_object (Vertex arm); library callers use the
    // B-rep-carrying variants (merge_into_brep, Mutable); 0 hits in the green difftest
    // sweep.
    Err(SolidError::unported(
        "SmVertex::SetExact",
        "topology.vertex",
    ))
}

/// `SmTol::GetScaledZero(const SmVertex&)`.
fn vertex_scaled_zero<O>(_vertex: O) -> Result<f64> {
    // UNPORTED(topology.vertex): SmTol::GetScaledZero(const SmVertex&) (SmVertex::GetPoint)
    // Proof (unreachable): adapter-less free fn used only by
    // PointClassification::merge_into_object (Vertex arm); library callers use the
    // B-rep-carrying variants (merge_into_brep, Mutable); 0 hits in the green difftest
    // sweep.
    Err(SolidError::unported(
        "SmVertex::GetPoint",
        "topology.vertex",
    ))
}

/// `SmTol::GetZoneTol3d(const SmVertex*)`.
fn vertex_tolerance<O>(_vertex: O) -> Result<f64> {
    // UNPORTED(topology.vertex): SmVertex::GetTolerance
    // Proof (unreachable): adapter-less free fn used only by
    // PointClassification::merge_into_object (Vertex arm); library callers use the
    // B-rep-carrying variants (merge_into_brep, Mutable); 0 hits in the green difftest
    // sweep.
    Err(SolidError::unported(
        "SmVertex::GetTolerance",
        "topology.vertex",
    ))
}

// ---- topology members, through the handle hooks ------------------------------------

/// The B-rep entity behind topology handle `o` ([`ClassGeometry::topology_entity`]). An
/// adapter without that hook keeps the earlier deferral: [`SolidError::Unported`] naming
/// the upstream member (`upstream`, of `unit`) it would reach.
fn topology_of<'g, O: ClassObject>(
    geometry: &'g impl ClassGeometry<O>,
    o: O,
    upstream: &'static str,
    unit: &'static str,
) -> Result<(&'g Brep, EntityRef)> {
    geometry
        .topology_entity(o)
        .ok_or(SolidError::unported(upstream, unit))
}

/// The edge of an edge handle, or `GetEdge()` of an edge use handle (whose `NULL` edge
/// upstream dereferences).
fn topology_edge<'g, O: ClassObject>(
    geometry: &'g impl ClassGeometry<O>,
    o: O,
    op: &'static str,
) -> Result<(&'g Brep, EdgeId)> {
    let (upstream, unit) = if o.class_kind() == ClassObjectKind::Edgeuse {
        ("SmEdgeuse::GetEdge", "topology.edgeuse_p1")
    } else {
        ("SmEdge::GetCurve", "topology.edge")
    };
    match topology_of(geometry, o, upstream, unit)? {
        (b, EntityRef::Edge(e)) => Ok((b, e)),
        (b, EntityRef::Edgeuse(eu)) => {
            let e = edgeuse::p1::edge(b, eu)?.ok_or_else(|| null_object(op))?;
            Ok((b, e))
        }
        _ => Err(wrong_entity(op)),
    }
}

/// The face of a face handle.
fn topology_face<'g, O: ClassObject>(
    geometry: &'g impl ClassGeometry<O>,
    o: O,
    op: &'static str,
) -> Result<(&'g Brep, FaceId)> {
    match topology_of(geometry, o, "SmFace::GetSurface", "topology.face_p2")? {
        (b, EntityRef::Face(f)) => Ok((b, f)),
        _ => Err(wrong_entity(op)),
    }
}

/// The caller's id of the B-rep a topology `GetBrep` returned
/// ([`ClassGeometry::brep_id`]); `None` stays `None` (upstream's `NULL`).
fn brep_id_of<O>(geometry: &impl ClassGeometry<O>, brep: Option<&Brep>) -> Result<Option<BrepId>> {
    match brep {
        None => Ok(None),
        Some(b) => geometry.brep_id(b).map(Some).ok_or(
            // DISPATCH GAP: SmBrep identity (an adapter without B-rep ids)
            SolidError::unported("SmBrep identity of GetBrep", "topology.brep_p1"),
        ),
    }
}

/// A handle whose entity is not of the handle's kind (upstream's unchecked cast).
const fn wrong_entity(op: &'static str) -> SolidError {
    SolidError::invalid_input(op, "topology handle resolves to an entity of another kind")
}

// ---- helpers ----------------------------------------------------------------------

fn unresolved(op: &'static str) -> SolidError {
    SolidError::invalid_input(op, "classification object handle does not resolve")
}

fn null_object(op: &'static str) -> SolidError {
    SolidError::null_pointer(op)
}

fn param_index(offset: usize, i: usize) -> Result<usize> {
    offset
        .checked_add(i)
        .ok_or_else(|| SolidError::invalid_input(OP_LOAD, "parameter index overflow"))
}

// ---- SmSpaceBend / SmSpaceUnbend type queries --------------------------------------

/// `SmSpaceBend::GetTypeString`.
pub const SPACE_BEND_TYPE_STRING: &str = "SmSpaceBend_TYPE";

/// `SmSpaceUnbend::GetTypeString`.
pub const SPACE_UNBEND_TYPE_STRING: &str = "SmSpaceUnbend_TYPE";

/// `SmSpaceBend::GetType`.
#[must_use]
pub const fn space_bend_type() -> i64 {
    SPACE_BEND_TYPE
}

/// `SmSpaceUnbend::GetType`.
#[must_use]
pub const fn space_unbend_type() -> i64 {
    SPACE_UNBEND_TYPE
}

/// `SmSpaceBend::IsKindOf`: its own type, then `SmObject`'s.
#[must_use]
pub const fn space_bend_is_kind_of(t: i64) -> bool {
    t == SPACE_BEND_TYPE || object::is_kind_of(t)
}

/// `SmSpaceUnbend::IsKindOf`: its own type, then `SmObject`'s.
#[must_use]
pub const fn space_unbend_is_kind_of(t: i64) -> bool {
    t == SPACE_UNBEND_TYPE || object::is_kind_of(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(ClassObjectKind, u32);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            self.0
        }
    }

    struct NoGeometry;

    impl ClassGeometry<H> for NoGeometry {
        fn curve(&self, _o: H) -> Option<&Curve> {
            None
        }
        fn surface(&self, _o: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _o: H) -> Option<&Volume> {
            None
        }
        fn point(&self, _o: H) -> Option<Point3> {
            Some(Point3::new(1.0, 2.0, 3.0))
        }
    }

    fn pc() -> PointClassification<H> {
        PointClassification::new(0.25)
    }

    #[test]
    fn new_resets() {
        let p = pc();
        assert_eq!(p.point_class(), PointClassType::Unknown);
        assert_eq!(p.object(), None);
        assert_eq!(p.src_zone_tol_3d(), 0.25);
        assert_eq!(p.pre_snap_param(), BIG_DOUBLE);
        assert!(!p.are_parameters_set());
    }

    #[test]
    fn set_class_object_rules() {
        let mut p = pc();
        p.set_class_object(PointClassType::Vertex, Some(H(ClassObjectKind::Vertex, 1)));
        assert!(p.are_parameters_set());
        assert_eq!(p.vertex_object(), Some(H(ClassObjectKind::Vertex, 1)));
        assert_eq!(p.edge_object(), None);
        p.set_class_object(PointClassType::Edge, None);
        assert_eq!(p.point_class(), PointClassType::Unknown);
        assert!(!p.are_parameters_set());
    }

    #[test]
    fn combine_prefers_lower_dimension() {
        let mut face = pc();
        face.set_class_object(PointClassType::Face, Some(H(ClassObjectKind::Face, 1)));
        let mut edge = pc();
        edge.set_class_object(PointClassType::Edge, Some(H(ClassObjectKind::Edge, 2)));
        assert_eq!(face.combine(&edge).ok(), Some(Precedence::Other));
        assert_eq!(edge.combine(&face).ok(), Some(Precedence::This));
    }

    #[test]
    fn combine_propagates_sss_point() {
        let mut edge = pc();
        edge.set_class_object(PointClassType::Edge, Some(H(ClassObjectKind::Edge, 2)));
        let mut face = pc();
        face.set_class_object(PointClassType::Face, Some(H(ClassObjectKind::Face, 1)));
        face.set_sss_point(Point3::new(4.0, 5.0, 6.0));
        assert_eq!(edge.combine(&face).ok(), Some(Precedence::This));
        assert!(edge.sss_point_flag());
        assert_eq!(edge.sss_point(), Point3::new(4.0, 5.0, 6.0));
    }

    #[test]
    fn find_point_and_errors() {
        let k = &Kernel::default();
        let mut p = pc();
        assert!(p.find_obj_point_3d(k, &NoGeometry).is_err());
        p.set_class_object(PointClassType::Point, Some(H(ClassObjectKind::Point, 3)));
        assert_eq!(
            p.find_obj_point_3d(k, &NoGeometry).ok(),
            Some(Point3::new(1.0, 2.0, 3.0))
        );
    }

    #[test]
    fn merge_rejects_geometry() {
        let mut p = pc();
        assert!(p
            .merge_into_object(&Kernel::default(), Point3::new(0.0, 0.0, 0.0))
            .is_err());
        p.set_class_object(PointClassType::Curve, Some(H(ClassObjectKind::Curve, 4)));
        let e = p.merge_into_object(&Kernel::default(), Point3::new(0.0, 0.0, 0.0));
        assert!(matches!(e, Err(SolidError::Upstream { code: SM_ERR, .. })));
    }

    #[test]
    fn merge_opaque_edge_defers_without_mutable_arena_context() {
        let mut p = pc();
        let object = H(ClassObjectKind::Edge, 2);
        p.set_class_object(PointClassType::Edge, Some(object));
        p.set_t_param(0.25, false);
        assert!(matches!(
            p.merge_into_object(&Kernel::default(), Point3::new(0.25, 0.0, 0.0)),
            Err(SolidError::Unported {
                upstream: "SmBrep mutable owning context",
                unit: "topology.brep_p1"
            })
        ));
        assert_eq!(p.object(), Some(object));
        assert_eq!(p.point_class(), PointClassType::Edge);
        assert_eq!(p.t_param(), 0.25);
    }

    #[test]
    fn merge_neighborhood_cases() {
        let o = Point3::new(0.0, 0.0, 0.0);
        let x = Point3::new(1.0, 0.0, 0.0);
        let m = merge_vertex_neighborhood(o, 0.1, o, 0.2, 1e-12);
        assert!(m.exact);
        assert_eq!(m.zone_tol_3d, 0.2);
        let m = merge_vertex_neighborhood(o, 3.0, x, 0.5, 1e-12);
        assert_eq!(m.position, o);
        let m = merge_vertex_neighborhood(o, 0.5, x, 3.0, 1e-12);
        assert_eq!(m.position, x);
        let m = merge_vertex_neighborhood(o, 0.5, x, 0.5, 1e-12);
        assert_eq!(m.position, Point3::new(0.5, 0.0, 0.0));
        assert_eq!(m.zone_tol_3d, 1.0);
    }

    #[test]
    fn load_from_intersection_kinds() {
        let mut p = pc();
        let mut end = SolutionEnd::new();
        end.solution_value = 0.5;
        end.parameters[0] = 1.0;
        end.parameters[1] = 2.0;
        assert!(p
            .load_from_intersection(0.1, H(ClassObjectKind::Surface, 1), &end, 0, 7.0)
            .is_ok());
        assert_eq!(p.point_class(), PointClassType::Surface);
        assert_eq!(p.uv_param(), Point2::new(1.0, 2.0));
        assert_eq!(p.pre_snap_param(), 7.0);
        assert!(p
            .load_from_intersection(0.3, H(ClassObjectKind::Region, 1), &end, 0, 7.0)
            .is_err());
        assert_eq!(p.tolerance(), 0.3);
    }

    #[test]
    fn space_bend_types() {
        assert!(space_bend_is_kind_of(SPACE_BEND_TYPE));
        assert!(space_unbend_is_kind_of(object::OBJECT_TYPE));
        assert!(!space_bend_is_kind_of(SPACE_UNBEND_TYPE));
    }
}
