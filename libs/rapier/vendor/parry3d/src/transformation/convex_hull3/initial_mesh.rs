use super::{ConvexHullError, TriangleFacet};
use crate::math::{MatExt, Vector2, Vector3};
use crate::shape::Triangle;
use crate::transformation;
use crate::transformation::convex_hull_utils::support_point_id;
use crate::utils;
use alloc::{vec, vec::Vec};
use core::cmp::Ordering;

#[derive(Debug)]
pub enum InitialMesh {
    Facets(Vec<TriangleFacet>),
    ResultMesh(Vec<Vector3>, Vec<[u32; 3]>),
}

fn build_degenerate_mesh_point(point: Vector3) -> (Vec<Vector3>, Vec<[u32; 3]>) {
    let ta = [0u32; 3];
    let tb = [0u32; 3];

    (vec![point], vec![ta, tb])
}

fn build_degenerate_mesh_segment(
    dir: Vector3,
    points: &[Vector3],
) -> (Vec<Vector3>, Vec<[u32; 3]>) {
    let a = utils::point_cloud_support_point(dir, points);
    let b = utils::point_cloud_support_point(-dir, points);

    let ta = [0u32, 1, 0];
    let tb = [1u32, 0, 0];

    (vec![a, b], vec![ta, tb])
}

