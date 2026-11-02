//! Ray solves with live topology and immutable evaluation geometry.
//!
//! Upstream: `source/SMLib/src/SmShell.cpp`, `SmTopologySolver.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `solve_mutable_ray` | `SmShell::RayFire`'s `ShapePointSolve` |
//! | `solve_mutable_brep_point` | `SmBrep::RayFire`'s `BrepPointSolve` |
//! | [`LiveBrepObjects`] | Geometry pointers and live topology reads during the solve |
//! | [`BrepFaceServices`], [`FaceCaches`], [`SidedFaceServices`], [`SideObject`] | `smsurf_GetSurfaceCache` / `SmFace::PointClassify` of face surfaces on mutable B-reps |
//! | [`CurveFaceServices`], [`CurveFaceObject`] | the same for a free curve's solve against a face's surface |
//!
//! Deviations: exact evaluation copies satisfy the solver's borrowed-geometry API.
//! A checked, operation-local `RefCell` lends the original B-rep to lazy face queries;
//! topology remains in its typed arenas. No B-rep copy or restarted solve is used.

use std::cell::RefCell;
use std::collections::BTreeMap;

use super::{cache_handle, face_surface_cache, prepare_surface_cache};
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::object::Shape;
use crate::geom::object::ShapeMember;
use crate::geom::solution_array::SolutionArray;
use crate::intersect::point_class::PointClassification;
use crate::intersect::topology_solver::p1::{TopologyObjects, TopologySolver};
use crate::intersect::topology_solver::p2::TopologySolveOptions;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3};
use crate::query::brep::BrepIdentity;
use crate::query::face::p1::Point3dClassifyOptions;
use crate::query::face::p2::{point_classify_live, PointClassifyOptions};
use crate::surface::surface::p1::SurfaceSolveContext;
use crate::surface::surface::Surface;
use crate::surface::surface_cache::face_context::FaceQueries;
use crate::surface::surface_cache::SurfaceCache;
use crate::topology::brep_cache::{BrepCache, BrepObjects, CacheObject};
use crate::topology::{Brep, EdgeId, FaceId};

const OP: &str = "query::brep::ray::live";
fn borrow_conflict(_: impl core::fmt::Debug) -> SolidError {
    SolidError::invalid_input(OP, "recursive topology borrow during ray solve")
}

pub(crate) fn solve_mutable_ray(
    k: &Kernel,
    brep: &mut Brep,
    identity: &BrepIdentity<'_>,
    shape: &Shape,
    point: Point3,
    options: TopologySolveOptions<'_>,
) -> Result<SolutionArray<CacheObject>> {
    let objects = LiveBrepObjects::new(brep, identity, None)?;
    TopologySolver::shape_point_solve(k, &objects, shape, point, options)
}

pub(crate) fn solve_mutable_brep_point(
    k: &Kernel,
    brep: &mut Brep,
    identity: &BrepIdentity<'_>,
    cache: &BrepCache,
    point: Point3,
    options: TopologySolveOptions<'_>,
) -> Result<SolutionArray<CacheObject>> {
    let objects = LiveBrepObjects::new(brep, identity, Some(cache))?;
    TopologySolver::brep_point_solve(k, &objects, Some(CacheObject::Brep), point, options)
}

impl<'a, 'i> LiveBrepObjects<'a, 'i> {
    /// The topology solver's view of `brep` for one solve: the geometry as exact
    /// evaluation copies, the topology read live, and the face caches and
    /// classifications the solve requests made on `brep` itself (upstream's
    /// `smsurf_GetSurfaceCache` and `SmFace::PointClassify`, which create UV trim
    /// curves). `cache` is the B-rep's cache for `BrepPointSolve`, `None` for shape
    /// solves.
    ///
    /// # Errors
    ///
    /// Those of the evaluation copies.
    pub fn new(
        brep: &'a mut Brep,
        identity: &'a BrepIdentity<'i>,
        cache: Option<&'a BrepCache>,
    ) -> Result<Self> {
        let curves = brep
            .edges()
            .iter()
            .filter_map(|(id, edge)| {
                edge.curve
                    .as_ref()
                    .map(|curve| curve.clone_for_evaluation().map(|c| (id, c)))
            })
            .collect::<Result<_>>()?;
        let surfaces = brep
            .faces()
            .iter()
            .filter_map(|(id, face)| {
                face.surface
                    .as_ref()
                    .map(|surface| surface.clone_for_evaluation().map(|s| (id, s)))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            faces: BrepFaceServices::new(brep),
            identity,
            cache,
            curves,
            surfaces,
        })
    }
}

