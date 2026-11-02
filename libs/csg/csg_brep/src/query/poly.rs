//! Polygonal B-reps (`SmPolyBrep`): the poly vertex / edge / loop / face / shell /
//! region graph, its traversals, bounding boxes and spheres, face normals, point in
//! polygon tests, degenerate edges, affine transforms and the object destructors.
//!
//! Upstream: `source/SMLib/inc/SmPoly.h`, `source/SMLib/src/SmPoly.cpp`,
//! `source/SMLib/src/SmPolyBrepData.cpp`, `source/SMLib/inc/SmPolyMerge.h`,
//! `source/SMLib/inc/SmGraphicsExtern.h`, `source/SM_API/src/SmApiGeneral.cpp`.
//!
//! | Rust                                         | Upstream                                          |
//! |----------------------------------------------|---------------------------------------------------|
//! | [`PolyBrep`]                                 | `SmPolyBrep` (and the objects it owns)            |
//! | [`PolyVertex`], [`PolyEdge`], [`PolyLoop`], [`PolyFace`], [`PolyShell`], [`PolyRegion`] | `SmPolyVertex`, `SmPolyEdge`, `SmPolyLoop`, `SmPolyFace` / `SmCPolyFace`, `SmPolyShell`, `SmPolyRegion` |
//! | [`PolyVertAuxData`]                          | `SmPolyVertAuxData`                               |
//! | [`PolyVertAuxData::set_index_value`]         | `SmPolyVertAuxData::SetIndexValue`                |
//! | [`PolyBrep::aux_remove_user`]                | `SmPolyVertAuxData::RemoveUser`                   |
//! | [`PolyBrep::aux_delete_if_no_users`]         | `SmPolyVertAuxData::DeleteIfNoUsers`              |
//! | [`PolyBrep::vertex_point`], [`PolyBrep::vertex_tolerance`], [`PolyBrep::vertex_original_vertex`], [`PolyBrep::vertex_normals`], [`PolyBrep::vertex_normals_mut`] | `SmPolyVertex::GetPoint` (both), `GetTolerance`, `GetOriginalVertex`, `GetNormalsRef` |
//! | [`PolyBrep::vertex_is_2d_or_zero`]           | `SmPolyVertex::Is2dOrZero`                        |
//! | [`PolyBrep::vertex_set_tolerance`]           | `SmPolyVertex::SetTolerance`                      |
//! | [`PolyBrep::vertex_set_point`]               | `SmPolyVertex::SetPoint`                          |
//! | [`PolyBrep::delete_vertex`]                  | `SmPolyVertex::~SmPolyVertex`                     |
//! | [`PolyBrep::vertex_bounding_box`]            | `SmPolyVertex::CalculateBoundingBox`              |
//! | [`PolyBrep::vertex_poly_faces`]              | `SmPolyVertex::GetPolyFaces`                      |
//! | [`PolyBrep::vertex_remove_poly_edge`]        | `SmPolyVertex::RemovePolyEdge`                    |
//! | [`PolyBrep::vertex_starting_poly_edges`]     | `SmPolyVertex::GetStartingPolyEdges`              |
//! | [`PolyBrep::vertex_poly_edges`]              | `SmPolyVertex::GetPolyEdges`                      |
//! | [`PolyBrep::edge_original_edgeuse`], [`PolyBrep::edge_tolerance`], [`PolyBrep::edge_aux_data`] | `SmPolyEdge::GetOriginalEdgeuse`, `GetTolerance`, `GetAuxData` |
//! | [`PolyBrep::edge_start_point`], [`PolyBrep::edge_end_point`] | `SmPolyEdge::GetStartPoint`, `GetEndPoint`  |
//! | [`PolyBrep::edge_start_poly_vertex`], [`PolyBrep::edge_end_poly_vertex`] | `SmPolyEdge::GetStartPolyVertex`, `GetEndPolyVertex` |
//! | [`PolyBrep::edge_cw_poly_edge`]              | `SmPolyEdge::GetCWPolyEdge`                       |
//! | [`PolyBrep::edge_radial`], [`PolyBrep::edge_all_radials`] | `SmPolyEdge::GetRadial`, `GetAllRadials`  |
//! | [`PolyBrep::edge_set_tolerance`]             | `SmPolyEdge::SetTolerance`                        |
//! | [`PolyBrep::edge_poly_loop`], [`PolyBrep::edge_poly_face`] | `SmPolyEdge::GetPolyLoop`, `GetPolyFace`    |
//! | [`PolyBrep::edge_ok_back_ptrs`]              | `SmPolyEdge::GetOKBackPtrs` (`GetPolyBrep` is `self`) |
//! | [`PolyBrep::delete_edge`]                    | `SmPolyEdge::~SmPolyEdge`                         |
//! | [`PolyBrep::edge_bounding_box`]              | `SmPolyEdge::CalculateBoundingBox`                |
//! | [`PolyBrep::edge_is_2d_or_zero`]             | `SmPolyEdge::Is2dOrZero`                          |
//! | [`PolyBrep::edge_is_degenerate`]             | `SmPolyEdge::IsDegenerate`                        |
//! | [`PolyBrep::edge_is_degenerate_with_owner`]  | `SmPolyEdge::IsDegenerate` (real-face adapter)    |
//! | [`PolyBrep::delete_loop`]                    | `SmPolyLoop::~SmPolyLoop`                         |
//! | [`PolyBrep::loop_poly_face`], [`PolyBrep::loop_first_poly_edge`], [`PolyBrep::loop_is_closed`], [`PolyBrep::loop_poly_edges`] | `SmPolyLoop::GetPolyFace`, `GetFirstPolyEdge`, `IsClosed`, `GetPolyEdges` (`GetPolyBrep` is `self`) |
//! | [`PolyBrep::loop_contains_point`]            | `SmPolyLoop::ContainsPoint`                       |
//! | [`PolyBrep::face_original_face`], [`PolyBrep::face_outer_poly_loop`], [`PolyBrep::face_tolerance`] | `SmPolyFace::GetOriginalFace`, `GetOuterPolyLoop`, `GetTolerance` |
//! | [`PolyBrep::face_set_tolerance`], [`PolyBrep::face_set_normal_uninit`] | `SmPolyFace::SetTolerance`, `SetNormalUnInit` |
//! | [`PolyBrep::face_poly_loops`], [`PolyBrep::face_poly_edges`] | `SmPolyFace::GetPolyLoops`, `GetPolyEdges`; `SmCPolyFace::GetPolyLoops`, `GetPolyEdges` |
//! | [`PolyBrep::face_ok_back_ptrs`]              | `SmPolyFace::GetOKBackPtrs`, `SmCPolyFace::GetOKBackPtrs` |
//! | [`PolyBrep::delete_face`]                    | `SmPolyFace::~SmPolyFace`, `SmCPolyFace::~SmCPolyFace` |
//! | [`PolyBrep::face_compute_sphere_bound`]      | `SmPolyFace::ComputeSphereBound`                  |
//! | [`PolyBrep::face_normal`]                    | `SmPolyFace::GetNormal`, `SmCPolyFace::GetNormal` |
//! | [`PolyBrep::face_set_original_face`]         | `SmPolyFace::SetOriginalFace`                     |
//! | [`PolyBrep::face_bounding_box`]              | `SmPolyFace::CalculateBoundingBox`, `SmCPolyFace::CalculateBoundingBox` |
//! | [`PolyBrep::face_point_in_polygon`]          | `SmPolyFace::PointInPolygon`                      |
//! | [`PolyBrep::cpoly_remove_poly_face`]         | `SmCPolyFace::RemovePolyFace`                     |
//! | [`PolyBrep::cpoly_faces_of_composite`]       | `SmCPolyFace::GetPolyFacesOfComposite`            |
//! | [`PolyBrep::shell_poly_faces`], [`PolyBrep::shell_poly_region`] | `SmPolyShell::GetPolyFaces`, `GetPolyRegion` (`GetPolyBrep` is `self`) |
//! | [`PolyBrep::delete_shell`]                   | `SmPolyShell::~SmPolyShell`                       |
//! | [`PolyBrep::region_poly_shells`]             | `SmPolyRegion::GetPolyShells` (`GetPolyBrep` is `self`) |
//! | [`PolyBrep::delete_region`]                  | `SmPolyRegion::~SmPolyRegion`                     |
//! | [`PolyBrep::tolerance`], [`PolyBrep::ok_back_ptrs`] | `SmPolyBrep::GetTolerance`, `GetOKBackPtrs` |
//! | [`PolyBrep::destroy`], [`PolyBrep::destroy_with_original`] | `SmPolyBrep::~SmPolyBrep`           |
//! | [`PolyBrep::calculate_bounding_box`]         | `SmPolyBrep::CalculateBoundingBox`                |
//! | [`PolyBrep::aux_data_list`]                  | `SmPolyBrep::GetAuxDataList`                      |
//! | [`PolyBrep::first_poly_face`]                | `SmPolyBrep::GetFirstPolyFace`                    |
//! | [`PolyBrep::poly_regions`], [`PolyBrep::poly_shells`], [`PolyBrep::poly_faces`], [`PolyBrep::poly_edges`], [`PolyBrep::poly_vertices`], [`PolyBrep::poly_points`], [`PolyBrep::cpoly_faces`] | `SmPolyBrep::GetPolyRegions`, `GetPolyShells`, `GetPolyFaces`, `GetPolyEdges`, `GetPolyVertices`, `GetPolyPoints`, `GetCPolyFaces` |
//! | [`PolyBrep::notify`]                         | `SmPolyBrep::Notify`                              |
//! | `transform_poly_normal` (private)            | `sm_TransformPolyNormal`                          |
//! | [`PolyBrep::transform`]                      | `SmPolyBrep::Transform`                           |
//! | [`PolyObjectKind`], [`PolyObjectKind::type_code`], [`PolyObjectKind::type_string`], [`PolyObjectKind::class_string`], [`PolyObjectKind::is_kind_of`] | `GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` of every poly class |
//! | [`DisplayList`], [`DisplayList::display_list_id`], [`DisplayList::set_display_list_id`] | `SmDisplayList` (its id), `GetDisplayListId`, `SetDisplayListId` |
//! | [`is_uniform_positive_scale`]                | `sm_IsUniformPositiveScale` (`SmApiGeneral.cpp`)  |
//! | [`report_invalid_transform_input`]           | `sm_ReportInvalidTransformInput` (`SmApiGeneral.cpp`) |
//! | `Drop` (implicit)                            | `~SmCopyPolyBrepMap`, `~SmPolyMerge`, `~SmTriangleBag`, `~SmPolyBrepData` |
//!
//! Construction (mirrors of upstream members outside this unit, used by callers and the
//! difftest to build a poly B-rep exactly as the C++ shim does): [`PolyBrep::new`]
//! (`SmPolyBrep(SmZoneTol3d)`), [`PolyBrep::add_region`] (`SmPolyRegion(pBrep)`),
//! [`PolyBrep::add_shell`] (`SmPolyShell(pRegion)`), [`PolyBrep::add_face`]
//! (`SmPolyFace(tol, NULL, NULL, pShell, pOptNormal)`), [`PolyBrep::add_vertex`]
//! (`SmPolyVertex(pt, tol)` posted on the vertex list), [`PolyBrep::start_poly_edge_loop`]
//! (`StartPolyEdgeLoop`), [`PolyBrep::add_poly_edge`] (`SmPolyLoop::AddPolyEdge` with a
//! start vertex and no end vertex, so no stitching), [`PolyBrep::finish_poly_edge_loop`],
//! [`PolyBrep::set_last_end_poly_vertex`], [`PolyBrep::add_cpoly_face`]
//! (`SmCPolyFace(pFace)`), [`PolyBrep::cpoly_add_poly_face`], [`PolyBrep::add_aux_data`],
//! [`PolyBrep::edge_set_aux_data`] (`SetAuxData`), [`PolyBrep::edge_set_original_edgeuse`],
//! [`PolyBrep::vertex_set_original_vertex`] (both without attribute propagation),
//! [`PolyBrep::vertex_add_face_normal`] (`AddFaceNormal`), [`PolyBrep::set_ok_back_ptrs`]
//! and [`PolyBrep::link_radials`] (sets the radial rings, which upstream builds in
//! `StitchEdgeTopological`).
//!
//! Deviations:
//! - **Data model.** Every poly object lives in a slot arena of its [`PolyBrep`] and is
//!   addressed by a typed id; a deleted object leaves an empty slot, and using its id
//!   is [`SolidError::InvalidInput`] (upstream reads freed arena memory). The owned
//!   lists of `SmOwningTopology` (brep regions, region shells, shell faces, face loops,
//!   the vertex and composite face lists) are ordered `Vec`s in list order; a loop's
//!   edges keep upstream's circular `m_pNext` / `m_pLast` links (needed for the CW /
//!   CCW neighbours) with the head and size on the loop. `PostInsert` appends, removal
//!   keeps order and moves a removed head to its successor, as upstream. A composite
//!   face (`SmCPolyFace`) is a [`PolyFace`] with a member list (`composite`); the
//!   virtual overrides of `SmCPolyFace` are the composite branches of the face
//!   methods. `GetPolyBrep` of every object is the `PolyBrep` itself.
//! - The original face / edge use / vertex back pointers are opaque [`ObjectKey`]
//!   labels. Reaching the objects behind them is outside this unit:
//!   `SmPolyEdge::IsDegenerate` needs `SmFace::GetSurface` (unit `topology.face_p2`) when
//!   the back pointers are valid and a face is set; `SetOriginalFace` with attribute
//!   propagation calls `SmFace::Notify` (unit `topology.face_p3`). The key-only APIs
//!   return [`SolidError::Unported`]; `IsDegenerate` additionally offers the explicit
//!   [`PolyBrep::edge_is_degenerate_with_owner`] adapter.
//! - The destructor of a poly B-rep with valid back pointers clears each original
//!   vertex's `m_pPolyVertex` that names its poly vertex: the caller gives the original
//!   B-rep and the resolver of the vertex keys ([`PolyBrep::destroy_with_original`]);
//!   [`PolyBrep::destroy`] without them is [`SolidError::InvalidInput`] when a back
//!   pointer would have to be cleared (upstream follows the pointer).
//! - `SmPolyBrep::CalculateBoundingBox` asked for a pseudo box without a point array
//!   dereferences the null `pOptPolyPoints` (it builds `sPolyPoints` and then reads the
//!   caller's pointer); Rust returns [`SolidError::InvalidInput`] (see
//!   `plan/upstream-issues.md`).
//! - `SmCPolyFace::RemovePolyFace` of a face that is not a member removes the first
//!   member (`FindElement` leaves the index at 0); kept (see `plan/upstream-issues.md`).
//! - A composite face cannot be a member of a composite face, nor the source of
//!   [`PolyBrep::add_cpoly_face`] ([`SolidError::InvalidInput`]). Upstream only ever
//!   adds shell faces (`SmLineSegClass.cpp`), but `AddPolyFace` would accept a composite
//!   (even itself), and the virtual `GetPolyEdges` / `GetNormal` /
//!   `CalculateBoundingBox` / `GetOKBackPtrs` of the composite would then recurse
//!   without bound; refusing nested composites keeps that recursion one level deep
//!   (WR-82 in `plan/upstream-issues.md`).
//! - `SmCPolyFace::GetNormal` of a composite without members returns `(1,1,1)` after an
//!   `SM_ASSERT`; kept.
//! - `SmPolyBrep::GetPolyEdges` with a common edge use walks each face's loop list as
//!   if it held edges (reading loops as `SmPolyEdge` objects); the port refuses that
//!   filter with [`SolidError::InvalidInput`] (see `plan/upstream-issues.md`).
//! - `SmPolyLoop::ContainsPoint` declares its basis vectors uninitialised; a loop always
//!   has a face here, so the only read of the uninitialised `sZ` cannot happen; the
//!   port starts it at `SM_UNDEF_DOUBLE`.
//! - `SmPolyBrep::Notify` and the notifications of `Transform` take a
//!   [`PolyNotifyEnv`] (records, attributes, caches, context and this object's
//!   [`ObjectRef`]); the poly B-rep's cache slot is its `aobject`.
//! - `SmPolyFace::SetOriginalFace` takes an optional [`PolyFaceCopyEnv`] only when
//!   changing to a non-null source with attribute propagation enabled. It uses the
//!   real source B-rep face and existing face notification/copy services; unchanged,
//!   null and disabled assignments need no notification context.
//! - `SmPolyFace::GetNormal` and `SmPolyLoop::ContainsPoint` are `const` upstream but
//!   cache the normal and raise tolerances through `const_cast`; here they take
//!   `&mut self`.
//! - `ULONG` index values and user counts are `u64` (LP64); `SM_UNDEF_ULONG` is
//!   [`UNDEF_ULONG`]. A user count decremented below zero wraps, as upstream.
//! - The vertex's face / index lists (`m_vFaces`, `m_vIndices`), visibility and user
//!   values are not used by this unit's functions; only `m_vNormals` is kept. The
//!   display list members of the base classes are not modelled. Attribute lists and
//!   topology notification records are supplied explicitly by callers.
//! - `sm_ReportInvalidTransformInput` sends its message through `smos_ErrorMessage`;
//!   here the message is the `reason` of the returned [`SolidError::InvalidInput`].
//! - `SmCopyPolyBrepMap`, `SmPolyMerge`, `SmTriangleBag` and `SmPolyBrepData` have no
//!   Rust type (their constructors and users are outside the in-scope call graph);
//!   their destructors only release memory, which is `Drop`.
//! - `SmOwningTopology::IsKindOf` (unit `topology.proto_topology_p1`) is co-ported as the
//!   private `owning_topology_is_kind_of`. `Transform` keeps the aux data it has already
//!   transformed in an ordered list instead of upstream's pointer-hashed
//!   `std::unordered_set`; only membership is used, so the order of work is the same.
//! - `SM_DEBUG_CODE` blocks, `WARN`, `SE` messages and `SM_ASSERT*` are not ported.
//! - `IsDegenerate` can resolve an original face through a caller-supplied B-rep
//!   adapter; the key-only API still defers. Surface evaluation errors propagate;
//!   upstream ignores their statuses.

use crate::error::{Result, SolidError};
use crate::geom::a_object::{self, AObject, CacheStore};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::Context;
use crate::geom::extent1d::ExtentPointType;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::polygon_area;
use crate::geom::geom_utility::p2::is_point_on_segment;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::point_set::PointSet3d;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::tol;
use crate::geom::vector2d::Vec2;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQ, UNDEF_DOUBLE};
use crate::topology::topology::TopologyRecords;
use crate::topology::{vertex, Brep, FaceId, VertexId};

