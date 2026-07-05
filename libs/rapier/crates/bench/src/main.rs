// Rapier counterpart of libs/box3d/examples/benchmark.rs, covering all nine
// scenes single-threaded. Scene geometry, material values, timestep and the
// timing protocol mirror the box3d benchmark exactly:
//
// - dt = 1/60, gravity (0, -10, 0), single threaded (no `parallel` feature)
// - rapier IntegrationParameters::default() with num_solver_iterations = 4
//   set explicitly (rapier's small-steps solver iterations vs box3d's 4
//   sub-steps — the closest semantic match, both TGS-soft style)
// - friction 0.6 (box3d default) unless the box3d scene overrides it,
//   restitution 0, densities as in the box3d scenes (default 1000)
// - sleeping enabled/disabled exactly where box3d does
// - timing: create world, one untimed step, time the remaining N-1 steps,
//   min over runs (box3d harness protocol)
//
// Known substitutions (see the comparison notes in the box3d README):
// - box3d rolling_resistance (trees 0.05, human capsules 0.2) has no rapier
//   equivalent — omitted.
// - box3d joint springs (hertz/damping toward the reference rotation) are
//   not mapped; joint motors (friction torque, target velocity 0) are.
// - box3d cone+twist spherical limits map to per-axis angular limits
//   (twist about the joint frame Z → ANG_X after a fixed basis change,
//   swing → ±cone angle on the other two axes).
// - Scene DRIVING math (kinematic targets, spawn positions) uses plain f32
//   trig instead of box3d's deterministic cos/sin — identical to a few ulp.
// - box3d negative filter group indices are emulated with 31 category bits
//   (rain groups alias mod 31, but aliased groups are spatially disjoint).
use rapier3d::prelude::*;
use std::time::Instant;

const PI: f32 = std::f32::consts::PI;
const DEG_TO_RAD: f32 = PI / 180.0;

struct World {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    ccd: CCDSolver,
    pipeline: PhysicsPipeline,
    params: IntegrationParameters,
}

impl World {
    fn new() -> World {
        let mut params = IntegrationParameters::default();
        params.dt = 1.0 / 60.0;
        // Explicit even though it is the default — pinned for the comparison.
        params.num_solver_iterations = 4;
        World {
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            ccd: CCDSolver::new(),
            pipeline: PhysicsPipeline::new(),
            params,
        }
    }

    fn step(&mut self) {
        self.pipeline.step(
            Vector::new(0.0, -10.0, 0.0),
            &self.params,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd,
            &(),
            &(),
        );
    }
}

fn quat(x: f32, y: f32, z: f32, s: f32) -> Rotation {
    Rotation::from_xyzw(x, y, z, s).normalize()
}

// ---------------------------------------------------------------------------
// Scene trait (box3d benchmark Scenario mirror)
// ---------------------------------------------------------------------------

trait Scene {
    fn create(&mut self, w: &mut World);
    fn step_scene(&mut self, _w: &mut World, _step_index: i32) {}
}

// ---------------------------------------------------------------------------
// large_pyramid / many_pyramids / joint_grid (unchanged from the 3-scene bench)
// ---------------------------------------------------------------------------

struct LargePyramid;
impl Scene for LargePyramid {
    // box3d LargePyramid: ground (400,1,400) at y=-1; 90-base pyramid of
    // half-extent 0.5 boxes, density 100, sleeping disabled world-wide.
    fn create(&mut self, w: &mut World) {
        let ground = w.bodies.insert(RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)));
        w.colliders.insert_with_parent(
            ColliderBuilder::cuboid(400.0, 1.0, 400.0).friction(0.6).restitution(0.0),
            ground,
            &mut w.bodies,
        );

        let base_count: i32 = 90;
        let h = 0.5f32;
        let shift = 1.0 * h;

        for i in 0..base_count {
            let y = (2.0 * i as f32 + 1.0) * shift;
            for j in i..base_count {
                let x = (i as f32 + 1.0) * shift + 2.0 * (j - i) as f32 * shift - h * base_count as f32;
                let b = w
                    .bodies
                    .insert(RigidBodyBuilder::dynamic().translation(Vector::new(x, y, 0.0)).can_sleep(false));
                w.colliders.insert_with_parent(
                    ColliderBuilder::cuboid(h, h, h).friction(0.6).restitution(0.0).density(100.0),
                    b,
                    &mut w.bodies,
                );
            }
        }
    }
}

fn create_small_pyramid(w: &mut World, base_count: i32, extent: f32, center_x: f32, base_z: f32) {
    for i in 0..base_count {
        let y = (2.0 * i as f32 + 1.0) * extent;
        for j in i..base_count {
            let x = (i as f32 + 1.0) * extent + 2.0 * (j - i) as f32 * extent + center_x - 0.5;
            let b = w
                .bodies
                .insert(RigidBodyBuilder::dynamic().translation(Vector::new(x, y, base_z)).can_sleep(false));
            w.colliders.insert_with_parent(
                ColliderBuilder::cuboid(extent, extent, extent).friction(0.6).restitution(0.0).density(100.0),
                b,
                &mut w.bodies,
            );
        }
    }
}

struct ManyPyramids;
impl Scene for ManyPyramids {
    fn create(&mut self, w: &mut World) {
        let base_count = 10;
        let extent = 0.5f32;
        let row_count: i32 = 14;
        let column_count: i32 = 14;
        let ground_extent = extent * column_count as f32 * (base_count as f32 + 1.0);

        let ground = w.bodies.insert(RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)));
        w.colliders.insert_with_parent(
            ColliderBuilder::cuboid(ground_extent, 1.0, ground_extent).friction(0.6).restitution(0.0),
            ground,
            &mut w.bodies,
        );

        let base_width = 2.0 * extent * base_count as f32;
        let mut base_z = -ground_extent + 2.0 * extent;
        let delta_z = 2.0 * (ground_extent - 2.0 * extent) / (row_count as f32 - 1.0);

        for _i in 0..row_count {
            for j in 0..column_count {
                let center_x = -ground_extent + j as f32 * (base_width + 2.0 * extent) + 2.0 * extent;
                create_small_pyramid(w, base_count, extent, center_x, base_z);
            }
            base_z += delta_z;
        }
    }
}

