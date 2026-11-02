//! Binary trees of axis-aligned boxes: object spatial trees (insert, remove and
//! box queries over a set of bounded objects) and the node relations of surface
//! subdivision trees (leaf neighbours, mark/class propagation).
//!
//! Upstream: `source/SMLib/src/SmTree.cpp`, `source/SMLib/inc/SmTree.h`.
//!
//! | Rust                                   | Upstream                                        |
//! |----------------------------------------|-------------------------------------------------|
//! | [`Tree`]                               | `SmTree`                                        |
//! | [`Tree::new`]                          | `SmTree(SmObject*)`                             |
//! | [`Tree::from_point`]                   | `SmTree(const SmPoint3d&, double, SmTreeNode*, SmObjectList*)` |
//! | [`Tree::new_spatial`]                  | `SmTree(const SmExtent3d&, ULONG, ULONG)`       |
//! | `Drop` (implicit)                      | `~SmTree`                                       |
//! | [`Tree::owner`]                        | `SmTree::GetOwner`, `SmTreeNode::GetOwner`      |
//! | [`Tree::set_owner_object`]             | `SmTree::SetOwnerObject`                        |
//! | [`Tree::owner_object`]                 | `SmTree::GetOwnerObject`                        |
//! | [`Tree::node_owner_object`]            | `SmTreeNode::GetOwnerObject`                    |
//! | [`Tree::map_objects`]                 | Rust handle adapter (no numerical changes)       |
//! | [`Tree::top_node`]                     | `SmTree::GetTopNode`                            |
//! | [`Tree::TYPE`], [`Tree::type_code`]    | `SmTree::GetType` (`SmTree_TYPE`)               |
//! | [`Tree::type_string`]                  | `SmTree::GetTypeString`                         |
//! | [`Tree::is_kind_of`]                   | `SmTree::IsKindOf`                              |
//! | [`Tree::add_to_spatial_tree`]          | `SmTree::AddToSpatialTree`                      |
//! | [`Tree::rm_from_spatial_tree`]         | `SmTree::RmFromSpatialTree`                     |
//! | [`Tree::all_tree_nodes`]               | `SmTree::GetAllTreeNodes`                       |
//! | [`Tree::find_node`]                    | `SmTree::FindNode`                              |
//! | [`Tree::node_object_list`]             | `SmTreeNode::GetObjectList(SmTArray<SmObjectList*>&)` |
//! | [`Tree::object_list`]                  | `SmTree::GetObjectList`                         |
//! | [`Tree::objects_in_box`]               | `SmTree::GetObjectsInBox`                       |
//! | [`Tree::intersects_box`]               | `SmTree::IntersectsBox`                         |
//! | [`Tree::find_object`]                  | `SmObjectList::FindObject`                      |
//! | [`Tree::first_bezier_patch`]           | `SmTreeNode::GetFirstBezierPatch` (patch data through caller resolvers) |
//! | [`Tree::propagate_to_parents`]         | `SmTreeNode::PropagateToParents`                |
//! | [`Tree::neighbors`]                    | `SmTreeNode::GetNeighbors`                      |
//! | [`Tree::left_neighbors`] .. [`Tree::top_neighbors`] | `SmTreeNode::GetLeftNeighbors` .. `GetTopNeighbors` |
//! | `collect_neighbors` (private)          | the shared body of `GetLeftNeighbors` .. `GetTopNeighbors` |
//! | `side` (private)                       | `SmTreeNode::GetRightSide`, `GetLeftSide`, `GetTopSide`, `GetBotSide` |
//! | [`TreeNode`], [`TreeNode::new`]        | `SmTreeNode`, `SmTreeNode()`                    |
//! | [`TreeNode::bounding_box`]             | `SmTreeNode::GetBoundingBox`                    |
//! | [`ObjectList`]                         | `SmObjectList`                                  |
//! | [`Tree::curve_spans`], [`Tree::curve_parents`], [`Tree::curve_aux`], [`Tree::curve_span`], [`Tree::set_curve_data`], [`CurveSpanRef`] | the owning `SmCurveCache`'s `m_sBezMgr` / `m_sParentMgr` and the casts of `m_pData` to `SmBezierAux1d*` / `SmBezierSpan*` |
//! | [`SurfaceAux`]                         | the `SmBezierAux2d` fields read here (`m_eSplitDir`, `m_bIsMarked`, `m_eNodeClass`, `m_pFace`, `mBA_pSurface`) and the arena slot of the rest |
//! | [`AuxDataType`], [`NodeGeomType`], [`SplitDir`], [`NodeClass`] | `SmAuxDataType`, `SmNodeGeomType`, `SmSurfParamType` (U/V), `SmNodeClassType` |
//!
//! Deviations:
//! - **Handle space.** `Tree::retag_curve_tree` (not upstream) copies a curve
//!   subdivision tree into another handle space, for the curve cache memo
//!   (`curve.curve_p5`).
//! - Pointers become arena indices: nodes live in [`Tree::nodes`] ([`NodeId`]) and
//!   object-list items in [`Tree::lists`] ([`ListId`]), both in allocation order,
//!   exactly the order of upstream's `SmMemBlockMgr::GetActiveElements`. Resetting
//!   a memory manager clears its arena. Block-size arguments have no counterpart.
//! - Objects are a caller-chosen `Copy + PartialEq` handle `O` (upstream
//!   `SmObject*`); a null object pointer is `None`.
//! - **Recursion.** `AddToSpatialTree` re-inserts every object after growing the top
//!   box around all of them, so a re-insertion never rebuilds again (NaN sides
//!   compare as contained). The nesting is still capped at
//!   [`MAX_SPATIAL_REBUILD_DEPTH`] (1: each level re-inserts every object); a deeper
//!   rebuild is [`SolidError::InvalidInput`], ignored by the re-inserting caller as
//!   upstream ignores its status.
//! - `SmTreeNode::m_pData` is [`NodeData`]: the head of an object list, the
//!   [`SurfaceAux`] subdivision data, or nothing. Code that would cast `m_pData` to
//!   the wrong type (undefined behaviour upstream) returns
//!   [`SolidError::InvalidInput`]; so does any use of a missing node/list id, a
//!   missing top node, a cyclic object list or cyclic parent/child links.
//! - The neighbour searches return a new `Vec` instead of appending to the
//!   caller's `SmTArray`; the four direction-specific recursions (and the four
//!   `Get*Side` functions) share one body parameterised by the
//!   search direction. Parent recursion is a loop; side descent uses an explicit
//!   DFS continuation stack in the same child order, retaining the cycle guard
//!   at depth > node count without an arbitrary geometry-depth bound.
//! - The single-point constructor always allocates its node (and object-list
//!   item) in the tree's arenas; upstream can also use caller-owned memory, which
//!   is then invisible to `GetAllTreeNodes`/`GetNodeCount`.
//! - The tree owner is a [`TreeOwner`] kind. A curve cache
//!   ([`crate::curve::curve_cache::CurveCache`]) hands its span data (upstream
//!   reached through `m_pData` pointers into the cache) and its curve handle to the
//!   tree it builds ([`Tree::set_curve_data`]); `GetOwnerObject` of a curve tree
//!   returns that handle. A surface cache
//!   ([`crate::surface::surface_cache::p1::SurfaceCache`]) hands its surface handle
//!   to its tree ([`Tree::set_owner_surface`]); `GetOwnerObject` of a surface tree
//!   returns it. The surface decomposition data (`SmBezierPatch`, subdivision
//!   surfaces) stays in the cache, so [`Tree::first_bezier_patch`] takes the two
//!   reads that need it as caller resolvers (`SmBezierPatch::GetBezierPtr` of a
//!   patch node, `SmBSplineSurface::GetGwNurbPointer` of a subdivision surface);
//!   `SurfaceCache::first_bezier_patch` answers the same walk from the cache.
//! - `SM_ASSERT`, `WARN` (bad split-direction data) and debug drawing are not
//!   ported; a `WARN` branch simply adds nothing, as upstream.

use crate::curve::curve_cache::{BezierAux1d, BezierSpan};
use crate::error::{Result, SolidError};
use crate::geom::extent3d::Extent3d;
use crate::geom::object;
use crate::math::{Point3, EFF_ZERO, EFF_ZERO_SQ};

/// `SmTree_TYPE` (`CONT_BASE_TYPE + 150`).
const SM_TREE_TYPE: i32 = 12150;

/// Deepest nesting of top-box rebuilds in [`Tree::add_to_spatial_tree`] (see the
/// module's Deviations).
pub const MAX_SPATIAL_REBUILD_DEPTH: usize = 1;

/// Index of a node in [`Tree::nodes`] (upstream `SmTreeNode*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

/// Index of an object-list item in [`Tree::lists`] (upstream `SmObjectList*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ListId(pub usize);

/// Kind of auxiliary data a node carries (`SmAuxDataType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AuxDataType {
    /// `SM_AD_NONE`.
    #[default]
    None,
    /// `SM_AD_BEZIER_CURVE` (`SmBezierSpan`).
    BezierCurve,
    /// `SM_AD_CURVE_DATA` (`SmBezierAux1d`).
    CurveData,
    /// `SM_AD_BEZIER_SURFACE` (`SmBezierPatch`).
    BezierSurface,
    /// `SM_AD_BEZIER_SURFACE_HEAD` (`SmBezierAux2d`).
    BezierSurfaceHead,
    /// `SM_AD_SURFACE_DATA` (unused upstream).
    SurfaceData,
    /// `SM_AD_AUX_DATA` (`SmBezierAux2d`).
    AuxData,
    /// `SM_AD_CUBEAUX_DATA` (unused upstream).
    CubeAuxData,
    /// `SM_AD_OBJECT_LIST` (`SmObjectList`).
    ObjectList,
}

