//! The design overlay (Shift+F10) a Window carries, with the reflection and the saved-theme store it reads and writes.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! and the data family (the dock and file tree the panel is made of), has no
//! features of its own, and compiles once whatever mix of families an app
//! picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;
use makepad_widgets_data::{dock, file_tree};

pub mod reflect;
pub mod theme_store;
pub mod tweaker;

/// Registers the overlay where the Window names it
/// ([`WindowFamilies::tweaker`]) and hands the core its hooks.
pub fn tweaker_mod(vm: &mut ScriptVm) {
    crate::tweaker::install_tweaker_hooks(vm.cx_mut());
    crate::tweaker::script_mod(vm);
}

// The whole registration the tests build on:
// the core with the overlay and the data and dock families it opens.
#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(
        vm,
        WindowFamilies { tweaker: Some(tweaker_mod), ..Default::default() },
        &[makepad_widgets_data::data_mod, makepad_widgets_data::dock_mod],
    );
}
