//! `SmFace` queries, part 2: classification of a parameter point
//! (`SmFace::PointClassify`) and of a curve (`SmFace::CurveOnClassify`) against the
//! face boundary, the two-face parameter classification of the proto topology
//! (`sm_ClassifyParamOnFaces`), and the point containment of a proto vertex.
//!
//! Upstream: `source/SMLib/src/SmFace.cpp`, `source/SMLib/src/SmProtoTopology.cpp`,
//! `source/SMLib/inc/SmProtoTopology.h`.
//!
//! | Rust                                  | Upstream                                          |
//! |---------------------------------------|---------------------------------------------------|
//! | [`point_classify`], [`point_classify_prepared`], [`PointClassifyOptions`] | `SmFace::PointClassify` |
//! | [`curve_on_classify`], [`CurveOnClassifyObjects`] | `SmFace::CurveOnClassify`             |
//! | [`curve_on_classify_mut`], [`CurveOnClassifyBrep`], [`FaceBrep`] | `SmFace::CurveOnClassify` on the face's own B-rep |
//! | [`classify_param_on_faces`]           | `sm_ClassifyParamOnFaces` (file static)           |
//! | [`proto_vertex_distance_to`]          | `SmProtoVertex::DistanceTo`                       |
//! | [`proto_vertex_capture_distance`]     | `SmProtoVertex::GetCaptureDistance`               |
//! | [`proto_vertex_contains_point`]       | `SmProtoVertex::ContainsPoint`                    |
//! | [`proto_vertex_containment_radius`]   | fork `SmProtoVertex::GetContainmentRadius`        |
//! | [`ProtoTopologyManager::capture_distance`] | `SmProtoTopologyManager::GetCaptureDistance` (already in `topology::proto_topology`) |
//! | [`face_vertices`]                     | `SmFace::GetVertices(SmTArray&, ..)` (CO-PORTED, `topology.face_p4`) |
//! | `ClassObject for EntityRef`           | the `SmObject*` of a classification of B-rep topology |
//! | [`BrepCacheFace`] | `GetFace()` of a face surface's trimmed cache (`SmTrimSrfCache::BuildTree`, `PointClassify`) |
//! | [`face_surface_cache`], [`FaceSurfaceCache`], [`note_face_surface_cache`] | `smsurf_GetSurfaceCache(GetSurface())` through `SmCacheMgrTSrf::CacheMakeOrValidate` |
//! | [`owned_surface_cache`], [`solve_surface_cache_of_face`] | `smsurf_GetSurfaceCache(pSurface)` / `GetTree` of a face-owned surface (a solve's cache) |
//! | [`surface_cache_state_of_face`] | `smsurf_GetSurfaceCache(pSurface)` of a face-owned surface without `GetTree` |
//! | [`face_cache_tree`] | `SmSurfaceCache::GetTree` of a face surface's trimmed cache |
//!
//! Deviations:
//! - **Opt-in fix (WR-305, #1057).** With `Kernel::fixes.classify_edge_interval`,
//!   `CurveOnClassify` runs the fork's `ClassifyEdgeInterval` on each interval before
//!   `ClassifyFaceInterval` (`CurveClassification::classify_edge_interval`).
//! - **Classification B-rep (temporary).** [`curve_on_classify_mut`] runs on the face's own
//!   B-rep through [`CurveOnClassifyBrep`] and makes upstream's lazy edits (UV trim curves,
//!   gap caches, the trimmed cache's tolerance growth) on it in place, in upstream order.
//!   Callers that still hold only `&Brep` use [`curve_on_classify`] /
//!   `curve_on_classify_cached`, which run the same code on a private copy of the B-rep
//!   (`SnapshotObjects` over `CurveOnClassifyCache`'s snapshot; the healer copies some of
//!   its edits back). That entry goes once its last callers are converted: the stage 3 /
//!   `ClassifyLoops` path, the `intersect::topology_solver` (its face silhouettes'
//!   `CreateCurvesByTrimming`, through `topology::face::p2`'s snapshot entry) and
//!   `blend::fillet_solver` classifications, `sm_CountContainedSamples`'
//!   `CalculateAnInternalPoint` in `query::api_queries`, the `&self`
//!   `ProtoBrep::face_curve_on_classify` (proto-topology's coincident-curve
//!   classifications), `topology::face::p3`'s `FaceBreps::Shared` callers, and the fillet
//!   session classifiers.
//! - **Object model.** The face is a record of a [`Brep`]; the functions take the
//!   B-rep and the face's id. Classification objects are [`EntityRef`]s (the face, edge
//!   or vertex). `SmFace::PointClassify`'s status is the result: `Ok(())` for
//!   `SM_SUCCESS`, [`SolidError::Upstream`] `SM_ERR` when no loop classifies the point
//!   (the classification is still updated, as upstream).
//! - **Unused parameters.** `PointClassify`'s `sSrcZoneTol3d`,
//!   `bDoBoundaryIntersections` and `bUVSpaceOkay` are `NotUsed` upstream; they are
//!   kept in [`PointClassifyOptions`] and ignored. `bUseUVClassification` only chooses
//!   between two identical `SM_ERR` returns.
//! - **Trimmed surface cache.** A face surface keeps its trimmed cache in its cache
//!   slot ([`Surface::take_trim_cache`]; upstream's `SM_OC_TRIMSRF` queue entry), made
//!   by the first [`face_surface_cache`] and reused by later ones as
//!   `CacheMakeOrValidate` reuses it (whether it was built with the face or
//!   standalone, while the owner was cleared). A lookup made while the cache is in use
//!   (taken out of the slot) gets a cache built the same way that is not kept. The
//!   global cache queue's size limit (200 trimmed caches, least recently used out) is
//!   not modelled. `SmCacheMgr::GetObjectCache(SM_OC_TRIMSRF, surface)` of
//!   `PointClassify` is the caller's [`PointClassifyOptions::trim_cache`], else the
//!   surface's kept cache (none while it is in use). The test
//!   `GetSurface()->GetFace() == this` is "the face's surface has a face owner"
//!   (the owner of a face's surface is that face).
//! - **Out-of-bounds evaluation.** `SmTemporaryChangeValue` on the surface's
//!   `GetOutOfBoundsEnabled()` becomes a copy of the surface with out-of-bounds
//!   evaluation enabled (B-spline kinds only), used for the evaluation and the loop
//!   containment tests; the B-rep is not changed.
//! - **Loop containment.** [`crate::query::loop::contains_uv_point`] walks the loop in
//!   the B-rep, which therefore is taken mutably (`SmEdgeuse::GetOrCreateUVTrimCurve`
//!   attaches the UV trim curves the loops lack); [`point_classify`] and
//!   [`classify_param_on_faces`] take `&mut Brep`. The surface it is given is an
//!   owned copy of the face's. Its zone tolerance (`SmTol::GetZoneTol3d(pSurface)`) is
//!   the loop face's tolerance. An edge use without edge or an edge without curve is
//!   [`SolidError::InvalidInput`] (upstream dereferences them).
//!   [`point_classify_prepared`] shares the same walk over read-only topology;
//!   missing UV trims or a required cache-tree build report [`SolidError::Unported`]
//!   only when reached. Evaluation copies retain exact NURBS data and tolerances.
//! - **Ignored statuses.** `Evaluate1stDerivatives` of the surface and `EvaluatePoint`
//!   of the on-boundary edge curve: on failure the point stays `SmPoint3d()`
//!   ([`Vec3::UNDEF`]); [`SolidError::Unported`] still propagates. The `PointClassify`
//!   statuses `sm_ClassifyParamOnFaces` ignores are ignored the same way.
//!   Part-9 boundary views also ignore ordinary `ComputeUVPoint` failures and keep
//!   the initialized [`Point2::UNDEF`]; unported errors still propagate.
//! - **Null dereferences** upstream (a face, loop, edge use or vertex use without the
//!   object it points to) are [`SolidError::InvalidInput`]; a face without surface is
//!   `NER` ([`SolidError::null_pointer`]).
//! - **`sm_ClassifyParamOnFaces`** takes the faces' tolerances for
//!   `SmTol::GetZoneTol3d_ForXSectResult(pFace1, pFace2)` (`SM_USE_OLDTOL`:
//!   `SmFace::GetTolerance`).
//! - **`SmProtoVertex`** keeps no pointer to its manager; the manager is passed.
//! - **`CurveOnClassify`** stores handles of the caller's choosing in the curve
//!   classification; [`CurveOnClassifyObjects`] makes the face and surface handles and
//!   resolves curve and loop handles, and the caller's [`ClassGeometry`] resolves them
//!   for `ClassifyFaceInterval` (`lFaceIndx` left at its default).
//!   `SmCurveClassification::ReSet` is [`CurveClassification::reset`]; its callees
//!   `IntersectTSBound` / `IntersectEdges` (part 9, wired through borrowed boundary
//!   views) and `SmFace::GetEdges` (`topology.face_p4`) retain their dependency
//!   boundaries. `pbOptModifiedCurve` is the result. `IntersectVertices` and
//!   `ClassifyFaceInterval` call their landed implementations through the caller's
//!   classification handles and [`ClassGeometry`] callbacks.
//! - Boundary views use required handle mappings and the same [`ClassGeometry`]
//!   implementation as part 5. Lazy UV and gap calculations run on a private B-rep
//!   snapshot. Mutable healer callers explicitly retain its vertex/face and edge/face
//!   gap-cache writes, including invalidations before errors; read-only callers discard
//!   them. Geometry, topology and UV trim edits are not copied back.
//! - **Vertex/face gaps.** With `bHasBigGaps`, `IntersectVertices` reads each vertex's
//!   `GetVertexuseOfFace(pFace)->GetVertexFaceGap()` length. [`curve_on_classify`]
//!   computes them from the B-rep (on one private snapshot per call, which keeps the
//!   gap caches across both passes) for the face's vertices before the vertex
//!   intersections, and answers [`ClassGeometry::vertex_face_gap`] for them around the
//!   caller's [`ClassGeometry`]; a gap error therefore surfaces before the earlier
//!   vertices are intersected (upstream: a null dereference or the same unported
//!   callee). A vertex without a face use is unresolved (upstream dereferences null).
//! - **`IntersectTSBound`** fetches the face surface's trimmed cache itself
//!   ([`face_surface_cache`] on the classification snapshot, whose face surfaces share
//!   the B-rep's cache slots: [`Brep::clone_sharing_surface_caches`]); when it is up to
//!   date the cached branch visits only the vertex uses and edge uses of the leaf nodes
//!   the curve's box meets, under a context mark
//!   ([`crate::geom::context::Context::claim_mark`]), and makes a visited edge use's
//!   missing UV trim curve on the snapshot (`GetOrCreateUVTrimCurve`).
//!   `IntersectVertices` / `IntersectEdges` clear the surface's owner while they read
//!   the vertex/face and edge/face gaps, so the gap reads run in an owner-cleared scope
//!   ([`Brep::begin_surface_owner_cleared`]) and a cache they make is standalone.
//! - The UV trim curves `IntersectTSBound`'s `ComputeUVPoint` calls make there (upstream
//!   on the face's own B-rep) stay visible for the rest of the classification through
//!   the private `WithMadeTrimCurves` geometry. The gap caches and these trim curves
//!   share one snapshot per call, as upstream mutates the one B-rep. The healer shares
//!   that snapshot across both seam directions before applying its gap-cache values.
//!   `WithMadeTrimCurves` forwards the topology hooks (`topology_entity`,
//!   `topology_handle(_in)`, `brep_id`, `owner_entity`, the intersector mate) to the
//!   caller's geometry with the face's B-rep resolved in the snapshot, and is the
//!   [`ClassGeometryMut`] of `ClassifyFaceInterval`
//!   ([`CurveClassification::classify_face_interval_mut`]): its face queries edit the
//!   snapshot, so their UV trims and tolerance changes stay for the rest of the
//!   classification (not for the caller's B-rep).
//! - **Vertex index (#431, fork `ef9041d`).** `SmBrep::AssertValid` classifies every
//!   vertex of a face on it, and each `PointClassify` listed the face's vertices
//!   again. `point_classify_indexed` takes a `FaceVertexIndex` built once per face
//!   and tests first the vertices a box search finds near the point (all of them
//!   when none of those is at it).
//! - No recursion.

use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignored, EntityRef, Result, SolidError, SM_ERR};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::point_set::PointContainment;
use crate::geom::tree::{NodeClass, NodeData};
use crate::intersect::curve_class::p1::{src_zone_tol_3d_of, CurveClassification};
use crate::intersect::curve_class::p2::ResetOptions;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::curve_class::p9::{
    BoundaryEdge, BoundaryEdgeuse, BoundaryFace, BoundaryLoop, BoundaryVertex,
    IntersectEdgesOptions,
};
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, ClassObjectKind, PointClassType, PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{max2, Point2, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::query::r#loop::{contains_uv_point_with_topology, UvContainment};
use crate::surface::surface::p1::{
    get_surface_cache_with_face, get_trim_surface_cache, make_surface_cache, make_trim_srf_cache,
    make_trim_srf_cache_with_face,
};
use crate::surface::surface::{CacheTreeState, Surface, SurfaceKind, TrimCacheSlot};
use crate::surface::surface_cache::p1::{FaceContainment, SurfaceCache};
use crate::surface::trim_srf_cache::{CacheFace, TrimSrfCache};
use crate::topology::brep::validation::spatial::SpatialIndex;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p2 as fp2;
use crate::topology::loopuse;
use crate::topology::loopuse::QueryTopology;
use crate::topology::proto_topology::{ProtoTopologyManager, ProtoVertex};
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, LoopId, VertexId, VertexuseId};

const OP_CLASSIFY: &str = "query::face::p2::point_classify";
const OP_CURVE: &str = "query::face::p2::curve_on_classify";

impl ClassObject for EntityRef {
    fn class_kind(&self) -> ClassObjectKind {
        match self {
            Self::Face(_) => ClassObjectKind::Face,
            Self::Edge(_) => ClassObjectKind::Edge,
            Self::Vertex(_) => ClassObjectKind::Vertex,
            Self::Loop(_) => ClassObjectKind::Loop,
            Self::Region(_) => ClassObjectKind::Region,
            Self::Faceuse(_) => ClassObjectKind::Faceuse,
            Self::Loopuse(_) => ClassObjectKind::Loopuse,
            Self::Edgeuse(_) => ClassObjectKind::Edgeuse,
            Self::Vertexuse(_) => ClassObjectKind::Vertexuse,
            Self::Brep(_) | Self::Shell(_) => ClassObjectKind::Other,
        }
    }
}

fn null(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::invalid_input(operation, reason)
}

// ---- the face of a face surface's trimmed cache ------------------------------------------

const OP_CACHE_FACE: &str = "query::face::p2::BrepCacheFace";

/// A face of a B-rep as the owning face of its surface's trimmed surface cache
/// ([`CacheFace`]: what `SmTrimSrfCache` reads through `GetFace()`). Handles are
/// [`EntityRef`]s; the B-rep is borrowed mutably because `CreateUVTrimCurves` and
/// `ComputeUVPoint` attach UV trim curves and the classification may too.
#[derive(Debug)]
pub struct BrepCacheFace<'a> {
    /// The B-rep holding the face.
    pub b: &'a mut Brep,
    /// The face.
    pub f: FaceId,
}

fn cache_face_entity(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP_CACHE_FACE, reason)
}

