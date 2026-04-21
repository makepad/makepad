//! Axis Aligned Bounding Box.

use crate::bounding_volume::{BoundingSphere, BoundingVolume};
use crate::math::{Pose, Real, Vector, DIM, TWO_DIM};
use crate::shape::{Cuboid, SupportMap};
use crate::utils::PoseOps;
use arrayvec::ArrayVec;
use num::Bounded;

use crate::query::{Ray, RayCast};

/// An Axis-Aligned Bounding Box (AABB).
///
/// An AABB is the simplest bounding volume, defined by its minimum and maximum corners.
/// It's called "axis-aligned" because its edges are always parallel to the coordinate axes
/// (X, Y, and Z in 3D), making it very fast to test and compute.
///
/// # Structure
///
/// - **mins**: The point with the smallest coordinates on each axis (bottom-left-back corner)
/// - **maxs**: The point with the largest coordinates on each axis (top-right-front corner)
/// - **Invariant**: `mins.x ≤ maxs.x`, `mins.y ≤ maxs.y` (and `mins.z ≤ maxs.z` in 3D)
///
/// # Properties
///
/// - **Axis-aligned**: Edges always parallel to coordinate axes
/// - **Conservative**: May be larger than the actual shape for rotated objects
/// - **Fast**: Intersection tests are very cheap (just coordinate comparisons)
/// - **Hierarchical**: Perfect for spatial data structures (BVH, quadtree, octree)
///
/// # Use Cases
///
/// AABBs are fundamental to collision detection and are used for:
///
/// - **Broad-phase collision detection**: Quickly eliminate distant object pairs
/// - **Spatial partitioning**: Building BVHs, quadtrees, and octrees
/// - **View frustum culling**: Determining what's visible
/// - **Ray tracing acceleration**: Quickly rejecting non-intersecting rays
/// - **Bounding volume for any shape**: Every shape can compute its AABB
///
/// # Performance
///
/// AABBs are the fastest bounding volume for:
/// - Intersection tests: O(1) with just 6 comparisons (3D)
/// - Merging: O(1) with component-wise min/max
/// - Contains test: O(1) with coordinate comparisons
///
/// # Limitations
///
/// - **Rotation invariance**: Must be recomputed when objects rotate
/// - **Tightness**: May waste space for rotated or complex shapes
/// - **No orientation**: Cannot represent oriented bounding boxes (OBB)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// // Create an AABB for a unit cube centered at origin
/// let mins = Vector::new(-0.5, -0.5, -0.5);
/// let maxs = Vector::new(0.5, 0.5, 0.5);
/// let aabb = Aabb::new(mins, maxs);
///
/// // Check if a point is inside
/// let point = Vector::ZERO;
/// assert!(aabb.contains_local_point(point));
///
/// // Get center and extents
/// assert_eq!(aabb.center(), Vector::ZERO);
/// assert_eq!(aabb.extents().x, 1.0); // Full width
/// assert_eq!(aabb.half_extents().x, 0.5); // Half width
/// # }
/// ```
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
///
/// // Create from a set of points
/// let points = vec![
///     Vector::new(1.0, 2.0, 3.0),
///     Vector::new(-1.0, 4.0, 2.0),
///     Vector::new(0.0, 0.0, 5.0),
/// ];
/// let aabb = Aabb::from_points(points);
///
/// // The AABB encloses all points
/// assert_eq!(aabb.mins, Vector::new(-1.0, 0.0, 2.0));
/// assert_eq!(aabb.maxs, Vector::new(1.0, 4.0, 5.0));
/// # }
/// ```
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Debug, PartialEq, Copy, Clone)]
#[repr(C)]
pub struct Aabb {
    /// The point with minimum coordinates (bottom-left-back corner).
    ///
    /// Each component (`x`, `y`, `z`) should be less than or equal to the
    /// corresponding component in `maxs`.
    pub mins: Vector,

    /// The point with maximum coordinates (top-right-front corner).
    ///
    /// Each component (`x`, `y`, `z`) should be greater than or equal to the
    /// corresponding component in `mins`.
    pub maxs: Vector,
}

