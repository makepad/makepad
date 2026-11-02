//! `SmBrep` members of unit `topology.brep_p1`: the tolerance and size-estimate
//! accessors, the topology collections, the infinite region, the region voting, the
//! blend, projection and intersection entry points, and the file-static helpers of
//! `SmBrep.cpp` the unit owns.
//!
//! Upstream: `source/SMLib/inc/SmBrep.h`, `source/SMLib/src/SmBrep.cpp`.
//!
//! | Rust                                              | Upstream                                      |
//! |---------------------------------------------------|-----------------------------------------------|
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`is_kind_of`] | `SmBrep::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`Brep::tolerance`] (in `topology.rs`)            | `SmBrep::GetTolerance`                        |
//! | [`Brep::this_model_size_estimate`]                | `SmBrep::GetThisModelSizeEstimate`            |
//! | [`Brep::set_this_model_size_estimate`]            | `SmBrep::SetThisModelSizeEstimate`            |
//! | [`Brep::this_large_small_size_ratio`]             | `SmBrep::GetThisLargeSmallSizeRatio`          |
//! | [`Brep::set_this_large_small_size_ratio`]         | `SmBrep::SetThisLargeSmallSizeRatio`          |
//! | [`Brep::infinite_region`] (in `topology.rs`), [`Brep::set_infinite_region`] | `SmBrep::GetInfiniteRegion`, `SetInfiniteRegion` |
//! | [`Brep::editing_enabled`], [`Brep::set_editing_enabled`] (in `topology.rs`) | `SmBrep::m_bEditingEnabled` |
//! | [`Brep::face_list`], [`Brep::edge_list`], [`Brep::vertex_list`] (+ `set_*_list`) | `m_pFaceListHead`, `m_pEdgeListHead`, `m_pVertexListHead` (list order) |
//! | [`Brep::add_deleted_surface`], [`Brep::deleted_surfaces`] | `SmBrep::AddDeletedSurface`, `m_pDeletedSurfaces` |
//! | [`num_faces`]                                     | `SmBrep::GetNumFaces`                         |
//! | [`is_live_topology_member`]                       | `SmBrep::IsLiveTopologyMember`                |
//! | [`regions`], [`faces`], [`edges`], [`vertices`]   | `SmBrep::GetRegions`, `GetFaces`, `GetEdges`, `GetVertices` |
//! | [`shells`]                                        | `SmBrep::GetShells`                           |
//! | [`tessellate_boundaries`], [`BoundaryTessellation`] | `SmBrep::TessellateBoundaries`              |
//! | [`surfaces`], [`curves`]                          | `SmBrep::GetSurfaces`, `GetCurves`            |
//! | [`wire_edges`], [`topological_edges`], [`topological_vertices`] | `SmBrep::GetWireEdges`, `GetTopologicalEdges`, `GetTopologicalVertices` |
//! | [`is_degenerate`]                                 | `SmBrep::IsDegenerate`                        |
//! | [`reset_infinite_region`], [`SetRegionVoidType`]  | `SmBrep::ResetInfiniteRegion`, `SmSetRegionVoidType` |
//! | [`set_region_is_void_flags_for_nested_solids`]    | `SmBrep::SetRegionIsVoidFlagsForNestedSolids` |
//! | [`heal_brep`]                                     | `SmBrep::HealBrep`                            |
//! | [`brep_intersect_mut`], [`brep_intersect_self`]   | `SmBrep::BrepIntersect`                       |
//! | [`create_corner_blend`]                           | `SmBrep::CreateCornerBlend`                   |
//! | [`create_edge_edge_blend`]                        | `SmBrep::CreateEdgeEdgeBlend`                 |
//! | [`create_parallel_projection_curves`], [`ParallelProjectionOptions`], [`ParallelProjectionCurves`] | `SmBrep::CreateParallelProjectionCurves` |
//! | [`update_vertex_edge_intervals`]                  | `SmBrep::UpdateVertexEdgeIntervals`           |
//! | [`LockBrepBox`]                                   | `SmLockBrepBox` (constructor, destructor)     |
//! | [`check_uv_trim_curve`]                           | `sm_CheckUVTrimCurve`                         |
//! | [`swap_face_uv`]                                  | `sm_SwapFaceUV`                               |
//! | [`mark_loops_to_delete`]                          | `sm_MarkLoopsToDelete`                        |
//! | [`rm_and_del_old_loop_vertices`]                  | `sm_RmAndDel_OldLoopVertices`                 |
//! | [`find_sector_of_face`]                           | `sm_FindSectorOfFace`                         |
//! | [`vote_for_region`], [`find_highest_vote`]        | `sm_VoteForRegion`, `sm_FindHighestVote`      |
//! | [`redistribute_seam_uv_trim_curves`]              | `sm_RedistributeSeamUVTrimCurves`             |
//! | `natural_seam_interval` (crate)                   | `sm_GetNaturalSeamInterval`                   |
//! | `is_first_nonseparating_seam` (crate)             | `sm_IsFirstNonseparatingSeam`, `sm_SeamIntervalLess` |
//! | [`add_attributes`]                                | `sm_AddAttributes`                            |
//! | [`PendingTopologyDelete`]                         | `SmPendingTopologyDelete` (`Clear`)           |
//! | [`make_vertex_split_uv_trim_curve`]               | `sm_MakeVertexSplitUVTrimCurve`               |
//! | [`ObjectMap`], [`set_object_map`], [`get_object_map`] | `sm_SetObjectMap`, `sm_GetObjectMap` (the copy maps are `topology.proto_topology_p1`'s [`CopyBrepMap`]) |
//! | [`reverse_reflection_edgeuses`]                   | `sm_ReverseReflectionEdgeuses`                |
//! | [`swap_reflection_shells`]                        | `sm_SwapReflectionShells`                     |
//! | [`find_side_of_curve`]                            | `sm_FindSideOfCurve`                          |
//!
//! Deviations:
//! - Out-params become return values; `SmTArray` outputs are `Vec`s; pointers are
//!   typed ids and `None` is `NULL`. Where upstream dereferences a null pointer the
//!   port returns [`SolidError::InvalidInput`]; `SM_ERR` returns are
//!   [`SolidError::Upstream`] with that code.
//! - The region list of the B-rep (`SmOwningTopology::m_pList`) is its recorded link
//!   order (taken whenever the head moves), then the regions appended since in arena
//!   order (`PostInsert`), rotated to [`Brep::region_list_head`]; `ResetInfiniteRegion`
//!   only moves the head. Regions not on the list (`Region::in_brep` false) are not
//!   reported by [`regions`].
//! - [`surfaces`] / [`curves`]: upstream's `GetOwner() == pFace` / `pEdge` test compares
//!   the geometry's owner with the face's / edge's
//!   [`arena_identity`] (the kernel's
//!   identity of arena entities; a B-rep whose geometry carries no owner reports no
//!   surfaces or curves, as upstream for unowned geometry); the attribute filter
//!   ([`GeometryAttributeFilter`]) looks the geometry up under its own keys.
//! - `SetThisModelSizeEstimate` / `SetThisLargeSmallSizeRatio` also write the B-rep's
//!   context (`m_cpContext`): the kernel's [`Context`](crate::geom::context::Context).
//! - Marks (`sm_SwapFaceUV`) are caller-owned
//!   [`Marks`] records, as the traverser.
//! - `sm_MarkLoopsToDelete` recurses once per nesting level of the loops and branches
//!   at every other loop of a face; the port stops with [`SolidError::InvalidInput`]
//!   past [`MAX_LOOP_NESTING`] levels and past [`MAX_LOOP_MARK_CALLS`] calls in all
//!   (upstream overflows its stack or hangs on a cyclic radial walk; real loop
//!   nestings are a handful).
//! - `sm_RmAndDel_OldLoopVertices` deletes two vertex uses: the port unlinks them from
//!   their vertex and resets their records (an arena record cannot be freed).
//! - `sm_SetObjectMap` / `sm_GetObjectMap` keep an [`EntityRef`] `BTreeMap`
//!   ([`ObjectMap`]) instead of the pointer hash map (plan hazard `ptr-hash`: upstream
//!   `SmMapPtrToPtr` iteration order is pointer order; only lookups are used here) and
//!   the ported [`CopyBrepMap`] for the optional copy maps.
//! - `SmPendingTopologyDelete`'s destructor work (detaching the pending object's
//!   attributes and deleting it) runs in `MakeTopologyFromData` (unit
//!   `topology.brep_p10`); this unit owns only `Clear`.
//! - `SmLockBrepBox` is a guard over `SmBrep::LockBoundingBox` / `UnLockBoundingBox`
//!   (unit `query.brep`).
//! - Statuses upstream does not check (`GetEnds`, point evaluations, `SetUVTrimCurve`,
//!   `Notify`, `ReverseParameterization`, `EditParameterization`) do not abort the port
//!   either: only [`SolidError::Unported`] propagates from them; a failed point
//!   evaluation leaves undefined coordinates (upstream leaves the `SmPoint3d` unset).
//!   Exception (#520): seam redistribution requires four edge uses, existing seam
//!   trims, successful neighbor trim creation and successful endpoint evaluations
//!   before classifying or swapping trims. Reflection helpers validate list members
//!   before editing and propagate list-edit errors.
//! - `sm_MakeVertexSplitUVTrimCurve` clones the face surface for the lift of the split
//!   point (upstream borrows it); `sm_FindSectorOfFace` returns the region as `Option`
//!   (upstream hands back the shell's region pointer unchecked).
//! - Calls into units not ported yet return [`SolidError::Unported`] (every site is
//!   tagged `UNPORTED(<unit>)`, one stub per call site with the code after it ported):
//!   `SmFace::FaceIntersect` (`topology.face_p3`), `SmBrep::CalculateBoundingBox`, `LockBoundingBox`,
//!   `UnLockBoundingBox` (`query.brep`).
//! - `CreateEdgeEdgeBlend` takes each edge use's B-rep (the second `None` when both are
//!   in one B-rep), mutably: the edge use evaluations of `topology.edgeuse_p3` take it so.
//!   It ignores both `EvaluateBinormal` statuses as upstream and goes on with the outputs
//!   upstream sees after a failure (undefined, or zeroed point, tangent and normal).
//! - `GetShells` dedups with a shell mark record local to the call (upstream marks the
//!   `SmShell` objects); `SetRegionIsVoidFlagsForNestedSolids` bounds its shell queue at
//!   `4 * shells + 16` passes.

use std::collections::BTreeMap;

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::a_object::sag_is_kind_of;
use crate::geom::attribute::{AttributeHandle, AttributeStore, ObjectKey};
use crate::geom::context::{with_mark_lock, MarkType, ALL_MARKS};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::tol;
use crate::heal::face_props::{HealerOpType, LoopObject};
use crate::heal::heal_data::p1::HealBrepTargets;
use crate::heal::heal_data::p2::HealData;
use crate::intersect::point_class::{ClassObject, ClassObjectKind};
use crate::kernel::Kernel;
use crate::math::{max2, Point2, Point3, Vec3, EFF_ZERO, EFF_ZERO_RAD, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::{BlendNeighbour, OrientType};
use crate::nurbs::knots::Side;
use crate::query::brep::BrepFaceServices;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::surface::surface::p2::{BlendEndType, CurvatureType, DerivSurfDefinition};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::topology::brep_cache::{arena_identity, CacheObject};
use crate::topology::edge::{self, AttributeFilter};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p2::BinormalEval;
use crate::topology::edgeuse::p3::{self as eu3, BinormalOptions};
use crate::topology::face::p1 as face1;
use crate::topology::face::p2 as face2;
use crate::topology::loopuse;
use crate::topology::proto_topology::p1::CopyBrepMap;
use crate::topology::topology::Topology;
use crate::topology::topology_traverser::{Entity, Marks};
use crate::topology::vertex;
use crate::topology::vertexuse;
use crate::topology::{
    Brep, EdgeId, EdgeuseId, FaceId, LoopId, NamingScheme, RegionId, ShellId, VertexId, Vertexuse,
    VertexuseId,
};

/// `SmBoolean` `TRUE` / `FALSE`.
const TRUE: i32 = 1;
const FALSE: i32 = 0;

const OP: &str = "topology::brep::p1";

/// `SmBrep::GetType`: `SmBrep_TYPE`.
pub const TYPE: i64 = 16011;
/// `SmBrep::GetTypeString`.
pub const TYPE_STRING: &str = "SmBrep_TYPE";
/// `SmBrep::GetClassString`.
pub const CLASS_STRING: &str = "SmBrep";

/// `SM_ERR_INVALID_INPUT`.
const SM_ERR_INVALID_INPUT: i32 = 1007;

/// Deepest loop nesting `sm_MarkLoopsToDelete` follows before failing (upstream
/// recurses without bound).
pub const MAX_LOOP_NESTING: u64 = 256;

/// `SmBrep::IsKindOf`: `SmBrep_TYPE` or a base class type (`SmSAGObject` and up).
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || sag_is_kind_of(t)
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

fn status(code: i32) -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code,
        entity: None,
    }
}

fn sm_err() -> SolidError {
    SolidError::sm_err(OP)
}

// ---- SmBrep members (SmBrep.h inline functions) -----------------------------------------

impl Brep {
    /// `GetThisModelSizeEstimate`: the B-rep's model size estimate; with
    /// `chase_defaults` (upstream default `TRUE`) a stored [`tol::USE_DEFAULT`] is
    /// replaced by the context's estimate (`m_cpContext->GetThisModelSizeEstimate()`,
    /// the kernel context).
    #[must_use]
    pub fn this_model_size_estimate(&self, k: &Kernel, chase_defaults: bool) -> f64 {
        if !chase_defaults {
            self.model_size_estimate
        } else if self.model_size_estimate == tol::USE_DEFAULT {
            k.context.this_model_size_estimate(k)
        } else {
            self.model_size_estimate
        }
    }

    /// `GetThisLargeSmallSizeRatio`: as [`Brep::this_model_size_estimate`] for the
    /// large/small size ratio (the kernel's system ratio when the context has none).
    #[must_use]
    pub fn this_large_small_size_ratio(&self, k: &Kernel, chase_defaults: bool) -> f64 {
        if !chase_defaults {
            self.large_small_size_ratio
        } else if self.large_small_size_ratio == tol::USE_DEFAULT {
            // SmContext::GetThisLargeSmallSizeRatio
            let ratio = k.context.stored_large_small_size_ratio();
            if ratio == tol::USE_DEFAULT {
                k.tolerances.large_small_size_ratio
            } else {
                ratio
            }
        } else {
            self.large_small_size_ratio
        }
    }

    /// `SetThisModelSizeEstimate` (upstream default `SM_USE_DEFAULT`): stores the
    /// estimate, passes it to the B-rep's context (the kernel context) and sets the
    /// B-rep tolerance to [`tol::size_estimate_to_zone_tol_3d`] of it; returns that
    /// tolerance.
    pub fn set_this_model_size_estimate(
        &mut self,
        k: &mut Kernel,
        model_size_estimate: f64,
    ) -> f64 {
        self.model_size_estimate = model_size_estimate;
        k.context.set_this_model_size_estimate(model_size_estimate);
        self.tolerance = tol::size_estimate_to_zone_tol_3d(self.model_size_estimate);
        self.tolerance
    }

    /// `SetThisLargeSmallSizeRatio` (upstream default `SM_USE_DEFAULT`): stores the
    /// ratio and passes it to the B-rep's context (the kernel context).
    pub fn set_this_large_small_size_ratio(&mut self, k: &mut Kernel, large_small_size_ratio: f64) {
        self.large_small_size_ratio = large_small_size_ratio;
        k.context
            .set_this_large_small_size_ratio(large_small_size_ratio);
    }

    /// `SetInfiniteRegion`: makes `region` the infinite region (the raw member write;
    /// [`reset_infinite_region`] does the bookkeeping).
    pub fn set_infinite_region(&mut self, region: Option<RegionId>) {
        self.set_infinite_region_record(region);
    }

    /// `m_pFaceListHead`: the faces in list order.
    #[must_use]
    pub fn face_list(&self) -> &[FaceId] {
        &self.face_list
    }

    /// `m_pEdgeListHead`: the edges in list order.
    #[must_use]
    pub fn edge_list(&self) -> &[EdgeId] {
        &self.edge_list
    }

    /// `m_pVertexListHead`: the vertices in list order.
    #[must_use]
    pub fn vertex_list(&self) -> &[VertexId] {
        &self.vertex_list
    }

    /// Reorders the face list (an importer's friend write): `order` must hold every
    /// face record exactly once.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] otherwise.
    pub fn set_face_list(&mut self, order: Vec<FaceId>) -> Result<()> {
        check_order(&order, self.faces.ids())?;
        self.face_list = order;
        Ok(())
    }

    /// Reorders the edge list (an importer's friend write): `order` must hold every
    /// edge record exactly once.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] otherwise.
    pub fn set_edge_list(&mut self, order: Vec<EdgeId>) -> Result<()> {
        check_order(&order, self.edges.ids())?;
        self.edge_list = order;
        Ok(())
    }

    /// Reorders the vertex list (an importer's friend write): `order` must hold every
    /// vertex record exactly once.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] otherwise.
    pub fn set_vertex_list(&mut self, order: Vec<VertexId>) -> Result<()> {
        check_order(&order, self.vertices.ids())?;
        self.vertex_list = order;
        Ok(())
    }

    /// The head of the B-rep's region list (`SmOwningTopology::m_pList`), `None` for
    /// the first region record.
    #[must_use]
    pub const fn region_list_head(&self) -> Option<RegionId> {
        self.region_list_head
    }

    /// Sets the head of the B-rep's region list (the friend write `m_pList = ..`). The
    /// circular link order is kept: later appends go before the new head.
    pub fn set_region_list_head(&mut self, region: Option<RegionId>) {
        let mut order = self.region_links();
        if let Some(pos) = region.and_then(|r| order.iter().position(|&x| x == r)) {
            order.rotate_left(pos);
        }
        self.region_order = order;
        self.region_list_head = region;
    }

    /// The regions on the list in link order from `Brep::region_order`'s start
    /// (not rotated to the head): the recorded order, then the appended regions.
    pub(crate) fn region_links(&self) -> Vec<RegionId> {
        let on = |r: RegionId| self.regions.get(r).is_some_and(|x| x.in_brep);
        let mut order: Vec<RegionId> = self
            .region_order
            .iter()
            .copied()
            .filter(|&r| on(r))
            .collect();
        for (id, r) in self.regions.iter() {
            if r.in_brep && !order.contains(&id) {
                order.push(id);
            }
        }
        order
    }

    /// Records the current link order (rotated to the head) as `Brep::region_order`.
    pub(crate) fn fix_region_order(&mut self) {
        let mut order = self.region_links();
        if let Some(pos) = self
            .region_list_head
            .and_then(|r| order.iter().position(|&x| x == r))
        {
            order.rotate_left(pos);
        }
        self.region_order = order;
    }

    /// `AddDeletedSurface`: keeps a surface removed from the B-rep until the operation
    /// ends (the list is created on first use upstream).
    pub fn add_deleted_surface(&mut self, surface: Surface) {
        self.deleted_surfaces.push(surface);
    }

    /// `m_pDeletedSurfaces`: the surfaces kept by [`Brep::add_deleted_surface`], in
    /// order.
    #[must_use]
    pub fn deleted_surfaces(&self) -> &[Surface] {
        &self.deleted_surfaces
    }
}

/// Whether `order` is a permutation of the live arena ids `live` (records removed by
/// `topology.brep_p4`'s deletions leave gaps in the id range).
fn check_order<I: crate::topology::ArenaId>(
    order: &[I],
    live: impl Iterator<Item = I>,
) -> Result<()> {
    let mut seen: BTreeMap<u32, bool> = live.map(|id| (id.index(), false)).collect();
    for id in order {
        match seen.get_mut(&id.index()) {
            Some(s) if !*s => *s = true,
            _ => return Err(null("list order is not a permutation of the records")),
        }
    }
    if order.len() != seen.len() {
        return Err(null("list order is not a permutation of the records"));
    }
    Ok(())
}

/// `GetNumFaces`: the face count (`m_pFaceListHead->GetSize()`).
#[must_use]
pub fn num_faces(b: &Brep) -> usize {
    b.face_list().len()
}

/// `IsLiveTopologyMember`: whether `target` is on the B-rep's face, edge or vertex
/// list (identity only; `None` is `FALSE`, as every other entity kind).
#[must_use]
pub fn is_live_topology_member(b: &Brep, target: Option<EntityRef>) -> bool {
    match target {
        Some(EntityRef::Face(f)) => b.face_list().contains(&f),
        Some(EntityRef::Edge(e)) => b.edge_list().contains(&e),
        Some(EntityRef::Vertex(v)) => b.vertex_list().contains(&v),
        _ => false,
    }
}

// ---- collections (SmOwningTopology::GetAll over the B-rep's lists) --------------------

fn filtered<I: Copy>(
    ids: impl Iterator<Item = I>,
    attribute: Option<&AttributeFilter<'_>>,
    entity: impl Fn(I) -> EntityRef,
) -> Vec<I> {
    match attribute {
        None => ids.collect(),
        Some(filter) => ids.filter(|&id| filter.matches(entity(id))).collect(),
    }
}

/// `GetRegions`: the B-rep's region list from its head ([`Brep::region_list_head`]),
/// those carrying `attribute` when given.
///
/// # Errors
///
/// [`SolidError::NotFound`] when the list head is a dangling id.
pub fn regions(b: &Brep, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<RegionId>> {
    let on_list = b.region_links();
    let start = match b.region_list_head() {
        None => 0,
        Some(head) => {
            b.region(head)?;
            // (upstream's head is on the list by construction)
            on_list
                .iter()
                .position(|&r| r == head)
                .ok_or_else(|| null("GetRegions: region list head not on the list"))?
        }
    };
    let rotated = on_list
        .iter()
        .skip(start)
        .chain(on_list.iter().take(start))
        .copied();
    Ok(filtered(rotated, attribute, EntityRef::Region))
}

/// `GetFaces`: every face in list order, those carrying `attribute` when given.
///
/// # Errors
///
/// None (the signature matches the other collections).
pub fn faces(b: &Brep, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<FaceId>> {
    Ok(filtered(
        b.face_list().iter().copied(),
        attribute,
        EntityRef::Face,
    ))
}

/// `GetEdges`: every edge in list order, those carrying `attribute` when given.
///
/// # Errors
///
/// None (the signature matches the other collections).
pub fn edges(b: &Brep, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<EdgeId>> {
    Ok(filtered(
        b.edge_list().iter().copied(),
        attribute,
        EntityRef::Edge,
    ))
}

/// The polylines of a B-rep's edges ([`tessellate_boundaries`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoundaryTessellation {
    /// Every edge's points, edge after edge (`rPoints`).
    pub points: Vec<Point3>,
    /// The number of points of each edge, `0` for one that failed (`rVertexCounts`).
    pub vertex_counts: Vec<usize>,
    /// The edges, in [`edges`] order (`rEdges`).
    pub edges: Vec<EdgeId>,
    /// Every edge was tessellated (`SM_SUCCESS`); `false` is upstream's `SM_ERR`, which
    /// comes with the other edges' points.
    pub complete: bool,
}

/// `TessellateBoundaries`: each edge's curve over the edge's interval as points
/// ([`Curve::tessellate`] with no chord height, `angle_tol_deg`, at least one segment),
/// in [`edges`] order. An edge without curve, whose tessellation fails or gives no
/// point, has no points and makes the result incomplete.
///
/// # Errors
///
/// `InvalidInput` for an angle tolerance that is not finite and positive (upstream:
/// `SM_ERR_INVALID_INPUT` with empty outputs); [`SolidError::Unported`] from a
/// tessellation.
pub fn tessellate_boundaries(
    k: &Kernel,
    b: &Brep,
    angle_tol_deg: f64,
) -> Result<BoundaryTessellation> {
    const OP: &str = "topology::brep::p1::tessellate_boundaries";
    if !angle_tol_deg.is_finite() || angle_tol_deg <= 0.0 {
        return Err(SolidError::invalid_input(
            OP,
            "angle tolerance must be finite and positive",
        ));
    }

    let mut out = BoundaryTessellation {
        edges: edges(b, None)?,
        complete: true,
        ..BoundaryTessellation::default()
    };
    for &e in &out.edges {
        let edge = b.edge(e)?;
        let points = match edge.curve.as_ref() {
            None => None,
            Some(curve) => match curve.tessellate(k, &edge.interval, 0.0, angle_tol_deg, 1) {
                Ok(t) if !t.points.is_empty() => Some(t.points),
                Err(
                    err @ (SolidError::Unported { .. } | SolidError::PreparationRequired { .. }),
                ) => return Err(err),
                Ok(_) | Err(_) => None,
            },
        };
        if let Some(points) = points {
            out.vertex_counts.push(points.len());
            out.points.extend(points);
        } else {
            out.complete = false;
            out.vertex_counts.push(0);
        }
    }
    Ok(out)
}

/// `GetVertices`: every vertex in list order, those carrying `attribute` when given.
///
/// # Errors
///
/// None (the signature matches the other collections).
pub fn vertices(b: &Brep, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<VertexId>> {
    Ok(filtered(
        b.vertex_list().iter().copied(),
        attribute,
        EntityRef::Vertex,
    ))
}

/// `GetShells`: the distinct shells of every region (`SmRegion::GetShells` under a new
/// mark, in region then shell order), those carrying `attribute` when given.
///
/// # Errors
///
/// [`SolidError::Unported`] from `SmRegion::GetShells` (unit `topology.shell`) for a
/// B-rep with regions; a B-rep without regions returns the empty list, as upstream
/// (the per-region loop never runs); the errors of the mark lock.
pub fn shells(
    k: &mut Kernel,
    b: &Brep,
    attribute: Option<&AttributeFilter<'_>>,
) -> Result<Vec<ShellId>> {
    // get regions
    let regions = regions(b, None)?;
    with_mark_lock(k, ALL_MARKS, |k, mark| -> Result<Vec<ShellId>> {
        let mut marked: BTreeMap<ShellId, Topology> = BTreeMap::new();
        let mut out = Vec::new();
        // for every region
        for r in regions {
            // get this region's shells
            for s in region_shells(b, r, attribute)? {
                // skip marked shells
                let rec = marked.entry(s).or_default();
                if rec.is_marked(&k.context, mark) {
                    continue;
                }
                // mark and add shell to output
                rec.mark(Some(&k.context), mark);
                out.push(s);
            }
        }
        Ok(out)
    })?
}

/// `GetWireEdges`: the wire edges of [`edges`] (`SmEdge::IsWire`).
///
/// # Errors
///
/// Those of [`edge::is_wire`].
pub fn wire_edges(b: &Brep, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<EdgeId>> {
    let mut out = Vec::new();
    // add wire edges to output array
    for e in edges(b, attribute)? {
        if edge::is_wire(b, e)? {
            out.push(e);
        }
    }
    Ok(out)
}

/// `GetTopologicalEdges`: the edges `SmEdge::IsTopological` (default gain) accepts:
/// those removable by merging their two faces. As upstream the coverage test may
/// expand a face's surface in place.
///
/// # Errors
///
/// Those of [`edge::is_topological`].
pub fn topological_edges(k: &Kernel, b: &mut Brep) -> Result<Vec<EdgeId>> {
    let mut out = Vec::new();
    for e in edges(b, None)? {
        if edge::is_topological(k, b, e, 1.0)?.is_topological {
            out.push(e);
        }
    }
    Ok(out)
}

/// `GetTopologicalVertices`: the vertices `SmVertex::IsTopologicalVertex` accepts:
/// those removable by merging their two edges.
///
/// # Errors
///
/// Those of [`vertex::is_topological_vertex`].
pub fn topological_vertices(k: &Kernel, b: &mut Brep) -> Result<Vec<VertexId>> {
    let mut out = Vec::new();
    for v in vertices(b, None)? {
        if vertex::is_topological_vertex(k, b, v)? {
            out.push(v);
        }
    }
    Ok(out)
}

/// The attribute filter of [`surfaces`] and [`curves`]: upstream tests the geometry's
/// own attributes (`pSurface->FindAttribute`, `pCurve->FindAttribute`), so the keys
/// name a face's surface and an edge's curve in `attributes`.
pub struct GeometryAttributeFilter<'a> {
    /// The attribute store.
    pub attributes: &'a AttributeStore,
    /// `*pOptAttributeId`.
    pub attribute_id: u64,
    /// The key of face `f`'s surface.
    pub surface_key_of: &'a dyn Fn(FaceId) -> ObjectKey,
    /// The key of edge `e`'s curve.
    pub curve_key_of: &'a dyn Fn(EdgeId) -> ObjectKey,
}

impl core::fmt::Debug for GeometryAttributeFilter<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GeometryAttributeFilter")
            .field("attribute_id", &self.attribute_id)
            .finish_non_exhaustive()
    }
}

/// The default expected owner of `f`'s surface under `b`'s recorded naming scheme,
/// falling back to a proven caller owner recorded on `b`.
pub(crate) fn default_face_owner(b: &Brep, f: FaceId) -> ObjectRef {
    let fallback = if b.naming() == NamingScheme::Notify {
        loopuse::notify_ref(EntityRef::Face(f))
    } else {
        arena_identity(CacheObject::Face(f))
    };
    let actual = b.face(f).ok().and_then(|x| x.surface.as_ref()?.owner());
    if actual != Some(fallback) {
        if let Some(proven) = b.geometry_owner_key(EntityRef::Face(f)) {
            if actual == Some(proven) {
                return proven;
            }
        }
    }
    fallback
}

/// The default expected owner of `e`'s curve under `b`'s recorded naming scheme,
/// falling back to a proven caller owner recorded on `b`.
pub(crate) fn default_edge_owner(b: &Brep, e: EdgeId) -> ObjectRef {
    let fallback = if b.naming() == NamingScheme::Notify {
        loopuse::notify_ref(EntityRef::Edge(e))
    } else {
        arena_identity(CacheObject::Edge(e))
    };
    let actual = b.edge(e).ok().and_then(|x| x.curve.as_ref()?.owner());
    if actual != Some(fallback) {
        if let Some(proven) = b.geometry_owner_key(EntityRef::Edge(e)) {
            if actual == Some(proven) {
                return proven;
            }
        }
    }
    fallback
}

/// `GetSurfaces`: the surfaces owned by their faces (`pSurface->GetOwner() == pFace`,
/// resolved in `b`'s recorded naming scheme), in face order, those carrying
/// `attribute` when given.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a face without surface (upstream dereferences
/// it).
pub fn surfaces<'b>(
    b: &'b Brep,
    attribute: Option<&GeometryAttributeFilter<'_>>,
) -> Result<Vec<&'b Surface>> {
    surface_owners(b, attribute, &|id| default_face_owner(b, id))?
        .into_iter()
        .map(|id| {
            b.face(id)?
                .surface
                .as_ref()
                .ok_or_else(|| null("GetSurfaces: face without surface"))
        })
        .collect()
}

/// The same native geometry collection with caller-supplied owner identities.
pub(crate) fn surface_owners(
    b: &Brep,
    attribute: Option<&GeometryAttributeFilter<'_>>,
    owner: &dyn Fn(FaceId) -> ObjectRef,
) -> Result<Vec<FaceId>> {
    let mut out = Vec::new();
    // for every face
    for f in faces(b, None)? {
        let surface = b
            .face(f)?
            .surface
            .as_ref()
            .ok_or_else(|| null("GetSurfaces: face without surface"))?;
        // add all standalone face->Surfaces to the list
        if surface.owner() == Some(owner(f))
            && attribute.is_none_or(|filter| {
                filter
                    .attributes
                    .find_attribute((filter.surface_key_of)(f), filter.attribute_id)
                    .is_some()
            })
        {
            out.push(f);
        }
    }
    Ok(out)
}

/// `GetCurves`: the 3D curves owned by their edges (`pCurve->GetOwner() == pEdge`,
/// resolved in `b`'s recorded naming scheme), in edge order, those carrying
/// `attribute` when given.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge without curve (upstream dereferences it).
pub fn curves<'b>(
    b: &'b Brep,
    attribute: Option<&GeometryAttributeFilter<'_>>,
) -> Result<Vec<&'b Curve>> {
    curve_owners(b, attribute, &|id| default_edge_owner(b, id))?
        .into_iter()
        .map(|id| {
            b.edge(id)?
                .curve
                .as_ref()
                .ok_or_else(|| null("GetCurves: edge without curve"))
        })
        .collect()
}

/// The same native geometry collection with caller-supplied owner identities.
pub(crate) fn curve_owners(
    b: &Brep,
    attribute: Option<&GeometryAttributeFilter<'_>>,
    owner: &dyn Fn(EdgeId) -> ObjectRef,
) -> Result<Vec<EdgeId>> {
    let mut out = Vec::new();
    for e in edges(b, None)? {
        let curve = b
            .edge(e)?
            .curve
            .as_ref()
            .ok_or_else(|| null("GetCurves: edge without curve"))?;
        if curve.owner() == Some(owner(e))
            && attribute.is_none_or(|filter| {
                filter
                    .attributes
                    .find_attribute((filter.curve_key_of)(e), filter.attribute_id)
                    .is_some()
            })
        {
            out.push(e);
        }
    }
    Ok(out)
}

/// `IsDegenerate`: whether every vertex lies within `tol_3d` of the first and the
/// B-rep's bounding box is point sized to `tol_3d`.
///
/// # Errors
///
/// [`SolidError::Unported`] when the vertex check passes: the bounding box is
/// `SmBrep::CalculateBoundingBox` (unit `query.brep`), then `SmExtent3d::IsPointSized`.
pub fn is_degenerate(k: &Kernel, b: &Brep, tol_3d: f64) -> Result<bool> {
    let _ = k;
    let tol_sq = tol_3d * tol_3d;
    let verts = vertices(b, None)?;

    // Do quick cheap check on Vertices
    if let Some(&first) = verts.first() {
        let p0 = b.vertex(first)?.point;
        for &v in verts.iter().skip(1) {
            if p0.distance_squared(b.vertex(v)?.point) > tol_sq {
                return Ok(false);
            }
        }
    }

    // If Brep might be degenerate - Do expensive full check (CalculateBoundingBox's
    // status is ignored: a failure leaves the default box)
    // CalculateBoundingBox(sBBox): header defaults bApprox = FALSE, bTight = FALSE,
    // bExpandByZoneTol3d = TRUE
    let bbox = ignored(qbrep::calculate_bounding_box(
        k,
        b,
        BoundingBoxOptions::default(),
    ))?
    .unwrap_or_default();
    Ok(bbox.is_point_sized(tol_3d))
}

// ---- infinite region ------------------------------------------------------------------

/// `SmSetRegionVoidType`: how `IsVoid` flags are set after the infinite region changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SetRegionVoidType {
    /// `SM_VS_NESTEDSOLID` (the default): a nested solid model
    /// ([`set_region_is_void_flags_for_nested_solids`]).
    #[default]
    NestedSolid,
    /// `SM_VS_INFINITEREGION_ONLY`: only the new infinite region becomes void.
    InfiniteRegionOnly,
    /// `SM_VS_INFINITEREGION_SWAP`: the old infinite region takes the new one's flag,
    /// the new one becomes void.
    InfiniteRegionSwap,
}

/// `ResetInfiniteRegion`: makes `new_infinite_region` (one of the B-rep's regions) the
/// infinite region and the head of the region list, then sets the `IsVoid` flags as
/// `void_setting` asks.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no region (`NULL`) or one not on the B-rep's list;
/// [`SolidError::Unported`] for the flag update (`SmRegion::SetIsVoid` / `IsVoid`,
/// unit `topology.shell`), after the region and list head have been changed. The
/// nested-solid update's statuses are ignored as upstream (only `Unported` and the
/// null-dereference `InvalidInput` propagate).
pub fn reset_infinite_region(
    k: &mut Kernel,
    b: &mut Brep,
    new_infinite_region: Option<RegionId>,
    void_setting: SetRegionVoidType,
) -> Result<()> {
    // No work: Null input
    let Some(new_infinite_region) = new_infinite_region else {
        return Err(status(SM_ERR_INVALID_INPUT));
    };

    // If given region is not one our our regions, fail.
    if !regions(b, None)?.contains(&new_infinite_region) {
        return Err(status(SM_ERR_INVALID_INPUT));
    }

    // Reset the infinite region, and make sure that the first in list is infinite region.
    let old_infinite_region = b.infinite_region();
    b.set_infinite_region(Some(new_infinite_region));
    b.set_region_list_head(Some(new_infinite_region));

    // manage IsVoid region bits
    match void_setting {
        SetRegionVoidType::NestedSolid => {
            // status ignored upstream; the port's null-dereference marker still propagates
            match set_region_is_void_flags_for_nested_solids(k, b) {
                Err(e @ (SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })) => {
                    return Err(e)
                }
                r => ignore_status(r)?,
            }
        }
        SetRegionVoidType::InfiniteRegionOnly => {
            region_set_is_void(b, new_infinite_region, TRUE)?;
        }
        SetRegionVoidType::InfiniteRegionSwap => {
            let Some(old) = old_infinite_region else {
                // upstream dereferences the (null) old infinite region here
                return Err(null(
                    "ResetInfiniteRegion swap without an old infinite region",
                ));
            };
            let new_is_void = region_is_void(b, new_infinite_region)?;
            region_set_is_void(b, old, new_is_void)?;
            region_set_is_void(b, new_infinite_region, TRUE)?;
        }
    }
    Ok(())
}

/// `SetRegionIsVoidFlagsForNestedSolids`: with the infinite region first on the
/// region list, marks it void and alternates the flag down every level of nested
/// shells (each nested region's outer shell becomes its list head).
///
/// # Errors
///
/// `SM_ERR` when the infinite region is not identified or not the list head;
/// [`SolidError::Unported`] from the region and shell calls (`SmRegion::SetIsVoid`,
/// `IsVoid`, `GetShells`, `SetList`, `SmShell::GetMatedShells`: their data
/// is unit `topology.shell`'s); [`SolidError::InvalidInput`] where upstream
/// dereferences null, and past `4 * shells + 16` queue passes.
pub fn set_region_is_void_flags_for_nested_solids(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    // check state - look for a properly labeled infinite region
    let Some(infinite) = b.infinite_region() else {
        return Err(sm_err());
    };
    if regions(b, None)?.first() != Some(&infinite) {
        return Err(sm_err());
    }

    // mark the infinite region as void
    region_set_is_void(b, infinite, TRUE)?;

    // get the infinite region's shells
    let mut queue = region_shells(b, infinite, None)?;

    // for every shell in the queue (bounded: a well-formed nesting visits each shell
    // once)
    let bound = b.shells().len().saturating_mul(4).saturating_add(16);
    let mut passes = 0usize;
    while !queue.is_empty() {
        passes += 1;
        if passes > bound {
            return Err(null(
                "SetRegionIsVoidFlagsForNestedSolids: shell nesting does not terminate",
            ));
        }
        let shell = queue.remove(0);
        let region = shell_region(b, shell)?
            .ok_or_else(|| null("SetRegionIsVoidFlagsForNestedSolids: shell without region"))?;

        // nested regions get the opposite flag (`!pRegion->IsVoid()` on the raw SmBoolean:
        // UNSURE (2) gives FALSE)
        let nested_is_void = i32::from(region_is_void(b, region)? == FALSE);

        // the shells mated to this one (increments an unused mark value)
        // (status ignored upstream: a failed call is read as no mated shells)
        for mated in ignored(shell_mated_shells(k, b, shell))?.unwrap_or_default() {
            let nested = shell_region(b, mated)?.ok_or_else(|| {
                null("SetRegionIsVoidFlagsForNestedSolids: mated shell without region")
            })?;
            region_set_is_void(b, nested, nested_is_void)?;

            // the mated shell is the nested region's outer shell
            region_set_list(b, nested, mated)?;

            // add all inner shells to the queue (the 1st shell is the outer shell)
            let mut nested_shells = region_shells(b, nested, None)?;
            // RemoveAt(0) of an empty array leaves it empty in the release build
            // (SM_ASSERT_BREAK is a no-op)
            if !nested_shells.is_empty() {
                nested_shells.remove(0);
            }
            queue.extend(nested_shells);
        }
    }
    Ok(())
}

// ---- region votes (sm_VoteForRegion, sm_FindHighestVote) -------------------------------

/// `sm_VoteForRegion`: adds a vote for `new_region` (appending it with one vote when
/// it is not listed yet). A null region (`None`, the result of a failed
/// `sm_FindSectorOfFace`) is a region of its own, as upstream.
pub fn vote_for_region(
    regions: &mut Vec<Option<RegionId>>,
    votes: &mut Vec<u64>,
    new_region: Option<RegionId>,
) {
    for (i, r) in regions.iter().enumerate() {
        if *r == new_region {
            if let Some(v) = votes.get_mut(i) {
                *v = v.saturating_add(1);
            }
            return;
        }
    }
    regions.push(new_region);
    votes.push(1);
}

/// `sm_FindHighestVote`: the first region with the most votes; `None` for no regions
/// and when the null region wins (upstream returns `NULL` in both cases).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when `votes` is shorter than `regions` (upstream reads
/// past the array).
pub fn find_highest_vote(regions: &[Option<RegionId>], votes: &[u64]) -> Result<Option<RegionId>> {
    if votes.len() < regions.len() {
        return Err(null("sm_FindHighestVote: fewer votes than regions"));
    }
    let mut highest = 0usize;
    for i in 1..regions.len() {
        if votes.get(highest) < votes.get(i) {
            highest = i;
        }
    }
    Ok(regions.get(highest).copied().flatten())
}

// ---- copy maps (sm_SetObjectMap, sm_GetObjectMap) ---------------------------------------

/// The `SmMapPtrToPtr<SmTopology, SmTopology>` of the B-rep copy: original object to
/// its copy (`None` is a null copy).
pub type ObjectMap = BTreeMap<EntityRef, Option<EntityRef>>;

/// `sm_SetObjectMap`: records `orig -> copy`, and in the optional `SmCopyBrepMap`s
/// ([`CopyBrepMap`], unit `topology.proto_topology_p1`) `copy -> orig` and `orig ->
/// copy`. A null copy is recorded only in `map`: upstream passes it to
/// `SmCopyBrepMap::SetAt`, which dereferences it (UB-98).
pub fn set_object_map(
    orig: EntityRef,
    copy: Option<EntityRef>,
    map: &mut ObjectMap,
    copy_brep_map: Option<&mut CopyBrepMap<EntityRef>>,
    reverse_map: Option<&mut CopyBrepMap<EntityRef>>,
) {
    // Use a map object track relationships.
    map.insert(orig, copy);
    if let (Some(m), Some(c)) = (copy_brep_map, copy) {
        m.set_at(c, orig);
    }
    if let (Some(m), Some(c)) = (reverse_map, copy) {
        m.set_at(orig, c);
    }
}