impl CacheFace<EntityRef> for BrepCacheFace<'_> {
    fn face(&self) -> EntityRef {
        EntityRef::Face(self.f)
    }

    fn brep_editing_enabled(&self) -> Result<bool> {
        Ok(self.b.editing_enabled())
    }

    fn set_brep_editing_enabled(&mut self, enabled: bool) -> Result<()> {
        self.b.set_editing_enabled(enabled);
        Ok(())
    }

    fn create_uv_trim_curves(&mut self, k: &Kernel) -> Result<()> {
        // (TRUE, NULL, NULL, ..): adjust tolerances, no given curves
        crate::topology::face::p3::create_uv_trim_curves(k, self.b, self.f, true, None).map(|_| ())
    }

    fn upward_loopuses(&self) -> Result<Vec<EntityRef>> {
        Ok(
            crate::topology::face::p4::upward_loopuses(self.b, self.f, false, None)?
                .into_iter()
                .map(EntityRef::Loopuse)
                .collect(),
        )
    }

    fn loopuse_edgeuses(&self, loopuse: EntityRef) -> Result<Vec<EntityRef>> {
        let EntityRef::Loopuse(lu) = loopuse else {
            return Err(cache_face_entity("not a loop use"));
        };
        Ok(loopuse::edgeuses(self.b, lu, None, None)?
            .into_iter()
            .map(EntityRef::Edgeuse)
            .collect())
    }

    fn loopuse_vertexuses(&self, loopuse: EntityRef) -> Result<Vec<EntityRef>> {
        let EntityRef::Loopuse(lu) = loopuse else {
            return Err(cache_face_entity("not a loop use"));
        };
        let mut out = Vec::new();
        for vu in loopuse::vertexuses(self.b, lu, None)? {
            // a null vertex use is dereferenced by ImplantVertexuse upstream
            let vu = vu.ok_or_else(|| cache_face_entity("null vertex use"))?;
            out.push(EntityRef::Vertexuse(vu));
        }
        Ok(out)
    }

    fn edgeuse_uv_trim_curve(&self, edgeuse: EntityRef) -> Result<Option<BSplineCurve>> {
        let EntityRef::Edgeuse(eu) = edgeuse else {
            return Err(cache_face_entity("not an edge use"));
        };
        // SM_CAST_PTR(SmBSplineCurve, ..): the analytic curves derive from it upstream
        Ok(match eu1::uv_trim_curve(self.b, eu)?.0.map(Curve::kind) {
            Some(CurveKind::BSpline(c)) => Some(c.clone_exact()),
            Some(CurveKind::Line(c)) => Some(c.base.clone_exact()),
            Some(CurveKind::Ellipse(c)) => Some(c.base().clone_exact()),
            Some(CurveKind::Hyperbola(c)) => Some(c.base().clone_exact()),
            Some(CurveKind::Parabola(c)) => Some(c.base().clone_exact()),
            _ => None,
        })
    }

    fn vertexuse_uv_point(&mut self, k: &Kernel, vertexuse: EntityRef) -> Result<Point2> {
        let EntityRef::Vertexuse(vu) = vertexuse else {
            return Err(cache_face_entity("not a vertex use"));
        };
        crate::topology::vertexuse::compute_uv_point(k, self.b, vu, true)
    }

    fn vertexuse_vertex_tolerance(&self, vertexuse: EntityRef) -> Result<f64> {
        let EntityRef::Vertexuse(vu) = vertexuse else {
            return Err(cache_face_entity("not a vertex use"));
        };
        let v = crate::topology::vertexuse::vertex(self.b, vu)?
            .ok_or_else(|| cache_face_entity("vertex use without vertex"))?;
        crate::topology::vertex::tolerance(self.b, v)
    }

    fn zone_tol_3d(&self, _k: &Kernel, object: EntityRef) -> Result<f64> {
        // SmTol::GetZoneTol3d(const SmObject*): the topology's GetTolerance
        match object {
            EntityRef::Face(f) => fp2::tolerance(self.b, f),
            EntityRef::Edge(e) => crate::topology::edge::tolerance(self.b, e),
            EntityRef::Vertex(v) => crate::topology::vertex::tolerance(self.b, v),
            EntityRef::Edgeuse(eu) => eu1::tolerance(self.b, eu),
            // no GetTolerance override: SmTopology::GetTolerance
            _ => Ok(crate::geom::tol::ZONE_TOL_3D),
        }
    }

    fn natural_uv_domain(&self) -> Result<Option<Extent2d>> {
        Ok(Some(
            fp2::surface(self.b, self.f)?
                .ok_or(SolidError::null_pointer(OP_CACHE_FACE))?
                // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
                .natural_uv_domain_if_built(),
        ))
    }

    fn point_classify(
        &mut self,
        k: &Kernel,
        uv_point: Point2,
        src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<EntityRef>)> {
        // SmPointClassification sPC(sSrcZoneTol3d, ..); sPC.SetSrcZoneTol3d(..)
        let mut pc = PointClassification::new(src_zone_tol_3d);
        pc.set_src_zone_tol_3d(src_zone_tol_3d);
        // PointClassify(uv, tol, FALSE, TRUE, sPC); the trimmed cache it would find is
        // the one asking (under construction, or the caller's)
        point_classify(
            k,
            self.b,
            self.f,
            uv_point,
            &mut pc,
            PointClassifyOptions {
                src_zone_tol_3d,
                do_boundary_intersections: false,
                use_uv_classification: true,
                ..PointClassifyOptions::default()
            },
        )?;
        Ok((pc.point_class() as i32, pc.object()))
    }
}

/// A face surface's cache as `smsurf_GetSurfaceCache` hands it out
/// ([`face_surface_cache`]).
#[derive(Debug)]
pub struct FaceSurfaceCache {
    /// The cache. Its handles are the face's topology ([`EntityRef`]); the surface's
    /// own handle is the face.
    pub cache: TrimSrfCache<EntityRef>,
    /// Whether [`FaceSurfaceCache::cache`] is the surface's trimmed surface cache
    /// (`SM_OC_TRIMSRF`), else the surface cache (`SM_OC_SURFACE`) the lookup falls back
    /// to when no trimmed cache could be made, wrapped as one
    /// ([`TrimSrfCache::from_base`]).
    pub trimmed: bool,
    /// Whether the cache was taken out of the surface's cache slot (the surface's own
    /// trimmed cache, put back by [`FaceSurfaceCache::release`]).
    kept: bool,
}

impl FaceSurfaceCache {
    /// Ends the use of the cache: the surface's own trimmed cache goes back into its
    /// cache slot (shared with every copy of the surface), with what the use did to it
    /// (a tree built on demand, a face containment computed); any other cache is
    /// dropped.
    pub fn release(self) {
        if self.kept {
            Surface::store_trim_cache(self.cache);
        }
    }
}

/// `smsurf_GetSurfaceCache(pFace->GetSurface())` (through
/// `SmCacheMgrTSrf::CacheMakeOrValidate`): the trimmed surface cache of face `f`'s
/// surface, else its plain surface cache wrapped as one. Like upstream's, the trimmed
/// cache is an object of the surface ([`Surface::take_trim_cache`]), made by the first
/// lookup and reused by every later one:
/// - a cache the surface already has is reused as it is (whether or not it was built
///   with the face), unless its face was modified (`m_bFaceWasModified`) and the
///   face's B-rep is not being edited, when it is deleted and made again;
/// - a new cache is built with the face ([`make_trim_srf_cache_with_face`]; this creates
///   the face's UV trim curves) unless the surface's owner is cleared
///   ([`Brep::is_surface_owner_cleared`], inside `CreateUVTrimCurve(s)`): the surface
///   then has no face upstream, so the cache is built standalone
///   ([`make_trim_srf_cache`]) and makes no UV trim curves; either way it is kept;
/// - a lookup while the cache is in use (taken out of the slot by an enclosing lookup,
///   upstream the same object, e.g. during its construction) gets a cache built the
///   same way that is not kept.
///
/// When no trimmed cache can be made, the surface cache is built fresh and not kept
/// ([`FaceSurfaceCache::trimmed`] is false). The caller ends the use with
/// [`FaceSurfaceCache::release`].
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a face without surface; those of the surface cache build
/// when neither cache can be made; [`SolidError::Unported`],
/// [`SolidError::InvalidInput`] and [`SolidError::NotFound`] from the trimmed cache
/// build.
pub fn face_surface_cache(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<FaceSurfaceCache> {
    // the cache copies the surface; the face's B-rep is changed while it is built
    let surface = b
        .face(f)?
        .surface
        .as_ref()
        .ok_or(SolidError::null_pointer(OP_CACHE_FACE))?
        .clone_for_evaluation()?;
    // `GetFace()` is NULL while the owner is cleared
    let cleared = b.is_surface_owner_cleared(f);
    surface_cache_of_face(k, b, f, &surface, cleared)
}

/// [`face_surface_cache`] of `surface`, a surface whose owner is face `f`: the face's
/// own surface, or a copy of it that kept the face as owner (an offset's extended
/// surface). The cache kept is `surface`'s own (its cache slot, shared with its
/// evaluation copies), as upstream keys it by the surface object; the face's surface
/// owner being cleared only matters when `surface` is that surface.
///
/// # Errors
///
/// Those of [`face_surface_cache`].
pub fn owned_surface_cache(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    surface: &Surface,
) -> Result<FaceSurfaceCache> {
    let cleared = b.is_surface_owner_cleared(f)
        && b.face(f)?
            .surface
            .as_ref()
            .is_some_and(|fs| fs.shares_cache_slot_with(surface));
    surface_cache_of_face(k, b, f, surface, cleared)
}

/// The cache a numerical solve on `surface` (owned by face `f`, owner key `owner`)
/// gets through `smsurf_GetSurfaceCache` / `GetTree`: the surface's trimmed cache
/// ([`owned_surface_cache`]: the one it keeps, else one made with the face and kept),
/// its tree built with the face as `GetTree` builds it ([`face_cache_tree`]), handed
/// out as a copy in the solve's handle space (`map`, with the solve's surface `handle`)
/// while the kept cache goes back to the surface. `None` when no trimmed cache could be made (the caller builds
/// the plain surface cache, as upstream falls back to it).
///
/// # Errors
///
/// Those of [`owned_surface_cache`], of the tree build and of `map`.
pub fn solve_surface_cache_of_face<O: Copy + PartialEq>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    owner: crate::geom::object::ObjectRef,
    surface: &Surface,
    handle: Option<O>,
    map: impl Fn(EntityRef) -> Result<O>,
) -> Result<Option<SurfaceCache<O>>> {
    surface_cache_copy_of_face(k, b, f, owner, surface, handle, map, true)
}

/// [`solve_surface_cache_of_face`] without the solve's `GetTree`: the surface's
/// trimmed cache as `smsurf_GetSurfaceCache` hands it to a reader of its state bits
/// only (`GlobalPointSolve`'s fast-drop face test). A kept cache is handed out as it
/// is; a new one has its tree only when its factory builds it (not during a Boolean
/// operation), unless the surface's cache slot says upstream's cache is built.
/// Deviation: a cache left without tree gets the face's missing UV trim curves made
/// now, which upstream's classification makes when it first needs them.
///
/// # Errors
///
/// As [`solve_surface_cache_of_face`].
pub fn surface_cache_state_of_face<O: Copy + PartialEq>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    owner: crate::geom::object::ObjectRef,
    surface: &Surface,
    handle: Option<O>,
    map: impl Fn(EntityRef) -> Result<O>,
) -> Result<Option<SurfaceCache<O>>> {
    // upstream reuses a cache whose tree is built, with the built tree's state bits
    let get_tree = surface.cache_tree_state() == CacheTreeState::Built;
    surface_cache_copy_of_face(k, b, f, owner, surface, handle, map, get_tree)
}

#[allow(clippy::too_many_arguments)]
fn surface_cache_copy_of_face<O: Copy + PartialEq>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    owner: crate::geom::object::ObjectRef,
    surface: &Surface,
    handle: Option<O>,
    map: impl Fn(EntityRef) -> Result<O>,
    get_tree: bool,
) -> Result<Option<SurfaceCache<O>>> {
    let mut lease = owned_surface_cache(k, b, f, surface)?;
    if !lease.trimmed {
        return Ok(None);
    }
    let built = (|| {
        let tolerance = BrepCacheFace { b: &mut *b, f }.zone_tol_3d(k, EntityRef::Face(f))?;
        lease
            .cache
            .bind_face(owner, EntityRef::Face(f), tolerance)?;
        if get_tree {
            face_cache_tree(k, b, f, &mut lease.cache)?;
        } else if lease.cache.base().tree().is_none() {
            // The state reader's face classifications are read-only
            // (`PreparedFaceQueries`), so the UV trim curves upstream's classification
            // makes on the face (`GetOrCreateUVTrimCurve`) are made here, as the tree
            // build makes them; the tree and its state bits wait for `GetTree`.
            let mut face = BrepCacheFace { b: &mut *b, f };
            let created = face.create_uv_trim_curves(k);
            // Creation may raise the owning face's tolerance even on an error.
            let refreshed = face
                .zone_tol_3d(k, EntityRef::Face(f))
                .map(|tol| lease.cache.base_mut().set_owner_zone_tol_3d(tol));
            created?;
            refreshed?;
        }
        let mut copy = lease.cache.map_handles(&map)?;
        copy.base_mut().set_surface_handle(handle);
        Ok(copy.into_base())
    })();
    lease.release();
    built.map(Some)
}

fn surface_cache_of_face(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    surface: &Surface,
    cleared: bool,
) -> Result<FaceSurfaceCache> {
    let mut standalone = surface.clone_for_evaluation()?;
    if cleared {
        let _ = standalone.set_owner(None::<crate::geom::object::ObjectRef>);
    }
    let handle = Some(EntityRef::Face(f));
    match surface.take_trim_cache() {
        TrimCacheSlot::Stored(mut cache) => {
            // the cache's surface is the face's surface: its owner as it is now
            cache.base_mut().surface_mut().adopt_owner_of(if cleared {
                &standalone
            } else {
                surface
            });
            // If the face was modified and Brep editing is no longer enabled
            // then delete the old cache and create a new one.
            if !(cache.base().face_was_modified && !cleared && !b.editing_enabled()) {
                // the Face was not modified or m_bEditingEnabled == TRUE,
                // then use the old cache
                return Ok(FaceSurfaceCache {
                    cache: *cache,
                    trimmed: true,
                    kept: true,
                });
            }
            surface.delete_trim_cache();
        }
        TrimCacheSlot::CheckedOut => {
            // the enclosing lookup's cache: build one the same way, not kept
            let built = if cleared {
                get_trim_surface_cache(k, &standalone, handle)
            } else {
                get_surface_cache_with_face(k, surface, handle, &mut BrepCacheFace { b, f })
            }?;
            let trimmed = built.base().trimmed_dispatch;
            return Ok(FaceSurfaceCache {
                cache: built,
                trimmed,
                kept: false,
            });
        }
        TrimCacheSlot::Empty => {}
    }

    // next create a new SmTrimSrfCache and store it with the surface (before its tree
    // is built: lookups during the build find it in use)
    surface.check_out_trim_cache();
    let built = if cleared {
        make_trim_srf_cache(k, &standalone, handle)
    } else {
        make_trim_srf_cache_with_face(k, surface, handle, &mut BrepCacheFace { b, f })
    };
    match built {
        Ok(cache) => Ok(FaceSurfaceCache {
            cache,
            trimmed: true,
            kept: true,
        }),
        Err(e) => {
            // when construction failed - the cache is deleted
            surface.delete_trim_cache();
            match e {
                SolidError::Unported { .. }
                | SolidError::InvalidInput { .. }
                | SolidError::PortInvariant { .. }
                | SolidError::NotFound { .. } => Err(e),
                // else - get or make SurfaceCache
                _ => Ok(FaceSurfaceCache {
                    cache: TrimSrfCache::from_base(make_surface_cache(
                        k,
                        if cleared { &standalone } else { surface },
                        handle,
                    )?),
                    trimmed: false,
                    kept: false,
                }),
            }
        }
    }
}

