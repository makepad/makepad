use super::BvhOptimizationHeapEntry;
use crate::bounding_volume::{Aabb, BoundingVolume};
use crate::math::{Real, Vector};
use crate::query::{Ray, RayCast};
use crate::utils::VecMap;
use alloc::collections::{BinaryHeap, VecDeque};
use alloc::vec::Vec;
use core::ops::{Deref, DerefMut, Index, IndexMut};

/// The strategy for one-time build of the BVH tree.
///
/// This enum controls which algorithm is used when constructing a BVH from scratch. Different
/// strategies offer different trade-offs between construction speed and final tree quality
/// (measured by ray-casting performance and other query efficiency).
///
/// # Strategy Comparison
///
/// - **Binned**: Fast construction with good overall quality. Best for general-purpose use.
/// - **PLOC**: Slower construction but produces higher quality trees. Best when ray-casting
///   performance is critical and construction time is less important.
///
/// # Performance Notes
///
/// - Neither strategy is currently parallelized, though PLOC is designed to support parallelization.
/// - Tree quality affects query performance: better trees mean fewer node visits during traversals.
/// - For dynamic scenes with frequent updates, choose based on initial construction performance.
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// // Create some AABBs for objects in the scene
/// let aabbs = vec![
///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
///     Aabb::new(Vector::new(0.0, 2.0, 0.0), Vector::new(1.0, 3.0, 1.0)),
/// ];
///
/// // Use binned strategy for general purpose (default)
/// let bvh_binned = Bvh::from_leaves(BvhBuildStrategy::Binned, &aabbs);
/// assert_eq!(bvh_binned.leaf_count(), 3);
///
/// // Use PLOC strategy for ray-casting heavy applications
/// let bvh_ploc = Bvh::from_leaves(BvhBuildStrategy::Ploc, &aabbs);
/// assert_eq!(bvh_ploc.leaf_count(), 3);
/// # }
/// ```
///
/// # See Also
///
/// - [`Bvh::from_leaves`] - Construct a BVH using a specific strategy
/// - [`Bvh::from_iter`] - Construct a BVH from an iterator
#[derive(Default, Clone, Debug, Copy, PartialEq, Eq)]
pub enum BvhBuildStrategy {
    /// The tree is built using the binned strategy.
    ///
    /// This implements the strategy from "On fast Construction of SAH-based Bounding Volume Hierarchies"
    /// by Ingo Wald. It uses binning to quickly approximate the Surface Area Heuristic (SAH) cost
    /// function, resulting in fast construction times with good tree quality.
    ///
    /// **Recommended for**: General-purpose usage, dynamic scenes, initial prototyping.
    #[default]
    Binned,
    /// The tree is built using the Locally-Ordered Clustering technique.
    ///
    /// This implements the strategy from "Parallel Locally-Ordered Clustering for Bounding Volume
    /// Hierarchy Construction" by Meister and Bittner. It produces higher quality trees at the cost
    /// of slower construction. The algorithm is designed for parallelization but the current
    /// implementation is sequential.
    ///
    /// **Recommended for**: Ray-casting heavy workloads, static scenes, when query performance
    /// is more important than construction time.
    Ploc,
}

/// Workspace data for various operations on the BVH tree.
///
/// This structure holds temporary buffers and working memory used during BVH operations
/// such as refitting, rebuilding, and optimization. The data inside can be freed at any time
/// without affecting the correctness of BVH results.
///
/// # Purpose
///
/// Many BVH operations require temporary allocations for intermediate results. By reusing
/// the same `BvhWorkspace` across multiple operations, you can significantly reduce allocation
/// overhead and improve performance, especially in performance-critical loops.
///
/// # Usage Pattern
///
/// 1. Create a workspace once (or use [`Default::default()`])
/// 2. Pass it to BVH operations that accept a workspace parameter
/// 3. Reuse the same workspace for subsequent operations
/// 4. The workspace grows to accommodate the largest operation size
///
/// # Memory Management
///
/// - The workspace grows as needed but never automatically shrinks
/// - You can drop and recreate the workspace to free memory
/// - All data is private and managed internally by the BVH
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhBuildStrategy, BvhWorkspace};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// let aabbs = vec![
///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
/// ];
///
/// let mut bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
/// let mut workspace = BvhWorkspace::default();
///
/// // Refit the tree after leaf movements
/// bvh.refit(&mut workspace);
///
/// // Reuse the same workspace for optimization
/// bvh.optimize_incremental(&mut workspace);
///
/// // The workspace can be reused across multiple BVH operations
/// # }
/// ```
///
/// # See Also
///
/// - [`Bvh::refit`] - Update AABBs after leaf movement
/// - [`Bvh::optimize_incremental`](Bvh::optimize_incremental) - Incremental tree optimization
#[derive(Clone, Default)]
pub struct BvhWorkspace {
    pub(super) refit_tmp: BvhNodeVec,
    pub(super) rebuild_leaves: Vec<BvhNode>,
    pub(super) rebuild_frame_index: u32,
    pub(super) rebuild_start_index: u32,
    pub(super) optimization_roots: Vec<u32>,
    pub(super) queue: BinaryHeap<BvhOptimizationHeapEntry>,
    pub(super) dequeue: VecDeque<u32>,
    pub(super) traversal_stack: Vec<u32>,
}

/// A piece of data packing state flags as well as leaf counts for a BVH tree node.
#[derive(Default, Copy, Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(transparent)]
pub struct BvhNodeData(u32);
const CHANGED: u32 = 0b01;
const CHANGE_PENDING: u32 = 0b11;

impl BvhNodeData {
    #[inline(always)]
    pub(super) fn with_leaf_count_and_pending_change(leaf_count: u32) -> Self {
        Self(leaf_count | (CHANGE_PENDING << 30))
    }

    #[inline(always)]
    pub(super) fn leaf_count(self) -> u32 {
        self.0 & 0x3fff_ffff
    }

    #[inline(always)]
    pub(super) fn is_changed(self) -> bool {
        self.0 >> 30 == CHANGED
    }

    #[inline(always)]
    pub(super) fn is_change_pending(self) -> bool {
        self.0 >> 30 == CHANGE_PENDING
    }

    #[inline(always)]
    pub(super) fn add_leaf_count(&mut self, added: u32) {
        self.0 += added;
    }

    #[inline(always)]
    pub(super) fn set_change_pending(&mut self) {
        self.0 |= CHANGE_PENDING << 30;
    }

    #[inline(always)]
    pub(super) fn resolve_pending_change(&mut self) {
        if self.is_change_pending() {
            *self = Self((self.0 & 0x3fff_ffff) | (CHANGED << 30));
        } else {
            *self = Self(self.0 & 0x3fff_ffff);
        }
    }

    pub(super) fn merged(self, other: Self) -> Self {
        let leaf_count = self.leaf_count() + other.leaf_count();
        let changed = (self.0 >> 30) | (other.0 >> 30);
        Self(leaf_count | changed << 30)
    }
}

/// A pair of tree nodes forming a 2-wide BVH node.
///
/// The BVH uses a memory layout where nodes are stored in pairs (left and right children)
/// to improve cache coherency and enable SIMD optimizations. This structure represents
/// a single entry in the BVH's node array.
///
/// # Node Validity
///
/// Both `left` and `right` are guaranteed to be valid except for one special case:
/// - **Single leaf tree**: Only `left` is valid, `right` is zeroed
/// - **All other cases**: Both `left` and `right` are valid (tree has at least 2 leaves)
///
/// # Memory Layout
///
/// In 3D with f32 precision and SIMD enabled, this structure is:
/// - **Size**: 64 bytes (cache line aligned)
/// - **Alignment**: 64 bytes (matches typical CPU cache lines)
/// - This alignment improves performance by reducing cache misses
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// let aabbs = vec![
///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
///     Aabb::new(Vector::new(4.0, 0.0, 0.0), Vector::new(5.0, 1.0, 1.0)),
/// ];
///
/// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
///
/// // Access the root node's children
/// // The BVH stores nodes as BvhNodeWide pairs internally
/// assert_eq!(bvh.leaf_count(), 3);
/// # }
/// ```
///
/// # See Also
///
/// - [`BvhNode`] - Individual node in the pair
/// - [`Bvh`] - The main BVH structure
#[derive(Copy, Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
// PERF: the size of this struct is 64 bytes but has a default alignment of 16 (in f32 + 3d + simd mode).
//       Forcing an alignment of 64 won’t add padding, and makes aligns it with most cache lines.
#[cfg_attr(all(feature = "dim3", feature = "f32"), repr(align(64)))]
pub struct BvhNodeWide {
    pub(super) left: BvhNode,
    pub(super) right: BvhNode,
}

