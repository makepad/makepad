// Port of box3d/test/test_hull.c
//
// Deviations:
// - byteCount assertions are skipped: the Rust HullData has no blob byte count.
// - memcmp(h1, h2, byteCount) → compare_hull_data (content comparison).
// - b3DestroyHull is Arc drop.

use makepad_box3d::hull::{
    clone_hull, compare_hull_data, create_cylinder, create_hull, make_box_hull,
};
use makepad_box3d::math_functions::{sub_mm, Vec3, PI};
use makepad_box3d::{ensure, ensure_small};

const S_CUBE_CORNERS: [Vec3; 8] = [
    Vec3 { x: 1.0, y: 1.0, z: 1.0 },
    Vec3 { x: -1.0, y: 1.0, z: 1.0 },
    Vec3 { x: -1.0, y: -1.0, z: 1.0 },
    Vec3 { x: 1.0, y: -1.0, z: 1.0 },
    Vec3 { x: 1.0, y: 1.0, z: -1.0 },
    Vec3 { x: -1.0, y: 1.0, z: -1.0 },
    Vec3 { x: -1.0, y: -1.0, z: -1.0 },
    Vec3 { x: 1.0, y: -1.0, z: -1.0 },
];

#[test]
fn create_hull_cube_test() {
    let hull = create_hull(&S_CUBE_CORNERS, 8);
    ensure!(hull.is_some());
    let hull = hull.unwrap();

    ensure!(hull.vertex_count() == 8);
    ensure!(hull.edge_count() == 24);
    ensure!(hull.face_count() == 6);

    // Euler's identity for convex polyhedron
    ensure!(hull.vertex_count() - hull.edge_count() / 2 + hull.face_count() == 2);

    let reference = make_box_hull(1.0, 1.0, 1.0);

    ensure_small!(hull.volume - reference.volume, 1e-4);
    ensure_small!(hull.surface_area - reference.surface_area, 1e-4);
    ensure_small!(hull.inner_radius - reference.inner_radius, f32::EPSILON);

    ensure_small!(hull.center.x - reference.center.x, 1e-5);
    ensure_small!(hull.center.y - reference.center.y, 1e-5);
    ensure_small!(hull.center.z - reference.center.z, 1e-5);

    ensure_small!(hull.aabb.lower_bound.x + 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.lower_bound.y + 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.lower_bound.z + 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.x - 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.y - 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.z - 1.0, f32::EPSILON);

    let d = sub_mm(hull.central_inertia, reference.central_inertia);
    ensure_small!(d.cx.x, 1e-4);
    ensure_small!(d.cy.y, 1e-4);
    ensure_small!(d.cz.z, 1e-4);
    ensure_small!(d.cx.y, 1e-4);
    ensure_small!(d.cx.z, 1e-4);
    ensure_small!(d.cy.z, 1e-4);
    ensure_small!(d.cy.x, 1e-4);
    ensure_small!(d.cz.x, 1e-4);
    ensure_small!(d.cz.y, 1e-4);
}

#[test]
fn create_hull_tetrahedron_test() {
    let points = [
        Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        Vec3 { x: 1.0, y: 0.0, z: 0.0 },
        Vec3 { x: 0.0, y: 1.0, z: 0.0 },
        Vec3 { x: 0.0, y: 0.0, z: 1.0 },
    ];

    let hull = create_hull(&points, 4);
    ensure!(hull.is_some());
    let hull = hull.unwrap();

    ensure!(hull.vertex_count() == 4);
    ensure!(hull.edge_count() == 12);
    ensure!(hull.face_count() == 4);
    ensure!(hull.vertex_count() - hull.edge_count() / 2 + hull.face_count() == 2);

    // Analytic values for the unit-corner tetrahedron at the origin.
    let expected_volume = 1.0f32 / 6.0;
    let expected_surface_area = 1.5 + 0.5 * 3.0f32.sqrt();
    let expected_inner_radius = 0.25 / 3.0f32.sqrt();

    ensure_small!(hull.volume - expected_volume, 1e-5);
    ensure_small!(hull.surface_area - expected_surface_area, 1e-5);
    ensure_small!(hull.inner_radius - expected_inner_radius, 1e-5);

    ensure_small!(hull.center.x - 0.25, 1e-5);
    ensure_small!(hull.center.y - 0.25, 1e-5);
    ensure_small!(hull.center.z - 0.25, 1e-5);

    ensure_small!(hull.aabb.lower_bound.x, f32::EPSILON);
    ensure_small!(hull.aabb.lower_bound.y, f32::EPSILON);
    ensure_small!(hull.aabb.lower_bound.z, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.x - 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.y - 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.z - 1.0, f32::EPSILON);
}