impl Aabb {
    /// The vertex indices of each edge of this `Aabb`.
    ///
    /// This gives, for each edge of this `Aabb`, the indices of its
    /// vertices when taken from the `self.vertices()` array.
    /// Here is how the faces are numbered, assuming
    /// a right-handed coordinate system:
    ///
    /// ```text
    ///    y             3 - 2
    ///    |           7 − 6 |
    ///    ___ x       |   | 1  (the zero is below 3 and on the left of 1,
    ///   /            4 - 5     hidden by the 4-5-6-7 face.)
    ///  z
    /// ```
    #[cfg(feature = "dim3")]
    pub const EDGES_VERTEX_IDS: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (3, 2),
        (0, 3),
        (4, 5),
        (5, 6),
        (7, 6),
        (4, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];

    /// The vertex indices of each face of this `Aabb`.
    ///
    /// This gives, for each face of this `Aabb`, the indices of its
    /// vertices when taken from the `self.vertices()` array.
    /// Here is how the faces are numbered, assuming
    /// a right-handed coordinate system:
    ///
    /// ```text
    ///    y             3 - 2
    ///    |           7 − 6 |
    ///    ___ x       |   | 1  (the zero is below 3 and on the left of 1,
    ///   /            4 - 5     hidden by the 4-5-6-7 face.)
    ///  z
    /// ```
    #[cfg(feature = "dim3")]
    pub const FACES_VERTEX_IDS: [(usize, usize, usize, usize); 6] = [
        // Face with normal +X
        (1, 2, 6, 5),
        // Face with normal -X
        (0, 3, 7, 4),
        // Face with normal +Y
        (2, 3, 7, 6),
        // Face with normal -Y
        (1, 0, 4, 5),
        // Face with normal +Z
        (4, 5, 6, 7),
        // Face with normal -Z
        (0, 1, 2, 3),
    ];

    /// The vertex indices of each face of this `Aabb`.
    ///
    /// This gives, for each face of this `Aabb`, the indices of its
    /// vertices when taken from the `self.vertices()` array.
    /// Here is how the faces are numbered, assuming
    /// a right-handed coordinate system:
    ///
    /// ```text
    ///    y             3 - 2
    ///    |             |   |
    ///    ___ x         0 - 1
    /// ```
    #[cfg(feature = "dim2")]
    pub const FACES_VERTEX_IDS: [(usize, usize); 4] = [
        // Face with normal +X
        (1, 2),
        // Face with normal -X
        (3, 0),
        // Face with normal +Y
        (2, 3),
        // Face with normal -Y
        (0, 1),
    ];

    /// Creates a new AABB from its minimum and maximum corners.
    ///
    /// # Arguments
    ///
    /// * `mins` - The point with the smallest coordinates on each axis
    /// * `maxs` - The point with the largest coordinates on each axis
    ///
    /// # Invariant
    ///
    /// Each component of `mins` should be ≤ the corresponding component of `maxs`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// // Create a 2x2x2 cube centered at origin
    /// let aabb = Aabb::new(
    ///     Vector::new(-1.0, -1.0, -1.0),
    ///     Vector::new(1.0, 1.0, 1.0)
    /// );
    ///
    /// assert_eq!(aabb.center(), Vector::ZERO);
    /// assert_eq!(aabb.extents(), Vector::new(2.0, 2.0, 2.0));
    /// # }
    /// ```
    #[inline]
    pub fn new(mins: Vector, maxs: Vector) -> Aabb {
        Aabb { mins, maxs }
    }

