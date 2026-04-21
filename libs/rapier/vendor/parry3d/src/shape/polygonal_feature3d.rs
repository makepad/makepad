use crate::math::{Pose, Vector};
use crate::shape::{PackedFeatureId, Segment, Triangle};
#[cfg(feature = "alloc")]
use crate::{
    math::{Real, Vector2},
    query::details::closest_points_segment_segment_2d,
    query::{ContactManifold, TrackedContact},
    utils::WBasis,
};

/// A polygonal feature representing the local polygonal approximation of
/// a vertex, face, or edge of a convex shape.
#[derive(Debug, Clone)]
pub struct PolygonalFeature {
    /// Up to four vertices forming this polygonal feature.
    pub vertices: [Vector; 4],
    /// The feature IDs of this polygon's vertices.
    pub vids: [PackedFeatureId; 4],
    /// The feature IDs of this polygon's edges.
    pub eids: [PackedFeatureId; 4],
    /// The feature ID of this polygonal feature.
    pub fid: PackedFeatureId,
    /// The number of vertices on this polygon (must be <= 4).
    pub num_vertices: usize,
}

impl Default for PolygonalFeature {
    fn default() -> Self {
        Self {
            vertices: [Vector::ZERO; 4],
            vids: [PackedFeatureId::UNKNOWN; 4],
            eids: [PackedFeatureId::UNKNOWN; 4],
            fid: PackedFeatureId::UNKNOWN,
            num_vertices: 0,
        }
    }
}

impl From<Triangle> for PolygonalFeature {
    fn from(tri: Triangle) -> Self {
        Self {
            vertices: [tri.a, tri.b, tri.c, tri.c],
            vids: PackedFeatureId::vertices([0, 1, 2, 2]),
            eids: PackedFeatureId::edges([0, 1, 2, 2]),
            fid: PackedFeatureId::face(0),
            num_vertices: 3,
        }
    }
}

impl From<Segment> for PolygonalFeature {
    fn from(seg: Segment) -> Self {
        Self {
            vertices: [seg.a, seg.b, seg.b, seg.b],
            vids: PackedFeatureId::vertices([0, 1, 1, 1]),
            eids: PackedFeatureId::edges([0, 0, 0, 0]),
            fid: PackedFeatureId::face(0),
            num_vertices: 2,
        }
    }
}

impl PolygonalFeature {
    /// Creates a new empty polygonal feature.
    pub fn new() -> Self {
        Self::default()
    }

    /// Transform each vertex of this polygonal feature by the given position `pos`.
    pub fn transform_by(&mut self, pos: &Pose) {
        for p in &mut self.vertices[0..self.num_vertices] {
            *p = pos * *p;
        }
    }

    /// Computes all the contacts between two polygonal features.
    #[cfg(feature = "alloc")]
    pub fn contacts<ManifoldData, ContactData: Default + Copy>(
        pos12: &Pose,
        _pos21: &Pose,
        sep_axis1: Vector,
        _sep_axis2: Vector,
        feature1: &Self,
        feature2: &Self,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
        flipped: bool,
    ) {
        match (feature1.num_vertices, feature2.num_vertices) {
            (2, 2) => {
                Self::contacts_edge_edge(pos12, feature1, sep_axis1, feature2, manifold, flipped)
            }
            _ => Self::contacts_face_face(pos12, feature1, sep_axis1, feature2, manifold, flipped),
        }
    }

