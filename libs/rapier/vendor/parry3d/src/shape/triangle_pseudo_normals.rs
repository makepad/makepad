use crate::math::Vector;

#[cfg(feature = "alloc")]
use crate::{math::Vector3, query::details::NormalConstraints};

// NOTE: ideally, the normal cone should take into account the point where the normal cone is
//       considered. But as long as we assume that the triangles are one-way we can get away with
//       just relying on the normal directions.
//       Taking the point into account would be technically doable (and desirable if we wanted
//       to define, e.g., a one-way mesh) but requires:
//       1. To make sure the edge pseudo-normals are given in the correct edge order.
//       2. To have access to the contact feature.
//       We can have access to both during the narrow-phase, but leave that as a future
//       potential improvements.
// NOTE: this isn’t equal to the "true" normal cones since concave edges will have pseudo-normals
//       still pointing outward (instead of inward or being empty).
/// The pseudo-normals of a triangle providing approximations of its feature’s normal cones.
#[derive(Clone, Debug)]
pub struct TrianglePseudoNormals {
    /// The triangle’s face normal.
    pub face: Vector,
    // TODO: if we switch this to providing pseudo-normals in a specific order
    //       (e.g. in the same order as the triangle’s edges), then we should
    //       think of fixing that order in the heightfield
    //       triangle_pseudo_normals code.
    /// The edges pseudo-normals, in no particular order.
    pub edges: [Vector; 3],
}

#[cfg(feature = "alloc")]
impl NormalConstraints for TrianglePseudoNormals {
    /// Projects the given direction to it is contained in the polygonal
    /// cone defined `self`.
    fn project_local_normal_mut(&self, dir: &mut Vector) -> bool {
        let dot_face = dir.dot(self.face);

        // Find the closest pseudo-normal.
        let dots = Vector3::new(
            dir.dot(self.edges[0]),
            dir.dot(self.edges[1]),
            dir.dot(self.edges[2]),
        );
        let closest_dot = dots.max_position();
        let closest_edge = self.edges[closest_dot];

        // Apply the projection. Note that this isn’t 100% accurate since this approximates the
        // vertex normal cone using the closest edge’s normal cone instead of the
        // true polygonal cone on S² (but taking into account this polygonal cone exactly
        // would be quite computationally expensive).

        if closest_edge == self.face {
            // The normal cone is degenerate, there is only one possible direction.
            *dir = self.face;
            return dot_face >= 0.0;
        }

        // TODO: take into account the two closest edges instead for better continuity
        //       of vertex normals?
        let dot_edge_face = self.face.dot(closest_edge);
        let dot_dir_face = self.face.dot(*dir);
        let dot_corrected_dir_face = 2.0 * dot_edge_face * dot_edge_face - 1.0; // cos(2 * angle(closest_edge, face))

        if dot_dir_face >= dot_corrected_dir_face {
            // The direction is in the pseudo-normal cone. No correction to apply.
            return true;
        }

        // We need to correct.
        let edge_on_normal = self.face * dot_edge_face;
        let edge_orthogonal_to_normal = closest_edge - edge_on_normal;

        let dir_on_normal = self.face * dot_dir_face;
        let dir_orthogonal_to_normal = *dir - dir_on_normal;
        let Some(unit_dir_orthogonal_to_normal) = dir_orthogonal_to_normal.try_normalize() else {
            return dot_face >= 0.0;
        };

        // NOTE: the normalization might be redundant as the result vector is guaranteed to be
        //       unit sized. Though some rounding errors could throw it off.
        let adjusted_pseudo_normal =
            edge_on_normal + unit_dir_orthogonal_to_normal * edge_orthogonal_to_normal.length();
        let (adjusted_pseudo_normal, length) = adjusted_pseudo_normal.normalize_and_length();
        if length <= 1.0e-6 {
            return dot_face >= 0.0;
        };

        // The reflection of the face normal wrt. the adjusted pseudo-normal gives us the
        // second end of the pseudo-normal cone the direction is projected on.
        *dir = adjusted_pseudo_normal * (2.0 * self.face.dot(adjusted_pseudo_normal)) - self.face;
        dot_face >= 0.0
    }
}

#[cfg(test)]
#[cfg(all(feature = "dim3", feature = "alloc"))]
mod test {
    use super::NormalConstraints;
    use crate::math::{Real, Vector};
    use crate::shape::TrianglePseudoNormals;

