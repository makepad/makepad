//! The tessellation surface cache `SmTessSrfCache`: a trimmed surface cache that
//! subdivides a face's surface to the tessellation tolerances (tighter on the
//! silhouette of a view direction), embeds the face's poly-edge boundary into the
//! leaves and classifies every leaf inside, outside or on the boundary; plus the
//! surface tessellation drivers and the poly-to-B-rep attributes.
//!
//! Upstream: `source/SMLib/src/SmTess.cpp`, `source/SMLib/inc/SmTess.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`TessSrfCache`], [`TessHandle`]                  | `SmTessSrfCache` (and the pointers it stores)    |
//! | [`TessSrfCache::new`]                             | `SmTessSrfCache::SmTessSrfCache`                 |
//! | `Drop` (implicit)                                 | `SmTessSrfCache::~SmTessSrfCache`                |
//! | [`TessSrfCache::face`], [`TessSrfCache::ts_poly_brep`] | `GetFace`, `GetTS_PolyBrep`                 |
//! | [`TYPE`], [`TYPE_STRING`], [`TessSrfCache::type_code`], [`TessSrfCache::type_string`] | `SmTessSrfCache::GetType`, `GetTypeString` (`SM_COMMON`) |
//! | [`is_kind_of`], [`TessSrfCache::is_kind_of`]      | `SmTessSrfCache::IsKindOf`                       |
//! | [`TessSrfCache::test_against_tolerances`]         | `SmTessSrfCache::TestAgainstTolerances`          |
//! | [`TessSrfCache::build_tree_with_subdivision`]     | `SmTessSrfCache::BuildTreeWithSubdivision`       |
//! | [`TessSrfCache::build_tree`]                      | `SmTessSrfCache::BuildTree`                      |
//! | [`TessSrfCache::implant_uv_segment`]              | `SmTessSrfCache::ImplantUVSegment`               |
//! | [`TessSrfCache::mark_face_containment`]           | `SmTessSrfCache::MarkFaceContainment`            |
//! | [`SurfaceTessDriver`], [`SurfaceTessDriver::new`], [`SurfaceTessDriverOptions`] | `SmSurfaceTessDriver`, its constructor and default arguments |
//! | [`SurfaceTessDriver::view_based`], [`ViewBasedTessDriverOptions`], [`ViewBased`] | `SmViewBasedTessDriver`, its constructor and default arguments |
//! | [`SurfaceTessDriver::type_code`], [`SURFACE_TESS_DRIVER_TYPE`], [`VIEW_BASED_TESS_DRIVER_TYPE`] | `SmSurfaceTessDriver::GetType`, `SmViewBasedTessDriver::GetType` |
//! | [`SurfaceTessDriver::view_vector`], [`SurfaceTessDriver::silhouette_chord_height`], [`SurfaceTessDriver::silhouette_angle_tolerance`] | `SmViewBasedTessDriver::GetViewVector`, `GetSilhouetteChordHeight`, `GetSilhouetteAngleTolerance` |
//! | [`poly_to_euvu_attribute`], [`AttributeValue::PolyToEuVu`] | `SmPolyToEUVUAttr::SmPolyToEUVUAttr` (`~` is `Drop`) |
//! | [`poly_edge_to_vertex_use_attribute`], [`AttributeValue::PolyEdgeToVertexUse`] | `SmPolyEdgeToVertexUseAttr::SmPolyEdgeToVertexUseAttr` (`~` is `Drop`) |
//! | [`Attribute::make_copy`] (unit `geom.attribute`)  | `SmPolyToEUVUAttr::MakeCopy`, `SmPolyEdgeToVertexUseAttr::MakeCopy` |
//!
//! Deviations:
//! - **Inheritance.** `SmTessSrfCache` derives from `SmTrimSrfCache`; here it wraps a
//!   [`TrimSrfCache`] over [`TessHandle`]s ([`TessSrfCache::trim`],
//!   [`TessSrfCache::base`]). Its constructor passes `dAngleTolerance` to the base's
//!   `dAngTolRad` unconverted, as upstream; the six tolerances are a
//!   [`SurfaceCacheOptions`].
//! - **Virtual dispatch.** `SubdivideToTolerances` (also inside the base class
//!   `BuildTree` this cache runs for non B-spline surfaces) calls the virtual
//!   `TestAgainstTolerances`: the port hands [`TessSrfCache::test_against_tolerances`]
//!   to [`SurfaceCache::subdivide_to_tolerances_with`] /
//!   [`SurfaceCache::build_tree_with`]. `GetTree` (virtual `BuildTree`) is run by this
//!   cache before `FindUVNodes` and in `MarkFaceContainment`; `FindUVNodes` then
//!   searches the built tree ([`SurfaceCache::find_uv_nodes_built`]).
//! - **Ownership and handles.** The cache owns its surface, its poly B-rep
//!   (`mTS_pPolyBrep`, deleted by the destructor upstream) and a copy of its surface
//!   tessellation driver (upstream keeps the caller's pointer; a missing driver is
//!   `None`). The face is an opaque [`ObjectKey`]. Node lists and `mBA_pSurface`
//!   hold [`TessHandle`]s: [`TessHandle::CacheSurface`] for the cache surface,
//!   [`TessHandle::Subdivision`] for the temporary subdivision surfaces (named in
//!   creation order), [`TessHandle::PolyEdge`] for poly edges of the cache's poly
//!   B-rep and [`TessHandle::Object`] for the original face of a poly face.
//! - **Face surface.** `m_pFace->GetSurface()` (unit `topology.face_p2`) is read as
//!   the cache's surface: the constructor builds the cache from that surface, so they
//!   are the same object upstream. `SM_CAST_PTR(SmBSplineSurface, pSurface)` holds for
//!   B-splines, planes and surfaces of revolution (their B-spline part is copied, as
//!   the slicing `SmBSplineSurface` copy constructor does, with the NURB that
//!   `GetOrCreateGwNurbPointer` makes when it is missing; upstream makes it on the face
//!   surface itself, the port on the copy; a failed `MakeNurb` copies the part as is).
//! - **Temporary state.** `SmTemporaryChangeValue` becomes explicit save / restore,
//!   also on errors; the face containment guard of `BuildTree` is cleared on success.
//! - **Ignored statuses** stay ignored (`CalculateBoundingBox` of the subdivision copy,
//!   the base class `BuildTree` for non B-spline surfaces), except
//!   [`SolidError::Unported`], which is always propagated.
//! - **Loops.** The parent walks of `MarkFaceContainment` stop with
//!   [`SolidError::InvalidInput`] after as many steps as the tree has nodes (a cyclic
//!   parent chain, which upstream follows forever); so does its descent from the top
//!   node after as many visits (a shared or cyclic child link, whose parent does not
//!   point back; fuzz `query.tess` fuzz00241, fuzz00353).
//! - **Recursion.** `BuildTree` implants the boundary through `GetTree`, which builds
//!   again when there is no tree. When the (ignored) base-class build left no tree,
//!   upstream recurses forever; here `GetTree` returns [`SolidError::InvalidInput`]
//!   when it finds no tree while a build is under way. Every other `GetTree` inside a
//!   build finds the tree, so the nesting is at most 1.
//! - **Undefined behaviour.** Where upstream dereferences a null tree, top node,
//!   poly B-rep, poly face or tessellation driver, or a node without
//!   `SmBezierAux2d` data, the port returns `SM_ERR_NULL_POINTER` or
//!   [`SolidError::InvalidInput`]. `SmPolyEdgeToVertexUseAttr` lives in `SmTess.cpp`
//!   only (no header), so it is unit tested, not compared with the C++.
//! - **Not ported here.** `RefineMeshBoundaries` (compiled out:
//!   `SM_USE_TESS_REFINE_MESH` is not defined), `FindUVNodesOfClass`, the `Draw` and
//!   `Dump` functions, `GetMemoryUsed`, and the members of `SmTess` /
//!   `SmCurveTessDriver` (other units).

