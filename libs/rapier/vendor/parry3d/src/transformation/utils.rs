//! Low-level utilities for mesh and geometry generation.
//!
//! This module provides primitive building blocks for constructing triangle meshes and other
//! geometric structures. These functions are primarily used internally by Parry's shape-to-mesh
//! conversion utilities, but are exposed for users who need fine-grained control over mesh
//! generation.
//!
//! # Overview
//!
//! The utilities fall into several categories:
//!
//! ## Vector Transformations
//! - [`transform`] / [`transformed`] - Apply rigid transformations (rotation + translation)
//! - [`scaled`] - Apply non-uniform scaling
//!
//! ## Vertex Generation
//! - `push_circle` - Generate circle points in 3D (XZ plane)
#![cfg_attr(
    feature = "dim2",
    doc = "- [`push_xy_arc`] - Generate arc points in 2D (XY plane)"
)]
//! - [`push_arc`] - Generate arc points between two endpoints
//!
//! ## Index Buffer Generation
//! - `push_ring_indices` / `push_open_ring_indices` - Connect two circles into a tube
//! - `push_rectangle_indices` - Generate two triangles forming a quad
//! - `push_degenerate_top_ring_indices` - Connect circle to a single apex point
//! - `push_filled_circle_indices` - Fill a circle with triangles (fan triangulation)
//!
//! ## Edge/Outline Generation
//! - `push_circle_outline_indices` - Edge loop for a closed circle
//! - `push_open_circle_outline_indices` - Edge chain (not closed)
//! - `push_arc_idx` - Edge indices for an arc
//!
//! ## Advanced Operations
//! - `reverse_clockwising` - Flip triangle winding order (reverse normals)
//! - `apply_revolution` - Create surface of revolution from a profile curve
//! - `push_arc_and_idx` - Generate arc geometry and indices together
//!
//! # Usage Patterns
//!
//! ## Building a Cylinder
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::transformation::utils::{push_circle, push_ring_indices, push_filled_circle_indices};
//! use parry3d::math::Vector;
//! use std::f32::consts::PI;
//!
//! let mut vertices = Vec::new();
//! let mut indices = Vec::new();
//!
//! let radius = 2.0;
//! let height = 10.0;
//! let nsubdiv = 16;
//! let dtheta = 2.0 * PI / nsubdiv as f32;
//!
//! // Create bottom and top circles
//! push_circle(radius, nsubdiv, dtheta, 0.0, &mut vertices);      // Bottom at y=0
//! push_circle(radius, nsubdiv, dtheta, height, &mut vertices);   // Top at y=height
//!
//! // Connect the circles to form the cylinder body
//! push_ring_indices(0, nsubdiv, nsubdiv, &mut indices);
//!
//! // Cap the bottom
//! push_filled_circle_indices(0, nsubdiv, &mut indices);
//!
//! // Cap the top
//! push_filled_circle_indices(nsubdiv, nsubdiv, &mut indices);
//!
//! println!("Cylinder: {} vertices, {} triangles", vertices.len(), indices.len());
//! # }
//! ```
//!
//! ## Building a Cone
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::transformation::utils::{push_circle, push_degenerate_top_ring_indices, push_filled_circle_indices};
//! use parry3d::math::Vector;
//! use std::f32::consts::PI;
//!
//! let mut vertices = Vec::new();
//! let mut indices = Vec::new();
//!
//! let radius = 3.0;
//! let height = 5.0;
//! let nsubdiv = 20;
//! let dtheta = 2.0 * PI / nsubdiv as f32;
//!
//! // Create the base circle
//! push_circle(radius, nsubdiv, dtheta, 0.0, &mut vertices);
//!
//! // Add apex point at the top
//! vertices.push(Vector::new(0.0, height, 0.0));
//! let apex_idx = (vertices.len() - 1) as u32;
//!
//! // Connect base circle to apex
//! push_degenerate_top_ring_indices(0, apex_idx, nsubdiv, &mut indices);
//!
//! // Cap the base
//! push_filled_circle_indices(0, nsubdiv, &mut indices);
//!
//! println!("Cone: {} vertices, {} triangles", vertices.len(), indices.len());
//! # }
//! ```
//!
//! ## Transforming Existing Geometry
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::transformation::utils::{transform, scaled};
//! use parry3d::math::{Vector, Pose, Rotation};
//! use core::f32::consts::PI;
//!
//! let mut points = vec![
//!     Vector::new(1.0, 0.0, 0.0),
//!     Vector::new(0.0, 1.0, 0.0),
//!     Vector::new(0.0, 0.0, 1.0),
//! ];
//!
//! // First, scale non-uniformly
//! let points = scaled(points, Vector::new(2.0, 1.0, 0.5));
//!
//! // Then rotate 45 degrees around Y axis
//! let rotation = Rotation::from_axis_angle(Vector::Y, PI / 4.0);
//! let translation = Vector::new(10.0, 5.0, 0.0);
//! let isometry = Pose::from_parts(translation, rotation);
//!
//! let final_points = parry3d::transformation::utils::transformed(points, isometry);
//! # }
//! ```
//!
//! # Design Philosophy
//!
//! These functions follow a "builder" pattern where:
//! 1. Vertices are pushed to a `Vec<Vector>`
//! 2. Indices are pushed to a `Vec<[u32; 3]>` (triangles) or `Vec<[u32; 2]>` (edges)
//! 3. Functions work with index offsets, allowing incremental construction
//! 4. No memory is allocated except for the output buffers
//!
//! This design allows for efficient, flexible mesh construction with minimal overhead.
//!
//! # Performance Notes
//!
//! - All functions use simple loops without SIMD (suitable for small to medium subdivisions)
//! - Index generation has O(n) complexity where n is the subdivision count
//! - Vector generation involves trigonometric functions (sin/cos) per subdivision
//! - For high subdivision counts (>1000), consider caching generated geometry
//!
//! # See Also
//!
//! - `to_trimesh` module - High-level shape to mesh conversion (see individual shape `to_trimesh()` methods)
//! - [`convex_hull`](crate::transformation::convex_hull) - Convex hull computation
//! - [`TriMesh`](crate::shape::TriMesh) - Triangle mesh shape

