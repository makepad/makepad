use crate::math::Vector;
#[cfg(feature = "dim3")]
use crate::shape::{Cone, Cylinder, PackedFeatureId};
use crate::shape::{Cuboid, PolygonalFeature, Segment, SupportMap, Triangle};

/// Trait implemented by convex shapes with features with polyhedral approximations.
pub trait PolygonalFeatureMap: SupportMap {
    /// Compute the support polygonal face of `self` towards the `dir`.
    fn local_support_feature(&self, dir: Vector, out_feature: &mut PolygonalFeature);

    // TODO: this is currently just a workaround for https://github.com/dimforge/rapier/issues/417
    //       until we get a better way to deal with the issue without breaking internal edges
    //       handling.
    /// Is this shape a `ConvexPolyhedron`?
    fn is_convex_polyhedron(&self) -> bool {
        false
    }
}

impl PolygonalFeatureMap for Segment {
    fn local_support_feature(&self, _: Vector, out_feature: &mut PolygonalFeature) {
        *out_feature = PolygonalFeature::from(*self);
    }
}

impl PolygonalFeatureMap for Triangle {
    fn local_support_feature(&self, dir: Vector, out_feature: &mut PolygonalFeature) {
        *out_feature = self.support_face(dir);
    }
}

impl PolygonalFeatureMap for Cuboid {
    fn local_support_feature(&self, dir: Vector, out_feature: &mut PolygonalFeature) {
        *out_feature = self.support_face(dir);
    }
}

#[cfg(feature = "dim3")]
impl PolygonalFeatureMap for Cylinder {
    fn local_support_feature(&self, dir: Vector, out_features: &mut PolygonalFeature) {
        use crate::math::Vector2;

        // About feature ids.
        // At all times, we consider our cylinder to be approximated as follows:
        // - The curved part is approximated by a single segment.
        // - Each flat cap of the cylinder is approximated by a square.
        // - The curved-part segment has a feature ID of 0, and its endpoint with negative
        //   `y` coordinate has an ID of 1.
        // - The bottom cap has its vertices with feature ID of 1,3,5,7 (in counter-clockwise order
        //   when looking at the cap with an eye looking towards +y).
        // - The bottom cap has its four edge feature IDs of 2,4,6,8, in counter-clockwise order.
        // - The bottom cap has its face feature ID of 9.
        // - The feature IDs of the top cap are the same as the bottom cap to which we add 10.
        //   So its vertices have IDs 11,13,15,17, its edges 12,14,16,18, and its face 19.
        // - Note that at all times, one of each cap's vertices are the same as the curved-part
        //   segment endpoints.
        let dir2 = Vector2::new(dir.x, dir.z)
            .try_normalize()
            .unwrap_or(Vector2::X);

        if dir.y.abs() < 0.5 {
            // We return a segment lying on the cylinder's curved part.
            out_features.vertices[0] = Vector::new(
                dir2.x * self.radius,
                -self.half_height,
                dir2.y * self.radius,
            );
            out_features.vertices[1] =
                Vector::new(dir2.x * self.radius, self.half_height, dir2.y * self.radius);
            out_features.eids = PackedFeatureId::edges([0, 0, 0, 0]);
            out_features.fid = PackedFeatureId::face(0);
            out_features.num_vertices = 2;
            out_features.vids = PackedFeatureId::vertices([1, 11, 11, 11]);
        } else {
            // We return a square approximation of the cylinder cap.
            let y = self.half_height.copysign(dir.y);
            out_features.vertices[0] = Vector::new(dir2.x * self.radius, y, dir2.y * self.radius);
            out_features.vertices[1] = Vector::new(-dir2.y * self.radius, y, dir2.x * self.radius);
            out_features.vertices[2] = Vector::new(-dir2.x * self.radius, y, -dir2.y * self.radius);
            out_features.vertices[3] = Vector::new(dir2.y * self.radius, y, -dir2.x * self.radius);

            if dir.y < 0.0 {
                out_features.eids = PackedFeatureId::edges([2, 4, 6, 8]);
                out_features.fid = PackedFeatureId::face(9);
                out_features.num_vertices = 4;
                out_features.vids = PackedFeatureId::vertices([1, 3, 5, 7]);
            } else {
                out_features.eids = PackedFeatureId::edges([12, 14, 16, 18]);
                out_features.fid = PackedFeatureId::face(19);
                out_features.num_vertices = 4;
                out_features.vids = PackedFeatureId::vertices([11, 13, 15, 17]);
            }
        }
    }
}

#[cfg(feature = "dim3")]
impl PolygonalFeatureMap for Cone {
    fn local_support_feature(&self, dir: Vector, out_features: &mut PolygonalFeature) {
        use crate::math::Vector2;

        // About feature ids. It is very similar to the feature ids of cylinders.
        // At all times, we consider our cone to be approximated as follows:
        // - The curved part is approximated by a single segment.
        // - The flat cap of the cone is approximated by a square.
        // - The curved-part segment has a feature ID of 0, and its endpoint with negative
        //   `y` coordinate has an ID of 1.
        // - The bottom cap has its vertices with feature ID of 1,3,5,7 (in counter-clockwise order
        //   when looking at the cap with an eye looking towards +y).
        // - The bottom cap has its four edge feature IDs of 2,4,6,8, in counter-clockwise order.
        // - The bottom cap has its face feature ID of 9.
        // - Note that at all times, one of the cap's vertices are the same as the curved-part
        //   segment endpoints.
        let dir2 = Vector2::new(dir.x, dir.z)
            .try_normalize()
            .unwrap_or(Vector2::X);

        if dir.y > 0.0 {
            // We return a segment lying on the cone's curved part.
            out_features.vertices[0] = Vector::new(
                dir2.x * self.radius,
                -self.half_height,
                dir2.y * self.radius,
            );
            out_features.vertices[1] = Vector::new(0.0, self.half_height, 0.0);
            out_features.eids = PackedFeatureId::edges([0, 0, 0, 0]);
            out_features.fid = PackedFeatureId::face(0);
            out_features.num_vertices = 2;
            out_features.vids = PackedFeatureId::vertices([1, 11, 11, 11]);
        } else {
            // We return a square approximation of the cone cap.
            let y = -self.half_height;
            out_features.vertices[0] = Vector::new(dir2.x * self.radius, y, dir2.y * self.radius);
            out_features.vertices[1] = Vector::new(-dir2.y * self.radius, y, dir2.x * self.radius);
            out_features.vertices[2] = Vector::new(-dir2.x * self.radius, y, -dir2.y * self.radius);
            out_features.vertices[3] = Vector::new(dir2.y * self.radius, y, -dir2.x * self.radius);

            out_features.eids = PackedFeatureId::edges([2, 4, 6, 8]);
            out_features.fid = PackedFeatureId::face(9);
            out_features.num_vertices = 4;
            out_features.vids = PackedFeatureId::vertices([1, 3, 5, 7]);
        }
    }
}
