use crate::math::Vector2;

use crate::math::Real;
use crate::shape::{SegmentPointLocation, Triangle, TriangleOrientation};

/// Intersection between two segments.
pub enum SegmentsIntersection {
    /// Single point of intersection.
    Point {
        /// Location of the intersection point on the first segment.
        loc1: SegmentPointLocation,
        /// Location of the intersection point on the second segment.
        loc2: SegmentPointLocation,
    },
    /// Intersection along a segment (when both segments are collinear).
    Segment {
        /// Location of the first intersection point on the first segment.
        first_loc1: SegmentPointLocation,
        /// Location of the first intersection point on the second segment.
        first_loc2: SegmentPointLocation,
        /// Location of the second intersection point on the first segment.
        second_loc1: SegmentPointLocation,
        /// Location of the second intersection point on the second segment.
        second_loc2: SegmentPointLocation,
    },
}

/// Computes the intersection between two segments.
pub fn segments_intersection2d(
    a: Vector2,
    b: Vector2,
    c: Vector2,
    d: Vector2,
    epsilon: Real,
) -> Option<SegmentsIntersection> {
    let denom = a.x * (d.y - c.y) + b.x * (c.y - d.y) + d.x * (b.y - a.y) + c.x * (a.y - b.y);

    // If denom is zero, then segments are parallel: handle separately.
    if denom.abs() < epsilon || ulps_eq!(denom, 0.0) {
        return parallel_intersection(a, b, c, d, epsilon);
    }

    let num = a.x * (d.y - c.y) + c.x * (a.y - d.y) + d.x * (c.y - a.y);
    let s = num / denom;

    let num = -(a.x * (c.y - b.y) + b.x * (a.y - c.y) + c.x * (b.y - a.y));
    let t = num / denom;

    if 0.0 > s || s > 1.0 || 0.0 > t || t > 1.0 {
        None
    } else {
        let loc1 = if s == 0.0 {
            SegmentPointLocation::OnVertex(0)
        } else if s == denom {
            SegmentPointLocation::OnVertex(1)
        } else {
            SegmentPointLocation::OnEdge([1.0 - s, s])
        };

        let loc2 = if t == 0.0 {
            SegmentPointLocation::OnVertex(0)
        } else if t == denom {
            SegmentPointLocation::OnVertex(1)
        } else {
            SegmentPointLocation::OnEdge([1.0 - t, t])
        };

        Some(SegmentsIntersection::Point { loc1, loc2 })
    }
}

fn parallel_intersection(
    a: Vector2,
    b: Vector2,
    c: Vector2,
    d: Vector2,
    epsilon: Real,
) -> Option<SegmentsIntersection> {
    if Triangle::orientation2d(a, b, c, epsilon) != TriangleOrientation::Degenerate {
        return None;
    }

    let ab_c = between(a, b, c);
    let ab_d = between(a, b, d);
    if let (Some(loc1), Some(loc2)) = (ab_c, ab_d) {
        return Some(SegmentsIntersection::Segment {
            first_loc1: loc1,
            first_loc2: SegmentPointLocation::OnVertex(0),
            second_loc1: loc2,
            second_loc2: SegmentPointLocation::OnVertex(1),
        });
    }

    let cd_a = between(c, d, a);
    let cd_b = between(c, d, b);
    if let (Some(loc1), Some(loc2)) = (cd_a, cd_b) {
        return Some(SegmentsIntersection::Segment {
            first_loc1: SegmentPointLocation::OnVertex(0),
            first_loc2: loc1,
            second_loc1: SegmentPointLocation::OnVertex(1),
            second_loc2: loc2,
        });
    }

    if let (Some(loc1), Some(loc2)) = (ab_c, cd_b) {
        return Some(SegmentsIntersection::Segment {
            first_loc1: loc1,
            first_loc2: SegmentPointLocation::OnVertex(0),
            second_loc1: SegmentPointLocation::OnVertex(1),
            second_loc2: loc2,
        });
    }

    if let (Some(loc1), Some(loc2)) = (ab_c, cd_a) {
        return Some(SegmentsIntersection::Segment {
            first_loc1: loc1,
            first_loc2: SegmentPointLocation::OnVertex(0),
            second_loc1: SegmentPointLocation::OnVertex(0),
            second_loc2: loc2,
        });
    }

    if let (Some(loc1), Some(loc2)) = (ab_d, cd_b) {
        return Some(SegmentsIntersection::Segment {
            first_loc1: loc1,
            first_loc2: SegmentPointLocation::OnVertex(1),
            second_loc1: SegmentPointLocation::OnVertex(1),
            second_loc2: loc2,
        });
    }

    if let (Some(loc1), Some(loc2)) = (ab_d, cd_a) {
        return Some(SegmentsIntersection::Segment {
            first_loc1: loc1,
            first_loc2: SegmentPointLocation::OnVertex(1),
            second_loc1: SegmentPointLocation::OnVertex(0),
            second_loc2: loc2,
        });
    }

    None
}

// Checks that `c` is in-between `a` and `b`.
// Assumes the three points are collinear.
fn between(a: Vector2, b: Vector2, c: Vector2) -> Option<SegmentPointLocation> {
    // If ab not vertical, check betweenness on x; else on y.
    // TODO: handle cases where we actually are on a vertex (to return OnEdge instead of OnVertex)?
    if a.x != b.x {
        if a.x <= c.x && c.x <= b.x {
            let bcoord = (c.x - a.x) / (b.x - a.x);
            return Some(SegmentPointLocation::OnEdge([1.0 - bcoord, bcoord]));
        } else if a.x >= c.x && c.x >= b.x {
            let bcoord = (c.x - b.x) / (a.x - b.x);
            return Some(SegmentPointLocation::OnEdge([bcoord, 1.0 - bcoord]));
        }
    } else if a.y != b.y {
        if a.y <= c.y && c.y <= b.y {
            let bcoord = (c.y - a.y) / (b.y - a.y);
            return Some(SegmentPointLocation::OnEdge([1.0 - bcoord, bcoord]));
        } else if a.y >= c.y && c.y >= b.y {
            let bcoord = (c.y - b.y) / (a.y - b.y);
            return Some(SegmentPointLocation::OnEdge([bcoord, 1.0 - bcoord]));
        }
    } else if a.x == c.x && a.y == c.y {
        return Some(SegmentPointLocation::OnVertex(0));
    }

    None
}
