use core::iter::IntoIterator;

use crate::bounding_volume::Aabb;
use crate::math::{Pose, Vector, DIM};
use crate::shape::SupportMap;

/// Computes the [`Aabb`] of an [support mapped shape](SupportMap).
#[cfg(feature = "dim3")]
pub fn support_map_aabb<G>(m: &Pose, i: &G) -> Aabb
where
    G: SupportMap,
{
    let mut min = Vector::ZERO;
    let mut max = Vector::ZERO;
    let mut basis = Vector::ZERO;

    for d in 0..DIM {
        // TODO: this could be further improved iterating on `m`'s columns, and passing
        // Id as the transformation matrix.
        basis[d] = 1.0;
        max[d] = i.support_point(m, basis)[d];

        basis[d] = -1.0;
        min[d] = i.support_point(m, basis)[d];

        basis[d] = 0.0;
    }

    Aabb::new(min, max)
}

/// Computes the [`Aabb`] of an [support mapped shape](SupportMap).
pub fn local_support_map_aabb<G>(i: &G) -> Aabb
where
    G: SupportMap,
{
    let mut min = Vector::ZERO;
    let mut max = Vector::ZERO;
    let mut basis = Vector::ZERO;

    for d in 0..DIM {
        // TODO: this could be further improved iterating on `m`'s columns, and passing
        // Id as the transformation matrix.
        basis[d] = 1.0;
        max[d] = i.local_support_point(basis)[d];

        basis[d] = -1.0;
        min[d] = i.local_support_point(basis)[d];

        basis[d] = 0.0;
    }

    Aabb::new(min, max)
}

/// Computes the [`Aabb`] of a set of point references transformed by `m`.
pub fn point_cloud_aabb_ref<'a, I>(m: &Pose, pts: I) -> Aabb
where
    I: IntoIterator<Item = &'a Vector>,
{
    point_cloud_aabb(m, pts.into_iter().copied())
}

/// Computes the [`Aabb`] of a set of points transformed by `m`.
pub fn point_cloud_aabb<I>(m: &Pose, pts: I) -> Aabb
where
    I: IntoIterator<Item = Vector>,
{
    let mut it = pts.into_iter();

    let p0 = it.next().expect(
        "Vector cloud Aabb construction: the input iterator should yield at least one point.",
    );
    let wp0 = m * p0;
    let mut min: Vector = wp0;
    let mut max: Vector = wp0;

    for pt in it {
        let wpt = m * pt;
        min = min.min(wpt);
        max = max.max(wpt);
    }

    Aabb::new(min, max)
}

/// Computes the [`Aabb`] of a set of points.
pub fn local_point_cloud_aabb_ref<'a, I>(pts: I) -> Aabb
where
    I: IntoIterator<Item = &'a Vector>,
{
    local_point_cloud_aabb(pts.into_iter().copied())
}

/// Computes the [`Aabb`] of a set of points.
pub fn local_point_cloud_aabb<I>(pts: I) -> Aabb
where
    I: IntoIterator<Item = Vector>,
{
    let mut it = pts.into_iter();

    let p0 = it.next().expect(
        "Vector cloud Aabb construction: the input iterator should yield at least one point.",
    );
    let mut min: Vector = p0;
    let mut max: Vector = p0;

    for pt in it {
        min = min.min(pt);
        max = max.max(pt);
    }

    Aabb::new(min, max)
}
