//! B-rep bounding boxes and point classification, the polygon decimation attributes,
//! and the checked surface evaluation entry points of `SM_API`.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`,
//! `source/SM_API/src/SmApiSurfaces.cpp`, `source/SMLib/inc/SmPolyDecimate.h`,
//! `source/SMLib/src/SmPolyDecimate.cpp`.
//!
//! | Rust                                         | Upstream                                       |
//! |----------------------------------------------|------------------------------------------------|
//! | [`calculate_bounding_box`], [`BoundingBoxOptions`] | `SmBrep::CalculateBoundingBox`           |
//! | [`calculate_tight_bounding_box`]             | `SmBrep::CalculateTightBoundingBox`            |
//! | [`lock_bounding_box`]                        | `SmBrep::LockBoundingBox`                      |
//! | [`unlock_bounding_box`]                      | `SmBrep::UnLockBoundingBox`                    |
//! | [`BrepQuery::point_3d_classify`]             | `SmBrep::Point3DClassify`                      |
//! | [`BrepQuery`], [`BrepIdentity`]              | the B-rep's `SM_OC_BREP` cache (`CacheMakeOrValidate`) and pointer identities |
//! | [`LazyBrep`]                                 | the `this` of the `const` box methods, which may edit the B-rep |
//! | [`BrepClassifier`]                           | `Point3DClassify` on a B-rep held `const` (its lazily made UV trim curves on a copy) |
//! | [`evaluate_surface_point`]                   | `SmApiEvaluateSurfacePoint`                    |
//! | [`evaluate_surface_normal`]                  | `SmApiEvaluateSurfaceNormal`                   |
//! | [`evaluate_surface_derivatives`]             | `SmApiEvaluateSurfaceDerivatives`              |
//! | [`PolyVertexClass`]                          | `SmPolyVertexClass` (`SmPoly.h`)               |
//! | [`decimate_vertex_attribute`], [`DecimateVertexData`] | `SmDecimateVertexAttr::SmDecimateVertexAttr` (`~` is `Drop`) |
//! | [`DecimateVertexData::make_copy`]            | `SmDecimateVertexAttr::MakeCopy`               |
//! | [`quadric_vertex_attribute`], [`QuadricVertexData`] | `SmQuadricVertexAttr::SmQuadricVertexAttr` (`~` is `Drop`) |
//! | [`Attribute::make_copy`] (data cloned)       | `SmQuadricVertexAttr::MakeCopy`                |
//! | [`quadric_edge_attribute`], [`QuadricEdgeData`] | `SmQuadricEdgeAttr::SmQuadricEdgeAttr` (`~` is `Drop`) |
//! | [`QuadricEdgeData::make_copy`]               | `SmQuadricEdgeAttr::MakeCopy`                  |
//!
//! Deviations:
//! - **Tolerances.** Upstream is built with `SM_USE_OLDTOL`, so the zone tolerance
//!   expansions use `GetTolerance()` of the vertex, edge and face (not
//!   `SmTol::GetZoneTol3d`).
//! - **Locked box.** `m_pLockedBoundingBox` is [`Brep::locked_bounding_box`], an
//!   `Option` instead of a heap pointer.
//! - **`const` boxes that edit.** A tight box classifies points on faces, which may
//!   attach missing UV trim curves and raise edge tolerances (upstream does this to
//!   the B-rep from its `const` methods). [`calculate_bounding_box`] and
//!   [`calculate_tight_bounding_box`] take a [`LazyBrep`]: pass `&mut Brep` for the
//!   upstream behaviour; with `&Brep` the edits are made on a private copy and lost.
//! - **Ignored statuses.** [`calculate_tight_bounding_box`] ignores the statuses of the
//!   edge and face boxes, as upstream, and keeps what a failed call leaves: the
//!   previous face's box after a failed loose face box (upstream writes it only on
//!   success), the box of the face's vertices and edges after a failure in the
//!   surface part of the tight face box
//!   ([`crate::query::face::p3::calculate_tight_bounding_box_into`]), the loose box
//!   after a failed tight edge box. A failed loose edge box leaves the freshly
//!   initialised (empty) box: every `CalculateBoundingBox` override (`SmCurve`,
//!   `SmBSplineCurve`, `SmLine`, `SmHermiteCurve`, `SmCrvOnSurf`, `SmCompositeCurve`)
//!   fails only before it writes the caller's box. [`SolidError::Unported`] still
//!   propagates.
//! - **Point classification.** [`BrepQuery::point_3d_classify`] returns the
//!   classification instead of filling the caller's (every upstream caller constructs a
//!   fresh one just before and drops it on failure), with [`EntityRef`] objects. Its
//!   boundary solve and ray fires need the B-rep's cache and object identities, which
//!   upstream reaches through the B-rep (`CacheMakeOrValidate`): a [`BrepQuery`] holds
//!   the identities and builds the cache lazily, on the first solve that needs it (so
//!   single-region classification without boundary intersections skips it, as upstream).
//!   A caller keeps one query while
//!   the B-rep is not edited and makes a fresh one after an edit (upstream revalidates).
//!   [`BrepQuery::of_brep`] and [`BrepClassifier::of_brep`] resolve a lone B-rep's
//!   identities from its [`Brep::naming`] scheme and proven owner keys
//!   ([`BrepIdentity::default`] assumes [`arena_identity`]). Face boundary solves prepare
//!   face caches, and ray fires may add missing UV trim curves, which upstream's `const`
//!   method does to the B-rep itself. [`BrepClassifier`] serves callers that hold the
//!   B-rep shared and makes a private copy on the first face boundary solve or ray fire
//!   ([`Brep::clone_standing_for_self`]: its geometry keeps its owners, which the copy
//!   constructor resets, so the copy's caches still hold its curves and surfaces).
//!   A ray fire that fails with an ordinary error moves on to the next direction,
//!   as a failing status does upstream; [`SolidError::Unported`] propagates.
//! - **Out-params.** The evaluated point, normal or point and derivatives are the
//!   return value; on failure there is none (upstream evaluates into locals and leaves
//!   the caller's outputs untouched).
//! - **Pointers.** The `NULL` surface the entry points test for is `None`.
//! - **Statuses.** `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; errors of
//!   the surface evaluations pass through unchanged.
//! - **Decimation attributes.** They are [`Attribute`]s whose [`AttributeValue`] is
//!   the class's data, as for the other attribute classes; `MakeCopy` is
//!   [`Attribute::make_copy`] (same id and behaviour, no users, upstream's `new
//!   (crContext)` has no counterpart). The destructors only overwrite fields of an
//!   object being freed (`~SmDecimateVertexAttr`); `Drop` frees it. `SmQuadricEdgeAttr`'s
//!   constructor leaves `m_lSortIndex` uninitialised: it is `None` until set
//!   ([`QuadricEdgeData::sort_index`]). `SmPolyVertexClass` (`SmPoly.h`) is defined
//!   here, the only place that uses it so far.

mod ray;

pub(crate) use ray::solve_mutable_brep_point;
pub(crate) use ray::PreparationRequest;
pub use ray::RayPreparation;
pub(crate) use ray::RaySurfaceContext;
pub use ray::{
    BrepFaceServices, CurveFaceObject, CurveFaceServices, FaceCaches, LiveBrepObjects, SideObject,
    SidedFaceServices,
};

use core::cell::OnceCell;

