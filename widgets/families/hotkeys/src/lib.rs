//! Hotkey bindings, the hotkey editor and the keyboard map.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `hotkeys` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod hotkey_editor;
pub mod hotkeys;
pub mod keyboard_map;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn hotkeys_mod(vm: &mut ScriptVm) {
    crate::hotkey_editor::script_mod(vm);
    crate::keyboard_map::script_mod(vm);
}