impl AuxDataType {
    /// Upstream enum value (`SM_AD_NONE` = 0 .. `SM_AD_OBJECT_LIST` = 8).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::None => 0,
            Self::BezierCurve => 1,
            Self::CurveData => 2,
            Self::BezierSurface => 3,
            Self::BezierSurfaceHead => 4,
            Self::SurfaceData => 5,
            Self::AuxData => 6,
            Self::CubeAuxData => 7,
            Self::ObjectList => 8,
        }
    }

    /// The type with upstream enum value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => Self::None,
            1 => Self::BezierCurve,
            2 => Self::CurveData,
            3 => Self::BezierSurface,
            4 => Self::BezierSurfaceHead,
            5 => Self::SurfaceData,
            6 => Self::AuxData,
            7 => Self::CubeAuxData,
            8 => Self::ObjectList,
            _ => return None,
        })
    }

    /// True for the surface subdivision kinds (`SM_AD_BEZIER_SURFACE`,
    /// `SM_AD_BEZIER_SURFACE_HEAD`, `SM_AD_AUX_DATA`), whose data is a
    /// `SmBezierAux2d`.
    #[must_use]
    pub const fn is_surface_subdivision(self) -> bool {
        matches!(
            self,
            Self::BezierSurface | Self::BezierSurfaceHead | Self::AuxData
        )
    }
}

/// Geometry represented by a node (`SmNodeGeomType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NodeGeomType {
    /// `SM_NG_NONE`: initialization default.
    #[default]
    None,
    /// `SM_NG_DEFAULT`: every tree except the following.
    Default,
    /// `SM_NG_LINE_SEG`: curve and poly-edge tree nodes.
    LineSeg,
    /// `SM_NG_POLYGON`: surface and poly-face tree nodes.
    Polygon,
}

impl NodeGeomType {
    /// Upstream enum value (`SM_NG_NONE` = 0 .. `SM_NG_POLYGON` = 3).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Default => 1,
            Self::LineSeg => 2,
            Self::Polygon => 3,
        }
    }

    /// The type with upstream enum value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => Self::None,
            1 => Self::Default,
            2 => Self::LineSeg,
            3 => Self::Polygon,
            _ => return None,
        })
    }
}

/// Direction in which a subdivision node's children split its domain
/// (`SmBezierAux2d::m_eSplitDir`, an `SmSurfParamType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SplitDir {
    /// `SM_SP_U`: split in u into left (child 1) and right (child 2).
    U,
    /// `SM_SP_V`: split in v into bottom (child 1) and top (child 2).
    V,
    /// `SM_SP_UNKNOWN` (or any other `SmSurfParamType`): not split / unknown.
    #[default]
    Unknown,
}

/// Classification of a subdivision node against the trim boundary
/// (`SmNodeClassType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NodeClass {
    /// `SM_NC_UNKNOWN`.
    #[default]
    Unknown,
    /// `SM_NC_INSIDE`.
    Inside,
    /// `SM_NC_OUTSIDE`.
    Outside,
    /// `SM_NC_ON_BOUNDARY`.
    OnBoundary,
}

impl NodeClass {
    /// Upstream enum value (`SM_NC_UNKNOWN` = 0 .. `SM_NC_ON_BOUNDARY` = 3).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Unknown => 0,
            Self::Inside => 1,
            Self::Outside => 2,
            Self::OnBoundary => 3,
        }
    }
}

/// The surface subdivision data of a node read by this module (the
/// corresponding fields of upstream `SmBezierAux2d`; the surface cache units own
/// the rest).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SurfaceAux<O> {
    /// `m_eSplitDir`: how this node's children split its domain.
    pub split_dir: SplitDir,
    /// `m_bIsMarked`: traversal mark.
    pub is_marked: bool,
    /// `m_eNodeClass`: classification against the trim boundary.
    pub node_class: NodeClass,
    /// `m_pFace`: face of an inside node.
    pub face: Option<O>,
    /// `mBA_pSurface`: subdivided surface of this node's domain, if any.
    pub surface: Option<O>,
    /// Where the rest of the node's `SmBezierAux2d` lives (upstream `m_pData` points
    /// at it): the index of its `SmBezierPatch` in the owning surface cache's patch
    /// arena for [`AuxDataType::BezierSurface`] nodes, of its `SmBezierAux2d` in the
    /// cache's aux arena otherwise (see [`crate::surface::surface_cache::p1`]);
    /// `None` for subdivision data no cache holds.
    pub slot: Option<usize>,
}

/// Curve decomposition data a node points at: an entry of [`Tree::curve_spans`]
/// or of [`Tree::curve_parents`] (upstream `SmBezierSpan*` in the curve cache's
/// `m_sBezMgr`, `SmBezierAux1d*` in its `m_sParentMgr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveSpanRef {
    /// Leaf span `i` (`SmBezierSpan`, `SM_AD_BEZIER_CURVE`).
    Span(usize),
    /// Parent data `i` (`SmBezierAux1d`, `SM_AD_CURVE_DATA`).
    Parent(usize),
}

/// Auxiliary data of a node (`SmTreeNode::m_pData`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum NodeData<O> {
    /// Null data pointer.
    #[default]
    None,
    /// Head of the node's linked list of object-list items (`SmObjectList*`).
    ObjectList(Option<ListId>),
    /// Surface subdivision data (`SmBezierAux2d*` / `SmBezierPatch*`).
    Surface(SurfaceAux<O>),
    /// Curve decomposition data (`SmBezierSpan*` / `SmBezierAux1d*`).
    Curve(CurveSpanRef),
}

/// One item of a node's linked list of objects (`SmObjectList`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectList<O> {
    /// `m_pNext`: next item of the list.
    pub next: Option<ListId>,
    /// `m_sBBox`: the object's bounding box.
    pub bbox: Extent3d,
    /// `m_pObject`: the object (`None` = null pointer).
    pub object: Option<O>,
}

/// One node of a [`Tree`] (`SmTreeNode`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeNode<O> {
    /// `m_pParent`.
    pub parent: Option<NodeId>,
    /// `m_pChild1`: bottom or left child.
    pub child1: Option<NodeId>,
    /// `m_pChild2`: top or right child.
    pub child2: Option<NodeId>,
    /// `m_sBBox`: this node's bounding box.
    pub bbox: Extent3d,
    /// `m_eAuxDataType`: kind of [`TreeNode::data`].
    pub aux_type: AuxDataType,
    /// `m_pData`: auxiliary data.
    pub data: NodeData<O>,
    /// `m_eGeomType`: geometry type represented by the node.
    pub geom_type: NodeGeomType,
}

impl<O> Default for TreeNode<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O> TreeNode<O> {
    /// A detached node with an "inside out" box and no data (`SmTreeNode()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            parent: None,
            child1: None,
            child2: None,
            bbox: Extent3d::new(),
            aux_type: AuxDataType::None,
            data: NodeData::None,
            geom_type: NodeGeomType::None,
        }
    }

    /// A node as handed out by upstream's `SmMemBlockMgr` (zero-filled memory:
    /// all-zero box, no links, no data).
    const fn zeroed() -> Self {
        Self {
            parent: None,
            child1: None,
            child2: None,
            bbox: Extent3d::from_bounds(0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            aux_type: AuxDataType::None,
            data: NodeData::None,
            geom_type: NodeGeomType::None,
        }
    }

    /// This node's bounding box (`GetBoundingBox`).
    #[must_use]
    pub const fn bounding_box(&self) -> &Extent3d {
        &self.bbox
    }

    /// The head of this node's object list, read from [`TreeNode::data`]
    /// whatever [`TreeNode::aux_type`] says (upstream reads `m_pData` directly).
    #[must_use]
    pub const fn list_head(&self) -> Option<ListId> {
        match self.data {
            NodeData::ObjectList(h) => h,
            NodeData::None | NodeData::Surface(_) | NodeData::Curve(_) => None,
        }
    }
}

/// Kind of object that owns a tree (`SmTree::m_pOwner`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeOwner {
    /// An `SmCurveCache`.
    CurveCache,
    /// An `SmSurfaceCache`.
    SurfaceCache,
    /// An `SmTrimSrfCache` (an `SmSurfaceCache`).
    TrimSrfCache,
    /// An `SmTessSrfCache` (an `SmTrimSrfCache`).
    TessSrfCache,
    /// An `SmBrepCache`.
    BrepCache,
    /// Any other object.
    Other,
}

