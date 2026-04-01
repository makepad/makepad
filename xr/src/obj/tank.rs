use crate::prelude::*;
use crate::scene::{xr_widget_world_transform, XrDrawContext, XrNode};
use crate::util::scene_draw::{apply_scene_to_draw_cube, compose_scene_node_transform};
use makepad_widgets::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::{ScriptAsyncId, ScriptAsyncResult},
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.TankBase = #(Tank::register_widget(vm))
    mod.widgets.Tank = set_type_default() do mod.widgets.TankBase{
        body: mod.widgets.XrBodyKind.Dynamic
        shared_object_policy: mod.widgets.XrSharedObjectPolicy.BootstrapShared
        hull_size: vec3(0.24, 0.08, 0.34)
        turret_size: vec3(0.13, 0.06, 0.13)
        barrel_size: vec3(0.045, 0.028, 0.19)
        hull_color: vec4(0.33, 0.42, 0.28, 1.0)
        turret_color: vec4(0.40, 0.49, 0.34, 1.0)
        barrel_color: vec4(0.18, 0.20, 0.16, 1.0)
        density: 1.2
        friction: 1.3
        restitution: 0.02
        drive_impulse_per_second: 1.6
        turn_gain: 1.35
        max_speed_mps: 1.45
        stick_deadzone: 0.16
        draw_hull +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_turret +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_barrel +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

#[derive(Script, Widget)]
pub struct Tank {
    #[redraw]
    #[live]
    draw_hull: DrawCube,
    #[redraw]
    #[live]
    draw_turret: DrawCube,
    #[redraw]
    #[live]
    draw_barrel: DrawCube,
    #[live(vec3(0.24, 0.08, 0.34))]
    hull_size: Vec3f,
    #[live(vec3(0.13, 0.06, 0.13))]
    turret_size: Vec3f,
    #[live(vec3(0.045, 0.028, 0.19))]
    barrel_size: Vec3f,
    #[live(vec4(0.33, 0.42, 0.28, 1.0))]
    hull_color: Vec4f,
    #[live(vec4(0.40, 0.49, 0.34, 1.0))]
    turret_color: Vec4f,
    #[live(vec4(0.18, 0.20, 0.16, 1.0))]
    barrel_color: Vec4f,
    #[live(1.6)]
    drive_impulse_per_second: f32,
    #[live(1.35)]
    turn_gain: f32,
    #[live(1.45)]
    max_speed_mps: f32,
    #[live(0.16)]
    stick_deadzone: f32,
    #[rust]
    cached_runtime_body: Option<TankRuntimeBodyCache>,
    #[cast]
    #[deref]
    node: XrNode,
}

#[derive(Clone, Copy, Debug)]
struct TankRuntimeBodyCache {
    pose: Pose,
    linvel: Vec3f,
    held_by: Option<XrSharedHand>,
}

fn stick_deadzone_scaled_direction(
    controller: &XrController,
    head_orientation: Quat,
    deadzone: f32,
) -> Option<(Vec3f, f32)> {
    if !controller.active() {
        return None;
    }
    let stick = controller.stick;
    let magnitude = stick.length();
    let deadzone = deadzone.clamp(0.0, 0.95);
    if !magnitude.is_finite() || magnitude <= deadzone {
        return None;
    }
    let forward = Tank::flat_forward(head_orientation);
    let right = Tank::flat_right(head_orientation);
    let direction = right * stick.x + forward * stick.y;
    if direction.length() <= 1.0e-6 {
        return None;
    }
    let scaled = ((magnitude - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0);
    Some((direction.normalize(), scaled))
}

fn differential_track_commands(
    pose: Pose,
    linvel: Vec3f,
    desired_direction: Vec3f,
    input_amount: f32,
    turn_gain: f32,
    max_speed_mps: f32,
) -> (f32, f32) {
    let body_forward = Tank::flat_forward(pose.orientation);
    let body_right = Tank::flat_right(pose.orientation);
    let mut forward_cmd = body_forward.dot(desired_direction) * input_amount;
    let turn_cmd = body_right.dot(desired_direction) * turn_gain.max(0.0) * input_amount;
    let max_speed = max_speed_mps.max(0.05);
    let mut flat_velocity = linvel;
    flat_velocity.y = 0.0;
    let forward_speed = flat_velocity.dot(body_forward);

    if forward_cmd.signum() == forward_speed.signum() {
        let speed_ratio = (forward_speed.abs() / max_speed).clamp(0.0, 1.2);
        forward_cmd *= (1.0 - speed_ratio).clamp(0.0, 1.0);
    }

    let mut left = forward_cmd - turn_cmd;
    let mut right = forward_cmd + turn_cmd;
    let max_component = left.abs().max(right.abs()).max(1.0);
    left /= max_component;
    right /= max_component;
    (left, right)
}

impl Tank {
    fn flat_forward(orientation: Quat) -> Vec3f {
        let mut forward = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-6 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        }
    }

    fn flat_right(orientation: Quat) -> Vec3f {
        let mut right = orientation.rotate_vec3(&vec3f(1.0, 0.0, 0.0));
        right.y = 0.0;
        if right.length() <= 1.0e-6 {
            vec3f(1.0, 0.0, 0.0)
        } else {
            right.normalize()
        }
    }

    fn physics_size(&self) -> Vec3f {
        vec3f(
            self.hull_size.x.max(0.0),
            (self.hull_size.y + self.turret_size.y * 0.55).max(0.0),
            self.hull_size.z.max(0.0),
        )
    }

    fn head_relative_drive_direction(
        &self,
        controller: &XrController,
        head_orientation: Quat,
    ) -> Option<(Vec3f, f32)> {
        stick_deadzone_scaled_direction(controller, head_orientation, self.stick_deadzone)
    }

    fn track_commands(
        &self,
        pose: Pose,
        linvel: Vec3f,
        desired_direction: Vec3f,
        input_amount: f32,
    ) -> (f32, f32) {
        differential_track_commands(
            pose,
            linvel,
            desired_direction,
            input_amount,
            self.turn_gain,
            self.max_speed_mps,
        )
    }

    fn emit_drive_impulses(&mut self, cx: &mut Cx, dt: f32, left: f32, right: f32, pose: Pose) {
        let impulse_scale = self.drive_impulse_per_second.max(0.0) * dt.clamp(1.0 / 240.0, 0.08);
        if impulse_scale <= 0.0 {
            return;
        }
        let body_forward = Self::flat_forward(pose.orientation);
        let body_right = Self::flat_right(pose.orientation);
        let track_half_width = (self.hull_size.x * 0.34).clamp(0.03, 0.20);
        let left_point = pose.position - body_right * track_half_width;
        let right_point = pose.position + body_right * track_half_width;

        if left.abs() > 0.0001 {
            cx.widget_action(
                self.widget_uid(),
                XrBodyImpulse {
                    widget_uid: self.widget_uid(),
                    point: left_point,
                    impulse: body_forward * (left * impulse_scale),
                },
            );
        }
        if right.abs() > 0.0001 {
            cx.widget_action(
                self.widget_uid(),
                XrBodyImpulse {
                    widget_uid: self.widget_uid(),
                    point: right_point,
                    impulse: body_forward * (right * impulse_scale),
                },
            );
        }
    }

    fn handle_xr_update(&mut self, cx: &mut Cx, update: &XrUpdateEvent) {
        let Some(runtime_body) = self.cached_runtime_body else {
            return;
        };
        if runtime_body.held_by.is_some() {
            return;
        }
        let Some((desired_direction, input_amount)) = self.head_relative_drive_direction(
            &update.state.right_controller,
            update.state.head_pose.orientation,
        ) else {
            return;
        };
        let dt = (update.state.time - update.last.time) as f32;
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let (left, right) = self.track_commands(
            runtime_body.pose,
            runtime_body.linvel,
            desired_direction,
            input_amount,
        );
        self.emit_drive_impulses(cx, dt, left, right, runtime_body.pose);
    }

    fn cache_runtime_body_state(&mut self, scope: &mut Scope) {
        self.cached_runtime_body = XrDrawContext::from_scope(scope)
            .runtime_body(self.widget_uid())
            .map(|body| TankRuntimeBodyCache {
                pose: body.pose,
                linvel: body.linvel,
                held_by: body.held_by,
            });
    }

    fn draw_part(
        draw_cube: &mut DrawCube,
        cx: &mut Cx3d,
        transform: Mat4f,
        size: Vec3f,
        color: Vec4f,
    ) {
        draw_cube.transform = transform;
        draw_cube.cube_pos = vec3f(0.0, 0.0, 0.0);
        draw_cube.cube_size = size;
        draw_cube.color = color;
        draw_cube.depth_clip = 1.0;
        draw_cube.draw(cx);
    }
}

impl ScriptHook for Tank {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.node.set_implicit_physics_size(self.physics_size());
    }
}

