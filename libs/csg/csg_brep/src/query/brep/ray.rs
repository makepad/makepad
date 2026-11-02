//! Operation-local face services for shell ray solvers.
//!
//! Upstream: `source/SMLib/src/SmShell.cpp`, `SmFace.cpp`, `SmTrimSrfCache.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`RayPreparation`] | Lazy `GetOrCreateUVTrimCurve` / `smsurf_GetSurfaceCache` |
//! | `RaySurfaceContext` | Owning-face services during a shell ray solve |
//!
//! Deviations: production shell rays and region-nesting B-rep rays use `live`'s
//! single-pass mutable services. The generic read-only fallback requests missing
//! trims or caches, releases its immutable view and restarts after preparation.
//! Each request is attempted once per edge use or face. That fallback can revisit
//! earlier candidates with raised tolerances and has quadratic worst-case work in
//! the number of requests. Cache handles are mapped explicitly; no B-rep copy is kept.

mod live;
pub(crate) use live::{solve_mutable_brep_point, solve_mutable_ray};
pub use live::{
    BrepFaceServices, CurveFaceObject, CurveFaceServices, FaceCaches, LiveBrepObjects, SideObject,
    SidedFaceServices,
};

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use crate::error::{EntityRef, Result, SolidError};
use crate::kernel::Kernel;
use crate::math::Point2;
use crate::query::face::p2::BrepCacheFace;
use crate::surface::surface::p1::{get_surface_cache_with_face, SurfaceSolveContext};
use crate::surface::surface::{CacheTreeState, Surface};
use crate::surface::surface_cache::face_context::{
    FaceQueries, MappedCacheFace, PreparedFaceQueries,
};
use crate::surface::surface_cache::SurfaceCache;
use crate::topology::brep_cache::CacheObject;
use crate::topology::edgeuse::{p1 as eu1, p3 as eu3};
use crate::topology::loopuse::notify_ref;
use crate::topology::{Brep, EdgeuseId, FaceId};

const OP: &str = "query::brep::ray";

/// Geometry requested by a shell ray solve and prepared by its mutable caller.
/// Keeps operation-local caches, not topology. Each edge-use/face request is attempted once.
#[derive(Debug, Default)]
pub struct RayPreparation {
    pending: Cell<Option<PreparationRequest>>,
    /// The B-rep the pending trim request was made on (compared, never dereferenced).
    trim_brep: Cell<usize>,
    attempted: BTreeSet<PreparationRequest>,
    failed_trims: BTreeSet<EdgeuseId>,
    surface_caches: BTreeMap<FaceId, SurfaceCache<CacheObject>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PreparationRequest {
    UvTrim(EdgeuseId),
    SurfaceCache(FaceId),
}

impl RayPreparation {
    pub(crate) fn request_trim(&self, b: &Brep, edgeuse: EdgeuseId) {
        self.trim_brep.set(core::ptr::from_ref(b) as usize);
        self.pending.set(Some(PreparationRequest::UvTrim(edgeuse)));
    }

    /// Whether the last trim request was made on `b` (identity, not equality).
    pub(crate) fn trim_requested_on(&self, b: &Brep) -> bool {
        self.trim_brep.get() == core::ptr::from_ref(b) as usize
    }

    pub(crate) fn trim_failed(&self, edgeuse: EdgeuseId) -> bool {
        self.failed_trims.contains(&edgeuse)
    }

    fn surface_cache(&self, face: FaceId) -> Result<SurfaceCache<CacheObject>> {
        if let Some(cache) = self.surface_caches.get(&face) {
            return cache.map_handles(Ok);
        }
        self.pending
            .set(Some(PreparationRequest::SurfaceCache(face)));
        Err(SolidError::PreparationRequired { operation: OP })
    }

    pub(crate) fn prepared_cache(&self, face: FaceId) -> Option<&SurfaceCache<CacheObject>> {
        self.surface_caches.get(&face)
    }

    pub(crate) fn take_request(&self) -> Option<PreparationRequest> {
        self.pending.take()
    }

