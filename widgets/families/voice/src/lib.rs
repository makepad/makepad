//! The voice input wave in a Window caption bar.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod voice_wave;
mod window_voice_input;

/// Registers the wave where the Window names it ([`WindowFamilies::voice`])
/// and lets the window hand it its actions.
pub fn voice_mod(vm: &mut ScriptVm) {
    crate::voice_wave::install_voice_hooks(vm.cx_mut());
    crate::voice_wave::script_mod(vm);
}
