//! `makepad-render-graph`: everything between a renderer's scene pass and
//! the host's pane, shared by Stage and Sandbox (KERNELS.md §3.4).
//!
//! Rust provides the graph, the pass slots, the accumulation and the
//! formats; every look is Splash (passes and kits), loaded at runtime.
//!
//! * [`plan`]: the post graph (`@hdr` / `@display` / `@final` passes reading
//!   named resources), compiled into a frame plan with pass ids, resource
//!   versions and admitted memory.
//! * [`pass`]: Splash passes, compiled at runtime (`program`); `runner`
//!   records a plan's stages in the slots a host gives them.
//!
//! Without the default `gpu` feature the crate is UI-free (plan, schedules,
//! locked-time rules, pass declarations, kits), for document readers.
//! * [`kits`]: the standard kits in Splash (Glow, Halation, Aberration,
//!   Grain, Vignette, FramePost, Lut, DepthOfField, Outline); [`script`]
//!   reads passes from documents.
//! * [`mode`]: realtime and locked time, canonical sub-frame times and the
//!   adaptive sub-frame schedule; [`accum`] accumulates sub-frames on the
//!   GPU with its convergence estimate; [`locked`] is locked time's history
//!   inventory.
//! * [`BloomPass`] and [`DrawSceneTexture`]: the Sandbox lane's bloom and
//!   auto-exposure chain and its composite (exposure, AgX, FXAA, dither).

#[cfg(feature = "gpu")]
use makepad_draw::*;

#[cfg(feature = "gpu")]
pub mod accum;
#[cfg(feature = "gpu")]
pub mod bloom;
#[cfg(feature = "gpu")]
pub mod composite;
pub mod kits;
pub mod locked;
pub mod mode;
pub mod pass;
pub mod plan;
#[cfg(feature = "gpu")]
pub mod program;
#[cfg(feature = "gpu")]
pub mod runner;
pub mod script;
#[cfg(feature = "gpu")]
pub mod tonemap;

#[cfg(feature = "gpu")]
pub use accum::{Accumulator, Step};
#[cfg(feature = "gpu")]
pub use bloom::BloomPass;
#[cfg(feature = "gpu")]
pub use composite::DrawSceneTexture;
#[cfg(feature = "gpu")]
pub use tonemap::{DrawToneMap, Grade, ToneCurve};
pub use mode::{AdaptiveSampling, Rational, RenderMode, Sampling};
pub use pass::{PassDecl, UniformDecl};
pub use plan::{Attachments, Format, PostGraph, Resource, Stage};
#[cfg(feature = "gpu")]
pub use runner::{FrameUniforms, GraphRunner, PassView, StageInputs};

/// A pass's uniform values for one frame, `[f32; 4]` per uniform in
/// declaration order.
pub type PassValues = Vec<[f32; 4]>;

#[cfg(feature = "gpu")]
/// Register the graph's draw shaders. Call after `makepad_widgets::script_mod`
/// (the composite uses the widgets prelude).
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    bloom::script_mod(vm);
    script_mod_passes(vm);
    composite::script_mod(vm)
}

#[cfg(feature = "gpu")]
/// The shared Splash stdlib (`mod.shared`) every pass's code has in scope:
/// registered once per VM, before a pass compiles.
pub fn pass_stdlib(vm: &mut ScriptVm) -> ScriptValue {
    makepad_script_compute::module::register_shared_std(vm);
    NIL
}

#[cfg(feature = "gpu")]
/// Register the pass, accumulator and tone-map shaders only (a host without
/// the Sandbox lane's bloom and composite), once per VM however many hosts
/// ask. Call after `makepad_widgets::script_mod`.
pub fn script_mod_passes(vm: &mut ScriptVm) {
    let draw = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str("draw").into(), NoTrap).as_object();
    let have = draw.is_some_and(|d| {
        let v = vm.bx.heap.value(d, LiveId::from_str("DrawGraphPass").into(), NoTrap);
        !v.is_nil() && !v.is_err()
    });
    if !have {
        program::script_mod(vm);
        accum::script_mod(vm);
        tonemap::script_mod(vm);
    }
}
