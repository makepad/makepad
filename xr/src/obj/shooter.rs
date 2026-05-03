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
const SHOOTER_INDEX_TIP_FALLBACK_EXTENSION_METERS: f32 = 0.035;
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

    fn hand_index_finger_emit_points(hand: &XrHand) -> Option<SmallVec<[Vec3f; 5]>> {
        let mut points = SmallVec::<[Vec3f; 5]>::new();
        points.extend(hand.finger_chain_positions_partial(XrHand::INDEX_TIP)?);
        if points.len() < 2 {
            return None;
        }
        if let Some(tip) = hand.tip_pos_checked(XrHand::INDEX_TIP) {
            if (tip - *points.last()?).length() > 0.0001 {
                points.push(tip);
                return (points.len() >= 3).then_some(points);
            }
        }
        let prev = points[points.len() - 2];
        let last = points[points.len() - 1];
        let direction = Self::normalized_segment_direction(prev, last)?;
        points.push(last + direction * SHOOTER_INDEX_TIP_FALLBACK_EXTENSION_METERS);
        (points.len() >= 3).then_some(points)
    }

    fn hand_index_finger_stretch_metrics_for_points(
        points: &[Vec3f],
    ) -> Option<ShooterHandEmitMetrics> {
        let mut chain_length = 0.0;
        let mut directions = SmallVec::<[Vec3f; 4]>::new();
        for segment in points.windows(2) {
            let delta = segment[1] - segment[0];
            let length = delta.length();
            if !length.is_finite() || length <= 0.0001 {
                return None;
            }
            chain_length += length;
            directions.push(delta / length);
        }

        if chain_length <= 0.0001 {
            return None;
        }
        let direct_length = (*points.last()? - points[0]).length();
        let mut max_bend_angle_degrees = 0.0;
        let mut straightness: f32 = 1.0;
        for pair in directions.windows(2) {
            let dot = pair[0].dot(pair[1]).clamp(-1.0, 1.0);
            max_bend_angle_degrees = max_bend_angle_degrees.max(dot.acos().to_degrees());
            straightness = straightness.min(dot);
        }
        Some(ShooterHandEmitMetrics {
            max_bend_angle_degrees,
            straightness,
            extension_ratio: direct_length / chain_length,
        })
    }

    #[cfg(test)]
    fn hand_index_finger_stretch_metrics(hand: &XrHand) -> Option<ShooterHandEmitMetrics> {
        let points = Self::hand_index_finger_emit_points(hand)?;
        Self::hand_index_finger_stretch_metrics_for_points(&points)
    }

    fn hand_index_forward_direction_for_points(points: &[Vec3f]) -> Option<Vec3f> {
        let mut blended = vec3f(0.0, 0.0, 0.0);
        let mut weight_sum = 0.0;
        for (index, segment) in points.windows(2).enumerate() {
            let direction = Self::normalized_segment_direction(segment[0], segment[1])?;
            let weight = (index + 1) as f32;
            blended += direction * weight;
            weight_sum += weight;
        }
        if weight_sum > 0.0 {
            blended /= weight_sum;
        }
        if blended.length() > 0.0001 {
            return Some(blended.normalize());
        }
        Self::normalized_segment_direction(points[0], *points.last()?)
    }

    fn hand_emit_metrics_active(metrics: ShooterHandEmitMetrics) -> bool {
        metrics.max_bend_angle_degrees <= SHOOTER_INDEX_BEND_MAX_DEGREES
            && metrics.straightness >= SHOOTER_INDEX_STRAIGHTNESS_MIN
            && metrics.extension_ratio >= SHOOTER_INDEX_EXTENSION_RATIO_MIN
    }

    #[cfg(test)]
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
        Self::hand_emit_metrics_active(metrics)
    }

    fn projectile_emitter_pose(hand: &XrHand) -> Option<(Vec3f, Vec3f)> {
        if !hand.in_view() {
            return None;
        }
        let points = Self::hand_index_finger_emit_points(hand)?;
        let metrics = Self::hand_index_finger_stretch_metrics_for_points(&points)?;
        if !Self::hand_emit_metrics_active(metrics) {
            return None;
        }
        let tip_position = points.last().copied()?;
        let direction = Self::hand_index_forward_direction_for_points(&points)?;
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
include!("../tests/obj/shooter.rs");