    /// Creates an invalid AABB with inverted bounds.
    ///
    /// The resulting AABB has `mins` set to maximum values and `maxs` set to
    /// minimum values. This is useful as an initial value for AABB merging
    /// algorithms (similar to starting a min operation with infinity).
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{Aabb, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let mut aabb = Aabb::new_invalid();
    ///
    /// // Merge with actual points to build proper AABB
    /// aabb.merge(&Aabb::new(Vector::new(1.0, 2.0, 3.0), Vector::new(1.0, 2.0, 3.0)));
    /// aabb.merge(&Aabb::new(Vector::new(-1.0, 0.0, 2.0), Vector::new(-1.0, 0.0, 2.0)));
    ///
    /// // Now contains the merged bounds
    /// assert_eq!(aabb.mins, Vector::new(-1.0, 0.0, 2.0));
    /// assert_eq!(aabb.maxs, Vector::new(1.0, 2.0, 3.0));
    /// # }
    /// ```
    #[inline]
    pub fn new_invalid() -> Self {
        Self::new(
            Vector::splat(Real::max_value()),
            Vector::splat(-Real::max_value()),
        )
    }

    /// Creates a new AABB from its center and half-extents.
    ///
    /// This is often more intuitive than specifying min and max corners.
    ///
    /// # Arguments
    ///
    /// * `center` - The center point of the AABB
    /// * `half_extents` - Half the dimensions along each axis
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// // Create a 10x6x8 box centered at (5, 0, 0)
    /// let aabb = Aabb::from_half_extents(
    ///     Vector::new(5.0, 0.0, 0.0),
    ///     Vector::new(5.0, 3.0, 4.0)
    /// );
    ///
    /// assert_eq!(aabb.mins, Vector::new(0.0, -3.0, -4.0));
    /// assert_eq!(aabb.maxs, Vector::new(10.0, 3.0, 4.0));
    /// assert_eq!(aabb.center(), Vector::new(5.0, 0.0, 0.0));
    /// # }
    /// ```
    #[inline]
    pub fn from_half_extents(center: Vector, half_extents: Vector) -> Self {
        Self::new(center - half_extents, center + half_extents)
    }

    /// Creates a new AABB that tightly encloses a set of points (references).
    ///
    /// Computes the minimum and maximum coordinates across all points.
    ///
    /// # Arguments
    ///
    /// * `pts` - An iterator over point references
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let points = vec![
    ///     Vector::new(1.0, 2.0, 3.0),
    ///     Vector::new(-1.0, 4.0, 2.0),
    ///     Vector::new(0.0, 0.0, 5.0),
    /// ];
    ///
    /// let aabb = Aabb::from_points_ref(&points);
    /// assert_eq!(aabb.mins, Vector::new(-1.0, 0.0, 2.0));
    /// assert_eq!(aabb.maxs, Vector::new(1.0, 4.0, 5.0));
    /// # }
    /// ```
    pub fn from_points_ref<'a, I>(pts: I) -> Self
    where
        I: IntoIterator<Item = &'a Vector>,
    {
        super::aabb_utils::local_point_cloud_aabb(pts.into_iter().copied())
    }

    /// Creates a new AABB that tightly encloses a set of points (values).
    ///
    /// Computes the minimum and maximum coordinates across all points.
    ///
    /// # Arguments
    ///
    /// * `pts` - An iterator over point values
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::from_points(vec![
    ///     Vector::new(1.0, 2.0, 3.0),
    ///     Vector::new(-1.0, 4.0, 2.0),
    ///     Vector::new(0.0, 0.0, 5.0),
    /// ]);
    ///
    /// assert_eq!(aabb.mins, Vector::new(-1.0, 0.0, 2.0));
    /// assert_eq!(aabb.maxs, Vector::new(1.0, 4.0, 5.0));
    /// # }
    /// ```
    pub fn from_points<I>(pts: I) -> Self
    where
        I: IntoIterator<Item = Vector>,
    {
        super::aabb_utils::local_point_cloud_aabb(pts)
    }

    /// Returns the center point of this AABB.
    ///
    /// The center is the midpoint between `mins` and `maxs`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(
    ///     Vector::new(-2.0, -3.0, -4.0),
    ///     Vector::new(2.0, 3.0, 4.0)
    /// );
    ///
    /// assert_eq!(aabb.center(), Vector::ZERO);
    /// # }
    /// ```
    #[inline]
    pub fn center(&self) -> Vector {
        self.mins.midpoint(self.maxs)
    }

    /// Returns the half-extents of this AABB.
    ///
    /// Half-extents are half the dimensions along each axis.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabb = Aabb::new(
    ///     Vector::new(-5.0, -3.0, -2.0),
    ///     Vector::new(5.0, 3.0, 2.0)
    /// );
    ///
    /// let half = aabb.half_extents();
    /// assert_eq!(half, Vector::new(5.0, 3.0, 2.0));
    ///
    /// // Full dimensions are 2 * half_extents
    /// let full = aabb.extents();
    /// assert_eq!(full, Vector::new(10.0, 6.0, 4.0));
    /// # }
    /// ```
    #[inline]
    pub fn half_extents(&self) -> Vector {
        let half: Real = 0.5;
        (self.maxs - self.mins) * half
    }

    /// Returns the volume of this AABB.
    ///
    /// - **2D**: Returns the area (width × height)
    /// - **3D**: Returns the volume (width × height × depth)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// // A 2x3x4 box
    /// let aabb = Aabb::new(
    ///     Vector::ZERO,
    ///     Vector::new(2.0, 3.0, 4.0)
    /// );
    ///
    /// assert_eq!(aabb.volume(), 24.0); // 2 * 3 * 4
    /// # }
    /// ```
    #[inline]
    pub fn volume(&self) -> Real {
        let extents = self.extents();
        #[cfg(feature = "dim2")]
        return extents.x * extents.y;
        #[cfg(feature = "dim3")]
        return extents.x * extents.y * extents.z;
    }

    /// In 3D, returns the half-area. In 2D returns the half-perimeter of the AABB.
    pub fn half_area_or_perimeter(&self) -> Real {
        #[cfg(feature = "dim2")]
        return self.half_perimeter();
        #[cfg(feature = "dim3")]
        return self.half_area();
    }

    /// The half perimeter of this `Aabb`.
    #[cfg(feature = "dim2")]
    pub fn half_perimeter(&self) -> Real {
        let extents = self.extents();
        extents.x + extents.y
    }

    /// The half area of this `Aabb`.
    #[cfg(feature = "dim3")]
    pub fn half_area(&self) -> Real {
        let extents = self.extents();
        extents.x * (extents.y + extents.z) + extents.y * extents.z
    }

    /// The extents of this `Aabb`.
    #[inline]
    pub fn extents(&self) -> Vector {
        self.maxs - self.mins
    }

    /// Enlarges this `Aabb` so it also contains the point `pt`.
    pub fn take_point(&mut self, pt: Vector) {
        self.mins = self.mins.min(pt);
        self.maxs = self.maxs.max(pt);
    }

    /// Computes the `Aabb` bounding `self` transformed by `m`.
    #[inline]
    pub fn transform_by(&self, m: &Pose) -> Self {
        let ls_center = self.center();
        let center = m * ls_center;
        let ws_half_extents = m.absolute_transform_vector(self.half_extents());

        Aabb::new(center + (-ws_half_extents), center + ws_half_extents)
    }

    /// Computes the Aabb bounding `self` translated by `translation`.
    #[inline]
    pub fn translated(mut self, translation: Vector) -> Self {
        self.mins += translation;
        self.maxs += translation;
        self
    }

    #[inline]
    pub fn scaled(self, scale: Vector) -> Self {
        let a = self.mins * scale;
        let b = self.maxs * scale;
        Self {
            mins: a.min(b),
            maxs: a.max(b),
        }
    }

    /// Returns an AABB with the same center as `self` but with extents scaled by `scale`.
    ///
    /// # Parameters
    /// - `scale`: the scaling factor. It can be non-uniform and/or negative. The AABB being
    ///   symmetric wrt. its center, a negative scale value has the same effect as scaling
    ///   by its absolute value.
    #[inline]
    #[must_use]
    pub fn scaled_wrt_center(self, scale: Vector) -> Self {
        let center = self.center();
        // Multiply the extents by the scale. Negative scaling might modify the half-extent
        // sign, so we take the absolute value. The AABB being symmetric that absolute value
        // is  valid.
        let half_extents = self.half_extents() * scale.abs();
        Self::from_half_extents(center, half_extents)
    }

    /// The smallest bounding sphere containing this `Aabb`.
    #[inline]
    pub fn bounding_sphere(&self) -> BoundingSphere {
        let center = self.center();
        let radius = self.mins.distance(self.maxs) * 0.5;
        BoundingSphere::new(center, radius)
    }

    /// Does this AABB contains a point expressed in the same coordinate frame as `self`?
    #[inline]
    pub fn contains_local_point(&self, point: Vector) -> bool {
        for i in 0..DIM {
            if point[i] < self.mins[i] || point[i] > self.maxs[i] {
                return false;
            }
        }

        true
    }

    /// Computes the distance between the origin and this AABB.
    pub fn distance_to_origin(&self) -> Real {
        self.mins.max(-self.maxs).max(Vector::ZERO).length()
    }

    /// Does this AABB intersects an AABB `aabb2` moving at velocity `vel12` relative to `self`?
    #[inline]
    pub fn intersects_moving_aabb(&self, aabb2: &Self, vel12: Vector) -> bool {
        // Minkowski sum.
        let msum = Aabb {
            mins: self.mins - aabb2.maxs,
            maxs: self.maxs - aabb2.mins,
        };
        let ray = Ray::new(Vector::ZERO, vel12);

        msum.intersects_local_ray(&ray, 1.0)
    }

    /// Computes the intersection of this `Aabb` and another one.
    pub fn intersection(&self, other: &Aabb) -> Option<Aabb> {
        let result = Aabb {
            mins: self.mins.max(other.mins),
            maxs: self.maxs.min(other.maxs),
        };

        for i in 0..DIM {
            if result.mins[i] > result.maxs[i] {
                return None;
            }
        }

        Some(result)
    }

    /// Computes two AABBs for the intersection between two translated and rotated AABBs.
    ///
    /// This method returns two AABBs: the first is expressed in the local-space of `self`,
    /// and the second is expressed in the local-space of `aabb2`.
    pub fn aligned_intersections(&self, pos12: &Pose, aabb2: &Self) -> Option<(Aabb, Aabb)> {
        let pos21 = pos12.inverse();

        let aabb2_1 = aabb2.transform_by(pos12);
        let inter1_1 = self.intersection(&aabb2_1)?;
        let inter1_2 = inter1_1.transform_by(&pos21);

        let aabb1_2 = self.transform_by(&pos21);
        let inter2_2 = aabb2.intersection(&aabb1_2)?;
        let inter2_1 = inter2_2.transform_by(pos12);

        Some((
            inter1_1.intersection(&inter2_1)?,
            inter1_2.intersection(&inter2_2)?,
        ))
    }

    /// Returns the difference between this `Aabb` and `rhs`.
    ///
    /// Removing another `Aabb` from `self` will result in zero, one, or up to 4 (in 2D) or 8 (in 3D)
    /// new smaller Aabbs.
    pub fn difference(&self, rhs: &Aabb) -> ArrayVec<Self, TWO_DIM> {
        self.difference_with_cut_sequence(rhs).0
    }

    /// Returns the difference between this `Aabb` and `rhs`.
    ///
    /// Removing another `Aabb` from `self` will result in zero, one, or up to 4 (in 2D) or 8 (in 3D)
    /// new smaller Aabbs.
    ///
    /// # Return
    /// This returns a pair where the first item are the new Aabbs and the second item is
    /// the sequence of cuts applied to `self` to obtain the new Aabbs. Each cut is performed
    /// along one axis identified by `-1, -2, -3` for `-X, -Y, -Z` and `1, 2, 3` for `+X, +Y, +Z`, and
    /// the plane’s bias.
    ///
    /// The cuts are applied sequentially. For example, if `result.1[0]` contains `1`, then it means
    /// that `result.0[0]` is equal to the piece of `self` lying in the negative half-space delimited
    /// by the plane with outward normal `+X`. Then, the other piece of `self` generated by this cut
    /// (i.e. the piece of `self` lying in the positive half-space delimited by the plane with outward
    /// normal `+X`) is the one that will be affected by the next cut.
    ///
    /// The returned cut sequence will be empty if the aabbs are disjoint.
    pub fn difference_with_cut_sequence(
        &self,
        rhs: &Aabb,
    ) -> (ArrayVec<Self, TWO_DIM>, ArrayVec<(i8, Real), TWO_DIM>) {
        let mut result = ArrayVec::new();
        let mut cut_sequence = ArrayVec::new();

        // NOTE: special case when the boxes are disjoint.
        //       This isn’t exactly the same as `!self.intersects(rhs)`
        //       because of the equality.
        for i in 0..DIM {
            if self.mins[i] >= rhs.maxs[i] || self.maxs[i] <= rhs.mins[i] {
                result.push(*self);
                return (result, cut_sequence);
            }
        }

        let mut rest = *self;

        for i in 0..DIM {
            if rhs.mins[i] > rest.mins[i] {
                let mut fragment = rest;
                fragment.maxs[i] = rhs.mins[i];
                rest.mins[i] = rhs.mins[i];
                result.push(fragment);
                cut_sequence.push((i as i8 + 1, rhs.mins[i]));
            }

            if rhs.maxs[i] < rest.maxs[i] {
                let mut fragment = rest;
                fragment.mins[i] = rhs.maxs[i];
                rest.maxs[i] = rhs.maxs[i];
                result.push(fragment);
                cut_sequence.push((-(i as i8 + 1), -rhs.maxs[i]));
            }
        }

        (result, cut_sequence)
    }

    /// Computes the vertices of this `Aabb`.
    ///
    /// The vertices are given in the following order in a right-handed coordinate system:
    /// ```text
    ///    y             3 - 2
    ///    |             |   |
    ///    ___ x         0 - 1
    /// ```
    #[inline]
    #[cfg(feature = "dim2")]
    pub fn vertices(&self) -> [Vector; 4] {
        [
            Vector::new(self.mins.x, self.mins.y),
            Vector::new(self.maxs.x, self.mins.y),
            Vector::new(self.maxs.x, self.maxs.y),
            Vector::new(self.mins.x, self.maxs.y),
        ]
    }

    /// Computes the vertices of this `Aabb`.
    ///
    /// The vertices are given in the following order, in a right-handed coordinate system:
    /// ```text
    ///    y             3 - 2
    ///    |           7 − 6 |
    ///    ___ x       |   | 1  (the zero is below 3 and on the left of 1,
    ///   /            4 - 5     hidden by the 4-5-6-7 face.)
    ///  z
    /// ```
    #[inline]
    #[cfg(feature = "dim3")]
    pub fn vertices(&self) -> [Vector; 8] {
        [
            Vector::new(self.mins.x, self.mins.y, self.mins.z),
            Vector::new(self.maxs.x, self.mins.y, self.mins.z),
            Vector::new(self.maxs.x, self.maxs.y, self.mins.z),
            Vector::new(self.mins.x, self.maxs.y, self.mins.z),
            Vector::new(self.mins.x, self.mins.y, self.maxs.z),
            Vector::new(self.maxs.x, self.mins.y, self.maxs.z),
            Vector::new(self.maxs.x, self.maxs.y, self.maxs.z),
            Vector::new(self.mins.x, self.maxs.y, self.maxs.z),
        ]
    }

    /// Splits this `Aabb` at its center, into four parts (as in a quad-tree).
    #[inline]
    #[cfg(feature = "dim2")]
    pub fn split_at_center(&self) -> [Aabb; 4] {
        let center = self.center();

        [
            Aabb::new(self.mins, center),
            Aabb::new(
                Vector::new(center.x, self.mins.y),
                Vector::new(self.maxs.x, center.y),
            ),
            Aabb::new(center, self.maxs),
            Aabb::new(
                Vector::new(self.mins.x, center.y),
                Vector::new(center.x, self.maxs.y),
            ),
        ]
    }

    /// Splits this `Aabb` at its center, into eight parts (as in an octree).
    #[inline]
    #[cfg(feature = "dim3")]
    pub fn split_at_center(&self) -> [Aabb; 8] {
        let center = self.center();

        [
            Aabb::new(
                Vector::new(self.mins.x, self.mins.y, self.mins.z),
                Vector::new(center.x, center.y, center.z),
            ),
            Aabb::new(
                Vector::new(center.x, self.mins.y, self.mins.z),
                Vector::new(self.maxs.x, center.y, center.z),
            ),
            Aabb::new(
                Vector::new(center.x, center.y, self.mins.z),
                Vector::new(self.maxs.x, self.maxs.y, center.z),
            ),
            Aabb::new(
                Vector::new(self.mins.x, center.y, self.mins.z),
                Vector::new(center.x, self.maxs.y, center.z),
            ),
            Aabb::new(
                Vector::new(self.mins.x, self.mins.y, center.z),
                Vector::new(center.x, center.y, self.maxs.z),
            ),
            Aabb::new(
                Vector::new(center.x, self.mins.y, center.z),
                Vector::new(self.maxs.x, center.y, self.maxs.z),
            ),
            Aabb::new(
                Vector::new(center.x, center.y, center.z),
                Vector::new(self.maxs.x, self.maxs.y, self.maxs.z),
            ),
            Aabb::new(
                Vector::new(self.mins.x, center.y, center.z),
                Vector::new(center.x, self.maxs.y, self.maxs.z),
            ),
        ]
    }

    /// Enlarges this AABB on each side by the given `half_extents`.
    #[must_use]
    pub fn add_half_extents(&self, half_extents: Vector) -> Self {
        Self {
            mins: self.mins - half_extents,
            maxs: self.maxs + half_extents,
        }
    }

    /// Projects every point of `Aabb` on an arbitrary axis.
    pub fn project_on_axis(&self, axis: Vector) -> (Real, Real) {
        let cuboid = Cuboid::new(self.half_extents());
        let shift = cuboid.local_support_point_toward(axis).dot(axis).abs();
        let center = self.center().dot(axis);
        (center - shift, center + shift)
    }

    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    pub fn intersects_spiral(
        &self,
        point: Vector,
        spiral_center: Vector,
        axis: Vector,
        linvel: Vector,
        angvel: Real,
    ) -> bool {
        use crate::math::{Matrix2, Vector2};
        use crate::utils::WBasis;
        use crate::utils::{Interval, IntervalFunction};
        use alloc::vec;

        struct SpiralPlaneDistance {
            spiral_center: Vector,
            tangents: [Vector; 2],
            linvel: Vector,
            angvel: Real,
            point: Vector2,
            plane: Vector,
            bias: Real,
        }

        impl SpiralPlaneDistance {
            fn spiral_pt_at(&self, t: Real) -> Vector {
                let angle = t * self.angvel;

                // NOTE: we construct the rotation matrix explicitly here instead
                //       of using `Rotation2::new()` because we will use similar
                //       formulas on the interval methods.
                let (sin, cos) = <Real as simba::scalar::ComplexField>::sin_cos(angle);
                // Mat2 in column-major: first column is [cos, sin], second is [-sin, cos]
                let rotmat = Matrix2::from_cols(Vector2::new(cos, sin), Vector2::new(-sin, cos));

                let rotated_pt = rotmat * self.point;
                let shift = self.tangents[0] * rotated_pt.x + self.tangents[1] * rotated_pt.y;
                self.spiral_center + self.linvel * t + shift
            }
        }

        impl IntervalFunction<Real> for SpiralPlaneDistance {
            fn eval(&self, t: Real) -> Real {
                let point_pos = self.spiral_pt_at(t);
                point_pos.dot(self.plane) - self.bias
            }

            fn eval_interval(&self, t: Interval<Real>) -> Interval<Real> {
                // This is the same as `self.eval` except that `t` is an interval.
                let angle = t * self.angvel;
                let (sin, cos) = angle.sin_cos();

                // Compute rotated point manually with interval arithmetic
                let rotated_pt_x =
                    cos * Interval::splat(self.point.x) - sin * Interval::splat(self.point.y);
                let rotated_pt_y =
                    sin * Interval::splat(self.point.x) + cos * Interval::splat(self.point.y);

                let shift_x = Interval::splat(self.tangents[0].x) * rotated_pt_x
                    + Interval::splat(self.tangents[1].x) * rotated_pt_y;
                let shift_y = Interval::splat(self.tangents[0].y) * rotated_pt_x
                    + Interval::splat(self.tangents[1].y) * rotated_pt_y;
                let shift_z = Interval::splat(self.tangents[0].z) * rotated_pt_x
                    + Interval::splat(self.tangents[1].z) * rotated_pt_y;

                let point_pos_x = Interval::splat(self.spiral_center.x)
                    + Interval::splat(self.linvel.x) * t
                    + shift_x;
                let point_pos_y = Interval::splat(self.spiral_center.y)
                    + Interval::splat(self.linvel.y) * t
                    + shift_y;
                let point_pos_z = Interval::splat(self.spiral_center.z)
                    + Interval::splat(self.linvel.z) * t
                    + shift_z;

                point_pos_x * Interval::splat(self.plane.x)
                    + point_pos_y * Interval::splat(self.plane.y)
                    + point_pos_z * Interval::splat(self.plane.z)
                    - Interval::splat(self.bias)
            }

            fn eval_interval_gradient(&self, t: Interval<Real>) -> Interval<Real> {
                let angle = t * self.angvel;
                let (sin, cos) = angle.sin_cos();
                let angvel_interval = Interval::splat(self.angvel);

                // Derivative of rotation matrix applied to point
                let rotated_pt_x = (-sin * angvel_interval) * Interval::splat(self.point.x)
                    - (cos * angvel_interval) * Interval::splat(self.point.y);
                let rotated_pt_y = (cos * angvel_interval) * Interval::splat(self.point.x)
                    + (-sin * angvel_interval) * Interval::splat(self.point.y);

                let shift_x = Interval::splat(self.tangents[0].x) * rotated_pt_x
                    + Interval::splat(self.tangents[1].x) * rotated_pt_y;
                let shift_y = Interval::splat(self.tangents[0].y) * rotated_pt_x
                    + Interval::splat(self.tangents[1].y) * rotated_pt_y;
                let shift_z = Interval::splat(self.tangents[0].z) * rotated_pt_x
                    + Interval::splat(self.tangents[1].z) * rotated_pt_y;

                let point_vel_x = shift_x + Interval::splat(self.linvel.x);
                let point_vel_y = shift_y + Interval::splat(self.linvel.y);
                let point_vel_z = shift_z + Interval::splat(self.linvel.z);

                point_vel_x * Interval::splat(self.plane.x)
                    + point_vel_y * Interval::splat(self.plane.y)
                    + point_vel_z * Interval::splat(self.plane.z)
            }
        }

        let tangents = axis.orthonormal_basis();
        let dpos = point - spiral_center;
        #[cfg(feature = "f32")]
        let spiral_point = glamx::Vec2::new(dpos.dot(tangents[0]), dpos.dot(tangents[1]));
        #[cfg(feature = "f64")]
        let spiral_point = glamx::DVec2::new(dpos.dot(tangents[0]), dpos.dot(tangents[1]));
        let mut distance_fn = SpiralPlaneDistance {
            spiral_center,
            tangents,
            linvel,
            angvel,
            point: spiral_point,
            plane: Vector::X,
            bias: 0.0,
        };

        // Check the 8 planar faces of the Aabb.
        let mut roots = vec![];
        let mut candidates = vec![];

        let planes = [
            (-self.mins[0], -Vector::X, 0),
            (self.maxs[0], Vector::X, 0),
            (-self.mins[1], -Vector::Y, 1),
            (self.maxs[1], Vector::Y, 1),
            (-self.mins[2], -Vector::Z, 2),
            (self.maxs[2], Vector::Z, 2),
        ];

        let range = self.project_on_axis(axis);
        let range_bias = spiral_center.dot(axis);
        let interval = Interval::sort(range.0, range.1) - range_bias;

        for (bias, axis, i) in &planes {
            distance_fn.plane = *axis;
            distance_fn.bias = *bias;

            crate::utils::find_root_intervals_to(
                &distance_fn,
                interval,
                1.0e-5,
                1.0e-5,
                100,
                &mut roots,
                &mut candidates,
            );

            for root in roots.drain(..) {
                let point = distance_fn.spiral_pt_at(root.midpoint());
                let (j, k) = ((i + 1) % 3, (i + 2) % 3);
                if point[j] >= self.mins[j]
                    && point[j] <= self.maxs[j]
                    && point[k] >= self.mins[k]
                    && point[k] <= self.maxs[k]
                {
                    return true;
                }
            }
        }

        false
    }
}

impl BoundingVolume for Aabb {
    #[inline]
    fn center(&self) -> Vector {
        self.center()
    }

    #[inline]
    fn intersects(&self, other: &Aabb) -> bool {
        (self.mins.cmple(other.maxs) & self.maxs.cmpge(other.mins)).all()
    }

    #[inline]
    fn contains(&self, other: &Aabb) -> bool {
        (self.mins.cmple(other.mins) & self.maxs.cmpge(other.maxs)).all()
    }

    #[inline]
    fn merge(&mut self, other: &Aabb) {
        self.mins = self.mins.min(other.mins);
        self.maxs = self.maxs.max(other.maxs);
    }

    #[inline]
    fn merged(&self, other: &Aabb) -> Aabb {
        Aabb {
            mins: self.mins.min(other.mins),
            maxs: self.maxs.max(other.maxs),
        }
    }

    #[inline]
    fn loosen(&mut self, amount: Real) {
        assert!(amount >= 0.0, "The loosening margin must be positive.");
        self.mins += Vector::splat(-amount);
        self.maxs += Vector::splat(amount);
    }

    #[inline]
    fn loosened(&self, amount: Real) -> Aabb {
        assert!(amount >= 0.0, "The loosening margin must be positive.");
        Aabb {
            mins: self.mins + Vector::splat(-amount),
            maxs: self.maxs + Vector::splat(amount),
        }
    }

    #[inline]
    fn tighten(&mut self, amount: Real) {
        assert!(amount >= 0.0, "The tightening margin must be positive.");
        self.mins += Vector::splat(amount);
        self.maxs += Vector::splat(-amount);
        assert!(
            self.mins.cmple(self.maxs).all(),
            "The tightening margin is to large."
        );
    }

    #[inline]
    fn tightened(&self, amount: Real) -> Aabb {
        assert!(amount >= 0.0, "The tightening margin must be positive.");

        Aabb::new(
            self.mins + Vector::splat(amount),
            self.maxs + Vector::splat(-amount),
        )
    }
}