use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::attribute::{Attribute, AttributeValue, Behavior};
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::ObjectRef;
use crate::intersect::point_class::{PointClassType, PointClassification};
use crate::intersect::topology_solver::p1::TopologySolver;
use crate::intersect::topology_solver::p2::TopologySolveOptions;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, BIG_DOUBLE};
use crate::nurbs::b_spline_curve::BoundingBoxRequest;
use crate::nurbs::knots::Side;
use crate::query::edge as qe;
use crate::query::face::{p1 as qf1, p3 as qf3};
use crate::surface::surface::Surface;
use crate::topology::brep::{p1 as bp1, p2 as bp2};
use crate::topology::brep_cache::{
    arena_identity, make_brep_cache, BrepCache, BrepObjects, CacheObject, CacheSource,
};
use crate::topology::{edge, face, vertex, Brep, BrepId, NamingScheme, RegionId};

const OP_CLASSIFY: &str = "query::brep::BrepQuery::point_3d_classify";
const OP_EVAL_POINT: &str = "query::brep::evaluate_surface_point";
const OP_EVAL_NORMAL: &str = "query::brep::evaluate_surface_normal";
const OP_EVAL_DERIVS: &str = "query::brep::evaluate_surface_derivatives";

/// The axis aligned box only (`CalculateBoundingBox(&box, NULL, NULL, ..)`).
const NORMAL_BOX: BoundingBoxRequest = BoundingBoxRequest {
    normal: true,
    pseudo: false,
    polar: false,
};

// ---- bounding boxes -----------------------------------------------------------------

/// With [`BoundingBoxOptions::approx`], a B-rep of more vertices than this (and more
/// edges than [`APPROX_EDGES`]) is boxed by its vertices only.
pub const APPROX_VERTICES: usize = 500;
/// See [`APPROX_VERTICES`].
pub const APPROX_EDGES: usize = 100;

/// The arguments of [`calculate_bounding_box`]; [`Default`] gives upstream's default
/// arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundingBoxOptions {
    /// `bApprox` (default `FALSE`): leave out the faces and, on a B-rep with more than
    /// [`APPROX_VERTICES`] vertices and more than [`APPROX_EDGES`] edges, the edges too.
    pub approx: bool,
    /// `bTight` (default `FALSE`): the tight box of every edge and face (expensive).
    pub tight: bool,
    /// `bExpandByZoneTol3d` (default `TRUE`): expand each vertex, edge and face box by
    /// that entity's tolerance.
    pub expand_by_zone_tol_3d: bool,
}

impl Default for BoundingBoxOptions {
    fn default() -> Self {
        Self {
            approx: false,
            tight: false,
            expand_by_zone_tol_3d: true,
        }
    }
}

/// `SmBrep::CalculateBoundingBox`: the locked box when there is one; else the union of
/// the B-rep's vertex points, the boxes of its edges with a curve and, unless
/// `approx`, the boxes of its faces (each expanded by the entity's tolerance with
/// `expand_by_zone_tol_3d`), in list order. With `approx` on a B-rep of more than
/// [`APPROX_VERTICES`] vertices and [`APPROX_EDGES`] edges only the vertices are used.
///
/// # Errors
///
/// Those of the B-rep lists, [`qe::calculate_bounding_box`] and
/// [`qf1::calculate_bounding_box`].
pub fn calculate_bounding_box<'a>(
    k: &Kernel,
    b: impl Into<LazyBrep<'a>>,
    opts: BoundingBoxOptions,
) -> Result<Extent3d> {
    let mut lb = b.into();
    let b = lb.get();
    // when LockedBoundingBox exists - return that value
    if let Some(locked) = b.locked_bounding_box() {
        return Ok(locked);
    }

    let mut brep_bbox = Extent3d::new();

    // add every Brep->Vertex->point to Bounding box
    let vertices = bp1::vertices(b, None)?;

    // for every vertex - union location into BBox
    for &v in &vertices {
        let mut v_bbox = Extent3d::from_point(vertex::point(b, v)?);

        if opts.expand_by_zone_tol_3d {
            v_bbox.expand_absolute(vertex::tolerance(b, v)?); // SM_USE_OLDTOL
        }

        // union vertex bounding box into brep bounding box
        brep_bbox = brep_bbox.union(&v_bbox);
    } // end iter every vertex

    // Union in every Brep->Edge bounding box
    let edges = bp1::edges(b, None)?;

    // when asked - if this is a large brep, settle for a fast approximation
    if opts.approx && vertices.len() > APPROX_VERTICES && edges.len() > APPROX_EDGES {
        return Ok(brep_bbox);
    }

    // for every edge - union together edge BBoxes
    for &e in &edges {
        if b.edge(e)?.curve.is_some() {
            let mut e_bbox = qe::calculate_bounding_box(k, b, e, NORMAL_BOX, opts.tight)?
                .normal
                .unwrap_or_default();

            if opts.expand_by_zone_tol_3d {
                e_bbox.expand_absolute(edge::tolerance(b, e)?); // SM_USE_OLDTOL
            }

            // union edge bounding box into brep bounding box
            brep_bbox = brep_bbox.union(&e_bbox);
        }
    }

    // when asked - union in every Brep->Face bounding box
    if !opts.approx {
        // for every face
        for f in bp1::faces(b, None)? {
            let mut f_bbox = qf1::calculate_bounding_box_in(k, &mut lb, f, opts.tight)?;

            if opts.expand_by_zone_tol_3d {
                f_bbox.expand_absolute(face::p2::tolerance(lb.get(), f)?); // SM_USE_OLDTOL
            }

            // union face bounding box into brep bounding box
            brep_bbox = brep_bbox.union(&f_bbox);
        }
    }

    Ok(brep_bbox)
}

/// `SmBrep::CalculateTightBoundingBox`: the union of the vertex points, then of the
/// tight box of every edge and face whose loose box is not already inside the box so
/// far (the locked box is not used, nothing is expanded). The statuses of the edge and
/// face boxes are ignored (see the module's deviations).
///
/// # Errors
///
/// Those of the B-rep lists and vertex points; [`SolidError::Unported`] from the edge
/// and face boxes.
pub fn calculate_tight_bounding_box<'a>(
    k: &Kernel,
    b: impl Into<LazyBrep<'a>>,
) -> Result<Extent3d> {
    let mut lb = b.into();
    let b = lb.get();
    let mut brep_bbox = Extent3d::new();

    for v in bp1::vertices(b, None)? {
        let v_bbox = Extent3d::from_point(vertex::point(b, v)?);
        brep_bbox = brep_bbox.union(&v_bbox);
    }

    // Union all of the Tight Edge bounding boxes.
    for e in bp1::edges(b, None)? {
        // (CalculateBoundingBox initialises the box first)
        let mut edge_bbox = ignored(qe::calculate_bounding_box(k, b, e, NORMAL_BOX, false))?
            .and_then(|boxes| boxes.normal)
            .unwrap_or_default();
        if edge_bbox.is_contained_by(&brep_bbox, 0.0) {
            continue;
        }
        // (an edge without curve, or a failure, leaves the loose box)
        if let Some(Some(tight)) = ignored(qe::calculate_tight_bounding_box(k, b, e))? {
            edge_bbox = tight;
        }
        brep_bbox = brep_bbox.union(&edge_bbox);
    }

    // Union all of the Tight Face bounding boxes (`rFaceBBox` is kept from face to
    // face).
    let mut face_bbox = Extent3d::new();
    for f in bp1::faces(b, None)? {
        if let Some(loose) = ignored(qf1::calculate_bounding_box(k, lb.get(), f, false))? {
            face_bbox = loose;
        }
        if face_bbox.is_contained_by(&brep_bbox, 0.0) {
            continue;
        }
        ignore_status(qf3::calculate_tight_bounding_box_into(
            k,
            &mut lb,
            f,
            &mut face_bbox,
        ))?;
        brep_bbox = brep_bbox.union(&face_bbox);
    }

    Ok(brep_bbox)
}