/// [`face_surface_cache`] for a caller that only needs its side effects on the face
/// (the UV trim curves a new cache creates) and the cache kept on the surface.
///
/// # Errors
///
/// Those of [`face_surface_cache`].
pub fn note_face_surface_cache(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<()> {
    face_surface_cache(k, b, f).map(FaceSurfaceCache::release)
}

/// `SmSurfaceCache::GetTree` of a face surface's trimmed cache: builds the tree with the
/// face when there is none, or when the surface belongs to a face whose boundary is
/// not in the tree yet and no build is under way.
///
/// # Errors
///
/// Those of [`TrimSrfCache::build_tree_with_face`].
pub fn face_cache_tree(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    cache: &mut TrimSrfCache<EntityRef>,
) -> Result<()> {
    let base = cache.base();
    if base.tree().is_none()
        || (!base.have_t_surface_cache
            && base.surface().face()?.is_some()
            && base.face_containment_done != FaceContainment::UnderConstruction)
    {
        cache.build_tree_with_face(k, &mut BrepCacheFace { b, f })?;
    }
    Ok(())
}

// ---- co-ported topology access ----------------------------------------------------------

/// `SmFace::GetVertices(SmTArray&, NULL, bTgtLoopsOnly, pOptTgtLoops)`
/// ([`crate::topology::face::p4::vertices`] without attribute filter): the vertices of the loop uses of the upward face use
/// selected as [`eu1::faceuse_loopuses`] selects them ([`loopuse::vertices`]: the vertex
/// of a vertex loop use, else the distinct start vertices of its edge uses in CCW
/// order), appended (a vertex shared by two loops is listed twice). None without face
/// use.
///
/// # Errors
///
/// Those of [`eu1::faceuse_loopuses`] and [`loopuse::vertices`];
/// [`SolidError::InvalidInput`] for a vertex use without vertex (upstream dereferences
/// it).
pub fn face_vertices(
    b: &Brep,
    f: FaceId,
    tgt_loops_only: bool,
    tgt_loops: Option<&[LoopId]>,
) -> Result<Vec<VertexId>> {
    crate::topology::face::p4::vertices(b, f, None, tgt_loops_only, tgt_loops)
}

/// A copy of `surface` with out-of-bounds evaluation enabled, when it is an
/// `SmBSplineSurface` (B-spline, plane, surface of revolution) without it; `None`
/// when nothing changes (`SmTemporaryChangeValue` on `GetOutOfBoundsEnabled()`).
fn with_out_of_bounds_enabled(surface: &Surface) -> Result<Option<Surface>> {
    let enabled = match surface.kind() {
        SurfaceKind::BSpline(s) => s.is_out_of_bounds_enabled(),
        SurfaceKind::Plane(p) => p.base().is_out_of_bounds_enabled(),
        SurfaceKind::Cone(s) => s.rev().base().is_out_of_bounds_enabled(),
        SurfaceKind::Sphere(s) => s.base().base().is_out_of_bounds_enabled(),
        SurfaceKind::Torus(s) => s.base().base().is_out_of_bounds_enabled(),
        SurfaceKind::SurfOfRevolution(s) => s.base().is_out_of_bounds_enabled(),
        SurfaceKind::SurfOfExtrusion(s) => s.base().is_out_of_bounds_enabled(),
        SurfaceKind::OffsetSurface(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::SrfInVolume(_) => {
            return Ok(None);
        }
    };
    if enabled {
        return Ok(None);
    }
    let mut copy = surface.clone_for_evaluation()?;
    match copy.kind_mut() {
        SurfaceKind::BSpline(s) => *s.out_of_bounds_enabled_mut() = true,
        SurfaceKind::Plane(p) => *p.out_of_bounds_enabled_mut() = true,
        SurfaceKind::Cone(s) => *s.rev.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::Sphere(s) => *s.base.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::Torus(s) => *s.base.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::SurfOfRevolution(s) => *s.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::SurfOfExtrusion(s) => *s.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::OffsetSurface(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::SrfInVolume(_) => {}
    }
    Ok(Some(copy))
}

// ---- SmFace::PointClassify --------------------------------------------------------------

/// The optional arguments of [`point_classify`]; [`Default`] gives upstream's
/// defaults for the defaulted ones (`bTgtLoopsOnly = FALSE`, `pOptTgtLoops = NULL`,
/// `bUVSpaceOkay = TRUE`) and `0` / `false` for the others.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean arguments
pub struct PointClassifyOptions<'a> {
    /// `sSrcZoneTol3d` (`NotUsed` upstream).
    pub src_zone_tol_3d: f64,
    /// `bDoBoundaryIntersections` (`NotUsed` upstream).
    pub do_boundary_intersections: bool,
    /// `bUseUVClassification` (no longer used: both of its branches return
    /// `SM_ERR`).
    pub use_uv_classification: bool,
    /// `bTgtLoopsOnly`: only classify against [`PointClassifyOptions::tgt_loops`]
    /// (default `false`: all loops of the face).
    pub tgt_loops_only: bool,
    /// `pOptTgtLoops`: with `tgt_loops_only`, the loops to classify against (`None` or
    /// empty: the outer loop).
    pub tgt_loops: Option<&'a [LoopId]>,
    /// `bUVSpaceOkay` (`NotUsed` upstream; upstream default `TRUE`).
    pub uv_space_okay: bool,
    /// The face surface's trimmed surface cache
    /// (`SmCacheMgr::GetObjectCache(SM_OC_TRIMSRF, GetSurface())`), `None` when the
    /// surface has none.
    pub trim_cache: Option<&'a mut TrimSrfCache<EntityRef>>,
}

impl Default for PointClassifyOptions<'_> {
    fn default() -> Self {
        Self {
            src_zone_tol_3d: 0.0,
            do_boundary_intersections: false,
            use_uv_classification: false,
            tgt_loops_only: false,
            tgt_loops: None,
            uv_space_okay: true,
            trim_cache: None,
        }
    }
}

/// `SmFace::PointClassify`: classifies the parameter point `uv` of face `f` into `pc`:
/// [`PointClassType::Face`] with the face when inside, [`PointClassType::Edge`] /
/// [`PointClassType::Vertex`] with the edge (and its parameter) or vertex when on the
/// boundary, [`PointClassType::Unknown`] when outside (a point outside the face's UV
/// domain, outside the outer loop or inside an inner loop). The face surface's
/// trimmed surface cache answers first when it is up to date; then the point's
/// surface position is tested against every face vertex (within the larger of the
/// face, vertex and `pc` tolerances), then the point against every loop
/// (`query::loop::contains_uv_point`). With `tgt_loops_only`, only the
/// selected loops (and their vertices) are used. `pc` keeps its other members; its
/// tolerance (`GetTolerance`) enters the vertex and loop tolerances.
///
/// # Errors
///
/// [`SolidError::Upstream`] `SM_ERR` when the point is neither outside nor inside nor
/// on the checked loops (a loop that cannot classify it, or only inner loops checked
/// without the point being outside all of them); `SM_ERR_NULL_POINTER` for a face
/// without surface; those of the trimmed surface cache search, of [`face_vertices`]
/// and `query::loop::contains_uv_point`; [`SolidError::InvalidInput`] for an
/// on-boundary edge without curve.
pub fn point_classify(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    uv: Point2,
    pc: &mut PointClassification<EntityRef>,
    opts: PointClassifyOptions<'_>,
) -> Result<()> {
    point_classify_with_topology(
        k,
        &mut QueryTopology::Mutable(b),
        f,
        uv,
        pc,
        opts,
        None,
        None,
    )
}

/// Classify `uv` against face `f` without mutating its prepared topology.
///
/// Uses the same algorithm, tolerances, options and lazy traversal order as
/// [`point_classify`]. All UV trims actually needed by the traversal must already
/// exist (on the edge use or its mate). Early domain, cache and vertex exits do not
/// require trims. An up-to-date trim cache may be read when its tree already exists.
///
/// # Errors
///
/// Those of [`point_classify`], plus [`SolidError::Unported`] if a visited edge use
/// needs a UV trim created or an applicable trim cache needs its tree built.
pub fn point_classify_prepared(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    uv: Point2,
    pc: &mut PointClassification<EntityRef>,
    opts: PointClassifyOptions<'_>,
) -> Result<()> {
    point_classify_with_topology(
        k,
        &mut QueryTopology::Prepared(b),
        f,
        uv,
        pc,
        opts,
        None,
        None,
    )
}

pub(crate) fn point_classify_requested(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    uv: Point2,
    pc: &mut PointClassification<EntityRef>,
    opts: PointClassifyOptions<'_>,
    preparation: &crate::query::brep::RayPreparation,
) -> Result<()> {
    point_classify_with_topology(
        k,
        &mut QueryTopology::Requested(b, preparation),
        f,
        uv,
        pc,
        opts,
        None,
        None,
    )
}

pub(crate) fn point_classify_live(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    uv: Point2,
    pc: &mut PointClassification<EntityRef>,
    opts: PointClassifyOptions<'_>,
    cache: Option<&SurfaceCache<crate::topology::brep_cache::CacheObject>>,
) -> Result<()> {
    point_classify_with_topology(
        k,
        &mut QueryTopology::Mutable(b),
        f,
        uv,
        pc,
        opts,
        cache,
        None,
    )
}

/// The vertices of a face in [`face_vertices`] order (all loops) with a box search
/// over their points, for a run of [`point_classify_indexed`] calls on a face whose
/// topology does not change between them (the fork's `SmFaceVertexIndex`, #431).
pub(crate) struct FaceVertexIndex {
    face: FaceId,
    vertices: Vec<VertexId>,
    search: SpatialIndex,
    max_vertex_tol: f64,
}

impl FaceVertexIndex {
    /// The index of face `f`'s vertices; `None` when they cannot be listed (the
    /// classification then lists them itself, and fails as it would).
    pub(crate) fn new(b: &Brep, f: FaceId) -> Option<Self> {
        let vertices = face_vertices(b, f, false, None).ok()?;
        let mut points = Vec::new();
        let mut max_vertex_tol = f64::NEG_INFINITY;
        for &v in &vertices {
            let vertex = b.vertex(v).ok()?;
            points.push(Some(Extent3d::from_point(vertex.point)));
            max_vertex_tol = max2(max_vertex_tol, vertex.tolerance);
        }
        Some(Self {
            face: f,
            vertices,
            search: SpatialIndex::new(points),
            max_vertex_tol,
        })
    }

    /// The vertices a box search finds within `pad` of `p`, in [`face_vertices`]
    /// order (all of them when `p` or `pad` is not finite).
    fn near(&self, p: Point3, pad: f64) -> Vec<VertexId> {
        self.search
            .near_point(p, pad)
            .into_iter()
            .filter_map(|i| self.vertices.get(i).copied())
            .collect()
    }
}

/// [`point_classify`] with the index of the face's vertices `vertex_index` (built by
/// [`FaceVertexIndex::new`] on the same face and topology), for `SmFace::AssertValid`
/// test 12, which reads only the class: the same class without listing the face's
/// vertices on every call. The vertex reported is upstream's (the first in
/// [`face_vertices`] order at the point) unless a vertex tolerance grew after the
/// index was built (a UV trim curve made since), when it can be a later one.
///
/// # Errors
///
/// Those of [`point_classify`].
pub(crate) fn point_classify_indexed(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    uv: Point2,
    pc: &mut PointClassification<EntityRef>,
    opts: PointClassifyOptions<'_>,
    vertex_index: Option<&FaceVertexIndex>,
) -> Result<()> {
    point_classify_with_topology(
        k,
        &mut QueryTopology::Mutable(b),
        f,
        uv,
        pc,
        opts,
        None,
        vertex_index,
    )
}

fn cache_is_current<O: Copy + PartialEq>(cache: &SurfaceCache<O>) -> bool {
    cache.have_t_surface_cache
        && cache.face_containment_done == FaceContainment::Done
        && !cache.face_was_modified
}

fn classify_trim_cache<O: Copy + PartialEq>(
    k: &Kernel,
    cache: &SurfaceCache<O>,
    uv: Point2,
    pc: &mut PointClassification<EntityRef>,
    entity_of: impl Fn(O) -> Result<EntityRef>,
) -> Result<bool> {
    let nodes = cache.find_uv_nodes_built(k, &Extent2d::from_point(uv), false)?;
    let tree = cache
        .tree()
        .ok_or_else(|| null(OP_CLASSIFY, "trimmed surface cache without tree"))?;
    for node in nodes {
        let node = tree.node(node)?;
        if node.child1.is_some() {
            continue;
        }
        let NodeData::Surface(aux) = &node.data else {
            return Err(null(OP_CLASSIFY, "leaf node without surface data"));
        };
        if aux.node_class == NodeClass::Outside {
            pc.set_class_object(PointClassType::Unknown, None);
            return Ok(true);
        }
        if aux.node_class == NodeClass::Inside {
            pc.set_class_object(PointClassType::Face, aux.face.map(&entity_of).transpose()?);
            pc.set_gap_3d(0.0);
            pc.set_pre_snap_param(BIG_DOUBLE);
            pc.set_uv_param(uv);
            return Ok(true);
        }
    }
    Ok(false)
}

/// `PointClassify`'s fast classification by the face surface's trimmed cache `tsc`
/// when it is up to date (`own`: the surface's own cache, whose tree is built with the
/// face when it has none): `true` when the cache decided the point.
#[allow(clippy::too_many_arguments)]
fn classify_by_trim_cache(
    k: &Kernel,
    b: &mut QueryTopology<'_>,
    f: FaceId,
    tsc: &mut TrimSrfCache<EntityRef>,
    own: bool,
    face_owned: bool,
    uv: Point2,
    pc: &mut PointClassification<EntityRef>,
) -> Result<bool> {
    if !(tsc.base().have_t_surface_cache
        && tsc.base().face_containment_done == FaceContainment::Done
        && !tsc.base().face_was_modified
        && face_owned)
    {
        return Ok(false);
    }
    // get every TrimSrfCache leaf node intersecting the UVPoint
    if tsc.base().tree().is_none() {
        match b {
            QueryTopology::Prepared(_) | QueryTopology::Requested(..) => {
                return Err(SolidError::Unported {
                    upstream: "SmSurfaceCache::BuildTree (prepared query requires a built tree)",
                    unit: "surface.surface_cache_p1",
                });
            }
            QueryTopology::Mutable(b) if own => face_cache_tree(k, b, f, tsc)?,
            QueryTopology::Mutable(_) => tsc.build_tree(k)?,
        }
    }
    classify_trim_cache(k, tsc.base(), uv, pc, Ok)
}

fn point_classify_with_topology(
    k: &Kernel,
    b: &mut QueryTopology<'_>,
    f: FaceId,
    uv: Point2,
    pc: &mut PointClassification<EntityRef>,
    opts: PointClassifyOptions<'_>,
    prepared_cache: Option<&SurfaceCache<crate::topology::brep_cache::CacheObject>>,
    vertex_index: Option<&FaceVertexIndex>,
) -> Result<()> {
    let PointClassifyOptions {
        tgt_loops_only,
        tgt_loops,
        use_uv_classification,
        trim_cache,
        ..
    } = opts;
    let face = b.face(f)?;
    let this = EntityRef::Face(f);
    let (face_tolerance, face_uv_domain) = (face.tolerance, face.uv_domain);

    // init output - SM_PC_UNKNOWN == 'Outside' Face
    pc.set_class_object(PointClassType::Unknown, None);

    // No Work: TargetPoint outside of Face->UVDomain
    let scaled_zero = EFF_ZERO_SQRT * (1.0 + face_uv_domain.size().length());
    if !face_uv_domain.contains_point(uv, scaled_zero) {
        // TargetPoint is outside of Face
        return Ok(());
    }

    // locals
    let surface = face.surface.as_ref();

    // The operation cache is the same cache the preceding surface solve built.
    // Consult its native fast path before the vertex/loop fallback.
    if !tgt_loops_only && surface.map_or(Ok(false), |s| s.face().map(|f| f.is_some()))? {
        let cache = prepared_cache.or_else(|| match &*b {
            QueryTopology::Requested(_, preparation) => preparation.prepared_cache(f),
            _ => None,
        });
        if let Some(cache) = cache {
            if cache_is_current(cache)
                && classify_trim_cache(k, cache, uv, pc, |object| match object {
                    crate::topology::brep_cache::CacheObject::Face(face) => {
                        Ok(EntityRef::Face(face))
                    }
                    _ => Err(SolidError::invalid_input(
                        OP_CLASSIFY,
                        "trim cache leaf without face",
                    )),
                })?
            {
                return Ok(());
            }
        }
    }

    // when doing all loops
    if !tgt_loops_only {
        let face_owned = surface.map_or(Ok(false), |s| s.face().map(|o| o.is_some()))?;
        // 1. try fast point TrimSrfCache classification.
        // First try to get the cache - will be NULL when object has no cache: the
        // caller's, else the surface's own (taken out of its slot while it is used)
        let tsc = if surface.is_some() { trim_cache } else { None };
        let own = match (&tsc, surface) {
            (None, Some(s)) => match s.take_trim_cache() {
                TrimCacheSlot::Stored(cache) => Some(*cache),
                TrimCacheSlot::Empty | TrimCacheSlot::CheckedOut => None,
            },
            _ => None,
        };
        let classified = if let Some(tsc) = tsc {
            classify_by_trim_cache(k, b, f, tsc, false, face_owned, uv, pc)
        } else if let Some(mut own) = own {
            let classified = classify_by_trim_cache(k, b, f, &mut own, true, face_owned, uv, pc);
            Surface::store_trim_cache(own);
            classified
        } else {
            Ok(false)
        };
        if classified? {
            return Ok(());
        }
    } // end when doing all loops

    // arrive here when PointClassify by TrimSrfCache failed or when bTgtLoopsOnly

    // init
    pc.set_class_object(PointClassType::Unknown, None);

    // We must enable out-of-bounds evaluation, otherwise a point outside the surface
    // domain will be classified on a boundary.
    let surface = b
        .face(f)?
        .surface
        .as_ref()
        .ok_or(SolidError::null_pointer(OP_CLASSIFY))?;
    // (an owned surface: the loop classification edits the B-rep while it walks)
    let surface: Surface = if let Some(copy) = with_out_of_bounds_enabled(surface)? {
        copy
    } else {
        surface.clone_for_evaluation()?
    };
    let surface = &surface;

    // Eval Surface at TargetPoint (status ignored)
    let surf_pt = ignored(surface.evaluate_1st_derivatives(k, uv, false, false))?
        .map_or(Vec3::UNDEF, |(p, _, _)| p);

    {
        // begin Scope for TargetPoint/Vertex intersection
        // (#431) with the caller's index of this face's vertices, those a box search
        // finds within the largest vertex tolerance of the point are tested first, in
        // GetVertices order, and all of them only when none of those is at the point
        let index = vertex_index.filter(|index| !tgt_loops_only && index.face == f);
        let mut verts = match index {
            Some(index) => {
                let pad = max2(max2(face_tolerance, index.max_vertex_tol), pc.tolerance());
                index.near(surf_pt, pad)
            }
            None => face_vertices(b, f, tgt_loops_only, tgt_loops)?,
        };
        let mut searched = index.is_some();

        loop {
            // see if the target point is on a vertex location
            for v in verts {
                let vert = b.vertex(v)?;
                let mut vert_tol = max2(face_tolerance, vert.tolerance);
                vert_tol = max2(vert_tol, pc.tolerance());

                if vert.point.closer_than(vert_tol, surf_pt) {
                    pc.set_class_object(PointClassType::Vertex, Some(EntityRef::Vertex(v)));

                    // Need to calculate the deviation.
                    pc.set_deviation(vert.point.distance(surf_pt));
                    pc.set_src_zone_tol_3d(vert_tol);
                    pc.set_pre_snap_param(BIG_DOUBLE);
                    return Ok(());
                }
            } // end iter every face vertex
            if !searched {
                break;
            }
            searched = false;
            verts = face_vertices(b, f, tgt_loops_only, tgt_loops)?;
        }
    }

    // arrive here when TargetPoint is not on a vertex,
    // next: check TargetPoint Loop containment.
    let mut type_inner = PointContainment::Unknown;
    let mut type_outer = PointContainment::Unknown;
    let mut checked_outer_loop = false;
    let outer_loop = fp2::outer_loops(b, f)?.first().copied();
    let loops = fp2::loops(b, f, tgt_loops_only, tgt_loops)?;

    // no work - no loops to check
    if loops.is_empty() {
        return Ok(());
    }

    // Get tolerance
    let tol_3d = max2(face_tolerance, pc.tolerance());

    // For every loop: check containment
    for l in loops {
        let is_outer = Some(l) == outer_loop;

        // check TargetPoint containment
        let UvContainment {
            containment,
            on_edge,
            edge_param,
            ..
        } = contains_uv_point_with_topology(k, b, l, Some(surface), uv, tol_3d)?;
        // when checking an outerloop
        if is_outer {
            type_outer = containment;

            // remember outer loop has been checked
            checked_outer_loop = true;

            // Exit Case: when outside the outer loop - outside the face
            if type_outer == PointContainment::Outside {
                return Ok(());
            }
            // Exit Case: when on outer loop - on a Face->Edge - Get Deviation
            else if type_outer == PointContainment::OnBoundary {
                return set_on_edge(k, b, pc, on_edge, edge_param, surf_pt, tol_3d);
            }
            // arrive here when TargetPoint is 'in' OuterLoop
        } else {
            // when checking an InnerLoop branch
            type_inner = containment;

            // Exit Case: Inside an InnerLoop = Outside Face
            if type_inner == PointContainment::Inside {
                return Ok(());
            }
            // Exit Case: On an InnerLoop bound = On a Face->Edge - Get Deviation
            else if type_inner == PointContainment::OnBoundary {
                return set_on_edge(k, b, pc, on_edge, edge_param, surf_pt, tol_3d);
            }
            // arrive here when TargetPoint is 'Out' of this InnerLoop
        }
    } // end iter every Loop to Check

    // When TargetPoint is 'in' Face
    if type_outer == PointContainment::Inside
        || (tgt_loops_only && !checked_outer_loop && type_inner == PointContainment::Outside)
    {
        // inside Face
        pc.set_class_object(PointClassType::Face, Some(this));
        pc.set_pre_snap_param(BIG_DOUBLE);
        pc.set_deviation(0.0);
        pc.set_src_zone_tol_3d(tol_3d);
        pc.set_uv_param(uv);
        return Ok(());
    }

    // PointClassify() fails for BadDatabase loops that cross seam boundaries
    let _ = use_uv_classification;
    Err(SolidError::Upstream {
        operation: OP_CLASSIFY,
        code: SM_ERR,
        entity: Some(this),
    })
}

/// The on-boundary exit of [`point_classify`]: the edge, its parameter, the gap from
/// the edge point to the surface point and the loop tolerance.
fn set_on_edge(
    k: &Kernel,
    b: &Brep,
    pc: &mut PointClassification<EntityRef>,
    on_edge: Option<EdgeId>,
    edge_param: f64,
    surf_pt: Point3,
    tol_3d: f64,
) -> Result<()> {
    pc.set_class_object(PointClassType::Edge, on_edge.map(EntityRef::Edge));
    pc.set_t_param(edge_param, false);

    // Need to calculate the deviation.
    let on_edge = on_edge.ok_or_else(|| null(OP_CLASSIFY, "on-boundary without edge"))?;
    let curve = b
        .edge(on_edge)?
        .curve
        .as_ref()
        .ok_or_else(|| null(OP_CLASSIFY, "edge without curve"))?;
    let edge_pt = ignored(curve.evaluate_point(k, edge_param))?.unwrap_or(Vec3::UNDEF);
    pc.set_deviation(edge_pt.distance(surf_pt));
    pc.set_src_zone_tol_3d(tol_3d);
    pc.set_pre_snap_param(BIG_DOUBLE);
    Ok(())
}

// ---- sm_ClassifyParamOnFaces ----------------------------------------------------------------

/// `sm_ClassifyParamOnFaces`: which of two faces the parameter point `uv` lies on: `1`
/// when it is in `domain1` only or classifies in `face1`, `2` when it is in `domain2`
/// only or classifies in `face2`, `0` when it is on the boundary of both or classifies
/// cleanly to neither. `src_zone_tol_3d` above `SM_EFF_ZERO` is the classification
/// tolerance, else the larger face tolerance. The statuses of the face classifications
/// are ignored.
///
/// # Errors
///
/// [`SolidError::Unported`] from the classifications; their
/// [`SolidError::InvalidInput`] and [`SolidError::NotFound`] (malformed B-rep).
pub fn classify_param_on_faces(
    k: &Kernel,
    b: &mut Brep,
    uv: Point2,
    src_zone_tol_3d: f64,
    face1: FaceId,
    domain1: &Extent2d,
    face2: FaceId,
    domain2: &Extent2d,
) -> Result<i32> {
    // quick BBox containment check
    let in_face1 = domain1.contains_point(uv, EFF_ZERO);
    let in_face2 = domain2.contains_point(uv, EFF_ZERO);

    // low work case - TgtUVParam is in only one TgtFace Domain BBox
    if in_face1 && !in_face2 {
        return Ok(1);
    } else if !in_face1 && in_face2 {
        return Ok(2);
    }

    // ZoneTol3d. When input ZoneTol3d is zero - set ZoneTol3d = Max(pFace1->ZoneTol3d,
    // pFace2->ZoneTol3d)
    let src_tol = if src_zone_tol_3d > EFF_ZERO {
        src_zone_tol_3d
    } else {
        max2(fp2::tolerance(b, face1)?, fp2::tolerance(b, face2)?)
    };

    // classify sFaceUVParam against pFace1
    let mut classify = |face: FaceId| -> Result<PointClassType> {
        let mut pc = PointClassification::new(src_tol);
        let opts = PointClassifyOptions {
            src_zone_tol_3d: src_tol,
            do_boundary_intersections: true,
            use_uv_classification: true,
            uv_space_okay: true,
            ..PointClassifyOptions::default()
        };
        match point_classify(k, b, face, uv, &mut pc, opts) {
            Err(
                e @ (SolidError::Unported { .. }
                | SolidError::InvalidInput { .. }
                | SolidError::PortInvariant { .. }
                | SolidError::NotFound { .. }),
            ) => Err(e),
            _ => Ok(pc.point_class()),
        }
    };
    let pc1 = classify(face1)?;
    if pc1 == PointClassType::Face {
        return Ok(1);
    }

    // classify sFaceUVParam against pFace2
    let pc2 = classify(face2)?;
    if pc2 == PointClassType::Face {
        return Ok(2);
    }

    // when UVParam is on both FaceBndrys its probably on the XSectCrve between the
    // faces - no action required
    if pc1 == PointClassType::Edge && pc2 == PointClassType::Edge {
        return Ok(0);
    }

    // arrive here when the UVPoint did not classify cleanly to one face or the other
    Ok(0)
}

// ---- SmProtoVertex --------------------------------------------------------------------------

/// `SmProtoVertex::DistanceTo`: the distance from the proto vertex's position to
/// `position`.
#[must_use]
pub fn proto_vertex_distance_to(pv: &ProtoVertex, position: Point3) -> f64 {
    pv.position().distance(position)
}

/// `SmProtoVertex::GetCaptureDistance`: the capture distance of the owning manager.
#[must_use]
pub fn proto_vertex_capture_distance<O: ClassObject>(mgr: &ProtoTopologyManager<O>) -> f64 {
    mgr.capture_distance()
}

/// The radius of the ball around the proto vertex's position whose points are in it:
/// the larger of its own tolerance (`m_dTol`) and the manager's capture distance (the
/// SMLib fork's `SmProtoVertex::GetContainmentRadius`, split out of `ContainsPoint`).
#[must_use]
pub fn proto_vertex_containment_radius<O: ClassObject>(
    mgr: &ProtoTopologyManager<O>,
    pv: &ProtoVertex,
) -> f64 {
    // m_dTol >= Local object Tols. Takes precedence over CaptureDistance [B673]
    max2(pv.raw_tolerance(), proto_vertex_capture_distance(mgr))
}

/// `SmProtoVertex::ContainsPoint`: whether `position` is within
/// [`proto_vertex_containment_radius`] of the proto vertex's position.
#[must_use]
pub fn proto_vertex_contains_point<O: ClassObject>(
    mgr: &ProtoTopologyManager<O>,
    pv: &ProtoVertex,
    position: Point3,
) -> bool {
    pv.position()
        .closer_than(proto_vertex_containment_radius(mgr, pv), position)
}

// ---- SmFace::CurveOnClassify ------------------------------------------------------------

/// What [`curve_on_classify`] needs of the caller's classification handles.
pub trait CurveOnClassifyObjects<O: ClassObject>: ClassGeometry<O> {
    /// The handle of face `f` (`this`).
    fn face(&self, f: FaceId) -> O;
    /// The classification handle of vertex `v`.
    fn vertex(&self, v: VertexId) -> O;
    /// The classification handle of edge `e`.
    fn edge(&self, e: EdgeId) -> O;
    /// The identity of vertex use `vu`, also used by trim-cache lists.
    fn vertexuse(&self, vu: VertexuseId) -> O;
    /// The identity of edge use `eu`, also used by trim-cache lists.
    fn edgeuse(&self, eu: EdgeuseId) -> O;
    /// The handle of edge `e`'s curve, resolvable through [`ClassGeometry::curve`].
    fn edge_curve(&self, e: EdgeId) -> O;
    /// The handle of the trim curve stored on `eu`, which may be the selected
    /// edgeuse's mate. Resolvable through [`ClassGeometry::curve`].
    fn trim_curve(&self, eu: EdgeuseId) -> O;
    /// The handle of face `f`'s surface, `None` without one.
    fn face_surface(&self, f: FaceId) -> Option<O>;
    /// The loop behind a target loop handle.
    fn loop_id(&self, tgt_loop: O) -> Option<LoopId>;
}

/// The B-rep of the face a classification runs on (`SmFace`'s owning `SmBrep`), as the
/// calls so far have left it.
pub trait FaceBrep {
    /// The B-rep.
    fn brep(&self) -> &Brep;
}

impl FaceBrep for &Brep {
    fn brep(&self) -> &Brep {
        self
    }
}

impl FaceBrep for &mut Brep {
    fn brep(&self) -> &Brep {
        self
    }
}

/// What [`curve_on_classify_mut`] needs: the caller's classification handles over the
/// face's own B-rep, which the classification edits in place where upstream's `const` face
/// queries edit theirs. The adapter owns the B-rep borrow; its [`ClassGeometry`] reads
/// must read [`FaceBrep::brep`] as it is now (raised tolerances, made UV trim curves).
pub trait CurveOnClassifyBrep<O: ClassObject>:
    CurveOnClassifyObjects<O> + ClassGeometryMut<O> + FaceBrep
{
    /// The face's B-rep, mutably.
    fn brep_mut(&mut self) -> &mut Brep;
    /// The surface of face `f` the classification evaluates (`Face->GetSurface()`).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling face id.
    fn classify_surface(&self, f: FaceId) -> Result<Option<&Surface>> {
        Ok(self.brep().face(f)?.surface.as_ref())
    }
    /// Whether a UV trim curve the classification makes on `eu` must be carried with
    /// the classification because [`ClassGeometry::curve`] cannot resolve it: `false`
    /// for an adapter that reads the B-rep it edits.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling edge use.
    fn keeps_made_trim(&self, _eu: EdgeuseId) -> Result<bool> {
        Ok(false)
    }
}

/// The lazy B-rep query state shared by classifications of the same face. Only gap
/// caches are copied back: replacing the B-rep would also replace geometry ownership
/// and unrelated topology. Keep this state alive on error, since `CalcVertexFaceGap`
/// invalidates same-face siblings before attempting the projection.
#[derive(Default)]
pub(crate) struct CurveOnClassifyCache {
    snapshot: Option<Brep>,
}

impl CurveOnClassifyCache {
    /// The B-rep as the queries sharing this state have left it: a copy of `b` (sharing
    /// its surface caches) made on first use.
    pub(crate) fn snapshot(&mut self, b: &Brep) -> &mut Brep {
        self.snapshot
            .get_or_insert_with(|| b.clone_sharing_surface_caches())
    }

    /// Copy back what the classification's lazy queries left on its snapshot that
    /// upstream keeps on the face's B-rep: the gap caches ([`Self::apply_gap_updates`]),
    /// the edge uses' UV trim curves and the edge and vertex tolerances
    /// `SetUVTrimCurve` raised to fit them.
    pub(crate) fn apply_uv_trim_updates(self, b: &mut Brep) -> Result<()> {
        if let Some(snapshot) = &self.snapshot {
            for (id, eu) in snapshot.edgeuses().iter() {
                if b.edgeuse(id)?.uv_trim_curve.is_none() {
                    if let Some(trim) = eu.uv_trim_curve.as_ref() {
                        // The trim `SetUVTrimCurve` made on the snapshot, owned by its
                        // edge use there; an exact copy keeps that owner (a plain
                        // clone is another object, without owner;
                        // source issue #1678).
                        b.edgeuse_mut(id)?.uv_trim_curve = Some(trim.clone_for_evaluation()?);
                    }
                }
            }
            for (id, e) in snapshot.edges().iter() {
                if e.tolerance > b.edge(id)?.tolerance {
                    b.edge_mut(id)?.tolerance = e.tolerance;
                }
            }
            for (id, v) in snapshot.vertices().iter() {
                if v.tolerance > b.vertex(id)?.tolerance {
                    b.vertex_mut(id)?.tolerance = v.tolerance;
                }
            }
        }
        self.apply_gap_updates(b)
    }

    pub(crate) fn apply_gap_updates(self, b: &mut Brep) -> Result<()> {
        if let Some(snapshot) = self.snapshot {
            for (id, vu) in snapshot.vertexuses().iter() {
                if b.vertexuse(id)?.vertex_face_gap != vu.vertex_face_gap {
                    b.vertexuse_mut(id)?.vertex_face_gap = vu.vertex_face_gap;
                }
            }
            for (id, eu) in snapshot.edgeuses().iter() {
                if b.edgeuse(id)?.max_edge_face_gap != eu.max_edge_face_gap {
                    b.edgeuse_mut(id)?.max_edge_face_gap = eu.max_edge_face_gap;
                }
            }
        }
        Ok(())
    }
}

/// `pVertex->GetVertexuseOfFace(pFace)->GetVertexFaceGap()->GetLength()`, 0 when the
/// vertex use has no vertex/face gap (`IntersectVertices`' `dMaxGap3d`), on the B-rep
/// snapshot `b` (which keeps the gap cache `GetVertexFaceGap` fills). `None` for a
/// vertex without a use on a face (upstream dereferences the null vertex use; part 5
/// reports the unresolved gap as invalid input).
fn vertex_face_gap_length(k: &Kernel, b: &mut Brep, v: VertexId, f: FaceId) -> Result<Option<f64>> {
    let Some(vu) = crate::topology::vertex::vertexuse_of_face(b, v, Some(f))? else {
        return Ok(None);
    };
    Ok(Some(
        crate::topology::vertexuse::vertex_face_gap(k, b, vu, false)?.map_or(0.0, |g| g.gap_3d),
    ))
}

/// The caller's [`ClassGeometry`] answering [`ClassGeometry::vertex_face_gap`] for the
/// vertices `IntersectVertices` intersects (their gaps computed from the B-rep, as
/// upstream's vertex pointers reach them); everything else, and the edits of its
/// surf/surf/surf block ([`ClassGeometryMut`]), is the caller's.
///
/// Every [`ClassGeometry`] and [`ClassGeometryMut`] method must be forwarded to `inner`
/// here, including any added later: a trait default would otherwise silently replace the
/// inner adapter's override.
struct WithVertexFaceGaps<'g, G, O> {
    inner: &'g mut G,
    gaps: Vec<(O, Option<f64>)>,
}

