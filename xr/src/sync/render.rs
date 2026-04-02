use super::*;

impl XrPeerSync {
    fn peer_base_color(peer_id: XrNetPeerId) -> Vec4f {
        match (peer_id.0 % 5) as usize {
            0 => vec4f(0.92, 0.38, 0.31, 1.0),
            1 => vec4f(0.24, 0.74, 0.58, 1.0),
            2 => vec4f(0.35, 0.58, 0.98, 1.0),
            3 => vec4f(0.93, 0.70, 0.28, 1.0),
            _ => vec4f(0.80, 0.48, 0.94, 1.0),
        }
    }

    fn peer_alpha(peer: &RemotePeerState) -> f32 {
        match peer.transform_source {
            RemoteTransformSource::Anchor | RemoteTransformSource::Descriptor => 1.0,
            RemoteTransformSource::Raw => 0.42,
        }
    }

    fn draw_cube_at(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        pose_transform: &Mat4f,
        size: Vec3f,
        color: Vec4f,
    ) {
        self.draw_cube.transform = Mat4f::mul(world, pose_transform);
        self.draw_cube.cube_pos = vec3(0.0, 0.0, 0.0);
        self.draw_cube.cube_size = size;
        self.draw_cube.color = color;
        self.draw_cube.depth_clip = 1.0;
        self.draw_cube.draw(cx);
    }

    fn descriptor_height_meters(height_u8: u8) -> f32 {
        if height_u8 == 0 {
            Self::DESCRIPTOR_MIN_HEIGHT_METERS
        } else {
            ((height_u8 as f32 / 255.0) * Self::DESCRIPTOR_MAX_HEIGHT_METERS).clamp(
                Self::DESCRIPTOR_MIN_HEIGHT_METERS,
                Self::DESCRIPTOR_MAX_HEIGHT_METERS,
            )
        }
    }

    pub(super) fn draw_local_descriptor(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let Some(vertical) = self
            .runtime
            .local
            .descriptor
            .as_ref()
            .and_then(|frame| frame.descriptor.vertical_descriptor.as_ref())
            .cloned()
        else {
            return;
        };
        let size = vertical.size as usize;
        if size == 0
            || vertical.vertical_surface_masks.len() != size * size
            || vertical.clutter_surface_masks.len() != size * size
            || vertical.height_u8.len() != size * size
        {
            return;
        }

        let cell_size = vertical.cell_size_meters;
        let footprint = cell_size * Self::DESCRIPTOR_CELL_FOOTPRINT;
        for z in 0..size {
            for x in 0..size {
                let index = x + z * size;
                let vertical_count = vertical.vertical_surface_masks[index].count_ones() as f32;
                let clutter_count = vertical.clutter_surface_masks[index].count_ones() as f32;
                if vertical_count <= 0.0 && clutter_count <= 0.0 {
                    continue;
                }
                let center_x = vertical.origin_x + (x as f32 + 0.5) * cell_size;
                let center_z = vertical.origin_z + (z as f32 + 0.5) * cell_size;
                let height = Self::descriptor_height_meters(vertical.height_u8[index]);
                let weight = (vertical_count + clutter_count).max(1.0);
                let vertical_mix = vertical_count / weight;
                let clutter_mix = clutter_count / weight;
                let alpha = (0.16 + 0.06 * weight.min(4.0)).clamp(0.16, 0.40);
                let color = vec4f(
                    0.14 + 0.18 * clutter_mix,
                    0.42 + 0.36 * clutter_mix,
                    0.96 - 0.48 * clutter_mix,
                    alpha,
                );
                let transform =
                    Pose::new(Quat::default(), vec3(center_x, height * 0.5, center_z)).to_mat4();
                self.draw_cube_at(
                    cx,
                    world,
                    &transform,
                    vec3(footprint, height, footprint),
                    vec4f(
                        color.x + 0.06 * vertical_mix,
                        color.y,
                        color.z + 0.04 * vertical_mix,
                        color.w,
                    ),
                );
            }
        }
    }

    fn draw_anchor_markers(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        anchor: XrAnchor,
        size: f32,
        left_color: Vec4f,
        right_color: Vec4f,
    ) {
        let left_transform = Pose::new(anchor.to_quat(), anchor.left).to_mat4();
        self.draw_cube_at(
            cx,
            world,
            &left_transform,
            vec3(size, size, size),
            left_color,
        );
        let right_transform = Pose::new(anchor.to_quat_rev(), anchor.right).to_mat4();
        self.draw_cube_at(
            cx,
            world,
            &right_transform,
            vec3(size, size, size),
            right_color,
        );
    }

    pub(super) fn draw_recent_anchor_confirmation(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let Some(confirmation) = self.runtime.recent_anchor_confirmation else {
            return;
        };
        if self.runtime.local.state_time > confirmation.visible_until {
            return;
        }
        self.draw_anchor_markers(
            cx,
            world,
            confirmation.anchor,
            Self::ANCHOR_MARKER_SIZE * 0.5,
            vec4f(1.0, 0.08, 0.08, 1.0),
            vec4f(1.0, 0.08, 0.08, 1.0),
        );
    }

