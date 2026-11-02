//! Video, the video player and its glass controls, the playback bar, level meters, the marquee, content placeholders
//! and media frames.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `media` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod level_meter;
pub mod marquee;
pub mod media;
pub mod placeholder;
pub mod playback_bar;
pub mod player_controls;
pub mod video;
pub mod video_player;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn media_mod(vm: &mut ScriptVm) {
    crate::playback_bar::script_mod(vm);
    crate::level_meter::script_mod(vm);
    crate::marquee::script_mod(vm);
    crate::placeholder::script_mod(vm);
    crate::media::script_mod(vm);
    crate::video::script_mod(vm);
    crate::player_controls::script_mod(vm);
    crate::video_player::script_mod(vm);
}

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[media_mod]);
}
