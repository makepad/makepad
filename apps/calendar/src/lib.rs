//! calendar as a library: one root widget, a pure engine, and the module
//! the window manager seats in-process. Built from
//! `local/agent_state/wm-all/apps/calendar-redesign.md`.

pub use makepad_widgets;

pub mod agenda;
pub mod ai;
pub mod components;
pub mod engine;
pub mod model;
pub mod module;
pub mod month;
pub mod navigation;
pub mod presentation;
pub mod seed;
pub mod storage;
pub mod timeline;
pub mod view;

pub use module::{CalendarModule, CALENDAR_MODULE};
pub use view::CalendarView;

use makepad_widgets::*;

/// This crate's Splash widgets. The host (or standalone `AppMain`) registers
/// stock widgets first; this only adds Calendar's family.
pub fn script_mod(vm: &mut ScriptVm) {
    presentation::script_mod(vm);
    components::script_mod(vm);
    navigation::script_mod(vm);
    month::script_mod(vm);
    agenda::script_mod(vm);
    timeline::script_mod(vm);
    view::script_mod(vm);
}