#[cfg(feature = "dim3")]
use crate::math::RealField;
#[cfg(feature = "dim2")]
use crate::math::VectorExt;
use crate::math::{ComplexField, Pose, Real, Vector};
use alloc::vec::Vec;
#[cfg(feature = "dim3")]
use {crate::math::DIM, num::Zero};

/// Applies in-place a transformation to an array of points.
///
/// This function modifies each point in the slice by applying the given isometry
/// (rigid transformation consisting of rotation and translation).
///
/// # Arguments
/// * `points` - A mutable slice of points to transform
/// * `m` - The isometry (rigid transformation) to apply
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::utils::transform;
/// use parry3d::math::{Vector, Pose, Rotation};
///
/// // Create some points
/// let mut points = vec![
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.0, 1.0, 0.0),
///     Vector::new(0.0, 0.0, 1.0),
/// ];
///
/// // Create a translation
/// let transform_iso = Pose::from_parts(
///     Vector::new(10.0, 20.0, 30.0),
///     Rotation::IDENTITY
/// );
///
/// // Apply the transformation in-place
/// transform(&mut points, transform_iso);
///
/// assert_eq!(points[0], Vector::new(11.0, 20.0, 30.0));
/// assert_eq!(points[1], Vector::new(10.0, 21.0, 30.0));
/// assert_eq!(points[2], Vector::new(10.0, 20.0, 31.0));
/// # }
/// ```
pub fn transform(points: &mut [Vector], m: Pose) {
    points.iter_mut().for_each(|p| *p = m * *p);
}

