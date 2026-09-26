//! Data grid, tree view and file tree, and the docking layout.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod data_grid;
pub mod data_grid_columns;
pub mod dock;
pub mod file_tree;
pub mod tree;

/// Registers the data grid, the tree view and the file tree.
pub fn data_mod(vm: &mut ScriptVm) {
    crate::data_grid::script_mod(vm);
    crate::tree::script_mod(vm);
    crate::file_tree::script_mod(vm);
}

/// Registers the dock and lets the widget tree's dumps print its tabs.
pub fn dock_mod(vm: &mut ScriptVm) {
    crate::dock::install_dock_hooks(vm.cx_mut());
    crate::dock::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with the data and dock families.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[data_mod, dock_mod]);
}

#[cfg(test)]
pub(crate) fn checkout_test_cx() -> PooledCx {
    makepad_widgets_core::test_cx::checkout_test_cx(script_mod)
}

#[cfg(test)]
pub(crate) fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    makepad_widgets_core::test_cx::on_test_cx(f)
}
