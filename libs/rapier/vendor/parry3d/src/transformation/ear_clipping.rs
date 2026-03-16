//! Ear-clipping algorithm for creating a triangle mesh from a simple polygon.
//! Based on <https://github.com/ivanfratric/polypartition>, contributed by embotech AG.

use crate::{
    math::{Real, Vector},
    utils::point_in_triangle::{corner_direction, is_point_in_triangle, Orientation},
};
use alloc::{vec, vec::Vec};

/// The information stored for each vertex in the ear clipping algorithm.
#[derive(Clone, Default)]
struct VertexInfo {
    /// Whether the vertex is still active i.e. it has not been clipped yet.
    is_active: bool,
    /// Whether the vertex is the tip of an ear and should be clipped.
    is_ear: bool,
    /// How small the angle of the ear is. Ears with a smaller angle are clipped first.
    pointiness: Real,
    /// The index of the previous vertex.
    p_prev: usize,
    /// The index of the next vertex.
    p_next: usize,
}

/// Updates the fields `pointiness` and `is_ear` for a given vertex index.
fn update_vertex(idx: usize, vertex_info: &mut VertexInfo, points: &[Vector]) -> bool {
    // Get the point and its neighbors.
    let p = points[idx];
    let p1 = points[vertex_info.p_prev];
    let p3 = points[vertex_info.p_next];

    // Get the pointiness.
    let vec1 = (p1 - p).normalize();
    let vec3 = (p3 - p).normalize();
    vertex_info.pointiness = vec1.dot(vec3);
    if vertex_info.pointiness.is_nan() {
        return false;
    }

    // A point is considered an ear when it is convex and no other points are
    // inside the triangle spanned by it and its two neighbors.
    let mut error = false;
    vertex_info.is_ear = corner_direction(p1, p, p3) == Orientation::Ccw
        && (0..points.len())
            .filter(|&i| i != vertex_info.p_prev && i != idx && i != vertex_info.p_next)
            .all(|i| {
                if let Some(is) = is_point_in_triangle(points[i], p1, p, p3) {
                    !is
                } else {
                    error = true;
                    true
                }
            });
    !error
}

/// Ear clipping triangulation algorithm.
pub(crate) fn triangulate_ear_clipping(vertices: &[Vector]) -> Option<Vec<[u32; 3]>> {
    let n_vertices = vertices.len();

    // Create a new vector to hold the information about vertices.
    let mut vertex_info = vec![VertexInfo::default(); n_vertices];

    // Initialize information for each vertex.
    let success = vertex_info.iter_mut().enumerate().all(|(i, info)| {
        info.is_active = true;
        info.p_prev = if i == 0 { n_vertices - 1 } else { i - 1 };
        info.p_next = if i == n_vertices - 1 { 0 } else { i + 1 };
        update_vertex(i, info, vertices)
    });
    if !success {
        return None;
    }

    // The output shapes
    let mut output_indices = Vec::new();

    for i in 0..n_vertices - 3 {
        // Search through all active ears and pick out the pointiest.
        let maybe_ear = vertex_info
            .iter()
            .enumerate()
            .filter(|(_, info)| info.is_active && info.is_ear)
            .max_by(|(_, info1), (_, info2)| {
                // The unwrap here is safe since we check for NaN when
                // we assign the pointiness value.
                info1.pointiness.partial_cmp(&info2.pointiness).unwrap()
            });

        // If we found an ear, clip it. Else the algorithm failed.
        let (ear_i, _) = maybe_ear?;

        // Deactivate the tip of the ear.
        vertex_info[ear_i].is_active = false;

        // Get the indices of the neighbors.
        let VertexInfo { p_prev, p_next, .. } = vertex_info[ear_i];

        // Extract the triangle that is the ear and add it to the index buffer.
        let triangle_points = [p_prev as u32, ear_i as u32, p_next as u32];
        output_indices.push(triangle_points);

        // Connect the remaining two vertices.
        vertex_info[p_prev].p_next = vertex_info[ear_i].p_next;
        vertex_info[p_next].p_prev = vertex_info[ear_i].p_prev;

        // Only three vertices remain and those are guaranteed to be convex so
        // there is no point in updating the remaining vertex information.
        if i == n_vertices - 4 {
            break;
        };

        // Update the info for the remaining two vertices.
        if !update_vertex(p_prev, &mut vertex_info[p_prev], vertices)
            || !update_vertex(p_next, &mut vertex_info[p_next], vertices)
        {
            return None;
        }
    }

    // Add the remaining triangle.
    if let Some((i, info)) = vertex_info
        .iter()
        .enumerate()
        .find(|(_, info)| info.is_active)
    {
        let triangle_points = [info.p_prev as u32, i as u32, info.p_next as u32];
        output_indices.push(triangle_points);
    }

    Some(output_indices)
}

// --- Unit tests ----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_ccw() {
        let vertices = vec![
            Vector::new(0., 0.),
            Vector::new(1., 0.),
            Vector::new(1., 1.),
        ];
        let triangles = triangulate_ear_clipping(&vertices);
        assert_eq!(triangles.unwrap(), vec![[2, 0, 1]]);
    }

    #[test]
    fn square_ccw() {
        let vertices = vec![
            Vector::new(0., 0.), // 0
            Vector::new(1., 0.), // 1
            Vector::new(1., 1.), // 2
            Vector::new(0., 1.), // 3
        ];
        let triangles = triangulate_ear_clipping(&vertices);
        assert_eq!(triangles.unwrap(), vec![[2, 3, 0], [2, 0, 1]]);
    }

    #[test]
    fn square_cw() {
        let vertices = vec![
            Vector::new(0., 1.), // 0
            Vector::new(1., 1.), // 1
            Vector::new(1., 0.), // 2
            Vector::new(0., 0.), // 3
        ];
        // This fails because we expect counter-clockwise ordering.
        let triangles = triangulate_ear_clipping(&vertices);
        assert!(triangles.is_none());
    }

    #[test]
    fn square_with_dent() {
        let vertices = vec![
            Vector::new(0., 0.),   // 0
            Vector::new(1., 0.),   // 1
            Vector::new(0.5, 0.5), // 2
            Vector::new(1., 1.),   // 3
            Vector::new(0., 1.),   // 4
        ];
        let triangles = triangulate_ear_clipping(&vertices);
        assert_eq!(triangles.unwrap(), vec![[2, 3, 4], [2, 4, 0], [2, 0, 1],]);
    }

    #[test]
    /// Checks the case where the origin is outside the shape.
    /// 4-----------------------3
    /// |                       |
    /// |                       |
    /// |       7-------0       |
    /// |       |       |       |
    /// |       |   °   |       |
    /// 5-------6       1-------2
    fn origin_outside_shape() {
        let vertices = vec![
            Vector::new(2.0, 2.0),   // 0
            Vector::new(2.0, -2.0),  // 1
            Vector::new(4.0, -2.0),  // 2
            Vector::new(4.0, 4.0),   // 3
            Vector::new(-4.0, 4.0),  // 4
            Vector::new(-4.0, -2.0), // 5
            Vector::new(-2.0, -2.0), // 6
            Vector::new(-2.0, 2.0),  // 7
        ];
        let triangles = triangulate_ear_clipping(&vertices).unwrap();

        assert_eq!(
            triangles,
            vec![
                [5, 6, 7],
                [4, 5, 7],
                [3, 4, 7],
                [3, 7, 0],
                [2, 3, 0],
                [2, 0, 1],
            ]
        );
    }
}