    pub(crate) fn prepare(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        request: PreparationRequest,
    ) -> Result<()> {
        if !self.attempted.insert(request) {
            return Err(SolidError::invalid_input(
                OP,
                "ray preparation made no progress",
            ));
        }
        match request {
            PreparationRequest::UvTrim(eu) => {
                let fetch = eu3::get_or_create_uv_trim_curve(k, b, eu, false, false, true)?;
                if fetch.status.is_err() || eu1::uv_trim_curve(b, eu)?.0.is_none() {
                    self.failed_trims.insert(eu);
                }
            }
            PreparationRequest::SurfaceCache(face) => {
                self.surface_caches
                    .insert(face, prepare_surface_cache(k, b, face)?);
            }
        }
        Ok(())
    }
}

fn cache_handle(entity: EntityRef) -> Result<CacheObject> {
    Ok(match entity {
        EntityRef::Face(f) => CacheObject::Face(f),
        EntityRef::Edge(e) => CacheObject::Edge(e),
        EntityRef::Vertex(v) => CacheObject::Vertex(v),
        other @ (EntityRef::Loopuse(_) | EntityRef::Edgeuse(_) | EntityRef::Vertexuse(_)) => {
            CacheObject::Other(notify_ref(other))
        }
        _ => {
            return Err(SolidError::invalid_input(
                OP,
                "unsupported trim-cache entity",
            ))
        }
    })
}

fn prepare_surface_cache(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
) -> Result<SurfaceCache<CacheObject>> {
    face_surface_cache(k, b, face, true)
}

/// `smsurf_GetSurfaceCache` of `face`'s surface: the trimmed cache its factory makes
/// (`CacheMakeOrValidate`), with its tree then built when `get_tree` (a solve's
/// `GetTree`, which creates the face's UV trim curves and implants its boundary).
/// Without `get_tree`, a cache the factory leaves without tree (made during a Boolean
/// operation, or reused unbuilt: [`Surface::cache_tree_state`]) keeps the state bits
/// the `SmTrimSrfCache` constructor set, as upstream's until its first `GetTree`; a
/// surface whose cache upstream has built is built here as well.
fn face_surface_cache(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    get_tree: bool,
) -> Result<SurfaceCache<CacheObject>> {
    let surface = b
        .face(face)?
        .surface
        .as_ref()
        .ok_or(SolidError::null_pointer(OP))?
        .clone_for_evaluation()?;
    // upstream reuses a cache whose tree is built, with the built tree's state bits
    let get_tree = get_tree || surface.cache_tree_state() == CacheTreeState::Built;
    let entities: BTreeMap<_, _> = b
        .loopuses()
        .iter()
        .map(|(id, _)| EntityRef::Loopuse(id))
        .chain(b.edgeuses().iter().map(|(id, _)| EntityRef::Edgeuse(id)))
        .chain(
            b.vertexuses()
                .iter()
                .map(|(id, _)| EntityRef::Vertexuse(id)),
        )
        .map(|entity| (notify_ref(entity), entity))
        .collect();
    let entity_of = |handle| match handle {
        CacheObject::Face(f) => Ok(EntityRef::Face(f)),
        CacheObject::Edge(e) => Ok(EntityRef::Edge(e)),
        CacheObject::Vertex(v) => Ok(EntityRef::Vertex(v)),
        CacheObject::Other(key) => entities
            .get(&key)
            .copied()
            .ok_or_else(|| SolidError::invalid_input(OP, "unknown cache topology handle")),
        _ => Err(SolidError::invalid_input(
            OP,
            "unexpected cache geometry handle",
        )),
    };
    let mut mapped = MappedCacheFace::new(BrepCacheFace { b, f: face }, &cache_handle, &entity_of)?;
    let mut cache =
        get_surface_cache_with_face(k, &surface, Some(CacheObject::Surface(face)), &mut mapped)?;
    // A Boolean operation can defer the factory's tree build until GetTree. A plain
    // SmSurfaceCache (the factory's fallback) runs its own GetTree: no face boundary.
    if get_tree {
        cache.get_tree_with_face(k, &mut mapped)?;
    }
    Ok(cache.into_base())
}

pub(crate) struct RaySurfaceContext<'a> {
    pub(crate) brep: &'a Brep,
    pub(crate) preparation: &'a RayPreparation,
}

