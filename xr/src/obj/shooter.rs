use crate::prelude::XrSharedObjectMode;
use crate::scene::{xr_widget_children, xr_widget_with_scene_node, XrBodySpawn, XrNode};
use makepad_widgets::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::{ScriptAsyncId, ScriptAsyncResult},
};

#[derive(Clone, Copy, Debug)]
pub struct XrProjectileEmitterConfig {
    pub rate_hz: f32,
    pub speed_mps: f32,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.ShooterBase = #(Shooter::register_widget(vm))
    mod.widgets.Shooter = set_type_default() do mod.widgets.ShooterBase{
        body: mod.widgets.XrBodyKind.Disabled
        projectile_emit_rate_hz: 4.0
        projectile_emit_speed_mps: 10.0
    }
}

#[derive(Script, Widget)]
pub struct Shooter {
    #[live(4.0)]
    projectile_emit_rate_hz: f32,
    #[live(10.0)]
    projectile_emit_speed_mps: f32,
    #[rust]
    projectile_pool_uids: Vec<WidgetUid>,
    #[rust]
    projectile_cursor: usize,
    #[rust]
    projectile_next_emit_at: Option<f64>,
    #[cast]
    #[deref]
    node: XrNode,
}

const SHOOTER_MAX_EMITS_PER_UPDATE: usize = 2;
const SHOOTER_PROJECTILE_SPAWN_OFFSET: f32 = 0.064;
const SHOOTER_INDEX_BEND_MAX_DEGREES: f32 = 34.0;
const SHOOTER_INDEX_STRAIGHTNESS_MIN: f32 = 0.78;
const SHOOTER_INDEX_EXTENSION_RATIO_MIN: f32 = 0.90;

#[derive(Clone, Copy)]
struct ShooterHandEmitMetrics {
    max_bend_angle_degrees: f32,
    straightness: f32,
    extension_ratio: f32,
}

impl Shooter {
    pub fn emitter_config(&self) -> XrProjectileEmitterConfig {
        XrProjectileEmitterConfig {
            rate_hz: self.projectile_emit_rate_hz(),
            speed_mps: self.projectile_emit_speed_mps(),
        }
    }

    pub fn projectile_emit_rate_hz(&self) -> f32 {
        self.projectile_emit_rate_hz.max(0.0)
    }

    pub fn projectile_emit_speed_mps(&self) -> f32 {
        self.projectile_emit_speed_mps.max(0.0)
    }

    pub fn node(&self) -> &XrNode {
        &self.node
    }

    fn refresh_projectile_pool(&mut self) {
        self.projectile_pool_uids.clear();
        self.node.children(&mut |_, child| {
            Self::collect_projectile_pool(&child, &mut self.projectile_pool_uids)
        });
        if self.projectile_pool_uids.is_empty() {
            self.projectile_cursor = 0;
        } else {
            self.projectile_cursor %= self.projectile_pool_uids.len();
        }
    }

    fn collect_projectile_pool(widget: &WidgetRef, projectile_pool_uids: &mut Vec<WidgetUid>) {
        if !widget.visible() {
            return;
        }
        if let Some(node) = xr_widget_with_scene_node(widget, |node| {
            if node.spawn_pool() {
                projectile_pool_uids.push(widget.widget_uid());
            }
        }) {
            let _ = node;
        }
        xr_widget_children(widget, &mut |_, child| {
            Self::collect_projectile_pool(&child, projectile_pool_uids)
        });
    }

    fn next_projectile_widget_uid(&mut self) -> Option<WidgetUid> {
        if self.projectile_pool_uids.is_empty() {
            self.refresh_projectile_pool();
        }
        let len = self.projectile_pool_uids.len();
        if len == 0 {
            return None;
        }
        let widget_uid = self.projectile_pool_uids[self.projectile_cursor % len];
        self.projectile_cursor = (self.projectile_cursor + 1) % len;
        Some(widget_uid)
    }

    fn normalized_segment_direction(a: Vec3f, b: Vec3f) -> Option<Vec3f> {
        let delta = b - a;
        (delta.length() > 0.0001).then_some(delta.normalize())
    }

