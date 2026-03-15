use crate::math::Vector;
#[cfg(feature = "dim2")]
use crate::utils;

// Features in clipping points are:
// 0 = First vertex.
// 1 = On the face.
// 2 = Second vertex.
pub type ClippingPoints = (Vector, Vector, usize, usize);

/// Projects two segments on one another towards the direction `normal`,
/// and compute their intersection.
#[cfg(feature = "dim2")]
pub fn clip_segment_segment_with_normal(
    mut seg1: (Vector, Vector),
    mut seg2: (Vector, Vector),
    normal: Vector,
) -> Option<(ClippingPoints, ClippingPoints)> {
    use crate::utils::WBasis;
    let tangent = normal.orthonormal_basis()[0];

    let mut range1 = [seg1.0.dot(tangent), seg1.1.dot(tangent)];
    let mut range2 = [seg2.0.dot(tangent), seg2.1.dot(tangent)];
    let mut features1 = [0, 2];
    let mut features2 = [0, 2];

    if range1[1] < range1[0] {
        range1.swap(0, 1);
        features1.swap(0, 1);
        core::mem::swap(&mut seg1.0, &mut seg1.1);
    }

    if range2[1] < range2[0] {
        range2.swap(0, 1);
        features2.swap(0, 1);
        core::mem::swap(&mut seg2.0, &mut seg2.1);
    }

    if range2[0] > range1[1] || range1[0] > range2[1] {
        // No clip point.
        return None;
    }

    let ca = if range2[0] > range1[0] {
        let bcoord = (range2[0] - range1[0]) * utils::inv(range1[1] - range1[0]);
        let p1 = seg1.0 + (seg1.1 - seg1.0) * bcoord;
        let p2 = seg2.0;

        (p1, p2, 1, features2[0])
    } else {
        let bcoord = (range1[0] - range2[0]) * utils::inv(range2[1] - range2[0]);
        let p1 = seg1.0;
        let p2 = seg2.0 + (seg2.1 - seg2.0) * bcoord;

        (p1, p2, features1[0], 1)
    };

    let cb = if range2[1] < range1[1] {
        let bcoord = (range2[1] - range1[0]) * utils::inv(range1[1] - range1[0]);
        let p1 = seg1.0 + (seg1.1 - seg1.0) * bcoord;
        let p2 = seg2.1;

        (p1, p2, 1, features2[1])
    } else {
        let bcoord = (range1[1] - range2[0]) * utils::inv(range2[1] - range2[0]);
        let p1 = seg1.1;
        let p2 = seg2.0 + (seg2.1 - seg2.0) * bcoord;

        (p1, p2, features1[1], 1)
    };

    Some((ca, cb))
}

/// Projects two segments on one another and compute their intersection.
pub fn clip_segment_segment(
    mut seg1: (Vector, Vector),
    mut seg2: (Vector, Vector),
) -> Option<(ClippingPoints, ClippingPoints)> {
    // NOTE: no need to normalize the tangent.
    let tangent1 = seg1.1 - seg1.0;
    let sqnorm_tangent1 = tangent1.length_squared();

    let mut range1 = [0.0, sqnorm_tangent1];
    let mut range2 = [
        (seg2.0 - seg1.0).dot(tangent1),
        (seg2.1 - seg1.0).dot(tangent1),
    ];
    let mut features1 = [0, 2];
    let mut features2 = [0, 2];

    if range1[1] < range1[0] {
        range1.swap(0, 1);
        features1.swap(0, 1);
        core::mem::swap(&mut seg1.0, &mut seg1.1);
    }

    if range2[1] < range2[0] {
        range2.swap(0, 1);
        features2.swap(0, 1);
        core::mem::swap(&mut seg2.0, &mut seg2.1);
    }

    if range2[0] > range1[1] || range1[0] > range2[1] {
        // No clip point.
        return None;
    }

    let length1 = range1[1] - range1[0];
    let length2 = range2[1] - range2[0];

    let ca = if range2[0] > range1[0] {
        let bcoord = (range2[0] - range1[0]) / length1;
        let p1 = seg1.0 + tangent1 * bcoord;
        let p2 = seg2.0;

        (p1, p2, 1, features2[0])
    } else {
        let bcoord = (range1[0] - range2[0]) / length2;
        let p1 = seg1.0;
        let p2 = seg2.0 + (seg2.1 - seg2.0) * bcoord;

        (p1, p2, features1[0], 1)
    };

    let cb = if range2[1] < range1[1] {
        let bcoord = (range2[1] - range1[0]) / length1;
        let p1 = seg1.0 + tangent1 * bcoord;
        let p2 = seg2.1;

        (p1, p2, 1, features2[1])
    } else {
        let bcoord = (range1[1] - range2[0]) / length2;
        let p1 = seg1.1;
        let p2 = seg2.0 + (seg2.1 - seg2.0) * bcoord;

        (p1, p2, features1[1], 1)
    };

    Some((ca, cb))
}
