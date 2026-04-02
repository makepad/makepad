use super::*;

impl XrPeerSync {
    pub(super) fn peer_label(peer_id: XrNetPeerId) -> String {
        format!("{:08x}", peer_id.0)
    }

    pub(super) fn peer_transform(peer: &RemotePeerState) -> Mat4f {
        peer.remote_to_local.unwrap_or_default()
    }

    pub(super) fn peer_remote_to_local_transform(&self, peer_id: XrNetPeerId) -> Mat4f {
        self.runtime
            .registry
            .peers
            .get(&peer_id)
            .map(Self::peer_transform)
            .unwrap_or_default()
    }

    pub(super) fn transform_point(transform: &Mat4f, point: Vec3f) -> Vec3f {
        let point = transform.transform_vec4(vec4f(point.x, point.y, point.z, 1.0));
        if point.w.abs() > 1.0e-6 {
            vec3f(point.x / point.w, point.y / point.w, point.z / point.w)
        } else {
            point.to_vec3f()
        }
    }

    pub(super) fn transform_anchor(transform: &Mat4f, anchor: XrAnchor) -> XrAnchor {
        XrAnchor {
            left: Self::transform_point(transform, anchor.left),
            right: Self::transform_point(transform, anchor.right),
        }
    }

    pub(super) fn transform_direction(transform: &Mat4f, direction: Vec3f) -> Vec3f {
        transform
            .transform_vec4(vec4f(direction.x, direction.y, direction.z, 0.0))
            .to_vec3f()
    }

    pub(super) fn transform_pose(transform: &Mat4f, pose: Pose) -> Pose {
        let position = Self::transform_point(transform, pose.position);
        let mut forward = Self::transform_direction(
            transform,
            pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0)),
        );
        let mut up = Self::transform_direction(
            transform,
            pose.orientation.rotate_vec3(&vec3f(0.0, 1.0, 0.0)),
        );
        if forward.length() <= 1.0e-6 {
            return Pose::new(pose.orientation, position);
        }
        forward = forward.normalize();
        if up.length() <= 1.0e-6 || Vec3f::cross(forward, up).length() <= 1.0e-6 {
            up = vec3f(0.0, 1.0, 0.0);
        } else {
            up = up.normalize();
        }
        Pose::new(Quat::look_rotation(forward, up), position)
    }

    fn hand_tracking_pose(hand: &XrHand) -> Option<Pose> {
        hand.tracking_pose()
    }

    fn controller_tracking_pose(controller: &XrController) -> Option<Pose> {
        let pose = controller.grip_pose;
        (controller.active() && pose.is_finite()).then_some(pose)
    }

    fn local_hand_state_from_frames(
        current: &XrState,
        previous: Option<&XrState>,
        shared_hand: XrSharedHand,
    ) -> Option<LocalSharedHandState> {
        let (pose, previous_pose, gripping) = match shared_hand {
            XrSharedHand::LeftController => {
                let pose = Self::controller_tracking_pose(&current.left_controller)?;
                let previous_pose = previous
                    .and_then(|state| Self::controller_tracking_pose(&state.left_controller))
                    .unwrap_or(pose);
                (pose, previous_pose, current.left_controller.grip >= 0.55)
            }
            XrSharedHand::RightController => {
                let pose = Self::controller_tracking_pose(&current.right_controller)?;
                let previous_pose = previous
                    .and_then(|state| Self::controller_tracking_pose(&state.right_controller))
                    .unwrap_or(pose);
                (pose, previous_pose, current.right_controller.grip >= 0.55)
            }
            XrSharedHand::LeftHand => {
                if current.left_controller.active() {
                    return None;
                }
                let pose = Self::hand_tracking_pose(&current.left_hand)?;
                let previous_pose = previous
                    .and_then(|state| Self::hand_tracking_pose(&state.left_hand))
                    .unwrap_or(pose);
                (pose, previous_pose, current.left_hand.grab_intent())
            }
            XrSharedHand::RightHand => {
                if current.right_controller.active() {
                    return None;
                }
                let pose = Self::hand_tracking_pose(&current.right_hand)?;
                let previous_pose = previous
                    .and_then(|state| Self::hand_tracking_pose(&state.right_hand))
                    .unwrap_or(pose);
                (pose, previous_pose, current.right_hand.grab_intent())
            }
            XrSharedHand::Unknown => return None,
        };
        let dt = previous
            .map(|previous| (current.time - previous.time).abs())
            .unwrap_or(0.0)
            .max(0.0001) as f32;
        Some(LocalSharedHandState {
            shared_hand,
            pose,
            linvel: (pose.position - previous_pose.position) * (1.0 / dt),
            gripping,
        })
    }

    pub(super) fn local_shared_hands(&self) -> Vec<LocalSharedHandState> {
        let Some(current) = self.runtime.local.latest_xr_state.as_ref() else {
            return Vec::new();
        };
        let previous = self.runtime.local.previous_xr_state.as_ref();
        [
            XrSharedHand::LeftController,
            XrSharedHand::RightController,
            XrSharedHand::LeftHand,
            XrSharedHand::RightHand,
        ]
        .into_iter()
        .filter_map(|shared_hand| {
            Self::local_hand_state_from_frames(current, previous, shared_hand)
        })
        .collect()
    }
}
