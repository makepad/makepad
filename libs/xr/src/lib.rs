pub use makepad_widgets;

use makepad_widgets::*;

pub mod passthrough_cube;
#[allow(dead_code)]
mod scene;

pub use passthrough_cube::DrawPassthroughCubeAtlas;
pub use scene::XrScene;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::passthrough_cube::script_mod(vm);
    crate::scene::script_mod(vm)
}
