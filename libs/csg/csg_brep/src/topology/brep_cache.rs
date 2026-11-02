//! Spatial trees of B-rep vertices, curves and surfaces.
//!
//! Upstream: `source/SMLib/inc/SmBrepCache.h`, `source/SMLib/src/SmBrepCache.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`BrepCache::new`] | `SmBrepCache::SmBrepCache` |
//! | [`BrepCache::build_trees`] | `SmBrepCache::BuildTrees` |
//! | [`BrepCache::resize_trees`] | `SmBrepCache::ReSizeTrees` |
//! | [`BrepCache::vertex_tree`], [`BrepCache::curve_tree`], [`BrepCache::surface_tree`] | `GetVertexTree`, `GetCurveTree`, `GetSurfaceTree` |
//! | [`make_brep_cache`] | `SmSAGObject::CacheMakeOrValidate` (new-cache path) |
//! | [`BrepObjects`] | `SmTopologySolver` object/cache access |
//! | [`TYPE`], [`is_kind_of`] | `GetType`, `IsKindOf` |
//! | `Drop` (implicit) | `SmBrepCache::~SmBrepCache` |
//!
//! Deviations: trees own typed object handles instead of pointers. The source is
//! supplied when building, so the cache does not borrow its geometry permanently;
//! the caller retains source identity and invalidates the cache after edits. Rust
//! drops trees automatically. Exact B-rep lists remain explicit
//! `topology.brep_p1` back-edges; arena allocation order is not list order.
//! Rebuilding an initialized cache and resizing an
//! uninitialized cache return `PortInvariant` (upstream asserts; as do
//! the adapter's handle and owner checks, which upstream follows by pointer). Ignored statuses
//! propagate Unported; an ordinary ignored box failure keeps the previous box,
//! as upstream. No new recursion; spatial-tree traversal uses geom.tree's bounds.
//! Face queries (`topology.face_p4`) remain explicit back-edges; the B-rep box calls
//! `query::brep`, face boxes `query::face::p1`; the polygon path is ported.

use crate::error::{Result, SolidError};
use crate::geom::a_object::{BREP_TYPE, POLY_BREP_TYPE};
use crate::geom::extent3d::Extent3d;
use crate::geom::object;
use crate::geom::tree::{NodeGeomType, Tree, TreeOwner};
use crate::kernel::Kernel;
use crate::math::EFF_ZERO;
use crate::nurbs::b_spline_curve::BoundingBoxRequest;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::query::poly::{PolyBrep, PolyEdgeId, PolyFaceId, PolyVertexId};
use crate::topology::{edge, Brep, EdgeId, FaceId, VertexId};

const OP: &str = "topology::brep_cache";

/// Whether `e` is this module's own identity or handle error: an owner, handle or tree
/// the B-rep cache could not resolve under the caller's identities. Upstream addresses
/// these objects by pointer, so no upstream status corresponds to it.
pub(crate) fn is_identity_error(e: &SolidError) -> bool {
    matches!(e, SolidError::PortInvariant { operation, .. } if *operation == OP)
}
/// `SmBrepCache_TYPE`.
pub const TYPE: i64 = 16400;
/// `SmBrepCache::GetTypeString`.
pub const TYPE_STRING: &str = "SmBrepCache_TYPE";
/// `SmBrepCache::GetClassString`.
pub const CLASS_STRING: &str = "SmBrepCache";

/// `SmBrepCache::IsKindOf` delegates directly to `SmObject`, skipping `SmCacheObj`.
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || object::is_kind_of(t)
}

/// Geometry held in a B-rep cache; curve and surface ids name their owning records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheObject {
    /// An owner outside the adapter's exact topology records.
    Other(object::ObjectRef),
    /// The source B-rep of a topology solver.
    Brep,
    /// An edge record (distinct from its curve).
    Edge(EdgeId),
    /// A face record (distinct from its surface).
    Face(FaceId),
    /// An exact vertex.
    Vertex(VertexId),
    /// A curve belonging to an edge.
    Curve(EdgeId),
    /// A surface belonging to a face.
    Surface(FaceId),
    /// A polygon vertex.
    PolyVertex(PolyVertexId),
    /// A polygon edge.
    PolyEdge(PolyEdgeId),
    /// A polygon face.
    PolyFace(PolyFaceId),
}

/// The source object of `BuildTrees`.
#[derive(Clone, Copy)]
pub enum CacheSource<'a> {
    /// An exact B-rep.
    Brep(&'a Brep),
    /// A polygonal B-rep.
    PolyBrep(&'a PolyBrep),
}

impl CacheSource<'_> {
    fn type_code(self) -> i64 {
        match self {
            Self::Brep(_) => BREP_TYPE,
            Self::PolyBrep(_) => POLY_BREP_TYPE,
        }
    }
}

