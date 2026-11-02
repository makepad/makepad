//! Uniform voxel grids for fast ray casting: elements are registered in every voxel
//! their bounding box touches, and a 3D-DDA walk visits the voxels along a ray and
//! reports the elements whose boxes the ray crosses, sorted by entry parameter.
//!
//! Upstream: `source/SMLib/src/SmGrid.cpp`, `source/SMLib/inc/SmGrid.h`.
//!
//! | Rust                                   | Upstream                                         |
//! |----------------------------------------|--------------------------------------------------|
//! | [`Grid`], [`Grid::new`]                | `SmGrid`, `SmGrid::SmGrid`                       |
//! | `Drop` (implicit)                      | `SmGrid::~SmGrid`                                |
//! | [`Grid::voxel`]                        | `SmGrid::GetVoxel`                               |
//! | [`Grid::voxel_bbox`]                   | `SmGrid::GetVoxelBBox`                           |
//! | [`Grid::voxel_address`]                | `SmGrid::GetVoxelAddress`                        |
//! | [`Grid::find_voxels_in_box`]           | `SmGrid::FindVoxelsInBox`                        |
//! | [`Grid::add_element`], [`ElementSource`] | `SmGrid::AddElement`, `SmGridElementType`      |
//! | [`Grid::add_surface_node`], [`Grid::add_surface_element`] | `SmGrid::AddElement(SM_GE_SURFACE)` (from a tree node; from prepared leaf data) |
//! | [`Grid::add_hierarchy_element`]        | `SmGrid::AddHierarchyElement`                    |
//! | [`Grid::expand_hierarchy`], [`Grid::expand_hierarchy_fetched`] | `SmSurfaceHierarchyElement::ExpandHierarchy` |
//! | [`Grid::expand_hierarchy_with`]        | `SmSurfaceHierarchyElement::ExpandHierarchy` over a caller-held surface cache tree |
//! | [`Grid::setup_3ddda`]                  | `SmGrid::Setup3DDDA`                             |
//! | `AxisStep` (private)                   | one axis of `SmGrid::Setup3DDDA`                 |
//! | [`Grid::step_3ddda`], [`StepHits`]     | `SmGrid::Step3DDDA`                              |
//! | `insertion_sort` (private)             | `sm_InsertionSort`                               |
//! | [`Dda`]                                | the 3DDDA scratch members of `SmGrid`            |
//! | [`Voxel`], `Default`                   | `SmVoxel`, `SmVoxel::Init`                       |
//! | [`GridElement`]                        | `SmGridElement`                                  |
//! | [`GridElement::type_code`]             | `SmGridElement::GetType`                         |
//! | [`GridItem`]                           | the derived `SmGridElement` classes and their `GetElement` |
//! | `GridElement::polygon` (private)       | `SmPolygonGridElement::Init`                     |
//! | `GridElement::hierarchy` (private)     | `SmHierarchyGridElement::Init`                   |
//! | `GridElement::user` (private)          | `SmUserGridElement::Init`                        |
//! | [`SurfaceGridElement`]                 | `SmSurfaceGridElement`                           |
//! | [`SurfaceGridElement::init`]           | `SmSurfaceGridElement::Init`                     |
//! | [`SurfaceGridElement::from_leaf_data`] | `SmSurfaceGridElement::Init` after the Bezier patch data (domain, boxes) is known |
//! | [`HierarchyElement::new`]              | `SmHierarchyElement::SmHierarchyElement`         |
//! | [`HierarchyElement::new_surface`]      | `SmSurfaceHierarchyElement::SmSurfaceHierarchyElement` |
//! | `Drop` (implicit)                      | `~SmHierarchyElement`, `~SmSurfaceHierarchyElement` |
//!
//! Deviations:
//! - Pointers become arena indices. The grid owns its elements ([`ElementId`], in
//!   creation order; upstream's `SmMemBlockMgr` pools) and its hierarchy elements
//!   ([`HierarchyId`], in insertion order; upstream `m_pHierarchyElements`). Voxels
//!   are stored flat in `x`-major order and addressed by `[ix, iy, iz]`. Objects are a
//!   caller-chosen `Copy` handle `O` (upstream `void*` user data and `SmSurface*`);
//!   polygon elements hold the [`NodeId`] of their `SmBrepCache` tree node.
//! - `SmGrid(bbox, lSize)` rejects a zero voxel count (upstream builds a grid its
//!   destructor then reads out of bounds) and a voxel count whose total overflows;
//!   a voxel array that cannot be allocated, or would take more than
//!   [`crate::limits::MAX_ALLOC_BYTES`], is `SM_ERR`, as upstream's failed
//!   `smos_Calloc` (upstream's own grids are at most `40 x 40 x 20`).
//! - `AddElement`'s `SM_GE_HIERARCHY` type (a null-pointer crash upstream) cannot be
//!   expressed: [`ElementSource`] has no such variant.
//! - **Surface elements.** The grid holds surface handles, not surfaces, so the
//!   operations that need a surface take the surface's `smsurf_GetSurfaceCache`
//!   cache from the caller: [`Grid::expand_hierarchy`] (`ExpandHierarchy`) and
//!   [`Grid::add_surface_node`] (`AddElement(SM_GE_SURFACE)`) read the subdivision
//!   tree and the patch data of a node from it ([`SurfaceGridElement::init`]), and
//!   the cache is the one the caller keeps for the surface, as upstream's, so polar
//!   boxes are built once. `ExpandHierarchy` on a cache of another surface is
//!   [`SolidError::InvalidInput`] (upstream always fetches the cache of
//!   `m_pSurface`); `Init` on a node that is not a Bezier patch node is too
//!   (upstream casts it and reads garbage). `SM_GE_SURFACE` is not an
//!   [`ElementSource`] variant for the same reason (its node needs the cache).
//!   [`Grid::expand_hierarchy_with`] is the tree-level primitive, with `Init` as a
//!   caller closure; [`Grid::add_surface_element`] adds a leaf whose data the caller
//!   made with [`SurfaceGridElement::from_leaf_data`] (the ray tracer's
//!   `add_leaf`). `ExpandHierarchy` on a plain [`HierarchyElement::new`] is
//!   upstream's base-class "pure virtual" body: `SM_ERR`, before the cache is
//!   touched.
//! - **Statuses of the surface path.** Upstream `Init` is `void` and signals with
//!   `SE`, then carries on; `ExpandHierarchy` ignores `AddElement`'s status
//!   (SmGrid.cpp:117) and `AddSurfaceToGrid` ignores `ExpandHierarchy`'s. The port
//!   propagates a failure: an `Evaluate` that fails even without `non_zero_tangents`
//!   (see [`SurfaceGridElement::from_leaf_data`], which retries the first attempt
//!   and keeps the cross product when `EvaluateNormal` fails, as upstream's `SE`
//!   carries on), and [`SolidError::Unported`].
//! - `Step3DDDA` returns [`SolidError::InvalidInput`] after more than
//!   `1 + nx + ny + nz` voxel steps in one call (a walk that advances leaves the grid
//!   sooner); upstream loops forever when the chosen axis has step 0.
//! - `GetVoxelAddress` truncates with Rust's saturating `as` cast, so a NaN
//!   coordinate (a grid with a zero-size axis) gives index 0 instead of undefined
//!   behaviour. Its status (always `SM_SUCCESS`) is dropped.
//! - `FindVoxelsInBox` returns voxel addresses instead of `SmVoxel*`; upstream also
//!   computes a tolerance-expanded copy of the box that it never uses, which is
//!   omitted.
//! - `Step3DDDA` returns its output lists ([`StepHits`]); its unused
//!   `lCurrentVoxel` argument is dropped. When the current voxel address is out of
//!   range (no successful [`Grid::setup_3ddda`] yet, or a step after the ray left the
//!   grid; undefined behaviour upstream) it fails with
//!   [`SolidError::InvalidInput`].
//! - The 3DDDA step/exit increments of an axis the ray does not move along keep
//!   their previous values (zero for a new grid); upstream leaves them
//!   uninitialised. They are read only by a walk that stalls (see the next item).
//! - `SmHierarchyGridElement`'s sphere centre starts as [`Vec3::UNDEF`] and its box as
//!   [`Extent3d::new`] (upstream's default constructors).
//! - `GetMemoryUsed`, the drawing methods and `SmVoxel::m_pGrid` (a back pointer)
//!   have no counterpart; `SM_ASSERT`, `SM_DBG_WARN` and the `SM_DEBUG_CODE`
//!   drawing are not ported.