/// A binary tree of boxes (`SmTree`): an object spatial tree, a single point
/// tree, or the skeleton of a curve/surface subdivision tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Tree<O> {
    /// `m_lMaxElementsPerNode`: list length that triggers a leaf split.
    max_elements_per_node: u64,
    /// `m_dMinSizeRatio`: smallest node size as a fraction of the top node.
    min_size_ratio: f64,
    /// `m_dNodeReduction`: child box size reduction when splitting.
    node_reduction: f64,
    /// `m_pOwner`.
    owner: Option<TreeOwner>,
    /// `m_pTopNode`.
    top: Option<NodeId>,
    /// `m_sNodeMgr`: every node, in allocation order.
    nodes: Vec<TreeNode<O>>,
    /// `m_sListMgr`: every object-list item, in allocation order.
    lists: Vec<ObjectList<O>>,
    /// The owning curve cache's leaf spans (its `m_sBezMgr`), for curve trees.
    curve_spans: Vec<BezierSpan>,
    /// The owning curve cache's parent data (its `m_sParentMgr`), for curve trees.
    curve_parents: Vec<BezierAux1d>,
    /// The curve of an owning curve cache (`SmCurveCache::GetCurve`), as the
    /// caller's handle.
    owner_curve: Option<O>,
    /// The surface of an owning surface cache (`SmSurfaceCache::GetSurface`), as the
    /// caller's handle.
    owner_surface: Option<O>,
}

/// C `int` expression `1 << count` widened to `ULONG` (sign-extended, as on
/// LP64 targets).
fn int_bit(count: u64) -> u64 {
    i64::from(1i32.wrapping_shl(count as u32)) as u64
}

/// A side of a subdivision node's domain: the split direction that separates it
/// and whether it is the child-1 half (left for `U`, bottom for `V`). Used both
/// for which child of its parent a node is (the `bLeft`/`bRight`/`bBot`/`bTop`
/// flags of the neighbour searches) and for the direction of a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Side {
    /// `SplitDir::U` or `SplitDir::V`.
    axis: SplitDir,
    /// True for the child-1 half (left / bottom).
    first: bool,
}

impl Side {
    const LEFT: Self = Self::new(SplitDir::U, true);
    const RIGHT: Self = Self::new(SplitDir::U, false);
    const BOT: Self = Self::new(SplitDir::V, true);
    const TOP: Self = Self::new(SplitDir::V, false);

    const fn new(axis: SplitDir, first: bool) -> Self {
        Self { axis, first }
    }

    /// The opposite half on the same axis.
    const fn opposite(self) -> Self {
        Self {
            axis: self.axis,
            first: !self.first,
        }
    }

    /// The operation name used in errors of a search in this direction.
    const fn op(self) -> &'static str {
        match (self.axis, self.first) {
            (SplitDir::U, true) => "geom::tree::Tree::left_neighbors",
            (SplitDir::U, false) => "geom::tree::Tree::right_neighbors",
            (SplitDir::V, true) => "geom::tree::Tree::bot_neighbors",
            (SplitDir::V, false) => "geom::tree::Tree::top_neighbors",
            (SplitDir::Unknown, _) => "geom::tree::Tree::neighbors",
        }
    }
}

/// Error for parent/child links that loop back on themselves (upstream would
/// recurse or loop forever).
fn cyclic_links(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "tree node links are cyclic",
    }
}

impl<O> Tree<O> {
    /// The leaf spans of a curve tree, in span order (the owning `SmCurveCache`'s
    /// `m_sBezMgr`).
    #[must_use]
    pub fn curve_spans(&self) -> &[BezierSpan] {
        &self.curve_spans
    }

    /// The parent data of a curve tree (the owning `SmCurveCache`'s `m_sParentMgr`).
    #[must_use]
    pub fn curve_parents(&self) -> &[BezierAux1d] {
        &self.curve_parents
    }

    /// Hands the curve decomposition data of the owning curve cache to the tree
    /// (the arenas [`CurveSpanRef`] indexes) together with the cache's curve handle
    /// ([`Tree::owner_object`]).
    pub fn set_curve_data(
        &mut self,
        spans: Vec<BezierSpan>,
        parents: Vec<BezierAux1d>,
        owner_curve: Option<O>,
    ) {
        self.curve_spans = spans;
        self.curve_parents = parents;
        self.owner_curve = owner_curve;
    }

    /// This curve subdivision tree in another handle space, its owner curve named
    /// `owner_curve` (not upstream: one memoised curve cache serves callers of every
    /// handle space, see [`crate::curve::curve::p5::make_curve_cache`]). `None` when the
    /// tree names other objects (object lists, surface data or an owner surface), which
    /// a curve cache's tree does not.
    pub(crate) fn retag_curve_tree<P>(&self, owner_curve: Option<P>) -> Option<Tree<P>> {
        if self.owner_surface.is_some() || self.lists.iter().any(|l| l.object.is_some()) {
            return None;
        }
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let data = match &n.data {
                NodeData::None => NodeData::None,
                NodeData::ObjectList(l) => NodeData::ObjectList(*l),
                NodeData::Curve(c) => NodeData::Curve(*c),
                NodeData::Surface(_) => return None,
            };
            nodes.push(TreeNode {
                parent: n.parent,
                child1: n.child1,
                child2: n.child2,
                bbox: n.bbox,
                aux_type: n.aux_type,
                data,
                geom_type: n.geom_type,
            });
        }
        let lists = self
            .lists
            .iter()
            .map(|l| ObjectList {
                next: l.next,
                bbox: l.bbox,
                object: None,
            })
            .collect();
        Some(Tree {
            max_elements_per_node: self.max_elements_per_node,
            min_size_ratio: self.min_size_ratio,
            node_reduction: self.node_reduction,
            owner: self.owner,
            top: self.top,
            nodes,
            lists,
            curve_spans: self.curve_spans.clone(),
            curve_parents: self.curve_parents.clone(),
            owner_curve,
            owner_surface: None,
        })
    }

    /// Sets the handle of the owning surface cache's surface
    /// (`SmSurfaceCache::GetSurface`, reported by [`Tree::owner_object`] for surface
    /// cache trees).
    pub fn set_owner_surface(&mut self, owner_surface: Option<O>) {
        self.owner_surface = owner_surface;
    }

    /// The curve decomposition data of `node`: its `SmBezierAux1d` part (upstream
    /// casts `m_pData`, for a leaf an `SmBezierSpan`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the node carries no curve data or the index is out of
    /// range (upstream reads through whatever `m_pData` points at).
    pub fn curve_aux(&self, node: &TreeNode<O>) -> Result<&BezierAux1d> {
        let found = match node.data {
            NodeData::Curve(CurveSpanRef::Span(i)) => self.curve_spans.get(i).map(|s| &s.aux),
            NodeData::Curve(CurveSpanRef::Parent(i)) => self.curve_parents.get(i),
            NodeData::None | NodeData::ObjectList(_) | NodeData::Surface(_) => None,
        };
        found.ok_or(SolidError::InvalidInput {
            operation: "geom::tree::Tree::curve_aux",
            reason: "node has no curve decomposition data",
        })
    }

    /// The leaf span of `node` (upstream casts `m_pData` to `SmBezierSpan*`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the node does not point at a leaf span.
    pub fn curve_span(&self, node: &TreeNode<O>) -> Result<&BezierSpan> {
        let found = match node.data {
            NodeData::Curve(CurveSpanRef::Span(i)) => self.curve_spans.get(i),
            NodeData::Curve(CurveSpanRef::Parent(_))
            | NodeData::None
            | NodeData::ObjectList(_)
            | NodeData::Surface(_) => None,
        };
        found.ok_or(SolidError::InvalidInput {
            operation: "geom::tree::Tree::curve_span",
            reason: "node has no curve leaf span",
        })
    }
}

impl<O: Copy + PartialEq> Tree<O> {
    /// `SmTree_TYPE`.
    pub const TYPE: i32 = SM_TREE_TYPE;

    /// An empty tree (no nodes) owned by `owner` (`SmTree(SmObject*)`).
    #[must_use]
    pub const fn new(owner: Option<TreeOwner>) -> Self {
        Self {
            max_elements_per_node: 10,
            min_size_ratio: 100.0,
            node_reduction: 0.4545,
            owner,
            top: None,
            nodes: Vec::new(),
            lists: Vec::new(),
            curve_spans: Vec::new(),
            curve_parents: Vec::new(),
            owner_curve: None,
            owner_surface: None,
        }
    }

    /// A one-node tree whose box is `point` grown by `point_size` in every
    /// direction (`SmTree(const SmPoint3d&, double, ...)`). With
    /// `with_object_list` the node gets one object-list item with a null object
    /// and a point-sized box at `point` (upstream `pOptExistingObjectList`).
    #[must_use]
    pub fn from_point(point: Point3, point_size: f64, with_object_list: bool) -> Self {
        let mut tree = Self::new(None);
        let mut node = TreeNode::zeroed();
        node.bbox = Extent3d::from_point(point);
        node.bbox.expand_absolute(point_size);
        if with_object_list {
            tree.lists.push(ObjectList {
                next: None,
                bbox: Extent3d::from_point(point),
                object: None,
            });
            node.data = NodeData::ObjectList(Some(ListId(0)));
            node.aux_type = AuxDataType::ObjectList;
        }
        tree.nodes.push(node);
        tree.top = Some(NodeId(0));
        tree
    }