    fn hand_index_finger_stretch_metrics(hand: &XrHand) -> Option<ShooterHandEmitMetrics> {
        let points = hand.finger_chain_positions(XrHand::INDEX_TIP)?;
        let [base, knuckle1, knuckle2, knuckle3, tip] =
            [points[0], points[1], points[2], points[3], points[4]];

        let Some(seg0) = Self::normalized_segment_direction(base, knuckle1) else {
            return None;
        };
        let Some(seg1) = Self::normalized_segment_direction(knuckle1, knuckle2) else {
            return None;
        };
        let Some(seg2) = Self::normalized_segment_direction(knuckle2, knuckle3) else {
            return None;
        };
        let Some(seg3) = Self::normalized_segment_direction(knuckle3, tip) else {
            return None;
        };

        let chain_length = (knuckle1 - base).length()
            + (knuckle2 - knuckle1).length()
            + (knuckle3 - knuckle2).length()
            + (tip - knuckle3).length();
        if chain_length <= 0.0001 {
            return None;
        }
        let direct_length = (tip - base).length();
        let segment_dots = [seg0.dot(seg1), seg1.dot(seg2), seg2.dot(seg3)];
        let max_bend_angle_degrees = segment_dots
            .into_iter()
            .map(|dot| dot.clamp(-1.0, 1.0).acos().to_degrees())
            .fold(0.0, f32::max);
        let straightness = segment_dots.into_iter().fold(1.0, f32::min);
        Some(ShooterHandEmitMetrics {
            max_bend_angle_degrees,
            straightness,
            extension_ratio: direct_length / chain_length,
        })
    }

    fn hand_emit_gesture_active(hand: &XrHand) -> bool {
        // Shooter owns its own firing gesture semantics. Do not couple emission to the
        // generic OpenXR hand-grab bit, which represents a broader whole-hand intent and
        // can stay active across close/open transitions on device.
        if !hand.in_view() {
            return false;
        }
        let Some(metrics) = Self::hand_index_finger_stretch_metrics(hand) else {
            return false;
        };
        metrics.max_bend_angle_degrees <= SHOOTER_INDEX_BEND_MAX_DEGREES
            && metrics.straightness >= SHOOTER_INDEX_STRAIGHTNESS_MIN
            && metrics.extension_ratio >= SHOOTER_INDEX_EXTENSION_RATIO_MIN
    }

    fn projectile_emitter_pose(hand: &XrHand) -> Option<(Vec3f, Vec3f)> {
        if !Self::hand_emit_gesture_active(hand) {
            return None;
        }
        let tip_position = hand.tip_pos_checked(XrHand::INDEX_TIP)?;
        let direction = if hand.aim_valid() && hand.aim_pose.is_finite() {
            hand.aim_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0))
        } else {
            tip_position - hand.joint_pose_checked(XrHand::INDEX_KNUCKLE3)?.position
        };
        let direction = if direction.length() > 0.0001 {
            direction.normalize()
        } else {
            hand.joint_pose_checked(XrHand::INDEX_KNUCKLE3)?
                .orientation
                .rotate_vec3(&vec3f(0.0, 0.0, -1.0))
                .normalize()
        };
        Some((tip_position, direction))
    }

    fn main_projectile_emitter_pose(update: &XrUpdateEvent) -> Option<(Vec3f, Vec3f)> {
        let left = Self::projectile_emitter_pose(&update.state.left_hand);
        let right = Self::projectile_emitter_pose(&update.state.right_hand);
        match (
            left,
            right,
            update.state.left_hand.dominant_hand(),
            update.state.right_hand.dominant_hand(),
        ) {
            (Some(left), Some(_), true, false) => Some(left),
            (Some(_), Some(right), false, true) => Some(right),
            (_, Some(right), _, _) => Some(right),
            (Some(left), None, _, _) => Some(left),
            (None, None, _, _) => None,
        }
    }

    fn emit_projectiles_for_update(&mut self, cx: &mut Cx, update: &XrUpdateEvent) {
        let emitter = self.emitter_config();
        if emitter.rate_hz <= 0.0 || emitter.speed_mps <= 0.0 {
            self.projectile_next_emit_at = None;
            return;
        }
        let emitter_pose = Self::main_projectile_emitter_pose(update);
        let Some((tip_position, direction)) = emitter_pose else {
            self.projectile_next_emit_at = None;
            return;
        };

        let interval = (1.0 / emitter.rate_hz).clamp(0.01, 10.0) as f64;
        let now = update.state.time;
        let mut next_emit_at = self.projectile_next_emit_at.unwrap_or(now);
        let mut emitted = 0usize;

        while now >= next_emit_at && emitted < SHOOTER_MAX_EMITS_PER_UPDATE {
            let Some(widget_uid) = self.next_projectile_widget_uid() else {
                self.projectile_next_emit_at = None;
                return;
            };
            cx.widget_action(
                self.widget_uid(),
                XrBodySpawn {
                    widget_uid,
                    shadow: false,
                    mode: XrSharedObjectMode::Dynamic,
                    pose: Pose::new(
                        Quat::default(),
                        tip_position + direction * SHOOTER_PROJECTILE_SPAWN_OFFSET,
                    ),
                    linvel: direction * emitter.speed_mps,
                    angvel: vec3f(0.0, 0.0, 0.0),
                },
            );
            cx.redraw_all();
            next_emit_at += interval;
            emitted += 1;
        }

        self.projectile_next_emit_at = Some(next_emit_at);
    }
}

impl ScriptHook for Shooter {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.refresh_projectile_pool();
    }
}

