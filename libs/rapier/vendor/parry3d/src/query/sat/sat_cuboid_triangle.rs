#[cfg(feature = "dim3")]
use crate::approx::AbsDiffEq;
use crate::math::{Pose, Real, Vector};
#[cfg(feature = "dim3")]
use crate::query::sat;
#[cfg(feature = "dim2")]
use crate::query::sat::support_map_support_map_compute_separation;
use crate::shape::{Cuboid, SupportMap, Triangle};

#[cfg(all(feature = "dim3", not(feature = "std")))]
use simba::scalar::ComplexField;

/// Finds the best separating axis by testing all edge-edge combinations between a cuboid and a triangle (3D only).
///
/// In 3D collision detection, when a box and triangle intersect, the contact may occur along an
/// axis perpendicular to an edge from each shape. This function tests all 3×3 = 9 such axes
/// (cross products of cuboid edges with triangle edges) to find the one with maximum separation.
///
/// # Parameters
///
/// - `cube1`: The cuboid (in its local coordinate frame)
/// - `triangle2`: The triangle
/// - `pos12`: The position of the triangle relative to the cuboid
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The maximum separation found across all edge-edge axes
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping (penetration depth)
/// - `Vector`: The axis direction that gives this separation
///
/// # The 9 Axes Tested
///
/// The function tests cross products between:
/// - 3 cuboid edge directions: X, Y, Z (aligned with cuboid axes)
/// - 3 triangle edge vectors: AB, BC, CA
///
/// This gives 3×3 = 9 potential separating axes. Each axis is normalized before testing,
/// and degenerate axes (near-zero length, indicating parallel edges) are skipped.
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Cuboid, Triangle};
/// use parry3d::query::sat::cuboid_triangle_find_local_separating_edge_twoway;
/// use parry3d::math::{Vector, Pose};
///
/// let cube = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
/// let triangle = Triangle::new(
///     Vector::ZERO,
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.0, 1.0, 0.0)
/// );
///
/// // Position triangle near the cube
/// let pos12 = Pose::translation(2.0, 0.0, 0.0);
///
/// let (separation, axis) = cuboid_triangle_find_local_separating_edge_twoway(
///     &cube,
///     &triangle,
///     &pos12
/// );
///
/// println!("Edge-edge separation: {} along {}", separation, axis);
/// # }
/// ```
///
/// # Usage in Complete SAT
///
/// For a complete cuboid-triangle SAT test, you must also test:
/// 1. Cuboid face normals (X, Y, Z axes)
/// 2. Triangle face normal (perpendicular to the triangle plane)
/// 3. Edge-edge axes (this function)
#[cfg(feature = "dim3")]
#[inline(always)]
pub fn cuboid_triangle_find_local_separating_edge_twoway(
    cube1: &Cuboid,
    triangle2: &Triangle,
    pos12: &Pose,
) -> (Real, Vector) {
    // NOTE: everything in this method will be expressed
    // in the local-space of the first triangle. So we
    // don't bother adding 2_1 suffixes (e.g. `a2_1`) to everything in
    // order to keep the code more readable.
    let a = pos12 * triangle2.a;
    let b = pos12 * triangle2.b;
    let c = pos12 * triangle2.c;

    let ab = b - a;
    let bc = c - b;
    let ca = a - c;

    // We have 3 * 3 = 9 axes to test.
    let axes = [
        // Vector::{x, y ,z}().cross(ab)
        Vector::new(0.0, -ab.z, ab.y),
        Vector::new(ab.z, 0.0, -ab.x),
        Vector::new(-ab.y, ab.x, 0.0),
        // Vector::{x, y ,z}().cross(bc)
        Vector::new(0.0, -bc.z, bc.y),
        Vector::new(bc.z, 0.0, -bc.x),
        Vector::new(-bc.y, bc.x, 0.0),
        // Vector::{x, y ,z}().cross(ca)
        Vector::new(0.0, -ca.z, ca.y),
        Vector::new(ca.z, 0.0, -ca.x),
        Vector::new(-ca.y, ca.x, 0.0),
    ];

    let tri_dots = [
        (axes[0].dot(a), axes[0].dot(c)),
        (axes[1].dot(a), axes[1].dot(c)),
        (axes[2].dot(a), axes[2].dot(c)),
        (axes[3].dot(a), axes[3].dot(c)),
        (axes[4].dot(a), axes[4].dot(c)),
        (axes[5].dot(a), axes[5].dot(c)),
        (axes[6].dot(a), axes[6].dot(b)),
        (axes[7].dot(a), axes[7].dot(b)),
        (axes[8].dot(a), axes[8].dot(b)),
    ];

    let mut best_sep = -Real::MAX;
    let mut best_axis = axes[0];

    for (i, axis) in axes.iter().enumerate() {
        let axis_norm_squared = axis.length_squared();

        if axis_norm_squared > Real::default_epsilon() {
            let axis_norm = axis_norm_squared.sqrt();

            // NOTE: for both axis and -axis, the dot1 will have the same
            // value because of the cuboid's symmetry.
            let local_pt1 = cube1.local_support_point(*axis);
            let dot1 = local_pt1.dot(*axis) / axis_norm;

            let (dot2_min, dot2_max) = crate::utils::sort2(tri_dots[i].0, tri_dots[i].1);

            let separation_a = dot2_min / axis_norm - dot1; // separation on axis
            let separation_b = -dot2_max / axis_norm - dot1; // separation on -axis

            if separation_a > best_sep {
                best_sep = separation_a;
                best_axis = *axis / axis_norm;
            }

            if separation_b > best_sep {
                best_sep = separation_b;
                best_axis = -*axis / axis_norm;
            }
        }
    }

    (best_sep, best_axis)
}