    pub(super) fn draw_pending_sync_anchor_preview(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let now = self.runtime.local.state_time;
        let recent_local_sync_anchors: Vec<TimedLocalSyncAnchor> = self
            .runtime
            .local
            .recent_sync_anchors
            .iter()
            .copied()
            .collect();
        for recent_sync in recent_local_sync_anchors.iter().rev() {
            let age = now - recent_sync.last_seen_at_local_time;
            if !(0.0..=Self::LOCAL_SYNC_SAMPLE_PREVIEW_SECONDS).contains(&age) {
                continue;
            }
            let fade = (1.0 - age as f32 / Self::LOCAL_SYNC_SAMPLE_PREVIEW_SECONDS as f32)
                .clamp(0.18, 1.0);
            let (left_color, right_color) = match recent_sync.sync.extrema {
                XrSyncAnchorExtrema::High => (
                    vec4f(1.0, 0.86, 0.44, 0.94 * fade),
                    vec4f(0.44, 1.0, 0.90, 0.94 * fade),
                ),
                XrSyncAnchorExtrema::Low => (
                    vec4f(1.0, 0.46, 0.40, 0.94 * fade),
                    vec4f(0.40, 0.70, 1.0, 0.94 * fade),
                ),
            };
            self.draw_anchor_markers(
                cx,
                world,
                recent_sync.sync.anchor,
                Self::ANCHOR_MARKER_SIZE,
                left_color,
                right_color,
            );
        }

        let Some((peer_id, peer_state)) = self.runtime.registry.preferred_peer() else {
            return;
        };
        let peer_transform = self.peer_remote_to_local_transform(peer_id);
        let recent_remote_sync_anchors: Vec<TimedRemoteSyncAnchor> =
            peer_state.recent_sync_anchors.iter().copied().collect();
        for recent_sync in recent_remote_sync_anchors.iter().rev() {
            let age = now - recent_sync.last_seen_at_local_time;
            if !(0.0..=Self::LOCAL_SYNC_SAMPLE_PREVIEW_SECONDS).contains(&age) {
                continue;
            }
            let fade = (1.0 - age as f32 / Self::LOCAL_SYNC_SAMPLE_PREVIEW_SECONDS as f32)
                .clamp(0.18, 1.0);
            let (left_color, right_color) = match recent_sync.sync.extrema {
                XrSyncAnchorExtrema::High => (
                    vec4f(1.0, 0.72, 0.18, 0.80 * fade),
                    vec4f(0.18, 0.96, 0.82, 0.80 * fade),
                ),
                XrSyncAnchorExtrema::Low => (
                    vec4f(1.0, 0.32, 0.26, 0.80 * fade),
                    vec4f(0.26, 0.58, 1.0, 0.80 * fade),
                ),
            };
            self.draw_anchor_markers(
                cx,
                world,
                Self::transform_anchor(&peer_transform, recent_sync.sync.anchor.mirrored()),
                Self::ANCHOR_MARKER_SIZE,
                left_color,
                right_color,
            );
        }
    }

    fn draw_peer_joint_cube(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        root_transform: &Mat4f,
        position: Vec3f,
        size: f32,
        color: Vec4f,
    ) {
        let transform = Mat4f::mul(
            root_transform,
            &Pose::new(Quat::default(), position).to_mat4(),
        );
        self.draw_cube_at(cx, world, &transform, vec3(size, size, size), color);
    }

    fn peer_bone_pose(a: Vec3f, b: Vec3f) -> Option<(Pose, f32)> {
        let delta = b - a;
        let length = delta.length();
        if !length.is_finite() || length <= 0.0001 {
            return None;
        }
        let forward = delta.scale(1.0 / length);
        let up = if forward.y.abs() >= 0.92 {
            vec3f(1.0, 0.0, 0.0)
        } else {
            vec3f(0.0, 1.0, 0.0)
        };
        Some((
            Pose::new(Quat::look_rotation(forward, up), (a + b) * 0.5),
            length,
        ))
    }

    fn draw_peer_bone_box(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        root_transform: &Mat4f,
        a: Vec3f,
        b: Vec3f,
        thickness: f32,
        color: Vec4f,
    ) {
        let Some((pose, length)) = Self::peer_bone_pose(a, b) else {
            return;
        };
        let transform = Mat4f::mul(root_transform, &pose.to_mat4());
        self.draw_cube_at(
            cx,
            world,
            &transform,
            vec3(thickness, thickness, length),
            color,
        );
    }

    fn draw_peer_finger_chain(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        root_transform: &Mat4f,
        hand: &XrHand,
        chain: &[usize],
        tip: usize,
        thickness: f32,
        color: Vec4f,
    ) {
        let Some(mut points) = hand.joint_chain_positions(chain) else {
            return;
        };
        if let Some(tip_position) = hand.tip_pos_checked(tip) {
            points.push(tip_position);
        }
        for &point in points.iter() {
            self.draw_peer_joint_cube(
                cx,
                world,
                root_transform,
                point,
                Self::REMOTE_HAND_JOINT_SIZE.max(thickness * 1.25),
                color,
            );
        }
        for segment in points.windows(2) {
            self.draw_peer_bone_box(
                cx,
                world,
                root_transform,
                segment[0],
                segment[1],
                thickness,
                color,
            );
        }
    }