struct JointGrid;
impl Scene for JointGrid {
    // box3d JointGrid: 100x100 spheres (radius 0.4, density 1000), column
    // i==0 static, spherical joints along columns and rows. Spheres carry
    // category 2 / mask !2 so they never collide with each other.
    fn create(&mut self, w: &mut World) {
        let n: i32 = 100;
        let groups = InteractionGroups::new(
            Group::from_bits_retain(2),
            Group::from_bits_retain(!2u32),
            InteractionTestMode::And,
        );

        let mut handles: Vec<RigidBodyHandle> = Vec::with_capacity((n * n) as usize);
        let mut index = 0usize;

        for k in 0..n {
            for i in 0..n {
                let builder = if i == 0 { RigidBodyBuilder::fixed() } else { RigidBodyBuilder::dynamic() };
                let body = w
                    .bodies
                    .insert(builder.translation(Vector::new(k as f32, -(i as f32), 0.0)).can_sleep(false));
                w.colliders.insert_with_parent(
                    ColliderBuilder::ball(0.4)
                        .friction(0.6)
                        .restitution(0.0)
                        .density(1000.0)
                        .collision_groups(groups),
                    body,
                    &mut w.bodies,
                );

                if i > 0 {
                    let joint = SphericalJointBuilder::new()
                        .local_anchor1(Vector::new(0.0, -0.5, 0.0))
                        .local_anchor2(Vector::new(0.0, 0.5, 0.0));
                    w.impulse_joints.insert(handles[index - 1], body, joint, true);
                }
                if k > 0 {
                    let joint = SphericalJointBuilder::new()
                        .local_anchor1(Vector::new(0.5, 0.0, 0.0))
                        .local_anchor2(Vector::new(-0.5, 0.0, 0.0));
                    w.impulse_joints.insert(handles[index - n as usize], body, joint, true);
                }

                handles.push(body);
                index += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared geometry builders (formulas copied from box3d mesh.rs / hull.rs)
// ---------------------------------------------------------------------------

// box3d create_wave_mesh
fn wave_mesh(x_count: i32, z_count: i32, cell_width: f32, amplitude: f32, row_hz: f32, col_hz: f32) -> (Vec<Vector>, Vec<[u32; 3]>) {
    let vertex_count = (x_count + 1) * (z_count + 1);
    let mut vertices = Vec::with_capacity(vertex_count as usize);

    let x_width = cell_width * x_count as f32;
    let z_width = cell_width * z_count as f32;
    let omega_z = 2.0 * PI * row_hz * cell_width;
    let omega_x = 2.0 * PI * col_hz * cell_width;

    let mut x = -0.5 * x_width;
    for ix in 0..=x_count {
        let row_height = (omega_x * ix as f32).sin();
        let mut z = -0.5 * z_width;
        for iz in 0..=z_count {
            let column_height = (omega_z * iz as f32).sin();
            vertices.push(Vector::new(x, amplitude * row_height * column_height, z));
            z += cell_width;
        }
        x += cell_width;
    }

    let mut indices: Vec<[u32; 3]> = Vec::with_capacity((2 * x_count * z_count) as usize);
    for ix in 0..x_count {
        for iz in 0..z_count {
            let i1 = (iz + (z_count + 1) * ix) as u32;
            let i2 = i1 + 1;
            let i3 = i2 + (z_count + 1) as u32;
            let i4 = i3 - 1;
            indices.push([i1, i2, i3]);
            indices.push([i3, i4, i1]);
        }
    }
    (vertices, indices)
}

// box3d create_grid_mesh (flat wave)
fn grid_mesh(x_count: i32, z_count: i32, cell_width: f32) -> (Vec<Vector>, Vec<[u32; 3]>) {
    wave_mesh(x_count, z_count, cell_width, 0.0, 0.0, 0.0)
}

// box3d create_torus_mesh
fn torus_mesh(radial: i32, tubular: i32, radius: f32, thickness: f32) -> (Vec<Vector>, Vec<[u32; 3]>) {
    let mut vertices = Vec::new();
    for ri in 0..radial {
        for ti in 0..tubular {
            let u = ti as f32 / tubular as f32 * 2.0 * PI;
            let v = ri as f32 / radial as f32 * 2.0 * PI;
            let x = (radius + thickness * v.cos()) * u.cos();
            let y = (radius + thickness * v.cos()) * u.sin();
            let z = thickness * v.sin();
            vertices.push(Vector::new(x, y, z));
        }
    }
    let mut indices = Vec::new();
    for r1 in 0..radial {
        let r2 = (r1 + 1) % radial;
        for t1 in 0..tubular {
            let t2 = (t1 + 1) % tubular;
            let i1 = (r1 * tubular + t1) as u32;
            let i2 = (r1 * tubular + t2) as u32;
            let i3 = (r2 * tubular + t2) as u32;
            let i4 = (r2 * tubular + t1) as u32;
            indices.push([i1, i2, i3]);
            indices.push([i3, i4, i1]);
        }
    }
    (vertices, indices)
}

// box3d create_cylinder (hull point set)
fn cylinder_points(height: f32, radius: f32, y_offset: f32, sides: i32) -> Vec<Vector> {
    let mut points = Vec::with_capacity((2 * sides) as usize);
    let mut alpha = 0.0f32;
    let delta = 2.0 * PI / sides as f32;
    for _ in 0..sides {
        let (s, c) = alpha.sin_cos();
        points.push(Vector::new(radius * c, y_offset, radius * s));
        points.push(Vector::new(radius * c, y_offset + height, radius * s));
        alpha += delta;
    }
    points
}

// box3d create_rock: 10-point Fibonacci lattice on the sphere, golden-ratio
// azimuth steps accumulated with the same cos/sin recurrence.
fn rock_points(radius: f32) -> Vec<Vector> {
    let point_count = 10;
    let phi = (1.0 + 5.0f32.sqrt()) / 2.0;
    let theta = 2.0 * PI / phi;
    let (d_sin, d_cos) = theta.sin_cos();
    let (mut cs_c, mut cs_s) = (1.0f32, 0.0f32);
    let mut points = Vec::with_capacity(point_count);
    for i in 0..point_count {
        let z = 1.0 - (2.0 * i as f32 + 1.0) / point_count as f32;
        let rxy = (1.0 - z * z).sqrt();
        points.push(Vector::new(radius * rxy * cs_c, radius * rxy * cs_s, radius * z));
        let (c0, s0) = (cs_c, cs_s);
        cs_c = d_cos * c0 - d_sin * s0;
        cs_s = d_sin * c0 + d_cos * s0;
    }
    points
}

// ---------------------------------------------------------------------------
// trees (Trees::new(scale)) — wave-mesh ground + 50 log stacks of 22 cylinder
// hulls each, friction 0.9, density 1.0, initial spin.
// ---------------------------------------------------------------------------

struct Trees {
    scale: i32,
}

impl Scene for Trees {
    fn create(&mut self, w: &mut World) {
        let scale = self.scale;
        let ground = w.bodies.insert(RigidBodyBuilder::fixed());
        let (verts, idx) = wave_mesh(scale * 150, scale * 200, 1.0 / scale as f32, 0.4, 0.05, 0.1);
        w.colliders.insert_with_parent(
            ColliderBuilder::trimesh_with_flags(verts, idx, TriMeshFlags::FIX_INTERNAL_EDGES)
                .expect("trees mesh")
                .friction(0.6)
                .restitution(0.0),
            ground,
            &mut w.bodies,
        );

        let body_count: i32 = 50;
        let hull_count = 22;
        let mut hulls: Vec<Vec<Vector>> = Vec::with_capacity(hull_count);
        let mut y = 1.0f32;
        let mut r = 0.75f32;
        let l = 1.5f32;
        for _ in 0..hull_count {
            hulls.push(cylinder_points(l + 2.0 * r, r, y - r, 6));
            y += l + 2.0 * r;
            r = 0.95 * r;
        }

        let mut angular_velocity = -0.5f32;
        let mut z: f32 = -70.0;
        for body_index in 0..body_count {
            let position = Vector::new(0.0, 1.0, z);
            let body = w.bodies.insert(RigidBodyBuilder::dynamic().translation(position));
            for h in &hulls {
                w.colliders.insert_with_parent(
                    ColliderBuilder::convex_hull(h).expect("tree hull").friction(0.9).restitution(0.0).density(1.0),
                    body,
                    &mut w.bodies,
                );
            }

            let velocity_scale = 0.5 + (0.5 * body_index as f32) / body_count as f32;
            let body_ref = &mut w.bodies[body];
            let center = body_ref.center_of_mass();
            let omega = Vector::new(0.0, 0.0, velocity_scale * angular_velocity);
            let v = omega.cross(center - position);
            body_ref.set_angvel(omega, true);
            body_ref.set_linvel(v, true);

            z += 3.0;
            angular_velocity = -angular_velocity;
        }
    }
}

// ---------------------------------------------------------------------------
// junkyard — ground + walls, 24x21x21 rock hulls, kinematic cylinder pusher
// orbiting at radius 35 driven by a target transform each step.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Junkyard {
    pusher: Option<RigidBodyHandle>,
    degrees: f32,
    radius: f32,
}

impl Scene for Junkyard {
    fn create(&mut self, w: &mut World) {
        let ground = w.bodies.insert(RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)));
        w.colliders.insert_with_parent(
            ColliderBuilder::cuboid(120.0, 1.0, 120.0).friction(0.6).restitution(0.0),
            ground,
            &mut w.bodies,
        );
        for (hx, hy, hz, ox, oy, oz) in [
            (1.0, 8.0, 50.0, -50.0, 8.0, 0.0),
            (1.0, 8.0, 50.0, 50.0, 8.0, 0.0),
            (50.0, 8.0, 1.0, 0.0, 8.0, -50.0),
            (50.0, 8.0, 1.0, 0.0, 8.0, 50.0),
        ] {
            w.colliders.insert_with_parent(
                ColliderBuilder::cuboid(hx, hy, hz)
                    .position(Pose::from_parts(Vector::new(ox, oy, oz), Rotation::IDENTITY))
                    .friction(0.6)
                    .restitution(0.0),
                ground,
                &mut w.bodies,
            );
        }

        let rock = rock_points(1.5);
        let count: i32 = 24;
        let height = 24.0f32;
        for y in 0..count {
            for x in 0..=20 {
                for z in 0..=20 {
                    let px = -40.0 + 4.0 * x as f32;
                    let py = 4.0 * y as f32 + height + 1.0;
                    let pz = -40.0 + 4.0 * z as f32;
                    let b = w.bodies.insert(RigidBodyBuilder::dynamic().translation(Vector::new(px, py, pz)));
                    w.colliders.insert_with_parent(
                        ColliderBuilder::convex_hull(&rock)
                            .expect("rock hull")
                            .friction(0.6)
                            .restitution(0.0)
                            .density(1000.0),
                        b,
                        &mut w.bodies,
                    );
                }
            }
        }

        self.radius = 35.0;
        self.degrees = 0.0;
        let pusher = w
            .bodies
            .insert(RigidBodyBuilder::kinematic_position_based().translation(Vector::new(self.radius, 0.0, 0.0)));
        let cyl = cylinder_points(24.0, 4.0, 0.0, 16);
        w.colliders.insert_with_parent(
            ColliderBuilder::convex_hull(&cyl).expect("pusher hull").friction(0.6).restitution(0.0).density(1000.0),
            pusher,
            &mut w.bodies,
        );
        self.pusher = Some(pusher);
    }

    fn step_scene(&mut self, w: &mut World, _step_index: i32) {
        let time_step = 1.0 / 60.0;
        let omega = -6.0f32;
        self.degrees += omega * time_step;
        let (s, c) = (self.degrees * PI / 180.0).sin_cos();
        let target = Vector::new(self.radius * c, 0.0, self.radius * s);
        let body = &mut w.bodies[self.pusher.unwrap()];
        body.set_next_kinematic_position(Pose::from_parts(target, Rotation::IDENTITY));
    }
}

// ---------------------------------------------------------------------------
// washer — velocity-kinematic drum (36 blade hulls + 4 paddles) + 8000 cubes.
// ---------------------------------------------------------------------------

struct Washer;
impl Scene for Washer {
    fn create(&mut self, w: &mut World) {
        let ground = w.bodies.insert(RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)));
        w.colliders.insert_with_parent(
            ColliderBuilder::cuboid(60.0, 1.0, 60.0).friction(0.6).restitution(0.0),
            ground,
            &mut w.bodies,
        );

