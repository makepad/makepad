//! Mail as a library: the mailbox widget, its engine, and the module a host
//! seats in-process. The standalone binary (`src/main.rs`) wraps `MailView{}`.

pub use makepad_widgets;
use makepad_widgets::*;

pub mod ai;
pub mod engine;
pub mod model;
pub mod module;
pub mod seed;
pub mod storage;
pub mod view;

pub use module::{MailModule, MAIL_MODULE};
pub use view::MailView;

/// This crate's widgets. Call after host widget registration; do not register
/// `makepad_widgets` again.
pub fn script_mod(vm: &mut ScriptVm) {
    view::script_mod(vm);
}
