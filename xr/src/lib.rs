pub use makepad_widgets;
pub use makepad_widgets::*;

pub mod algorithms;
pub mod net;
pub mod obj;
pub mod scene;
mod util;

pub mod render {
    pub use crate::util::gltf_bridge::{
        GltfDecodedMeshes, GltfDecodedPrimitiveObject, GltfDefaultView, GltfDrawObject,
        GltfMaterialState, GltfMeshObjects, GltfPrimitiveObject, GltfRenderer,
    };
    pub use crate::util::passthrough_env::DrawPassthroughEnvFace;
}

pub(crate) mod prelude {
    pub use crate::algorithms::depth_align::*;
    pub use crate::{net::*, render::*, scene::*};
    pub use makepad_widgets::*;
}

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    scene::xr_node::script_mod(vm);
    obj::gltf::script_mod(vm);
    obj::icosphere::script_mod(vm);
    obj::cube::script_mod(vm);
    scene::xr_permissions_flow::script_mod(vm);
    obj::physics_view::script_mod(vm);
    obj::refractive_cube::script_mod(vm);
    obj::shooter::script_mod(vm);

    util::passthrough_env::script_mod(vm);
    obj::tree::script_mod(vm);
    obj::view_splat::script_mod(vm);
    scene::xr_env::script_mod(vm);
    scene::xr_peer_sync::script_mod(vm);
    scene::xr_select::script_mod(vm);
    scene::xr_view::script_mod(vm);
    scene::xr_root::script_mod(vm)
}