/// `SmBrepCache`: three optional spatial trees. Object identities can be mapped
/// to a solver adapter's handles during construction.
#[derive(Debug, Clone, PartialEq)]
pub struct BrepCache<O = CacheObject> {
    target_type: i64,
    vertices: Option<Tree<O>>,
    curves: Option<Tree<O>>,
    surfaces: Option<Tree<O>>,
}

impl<O> BrepCache<O> {
    /// Constructs an empty cache for an `SmBrep` or `SmPolyBrep` type code.
    /// Other codes return upstream's `SM_ERR`.
    pub fn new(target_type: i64) -> Result<Self> {
        if target_type != BREP_TYPE && target_type != POLY_BREP_TYPE {
            return Err(SolidError::sm_err(OP));
        }
        Ok(Self {
            target_type,
            vertices: None,
            curves: None,
            surfaces: None,
        })
    }

    /// `GetVertexTree`: absent until `BuildTrees` succeeds.
    pub const fn vertex_tree(&self) -> Option<&Tree<O>> {
        self.vertices.as_ref()
    }
    /// `GetCurveTree`: absent until `BuildTrees` succeeds.
    pub const fn curve_tree(&self) -> Option<&Tree<O>> {
        self.curves.as_ref()
    }
    /// `GetSurfaceTree`: absent until `BuildTrees` succeeds.
    pub const fn surface_tree(&self) -> Option<&Tree<O>> {
        self.surfaces.as_ref()
    }
    /// Mutable `GetVertexTree` (the tree a B-rep notification edits).
    pub fn vertex_tree_mut(&mut self) -> Option<&mut Tree<O>> {
        self.vertices.as_mut()
    }
    /// Mutable `GetCurveTree` (the tree a B-rep notification edits).
    pub fn curve_tree_mut(&mut self) -> Option<&mut Tree<O>> {
        self.curves.as_mut()
    }
    /// Mutable `GetSurfaceTree` (the tree a B-rep notification edits).
    pub fn surface_tree_mut(&mut self) -> Option<&mut Tree<O>> {
        self.surfaces.as_mut()
    }
}

