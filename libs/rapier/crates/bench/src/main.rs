// Rapier counterpart of libs/box3d/examples/benchmark.rs for the scenes
// large_pyramid, many_pyramids and joint_grid. Scene geometry, material
// values, timestep and timing protocol mirror the box3d benchmark exactly:
//
// - dt = 1/60, gravity (0, -10, 0), single threaded (no `parallel` feature)
// - rapier IntegrationParameters::default() with num_solver_iterations = 4
//   set explicitly (rapier's small-steps solver iterations vs box3d's 4
//   sub-steps — the closest semantic match, both TGS-soft style)
// - friction 0.6 (box3d default), restitution 0, density 100 for pyramid
//   boxes / 1000 (box3d water default) for joint-grid spheres
// - sleeping disabled exactly where box3d disables it
// - timing: create world, one untimed step, time the remaining N-1 steps,
//   min over runs (box3d harness protocol)
use rapier3d::prelude::*;
use std::time::Instant;

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

// box3d LargePyramid: ground (400,1,400) at y=-1; 90-base pyramid of
// half-extent 0.5 boxes, density 100, sleeping disabled world-wide.
fn create_large_pyramid(w: &mut World) {
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

// box3d create_small_pyramid + ManyPyramids: 14x14 grid of 10-base pyramids.
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

fn create_many_pyramids(w: &mut World) {
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

// box3d JointGrid: 100x100 spheres (radius 0.4, density 1000 = box3d shape
// default), column i==0 static, spherical joints down each column and across
// each row with the same local anchors. Spheres are in collision group 2 and
// masked against group 2 so they never collide with each other (box3d
// category_bits = 2, mask_bits = !2).
fn create_joint_grid(w: &mut World) {
    let n: i32 = 100;
    // And-mode = box3d's should_collide semantics ((catA&maskB) && (catB&maskA)).
    let groups =
        InteractionGroups::new(Group::from_bits_retain(2), Group::from_bits_retain(!2u32), InteractionTestMode::And);

    let mut handles: Vec<RigidBodyHandle> = Vec::with_capacity((n * n) as usize);
    let mut index = 0usize;

    for k in 0..n {
        for i in 0..n {
            let builder = if i == 0 { RigidBodyBuilder::fixed() } else { RigidBodyBuilder::dynamic() };
            let body = w
                .bodies
                .insert(builder.translation(Vector::new(k as f32, -(i as f32), 0.0)).can_sleep(false));
            w.colliders.insert_with_parent(
                ColliderBuilder::ball(0.4).friction(0.6).restitution(0.0).density(1000.0).collision_groups(groups),
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
    let scenes: Vec<(&str, fn(&mut World), i32)> = vec![
        ("large_pyramid", create_large_pyramid, 200),
        ("many_pyramids", create_many_pyramids, 100),
        ("joint_grid", create_joint_grid, 100),
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

    for (index, (name, create, step_count)) in scenes.iter().enumerate() {
        if single >= 0 && index as i32 != single {
            continue;
        }
        println!("benchmark: {}, steps = {}", name, step_count);

        let mut min_ms = f64::MAX;
        let mut counts = (0, 0, 0, 0.0, 0.0);
        for run in 0..run_count {
            let mut w = World::new();
            create(&mut w);

            // box3d protocol: first step untimed, time the rest.
            w.step();
            let t0 = Instant::now();
            for _ in 1..*step_count {
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
