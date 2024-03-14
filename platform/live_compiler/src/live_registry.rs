//use crate::id::Id;
use {
    std::collections::{HashMap, BTreeSet},
    crate::{
        makepad_live_id::*,
       // makepad_error_log::*,
        makepad_live_tokenizer::{TokenWithLen, Delim, FullToken, State, Cursor, live_error_origin, LiveErrorOrigin},
        live_error::{LiveError, LiveErrorSpan, LiveFileError},
        live_parser::LiveParser,
        live_document::{LiveOriginal, LiveExpanded},
        live_node::{LiveNodeOrigin, LiveNode, LiveValue, LiveType, LiveTypeInfo, LiveIdAsProp},
        /*live_node_reader::{LiveNodeMutReader},*/
        live_node_vec::{LiveNodeSliceApi, /*LiveNodeVecApi*/},
        live_ptr::{LiveFileId, LivePtr, LiveModuleId, LiveFileGeneration},
        live_token::{LiveToken, LiveTokenId, TokenWithSpan},
        span::{TextSpan, TextPos},
        live_expander::{LiveExpander},
        live_component::{LiveComponentRegistries}
    }
};

#[derive(Default)]
pub struct LiveFile {
    pub (crate) reexpand: bool,
    
    pub module_id: LiveModuleId,
    pub (crate) start_pos: TextPos,
    pub file_name: String,
    pub cargo_manifest_path: String,
    pub (crate) source: String,
    pub (crate) deps: BTreeSet<LiveModuleId>,
    
    pub generation: LiveFileGeneration,
    pub original: LiveOriginal,
    pub next_original: Option<LiveOriginal>,
    pub expanded: LiveExpanded,
    
    pub live_type_infos: Vec<LiveTypeInfo>,
}

pub struct LiveRegistry {
    pub (crate) file_ids: HashMap<String, LiveFileId>,
    pub module_id_to_file_id: HashMap<LiveModuleId, LiveFileId>,
    pub live_files: Vec<LiveFile>,
    pub live_type_infos: HashMap<LiveType, LiveTypeInfo>,
    //pub ignore_no_dsl: HashSet<LiveId>,
    pub main_module: Option<(LiveModuleId, LiveId)>,
    pub components: LiveComponentRegistries,
    pub package_root: Option<String>
}

impl Default for LiveRegistry {
    fn default() -> Self {
        Self {
            main_module: None,
            file_ids: HashMap::new(),
            module_id_to_file_id: HashMap::new(),
            live_files: Vec::new(),
            live_type_infos: HashMap::new(),
            components: LiveComponentRegistries::default(),
            package_root: None
        }
    }
}
/*
pub struct LiveDocNodes<'a> {
    pub nodes: &'a [LiveNode],
    pub file_id: LiveFileId,
    pub index: usize
}*/

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LiveScopeTarget {
    LocalPtr(usize),
    LivePtr(LivePtr)
}


#[derive(Clone, Debug, PartialEq)]
pub struct LiveFileChange {
    pub file_name: String,
    pub content: String
}

impl LiveRegistry {
    
    pub fn generation_valid(&self, live_ptr: LivePtr) -> bool {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        doc.generation == live_ptr.generation
    }
    
    pub fn ptr_to_node(&self, live_ptr: LivePtr) -> &LiveNode {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation {
            panic!("ptr_to_node generation invalid for file {} gen:{} ptr:{}", doc.file_name, doc.generation, live_ptr.generation);
        }
        doc.expanded.resolve_ptr(live_ptr.index as usize)
    }
    
    pub fn file_name_to_file_id(&self, file_name: &str) -> Option<LiveFileId> {
        for (index, file) in self.live_files.iter().enumerate() {
            if file.file_name == file_name {
                return Some(LiveFileId::new(index))
            }
        }
        None
    }
    
    pub fn file_id_to_file_name(&self, file_id: LiveFileId) -> &str {
        &self.live_files[file_id.to_index()].file_name
    }
    
    pub fn file_id_to_cargo_manifest_path(&self, file_id: LiveFileId) -> String {
        let file = &self.live_files[file_id.to_index()];
        let manifest_path = &file.cargo_manifest_path;
        if let Some(package_root) = &self.package_root {
            if file.module_id.0.0 == 0 {
                return package_root.to_string();
            }
            return format!("{}/{}", package_root, file.module_id.0);
        }
        manifest_path.to_string()
    }
    
