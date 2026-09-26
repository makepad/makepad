//! The map view.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod map;

/// Registers the map style and the map view.
pub fn maps_mod(vm: &mut ScriptVm) {
    crate::map::style::script_mod(vm);
    crate::map::view::script_mod(vm);
}

// The whole registration the tests build on:
// the core with the map view.
#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[maps_mod]);
}
