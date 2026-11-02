//! The trimmed surface cache `SmTrimSrfCache`: a surface cache whose decomposition
//! tree also records the face's trim curves (edge uses), vertices and poly edges per
//! node, classifies its nodes in, on or out of the face, and tests and classifies
//! parameter points against the face boundary.
//!
//! Upstream: `source/SMLib/src/SmTrimSrfCache.cpp`, `source/SMLib/inc/SmTrimSrfCache.h`.
//!
//! | Rust                                      | Upstream                                   |
//! |-------------------------------------------|--------------------------------------------|
//! | [`TrimSrfCache`]                          | `SmTrimSrfCache`                           |
//! | [`TrimSrfCache::new`]                     | `SmTrimSrfCache::SmTrimSrfCache`           |
//! | `Drop` (implicit)                         | `SmTrimSrfCache::~SmTrimSrfCache`          |
//! | [`TYPE`], [`TYPE_STRING`], [`TrimSrfCache::type_code`], [`TrimSrfCache::type_string`] | `GetType`, `GetTypeString` (`SM_COMMON`) |
//! | [`is_kind_of`], [`TrimSrfCache::is_kind_of`] | `SmTrimSrfCache::IsKindOf`              |
//! | [`TrimSrfCache::point_test`]              | `SmTrimSrfCache::PointTest`                |
//! | [`TrimSrfCache::point_classify`]          | `SmTrimSrfCache::PointClassify`            |
//! | [`TrimSrfCache::build_tree`], [`TrimSrfCache::build_tree_with_face`] | `SmTrimSrfCache::BuildTree` |
//! | [`TrimSrfCache::get_tree_with_face`]      | `SmSurfaceCache::GetTree` of a trimmed cache (virtual `BuildTree`) |
//! | [`TrimSrfCache::point_test_in`], [`TrimSrfCache::point_classify_in`] | `PointTest`, `PointClassify` over the given face |
//! | [`CacheFace`]                             | `GetFace()` and what the cache reads from the face and its B-rep |
//! | [`TrimSrfCache::bind_face`]                 | `GetFace()` with an explicit caller-handle mapping |
//! | [`TrimSrfCache::implant_edgeuse`]         | `SmTrimSrfCache::ImplantEdgeuse`           |
//! | [`TrimSrfCache::implant_poly_edge`], [`PolyEdgeInput`] | `SmTrimSrfCache::ImplantPolyEdge` |
//! | [`TrimSrfCache::implant_vertexuse`], [`VertexuseInput`] | `SmTrimSrfCache::ImplantVertexuse` |
//!
//! Deviations:
//! - **Inheritance.** `SmTrimSrfCache` derives from `SmSurfaceCache`; here it wraps a
//!   [`SurfaceCache`] ([`TrimSrfCache::base`], [`TrimSrfCache::base_mut`]) and adds
//!   the overriding methods. The constructor's `dAngleTolDeg` is handed to the base
//!   constructor's `dAngTolRad` unconverted, as upstream does: pass it as
//!   [`SurfaceCacheOptions::ang_tol_rad`].
//! - **Topology reads.** The topology objects are caller handles (`O`), so what the
//!   implant functions read from them is passed in: the edge use's UV trim curve
//!   (`SmEdgeuse::GetUVTrimCurve`), the vertex use's parameter point and vertex
//!   tolerance (`SmVertexuse::ComputeUVPoint`, `SmVertex::GetTolerance`), the poly
//!   edge's end points and tolerance and the surface of its original face
//!   (`SmPolyEdge::GetStartPoint`, `GetEndPoint`, `GetTolerance`,
//!   `GetPolyFace()->GetOriginalFace()->GetSurface()`). `GetFace()` is a
//!   [`CacheFace`] the caller gives ([`TrimSrfCache::build_tree_with_face`],
//!   [`TrimSrfCache::point_test_in`], [`TrimSrfCache::point_classify_in`]): the
//!   surface's owner is an object reference the cache cannot follow to the B-rep.
//!   Without it ([`TrimSrfCache::build_tree`], [`TrimSrfCache::point_test`],
//!   [`TrimSrfCache::point_classify`]) a face-owned surface stops at
//!   [`SurfaceCache::face`] ([`SolidError::Unported`]). `BuildTree` reads the natural
//!   domain of `GetFace()->GetSurface()` through [`CacheFace::natural_uv_domain`],
//!   falling back to the cache surface only when the adapter does not supply it.
//!   An explicit [`TrimSrfCache::bind_face`] validates the source owner key and
//!   records its caller handle without decoding the key as a topology id.
//!   `SubdivideNode` re-implants a moved edge use with its retained exact UV curve
//!   and resolved edge owner tolerance, without cloning it for either child.
//!   [`EdgeuseInput`] transfers that input into the cache; face construction resolves
//!   the tolerance through [`CacheFace::zone_tol_3d`], and `None` is reserved for
//!   ownerless standalone UV curves. Handle mapping retains the same tolerance.
//!   Poly edges retain their endpoints, tolerance and an evaluation copy of their
//!   original face surface;
//!   callers must reimplant an edited edge before subdividing, as for edge-use curves.
//! - **Node lists.** `SmTList::Prepend` onto a node's edge use, vertex and poly edge
//!   lists inserts at the front of the [`BezierAux2d`] `Vec`s (list order, head first).
//!   The list memory managers (`m_sVLMgr`, `m_sEdgeuseListMgr`) have no counterpart.
//! - **Temporary state.** `SmTemporaryChangeValue` (restored on every return) becomes
//!   explicit save / restore around the inner bodies; as upstream, the
//!   `m_bPointTestEnabled` set at the end of the face branch is overwritten by the
//!   restore when the branch ends.
//! - **Virtual dispatch.** `MarkFaceContainment` runs the base traversal through
//!   [`sc::FaceContainmentClassifier`], with the face as classifier. Point tests on
//!   an unwrapped base retain trimmed dispatch and require an operation-scoped
//!   [`FaceQueries`] service. A base fallback wrapped by [`TrimSrfCache::from_base`]
//!   keeps its untrimmed dispatch. Poly-edge reimplantation reads the retained
//!   geometry, visits the parent list head first and leaves that list intact, as native.
//! - **Recursion.** Edge implantation uses an explicit subdivision stack. Its
//!   nested reimplantation always disables subdivision, so that call chain has
//!   at most one nested implantation. Geometric subdivision retains the base
//!   cache's documented bounds; construction uses its containment guard.
//! - **Unported callees** (all return [`SolidError::Unported`]): `SmSurface::GetFace`
//!   without a [`CacheFace`] (through [`SurfaceCache::face`]); `SmTol::GetZoneTol3d`
//!   and `SmFace::PointClassify` of [`TrimSrfCache::point_test`] /
//!   [`TrimSrfCache::point_classify`] without a [`CacheFace`].
//! - **Undefined behaviour.** Upstream dereferences a null face in `PointTest` /
//!   `PointClassify` of a cache whose surface has no face; the port returns
//!   `SM_ERR_NULL_POINTER`. A node without surface data in this cache gives
//!   [`SolidError::InvalidInput`].
//! - **Not ported here.** `GetMemoryUsed` (the base class sum) and `Dump` (debug
//!   output).

use crate::curve::curve::Curve;
use crate::curve::curve_cache::{CurveCache, CurveCacheOptions};
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::object::ObjectRef;
use crate::geom::tree::{NodeClass, NodeData, NodeId, Tree};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::{BSplineCurve, BoundingBoxRequest};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::surface::surface::p2::PLANE_TYPE;
use crate::surface::surface::Surface;
use crate::surface::surface_cache::face_context::FaceQueries;
use crate::surface::surface_cache::p1::{self as sc, FaceContainment, SurfaceCacheOptions};
use crate::surface::surface_cache::{BezierAux2d, SurfaceCache};

/// `SmTrimSrfCache_TYPE` (`SURF_BASE_TYPE + 120`).
pub const TYPE: i64 = 15120;

/// `SmTrimSrfCache::GetTypeString`.
pub const TYPE_STRING: &str = "SmTrimSrfCache_TYPE";

/// `SM_ZONE_TOL_3D`.
const ZONE_TOL_3D: f64 = crate::geom::tol::ZONE_TOL_3D;

/// `SM_SS_UMIN`.
const SS_UMIN: u64 = 1;
/// `SM_SS_VMIN`.
const SS_VMIN: u64 = 2;
/// `SM_SS_UMAX`.
const SS_UMAX: u64 = 4;
/// `SM_SS_VMAX`.
const SS_VMAX: u64 = 8;
/// `SM_SS_NONE`.
const SS_NONE: u64 = 0;

/// `SM_PC_UNKNOWN` (`SmPointClassificationType`).
pub const POINT_CLASS_UNKNOWN: i32 = 100;
/// `SM_PC_FACE`.
pub const POINT_CLASS_FACE: i32 = 102;
/// `SM_PC_EDGE`.
pub const POINT_CLASS_EDGE: i32 = 103;
/// `SM_PC_VERTEX`.
pub const POINT_CLASS_VERTEX: i32 = 104;

const OP_CLASSIFY: &str = "surface::trim_srf_cache::TrimSrfCache::point_classify";
const OP_EU: &str = "surface::trim_srf_cache::TrimSrfCache::implant_edgeuse";
const OP_VU: &str = "surface::trim_srf_cache::TrimSrfCache::implant_vertexuse";
const OP_GET_TREE: &str = "surface::trim_srf_cache::TrimSrfCache::get_tree_with_face";
const OP_BUILD: &str = "surface::trim_srf_cache::TrimSrfCache::build_tree_with_face";

/// `SmTrimSrfCache::IsKindOf`: the trimmed surface cache or a base type.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || sc::is_kind_of(t)
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// What `ImplantVertexuse` reads from the vertex use (see the module deviations).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexuseInput<O> {
    /// The vertex use (stored in the node vertex lists).
    pub vertexuse: O,
    /// `SmVertexuse::ComputeUVPoint`.
    pub uv_point: Point2,
    /// `GetVertex()->GetTolerance()`.
    pub vertex_tolerance: f64,
}