/// `sm_GetObjectMap`: the copy recorded for `from` (`None` when none, or a null copy).
#[must_use]
pub fn get_object_map(from: EntityRef, map: &ObjectMap) -> Option<EntityRef> {
    map.get(&from).copied().flatten()
}

// ---- attributes (sm_AddAttributes, SmPendingTopologyDelete) -----------------------------

/// `sm_AddAttributes`: adds the attributes `attributes[i]` for every index `i` of
/// `indexed_attributes` to the object `obj`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an index past `attributes` (upstream reads past the
/// array); those of [`AttributeStore::add_attribute`].
pub fn add_attributes(
    store: &mut AttributeStore,
    obj: ObjectKey,
    indexed_attributes: &[usize],
    attributes: &[AttributeHandle],
) -> Result<()> {
    // for every Attribute index
    for &index in indexed_attributes {
        // get attribute
        let attr = attributes
            .get(index)
            .copied()
            .ok_or_else(|| null("sm_AddAttributes: attribute index out of range"))?;
        // add attribute to object
        store.add_attribute(obj, attr)?;
    }
    Ok(())
}

/// `SmPendingTopologyDelete`: the topology object `MakeTopologyFromData` must delete
/// (after detaching its borrowed attributes) if it exits early; [`Self::clear`]
/// releases it once it is linked into the B-rep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingTopologyDelete {
    pending: Option<EntityRef>,
}