use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::{Extent3d, LineHit};
use crate::geom::polar_box::PolarBox;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::tree::{AuxDataType, NodeClass, NodeData, NodeId, Tree};
use crate::kernel::Kernel;
use crate::limits::try_filled;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::surface::Surface;
use crate::surface::surface_cache::SurfaceCache;

/// `SmGridElement_TYPE` (`CONT_BASE_TYPE + 155`).
pub const GRID_ELEMENT_TYPE: i64 = 12155;
/// `SmSurfaceGridElement_TYPE` (`CONT_BASE_TYPE + 159`).
pub const SURFACE_GRID_ELEMENT_TYPE: i64 = 12159;
/// `SmPolygonGridElement_TYPE` (`CONT_BASE_TYPE + 213`).
pub const POLYGON_GRID_ELEMENT_TYPE: i64 = 12213;
/// `SmHierarchyGridElement_TYPE` (`CONT_BASE_TYPE + 217`).
pub const HIERARCHY_GRID_ELEMENT_TYPE: i64 = 12217;
/// `SmUserGridElement_TYPE` (`CONT_BASE_TYPE + 221`).
pub const USER_GRID_ELEMENT_TYPE: i64 = 12221;

/// Index of a grid element in [`Grid::elements`] (upstream `SmGridElement*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementId(pub usize);

/// Index of a hierarchy element in [`Grid::hierarchy_elements`] (upstream
/// `SmHierarchyElement*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HierarchyId(pub usize);

/// Voxel indices `[ix, iy, iz]` (upstream `long lVoxelAddress[3]`).
pub type VoxelAddress = [usize; 3];

/// The cached data of one surface subdivision leaf (`SmSurfaceGridElement`): the
/// surface, the leaf's parameter domain, boxes and the mid-point data read by
/// `SmLSI3LocalSolver::LocalSolve`. [`SurfaceGridElement::init`] makes it from a
/// subdivision tree node (`SmSurfaceGridElement::Init`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGridElement<O> {
    /// `m_pSurface`: the leaf's surface.
    pub surface: O,
    /// `m_sUVDomain`: the leaf's parameter domain.
    pub uv_domain: Extent2d,
    /// `m_sBBox` (the derived class's own copy): the leaf's bounding box.
    pub bbox: Extent3d,
    /// `m_sPseudoBox`: the leaf's pseudo box.
    pub pseudo_box: PseudoBox,
    /// `m_sPolarBox`: the leaf's polar box.
    pub polar_box: PolarBox,
    /// `m_sMidPoint`: the surface point at the middle of the domain.
    pub mid_point: Vec3,
    /// `m_sMidDU`: the `u` derivative there.
    pub mid_du: Vec3,
    /// `m_sMidDV`: the `v` derivative there.
    pub mid_dv: Vec3,
    /// `m_sMidNormal`: the unit normal there.
    pub mid_normal: Vec3,
}

impl<O: Copy> SurfaceGridElement<O> {
    /// The leaf data `SmSurfaceGridElement::Init` derives from the surface once it has
    /// the Bezier patch data (domain, box, pseudo box and polar box): the surface
    /// point, derivatives and unit normal at the middle of the domain. `handle` is the
    /// surface's handle (`m_pSurface`).
    ///
    /// # Errors
    ///
    /// Only [`SolidError::Unported`], or an `Evaluate` that also fails without
    /// `non_zero_tangents`: upstream signals those with `SE` and carries on.
    pub fn from_leaf_data(
        k: &Kernel,
        handle: O,
        surface: &Surface,
        uv_domain: &Extent2d,
        bbox: &Extent3d,
        pseudo_box: &PseudoBox,
        polar_box: &PolarBox,
    ) -> Result<Self> {
        // Precomputation of derivative information for this little patch
        let uv_mid = uv_domain.evaluate(0.5, 0.5);
        let eval_flags = |non_zero_tangents: bool| EvaluateFlags {
            u_side: Side::Left,
            v_side: Side::Left,
            only_upper_half: true,
            non_zero_tangents,
        };
        // `SE(Evaluate)` signals the error and carries on. Asking for (1,1) derivatives
        // makes `SmBSplineSurface::Evaluate` take its "not enough derivatives" branch,
        // `SER(Evaluate(crUV, 2, 2, ...))` (SmBSplineSurfaceNLib.cpp:8335), which returns
        // before the fixed-up tangents are written back: `sEval` keeps the plain NLib
        // derivatives, which is what `non_zero_tangents: false` gives.
        let eval = match surface.evaluate(k, uv_mid, 1, 1, eval_flags(true)) {
            Ok(eval) => eval,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => surface.evaluate(k, uv_mid, 1, 1, eval_flags(false))?,
        };
        let at = |i: usize, j: usize| -> Result<Vec3> {
            eval.get(i)
                .and_then(|r| r.get(j))
                .copied()
                .ok_or_else(|| SolidError::sm_err("geom::grid::SurfaceGridElement::from_leaf_data"))
        };
        let mid_point = at(0, 0)?;
        let mid_du = at(1, 0)?;
        let mid_dv = at(0, 1)?;
        let mut mid_normal = mid_du.cross(mid_dv);
        if mid_normal.length_squared() > EFF_ZERO {
            mid_normal = mid_normal.unitize()?;
        } else {
            // `SE(EvaluateNormal)`: on failure keep the cross product (the unit's
            // `m_sMidNormal` is whatever the call left in it; see findings.md)
            mid_normal = match surface.evaluate_normal(k, uv_mid, Side::Left, Side::Left) {
                Ok(n) => n,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => mid_normal,
            };
        }
        Ok(Self {
            surface: handle,
            uv_domain: *uv_domain,
            bbox: *bbox,
            pseudo_box: *pseudo_box,
            polar_box: *polar_box,
            mid_point,
            mid_du,
            mid_dv,
            mid_normal,
        })
    }
}

impl<O: Copy + PartialEq> SurfaceGridElement<O> {
    /// `SmSurfaceGridElement::Init(pNode)`: the leaf for the `SM_AD_BEZIER_SURFACE`
    /// subdivision tree node `node` of `cache`, in upstream's statement order. The
    /// surface is the node's owner object, the domain and pseudo box those of the
    /// node's `SmBezierPatch`, the box the node's own (unexpanded) one, and the polar
    /// box the one `SmBezierAux2d::GetPolarBox` builds on first use.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the tree has no owner object (upstream `SM_ASSERT`);
    /// `InvalidInput` for a cache without a tree (a node can only come from a built
    /// one: upstream `Init` never fetches the tree, which for a face-owned surface
    /// would rebuild it), for an unknown node and for a node that is not a Bezier
    /// patch node (upstream casts it and reads garbage); those of
    /// [`SurfaceGridElement::from_leaf_data`].
    pub fn init(k: &Kernel, cache: &mut SurfaceCache<O>, node: NodeId) -> Result<Self> {
        const OP: &str = "geom::grid::SurfaceGridElement::init";
        let no_tree = || SolidError::InvalidInput {
            operation: OP,
            reason: "surface cache has no tree",
        };
        let owner = cache
            .tree()
            .ok_or_else(no_tree)?
            .node_owner_object(node)?
            .ok_or(SolidError::sm_err(OP))?;
        let (uv_domain, pseudo_box) = {
            let patch = cache.patch(node)?;
            (patch.aux.uv_domain, *patch.pseudo_box())
        };
        let bbox = cache.tree().ok_or_else(no_tree)?.node(node)?.bbox;
        let polar_box = cache.polar_box(k, node)?;
        let surface = cache.node_surface(Some(owner))?;
        Self::from_leaf_data(
            k,
            owner,
            surface,
            &uv_domain,
            &bbox,
            &pseudo_box,
            &polar_box,
        )
    }
}

/// What a [`GridElement`] stands for (the derived `SmGridElement` classes, and
/// what their `GetElement` returns).
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::large_enum_variant)] // boxing the surface leaf would end `Copy`
pub enum GridItem<O> {
    /// `SmSurfaceGridElement`: one leaf of a surface subdivision.
    Surface(SurfaceGridElement<O>),
    /// `SmPolygonGridElement`: a leaf node of an `SmBrepCache` surface tree.
    Polygon(NodeId),
    /// `SmHierarchyGridElement`: back reference to its owning hierarchy element
    /// (`None` until the element is added to a grid).
    Hierarchy(Option<HierarchyId>),
    /// `SmUserGridElement`: anything the application stores.
    User(O),
}