pub mod copy;

/// `SM_UNDEF_ULONG` on LP64 (`0xFFFFFFFFFFFFFFFE`... upstream's `ULONG_MAX - 1`).
pub const UNDEF_ULONG: u64 = u64::MAX - 1;

/// `SM_ERR_INVALID_INPUT`.
const SM_ERR_INVALID_INPUT: i32 = 1007;

macro_rules! poly_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            /// Builds an id from a raw slot index.
            #[must_use]
            pub const fn from_index(index: u32) -> Self {
                Self(index)
            }

            /// The raw slot index of this id.
            #[must_use]
            pub const fn index(self) -> u32 {
                self.0
            }
        }
    };
}

poly_id!(
    /// Identifies a [`PolyVertex`] of a [`PolyBrep`].
    PolyVertexId
);
poly_id!(
    /// Identifies a [`PolyEdge`] of a [`PolyBrep`].
    PolyEdgeId
);
poly_id!(
    /// Identifies a [`PolyLoop`] of a [`PolyBrep`].
    PolyLoopId
);
poly_id!(
    /// Identifies a [`PolyFace`] (plain or composite) of a [`PolyBrep`].
    PolyFaceId
);
poly_id!(
    /// Identifies a [`PolyShell`] of a [`PolyBrep`].
    PolyShellId
);
poly_id!(
    /// Identifies a [`PolyRegion`] of a [`PolyBrep`].
    PolyRegionId
);
poly_id!(
    /// Identifies a [`PolyVertAuxData`] of a [`PolyBrep`].
    PolyAuxId
);

/// Where a point lies relative to a polygon (`SmPolyContainmentType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum PolyContainment {
    /// `SM_PCT_UNKNOWN`: degenerate loop.
    Unknown = 0,
    /// `SM_PCT_INSIDE`.
    Inside = 1,
    /// `SM_PCT_OUTSIDE`.
    Outside = 2,
    /// `SM_PCT_ON_BOUNDARY`.
    OnBoundary = 3,
}

/// Loop orientation in its face (`SmOrientType` values used by poly loops).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum PolyLoopOrientation {
    /// `SM_OT_UNKNOWN` (after destruction).
    Unknown = 0,
    /// `SM_OT_SAME`: outer loop.
    Same = 1,
    /// `SM_OT_OPPOSITE`: inner loop.
    Opposite = 2,
}

/// Normal and UV data shared by poly edges (`SmPolyVertAuxData`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolyVertAuxData {
    /// `m_lNumUsers`.
    num_users: u64,
    /// `m_lIndexValue`.
    index_value: u64,
    /// `m_vNormal`: unit normal of the original surface.
    pub normal: Vec3,
    /// `m_vUV`: UV of the original surface or a texture coordinate.
    pub uv: Vec2,
}

impl PolyVertAuxData {
    /// `SmPolyVertAuxData()`: no users, index unset, zero normal and UV.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            num_users: 0,
            index_value: UNDEF_ULONG,
            normal: Vec3::ZERO,
            uv: Vec2::new(0.0, 0.0),
        }
    }

    /// `GetNumUsers`.
    #[must_use]
    pub const fn num_users(&self) -> u64 {
        self.num_users
    }

    /// `m_lIndexValue` (`GetIndexValue` without its assertion).
    #[must_use]
    pub const fn index_value(&self) -> u64 {
        self.index_value
    }

    /// `SetIndexValue`.
    pub fn set_index_value(&mut self, index_value: u64) {
        self.index_value = index_value;
    }
}

impl Default for PolyVertAuxData {
    fn default() -> Self {
        Self::new()
    }
}

/// A polygon vertex (`SmPolyVertex`).
#[derive(Debug, Clone, PartialEq)]
pub struct PolyVertex {
    /// `m_sZoneTol3d`.
    tolerance: f64,
    /// `m_vPoint`.
    point: Point3,
    /// `m_pPolyEdgeList`: head of the ring of edges starting here.
    edge_list: Option<PolyEdgeId>,
    /// `m_lIndexValue`.
    index_value: u64,
    /// `m_pOriginalVertex`.
    original_vertex: Option<ObjectKey>,
    /// `m_vNormals`.
    normals: Vec<Vec3>,
    /// On the poly B-rep's vertex list (`m_pListOwner != NULL`).
    listed: bool,
}

/// A polygon edge (`SmPolyEdge`): the line from its start vertex to the start vertex of
/// the next edge of its loop.
#[derive(Debug, Clone, PartialEq)]
pub struct PolyEdge {
    /// `m_pListOwner`.
    owner: Option<PolyLoopId>,
    /// `m_pNext` in the loop's edge list (CCW neighbour).
    next: Option<PolyEdgeId>,
    /// `m_pLast` in the loop's edge list (CW neighbour).
    last: Option<PolyEdgeId>,
    /// `m_pNextRadialE`.
    next_radial: PolyEdgeId,
    /// `m_pLastRadialE`.
    last_radial: PolyEdgeId,
    /// `m_sZoneTol3d`.
    tolerance: f64,
    /// `m_pNextEdgeAtV`.
    next_at_v: Option<PolyEdgeId>,
    /// `m_pPrevEdgeAtV`.
    prev_at_v: Option<PolyEdgeId>,
    /// `m_pPolyVertex`: start vertex.
    vertex: Option<PolyVertexId>,
    /// `m_pAuxData`.
    aux: Option<PolyAuxId>,
    /// `m_pOriginalEdgeuse`.
    original_edgeuse: Option<ObjectKey>,
    /// `m_lIndexValue`.
    index_value: u64,
}

/// A polygon loop (`SmPolyLoop`): an ordered ring of edges.
#[derive(Debug, Clone, PartialEq)]
pub struct PolyLoop {
    /// `m_pListOwner`.
    face: Option<PolyFaceId>,
    /// `m_pList`.
    head: Option<PolyEdgeId>,
    /// `m_lListSize`.
    size: usize,
    /// `m_eOrientation`.
    orientation: PolyLoopOrientation,
    /// `m_bClosed`.
    closed: bool,
    /// `m_sLastEndPoint`.
    last_end_point: Point3,
    /// `m_pLastEndPolyVertex`.
    last_end_vertex: Option<PolyVertexId>,
}

/// A polygon face (`SmPolyFace`), or a composite face (`SmCPolyFace`) when
/// `composite` is set.
#[derive(Debug, Clone, PartialEq)]
pub struct PolyFace {
    /// `m_pListOwner` (the shell; `None` for a composite face, owned by the B-rep's
    /// composite face list).
    shell: Option<PolyShellId>,
    /// `m_pList` loops (first the outer loop).
    loops: Vec<PolyLoopId>,
    /// `m_pCPolyFace`.
    cpoly_face: Option<PolyFaceId>,
    /// `m_pOriginalFace`.
    original_face: Option<ObjectKey>,
    /// `m_sZoneTol3d`.
    tolerance: f64,
    /// `m_vNormal`.
    normal: Vec3,
    /// `m_dMaxDeviation`.
    max_deviation: f64,
    /// `m_vSphereCenter`.
    sphere_center: Point3,
    /// `m_dSphereRadius`.
    sphere_radius: f64,
    /// `m_lIndexValue`.
    index_value: u64,
    /// `SmCPolyFace::m_vPolyFaces`.
    composite: Option<Vec<PolyFaceId>>,
}

/// A polygon shell (`SmPolyShell`).
#[derive(Debug, Clone, PartialEq)]
pub struct PolyShell {
    /// `m_pListOwner`.
    region: Option<PolyRegionId>,
    /// `m_pList` faces.
    faces: Vec<PolyFaceId>,
}

/// A polygon region (`SmPolyRegion`).
#[derive(Debug, Clone, PartialEq)]
pub struct PolyRegion {
    /// Owned by the poly B-rep's region list.
    listed: bool,
    /// `m_pList` shells (first the outer shell).
    shells: Vec<PolyShellId>,
}

/// The objects a poly B-rep notification reaches: topology records, attributes,
/// caches, the context and the poly B-rep's own reference (`this`).
#[derive(Debug)]
pub struct PolyNotifyEnv<'a, R: ?Sized, T> {
    /// Topology records of the notified objects.
    pub records: &'a mut R,
    /// The attribute store.
    pub attributes: &'a mut AttributeStore,
    /// The cache store.
    pub caches: &'a mut CacheStore<T>,
    /// The context (`None` for a null context).
    pub context: Option<&'a mut Context>,
    /// This poly B-rep as a notified object.
    pub this: ObjectRef,
}

/// The real source face and notification state used by
/// [`PolyBrep::face_set_original_face`].
///
/// `notify.key_of(EntityRef::Face(source_face))` must identify the actual source
/// face and equal the setter's original key. The caller must give `destination`
/// the identity of the actual destination poly face (not its owning poly B-rep),
/// distinct from the source, and keep both objects' topology records and attribute
/// lists under those same keys. A null `notify.context` means no context callbacks,
/// not missing source/attribute state.
#[derive(Debug)]
pub struct PolyFaceCopyEnv<'a> {
    /// The B-rep containing the original face.
    pub source_brep: &'a mut Brep,
    /// The actual original face in `source_brep`.
    pub source_face: FaceId,
    /// Existing face notification services and consistent source/destination records.
    pub notify: crate::topology::edge::NotifyEnv<'a>,
    /// The actual destination poly face, with its concrete face/composite-face type.
    pub destination: ObjectRef,
}

/// Outputs of [`PolyBrep::calculate_bounding_box`].
#[derive(Debug, Clone, PartialEq)]
pub struct PolyBrepBoxes {
    /// `rNormalBox`.
    pub normal_box: Extent3d,
    /// `*pOptPseudoBox` when asked for.
    pub pseudo_box: Option<PseudoBox>,
    /// `*pOptCentroid` when asked for.
    pub centroid: Option<Point3>,
}

/// The poly classes, for the run-time type queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyObjectKind {
    /// `SmPolyBrep`.
    Brep,
    /// `SmPolyRegion`.
    Region,
    /// `SmPolyShell`.
    Shell,
    /// `SmCPolyFace`.
    CPolyFace,
    /// `SmPolyFace`.
    Face,
    /// `SmPolyLoop`.
    Loop,
    /// `SmPolyEdge`.
    Edge,
    /// `SmPolyVertex`.
    Vertex,
}

/// `SmOwningTopology_TYPE` (`TOPO_BASE_TYPE + 10`).
const OWNING_TOPOLOGY_TYPE: i64 = 16010;

/// `SmOwningTopology::IsKindOf` (CO-PORTED from `topology.proto_topology_p1`).
fn owning_topology_is_kind_of(t: i64) -> bool {
    t == OWNING_TOPOLOGY_TYPE || crate::topology::topology::is_kind_of(t)
}

impl PolyObjectKind {
    /// `GetType` (`POLY_BASE_TYPE + n`).
    #[must_use]
    pub const fn type_code(self) -> i64 {
        match self {
            Self::Brep => 18001,
            Self::Region => 18002,
            Self::Shell => 18003,
            Self::CPolyFace => 18004,
            Self::Face => 18005,
            Self::Loop => 18006,
            Self::Edge => 18007,
            Self::Vertex => 18008,
        }
    }

    /// `GetTypeString`.
    #[must_use]
    pub const fn type_string(self) -> &'static str {
        match self {
            Self::Brep => "SmPolyBrep_TYPE",
            Self::Region => "SmPolyRegion_TYPE",
            Self::Shell => "SmPolyShell_TYPE",
            Self::CPolyFace => "SmCPolyFace_TYPE",
            Self::Face => "SmPolyFace_TYPE",
            Self::Loop => "SmPolyLoop_TYPE",
            Self::Edge => "SmPolyEdge_TYPE",
            Self::Vertex => "SmPolyVertex_TYPE",
        }
    }

    /// `GetClassString`.
    #[must_use]
    pub const fn class_string(self) -> &'static str {
        match self {
            Self::Brep => "SmPolyBrep",
            Self::Region => "SmPolyRegion",
            Self::Shell => "SmPolyShell",
            Self::CPolyFace => "SmCPolyFace",
            Self::Face => "SmPolyFace",
            Self::Loop => "SmPolyLoop",
            Self::Edge => "SmPolyEdge",
            Self::Vertex => "SmPolyVertex",
        }
    }

    /// `IsKindOf(t)`: the class's own type, else its base class's answer.
    #[must_use]
    pub fn is_kind_of(self, t: i64) -> bool {
        if t == self.type_code() {
            return true;
        }
        match self {
            // SmPolyBrep : SmSAGObject
            Self::Brep => a_object::sag_is_kind_of(t),
            // SmCPolyFace : SmPolyFace
            Self::CPolyFace => Self::Face.is_kind_of(t),
            // SmPolyRegion, SmPolyShell, SmPolyFace, SmPolyLoop : SmOwningTopology
            Self::Region | Self::Shell | Self::Face | Self::Loop => owning_topology_is_kind_of(t),
            // SmPolyEdge, SmPolyVertex : SmTopology
            Self::Edge | Self::Vertex => crate::topology::topology::is_kind_of(t),
        }
    }
}

/// The display list id of a graphics display list (`SmDisplayList`; only the id is
/// modelled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayList {
    /// `m_lDisplayListId`.
    id: u32,
}

impl DisplayList {
    /// A display list with id `id`.
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self { id }
    }

    /// `GetDisplayListId`.
    #[must_use]
    pub const fn display_list_id(&self) -> u32 {
        self.id
    }

    /// `SetDisplayListId`: stores `id` and returns the previous one.
    pub fn set_display_list_id(&mut self, id: u32) -> u32 {
        let old = self.id;
        self.id = id;
        old
    }
}

/// `sm_IsUniformPositiveScale`: all three factors the same (`SM_ARE_SAME`) and above
/// `SM_EFF_ZERO`.
#[must_use]
pub fn is_uniform_positive_scale(scale: Vec3) -> bool {
    tol::are_same(scale.x, scale.y) && tol::are_same(scale.x, scale.z) && scale.x > EFF_ZERO
}

/// `sm_ReportInvalidTransformInput`: `SM_ERR_INVALID_INPUT` carrying `message`.
#[must_use]
pub const fn report_invalid_transform_input(message: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: "query::poly::report_invalid_transform_input",
        reason: message,
    }
}

const fn dead(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "poly object id is unknown or deleted",
    }
}

const fn nested_composite(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "a composite poly face cannot be a member of a composite poly face",
    }
}

fn slot<'a, T>(v: &'a [Option<T>], i: u32, op: &'static str) -> Result<&'a T> {
    v.get(i as usize).and_then(Option::as_ref).ok_or(dead(op))
}

fn slot_mut<'a, T>(v: &'a mut [Option<T>], i: u32, op: &'static str) -> Result<&'a mut T> {
    v.get_mut(i as usize)
        .and_then(Option::as_mut)
        .ok_or(dead(op))
}

fn push_slot<T>(v: &mut Vec<Option<T>>, item: T, op: &'static str) -> Result<u32> {
    let i = u32::try_from(v.len()).map_err(|_| SolidError::InvalidInput {
        operation: op,
        reason: "too many poly objects",
    })?;
    v.push(Some(item));
    Ok(i)
}

/// `POLY_QUADRANT(VPoint, TestPoint)`.
fn poly_quadrant(v: Point3, t: Point3) -> i64 {
    if v.x > t.x {
        if v.y > t.y {
            0
        } else {
            3
        }
    } else if v.y > t.y {
        1
    } else {
        2
    }
}

/// `POLY_X_INTERCEPT(pt1, pt2, yy)`.
fn poly_x_intercept(pt1: Point3, pt2: Point3, yy: f64) -> f64 {
    pt2.x - ((pt2.y - yy) * ((pt1.x - pt2.x) / (pt1.y - pt2.y)))
}

/// `POLY_ADJUST_DELTA(delta, vpoint, nextvpoint, testpoint)`.
fn poly_adjust_delta(delta: i64, v: Point3, next: Point3, t: Point3) -> i64 {
    match delta {
        3 => -1,
        -3 => 1,
        2 | -2 => {
            if poly_x_intercept(v, next, t.y) > t.x {
                -delta
            } else {
                delta
            }
        }
        _ => delta,
    }
}

/// `sm_TransformPolyNormal`: maps a surface normal through the optional diagonal scale
/// (by its cofactor, so winding is kept) and the rigid motion, then unitizes it.
fn transform_poly_normal(
    rotate_n_move: &Axis2Placement,
    scale: Option<Vec3>,
    normal: &mut Vec3,
) -> Result<()> {
    if let Some(s) = scale {
        // Polygon winding is unchanged by Transform, so normals follow the cofactor of
        // the scale (bounded common multiple of it for a nonsingular scale).
        let zero_scales = u32::from(s.x == 0.0) + u32::from(s.y == 0.0) + u32::from(s.z == 0.0);
        if zero_scales == 0 {
            let min_abs_scale = s.min_dimension();
            let parity = if (s.x < 0.0) ^ (s.y < 0.0) ^ (s.z < 0.0) {
                -1.0
            } else {
                1.0
            };
            *normal = Vec3::new(
                normal.x * (parity * (min_abs_scale / s.x)),
                normal.y * (parity * (min_abs_scale / s.y)),
                normal.z * (parity * (min_abs_scale / s.z)),
            );
        } else if zero_scales == 1 {
            // The cofactor of a rank-two diagonal scale has one surviving axis.
            if s.x == 0.0 {
                let sign = if (s.y < 0.0) ^ (s.z < 0.0) { -1.0 } else { 1.0 };
                *normal = Vec3::new(sign * normal.x, 0.0, 0.0);
            } else if s.y == 0.0 {
                let sign = if (s.x < 0.0) ^ (s.z < 0.0) { -1.0 } else { 1.0 };
                *normal = Vec3::new(0.0, sign * normal.y, 0.0);
            } else {
                let sign = if (s.x < 0.0) ^ (s.y < 0.0) { -1.0 } else { 1.0 };
                *normal = Vec3::new(0.0, 0.0, sign * normal.z);
            }
        } else {
            // Rank-one and point collapses have no transformed surface normal.
            *normal = Vec3::ZERO;
        }
    }

    let normal_scale = normal.max_dimension();
    if normal_scale > 0.0 {
        // Rescale before Unitize so very large model scales cannot underflow the
        // squared length of an otherwise valid unit normal.
        *normal /= normal_scale;
    }

    *normal = rotate_n_move.transform_vector(*normal);
    if normal_scale > 0.0 {
        let r = normal.unitize();
        *normal = normal.unitize_or_z();
        r?;
    }
    Ok(())
}

