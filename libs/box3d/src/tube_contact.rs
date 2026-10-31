use crate::constants::MAX_MANIFOLD_POINTS;
use crate::contact::{mix_friction, mix_restitution, Contact, SIM_ENABLE_HIT_EVENT};
use crate::convex_manifold::{collide_capsule_and_sphere, collide_capsules, collide_hull_and_capsule};
use crate::manifold::make_feature_id;
use crate::math_functions::*;
use crate::physics_world::{TaskContext, World};
use crate::shape::{get_shape_materials, Shape, ShapeGeometry};
use crate::types::{LocalManifold, Manifold, SimplexCache};

const MAX_TUBE_MANIFOLDS: usize = 8;

/// Per-worker buffers retained across tube contact updates.
#[derive(Clone, Debug, Default)]
pub struct TubeCollideScratch {
    old_manifolds: Vec<Manifold>,
    manifolds: Vec<(f32, Manifold)>,
    geom_manifold: LocalManifold,
}

fn aabb_distance_squared(a: AABB, b: AABB) -> f32 {
    let gap = max(Vec3::ZERO, max(sub(a.lower_bound, b.upper_bound), sub(b.lower_bound, a.upper_bound)));
    length_squared(gap)
}

pub fn update_tube_contact(
    world: &World,
    task_context: &mut TaskContext,
    contact: &mut Contact,
    shape_a: &Shape,
    xf_a: WorldTransform,
    shape_b: &Shape,
    xf_b: WorldTransform,
) -> bool {
    let tube_a = shape_a.as_tube();
    let transform_b_to_a = inv_mul_world_transforms(xf_a, xf_b);
    let transform_a_to_b = invert_transform(transform_b_to_a);
    let spec = world.speculative_distance;
    let part_count_b = match &shape_b.geom {
        ShapeGeometry::Tube(tube) => tube.points.len(),
        ShapeGeometry::Capsule(_) | ShapeGeometry::Sphere(_) | ShapeGeometry::Hull(_) => 1,
        _ => unreachable!("unsupported tube contact"),
    };

    // Transform each B bound once, rather than once per segment of A.
    let mut bounds_b = [AABB::default(); 64];
    for j in 0..part_count_b {
        bounds_b[j] = match &shape_b.geom {
            ShapeGeometry::Tube(tube) => crate::capsule::compute_capsule_aabb(&tube.segment(j), transform_b_to_a),
            _ => crate::shape::compute_shape_aabb(shape_b, transform_b_to_a),
        };
    }

    let scratch = &mut task_context.tube_scratch;
    scratch.old_manifolds.clear();
    scratch.old_manifolds.extend_from_slice(&contact.manifolds);
    scratch.manifolds.clear();
    scratch.manifolds.reserve(MAX_TUBE_MANIFOLDS);
    scratch.geom_manifold.points.clear();
    scratch.geom_manifold.points.reserve(MAX_MANIFOLD_POINTS);

    let matrix_a = make_matrix_from_quat(xf_a.q);
    let matrix_b = make_matrix_from_quat(xf_b.q);
    let offset_b = sub_pos(xf_a.p, xf_b.p);
    for i in 0..tube_a.points.len() {
        let capsule_a = tube_a.segment(i);
        let bounds_a = crate::capsule::compute_capsule_aabb(&capsule_a, Transform::IDENTITY);
        for j in 0..part_count_b {
            if aabb_distance_squared(bounds_a, bounds_b[j]) > spec * spec {
                continue;
            }

            let geom = &mut scratch.geom_manifold;
            let capacity = MAX_MANIFOLD_POINTS as i32;
            let flip = match &shape_b.geom {
                ShapeGeometry::Tube(tube) => {
                    collide_capsules(geom, capacity, &capsule_a, &tube.segment(j), transform_b_to_a, spec);
                    false
                }
                ShapeGeometry::Capsule(capsule) => {
                    collide_capsules(geom, capacity, &capsule_a, capsule, transform_b_to_a, spec);
                    false
                }
                ShapeGeometry::Sphere(sphere) => {
                    collide_capsule_and_sphere(geom, capacity, &capsule_a, sphere, transform_b_to_a, spec);
                    false
                }
                ShapeGeometry::Hull(hull) => {
                    let mut cache = SimplexCache::default();
                    collide_hull_and_capsule(geom, capacity, hull, &capsule_a, transform_a_to_b, &mut cache, spec);
                    true
                }
                _ => unreachable!("unsupported tube contact"),
            };
            let depth = geom.points.iter().fold(f32::MAX, |depth, p| min_float(depth, p.separation));
            if depth >= spec {
                continue;
            }

            let mut manifold = Manifold::default();
            manifold.normal = if flip { neg(mul_mv(matrix_b, geom.normal)) } else { mul_mv(matrix_a, geom.normal) };
            manifold.point_count = geom.points.len().min(MAX_MANIFOLD_POINTS) as i32;
            for k in 0..manifold.point_count as usize {
                let source = &geom.points[k];
                let target = &mut manifold.points[k];
                if flip {
                    // Hull manifolds are in B's frame; swap the world-oriented anchors.
                    target.anchor_b = mul_mv(matrix_b, source.point);
                    target.anchor_a = sub(target.anchor_b, offset_b);
                } else {
                    target.anchor_a = mul_mv(matrix_a, source.point);
                    target.anchor_b = add(target.anchor_a, offset_b);
                }
                target.separation = source.separation;
                target.feature_id = make_feature_id(source.pair);
                target.triangle_index = (i * 64 + j) as i32;
            }

            if scratch.manifolds.len() < MAX_TUBE_MANIFOLDS {
                scratch.manifolds.push((depth, manifold));
            } else {
                let mut shallowest = 0;
                for k in 1..scratch.manifolds.len() {
                    if scratch.manifolds[k].0 > scratch.manifolds[shallowest].0 {
                        shallowest = k;
                    }
                }
                if depth < scratch.manifolds[shallowest].0 {
                    scratch.manifolds[shallowest] = (depth, manifold);
                }
            }
        }
    }

    // Keep solver order stable when the depths change.
    scratch.manifolds.sort_unstable_by_key(|(_, m)| m.points[0].triangle_index);
    contact.manifolds.set_count(scratch.manifolds.len() as i32);
    for (target, (_, source)) in contact.manifolds.iter_mut().zip(&scratch.manifolds) {
        *target = *source;
        let key = target.points[0].triangle_index;
        if let Some(old) = scratch.old_manifolds.iter_mut().find(|m| m.point_count > 0 && m.points[0].triangle_index == key) {
            target.friction_impulse = old.friction_impulse;
            target.rolling_impulse = old.rolling_impulse;
            target.twist_impulse = old.twist_impulse;
            for point in &mut target.points[..target.point_count as usize] {
                for old_point in &mut old.points[..old.point_count as usize] {
                    if point.feature_id == old_point.feature_id {
                        point.normal_impulse = old_point.normal_impulse;
                        point.persisted = true;
                        old_point.feature_id = u32::MAX;
                        break;
                    }
                }
            }
        }
    }

    let material_a = get_shape_materials(shape_a)[0];
    let material_b = get_shape_materials(shape_b)[0];
    contact.friction = mix_friction(world, material_a.friction, material_a.user_material_id, material_b.friction, material_b.user_material_id);
    contact.restitution = mix_restitution(world, material_a.restitution, material_a.user_material_id, material_b.restitution, material_b.user_material_id);
    contact.rolling_resistance = 0.0;
    contact.tangent_velocity = sub(rotate_vector(xf_a.q, material_a.tangent_velocity), rotate_vector(xf_b.q, material_b.tangent_velocity));

    let touching = !contact.manifolds.is_empty();
    if touching && (shape_a.enable_hit_events || shape_b.enable_hit_events) {
        contact.flags |= SIM_ENABLE_HIT_EVENT;
    } else {
        contact.flags &= !SIM_ENABLE_HIT_EVENT;
    }
    touching
}
