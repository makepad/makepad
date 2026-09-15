//! Playback-free score engraving, retained pages, score builders, and a lean widget.

pub use makepad_widgets;

pub mod build;
pub mod document;
pub mod engrave;
pub mod font;
pub mod spacing;
mod title;
pub mod view;

pub use build::*;
pub use document::*;
pub use font::{
    ensure_default_font, music_font, music_font_summary, set_embedded_music_font, EmbeddedFont,
};
#[cfg(feature = "embed-bravura")]
pub use font::bravura;
pub use view::*;

use makepad_widgets::ScriptVm;

/// Register the draw-only score widget.
pub fn script_mod(vm: &mut ScriptVm) {
    view::script_mod(vm);
}
