use crate::capsule::*;
use crate::math_functions::*;
use crate::types::{Capsule, CastOutput, MassData, PlaneResult, RayCastInput, ShapeCastInput, ShapeProxy, Tube};

pub fn is_valid_tube(tube: &Tube) -> bool {
    (3..=64).contains(&tube.points.len())
        && is_valid_float(tube.radius)
        && tube.radius >= 0.0
        && tube.points.iter().all(|&p| is_valid_vec3(p))
}

pub fn compute_tube_mass(tube: &Tube, density: f32) -> MassData {
    let mut parts = [MassData::default(); 64];
    let mut mass = 0.0;
    let mut center = Vec3::ZERO;
    for i in 0..tube.points.len() {
        let part = compute_capsule_mass(&tube.segment(i), 1.0);
        parts[i] = part;
        mass += part.mass;
        center = mul_add(center, part.mass, part.center);
    }
    if mass > 0.0 {
        center = mul_sv(1.0 / mass, center);
    } else {
        // Zero-radius tubes still need a useful geometric centroid.
        for &point in &tube.points {
            center = add(center, point);
        }
        center = mul_sv(1.0 / tube.points.len() as f32, center);
    }
    let mut inertia = Matrix3::ZERO;
    for part in &parts[..tube.points.len()] {
        inertia = add_mm(inertia, add_mm(part.inertia, steiner(part.mass, sub(part.center, center))));
    }
    MassData { mass: density * mass, center, inertia: mul_sm(density, inertia) }
}

pub fn compute_tube_aabb(tube: &Tube, transform: Transform) -> AABB {
    let mut bounds = compute_capsule_aabb(&tube.segment(0), transform);
    for i in 1..tube.points.len() {
        bounds = aabb_union(bounds, compute_capsule_aabb(&tube.segment(i), transform));
    }
    bounds
}

pub fn compute_swept_tube_aabb(tube: &Tube, xf1: Transform, xf2: Transform) -> AABB {
    let mut bounds = compute_swept_capsule_aabb(&tube.segment(0), xf1, xf2);
    for i in 1..tube.points.len() {
        bounds = aabb_union(bounds, compute_swept_capsule_aabb(&tube.segment(i), xf1, xf2));
    }
    bounds
}

pub fn ray_cast_tube(tube: &Tube, input: &RayCastInput) -> CastOutput {
    let mut closest = CastOutput::default();
    let mut input = *input;
    for i in 0..tube.points.len() {
        let hit = ray_cast_capsule(&tube.segment(i), &input);
        if hit.hit && (!closest.hit || hit.fraction < closest.fraction) {
            input.max_fraction = hit.fraction;
            closest = hit;
        }
    }
    closest
}

pub fn shape_cast_tube(tube: &Tube, input: &ShapeCastInput) -> CastOutput {
    let mut closest = CastOutput::default();
    let mut input = *input;
    for i in 0..tube.points.len() {
        let hit = shape_cast_capsule(&tube.segment(i), &input);
        if hit.hit && (!closest.hit || hit.fraction < closest.fraction) {
            input.max_fraction = hit.fraction;
            closest = hit;
        }
    }
    closest
}

pub fn overlap_tube(tube: &Tube, transform: Transform, proxy: &ShapeProxy) -> bool {
    (0..tube.points.len()).any(|i| overlap_capsule(&tube.segment(i), transform, proxy))
}

pub fn collide_mover_and_tube(planes: &mut [PlaneResult], tube: &Tube, mover: &Capsule) -> i32 {
    let mut count = 0;
    for i in 0..tube.points.len() {
        if count == planes.len() {
            break;
        }
        count += collide_mover_and_capsule(&mut planes[count], &tube.segment(i), mover) as usize;
    }
    count as i32
}

pub(crate) fn closest_point_on_tube(tube: &Tube, target: Vec3, use_radii: bool) -> (Vec3, f32) {
    let mut closest = Vec3::ZERO;
    let mut distance = f32::MAX;
    for i in 0..tube.points.len() {
        let capsule = tube.segment(i);
        let points = [capsule.center1, capsule.center2];
        let input = crate::types::DistanceInput {
            proxy_a: ShapeProxy { points: &points, radius: tube.radius },
            proxy_b: ShapeProxy { points: std::slice::from_ref(&target), radius: 0.0 },
            transform: Transform::IDENTITY,
            use_radii,
        };
        let mut cache = crate::types::SimplexCache::default();
        let output = crate::distance::shape_distance(&input, &mut cache, None);
        if output.distance < distance {
            closest = output.point_a;
            distance = output.distance;
        }
    }
    (closest, distance)
}

/// Time of impact over the convex segment pairs; the loop's interior stays empty.
pub(crate) fn tube_time_of_impact(
    shape_a: &crate::shape::Shape,
    shape_b: &crate::shape::Shape,
    sweep_a: &crate::types::Sweep,
    sweep_b: &crate::types::Sweep,
    max_fraction: f32,
) -> crate::types::TOIOutput {
    use crate::shape::ShapeGeometry;
    use crate::types::{TOIInput, TOIOutput, TOIState};

    let part_count = |shape: &crate::shape::Shape| match &shape.geom {
        ShapeGeometry::Tube(tube) => tube.points.len(),
        ShapeGeometry::Capsule(_) | ShapeGeometry::Sphere(_) | ShapeGeometry::Hull(_) => 1,
        _ => 0,
    };
    let mut closest = TOIOutput { state: TOIState::Separated, fraction: max_fraction, ..Default::default() };
    for i in 0..part_count(shape_a) {
        let mut buffer_a = [Vec3::ZERO; 2];
        let proxy_a = match &shape_a.geom {
            ShapeGeometry::Tube(tube) => {
                let segment = tube.segment(i);
                buffer_a = [segment.center1, segment.center2];
                ShapeProxy { points: &buffer_a, radius: tube.radius }
            }
            _ => crate::shape::make_shape_proxy(shape_a, &mut buffer_a),
        };
        for j in 0..part_count(shape_b) {
            let mut buffer_b = [Vec3::ZERO; 2];
            let proxy_b = match &shape_b.geom {
                ShapeGeometry::Tube(tube) => {
                    let segment = tube.segment(j);
                    buffer_b = [segment.center1, segment.center2];
                    ShapeProxy { points: &buffer_b, radius: tube.radius }
                }
                _ => crate::shape::make_shape_proxy(shape_b, &mut buffer_b),
            };
            let input = TOIInput { proxy_a, proxy_b, sweep_a: *sweep_a, sweep_b: *sweep_b, max_fraction: closest.fraction };
            let output = crate::distance::time_of_impact(&input);
            if 0.0 < output.fraction && output.fraction < closest.fraction {
                closest = output;
            }
        }
    }
    closest
}