/// `SmBrep::LockBoundingBox(bTight)` (upstream default `FALSE`): unlocks the box, then
/// computes it ([`calculate_tight_bounding_box`] with `tight`, else
/// [`calculate_bounding_box`] with the default arguments) and locks it. On failure the
/// box stays unlocked.
///
/// # Errors
///
/// Those of the box computation.
pub fn lock_bounding_box(k: &Kernel, b: &mut Brep, tight: bool) -> Result<()> {
    if b.locked_bounding_box().is_some() {
        unlock_bounding_box(b);
    }

    let bbox = if tight {
        calculate_tight_bounding_box(k, &mut *b)?
    } else {
        calculate_bounding_box(k, &mut *b, BoundingBoxOptions::default())?
    };

    // Okay, now lock it.
    b.set_locked_bounding_box(Some(bbox));
    Ok(())
}

/// `SmBrep::UnLockBoundingBox`: forgets the locked box, if any.
pub fn unlock_bounding_box(b: &mut Brep) {
    b.set_locked_bounding_box(None);
}

// ---- point classification -----------------------------------------------------------

/// The identities a B-rep's cache is keyed by (upstream: the B-rep's own pointers):
/// the B-rep in the caller's shape store, the key of each of its cache objects, and
/// the owners of intermediate geometry (as for `Curve::edge`).
#[derive(Clone, Copy)]
pub struct BrepIdentity<'a> {
    /// The B-rep in the caller's shape store.
    pub brep_id: BrepId,
    /// The identity of each cache object.
    pub keys: &'a dyn Fn(CacheObject) -> ObjectRef,
    /// The owner of an intermediate generated curve or edge use.
    pub owner_of: &'a dyn Fn(ObjectRef) -> Option<ObjectRef>,
}

impl core::fmt::Debug for BrepIdentity<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BrepIdentity")
            .field("brep_id", &self.brep_id)
            .finish_non_exhaustive()
    }
}

/// No intermediate geometry owners.
fn no_owner(_: ObjectRef) -> Option<ObjectRef> {
    None
}

impl BrepIdentity<'static> {
    /// The identities of a B-rep whose geometry is owned by its records'
    /// [`arena_identity`] (such as imported B-reps), without intermediate owners. For a
    /// B-rep whose naming scheme may be [`NamingScheme::Notify`], use [`Self::of_brep`].
    #[must_use]
    pub fn arena(brep_id: BrepId) -> Self {
        Self {
            brep_id,
            keys: &arena_identity,
            owner_of: &no_owner,
        }
    }
}

impl BrepIdentity<'static> {
    /// The base identities of `b` at `brep_id` by its recorded naming scheme alone
    /// ([`Brep::naming`]): the Euler operators' notification identities for a
    /// [`NamingScheme::Notify`] B-rep (one built through `new_brep` and the face
    /// builders), else [`BrepIdentity::arena`]. It ignores the proven owner keys `b`
    /// records (`Brep::geometry_owner_keys`), so it misses the geometry of a copy whose
    /// owners are the caller's keys ([`crate::topology::brep::p3::api_brep_copy`] under
    /// a caller `key_of`): prefer [`Self::of_brep_with_id`].
    #[must_use]
    pub fn of_naming_with_id(brep_id: BrepId, b: &Brep) -> Self {
        match b.naming() {
            NamingScheme::Notify => Self {
                brep_id,
                keys: &notify_identity,
                owner_of: &no_owner,
            },
            _ => Self::arena(brep_id),
        }
    }

    /// The identities `b`'s geometry owners use, at `brep_id`: its naming scheme
    /// ([`Self::of_naming_with_id`]) overridden by the proven producer owner keys it
    /// records (`Brep::geometry_owner_keys`), as [`ProvenBrepIdentity::new`]. The
    /// overrides are owned, so the result is a [`ProvenBrepIdentity`]; pass
    /// [`ProvenBrepIdentity::identity`] where a [`BrepIdentity`] is taken. A query
    /// under the wrong identities resolves no geometry owner and finds nothing
    /// (`plan/findings.md`, "two owner-naming schemes"; #1440).
    #[must_use]
    pub fn of_brep_with_id(brep_id: BrepId, b: &Brep) -> ProvenBrepIdentity<'static> {
        ProvenBrepIdentity::new(Self::of_naming_with_id(brep_id, b), b)
    }

    /// [`Self::of_brep_with_id`] at [`BrepId`] `0`.
    #[must_use]
    pub fn of_brep(b: &Brep) -> ProvenBrepIdentity<'static> {
        Self::of_brep_with_id(BrepId::from_index(0), b)
    }
}

type ProvenKeyFn<'a> = Box<dyn Fn(CacheObject) -> ObjectRef + 'a>;

/// A [`BrepIdentity`] with the proven producer owner keys of its B-rep
/// (`Brep::geometry_owner_keys`) overriding the base keys where they differ: what
/// [`BrepIdentity::of_brep`] and [`BrepQuery::of_brep`] resolve a B-rep's records by.
/// It owns its overrides and does not borrow the B-rep.
pub struct ProvenBrepIdentity<'a> {
    base: BrepIdentity<'a>,
    proven_keys: Option<ProvenKeyFn<'a>>,
}

impl core::fmt::Debug for ProvenBrepIdentity<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProvenBrepIdentity")
            .field("base", &self.base)
            .field("overridden", &self.proven_keys.is_some())
            .finish()
    }
}

impl<'a> ProvenBrepIdentity<'a> {
    /// `base` without overrides.
    #[must_use]
    pub const fn plain(base: BrepIdentity<'a>) -> Self {
        Self {
            base,
            proven_keys: None,
        }
    }

    /// `base`, overriding edge/face keys where `b` records a proven producer
    /// geometry-owner key (`Brep::geometry_owner_keys`) that differs from `base.keys`
    /// for geometry actually owned by it. Unowned (`None`) and foreign-owned geometry
    /// not recorded in `b.geometry_owner_keys()` is still excluded.
    #[must_use]
    pub fn new(base: BrepIdentity<'a>, b: &Brep) -> Self {
        let base_keys = base.keys;
        let mut overrides = std::collections::BTreeMap::new();
        for (&entity, &proven) in b.geometry_owner_keys() {
            let matches_base = match entity {
                EntityRef::Edge(e) => b
                    .edges()
                    .get(e)
                    .and_then(|edge| edge.curve.as_ref())
                    .and_then(crate::curve::curve::p2::Curve::owner)
                    .is_some_and(|actual| actual == base_keys(CacheObject::Edge(e))),
                EntityRef::Face(f) => b
                    .faces()
                    .get(f)
                    .and_then(|face| face.surface.as_ref())
                    .and_then(Surface::owner)
                    .is_some_and(|actual| actual == base_keys(CacheObject::Face(f))),
                _ => true,
            };
            if !matches_base {
                overrides.insert(entity, proven);
            }
        }
        let proven_keys: Option<ProvenKeyFn<'a>> = (!overrides.is_empty()).then(|| {
            Box::new(move |object| {
                let entity = match object {
                    CacheObject::Edge(e) | CacheObject::Curve(e) => Some(EntityRef::Edge(e)),
                    CacheObject::Face(f) | CacheObject::Surface(f) => Some(EntityRef::Face(f)),
                    _ => None,
                };
                entity
                    .and_then(|e| overrides.get(&e).copied())
                    .unwrap_or_else(|| base_keys(object))
            }) as ProvenKeyFn<'a>
        });
        Self { base, proven_keys }
    }

    /// The base identities, without the overrides.
    #[must_use]
    pub const fn base(&self) -> BrepIdentity<'a> {
        self.base
    }

    /// The effective identities: the base with the proven overrides applied.
    #[must_use]
    pub fn identity(&self) -> BrepIdentity<'_> {
        BrepIdentity {
            brep_id: self.base.brep_id,
            keys: match &self.proven_keys {
                Some(keys) => keys.as_ref(),
                None => self.base.keys,
            },
            owner_of: self.base.owner_of,
        }
    }
}