    fn draw_peer_hand_skeleton(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        root_transform: &Mat4f,
        hand: &XrHand,
        color: Vec4f,
    ) -> bool {
        if !hand.in_view() {
            return false;
        }
        if let Some(palm_pose) = hand.tracking_pose() {
            let transform = Mat4f::mul(root_transform, &palm_pose.to_mat4());
            self.draw_cube_at(cx, world, &transform, Self::REMOTE_HAND_PALM_SIZE, color);
        }
        for joint in [XrHand::WRIST, XrHand::CENTER] {
            if let Some(position) = hand.joint_pose_checked(joint).map(|pose| pose.position) {
                self.draw_peer_joint_cube(
                    cx,
                    world,
                    root_transform,
                    position,
                    Self::REMOTE_HAND_JOINT_SIZE * 1.15,
                    color,
                );
            }
        }
        self.draw_peer_finger_chain(
            cx,
            world,
            root_transform,
            hand,
            &[
                XrHand::THUMB_BASE,
                XrHand::THUMB_KNUCKLE1,
                XrHand::THUMB_KNUCKLE2,
            ],
            XrHand::THUMB_TIP,
            0.016,
            color,
        );
        self.draw_peer_finger_chain(
            cx,
            world,
            root_transform,
            hand,
            &[
                XrHand::INDEX_BASE,
                XrHand::INDEX_KNUCKLE1,
                XrHand::INDEX_KNUCKLE2,
                XrHand::INDEX_KNUCKLE3,
            ],
            XrHand::INDEX_TIP,
            0.014,
            color,
        );
        self.draw_peer_finger_chain(
            cx,
            world,
            root_transform,
            hand,
            &[
                XrHand::MIDDLE_BASE,
                XrHand::MIDDLE_KNUCKLE1,
                XrHand::MIDDLE_KNUCKLE2,
                XrHand::MIDDLE_KNUCKLE3,
            ],
            XrHand::MIDDLE_TIP,
            0.015,
            color,
        );
        self.draw_peer_finger_chain(
            cx,
            world,
            root_transform,
            hand,
            &[
                XrHand::RING_BASE,
                XrHand::RING_KNUCKLE1,
                XrHand::RING_KNUCKLE2,
                XrHand::RING_KNUCKLE3,
            ],
            XrHand::RING_TIP,
            0.014,
            color,
        );
        self.draw_peer_finger_chain(
            cx,
            world,
            root_transform,
            hand,
            &[
                XrHand::LITTLE_BASE,
                XrHand::LITTLE_KNUCKLE1,
                XrHand::LITTLE_KNUCKLE2,
                XrHand::LITTLE_KNUCKLE3,
            ],
            XrHand::LITTLE_TIP,
            0.013,
            color,
        );
        true
    }

    fn draw_peer_hand(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        root_transform: &Mat4f,
        hand: &XrHand,
        controller: &XrController,
        color: Vec4f,
    ) {
        if self.draw_peer_hand_skeleton(cx, world, root_transform, hand, color) {
            return;
        }
        let pose = if controller.active() {
            Some(controller.grip_pose)
        } else {
            None
        };
        let Some(pose) = pose else {
            return;
        };
        let transform = Mat4f::mul(root_transform, &pose.to_mat4());
        self.draw_cube_at(cx, world, &transform, Self::HAND_SIZE, color);
    }

    pub(super) fn draw_remote_peers(&mut self, cx: &mut Cx3d, world: &Mat4f) {
        let peer_ids = self.runtime.registry.peer_ids();
        for peer_id in peer_ids {
            let Some(peer) = self.runtime.registry.peers.get(&peer_id).cloned() else {
                continue;
            };
            let Some(state_frame) = peer.latest_state.as_ref() else {
                continue;
            };

            let alpha = Self::peer_alpha(&peer);
            let base = Self::peer_base_color(peer.peer.id);
            let root_transform = Self::peer_transform(&peer);
            let head_color = vec4f(base.x, base.y, base.z, alpha);
            let left_color = vec4f(
                (base.x * 0.72).min(1.0),
                (base.y * 1.05).min(1.0),
                1.0,
                alpha,
            );
            let right_color = vec4f(
                1.0,
                (base.y * 0.82).min(1.0),
                (base.z * 0.72).min(1.0),
                alpha,
            );

            let head_pose = state_frame.state.head_pose;
            self.draw_cube_at(
                cx,
                world,
                &Mat4f::mul(&root_transform, &head_pose.to_mat4()),
                Self::HEADSET_SIZE,
                head_color,
            );
            self.draw_peer_hand(
                cx,
                world,
                &root_transform,
                &state_frame.state.left_hand,
                &state_frame.state.left_controller,
                left_color,
            );
            self.draw_peer_hand(
                cx,
                world,
                &root_transform,
                &state_frame.state.right_hand,
                &state_frame.state.right_controller,
                right_color,
            );
        }
    }
}