/// Live topology objects for the topology solver over one mutable B-rep (see
/// [`LiveBrepObjects::new`]).
pub struct LiveBrepObjects<'a, 'i> {
    faces: BrepFaceServices<'a>,
    identity: &'a BrepIdentity<'i>,
    cache: Option<&'a BrepCache>,
    curves: BTreeMap<EdgeId, Curve>,
    surfaces: BTreeMap<FaceId, Surface>,
}

impl LiveBrepObjects<'_, '_> {
    /// Runs `f` on the B-rep between solves (what a caller reads from it after one).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when called from inside a solve that holds the B-rep mutably;
    /// those of `f`.
    pub fn with_brep<T>(&self, f: impl FnOnce(&Brep) -> Result<T>) -> Result<T> {
        self.faces.with_brep(f)
    }

    fn read<T>(&self, f: impl FnOnce(&BrepObjects<'_>) -> Result<T>) -> Result<T> {
        let brep = self.faces.brep.try_borrow().map_err(borrow_conflict)?;
        f(&BrepObjects {
            preparation: None,
            brep: &brep,
            brep_id: self.identity.brep_id,
            cache: self.cache,
            keys: self.identity.keys,
            owner_of: self.identity.owner_of,
        })
    }

    fn cache(&self, k: &Kernel, face: FaceId) -> Result<SurfaceCache<CacheObject>> {
        self.faces.cache(k, face)
    }

    #[allow(clippy::too_many_arguments)]
    fn classify(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        src_tol: f64,
        do_boundaries: bool,
        use_uv: bool,
        ignore_native_status: bool,
    ) -> Result<(i32, Option<CacheObject>)> {
        self.faces.classify(
            k,
            face,
            uv,
            src_tol,
            do_boundaries,
            use_uv,
            ignore_native_status,
        )
    }
}

impl TopologyObjects<CacheObject> for LiveBrepObjects<'_, '_> {
    fn brep_cache(&self, brep: CacheObject) -> Result<&BrepCache> {
        if brep != CacheObject::Brep {
            return Err(SolidError::invalid_input(OP, "expected B-rep handle"));
        }
        self.cache
            .ok_or_else(|| SolidError::invalid_input(OP, "ray solve has no B-rep cache"))
    }
    fn surface_solve_context(&self) -> Option<Box<dyn SurfaceSolveContext<CacheObject> + '_>> {
        Some(Box::new(LiveSurfaceContext(self)))
    }
    fn classify_face_point(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        tolerance: f64,
        boundaries: bool,
        use_uv: bool,
    ) -> Option<Result<i32>> {
        Some(
            self.classify(k, face, uv, tolerance, boundaries, use_uv, true)
                .map(|r| r.0),
        )
    }
    fn classifies_on_face(
        &self,
        k: &Kernel,
        face: CacheObject,
        point: Point3,
        tolerance: f64,
    ) -> Option<Result<bool>> {
        Some((|| {
            let CacheObject::Face(face) = face else {
                return Err(SolidError::invalid_input(OP, "expected face handle"));
            };
            let mut brep = self.faces.brep.try_borrow_mut().map_err(borrow_conflict)?;
            let mut pc = PointClassification::new(tolerance);
            for tol in [tolerance, tolerance * 100.0] {
                match crate::query::face::p1::point_3d_classify(
                    k,
                    &mut brep,
                    face,
                    point,
                    tol,
                    true,
                    &mut pc,
                    Point3dClassifyOptions::default(),
                ) {
                    Ok(()) => {
                        return Ok(
                            pc.point_class() == crate::intersect::point_class::PointClassType::Face
                        )
                    }
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => {}
                }
            }
            Ok(false)
        })())
    }
    fn curve(&self, object: CacheObject) -> Result<&Curve> {
        let CacheObject::Curve(edge) = object else {
            return Err(SolidError::invalid_input(OP, "expected curve handle"));
        };
        self.curves.get(&edge).ok_or(SolidError::null_pointer(OP))
    }
    fn surface(&self, object: CacheObject) -> Result<&Surface> {
        let CacheObject::Surface(face) = object else {
            return Err(SolidError::invalid_input(OP, "expected surface handle"));
        };
        self.surfaces.get(&face).ok_or(SolidError::null_pointer(OP))
    }
    fn analytic_curve_handle(&self, curve: CacheObject) -> Option<CacheObject> {
        Some(curve)
    }
    fn vertex_point(&self, vertex: CacheObject) -> Result<Point3> {
        self.read(|b| b.vertex_point(vertex))
    }
    fn vertex_tolerance(&self, vertex: CacheObject) -> Result<f64> {
        self.read(|b| b.vertex_tolerance(vertex))
    }
    fn curve_edge(&self, curve: CacheObject) -> Result<Option<CacheObject>> {
        self.read(|b| b.curve_edge(curve))
    }
    fn edge_curve(&self, edge: CacheObject) -> Result<Option<CacheObject>> {
        self.read(|b| b.edge_curve(edge))
    }
    fn edge_interval(&self, edge: CacheObject) -> Result<Extent1d> {
        self.read(|b| b.edge_interval(edge))
    }
    fn edge_tolerance(&self, edge: CacheObject) -> Result<f64> {
        self.read(|b| b.edge_tolerance(edge))
    }
    fn surface_face(&self, surface: CacheObject) -> Result<Option<CacheObject>> {
        self.read(|b| b.surface_face(surface))
    }
    fn face_surface(&self, face: CacheObject) -> Result<CacheObject> {
        self.read(|b| b.face_surface(face))
    }
    fn face_uv_domain(&self, face: CacheObject) -> Result<Extent2d> {
        self.read(|b| b.face_uv_domain(face))
    }
    fn face_tolerance(&self, face: CacheObject) -> Result<f64> {
        self.read(|b| b.face_tolerance(face))
    }
    fn surface_owner(&self, surface: CacheObject) -> Result<Option<CacheObject>> {
        self.read(|b| b.surface_owner(surface))
    }
    fn object_zone_tol_3d(&self, k: &Kernel, object: CacheObject) -> Result<f64> {
        self.read(|b| b.object_zone_tol_3d(k, object))
    }
    fn shape_member_object(&self, member: ShapeMember) -> Result<CacheObject> {
        self.read(|b| b.shape_member_object(member))
    }
    fn shape_member(&self, object: CacheObject) -> ShapeMember {
        let brep = self.identity.brep_id;
        match object {
            CacheObject::Vertex(v) => ShapeMember::Vertex((brep, v)),
            CacheObject::Edge(e) => ShapeMember::Edge((brep, e)),
            CacheObject::Face(f) => ShapeMember::Face((brep, f)),
            _ => ShapeMember::Other,
        }
    }
}

