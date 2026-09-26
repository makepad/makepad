//! Text flow, rich text, HTML, Markdown, LaTeX maths and the log list.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;
pub use makepad_html;

pub mod html;
pub mod log_list;
pub mod markdown;
pub mod math_view;
pub mod rich_text;
pub mod text_flow;

#[cfg(test)]
mod portal_list_tests;

/// Registers the family: the text flow after the rich text editor, the
/// log list, HTML and Markdown on the text flow, then the maths view.
pub fn rich_text_mod(vm: &mut ScriptVm) {
    crate::rich_text::script_mod(vm);
    crate::text_flow::script_mod(vm);
    crate::log_list::script_mod(vm);
    crate::html::script_mod(vm);
    crate::markdown::script_mod(vm);
    crate::math_view::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with the rich text family.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[rich_text_mod]);
}

#[cfg(test)]
pub(crate) fn checkout_test_cx() -> PooledCx {
    makepad_widgets_core::test_cx::checkout_test_cx(script_mod)
}

#[cfg(test)]
pub(crate) fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    makepad_widgets_core::test_cx::on_test_cx(f)
}
