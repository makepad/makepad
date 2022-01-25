//use crate::id::Id;
use {
    std::collections::{HashMap, HashSet, BTreeSet},
    crate::{
        makepad_id_macros::*,
        makepad_live_tokenizer::{Delim, TokenPos, TokenRange, TokenWithLen, FullToken, LiveId, State, Cursor},
        live_error::{LiveError, LiveErrorSpan, LiveErrorOrigin, LiveFileError},
        live_parser::LiveParser,
        live_document::{LiveOriginal, LiveExpanded},
        live_node::{LiveNodeOrigin, LiveNode, LiveValue, LiveType, LiveTypeInfo},
        live_node_vec::{LiveNodeSlice, LiveNodeVec, LiveNodeMutReader},
        live_ptr::{LiveFileId, LivePtr, LiveModuleId, LiveFileGeneration},
        live_token::{LiveToken, LiveTokenId, TokenWithSpan},
        span::{TextSpan, TextPos},
        live_expander::{LiveExpander}
    }
};

#[derive(Default)]
pub struct LiveFile {
    pub reexpand: bool,
    
    pub module_id: LiveModuleId,
    pub start_pos: TextPos,
    pub file_name: String,
    pub source: String,
    pub deps: BTreeSet<LiveModuleId>,
    
    pub generation: LiveFileGeneration,
    
    pub original: LiveOriginal,
    pub next_original: Option<LiveOriginal>,
    pub expanded: LiveExpanded,
    
    pub live_type_infos: Vec<LiveTypeInfo>,
}


pub struct LiveRegistry {
    pub file_ids: HashMap<String, LiveFileId>,
    pub module_id_to_file_id: HashMap<LiveModuleId, LiveFileId>,
    pub live_files: Vec<LiveFile>,
    pub live_type_infos: HashMap<LiveType, LiveTypeInfo>,
    pub ignore_no_dsl: HashSet<LiveId>,
    pub main_module: Option<LiveFileId>,
}

impl Default for LiveRegistry {
    fn default() -> Self {
        let mut ignore_no_dsl = HashSet::new();
        ignore_no_dsl.insert(id!(Namespace));
        ignore_no_dsl.insert(id!(Struct));
        Self {
            ignore_no_dsl,
            main_module: None,
            file_ids: HashMap::new(),
            module_id_to_file_id: HashMap::new(),
            live_files: Vec::new(),
            live_type_infos: HashMap::new(),
            //mutated_apply: None,
            //mutated_tokens: None
        }
    }
}

pub struct LiveDocNodes<'a> {
    pub nodes: &'a [LiveNode],
    pub file_id: LiveFileId,
    pub index: usize
}

#[derive(Copy, Clone, Debug)]
pub enum LiveScopeTarget {
    LocalPtr(usize),
    LivePtr(LivePtr)
}

#[derive(Clone, Debug, PartialEq)]
pub enum LiveEditEvent {
    ReparseDocument,
    Mutation {tokens: Vec<LiveTokenId>, apply: Vec<LiveNode>, live_ptrs: Vec<LivePtr>},
}

impl LiveRegistry {
    pub fn add_ignore_no_dsl(&mut self, live_ids:&[LiveId]){
        for id in live_ids{
            self.ignore_no_dsl.insert(*id);
        }
    }
    
    pub fn generation_valid(&self, live_ptr: LivePtr) -> bool {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        doc.generation == live_ptr.generation 
    }
    
    pub fn ptr_to_node(&self, live_ptr: LivePtr) -> &LiveNode {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation{
            panic!("ptr_to_node generation invalid for file {} gen:{} ptr:{}", doc.file_name, doc.generation, live_ptr.generation);
        }
        &doc.expanded.resolve_ptr(live_ptr.index as usize)
    }
    
    pub fn file_id_to_file_name(&self, file_id: LiveFileId) -> &str {
        &self.live_files[file_id.to_index()].file_name
    }
    
    pub fn ptr_to_doc_node(&self, live_ptr: LivePtr) -> (&LiveExpanded, &LiveNode) {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation{
            panic!("ptr_to_doc_node generation invalid for file {} gen:{} ptr:{}", doc.file_name, doc.generation, live_ptr.generation);
        }
        (&doc.expanded, &doc.expanded.resolve_ptr(live_ptr.index as usize))
    }
    