    pub fn crate_name_to_cargo_manifest_path(&self, crate_name: &str) -> Option<String> {
        let crate_name = crate_name.replace('-', "_");
        let base_crate = LiveId::from_str_with_lut(&crate_name).unwrap();
        for file in &self.live_files {
            if file.module_id.0 == base_crate {
                if let Some(package_root) = &self.package_root {
                    return Some(format!("{}/{}", package_root, crate_name));
                }
                return Some(file.cargo_manifest_path.to_string())
            }
        }
        None
    }
    
    pub fn ptr_to_doc_node(&self, live_ptr: LivePtr) -> (&LiveExpanded, &LiveNode) {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation {
            panic!("ptr_to_doc_node generation invalid for file {} gen:{} ptr:{}", doc.file_name, doc.generation, live_ptr.generation);
        }
        (&doc.expanded, doc.expanded.resolve_ptr(live_ptr.index as usize))
    }
    
    pub fn ptr_to_doc(&self, live_ptr: LivePtr) -> &LiveExpanded {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation {
            panic!("ptr_to_doc generation invalid for file {} gen:{} ptr:{}", doc.file_name, doc.generation, live_ptr.generation);
        }
        &doc.expanded
    }
    
    pub fn file_id_to_file(&self, file_id: LiveFileId) -> &LiveFile {
        &self.live_files[file_id.to_index()]
    }
    
    pub fn file_id_to_file_mut(&mut self, file_id: LiveFileId) -> &mut LiveFile {
        &mut self.live_files[file_id.to_index()]
    }
    
    pub fn file_id_index_to_live_ptr(&self, file_id: LiveFileId, index: usize) -> LivePtr {
        LivePtr {
            file_id,
            index: index as u32,
            generation: self.live_files[file_id.to_index()].generation
        }
    }
    
