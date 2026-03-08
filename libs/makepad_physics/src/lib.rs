pub mod broad_phase;
pub mod contact;
pub mod hash;
pub mod narrow_phase;
pub mod rigid_body;
pub mod solver;
pub mod world;

pub use contact::*;
pub use hash::*;
pub use rigid_body::*;
pub use world::*;

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_math::*;

    // ---- Math tests ----

    #[test]
    fn test_mat3f_identity_mul_vec() {
        let m = Mat3f::identity();
        let v = vec3f(1.0, 2.0, 3.0);
        let r = m.mul_vec3(v);
        assert_eq!(r.x, v.x);
        assert_eq!(r.y, v.y);
        assert_eq!(r.z, v.z);
    }

    #[test]
    fn test_mat3f_diagonal() {
        let m = Mat3f::from_diagonal(vec3f(2.0, 3.0, 4.0));
        let v = vec3f(1.0, 1.0, 1.0);
        let r = m.mul_vec3(v);
        assert_eq!(r.x, 2.0);
        assert_eq!(r.y, 3.0);
        assert_eq!(r.z, 4.0);
    }

    #[test]
    fn test_mat3f_transpose() {
        let m = Mat3f {
            c0: vec3f(1.0, 2.0, 3.0),
            c1: vec3f(4.0, 5.0, 6.0),
            c2: vec3f(7.0, 8.0, 9.0),
        };
        let mt = m.transpose();
        assert_eq!(mt.c0, vec3f(1.0, 4.0, 7.0));
        assert_eq!(mt.c1, vec3f(2.0, 5.0, 8.0));
        assert_eq!(mt.c2, vec3f(3.0, 6.0, 9.0));
    }

    #[test]
    fn test_mat3f_from_quat_identity() {
        let q = Quat::default(); // identity
        let m = Mat3f::from_quat(q);
        let v = vec3f(1.0, 2.0, 3.0);
        let r = m.mul_vec3(v);
        assert!((r.x - v.x).abs() < 1e-6);
        assert!((r.y - v.y).abs() < 1e-6);
        assert!((r.z - v.z).abs() < 1e-6);
    }

    #[test]
    fn test_mat3f_mul_mat3_identity() {
        let a = Mat3f::from_diagonal(vec3f(2.0, 3.0, 4.0));
        let i = Mat3f::identity();
        let r = a.mul_mat3(&i);
        assert_eq!(r.c0, a.c0);
        assert_eq!(r.c1, a.c1);
        assert_eq!(r.c2, a.c2);
    }

    #[test]
    fn test_quat_integrate_stays_normalized() {
        let q = Quat::default();
        let omega = vec3f(1.0, 0.5, -0.3);
        let q2 = q.integrate(omega, 1.0 / 60.0);
        let len = q2.length();
        assert!(
            (len - 1.0).abs() < 1e-5,
            "Quaternion length after integrate: {}",
            len
        );
    }

    #[test]
    fn test_aabb_overlap() {
        let a = Aabb {
            min: vec3f(-1.0, -1.0, -1.0),
            max: vec3f(1.0, 1.0, 1.0),
        };
        let b = Aabb {
            min: vec3f(0.5, 0.5, 0.5),
            max: vec3f(2.0, 2.0, 2.0),
        };
        let c = Aabb {
            min: vec3f(3.0, 3.0, 3.0),
            max: vec3f(4.0, 4.0, 4.0),
        };
        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c));
    }

    #[test]
    fn test_aabb_from_rotated_cuboid() {
        let pose = Pose {
            position: vec3f(0.0, 5.0, 0.0),
            orientation: Quat::from_axis_angle(vec3f(0.0, 0.0, 1.0), std::f32::consts::FRAC_PI_4),
        };
        let he = vec3f(1.0, 0.5, 0.5);
        let aabb = Aabb::from_cuboid(he, &pose);
        // After 45-degree rotation around Z, a 2x1x1 box should have larger X and Y extent
        assert!(aabb.max.x > 0.7);
        assert!(aabb.max.y > 5.7);
        assert!(aabb.min.y < 4.3);
    }

    // ---- Physics tests ----

    fn make_single_cube_world() -> PhysicsWorld {
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        world.step(&[PhysicsOp::SpawnDynamic {
            position: vec3f(0.0, 5.0, 0.0),
            half_extents: vec3f(0.5, 0.5, 0.5),
            velocity: Vec3f::default(),
            density: 1.0,
        }]);
        world
    }

    #[test]
    fn test_single_cube_drop() {
        let mut world = make_single_cube_world();
        // Run 300 frames (~5 seconds)
        for _ in 0..300 {
            world.step(&[]);
        }
        // Cube should have settled near ground
        let y = world.bodies[0].pose.position.y;
        assert!(y > 0.0, "Cube fell through ground: y={}", y);
        assert!(y < 2.0, "Cube didn't fall enough: y={}", y);

        // Velocity should be near zero (settled)
        let vy = world.bodies[0].linear_velocity.y.abs();
        assert!(vy < 1.0, "Cube still moving fast: vy={}", vy);
    }

    #[test]
    fn test_cube_stack() {
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        // Two cubes stacked
        world.step(&[
            PhysicsOp::SpawnDynamic {
                position: vec3f(0.0, 0.5, 0.0),
                half_extents: vec3f(0.5, 0.5, 0.5),
                velocity: Vec3f::default(),
                density: 1.0,
            },
            PhysicsOp::SpawnDynamic {
                position: vec3f(0.0, 1.5, 0.0),
                half_extents: vec3f(0.5, 0.5, 0.5),
                velocity: Vec3f::default(),
                density: 1.0,
            },
        ]);
        for _ in 0..600 {
            world.step(&[]);
        }
        // Both cubes should be above ground
        assert!(
            world.bodies[0].pose.position.y > 0.0,
            "Bottom cube fell through ground"
        );
        assert!(
            world.bodies[1].pose.position.y > world.bodies[0].pose.position.y,
            "Top cube not above bottom cube"
        );
    }

    #[test]
    fn test_no_ground_penetration() {
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        // Spawn several cubes at different heights
        let ops: Vec<PhysicsOp> = (0..10)
            .map(|i| PhysicsOp::SpawnDynamic {
                position: vec3f(0.0, 2.0 + i as f32 * 1.5, 0.0),
                half_extents: vec3f(0.5, 0.5, 0.5),
                velocity: Vec3f::default(),
                density: 1.0,
            })
            .collect();
        world.step(&ops);

        for _ in 0..1000 {
            world.step(&[]);
        }

        for (i, body) in world.bodies.iter().enumerate() {
            // Bottom of cube = position.y - half_extent.y
            let bottom = body.pose.position.y - body.half_extents.y;
            assert!(
                bottom > -0.1,
                "Body {} penetrated ground: bottom_y={}",
                i,
                bottom
            );
        }
    }

    #[test]
    fn test_energy_dissipation() {
        let mut world = make_single_cube_world();

        // Run a bit to get the cube moving
        for _ in 0..30 {
            world.step(&[]);
        }

        let ke_early = kinetic_energy(&world);
        assert!(
            ke_early > 0.0,
            "Cube should have kinetic energy while falling"
        );

        // Run longer
        for _ in 0..600 {
            world.step(&[]);
        }

        let ke_late = kinetic_energy(&world);
        assert!(
            ke_late < ke_early,
            "Energy should decrease over time: early={} late={}",
            ke_early,
            ke_late
        );
    }

    #[test]
    fn test_pile_stays_compact() {
        // Regression test for runaway lateral energy in the default 5x5x5 pile.
        // This matches the example scene setup.
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        let mut ops = Vec::new();
        let grid = 5usize;
        let spacing = 1.1f32;
        let half = 0.5f32;
        for y in 0..grid {
            for x in 0..grid {
                for z in 0..grid {
                    ops.push(PhysicsOp::SpawnDynamic {
                        position: vec3f(
                            (x as f32 - grid as f32 / 2.0 + 0.5) * spacing,
                            2.0 + y as f32 * spacing,
                            (z as f32 - grid as f32 / 2.0 + 0.5) * spacing,
                        ),
                        half_extents: vec3f(half, half, half),
                        velocity: Vec3f::default(),
                        density: 1.0,
                    });
                }
            }
        }
        world.step(&ops);

        let mut max_xz_dist = 0.0f32;
        let mut max_speed_after_settle = 0.0f32;

        for frame in 0..1200 {
            world.step(&[]);
            for body in &world.bodies {
                let xz_dist = (body.pose.position.x * body.pose.position.x
                    + body.pose.position.z * body.pose.position.z)
                    .sqrt();
                max_xz_dist = max_xz_dist.max(xz_dist);
                if frame >= 600 {
                    max_speed_after_settle =
                        max_speed_after_settle.max(body.linear_velocity.length());
                }
            }
        }

        assert!(
            max_xz_dist < 5.0,
            "Pile spread too far laterally: max_xz_dist={}",
            max_xz_dist
        );
        assert!(
            max_speed_after_settle < 3.0,
            "Pile still has excessive speed after settling: max_speed={}",
            max_speed_after_settle
        );
    }

    #[test]
    fn test_kicked_tower_settles() {
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        let mut ops = Vec::new();
        let grid = 4usize;
        let height = 8usize;
        let spacing = 1.1f32;
        let half = 0.5f32;
        let mut kick_body = 0usize;
        let mut body_index = 0usize;

        for y in 0..height {
            for x in 0..grid {
                for z in 0..grid {
                    if x == grid - 1 && y == height / 2 && z == grid / 2 {
                        kick_body = body_index;
                    }
                    ops.push(PhysicsOp::SpawnDynamic {
                        position: vec3f(
                            (x as f32 - grid as f32 / 2.0 + 0.5) * spacing,
                            2.0 + y as f32 * spacing,
                            (z as f32 - grid as f32 / 2.0 + 0.5) * spacing,
                        ),
                        half_extents: vec3f(half, half, half),
                        velocity: Vec3f::default(),
                        density: 1.0,
                    });
                    body_index += 1;
                }
            }
        }

        world.step(&ops);
        for _ in 0..90 {
            world.step(&[]);
        }

        world.step(&[PhysicsOp::ApplyImpulse {
            body: kick_body,
            impulse: vec3f(9.0, 5.5, 2.0),
        }]);

        let mut max_linear_after_settle = 0.0f32;
        let mut max_angular_after_settle = 0.0f32;
        for frame in 0..1200 {
            world.step(&[]);
            if frame >= 900 {
                for body in &world.bodies {
                    max_linear_after_settle =
                        max_linear_after_settle.max(body.linear_velocity.length());
                    max_angular_after_settle =
                        max_angular_after_settle.max(body.angular_velocity.length());
                }
            }
        }

        assert!(
            max_linear_after_settle < 0.4,
            "Kicked tower kept sliding too fast after settling: max_linear_speed={}",
            max_linear_after_settle
        );
        assert!(
            max_angular_after_settle < 0.8,
            "Kicked tower kept rotating too fast after settling: max_angular_speed={}",
            max_angular_after_settle
        );
    }

    fn kinetic_energy(world: &PhysicsWorld) -> f32 {
        let mut ke = 0.0f32;
        for body in &world.bodies {
            if body.is_dynamic() {
                let mass = if body.inv_mass > 0.0 {
                    1.0 / body.inv_mass
                } else {
                    0.0
                };
                ke += 0.5 * mass * body.linear_velocity.length_squared();
            }
        }
        ke
    }

    #[test]
    #[ignore]
    fn debug_kicked_tower_profile() {
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        let mut ops = Vec::new();
        let grid = 4usize;
        let height = 8usize;
        let spacing = 1.1f32;
        let half = 0.5f32;
        let mut kick_body = 0usize;
        let mut body_index = 0usize;

        for y in 0..height {
            for x in 0..grid {
                for z in 0..grid {
                    if x == grid - 1 && y == height / 2 && z == grid / 2 {
                        kick_body = body_index;
                    }
                    ops.push(PhysicsOp::SpawnDynamic {
                        position: vec3f(
                            (x as f32 - grid as f32 / 2.0 + 0.5) * spacing,
                            2.0 + y as f32 * spacing,
                            (z as f32 - grid as f32 / 2.0 + 0.5) * spacing,
                        ),
                        half_extents: vec3f(half, half, half),
                        velocity: Vec3f::default(),
                        density: 1.0,
                    });
                    body_index += 1;
                }
            }
        }

        world.step(&ops);
        for _ in 0..90 {
            world.step(&[]);
        }
        world.step(&[PhysicsOp::ApplyImpulse {
            body: kick_body,
            impulse: vec3f(9.0, 5.5, 2.0),
        }]);

        for frame in 0..1200 {
            world.step(&[]);
            if frame % 120 == 119 {
                let max_speed = world
                    .bodies
                    .iter()
                    .map(|body| body.linear_velocity.length())
                    .fold(0.0f32, f32::max);
                println!("frame={} max_speed={}", frame + 1, max_speed);
            }
        }

        let mut ranked: Vec<_> = world
            .bodies
            .iter()
            .enumerate()
            .map(|(i, body)| {
                (
                    i,
                    body.pose.position,
                    body.pose.orientation,
                    body.linear_velocity.length(),
                    body.angular_velocity.length(),
                )
            })
            .collect();
        ranked.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());

        for (i, position, orientation, lin, ang) in ranked.into_iter().take(12) {
            println!(
                "body={} pos=({:.3},{:.3},{:.3}) rot=({:.4},{:.4},{:.4},{:.4}) lin={:.4} ang={:.4}",
                i,
                position.x,
                position.y,
                position.z,
                orientation.x,
                orientation.y,
                orientation.z,
                orientation.w,
                lin,
                ang
            );
        }
    }

    #[test]
    #[ignore]
    fn debug_single_escape_cube() {
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        world.step(&[PhysicsOp::SpawnDynamic {
            position: vec3f(18.092, 0.996, -76.012),
            half_extents: vec3f(0.5, 0.5, 0.5),
            velocity: Vec3f::default(),
            density: 1.0,
        }]);

        let body = &mut world.bodies[0];
        body.pose.orientation = Quat {
            x: -0.4656,
            y: 0.1799,
            z: 0.8665,
            w: 0.0089,
        };
        body.linear_velocity = vec3f(4.5503, 0.0, 0.0);
        body.angular_velocity = vec3f(0.0, 0.0, 6.0433);
        body.wake_up();

        for frame in 0..600 {
            world.step(&[]);
            if frame % 60 == 59 {
                let body = &world.bodies[0];
                println!(
                    "frame={} pos=({:.3},{:.3},{:.3}) lin={:.4} ang={:.4}",
                    frame + 1,
                    body.pose.position.x,
                    body.pose.position.y,
                    body.pose.position.z,
                    body.linear_velocity.length(),
                    body.angular_velocity.length()
                );
            }
        }
    }

    #[test]
    #[ignore]
    fn debug_single_escape_cube_vs_rapier() {
        use rapier3d::prelude::*;

        fn rvec(x: f32, y: f32, z: f32) -> rapier3d::math::Vector {
            rapier3d::math::Vector::new(x, y, z)
        }

        let mut our_world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        our_world.step(&[PhysicsOp::SpawnDynamic {
            position: vec3f(18.092, 0.996, -76.012),
            half_extents: vec3f(0.5, 0.5, 0.5),
            velocity: Vec3f::default(),
            density: 1.0,
        }]);
        {
            let body = &mut our_world.bodies[0];
            body.pose.orientation = Quat {
                x: -0.4656,
                y: 0.1799,
                z: 0.8665,
                w: 0.0089,
            };
            body.linear_velocity = vec3f(4.5503, 0.0, 0.0);
            body.angular_velocity = vec3f(0.0, 0.0, 6.0433);
            body.wake_up();
        }

        let mut bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut islands = IslandManager::new();
        let mut broad_phase = BroadPhaseBvh::new();
        let mut narrow_phase = NarrowPhase::new();
        let mut ccd_solver = CCDSolver::new();
        let mut pipeline = PhysicsPipeline::new();
        let gravity = rvec(0.0, -9.81, 0.0);
        let integration_parameters = IntegrationParameters {
            dt: 1.0 / 60.0,
            ..IntegrationParameters::default()
        };

        let ground = bodies.insert(RigidBodyBuilder::fixed().build());
        let ground_collider = ColliderBuilder::new(SharedShape::halfspace(rvec(0.0, 1.0, 0.0)))
            .friction(0.5)
            .restitution(0.0);
        let ground_collider = colliders.insert_with_parent(ground_collider, ground, &mut bodies);

        let cube = bodies.insert(
            RigidBodyBuilder::dynamic()
                .translation(rvec(18.092, 0.996, -76.012))
                .can_sleep(false)
                .build(),
        );
        let cube_collider = ColliderBuilder::cuboid(0.5, 0.5, 0.5)
            .density(1.0)
            .friction(0.5)
            .restitution(0.0);
        let cube_collider = colliders.insert_with_parent(cube_collider, cube, &mut bodies);
        {
            let body = bodies.get_mut(cube).unwrap();
            body.set_rotation(
                Rotation::from_xyzw(-0.4656, 0.1799, 0.8665, 0.0089).normalize(),
                true,
            );
            body.set_linvel(rvec(4.5503, 0.0, 0.0), true);
            body.set_angvel(rvec(0.0, 0.0, 6.0433), true);
        }

        let mut our_aabbs = Vec::new();
        let mut our_pairs = Vec::new();
        let mut our_manifolds = Vec::new();
        let mut our_solver_contacts = Vec::new();
        let mut our_solver_frictions = Vec::new();
        crate::broad_phase::broad_phase(&our_world.bodies, &mut our_aabbs, &mut our_pairs);
        crate::narrow_phase::narrow_phase(
            &our_world.bodies,
            &our_pairs,
            our_world.ground_y,
            &[],
            &mut our_manifolds,
        );
        crate::solver::prepare_contacts(
            &our_world.bodies,
            &our_manifolds,
            (1.0 / 60.0) / 4.0,
            &mut our_solver_contacts,
            &mut our_solver_frictions,
        );
        fn step_our_from_state(
            position: Vec3f,
            orientation: Quat,
            linear_velocity: Vec3f,
            angular_velocity: Vec3f,
        ) -> (Vec3f, f32, f32) {
            let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
            let mut body =
                crate::rigid_body::RigidBody::new_dynamic(position, vec3f(0.5, 0.5, 0.5), 1.0);
            body.pose.orientation = orientation;
            body.linear_velocity = linear_velocity;
            body.angular_velocity = angular_velocity;
            body.wake_up();
            world.bodies.push(body);
            world.step(&[]);
            let body = &world.bodies[0];
            (
                body.pose.position,
                body.linear_velocity.length(),
                body.angular_velocity.length(),
            )
        }
        fn step_our_with_history(
            world: &mut PhysicsWorld,
            position: Vec3f,
            orientation: Quat,
            linear_velocity: Vec3f,
            angular_velocity: Vec3f,
        ) -> (Vec3f, f32, f32) {
            if world.bodies.is_empty() {
                let mut body = crate::rigid_body::RigidBody::new_dynamic(
                    position,
                    vec3f(0.5, 0.5, 0.5),
                    1.0,
                );
                body.pose.orientation = orientation;
                body.linear_velocity = linear_velocity;
                body.angular_velocity = angular_velocity;
                body.wake_up();
                world.bodies.push(body);
            } else {
                let body = &mut world.bodies[0];
                body.pose.position = position;
                body.pose.orientation = orientation;
                body.linear_velocity = linear_velocity;
                body.angular_velocity = angular_velocity;
                body.sleeping = false;
                body.sleep_time = 0.0;
            }

            world.step(&[]);
            let body = &world.bodies[0];
            (
                body.pose.position,
                body.linear_velocity.length(),
                body.angular_velocity.length(),
            )
        }
        println!(
            "initial ours manifolds={} points={} solver_contacts={} friction_contacts={}",
            our_manifolds.len(),
            our_manifolds.first().map(|m| m.num_points).unwrap_or(0),
            our_solver_contacts.len(),
            our_solver_frictions
                .first()
                .map(|f| f.num_contacts)
                .unwrap_or(0)
        );
        if let Some(manifold) = our_manifolds.first() {
            for i in 0..manifold.num_points {
                let point = &manifold.points[i];
                println!(
                    "  ours_contact[{}] p_a=({:.3},{:.3},{:.3}) p_b=({:.3},{:.3},{:.3}) pen={:.5}",
                    i,
                    point.world_point_a.x,
                    point.world_point_a.y,
                    point.world_point_a.z,
                    point.world_point_b.x,
                    point.world_point_b.y,
                    point.world_point_b.z,
                    point.penetration,
                );
            }
        }

        let mut rapier_frame15: Option<(Vec3f, Quat, Vec3f, Vec3f)> = None;
        let mut rapier_frame19: Option<(Vec3f, Quat, Vec3f, Vec3f)> = None;
        let mut rapier_frame42: Option<(Vec3f, Quat, Vec3f, Vec3f)> = None;
        let mut rapier_frame49: Option<(Vec3f, Quat, Vec3f, Vec3f)> = None;
        let mut our_history_probe = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        let mut max_history_pos_err = 0.0f32;
        let mut max_history_lin_err = 0.0f32;
        let mut max_history_ang_err = 0.0f32;
        let mut max_history_frame = 0usize;

        for frame in 0..600 {
            let rapier_body_before = bodies.get(cube).unwrap();
            let contact_pair_before = narrow_phase.contact_pair(ground_collider, cube_collider);
            let rapier_rotation_before = rapier_body_before.rotation();
            let rapier_state_before = (
                vec3f(
                    rapier_body_before.translation().x,
                    rapier_body_before.translation().y,
                    rapier_body_before.translation().z,
                ),
                Quat {
                    x: rapier_rotation_before.x,
                    y: rapier_rotation_before.y,
                    z: rapier_rotation_before.z,
                    w: rapier_rotation_before.w,
                },
                vec3f(
                    rapier_body_before.linvel().x,
                    rapier_body_before.linvel().y,
                    rapier_body_before.linvel().z,
                ),
                vec3f(
                    rapier_body_before.angvel().x,
                    rapier_body_before.angvel().y,
                    rapier_body_before.angvel().z,
                ),
            );
            if frame == 19 || frame == 42 {
                let mut probe_body = crate::rigid_body::RigidBody::new_dynamic(
                    rapier_state_before.0,
                    vec3f(0.5, 0.5, 0.5),
                    1.0,
                );
                probe_body.pose.orientation = rapier_state_before.1;
                probe_body.linear_velocity = rapier_state_before.2;
                probe_body.angular_velocity = rapier_state_before.3;
                let probe_bodies = [probe_body];
                let mut probe_aabbs = Vec::new();
                let mut probe_pairs = Vec::new();
                let mut probe_manifolds = Vec::new();
                let mut probe_solver_contacts = Vec::new();
                let mut probe_solver_frictions = Vec::new();
                crate::broad_phase::broad_phase(&probe_bodies, &mut probe_aabbs, &mut probe_pairs);
                crate::narrow_phase::narrow_phase(
                    &probe_bodies,
                    &probe_pairs,
                    0.0,
                    &[],
                    &mut probe_manifolds,
                );
                crate::solver::prepare_contacts(
                    &probe_bodies,
                    &probe_manifolds,
                    (1.0 / 60.0) / 4.0,
                    &mut probe_solver_contacts,
                    &mut probe_solver_frictions,
                );
                println!(
                    "prestep_frame={} rapier_points={} rapier_solver_contacts={} probe_points={} probe_solver_contacts={}",
                    frame,
                    contact_pair_before
                        .and_then(|pair| pair.manifolds.first())
                        .map(|m| m.points.len())
                        .unwrap_or(0),
                    contact_pair_before
                        .and_then(|pair| pair.manifolds.first())
                        .map(|m| m.data.solver_contacts.len())
                        .unwrap_or(0),
                    probe_manifolds.first().map(|m| m.num_points).unwrap_or(0),
                    probe_solver_contacts.len(),
                );
                if let Some(manifold) = contact_pair_before.and_then(|pair| pair.manifolds.first()) {
                    for (i, point) in manifold.points.iter().enumerate() {
                        println!(
                            "  rapier_pre_contact[{}] p1=({:.6},{:.6},{:.6}) p2=({:.6},{:.6},{:.6}) dist={:.6}",
                            i,
                            point.local_p1.x,
                            point.local_p1.y,
                            point.local_p1.z,
                            point.local_p2.x,
                            point.local_p2.y,
                            point.local_p2.z,
                            point.dist,
                        );
                    }
                    for (i, contact) in manifold.data.solver_contacts.iter().enumerate() {
                        println!(
                            "  rapier_pre_solver[{}] point=({:.6},{:.6},{:.6}) dist={:.6} warm_n={:.6}",
                            i,
                            contact.point.x,
                            contact.point.y,
                            contact.point.z,
                            contact.dist,
                            contact.warmstart_impulse,
                        );
                    }
                }
                if let Some(manifold) = probe_manifolds.first() {
                    for i in 0..manifold.num_points {
                        let point = &manifold.points[i];
                        println!(
                            "  probe_pre_contact[{}] pa=({:.6},{:.6},{:.6}) pb=({:.6},{:.6},{:.6}) pen={:.6}",
                            i,
                            point.local_point_a.x,
                            point.local_point_a.y,
                            point.local_point_a.z,
                            point.local_point_b.x,
                            point.local_point_b.y,
                            point.local_point_b.z,
                            point.penetration,
                        );
                    }
                }
                for (i, contact) in probe_solver_contacts.iter().enumerate() {
                    println!(
                        "  probe_pre_solver[{}] point_index={} dist={:.6} local_p1=({:.6},{:.6},{:.6}) local_p2=({:.6},{:.6},{:.6}) r={:.6}",
                        i,
                        contact.point_index,
                        contact.dist,
                        contact.local_p1.x,
                        contact.local_p1.y,
                        contact.local_p1.z,
                        contact.local_p2.x,
                        contact.local_p2.y,
                        contact.local_p2.z,
                        contact.r_normal,
                    );
                }
            }

            our_world.step(&[]);
            let (history_probe_pos, history_probe_lin, history_probe_ang) = step_our_with_history(
                &mut our_history_probe,
                rapier_state_before.0,
                rapier_state_before.1,
                rapier_state_before.2,
                rapier_state_before.3,
            );
            pipeline.step(
                gravity,
                &integration_parameters,
                &mut islands,
                &mut broad_phase,
                &mut narrow_phase,
                &mut bodies,
                &mut colliders,
                &mut impulse_joints,
                &mut multibody_joints,
                &mut ccd_solver,
                &(),
                &(),
            );

            let mut our_aabbs = Vec::new();
            let mut our_pairs = Vec::new();
            let mut our_manifolds = Vec::new();
            let mut our_solver_contacts = Vec::new();
            let mut our_solver_frictions = Vec::new();
            crate::broad_phase::broad_phase(&our_world.bodies, &mut our_aabbs, &mut our_pairs);
            crate::narrow_phase::narrow_phase(
                &our_world.bodies,
                &our_pairs,
                our_world.ground_y,
                &[],
                &mut our_manifolds,
            );
            crate::solver::prepare_contacts(
                &our_world.bodies,
                &our_manifolds,
                (1.0 / 60.0) / 4.0,
                &mut our_solver_contacts,
                &mut our_solver_frictions,
            );

            let contact_pair = narrow_phase.contact_pair(ground_collider, cube_collider).unwrap();

            if frame == 0 {
                let contact_pair = narrow_phase.contact_pair(ground_collider, cube_collider).unwrap();
                println!(
                    "frame=1 rapier manifolds={} points={} solver_contacts={}",
                    contact_pair.manifolds.len(),
                    contact_pair.manifolds.first().map(|m| m.points.len()).unwrap_or(0),
                    contact_pair
                        .manifolds
                        .first()
                        .map(|m| m.data.solver_contacts.len())
                        .unwrap_or(0),
                );
                if let Some(manifold) = contact_pair.manifolds.first() {
                    for (i, point) in manifold.points.iter().enumerate() {
                        let p1 = point.local_p1;
                        let p2 = point.local_p2;
                        println!(
                            "  rapier_contact[{}] p1=({:.3},{:.3},{:.3}) p2=({:.3},{:.3},{:.3}) dist={:.5}",
                            i, p1.x, p1.y, p1.z, p2.x, p2.y, p2.z, point.dist
                        );
                    }
                }
            }

            if frame < 60 && ((frame + 1) % 10 == 0 || !our_solver_contacts.is_empty()) {
                let rapier_body = bodies.get(cube).unwrap();
                let rapier_rot = rapier_body.rotation();
                let rapier_linvel = rapier_body.linvel();
                let rapier_angvel = rapier_body.angvel();
                let mut probe_body = crate::rigid_body::RigidBody::new_dynamic(
                    vec3f(0.0, 0.0, 0.0),
                    vec3f(0.5, 0.5, 0.5),
                    1.0,
                );
                probe_body.pose.position = vec3f(
                    rapier_body.translation().x,
                    rapier_body.translation().y,
                    rapier_body.translation().z,
                );
                probe_body.pose.orientation = Quat {
                    x: rapier_rot.x,
                    y: rapier_rot.y,
                    z: rapier_rot.z,
                    w: rapier_rot.w,
                };
                probe_body.linear_velocity = vec3f(rapier_linvel.x, rapier_linvel.y, rapier_linvel.z);
                probe_body.angular_velocity =
                    vec3f(rapier_angvel.x, rapier_angvel.y, rapier_angvel.z);
                let probe_bodies = [probe_body];

                let mut probe_aabbs = Vec::new();
                let mut probe_pairs = Vec::new();
                let mut probe_manifolds = Vec::new();
                let mut probe_solver_contacts = Vec::new();
                let mut probe_solver_frictions = Vec::new();
                crate::broad_phase::broad_phase(&probe_bodies, &mut probe_aabbs, &mut probe_pairs);
                crate::narrow_phase::narrow_phase(
                    &probe_bodies,
                    &probe_pairs,
                    0.0,
                    &[],
                    &mut probe_manifolds,
                );
                crate::solver::prepare_contacts(
                    &probe_bodies,
                    &probe_manifolds,
                    (1.0 / 60.0) / 4.0,
                    &mut probe_solver_contacts,
                    &mut probe_solver_frictions,
                );
                println!(
                    "contact_frame={} ours_points={} ours_solver_contacts={} rapier_points={} rapier_solver_contacts={} probe_points={} probe_solver_contacts={}",
                    frame + 1,
                    our_manifolds.first().map(|m| m.num_points).unwrap_or(0),
                    our_solver_contacts.len(),
                    contact_pair.manifolds.first().map(|m| m.points.len()).unwrap_or(0),
                    contact_pair
                        .manifolds
                        .first()
                        .map(|m| m.data.solver_contacts.len())
                        .unwrap_or(0),
                    probe_manifolds.first().map(|m| m.num_points).unwrap_or(0),
                    probe_solver_contacts.len(),
                );
            }

            let rapier_body = bodies.get(cube).unwrap();
            let rapier_rotation = rapier_body.rotation();
            let rapier_state = (
                vec3f(
                    rapier_body.translation().x,
                    rapier_body.translation().y,
                    rapier_body.translation().z,
                ),
                Quat {
                    x: rapier_rotation.x,
                    y: rapier_rotation.y,
                    z: rapier_rotation.z,
                    w: rapier_rotation.w,
                },
                vec3f(
                    rapier_body.linvel().x,
                    rapier_body.linvel().y,
                    rapier_body.linvel().z,
                ),
                vec3f(
                    rapier_body.angvel().x,
                    rapier_body.angvel().y,
                    rapier_body.angvel().z,
                ),
            );
            if frame + 1 == 15 {
                rapier_frame15 = Some(rapier_state);
            }
            if frame + 1 == 19 {
                rapier_frame19 = Some(rapier_state);
            }
            if frame + 1 == 16 {
                if let Some((position, orientation, linvel, angvel)) = rapier_frame15 {
                    let (our_pos, our_lin, our_ang) =
                        step_our_from_state(position, orientation, linvel, angvel);
                    println!(
                        "single_step_15_to_16 our_pos=({:.3},{:.3},{:.3}) our_lin={:.4} our_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                        our_pos.x,
                        our_pos.y,
                        our_pos.z,
                        our_lin,
                        our_ang,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.length(),
                        rapier_state.3.length(),
                    );
                    println!(
                        "history_step_15_to_16 our_pos=({:.3},{:.3},{:.3}) our_lin={:.4} our_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                        history_probe_pos.x,
                        history_probe_pos.y,
                        history_probe_pos.z,
                        history_probe_lin,
                        history_probe_ang,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.length(),
                        rapier_state.3.length(),
                    );
                }
            }
            if frame + 1 == 20 {
                if let Some((position, orientation, linvel, angvel)) = rapier_frame19 {
                    let (our_pos, our_lin, our_ang) =
                        step_our_from_state(position, orientation, linvel, angvel);
                    println!(
                        "single_step_19_to_20 our_pos=({:.3},{:.3},{:.3}) our_lin={:.4} our_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                        our_pos.x,
                        our_pos.y,
                        our_pos.z,
                        our_lin,
                        our_ang,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.length(),
                        rapier_state.3.length(),
                    );
                    println!(
                        "history_step_19_to_20 our_pos=({:.3},{:.3},{:.3}) our_lin={:.4} our_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                        history_probe_pos.x,
                        history_probe_pos.y,
                        history_probe_pos.z,
                        history_probe_lin,
                        history_probe_ang,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.length(),
                        rapier_state.3.length(),
                    );
                }
            }
            if frame + 1 == 42 {
                rapier_frame42 = Some(rapier_state);
            }
            if frame + 1 == 49 {
                rapier_frame49 = Some(rapier_state);
            }
            if let Some(history_body) = our_history_probe.bodies.first() {
                let pos_err = (history_body.pose.position - rapier_state.0).length();
                let lin_err = (history_body.linear_velocity - rapier_state.2).length();
                let ang_err = (history_body.angular_velocity - rapier_state.3).length();
                if pos_err > max_history_pos_err
                    || lin_err > max_history_lin_err
                    || ang_err > max_history_ang_err
                {
                    max_history_pos_err = max_history_pos_err.max(pos_err);
                    max_history_lin_err = max_history_lin_err.max(lin_err);
                    max_history_ang_err = max_history_ang_err.max(ang_err);
                    max_history_frame = frame + 1;
                    println!(
                        "history_probe_error frame={} pos_err={:.6} lin_err={:.6} ang_err={:.6} our_pos=({:.6},{:.6},{:.6}) our_lin=({:.6},{:.6},{:.6}) our_ang=({:.6},{:.6},{:.6}) rapier_pos=({:.6},{:.6},{:.6}) rapier_lin=({:.6},{:.6},{:.6}) rapier_ang=({:.6},{:.6},{:.6})",
                        frame + 1,
                        pos_err,
                        lin_err,
                        ang_err,
                        history_body.pose.position.x,
                        history_body.pose.position.y,
                        history_body.pose.position.z,
                        history_body.linear_velocity.x,
                        history_body.linear_velocity.y,
                        history_body.linear_velocity.z,
                        history_body.angular_velocity.x,
                        history_body.angular_velocity.y,
                        history_body.angular_velocity.z,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.x,
                        rapier_state.2.y,
                        rapier_state.2.z,
                        rapier_state.3.x,
                        rapier_state.3.y,
                        rapier_state.3.z,
                    );
                }
            }
            if frame + 1 == 50 {
                if let Some((position, orientation, linvel, angvel)) = rapier_frame49 {
                    let (our_pos, our_lin, our_ang) =
                        step_our_from_state(position, orientation, linvel, angvel);
                    println!(
                        "single_step_49_to_50 our_pos=({:.3},{:.3},{:.3}) our_lin={:.4} our_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                        our_pos.x,
                        our_pos.y,
                        our_pos.z,
                        our_lin,
                        our_ang,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.length(),
                        rapier_state.3.length(),
                    );
                    println!(
                        "history_step_49_to_50 our_pos=({:.3},{:.3},{:.3}) our_lin={:.4} our_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                        history_probe_pos.x,
                        history_probe_pos.y,
                        history_probe_pos.z,
                        history_probe_lin,
                        history_probe_ang,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.length(),
                        rapier_state.3.length(),
                    );
                }
            }
            if frame + 1 == 43 {
                if let Some((position, orientation, linvel, angvel)) = rapier_frame42 {
                    let (our_pos, our_lin, our_ang) =
                        step_our_from_state(position, orientation, linvel, angvel);
                    println!(
                        "single_step_42_to_43 our_pos=({:.3},{:.3},{:.3}) our_lin={:.4} our_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                        our_pos.x,
                        our_pos.y,
                        our_pos.z,
                        our_lin,
                        our_ang,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.length(),
                        rapier_state.3.length(),
                    );
                    println!(
                        "history_step_42_to_43 our_pos=({:.3},{:.3},{:.3}) our_lin={:.4} our_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                        history_probe_pos.x,
                        history_probe_pos.y,
                        history_probe_pos.z,
                        history_probe_lin,
                        history_probe_ang,
                        rapier_state.0.x,
                        rapier_state.0.y,
                        rapier_state.0.z,
                        rapier_state.2.length(),
                        rapier_state.3.length(),
                    );
                }
            }

            if frame % 60 == 59 {
                let our_body = &our_world.bodies[0];
                let rapier_body = bodies.get(cube).unwrap();
                println!(
                    "frame={} ours_pos=({:.3},{:.3},{:.3}) ours_lin={:.4} ours_ang={:.4} rapier_pos=({:.3},{:.3},{:.3}) rapier_lin={:.4} rapier_ang={:.4}",
                    frame + 1,
                    our_body.pose.position.x,
                    our_body.pose.position.y,
                    our_body.pose.position.z,
                    our_body.linear_velocity.length(),
                    our_body.angular_velocity.length(),
                    rapier_body.translation().x,
                    rapier_body.translation().y,
                    rapier_body.translation().z,
                    rapier_body.linvel().length(),
                    rapier_body.angvel().length(),
                );
            }
        }
        println!(
            "history_probe_max_error frame={} pos_err={:.6} lin_err={:.6} ang_err={:.6}",
            max_history_frame,
            max_history_pos_err,
            max_history_lin_err,
            max_history_ang_err,
        );
    }

    // ---- Determinism tests ----

    fn run_simulation(num_cubes_per_axis: usize, frames: usize) -> Vec<u64> {
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);

        let mut spawn_ops = Vec::new();
        for x in 0..num_cubes_per_axis {
            for y in 0..num_cubes_per_axis {
                for z in 0..num_cubes_per_axis {
                    spawn_ops.push(PhysicsOp::SpawnDynamic {
                        position: vec3f(x as f32 * 1.2, 2.0 + y as f32 * 1.2, z as f32 * 1.2),
                        half_extents: vec3f(0.5, 0.5, 0.5),
                        velocity: Vec3f::default(),
                        density: 1.0,
                    });
                }
            }
        }
        world.step(&spawn_ops);

        let mut hashes = Vec::with_capacity(frames);
        for _ in 0..frames {
            world.step(&[]);
            hashes.push(world.hash_state());
        }
        hashes
    }

    #[test]
    fn test_determinism_small() {
        let hashes_a = run_simulation(2, 300);
        let hashes_b = run_simulation(2, 300);
        assert_eq!(hashes_a.len(), hashes_b.len());
        for (frame, (ha, hb)) in hashes_a.iter().zip(hashes_b.iter()).enumerate() {
            assert_eq!(
                ha, hb,
                "Determinism failure at frame {}: {:016x} vs {:016x}",
                frame, ha, hb
            );
        }
    }

    #[test]
    fn test_determinism_large() {
        let hashes_a = run_simulation(3, 300);
        let hashes_b = run_simulation(3, 300);
        for (frame, (ha, hb)) in hashes_a.iter().zip(hashes_b.iter()).enumerate() {
            assert_eq!(
                ha, hb,
                "Determinism failure at frame {}: {:016x} vs {:016x}",
                frame, ha, hb
            );
        }
    }

    #[test]
    fn test_determinism_with_ops() {
        fn run_with_ops() -> Vec<u64> {
            let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
            world.step(&[PhysicsOp::SpawnDynamic {
                position: vec3f(0.0, 3.0, 0.0),
                half_extents: vec3f(0.5, 0.5, 0.5),
                velocity: Vec3f::default(),
                density: 1.0,
            }]);

            let mut hashes = Vec::new();
            for i in 0..200 {
                let ops = if i == 50 {
                    vec![PhysicsOp::SpawnDynamic {
                        position: vec3f(0.0, 5.0, 0.0),
                        half_extents: vec3f(0.5, 0.5, 0.5),
                        velocity: vec3f(0.5, 0.0, 0.0),
                        density: 1.0,
                    }]
                } else {
                    vec![]
                };
                world.step(&ops);
                hashes.push(world.hash_state());
            }
            hashes
        }

        let a = run_with_ops();
        let b = run_with_ops();
        for (frame, (ha, hb)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(ha, hb, "Op determinism failure at frame {}", frame);
        }
    }

    // ---- Snapshot + resync tests ----

    #[test]
    fn test_snapshot_restore_replay() {
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        world.step(&[PhysicsOp::SpawnDynamic {
            position: vec3f(0.0, 5.0, 0.0),
            half_extents: vec3f(0.5, 0.5, 0.5),
            velocity: Vec3f::default(),
            density: 1.0,
        }]);

        // Run to frame 100 and snapshot
        for _ in 0..100 {
            world.step(&[]);
        }
        let snap = world.snapshot();
        let hash_at_100 = world.hash_state();

        // Continue to frame 200
        for _ in 0..100 {
            world.step(&[]);
        }
        let hash_at_200 = world.hash_state();

        // Restore snapshot and replay to frame 200
        world.restore(&snap);
        assert_eq!(
            world.hash_state(),
            hash_at_100,
            "Hash should match after restore"
        );
        for _ in 0..100 {
            world.step(&[]);
        }
        assert_eq!(
            world.hash_state(),
            hash_at_200,
            "Hash should match after replay"
        );
    }

    #[test]
    fn test_resync_matches_clean_run() {
        // Do a clean run
        let mut clean = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        clean.step(&[PhysicsOp::SpawnDynamic {
            position: vec3f(0.0, 5.0, 0.0),
            half_extents: vec3f(0.5, 0.5, 0.5),
            velocity: Vec3f::default(),
            density: 1.0,
        }]);
        for _ in 0..200 {
            clean.step(&[]);
        }
        let clean_hash = clean.hash_state();

        // Do a run, snapshot at 50, then mess things up, then resync
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);
        world.step(&[PhysicsOp::SpawnDynamic {
            position: vec3f(0.0, 5.0, 0.0),
            half_extents: vec3f(0.5, 0.5, 0.5),
            velocity: Vec3f::default(),
            density: 1.0,
        }]);
        for _ in 0..50 {
            world.step(&[]);
        }
        let snap = world.snapshot();

        // Diverge: apply a bogus force
        world.step(&[PhysicsOp::ApplyImpulse {
            body: 0,
            impulse: vec3f(100.0, 0.0, 0.0),
        }]);
        for _ in 0..50 {
            world.step(&[]);
        }
        // State is now wrong — resync
        let op_log: Vec<(u64, &[PhysicsOp])> = (snap.frame..snap.frame + 150)
            .map(|f| (f, [].as_slice()))
            .collect();
        world.resync(&snap, &op_log);
        assert_eq!(
            world.hash_state(),
            clean_hash,
            "Resync should match clean run"
        );
    }
}

