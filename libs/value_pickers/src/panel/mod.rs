//! The widgets of a scene's editor panel: the small Splash program the
//! scene's agent writes (`let editor_panel = fn() { View{ … } }`), run by
//! the desk in a Splash isolate. Every widget here binds itself to a path
//! in the scene's code (`path: "WORD_SPARKS.count"`): it shows what is
//! written there and sets it as it moves ([`host`]); an optional
//! `on_change: |v| { … }` runs after each change (a macro driving several
//! paths), `on_commit` when a gesture ends.
//!
//! The catalogue, as the scene's agent reads it, is [`CATALOGUE`]: every
//! widget, one line and an example each (numbers, choices, colour and type,
//! shapes, modules with presets / A-B morph / randomise / mood pad, the
//! palette editor and its generators).

pub mod control;
pub mod field;
pub mod host;
pub mod instruments;
pub mod module;
pub mod palette;
pub mod palette_gen;
pub mod rich;
pub mod scene;
pub mod tree;

pub use control::{PanelControl, PanelControlAction, PanelControlRef, PanelKind};
pub use host::{PanelHost, PanelValue};

/// The widgets as the scene's agent is told them: each one line and an
/// example, and a whole panel.
pub const CATALOGUE: &str = include_str!("catalogue.md");

use makepad_widgets::*;

/// Register the panel's widgets.
pub fn script_mod(vm: &mut ScriptVm) {
    control::script_mod(vm);
    rich::script_mod(vm);
    palette::script_mod(vm);
    module::script_mod(vm);
    instruments::script_mod(vm);
    tree::script_mod(vm);
    scene::script_mod(vm);
}

/// Everything a panel's isolate needs, for the host to give each new Splash
/// isolate (`register_splash_isolate_mod`): the console palette (`mod.stage`)
/// and controls the pickers are drawn with, the pickers, and the panel's
/// widgets.
pub fn isolate_mod(vm: &mut ScriptVm) {
    makepad_look::palette::script_mod(vm);
    makepad_look::console_controls::script_mod(vm);
    crate::script_mod(vm);
    // The isolate's prelude was made from `mod.widgets` before these were
    // in it: made again, so a panel names them as it names a View.
    makepad_widgets::script_eval!(vm, {
        mod.prelude.widgets = {
            ..mod.prelude.widgets_header,
            theme: mod.theme,
            ..mod.widgets,
        }
    });
}