impl<O: Copy + PartialEq> BrepCache<O> {
    /// Builds all trees atomically in upstream vertex/edge/face order. `map`
    /// translates arena ids to caller-owned object handles, without changing order.
    /// `keys` identifies exact edge/face owners independently of arena numbering.
    pub fn build_trees(
        &mut self,
        k: &Kernel,
        source: CacheSource<'_>,
        mut map: impl FnMut(CacheObject) -> O,
        keys: &dyn Fn(CacheObject) -> object::ObjectRef,
    ) -> Result<()> {
        if self.vertices.is_some()
            || self.curves.is_some()
            || self.surfaces.is_some()
            || self.target_type != source.type_code()
        {
            return Err(SolidError::port_invariant(
                OP,
                "cache already built or source type mismatch",
            ));
        }
        let mut bbox = match source {
            CacheSource::Brep(b) => {
                let mut bbox = qbrep::calculate_bounding_box(k, b, BoundingBoxOptions::default())?;
                bbox.expand_absolute(b.tolerance() * 100.0);
                let size = bbox.size();
                bbox.add_point(bbox.max() + 0.15 * size);
                bbox.add_point(bbox.min() - 0.15 * size);
                bbox
            }
            CacheSource::PolyBrep(b) => {
                let mut bbox = b
                    .calculate_bounding_box(false, None, false, None)?
                    .normal_box;
                bbox.expand_absolute(b.tolerance() * 100.0);
                bbox
            }
        };
        // Preserve the source's inside-out empty box, including its arithmetic.
        let mut vertices = Tree::new_spatial(&bbox);
        let mut curves = Tree::new_spatial(&bbox);
        let mut surfaces = Tree::new_spatial(&bbox);
        for (tree, geom) in [
            (&mut vertices, None),
            (&mut curves, Some(NodeGeomType::LineSeg)),
            (&mut surfaces, Some(NodeGeomType::Polygon)),
        ] {
            tree.set_owner_object(Some(TreeOwner::BrepCache));
            if let Some(geom) = geom {
                let top = tree.top_node().ok_or(SolidError::null_pointer(OP))?;
                tree.node_mut(top)?.geom_type = geom;
            }
        }
        match source {
            CacheSource::PolyBrep(b) => {
                for v in b.poly_vertices() {
                    bbox = Extent3d::from_point(b.vertex_point(v)?);
                    bbox.expand_absolute(b.vertex_tolerance(v)?);
                    vertices.add_to_spatial_tree(
                        &bbox,
                        Some(map(CacheObject::PolyVertex(v))),
                        NodeGeomType::Default,
                    )?;
                }
                for e in b.poly_edges(None)? {
                    bbox = b.edge_bounding_box(e)?;
                    bbox.expand_absolute(b.edge_tolerance(e)?);
                    curves.add_to_spatial_tree(
                        &bbox,
                        Some(map(CacheObject::PolyEdge(e))),
                        NodeGeomType::LineSeg,
                    )?;
                }
                for f in b.poly_faces(None)? {
                    bbox = b.face_bounding_box(f)?;
                    surfaces.add_to_spatial_tree(
                        &bbox,
                        Some(map(CacheObject::PolyFace(f))),
                        NodeGeomType::Polygon,
                    )?;
                }
            }
            CacheSource::Brep(b) => {
                for v in brep_vertices(b)? {
                    let data = b.vertex(v)?;
                    bbox = Extent3d::from_point(data.point);
                    bbox.expand_absolute(data.tolerance);
                    vertices.add_to_spatial_tree(
                        &bbox,
                        Some(map(CacheObject::Vertex(v))),
                        NodeGeomType::Default,
                    )?;
                }
                for e in brep_edges(b)? {
                    let data = b.edge(e)?;
                    let curve = data.curve.as_ref().ok_or(SolidError::null_pointer(OP))?;
                    if curve.owner() == Some(keys(CacheObject::Edge(e))) {
                        bbox = curve
                            .calculate_bounding_box(
                                k,
                                &data.interval,
                                BoundingBoxRequest {
                                    normal: true,
                                    pseudo: false,
                                    polar: false,
                                },
                                true,
                            )?
                            .normal
                            .ok_or(SolidError::null_pointer(OP))?;
                        bbox.expand_absolute(data.tolerance);
                        curves.add_to_spatial_tree(
                            &bbox,
                            Some(map(CacheObject::Curve(e))),
                            NodeGeomType::Default,
                        )?;
                    }
                }
                for f in brep_faces(b)? {
                    let data = b.face(f)?;
                    let surface = data.surface.as_ref().ok_or(SolidError::null_pointer(OP))?;
                    if surface.owner() == Some(keys(CacheObject::Face(f))) {
                        bbox = face_bounding_box(k, b, f)?;
                        bbox.expand_absolute(data.tolerance);
                        surfaces.add_to_spatial_tree(
                            &bbox,
                            Some(map(CacheObject::Surface(f))),
                            NodeGeomType::Default,
                        )?;
                    }
                }
            }
        }
        self.vertices = Some(vertices);
        self.curves = Some(curves);
        self.surfaces = Some(surfaces);
        Ok(())
    }

    /// Expands the three trees to include exact faces, edges and vertices. This
    /// does not insert objects; all entity tolerances contribute to the SAME
    /// maximum used for every tree, as upstream. Empty slices are null arrays.
    /// `keys` supplies the identities used by the geometry owner links.
    /// `owner_of` resolves intermediate generated-curve and edgeuse owners.
    pub fn resize_trees(
        &mut self,
        k: &Kernel,
        b: &Brep,
        faces: &[FaceId],
        edges: &[EdgeId],
        vertices: &[VertexId],
        keys: &dyn Fn(CacheObject) -> object::ObjectRef,
        owner_of: &dyn Fn(object::ObjectRef) -> Option<object::ObjectRef>,
    ) -> Result<()> {
        if self.vertices.is_none() || self.curves.is_none() || self.surfaces.is_none() {
            return Err(SolidError::port_invariant(OP, "cache has not been built"));
        }
        let mut surface_union = Extent3d::new();
        let mut curve_union = Extent3d::new();
        let mut vertex_union = Extent3d::new();
        let mut surface_box = Extent3d::new();
        let mut curve_box = Extent3d::new();
        let mut tol = 0.0;
        for &f in faces {
            let face = b.face(f)?;
            // The original surface's owning face, not necessarily the input face.
            let owner = face
                .surface
                .as_ref()
                .ok_or(SolidError::null_pointer(OP))?
                .face()?
                .ok_or(SolidError::null_pointer(OP))?;
            let owner = face_id(b, owner, keys)?;
            match face_bounding_box(k, b, owner) {
                Ok(bbox) => surface_box = bbox,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {}
            }
            surface_union = surface_union.union(&surface_box);
            if face.tolerance > tol {
                tol = face.tolerance;
            }
            for e in face_edges(b, f)? {
                resize_edge(
                    k,
                    b,
                    e,
                    &mut curve_box,
                    &mut curve_union,
                    &mut vertex_union,
                    &mut tol,
                    keys,
                    owner_of,
                )?;
            }
        }
        for &e in edges {
            resize_edge(
                k,
                b,
                e,
                &mut curve_box,
                &mut curve_union,
                &mut vertex_union,
                &mut tol,
                keys,
                owner_of,
            )?;
        }
        for &v in vertices {
            let vertex = b.vertex(v)?;
            if vertex.tolerance > tol {
                tol = vertex.tolerance;
            }
            vertex_union.add_point(vertex.point);
        }
        for (tree, mut bbox, containment_tol) in [
            (&mut self.surfaces, surface_union, 0.0),
            (&mut self.curves, curve_union, 0.0),
            (&mut self.vertices, vertex_union, EFF_ZERO),
        ] {
            if !bbox.has_negative_volume() {
                bbox.expand_absolute(100.0 * tol);
                let tree = tree.as_mut().ok_or(SolidError::null_pointer(OP))?;
                let top = tree.top_node().ok_or(SolidError::null_pointer(OP))?;
                if !bbox.is_contained_by(&tree.node(top)?.bbox, containment_tol) {
                    crate::error::ignore_status(tree.add_to_spatial_tree(
                        &bbox,
                        None,
                        NodeGeomType::Default,
                    ))?;
                }
            }
        }
        Ok(())
    }
}