use crate::error::{Result, SolidError};
use crate::geom::attribute::{Attribute, AttributeValue, Behavior, ObjectKey};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::tree::{
    AuxDataType, NodeClass, NodeData, NodeGeomType, NodeId, SplitDir, SurfaceAux, Tree, TreeNode,
    TreeOwner,
};
use crate::kernel::Kernel;
use crate::math::{min2, Point2, Point3, Vec2, Vec3};
use crate::nurbs::b_spline_surface::{BSplineSurface, BoundingBoxRequest};
use crate::nurbs::knots::Side;
use crate::query::poly::{PolyBrep, PolyEdgeId};
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::surface_cache::p1::SurfaceCacheOptions;
use crate::surface::surface_cache::p2::ToleranceTest;
use crate::surface::surface_cache::{BezierAux2d, SurfaceCache};
use crate::surface::trim_srf_cache::{self as trim, TrimSrfCache};

/// `SmTessSrfCache_TYPE` (`SURF_BASE_TYPE + 150`).
pub const TYPE: i64 = 15150;

/// `SmTessSrfCache::GetTypeString`.
pub const TYPE_STRING: &str = "SmTessSrfCache_TYPE";

/// `SmFace_TYPE` (the owner type `GetTree` looks for).
const FACE_TYPE: i64 = 16002;

/// `SmSurfaceTessDriver_TYPE` (`SURF_BASE_TYPE + 152`).
pub const SURFACE_TESS_DRIVER_TYPE: i64 = 15152;

/// `SmViewBasedTessDriver_TYPE` (`SURF_BASE_TYPE + 154`).
pub const VIEW_BASED_TESS_DRIVER_TYPE: i64 = 15154;

const OP_TEST: &str = "query::tess::TessSrfCache::test_against_tolerances";
const OP_SUB: &str = "query::tess::TessSrfCache::build_tree_with_subdivision";
const OP_BUILD: &str = "query::tess::TessSrfCache::build_tree";
const OP_UV: &str = "query::tess::TessSrfCache::implant_uv_segment";
const OP_MARK: &str = "query::tess::TessSrfCache::mark_face_containment";
const OP_GET_TREE: &str = "query::tess::TessSrfCache::get_tree";

/// `SmTessSrfCache::IsKindOf`: the tessellation cache or a base type.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || trim::is_kind_of(t)
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// Keeps [`SolidError::Unported`], drops any other error of an ignored status.
fn ignore<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

// ---- attributes ---------------------------------------------------------------------

/// `SmPolyToEUVUAttr(lAttributeID, eBehavior)` (upstream default
/// [`Behavior::Copy`]): no edge use, no vertex use. Its `MakeCopy` is
/// [`Attribute::make_copy`] (same id, behaviour and pointers).
#[must_use]
pub fn poly_to_euvu_attribute(attribute_id: u64, behavior: Behavior) -> Attribute {
    Attribute::with_value(
        attribute_id,
        behavior,
        AttributeValue::PolyToEuVu {
            edgeuse: None,
            vertexuse: None,
        },
    )
}

/// `SmPolyEdgeToVertexUseAttr(lAttributeID, eBehavior)` (upstream default
/// [`Behavior::Copy`]): no vertex use. Its `MakeCopy` is [`Attribute::make_copy`].
#[must_use]
pub fn poly_edge_to_vertex_use_attribute(attribute_id: u64, behavior: Behavior) -> Attribute {
    Attribute::with_value(
        attribute_id,
        behavior,
        AttributeValue::PolyEdgeToVertexUse { vertexuse: None },
    )
}

// ---- tessellation drivers -----------------------------------------------------------

/// The arguments of `SmSurfaceTessDriver`'s constructor (a zero tolerance is
/// ignored).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceTessDriverOptions {
    /// `dChordHeight`: largest element control point to base plane distance.
    pub chord_height: f64,
    /// `dAngTolDeg`: largest element control polygon turning angle.
    pub ang_tol_deg: f64,
    /// `dMaxEdgeLength3D`: largest element side length.
    pub max_edge_length_3d: f64,
    /// `dMinEdgeLength3D`: smallest element side length.
    pub min_edge_length_3d: f64,
    /// `dMinEdgeLengthRatioUV`: smallest element side / domain size ratio.
    pub min_edge_length_ratio_uv: f64,
    /// `dMaxAspectRatio`: largest element aspect ratio.
    pub max_aspect_ratio: f64,
}

