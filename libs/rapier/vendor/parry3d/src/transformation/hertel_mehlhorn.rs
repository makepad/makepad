//! Hertel-Mehlhorn algorithm for convex partitioning.
//! Based on <https://github.com/ivanfratric/polypartition>, contributed by embotech AG.

use alloc::vec::Vec;

use crate::math::Vector;
use crate::utils::point_in_triangle::{corner_direction, Orientation};

/// Checks if the counter-clockwise polygon `poly` has an edge going counter-clockwise from `p1` to `p2`.
/// Returns the edge point's indices in the second polygon. Returns `None` if none were found.
fn find_edge_index_in_polygon(p1: u32, p2: u32, indices: &[u32]) -> Option<(usize, usize)> {
    for i1 in 0..indices.len() {
        let i2 = (i1 + 1) % indices.len();
        if p1 == indices[i1] && p2 == indices[i2] {
            return Some((i1, i2));
        }
    }
    None
}

/// The Hertel-Mehlhorn algorithm.
///
/// Takes a set of triangles and returns a vector of convex polygons.
///
/// Time/Space complexity: O(n^2)/O(n) Where `n` is the number of points in the input polygon.
///
/// Quality of solution: This algorithm is a heuristic. At most four times the minimum number of convex
/// polygons is created. However, in practice it works much better than that and often returns the optimal
/// partitioning.
///
/// This algorithm is described in <https://people.mpi-inf.mpg.de/~mehlhorn/ftp/FastTriangulation.pdf>.
pub fn hertel_mehlhorn(vertices: &[Vector], indices: &[[u32; 3]]) -> Vec<Vec<Vector>> {
    hertel_mehlhorn_idx(vertices, indices)
        .into_iter()
        .map(|poly_indices| {
            poly_indices
                .into_iter()
                .map(|idx| vertices[idx as usize])
                .collect()
        })
        .collect()
}

/// Internal implementation of the Hertel-Mehlhorn algorithm that returns the polygon indices.
pub fn hertel_mehlhorn_idx(vertices: &[Vector], indices: &[[u32; 3]]) -> Vec<Vec<u32>> {
    let mut indices: Vec<Vec<u32>> = indices.iter().map(|indices| indices.to_vec()).collect();
    // Iterate over all polygons.
    let mut i_poly1 = 0;
    while i_poly1 < indices.len() {
        // Iterate over their points.
        let mut i11 = 0;
        while i11 < indices[i_poly1].len() {
            let polygon1 = &indices[i_poly1];

            // Get the next point index.
            let i12 = (i11 + 1) % polygon1.len();

            // Get the current edge.
            let edge_start = polygon1[i11];
            let edge_end = polygon1[i12];

            // Iterate over the remaining polygons and find an edge to the current polygon.
            let (i_poly2, edge_indices) = match indices
                .iter()
                .enumerate()
                .skip(i_poly1 + 1)
                .find_map(|(i, poly_indices)| {
                    // Check if the edge is in the second polygon. Start and end are switched because
                    // the edge direction is flipped in the second polygon.
                    find_edge_index_in_polygon(edge_end, edge_start, poly_indices)
                        .map(|edge_indices| (i, edge_indices))
                }) {
                Some(found) => found,
                None => {
                    // Go to the next point if there was no common edge.
                    i11 += 1;
                    continue;
                }
            };

            // Check if the connections between the polygons are convex:
            let (i21, i22) = edge_indices;
            let polygon2 = &indices[i_poly2];

            // First connection:
            let i13 = (polygon1.len() + i11 - 1) % polygon1.len();
            let i23 = (i22 + 1) % polygon2.len();
            let p1 = vertices[polygon2[i23] as usize];
            let p2 = vertices[polygon1[i13] as usize];
            let p3 = vertices[polygon1[i11] as usize];
            // Go to the next point if this section isn't convex.
            if corner_direction(p1, p2, p3) == Orientation::Cw {
                i11 += 1;
                continue;
            }
            // Second connection:
            let i13 = (i12 + 1) % polygon1.len();
            let i23 = (polygon2.len() + i21 - 1) % polygon2.len();
            let p1 = vertices[polygon1[i13] as usize];
            let p2 = vertices[polygon2[i23] as usize];
            let p3 = vertices[polygon1[i12] as usize];
            // Go to the next point if this section isn't convex.
            if corner_direction(p1, p2, p3) == Orientation::Cw {
                i11 += 1;
                continue;
            }

            // Connection is convex, merge the polygons.
            let mut new_polygon = Vec::with_capacity(polygon1.len() + polygon2.len() - 2);
            new_polygon.extend(polygon1.iter().cycle().skip(i12).take(polygon1.len() - 1));
            new_polygon.extend(polygon2.iter().cycle().skip(i22).take(polygon2.len() - 1));

            // Remove the polygon from the list.
            let _ = indices.remove(i_poly2);
            // Overwrite the first polygon with the new one.
            indices[i_poly1] = new_polygon;
            // Start from the first point.
            i11 = 0;
        }

        // Go to the next polygon.
        i_poly1 += 1;
    }

    indices
}

// --- Unit tests ----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::hertel_mehlhorn_idx;
    use crate::math::Vector;

    #[test]
    fn origin_outside_shape() {
        // Expected result of convex decomposition:
        // 4-----------------------3
        // |  .       (2)       .  |
        // |    .             .    |
        // |       7-------0       |
        // |  (1)  |       |  (3)  |
        // |       |   °   |       |
        // 5-------6       1-------2
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

        let triangles = [
            [5, 6, 7],
            [4, 5, 7],
            [3, 4, 7],
            [3, 7, 0],
            [2, 3, 0],
            [2, 0, 1],
        ];

        let indices = hertel_mehlhorn_idx(&vertices, &triangles);

        let expected_indices = vec![
            // (1)
            vec![5, 6, 7, 4],
            // (2)
            vec![3, 4, 7, 0],
            // (3)
            vec![2, 3, 0, 1],
        ];

        assert_eq!(indices, expected_indices);
    }
}
