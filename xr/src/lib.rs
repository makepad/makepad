pub use makepad_widgets;
pub use makepad_widgets::*;

#[path = "obj/cube.rs"]
pub mod cube;
#[path = "obj/gltf_3d.rs"]
pub mod gltf_3d;
#[path = "util/gltf_bridge.rs"]
pub mod gltf_bridge;
#[path = "util/mesh_generators.rs"]
pub mod mesh_generators;
#[path = "scene/xr_node.rs"]
pub mod xr_node;
#[path = "util/passthrough_env.rs"]
pub mod passthrough_env;
#[path = "scene/xr_permissions_flow.rs"]
pub mod xr_permissions_flow;
#[path = "obj/physics_view.rs"]
pub mod physics_view;
#[path = "obj/refractive_cube.rs"]
pub mod refractive_cube;
#[path = "scene/xr_root.rs"]
pub mod xr_root;
#[path = "scene/xr_scene.rs"]
mod xr_scene;
#[path = "util/scene_draw.rs"]
mod scene_draw;
#[path = "obj/tree.rs"]
pub mod tree;
#[path = "obj/view_splat.rs"]
pub mod view_splat;

pub use cube::*;
pub use gltf_3d::*;
pub use gltf_bridge::*;
pub use xr_node::*;
pub use passthrough_env::DrawPassthroughEnvAtlas;
pub use xr_permissions_flow::*;
pub use physics_view::*;
pub use refractive_cube::*;
pub use xr_root::XrRoot;
pub use xr_scene::XrScene;
pub use tree::{
    CpuPythagoreanTree, DrawTreeBranches, DrawTreeLeaves, PYTHAGOREAN_TREE_ROOT_DROP,
};
pub use view_splat::*;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    gltf_3d::script_mod(vm);
    view_splat::script_mod(vm);

    xr_node::script_mod(vm);
    cube::script_mod(vm);
    xr_permissions_flow::script_mod(vm);
    physics_view::script_mod(vm);
    refractive_cube::script_mod(vm);

    passthrough_env::script_mod(vm);
    tree::script_mod(vm);
    xr_scene::script_mod(vm);
    xr_root::script_mod(vm)
}
