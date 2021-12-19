use {
    makepad_render::makepad_live_tokenizer::{
        delta::{Delta, OperationRange},
        text::Text,
        range::Range,
        position::Position,
        token::{Delim, TokenKind},
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
    pub register_range: Option<Range>,
    pub is_clean: bool,
}

impl InlineCache {
    pub fn new(text: &Text) -> InlineCache {
        InlineCache {
            is_clean: false,
            register_range: None,
            lines: (0..text.as_lines().len())
                .map( | _ | Line::default())
                .collect::<Vec<_ >> (),
        }
    }
    
    pub fn parse_live_register_range(&mut self, token_cache: &TokenCache) {
        enum State {
            Scan,
            Bang,
            Brace,
            First,
            Stack(Position, usize),
            Term(Position, Position)
        }
        let mut state = State::Scan;
        // first we scan for live_register!{ident
        'outer: for (line, token_line) in token_cache.iter().enumerate() {
            let mut column = 0;
            for token in token_line.tokens() {
                match state {
                    State::Scan => match token.kind {
                        TokenKind::Ident(id!(live_register)) => {state = State::Bang}
                        _ => ()
                    }
                    State::Bang => match token.kind {
                        TokenKind::Punct(id!(!)) => {state = State::Brace}
                        TokenKind::Whitespace | TokenKind::Comment => (),
                        _ => {state = State::Scan}
                    }
                    State::Brace => match token.kind {
                        TokenKind::Open(Delim::Brace) => {state = State::First}
                        TokenKind::Whitespace | TokenKind::Comment => (),
                        _ => {state = State::Scan}
                    }
                    State::First => match token.kind {
                        TokenKind::Whitespace | TokenKind::Comment => (),
                        _=>{state = State::Stack(Position {line, column}, 0)}
                    }
                    State::Stack(start, depth) => {
                        match token.kind {
                            TokenKind::Open(_) => {state = State::Stack(start, depth + 1)}
                            TokenKind::Close(_) => {
                                if depth == 0 { // end of scan
                                    state = State::Term(start, Position {line, column});
                                    break 'outer
                                }
                                state = State::Stack(start, depth - 1)
                            }
                            TokenKind::Whitespace | TokenKind::Comment => (),
                            _ => ()
                        }
                    }
                    State::Term(_,_)=>panic!()
                }
                column += token.len;
            }
        }
        if let State::Term(start,end) = state{
            // alright we have a range.
            println!("WE HAVE A RANGE");
            self.register_range = Some(Range{start, end})
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
        
        //if self.register_range.is_none() {
        self.parse_live_register_range(token_cache);
        //}
        
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        
        let file_id = live_registry.path_str_to_file_id(path).unwrap();
        
        let live_file = &live_registry.live_files[file_id.to_index()];
        let expanded = &live_registry.expanded[file_id.to_index()];
        
        if self.lines.len() != token_cache.len() {
            panic!();
        }
        for (line, line_cache) in self.lines.iter_mut().enumerate() {
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
                if let TokenKind::Ident(_) = token.kind {
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
