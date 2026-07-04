// Port of box3d/src/capsule.c

use crate::b3_assert;
use crate::constants::{linear_slop, overlap_slop};
use crate::math_functions::*;
use crate::sphere::ray_cast_sphere;
use crate::types::{
    Capsule, CastOutput, DistanceInput, MassData, PlaneResult, RayCastInput, ShapeCastInput,
    ShapeCastPairInput, ShapeProxy, SimplexCache, Sphere,
};

pub fn compute_capsule_mass(shape: &Capsule, density: f32) -> MassData {
    let c1 = shape.center1;
    let c2 = shape.center2;
    let r = shape.radius;

    // Cylinder
    let cylinder_height = distance(c1, c2);
    let cylinder_volume = PI * r * r * cylinder_height;
    let cylinder_mass = cylinder_volume * density;

    // Sphere
    let sphere_volume = (4.0 / 3.0) * PI * r * r * r;
    let sphere_mass = sphere_volume * density;

    // Local accumulated inertia
    let mut inertia = add_mm(cylinder_inertia(cylinder_mass, r, cylinder_height), sphere_inertia(sphere_mass, r));

    let steiner = 0.125 * sphere_mass * (3.0 * r + 2.0 * cylinder_height) * cylinder_height;
    inertia.cx.x += steiner;
    inertia.cz.z += steiner;

    // Align capsule axis with chosen up-axis
    let mut rotation = Matrix3::IDENTITY;
    if cylinder_height * cylinder_height > 1000.0 * f32::MIN_POSITIVE {
        let direction = normalize(sub(c2, c1));
        let q = compute_quat_between_unit_vectors(Vec3::AXIS_Y, direction);
        rotation = make_matrix_from_quat(q);
    }

    let mass = sphere_mass + cylinder_mass;
    let center = mul_sv(0.5, add(c1, c2));

    MassData {
        mass,
        center,
        // Rotate the central inertia into the shape frame
        inertia: mul_mm(rotation, mul_mm(inertia, transpose(rotation))),
    }
}

pub fn compute_capsule_aabb(shape: &Capsule, transform: Transform) -> AABB {
    let r = shape.radius;

    let center1 = transform_point(transform, shape.center1);
    let center2 = transform_point(transform, shape.center2);
    let extent = vec3(r, r, r);

    AABB {
        lower_bound: sub(min(center1, center2), extent),
        upper_bound: add(max(center1, center2), extent),
    }
}

pub fn compute_swept_capsule_aabb(shape: &Capsule, xf1: Transform, xf2: Transform) -> AABB {
    let r = vec3(shape.radius, shape.radius, shape.radius);
    let a = transform_point(xf1, shape.center1);
    let b = transform_point(xf1, shape.center2);
    let c = transform_point(xf2, shape.center1);
    let d = transform_point(xf2, shape.center2);

    AABB {
        lower_bound: sub(min(min(a, b), min(c, d)), r),
        upper_bound: add(max(max(a, b), max(c, d)), r),
    }
}

