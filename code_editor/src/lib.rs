pub use makepad_widgets;
use makepad_widgets::*;

pub mod change;
pub mod char;
pub mod code_editor;
pub mod extent;
pub mod history;
pub mod inlays;
pub mod iter;
pub mod line;
pub mod move_ops;
pub mod point;
pub mod range;
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
    change::Change,
    code_editor::CodeEditor,
    extent::Extent,
    history::History,
    line::Line,
    point::Point,
    range::Range,
    selection::Selection,
    settings::Settings,
    state::{Document, Session},
    text::Text,
    token::Token,
    tokenizer::Tokenizer,
};

pub fn live_design(cx: &mut Cx) {
    crate::code_editor::live_design(cx);
}
