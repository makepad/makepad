use super::TriMeshBuilderError;
use crate::math::{IVector, Pose, Real, Vector, DIM};
#[cfg(feature = "dim2")]
use crate::shape::ConvexPolygon;
#[cfg(feature = "serde-serialize")]
use crate::shape::DeserializableTypedShape;
#[cfg(feature = "dim3")]
use crate::shape::HeightFieldFlags;
use crate::shape::{
    Ball, Capsule, Compound, Cuboid, HalfSpace, HeightField, Polyline, RoundShape, Segment, Shape,
    TriMesh, TriMeshFlags, Triangle, TypedShape, Voxels,
};
#[cfg(feature = "dim3")]
use crate::shape::{Cone, ConvexPolyhedron, Cylinder};
use crate::transformation::vhacd::{VHACDParameters, VHACD};
use crate::transformation::voxelization::{FillMode, VoxelSet};
#[cfg(feature = "dim3")]
use crate::utils::Array2;
use alloc::sync::Arc;
use alloc::{vec, vec::Vec};
use core::fmt;
use core::ops::Deref;

/// A reference-counted, shareable geometric shape.
///
/// `SharedShape` is a wrapper around [`Arc<dyn Shape>`] that allows multiple parts of your
/// code to share ownership of the same shape without copying it. This is particularly useful
/// when the same geometric shape is used by multiple colliders or when you want to avoid
/// the memory overhead of duplicating complex shapes like triangle meshes.
///
/// # Why use SharedShape?
///
/// - **Memory efficiency**: Share expensive shapes (like [`TriMesh`] or [`HeightField`]) across multiple colliders
/// - **Performance**: Cloning a `SharedShape` only increments a reference count, not the shape data
/// - **Type erasure**: Store different shape types uniformly via the [`Shape`] trait
/// - **Flexibility**: Can be converted to a unique instance via [`make_mut`](Self::make_mut) when needed
///
/// # When to use SharedShape?
///
/// Use `SharedShape` when:
/// - You need multiple colliders with the same geometry
/// - You're working with large, complex shapes (meshes, compounds, heightfields)
/// - You want to store heterogeneous shape types in a collection
///
/// Use concrete shape types directly when:
/// - You have a single, simple shape that won't be shared
/// - You need direct access to shape-specific methods
///
/// # Examples
///
/// Creating and sharing a ball shape:
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::shape::SharedShape;
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::shape::SharedShape;
///
/// // Create a ball with radius 1.0
/// let shape = SharedShape::ball(1.0);
///
/// // Clone it cheaply - only the Arc is cloned, not the shape data
/// let shape_clone = shape.clone();
///
/// // Both shapes reference the same underlying ball
/// assert_eq!(shape.as_ball().unwrap().radius, 1.0);
/// assert_eq!(shape_clone.as_ball().unwrap().radius, 1.0);
/// # }
/// # }
/// ```
#[derive(Clone)]
pub struct SharedShape(pub Arc<dyn Shape>);

impl Deref for SharedShape {
    type Target = dyn Shape;
    fn deref(&self) -> &dyn Shape {
        &*self.0
    }
}

impl AsRef<dyn Shape> for SharedShape {
    fn as_ref(&self) -> &dyn Shape {
        &*self.0
    }
}

impl fmt::Debug for SharedShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let typed_shape: TypedShape = (*self.0).as_typed_shape();
        write!(f, "SharedShape ( Arc<{typed_shape:?}> )")
    }
}

impl SharedShape {
    /// Wraps any shape type into a `SharedShape`.
    ///
    /// This constructor accepts any type that implements the [`Shape`] trait and wraps it
    /// in an `Arc` for shared ownership.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{SharedShape, Ball};
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::{SharedShape, Ball};
    ///
    /// let ball = Ball::new(1.0);
    /// let shared = SharedShape::new(ball);
    /// # }
    /// # }
    /// ```
    pub fn new(shape: impl Shape) -> Self {
        Self(Arc::new(shape))
    }

