pub mod code_editor;
pub mod root;

pub use self::{code_editor::CodeEditor, root::Root};

use makepad_render::*;

pub fn style(cx: &mut Cx) {
    CodeEditor::style(cx);
    Root::style(cx);
}