        let motor_speed = 25.0f32;
        let drum = w.bodies.insert(
            RigidBodyBuilder::kinematic_velocity_based()
                .translation(Vector::new(0.0, 21.0, 0.0))
                .angvel(Vector::new(0.0, 0.0, (PI / 180.0) * motor_speed))
                .linvel(Vector::new(0.001, -0.002, 0.0)),
        );

        let r0 = 14.0f32;
        let r1 = 16.0f32;
        let r2 = 18.0f32;
        let nd = Vector::new(0.0, 0.0, -10.0);
        let pd = Vector::new(0.0, 0.0, 10.0);
        let angle = PI / 18.0;
        let q = Rotation::from_axis_angle(Vector::Z, angle);
        let qo = Rotation::from_axis_angle(Vector::Z, 0.1 * angle);
        let mut u1 = Vector::new(1.0, 0.0, 0.0);
        for i in 0..36 {
            let u2 = if i == 35 { Vector::new(1.0, 0.0, 0.0) } else { q * u1 };
            {
                let a1 = qo.inverse() * u1;
                let a2 = qo * u2;
                // box3d mul_add(a, s, b) = a + s*b
                let pts = [
                    nd + r1 * a1,
                    nd + r2 * a1,
                    nd + r1 * a2,
                    nd + r2 * a2,
                    pd + r1 * a1,
                    pd + r2 * a1,
                    pd + r1 * a2,
                    pd + r2 * a2,
                ];
                w.colliders.insert_with_parent(
                    ColliderBuilder::convex_hull(&pts)
                        .expect("blade hull")
                        .friction(0.6)
                        .restitution(0.0)
                        .density(1000.0),
                    drum,
                    &mut w.bodies,
                );
            }
            if i % 9 == 0 {
                let pts = [
                    nd + r0 * u1,
                    nd + r1 * u1,
                    nd + r0 * u2,
                    nd + r1 * u2,
                    pd + r0 * u1,
                    pd + r1 * u1,
                    pd + r0 * u2,
                    pd + r1 * u2,
                ];
                w.colliders.insert_with_parent(
                    ColliderBuilder::convex_hull(&pts)
                        .expect("paddle hull")
                        .friction(0.6)
                        .restitution(0.0)
                        .density(1000.0),
                    drum,
                    &mut w.bodies,
                );
            }
            u1 = u2;
        }