// NOTE: if this assertion fails with a weird "0 - 1 would overflow" error, it means the equality doesn’t hold.
#[cfg(all(feature = "dim3", feature = "f32"))]
static_assertions::const_assert_eq!(align_of::<BvhNodeWide>(), 64);
#[cfg(all(feature = "dim3", feature = "f32"))]
static_assertions::assert_eq_size!(BvhNodeWide, [u8; 64]);

impl BvhNodeWide {
    /// Creates a new `BvhNodeWide` with both children zeroed out.
    ///
    /// This is primarily used internally during BVH construction and should rarely
    /// be needed in user code.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNodeWide;
    ///
    /// let node_wide = BvhNodeWide::zeros();
    /// assert_eq!(node_wide.leaf_count(), 0);
    /// # }
    /// ```
    #[inline(always)]
    pub fn zeros() -> Self {
        Self {
            left: BvhNode::zeros(),
            right: BvhNode::zeros(),
        }
    }

    /// Returns the two nodes as an array of references.
    ///
    /// This is useful for accessing the left or right node by index (0 or 1 respectively)
    /// instead of by name. Index 0 is the left node, index 1 is the right node.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::{Aabb, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    /// // The root AABB should contain both leaves
    /// assert!(bvh.root_aabb().contains(&aabbs[0]));
    /// assert!(bvh.root_aabb().contains(&aabbs[1]));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`as_array_mut`](Self::as_array_mut) - Mutable version
    #[inline(always)]
    pub fn as_array(&self) -> [&BvhNode; 2] {
        [&self.left, &self.right]
    }

    /// Returns the two nodes as an array of mutable references.
    ///
    /// This is useful for modifying the left or right node by index (0 or 1 respectively)
    /// instead of by name. Index 0 is the left node, index 1 is the right node.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNodeWide;
    /// use parry3d::math::Vector;
    ///
    /// let mut node_wide = BvhNodeWide::zeros();
    /// let nodes = node_wide.as_array_mut();
    ///
    /// // Scale both nodes by 2.0
    /// let scale = Vector::splat(2.0);
    /// nodes[0].scale(scale);
    /// nodes[1].scale(scale);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`as_array`](Self::as_array) - Immutable version
    #[inline(always)]
    pub fn as_array_mut(&mut self) -> [&mut BvhNode; 2] {
        [&mut self.left, &mut self.right]
    }

    /// Merges both child nodes to create their parent node.
    ///
    /// The parent's AABB will be the union of both children's AABBs, and the parent's
    /// leaf count will be the sum of both children's leaf counts. The `my_id` parameter
    /// becomes the parent's `children` field, pointing back to this `BvhNodeWide`.
    ///
    /// # Arguments
    ///
    /// * `my_id` - The index of this `BvhNodeWide` in the BVH's node array
    ///
    /// # Returns
    ///
    /// A new `BvhNode` representing the parent of both children.
    pub fn merged(&self, my_id: u32) -> BvhNode {
        self.left.merged(&self.right, my_id)
    }

    /// Returns the total number of leaves contained in both child nodes.
    ///
    /// This is the sum of the leaf counts of the left and right children. For leaf
    /// nodes, the count is 1. For internal nodes, it's the sum of their descendants.
    ///
    /// # Returns
    ///
    /// The total number of leaves in the subtrees rooted at both children.
    pub fn leaf_count(&self) -> u32 {
        self.left.leaf_count() + self.right.leaf_count()
    }
}

#[repr(C)] // SAFETY: needed to ensure SIMD aabb checks rely on the layout.
#[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
pub(super) struct BvhNodeSimd {
    mins: glamx::Vec3A,
    maxs: glamx::Vec3A,
}

// SAFETY: compile-time assertions to ensure we can transmute between `BvhNode` and `BvhNodeSimd`.
#[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
static_assertions::assert_eq_align!(BvhNode, BvhNodeSimd);
#[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
static_assertions::assert_eq_size!(BvhNode, BvhNodeSimd);

/// A single node (internal or leaf) of a BVH.
///
/// Each node stores an axis-aligned bounding box (AABB) that encompasses all geometry
/// contained within its subtree. A node is either:
/// - **Leaf**: Contains a single piece of geometry (leaf_count == 1)
/// - **Internal**: Contains two child nodes (leaf_count > 1)
///
/// # Structure
///
/// - **AABB**: Stored as separate `mins` and `maxs` points for efficiency
/// - **Children**: For internal nodes, index to a `BvhNodeWide` containing two child nodes.
///   For leaf nodes, this is the user-provided leaf data (typically an index).
/// - **Leaf Count**: Number of leaves in the subtree (1 for leaves, sum of children for internal)
///
/// # Memory Layout
///
/// The structure is carefully laid out for optimal performance:
/// - In 3D with f32: 32 bytes, 16-byte aligned (for SIMD operations)
/// - Fields ordered to enable efficient SIMD AABB tests
/// - The `#[repr(C)]` ensures predictable layout for unsafe optimizations
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::BvhNode;
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// // Create a leaf node
/// let aabb = Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0));
/// let leaf = BvhNode::leaf(aabb, 42);
///
/// assert!(leaf.is_leaf());
/// assert_eq!(leaf.leaf_data(), Some(42));
/// assert_eq!(leaf.aabb(), aabb);
/// # }
/// ```
///
/// # See Also
///
/// - `BvhNodeWide` - Pair of nodes stored together
/// - [`Bvh`] - The main BVH structure
#[derive(Copy, Clone, Debug)]
#[repr(C)] // SAFETY: needed to ensure SIMD aabb checks rely on the layout.
#[cfg_attr(all(feature = "f32", feature = "dim3"), repr(align(16)))]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct BvhNode {
    /// Mins coordinates of the node’s bounding volume.
    pub(super) mins: Vector,
    /// Children of this node. A node has either 0 (i.e. it’s a leaf) or 2 children.
    ///
    /// If [`Self::leaf_count`] is 1, then the node has 0 children and is a leaf.
    pub(super) children: u32,
    /// Maxs coordinates of this node’s bounding volume.
    pub(super) maxs: Vector,
    /// Packed data associated to this node (leaf count and flags).
    pub(super) data: BvhNodeData,
}

impl BvhNode {
    #[inline(always)]
    pub(super) fn zeros() -> Self {
        Self {
            mins: Vector::ZERO,
            children: 0,
            maxs: Vector::ZERO,
            data: BvhNodeData(0),
        }
    }

    /// Creates a new leaf node with the given AABB and user data.
    ///
    /// Leaf nodes represent actual geometry in the scene. Each leaf stores:
    /// - The AABB of the geometry it represents
    /// - A user-provided `leaf_data` value (typically an index into your geometry array)
    ///
    /// # Arguments
    ///
    /// * `aabb` - The axis-aligned bounding box for this leaf's geometry
    /// * `leaf_data` - User data associated with this leaf (typically an index or ID)
    ///
    /// # Returns
    ///
    /// A new `BvhNode` representing a leaf with the given properties.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// // Create an AABB for a unit cube
    /// let aabb = Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0));
    ///
    /// // Create a leaf node with index 0
    /// let leaf = BvhNode::leaf(aabb, 0);
    ///
    /// assert!(leaf.is_leaf());
    /// assert_eq!(leaf.leaf_data(), Some(0));
    /// assert_eq!(leaf.aabb(), aabb);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`is_leaf`](Self::is_leaf) - Check if a node is a leaf
    /// - [`leaf_data`](Self::leaf_data) - Get the leaf data back
    #[inline(always)]
    pub fn leaf(aabb: Aabb, leaf_data: u32) -> BvhNode {
        Self {
            mins: aabb.mins,
            maxs: aabb.maxs,
            children: leaf_data,
            data: BvhNodeData::with_leaf_count_and_pending_change(1),
        }
    }