impl core::fmt::Debug for RaySurfaceContext<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RaySurfaceContext").finish_non_exhaustive()
    }
}

impl RaySurfaceContext<'_> {
    fn classify(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        src_tol: f64,
        fast_drop: bool,
    ) -> Result<(i32, Option<CacheObject>)> {
        let CacheObject::Face(face) = face else {
            return Err(SolidError::invalid_input(OP, "expected face"));
        };
        let mut pc = crate::intersect::point_class::PointClassification::new(src_tol);
        crate::query::face::p2::point_classify_requested(
            k,
            self.brep,
            face,
            uv,
            &mut pc,
            crate::query::face::p2::PointClassifyOptions {
                src_zone_tol_3d: src_tol,
                do_boundary_intersections: fast_drop,
                use_uv_classification: !fast_drop,
                ..Default::default()
            },
            self.preparation,
        )?;
        Ok((
            pc.point_class().code(),
            pc.object().map(cache_handle).transpose()?,
        ))
    }
}

impl SurfaceSolveContext<CacheObject> for RaySurfaceContext<'_> {
    fn build_surface_cache(
        &mut self,
        _: &Kernel,
        surface: &Surface,
        handle: Option<CacheObject>,
    ) -> Result<SurfaceCache<CacheObject>> {
        let Some(CacheObject::Surface(face)) = handle else {
            return Err(SolidError::invalid_input(OP, "expected face surface"));
        };
        let owner = self
            .brep
            .face(face)?
            .surface
            .as_ref()
            .ok_or(SolidError::null_pointer(OP))?
            .face()?;
        if surface.face()? != owner {
            return Err(SolidError::invalid_input(OP, "surface owner mismatch"));
        }
        self.preparation.surface_cache(face)
    }

    fn face_queries(&self) -> Option<&dyn FaceQueries<CacheObject>> {
        Some(self)
    }
}