/// One element registered in a grid (`SmGridElement`); one element may sit in
/// several voxels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridElement<O> {
    /// `m_lMark`: matched with the grid's current mark during a ray walk.
    mark: u64,
    /// The derived-class payload (and so `m_lType`).
    item: GridItem<O>,
    /// `m_sSphereCenter`: centre of the bounding sphere.
    sphere_center: Point3,
    /// `m_dSphereRadius`: radius of the bounding sphere.
    sphere_radius: f64,
    /// `m_sBBox`: bounding box.
    bbox: Extent3d,
}

impl<O: Copy> GridElement<O> {
    /// A fresh element as left by an `Init` (upstream pool memory: mark 0, the
    /// rest set by the caller).
    const fn init(item: GridItem<O>) -> Self {
        Self {
            mark: 0,
            item,
            sphere_center: Vec3::UNDEF,
            sphere_radius: 0.0,
            bbox: Extent3d::new(),
        }
    }

    /// `SmPolygonGridElement::Init`.
    const fn polygon(node: NodeId) -> Self {
        Self::init(GridItem::Polygon(node))
    }

    /// `SmHierarchyGridElement::Init`.
    const fn hierarchy(owner: Option<HierarchyId>) -> Self {
        Self::init(GridItem::Hierarchy(owner))
    }

    /// `SmUserGridElement::Init`.
    const fn user(object: O) -> Self {
        Self::init(GridItem::User(object))
    }

    /// The derived type code (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        match self.item {
            GridItem::Surface(_) => SURFACE_GRID_ELEMENT_TYPE,
            GridItem::Polygon(_) => POLYGON_GRID_ELEMENT_TYPE,
            GridItem::Hierarchy(_) => HIERARCHY_GRID_ELEMENT_TYPE,
            GridItem::User(_) => USER_GRID_ELEMENT_TYPE,
        }
    }

    /// What this element stands for (`GetElement` of the derived classes).
    #[must_use]
    pub const fn item(&self) -> GridItem<O> {
        self.item
    }

    /// Centre of the bounding sphere (`GetSphereCenter`).
    #[must_use]
    pub const fn sphere_center(&self) -> Point3 {
        self.sphere_center
    }

    /// Radius of the bounding sphere (`GetSphereRadius`).
    #[must_use]
    pub const fn sphere_radius(&self) -> f64 {
        self.sphere_radius
    }

    /// Bounding box (`GetBoundingBox`).
    #[must_use]
    pub const fn bounding_box(&self) -> &Extent3d {
        &self.bbox
    }

    /// The ray-walk mark (`m_lMark`).
    #[must_use]
    pub const fn mark(&self) -> u64 {
        self.mark
    }
}

/// An element that can be expanded into many grid elements (`SmHierarchyElement`):
/// currently only a surface whose subdivision leaves go into the voxels
/// (`SmSurfaceHierarchyElement`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HierarchyElement<O> {
    /// `m_vGridElement`: box, bounding sphere and back reference.
    grid_element: GridElement<O>,
    /// `m_dExpansionTolerance`: how much to grow boxes by.
    expansion_tolerance: f64,
    /// `SmSurfaceHierarchyElement::m_pSurface`; `None` for the abstract base class.
    surface: Option<O>,
}

impl<O: Copy> HierarchyElement<O> {
    /// A base-class hierarchy element (`SmHierarchyElement(dExpansionTolerance)`);
    /// expanding it fails, as upstream's "pure virtual" body.
    #[must_use]
    pub const fn new(expansion_tolerance: f64) -> Self {
        Self {
            grid_element: GridElement::hierarchy(None),
            expansion_tolerance,
            surface: None,
        }
    }

    /// A surface hierarchy element
    /// (`SmSurfaceHierarchyElement(pSurface, dExpansionTolerance)`).
    #[must_use]
    pub const fn new_surface(surface: O, expansion_tolerance: f64) -> Self {
        Self {
            grid_element: GridElement::hierarchy(None),
            expansion_tolerance,
            surface: Some(surface),
        }
    }

    /// The embedded grid element (`GetHierarchyGridElement`).
    #[must_use]
    pub const fn grid_element(&self) -> &GridElement<O> {
        &self.grid_element
    }

    /// The expansion tolerance (`m_dExpansionTolerance`).
    #[must_use]
    pub const fn expansion_tolerance(&self) -> f64 {
        self.expansion_tolerance
    }

    /// The surface of a surface hierarchy element (`GetSurface`).
    #[must_use]
    pub const fn surface(&self) -> Option<O> {
        self.surface
    }
}

/// One cell of a [`Grid`] (`SmVoxel`). `Default` is `SmVoxel::Init`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Voxel {
    /// `m_pHierarchyElements`: hierarchy elements whose box meets this voxel
    /// (`None` until the first one is added).
    hierarchy_elements: Option<Vec<HierarchyId>>,
    /// `m_pElements`: grid elements whose box meets this voxel (`None` until the
    /// first one is added).
    elements: Option<Vec<ElementId>>,
}

impl Voxel {
    /// Hierarchy elements in this voxel, in insertion order
    /// (`GetHierarchyElements`; `None` = null list).
    #[must_use]
    pub fn hierarchy_elements(&self) -> Option<&[HierarchyId]> {
        self.hierarchy_elements.as_deref()
    }

    /// Grid elements in this voxel, in insertion order (`GetGridElements`;
    /// `None` = null list).
    #[must_use]
    pub fn grid_elements(&self) -> Option<&[ElementId]> {
        self.elements.as_deref()
    }
}

/// The 3DDDA ray-walk state of a grid (set by [`Grid::setup_3ddda`], advanced by
/// [`Grid::step_3ddda`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Dda {
    /// `m_sRayPoint`: ray start.
    pub ray_point: Point3,
    /// `m_sRayVector`: unit ray direction.
    pub ray_vector: Vec3,
    /// `m_dRayT`: ray parameter where the current voxel is entered.
    pub ray_t: f64,
    /// `m_sCurrPoss`: the point at [`Dda::ray_t`].
    pub curr_pos: Point3,
    /// `m_dNextT`: ray parameters of the next x, y and z voxel planes.
    pub next_t: Vec3,
    /// `m_dDeltaT`: ray parameter between consecutive x, y and z planes.
    pub delta_t: Vec3,
    /// `m_sDeltaXVec`: ray step between consecutive x planes.
    pub delta_x_vec: Vec3,
    /// `m_sDeltaYVec`: ray step between consecutive y planes.
    pub delta_y_vec: Vec3,
    /// `m_sDeltaZVec`: ray step between consecutive z planes.
    pub delta_z_vec: Vec3,
    /// `m_sNextX`: next x-plane crossing.
    pub next_x: Point3,
    /// `m_sNextY`: next y-plane crossing.
    pub next_y: Point3,
    /// `m_sNextZ`: next z-plane crossing.
    pub next_z: Point3,
    /// `m_lStepX/Y/Z`: voxel index increments (`1`, `-1`).
    pub step: [i64; 3],
    /// `m_lOutX/Y/Z`: voxel index that means the ray left the grid (`-1` or the size).
    pub out: [i64; 3],
    /// `m_lAddress`: indices of the current voxel.
    pub address: [i64; 3],
}

/// Output of [`Grid::step_3ddda`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StepHits {
    /// `rbExitedGrid`: the ray left the grid.
    pub exited_grid: bool,
    /// `rSortedElementsInVoxel`: elements whose boxes the ray crosses, by
    /// increasing entry parameter.
    pub elements: Vec<ElementId>,
    /// `rMinDistances`: entry parameter of each element box.
    pub min_distances: Vec<f64>,
    /// `rMaxDistances`: exit parameter of each element box.
    pub max_distances: Vec<f64>,
}

/// What to register with [`Grid::add_element`] (upstream `pElement` and its
/// `SmGridElementType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementSource<O> {
    /// `SM_GE_POLYBREP`: a leaf node of a poly B-rep cache tree.
    PolyBrep(NodeId),
    /// `SM_GE_USERDATA`: application data.
    UserData(O),
}

/// A uniform grid of voxels over a box (`SmGrid`).
#[derive(Debug, Clone, PartialEq)]
pub struct Grid<O> {
    /// `m_sBBox`: extent of the grid.
    bbox: Extent3d,
    /// `m_lSize`: voxels along x, y and z.
    size: [usize; 3],
    /// `m_sVoxelSize`: size of one voxel.
    voxel_size: Vec3,
    /// `m_dVoxelTolerance`.
    voxel_tolerance: f64,
    /// `m_lCurrentMark`.
    current_mark: u64,
    /// `m_aVoxels`, flattened (`x`-major).
    voxels: Vec<Voxel>,
    /// Every grid element, in creation order (the element pools).
    elements: Vec<GridElement<O>>,
    /// `m_pHierarchyElements`: every hierarchy element, in insertion order.
    hierarchy: Vec<HierarchyElement<O>>,
    /// The 3DDDA scratch state.
    dda: Dda,
}