struct LiveSurfaceContext<'a, 'b, 'i>(&'a LiveBrepObjects<'b, 'i>);
impl core::fmt::Debug for LiveSurfaceContext<'_, '_, '_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LiveSurfaceContext").finish_non_exhaustive()
    }
}
impl SurfaceSolveContext<CacheObject> for LiveSurfaceContext<'_, '_, '_> {
    fn build_surface_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<CacheObject>,
    ) -> Result<SurfaceCache<CacheObject>> {
        let face = match handle {
            Some(CacheObject::Surface(face)) => face,
            // a surface outside the B-rep (no handle): its own cache, as a solve
            // without context builds it
            None => return crate::surface::surface::p2::surface_cache(k, surface, None),
            Some(_) => return Err(SolidError::invalid_input(OP, "expected surface handle")),
        };
        self.0.cache(k, face)
    }
    fn surface_cache_state(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<CacheObject>,
    ) -> Result<SurfaceCache<CacheObject>> {
        let face = match handle {
            Some(CacheObject::Surface(face)) => face,
            None => return crate::surface::surface::p2::surface_cache(k, surface, None),
            Some(_) => return Err(SolidError::invalid_input(OP, "expected surface handle")),
        };
        self.0.faces.cache_state_mapped(k, face, Ok)
    }
    fn face_queries(&self) -> Option<&dyn FaceQueries<CacheObject>> {
        Some(self)
    }
}
impl FaceQueries<CacheObject> for LiveSurfaceContext<'_, '_, '_> {
    fn face_zone_tol_3d(&self, face: CacheObject) -> Result<f64> {
        self.0.face_tolerance(face)
    }
    fn face_point_classify(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<(i32, Option<CacheObject>)> {
        self.0.classify(k, face, uv, tolerance, false, true, false)
    }
    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<(i32, Option<CacheObject>)> {
        self.0.classify(k, face, uv, tolerance, true, false, false)
    }
    fn curve_edge_tolerance(&self, curve: CacheObject) -> Option<Result<f64>> {
        match self.0.curve_edge(curve) {
            Ok(Some(edge)) => Some(self.0.edge_tolerance(edge)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

/// A mutable B-rep's owning-face services for surface solves (upstream's
/// `smsurf_GetSurfaceCache` and `SmFace::PointClassify` on a face's surface): the
/// trimmed cache of a face's surface is made on the B-rep (creating its UV trim
/// curves) the first time a solve asks for it and kept for the operation, and the
/// face classifications run on the B-rep with it. Handles are
/// [`CacheObject::Surface`] of a face; a surface without handle gets its own cache.
pub struct BrepFaceServices<'a> {
    brep: RefCell<&'a mut Brep>,
    caches: RefCell<FaceCaches>,
}

/// The trimmed surface caches a [`BrepFaceServices`] made, by face. A caller that
/// gives several services the same unchanged B-rep keeps them from one to the next
/// ([`BrepFaceServices::with_caches`], [`BrepFaceServices::into_caches`]), as
/// upstream keeps each cache on its surface until the face changes.
pub type FaceCaches = BTreeMap<FaceId, SurfaceCache<CacheObject>>;

impl core::fmt::Debug for BrepFaceServices<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BrepFaceServices").finish_non_exhaustive()
    }
}

impl<'a> BrepFaceServices<'a> {
    /// The services of `brep` (no cache is made yet).
    #[must_use]
    pub fn new(brep: &'a mut Brep) -> Self {
        Self::with_caches(brep, FaceCaches::new())
    }

    /// The services of `brep` with the face caches an earlier service of the same
    /// B-rep made (see [`FaceCaches`]); the B-rep must not have changed since.
    #[must_use]
    pub fn with_caches(brep: &'a mut Brep, caches: FaceCaches) -> Self {
        Self {
            brep: RefCell::new(brep),
            caches: RefCell::new(caches),
        }
    }

    /// The face caches made so far, for [`BrepFaceServices::with_caches`].
    #[must_use]
    pub fn into_caches(self) -> FaceCaches {
        self.caches.into_inner()
    }

    /// Runs `f` on the B-rep between solves.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when called from inside a solve that holds the B-rep mutably;
    /// those of `f`.
    pub fn with_brep<T>(&self, f: impl FnOnce(&Brep) -> Result<T>) -> Result<T> {
        let brep = self.brep.try_borrow().map_err(borrow_conflict)?;
        f(&brep)
    }

    fn cache(&self, k: &Kernel, face: FaceId) -> Result<SurfaceCache<CacheObject>> {
        self.cache_mapped(k, face, Ok)
    }

    /// [`BrepFaceServices::cache`] with its handles mapped by `map`: one copy of the
    /// kept cache, made directly in the caller's handle space.
    fn cache_mapped<P: Copy + PartialEq>(
        &self,
        k: &Kernel,
        face: FaceId,
        map: impl Fn(CacheObject) -> Result<P>,
    ) -> Result<SurfaceCache<P>> {
        // (a kept cache without tree, made for `cache_state_mapped`, is made again
        // and built: its first `GetTree`)
        if let Some(cache) = self
            .caches
            .try_borrow()
            .map_err(borrow_conflict)?
            .get(&face)
            .filter(|cache| cache.tree().is_some())
        {
            return cache.map_handles(map);
        }
        let cache = prepare_surface_cache(
            k,
            &mut **self.brep.try_borrow_mut().map_err(borrow_conflict)?,
            face,
        )?;
        let result = cache.map_handles(map)?;
        self.caches
            .try_borrow_mut()
            .map_err(borrow_conflict)?
            .insert(face, cache);
        Ok(result)
    }

    /// The face's cache as `smsurf_GetSurfaceCache` hands it out without a `GetTree`
    /// (the state bits `GlobalPointSolve`'s fast-drop face test reads): the kept cache,
    /// else a new one kept without the tree its factory defers
    /// (`face_surface_cache`), until a solve's [`BrepFaceServices::cache_mapped`]
    /// builds it.
    fn cache_state_mapped<P: Copy + PartialEq>(
        &self,
        k: &Kernel,
        face: FaceId,
        map: impl Fn(CacheObject) -> Result<P>,
    ) -> Result<SurfaceCache<P>> {
        if let Some(cache) = self
            .caches
            .try_borrow()
            .map_err(borrow_conflict)?
            .get(&face)
        {
            return cache.map_handles(map);
        }
        let cache = face_surface_cache(
            k,
            &mut **self.brep.try_borrow_mut().map_err(borrow_conflict)?,
            face,
            false,
        )?;
        let result = cache.map_handles(map)?;
        self.caches
            .try_borrow_mut()
            .map_err(borrow_conflict)?
            .insert(face, cache);
        Ok(result)
    }

    fn classify(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        src_tol: f64,
        do_boundaries: bool,
        use_uv: bool,
        ignore_native_status: bool,
    ) -> Result<(i32, Option<CacheObject>)> {
        let CacheObject::Face(face) = face else {
            return Err(SolidError::invalid_input(OP, "expected face handle"));
        };
        let mut pc = PointClassification::new(src_tol);
        let caches = self.caches.try_borrow().map_err(borrow_conflict)?;
        let status = point_classify_live(
            k,
            &mut **self.brep.try_borrow_mut().map_err(borrow_conflict)?,
            face,
            uv,
            &mut pc,
            PointClassifyOptions {
                src_zone_tol_3d: src_tol,
                do_boundary_intersections: do_boundaries,
                use_uv_classification: use_uv,
                ..Default::default()
            },
            caches.get(&face),
        );
        match status {
            Err(
                e @ (SolidError::Unported { .. }
                | SolidError::InvalidInput { .. }
                | SolidError::PortInvariant { .. }
                | SolidError::PreparationRequired { .. }),
            ) => return Err(e),
            Err(_) if ignore_native_status => {}
            status => status?,
        }
        Ok((
            pc.point_class().code(),
            pc.object().map(cache_handle).transpose()?,
        ))
    }
}

impl SurfaceSolveContext<CacheObject> for BrepFaceServices<'_> {
    fn build_surface_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<CacheObject>,
    ) -> Result<SurfaceCache<CacheObject>> {
        match handle {
            Some(CacheObject::Surface(face)) => self.cache(k, face),
            // a surface outside the B-rep: its own cache, as a solve without context
            None => crate::surface::surface::p2::surface_cache(k, surface, None),
            Some(_) => Err(SolidError::invalid_input(OP, "expected surface handle")),
        }
    }
    fn surface_cache_state(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<CacheObject>,
    ) -> Result<SurfaceCache<CacheObject>> {
        match handle {
            Some(CacheObject::Surface(face)) => self.cache_state_mapped(k, face, Ok),
            None => crate::surface::surface::p2::surface_cache(k, surface, None),
            Some(_) => Err(SolidError::invalid_input(OP, "expected surface handle")),
        }
    }
    fn face_queries(&self) -> Option<&dyn FaceQueries<CacheObject>> {
        Some(self)
    }
}