    /// A spatial tree with one empty object-list node whose box is
    /// `spatial_box` (`SmTree(const SmExtent3d&, ULONG, ULONG)`). The box should
    /// contain every object added later; [`Tree::add_to_spatial_tree`] rebuilds
    /// the tree (expensively) when it does not.
    #[must_use]
    pub fn new_spatial(spatial_box: &Extent3d) -> Self {
        let mut tree = Self::new(None);
        let mut node = TreeNode::zeroed();
        node.aux_type = AuxDataType::ObjectList;
        node.data = NodeData::ObjectList(None);
        node.bbox = *spatial_box;
        tree.nodes.push(node);
        tree.top = Some(NodeId(0));
        tree
    }

    /// `SmTree_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i32 {
        Self::TYPE
    }

    /// `"SmTree_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmTree_TYPE"
    }

    /// True for `SmTree_TYPE` and its base `SmObject_TYPE` (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i32) -> bool {
        // SM_COMMON_BASE ends the chain with SmObject::IsKindOf.
        t == SM_TREE_TYPE || object::is_kind_of(t as i64)
    }

    /// The owning cache, if any (`GetOwner`).
    #[must_use]
    pub const fn owner(&self) -> Option<TreeOwner> {
        self.owner
    }

    /// Sets the owner (`SetOwnerObject`).
    pub fn set_owner_object(&mut self, owner: Option<TreeOwner>) {
        self.owner = owner;
    }

    /// The curve or surface the tree was built for, when the owner is a curve
    /// or surface cache (`SmTree::GetOwnerObject`).
    pub fn owner_object(&self) -> Result<Option<O>> {
        match self.owner {
            Some(TreeOwner::CurveCache) => Ok(self.owner_curve),
            // SmSurfaceCache::GetSurface
            Some(TreeOwner::SurfaceCache | TreeOwner::TrimSrfCache | TreeOwner::TessSrfCache) => {
                Ok(self.owner_surface)
            }
            Some(TreeOwner::BrepCache | TreeOwner::Other) | None => Ok(None),
        }
    }

    /// [`Tree::owner_object`] seen from a node (`SmTreeNode::GetOwnerObject`).
    pub fn node_owner_object(&self, node: NodeId) -> Result<Option<O>> {
        self.node(node)?;
        self.owner_object()
    }

    /// The top node (`GetTopNode`); `None` for an empty tree.
    #[must_use]
    pub const fn top_node(&self) -> Option<NodeId> {
        self.top
    }

    /// Every node, in allocation order.
    #[must_use]
    pub fn nodes(&self) -> &[TreeNode<O>] {
        &self.nodes
    }

    /// Every object-list item, in allocation order.
    #[must_use]
    pub fn lists(&self) -> &[ObjectList<O>] {
        &self.lists
    }

    /// Copies this tree with object handles translated by `map`, preserving node
    /// and list ids, traversal order, boxes and decomposition data.
    ///
    /// This Rust adapter lets typed shape trees participate in a solver over a
    /// caller's common object-handle type. `map` must name the same target object
    /// every time it receives the same source handle.
    ///
    /// # Errors
    /// Any error returned by `map`.
    pub fn map_objects<P: Copy + PartialEq>(
        &self,
        map: impl Fn(O) -> Result<P>,
    ) -> Result<Tree<P>> {
        let mut lists = Vec::new();
        for item in &self.lists {
            lists.push(ObjectList {
                next: item.next,
                bbox: item.bbox,
                object: item.object.map(&map).transpose()?,
            });
        }
        let mut nodes = Vec::new();
        for node in &self.nodes {
            let data = match node.data {
                NodeData::None => NodeData::None,
                NodeData::ObjectList(head) => NodeData::ObjectList(head),
                NodeData::Curve(span) => NodeData::Curve(span),
                NodeData::Surface(aux) => NodeData::Surface(SurfaceAux {
                    split_dir: aux.split_dir,
                    is_marked: aux.is_marked,
                    node_class: aux.node_class,
                    face: aux.face.map(&map).transpose()?,
                    surface: aux.surface.map(&map).transpose()?,
                    slot: aux.slot,
                }),
            };
            nodes.push(TreeNode {
                parent: node.parent,
                child1: node.child1,
                child2: node.child2,
                bbox: node.bbox,
                aux_type: node.aux_type,
                data,
                geom_type: node.geom_type,
            });
        }
        Ok(Tree {
            max_elements_per_node: self.max_elements_per_node,
            min_size_ratio: self.min_size_ratio,
            node_reduction: self.node_reduction,
            owner: self.owner,
            top: self.top,
            nodes,
            lists,
            curve_spans: self.curve_spans.clone(),
            curve_parents: self.curve_parents.clone(),
            owner_curve: self.owner_curve.map(&map).transpose()?,
            owner_surface: self.owner_surface.map(map).transpose()?,
        })
    }

    /// The node `id`.
    pub fn node(&self, id: NodeId) -> Result<&TreeNode<O>> {
        self.nodes.get(id.0).ok_or(SolidError::InvalidInput {
            operation: "geom::tree::Tree::node",
            reason: "node id out of range",
        })
    }

    /// The node `id`, mutable.
    pub fn node_mut(&mut self, id: NodeId) -> Result<&mut TreeNode<O>> {
        self.nodes.get_mut(id.0).ok_or(SolidError::InvalidInput {
            operation: "geom::tree::Tree::node_mut",
            reason: "node id out of range",
        })
    }

    /// The object-list item `id`.
    pub fn list(&self, id: ListId) -> Result<&ObjectList<O>> {
        self.lists.get(id.0).ok_or(SolidError::InvalidInput {
            operation: "geom::tree::Tree::list",
            reason: "object list id out of range",
        })
    }

    /// The object-list item `id`, mutable.
    pub fn list_mut(&mut self, id: ListId) -> Result<&mut ObjectList<O>> {
        self.lists.get_mut(id.0).ok_or(SolidError::InvalidInput {
            operation: "geom::tree::Tree::list_mut",
            reason: "object list id out of range",
        })
    }

    /// Appends `node` to the node arena and returns its id (upstream
    /// `m_sNodeMgr.GetNewElement()` followed by field initialisation).
    pub fn add_node(&mut self, node: TreeNode<O>) -> NodeId {
        self.nodes.push(node);
        NodeId(self.nodes.len() - 1)
    }

    /// Makes `top` the top node (upstream assigns `m_pTopNode`).
    pub fn set_top_node(&mut self, top: Option<NodeId>) {
        self.top = top;
    }

    /// The top node id, or an error for an empty tree (upstream dereferences
    /// the null top node).
    fn top_id(&self, operation: &'static str) -> Result<NodeId> {
        self.top.ok_or(SolidError::InvalidInput {
            operation,
            reason: "tree has no top node",
        })
    }

    /// The subdivision data of `id` (upstream casts `m_pData` to
    /// `SmBezierAux2d*`).
    fn surface_aux(&self, id: NodeId, operation: &'static str) -> Result<&SurfaceAux<O>> {
        match &self.node(id)?.data {
            NodeData::Surface(a) => Ok(a),
            NodeData::None | NodeData::ObjectList(_) | NodeData::Curve(_) => {
                Err(SolidError::InvalidInput {
                    operation,
                    reason: "node has no surface subdivision data",
                })
            }
        }
    }

    /// Mutable [`Tree::surface_aux`].
    fn surface_aux_mut(
        &mut self,
        id: NodeId,
        operation: &'static str,
    ) -> Result<&mut SurfaceAux<O>> {
        match &mut self.node_mut(id)?.data {
            NodeData::Surface(a) => Ok(a),
            NodeData::None | NodeData::ObjectList(_) | NodeData::Curve(_) => {
                Err(SolidError::InvalidInput {
                    operation,
                    reason: "node has no surface subdivision data",
                })
            }
        }
    }

    /// The items of the linked list starting at `head`, in list order.
    fn walk_list(&self, head: Option<ListId>, operation: &'static str) -> Result<Vec<ListId>> {
        let mut out = Vec::new();
        let mut cur = head;
        while let Some(id) = cur {
            if out.len() >= self.lists.len() {
                return Err(SolidError::InvalidInput {
                    operation,
                    reason: "object list is cyclic",
                });
            }
            out.push(id);
            cur = self.list(id)?.next;
        }
        Ok(out)
    }

    /// Searches the list starting at `head` for `object`
    /// (`SmObjectList::FindObject`). Returns whether it was found and the item
    /// before the last one visited: when found, the item before the match
    /// (`None` when the match is `head`); when not found, the last item.
    pub fn find_object(&self, head: ListId, object: Option<O>) -> Result<(bool, Option<ListId>)> {
        let mut prev = None;
        let mut cur = Some(head);
        let mut steps = 0usize;
        while let Some(id) = cur {
            let item = self.list(id)?;
            if item.object == object {
                return Ok((true, prev));
            }
            steps += 1;
            if steps > self.lists.len() {
                return Err(SolidError::InvalidInput {
                    operation: "geom::tree::Tree::find_object",
                    reason: "object list is cyclic",
                });
            }
            prev = Some(id);
            cur = item.next;
        }
        Ok((false, prev))
    }

    /// Adds `object` with box `extent` to the spatial tree, splitting a leaf
    /// whose list grows past the element limit (`AddToSpatialTree`). When
    /// `extent` is not inside the top node's box, the whole tree is first
    /// rebuilt around a grown box (expensive). `object = None` only performs that
    /// size check. An object already stored in the node that would hold it gets
    /// its box enlarged instead.
    pub fn add_to_spatial_tree(
        &mut self,
        extent: &Extent3d,
        object: Option<O>,
        geom_type: NodeGeomType,
    ) -> Result<()> {
        self.add_to_spatial_tree_at(extent, object, geom_type, 0)
    }

    /// [`Tree::add_to_spatial_tree`] nested `depth` rebuilds deep.
    fn add_to_spatial_tree_at(
        &mut self,
        extent: &Extent3d,
        object: Option<O>,
        geom_type: NodeGeomType,
        depth: usize,
    ) -> Result<()> {
        const OP: &str = "geom::tree::Tree::add_to_spatial_tree";
        let top = self.top_id(OP)?;

        // rebuild the tree with a larger box when the extent sticks out
        if !extent.is_contained_by(&self.node(top)?.bbox, EFF_ZERO) {
            if depth >= MAX_SPATIAL_REBUILD_DEPTH {
                return Err(SolidError::invalid_input(
                    OP,
                    "re-inserted box outside the rebuilt top box",
                ));
            }
            let all = self.object_list()?;
            let mut union = *extent;
            let mut objects = Vec::with_capacity(all.len());
            let mut extents = Vec::with_capacity(all.len());
            for id in all {
                let item = self.list(id)?;
                objects.push(item.object);
                extents.push(item.bbox);
                union = union.union(&item.bbox);
            }

            let size = union.size();
            let tol = EFF_ZERO_SQ * size.max_dimension();
            union.expand_absolute(tol);
            union.add_point(union.max() + 0.15 * size);
            union.add_point(union.min() - 0.15 * size);

            self.lists.clear();
            self.nodes.clear();

            let mut node = TreeNode::zeroed();
            node.aux_type = AuxDataType::ObjectList;
            node.data = NodeData::ObjectList(None);
            node.bbox = union;
            node.geom_type = geom_type;
            self.nodes.push(node);
            self.top = Some(NodeId(0));

            for (bbox, obj) in extents.iter().zip(objects) {
                // upstream ignores the status of the re-insertions
                let _status = self.add_to_spatial_tree_at(bbox, obj, geom_type, depth + 1);
            }
        }

        let Some(object) = object else {
            return Ok(());
        };

        let found = self.find_node(extent)?.ok_or(SolidError::InvalidInput {
            operation: OP,
            reason: "no node contains the object box",
        })?;

        let head = self.node(found)?.list_head();
        let (contains, prev) = match head {
            Some(h) => self.find_object(h, Some(object))?,
            None => (false, None),
        };
        if contains {
            let this = match prev {
                None => head,
                Some(p) => self.list(p)?.next,
            };
            if let Some(t) = this {
                let item = self.list_mut(t)?;
                item.bbox = item.bbox.union(extent);
            }
            return Ok(());
        }

        // new list item at the front of the found node's list
        let new_id = ListId(self.lists.len());
        self.lists.push(ObjectList {
            next: head,
            bbox: *extent,
            object: Some(object),
        });
        self.node_mut(found)?.data = NodeData::ObjectList(Some(new_id));

        let count = self.walk_list(Some(new_id), OP)?.len() as u64;

        let found_node = *self.node(found)?;
        let split = if found_node.child1.is_some() || count < self.max_elements_per_node {
            false
        } else {
            let top_ext = self.node(self.top_id(OP)?)?.bbox;
            let curr_ext = found_node.bbox;
            let top_dist = top_ext.max().distance(top_ext.min());
            let curr_dist = curr_ext.max().distance(curr_ext.min());
            // keep upstream's comparison (NaN distances split)
            let too_small = top_dist > curr_dist * self.min_size_ratio;
            !too_small
        };

        if split {
            let curr_ext = found_node.bbox;
            let child1 = NodeId(self.nodes.len());
            let child2 = NodeId(self.nodes.len() + 1);
            for _ in 0..2 {
                let mut child = TreeNode::zeroed();
                child.parent = Some(found);
                child.aux_type = AuxDataType::ObjectList;
                child.data = NodeData::ObjectList(None);
                child.geom_type = geom_type;
                self.nodes.push(child);
            }
            {
                let f = self.node_mut(found)?;
                f.child1 = Some(child1);
                f.child2 = Some(child2);
            }

            let diff = curr_ext.max() - curr_ext.min();
            let mut new_min = curr_ext.min();
            let mut new_max = curr_ext.max();
            let red = self.node_reduction;
            if diff.x >= diff.y && diff.x >= diff.z {
                new_max.x -= diff.x * red;
                new_min.x += diff.x * red;
            } else if diff.y >= diff.x && diff.y >= diff.z {
                new_max.y -= diff.y * red;
                new_min.y += diff.y * red;
            } else {
                new_max.z -= diff.z * red;
                new_min.z += diff.z * red;
            }
            // upstream ignores SetMinMax's status (the box then stays zeroed)
            let _status = self
                .node_mut(child1)?
                .bbox
                .set_min_max(curr_ext.min(), new_max);
            let _status = self
                .node_mut(child2)?
                .bbox
                .set_min_max(new_min, curr_ext.max());

            // move the old leaf's objects to the smallest containing nodes
            let items = self.node_object_list(found)?;
            self.node_mut(found)?.data = NodeData::ObjectList(None);
            for id in items {
                self.list_mut(id)?.next = None;
                let bbox = self.list(id)?.bbox;
                let fnd = self.find_node(&bbox)?.ok_or(SolidError::InvalidInput {
                    operation: OP,
                    reason: "no node contains the object box",
                })?;
                let fnd_head = self.node(fnd)?.list_head();
                self.list_mut(id)?.next = fnd_head;
                self.node_mut(fnd)?.data = NodeData::ObjectList(Some(id));
            }
        }
        Ok(())
    }

    /// Removes `object` from the node holding `extent` or, failing that, from
    /// one of that node's ancestors (`RmFromSpatialTree`). Returns whether it
    /// was found and removed; `false` for a tree that is not an object tree.
    pub fn rm_from_spatial_tree(&mut self, extent: &Extent3d, object: Option<O>) -> Result<bool> {
        let Some(top) = self.top else {
            return Ok(false);
        };
        if self.node(top)?.aux_type != AuxDataType::ObjectList {
            return Ok(false);
        }
        let Some(found) = self.find_node(extent)? else {
            return Ok(false);
        };

        let mut cur = Some(found);
        let mut steps = 0usize;
        while let Some(id) = cur {
            steps += 1;
            if steps > self.nodes.len() {
                return Err(cyclic_links("geom::tree::Tree::rm_from_spatial_tree"));
            }
            let node = *self.node(id)?;
            let head = if node.aux_type == AuxDataType::ObjectList {
                node.list_head()
            } else {
                None
            };
            if let Some(h) = head {
                let (ok, prev) = self.find_object(h, object)?;
                if ok {
                    if let Some(p) = prev {
                        let skip = self.list(p)?.next;
                        let after = match skip {
                            Some(s) => self.list(s)?.next,
                            None => None,
                        };
                        self.list_mut(p)?.next = after;
                    } else {
                        let after = self.list(h)?.next;
                        self.node_mut(id)?.data = NodeData::ObjectList(after);
                    }
                    return Ok(true);
                }
            }
            cur = node.parent;
        }
        Ok(false)
    }

    /// Every node id in allocation order (`GetAllTreeNodes`).
    #[must_use]
    pub fn all_tree_nodes(&self) -> Vec<NodeId> {
        (0..self.nodes.len()).map(NodeId).collect()
    }

    /// The smallest node whose box contains `bbox` (within `EFF_ZERO`), leaf or
    /// internal (`FindNode`). A childless top node is returned unchecked;
    /// `None` when `bbox` does not fit the tree.
    pub fn find_node(&self, bbox: &Extent3d) -> Result<Option<NodeId>> {
        const OP: &str = "geom::tree::Tree::find_node";
        let mut id = self.top_id(OP)?;
        for _ in 0..=self.nodes.len() {
            let node = self.node(id)?;
            let Some(c1) = node.child1 else {
                return Ok(Some(id));
            };
            if bbox.is_contained_by(&self.node(c1)?.bbox, EFF_ZERO) {
                id = c1;
                continue;
            }
            let c2 = node.child2.ok_or(SolidError::InvalidInput {
                operation: OP,
                reason: "node has child 1 but no child 2",
            })?;
            if bbox.is_contained_by(&self.node(c2)?.bbox, EFF_ZERO) {
                id = c2;
                continue;
            }
            if !bbox.is_contained_by(&node.bbox, EFF_ZERO) {
                return Ok(None);
            }
            return Ok(Some(id));
        }
        Err(cyclic_links(OP))
    }

    /// The object-list items of `node`, in list order
    /// (`SmTreeNode::GetObjectList`); fails for a node that is not an object
    /// list node.
    pub fn node_object_list(&self, node: NodeId) -> Result<Vec<ListId>> {
        const OP: &str = "geom::tree::Tree::node_object_list";
        let n = self.node(node)?;
        if n.aux_type != AuxDataType::ObjectList {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "node is not an object list node",
            });
        }
        self.walk_list(n.list_head(), OP)
    }

    /// Every object-list item of every object-list node, nodes in allocation
    /// order, each node's items in list order (`SmTree::GetObjectList`).
    pub fn object_list(&self) -> Result<Vec<ListId>> {
        let mut out = Vec::new();
        for node in &self.nodes {
            if node.aux_type != AuxDataType::ObjectList {
                continue;
            }
            out.extend(self.walk_list(node.list_head(), "geom::tree::Tree::object_list")?);
        }
        Ok(out)
    }

    /// Objects whose boxes touch `bbox`, in depth-first order (child 2 before
    /// child 1) (`GetObjectsInBox`).
    pub fn objects_in_box(&self, bbox: &Extent3d) -> Result<Vec<Option<O>>> {
        const OP: &str = "geom::tree::Tree::objects_in_box";
        let mut out = Vec::new();
        let mut stack = vec![self.top_id(OP)?];
        while let Some(id) = stack.pop() {
            let node = self.node(id)?;
            if !node.bbox.are_disjoint(bbox, 0.0) {
                for item in self.walk_list(node.list_head(), OP)? {
                    let item = self.list(item)?;
                    if !item.bbox.are_disjoint(bbox, 0.0) {
                        out.push(item.object);
                    }
                }
                if let Some(c1) = node.child1 {
                    stack.push(c1);
                    stack.push(node.child2.ok_or(SolidError::InvalidInput {
                        operation: OP,
                        reason: "node has child 1 but no child 2",
                    })?);
                }
            }
        }
        Ok(out)
    }

    /// True when some leaf box touches `bbox` (`IntersectsBox`).
    pub fn intersects_box(&self, bbox: &Extent3d) -> Result<bool> {
        const OP: &str = "geom::tree::Tree::intersects_box";
        let mut stack = vec![self.top_id(OP)?];
        while let Some(id) = stack.pop() {
            let node = self.node(id)?;
            if !node.bbox.are_disjoint(bbox, 0.0) {
                match node.child1 {
                    None => return Ok(true),
                    Some(c1) => {
                        stack.push(c1);
                        stack.push(node.child2.ok_or(SolidError::InvalidInput {
                            operation: OP,
                            reason: "node has child 1 but no child 2",
                        })?);
                    }
                }
            }
        }
        Ok(false)
    }

    /// The Bezier patch describing `node`'s shape, falling back to ancestors
    /// (`GetFirstBezierPatch`). `None` for nodes that are not surface
    /// subdivision nodes or have no shape.
    ///
    /// The patch data lives in the owning surface cache, so the two reads of it are
    /// resolvers: `bezier(node)` is `SmBezierPatch::GetBezierPtr` of a
    /// `SM_AD_BEZIER_SURFACE` node (`None`: no construction Bezier, the walk goes
    /// on), `nurb(surface)` is `SmBSplineSurface::GetGwNurbPointer` of a node's
    /// subdivision surface (upstream casts unchecked).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for nodes without surface data or cyclic parent links; those
    /// of the resolvers.
    pub fn first_bezier_patch<P>(
        &self,
        node: NodeId,
        mut bezier: impl FnMut(NodeId) -> Result<Option<P>>,
        mut nurb: impl FnMut(&O) -> Result<Option<P>>,
    ) -> Result<Option<P>> {
        const OP: &str = "geom::tree::Tree::first_bezier_patch";
        // upstream recurses into the parent; a loop with a cycle guard here
        let mut cur = node;
        for _ in 0..=self.nodes.len() {
            let n = self.node(cur)?;
            // check the AuxData Object for type SmBezier Patch
            if n.aux_type == AuxDataType::BezierSurface {
                if let Some(b) = bezier(cur)? {
                    return Ok(Some(b));
                }
            }
            // check AuxData object for type SmBezierAux2d
            if !n.aux_type.is_surface_subdivision() {
                return Ok(None);
            }
            let owner_surface = self.owner_object()?;
            // (SM_ASSERT on SmSurface::IsKindOf: debug only)
            let aux = self.surface_aux(cur, OP)?;
            // if the AuxData has a temporary Subdivision Surface pointer - use it
            if let Some(surface) = &aux.surface {
                if n.parent.is_none() || owner_surface.as_ref() != Some(surface) {
                    return nurb(surface);
                }
            }
            // else try to get the parent's shape - skip the owner surface
            match n.parent {
                Some(p) if owner_surface != aux.surface => cur = p,
                _ => return Ok(None),
            }
        }
        Err(cyclic_links(OP))
    }

    /// After `node`'s mark or class changed, updates its ancestors
    /// (`PropagateToParents`): a parent is marked when both children are, on the
    /// boundary when a child is, inside when both are, outside when this child
    /// is outside and its sibling inside. Stops at the first unchanged parent.
    pub fn propagate_to_parents(&mut self, node: NodeId) -> Result<()> {
        const OP: &str = "geom::tree::Tree::propagate_to_parents";
        let mut cur = node;
        for _ in 0..=self.nodes.len() {
            let n = *self.node(cur)?;
            if !n.aux_type.is_surface_subdivision() {
                return Ok(());
            }
            let Some(parent) = n.parent else {
                return Ok(());
            };
            let p = *self.node(parent)?;
            let sibling = if p.child1 == Some(cur) {
                p.child2
            } else {
                p.child1
            }
            .ok_or(SolidError::InvalidInput {
                operation: OP,
                reason: "node has no sibling",
            })?;
            let this_aux = *self.surface_aux(cur, OP)?;
            let sib_aux = *self.surface_aux(sibling, OP)?;
            let parent_aux = self.surface_aux_mut(parent, OP)?;
            let mut changed = false;

            if this_aux.is_marked && sib_aux.is_marked && !parent_aux.is_marked {
                parent_aux.is_marked = true;
                changed = true;
            }

            if this_aux.node_class == NodeClass::OnBoundary {
                if parent_aux.node_class != NodeClass::OnBoundary {
                    parent_aux.node_class = NodeClass::OnBoundary;
                    changed = true;
                }
            } else if this_aux.node_class == NodeClass::Inside
                && sib_aux.node_class == NodeClass::Inside
            {
                if parent_aux.node_class != NodeClass::Inside {
                    parent_aux.node_class = NodeClass::Inside;
                    parent_aux.face = this_aux.face;
                    changed = true;
                }
            } else if this_aux.node_class == NodeClass::Outside
                && sib_aux.node_class == NodeClass::Inside
                && parent_aux.node_class != NodeClass::Outside
            {
                parent_aux.node_class = NodeClass::Outside;
                changed = true;
            }

            if !changed {
                return Ok(());
            }
            cur = parent;
        }
        Err(cyclic_links(OP))
    }

    /// The left, right, bottom and top leaf neighbours of `node`, in that order
    /// (`GetNeighbors`); empty for non-subdivision nodes.
    pub fn neighbors(&self, node: NodeId) -> Result<Vec<NodeId>> {
        let mut out = Vec::new();
        if !self.node(node)?.aux_type.is_surface_subdivision() {
            return Ok(out);
        }
        for toward in [Side::LEFT, Side::RIGHT, Side::BOT, Side::TOP] {
            self.collect_neighbors(node, toward, &mut out)?;
        }
        Ok(out)
    }

    /// The leaf nodes left of `node` (`GetLeftNeighbors`).
    pub fn left_neighbors(&self, node: NodeId) -> Result<Vec<NodeId>> {
        self.neighbors_toward(node, Side::LEFT)
    }

    /// The leaf nodes right of `node` (`GetRightNeighbors`).
    pub fn right_neighbors(&self, node: NodeId) -> Result<Vec<NodeId>> {
        self.neighbors_toward(node, Side::RIGHT)
    }

    /// The leaf nodes below `node` (`GetBotNeighbors`).
    pub fn bot_neighbors(&self, node: NodeId) -> Result<Vec<NodeId>> {
        self.neighbors_toward(node, Side::BOT)
    }

    /// The leaf nodes above `node` (`GetTopNeighbors`).
    pub fn top_neighbors(&self, node: NodeId) -> Result<Vec<NodeId>> {
        self.neighbors_toward(node, Side::TOP)
    }

    fn neighbors_toward(&self, node: NodeId, toward: Side) -> Result<Vec<NodeId>> {
        let mut out = Vec::new();
        self.collect_neighbors(node, toward, &mut out)?;
        Ok(out)
    }

    /// Which half of `parent` the node `node` is (per the parent's split
    /// direction); `None` when the split direction is unknown or `node` is not a
    /// child of `parent`.
    fn child_side(&self, node: NodeId, parent: NodeId, op: &'static str) -> Result<Option<Side>> {
        let p = self.node(parent)?;
        let split = self.surface_aux(parent, op)?.split_dir;
        let is1 = p.child1 == Some(node);
        let is2 = p.child2 == Some(node);
        Ok(match split {
            SplitDir::U | SplitDir::V if is1 => Some(Side::new(split, true)),
            SplitDir::U | SplitDir::V if is2 => Some(Side::new(split, false)),
            SplitDir::U | SplitDir::V | SplitDir::Unknown => None,
        })
    }

    /// Child 1 (`first`) or child 2 of `id`; error when missing (upstream
    /// dereferences null).
    fn child(&self, id: NodeId, first: bool, op: &'static str) -> Result<NodeId> {
        let n = self.node(id)?;
        if first { n.child1 } else { n.child2 }.ok_or(SolidError::InvalidInput {
            operation: op,
            reason: "missing child node",
        })
    }

    /// Appends the leaf neighbours of `node` on side `toward` (the shared body of
    /// `GetLeftNeighbors` .. `GetTopNeighbors`). Climbs while `node` is on the
    /// `toward` half of its parent, recording in `(bits, count)` which halves of
    /// the cross-direction splits it came from; at the first parent where it is
    /// on the opposite half, collects the matching leaves of the sibling's facing
    /// side.
    fn collect_neighbors(&self, node: NodeId, toward: Side, out: &mut Vec<NodeId>) -> Result<()> {
        let op = toward.op();
        let (mut cur, mut bits, mut count) = (node, 0u64, 0u64);
        for _ in 0..=self.nodes.len() {
            let n = self.node(cur)?;
            if !n.aux_type.is_surface_subdivision() {
                return Ok(());
            }
            if count == 0 {
                bits = 0;
            }
            let Some(parent) = n.parent else {
                return Ok(());
            };
            let Some(side) = self.child_side(cur, parent, op)? else {
                return Ok(());
            };
            if side.axis == toward.axis {
                if side.first != toward.first {
                    let sibling = self.child(parent, toward.first, op)?;
                    let search = SideSearch {
                        target_bits: bits,
                        target_count: count,
                        facing: toward.opposite(),
                        op,
                    };
                    return self.side(sibling, out, &search, 0, 0, 0);
                }
            } else {
                if !side.first {
                    bits |= int_bit(count);
                }
                count += 1;
            }
            cur = parent;
        }
        Err(cyclic_links(op))
    }

    /// Whether the descendants `(bits << 1) | 0` and `(bits << 1) | 1` one split
    /// below `(bits, count)` line up with the target `(target_bits,
    /// target_count)` (the `bChild1`/`bChild2` tests of the `Get*Side` functions).
    fn split_targets(target_bits: u64, target_count: u64, bits: u64, count: u64) -> (bool, bool) {
        if count + 1 > target_count {
            return (true, true);
        }
        let want = target_bits.wrapping_shr((target_count - (count + 1)) as u32);
        ((bits << 1) == want, ((bits << 1) | 1) == want)
    }

    /// The `Get*Side` functions: appends the leaves of `node` on side
    /// `search.facing` whose cross-direction position matches the target bits.
    /// Splits along the facing axis pass straight to the facing child; cross
    /// splits descend into the children that line up with the target.
    fn side(
        &self,
        node: NodeId,
        out: &mut Vec<NodeId>,
        search: &SideSearch,
        bits: u64,
        count: u64,
        depth: usize,
    ) -> Result<()> {
        enum Frame {
            Visit(NodeId, u64, u64, usize),
            // Resolve child 2 only after child 1 finishes, preserving error timing.
            SecondChild(NodeId, u64, u64, usize),
        }

        let mut stack = vec![Frame::Visit(node, bits, count, depth)];
        while let Some(frame) = stack.pop() {
            let (node, mut bits, count, depth) = match frame {
                Frame::Visit(node, bits, count, depth) => (node, bits, count, depth),
                Frame::SecondChild(node, bits, count, depth) => {
                    let c2 = self.child(node, false, search.op)?;
                    (c2, (bits << 1) | 1, count + 1, depth + 1)
                }
            };
            if depth > self.nodes.len() {
                return Err(cyclic_links(search.op));
            }
            let n = self.node(node)?;
            if !n.aux_type.is_surface_subdivision() {
                continue;
            }
            if count == 0 {
                bits = 0;
            }
            let split = self.surface_aux(node, search.op)?.split_dir;
            let facing = search.facing.axis;
            let cross = match facing {
                SplitDir::U => SplitDir::V,
                SplitDir::V => SplitDir::U,
                SplitDir::Unknown => SplitDir::Unknown,
            };
            if n.child1.is_none() {
                out.push(node);
            } else if split == facing {
                let c = self.child(node, search.facing.first, search.op)?;
                stack.push(Frame::Visit(c, bits, count, depth + 1));
            } else if split == cross {
                let (b1, b2) =
                    Self::split_targets(search.target_bits, search.target_count, bits, count);
                if b2 {
                    stack.push(Frame::SecondChild(node, bits, count, depth));
                }
                if b1 {
                    let c1 = self.child(node, true, search.op)?;
                    stack.push(Frame::Visit(c1, bits << 1, count + 1, depth + 1));
                }
            }
        }
        Ok(())
    }
}

