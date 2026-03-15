use crate::bounding_volume::Aabb;
use crate::math::{Real, Vector, DIM};
use crate::query::Ray;
use crate::shape::Segment;
use num::{Bounded, Zero};

impl Aabb {
    /// Computes the intersection of a segment with this Aabb.
    ///
    /// Returns `None` if there is no intersection or if `pa` is invalid (contains `NaN`).
    #[inline]
    pub fn clip_segment(&self, pa: Vector, pb: Vector) -> Option<Segment> {
        let ab = pb - pa;
        clip_aabb_line(self, pa, ab).map(|clip| {
            Segment::new(
                pa + (ab * (clip.0).0.max(0.0)),
                pa + (ab * (clip.1).0.min(1.0)),
            )
        })
    }

    /// Computes the parameters of the two intersection points between a line and this Aabb.
    ///
    /// The parameters are such that the point are given by `orig + dir * parameter`.
    /// Returns `None` if there is no intersection or if `orig` is invalid (contains `NaN`).
    #[inline]
    pub fn clip_line_parameters(&self, orig: Vector, dir: Vector) -> Option<(Real, Real)> {
        clip_aabb_line(self, orig, dir).map(|clip| ((clip.0).0, (clip.1).0))
    }

    /// Computes the intersection segment between a line and this Aabb.
    ///
    /// Returns `None` if there is no intersection or if `orig` is invalid (contains `NaN`).
    #[inline]
    pub fn clip_line(&self, orig: Vector, dir: Vector) -> Option<Segment> {
        clip_aabb_line(self, orig, dir)
            .map(|clip| Segment::new(orig + (dir * (clip.0).0), orig + (dir * (clip.1).0)))
    }

    /// Computes the parameters of the two intersection points between a ray and this Aabb.
    ///
    /// The parameters are such that the point are given by `ray.orig + ray.dir * parameter`.
    /// Returns `None` if there is no intersection or if `ray.orig` is invalid (contains `NaN`).
    #[inline]
    pub fn clip_ray_parameters(&self, ray: &Ray) -> Option<(Real, Real)> {
        self.clip_line_parameters(ray.origin, ray.dir)
            .and_then(|clip| {
                let t0 = clip.0;
                let t1 = clip.1;

                if t1 < 0.0 {
                    None
                } else {
                    Some((t0.max(0.0), t1))
                }
            })
    }

    /// Computes the intersection segment between a ray and this Aabb.
    ///
    /// Returns `None` if there is no intersection.
    #[inline]
    pub fn clip_ray(&self, ray: &Ray) -> Option<Segment> {
        self.clip_ray_parameters(ray)
            .map(|clip| Segment::new(ray.point_at(clip.0), ray.point_at(clip.1)))
    }
}

/// Computes the segment given by the intersection of a line and an Aabb.
///
/// Returns the two intersections represented as `(t, normal, side)` such that:
/// - `origin + dir * t` gives the intersection points.
/// - `normal` is the face normal at the intersection. This is equal to the zero vector if `dir`
///   is invalid (a zero vector or NaN) and `origin` is inside the AABB.
/// - `side` is the side of the AABB that was hit. This is an integer in [-3, 3] where `1` represents
///   the `+X` axis, `2` the `+Y` axis, etc., and negative values represent the corresponding
///   negative axis. The special value of `0` indicates that the provided `dir` is zero or `NaN`
///   and the line origin is inside the AABB.
pub fn clip_aabb_line(
    aabb: &Aabb,
    origin: Vector,
    dir: Vector,
) -> Option<((Real, Vector, isize), (Real, Vector, isize))> {
    let mut tmax: Real = Bounded::max_value();
    let mut tmin: Real = -tmax;
    let mut near_side = 0;
    let mut far_side = 0;
    let mut near_diag = false;
    let mut far_diag = false;

    for i in 0usize..DIM {
        if dir[i].is_zero() {
            if origin[i] < aabb.mins[i] || origin[i] > aabb.maxs[i] {
                return None;
            }
        } else {
            let denom = 1.0 / dir[i];
            let flip_sides;
            let mut inter_with_near_halfspace = (aabb.mins[i] - origin[i]) * denom;
            let mut inter_with_far_halfspace = (aabb.maxs[i] - origin[i]) * denom;

            if inter_with_near_halfspace > inter_with_far_halfspace {
                flip_sides = true;
                core::mem::swap(
                    &mut inter_with_near_halfspace,
                    &mut inter_with_far_halfspace,
                )
            } else {
                flip_sides = false;
            }

            if inter_with_near_halfspace > tmin {
                tmin = inter_with_near_halfspace;
                near_side = if flip_sides {
                    -(i as isize + 1)
                } else {
                    i as isize + 1
                };
                near_diag = false;
            } else if inter_with_near_halfspace == tmin {
                near_diag = true;
            }

            if inter_with_far_halfspace < tmax {
                tmax = inter_with_far_halfspace;
                far_side = if !flip_sides {
                    -(i as isize + 1)
                } else {
                    i as isize + 1
                };
                far_diag = false;
            } else if inter_with_far_halfspace == tmax {
                far_diag = true;
            }

            if tmax < 0.0 || tmin > tmax {
                return None;
            }
        }
    }

    let near = if near_diag {
        (tmin, -dir.normalize(), near_side)
    } else {
        // If near_side is 0, we are in a special case where `dir` is
        // zero or NaN. Return `Some` only if the ray starts inside the
        // aabb.
        if near_side == 0 {
            let zero = (0.0, Vector::ZERO, 0);
            return aabb.contains_local_point(origin).then_some((zero, zero));
        }

        let mut normal = Vector::ZERO;

        if near_side < 0 {
            normal[(-near_side - 1) as usize] = 1.0;
        } else {
            normal[(near_side - 1) as usize] = -1.0;
        }

        (tmin, normal, near_side)
    };

    let far = if far_diag {
        (tmax, -dir.normalize(), far_side)
    } else {
        // If far_side is 0, we are in a special case where `dir` is
        // zero or NaN. Return `Some` only if the ray starts inside the
        // aabb.
        if far_side == 0 {
            let zero = (0.0, Vector::ZERO, 0);
            return aabb.contains_local_point(origin).then_some((zero, zero));
        }

        let mut normal = Vector::ZERO;

        if far_side < 0 {
            normal[(-far_side - 1) as usize] = -1.0;
        } else {
            normal[(far_side - 1) as usize] = 1.0;
        }

        (tmax, normal, far_side)
    };

    Some((near, far))
}
#[cfg(test)]
mod test {
    use super::*;

    #[test]
    pub fn clip_empty_aabb_line() {
        assert!(clip_aabb_line(
            &Aabb::new(Vector::ZERO, Vector::ZERO),
            Vector::ZERO,
            Vector::ZERO,
        )
        .is_some());
        assert!(clip_aabb_line(
            &Aabb::new(Vector::splat(1.0).into(), Vector::splat(2.0).into()),
            Vector::ZERO,
            Vector::ZERO,
        )
        .is_none());
    }

    #[test]
    pub fn clip_empty_aabb_segment() {
        let aabb_origin = Aabb::new(Vector::ZERO, Vector::ZERO);
        let aabb_shifted = Aabb::new(Vector::splat(1.0), Vector::splat(2.0));
        assert!(aabb_origin
            .clip_segment(Vector::ZERO, Vector::splat(Real::NAN))
            .is_some());
        assert!(aabb_shifted
            .clip_segment(Vector::ZERO, Vector::splat(Real::NAN))
            .is_none());
    }
}