impl Default for SurfaceTessDriverOptions {
    /// Upstream's default arguments: `(0.0, 25.0, 0.0, 0.0, 0.001, 0.0)`.
    fn default() -> Self {
        Self {
            chord_height: 0.0,
            ang_tol_deg: 25.0,
            max_edge_length_3d: 0.0,
            min_edge_length_3d: 0.0,
            min_edge_length_ratio_uv: 0.001,
            max_aspect_ratio: 0.0,
        }
    }
}

/// The arguments of `SmViewBasedTessDriver`'s constructor after the view vector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewBasedTessDriverOptions {
    /// `dSilhouetteChordHeight`.
    pub silhouette_chord_height: f64,
    /// `dSilhouetteAngleToleranceDeg`.
    pub silhouette_angle_tolerance_deg: f64,
    /// The `SmSurfaceTessDriver` arguments.
    pub base: SurfaceTessDriverOptions,
}

impl Default for ViewBasedTessDriverOptions {
    /// Upstream's default arguments: `(0.0, 5.0, 0.0, 20.0, 0.0, 0.0, 0.001, 0.0)`.
    fn default() -> Self {
        Self {
            silhouette_chord_height: 0.0,
            silhouette_angle_tolerance_deg: 5.0,
            base: SurfaceTessDriverOptions {
                ang_tol_deg: 20.0,
                ..SurfaceTessDriverOptions::default()
            },
        }
    }
}

/// The view data of an `SmViewBasedTessDriver`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewBased {
    /// `m_vViewVector`.
    pub view_vector: Vec3,
    /// `m_dSilhouetteChordHeight`.
    pub silhouette_chord_height: f64,
    /// `m_dSilhouetteAngleToleranceDeg`.
    pub silhouette_angle_tolerance_deg: f64,
}

/// How surfaces are subdivided for tessellation (`SmSurfaceTessDriver`, or an
/// `SmViewBasedTessDriver` when [`SurfaceTessDriver::view`] is set).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceTessDriver {
    /// `m_bEvalBasedTessellation` (`FALSE` from both constructors).
    pub eval_based_tessellation: bool,
    /// The tolerances (`m_dChordHeight`, `m_dAngTolDeg`, ...).
    pub options: SurfaceTessDriverOptions,
    /// The view data of an `SmViewBasedTessDriver`.
    pub view: Option<ViewBased>,
}

impl Default for SurfaceTessDriver {
    fn default() -> Self {
        Self::new(SurfaceTessDriverOptions::default())
    }
}

impl SurfaceTessDriver {
    /// `SmSurfaceTessDriver(dChordHeight, dAngTolDeg, dMaxEdgeLength3D,
    /// dMinEdgeLength3D, dMinEdgeLengthRatioUV, dMaxAspectRatio)`.
    #[must_use]
    pub const fn new(options: SurfaceTessDriverOptions) -> Self {
        Self {
            eval_based_tessellation: false,
            options,
            view: None,
        }
    }

    /// `SmViewBasedTessDriver(crViewVector, dSilhouetteChordHeight,
    /// dSilhouetteAngleToleranceDeg, ...)`.
    #[must_use]
    pub const fn view_based(view_vector: Vec3, options: ViewBasedTessDriverOptions) -> Self {
        Self {
            eval_based_tessellation: false,
            options: options.base,
            view: Some(ViewBased {
                view_vector,
                silhouette_chord_height: options.silhouette_chord_height,
                silhouette_angle_tolerance_deg: options.silhouette_angle_tolerance_deg,
            }),
        }
    }

    /// `GetType`: [`SURFACE_TESS_DRIVER_TYPE`] or [`VIEW_BASED_TESS_DRIVER_TYPE`].
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        if self.view.is_some() {
            VIEW_BASED_TESS_DRIVER_TYPE
        } else {
            SURFACE_TESS_DRIVER_TYPE
        }
    }

    /// `SmViewBasedTessDriver::GetViewVector`; `None` for a plain driver.
    #[must_use]
    pub fn view_vector(&self) -> Option<Vec3> {
        self.view.map(|v| v.view_vector)
    }

    /// `SmViewBasedTessDriver::GetSilhouetteChordHeight`; `None` for a plain driver.
    #[must_use]
    pub fn silhouette_chord_height(&self) -> Option<f64> {
        self.view.map(|v| v.silhouette_chord_height)
    }

    /// `SmViewBasedTessDriver::GetSilhouetteAngleTolerance` (degrees); `None` for a
    /// plain driver.
    #[must_use]
    pub fn silhouette_angle_tolerance(&self) -> Option<f64> {
        self.view.map(|v| v.silhouette_angle_tolerance_deg)
    }
}

// ---- the cache ----------------------------------------------------------------------

/// What the nodes of a [`TessSrfCache`] point at (see the module deviations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TessHandle {
    /// The cache's surface (`GetSurface()`).
    CacheSurface,
    /// The `n`-th subdivision surface the cache made.
    Subdivision(u64),
    /// A topology object (the original face of a poly face).
    Object(ObjectKey),
    /// A poly edge of the cache's poly B-rep.
    PolyEdge(PolyEdgeId),
}

fn subdivision_handle(n: u64) -> TessHandle {
    TessHandle::Subdivision(n)
}

/// The tessellation surface cache (`SmTessSrfCache`).
#[derive(Debug, Clone)]
pub struct TessSrfCache {
    /// The `SmTrimSrfCache` base.
    trim: TrimSrfCache<TessHandle>,
    /// `m_pFace`.
    face: ObjectKey,
    /// `mTS_pPolyBrep`.
    poly_brep: Option<PolyBrep>,
    /// `m_pSurfaceTessDriver`.
    driver: Option<SurfaceTessDriver>,
}