/// Returns the transformed version of a vector of points.
///
/// This function takes ownership of a vector of points, applies the given isometry
/// transformation, and returns the transformed vector.
///
/// # Arguments
/// * `points` - A vector of points to transform (ownership is taken)
/// * `m` - The isometry (rigid transformation) to apply
///
/// # Returns
/// A new vector containing the transformed points
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::transformation::utils::transformed;
/// use parry2d::math::{Vector, Pose, Rotation};
/// use std::f32::consts::PI;
///
/// let points = vec![
///     Vector::new(1.0, 0.0),
///     Vector::new(0.0, 1.0),
/// ];
///
/// // Rotate 90 degrees counter-clockwise around origin
/// let rotation = Rotation::new(PI / 2.0);
/// let transform = Pose::from_parts(Vector::ZERO, rotation);
///
/// let result = transformed(points, transform);
///
/// // Vectors are now rotated
/// assert!((result[0].x - 0.0).abs() < 1e-6);
/// assert!((result[0].y - 1.0).abs() < 1e-6);
/// # }
/// ```
pub fn transformed(mut points: Vec<Vector>, m: Pose) -> Vec<Vector> {
    transform(&mut points, m);
    points
}

/// Returns the scaled version of a vector of points.
///
/// This function takes ownership of a vector of points and applies a non-uniform
/// scale factor to each component. Unlike [`transformed`], this is a non-rigid
/// transformation that can stretch or compress points along different axes.
///
/// # Arguments
/// * `points` - A vector of points to scale (ownership is taken)
/// * `scale` - The scale factor for each axis
///
/// # Returns
/// A new vector containing the scaled points
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::utils::scaled;
/// use parry3d::math::Vector;
///
/// let points = vec![
///     Vector::new(1.0, 2.0, 3.0),
///     Vector::new(4.0, 5.0, 6.0),
/// ];
///
/// // Scale x by 2, y by 3, z by 0.5
/// let scale = Vector::new(2.0, 3.0, 0.5);
/// let result = scaled(points, scale);
///
/// assert_eq!(result[0], Vector::new(2.0, 6.0, 1.5));
/// assert_eq!(result[1], Vector::new(8.0, 15.0, 3.0));
/// # }
/// ```
pub fn scaled(mut points: Vec<Vector>, scale: Vector) -> Vec<Vector> {
    points.iter_mut().for_each(|p| *p *= scale);
    points
}

// TODO: remove that in favor of `push_xy_circle` ?
/// Pushes a discretized counterclockwise circle to a buffer.
///
/// This function generates points along a circle in the XZ plane at a given Y coordinate.
/// Vectors are generated counter-clockwise when viewed from above (positive Y direction).
///
/// # Arguments
/// * `radius` - The radius of the circle
/// * `nsubdiv` - Number of subdivisions (points to generate)
/// * `dtheta` - Angle increment between consecutive points (in radians)
/// * `y` - The Y coordinate of the circle plane
/// * `out` - Output buffer where circle points will be pushed
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::utils::push_circle;
/// use parry3d::math::Vector;
/// use std::f32::consts::PI;
///
/// let mut points = Vec::new();
/// let radius = 5.0;
/// let nsubdiv = 8; // Octagon
/// let dtheta = 2.0 * PI / nsubdiv as f32;
/// let y_level = 10.0;
///
/// push_circle(radius, nsubdiv, dtheta, y_level, &mut points);
///
/// assert_eq!(points.len(), 8);
/// // All points are at Y = 10.0
/// assert!(points.iter().all(|p| (p.y - 10.0).abs() < 1e-6));
/// // First point is at (radius, y, 0)
/// assert!((points[0].x - radius).abs() < 1e-6);
/// assert!((points[0].z).abs() < 1e-6);
/// # }
/// ```
#[cfg(feature = "dim3")]
#[inline]
pub fn push_circle(radius: Real, nsubdiv: u32, dtheta: Real, y: Real, out: &mut Vec<Vector>) {
    let mut curr_theta = Real::zero();

    for _ in 0..nsubdiv {
        out.push(Vector::new(
            <Real as ComplexField>::cos(curr_theta) * radius,
            y,
            <Real as ComplexField>::sin(curr_theta) * radius,
        ));
        curr_theta += dtheta;
    }
}

