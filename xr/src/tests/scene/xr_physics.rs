mod tests {
    use super::*;
    use super::super::xr_depth::sync_depth_query_surfaces_with_store;
    use crate::algorithms::tsdf_query::{
        DepthQueryCollider, DepthQueryColliderGeometry, DepthQuerySupportPlane,
    };
    use makepad_widgets::makepad_platform::{
        ChunkKey, SparseTsdGridReadSnapshot, SparseTsdReadChunk, TsdfPublishedSnapshot,
        XrTsdfStore,
    };
    use std::{collections::HashMap, sync::Arc};

    fn set_normalized_distance(
        chunks: &mut HashMap<ChunkKey, Arc<SparseTsdReadChunk>>,
        chunk_edge: i32,
        x: i32,
        y: i32,
        z: i32,
        normalized_distance: f32,
    ) {
        let cx = x.div_euclid(chunk_edge);
        let cy = y.div_euclid(chunk_edge);
        let cz = z.div_euclid(chunk_edge);
        let lx = x.rem_euclid(chunk_edge) as usize;
        let ly = y.rem_euclid(chunk_edge) as usize;
        let lz = z.rem_euclid(chunk_edge) as usize;
        let edge = chunk_edge as usize;
        let id = lx + ly * edge + lz * edge * edge;
        let chunk = Arc::make_mut(
            chunks
                .entry(ChunkKey::new(cx, cy, cz))
                .or_insert_with(|| Arc::new(SparseTsdReadChunk::new(edge * edge * edge))),
        );
        chunk.set_value(id, normalized_distance, 8, 1);
    }

    fn make_flat_floor_snapshot(voxel_size: f32) -> TsdfPublishedSnapshot {
        let chunk_edge = 8;
        let mut chunks = HashMap::new();
        let tsd_distance_meters = voxel_size * 2.0;
        let mut active_value_count = 0usize;
        for z in -6..=6 {
            for y in -6..=6 {
                for x in -6..=6 {
                    let world_y = (y as f32 + 0.5) * voxel_size;
                    let normalized = (world_y / tsd_distance_meters).clamp(-1.0, 1.0);
                    set_normalized_distance(
                        &mut chunks,
                        chunk_edge,
                        x,
                        y,
                        z,
                        normalized,
                    );
                    active_value_count += 1;
                }
            }
        }
        TsdfPublishedSnapshot {
            generation: 1,
            latest_topology_generation: 1,
            update_sequence: 1,
            grid: Arc::new(SparseTsdGridReadSnapshot {
                voxel_size,
                chunk_edge,
                chunk_edge_shift: Some(chunk_edge.trailing_zeros() as u8),
                chunk_edge_mask: chunk_edge - 1,
                chunk_volume: (chunk_edge as usize).pow(3),
                active_value_count,
                active_bounds: Some((
                    vec3f(-6.0 * voxel_size, -6.0 * voxel_size, -6.0 * voxel_size),
                    vec3f(7.0 * voxel_size, 7.0 * voxel_size, 7.0 * voxel_size),
                )),
                chunks,
            }),
            height_map: None,
        }
    }

    fn assert_vec3_close(actual: Vec3f, expected: Vec3f, tolerance: f32) {
        assert!(
            (actual - expected).length() <= tolerance,
            "expected {:?} to be within {} of {:?}",
            actual,
            tolerance,
            expected
        );
    }

    #[test]
    fn impact_surface_enables_before_current_body_overlaps_quad() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(1.0, 0.0, 0.0),
            normal: vec3f(-1.0, 0.0, 0.0),
            tangent: vec3f(0.0, 1.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.08,
            half_extent_bitangent: 0.08,
        };
        let target = DepthQuerySurfaceTarget {
            collider: DepthQueryCollider {
                fingerprint: 1,
                geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                role: DepthQueryColliderRole::Impact,
                restitution: 0.38,
            },
        };

        assert!(depth_query_surface_target_should_enable(
            target,
            vec3f(0.78, 0.0, 0.0),
            vec3f(0.55, 0.0, 0.0),
            0.05,
            0.004,
        ));
    }

    #[test]
    fn respawn_body_applies_shadow_contact_dominated_and_sleeping_modes() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(41);
        let pose = Pose::new(Quat::default(), vec3f(0.08, 1.12, -0.44));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.04, 0.04, 0.04),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(1.0, 2.0, 3.0),
            vec3f(4.0, 5.0, 6.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("spawned cube body should exist");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.shadow_body_motion_for_body(cube.body),
            Some((vec3f(1.0, 2.0, 3.0), vec3f(4.0, 5.0, 6.0)))
        );

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::ContactDominated {
                authority: XrNetPeerId(7),
                hand: XrSharedHand::RightHand,
            },
            pose,
            vec3f(0.5, 0.0, -0.5),
            vec3f(0.0, 1.0, 0.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after contact-dominated respawn");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert!(!body.is_sleeping());

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::Sleeping,
            pose,
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.0, 0.0, 0.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after sleeping respawn");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
        assert_vec3_close(
            vec3f(body.linvel().x, body.linvel().y, body.linvel().z),
            vec3f(0.0, 0.0, 0.0),
            0.0001,
        );
        assert_vec3_close(
            vec3f(body.angvel().x, body.angvel().y, body.angvel().z),
            vec3f(0.0, 0.0, 0.0),
            0.0001,
        );
    }

    #[test]
    fn shadow_respawn_keeps_projecting_motion_between_corrections() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(410);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.2));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -1.5),
            vec3f(0.0, 0.0, 0.0),
        );
        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("shadow body should exist after a step");
        let stepped_pose = makepad_pose(body.position());
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert!(
            stepped_pose.position.z < pose.position.z - 0.0001,
            "shadow body should keep moving forward between network corrections: {stepped_pose:?}"
        );
        assert_eq!(
            scene
                .shadow_body_motion_for_body(cube.body)
                .map(|(linvel, _)| linvel)
                .unwrap_or_default(),
            vec3f(0.0, 0.0, -1.5)
        );
    }

    #[test]
    fn shadow_dynamic_body_ignores_local_wall_until_extrapolation_runs_out() {
        let mut scene = RapierScene::new(0.0);
        let projectile_uid = WidgetUid(411);
        let wall_uid = WidgetUid(412);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.2));
        scene.spawn_dynamic_sphere(
            projectile_uid,
            pose,
            0.05,
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
            0.95,
        );
        scene.spawn_fixed_box(
            wall_uid,
            Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.55)),
            vec3f(1.0, 1.0, 0.05),
            vec3f(1.0, 1.0, 1.0),
            0.0,
            0.95,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            projectile_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -2.0),
            vec3f(0.0, 0.0, 0.0),
        );
        for _ in 0..30 {
            scene.step();
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("shadow projectile should still exist after stepping");
        assert!(
            makepad_pose(body.position()).position.z <= -0.39,
            "shadow projectile should keep dead-reckoning forward instead of locally bouncing on observer-only walls: {:?}",
            makepad_pose(body.position()).position
        );
        assert!(
            makepad_pose(body.position()).position.z >= -0.45,
            "shadow projectile should stop near the extrapolation horizon when no fresh authority samples arrive: {:?}",
            makepad_pose(body.position()).position
        );
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
    }

    #[test]
    fn apply_impulse_only_affects_dynamic_enabled_bodies() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(42);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        assert!(scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0),));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after impulse");
        assert!(body.linvel().z < 0.0);

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.0, 0.0, 0.0),
        );
        assert!(
            !scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0)),
            "shadow bodies should reject dynamic impulses"
        );

        scene.despawn_body(widget_uid);
        assert!(
            !scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0)),
            "disabled bodies should reject impulses"
        );
    }

    #[test]
    fn tsdf_floor_halfspace_catches_falling_body() {
        let mut scene = RapierScene::new(9.81);
        scene.sync_floor_halfspace(Some(-0.25));
        let widget_uid = WidgetUid(4199);
        scene.spawn_dynamic_box(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.20, 0.0)),
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.9,
            0.0,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("dynamic body should exist");

        for _ in 0..180 {
            scene.step();
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("body should still exist after settling on the TSDF floor");
        let position = makepad_pose(body.position()).position;
        assert!(
            (position.y - (-0.20)).abs() <= 0.04,
            "body should settle on the injected floor half-space near y=-0.20, got {position:?}"
        );
    }

    #[test]
    fn apply_drive_moves_a_resting_supported_body() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 480.0);

        scene.spawn_fixed_box(
            WidgetUid(4200),
            Pose::new(Quat::default(), vec3f(0.0, -0.05, 0.0)),
            vec3f(2.0, 0.05, 2.0),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
        );

        let widget_uid = WidgetUid(4201);
        scene.spawn_dynamic_box(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.05, 0.0)),
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            850.0,
            1.0,
            0.0,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("dynamic test body should exist");

        for _ in 0..8 {
            scene.step();
        }

        let mut most_negative_z = 0.0_f32;
        let mut most_negative_position_z = 0.0_f32;
        for _ in 0..60 {
            scene.step();
            assert!(
                scene.apply_drive(
                    widget_uid,
                    vec3f(0.0, 0.0, -0.8),
                    vec3f(0.0, 0.0, 0.0),
                    2.8,
                    8.0,
                    true,
                    scene.simulation_dt(),
                ),
                "drive commands should be accepted for a supported dynamic body"
            );
            let body = scene
                .bodies
                .get(cube.body)
                .expect("driven body should still exist");
            most_negative_z = most_negative_z.min(body.linvel().z);
            most_negative_position_z =
                most_negative_position_z.min(makepad_pose(body.position()).position.z);
        }
        scene.step();
        let body = scene
            .bodies
            .get(cube.body)
            .expect("driven body should still exist after the final integration step");
        most_negative_position_z =
            most_negative_position_z.min(makepad_pose(body.position()).position.z);

        assert!(
            most_negative_z < -0.03,
            "a driven resting body should accumulate forward speed instead of getting stuck at rest; best z velocity was {most_negative_z}"
        );
        assert!(
            most_negative_position_z < -0.01,
            "a driven resting body should translate across the floor once post-step drive velocities are applied; best z position was {most_negative_position_z}"
        );
    }

    #[test]
    fn four_wheel_vehicle_creates_four_support_query_sources() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(4202);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.20, 0.0)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );

        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel cube should exist");
        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("four-wheel cube should have a vehicle controller");
        let support_count = cube.linked_support_bodies.iter().flatten().count();
        let query_source_count = scene
            .cube_depth_query_sources(cube)
            .into_iter()
            .flatten()
            .count();

        assert_eq!(
            support_count, 4,
            "four-wheel cars should spawn four wheel markers"
        );
        assert_eq!(
            scene.vehicles[vehicle_index].wheels.len(),
            4,
            "vehicle controller should be configured with four wheels"
        );
        assert_eq!(
            query_source_count, 4,
            "four-wheel cars should expose one TSDF query source per wheel"
        );
    }

    #[test]
    fn four_wheel_vehicle_stays_supported_by_depth_query_floor_planes() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);

        let widget_uid = WidgetUid(42021);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.18, 0.0)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("four-wheel cube should have a vehicle controller");

        for frame in 0..180 {
            scene.sync_vehicle_query_sources_pre_step();
            let query_sources = scene.cube_depth_query_sources(cube);
            for (slot, source) in query_sources.into_iter().flatten().enumerate() {
                let Some(body) = scene.bodies.get(source.body) else {
                    continue;
                };
                let query_pose = makepad_pose(body.position());
                let plane = DepthQuerySupportPlane {
                    point: vec3f(query_pose.position.x, 0.0, query_pose.position.z),
                    normal: vec3f(0.0, 1.0, 0.0),
                    tangent: vec3f(1.0, 0.0, 0.0),
                    bitangent: vec3f(0.0, 0.0, 1.0),
                    half_extent_tangent: (source.query_radius * 2.0).max(0.24),
                    half_extent_bitangent: (source.query_radius * 2.0).max(0.24),
                };
                scene.sync_depth_query_surface_set(
                    source.set_index,
                    &[
                        Some(DepthQuerySurfaceTarget {
                            collider: DepthQueryCollider {
                                fingerprint: 10_000 + slot as u64,
                                geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                                role: DepthQueryColliderRole::Support,
                                restitution: 0.0,
                            },
                        }),
                        None,
                    ],
                );
            }
            scene.step();

            let body = scene
                .bodies
                .get(cube.body)
                .expect("four-wheel body should still exist during depth support test");
            let position = makepad_pose(body.position()).position;
            assert!(
                position.y > -0.18,
                "depth-query wheel support should stop the chassis from falling through the floor plane; frame={frame} position={position:?}"
            );
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("four-wheel body should still exist after depth support test");
        let position = makepad_pose(body.position()).position;
        let wheel_contact_count = scene.vehicles[vehicle_index]
            .controller
            .wheels()
            .iter()
            .filter(|wheel| wheel.raycast_info().is_in_contact)
            .count();
        assert!(
            position.y > -0.05,
            "depth-query wheel support should keep the chassis near the injected floor planes: {position:?}"
        );
        assert!(
            wheel_contact_count >= 2,
            "depth-query wheel support should leave multiple wheels in contact, got {wheel_contact_count}"
        );
    }

    #[test]
    fn four_wheel_vehicle_stays_supported_by_flat_tsdf_floor() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);
        let mut retained_hits = HashMap::new();
        let depth_mesh = XrTsdfStore::default();
        depth_mesh.publish_tsdf_snapshot(make_flat_floor_snapshot(0.05));

        let widget_uid = WidgetUid(42022);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.18, 0.0)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("four-wheel cube should have a vehicle controller");

        for frame in 0..180 {
            scene.sync_vehicle_query_sources_pre_step();
            sync_depth_query_surfaces_with_store(&mut retained_hits, Some(&mut scene), &depth_mesh);
            scene.step();
            let body = scene
                .bodies
                .get(cube.body)
                .expect("four-wheel body should still exist during TSDF support test");
            let position = makepad_pose(body.position()).position;
            assert!(
                position.y > -0.18,
                "flat-floor TSDF support should stop the chassis from falling through; frame={frame} position={position:?}"
            );
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("four-wheel body should still exist after TSDF support test");
        let position = makepad_pose(body.position()).position;
        let wheel_contact_count = scene.vehicles[vehicle_index]
            .controller
            .wheels()
            .iter()
            .filter(|wheel| wheel.raycast_info().is_in_contact)
            .count();
        assert!(
            position.y > -0.05,
            "flat-floor TSDF support should keep the chassis near the floor: {position:?}"
        );
        assert!(
            wheel_contact_count >= 2,
            "flat-floor TSDF support should leave multiple wheels in contact, got {wheel_contact_count}"
        );
    }

    #[test]
    fn four_wheel_vehicle_body_depth_query_planes_provide_hybrid_catch_support() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);

        let widget_uid = WidgetUid(42023);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.18, 0.0)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        assert!(
            scene.cube_depth_query_sources(cube)[0].is_some(),
            "four-wheel vehicles should expose a chassis depth-query source for the hybrid catch path"
        );

        for frame in 0..180 {
            scene.sync_vehicle_query_sources_pre_step();
            let query_sources = scene.cube_depth_query_sources(cube);
            for (slot, source) in query_sources.into_iter().enumerate() {
                let Some(source) = source else {
                    continue;
                };
                if slot == 0 {
                    let Some(body) = scene.bodies.get(source.body) else {
                        continue;
                    };
                    let query_pose = makepad_pose(body.position());
                    let plane = DepthQuerySupportPlane {
                        point: vec3f(query_pose.position.x, 0.0, query_pose.position.z),
                        normal: vec3f(0.0, 1.0, 0.0),
                        tangent: vec3f(1.0, 0.0, 0.0),
                        bitangent: vec3f(0.0, 0.0, 1.0),
                        half_extent_tangent: (source.query_radius * 2.0).max(0.24),
                        half_extent_bitangent: (source.query_radius * 2.0).max(0.24),
                    };
                    scene.sync_depth_query_surface_set(
                        source.set_index,
                        &[
                            Some(DepthQuerySurfaceTarget {
                                collider: DepthQueryCollider {
                                    fingerprint: 30_000,
                                    geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                                    role: DepthQueryColliderRole::Support,
                                    restitution: 0.0,
                                },
                            }),
                            None,
                        ],
                    );
                } else {
                    scene.sync_depth_query_surface_set(source.set_index, &[None, None]);
                }
            }
            scene.step();

            let body = scene
                .bodies
                .get(cube.body)
                .expect("four-wheel body should still exist during body-catch support test");
            let position = makepad_pose(body.position()).position;
            assert!(
                position.y > -0.18,
                "body depth-query support should stop the chassis from falling through even without wheel support planes; frame={frame} position={position:?}"
            );
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("four-wheel body should still exist after body-catch support test");
        let position = makepad_pose(body.position()).position;
        assert!(
            position.y > -0.05,
            "body depth-query support should keep the chassis near the injected floor plane: {position:?}"
        );
    }

    #[test]
    fn car_control_drives_four_wheel_vehicle_forward() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);
        scene.spawn_fixed_box(
            WidgetUid(4203),
            Pose::new(Quat::default(), vec3f(0.0, -0.05, 0.0)),
            vec3f(3.0, 0.05, 3.0),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
        );

        let widget_uid = WidgetUid(4204);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.16, 0.0)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        let start_position = scene
            .bodies
            .get(cube.body)
            .map(|body| makepad_pose(body.position()).position)
            .expect("four-wheel body should exist at spawn");
        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("four-wheel cube should have a vehicle controller");
        let mut saw_ground_contact = false;

        for _ in 0..90 {
            scene.clear_car_controls();
            assert!(
                scene.apply_car_control(XrCarControl {
                    widget_uid,
                    steer: 0.0,
                    throttle: 1.0,
                    brake: 0.0,
                }),
                "car controls should target the four-wheel vehicle"
            );
            scene.step();
            saw_ground_contact |= scene.vehicles[vehicle_index]
                .controller
                .wheels()
                .iter()
                .any(|wheel| wheel.raycast_info().is_in_contact);
        }

        let end_position = scene
            .bodies
            .get(cube.body)
            .map(|body| makepad_pose(body.position()).position)
            .expect("four-wheel body should still exist after driving");

        assert!(
            end_position.z > start_position.z + 0.02,
            "forward throttle should move the Rapier four-wheel vehicle along +Z: start={start_position:?} end={end_position:?}"
        );
        assert!(
            saw_ground_contact,
            "driven vehicle should establish wheel contact while accelerating across the flat floor"
        );
    }

    #[test]
    fn four_wheel_chassis_keeps_normal_fixed_world_collisions() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);
        scene.spawn_fixed_box(
            WidgetUid(4205),
            Pose::new(Quat::default(), vec3f(0.0, -0.05, 0.0)),
            vec3f(3.0, 0.05, 3.0),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
        );

        let widget_uid = WidgetUid(4206);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(
                Quat::from_axis_angle(vec3f(0.0, 0.0, 1.0), std::f32::consts::FRAC_PI_2),
                vec3f(0.0, 0.02, 0.0),
            ),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");

        for _ in 0..45 {
            scene.step();
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("rolled four-wheel chassis should still exist");
        let position = makepad_pose(body.position()).position;
        assert!(
            position.y > -0.20,
            "rolled four-wheel chassis should still collide with normal fixed bodies instead of falling straight through them: {position:?}"
        );
    }

    #[test]
    fn car_control_steers_four_wheel_vehicle_off_centerline() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);
        scene.spawn_fixed_box(
            WidgetUid(4207),
            Pose::new(Quat::default(), vec3f(0.0, -0.05, 0.0)),
            vec3f(4.0, 0.05, 4.0),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
        );

        let widget_uid = WidgetUid(4208);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.18, 0.0)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("four-wheel cube should have a vehicle controller");
        let mut max_abs_x = 0.0_f32;
        let mut max_abs_forward_x = 0.0_f32;
        let mut saw_front_contact = false;

        for _ in 0..45 {
            scene.clear_car_controls();
            scene.apply_car_control(XrCarControl {
                widget_uid,
                steer: 0.0,
                throttle: 1.0,
                brake: 0.0,
            });
            scene.step();
            let body = scene
                .bodies
                .get(cube.body)
                .expect("four-wheel body should exist during steering warmup");
            let pose = makepad_pose(body.position());
            let forward = pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, 1.0));
            max_abs_x = max_abs_x.max(pose.position.x.abs());
            max_abs_forward_x = max_abs_forward_x.max(forward.x.abs());
        }
        for _ in 0..120 {
            scene.clear_car_controls();
            scene.apply_car_control(XrCarControl {
                widget_uid,
                steer: 0.9,
                throttle: 1.0,
                brake: 0.0,
            });
            scene.step();
            let body = scene
                .bodies
                .get(cube.body)
                .expect("steered four-wheel body should still exist during steering");
            let pose = makepad_pose(body.position());
            let forward = pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, 1.0));
            max_abs_x = max_abs_x.max(pose.position.x.abs());
            max_abs_forward_x = max_abs_forward_x.max(forward.x.abs());
            let wheels = scene.vehicles[vehicle_index].controller.wheels();
            saw_front_contact |= wheels
                .get(0)
                .map(|wheel| wheel.raycast_info().is_in_contact)
                .unwrap_or(false)
                || wheels
                    .get(2)
                    .map(|wheel| wheel.raycast_info().is_in_contact)
                    .unwrap_or(false);
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("steered four-wheel body should still exist");
        let pose = makepad_pose(body.position());
        let forward = pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, 1.0));
        let wheel_summary = scene.vehicles[vehicle_index]
            .controller
            .wheels()
            .iter()
            .enumerate()
            .map(|(index, wheel)| {
                format!(
                    "{index}:contact={} steer={:.3} side={:.3} fwd={:.3} susp={:.3}",
                    wheel.raycast_info().is_in_contact,
                    wheel.steering,
                    wheel.side_impulse,
                    wheel.forward_impulse,
                    wheel.wheel_suspension_force,
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            max_abs_x > 0.05 || max_abs_forward_x > 0.08,
            "steering should move the vehicle off the centerline or rotate its forward axis: pos={:?} forward={:?} max_abs_x={:.3} max_abs_forward_x={:.3} saw_front_contact={} wheels={}",
            pose.position,
            forward
            ,
            max_abs_x,
            max_abs_forward_x,
            saw_front_contact,
            wheel_summary,
        );
    }

    #[test]
    fn four_wheel_vehicle_rides_on_wheels_instead_of_bottoming_out_on_flat_floor() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);
        let floor_uid = WidgetUid(4209);
        scene.spawn_fixed_box(
            floor_uid,
            Pose::new(Quat::default(), vec3f(0.0, -0.05, 0.0)),
            vec3f(4.0, 0.05, 4.0),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
        );

        let widget_uid = WidgetUid(4210);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.18, 0.0)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );

        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        let floor = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == floor_uid)
            .copied()
            .expect("flat floor should exist");
        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("four-wheel cube should have a vehicle controller");

        for _ in 0..90 {
            scene.step();
        }

        let deepest_wheel_bottom_below_floor = scene.vehicles[vehicle_index]
            .controller
            .wheels()
            .iter()
            .filter(|wheel| wheel.raycast_info().is_in_contact)
            .map(|wheel| 0.0 - (wheel.center().y - wheel.radius))
            .fold(0.0, f32::max);
        let front_wheels_in_contact = scene.vehicles[vehicle_index]
            .controller
            .wheels()
            .iter()
            .enumerate()
            .filter(|(index, _)| *index == 0 || *index == 2)
            .all(|(_, wheel)| wheel.raycast_info().is_in_contact);
        let chassis_contact_debug = scene
            .narrow_phase
            .contact_pairs_with(cube.collider)
            .filter(|pair| {
                pair.has_any_active_contact()
                    && ((pair.collider1 == cube.collider && pair.collider2 == floor.collider)
                        || (pair.collider2 == cube.collider && pair.collider1 == floor.collider))
            })
            .flat_map(|pair| {
                pair.manifolds.iter().map(|manifold| {
                    (
                        manifold.data.normal,
                        manifold
                            .data
                            .solver_contacts
                            .iter()
                            .map(|contact| (contact.point, contact.dist))
                            .collect::<Vec<_>>(),
                    )
                })
            })
            .collect::<Vec<_>>();
        let deepest_chassis_penetration = chassis_contact_debug
            .iter()
            .flat_map(|(_, contacts)| contacts.iter().map(|(_, dist)| (-*dist).max(0.0)))
            .fold(0.0, f32::max);
        let chassis_pose = scene
            .bodies
            .get(cube.body)
            .map(|body| makepad_pose(body.position()))
            .expect("four-wheel body should still exist after settling");

        assert!(
            deepest_wheel_bottom_below_floor < 0.01,
            "resting wheel bottoms should stay at or above the floor top instead of sinking through it: deepest_wheel_bottom_below_floor={deepest_wheel_bottom_below_floor:.4} wheels={:?}",
            scene.vehicles[vehicle_index]
                .controller
                .wheels()
                .iter()
                .map(|wheel| (
                    wheel.center().y,
                    wheel.radius,
                    wheel.raycast_info().contact_point_ws.y,
                    wheel.raycast_info().suspension_length,
                ))
                .collect::<Vec<_>>(),
        );
        assert!(
            front_wheels_in_contact,
            "front wheels should still have floor contact while the vehicle is resting on a flat floor"
        );
        assert!(
            deepest_chassis_penetration < 0.001,
            "the chassis collider should not meaningfully support the vehicle on a flat floor once the wheels settle: deepest_chassis_penetration={deepest_chassis_penetration:.6} pose={:?} chassis_contact_debug={:?}",
            chassis_pose,
            chassis_contact_debug,
        );
    }

    #[test]
    fn scaled_four_wheel_vehicle_keeps_suspension_clearance_on_plate_top() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);

        let scene_scale = vec3f(0.62, 0.62, 0.62);
        let plate_half_extents = vec3f(1.612 * 0.5, 0.08 * 0.5, 1.612 * 0.5);
        let plate_pose = Pose::new(
            Quat::default(),
            vec3f(0.0, -0.16 + -0.06 * scene_scale.y, 0.0),
        );
        let plate_half_extents_world = vec3f(
            plate_half_extents.x * scene_scale.x,
            plate_half_extents.y * scene_scale.y,
            plate_half_extents.z * scene_scale.z,
        );
        let plate_top_y = plate_pose.position.y + plate_half_extents_world.y;
        let floor_uid = WidgetUid(4211);
        scene.spawn_fixed_box(
            floor_uid,
            plate_pose,
            plate_half_extents_world,
            vec3f(1.0, 1.0, 1.0),
            1.8,
            0.0,
        );

        let chassis_half_extents = vec3f(
            0.29 * 0.5 * scene_scale.x,
            0.09 * 0.5 * scene_scale.y,
            0.41 * 0.5 * scene_scale.z,
        );
        let support_radius = four_wheel_support_radius(chassis_half_extents);
        let support_rest_length =
            (support_radius * XR_FOUR_WHEEL_REST_LENGTH_SCALE).clamp(0.018, 0.072);
        let widget_uid = WidgetUid(4212);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(
                Quat::default(),
                vec3f(
                    0.0,
                    plate_top_y
                        + chassis_half_extents.y
                        + support_rest_length
                        + support_radius
                        + 0.03,
                    0.0,
                ),
            ),
            chassis_half_extents,
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );

        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("scaled four-wheel cube should have a vehicle controller");

        for _ in 0..120 {
            scene.step();
        }

        let vehicle = &scene.vehicles[vehicle_index];
        let wheel_debug = vehicle
            .controller
            .wheels()
            .iter()
            .map(|wheel| {
                (
                    wheel.raycast_info().is_in_contact,
                    wheel.raycast_info().hard_point_ws.y,
                    wheel.raycast_info().contact_point_ws.y,
                    wheel.raycast_info().suspension_length,
                )
            })
            .collect::<Vec<_>>();
        let min_suspension_length = vehicle
            .controller
            .wheels()
            .iter()
            .map(|wheel| wheel.raycast_info().suspension_length)
            .fold(f32::INFINITY, f32::min);
        let deepest_contact_below_top = vehicle
            .controller
            .wheels()
            .iter()
            .filter(|wheel| wheel.raycast_info().is_in_contact)
            .map(|wheel| plate_top_y - wheel.raycast_info().contact_point_ws.y)
            .fold(0.0, f32::max);
        let deepest_visual_bottom_below_top = vehicle
            .controller
            .wheels()
            .iter()
            .filter(|wheel| wheel.raycast_info().is_in_contact)
            .map(|wheel| plate_top_y - (wheel.center().y - wheel.radius))
            .fold(0.0, f32::max);

        assert!(
            min_suspension_length > 0.01,
            "scaled XR vehicle suspension should keep a non-zero ride height instead of collapsing flat: min_suspension_length={min_suspension_length:.4} wheels={wheel_debug:?}",
        );
        assert!(
            deepest_contact_below_top < 0.01,
            "scaled XR vehicle wheels should contact the plate near its top surface instead of sinking through it: deepest_contact_below_top={deepest_contact_below_top:.4} plate_top_y={plate_top_y:.4} wheels={wheel_debug:?}"
        );
        assert!(
            deepest_visual_bottom_below_top < 0.01,
            "scaled XR vehicle wheel visuals should stay on top of the plate instead of sinking through it: deepest_visual_bottom_below_top={deepest_visual_bottom_below_top:.4} plate_top_y={plate_top_y:.4} wheels={wheel_debug:?}"
        );
    }

    #[test]
    fn four_wheel_vehicle_support_pose_axes_match_controller_wheel_axes() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);
        scene.spawn_fixed_box(
            WidgetUid(4213),
            Pose::new(Quat::default(), vec3f(0.0, -0.05, 0.0)),
            vec3f(4.0, 0.05, 4.0),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
        );

        let widget_uid = WidgetUid(4214);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.20, 0.0)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("four-wheel cube should have a vehicle controller");

        for _ in 0..40 {
            scene.clear_car_controls();
            scene.apply_car_control(XrCarControl {
                widget_uid,
                steer: 0.0,
                throttle: 1.0,
                brake: 0.0,
            });
            scene.step();
        }
        for _ in 0..80 {
            scene.clear_car_controls();
            scene.apply_car_control(XrCarControl {
                widget_uid,
                steer: 0.7,
                throttle: 1.0,
                brake: 0.0,
            });
            scene.step();
        }

        let owner_pose = scene
            .bodies
            .get(cube.body)
            .map(|body| makepad_pose(body.position()))
            .expect("vehicle body should still exist");
        let owner_inverse = owner_pose.orientation.invert();
        let local_pose = scene.cube_linked_support_local_poses(cube)[0]
            .expect("front-left wheel support pose should exist");
        let wheel = &scene.vehicles[vehicle_index].controller.wheels()[0];

        let expected_axle_local =
            owner_inverse.rotate_vec3(&makepad_vec3(wheel.axle()).normalize());
        let actual_axle_local = local_pose
            .orientation
            .rotate_vec3(&vec3f(-1.0, 0.0, 0.0))
            .normalize();

        assert_vec3_close(actual_axle_local, expected_axle_local, 0.04);
    }

    #[test]
    fn four_wheel_vehicle_keeps_moving_when_one_side_climbs_a_ramp() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);
        scene.spawn_fixed_box(
            WidgetUid(4215),
            Pose::new(Quat::default(), vec3f(0.0, -0.05, 0.0)),
            vec3f(4.0, 0.05, 4.0),
            vec3f(1.0, 1.0, 1.0),
            1.8,
            0.0,
        );

        let ramp_half_extents = vec3f(0.12, 0.02, 0.55);
        let ramp_angle = 0.24;
        let ramp_bottom_offset =
            ramp_half_extents.y * ramp_angle.cos() + ramp_half_extents.z * ramp_angle.sin();
        scene.spawn_fixed_box(
            WidgetUid(4216),
            Pose::new(
                Quat::from_axis_angle(vec3f(1.0, 0.0, 0.0), ramp_angle),
                vec3f(-0.16, ramp_bottom_offset, 0.45),
            ),
            ramp_half_extents,
            vec3f(1.0, 1.0, 1.0),
            1.8,
            0.0,
        );

        let widget_uid = WidgetUid(4217);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.20, -0.55)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        let start_position = scene
            .bodies
            .get(cube.body)
            .map(|body| makepad_pose(body.position()).position)
            .expect("four-wheel body should exist at spawn");
        let vehicle_index = scene
            .vehicle_index_for_widget_uid(widget_uid)
            .expect("four-wheel cube should have a vehicle controller");
        let mut max_left_contact_y = 0.0_f32;

        for _ in 0..220 {
            scene.clear_car_controls();
            scene.apply_car_control(XrCarControl {
                widget_uid,
                steer: 0.0,
                throttle: 1.0,
                brake: 0.0,
            });
            scene.step();
            let body = scene
                .bodies
                .get(cube.body)
                .expect("four-wheel body should still exist while climbing");
            let _pose = makepad_pose(body.position());
            for wheel in scene.vehicles[vehicle_index]
                .controller
                .wheels()
                .iter()
                .take(2)
            {
                if wheel.raycast_info().is_in_contact {
                    max_left_contact_y =
                        max_left_contact_y.max(wheel.raycast_info().contact_point_ws.y);
                }
            }
        }

        let end_position = scene
            .bodies
            .get(cube.body)
            .map(|body| makepad_pose(body.position()).position)
            .expect("four-wheel body should still exist after climbing");

        assert!(
            end_position.z > start_position.z + 0.45,
            "single-side ramp contact should not stall the vehicle; start={start_position:?} end={end_position:?}"
        );
        assert!(
            max_left_contact_y > 0.04,
            "single-side ramp contact should let the climbing-side wheels reach elevated support instead of skimming flat ground; max_left_contact_y={max_left_contact_y:.3} end={end_position:?}",
        );
    }

    #[test]
    fn four_wheel_vehicle_resists_stalling_on_a_low_hump() {
        let mut scene = RapierScene::new(9.81);
        scene.set_simulation_dt(1.0 / 240.0);
        scene.spawn_fixed_box(
            WidgetUid(4218),
            Pose::new(Quat::default(), vec3f(0.0, -0.05, 0.0)),
            vec3f(4.0, 0.05, 4.0),
            vec3f(1.0, 1.0, 1.0),
            1.8,
            0.0,
        );
        scene.spawn_fixed_box(
            WidgetUid(4219),
            Pose::new(Quat::default(), vec3f(0.0, 0.030, 0.38)),
            vec3f(0.26, 0.030, 0.065),
            vec3f(1.0, 1.0, 1.0),
            1.8,
            0.0,
        );

        let widget_uid = WidgetUid(4220);
        scene.spawn_dynamic_box_with_support(
            widget_uid,
            Pose::new(Quat::default(), vec3f(0.0, 0.22, -0.55)),
            vec3f(0.145, 0.045, 0.205),
            vec3f(1.0, 1.0, 1.0),
            120.0,
            1.35,
            0.02,
            XrDepthQuerySupportRig::FourWheels,
        );
        let cube = scene
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
            .expect("four-wheel vehicle should exist");
        let start_position = scene
            .bodies
            .get(cube.body)
            .map(|body| makepad_pose(body.position()).position)
            .expect("four-wheel body should exist at spawn");

        for _ in 0..260 {
            scene.clear_car_controls();
            scene.apply_car_control(XrCarControl {
                widget_uid,
                steer: 0.0,
                throttle: 1.0,
                brake: 0.0,
            });
            scene.step();
        }

        let end_position = scene
            .bodies
            .get(cube.body)
            .map(|body| makepad_pose(body.position()).position)
            .expect("four-wheel body should still exist after the hump");

        assert!(
            end_position.z > start_position.z + 0.70,
            "low humps should not pin the chassis before the vehicle can crawl over them; start={start_position:?} end={end_position:?}"
        );
    }

    #[test]
    fn spawn_pool_respawn_reenables_body_and_survives_a_step() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(420);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.4));
        scene.spawn_dynamic_sphere(widget_uid, pose, 0.04, vec3f(1.0, 1.0, 1.0), 1.0, 0.5, 0.0);
        scene.register_spawn_pool_cube(0);

        let cube = scene.cubes[0];
        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should exist before respawn");
        assert!(!body.is_enabled(), "projectile pool bodies start disabled");

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -8.0),
            vec3f(0.0, 0.0, 0.0),
        );

        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should still exist after respawn");
        assert!(
            body.is_enabled(),
            "respawn should re-enable the pooled body"
        );
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);

        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should still exist after a step");
        assert!(
            body.is_enabled(),
            "a respawned pooled body should remain enabled after stepping"
        );
    }

    #[test]
    fn hand_contact_grab_and_release_restores_dynamic_body_velocity() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(43);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let hand_pose = Pose::new(Quat::default(), pose.position);

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: hand_pose,
            previous_pose: hand_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };

        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after grab");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.held_by_for_body(cube.body),
            Some(XrSharedHand::LeftHand)
        );

        let release_velocity = vec3f(0.6, 0.0, -0.4);
        scene.left_hand_grab.linvel = release_velocity;
        scene.left_hand_grab.gripping = false;
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after release");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
        assert_vec3_close(
            vec3f(body.linvel().x, body.linvel().y, body.linvel().z),
            release_velocity,
            0.0001,
        );
    }

    #[test]
    fn hand_grab_anchors_body_surface_to_hand_pose_instead_of_body_center() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(430);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        let half_extents = vec3f(0.05, 0.05, 0.05);
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            half_extents,
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let acquire_pose = Pose::new(Quat::default(), pose.position + vec3f(0.0, 0.0, 0.11));
        let moved_pose = Pose::new(Quat::default(), pose.position + vec3f(0.16, 0.0, 0.11));

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: acquire_pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: acquire_pose,
            previous_pose: acquire_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };

        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));

        scene.left_hand_grab.previous_pose = scene.left_hand_grab.pose;
        scene.left_hand_grab.pose = moved_pose;
        scene.apply_held_body_targets();
        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after moving the held hand");
        let body_pose = makepad_pose(body.position());
        let center_distance = (body_pose.position - moved_pose.position).length();
        assert!(
            (center_distance - half_extents.z).abs() <= 0.012,
            "center_distance={center_distance:?} body_pose={body_pose:?} moved_pose={moved_pose:?}"
        );

        let local_anchor = scene.left_hand_grab.grab_offset.invert().position;
        assert!(
            (local_anchor.z - half_extents.z).abs() <= 0.012,
            "local_anchor={local_anchor:?} half_extents={half_extents:?}"
        );
    }

    #[test]
    fn controller_grip_grabs_and_releases_body_as_left_controller() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(431);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        let mut left_controller = XrController::default();
        left_controller.buttons = XrController::ACTIVE;
        left_controller.grip = 1.0;
        left_controller.grip_pose = Pose::new(Quat::default(), pose.position);

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Box {
                pose: left_controller.grip_pose,
                half_extents: vec3f(0.032, 0.030, 0.055),
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.sync_tracked_hands(
            &XrHand::default(),
            &XrHand::default(),
            &left_controller,
            &XrController::default(),
        );

        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        assert_eq!(
            scene.held_by_for_body(cube.body),
            Some(XrSharedHand::LeftController)
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after controller grab");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);

        left_controller.grip = 0.0;
        scene.sync_tracked_hands(
            &XrHand::default(),
            &XrHand::default(),
            &left_controller,
            &XrController::default(),
        );
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after controller release");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
    }

    #[test]
    fn secondary_hand_can_join_existing_hold_and_keep_body_kinematic() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(44);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let left_pose = Pose::new(Quat::default(), pose.position);
        let right_pose = Pose::new(Quat::default(), pose.position + vec3f(0.08, 0.0, 0.0));

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: left_pose,
            previous_pose: left_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };
        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));

        RapierScene::sync_hand_bodies(
            &scene.right_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.right_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::RightHand,
            pose: right_pose,
            previous_pose: right_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };
        scene.step();

        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        assert_eq!(scene.right_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist while two hands hold it");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);

        scene.left_hand_grab.gripping = false;
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        assert_eq!(scene.right_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after primary hand drops");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.held_by_for_body(cube.body),
            Some(XrSharedHand::RightHand)
        );
    }

    #[test]
    fn sticky_raw_grab_bit_does_not_keep_pointing_hand_in_grab_state() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(45);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let hand_pose = Pose::new(Quat::default(), pose.position);

        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: hand_pose,
            previous_pose: hand_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: Some(cube.body),
            grab_offset: Pose::default(),
        };

        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE | (1 << XrHand::INDEX_TIP);
        hand.tips[XrHand::INDEX_TIP] = 0.038;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), pose.position);
        hand.joints[XrHand::WRIST] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, -0.03, 0.05));
        hand.joints[XrHand::INDEX_BASE] = Pose::new(Quat::default(), pose.position);
        hand.joints[XrHand::INDEX_KNUCKLE1] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, 0.0, -0.041));
        hand.joints[XrHand::INDEX_KNUCKLE2] =
            Pose::new(Quat::default(), pose.position + vec3f(0.001, 0.002, -0.082));
        hand.joints[XrHand::INDEX_KNUCKLE3] =
            Pose::new(Quat::default(), pose.position + vec3f(0.002, 0.004, -0.122));

        scene.sync_tracked_hands(
            &hand,
            &XrHand::default(),
            &XrController::default(),
            &XrController::default(),
        );
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after sticky-grab release");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
    }

    #[test]
    fn hand_grab_pose_uses_index_tip_when_pinch_midpoint_is_unavailable() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(46);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let hand_pose = Pose::new(Quat::default(), pose.position);

        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: hand_pose,
            previous_pose: hand_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: Some(cube.body),
            grab_offset: Pose::default(),
        };

        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE | (1 << XrHand::INDEX_TIP);
        hand.tips[XrHand::INDEX_TIP] = 0.034;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), pose.position);
        hand.joints[XrHand::WRIST] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, -0.03, 0.05));
        hand.joints[XrHand::INDEX_BASE] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, 0.0, -0.010));
        hand.joints[XrHand::INDEX_KNUCKLE1] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, -0.005, -0.030));
        hand.joints[XrHand::INDEX_KNUCKLE2] = Pose::new(
            Quat::default(),
            pose.position + vec3f(0.012, -0.018, -0.040),
        );
        hand.joints[XrHand::INDEX_KNUCKLE3] = Pose::new(
            Quat::default(),
            pose.position + vec3f(0.022, -0.032, -0.020),
        );

        assert!(
            hand.grab_intent(),
            "curled hand should still report grab intent"
        );
        assert!(
            hand.tracking_pose().is_some(),
            "palm tracking should still be valid for the sample"
        );
        assert!(
            hand.pinch_anchor_pose().is_none(),
            "sample must not expose a pinch anchor"
        );
        let index_tip = hand
            .tip_pos_checked(XrHand::INDEX_TIP)
            .expect("index tip should still be valid");
        let palm_pose = hand
            .tracking_pose()
            .expect("tracking pose should still be valid");

        scene.sync_tracked_hands(
            &hand,
            &XrHand::default(),
            &XrController::default(),
            &XrController::default(),
        );
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        assert!(scene.left_hand_grab.tracked);
        assert!(scene.left_hand_grab.gripping);
        assert_vec3_close(scene.left_hand_grab.pose.position, index_tip, 0.0001);
        assert!(
            (scene.left_hand_grab.pose.position - palm_pose.position).length() > 0.01,
            "grab pose should stay on the finger anchor, not the palm"
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after index-tip hold update");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
    }
}