        let grid_count: i32 = 20;
        let a = 0.2f32;
        let mut x = -2.0 * a * grid_count as f32;
        for _i in 0..grid_count {
            let mut y = -2.0 * a * grid_count as f32 + 21.0;
            for _j in 0..grid_count {
                let mut z = -2.0 * a * grid_count as f32;
                for _k in 0..grid_count {
                    let b = w.bodies.insert(RigidBodyBuilder::dynamic().translation(Vector::new(x, y, z)));
                    w.colliders.insert_with_parent(
                        ColliderBuilder::cuboid(a, a, a).friction(0.6).restitution(0.0).density(1000.0),
                        b,
                        &mut w.bodies,
                    );
                    z += 4.0 * a;
                }
                y += 4.0 * a;
            }
            x += 4.0 * a;
        }
    }
}

// ---------------------------------------------------------------------------
// rain — 10x10 mesh cells (flat grid + torus each), ragdoll humans spawned in
// column waves every 48 steps (human.c port, 14 capsules + 13 joints + one
// collision-filter joint per human).
// ---------------------------------------------------------------------------

const RAIN_GRID_SIZE: f32 = 15.0;
const RAIN_GRID_COUNT: usize = 10;
const RAIN_GROUP_SIZE: usize = 3;

#[derive(Clone, Copy)]
enum BoneJoint {
    None,
    Revolute,
    Spherical,
}

