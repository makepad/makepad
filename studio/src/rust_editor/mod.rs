use crate::makepad_platform::*;

pub mod rust_editor;
pub mod rust_tokenizer;

pub fn live_design(cx: &mut Cx){
    crate::rust_editor::rust_editor::live_design(cx);
}
