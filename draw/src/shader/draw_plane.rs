pub use super::draw_projective_quad::{DrawPlane3d, DrawProjectiveQuad};
use crate::makepad_platform::*;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    super::draw_projective_quad::script_mod(vm)
}
