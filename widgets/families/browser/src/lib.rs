//! A web view, composited by the OS or drawn from the browser host's frame ring.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! and the engine-free `makepad-browser-view` client, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod web_view;
pub mod native_app_view;
mod textured;

/// Registers the browser view.
pub fn browser_mod(vm: &mut ScriptVm) {
    crate::web_view::script_mod(vm);
    crate::native_app_view::script_mod(vm);
}
