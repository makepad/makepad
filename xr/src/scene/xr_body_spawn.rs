use crate::*;

#[derive(Clone, Copy, Debug)]
pub struct XrBodySpawn {
    pub widget_uid: WidgetUid,
    pub pose: Pose,
    pub linvel: Vec3f,
    pub angvel: Vec3f,
}