/// `lNumFound`/`rdTEnter`/`rdTExit` of an `IntersectLine` call whose `rdTEnter`
/// and `rdTExit` started at `0.0`.
const fn line_hit(hit: LineHit) -> (u32, f64, f64) {
    match hit {
        LineHit::Miss => (0, 0.0, 0.0),
        LineHit::Touch { t } => (1, t, t),
        LineHit::Through { enter, exit } => (2, enter, exit),
    }
}

/// `sm_InsertionSort`: inserts `elem` with its interval before the first entry
/// with a larger entry parameter.
fn insertion_sort(elem: ElementId, t_enter: f64, t_exit: f64, hits: &mut StepHits) {
    let mut insert_at = hits.min_distances.len();
    for (i, &d) in hits.min_distances.iter().enumerate() {
        if t_enter < d {
            insert_at = i;
            break;
        }
    }
    hits.elements.insert(insert_at, elem);
    hits.min_distances.insert(insert_at, t_enter);
    hits.max_distances.insert(insert_at, t_exit);
}

fn bad_address(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "voxel address outside the grid",
    }
}

fn bad_id(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "unknown grid element id",
    }
}

impl<O: Copy> Grid<O> {
    /// A grid of `size[0] x size[1] x size[2]` empty voxels over `bbox`
    /// (`SmGrid(crGridBBox, lSize)`).
    ///
    /// Fails when a count is zero, the voxel total overflows or the voxels cannot be
    /// allocated (including more than [`crate::limits::MAX_ALLOC_BYTES`] of voxels).
    pub fn new(bbox: &Extent3d, size: [usize; 3]) -> Result<Self> {
        const OP: &str = "geom::grid::Grid::new";
        let [nx, ny, nz] = size;
        if nx == 0 || ny == 0 || nz == 0 {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "zero voxel count",
            });
        }
        let total =
            nx.checked_mul(ny)
                .and_then(|n| n.checked_mul(nz))
                .ok_or(SolidError::InvalidInput {
                    operation: OP,
                    reason: "voxel count overflows",
                })?;

        let mut voxel_size = bbox.size();
        voxel_size.x /= nx as f64;
        voxel_size.y /= ny as f64;
        voxel_size.z /= nz as f64;

        let voxel_tolerance = EFF_ZERO_SQRT * (1.0 + voxel_size.max_dimension()) / 100.0;

        // Upstream's `smos_Calloc` of the voxel array fails softly (`SM_ERR`); above
        // `MAX_ALLOC_BYTES` the port fails the same way without trying.
        let voxels = try_filled(total, Voxel::default()).ok_or_else(|| SolidError::sm_err(OP))?;

        Ok(Self {
            bbox: *bbox,
            size,
            voxel_size,
            voxel_tolerance,
            current_mark: 0,
            voxels,
            elements: Vec::new(),
            hierarchy: Vec::new(),
            dda: Dda::default(),
        })
    }

    /// Extent of the grid (`m_sBBox`).
    #[must_use]
    pub const fn bbox(&self) -> &Extent3d {
        &self.bbox
    }

    /// Voxel counts along x, y and z (`m_lSize`).
    #[must_use]
    pub const fn size(&self) -> [usize; 3] {
        self.size
    }

    /// Size of one voxel (`m_sVoxelSize`).
    #[must_use]
    pub const fn voxel_size(&self) -> Vec3 {
        self.voxel_size
    }

    /// Tolerance of voxel computations (`m_dVoxelTolerance`).
    #[must_use]
    pub const fn voxel_tolerance(&self) -> f64 {
        self.voxel_tolerance
    }

    /// The current ray-walk mark (`m_lCurrentMark`).
    #[must_use]
    pub const fn current_mark(&self) -> u64 {
        self.current_mark
    }

    /// The 3DDDA ray-walk state.
    #[must_use]
    pub const fn dda(&self) -> &Dda {
        &self.dda
    }

    /// Every grid element, in creation order.
    #[must_use]
    pub fn elements(&self) -> &[GridElement<O>] {
        &self.elements
    }

    /// Every hierarchy element, in insertion order (`m_pHierarchyElements`).
    #[must_use]
    pub fn hierarchy_elements(&self) -> &[HierarchyElement<O>] {
        &self.hierarchy
    }

    /// The grid element `id`.
    pub fn element(&self, id: ElementId) -> Result<&GridElement<O>> {
        self.elements
            .get(id.0)
            .ok_or_else(|| bad_id("geom::grid::Grid::element"))
    }

    /// The hierarchy element `id`.
    pub fn hierarchy_element(&self, id: HierarchyId) -> Result<&HierarchyElement<O>> {
        self.hierarchy
            .get(id.0)
            .ok_or_else(|| bad_id("geom::grid::Grid::hierarchy_element"))
    }

    /// Flat index of a signed voxel address, if it is inside the grid.
    fn voxel_index(&self, a: [i64; 3]) -> Option<usize> {
        let ix = usize::try_from(a[0]).ok()?;
        let iy = usize::try_from(a[1]).ok()?;
        let iz = usize::try_from(a[2]).ok()?;
        let [nx, ny, nz] = self.size;
        if ix >= nx || iy >= ny || iz >= nz {
            return None;
        }
        Some((ix * ny + iy) * nz + iz)
    }

    /// The voxel at `address` (`GetVoxel`).
    pub fn voxel(&self, address: VoxelAddress) -> Result<&Voxel> {
        const OP: &str = "geom::grid::Grid::voxel";
        let a = signed(address).ok_or_else(|| bad_address(OP))?;
        self.voxel_index(a)
            .and_then(|i| self.voxels.get(i))
            .ok_or_else(|| bad_address(OP))
    }

    /// The box of the voxel at `address` (`GetVoxelBBox`); any address is
    /// accepted.
    #[must_use]
    pub fn voxel_bbox(&self, address: [i64; 3]) -> Extent3d {
        let lo = self.bbox.min();
        let min = Point3::new(
            lo.x + address[0] as f64 * self.voxel_size.x,
            lo.y + address[1] as f64 * self.voxel_size.y,
            lo.z + address[2] as f64 * self.voxel_size.z,
        );
        let max = min + self.voxel_size;
        Extent3d::from_points(min, max)
    }

    /// The voxel containing `point`, or the nearest one when `point` is outside
    /// the grid, and whether it was outside (`GetVoxelAddress`).
    #[must_use]
    pub fn voxel_address(&self, point: Point3) -> (VoxelAddress, bool) {
        let mut n = point - self.bbox.min();
        let s = self.bbox.size();
        n.x /= s.x;
        n.y /= s.y;
        n.z /= s.z;

        let outside = [n.x, n.y, n.z].iter().any(|&c| c < 0.0 || c > 1.0);

        n.x = n.x.clamp(0.0, 1.0);
        n.y = n.y.clamp(0.0, 1.0);
        n.z = n.z.clamp(0.0, 1.0);

        if n.x > 1.0 - EFF_ZERO {
            n.x -= EFF_ZERO;
        }
        if n.y > 1.0 - EFF_ZERO {
            n.y -= EFF_ZERO;
        }
        if n.z > 1.0 - EFF_ZERO {
            n.z -= EFF_ZERO;
        }
        let [nx, ny, nz] = self.size;
        let mut a = [
            (n.x * nx as f64) as usize,
            (n.y * ny as f64) as usize,
            (n.z * nz as f64) as usize,
        ];
        for (ai, ni) in a.iter_mut().zip(self.size) {
            if *ai == ni {
                *ai -= 1;
            }
        }
        (a, outside)
    }

    /// Every voxel between the voxels of `bbox`'s corners, `x` outermost
    /// (`FindVoxelsInBox`).
    #[must_use]
    pub fn find_voxels_in_box(&self, bbox: &Extent3d) -> Vec<VoxelAddress> {
        let (lo, _) = self.voxel_address(bbox.min());
        let (hi, _) = self.voxel_address(bbox.max());
        let mut out = Vec::new();
        for ix in lo[0]..=hi[0] {
            for iy in lo[1]..=hi[1] {
                for iz in lo[2]..=hi[2] {
                    out.push([ix, iy, iz]);
                }
            }
        }
        out
    }

    /// Mutable voxel at a valid address.
    fn voxel_mut(&mut self, address: VoxelAddress, op: &'static str) -> Result<&mut Voxel> {
        let i = signed(address)
            .and_then(|a| self.voxel_index(a))
            .ok_or_else(|| bad_address(op))?;
        self.voxels.get_mut(i).ok_or_else(|| bad_address(op))
    }

    /// Takes ownership of `element`, sets its grid element's box (`bbox` grown by
    /// the expansion tolerance) and bounding sphere, and lists it in every voxel
    /// that `bbox` meets (`AddHierarchyElement`).
    pub fn add_hierarchy_element(
        &mut self,
        mut element: HierarchyElement<O>,
        bbox: &Extent3d,
    ) -> Result<HierarchyId> {
        const OP: &str = "geom::grid::Grid::add_hierarchy_element";
        let id = HierarchyId(self.hierarchy.len());
        let ge = &mut element.grid_element;
        ge.bbox = *bbox;
        ge.bbox.expand_absolute(element.expansion_tolerance);
        let sb = ge.bbox.sphere_bound();
        ge.sphere_center = sb.center;
        ge.sphere_radius = sb.radius;
        ge.item = GridItem::Hierarchy(Some(id));
        self.hierarchy.push(element);

        for a in self.find_voxels_in_box(bbox) {
            self.voxel_mut(a, OP)?
                .hierarchy_elements
                .get_or_insert_with(Vec::new)
                .push(id);
        }
        Ok(id)
    }

    /// Makes a grid element for `source` with box `bbox` and lists it in every
    /// voxel that `bbox` meets (`AddElement`).
    pub fn add_element(&mut self, source: ElementSource<O>, bbox: &Extent3d) -> Result<ElementId> {
        const OP: &str = "geom::grid::Grid::add_element";
        let mut ge = match source {
            ElementSource::PolyBrep(node) => GridElement::polygon(node),
            ElementSource::UserData(o) => GridElement::user(o),
        };
        self.register_element(&mut ge, bbox, OP)
    }

    /// `AddElement(SM_GE_SURFACE)` with the leaf data supplied by the caller instead
    /// of computed by `SmSurfaceGridElement::Init` from a subdivision tree node (see
    /// [`Grid::add_surface_node`] for that): lists the element in every voxel that
    /// `bbox` meets.
    pub fn add_surface_element(
        &mut self,
        leaf: SurfaceGridElement<O>,
        bbox: &Extent3d,
    ) -> Result<ElementId> {
        let mut ge = GridElement::init(GridItem::Surface(leaf));
        self.register_element(&mut ge, bbox, "geom::grid::Grid::add_surface_element")
    }

    /// The tail of `AddElement`: sphere bound, box, pool entry and voxel lists.
    fn register_element(
        &mut self,
        ge: &mut GridElement<O>,
        bbox: &Extent3d,
        op: &'static str,
    ) -> Result<ElementId> {
        let sb = bbox.sphere_bound();
        ge.sphere_center = sb.center;
        ge.sphere_radius = sb.radius;
        ge.bbox = *bbox;
        let id = ElementId(self.elements.len());
        self.elements.push(*ge);

        for a in self.find_voxels_in_box(bbox) {
            self.voxel_mut(a, op)?
                .elements
                .get_or_insert_with(Vec::new)
                .push(id);
        }
        Ok(id)
    }

    /// `SmSurfaceHierarchyElement::ExpandHierarchy` over the subdivision `tree` of
    /// the element's surface cache (the `smsurf_GetSurfaceCache(m_pSurface)->GetTree()`
    /// the caller holds): for every node in `GetAllTreeNodes` order that is the
    /// lowest `SM_AD_BEZIER_SURFACE` node (its first child, if any, is not one) and
    /// not classified outside, adds a surface element made by `init`
    /// (`SmSurfaceGridElement::Init` of that node, given the element's surface) over
    /// the node box grown by the expansion tolerance. A base-class element fails
    /// with `SM_ERR`, as [`Grid::expand_hierarchy`]. This is the tree-level primitive
    /// of [`Grid::expand_hierarchy`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an unknown id; `SM_ERR` for a base-class element;
    /// `SM_ERR_NULL_POINTER` for a Bezier node without subdivision data (upstream
    /// `NER`); those of `init` and of the voxel lookup.
    pub fn expand_hierarchy_with<T: Copy + PartialEq>(
        &mut self,
        id: HierarchyId,
        tree: &Tree<T>,
        mut init: impl FnMut(O, NodeId) -> Result<SurfaceGridElement<O>>,
    ) -> Result<()> {
        const OP: &str = "geom::grid::Grid::expand_hierarchy_with";
        let element = self.hierarchy.get(id.0).ok_or_else(|| bad_id(OP))?;
        let surface = element.surface.ok_or(SolidError::sm_err(OP))?;
        let expansion_tolerance = element.expansion_tolerance;

        // For every Surface subdivision tree node
        for node_id in tree.all_tree_nodes() {
            let node = tree.node(node_id)?;

            // skip non-leaf nodes: add the lowest SM_AD_BEZIER_SURFACE node
            let child_is_bezier = match node.child1 {
                Some(c) => tree.node(c)?.aux_type == AuxDataType::BezierSurface,
                None => false,
            };
            if node.aux_type != AuxDataType::BezierSurface || child_is_bezier {
                continue;
            }

            // skip leaf-nodes classified outside of associated trim boundaries
            let node_class = match &node.data {
                NodeData::Surface(a) => a.node_class,
                NodeData::None | NodeData::ObjectList(_) | NodeData::Curve(_) => {
                    return Err(SolidError::null_pointer(OP));
                }
            };
            if node_class == NodeClass::Outside {
                continue;
            }

            // expand leaf-node bounding box by tolerance
            let mut bbox = node.bbox;
            bbox.expand_absolute(expansion_tolerance);

            // AddElement(pNode, SM_GE_SURFACE, sBBox)
            let leaf = init(surface, node_id)?;
            self.add_surface_element(leaf, &bbox)?;
        }
        Ok(())
    }

    /// Starts a 3DDDA walk of the ray `start + t * vector` (`Setup3DDDA`): finds
    /// the first voxel and the plane steps, and increments the current mark.
    ///
    /// Returns `true` (upstream `rbHitsNothing`) when the ray misses the grid; the
    /// mark is then unchanged. Fails with `SM_ERR` when `vector` is shorter than
    /// `sqrt(EFF_ZERO)`.
    pub fn setup_3ddda(&mut self, start: Point3, vector: Vec3) -> Result<bool> {
        const OP: &str = "geom::grid::Grid::setup_3ddda";
        if vector.length_squared() < EFF_ZERO {
            return Err(SolidError::sm_err(OP));
        }

        self.dda.ray_point = start;
        self.dda.ray_vector = vector;
        self.dda.ray_vector = self.dda.ray_vector.unitize()?;
        let rp = self.dda.ray_point;
        let rv = self.dda.ray_vector;

        if self.bbox.contains_point(rp, 0.0) {
            let (a, outside) = self.voxel_address(rp);
            if outside {
                return Err(SolidError::sm_err(OP));
            }
            let voxel_box = self.voxel_bbox(signed(a).ok_or_else(|| bad_address(OP))?);
            let (_, t_enter, _) = line_hit(voxel_box.intersect_line(rp, rv)?);
            self.dda.ray_t = t_enter;
        } else {
            let (found, t_enter, _) = line_hit(self.bbox.intersect_line(rp, rv)?);
            if found == 0 {
                return Ok(true);
            }
            self.dda.ray_t = t_enter;
        }

        let test_pnt = rp + (self.dda.ray_t + self.voxel_tolerance) * rv;
        self.dda.curr_pos = rp + self.dda.ray_t * rv;

        let (a, outside) = self.voxel_address(test_pnt);
        let address = signed(a).ok_or_else(|| bad_address(OP))?;
        self.dda.address = address;
        if outside {
            return Ok(true);
        }

        let voxel_box = self.voxel_bbox(address);
        let (vmin, vmax) = (voxel_box.min(), voxel_box.max());
        let ray_t = self.dda.ray_t;
        let cur = self.dda.curr_pos;
        let vs = self.voxel_size;
        let [nx, ny, nz] = signed(self.size).ok_or_else(|| bad_address(OP))?;
        let axes = [
            AxisStep::new(ray_t, rv.x, vmin.x, vmax.x, cur.x, vs.x, nx),
            AxisStep::new(ray_t, rv.y, vmin.y, vmax.y, cur.y, vs.y, ny),
            AxisStep::new(ray_t, rv.z, vmin.z, vmax.z, cur.z, vs.z, nz),
        ];
        let d = &mut self.dda;
        let [ax, ay, az] = axes;
        d.next_t = Vec3::new(ax.next_t, ay.next_t, az.next_t);
        d.delta_t = Vec3::new(ax.delta_t, ay.delta_t, az.delta_t);
        for ((axis, step), out) in axes.iter().zip(&mut d.step).zip(&mut d.out) {
            // an axis the ray does not move along keeps its step and exit index
            if let Some((s, o)) = axis.step_out {
                *step = s;
                *out = o;
            }
        }

        d.delta_x_vec = rv * d.delta_t.x;
        d.delta_y_vec = rv * d.delta_t.y;
        d.delta_z_vec = rv * d.delta_t.z;

        if d.next_t.x < BIG_DOUBLE {
            d.next_x = rp + d.next_t.x * rv;
        }
        if d.next_t.y < BIG_DOUBLE {
            d.next_y = rp + d.next_t.y * rv;
        }
        if d.next_t.z < BIG_DOUBLE {
            d.next_z = rp + d.next_t.z * rv;
        }

        self.current_mark = self.current_mark.wrapping_add(1);
        Ok(false)
    }

    /// Walks the ray from voxel to voxel until some voxel holds elements whose
    /// boxes the ray crosses no later than `min_found_t`, or the ray leaves the
    /// grid (`Step3DDDA`). Each element is reported once per walk (it is marked
    /// with the current mark).
    pub fn step_3ddda(&mut self, min_found_t: f64) -> Result<StepHits> {
        const OP: &str = "geom::grid::Grid::step_3ddda";
        let mut hits = StepHits::default();
        let rp = self.dda.ray_point;
        let rv = self.dda.ray_vector;
        let mark = self.current_mark;

        // Every step moves one address coordinate one voxel towards its exit, so a
        // walk that advances leaves the grid within nx + ny + nz steps. Upstream
        // loops forever when the chosen axis does not move (step 0, e.g. all
        // plane parameters at SM_BIG_DOUBLE on a grid with huge voxels).
        let max_steps = self
            .size
            .iter()
            .fold(1usize, |acc, &n| acc.saturating_add(n));
        let mut steps = 0usize;

        loop {
            steps += 1;
            if steps > max_steps {
                return Err(SolidError::InvalidInput {
                    operation: OP,
                    reason: "3DDDA ray walk does not advance",
                });
            }
            let vi = self
                .voxel_index(self.dda.address)
                .ok_or_else(|| bad_address(OP))?;
            let voxel = self.voxels.get(vi).ok_or_else(|| bad_address(OP))?;

            if let Some(list) = &voxel.elements {
                for &eid in list {
                    let e = self.elements.get_mut(eid.0).ok_or_else(|| bad_id(OP))?;

                    if e.mark == mark {
                        continue;
                    }

                    let vec_to_center = e.sphere_center - rp;
                    let center_t = vec_to_center.dot(rv);
                    let point_on_ray = rp + center_t * rv;
                    let vec_to = point_on_ray - e.sphere_center;
                    let dist_sq = vec_to.length_squared();

                    if dist_sq > e.sphere_radius * e.sphere_radius {
                        continue;
                    }

                    let (found, t_enter, t_exit) = line_hit(e.bbox.intersect_line(rp, rv)?);

                    if found == 0 {
                        continue;
                    }
                    if t_exit < -EFF_ZERO {
                        continue;
                    }
                    if t_enter > min_found_t {
                        continue;
                    }

                    e.mark = mark;
                    insertion_sort(eid, t_enter, t_exit, &mut hits);
                }
            }

            let d = &mut self.dda;
            if d.next_t.x < d.next_t.y && d.next_t.x < d.next_t.z {
                d.address[0] += d.step[0];
                if d.address[0] == d.out[0] {
                    hits.exited_grid = true;
                    return Ok(hits);
                }
                d.ray_t = d.next_t.x;
                d.next_t.x += d.delta_t.x;
                d.curr_pos = d.next_x;
                d.next_x += d.delta_x_vec;
            } else if d.next_t.y < d.next_t.z {
                d.address[1] += d.step[1];
                if d.address[1] == d.out[1] {
                    hits.exited_grid = true;
                    return Ok(hits);
                }
                d.ray_t = d.next_t.y;
                d.next_t.y += d.delta_t.y;
                d.curr_pos = d.next_y;
                d.next_y += d.delta_y_vec;
            } else {
                d.address[2] += d.step[2];
                if d.address[2] == d.out[2] {
                    hits.exited_grid = true;
                    return Ok(hits);
                }
                d.ray_t = d.next_t.z;
                d.next_t.z += d.delta_t.z;
                d.curr_pos = d.next_z;
                d.next_z += d.delta_z_vec;
            }

            if !hits.elements.is_empty() {
                break;
            }
        }
        Ok(hits)
    }
}

