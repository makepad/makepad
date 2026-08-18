//! 3D forward renderer: GLB/skin, stage, lighting, sky, terrain.
//!
//! [`Renderer`] owns the GPU-side caches (geometries, slabs). Draw structs
//! stay `#[live]` fields on the host widget so script-side theming keeps
//! working — the widget lends them per frame via [`SceneDraws`]. Camera
//! state is per-view ([`CameraRig`]).
//!
//! Asset UI / VJ use [`PreviewLook`] + [`Renderer::draw_preview`]. Arcade
//! still draws a full world through [`Renderer::draw_scene_full`].

pub mod ao;
pub mod ao_atlas;
pub mod ao_lightmapper;
pub mod aobaker_port;
pub mod bakerboy;
pub mod bake;
pub mod firework;
pub mod geometry;
pub mod gpu_lightmap;
pub mod hud;
pub mod light_grid;
pub mod lightmap;
pub mod model;
pub mod renderer;
pub mod scene;
pub mod shaders;
pub mod skin;
pub mod stage;
pub mod particles;
pub mod play;
pub mod preview;
pub mod shadow;
pub mod shadow_mesh;
pub mod shadow_csm;
pub mod shadow_sdf;
pub mod sky;
pub mod sun;
pub mod thermometer;

pub use bake::*;
pub use gpu_lightmap::{
    dynamic_shadow_tiers, CsmConfig, DynamicShadowTiers, GpuLightmapMode, GpuLmMover,
    GpuLmSkin, DEFAULT_CSM_CONFIG,
};
pub use geometry::*;
pub use hud::*;
pub use model::*;
pub use renderer::*;
pub use scene::*;
pub use shaders::*;
pub use stage::*;
pub use particles::*;
pub use play::*;
pub use preview::*;
pub use shadow::*;
pub use shadow_mesh::*;
pub use sun::*;

use makepad_draw::*;

/// Register the scene draw shaders into the script VM. Call after
/// `makepad_widgets::script_mod` (the shader block uses the widgets prelude)
/// and before any widget that declares these draw types.
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    shaders::script_mod(vm)
}