impl PendingTopologyDelete {
    /// `SmPendingTopologyDelete(pTopology)`.
    #[must_use]
    pub const fn new(topology: Option<EntityRef>) -> Self {
        Self { pending: topology }
    }

    /// `Clear`: the object is linked in; nothing is deleted.
    pub fn clear(&mut self) {
        self.pending = None;
    }

    /// The object still pending deletion (`m_pTopology`), read by the destructor
    /// (unit `topology.brep_p10`).
    #[must_use]
    pub const fn pending(&self) -> Option<EntityRef> {
        self.pending
    }
}

// ---- reflection helpers (sm_ReverseReflectionEdgeuses, sm_SwapReflectionShells) ---------

/// `sm_ReverseReflectionEdgeuses`: swaps the members of every mated pair of the edge's
/// edge use list (`[a0 a1 b0 b1 ..]` becomes `[a1 a0 b1 b0 ..]`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for an odd
/// edge use count (upstream reads past the array) or inconsistent list membership.
pub fn reverse_reflection_edgeuses(b: &mut Brep, e: EdgeId) -> Result<()> {
    let uses = b.edge(e)?.edgeuses.clone();
    if uses.len() % 2 != 0 {
        return Err(null("sm_ReverseReflectionEdgeuses: odd edge use count"));
    }
    for &eu in &uses {
        if b.edgeuse(eu)?.edge != Some(e) {
            return Err(null(
                "sm_ReverseReflectionEdgeuses: inconsistent edge use owner",
            ));
        }
    }
    let mut relinked = Vec::with_capacity(uses.len());
    for pair in uses.chunks_exact(2) {
        if let [first, second] = *pair {
            relinked.push(second);
            relinked.push(first);
        }
    }
    b.edge_mut(e)?.edgeuses = relinked;
    Ok(())
}

/// `sm_SwapReflectionShells`: when the face's two face uses sit in different shells,
/// exchanges the shells they belong to.
///
/// Upstream removes each face use from its shell's list and appends it to the other's
/// (`Remove` / `PostInsert`): both ordered member lists and sibling links follow the
/// new owners. `Face::shell` and `Shell::faces` have no upstream counterpart
/// (`SmFace` holds no shell; the arena builders fill them, the import leaves
/// `Shell::faces` empty): they follow the first face use, so the face moves to the
/// other shell's list.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (a face without two face uses, a face use without
/// shell) or inconsistent shell membership. Invalid lists are rejected before
/// changing either owner.
pub fn swap_reflection_shells(b: &mut Brep, f: FaceId) -> Result<()> {
    let (first, second) = face2::faceuses(b, f)?;
    let first = first.ok_or_else(|| null("sm_SwapReflectionShells: face without face use"))?;
    let second = second.ok_or_else(|| null("sm_SwapReflectionShells: face use without mate"))?;
    let a = eu1::faceuse_shell(b, first)?
        .ok_or_else(|| null("sm_SwapReflectionShells: face use without shell"))?;
    let bb = eu1::faceuse_shell(b, second)?
        .ok_or_else(|| null("sm_SwapReflectionShells: mate without shell"))?;
    if a != bb {
        // Check both lists before the first edit, including references that the
        // sibling-link refresh will visit. Otherwise a later failed edit could
        // leave the first face use in the wrong shell (#520).
        for (shell, member) in [(a, first), (bb, second)] {
            let members = b.shell(shell)?.get_faceuses()?;
            if !members.contains(&member) {
                return Err(null("sm_SwapReflectionShells: face use missing from shell"));
            }
            for id in members {
                if b.faceuse(id)?.shell != Some(shell) {
                    return Err(null("sm_SwapReflectionShells: inconsistent face use owner"));
                }
            }
        }
        crate::topology::shell::move_faceuse(b, first, bb)?;
        crate::topology::shell::move_faceuse(b, second, a)?;
        // the face record follows its first face use: move it to the end of the other
        // shell's face list (PostInsert appends)
        b.shell_mut(a)?.faces.retain(|&x| x != f);
        b.shell_mut(bb)?.faces.retain(|&x| x != f);
        b.shell_mut(bb)?.faces.push(f);
        b.face_mut(f)?.shell = bb;
    }
    Ok(())
}

// ---- loop vertices (sm_RmAndDel_OldLoopVertices) ----------------------------------------

/// `SmTopology::GetNext` / `GetLast` of a vertex use within its vertex's circular
/// vertex use list.
fn vertexuse_neighbour(b: &Brep, vu: VertexuseId, next: bool) -> Result<Option<VertexuseId>> {
    let Some(v) = b.vertexuse(vu)?.vertex else {
        return Ok(None);
    };
    let list = &b.vertex(v)?.vertexuses;
    let Some(i) = list.iter().position(|&x| x == vu) else {
        return Ok(None);
    };
    let n = list.len();
    let j = if next { (i + 1) % n } else { (i + n - 1) % n };
    Ok(list.get(j).copied())
}

/// The loop of a vertex use's loop use (`GetLoopuse()->GetLoop()`).
fn vertexuse_loop(b: &Brep, vu: VertexuseId) -> Result<Option<LoopId>> {
    let lu = vertexuse::loopuse(b, vu)?
        .ok_or_else(|| null("sm_RmAndDel_OldLoopVertices: loop vertex use without loop use"))?;
    loopuse::loop_(b, lu)
}

/// `sm_RmAndDel_OldLoopVertices`: when `vu` is a loop vertex use, finds its partner
/// (the next, else the previous, loop vertex use on the same loop in the vertex's
/// list), removes both from the vertex and deletes them (their records are reset).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; `SM_ERR_NULL_POINTER` for a vertex use
/// without list neighbour (`NER`); [`SolidError::InvalidInput`] where upstream
/// dereferences null (no loop use, no vertex); `SM_ERR` when no partner is found.
pub fn rm_and_del_old_loop_vertices(b: &mut Brep, vu: VertexuseId) -> Result<()> {
    // when pVertexuse is a LoopVertexuse
    if !vertexuse::is_loop_vertexuse(b, vu)? {
        return Ok(());
    }
    let this_loop = vertexuse_loop(b, vu)?;
    let partner = |b: &Brep, other: VertexuseId| -> Result<bool> {
        Ok(vertexuse::is_loop_vertexuse(b, other)? && vertexuse_loop(b, other)? == this_loop)
    };

    let other = vertexuse_neighbour(b, vu, true)?.ok_or_else(|| SolidError::null_pointer(OP))?; // NER

    // When OtherVU is a LoopVertexuse attached to the same Loop as input VU
    let del = if partner(b, other)? {
        other
    } else {
        // try the VU in the other direction of the Vertex's doubly linked Vertexuse list
        let other =
            vertexuse_neighbour(b, vu, false)?.ok_or_else(|| SolidError::null_pointer(OP))?; // NER
        if partner(b, other)? {
            other
        } else {
            // Unable to find the partner of this loop vertexuse
            return Err(sm_err());
        }
    };

    // remove the Tgt LoopVertexuse and its mated pair from their Vertex
    let v = b
        .vertexuse(vu)?
        .vertex
        .ok_or_else(|| null("sm_RmAndDel_OldLoopVertices: vertex use without vertex"))?;
    b.vertex_mut(v)?.vertexuses.retain(|&x| x != vu && x != del);

    // delete the LoopVertexuses
    *b.vertexuse_mut(vu)? = Vertexuse::default();
    *b.vertexuse_mut(del)? = Vertexuse::default();
    Ok(())
}

// ---- loop marking (sm_MarkLoopsToDelete) ------------------------------------------------

/// `sm_MarkLoopsToDelete`: given a 'big' face made from a surface's natural trim and
/// the inner topology to keep, appends to `loops_to_delete` the loops whose nesting
/// level (`rec_level`, even levels) marks them for deletion, following each other loop
/// of the face across its first edge's radial neighbour.
///
/// # Errors
///
/// `SM_ERR` for an edge that is a wire, lamina or not manifold; [`SolidError::NotFound`]
/// for a dangling id; [`SolidError::InvalidInput`] where upstream dereferences null or
/// reads an empty edge list, past [`MAX_LOOP_NESTING`] levels and past
/// [`MAX_LOOP_MARK_CALLS`] calls in all (the breadth bound).
pub fn mark_loops_to_delete(
    b: &Brep,
    loops_to_delete: &mut Vec<LoopId>,
    loop_: LoopId,
    rec_level: u64,
) -> Result<()> {
    let mut calls = 0u64;
    mark_loops_to_delete_bounded(b, loops_to_delete, loop_, rec_level, &mut calls)
}

/// The bound on the total number of `sm_MarkLoopsToDelete` calls of one marking: the
/// recursion is bounded in depth by [`MAX_LOOP_NESTING`] but branches at every other
/// loop of a face, so a radial chain that cycles through faces with three or more
/// loops would otherwise branch without bound (upstream hangs there).
pub const MAX_LOOP_MARK_CALLS: u64 = 1 << 20;

fn mark_loops_to_delete_bounded(
    b: &Brep,
    loops_to_delete: &mut Vec<LoopId>,
    loop_: LoopId,
    rec_level: u64,
    calls: &mut u64,
) -> Result<()> {
    if rec_level > MAX_LOOP_NESTING {
        return Err(null("sm_MarkLoopsToDelete: loop nesting too deep"));
    }
    *calls += 1;
    if *calls > MAX_LOOP_MARK_CALLS {
        return Err(null("sm_MarkLoopsToDelete: too many loop visits"));
    }
    let face = loopuse::loop_face(b, loop_)?
        .ok_or_else(|| null("sm_MarkLoopsToDelete: loop without face"))?;
    let even_level = rec_level % 2 == 0;

    for l in face2::loops(b, face, false, None)? {
        if l == loop_ {
            continue;
        }
        let edges = loopuse::loop_edges(b, l)?;
        let e = edges
            .first()
            .copied()
            .ok_or_else(|| null("sm_MarkLoopsToDelete: loop without edges"))?
            .ok_or_else(|| null("sm_MarkLoopsToDelete: edge use without edge"))?;
        if edge::is_wire(b, e)? {
            return Err(sm_err());
        }
        if edge::is_lamina(b, e)? {
            return Err(sm_err());
        }
        if !edge::is_manifold(b, e)? {
            return Err(sm_err());
        }

        let eu = edge::primary_edgeuse(b, e)?
            .ok_or_else(|| null("sm_MarkLoopsToDelete: edge without edge use"))?; // may not belong to pF
        let eu_rad = eu1::radial(b, eu)?
            .ok_or_else(|| null("sm_MarkLoopsToDelete: edge use without radial"))?;
        let loop_of = |eu: EdgeuseId| -> Result<LoopId> {
            let lu = eu1::loopuse(b, eu)?
                .ok_or_else(|| null("sm_MarkLoopsToDelete: edge use without loop use"))?;
            loopuse::loop_(b, lu)?
                .ok_or_else(|| null("sm_MarkLoopsToDelete: loop use without loop"))
        };
        let mut rad_l = loop_of(eu_rad)?;
        if loopuse::loop_face(b, rad_l)? == Some(face) {
            rad_l = loop_of(eu)?;
        }
        mark_loops_to_delete_bounded(b, loops_to_delete, rad_l, rec_level + 1, calls)?;
        // recursion level
    }

    if even_level {
        loops_to_delete.push(loop_);
    }
    Ok(())
}

// ---- face UV swap (sm_SwapFaceUV) --------------------------------------------------------

/// `sm_SwapFaceUV`: flips the face's normal (`SmFace::SwapUV`) unless its `mark` is
/// set, and sets the mark. `SwapUV`'s failing status is ignored (upstream's release
/// `SE`), so the face is marked whatever the swap did.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling face; [`SolidError::Unported`] from
/// `SwapUV`'s unported callees; [`SolidError::InvalidInput`] where `SwapUV`
/// dereferences null upstream (a face without surface or face use).
pub fn swap_face_uv(
    k: &Kernel,
    b: &mut Brep,
    marks: &mut Marks,
    face: FaceId,
    mark: MarkType,
) -> Result<()> {
    b.face(face)?;
    // skip faces already marked
    if marks.is_marked(k, Entity::Face(face), mark) {
        return Ok(());
    }
    // swap and mark the face (SE: the status is ignored). The port's InvalidInput marks
    // a null pointer upstream's SwapUV dereferences (undefined there, not a status): it
    // propagates, as this unit's own null dereferences.
    match face1::swap_uv(k, b, face) {
        // face_p1's own InvalidInput is its null-dereference marker; a callee's
        // InvalidInput is that callee's SM_ERR_INVALID_INPUT status, ignored as SE
        Err(SolidError::InvalidInput {
            operation: "topology::face::p1",
            ..
        }) => {
            return Err(SolidError::invalid_input(
                OP,
                "sm_SwapFaceUV: SmFace::SwapUV dereferences a null pointer",
            ));
        }
        r => ignore_status(r)?,
    }
    marks.mark(k, Entity::Face(face), mark);
    Ok(())
}

// ---- UV trim curve check (sm_CheckUVTrimCurve) -----------------------------------------

/// The vertex of an edge use's vertex use (`GetVertexuse()->GetVertex()`).
fn start_vertex(b: &Brep, eu: EdgeuseId) -> Result<VertexId> {
    let vu = eu1::vertexuse(b, eu)?.ok_or_else(|| null("edge use without vertex use"))?;
    b.vertexuse(vu)?
        .vertex
        .ok_or_else(|| null("vertex use without vertex"))
}

/// `sm_CheckUVTrimCurve`: checks the edge use's own UV trim curve (not its mate's):
/// at both ends the surface point of the trim curve must be within the face tolerance
/// plus the vertex tolerance of the vertex, and within the face tolerance plus the
/// edge tolerance of the edge curve's end. Nothing is checked without a trim curve.
///
/// # Errors
///
/// `SM_ERR` when a distance is too large; [`SolidError::InvalidInput`] where upstream
/// dereferences null (no edge, curve, mate, vertex use, vertex, face or surface);
/// those of the evaluations.
pub fn check_uv_trim_curve(k: &Kernel, b: &Brep, eu: EdgeuseId) -> Result<()> {
    let _ = k;
    let Some(trim_crv) = eu1::uv_trim_curve_pointer(b, eu)? else {
        return Ok(());
    };

    // 'Start' will be the start of pTrimCrv, same as start of Edge and Edge curve.
    let reverse = eu1::orientation(b, eu)? == OrientType::Opposite;
    let mate = eu1::mate(b, eu)?;

    // Trim curve should start and end right at Edge ends. (GetEnds and the point
    // evaluations below are not status-checked upstream: a failure leaves the points
    // unset; the port uses undefined coordinates there.)
    let (start_uv, end_uv) =
        ignored(trim_crv.ends(k))?.map_or((Point3::UNDEF, Point3::UNDEF), |e| (e.start, e.end));

    let e = eu1::edge(b, eu)?.ok_or_else(|| null("sm_CheckUVTrimCurve: edge use without edge"))?;
    let edge_ivl = edge::interval(b, e)?;
    let curve = b
        .edge(e)?
        .curve
        .as_ref()
        .ok_or_else(|| null("sm_CheckUVTrimCurve: edge without curve"))?;
    let edge_3d_start =
        ignored(curve.evaluate_point(k, edge_ivl.evaluate(0.0)))?.unwrap_or(Point3::UNDEF);

    let mate_vertex = |b: &Brep| -> Result<VertexId> {
        start_vertex(
            b,
            mate.ok_or_else(|| null("sm_CheckUVTrimCurve: edge use without mate"))?,
        )
    };
    let start_vtx = if reverse {
        mate_vertex(b)?
    } else {
        start_vertex(b, eu)?
    };
    let vrtx_3d_start = vertex::point(b, start_vtx)?;

    let face =
        eu1::face(b, eu)?.ok_or_else(|| null("sm_CheckUVTrimCurve: edge use without face"))?;
    let surf = face2::surface(b, face)?
        .ok_or_else(|| null("sm_CheckUVTrimCurve: face without surface"))?;
    let face_3d_start = ignored(surf.evaluate_point(k, Point2::new(start_uv.x, start_uv.y)))?
        .unwrap_or(Point3::UNDEF);

    let face_tol = face2::tolerance(b, face)?;
    let edge_tol = edge::tolerance(b, e)?;
    let mut vrtx_tol = vertex::tolerance(b, start_vtx)?;

    let dist_face_vrtx_start = face_3d_start.distance(vrtx_3d_start);
    if dist_face_vrtx_start > face_tol + vrtx_tol {
        return Err(sm_err());
    }
    let dist_face_edge_start = face_3d_start.distance(edge_3d_start);
    if dist_face_edge_start > face_tol + edge_tol {
        return Err(sm_err());
    }

    // Now the other end:
    let edge_3d_end =
        ignored(curve.evaluate_point(k, edge_ivl.evaluate(1.0)))?.unwrap_or(Point3::UNDEF);
    let end_vtx = if reverse {
        start_vertex(b, eu)?
    } else {
        mate_vertex(b)?
    };
    let vrtx_3d_end = vertex::point(b, end_vtx)?;
    let face_3d_end =
        ignored(surf.evaluate_point(k, Point2::new(end_uv.x, end_uv.y)))?.unwrap_or(Point3::UNDEF);
    vrtx_tol = vertex::tolerance(b, end_vtx)?; // have to update this to End vtx.

    let dist_face_vrtx_end = face_3d_end.distance(vrtx_3d_end);
    if dist_face_vrtx_end > face_tol + vrtx_tol {
        return Err(sm_err());
    }
    let dist_face_edge_end = face_3d_end.distance(edge_3d_end);
    if dist_face_edge_end > face_tol + edge_tol {
        return Err(sm_err());
    }
    Ok(())
}