// Kept for reference only. This module depends on rapier3d and is intentionally
// disabled so the crate stays self-contained/committable without local rapier.
#[cfg(any())]
mod rapier_comparison {
    use makepad_math::*;
    // Use our types explicitly to avoid name clashes with rapier
    use crate::rigid_body::RigidBody as OurRigidBody;
    use crate::world::PhysicsWorld;
    use rapier3d::prelude::*;

    fn rvec(x: f32, y: f32, z: f32) -> rapier3d::math::Vector {
        rapier3d::math::Vector::new(x, y, z)
    }

    /// Run a single cube drop in rapier and our engine, compare positions per-frame.
    /// Both use identical parameters: gravity=-9.81, dt=1/60, density=1.0,
    /// friction=0.5, restitution=0.0, half_extents=0.5, start_y=5.0.
    #[test]
    fn compare_single_cube_drop() {
        let num_frames = 300;
        let dt = 1.0 / 60.0;
        let grav = -9.81f32;
        let half_ext = 0.5f32;
        let start_y = 5.0f32;
        let density = 1.0f32;
        let friction = 0.5f32;
        let restitution = 0.0f32;

        // ---- Rapier simulation ----
        let rapier_positions = {
            let mut bodies = RigidBodySet::new();
            let mut colliders = ColliderSet::new();
            let mut impulse_joints = ImpulseJointSet::new();
            let mut multibody_joints = MultibodyJointSet::new();
            let mut islands = IslandManager::new();
            let mut broad_phase = BroadPhaseBvh::new();
            let mut narrow_phase = NarrowPhase::new();
            let mut ccd_solver = CCDSolver::new();
            let mut pipeline = PhysicsPipeline::new();
            let gravity = rvec(0.0, grav, 0.0);
            let integration_parameters = IntegrationParameters {
                dt,
                ..IntegrationParameters::default()
            };

            // Ground: large thin cuboid at y=-0.1 (top surface at y=0)
            let ground_body = RigidBodyBuilder::fixed().translation(rvec(0.0, -0.1, 0.0));
            let ground_handle = bodies.insert(ground_body);
            let ground_collider = ColliderBuilder::cuboid(100.0, 0.1, 100.0)
                .friction(friction)
                .restitution(restitution);
            colliders.insert_with_parent(ground_collider, ground_handle, &mut bodies);

            // Dynamic cube
            let cube_body = RigidBodyBuilder::dynamic()
                .translation(rvec(0.0, start_y, 0.0))
                .can_sleep(false);
            let cube_handle = bodies.insert(cube_body);
            let cube_collider = ColliderBuilder::cuboid(half_ext, half_ext, half_ext)
                .density(density)
                .friction(friction)
                .restitution(restitution);
            colliders.insert_with_parent(cube_collider, cube_handle, &mut bodies);

            let mut positions = Vec::with_capacity(num_frames);
            for _ in 0..num_frames {
                pipeline.step(
                    gravity,
                    &integration_parameters,
                    &mut islands,
                    &mut broad_phase,
                    &mut narrow_phase,
                    &mut bodies,
                    &mut colliders,
                    &mut impulse_joints,
                    &mut multibody_joints,
                    &mut ccd_solver,
                    &(),
                    &(),
                );
                let body = bodies.get(cube_handle).unwrap();
                let pos = body.translation();
                let vel = body.linvel();
                positions.push((pos.y, vel.y));
            }
            positions
        };

        // ---- Our engine simulation ----
        let our_positions = {
            let mut world = PhysicsWorld::new(vec3f(0.0, grav, 0.0), dt);
            let mut body = OurRigidBody::new_dynamic(
                vec3f(0.0, start_y, 0.0),
                vec3f(half_ext, half_ext, half_ext),
                density,
            );
            body.restitution = restitution;
            body.friction = friction;
            world.bodies.push(body);

            let mut positions = Vec::with_capacity(num_frames);
            for _ in 0..num_frames {
                world.step(&[]);
                let b = &world.bodies[0];
                positions.push((b.pose.position.y, b.linear_velocity.y));
            }
            positions
        };

        // ---- Compare ----
        eprintln!(
            "\n{:>5}  {:>10} {:>10}  {:>10} {:>10}  {:>8} {:>8}",
            "frame", "rapier_y", "rapier_vy", "ours_y", "ours_vy", "dy", "dvy"
        );
        let mut max_dy = 0.0f32;
        let mut max_dvy = 0.0f32;
        for (frame, ((ry, rvy), (oy, ovy))) in rapier_positions
            .iter()
            .zip(our_positions.iter())
            .enumerate()
        {
            let dy = (ry - oy).abs();
            let dvy = (rvy - ovy).abs();
            max_dy = max_dy.max(dy);
            max_dvy = max_dvy.max(dvy);

            if frame < 10 || frame % 30 == 0 || dy > 0.1 {
                eprintln!(
                    "{:5}  {:10.4} {:10.4}  {:10.4} {:10.4}  {:8.4} {:8.4}",
                    frame, ry, rvy, oy, ovy, dy, dvy
                );
            }
        }
        eprintln!("\nmax position error: {:.6}", max_dy);
        eprintln!("max velocity error: {:.6}", max_dvy);

        // During free-fall (before ground contact), should be very close
        let final_ry = rapier_positions.last().unwrap().0;
        let final_oy = our_positions.last().unwrap().0;
        assert!(
            (final_ry - final_oy).abs() < 0.5,
            "Final positions diverged too much: rapier={:.4} ours={:.4}",
            final_ry,
            final_oy,
        );

        // Free-fall phase (first ~40 frames before hitting ground)
        for frame in 0..40 {
            let (ry, _) = rapier_positions[frame];
            let (oy, _) = our_positions[frame];
            assert!(
                (ry - oy).abs() < 0.01,
                "Free-fall divergence at frame {}: rapier={:.6} ours={:.6}",
                frame,
                ry,
                oy,
            );
        }
    }