/// The cache-object identities of a [`NamingScheme::Notify`] B-rep
/// ([`crate::topology::loopuse::notify_ref`] for its edges, faces and vertices).
fn notify_identity(object: CacheObject) -> ObjectRef {
    crate::topology::brep::p2::caller_cache_key(&crate::topology::loopuse::notify_ref, object)
}

impl Default for BrepIdentity<'_> {
    /// [`BrepIdentity::arena`] of the first B-rep of the caller's store (a caller
    /// querying one B-rep at a time).
    fn default() -> Self {
        BrepIdentity::arena(BrepId::from_index(0))
    }
}

/// The ray directions of [`BrepQuery::point_3d_classify`], in firing order: each axis
/// forward, then backward when the forward ray fails.
const CLASSIFY_RAYS: [(Vec3, Vec3); 3] = [
    (Vec3::new(1.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)),
    (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -1.0, 0.0)),
    (Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, -1.0)),
];

/// A B-rep's queries over its cache: the identities and the `SM_OC_BREP` cache, which
/// upstream builds when a query first needs it and keeps on the B-rep
/// (`SmSAGObject::CacheMakeOrValidate`). Keep one per B-rep state and reuse it across
/// queries; edits to the B-rep invalidate it.
pub struct BrepQuery<'a> {
    identity: ProvenBrepIdentity<'a>,
    cache: OnceCell<BrepCache>,
}

impl core::fmt::Debug for BrepQuery<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BrepQuery")
            .field("identity", &self.identity)
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl Default for BrepQuery<'static> {
    /// The queries of a B-rep with the default identities ([`BrepIdentity::default`]).
    fn default() -> Self {
        Self::new(BrepIdentity::default())
    }
}

impl BrepQuery<'static> {
    /// Queries of `b` at [`BrepId`] `0`, resolving records by `b`'s naming scheme and
    /// proven producer geometry-owner identities (`Brep::geometry_owner_keys`).
    #[must_use]
    pub fn of_brep(b: &Brep) -> Self {
        Self::of_brep_with_id(BrepId::from_index(0), b)
    }

    /// Queries of `b` at `brep_id`, resolving records by `b`'s naming scheme and proven
    /// producer geometry-owner identities (`Brep::geometry_owner_keys`).
    #[must_use]
    pub fn of_brep_with_id(brep_id: BrepId, b: &Brep) -> Self {
        Self::from_proven(BrepIdentity::of_brep_with_id(brep_id, b))
    }
}

impl<'a> BrepQuery<'a> {
    /// The queries of the B-rep with `identity`; its cache is built on first use.
    #[must_use]
    pub const fn new(identity: BrepIdentity<'a>) -> Self {
        Self::from_proven(ProvenBrepIdentity::plain(identity))
    }

    /// The queries of the B-rep with `identity` (proven overrides included).
    #[must_use]
    pub const fn from_proven(identity: ProvenBrepIdentity<'a>) -> Self {
        Self {
            identity,
            cache: OnceCell::new(),
        }
    }

    /// Queries of `b` with `identity` as the base naming, overriding edge/face keys where
    /// `b` records a proven producer geometry-owner key (`Brep::geometry_owner_keys`)
    /// ([`ProvenBrepIdentity::new`]).
    #[must_use]
    pub fn with_proven_owners(identity: BrepIdentity<'a>, b: &Brep) -> Self {
        Self::from_proven(ProvenBrepIdentity::new(identity, b))
    }

    /// The queries over a cache the caller already holds for the B-rep.
    #[must_use]
    pub fn with_cache(identity: BrepIdentity<'a>, cache: BrepCache) -> Self {
        Self {
            identity: ProvenBrepIdentity::plain(identity),
            cache: OnceCell::from(cache),
        }
    }

    /// The base identities (without proven overrides).
    #[must_use]
    pub const fn identity(&self) -> BrepIdentity<'a> {
        self.identity.base()
    }

    /// The effective [`BrepIdentity`], including any proven geometry-owner overrides
    /// recorded on the B-rep when built with [`Self::of_brep`] or
    /// [`Self::with_proven_owners`] (the same as [`BrepIdentity::of_brep`]'s
    /// [`ProvenBrepIdentity::identity`]).
    #[must_use]
    pub fn effective_identity(&self) -> BrepIdentity<'_> {
        self.identity.identity()
    }

    /// The cache of `b`, built keyed by the identities when first asked for.
    ///
    /// # Errors
    ///
    /// Those of [`make_brep_cache`].
    pub fn cache(&self, k: &Kernel, b: &Brep) -> Result<&BrepCache> {
        if let Some(cache) = self.cache.get() {
            return Ok(cache);
        }
        let identity = self.effective_identity();
        let cache = make_brep_cache(k, CacheSource::Brep(b), identity.keys)?;
        Ok(self.cache.get_or_init(|| cache))
    }

    /// Mutable ray solve used by region nesting after stitching. Keeps lazy trim
    /// preparation in the original solve order.
    pub(crate) fn ray_fire_live(
        &self,
        k: &Kernel,
        b: &mut Brep,
        start: Point3,
        direction: Vec3,
        tolerance: f64,
    ) -> Result<crate::topology::brep::p2::RayFireResult> {
        let identity = self.effective_identity();
        let cache = self.cache(k, b)?;
        crate::topology::brep::p2::ray_fire_with_solver(
            k,
            b,
            start,
            direction,
            tolerance,
            None,
            |b, point, options| solve_mutable_brep_point(k, b, &identity, cache, point, options),
        )
    }

    /// `SmBrep::Point3DClassify`: classifies `point` (zone tolerance
    /// `point_zone_tol_3d`) against `b`, the B-rep this query was built for. With
    /// `do_boundary_intersections` the lowest dimensional vertex, edge or face the point
    /// lies on (`BrepPointSolve`, `SM_SO_INTERSECT`, single solution) is the answer when
    /// there is one. Otherwise the region: the only one when there is one, else the
    /// region a ray fire ([`bp2::ray_fire`]) from the point finds along `+x` (then `-x`
    /// when that fails), then `+y`/`-y`, then `+z`/`-z` while none was found. The ray
    /// fires may make missing UV trim curves on `b` (upstream's `const` method makes them
    /// too); a caller holding the B-rep shared classifies on a copy.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when no ray finds a region; those of the boundary solve, the region list
    /// and the classification load; [`SolidError::Unported`] from the ray fires.
    pub fn point_3d_classify(
        &self,
        k: &Kernel,
        b: &mut Brep,
        point: Point3,
        point_zone_tol_3d: f64,
        do_boundary_intersections: bool,
    ) -> Result<PointClassification<EntityRef>> {
        self.classify(
            k,
            Target::Own(b),
            point,
            point_zone_tol_3d,
            do_boundary_intersections,
        )
    }

    /// [`BrepQuery::point_3d_classify`] on `target`.
    fn classify(
        &self,
        k: &Kernel,
        mut target: Target<'_>,
        point: Point3,
        point_zone_tol_3d: f64,
        do_boundary_intersections: bool,
    ) -> Result<PointClassification<EntityRef>> {
        let identity = self.effective_identity();
        let BrepIdentity {
            brep_id,
            keys,
            owner_of,
        } = identity;

        // init output
        let mut pc = PointClassification::new(point_zone_tol_3d);
        pc.set_class_object(PointClassType::Unknown, None);
        pc.set_src_zone_tol_3d(point_zone_tol_3d);

        // Intersect the point with the boundary elements of the Brep
        if do_boundary_intersections {
            let options = TopologySolveOptions {
                operation: SolverOperation::Intersect,
                requested: SolutionRequested::Single,
                tolerance_3d: point_zone_tol_3d,
                best_answer_so_far_sq: 0.0,
                opt_vectors: &[],
            };
            // XSects point with the Brep verts, edges, and faces returning lowest dim
            // topology intersection found.
            // A face-owned surface cache needs mutable access to its actual face.
            // A shared target without faces cannot reach that lookup and can use
            // the read-only solver without copying the B-rep.
            let solutions = if matches!(&target, Target::Shared { .. })
                && target.shared().faces().ids().next().is_none()
            {
                let b = target.shared();
                let objects = BrepObjects {
                    preparation: None,
                    brep: b,
                    brep_id,
                    cache: Some(self.cache(k, b)?),
                    keys,
                    owner_of,
                };
                TopologySolver::brep_point_solve(
                    k,
                    &objects,
                    Some(CacheObject::Brep),
                    point,
                    options,
                )?
            } else {
                let b = target.mutable()?;
                let cache = self.cache(k, b)?;
                solve_mutable_brep_point(k, b, &identity, cache, point, options)?
            };

            // when the point is on a Brep Face
            if let Some(sol) = solutions.get(0) {
                // Just grab the first one -- we asked for Single.
                let object = sol.objects[0].ok_or(SolidError::null_pointer(OP_CLASSIFY))?;
                pc.load_from_intersection(
                    point_zone_tol_3d,
                    bp2::cache_entity(object)?,
                    &sol.start,
                    0,
                    BIG_DOUBLE,
                )?;
                return Ok(pc);
            }
        } // end bDoBoundaryIntersections check

        // get Brep->Regions
        let regions = bp1::regions(target.shared(), None)?;

        // Check for special case where only one region exists then we
        // do not have to do any ray firing.
        if let [region] = regions.as_slice() {
            pc.set_class_object(PointClassType::Region, Some(EntityRef::Region(*region)));
            pc.set_gap_3d(0.0);
            pc.set_pre_snap_param(BIG_DOUBLE);
            return Ok(pc);
        }

        // Now do a ray fire to determine which region this lies in. Fire extra rays
        // only if the first rays fail: `Some(region)` on success (the region may be
        // none), `None` on failure.
        // (the solve of each ray fire builds the cache when needed)
        let mut fire = |direction: Vec3| -> Result<Option<Option<RegionId>>> {
            let b = target.mutable()?;
            let fired = self.cache(k, b).and_then(|cache| {
                bp2::ray_fire(
                    k,
                    b,
                    brep_id,
                    cache,
                    keys,
                    owner_of,
                    point,
                    direction,
                    point_zone_tol_3d,
                    None,
                )
            });
            Ok(ignored(fired)?.map(|r| r.region))
        };
        let mut region = None;
        for (forward, backward) in CLASSIFY_RAYS {
            region = match fire(forward)? {
                Some(r) => r,
                None => fire(backward)?.flatten(),
            };
            if region.is_some() {
                break;
            }
        }

        let region = region.ok_or(SolidError::sm_err(OP_CLASSIFY))?;

        // load output
        pc.set_class_object(PointClassType::Region, Some(EntityRef::Region(region)));
        pc.set_deviation(0.0); // SM_USE_OLDTOL
        pc.set_pre_snap_param(BIG_DOUBLE);
        Ok(pc)
    }
}