/// A polygonal B-rep (`SmPolyBrep`) and every poly object it owns.
#[derive(Debug, Clone, PartialEq)]
pub struct PolyBrep {
    /// `m_sZoneTol3d`.
    tolerance: f64,
    /// `m_bOKBackPtrs`.
    ok_back_ptrs: bool,
    /// `m_bOwnsContext`.
    owns_context: bool,
    /// The `SmAObject` cache slot.
    pub aobject: AObject,
    /// `m_pList` regions.
    regions: Vec<PolyRegionId>,
    /// `m_pPolyVertexListHead` list.
    vertex_list: Vec<PolyVertexId>,
    /// `m_pCPolyFaceListHead` list.
    cpoly_list: Vec<PolyFaceId>,
    region_slots: Vec<Option<PolyRegion>>,
    shell_slots: Vec<Option<PolyShell>>,
    face_slots: Vec<Option<PolyFace>>,
    loop_slots: Vec<Option<PolyLoop>>,
    edge_slots: Vec<Option<PolyEdge>>,
    vertex_slots: Vec<Option<PolyVertex>>,
    aux_slots: Vec<Option<PolyVertAuxData>>,
}

/// Resolves an original vertex's key to its id in the original B-rep (see
/// [`PolyBrep::destroy_with_original`]).
pub type VertexOfKey<'a> = &'a dyn Fn(ObjectKey) -> Option<VertexId>;

impl PolyBrep {
    // ---- construction (mirrors of upstream constructors outside this unit) ---------

    /// `SmPolyBrep(SmZoneTol3d, ..)`: an empty poly B-rep with zone tolerance
    /// `SmZoneTol3d(zone_tol)`, not owning its context, back pointers not valid.
    #[must_use]
    pub fn new(k: &Kernel, zone_tol: f64) -> Self {
        Self {
            tolerance: tol::zone_tol3d(k, zone_tol),
            ok_back_ptrs: false,
            owns_context: false,
            aobject: AObject::new(),
            regions: Vec::new(),
            vertex_list: Vec::new(),
            cpoly_list: Vec::new(),
            region_slots: Vec::new(),
            shell_slots: Vec::new(),
            face_slots: Vec::new(),
            loop_slots: Vec::new(),
            edge_slots: Vec::new(),
            vertex_slots: Vec::new(),
            aux_slots: Vec::new(),
        }
    }

    /// `SmPolyRegion(pPolyBrep)`: a new empty region posted on the region list.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the arena is full.
    pub fn add_region(&mut self) -> Result<PolyRegionId> {
        let id = PolyRegionId(push_slot(
            &mut self.region_slots,
            PolyRegion {
                listed: true,
                shells: Vec::new(),
            },
            "query::poly::PolyBrep::add_region",
        )?);
        self.regions.push(id);
        Ok(id)
    }

    /// `SmPolyShell(pPolyRegion)`: a new empty shell posted on `region`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown region.
    pub fn add_shell(&mut self, region: PolyRegionId) -> Result<PolyShellId> {
        const OP: &str = "query::poly::PolyBrep::add_shell";
        slot(&self.region_slots, region.0, OP)?;
        let id = PolyShellId(push_slot(
            &mut self.shell_slots,
            PolyShell {
                region: Some(region),
                faces: Vec::new(),
            },
            OP,
        )?);
        slot_mut(&mut self.region_slots, region.0, OP)?
            .shells
            .push(id);
        Ok(id)
    }

    /// `SmPolyFace(SmZoneTol3d(zone_tol), NULL, NULL, pShell, pOptNormal)`: a new face
    /// without loops posted on `shell`. A given normal is unitized; a zero one leaves
    /// the normal uninitialised.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown shell.
    pub fn add_face(
        &mut self,
        k: &Kernel,
        shell: PolyShellId,
        zone_tol: f64,
        normal: Option<Vec3>,
    ) -> Result<PolyFaceId> {
        const OP: &str = "query::poly::PolyBrep::add_face";
        slot(&self.shell_slots, shell.0, OP)?;
        let mut face = Self::blank_face(tol::zone_tol3d(k, zone_tol));
        face.shell = Some(shell);
        if let Some(n) = normal {
            face.normal = n.unitize().unwrap_or(Vec3::UNDEF);
        }
        let id = PolyFaceId(push_slot(&mut self.face_slots, face, OP)?);
        slot_mut(&mut self.shell_slots, shell.0, OP)?.faces.push(id);
        Ok(id)
    }

    fn blank_face(tolerance: f64) -> PolyFace {
        PolyFace {
            shell: None,
            loops: Vec::new(),
            cpoly_face: None,
            original_face: None,
            tolerance,
            // default-constructed SmVector3d members are SM_UNDEF_DOUBLE
            normal: Vec3::UNDEF,
            max_deviation: -BIG_DOUBLE,
            sphere_center: Vec3::UNDEF,
            sphere_radius: 0.0,
            index_value: UNDEF_ULONG,
            composite: None,
        }
    }

    /// `SmPolyVertex(pt, SmZoneTol3d(zone_tol))`, posted on the poly B-rep's vertex
    /// list.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the arena is full.
    pub fn add_vertex(&mut self, k: &Kernel, point: Point3, zone_tol: f64) -> Result<PolyVertexId> {
        let id = PolyVertexId(push_slot(
            &mut self.vertex_slots,
            PolyVertex {
                tolerance: tol::zone_tol3d(k, zone_tol),
                point,
                edge_list: None,
                index_value: UNDEF_ULONG,
                original_vertex: None,
                normals: Vec::new(),
                listed: true,
            },
            "query::poly::PolyBrep::add_vertex",
        )?);
        self.vertex_list.push(id);
        Ok(id)
    }

    /// `SmPolyFace::StartPolyEdgeLoop`: a new empty loop posted on `face`; an outer
    /// loop when it is the face's first loop, else an inner one.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown face.
    pub fn start_poly_edge_loop(&mut self, face: PolyFaceId) -> Result<PolyLoopId> {
        const OP: &str = "query::poly::PolyBrep::start_poly_edge_loop";
        slot(&self.face_slots, face.0, OP)?;
        let id = PolyLoopId(push_slot(
            &mut self.loop_slots,
            PolyLoop {
                face: Some(face),
                head: None,
                size: 0,
                orientation: PolyLoopOrientation::Same,
                closed: false,
                last_end_point: Vec3::ZERO,
                last_end_vertex: None,
            },
            OP,
        )?);
        let f = slot_mut(&mut self.face_slots, face.0, OP)?;
        f.loops.push(id);
        if f.loops.len() > 1 {
            slot_mut(&mut self.loop_slots, id.0, OP)?.orientation = PolyLoopOrientation::Opposite;
        }
        Ok(id)
    }