impl FaceQueries<CacheObject> for BrepFaceServices<'_> {
    fn face_zone_tol_3d(&self, face: CacheObject) -> Result<f64> {
        let CacheObject::Face(id) = face else {
            return Err(SolidError::invalid_input(OP, "expected face handle"));
        };
        Ok(self
            .brep
            .try_borrow()
            .map_err(borrow_conflict)?
            .face(id)?
            .tolerance)
    }
    fn face_point_classify(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<(i32, Option<CacheObject>)> {
        self.classify(k, face, uv, tolerance, false, true, false)
    }
    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: CacheObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<(i32, Option<CacheObject>)> {
        self.classify(k, face, uv, tolerance, true, false, false)
    }
    fn curve_edge_tolerance(&self, curve: CacheObject) -> Option<Result<f64>> {
        let (CacheObject::Curve(edge) | CacheObject::Edge(edge)) = curve else {
            return None;
        };
        Some(
            self.brep
                .try_borrow()
                .map_err(borrow_conflict)
                .and_then(|brep| crate::topology::edge::tolerance(&brep, edge)),
        )
    }
}

/// An object of one of several B-reps: the index of its B-rep and its handle there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideObject(pub usize, pub CacheObject);

impl crate::geom::solution_array::SolutionObject for SideObject {
    fn kind(&self) -> crate::geom::solution_array::ObjectKind {
        self.1.kind()
    }
}

