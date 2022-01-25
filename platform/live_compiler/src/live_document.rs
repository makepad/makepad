//use makepad_id_macros2::*;
use {
    crate::{
        span::{TextPos, TextSpan},
        live_token::{TokenWithSpan,LiveTokenId},
        live_node::LiveNode,
    }
};

#[derive(Default)]
pub struct LiveOriginal {
    pub nodes: Vec<LiveNode >,
    pub edit_info: Vec<LiveNode>,

    pub strings: Vec<char>,
    pub tokens: Vec<TokenWithSpan>,
}

#[derive(Default)]
pub struct LiveExpanded {
    pub nodes: Vec<LiveNode >,
}

impl LiveExpanded {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
        }
    }

    pub fn resolve_ptr(&self, index: usize) -> &LiveNode {
        &self.nodes[index]
    }

}

impl LiveOriginal {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edit_info: Vec::new(),
            strings: Vec::new(),
            tokens: Vec::new(),
        }
    }
    
    pub fn resolve_ptr(&self, index: usize) -> &LiveNode {
        &self.nodes[index]
    }
    
    pub fn get_tokens(&self, token_start: usize, token_count: usize) -> &[TokenWithSpan] {
        &self.tokens[token_start..(token_start + token_count)]
    }
    
    pub fn find_token_by_pos(&self, pos:TextPos) -> Option<usize> {
        for (token_index, token) in self.tokens.iter().enumerate() {
            if pos.line  == token.span.start.line
                && pos.column >= token.span.start.column
                && pos.column < token.span.end.column {
                    return Some(token_index)
            }
        }
        None
    }

    pub fn get_string(&self, string_start: usize, string_count: usize, out:&mut String) {
        let chunk = &self.strings[string_start..(string_start + string_count)];
        out.clear();
        for chr in chunk {
            out.push(*chr);
        }
    }
    
    pub fn token_id_to_span(&self, token_id: LiveTokenId) -> TextSpan {
        self.tokens[token_id.token_index() as usize].span
    }
}