/// Pushes a discretized counterclockwise arc to a buffer.
///
/// This function generates points along an arc in the XY plane (2D).
/// The arc is contained on the plane spanned by the X and Y axes.
/// Vectors are generated counter-clockwise starting from angle 0 (positive X axis).
///
/// # Arguments
/// * `radius` - The radius of the arc
/// * `nsubdiv` - Number of subdivisions (points to generate)
/// * `dtheta` - Angle increment between consecutive points (in radians)
/// * `out` - Output buffer where arc points will be pushed
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::transformation::utils::push_xy_arc;
/// use parry2d::math::Vector;
/// use std::f32::consts::PI;
///
/// let mut points = Vec::new();
/// let radius = 3.0;
/// let nsubdiv = 4; // Quarter circle
/// let dtheta = PI / 2.0 / (nsubdiv - 1) as f32;
///
/// push_xy_arc(radius, nsubdiv, dtheta, &mut points);
///
/// assert_eq!(points.len(), 4);
/// // First point is approximately at (radius, 0)
/// assert!((points[0].x - radius).abs() < 1e-6);
/// assert!((points[0].y).abs() < 1e-6);
/// # }
/// ```
#[inline]
#[cfg(feature = "dim2")]
pub fn push_xy_arc(radius: Real, nsubdiv: u32, dtheta: Real, out: &mut Vec<Vector>) {
    let mut curr_theta: Real = 0.0;

    for _ in 0..nsubdiv {
        let mut pt_coords = Vector::ZERO;

        pt_coords[0] = <Real as ComplexField>::cos(curr_theta) * radius;
        pt_coords[1] = <Real as ComplexField>::sin(curr_theta) * radius;
        out.push(pt_coords);

        curr_theta += dtheta;
    }
}

/// Creates the triangle faces connecting two circles with the same discretization.
///
/// This function generates triangle indices to form a closed ring (tube segment) between
/// two parallel circles. The circles must have the same number of points. The ring wraps
/// around completely, connecting the last points back to the first.
///
/// # Arguments
/// * `base_lower_circle` - Index of the first point of the lower circle
/// * `base_upper_circle` - Index of the first point of the upper circle
/// * `nsubdiv` - Number of points in each circle
/// * `out` - Output buffer where triangle indices will be pushed
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::utils::{push_circle, push_ring_indices};
/// use parry3d::math::Vector;
/// use std::f32::consts::PI;
///
/// let mut vertices = Vec::new();
/// let mut indices = Vec::new();
///
/// let nsubdiv = 8;
/// let dtheta = 2.0 * PI / nsubdiv as f32;
///
/// // Create two circles at different heights
/// push_circle(2.0, nsubdiv, dtheta, 0.0, &mut vertices);  // Lower circle
/// push_circle(2.0, nsubdiv, dtheta, 5.0, &mut vertices);  // Upper circle
///
/// // Connect them with triangles
/// push_ring_indices(0, nsubdiv, nsubdiv, &mut indices);
///
/// // A ring with n subdivisions creates 2*n triangles
/// assert_eq!(indices.len(), 2 * nsubdiv as usize);
/// # }
/// ```
#[cfg(feature = "dim3")]
#[inline]
pub fn push_ring_indices(
    base_lower_circle: u32,
    base_upper_circle: u32,
    nsubdiv: u32,
    out: &mut Vec<[u32; DIM]>,
) {
    push_open_ring_indices(base_lower_circle, base_upper_circle, nsubdiv, out);

    // adjust the last two triangles
    push_rectangle_indices(
        base_upper_circle,
        base_upper_circle + nsubdiv - 1,
        base_lower_circle,
        base_lower_circle + nsubdiv - 1,
        out,
    );
}

