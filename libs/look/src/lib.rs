//! Stage's look.
//!
//! [`palette`] is the one colour table Stage paints with; [`console_controls`]
//! and [`fit_tick`] are the console's knobs, faders and ticks drawn in it.
//!
//! [`script_mod`] registers these widgets only. The host calls
//! `makepad_widgets::script_mod` first.

pub mod console_controls;
pub mod fit_tick;
pub mod palette;

/// Register the palette and the controls on `mod.widgets`.
pub fn script_mod(vm: &mut makepad_widgets::ScriptVm) {
    palette::script_mod(vm);
    console_controls::script_mod(vm);
    fit_tick::script_mod(vm);
}