// ---- bounding box lock (SmLockBrepBox) --------------------------------------------------

/// `SmLockBrepBox`: locks the B-rep's bounding box for its lifetime
/// (`SmBrep::LockBoundingBox` on construction, `UnLockBoundingBox` on drop).
#[derive(Debug)]
pub struct LockBrepBox<'b> {
    brep: &'b mut Brep,
}

impl<'b> LockBrepBox<'b> {
    /// `SmLockBrepBox(pBrep)`: `LockBoundingBox()` (default `bTight = FALSE`), whose
    /// status the constructor cannot report (a failure leaves the box unlocked).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the box computation.
    pub fn new(k: &Kernel, brep: &'b mut Brep) -> Result<Self> {
        ignore_status(crate::query::brep::lock_bounding_box(k, brep, false))?;
        Ok(Self { brep })
    }

    /// The locked B-rep.
    #[must_use]
    pub fn brep(&mut self) -> &mut Brep {
        self.brep
    }
}

impl Drop for LockBrepBox<'_> {
    fn drop(&mut self) {
        crate::query::brep::unlock_bounding_box(self.brep);
    }
}

// ---- calls into unit topology.edgeuse_p3 ------------------------------------------------

/// `SmEdgeuse::NormalizedEvaluate(t, bParameterSpaceEval, rPoint, pOptFirstDeriv)`
/// (`topology::edgeuse::p3::normalized_evaluate`; header default
/// `bOKToMakeUVTrimCurve = TRUE`): `rPoint`. `want_first_deriv` is
/// `pOptFirstDeriv != NULL` (the derivative itself is unused by every caller here).
fn normalized_evaluate(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    parameter_space_eval: bool,
    want_first_deriv: bool,
) -> Result<Point3> {
    Ok(eu3::normalized_evaluate(k, b, eu, t, parameter_space_eval, want_first_deriv, true)?.point)
}

/// `SmRegion::GetShells(rShells, pOptAttributeId)` (`Region::get_shells`, unit
/// `topology.shell`): the region's shell list, those carrying `attribute` when given.
fn region_shells(
    b: &Brep,
    region: RegionId,
    attribute: Option<&AttributeFilter<'_>>,
) -> Result<Vec<ShellId>> {
    b.region(region)?
        .get_shells(attribute.map(|f| f.attribute_id), |shell, _| {
            Ok(attribute.is_some_and(|f| f.matches(EntityRef::Shell(shell))))
        })
}

/// `SmRegion::SetIsVoid(flag)` (`Region::set_is_void`): the raw `SmBoolean` flag.
fn region_set_is_void(b: &mut Brep, region: RegionId, is_void: i32) -> Result<()> {
    b.region_mut(region)?.set_is_void(is_void);
    Ok(())
}

/// `SmRegion::IsVoid()` (`Region::is_void`): the raw `SmBoolean` flag (2 is `UNSURE`).
fn region_is_void(b: &Brep, region: RegionId) -> Result<i32> {
    Ok(b.region(region)?.is_void())
}

/// `SmRegion::SetList(shell)` (`SmOwningTopology::SetList`, a header inline): makes
/// `shell` the head of the region's circular shell list (a rotation of the typed list).
fn region_set_list(b: &mut Brep, region: RegionId, shell: ShellId) -> Result<()> {
    let shells = &mut b.region_mut(region)?.shells;
    let at = shells
        .iter()
        .position(|&s| s == shell)
        .ok_or_else(|| null("SmRegion::SetList: the shell is not on the region's list"))?;
    shells.rotate_left(at);
    Ok(())
}

/// `SmShell::GetRegion()`: the shell's `m_pListOwner` (co-ported as `Shell::region`).
fn shell_region(b: &Brep, shell: ShellId) -> Result<Option<RegionId>> {
    Ok(b.shell(shell)?.region)
}

/// `SmShell::GetMatedShells(rShells)` (`Shell::mated_shells`, unit `topology.shell`:
/// claims a mark).
fn shell_mated_shells(k: &mut Kernel, b: &Brep, shell: ShellId) -> Result<Vec<ShellId>> {
    b.shell(shell)?.mated_shells(k, b)
}

/// `SmEdgeuse::GetOrCreateUVTrimCurve(rpUVTrimCurve, pdOptDistToSurf, pdOptDeviation,
/// bUVSpaceOkay)` (`topology::edgeuse::p3::get_or_create_uv_trim_curve`; header defaults
/// `NULL`, `NULL`, `TRUE`): the returned status, `None` for `SM_SUCCESS`. The curve
/// upstream hands back in `rpUVTrimCurve` (also alongside a failure status) is the edge
/// use's or its mate's attached one afterwards, [`eu1::uv_trim_curve`]. `want_dist` /
/// `want_deviation` are `pdOpt... != NULL`. `Err` only where upstream has no status
/// (`Unported`, dangling ids, null dereferences).
fn get_or_create_uv_trim_curve(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    want_dist: bool,
    want_deviation: bool,
    uv_space_okay: bool,
) -> Result<Option<SolidError>> {
    Ok(
        eu3::get_or_create_uv_trim_curve(k, b, eu, want_dist, want_deviation, uv_space_okay)?
            .status
            .err(),
    )
}

/// `SmEdgeuse::EvaluateBinormal(dParameter, bUse3DOnly, rBinormalPoint, rBinormal,
/// pOptEdgeuseTangent, pOptFaceuseNormal)` (`topology::edgeuse::p3::evaluate_binormal`;
/// header defaults `pOptUVParameter = pOptUVDir = NULL`).
fn evaluate_binormal(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    parameter: f64,
    use_3d_only: bool,
) -> Result<BinormalEval> {
    eu3::evaluate_binormal(
        k,
        b,
        eu,
        parameter,
        use_3d_only,
        &BinormalOptions::default(),
    )
}

/// `EvaluateBinormal(dParameter, TRUE, rPos, rBinormal, &rTangent, &rNormal)` with its
/// status ignored, as `CreateEdgeEdgeBlend` calls it: the outputs upstream goes on with.
/// The out-params start as `SM_UNDEF_DOUBLE` (`SmVector3d`'s default). `EvaluateBinormal`
/// writes them all only on success; its one status before its "Init outputs" step (not
/// a loop edge use) leaves them undefined, every later one leaves the point, tangent and
/// normal zeroed and the binormal undefined. `Err` for [`SolidError::Unported`] and for
/// `topology.edgeuse_p3`'s own [`SolidError::InvalidInput`] (an upstream null dereference).
fn evaluate_binormal_status_ignored(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    parameter: f64,
) -> Result<BinormalEval> {
    let undefined = BinormalEval {
        point: Vec3::UNDEF,
        binormal: Vec3::UNDEF,
        tangent: Vec3::UNDEF,
        face_normal: Vec3::UNDEF,
        uv: Point2::UNDEF,
    };
    if !eu1::is_loop_edgeuse(b, eu)? {
        return Ok(undefined); // SER(SM_ERR_INVALID_INPUT) before the outputs are set
    }
    match evaluate_binormal(k, b, eu, parameter, true) {
        Ok(eval) => Ok(eval),
        Err(e) if e.is_unported() => Err(e),
        Err(
            e @ SolidError::InvalidInput {
                operation: "topology::edgeuse::p3",
                ..
            },
        ) => Err(e),
        // a status after "Init outputs": point, tangent and normal set to zero
        Err(_) => Ok(BinormalEval {
            point: Vec3::new(0.0, 0.0, 0.0),
            tangent: Vec3::new(0.0, 0.0, 0.0),
            face_normal: Vec3::new(0.0, 0.0, 0.0),
            ..undefined
        }),
    }
}

// ---- curve side (sm_FindSideOfCurve) ----------------------------------------------------

/// `sm_FindSideOfCurve`: projects `point` onto `curve` along `projection_direction`
/// and tells whether the point lies on the right-hand side of the curve for an observer
/// on the positive side of the projection plane walking along the curve tangent.
///
/// # Errors
///
/// `SM_ERR` when the projection does not give exactly one solution, the point is on
/// the curve (within `tol`), the tangent is along the projection direction, or the
/// projected vector and tangent are parallel (upstream also leaves `UNSURE`); those of
/// the solve and evaluation.
pub fn find_side_of_curve(
    k: &Kernel,
    curve: &Curve,
    point: Point3,
    projection_direction: Vec3,
    tol: f64,
) -> Result<bool> {
    // Project Point onto Curve along the Projection direction
    let vectors = [projection_direction];
    let solutions = curve.global_point_solve::<NoObject>(
        k,
        &curve.natural_interval(),
        SolverOperation::ProjectedMinimize,
        point,
        EFF_ZERO_SQRT,
        SolutionRequested::Single,
        &GlobalPointSolveOptions {
            target_distance: None,
            opt_vectors: Some(&vectors),
            handle: None,
            analytic_handle: None,
            zone_tol_3d: None,
        },
    )?;

    // Determine if point is on left or right side of the curve relative to the projection plane.
    if solutions.len() != 1 {
        return Err(sm_err());
    }
    let sol = solutions.get(0).ok_or_else(sm_err)?;

    let pv = curve.evaluate(k, sol.start.param(0)?, 1, true, true)?;
    let (p0, p1) = match pv.as_slice() {
        [p0, p1, ..] => (*p0, *p1),
        _ => return Err(null("sm_FindSideOfCurve: evaluation without tangent")),
    };
    let mut vec = point - p0;

    // Project vectors into projection plane
    vec = projection_direction.cross(vec).cross(projection_direction);

    // point is on the curve
    if vec.length() < tol {
        return Err(sm_err());
    }

    vec = vec.unitize_or_z();
    let mut tan = projection_direction.cross(p1).cross(projection_direction);

    // projection direction is the same as the curve tangent or curve tangent is zero
    if tan.length() < tol {
        return Err(sm_err());
    }
    tan = tan.unitize_or_z();

    // projected vec and curve tangent are parallel
    let side = tan.cross(vec);
    if side.length() < EFF_ZERO {
        return Err(sm_err());
    }

    // set output
    Ok(side.dot(projection_direction) <= 0.0)
}

// ---- face sectors (sm_FindSectorOfFace) -------------------------------------------------

fn face_surface(b: &Brep, face: FaceId) -> Result<&Surface> {
    face2::surface(b, face)?.ok_or_else(|| null("face without surface"))
}

/// `sm_FindSectorOfFace`: the region on the side of `face` a test point (known to be
/// near an intersection with the face) lies in, and its distance from the face. With
/// `uv_guess` (a point on the face) the side is taken from `test_dir` unless it is
/// tangent to the face, where (as without a guess) the test point is dropped to the
/// surface and the side taken from the drop vector.
///
/// # Errors
///
/// `SM_ERR` when the global drop finds nothing or the test point is on the surface;
/// [`SolidError::InvalidInput`] where upstream dereferences null (no surface, face
/// use, mate or shell; a shell without region returns `None`, as upstream); those of
/// the surface evaluations and solves. Without a guess the face's surface cache is
/// fetched (`smsurf_GetSurfaceCache`, which makes the face's trimmed cache and its UV
/// trim curves on `b`) and checked out with its point test off around the
/// `GlobalPointSolve`, so every solution is kept unclassified, as upstream.
pub fn find_sector_of_face(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    uv_guess: Option<Point2>,
    test_point: Point3,
    test_dir: Vec3,
) -> Result<(f64, Option<RegionId>)> {
    // (an exact evaluation copy: the cache lookup below changes the B-rep)
    let surface = &face_surface(b, face)?.clone_for_evaluation()?;
    let eval = |uv: Point2| -> Result<(Point3, Vec3)> {
        // found Surface Point, Found Surface Point Normal
        let p = surface.evaluate_point(k, uv)?;
        let n = surface.evaluate_normal(k, uv, Side::Left, Side::Left)?;
        Ok((p, n))
    };
    let uv_of = |start: &crate::geom::solution_array::SolutionEnd| -> Result<Point2> {
        Ok(Point2::new(start.param(0)?, start.param(1)?))
    };

    // Get SurfacePoint, SurfaceNormal, rdDistance, and sTestVec
    let (surf_pnt, norm, test_vec) = if let Some(guess) = uv_guess {
        {
            // assume that the given cpUVGuess is on the face as expected
            let (surf_pnt, norm) = eval(guess)?;

            // Can't classify a point on the surface - that point won't be in a region sector
            // (upstream passes SM_EFF_ZERO_RAD to the degree tolerance of IsPerpendicularTo)
            if norm.is_perpendicular_to(test_dir, EFF_ZERO_RAD) {
                // try old method of dropping a point to the surface

                // with cpUVGuess do a cheap local solve
                let solve = surface.local_point_solve::<NoObject>(
                    k,
                    &face2::uv_domain(b, face)?,
                    SolverOperation::Minimize,
                    test_point,
                    guess,
                )?;
                let uv_found = if solve.found {
                    uv_of(&solve.solution.start)?
                } else {
                    guess
                };
                let (surf_pnt, norm) = eval(uv_found)?;
                (surf_pnt, norm, test_point - surf_pnt)
            } else {
                (surf_pnt, norm, test_dir)
            }
        }
    } else {
        {
            // without cpUVGuess do a more expensive global solve
            // smsurf_GetSurfaceCache(pFace->GetSurface()), NER; checked out
            // (SmCacheCheckOutIn) unless an enclosing caller already holds it
            let lease = if surface.trim_cache_in_use() {
                None
            } else {
                match crate::query::face::p2::face_surface_cache(k, b, face) {
                    Ok(lease) => Some(lease),
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => return Err(SolidError::null_pointer(OP)),
                }
            };
            // Turn off point testing so GlobalPointSolve() will keep all point
            // solutions without classifying the solution point against the trim
            // boundaries.
            let solved = face2::uv_domain(b, face).and_then(|domain| {
                surface.global_point_solve_point_test_off::<NoObject>(
                    k,
                    Some(NoObject),
                    &domain,
                    SolverOperation::Minimize,
                    test_point,
                    EFF_ZERO,
                    None,
                    SolutionRequested::All,
                )
            });
            if let Some(lease) = lease {
                lease.release();
            }
            let solutions = solved?;
            let Some(first) = solutions.get(0) else {
                return Err(sm_err());
            };
            let uv_found = uv_of(&first.start)?;
            let (surf_pnt, norm) = eval(uv_found)?;
            (surf_pnt, norm, test_point - surf_pnt)
        }
    };

    // size of sTestVec (From sSurfPnt to crTestPoint or crTestDir)
    let distance = test_vec.length();

    // Can't classify a point on the surface - that point won't be in a region sector
    if distance < EFF_ZERO {
        return Err(sm_err());
    }

    // classify testPoint based on angle between FacePointNormal and TestVec
    let mut fu = face2::upward_faceuse(b, face)?
        .ok_or_else(|| null("sm_FindSectorOfFace: face without face use"))?;
    // (upstream: `if (dot > 0.0) up else mate`, so a NaN dot product takes the mate)
    let up = norm.dot(test_vec) > 0.0;
    if !up {
        fu = eu1::faceuse_mate(b, fu)?
            .ok_or_else(|| null("sm_FindSectorOfFace: face use without mate"))?;
    }
    let shell = eu1::faceuse_shell(b, fu)?
        .ok_or_else(|| null("sm_FindSectorOfFace: face use without shell"))?;
    let region = eu1::shell_region(b, shell)?;
    let _ = surf_pnt;
    Ok((distance, region))
}

// ---- vertex edge intervals (SmBrep::UpdateVertexEdgeIntervals) ---------------------------

/// `UpdateVertexEdgeIntervals`: moves the end of every edge at `vertex` to the drop of
/// the vertex point onto the edge curve when that is closer than the current end
/// (notifying the edge of the pre-edit and rebuilding the UV trim curves of its edge
/// use pairs), then sets the vertex tolerance to twice the largest remaining gap.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge without curve (upstream dereferences it);
/// those of the evaluations, drops and notifications.
pub fn update_vertex_edge_intervals(
    k: &Kernel,
    b: &mut Brep,
    env: &mut edge::NotifyEnv<'_>,
    vertex: VertexId,
) -> Result<()> {
    let mut max_dist = 0.0;
    let brep_ref = ObjectRef {
        type_code: TYPE,
        key: b.key(),
    };

    // for every vertex edge
    for e in vertex::edges(b, vertex, None)? {
        // locals
        let curve = b
            .edge(e)?
            .curve
            .clone()
            .ok_or_else(|| null("UpdateVertexEdgeIntervals: edge without curve"))?;
        let vertex_point = vertex::point(b, vertex)?;
        let vertex_tol = vertex::tolerance(b, vertex)?;
        let interval = edge::interval(b, e)?;
        let mut edited_edge = false;

        let drop = |t: f64| -> Result<crate::curve::curve::p6::DroppedPoint> {
            // drop vertex point to curve (cheap LocalPointSolve with the guess)
            curve.drop_point(
                k,
                &curve.natural_interval(),
                vertex_point,
                None,
                vertex_tol,
                Some(t),
                SolverOperation::Minimize,
            )
        };
        let mut orig_dist;
        if edge::start_vertex(b, e)? == Some(vertex) {
            // eval edge's start point
            let min_t = interval.min();
            let pnt = curve.evaluate_point(k, min_t)?;

            // get edgeStart to vertex distance
            orig_dist = pnt.distance(vertex_point);

            let d = drop(min_t)?;

            // when drop point is better choice than current edgeStart point
            if d.distance < orig_dist && d.parameter < interval.max() - EFF_ZERO {
                // reset the curve interval
                let edge_ref = (env.key_of)(EntityRef::Edge(e));
                ignored(edge::notify(
                    b,
                    e,
                    env,
                    NotifyOperation::PreEdit,
                    [Some(edge_ref), Some(brep_ref), None],
                ))?; // (status not checked upstream)
                edited_edge = true;
                let ivl = Extent1d::from_min_max(d.parameter, interval.max());
                edge::set_interval(b, e, &ivl)?;
                orig_dist = d.distance;
            }
        } else {
            // (SM_ASSERT: the edge's other vertex is the target vertex)
            // eval EdgeEnd point
            let max_t = interval.max();
            let pnt = curve.evaluate_point(k, max_t)?;

            // get EdgeEnd to Vertex distance
            orig_dist = pnt.distance(vertex_point);

            let d = drop(max_t)?;

            // when drop point is better than current edgeEnd
            if d.distance < orig_dist && d.parameter > interval.min() + EFF_ZERO {
                // retset the edge's interval
                let edge_ref = (env.key_of)(EntityRef::Edge(e));
                ignored(edge::notify(
                    b,
                    e,
                    env,
                    NotifyOperation::PreEdit,
                    [Some(edge_ref), Some(brep_ref), None],
                ))?; // (status not checked upstream)
                edited_edge = true;
                let ivl = Extent1d::from_min_max(interval.min(), d.parameter);
                edge::set_interval(b, e, &ivl)?;
                orig_dist = d.distance;
            }
        }

        // when edge's interval was modified
        if edited_edge {
            // for every edgeuse mated pair
            let eus = b.edge(e)?.edgeuses.clone();
            for &eu in eus.iter().step_by(2) {
                // for edges with UVTrimCurves - rebuild UVTrimCurve
                if eu1::uv_trim_curve(b, eu)?.0.is_some() {
                    // TRUE = delete existing UVTrimCurve (statuses not checked upstream)
                    ignored(eu1::set_uv_trim_curve(
                        b,
                        eu,
                        eu1::UvTrimCurveArg::Null,
                        0.0,
                        true,
                        None,
                    ))?;
                    let _ignored_status =
                        get_or_create_uv_trim_curve(k, b, eu, false, false, true)?;
                }
            }
        }

        max_dist = max2(max_dist, orig_dist);
    }

    vertex::set_tolerance(b, vertex, max_dist * 2.0, true, true)?;
    Ok(())
}