impl<G: ClassGeometry<O>, O: ClassObject> ClassGeometry<O> for WithVertexFaceGaps<'_, G, O> {
    fn curve(&self, o: O) -> Option<&Curve> {
        self.inner.curve(o)
    }
    fn surface(&self, o: O) -> Option<&Surface> {
        self.inner.surface(o)
    }
    fn volume(&self, o: O) -> Option<&crate::surface::volume::Volume> {
        self.inner.volume(o)
    }
    fn point(&self, o: O) -> Option<Point3> {
        self.inner.point(o)
    }
    fn classification_vertex_point(&self, vertex: O) -> Option<Point3> {
        self.inner.classification_vertex_point(vertex)
    }
    fn topology_zone_tol_3d(&self, object: O) -> Option<f64> {
        self.inner.topology_zone_tol_3d(object)
    }
    fn vertex_face_gap(&self, vertex: O, surface: &Surface, face: Option<O>) -> Option<f64> {
        match self.gaps.iter().find(|(h, _)| *h == vertex) {
            Some((_, gap)) => *gap,
            None => self.inner.vertex_face_gap(vertex, surface, face),
        }
    }
    fn topology_entity(&self, object: O) -> Option<(&Brep, EntityRef)> {
        self.inner.topology_entity(object)
    }
    fn topology_handle(&self, entity: EntityRef) -> Option<O> {
        self.inner.topology_handle(entity)
    }
    fn owner_of(
        &self,
        owner: crate::geom::object::ObjectRef,
    ) -> Option<crate::geom::object::ObjectRef> {
        self.inner.owner_of(owner)
    }
}