pub fn try_get_initial_mesh(
    original_points: &[Vector3],
    normalized_points: &mut [Vector3],
    undecidable: &mut Vec<usize>,
) -> Result<InitialMesh, ConvexHullError> {
    /*
     * Compute the eigenvectors to see if the input data live on a subspace.
     */
    let eigvec;
    let eigval;

    #[cfg(not(feature = "improved_fixed_point_support"))]
    {
        let cov_mat = utils::cov(normalized_points);
        let eig = cov_mat.symmetric_eigen();
        eigvec = eig.eigenvectors;
        eigval = eig.eigenvalues;
    }

    #[cfg(feature = "improved_fixed_point_support")]
    {
        use crate::math::{Matrix3, MatrixExt};
        eigvec = Matrix3::identity();
        eigval = Vector3::splat(1.0);
    }

    let mut eigpairs = [
        (eigvec.x_axis, eigval[0]),
        (eigvec.y_axis, eigval[1]),
        (eigvec.z_axis, eigval[2]),
    ];

    /*
     * Sort in decreasing order wrt. eigenvalues.
     */
    eigpairs.sort_by(|a, b| {
        if a.1 > b.1 {
            Ordering::Less // `Less` and `Greater` are reversed.
        } else if a.1 < b.1 {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });

    /*
     * Count the dimension the data lives in.
     */
    let mut dimension = 0;
    while dimension < 3 {
        if relative_eq!(eigpairs[dimension].1, 0.0, epsilon = 1.0e-7) {
            break;
        }

        dimension += 1;
    }

    match dimension {
        0 => {
            // The hull is a point.
            let (vtx, idx) = build_degenerate_mesh_point(original_points[0]);
            Ok(InitialMesh::ResultMesh(vtx, idx))
        }
        1 => {
            // The hull is a segment.
            let (vtx, idx) = build_degenerate_mesh_segment(eigpairs[0].0, original_points);
            Ok(InitialMesh::ResultMesh(vtx, idx))
        }
        2 => {
            // The hull is a triangle.
            // Project into the principal halfspace…
            let axis1 = eigpairs[0].0;
            let axis2 = eigpairs[1].0;

            let mut subspace_points = Vec::with_capacity(normalized_points.len());

            for point in normalized_points.iter() {
                subspace_points.push(Vector2::new(point.dot(axis1), point.dot(axis2)))
            }

            // … and compute the 2d convex hull.
            let idx = transformation::convex_hull2_idx(&subspace_points[..]);

            // Finalize the result, triangulating the polyline.
            let npoints = idx.len();
            let coords = idx.into_iter().map(|i| original_points[i]).collect();
            let mut triangles = Vec::with_capacity(npoints + npoints - 4);

            for id in 1u32..npoints as u32 - 1 {
                triangles.push([0, id, id + 1]);
            }

            // NOTE: We use a different starting point for the triangulation
            // of the bottom faces in order to avoid bad topology where
            // and edge would end be being shared by more than two triangles.
            for id in 0u32..npoints as u32 - 2 {
                let a = npoints as u32 - 1;
                triangles.push([a, id + 1, id]);
            }

            Ok(InitialMesh::ResultMesh(coords, triangles))
        }
        3 => {
            // The hull is a polyhedron.
            // Find a initial triangle lying on the principal halfspace…
            let center = utils::center(normalized_points);

            // Get the maximum absolute value from the eigenvalues
            let max_eigval = {
                let ax = eigval.x.abs();
                let ay = eigval.y.abs();
                let az = eigval.z.abs();
                ax.max(ay).max(az)
            };

            for point in normalized_points.iter_mut() {
                *point = Vector3::from((*point - center) / max_eigval);
            }

            let p1 = support_point_id(eigpairs[0].0, normalized_points)
                .ok_or(ConvexHullError::MissingSupportPoint)?;
            let p2 = support_point_id(-eigpairs[0].0, normalized_points)
                .ok_or(ConvexHullError::MissingSupportPoint)?;

            let mut max_area = 0.0;
            let mut p3 = usize::MAX;

            for (i, point) in normalized_points.iter().enumerate() {
                let area =
                    Triangle::new(normalized_points[p1], normalized_points[p2], *point).area();

                if area > max_area {
                    max_area = area;
                    p3 = i;
                }
            }

            if p3 == usize::MAX {
                Err(ConvexHullError::InternalError("no triangle found."))
            } else {
                // Build two facets with opposite normals
                let mut f1 = TriangleFacet::new(p1, p2, p3, normalized_points);
                let mut f2 = TriangleFacet::new(p2, p1, p3, normalized_points);

                // Link the facets together
                f1.set_facets_adjascency(1, 1, 1, 0, 2, 1);
                f2.set_facets_adjascency(0, 0, 0, 0, 2, 1);

                let mut facets = vec![f1, f2];

                // … and attribute visible points to each one of them.
                // TODO: refactor this with the two others.
                for point in 0..normalized_points.len() {
                    if normalized_points[point] == normalized_points[p1]
                        || normalized_points[point] == normalized_points[p2]
                        || normalized_points[point] == normalized_points[p3]
                    {
                        continue;
                    }

                    let mut furthest = usize::MAX;
                    let mut furthest_dist = 0.0;

                    for (i, curr_facet) in facets.iter().enumerate() {
                        if curr_facet.can_see_point(point, normalized_points) {
                            let distance = curr_facet.distance_to_point(point, normalized_points);

                            if distance > furthest_dist {
                                furthest = i;
                                furthest_dist = distance;
                            }
                        }
                    }

                    if furthest != usize::MAX {
                        facets[furthest].add_visible_point(point, normalized_points);
                    } else {
                        undecidable.push(point);
                    }

                    // If none of the facet can be seen from the point, it is naturally deleted.
                }

                super::check_facet_links(0, &facets[..]);
                super::check_facet_links(1, &facets[..]);

                Ok(InitialMesh::Facets(facets))
            }
        }
        _ => Err(ConvexHullError::Unreachable),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(feature = "f32")]
    fn try_get_initial_mesh_should_fail_for_missing_support_points() {
        use super::*;
        use crate::math::Vector;
        use crate::transformation::try_convex_hull;

        let point_cloud = vec![
            Vector::new(103.05024, 303.44974, 106.125),
            Vector::new(103.21692, 303.44974, 106.125015),
            Vector::new(104.16538, 303.44974, 106.125),
            Vector::new(106.55025, 303.44974, 106.125),
            Vector::new(106.55043, 303.44974, 106.125),
        ];
        let result = try_convex_hull(&point_cloud);
        assert!(result.is_ok());
    }
}
