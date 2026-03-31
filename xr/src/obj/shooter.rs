use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::{ScriptAsyncId, ScriptAsyncResult},
    XrBodySpawn,
};

use super::XrNode;

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
        if let Some(node) = widget.cast_inner::<XrNode>() {
            if node.projectile_pool() {
                projectile_pool_uids.push(widget.widget_uid());
            }
        }
        widget
            .children(&mut |_, child| Self::collect_projectile_pool(&child, projectile_pool_uids));
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

    fn hand_index_finger_straight(hand: &XrHand) -> bool {
        if !hand.in_view() || !hand.tip_active(XrHand::INDEX_TIP) {
            return false;
        }

        let base = hand.joints[XrHand::INDEX_BASE].position;
        let knuckle1 = hand.joints[XrHand::INDEX_KNUCKLE1].position;
        let knuckle2 = hand.joints[XrHand::INDEX_KNUCKLE2].position;
        let knuckle3 = hand.joints[XrHand::INDEX_KNUCKLE3].position;
        let tip = hand.tip_pos_index();

        let Some(seg0) = Self::normalized_segment_direction(base, knuckle1) else {
            return false;
        };
        let Some(seg1) = Self::normalized_segment_direction(knuckle1, knuckle2) else {
            return false;
        };
        let Some(seg2) = Self::normalized_segment_direction(knuckle2, knuckle3) else {
            return false;
        };
        let Some(seg3) = Self::normalized_segment_direction(knuckle3, tip) else {
            return false;
        };

        let chain_length = (knuckle1 - base).length()
            + (knuckle2 - knuckle1).length()
            + (knuckle3 - knuckle2).length()
            + (tip - knuckle3).length();
        let direct_length = (tip - base).length();
        if chain_length <= 0.0001 {
            return false;
        }

        let straightness = seg0.dot(seg1).min(seg1.dot(seg2)).min(seg2.dot(seg3));
        let extension_ratio = direct_length / chain_length;
        straightness >= 0.92 && extension_ratio >= 0.96
    }

    fn projectile_emitter_pose(hand: &XrHand) -> Option<(Vec3f, Vec3f)> {
        if !Self::hand_index_finger_straight(hand) {
            return None;
        }
        let tip_position = hand.tip_pos_index();
        let knuckle_position = hand.joints[XrHand::INDEX_KNUCKLE3].position;
        let direction = tip_position - knuckle_position;
        let direction = if direction.length() > 0.0001 {
            direction.normalize()
        } else {
            hand.joints[XrHand::INDEX_KNUCKLE3]
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
