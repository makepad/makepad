use super::*;
use super::physics::{capsule_pose, makepad_pose, HandCollider, HandColliderBody, RapierScene};

impl XrScene {
    fn pose_point_world(pose: Pose, local: Vec3f) -> Vec3f {
        pose.to_mat4().transform_vec4(local.to_vec4()).to_vec3f()
    }

    fn append_capsule_collider(colliders: &mut Vec<HandCollider>, a: Vec3f, b: Vec3f, radius: f32) {
        colliders.push(HandCollider::Capsule { a, b, radius });
    }

    fn append_ball_collider(colliders: &mut Vec<HandCollider>, center: Vec3f, radius: f32) {
        colliders.push(HandCollider::Ball { center, radius });
    }

    fn append_box_collider(colliders: &mut Vec<HandCollider>, pose: Pose, half_extents: Vec3f) {
        colliders.push(HandCollider::Box { pose, half_extents });
    }

    fn hand_plate_pose(hand: &XrHand) -> Pose {
        let palm_pose = hand.joints[XrHand::CENTER];
        Pose::new(
            palm_pose.orientation,
            Self::pose_point_world(palm_pose, vec3f(0.0, 0.0, XR_HAND_PLATE_FORWARD_OFFSET)),
        )
    }

    fn hand_tip_world(hand: &XrHand, finger_index: usize) -> Vec3f {
        let tip_len = hand.tips[finger_index].max(0.0);
        hand.joints[XrHand::END_KNUCKLES[finger_index]]
            .to_mat4()
            .transform_vec4(vec4(0.0, 0.0, -tip_len, 1.0))
            .to_vec3f()
    }

    fn append_finger_chain_colliders(
        colliders: &mut Vec<HandCollider>,
        hand: &XrHand,
        chain: &[usize],
        tip_index: usize,
        radius: f32,
    ) {
        for segment in chain.windows(2) {
            Self::append_capsule_collider(
                colliders,
                hand.joints[segment[0]].position,
                hand.joints[segment[1]].position,
                radius,
            );
        }
        if hand.tip_active(tip_index) {
            let end_joint = *chain.last().unwrap_or(&XrHand::CENTER);
            Self::append_capsule_collider(
                colliders,
                hand.joints[end_joint].position,
                Self::hand_tip_world(hand, tip_index),
                radius * 0.85,
            );
        }
    }

    fn append_fingertip_collider(
        colliders: &mut Vec<HandCollider>,
        hand: &XrHand,
        tip_index: usize,
        radius: f32,
    ) {
        if hand.tip_active(tip_index) {
            Self::append_ball_collider(colliders, Self::hand_tip_world(hand, tip_index), radius);
        }
    }

