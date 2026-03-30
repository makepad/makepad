use crate::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrBodySpawn {
    pub widget_uid: WidgetUid,
    pub pose: Pose,
    pub linvel: Vec3f,
    pub angvel: Vec3f,
}