impl FaceQueries<CacheObject> for RaySurfaceContext<'_> {
    fn face_zone_tol_3d(&self, face: CacheObject) -> Result<f64> {
        let CacheObject::Face(id) = face else {
            return Err(SolidError::invalid_input(OP, "expected face"));
        };
        PreparedFaceQueries::new(self.brep, id, &cache_handle)?.face_zone_tol_3d(face)
    }

    fn face_point_classify(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<CacheObject>)> {
        self.classify(k, face, uv, src_tol, false)
    }

    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<CacheObject>)> {
        self.classify(k, face, uv, src_tol, true)
    }

    fn curve_edge_tolerance(&self, curve: CacheObject) -> Option<Result<f64>> {
        let (CacheObject::Curve(edge) | CacheObject::Edge(edge)) = curve else {
            return None;
        };
        Some(crate::topology::edge::tolerance(self.brep, edge))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_reads_the_prepared_cache_before_vertex_fallback() {
        use crate::geom::tree::{NodeClass, NodeData, NodeId};
        use crate::intersect::point_class::PointClassType;
        let mut k = Kernel::default();
        let mut b = crate::primitive::api_primitives::create_torus(
            &mut k,
            crate::math::Vec3::ZERO,
            5.0,
            1.5,
        )
        .unwrap();
        let face = crate::topology::brep::p1::faces(&b, None)
            .unwrap()
            .into_iter()
            .find(|&f| {
                matches!(
                    b.face(f).unwrap().surface.as_ref().unwrap().kind(),
                    crate::surface::surface::SurfaceKind::Torus(_)
                )
            })
            .unwrap();
        let mut preparation = RayPreparation::default();
        preparation
            .prepare(&k, &mut b, PreparationRequest::SurfaceCache(face))
            .unwrap();
        let cache = preparation.prepared_cache(face).unwrap();
        let (index, _) = cache
            .tree()
            .unwrap()
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, node)| {
                node.child1.is_none()
                    && matches!(node.data,
                NodeData::Surface(aux) if aux.node_class == NodeClass::Inside)
            })
            .unwrap();
        let uv = cache
            .aux(NodeId(index))
            .unwrap()
            .uv_domain
            .evaluate(0.5, 0.5);
        // An oversized source tolerance makes the uncached fallback snap to a
        // vertex. Native cache-first classification returns the interior face.
        let query = RaySurfaceContext {
            brep: &b,
            preparation: &preparation,
        };
        assert_eq!(
            query
                .face_point_classify(&k, CacheObject::Face(face), uv, 100.0)
                .unwrap(),
            (PointClassType::Face.code(), Some(CacheObject::Face(face)))
        );
        preparation
            .surface_caches
            .get_mut(&face)
            .unwrap()
            .face_was_modified = true;
        let query = RaySurfaceContext {
            brep: &b,
            preparation: &preparation,
        };
        assert_eq!(
            query
                .face_point_classify(&k, CacheObject::Face(face), uv, 100.0)
                .unwrap()
                .0,
            PointClassType::Vertex.code()
        );
    }

    /// During a Boolean operation `smsurf_GetSurfaceCache` defers the tree build
    /// (`CacheMakeOrValidate`), so `GlobalPointSolve`'s fast-drop face test reads the
    /// `SmTrimSrfCache` constructor's state bits until a solve's `GetTree` builds the
    /// tree (whose build turns point testing off on a face without UV trim curves:
    /// `blend.fillet_executive_p3` rollover-404).
    #[test]
    fn fast_drop_cache_state_defers_the_tree_during_a_boolean() {
        let mut k = Kernel::default();
        let mut b = crate::primitive::api_primitives::create_box(
            &mut k,
            crate::math::Point3::ZERO,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
        let face = crate::topology::brep::p1::faces(&b, None)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let surface = b
            .face(face)
            .unwrap()
            .surface
            .as_ref()
            .unwrap()
            .clone_for_evaluation()
            .unwrap();
        surface.set_cache_tree_state(CacheTreeState::NoCache);
        k.context.set_doing_boolean(true);
        let handle = Some(CacheObject::Surface(face));
        let mut services = BrepFaceServices::new(&mut b);

        let state = services.surface_cache_state(&k, &surface, handle).unwrap();
        assert!(state.is_trimmed());
        assert!(state.tree().is_none());
        assert!(state.point_test_enabled);
        assert!(!state.have_t_surface_cache);
        assert_eq!(surface.cache_tree_state(), CacheTreeState::Unbuilt);

        // a solve's GetTree builds the kept cache; later readers see the built tree
        let built = services.build_surface_cache(&k, &surface, handle).unwrap();
        assert!(built.tree().is_some());
        assert_eq!(surface.cache_tree_state(), CacheTreeState::Built);
        let state = services.surface_cache_state(&k, &surface, handle).unwrap();
        assert!(state.tree().is_some());
        assert_eq!(state.point_test_enabled, built.point_test_enabled);
    }

    #[test]
    fn surface_cache_requests_are_scoped_and_bounded() {
        let mut k = Kernel::default();
        let mut b = crate::primitive::api_primitives::create_cylinder(
            &mut k,
            crate::math::Point3::ZERO,
            5.0,
            20.0,
        )
        .unwrap();
        let face = crate::topology::brep::p1::faces(&b, None)
            .unwrap()
            .into_iter()
            .find(|f| {
                matches!(
                    b.face(*f).unwrap().surface.as_ref().unwrap().kind(),
                    crate::surface::surface::SurfaceKind::Cone(_)
                )
            })
            .unwrap();
        let mut preparation = RayPreparation::default();
        assert!(matches!(
            preparation.surface_cache(face),
            Err(SolidError::PreparationRequired { .. })
        ));
        let request = PreparationRequest::SurfaceCache(face);
        assert_eq!(preparation.take_request(), Some(request));
        preparation.prepare(&k, &mut b, request).unwrap();
        let cache = preparation.surface_cache(face).unwrap();
        assert!(cache.tree().is_some());
        assert_eq!(cache.face().unwrap(), Some(CacheObject::Face(face)));
        assert_eq!(preparation.take_request(), None);
        assert!(matches!(
            preparation.prepare(&k, &mut b, request),
            Err(SolidError::InvalidInput { .. })
        ));
    }
}