/// The B-rep's ray queries at a shell (`SmShell::RayFire`): the shape solve reads the
/// B-rep's geometry under these identities, without the B-rep's cache (a shape has
/// its own trees), and the hits are its vertices, edges and faces.
impl crate::topology::shell::ShellRayWorld<CacheObject> for BrepQuery<'_> {
    type Objects<'a>
        = BrepObjects<'a>
    where
        Self: 'a;

    fn objects<'a>(&'a self, brep: (BrepId, &'a Brep)) -> Result<BrepObjects<'a>> {
        let (brep_id, brep) = brep;
        let identity = self.effective_identity();
        if brep_id != identity.brep_id {
            return Err(SolidError::invalid_input(
                "query::brep::BrepQuery::objects",
                "ray fire on another B-rep than the query's",
            ));
        }
        Ok(BrepObjects {
            preparation: None,
            brep,
            brep_id,
            cache: None,
            keys: identity.keys,
            owner_of: identity.owner_of,
        })
    }

    fn objects_for_preparation<'a>(
        &'a self,
        brep: (BrepId, &'a Brep),
        preparation: &'a RayPreparation,
    ) -> Result<BrepObjects<'a>> {
        let mut objects = self.objects(brep)?;
        objects.preparation = Some(preparation);
        Ok(objects)
    }

    fn solve_shape_ray(
        &self,
        k: &Kernel,
        brep: &mut Brep,
        brep_id: BrepId,
        shape: &crate::geom::object::Shape,
        point: Point3,
        options: TopologySolveOptions<'_>,
    ) -> Result<crate::geom::solution_array::SolutionArray<CacheObject>> {
        let identity = self.effective_identity();
        if brep_id != identity.brep_id {
            return Err(SolidError::invalid_input(
                "query::brep::BrepQuery::solve_shape_ray",
                "ray fire on another B-rep",
            ));
        }
        ray::solve_mutable_ray(k, brep, &identity, shape, point, options)
    }

    fn class_entity(&self, object: CacheObject) -> Option<EntityRef> {
        bp2::cache_entity(object).ok()
    }
}

/// A B-rep an upstream `const` query may still edit (`PointClassify` attaches missing
/// UV trim curves, and `SetUVTrimCurve` raises the edge tolerances to fit them): the
/// caller's, lent mutably, where the edits persist as upstream; or one the caller holds
/// shared, edited on a private copy made at the first edit, whose edits are lost when
/// the query returns. Built with `From` from `&mut Brep` or `&Brep`.
#[derive(Debug)]
pub enum LazyBrep<'a> {
    /// The caller's B-rep, edited in place.
    Own(&'a mut Brep),
    /// A B-rep the caller holds shared.
    Shared {
        /// The caller's B-rep.
        source: &'a Brep,
        /// Its private copy, made at the first edit.
        copy: Option<Box<Brep>>,
    },
}

impl<'a> From<&'a mut Brep> for LazyBrep<'a> {
    fn from(b: &'a mut Brep) -> Self {
        Self::Own(b)
    }
}

impl<'a> From<&'a Brep> for LazyBrep<'a> {
    fn from(source: &'a Brep) -> Self {
        Self::Shared { source, copy: None }
    }
}

impl LazyBrep<'_> {
    /// The B-rep as it stands (the copy once made).
    #[must_use]
    pub fn get(&self) -> &Brep {
        match self {
            Self::Own(b) => b,
            Self::Shared { source, copy } => copy.as_deref().unwrap_or(source),
        }
    }

    /// The B-rep to edit, copying a shared one first.
    pub fn get_mut(&mut self) -> &mut Brep {
        match self {
            Self::Own(b) => b,
            Self::Shared { source, copy } => {
                copy.get_or_insert_with(|| Box::new((*source).clone()))
            }
        }
    }
}

/// The B-rep a classification runs on: the caller's, lent mutably, or one it holds
/// shared, copied when a face boundary solve or ray fire needs it mutably.
enum Target<'t> {
    Own(&'t mut Brep),
    Shared {
        source: &'t Brep,
        copy: &'t mut Option<Brep>,
    },
}