#[test]
fn create_hull_determinism_test() {
    let h1 = create_hull(&S_CUBE_CORNERS, 8);
    let h2 = create_hull(&S_CUBE_CORNERS, 8);
    ensure!(h1.is_some() && h2.is_some());
    let h1 = h1.unwrap();
    let h2 = h2.unwrap();

    // C: h1->byteCount == h2->byteCount — no byte count in the Rust port.
    ensure!(h1.hash != 0);
    ensure!(h1.hash == h2.hash);
    // C: memcmp(h1, h2, byteCount) == 0
    ensure!(compare_hull_data(&h1, &h2));
}

const SPHERE_N: usize = 6;

#[test]
fn create_hull_max_vertex_test() {
    // Sphere-sampled point cloud, dense enough that the builder has room to grow.
    let mut points = [Vec3::ZERO; SPHERE_N * SPHERE_N];
    let mut index = 0;
    for i in 0..SPHERE_N {
        let theta = PI * i as f32 / (SPHERE_N - 1) as f32;
        for j in 0..SPHERE_N {
            let phi = 2.0 * PI * j as f32 / SPHERE_N as f32;
            points[index].x = theta.sin() * phi.cos();
            points[index].y = theta.sin() * phi.sin();
            points[index].z = theta.cos();
            index += 1;
        }
    }

    // maxVertexCount honored as a strict cap.
    let h1 = create_hull(&points, 8);
    ensure!(h1.is_some());
    let h1 = h1.unwrap();
    ensure!(h1.vertex_count() <= 8);

    // Below the floor: clamps up to 4.
    let h2 = create_hull(&points, 1);
    ensure!(h2.is_some());
    let h2 = h2.unwrap();
    ensure!(h2.vertex_count() >= 4 && h2.vertex_count() <= 255);

    // Above the ceiling: clamps down to 255.
    let h3 = create_hull(&points, 1000);
    ensure!(h3.is_some());
    let h3 = h3.unwrap();
    ensure!(h3.vertex_count() >= 4 && h3.vertex_count() <= 255);
}

#[test]
fn create_hull_redundant_input_test() {
    // 8 cube corners + duplicates + interior points. Builder should produce the cube.
    let points = [
        Vec3 { x: 1.0, y: 1.0, z: 1.0 }, // corners
        Vec3 { x: -1.0, y: 1.0, z: 1.0 },
        Vec3 { x: -1.0, y: -1.0, z: 1.0 },
        Vec3 { x: 1.0, y: -1.0, z: 1.0 },
        Vec3 { x: 1.0, y: 1.0, z: -1.0 },
        Vec3 { x: -1.0, y: 1.0, z: -1.0 },
        Vec3 { x: -1.0, y: -1.0, z: -1.0 },
        Vec3 { x: 1.0, y: -1.0, z: -1.0 },
        Vec3 { x: 1.0, y: 1.0, z: 1.0 }, // duplicates
        Vec3 { x: 1.0, y: 1.0, z: 1.0 },
        Vec3 { x: 0.0, y: 0.0, z: 0.0 }, // interior points
        Vec3 { x: 0.5, y: 0.0, z: 0.0 },
        Vec3 { x: 0.0, y: 0.5, z: 0.0 },
        Vec3 { x: 0.0, y: 0.0, z: 0.5 },
        Vec3 { x: -0.5, y: 0.0, z: 0.0 },
        Vec3 { x: 0.0, y: -0.5, z: 0.0 },
        Vec3 { x: 0.0, y: 0.0, z: -0.5 },
        Vec3 { x: 0.25, y: 0.25, z: 0.25 },
        Vec3 { x: -0.25, y: -0.25, z: -0.25 },
        Vec3 { x: 0.5, y: 0.5, z: 0.5 },
    ];

    let hull = create_hull(&points, 8);
    ensure!(hull.is_some());
    let hull = hull.unwrap();

    ensure!(hull.vertex_count() == 8);
    ensure!(hull.edge_count() == 24);
    ensure!(hull.face_count() == 6);

    let reference = make_box_hull(1.0, 1.0, 1.0);

    ensure_small!(hull.volume - reference.volume, 1e-4);
    ensure_small!(hull.surface_area - reference.surface_area, 1e-4);
    ensure_small!(hull.inner_radius - reference.inner_radius, f32::EPSILON);

    ensure_small!(hull.center.x - reference.center.x, 1e-5);
    ensure_small!(hull.center.y - reference.center.y, 1e-5);
    ensure_small!(hull.center.z - reference.center.z, 1e-5);

    ensure_small!(hull.aabb.lower_bound.x + 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.lower_bound.y + 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.lower_bound.z + 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.x - 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.y - 1.0, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.z - 1.0, f32::EPSILON);
}