impl<G: ClassGeometryMut<O>, O: ClassObject> ClassGeometryMut<O> for WithVertexFaceGaps<'_, G, O> {
    fn curve_mut(&mut self, o: O) -> Option<&mut Curve> {
        self.inner.curve_mut(o)
    }
    fn topology_entity_mut(&mut self, o: O) -> Option<(&mut Brep, EntityRef)> {
        self.inner.topology_entity_mut(o)
    }
    fn set_filleting_edge_curve(&mut self, edge: O, curve: Curve) -> Result<()> {
        self.inner.set_filleting_edge_curve(edge, curve)
    }
    fn topology_entity_for_query(&mut self, o: O) -> Option<(&mut Brep, EntityRef)> {
        self.inner.topology_entity_for_query(o)
    }
}

/// Resolve stored edge data and its optional gap on this face, on the face's B-rep
/// (whose gap cache `GetMaxEdgeFaceGap` fills).
fn boundary_edge<O: ClassObject>(
    k: &Kernel,
    objects: &mut impl CurveOnClassifyBrep<O>,
    face: FaceId,
    edge: EdgeId,
    has_big_gaps: bool,
) -> Result<BoundaryEdge<O>> {
    let face_gap = if has_big_gaps {
        match crate::topology::edge::edgeuse_of_face(objects.brep(), edge, Some(face))? {
            Some(eu) => eu1::max_edge_face_gap(k, objects.brep_mut(), eu, false)?.map(|g| g.gap_3d),
            None => None,
        }
    } else {
        None
    };
    let record = objects.brep().edge(edge)?;
    if record.curve.is_none() {
        return Err(SolidError::null_pointer(OP_CURVE));
    }
    Ok(BoundaryEdge {
        edge: objects.edge(edge),
        curve: objects.edge_curve(edge),
        interval: record.interval,
        zone_tol_3d: record.tolerance,
        face_gap,
    })
}

/// The caller's classification objects `inner` over a private copy `snapshot` of the
/// face's B-rep `brep` (see the module Deviations: a temporary stand-in for callers that
/// hold only `&Brep`).
///
/// The topology hooks forward to the caller's geometry, with `brep`'s entities
/// resolved in the snapshot (and the snapshot's handles and B-rep id looked up as
/// `brep`'s), so the topology members the classification calls read the B-rep as
/// upstream's calls leave it. Through [`ClassGeometryMut`] the face queries of
/// `ClassifyFaceInterval` edit the snapshot, as upstream's `const` members edit the
/// face's B-rep. Entities of another B-rep resolve as the caller's geometry resolves
/// them (read-only). The UV trim curves the classification makes on the snapshot are
/// unknown to `inner`; they are carried with the classification
/// ([`CurveOnClassifyBrep::keeps_made_trim`]).
pub(crate) struct SnapshotObjects<'g, G> {
    inner: &'g G,
    brep: &'g Brep,
    snapshot: &'g mut Brep,
}

impl<'g, G> SnapshotObjects<'g, G> {
    /// `inner` over `brep`, classifying on `snapshot` (a copy of `brep` sharing its
    /// surface caches, [`Brep::clone_sharing_surface_caches`]).
    pub(crate) fn new(inner: &'g G, brep: &'g Brep, snapshot: &'g mut Brep) -> Self {
        Self {
            inner,
            brep,
            snapshot,
        }
    }
    /// `b` as the caller's geometry knows it: the snapshot stands for `brep`.
    fn caller_brep<'a>(&'a self, b: &'a Brep) -> &'a Brep {
        if core::ptr::eq(b, &raw const *self.snapshot) {
            self.brep
        } else {
            b
        }
    }
    /// An entity the caller's geometry resolved, with `brep`'s moved to the snapshot.
    fn in_snapshot<'a>(
        &'a self,
        r: Option<(&'a Brep, EntityRef)>,
    ) -> Option<(&'a Brep, EntityRef)> {
        match r {
            Some((b, e)) if core::ptr::eq(b, self.brep) => Some((&*self.snapshot, e)),
            other => other,
        }
    }
}

impl<O: ClassObject, G: ClassGeometry<O>> ClassGeometry<O> for SnapshotObjects<'_, G> {
    fn curve(&self, o: O) -> Option<&Curve> {
        self.inner.curve(o)
    }
    fn surface(&self, o: O) -> Option<&Surface> {
        self.inner.surface(o)
    }
    fn volume(&self, o: O) -> Option<&crate::surface::volume::Volume> {
        self.inner.volume(o)
    }
    fn point(&self, o: O) -> Option<Point3> {
        self.inner.point(o)
    }
    fn classification_vertex_point(&self, vertex: O) -> Option<Point3> {
        self.inner.classification_vertex_point(vertex)
    }
    fn topology_zone_tol_3d(&self, object: O) -> Option<f64> {
        // `SmTol::GetZoneTol3d` of an entity of the face's B-rep reads it as the
        // classification has left it: the trimmed cache it builds on the snapshot
        // (`smsurf_GetSurfaceCache`, upstream on the face's own B-rep) can raise vertex
        // and edge tolerances
        match self.inner.topology_entity(object) {
            Some((b, e)) if core::ptr::eq(b, self.brep) => {
                let s: &Brep = self.snapshot;
                match e {
                    EntityRef::Vertex(v) => s.vertex(v).ok().map(|v| v.tolerance),
                    EntityRef::Edge(ed) => s.edge(ed).ok().map(|ed| ed.tolerance),
                    EntityRef::Face(f) => s.face(f).ok().map(|f| f.tolerance),
                    _ => self.inner.topology_zone_tol_3d(object),
                }
            }
            _ => self.inner.topology_zone_tol_3d(object),
        }
    }
    fn vertex_face_gap(&self, vertex: O, surface: &Surface, face: Option<O>) -> Option<f64> {
        self.inner.vertex_face_gap(vertex, surface, face)
    }
    fn topology_entity(&self, object: O) -> Option<(&Brep, EntityRef)> {
        self.in_snapshot(self.inner.topology_entity(object))
    }
    fn topology_handle(&self, entity: EntityRef) -> Option<O> {
        self.inner.topology_handle(entity)
    }
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<O> {
        self.inner
            .topology_handle_in(self.caller_brep(brep), entity)
    }
    fn topology_intersector_mate(&self, ti: O, entity: O) -> Option<Option<O>> {
        self.inner.topology_intersector_mate(ti, entity)
    }
    fn brep_id(&self, brep: &Brep) -> Option<crate::topology::BrepId> {
        self.inner.brep_id(self.caller_brep(brep))
    }
    fn owner_entity(&self, owner: crate::geom::object::ObjectRef) -> Option<(&Brep, EntityRef)> {
        self.in_snapshot(self.inner.owner_entity(owner))
    }
    fn owner_of(
        &self,
        owner: crate::geom::object::ObjectRef,
    ) -> Option<crate::geom::object::ObjectRef> {
        self.inner.owner_of(owner)
    }
}

impl<O: ClassObject, G: CurveOnClassifyObjects<O>> CurveOnClassifyObjects<O>
    for SnapshotObjects<'_, G>
{
    fn face(&self, f: FaceId) -> O {
        self.inner.face(f)
    }
    fn vertex(&self, v: VertexId) -> O {
        self.inner.vertex(v)
    }
    fn edge(&self, e: EdgeId) -> O {
        self.inner.edge(e)
    }
    fn vertexuse(&self, vu: VertexuseId) -> O {
        self.inner.vertexuse(vu)
    }
    fn edgeuse(&self, eu: EdgeuseId) -> O {
        self.inner.edgeuse(eu)
    }
    fn edge_curve(&self, e: EdgeId) -> O {
        self.inner.edge_curve(e)
    }
    fn trim_curve(&self, eu: EdgeuseId) -> O {
        self.inner.trim_curve(eu)
    }
    fn face_surface(&self, f: FaceId) -> Option<O> {
        self.inner.face_surface(f)
    }
    fn loop_id(&self, tgt_loop: O) -> Option<LoopId> {
        self.inner.loop_id(tgt_loop)
    }
}

impl<O: ClassObject, G: ClassGeometry<O>> ClassGeometryMut<O> for SnapshotObjects<'_, G> {
    fn curve_mut(&mut self, _o: O) -> Option<&mut Curve> {
        // the classification curve is the caller's (read-only); ClassifyFaceInterval
        // does not edit it
        None
    }
    fn topology_entity_mut(&mut self, o: O) -> Option<(&mut Brep, EntityRef)> {
        match self.inner.topology_entity(o) {
            Some((b, e)) if core::ptr::eq(b, self.brep) => Some((&mut *self.snapshot, e)),
            _ => None,
        }
    }
}

impl<G> FaceBrep for SnapshotObjects<'_, G> {
    fn brep(&self) -> &Brep {
        self.snapshot
    }
}

impl<O: ClassObject, G: CurveOnClassifyObjects<O>> CurveOnClassifyBrep<O>
    for SnapshotObjects<'_, G>
{
    fn brep_mut(&mut self) -> &mut Brep {
        self.snapshot
    }
    fn classify_surface(&self, f: FaceId) -> Result<Option<&Surface>> {
        // the caller's surface: its handles and gap lookups know that one
        Ok(self.brep.face(f)?.surface.as_ref())
    }
    fn keeps_made_trim(&self, eu: EdgeuseId) -> Result<bool> {
        Ok(eu1::uv_trim_curve(self.brep, eu)?.0.is_none())
    }
}

/// The classification objects `inner` (`&G` or `&mut G`) plus the UV trim curves the
/// classification made that `inner` cannot resolve (`trims`: their handles and curves;
/// empty for an adapter over the face's own B-rep).
///
/// `IntersectTSBound`'s `pVertexuse->ComputeUVPoint(sUV)` (SmCurveClass.cpp:15964,
/// `bOKToMakeUVTrimCurve = TRUE`) makes the missing UV trim curves of the vertex use's
/// edge use and its predecessor (`NormalizedEvaluate(.., bOKToMakeUVTrimCurve)`,
/// SmVertexuse.cpp:223-226) on the face's B-rep, so the edge use pass after it
/// (`pEU->GetUVTrimCurve(); NER(..)`, SmCurveClass.cpp:16092) and the interval
/// classification see them.
///
/// Every [`ClassGeometry`] and [`ClassGeometryMut`] method must be forwarded to
/// `inner` here, including any added later.
struct WithMadeTrimCurves<'t, O, R> {
    inner: R,
    trims: &'t [(O, Curve)],
}

impl<O: ClassObject, R: core::ops::Deref> ClassGeometry<O> for WithMadeTrimCurves<'_, O, R>
where
    R::Target: ClassGeometry<O>,
{
    fn curve(&self, o: O) -> Option<&Curve> {
        match self.trims.iter().find(|(h, _)| *h == o) {
            Some((_, c)) => Some(c),
            None => self.inner.curve(o),
        }
    }
    fn surface(&self, o: O) -> Option<&Surface> {
        self.inner.surface(o)
    }
    fn volume(&self, o: O) -> Option<&crate::surface::volume::Volume> {
        self.inner.volume(o)
    }
    fn point(&self, o: O) -> Option<Point3> {
        self.inner.point(o)
    }
    fn classification_vertex_point(&self, vertex: O) -> Option<Point3> {
        self.inner.classification_vertex_point(vertex)
    }
    fn topology_zone_tol_3d(&self, object: O) -> Option<f64> {
        self.inner.topology_zone_tol_3d(object)
    }
    fn vertex_face_gap(&self, vertex: O, surface: &Surface, face: Option<O>) -> Option<f64> {
        self.inner.vertex_face_gap(vertex, surface, face)
    }
    fn topology_entity(&self, object: O) -> Option<(&Brep, EntityRef)> {
        self.inner.topology_entity(object)
    }
    fn topology_handle(&self, entity: EntityRef) -> Option<O> {
        self.inner.topology_handle(entity)
    }
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<O> {
        self.inner.topology_handle_in(brep, entity)
    }
    fn topology_intersector_mate(&self, ti: O, entity: O) -> Option<Option<O>> {
        self.inner.topology_intersector_mate(ti, entity)
    }
    fn brep_id(&self, brep: &Brep) -> Option<crate::topology::BrepId> {
        self.inner.brep_id(brep)
    }
    fn owner_entity(&self, owner: crate::geom::object::ObjectRef) -> Option<(&Brep, EntityRef)> {
        self.inner.owner_entity(owner)
    }
    fn owner_of(
        &self,
        owner: crate::geom::object::ObjectRef,
    ) -> Option<crate::geom::object::ObjectRef> {
        self.inner.owner_of(owner)
    }
}

