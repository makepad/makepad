pub use makepad_widgets;
use makepad_widgets::*;

pub mod char;
pub mod code_editor;
pub mod code_view;
pub mod decoration;
pub mod document;
pub mod draw_selection;
pub mod history;
pub mod inlays;
pub mod iter;
pub mod layout;
pub mod selection;
pub mod session;
pub mod settings;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;
pub mod widgets;
pub mod wrap;

pub use self::{
    code_editor::CodeEditor, document::CodeDocument, history::History, layout::Line,
    selection::Selection, session::CodeSession, settings::Settings, token::Token,
    tokenizer::Tokenizer,
};

pub fn script_mod(vm: &mut ScriptVm) {
    crate::draw_selection::script_mod(vm);
    crate::code_editor::script_mod(vm);
    crate::code_view::script_mod(vm);
}