struct BoneDef {
    parent: i32,
    ref_p: [f32; 3],
    ref_q: [f32; 4],
    joint: BoneJoint,
    fa_p: [f32; 3],
    fa_q: [f32; 4],
    fb_p: [f32; 3],
    fb_q: [f32; 4],
    swing_deg: f32,
    twist_deg: [f32; 2],
    joint_friction: f32,
    cap_a: [f32; 3],
    cap_b: [f32; 3],
    cap_r: f32,
    neg_group: bool,
}

// Transcribed 1:1 from box3d's create_human (examples/benchmark.rs, itself a
// port of box3d human.c).
#[rustfmt::skip]
const BONES: [BoneDef; 14] = [
    BoneDef { parent: -1, ref_p: [0.0, 0.932087, -0.051708], ref_q: [0.739169, 0.0, 0.0, 0.673520], joint: BoneJoint::None, fa_p: [0.0; 3], fa_q: [0.0, 0.0, 0.0, 1.0], fb_p: [0.0; 3], fb_q: [0.0, 0.0, 0.0, 1.0], swing_deg: 0.0, twist_deg: [0.0, 0.0], joint_friction: 1.0, cap_a: [0.07, 0.0, -0.08], cap_b: [-0.07, 0.0, -0.08], cap_r: 0.13, neg_group: false },
    BoneDef { parent: 0, ref_p: [0.0, 1.113505, -0.03481], ref_q: [0.739973, 0.0, 0.0, 0.672637], joint: BoneJoint::Spherical, fa_p: [0.0, 0.0, -0.182204], fa_q: [-0.999999, 0.0, -0.0, 0.001194], fb_p: [0.0, 0.0, -0.007736], fb_q: [-1.0, 0.0, -0.0, 0.0], swing_deg: 25.0, twist_deg: [-15.0, 15.0], joint_friction: 1.0, cap_a: [0.06, -0.0, -0.052264], cap_b: [-0.06, 0.0, -0.052264], cap_r: 0.12, neg_group: true },
    BoneDef { parent: 1, ref_p: [0.0, 1.194336, -0.027087], ref_q: [0.703611, 0.0, 0.0, 0.710586], joint: BoneJoint::Spherical, fa_p: [0.0, -0.0, -0.088935], fa_q: [-0.998619, -0.0, 0.0, -0.052540], fb_p: [-0.0, 0.0, -0.008199], fb_q: [-1.0, 0.0, -0.0, 0.0], swing_deg: 25.0, twist_deg: [-15.0, 15.0], joint_friction: 1.0, cap_a: [0.08, -0.015133, -0.091801], cap_b: [-0.08, -0.015133, -0.091801], cap_r: 0.10, neg_group: false },
    BoneDef { parent: 2, ref_p: [-0.0, 1.31043, -0.028232], ref_q: [0.669856, 0.000001, -0.000001, 0.742491], joint: BoneJoint::Spherical, fa_p: [-0.0, 0.0, -0.124298], fa_q: [-0.998921, 0.000001, -0.000001, -0.046434], fb_p: [0.0, 0.0, 0.0], fb_q: [-1.0, 0.0, -0.000001, 0.0], swing_deg: 15.0, twist_deg: [-10.0, 10.0], joint_friction: 1.0, cap_a: [0.11, -0.039753, -0.13], cap_b: [-0.11, -0.039753, -0.13], cap_r: 0.145, neg_group: false },
    BoneDef { parent: 3, ref_p: [0.0, 1.575582, -0.055837], ref_q: [0.879922, 0.0, 0.0, 0.475118], joint: BoneJoint::Spherical, fa_p: [0.000001, -0.000259, -0.266585], fa_q: [-0.942192, -0.000001, 0.0, 0.335074], fb_p: [0.0, 0.0, 0.0], fb_q: [-1.0, 0.0, -0.000001, 0.0], swing_deg: 45.0, twist_deg: [-15.0, 15.0], joint_friction: 0.8, cap_a: [-0.000001, -0.0, -0.02], cap_b: [0.0, -0.005, -0.08], cap_r: 0.07, neg_group: false },
    BoneDef { parent: 4, ref_p: [0.0, 1.653348, -0.003241], ref_q: [0.750288, 0.0, 0.0, 0.661111], joint: BoneJoint::Spherical, fa_p: [0.0, 0.001321, -0.093873], fa_q: [-0.974301, -0.0, -0.0, -0.225251], fb_p: [0.0, 0.001268, -0.005104], fb_q: [-1.0, 0.0, -0.0, 0.0], swing_deg: 15.0, twist_deg: [-15.0, 15.0], joint_friction: 0.4, cap_a: [-0.000001, 0.016892, -0.05869], cap_b: [0.0, -0.003629, -0.115072], cap_r: 0.0975, neg_group: false },
    BoneDef { parent: 0, ref_p: [0.090416, 0.986104, -0.035090], ref_q: [-0.703287, -0.070715, 0.053866, 0.705327], joint: BoneJoint::Spherical, fa_p: [0.05, 0.011537, -0.055325], fa_q: [-0.714896, -0.022305, -0.698361, -0.026790], fb_p: [0.0, 0.0, 0.0], fb_q: [-0.002064, 0.758987, 0.017046, 0.650880], swing_deg: 10.0, twist_deg: [-60.0, 40.0], joint_friction: 1.0, cap_a: [0.023719, 0.006008, -0.039068], cap_b: [-0.064492, -0.004664, -0.424718], cap_r: 0.09, neg_group: true },
    BoneDef { parent: 6, ref_p: [0.101198, 0.527027, -0.037374], ref_q: [-0.653328, -0.066860, 0.058582, 0.751838], joint: BoneJoint::Revolute, fa_p: [-0.069989, 0.000253, -0.453844], fa_q: [-0.000677, 0.760087, 0.105674, 0.641171], fb_p: [0.0, 0.0, 0.0], fb_q: [-0.044589, 0.765540, 0.053368, 0.639619], swing_deg: 0.0, twist_deg: [-5.0, 45.0], joint_friction: 1.0, cap_a: [0.001778, 0.0, 0.009841], cap_b: [-0.078577, 0.014707, -0.41816], cap_r: 0.075, neg_group: false },
    BoneDef { parent: 0, ref_p: [-0.090416, 0.986104, -0.03509], ref_q: [-0.703287, 0.070715, -0.053865, 0.705326], joint: BoneJoint::Spherical, fa_p: [-0.05, 0.011537, -0.055326], fa_q: [-0.039089, -0.714094, 0.043177, 0.697623], fb_p: [0.0, 0.0, 0.0], fb_q: [0.758805, -0.019886, -0.651012, -0.001759], swing_deg: 10.0, twist_deg: [-30.0, 60.0], joint_friction: 1.0, cap_a: [-0.023719, 0.006008, -0.039068], cap_b: [0.064492, -0.004664, -0.424718], cap_r: 0.09, neg_group: true },
    BoneDef { parent: 8, ref_p: [-0.101198, 0.527027, -0.037373], ref_q: [-0.653327, 0.06686, -0.058582, 0.751839], joint: BoneJoint::Revolute, fa_p: [0.069988, 0.000253, -0.453844], fa_q: [0.760086, -0.000675, -0.641171, -0.105676], fb_p: [0.0, 0.0, 0.0], fb_q: [0.765540, -0.044589, -0.639619, -0.053368], swing_deg: 0.0, twist_deg: [-45.0, 5.0], joint_friction: 1.0, cap_a: [-0.001820, 0.0, 0.010071], cap_b: [0.077883, 0.014825, -0.418047], cap_r: 0.075, neg_group: false },
    BoneDef { parent: 3, ref_p: [0.20378, 1.484275, -0.115897], ref_q: [0.143082, 0.695980, -0.690130, 0.13733], joint: BoneJoint::Spherical, fa_p: [0.203780, -0.069369, -0.181921], fa_q: [-0.278486, 0.445600, -0.097014, 0.845266], fb_p: [0.0, 0.0, 0.0], fb_q: [-0.201396, -0.001586, 0.901850, 0.382234], swing_deg: 60.0, twist_deg: [-5.0, 5.0], joint_friction: 1.0, cap_a: [0.0, 0.0, 0.0], cap_b: [-0.091118, 0.037775, 0.229719], cap_r: 0.075, neg_group: false },
    BoneDef { parent: 10, ref_p: [0.305614, 1.242908, -0.117599], ref_q: [0.165048, 0.563437, -0.802002, 0.109959], joint: BoneJoint::Revolute, fa_p: [-0.095482, 0.039584, 0.240723], fa_q: [0.512487, -0.180629, 0.839474, 0.003742], fb_p: [0.0, 0.0, 0.0], fb_q: [0.503803, -0.029831, 0.858168, 0.094017], swing_deg: 0.0, twist_deg: [-5.0, 60.0], joint_friction: 1.0, cap_a: [0.0, 0.0, 0.0], cap_b: [-0.142406, 0.039392, 0.261092], cap_r: 0.05, neg_group: false },
    BoneDef { parent: 3, ref_p: [-0.20378, 1.484276, -0.115899], ref_q: [0.143083, -0.695978, 0.690132, 0.137329], joint: BoneJoint::Spherical, fa_p: [-0.203779, -0.069371, -0.181922], fa_q: [-0.253621, -0.414842, 0.106962, 0.867261], fb_p: [0.0, 0.0, 0.0], fb_q: [-0.201397, 0.001587, -0.901850, 0.382233], swing_deg: 60.0, twist_deg: [-5.0, 5.0], joint_friction: 1.0, cap_a: [0.0, 0.0, 0.0], cap_b: [0.091118, 0.037775, 0.229718], cap_r: 0.075, neg_group: false },
    BoneDef { parent: 12, ref_p: [-0.305614, 1.242907, -0.117599], ref_q: [0.165048, -0.563437, 0.802002, 0.109959], joint: BoneJoint::Revolute, fa_p: [0.095484, 0.039585, 0.240723], fa_q: [-0.180627, 0.512487, -0.003744, -0.839474], fb_p: [0.0, 0.0, 0.0], fb_q: [-0.029831, 0.503803, -0.094017, -0.858169], swing_deg: 0.0, twist_deg: [-60.0, 5.0], joint_friction: 1.0, cap_a: [0.0, 0.0, 0.0], cap_b: [0.142406, 0.039392, 0.261092], cap_r: 0.05, neg_group: false },
];