impl<O: ClassObject, R: core::ops::DerefMut> ClassGeometryMut<O> for WithMadeTrimCurves<'_, O, R>
where
    R::Target: ClassGeometryMut<O>,
{
    fn curve_mut(&mut self, o: O) -> Option<&mut Curve> {
        self.inner.curve_mut(o)
    }
    fn topology_entity_mut(&mut self, o: O) -> Option<(&mut Brep, EntityRef)> {
        self.inner.topology_entity_mut(o)
    }
}

/// The start of an `SmTemporaryChangeValue<SmObject*>(pSurface->m_pOwner, NULL)` scope
/// on the snapshot's face `f`: its surface loses its owner (so `GetFace()` finds no face
/// and the system zone tolerance applies), recorded on the B-rep
/// ([`Brep::begin_surface_owner_cleared`]); [`restore_surface_owner`] ends it.
fn clear_surface_owner(snapshot: &mut Brep, f: FaceId) -> Result<Option<fp2::RescueSurfaceOwner>> {
    let rescue = snapshot
        .face(f)?
        .surface
        .as_ref()
        .map(fp2::RescueSurfaceOwner::new);
    if let Some(s) = snapshot.face_mut(f)?.surface.as_mut() {
        s.set_owner(None);
    }
    snapshot.begin_surface_owner_cleared(f);
    Ok(rescue)
}

/// The end of a [`clear_surface_owner`] scope: the owner and face tolerance come back.
fn restore_surface_owner(
    snapshot: &mut Brep,
    f: FaceId,
    rescue: Option<fp2::RescueSurfaceOwner>,
) -> Result<()> {
    snapshot.end_surface_owner_cleared(f);
    if let (Some(rescue), Some(s)) = (rescue, snapshot.face_mut(f)?.surface.as_mut()) {
        rescue.restore(s);
    }
    snapshot.sync_face_surface_tolerance(f)
}

/// Build real ordered topology views; only handle identity is caller-specific.
/// UV computation uses the existing vertexuse port on the face's B-rep; the UV trim
/// curves it makes that `objects` cannot resolve are added to `made_trims`.
fn boundary_loops<O: ClassObject>(
    k: &Kernel,
    objects: &mut impl CurveOnClassifyBrep<O>,
    made_trims: &mut Vec<(O, Curve)>,
    face: FaceId,
    cached: bool,
) -> Result<Vec<BoundaryLoop<O>>> {
    let fu =
        fp2::upward_faceuse(objects.brep(), face)?.ok_or(SolidError::null_pointer(OP_CURVE))?;
    let loopuses = objects.brep().faceuse(fu)?.loopuses.clone();
    let mut loops = Vec::new();
    for lu in loopuses {
        let mut vertices = Vec::new();
        for vu in loopuse::vertexuses(objects.brep(), lu, None)? {
            let vu = vu.ok_or(SolidError::null_pointer(OP_CURVE))?;
            let b = objects.brep();
            let vertex = b
                .vertexuse(vu)?
                .vertex
                .ok_or(SolidError::null_pointer(OP_CURVE))?;
            let record = b.vertex(vertex)?;
            let (point, zone_tol_3d) = (record.point, record.tolerance);
            // Cached traversal supplies its own UV only for visited vertices.
            let uv = if cached {
                Point2::UNDEF
            } else {
                ignored(crate::topology::vertexuse::compute_uv_point(
                    k,
                    objects.brep_mut(),
                    vu,
                    true,
                ))?
                .unwrap_or(Point2::UNDEF)
            };
            vertices.push(BoundaryVertex {
                vertexuse: objects.vertexuse(vu),
                vertex: objects.vertex(vertex),
                uv,
                point,
                zone_tol_3d,
            });
        }
        let mut edges = Vec::new();
        for eu in loopuse::edgeuses(objects.brep(), lu, None, None)? {
            let edge = objects
                .brep()
                .edgeuse(eu)?
                .edge
                .ok_or(SolidError::null_pointer(OP_CURVE))?;
            // the trim curve as of now: ComputeUVPoint above may have made it
            let (trim, owner) = eu1::uv_trim_curve(objects.brep(), eu)?;
            let trim_curve = match (trim, owner) {
                (Some(trim), Some(owner)) => {
                    let handle = objects.trim_curve(owner);
                    if objects.keeps_made_trim(eu)? && !made_trims.iter().any(|(h, _)| *h == handle)
                    {
                        made_trims.push((handle, trim.clone()));
                    }
                    Some(handle)
                }
                _ => None,
            };
            edges.push(BoundaryEdgeuse {
                edgeuse: objects.edgeuse(eu),
                edge: boundary_edge(k, objects, face, edge, false)?,
                trim_curve,
            });
        }
        loops.push(BoundaryLoop { vertices, edges });
    }
    Ok(loops)
}

/// `SmFace::CurveOnClassify`: splits the curve of `cc` into intervals at its
/// intersections with the face boundary (the trim curves of a 2D curve, else the face's
/// vertices and edges, redone once when the intersection changed the curve) and
/// classifies each interval in, out or on the face (by the interval end
/// classifications, or by point classification with `do_point_classify`). Returns
/// `pbOptModifiedCurve`: whether the curve was modified and the classification rebuilt.
///
/// Runs on a private copy of `b` (module Deviations); callers that can lend the face's
/// B-rep mutably use [`curve_on_classify_mut`].
///
/// # Errors
///
/// [`SolidError::Unported`]: the boundary intersections and interval classification (see
/// the module Deviations); `SM_ERR` when the trim boundary intersection fails;
/// [`SolidError::InvalidInput`] for a classification without (resolvable) curve or an
/// unresolvable target loop.
#[allow(clippy::fn_params_excessive_bools)] // upstream's SmBoolean arguments
pub fn curve_on_classify<O: ClassObject>(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    objects: &impl CurveOnClassifyObjects<O>,
    do_point_classify: bool,
    cc: &mut CurveClassification<O>,
    has_big_gaps: bool,
    do_check_3d: bool,
    uv_space_okay: bool,
) -> Result<bool> {
    curve_on_classify_cached(
        k,
        b,
        f,
        objects,
        do_point_classify,
        cc,
        has_big_gaps,
        do_check_3d,
        uv_space_okay,
        &mut CurveOnClassifyCache::default(),
    )
}

/// The same classification with explicit query state (the private copy of `b` it runs
/// on), shared by the healer's seam directions and retained even when the
/// classification returns an error.
#[allow(clippy::fn_params_excessive_bools)] // upstream's SmBoolean arguments
pub(crate) fn curve_on_classify_cached<O: ClassObject>(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    objects: &impl CurveOnClassifyObjects<O>,
    do_point_classify: bool,
    cc: &mut CurveClassification<O>,
    has_big_gaps: bool,
    do_check_3d: bool,
    uv_space_okay: bool,
    cache: &mut CurveOnClassifyCache,
) -> Result<bool> {
    curve_on_classify_mut(
        k,
        f,
        &mut SnapshotObjects::new(objects, b, cache.snapshot(b)),
        do_point_classify,
        cc,
        has_big_gaps,
        do_check_3d,
        uv_space_okay,
    )
}

/// [`curve_on_classify`] on the face's own B-rep (`objects`' [`FaceBrep::brep`]): the
/// UV trim curves `ComputeUVPoint` / `GetOrCreateUVTrimCurve` make, the gap caches
/// `GetVertexFaceGap` / `GetMaxEdgeFaceGap` fill, the trimmed surface cache and the
/// tolerance growth it brings, and the edits of `ClassifyFaceInterval`'s face queries
/// stay on that B-rep, made in upstream's order.
///
/// # Errors
///
/// As [`curve_on_classify`].
#[allow(clippy::fn_params_excessive_bools)] // upstream's SmBoolean arguments
pub fn curve_on_classify_mut<O: ClassObject>(
    k: &Kernel,
    f: FaceId,
    objects: &mut impl CurveOnClassifyBrep<O>,
    do_point_classify: bool,
    cc: &mut CurveClassification<O>,
    has_big_gaps: bool,
    do_check_3d: bool,
    uv_space_okay: bool,
) -> Result<bool> {
    // init output
    reset_classification(k, cc)?;
    cc.set_face(Some(objects.face(f)));
    let mut modified = false;
    // the UV trim curves made by the trim boundary intersection's ComputeUVPoint calls
    // that `objects` cannot resolve (see WithMadeTrimCurves)
    let mut made_trims: Vec<(O, Curve)> = Vec::new();

    // for two iterations to allow the surf/surf/surf intersection heuristic a chance to
    // update curve and edge shapes.
    for ii in 0..2 {
        // skip 2nd classification when its not needed else set up for DoOver
        if ii > 0 {
            if cc.is_curve_current() {
                continue;
            }
            modified = true;
            reset_classification(k, cc)?;
        }

        let curve = classification_curve(&*objects, cc)?;
        if curve.dim() == 2 {
            // 2D Curve branch - work in surface's uv space directly on the Face's UVcurves
            cc.set_surface1(objects.face_surface(f));
            match intersect_ts_bound(k, objects, &mut made_trims, cc, f, do_check_3d) {
                Ok(()) => {}
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Err(SolidError::sm_err(OP_CURVE)),
            }
        } else {
            // 3D Curve Branch
            // 1st intersect face vertices with the curve
            let tgt_loops = cc
                .tgt_loops()
                .iter()
                .map(|&o| {
                    objects
                        .loop_id(o)
                        .ok_or_else(|| null(OP_CURVE, "unresolvable target loop"))
                })
                .collect::<Result<Vec<_>>>()?;
            let vertices = face_vertices(objects.brep(), f, cc.tgt_loops_only(), Some(&tgt_loops))?;
            if objects.classify_surface(f)?.is_none() {
                return Err(null(OP_CURVE, "face without surface"));
            }
            // bHasBigGaps: each vertex's GetVertexuseOfFace(pFace)->GetVertexFaceGap()
            // (pFace is this face, set on the classification above)
            let gaps = if has_big_gaps {
                // IntersectVertices reads them with the surface's owner cleared
                // (SmTemporaryChangeValue on m_pOwner), so a surface cache made for
                // them is a standalone one
                let rescue = clear_surface_owner(objects.brep_mut(), f)?;
                let mut gaps = Ok(Vec::new());
                for &v in &vertices {
                    match vertex_face_gap_length(k, objects.brep_mut(), v, f) {
                        Ok(gap) => {
                            if let Ok(g) = gaps.as_mut() {
                                g.push((objects.vertex(v), gap));
                            }
                        }
                        Err(e) => {
                            gaps = Err(e);
                            break;
                        }
                    }
                }
                restore_surface_owner(objects.brep_mut(), f, rescue)?;
                gaps?
            } else {
                Vec::new()
            };
            let vertices = vertices
                .iter()
                .map(|&v| objects.vertex(v))
                .collect::<Vec<_>>();
            // (an evaluation copy of the face's surface: the classification below edits
            // through `objects`; the copy keeps the owner, so the face tolerance)
            let surface = objects
                .classify_surface(f)?
                .ok_or_else(|| null(OP_CURVE, "face without surface"))?
                .clone_for_evaluation()?;
            // the surf/surf/surf block's edits (sm_SetXSectCurveThroughPoint on the
            // classification curve, the vertices' edge uses' UV trim curves) are made
            let mut geometry = WithVertexFaceGaps {
                inner: &mut *objects,
                gaps,
            };
            cc.intersect_vertices_mut(k, &mut geometry, &vertices, &surface, has_big_gaps, true)?;

            // 2nd intersect face edges with the curve
            let edges = face_edges(objects.brep(), f, cc.tgt_loops_only(), Some(&tgt_loops))?;
            intersect_edges(k, objects, cc, &edges, f, has_big_gaps)?;
        }
    }

    // 2. classify each interval relative to the face
    let src_zone_tol_3d = src_zone_tol_3d_of(k, Some(cc));
    let face_handle = objects.face(f);
    for ii in 0..cc.size() {
        let degenerate = classification_curve(&*objects, cc)?.is_degenerate(k, EFF_ZERO, None)?;
        let ivl = cc.at_mut(ii)?;

        // set Face_BackPtr and SrcZoneTol3d values in interval's PointClassifications
        for p in [&mut ivl.start, &mut ivl.mid, &mut ivl.end] {
            p.set_face(Some(face_handle));
            p.set_src_zone_tol_3d(src_zone_tol_3d);
        }

        // when curve is degenerate
        if degenerate {
            ivl.mid = ivl.start;
            ivl.end = ivl.start;
        }

        // low work - go to next interval
        if !do_point_classify
            && ivl.start.point_class() == PointClassType::Unknown
            && ivl.end.point_class() == PointClassType::Unknown
        {
            continue;
        }

        // WR-305: an interval running along a boundary edge between two points on it is
        // on that edge
        if k.fixes.classify_edge_interval {
            cc.classify_edge_interval(k, &*objects, ii)?;
        }

        // classify the interval - avoiding point classification if possible
        if cc.at(ii)?.mid.point_class() == PointClassType::Unknown {
            cc.classify_face_interval_mut(
                k,
                &mut WithMadeTrimCurves {
                    inner: &mut *objects,
                    trims: made_trims.as_slice(),
                },
                ii,
                do_point_classify,
                face_handle,
                None,
                uv_space_okay,
                None,
            )?;
        }
    }
    Ok(modified)
}

/// The curve of `cc` (`GetCurve()`, which upstream dereferences).
fn classification_curve<'o, O: ClassObject>(
    objects: &'o impl CurveOnClassifyObjects<O>,
    cc: &CurveClassification<O>,
) -> Result<&'o Curve> {
    cc.curve()
        .and_then(|c| objects.curve(c))
        .ok_or_else(|| null(OP_CURVE, "classification without curve"))
}

/// `SmCurveClassification::ReSet()` with its defaults.
fn reset_classification<O: ClassObject>(k: &Kernel, cc: &mut CurveClassification<O>) -> Result<()> {
    cc.reset(k, &ResetOptions::default())
}

/// `SmCurveClassification::IntersectTSBound(this, bDoCheck3d)`, on the face's B-rep
/// (see [`WithMadeTrimCurves`]).
fn intersect_ts_bound<O: ClassObject>(
    k: &Kernel,
    objects: &mut impl CurveOnClassifyBrep<O>,
    made_trims: &mut Vec<(O, Curve)>,
    cc: &mut CurveClassification<O>,
    face: FaceId,
    do_check_3d: bool,
) -> Result<()> {
    if objects.classify_surface(face)?.is_none() {
        return Err(SolidError::null_pointer(OP_CURVE));
    }
    // Face->surface and TrimCache: smsurf_GetSurfaceCache(pSurface) (made on the
    // face's B-rep); NER of a surface cache that is not a trimmed one
    let lease = face_surface_cache(k, objects.brep_mut(), face)?;
    if !lease.trimmed {
        lease.release();
        return Err(SolidError::null_pointer(OP_CURVE));
    }
    // the cached branch reads the vertex uses' UV points from the cache
    let base = lease.cache.base();
    let cached = !base.face_was_modified
        && base.have_t_surface_cache
        && base.face_containment_done != FaceContainment::NotDone;
    let result = boundary_loops(k, objects, made_trims, face, cached).and_then(|loops| {
        let visits = {
            let surface = objects
                .classify_surface(face)?
                .ok_or(SolidError::null_pointer(OP_CURVE))?;
            let objects_r = &*objects;
            let cache_handle = |h: EntityRef| match h {
                EntityRef::Vertexuse(vu) => Some(objects_r.vertexuse(vu)),
                EntityRef::Edgeuse(eu) => Some(objects_r.edgeuse(eu)),
                _ => None,
            };
            let boundary = BoundaryFace {
                surface,
                loops: &loops,
                cache: &lease.cache,
                cache_handle: &cache_handle,
            };
            let geometry = WithMadeTrimCurves {
                inner: objects_r,
                trims: made_trims.as_slice(),
            };
            match cc.ts_bound_cached_visits(k, &geometry, &boundary)? {
                // no up-to-date trimmed cache: every vertex use and edge use of the face
                None => return cc.intersect_ts_bound(k, &geometry, &boundary, do_check_3d),
                // no work - curve is outside of the domain of the surface
                Some(visits) if visits.no_nodes => return Ok(()),
                Some(visits) => visits,
            }
        };
        cached_ts_bound(k, objects, made_trims, cc, face, visits, do_check_3d)
    });
    lease.release();
    result
}