// ---- seam UV trim curves (sm_RedistributeSeamUVTrimCurves) -------------------------------

/// `sm_RedistributeSeamUVTrimCurves`: after a new seam edge (`uv_trim_curve` given,
/// `seam_edge` a seam direction) has been connected to its vertices, checks that the
/// UV trim curves on either side of the seam continue their predecessors' side and
/// swaps them between the two mated pairs when not. Returns whether they were swapped.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a seam without exactly four edge uses or missing
/// trim curves; those of UV trim creation, endpoint evaluations, the singularity
/// test and the trim curve swap. No trims are swapped after a failed prerequisite.
pub fn redistribute_seam_uv_trim_curves(
    k: &Kernel,
    b: &mut Brep,
    new_edge: EdgeId,
    uv_trim_curve: Option<&Curve>,
    seam_edge: SurfParamType,
    srf_domain: &Extent2d,
) -> Result<bool> {
    // when NewEdge has UVTrimCurves and NewEdge is a SeamEdge - see if UVTrimCurves on either
    // side of seam are properly distributed
    if uv_trim_curve.is_none() || seam_edge == SurfParamType::Neither {
        return Ok(false);
    }
    let eus = b.edge(new_edge)?.edgeuses.clone();
    // SM_ASSERT upstream; this algorithm handles exactly two mated pairs (#520).
    if eus.len() != 4 {
        return Err(null(
            "sm_RedistributeSeamUVTrimCurves: expected 4 edge uses",
        ));
    }
    // sEdgeuse[0] = Starts At StartVertex OneFace, sEdgeuse[3] = Starts At EndVertex OtherFace
    let this_eu = *eus
        .first()
        .ok_or_else(|| null("sm_RedistributeSeamUVTrimCurves: no edge uses"))?;
    let other_eu = *eus
        .get(3)
        .ok_or_else(|| null("sm_RedistributeSeamUVTrimCurves: fewer than 4 edge uses"))?;
    let prev_this = eu1::cw_edgeuse(b, this_eu)?
        .ok_or_else(|| null("sm_RedistributeSeamUVTrimCurves: edge use without CW"))?;
    let prev_other = eu1::cw_edgeuse(b, other_eu)?
        .ok_or_else(|| null("sm_RedistributeSeamUVTrimCurves: edge use without CW"))?;
    for eu in [this_eu, other_eu] {
        if eu1::uv_trim_curve(b, eu)?.0.is_none() {
            return Err(null(
                "sm_RedistributeSeamUVTrimCurves: edge use without UV trim curve",
            ));
        }
    }

    // Both the outer Result and the retained projection status must succeed.
    // Upstream checks these statuses and curves only with SM_ASSERT (#520).
    for eu in [prev_this, prev_other] {
        eu3::get_or_create_uv_trim_curve(k, b, eu, false, false, true)?.status?;
        if eu1::uv_trim_curve(b, eu)?.0.is_none() {
            return Err(null(
                "sm_RedistributeSeamUVTrimCurves: neighbor without UV trim curve",
            ));
        }
    }

    // A failed evaluation must not feed unset points into seam-side classification.
    // First derivatives are requested but unused, as upstream.
    // NewE Start UV values from UVTrimCurves
    let this_point_uv = normalized_evaluate(k, b, this_eu, 0.0, true, true)?;
    let other_point_uv = normalized_evaluate(k, b, other_eu, 0.0, true, true)?;
    // Prev Start UV values from PrevUVtrimCurves
    let prev_this_point_uv = normalized_evaluate(k, b, prev_this, 1.0, true, true)?;
    let prev_other_point_uv = normalized_evaluate(k, b, prev_other, 1.0, true, true)?;

    // make sure sThisPointUV == sPrevThisPointUV, sOtherPointUV == sPrevOtherPointUV are
    // the same to tol by switching the UVTrimCurves if needed. Expect all 4 PointUVs to
    // lie on one side or the other of the Seam.
    // pick large bndry search tolerance - just need to know which side of the seam the
    // UVPoints lie.
    let tol_u = srf_domain.x_length() / 4.0;
    let tol_v = srf_domain.y_length() / 4.0;
    // switch to SmPoint2d objs - yuk
    let this_point_2d = Point2::new(this_point_uv.x, this_point_uv.y);
    let prev_this_point_2d = Point2::new(prev_this_point_uv.x, prev_this_point_uv.y);
    let other_point_2d = Point2::new(other_point_uv.x, other_point_uv.y);
    let prev_other_point_2d = Point2::new(prev_other_point_uv.x, prev_other_point_uv.y);
    // classify the 4 UVPoints
    let (this_u, this_v) = srf_domain.classify_point(this_point_2d, tol_u, Some(tol_v));
    let (prev_this_u, prev_this_v) =
        srf_domain.classify_point(prev_this_point_2d, tol_u, Some(tol_v));
    let (_other_u, _other_v) = srf_domain.classify_point(other_point_2d, tol_u, Some(tol_v)); // (debug asserts only)
    let (prev_other_u, prev_other_v) =
        srf_domain.classify_point(prev_other_point_2d, tol_u, Some(tol_v));
    let this_seam_edge = match seam_edge {
        SurfParamType::UMin | SurfParamType::UMax => SurfParamType::U,
        SurfParamType::VMin | SurfParamType::VMax => SurfParamType::V,
        other => other,
    };
    let face = eu1::face(b, this_eu)?
        .ok_or_else(|| null("sm_RedistributeSeamUVTrimCurves: edge use without face"))?;
    let surface = face2::surface(b, face)?
        .ok_or_else(|| null("sm_RedistributeSeamUVTrimCurves: face without surface"))?;
    // (IsSingularity is a boolean: a failing evaluation reads TRUE upstream)
    let this_pole = ignored(surface.is_singularity(k, this_point_2d, EFF_ZERO, false))?
        .is_none_or(|sg| sg.singular);
    let other_pole = ignored(surface.is_singularity(k, other_point_2d, EFF_ZERO, false))?
        .is_none_or(|sg| sg.singular);

    // decide if we have to swap UV curves
    let mut swap = (this_seam_edge == SurfParamType::U
        && this_u != prev_this_u
        && this_u == prev_other_u)
        || (this_seam_edge == SurfParamType::V && this_v != prev_this_v && this_v == prev_other_v);

    // A pole maps to an entire UV boundary. Its neighboring trim endpoint cannot select
    // the seam side. As in CreateUVTrimCurve, compare the inward binormal with the
    // cross-boundary derivative away from the poles.
    if this_pole || other_pole {
        let parameter = edge::interval(b, new_edge)?.evaluate(0.48765); // perturb off midpoint for evaluation
        let this_uv_trim_curve = eu1::uv_trim_curve(b, this_eu)?.0.ok_or_else(|| {
            null("sm_RedistributeSeamUVTrimCurves: edge use without UV trim curve")
        })?;
        // (the three statuses are tested, not propagated)
        let uv = ignored(this_uv_trim_curve.evaluate_point(k, parameter))?;
        let binormal = match uv {
            Some(_) => ignored(evaluate_binormal(k, b, this_eu, parameter, true))?,
            None => None,
        };
        if let (Some(uv), Some(BinormalEval { binormal, .. })) = (uv, binormal) {
            // (the face surface fetched above; re-borrowed past the binormal evaluation)
            let surface = face2::surface(b, face)?
                .ok_or_else(|| null("sm_RedistributeSeamUVTrimCurves: face without surface"))?;
            if let Some((_surface_point, du, dv)) =
                ignored(surface.evaluate_1st_derivatives(k, Point2::new(uv.x, uv.y), true, true))?
            {
                let across = if this_seam_edge == SurfParamType::U {
                    du
                } else {
                    dv
                };
                let side = if this_seam_edge == SurfParamType::U {
                    this_u
                } else {
                    this_v
                };
                let inward = across.dot(binormal);
                // The derivative points into the domain at its minimum and out at its
                // maximum. The edgeuse's binormal must point inside.
                swap = (side == ExtentPointType::Start && inward < 0.0)
                    || (side == ExtentPointType::End && inward > 0.0);
            }
        }
    }

    // When we need to swap the UVCurves to get the connected UVPoints on the same side
    // of the Seam
    if swap {
        // note: SetUVTrimCurve sets EU->m_pUVTrimCurve and m_pUVTrimCurve->Owner
        // (`set_member_uv_trim_curve` takes the curve by value: a mated pair without
        // one, which upstream would clear with NULL, is reported as a null dereference)
        let this_uv_trim_curve = eu1::uv_trim_curve(b, this_eu)?.0.cloned().ok_or_else(|| {
            null("sm_RedistributeSeamUVTrimCurves: edge use without UV trim curve")
        })?;
        let other_uv_trim_curve = eu1::uv_trim_curve(b, other_eu)?.0.cloned().ok_or_else(|| {
            null("sm_RedistributeSeamUVTrimCurves: edge use without UV trim curve")
        })?;
        let this_drop_curve_fail = eu1::drop_curve_fail(b, this_eu)?.0.cloned();
        let other_drop_curve_fail = eu1::drop_curve_fail(b, other_eu)?.0.cloned();
        // connect ThisEdgeuse MatedPair set with OtherUVTrimCurve
        eu1::set_member_uv_trim_curve(b, this_eu, other_uv_trim_curve, other_drop_curve_fail)?;
        // connect OtherEdgeuse MatedPair set with ThisUVTrimCurve
        eu1::set_member_uv_trim_curve(b, other_eu, this_uv_trim_curve, this_drop_curve_fail)?;
        // inform the public
        return Ok(true);
    }

    // all done
    Ok(false)
}

/// `sm_GetNaturalSeamInterval`: the constant-parameter direction and varying UV interval
/// of `curve` over `interval` when it is a straight natural-boundary curve of `surface`
/// (`domain` its natural domain): its base surface equals `surface`, its UV curve is a
/// degree-1 line of two control points, and both ends lie on one U (or V) boundary.
/// [`SurfParamType::Neither`] (and an initial interval) otherwise, or for an interval
/// shorter than the domain-scaled parameter tolerance.
///
/// # Errors
///
/// Those of the surface comparison and of the UV curve's evaluations (`SER`).
pub(crate) fn natural_seam_interval(
    k: &Kernel,
    curve: &CrvOnSurf,
    interval: &Extent1d,
    surface: &Surface,
    domain: &Extent2d,
) -> Result<(SurfParamType, Extent1d)> {
    // Init returns
    let mut seam_interval = Extent1d::new();
    let none = Ok((SurfParamType::Neither, Extent1d::new()));

    // If the defining curve and surface are not the same, then it is not a seam.
    if !curve.surface().approx_eq(k, surface)? {
        return none;
    }

    // If the defining UV curve is not a line, then it is not a seam.
    let uv = curve.uv_curve();
    if uv.number_control_points()? != 2 || uv.degree()? != 1 {
        return none;
    }

    // Evaluate the endpoints of this segment, which may cover only part of a seam.
    let start = uv.evaluate_point(k, interval.min())?;
    let end = uv.evaluate_point(k, interval.max())?;

    // Match the domain-scaled parameter tolerance used for full-period checks.
    let (min, max) = (domain.min(), domain.max());
    let u_tol = (1e-8 * domain.x_length()).abs();
    let v_tol = (1e-8 * domain.y_length()).abs();
    let same = |a: f64, b: f64, tol: f64| (a - b).abs() < tol;

    // A U seam has constant U on either U boundary; a V seam is analogous.
    let u_seam =
        same(start.x, end.x, u_tol) && (same(start.x, min.x, u_tol) || same(start.x, max.x, u_tol));
    let v_seam =
        same(start.y, end.y, v_tol) && (same(start.y, min.y, v_tol) || same(start.y, max.y, v_tol));
    if !u_seam && !v_seam {
        return none;
    }

    // Intervals shorter than the tolerance are not valid seam segments.
    seam_interval.add_value(if u_seam { start.y } else { start.x });
    seam_interval.add_value(if u_seam { end.y } else { end.x });
    if seam_interval.length() <= if u_seam { v_tol } else { u_tol } {
        return none;
    }

    // Set the seam parameter direction and return success.
    let seam = if u_seam {
        SurfParamType::U
    } else {
        SurfParamType::V
    };
    Ok((seam, seam_interval))
}

/// `sm_IsFirstNonseparatingSeam`: whether inserting `curve` over `interval` between
/// `start_vu` and `end_vu` completes the first non-separating seam of `face`, so the
/// edge makes a new loop without a new face. The curve, with any segments already in
/// its loop, must cover one full natural boundary of a surface periodic in U and V
/// without poles exactly once; every other loop must be an isolated vertex or an
/// interior hole away from the seams. Faces already cut and singly periodic surfaces
/// (where even a closed seam can separate the face) are excluded.
///
/// # Errors
///
/// [`SolidError::Unported`] from the queries; every other failure is a `FALSE` answer,
/// as upstream's statuses are.
pub(crate) fn is_first_nonseparating_seam(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    start_vu: VertexuseId,
    end_vu: VertexuseId,
    curve: &CrvOnSurf,
    interval: &Extent1d,
) -> Result<bool> {
    // upstream's statuses inside this SmBoolean function are FALSE answers
    let no = |r: Result<bool>| match r {
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(false),
        ok => ok,
    };
    no(first_nonseparating_seam(
        k, b, face, start_vu, end_vu, curve, interval,
    ))
}

fn first_nonseparating_seam(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    start_vu: VertexuseId,
    end_vu: VertexuseId,
    curve: &CrvOnSurf,
    interval: &Extent1d,
) -> Result<bool> {
    // A negative seam classification is expected for ordinary separating cuts.
    // Completing the seam must join two ends of the same loopuse.
    let Some(seam_lu) = vertexuse::loopuse(b, start_vu)? else {
        return Ok(false);
    };
    if Some(seam_lu) != vertexuse::loopuse(b, end_vu)? {
        return Ok(false);
    }

    // Get the surface and check that it is periodic in U and V without poles.
    let face_tolerance = face2::tolerance(b, face)?;
    let surface = face_surface(b, face)?;
    let domain = surface.natural_uv_domain_with_nurb(k)?;
    if !surface.is_periodic(k, &domain, SurfParamType::U)?
        || !surface.is_periodic(k, &domain, SurfParamType::V)?
        || surface
            .get_singularities_of_face(k, face_tolerance, false, false)?
            .singular
            != 0
    {
        return Ok(false);
    }

    let (min, max) = (domain.min(), domain.max());
    let (width, height) = (domain.x_length(), domain.y_length());
    // AER: an assertion that returns FALSE
    if !(width > 0.0 && height > 0.0) {
        return Ok(false);
    }

    // Find the natural-boundary interval supplied by the new edge.
    let (seam, seam_interval) = natural_seam_interval(k, curve, interval, surface, &domain)?;
    if seam == SurfParamType::Neither {
        return Ok(false);
    }

    let mut intervals = vec![seam_interval];
    if !loopuse::is_vertex_loopuse(b, seam_lu)? {
        // Earlier intervals join vertex loops into one open chain. Its edges
        // must each be used twice, in opposite directions, by this loopuse.
        let start_vertex = b.vertexuse(start_vu)?.vertex;
        let end_vertex = b.vertexuse(end_vu)?.vertex;
        if start_vertex == end_vertex {
            return Ok(false);
        }

        let mut start_degree = 0usize;
        let mut end_degree = 0usize;
        for e in loopuse::edges(b, seam_lu, None)? {
            let e = e.ok_or_else(|| SolidError::null_pointer(OP))?;
            let record = b.edge(e)?;
            let Some(segment) = record.curve.as_ref().and_then(|c| match c.kind() {
                CurveKind::CrvOnSurf(c) => Some(c.as_ref()),
                _ => None,
            }) else {
                return Ok(false);
            };
            let (segment_seam, segment_interval) =
                natural_seam_interval(k, segment, &record.interval, surface, &domain)?;
            if segment_seam != seam {
                return Ok(false);
            }

            let (mut same, mut opposite) = (0usize, 0usize);
            for eu in edge::edgeuses(b, e, None, None)? {
                if eu1::loopuse(b, eu)? == Some(seam_lu) {
                    match b.edgeuse(eu)?.orientation {
                        OrientType::Same => same += 1,
                        OrientType::Opposite => opposite += 1,
                        OrientType::Unknown => {}
                    }
                }
            }
            if same != 1 || opposite != 1 {
                return Ok(false);
            }

            let (edge_start, edge_end) = edge::start_end_vertices(b, e)?;
            start_degree +=
                usize::from(edge_start == start_vertex) + usize::from(edge_end == start_vertex);
            end_degree +=
                usize::from(edge_start == end_vertex) + usize::from(edge_end == end_vertex);
            intervals.push(segment_interval);
        }
        if start_degree != 1 || end_degree != 1 {
            return Ok(false);
        }
    }

    // A complete period must have neither gaps nor overlapping segments.
    let period = if seam == SurfParamType::U {
        domain.v_interval()
    } else {
        domain.u_interval()
    };
    let param_tol = (1e-8 * period.length()).abs();
    intervals.sort_by(|x, y| x.min().total_cmp(&y.min()));
    let mut end = period.min();
    for i in &intervals {
        if (end - i.min()).abs() >= param_tol {
            return Ok(false);
        }
        end = i.max();
    }
    if (end - period.max()).abs() >= param_tol {
        return Ok(false);
    }

    // All other edge loops must bound interior holes away from the seams.
    let seam_loop = b.loopuse(seam_lu)?.loop_;
    for l in face2::loops(b, face, false, None)? {
        if Some(l) == seam_loop {
            continue;
        }

        // Vertex loops do not separate the face. CurveOnClassify has already
        // divided the input curve at any vertices that lie on this seam.
        let lu = b
            .loop_(l)?
            .loopuse
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        if loopuse::is_vertex_loopuse(b, lu)? {
            continue;
        }

        let mut bounds = Extent2d::new();
        if !loopuse::loop_is_closed_ptrs(b, l)? {
            return Ok(false);
        }
        match crate::topology::face::p3::compute_loop_orientation(
            k,
            b,
            face,
            l,
            Some(&mut bounds),
            true,
        ) {
            Ok(OrientType::Opposite) => {}
            Err(e) if e.is_unported() => return Err(e),
            _ => return Ok(false),
        }

        // Check that the hole stays farther than tolerance from every seam.
        let uv_tol = crate::surface::surface::p1::map_to_2d(
            k,
            face2::tolerance(b, face)?,
            bounds.min(),
            face_surface(b, face)?,
        )?;
        if bounds.min().x <= min.x + uv_tol
            || bounds.max().x >= max.x - uv_tol
            || bounds.min().y <= min.y + uv_tol
            || bounds.max().y >= max.y - uv_tol
        {
            return Ok(false);
        }
    }
    Ok(true)
}

// ---- vertex split UV trim curves (sm_MakeVertexSplitUVTrimCurve) -------------------------

/// The `SmBSplineCurve` part of a B-spline-derived curve (`SM_CAST_PTR(SmBSplineCurve,
/// ..)`), `None` for other kinds.
fn bspline_part(curve: &Curve) -> Option<BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(c) => Some(c.clone()),
        CurveKind::Line(c) => Some(c.base.clone()),
        CurveKind::Ellipse(c) => Some(c.base().clone()),
        CurveKind::Hyperbola(c) => Some(c.base().clone()),
        CurveKind::Parabola(c) => Some(c.base().clone()),
        _ => None,
    }
}