impl TessSrfCache {
    /// A tessellation cache of `face`, whose surface is `surface`, not built
    /// (`SmTessSrfCache::SmTessSrfCache`): `poly_brep` is the face's boundary
    /// tessellation (phase 1), `driver` the surface tessellation driver, `opts` the
    /// tolerances (`dChordHeightTolerance`, `dAngleTolerance` — stored unconverted as
    /// the base's `dAngTolRad` —, `dMaxEdgeLength3D`, `dMinEdgeLength3D`,
    /// `dMinEdgeLengthRatioUV`, `dMaxAspectRatio`).
    #[must_use]
    pub fn new(
        face: ObjectKey,
        surface: Surface,
        poly_brep: Option<PolyBrep>,
        driver: Option<SurfaceTessDriver>,
        opts: SurfaceCacheOptions,
    ) -> Self {
        let mut trim = TrimSrfCache::new(
            surface,
            SurfaceCacheOptions {
                chord_height_tolerance: opts.chord_height_tolerance,
                ang_tol_rad: opts.ang_tol_rad,
                ..SurfaceCacheOptions::default()
            },
        );
        let base = trim.base_mut();
        base.set_surface_handle(Some(TessHandle::CacheSurface));
        base.set_subdivision_handles(Some(subdivision_handle));
        let o = base.options_mut();
        o.aspect_ratio_3d = opts.aspect_ratio_3d;
        o.max_side_length_3d = opts.max_side_length_3d; // gives lots of cache on large planes
        o.min_side_length_3d = opts.min_side_length_3d;
        o.min_side_length_ratio_uv = opts.min_side_length_ratio_uv;
        Self {
            trim,
            face,
            poly_brep,
            driver,
        }
    }

    /// The `SmTrimSrfCache` base.
    #[must_use]
    pub const fn trim(&self) -> &TrimSrfCache<TessHandle> {
        &self.trim
    }

    /// The `SmTrimSrfCache` base, mutable.
    pub fn trim_mut(&mut self) -> &mut TrimSrfCache<TessHandle> {
        &mut self.trim
    }

    /// The `SmSurfaceCache` base.
    #[must_use]
    pub const fn base(&self) -> &SurfaceCache<TessHandle> {
        self.trim.base()
    }

    /// The `SmSurfaceCache` base, mutable.
    pub fn base_mut(&mut self) -> &mut SurfaceCache<TessHandle> {
        self.trim.base_mut()
    }

    /// The face being tessellated (`GetFace`).
    #[must_use]
    pub const fn face(&self) -> ObjectKey {
        self.face
    }

    /// The face's poly B-rep (`GetTS_PolyBrep`).
    #[must_use]
    pub const fn ts_poly_brep(&self) -> Option<&PolyBrep> {
        self.poly_brep.as_ref()
    }

    /// The face's poly B-rep, mutable.
    pub fn ts_poly_brep_mut(&mut self) -> Option<&mut PolyBrep> {
        self.poly_brep.as_mut()
    }

    /// The surface tessellation driver (`m_pSurfaceTessDriver`).
    #[must_use]
    pub const fn driver(&self) -> Option<&SurfaceTessDriver> {
        self.driver.as_ref()
    }

