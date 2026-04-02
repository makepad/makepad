use crate::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrBodySpawn {
    pub widget_uid: WidgetUid,
    pub shadow: bool,
    pub mode: XrSharedObjectMode,
    pub pose: Pose,
    pub linvel: Vec3f,
    pub angvel: Vec3f,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrBodyImpulse {
    pub widget_uid: WidgetUid,
    pub point: Vec3f,
    pub impulse: Vec3f,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrBodyWrench {
    pub widget_uid: WidgetUid,
    pub force: Vec3f,
    pub torque: Vec3f,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrBodyDrive {
    pub widget_uid: WidgetUid,
    pub target_linvel: Vec3f,
    pub target_angvel: Vec3f,
    pub max_linear_accel: f32,
    pub max_angular_accel: f32,
    pub preserve_vertical_linvel: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrCarControl {
    pub widget_uid: WidgetUid,
    pub steer: f32,
    pub throttle: f32,
    pub brake: f32,
}
