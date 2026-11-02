//! Floating action, radial, pill-nav, line and hamburger menus, and the command palette.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod command_palette;
pub mod floating_action;
pub mod hamburger_menu;
pub mod line_menu;
pub mod pill_nav;
pub mod radial_menu;

/// Registers the navigation menus: the line menu after the pill nav whose
/// surface it draws with.
pub fn nav_menus_mod(vm: &mut ScriptVm) {
    crate::pill_nav::script_mod(vm);
    crate::radial_menu::script_mod(vm);
    crate::floating_action::script_mod(vm);
    crate::line_menu::script_mod(vm);
    crate::hamburger_menu::script_mod(vm);
}

/// Registers the command palette.
pub fn command_palette_mod(vm: &mut ScriptVm) {
    crate::command_palette::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with the navigation menus and the command palette.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(
        vm,
        WindowFamilies::default(),
        &[nav_menus_mod, command_palette_mod],
    );
}

#[cfg(test)]
pub(crate) fn checkout_test_cx() -> PooledCx {
    makepad_widgets_core::test_cx::checkout_test_cx(script_mod)
}

#[cfg(test)]
pub(crate) fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    makepad_widgets_core::test_cx::on_test_cx(f)
}
