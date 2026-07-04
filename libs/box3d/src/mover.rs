// Port of box3d/src/mover.c — character mover plane solver.

use crate::constants::linear_slop;
use crate::math_functions::{abs_float, clamp_float, dot, min_float, mul_add, mul_sub, Vec3};
use crate::math_internal::plane_separation;
use crate::types::{CollisionPlane, PlaneSolverResult};

/// Solves the position of a mover that satisfies the given collision planes.
/// C: b3SolvePlanes(targetDelta, planes, count) — count is planes.len().
pub fn solve_planes(target_delta: Vec3, planes: &mut [CollisionPlane]) -> PlaneSolverResult {
    let count = planes.len();
    for i in 0..count {
        planes[i].push = 0.0;
    }

    let mut delta = target_delta;
    let tolerance = linear_slop();

    let mut iteration = 0;
    while iteration < 20 {
        let mut total_push = 0.0;
        for plane_index in 0..count {
            let plane = &mut planes[plane_index];

            // Add slop to prevent jitter
            let separation = plane_separation(plane.plane, delta) + linear_slop();

            let mut push = -separation;

            // Clamp accumulated push
            let accumulated_push = plane.push;
            plane.push = clamp_float(plane.push + push, 0.0, plane.push_limit);
            push = plane.push - accumulated_push;
            delta = mul_add(delta, push, plane.plane.normal);

            // Track maximum push for convergence
            total_push += abs_float(push);
        }

        if total_push < tolerance {
            break;
        }

        iteration += 1;
    }

    PlaneSolverResult { delta, iteration_count: iteration }
}

/// Clips the velocity against the given collision planes. Planes with zero push
/// or clip_velocity set to false are skipped.
/// C: b3ClipVector(vector, planes, count) — count is planes.len().
pub fn clip_vector(vector: Vec3, planes: &[CollisionPlane]) -> Vec3 {
    let mut v = vector;

    for plane in planes {
        if plane.push == 0.0 || !plane.clip_velocity {
            continue;
        }

        v = mul_sub(v, min_float(0.0, dot(v, plane.plane.normal)), plane.plane.normal);
    }

    v
}