/// The owning-face services of several mutable B-reps, on [`SideObject`] handles
/// (B-rep `i` is `self.0[i]`).
#[derive(Debug)]
pub struct SidedFaceServices<'a>(pub Vec<BrepFaceServices<'a>>);

impl<'a> SidedFaceServices<'a> {
    fn side(&self, o: SideObject) -> Result<&BrepFaceServices<'a>> {
        self.0
            .get(o.0)
            .ok_or_else(|| SolidError::invalid_input(OP, "handle of an absent B-rep"))
    }
}

impl SurfaceSolveContext<SideObject> for SidedFaceServices<'_> {
    fn build_surface_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<SideObject>,
    ) -> Result<SurfaceCache<SideObject>> {
        let Some(SideObject(i, c)) = handle else {
            return crate::surface::surface::p2::surface_cache(k, surface, None);
        };
        let services = self
            .0
            .get(i)
            .ok_or_else(|| SolidError::invalid_input(OP, "handle of an absent B-rep"))?;
        // (as `BrepFaceServices::build_surface_cache`, mapped to this side in the one
        // copy the fetch makes)
        match c {
            CacheObject::Surface(face) => services.cache_mapped(k, face, |c| Ok(SideObject(i, c))),
            _ => Err(SolidError::invalid_input(OP, "expected surface handle")),
        }
    }
    fn surface_cache_state(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<SideObject>,
    ) -> Result<SurfaceCache<SideObject>> {
        let Some(SideObject(i, c)) = handle else {
            return crate::surface::surface::p2::surface_cache(k, surface, None);
        };
        let services = self
            .0
            .get(i)
            .ok_or_else(|| SolidError::invalid_input(OP, "handle of an absent B-rep"))?;
        match c {
            CacheObject::Surface(face) => {
                services.cache_state_mapped(k, face, |c| Ok(SideObject(i, c)))
            }
            _ => Err(SolidError::invalid_input(OP, "expected surface handle")),
        }
    }
    fn face_queries(&self) -> Option<&dyn FaceQueries<SideObject>> {
        Some(self)
    }
}