impl Target<'_> {
    /// The B-rep as it stands (the copy once made: it may hold new UV trim curves).
    fn shared(&self) -> &Brep {
        match self {
            Self::Own(b) => b,
            Self::Shared { source, copy } => copy.as_ref().unwrap_or(source),
        }
    }

    /// The B-rep to edit, copying a shared one first: the copy stands for the B-rep
    /// ([`Brep::clone_standing_for_self`]), so its geometry keeps naming its edges and
    /// faces and the face boundary solves' and ray fires' caches hold them.
    fn mutable(&mut self) -> Result<&mut Brep> {
        match self {
            Self::Own(b) => Ok(b),
            Self::Shared { source, copy } => match copy {
                Some(b) => Ok(b),
                None => Ok(copy.insert(source.clone_standing_for_self()?)),
            },
        }
    }
}

/// Point classification against a B-rep the caller holds shared: its [`BrepQuery`], and
/// a private copy for face boundary solves and ray fires, which may add missing UV trim
/// curves (upstream's `const` `Point3DClassify` adds them to the B-rep itself). The
/// copy is made when either solve first needs it. Keep one per B-rep state and classify
/// every point through it: the copy and the cache are made once.
#[derive(Debug)]
pub struct BrepClassifier<'a, 'b> {
    query: BrepQuery<'a>,
    source: &'b Brep,
    copy: Option<Brep>,
}

impl<'b> BrepClassifier<'static, 'b> {
    /// The classifier of `brep` at [`BrepId`] `0`, resolving records by `brep`'s naming
    /// scheme and proven producer geometry-owner identities ([`BrepQuery::of_brep`]).
    #[must_use]
    pub fn of_brep(brep: &'b Brep) -> Self {
        Self::of_brep_with_id(BrepId::from_index(0), brep)
    }

    /// The classifier of `brep` at `brep_id`, resolving records by `brep`'s naming
    /// scheme and proven producer geometry-owner identities
    /// ([`BrepQuery::of_brep_with_id`]).
    #[must_use]
    pub fn of_brep_with_id(brep_id: BrepId, brep: &'b Brep) -> Self {
        Self {
            query: BrepQuery::of_brep_with_id(brep_id, brep),
            source: brep,
            copy: None,
        }
    }
}

impl<'a, 'b> BrepClassifier<'a, 'b> {
    /// The classifier of `brep` with `identity` (nothing is copied or built yet).
    #[must_use]
    pub const fn new(brep: &'b Brep, identity: BrepIdentity<'a>) -> Self {
        Self {
            query: BrepQuery::new(identity),
            source: brep,
            copy: None,
        }
    }

    /// The B-rep the classifications run on (the private copy once made; its regions
    /// are the caller's).
    #[must_use]
    pub fn brep(&self) -> &Brep {
        self.copy.as_ref().unwrap_or(self.source)
    }

    /// Whether a classification has copied the B-rep.
    #[must_use]
    pub const fn has_copied(&self) -> bool {
        self.copy.is_some()
    }

    /// [`BrepQuery::point_3d_classify`] on the B-rep, copied when a face boundary solve
    /// or ray fire first needs it.
    ///
    /// # Errors
    ///
    /// Those of [`BrepQuery::point_3d_classify`].
    pub fn point_3d_classify(
        &mut self,
        k: &Kernel,
        point: Point3,
        point_zone_tol_3d: f64,
        do_boundary_intersections: bool,
    ) -> Result<PointClassification<EntityRef>> {
        let target = Target::Shared {
            source: self.source,
            copy: &mut self.copy,
        };
        self.query.classify(
            k,
            target,
            point,
            point_zone_tol_3d,
            do_boundary_intersections,
        )
    }
}

// ---- SM_API surface evaluation ------------------------------------------------------

/// The input checks the three evaluation entry points share: a surface, and a
/// parameter inside its natural domain (bounds included; `GetNaturalUVDomain`, which
/// makes a lazy analytic's missing NURB).
fn checked_surface<'s>(
    k: &Kernel,
    surface: Option<&'s Surface>,
    uv: Point2,
    op: &'static str,
) -> Result<&'s Surface> {
    let Some(surface) = surface else {
        return Err(SolidError::invalid_input(op, "no surface"));
    };

    // Negated in-range test so a NaN parameter is rejected too.
    let domain = surface.natural_uv_domain_with_nurb(k)?;
    let (lo, hi) = (domain.min(), domain.max());
    if !(uv.x >= lo.x && uv.x <= hi.x && uv.y >= lo.y && uv.y <= hi.y) {
        return Err(SolidError::invalid_input(
            op,
            "parameter outside the natural domain",
        ));
    }
    Ok(surface)
}

/// `SmApiEvaluateSurfacePoint`: the point of `surface` at `uv`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] without a surface or for a parameter outside (or NaN
/// against) the surface's natural domain; those of [`Surface::evaluate_point`].
pub fn evaluate_surface_point(k: &Kernel, surface: Option<&Surface>, uv: Point2) -> Result<Point3> {
    checked_surface(k, surface, uv, OP_EVAL_POINT)?.evaluate_point(k, uv)
}

/// `SmApiEvaluateSurfaceNormal`: the unit normal of `surface` at `uv`, from the upper
/// interval in both directions on a knot (`EvaluateNormal(uv, TRUE, TRUE)`).
///
/// # Errors
///
/// Those of [`evaluate_surface_point`]'s input checks; those of
/// [`Surface::evaluate_normal`].
pub fn evaluate_surface_normal(k: &Kernel, surface: Option<&Surface>, uv: Point2) -> Result<Vec3> {
    checked_surface(k, surface, uv, OP_EVAL_NORMAL)?.evaluate_normal(k, uv, Side::Left, Side::Left)
}

/// `SmApiEvaluateSurfaceDerivatives`: the point and first derivatives `(P, Du, Dv)` of
/// `surface` at `uv`, from the upper interval in both directions on a knot
/// (`Evaluate1stDerivatives(uv, TRUE, TRUE)`).
///
/// # Errors
///
/// Those of [`evaluate_surface_point`]'s input checks; those of
/// [`Surface::evaluate_1st_derivatives`].
pub fn evaluate_surface_derivatives(
    k: &Kernel,
    surface: Option<&Surface>,
    uv: Point2,
) -> Result<(Point3, Vec3, Vec3)> {
    checked_surface(k, surface, uv, OP_EVAL_DERIVS)?.evaluate_1st_derivatives(k, uv, true, true)
}

// ---- polygon decimation attributes ---------------------------------------------------

/// `SmPolyVertexClass`: the class of a polygon vertex during decimation; the
/// discriminant is the upstream value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PolyVertexClass {
    /// `SM_PVC_NOT_COMPUTED`: not yet computed.
    NotComputed = 0,
    /// `SM_PVC_UNKNOWN`: connected to a spine edge, or non-manifold.
    Unknown = 1,
    /// `SM_PVC_SIMPLE`: an interior vertex on no lamina, feature or silhouette edge.
    Simple = 2,
    /// `SM_PVC_BOUNDARY`: on the boundary of the mesh (has lamina edges).
    Boundary = 3,
    /// `SM_PVC_INTERIOR`: an interior point between two feature edges.
    Interior = 4,
    /// `SM_PVC_CORNER`: an interior point between three or more feature edges.
    Corner = 5,
}

impl PolyVertexClass {
    /// Every class, in discriminant order.
    pub const ALL: [Self; 6] = [
        Self::NotComputed,
        Self::Unknown,
        Self::Simple,
        Self::Boundary,
        Self::Interior,
        Self::Corner,
    ];
}