#[test]
fn create_hull_clone_test() {
    let original = create_hull(&S_CUBE_CORNERS, 8);
    ensure!(original.is_some());
    let original = original.unwrap();

    let clone = clone_hull(&original);
    ensure!(clone.is_some());
    let clone = clone.unwrap();
    // C: clone->byteCount == original->byteCount — no byte count in the Rust port.
    // C: memcmp(clone, original, byteCount) == 0
    ensure!(compare_hull_data(&clone, &original));
}

#[test]
fn create_hull_cylinder_test() {
    let height = 2.0f32;
    let radius = 1.0f32;
    let sides = 8;
    let y_offset = 0.0f32;

    let hull = create_cylinder(height, radius, y_offset, sides);

    ensure!(hull.vertex_count() == 2 * sides);
    ensure!(hull.edge_count() == 6 * sides);
    ensure!(hull.face_count() == sides + 2);

    // Analytic n-gon prism values (exact targets, not the circular cylinder approximations).
    let half_angle = PI / sides as f32;
    let cap_area = sides as f32 * 0.5 * radius * radius * (2.0 * half_angle).sin();
    let chord_len = 2.0 * radius * half_angle.sin();
    let lateral_area = sides as f32 * chord_len * height;

    let expected_volume = cap_area * height;
    let expected_surface_area = 2.0 * cap_area + lateral_area;
    let expected_inner_radius = radius * half_angle.cos();

    ensure_small!((hull.volume - expected_volume) / expected_volume, 1e-4);
    ensure_small!((hull.surface_area - expected_surface_area) / expected_surface_area, 1e-4);
    ensure_small!(hull.inner_radius - expected_inner_radius, 1e-5);

    ensure_small!(hull.center.x, 1e-5);
    ensure_small!(hull.center.y - (y_offset + 0.5 * height), 1e-5);
    ensure_small!(hull.center.z, 1e-5);

    ensure_small!(hull.aabb.lower_bound.x + radius, f32::EPSILON);
    ensure_small!(hull.aabb.lower_bound.y - y_offset, f32::EPSILON);
    ensure_small!(hull.aabb.lower_bound.z + radius, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.x - radius, f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.y - (y_offset + height), f32::EPSILON);
    ensure_small!(hull.aabb.upper_bound.z - radius, f32::EPSILON);
}

// Inlined XorShift32 + Shoemake unit-vector recipe matches shared/utils.h exactly so
// generated points are bit-identical to samples that share the seed.
fn fill_sphere_sample(points: &mut [Vec3], seed: u32) {
    const RAND_LIMIT_LOCAL: u32 = 32767;
    let mut seed = seed;
    for point in points.iter_mut() {
        let mut u = [0.0f32; 3];
        for k in 0..3 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let mut r = (seed & RAND_LIMIT_LOCAL) as f32;
            r /= RAND_LIMIT_LOCAL as f32;
            u[k] = r;
        }
        let u1 = u[0];
        let u2 = 2.0 * PI * u[1];
        let u3 = 2.0 * PI * u[2];
        let sqrt1_minus_u1 = (1.0 - u1).sqrt();
        let sqrt_u1 = u1.sqrt();
        point.x = sqrt1_minus_u1 * u2.sin();
        point.y = sqrt1_minus_u1 * u2.cos();
        point.z = sqrt_u1 * u3.sin();
    }
}

// Reproduces the HullReduction sample (Sphere, 64 points, count=20) that used to assert on
// b->faceCount < b->faceCapacity.
#[test]
fn create_hull_sphere_reduction_test() {
    let mut points = [Vec3::ZERO; 64];
    fill_sphere_sample(&mut points, 12345); // RAND_SEED

    let hull = create_hull(&points, 20);
    ensure!(hull.is_some());
    let hull = hull.unwrap();
    ensure!(hull.vertex_count() >= 4 && hull.vertex_count() <= 20);
    ensure!(hull.vertex_count() - hull.edge_count() / 2 + hull.face_count() == 2);
}

