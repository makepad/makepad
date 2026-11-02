//! Handle mapping and immutable operation-scoped face queries for trim caches.
//!
//! Upstream: `source/SMLib/src/SmTrimSrfCache.cpp`, `source/SMLib/src/SmFace.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`MappedCacheFace`] | `GetFace()->GetBrep()` during `SmTrimSrfCache::BuildTree` |
//! | [`FaceQueries`], [`PreparedFaceQueries`] | `SmFace::PointClassify`, `SmTol::GetZoneTol3d` |
//! | [`prepare_face_surface_cache`] | `smsurf_GetSurfaceCache` of a face's surface, then `GetTree` |
//! | [`prepare_face_surface_cache_state`] | `smsurf_GetSurfaceCache` of a face's surface (no `GetTree`) |
//!
//! Deviations: [`MappedCacheFace`] only translates handles over the shared
//! [`BrepCacheFace`] implementation of [`CacheFace`]; it does not duplicate topology
//! construction. Both mappings are explicit and checked in both directions; object
//! keys are never decoded as arena ids. No topology is retained by a cache.

use core::fmt::{self, Debug};

use crate::error::{EntityRef, Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::intersect::point_class::PointClassification;
use crate::kernel::Kernel;
use crate::math::Point2;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::query::face::p2::{point_classify_prepared, BrepCacheFace, PointClassifyOptions};
use crate::surface::surface::p1::get_surface_cache_with_face;
use crate::surface::surface::CacheTreeState;
use crate::surface::surface_cache::p1::{FaceContainment, SurfaceCache};
use crate::surface::trim_srf_cache::CacheFace;
use crate::topology::{Brep, FaceId};

/// Face operations needed by a trimmed surface cache.
pub trait FaceQueries<O>: Debug {
    /// Returns the face's 3D zone tolerance.
    fn face_zone_tol_3d(&self, face: O) -> Result<f64>;

    /// Classifies a UV point against a face, returning its class and optional object.
    fn face_point_classify(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<O>)>;

    /// `GlobalPointSolve`'s fast-drop classification uses boundary intersections
    /// on and forced UV classification off, unlike the trim-cache override.
    /// Adapters must explicitly support these flags rather than dropping them.
    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<O>)>;

    /// `SmEdge::GetTolerance` of the edge owning the curve behind `curve` (upstream's
    /// `crCurve.GetOwner()`), for the curve/surface solves of a face's surface.
    /// `None` (the default) when the adapter cannot resolve the curve's edge.
    fn curve_edge_tolerance(&self, curve: O) -> Option<Result<f64>> {
        let _ = curve;
        None
    }
}

/// The shared B-rep cache-face adapter in a caller's handle space.
///
/// The mappings must be inverse for every entity used by the cache. The adapter
/// checks that contract on every translation, rejecting aliases instead of
/// decoding object keys or casting handles to typed ids.
pub struct MappedCacheFace<'a, O> {
    inner: BrepCacheFace<'a>,
    face_handle: O,
    handle_of: &'a dyn Fn(EntityRef) -> Result<O>,
    entity_of: &'a dyn Fn(O) -> Result<EntityRef>,
}

impl<O> Debug for MappedCacheFace<'_, O> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MappedCacheFace")
            .field("face", &self.inner.f)
            .finish_non_exhaustive()
    }
}

impl<'a, O: Copy + PartialEq> MappedCacheFace<'a, O> {
    /// Wraps the shared adapter with explicit forward and reverse handle mappings.
    ///
    /// # Errors
    ///
    /// An invalid face, a failing mapping, or mappings that are not inverse.
    pub fn new(
        inner: BrepCacheFace<'a>,
        handle_of: &'a dyn Fn(EntityRef) -> Result<O>,
        entity_of: &'a dyn Fn(O) -> Result<EntityRef>,
    ) -> Result<Self> {
        inner.b.face(inner.f)?;
        let entity = inner.face();
        let face_handle = handle_of(entity)?;
        if entity_of(face_handle)? != entity {
            return Err(Self::mapping_error());
        }
        Ok(Self {
            inner,
            face_handle,
            handle_of,
            entity_of,
        })
    }

    fn mapping_error() -> SolidError {
        SolidError::invalid_input(
            "surface::surface_cache::face_context::MappedCacheFace",
            "handle and entity mappings are not inverse",
        )
    }

    fn handle(&self, entity: EntityRef) -> Result<O> {
        let handle = (self.handle_of)(entity)?;
        if (self.entity_of)(handle)? != entity {
            return Err(Self::mapping_error());
        }
        Ok(handle)
    }

