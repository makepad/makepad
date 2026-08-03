//! Makepad Arcade forward renderer (game.md §Engine design — game_render).
//!
//! M0 stage B: the gamemaker render path moved here verbatim from
//! game_view.rs — the 5 game draw shaders, unit shape geometries, the packed
//! static-instance slabs, the scene pass (sky → terrain → opaque → alpha) and
//! the 2D HUD/label overlays. Cx-coupled by design; the deterministic sim it
//! draws lives in makepad-game-sim.
//!
//! Shape: [`GameRenderer`] owns the GPU-side caches (geometries, slabs);
//! the draw structs stay `#[live]` fields on the host widget so script-side
//! theming keeps working — the widget lends them per frame via [`GameDraws`].
//! Camera state is per-view ([`CameraRig`]) — nothing here assumes one view.

pub mod ao;
pub mod bake;
pub mod geometry;
pub mod hud;
pub mod model;
pub mod renderer;
pub mod scene;
pub mod shaders;
pub mod skin;
pub mod stage;
pub mod particles;
pub mod shadow;
pub mod shadow_mesh;
pub mod sun;
pub mod thermometer;

pub use bake::*;
pub use geometry::*;
pub use hud::*;
pub use model::*;
pub use renderer::*;
pub use scene::*;
pub use shaders::*;
pub use stage::*;
pub use particles::*;
pub use shadow::*;
pub use shadow_mesh::*;
pub use sun::*;

use makepad_draw::*;

/// Register the game draw shaders into the script VM. Call after
/// `makepad_widgets::script_mod` (the shader block uses the widgets prelude)
/// and before any widget that declares these draw types.
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    shaders::script_mod(vm)
}