/// Creates the triangle faces connecting two circles, leaving the ring open.
///
/// This is similar to `push_ring_indices`, but doesn't close the ring. The connection
/// between the last point and the first point is not made, leaving a gap. This is useful
/// for creating open cylinders or partial tubes.
///
/// # Arguments
/// * `base_lower_circle` - Index of the first point of the lower circle
/// * `base_upper_circle` - Index of the first point of the upper circle
/// * `nsubdiv` - Number of points in each circle
/// * `out` - Output buffer where triangle indices will be pushed
///
/// # Panics
/// Panics if `nsubdiv` is 0.
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::utils::{push_circle, push_open_ring_indices};
/// use parry3d::math::Vector;
/// use std::f32::consts::PI;
///
/// let mut vertices = Vec::new();
/// let mut indices = Vec::new();
///
/// let nsubdiv = 8;
/// let dtheta = 2.0 * PI / nsubdiv as f32;
///
/// // Create two circles
/// push_circle(2.0, nsubdiv, dtheta, 0.0, &mut vertices);
/// push_circle(2.0, nsubdiv, dtheta, 5.0, &mut vertices);
///
/// // Connect them without closing the ring
/// push_open_ring_indices(0, nsubdiv, nsubdiv, &mut indices);
///
/// // Open ring has 2 fewer triangles than closed ring
/// assert_eq!(indices.len(), 2 * (nsubdiv - 1) as usize);
/// # }
/// ```
#[cfg(feature = "dim3")]
#[inline]
pub fn push_open_ring_indices(
    base_lower_circle: u32,
    base_upper_circle: u32,
    nsubdiv: u32,
    out: &mut Vec<[u32; DIM]>,
) {
    assert!(nsubdiv > 0);

    for i in 0..nsubdiv - 1 {
        let bli = base_lower_circle + i;
        let bui = base_upper_circle + i;
        push_rectangle_indices(bui + 1, bui, bli + 1, bli, out);
    }
}

/// Creates the faces from a circle and a point that is shared by all triangle.
#[cfg(feature = "dim3")]
#[inline]
pub fn push_degenerate_top_ring_indices(
    base_circle: u32,
    point: u32,
    nsubdiv: u32,
    out: &mut Vec<[u32; DIM]>,
) {
    push_degenerate_open_top_ring_indices(base_circle, point, nsubdiv, out);

    out.push([base_circle + nsubdiv - 1, point, base_circle]);
}

/// Creates the faces from a circle and a point that is shared by all triangle.
#[cfg(feature = "dim3")]
#[inline]
pub fn push_degenerate_open_top_ring_indices(
    base_circle: u32,
    point: u32,
    nsubdiv: u32,
    out: &mut Vec<[u32; DIM]>,
) {
    assert!(nsubdiv > 0);

    for i in 0..nsubdiv - 1 {
        out.push([base_circle + i, point, base_circle + i + 1]);
    }
}

/// Pushes indices so that a circle is filled with triangles. Each triangle will have the
/// `base_circle` point in common.
/// Pushes `nsubdiv - 2` elements to `out`.
#[cfg(feature = "dim3")]
#[inline]
pub fn push_filled_circle_indices(base_circle: u32, nsubdiv: u32, out: &mut Vec<[u32; DIM]>) {
    for i in base_circle + 1..base_circle + nsubdiv - 1 {
        out.push([base_circle, i, i + 1]);
    }
}

/// Pushes two triangles forming a rectangle to the index buffer.
///
/// Given four corner point indices, this function creates two counter-clockwise triangles
/// that form a rectangle (quad). The winding order ensures the normal points in the
/// consistent direction based on the right-hand rule.
///
/// # Arguments
/// * `ul` - Index of the upper-left point
/// * `ur` - Index of the upper-right point
/// * `dl` - Index of the down-left point
/// * `dr` - Index of the down-right point
/// * `out` - Output buffer where triangle indices will be pushed
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::utils::push_rectangle_indices;
///
/// let mut indices = Vec::new();
///
/// // Create a quad from points 0, 1, 2, 3
/// // Layout:  0 --- 1
/// //          |     |
/// //          2 --- 3
/// push_rectangle_indices(0, 1, 2, 3, &mut indices);
///
/// assert_eq!(indices.len(), 2); // Two triangles
/// assert_eq!(indices[0], [0, 2, 3]); // First triangle
/// assert_eq!(indices[1], [3, 1, 0]); // Second triangle
/// # }
/// ```
#[cfg(feature = "dim3")]
#[inline]
pub fn push_rectangle_indices(ul: u32, ur: u32, dl: u32, dr: u32, out: &mut Vec<[u32; DIM]>) {
    out.push([ul, dl, dr]);
    out.push([dr, ur, ul]);
}

