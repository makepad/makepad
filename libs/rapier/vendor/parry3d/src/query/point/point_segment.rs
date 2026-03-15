use crate::math::Vector;
use crate::query::{PointProjection, PointQuery, PointQueryWithLocation};
use crate::shape::{FeatureId, Segment, SegmentPointLocation};

impl PointQuery for Segment {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        self.project_local_point_and_get_location(pt, solid).0
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        let (proj, loc) = self.project_local_point_and_get_location(pt, false);
        let feature = match loc {
            SegmentPointLocation::OnVertex(i) => FeatureId::Vertex(i),
            SegmentPointLocation::OnEdge(..) => {
                #[cfg(feature = "dim2")]
                {
                    let dir = self.scaled_direction();
                    let dpt = pt - proj.point;
                    if dpt.perp_dot(dir) >= 0.0 {
                        FeatureId::Face(0)
                    } else {
                        FeatureId::Face(1)
                    }
                }

                #[cfg(feature = "dim3")]
                {
                    FeatureId::Edge(0)
                }
            }
        };

        (proj, feature)
    }

    // NOTE: the default implementation of `.distance_to_point(...)` will return the error that was
    // eaten by the `::approx_eq(...)` on `project_point(...)`.
}

impl PointQueryWithLocation for Segment {
    type Location = SegmentPointLocation;

    #[inline]
    fn project_local_point_and_get_location(
        &self,
        pt: Vector,
        _: bool,
    ) -> (PointProjection, Self::Location) {
        let ab = self.b - self.a;
        let ap = pt - self.a;
        let ab_ap = ab.dot(ap);
        let sqnab = ab.length_squared();

        let proj;
        let location;

        if ab_ap <= 0.0 {
            // Voronoï region of vertex 'a'.
            location = SegmentPointLocation::OnVertex(0);
            proj = self.a;
        } else if ab_ap >= sqnab {
            // Voronoï region of vertex 'b'.
            location = SegmentPointLocation::OnVertex(1);
            proj = self.b;
        } else {
            assert!(sqnab != 0.0);

            // Voronoï region of the segment interior.
            let u = ab_ap / sqnab;
            let bcoords = [1.0 - u, u];
            location = SegmentPointLocation::OnEdge(bcoords);
            proj = self.a + ab * u;
        }

        // TODO: is this acceptable?
        let inside = relative_eq!(proj, pt);

        (PointProjection::new(inside, proj), location)
    }
}