fn resize_edge(
    k: &Kernel,
    b: &Brep,
    e: EdgeId,
    curve_box: &mut Extent3d,
    curve_union: &mut Extent3d,
    vertex_union: &mut Extent3d,
    tol: &mut f64,
    keys: &dyn Fn(CacheObject) -> object::ObjectRef,
    owner_of: &dyn Fn(object::ObjectRef) -> Option<object::ObjectRef>,
) -> Result<()> {
    let data = b.edge(e)?;
    if data.tolerance > *tol {
        *tol = data.tolerance;
    }
    let owner = data
        .curve
        .as_ref()
        .ok_or(SolidError::null_pointer(OP))?
        .edge(owner_of)?
        .ok_or(SolidError::null_pointer(OP))?;
    let owner = edge_id(b, owner, keys)?;
    match crate::query::edge::calculate_bounding_box(
        k,
        b,
        owner,
        BoundingBoxRequest {
            normal: true,
            pseudo: false,
            polar: false,
        },
        false,
    ) {
        Ok(boxes) => {
            if let Some(bbox) = boxes.normal {
                *curve_box = bbox;
            }
        }
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => {}
    }
    *curve_union = curve_union.union(curve_box);
    let first = edge::vertex(b, e)?.ok_or(SolidError::null_pointer(OP))?;
    let second = edge::other_vertex(b, e, Some(first))?.ok_or(SolidError::null_pointer(OP))?;
    for v in [first, second] {
        let data = b.vertex(v)?;
        if data.tolerance > *tol {
            *tol = data.tolerance;
        }
        vertex_union.add_point(data.point);
    }
    Ok(())
}

// (SmBrep::GetVertices / GetEdges / GetFaces are wired to topology::brep::p1)
fn brep_vertices(b: &Brep) -> Result<Vec<VertexId>> {
    crate::topology::brep::p1::vertices(b, None)
}

fn brep_edges(b: &Brep) -> Result<Vec<EdgeId>> {
    crate::topology::brep::p1::edges(b, None)
}

fn brep_faces(b: &Brep) -> Result<Vec<FaceId>> {
    crate::topology::brep::p1::faces(b, None)
}

fn face_bounding_box(k: &Kernel, b: &Brep, f: FaceId) -> Result<Extent3d> {
    crate::query::face::p1::calculate_bounding_box(k, b, f, false)
}

fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, false, None)
}

/// Builds a new cache for `SmSAGObject::CacheMakeOrValidate`. The source geometry
/// is required, as for the curve/surface cache factories.
pub fn make_brep_cache(
    k: &Kernel,
    source: CacheSource<'_>,
    keys: &dyn Fn(CacheObject) -> object::ObjectRef,
) -> Result<BrepCache> {
    let mut cache = BrepCache::new(source.type_code())?;
    cache.build_trees(k, source, |object| object, keys)?;
    Ok(cache)
}

