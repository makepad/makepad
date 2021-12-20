use {
    makepad_render::makepad_live_tokenizer::{
        delta::{Delta, OperationRange},
        text::Text,
        full_token::{Delim, FullToken},
        tokenizer::{TokenRange, TokenPos}
    },
    crate::code_editor::{
        token_cache::TokenCache,
    },
    std::{
        ops::{Deref, Index},
        slice::Iter,
    },
    makepad_render::makepad_live_compiler::{TextPos, LivePtr},
    makepad_render::*,
};

#[derive(Debug)]
pub struct LineItem {
    pub live_ptr: LivePtr,
    pub edit_token_index: usize,
}

#[derive(Debug, Default)]
pub struct Line {
    pub is_clean: bool,
    pub items: Vec<LineItem>
}

pub struct InlineCache {
    pub lines: Vec<Line>,
    pub token_range: Option<TokenRange>,
    pub is_clean: bool,
}

impl InlineCache {
    pub fn new(text: &Text) -> InlineCache {
        InlineCache {
            is_clean: false,
            token_range: None,
            lines: (0..text.as_lines().len())
                .map( | _ | Line::default())
                .collect::<Vec<_ >> (),
        }
    }
    
    pub fn refresh_token_range(&mut self, token_cache: &TokenCache) {
        enum State {
            Scan,
            Bang,
            Brace,
            First,
            Stack(TokenPos, usize),
            Term(TokenPos, TokenPos)
        }
        let mut state = State::Scan;
        // first we scan for live_register!{ident
        'outer: for (line, token_line) in token_cache.iter().enumerate() {
            //let mut column = 0;
            for (index,token) in token_line.tokens().iter().enumerate() {
                match state {
                    State::Scan => match token.token {
                        FullToken::Ident(id!(live_register)) => {state = State::Bang}
                        _ => ()
                    }
                    State::Bang => match token.token {
                        FullToken::Punct(id!(!)) => {state = State::Brace}
                        FullToken::Whitespace | FullToken::Comment => (),
                        _ => {state = State::Scan}
                    }
                    State::Brace => match token.token {
                        FullToken::Open(Delim::Brace) => {state = State::First}
                        FullToken::Whitespace | FullToken::Comment => (),
                        _ => {state = State::Scan}
                    }
                    State::First => match token.token {
                        FullToken::Whitespace | FullToken::Comment => (),
                        _=>{state = State::Stack(TokenPos {line, index}, 0)}
                    }
                    State::Stack(start, depth) => {
                        match token.token {
                            FullToken::Open(_) => {state = State::Stack(start, depth + 1)}
                            FullToken::Close(_) => {
                                if depth == 0 { // end of scan
                                    state = State::Term(start, TokenPos {line, index});
                                    break 'outer
                                }
                                state = State::Stack(start, depth - 1)
                            }
                            FullToken::Whitespace | FullToken::Comment => (),
                            _ => ()
                        }
                    }
                    State::Term(_,_)=>panic!()
                }
                //column += token.len;
            }
        }
        if let State::Term(start,end) = state{
            // alright we have a range.
            self.token_range = Some(TokenRange{start, end})
        }
    }
    
    pub fn invalidate(&mut self, delta: &Delta) {
        for operation_range in delta.operation_ranges() {
            match operation_range {
                OperationRange::Insert(range) => {
                    self.is_clean = false;
                    self.lines[range.start.line] = Line::default();
                    self.lines.splice(
                        range.start.line..range.start.line,
                        (0..range.end.line - range.start.line).map( | _ | Line::default()),
                    );
                }
                OperationRange::Delete(range) => {
                    self.is_clean = false;
                    self.lines.drain(range.start.line..range.end.line);
                    self.lines[range.start.line] = Line::default();
                }
            }
        }
    }
    
    pub fn refresh(&mut self, cx: &mut Cx, path: &str, token_cache: &TokenCache) {
        if self.is_clean {
            return
        }
        self.is_clean = true;
        
        if self.token_range.is_none() {
            self.refresh_token_range(token_cache);
        }
        
        if self.token_range.is_none(){
            return
        }
        let range = self.token_range.unwrap();
        
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        
        let file_id = live_registry.path_str_to_file_id(path).unwrap();
        
        let live_file = &live_registry.live_files[file_id.to_index()];
        let expanded = &live_registry.expanded[file_id.to_index()];
        
        if self.lines.len() != token_cache.len() {
            panic!();
        }

        for line_cache in self.lines[0..range.start.line].iter_mut() {
            line_cache.items.clear();
            line_cache.is_clean = true;
        }
        for line_cache in self.lines[range.end.line..].iter_mut() {
            line_cache.items.clear();
            line_cache.is_clean = true;
        }
        for (line, line_cache) in self.lines[range.start.line..range.end.line].iter_mut().enumerate() {
            let line = line + range.start.line;
            if line_cache.is_clean { // line not dirty
                continue
            }
            
            line_cache.is_clean = true;
            if line_cache.items.len() != 0 {
                panic!();
            }
            let tokens_line = &token_cache[line];
            let mut column = 0;
            for (edit_token_index, token) in tokens_line.tokens().iter().enumerate() {
                if let FullToken::Ident(_) = token.token {
                    if let Some(live_token_index) = live_file.document.find_token_by_pos(TextPos {line: line as u32, column}) {
                        let match_token_id = makepad_live_compiler::TokenId::new(file_id, live_token_index);
                        if let Some(node_index) = expanded.nodes.first_node_with_token_id(match_token_id) {
                            let live_ptr = LivePtr {file_id, index: node_index as u32};
                            
                            line_cache.items.push(LineItem {
                                live_ptr,
                                edit_token_index: edit_token_index
                            });
                        }
                    }
                }
                column += token.len as u32;
            }
        }
    }
}


impl Deref for InlineCache {
    type Target = [Line];
    
    fn deref(&self) -> &Self::Target {
        &self.lines
    }
}

impl Index<usize> for InlineCache {
    type Output = Line;
    
    fn index(&self, index: usize) -> &Self::Output {
        &self.lines[index]
    }
}

impl<'a> IntoIterator for &'a InlineCache {
    type Item = &'a Line;
    type IntoIter = Iter<'a, Line>;
    
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Line {
}
