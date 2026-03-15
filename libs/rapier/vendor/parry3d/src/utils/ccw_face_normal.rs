use crate::math::Vector;

/// Computes the direction pointing toward the right-hand-side of an oriented segment.
///
/// Returns `None` if the segment is degenerate.
#[inline]
#[cfg(feature = "dim2")]
pub fn ccw_face_normal(pts: [Vector; 2]) -> Option<Vector> {
    let ab = pts[1] - pts[0];
    let res = Vector::new(ab[1], -ab[0]);

    res.try_normalize()
}

/// Computes the normal of a counter-clock-wise triangle.
///
/// Returns `None` if the triangle is degenerate.
#[inline]
#[cfg(feature = "dim3")]
pub fn ccw_face_normal(pts: [Vector; 3]) -> Option<Vector> {
    let ab = pts[1] - pts[0];
    let ac = pts[2] - pts[0];
    let res = ab.cross(ac);

    res.try_normalize()
}
