// PORT EXTENSION (not in upstream C): a solid given by a distance query, for shapes that have no
// convex form and no mesh worth colliding with. It collides with spheres only, the query answers
// for one point at a time, and it has no mass of its own: the body's mass is set by the caller.

use crate::constants::overlap_slop;
use crate::manifold::FEATURE_PAIR_SINGLE;
use crate::math_functions::*;
use crate::types::{CastOutput, LocalManifold, LocalManifoldPoint, RayCastInput, ShapeProxy, Sphere};

/// A solid, in its shape's frame.
pub trait DistanceField: Send + Sync + std::fmt::Debug {
    /// How far `p` is from the surface (negative inside) and the unit direction that grows fastest.
    fn distance(&self, p: Vec3) -> (f32, Vec3);
    /// A box holding the solid.
    fn bounds(&self) -> AABB;
}

pub fn compute_field_aabb(field: &dyn DistanceField, transform: Transform) -> AABB {
    aabb_transform(transform, field.bounds())
}

/// Steps along a ball's way no shorter than this: a wall thinner than this can be missed.
const SWEEP_LEAST_STEP: f32 = 0.5;

/// A contact on the surface towards the sphere, from the field's frame. `motion`: how far the
/// sphere's centre goes against the field this step. A surface the way runs into counts as soon as it
/// can be reached, so a wall can't pass a sphere in one step however fast they go; of it and the
/// nearest surface, the contact is on the one the step would go furthest past.
pub fn collide_field_and_sphere(manifold: &mut LocalManifold, capacity: i32, field: &dyn DistanceField, sphere: &Sphere, transform_b_to_a: Transform, motion: Vec3, speculative: f32) {
    manifold.points.clear();
    if capacity == 0 {
        return;
    }
    let center = transform_point(transform_b_to_a, sphere.center);
    let (nearest, nearest_normal) = field.distance(center);
    let clear = nearest - sphere.radius;
    let reach = length(motion);
    if clear > speculative + reach {
        return;
    }
    // how far the step goes past the plane of a surface this far from the centre
    let past = |distance: f32, normal: Vec3| -dot(motion, normal) - (distance - sphere.radius);
    let (mut distance, mut normal) = (nearest, nearest_normal);
    if reach > clear.max(0.0) {
        let way = mul_sv(1.0 / reach, motion);
        let mut t = clear.max(SWEEP_LEAST_STEP);
        loop {
            let last = t >= reach;
            let along = t.min(reach);
            let (d, n) = field.distance(mul_add(center, along, way));
            let room = d - sphere.radius;
            if room <= 0.0 && dot(way, n) < 0.0 {
                let to_plane = d - along * dot(way, n);
                if past(to_plane, n) > past(distance, normal) {
                    (distance, normal) = (to_plane, n);
                }
                break;
            }
            if last {
                break;
            }
            t += room.max(SWEEP_LEAST_STEP);
        }
    }
    let separation = distance - sphere.radius;
    if separation > speculative && past(distance, normal) <= 0.0 {
        return;
    }
    manifold.normal = normal;
    let mut pt = LocalManifoldPoint::default();
    pt.point = mul_sub(center, 0.5 * (distance + sphere.radius), normal);
    pt.separation = separation;
    pt.pair = FEATURE_PAIR_SINGLE;
    manifold.points.push(pt);
}

/// Marches the ray by the distance it has to spare, so the field must not grow faster than 1.
pub fn ray_cast_field(field: &dyn DistanceField, input: &RayCastInput) -> CastOutput {
    let mut output = CastOutput::default();
    let mut total = 0.0;
    let direction = get_length_and_normalize(&mut total, input.translation);
    let end = total * input.max_fraction;
    let mut t = 0.0;
    for iteration in 0..64 {
        let p = mul_add(input.origin, t, direction);
        let (distance, normal) = field.distance(p);
        if distance <= overlap_slop() {
            output.hit = true;
            output.point = p;
            output.normal = normal;
            output.fraction = if total > 0.0 { t / total } else { 0.0 };
            output.iterations = iteration;
            return output;
        }
        t += distance;
        if t > end {
            break;
        }
    }
    output
}

/// Whether any of the proxy's points comes within its radius of the solid.
pub fn overlap_field(field: &dyn DistanceField, transform: Transform, proxy: &ShapeProxy) -> bool {
    proxy.points.iter().any(|&p| field.distance(inv_transform_point(transform, p)).0 < proxy.radius + overlap_slop())
}

/// The closest point of the solid to `target`, and how far it is.
pub fn closest_point_on_field(field: &dyn DistanceField, target: Vec3) -> (Vec3, f32) {
    let (distance, normal) = field.distance(target);
    (mul_sub(target, distance, normal), distance)
}