    /// Returns the user data associated with this leaf node, if it is a leaf.
    ///
    /// For leaf nodes, this returns the `leaf_data` value that was provided when the
    /// leaf was created (typically an index into your geometry array). For internal
    /// nodes, this returns `None`.
    ///
    /// # Returns
    ///
    /// - `Some(leaf_data)` if this is a leaf node
    /// - `None` if this is an internal node
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0));
    /// let leaf = BvhNode::leaf(aabb, 42);
    ///
    /// assert_eq!(leaf.leaf_data(), Some(42));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`leaf`](Self::leaf) - Create a leaf node
    /// - [`is_leaf`](Self::is_leaf) - Check if a node is a leaf
    #[inline(always)]
    pub fn leaf_data(&self) -> Option<u32> {
        self.is_leaf().then_some(self.children)
    }

    /// Returns `true` if this node is a leaf.
    ///
    /// A node is a leaf if its leaf count is exactly 1, meaning it represents a single
    /// piece of geometry rather than a subtree of nodes.
    ///
    /// # Returns
    ///
    /// `true` if this is a leaf node, `false` if it's an internal node.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0));
    /// let leaf = BvhNode::leaf(aabb, 0);
    ///
    /// assert!(leaf.is_leaf());
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`leaf_data`](Self::leaf_data) - Get the leaf's user data
    #[inline(always)]
    pub fn is_leaf(&self) -> bool {
        self.leaf_count() == 1
    }

    #[inline(always)]
    pub(super) fn leaf_count(&self) -> u32 {
        self.data.leaf_count()
    }

    #[inline(always)]
    #[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
    pub(super) fn as_simd(&self) -> &BvhNodeSimd {
        // SAFETY: BvhNode is declared with the alignment
        //         and size of two SimdReal.
        unsafe { core::mem::transmute(self) }
    }

    #[inline(always)]
    pub(super) fn merged(&self, other: &Self, children: u32) -> Self {
        // TODO PERF: simd optimizations?
        Self {
            mins: self.mins.min(other.mins),
            children,
            maxs: self.maxs.max(other.maxs),
            data: self.data.merged(other.data),
        }
    }

    /// Returns the minimum corner of this node's AABB.
    ///
    /// The AABB (axis-aligned bounding box) is defined by two corners: the minimum
    /// corner (with the smallest coordinates on all axes) and the maximum corner.
    ///
    /// # Returns
    ///
    /// A point representing the minimum corner of the AABB.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(Vector::new(1.0, 2.0, 3.0), Vector::new(4.0, 5.0, 6.0));
    /// let node = BvhNode::leaf(aabb, 0);
    ///
    /// assert_eq!(node.mins(), Vector::new(1.0, 2.0, 3.0));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`maxs`](Self::maxs) - Get the maximum corner
    /// - [`aabb`](Self::aabb) - Get the full AABB
    #[inline]
    pub fn mins(&self) -> Vector {
        self.mins
    }

    /// Returns the maximum corner of this node's AABB.
    ///
    /// The AABB (axis-aligned bounding box) is defined by two corners: the minimum
    /// corner and the maximum corner (with the largest coordinates on all axes).
    ///
    /// # Returns
    ///
    /// A point representing the maximum corner of the AABB.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(Vector::new(1.0, 2.0, 3.0), Vector::new(4.0, 5.0, 6.0));
    /// let node = BvhNode::leaf(aabb, 0);
    ///
    /// assert_eq!(node.maxs(), Vector::new(4.0, 5.0, 6.0));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`mins`](Self::mins) - Get the minimum corner
    /// - [`aabb`](Self::aabb) - Get the full AABB
    #[inline]
    pub fn maxs(&self) -> Vector {
        self.maxs
    }

    /// Returns this node's AABB as an `Aabb` struct.
    ///
    /// Nodes store their AABBs as separate `mins` and `maxs` points for efficiency.
    /// This method reconstructs the full `Aabb` structure.
    ///
    /// # Returns
    ///
    /// An `Aabb` representing this node's bounding box.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let original_aabb = Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0));
    /// let node = BvhNode::leaf(original_aabb, 0);
    ///
    /// assert_eq!(node.aabb(), original_aabb);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`mins`](Self::mins) - Get just the minimum corner
    /// - [`maxs`](Self::maxs) - Get just the maximum corner
    #[inline]
    pub fn aabb(&self) -> Aabb {
        Aabb {
            mins: self.mins,
            maxs: self.maxs,
        }
    }

    /// Returns the center point of this node's AABB.
    ///
    /// The center is calculated as the midpoint between the minimum and maximum corners
    /// on all axes: `(mins + maxs) / 2`.
    ///
    /// # Returns
    ///
    /// A point representing the center of the AABB.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(Vector::ZERO, Vector::new(2.0, 4.0, 6.0));
    /// let node = BvhNode::leaf(aabb, 0);
    ///
    /// assert_eq!(node.center(), Vector::new(1.0, 2.0, 3.0));
    /// # }
    /// ```
    #[inline]
    pub fn center(&self) -> Vector {
        self.mins.midpoint(self.maxs)
    }

    /// Returns `true` if this node has been marked as changed.
    ///
    /// The BVH uses change tracking during incremental updates to identify which parts
    /// of the tree need refitting or optimization. This flag is set when a node or its
    /// descendants have been modified.
    ///
    /// # Returns
    ///
    /// `true` if the node is marked as changed, `false` otherwise.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0));
    /// let node = BvhNode::leaf(aabb, 0);
    ///
    /// // New leaf nodes are marked as changed (pending change)
    /// // This is used internally for tracking modifications
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`Bvh::refit`] - Uses change tracking to update the tree
    #[inline(always)]
    pub fn is_changed(&self) -> bool {
        self.data.is_changed()
    }

    /// Scales this node's AABB by the given factor.
    ///
    /// Each coordinate of both the minimum and maximum corners is multiplied by the
    /// corresponding component of the scale vector. This is useful when scaling an
    /// entire scene or object.
    ///
    /// # Arguments
    ///
    /// * `scale` - The scale factor to apply (per-axis)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(Vector::new(1.0, 1.0, 1.0), Vector::new(2.0, 2.0, 2.0));
    /// let mut node = BvhNode::leaf(aabb, 0);
    ///
    /// node.scale(Vector::new(2.0, 2.0, 2.0));
    ///
    /// assert_eq!(node.mins(), Vector::new(2.0, 2.0, 2.0));
    /// assert_eq!(node.maxs(), Vector::new(4.0, 4.0, 4.0));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`Bvh::scale`] - Scale an entire BVH tree
    #[inline]
    pub fn scale(&mut self, scale: Vector) {
        self.mins *= scale;
        self.maxs *= scale;
    }

    /// Calculates the volume of this node's AABB.
    ///
    /// The volume is the product of the extents on all axes:
    /// - In 2D: width × height (returns area)
    /// - In 3D: width × height × depth (returns volume)
    ///
    /// # Returns
    ///
    /// The volume (or area in 2D) of the AABB.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// // Create a 2×3×4 box
    /// let aabb = Aabb::new(Vector::ZERO, Vector::new(2.0, 3.0, 4.0));
    /// let node = BvhNode::leaf(aabb, 0);
    ///
    /// assert_eq!(node.volume(), 24.0); // 2 * 3 * 4 = 24
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`merged_volume`](Self::merged_volume) - Volume of merged AABBs
    #[inline]
    pub fn volume(&self) -> Real {
        // TODO PERF: simd optimizations?
        let extents = self.maxs - self.mins;
        #[cfg(feature = "dim2")]
        return extents.x * extents.y;
        #[cfg(feature = "dim3")]
        return extents.x * extents.y * extents.z;
    }

    /// Calculates the volume of the AABB that would result from merging this node with another.
    ///
    /// This computes the volume of the smallest AABB that would contain both this node's
    /// AABB and the other node's AABB, without actually creating the merged AABB. This is
    /// useful during BVH construction for evaluating different tree configurations.
    ///
    /// # Arguments
    ///
    /// * `other` - The other node to merge with
    ///
    /// # Returns
    ///
    /// The volume (or area in 2D) of the merged AABB.
    ///
    /// # Performance
    ///
    /// This is more efficient than creating the merged AABB and then computing its volume.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb1 = Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0));
    /// let aabb2 = Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0));
    ///
    /// let node1 = BvhNode::leaf(aabb1, 0);
    /// let node2 = BvhNode::leaf(aabb2, 1);
    ///
    /// // Merged AABB spans from (0,0,0) to (3,1,1) = 3×1×1 = 3
    /// assert_eq!(node1.merged_volume(&node2), 3.0);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`volume`](Self::volume) - Volume of a single node
    pub fn merged_volume(&self, other: &Self) -> Real {
        // TODO PERF: simd optimizations?
        let mins = self.mins.min(other.mins);
        let maxs = self.maxs.max(other.maxs);
        let extents = maxs - mins;

        #[cfg(feature = "dim2")]
        return extents.x * extents.y;
        #[cfg(feature = "dim3")]
        return extents.x * extents.y * extents.z;
    }

    /// Tests if this node's AABB intersects another node's AABB.
    ///
    /// Two AABBs intersect if they overlap on all axes. This includes cases where
    /// they only touch at their boundaries.
    ///
    /// # Arguments
    ///
    /// * `other` - The other node to test intersection with
    ///
    /// # Returns
    ///
    /// `true` if the AABBs intersect, `false` otherwise.
    ///
    /// # Performance
    ///
    /// When SIMD is enabled (3D, f32, simd-is-enabled feature), this uses vectorized
    /// comparisons for improved performance.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb1 = Aabb::new(Vector::ZERO, Vector::new(2.0, 2.0, 2.0));
    /// let aabb2 = Aabb::new(Vector::new(1.0, 1.0, 1.0), Vector::new(3.0, 3.0, 3.0));
    /// let aabb3 = Aabb::new(Vector::new(5.0, 5.0, 5.0), Vector::new(6.0, 6.0, 6.0));
    ///
    /// let node1 = BvhNode::leaf(aabb1, 0);
    /// let node2 = BvhNode::leaf(aabb2, 1);
    /// let node3 = BvhNode::leaf(aabb3, 2);
    ///
    /// assert!(node1.intersects(&node2)); // Overlapping
    /// assert!(!node1.intersects(&node3)); // Separated
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`contains`](Self::contains) - Check full containment
    #[cfg(not(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32")))]
    pub fn intersects(&self, other: &Self) -> bool {
        self.mins.cmple(other.maxs).all() && self.maxs.cmpge(other.mins).all()
    }

    /// Tests if this node's AABB intersects another node's AABB.
    ///
    /// Two AABBs intersect if they overlap on all axes. This includes cases where
    /// they only touch at their boundaries.
    ///
    /// # Arguments
    ///
    /// * `other` - The other node to test intersection with
    ///
    /// # Returns
    ///
    /// `true` if the AABBs intersect, `false` otherwise.
    ///
    /// # Performance
    ///
    /// This version uses SIMD optimizations for improved performance on supported platforms.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::bvh::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb1 = Aabb::new(Vector::ZERO, Vector::new(2.0, 2.0, 2.0));
    /// let aabb2 = Aabb::new(Vector::new(1.0, 1.0, 1.0), Vector::new(3.0, 3.0, 3.0));
    /// let aabb3 = Aabb::new(Vector::new(5.0, 5.0, 5.0), Vector::new(6.0, 6.0, 6.0));
    ///
    /// let node1 = BvhNode::leaf(aabb1, 0);
    /// let node2 = BvhNode::leaf(aabb2, 1);
    /// let node3 = BvhNode::leaf(aabb3, 2);
    ///
    /// assert!(node1.intersects(&node2)); // Overlapping
    /// assert!(!node1.intersects(&node3)); // Separated
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`contains`](Self::contains) - Check full containment
    #[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
    pub fn intersects(&self, other: &Self) -> bool {
        let simd_self = self.as_simd();
        let simd_other = other.as_simd();
        (simd_self.mins.cmple(simd_other.maxs) & simd_self.maxs.cmpge(simd_other.mins)).all()
    }

    /// Tests if this node's AABB fully contains another node's AABB.
    ///
    /// One AABB contains another if the other AABB is completely inside or on the
    /// boundary of this AABB on all axes.
    ///
    /// # Arguments
    ///
    /// * `other` - The other node to test containment of
    ///
    /// # Returns
    ///
    /// `true` if this AABB fully contains the other AABB, `false` otherwise.
    ///
    /// # Performance
    ///
    /// When SIMD is enabled (3D, f32, simd-is-enabled feature), this uses vectorized
    /// comparisons for improved performance.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let large = Aabb::new(Vector::ZERO, Vector::new(10.0, 10.0, 10.0));
    /// let small = Aabb::new(Vector::new(2.0, 2.0, 2.0), Vector::new(5.0, 5.0, 5.0));
    ///
    /// let node_large = BvhNode::leaf(large, 0);
    /// let node_small = BvhNode::leaf(small, 1);
    ///
    /// assert!(node_large.contains(&node_small)); // Large contains small
    /// assert!(!node_small.contains(&node_large)); // Small doesn't contain large
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`intersects`](Self::intersects) - Check any overlap
    /// - [`contains_aabb`](Self::contains_aabb) - Contains an `Aabb` directly
    #[cfg(not(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32")))]
    pub fn contains(&self, other: &Self) -> bool {
        self.mins.cmple(other.mins).all() && self.maxs.cmpge(other.maxs).all()
    }

    /// Tests if this node's AABB fully contains another node's AABB.
    ///
    /// One AABB contains another if the other AABB is completely inside or on the
    /// boundary of this AABB on all axes.
    ///
    /// # Arguments
    ///
    /// * `other` - The other node to test containment of
    ///
    /// # Returns
    ///
    /// `true` if this AABB fully contains the other AABB, `false` otherwise.
    ///
    /// # Performance
    ///
    /// This version uses SIMD optimizations for improved performance on supported platforms.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::bvh::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let large = Aabb::new(Vector::ZERO, Vector::new(10.0, 10.0, 10.0));
    /// let small = Aabb::new(Vector::new(2.0, 2.0, 2.0), Vector::new(5.0, 5.0, 5.0));
    ///
    /// let node_large = BvhNode::leaf(large, 0);
    /// let node_small = BvhNode::leaf(small, 1);
    ///
    /// assert!(node_large.contains(&node_small)); // Large contains small
    /// assert!(!node_small.contains(&node_large)); // Small doesn't contain large
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`intersects`](Self::intersects) - Check any overlap
    /// - [`contains_aabb`](Self::contains_aabb) - Contains an `Aabb` directly
    #[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
    pub fn contains(&self, other: &Self) -> bool {
        let simd_self = self.as_simd();
        let simd_other = other.as_simd();
        (simd_self.mins.cmple(simd_other.mins) & simd_self.maxs.cmpge(simd_other.maxs)).all()
    }

    /// Tests if this node's AABB fully contains the given AABB.
    ///
    /// This is similar to [`contains`](Self::contains) but takes an `Aabb` directly
    /// instead of another `BvhNode`.
    ///
    /// # Arguments
    ///
    /// * `other` - The AABB to test containment of
    ///
    /// # Returns
    ///
    /// `true` if this node's AABB fully contains the other AABB, `false` otherwise.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let large = Aabb::new(Vector::ZERO, Vector::new(10.0, 10.0, 10.0));
    /// let small = Aabb::new(Vector::new(2.0, 2.0, 2.0), Vector::new(5.0, 5.0, 5.0));
    ///
    /// let node = BvhNode::leaf(large, 0);
    ///
    /// assert!(node.contains_aabb(&small));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`contains`](Self::contains) - Contains another `BvhNode`
    pub fn contains_aabb(&self, other: &Aabb) -> bool {
        // TODO PERF: simd optimizations?
        self.mins.cmple(other.mins).all() && self.maxs.cmpge(other.maxs).all()
    }

    /// Casts a ray against this node's AABB.
    ///
    /// Computes the time of impact (parameter `t`) where the ray first intersects
    /// the AABB. The actual hit point is `ray.origin + ray.dir * t`.
    ///
    /// # Arguments
    ///
    /// * `ray` - The ray to cast
    /// * `max_toi` - Maximum time of impact to consider (typically use `f32::MAX` or `f64::MAX`)
    ///
    /// # Returns
    ///
    /// - The time of impact if the ray hits the AABB within `max_toi`
    /// - `Real::MAX` if there is no hit or the hit is beyond `max_toi`
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNode;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::query::Ray;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(Vector::new(5.0, -1.0, -1.0), Vector::new(6.0, 1.0, 1.0));
    /// let node = BvhNode::leaf(aabb, 0);
    ///
    /// // Ray from origin along X axis
    /// let ray = Ray::new(Vector::ZERO, Vector::new(1.0, 0.0, 0.0));
    ///
    /// let toi = node.cast_ray(&ray, f32::MAX);
    /// assert_eq!(toi, 5.0); // Ray hits at x=5.0
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`Ray`] - Ray structure
    /// - [`Bvh::traverse`] - For traversing the full BVH with ray casts
    pub fn cast_ray(&self, ray: &Ray, max_toi: Real) -> Real {
        self.aabb()
            .cast_local_ray(ray, max_toi, true)
            .unwrap_or(Real::MAX)
    }

    /// Casts a ray on this AABB, with SIMD optimizations.
    ///
    /// Returns `Real::MAX` if there is no hit.
    #[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
    pub(super) fn cast_inv_ray_simd(&self, ray: &super::bvh_queries::SimdInvRay) -> f32 {
        let simd_self = self.as_simd();
        let t1 = (simd_self.mins - ray.origin) * ray.inv_dir;
        let t2 = (simd_self.maxs - ray.origin) * ray.inv_dir;

        let tmin = t1.min(t2);
        let tmax = t1.max(t2);
        // let tmin = tmin.as_array_ref();
        // let tmax = tmax.as_array_ref();
        let tmin_n = tmin.max_element(); // tmin[0].max(tmin[1].max(tmin[2]));
        let tmax_n = tmax.min_element(); // tmax[0].min(tmax[1].min(tmax[2]));

        if tmax_n >= tmin_n && tmax_n >= 0.0 {
            tmin_n
        } else {
            f32::MAX
        }
    }
}