    /// Diagnostic: check if cube-cube collisions are actually detected
    #[test]
    fn debug_cube_cube_collision() {
        use crate::broad_phase;
        use crate::narrow_phase;

        // Two cubes: one at y=0.5, one at y=1.4 (overlapping by 0.1)
        let bodies = vec![
            OurRigidBody::new_dynamic(vec3f(0.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5), 1.0),
            OurRigidBody::new_dynamic(vec3f(0.0, 1.4, 0.0), vec3f(0.5, 0.5, 0.5), 1.0),
        ];

        let mut aabbs = Vec::new();
        let mut pairs = Vec::new();
        broad_phase::broad_phase(&bodies, &mut aabbs, &mut pairs);
        eprintln!("AABBs: {:?}", aabbs);
        eprintln!("Pairs: {:?}", pairs);

        let mut manifolds = Vec::new();
        narrow_phase::narrow_phase(&bodies, &pairs, 0.0, &mut manifolds);
        eprintln!("Manifolds: {}", manifolds.len());
        for m in &manifolds {
            eprintln!(
                "  body_a={} body_b={} points={}",
                m.body_a, m.body_b, m.num_points
            );
            for pi in 0..m.num_points {
                let p = &m.points[pi];
                eprintln!(
                    "    world={:.3},{:.3},{:.3} pen={:.4} normal={:.3},{:.3},{:.3}",
                    p.world_point.x,
                    p.world_point.y,
                    p.world_point.z,
                    p.penetration,
                    p.normal.x,
                    p.normal.y,
                    p.normal.z,
                );
            }
        }

        let body_body = manifolds
            .iter()
            .find(|m| m.body_a != usize::MAX && m.body_b != usize::MAX);
        assert!(
            body_body.is_some(),
            "Should have body-body contacts between overlapping cubes"
        );
        let m = body_body.unwrap();
        assert!(m.num_points > 0, "Manifold should have contact points");

        // Manual solver debug: 2 cubes touching, upper has downward velocity
        use crate::solver;

        let mut dbg_bodies = vec![
            OurRigidBody::new_dynamic(vec3f(0.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5), 1.0),
            OurRigidBody::new_dynamic(vec3f(0.0, 1.5, 0.0), vec3f(0.5, 0.5, 0.5), 1.0),
        ];
        dbg_bodies[1].linear_velocity = vec3f(0.0, -0.5, 0.0);

        let substep_dt = (1.0f32 / 60.0) / 4.0;
        let mut dbg_aabbs = Vec::new();
        let mut dbg_pairs = Vec::new();
        broad_phase::broad_phase(&dbg_bodies, &mut dbg_aabbs, &mut dbg_pairs);
        let mut dbg_manifolds = Vec::new();
        narrow_phase::narrow_phase(&dbg_bodies, &dbg_pairs, 0.0, &mut dbg_manifolds);

        eprintln!("\n--- Manual solver debug ---");
        for dm in &dbg_manifolds {
            let bb = dm.body_a != usize::MAX && dm.body_b != usize::MAX;
            eprintln!(
                "manifold a={} b={} pts={} body_body={}",
                dm.body_a, dm.body_b, dm.num_points, bb
            );
            for pi in 0..dm.num_points {
                let p = &dm.points[pi];
                eprintln!(
                    "  pen={:.4} n=({:.3},{:.3},{:.3})",
                    p.penetration, p.normal.x, p.normal.y, p.normal.z
                );
            }
        }

        let mut dbg_sc = Vec::new();
        solver::prepare_contacts(&dbg_bodies, &dbg_manifolds, substep_dt, &mut dbg_sc);
        eprintln!("\nsolver_contacts: {}", dbg_sc.len());
        for (i, sc) in dbg_sc.iter().enumerate() {
            let bb = sc.body_a != usize::MAX && sc.body_b != usize::MAX;
            if bb {
                eprintln!(
                    "  sc[{}] body_body: dir1=({:.3},{:.3},{:.3}) r_n={:.6} rhs={:.6} dist={:.6}",
                    i, sc.dir1.x, sc.dir1.y, sc.dir1.z, sc.r_normal, sc.rhs, sc.dist
                );
            }
        }

        eprintln!(
            "\nbefore: v0y={:.4} v1y={:.4}",
            dbg_bodies[0].linear_velocity.y, dbg_bodies[1].linear_velocity.y
        );
        solver::solve_contacts(&mut dbg_bodies, &mut dbg_sc, 1);
        eprintln!(
            "after:  v0y={:.4} v1y={:.4}",
            dbg_bodies[0].linear_velocity.y, dbg_bodies[1].linear_velocity.y
        );
        for (i, sc) in dbg_sc.iter().enumerate() {
            if sc.body_a != usize::MAX && sc.body_b != usize::MAX {
                eprintln!("  sc[{}] impulse_n={:.6}", i, sc.impulse_normal);
            }
        }

        assert!(
            dbg_bodies[1].linear_velocity.y > -0.3,
            "Solver should reduce downward vel, got vy={:.4}",
            dbg_bodies[1].linear_velocity.y,
        );
    }