    fn bisector(v1: Vector, v2: Vector) -> Vector {
        (v1 + v2).normalize()
    }

    fn bisector_y(v: Vector) -> Vector {
        bisector(v, Vector::Y)
    }

    #[test]
    fn trivial_pseudo_normals_projection() {
        let pn = TrianglePseudoNormals {
            face: Vector::Y,
            edges: [Vector::Y; 3],
        };

        assert_eq!(
            pn.project_local_normal(Vector::new(1.0, 1.0, 1.0)),
            Some(Vector::Y)
        );
        assert!(pn.project_local_normal(-Vector::Y).is_none());
    }

    #[test]
    fn edge_pseudo_normals_projection_strictly_positive() {
        let bisector = |v1: Vector, v2: Vector| (v1 + v2).normalize();
        let bisector_y = |v: Vector| bisector(v, Vector::Y);

        // The normal cones for this test will be fully contained in the +Y half-space.
        let cones_ref_dir = [
            -Vector::Z,
            -Vector::X,
            Vector::new(1.0, 0.0, 1.0).normalize(),
        ];
        let cones_ends = cones_ref_dir.map(bisector_y);
        let cones_axes = cones_ends.map(bisector_y);

        let pn = TrianglePseudoNormals {
            face: Vector::Y,
            edges: cones_axes.map(|v| v.normalize()),
        };

        for i in 0..3 {
            assert!(pn
                .project_local_normal(cones_ends[i])
                .unwrap()
                .abs_diff_eq(cones_ends[i], 1.0e-5));
            assert_eq!(pn.project_local_normal(cones_axes[i]), Some(cones_axes[i]));

            // Guaranteed to be inside the normal cone of edge i.
            let subdivs = 100;

            for k in 1..100 {
                let v = Vector::Y
                    .lerp(cones_ends[i], k as Real / (subdivs as Real))
                    .normalize();
                assert_eq!(pn.project_local_normal(v).unwrap(), v);
            }

            // Guaranteed to be outside the normal cone of edge i.
            for k in 1..subdivs {
                let v = cones_ref_dir[i]
                    .lerp(cones_ends[i], k as Real / (subdivs as Real))
                    .normalize();
                assert!(pn
                    .project_local_normal(v)
                    .unwrap()
                    .abs_diff_eq(cones_ends[i], 1.0e-5));
            }

            // Guaranteed to be outside the normal cone, and in the -Y half-space.
            for k in 1..subdivs {
                let v = cones_ref_dir[i]
                    .lerp(-Vector::Y, k as Real / (subdivs as Real))
                    .normalize();
                assert!(pn.project_local_normal(v).is_none(),);
            }
        }
    }

    #[test]
    fn edge_pseudo_normals_projection_negative() {
        // The normal cones for this test will be fully contained in the +Y half-space.
        let cones_ref_dir = [
            -Vector::Z,
            -Vector::X,
            Vector::new(1.0, 0.0, 1.0).normalize(),
        ];
        let cones_ends = cones_ref_dir.map(|v| bisector(v, -Vector::Y));
        let cones_axes = [
            bisector(bisector_y(cones_ref_dir[0]), cones_ref_dir[0]),
            bisector(bisector_y(cones_ref_dir[1]), cones_ref_dir[1]),
            bisector(bisector_y(cones_ref_dir[2]), cones_ref_dir[2]),
        ];

        let pn = TrianglePseudoNormals {
            face: Vector::Y,
            edges: cones_axes.map(|v| v.normalize()),
        };

        for i in 0..3 {
            assert_eq!(pn.project_local_normal(cones_axes[i]), Some(cones_axes[i]));

            // Guaranteed to be inside the normal cone of edge i.
            let subdivs = 100;

            for k in 1..subdivs {
                let v = Vector::Y
                    .lerp(cones_ends[i], k as Real / (subdivs as Real))
                    .normalize();
                assert_eq!(pn.project_local_normal(v).unwrap(), v);
            }

            // Guaranteed to be outside the normal cone of edge i.
            // Since it is additionally guaranteed to be in the -Y half-space, we should get None.
            for k in 1..subdivs {
                let v = (-Vector::Y)
                    .lerp(cones_ends[i], k as Real / (subdivs as Real))
                    .normalize();
                assert!(pn.project_local_normal(v).is_none());
            }
        }
    }
}