    #[cfg(feature = "alloc")]
    fn contacts_edge_edge<ManifoldData, ContactData: Default + Copy>(
        pos12: &Pose,
        face1: &PolygonalFeature,
        sep_axis1: Vector,
        face2: &PolygonalFeature,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
        flipped: bool,
    ) {
        // Project the faces to a 2D plane for contact clipping.
        // The plane they are projected onto has normal sep_axis1
        // and contains the origin (this is numerically OK because
        // we are not working in world-space here).
        let basis = sep_axis1.orthonormal_basis();
        let projected_edge1 = [
            Vector2::new(
                face1.vertices[0].dot(basis[0]),
                face1.vertices[0].dot(basis[1]),
            ),
            Vector2::new(
                face1.vertices[1].dot(basis[0]),
                face1.vertices[1].dot(basis[1]),
            ),
        ];

        let vertices2_1 = [pos12 * face2.vertices[0], pos12 * face2.vertices[1]];
        let projected_edge2 = [
            Vector2::new(vertices2_1[0].dot(basis[0]), vertices2_1[0].dot(basis[1])),
            Vector2::new(vertices2_1[1].dot(basis[0]), vertices2_1[1].dot(basis[1])),
        ];

        let tangent1 = (projected_edge1[1] - projected_edge1[0]).try_normalize();
        let tangent2 = (projected_edge2[1] - projected_edge2[0]).try_normalize();

        // TODO: not sure what the best value for eps is.
        // Empirically, it appears that an epsilon smaller than 1.0e-3 is too small.
        if let (Some(tangent1), Some(tangent2)) = (tangent1, tangent2) {
            let parallel = tangent1.dot(tangent2) >= crate::utils::COS_FRAC_PI_8;

            if !parallel {
                let seg1 = (&projected_edge1[0], &projected_edge1[1]);
                let seg2 = (&projected_edge2[0], &projected_edge2[1]);
                let (loc1, loc2) = closest_points_segment_segment_2d(seg1, seg2);

                // Found a contact between the two edges.
                let bcoords1 = loc1.barycentric_coordinates();
                let bcoords2 = loc2.barycentric_coordinates();

                let edge1 = (face1.vertices[0], face1.vertices[1]);
                let edge2 = (vertices2_1[0], vertices2_1[1]);
                let local_p1 = edge1.0 * bcoords1[0] + edge1.1 * bcoords1[1];
                let local_p2_1 = edge2.0 * bcoords2[0] + edge2.1 * bcoords2[1];
                let dist = (local_p2_1 - local_p1).dot(sep_axis1);

                manifold.points.push(TrackedContact::flipped(
                    local_p1,
                    pos12.inverse_transform_point(local_p2_1),
                    face1.eids[0],
                    face2.eids[0],
                    dist,
                    flipped,
                ));

                return;
            }
        }

        // The lines are parallel so we are having a conformal contact.
        // Let's use a range-based clipping to extract two contact points.
        // TODO: would it be better and/or more efficient to do the
        //clipping in 2D?
        if let Some(clips) = crate::query::details::clip_segment_segment(
            (face1.vertices[0], face1.vertices[1]),
            (vertices2_1[0], vertices2_1[1]),
        ) {
            let feature_at = |face: &PolygonalFeature, id| match id {
                0 => face.vids[0],
                1 => face.eids[0],
                2 => face.vids[1],
                _ => unreachable!(),
            };

            manifold.points.push(TrackedContact::flipped(
                (clips.0).0,
                pos12.inverse_transform_point((clips.0).1),
                feature_at(face1, (clips.0).2),
                feature_at(face2, (clips.0).3),
                ((clips.0).1 - (clips.0).0).dot(sep_axis1),
                flipped,
            ));

            manifold.points.push(TrackedContact::flipped(
                (clips.1).0,
                pos12.inverse_transform_point((clips.1).1),
                feature_at(face1, (clips.1).2),
                feature_at(face2, (clips.1).3),
                ((clips.1).1 - (clips.1).0).dot(sep_axis1),
                flipped,
            ));
        }
    }

    #[cfg(feature = "alloc")]
    fn contacts_face_face<ManifoldData, ContactData: Default + Copy>(
        pos12: &Pose,
        face1: &PolygonalFeature,
        sep_axis1: Vector,
        face2: &PolygonalFeature,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
        flipped: bool,
    ) {
        // Project the faces to a 2D plane for contact clipping.
        // The plane they are projected onto has normal sep_axis1
        // and contains the origin (this is numerically OK because
        // we are not working in world-space here).
        let basis = sep_axis1.orthonormal_basis();
        let projected_face1 = [
            Vector2::new(
                face1.vertices[0].dot(basis[0]),
                face1.vertices[0].dot(basis[1]),
            ),
            Vector2::new(
                face1.vertices[1].dot(basis[0]),
                face1.vertices[1].dot(basis[1]),
            ),
            Vector2::new(
                face1.vertices[2].dot(basis[0]),
                face1.vertices[2].dot(basis[1]),
            ),
            Vector2::new(
                face1.vertices[3].dot(basis[0]),
                face1.vertices[3].dot(basis[1]),
            ),
        ];

        let vertices2_1 = [
            pos12 * face2.vertices[0],
            pos12 * face2.vertices[1],
            pos12 * face2.vertices[2],
            pos12 * face2.vertices[3],
        ];
        let projected_face2 = [
            Vector2::new(vertices2_1[0].dot(basis[0]), vertices2_1[0].dot(basis[1])),
            Vector2::new(vertices2_1[1].dot(basis[0]), vertices2_1[1].dot(basis[1])),
            Vector2::new(vertices2_1[2].dot(basis[0]), vertices2_1[2].dot(basis[1])),
            Vector2::new(vertices2_1[3].dot(basis[0]), vertices2_1[3].dot(basis[1])),
        ];

        // Also find all the vertices located inside of the other projected face.
        if face2.num_vertices > 2 {
            let normal2_1 =
                (vertices2_1[2] - vertices2_1[1]).cross(vertices2_1[0] - vertices2_1[1]);
            let denom = normal2_1.dot(sep_axis1);

            if !relative_eq!(denom, 0.0) {
                let last_index2 = face2.num_vertices - 1;

                #[allow(clippy::needless_range_loop)] // Would make it much more verbose.
                'point_loop1: for i in 0..face1.num_vertices {
                    let p1 = projected_face1[i];

                    let mut sign = (projected_face2[0] - projected_face2[last_index2])
                        .perp_dot(p1 - projected_face2[last_index2]);
                    for j in 0..last_index2 {
                        let new_sign = (projected_face2[j + 1] - projected_face2[j])
                            .perp_dot(p1 - projected_face2[j]);

                        if sign == 0.0 {
                            sign = new_sign;
                        } else if sign * new_sign < 0.0 {
                            // The point lies outside.
                            continue 'point_loop1;
                        }
                    }

                    // All the perp had the same sign: the point is inside of the other shapes projection.
                    // Output the contact.
                    let dist = (vertices2_1[0] - face1.vertices[i]).dot(normal2_1) / denom;
                    let local_p1 = face1.vertices[i];
                    let local_p2_1 = face1.vertices[i] + dist * sep_axis1;

                    manifold.points.push(TrackedContact::flipped(
                        local_p1,
                        pos12.inverse_transform_point(local_p2_1),
                        face1.vids[i],
                        face2.fid,
                        dist,
                        flipped,
                    ));
                }
            }
        }