    /// Compare a stack of cubes — the main stress test for solver quality.
    #[test]
    fn compare_cube_stack() {
        let num_frames = 300;
        let dt = 1.0 / 60.0;
        let grav = -9.81f32;
        let half_ext = 0.5f32;
        let density = 1.0f32;
        let friction = 0.5f32;
        let restitution = 0.0f32;
        let num_cubes = 3;

        // ---- Rapier ----
        let rapier_final: Vec<f32> = {
            let mut bodies = RigidBodySet::new();
            let mut colliders = ColliderSet::new();
            let mut impulse_joints = ImpulseJointSet::new();
            let mut multibody_joints = MultibodyJointSet::new();
            let mut islands = IslandManager::new();
            let mut broad_phase = BroadPhaseBvh::new();
            let mut narrow_phase = NarrowPhase::new();
            let mut ccd_solver = CCDSolver::new();
            let mut pipeline = PhysicsPipeline::new();
            let gravity = rvec(0.0, grav, 0.0);
            let integration_parameters = IntegrationParameters {
                dt,
                ..IntegrationParameters::default()
            };

            // Ground
            let ground_body = RigidBodyBuilder::fixed().translation(rvec(0.0, -0.1, 0.0));
            let gh = bodies.insert(ground_body);
            let gc = ColliderBuilder::cuboid(100.0, 0.1, 100.0)
                .friction(friction)
                .restitution(restitution);
            colliders.insert_with_parent(gc, gh, &mut bodies);

            // Stack of cubes
            let mut handles = Vec::new();
            for i in 0..num_cubes {
                let y = half_ext + i as f32 * (2.0 * half_ext);
                let rb = RigidBodyBuilder::dynamic()
                    .translation(rvec(0.0, y, 0.0))
                    .can_sleep(false);
                let h = bodies.insert(rb);
                let c = ColliderBuilder::cuboid(half_ext, half_ext, half_ext)
                    .density(density)
                    .friction(friction)
                    .restitution(restitution);
                colliders.insert_with_parent(c, h, &mut bodies);
                handles.push(h);
            }

            for _ in 0..num_frames {
                pipeline.step(
                    gravity,
                    &integration_parameters,
                    &mut islands,
                    &mut broad_phase,
                    &mut narrow_phase,
                    &mut bodies,
                    &mut colliders,
                    &mut impulse_joints,
                    &mut multibody_joints,
                    &mut ccd_solver,
                    &(),
                    &(),
                );
            }

            handles
                .iter()
                .map(|h| bodies.get(*h).unwrap().translation().y)
                .collect()
        };

        // ---- Ours ----
        let our_final: Vec<f32> = {
            let mut world = PhysicsWorld::new(vec3f(0.0, grav, 0.0), dt);
            for i in 0..num_cubes {
                let y = half_ext + i as f32 * (2.0 * half_ext);
                let mut body = OurRigidBody::new_dynamic(
                    vec3f(0.0, y, 0.0),
                    vec3f(half_ext, half_ext, half_ext),
                    density,
                );
                body.restitution = restitution;
                body.friction = friction;
                world.bodies.push(body);
            }
            for _ in 0..num_frames {
                world.step(&[]);
            }
            world.bodies.iter().map(|b| b.pose.position.y).collect()
        };

        // ---- Compare ----
        eprintln!("\nCube stack final positions after {} frames:", num_frames);
        for i in 0..num_cubes {
            eprintln!(
                "  cube {}: rapier={:.4}  ours={:.4}  diff={:.4}",
                i,
                rapier_final[i],
                our_final[i],
                (rapier_final[i] - our_final[i]).abs()
            );
        }

        // Each cube should be stacked correctly above ground
        for i in 0..num_cubes {
            assert!(our_final[i] > 0.0, "Our cube {} fell through ground", i);
            assert!(
                rapier_final[i] > 0.0,
                "Rapier cube {} fell through ground",
                i
            );
        }

        // Final positions should be qualitatively similar
        for i in 0..num_cubes {
            let diff = (rapier_final[i] - our_final[i]).abs();
            assert!(
                diff < 1.0,
                "Cube {} positions diverged: rapier={:.4} ours={:.4} diff={:.4}",
                i,
                rapier_final[i],
                our_final[i],
                diff
            );
        }
    }
}