impl Widget for Tank {
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
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::XrUpdate(update) = event {
            self.handle_xr_update(cx, update);
        }
        self.node.handle_event(cx, event, scope);
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(_scene) = apply_scene_to_draw_cube(&mut self.draw_hull, cx) else {
            return DrawStep::done();
        };
        let _ = apply_scene_to_draw_cube(&mut self.draw_turret, cx);
        let _ = apply_scene_to_draw_cube(&mut self.draw_barrel, cx);

        self.cache_runtime_body_state(scope);

        let tank_world = xr_widget_world_transform(cx, scope, self.widget_uid(), &self.node);
        let turret_transform = Mat4f::mul(
            &tank_world,
            &compose_scene_node_transform(
                vec3f(0.0, self.hull_size.y * 0.42 + self.turret_size.y * 0.48, 0.0),
                vec3f(0.0, 0.0, 0.0),
                vec3f(1.0, 1.0, 1.0),
            ),
        );
        let barrel_transform = Mat4f::mul(
            &tank_world,
            &compose_scene_node_transform(
                vec3f(
                    0.0,
                    self.hull_size.y * 0.44 + self.turret_size.y * 0.26,
                    -(self.turret_size.z * 0.42 + self.barrel_size.z * 0.46),
                ),
                vec3f(0.0, 0.0, 0.0),
                vec3f(1.0, 1.0, 1.0),
            ),
        );

        Self::draw_part(
            &mut self.draw_hull,
            cx,
            tank_world,
            self.hull_size,
            self.hull_color,
        );
        Self::draw_part(
            &mut self.draw_turret,
            cx,
            turret_transform,
            self.turret_size,
            self.turret_color,
        );
        Self::draw_part(
            &mut self.draw_barrel,
            cx,
            barrel_transform,
            self.barrel_size,
            self.barrel_color,
        );

        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

#[cfg(test)]
include!("../tests/obj/tank.rs");