/// `sm_MakeVertexSplitUVTrimCurve` (a helper of `SmBrep::MakeVertexSplitEdge`): splits
/// the UV trim curve of `eu_orig` at `edge_split_parameter` into the parts of the
/// original and the new edge use (`eu_new`), reparameterised to the two halves of the
/// edge interval; the trim curve is deleted instead when the split is within
/// `scaled_zero` of its ends or its 3D image is farther than a hundredth of the edge
/// tolerance from `split_pnt` (unless `for_tesselation` re-projects the split point and
/// finds it). `orient` says which half the original edge use keeps.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for an edge use without face or a face without surface
/// (`NER`); [`SolidError::InvalidInput`] for an edge use without edge; those of the
/// copy, evaluation, solve, trim and reparameterisation; [`SolidError::Unported`] for a
/// face surface that is not a B-spline or plane.
#[allow(clippy::too_many_arguments)]
pub fn make_vertex_split_uv_trim_curve(
    k: &Kernel,
    b: &mut Brep,
    eu_orig: EdgeuseId,
    eu_new: EdgeuseId,
    split_pnt: Point3,
    edge_split_parameter: f64,
    scaled_zero: f64,
    orient: OrientType,
    for_tesselation: bool,
) -> Result<()> {
    let Some(uv_trim_curve) = eu1::uv_trim_curve_pointer(b, eu_orig)?.cloned() else {
        return Ok(());
    };
    // when edgeuse has a UVTrimCurve

    // get UVTrimCurve interval
    let mut ivl1 = uv_trim_curve.natural_interval();

    // when SplitParameter is near UVTrimCurve->Ends
    if edge_split_parameter < ivl1.min() + scaled_zero
        || edge_split_parameter > ivl1.max() - scaled_zero
    {
        // (SetUVTrimCurve statuses are not checked upstream)
        ignored(eu1::set_uv_trim_curve(
            b,
            eu_orig,
            eu1::UvTrimCurveArg::Null,
            0.0,
            true,
            None,
        ))?;
        return Ok(());
    }
    // split param is far from UVTrimCurve->Ends

    // copy UVTrimCurve
    let mut new_uv_curve = uv_trim_curve.copy(k)?;

    // make a 3dCurve from the UVTrimCurve (SmCrvOnSurf(*pNewUVCurve, *pSurface)):
    // its point evaluation is the UV curve point lifted by the face surface.
    let edge_to_split = eu1::edge(b, eu_orig)?
        .ok_or_else(|| null("sm_MakeVertexSplitUVTrimCurve: edge use without edge"))?;
    let face = eu1::face(b, eu_orig)?.ok_or_else(|| SolidError::null_pointer(OP))?; // NER
    let surface = face2::surface(b, face)?
        .ok_or_else(|| SolidError::null_pointer(OP))?
        .clone_for_evaluation()?; // NER
    let lift = |t: f64| -> Result<Point3> {
        let p = new_uv_curve.evaluate_point(k, t)?;
        surface.evaluate_point(k, Point2::new(p.x, p.y))
    };

    // when 3D distance from splitPoint to UVTrimCurvePoint > tol/100 (was "tol/30" [B606])
    let mut edge_split_parameter_uv = edge_split_parameter;
    let p3d = lift(edge_split_parameter_uv)?;

    if p3d.distance(split_pnt) > edge::tolerance(b, edge_to_split)? / 100.0 {
        let mut reuse_trim_curve = false;
        if for_tesselation {
            let uv_3d_curve = Curve::from(CrvOnSurf::new(
                k,
                new_uv_curve.clone_for_evaluation()?,
                surface.clone_for_evaluation()?,
                None,
                0,
            )?);
            // get best UVTrimCurve param possible by projecting splitPoint to 3d UVTrimCurve
            let (found, sol) = uv_3d_curve.local_point_solve::<NoObject>(
                k,
                &new_uv_curve.natural_interval(),
                SolverOperation::Minimize,
                split_pnt,
                &PointSolveOptions {
                    distance_tolerance: None,
                    target_distance: None,
                    vectors: None,
                },
                edge_split_parameter,
            )?;

            // If we have a solution and it is central to the interval, use this trim curve
            if found
                && edge_split_parameter_uv > ivl1.min() + scaled_zero
                && edge_split_parameter_uv < ivl1.max() - scaled_zero
            {
                edge_split_parameter_uv = sol.start.param(0)?;
                reuse_trim_curve = true;
            }
        }

        if !reuse_trim_curve {
            ignored(eu1::set_uv_trim_curve(
                b,
                eu_orig,
                eu1::UvTrimCurveArg::Null,
                0.0,
                true,
                None,
            ))?;
            return Ok(());
        }
    }

    // set up intervals for 3D and UV curve split segments
    //    sIvl1,   sIvl2   = UVTrimCurve [start,SplitParamUV], [SplitParamUV, end]
    //    sIvl13D, sIvl23D = 3DCurve     [start,SplitParam],   [SplitParam,   end]
    let mut ivl2 = ivl1;
    // (the SetMinMax statuses are not checked upstream)
    ignored(ivl1.set_min_max(ivl1.min(), edge_split_parameter_uv))?;
    let ivl1_3d = Extent1d::from_min_max(ivl1.min(), edge_split_parameter);
    ignored(ivl2.set_min_max(edge_split_parameter_uv, ivl2.max()))?;
    let ivl2_3d = Extent1d::from_min_max(edge_split_parameter, ivl2.max());

    // trim and bind UVTrimCurves to Edgeuses (the attached curve is edited in place)
    {
        let attached = b
            .edgeuse_mut(eu_orig)?
            .uv_trim_curve
            .as_mut()
            .ok_or_else(|| null("sm_MakeVertexSplitUVTrimCurve: trim curve vanished"))?;
        if orient == OrientType::Same {
            attached.trim(k, &mut ivl1, TrimMode::Notify)?;
            attached.edit_parameterization(k, &ivl1_3d)?;
            new_uv_curve.trim(k, &mut ivl2, TrimMode::Notify)?;
            new_uv_curve.edit_parameterization(k, &ivl2_3d)?;
        } else {
            attached.trim(k, &mut ivl2, TrimMode::Notify)?;
            attached.edit_parameterization(k, &ivl2_3d)?;
            new_uv_curve.trim(k, &mut ivl1, TrimMode::Notify)?;
            new_uv_curve.edit_parameterization(k, &ivl1_3d)?;
        }
    }

    ignored(eu1::set_uv_trim_curve(
        b,
        eu_new,
        eu1::UvTrimCurveArg::New(Box::new(new_uv_curve)),
        0.0,
        false,
        None,
    ))?;
    Ok(())
}

// ---- B-rep intersection (SmBrep::BrepIntersect) -----------------------------------------

/// `BrepIntersect`: intersects every face of this B-rep with every face of `other`
/// (`SmFace::FaceIntersect`) and returns the 3D curves (duplicates removed) and, when
/// `want_points`, the singular points. The two distinct B-reps are lent mutably, as
/// upstream's faces reach theirs: the faces' trimmed surface caches and UV trim curves
/// are made, and the curves and touching points classified, on their owning B-reps.
/// Wire edges and shell vertices are not handled. Use [`brep_intersect_self`] when
/// both operands are the same B-rep.
///
/// # Errors
///
/// Those of face enumeration, `SmFace::FaceIntersect` and duplicate-curve removal.
pub fn brep_intersect_mut(
    k: &Kernel,
    b: &mut Brep,
    other: &mut Brep,
    approx_tol_3d: f64,
    crv_angle_tolerance_deg: f64,
    want_points: bool,
) -> Result<(Vec<Curve>, Option<Vec<Point3>>)> {
    let (curves, points, status) = brep_intersect_mut_with_partial(
        k,
        b,
        other,
        approx_tol_3d,
        crv_angle_tolerance_deg,
        want_points,
    );
    status?;
    Ok((curves, points))
}

/// [`brep_intersect_mut`] for a caller that reads the outputs whatever the call returned
/// (`SmApiIntersectBreps`): the curves and points with the status. On a failed face pair
/// they are those the earlier pairs accumulated (duplicates not removed yet), as
/// upstream's `SER` leaves them in the caller's arrays.
pub fn brep_intersect_mut_with_partial(
    k: &Kernel,
    b: &mut Brep,
    other: &mut Brep,
    approx_tol_3d: f64,
    crv_angle_tolerance_deg: f64,
    want_points: bool,
) -> (Vec<Curve>, Option<Vec<Point3>>, Result<()>) {
    let faces_of = |b: &Brep, other: &Brep| -> Result<(Vec<FaceId>, Vec<FaceId>)> {
        Ok((faces(b, None)?, faces(other, None)?))
    };
    let (this_faces, other_faces) = match faces_of(&*b, &*other) {
        Ok(f) => f,
        Err(e) => return (Vec::new(), want_points.then(Vec::new), Err(e)),
    };
    brep_intersect_faces(
        k,
        &this_faces,
        &other_faces,
        want_points,
        |this_face, other_face| {
            let options = crate::topology::face::p3::FaceIntersectOptions {
                want_points: true,
                ..Default::default()
            };
            let out = crate::topology::face::p3::face_intersect_breps(
                k,
                crate::topology::face::p3::FaceBreps::Two(&mut *b, &mut *other),
                this_face,
                other_face,
                approx_tol_3d,
                crv_angle_tolerance_deg,
                &options,
            )?;
            Ok((
                out.curves_3d.into_iter().map(Some).collect(),
                out.points_3d.unwrap_or_default(),
            ))
        },
    )
}

/// [`brep_intersect_mut`] of a B-rep with itself (`brep->BrepIntersect(ctx, brep, ..)`),
/// the B-rep lent mutably: each face's surface reaches its face, so the faces' trimmed
/// caches (and their UV trim curves) are made on it as upstream's
/// `smsurf_GetSurfaceCache` does.
///
/// # Errors
///
/// As [`brep_intersect_mut`].
pub fn brep_intersect_self(
    k: &Kernel,
    b: &mut Brep,
    approx_tol_3d: f64,
    crv_angle_tolerance_deg: f64,
    want_points: bool,
) -> Result<(Vec<Curve>, Option<Vec<Point3>>)> {
    let (curves, points, status) =
        brep_intersect_self_with_partial(k, b, approx_tol_3d, crv_angle_tolerance_deg, want_points);
    status?;
    Ok((curves, points))
}

/// [`brep_intersect_self`] with the partial outputs of a failed call, as
/// [`brep_intersect_mut_with_partial`].
pub fn brep_intersect_self_with_partial(
    k: &Kernel,
    b: &mut Brep,
    approx_tol_3d: f64,
    crv_angle_tolerance_deg: f64,
    want_points: bool,
) -> (Vec<Curve>, Option<Vec<Point3>>, Result<()>) {
    let this_faces = match faces(b, None) {
        Ok(f) => f,
        Err(e) => return (Vec::new(), want_points.then(Vec::new), Err(e)),
    };
    brep_intersect_faces(
        k,
        &this_faces,
        &this_faces,
        want_points,
        |this_face, other_face| {
            let options = crate::topology::face::p3::FaceIntersectOptions {
                want_points: true,
                ..Default::default()
            };
            let out = crate::topology::face::p3::face_intersect_breps(
                k,
                crate::topology::face::p3::FaceBreps::One(&mut *b),
                this_face,
                other_face,
                approx_tol_3d,
                crv_angle_tolerance_deg,
                &options,
            )?;
            Ok((
                out.curves_3d.into_iter().map(Some).collect(),
                out.points_3d.unwrap_or_default(),
            ))
        },
    )
}

/// The face-pair loop of `SmBrep::BrepIntersect`, with `face_pair` the
/// `SmFace::FaceIntersect` of one pair: the curves, the points (when `want_points`) and
/// the status. A failed pair (`SER`) leaves the outputs of the pairs before it.
fn brep_intersect_faces(
    k: &Kernel,
    this_faces: &[FaceId],
    other_faces: &[FaceId],
    want_points: bool,
    mut face_pair: impl FnMut(FaceId, FaceId) -> Result<(Vec<Option<Curve>>, Vec<Point3>)>,
) -> (Vec<Curve>, Option<Vec<Point3>>, Result<()>) {
    // init output
    let mut curves_3d: Vec<Option<Curve>> = Vec::new();
    let mut points: Option<Vec<Point3>> = want_points.then(Vec::new);

    // for every thisBrep Face
    for &this_face in this_faces {
        // (NER: arena ids are never null)
        // for every otherBrep Face
        for &other_face in other_faces {
            // intersect the faces
            let (mut curves, mut pts) = match face_pair(this_face, other_face) {
                Ok(out) => out,
                Err(e) => return (curves_3d.into_iter().flatten().collect(), points, Err(e)),
            };

            // accumulate the outputs
            curves_3d.append(&mut curves);
            if let Some(points) = points.as_mut() {
                points.append(&mut pts);
            }
        }
    }

    let status = crate::curve::curve::p5::remove_duplicate_curves(k, &mut curves_3d).map(|_| ());
    (curves_3d.into_iter().flatten().collect(), points, status)
}

// ---- corner blends (SmBrep::CreateCornerBlend) ------------------------------------------

/// `CreateCornerBlend`: a corner blend patch over a geometrically connected (unordered)
/// set of edge uses: orders them end to end (growing the match tolerance up to a
/// hundredfold), takes trimmed copies of their curves and, where tangent to the face is
/// wanted (`tangent_to_face`, default all; never for wire edges), their UV trim curves
/// and surfaces, pads two- and three-sided inputs with point curves, and passes the
/// arrays to `SmBSplineSurface::CreateCornerBlend`.
///
/// # Errors
///
/// `SM_ERR` for fewer than two edge uses or a gap in the chain; those of the edge use
/// evaluations; [`SolidError::InvalidInput`]
/// where upstream dereferences null (an opposite edge use without mate, an edge
/// without curve, a tangent edge use without face) or reads past
/// `tangent_to_face`; those of the copies, trims and the blend.
pub fn create_corner_blend(
    k: &Kernel,
    b: &mut Brep,
    edgeuses: &[EdgeuseId],
    tangent_to_face: Option<&[bool]>,
    approx_tol_3d: f64,
    tangency_tol_radians: f64,
) -> Result<Vec<BSplineSurface>> {
    // Need at least two edges to blend
    if edgeuses.len() < 2 {
        return Err(sm_err());
    }
    // locals
    let mut work_eus: Vec<EdgeuseId> = edgeuses.to_vec();
    let mut ordered_eus: Vec<EdgeuseId> = Vec::new();
    let mut eu_orients: Vec<OrientType> = Vec::new();

    // set sWorkTan: Copied from optional input else set to TRUE for all edgeuses
    let mut work_tan: Vec<bool> = match tangent_to_face {
        Some(t) => t.to_vec(),
        None => vec![true; edgeuses.len()],
    };

    // set sWorkEUS = list of all input edgeuses running in the SM_OT_SAME direction
    for i in 0..work_eus.len() {
        let eu = work_eus.get(i).copied().ok_or_else(sm_err)?;
        let eu = if eu1::orientation(b, eu)? == OrientType::Opposite {
            eu1::mate(b, eu)?
                .ok_or_else(|| null("CreateCornerBlend: opposite edge use without mate"))?
        } else {
            eu
        };
        if let Some(slot) = work_eus.get_mut(i) {
            *slot = eu;
        }
        let e =
            eu1::edge(b, eu)?.ok_or_else(|| null("CreateCornerBlend: edge use without edge"))?;
        if edge::is_wire(b, e)? {
            *work_tan
                .get_mut(i)
                .ok_or_else(|| null("CreateCornerBlend: tangent flags shorter than edge uses"))? =
                false;
        }
    }

    // locals for upcoming SmSurface::CreateCornerBlend call
    let mut tangencies: Vec<bool> = vec![*work_tan
        .first()
        .ok_or_else(|| null("CreateCornerBlend: tangent flags shorter than edge uses"))?];
    ordered_eus.push(work_eus.remove(0));
    let first_eu = *ordered_eus.first().ok_or_else(sm_err)?;
    eu_orients.push(eu1::orientation(b, first_eu)?);
    work_tan.remove(0);

    let mut tol_scale = 1.0;

    // While there are sWorkEUS - find the next one sharing an endPoint with the last
    while !work_eus.is_empty() {
        let mut found = false;

        // for every sWorkEUS edgeuse - look for one sharing a common endPoint with the last
        for i in 0..work_eus.len() {
            let eu = *work_eus.get(i).ok_or_else(sm_err)?;
            let tan = *work_tan
                .get(i)
                .ok_or_else(|| null("CreateCornerBlend: tangent flags shorter than edge uses"))?;
            let last_eu = *ordered_eus.last().ok_or_else(sm_err)?;
            let param = if eu_orients.last() == Some(&OrientType::Same) {
                1.0
            } else {
                0.0
            };

            // lastEdgeuse endpoint (3d Eval)
            let end_point = normalized_evaluate(k, b, last_eu, param, false, false)?;

            // TestEdgeuse StartPoint
            let test_pt = normalized_evaluate(k, b, eu, 0.0, false, false)?;

            // when LastEdge EndPoint coincides with TestEdge Start
            if test_pt.distance(end_point) < approx_tol_3d * tol_scale {
                // insert this edge into local inputs in SAME orientation
                tangencies.push(tan);
                ordered_eus.push(eu);
                eu_orients.push(OrientType::Same);
                work_eus.remove(i);
                work_tan.remove(i);
                found = true;
                tol_scale = 1.0;
                break; // go find next connected edgeuse
            }

            // TestEdgeuse EndPoint
            let test_pt = normalized_evaluate(k, b, eu, 1.0, false, false)?;

            // When LastEdge EndPoint coincides with TestEdge End
            if test_pt.distance(end_point) < approx_tol_3d * tol_scale {
                // insert this edge into local inputs in OPPOSITE orientation
                tangencies.push(tan);
                ordered_eus.push(eu);
                eu_orients.push(OrientType::Opposite);
                work_eus.remove(i);
                work_tan.remove(i);
                found = true;
                tol_scale = 1.0;
                break; // go find next connected edgeuse
            }
        }

        // arrive here when none of the remaining edges connect to the growing chain within current tol

        // exit error: A gap in the input edgeuses
        if !found && tol_scale > 100.0 {
            return Err(sm_err());
        }

        // arrive here to try again - increase check tolerance and try searching the edgeuses again.
        if !found {
            tol_scale *= 2.5;
        }
    }

    // SmSurface::CreateCornerBlend locals
    let mut ordered_uv: Vec<Option<BSplineCurve>> = Vec::new();
    let mut ordered_3d: Vec<Curve> = Vec::new();
    // (the faces whose surfaces are sOrderedSurfs)
    let mut ordered_surfs: Vec<Option<FaceId>> = Vec::new();

    // for every ordered Edgeuse - load a trimmed copy of Edgeuse->Edge->Curve to sOrdered3D
    //   and when tangency to adjacent surfaces are needed build sOrderedUV and sOrderedSurf arrays
    for (i, &eu) in ordered_eus.iter().enumerate() {
        let e =
            eu1::edge(b, eu)?.ok_or_else(|| null("CreateCornerBlend: edge use without edge"))?;
        let curve = b
            .edge(e)?
            .curve
            .as_ref()
            .ok_or_else(|| null("CreateCornerBlend: edge without curve"))?;

        // copy Edgeuse->Edge->Curve, trim the copy (may snap sIvl by tol to existing knots)
        let mut copy = curve.copy(k)?;
        let mut e_ivl = edge::interval(b, e)?;
        copy.trim(k, &mut e_ivl, TrimMode::Notify)?;

        // build the sOrdered3D array (any SmCurve, as upstream)
        ordered_3d.push(copy);

        // When Needed, put in UVTrimCurve and Surface in sOrderedUV and sOrderedSurfs arrays
        let tangent = tangencies.get(i).copied().unwrap_or(true);
        if tangent {
            // get Edgeuse->UVTrimCurve (status ignored upstream; SM_ASSERT(pUVCurve != NULL)
            // is a release no-op and the copy dereferences it), copy UVTrimCurve
            // (the status is ignored: upstream copies whatever curve it returns)
            let _ignored_status = get_or_create_uv_trim_curve(k, b, eu, false, false, true)?;
            let uv_curve = eu1::uv_trim_curve(b, eu)?.0.ok_or_else(|| {
                null("CreateCornerBlend: GetOrCreateUVTrimCurve left no UV trim curve")
            })?;
            let uv_copy = uv_curve.copy(k)?;
            let Some(uv_bspline) = bspline_part(&uv_copy) else {
                return Err(sm_err());
            };
            ordered_uv.push(Some(uv_bspline));
            let face = eu1::face(b, eu)?.ok_or_else(|| SolidError::null_pointer(OP))?; // NER

            // pF->GetSurface(): the face's own surface (kind and owner), read below once
            // the edge uses are done editing the B-rep
            ordered_surfs.push(Some(face));
        } else {
            ordered_uv.push(None);
            ordered_surfs.push(None);
        }
    }

    // ensure input arrays have at least 4 entries by inserting degenerate 'point' curves as needed
    // assume the input always starts with at least 2 curves
    for ii in 0..2usize {
        // skip inputs that already have 4 or more curve
        if ordered_3d.len() >= 4 {
            break;
        }

        // grab the second curve - pass 1: use curve end, pass 2: use curve start
        let curve_3d = ordered_3d.get(1).ok_or_else(sm_err)?;
        let mut param = if eu_orients.get(1) == Some(&OrientType::Same) {
            1.0
        } else {
            0.0
        };
        if ii == 2 {
            param = 1.0 - param; // Beginning of curve second time (never: ii < 2, as upstream)
        }
        // (upstream fetches the natural interval here and leaves it unused: the
        // evaluation below takes the raw 0/1 parameter)

        // Curve EndPoint
        let pnt = curve_3d.evaluate_point(k, param)?;

        // create a degenerate 'point' curve at the second curve's endPoint (parameterized from 0 to 1)
        let top_curve = BSplineCurve::create_point_curve(pnt)?;

        // add it to the input arrays at the proper location
        ordered_3d.insert(2 - ii, Curve::from(top_curve));
        eu_orients.insert(2 - ii, OrientType::Same);
        ordered_uv.insert(2 - ii, None);
        ordered_surfs.insert(2 - ii, None);
    }

    // move the last entry of every input array to the 1st position - GWC:Why?
    ordered_3d.rotate_right(1);
    eu_orients.rotate_right(1);
    ordered_uv.rotate_right(1);
    ordered_surfs.rotate_right(1);

    // pass the call along (FALSE = bicubic patch through curves and crossTangents)
    // the neighbour surfaces are evaluation copies (keeping their owners), so the
    // drops can hold the B-rep mutably through the face services
    let mut owned: Vec<Option<Surface>> = Vec::with_capacity(ordered_surfs.len());
    for face in &ordered_surfs {
        let copy = match face {
            Some(f) => face2::surface(b, *f)?
                .map(Surface::clone_for_evaluation)
                .transpose()?,
            None => None,
        };
        owned.push(copy);
    }
    let handles: Vec<Option<CacheObject>> = ordered_surfs
        .iter()
        .map(|face| face.map(CacheObject::Surface))
        .collect();
    let surfs: Vec<Option<BlendNeighbour<'_>>> = owned
        .iter()
        .map(|s| s.as_ref().map(|surface| BlendNeighbour { surface }))
        .collect();
    let mut services = BrepFaceServices::new(&mut *b);
    BSplineSurface::create_corner_blend_with_context(
        k,
        &ordered_3d,
        &eu_orients,
        &ordered_uv,
        &surfs,
        approx_tol_3d,
        tangency_tol_radians,
        false,
        Some((&mut services, &handles)),
    )
}

