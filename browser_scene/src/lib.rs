
pub use makepad_compositor::{MpBackfaceVisibility, MpTransformStyle};
pub use makepad_widgets;
pub use makepad_widgets::*;

pub mod clip;
pub mod effect;
pub mod embed;
pub mod example_support;
pub mod hit_test;
pub mod primitive;
pub mod renderer;
pub mod resource;
pub mod scene;
pub mod spatial;
pub mod transaction;

pub use clip::*;
pub use effect::*;
pub use embed::*;
pub use example_support::*;
pub use hit_test::*;
pub use primitive::*;
pub use renderer::*;
pub use resource::*;
pub use scene::*;
pub use spatial::*;
pub use transaction::*;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    makepad_compositor::script_mod(vm);
    NIL
}