struct HumanInstance {
    bodies: Vec<RigidBodyHandle>,
    joints: Vec<ImpulseJointHandle>,
}

// box3d joints are about the local frame Z axis; rapier generic joints use X
// as the principal axis. B rotates rapier-X onto box3d-Z (rotation about Y by
// -pi/2), applied to both joint frames.
fn basis_change() -> Rotation {
    Rotation::from_axis_angle(Vector::Y, -0.5 * PI)
}

fn create_human(w: &mut World, position: Vector, friction_torque: f32, group_index: i32) -> HumanInstance {
    let mut bodies = Vec::with_capacity(14);
    let mut joints = Vec::new();

    // box3d negative filter group: shapes with the same negative group index
    // never collide. Emulated with a per-rain-group category bit (mod 31 —
    // aliasing groups are spatially disjoint). Bit 0 is the default world.
    let neg_groups = if group_index > 0 {
        let bit = 1u32 << (1 + ((group_index as u32 - 1) % 31));
        InteractionGroups::new(
            Group::from_bits_retain(bit),
            Group::from_bits_retain(!bit),
            InteractionTestMode::And,
        )
    } else {
        InteractionGroups::default()
    };

    let b = basis_change();

    for bone in BONES.iter() {
        let ref_q = quat(bone.ref_q[0], bone.ref_q[1], bone.ref_q[2], bone.ref_q[3]);
        let ref_p = Vector::new(bone.ref_p[0], bone.ref_p[1], bone.ref_p[2]) + position;
        let body = w.bodies.insert(RigidBodyBuilder::dynamic().pose(Pose::from_parts(ref_p, ref_q)));

        let ca = Vector::new(bone.cap_a[0], bone.cap_a[1], bone.cap_a[2]);
        let cb = Vector::new(bone.cap_b[0], bone.cap_b[1], bone.cap_b[2]);
        let mut collider = ColliderBuilder::capsule_from_endpoints(ca, cb, bone.cap_r)
            .friction(0.6)
            .restitution(0.0)
            .density(1000.0);
        if bone.neg_group {
            collider = collider.collision_groups(neg_groups);
        }
        w.colliders.insert_with_parent(collider, body, &mut w.bodies);
        bodies.push(body);
    }

    for (i, bone) in BONES.iter().enumerate() {
        if bone.parent < 0 {
            continue;
        }
        let parent = bodies[bone.parent as usize];
        let child = bodies[i];

        let fa = Pose::from_parts(
            Vector::new(bone.fa_p[0], bone.fa_p[1], bone.fa_p[2]),
            quat(bone.fa_q[0], bone.fa_q[1], bone.fa_q[2], bone.fa_q[3]) * b,
        );
        let fb = Pose::from_parts(
            Vector::new(bone.fb_p[0], bone.fb_p[1], bone.fb_p[2]),
            quat(bone.fb_q[0], bone.fb_q[1], bone.fb_q[2], bone.fb_q[3]) * b,
        );

        let max_torque = bone.joint_friction * friction_torque;
        let joint = match bone.joint {
            BoneJoint::Revolute => GenericJointBuilder::new(JointAxesMask::LOCKED_REVOLUTE_AXES)
                .local_frame1(fa)
                .local_frame2(fb)
                .limits(JointAxis::AngX, [bone.twist_deg[0] * DEG_TO_RAD, bone.twist_deg[1] * DEG_TO_RAD])
                .motor_velocity(JointAxis::AngX, 0.0, 1.0)
                .motor_max_force(JointAxis::AngX, max_torque)
                .contacts_enabled(false)
                .build(),
            BoneJoint::Spherical => {
                let swing = bone.swing_deg * DEG_TO_RAD;
                GenericJointBuilder::new(JointAxesMask::LOCKED_SPHERICAL_AXES)
                    .local_frame1(fa)
                    .local_frame2(fb)
                    .limits(JointAxis::AngX, [bone.twist_deg[0] * DEG_TO_RAD, bone.twist_deg[1] * DEG_TO_RAD])
                    .limits(JointAxis::AngY, [-swing, swing])
                    .limits(JointAxis::AngZ, [-swing, swing])
                    .motor_velocity(JointAxis::AngX, 0.0, 1.0)
                    .motor_max_force(JointAxis::AngX, max_torque)
                    .motor_velocity(JointAxis::AngY, 0.0, 1.0)
                    .motor_max_force(JointAxis::AngY, max_torque)
                    .motor_velocity(JointAxis::AngZ, 0.0, 1.0)
                    .motor_max_force(JointAxis::AngZ, max_torque)
                    .contacts_enabled(false)
                    .build()
            }
            BoneJoint::None => unreachable!(),
        };
        joints.push(w.impulse_joints.insert(parent, child, joint, true));
    }

    // box3d filter joint between the thighs: a constraint-free joint that
    // only disables collision between the pair.
    let filter = GenericJointBuilder::new(JointAxesMask::empty()).contacts_enabled(false).build();
    joints.push(w.impulse_joints.insert(bodies[6], bodies[8], filter, true));

    HumanInstance { bodies, joints }
}

