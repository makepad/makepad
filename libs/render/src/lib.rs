//! 3D forward renderer: GLB/skin, stage, lighting, sky, terrain.
//!
//! [`Renderer`] owns the GPU-side caches (geometries, slabs). Draw structs
//! stay `#[live]` fields on the host widget so script-side theming keeps
//! working — the widget lends them per frame via [`SceneDraws`]. Camera
//! state is per-view ([`CameraRig`]).
//!
//! Asset UI / VJ use [`PreviewLook`] + [`Renderer::draw_preview`]. Arcade
//! still draws a full world through [`Renderer::draw_scene_full`].

// Shader/live literals intentionally use their source-language spelling.
#![allow(clippy::approx_constant)]

pub mod asset_lights;
pub mod asset_morph;
pub mod asset_rigid;
pub mod asset_animation;
pub mod asset_sockets;
pub mod asset_metadata;
pub mod asset_lod;
pub mod lod_view;
pub mod ao;
pub mod custom_material;
pub mod clustered;
pub mod fast_gi;
pub mod entity_lights;
pub mod local_shadows;
pub mod ao_atlas;
pub mod ao_lightmapper;
pub mod aobaker_port;
pub mod bakerboy;
pub mod bake;
pub mod firework;
pub mod geometry;
pub mod grass;
pub mod impostor;
pub mod gpu_lightmap;
pub mod light_grid;
pub mod lightmap;
pub mod model;
pub mod material_surface;
pub mod texture_compression;
pub mod texture_pack;
pub mod texture_restore;
pub use makepad_texcomp;
pub mod renderer;
pub mod scene;
pub mod shaders;
pub mod skin;
pub mod stage;
pub mod particles;
pub mod quality_tier;
pub mod play;
pub mod preview;
pub mod shadow;
pub mod shadow_mesh;
pub mod shadow_csm;
pub mod shadow_sdf;
// Environment maps: their IBL preparation, a generated map's key, a photographed sky's sun.
pub mod env_map;
pub mod sky;
pub mod smoke;
pub mod vfx;
pub mod ssao;
pub mod cortao;
pub mod volume_fog;
pub mod ssr;
pub mod trample;
pub mod still_aa;
pub mod stream;
pub mod sun;
pub mod thermometer;
pub mod world_lights;

pub use bake::*;
pub use custom_material::{DrawSceneCustom, DrawMaterialShadow};
pub use makepad_render_material;
pub use clustered::{ClusterConfig, ClusterStats};
pub use fast_gi::{GiMode, GiConfig, GiStats, GiDebug, GiInstance, GiMesh, GiField};
pub use local_shadows::{LocalShadowConfig, LocalShadowStats};
pub use gpu_lightmap::{
    dynamic_shadow_tiers, CascadeCasters, CascadeTile, CsmConfig, DynamicShadowTiers, GpuLightmapMode, GpuLmMover,
    GpuLmSkin, DEFAULT_CSM_CONFIG,
};
pub use geometry::*;
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
pub use ssao::*;
pub use ssr::SsrSettings;
pub use still_aa::{jittered_projection, still_aa_offset, DrawStillAaBlend, StillAa, STILL_AA_SAMPLES};
pub use smoke::SmokeVolume;
pub use sun::*;
// `sun::solar_dir` (axis-mapped game-space wrapper) and
// `makepad_draw::solar_dir` (the underlying shared solar model, pulled in
// by the glob import below) share a name; this crate's own wrapper is the
// intended public `makepad_render::solar_dir` — an explicit re-export wins
// over both globs and resolves the ambiguity.
pub use sun::solar_dir;

use makepad_draw::*;

/// Register the scene draw shaders into the script VM. Call after
/// `makepad_widgets::script_mod` (the shader block uses the widgets prelude)
/// and before any widget that declares these draw types.
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    for (_, step) in script_mod_steps() {
        step(vm);
    }
    NIL
}

/// One module of the scene layer's registration: its name and what registers it.
pub type ScriptModStep = (&'static str, fn(&mut ScriptVm) -> ScriptValue);

/// The modules `script_mod` registers, one at a time in its order, for a host
/// that spreads the registration over frames (`custom_material::Registration`).
pub fn script_mod_steps() -> impl Iterator<Item = ScriptModStep> {
    const BEFORE_SHADERS: &[ScriptModStep] = &[
        // The shared Splash stdlib (`mod.shared`: sRGB, hashes) the shaders use.
        ("pass_stdlib", makepad_render_graph::pass_stdlib),
        ("shadow sampling", local_shadows::sampling::script_mod),
        ("hardware shadow sampling", hardware_sampling),
        ("clustered", clustered::script_mod),
        ("fast_gi", fast_gi::script_mod),
        ("ssao", ssao::script_mod),
        ("cortao", cortao::script_mod),
        ("still_aa", still_aa::script_mod),
        ("render_graph", makepad_render_graph::script_mod),
        ("smoke", smoke::script_mod),
        ("vfx", vfx::shader::script_mod),
    ];
    const AFTER_SHADERS: &[ScriptModStep] = &[
        ("volume_fog", volume_fog::script_mod),
        ("ssr", ssr::script_mod),
        ("trample", trample::script_mod),
        ("local_shadows", local_shadows::script_mod),
    ];
    fn hardware_sampling(vm: &mut ScriptVm) -> ScriptValue {
        if local_shadows::hardware_shadow_maps() {
            local_shadows::hardware_sampling::script_mod(vm);
        }
        NIL
    }
    BEFORE_SHADERS.iter().chain(shaders::SCRIPT_MOD_STEPS).chain(AFTER_SHADERS).copied()
}