/// The traversals of `IntersectTSBound`'s up-to-date trimmed cache branch, under its
/// context mark (`SmNewMarkAndLock`): the vertex uses, then the edge uses, whose
/// `GetOrCreateUVTrimCurve` makes a missing UV trim curve on the face's B-rep (status
/// ignored, `SE`) and whose edge tolerance is read when it is visited.
fn cached_ts_bound<O: ClassObject>(
    k: &Kernel,
    objects: &mut impl CurveOnClassifyBrep<O>,
    made_trims: &mut Vec<(O, Curve)>,
    cc: &mut CurveClassification<O>,
    face: FaceId,
    visits: crate::intersect::curve_class::p9::CachedVisits<O>,
    do_check_3d: bool,
) -> Result<()> {
    // the face's edge uses behind the handles
    let b = objects.brep();
    let fu = fp2::upward_faceuse(b, face)?.ok_or(SolidError::null_pointer(OP_CURVE))?;
    let mut ids = Vec::new();
    for &lu in &b.faceuse(fu)?.loopuses {
        for eu in loopuse::edgeuses(b, lu, None, None)? {
            let edge = b
                .edgeuse(eu)?
                .edge
                .ok_or(SolidError::null_pointer(OP_CURVE))?;
            ids.push((objects.edgeuse(eu), eu, edge));
        }
    }

    // increment the mark on the context by 1
    let _mark_lock = k.context.claim_mark(crate::geom::context::ALL_MARKS);
    for vertex in visits.vertices {
        let surface = objects
            .classify_surface(face)?
            .ok_or(SolidError::null_pointer(OP_CURVE))?;
        cc.ts_bound_vertex(
            k,
            &WithMadeTrimCurves {
                inner: &*objects,
                trims: made_trims.as_slice(),
            },
            surface,
            vertex,
            do_check_3d,
        )?;
    }
    for mut edgeuse in visits.edgeuses {
        let &(_, eu, edge) = ids
            .iter()
            .find(|(h, _, _)| *h == edgeuse.edgeuse)
            .ok_or(SolidError::null_pointer(OP_CURVE))?;
        // get/create the UVCurve for this Edgeuse
        if edgeuse.trim_curve.is_none() {
            let fetch = crate::topology::edgeuse::p3::get_or_create_uv_trim_curve(
                k,
                objects.brep_mut(),
                eu,
                false,
                false,
                true,
            )?;
            // SE: a failed status is only reported
            let _ = fetch.status;
            if let (Some(trim), Some(owner)) = eu1::uv_trim_curve(objects.brep(), eu)? {
                let handle = objects.trim_curve(owner);
                if objects.keeps_made_trim(eu)? && !made_trims.iter().any(|(h, _)| *h == handle) {
                    made_trims.push((handle, trim.clone()));
                }
                edgeuse.trim_curve = Some(handle);
            }
        }
        // SmTol::GetZoneTol3d(pE) as it is now
        edgeuse.edge = boundary_edge(k, objects, face, edge, false)?;
        let surface = objects
            .classify_surface(face)?
            .ok_or(SolidError::null_pointer(OP_CURVE))?;
        cc.ts_bound_edgeuse(
            k,
            &WithMadeTrimCurves {
                inner: &*objects,
                trims: made_trims.as_slice(),
            },
            surface,
            edgeuse,
        )?;
    }
    Ok(())
}

/// `SmFace::GetEdges(SmTArray&, NULL, bTgtLoopsOnly, pOptTgtLoops)`.
fn face_edges(
    b: &Brep,
    f: FaceId,
    tgt_loops_only: bool,
    tgt_loops: Option<&[LoopId]>,
) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, tgt_loops_only, tgt_loops)
}

