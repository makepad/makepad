pub use makepad_widgets;
pub use makepad_widgets::*;

#[path = "obj/cube.rs"]
pub mod cube;
#[path = "obj/gltf.rs"]
pub mod gltf;
#[path = "util/gltf_bridge.rs"]
pub mod gltf_bridge;
#[path = "obj/icosphere.rs"]
pub mod icosphere;
#[path = "util/depth_debug_mesh.rs"]
pub mod depth_debug_mesh;
#[path = "util/depth_debug_mesh_worker.rs"]
mod depth_debug_mesh_worker;
#[path = "util/mesh_generators.rs"]
pub mod mesh_generators;
#[path = "util/passthrough_env.rs"]
pub mod passthrough_env;
#[path = "util/tsdf_query.rs"]
mod tsdf_query;
#[path = "obj/physics_view.rs"]
pub mod physics_view;
#[path = "obj/refractive_cube.rs"]
pub mod refractive_cube;
#[path = "util/scene_draw.rs"]
mod scene_draw;
#[path = "obj/shooter.rs"]
pub mod shooter;
#[path = "obj/tree.rs"]
pub mod tree;
#[path = "obj/view_splat.rs"]
pub mod view_splat;
#[path = "scene/xr_body_spawn.rs"]
pub mod xr_body_spawn;
#[path = "scene/xr_env.rs"]
pub mod xr_env;
#[path = "scene/xr_gesture.rs"]
mod xr_gesture;
#[path = "xr_net.rs"]
pub mod xr_net;
#[path = "scene/xr_node.rs"]
pub mod xr_node;
#[path = "scene/xr_people_debug.rs"]
pub mod xr_people_debug;
#[path = "scene/xr_permissions_flow.rs"]
pub mod xr_permissions_flow;
#[path = "scene/xr_root.rs"]
pub mod xr_root;
#[path = "scene/xr_select.rs"]
pub mod xr_select;
#[path = "scene/xr_view.rs"]
pub mod xr_view;

pub use cube::*;
pub use gltf::*;
pub use gltf_bridge::*;
pub use icosphere::*;
pub use passthrough_env::DrawPassthroughEnvFace;
pub use physics_view::*;
pub use refractive_cube::*;
pub use shooter::*;
pub use tree::{
    CpuPythagoreanTree, DrawTreeBranches, DrawTreeLeaves, Tree, PYTHAGOREAN_TREE_ROOT_DROP,
};
pub use view_splat::*;
pub use xr_body_spawn::*;
pub use xr_env::XrEnv;
pub use xr_net::*;
pub use xr_node::*;
pub use xr_people_debug::XrPeopleDebug;
pub use xr_permissions_flow::*;
pub use xr_root::XrRoot;
pub use xr_select::XrSelect;
pub use xr_view::XrView;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    xr_node::script_mod(vm);
    gltf::script_mod(vm);
    icosphere::script_mod(vm);
    cube::script_mod(vm);
    xr_permissions_flow::script_mod(vm);
    physics_view::script_mod(vm);
    refractive_cube::script_mod(vm);
    shooter::script_mod(vm);

    passthrough_env::script_mod(vm);
    tree::script_mod(vm);
    view_splat::script_mod(vm);
    xr_env::script_mod(vm);
    xr_people_debug::script_mod(vm);
    xr_select::script_mod(vm);
    xr_view::script_mod(vm);
    xr_root::script_mod(vm)
}