    pub fn ptr_to_doc(&self, live_ptr: LivePtr) -> &LiveExpanded {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation{
            panic!("ptr_to_doc generation invalid for file {} gen:{} ptr:{}", doc.file_name, doc.generation, live_ptr.generation);
        }
        &doc.expanded
    }
    
    pub fn file_id_to_file(&self, file_id: LiveFileId) -> &LiveFile {
        &self.live_files[file_id.to_index()]
    }
    
    pub fn ptr_to_nodes_index(&self, live_ptr: LivePtr) -> (&[LiveNode], usize) {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation{
            panic!("ptr_to_nodes_index generation invalid for file {} gen:{} ptr:{}", doc.file_name, doc.generation, live_ptr.generation);
        }
        (&doc.expanded.nodes, live_ptr.index as usize)
    }
    
    pub fn path_str_to_file_id(&self, path: &str) -> Option<LiveFileId> {
        for (index, file) in self.live_files.iter().enumerate() {
            if file.file_name == path {
                return Some(LiveFileId(index as u16))
            }
        }
        None
    }
    
    pub fn token_id_to_origin_doc(&self, token_id: LiveTokenId) -> &LiveOriginal {
        &self.live_files[token_id.file_id().to_index()].original
    }
    
    pub fn token_id_to_token(&self, token_id: LiveTokenId) -> &TokenWithSpan {
        &self.live_files[token_id.file_id().to_index()].original.tokens[token_id.token_index()]
    }
    
    pub fn token_id_to_expanded_doc(&self, token_id: LiveTokenId) -> &LiveExpanded {
        &self.live_files[token_id.file_id().to_index()].expanded
    }
    
    pub fn module_id_to_file_id(&self, module_id: LiveModuleId) -> Option<LiveFileId> {
        self.module_id_to_file_id.get(&module_id).cloned()
    }

    pub fn live_node_as_string(&self, node:&LiveNode) -> Option<String> {
        match &node.value{
            LiveValue::Str(v) => {
                Some(v.to_string())
            }
            LiveValue::FittedString(v) => {
                Some(v.as_str().to_string())
            }
            LiveValue::InlineString(v) => {
                Some(v.as_str().to_string())
            }
            LiveValue::DocumentString {string_start, string_count} => {
                let origin_doc = self.token_id_to_origin_doc(node.origin.token_id().unwrap());
                let mut out = String::new();
                origin_doc.get_string(*string_start, *string_count, &mut out);
                Some(out)
            }
            _=>None
        }
    }
    
    // this looks at the 'id' before the live token id
    pub fn get_node_prefix(&self, origin: LiveNodeOrigin) -> Option<LiveId> {
        if !origin.node_has_prefix() {
            return None
        }
        let first_def = origin.first_def().unwrap();
        let token_index = first_def.token_index();
        if token_index == 0 {
            return None;
        }
        let doc = &self.live_files[first_def.file_id().to_index()].original;
        let token = doc.tokens[token_index - 1];
        if let LiveToken::Ident(id) = token.token {
            return Some(id)
        }
        None
    }
    
    pub fn module_id_and_name_to_doc(&self, module_id: LiveModuleId, name: LiveId) -> Option<LiveDocNodes> {
        if let Some(file_id) = self.module_id_to_file_id.get(&module_id) {
            let doc = &self.live_files[file_id.to_index()].expanded;
            if name != LiveId::empty() {
                if doc.nodes.len() == 0 {
                    println!("module_path_id_to_doc zero nodelen {}", self.file_id_to_file_name(*file_id));
                    return None
                }
                if let Some(index) = doc.nodes.child_by_name(0, name) {
                    return Some(LiveDocNodes {nodes: &doc.nodes, file_id: *file_id, index});
                }
                else {
                    return None
                }
            }
            else {
                return Some(LiveDocNodes {nodes: &doc.nodes, file_id: *file_id, index: 0});
            }
        }
        None
    }
    
