use {
    crate::{
        makepad_platform::*,
        makepad_live_tokenizer::{
            delta::{Delta, OperationRange},
            text::Text,
            full_token::{Delim, FullToken},
            tokenizer::{TokenRange, TokenPos}
        },
        makepad_live_compiler::{LiveTokenId, LiveToken, TextPos, LivePtr},
        code_editor::{
            token_cache::TokenCache,
        },
    },
    std::{
        ops::{Deref, DerefMut, Index},
        slice::Iter,
    },
};

#[derive(Debug, Clone, Copy)]
pub struct InlineEditBind {
    pub live_token: LiveToken,
    pub live_ptr: LivePtr,
    pub live_token_id: LiveTokenId,
    pub edit_token_index: usize,
}

#[derive(Debug, Default)]
pub struct Line {
    pub fold_button_id: Option<u64>,
    pub is_clean: bool,
    pub line_opened: f32,
    pub line_zoomout: f32,
    pub items: Vec<InlineEditBind>
}

pub struct InlineCache {
    pub fold_button_alloc: u64,
    pub lines: Vec<Line>,
    pub live_register_range: Option<TokenRange>,
    pub is_clean: bool,
}

impl InlineCache {
    pub fn new(text: &Text) -> InlineCache {
        InlineCache {
            fold_button_alloc: 0,
            is_clean: false,
            live_register_range: None,
            lines: (0..text.as_lines().len())
                .map( | _ | Line::default())
                .collect::<Vec<_ >> (),
        }
    }
    
    pub fn refresh_live_register_range(&mut self, token_cache: &TokenCache) {
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
            for (index, token) in token_line.tokens().iter().enumerate() {
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
                        _ => {state = State::Stack(TokenPos {line, index}, 0)}
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
                    State::Term(_, _) => panic!()
                }
                //column += token.len;
            }
        }
        if let State::Term(start, end) = state {
            // alright we have a range.
            self.live_register_range = Some(TokenRange {start, end})
        }
    }
    
    pub fn invalidate_all(&mut self) {
        self.is_clean = false;
        for line in &mut self.lines {
            line.items.clear();
            line.is_clean = false;
        }
    }
    
    pub fn invalidate(&mut self, delta: &Delta) {
        
        // detect no-op line wise, to keep the folding state
        let ranges = delta.operation_ranges();
        if ranges.count() == 2 {
            let mut ranges = delta.operation_ranges();
            if let OperationRange::Delete(del_range) = ranges.next().unwrap() {
                if let OperationRange::Insert(insert_range) = ranges.next().unwrap() {
                    if del_range.start.line == insert_range.start.line &&
                    del_range.end.line == insert_range.end.line {
                        self.is_clean = false;
                        for line in &mut self.lines[insert_range.start.line..insert_range.end.line] {
                            line.is_clean = false;
                            line.items.clear();
                        }
                        return
                    }
                }
            }
        }
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
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        
        // lets check all our matched pointers generations.
        for line_cache in self.lines.iter_mut() {
            if line_cache.items.iter().any( | bind | !live_registry.generation_valid(bind.live_ptr)) {
                line_cache.items.clear();
                line_cache.is_clean = false;
                self.is_clean = false;
            }
        }
        
        if self.is_clean {
            return
        }
        self.is_clean = true;
        
        if self.live_register_range.is_none() {
            self.refresh_live_register_range(token_cache);
        }
        
        if self.live_register_range.is_none() {
            self.invalidate_all();
            return
        }
        let range = self.live_register_range.unwrap();
        
        let path = if let Some(prefix) = path.strip_prefix("/Users/admin/makepad/edit_repo/sub_repo") {prefix}
        else {
            path
        };
        //println!("{}", &path.strip_prefix("/Users/admin/makepad/edit_repo/").unwrap());
        let file_id = if let Some(file_id) = live_registry.path_str_to_file_id(path) {file_id}
        else {
            println!("inline_cache::refresh: File not found {} ", path);
            return
        };
        let live_file = &live_registry.live_files[file_id.to_index()];
        let expanded = &live_file.expanded;
        
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
            if line_cache.is_clean {
                continue
            }
            
            line_cache.is_clean = true;
            if line_cache.items.len() != 0 {
                panic!();
            }
            if line_cache.fold_button_id.is_none() {
                line_cache.fold_button_id = Some(self.fold_button_alloc);
                self.fold_button_alloc += 1;
            }
            
            
            let tokens_line = &token_cache[line].tokens();
            let mut column = 0;
            for (edit_token_index, token) in tokens_line.iter().enumerate() {
                
                // try to filter things a bit before plugging it into the expensive search process
                let is_prop_assign =
                token.is_ident()
                    && edit_token_index + 1 < tokens_line.len()
                    && tokens_line[edit_token_index + 1].is_punct_id(id!(:));
                
                if is_prop_assign || token.is_value_type() {
                    
                    if let Some(live_token_index) = live_file.original.find_token_by_pos(TextPos {line: line as u32, column}) {
                        
                        let live_token_id = makepad_live_compiler::LiveTokenId::new(file_id, live_token_index);
                        let search_in_dsl = token.is_value_type();
                        
                        if let Some(node_index) = expanded.nodes.first_node_with_token_id(live_token_id, search_in_dsl) {
                            
                            let live_token_id = if is_prop_assign { // get the thing after the :
                                makepad_live_compiler::LiveTokenId::new(file_id, live_token_index + 2)
                            }
                            else {live_token_id};
                            
                            let live_ptr = LivePtr {file_id, index: node_index as u32, generation: live_file.generation};
                            // if its a DSL, we should filter here
                            //let live_node = live_registry.ptr_to_node(live_ptr);
                            let bind = InlineEditBind {
                                live_token: *live_file.original.tokens[live_token_index],
                                live_ptr,
                                live_token_id,
                                edit_token_index: edit_token_index
                            };
                            line_cache.items.push(bind);
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

impl DerefMut for InlineCache {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.lines
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