/// The data of an `SmDecimateVertexAttr` (attribute id `SM_AI_VERTEX_DECIMATION_VALUES`).
#[derive(Debug, Clone, PartialEq)]
pub struct DecimateVertexData {
    /// `m_lSortIndex`.
    pub sort_index: u64,
    /// `m_eVertClass`.
    pub vertex_class: PolyVertexClass,
    /// `m_dVertexDistance`: the weighted chord height from the vertex to the baseline
    /// between its feature or boundary edges.
    pub vertex_distance: f64,
    /// `m_dMaxFaceError`: the largest error of the faces around the vertex.
    pub max_face_error: f64,
    /// `m_vAveragePlaneNormal`: the area-weighted normal of the faces around the vertex.
    pub average_plane_normal: Vec3,
}

impl DecimateVertexData {
    /// `SmDecimateVertexAttr::MakeCopy`'s data: everything but the sort index, which
    /// keeps the constructor's `0`.
    #[must_use]
    pub fn make_copy(&self) -> Self {
        Self {
            sort_index: 0,
            ..self.clone()
        }
    }
}

/// The data of an `SmQuadricVertexAttr` (attribute id `SM_AI_QUADRIC_VERTEX_VALUES`): the
/// quadric error form `NewVert . Q . NewVert + 2 b . NewVert + c` of the planes of the
/// original polygons around a vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct QuadricVertexData {
    /// `m_Q`: the rows of the quadratic error matrix.
    pub q: [Vec3; 3],
    /// `m_b`.
    pub b: Vec3,
    /// `m_c`.
    pub c: f64,
}

/// The data of an `SmQuadricEdgeAttr` (attribute id `SM_AI_QUADRIC_EDGE_VALUES`): the
/// vertex an edge contracts to and its error.
#[derive(Debug, Clone, PartialEq)]
pub struct QuadricEdgeData {
    /// `m_vNewPoint`.
    pub new_point: Point3,
    /// `m_dError`.
    pub error: f64,
    /// `m_lSortIndex`; `None` while uninitialised (as the constructor leaves it).
    pub sort_index: Option<u64>,
}

impl QuadricEdgeData {
    /// `SmQuadricEdgeAttr::MakeCopy`'s data: the point and error, sort index `0`.
    #[must_use]
    pub fn make_copy(&self) -> Self {
        Self {
            new_point: self.new_point,
            error: self.error,
            sort_index: Some(0),
        }
    }
}

/// `SmDecimateVertexAttr(lAttributeID, eBehavior)` (upstream default
/// [`Behavior::Copy`]): sort index `0`, class not computed, vertex distance
/// `-SM_BIG_DOUBLE`, no face error, zero average normal.
#[must_use]
pub fn decimate_vertex_attribute(attribute_id: u64, behavior: Behavior) -> Attribute {
    Attribute::with_value(
        attribute_id,
        behavior,
        AttributeValue::DecimateVertex(DecimateVertexData {
            sort_index: 0,
            vertex_class: PolyVertexClass::NotComputed,
            vertex_distance: -BIG_DOUBLE,
            max_face_error: 0.0,
            average_plane_normal: Vec3::new(0.0, 0.0, 0.0),
        }),
    )
}

/// `SmQuadricVertexAttr(lAttributeID, eBehavior)` (upstream default
/// [`Behavior::Copy`]): the zero error form.
#[must_use]
pub fn quadric_vertex_attribute(attribute_id: u64, behavior: Behavior) -> Attribute {
    let zero = Vec3::new(0.0, 0.0, 0.0);
    Attribute::with_value(
        attribute_id,
        behavior,
        AttributeValue::QuadricVertex(QuadricVertexData {
            q: [zero, zero, zero],
            b: zero,
            c: 0.0,
        }),
    )
}

