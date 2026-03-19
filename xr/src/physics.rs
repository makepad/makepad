use super::*;
use super::depth::{
    DepthQuerySurfaceCollider, DepthQuerySurfaceShape, DepthQuerySurfaceTarget,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum HandCollider {
    Capsule { a: Vec3f, b: Vec3f, radius: f32 },
    Ball { center: Vec3f, radius: f32 },
    Box { pose: Pose, half_extents: Vec3f },
}

#[derive(Clone, Copy)]
pub(super) struct PhysicsCube {
    pub(super) body: RigidBodyHandle,
    pub(super) collider: ColliderHandle,
    pub(super) half_extents: Vec3f,
}

#[derive(Clone, Copy)]
pub(super) struct HandColliderBody {
    pub(super) body: RigidBodyHandle,
    pub(super) collider: ColliderHandle,
}

pub(super) struct RapierScene {
    gravity: RapierVector,
    integration_parameters: IntegrationParameters,
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    pub(super) bodies: RigidBodySet,
    pub(super) colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    pub(super) cubes: Vec<PhysicsCube>,
    depth_query_surfaces: Vec<DepthQuerySurfaceCollider>,
    pub(super) left_hand: Vec<HandColliderBody>,
    pub(super) right_hand: Vec<HandColliderBody>,
}

fn rapier_vec3(v: Vec3f) -> RapierVector {
    RapierVector::new(v.x, v.y, v.z)
}

fn rapier_rotation(q: Quat) -> RapierRotation {
    RapierRotation::from_xyzw(q.x, q.y, q.z, q.w)
}

fn rapier_pose(pose: Pose) -> RapierPose {
    RapierPose::from_parts(
        rapier_vec3(pose.position),
        rapier_rotation(pose.orientation),
    )
}

pub(super) fn makepad_pose(pose: &RapierPose) -> Pose {
    Pose::new(
        Quat {
            x: pose.rotation.x,
            y: pose.rotation.y,
            z: pose.rotation.z,
            w: pose.rotation.w,
        },
        vec3f(pose.translation.x, pose.translation.y, pose.translation.z),
    )
}

pub(super) fn capsule_pose(a: Vec3f, b: Vec3f) -> (RapierPose, RapierReal) {
    let delta = b - a;
    let length = delta.length();
    let rotation = if length > 1.0e-4 {
        RapierRotation::from_rotation_arc(RapierVector::Y, rapier_vec3(delta * (1.0 / length)))
    } else {
        RapierRotation::IDENTITY
    };
    (
        RapierPose::from_parts(rapier_vec3((a + b) * 0.5), rotation),
        (length * 0.5).max(0.0005),
    )
}

impl RapierScene {
    pub(super) fn spawn_dynamic_box(&mut self, pose: Pose, half_extents: Vec3f) {
        let body = self.bodies.insert(
            RigidBodyBuilder::dynamic()
                .pose(rapier_pose(pose))
                .ccd_enabled(true)
                .linear_damping(XR_BODY_LINEAR_DAMPING)
                .angular_damping(XR_BODY_ANGULAR_DAMPING)
                .additional_solver_iterations(XR_BODY_ADDITIONAL_SOLVER_ITERATIONS),
        );
        if let Some(rigid_body) = self.bodies.get_mut(body) {
            let activation = rigid_body.activation_mut();
            activation.angular_threshold = XR_BODY_SLEEP_ANGULAR_THRESHOLD;
            activation.time_until_sleep = XR_BODY_SLEEP_TIME;
        }
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
                .density(1.0)
                .friction(0.8)
                .restitution(0.0),
            body,
            &mut self.bodies,
        );
        self.cubes.push(PhysicsCube {
            body,
            collider,
            half_extents,
        });
    }

    pub(super) fn spawn_fixed_box(&mut self, pose: Pose, half_extents: Vec3f, friction: f32) {
        let body = self
            .bodies
            .insert(RigidBodyBuilder::fixed().pose(rapier_pose(pose)));
        self.colliders.insert_with_parent(
            ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
                .friction(friction),
            body,
            &mut self.bodies,
        );
    }

    pub(super) fn new(_center: Vec3f) -> Self {
        let mut scene = Self {
            gravity: RapierVector::new(0.0, -9.81, 0.0),
            integration_parameters: IntegrationParameters {
                dt: XR_SIMULATION_DT,
                ..IntegrationParameters::default()
            },
            pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            cubes: Vec::new(),
            depth_query_surfaces: Vec::new(),
            left_hand: Vec::new(),
            right_hand: Vec::new(),
        };

        // Invisible floor at XR ground level (y=0).
        let floor = scene.bodies.insert(RigidBodyBuilder::fixed().build());
        scene.colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::halfspace(RapierVector::new(0.0, 1.0, 0.0)))
                .friction(0.9),
            floor,
            &mut scene.bodies,
        );
        if XR_ENABLE_DEPTH_QUERY_PHYSICS {
            scene.depth_query_surfaces = (0..XR_DEPTH_QUERY_SHARED_SURFACE_POOL_SIZE)
                .map(|_| scene.spawn_depth_query_surface())
                .collect();
        }
        if XR_ENABLE_HAND_PHYSICS {
            scene.left_hand = scene.spawn_hand_colliders(XR_HAND_COLLIDER_SLOTS_PER_HAND);
            scene.right_hand = scene.spawn_hand_colliders(XR_HAND_COLLIDER_SLOTS_PER_HAND);
        }
        scene.step();
        scene
    }

    fn spawn_hand_colliders(&mut self, count: usize) -> Vec<HandColliderBody> {
        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            let body = self
                .bodies
                .insert(RigidBodyBuilder::kinematic_position_based().pose(RapierPose::IDENTITY));
            let collider = self.colliders.insert_with_parent(
                ColliderBuilder::capsule_y(0.01, 0.01)
                    .friction(XR_HAND_COLLIDER_FRICTION)
                    .restitution(0.0),
                body,
                &mut self.bodies,
            );
            if let Some(collider) = self.colliders.get_mut(collider) {
                collider.set_enabled(false);
            }
            result.push(HandColliderBody { body, collider });
        }
        result
    }

    fn spawn_depth_query_surface(&mut self) -> DepthQuerySurfaceCollider {
        let body = self.bodies.insert(RigidBodyBuilder::fixed().build());
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::triangle(
                RapierVector::new(0.0, -1000.0, 0.0),
                RapierVector::new(0.0, -1000.0, 0.01),
                RapierVector::new(0.01, -1000.0, 0.0),
            )
            .friction(XR_DEPTH_QUERY_FRICTION),
            body,
            &mut self.bodies,
        );
        if let Some(collider) = self.colliders.get_mut(collider) {
            collider.set_enabled(false);
        }
        DepthQuerySurfaceCollider {
            collider,
            fingerprint: 0,
        }
    }

    pub(super) fn sync_hand_bodies(
        bodies: &[HandColliderBody],
        colliders: &[HandCollider],
        rigid_bodies: &mut RigidBodySet,
        collider_set: &mut ColliderSet,
    ) {
        for (index, slot) in bodies.iter().enumerate() {
            let active = index < colliders.len();
            if active {
                let (target_pose, shape) = match colliders[index] {
                    HandCollider::Capsule { a, b, radius } => {
                        let (target_pose, half_height) = capsule_pose(a, b);
                        (target_pose, SharedShape::capsule_y(half_height, radius))
                    }
                    HandCollider::Ball { center, radius } => (
                        RapierPose::from_parts(rapier_vec3(center), RapierRotation::IDENTITY),
                        SharedShape::ball(radius),
                    ),
                    HandCollider::Box { pose, half_extents } => (
                        rapier_pose(pose),
                        SharedShape::cuboid(half_extents.x, half_extents.y, half_extents.z),
                    ),
                };
                let was_enabled = collider_set
                    .get(slot.collider)
                    .map(|collider| collider.is_enabled())
                    .unwrap_or(false);
                if let Some(collider) = collider_set.get_mut(slot.collider) {
                    collider.set_shape(shape);
                    collider.set_enabled(true);
                }
                if let Some(body) = rigid_bodies.get_mut(slot.body) {
                    if !was_enabled {
                        // Reset the body pose on reacquire so tracking loss doesn't inject a huge velocity spike.
                        body.set_position(target_pose, false);
                    }
                    body.set_next_kinematic_position(target_pose);
                }
            } else if let Some(collider) = collider_set.get_mut(slot.collider) {
                collider.set_enabled(false);
            }
        }
    }

    pub(super) fn step(&mut self) {
        self.pipeline.step(
            self.gravity,
            &self.integration_parameters,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            &(),
            &(),
        );
        self.settle_resting_bodies();
    }

    fn settle_resting_bodies(&mut self) {
        let linear_speed_sq = XR_BODY_SNAP_SLEEP_LINEAR_SPEED * XR_BODY_SNAP_SLEEP_LINEAR_SPEED;
        let angular_speed_sq = XR_BODY_SNAP_SLEEP_ANGULAR_SPEED * XR_BODY_SNAP_SLEEP_ANGULAR_SPEED;
        let mut to_sleep = Vec::new();

        for cube in &self.cubes {
            let has_active_contact = self
                .narrow_phase
                .contact_pairs_with(cube.collider)
                .any(|pair| pair.has_any_active_contact());
            if !has_active_contact {
                continue;
            }

            let Some(body) = self.bodies.get(cube.body) else {
                continue;
            };
            if body.is_sleeping() {
                continue;
            }

            let linvel = body.linvel();
            let angvel = body.angvel();
            let linvel_sq = linvel.x * linvel.x + linvel.y * linvel.y + linvel.z * linvel.z;
            let angvel_sq = angvel.x * angvel.x + angvel.y * angvel.y + angvel.z * angvel.z;
            if linvel_sq <= linear_speed_sq && angvel_sq <= angular_speed_sq {
                to_sleep.push(cube.body);
            }
        }

        for handle in to_sleep {
            if let Some(body) = self.bodies.get_mut(handle) {
                body.set_linvel(RapierVector::ZERO, false);
                body.set_angvel(RapierVector::ZERO, false);
            }
        }
    }

    pub(super) fn depth_query_key(index: usize) -> u64 {
        index as u64 + 1
    }

    pub(super) fn sync_depth_query_surface_pool(&mut self, targets: &[DepthQuerySurfaceTarget]) {
        for (surface, target) in self.depth_query_surfaces.iter_mut().zip(targets.iter()) {
            if surface.fingerprint != target.fingerprint {
                if let Some(collider) = self.colliders.get_mut(surface.collider) {
                    let shape = match target.shape {
                        DepthQuerySurfaceShape::Triangle(triangle) => SharedShape::triangle(
                            rapier_vec3(triangle[0]),
                            rapier_vec3(triangle[1]),
                            rapier_vec3(triangle[2]),
                        ),
                        DepthQuerySurfaceShape::Quad(quad) => SharedShape::trimesh(
                            vec![
                                rapier_vec3(quad[0]),
                                rapier_vec3(quad[1]),
                                rapier_vec3(quad[2]),
                                rapier_vec3(quad[3]),
                            ],
                            vec![[0, 1, 2], [0, 2, 3]],
                        )
                        .unwrap_or_else(|_| {
                            SharedShape::triangle(
                                rapier_vec3(quad[0]),
                                rapier_vec3(quad[1]),
                                rapier_vec3(quad[2]),
                            )
                        }),
                    };
                    collider.set_shape(shape);
                }
                surface.fingerprint = target.fingerprint;
            }
            if let Some(collider) = self.colliders.get_mut(surface.collider) {
                collider.set_enabled(true);
            }
        }
        for surface in self.depth_query_surfaces.iter_mut().skip(targets.len()) {
            if let Some(collider) = self.colliders.get_mut(surface.collider) {
                collider.set_enabled(false);
            }
            surface.fingerprint = 0;
        }
    }
}
