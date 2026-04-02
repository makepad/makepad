use super::xr_physics::{capsule_pose, makepad_pose, HandCollider, HandColliderBody, RapierScene};
use super::*;
use makepad_widgets::makepad_platform::event::XrController;

impl XrHandSystem {
    fn controller_grip_pose(controller: &XrController) -> Option<Pose> {
        let pose = controller.grip_pose;
        (controller.active() && pose.is_finite()).then_some(pose)
    }

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

    fn hand_plate_pose(hand: &XrHand) -> Option<Pose> {
        let palm_pose = hand.tracking_pose()?;
        Some(Pose::new(
            palm_pose.orientation,
            Self::pose_point_world(palm_pose, vec3f(0.0, 0.0, XR_HAND_PLATE_FORWARD_OFFSET)),
        ))
    }

    fn hand_tip_world(hand: &XrHand, finger_index: usize) -> Option<Vec3f> {
        hand.tip_pos_checked(finger_index)
    }

    fn append_finger_chain_colliders(
        colliders: &mut Vec<HandCollider>,
        hand: &XrHand,
        chain: &[usize],
        tip_index: usize,
        radius: f32,
    ) {
        let Some(points) = hand.joint_chain_positions(chain) else {
            return;
        };
        for segment in points.windows(2) {
            Self::append_capsule_collider(colliders, segment[0], segment[1], radius);
        }
        if hand.tip_active(tip_index) {
            if let (Some(end_joint), Some(tip_world)) = (
                points.last().copied(),
                Self::hand_tip_world(hand, tip_index),
            ) {
                Self::append_capsule_collider(colliders, end_joint, tip_world, radius * 0.85);
            }
        }
    }

    fn append_fingertip_collider(
        colliders: &mut Vec<HandCollider>,
        hand: &XrHand,
        tip_index: usize,
        radius: f32,
    ) {
        if hand.tip_active(tip_index) {
            if let Some(tip_world) = Self::hand_tip_world(hand, tip_index) {
                Self::append_ball_collider(colliders, tip_world, radius);
            }
        }
    }

