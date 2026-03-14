pub use makepad_draw;
pub use makepad_draw::*;

pub mod quad;
pub mod surface;

pub use crate::quad::{MpCompositedQuad, MpCompositor, MP_MAX_CLIP_PLANES};
pub use crate::surface::{MpSurface, MpSurfaceColorFormat};

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::quad::script_mod(vm);
    NIL
}