    pub fn find_scope_item_via_class_parent(&self, start_ptr: LivePtr, item: LiveId) -> Option<(&[LiveNode], usize)> {
        let (nodes, index) = self.ptr_to_nodes_index(start_ptr);
        if let LiveValue::Class {class_parent, ..} = &nodes[index].value {
            // ok its a class so now first scan up from here.
            
            if let Some(index) = nodes.scope_up_down_by_name(index, item) {
                // item can be a 'use' as well.
                // if its a use we need to resolve it, otherwise w found it
                if let LiveValue::Use(module_id) = &nodes[index].value {
                    if let Some(ldn) = self.module_id_and_name_to_doc(*module_id, nodes[index].id) {
                        return Some((ldn.nodes, ldn.index))
                    }
                }
                else {
                    return Some((nodes, index))
                }
            }
            else {
                if let Some(class_parent) = class_parent {
                    if class_parent.file_id != start_ptr.file_id {
                        return self.find_scope_item_via_class_parent(*class_parent, item)
                    }
                }
                
            }
        }
        else {
            println!("WRONG TYPE  {:?}", nodes[index].value);
        }
        None
    }
    
    pub fn find_scope_target_via_start(&self, item: LiveId, index: usize, nodes: &[LiveNode]) -> Option<LiveScopeTarget> {
        if let Some(index) = nodes.scope_up_down_by_name(index, item) {
            if let LiveValue::Use(module_id) = &nodes[index].value {
                // ok lets find it in that other doc
                if let Some(file_id) = self.module_id_to_file_id(*module_id) {
                    let file = self.file_id_to_file(file_id);
                    if let Some(index) = file.expanded.nodes.child_by_name(0, item) {
                        return Some(LiveScopeTarget::LivePtr(
                            LivePtr {file_id: file_id, index: index as u32, generation:file.generation}
                        ))
                    }
                }
            }
            else {
                return Some(LiveScopeTarget::LocalPtr(index))
            }
        }
        // ok now look at the glob use * things
        let mut node_iter = Some(1);
        while let Some(index) = node_iter {
            if let LiveValue::Use(module_id) = &nodes[index].value {
                if nodes[index].id == LiveId::empty() { // glob
                    if let Some(file_id) = self.module_id_to_file_id(*module_id) {
                        let file = self.file_id_to_file(file_id);
                        if let Some(index) = file.expanded.nodes.child_by_name(0, item) {
                            return Some(LiveScopeTarget::LivePtr(
                                LivePtr {file_id: file_id, index: index as u32, generation:file.generation}
                            ))
                        }
                    }
                }
            }
            node_iter = nodes.next_child(index);
        }
        None
    }
    
    
    pub fn find_scope_ptr_via_expand_index(&self, file_id: LiveFileId, index: usize, item: LiveId) -> Option<LivePtr> {
        // ok lets start
        // let token_id = origin.token_id().unwrap();
        //let index = origin.node_index().unwrap();
        //let file_id = token_id.file_id();
        let file = self.file_id_to_file(file_id);
        match self.find_scope_target_via_start(item, index, &file.expanded.nodes) {
            Some(LiveScopeTarget::LocalPtr(index)) => Some(LivePtr {file_id: file_id, index: index as u32,generation:file.generation}),
            Some(LiveScopeTarget::LivePtr(ptr)) => Some(ptr),
            None => None
        }
    }
    
    pub fn live_error_to_live_file_error(&self, live_error: LiveError) -> LiveFileError {
        match live_error.span {
            LiveErrorSpan::Text(text_span) => {
                let live_file = &self.live_files[text_span.file_id.to_index()];
                LiveFileError {
                    origin: live_error.origin,
                    file: live_file.file_name.clone(),
                    span: text_span,
                    message: live_error.message
                }
            }
            LiveErrorSpan::Token(token_span) => {
                let live_file = &self.live_files[token_span.token_id.file_id().to_index()];
                let token = live_file.original.tokens[token_span.token_id.token_index()];
                LiveFileError {
                    origin: live_error.origin,
                    file: live_file.file_name.clone(),
                    span: token.span,
                    message: live_error.message
                }
            }
        }
    }
    
    pub fn token_id_to_span(&self, token_id: LiveTokenId) -> TextSpan {
        self.live_files[token_id.file_id().to_index()].original.token_id_to_span(token_id)
    }
    
