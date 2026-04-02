use crate::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct TankDriveConfig {
    pub turn_gain: f32,
    pub max_speed_mps: f32,
    pub max_yaw_speed_radps: f32,
    pub max_linear_accel_mps2: f32,
    pub max_angular_accel_radps2: f32,
    pub stick_deadzone: f32,
    pub stick_response_exponent: f32,
}

impl Default for TankDriveConfig {
    fn default() -> Self {
        Self {
            turn_gain: 0.32,
            max_speed_mps: 0.72,
            max_yaw_speed_radps: 1.25,
            max_linear_accel_mps2: 1.8,
            max_angular_accel_radps2: 3.2,
            stick_deadzone: 0.24,
            stick_response_exponent: 1.75,
        }
    }
}

pub fn script_mod(_vm: &mut ScriptVm) -> ScriptValue {
    NIL
}

pub fn tank_drive_command(
    widget_uid: WidgetUid,
    pose: Pose,
    held_by: Option<XrSharedHand>,
    controller: &XrController,
    config: TankDriveConfig,
) -> Option<XrBodyDrive> {
    if held_by.is_some() {
        return None;
    }
    let (forward, turn) = tank_stick_axes(controller.stick, config);
    let body_forward = flat_forward(pose.orientation);
    let world_up = vec3f(0.0, 1.0, 0.0);
    let target_linvel = body_forward * (forward * config.max_speed_mps.max(0.0));
    let target_angvel =
        world_up * (turn * config.turn_gain.max(0.0) * config.max_yaw_speed_radps.max(0.0));
    Some(XrBodyDrive {
        widget_uid,
        target_linvel,
        target_angvel,
        max_linear_accel: config.max_linear_accel_mps2.max(0.0),
        max_angular_accel: config.max_angular_accel_radps2.max(0.0),
        preserve_vertical_linvel: true,
    })
}

pub fn tank_stick_axes(stick: Vec2f, config: TankDriveConfig) -> (f32, f32) {
    stick_deadzone_scaled_axes(stick, config.stick_deadzone, config.stick_response_exponent)
}

fn stick_deadzone_scaled_axes(stick: Vec2f, deadzone: f32, exponent: f32) -> (f32, f32) {
    (
        deadzone_scaled_axis(-stick.y, deadzone, exponent),
        deadzone_scaled_axis(-stick.x, deadzone, exponent),
    )
}

fn deadzone_scaled_axis(value: f32, deadzone: f32, exponent: f32) -> f32 {
    let deadzone = deadzone.clamp(0.0, 0.95);
    if !value.is_finite() {
        return 0.0;
    }
    let magnitude = value.abs();
    if magnitude <= deadzone {
        return 0.0;
    }
    let scaled = ((magnitude - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0);
    let response = scaled.powf(exponent.clamp(1.0, 3.0));
    response.copysign(value)
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

#[cfg(test)]
include!("../tests/obj/tank.rs");