/// What `ImplantEdgeuse` reads from the edge use, transferred into the cache for
/// later child reimplantation without copying the curve.
#[derive(Debug)]
pub struct EdgeuseInput<O> {
    /// The edge use stored in the node lists.
    pub edgeuse: O,
    /// The exact `GetUVTrimCurve()` geometry; `None` preserves the native null error.
    pub uv_trim_curve: Option<BSplineCurve>,
    /// The zone tolerance of the edge reached through the curve's owning edge use.
    /// Supply `Some` for an owned UV curve, resolved from its actual B-rep; `None`
    /// is only for an ownerless standalone curve and retains the system tolerance.
    pub owner_zone_tol_3d: Option<f64>,
}

/// What `ImplantPolyEdge` reads from the poly edge (see the module deviations).
#[derive(Debug, Clone, Copy)]
pub struct PolyEdgeInput<'a, O> {
    /// The poly edge (stored in the node poly edge lists).
    pub poly_edge: O,
    /// `GetStartPoint()` (its x and y are the parameter point).
    pub start: Point3,
    /// `GetEndPoint()`.
    pub end: Point3,
    /// `GetTolerance()`.
    pub tolerance: f64,
    /// `GetPolyFace()->GetOriginalFace()->GetSurface()`.
    pub surface: &'a Surface,
}

/// The owning face of a trimmed surface cache's surface (`GetFace()`) and what
/// `BuildTree` and `PointClassify` read from it and its B-rep. The face, its loop uses,
/// edge uses and vertex uses are handles in the cache's object space (`O`); the B-rep
/// layer implements this over its records (`query::face::p2`), so the cache reaches
/// the face the surface's owner names without holding the B-rep.
pub trait CacheFace<O> {
    /// The face (`GetFace()`).
    fn face(&self) -> O;

    /// `GetFace()->GetBrep()->m_bEditingEnabled`.
    ///
    /// # Errors
    ///
    /// The implementor's (a face without B-rep).
    fn brep_editing_enabled(&self) -> Result<bool>;

    /// Sets `GetFace()->GetBrep()->m_bEditingEnabled`.
    ///
    /// # Errors
    ///
    /// The implementor's (a face without B-rep).
    fn set_brep_editing_enabled(&mut self, enabled: bool) -> Result<()>;

    /// `GetFace()->CreateUVTrimCurves(TRUE, NULL, NULL, ..)` (its gap statistics are
    /// not read).
    ///
    /// # Errors
    ///
    /// Those of `CreateUVTrimCurves`.
    fn create_uv_trim_curves(&mut self, k: &Kernel) -> Result<()>;

    /// `GetFace()->GetUpwardLoopuses(sLoopuses)`.
    ///
    /// # Errors
    ///
    /// The implementor's.
    fn upward_loopuses(&self) -> Result<Vec<O>>;

    /// `pLU->GetEdgeuses(sEUses)`.
    ///
    /// # Errors
    ///
    /// The implementor's.
    fn loopuse_edgeuses(&self, loopuse: O) -> Result<Vec<O>>;

    /// `pLU->GetVertexuses(sVUses)`.
    ///
    /// # Errors
    ///
    /// The implementor's (a null vertex use, which upstream dereferences).
    fn loopuse_vertexuses(&self, loopuse: O) -> Result<Vec<O>>;

    /// `SM_CAST_PTR(SmBSplineCurve, pEdgeuse->GetUVTrimCurve())` (`None` when there is
    /// none or it is no B-spline curve).
    ///
    /// # Errors
    ///
    /// The implementor's.
    fn edgeuse_uv_trim_curve(&self, edgeuse: O) -> Result<Option<BSplineCurve>>;

    /// `pVertexuse->ComputeUVPoint(sUV)` (`bOKToMakeUVTrimCurve` default `TRUE`).
    ///
    /// # Errors
    ///
    /// Those of `ComputeUVPoint`.
    fn vertexuse_uv_point(&mut self, k: &Kernel, vertexuse: O) -> Result<Point2>;

    /// `pVertexuse->GetVertex()->GetTolerance()`.
    ///
    /// # Errors
    ///
    /// The implementor's (a vertex use without vertex).
    fn vertexuse_vertex_tolerance(&self, vertexuse: O) -> Result<f64>;

    /// `SmTol::GetZoneTol3d(pObject)` of the face or of an object a node records.
    ///
    /// # Errors
    ///
    /// The implementor's.
    fn zone_tol_3d(&self, k: &Kernel, object: O) -> Result<f64>;

    /// The natural domain of `GetFace()->GetSurface()`, when available. It can
    /// differ from the generated surface whose decomposition is being cached.
    /// `None` preserves the cache-surface domain for adapters without this query.
    ///
    /// # Errors
    ///
    /// The implementor's (for example, a face without a surface).
    fn natural_uv_domain(&self) -> Result<Option<Extent2d>> {
        Ok(None)
    }

    /// `GetFace()->PointClassify(uv, tol, FALSE, TRUE, sPC)` into a fresh
    /// `SmPointClassification(tol)`: its class (`SmPointClassificationType`) and
    /// object.
    ///
    /// # Errors
    ///
    /// Those of `SmFace::PointClassify`.
    fn point_classify(
        &mut self,
        k: &Kernel,
        uv_point: Point2,
        src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<O>)>;
}

/// [`CacheFace`] as `MarkFaceContainment`'s classifier: `PointClassify` dispatches to
/// [`TrimSrfCache::point_classify_in`].
struct FaceClassifier<'f, O>(&'f mut dyn CacheFace<O>);

impl<O> sc::FaceContainmentClassifier<O> for FaceClassifier<'_, O> {
    fn zone_tol_3d(&mut self, k: &Kernel, face: O) -> Result<f64> {
        self.0.zone_tol_3d(k, face)
    }

    fn point_classify(
        &mut self,
        k: &Kernel,
        uv_point: Point2,
        src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<O>)> {
        classify_in_face(k, self.0, uv_point, src_zone_tol_3d)
    }
}

/// The body of `SmTrimSrfCache::PointClassify` over the cache's face.
fn classify_in_face<O>(
    k: &Kernel,
    face: &mut dyn CacheFace<O>,
    uv_point: Point2,
    src_zone_tol_3d: f64,
) -> Result<(i32, Option<O>)> {
    // Classify the Point against the surface->Face
    let (class, object) = face.point_classify(k, uv_point, src_zone_tol_3d)?;

    // set output
    let object = if class == POINT_CLASS_UNKNOWN {
        None
    } else {
        object
    };
    Ok((class, object))
}

/// Geometry read again when `SubdivideNode` reimplants a polygon edge. The surface
/// is an exact evaluation copy, independent of the cache's surface and handle space.
#[derive(Debug, Clone)]
struct PolyEdgeGeometry {
    start: Point3,
    end: Point3,
    tolerance: f64,
    surface: Surface,
}

impl PolyEdgeGeometry {
    fn input<O>(&self, poly_edge: O) -> PolyEdgeInput<'_, O> {
        PolyEdgeInput {
            poly_edge,
            start: self.start,
            end: self.end,
            tolerance: self.tolerance,
            surface: &self.surface,
        }
    }
}

/// Exact UV input and its resolved owner context, retained together across
/// subdivision and handle mapping.
#[derive(Debug, Clone)]
struct EdgeuseGeometry {
    curve: BSplineCurve,
    owner_zone_tol_3d: Option<f64>,
}

/// The trimmed surface cache (`SmTrimSrfCache`): a [`SurfaceCache`] whose tree
/// knows the face boundary.
#[derive(Debug, Clone)]
pub struct TrimSrfCache<O> {
    /// The `SmSurfaceCache` base.
    base: SurfaceCache<O>,
    /// UV trim geometry and resolved owner tolerance, read again when subdivision
    /// moves an edge use into the new children.
    edgeuse_curves: Vec<(O, EdgeuseGeometry)>,
    /// Geometry of implanted polygon edges, keyed by their opaque handles.
    poly_edge_geometry: Vec<(O, PolyEdgeGeometry)>,
}

impl<O: Copy + PartialEq> TrimSrfCache<O> {
    /// A trimmed cache of `surface` over its natural domain, not built
    /// (`SmTrimSrfCache::SmTrimSrfCache`): the state anticipates a face with trim
    /// curves (point testing on, boundary curves off) until [`TrimSrfCache::build_tree`]
    /// finds out.
    #[must_use]
    pub fn new(surface: Surface, opts: SurfaceCacheOptions) -> Self {
        // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
        let domain = surface.natural_uv_domain_if_built();
        let mut base = SurfaceCache::new(surface, domain, opts);

        // set state bits anticipating a Trim Surface cache with UVTrimCurves.
        // BuildTree() will change these values when there are no UVTrimCurves.
        base.face_was_modified = false;
        base.process_boundary_curves = false; // solvers skip naturalBoundaryCurve/target solutions
        base.point_test_enabled = true; // solvers only accept point solutions within trim boundaries
        base.have_t_surface_cache = false; // Set to true after adding UVTrimCurves to cache
        base.face_containment_done = FaceContainment::NotDone;
        base.trimmed_dispatch = true;
        Self {
            base,
            edgeuse_curves: Vec::new(),
            poly_edge_geometry: Vec::new(),
        }
    }

    /// Wraps an existing cache as is (no state change).
    #[must_use]
    pub const fn from_base(base: SurfaceCache<O>) -> Self {
        Self {
            base,
            edgeuse_curves: Vec::new(),
            poly_edge_geometry: Vec::new(),
        }
    }

    /// The `SmSurfaceCache` base.
    #[must_use]
    pub const fn base(&self) -> &SurfaceCache<O> {
        &self.base
    }

    /// The `SmSurfaceCache` base, mutable.
    pub fn base_mut(&mut self) -> &mut SurfaceCache<O> {
        &mut self.base
    }

