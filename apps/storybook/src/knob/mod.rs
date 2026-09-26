//! The Material Bench's knob engine, ported into the storybook.
//!
//! The bench (`local/agent_state/surface-material/bench/material-bench-v74.html`)
//! is a WebGL page whose knob is a solid of revolution with a grip, a wing
//! and cuts, shaded from its height field: a GGX highlight, three studios to
//! reflect, metal and clear coat, an analytic cast shadow and a baked
//! self-shadow, and marks in four finishes. This module is that engine:
//!
//! * [`presets`]: its eleven materials and nineteen styles, as data;
//! * [`bake`]: its JavaScript bakes -- the curves, the wing's constants, the
//!   shadow's knots and the self-shadow table;
//! * [`shader`]: its GLSL in the shader DSL, one set of solid and shading
//!   functions spread into the 2D knob, the 3D view and the ground;
//! * [`widgets`]: `TurnedKnob` and `KnobView3d`.
//!
//! It lives in the storybook: nothing in `widgets/` or `draw/` depends on it.
use crate::makepad_widgets::*;

pub mod bake;
pub mod presets;
pub mod shader;
pub mod widgets;

pub use presets::{KnobMaterial, KnobStyle, MATERIALS, STYLES};
pub use widgets::{KnobView3d, KnobView3dRef, TurnedKnob, TurnedKnobAction, TurnedKnobRef};

/// Register the shaders and the widgets. The story that uses them calls this
/// before its own templates.
pub fn script_mod(vm: &mut ScriptVm) {
    shader::script_mod(vm);
    widgets::script_mod(vm);
}