fn destroy_human(w: &mut World, human: &mut HumanInstance) {
    for j in human.joints.drain(..) {
        w.impulse_joints.remove(j, false);
    }
    for b in human.bodies.drain(..) {
        w.bodies.remove(b, &mut w.islands, &mut w.colliders, &mut w.impulse_joints, &mut w.multibody_joints, true);
    }
}

#[derive(Default)]
struct Rain {
    groups: Vec<Vec<HumanInstance>>,
    column_count: usize,
    column_index: usize,
}

impl Rain {
    fn create_group(&mut self, w: &mut World, row_index: usize, column_index: usize) {
        let group_index = row_index * RAIN_GRID_COUNT + column_index;
        let span = RAIN_GRID_COUNT as f32 * RAIN_GRID_SIZE;
        let group_distance = 1.0 * span / RAIN_GRID_COUNT as f32;
        let mut position = Vector::new(
            -0.5 * span + group_distance * (column_index as f32 + 0.5),
            20.0,
            -0.5 * span + group_distance * (row_index as f32 + 0.5),
        );
        for _ in 0..RAIN_GROUP_SIZE {
            let human = create_human(w, position, 5.0, group_index as i32);
            self.groups[group_index].push(human);
            position.x += 0.75;
        }
    }

    fn destroy_group(&mut self, w: &mut World, row_index: usize, column_index: usize) {
        let group_index = row_index * RAIN_GRID_COUNT + column_index;
        let mut humans = std::mem::take(&mut self.groups[group_index]);
        for human in humans.iter_mut() {
            destroy_human(w, human);
        }
    }
}