impl crate::geom::solution_array::SolutionObject for CacheObject {
    fn kind(&self) -> crate::geom::solution_array::ObjectKind {
        use crate::geom::solution_array::ObjectKind;
        match self {
            Self::Vertex(_) => ObjectKind::Vertex,
            Self::Curve(_) => ObjectKind::Curve,
            Self::Surface(_) => ObjectKind::Surface,
            Self::Edge(_) => ObjectKind::Edge,
            Self::Face(_) => ObjectKind::Face,
            Self::PolyVertex(_) => ObjectKind::PolyVertex,
            Self::PolyEdge(_) => ObjectKind::PolyEdge,
            Self::PolyFace(_) => ObjectKind::PolyFace,
            Self::Brep | Self::Other(_) => ObjectKind::Other,
        }
    }
}

/// Exact B-rep geometry and its cache as seen by the topology solver. Cache
/// invalidation is the caller's responsibility; both borrows must name one B-rep.
pub struct BrepObjects<'a> {
    /// A mutable caller may resolve trim requests after the read-only solve stops.
    pub preparation: Option<&'a crate::query::brep::RayPreparation>,
    /// Source arenas.
    pub brep: &'a Brep,
    /// Identity of the source B-rep in the caller's shape store.
    pub brep_id: crate::topology::BrepId,
    /// Cache built from these arenas with identity handles. Shape solves read the
    /// shape's own trees, so a view for them needs none (`None`).
    pub cache: Option<&'a BrepCache>,
    /// Identity of each edge/face record (independent of its arena index).
    pub keys: &'a dyn Fn(CacheObject) -> object::ObjectRef,
    /// Owners of intermediate generated curves and edge uses, as for `Curve::edge`.
    pub owner_of: &'a dyn Fn(object::ObjectRef) -> Option<object::ObjectRef>,
}