/// An index identifying a single BVH tree node.
///
/// The BVH stores nodes in pairs (`BvhNodeWide`), where each pair contains a left and
/// right child. This index encodes both which pair and which side (left or right) in a
/// single `usize` value for efficient storage and manipulation.
///
/// # Encoding
///
/// The index is encoded as: `(wide_node_index << 1) | is_right`
/// - The upper bits identify the `BvhNodeWide` (pair of nodes)
/// - The lowest bit indicates left (0) or right (1)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::BvhNodeIndex;
///
/// // Create indices for the left and right children of node pair 5
/// let left = BvhNodeIndex::left(5);
/// let right = BvhNodeIndex::right(5);
///
/// assert_eq!(left.sibling(), right);
/// assert_eq!(right.sibling(), left);
///
/// // Decompose to get the pair index and side
/// let (pair_idx, is_right) = left.decompose();
/// assert_eq!(pair_idx, 5);
/// assert_eq!(is_right, false);
/// # }
/// ```
///
/// # See Also
///
/// - `BvhNodeWide` - The pair of nodes this index points into
/// - [`Bvh`] - The main BVH structure
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct BvhNodeIndex(pub usize);

impl BvhNodeIndex {
    pub(super) const LEFT: bool = false;
    pub(super) const RIGHT: bool = true;