    pub(super) fn build_hand_colliders(
        &self,
        hand: &XrHand,
        controller: &XrController,
    ) -> Vec<HandCollider> {
        let mut colliders = Vec::with_capacity(XR_HAND_COLLIDER_SLOTS_PER_HAND);
        if let Some(grip_pose) = Self::controller_grip_pose(controller) {
            Self::append_box_collider(&mut colliders, grip_pose, vec3f(0.032, 0.030, 0.055));
            return colliders;
        }
        if !hand.in_view() {
            return colliders;
        }

        if let Some(hand_plate_pose) = Self::hand_plate_pose(hand) {
            Self::append_box_collider(
                &mut colliders,
                hand_plate_pose,
                vec3f(
                    XR_HAND_PLATE_HALF_WIDTH,
                    XR_HAND_PLATE_HALF_HEIGHT,
                    XR_HAND_PLATE_HALF_DEPTH,
                ),
            );
        }

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

    #[allow(dead_code)]
    pub(super) fn collect_live_hand_colliders(
        &self,
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

    fn hand_influence_tip_world(&self, hand: &XrHand, tip: usize) -> Option<Vec3f> {
        if !hand.in_view() || !hand.tip_active(tip) {
            return None;
        }
        hand.tip_pos_checked(tip)
    }

    fn hand_influence_point(
        &self,
        pos: Vec3f,
        gain_scale: f32,
        radius_scale: f32,
    ) -> XrHandInfluencePoint {
        XrHandInfluencePoint {
            pos,
            gain_scale,
            radius_scale,
        }
    }

    fn palm_world(&self, hand: &XrHand) -> Option<Vec3f> {
        let center = hand.joint_pose_checked(XrHand::CENTER)?.position;
        let wrist = hand.joint_pose_checked(XrHand::WRIST)?.position;
        let thumb = hand.joint_pose_checked(XrHand::THUMB_BASE)?.position;
        let index = hand.joint_pose_checked(XrHand::INDEX_BASE)?.position;
        let middle = hand.joint_pose_checked(XrHand::MIDDLE_BASE)?.position;
        let ring = hand.joint_pose_checked(XrHand::RING_BASE)?.position;
        let little = hand.joint_pose_checked(XrHand::LITTLE_BASE)?.position;
        Some(
            center * 0.28
                + wrist * 0.10
                + thumb * 0.12
                + index * 0.13
                + middle * 0.18
                + ring * 0.11
                + little * 0.08,
        )
    }

    fn write_influence_points(&self, hand: &XrHand, target: &mut [Option<XrHandInfluencePoint>]) {
        debug_assert_eq!(target.len(), XR_HAND_INFLUENCE_POINTS_PER_HAND);
        target[0] = self
            .hand_influence_tip_world(hand, XrHand::THUMB_TIP)
            .map(|pos| self.hand_influence_point(pos, 0.72, 0.92));
        target[1] = self
            .hand_influence_tip_world(hand, XrHand::INDEX_TIP)
            .map(|pos| self.hand_influence_point(pos, 1.00, 1.00));
        target[2] = self
            .hand_influence_tip_world(hand, XrHand::MIDDLE_TIP)
            .map(|pos| self.hand_influence_point(pos, 0.96, 1.00));
        target[3] = self
            .hand_influence_tip_world(hand, XrHand::RING_TIP)
            .map(|pos| self.hand_influence_point(pos, 0.82, 0.94));
        target[4] = self
            .hand_influence_tip_world(hand, XrHand::LITTLE_TIP)
            .map(|pos| self.hand_influence_point(pos, 0.68, 0.88));
        target[5] = self
            .palm_world(hand)
            .map(|pos| self.hand_influence_point(pos, 1.30, 2.40));
    }

    pub(super) fn draw_scope_hand_influence_points(
        &self,
        state: Option<&XrState>,
    ) -> [Option<XrHandInfluencePoint>; XR_HAND_INFLUENCE_POINT_COUNT] {
        let mut points = [None; XR_HAND_INFLUENCE_POINT_COUNT];
        let Some(state) = state else {
            return points;
        };
        let (left_points, right_points) = points.split_at_mut(XR_HAND_INFLUENCE_POINTS_PER_HAND);
        self.write_influence_points(&state.left_hand, left_points);
        self.write_influence_points(&state.right_hand, right_points);
        points
    }
}

impl XrEnv {
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
            raw_colliders = self
                .world
                .hands
                .build_hand_colliders(hand, &XrController::default());
            &raw_colliders
        };
        self.draw_hand_shapes(cx, colliders, is_left);

        self.draw_cube.begin_many_instances(cx);
        for joint in &hand.joints {
            self.draw_pose_box(cx, *joint, vec3(0.011, 0.011, 0.016), joint_color, 0.0);
        }
        self.draw_cube.end_many_instances(cx);
    }
}

pub(super) fn build_hand_colliders_for_physics(
    hand: &XrHand,
    controller: &XrController,
) -> Vec<HandCollider> {
    XrHandSystem.build_hand_colliders(hand, controller)
}

pub(super) fn sync_hands_on_scene(
    scene: Option<&mut RapierScene>,
    left_hand: &XrHand,
    right_hand: &XrHand,
    left_controller: &XrController,
    right_controller: &XrController,
) {
    if !XR_ENABLE_HAND_PHYSICS {
        return;
    }

    let Some(scene) = scene else {
        return;
    };

    let left = build_hand_colliders_for_physics(left_hand, left_controller);
    let right = build_hand_colliders_for_physics(right_hand, right_controller);
    let RapierScene {
        bodies,
        colliders,
        left_hand: left_slots,
        right_hand: right_slots,
        ..
    } = scene;
    RapierScene::sync_hand_bodies(left_slots, &left, bodies, colliders);
    RapierScene::sync_hand_bodies(right_slots, &right, bodies, colliders);
    scene.sync_tracked_hands(left_hand, right_hand, left_controller, right_controller);
}