/// The fixed arguments of one `Get*Side` descent.
struct SideSearch {
    /// `lSplitBits` of the neighbour search.
    target_bits: u64,
    /// `lSplitCount` of the neighbour search.
    target_count: u64,
    /// The side of the sibling that faces the searching node.
    facing: Side,
    /// Operation name for errors.
    op: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec3;

    fn ext(a: [f64; 3], b: [f64; 3]) -> Extent3d {
        Extent3d::from_points(Vec3::new(a[0], a[1], a[2]), Vec3::new(b[0], b[1], b[2]))
    }

    fn unit_box() -> Extent3d {
        ext([0.0; 3], [1.0; 3])
    }

    #[test]
    fn spatial_insert_and_query() {
        let mut t: Tree<u32> = Tree::new_spatial(&ext([0.0; 3], [10.0; 3]));
        for i in 0..30u32 {
            let x = f64::from(i) * 0.3;
            t.add_to_spatial_tree(
                &ext([x, 0.0, 0.0], [x + 0.1, 0.1, 0.1]),
                Some(i),
                NodeGeomType::Default,
            )
            .unwrap();
        }
        assert!(t.nodes().len() > 1, "a leaf with 10+ objects is split");
        assert_eq!(t.object_list().unwrap().len(), 30);
        let hits = t.objects_in_box(&ext([0.0; 3], [0.35, 1.0, 1.0])).unwrap();
        let mut ids: Vec<u32> = hits.into_iter().flatten().collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![0, 1]);
        assert!(t.intersects_box(&unit_box()).unwrap());
        assert!(!t.intersects_box(&ext([20.0; 3], [21.0; 3])).unwrap());
    }

    #[test]
    fn re_adding_grows_box_and_remove() {
        let mut t: Tree<u32> = Tree::new_spatial(&ext([0.0; 3], [10.0; 3]));
        t.add_to_spatial_tree(&unit_box(), Some(7), NodeGeomType::Default)
            .unwrap();
        t.add_to_spatial_tree(&ext([0.5; 3], [2.0; 3]), Some(7), NodeGeomType::Default)
            .unwrap();
        assert_eq!(t.lists().len(), 1);
        assert_eq!(t.lists()[0].bbox, ext([0.0; 3], [2.0; 3]));
        assert!(t.rm_from_spatial_tree(&unit_box(), Some(7)).unwrap());
        assert!(!t.rm_from_spatial_tree(&unit_box(), Some(7)).unwrap());
        assert!(t.object_list().unwrap().is_empty());
    }

    #[test]
    fn outside_box_rebuilds() {
        let mut t: Tree<u32> = Tree::new_spatial(&unit_box());
        t.add_to_spatial_tree(&unit_box(), Some(1), NodeGeomType::LineSeg)
            .unwrap();
        t.add_to_spatial_tree(&ext([5.0; 3], [6.0; 3]), Some(2), NodeGeomType::LineSeg)
            .unwrap();
        let top = t.node(t.top_node().unwrap()).unwrap();
        assert_eq!(top.geom_type, NodeGeomType::LineSeg);
        assert!(ext([5.0; 3], [6.0; 3]).is_contained_by(&top.bbox, 0.0));
        assert_eq!(t.object_list().unwrap().len(), 2);
        // extent-only check
        t.add_to_spatial_tree(&ext([-9.0; 3], [0.0; 3]), None, NodeGeomType::Default)
            .unwrap();
        assert_eq!(t.object_list().unwrap().len(), 2);
    }

    #[test]
    fn non_finite_boxes_rebuild_at_most_once() {
        let mut t: Tree<u32> = Tree::new_spatial(&unit_box());
        t.add_to_spatial_tree(&unit_box(), Some(1), NodeGeomType::Default)
            .unwrap();
        let nan = ext([f64::NAN, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let _ = t.add_to_spatial_tree(&nan, Some(2), NodeGeomType::Default);
        let inf = ext([f64::NEG_INFINITY; 3], [f64::INFINITY; 3]);
        let _ = t.add_to_spatial_tree(&inf, Some(3), NodeGeomType::Default);
        assert!(t.object_list().is_ok());
    }

    #[test]
    fn empty_tree_errors_and_point_tree() {
        let t: Tree<u32> = Tree::new(None);
        assert!(t.find_node(&unit_box()).is_err());
        assert!(t.objects_in_box(&unit_box()).is_err());
        let mut t2 = t.clone();
        assert!(!t2.rm_from_spatial_tree(&unit_box(), Some(1)).unwrap());
        let p: Tree<u32> = Tree::from_point(Vec3::new(1.0, 2.0, 3.0), 0.5, true);
        assert_eq!(p.nodes()[0].bbox, ext([0.5, 1.5, 2.5], [1.5, 2.5, 3.5]));
        assert_eq!(
            p.objects_in_box(&ext([0.0; 3], [5.0; 3])).unwrap(),
            vec![None]
        );
        assert_eq!(p.type_string(), "SmTree_TYPE");
        assert!(p.is_kind_of(Tree::<u32>::TYPE) && !p.is_kind_of(1));
    }

    fn surf(split: SplitDir) -> NodeData<u32> {
        NodeData::Surface(SurfaceAux {
            split_dir: split,
            ..SurfaceAux::default()
        })
    }

    /// 2x2 grid: root split U into left (L) / right (R), both split V.
    fn grid() -> (Tree<u32>, [NodeId; 7]) {
        let mut t: Tree<u32> = Tree::new(None);
        let mk = |t: &mut Tree<u32>, parent: Option<NodeId>, split| {
            t.add_node(TreeNode {
                parent,
                aux_type: AuxDataType::AuxData,
                data: surf(split),
                ..TreeNode::new()
            })
        };
        let root = mk(&mut t, None, SplitDir::U);
        let l = mk(&mut t, Some(root), SplitDir::V);
        let r = mk(&mut t, Some(root), SplitDir::V);
        let lb = mk(&mut t, Some(l), SplitDir::Unknown);
        let lt = mk(&mut t, Some(l), SplitDir::Unknown);
        let rb = mk(&mut t, Some(r), SplitDir::Unknown);
        let rt = mk(&mut t, Some(r), SplitDir::Unknown);
        t.node_mut(root).unwrap().child1 = Some(l);
        t.node_mut(root).unwrap().child2 = Some(r);
        t.node_mut(l).unwrap().child1 = Some(lb);
        t.node_mut(l).unwrap().child2 = Some(lt);
        t.node_mut(r).unwrap().child1 = Some(rb);
        t.node_mut(r).unwrap().child2 = Some(rt);
        t.set_top_node(Some(root));
        (t, [root, l, r, lb, lt, rb, rt])
    }

    #[test]
    fn mapped_surface_tree_preserves_neighbors_and_classification() {
        let (mut tree, [root, _, _, leaf, _, _, _]) = grid();
        tree.set_owner_surface(Some(7));
        if let NodeData::Surface(aux) = &mut tree.node_mut(leaf).unwrap().data {
            aux.face = Some(3);
            aux.surface = Some(7);
            aux.node_class = NodeClass::Inside;
            aux.is_marked = true;
        }
        let mapped = tree
            .map_objects(|object| Ok(u64::from(object) + 1000))
            .unwrap();
        assert_eq!(mapped.top, Some(root));
        assert_eq!(mapped.owner_surface, Some(1007));
        assert_eq!(
            mapped.neighbors(leaf).unwrap(),
            tree.neighbors(leaf).unwrap()
        );
        let NodeData::Surface(aux) = mapped.node(leaf).unwrap().data else {
            panic!("mapped surface lost its classification data");
        };
        assert_eq!(aux.face, Some(1003));
        assert_eq!(aux.surface, Some(1007));
        assert_eq!(aux.node_class, NodeClass::Inside);
        assert!(aux.is_marked);
        assert!(tree
            .map_objects::<u64>(|_| Err(SolidError::sm_err("test resolver")))
            .is_err());
        assert_eq!(tree.owner_surface, Some(7));
    }

    #[test]
    fn grid_neighbors() {
        let (t, [_, _, _, lb, lt, rb, rt]) = grid();
        assert_eq!(t.right_neighbors(lb).unwrap(), vec![rb]);
        assert_eq!(t.left_neighbors(rt).unwrap(), vec![lt]);
        assert_eq!(t.top_neighbors(lb).unwrap(), vec![lt]);
        assert_eq!(t.bot_neighbors(rt).unwrap(), vec![rb]);
        assert_eq!(t.neighbors(lt).unwrap(), vec![rt, lb]);
    }

    #[test]
    fn cyclic_links_are_errors() {
        // a subdivision node that is its own parent and child
        let (mut t, [_, l, ..]) = grid();
        let a = t.node_mut(l).unwrap();
        a.parent = Some(l);
        a.child1 = Some(l);
        assert!(matches!(
            t.left_neighbors(l),
            Err(SolidError::InvalidInput { .. })
        ));
        // a spatial tree whose top node is its own child
        let mut s: Tree<u32> = Tree::new_spatial(&ext([0.0; 3], [4.0; 3]));
        let top = s.node_mut(NodeId(0)).unwrap();
        top.child1 = Some(NodeId(0));
        top.child2 = Some(NodeId(0));
        assert!(matches!(
            s.find_node(&unit_box()),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    fn surface_leaf(t: &mut Tree<u32>, parent: Option<NodeId>) -> NodeId {
        t.add_node(TreeNode {
            parent,
            aux_type: AuxDataType::AuxData,
            data: surf(SplitDir::Unknown),
            ..TreeNode::new()
        })
    }

    fn split_surface_leaf(t: &mut Tree<u32>, node: NodeId, split: SplitDir) -> [NodeId; 2] {
        let c1 = surface_leaf(t, Some(node));
        let c2 = surface_leaf(t, Some(node));
        let n = t.node_mut(node).unwrap();
        n.data = surf(split);
        n.child1 = Some(c1);
        n.child2 = Some(c2);
        [c1, c2]
    }

    #[test]
    fn right_neighbors_deep_u_split_sibling_chain() {
        let mut t = Tree::new(None);
        let root = surface_leaf(&mut t, None);
        t.set_top_node(Some(root));
        let [left, mut facing] = split_surface_leaf(&mut t, root, SplitDir::U);
        // Linear-time public-API construction: append two arena nodes and update
        // just their parent per level, with no ancestor walks or tree cloning.
        for _ in 0..20_000 {
            [facing, _] = split_surface_leaf(&mut t, facing, SplitDir::U);
        }
        assert_eq!(t.right_neighbors(left).unwrap(), vec![facing]);
    }

    #[test]
    fn mixed_split_neighbors_preserve_depth_first_order() {
        let mut t = Tree::new(None);
        let root = surface_leaf(&mut t, None);
        t.set_top_node(Some(root));
        let [left, right] = split_surface_leaf(&mut t, root, SplitDir::U);
        let [lb, lt] = split_surface_leaf(&mut t, left, SplitDir::V);
        let [rb, rt] = split_surface_leaf(&mut t, right, SplitDir::V);
        let [rb_facing, _] = split_surface_leaf(&mut t, rb, SplitDir::U);
        let [rbb, rbt] = split_surface_leaf(&mut t, rb_facing, SplitDir::V);
        let [rbt_facing, _] = split_surface_leaf(&mut t, rbt, SplitDir::U);
        let [rtb, rtt] = split_surface_leaf(&mut t, rt, SplitDir::V);
        assert_eq!(
            t.right_neighbors(left).unwrap(),
            vec![rbb, rbt_facing, rtb, rtt]
        );
        // The left V split supplies nonzero target_count and both target bits.
        assert_eq!(t.right_neighbors(lb).unwrap(), vec![rbb, rbt_facing]);
        assert_eq!(t.right_neighbors(lt).unwrap(), vec![rtb, rtt]);
    }

    #[test]
    fn downward_side_cycles_preserve_error_timing() {
        for split in [SplitDir::U, SplitDir::V] {
            let (mut t, [_, left, right, ..]) = grid();
            let n = t.node_mut(right).unwrap();
            n.data = surf(split);
            n.child1 = Some(right);
            // For a cross split, child 2 must not be resolved before the cyclic
            // child-1 descent returns its error.
            n.child2 = None;
            assert!(matches!(
                t.right_neighbors(left),
                Err(SolidError::InvalidInput {
                    operation: "geom::tree::Tree::right_neighbors",
                    reason: "tree node links are cyclic",
                })
            ));
        }
    }

    #[test]
    fn propagate_marks_and_classes() {
        let (mut t, [root, l, _, lb, lt, _, _]) = grid();
        for (id, face) in [(lb, 3), (lt, 3)] {
            if let NodeData::Surface(a) = &mut t.node_mut(id).unwrap().data {
                a.is_marked = true;
                a.node_class = NodeClass::Inside;
                a.face = Some(face);
            }
        }
        t.propagate_to_parents(lb).unwrap();
        let la = t.surface_aux(l, "t").unwrap();
        assert!(la.is_marked);
        assert_eq!(la.node_class, NodeClass::Inside);
        assert_eq!(la.face, Some(3));
        let ra = t.surface_aux(root, "t").unwrap();
        assert!(!ra.is_marked);
        assert_eq!(ra.node_class, NodeClass::Unknown);
    }

    #[test]
    fn bezier_patch_and_owner() {
        let (mut t, [root, ..]) = grid();
        let none = |_: NodeId| -> Result<Option<i32>> { Ok(None) };
        let nurb = |s: &u32| -> Result<Option<i32>> { Ok(Some(100 + *s as i32)) };
        assert_eq!(t.first_bezier_patch(root, none, nurb).unwrap(), None);
        t.set_owner_object(Some(TreeOwner::SurfaceCache));
        assert_eq!(t.owner_object().unwrap(), None);
        t.set_owner_surface(Some(5));
        assert_eq!(t.owner_object().unwrap(), Some(5));
        assert_eq!(t.first_bezier_patch(root, none, nurb).unwrap(), None);
        t.node_mut(root).unwrap().aux_type = AuxDataType::BezierSurface;
        let patch = |n: NodeId| -> Result<Option<i32>> { Ok(Some(n.0 as i32)) };
        assert_eq!(
            t.first_bezier_patch(root, patch, nurb).unwrap(),
            Some(root.0 as i32)
        );
        t.set_owner_object(Some(TreeOwner::BrepCache));
        assert_eq!(t.owner_object().unwrap(), None);
    }
}