    fn entity(&self, handle: O) -> Result<EntityRef> {
        let entity = (self.entity_of)(handle)?;
        if (self.handle_of)(entity)? != handle {
            return Err(Self::mapping_error());
        }
        Ok(entity)
    }

    fn handles(&self, entities: Vec<EntityRef>) -> Result<Vec<O>> {
        entities.into_iter().map(|e| self.handle(e)).collect()
    }
}

impl<O: Copy + PartialEq> CacheFace<O> for MappedCacheFace<'_, O> {
    fn face(&self) -> O {
        self.face_handle
    }

    fn brep_editing_enabled(&self) -> Result<bool> {
        self.inner.brep_editing_enabled()
    }

    fn set_brep_editing_enabled(&mut self, enabled: bool) -> Result<()> {
        self.inner.set_brep_editing_enabled(enabled)
    }

    fn create_uv_trim_curves(&mut self, k: &Kernel) -> Result<()> {
        self.inner.create_uv_trim_curves(k)
    }

    fn upward_loopuses(&self) -> Result<Vec<O>> {
        self.handles(self.inner.upward_loopuses()?)
    }

    fn loopuse_edgeuses(&self, loopuse: O) -> Result<Vec<O>> {
        self.handles(self.inner.loopuse_edgeuses(self.entity(loopuse)?)?)
    }

    fn loopuse_vertexuses(&self, loopuse: O) -> Result<Vec<O>> {
        self.handles(self.inner.loopuse_vertexuses(self.entity(loopuse)?)?)
    }

    fn edgeuse_uv_trim_curve(&self, edgeuse: O) -> Result<Option<BSplineCurve>> {
        self.inner.edgeuse_uv_trim_curve(self.entity(edgeuse)?)
    }

    fn vertexuse_uv_point(&mut self, k: &Kernel, vertexuse: O) -> Result<Point2> {
        let entity = self.entity(vertexuse)?;
        self.inner.vertexuse_uv_point(k, entity)
    }

    fn vertexuse_vertex_tolerance(&self, vertexuse: O) -> Result<f64> {
        self.inner
            .vertexuse_vertex_tolerance(self.entity(vertexuse)?)
    }

    fn zone_tol_3d(&self, k: &Kernel, object: O) -> Result<f64> {
        self.inner.zone_tol_3d(k, self.entity(object)?)
    }

    fn natural_uv_domain(&self) -> Result<Option<Extent2d>> {
        self.inner.natural_uv_domain()
    }

    fn point_classify(
        &mut self,
        k: &Kernel,
        uv_point: Point2,
        src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<O>)> {
        let (class, entity) = self.inner.point_classify(k, uv_point, src_zone_tol_3d)?;
        Ok((class, entity.map(|e| self.handle(e)).transpose()?))
    }
}

/// Read-only face queries after UV trim preparation has finished.
///
/// This adapter borrows the real topology, not a B-rep copy. Its classifier cannot
/// create trims or re-enter construction of the cache being classified.
pub struct PreparedFaceQueries<'a, O> {
    brep: &'a Brep,
    face: FaceId,
    face_handle: O,
    handle_of: &'a dyn Fn(EntityRef) -> Result<O>,
}

impl<O> Debug for PreparedFaceQueries<'_, O> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedFaceQueries")
            .field("face", &self.face)
            .finish_non_exhaustive()
    }
}

impl<'a, O: Copy + PartialEq> PreparedFaceQueries<'a, O> {
    /// Borrows one prepared face and the caller's typed-entity handle mapping.
    ///
    /// # Errors
    ///
    /// Those of looking up the face and mapping its handle. Missing trim geometry
    /// is reported by classification rather than synthesized by this adapter.
    pub fn new(
        brep: &'a Brep,
        face: FaceId,
        handle_of: &'a dyn Fn(EntityRef) -> Result<O>,
    ) -> Result<Self> {
        brep.face(face)?;
        Ok(Self {
            brep,
            face,
            face_handle: handle_of(EntityRef::Face(face))?,
            handle_of,
        })
    }

    fn check_face(&self, face: O) -> Result<()> {
        if face == self.face_handle {
            Ok(())
        } else {
            Err(SolidError::invalid_input(
                "surface::surface_cache::face_context::PreparedFaceQueries",
                "query handle does not name the prepared face",
            ))
        }
    }

