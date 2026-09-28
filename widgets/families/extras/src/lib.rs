//! Charts, the colour picker family, dates, the dropzone, chat, glass panels and the vector widget: small families that share one crate.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod calendar;
pub mod chart;
pub mod chart_shapes;
pub mod chat;
pub mod color;
pub mod date_picker;
pub mod dropzone;
pub mod glass_panel;
pub mod time_picker;
pub mod vector;
pub mod waveform;

/// Registers the glass panel family.
pub fn glass_mod(vm: &mut ScriptVm) {
    crate::glass_panel::script_mod(vm);
}

/// Registers the calendar, the date picker and the time picker.
pub fn dates_mod(vm: &mut ScriptVm) {
    crate::calendar::script_mod(vm);
    crate::date_picker::script_mod(vm);
    crate::time_picker::script_mod(vm);
}

/// Registers the colour picker family.
pub fn color_mod(vm: &mut ScriptVm) {
    crate::color::script_mod(vm);
}

/// Registers the waveform lane, the chart shapes and the charts.
pub fn charts_mod(vm: &mut ScriptVm) {
    crate::waveform::script_mod(vm);
    crate::chart_shapes::script_mod(vm);
    crate::chart::script_mod(vm);
}

/// Registers the chat bubbles and the chat list.
pub fn chat_mod(vm: &mut ScriptVm) {
    crate::chat::script_mod(vm);
}

/// Registers the file drop target and the upload list.
pub fn dropzone_mod(vm: &mut ScriptVm) {
    crate::dropzone::script_mod(vm);
}

/// Registers the vector drawing widget.
pub fn vector_mod(vm: &mut ScriptVm) {
    crate::vector::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with every family in this crate.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(
        vm,
        WindowFamilies::default(),
        &[glass_mod, dates_mod, color_mod, charts_mod, chat_mod, dropzone_mod, vector_mod],
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