impl FaceQueries<SideObject> for SidedFaceServices<'_> {
    fn face_zone_tol_3d(&self, face: SideObject) -> Result<f64> {
        self.side(face)?.face_zone_tol_3d(face.1)
    }
    fn face_point_classify(
        &self,
        k: &Kernel,
        face: SideObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<(i32, Option<SideObject>)> {
        let (class, object) = self
            .side(face)?
            .face_point_classify(k, face.1, uv, tolerance)?;
        Ok((class, object.map(|o| SideObject(face.0, o))))
    }
    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: SideObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<(i32, Option<SideObject>)> {
        let (class, object) = self
            .side(face)?
            .face_point_classify_fast_drop(k, face.1, uv, tolerance)?;
        Ok((class, object.map(|o| SideObject(face.0, o))))
    }
    fn curve_edge_tolerance(&self, curve: SideObject) -> Option<Result<f64>> {
        match self.side(curve) {
            Ok(side) => side.curve_edge_tolerance(curve.1),
            Err(e) => Some(Err(e)),
        }
    }
}

/// A solution object of a free curve's solve against a B-rep face's surface (upstream's
/// solutions name the curve, which belongs to no B-rep, and the face's surface): the
/// curve with its natural interval, or a [`CacheObject`] of the B-rep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CurveFaceObject {
    /// The free curve (`SmCurve::GetNaturalInterval` is its interval).
    Curve(Extent1d),
    /// An object of the B-rep.
    Brep(CacheObject),
}