    /// `SmPolyVertex::AddPolyEdge`: makes `vertex` the start of `edge` and puts the
    /// edge at the front of the vertex's edge ring.
    fn vertex_add_poly_edge(&mut self, vertex: PolyVertexId, edge: PolyEdgeId) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::vertex_add_poly_edge";
        slot_mut(&mut self.edge_slots, edge.0, OP)?.vertex = Some(vertex);
        let head = slot(&self.vertex_slots, vertex.0, OP)?.edge_list;
        match head {
            None => {
                let e = slot_mut(&mut self.edge_slots, edge.0, OP)?;
                e.next_at_v = Some(edge);
                e.prev_at_v = Some(edge);
            }
            Some(h) => {
                let h_prev = slot(&self.edge_slots, h.0, OP)?.prev_at_v.ok_or(dead(OP))?;
                {
                    let e = slot_mut(&mut self.edge_slots, edge.0, OP)?;
                    e.next_at_v = Some(h);
                    e.prev_at_v = Some(h_prev);
                }
                slot_mut(&mut self.edge_slots, h_prev.0, OP)?.next_at_v = Some(edge);
                slot_mut(&mut self.edge_slots, h.0, OP)?.prev_at_v = Some(edge);
            }
        }
        slot_mut(&mut self.vertex_slots, vertex.0, OP)?.edge_list = Some(edge);
        Ok(())
    }

    /// Appends `edge` at the end of `lp`'s circular edge list
    /// (`SmOwningTopology::PostInsert`).
    fn loop_post_insert(&mut self, lp: PolyLoopId, edge: PolyEdgeId) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::loop_post_insert";
        let head = slot(&self.loop_slots, lp.0, OP)?.head;
        match head {
            None => {
                let e = slot_mut(&mut self.edge_slots, edge.0, OP)?;
                e.next = Some(edge);
                e.last = Some(edge);
            }
            Some(h) => {
                let tail = slot(&self.edge_slots, h.0, OP)?.last.ok_or(dead(OP))?;
                {
                    let e = slot_mut(&mut self.edge_slots, edge.0, OP)?;
                    e.next = Some(h);
                    e.last = Some(tail);
                }
                slot_mut(&mut self.edge_slots, tail.0, OP)?.next = Some(edge);
                slot_mut(&mut self.edge_slots, h.0, OP)?.last = Some(edge);
            }
        }
        slot_mut(&mut self.edge_slots, edge.0, OP)?.owner = Some(lp);
        let l = slot_mut(&mut self.loop_slots, lp.0, OP)?;
        if l.head.is_none() {
            l.head = Some(edge);
        }
        l.size += 1;
        Ok(())
    }

    /// Removes `edge` from its loop's edge list (`SmOwningTopology::Remove`).
    fn loop_remove(&mut self, edge: PolyEdgeId) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::loop_remove";
        let (owner, next, last) = {
            let e = slot(&self.edge_slots, edge.0, OP)?;
            (e.owner, e.next, e.last)
        };
        let (Some(lp), Some(next), Some(last)) = (owner, next, last) else {
            return Ok(());
        };
        if next == edge {
            let l = slot_mut(&mut self.loop_slots, lp.0, OP)?;
            l.size = l.size.saturating_sub(1);
            l.head = None;
        } else {
            let l = slot_mut(&mut self.loop_slots, lp.0, OP)?;
            if l.head == Some(edge) {
                l.head = Some(next);
            }
            l.size = l.size.saturating_sub(1);
            slot_mut(&mut self.edge_slots, next.0, OP)?.last = Some(last);
            slot_mut(&mut self.edge_slots, last.0, OP)?.next = Some(next);
        }
        let e = slot_mut(&mut self.edge_slots, edge.0, OP)?;
        e.owner = None;
        e.next = None;
        e.last = None;
        Ok(())
    }

    /// `SmPolyLoop::AddPolyEdge(SmZoneTol3d(zone_tol), start, end, start_vertex, NULL,
    /// pPolyBrep, ..)`: a new lamina edge appended to `lp`, starting at the stored last
    /// end vertex or `start_vertex`. No end vertex is given, so nothing is stitched.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the gap from the previous edge's end point exceeds
    /// `100 * zone_tol` (the edge stays in the loop, as upstream);
    /// [`SolidError::InvalidInput`] for unknown ids.
    pub fn add_poly_edge(
        &mut self,
        k: &Kernel,
        lp: PolyLoopId,
        zone_tol: f64,
        start: Point3,
        end: Point3,
        start_vertex: PolyVertexId,
    ) -> Result<PolyEdgeId> {
        const OP: &str = "query::poly::PolyBrep::add_poly_edge";
        let zt = tol::zone_tol3d(k, zone_tol);
        slot(&self.loop_slots, lp.0, OP)?;
        slot(&self.vertex_slots, start_vertex.0, OP)?;
        let id = PolyEdgeId(push_slot(&mut self.edge_slots, Self::blank_edge(zt), OP)?);
        {
            let e = slot_mut(&mut self.edge_slots, id.0, OP)?;
            e.next_radial = id;
            e.last_radial = id;
        }
        self.loop_post_insert(lp, id)?;

        let first = slot(&self.edge_slots, id.0, OP)?.last == Some(id);
        if first {
            if !slot(&self.vertex_slots, start_vertex.0, OP)?.listed {
                self.vertex_list.push(start_vertex);
                slot_mut(&mut self.vertex_slots, start_vertex.0, OP)?.listed = true;
            }
            self.vertex_add_poly_edge(start_vertex, id)?;
            slot_mut(&mut self.loop_slots, lp.0, OP)?.last_end_vertex = None;
        } else {
            let l = slot(&self.loop_slots, lp.0, OP)?;
            if l.last_end_point.distance(start) > zt * 100.0 {
                return Err(SolidError::sm_err(OP));
            }
            let v = l.last_end_vertex.unwrap_or(start_vertex);
            if !slot(&self.vertex_slots, v.0, OP)?.listed {
                self.vertex_list.push(v);
                slot_mut(&mut self.vertex_slots, v.0, OP)?.listed = true;
            }
            self.vertex_add_poly_edge(v, id)?;
            slot_mut(&mut self.loop_slots, lp.0, OP)?.last_end_vertex = None;
        }

        let l = slot_mut(&mut self.loop_slots, lp.0, OP)?;
        l.last_end_point = end;
        l.last_end_vertex = None;
        Ok(id)
    }

    fn blank_edge(tolerance: f64) -> PolyEdge {
        PolyEdge {
            owner: None,
            next: None,
            last: None,
            next_radial: PolyEdgeId(0),
            last_radial: PolyEdgeId(0),
            tolerance,
            next_at_v: None,
            prev_at_v: None,
            vertex: None,
            aux: None,
            original_edgeuse: None,
            index_value: UNDEF_ULONG,
        }
    }

    /// `SmPolyLoop::FinishPolyEdgeLoop`: closes the loop.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a loop without edges, or when the first start point is farther
    /// than its vertex tolerance from the last end point (then the loop stays open).
    pub fn finish_poly_edge_loop(&mut self, lp: PolyLoopId) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::finish_poly_edge_loop";
        let l = slot(&self.loop_slots, lp.0, OP)?;
        let first = l.head.ok_or(SolidError::sm_err(OP))?;
        let last_end = l.last_end_point;
        let v = self.edge_vertex(first, OP)?;
        let vx = slot(&self.vertex_slots, v.0, OP)?;
        if vx.point.distance(last_end) > vx.tolerance {
            return Err(SolidError::sm_err(OP));
        }
        let l = slot_mut(&mut self.loop_slots, lp.0, OP)?;
        l.closed = true;
        l.last_end_vertex = None;
        l.last_end_point = Vec3::ZERO;
        Ok(())
    }

    /// `SmPolyLoop::SetLastEndPolyVertex`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown loop.
    pub fn set_last_end_poly_vertex(
        &mut self,
        lp: PolyLoopId,
        vertex: Option<PolyVertexId>,
    ) -> Result<()> {
        slot_mut(
            &mut self.loop_slots,
            lp.0,
            "query::poly::PolyBrep::set_last_end_poly_vertex",
        )?
        .last_end_vertex = vertex;
        Ok(())
    }

    /// Links `edges` into one radial ring in the given order (`m_pNextRadialE` of each
    /// is the next one, cyclically), as `StitchEdgeTopological` leaves them.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown edge.
    pub fn link_radials(&mut self, edges: &[PolyEdgeId]) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::link_radials";
        let n = edges.len();
        for (i, &e) in edges.iter().enumerate() {
            let next = edges.get((i + 1) % n).copied().ok_or(dead(OP))?;
            let last = edges.get((i + n - 1) % n).copied().ok_or(dead(OP))?;
            let r = slot_mut(&mut self.edge_slots, e.0, OP)?;
            r.next_radial = next;
            r.last_radial = last;
        }
        Ok(())
    }

    /// `new SmPolyVertAuxData()` with the given normal and UV (no users yet).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the arena is full.
    pub fn add_aux_data(&mut self, normal: Vec3, uv: Vec2) -> Result<PolyAuxId> {
        let mut a = PolyVertAuxData::new();
        a.normal = normal;
        a.uv = uv;
        Ok(PolyAuxId(push_slot(
            &mut self.aux_slots,
            a,
            "query::poly::PolyBrep::add_aux_data",
        )?))
    }

    /// `SmPolyEdge::SetAuxData`: adds a user to the new data and removes one from the
    /// old data (deleting it when it has no users left).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown ids, without modifying the B-rep.
    pub fn edge_set_aux_data(&mut self, edge: PolyEdgeId, aux: Option<PolyAuxId>) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::edge_set_aux_data";
        let old = slot(&self.edge_slots, edge.0, OP)?.aux;
        if let Some(a) = aux {
            slot(&self.aux_slots, a.0, OP)?;
        }
        if old == aux {
            return Ok(());
        }
        slot_mut(&mut self.edge_slots, edge.0, OP)?.aux = aux;
        if let Some(a) = aux {
            let d = slot_mut(&mut self.aux_slots, a.0, OP)?;
            d.num_users = d.num_users.wrapping_add(1);
        }
        if let Some(o) = old {
            self.aux_remove_user(o)?;
        }
        Ok(())
    }

    /// `SmPolyVertex::AddFaceNormal`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown vertex.
    pub fn vertex_add_face_normal(&mut self, vertex: PolyVertexId, normal: Vec3) -> Result<()> {
        slot_mut(
            &mut self.vertex_slots,
            vertex.0,
            "query::poly::PolyBrep::vertex_add_face_normal",
        )?
        .normals
        .push(normal);
        Ok(())
    }

    /// `SmCPolyFace(pPolyFace)`: a composite face copying `face`'s back pointer,
    /// tolerance, normal and deviation, with `face` as its first member, posted on the
    /// composite face list.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown face or a composite `face` (nested
    /// composites would make the composite queries recurse without bound).
    pub fn add_cpoly_face(&mut self, face: PolyFaceId) -> Result<PolyFaceId> {
        const OP: &str = "query::poly::PolyBrep::add_cpoly_face";
        let f = slot(&self.face_slots, face.0, OP)?;
        if f.composite.is_some() {
            return Err(nested_composite(OP));
        }
        let mut c = Self::blank_face(f.tolerance);
        c.original_face = f.original_face;
        c.normal = f.normal;
        c.max_deviation = f.max_deviation;
        c.composite = Some(Vec::new());
        let id = PolyFaceId(push_slot(&mut self.face_slots, c, OP)?);
        slot_mut(&mut self.face_slots, id.0, OP)?.cpoly_face = Some(id);
        self.cpoly_add_poly_face(id, face, false)?;
        self.cpoly_list.push(id);
        Ok(id)
    }

    /// `SmCPolyFace::AddPolyFace`: adds `face` (once, unless `allow_duplicates`) and
    /// points its composite back pointer here.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown ids, a `cpoly` that is not composite or
    /// a composite `face` (nested composites would make the composite queries recurse
    /// without bound).
    pub fn cpoly_add_poly_face(
        &mut self,
        cpoly: PolyFaceId,
        face: PolyFaceId,
        allow_duplicates: bool,
    ) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::cpoly_add_poly_face";
        if slot(&self.face_slots, face.0, OP)?.composite.is_some() {
            return Err(nested_composite(OP));
        }
        let members = slot_mut(&mut self.face_slots, cpoly.0, OP)?
            .composite
            .as_mut()
            .ok_or(dead(OP))?;
        if allow_duplicates || !members.contains(&face) {
            members.push(face);
        }
        slot_mut(&mut self.face_slots, face.0, OP)?.cpoly_face = Some(cpoly);
        Ok(())
    }

    /// `SmPolyEdge::SetOriginalEdgeuse(pEdgeuse, FALSE)`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown edge.
    pub fn edge_set_original_edgeuse(
        &mut self,
        edge: PolyEdgeId,
        edgeuse: Option<ObjectKey>,
    ) -> Result<()> {
        slot_mut(
            &mut self.edge_slots,
            edge.0,
            "query::poly::PolyBrep::edge_set_original_edgeuse",
        )?
        .original_edgeuse = edgeuse;
        Ok(())
    }

    /// `SmPolyVertex::SetOriginalVertex(pVertex, FALSE)`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown vertex.
    pub fn vertex_set_original_vertex(
        &mut self,
        vertex: PolyVertexId,
        original: Option<ObjectKey>,
    ) -> Result<()> {
        slot_mut(
            &mut self.vertex_slots,
            vertex.0,
            "query::poly::PolyBrep::vertex_set_original_vertex",
        )?
        .original_vertex = original;
        Ok(())
    }

    /// Sets `m_bOKBackPtrs` (tessellation keeps the back pointers valid).
    pub fn set_ok_back_ptrs(&mut self, ok: bool) {
        self.ok_back_ptrs = ok;
    }

    // ---- accessors ------------------------------------------------------------------

    fn vertex(&self, v: PolyVertexId) -> Result<&PolyVertex> {
        slot(&self.vertex_slots, v.0, "query::poly::PolyBrep::vertex")
    }

    fn edge(&self, e: PolyEdgeId) -> Result<&PolyEdge> {
        slot(&self.edge_slots, e.0, "query::poly::PolyBrep::edge")
    }

    fn lp(&self, l: PolyLoopId) -> Result<&PolyLoop> {
        slot(&self.loop_slots, l.0, "query::poly::PolyBrep::loop")
    }

    fn face(&self, f: PolyFaceId) -> Result<&PolyFace> {
        slot(&self.face_slots, f.0, "query::poly::PolyBrep::face")
    }

    fn edge_vertex(&self, e: PolyEdgeId, op: &'static str) -> Result<PolyVertexId> {
        self.edge(e)?.vertex.ok_or(SolidError::null_pointer(op))
    }

    /// The poly vertex record `v`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn poly_vertex(&self, v: PolyVertexId) -> Result<&PolyVertex> {
        self.vertex(v)
    }

    /// The poly face record `f`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn poly_face(&self, f: PolyFaceId) -> Result<&PolyFace> {
        self.face(f)
    }

    /// The aux data record `a`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted aux data.
    pub fn aux_data(&self, a: PolyAuxId) -> Result<&PolyVertAuxData> {
        slot(&self.aux_slots, a.0, "query::poly::PolyBrep::aux_data")
    }

    /// Mutable aux data record `a` (its public normal and UV).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted aux data.
    pub fn aux_data_mut(&mut self, a: PolyAuxId) -> Result<&mut PolyVertAuxData> {
        slot_mut(
            &mut self.aux_slots,
            a.0,
            "query::poly::PolyBrep::aux_data_mut",
        )
    }

    /// True while slot `e` holds an edge.
    #[must_use]
    pub fn edge_is_live(&self, e: PolyEdgeId) -> bool {
        self.edge(e).is_ok()
    }

    /// True while slot `f` holds a face.
    #[must_use]
    pub fn face_is_live(&self, f: PolyFaceId) -> bool {
        self.face(f).is_ok()
    }

    /// Number of edge slots (live and deleted).
    #[must_use]
    pub fn edge_slot_count(&self) -> usize {
        self.edge_slots.len()
    }

    /// Number of face slots (live and deleted, plain and composite).
    #[must_use]
    pub fn face_slot_count(&self) -> usize {
        self.face_slots.len()
    }

    /// Number of loop slots (live and deleted).
    #[must_use]
    pub fn loop_slot_count(&self) -> usize {
        self.loop_slots.len()
    }

    /// `SmPolyBrep::GetTolerance`.
    #[must_use]
    pub const fn tolerance(&self) -> f64 {
        self.tolerance
    }

    /// `SmPolyBrep::GetOKBackPtrs`.
    #[must_use]
    pub const fn ok_back_ptrs(&self) -> bool {
        self.ok_back_ptrs
    }

    /// `m_bOwnsContext`.
    #[must_use]
    pub const fn owns_context(&self) -> bool {
        self.owns_context
    }

    // ---- aux data ---------------------------------------------------------------

    /// `SmPolyVertAuxData::RemoveUser`: one user less, then `DeleteIfNoUsers`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted aux data.
    pub fn aux_remove_user(&mut self, a: PolyAuxId) -> Result<Option<PolyAuxId>> {
        let d = slot_mut(
            &mut self.aux_slots,
            a.0,
            "query::poly::PolyBrep::aux_remove_user",
        )?;
        d.num_users = d.num_users.wrapping_sub(1);
        self.aux_delete_if_no_users(a)
    }

    /// `SmPolyVertAuxData::DeleteIfNoUsers`: deletes the data when nobody uses it;
    /// returns it when it survives.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted aux data.
    pub fn aux_delete_if_no_users(&mut self, a: PolyAuxId) -> Result<Option<PolyAuxId>> {
        let d = slot(
            &self.aux_slots,
            a.0,
            "query::poly::PolyBrep::aux_delete_if_no_users",
        )?;
        if d.num_users == 0 {
            if let Some(s) = self.aux_slots.get_mut(a.0 as usize) {
                *s = None;
            }
            return Ok(None);
        }
        Ok(Some(a))
    }

    // ---- vertices -------------------------------------------------------------------

    /// `SmPolyVertex::GetPoint`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_point(&self, v: PolyVertexId) -> Result<Point3> {
        Ok(self.vertex(v)?.point)
    }

    /// `SmPolyVertex::GetTolerance`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_tolerance(&self, v: PolyVertexId) -> Result<f64> {
        Ok(self.vertex(v)?.tolerance)
    }

    /// `SmPolyVertex::GetOriginalVertex`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_original_vertex(&self, v: PolyVertexId) -> Result<Option<ObjectKey>> {
        Ok(self.vertex(v)?.original_vertex)
    }

    /// `SmPolyVertex::GetNormalsRef` (read).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_normals(&self, v: PolyVertexId) -> Result<&[Vec3]> {
        Ok(&self.vertex(v)?.normals)
    }

    /// `SmPolyVertex::GetNormalsRef` (write).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_normals_mut(&mut self, v: PolyVertexId) -> Result<&mut Vec<Vec3>> {
        Ok(&mut slot_mut(
            &mut self.vertex_slots,
            v.0,
            "query::poly::PolyBrep::vertex_normals_mut",
        )?
        .normals)
    }

    /// `SmPolyVertex::Is2dOrZero`: the point's z is `NL_NOZ` or `0.0`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_is_2d_or_zero(&self, v: PolyVertexId) -> Result<bool> {
        let p = self.vertex(v)?.point;
        Ok(p.is_2d() || p.z == 0.0)
    }

    /// `SmPolyVertex::SetTolerance(zone_tol, update_only_if_larger)`: stores the
    /// tolerance. Upstream signals a tolerance more than 1000 times the current one but
    /// stores it anyway (`#define cbi` just above the function); the second argument is
    /// not used upstream.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_set_tolerance(
        &mut self,
        v: PolyVertexId,
        zone_tol: f64,
        _update_only_if_larger: bool,
    ) -> Result<()> {
        let x = slot_mut(
            &mut self.vertex_slots,
            v.0,
            "query::poly::PolyBrep::vertex_set_tolerance",
        )?;
        // both branches store (cbi is defined); the large one also signals SE(SM_ERR)
        x.tolerance = zone_tol;
        Ok(())
    }

    /// `SmPolyVertex::SetPoint`: moving the point by more than
    /// `SM_EFF_ZERO * (1 + max |new|)` invalidates the normals of its faces.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex or a broken edge
    /// ring.
    pub fn vertex_set_point(&mut self, v: PolyVertexId, new_point: Point3) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::vertex_set_point";
        // This can invalidate face normals.
        let scaled_zero = EFF_ZERO * (1.0 + new_point.max_dimension());
        if self.vertex(v)?.point.distance(new_point) > scaled_zero {
            for f in self.vertex_poly_faces(v)?.into_iter().flatten() {
                // a deleted face (upstream writes into its dead arena memory) is skipped
                if let Ok(fx) = slot_mut(&mut self.face_slots, f.0, OP) {
                    fx.max_deviation = -BIG_DOUBLE;
                }
            }
        }
        slot_mut(&mut self.vertex_slots, v.0, OP)?.point = new_point;
        Ok(())
    }

    /// `SmPolyVertex::~SmPolyVertex` (with `~SmTopology`): takes the vertex off the
    /// vertex list and deletes it. Edges starting at it keep the dead id, as upstream
    /// keeps the dangling pointer.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn delete_vertex(&mut self, v: PolyVertexId) -> Result<()> {
        self.vertex(v)?;
        self.vertex_list.retain(|&x| x != v);
        if let Some(s) = self.vertex_slots.get_mut(v.0 as usize) {
            *s = None;
        }
        Ok(())
    }

    /// `SmPolyVertex::CalculateBoundingBox`: the point's box expanded by the vertex
    /// tolerance.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_bounding_box(&self, v: PolyVertexId) -> Result<Extent3d> {
        let x = self.vertex(v)?;
        let mut b = Extent3d::new();
        b.add_point(x.point);
        b.expand_absolute(x.tolerance);
        Ok(b)
    }

    /// `SmPolyVertex::GetPolyFaces`: the faces of the edges of
    /// [`PolyBrep::vertex_poly_edges`], each once (`None` for an edge without a face).
    ///
    /// # Errors
    ///
    /// Those of [`PolyBrep::vertex_poly_edges`].
    pub fn vertex_poly_faces(&self, v: PolyVertexId) -> Result<Vec<Option<PolyFaceId>>> {
        let mut faces: Vec<Option<PolyFaceId>> = Vec::new();
        for e in self.vertex_poly_edges(v)? {
            let f = self.edge_poly_face(e)?;
            if !faces.contains(&f) {
                faces.push(f);
            }
        }
        Ok(faces)
    }

    /// `SmPolyVertex::RemovePolyEdge`: takes `edge` out of the vertex's edge ring and
    /// clears its start vertex.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `edge` starts at another vertex or the ring is inconsistent;
    /// [`SolidError::InvalidInput`] for unknown ids.
    pub fn vertex_remove_poly_edge(&mut self, v: PolyVertexId, edge: PolyEdgeId) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::vertex_remove_poly_edge";
        let e = self.edge(edge)?;
        // no work - PolyEdge already removed
        let Some(start) = e.vertex else {
            return Ok(());
        };
        // error - bad back pointers
        if start != v {
            return Err(SolidError::sm_err(OP));
        }
        let (prev, next) = (e.prev_at_v, e.next_at_v);
        {
            let e = slot_mut(&mut self.edge_slots, edge.0, OP)?;
            e.prev_at_v = None;
            e.next_at_v = None;
        }
        let x = slot_mut(&mut self.vertex_slots, v.0, OP)?;
        // If it was the first element in the list then set it to the next edge
        if x.edge_list == Some(edge) {
            x.edge_list = next;
        }
        if prev == Some(edge) {
            if next != prev {
                return Err(SolidError::sm_err(OP));
            }
            // Only one element in the list - just remove it.
            x.edge_list = None;
        } else {
            let (Some(p), Some(n)) = (prev, next) else {
                return Err(dead(OP));
            };
            slot_mut(&mut self.edge_slots, n.0, OP)?.prev_at_v = Some(p);
            slot_mut(&mut self.edge_slots, p.0, OP)?.next_at_v = Some(n);
        }
        // clear edge->Vertex pointer
        slot_mut(&mut self.edge_slots, edge.0, OP)?.vertex = None;
        Ok(())
    }

    /// The ring of edges at a vertex, starting at `head`, following `next_at_v`.
    fn ring_at_v(&self, head: Option<PolyEdgeId>, op: &'static str) -> Result<Vec<PolyEdgeId>> {
        let mut out = Vec::new();
        let Some(h) = head else {
            return Ok(out);
        };
        let mut e = h;
        loop {
            out.push(e);
            if out.len() > self.edge_slots.len() {
                return Err(SolidError::sm_err(op));
            }
            e = self.edge(e)?.next_at_v.ok_or(dead(op))?;
            if e == h {
                break;
            }
        }
        Ok(out)
    }

    /// `SmPolyVertex::GetStartingPolyEdges`: the edge ring at the vertex.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown ids or a broken ring.
    pub fn vertex_starting_poly_edges(&self, v: PolyVertexId) -> Result<Vec<PolyEdgeId>> {
        self.ring_at_v(
            self.vertex(v)?.edge_list,
            "query::poly::PolyBrep::vertex_starting_poly_edges",
        )
    }

    /// `SmPolyVertex::GetPolyEdges`: for each edge starting here, the edge and (when
    /// different) its CW neighbour, which ends here.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown ids or a broken ring.
    pub fn vertex_poly_edges(&self, v: PolyVertexId) -> Result<Vec<PolyEdgeId>> {
        let mut out = Vec::new();
        for e in self.vertex_starting_poly_edges(v)? {
            out.push(e);
            let cw = self.edge_cw_poly_edge(e)?;
            if let Some(c) = cw {
                if c != e {
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    // ---- edges ----------------------------------------------------------------------

    /// `SmPolyEdge::GetOriginalEdgeuse`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_original_edgeuse(&self, e: PolyEdgeId) -> Result<Option<ObjectKey>> {
        Ok(self.edge(e)?.original_edgeuse)
    }

    /// `SmPolyEdge::GetTolerance`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_tolerance(&self, e: PolyEdgeId) -> Result<f64> {
        Ok(self.edge(e)?.tolerance)
    }

    /// `SmPolyEdge::SetTolerance` (`zone_tol` is an `SmZoneTol3d` value).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_set_tolerance(&mut self, e: PolyEdgeId, zone_tol: f64) -> Result<()> {
        slot_mut(
            &mut self.edge_slots,
            e.0,
            "query::poly::PolyBrep::edge_set_tolerance",
        )?
        .tolerance = zone_tol;
        Ok(())
    }

    /// `SmPolyEdge::GetAuxData`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_aux_data(&self, e: PolyEdgeId) -> Result<Option<PolyAuxId>> {
        Ok(self.edge(e)?.aux)
    }

    /// `SmPolyEdge::GetStartPolyVertex`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_start_poly_vertex(&self, e: PolyEdgeId) -> Result<Option<PolyVertexId>> {
        Ok(self.edge(e)?.vertex)
    }

    /// `SmPolyEdge::GetStartPoint`.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without a start vertex; [`SolidError::InvalidInput`] for
    /// unknown ids.
    pub fn edge_start_point(&self, e: PolyEdgeId) -> Result<Point3> {
        let v = self.edge_vertex(e, "query::poly::PolyBrep::edge_start_point")?;
        Ok(self.vertex(v)?.point)
    }

    /// `SmPolyEdge::GetCWPolyEdge`: the previous edge of the loop.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_cw_poly_edge(&self, e: PolyEdgeId) -> Result<Option<PolyEdgeId>> {
        Ok(self.edge(e)?.last)
    }

    /// `SmPolyEdge::GetCCWPolyEdge`: the next edge of the loop.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_ccw_poly_edge(&self, e: PolyEdgeId) -> Result<Option<PolyEdgeId>> {
        Ok(self.edge(e)?.next)
    }

    /// `SmPolyEdge::GetRadial`: the next radial edge, `None` for a lamina edge.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_radial(&self, e: PolyEdgeId) -> Result<Option<PolyEdgeId>> {
        let n = self.edge(e)?.next_radial;
        Ok(if n == e { None } else { Some(n) })
    }

    /// `SmPolyEdge::GetAllRadials`: the radial ring starting with `e`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown ids or a broken ring.
    pub fn edge_all_radials(&self, e: PolyEdgeId) -> Result<Vec<PolyEdgeId>> {
        const OP: &str = "query::poly::PolyBrep::edge_all_radials";
        let mut out = Vec::new();
        let mut cur = e;
        loop {
            out.push(cur);
            if out.len() > self.edge_slots.len() {
                return Err(SolidError::sm_err(OP));
            }
            match self.edge_radial(cur)? {
                None => break,
                Some(n) if n == e => break,
                Some(n) => cur = n,
            }
        }
        Ok(out)
    }

    /// `SmPolyEdge::GetPolyLoop`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_poly_loop(&self, e: PolyEdgeId) -> Result<Option<PolyLoopId>> {
        Ok(self.edge(e)?.owner)
    }

    /// `SmPolyEdge::GetPolyFace`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown or deleted ids.
    pub fn edge_poly_face(&self, e: PolyEdgeId) -> Result<Option<PolyFaceId>> {
        match self.edge(e)?.owner {
            Some(l) => Ok(self.lp(l)?.face),
            None => Ok(None),
        }
    }

    /// `SmPolyEdge::GetOKBackPtrs`: the poly B-rep's flag for an edge in a loop.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown or deleted ids.
    pub fn edge_ok_back_ptrs(&self, e: PolyEdgeId) -> Result<bool> {
        // GetPolyBrep: loop -> face -> shell; a deleted face is in no shell
        let in_shell = match self.edge_poly_face(e)? {
            Some(f) => self
                .face_slots
                .get(f.0 as usize)
                .and_then(Option::as_ref)
                .is_some_and(|x| x.shell.is_some()),
            None => false,
        };
        Ok(in_shell && self.ok_back_ptrs)
    }

    /// `SmPolyEdge::GetEndPolyVertex`: the next edge's start vertex; for the last edge
    /// of its loop, the loop's first start vertex when the loop is closed or has no
    /// stored last end vertex, else that vertex.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for an edge outside any loop (upstream dereferences its
    /// null `m_pNext`); [`SolidError::InvalidInput`] for unknown ids.
    pub fn edge_end_poly_vertex(&self, e: PolyEdgeId) -> Result<Option<PolyVertexId>> {
        const OP: &str = "query::poly::PolyBrep::edge_end_poly_vertex";
        let x = self.edge(e)?;
        let next = x.next.ok_or(SolidError::null_pointer(OP))?;
        let last_edge = match x.owner {
            Some(l) => self.lp(l)?.head == Some(next),
            None => false,
        };
        if last_edge {
            let l = self.lp(x.owner.ok_or(dead(OP))?)?;
            if l.closed || l.last_end_vertex.is_none() {
                let h = l.head.ok_or(dead(OP))?;
                Ok(self.edge(h)?.vertex)
            } else {
                Ok(l.last_end_vertex)
            }
        } else {
            Ok(self.edge(next)?.vertex)
        }
    }

    /// `SmPolyEdge::GetEndPoint`.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without an end vertex; those of
    /// [`PolyBrep::edge_end_poly_vertex`].
    pub fn edge_end_point(&self, e: PolyEdgeId) -> Result<Point3> {
        let v = self
            .edge_end_poly_vertex(e)?
            .ok_or(SolidError::null_pointer(
                "query::poly::PolyBrep::edge_end_point",
            ))?;
        Ok(self.vertex(v)?.point)
    }

    /// `SmPolyEdge::~SmPolyEdge` (with `~SmTopology`): cuts the edge out of its radial
    /// ring, its vertex's edge ring and its loop, drops a user of its aux data and
    /// deletes it.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown or deleted ids.
    pub fn delete_edge(&mut self, e: PolyEdgeId) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::delete_edge";
        let x = self.edge(e)?.clone();
        // Cut this polyedge out of the doubly linked list of radial polyedges
        let radial = x.next_radial;
        if radial != e {
            slot_mut(&mut self.edge_slots, radial.0, OP)?.last_radial = x.last_radial;
            let lr = slot(&self.edge_slots, radial.0, OP)?.last_radial;
            slot_mut(&mut self.edge_slots, lr.0, OP)?.next_radial = radial;
        }
        // Cut this polyedge out of the doubly linked list of edges from vertex
        if let Some(v) = x.vertex {
            // the status is ignored upstream
            if let Err(err) = self.vertex_remove_poly_edge(v, e) {
                if matches!(
                    err,
                    SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. }
                ) {
                    return Err(err);
                }
            }
        }
        // Remove the aux data
        if let Some(a) = x.aux {
            self.aux_remove_user(a)?;
        }
        self.loop_remove(e)?;
        if let Some(s) = self.edge_slots.get_mut(e.0 as usize) {
            *s = None;
        }
        Ok(())
    }

    /// `SmPolyEdge::CalculateBoundingBox`: the union of both vertices' boxes.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a missing vertex; [`SolidError::InvalidInput`] for
    /// unknown ids.
    pub fn edge_bounding_box(&self, e: PolyEdgeId) -> Result<Extent3d> {
        const OP: &str = "query::poly::PolyBrep::edge_bounding_box";
        let s = self.edge_vertex(e, OP)?;
        let end = self
            .edge_end_poly_vertex(e)?
            .ok_or(SolidError::null_pointer(OP))?;
        let b = self.vertex_bounding_box(s)?;
        let bv = self.vertex_bounding_box(end)?;
        Ok(b.union(&bv))
    }

    /// `SmPolyEdge::Is2dOrZero`: both vertices are 2D or at `z == 0` (a missing end
    /// vertex counts as such).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without a start vertex; [`SolidError::InvalidInput`] for
    /// unknown ids.
    pub fn edge_is_2d_or_zero(&self, e: PolyEdgeId) -> Result<bool> {
        let end = self.edge_end_poly_vertex(e)?;
        let s = self.edge_vertex(e, "query::poly::PolyBrep::edge_is_2d_or_zero")?;
        Ok(self.vertex_is_2d_or_zero(s)?
            && match end {
                None => true,
                Some(v) => self.vertex_is_2d_or_zero(v)?,
            })
    }

    /// `SmPolyEdge::IsDegenerate(pOptXSectTol3d)`: the chord is shorter than the
    /// intersection tolerance (default the larger of the edge's and the two vertices'
    /// `SmTol::GetXSectTol3d`). Points with `z == 0` or 2D are flattened to `z = 0`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] (`topology.face_p2`, `SmFace::GetSurface`) when the
    /// face has an original face and the back pointers are valid (then upstream maps
    /// UV points through the surface); those of the edge queries.
    pub fn edge_is_degenerate(
        &self,
        k: &Kernel,
        e: PolyEdgeId,
        xsect_tol: Option<f64>,
    ) -> Result<bool> {
        self.edge_is_degenerate_with_owner(k, e, xsect_tol, |_| None)
    }

    /// `SmPolyEdge::IsDegenerate` with access to the original face's real B-rep.
    ///
    /// `resolve` maps the original face key to its owning B-rep and face id. `None`
    /// means unresolved, not a face without a surface: the latter is represented by
    /// a valid face whose `surface` is `None`. With valid back pointers, 2D endpoints
    /// are evaluated on that surface, including upstream's pole/seam midpoint test.
    /// A resolver is not consulted when the back pointers are invalid.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for an unresolved original face; errors of the face
    /// and edge accessors and of surface evaluation. Upstream ignores evaluation
    /// statuses; this method propagates them.
    pub fn edge_is_degenerate_with_owner<'a>(
        &self,
        k: &Kernel,
        e: PolyEdgeId,
        xsect_tol: Option<f64>,
        resolve: impl FnOnce(ObjectKey) -> Option<(&'a Brep, FaceId)>,
    ) -> Result<bool> {
        let face = self.edge_poly_face(e)?;
        // a deleted face has cleared its back pointer
        let orig_face = face
            .and_then(|f| self.face_slots.get(f.0 as usize).and_then(Option::as_ref))
            .and_then(|x| x.original_face);
        let surface = match orig_face {
            Some(key) if self.edge_ok_back_ptrs(e)? => {
                // DISPATCH GAP: an opaque key alone cannot identify an owning B-rep.
                let (brep, face) = resolve(key).ok_or_else(|| {
                    SolidError::unported("SmFace::GetSurface", "topology.face_p2")
                })?;
                crate::topology::face::p2::surface(brep, face)?
            }
            _ => None,
        };

        // Use a local tolerance instead of SmScaledZero
        let xsect = if let Some(t) = xsect_tol {
            t
        } else {
            {
                let et = self.edge(e)?.tolerance;
                let edge_xsect = tol::xsect_tol3d(k, et + et);
                // SmTol::GetZoneTol3d(NULL) is the system zone tolerance
                let zone = |v: Option<PolyVertexId>| -> Result<f64> {
                    match v {
                        Some(v) => Ok(self.vertex(v)?.tolerance),
                        None => Ok(tol::zone_tol_3d(k, tol::SmallTopoType::NormalSize)),
                    }
                };
                let s = self.edge(e)?.vertex;
                let end = self.edge_end_poly_vertex(e)?;
                let vertex_xsect = tol::xsect_tol3d(k, zone(s)? + zone(end)?);
                if edge_xsect > vertex_xsect {
                    edge_xsect
                } else {
                    vertex_xsect
                }
            }
        };

        // Get PolyEdge endPoints.
        let mut start = self.edge_start_point(e)?;
        let mut end = self.edge_end_point(e)?;
        let start_uv = Vec2::new(start.x, start.y);
        let end_uv = Vec2::new(end.x, end.y);
        let mut mid = (start + end) / 2.0;
        let (mut su, mut sv, mut eu, mut ev) = (
            ExtentPointType::Unknown,
            ExtentPointType::Unknown,
            ExtentPointType::Unknown,
            ExtentPointType::Unknown,
        );
        let mut check_pole = false;

        // Match SmPolyEdge::IsDegenerate's imperfect UV/XYZ test, not a new one.
        if self.edge_is_2d_or_zero(e)? {
            if let Some(surface) = surface {
                let domain = surface.natural_uv_domain_with_nurb(k)?;
                (su, sv) = domain.classify_point(start_uv, EFF_ZERO_PARAM, None);
                (eu, ev) = domain.classify_point(end_uv, EFF_ZERO_PARAM, None);
                if su != ExtentPointType::Outside && sv != ExtentPointType::Outside {
                    check_pole = (su == eu
                        && matches!(su, ExtentPointType::Start | ExtentPointType::End))
                        || (sv == ev
                            && matches!(sv, ExtentPointType::Start | ExtentPointType::End));
                }
                start = surface.evaluate_point(k, start_uv)?;
                end = surface.evaluate_point(k, end_uv)?;
            } else {
                start.z = 0.0;
                end.z = 0.0;
                mid.z = 0.0;
            }
        }

        let mut degenerate = start.closer_than(xsect, end);
        if degenerate && self.edge_is_2d_or_zero(e)? {
            if let Some(surface) = surface {
                let opposite = |a, b| {
                    (a == ExtentPointType::Start && b == ExtentPointType::End)
                        || (a == ExtentPointType::End && b == ExtentPointType::Start)
                };
                let mut check_seam =
                    opposite(su, eu) && (start_uv.y - end_uv.y).abs() < EFF_ZERO_PARAM;
                if !check_seam {
                    check_seam = opposite(sv, ev) && (start_uv.x - end_uv.x).abs() < EFF_ZERO_PARAM;
                }
                if check_seam {
                    if check_pole {
                        mid = surface.evaluate_point(k, (start_uv + end_uv) / 2.0)?;
                    }
                    degenerate = start.closer_than(xsect, mid);
                }
            }
        }
        Ok(degenerate)
    }

    // ---- loops ----------------------------------------------------------------------

    /// `SmPolyLoop::GetPolyFace`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted loop.
    pub fn loop_poly_face(&self, l: PolyLoopId) -> Result<Option<PolyFaceId>> {
        Ok(self.lp(l)?.face)
    }

    /// `SmPolyLoop::GetFirstPolyEdge`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted loop.
    pub fn loop_first_poly_edge(&self, l: PolyLoopId) -> Result<Option<PolyEdgeId>> {
        Ok(self.lp(l)?.head)
    }

    /// `SmPolyLoop::IsClosed`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted loop.
    pub fn loop_is_closed(&self, l: PolyLoopId) -> Result<bool> {
        Ok(self.lp(l)?.closed)
    }

    /// `SmPolyLoop::GetOrientation`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted loop.
    pub fn loop_orientation(&self, l: PolyLoopId) -> Result<PolyLoopOrientation> {
        Ok(self.lp(l)?.orientation)
    }

    /// `SmPolyLoop::GetPolyEdges`: the edges in loop order from the head
    /// (`SmOwningTopology::GetAll`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the ring is longer than the stored size;
    /// [`SolidError::InvalidInput`] for unknown ids.
    pub fn loop_poly_edges(&self, l: PolyLoopId) -> Result<Vec<PolyEdgeId>> {
        const OP: &str = "query::poly::PolyBrep::loop_poly_edges";
        let x = self.lp(l)?;
        let mut out = Vec::new();
        let Some(h) = x.head else {
            return Ok(out);
        };
        let mut e = h;
        loop {
            out.push(e);
            if out.len() > x.size {
                return Err(SolidError::sm_err(OP));
            }
            match self.edge(e)?.next {
                Some(n) if n != h => e = n,
                _ => break,
            }
        }
        Ok(out)
    }

    /// `SmPolyLoop::~SmPolyLoop` (with `~SmTopology`): takes the loop off its face and
    /// deletes it; its edges keep the dead loop id.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted loop.
    pub fn delete_loop(&mut self, l: PolyLoopId) -> Result<()> {
        let face = self.lp(l)?.face;
        if let Some(f) = face {
            if let Ok(fx) = slot_mut(
                &mut self.face_slots,
                f.0,
                "query::poly::PolyBrep::delete_loop",
            ) {
                fx.loops.retain(|&x| x != l);
            }
        }
        if let Some(s) = self.loop_slots.get_mut(l.0 as usize) {
            *s = None;
        }
        Ok(())
    }

    /// `SmPolyLoop::ContainsPoint(point, eInside, tol, bWorkIn3D)` (upstream default
    /// `tol` is `SM_EFF_ZERO`, `work_in_3d` `FALSE`): winding-number classification of
    /// `point` against the loop, in the XY plane or, in 3D, in the loop's plane.
    /// Isthmus / strut edges (with a radial partner in the same loop) are skipped.
    ///
    /// # Errors
    ///
    /// Those of the basis construction (`Unitize`, `SetCanonical`) and of
    /// [`PolyBrep::face_normal`].
    pub fn loop_contains_point(
        &mut self,
        k: &Kernel,
        l: PolyLoopId,
        point: Point3,
        tol: f64,
        work_in_3d: bool,
    ) -> Result<PolyContainment> {
        let edges = self.loop_poly_edges(l)?;

        // skip degenerate PolyLoops
        if edges.len() <= 2 {
            return Ok(PolyContainment::Unknown);
        }

        // Get Bounding box of all edge start points.
        let mut bbox = Extent3d::new();
        for &e in &edges {
            bbox.add_point(self.edge_start_point(e)?);
        }

        // when point is outside polyLoop bbox
        let mut point_box = Extent3d::from_point(point);
        point_box.expand_absolute(tol * 10.0);
        if point_box.are_disjoint(&bbox, 0.0) {
            return Ok(PolyContainment::Outside);
        }

        let mut xform = Axis2Placement::default();

        if work_in_3d {
            // First find a reasonable tolerance for vector lengths.
            let min_len_sq = bbox.size().length_squared() / (10000.0 * 10000.0);
            let n = edges.len();
            let first = *edges
                .first()
                .ok_or(dead("query::poly::PolyBrep::loop_contains_point"))?;
            let mut origin = self.edge_start_point(first)?;
            let mut x_axis = Vec3::UNDEF;
            let mut y_axis = Vec3::UNDEF;
            let mut z_axis = Vec3::UNDEF;

            // Find the x-axis: the first non-zero leg.
            let mut i_temp = 0usize;
            for (ii, &e) in edges.iter().enumerate() {
                x_axis = self.edge_end_point(e)? - origin;
                if x_axis.length_squared() > min_len_sq {
                    x_axis = x_axis.unitize_or_z();
                    i_temp = ii;
                    break;
                }
                if ii >= n - 1 {
                    return Ok(PolyContainment::Unknown); // Unable to find plane normal
                }
            }

            // Got X, now find a second vector for a basis; try the face's normal.
            let mut got_basis = false;
            if let Some(face) = self.lp(l)?.face {
                z_axis = self.face_normal(k, face, false, true)?.0;
                if z_axis.length_squared() < 1.01 {
                    // if uninitialized, Z is (1,1,1)
                    y_axis = z_axis.cross(x_axis);
                    let r = y_axis.unitize();
                    y_axis = y_axis.unitize_or_z();
                    if r.is_ok() {
                        got_basis = true;
                    }
                }
            }

            if !got_basis {
                // Try to find our own.
                for (ii, &e) in edges.iter().enumerate().skip(i_temp + 1) {
                    y_axis = self.edge_end_point(e)? - origin;
                    z_axis = x_axis.cross(y_axis);
                    if z_axis.length_squared() > min_len_sq {
                        z_axis = z_axis.unitize_or_z();
                        break;
                    }
                    if ii >= n - 1 {
                        return Ok(PolyContainment::Unknown); // Unable to find plane normal
                    }
                }

                // Make an orthogonal basis
                y_axis = z_axis.cross(x_axis);
                let r = y_axis.unitize();
                y_axis = y_axis.unitize_or_z();
                r?;
            }

            // Set up the transform from our basis vectors.
            origin = point;
            let mut plane_position = Axis2Placement::default();
            plane_position.set_canonical(origin, x_axis, y_axis)?;
            xform = plane_position.invert();
        }

        // transform the point
        let test_pnt = xform.transform_point(point);

        // Winding-number check over each edge from start to end, skipping isthmus and
        // strut edges.
        let mut quadrant_change_count: i64 = 0;
        for &seg in &edges {
            let seg_loop = self.edge(seg)?.owner;
            let mut ignore = false;
            for rad in self.edge_all_radials(seg)? {
                if rad != seg && seg_loop == self.edge(rad)?.owner {
                    ignore = true;
                    break;
                }
            }
            if ignore {
                continue;
            }

            // Get start and end points for this edge.
            let start_pnt = xform.transform_point(self.edge_start_point(seg)?);
            let end_pnt = xform.transform_point(self.edge_end_point(seg)?);

            // Check for 'on'.
            if is_point_on_segment(start_pnt, end_pnt, test_pnt, tol)?.on {
                return Ok(PolyContainment::OnBoundary);
            }

            // Get start and end quadrants for this edge.
            let this_quadrant = poly_quadrant(start_pnt, test_pnt);
            let next_quadrant = poly_quadrant(end_pnt, test_pnt);
            let delta =
                poly_adjust_delta(next_quadrant - this_quadrant, start_pnt, end_pnt, test_pnt);
            quadrant_change_count += delta;
        }

        // Sign doesn't matter now.
        let count = quadrant_change_count.abs();
        Ok(if count == 4 {
            PolyContainment::Inside
        } else if count == 0 {
            PolyContainment::Outside
        } else if count > 2 {
            // Best guess (can happen when strut-type edges are skipped)
            PolyContainment::Inside
        } else {
            PolyContainment::Outside
        })
    }

    // ---- faces ----------------------------------------------------------------------

    /// `SmPolyFace::GetOriginalFace`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_original_face(&self, f: PolyFaceId) -> Result<Option<ObjectKey>> {
        Ok(self.face(f)?.original_face)
    }

    /// `SmPolyFace::GetOuterPolyLoop`: the first loop.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_outer_poly_loop(&self, f: PolyFaceId) -> Result<Option<PolyLoopId>> {
        Ok(self.face(f)?.loops.first().copied())
    }

    /// `SmPolyFace::GetTolerance`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_tolerance(&self, f: PolyFaceId) -> Result<f64> {
        Ok(self.face(f)?.tolerance)
    }

    /// `SmPolyFace::GetMaxDeviation`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_max_deviation(&self, f: PolyFaceId) -> Result<f64> {
        Ok(self.face(f)?.max_deviation)
    }

    /// The stored normal `m_vNormal` (no recomputation).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_stored_normal(&self, f: PolyFaceId) -> Result<Vec3> {
        Ok(self.face(f)?.normal)
    }

    /// The stored bounding sphere (`m_vSphereCenter`, `m_dSphereRadius`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_stored_sphere(&self, f: PolyFaceId) -> Result<(Point3, f64)> {
        let x = self.face(f)?;
        Ok((x.sphere_center, x.sphere_radius))
    }

    /// `SmPolyFace::GetCPolyFace`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_cpoly_face(&self, f: PolyFaceId) -> Result<Option<PolyFaceId>> {
        Ok(self.face(f)?.cpoly_face)
    }

    /// `SmPolyFace::GetPolyShell`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_poly_shell(&self, f: PolyFaceId) -> Result<Option<PolyShellId>> {
        Ok(self.face(f)?.shell)
    }

    /// `SmPolyFace::SetTolerance` (`zone_tol` is an `SmZoneTol3d` value).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_set_tolerance(&mut self, f: PolyFaceId, zone_tol: f64) -> Result<()> {
        slot_mut(
            &mut self.face_slots,
            f.0,
            "query::poly::PolyBrep::face_set_tolerance",
        )?
        .tolerance = zone_tol;
        Ok(())
    }

    /// `SmPolyFace::SetNormalUnInit`: forces the next [`PolyBrep::face_normal`] to
    /// recompute.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn face_set_normal_uninit(&mut self, f: PolyFaceId) -> Result<()> {
        slot_mut(
            &mut self.face_slots,
            f.0,
            "query::poly::PolyBrep::face_set_normal_uninit",
        )?
        .max_deviation = -BIG_DOUBLE;
        Ok(())
    }

    /// `GetPolyLoops` (virtual): a face's loops; a composite face's member faces'
    /// loops in member order.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown or deleted ids.
    pub fn face_poly_loops(&self, f: PolyFaceId) -> Result<Vec<PolyLoopId>> {
        let x = self.face(f)?;
        match &x.composite {
            None => Ok(x.loops.clone()),
            Some(members) => {
                let mut out = Vec::new();
                for &m in members {
                    out.extend(self.face(m)?.loops.iter().copied());
                }
                Ok(out)
            }
        }
    }

    /// `GetPolyEdges` (virtual): the edges of every loop in loop order; for a
    /// composite face, of every member face.
    ///
    /// # Errors
    ///
    /// Those of [`PolyBrep::loop_poly_edges`].
    pub fn face_poly_edges(&self, f: PolyFaceId) -> Result<Vec<PolyEdgeId>> {
        let x = self.face(f)?;
        match &x.composite {
            None => {
                let mut out = Vec::new();
                for &l in &x.loops {
                    out.extend(self.loop_poly_edges(l)?);
                }
                Ok(out)
            }
            Some(members) => {
                let mut out = Vec::new();
                for &m in members {
                    out.extend(self.face_poly_edges(m)?);
                }
                Ok(out)
            }
        }
    }

    /// `GetOKBackPtrs` (virtual): a face in a shell answers the poly B-rep's flag; a
    /// composite face its first member's answer (`FALSE` without members).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for unknown or deleted ids.
    pub fn face_ok_back_ptrs(&self, f: PolyFaceId) -> Result<bool> {
        let x = self.face(f)?;
        match &x.composite {
            None => Ok(x.shell.is_some() && self.ok_back_ptrs),
            Some(members) => match members.first() {
                Some(&m) => self.face_ok_back_ptrs(m),
                None => Ok(false),
            },
        }
    }

    /// `SmCPolyFace::GetPolyFacesOfComposite`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown face or one that is not composite.
    pub fn cpoly_faces_of_composite(&self, cpoly: PolyFaceId) -> Result<Vec<PolyFaceId>> {
        self.face(cpoly)?
            .composite
            .clone()
            .ok_or(dead("query::poly::PolyBrep::cpoly_faces_of_composite"))
    }

    /// `SmCPolyFace::RemovePolyFace`: removes the first occurrence of `face` from the
    /// members; a face that is not a member removes the first member (upstream's
    /// found index stays 0), nothing when there are no members.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown face or one that is not composite.
    pub fn cpoly_remove_poly_face(&mut self, cpoly: PolyFaceId, face: PolyFaceId) -> Result<()> {
        let members = slot_mut(
            &mut self.face_slots,
            cpoly.0,
            "query::poly::PolyBrep::cpoly_remove_poly_face",
        )?
        .composite
        .as_mut()
        .ok_or(dead("query::poly::PolyBrep::cpoly_remove_poly_face"))?;
        let found = members.iter().position(|&m| m == face).unwrap_or(0);
        if found < members.len() {
            members.remove(found);
        }
        Ok(())
    }

    /// `SmPolyFace::~SmPolyFace` / `SmCPolyFace::~SmCPolyFace` (with `~SmTopology`): a
    /// composite face first clears its members' back pointers; a member face leaves
    /// its composite. The face then leaves its shell (or the composite face list) and
    /// is deleted; its loops keep the dead face id.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted face.
    pub fn delete_face(&mut self, f: PolyFaceId) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::delete_face";
        let x = self.face(f)?.clone();
        if let Some(members) = &x.composite {
            // ~SmCPolyFace: clear the faces' back pointers
            for &m in members {
                if let Ok(mf) = slot_mut(&mut self.face_slots, m.0, OP) {
                    mf.cpoly_face = None;
                }
            }
            if let Some(c) = slot_mut(&mut self.face_slots, f.0, OP)?.composite.as_mut() {
                c.clear();
            }
        }
        // when member of CFace - remove from owning CFace
        let cp = slot(&self.face_slots, f.0, OP)?.cpoly_face;
        if let Some(c) = cp {
            // a deleted composite face has no members left (upstream reads its reset
            // member array): nothing to remove
            if c != f && self.face(c).is_ok() {
                self.cpoly_remove_poly_face(c, f)?;
            }
        }
        // ~SmTopology: leave the owning list
        if x.composite.is_some() {
            self.cpoly_list.retain(|&c| c != f);
        } else if let Some(s) = x.shell {
            if let Ok(sx) = slot_mut(&mut self.shell_slots, s.0, OP) {
                sx.faces.retain(|&y| y != f);
            }
        }
        if let Some(s) = self.face_slots.get_mut(f.0 as usize) {
            *s = None;
        }
        Ok(())
    }

    /// `SmPolyFace::ComputeSphereBound(bForceUpdate)`: the average of the edge start
    /// points and the largest distance to them plus the largest vertex tolerance;
    /// cached on the face (reused while the radius is positive unless forced).
    ///
    /// # Errors
    ///
    /// Those of the edge queries.
    pub fn face_compute_sphere_bound(
        &mut self,
        f: PolyFaceId,
        force_update: bool,
    ) -> Result<(Point3, f64)> {
        const OP: &str = "query::poly::PolyBrep::face_compute_sphere_bound";
        let x = self.face(f)?;
        // no work - work already done
        if x.sphere_radius > 0.0 && !force_update {
            return Ok((x.sphere_center, x.sphere_radius));
        }
        let mut max_tol = 0.0;
        let mut max_rad_sq = 0.0;
        let mut center = Vec3::ZERO;
        let edges = self.face_poly_edges(f)?;
        for &e in &edges {
            let v = self.vertex(self.edge_vertex(e, OP)?)?;
            if v.tolerance > max_tol {
                max_tol = v.tolerance;
            }
            center += v.point;
        }
        // sCenter = Avg(Points) (ULONG count converted to double)
        // plain operator/ (operator/= would skip tiny divisors)
        let avg = center / edges.len() as f64;
        center = avg;
        for &e in &edges {
            let d = self.edge_start_point(e)?.distance_squared(center);
            if d > max_rad_sq {
                max_rad_sq = d;
            }
        }
        let radius = crate::geom::vector3d::sqrt(max_rad_sq) + max_tol;
        let fx = slot_mut(&mut self.face_slots, f.0, OP)?;
        fx.sphere_radius = radius;
        fx.sphere_center = center;
        Ok((center, radius))
    }

    /// `GetNormal(bForceRecompute, bUpdateTolerances, pbIsDegenerate)` (virtual;
    /// upstream defaults `FALSE`, `TRUE`): the face's unit normal and whether it is
    /// degenerate. Triangles (and 2-edge loops) use the cross product of two edges,
    /// larger polygons Newell's sum oriented by the outer loop's area; a face whose
    /// normal cannot be computed answers `(1,1,1)` (stored only when no normal was
    /// given). The normal and the largest vertex deviation from the plane are cached;
    /// with `update_tolerances` edge, radial and vertex tolerances are raised to twice
    /// that deviation (unless it exceeds 100 face tolerances). A composite face
    /// answers its first member's normal, `(1,1,1)` without members.
    ///
    /// # Errors
    ///
    /// Those of the edge queries.
    pub fn face_normal(
        &mut self,
        k: &Kernel,
        f: PolyFaceId,
        force_recompute: bool,
        update_tolerances: bool,
    ) -> Result<(Vec3, bool)> {
        const OP: &str = "query::poly::PolyBrep::face_normal";
        if let Some(members) = &self.face(f)?.composite {
            return match members.first() {
                Some(&m) => self.face_normal(k, m, force_recompute, update_tolerances),
                None => Ok((Vec3::new(1.0, 1.0, 1.0), false)),
            };
        }

        let mut degenerate = false;
        let x = self.face(f)?;
        // no work - normal already computed
        if !force_recompute && x.max_deviation > -BIG_DOUBLE {
            if x.normal.length_squared() > 1.1 {
                degenerate = true;
            }
            return Ok((x.normal, degenerate));
        }

        let mut normal = Vec3::new(1.0, 1.0, 1.0); // init value has magnitude greater than 1.1
        let mut max_deviation = 0.0;
        let edges = self.face_poly_edges(f)?;
        let total = edges.len();
        let edge_at = |i: usize| edges.get(i).copied().ok_or(dead(OP));

        if total == 3 || total == 2 {
            let e0 = edge_at(0)?;
            let p = self.edge_start_point(e0)?;
            let v1 = self.edge_end_point(e0)? - p;
            let v2 = self.edge_end_point(edge_at(1)?)? - p;
            // let normal = crossProduct of two PolyFace->PolyEdge directions
            normal = v1.cross(v2);
            if normal.length_squared() > EFF_ZERO_SQ {
                normal = normal.unitize_or_z();
            } else {
                normal = Vec3::new(1.0, 1.0, 1.0);
            }
        } else if total > 3 {
            let (mut dx, mut dy, mut dz) = (0.0, 0.0, 0.0);
            let loops = self.face_poly_loops(f)?;
            let first_loop = *loops.first().ok_or(dead(OP))?;
            let outer_edges = self.loop_poly_edges(first_loop)?.len();
            let mut points: Vec<Point3> = Vec::new();

            for ii in 0..total {
                let p1 = self.edge_start_point(edge_at(ii)?)?;
                if ii < outer_edges {
                    points.push(p1);
                }
                let p2 = self.edge_start_point(edge_at((ii + 1) % total)?)?;
                dx += p1.y * p2.z - p1.z * p2.y;
                dy += p1.z * p2.x - p1.x * p2.z;
                dz += p1.x * p2.y - p1.y * p2.x;
            }

            normal = Vec3::new(dx, dy, dz);
            if normal.length_squared() > EFF_ZERO_SQ {
                normal = normal.unitize_or_z();
            } else {
                // ambiguous polygons, e.g. symmetric bow ties or polylines
                degenerate = true;
                let closed = self.lp(first_loop)?.closed;
                for ii in 0..total {
                    // watch out for open loops
                    if !closed && (ii + 2) >= total {
                        break;
                    }
                    let p1 = self.edge_start_point(edge_at(ii)?)?;
                    let p2 = self.edge_start_point(edge_at((ii + 1) % total)?)?;
                    let p3 = self.edge_start_point(edge_at((ii + 2) % total)?)?;
                    normal = (p2 - p1).cross(p3 - p2);
                    if normal.length_squared() > EFF_ZERO_SQ {
                        normal = normal.unitize_or_z();
                        break;
                    }
                }
                if normal.length_squared() < EFF_ZERO_SQ {
                    normal = Vec3::new(1.0, 1.0, 1.0);
                }
            }

            // check polygon area to determine which side is positive
            if let Ok(area) = polygon_area(&points, normal) {
                if area < 0.0 {
                    normal = -normal;
                }
            }

            // for polygons with computable normals - get max vertex/plane deviation
            if tol::is_zero(normal.length_squared() - 1.0) {
                let start = self.edge_start_point(edge_at(0)?)?;
                for &e in &edges {
                    let v = self.edge_start_point(e)? - start;
                    let dev = v.dot(normal).abs();
                    if dev > max_deviation {
                        max_deviation = dev;
                    }
                }
            }
        }

        // when a normal could not be computed for this problem polygon
        if normal.length_squared() > 1.1 {
            degenerate = true;
            let fx = slot_mut(&mut self.face_slots, f.0, OP)?;
            if !fx.normal.is_initialized() {
                fx.max_deviation = 0.0;
                fx.normal = Vec3::new(1.0, 1.0, 1.0);
            }
            return Ok((fx.normal, degenerate));
        }

        // when asked - ensure all PolyEdge and PolyVertex tolerances >= 2 * MaxDeviation
        if update_tolerances {
            let face_tol = self.face(f)?.tolerance;
            if max_deviation > face_tol && max_deviation <= 100.0 * face_tol {
                let twice = max_deviation * 2.0;
                for &e in &edges {
                    let radial = self.edge_radial(e)?;
                    let v = self.edge_vertex(e, OP)?;
                    if self.edge(e)?.tolerance < twice {
                        self.edge_set_tolerance(e, tol::zone_tol3d(k, twice))?;
                    }
                    if let Some(r) = radial {
                        if self.edge(r)?.tolerance < twice {
                            self.edge_set_tolerance(r, tol::zone_tol3d(k, twice))?;
                        }
                    }
                    if self.vertex(v)?.tolerance < twice {
                        self.vertex_set_tolerance(v, tol::zone_tol3d(k, twice), true)?;
                    }
                }
            }
        }

        let fx = slot_mut(&mut self.face_slots, f.0, OP)?;
        fx.max_deviation = max_deviation;
        fx.normal = normal;
        Ok((normal, degenerate))
    }

    /// `SmPolyFace::SetOriginalFace(pOriginalFace, bPropagateAttribs)` (upstream default
    /// `FALSE`): stores the new original key first, then optionally notifies the real
    /// source face with `Copy` to propagate attributes, flags and user values while
    /// resetting the destination's three general marks.
    ///
    /// `copy_env` is required only for a changed, non-null original with propagation
    /// enabled. Ownerless labels remain usable without it when propagation is
    /// disabled; unchanged and null assignments never inspect it.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown poly face, missing required copy
    /// context, inconsistent source identity, or an incorrectly typed/colliding
    /// destination identity; those of the source face and its notification services.
    /// Notification errors leave the new original key stored, as upstream assigns
    /// before notifying.
    pub fn face_set_original_face(
        &mut self,
        f: PolyFaceId,
        original: Option<ObjectKey>,
        propagate_attribs: bool,
        copy_env: Option<&mut PolyFaceCopyEnv<'_>>,
    ) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::face_set_original_face";
        let x = slot_mut(&mut self.face_slots, f.0, OP)?;
        if x.original_face == original {
            return Ok(());
        }
        x.original_face = original;
        let Some(original) = original.filter(|_| propagate_attribs) else {
            return Ok(());
        };
        let env = copy_env.ok_or(SolidError::InvalidInput {
            operation: OP,
            reason: "attribute propagation requires a real source face and notification context",
        })?;
        let source = (env.notify.key_of)(crate::EntityRef::Face(env.source_face));
        if source.key != original || source.type_code != crate::topology::face::p2::TYPE {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "original key does not identify the supplied source face",
            });
        }
        let destination_type = if x.composite.is_some() {
            PolyObjectKind::CPolyFace.type_code()
        } else {
            PolyObjectKind::Face.type_code()
        };
        if env.destination.type_code != destination_type || env.destination.key == source.key {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "copy destination must identify this distinct concrete poly face",
            });
        }
        env.source_brep.face(env.source_face)?;
        crate::topology::face::p3::notify(
            env.source_brep,
            env.source_face,
            &mut env.notify,
            None,
            NotifyOperation::Copy,
            [Some(env.destination), None, None],
        )
    }

    /// `CalculateBoundingBox` (virtual): a face's box is the union of the boxes of
    /// its edges' start vertices (an empty face gives the default box); a composite
    /// face's the union of its members' boxes with the default box.
    ///
    /// # Errors
    ///
    /// Those of the edge queries.
    pub fn face_bounding_box(&self, f: PolyFaceId) -> Result<Extent3d> {
        let x = self.face(f)?;
        if let Some(members) = &x.composite {
            let mut ret = Extent3d::new();
            for &m in members {
                let e = self.face_bounding_box(m)?;
                ret = ret.union(&e);
            }
            return Ok(ret);
        }
        let mut ext = Extent3d::new();
        for (ii, e) in self.face_poly_edges(f)?.into_iter().enumerate() {
            if let Some(v) = self.edge(e)?.vertex {
                let vb = self.vertex_bounding_box(v)?;
                ext = if ii == 0 { vb } else { ext.union(&vb) };
            }
        }
        Ok(ext)
    }

    /// `SmPolyFace::PointInPolygon(point, rbInside, bWorkIn3D)`: inside or on the
    /// first loop and not strictly inside any other loop, tested with the face
    /// tolerance.
    ///
    /// # Errors
    ///
    /// Those of [`PolyBrep::loop_contains_point`].
    pub fn face_point_in_polygon(
        &mut self,
        k: &Kernel,
        f: PolyFaceId,
        point: Point3,
        work_in_3d: bool,
    ) -> Result<bool> {
        let tol = self.face(f)?.tolerance;
        for (ii, l) in self.face_poly_loops(f)?.into_iter().enumerate() {
            let c = self.loop_contains_point(k, l, point, tol, work_in_3d)?;
            if (ii == 0 && c != PolyContainment::Inside && c != PolyContainment::OnBoundary)
                || (ii != 0 && c == PolyContainment::Inside)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // ---- shells and regions ---------------------------------------------------------

    /// `SmPolyShell::GetPolyFaces`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted shell.
    pub fn shell_poly_faces(&self, s: PolyShellId) -> Result<Vec<PolyFaceId>> {
        Ok(slot(
            &self.shell_slots,
            s.0,
            "query::poly::PolyBrep::shell_poly_faces",
        )?
        .faces
        .clone())
    }

    /// `SmPolyShell::GetPolyRegion`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted shell.
    pub fn shell_poly_region(&self, s: PolyShellId) -> Result<Option<PolyRegionId>> {
        Ok(slot(
            &self.shell_slots,
            s.0,
            "query::poly::PolyBrep::shell_poly_region",
        )?
        .region)
    }

    /// `SmPolyShell::~SmPolyShell` (with `~SmTopology`): takes the shell off its region
    /// and deletes it; its faces keep the dead shell id.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted shell.
    pub fn delete_shell(&mut self, s: PolyShellId) -> Result<()> {
        let region = self.shell_poly_region(s)?;
        if let Some(r) = region {
            if let Ok(rx) = slot_mut(
                &mut self.region_slots,
                r.0,
                "query::poly::PolyBrep::delete_shell",
            ) {
                rx.shells.retain(|&x| x != s);
            }
        }
        if let Some(x) = self.shell_slots.get_mut(s.0 as usize) {
            *x = None;
        }
        Ok(())
    }

    /// `SmPolyRegion::GetPolyShells`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted region.
    pub fn region_poly_shells(&self, r: PolyRegionId) -> Result<Vec<PolyShellId>> {
        Ok(slot(
            &self.region_slots,
            r.0,
            "query::poly::PolyBrep::region_poly_shells",
        )?
        .shells
        .clone())
    }

    /// `SmPolyRegion::~SmPolyRegion` (with `~SmTopology`): takes the region off the
    /// poly B-rep and deletes it.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted region.
    pub fn delete_region(&mut self, r: PolyRegionId) -> Result<()> {
        let listed = slot(
            &self.region_slots,
            r.0,
            "query::poly::PolyBrep::delete_region",
        )?
        .listed;
        if listed {
            self.regions.retain(|&x| x != r);
        }
        if let Some(x) = self.region_slots.get_mut(r.0 as usize) {
            *x = None;
        }
        Ok(())
    }

    // ---- the poly B-rep ---------------------------------------------------------------

    /// `SmPolyBrep::GetPolyRegions`.
    #[must_use]
    pub fn poly_regions(&self) -> Vec<PolyRegionId> {
        self.regions.clone()
    }

    /// `SmPolyBrep::GetPolyVertices`: the vertex list.
    #[must_use]
    pub fn poly_vertices(&self) -> Vec<PolyVertexId> {
        self.vertex_list.clone()
    }

    /// `SmPolyBrep::GetCPolyFaces`: the composite face list.
    #[must_use]
    pub fn cpoly_faces(&self) -> Vec<PolyFaceId> {
        self.cpoly_list.clone()
    }

    /// `SmPolyBrep::GetPolyPoints`: the points of the vertex list.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted listed vertex.
    pub fn poly_points(&self) -> Result<Vec<Point3>> {
        self.vertex_list
            .iter()
            .map(|&v| self.vertex_point(v))
            .collect()
    }

    /// `SmPolyBrep::GetPolyShells`: the shells of every region in region order.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted listed region.
    pub fn poly_shells(&self) -> Result<Vec<PolyShellId>> {
        let mut out = Vec::new();
        for &r in &self.regions {
            out.extend(self.region_poly_shells(r)?);
        }
        Ok(out)
    }

    /// `SmPolyBrep::GetPolyFaces(pOptCommonFace)`: the faces of every shell; with a
    /// common face only those whose original face is it (upstream default `NULL`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted listed objects.
    pub fn poly_faces(&self, common_face: Option<ObjectKey>) -> Result<Vec<PolyFaceId>> {
        let mut out = Vec::new();
        for s in self.poly_shells()? {
            for f in self.shell_poly_faces(s)? {
                if common_face.is_none() || common_face == self.face(f)?.original_face {
                    out.push(f);
                }
            }
        }
        Ok(out)
    }

    /// `SmPolyBrep::GetFirstPolyFace`: the first face of the first shell of the first
    /// region; `None` when a list on the way is empty (`NERN`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects.
    pub fn first_poly_face(&self) -> Result<Option<PolyFaceId>> {
        let Some(&r) = self.regions.first() else {
            return Ok(None);
        };
        let Some(&s) = self.region_poly_shells(r)?.first() else {
            return Ok(None);
        };
        Ok(self.shell_poly_faces(s)?.first().copied())
    }

    /// `SmPolyBrep::GetPolyEdges(pOptCommonEdgeuse)` (upstream default `NULL`): the
    /// edges of every face.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a common edge use: upstream's filtered branch
    /// walks each face's loop list as if it held edges and reads the loops as
    /// `SmPolyEdge` objects (see `plan/upstream-issues.md`); errors of
    /// [`PolyBrep::face_poly_edges`].
    pub fn poly_edges(&self, common_edgeuse: Option<ObjectKey>) -> Result<Vec<PolyEdgeId>> {
        if common_edgeuse.is_some() {
            return Err(SolidError::InvalidInput {
                operation: "query::poly::PolyBrep::poly_edges",
                reason: "common edge use filter reads loops as edges upstream",
            });
        }
        let mut out = Vec::new();
        for f in self.poly_faces(None)? {
            out.extend(self.face_poly_edges(f)?);
        }
        Ok(out)
    }

    /// `SmPolyBrep::GetAuxDataList`: the aux data of every edge starting at each
    /// listed vertex, in vertex and ring order (duplicates kept), each index value set
    /// to 1.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects or a broken ring.
    pub fn aux_data_list(&mut self) -> Result<Vec<PolyAuxId>> {
        const OP: &str = "query::poly::PolyBrep::aux_data_list";
        let mut out = Vec::new();
        for v in self.vertex_list.clone() {
            for e in self.vertex_starting_poly_edges(v)? {
                if let Some(a) = self.edge(e)?.aux {
                    slot_mut(&mut self.aux_slots, a.0, OP)?.set_index_value(1);
                    out.push(a);
                }
            }
        }
        Ok(out)
    }

    /// `SmPolyBrep::CalculateBoundingBox(rNormalBox, pOptPseudoBox, pOptPolyPoints,
    /// pOptCentroid, pOptInputPseudoBoxNormal)`.
    ///
    /// With `poly_points` (filled with the vertex points when empty) the boxes and
    /// centroid are those of that point set (tolerance `FALSE`, i.e. `0.0`), not
    /// expanded; `pseudo_box_normal` is unitized in place and fixes the pseudo box's
    /// third axis. Without, the normal box is the union of the vertices' expanded
    /// boxes and the centroid the average vertex point.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a pseudo box is asked for without
    /// `poly_points` (upstream dereferences the null pointer); those of the point set.
    pub fn calculate_bounding_box(
        &self,
        want_pseudo_box: bool,
        poly_points: Option<&mut Vec<Point3>>,
        want_centroid: bool,
        mut pseudo_box_normal: Option<&mut Vec3>,
    ) -> Result<PolyBrepBoxes> {
        if let Some(pts) = poly_points {
            // If passed in but empty, fill it.
            if pts.is_empty() {
                *pts = self.poly_points()?;
            }
            let mut set = PointSet3d::new(pts.clone(), 0.0, None);
            let normal_box = set.bbox(pseudo_box_normal.as_deref_mut())?;
            let pseudo_box = if want_pseudo_box {
                Some(set.tight_pbox(pseudo_box_normal)?)
            } else {
                None
            };
            let centroid = if want_centroid {
                Some(set.centroid())
            } else {
                None
            };
            return Ok(PolyBrepBoxes {
                normal_box,
                pseudo_box,
                centroid,
            });
        }

        let mut bbox = Extent3d::new();
        let mut center = Vec3::ZERO;
        for (ii, &v) in self.vertex_list.iter().enumerate() {
            let vb = self.vertex_bounding_box(v)?;
            bbox = if ii == 0 { vb } else { bbox.union(&vb) };
            if want_centroid {
                center += self.vertex_point(v)?;
            }
        }
        let centroid = if want_centroid {
            Some(center / self.vertex_list.len() as f64)
        } else {
            None
        };
        if want_pseudo_box {
            // upstream builds the point set from the null pOptPolyPoints
            return Err(SolidError::InvalidInput {
                operation: "query::poly::PolyBrep::calculate_bounding_box",
                reason: "pseudo box without a point array (upstream null dereference)",
            });
        }
        Ok(PolyBrepBoxes {
            normal_box: bbox,
            pseudo_box: None,
            centroid,
        })
    }

    /// `SmPolyBrep::Notify`: drops the poly B-rep's cache before and after an edit, on
    /// removal, merge and destruction, then passes the notification to
    /// `SmSAGObject::Notify`.
    ///
    /// # Errors
    ///
    /// Those of the cache deletion and of [`a_object::sag_notify`].
    pub fn notify<R: TopologyRecords + ?Sized, T>(
        &mut self,
        env: &mut PolyNotifyEnv<'_, R, T>,
        operation: NotifyOperation,
        data1: Option<ObjectRef>,
        data2: Option<ObjectRef>,
        data3: Option<ObjectRef>,
    ) -> Result<()> {
        match operation {
            NotifyOperation::PreEdit
            | NotifyOperation::PostEdit
            | NotifyOperation::RmFromBrep
            | NotifyOperation::Destruction
            | NotifyOperation::MergeInBrep => {
                a_object::delete_objects_cache(env.caches, &mut self.aobject)?;
            }
            // SM_NO_UNKNOWN only prints a message; nothing else to do
            _ => {}
        }
        // Propagate notification up hierarchy
        a_object::sag_notify(
            env.records,
            env.attributes,
            env.context.as_deref_mut(),
            operation,
            env.this,
            data1,
            data2,
            data3,
        )
    }

    /// `SmPolyBrep::Transform(crRotateNMove, cpOptScale)`: scales every vertex point
    /// (about the origin) and then moves it rigidly; scales the poly B-rep, vertex, edge
    /// and face tolerances by the largest absolute scale factor; maps vertex and aux
    /// data normals through the scale's cofactor and the rotation; recomputes every
    /// face normal. Identity transforms do nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` for an undefined scale; errors of the normal mapping
    /// (then the edit stops half done, as upstream) and of the notifications.
    pub fn transform<R: TopologyRecords + ?Sized, T>(
        &mut self,
        k: &Kernel,
        env: &mut PolyNotifyEnv<'_, R, T>,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::transform";
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        if scale.is_some_and(Vec3::is_undef) {
            return Err(SolidError::Upstream {
                operation: OP,
                code: SM_ERR_INVALID_INPUT,
                entity: None,
            });
        }

        // we're starting - inform the public
        let this = Some(env.this);
        self.notify(env, NotifyOperation::PreEdit, this, None, None)?;

        let mut max_scale = 1.0;
        if let Some(s) = scale {
            max_scale = s.max_dimension();
            self.tolerance = tol::tol3d(self.tolerance * max_scale);
        }

        // Transform PolyVertices
        for v in self.vertex_list.clone() {
            let mut p = self.vertex_point(v)?;
            if let Some(s) = scale {
                p.x *= s.x;
                p.y *= s.y;
                p.z *= s.z;
                let t = tol::zone_tol3d(k, max_scale * self.vertex_tolerance(v)?);
                self.vertex_set_tolerance(v, t, true)?;
            }
            p = rotate_n_move.transform_point(p);
            self.vertex_set_point(v, p)?;

            let normals = self.vertex_normals_mut(v)?;
            for n in normals.iter_mut() {
                transform_poly_normal(rotate_n_move, scale, n)?;
            }
        }

        // transform normals in SmPolyVertAuxData objects, each once
        let aux = self.aux_data_list()?;
        let mut done: Vec<PolyAuxId> = Vec::new();
        for a in aux {
            if !done.contains(&a) {
                done.push(a);
                let d = slot_mut(&mut self.aux_slots, a.0, OP)?;
                transform_poly_normal(rotate_n_move, scale, &mut d.normal)?;
            }
        }

        // transform PolyEdges
        for e in self.poly_edges(None)? {
            let t = tol::zone_tol3d(k, self.edge_tolerance(e)? * max_scale);
            self.edge_set_tolerance(e, t)?;
        }

        // transform PolyFaces
        for f in self.poly_faces(None)? {
            let t = tol::zone_tol3d(k, self.face_tolerance(f)? * max_scale);
            self.face_set_tolerance(f, t)?;
            self.face_normal(k, f, true, true)?;
        }

        // we're done - inform the public
        self.notify(env, NotifyOperation::PostEdit, this, None, None)?;
        Ok(())
    }

    /// `SmPolyBrep::~SmPolyBrep` without the original B-rep
    /// ([`PolyBrep::destroy_with_original`] with `None`).
    ///
    /// # Errors
    ///
    /// As [`PolyBrep::destroy_with_original`].
    pub fn destroy(self) -> Result<()> {
        self.destroy_with_original(None)
    }

    /// `SmPolyBrep::~SmPolyBrep`: deletes the composite faces, then every edge, loop,
    /// face, shell and region, then the vertices. With valid back pointers, each poly
    /// vertex with an original vertex whose poly vertex is this one clears it
    /// (`SetPolyVertex(NULL)`) on `original`: the B-rep of the original vertices and the
    /// resolver of their keys.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a back pointer has to be followed without
    /// `original`, or its resolver does not know the key (upstream's pointer is always
    /// live); [`SolidError::NotFound`] for a resolved vertex not in `original`.
    pub fn destroy_with_original(
        mut self,
        mut original: Option<(&mut Brep, VertexOfKey<'_>)>,
    ) -> Result<()> {
        const OP: &str = "query::poly::PolyBrep::destroy_with_original";

        // delete m_pCPolyFaceListHead list members
        for c in self.cpoly_faces() {
            self.delete_face(c)?;
        }

        // delete m_pVertexListHead list members
        for v in self.poly_vertices() {
            // if this PolyBrep is pointing to a current Brep - clear Brep back pointers if
            // possible
            let Some(key) = self.vertex(v)?.original_vertex else {
                continue;
            };
            if !self.ok_back_ptrs {
                continue;
            }
            let Some((b, vertex_of)) = original.as_mut() else {
                return Err(SolidError::invalid_input(
                    OP,
                    "the original B-rep is needed to clear its poly vertex back pointers",
                ));
            };
            let vertex_of: &dyn Fn(ObjectKey) -> Option<VertexId> = *vertex_of;
            let vertex = vertex_of(key)
                .ok_or_else(|| SolidError::invalid_input(OP, "unknown original vertex"))?;
            if vertex::poly_vertex(b, vertex)? == Some(v) {
                vertex::set_poly_vertex(b, vertex, None)?;
            }
        }

        // clear remaining internal member values
        self.tolerance = UNDEF_DOUBLE;
        self.ok_back_ptrs = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(k: &Kernel) -> (PolyBrep, PolyFaceId, PolyLoopId, Vec<PolyEdgeId>) {
        let mut b = PolyBrep::new(k, 1.0e-5);
        let r = b.add_region().unwrap();
        let s = b.add_shell(r).unwrap();
        let f = b.add_face(k, s, 1.0e-5, None).unwrap();
        let pts = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let vs: Vec<_> = pts
            .iter()
            .map(|&p| b.add_vertex(k, p, 1.0e-5).unwrap())
            .collect();
        let l = b.start_poly_edge_loop(f).unwrap();
        let mut es = Vec::new();
        for j in 0..4 {
            es.push(
                b.add_poly_edge(k, l, 1.0e-5, pts[j], pts[(j + 1) % 4], vs[j])
                    .unwrap(),
            );
        }
        b.finish_poly_edge_loop(l).unwrap();
        (b, f, l, es)
    }

    #[test]
    fn destroy_clears_original_back_pointers() {
        let k = Kernel::default();
        let build = |ok: bool| {
            let (mut poly, ..) = square(&k);
            let pv = poly.poly_vertices();
            let mut b = Brep::new();
            let ov: Vec<VertexId> = (0..3)
                .map(|_| {
                    b.add_vertex_record(crate::topology::Vertex::default())
                        .unwrap()
                })
                .collect();
            // ov[0] names pv[0]; ov[1] names another poly vertex; ov[2] names none.
            vertex::set_poly_vertex(&mut b, ov[0], Some(pv[0])).unwrap();
            vertex::set_poly_vertex(&mut b, ov[1], Some(pv[3])).unwrap();
            for (j, &v) in pv.iter().take(3).enumerate() {
                poly.vertex_set_original_vertex(v, Some(ObjectKey(j as u64 + 1)))
                    .unwrap();
            }
            poly.set_ok_back_ptrs(ok);
            (poly, b, ov, pv)
        };
        let resolve = |ov: Vec<VertexId>| {
            move |key: ObjectKey| {
                ov.get(usize::try_from(key.0).ok()?.checked_sub(1)?)
                    .copied()
            }
        };

        let (poly, mut b, ov, pv) = build(true);
        let r = resolve(ov.clone());
        let r: &dyn Fn(ObjectKey) -> Option<VertexId> = &r;
        poly.destroy_with_original(Some((&mut b, r))).unwrap();
        assert_eq!(vertex::poly_vertex(&b, ov[0]).unwrap(), None);
        assert_eq!(vertex::poly_vertex(&b, ov[1]).unwrap(), Some(pv[3]));
        assert_eq!(vertex::poly_vertex(&b, ov[2]).unwrap(), None);

        // Without valid back pointers nothing is followed.
        let (poly, b, ov, pv) = build(false);
        poly.destroy().unwrap();
        assert_eq!(vertex::poly_vertex(&b, ov[0]).unwrap(), Some(pv[0]));

        // A back pointer to follow needs the original B-rep and a known key.
        let (poly, ..) = build(true);
        assert!(matches!(
            poly.destroy(),
            Err(SolidError::InvalidInput { .. })
        ));
        let (poly, mut b, ..) = build(true);
        let none: &dyn Fn(ObjectKey) -> Option<VertexId> = &|_| None;
        assert!(matches!(
            poly.destroy_with_original(Some((&mut b, none))),
            Err(SolidError::InvalidInput { .. })
        ));

        // No original vertices: nothing to clear.
        let (poly, ..) = square(&k);
        poly.destroy().unwrap();
    }

    #[test]
    fn square_topology_and_normal() {
        let k = Kernel::default();
        let (mut b, f, l, es) = square(&k);
        assert_eq!(b.loop_poly_edges(l).unwrap(), es);
        assert_eq!(b.edge_cw_poly_edge(es[0]).unwrap(), Some(es[3]));
        assert_eq!(
            b.edge_end_poly_vertex(es[3]).unwrap(),
            b.edge_start_poly_vertex(es[0]).unwrap()
        );
        let (n, deg) = b.face_normal(&k, f, false, true).unwrap();
        assert!(!deg);
        assert_eq!(n, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(b.first_poly_face().unwrap(), Some(f));
        let bb = b.face_bounding_box(f).unwrap();
        assert!((bb.max().x - (1.0 + 1.0e-5)).abs() < 1e-15);
    }

    #[test]
    fn containment() {
        let k = Kernel::default();
        let (mut b, f, l, _) = square(&k);
        let c =
            |b: &mut PolyBrep, p: Vec3, d3| b.loop_contains_point(&k, l, p, 1.0e-6, d3).unwrap();
        assert_eq!(
            c(&mut b, Vec3::new(0.5, 0.5, 0.0), false),
            PolyContainment::Inside
        );
        assert_eq!(
            c(&mut b, Vec3::new(0.5, 0.5, 0.0), true),
            PolyContainment::Inside
        );
        assert_eq!(
            c(&mut b, Vec3::new(1.5, 0.5, 0.0), true),
            PolyContainment::Outside
        );
        assert_eq!(
            c(&mut b, Vec3::new(1.0, 0.5, 0.0), false),
            PolyContainment::OnBoundary
        );
        assert!(b
            .face_point_in_polygon(&k, f, Vec3::new(0.2, 0.2, 0.0), false)
            .unwrap());
    }

    #[test]
    fn edge_set_aux_data_rejects_invalid_replacement_without_mutation() {
        let k = Kernel::default();
        for has_old in [false, true] {
            for deleted in [false, true] {
                let (mut b, _, l, es) = square(&k);
                let old = if has_old {
                    let a = b.add_aux_data(Vec3::ZERO, Vec2::new(0.0, 0.0)).unwrap();
                    b.edge_set_aux_data(es[0], Some(a)).unwrap();
                    Some(a)
                } else {
                    None
                };
                let invalid = if deleted {
                    let a = b.add_aux_data(Vec3::ZERO, Vec2::new(0.0, 0.0)).unwrap();
                    b.aux_delete_if_no_users(a).unwrap();
                    a
                } else {
                    PolyAuxId::from_index(u32::MAX)
                };
                let before = b.clone();
                assert!(matches!(
                    b.edge_set_aux_data(es[0], Some(invalid)),
                    Err(SolidError::InvalidInput { .. })
                ));
                assert_eq!(b, before);

                b.delete_edge(es[0]).unwrap();
                assert_eq!(b.loop_poly_edges(l).unwrap(), es[1..]);
                assert!(b.edge_aux_data(es[0]).is_err());
                if let Some(a) = old {
                    assert!(b.aux_data(a).is_err());
                }
            }
        }
    }

    #[test]
    fn edge_set_aux_data_same_live_id_preserves_counts() {
        let k = Kernel::default();
        let (mut b, _, _, es) = square(&k);
        let a = b.add_aux_data(Vec3::ZERO, Vec2::new(0.0, 0.0)).unwrap();
        b.edge_set_aux_data(es[0], Some(a)).unwrap();
        assert_eq!(b.aux_data(a).unwrap().num_users(), 1);
        let before = b.clone();
        b.edge_set_aux_data(es[0], Some(a)).unwrap();
        assert_eq!(b, before);
        assert_eq!(b.aux_data(a).unwrap().num_users(), 1);
    }

    #[test]
    fn edge_set_aux_data_replacement_deletes_exclusively_owned_record() {
        let k = Kernel::default();
        let (mut b, _, _, es) = square(&k);
        let old = b.add_aux_data(Vec3::ZERO, Vec2::new(0.0, 0.0)).unwrap();
        let new = b.add_aux_data(Vec3::ZERO, Vec2::new(1.0, 1.0)).unwrap();
        b.edge_set_aux_data(es[0], Some(old)).unwrap();
        b.edge_set_aux_data(es[0], Some(new)).unwrap();
        assert_eq!(b.edge_aux_data(es[0]).unwrap(), Some(new));
        assert!(b.aux_data(old).is_err());
        assert_eq!(b.aux_data(new).unwrap().num_users(), 1);
    }

    #[test]
    fn edge_set_aux_data_shared_record_survives_until_last_user_clears() {
        let k = Kernel::default();
        let (mut b, _, _, es) = square(&k);
        let a = b.add_aux_data(Vec3::ZERO, Vec2::new(0.0, 0.0)).unwrap();
        b.edge_set_aux_data(es[0], Some(a)).unwrap();
        b.edge_set_aux_data(es[1], Some(a)).unwrap();
        assert_eq!(b.aux_data(a).unwrap().num_users(), 2);
        b.edge_set_aux_data(es[0], None).unwrap();
        assert_eq!(b.edge_aux_data(es[0]).unwrap(), None);
        assert_eq!(b.edge_aux_data(es[1]).unwrap(), Some(a));
        assert_eq!(b.aux_data(a).unwrap().num_users(), 1);
        b.edge_set_aux_data(es[1], None).unwrap();
        assert_eq!(b.edge_aux_data(es[1]).unwrap(), None);
        assert!(b.aux_data(a).is_err());
    }

    #[test]
    fn delete_edge_unlinks() {
        let k = Kernel::default();
        let (mut b, _, l, es) = square(&k);
        b.link_radials(&[es[0], es[2]]).unwrap();
        let v0 = b.edge_start_poly_vertex(es[0]).unwrap().unwrap();
        b.delete_edge(es[0]).unwrap();
        assert_eq!(b.loop_poly_edges(l).unwrap(), vec![es[1], es[2], es[3]]);
        assert_eq!(b.edge_radial(es[2]).unwrap(), None);
        assert!(b.vertex_starting_poly_edges(v0).unwrap().is_empty());
        assert!(b.edge_tolerance(es[0]).is_err());
    }

    #[test]
    fn original_face_context_is_required_only_after_changed_enabled_assignment() {
        let k = Kernel::default();
        let (mut poly, face, _, _) = square(&k);
        poly.face_set_original_face(face, None, true, None).unwrap();
        poly.face_set_original_face(face, Some(ObjectKey(1)), false, None)
            .unwrap();
        // Same original is a no-op even when propagation is subsequently enabled.
        poly.face_set_original_face(face, Some(ObjectKey(1)), true, None)
            .unwrap();
        assert!(matches!(
            poly.face_set_original_face(face, Some(ObjectKey(2)), true, None),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(poly.face_original_face(face).unwrap(), Some(ObjectKey(2)));
        // A failed notification still leaves the new key assigned.
        poly.face_set_original_face(face, Some(ObjectKey(2)), true, None)
            .unwrap();
        poly.face_set_original_face(face, None, true, None).unwrap();
        assert_eq!(poly.face_original_face(face).unwrap(), None);
    }

    #[test]
    fn original_face_copy_uses_real_notification_and_shared_attribute_lifetimes() {
        use crate::geom::attribute::{Attribute, Behavior};
        use crate::geom::context::MarkType;
        use crate::topology::topology::Topology;
        use crate::topology::{Face, Shell};
        use crate::EntityRef;

        let k = Kernel::default();
        for composite in [false, true] {
            let (mut poly, plain, _, _) = square(&k);
            let face = if composite {
                poly.add_cpoly_face(plain).unwrap()
            } else {
                plain
            };
            let mut source = Brep::new();
            let shell = source.add_shell_record(Shell::default()).unwrap();
            let source_face = source.add_face_record(Face::new(shell)).unwrap();
            let from = ObjectKey(1);
            let to = ObjectKey(2);
            let key_of = |entity| {
                assert_eq!(entity, EntityRef::Face(source_face));
                ObjectRef {
                    type_code: crate::topology::face::p2::TYPE,
                    key: from,
                }
            };
            let context = Context::default();
            let mut src_record = Topology::new();
            src_record.set_all_flags(16);
            src_record.mark(Some(&context), MarkType::Mark);
            src_record.mark(Some(&context), MarkType::MarkIo);
            let mut dst_record = Topology::new();
            dst_record.set_all_flags(2);
            dst_record.mark(Some(&context), MarkType::Mark);
            let destination_io_mark = dst_record.get_mark(MarkType::MarkIo);
            let mut records =
                crate::topology::topology::NotifyRecords::from(std::collections::BTreeMap::from([
                    (from, src_record),
                    (to, dst_record),
                ]));
            let mut attributes = AttributeStore::new();
            let long = attributes
                .insert(Attribute::long(501, 41, Behavior::Copy))
                .unwrap();
            let color = attributes
                .insert(Attribute::color(502, Vec3::X, Behavior::Reference))
                .unwrap();
            let temp = attributes
                .insert(Attribute::long(503, 81, Behavior::Temp))
                .unwrap();
            for h in [long, color, temp] {
                attributes.add_attribute(from, h).unwrap();
            }
            let mut env = PolyFaceCopyEnv {
                source_brep: &mut source,
                source_face,
                notify: crate::topology::edge::NotifyEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: None, // legitimate null context: real state still supplied
                    key_of: &key_of,
                },
                destination: ObjectRef {
                    type_code: if composite {
                        PolyObjectKind::CPolyFace
                    } else {
                        PolyObjectKind::Face
                    }
                    .type_code(),
                    key: to,
                },
            };
            poly.face_set_original_face(face, Some(from), true, Some(&mut env))
                .unwrap();
            assert_eq!(records[&to].all_flags(), 16);
            assert_eq!(records[&to].get_mark(MarkType::Mark), 0);
            assert_eq!(records[&to].get_mark(MarkType::MarkIo), destination_io_mark);
            let copied = attributes.find_attribute(to, 501).unwrap();
            assert_ne!(copied, long);
            assert_eq!(attributes.get(copied).unwrap().long_value(), Some(41));
            assert_eq!(attributes.find_attribute(to, 502), Some(color));
            assert_eq!(attributes.get(color).unwrap().users().len(), 2);
            assert_eq!(attributes.find_attribute(to, 503), None);
            attributes
                .get_mut(color)
                .unwrap()
                .set_vector3d_value(Vec3::Y)
                .unwrap();
            assert_eq!(
                attributes.get(color).unwrap().vector3d_value(),
                Some(Vec3::Y)
            );
            attributes.clear_attributes(from).unwrap();
            assert!(attributes.get(long).is_none());
            assert_eq!(attributes.get(copied).unwrap().long_value(), Some(41));
            assert_eq!(attributes.get(color).unwrap().users(), &[to]);
            attributes.clear_attributes(to).unwrap();
            assert!(attributes.get(copied).is_none());
            assert!(attributes.get(color).is_none());
        }
    }

    #[test]
    fn original_face_copy_rejects_inconsistent_identities_after_assignment() {
        use crate::topology::topology::Topology;
        use crate::topology::{Face, Shell};

        let k = Kernel::default();
        // Wrong original key, wrong source type, wrong destination type, key collision,
        // missing source record and missing destination record are distinct errors.
        for invalid in 0..6 {
            let (mut poly, face, _, _) = square(&k);
            let mut source = Brep::new();
            let shell = source.add_shell_record(Shell::default()).unwrap();
            let source_face = source.add_face_record(Face::new(shell)).unwrap();
            let mut records =
                crate::topology::topology::NotifyRecords::from(std::collections::BTreeMap::from([
                    (ObjectKey(1), Topology::new()),
                    (ObjectKey(2), Topology::new()),
                ]));
            if invalid == 4 {
                records.remove(&ObjectKey(1));
            }
            if invalid == 5 {
                records.remove(&ObjectKey(2));
            }
            let mut attributes = AttributeStore::new();
            let key_of = |_| ObjectRef {
                type_code: if invalid == 1 {
                    PolyObjectKind::Face.type_code()
                } else {
                    crate::topology::face::p2::TYPE
                },
                key: ObjectKey(1),
            };
            let mut env = PolyFaceCopyEnv {
                source_brep: &mut source,
                source_face,
                notify: crate::topology::edge::NotifyEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: None,
                    key_of: &key_of,
                },
                destination: ObjectRef {
                    type_code: if invalid == 2 {
                        PolyObjectKind::Brep.type_code()
                    } else {
                        PolyObjectKind::Face.type_code()
                    },
                    key: ObjectKey(if invalid == 3 { 1 } else { 2 }),
                },
            };
            let original = Some(ObjectKey(if invalid == 0 { 9 } else { 1 }));
            assert!(matches!(
                poly.face_set_original_face(face, original, true, Some(&mut env)),
                Err(SolidError::InvalidInput { .. })
            ));
            assert_eq!(poly.face_original_face(face).unwrap(), original);
            assert!(attributes.attributes(ObjectKey(2)).is_empty());
        }
    }

    #[test]
    fn transform_scales_and_rejects_undefined() {
        let k = Kernel::default();
        let (mut b, f, _, _) = square(&k);
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::new();
        let mut caches: CacheStore<()> = CacheStore::new();
        let mut env = PolyNotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            caches: &mut caches,
            context: None,
            this: ObjectRef {
                type_code: PolyObjectKind::Brep.type_code(),
                key: ObjectKey(1),
            },
        };
        let id = Axis2Placement::default();
        b.transform(&k, &mut env, &id, Some(Vec3::new(-2.0, 3.0, 4.0)))
            .unwrap();
        assert_eq!(b.poly_points().unwrap()[1], Vec3::new(-2.0, 0.0, 0.0));
        // winding kept: the normal follows the cofactor (sign of the scale parity)
        assert_eq!(b.face_stored_normal(f).unwrap(), Vec3::new(0.0, 0.0, -1.0));
        assert!((b.tolerance() - 4.0e-5).abs() < 1e-18);
        let r = b.transform(&k, &mut env, &id, Some(Vec3::new(1.0e19, 1.0, 1.0)));
        assert!(matches!(r, Err(SolidError::Upstream { code: 1007, .. })));
    }

    #[test]
    fn upstream_quirks() {
        let k = Kernel::default();
        let (b, f, _, _) = square(&k);
        // pseudo box without a point array dereferences null upstream
        assert!(b.calculate_bounding_box(true, None, false, None).is_err());
        assert!(b.poly_edges(Some(ObjectKey(3))).is_err());
        let mut b = b;
        let c = b.add_cpoly_face(f).unwrap();
        // removing a non-member removes the first member
        b.cpoly_remove_poly_face(c, PolyFaceId::from_index(99))
            .unwrap();
        assert!(b.cpoly_faces_of_composite(c).unwrap().is_empty());
    }

    #[test]
    fn nested_composite_rejected() {
        let k = Kernel::default();
        let (mut b, f, _, _) = square(&k);
        let c = b.add_cpoly_face(f).unwrap();
        // a composite inside itself (or another composite) would recurse without bound
        assert!(b.cpoly_add_poly_face(c, c, false).is_err());
        assert!(b.cpoly_add_poly_face(c, c, true).is_err());
        assert!(b.add_cpoly_face(c).is_err());
        assert_eq!(b.cpoly_faces_of_composite(c).unwrap(), vec![f]);
        assert_eq!(b.face_poly_edges(c).unwrap().len(), 4);
        assert!(b.face_bounding_box(c).is_ok());
        assert!(b.face_normal(&k, c, true, true).is_ok());
    }

    #[test]
    fn degeneracy_resolves_real_face_but_not_an_opaque_key() {
        let k = Kernel::default();
        let (mut poly, face, _, edges) = square(&k);
        let edge = edges[0];
        poly.face_set_original_face(face, Some(ObjectKey(7)), false, None)
            .unwrap();
        poly.set_ok_back_ptrs(true);
        assert!(poly
            .edge_is_degenerate(&k, edge, Some(0.5))
            .is_err_and(|e| e.is_unported()));

        let mut owner = Brep::new();
        let shell = owner
            .add_shell_record(crate::topology::Shell::default())
            .unwrap();
        let f = owner
            .add_face_record(crate::topology::Face::new(shell))
            .unwrap();
        // A resolved face without geometry takes upstream's no-surface branch.
        assert!(!poly
            .edge_is_degenerate_with_owner(&k, edge, Some(0.5), |_| Some((&owner, f)))
            .unwrap());
        owner.face_mut(f).unwrap().surface = Some(crate::surface::surface::Surface::from(
            crate::surface::plane::Plane::new(
                Vec3::ZERO,
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec2::new(0.25, 1.0),
                &crate::geom::extent2d::Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            ),
        ));
        assert!(poly
            .edge_is_degenerate_with_owner(&k, edge, Some(0.5), |_| Some((&owner, f)))
            .unwrap());
        assert!(poly
            .edge_is_degenerate_with_owner(&k, edge, Some(0.5), |_| Some((
                &owner,
                FaceId::from_index(u32::MAX)
            )))
            .is_err());

        poly.set_ok_back_ptrs(false);
        assert!(!poly
            .edge_is_degenerate_with_owner(&k, edge, Some(0.5), |_| panic!(
                "invalid back pointers must not resolve the owner"
            ))
            .unwrap());
    }

    #[test]
    fn kinds_and_helpers() {
        assert!(PolyObjectKind::CPolyFace.is_kind_of(PolyObjectKind::Face.type_code()));
        assert!(PolyObjectKind::Face.is_kind_of(OWNING_TOPOLOGY_TYPE));
        assert!(!PolyObjectKind::Edge.is_kind_of(OWNING_TOPOLOGY_TYPE));
        assert!(PolyObjectKind::Brep.is_kind_of(a_object::SAG_TYPE));
        assert_eq!(PolyObjectKind::Loop.class_string(), "SmPolyLoop");
        assert_eq!(PolyObjectKind::Vertex.type_string(), "SmPolyVertex_TYPE");
        assert!(is_uniform_positive_scale(Vec3::new(2.0, 2.0, 2.0)));
        assert!(!is_uniform_positive_scale(Vec3::new(-2.0, -2.0, -2.0)));
        assert!(!is_uniform_positive_scale(Vec3::new(2.0, 2.0, 3.0)));
        assert!(matches!(
            report_invalid_transform_input("bad scale"),
            SolidError::InvalidInput {
                reason: "bad scale",
                ..
            }
        ));
        let mut d = DisplayList::new(3);
        assert_eq!(d.set_display_list_id(7), 3);
        assert_eq!(d.display_list_id(), 7);
    }
}