/// Reverses the winding order of triangle faces.
///
/// This function flips the winding order of triangles from counter-clockwise to clockwise
/// or vice versa. This effectively flips the direction of face normals, which is useful
/// when you need to invert a mesh or correct winding order issues.
///
/// # Arguments
/// * `indices` - Mutable slice of triangle indices to reverse
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::utils::reverse_clockwising;
///
/// let mut triangles = vec![
///     [0, 1, 2],
///     [2, 3, 0],
/// ];
///
/// // Reverse the winding order
/// reverse_clockwising(&mut triangles);
///
/// // First two vertices of each triangle are swapped
/// assert_eq!(triangles[0], [1, 0, 2]);
/// assert_eq!(triangles[1], [3, 2, 0]);
/// # }
/// ```
#[cfg(feature = "dim3")]
#[inline]
pub fn reverse_clockwising(indices: &mut [[u32; DIM]]) {
    indices.iter_mut().for_each(|idx| idx.swap(0, 1));
}

/// Pushes the index buffer of a closed loop.
#[cfg(feature = "dim3")]
#[inline]
pub fn push_circle_outline_indices(indices: &mut Vec<[u32; 2]>, range: core::ops::Range<u32>) {
    indices.extend((range.start..range.end - 1).map(|i| [i, i + 1]));
    indices.push([range.end - 1, range.start]);
}

/// Pushes the index buffer of an open chain.
#[cfg(feature = "dim3")]
#[inline]
pub fn push_open_circle_outline_indices(indices: &mut Vec<[u32; 2]>, range: core::ops::Range<u32>) {
    indices.extend((range.start..range.end - 1).map(|i| [i, i + 1]));
}

/// Pushes to `out_vtx` a set of points forming an arc starting at `start`, ending at `end` with
/// revolution center at `center`. The curve is approximated by pushing `nsubdivs` points.
/// The `start` and `end` point are not pushed to `out_vtx`.
///
/// Also pushes to `out_idx` the appropriate index buffer to form the arc (including attaches to
/// the `start` and `end` points).
#[cfg(feature = "dim3")]
pub fn push_arc_and_idx(
    center: Vector,
    start: u32,
    end: u32,
    nsubdivs: u32,
    out_vtx: &mut Vec<Vector>,
    out_idx: &mut Vec<[u32; 2]>,
) {
    let base = out_vtx.len() as u32;
    push_arc(
        center,
        out_vtx[start as usize],
        out_vtx[end as usize],
        nsubdivs,
        out_vtx,
    );
    push_arc_idx(start, base..base + nsubdivs - 1, end, out_idx);
}

