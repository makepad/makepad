//! The realtime SDF engine (shared by Fab and Sandbox).
//!
//! - [`program`]: a `csg_sdf` shape lowered to a flat f32 program;
//! - [`twin`]: the CPU interpreter of that program, bit-exact with the GPU;
//! - [`volume`]: an object's sparse brick volume (lattice, allocation,
//!   per-brick culled programs, dirty re-builds);
//! - [`kernel`]: Splash SDF kernels compiled into the bake shader;
//! - [`budget`]: GPU safety (measured frame costs, the bake's steering, caps);
//! - [`bake`]: the GPU bake into persistent R32F/R8 atlases by draw passes;
//! - [`march`]: the coarse pass, the march (depth + G-buffer) and the shade;
//! - [`pick`]: one ray against the exact field, naming the feature hit;
//! - [`splat`]: the splat look (surface splats from the bricks).
//!
//! No GPU compute: every GPU step is a draw pass into a render target.

pub use makepad_csg_sdf;
pub use makepad_draw;

pub mod bake;
pub mod budget;
pub mod kernel;
pub mod march;
pub mod pick;
pub mod program;
pub mod splat;
pub mod twin;
pub mod volume;

use makepad_draw::*;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::bake::script_mod(vm);
    crate::march::script_mod(vm);
    crate::splat::script_mod(vm)
}