        if face1.num_vertices > 2 {
            let normal1 = (face1.vertices[2] - face1.vertices[1])
                .cross(face1.vertices[0] - face1.vertices[1]);

            let denom = -normal1.dot(sep_axis1);
            if !relative_eq!(denom, 0.0) {
                let last_index1 = face1.num_vertices - 1;
                'point_loop2: for i in 0..face2.num_vertices {
                    let p2 = projected_face2[i];

                    let mut sign = (projected_face1[0] - projected_face1[last_index1])
                        .perp_dot(p2 - projected_face1[last_index1]);
                    for j in 0..last_index1 {
                        let new_sign = (projected_face1[j + 1] - projected_face1[j])
                            .perp_dot(p2 - projected_face1[j]);

                        if sign == 0.0 {
                            sign = new_sign;
                        } else if sign * new_sign < 0.0 {
                            // The point lies outside.
                            continue 'point_loop2;
                        }
                    }

                    // All the perp had the same sign: the point is inside of the other shapes projection.
                    // Output the contact.
                    let dist = (face1.vertices[0] - vertices2_1[i]).dot(normal1) / denom;
                    let local_p2_1 = vertices2_1[i];
                    let local_p1 = vertices2_1[i] - dist * sep_axis1;

                    manifold.points.push(TrackedContact::flipped(
                        local_p1,
                        pos12.inverse_transform_point(local_p2_1),
                        face1.fid,
                        face2.vids[i],
                        dist,
                        flipped,
                    ));
                }
            }
        }

        // Now we have to compute the intersection between all pairs of
        // edges from the face 1 and from the face2.
        for j in 0..face2.num_vertices {
            let projected_edge2 = [
                projected_face2[j],
                projected_face2[(j + 1) % face2.num_vertices],
            ];

            for i in 0..face1.num_vertices {
                let projected_edge1 = [
                    projected_face1[i],
                    projected_face1[(i + 1) % face1.num_vertices],
                ];
                if let Some(bcoords) = closest_points_line2d(projected_edge1, projected_edge2) {
                    if bcoords.0 > 0.0 && bcoords.0 < 1.0 && bcoords.1 > 0.0 && bcoords.1 < 1.0 {
                        // Found a contact between the two edges.
                        let edge1 = (
                            face1.vertices[i],
                            face1.vertices[(i + 1) % face1.num_vertices],
                        );
                        let edge2 = (vertices2_1[j], vertices2_1[(j + 1) % face2.num_vertices]);
                        let local_p1 = edge1.0 * (1.0 - bcoords.0) + edge1.1 * bcoords.0;
                        let local_p2_1 = edge2.0 * (1.0 - bcoords.1) + edge2.1 * bcoords.1;
                        let dist = (local_p2_1 - local_p1).dot(sep_axis1);

                        manifold.points.push(TrackedContact::flipped(
                            local_p1,
                            pos12.inverse_transform_point(local_p2_1),
                            face1.eids[i],
                            face2.eids[j],
                            dist,
                            flipped,
                        ));
                    }
                }
            }
        }
    }
}

/// Compute the barycentric coordinates of the intersection between the two given lines.
/// Returns `None` if the lines are parallel.
#[cfg(feature = "alloc")]
fn closest_points_line2d(edge1: [Vector2; 2], edge2: [Vector2; 2]) -> Option<(Real, Real)> {
    use crate::approx::AbsDiffEq;

    // Inspired by Real-time collision detection by Christer Ericson.
    let dir1 = edge1[1] - edge1[0];
    let dir2 = edge2[1] - edge2[0];
    let r = edge1[0] - edge2[0];

    let a = dir1.length_squared();
    let e = dir2.length_squared();
    let f = dir2.dot(r);

    let eps = Real::default_epsilon();

    if a <= eps && e <= eps {
        Some((0.0, 0.0))
    } else if a <= eps {
        Some((0.0, f / e))
    } else {
        let c = dir1.dot(r);
        if e <= eps {
            Some((-c / a, 0.0))
        } else {
            let b = dir1.dot(dir2);
            let ae = a * e;
            let bb = b * b;
            let denom = ae - bb;

            // Use absolute and ulps error to test collinearity.
            let parallel = denom <= eps || approx::ulps_eq!(ae, bb);

            let s = if !parallel {
                (b * f - c * e) / denom
            } else {
                0.0
            };

            if parallel {
                None
            } else {
                Some((s, (b * s + f) / e))
            }
        }
    }
}