/// `SmQuadricEdgeAttr(lAttributeID, eBehavior)` (upstream default [`Behavior::Copy`]):
/// the origin as new point, no error, no sort index.
#[must_use]
pub fn quadric_edge_attribute(attribute_id: u64, behavior: Behavior) -> Attribute {
    Attribute::with_value(
        attribute_id,
        behavior,
        AttributeValue::QuadricEdge(QuadricEdgeData {
            new_point: Vec3::new(0.0, 0.0, 0.0),
            error: 0.0,
            sort_index: None,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent2d::Extent2d;
    use crate::geom::vector2d::Vec2;
    use crate::surface::plane::p1::Plane;

    fn unit_plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    #[test]
    fn rejects_missing_surface_and_out_of_domain_parameters() {
        let k = Kernel::default();
        let s = unit_plane();
        let bad = |r: Result<Point3>| matches!(r, Err(SolidError::InvalidInput { .. }));
        assert!(bad(evaluate_surface_point(&k, None, Point2::new(0.5, 0.5))));
        assert!(bad(evaluate_surface_point(
            &k,
            Some(&s),
            Point2::new(1.5, 0.5)
        )));
        assert!(bad(evaluate_surface_point(
            &k,
            Some(&s),
            Point2::new(f64::NAN, 0.5)
        )));
        // the domain bounds are inside
        assert!(evaluate_surface_point(&k, Some(&s), Point2::new(1.0, 0.0)).is_ok());
    }

    #[test]
    fn bounding_box_lock_and_vertex_only_paths() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let v0 = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        b.add_vertex(Point3::new(1.0, 2.0, 3.0)).unwrap();
        b.vertex_mut(v0).unwrap().tolerance = 0.5;

        let expanded = calculate_bounding_box(&k, &b, BoundingBoxOptions::default()).unwrap();
        assert_eq!(expanded.min().x, -0.5);
        let plain = BoundingBoxOptions {
            expand_by_zone_tol_3d: false,
            ..BoundingBoxOptions::default()
        };
        let bare = calculate_bounding_box(&k, &b, plain).unwrap();
        assert_eq!((bare.min().x, bare.max().z), (0.0, 3.0));
        assert_eq!(calculate_tight_bounding_box(&k, &b).unwrap(), bare);

        lock_bounding_box(&k, &mut b, false).unwrap();
        b.vertex_mut(v0).unwrap().point = Point3::new(-9.0, 0.0, 0.0);
        // the locked box is returned whatever the arguments; the tight box ignores it
        assert_eq!(calculate_bounding_box(&k, &b, plain).unwrap(), expanded);
        assert_eq!(calculate_tight_bounding_box(&k, &b).unwrap().min().x, -9.0);
        // relocking recomputes
        lock_bounding_box(&k, &mut b, true).unwrap();
        assert_eq!(calculate_bounding_box(&k, &b, plain).unwrap().min().x, -9.0);
        unlock_bounding_box(&mut b);
        assert_eq!(b.locked_bounding_box(), None);
        assert_eq!(calculate_bounding_box(&k, &b, plain).unwrap().min().x, -9.0);
    }

    #[test]
    fn classification_builds_the_cache_only_when_a_solve_needs_it() {
        let k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        let query = BrepQuery::new(BrepIdentity::arena(BrepId::from_index(0)));
        let p = Point3::new(1.0, 2.0, 3.0);
        // one region: no ray fire, no cache (as upstream)
        let pc = query.point_3d_classify(&k, &mut b, p, 1e-6, false).unwrap();
        assert_eq!(pc.point_class(), PointClassType::Region);
        assert!(query.cache.get().is_none());
        // the boundary solve builds it; an empty B-rep has no boundary to hit
        let pc = query.point_3d_classify(&k, &mut b, p, 1e-6, true).unwrap();
        assert_eq!(pc.point_class(), PointClassType::Region);
        assert!(query.cache.get().is_some());

        let mut classifier = BrepClassifier::new(&b, BrepIdentity::arena(BrepId::from_index(0)));
        let pc = classifier.point_3d_classify(&k, p, 1e-6, true).unwrap();
        assert_eq!(pc.object(), b.infinite_region().map(EntityRef::Region));
    }

    #[test]
    fn classifier_fires_rays_on_a_copy_and_leaves_the_callers_brep_alone() {
        let k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        b.add_region_record(crate::topology::Region {
            in_brep: true,
            ..Default::default()
        })
        .unwrap();
        let before = format!("{b:?}");

        let mut classifier = BrepClassifier::new(&b, BrepIdentity::default());
        assert!(!classifier.has_copied());
        // two regions: the ray fire needs the B-rep mutable, so it works on a copy
        let _ = classifier.point_3d_classify(&k, Point3::new(1.0, 2.0, 3.0), 1e-6, false);
        assert!(classifier.has_copied());
        let copied: *const Brep = classifier.brep();
        let _ = classifier.point_3d_classify(&k, Point3::new(3.0, 2.0, 1.0), 1e-6, false);
        assert!(core::ptr::eq(classifier.brep(), copied), "copied once");
        assert_eq!(format!("{b:?}"), before);
    }

    /// A unit box from `create_box` without its UV trims (upstream's classification
    /// makes the ones its loop walks reach), and a query under the box's own keys
    /// (`notify_ref`, as `create_box` owns its geometry).
    fn box_without_trims(k: &mut Kernel) -> (Brep, BrepQuery<'static>) {
        fn keys(o: CacheObject) -> ObjectRef {
            use crate::topology::loopuse::notify_ref;
            match o {
                CacheObject::Edge(e) | CacheObject::Curve(e) => notify_ref(EntityRef::Edge(e)),
                CacheObject::Face(f) | CacheObject::Surface(f) => notify_ref(EntityRef::Face(f)),
                CacheObject::Vertex(v) => notify_ref(EntityRef::Vertex(v)),
                _ => notify_ref(EntityRef::Brep(BrepId::from_index(0))),
            }
        }
        fn no_owner(_: ObjectRef) -> Option<ObjectRef> {
            None
        }
        let mut b =
            crate::primitive::api_primitives::create_box(k, Point3::ZERO, 1.0, 1.0, 1.0).unwrap();
        let uses: Vec<_> = b.edgeuses().ids().collect();
        for u in uses {
            b.edgeuse_mut(u).unwrap().uv_trim_curve = None;
        }
        let query = BrepQuery::new(BrepIdentity {
            brep_id: BrepId::from_index(0),
            keys: &keys,
            owner_of: &no_owner,
        });
        (b, query)
    }

    fn trim_count(b: &Brep) -> usize {
        b.edgeuses()
            .iter()
            .filter(|(_, u)| u.uv_trim_curve.is_some())
            .count()
    }

    #[test]
    fn ray_fires_make_missing_uv_trims_on_the_callers_brep() {
        let mut k = Kernel::default();
        let (mut b, query) = box_without_trims(&mut k);
        let solid = b
            .regions()
            .ids()
            .find(|&r| Some(r) != b.infinite_region())
            .map(EntityRef::Region);
        let inside = query
            .point_3d_classify(&k, &mut b, Point3::new(0.5, 0.5, 0.5), 1e-6, false)
            .unwrap();
        assert_eq!(inside.object(), solid);
        assert!(trim_count(&b) > 0, "the trims are made on the box itself");
    }

    #[test]
    fn lazy_brep_edits_the_callers_brep_or_a_copy() {
        let mut b = Brep::new();
        let v = b.add_vertex(Point3::new(1.0, 2.0, 3.0)).unwrap();

        let mut shared = LazyBrep::from(&b);
        shared.get_mut().vertex_mut(v).unwrap().tolerance = 0.5;
        assert_eq!(shared.get().vertex(v).unwrap().tolerance, 0.5);
        drop(shared);
        assert_ne!(b.vertex(v).unwrap().tolerance, 0.5);

        let mut own = LazyBrep::from(&mut b);
        own.get_mut().vertex_mut(v).unwrap().tolerance = 0.5;
        drop(own);
        assert_eq!(b.vertex(v).unwrap().tolerance, 0.5);
    }

    #[test]
    fn a_clone_keeps_the_locked_box() {
        let k = Kernel::default();
        let mut b = Brep::new();
        b.add_vertex(Point3::new(1.0, 2.0, 3.0)).unwrap();
        lock_bounding_box(&k, &mut b, false).unwrap();
        let locked = b.locked_bounding_box();
        assert!(locked.is_some());
        assert_eq!(b.clone().locked_bounding_box(), locked);
    }

    #[test]
    fn decimation_attribute_copies_reset_sort_indices() {
        let mut a = decimate_vertex_attribute(7, Behavior::Reference);
        if let AttributeValue::DecimateVertex(d) = a.value_mut() {
            d.sort_index = 9;
            d.vertex_class = PolyVertexClass::Corner;
            d.max_face_error = 0.5;
        }
        let c = a.make_copy();
        assert_eq!((c.attribute_id(), c.behavior()), (7, Behavior::Reference));
        let AttributeValue::DecimateVertex(d) = c.value() else {
            panic!("class lost");
        };
        assert_eq!(
            (d.sort_index, d.vertex_class, d.max_face_error),
            (0, PolyVertexClass::Corner, 0.5)
        );
        assert_eq!(d.vertex_distance, -BIG_DOUBLE);

        let e = quadric_edge_attribute(8, Behavior::Copy);
        let AttributeValue::QuadricEdge(d) = e.make_copy().value().clone() else {
            panic!("class lost");
        };
        assert_eq!(d.sort_index, Some(0));
        assert_eq!(e.type_code(), crate::geom::attribute::type_code::ATTRIBUTE);
    }

    #[test]
    fn evaluates_a_plane() {
        let k = Kernel::default();
        let s = unit_plane();
        let uv = Point2::new(0.25, 0.75);
        let p = evaluate_surface_point(&k, Some(&s), uv).unwrap();
        assert_eq!((p.x, p.y, p.z), (0.25, 0.75, 0.0));
        let n = evaluate_surface_normal(&k, Some(&s), uv).unwrap();
        assert_eq!((n.x, n.y, n.z), (0.0, 0.0, 1.0));
        let (q, du, dv) = evaluate_surface_derivatives(&k, Some(&s), uv).unwrap();
        assert_eq!((q.x, q.y, q.z), (p.x, p.y, p.z));
        assert_eq!(
            (du.x, du.y, du.z, dv.x, dv.y, dv.z),
            (1.0, 0.0, 0.0, 0.0, 1.0, 0.0)
        );
    }

    #[test]
    fn notify_named_primitive_box_builds_nonempty_trees_and_classifies_interior() {
        let mut k = Kernel::default();
        let box_brep =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 1.0, 1.0, 1.0)
                .unwrap();
        assert_eq!(box_brep.naming(), NamingScheme::Notify);

        let query = BrepQuery::of_brep(&box_brep);
        let cache = query.cache(&k, &box_brep).unwrap();
        assert!(cache.vertex_tree().is_some());
        assert!(cache.curve_tree().is_some());
        assert!(cache.surface_tree().is_some());

        let inf = box_brep.infinite_region().unwrap();
        let mut classifier = BrepClassifier::of_brep(&box_brep);
        let inside = classifier
            .point_3d_classify(&k, Point3::new(0.5, 0.5, 0.5), 1e-5, false)
            .unwrap();
        assert_eq!(inside.point_class(), PointClassType::Region);
        let Some(EntityRef::Region(r_in)) = inside.object() else {
            panic!("expected region classification, got {inside:?}");
        };
        assert_ne!(r_in, inf);
        assert_eq!(classifier.brep().region(r_in).unwrap().is_void(), 0);

        let outside = classifier
            .point_3d_classify(&k, Point3::new(2.0, 2.0, 2.0), 1e-5, false)
            .unwrap();
        assert_eq!(outside.point_class(), PointClassType::Region);
        assert_eq!(outside.object(), Some(EntityRef::Region(inf)));
    }
}
