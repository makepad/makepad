pub use makepad_widgets;
use makepad_widgets::*;

pub mod char;
pub mod code_editor;
pub mod decoration;
pub mod document;
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
pub mod code_view;

pub use self::{
    code_editor::CodeEditor, document::CodeDocument, history::History, layout::Line,
    selection::Selection, session::CodeSession, settings::Settings, token::Token, tokenizer::Tokenizer,
};

pub fn live_design(cx: &mut Cx) {
    crate::code_editor::live_design(cx);
    crate::code_view::live_design(cx);
}
