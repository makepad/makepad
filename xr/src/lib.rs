pub use makepad_widgets;
pub use makepad_widgets::*;

pub mod bar_chart_3d;
pub mod chart_3d;
pub mod cube_3d;
#[cfg(feature = "gltf")]
pub mod gltf_3d;
#[cfg(feature = "gltf")]
pub mod gltf_bridge;
pub mod grid_3d;
pub mod mesh_generators;
pub mod node_3d;
pub mod passthrough_cube;
pub mod permissions_flow;
pub mod physics_view;
mod scene;
pub mod scene_3d;
pub mod tree;
pub mod view_3d;
#[cfg(feature = "splat")]
pub mod view_splat;

pub use bar_chart_3d::*;
pub use chart_3d::*;
pub use cube_3d::*;
#[cfg(feature = "gltf")]
pub use gltf_3d::*;
#[cfg(feature = "gltf")]
pub use gltf_bridge::*;
pub use grid_3d::*;
pub use node_3d::*;
pub use passthrough_cube::DrawPassthroughCubeAtlas;
pub use permissions_flow::*;
pub use physics_view::*;
pub use scene::XrScene;
pub use scene_3d::*;
pub use tree::{
    CpuPythagoreanTree, DrawTreeBranches, DrawTreeLeaves, PYTHAGOREAN_TREE_ROOT_DROP,
};
pub use view_3d::*;
#[cfg(feature = "splat")]
pub use view_splat::*;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    scene_3d::script_mod(vm);
    node_3d::script_mod(vm);
    chart_3d::script_mod(vm);
    grid_3d::script_mod(vm);
    bar_chart_3d::script_mod(vm);
    #[cfg(feature = "gltf")]
    gltf_3d::script_mod(vm);
    #[cfg(feature = "splat")]
    view_splat::script_mod(vm);
    view_3d::script_mod(vm);

    cube_3d::script_mod(vm);
    permissions_flow::script_mod(vm);
    physics_view::script_mod(vm);

    passthrough_cube::script_mod(vm);
    tree::script_mod(vm);
    scene::script_mod(vm)
}
