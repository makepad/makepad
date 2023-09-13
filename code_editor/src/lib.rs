pub use makepad_widgets;
use makepad_widgets::*;

pub mod char;
pub mod code_editor;
pub mod document;
pub mod history;
pub mod inlays;
pub mod iter;
pub mod layout;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;
pub mod widgets;
pub mod wrap;

pub use self::{
    code_editor::CodeEditor, document::Document, history::History, layout::Line,
    selection::Selection, settings::Settings, state::Session, token::Token, tokenizer::Tokenizer,
};

pub fn live_design(cx: &mut Cx) {
    crate::code_editor::live_design(cx);
}