    /// The same cache with every handle mapped by `map` (the base's
    /// [`SurfaceCache::map_handles`] and the implanted edge-use and polygon-edge geometry).
    ///
    /// # Errors
    ///
    /// Those of `map` and of the base's mapping.
    pub fn map_handles<P: Copy + PartialEq>(
        &self,
        map: impl Fn(O) -> Result<P>,
    ) -> Result<TrimSrfCache<P>> {
        let edgeuse_curves = self
            .edgeuse_curves
            .iter()
            .map(|(o, g)| {
                Ok((
                    map(*o)?,
                    EdgeuseGeometry {
                        curve: g.curve.clone_exact(),
                        owner_zone_tol_3d: g.owner_zone_tol_3d,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        Ok(TrimSrfCache {
            base: self.base.map_handles(&map)?,
            edgeuse_curves,
            poly_edge_geometry: self
                .poly_edge_geometry
                .iter()
                .map(|(o, g)| {
                    Ok((
                        map(*o)?,
                        PolyEdgeGeometry {
                            start: g.start,
                            end: g.end,
                            tolerance: g.tolerance,
                            surface: g.surface.clone_for_evaluation()?,
                        },
                    ))
                })
                .collect::<Result<_>>()?,
        })
    }

    /// Unwraps the `SmSurfaceCache` base.
    #[must_use]
    pub fn into_base(self) -> SurfaceCache<O> {
        self.base
    }

    /// `SmTrimSrfCache_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        TYPE
    }

    /// `"SmTrimSrfCache_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        TYPE_STRING
    }

    /// [`is_kind_of`] (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        is_kind_of(t)
    }

    // ---- point tests ------------------------------------------------------------------

    /// Whether `uv_point` lies in or on the face boundary (`PointTest`): `(ok,
    /// object)`, `ok` true when point testing is off, the face was modified, or the
    /// point classifies to the face, one of its edges or vertices (`object`).
    /// `opt_zone_tol_3d` is the tolerance of the object the point comes from (else the
    /// face's).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the surface has no face; those of the face
    /// classification; [`SolidError::Unported`] for its unported callees.
    pub fn point_test(
        &self,
        k: &Kernel,
        uv_point: Point2,
        opt_zone_tol_3d: Option<f64>,
    ) -> Result<(bool, Option<O>)> {
        self.base
            .point_test_with(k, uv_point, opt_zone_tol_3d, None)
    }

    /// Point test using an operation-scoped prepared face-query service.
    pub fn point_test_with(
        &self,
        k: &Kernel,
        uv_point: Point2,
        opt_zone_tol_3d: Option<f64>,
        queries: Option<&dyn FaceQueries<O>>,
    ) -> Result<(bool, Option<O>)> {
        self.base
            .point_test_with(k, uv_point, opt_zone_tol_3d, queries)
    }

    /// Classifies a point using the shared operation-scoped face-query service.
    pub fn point_classify_with(
        &self,
        k: &Kernel,
        uv_point: Point2,
        src_zone_tol_3d: f64,
        face: O,
        queries: &dyn FaceQueries<O>,
    ) -> Result<(i32, Option<O>)> {
        if self.base.face()? != Some(face) {
            return Err(invalid(
                OP_CLASSIFY,
                "classifier face does not match cache binding",
            ));
        }
        self.base
            .point_classify_with(k, uv_point, src_zone_tol_3d, queries)
    }

    /// Binds this cache to the caller's actual owning face without building its
    /// tree. Classification can then run while Boolean construction is delayed.
    ///
    /// # Errors
    ///
    /// A key that does not match the source surface's reached face owner. The
    /// caller supplies the key and handle explicitly; neither is decoded as an id.
    pub fn bind_face(&mut self, key: ObjectRef, face: O, tolerance: f64) -> Result<()> {
        self.base.bind_trimmed_face(key, face)?;
        self.base.set_owner_zone_tol_3d(tolerance);
        Ok(())
    }

    fn set_standalone_flags(&mut self) {
        self.base.process_boundary_curves = true;
        self.base.point_test_enabled = false;
        self.base.have_t_surface_cache = false;
    }

    /// Restores the containment guard and releases temporary subdivision geometry,
    /// including on errors. Edge-use curves stay available for reimplantation.
    fn finish_build(
        &mut self,
        result: Result<bool>,
        saved: FaceContainment,
        fresh_tree: bool,
    ) -> Result<()> {
        let cleanup = if fresh_tree {
            self.clean_up_build_data()
        } else {
            Ok(())
        };
        let result = result.and_then(|done| cleanup.map(|()| done));
        self.base.face_containment_done = if matches!(result, Ok(true)) {
            FaceContainment::Done
        } else {
            saved
        };
        if result.is_err() {
            self.base.have_t_surface_cache = false;
        }
        result.map(|_| ())
    }

    /// Classifies `uv_point` against the face (`PointClassify`): the
    /// `SmPointClassificationType` value and the face, edge or vertex the point is
    /// in or on (`None` for `SM_PC_UNKNOWN`). `src_zone_tol_3d` is the tolerance of
    /// the object the point comes from.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the surface has no face; those of the face
    /// classification; [`SolidError::Unported`] for its unported callees.
    pub fn point_classify(
        &self,
        _k: &Kernel,
        _uv_point: Point2,
        _src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<O>)> {
        self.base
            .face()?
            .ok_or(SolidError::null_pointer(OP_CLASSIFY))?;
        Err(SolidError::unported(
            "prepared face classifier for bound trim",
            "query.face_p2",
        ))
    }

    /// [`TrimSrfCache::point_test`] with the surface's face given (`GetFace()`).
    ///
    /// # Errors
    ///
    /// Those of the face classification.
    pub fn point_test_in(
        &self,
        k: &Kernel,
        face: &mut dyn CacheFace<O>,
        uv_point: Point2,
        opt_zone_tol_3d: Option<f64>,
    ) -> Result<(bool, Option<O>)> {
        // when testing is off or cache is not current
        if !self.base.point_test_enabled || self.base.face_was_modified {
            // return TRUE so this point won't be culled
            return Ok((true, None));
        }

        self.check_face_binding(face.face())?;

        // classify point (with optional tolerance) against face
        let src_zone_tol_3d = match opt_zone_tol_3d {
            Some(t) => t,
            None => face.zone_tol_3d(k, face.face())?,
        };
        let (class, object) = face.point_classify(k, uv_point, src_zone_tol_3d)?;

        // Point is ok when it classified to a topology object
        let ok =
            class == POINT_CLASS_FACE || class == POINT_CLASS_EDGE || class == POINT_CLASS_VERTEX;
        Ok((ok, object))
    }

    /// [`TrimSrfCache::point_classify`] with the surface's face given (`GetFace()`).
    ///
    /// # Errors
    ///
    /// Those of the face classification.
    pub fn point_classify_in(
        &self,
        k: &Kernel,
        face: &mut dyn CacheFace<O>,
        uv_point: Point2,
        src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<O>)> {
        self.check_face_binding(face.face())?;
        classify_in_face(k, face, uv_point, src_zone_tol_3d)
    }

    fn check_face_binding(&self, face: O) -> Result<()> {
        if self.base.trimmed_face.is_some() && self.base.face()? != Some(face) {
            return Err(invalid(
                OP_BUILD,
                "supplied face does not match cache binding",
            ));
        }
        Ok(())
    }

    // ---- construction ------------------------------------------------------------------

    /// Builds a standalone surface's decomposition tree (`BuildTree`). A face-owned
    /// surface requires [`Self::build_tree_with_face`] to prepare and implant its
    /// actual trims; opaque handles alone cannot service that build.
    ///
    /// # Errors
    ///
    /// Those of tree construction; [`SolidError::Unported`] for a face-owned surface
    /// without a [`CacheFace`] adapter.
    pub fn build_tree(&mut self, k: &Kernel) -> Result<()> {
        self.build_tree_in(k, None)
    }

    /// [`TrimSrfCache::build_tree`] with the surface's face given (`GetFace()`): `face`
    /// must be the face owning the cache's surface. Its UV trim curves are created
    /// (`CreateUVTrimCurves`, which changes the B-rep) and its edge uses and vertex uses
    /// implanted unless its B-rep is being edited.
    ///
    /// # Errors
    ///
    /// Those of the tree construction, of the face reads, the implanting and the
    /// classification.
    pub fn build_tree_with_face(&mut self, k: &Kernel, face: &mut dyn CacheFace<O>) -> Result<()> {
        self.check_face_binding(face.face())?;
        self.build_tree_in(k, Some(face))
    }

    /// The tree, built first when there is none, or when the surface belongs to a face
    /// whose boundary is not in the tree yet and no build is under way
    /// (`SmSurfaceCache::GetTree`, whose virtual `BuildTree` is this cache's
    /// [`TrimSrfCache::build_tree_with_face`]); `face` is the face owning the cache's
    /// surface (`GetFace()`). A cache that is not a trimmed one (the plain
    /// `SmSurfaceCache` fallback of `smsurf_GetSurfaceCache`) runs its own
    /// [`SurfaceCache::get_tree`].
    ///
    /// # Errors
    ///
    /// Those of [`TrimSrfCache::build_tree_with_face`] (upstream returns NULL), and
    /// of [`SurfaceCache::get_tree`] for a plain cache.
    pub fn get_tree_with_face(
        &mut self,
        k: &Kernel,
        face: &mut dyn CacheFace<O>,
    ) -> Result<&mut Tree<O>> {
        if !self.base.trimmed_dispatch {
            return self.base.get_tree(k);
        }
        let face_owned = self.base.surface().face()?.is_some();
        if self.base.tree().is_none()
            || (!self.base.have_t_surface_cache
                && face_owned
                && self.base.face_containment_done != FaceContainment::UnderConstruction)
        {
            self.build_tree_with_face(k, face)?;
        }
        self.base
            .tree_mut()
            .ok_or_else(|| invalid(OP_GET_TREE, "cache has no tree"))
    }

    fn build_tree_in(&mut self, k: &Kernel, face: Option<&mut dyn CacheFace<O>>) -> Result<()> {
        // prevent bad recursion back into BuildTree
        let saved = self.base.face_containment_done;
        let fresh_tree = self.base.tree().is_none();
        self.base.face_containment_done = FaceContainment::UnderConstruction;
        let result = self.build_tree_body(k, face);
        self.finish_build(result, saved, fresh_tree)
    }

    /// `BuildTree` inside the face containment guard; `true` when the guard was
    /// cleared (containment done).
    fn build_tree_body(&mut self, k: &Kernel, face: Option<&mut dyn CacheFace<O>>) -> Result<bool> {
        // Build m_ptree surface subdivision tree when it does not exist
        if self.base.tree().is_none() {
            surface_cache_build_tree(&mut self.base, k)?;
        }

        // skip inserting UVTrimCurves and classifying node containment
        //   when Surface is not owned by a face (no UVTrimCurves to process)
        //     or Brep Editing is enabled (face data structure may be in an intermediate state)
        // (`GetFace()`: the caller's face, else the surface's owner, which has no
        // handle here when it is a face)
        let face = if let Some(f) = face {
            Some(f)
        } else {
            self.base.face()?;
            // A bound handle is not a mutable topology service. Do not silently
            // turn a face-owned cache into an untrimmed rectangle without one.
            if self.base.surface().face()?.is_some() {
                return Err(SolidError::unported(
                    "TrimSrfCache::build_tree_with_face for a face-owned surface",
                    "surface.trim_srf_cache",
                ));
            }
            None
        };
        let mut cleared = false;
        let editing_face = match face {
            None => None,
            Some(f) => {
                if f.brep_editing_enabled()? {
                    None
                } else {
                    Some(f)
                }
            }
        };
        match editing_face {
            None => self.set_standalone_flags(),
            Some(f) => {
                // SmTemporaryChangeValue restores point testing on every exit.
                let saved_point_test = self.base.point_test_enabled;
                self.base.point_test_enabled = false;
                let result = self.implant_face(k, f);
                self.base.point_test_enabled = saved_point_test;
                cleared = result?;
            }
        }
        Ok(cleared)
    }

    /// The face branch of `BuildTree` (editing disabled in the brep): implant every
    /// edge use and vertex use, reduce the cache domain to the face domain and
    /// classify the nodes. `true` when the face containment is done.
    fn implant_face(&mut self, k: &Kernel, face: &mut dyn CacheFace<O>) -> Result<bool> {
        let saved_editing = face.brep_editing_enabled()?;
        face.set_brep_editing_enabled(true)?;
        let result = self.implant_face_body(k, face);
        // SmTemporaryChangeValue restores the brep editing flag on errors too.
        face.set_brep_editing_enabled(saved_editing)?;
        result
    }

    fn implant_face_body(&mut self, k: &Kernel, face: &mut dyn CacheFace<O>) -> Result<bool> {
        // create UVTrimCurve for every face->edge
        let created = face.create_uv_trim_curves(k);
        // Creation may raise the owning face's tolerance even on an error.
        // Refresh the evaluation copy before propagating either failure.
        let refreshed = face
            .zone_tol_3d(k, face.face())
            .map(|tol| self.base.set_owner_zone_tol_3d(tol));
        created?;
        refreshed?;

        // Now implant edgeuses and vertexuses into the cache tree
        let loopuses = face.upward_loopuses()?;
        let mut face_domain = Extent2d::new();

        // do subdivision on nonPlanar faces.
        let do_subdivision = !self.base.surface().is_kind_of(PLANE_TYPE)?;

        // for every loopuse - implant edgeuses into TrimSurfaceCache
        for &lu in &loopuses {
            for eu in face.loopuse_edgeuses(lu)? {
                let curve = face.edgeuse_uv_trim_curve(eu)?;
                self.implant_edgeuse(
                    k,
                    EdgeuseInput {
                        edgeuse: eu,
                        owner_zone_tol_3d: curve
                            .as_ref()
                            .map(|_| face.zone_tol_3d(k, eu))
                            .transpose()?,
                        uv_trim_curve: curve,
                    },
                    do_subdivision,
                    &mut face_domain,
                    None,
                )?;
            }
        }

        // for every Loopuse - implant every vertexuse into the TrimSurfCache
        for &lu in &loopuses {
            for vu in face.loopuse_vertexuses(lu)? {
                let input = VertexuseInput {
                    vertexuse: vu,
                    uv_point: face.vertexuse_uv_point(k, vu)?,
                    vertex_tolerance: face.vertexuse_vertex_tolerance(vu)?,
                };
                self.implant_vertexuse(k, input, &mut face_domain)?;
            }
        }

        // set SurfaceCacheDomain as SurfaceNaturalDomain/FaceDomain intersection
        let surface_domain = match face.natural_uv_domain()? {
            Some(d) => d,
            None => self.base.surface().natural_uv_domain_with_nurb(k)?,
        };
        let domain = surface_domain.intersect(&face_domain)?;
        self.base.set_uv_domain(domain);

        // When the surface has UVTrimCurves
        if loopuses.is_empty() {
            // there are no UVTrimCurves - treat as a standalone surface
            self.set_standalone_flags();
            Ok(false)
        } else {
            // mark state to run Solvers for TrimSurfaces
            self.base.process_boundary_curves = false;
            self.base.point_test_enabled = true;
            self.base.have_t_surface_cache = true;

            // Classify every TrimSurfCache Node not on a boundary as INSIDE or OUTSIDE
            //
            // (the virtual `MarkFaceContainment`, whose `PointClassify` is this cache's
            // `SmTrimSrfCache::PointClassify` over the face)
            self.base
                .mark_face_containment_with(k, &mut FaceClassifier(face))?;
            // set a permament value into m_bFaceContainment
            self.base.face_containment_done = FaceContainment::Done;
            Ok(true)
        }
    }

    /// Drops the construction data of a freshly built tree: the subdivision surfaces
    /// and the patches' Bezier surfaces; every node's surface becomes the cache
    /// surface.
    fn clean_up_build_data(&mut self) -> Result<()> {
        self.base.subdivision_surfaces_mut().clear();
        for p in self.base.patches_mut() {
            p.bezier = None;
        }
        let handle = self.base.surface_handle();
        if let Some(tree) = self.base.tree_mut() {
            let ids: Vec<NodeId> = (0..tree.nodes().len()).map(NodeId).collect();
            for id in ids {
                if let NodeData::Surface(a) = &mut tree.node_mut(id)?.data {
                    if a.slot.is_some() {
                        a.surface = handle;
                    }
                }
            }
        }
        Ok(())
    }

    // ---- implanting -----------------------------------------------------------------

    /// Puts `node` on the boundary (`SM_NC_ON_BOUNDARY`), marked when `mark`.
    fn set_on_boundary(&mut self, node: NodeId, mark: bool) -> Result<()> {
        let tree = self
            .base
            .tree_mut()
            .ok_or_else(|| invalid(OP_EU, "cache has no tree"))?;
        match &mut tree.node_mut(node)?.data {
            NodeData::Surface(a) => {
                a.node_class = NodeClass::OnBoundary;
                if mark {
                    a.is_marked = true;
                }
            }
            _ => return Err(invalid(OP_EU, "node has no surface data")),
        }
        Ok(())
    }

    /// `SmTreeNode::PropagateToParents` of `node`.
    fn propagate(&mut self, node: NodeId) -> Result<()> {
        self.base
            .tree_mut()
            .ok_or_else(|| invalid(OP_EU, "cache has no tree"))?
            .propagate_to_parents(node)
    }

    fn aux_mut(&mut self, node: NodeId) -> Result<&mut BezierAux2d<O>> {
        self.base.aux_mut(node)
    }

    /// Embeds the edge use and its UV curve / owner context from `input` into
    /// the decomposition (`ImplantEdgeuse`): every leaf near the curve (or near a pole
    /// side of the surface) is marked on the boundary and gets the edge use, the
    /// change is propagated to its ancestors; with `do_subdivision` large leaves are
    /// split first. Only `opt_tree_node` is visited when given, else every node whose
    /// domain meets the curve box. `face_extent` grows by the curve box.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without trim curve; those of the curve box, the curve
    /// cache and the tree; [`SolidError::Unported`] for `FindUVNodes` and
    /// `SubdivideNode`.
    pub fn implant_edgeuse(
        &mut self,
        k: &Kernel,
        input: EdgeuseInput<O>,
        do_subdivision: bool,
        face_extent: &mut Extent2d,
        opt_tree_node: Option<NodeId>,
    ) -> Result<()> {
        let geometry = EdgeuseGeometry {
            curve: input.uv_trim_curve.ok_or(SolidError::null_pointer(OP_EU))?,
            owner_zone_tol_3d: input.owner_zone_tol_3d,
        };
        let index = if let Some(index) = self
            .edgeuse_curves
            .iter()
            .position(|(handle, _)| *handle == input.edgeuse)
        {
            let (_, retained) = self
                .edgeuse_curves
                .get_mut(index)
                .ok_or_else(|| invalid(OP_EU, "edge use has no retained geometry"))?;
            *retained = geometry;
            index
        } else {
            let index = self.edgeuse_curves.len();
            self.edgeuse_curves.push((input.edgeuse, geometry));
            index
        };
        self.implant_retained_edgeuse(k, index, do_subdivision, face_extent, opt_tree_node)
    }

    #[allow(clippy::too_many_lines)]
    fn implant_retained_edgeuse(
        &mut self,
        k: &Kernel,
        index: usize,
        do_subdivision: bool,
        face_extent: &mut Extent2d,
        opt_tree_node: Option<NodeId>,
    ) -> Result<()> {
        let (edgeuse, geometry) = self
            .edgeuse_curves
            .get(index)
            .ok_or_else(|| invalid(OP_EU, "edge use has no retained geometry"))?;
        let edgeuse = *edgeuse;
        let eu_curve = &geometry.curve;

        // set up for poles
        let (poles, u_ivl, v_ivl, u_tol, v_tol) = {
            let uv_domain = self.base.surface().natural_uv_domain_with_nurb(k)?;

            // save natural domain sizes
            let u_ivl = uv_domain.u_interval();
            let v_ivl = uv_domain.v_interval();
            let u_tol = u_ivl.length() / 100.0;
            let v_tol = v_ivl.length() / 100.0;

            // list m_sUVDomain boundaries near SrfNaturalDomain boundaries
            let d = self.base.uv_domain();
            let mut poles = u64::from((d.min().x - u_ivl.min()).abs() < u_tol)
                + (if (d.min().y - v_ivl.min()).abs() < v_tol {
                    2
                } else {
                    0
                })
                + (if (d.max().x - u_ivl.max()).abs() < u_tol {
                    4
                } else {
                    0
                })
                + (if (d.max().y - v_ivl.max()).abs() < v_tol {
                    8
                } else {
                    0
                });

            // eliminate boundaries which are not poles
            if poles != 0 {
                let srf_poles = surface_singularities(k, self.base.surface())?;
                poles &= srf_poles;
            }
            (poles, u_ivl, v_ivl, u_tol, v_tol)
        };

        // sCurveBBox = EUCurve 2D Bounding Box expanded by tolerance
        let bbox = eu_curve
            .calculate_bounding_box(
                &eu_curve.natural_interval(),
                BoundingBoxRequest {
                    normal: true,
                    ..BoundingBoxRequest::default()
                },
            )?
            .normal
            .ok_or_else(|| invalid(OP_EU, "no curve box"))?;
        let mut curve_bbox =
            Extent2d::from_bounds(bbox.min().x, bbox.min().y, bbox.max().x, bbox.max().y);
        let expand = (1.0 + curve_bbox.size().length()) * EFF_ZERO_SQRT;
        curve_bbox.expand_absolute(expand);

        // when EUCurve is a line - get its local values
        let line = eu_curve
            .is_line(4, ZONE_TOL_3D / 10.0)?
            .map(|(p, v)| (Point2::new(p.x, p.y), Vec2::new(v.x, v.y)));

        // Compute OffAxisTolerance - maximum size of non largest component
        //                            of the bounding box of the curve segment.
        let mut diff = self.base.uv_domain().size();
        let mut scale = 1.0;

        let mut clamp_node_growth = false;
        if diff.x > diff.y * 20.0 {
            clamp_node_growth = true;
            scale = diff.x / diff.y;
        }
        if diff.y > diff.x * 20.0 {
            clamp_node_growth = true;
            scale = diff.x / diff.y;
        }
        // Scale things to smaller size if one dimension of UV domain is
        // very large compared to the other.
        if diff.x > diff.y * 100.0 {
            diff.x /= 100.0;
        } else if diff.x > diff.y * 10.0 {
            diff.x = diff.y * 10.0;
        }

        if diff.y > diff.x * 100.0 {
            diff.y /= 100.0;
        } else if diff.y > diff.x * 10.0 {
            diff.y = diff.x * 10.0;
        }

        let uv_size = diff.length();
        let off_axis_tolerance = uv_size / self.base.trim_subdivision_factor;
        let mut min_node_size_squared = off_axis_tolerance * off_axis_tolerance;

        // increase dMinNodeSizeSquared when working on very long lines
        if line.is_some() {
            let line_size = curve_bbox.size().length() / 10.0;
            min_node_size_squared = max2(min_node_size_squared, line_size * line_size);
        }

        // sMinSize
        let mut min_size = Vec2::new(0.0, 0.0);
        if clamp_node_growth {
            let diff2 = self.base.uv_domain().size();
            min_size = diff2 / self.base.trim_subdivision_factor;
        }

        // find all tree nodes that intersect sCurveBBox - use pOptTreeNode if given
        let nodes = match opt_tree_node {
            None => find_uv_nodes(&mut self.base, k, &curve_bbox, true)?,
            Some(n) => vec![n],
        };

        // no work - no nodes
        if nodes.is_empty() {
            return Ok(());
        }

        // Create the curve cache for nonLinear EUCurves
        let ivl = eu_curve.natural_interval();
        let curve_cache = if line.is_none() {
            let mut cc: CurveCache<O> = CurveCache::new(
                Curve::from(eu_curve.clone_exact()),
                ivl,
                CurveCacheOptions {
                    make_full_tree: true,
                    chord_height_tolerance: 0.0,
                    angle_tol_deg: 20.0,
                    off_axis_tolerance,
                    min_segments: 0,
                    compute_aux_tree_data: true,
                    ..CurveCacheOptions::default()
                },
            );
            cc.set_owner_zone_tol_3d(geometry.owner_zone_tol_3d);
            cc.tessellate(k)?;
            Some(cc)
        } else {
            None
        };

        // for every node that intersects the EUCurve bounding box
        let mut stack: Vec<NodeId> = Vec::new();
        for &start in &nodes {
            // init stack with current node
            stack.push(start);

            // while nodes exist in the stack
            while let Some(node) = stack.pop() {
                // skip non-leaf nodes
                let tree = self
                    .base
                    .tree()
                    .ok_or_else(|| invalid(OP_EU, "cache has no tree"))?;
                if tree.node(node)?.child1.is_some() {
                    continue;
                }

                // sNodeBBox = this node's 2D bounding box
                let node_dom = self.base.aux(node)?.uv_domain;
                let node_bbox =
                    Extent3d::from_points(Vec3::from(node_dom.min()), Vec3::from(node_dom.max()));

                // nodes by position
                let keep_for_position = match (&curve_cache, line) {
                    // when working with nonlinear curves - keep nodes that intersect the curve cache
                    (Some(cc), _) => cc
                        .tree()
                        .ok_or_else(|| invalid(OP_EU, "curve cache has no tree"))?
                        .intersects_box(&node_bbox)?,
                    // when working with lines - skip nodes too far from line
                    (None, Some((line_point, line_vec))) => {
                        // First do bounding box check (uv).
                        let mut keep =
                            !curve_bbox.are_disjoint(&Extent2d::from_extent3d(&node_bbox), 0.0);
                        // BBoxes intersect, check against line.
                        if keep {
                            keep = node_dom
                                .intersect_with_infinite_line(line_point, line_vec, None, None)
                                .is_some();
                        }
                        keep
                    }
                    (None, None) => false,
                };

                // also keep nodes near poles
                let mut keep_for_pole = false;
                if poles != 0 && !keep_for_position {
                    keep_for_pole = ((poles & SS_UMIN) != 0
                        && (node_dom.min().x - u_ivl.min()).abs() < u_tol)
                        || ((poles & SS_UMAX) != 0
                            && (node_dom.max().x - u_ivl.max()).abs() < u_tol)
                        || ((poles & SS_VMIN) != 0
                            && (node_dom.min().y - v_ivl.min()).abs() < v_tol)
                        || ((poles & SS_VMAX) != 0
                            && (node_dom.max().y - v_ivl.max()).abs() < v_tol);
                }

                // skip nodes too far away from this node's bounding box
                if !keep_for_position && !keep_for_pole {
                    continue;
                }

                // When not doing subdividing, when node is small enough
                let uv_diff = node_dom.size();
                if !do_subdivision
                    || uv_diff.length_squared() < min_node_size_squared
                    || uv_diff.x < min_size.x
                    || uv_diff.y < min_size.y
                {
                    // mark the AuxData as on the boundary, add the Edgeuse to the
                    // AuxData's curve list, propagate state change to ancestors
                    self.set_on_boundary(node, true)?;
                    self.aux_mut(node)?.edgeuse_list.insert(0, edgeuse);
                    self.propagate(node)?;
                } else {
                    // subdivide the node's largest UV direction
                    let dir = if uv_diff.x > uv_diff.y * scale {
                        SurfParamType::U
                    } else {
                        SurfParamType::V
                    };

                    // promote leaf node into a parent->Child(1,2) triplet
                    let (c1, c2) = self.subdivide_node(k, node, dir)?;

                    // put the children on the stack
                    stack.push(c1);
                    stack.push(c2);
                }
            }
        }

        // Update the domain of the face extents
        if face_extent.is_init() || face_extent.has_negative_area() {
            *face_extent = curve_bbox;
        } else {
            *face_extent = curve_bbox.union(face_extent);
        }

        // all done
        Ok(())
    }

    /// `SmSurfaceCache::SubdivideNode(node, dir, TRUE, NULL)` on this cache: the two new
    /// children. A parent on the boundary moves its edge uses into them through the
    /// trimmed surface cache's `ImplantEdgeuse(pEU, FALSE, sTempExt, pNode)` (virtual
    /// upstream), last first, and its poly edges through `ImplantPolyEdge`.
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceCache::subdivide_node_split`]. `InvalidInput` for a polygon
    /// edge handle inserted directly into the base lists without its geometry. An
    /// edge use's `GetUVTrimCurve` is the curve it was implanted with.
    fn subdivide_node(
        &mut self,
        k: &Kernel,
        node: NodeId,
        dir: SurfParamType,
    ) -> Result<(NodeId, NodeId)> {
        const OP: &str = "surface::trim_srf_cache::TrimSrfCache::subdivide_node";
        if let Some((node1, node2)) = self.base.subdivide_node_split(k, node, dir, true)? {
            if self.base.aux(node)?.poly_edge_list.is_empty() {
                // for every edge use in the parent node, last first
                while let Some(eu) = self.base.aux(node)?.edgeuse_list.last().copied() {
                    // implant the edge use in both children without subsequent
                    // subdivision (the status is ignored)
                    let index = self.edgeuse_curves.iter().position(|(e, _)| *e == eu);
                    for child in [node1, node2] {
                        let mut temp_ext = Extent2d::new();
                        let r = if let Some(index) = index {
                            self.implant_retained_edgeuse(
                                k,
                                index,
                                false,
                                &mut temp_ext,
                                Some(child),
                            )
                        } else {
                            Err(SolidError::null_pointer(OP_EU))
                        };
                        if let Err(e) = r {
                            if e.is_unported() {
                                return Err(e);
                            }
                        }
                    }
                    self.base.aux_mut(node)?.edgeuse_list.pop();
                }
            } else {
                // Native GetAllNodes visits head first and leaves the parent's list
                // intact. Each child's Prepend reverses that order independently.
                let poly_edges = self.base.aux(node)?.poly_edge_list.clone();
                for pe in poly_edges {
                    for child in [node1, node2] {
                        let geometry = self
                            .poly_edge_geometry
                            .iter()
                            .find(|(handle, _)| *handle == pe)
                            .map(|(_, geometry)| geometry)
                            .ok_or_else(|| invalid(OP, "polygon edge has no retained geometry"))?;
                        let domain = self.base.aux(child)?.uv_domain;
                        // The native caller ignores ordinary implantation errors.
                        let embed = match poly_edge_in_domain(k, &geometry.input(pe), domain) {
                            Ok(embed) => embed,
                            Err(e) if e.is_unported() => return Err(e),
                            Err(_) => continue,
                        };
                        if embed {
                            self.set_on_boundary(child, false)?;
                            self.aux_mut(child)?.poly_edge_list.insert(0, pe);
                        }
                    }
                }
            }
        }
        let n = self
            .base
            .tree()
            .ok_or(SolidError::InvalidInput {
                operation: OP,
                reason: "cache has no tree",
            })?
            .node(node)?;
        match (n.child1, n.child2) {
            (Some(c1), Some(c2)) => Ok((c1, c2)),
            _ => Err(SolidError::InvalidInput {
                operation: OP,
                reason: "split node has no children",
            }),
        }
    }

    /// Embeds a poly edge segment into `node` (`ImplantPolyEdge`): when the node's
    /// domain, enlarged by ten times the parametric tolerances, holds an end point,
    /// or the segment's line crosses the domain within the segment, the node is put
    /// on the boundary (neither marked nor propagated) and gets the poly edge.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a node without surface data; errors from tolerance mapping
    /// or from making the bounded evaluation copy of the original surface.
    pub fn implant_poly_edge(
        &mut self,
        k: &Kernel,
        input: PolyEdgeInput<'_, O>,
        node: NodeId,
    ) -> Result<()> {
        let domain = self.base.aux(node)?.uv_domain;
        let embed = poly_edge_in_domain(k, &input, domain)?;
        // An existing handle can occur in other nodes even when this particular
        // implantation misses. Those nodes must also see the caller's latest input.
        if embed
            || self
                .poly_edge_geometry
                .iter()
                .any(|(handle, _)| *handle == input.poly_edge)
        {
            let geometry = PolyEdgeGeometry {
                start: input.start,
                end: input.end,
                tolerance: input.tolerance,
                surface: input.surface.clone_for_evaluation()?,
            };
            if let Some((_, retained)) = self
                .poly_edge_geometry
                .iter_mut()
                .find(|(handle, _)| *handle == input.poly_edge)
            {
                *retained = geometry;
            } else {
                self.poly_edge_geometry.push((input.poly_edge, geometry));
            }
        }
        if embed {
            self.set_on_boundary(node, false)?;
            self.aux_mut(node)?
                .poly_edge_list
                .insert(0, input.poly_edge);
        }
        Ok(())
    }

    /// Embeds a vertex use into the decomposition (`ImplantVertexuse`): every leaf
    /// meeting the point's parametric tolerance box (widened to a whole pole side) is
    /// marked on the boundary and gets the vertex use; `face_extent` grows by the box.
    ///
    /// # Errors
    ///
    /// Those of the tree, of `Evaluate1stDerivatives` and of
    /// [`SurfaceCache::find_uv_nodes`] and `GetSingularities`.
    pub fn implant_vertexuse(
        &mut self,
        k: &Kernel,
        input: VertexuseInput<O>,
        face_extent: &mut Extent2d,
    ) -> Result<()> {
        let _ = k;
        // locals: UVPoint, Vertex, 3D Surface Properties
        let uv = input.uv_point;
        let (_pnt, du, dv) = evaluate_1st_derivatives(k, self.base.surface(), uv)?;

        // get surface u and v speeds
        let mut du_len = du.length();
        let mut dv_len = dv.length();

        // keep the parameter space tolerance from getting too long and skinny
        if du_len < dv_len / 10.0 {
            du_len = dv_len;
        }
        if dv_len < du_len / 10.0 {
            dv_len = du_len;
        }

        // pick u and v tolerance
        let mut uv_tol = Vec2::new(
            input.vertex_tolerance / du_len,
            input.vertex_tolerance / dv_len,
        );

        // expand the UV tolerances when near surface poles
        let uv_domain = self.base.surface().natural_uv_domain_with_nurb(k)?;
        let u_ivl = uv_domain.u_interval();
        let v_ivl = uv_domain.v_interval();
        let u_tol = u_ivl.length() / 100.0;
        let v_tol = v_ivl.length() / 100.0;

        // Get boundaries close to sUV
        let mut poles = u64::from((uv.x - u_ivl.min()).abs() < u_tol)
            + (if (uv.y - v_ivl.min()).abs() < v_tol {
                2
            } else {
                0
            })
            + (if (uv.x - u_ivl.max()).abs() < u_tol {
                4
            } else {
                0
            })
            + (if (uv.y - v_ivl.max()).abs() < v_tol {
                8
            } else {
                0
            });

        // when sUV is close enough to boundaries for a pole check
        if poles != SS_NONE {
            // only check those boundaries which are poles
            poles &= surface_singularities(k, self.base.surface())?;

            // when UV point is in constant U singular side
            if (poles & SS_UMIN) != 0 || (poles & SS_UMAX) != 0 {
                uv_tol.y = v_ivl.length();
            }

            // when UV point is in constant V singular side
            if (poles & SS_VMIN) != 0 || (poles & SS_VMAX) != 0 {
                uv_tol.x = u_ivl.length();
            }
        }

        // set point extent to include the point and its tolerance
        let uv_ext = Extent2d::from_points(uv - uv_tol, uv + uv_tol);

        // get all nodes that intersect the point extent
        let nodes = find_uv_nodes(&mut self.base, k, &uv_ext, false)?;

        // for every intersecting node
        for node in nodes {
            // skip non-leaf nodes
            let tree = self
                .base
                .tree()
                .ok_or_else(|| invalid(OP_VU, "cache has no tree"))?;
            if tree.node(node)?.child1.is_some() {
                continue;
            }

            // mark node as on boundary, add vertexuse to node's vertex list,
            // propagate state change to ancestors
            self.set_on_boundary(node, true)?;
            self.aux_mut(node)?.vertex_list.insert(
                0,
                sc::VertexListItem {
                    vertexuse: Some(input.vertexuse),
                    uv_point: uv,
                    uv_tol,
                },
            );
            self.propagate(node)?;
        }

        // Update the domain of the face extents
        let size = face_extent.size();
        if size.x < 0.0 || size.y < 0.0 {
            *face_extent = uv_ext;
        } else {
            *face_extent = uv_ext.union(face_extent);
        }

        // all done
        Ok(())
    }
}

// ---- shared construction and geometry operations ------------------------------------

/// `SmSurfaceCache::BuildTree(&sBezierBlock)` (base class tree construction for a derived
/// cache, which finishes it).
fn surface_cache_build_tree<O: Copy + PartialEq>(
    cache: &mut SurfaceCache<O>,
    k: &Kernel,
) -> Result<()> {
    cache.build_tree(k, true)
}

/// `SmSurfaceCache::FindUVNodes`.
fn find_uv_nodes<O: Copy + PartialEq>(
    cache: &mut SurfaceCache<O>,
    k: &Kernel,
    uv_box: &Extent2d,
    near_poles: bool,
) -> Result<Vec<NodeId>> {
    cache.find_uv_nodes(k, uv_box, near_poles)
}

/// `SmSurface::GetSingularities`: the `SM_SS_*` sides that are poles.
fn surface_singularities(k: &Kernel, surface: &Surface) -> Result<u64> {
    Ok(u64::from(
        surface.get_singularities(k, false, false)?.singular,
    ))
}

/// `SmSurface::Evaluate1stDerivatives(uv, TRUE, TRUE, ..)`: point, `dS/du`, `dS/dv`.
///
/// When wiring, keep upstream's flag arguments exactly: `(uv, TRUE, TRUE, ..)`
/// (`SmTrimSrfCache.cpp:856`).
fn evaluate_1st_derivatives(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
) -> Result<(Point3, Vec3, Vec3)> {
    crate::surface::surface::p1::evaluate_1st_derivatives(
        k,
        surface,
        uv,
        crate::nurbs::knots::Side::Left,
        crate::nurbs::knots::Side::Left,
    )
}

/// The geometric test shared by initial polygon-edge implantation and subdivision.
fn poly_edge_in_domain<O>(
    k: &Kernel,
    input: &PolyEdgeInput<'_, O>,
    domain: Extent2d,
) -> Result<bool> {
    // get segment endpoints
    let start = Point2::new(input.start.x, input.start.y);
    let end = Point2::new(input.end.x, input.end.y);

    // Tolerance: get uv space tols, expand the uv domain of the node by tols
    let tol_3d = input.tolerance;
    let mid_uv = (start + end) / 2.0;
    let tol_u = map_to_2d(k, tol_3d, mid_uv, Vec2::new(1.0, 0.0), input.surface)?;
    let tol_v = map_to_2d(k, tol_3d, mid_uv, Vec2::new(0.0, 1.0), input.surface)?;
    let tol_expansion = Vec2::new(tol_u, tol_v);

    let mut domain_exp = domain;
    domain_exp.expand_absolute_vec(tol_expansion * 10.0);

    // locals for intersection test with node domain
    let line_vec = end - start;
    let is_degenerate = line_vec.x.abs() <= tol_u && line_vec.y.abs() <= tol_v;
    let unit = Extent1d::from_min_max(0.0, 1.0);

    // testing to see if node contains the start/end points of PolyEdge or
    // PolyEdge intersects the domain
    let mut embed = true;
    if !domain_exp.contains_point(start, 0.0) && !domain_exp.contains_point(end, 0.0) {
        if is_degenerate {
            embed = false;
        } else {
            match domain.intersect_with_infinite_line(start, line_vec, None, None) {
                None => embed = false,
                Some(ivl) => {
                    if ivl.intersect(&unit).is_err() {
                        embed = false;
                    }
                }
            }
        }
    }

    Ok(embed)
}

/// `SmTol::MapTo2d(tol, uv, dir, surface)`.
fn map_to_2d(k: &Kernel, tol_3d: f64, uv: Point2, dir: Vec2, surface: &Surface) -> Result<f64> {
    crate::surface::surface::p1::map_to_2d_dir(k, tol_3d, uv, dir, surface)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EntityRef;
    use crate::geom::tree::{
        AuxDataType, NodeGeomType, SplitDir, SurfaceAux, Tree, TreeNode, TreeOwner,
    };
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::crv_basic::Curve as NlCurve;
    use crate::nurbs::math::{CPoint, NOW, NOZ};
    use crate::nurbs::srf_basic::Surface as NlSurface;
    use crate::query::face::p2::BrepCacheFace;
    use crate::surface::surface_cache::face_context::MappedCacheFace;

    fn plane() -> Surface {
        let net = (0..2)
            .map(|i| {
                (0..2)
                    .map(|j| CPoint::new(f64::from(i), f64::from(j), 0.0, NOW))
                    .collect()
            })
            .collect();
        let s = NlSurface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .expect("surface");
        Surface::from(BSplineSurface::from_nurb(s, false))
    }

    fn line_2d(a: [f64; 2], b: [f64; 2]) -> BSplineCurve {
        let pw = vec![
            CPoint::new(a[0], a[1], NOZ, NOW),
            CPoint::new(b[0], b[1], NOZ, NOW),
        ];
        BSplineCurve::from_nurb(
            NlCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).expect("curve"),
            2,
            false,
        )
    }

    /// A cache over [0.2, 0.8]^2 whose tree is the root and two halves split in u.
    fn cache() -> TrimSrfCache<i64> {
        let mut c = TrimSrfCache::new(plane(), SurfaceCacheOptions::default());
        c.base_mut()
            .set_uv_domain(Extent2d::from_bounds(0.2, 0.2, 0.8, 0.8));
        let doms = [
            Extent2d::from_bounds(0.2, 0.2, 0.8, 0.8),
            Extent2d::from_bounds(0.2, 0.2, 0.5, 0.8),
            Extent2d::from_bounds(0.5, 0.2, 0.8, 0.8),
        ];
        let links = [
            (None, Some(1), Some(2)),
            (Some(0), None, None),
            (Some(0), None, None),
        ];
        let mut tree: Tree<i64> = Tree::new(Some(TreeOwner::SurfaceCache));
        let mut auxs = Vec::new();
        for (k, (d, (p, c1, c2))) in doms.iter().zip(links).enumerate() {
            auxs.push(BezierAux2d {
                uv_domain: *d,
                owning_tree_node: Some(NodeId(k)),
                ..BezierAux2d::default()
            });
            tree.add_node(TreeNode {
                parent: p.map(NodeId),
                child1: c1.map(NodeId),
                child2: c2.map(NodeId),
                aux_type: AuxDataType::AuxData,
                data: NodeData::Surface(SurfaceAux {
                    split_dir: SplitDir::U,
                    is_marked: false,
                    node_class: NodeClass::Unknown,
                    face: None,
                    surface: None,
                    slot: Some(k),
                }),
                geom_type: NodeGeomType::Default,
                ..TreeNode::new()
            });
        }
        tree.set_top_node(Some(NodeId(0)));
        *c.base_mut().auxs_mut() = auxs;
        c.base_mut().set_tree(Some(tree));
        c
    }

    #[test]
    fn constructor_state() {
        let c: TrimSrfCache<i64> = TrimSrfCache::new(plane(), SurfaceCacheOptions::default());
        assert!(c.base().point_test_enabled);
        assert!(!c.base().process_boundary_curves);
        assert_eq!(c.type_code(), TYPE);
        assert!(c.is_kind_of(TYPE) && c.is_kind_of(sc::TYPE));
        assert!(!c.is_kind_of(16002));
    }

    #[test]
    fn point_test_off() {
        let k = Kernel::default();
        let mut c: TrimSrfCache<i64> = TrimSrfCache::new(plane(), SurfaceCacheOptions::default());
        c.base_mut().point_test_enabled = false;
        assert_eq!(
            c.point_test(&k, Point2::new(5.0, 5.0), None).unwrap(),
            (true, None)
        );
        c.base_mut().point_test_enabled = true;
        // no face to classify against (SM_ERR_NULL_POINTER)
        assert!(matches!(
            c.point_test(&k, Point2::new(5.0, 5.0), None),
            Err(SolidError::Upstream { code: 1006, .. })
        ));
    }

    #[test]
    fn implant_line_in_leaf() {
        let k = Kernel::default();
        let mut c = cache();
        let mut ext = Extent2d::new();
        let crv = line_2d([0.3, 0.3], [0.4, 0.7]);
        c.implant_edgeuse(
            &k,
            EdgeuseInput {
                edgeuse: 7,
                uv_trim_curve: Some(crv.clone_exact()),
                owner_zone_tol_3d: None,
            },
            false,
            &mut ext,
            Some(NodeId(1)),
        )
        .unwrap();
        assert_eq!(c.base().aux(NodeId(1)).unwrap().edgeuse_list, vec![7]);
        // the other leaf is not visited, the root is not a leaf
        c.implant_edgeuse(
            &k,
            EdgeuseInput {
                edgeuse: 8,
                uv_trim_curve: Some(crv),
                owner_zone_tol_3d: None,
            },
            false,
            &mut ext,
            Some(NodeId(2)),
        )
        .unwrap();
        assert!(c.base().aux(NodeId(2)).unwrap().edgeuse_list.is_empty());
        assert!(ext.min().x < 0.3 && ext.max().y > 0.7);
        // without trim curve
        assert!(c
            .implant_edgeuse(
                &k,
                EdgeuseInput {
                    edgeuse: 9,
                    uv_trim_curve: None,
                    owner_zone_tol_3d: None,
                },
                false,
                &mut ext,
                Some(NodeId(1)),
            )
            .is_err());
    }

    #[test]
    fn invalid_node_returns_error_without_changing_boundary() {
        let k = Kernel::default();
        let mut c = cache();
        let crv = line_2d([0.3, 0.3], [0.4, 0.7]);
        let mut ext = Extent2d::new();
        assert!(c
            .implant_edgeuse(
                &k,
                EdgeuseInput {
                    edgeuse: 7,
                    uv_trim_curve: Some(crv),
                    owner_zone_tol_3d: None,
                },
                false,
                &mut ext,
                Some(NodeId(usize::MAX)),
            )
            .is_err());
        assert!(c.base().aux(NodeId(1)).unwrap().edgeuse_list.is_empty());
        assert!(ext.is_init());
    }

    #[test]
    fn implant_vertexuse_away_from_poles_marks_the_leaf() {
        let k = Kernel::default();
        let mut c = cache();
        let mut ext = Extent2d::new();
        let vu = VertexuseInput {
            vertexuse: 7,
            uv_point: Point2::new(0.4, 0.4),
            vertex_tolerance: 0.001,
        };
        // away from the domain sides no pole check (GetSingularities) is made
        c.implant_vertexuse(&k, vu, &mut ext).unwrap();
        assert_eq!(ext, Extent2d::from_bounds(0.399, 0.399, 0.401, 0.401));
        let leaf = c.base().aux(NodeId(1)).unwrap();
        assert_eq!(leaf.vertex_list.len(), 1);
        assert_eq!(leaf.vertex_list[0].vertexuse, Some(7));
        assert!(c.base().aux(NodeId(2)).unwrap().vertex_list.is_empty());
    }

    #[test]
    fn implant_poly_edge_marks_the_leaf() {
        let k = &Kernel::default();
        let mut c = cache();
        let surface = plane();
        let edge = PolyEdgeInput {
            poly_edge: 8,
            start: Point3::new(0.3, 0.3, 0.0),
            end: Point3::new(0.4, 0.7, 0.0),
            tolerance: 0.001,
            surface: &surface,
        };
        // (SmTol::MapTo2d is ported with surface.surface_p1)
        c.implant_poly_edge(k, edge, NodeId(1)).unwrap();
        let aux = c.base().aux(NodeId(1)).unwrap();
        assert_eq!((aux.poly_edge_list.len(), aux.vertex_list.len()), (1, 0));
    }

    #[test]
    fn subdivision_preserves_parent_polygons_and_reverses_child_list_order() {
        let k = Kernel::default();
        let mut c = cache();
        let surface = plane();
        for poly_edge in [7, 8] {
            c.implant_poly_edge(
                &k,
                PolyEdgeInput {
                    poly_edge,
                    start: Point3::new(0.21, 0.4, 0.0),
                    end: Point3::new(0.49, 0.4, 0.0),
                    tolerance: 0.001,
                    surface: &surface,
                },
                NodeId(1),
            )
            .unwrap();
        }
        // Map the retained geometry as well as the node lists, then drop the input
        // surface. Subdivision must still resolve every polygon's original surface.
        let mut c = c.map_handles(|h| Ok(h + 100)).unwrap();
        drop(surface);
        let (left, right) = c.subdivide_node(&k, NodeId(1), SurfParamType::U).unwrap();
        assert_eq!(c.base().aux(NodeId(1)).unwrap().poly_edge_list, [108, 107]);
        for child in [left, right] {
            assert_eq!(c.base().aux(child).unwrap().poly_edge_list, [107, 108]);
        }
        let (lower, upper) = c.subdivide_node(&k, left, SurfParamType::V).unwrap();
        assert_eq!(c.base().aux(lower).unwrap().poly_edge_list, [108, 107]);
        assert!(c.base().aux(upper).unwrap().poly_edge_list.is_empty());
    }

    #[test]
    fn reimplanting_an_edited_polygon_updates_the_retained_geometry() {
        let k = Kernel::default();
        let mut c = cache();
        let surface = plane();
        for (x, node) in [(0.22, NodeId(1)), (0.48, NodeId(2))] {
            c.implant_poly_edge(
                &k,
                PolyEdgeInput {
                    poly_edge: 8,
                    start: Point3::new(x, 0.3, 0.0),
                    end: Point3::new(x, 0.7, 0.0),
                    tolerance: 0.001,
                    surface: &surface,
                },
                node,
            )
            .unwrap();
        }
        let (left, right) = c.subdivide_node(&k, NodeId(1), SurfParamType::U).unwrap();
        assert!(c.base().aux(left).unwrap().poly_edge_list.is_empty());
        assert_eq!(c.base().aux(right).unwrap().poly_edge_list, [8]);
    }

    #[test]
    fn build_tree_without_face_restores_state() {
        let k = Kernel::default();
        let mut c: TrimSrfCache<i64> = TrimSrfCache::new(plane(), SurfaceCacheOptions::default());
        c.build_tree(&k).unwrap();
        assert_eq!(c.base().face_containment_done, FaceContainment::NotDone);
    }

    fn face_key(entity: EntityRef) -> ObjectRef {
        let EntityRef::Face(face) = entity else {
            panic!("test only maps faces");
        };
        ObjectRef {
            type_code: crate::topology::face::p2::TYPE,
            key: crate::geom::attribute::ObjectKey(10_000 + u64::from(face.index())),
        }
    }

    fn face_handle(entity: EntityRef) -> Result<i64> {
        let EntityRef::Face(face) = entity else {
            return Err(invalid(OP_BUILD, "test only maps faces"));
        };
        Ok(i64::from(face.index()) + 100)
    }

    fn bare_owned_face() -> (crate::topology::Brep, crate::topology::FaceId) {
        use crate::topology::{Brep, Face, ShellId};
        let mut brep = Brep::new();
        let surface = plane();
        let mut face = Face::new(ShellId::from_index(0));
        face.uv_domain = surface.natural_uv_domain_if_built();
        face.surface = Some(surface);
        face.tolerance = 2.5e-4;
        let face = brep.add_face_record(face).unwrap();
        brep.face_mut(face)
            .unwrap()
            .surface
            .as_mut()
            .unwrap()
            .set_owner(Some(face_key(EntityRef::Face(face))));
        brep.sync_face_surface_tolerance(face).unwrap();
        (brep, face)
    }

    #[derive(Debug)]
    struct Outside;

    impl FaceQueries<i64> for Outside {
        fn face_zone_tol_3d(&self, _face: i64) -> Result<f64> {
            Ok(2.5e-4)
        }

        fn face_point_classify(
            &self,
            _k: &Kernel,
            _face: i64,
            _uv: Point2,
            _tol: f64,
        ) -> Result<(i32, Option<i64>)> {
            Ok((POINT_CLASS_UNKNOWN, None))
        }

        fn face_point_classify_fast_drop(
            &self,
            k: &Kernel,
            face: i64,
            uv: Point2,
            src_tol: f64,
        ) -> Result<(i32, Option<i64>)> {
            self.face_point_classify(k, face, uv, src_tol)
        }
    }

    #[test]
    fn bound_trim_preserves_dispatch_mapping_and_face_tolerance() {
        let k = Kernel::default();
        let (mut brep, face) = bare_owned_face();
        brep.set_editing_enabled(true);
        let surface = brep
            .face(face)
            .unwrap()
            .surface
            .as_ref()
            .unwrap()
            .clone_for_evaluation()
            .unwrap();
        let mut cache = TrimSrfCache::new(surface, SurfaceCacheOptions::default());
        cache
            .bind_face(
                face_key(EntityRef::Face(face)),
                face_handle(EntityRef::Face(face)).unwrap(),
                2.5e-4,
            )
            .unwrap();
        let entity_of = |handle| {
            if handle == face_handle(EntityRef::Face(face))? {
                Ok(EntityRef::Face(face))
            } else {
                Err(invalid(OP_BUILD, "unknown test handle"))
            }
        };
        cache
            .build_tree_with_face(
                &k,
                &mut MappedCacheFace::new(
                    BrepCacheFace {
                        b: &mut brep,
                        f: face,
                    },
                    &face_handle,
                    &entity_of,
                )
                .unwrap(),
            )
            .unwrap();
        assert!(brep.editing_enabled());
        assert_eq!(cache.base().face().unwrap(), Some(100));
        assert_eq!(cache.base().surface().owner_zone_tol_3d(), Some(2.5e-4));

        let mut base = cache.into_base();
        base.point_test_enabled = true;
        let uv = Point2::new(0.5, 0.5);
        assert!(base.uv_domain().contains_point(uv, 0.0));
        assert!(base.point_test(uv).unwrap_err().is_unported());
        assert!(base
            .point_test_with(&k, uv, None, None)
            .unwrap_err()
            .is_unported());
        assert_eq!(
            base.point_test_with(&k, uv, None, Some(&Outside)).unwrap(),
            (false, None)
        );

        let mapped = base.map_handles(|handle| Ok(handle + 1_000)).unwrap();
        assert!(mapped.is_trimmed());
        assert_eq!(mapped.face().unwrap(), Some(1_100));
        assert_eq!(mapped.surface().owner(), base.surface().owner());
        assert_eq!(mapped.surface().owner_zone_tol_3d(), Some(2.5e-4));
        assert!(base
            .map_handles::<i64>(|_| Err(invalid(OP_BUILD, "unmappable")))
            .is_err());

        base.face_was_modified = true;
        assert_eq!(
            base.point_test_with(&k, uv, None, None).unwrap(),
            (true, None)
        );
        base.face_was_modified = false;
        base.point_test_enabled = false;
        assert_eq!(
            base.point_test_with(&k, uv, None, None).unwrap(),
            (true, None)
        );
    }

    #[test]
    fn mapped_face_checks_both_directions_and_delegates_domain_queries() {
        let (mut brep, face) = bare_owned_face();
        let other_face = crate::topology::FaceId::from_index(99);
        let wrong_entity = |_| Ok(EntityRef::Face(other_face));
        assert!(MappedCacheFace::new(
            BrepCacheFace {
                b: &mut brep,
                f: face
            },
            &face_handle,
            &wrong_entity,
        )
        .is_err());

        let entity_of = |_| Ok(EntityRef::Face(face));
        let mapped = MappedCacheFace::new(
            BrepCacheFace {
                b: &mut brep,
                f: face,
            },
            &face_handle,
            &entity_of,
        )
        .unwrap();
        assert_eq!(mapped.face(), 100);
        assert_eq!(
            mapped.natural_uv_domain().unwrap(),
            Some(plane().natural_uv_domain_if_built())
        );
        assert_eq!(mapped.zone_tol_3d(&Kernel::default(), 100).unwrap(), 2.5e-4);
        // Even if the reverse mapping accepts this alias, the forward mapping
        // must name the same handle. No object key or index is decoded here.
        assert!(mapped.zone_tol_3d(&Kernel::default(), 101).is_err());
    }

    #[test]
    fn base_fallback_keeps_untrimmed_dispatch() {
        let surface = plane();
        let domain = surface.natural_uv_domain_if_built();
        let mut base: SurfaceCache<i64> =
            SurfaceCache::new(surface, domain, SurfaceCacheOptions::default());
        base.point_test_enabled = true;
        let cache = TrimSrfCache::from_base(base);
        assert!(!cache.base().is_trimmed());
        assert_eq!(
            cache
                .point_test(&Kernel::default(), Point2::new(0.5, 0.5), None)
                .unwrap(),
            (true, None)
        );
        assert_eq!(
            cache
                .point_test(&Kernel::default(), Point2::new(2.0, 2.0), None)
                .unwrap(),
            (false, None)
        );
        assert!(!cache.into_base().is_trimmed());
    }

    #[test]
    fn face_build_restores_guards_when_real_topology_is_incomplete() {
        let k = Kernel::default();
        let (mut brep, face) = bare_owned_face();
        let surface = brep
            .face(face)
            .unwrap()
            .surface
            .as_ref()
            .unwrap()
            .clone_for_evaluation()
            .unwrap();
        let mut cache = TrimSrfCache::new(surface, SurfaceCacheOptions::default());
        cache
            .bind_face(
                face_key(EntityRef::Face(face)),
                EntityRef::Face(face),
                2.5e-4,
            )
            .unwrap();
        assert!(cache
            .build_tree_with_face(
                &k,
                &mut BrepCacheFace {
                    b: &mut brep,
                    f: face
                },
            )
            .is_err());
        assert!(!brep.editing_enabled());
        assert!(cache.base().point_test_enabled);
        assert_eq!(cache.base().face_containment_done, FaceContainment::NotDone);
        assert!(!cache.base().have_t_surface_cache);
        assert!(cache.edgeuse_curves.is_empty());
        assert!(cache.base().patches().iter().all(|p| p.bezier.is_none()));
        assert!(cache.base_mut().subdivision_surfaces_mut().is_empty());
    }

    #[test]
    fn binding_rejects_a_different_owner_key_before_mutating_state() {
        let (brep, face) = bare_owned_face();
        let surface = brep
            .face(face)
            .unwrap()
            .surface
            .as_ref()
            .unwrap()
            .clone_for_evaluation()
            .unwrap();
        let mut cache = TrimSrfCache::new(surface, SurfaceCacheOptions::default());
        let mut wrong_key = face_key(EntityRef::Face(face));
        wrong_key.key.0 += 1;
        assert!(matches!(
            cache.bind_face(wrong_key, EntityRef::Face(face), 2.5e-4),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(!brep.editing_enabled());
        assert!(cache.base().tree().is_none());
        assert_eq!(cache.base().face_containment_done, FaceContainment::NotDone);
    }

    #[test]
    fn standalone_trimmed_cache_can_build_lazily_through_its_base() {
        let k = Kernel::default();
        let cache: TrimSrfCache<i64> = TrimSrfCache::new(plane(), SurfaceCacheOptions::default());
        let mut base = cache.into_base();
        assert!(base.tree().is_none());
        base.get_tree(&k).unwrap();
        assert!(base.is_trimmed());
        assert!(base.process_boundary_curves);
        assert!(!base.point_test_enabled);
        assert!(!base.have_t_surface_cache);
        assert_eq!(base.face_containment_done, FaceContainment::NotDone);
    }
}
