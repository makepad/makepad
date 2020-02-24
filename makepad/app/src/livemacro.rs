/*
use makepad_render::*;
use makepad_widget::*;

pub struct LiveMacroPick{
    token:usize,
}

pub struct LiveMacroShader{
    token:usize,
}

pub enum LiveMacro {
    Pick(LiveMacroPick),
    Shader(LiveMacroShader)
}

pub struct LiveMacros{
    macros: Vec<LiveMacro>
}

fn tok_cmp(name: &str, tok: &[char]) -> bool {
    for (index, c) in name.chars().enumerate() {
        if tok[index] != c {
            return false
        }
    }
    return true
}

impl LiveMacros{
    pub fn parse(old_index:&mut usize, new_index:&mut usize, text_buffer:&TextBuffer){
        //let new_tok = &text_buffer.token_chunks[new_index];
        //let new_tok_slice = &text_buffer.flat_text[new_tok.offset..new_tok.offset + new_tok.len];
        //let old_tok = &text_buffer.old_token_chunks[old_index];
        //let old_tok_slice = &text_buffer.flat_text[old_tok.offset..old_tok.offset + old_tok.len];
    }
}*/