/// Pushes points forming an arc between two points around a center.
///
/// This function generates intermediate points along a circular arc from `start` to `end`,
/// rotating around `center`. The arc is approximated by `nsubdivs` points. The `start` and
/// `end` points themselves are NOT added to the output buffer - only intermediate points.
///
/// The function interpolates both the angle and the radius, so it can handle arcs where
/// the start and end points are at different distances from the center (spiral-like paths).
///
/// # Arguments
/// * `center` - The center point of rotation
/// * `start` - Starting point of the arc (not included in output)
/// * `end` - Ending point of the arc (not included in output)
/// * `nsubdivs` - Number of intermediate points to generate
/// * `out` - Output buffer where arc points will be pushed
///
/// # Panics
/// Panics if `nsubdivs` is 0.
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::transformation::utils::push_arc;
/// use parry2d::math::Vector;
///
/// let mut points = Vec::new();
/// let center = Vector::new(0.0, 0.0);
/// let start = Vector::new(5.0, 0.0);  // 5 units to the right
/// let end = Vector::new(0.0, 5.0);    // 5 units up (90 degree arc)
///
/// // Generate 3 intermediate points
/// push_arc(center, start, end, 3, &mut points);
///
/// // Should have 2 intermediate points (nsubdivs - 1)
/// assert_eq!(points.len(), 2);
/// # }
/// ```
pub fn push_arc(center: Vector, start: Vector, end: Vector, nsubdivs: u32, out: &mut Vec<Vector>) {
    assert!(nsubdivs > 0);
    let (start_dir, start_len) = (start - center).normalize_and_length();
    let (end_dir, end_len) = (end - center).normalize_and_length();

    if start_len > 0.0 && end_len > 0.0 {
        let len_inc = (end_len - start_len) / nsubdivs as Real;

        #[cfg(feature = "dim2")]
        let rot = {
            use crate::math::Rot2;
            let angle = start_dir.angle(end_dir);
            let angle_inc = angle / nsubdivs as Real;
            Rot2::new(angle_inc)
        };

        #[cfg(feature = "dim3")]
        let rot = {
            use crate::math::Rot3;
            // Compute rotation from start_dir to end_dir
            let rot = Rot3::from_rotation_arc(start_dir, end_dir);
            let axisangle = rot.to_scaled_axis();
            Rot3::from_scaled_axis(axisangle / nsubdivs as Real)
        };

        let mut curr_dir = start_dir;
        let mut curr_len = start_len;

        for _ in 0..nsubdivs - 1 {
            curr_dir = rot * curr_dir;
            curr_len += len_inc;

            out.push(center + curr_dir * curr_len);
        }
    }
}

/// Pushes the index buffer for an arc between `start` and `end` and intermediate points in the
/// range `arc`.
#[cfg(feature = "dim3")]
pub fn push_arc_idx(start: u32, arc: core::ops::Range<u32>, end: u32, out: &mut Vec<[u32; 2]>) {
    if arc.is_empty() {
        out.push([start, end]);
    } else {
        out.push([start, arc.start]);
        for i in arc.start..arc.end - 1 {
            out.push([i, i + 1])
        }
        out.push([arc.end - 1, end])
    }
}

/// Applies a revolution, using the Y symmetry axis passing through the origin.
#[cfg(feature = "dim3")]
pub fn apply_revolution(
    collapse_bottom: bool,
    collapse_top: bool,
    circle_ranges: &[core::ops::Range<u32>],
    nsubdivs: u32,
    out_vtx: &mut Vec<Vector>, // Must be set to the half-profile.
    out_idx: &mut Vec<[u32; 2]>,
) {
    let ang_increment = Real::two_pi() / (nsubdivs as Real);
    let angles = [
        ang_increment * (nsubdivs / 4) as Real,
        ang_increment * (nsubdivs / 2) as Real,
        ang_increment * ((3 * nsubdivs) / 4) as Real,
    ];

    let half_profile_len = out_vtx.len();

    for k in 0..half_profile_len as u32 - 1 {
        out_idx.push([k, k + 1]);
    }

    let mut range = 0..half_profile_len;

    if collapse_bottom {
        range.start += 1;
    }
    if collapse_top {
        range.end -= 1;
    }

    // Push rotated profiles.
    for angle in angles {
        let base = out_vtx.len() as u32;
        let rot = crate::math::Rot3::from_axis_angle(Vector::Y, angle);

        if collapse_bottom {
            out_idx.push([0, base]);
        }

        for k in range.clone() {
            out_vtx.push(rot * out_vtx[k]);
        }

        for k in 0..range.len() as u32 - 1 {
            out_idx.push([base + k, base + k + 1]);
        }

        if collapse_top {
            out_idx.push([base + range.len() as u32 - 1, half_profile_len as u32 - 1]);
        }
    }

    // Push circles.
    // TODO: right now, this duplicates some points, to simplify the index
    //       buffer construction.
    for circle_range in circle_ranges {
        for i in circle_range.clone() {
            let pt = out_vtx[i as usize];
            let base = out_vtx.len() as u32;
            push_circle(
                crate::math::Vector2::new(pt.x, pt.z).length(),
                nsubdivs,
                ang_increment,
                pt.y,
                out_vtx,
            );
            push_circle_outline_indices(out_idx, base..base + nsubdivs)
        }
    }
}