/// One axis of the 3DDDA set-up in [`Grid::setup_3ddda`].
#[derive(Clone, Copy)]
struct AxisStep {
    /// `m_dNextT` component: ray parameter of the next voxel plane on this axis.
    next_t: f64,
    /// `m_dDeltaT` component: ray parameter between consecutive planes.
    delta_t: f64,
    /// `(m_lStep, m_lOut)` for this axis; `None` when the ray does not move along it.
    step_out: Option<(i64, i64)>,
}

impl AxisStep {
    /// The set-up for ray component `rv` entering the voxel `[vmin, vmax]` at
    /// `cur` (ray parameter `ray_t`), with voxel size `vs` and `n` voxels.
    fn new(ray_t: f64, rv: f64, vmin: f64, vmax: f64, cur: f64, vs: f64, n: i64) -> Self {
        if rv.abs() < EFF_ZERO {
            Self {
                next_t: BIG_DOUBLE,
                delta_t: 0.0,
                step_out: None,
            }
        } else if rv < 0.0 {
            Self {
                next_t: ray_t + (vmin - cur) / rv,
                delta_t: vs / -rv,
                step_out: Some((-1, -1)),
            }
        } else {
            Self {
                next_t: ray_t + (vmax - cur) / rv,
                delta_t: vs / rv,
                step_out: Some((1, n)),
            }
        }
    }
}