    /// Decomposes this index into its components.
    ///
    /// Returns a tuple of `(wide_node_index, is_right)` where:
    /// - `wide_node_index` is the index into the BVH's array of `BvhNodeWide` pairs
    /// - `is_right` is `false` for left child, `true` for right child
    ///
    /// # Returns
    ///
    /// A tuple `(usize, bool)` containing the pair index and side flag.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNodeIndex;
    ///
    /// let left = BvhNodeIndex::left(10);
    /// let (pair_idx, is_right) = left.decompose();
    ///
    /// assert_eq!(pair_idx, 10);
    /// assert_eq!(is_right, false);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`new`](Self::new) - Construct from components
    #[inline]
    pub fn decompose(self) -> (usize, bool) {
        (self.0 >> 1, (self.0 & 0b01) != 0)
    }

    /// Returns the sibling of this node.
    ///
    /// If this index points to the left child of a pair, returns the right child.
    /// If this index points to the right child, returns the left child.
    ///
    /// # Returns
    ///
    /// The `BvhNodeIndex` of the sibling node.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNodeIndex;
    ///
    /// let left = BvhNodeIndex::left(5);
    /// let right = BvhNodeIndex::right(5);
    ///
    /// assert_eq!(left.sibling(), right);
    /// assert_eq!(right.sibling(), left);
    /// # }
    /// ```
    #[inline]
    pub fn sibling(self) -> Self {
        // Just flip the first bit to switch between left and right child.
        Self(self.0 ^ 0b01)
    }

    /// Creates an index for the left child of a node pair.
    ///
    /// # Arguments
    ///
    /// * `id` - The index of the `BvhNodeWide` pair in the BVH's node array
    ///
    /// # Returns
    ///
    /// A `BvhNodeIndex` pointing to the left child of the specified pair.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNodeIndex;
    ///
    /// let left_child = BvhNodeIndex::left(0);
    /// let (pair_idx, is_right) = left_child.decompose();
    ///
    /// assert_eq!(pair_idx, 0);
    /// assert_eq!(is_right, false);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`right`](Self::right) - Create index for right child
    /// - [`new`](Self::new) - Create index with explicit side
    #[inline]
    pub fn left(id: u32) -> Self {
        Self::new(id, Self::LEFT)
    }

    /// Creates an index for the right child of a node pair.
    ///
    /// # Arguments
    ///
    /// * `id` - The index of the `BvhNodeWide` pair in the BVH's node array
    ///
    /// # Returns
    ///
    /// A `BvhNodeIndex` pointing to the right child of the specified pair.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNodeIndex;
    ///
    /// let right_child = BvhNodeIndex::right(0);
    /// let (pair_idx, is_right) = right_child.decompose();
    ///
    /// assert_eq!(pair_idx, 0);
    /// assert_eq!(is_right, true);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`left`](Self::left) - Create index for left child
    /// - [`new`](Self::new) - Create index with explicit side
    #[inline]
    pub fn right(id: u32) -> Self {
        Self::new(id, Self::RIGHT)
    }