// ---- edge-to-edge blends (SmBrep::CreateEdgeEdgeBlend) ----------------------------------

/// `CreateEdgeEdgeBlend`: a ruled (`blend_flag` 0) or G1/G2/G3 blend (`1`/`2`/`3`)
/// surface between the trimmed NURBS curves of two edges: `eu_1` in `b_1`, `eu_2` in
/// `b_2` (`None`: also in `b_1`; upstream's edge uses may belong to two B-reps).
/// `curves_dir_flag`: `0`
/// guesses the pairing from the rail distances, `1` starts both at their curve starts,
/// `2` pairs start with end. `opt_ds_def` (its curvature type is overwritten from
/// `blend_flag`) tunes a blend.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a null edge use (`NER`); `SM_ERR_INVALID_INPUT` for an
/// edge use without edge, an edge without B-spline curve or a face without surface;
/// [`SolidError::InvalidInput`] for a blend edge use without face (upstream
/// dereferences it); those of the knot dampening and the ruled surface (the binormal
/// statuses are ignored as upstream).
#[allow(clippy::too_many_arguments)]
pub fn create_edge_edge_blend(
    k: &Kernel,
    blend_flag: u64,
    b_1: &mut Brep,
    eu_1: Option<EdgeuseId>,
    b_2: Option<&mut Brep>,
    eu_2: Option<EdgeuseId>,
    curves_dir_flag: u64,
    opt_ds_def: Option<&mut DerivSurfDefinition<'_>>,
) -> Result<Surface> {
    let eu_1 = eu_1.ok_or_else(|| SolidError::null_pointer(OP))?; // NER
    let eu_2 = eu_2.ok_or_else(|| SolidError::null_pointer(OP))?; // NER

    // eu_2's B-rep: `b_2`, or `b_1` when both edge uses are in one B-rep (each edge use's
    // reads touch only its own B-rep, as upstream's)
    let mut b_2 = b_2;
    macro_rules! b2_mut {
        () => {
            match b_2.as_deref_mut() {
                Some(other) => other,
                None => &mut *b_1,
            }
        };
    }
    macro_rules! b2 {
        () => {
            match b_2.as_deref() {
                Some(other) => other,
                None => &*b_1,
            }
        };
    }

    // locals
    let (Some(edge1), Some(edge2)) = (eu1::edge(b_1, eu_1)?, eu1::edge(b2!(), eu_2)?) else {
        return Err(status(SM_ERR_INVALID_INPUT)); // Could not fetch Edge from Edgeuse
    };

    // edge locals
    let ivl1 = edge::interval(b_1, edge1)?;
    let mut ivl2 = edge::interval(b2!(), edge2)?;
    let rail1 = edge::create_trimmed_nurbs_curve(k, b_1, edge1)?;
    let rail2 = edge::create_trimmed_nurbs_curve(k, b2!(), edge2)?;
    let tolerance = max2(edge::tolerance(b_1, edge1)?, edge::tolerance(b2!(), edge2)?);
    let (Some(rail1), Some(rail2)) = (rail1, rail2) else {
        return Err(status(SM_ERR_INVALID_INPUT)); // Could not build BSplineCurves from Edges
    };
    let (Some(mut rail1), Some(mut rail2)) = (bspline_part(&rail1), bspline_part(&rail2)) else {
        return Err(status(SM_ERR_INVALID_INPUT));
    };

    // to preserve previous behavior - use backward compatible style for ruled surfaces
    if blend_flag == 0 {
        // insert knots when needed to make knot spacing more regular
        if rail1.degree()? >= 3 {
            rail1.dampen_knot_spacing(2.0, 3)?;
        }
        if rail2.degree()? >= 3 {
            rail2.dampen_knot_spacing(2.0, 3)?;
        }

        // create the ruled surface - passes call to NLib N_CreateRuledSrf() which makes
        // curves compatible (knot tol = .001)
        let new_surface = BSplineSurface::create_ruled_surface(&rail1, &rail2, SurfParamType::U)?;
        return Ok(Surface::from(new_surface));
    }

    // arrive here to build a blend surface between two edges

    // make sure the input is self-consistent: m_eCurvatureType matches lBlendFlag value
    let mut ds_def = opt_ds_def;
    if let Some(d) = ds_def.as_deref_mut() {
        d.curvature_type = match blend_flag {
            1 => CurvatureType::NoCurvature,
            3 => CurvatureType::G3FromSurface,
            _ => CurvatureType::G2FromSurface,
        };
    }

    // Guess curve directions if necessary
    let same_dir_curves = match curves_dir_flag {
        1 => true,
        2 => false,
        _ => {
            // Get positions along railcurves in order of parameterization
            let num_loc = 11usize;
            let mut rail1_pos = Vec::with_capacity(num_loc);
            let mut rail2_pos = Vec::with_capacity(num_loc);
            for ii in 0..num_loc {
                // (evaluation statuses are not checked upstream: a failure leaves the
                // sample unset; undefined coordinates here)
                let t = ii as f64 / (num_loc - 1) as f64;
                rail1_pos.push(
                    ignored(rail1.evaluate_point(ivl1.evaluate(t)))?.unwrap_or(Point3::UNDEF),
                );
                rail2_pos.push(
                    ignored(rail2.evaluate_point(ivl2.evaluate(t)))?.unwrap_or(Point3::UNDEF),
                );
            }

            // Check under which orientation the distance between rail curves is less
            let mut same_direction = 0.0;
            let mut opp_direction = 0.0;
            for ((p1, p2), p2r) in rail1_pos.iter().zip(&rail2_pos).zip(rail2_pos.iter().rev()) {
                same_direction += p1.distance(*p2);
                opp_direction += p1.distance(*p2r);
            }
            same_direction <= opp_direction
        }
    };

    // face locals
    let face1 =
        eu1::face(b_1, eu_1)?.ok_or_else(|| null("CreateEdgeEdgeBlend: edge use without face"))?;
    let face2_ = eu1::face(b2!(), eu_2)?
        .ok_or_else(|| null("CreateEdgeEdgeBlend: edge use without face"))?;
    let surfaces_present = (
        face2::surface(b_1, face1)?.is_some(),
        face2::surface(b2!(), face2_)?.is_some(),
    );
    if !(surfaces_present.0 && surfaces_present.1) {
        return Err(status(SM_ERR_INVALID_INPUT)); // Could not fetch Surface from Edgeuse
    }

    // set the bIntoFace values for curve/Surface pairs
    // (both statuses ignored upstream: a failure leaves defined UNDEF / zero outputs)
    let BinormalEval {
        binormal: edge1_binormal,
        tangent: mut edge1_tangent,
        face_normal: mut face1_normal,
        ..
    } = evaluate_binormal_status_ignored(k, b_1, eu_1, ivl1.evaluate(0.5))?;
    let BinormalEval {
        binormal: edge2_binormal,
        tangent: mut edge2_tangent,
        face_normal: mut face2_normal,
        ..
    } = evaluate_binormal_status_ignored(k, b2_mut!(), eu_2, ivl2.evaluate(0.5))?;
    if eu1::orientation(b_1, eu_1)? == OrientType::Opposite {
        edge1_tangent = -edge1_tangent;
    }
    if eu1::orientation(b2!(), eu_2)? == OrientType::Opposite {
        edge2_tangent = -edge2_tangent;
    }
    let fu_orient = |b: &Brep, eu: EdgeuseId| -> Result<OrientType> {
        let fu = eu1::faceuse(b, eu)?
            .ok_or_else(|| null("CreateEdgeEdgeBlend: edge use without face use"))?;
        eu1::faceuse_orientation(b, fu)
    };
    if fu_orient(b_1, eu_1)? == OrientType::Opposite {
        face1_normal = -face1_normal;
    }
    if fu_orient(b2!(), eu_2)? == OrientType::Opposite {
        face2_normal = -face2_normal;
    }

    // set edge1 blend direction to run out of the face, edge2 blend direction to run into the face
    let curve1_left = face1_normal.cross(edge1_tangent);
    let curve2_left = face2_normal.cross(edge2_tangent);
    let mut off_to_the_right = edge1_binormal.dot(curve1_left) >= 0.0;
    let mut in_from_the_left = edge2_binormal.dot(curve2_left) < 0.0;

    // make sure two curve intervals run in the same direction and share a common parameterization
    if !same_dir_curves {
        // (status not checked upstream; the interval is left as is on failure)
        ivl2 = ignored(rail2.reverse_parameterization(&ivl2))?.unwrap_or(ivl2);
        in_from_the_left = !in_from_the_left;
    }

    let rescale = |rail2: &mut BSplineCurve, ivl2: &mut Extent1d| -> Result<()> {
        if !ivl1.are_equal(ivl2, 0.0) {
            // rescale Curve2 (the offset expression keeps upstream's operator precedence)
            let scale = (ivl1.min() - ivl1.max()) / (ivl2.min() - ivl2.max());
            let offset =
                (ivl2.min() * ivl1.max()) - (ivl1.min() * ivl2.max()) / (ivl2.min() - ivl2.max());

            let nat = rail2.natural_interval();
            let mut new_ivl = nat;
            // (neither status is checked upstream)
            ignored(new_ivl.set_min_max(nat.min() * scale + offset, nat.max() * scale + offset))?;
            ignored(rail2.edit_parameterization(&new_ivl))?;
            *ivl2 = ivl1;
        }
        Ok(())
    };
    rescale(&mut rail2, &mut ivl2)?;

    // make blends either G* or D*
    if let Some(d) = ds_def.as_deref() {
        match d.blend_end_type {
            BlendEndType::NoCusp => {}
            BlendEndType::FirstCusp => off_to_the_right = !off_to_the_right,
            BlendEndType::SecondCusp => in_from_the_left = !in_from_the_left,
            BlendEndType::BothCusp => {
                off_to_the_right = !off_to_the_right;
                in_from_the_left = !in_from_the_left;
            }
        }
    }

    // make sure two curve intervals run in the same direction and share a common
    // parameterization (upstream repeats both steps here)
    if !same_dir_curves {
        ivl2 = ignored(rail2.reverse_parameterization(&ivl2))?.unwrap_or(ivl2);
    }
    rescale(&mut rail2, &mut ivl2)?;

    // build blend surface
    let curve1 = Curve::from(rail1);
    let curve2 = Curve::from(rail2);
    // (the face surfaces fetched above; re-borrowed past the edge use evaluations)
    let surface1 = face2::surface(b_1, face1)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let surface2 = face2::surface(b2!(), face2_)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    surface1.create_blend_surface(
        k,
        &curve1,
        &ivl1,
        off_to_the_right,
        surface2,
        &curve2,
        &ivl2,
        in_from_the_left,
        tolerance,
        ds_def.as_deref(),
    )
}

// ---- parallel projections (SmBrep::CreateParallelProjectionCurves) ----------------------

/// The optional inputs of [`create_parallel_projection_curves`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ParallelProjectionOptions<'f> {
    /// `cpdOptProjectionDistance`: how far to project (through the surface when `None`).
    pub projection_distance: Option<f64>,
    /// `cpdOpt3DApproximationTol`: the 3D approximation tolerance (also the curve
    /// tolerance of the trimming).
    pub approx_tol_3d: Option<f64>,
    /// `cpdOptAngleTolRadians`.
    pub angle_tol_radians: Option<f64>,
    /// `pOptSubsetFaces`: only these faces (default every face of the B-rep).
    pub subset_faces: Option<&'f [FaceId]>,
    /// `pOpt3DProjectedCurves` wanted.
    pub want_3d: bool,
    /// `pOptUVProjectedCurves` wanted.
    pub want_uv: bool,
    /// `pOptFaceOfUVCurves` wanted.
    pub want_faces: bool,
}

/// The outputs of [`create_parallel_projection_curves`].
#[derive(Debug, Clone, Default)]
pub struct ParallelProjectionCurves {
    /// `pOpt3DProjectedCurves`: the 3D projected curves trimmed to the faces.
    pub curves_3d: Option<Vec<Curve>>,
    /// `pOptUVProjectedCurves`: the UV projected curves trimmed to the faces (`None`
    /// entries as `SmFace::CreateCurvesByTrimming` leaves them).
    pub uv_curves: Option<Vec<Option<Curve>>>,
    /// `pOptFaceOfUVCurves`: the face of each UV curve.
    pub faces_of_uv_curves: Option<Vec<FaceId>>,
}

/// `CreateParallelProjectionCurves`: projects `curve` along `projection_vector` onto
/// every face (or `subset_faces`) of the B-rep (`SmSurface::CreateParallelProjectionCurves`
/// over the face's UV domain, then `SmFace::CreateCurvesByTrimming`), collecting the
/// outputs asked for in `opts`. `curve` is one of the `SmBSplineCurve` classes.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a face without surface (upstream dereferences
/// it); those of the projection and the trimming (upstream deletes the curves made so
/// far on failure; here they are dropped with the error).
pub fn create_parallel_projection_curves(
    k: &Kernel,
    b: &mut Brep,
    curve: &Curve,
    projection_vector: Vec3,
    opts: &ParallelProjectionOptions<'_>,
) -> Result<ParallelProjectionCurves> {
    let mut out = ParallelProjectionCurves {
        curves_3d: opts.want_3d.then(Vec::new),
        uv_curves: opts.want_uv.then(Vec::new),
        faces_of_uv_curves: opts.want_faces.then(Vec::new),
    };

    let face_list: Vec<FaceId> = match opts.subset_faces {
        Some(s) => s.to_vec(),
        None => faces(b, None)?,
    };

    for face in face_list {
        let uv_domain = face2::uv_domain(b, face)?;
        // pFace->GetSurface()->CreateParallelProjectionCurves(..): the surface reaches
        // its face's cache (smsurf_GetSurfaceCache) through the B-rep, lent to the
        // projection's live view
        let query = crate::query::brep::BrepQuery::of_brep(b);
        let identity = query.effective_identity();
        let (curves_3d, uv_curves) = {
            use crate::intersect::topology_solver::p1::TopologyObjects;
            let live = crate::query::brep::LiveBrepObjects::new(b, &identity, None)?;
            let surface = live.surface(CacheObject::Surface(face))?;
            let mut context = live
                .surface_solve_context()
                .ok_or_else(|| null("CreateParallelProjectionCurves: no surface context"))?;
            surface.create_parallel_projection_curves_with_context(
                k,
                &uv_domain,
                curve,
                projection_vector,
                opts.projection_distance,
                opts.approx_tol_3d,
                opts.angle_tol_radians,
                &mut *context,
                Some(CacheObject::Surface(face)),
            )?
        };
        let curves_3d: Vec<Option<Curve>> = curves_3d.into_iter().map(Some).collect();

        let trimmed = face2::create_curves_by_trimming(
            k,
            b,
            face,
            opts.approx_tol_3d,
            Some(&curves_3d),
            Some(&uv_curves),
            true,
            true,
        )?;
        let trim_3d = trimmed.curves_3d.unwrap_or_default();
        let trim_uv = trimmed.uv_curves.unwrap_or_default();

        if let Some(fs) = out.faces_of_uv_curves.as_mut() {
            for _ in 0..trim_uv.len() {
                fs.push(face);
            }
        }
        if let Some(c) = out.curves_3d.as_mut() {
            c.extend(trim_3d);
        }
        if let Some(c) = out.uv_curves.as_mut() {
            c.extend(trim_uv);
        }
    } // for each Face

    Ok(out)
}

// ---- healing (SmBrep::HealBrep) ---------------------------------------------------------

/// The classification handle of the healer `SmBrep::HealBrep` creates locally
/// (`SmHealData sHealData`): the loops the healer classifies, and the face, surface,
/// topology and curves of `SmFace::ClassifyLoops`' loop bisector (reached through
/// `SetProps_Stage3`; a handle type that cannot name them makes `ClassifyLoops` fail
/// with [`crate::query::face::p3::NO_CURVE_HANDLES`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealLoopHandle {
    /// A B-rep entity (a loop, the face, or the vertex, edge, vertex use or edge use a
    /// classification names).
    Topo(EntityRef),
    /// The surface of a face.
    Surface(FaceId),
    /// A curve the bisector makes: its 3D curve (`false`) or its UV curve (`true`).
    Curve(bool),
    /// The curve of an edge.
    EdgeCurve(EdgeId),
    /// The UV trim curve of an edge use.
    Trim(EdgeuseId),
    /// A temporary curve ([`ClassObject::temporary`]): the seam curves of
    /// `SmFace::HasSeamProblem`.
    Temp(usize),
}

impl ClassObject for HealLoopHandle {
    fn class_kind(&self) -> ClassObjectKind {
        match self {
            Self::Topo(e) => e.class_kind(),
            Self::Surface(_) => ClassObjectKind::Surface,
            Self::Curve(_) | Self::EdgeCurve(_) | Self::Trim(_) | Self::Temp(_) => {
                ClassObjectKind::Curve
            }
        }
    }

    fn temporary(index: usize, kind: ClassObjectKind) -> Option<Self> {
        (kind == ClassObjectKind::Curve).then_some(Self::Temp(index))
    }
}

impl LoopObject for HealLoopHandle {
    fn from_loop(l: LoopId) -> Self {
        Self::Topo(EntityRef::Loop(l))
    }

    fn as_loop(&self) -> Option<LoopId> {
        match self {
            Self::Topo(e) => e.as_loop(),
            _ => None,
        }
    }

    fn as_vertex(&self) -> Option<VertexId> {
        match self {
            Self::Topo(e) => e.as_vertex(),
            _ => None,
        }
    }

    fn from_face(f: FaceId) -> Option<Self> {
        Some(Self::Topo(EntityRef::Face(f)))
    }

    fn from_face_surface(f: FaceId) -> Option<Self> {
        Some(Self::Surface(f))
    }

