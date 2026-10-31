// A fixed box3d scene (shared by the rwbench `box3d_physics` workload and the diff_fast package program
// tools/run/fixture/diffpkg, which includes this file by path): a hull-box pyramid on a hull ground
// (stacking, many persistent contacts), a revolute-jointed plank bridge with a motorised end and limits,
// a spherical-jointed sphere rope ending in a capsule, a distance-joint pendulum, and a rain of spheres,
// capsules and rocks onto the pyramid (new contacts every step, continuous collision on). Serial solver
// (worker_count 0), sleeping off, so every step does the same work. Neutral content only.
use makepad_box3d::body::*;
use makepad_box3d::hull::{create_cylinder, create_rock, make_box_hull};
use makepad_box3d::id::BodyId;
use makepad_box3d::joint::*;
use makepad_box3d::math_functions::{make_quat_from_axis_angle, pos, vec3, Pos, Quat, Vec3};
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::{create_capsule_shape, create_hull_shape, create_sphere_shape};
use makepad_box3d::types::*;

pub struct Scene {
    pub world: World,
    pub bodies: Vec<BodyId>,
}

/// `scale` grows the pyramid base and the rain (1 = the diff program's small scene).
pub fn build(scale: i32) -> Scene {
    let mut world_def = default_world_def();
    world_def.enable_sleep = false;
    world_def.enable_continuous = true;
    let mut world = create_world(&world_def);
    world_enable_sleeping(&mut world, false);
    let mut bodies = Vec::new();

    // ground: one static hull slab
    {
        let mut bd = default_body_def();
        bd.position = pos(0.0, -1.0, 0.0);
        let g = create_body(&mut world, &bd);
        create_hull_shape(&mut world, g, &default_shape_def(), &make_box_hull(60.0, 1.0, 60.0));
    }

    // pyramid of hull boxes
    let base = 6 + 2 * scale;
    let h = 0.5f32;
    let cube = make_box_hull(h, h, h);
    let mut sd = default_shape_def();
    sd.density = 50.0;
    sd.base_material.friction = 0.6;
    for i in 0..base {
        for j in i..base {
            let mut bd = default_body_def();
            bd.body_type = BodyType::Dynamic;
            let x = (i as f32 + 1.0) * h + 2.0 * (j - i) as f32 * h - h * base as f32;
            bd.position = pos(x, (2.0 * i as f32 + 1.0) * h, 0.0);
            let b = create_body(&mut world, &bd);
            create_hull_shape(&mut world, b, &sd, &cube);
            bodies.push(b);
        }
    }

    // plank bridge: statics at both ends, revolute hinges about z, the first hinge motorised, all limited
    {
        let n = 12;
        let plank = make_box_hull(0.5, 0.1, 0.8);
        let z = -6.0f32;
        let mut prev = {
            let mut bd = default_body_def();
            bd.position = pos(-7.0, 4.0, z);
            create_body(&mut world, &bd)
        };
        let mut sd = default_shape_def();
        sd.density = 20.0;
        for k in 0..=n {
            let next = if k < n {
                let mut bd = default_body_def();
                bd.body_type = BodyType::Dynamic;
                bd.position = pos(-6.5 + k as f32, 4.0, z);
                let b = create_body(&mut world, &bd);
                create_hull_shape(&mut world, b, &sd, &plank);
                bodies.push(b);
                b
            } else {
                let mut bd = default_body_def();
                bd.position = pos(-6.5 + n as f32, 4.0, z);
                create_body(&mut world, &bd)
            };
            let mut jd = default_revolute_joint_def();
            jd.base.body_id_a = prev;
            jd.base.body_id_b = next;
            jd.base.local_frame_a.p = if k == 0 { vec3(0.0, 0.0, 0.0) } else { vec3(0.5, 0.0, 0.0) };
            jd.base.local_frame_b.p = if k < n { vec3(-0.5, 0.0, 0.0) } else { vec3(0.0, 0.0, 0.0) };
            jd.enable_limit = true;
            jd.lower_angle = -0.4;
            jd.upper_angle = 0.4;
            if k == 0 {
                jd.enable_motor = true;
                jd.max_motor_torque = 40.0;
                jd.motor_speed = 0.3;
            }
            create_revolute_joint(&mut world, &jd);
            prev = next;
        }
    }

    // sphere rope on spherical joints, ending in a capsule, released sideways
    {
        let mut prev = {
            let mut bd = default_body_def();
            bd.position = pos(6.0, 9.0, 4.0);
            create_body(&mut world, &bd)
        };
        let mut sd = default_shape_def();
        sd.density = 10.0;
        let links = 10;
        for k in 0..links {
            let mut bd = default_body_def();
            bd.body_type = BodyType::Dynamic;
            bd.position = pos(6.0 + 0.5 * (k + 1) as f32, 9.0, 4.0);
            let b = create_body(&mut world, &bd);
            if k + 1 < links {
                create_sphere_shape(&mut world, b, &sd, &Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.2 });
            } else {
                let cap = Capsule { center1: vec3(-0.3, 0.0, 0.0), center2: vec3(0.3, 0.0, 0.0), radius: 0.25 };
                create_capsule_shape(&mut world, b, &sd, &cap);
            }
            let mut jd = default_spherical_joint_def();
            jd.base.body_id_a = prev;
            jd.base.body_id_b = b;
            jd.base.local_frame_a.p = if k == 0 { vec3(0.0, 0.0, 0.0) } else { vec3(0.25, 0.0, 0.0) };
            jd.base.local_frame_b.p = vec3(-0.25, 0.0, 0.0);
            create_spherical_joint(&mut world, &jd);
            bodies.push(b);
            prev = b;
        }
    }

    // distance-joint pendulum with a cylinder bob and a spring
    {
        let mut bd = default_body_def();
        bd.position = pos(-6.0, 8.0, 5.0);
        let anchor = create_body(&mut world, &bd);
        let mut bd = default_body_def();
        bd.body_type = BodyType::Dynamic;
        bd.position = pos(-3.5, 7.0, 5.0);
        bd.angular_velocity = vec3(0.0, 1.5, 0.0);
        let bob = create_body(&mut world, &bd);
        create_hull_shape(&mut world, bob, &default_shape_def(), &create_cylinder(0.6, 0.4, 0.0, 12));
        let mut jd = default_distance_joint_def();
        jd.base.body_id_a = anchor;
        jd.base.body_id_b = bob;
        jd.length = 2.5;
        jd.enable_spring = true;
        jd.hertz = 2.0;
        jd.damping_ratio = 0.3;
        jd.enable_limit = true;
        jd.min_length = 1.5;
        jd.max_length = 3.5;
        create_distance_joint(&mut world, &jd);
        bodies.push(bob);
    }

    // rain onto the pyramid: spheres, capsules and rocks with spin, a fixed lattice (no RNG)
    {
        let rock = create_rock(0.35);
        let count = 12 * scale;
        for k in 0..count {
            let mut bd = default_body_def();
            bd.body_type = BodyType::Dynamic;
            let fx = (k % 6) as f32 - 2.5;
            let fz = ((k / 6) % 3) as f32 - 1.0;
            bd.position = pos(fx * 0.9, base as f32 + 2.0 + (k / 18) as f32 * 1.3, fz * 0.7);
            bd.rotation = make_quat_from_axis_angle(vec3(0.6, 0.0, 0.8), 0.37 * k as f32);
            bd.linear_velocity = vec3(0.3 * fz, -1.0, -0.2 * fx);
            bd.angular_velocity = vec3(0.5, 0.25 * fx, 0.0);
            let b = create_body(&mut world, &bd);
            let mut sd = default_shape_def();
            sd.base_material.restitution = 0.2;
            sd.base_material.friction = 0.5;
            match k % 3 {
                0 => {
                    create_sphere_shape(&mut world, b, &sd, &Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.3 });
                }
                1 => {
                    let cap = Capsule { center1: vec3(0.0, -0.25, 0.0), center2: vec3(0.0, 0.25, 0.0), radius: 0.2 };
                    create_capsule_shape(&mut world, b, &sd, &cap);
                }
                _ => {
                    create_hull_shape(&mut world, b, &sd, &rock);
                }
            }
            bodies.push(b);
        }
    }
    Scene { world, bodies }
}

