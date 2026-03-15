use crate::math::{Pose, Real, Vector2};
use crate::query::ClosestPoints;
use crate::shape::{Segment, SegmentPointLocation};

use crate::math::Vector;

/// Closest points between segments.
#[inline]
pub fn closest_points_segment_segment(
    pos12: &Pose,
    seg1: &Segment,
    seg2: &Segment,
    margin: Real,
) -> ClosestPoints {
    let (loc1, loc2) = closest_points_segment_segment_with_locations(pos12, seg1, seg2);
    let p1 = seg1.point_at(&loc1);
    let p2 = seg2.point_at(&loc2);

    if (p1 - (pos12 * p2)).length_squared() <= margin * margin {
        ClosestPoints::WithinMargin(p1, p2)
    } else {
        ClosestPoints::Disjoint
    }
}

// TODO: use this specialized procedure for distance/interference/contact determination as well.
/// Closest points between two segments.
#[inline]
pub fn closest_points_segment_segment_with_locations(
    pos12: &Pose,
    seg1: &Segment,
    seg2: &Segment,
) -> (SegmentPointLocation, SegmentPointLocation) {
    let seg2_1 = seg2.transformed(pos12);
    closest_points_segment_segment_with_locations_nD((&seg1.a, &seg1.b), (&seg2_1.a, &seg2_1.b))
}

macro_rules! impl_closest_points(
    ($Vect: ty, $fname: ident) => {
        /// Segment-segment closest points computation in an arbitrary dimension.
        #[allow(non_snake_case)]
        #[inline]
        pub fn $fname(
            seg1: (&$Vect, &$Vect),
            seg2: (&$Vect, &$Vect),
        ) -> (SegmentPointLocation, SegmentPointLocation) {
            // Inspired by RealField-time collision detection by Christer Ericson.
            let d1 = seg1.1 - seg1.0;
            let d2 = seg2.1 - seg2.0;
            let r = seg1.0 - seg2.0;

            let a = d1.length_squared();
            let e = d2.length_squared();
            let f = d2.dot(r);

            let mut s;
            let mut t;

            let _eps = crate::math::DEFAULT_EPSILON;
            if a <= _eps && e <= _eps {
                s = 0.0;
                t = 0.0;
            } else if a <= _eps {
                s = 0.0;
                t = (f / e).clamp(0.0, 1.0);
            } else {
                let c = d1.dot(r);
                if e <= _eps {
                    t = 0.0;
                    s = (-c / a).clamp(0.0, 1.0);
                } else {
                    let b = d1.dot(d2);
                    let ae = a * e;
                    let bb = b * b;
                    let denom = ae - bb;

                    // Use absolute and ulps error to test collinearity.
                    if denom > _eps && !ulps_eq!(ae, bb) {
                        s = ((b * f - c * e) / denom).clamp(0.0, 1.0);
                    } else {
                        s = 0.0;
                    }

                    t = (b * s + f) / e;

                    if t < 0.0 {
                        t = 0.0;
                        s = (-c / a).clamp(0.0, 1.0);
                    } else if t > 1.0 {
                        t = 1.0;
                        s = ((b - c) / a).clamp(0.0, 1.0);
                    }
                }
            }

            let loc1 = if s == 0.0 {
                SegmentPointLocation::OnVertex(0)
            } else if s == 1.0 {
                SegmentPointLocation::OnVertex(1)
            } else {
                SegmentPointLocation::OnEdge([1.0 - s, s])
            };

            let loc2 = if t == 0.0 {
                SegmentPointLocation::OnVertex(0)
            } else if t == 1.0 {
                SegmentPointLocation::OnVertex(1)
            } else {
                SegmentPointLocation::OnEdge([1.0 - t, t])
            };

            (loc1, loc2)
        }
    }
);

impl_closest_points!(Vector2, closest_points_segment_segment_2d);
impl_closest_points!(Vector, closest_points_segment_segment_with_locations_nD);