impl crate::intersect::topology_solver::p1::TopologyObjects<CacheObject> for BrepObjects<'_> {
    fn surface_solve_context(
        &self,
    ) -> Option<Box<dyn crate::surface::surface::p1::SurfaceSolveContext<CacheObject> + '_>> {
        self.preparation.map(|preparation| {
            Box::new(crate::query::brep::RaySurfaceContext {
                brep: self.brep,
                preparation,
            }) as Box<dyn crate::surface::surface::p1::SurfaceSolveContext<CacheObject>>
        })
    }

    fn uv_trim_preparation(&self) -> Option<&crate::query::brep::RayPreparation> {
        self.preparation
    }
    fn brep_cache(&self, object: CacheObject) -> Result<&BrepCache> {
        if object != CacheObject::Brep {
            return Err(SolidError::port_invariant(OP, "expected B-rep handle"));
        }
        self.cache.ok_or(SolidError::null_pointer(OP))
    }
    fn curve(&self, object: CacheObject) -> Result<&crate::curve::curve::Curve> {
        let CacheObject::Curve(e) = object else {
            return Err(SolidError::port_invariant(OP, "expected curve handle"));
        };
        self.brep
            .edge(e)?
            .curve
            .as_ref()
            .ok_or(SolidError::null_pointer(OP))
    }
    fn analytic_curve_handle(&self, object: CacheObject) -> Option<CacheObject> {
        Some(object)
    }
    fn vertex_point(&self, object: CacheObject) -> Result<crate::math::Point3> {
        let CacheObject::Vertex(v) = object else {
            return Err(SolidError::port_invariant(OP, "expected vertex handle"));
        };
        Ok(self.brep.vertex(v)?.point)
    }
    fn vertex_tolerance(&self, object: CacheObject) -> Result<f64> {
        let CacheObject::Vertex(v) = object else {
            return Err(SolidError::port_invariant(OP, "expected vertex handle"));
        };
        Ok(self.brep.vertex(v)?.tolerance)
    }
    fn curve_edge(&self, object: CacheObject) -> Result<Option<CacheObject>> {
        // a solve's own curve outside the B-rep's records (e.g. the line a caller
        // intersects the B-rep with): no edge of the B-rep owns it, so its
        // `GetEdge()` is null
        if let CacheObject::Other(_) = object {
            return Ok(None);
        }
        let owner = self.curve(object)?.edge(self.owner_of)?;
        owner
            .map(|o| edge_id(self.brep, o, self.keys).map(CacheObject::Edge))
            .transpose()
    }
    fn edge_curve(&self, object: CacheObject) -> Result<Option<CacheObject>> {
        let CacheObject::Edge(e) = object else {
            return Err(SolidError::port_invariant(OP, "expected edge handle"));
        };
        Ok(self
            .brep
            .edge(e)?
            .curve
            .as_ref()
            .map(|_| CacheObject::Curve(e)))
    }
    fn edge_interval(&self, object: CacheObject) -> Result<crate::geom::extent1d::Extent1d> {
        let CacheObject::Edge(e) = object else {
            return Err(SolidError::port_invariant(OP, "expected edge handle"));
        };
        Ok(self.brep.edge(e)?.interval)
    }
    fn edge_tolerance(&self, object: CacheObject) -> Result<f64> {
        let CacheObject::Edge(e) = object else {
            return Err(SolidError::port_invariant(OP, "expected edge handle"));
        };
        Ok(self.brep.edge(e)?.tolerance)
    }
    fn surface_face(&self, object: CacheObject) -> Result<Option<CacheObject>> {
        let owner = self.surface(object)?.face()?;
        owner
            .map(|o| face_id(self.brep, o, self.keys).map(CacheObject::Face))
            .transpose()
    }
    fn face_surface(&self, object: CacheObject) -> Result<CacheObject> {
        let CacheObject::Face(f) = object else {
            return Err(SolidError::port_invariant(OP, "expected face handle"));
        };
        self.brep
            .face(f)?
            .surface
            .as_ref()
            .ok_or(SolidError::null_pointer(OP))?;
        Ok(CacheObject::Surface(f))
    }
    fn surface(&self, object: CacheObject) -> Result<&crate::surface::surface::Surface> {
        let CacheObject::Surface(f) = object else {
            return Err(SolidError::port_invariant(OP, "expected surface handle"));
        };
        self.brep
            .face(f)?
            .surface
            .as_ref()
            .ok_or(SolidError::null_pointer(OP))
    }
    fn face_uv_domain(&self, object: CacheObject) -> Result<crate::geom::extent2d::Extent2d> {
        let CacheObject::Face(f) = object else {
            return Err(SolidError::port_invariant(OP, "expected face handle"));
        };
        Ok(self.brep.face(f)?.uv_domain)
    }
    fn face_tolerance(&self, object: CacheObject) -> Result<f64> {
        let CacheObject::Face(f) = object else {
            return Err(SolidError::port_invariant(OP, "expected face handle"));
        };
        Ok(self.brep.face(f)?.tolerance)
    }
    fn face_entity(&self, object: CacheObject) -> Option<(&Brep, FaceId)> {
        match object {
            CacheObject::Face(f) => Some((self.brep, f)),
            _ => None,
        }
    }
    fn surface_owner(&self, object: CacheObject) -> Result<Option<CacheObject>> {
        self.surface(object)?
            .owner()
            .map(|owner| {
                if owner.type_code == crate::topology::face::p2::TYPE {
                    face_id(self.brep, owner, self.keys).map(CacheObject::Face)
                } else {
                    Ok(CacheObject::Other(owner))
                }
            })
            .transpose()
    }
    fn object_zone_tol_3d(&self, k: &Kernel, object: CacheObject) -> Result<f64> {
        use crate::geom::tol::{zone_tol_3d, SmallTopoType};
        let system = zone_tol_3d(k, SmallTopoType::NormalSize);
        match object {
            // topology: GetTolerance (SM_USE_OLDTOL)
            CacheObject::Vertex(v) => crate::topology::vertex::tolerance(self.brep, v),
            CacheObject::Edge(e) => edge::tolerance(self.brep, e),
            CacheObject::Face(f) => crate::topology::face::p2::tolerance(self.brep, f),
            // geometry: the tolerance of its edge or face, else the system's
            CacheObject::Curve(_) => match self.curve_edge(object)? {
                Some(CacheObject::Edge(e)) => edge::tolerance(self.brep, e),
                Some(_) => Err(SolidError::port_invariant(OP, "curve edge is not an edge")),
                None => Ok(system),
            },
            CacheObject::Surface(_) => match self.surface_face(object)? {
                Some(CacheObject::Face(f)) => crate::topology::face::p2::tolerance(self.brep, f),
                Some(_) => Err(SolidError::port_invariant(OP, "surface face is not a face")),
                None => Ok(system),
            },
            // SmBrep: GetZoneTol3d(const SmBrep*)
            CacheObject::Brep => Ok(self.brep.tolerance()),
            // UNPORTED(query.poly): SmPolyVertex/SmPolyEdge/SmPolyFace::GetTolerance (the
            // poly B-rep is not in this view)
            CacheObject::PolyVertex(_) | CacheObject::PolyEdge(_) | CacheObject::PolyFace(_) => {
                Err(SolidError::unported(
                    "SmPolyVertex::GetTolerance",
                    "query.poly",
                ))
            }
            // UNPORTED(geom.tol): SmTol::GetZoneTol3d(const SmObject*) on an owner outside
            // this view's topology records
            // Proof (unreachable): an owner outside this view's topology records has no library
            // path; 0 hits in the green difftest sweep.
            CacheObject::Other(_) => Err(SolidError::unported(
                "SmTol::GetZoneTol3d(const SmObject*)",
                "geom.tol",
            )),
        }
    }
    fn shape_member(&self, object: CacheObject) -> object::ShapeMember {
        use crate::geom::object::ShapeMember;
        let b = self.brep_id;
        match object {
            CacheObject::Vertex(v) => ShapeMember::Vertex((b, v)),
            CacheObject::Edge(e) => ShapeMember::Edge((b, e)),
            CacheObject::Face(f) => ShapeMember::Face((b, f)),
            _ => ShapeMember::Other,
        }
    }
    fn shape_member_object(&self, member: object::ShapeMember) -> Result<CacheObject> {
        use crate::geom::object::ShapeMember;
        let (brep, object) = match member {
            ShapeMember::Vertex((b, v)) => (b, CacheObject::Vertex(v)),
            ShapeMember::Edge((b, e)) => (b, CacheObject::Edge(e)),
            ShapeMember::Face((b, f)) => (b, CacheObject::Face(f)),
            ShapeMember::Other => {
                return Err(SolidError::port_invariant(OP, "not a shape member"));
            }
        };
        if brep != self.brep_id {
            return Err(SolidError::port_invariant(
                OP,
                "shape member of another B-rep",
            ));
        }
        Ok(object)
    }
}