// XorShift32 + uniform-cube point generator. Same engine as fill_sphere_sample.
fn fill_cube_sample(points: &mut [Vec3], seed: u32) {
    const RAND_LIMIT_LOCAL: u32 = 32767;
    let mut seed = seed;
    for point in points.iter_mut() {
        let mut v = [0.0f32; 3];
        for k in 0..3 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let mut r = (seed & RAND_LIMIT_LOCAL) as f32;
            r /= RAND_LIMIT_LOCAL as f32;
            v[k] = 2.0 * r - 1.0;
        }
        point.x = v[0];
        point.y = v[1];
        point.z = v[2];
    }
}

// Pushes the working-stage bump capacities close to their peak by sweeping M
// on a dense random sphere.
#[test]
fn create_hull_sphere_stress_test() {
    const N: usize = 512;
    let mut points = [Vec3::ZERO; N];

    // Multiple seeds exercise different horizon-size / merge-cascade sequences.
    let seeds: [u32; 4] = [12345, 1, 0xdeadbeef, 0xcafef00d];

    // M kept <= 40: random sphere inputs at higher M exceed the half edge limit of u8::MAX
    let m_values = [16, 24, 32, 40];

    for &seed in &seeds {
        fill_sphere_sample(&mut points, seed);

        for &m in &m_values {
            let hull = create_hull(&points, m);
            ensure!(hull.is_some());
            let hull = hull.unwrap();
            ensure!(hull.vertex_count() >= 4 && hull.vertex_count() <= m);
            ensure!(hull.vertex_count() - hull.edge_count() / 2 + hull.face_count() == 2);
            ensure!(hull.face_count() >= 4);
        }
    }
}

// Random points inside a cube produce a small final hull (8 corners, 6 faces) but
// generate heavy internal churn.
#[test]
fn create_hull_merge_churn_stress_test() {
    const N: usize = 4096;
    let mut points = vec![Vec3::ZERO; N];

    let seeds: [u32; 2] = [12345, 0xdeadbeef];

    for &seed in &seeds {
        fill_cube_sample(&mut points, seed);

        // Stamp the 8 corners last so they're guaranteed extremes.
        for c in 0..8usize {
            points[N - 8 + c].x = if c & 1 != 0 { 1.0 } else { -1.0 };
            points[N - 8 + c].y = if c & 2 != 0 { 1.0 } else { -1.0 };
            points[N - 8 + c].z = if c & 4 != 0 { 1.0 } else { -1.0 };
        }

        let hull = create_hull(&points, 64);
        ensure!(hull.is_some());
        let hull = hull.unwrap();
        ensure!(hull.vertex_count() == 8);
        ensure!(hull.edge_count() == 24);
        ensure!(hull.face_count() == 6);
    }
}

#[test]
fn create_hull_degenerate_test() {
    // Real (non-null) buffer; pointCount < 4 cases are guarded inside Construct().
    let collinear = [
        Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        Vec3 { x: 1.0, y: 0.0, z: 0.0 },
        Vec3 { x: 2.0, y: 0.0, z: 0.0 },
        Vec3 { x: 3.0, y: 0.0, z: 0.0 },
        Vec3 { x: 4.0, y: 0.0, z: 0.0 },
        Vec3 { x: 5.0, y: 0.0, z: 0.0 },
        Vec3 { x: 6.0, y: 0.0, z: 0.0 },
        Vec3 { x: 7.0, y: 0.0, z: 0.0 },
    ];

    // Empty input.
    ensure!(create_hull(&collinear[..0], 8).is_none());

    // Fewer than 4 points.
    ensure!(create_hull(&collinear[..3], 8).is_none());

    // 8 coincident points.
    let coincident = [Vec3 { x: 1.0, y: 2.0, z: 3.0 }; 8];
    ensure!(create_hull(&coincident, 8).is_none());

    // Collinear (along x-axis).
    ensure!(create_hull(&collinear, 8).is_none());

    // Coplanar (in the xy-plane).
    let coplanar = [
        Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        Vec3 { x: 1.0, y: 0.0, z: 0.0 },
        Vec3 { x: 0.0, y: 1.0, z: 0.0 },
        Vec3 { x: 1.0, y: 1.0, z: 0.0 },
        Vec3 { x: 2.0, y: 0.5, z: 0.0 },
        Vec3 { x: 0.5, y: 2.0, z: 0.0 },
    ];
    ensure!(create_hull(&coplanar, 8).is_none());
}
