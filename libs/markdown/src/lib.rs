//! Markdown parsing independent of the host's document, storage, and UI model.
//!
//! [`markdown_render::render`] resolves references and other document-wide
//! structure before returning fragments with their source positions. Source HTML
//! is sanitized before trusted [`render::Renderer`] extensions are inserted.
//! The host owns resource loading, navigation, and typesetting; this crate does
//! not access the filesystem or network and does not require a browser engine.
//!
//! Optional `emoji` support provides standard shortcodes. The `editormd` feature
//! additionally enables Editor.md aliases in rendered block text; host HTML
//! renderers opt into those aliases through [`render::editormd_emoji_text`].

pub mod markdown_render;
pub mod markup;
pub mod math;
pub mod render;

/// Escape text for HTML content or double-quoted attribute values.
pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
