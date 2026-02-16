use crate::narrow_phase::PREDICTION_DISTANCE;
use crate::rigid_body::{BodyType, RigidBody};
use makepad_math::*;

/// Brute-force O(n^2) broad phase: test all AABB pairs.
/// Writes pairs (i, j) where i < j, AABBs overlap (with prediction margin),
/// and at least one body is dynamic.
/// Clears and reuses the provided buffers — no allocations after warmup.
pub fn broad_phase(bodies: &[RigidBody], aabbs: &mut Vec<Aabb>, pairs: &mut Vec<(usize, usize)>) {
    let n = bodies.len();
    let margin = vec3f(
        PREDICTION_DISTANCE,
        PREDICTION_DISTANCE,
        PREDICTION_DISTANCE,
    );

    aabbs.clear();
    for b in bodies {
        let mut aabb = Aabb::from_cuboid(b.half_extents, &b.pose);
        // Expand AABB by prediction distance so near-touching pairs enter narrow phase
        aabb.min = aabb.min - margin;
        aabb.max = aabb.max + margin;
        aabbs.push(aabb);
    }

    pairs.clear();
    for i in 0..n {
        for j in (i + 1)..n {
            if bodies[i].body_type == BodyType::Fixed && bodies[j].body_type == BodyType::Fixed {
                continue;
            }
            if aabbs[i].overlaps(&aabbs[j]) {
                pairs.push((i, j));
            }
        }
    }
}
