//! The AI chat slot (F10) a Window carries when the app links the `makepad-aichat` crate.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod ai_slot;

/// Registers the slot where the Window names it ([`WindowFamilies::ai`])
/// and hands the window its F10 intercept.
pub fn ai_mod(vm: &mut ScriptVm) {
    crate::ai_slot::install_ai_hooks(vm.cx_mut());
    crate::ai_slot::script_mod(vm);
}