/// The operations that read a surface cache, whose handles need `PartialEq`.
impl<O: Copy + PartialEq> Grid<O> {
    /// Embeds the subdivision leaves of hierarchy element `id` into the voxels
    /// (`SmSurfaceHierarchyElement::ExpandHierarchy`; on a base-class element,
    /// `SmHierarchyElement::ExpandHierarchy`, which fails with `SM_ERR`).
    ///
    /// `cache` is `smsurf_GetSurfaceCache(m_pSurface)`: the one cache of the element's
    /// surface, which the caller holds and keeps between calls (so its polar boxes are
    /// built once). The leaves are made by [`SurfaceGridElement::init`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an unknown id and for a cache of another surface (upstream
    /// always fetches the cache of `m_pSurface`); `SM_ERR` for a base-class element,
    /// before the cache is touched; those of [`Grid::expand_hierarchy_with`] and of
    /// the cache's tree build.
    pub fn expand_hierarchy(
        &mut self,
        k: &Kernel,
        id: HierarchyId,
        cache: &mut SurfaceCache<O>,
    ) -> Result<()> {
        self.expand_hierarchy_in(k, id, cache, true)
    }

    /// [`Grid::expand_hierarchy`] for a cache whose `GetTree` the caller has already
    /// run (a face-owned trimmed cache, whose tree build needs its face:
    /// [`TrimSrfCache::get_tree_with_face`](crate::surface::trim_srf_cache::TrimSrfCache::get_tree_with_face)):
    /// the cache's tree is read as is, never rebuilt.
    ///
    /// # Errors
    ///
    /// As [`Grid::expand_hierarchy`]; `InvalidInput` when the cache has no tree.
    pub fn expand_hierarchy_fetched(
        &mut self,
        k: &Kernel,
        id: HierarchyId,
        cache: &mut SurfaceCache<O>,
    ) -> Result<()> {
        self.expand_hierarchy_in(k, id, cache, false)
    }

