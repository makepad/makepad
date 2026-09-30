//! `makepad-render-graph`: what runs between a renderer's scene pass and the
//! host's pane (KERNELS.md §3.4).
//!
//! * [`BloomPass`]: the HDR lane's energy-conserving bloom and
//!   auto-exposure chain.
//! * [`DrawSceneTexture`]: the composite that brings the scene target into
//!   the host pane (exposure, AgX, FXAA, dither on the HDR lane; a plain
//!   blit on the legacy display-space lane).

use makepad_draw::*;

pub mod bloom;
pub mod composite;

pub use bloom::BloomPass;
pub use composite::DrawSceneTexture;

/// Register the graph's draw shaders. Call after `makepad_widgets::script_mod`
/// (the composite uses the widgets prelude).
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    bloom::script_mod(vm);
    composite::script_mod(vm)
}
