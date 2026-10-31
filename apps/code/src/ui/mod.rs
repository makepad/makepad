//! The window: `view` is the one widget, `model` the transcript it draws.
//! The agent (or its scripted stand-in) comes through
//! `makepad_code_agent::port`.
pub mod charts;
pub mod hud;
pub mod layout;
pub mod look;
pub mod model;
pub mod palette;
pub mod paths;
pub mod theme;
pub mod view;

use makepad_widgets::*;

pub fn script_mod(vm: &mut ScriptVm) {
    theme::script_mod(vm);
    makepad_code_editor::script_mod(vm);
    charts::script_mod(vm);
    hud::script_mod(vm);
    layout::script_mod(vm);
}