    pub fn ptr_to_nodes_index(&self, live_ptr: LivePtr) -> (&[LiveNode], usize) {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation {
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
        &self.live_files[token_id.file_id().unwrap().to_index()].original
    }
    
    pub fn token_id_to_token(&self, token_id: LiveTokenId) -> &TokenWithSpan {
        &self.live_files[token_id.file_id().unwrap().to_index()].original.tokens[token_id.token_index()]
    }
    
    pub fn token_id_to_expanded_doc(&self, token_id: LiveTokenId) -> &LiveExpanded {
        &self.live_files[token_id.file_id().unwrap().to_index()].expanded
    }
    
    pub fn module_id_to_file_id(&self, module_id: LiveModuleId) -> Option<LiveFileId> {
        self.module_id_to_file_id.get(&module_id).cloned()
    }
    
    pub fn file_id_to_module_id(&self, file_id: LiveFileId) -> Option<LiveModuleId> {
        if let Some((k,_v)) = self.module_id_to_file_id.iter().find(|(_k,v)| **v == file_id){
            return Some(*k)
        }
        None
    }
    
    pub fn live_node_as_string(&self, node: &LiveNode) -> Option<String> {
        match &node.value {
            LiveValue::Str(v) => {
                Some(v.to_string())
            }
            LiveValue::String(v) => {
                Some(v.as_str().to_string())
            }
            LiveValue::InlineString(v) => {
                Some(v.as_str().to_string())
            }
            LiveValue::Dependency (v) => {
                Some(v.as_str().to_string())
            }
            _ => None
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
        let doc = &self.live_files[first_def.file_id().unwrap().to_index()].original;
        let token = &doc.tokens[token_index - 1];
        if let LiveToken::Ident(id) = token.token {
            return Some(id)
        }
        None
    }
    
    pub fn module_id_to_expanded_nodes(&self, module_id: LiveModuleId) -> Option<&[LiveNode]> {
        if let Some(file_id) = self.module_id_to_file_id.get(&module_id) {
            let doc = &self.live_files[file_id.to_index()].expanded;
            return Some(&doc.nodes)
        }
        None
    }
    
    pub fn module_id_and_name_to_ptr(&self, module_id: LiveModuleId, name: LiveId) -> Option<LivePtr> {
        if let Some(file_id) = self.module_id_to_file_id.get(&module_id) {
            let live = &self.live_files[file_id.to_index()];
            let doc = &live.expanded;
            if name != LiveId::empty() {
                if doc.nodes.is_empty() {
                    eprintln!("module_path_id_to_doc zero nodelen {}", self.file_id_to_file_name(*file_id));
                    return None
                }
                if let Some(index) = doc.nodes.child_by_name(0, name.as_instance()) {
                    return Some(LivePtr {file_id: *file_id, index: index as u32, generation: live.generation});
                }
                else {
                    return None
                }
            }
            else {
                return Some(LivePtr {file_id: *file_id, index: 0, generation: live.generation});
            }
        }
        None
    }
    /*
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
    }*/
    
    pub fn find_module_id_name(&self, item: LiveId, module_id: LiveModuleId) -> Option<LiveScopeTarget> {
        // ok lets find it in that other doc
        if let Some(file_id) = self.module_id_to_file_id(module_id) {
            let file = self.file_id_to_file(file_id);
            if file.expanded.nodes.is_empty() {
                println!("Looking for {} but its not expanded yet, dependency order bug", file.file_name);
                return None
            }
            if let Some(index) = file.expanded.nodes.child_by_name(0, item.as_instance()) {
                return Some(LiveScopeTarget::LivePtr(
                    LivePtr {file_id, index: index as u32, generation: file.generation}
                ))
            }
        }
        None
    }
    
    pub fn find_scope_target(&self, item: LiveId, nodes: &[LiveNode]) -> Option<LiveScopeTarget> {
        if let LiveValue::Root {id_resolve} = &nodes[0].value {
            id_resolve.get(&item).cloned()
        }
        else {
            println!("Can't find scope target on rootnode without id_resolve");
            None
        }
    }
    
    
    pub fn find_scope_target_one_level_or_global(&self, item: LiveId, index: usize, nodes: &[LiveNode]) -> Option<LiveScopeTarget> {
        if let Some(index) = nodes.scope_up_down_by_name(index, item.as_instance(), 1) {
            Some(LiveScopeTarget::LocalPtr(index))
        } else {
            self.find_scope_target(item, nodes)
        }
    }
    
    pub fn find_scope_ptr_via_expand_index(&self, file_id: LiveFileId, index: usize, item: LiveId) -> Option<LivePtr> {
        // ok lets start
        // let token_id = origin.token_id().unwrap();
        //let index = origin.node_index().unwrap();
        //let file_id = token_id.file_id();
        let file = self.file_id_to_file(file_id);
        match self.find_scope_target_one_level_or_global(item, index, &file.expanded.nodes) {
            Some(LiveScopeTarget::LocalPtr(index)) => Some(LivePtr {file_id, index: index as u32, generation: file.generation}),
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
                let (file_name, span) = if let Some(file_id) = token_span.token_id.file_id() {
                    let live_file = &self.live_files[file_id.to_index()];
                    (live_file.file_name.as_str(), live_file.original.tokens[token_span.token_id.token_index()].span)
                }
                else {
                    ("<file id is not defined>", TextSpan::default())
                };
                LiveFileError {
                    origin: live_error.origin,
                    file: file_name.to_string(),
                    span,
                    message: live_error.message
                }
            }
        }
    }
    
    pub fn token_id_to_span(&self, token_id: LiveTokenId) -> TextSpan {
        self.live_files[token_id.file_id().unwrap().to_index()].original.token_id_to_span(token_id)
    }
    
    pub fn tokenize_from_str(source: &str, start_pos: TextPos, file_id: LiveFileId) -> Result<Vec<TokenWithSpan>, LiveError> {
        let mut chars = Vec::new();
        chars.extend(source.chars());
        let mut state = State::default();
        let mut scratch = String::new();
        let mut tokens = Vec::new();
        let mut line_start = start_pos.line;
        let mut cursor = Cursor::new(&chars, &mut scratch);
        let mut last_index = 0usize;
        let mut last_new_line = 0usize;
        loop {
            let (next_state, full_token) = state.next(&mut cursor);
            if let Some(full_token) = full_token {
                // lets count the newlines 
                let mut line_end = line_start;
                let mut next_new_line = last_new_line;
                for i in 0..full_token.len{
                    if chars[last_index + i] == '\n'{
                        line_end += 1;
                        next_new_line = last_index + 2;
                    }
                }
                let span = TextSpan {
                    file_id,
                    start: TextPos {column:  (last_index - last_new_line)  as u32, line: line_start},
                    end: TextPos {column:  (last_index - last_new_line) as u32 + full_token.len  as u32, line: line_end}
                };
                match full_token.token {
                    FullToken::Unknown | FullToken::OtherNumber | FullToken::Lifetime => {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: span.into(),
                            message: "Error tokenizing".to_string()
                        })
                    },
                    _ => if let Some(live_token) = LiveToken::from_full_token(&full_token.token) {
                        // lets build up the span info
                        tokens.push(TokenWithSpan {span, token: live_token})
                    },
                }
                line_start = line_end;
                last_new_line = next_new_line;
            }
            else {
                break;
            }
            state = next_state;
            last_index = cursor.index()
        }
        tokens.push(TokenWithSpan {span: TextSpan::default(), token: LiveToken::Eof});
        Ok(tokens)
    }
    
    pub fn tokenize_from_str_live_design(source: &str, start_pos: TextPos, file_id: LiveFileId, mut negative:Option<&mut Vec<TokenWithLen>>) -> Result<Vec<TokenWithSpan>, LiveError> {
        let mut chars = Vec::new();
        chars.extend(source.chars());
        let mut state = State::default();
        let mut scratch = String::new();
        let mut tokens = Vec::new();
        let mut line_start = start_pos.line;
        #[derive(Debug)]
        enum Parse{
            Before,
            After,
            Bang,
            Brace,
            Body(usize),
        } 
        let mut parse = Parse::Before;
        let mut cursor = Cursor::new(&chars, &mut scratch);
        let mut last_index = 0usize;
        let mut last_new_line = 0usize;
        loop {
            let (next_state, full_token) = state.next(&mut cursor);
            if let Some(full_token) = full_token {
                let mut line_end = line_start;
                let mut next_new_line = last_new_line;
                for i in 0..full_token.len{
                    if chars[last_index + i] == '\n'{
                        line_end += 1;
                        next_new_line = last_index + 1;
                    }
                }
                //log!("PARSE STATE {:?} {:?}", parse, full_token);
                match parse{
                    Parse::Before=>{
                        if let FullToken::Ident(live_id!(live_design)) = &full_token.token{
                            parse = Parse::Bang;
                        }
                        else if let Some(negative) = &mut negative{
                            negative.push(full_token);
                        }
                    }
                    Parse::Bang=> if let FullToken::Punct(live_id!(!)) = &full_token.token{
                        parse = Parse::Brace;
                    }
                    else if let FullToken::Whitespace = &full_token.token{
                    }
                    else{
                        parse = Parse::Before;
                    }
                    Parse::Brace=> if let FullToken::Open(Delim::Brace) = &full_token.token{
                        parse = Parse::Body(0);
                    }
                    else if let FullToken::Whitespace = &full_token.token{
                    }
                    else{
                        parse = Parse::Before;
                    }
                    Parse::Body(depth)=>{
                        if let FullToken::Open(Delim::Brace) = &full_token.token{
                                parse = Parse::Body(depth + 1)
                        }
                        if let FullToken::Close(Delim::Brace) = &full_token.token{
                            if depth == 0{
                                last_index = cursor.index();
                                parse = Parse::After;
                                continue;
                            }
                            parse = Parse::Body(depth - 1);
                        }
                        let span = TextSpan {
                            file_id,
                            start: TextPos {column: (last_index - last_new_line)  as u32, line: line_start},
                            end: TextPos {column: (last_index - last_new_line) as u32 + full_token.len  as u32, line: line_end}
                        };
                        if let Some(live_token) = LiveToken::from_full_token(&full_token.token) {
                            tokens.push(TokenWithSpan {span, token: live_token})
                        }
                    }
                    Parse::After=>{
                        if let Some(negative) = &mut negative{
                            negative.push(full_token);
                        }
                        else{
                            break;
                        }
                    }
                }
                last_new_line = next_new_line;
                line_start = line_end;
            }
            else {
                break;
            }
            state = next_state;
            last_index = cursor.index();
        }
        tokens.push(TokenWithSpan {span: TextSpan::default(), token: LiveToken::Eof});
        Ok(tokens)
    }
    
    pub fn process_file_changes(&mut self, changes: Vec<LiveFileChange>, errors:&mut Vec<LiveError >){
        let mut any_changes = false;
        for change in changes {
            if let Some(file_id) = self.file_name_to_file_id(&change.file_name){
                let module_id = self.file_id_to_module_id(file_id).unwrap();
                let live_file = self.file_id_to_file_mut(file_id);
                match Self::tokenize_from_str_live_design(&change.content, TextPos::default(), file_id, None) {
                    Err(msg) => errors.push(msg), //panic!("Lex error {}", msg),
                    Ok(new_tokens) => {
                        let mut parser = LiveParser::new(&new_tokens, &live_file.live_type_infos, file_id);
                        match parser.parse_live_document() {
                            Err(msg) => {
                                errors.push(msg);
                            },
                            Ok(mut ld) => { // only swap it out when it parses
                                for node in &mut ld.nodes {
                                    match &mut node.value {
                                        LiveValue::Import(live_import) => {
                                            if live_import.module_id.0 == live_id!(crate) { // patch up crate refs
                                               live_import.module_id.0 = module_id.0
                                            };
                                        }
                                        _=>()
                                    }
                                }
                                any_changes = true;
                                ld.tokens = new_tokens;
                                live_file.original = ld;
                                live_file.reexpand = true;
                                live_file.generation.next_gen();
                            }
                        };
                    }
                }
            }
        }
        if any_changes{
            // try to re-expand
            self.expand_all_documents(errors);
        }
    }

    pub fn register_live_file(
        &mut self,
        file_name: &str,
        cargo_manifest_path: &str,
        own_module_id: LiveModuleId,
        source: String,
        live_type_infos: Vec<LiveTypeInfo>,
        start_pos: TextPos,
    ) -> Result<LiveFileId, LiveFileError> {
        
        // lets register our live_type_infos
        if self.file_ids.get(file_name).is_some() {
            panic!("cant register same file twice {}", file_name);
        }
        let file_id = LiveFileId::new(self.live_files.len());
        
        let tokens = match Self::tokenize_from_str(&source, start_pos, file_id) {
            Err(msg) => return Err(msg.into_live_file_error(file_name)), //panic!("Lex error {}", msg),
            Ok(lex_result) => lex_result
        };
        
        let mut parser = LiveParser::new(&tokens, &live_type_infos, file_id);
        
        let mut original = match parser.parse_live_document() {
            Err(msg) => return Err(msg.into_live_file_error(file_name)), //panic!("Parse error {}", msg.to_live_file_error(file, &source)),
            Ok(ld) => ld
        };
        
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
                LiveValue::Import(live_import) => {
                    if live_import.module_id.0 == live_id!(crate) { // patch up crate refs
                       live_import.module_id.0 = own_module_id.0
                    };
                    deps.insert(live_import.module_id);
                }, // import
                /*LiveValue::Registry(component_id) => {
                    let reg = self.components.0.borrow();
                    if let Some(entry) = reg.values().find(|entry| entry.component_type() == *component_id){
                        entry.get_module_set(&mut deps);
                    }
                }, */
                LiveValue::Deref {live_type, ..} => { // hold up. this is always own_module_path
                    let infos = self.live_type_infos.get(live_type).unwrap();
                    for sub_type in infos.fields.clone() {
                        let sub_module_id = sub_type.live_type_info.module_id;
                        if sub_module_id != own_module_id {
                            deps.insert(sub_module_id);
                        }
                    }
                }
                LiveValue::Class {live_type, ..} => { // hold up. this is always own_module_path
                    let infos = self.live_type_infos.get(live_type).unwrap();
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
            cargo_manifest_path: cargo_manifest_path.to_string(),
            reexpand: true,
            module_id: own_module_id,
            file_name: file_name.to_string(),
            start_pos,
            deps,
            source,
            generation: LiveFileGeneration::default(),
            live_type_infos,
            original,
            next_original: None,
            expanded: LiveExpanded::new()
        };
        self.module_id_to_file_id.insert(own_module_id, file_id);
        
        self.file_ids.insert(file_name.to_string(), file_id);
        self.live_files.push(live_file);
        
        Ok(file_id)
    }
    
    pub fn expand_all_documents(&mut self, errors: &mut Vec<LiveError>) {
        // lets build up all dependencies here
        
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
                if !self.files_done.iter().any( | v | *v == dep_id) {
                    self.files_todo.push(dep_id);
                }
            }
        }
    }
}