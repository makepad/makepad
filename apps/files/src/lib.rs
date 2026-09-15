//! files as a library: the browser widget, its folder views, and — for a
//! host that links it — the module ([`module`]): what the window manager
//! seats in a tile IN-PROCESS, one instance per isolate, without a `Window`
//! or a process. The standalone binary (`src/main.rs`, feature `standalone`)
//! is the same crate with a `Window` around `FilesView{}`.

pub use makepad_widgets;

pub mod ai_service;
pub mod bookmarks;
#[cfg(feature = "chat")]
pub mod chat_agent;
#[cfg(feature = "chat")]
pub mod chat_panel;
pub mod chat_tools;
pub mod contents;
pub mod demo;
pub mod menu;
pub mod model;
pub mod module;
pub mod ops;
pub mod preview;
pub mod rename;
pub mod theme;
pub mod thumbs;
pub mod vfs;
pub mod view;

pub use module::{FilesModule, FILES_MODULE};
pub use view::FilesView;

use makepad_widgets::*;

/// This crate's palette tokens and widget families. Call after host widget
/// registration and the WM theme; do not register `makepad_widgets` again.
pub fn script_mod(vm: &mut ScriptVm) {
    theme::Palette::for_vm(vm).publish(vm);
    theme::script_mod(vm);
    thumbs::script_mod(vm);
    makepad_diskmap::script_mod(vm);
    contents::script_mod(vm);
    #[cfg(feature = "chat")]
    chat_panel::script_mod(vm);
    #[cfg(not(feature = "chat"))]
    view::no_chat::script_mod(vm);
    view::script_mod(vm);
}
