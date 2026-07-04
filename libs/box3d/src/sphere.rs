// Port of box3d/src/sphere.c
// Dirk Gregorius contributed portions of this code

use crate::b3_assert;
use crate::constants::{linear_slop, overlap_slop};
use crate::math_functions::*;
use crate::math_internal::make_diagonal_matrix;
use crate::types::{
    Capsule, CastOutput, DistanceInput, MassData, PlaneResult, RayCastInput, ShapeCastInput,
    ShapeCastPairInput, ShapeProxy, SimplexCache, Sphere,
};

pub fn compute_sphere_mass(shape: &Sphere, density: f32) -> MassData {
    let center = shape.center;
    let radius = shape.radius;

    let volume = 4.0 / 3.0 * PI * radius * radius * radius;
    let mass = volume * density;
    let ixx = 0.4 * mass * radius * radius;

    MassData {
        mass,
        center,
        // Inertia about the center of mass
        inertia: make_diagonal_matrix(ixx, ixx, ixx),
    }
}

pub fn compute_sphere_aabb(shape: &Sphere, transform: Transform) -> AABB {
    let center = transform_point(transform, shape.center);
    let radius = shape.radius;
    let extent = vec3(radius, radius, radius);
    AABB { lower_bound: sub(center, extent), upper_bound: add(center, extent) }
}

pub fn compute_swept_sphere_aabb(shape: &Sphere, xf1: Transform, xf2: Transform) -> AABB {
    let r = vec3(shape.radius, shape.radius, shape.radius);
    let center1 = transform_point(xf1, shape.center);
    let center2 = transform_point(xf2, shape.center);
    AABB {
        lower_bound: sub(min(center1, center2), r),
        upper_bound: add(max(center1, center2), r),
    }
}

pub fn overlap_sphere(shape: &Sphere, shape_transform: Transform, proxy: &ShapeProxy) -> bool {
    let input = DistanceInput {
        proxy_a: ShapeProxy { points: std::slice::from_ref(&shape.center), radius: shape.radius },
        proxy_b: *proxy,
        transform: inv_mul_transforms(shape_transform, Transform::IDENTITY),
        use_radii: true,
    };

    let mut cache = SimplexCache::default();
    let output = crate::distance::shape_distance(&input, &mut cache, None);
    output.distance < overlap_slop()
}

// Precision Improvements for Ray / Sphere Intersection - Ray Tracing Gems 2019
// http://www.codercorner.com/blog/?p=321
pub fn ray_cast_sphere(shape: &Sphere, input: &RayCastInput) -> CastOutput {
    b3_assert!(is_valid_ray(input));
    let mut output = CastOutput::default();

    let p = shape.center;

    // Shift ray so sphere center is the origin
    let s = sub(input.origin, p);

    let r = shape.radius;
    let rr = r * r;

    let mut length = 0.0;
    let d = get_length_and_normalize(&mut length, input.translation);
    if length == 0.0 {
        // zero length ray

        if length_squared(s) < rr {
            // initial overlap
            output.point = input.origin;
            output.hit = true;
        }

        return output;
    }

    // Find closest point on ray to origin

    // solve: dot(s + t * d, d) = 0
    let t = -dot(s, d);

    // c is the closest point on the line to the origin
    let c = mul_add(s, t, d);

    let cc = dot(c, c);

    if cc > rr {
        // closest point is outside the sphere
        return output;
    }

    // Pythagoras
    let h = (rr - cc).sqrt();

    let fraction = t - h;

    if fraction < 0.0 || input.max_fraction * length < fraction {
        // intersection is point outside the range of the ray segment

        if length_squared(s) < rr {
            // initial overlap
            output.point = input.origin;
            output.hit = true;
        }

        return output;
    }

    let hit_point = mul_add(s, fraction, d);

    output.fraction = fraction / length;

    if output.fraction > input.max_fraction {
        crate::core::log(&format!(
            "sphere input fraction = {}, output fraction = {}",
            input.max_fraction, output.fraction
        ));
        output.fraction = input.max_fraction;
    }

    output.normal = normalize(hit_point);
    output.point = mul_add(p, shape.radius, output.normal);
    output.hit = true;

    output
}

// Precision Improvements for Ray / Sphere Intersection - Ray Tracing Gems 2019
// http://www.codercorner.com/blog/?p=321
// This will do interior hits.
pub fn ray_cast_hollow_sphere(sphere: &Sphere, input: &RayCastInput) -> CastOutput {
    let p = sphere.center;

    let mut output = CastOutput::default();

    // Shift ray so sphere center is the origin
    let s = sub(input.origin, p);
    let d = normalize(input.translation);

    // Find closest point on ray to origin

    // solve: dot(s + t * d, d) = 0
    let t = -dot(s, d);

    // c is the closest point on the line to the origin
    let c = mul_add(s, t, d);

    let cc = dot(c, c);
    let r = sphere.radius;
    let rr = r * r;

    if cc > rr {
        // closest point is outside the sphere
        return output;
    }

    // Pythagoras
    let h = (rr - cc).sqrt();

    let mut fraction = t - h;

    if fraction < 0.0 {
        fraction = t + h;
    }

    if fraction < 0.0 {
        // behind the ray
        return output;
    }

    if fraction > input.max_fraction {
        return output;
    }

    let hit_point = mul_add(s, fraction, d);

    output.fraction = fraction;
    output.normal = normalize(hit_point);
    output.point = mul_add(p, sphere.radius, output.normal);
    output.hit = true;

    output
}

pub fn shape_cast_sphere(sphere: &Sphere, input: &ShapeCastInput) -> CastOutput {
    let pair_input = ShapeCastPairInput {
        proxy_a: ShapeProxy { points: std::slice::from_ref(&sphere.center), radius: sphere.radius },
        proxy_b: input.proxy,
        transform: Transform::IDENTITY,
        translation_b: input.translation,
        max_fraction: input.max_fraction,
        can_encroach: input.can_encroach,
    };

    crate::distance::shape_cast(&pair_input)
}

pub fn collide_mover_and_sphere(result: &mut PlaneResult, shape: &Sphere, mover: &Capsule) -> i32 {
    let total_radius = mover.radius + shape.radius;
    let closest = point_to_segment_distance(mover.center1, mover.center2, shape.center);

    // The normal points from the sphere toward the mover.
    let mut distance = 0.0;
    let mut normal = get_length_and_normalize(&mut distance, sub(closest, shape.center));

    if distance > total_radius {
        return 0;
    }

    let linear_slop = linear_slop();
    if distance < linear_slop {
        // Deep overlap: the mover axis passes through the sphere center, so no
        // direction is preferred. Push perpendicular to the mover axis.
        let mut length = 0.0;
        let axis = get_length_and_normalize(&mut length, sub(mover.center2, mover.center1));
        normal = if length > linear_slop { perp(axis) } else { Vec3::AXIS_Y };
        distance = 0.0;
    }

    let plane = Plane { normal, offset: total_radius - distance };
    *result = PlaneResult { plane, point: shape.center };
    1
}
