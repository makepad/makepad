// Port of box3d/test/test_distance.c

use makepad_box3d::distance::{shape_cast, shape_distance, time_of_impact};
use makepad_box3d::math_functions::*;
use makepad_box3d::types::{
    DistanceInput, ShapeCastPairInput, ShapeProxy, SimplexCache, Sweep, TOIInput, TOIState,
};
use makepad_box3d::{ensure, ensure_small};

#[test]
fn segment_distance_test() {
    let p1 = vec3(-1.0, -1.0, 0.0);
    let q1 = vec3(-1.0, 1.0, 0.0);
    let p2 = vec3(2.0, 0.0, 0.0);
    let q2 = vec3(1.0, 0.0, 0.0);

    let result = segment_distance(p1, q1, p2, q2);

    ensure_small!(result.fraction1 - 0.5, f32::EPSILON);
    ensure_small!(result.fraction2 - 1.0, f32::EPSILON);
    ensure_small!(result.point1.x + 1.0, f32::EPSILON);
    ensure_small!(result.point1.y, f32::EPSILON);
    ensure_small!(result.point1.z, f32::EPSILON);
    ensure_small!(result.point2.x - 1.0, f32::EPSILON);
    ensure_small!(result.point2.y, f32::EPSILON);
    ensure_small!(result.point2.z, f32::EPSILON);
}

#[test]
fn shape_distance_test() {
    let vas = [
        vec3(-1.0, -1.0, 0.0),
        vec3(1.0, -1.0, 0.0),
        vec3(1.0, 1.0, 0.0),
        vec3(-1.0, 1.0, 0.0),
    ];

    let vbs = [vec3(2.0, -1.0, 0.0), vec3(2.0, 1.0, 0.0)];

    let input = DistanceInput {
        proxy_a: ShapeProxy { points: &vas, radius: 0.0 },
        proxy_b: ShapeProxy { points: &vbs, radius: 0.0 },
        transform: Transform::IDENTITY,
        use_radii: false,
    };

    let mut cache = SimplexCache::default();
    let output = shape_distance(&input, &mut cache, None);

    ensure_small!(output.distance - 1.0, f32::EPSILON);
}

#[test]
fn shape_cast_test() {
    let vas = [
        vec3(-1.0, -1.0, 0.0),
        vec3(1.0, -1.0, 0.0),
        vec3(1.0, 1.0, 0.0),
        vec3(-1.0, 1.0, 0.0),
    ];

    let vbs = [vec3(2.0, -1.0, 0.0), vec3(2.0, 1.0, 0.0)];

    let input = ShapeCastPairInput {
        proxy_a: ShapeProxy { points: &vas, radius: 0.0 },
        proxy_b: ShapeProxy { points: &vbs, radius: 0.0 },
        transform: Transform::IDENTITY,
        translation_b: vec3(-2.0, 0.0, 0.0),
        max_fraction: 1.0,
        can_encroach: false,
    };

    let output = shape_cast(&input);

    ensure!(output.hit);
    ensure_small!(output.fraction - 0.5, 0.005);
}

#[test]
fn time_of_impact_test() {
    let vas = [
        vec3(-1.0, -1.0, 0.0),
        vec3(1.0, -1.0, 0.0),
        vec3(1.0, 1.0, 0.0),
        vec3(-1.0, 1.0, 0.0),
    ];

    let vbs = [vec3(2.0, -1.0, 0.0), vec3(2.0, 1.0, 0.0)];

    let input = TOIInput {
        proxy_a: ShapeProxy { points: &vas, radius: 0.0 },
        proxy_b: ShapeProxy { points: &vbs, radius: 0.0 },
        sweep_a: Sweep {
            local_center: Vec3::ZERO,
            c1: Vec3::ZERO,
            c2: Vec3::ZERO,
            q1: Quat::IDENTITY,
            q2: Quat::IDENTITY,
        },
        sweep_b: Sweep {
            local_center: Vec3::ZERO,
            c1: Vec3::ZERO,
            c2: vec3(-2.0, 0.0, 0.0),
            q1: Quat::IDENTITY,
            q2: Quat::IDENTITY,
        },
        max_fraction: 1.0,
    };

    let output = time_of_impact(&input);

    ensure!(output.state == TOIState::Hit);
    ensure_small!(output.fraction - 0.5, 0.005);
}