pub fn overlap_capsule(shape: &Capsule, shape_transform: Transform, proxy: &ShapeProxy) -> bool {
    let points_a = [shape.center1, shape.center2];
    let input = DistanceInput {
        proxy_a: ShapeProxy { points: &points_a, radius: shape.radius },
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
pub fn ray_cast_capsule(shape: &Capsule, input: &RayCastInput) -> CastOutput {
    b3_assert!(is_valid_ray(input));

    let c1 = shape.center1;
    let c2 = shape.center2;
    let r = shape.radius;

    // Initialize result structure
    let mut output = CastOutput::default();

    let d = sub(c2, c1);

    // Fall back to sphere if the capsule is short
    let tol = 0.01 * linear_slop();
    let length_squared_d = length_squared(d);
    if length_squared_d < tol * tol {
        let sphere_center = mul_sv(0.5, add(shape.center1, shape.center2));
        let sphere = Sphere { center: sphere_center, radius: shape.radius };
        return ray_cast_sphere(&sphere, input);
    }

    // Vector from first center to ray origin.
    let s = sub(input.origin, c1);

    // Capsule axis
    let length = length_squared_d.sqrt();
    let axis = mul_sv(1.0 / length, d);

    // Project ray origin onto capsule axis.
    let u = dot(s, axis);

    // Closest point on infinite capsule axis, relative to c1.
    let c = mul_sv(u, axis);

    // Vector from closest point to ray origin
    let sc = sub(s, c);

    // Squared distance from ray origin to capsule axis
    let sc2 = length_squared(sc);

    // Is the ray origin within the infinite cylinder along the capsule axis?
    if sc2 < r * r {
        // Clamped barycentric coordinate of ray origin projected onto capsule axis.
        let u_clamped = clamp_float(u, 0.0, length);

        // The closest point on the bounded capsule segment, relative to c1.
        let cp = mul_sv(u_clamped, axis);

        // Vector from ray origin to closest point on segment.
        let scp = sub(s, cp);

        // Squared distance of ray origin from capsule segment.
        let scp2 = length_squared(scp);

        // Is the ray origin within the capsule?
        if scp2 < r * r {
            output.hit = true;
            output.point = input.origin;
            return output;
        }

        // The ray can hit an endcap.
        let sphere = Sphere { center: add(c1, cp), radius: r };

        return ray_cast_sphere(&sphere, input);
    }

    // Ray axis. A zero length ray reaching here starts outside the capsule, so it misses.
    // Same zero length convention as ray_cast_sphere.
    let dr = input.translation;
    let mut ray_length = 0.0;
    let ray_axis = get_length_and_normalize(&mut ray_length, dr);
    if ray_length == 0.0 {
        return output;
    }

    // Barycentric coordinate of ray end point.
    let v = u + input.max_fraction * dot(dr, axis);

    // Early out: does the projected ray fall outside the capsule?
    if (u < -r && v < -r) || (length + r < u && length + r < v) {
        return output;
    }

    // Compute the closest point between the ray segment and the capsule segment.
    // See Real-Time Collision Detection, section 5.1.9

    // Closest point on capsule : a1 = segment unit axis, t1 = unknown fraction
    // p1 = t1 * a1

    // Closet point on ray : a2 = ray unit axis, t2 = unknown fraction
    // p2 = s + t2 * a2

    // Closest point perpendicularity conditions.
    // dot(p2 - p1, a1) = 0
    // dot(p2 - p1, a2) = 0

    // Group : a12 = dot(a1, a2), sa1 = dot(s, a1), sa2 = dot(s, a2)
    // t1       - a12 * t2 = sa1
    // a12 * t1 -       t2 = sa2

    // Solve
    // https://en.wikipedia.org/wiki/Cramer%27s_rule
    // det = 1 - a12 * a12
    // t1 = (sa1 - a12 * sa2) / det
    // t2 = (a12 * sa1 - sa2) / det

    let a1 = axis;
    let a2 = ray_axis;
    let a12 = dot(a1, a2);

    // Ray distance to the near intersection with the infinite cylinder. Length units.
    let tr;

    let det = 1.0 - a12 * a12;
    if det < f32::EPSILON {
        // Solve the 2D problem of ray versus circle starting at the ray origin, where the circle is
        // the axial view of the infinite capsule cylinder. This works well when the ray origin is
        // not too far from the capsule axis.

        // Instead of a cross product, subtract the parallel part to get a perpendicular vector. Non-dimensional.
        let perp = mul_sub(a2, a12, a1);
        let perp2 = length_squared(perp);

        // Project to origin to infinite capsule axis vector onto the perpendicular vector. beta has length units.
        let beta = dot(sc, perp);

        // Setup quadratic root finder.
        let gamma = sc2 - r * r;

        // Discriminant
        let disc = beta * beta - perp2 * gamma;

        // Casting away from the axis, or the perpendicular gap never closes to the radius.
        if beta >= 0.0 || disc < 0.0 {
            return output;
        }

        // Quadratic near root. Expressed in an alternate form to avoid the (-beta - sqrt) cancellation as
        // the ray nears parallel.
        tr = gamma / (-beta + disc.sqrt());
    } else {
        // Ray and capsules axes are not parallel.

        // Closest points between the infinite ray and the infinite capsule axis.
        let inv_det = 1.0 / det;
        let sa1 = u;
        let sa2 = dot(s, a2);

        let t1 = (sa1 - a12 * sa2) * inv_det;
        let t2 = (a12 * sa1 - sa2) * inv_det;

        // Closest points
        let p1 = mul_sv(t1, a1);
        let p2 = mul_add(s, t2, a2);

        // Vector from closest point on infinite capsule to infinite ray.
        let g = sub(p2, p1);

        let g2 = length_squared(g);
        if g2 > r * r {
            // Early out: closest point on infinite ray is outside infinite cylinder.
            return output;
        }

        // Intersect the infinite ray with the infinite cylinder. Like ray versus sphere this is done
        // relative to the closest point to avoid round-off errors. Length units, not a fraction.
        // https://en.wikipedia.org/wiki/Line-cylinder_intersection
        let h = ((r * r - g2) * inv_det).sqrt();

        tr = t2 - h;
    }

    // Outside ray?
    if tr < 0.0 || input.max_fraction * ray_length < tr {
        return output;
    }

    // The corresponding distance on the capsule axis. Length units.
    let tc = u + tr * a12;

    // Outside c1 end?
    if tc < 0.0 {
        // Ray cast sphere 1.
        let sphere = Sphere { center: c1, radius: r };
        return ray_cast_sphere(&sphere, input);
    }

    // Outside c2 end?
    if length < tc {
        // Ray cast sphere 2.
        let sphere = Sphere { center: c2, radius: r };
        return ray_cast_sphere(&sphere, input);
    }

    // Hit point on capsule side, relative to c1.
    let p = mul_add(s, tr, ray_axis);

    // Hit normal.
    let mut normal = mul_sub(p, tc, axis);
    normal = normalize(normal);

    output.point = add(c1, p);
    output.normal = normal;
    output.fraction = clamp_float(tr / ray_length, 0.0, input.max_fraction);
    output.hit = true;
    output
}

pub fn shape_cast_capsule(capsule: &Capsule, input: &ShapeCastInput) -> CastOutput {
    let points_a = [capsule.center1, capsule.center2];
    let pair_input = ShapeCastPairInput {
        proxy_a: ShapeProxy { points: &points_a, radius: capsule.radius },
        proxy_b: input.proxy,
        transform: Transform::IDENTITY,
        translation_b: input.translation,
        max_fraction: input.max_fraction,
        can_encroach: input.can_encroach,
    };

    crate::distance::shape_cast(&pair_input)
}

pub fn collide_mover_and_capsule(result: &mut PlaneResult, shape: &Capsule, mover: &Capsule) -> i32 {
    let total_radius = mover.radius + shape.radius;

    let approach = segment_distance(shape.center1, shape.center2, mover.center1, mover.center2);

    // The normal points from the shape toward the mover.
    let mut distance = 0.0;
    let mut normal = get_length_and_normalize(&mut distance, sub(approach.point2, approach.point1));

    if distance > total_radius {
        return 0;
    }

    let linear_slop = linear_slop();
    if distance < linear_slop {
        // Deep overlap: the core segments intersect. Pick an arbitrary direction perpendicular
        // the to capsule axis.
        let mut mover_length = 0.0;
        let mover_axis = get_length_and_normalize(&mut mover_length, sub(mover.center2, mover.center1));
        normal = if mover_length > linear_slop { perp(mover_axis) } else { Vec3::AXIS_Y };
        distance = 0.0;
    }

    let plane = Plane { normal, offset: total_radius - distance };
    *result = PlaneResult { plane, point: approach.point1 };
    1
}