    /// Returns a mutable reference to the underlying shape, cloning it if necessary.
    ///
    /// This method implements copy-on-write semantics. If the `Arc` has multiple references,
    /// it will clone the shape data to create a unique instance. Otherwise, no cloning occurs.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::SharedShape;
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::SharedShape;
    ///
    /// let mut shape1 = SharedShape::ball(1.0);
    /// let shape2 = shape1.clone(); // Shares the same Arc
    ///
    /// // This will clone the shape because it's shared
    /// let ball = shape1.make_mut().as_ball_mut().unwrap();
    /// ball.radius = 2.0;
    ///
    /// // shape1 has been modified, shape2 still has the original value
    /// assert_eq!(shape1.as_ball().unwrap().radius, 2.0);
    /// assert_eq!(shape2.as_ball().unwrap().radius, 1.0);
    /// # }
    /// # }
    /// ```
    pub fn make_mut(&mut self) -> &mut dyn Shape {
        if Arc::get_mut(&mut self.0).is_none() {
            let unique_self = self.0.clone_dyn();
            self.0 = unique_self.into();
        }
        Arc::get_mut(&mut self.0).unwrap()
    }

    /// Creates a compound shape made of multiple subshapes.
    ///
    /// Each subshape has its own position and orientation relative to the compound's origin.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::SharedShape;
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::math::Pose;
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::SharedShape;
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::math::Pose;
    ///
    /// let ball1 = SharedShape::ball(0.5);
    /// let ball2 = SharedShape::ball(0.5);
    ///
    /// #[cfg(feature = "dim3")]
    /// let compound = SharedShape::compound(vec![
    ///     (Pose::translation(1.0, 0.0, 0.0), ball1),
    ///     (Pose::translation(-1.0, 0.0, 0.0), ball2),
    /// ]);
    ///
    /// #[cfg(feature = "dim2")]
    /// let compound = SharedShape::compound(vec![
    ///     (Pose::translation(1.0, 0.0), ball1),
    ///     (Pose::translation(-1.0, 0.0), ball2),
    /// ]);
    /// # }
    /// # }
    /// # }
    /// # }
    /// ```
    pub fn compound(shapes: Vec<(Pose, SharedShape)>) -> Self {
        let raw_shapes = shapes.into_iter().map(|s| (s.0, s.1)).collect();
        let compound = Compound::new(raw_shapes);
        SharedShape(Arc::new(compound))
    }

    /// Creates a ball (sphere in 3D, circle in 2D) with the specified radius.
    ///
    /// A ball is the simplest and most efficient collision shape.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::SharedShape;
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::SharedShape;
    ///
    /// let ball = SharedShape::ball(1.0);
    /// assert_eq!(ball.as_ball().unwrap().radius, 1.0);
    /// # }
    /// # }
    /// ```
    pub fn ball(radius: Real) -> Self {
        SharedShape(Arc::new(Ball::new(radius)))
    }

    /// Initialize a plane shape defined by its outward normal.
    pub fn halfspace(outward_normal: Vector) -> Self {
        SharedShape(Arc::new(HalfSpace::new(outward_normal)))
    }

    /// Initialize a cylindrical shape defined by its half-height
    /// (along the y axis) and its radius.
    #[cfg(feature = "dim3")]
    pub fn cylinder(half_height: Real, radius: Real) -> Self {
        SharedShape(Arc::new(Cylinder::new(half_height, radius)))
    }

    /// Initialize a rounded cylindrical shape defined by its half-height
    /// (along along the y axis), its radius, and its roundedness (the
    /// radius of the sphere used for dilating the cylinder).
    #[cfg(feature = "dim3")]
    pub fn round_cylinder(half_height: Real, radius: Real, border_radius: Real) -> Self {
        SharedShape(Arc::new(RoundShape {
            inner_shape: Cylinder::new(half_height, radius),
            border_radius,
        }))
    }

    /// Initialize a rounded cone shape defined by its half-height
    /// (along along the y axis), its radius, and its roundedness (the
    /// radius of the sphere used for dilating the cylinder).
    #[cfg(feature = "dim3")]
    pub fn round_cone(half_height: Real, radius: Real, border_radius: Real) -> Self {
        SharedShape(Arc::new(RoundShape {
            inner_shape: Cone::new(half_height, radius),
            border_radius,
        }))
    }

    /// Initialize a cone shape defined by its half-height
    /// (along along the y axis) and its basis radius.
    #[cfg(feature = "dim3")]
    pub fn cone(half_height: Real, radius: Real) -> Self {
        SharedShape(Arc::new(Cone::new(half_height, radius)))
    }

