//! calculator as a library: the expression engine, the one root widget, and
//! the module the window manager seats in-process.

pub use makepad_widgets;

pub mod ai;
pub mod engine;
pub mod model;
pub mod module;
pub mod seed;
pub mod view;

pub use module::{CalculatorModule, CALCULATOR_MODULE};
pub use view::CalculatorView;

use makepad_widgets::*;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    view::script_mod(vm)
}
