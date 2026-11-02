//! The fab design-tool controls (sliders, knobs, colour picks, value inputs
//! and their palette), the property inspector, the menu bar and the tween
//! inspector.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `fab` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod fab_controls;
pub mod menu_bar;
pub mod property_inspector;
pub mod tween_inspector;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn fab_mod(vm: &mut ScriptVm) {
    crate::tween_inspector::script_mod(vm);
    crate::fab_controls::script_mod(vm);
    crate::menu_bar::script_mod(vm);
    crate::property_inspector::script_mod(vm);
}

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[fab_mod]);
}