    fn classify(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_tol: f64,
        do_boundary_intersections: bool,
        use_uv_classification: bool,
    ) -> Result<(i32, Option<O>)> {
        self.check_face(face)?;
        let mut pc = PointClassification::new(src_tol);
        point_classify_prepared(
            k,
            self.brep,
            self.face,
            uv,
            &mut pc,
            PointClassifyOptions {
                src_zone_tol_3d: src_tol,
                do_boundary_intersections,
                use_uv_classification,
                ..PointClassifyOptions::default()
            },
        )?;
        Ok((
            pc.point_class().code(),
            pc.object().map(self.handle_of).transpose()?,
        ))
    }
}

impl<O: Copy + PartialEq> FaceQueries<O> for PreparedFaceQueries<'_, O> {
    fn face_zone_tol_3d(&self, face: O) -> Result<f64> {
        self.check_face(face)?;
        crate::topology::face::p2::tolerance(self.brep, self.face)
    }

    fn face_point_classify(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<O>)> {
        // The classification object's tolerance is read by vertex/loop tests;
        // setting only the options' source tolerance is not equivalent.
        self.classify(k, face, uv, src_tol, false, true)
    }

    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<O>)> {
        self.classify(k, face, uv, src_tol, true, false)
    }
}

/// `smsurf_GetSurfaceCache` of the surface of face `face` of `b` followed by the
/// solve's `GetTree`: the face surface's trimmed cache built through the face (which
/// may create the face's UV trim curves on `b`), with its tree ready, in the caller's
/// handle space. `surface_handle` names the face's surface; `handle_of` and
/// `entity_of` are the caller's inverse mappings of `b`'s entities (faces, edges,
/// vertices, loop uses, edge uses and vertex uses), as for [`MappedCacheFace`].
///
/// # Errors
///
/// Those of the face lookups, the mappings and the cache build.
pub fn prepare_face_surface_cache<O: Copy + PartialEq>(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    surface_handle: O,
    handle_of: &dyn Fn(EntityRef) -> Result<O>,
    entity_of: &dyn Fn(O) -> Result<EntityRef>,
) -> Result<SurfaceCache<O>> {
    face_surface_cache(k, b, face, surface_handle, handle_of, entity_of, true)
}

/// [`prepare_face_surface_cache`] without the solve's `GetTree`: the cache as
/// `smsurf_GetSurfaceCache` hands it to a reader that reads only its state bits
/// (`GlobalPointSolve`'s fast-drop face test). A cache the factory leaves without
/// tree (made during a Boolean operation, or reused unbuilt:
/// [`crate::surface::surface::Surface::cache_tree_state`]) keeps the state bits the `SmTrimSrfCache`
/// constructor set; a surface whose cache upstream has built is built here as well.
///
/// # Errors
///
/// As [`prepare_face_surface_cache`].
pub fn prepare_face_surface_cache_state<O: Copy + PartialEq>(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    surface_handle: O,
    handle_of: &dyn Fn(EntityRef) -> Result<O>,
    entity_of: &dyn Fn(O) -> Result<EntityRef>,
) -> Result<SurfaceCache<O>> {
    face_surface_cache(k, b, face, surface_handle, handle_of, entity_of, false)
}

fn face_surface_cache<O: Copy + PartialEq>(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    surface_handle: O,
    handle_of: &dyn Fn(EntityRef) -> Result<O>,
    entity_of: &dyn Fn(O) -> Result<EntityRef>,
    get_tree: bool,
) -> Result<SurfaceCache<O>> {
    const OP: &str = "surface::surface_cache::face_context::prepare_face_surface_cache";
    // the cache refers to the face's surface (an exact evaluation copy: the face's
    // B-rep is changed through the face while the cache is built)
    let surface = b
        .face(face)?
        .surface
        .as_ref()
        .ok_or(SolidError::null_pointer(OP))?
        .clone_for_evaluation()?;
    // upstream reuses a cache whose tree is built, with the built tree's state bits
    let get_tree = get_tree || surface.cache_tree_state() == CacheTreeState::Built;
    let mut mapped = MappedCacheFace::new(BrepCacheFace { b, f: face }, handle_of, entity_of)?;
    let mut cache = get_surface_cache_with_face(k, &surface, Some(surface_handle), &mut mapped)?;
    // A Boolean operation can defer the factory's tree build until GetTree.
    if get_tree
        && (cache.base().tree().is_none()
            || (!cache.base().have_t_surface_cache
                && surface.face()?.is_some()
                && cache.base().face_containment_done != FaceContainment::UnderConstruction))
    {
        cache.build_tree_with_face(k, &mut mapped)?;
    }
    Ok(cache.into_base())
}
