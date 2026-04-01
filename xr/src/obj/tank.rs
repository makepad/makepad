use crate::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct TankDriveConfig {
    pub drive_impulse_per_second: f32,
    pub turn_gain: f32,
    pub max_speed_mps: f32,
    pub stick_deadzone: f32,
    pub track_half_width: f32,
}

impl Default for TankDriveConfig {
    fn default() -> Self {
        Self {
            drive_impulse_per_second: 82.0,
            turn_gain: 1.2,
            max_speed_mps: 2.4,
            stick_deadzone: 0.16,
            track_half_width: 0.16,
        }
    }
}

pub fn script_mod(_vm: &mut ScriptVm) -> ScriptValue {
    NIL
}

pub fn tank_drive_impulses(
    widget_uid: WidgetUid,
    pose: Pose,
    linvel: Vec3f,
    held_by: Option<XrSharedHand>,
    controller: &XrController,
    dt: f32,
    config: TankDriveConfig,
) -> Vec<XrBodyImpulse> {
    if held_by.is_some() || !dt.is_finite() || dt <= 0.0 {
        return Vec::new();
    }
    let (forward, turn) = stick_deadzone_scaled_axes(controller.stick, config.stick_deadzone);
    if forward.abs() <= 1.0e-4 && turn.abs() <= 1.0e-4 {
        return Vec::new();
    }
    let (left, right) = differential_track_commands(
        pose,
        linvel,
        forward,
        turn,
        config.turn_gain,
        config.max_speed_mps,
    );
    emit_drive_impulses(widget_uid, dt, left, right, pose, config)
}

fn stick_deadzone_scaled_axes(stick: Vec2f, deadzone: f32) -> (f32, f32) {
    (
        deadzone_scaled_axis(stick.y, deadzone),
        deadzone_scaled_axis(stick.x, deadzone),
    )
}

fn deadzone_scaled_axis(value: f32, deadzone: f32) -> f32 {
    let deadzone = deadzone.clamp(0.0, 0.95);
    if !value.is_finite() {
        return 0.0;
    }
    let magnitude = value.abs();
    if magnitude <= deadzone {
        return 0.0;
    }
    let scaled = ((magnitude - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0);
    scaled.copysign(value)
}

fn differential_track_commands(
    pose: Pose,
    linvel: Vec3f,
    forward_input: f32,
    turn_input: f32,
    turn_gain: f32,
    max_speed_mps: f32,
) -> (f32, f32) {
    let body_forward = flat_forward(pose.orientation);
    let mut forward_cmd = forward_input.clamp(-1.0, 1.0);
    let turn_cmd = turn_input.clamp(-1.0, 1.0) * turn_gain.max(0.0);
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

fn emit_drive_impulses(
    widget_uid: WidgetUid,
    dt: f32,
    left: f32,
    right: f32,
    pose: Pose,
    config: TankDriveConfig,
) -> Vec<XrBodyImpulse> {
    let impulse_scale =
        config.drive_impulse_per_second.max(0.0) * dt.clamp(1.0 / 240.0, 0.08);
    if impulse_scale <= 0.0 {
        return Vec::new();
    }
    let body_forward = flat_forward(pose.orientation);
    let body_right = flat_right(pose.orientation);
    let track_half_width = config.track_half_width.clamp(0.03, 0.24);
    let left_point = pose.position - body_right * track_half_width;
    let right_point = pose.position + body_right * track_half_width;
    let mut impulses = Vec::with_capacity(2);

    if left.abs() > 0.0001 {
        impulses.push(XrBodyImpulse {
            widget_uid,
            point: left_point,
            impulse: body_forward * (left * impulse_scale),
        });
    }
    if right.abs() > 0.0001 {
        impulses.push(XrBodyImpulse {
            widget_uid,
            point: right_point,
            impulse: body_forward * (right * impulse_scale),
        });
    }
    impulses
}

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

#[cfg(test)]
include!("../tests/obj/tank.rs");