    fn from_classify_curve(uv: bool) -> Option<Self> {
        Some(Self::Curve(uv))
    }

    fn from_topology(t: EntityRef) -> Option<Self> {
        Some(Self::Topo(t))
    }

    fn as_topology(&self) -> Option<EntityRef> {
        match self {
            Self::Topo(t) => Some(*t),
            _ => None,
        }
    }

    fn from_edge_curve(e: EdgeId) -> Option<Self> {
        Some(Self::EdgeCurve(e))
    }

    fn as_edge_curve(&self) -> Option<EdgeId> {
        match self {
            Self::EdgeCurve(e) => Some(*e),
            _ => None,
        }
    }

    fn from_trim_curve(eu: EdgeuseId) -> Option<Self> {
        Some(Self::Trim(eu))
    }

    fn as_trim_curve(&self) -> Option<EdgeuseId> {
        match self {
            Self::Trim(eu) => Some(*eu),
            _ => None,
        }
    }
}

/// `HealBrep`: runs the healer sequence (`SmHealData::HealBrep`, on a fresh
/// [`HealData`]) up to and including `healer_op` on the given vertices, edges and
/// faces (default: all), optionally making the UV trim curves. Upstream returns
/// `SM_SUCCESS` whatever the healer reports: only [`SolidError::Unported`] (from the
/// healer's own stubs, `heal.heal_data_p3` and later) propagates.
///
/// # Errors
///
/// [`SolidError::Unported`] from the healer.
pub fn heal_brep(
    k: &mut Kernel,
    b: &mut Brep,
    healer_op: HealerOpType,
    opt_vertices: Option<&[VertexId]>,
    opt_edges: Option<&[EdgeId]>,
    opt_faces: Option<&[FaceId]>,
    make_uv_trim_curves: bool,
) -> Result<()> {
    let mut heal_data = HealData::<HealLoopHandle>::default();
    // pass the call along (status ignored upstream)
    ignored(heal_data.heal_brep(
        k,
        Some(b),
        healer_op,
        HealBrepTargets {
            vertices: opt_vertices,
            edges: opt_edges,
            faces: opt_faces,
        },
        make_uv_trim_curves,
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::a_object;
    use crate::topology::Region;

    fn cross_body_face(k: &mut Kernel, surface: Surface) -> Brep {
        use crate::geom::tol;
        use crate::topology::brep::p2::ArenaNotifyState;
        use crate::topology::brep::p8::{create_face_from_surface, FaceBuildEnv};
        use crate::topology::brep_cache::{arena_identity, CacheObject};
        use crate::topology::topology_traverser::Marks;

        let mut b =
            crate::topology::brep::p3::new_brep(k, tol::USE_DEFAULT, tol::USE_DEFAULT).unwrap();
        let mut state = ArenaNotifyState::new(&b);
        let mut env = state.env(None);
        let mut marks = Marks::default();
        let key = |f| arena_identity(CacheObject::Face(f));
        let mut fenv = FaceBuildEnv {
            env: &mut env,
            marks: &mut marks,
            this_face: &key,
        };
        create_face_from_surface(
            k,
            &mut b,
            Some(surface),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            &mut fenv,
        )
        .unwrap();
        assert_eq!(
            (b.faces().len(), b.edges().len(), b.vertices().len()),
            (1, 4, 4)
        );
        b
    }

    fn cross_body_pair(k: &mut Kernel, z: f64) -> (Brep, Brep) {
        use crate::math::Point2;
        use crate::nurbs::{math::CPoint, srf_basic::Surface as NurbSurface};
        use crate::surface::plane::Plane;

        let q = [0.25, -0.25, 0.25];
        let net = (0..3)
            .map(|i| {
                (0..3)
                    .map(|j| CPoint::new(i as f64 / 2.0, j as f64 / 2.0, q[i] + q[j], 1.0))
                    .collect()
            })
            .collect();
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let paraboloid = Surface::from(BSplineSurface::from_nurb(
            NurbSurface::new(net, 2, 2, knots.clone(), knots).unwrap(),
            false,
        ));
        let plane = Surface::from(Plane::new(
            Point3::new(0.0, 0.0, z),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ));
        for (u, v) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (0.25, 0.75)] {
            let uv = Point2::new(u, v);
            assert_eq!(
                paraboloid.evaluate_point(k, uv).unwrap(),
                Point3::new(u, v, (u - 0.5) * (u - 0.5) + (v - 0.5) * (v - 0.5))
            );
            assert_eq!(plane.evaluate_point(k, uv).unwrap(), Point3::new(u, v, z));
        }
        (cross_body_face(k, paraboloid), cross_body_face(k, plane))
    }

    #[test]
    fn brep_intersect_mut_empty_controls_and_point_options() {
        let mut k = Kernel::default();
        // Native also returns no singular point for this exact tangency.
        for z in [0.0, -0.125] {
            for reverse in [false, true] {
                for want_points in [false, true] {
                    let (mut b, mut other) = cross_body_pair(&mut k, z);
                    let (first, second) = if reverse {
                        (&mut other, &mut b)
                    } else {
                        (&mut b, &mut other)
                    };
                    let (curves, points) =
                        brep_intersect_mut(&k, first, second, 1e-6, 2.0, want_points).unwrap();
                    assert!(curves.is_empty());
                    assert_eq!(points, want_points.then(Vec::new));
                }
            }
        }
    }

    #[test]
    fn brep_intersect_mut_transverse_geometry_and_reversal() {
        use crate::curve::curve::CurveKind;

        let mut k = Kernel::default();
        // Real native samples for the same +X/+Y finite-plane parameterization.
        let native_samples = [
            (0.0, Point3::new(0.25, 0.25, 0.12499999999999994)),
            (
                0.17,
                Point3::new(0.5986382760814888, 0.16048491945102275, 0.12500000000000014),
            ),
            (
                0.37,
                Point3::new(0.8533789339730656, 0.4888946399516082, 0.12500000000000017),
            ),
            (
                0.5,
                Point3::new(0.7499999845540126, 0.7500000141604762, 0.12500000000000014),
            ),
            (
                0.83,
                Point3::new(0.160484902323279, 0.5986382191973311, 0.1250000000000001),
            ),
            (1.0, Point3::new(0.25, 0.25, 0.12499999999999994)),
        ];
        for reverse in [false, true] {
            for want_points in [false, true] {
                let (mut b, mut other) = cross_body_pair(&mut k, 0.125);
                let (first, second) = if reverse {
                    (&mut other, &mut b)
                } else {
                    (&mut b, &mut other)
                };
                let (curves, points) =
                    brep_intersect_mut(&k, first, second, 1e-6, 2.0, want_points).unwrap();
                assert_eq!(points, want_points.then(Vec::new));
                assert_eq!(curves.len(), 1);
                let curve = &curves[0];
                assert!(matches!(curve.kind(), CurveKind::BSpline(_)));
                let domain = curve.natural_interval();
                assert_eq!(domain.min(), 0.0);
                assert!((domain.max() - 2.2213861199585656).abs() <= 1e-9 * domain.max());
                for (fraction, expected) in native_samples {
                    let t = domain.min() + (domain.max() - domain.min()) * fraction;
                    let p = curve.evaluate_point(&k, t).unwrap();
                    assert!((0.0..=1.0).contains(&p.x) && (0.0..=1.0).contains(&p.y));
                    assert!((p.z - 0.125).abs() <= 1e-9 * p.z.abs());
                    // The curve approximates the circle on the paraboloid to the
                    // requested 3D approximation tolerance, regardless of orientation.
                    assert!(((p.x - 0.5).powi(2) + (p.y - 0.5).powi(2) - 0.125).abs() <= 1e-6);
                    if !reverse {
                        for (actual, expected) in
                            [(p.x, expected.x), (p.y, expected.y), (p.z, expected.z)]
                        {
                            assert!(
                                (actual - expected).abs()
                                    <= 1e-9 * actual.abs().max(expected.abs())
                            );
                        }
                    }
                }
                let start = curve.evaluate_point(&k, domain.min()).unwrap();
                let end = curve.evaluate_point(&k, domain.max()).unwrap();
                assert!((start - end).length() <= 1e-9);
            }
        }
    }

    fn seam_fixture() -> (Brep, EdgeId, [EdgeuseId; 4], [EdgeuseId; 2]) {
        use crate::curve::line::Line;
        use crate::surface::plane::Plane;
        use crate::topology::{Edgeuse, EdgeuseOwner, Faceuse, Loopuse};

        let trim = |x| {
            Curve::from(Line::from_points(
                Point3::new(x, 0.0, 0.0),
                Point3::new(x, 1.0, 0.0),
                2,
            ))
        };
        let mut b = Brep::new();
        let shell = b.add_shell().unwrap();
        let face = b.add_face(shell).unwrap();
        b.face_mut(face).unwrap().surface = Some(Surface::from(Plane::from_normal(
            Point3::ZERO,
            Point3::new(0.0, 0.0, 1.0),
        )));
        let fu = b
            .add_faceuse_record(Faceuse {
                face: Some(face),
                shell: Some(shell),
                ..Faceuse::default()
            })
            .unwrap();
        let lu = b
            .add_loopuse_record(Loopuse {
                faceuse: Some(fu),
                ..Loopuse::default()
            })
            .unwrap();
        let v = b.add_vertex(Point3::ZERO).unwrap();
        let edge = b.add_edge(v, v).unwrap();
        let uses = std::array::from_fn(|i| {
            b.add_edgeuse_record(Edgeuse {
                edge: Some(edge),
                owner: EdgeuseOwner::Loopuse(Some(lu)),
                orientation: if i % 2 == 0 {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                },
                uv_trim_curve: (i % 2 == 0).then(|| trim((i / 2) as f64)),
                ..Edgeuse::default()
            })
            .unwrap()
        });
        b.edge_mut(edge).unwrap().edgeuses = uses.to_vec();
        let previous = std::array::from_fn(|i| {
            b.add_edgeuse_record(Edgeuse {
                orientation: OrientType::Same,
                uv_trim_curve: Some(trim((1 - i) as f64)),
                ..Edgeuse::default()
            })
            .unwrap()
        });
        b.edgeuse_mut(uses[0]).unwrap().cw = Some(previous[0]);
        b.edgeuse_mut(uses[3]).unwrap().cw = Some(previous[1]);
        (b, edge, uses, previous)
    }

    fn redistribute_fixture(b: &mut Brep, edge: EdgeId) -> Result<bool> {
        let trim = Curve::from(crate::curve::line::Line::from_points(
            Point3::ZERO,
            Point3::new(1.0, 0.0, 0.0),
            2,
        ));
        redistribute_seam_uv_trim_curves(
            &Kernel::default(),
            b,
            edge,
            Some(&trim),
            SurfParamType::U,
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        )
    }

    fn seam_sides(b: &Brep, uses: [EdgeuseId; 4]) -> [f64; 2] {
        [uses[0], uses[3]].map(|eu| {
            eu1::uv_trim_curve(b, eu)
                .unwrap()
                .0
                .unwrap()
                .evaluate_point(&Kernel::default(), 0.0)
                .unwrap()
                .x
        })
    }

    #[test]
    fn seam_redistribution_preserves_trims_after_failed_projection() {
        use crate::surface::surface::p2::DropCurveFail;

        for failed_previous in 0..2 {
            let (mut b, edge, uses, previous) = seam_fixture();
            b.edgeuse_mut(previous[failed_previous])
                .unwrap()
                .drop_curve_fail = Some(Box::new(DropCurveFail {
                fail_status: SM_ERR_INVALID_INPUT,
                ..DropCurveFail::default()
            }));
            assert!(matches!(
                redistribute_fixture(&mut b, edge),
                Err(SolidError::Upstream {
                    code: SM_ERR_INVALID_INPUT,
                    ..
                })
            ));
            assert_eq!(seam_sides(&b, uses), [0.0, 1.0]);
        }
    }

    #[test]
    fn seam_redistribution_requires_four_uses_and_usable_trims() {
        let (b, edge, uses, previous) = seam_fixture();
        for count in [0, 2, 6] {
            let mut bad = b.clone();
            bad.edge_mut(edge).unwrap().edgeuses = uses.into_iter().cycle().take(count).collect();
            assert!(redistribute_fixture(&mut bad, edge).is_err());
        }
        for eu in [uses[0], uses[2], previous[0], previous[1]] {
            let mut bad = b.clone();
            bad.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(Curve::from(BSplineCurve::default()));
            assert!(redistribute_fixture(&mut bad, edge).is_err());
        }
        for eu in [uses[0], uses[2]] {
            let mut bad = b.clone();
            bad.edgeuse_mut(eu).unwrap().uv_trim_curve = None;
            let before = format!("{bad:?}");
            assert!(redistribute_fixture(&mut bad, edge).is_err());
            assert_eq!(format!("{bad:?}"), before);
        }
        let mut good = b;
        assert!(redistribute_fixture(&mut good, edge).unwrap());
        assert_eq!(seam_sides(&good, uses), [1.0, 0.0]);
        assert!(!redistribute_fixture(&mut good, edge).unwrap());
    }

    fn reflection_fixture() -> (Brep, FaceId, [crate::topology::FaceuseId; 2], [ShellId; 2]) {
        use crate::topology::shell::{ShellMembers, ShellType};
        use crate::topology::Faceuse;

        let mut b = Brep::new();
        let shells = [b.add_shell().unwrap(), b.add_shell().unwrap()];
        let face = b.add_face(shells[0]).unwrap();
        let uses = std::array::from_fn(|i| {
            b.add_faceuse_record(Faceuse {
                face: Some(face),
                shell: Some(shells[i]),
                ..Faceuse::default()
            })
            .unwrap()
        });
        for i in 0..2 {
            b.faceuse_mut(uses[i]).unwrap().mate = Some(uses[1 - i]);
            let shell = b.shell_mut(shells[i]).unwrap();
            shell.shell_type = ShellType::Faceuse;
            shell.members = ShellMembers::Faceuses(vec![uses[i]]);
        }
        b.face_mut(face).unwrap().faceuse = Some(uses[0]);
        (b, face, uses, shells)
    }

    #[test]
    fn reflection_updates_shell_members_and_links() {
        use crate::topology::shell::faceuse_sibling_links;

        let (mut b, face, uses, shells) = reflection_fixture();
        swap_reflection_shells(&mut b, face).unwrap();
        for i in 0..2 {
            assert_eq!(
                b.shell(shells[i]).unwrap().get_faceuses().unwrap(),
                vec![uses[1 - i]]
            );
            assert_eq!(b.faceuse(uses[i]).unwrap().shell, Some(shells[1 - i]));
            assert_eq!(
                faceuse_sibling_links(&b, uses[i]).unwrap(),
                (Some(uses[i]), Some(uses[i]))
            );
        }
        swap_reflection_shells(&mut b, face).unwrap();
        for i in 0..2 {
            assert_eq!(
                b.shell(shells[i]).unwrap().get_faceuses().unwrap(),
                vec![uses[i]]
            );
        }
    }

    #[test]
    fn reflection_rejects_bad_lists_before_mutation() {
        use crate::topology::shell::ShellMembers;

        for bad_member in 0..2 {
            let (mut b, face, uses, shells) = reflection_fixture();
            b.shell_mut(shells[bad_member]).unwrap().members = ShellMembers::Empty;
            let before = format!("{b:?}");
            assert!(swap_reflection_shells(&mut b, face).is_err());
            assert_eq!(format!("{b:?}"), before);
            assert_eq!(b.faceuse(uses[0]).unwrap().shell, Some(shells[0]));
        }
    }

    #[test]
    fn reflection_edgeuses_require_live_members_of_the_edge() {
        let (b, edge, uses, _) = seam_fixture();
        let mut good = b.clone();
        reverse_reflection_edgeuses(&mut good, edge).unwrap();
        assert_eq!(
            good.edge(edge).unwrap().edgeuses,
            [uses[1], uses[0], uses[3], uses[2]]
        );
        for bad_member in 0..4 {
            let mut bad = b.clone();
            bad.edgeuse_mut(uses[bad_member]).unwrap().edge = None;
            assert!(reverse_reflection_edgeuses(&mut bad, edge).is_err());
            assert_eq!(bad.edge(edge).unwrap().edgeuses, uses);
        }
        let mut bad = b;
        bad.edge_mut(edge).unwrap().edgeuses.pop();
        let before = bad.edge(edge).unwrap().edgeuses.clone();
        assert!(reverse_reflection_edgeuses(&mut bad, edge).is_err());
        assert_eq!(bad.edge(edge).unwrap().edgeuses, before);
    }

    #[test]
    fn type_queries() {
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(a_object::SAG_TYPE));
        assert!(is_kind_of(a_object::TYPE));
        assert!(!is_kind_of(16012));
        assert_eq!(CLASS_STRING, "SmBrep");
    }

    #[test]
    fn region_list_rotates_to_head() {
        let mut b = Brep::new();
        let r: Vec<RegionId> = (0..3)
            .map(|_| {
                b.add_region_record(Region {
                    in_brep: true,
                    ..Region::default()
                })
                .unwrap()
            })
            .collect();
        let off = b
            .add_region_record(Region {
                in_brep: false,
                ..Region::default()
            })
            .unwrap();
        assert_eq!(regions(&b, None).unwrap(), r);
        b.set_region_list_head(Some(r[1]));
        assert_eq!(regions(&b, None).unwrap(), vec![r[1], r[2], r[0]]);
        assert!(!regions(&b, None).unwrap().contains(&off));
        assert!(!is_live_topology_member(&b, None));
    }

    #[test]
    fn votes() {
        let mut rs = Vec::new();
        let mut vs = Vec::new();
        let (a, c) = (Some(RegionId::from_index(0)), Some(RegionId::from_index(1)));
        assert_eq!(find_highest_vote(&rs, &vs).unwrap(), None);
        vote_for_region(&mut rs, &mut vs, a);
        vote_for_region(&mut rs, &mut vs, c);
        vote_for_region(&mut rs, &mut vs, c);
        assert_eq!((rs.clone(), vs.clone()), (vec![a, c], vec![1, 2]));
        assert_eq!(find_highest_vote(&rs, &vs).unwrap(), c);
        vote_for_region(&mut rs, &mut vs, a);
        // ties keep the first
        assert_eq!(find_highest_vote(&rs, &vs).unwrap(), a);
        // the null region is a region of its own; a null winner reads None, as upstream
        vote_for_region(&mut rs, &mut vs, None);
        vote_for_region(&mut rs, &mut vs, None);
        vote_for_region(&mut rs, &mut vs, None);
        assert_eq!((rs.clone(), vs.clone()), (vec![a, c, None], vec![2, 2, 3]));
        assert_eq!(find_highest_vote(&rs, &vs).unwrap(), None);
        assert!(find_highest_vote(&rs, &[1]).is_err());
    }

    #[test]
    fn size_estimates_follow_the_context() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        assert_eq!(b.this_model_size_estimate(&k, false), tol::USE_DEFAULT);
        assert_eq!(
            b.this_model_size_estimate(&k, true),
            tol::MODEL_SIZE_ESTIMATE
        );
        assert_eq!(b.set_this_model_size_estimate(&mut k, 1000.0), 1.0e-4);
        assert_eq!(b.tolerance(), 1.0e-4);
        assert_eq!(k.context.stored_model_size_estimate(), 1000.0);
        b.set_this_large_small_size_ratio(&mut k, 7.0);
        assert_eq!(b.this_large_small_size_ratio(&k, false), 7.0);
        assert_eq!(k.context.stored_large_small_size_ratio(), 7.0);
    }

    #[test]
    fn object_maps() {
        let (a, c) = (
            EntityRef::Face(FaceId::from_index(0)),
            EntityRef::Face(FaceId::from_index(1)),
        );
        let mut map = ObjectMap::new();
        let mut copy = CopyBrepMap::new();
        let mut rev = CopyBrepMap::new();
        set_object_map(a, Some(c), &mut map, Some(&mut copy), Some(&mut rev));
        set_object_map(c, None, &mut map, Some(&mut copy), None);
        assert_eq!(get_object_map(a, &map), Some(c));
        assert_eq!(get_object_map(c, &map), None);
        assert_eq!(copy.get_at(Some(c)).unwrap(), Some(a));
        assert_eq!(rev.get_at(Some(a)).unwrap(), Some(c));
        assert_eq!(copy.len(), 1);
    }
}
