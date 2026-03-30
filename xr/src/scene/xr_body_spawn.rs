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