    /// `SmTessSrfCache_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        TYPE
    }

    /// `"SmTessSrfCache_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        TYPE_STRING
    }

    /// [`is_kind_of`] (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        is_kind_of(t)
    }

    // ---- tolerance test -------------------------------------------------------------

    /// Tests `node` against the tessellation tolerances (`TestAgainstTolerances`): the
    /// surface cache tests, then, for a view based driver, a node passing them whose
    /// centre and corner normals change side of the view direction's orthogonal
    /// plane (a silhouette node) is tested again with the driver's silhouette chord
    /// height and angle (the angle in degrees goes into the radian tolerance, as
    /// upstream).
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceCache::test_against_tolerances`] and `EvaluateNormal`;
    /// `SM_ERR_NULL_POINTER` without driver once the node passes.
    pub fn test_against_tolerances(&mut self, k: &Kernel, node: NodeId) -> Result<ToleranceTest> {
        let driver = self.driver;
        tess_test_against_tolerances(self.trim.base_mut(), k, node, driver.as_ref())
    }

    // ---- tree construction ----------------------------------------------------------

    /// Starts a new tree with one node over the natural domain of `bss`, holding a
    /// copy of `bss` as its subdivision surface, and subdivides it until every leaf
    /// passes [`TessSrfCache::test_against_tolerances`]
    /// (`BuildTreeWithSubdivision`); point testing is off meanwhile. Every node has
    /// `SM_AD_AUX_DATA`.
    ///
    /// # Errors
    ///
    /// Those of the subdivision ([`SurfaceCache::subdivide_to_tolerances_with`]).
    pub fn build_tree_with_subdivision(&mut self, k: &Kernel, bss: &BSplineSurface) -> Result<()> {
        // allocate the decomposition tree stored within the cache
        let base = self.trim.base_mut();
        base.set_tree(Some(Tree::new(Some(TreeOwner::TessSrfCache))));

        // initialize parent subdivision tree m_pData objects
        base.auxs_mut().clear();

        // to prevent bad recursion back into BuildTree - turn off point testing
        let saved = base.point_test_enabled;
        base.point_test_enabled = false;
        let r = self.build_tree_with_subdivision_body(k, bss);
        self.trim.base_mut().point_test_enabled = saved;
        r
    }

    fn build_tree_with_subdivision_body(&mut self, k: &Kernel, bss: &BSplineSurface) -> Result<()> {
        let driver = self.driver;
        let base = self.trim.base_mut();

        // allocate 1 top node and its m_pData object
        let slot = base.auxs().len();
        let top = base
            .tree_mut()
            .ok_or_else(|| invalid(OP_SUB, "cache has no tree"))?
            .add_node(TreeNode {
                parent: None,
                child1: None,
                child2: None,
                bbox: Extent3d::from_bounds(0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
                aux_type: AuxDataType::AuxData,
                data: NodeData::Surface(SurfaceAux {
                    split_dir: SplitDir::U,
                    is_marked: false,
                    node_class: NodeClass::Unknown,
                    face: None,
                    surface: None,
                    slot: Some(slot),
                }),
                geom_type: NodeGeomType::Default,
            });
        let domain = bss.natural_uv_domain();
        base.auxs_mut().push(BezierAux2d {
            owning_tree_node: Some(top),
            is_tess_only: false,
            uv_domain: domain,
            ..BezierAux2d::default()
        });
        base.tree_mut()
            .ok_or_else(|| invalid(OP_SUB, "cache has no tree"))?
            .set_top_node(Some(top));

        // prepare for subdivision - point the node to a copy of the surface
        let copy = bss.clone();
        let handle = base
            .new_subdivision_handle()
            .ok_or_else(|| invalid(OP_SUB, "cache cannot name subdivision surfaces"))?;
        let bbox = ignore(copy.calculate_bounding_box(
            k,
            &domain,
            &BoundingBoxRequest {
                normal: true,
                expand_by_zone_tol: true,
                ..BoundingBoxRequest::default()
            },
        ))?
        .and_then(|b| b.normal);
        base.subdivision_surfaces_mut()
            .push((handle, Surface::from(copy)));
        {
            let tree = base
                .tree_mut()
                .ok_or_else(|| invalid(OP_SUB, "cache has no tree"))?;
            let n = tree.node_mut(top)?;
            if let NodeData::Surface(a) = &mut n.data {
                a.surface = Some(handle);
            }
            if let Some(b) = bbox {
                n.bbox = b;
            }
        }
        {
            let aux = base.aux_mut(top)?;
            aux.chord_height_satisfied = false;
            aux.angle_tol_satisfied = false;
        }

        // subdivide the whole surface node into pieces that all meet the requirements
        base.subdivide_to_tolerances_with(k, top, 0, &mut |c, k, n| {
            tess_test_against_tolerances(c, k, n, driver.as_ref())
        })
    }

    /// Builds the tree, embeds the face's poly edges and labels the nodes
    /// (`BuildTree`): a missing tree is built by
    /// [`TessSrfCache::build_tree_with_subdivision`] for B-spline surfaces, else by
    /// the surface cache's `BuildTree` (status ignored); then, when the cache has its
    /// poly B-rep, every poly edge of its first poly face is implanted
    /// ([`TessSrfCache::implant_uv_segment`], point testing off), the subdivision
    /// surfaces are dropped (every node refers to the cache surface), the trimmed
    /// state is set and the nodes are classified
    /// ([`TessSrfCache::mark_face_containment`]).
    ///
    /// # Errors
    ///
    /// Those of the subdivision, the implanting and the classification;
    /// `SM_ERR_NULL_POINTER` for a poly B-rep without poly face.
    pub fn build_tree(&mut self, k: &Kernel) -> Result<()> {
        // prevent bad recursion back into BuildTree
        let saved = self.base().face_containment_done;
        self.base_mut().face_containment_done =
            crate::surface::surface_cache::p1::FaceContainment::UnderConstruction;
        let r = self.build_tree_body(k);
        if !matches!(r, Ok(true)) {
            self.base_mut().face_containment_done = saved;
        }
        r.map(|_| ())
    }

    /// `BuildTree` inside the face containment guard; `true` when the guard was
    /// cleared (containment done).
    fn build_tree_body(&mut self, k: &Kernel) -> Result<bool> {
        // Build the m_pTree subdivision tree of the surface
        if self.base().tree().is_none() {
            // (`GetNaturalUVDomain` and the copy constructor read the NURB through
            // `GetOrCreateGwNurbPointer`, which makes a plane's or a surface of
            // revolution's missing NURB first)
            let bss: Option<BSplineSurface> = match self.base().surface().kind() {
                SurfaceKind::BSpline(b) => Some(b.clone()),
                SurfaceKind::Plane(p) => Some(match ignore(p.base_with_nurb())? {
                    Some(b) => b.into_owned(),
                    None => p.base().clone(),
                }),
                // (kept on the surface: `Surface::kind_with_nurb`, status ignored)
                SurfaceKind::Cone(_)
                | SurfaceKind::Sphere(_)
                | SurfaceKind::Torus(_)
                | SurfaceKind::SurfOfRevolution(_) => {
                    self.base().surface().revolution_base_with_nurb(k)?.cloned()
                }
                SurfaceKind::SurfOfExtrusion(s) => Some(if s.base().gw_nurb().is_some() {
                    s.base().clone()
                } else {
                    let mut c = s.as_ref().clone();
                    match ignore(c.make_nurb(k))? {
                        Some(()) => c.base().clone(),
                        None => s.base().clone(),
                    }
                }),
                SurfaceKind::CurveBounded(_)
                | SurfaceKind::Step(_)
                | SurfaceKind::SrfInVolume(_)
                | SurfaceKind::OffsetSurface(_) => None,
            };
            if let Some(bss) = bss {
                self.build_tree_with_subdivision(k, &bss)?;
            } else {
                // surface is not a B-spline surface: starter Bezier approximations
                let driver = self.driver;
                ignore(
                    self.trim
                        .base_mut()
                        .build_tree_with(k, false, &mut |c, k, n| {
                            tess_test_against_tolerances(c, k, n, driver.as_ref())
                        }),
                )?;
            }
        }

        // quit if this surface's owner face has not gone through phase 1
        let Some(pb) = self.poly_brep.as_ref() else {
            return Ok(false);
        };

        // the PolyEdges of the first poly face (phase 1 poly breps have one face)
        let poly_face = pb
            .first_poly_face()?
            .ok_or(SolidError::null_pointer(OP_BUILD))?;
        let poly_edges = pb.face_poly_edges(poly_face)?;

        // implant every PolyEdge into the subdivision tree - point testing off
        {
            let saved = self.base().point_test_enabled;
            self.base_mut().point_test_enabled = false;
            let mut r = Ok(());
            for &e in &poly_edges {
                r = self.implant_uv_segment(k, e);
                if r.is_err() {
                    break;
                }
            }
            self.base_mut().point_test_enabled = saved;
            r?;
        }

        // clean up the subdivision surfaces: every node refers to the source surface
        {
            let base = self.base_mut();
            base.subdivision_surfaces_mut().clear();
            if let Some(tree) = base.tree_mut() {
                for id in 0..tree.nodes().len() {
                    if let NodeData::Surface(a) = &mut tree.node_mut(NodeId(id))?.data {
                        if a.slot.is_some() {
                            a.surface = Some(TessHandle::CacheSurface);
                        }
                    }
                }
            }
        }

        // set cache state bits
        {
            let base = self.base_mut();
            base.process_boundary_curves = false;
            base.point_test_enabled = true;
            base.have_t_surface_cache = true;
        }

        // label every node inside / outside / on the boundary
        self.mark_face_containment(k)?;
        self.base_mut().face_containment_done =
            crate::surface::surface_cache::p1::FaceContainment::Done;
        Ok(true)
    }

    /// `GetTree` with the virtual `BuildTree` of this cache: builds when there is no
    /// tree, or when the surface belongs to a face whose boundary is not in the tree
    /// yet and no build is under way.
    fn get_tree(&mut self, k: &Kernel) -> Result<()> {
        let face_owned = self
            .base()
            .surface()
            .owner()
            .is_some_and(|o| o.type_code == FACE_TYPE);
        let b = self.base();
        let building = b.face_containment_done
            == crate::surface::surface_cache::p1::FaceContainment::UnderConstruction;
        // (port) a missing tree mid-build means the build in progress already failed
        // (softly); building again would repeat it and recurse without end
        if b.tree().is_none() && building {
            return Err(invalid(OP_GET_TREE, "tree build re-entered without a tree"));
        }
        if b.tree().is_none()
            || (!b.have_t_surface_cache
                && face_owned
                && b.face_containment_done
                    != crate::surface::surface_cache::p1::FaceContainment::UnderConstruction)
        {
            self.build_tree(k)?;
        }
        if self.base().tree().is_none() {
            return Err(invalid(OP_GET_TREE, "cache has no tree"));
        }
        Ok(())
    }

    // ---- boundary -------------------------------------------------------------------

    /// Adds the poly edge `segment` (of the cache's poly B-rep) to every leaf whose
    /// domain, grown by ten times the segment's parametric tolerances, holds an end
    /// point of the (domain clamped) segment or meets its line (unless the segment is
    /// degenerate); those leaves go on the boundary (`ImplantUVSegment`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without poly B-rep; `SM_ERR` when no node meets the
    /// segment's box; those of the poly edge queries, `SmTol::MapTo2d` and the tree.
    pub fn implant_uv_segment(&mut self, k: &Kernel, segment: PolyEdgeId) -> Result<()> {
        // get segment endpoints and tolerance
        let pb = self
            .poly_brep
            .as_ref()
            .ok_or(SolidError::null_pointer(OP_UV))?;
        let s3 = pb.edge_start_point(segment)?;
        let e3 = pb.edge_end_point(segment)?;
        let tol_3d = pb.edge_tolerance(segment)?;
        let mut start = Point2::new(s3.x, s3.y);
        let mut end = Point2::new(e3.x, e3.y);

        // Tolerance: uv space tols limited to a small fraction of the surface domain
        let (tol_u, tol_v, uv_domain) = {
            let base = self.base();
            let surface = base.surface();
            let uv_domain = base.uv_domain();
            let mid_uv = (start + end) / 2.0;
            let tol_u = min2(
                map_to_2d(k, tol_3d, mid_uv, Vec2::new(1.0, 0.0), surface)?,
                uv_domain.x_length() / 100.0,
            )
            .abs();
            let tol_v = min2(
                map_to_2d(k, tol_3d, mid_uv, Vec2::new(0.0, 1.0), surface)?,
                uv_domain.y_length() / 100.0,
            )
            .abs();
            (tol_u, tol_v, uv_domain)
        };
        let tol_expansion = Vec2::new(tol_u, tol_v);

        // clamp to the domain (UV curves can wander slightly outside it)
        start = uv_domain.clamp_point(start);
        end = uv_domain.clamp_point(end);

        let line_vec = end - start;
        let is_degenerate = line_vec.x.abs() <= tol_u && line_vec.y.abs() <= tol_v;

        // build trimmed segment bounding box - increased by tolerance
        let mut seg_extent = Extent2d::from_point(start);
        seg_extent.add_point(end);
        seg_extent.expand_absolute_vec(tol_expansion * 10.0);

        // get tree nodes that intersect the segment box (leaves and parents)
        self.get_tree(k)?;
        let nodes = self.base().find_uv_nodes_built(k, &seg_extent, false)?;

        // check state - segment should intersect some subdivision tree nodes
        if nodes.is_empty() {
            return Err(SolidError::sm_err(OP_UV));
        }

        // for every subdivision leaf within the segment box
        for node in nodes {
            let base = self.trim.base_mut();
            let tree = base
                .tree()
                .ok_or_else(|| invalid(OP_UV, "cache has no tree"))?;
            if tree.node(node)?.child1.is_some() {
                continue;
            }

            let domain = base.aux(node)?.uv_domain;
            let mut domain_exp = domain;
            domain_exp.expand_absolute_vec(tol_expansion * 10.0);

            // skip nodes that don't meet the segment
            if !domain_exp.contains_point(start, 0.0) && !domain_exp.contains_point(end, 0.0) {
                if is_degenerate {
                    continue;
                }
                if domain_exp
                    .intersect_with_infinite_line(start, line_vec, None, None)
                    .is_none()
                {
                    continue;
                }
            }

            // Add segment to this node's m_sPolyEdgeList
            set_class(base, node, NodeClass::OnBoundary, OP_UV)?;
            base.aux_mut(node)?
                .poly_edge_list
                .insert(0, TessHandle::PolyEdge(segment));
        }

        Ok(())
    }

    /// Labels the nodes (`MarkFaceContainment`): boundary leaves are marked and their
    /// ancestors put on the boundary; then each unmarked unknown leaf takes the class
    /// of an inside or outside leaf meeting its domain, else the point in polygon
    /// result of its domain centre against the first poly face, and passes it (with
    /// the original face) to every unmarked leaf meeting its domain and up to their
    /// ancestors whose two children agree.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without poly B-rep or poly face when a point test is
    /// needed; those of the tree, `PointInPolygon` and `SetOriginalFace`.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn mark_face_containment(&mut self, k: &Kernel) -> Result<()> {
        self.get_tree(k)?;
        let top = self
            .base()
            .tree()
            .and_then(Tree::top_node)
            .ok_or_else(|| invalid(OP_MARK, "tree has no top node"))?;
        let limit = self.base().tree().map_or(0, |t| t.nodes().len());

        // Propagate up ON_BOUNDARY flag and a mark when all subnodes are classified
        let mut stack = vec![top];
        let mut visits = 0;
        while let Some(node) = stack.pop() {
            // (not upstream) a tree whose children point back to their parents visits
            // each node once: more visits mean a shared or cyclic child link (upstream
            // follows a cycle forever)
            visits += 1;
            if visits > limit {
                return Err(invalid(OP_MARK, "cyclic child links"));
            }
            let base = self.trim.base_mut();
            let (c1, c2, parent) = links(base, node)?;
            let own = fields(base, node, OP_MARK)?;
            if let Some(c1) = c1 {
                // Non-leaf node
                let c2 = c2.ok_or_else(|| invalid(OP_MARK, "node has one child"))?;
                stack.push(c1);
                stack.push(c2);
            } else if own.node_class == NodeClass::OnBoundary {
                fields_mut(base, node, OP_MARK)?.is_marked = true;
                // Propagate mark up tree if both children are marked, and the boundary
                let mut p = parent;
                let mut steps = 0;
                while let Some(pn) = p {
                    steps += 1;
                    if steps > limit {
                        return Err(invalid(OP_MARK, "cyclic parent links"));
                    }
                    let (pc1, pc2, pp) = links(base, pn)?;
                    let a1 = fields(base, pc1.ok_or_else(no_child)?, OP_MARK)?;
                    let a2 = fields(base, pc2.ok_or_else(no_child)?, OP_MARK)?;
                    let pf = fields_mut(base, pn, OP_MARK)?;
                    if a1.is_marked && a2.is_marked {
                        pf.is_marked = true;
                    }
                    pf.node_class = NodeClass::OnBoundary;
                    p = pp;
                }
            }
        }

        stack.clear();
        stack.push(top);
        let mut curr = self
            .base()
            .find_unmarked_leaf_node(NodeClass::Unknown, &mut stack)?;

        while let Some(cn) = curr {
            let domain = self.base().aux(cn)?.uv_domain;
            let mut found_class = NodeClass::Unknown;
            let mut found_face: Option<TessHandle> = None;

            // gather all nodes intersecting this node's uv domain
            self.get_tree(k)?;
            let adjacent = self.base().find_uv_nodes_built(k, &domain, false)?;

            // a classified neighbour leaf gives the classification
            for &adj in &adjacent {
                let base = self.base();
                let (c1, _, _) = links(base, adj)?;
                if c1.is_some() {
                    continue;
                }
                let f = fields(base, adj, OP_MARK)?;
                if f.node_class == NodeClass::Inside || f.node_class == NodeClass::Outside {
                    found_class = f.node_class;
                    if f.node_class == NodeClass::Inside {
                        found_face = f.face;
                    }
                    break;
                }
            }

            // when no neighbour was classified - classify the node's centre
            if found_class == NodeClass::Unknown {
                let pb = self
                    .poly_brep
                    .as_mut()
                    .ok_or(SolidError::null_pointer(OP_MARK))?;
                let pf = pb
                    .first_poly_face()?
                    .ok_or(SolidError::null_pointer(OP_MARK))?;
                let original = pb.face_original_face(pf)?;
                found_face = original.map(TessHandle::Object);
                let mid = domain.evaluate(0.5, 0.5);
                let inside =
                    pb.face_point_in_polygon(k, pf, Point3::new(mid.x, mid.y, 0.0), false)?;
                if inside {
                    found_class = NodeClass::Inside;
                    // The source key was read from this same face above: native
                    // SetOriginalFace takes its unchanged branch, with no notify.
                    pb.face_set_original_face(pf, original, true, None)?;
                } else {
                    found_class = NodeClass::Outside;
                }
            }

            // mark every unmarked neighbour leaf with the classification
            for &adj in &adjacent {
                let base = self.trim.base_mut();
                let (c1, _, parent) = links(base, adj)?;
                if c1.is_some() {
                    continue;
                }
                if fields(base, adj, OP_MARK)?.is_marked {
                    continue;
                }
                {
                    let f = fields_mut(base, adj, OP_MARK)?;
                    f.is_marked = true;
                    f.node_class = found_class;
                    f.face = found_face;
                }

                // Propagate information up tree
                let mut p = parent;
                let mut steps = 0;
                while let Some(pn) = p {
                    steps += 1;
                    if steps > limit {
                        return Err(invalid(OP_MARK, "cyclic parent links"));
                    }
                    let (pc1, pc2, pp) = links(base, pn)?;
                    let a1 = fields(base, pc1.ok_or_else(no_child)?, OP_MARK)?;
                    let a2 = fields(base, pc2.ok_or_else(no_child)?, OP_MARK)?;
                    let pf = fields_mut(base, pn, OP_MARK)?;
                    // when both children are marked - mark the parent
                    if a1.is_marked && a2.is_marked {
                        pf.is_marked = true;
                    }
                    // when both children have the current classification
                    if a1.node_class == found_class && a2.node_class == found_class {
                        pf.node_class = found_class;
                        pf.face = found_face;
                    }
                    p = pp;
                }
            }

            // find target node for next iteration
            curr = self
                .base()
                .find_unmarked_leaf_node(NodeClass::Unknown, &mut stack)?;
        }

        Ok(())
    }
}

// ---- helpers ------------------------------------------------------------------------

/// `SmTessSrfCache::TestAgainstTolerances` on the base cache `cache` with the
/// tessellation driver `driver` (the virtual override handed to
/// `SubdivideToTolerances`).
fn tess_test_against_tolerances(
    cache: &mut SurfaceCache<TessHandle>,
    k: &Kernel,
    node: NodeId,
    driver: Option<&SurfaceTessDriver>,
) -> Result<ToleranceTest> {
    // Make all the standard surface tolerance tests
    let out = cache.test_against_tolerances(k, node)?;

    // no more work - element not yet good enough, or no view specific tessellation
    if out.needs_subdivision {
        return Ok(out);
    }
    let driver = driver.ok_or(SolidError::null_pointer(OP_TEST))?;
    let Some(view) = driver.view else {
        return Ok(out);
    };

    // samples - center and four corners
    let uv_domain = cache.aux(node)?.uv_domain;
    let uvs = [
        uv_domain.evaluate(0.5, 0.5),
        uv_domain.evaluate(0.0, 0.0),
        uv_domain.evaluate(0.0, 1.0),
        uv_domain.evaluate(1.0, 0.0),
        uv_domain.evaluate(1.0, 1.0),
    ];

    // see if the sample normals span the orthogonal to the viewing vector
    let mut silhouette = false;
    let mut direction = 0.0;
    for uv in uvs {
        let norm = cache
            .surface()
            .evaluate_normal(k, uv, Side::Left, Side::Left)?;
        let dot = norm.dot(view.view_vector);
        if dot * direction < 0.0 {
            silhouette = true;
            break;
        }
        direction = dot;
    }

    // no work - node is not part of the silhouette for this viewing angle
    if !silhouette {
        return Ok(out);
    }

    // switch to the tighter silhouette tessellation tolerances
    let saved = *cache.options();
    {
        let o = cache.options_mut();
        o.chord_height_tolerance = view.silhouette_chord_height;
        o.ang_tol_rad = view.silhouette_angle_tolerance_deg;
    }
    let r = (|| {
        let aux = cache.aux_mut(node)?;
        aux.chord_height_satisfied = false;
        aux.angle_tol_satisfied = false;

        // subdivide this node to the tighter silhouette tolerances
        cache.test_against_tolerances(k, node)
    })();
    {
        let o = cache.options_mut();
        o.chord_height_tolerance = saved.chord_height_tolerance;
        o.ang_tol_rad = saved.ang_tol_rad;
    }
    r
}

/// `SmTol::MapTo2d(tol, uv, dir, surface)`.
fn map_to_2d(k: &Kernel, tol_3d: f64, uv: Point2, dir: Vec2, surface: &Surface) -> Result<f64> {
    crate::surface::surface::p1::map_to_2d_dir(k, tol_3d, uv, dir, surface)
}

fn no_child() -> SolidError {
    invalid(OP_MARK, "parent node without its two children")
}

/// `(child1, child2, parent)` of `node`.
fn links(
    base: &SurfaceCache<TessHandle>,
    node: NodeId,
) -> Result<(Option<NodeId>, Option<NodeId>, Option<NodeId>)> {
    let n = base
        .tree()
        .ok_or_else(|| invalid(OP_MARK, "cache has no tree"))?
        .node(node)?;
    Ok((n.child1, n.child2, n.parent))
}

/// The tree-held `SmBezierAux2d` fields of `node` (it must have them).
fn fields(
    base: &SurfaceCache<TessHandle>,
    node: NodeId,
    op: &'static str,
) -> Result<SurfaceAux<TessHandle>> {
    match base
        .tree()
        .ok_or_else(|| invalid(op, "cache has no tree"))?
        .node(node)?
        .data
    {
        NodeData::Surface(a) => Ok(a),
        _ => Err(invalid(op, "node has no surface data")),
    }
}

fn fields_mut<'a>(
    base: &'a mut SurfaceCache<TessHandle>,
    node: NodeId,
    op: &'static str,
) -> Result<&'a mut SurfaceAux<TessHandle>> {
    match &mut base
        .tree_mut()
        .ok_or_else(|| invalid(op, "cache has no tree"))?
        .node_mut(node)?
        .data
    {
        NodeData::Surface(a) => Ok(a),
        _ => Err(invalid(op, "node has no surface data")),
    }
}

fn set_class(
    base: &mut SurfaceCache<TessHandle>,
    node: NodeId,
    class: NodeClass,
    op: &'static str,
) -> Result<()> {
    // (upstream writes through the node's SmBezierAux2d, which every node here has)
    base.aux(node)?;
    fields_mut(base, node, op)?.node_class = class;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::nurbs::srf_basic::Surface as NlSurface;

    fn plane_bss() -> BSplineSurface {
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
        BSplineSurface::from_nurb(s, false)
    }

    fn cache(driver: Option<SurfaceTessDriver>) -> TessSrfCache {
        TessSrfCache::new(
            ObjectKey(1),
            Surface::from(plane_bss()),
            None,
            driver,
            SurfaceCacheOptions {
                chord_height_tolerance: 0.01,
                ang_tol_rad: 0.3,
                ..SurfaceCacheOptions::default()
            },
        )
    }

    #[test]
    fn constructor_and_types() {
        let c = cache(Some(SurfaceTessDriver::default()));
        assert_eq!(c.type_code(), TYPE);
        assert_eq!(c.type_string(), "SmTessSrfCache_TYPE");
        assert!(c.is_kind_of(TYPE) && c.is_kind_of(trim::TYPE));
        assert!(!c.is_kind_of(SURFACE_TESS_DRIVER_TYPE));
        assert_eq!(c.face(), ObjectKey(1));
        assert!(c.ts_poly_brep().is_none());
        assert_eq!(c.base().options().min_side_length_ratio_uv, 0.001);
        assert!(c.base().point_test_enabled);
    }

    #[test]
    fn drivers() {
        let d = SurfaceTessDriver::default();
        assert_eq!(d.type_code(), SURFACE_TESS_DRIVER_TYPE);
        assert_eq!(d.options.ang_tol_deg, 25.0);
        assert_eq!(d.view_vector(), None);
        let v = SurfaceTessDriver::view_based(
            Vec3::new(0.0, 0.0, 1.0),
            ViewBasedTessDriverOptions::default(),
        );
        assert_eq!(v.type_code(), VIEW_BASED_TESS_DRIVER_TYPE);
        assert_eq!(v.options.ang_tol_deg, 20.0);
        assert_eq!(v.silhouette_angle_tolerance(), Some(5.0));
        assert_eq!(v.silhouette_chord_height(), Some(0.0));
    }

    #[test]
    fn attributes_copy() {
        let a = poly_to_euvu_attribute(9, Behavior::Copy);
        let c = a.make_copy();
        assert_eq!(c.attribute_id(), 9);
        assert_eq!(c.value(), a.value());
        assert_eq!(c.type_code(), a.type_code());
        let b = poly_edge_to_vertex_use_attribute(4, Behavior::Reference);
        assert_eq!(b.make_copy().behavior(), Behavior::Reference);
    }

    #[test]
    fn flat_plane_builds_one_node() {
        let k = Kernel::default();
        let mut c = cache(Some(SurfaceTessDriver::default()));
        c.build_tree(&k).unwrap();
        let tree = c.base().tree().unwrap();
        assert!(!tree.nodes().is_empty());
        // without poly B-rep the tree is built and the containment state restored
        assert_eq!(
            c.base().face_containment_done,
            crate::surface::surface_cache::p1::FaceContainment::NotDone
        );
    }

    #[test]
    fn implant_without_poly_brep_is_null_pointer() {
        let k = Kernel::default();
        let mut c = cache(Some(SurfaceTessDriver::default()));
        assert!(c.implant_uv_segment(&k, PolyEdgeId::from_index(0)).is_err());
    }
}
