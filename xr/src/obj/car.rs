use crate::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct CarDriveConfig {
    pub stick_deadzone: f32,
    pub stick_response_exponent: f32,
    pub trigger_deadzone: f32,
}

impl Default for CarDriveConfig {
    fn default() -> Self {
        Self {
            stick_deadzone: 0.24,
            stick_response_exponent: 1.75,
            trigger_deadzone: 0.08,
        }
    }
}

pub fn script_mod(_vm: &mut ScriptVm) -> ScriptValue {
    NIL
}

pub fn car_drive_command(
    widget_uid: WidgetUid,
    held_by: Option<XrSharedHand>,
    steer_stick: Vec2f,
    accelerate_trigger: f32,
    reverse_trigger: f32,
    config: CarDriveConfig,
) -> Option<XrCarControl> {
    if held_by.is_some() {
        return None;
    }
    let (_, steer) = car_stick_axes(steer_stick, config);
    let accelerate = deadzone_scaled_trigger(accelerate_trigger, config.trigger_deadzone);
    let reverse = deadzone_scaled_trigger(reverse_trigger, config.trigger_deadzone);
    Some(XrCarControl {
        widget_uid,
        steer,
        throttle: (accelerate - reverse).clamp(-1.0, 1.0),
        brake: 0.0,
    })
}

pub fn car_stick_axes(stick: Vec2f, config: CarDriveConfig) -> (f32, f32) {
    (
        deadzone_scaled_axis(
            -stick.y,
            config.stick_deadzone,
            config.stick_response_exponent,
        ),
        deadzone_scaled_axis(
            stick.x,
            config.stick_deadzone,
            config.stick_response_exponent,
        ),
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

fn deadzone_scaled_trigger(value: f32, deadzone: f32) -> f32 {
    let deadzone = deadzone.clamp(0.0, 0.95);
    let value = value.clamp(0.0, 1.0);
    if !value.is_finite() || value <= deadzone {
        0.0
    } else {
        ((value - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
include!("../tests/obj/car.rs");