impl Scene for Rain {
    fn create(&mut self, w: &mut World) {
        self.groups.clear();
        for _ in 0..RAIN_GRID_COUNT * RAIN_GRID_COUNT {
            self.groups.push(Vec::new());
        }
        self.column_count = 0;
        self.column_index = 0;

        let half = 4;
        let cell = RAIN_GRID_SIZE / (2.0 * half as f32);
        let (gv, gi) = grid_mesh(2 * half, 2 * half, cell);
        let (tv, ti) = torus_mesh(16, 16, 0.25 * RAIN_GRID_SIZE, 1.0);

        let span = RAIN_GRID_SIZE * RAIN_GRID_COUNT as f32;
        let mut px = -0.5 * span + 0.5 * RAIN_GRID_SIZE;
        for _i in 0..RAIN_GRID_COUNT {
            let mut pz = -0.5 * span + 0.5 * RAIN_GRID_SIZE;
            for _j in 0..RAIN_GRID_COUNT {
                let body = w.bodies.insert(RigidBodyBuilder::fixed().translation(Vector::new(px, 0.0, pz)));
                w.colliders.insert_with_parent(
                    ColliderBuilder::trimesh_with_flags(gv.clone(), gi.clone(), TriMeshFlags::FIX_INTERNAL_EDGES)
                        .expect("rain grid mesh")
                        .friction(0.6)
                        .restitution(0.0),
                    body,
                    &mut w.bodies,
                );
                w.colliders.insert_with_parent(
                    ColliderBuilder::trimesh_with_flags(tv.clone(), ti.clone(), TriMeshFlags::FIX_INTERNAL_EDGES)
                        .expect("rain torus mesh")
                        .friction(0.6)
                        .restitution(0.0),
                    body,
                    &mut w.bodies,
                );
                pz += RAIN_GRID_SIZE;
            }
            px += RAIN_GRID_SIZE;
        }
    }

    fn step_scene(&mut self, w: &mut World, step_index: i32) {
        let delay: i32 = 0x2F;
        let increment = 1usize;

        if (step_index & delay) == 0 {
            if self.column_count < RAIN_GRID_COUNT {
                let mut i = 0;
                while i < RAIN_GRID_COUNT {
                    let column = self.column_count;
                    self.create_group(w, i, column);
                    i += increment;
                }
                self.column_count = (self.column_count + increment).min(RAIN_GRID_COUNT);
            } else {
                let mut i = 0;
                while i < RAIN_GRID_COUNT {
                    let column = self.column_index;
                    self.destroy_group(w, i, column);
                    self.create_group(w, i, column);
                    i += increment;
                }
                self.column_index += increment;
                if self.column_index >= RAIN_GRID_COUNT {
                    self.column_index = 0;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------

fn sanity(w: &World) -> (usize, usize, usize, f32, f32) {
    let mut max_abs = 0.0f32;
    let mut max_y = f32::MIN;
    for (_, body) in w.bodies.iter() {
        if body.is_dynamic() {
            let p = body.translation();
            max_abs = max_abs.max(p.x.abs()).max(p.y.abs()).max(p.z.abs());
            max_y = max_y.max(p.y);
        }
    }
    (w.bodies.len(), w.colliders.len(), w.impulse_joints.len(), max_abs, max_y)
}

fn main() {
    // Indices match the box3d benchmark: -b=0..8.
    let scenes: Vec<(&str, fn() -> Box<dyn Scene>, i32)> = vec![
        ("trees100", || Box::new(Trees { scale: 1 }), 500),
        ("trees50", || Box::new(Trees { scale: 2 }), 500),
        ("trees25", || Box::new(Trees { scale: 4 }), 500),
        ("joint_grid", || Box::new(JointGrid), 100),
        ("junkyard", || Box::new(Junkyard::default()), 500),
        ("large_pyramid", || Box::new(LargePyramid), 200),
        ("many_pyramids", || Box::new(ManyPyramids), 100),
        ("rain", || Box::new(Rain::default()), 400),
        ("washer", || Box::new(Washer), 1000),
    ];

    let mut run_count = 4;
    let mut single: i32 = -1;
    for arg in std::env::args().skip(1) {
        if let Some(v) = arg.strip_prefix("-b=") {
            single = v.parse().unwrap_or(-1);
        } else if let Some(v) = arg.strip_prefix("-r=") {
            run_count = v.parse().unwrap_or(4);
        }
    }

    let det = cfg!(feature = "det");
    println!("rapier3d 0.32 benchmark (single thread, enhanced-determinism: {})", det);
    println!("parry SIMD_WIDTH: {}", rapier3d::parry::math::SIMD_WIDTH);

    for (index, (name, make, step_count)) in scenes.iter().enumerate() {
        if single >= 0 && index as i32 != single {
            continue;
        }
        println!("benchmark: {}, steps = {}", name, step_count);

        let mut min_ms = f64::MAX;
        let mut counts = (0, 0, 0, 0.0, 0.0);
        for run in 0..run_count {
            let mut scene = make();
            let mut w = World::new();
            scene.create(&mut w);

            // box3d protocol: scenario.step(0) + first world step untimed,
            // time the rest.
            scene.step_scene(&mut w, 0);
            w.step();
            let t0 = Instant::now();
            for step_index in 1..*step_count {
                scene.step_scene(&mut w, step_index);
                w.step();
            }
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            println!("run {} : {:.2} (ms)", run, ms);
            min_ms = min_ms.min(ms);
            counts = sanity(&w);
        }
        let (nb, nc, nj, max_abs, max_y) = counts;
        println!(
            "bodies {} / colliders {} / joints {} / max|p| {:.1} / max y {:.1}",
            nb, nc, nj, max_abs, max_y
        );
        println!("{}: min {:.2} ms, {:.3} ms/step\n", name, min_ms, min_ms / (*step_count as f64 - 1.0));
    }
}