    /// Creates a new node index from a pair ID and side flag.
    ///
    /// # Arguments
    ///
    /// * `id` - The index of the `BvhNodeWide` pair in the BVH's node array
    /// * `is_right` - `false` for left child, `true` for right child
    ///
    /// # Returns
    ///
    /// A `BvhNodeIndex` encoding both the pair and the side.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::BvhNodeIndex;
    ///
    /// let left = BvhNodeIndex::new(3, false);
    /// let right = BvhNodeIndex::new(3, true);
    ///
    /// assert_eq!(left, BvhNodeIndex::left(3));
    /// assert_eq!(right, BvhNodeIndex::right(3));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`left`](Self::left) - Convenience method for left child
    /// - [`right`](Self::right) - Convenience method for right child
    #[inline]
    pub fn new(id: u32, is_right: bool) -> Self {
        Self(((id as usize) << 1) | (is_right as usize))
    }
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub(crate) struct BvhNodeVec(pub(crate) Vec<BvhNodeWide>);

impl Deref for BvhNodeVec {
    type Target = Vec<BvhNodeWide>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BvhNodeVec {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Index<usize> for BvhNodeVec {
    type Output = BvhNodeWide;

    #[inline(always)]
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for BvhNodeVec {
    #[inline(always)]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Index<BvhNodeIndex> for BvhNodeVec {
    type Output = BvhNode;

    #[inline(always)]
    fn index(&self, index: BvhNodeIndex) -> &Self::Output {
        self.0[index.0 >> 1].as_array()[index.0 & 1]
    }
}

impl IndexMut<BvhNodeIndex> for BvhNodeVec {
    #[inline(always)]
    fn index_mut(&mut self, index: BvhNodeIndex) -> &mut Self::Output {
        self.0[index.0 >> 1].as_array_mut()[index.0 & 1]
    }
}

/// A Bounding Volume Hierarchy (BVH) for spatial queries and collision detection.
///
/// A BVH is a tree structure where each node contains an Axis-Aligned Bounding Box (AABB)
/// that encloses all geometry in its subtree. Leaf nodes represent individual objects,
/// while internal nodes partition space hierarchically. This enables efficient spatial
/// queries by allowing entire subtrees to be culled during traversal.
///
/// # What is a BVH and Why Use It?
///
/// A Bounding Volume Hierarchy organizes geometric objects (represented by their AABBs)
/// into a binary tree. Each internal node's AABB bounds the union of its two children's
/// AABBs. This hierarchical structure enables:
///
/// - **Fast spatial queries**: Ray casting, point queries, and AABB intersection tests
/// - **Broad-phase collision detection**: Quickly find potentially colliding pairs
/// - **Efficient culling**: Skip entire branches that don't intersect query regions
///
/// ## Performance Benefits
///
/// Without a BVH, testing N objects against M queries requires O(N × M) tests.
/// With a BVH, this reduces to approximately O(M × log N) for most queries,
/// providing dramatic speedups for scenes with many objects:
///
/// - **1,000 objects**: ~10x faster for ray casting
/// - **10,000 objects**: ~100x faster for ray casting
/// - **Critical for**: Real-time applications (games, physics engines, robotics)
///
/// ## Structure
///
/// The BVH is a binary tree where:
/// - **Leaf nodes**: Contain references to actual geometry (via user-provided indices)
/// - **Internal nodes**: Contain two children and an AABB encompassing both
/// - **Root**: The top-level node encompassing the entire scene
///
/// # Basic Usage - Static Scenes
///
/// For scenes where objects don't move, build the BVH once and query repeatedly:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// // Create AABBs for your objects
/// let objects = vec![
///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
///     Aabb::new(Vector::new(5.0, 0.0, 0.0), Vector::new(6.0, 1.0, 1.0)),
///     Aabb::new(Vector::new(10.0, 0.0, 0.0), Vector::new(11.0, 1.0, 1.0)),
/// ];
///
/// // Build the BVH - the index of each AABB becomes its leaf ID
/// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &objects);
///
/// // Query which objects intersect a region
/// let query_region = Aabb::new(
///     Vector::new(-1.0, -1.0, -1.0),
///     Vector::new(2.0, 2.0, 2.0)
/// );
///
/// for leaf_id in bvh.intersect_aabb(&query_region) {
///     println!("Object {} intersects the query region", leaf_id);
///     // leaf_id corresponds to the index in the original 'objects' vec
/// }
/// # }
/// ```
///
/// # Dynamic Scenes - Adding and Updating Objects
///
/// The BVH supports dynamic scenes where objects move or are added/removed:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhWorkspace};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// let mut bvh = Bvh::new();
/// let mut workspace = BvhWorkspace::default();
///
/// // Add objects dynamically with custom IDs
/// bvh.insert(Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)), 100);
/// bvh.insert(Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)), 200);
///
/// // Update an object's position (by re-inserting with same ID)
/// bvh.insert(Aabb::new(Vector::new(0.5, 0.5, 0.0), Vector::new(1.5, 1.5, 1.0)), 100);
///
/// // Refit the tree after updates for optimal query performance
/// bvh.refit(&mut workspace);
///
/// // Remove an object
/// bvh.remove(200);
/// # }
/// ```
///
/// # Ray Casting Example
///
/// Find the closest object hit by a ray:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::query::{Ray, RayCast};
/// use parry3d::math::Vector;
///
/// let objects = vec![
///     Aabb::new(Vector::new(0.0, 0.0, 5.0), Vector::new(1.0, 1.0, 6.0)),
///     Aabb::new(Vector::new(0.0, 0.0, 10.0), Vector::new(1.0, 1.0, 11.0)),
/// ];
///
/// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &objects);
///
/// // Cast a ray forward along the Z axis
/// let ray = Ray::new(Vector::new(0.5, 0.5, 0.0), Vector::new(0.0, 0.0, 1.0));
/// let max_distance = 100.0;
///
/// // The BVH finds potentially intersecting leaves, then you test actual geometry
/// if let Some((leaf_id, hit_time)) = bvh.cast_ray(&ray, max_distance, |leaf_id, best_hit| {
///     // Test ray against the actual geometry for this leaf
///     // For this example, we test against the AABB itself
///     let aabb = &objects[leaf_id as usize];
///     aabb.cast_local_ray(&ray, best_hit, true)
/// }) {
///     println!("Ray hit object {} at distance {}", leaf_id, hit_time);
///     let hit_point = ray.point_at(hit_time);
///     println!("Hit point: {:?}", hit_point);
/// }
/// # }
/// ```
///
/// # Construction Strategies
///
/// Different build strategies offer trade-offs between build time and query performance:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// let aabbs = vec![
///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
/// ];
///
/// // Binned strategy: Fast construction, good quality (recommended default)
/// let bvh_binned = Bvh::from_leaves(BvhBuildStrategy::Binned, &aabbs);
///
/// // PLOC strategy: Slower construction, best quality for ray-casting
/// // Use this for static scenes with heavy query workloads
/// let bvh_ploc = Bvh::from_leaves(BvhBuildStrategy::Ploc, &aabbs);
/// # }
/// ```
///
/// # Maintenance for Dynamic Scenes
///
/// The BVH provides operations to maintain good performance as scenes change:
///
/// ## Refitting
///
/// After objects move, update the tree's AABBs efficiently:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhWorkspace};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// let mut bvh = Bvh::new();
/// let mut workspace = BvhWorkspace::default();
///
/// // Insert initial objects
/// bvh.insert(Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)), 0);
/// bvh.insert(Aabb::new(Vector::new(5.0, 0.0, 0.0), Vector::new(6.0, 1.0, 1.0)), 1);
///
/// // Simulate object movement every frame
/// for frame in 0..100 {
///     let offset = frame as f32 * 0.1;
///     bvh.insert(Aabb::new(
///         Vector::new(offset, 0.0, 0.0),
///         Vector::new(1.0 + offset, 1.0, 1.0)
///     ), 0);
///
///     // Refit updates internal AABBs - very fast operation
///     bvh.refit(&mut workspace);
///
///     // Now you can query the BVH with updated positions
/// }
/// # }
/// ```
///
/// ## Incremental Optimization
///
/// For scenes with continuous movement, incrementally improve tree quality:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::partitioning::{Bvh, BvhWorkspace};
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// let mut bvh = Bvh::new();
/// let mut workspace = BvhWorkspace::default();
///
/// // Build initial tree
/// for i in 0..1000 {
///     let aabb = Aabb::new(
///         Vector::new(i as f32, 0.0, 0.0),
///         Vector::new(i as f32 + 1.0, 1.0, 1.0)
///     );
///     bvh.insert(aabb, i);
/// }
///
/// // In your update loop:
/// for frame in 0..100 {
///     // Update object positions...
///
///     bvh.refit(&mut workspace);
///
///     // Incrementally optimize tree quality (rebuilds small parts of tree)
///     // Call this every few frames, not every frame
///     if frame % 5 == 0 {
///         bvh.optimize_incremental(&mut workspace);
///     }
/// }
/// # }
/// ```
///
/// # Typical Workflows
///
/// ## Static Scene (Build Once, Query Many Times)
/// 1. Create AABBs for all objects
/// 2. Build BVH with `from_leaves`
/// 3. Query repeatedly (ray casting, intersection tests, etc.)
///
/// ## Dynamic Scene (Objects Move)
/// 1. Build initial BVH or start empty
/// 2. Each frame:
///    - Update positions with `insert`
///    - Call `refit` to update tree AABBs
///    - Perform queries
/// 3. Occasionally call `optimize_incremental` (every 5-10 frames)
///
/// ## Fully Dynamic (Objects Added/Removed)
/// 1. Start with empty BVH
/// 2. Add objects with `insert` as they're created
/// 3. Remove objects with `remove` as they're destroyed
/// 4. Call `refit` after batch updates
/// 5. Call `optimize_incremental` periodically
///
/// # Performance Tips
///
/// - **Reuse `BvhWorkspace`**: Pass the same workspace to multiple operations to avoid
///   allocations
/// - **Batch updates**: Update many leaves, then call `refit` once instead of refitting
///   after each update
/// - **Optimize periodically**: Call `optimize_incremental` every few frames for highly
///   dynamic scenes, not every frame
/// - **Choose right strategy**: Use Binned for most cases, PLOC for static scenes with
///   heavy ray-casting
/// - **Use `insert_or_update_partially`**: For bulk updates followed by a single `refit`
///
/// # Complexity
///
/// - **Construction**: O(n log n) where n is the number of leaves
/// - **Query (average)**: O(log n) for well-balanced trees
/// - **Insert**: O(log n) average
/// - **Remove**: O(log n) average
/// - **Refit**: O(n) but very fast (just updates AABBs)
/// - **Memory**: ~64 bytes per pair of children (3D f32 SIMD), O(n) total
///
/// # See Also
///
/// - [`BvhBuildStrategy`] - Choose construction algorithm (Binned vs PLOC)
/// - [`BvhWorkspace`] - Reusable workspace to avoid allocations
/// - [`BvhNode`] - Individual tree nodes with AABBs
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct Bvh {
    pub(super) nodes: BvhNodeVec,
    // Parent indices for elements in `nodes`.
    // We don’t store this in `Self::nodes` since it’s only useful for node removal.
    pub(super) parents: Vec<BvhNodeIndex>,
    pub(super) leaf_node_indices: VecMap<BvhNodeIndex>,
}

impl Bvh {
    /// Creates an empty BVH with no leaves.
    ///
    /// This is equivalent to `Bvh::default()` but more explicit. Use this when you plan
    /// to incrementally build the tree using [`insert`](Self::insert), or when you need
    /// an empty placeholder BVH.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::Bvh;
    ///
    /// let bvh = Bvh::new();
    /// assert!(bvh.is_empty());
    /// assert_eq!(bvh.leaf_count(), 0);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`from_leaves`](Self::from_leaves) - Build from AABBs
    /// - [`from_iter`](Self::from_iter) - Build from an iterator
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new BVH from a slice of AABBs.
    ///
    /// Each AABB in the slice becomes a leaf in the BVH. The leaf at index `i` in the slice
    /// will have leaf data `i`, which can be used to identify which object a query result
    /// refers to.
    ///
    /// # Arguments
    ///
    /// * `strategy` - The construction algorithm to use (see [`BvhBuildStrategy`])
    /// * `leaves` - Slice of AABBs, one for each object in the scene
    ///
    /// # Returns
    ///
    /// A new `Bvh` containing all the leaves organized in a tree structure.
    ///
    /// # Performance
    ///
    /// - **Time**: O(n log n) where n is the number of leaves
    /// - **Space**: O(n) additional memory during construction
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(4.0, 0.0, 0.0), Vector::new(5.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::Binned, &aabbs);
    ///
    /// assert_eq!(bvh.leaf_count(), 3);
    /// // Leaf 0 corresponds to aabbs[0], leaf 1 to aabbs[1], etc.
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`from_iter`](Self::from_iter) - Build from an iterator with custom indices
    /// - [`BvhBuildStrategy`] - Choose construction algorithm
    pub fn from_leaves(strategy: BvhBuildStrategy, leaves: &[Aabb]) -> Self {
        Self::from_iter(strategy, leaves.iter().copied().enumerate())
    }

    /// Creates a new BVH from an iterator of (index, AABB) pairs.
    ///
    /// This is more flexible than [`from_leaves`](Self::from_leaves) as it allows you to
    /// provide custom leaf indices. This is useful when your objects don't have contiguous
    /// indices, or when you want to use sparse IDs.
    ///
    /// # Arguments
    ///
    /// * `strategy` - The construction algorithm to use (see [`BvhBuildStrategy`])
    /// * `leaves` - Iterator yielding `(index, aabb)` pairs
    ///
    /// # Returns
    ///
    /// A new `Bvh` containing all the leaves organized in a tree structure.
    ///
    /// # Notes
    ///
    /// - Indices are stored internally as `u32`, but the iterator accepts `usize` for convenience
    /// - You can use `.enumerate()` directly on an AABB iterator
    /// - Indices larger than `u32::MAX` will overflow
    ///
    /// # Performance
    ///
    /// - **Time**: O(n log n) where n is the number of leaves
    /// - **Space**: O(n) additional memory during construction
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// // Create a BVH with custom indices
    /// let leaves = vec![
    ///     (10, Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0))),
    ///     (20, Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0))),
    ///     (30, Aabb::new(Vector::new(4.0, 0.0, 0.0), Vector::new(5.0, 1.0, 1.0))),
    /// ];
    ///
    /// let bvh = Bvh::from_iter(BvhBuildStrategy::Binned, leaves.into_iter());
    ///
    /// assert_eq!(bvh.leaf_count(), 3);
    /// // Leaf data will be 10, 20, 30 instead of 0, 1, 2
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`from_leaves`](Self::from_leaves) - Simpler version with automatic indices
    /// - [`BvhBuildStrategy`] - Choose construction algorithm
    pub fn from_iter<It>(strategy: BvhBuildStrategy, leaves: It) -> Self
    where
        It: IntoIterator<Item = (usize, Aabb)>,
    {
        let leaves = leaves.into_iter();
        let (capacity_lo, capacity_up) = leaves.size_hint();
        let capacity = capacity_up.unwrap_or(capacity_lo);

        let mut result = Self::new();
        let mut workspace = BvhWorkspace::default();
        workspace.rebuild_leaves.reserve(capacity);
        result.leaf_node_indices.reserve_len(capacity);

        for (leaf_id, leaf_aabb) in leaves {
            workspace
                .rebuild_leaves
                .push(BvhNode::leaf(leaf_aabb, leaf_id as u32));
            let _ = result
                .leaf_node_indices
                .insert(leaf_id, BvhNodeIndex::default());
        }

        // Handle special cases that don’t play well with the rebuilds.
        match workspace.rebuild_leaves.len() {
            0 => {}
            1 => {
                result.nodes.push(BvhNodeWide {
                    left: workspace.rebuild_leaves[0],
                    right: BvhNode::zeros(),
                });
                result.parents.push(BvhNodeIndex::default());
                result.leaf_node_indices[0] = BvhNodeIndex::left(0);
            }
            2 => {
                result.nodes.push(BvhNodeWide {
                    left: workspace.rebuild_leaves[0],
                    right: workspace.rebuild_leaves[1],
                });
                result.parents.push(BvhNodeIndex::default());
                result.leaf_node_indices[0] = BvhNodeIndex::left(0);
                result.leaf_node_indices[1] = BvhNodeIndex::right(0);
            }
            _ => {
                result.nodes.reserve(capacity);
                result.parents.reserve(capacity);
                result.parents.clear();
                result.nodes.push(BvhNodeWide::zeros());
                result.parents.push(BvhNodeIndex::default());

                match strategy {
                    BvhBuildStrategy::Ploc => {
                        result.rebuild_range_ploc(0, &mut workspace.rebuild_leaves)
                    }
                    BvhBuildStrategy::Binned => {
                        result.rebuild_range_binned(0, &mut workspace.rebuild_leaves)
                    }
                }

                // Layout in depth-first order.
                result.refit(&mut workspace);
            }
        }

        result
    }

    /// Returns the AABB that bounds all geometry in this BVH.
    ///
    /// This is the AABB of the root node, which encompasses all leaves in the tree.
    /// For an empty BVH, returns an invalid AABB (with mins > maxs).
    ///
    /// # Returns
    ///
    /// An `Aabb` that contains all objects in the BVH.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::{Aabb, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(5.0, 0.0, 0.0), Vector::new(6.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    /// let root_aabb = bvh.root_aabb();
    ///
    /// // Root AABB contains both leaves
    /// assert!(root_aabb.contains(&aabbs[0]));
    /// assert!(root_aabb.contains(&aabbs[1]));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`is_empty`](Self::is_empty) - Check if BVH has no leaves
    pub fn root_aabb(&self) -> Aabb {
        match self.leaf_count() {
            0 => Aabb::new_invalid(),
            1 => self.nodes[0].left.aabb(),
            _ => self.nodes[0]
                .left
                .aabb()
                .merged(&self.nodes[0].right.aabb()),
        }
    }

    /// Scales all AABBs in the tree by the given factors.
    ///
    /// This multiplies all AABB coordinates (mins and maxs) by the corresponding components
    /// of the scale vector. This is useful when scaling an entire scene or changing coordinate
    /// systems.
    ///
    /// # Arguments
    ///
    /// * `scale` - Per-axis scale factors (must all be positive)
    ///
    /// # Panics
    ///
    /// This function has undefined behavior if any scale component is negative or zero.
    /// Always use positive scale values.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    /// ];
    ///
    /// let mut bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    ///
    /// // Scale by 2x on all axes
    /// bvh.scale(Vector::new(2.0, 2.0, 2.0));
    ///
    /// let root = bvh.root_aabb();
    /// assert_eq!(root.maxs, Vector::new(2.0, 2.0, 2.0));
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`BvhNode::scale`] - Scale a single node
    pub fn scale(&mut self, scale: Vector) {
        for node in self.nodes.0.iter_mut() {
            node.left.scale(scale);
            node.right.scale(scale);
        }
    }

