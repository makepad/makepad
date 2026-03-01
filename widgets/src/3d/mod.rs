pub mod bar_chart_3d;
pub mod chart_3d;
pub mod gltf_bridge;
pub mod gltf_3d;
pub mod grid_3d;
pub mod scene_3d;
pub mod view_3d;
pub mod view_splat;

pub use bar_chart_3d::*;
pub use chart_3d::*;
pub use gltf_bridge::*;
pub use gltf_3d::*;
pub use grid_3d::*;
pub use scene_3d::*;
pub use view_3d::*;
pub use view_splat::*;

use crate::makepad_draw::*;

pub fn script_mod(vm: &mut ScriptVm) {
    scene_3d::script_mod(vm);
    chart_3d::script_mod(vm);
    grid_3d::script_mod(vm);
    bar_chart_3d::script_mod(vm);
    gltf_3d::script_mod(vm);
    view_splat::script_mod(vm);
    view_3d::script_mod(vm);
}
