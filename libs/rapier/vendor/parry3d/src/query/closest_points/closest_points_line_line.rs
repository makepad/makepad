use crate::math::{Real, Vector};

/// Closest points between two lines.
///
/// The result, say `res`, is such that the closest points between both lines are
/// `orig1 + dir1 * res.0` and `orig2 + dir2 * res.1`.
#[inline]
pub fn closest_points_line_line_parameters(
    orig1: Vector,
    dir1: Vector,
    orig2: Vector,
    dir2: Vector,
) -> (Real, Real) {
    let res = closest_points_line_line_parameters_eps(
        orig1,
        dir1,
        orig2,
        dir2,
        crate::math::DEFAULT_EPSILON,
    );
    (res.0, res.1)
}

/// Closest points between two lines with a custom tolerance epsilon.
///
/// The result, say `res`, is such that the closest points between both lines are
/// `orig1 + dir1 * res.0` and `orig2 + dir2 * res.1`. If the lines are parallel
/// then `res.2` is set to `true` and the returned closest points are `orig1` and
/// its projection on the second line.
#[inline]
pub fn closest_points_line_line_parameters_eps(
    orig1: Vector,
    dir1: Vector,
    orig2: Vector,
    dir2: Vector,
    eps: Real,
) -> (Real, Real, bool) {
    // Inspired by RealField-time collision detection by Christer Ericson.
    let r = orig1 - orig2;

    let a = dir1.length_squared();
    let e = dir2.length_squared();
    let f = dir2.dot(r);

    if a <= eps && e <= eps {
        (0.0, 0.0, false)
    } else if a <= eps {
        (0.0, f / e, false)
    } else {
        let c = dir1.dot(r);
        if e <= eps {
            (-c / a, 0.0, false)
        } else {
            let b = dir1.dot(dir2);
            let ae = a * e;
            let bb = b * b;
            let denom = ae - bb;

            // Use absolute and ulps error to test collinearity.
            let parallel = denom <= eps || ulps_eq!(ae, bb);

            let s = if !parallel {
                (b * f - c * e) / denom
            } else {
                0.0
            };

            (s, (b * s + f) / e, parallel)
        }
    }
}

// TODO: can we re-use this for the segment/segment case?
/// Closest points between two segments.
#[inline]
pub fn closest_points_line_line(
    orig1: Vector,
    dir1: Vector,
    orig2: Vector,
    dir2: Vector,
) -> (Vector, Vector) {
    let (s, t) = closest_points_line_line_parameters(orig1, dir1, orig2, dir2);
    (orig1 + dir1 * s, orig2 + dir2 * t)
}