    /// Returns `true` if this BVH contains no leaves.
    ///
    /// An empty BVH has no geometry and cannot be queried meaningfully.
    ///
    /// # Returns
    ///
    /// `true` if the BVH is empty, `false` otherwise.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::Bvh;
    ///
    /// let empty_bvh = Bvh::new();
    /// assert!(empty_bvh.is_empty());
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`leaf_count`](Self::leaf_count) - Get the number of leaves
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns a reference to the leaf node with the given index.
    ///
    /// The `leaf_key` is the index that was provided when constructing the BVH
    /// (either the position in the slice for [`from_leaves`](Self::from_leaves),
    /// or the custom index for [`from_iter`](Self::from_iter)).
    ///
    /// # Arguments
    ///
    /// * `leaf_key` - The leaf index to look up
    ///
    /// # Returns
    ///
    /// - `Some(&BvhNode)` if a leaf with that index exists
    /// - `None` if no leaf with that index exists
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    ///
    /// // Leaf 0 exists (from aabbs[0])
    /// assert!(bvh.leaf_node(0).is_some());
    ///
    /// // Leaf 1 doesn't exist
    /// assert!(bvh.leaf_node(1).is_none());
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`remove`](Self::remove) - Remove a leaf by index
    pub fn leaf_node(&self, leaf_key: u32) -> Option<&BvhNode> {
        let idx = self.leaf_node_indices.get(leaf_key as usize)?;
        Some(&self.nodes[*idx])
    }

    /// Estimates the total memory usage of this BVH in bytes.
    ///
    /// This includes both the stack size of the `Bvh` struct itself and all
    /// heap-allocated memory (node arrays, parent indices, leaf index maps).
    ///
    /// # Returns
    ///
    /// Approximate memory usage in bytes.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs: Vec<_> = (0..100)
    ///     .map(|i| {
    ///         let f = i as f32;
    ///         Aabb::new(Vector::new(f, 0.0, 0.0), Vector::new(f + 1.0, 1.0, 1.0))
    ///     })
    ///     .collect();
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    ///
    /// println!("BVH memory usage: {} bytes", bvh.total_memory_size());
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`heap_memory_size`](Self::heap_memory_size) - Only heap-allocated memory
    pub fn total_memory_size(&self) -> usize {
        size_of::<Self>() + self.heap_memory_size()
    }

    /// Estimates the heap-allocated memory usage of this BVH in bytes.
    ///
    /// This only counts dynamically allocated memory (nodes, indices, etc.) and
    /// excludes the stack size of the `Bvh` struct itself.
    ///
    /// # Returns
    ///
    /// Approximate heap memory usage in bytes.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs: Vec<_> = (0..100)
    ///     .map(|i| {
    ///         let f = i as f32;
    ///         Aabb::new(Vector::new(f, 0.0, 0.0), Vector::new(f + 1.0, 1.0, 1.0))
    ///     })
    ///     .collect();
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    ///
    /// println!("BVH heap memory: {} bytes", bvh.heap_memory_size());
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`total_memory_size`](Self::total_memory_size) - Total memory including stack
    pub fn heap_memory_size(&self) -> usize {
        let Self {
            nodes,
            parents,
            leaf_node_indices,
        } = self;
        nodes.capacity() * size_of::<BvhNodeWide>()
            + parents.capacity() * size_of::<BvhNodeIndex>()
            + leaf_node_indices.capacity() * size_of::<BvhNodeIndex>()
    }

    /// Computes the depth of the subtree rooted at the specified node.
    ///
    /// The depth is the number of levels from the root to the deepest leaf. A single
    /// node has depth 1, a node with two leaf children has depth 2, etc.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The index of the root node of the subtree (use 0 for the entire tree)
    ///
    /// # Returns
    ///
    /// The depth of the subtree, or 0 for an empty tree.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs: Vec<_> = (0..4)
    ///     .map(|i| {
    ///         let f = i as f32;
    ///         Aabb::new(Vector::new(f, 0.0, 0.0), Vector::new(f + 1.0, 1.0, 1.0))
    ///     })
    ///     .collect();
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    ///
    /// // Get depth of entire tree
    /// let depth = bvh.subtree_depth(0);
    /// assert!(depth >= 2); // At least 2 levels with 4 leaves
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`leaf_count`](Self::leaf_count) - Number of leaves in the tree
    pub fn subtree_depth(&self, node_id: u32) -> u32 {
        if node_id == 0 && self.nodes.is_empty() {
            return 0;
        } else if node_id == 0 && self.nodes.len() == 1 {
            return 1 + (self.nodes[0].right.leaf_count() != 0) as u32;
        }

        let node = &self.nodes[node_id as usize];

        let left_depth = if node.left.is_leaf() {
            1
        } else {
            self.subtree_depth(node.left.children)
        };

        let right_depth = if node.right.is_leaf() {
            1
        } else {
            self.subtree_depth(node.right.children)
        };

        left_depth.max(right_depth) + 1
    }

    /// Returns the number of leaves in this BVH.
    ///
    /// Each leaf represents one geometric object that was provided during construction
    /// or added via [`insert`](Self::insert).
    ///
    /// # Returns
    ///
    /// The total number of leaves in the tree.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(4.0, 0.0, 0.0), Vector::new(5.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    /// assert_eq!(bvh.leaf_count(), 3);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`is_empty`](Self::is_empty) - Check if the tree has no leaves
    pub fn leaf_count(&self) -> u32 {
        if self.nodes.is_empty() {
            0
        } else {
            self.nodes[0].leaf_count()
        }
    }

    /// Removes a leaf from the BVH.
    ///
    /// This removes the leaf with the specified index and updates the tree structure
    /// accordingly. The sibling of the removed leaf moves up to take its parent's place,
    /// and all ancestor AABBs and leaf counts are updated.
    ///
    /// # Arguments
    ///
    /// * `leaf_index` - The index of the leaf to remove (the same index used when constructing)
    ///
    /// # Performance
    ///
    /// - **Time**: O(h) where h is the tree height (typically O(log n))
    /// - Updates AABBs and leaf counts for all ancestors of the removed leaf
    /// - For heavily unbalanced trees, consider rebuilding or rebalancing after many removals
    ///
    /// # Notes
    ///
    /// - If the leaf doesn't exist, this is a no-op
    /// - Removing the last leaf results in an empty BVH
    /// - The tree structure remains valid after removal
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(4.0, 0.0, 0.0), Vector::new(5.0, 1.0, 1.0)),
    /// ];
    ///
    /// let mut bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    /// assert_eq!(bvh.leaf_count(), 3);
    ///
    /// // Remove the middle leaf
    /// bvh.remove(1);
    /// assert_eq!(bvh.leaf_count(), 2);
    ///
    /// // Leaf 1 no longer exists
    /// assert!(bvh.leaf_node(1).is_none());
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`insert`](Self::insert) - Add a new leaf to the BVH
    /// - [`refit`](Self::refit) - Update AABBs after leaf movements
    /// - [`optimize_incremental`](Self::optimize_incremental) - Improve tree quality
    // TODO: should we make a version that doesn't traverse the parents?
    //       If we do, we must be very careful that the leaf counts that become
    //       invalid don't break other algorithm… (and, in particular, the root
    //       special case that checks if its right element has 0 leaf count).
    pub fn remove(&mut self, leaf_index: u32) {
        if let Some(node_index) = self.leaf_node_indices.remove(leaf_index as usize) {
            if self.leaf_node_indices.is_empty() {
                // We deleted the last leaf! Remove the root.
                self.nodes.clear();
                self.parents.clear();
                return;
            }

            let sibling = node_index.sibling();
            let (wide_node_index, is_right) = node_index.decompose();

            if wide_node_index == 0 {
                if self.nodes[sibling].is_leaf() {
                    // If the sibling is a leaf, we end up with a partial root.
                    // There is no parent pointer to update.
                    if !is_right {
                        // We remove the left leaf. Move the right leaf in its place.
                        let moved_index = self.nodes[0].right.children;
                        self.nodes[0].left = self.nodes[0].right;
                        self.leaf_node_indices[moved_index as usize] = BvhNodeIndex::left(0);
                    }

                    // Now we can just clear the right leaf.
                    self.nodes[0].right = BvhNode::zeros();
                } else {
                    // The sibling isn’t a leaf. It becomes the new root at index 0.
                    self.nodes[0] = self.nodes[self.nodes[sibling].children as usize];
                    // Both parent pointers need to be updated since both nodes moved to the root.
                    let new_root = &mut self.nodes[0];
                    if new_root.left.is_leaf() {
                        self.leaf_node_indices[new_root.left.children as usize] =
                            BvhNodeIndex::left(0);
                    } else {
                        self.parents[new_root.left.children as usize] = BvhNodeIndex::left(0);
                    }
                    if new_root.right.is_leaf() {
                        self.leaf_node_indices[new_root.right.children as usize] =
                            BvhNodeIndex::right(0);
                    } else {
                        self.parents[new_root.right.children as usize] = BvhNodeIndex::right(0);
                    }
                }
            } else {
                // The sibling moves to the parent. The affected wide node is no longer accessible,
                // but we can just leave it there, it will get cleaned up at the next refit.
                let parent = self.parents[wide_node_index];
                let sibling = &self.nodes[sibling];

                if sibling.is_leaf() {
                    self.leaf_node_indices[sibling.children as usize] = parent;
                } else {
                    self.parents[sibling.children as usize] = parent;
                }

                self.nodes[parent] = *sibling;

                // TODO: we could use that propagation as an opportunity to
                //       apply some rotations?
                let mut curr = parent.decompose().0;
                while curr != 0 {
                    let parent = self.parents[curr];
                    self.nodes[parent] = self.nodes[curr].merged(curr as u32);
                    curr = parent.decompose().0;
                }
            }
        }
    }

    // pub fn quality_metric(&self) -> Real {
    //     let mut metric = 0.0;
    //     for i in 0..self.nodes.len() {
    //         if !self.nodes[i].is_leaf() {
    //             metric += self.sah_cost(i);
    //         }
    //     }
    //     metric
    // }
}