impl Widget for Shooter {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        self.node.script_call(vm, method, args)
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        self.node.script_result(vm, id, result);
        self.refresh_projectile_pool();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::XrUpdate(update) = event {
            self.emit_projectiles_for_update(cx, update);
        }
        self.node.handle_event(cx, event, scope);
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn point_pose(base: Vec3f, z: f32) -> Pose {
        Pose::new(Quat::default(), base + vec3f(0.0, 0.0, z))
    }

    fn make_pointing_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID | XrHand::DOMINANT_HAND;
        hand.tips_active = 1 << XrHand::INDEX_TIP;
        hand.tips[XrHand::INDEX_TIP] = 0.038;

        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.joints[XrHand::INDEX_BASE] = point_pose(base, 0.0);
        hand.joints[XrHand::INDEX_KNUCKLE1] = point_pose(base, -0.041);
        hand.joints[XrHand::INDEX_KNUCKLE2] = point_pose(base + vec3f(0.001, 0.002, 0.0), -0.082);
        hand.joints[XrHand::INDEX_KNUCKLE3] = point_pose(base + vec3f(0.002, 0.004, 0.0), -0.122);
        hand.aim_pose = Pose::new(Quat::default(), base + vec3f(0.0, 0.0, -0.16));
        hand
    }

    fn make_curled_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = 1 << XrHand::INDEX_TIP;
        hand.tips[XrHand::INDEX_TIP] = 0.030;

        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.joints[XrHand::INDEX_BASE] = point_pose(base, 0.0);
        hand.joints[XrHand::INDEX_KNUCKLE1] = point_pose(base, -0.030);
        hand.joints[XrHand::INDEX_KNUCKLE2] =
            Pose::new(Quat::default(), base + vec3f(0.018, -0.012, -0.040));
        hand.joints[XrHand::INDEX_KNUCKLE3] =
            Pose::new(Quat::default(), base + vec3f(0.034, -0.030, -0.032));
        hand.aim_pose = Pose::new(Quat::default(), base + vec3f(0.0, 0.0, -0.12));
        hand
    }

    fn make_sparse_tracking_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.aim_pose = Pose::new(Quat::default(), base + vec3f(0.0, 0.0, -0.12));
        hand
    }

    #[test]
    fn point_gesture_is_considered_an_emit_gesture() {
        let hand = make_pointing_hand();
        assert!(Shooter::hand_emit_gesture_active(&hand));
        assert!(Shooter::projectile_emitter_pose(&hand).is_some());
    }

    #[test]
    fn generic_grab_bit_does_not_block_emit_gesture_when_pointing_pose_is_valid() {
        let mut hand = make_pointing_hand();
        hand.tips_active |= XrHand::GRAB_ACTIVE;
        assert!(Shooter::hand_emit_gesture_active(&hand));
        assert!(Shooter::projectile_emitter_pose(&hand).is_some());
    }

    #[test]
    fn point_gesture_without_tip_active_bit_still_emits_from_joint_chain() {
        let mut hand = make_pointing_hand();
        hand.tips_active &= !(1 << XrHand::INDEX_TIP);
        let metrics = Shooter::hand_index_finger_stretch_metrics(&hand)
            .expect("joint-chain metrics should still be derivable without the tip-active bit");
        assert!(metrics.max_bend_angle_degrees <= SHOOTER_INDEX_BEND_MAX_DEGREES);
        assert!(Shooter::hand_emit_gesture_active(&hand));
        assert!(Shooter::projectile_emitter_pose(&hand).is_some());
    }

    #[test]
    fn curled_index_finger_is_rejected() {
        let hand = make_curled_hand();
        assert!(
            Shooter::hand_index_finger_stretch_metrics(&hand)
                .is_some_and(|metrics| {
                    metrics.max_bend_angle_degrees > SHOOTER_INDEX_BEND_MAX_DEGREES
                })
        );
        assert!(!Shooter::hand_emit_gesture_active(&hand));
    }

    #[test]
    fn sparse_tracking_sample_is_rejected_for_emit_gesture() {
        let hand = make_sparse_tracking_hand();
        assert!(Shooter::hand_index_finger_stretch_metrics(&hand).is_none());
        assert!(!Shooter::hand_emit_gesture_active(&hand));
        assert!(Shooter::projectile_emitter_pose(&hand).is_none());
    }

    #[test]
    fn dominant_hand_selection_prefers_pointing_hand() {
        let mut update = XrUpdateEvent {
            state: Rc::new(XrState::default()),
            last: Rc::new(XrState::default()),
        };
        let mut left = make_pointing_hand();
        left.flags |= XrHand::DOMINANT_HAND;
        let right = make_curled_hand();
        Rc::make_mut(&mut update.state).left_hand = left;
        Rc::make_mut(&mut update.state).right_hand = right;
        let emitter = Shooter::main_projectile_emitter_pose(&update);
        assert!(emitter.is_some());
        let (pos, dir) = emitter.unwrap();
        assert!(pos.z < -0.30);
        assert!(dir.z < -0.8);
    }
}
