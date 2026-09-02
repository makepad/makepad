//! Professional music-notation application UI.
//!
//! The crate owns the product state, score surface, pianist experience,
//! editing chrome, dialogs, keymap, playback bridge, and file seams. The
//! `apps/score` crate is intentionally only a window and event adapter.

pub use makepad_widgets;
pub use makepad_score_view::{
    build, build_bass_tab_score, build_drum_score, build_pitched_score, view, BuildOptions,
    DrumHit, DrumVoice, PitchedNote, ScoreView, ScoreViewRef, ScoreViewWidgetExt,
    ScoreViewWidgetRefExt,
};
pub use makepad_score_view::ScoreDocument as ScoreViewDocument;

pub mod action;
pub mod document;
pub mod hybrid;
pub mod engrave;
pub mod font;
pub mod keymap;
pub mod library;
pub mod playback;
pub mod prefs;
pub mod sound;
pub mod spacing;
pub mod state;
pub mod theme;
pub mod ui;

pub use action::*;
pub use library::{LibraryEntry, MusicLibrary};
pub use prefs::ScorePrefs;
pub use sound::{SoundParam, SoundSettings};
pub use document::*;
pub use keymap::*;
pub use state::*;

use makepad_widgets::ScriptVm;

/// Register the score theme and widgets in dependency order.
pub fn script_mod(vm: &mut ScriptVm) {
    makepad_score_view::script_mod(vm);
    theme::script_mod(vm);
    ui::widgets::script_mod(vm);
    ui::canvas::script_mod(vm);
    ui::shell::script_mod(vm);
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::{Cx, Event, ScriptVmBase};

    #[test]
    fn score_widget_dsl_registers_without_errors() {
        let mut cx = Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}));
        let mut std = ();
        let mut vm = ScriptVm {
            host: &mut cx,
            std: &mut std,
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.captured_errors = Some(Vec::new());
        makepad_widgets::makepad_platform::script::script_mod(&mut vm);
        makepad_widgets::script_mod(&mut vm);
        super::script_mod(&mut vm);
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "score widget DSL errors: {errors:#?}");
    }
}
