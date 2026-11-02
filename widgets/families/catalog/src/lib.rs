//! Alerts and banners, accordions, breadcrumbs, pagination, floating panels,
//! spinners, toasts, empty states, code blocks and the tour.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `catalog` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod accordion;
pub mod alert;
pub mod breadcrumb;
pub mod code_block;
pub mod empty_state;
pub mod floating_panel;
pub mod pagination;
pub mod spinner;
pub mod toast;
pub mod tour;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn catalog_mod(vm: &mut ScriptVm) {
    crate::alert::script_mod(vm);
    crate::accordion::script_mod(vm);
    crate::breadcrumb::script_mod(vm);
    crate::pagination::script_mod(vm);
    crate::floating_panel::script_mod(vm);
    crate::spinner::script_mod(vm);
    crate::toast::script_mod(vm);
    crate::empty_state::script_mod(vm);
    crate::code_block::script_mod(vm);
    crate::tour::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with this family.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[catalog_mod]);
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn checkout_test_cx() -> PooledCx {
    makepad_widgets_core::test_cx::checkout_test_cx(script_mod)
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    makepad_widgets_core::test_cx::on_test_cx(f)
}