impl crate::intersect::point_class::ClassObject for CacheObject {
    fn class_kind(&self) -> crate::intersect::point_class::ClassObjectKind {
        use crate::intersect::point_class::ClassObjectKind;
        match self {
            Self::Vertex(_) => ClassObjectKind::Vertex,
            Self::Curve(_) => ClassObjectKind::Curve,
            Self::Surface(_) => ClassObjectKind::Surface,
            Self::Edge(_) => ClassObjectKind::Edge,
            Self::Face(_) => ClassObjectKind::Face,
            Self::Brep
            | Self::Other(_)
            | Self::PolyVertex(_)
            | Self::PolyEdge(_)
            | Self::PolyFace(_) => ClassObjectKind::Other,
        }
    }
}

/// Conventional identities for a single B-rep: the type code and raw arena index.
/// Callers using other `ObjectKey`s pass their own mapping to build/resize instead.
pub fn arena_identity(object: CacheObject) -> object::ObjectRef {
    use crate::geom::attribute::ObjectKey;
    let (type_code, key) = match object {
        CacheObject::Edge(e) | CacheObject::Curve(e) => (edge::TYPE, u64::from(e.index())),
        CacheObject::Face(f) | CacheObject::Surface(f) => {
            (crate::topology::face::p2::TYPE, u64::from(f.index()))
        }
        CacheObject::Vertex(v) => (crate::topology::vertex::TYPE, u64::from(v.index())),
        _ => (BREP_TYPE, 0),
    };
    object::ObjectRef {
        type_code,
        key: ObjectKey(key),
    }
}

