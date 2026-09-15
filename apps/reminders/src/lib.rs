//! reminders as a library: the app around one root widget, and the module the
//! window manager seats in-process. Built from local/agent_state/wm-all/apps/reminders-spec.md.

pub use makepad_widgets;

pub mod ai;
pub mod engine;
pub mod model;
pub mod module;
pub mod seed;
pub mod storage;
pub mod view;

pub use module::{RemindersModule, REMINDERS_MODULE};
pub use view::RemindersView;

pub fn script_mod(vm: &mut makepad_widgets::ScriptVm) -> makepad_widgets::ScriptValue {
    view::script_mod(vm)
}