pub fn step(scene: &mut Scene) {
    world_step(&mut scene.world, 1.0 / 60.0, 4);
}

pub fn fold(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn fold_pos(h: u64, p: Pos) -> u64 {
    fold(fold(fold(h, p.x.to_bits() as u64), p.y.to_bits() as u64), p.z.to_bits() as u64)
}

fn fold_vec(h: u64, v: Vec3) -> u64 {
    fold(fold(fold(h, v.x.to_bits() as u64), v.y.to_bits() as u64), v.z.to_bits() as u64)
}

fn fold_quat(h: u64, q: Quat) -> u64 {
    fold(fold_vec(h, q.v), q.s.to_bits() as u64)
}

/// Bit hash of one body's transform and velocities.
pub fn body_hash(scene: &Scene, i: usize) -> u64 {
    let w = &scene.world;
    let b = scene.bodies[i];
    let t = body_get_transform(w, b);
    let h = fold_quat(fold_pos(0, t.p), t.q);
    fold_vec(fold_vec(h, body_get_linear_velocity(w, b)), body_get_angular_velocity(w, b))
}

/// Bit hash of every dynamic body's state and the contact count.
pub fn state_hash(scene: &Scene) -> u64 {
    let mut h = 0u64;
    for i in 0..scene.bodies.len() {
        h = fold(h, body_hash(scene, i));
    }
    fold(h, contact_count(scene) as u64)
}

pub fn contact_count(scene: &Scene) -> i32 {
    world_get_counters(&scene.world).contact_count
}