    /// Initialize a cuboid shape defined by its half-extents.
    #[cfg(feature = "dim2")]
    pub fn cuboid(hx: Real, hy: Real) -> Self {
        SharedShape(Arc::new(Cuboid::new(Vector::new(hx, hy))))
    }

    /// Initialize a round cuboid shape defined by its half-extents and border radius.
    #[cfg(feature = "dim2")]
    pub fn round_cuboid(hx: Real, hy: Real, border_radius: Real) -> Self {
        SharedShape(Arc::new(RoundShape {
            inner_shape: Cuboid::new(Vector::new(hx, hy)),
            border_radius,
        }))
    }

    /// Creates a cuboid (box) with specified half-extents.
    ///
    /// In 3D: creates a box defined by half-extents along X, Y, and Z axes.
    /// In 2D: creates a rectangle defined by half-extents along X and Y axes.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::SharedShape;
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::SharedShape;
    ///
    /// #[cfg(feature = "dim3")]
    /// let cuboid = SharedShape::cuboid(1.0, 2.0, 3.0); // Box with dimensions 2x4x6
    /// #[cfg(feature = "dim2")]
    /// let cuboid = SharedShape::cuboid(1.0, 2.0); // Rectangle with dimensions 2x4
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "dim3")]
    pub fn cuboid(hx: Real, hy: Real, hz: Real) -> Self {
        SharedShape(Arc::new(Cuboid::new(Vector::new(hx, hy, hz))))
    }

    /// Initialize a round cuboid shape defined by its half-extents and border radius.
    #[cfg(feature = "dim3")]
    pub fn round_cuboid(hx: Real, hy: Real, hz: Real, border_radius: Real) -> Self {
        SharedShape(Arc::new(RoundShape {
            inner_shape: Cuboid::new(Vector::new(hx, hy, hz)),
            border_radius,
        }))
    }

    /// Initialize a capsule shape from its endpoints and radius.
    pub fn capsule(a: Vector, b: Vector, radius: Real) -> Self {
        SharedShape(Arc::new(Capsule::new(a, b, radius)))
    }

    /// Initialize a capsule shape aligned with the `x` axis.
    pub fn capsule_x(half_height: Real, radius: Real) -> Self {
        let p = Vector::X * half_height;
        Self::capsule(-p, p, radius)
    }

    /// Creates a capsule aligned with the Y axis.
    ///
    /// A capsule is a line segment with rounded ends, commonly used for character controllers.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::SharedShape;
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::SharedShape;
    ///
    /// // Create a character capsule: 1.8 units tall with 0.3 radius
    /// let character = SharedShape::capsule_y(0.9, 0.3);
    /// # }
    /// # }
    /// ```
    pub fn capsule_y(half_height: Real, radius: Real) -> Self {
        let p = Vector::Y * half_height;
        Self::capsule(-p, p, radius)
    }

    /// Initialize a capsule shape aligned with the `z` axis.
    #[cfg(feature = "dim3")]
    pub fn capsule_z(half_height: Real, radius: Real) -> Self {
        let p = Vector::Z * half_height;
        Self::capsule(-p, p, radius)
    }

    /// Initialize a segment shape from its endpoints.
    pub fn segment(a: Vector, b: Vector) -> Self {
        SharedShape(Arc::new(Segment::new(a, b)))
    }

    /// Initializes a triangle shape.
    pub fn triangle(a: Vector, b: Vector, c: Vector) -> Self {
        SharedShape(Arc::new(Triangle::new(a, b, c)))
    }
    /// Initializes a triangle shape with round corners.
    pub fn round_triangle(a: Vector, b: Vector, c: Vector, border_radius: Real) -> Self {
        SharedShape(Arc::new(RoundShape {
            inner_shape: Triangle::new(a, b, c),
            border_radius,
        }))
    }

    /// Initializes a polyline shape defined by its vertex and index buffers.
    ///
    /// If no index buffer is provided, the polyline is assumed to describe a line strip.
    pub fn polyline(vertices: Vec<Vector>, indices: Option<Vec<[u32; 2]>>) -> Self {
        SharedShape(Arc::new(Polyline::new(vertices, indices)))
    }

    /// Creates a triangle mesh shape from vertices and triangle indices.
    ///
    /// A triangle mesh represents a complex surface as a collection of triangles, useful for
    /// terrain, static geometry, or any shape that can't be represented by simpler primitives.
    ///
    /// # Returns
    ///
    /// Returns `Ok(SharedShape)` if the mesh is valid, or an error if indices are out of bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::SharedShape;
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::math::Vector;
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::SharedShape;
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::math::Vector;
    ///
    /// #[cfg(feature = "dim3")]
    /// let vertices = vec![
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// #[cfg(feature = "dim3")]
    /// let indices = vec![[0, 1, 2]];
    /// #[cfg(feature = "dim3")]
    /// let mesh = SharedShape::trimesh(vertices, indices).unwrap();
    /// # }
    /// # }
    /// # }
    /// # }
    /// ```
    pub fn trimesh(
        vertices: Vec<Vector>,
        indices: Vec<[u32; 3]>,
    ) -> Result<Self, TriMeshBuilderError> {
        Ok(SharedShape(Arc::new(TriMesh::new(vertices, indices)?)))
    }

    /// Initializes a triangle mesh shape defined by its vertex and index buffers and
    /// pre-processing flags.
    pub fn trimesh_with_flags(
        vertices: Vec<Vector>,
        indices: Vec<[u32; 3]>,
        flags: TriMeshFlags,
    ) -> Result<Self, TriMeshBuilderError> {
        Ok(SharedShape(Arc::new(TriMesh::with_flags(
            vertices, indices, flags,
        )?)))
    }

    /// Initializes a shape made of voxels.
    ///
    /// Each voxel has the size `voxel_size` and grid coordinate given by `grid_coords`.
    /// The `primitive_geometry` controls the behavior of collision detection at voxels boundaries.
    ///
    /// For initializing a voxels shape from points in space, see [`Self::voxels_from_points`].
    /// For initializing a voxels shape from a mesh to voxelize, see [`Self::voxelized_mesh`].
    /// For initializing multiple voxels shape from the convex decomposition of a mesh, see
    /// [`Self::voxelized_convex_decomposition`].
    pub fn voxels(voxel_size: Vector, grid_coords: &[IVector]) -> Self {
        let shape = Voxels::new(voxel_size, grid_coords);
        SharedShape::new(shape)
    }

    /// Initializes a shape made of voxels.
    ///
    /// Each voxel has the size `voxel_size` and contains at least one point from `centers`.
    /// The `primitive_geometry` controls the behavior of collision detection at voxels boundaries.
    pub fn voxels_from_points(voxel_size: Vector, points: &[Vector]) -> Self {
        let shape = Voxels::from_points(voxel_size, points);
        SharedShape::new(shape)
    }

    /// Initializes a voxels shape obtained from the decomposition of the given trimesh (in 3D)
    /// or polyline (in 2D) into voxelized convex parts.
    pub fn voxelized_mesh(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        voxel_size: Real,
        fill_mode: FillMode,
    ) -> Self {
        let mut voxels = VoxelSet::with_voxel_size(vertices, indices, voxel_size, fill_mode, true);
        voxels.compute_bb();
        Self::from_voxel_set(&voxels)
    }

    fn from_voxel_set(vox_set: &VoxelSet) -> Self {
        let centers: Vec<_> = vox_set
            .voxels()
            .iter()
            .map(|v| vox_set.get_voxel_point(v))
            .collect();
        let shape = Voxels::from_points(Vector::splat(vox_set.scale), &centers);
        SharedShape::new(shape)
    }

    /// Initializes a compound shape obtained from the decomposition of the given trimesh (in 3D)
    /// or polyline (in 2D) into voxelized convex parts.
    pub fn voxelized_convex_decomposition(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
    ) -> Vec<Self> {
        Self::voxelized_convex_decomposition_with_params(
            vertices,
            indices,
            &VHACDParameters::default(),
        )
    }

    /// Initializes a compound shape obtained from the decomposition of the given trimesh (in 3D)
    /// or polyline (in 2D) into voxelized convex parts.
    pub fn voxelized_convex_decomposition_with_params(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        params: &VHACDParameters,
    ) -> Vec<Self> {
        let mut parts = vec![];
        let decomp = VHACD::decompose(params, vertices, indices, true);

        for vox_set in decomp.voxel_parts() {
            parts.push(Self::from_voxel_set(vox_set));
        }

        parts
    }

    /// Initializes a compound shape obtained from the decomposition of the given trimesh (in 3D) or
    /// polyline (in 2D) into convex parts.
    pub fn convex_decomposition(vertices: &[Vector], indices: &[[u32; DIM]]) -> Self {
        Self::convex_decomposition_with_params(vertices, indices, &VHACDParameters::default())
    }

    /// Initializes a compound shape obtained from the decomposition of the given trimesh (in 3D) or
    /// polyline (in 2D) into convex parts dilated with round corners.
    pub fn round_convex_decomposition(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        border_radius: Real,
    ) -> Self {
        Self::round_convex_decomposition_with_params(
            vertices,
            indices,
            &VHACDParameters::default(),
            border_radius,
        )
    }

    /// Initializes a compound shape obtained from the decomposition of the given trimesh (in 3D) or
    /// polyline (in 2D) into convex parts.
    pub fn convex_decomposition_with_params(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        params: &VHACDParameters,
    ) -> Self {
        let mut parts = vec![];
        let decomp = VHACD::decompose(params, vertices, indices, true);

        #[cfg(feature = "dim2")]
        for vertices in decomp.compute_exact_convex_hulls(vertices, indices) {
            if let Some(convex) = Self::convex_polyline(vertices) {
                parts.push((Pose::IDENTITY, convex));
            }
        }

        #[cfg(feature = "dim3")]
        for (vertices, indices) in decomp.compute_exact_convex_hulls(vertices, indices) {
            if let Some(convex) = Self::convex_mesh(vertices, &indices) {
                parts.push((Pose::IDENTITY, convex));
            }
        }

        Self::compound(parts)
    }

    /// Initializes a compound shape obtained from the decomposition of the given trimesh (in 3D) or
    /// polyline (in 2D) into convex parts dilated with round corners.
    pub fn round_convex_decomposition_with_params(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        params: &VHACDParameters,
        border_radius: Real,
    ) -> Self {
        let mut parts = vec![];
        let decomp = VHACD::decompose(params, vertices, indices, true);

        #[cfg(feature = "dim2")]
        for vertices in decomp.compute_exact_convex_hulls(vertices, indices) {
            if let Some(convex) = Self::round_convex_polyline(vertices, border_radius) {
                parts.push((Pose::IDENTITY, convex));
            }
        }

        #[cfg(feature = "dim3")]
        for (vertices, indices) in decomp.compute_exact_convex_hulls(vertices, indices) {
            if let Some(convex) = Self::round_convex_mesh(vertices, &indices, border_radius) {
                parts.push((Pose::IDENTITY, convex));
            }
        }

        Self::compound(parts)
    }

    /// Creates a new shared shape that is the convex-hull of the given points.
    pub fn convex_hull(points: &[Vector]) -> Option<Self> {
        #[cfg(feature = "dim2")]
        return ConvexPolygon::from_convex_hull(points).map(|ch| SharedShape(Arc::new(ch)));
        #[cfg(feature = "dim3")]
        return ConvexPolyhedron::from_convex_hull(points).map(|ch| SharedShape(Arc::new(ch)));
    }

    /// Creates a new shared shape that is a 2D convex polygon from a set of points assumed to
    /// describe a counter-clockwise convex polyline.
    ///
    /// This does **not** compute the convex-hull of the input points: convexity of the input is
    /// assumed and not checked. For a version that calculates the convex hull of the input points,
    /// use [`SharedShape::convex_hull`] instead.
    ///
    /// The generated [`ConvexPolygon`] will contain the given `points` with any point
    /// collinear to the previous and next ones removed. For a version that leaves the input
    /// `points` unmodified, use [`SharedShape::convex_polyline_unmodified`].
    ///
    /// Returns `None` if all points form an almost flat line.
    #[cfg(feature = "dim2")]
    pub fn convex_polyline(points: Vec<Vector>) -> Option<Self> {
        ConvexPolygon::from_convex_polyline(points).map(|ch| SharedShape(Arc::new(ch)))
    }

    /// Creates a new shared shape that is a 2D convex polygon from a set of points assumed to
    /// describe a counter-clockwise convex polyline.
    ///
    /// This is the same as [`SharedShape::convex_polyline`] but without removing any point
    /// from the input even if some are coplanar.
    ///
    /// Returns `None` if `points` doesn’t contain at least three points.
    #[cfg(feature = "dim2")]
    pub fn convex_polyline_unmodified(points: Vec<Vector>) -> Option<Self> {
        ConvexPolygon::from_convex_polyline_unmodified(points).map(|ch| SharedShape(Arc::new(ch)))
    }

    /// Creates a new shared shape that is a convex polyhedron formed by the
    /// given set of points assumed to form a convex mesh (no convex-hull will be automatically
    /// computed).
    #[cfg(feature = "dim3")]
    pub fn convex_mesh(points: Vec<Vector>, indices: &[[u32; 3]]) -> Option<Self> {
        ConvexPolyhedron::from_convex_mesh(points, indices).map(|ch| SharedShape(Arc::new(ch)))
    }

    /// Creates a new shared shape with rounded corners that is the
    /// convex-hull of the given points, dilated by `border_radius`.
    pub fn round_convex_hull(points: &[Vector], border_radius: Real) -> Option<Self> {
        #[cfg(feature = "dim2")]
        return ConvexPolygon::from_convex_hull(points).map(|ch| {
            SharedShape(Arc::new(RoundShape {
                inner_shape: ch,
                border_radius,
            }))
        });
        #[cfg(feature = "dim3")]
        return ConvexPolyhedron::from_convex_hull(points).map(|ch| {
            SharedShape(Arc::new(RoundShape {
                inner_shape: ch,
                border_radius,
            }))
        });
    }

    /// Creates a new shared shape with round corners that is a convex polygon formed by the
    /// given set of points assumed to form a convex polyline (no convex-hull will be automatically
    /// computed).
    #[cfg(feature = "dim2")]
    pub fn round_convex_polyline(points: Vec<Vector>, border_radius: Real) -> Option<Self> {
        ConvexPolygon::from_convex_polyline(points).map(|ch| {
            SharedShape(Arc::new(RoundShape {
                inner_shape: ch,
                border_radius,
            }))
        })
    }

    /// Creates a new shared shape with round corners that is a convex polyhedron formed by the
    /// given set of points assumed to form a convex mesh (no convex-hull will be automatically
    /// computed).
    #[cfg(feature = "dim3")]
    pub fn round_convex_mesh(
        points: Vec<Vector>,
        indices: &[[u32; 3]],
        border_radius: Real,
    ) -> Option<Self> {
        ConvexPolyhedron::from_convex_mesh(points, indices).map(|ch| {
            SharedShape(Arc::new(RoundShape {
                inner_shape: ch,
                border_radius,
            }))
        })
    }

    /// Initializes a heightfield shape defined by its set of height and a scale
    /// factor along each coordinate axis.
    #[cfg(feature = "dim2")]
    pub fn heightfield(heights: Vec<Real>, scale: Vector) -> Self {
        SharedShape(Arc::new(HeightField::new(heights, scale)))
    }

    /// Initializes a heightfield shape on the x-z plane defined by its set of height and a scale
    /// factor along each coordinate axis.
    #[cfg(feature = "dim3")]
    pub fn heightfield(heights: Array2<Real>, scale: Vector) -> Self {
        SharedShape(Arc::new(HeightField::new(heights, scale)))
    }

    /// Initializes a heightfield shape on the x-z plane defined by its set of height, a scale
    /// factor along each coordinate axis, and [`HeightFieldFlags`].
    #[cfg(feature = "dim3")]
    pub fn heightfield_with_flags(
        heights: Array2<Real>,
        scale: Vector,
        flags: HeightFieldFlags,
    ) -> Self {
        SharedShape(Arc::new(HeightField::with_flags(heights, scale, flags)))
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for SharedShape {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.as_typed_shape().serialize(serializer)
    }
}

#[cfg(feature = "serde-serialize")]
impl<'de> serde::Deserialize<'de> for SharedShape {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use crate::serde::de::Error;
        DeserializableTypedShape::deserialize(deserializer)?
            .into_shared_shape()
            .ok_or(D::Error::custom("Cannot deserialize custom shape."))
    }
}