/// `SmCurveClassification::IntersectEdges(edges, *this, bHasBigGaps)`, its gap reads on
/// the face's B-rep.
fn intersect_edges<O: ClassObject>(
    k: &Kernel,
    objects: &mut impl CurveOnClassifyBrep<O>,
    cc: &mut CurveClassification<O>,
    edges: &[EdgeId],
    face: FaceId,
    has_big_gaps: bool,
) -> Result<()> {
    if objects.classify_surface(face)?.is_none() {
        return Err(SolidError::null_pointer(OP_CURVE));
    }
    // IntersectEdges reads the edge/face gaps with the surface's owner cleared
    // (SmTemporaryChangeValue on m_pOwner)
    let rescue = clear_surface_owner(objects.brep_mut(), face)?;
    let mut views = Ok(Vec::new());
    for &edge in edges {
        match boundary_edge(k, objects, face, edge, has_big_gaps) {
            Ok(view) => {
                if let Ok(v) = views.as_mut() {
                    v.push(view);
                }
            }
            Err(e) => {
                views = Err(e);
                break;
            }
        }
    }
    let views = match views {
        Ok(v) => v,
        Err(e) => {
            restore_surface_owner(objects.brep_mut(), face, rescue)?;
            return Err(e);
        }
    };
    // the owner stays cleared over the edge loop, as upstream's SmTemporaryChangeValue;
    // the insertions refine and edit the curves through the mutable adapter
    // (bUVSpaceOkay is TRUE: the surface lifts no 2D source)
    let r = cc.intersect_edges_mut(
        k,
        objects,
        &views,
        None,
        IntersectEdgesOptions {
            has_big_gaps,
            ..IntersectEdgesOptions::default()
        },
    );
    restore_surface_owner(objects.brep_mut(), face, rescue)?;
    r
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::curve::curve::Curve;
    use crate::curve::line::{BoundedLineOptions, Line};
    use crate::geom::extent1d::Extent1d;
    use crate::geom::vector2d::Vec2;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::surface::plane::Plane;
    use crate::topology::{
        Edge, Edgeuse, EdgeuseId, EdgeuseOwner, Face, Faceuse, FaceuseId, Loop, Loopuse, LoopuseId,
        LoopuseStart, Vertex, Vertexuse, VertexuseId, VertexuseOwner,
    };

    /// A unit-square planar face over `[0, 1]^2` with UV trim lines.
    pub(crate) fn square(b: &mut Brep) -> FaceId {
        let s = b.add_shell().unwrap();
        let o = Point3::new(0.0, 0.0, 0.0);
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let plane = Plane::new(o, Vec3::X, Vec3::Y, Vec2::new(1.0, 1.0), &dom);
        let f = b
            .add_face_record(Face {
                surface: Some(Surface::from(plane)),
                faceuse: Some(FaceuseId::from_index(0)),
                uv_domain: dom,
                tolerance: 1e-6,
                ..Face::new(s)
            })
            .unwrap();
        let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        for &(x, y) in &corners {
            b.add_vertex_record(Vertex {
                point: Point3::new(x, y, 0.0),
                tolerance: 1e-6,
                ..Vertex::default()
            })
            .unwrap();
        }
        b.add_faceuse_record(Faceuse {
            shell: Some(s),
            face: Some(f),
            mate: None,
            orientation: OrientType::Same,
            loopuses: vec![LoopuseId::from_index(0)],
            sibling_links: None,
        })
        .unwrap();
        b.add_loopuse_record(Loopuse {
            faceuse: Some(FaceuseId::from_index(0)),
            loop_: Some(LoopId::from_index(0)),
            mate: None,
            start: LoopuseStart::Edgeuse(Some(EdgeuseId::from_index(0))),
            ..Loopuse::default()
        })
        .unwrap();
        b.add_loop_record(Loop {
            face: f,
            edges: Vec::new(),
            loopuse: Some(LoopuseId::from_index(0)),
        })
        .unwrap();
        for i in 0..4u32 {
            let (x0, y0) = corners[i as usize];
            let (x1, y1) = corners[((i + 1) % 4) as usize];
            let a = Point3::new(x0, y0, 0.0);
            let c = Point3::new(x1, y1, 0.0);
            let unit = Extent1d::from_min_max(0.0, 1.0);
            let line = |a: Point3, c: Point3| {
                Curve::from(Line::bounded(
                    a,
                    c - a,
                    &unit,
                    &BoundedLineOptions::default(),
                ))
            };
            b.add_edge_record(Edge {
                vertices: [VertexId::from_index(i), VertexId::from_index((i + 1) % 4)],
                edgeuses: vec![EdgeuseId::from_index(i)],
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 1e-6,
                curve: Some(line(a, c)),
            })
            .unwrap();
            let uv_line = Curve::from(Line::bounded(
                a,
                c - a,
                &unit,
                &BoundedLineOptions {
                    dim: 2,
                    ..BoundedLineOptions::default()
                },
            ));
            b.add_edgeuse_record(Edgeuse {
                edge: Some(EdgeId::from_index(i)),
                owner: EdgeuseOwner::Loopuse(Some(LoopuseId::from_index(0))),
                ccw: Some(EdgeuseId::from_index((i + 1) % 4)),
                cw: Some(EdgeuseId::from_index((i + 3) % 4)),
                vertexuse: Some(VertexuseId::from_index(i)),
                orientation: OrientType::Same,
                uv_trim_curve: Some(uv_line),
                ..Edgeuse::default()
            })
            .unwrap();
            b.add_vertexuse_record(Vertexuse {
                vertex: Some(VertexId::from_index(i)),
                owner: VertexuseOwner::Edgeuse(Some(EdgeuseId::from_index(i))),
                ..Vertexuse::default()
            })
            .unwrap();
        }
        f
    }

    fn classify(
        b: &mut Brep,
        f: FaceId,
        uv: Point2,
    ) -> (Result<()>, PointClassification<EntityRef>) {
        let k = Kernel::default();
        let mut pc = PointClassification::new(1e-6);
        let r = point_classify(&k, b, f, uv, &mut pc, PointClassifyOptions::default());
        (r, pc)
    }

    #[test]
    fn outside_the_domain_is_unknown_without_work() {
        let mut b = Brep::new();
        let f = square(&mut b);
        let (r, pc) = classify(&mut b, f, Point2::new(2.0, 0.5));
        assert!(r.is_ok());
        assert_eq!(pc.point_class(), PointClassType::Unknown);
        assert_eq!(pc.object(), None);
    }

    #[test]
    fn vertices_are_found_first() {
        let mut b = Brep::new();
        let f = square(&mut b);
        assert_eq!(
            face_vertices(&b, f, false, None).unwrap(),
            (0..4).map(VertexId::from_index).collect::<Vec<_>>()
        );
        let (r, pc) = classify(&mut b, f, Point2::new(1.0, 1.0));
        assert!(r.is_ok());
        assert_eq!(pc.point_class(), PointClassType::Vertex);
        assert_eq!(
            pc.object(),
            Some(EntityRef::Vertex(VertexId::from_index(2)))
        );
        assert_eq!(pc.pre_snap_param(), BIG_DOUBLE);
    }

    #[test]
    fn interior_point_is_on_the_face() {
        let mut b = Brep::new();
        let f = square(&mut b);
        let (r, pc) = classify(&mut b, f, Point2::new(0.25, 0.5));
        r.unwrap();
        assert_eq!(pc.point_class(), PointClassType::Face);
        assert_eq!(pc.object(), Some(EntityRef::Face(f)));
        assert_eq!(pc.uv_param(), Point2::new(0.25, 0.5));
        assert_eq!(pc.src_zone_tol_3d(), 1e-6);
    }

    #[test]
    fn face_surface_cache_is_kept_on_the_surface_and_reused() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = square(&mut b);
        let surface = || b.face(f).unwrap().surface.as_ref().unwrap();
        assert!(!surface().has_trim_cache());

        // the first lookup makes the cache and keeps it on the surface
        let mut lease = face_surface_cache(&k, &mut b, f).unwrap();
        assert!(lease.trimmed);
        let surface = b.face(f).unwrap().surface.as_ref().unwrap();
        assert!(surface.trim_cache_in_use());
        // a lookup while it is in use gets a cache that is not kept
        let nested = face_surface_cache(&k, &mut b, f).unwrap();
        assert!(!nested.kept);
        nested.release();
        // a later lookup finds the same object, with what its use did to it
        lease.cache.base_mut().process_boundary_curves = true;
        let flag_before = lease.cache.base().point_test_enabled;
        lease.cache.base_mut().point_test_enabled = !flag_before;
        lease.release();
        let surface = b.face(f).unwrap().surface.as_ref().unwrap();
        assert!(surface.has_trim_cache() && !surface.trim_cache_in_use());
        let mut lease = face_surface_cache(&k, &mut b, f).unwrap();
        assert!(lease.kept);
        assert_eq!(lease.cache.base().point_test_enabled, !flag_before);

        // a modified face while its B-rep is edited: still reused
        lease.cache.base_mut().face_was_modified = true;
        lease.release();
        b.set_editing_enabled(true);
        let lease = face_surface_cache(&k, &mut b, f).unwrap();
        assert!(lease.cache.base().face_was_modified);
        lease.release();
        // editing over: deleted and made again
        b.set_editing_enabled(false);
        let lease = face_surface_cache(&k, &mut b, f).unwrap();
        assert!(!lease.cache.base().face_was_modified);
        assert_eq!(lease.cache.base().point_test_enabled, flag_before);
        lease.release();

        // a pre-edit notification marks the kept cache modified, also while in use
        let surface = b.face(f).unwrap().surface.as_ref().unwrap();
        surface.mark_trim_cache_face_modified();
        let lease = face_surface_cache(&k, &mut b, f).unwrap();
        // (editing is off: the modified cache was made again)
        assert!(!lease.cache.base().face_was_modified);
        b.face(f)
            .unwrap()
            .surface
            .as_ref()
            .unwrap()
            .mark_trim_cache_face_modified();
        lease.release();
        b.set_editing_enabled(true);
        let lease = face_surface_cache(&k, &mut b, f).unwrap();
        assert!(lease.cache.base().face_was_modified);
        lease.release();
    }

    #[test]
    fn a_cache_made_while_the_owner_is_cleared_is_standalone_and_kept() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = square(&mut b);
        b.begin_surface_owner_cleared(f);
        let lease = face_surface_cache(&k, &mut b, f).unwrap();
        assert!(lease.kept && !lease.cache.base().have_t_surface_cache);
        lease.release();
        b.end_surface_owner_cleared(f);
        // reused as it is: no face data, so IntersectTSBound walks the loop uses
        let lease = face_surface_cache(&k, &mut b, f).unwrap();
        assert!(lease.kept && !lease.cache.base().have_t_surface_cache);
        lease.release();
    }

    #[test]
    fn a_classification_snapshot_shares_the_surface_caches() {
        let mut b = Brep::new();
        let f = square(&mut b);
        let snapshot = b.clone_sharing_surface_caches();
        let original = b.face(f).unwrap().surface.as_ref().unwrap();
        let copy = snapshot.face(f).unwrap().surface.as_ref().unwrap();
        assert!(copy.shares_cache_slot_with(original));
        // a plain copy (the copy constructor) does not
        let plain = b.clone();
        let copy = plain.face(f).unwrap().surface.as_ref().unwrap();
        assert!(!copy.shares_cache_slot_with(original));
    }

    #[test]
    fn prepared_classification_matches_mutable_state() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = square(&mut b);
        let targets = [LoopId::from_index(0)];
        for tgt_loops_only in [false, true] {
            for (uv, expected) in [
                (Point2::new(2.0, 0.5), PointClassType::Unknown),
                (Point2::new(1.0, 1.0), PointClassType::Vertex),
                (Point2::new(0.25, 0.5), PointClassType::Face),
                (Point2::new(0.5, 0.0), PointClassType::Edge),
                (Point2::new(1.0, 0.5), PointClassType::Edge),
            ] {
                let mut prepared = PointClassification::new(7e-6);
                prepared.set_face(Some(EntityRef::Face(f)));
                prepared.set_t_param(0.125, false);
                prepared.set_uv_param(Point2::new(0.75, 0.25));
                prepared.set_sss_point(Point3::new(3.0, 4.0, 5.0));
                let mut mutable = prepared;
                let options = || PointClassifyOptions {
                    tgt_loops_only,
                    tgt_loops: Some(&targets),
                    src_zone_tol_3d: 0.25, // ignored upstream: pc's tolerance is used
                    do_boundary_intersections: true,
                    use_uv_classification: true,
                    uv_space_okay: false,
                    ..PointClassifyOptions::default()
                };
                point_classify_prepared(&k, &b, f, uv, &mut prepared, options()).unwrap();
                point_classify(&k, &mut b, f, uv, &mut mutable, options()).unwrap();
                assert_eq!(prepared.point_class(), expected);
                assert_eq!(prepared, mutable, "uv={uv:?}, target-only={tgt_loops_only}");
            }
        }
    }

    #[test]
    fn prepared_classification_does_not_require_unvisited_trims() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = square(&mut b);
        for i in 0..4 {
            b.edgeuse_mut(EdgeuseId::from_index(i))
                .unwrap()
                .uv_trim_curve = None;
        }
        for (uv, expected) in [
            (Point2::new(2.0, 0.5), PointClassType::Unknown),
            (Point2::new(1.0, 1.0), PointClassType::Vertex),
        ] {
            let mut prepared = PointClassification::new(1e-6);
            let mut mutable = prepared;
            point_classify_prepared(
                &k,
                &b,
                f,
                uv,
                &mut prepared,
                PointClassifyOptions::default(),
            )
            .unwrap();
            point_classify(
                &k,
                &mut b,
                f,
                uv,
                &mut mutable,
                PointClassifyOptions::default(),
            )
            .unwrap();
            assert_eq!(prepared.point_class(), expected);
            assert_eq!(prepared, mutable);
        }
        let mut pc = PointClassification::new(1e-6);
        assert!(matches!(
            point_classify_prepared(
                &k,
                &b,
                f,
                Point2::new(0.5, 0.5),
                &mut pc,
                PointClassifyOptions::default(),
            ),
            Err(SolidError::Unported {
                unit: "topology.edgeuse_p3",
                ..
            })
        ));
        assert!(b
            .edgeuses()
            .iter()
            .all(|(_, eu)| eu.uv_trim_curve.is_none()));
    }

    #[test]
    fn out_of_bounds_evaluation_copy_keeps_surface_tolerance() {
        let mut b = Brep::new();
        let f = square(&mut b);
        let surface = b.face_mut(f).unwrap().surface.as_mut().unwrap();
        let SurfaceKind::Plane(plane) = surface.kind_mut() else {
            panic!("expected plane")
        };
        *plane.out_of_bounds_enabled_mut() = false;
        surface.set_owner(Some(crate::geom::object::ObjectRef {
            type_code: crate::geom::assert_array::labels::FACE_TYPE,
            key: crate::geom::attribute::ObjectKey(u64::from(f.index())),
        }));
        surface.set_owner_zone_tol_3d(Some(0.03125));
        let copy = with_out_of_bounds_enabled(surface).unwrap().unwrap();
        assert_eq!(copy.owner(), surface.owner());
        assert_eq!(copy.owner_zone_tol_3d(), Some(0.03125));
        let SurfaceKind::Plane(copy_plane) = copy.kind() else {
            panic!("expected plane")
        };
        assert!(copy_plane.base().is_out_of_bounds_enabled());
        let SurfaceKind::Plane(original_plane) = surface.kind() else {
            panic!("expected plane")
        };
        assert!(!original_plane.base().is_out_of_bounds_enabled());
    }

    #[test]
    fn classify_param_on_faces_by_domain() {
        let mut b = Brep::new();
        let f = square(&mut b);
        let k = Kernel::default();
        let d1 = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let d2 = Extent2d::from_bounds(2.0, 2.0, 3.0, 3.0);
        let uv = Point2::new(0.5, 0.5);
        assert_eq!(
            classify_param_on_faces(&k, &mut b, uv, 0.0, f, &d1, f, &d2).unwrap(),
            1
        );
        assert_eq!(
            classify_param_on_faces(&k, &mut b, uv, 0.0, f, &d2, f, &d1).unwrap(),
            2
        );
        assert_eq!(
            classify_param_on_faces(&k, &mut b, uv, 0.0, f, &d1, f, &d1).unwrap(),
            1
        );
    }

    #[test]
    fn missing_surface_is_a_null_pointer() {
        let mut b = Brep::new();
        let f = square(&mut b);
        b.face_mut(f).unwrap().surface = None;
        let (r, _) = classify(&mut b, f, Point2::new(0.5, 0.5));
        assert!(matches!(
            r,
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }

    #[test]
    fn proto_vertex_containment_uses_the_larger_tolerance() {
        let e = Brep::new();
        let mut mgr: ProtoTopologyManager<EntityRef> = ProtoTopologyManager::new(
            &Kernel::default(),
            (EntityRef::Face(FaceId::from_index(0)), &e),
            (EntityRef::Face(FaceId::from_index(1)), &e),
            EntityRef::Face(FaceId::from_index(2)),
            1e-3,
            0.5,
            0.0,
        )
        .unwrap();
        let id = mgr
            .new_proto_vertex(Point3::new(0.0, 0.0, 0.0), 0.1)
            .unwrap();
        let pv = mgr.proto_vertex(id).unwrap().clone();
        assert!(proto_vertex_contains_point(
            &mgr,
            &pv,
            Point3::new(0.4, 0.0, 0.0)
        ));
        assert!(!proto_vertex_contains_point(
            &mgr,
            &pv,
            Point3::new(0.6, 0.0, 0.0)
        ));
        assert_eq!(
            proto_vertex_distance_to(&pv, Point3::new(3.0, 4.0, 0.0)),
            5.0
        );
        mgr.set_capture_distance(0.0);
        assert_eq!(proto_vertex_capture_distance(&mgr), 0.0);
        assert!(!proto_vertex_contains_point(
            &mgr,
            &pv,
            Point3::new(0.4, 0.0, 0.0)
        ));
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Handle {
        Topology(EntityRef),
        Source,
        EdgeCurve(EdgeId),
        Trim(EdgeuseId),
        Surface(FaceId),
    }
    impl ClassObject for Handle {
        fn class_kind(&self) -> ClassObjectKind {
            match self {
                Self::Topology(entity) => entity.class_kind(),
                Self::Source | Self::EdgeCurve(_) | Self::Trim(_) => ClassObjectKind::Curve,
                Self::Surface(_) => ClassObjectKind::Surface,
            }
        }
    }
    struct Handles<'a> {
        b: &'a Brep,
        source: Curve,
    }
    impl ClassGeometry<Handle> for Handles<'_> {
        fn curve(&self, h: Handle) -> Option<&Curve> {
            match h {
                Handle::Source => Some(&self.source),
                Handle::EdgeCurve(e) => self.b.edge(e).ok()?.curve.as_ref(),
                Handle::Trim(eu) => eu1::uv_trim_curve(self.b, eu).ok()?.0,
                _ => None,
            }
        }
        fn surface(&self, h: Handle) -> Option<&Surface> {
            if let Handle::Surface(f) = h {
                self.b.face(f).ok()?.surface.as_ref()
            } else {
                None
            }
        }
        fn volume(&self, _: Handle) -> Option<&crate::surface::volume::Volume> {
            None
        }
        fn point(&self, _: Handle) -> Option<Point3> {
            None
        }
        fn topology_zone_tol_3d(&self, h: Handle) -> Option<f64> {
            match h {
                Handle::Topology(EntityRef::Vertex(v)) => {
                    self.b.vertex(v).ok().map(|v| v.tolerance)
                }
                Handle::Topology(EntityRef::Edge(e)) => self.b.edge(e).ok().map(|e| e.tolerance),
                _ => None,
            }
        }
    }
    impl CurveOnClassifyObjects<Handle> for Handles<'_> {
        fn face(&self, f: FaceId) -> Handle {
            Handle::Topology(EntityRef::Face(f))
        }
        fn vertex(&self, v: VertexId) -> Handle {
            Handle::Topology(EntityRef::Vertex(v))
        }
        fn edge(&self, e: EdgeId) -> Handle {
            Handle::Topology(EntityRef::Edge(e))
        }
        fn vertexuse(&self, vu: VertexuseId) -> Handle {
            Handle::Topology(EntityRef::Vertexuse(vu))
        }
        fn edgeuse(&self, eu: EdgeuseId) -> Handle {
            Handle::Topology(EntityRef::Edgeuse(eu))
        }
        fn edge_curve(&self, e: EdgeId) -> Handle {
            Handle::EdgeCurve(e)
        }
        fn trim_curve(&self, eu: EdgeuseId) -> Handle {
            Handle::Trim(eu)
        }
        fn face_surface(&self, f: FaceId) -> Option<Handle> {
            Some(Handle::Surface(f))
        }
        fn loop_id(&self, _: Handle) -> Option<LoopId> {
            None
        }
    }

    fn with_same_face_sibling(b: &mut Brep) -> (FaceId, VertexuseId) {
        let f = square(b);
        for i in 0..4 {
            b.vertex_mut(VertexId::from_index(i))
                .unwrap()
                .vertexuses
                .push(VertexuseId::from_index(i));
        }
        let sibling = b
            .add_vertexuse_record(Vertexuse {
                vertex: Some(VertexId::from_index(0)),
                owner: VertexuseOwner::Loopuse(Some(LoopuseId::from_index(0))),
                ..Vertexuse::default()
            })
            .unwrap();
        b.vertex_mut(VertexId::from_index(0))
            .unwrap()
            .vertexuses
            .push(sibling);
        b.vertexuse_mut(sibling).unwrap().vertex_face_gap = crate::heal::gap::VertexFaceGap {
            gap_3d: 7.0,
            vertexuse: Some(sibling),
            face_uv: Point2::ZERO,
        };
        (f, sibling)
    }

    #[test]
    fn query_gap_state_retains_sibling_invalidation_after_classification_error() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let (f, sibling) = with_same_face_sibling(&mut b);
        let objects = Handles {
            b: &b,
            source: Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3)),
        };
        let mut cc = CurveClassification::with_curve_interval(
            &k,
            Handle::Source,
            Extent1d::from_min_max(0.0, 1.0),
            1e-6,
        );
        let mut cache = CurveOnClassifyCache::default();
        // Handles deliberately supplies no vertex points: IntersectVertices fails,
        // but its preceding lazy gap queries have already written their caches.
        let result = curve_on_classify_cached(
            &k, &b, f, &objects, false, &mut cc, true, false, false, &mut cache,
        );
        assert!(matches!(result, Err(SolidError::InvalidInput { .. })));
        assert_eq!(b.vertexuse(sibling).unwrap().vertex_face_gap.gap_3d, 7.0);
        let snapshot = cache.snapshot.as_ref().unwrap();
        let first = VertexuseId::from_index(0);
        assert!(snapshot.vertexuse(first).unwrap().vertex_face_gap.is_init());
        assert_eq!(
            snapshot.vertexuse(sibling).unwrap().vertex_face_gap,
            crate::heal::gap::VertexFaceGap::new()
        );
        cache.apply_gap_updates(&mut b).unwrap();
        assert!(b.vertexuse(first).unwrap().vertex_face_gap.is_init());
        assert_eq!(
            b.vertexuse(sibling).unwrap().vertex_face_gap,
            crate::heal::gap::VertexFaceGap::new()
        );
        assert_eq!(b.vertex(VertexId::from_index(0)).unwrap().point, Vec3::ZERO);
    }

    #[test]
    fn query_gap_state_keeps_resets_before_a_failed_projection() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let (f, sibling) = with_same_face_sibling(&mut b);
        let first = VertexuseId::from_index(0);
        b.vertexuse_mut(first).unwrap().vertex_face_gap = crate::heal::gap::VertexFaceGap {
            gap_3d: 3.0,
            vertexuse: Some(first),
            face_uv: Point2::ZERO,
        };
        b.face_mut(f).unwrap().surface = None;
        let mut cache = CurveOnClassifyCache {
            snapshot: Some(b.clone()),
        };
        let result = crate::topology::vertexuse::calc_vertex_face_gap(
            &k,
            cache.snapshot.as_mut().unwrap(),
            first,
            false,
            true,
        );
        assert!(result.is_err());
        cache.apply_gap_updates(&mut b).unwrap();
        for vu in [first, sibling] {
            assert_eq!(
                b.vertexuse(vu).unwrap().vertex_face_gap,
                crate::heal::gap::VertexFaceGap::new()
            );
        }
    }

    #[test]
    fn unused_query_cache_does_not_force_gap_calculation() {
        let mut b = Brep::new();
        let (_, sibling) = with_same_face_sibling(&mut b);
        let before = *b.vertexuse(sibling).unwrap();
        CurveOnClassifyCache::default()
            .apply_gap_updates(&mut b)
            .unwrap();
        assert_eq!(b.vertexuse(sibling).unwrap(), &before);
        assert!(!b
            .vertexuse(VertexuseId::from_index(0))
            .unwrap()
            .vertex_face_gap
            .is_init());
    }

    #[test]
    fn nonempty_boundary_adapters_reach_both_part9_methods() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = square(&mut b);
        // Edge-loop UV computation retains its genuine NormalizedEvaluate
        // dependency. An isolated vertex loop can use the existing projection.
        b.vertexuse_mut(VertexuseId::from_index(0)).unwrap().owner =
            VertexuseOwner::Loopuse(Some(LoopuseId::from_index(0)));
        b.loopuse_mut(LoopuseId::from_index(0)).unwrap().start =
            LoopuseStart::Vertexuse(Some(VertexuseId::from_index(0)));
        let objects = Handles {
            b: &b,

            source: Curve::from(Line::from_points(
                Point3::new(0.2, 1.5, 0.),
                Point3::new(0.8, 1.5, 0.),
                2,
            )),
        };
        let mut copy = b.clone();
        let loops = boundary_loops(
            &k,
            &mut SnapshotObjects::new(&objects, &b, &mut copy),
            &mut Vec::new(),
            f,
            false,
        )
        .unwrap();
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].vertices.len(), 1);
        assert!(loops[0].edges.is_empty());
        assert!(
            loops[0].vertices[0].vertex
                == Handle::Topology(EntityRef::Vertex(VertexId::from_index(0)))
        );
        let mut cc = CurveClassification::with_curve_interval(
            &k,
            Handle::Source,
            Extent1d::from_min_max(0., 1.),
            1e-6,
        );
        let mut snapshot = b.clone();
        let mut made = Vec::new();
        intersect_ts_bound(
            &k,
            &mut SnapshotObjects::new(&objects, &b, &mut snapshot),
            &mut made,
            &mut cc,
            f,
            false,
        )
        .unwrap();
        assert_eq!(cc.size(), 1);
        let objects = Handles {
            b: &b,

            source: Curve::from(Line::from_points(
                Point3::new(0.2, 0.5, 2.),
                Point3::new(0.8, 0.5, 2.),
                3,
            )),
        };
        let mut copy = b.clone();
        intersect_edges(
            &k,
            &mut SnapshotObjects::new(&objects, &b, &mut copy),
            &mut cc,
            &[EdgeId::from_index(0)],
            f,
            false,
        )
        .unwrap();
        assert_eq!(cc.size(), 1);
    }
    #[test]
    fn cached_boundary_views_resolve_real_edge_and_trim_handles() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = square(&mut b);
        let objects = Handles {
            b: &b,
            source: Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 2)),
        };
        let mut copy = b.clone();
        let loops = boundary_loops(
            &k,
            &mut SnapshotObjects::new(&objects, &b, &mut copy),
            &mut Vec::new(),
            f,
            true,
        )
        .unwrap();
        assert_eq!(loops[0].vertices.len(), 4);
        assert_eq!(loops[0].edges.len(), 4);
        for eu in &loops[0].edges {
            assert_eq!(objects.curve(eu.edge.curve).unwrap().dim(), 3);
            assert_eq!(objects.curve(eu.trim_curve.unwrap()).unwrap().dim(), 2);
        }
    }
}