fn edge_id(
    b: &Brep,
    owner: object::ObjectRef,
    keys: &dyn Fn(CacheObject) -> object::ObjectRef,
) -> Result<EdgeId> {
    b.edges()
        .iter()
        .find(|(id, _)| keys(CacheObject::Edge(*id)) == owner)
        .map(|(id, _)| id)
        .ok_or_else(|| SolidError::port_invariant(OP, "edge owner is not in source B-rep"))
}
fn face_id(
    b: &Brep,
    owner: object::ObjectRef,
    keys: &dyn Fn(CacheObject) -> object::ObjectRef,
) -> Result<FaceId> {
    b.faces()
        .iter()
        .find(|(id, _)| keys(CacheObject::Face(*id)) == owner)
        .map(|(id, _)| id)
        .ok_or_else(|| SolidError::port_invariant(OP, "face owner is not in source B-rep"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solver_adapter_uses_caller_owner_chain_and_brep_identity() {
        use crate::curve::curve::Curve;
        use crate::curve::line::Line;
        use crate::geom::attribute::ObjectKey;
        use crate::intersect::topology_solver::p1::TopologyObjects;
        use crate::math::Point3;
        use crate::topology::{BrepId, Edge, Vertex};

        let mut b = Brep::new();
        let a = b.add_vertex_record(Vertex::default()).unwrap();
        let z = b.add_vertex_record(Vertex::default()).unwrap();
        let e = b.add_edge_record(Edge::new([a, z])).unwrap();
        let keys = |o| {
            let mut key = arena_identity(o);
            key.key.0 += 1000000000;
            key
        };
        let generated = object::ObjectRef {
            type_code: crate::curve::curve::p2::TYPE,
            key: ObjectKey(500),
        };
        let edgeuse = object::ObjectRef {
            type_code: crate::topology::edgeuse::p1::TYPE,
            key: ObjectKey(600),
        };
        let owner_of = |o| {
            if o == generated {
                Some(edgeuse)
            } else if o == edgeuse {
                Some(keys(CacheObject::Edge(e)))
            } else {
                None
            }
        };
        let mut curve = Curve::from(
            Line::create_line_segment(3, Point3::ZERO, Point3::new(1.0, 0.0, 0.0), None).unwrap(),
        );
        curve.set_owner(Some(generated), None);
        b.edge_mut(e).unwrap().curve = Some(curve);
        let cache = BrepCache::new(BREP_TYPE).unwrap();
        let brep_id = BrepId::from_index(7);
        let mut objects = BrepObjects {
            preparation: None,
            brep: &b,
            brep_id,
            cache: Some(&cache),
            keys: &keys,
            owner_of: &owner_of,
        };
        assert_eq!(
            objects.curve_edge(CacheObject::Curve(e)).unwrap(),
            Some(CacheObject::Edge(e))
        );
        assert_eq!(
            objects.shape_member(CacheObject::Edge(e)),
            object::ShapeMember::Edge((brep_id, e))
        );
        objects.keys = &arena_identity;
        assert!(matches!(
            objects.curve_edge(CacheObject::Curve(e)),
            Err(SolidError::PortInvariant { .. })
        ));
    }

    #[test]
    fn initialization_contract_and_atomic_build_failure() {
        let k = Kernel::default();
        assert!(matches!(
            BrepCache::<CacheObject>::new(0),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        let mut cache = BrepCache::new(BREP_TYPE).unwrap();
        assert!(cache.vertex_tree().is_none());
        let b = Brep::new();
        assert!(matches!(
            cache.resize_trees(&k, &b, &[], &[], &[], &arena_identity, &|_| None),
            Err(SolidError::PortInvariant { .. })
        ));
        // an edge without curve fails the build after the B-rep box, leaving no tree
        let mut open = Brep::new();
        let v0 = open.add_vertex(crate::math::Point3::ZERO).unwrap();
        let v1 = open
            .add_vertex(crate::math::Point3::new(1.0, 0.0, 0.0))
            .unwrap();
        open.add_edge(v0, v1).unwrap();
        assert!(matches!(
            cache.build_trees(&k, CacheSource::Brep(&open), |o| o, &arena_identity),
            Err(SolidError::Upstream { code: 1006, .. })
        ));
        assert!(
            cache.vertex_tree().is_none()
                && cache.curve_tree().is_none()
                && cache.surface_tree().is_none()
        );
        // an empty B-rep builds (its box is the inside-out empty box)
        let mut empty = BrepCache::new(BREP_TYPE).unwrap();
        empty
            .build_trees(&k, CacheSource::Brep(&b), |o| o, &arena_identity)
            .unwrap();
        assert!(empty.vertex_tree().is_some());
        let p = PolyBrep::new(&k, 1e-6);
        assert!(matches!(
            cache.build_trees(&k, CacheSource::PolyBrep(&p), |o| o, &arena_identity),
            Err(SolidError::PortInvariant { .. })
        ));
    }

    #[test]
    fn resize_retains_object_identity_and_rejects_dangling_ids() {
        let k = Kernel::default();
        let mut p = PolyBrep::new(&k, 1e-6);
        let v = p.add_vertex(&k, crate::math::Point3::ZERO, 1e-6).unwrap();
        let mut cache = make_brep_cache(&k, CacheSource::PolyBrep(&p), &arena_identity).unwrap();
        assert!(matches!(
            cache.build_trees(&k, CacheSource::PolyBrep(&p), |o| o, &arena_identity),
            Err(SolidError::PortInvariant { .. })
        ));
        let mut b = Brep::new();
        let extra = b
            .add_vertex_record(crate::topology::Vertex {
                point: crate::math::Point3::new(100.0, 0.0, 0.0),
                tolerance: 0.25,
                ..crate::topology::Vertex::default()
            })
            .unwrap();
        cache
            .resize_trees(&k, &b, &[], &[], &[extra], &arena_identity, &|_| None)
            .unwrap();
        let t = cache.vertex_tree().unwrap();
        let list = t.object_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            t.list(list[0]).unwrap().object,
            Some(CacheObject::PolyVertex(v))
        );
        assert!(t.node(t.top_node().unwrap()).unwrap().bbox.max().x >= 125.0);
        assert!(matches!(
            cache.resize_trees(
                &k,
                &b,
                &[],
                &[],
                &[VertexId::from_index(u32::MAX)],
                &arena_identity,
                &|_| None,
            ),
            Err(SolidError::NotFound { .. })
        ));
    }
}
