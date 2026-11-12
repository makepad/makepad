//! A Markdown document shown as its final text and edited there: the rows of
//! a virtual list drawn by the markdown walk of `makepad-widgets-rich-text`,
//! over the `CodeDocument` a code view edits as source. The block index and
//! the source mapping live in `makepad-markdown-edit`, which needs no window.

pub use makepad_markdown_edit;
use makepad_widgets_core::*;

pub mod editor;
mod rowmap;

pub use editor::{MarkdownEditor, MarkdownEditorAction};

/// Registers the final view. Call after the rich text family and the code
/// editor are registered.
pub fn script_mod(vm: &mut ScriptVm) {
    crate::editor::script_mod(vm);
}
