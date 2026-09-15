//! notes as a library: one root widget, a pure document engine, and the
//! module the window manager seats in-process. Built from
//! the Notes redesign, with source-preserving formatted editing.

pub use makepad_widgets;

pub mod ai;
pub mod controls;
pub mod engine;
pub mod model;
pub mod module;
pub mod projection;
pub mod seed;
pub mod storage;
pub mod text_surface;
mod ui;
pub mod view;

pub use module::{NotesModule, NOTES_MODULE};
pub use view::NotesView;

use makepad_widgets::*;

/// Apply the WM theme and register this crate's recipes. The host (or the
/// standalone binary) registers widgets first; this function does not.
pub fn script_mod(vm: &mut ScriptVm) {
    makepad_wm_theme::apply(vm);
    controls::script_mod(vm);
    text_surface::script_mod(vm);
    ui::script_mod(vm);
}