    fn build_hand_colliders(hand: &XrHand) -> Vec<HandCollider> {
        let mut colliders = Vec::with_capacity(XR_HAND_COLLIDER_SLOTS_PER_HAND);
        if !hand.in_view() {
            return colliders;
        }

        Self::append_box_collider(
            &mut colliders,
            Self::hand_plate_pose(hand),
            vec3f(
                XR_HAND_PLATE_HALF_WIDTH,
                XR_HAND_PLATE_HALF_HEIGHT,
                XR_HAND_PLATE_HALF_DEPTH,
            ),
        );

        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::THUMB_BASE,
                XrHand::THUMB_KNUCKLE1,
                XrHand::THUMB_KNUCKLE2,
            ],
            XrHand::THUMB_TIP,
            0.015,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::THUMB_TIP,
            0.015 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::INDEX_BASE,
                XrHand::INDEX_KNUCKLE1,
                XrHand::INDEX_KNUCKLE2,
                XrHand::INDEX_KNUCKLE3,
            ],
            XrHand::INDEX_TIP,
            0.014,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::INDEX_TIP,
            0.014 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::MIDDLE_BASE,
                XrHand::MIDDLE_KNUCKLE1,
                XrHand::MIDDLE_KNUCKLE2,
                XrHand::MIDDLE_KNUCKLE3,
            ],
            XrHand::MIDDLE_TIP,
            0.015,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::MIDDLE_TIP,
            0.015 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::RING_BASE,
                XrHand::RING_KNUCKLE1,
                XrHand::RING_KNUCKLE2,
                XrHand::RING_KNUCKLE3,
            ],
            XrHand::RING_TIP,
            0.014,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::RING_TIP,
            0.014 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::LITTLE_BASE,
                XrHand::LITTLE_KNUCKLE1,
                XrHand::LITTLE_KNUCKLE2,
                XrHand::LITTLE_KNUCKLE3,
            ],
            XrHand::LITTLE_TIP,
            0.013,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::LITTLE_TIP,
            0.013 * XR_HAND_TIP_RADIUS_SCALE,
        );

        colliders
    }

    pub(super) fn collect_live_hand_colliders(
        scene: &RapierScene,
        slots: &[HandColliderBody],
    ) -> Vec<HandCollider> {
        let mut colliders = Vec::with_capacity(slots.len());
        for slot in slots {
            let Some(collider) = scene.colliders.get(slot.collider) else {
                continue;
            };
            if !collider.is_enabled() {
                continue;
            }

            let pose = makepad_pose(collider.position());
            let shape = collider.shape();
            if let Some(capsule) = shape.as_capsule() {
                colliders.push(HandCollider::Capsule {
                    a: Self::pose_point_world(
                        pose,
                        vec3f(
                            capsule.segment.a.x,
                            capsule.segment.a.y,
                            capsule.segment.a.z,
                        ),
                    ),
                    b: Self::pose_point_world(
                        pose,
                        vec3f(
                            capsule.segment.b.x,
                            capsule.segment.b.y,
                            capsule.segment.b.z,
                        ),
                    ),
                    radius: capsule.radius,
                });
            } else if let Some(ball) = shape.as_ball() {
                colliders.push(HandCollider::Ball {
                    center: pose.position,
                    radius: ball.radius,
                });
            } else if let Some(cuboid) = shape.as_cuboid() {
                colliders.push(HandCollider::Box {
                    pose,
                    half_extents: vec3f(
                        cuboid.half_extents.x,
                        cuboid.half_extents.y,
                        cuboid.half_extents.z,
                    ),
                });
            }
        }
        colliders
    }

    fn draw_hand_shapes(&mut self, cx: &mut Cx2d, colliders: &[HandCollider], is_left: bool) {
        let color = if is_left {
            vec4(0.18, 0.72, 1.0, 1.0)
        } else {
            vec4(1.0, 0.62, 0.20, 1.0)
        };
        for collider in colliders {
            match collider {
                HandCollider::Capsule { a, b, radius } => {
                    let (pose, half_height) = capsule_pose(*a, *b);
                    self.draw_pbr_capsule(
                        cx,
                        makepad_pose(&pose),
                        *radius,
                        half_height,
                        color,
                        0.58,
                    );
                }
                HandCollider::Ball { center, radius } => {
                    self.draw_pbr_sphere(cx, *center, *radius, color, 0.56);
                }
                HandCollider::Box { pose, half_extents } => {
                    self.draw_pbr_rounded_cube(cx, *pose, *half_extents, 0.0, color, 0.60);
                }
            }
        }
    }

    pub(super) fn draw_hand(
        &mut self,
        cx: &mut Cx2d,
        hand: &XrHand,
        physics_colliders: Option<&[HandCollider]>,
        is_left: bool,
    ) {
        if !XR_RENDER_HAND_GEOMETRY || !hand.in_view() {
            return;
        }

        let joint_color = if is_left {
            vec4(0.22, 0.78, 1.0, 1.0)
        } else {
            vec4(1.0, 0.68, 0.30, 1.0)
        };
        let raw_colliders;
        let colliders = if let Some(physics_colliders) = physics_colliders {
            physics_colliders
        } else {
            raw_colliders = Self::build_hand_colliders(hand);
            &raw_colliders
        };
        self.draw_hand_shapes(cx, colliders, is_left);

        self.draw_cube.begin_many_instances(cx);
        for joint in &hand.joints {
            self.draw_pose_box(cx, *joint, vec3(0.011, 0.011, 0.016), joint_color, 0.0);
        }
        self.draw_cube.end_many_instances(cx);
    }

    pub(super) fn sync_hands(&mut self, state: &XrState) {
        if !XR_ENABLE_HAND_PHYSICS {
            return;
        }

        let Some(scene) = self.scene.as_mut() else {
            return;
        };

        let left = Self::build_hand_colliders(&state.left_hand);
        let right = Self::build_hand_colliders(&state.right_hand);
        let RapierScene {
            bodies,
            colliders,
            left_hand,
            right_hand,
            ..
        } = scene;
        RapierScene::sync_hand_bodies(left_hand, &left, bodies, colliders);
        RapierScene::sync_hand_bodies(right_hand, &right, bodies, colliders);
    }
}
