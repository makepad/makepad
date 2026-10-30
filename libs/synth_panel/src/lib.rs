//! The synthesiser's widgets for a score's editor panel (the AI-written
//! `let editor_panel = fn() { View{ … } }` beside the code): pictures of an
//! instrument that are dragged (envelope, filter, wave), its live sound
//! (scope and level), a keyboard to hear it on, and the instrument switch.
//! They bind to score nodes by name and write through the panel widgets'
//! host, beside `makepad_value_pickers::panel`'s knobs, sliders,
//! steps and modules.
//!
//! - [`displays`]: `SynthEnvelope`, `SynthFilter`, `SynthWave`, `SynthScope`.
//! - [`keys`]: `SynthKeys`.
//! - [`morph`]: a Morph's `SynthTable`, `SynthLfo` and `SynthMods`.
//! - [`picker`]: `SynthPicker`.
//! - [`generators`]: `SynthGen`, a code generator as a control.
//! - [`tree`]: `ScoreTree`, the voices and their instruments (the panel's
//!   fixed top), unfolding in place under a voice: [`track`]: `SynthTrack`,
//!   its timeline and melody variants; [`tweaks`]: `SynthTweaks`, its
//!   instrument's controls (both drawn by the tree there).
//! - [`host`]: what the desk gives them (types, live sound, choices).
//! - [`spec`]: each field's range, taper and unit; [`dsp`]: the curves.

pub mod choices;
pub mod displays;
pub mod dsp;
pub mod generators;
pub mod host;
pub mod keys;
pub mod live;
pub mod make;
pub mod morph;
pub mod picker;
pub mod shaders;
pub mod spec;
pub mod track;
pub mod tree;
pub mod tweaks;
pub mod values;

/// The widgets as a score's agent is told them.
pub const CATALOGUE: &str = include_str!("catalogue.md");

/// Register the widgets (the screen shader first).
pub fn script_mod(vm: &mut makepad_widgets::ScriptVm) {
    shaders::script_mod(vm);
    displays::script_mod(vm);
    keys::script_mod(vm);
    morph::script_mod(vm);
    picker::script_mod(vm);
    generators::script_mod(vm);
    tree::script_mod(vm);
    track::script_mod(vm);
    tweaks::script_mod(vm);
}

/// What a panel's isolate needs to name them (`register_splash_isolate_mod`,
/// after the panel widgets' own): the widgets, and the isolate's prelude
/// made again so a panel names them as it names a View.
pub fn isolate_mod(vm: &mut makepad_widgets::ScriptVm) {
    use makepad_widgets::*;
    // This crate's widgets (a bare `script_mod` here would be the widget
    // library's own, glob-imported above).
    crate::script_mod(vm);
    makepad_widgets::script_eval!(vm, {
        mod.prelude.widgets = {
            ..mod.prelude.widgets_header,
            theme: mod.theme,
            ..mod.widgets,
        }
    });
}

#[cfg(test)]
mod tests;