/// Finds the best separating axis by testing the edge normals of a triangle against a support map shape (2D only).
///
/// In 2D, a triangle has three edges, each with an associated outward-pointing normal.
/// This function tests all three edge normals to find which gives the maximum separation
/// from the support map shape.
///
/// # Parameters
///
/// - `triangle1`: The triangle whose edge normals will be tested
/// - `shape2`: Any convex shape implementing [`SupportMap`]
/// - `pos12`: The position of `shape2` relative to `triangle1`
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The maximum separation found among the triangle's edge normals
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
/// - `Vector`: The edge normal direction that gives this separation
///
/// # 2D vs 3D
///
/// In 2D, triangles are true polygons with edges that have normals. In 3D, triangles are
/// planar surfaces with a face normal (see the 3D version of this function).
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::{Triangle, Ball};
/// use parry2d::query::sat::triangle_support_map_find_local_separating_normal_oneway;
/// use parry2d::math::{Vector, Pose};
///
/// let triangle = Triangle::new(
///     Vector::ZERO,
///     Vector::new(2.0, 0.0),
///     Vector::new(1.0, 2.0)
/// );
/// let sphere = Ball::new(0.5);
///
/// let pos12 = Pose::translation(3.0, 1.0);
///
/// let (separation, normal) = triangle_support_map_find_local_separating_normal_oneway(
///     &triangle,
///     &sphere,
///     &pos12
/// );
///
/// if separation > 0.0 {
///     println!("Separated by {} along edge normal {}", separation, normal);
/// }
/// # }
/// ```
#[cfg(feature = "dim2")]
pub fn triangle_support_map_find_local_separating_normal_oneway(
    triangle1: &Triangle,
    shape2: &impl SupportMap,
    pos12: &Pose,
) -> (Real, Vector) {
    let mut best_sep = -Real::MAX;
    let mut best_normal = Vector::ZERO;

    for edge in &triangle1.edges() {
        if let Some(normal) = edge.normal() {
            let sep = support_map_support_map_compute_separation(triangle1, shape2, pos12, normal);

            if sep > best_sep {
                best_sep = sep;
                best_normal = normal;
            }
        }
    }

    (best_sep, best_normal)
}

/// Finds the best separating axis by testing a triangle's normals against a cuboid (2D only).
///
/// This is a specialized version of [`triangle_support_map_find_local_separating_normal_oneway`]
/// for the specific case of a triangle and cuboid. In 2D, it tests the three edge normals of
/// the triangle.
///
/// # Parameters
///
/// - `triangle1`: The triangle whose edge normals will be tested
/// - `shape2`: The cuboid
/// - `pos12`: The position of the cuboid relative to the triangle
///
/// # Returns
///
/// A tuple containing the maximum separation and the corresponding edge normal direction.
///
/// See [`triangle_support_map_find_local_separating_normal_oneway`] for more details and examples.
#[cfg(feature = "dim2")]
pub fn triangle_cuboid_find_local_separating_normal_oneway(
    triangle1: &Triangle,
    shape2: &Cuboid,
    pos12: &Pose,
) -> (Real, Vector) {
    triangle_support_map_find_local_separating_normal_oneway(triangle1, shape2, pos12)
}

/// Finds the best separating axis by testing a triangle's face normal against a cuboid (3D only).
///
/// In 3D, a triangle is a planar surface with a single face normal (perpendicular to the plane).
/// This function tests both directions of this normal (+normal and -normal) to find the maximum
/// separation from the cuboid.
///
/// # How It Works
///
/// The function uses the triangle's face normal and one of its vertices (point A) to represent
/// the triangle as a point-with-normal. It then delegates to
/// [`point_cuboid_find_local_separating_normal_oneway`](super::point_cuboid_find_local_separating_normal_oneway)
/// which efficiently handles this case.
///
/// # Parameters
///
/// - `triangle1`: The triangle whose face normal will be tested
/// - `shape2`: The cuboid
/// - `pos12`: The position of the cuboid relative to the triangle
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The separation distance along the triangle's face normal
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
/// - `Vector`: The face normal direction (or its negation) that gives this separation
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Triangle, Cuboid};
/// use parry3d::query::sat::triangle_cuboid_find_local_separating_normal_oneway;
/// use parry3d::math::{Vector, Pose};
///
/// // Horizontal triangle in the XY plane
/// let triangle = Triangle::new(
///     Vector::ZERO,
///     Vector::new(2.0, 0.0, 0.0),
///     Vector::new(1.0, 2.0, 0.0)
/// );
/// let cube = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
///
/// // Position cube above the triangle
/// let pos12 = Pose::translation(1.0, 1.0, 2.0);
///
/// let (separation, normal) = triangle_cuboid_find_local_separating_normal_oneway(
///     &triangle,
///     &cube,
///     &pos12
/// );
///
/// println!("Separation along triangle normal: {}", separation);
/// # }
/// ```
///
/// # 2D vs 3D
///
/// - **2D version**: Tests three edge normals (one per triangle edge)
/// - **3D version** (this function): Tests one face normal (perpendicular to triangle plane)
#[cfg(feature = "dim3")]
#[inline(always)]
pub fn triangle_cuboid_find_local_separating_normal_oneway(
    triangle1: &Triangle,
    shape2: &Cuboid,
    pos12: &Pose,
) -> (Real, Vector) {
    sat::point_cuboid_find_local_separating_normal_oneway(
        triangle1.a,
        triangle1.normal(),
        shape2,
        pos12,
    )
}