impl crate::geom::solution_array::SolutionObject for CurveFaceObject {
    fn kind(&self) -> crate::geom::solution_array::ObjectKind {
        match self {
            Self::Curve(_) => crate::geom::solution_array::ObjectKind::Curve,
            Self::Brep(c) => c.kind(),
        }
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        match self {
            Self::Curve(ivl) => Some(*ivl),
            Self::Brep(_) => None,
        }
    }
}

/// A [`BrepFaceServices`] on [`CurveFaceObject`] handles: the face caches and
/// classifications of its B-rep for a free curve's solve against a face's surface.
#[derive(Debug)]
pub struct CurveFaceServices<'s, 'a>(pub &'s BrepFaceServices<'a>);

fn brep_object(o: CurveFaceObject) -> Result<CacheObject> {
    match o {
        CurveFaceObject::Brep(c) => Ok(c),
        CurveFaceObject::Curve(_) => Err(SolidError::invalid_input(OP, "expected B-rep handle")),
    }
}

impl SurfaceSolveContext<CurveFaceObject> for CurveFaceServices<'_, '_> {
    fn build_surface_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<CurveFaceObject>,
    ) -> Result<SurfaceCache<CurveFaceObject>> {
        // (as `BrepFaceServices::build_surface_cache`, mapped in the one copy the fetch
        // makes)
        match handle {
            Some(CurveFaceObject::Brep(CacheObject::Surface(face))) => {
                self.0
                    .cache_mapped(k, face, |c| Ok(CurveFaceObject::Brep(c)))
            }
            None => crate::surface::surface::p2::surface_cache(k, surface, None),
            Some(_) => Err(SolidError::invalid_input(OP, "expected surface handle")),
        }
    }
    fn surface_cache_state(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<CurveFaceObject>,
    ) -> Result<SurfaceCache<CurveFaceObject>> {
        match handle {
            Some(CurveFaceObject::Brep(CacheObject::Surface(face))) => {
                self.0
                    .cache_state_mapped(k, face, |c| Ok(CurveFaceObject::Brep(c)))
            }
            None => crate::surface::surface::p2::surface_cache(k, surface, None),
            Some(_) => Err(SolidError::invalid_input(OP, "expected surface handle")),
        }
    }
    fn face_queries(&self) -> Option<&dyn FaceQueries<CurveFaceObject>> {
        Some(self)
    }
}

impl FaceQueries<CurveFaceObject> for CurveFaceServices<'_, '_> {
    fn face_zone_tol_3d(&self, face: CurveFaceObject) -> Result<f64> {
        self.0.face_zone_tol_3d(brep_object(face)?)
    }
    fn face_point_classify(
        &self,
        k: &Kernel,
        face: CurveFaceObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<(i32, Option<CurveFaceObject>)> {
        let (class, object) = self
            .0
            .face_point_classify(k, brep_object(face)?, uv, tolerance)?;
        Ok((class, object.map(CurveFaceObject::Brep)))
    }
    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        face: CurveFaceObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<(i32, Option<CurveFaceObject>)> {
        let (class, object) =
            self.0
                .face_point_classify_fast_drop(k, brep_object(face)?, uv, tolerance)?;
        Ok((class, object.map(CurveFaceObject::Brep)))
    }
    fn curve_edge_tolerance(&self, curve: CurveFaceObject) -> Option<Result<f64>> {
        // (the free curve has no owning edge: upstream's `GetOwner()` is NULL)
        match curve {
            CurveFaceObject::Curve(_) => None,
            CurveFaceObject::Brep(c) => self.0.curve_edge_tolerance(c),
        }
    }
}
