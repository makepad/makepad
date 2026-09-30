//! `makepad-render-graph`: everything between a renderer's scene pass and
//! the host's pane, shared by Stage and Sandbox (KERNELS.md §3.4).
//!
//! Rust provides the graph, the pass slots, the accumulation and the
//! formats; every look is Splash (passes and kits), loaded at runtime.
//!
//! * [`plan`]: the post graph (`@hdr` / `@display` / `@final` passes reading
//!   named resources), compiled into a frame plan with pass ids, resource
//!   versions and admitted memory.
//! * [`pass`]: Splash passes, compiled at runtime; [`runner`] records a
//!   plan's stages in the slots a host gives them.
//! * [`kits`]: the standard kits in Splash (Glow, Halation, Aberration,
//!   Grain, Vignette, FramePost, Lut, DepthOfField, Outline); [`script`]
//!   reads passes from documents.
//! * [`mode`]: realtime and locked time, canonical sub-frame times and the
//!   adaptive sub-frame schedule; [`accum`] accumulates sub-frames on the
//!   GPU with its convergence estimate; [`locked`] is locked time's history
//!   inventory.
//! * [`BloomPass`] and [`DrawSceneTexture`]: the Sandbox lane's bloom and
//!   auto-exposure chain and its composite (exposure, AgX, FXAA, dither).

use makepad_draw::*;

pub mod accum;
pub mod bloom;
pub mod composite;
pub mod kits;
pub mod locked;
pub mod mode;
pub mod pass;
pub mod plan;
pub mod runner;
pub mod script;

pub use accum::{Accumulator, Step};
pub use bloom::BloomPass;
pub use composite::DrawSceneTexture;
pub use mode::{AdaptiveSampling, Rational, RenderMode, Sampling};
pub use pass::{PassDecl, UniformDecl};
pub use plan::{Attachments, Format, PostGraph, Resource, Stage};
pub use runner::{FrameUniforms, GraphRunner, PassValues, StageInputs};

/// Register the graph's draw shaders. Call after `makepad_widgets::script_mod`
/// (the composite uses the widgets prelude).
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    bloom::script_mod(vm);
    script_mod_passes(vm);
    composite::script_mod(vm)
}

/// Register the pass and accumulator shaders only (a host without the
/// Sandbox lane's bloom and composite), once per VM however many hosts ask.
pub fn script_mod_passes(vm: &mut ScriptVm) {
    let draw = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str("draw").into(), NoTrap).as_object();
    let have = draw.is_some_and(|d| {
        let v = vm.bx.heap.value(d, LiveId::from_str("DrawGraphPass").into(), NoTrap);
        !v.is_nil() && !v.is_err()
    });
    if !have {
        pass::script_mod(vm);
        accum::script_mod(vm);
    }
}