    fn expand_hierarchy_in(
        &mut self,
        k: &Kernel,
        id: HierarchyId,
        cache: &mut SurfaceCache<O>,
        fetch: bool,
    ) -> Result<()> {
        const OP: &str = "geom::grid::Grid::expand_hierarchy";
        let element = self.hierarchy.get(id.0).ok_or_else(|| bad_id(OP))?;
        let surface = element.surface.ok_or(SolidError::sm_err(OP))?;
        if cache.surface_handle() != Some(surface) {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "surface cache is not the cache of the element's surface",
            });
        }
        // The clone lets `init` borrow the cache mutably (it builds polar boxes).
        let tree = if fetch {
            cache.get_tree(k)?.clone()
        } else {
            cache
                .tree()
                .ok_or(SolidError::InvalidInput {
                    operation: OP,
                    reason: "surface cache has no tree",
                })?
                .clone()
        };
        self.expand_hierarchy_with(id, &tree, |_, node| {
            SurfaceGridElement::init(k, cache, node)
        })
    }

    /// `AddElement(pNode, SM_GE_SURFACE, bbox)`: `SmSurfaceGridElement::Init` of the
    /// Bezier patch node `node` of `cache` first (polar box included), then the
    /// sphere bound, box and voxel lists as [`Grid::add_surface_element`].
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceGridElement::init`] and of the voxel lookup.
    pub fn add_surface_node(
        &mut self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        node: NodeId,
        bbox: &Extent3d,
    ) -> Result<ElementId> {
        let leaf = SurfaceGridElement::init(k, cache, node)?;
        self.add_surface_element(leaf, bbox)
    }
}

/// A voxel address (or count triple) as upstream `long`s.
fn signed(a: [usize; 3]) -> Option<[i64; 3]> {
    Some([
        i64::try_from(a[0]).ok()?,
        i64::try_from(a[1]).ok()?,
        i64::try_from(a[2]).ok()?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::polar_box::PolarBoxState;
    use crate::geom::vector2d::{Point2, Vec2};
    use crate::kernel::Kernel;
    use crate::nurbs::b_spline_surface::p1::SurfParamType;
    use crate::surface::plane::Plane;
    use crate::surface::surface::p2::surface_cache;
    use crate::surface::surface::Surface;
    use crate::surface::surface_cache::SurfaceCache;

    fn unit_grid(n: usize) -> Grid<u32> {
        let bbox = Extent3d::from_bounds(0.0, 0.0, 0.0, 1.0, 1.0, 1.0);
        Grid::new(&bbox, [n, n, n]).unwrap()
    }

    #[test]
    fn construction() {
        let bbox = Extent3d::from_bounds(0.0, 0.0, 0.0, 2.0, 4.0, 6.0);
        let g: Grid<u32> = Grid::new(&bbox, [2, 4, 3]).unwrap();
        assert_eq!(g.voxel_size(), Vec3::new(1.0, 1.0, 2.0));
        assert_eq!(g.voxel_tolerance(), EFF_ZERO_SQRT * 3.0 / 100.0);
        assert!(g.voxel([1, 3, 2]).is_ok());
        assert!(g.voxel([2, 0, 0]).is_err());
        assert!(Grid::<u32>::new(&bbox, [0, 1, 1]).is_err());
        assert!(Grid::<u32>::new(&bbox, [usize::MAX, 2, 1]).is_err());
    }

    /// Fuzz regression (geom.grid fuzz09254): with voxels of size ~1e299 the
    /// plane parameters reach `SM_BIG_DOUBLE` and the walk picks an axis that does
    /// not move; upstream loops forever, the port stops with an error.
    #[test]
    fn stalled_walk_is_an_error() {
        let bbox = Extent3d::from_bounds(-1e-4, -1e-4, -1e-4, 1e300, 1.0001, 1.0001);
        let mut g: Grid<u32> = Grid::new(&bbox, [10, 10, 10]).unwrap();
        let start = Point3::new(1.000003, 0.5, 0.5);
        assert!(!g.setup_3ddda(start, Vec3::new(1.0, 0.0, 0.0)).unwrap());
        assert!(matches!(
            g.step_3ddda(1e20),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn voxel_address_and_box() {
        let g = unit_grid(4);
        assert_eq!(
            g.voxel_address(Point3::new(0.3, 0.6, 0.99)),
            ([1, 2, 3], false)
        );
        assert_eq!(
            g.voxel_address(Point3::new(1.0, 1.0, 1.0)),
            ([3, 3, 3], false)
        );
        assert_eq!(
            g.voxel_address(Point3::new(-5.0, 2.0, 0.5)),
            ([0, 3, 2], true)
        );
        let b = g.voxel_bbox([1, 2, 3]);
        assert_eq!(b.min(), Point3::new(0.25, 0.5, 0.75));
        assert_eq!(b.max(), Point3::new(0.5, 0.75, 1.0));
    }

    #[test]
    fn find_and_add() {
        let mut g = unit_grid(4);
        let b = Extent3d::from_bounds(0.1, 0.1, 0.1, 0.4, 0.2, 0.1);
        assert_eq!(g.find_voxels_in_box(&b), vec![[0, 0, 0], [1, 0, 0]]);
        let id = g.add_element(ElementSource::UserData(7), &b).unwrap();
        assert_eq!(g.voxel([1, 0, 0]).unwrap().grid_elements(), Some(&[id][..]));
        assert_eq!(g.voxel([2, 0, 0]).unwrap().grid_elements(), None);
        assert_eq!(g.element(id).unwrap().type_code(), USER_GRID_ELEMENT_TYPE);
        assert_eq!(g.element(id).unwrap().item(), GridItem::User(7));

        let h = g
            .add_hierarchy_element(HierarchyElement::new(0.01), &b)
            .unwrap();
        let he = g.hierarchy_element(h).unwrap();
        assert_eq!(he.grid_element().item(), GridItem::Hierarchy(Some(h)));
        assert_eq!(he.grid_element().bounding_box().min().x, 0.1 - 0.01);
        // a base-class element fails before the cache is touched
        let k = &Kernel::default();
        let mut cache = plane_cache(k, Some(3));
        assert!(matches!(
            g.expand_hierarchy(k, h, &mut cache),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR,
                ..
            })
        ));
        assert!(matches!(
            g.expand_hierarchy(k, HierarchyId(99), &mut cache),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    fn unit_plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn plane_cache(k: &Kernel, handle: Option<u32>) -> SurfaceCache<u32> {
        surface_cache(k, &unit_plane(), handle).unwrap()
    }

    /// The box of the cache's top node and the ids of its lowest Bezier leaves.
    fn top_box_and_leaves(k: &Kernel, cache: &mut SurfaceCache<u32>) -> (Extent3d, Vec<NodeId>) {
        let tree = cache.get_tree(k).unwrap().clone();
        let top_box = tree.node(tree.top_node().unwrap()).unwrap().bbox;
        let leaves = tree
            .all_tree_nodes()
            .into_iter()
            .filter(|&n| {
                let node = tree.node(n).unwrap();
                node.aux_type == AuxDataType::BezierSurface
                    && node.child1.is_none_or(|c| {
                        tree.node(c).unwrap().aux_type != AuxDataType::BezierSurface
                    })
            })
            .collect();
        (top_box, leaves)
    }

    fn surface_leaves(g: &Grid<u32>) -> Vec<(ElementId, SurfaceGridElement<u32>)> {
        g.elements()
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e.item() {
                GridItem::Surface(leaf) => Some((ElementId(i), leaf)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn expanding_a_surface_hierarchy_adds_its_leaves() {
        let k = &Kernel::default();
        let mut g = unit_grid(4);
        let mut cache = plane_cache(k, Some(3));
        let (top_box, leaves) = top_box_and_leaves(k, &mut cache);
        let h = g
            .add_hierarchy_element(HierarchyElement::new_surface(3, 0.05), &top_box)
            .unwrap();
        g.expand_hierarchy(k, h, &mut cache).unwrap();

        let added = surface_leaves(&g);
        assert!(!leaves.is_empty());
        assert_eq!(added.len(), leaves.len());
        for (id, leaf) in added {
            // the owner object of the node, not the hierarchy element's index
            assert_eq!(leaf.surface, 3);
            // the leaf keeps the node box, the grid element has it grown
            let mut grown = leaf.bbox;
            grown.expand_absolute(0.05);
            assert_eq!(g.element(id).unwrap().bounding_box(), &grown);
            assert_ne!(leaf.bbox, grown);
            // a plane's normals are one vector
            assert_eq!(leaf.polar_box.state(), PolarBoxState::SinglePoint);
            assert!((leaf.mid_normal.z.abs() - 1.0).abs() < 1e-12);
            assert!(leaf.uv_domain.contains_point(Point2::new(0.5, 0.5), 0.0));
        }
    }

    /// Upstream `Init` reads a node of an existing tree: it never fetches (so never
    /// builds or, for a face-owned surface, rebuilds) the tree. Only `ExpandHierarchy`
    /// does, once.
    #[test]
    fn init_reads_the_tree_and_never_builds_it() {
        use crate::surface::surface_cache::SurfaceCacheOptions;
        let k = &Kernel::default();
        let mut g = unit_grid(4);
        // a cache whose tree is not built (a node can only come from a built tree)
        let mut cache = SurfaceCache::<u32>::new(
            unit_plane(),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            SurfaceCacheOptions::default(),
        );
        cache.set_surface_handle(Some(3));
        assert!(cache.tree().is_none());
        let b = Extent3d::from_bounds(0.1, 0.1, 0.1, 0.4, 0.2, 0.1);
        assert!(matches!(
            g.add_surface_node(k, &mut cache, NodeId(0), &b),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(cache.tree().is_none());
        assert!(g.elements().is_empty());
    }

    #[test]
    fn a_cache_of_another_surface_is_rejected() {
        let k = &Kernel::default();
        let mut g = unit_grid(4);
        let mut cache = plane_cache(k, Some(3));
        let (top_box, _) = top_box_and_leaves(k, &mut cache);
        let h = g
            .add_hierarchy_element(HierarchyElement::new_surface(3, 0.0), &top_box)
            .unwrap();
        let mut other = plane_cache(k, Some(4));
        assert!(matches!(
            g.expand_hierarchy(k, h, &mut other),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(g.elements().is_empty());
    }

    #[test]
    fn add_surface_node_makes_the_leaf_from_the_node() {
        let k = &Kernel::default();
        let mut g = unit_grid(4);
        let mut cache = plane_cache(k, Some(3));
        let (_, leaves) = top_box_and_leaves(k, &mut cache);
        let node = *leaves.first().unwrap();
        let node_box = cache.tree().unwrap().node(node).unwrap().bbox;
        let id = g.add_surface_node(k, &mut cache, node, &node_box).unwrap();
        let e = g.element(id).unwrap();
        assert_eq!(e.type_code(), SURFACE_GRID_ELEMENT_TYPE);
        assert_eq!(e.bounding_box(), &node_box);
        let GridItem::Surface(leaf) = e.item() else {
            panic!("not a surface element")
        };
        assert_eq!(leaf.surface, 3);
        assert_eq!(leaf.bbox, node_box);
        // the sphere bound and voxel lists are those of `register_element`
        let mid = node_box.sphere_bound();
        assert_eq!(e.sphere_center(), mid.center);
        let (v, _) = g.voxel_address(node_box.min());
        assert!(g.voxel(v).unwrap().grid_elements().unwrap().contains(&id));
    }

    #[test]
    fn add_surface_node_rejects_what_has_no_patch() {
        let k = &Kernel::default();
        let mut g = unit_grid(4);
        let mut cache = plane_cache(k, Some(3));
        let b = Extent3d::from_bounds(0.1, 0.1, 0.1, 0.4, 0.2, 0.1);
        // a node of the tree that is not a Bezier patch node (upstream casts it
        // anyway and reads garbage): splitting a node with `only_aux_data` makes
        // `SM_AD_AUX_DATA` children
        let top = cache.get_tree(k).unwrap().top_node().unwrap();
        cache.set_subdivision_handles(Some(|n| 100 + n as u32));
        cache
            .subdivide_node(k, top, SurfParamType::U, true)
            .unwrap();
        let aux = cache.tree().unwrap().node(top).unwrap().child1.unwrap();
        assert_eq!(
            cache.tree().unwrap().node(aux).unwrap().aux_type,
            AuxDataType::AuxData
        );
        assert!(matches!(
            g.add_surface_node(k, &mut cache, aux, &b),
            Err(SolidError::InvalidInput { .. })
        ));
        // a node the tree does not have
        assert!(matches!(
            g.add_surface_node(k, &mut cache, NodeId(usize::MAX), &b),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(g.elements().is_empty());
        // a cache without a surface handle: the node has no owner object
        // (upstream `SM_ASSERT`)
        let mut anonymous = plane_cache(k, None);
        let (_, leaves) = top_box_and_leaves(k, &mut anonymous);
        assert!(matches!(
            g.add_surface_node(k, &mut anonymous, leaves[0], &b),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR,
                ..
            })
        ));
        assert!(g.elements().is_empty());
    }

    #[test]
    fn expand_hierarchy_over_a_tree() {
        use crate::geom::tree::{SurfaceAux, TreeNode};
        let mut g = unit_grid(4);
        let mut t: Tree<u32> = Tree::new(None);
        let mk = |t: &mut Tree<u32>, parent, aux_type, class, bbox| {
            t.add_node(TreeNode {
                parent,
                aux_type,
                bbox,
                data: NodeData::Surface(SurfaceAux {
                    node_class: class,
                    ..SurfaceAux::default()
                }),
                ..TreeNode::new()
            })
        };
        let left = Extent3d::from_bounds(0.1, 0.1, 0.1, 0.2, 0.2, 0.2);
        let right = Extent3d::from_bounds(0.6, 0.6, 0.6, 0.7, 0.7, 0.7);
        let all = Extent3d::from_bounds(0.1, 0.1, 0.1, 0.7, 0.7, 0.7);
        let root = mk(
            &mut t,
            None,
            AuxDataType::BezierSurface,
            NodeClass::Unknown,
            all,
        );
        let a = mk(
            &mut t,
            Some(root),
            AuxDataType::BezierSurface,
            NodeClass::Inside,
            left,
        );
        let b = mk(
            &mut t,
            Some(root),
            AuxDataType::BezierSurface,
            NodeClass::Outside,
            right,
        );
        t.node_mut(root).unwrap().child1 = Some(a);
        t.node_mut(root).unwrap().child2 = Some(b);
        t.set_top_node(Some(root));

        let base = g
            .add_hierarchy_element(HierarchyElement::new(0.0), &all)
            .unwrap();
        assert!(g
            .expand_hierarchy_with(base, &t, |_, _| unreachable!())
            .is_err());

        let h = g
            .add_hierarchy_element(HierarchyElement::new_surface(9, 0.05), &all)
            .unwrap();
        let mut seen = Vec::new();
        g.expand_hierarchy_with(h, &t, |s, n| {
            seen.push((s, n));
            Ok(SurfaceGridElement {
                surface: s,
                uv_domain: Extent2d::default(),
                bbox: left,
                pseudo_box: PseudoBox::default(),
                polar_box: PolarBox::default(),
                mid_point: Vec3::ZERO,
                mid_du: Vec3::ZERO,
                mid_dv: Vec3::ZERO,
                mid_normal: Vec3::ZERO,
            })
        })
        .unwrap();
        // the root has a Bezier first child, the right leaf is outside
        assert_eq!(seen, vec![(9, a)]);
        let mut grown = left;
        grown.expand_absolute(0.05);
        let e = g.voxel([0, 0, 0]).unwrap().grid_elements().unwrap();
        let last = *e.last().unwrap();
        assert_eq!(g.element(last).unwrap().bounding_box(), &grown);
    }

    #[test]
    fn ray_walk() {
        let mut g = unit_grid(4);
        let near = Extent3d::from_bounds(0.6, 0.45, 0.45, 0.7, 0.55, 0.55);
        let far = Extent3d::from_bounds(0.1, 0.4, 0.4, 0.2, 0.6, 0.6);
        let a = g.add_element(ElementSource::UserData(1), &far).unwrap();
        let b = g.add_element(ElementSource::UserData(2), &near).unwrap();
        // Ray along -x through the middle.
        assert!(!g
            .setup_3ddda(Point3::new(2.0, 0.5, 0.5), Vec3::new(-1.0, 0.0, 0.0))
            .unwrap());
        assert_eq!(g.dda().ray_t, 1.0);
        assert_eq!(g.dda().address, [3, 2, 2]);
        let h = g.step_3ddda(BIG_DOUBLE).unwrap();
        assert!(!h.exited_grid);
        assert_eq!(h.elements, vec![b]);
        assert!((h.min_distances[0] - 1.3).abs() < 1e-12);
        // The last voxel's hits come back together with the exit.
        let h = g.step_3ddda(BIG_DOUBLE).unwrap();
        assert!(h.exited_grid);
        assert_eq!(h.elements, vec![a]);
        assert!(g.step_3ddda(BIG_DOUBLE).is_err());

        // Misses and bad directions.
        assert!(g
            .setup_3ddda(Point3::new(2.0, 2.0, 2.0), Vec3::new(1.0, 0.0, 0.0))
            .unwrap());
        assert!(g
            .setup_3ddda(Point3::ZERO, Vec3::new(1e-7, 0.0, 0.0))
            .is_err());
    }

    #[test]
    fn insertion_order() {
        let mut h = StepHits::default();
        insertion_sort(ElementId(0), 2.0, 3.0, &mut h);
        insertion_sort(ElementId(1), 1.0, 3.0, &mut h);
        insertion_sort(ElementId(2), 2.0, 4.0, &mut h);
        assert_eq!(h.elements, vec![ElementId(1), ElementId(0), ElementId(2)]);
        assert_eq!(h.max_distances, vec![3.0, 3.0, 4.0]);
    }
}