    pub fn tokenize_from_str(source: &str, start_pos: TextPos, file_id: LiveFileId) -> Result<(Vec<TokenWithSpan>, Vec<char>), LiveError> {
        let mut line_chars = Vec::new();
        let mut state = State::default();
        let mut scratch = String::new();
        let mut strings = Vec::new();
        let mut tokens = Vec::new();
        let mut pos = start_pos;
        for line_str in source.lines() {
            line_chars.clear();
            line_chars.extend(line_str.chars());
            let mut cursor = Cursor::new(&line_chars, &mut scratch);
            loop {
                let (next_state, full_token) = state.next(&mut cursor);
                if let Some(full_token) = full_token {
                    let span = TextSpan {
                        file_id,
                        start: pos,
                        end: TextPos {column: pos.column + full_token.len as u32, line: pos.line}
                    };
                    match full_token.token {
                        FullToken::Unknown | FullToken::OtherNumber | FullToken::Lifetime => {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: span.into(),
                                message: format!("Error tokenizing")
                            })
                        },
                        FullToken::String => {
                            let len = full_token.len - 2;
                            tokens.push(TokenWithSpan {span: span, token: LiveToken::String {
                                index: strings.len() as u32,
                                len: len as u32
                            }});
                            let col = pos.column as usize + 1;
                            strings.extend(&line_chars[col..col + len]);
                        },
                        _ => match LiveToken::from_full_token(full_token.token) {
                            Some(live_token) => {
                                // lets build up the span info
                                tokens.push(TokenWithSpan {span: span, token: live_token})
                            },
                            _ => ()
                        },
                    }
                    pos.column += full_token.len as u32;
                }
                else {
                    break;
                }
                state = next_state;
            }
            pos.line += 1;
            pos.column = 0;
        }
        tokens.push(TokenWithSpan {span: TextSpan::default(), token: LiveToken::Eof});
        Ok((tokens, strings))
    }
    
    // called by the live editor to update a live file
    pub fn live_edit_file<'a, CB>(
        &mut self,
        file_name: &str,
        range: TokenRange,
        mut get_line: CB
    ) -> Result<Option<LiveEditEvent>, LiveError>
    where CB: FnMut(usize) -> (&'a [char], &'a [TokenWithLen])
    {
        let file_id = *self.file_ids.get(file_name).unwrap();
        let mut live_index = 0;
        let live_file = &mut self.live_files[file_id.to_index()];
        let original = &mut live_file.original;
        
        let mut live_tokens = &mut original.tokens;
        let mut new_tokens = Vec::new();
        let old_strings = &original.strings;
        let mut new_strings: Vec<char> = Vec::new();
        
        let mut mutated_tokens = Vec::new();
        
        let mut parse_changed = false;
        
        for line in range.start.line..range.end.line {
            let (line_chars, full_tokens) = get_line(line);
            // OK SO now we diff as we go
            let mut column = 0usize;
            for (token_index, full_token) in full_tokens.iter().enumerate() {
                
                if range.is_in_range(TokenPos {line: line, index: token_index}) {
                    // ok so. now we filter the token
                    let span = TextSpan {
                        file_id,
                        start: TextPos {column: column as u32, line: line as u32},
                        end: TextPos {column: (column + full_token.len) as u32, line: line as u32}
                    };
                    
                    match full_token.token {
                        FullToken::Unknown | FullToken::OtherNumber | FullToken::Lifetime => {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: span.into(),
                                message: format!("Error tokenizing")
                            })
                        },
                        FullToken::String => {
                            let new_len = full_token.len - 2;
                            let new_col = column as usize + 1;
                            let new_chars = &line_chars[new_col..new_col + new_len];
                            let new_string = LiveToken::String {
                                index: new_strings.len() as u32,
                                len: new_len as u32
                            };
                            if live_index >= live_tokens.len() { // just append
                                if !parse_changed {
                                    new_tokens = live_tokens.clone();
                                    live_tokens = &mut new_tokens;
                                    parse_changed = true;
                                }
                                live_tokens.push(TokenWithSpan {span: span, token: new_string});
                            }
                            else if let LiveToken::String {index, len} = live_tokens[live_index].token {
                                let old_chars = &old_strings[index as usize ..(index + len) as usize];
                                // compare string or len/position
                                if new_chars != old_chars || new_strings.len() as u32 != index || new_len as u32 != len {
                                    mutated_tokens.push(LiveTokenId::new(file_id, live_index));
                                }
                            }
                            else { // cant replace a sttring type with something else without a reparse
                                if !parse_changed {
                                    new_tokens = live_tokens.clone();
                                    live_tokens = &mut new_tokens;
                                    parse_changed = true;
                                }
                                live_tokens[live_index] = TokenWithSpan {span: span, token: new_string};
                            }
                            new_strings.extend(new_chars);
                            live_index += 1;
                        },
                        _ => match LiveToken::from_full_token(full_token.token) {
                            Some(live_token) => {
                                if live_index >= live_tokens.len() { // just append
                                    if !parse_changed {
                                        new_tokens = live_tokens.clone();
                                        live_tokens = &mut new_tokens;
                                        parse_changed = true;
                                    }
                                    live_tokens.push(TokenWithSpan {span: span, token: live_token})
                                }
                                else {
                                    if live_tokens[live_index].is_parse_equal(live_token) { // token value changed
                                        if live_tokens[live_index].token != live_token {
                                            live_tokens[live_index].token = live_token;
                                            mutated_tokens.push(LiveTokenId::new(file_id, live_index));
                                        }
                                    }
                                    else { // token value changed in a way that changes parsing
                                        // lets special case the {{id}} situation
                                        if live_index > 2
                                            && live_tokens[live_index - 2].is_open_delim(Delim::Brace)
                                            && live_tokens[live_index - 1].is_open_delim(Delim::Brace)
                                            && live_tokens[live_index].is_ident()
                                            && live_token.is_ident() {
                                        }
                                        else {
                                            if !parse_changed {
                                                new_tokens = live_tokens.clone();
                                                live_tokens = &mut new_tokens;
                                                parse_changed = true;
                                            }
                                            live_tokens[live_index].token = live_token;
                                        }
                                    }
                                    // always update the spans
                                    live_tokens[live_index].span = span;
                                }
                                live_index += 1;
                            },
                            _ => ()
                        },
                    }
                }
                column += full_token.len;
            }
        }
        if live_index < live_tokens.len() - 1 { // the tokenlist shortened
            if !parse_changed{
                new_tokens = live_tokens.clone();
                live_tokens = &mut new_tokens;
                parse_changed = true;
            }
        }
        
        live_tokens.truncate(live_index);
        live_tokens.push(TokenWithSpan {token: LiveToken::Eof, span: TextSpan {file_id, start: TextPos::default(), end: TextPos::default()}});
        
        if parse_changed {
            // we have to be able to delay this to onkeyup 
            let mut parser = LiveParser::new(&new_tokens, &live_file.live_type_infos, file_id);
            match parser.parse_live_document() {
                Err(msg) => return Err(msg), //panic!("Parse error {}", msg.to_live_file_error(file, &source)),
                Ok(mut ld) => { // only swap it out when it parses
                    ld.strings = new_strings;
                    ld.tokens = new_tokens;
                    live_file.next_original = Some(ld);
                }
            };
            
            return Ok(Some(LiveEditEvent::ReparseDocument));
        }
        else if mutated_tokens.len()>0 { // its a hotpatch
            // means if we had a next_original its now cancelled
            live_file.next_original = None;
            original.strings = new_strings;
            
            let (apply, live_ptrs) = self.update_documents_from_mutated_tokens(&mutated_tokens);
            
            return Ok(Some(LiveEditEvent::Mutation {tokens: mutated_tokens, apply, live_ptrs}))
        }
        
        Ok(None)
    }
    
    pub fn process_next_originals_and_expand(&mut self)->Result<(), Vec<LiveError>>{
        for live_file in &mut self.live_files{
            if live_file.next_original.is_some(){
                live_file.original = live_file.next_original.take().unwrap();
                live_file.reexpand = true;
                live_file.generation.next_gen();
            }
        }
        
        let mut errors = Vec::new();
        self.expand_all_documents(&mut errors);
        
        if errors.len()>0 {
            return Err(errors);
        }
        return Ok(())
    }
    
    fn update_documents_from_mutated_tokens(
        &mut self,
        mutated_tokens: &[LiveTokenId]
    ) -> (Vec<LiveNode>, Vec<LivePtr>) {
        //let mutated_tokens = self.mutated_tokens.take().unwrap();
        let mut diff = Vec::new();
        let mut live_ptrs = Vec::new();
        diff.open();
        for token_id in mutated_tokens {
            let token_index = token_id.token_index();
            let file_id = token_id.file_id();
            // ok this becomes the patch-map for shader constants
            
            //let live_file = &self.live_files[file_id.to_index()];
            //let original = &live_file.original;
            let mut live_tokens = Vec::new();
            std::mem::swap(&mut live_tokens, &mut self.live_files[file_id.to_index()].original.tokens);
            
            let is_prop_assign = token_index > 2
                && live_tokens[token_index - 2].is_ident()
                && live_tokens[token_index - 1].is_punct_id(id!(:));
            
            if is_prop_assign || live_tokens[token_index].is_value_type() {
                let token_id = LiveTokenId::new(file_id, token_index - 2);
                
                // ok lets scan for this one.
                let mut file_dep_iter = FileDepIter::new(file_id);
                let mut path = Vec::new();
                while let Some(file_id) = file_dep_iter.pop_todo() {
                    let is_main = self.main_module == Some(file_id);
                    
                    let mut expanded_nodes = Vec::new();
                    std::mem::swap(&mut expanded_nodes, &mut self.live_files[file_id.to_index()].expanded.nodes);
                    
                    let mut reader = LiveNodeMutReader::new(0, &mut expanded_nodes);
                    path.clear();
                    reader.walk();
                    while !reader.is_eot() {
                        if reader.is_open() {
                            path.push(reader.id)
                        }
                        else if reader.is_close() {
                            path.pop();
                        }
                        // ok this is a direct patch
                        else if is_prop_assign && reader.origin.token_id() == Some(token_id) {
                            let live_ptr = LivePtr {file_id, index: reader.index() as u32, generation:self.live_files[file_id.to_index()].generation};
                            if !reader.update_from_live_token(&live_tokens[token_index].token) {
                                println!("update_from_live_token returns false investigate! {:?}", reader.node());
                            }
                            live_ptrs.push(live_ptr);
                            if is_main {
                                // ok so. lets write by path here
                                path.push(reader.id);
                                diff.replace_or_insert_last_node_by_path(0, &path, reader.node_slice());
                                path.pop();
                            }
                        }
                        else if reader.is_token_id_inside_dsl(token_id) {
                            if is_main {
                                // ok so. lets write by path here
                                path.push(reader.id);
                                diff.replace_or_insert_last_node_by_path(0, &path, reader.node_slice());
                                path.pop();
                            }
                        }
                        reader.walk();
                    }
                    std::mem::swap(&mut expanded_nodes, &mut self.live_files[file_id.to_index()].expanded.nodes);
                    file_dep_iter.scan_next(&self.live_files);
                }
            }
            std::mem::swap(&mut live_tokens, &mut self.live_files[file_id.to_index()].original.tokens);
        }
        diff.close();
        (diff, live_ptrs)
    }
    
    pub fn register_live_file(
        &mut self,
        file_name: &str,
        own_module_id: LiveModuleId,
        source: String,
        live_type_infos: Vec<LiveTypeInfo>,
        start_pos: TextPos,
    ) -> Result<LiveFileId, LiveFileError> {
        
        // lets register our live_type_infos
        if self.file_ids.get(file_name).is_some() {
            panic!("cant register same file twice");
        }
        let file_id = LiveFileId::new(self.live_files.len());
        
        let (tokens, strings) = match Self::tokenize_from_str(&source, start_pos, file_id) {
            Err(msg) => return Err(msg.into_live_file_error(file_name)), //panic!("Lex error {}", msg),
            Ok(lex_result) => lex_result
        };
        
        let mut parser = LiveParser::new(&tokens, &live_type_infos, file_id);
        
        let mut original = match parser.parse_live_document() {
            Err(msg) => return Err(msg.into_live_file_error(file_name)), //panic!("Parse error {}", msg.to_live_file_error(file, &source)),
            Ok(ld) => ld
        };
        
        original.strings = strings;
        original.tokens = tokens;
        
        // update our live type info
        for live_type_info in &live_type_infos {
            if let Some(info) = self.live_type_infos.get(&live_type_info.live_type) {
                if info.module_id != live_type_info.module_id
                    || info.live_type != live_type_info.live_type {
                    panic!()
                }
            };
            self.live_type_infos.insert(live_type_info.live_type, live_type_info.clone());
        }
        
        let mut deps = BTreeSet::new();
        
        for node in &mut original.nodes {
            match &mut node.value {
                LiveValue::Use(module_id) => {
                    if module_id.0 == id!(crate) { // patch up crate refs
                        module_id.0 = own_module_id.0
                    };
                    deps.insert(*module_id);
                }, // import
                LiveValue::Class {live_type, ..} => { // hold up. this is always own_module_path
                    let infos = self.live_type_infos.get(&live_type).unwrap();
                    for sub_type in infos.fields.clone() {
                        let sub_module_id = sub_type.live_type_info.module_id;
                        if sub_module_id != own_module_id {
                            deps.insert(sub_module_id);
                        }
                    }
                }
                _ => {
                }
            }
        }
        
        let live_file = LiveFile {
            reexpand: true,
            module_id: own_module_id,
            file_name: file_name.to_string(),
            start_pos,
            deps,
            source,
            generation:LiveFileGeneration::default(),
            live_type_infos,
            original,
            next_original: None,
            expanded: LiveExpanded::new()
        };
        self.module_id_to_file_id.insert(own_module_id, file_id);
        
        self.file_ids.insert(file_name.to_string(), file_id);
        self.live_files.push(live_file);
        
        return Ok(file_id)
    }
    
    pub fn expand_all_documents(&mut self, errors: &mut Vec<LiveError>) {
        
        // alright so. we iterate
        let mut dep_order = Vec::new();
        
        fn recur_insert_dep(parent_index: usize, dep_order: &mut Vec<LiveModuleId>, current: LiveModuleId, files: &Vec<LiveFile>) {
            let file = if let Some(file) = files.iter().find( | v | v.module_id == current) {
                file
            }
            else {
                return
            };
            let final_index = if let Some(index) = dep_order.iter().position( | v | *v == current) {
                if index > parent_index { // insert before
                    dep_order.remove(index);
                    dep_order.insert(parent_index, current);
                    parent_index
                }
                else {
                    index
                }
            }
            else {
                dep_order.insert(parent_index, current);
                parent_index
            };
            
            for dep in &file.deps {
                recur_insert_dep(final_index, dep_order, *dep, files);
            }
        }
        
        for file in &self.live_files {
            recur_insert_dep(dep_order.len(), &mut dep_order, file.module_id, &self.live_files);
        }
        
        // now lets do the recursive recompile parsing.
        fn recur_check_reexpand(current: LiveModuleId, files: &Vec<LiveFile>) -> bool {
            let file = if let Some(file) = files.iter().find( | v | v.module_id == current) {
                file
            }
            else {
                return false
            };
            
            if file.reexpand {
                return true;
            }
            
            for dep in &file.deps {
                if recur_check_reexpand(*dep, files) {
                    return true
                }
            }
            false
        }
        
        for i in 0..self.live_files.len() {
            if recur_check_reexpand(self.live_files[i].module_id, &self.live_files) {
                self.live_files[i].reexpand = true;
            }
        }
        
        for module_id in dep_order {
            let file_id = if let Some(file_id) = self.module_id_to_file_id.get(&module_id) {
                file_id
            }
            else {
                continue
            };
            
            if !self.live_files[file_id.to_index()].reexpand {
                continue;
            }
            
            let mut out_doc = LiveExpanded::new();
            std::mem::swap(&mut out_doc, &mut self.live_files[file_id.to_index()].expanded);
            
            out_doc.nodes.clear();
            
            let in_doc = &self.live_files[file_id.to_index()].original;
            
            let mut live_document_expander = LiveExpander {
                live_registry: self,
                in_crate: module_id.0,
                in_file_id: *file_id,
                errors
            };
            live_document_expander.expand(in_doc, &mut out_doc, self.live_files[file_id.to_index()].generation);
            
            self.live_files[file_id.to_index()].reexpand = false;
            
            std::mem::swap(&mut out_doc, &mut self.live_files[file_id.to_index()].expanded);
        }
    }
}

struct FileDepIter {
    files_todo: Vec<LiveFileId>,
    files_done: Vec<LiveFileId>
}

impl FileDepIter {
    pub fn new(start: LiveFileId) -> Self {
        Self {
            files_todo: vec![start],
            files_done: Vec::new()
        }
    }
    
    pub fn pop_todo(&mut self) -> Option<LiveFileId> {
        if let Some(file_id) = self.files_todo.pop() {
            self.files_done.push(file_id);
            Some(file_id)
        }
        else {
            None
        }
    }
    
    pub fn scan_next(&mut self, live_files: &[LiveFile]) {
        let last_file_id = self.files_done.last().unwrap();
        let module_id = live_files[last_file_id.to_index()].module_id;
        
        for (file_index, live_file) in live_files.iter().enumerate() {
            if live_file.deps.contains(&module_id) {
                let dep_id = LiveFileId::new(file_index);
                if self.files_done.iter().position( | v | *v == dep_id).is_none() {
                    self.files_todo.push(dep_id);
                }
            }
        }
    }
}
