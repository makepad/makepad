//! Progressive CPU/GPU ray tracing, scene packing, sky lighting, and image
//! output. The crate is independent of applications and file formats.
//!
//! * [`scene::SceneInput`] — the flat input snapshot (any app builds one).
//! * [`gpu::RayTracer`] — the renderer: `set_scene`, then `draw` every frame
//!   from a widget; `view_texture` is the tonemapped image.
//! * [`cpu_ref::CpuTracer`] — the CPU twin that gates the GPU.
//! * [`env_map`] — environment-map radiance and importance sampling.

pub mod building;
pub mod bvh;
pub mod cpu_ref;
pub mod env_map;
pub mod glb;
pub mod gpu;
pub mod pack;
pub mod png;
pub mod rng;
pub mod scene;
pub mod sky;

pub use env_map::{EnvironmentFrame, EnvironmentLight, EnvironmentMap};
pub use gpu::{DisplayCurve, RayTracer, RenderSettings, RenderStats};
pub use scene::{Camera, Image, Material, SceneInput, Sun};

use makepad_draw::*;

/// Register the renderer's shaders. Call once after `makepad_widgets::script_mod`.
pub fn script_mod(vm: &mut ScriptVm) {
    // The tone map takes its curves from the shared copy.
    makepad_render_graph::curves::register(vm);
    crate::gpu::script_mod(vm);
}
