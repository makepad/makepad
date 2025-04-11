//use crate::id::Id;
use {
    std::collections::{BTreeMap, HashMap},
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
    
    pub generation: LiveFileGeneration,
    pub original: LiveOriginal,
    pub next_original: Option<LiveOriginal>,
    pub expanded: LiveExpanded,
    
    pub live_type_infos: Vec<LiveTypeInfo>,
}

#[derive(Default)]
pub struct LiveLinkTarget{
    targets: Vec<LiveFileId>,
    combined_exports: Option<HashMap<LiveId, LiveFileId>>
}

pub struct LiveRegistry {
    pub (crate) file_ids: BTreeMap<String, LiveFileId>,
    pub (crate) link_targets: BTreeMap<LiveId, LiveLinkTarget>,
    pub (crate) link_connections: BTreeMap<LiveId, LiveId>,
    
    pub module_id_to_file_id: BTreeMap<LiveModuleId, LiveFileId>,
    pub live_files: Vec<LiveFile>,
    pub live_type_infos: BTreeMap<LiveType, LiveTypeInfo>,
    //pub ignore_no_dsl: HashSet<LiveId>,
    pub main_module: Option<LiveTypeInfo>,
    pub components: LiveComponentRegistries,
    pub package_root: Option<String>
}

impl Default for LiveRegistry {
    fn default() -> Self {
        Self {
            main_module: None,
            file_ids: Default::default(),
            link_targets: Default::default(),
            link_connections: Default::default(),
            module_id_to_file_id: Default::default(),
            live_files: Vec::new(),
            live_type_infos: Default::default(),
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

#[derive(Default)]
struct ProcessedImports{
    set: HashMap<LiveId,LiveFileId>
}


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
    pub fn link(&mut self, from:LiveId, to:LiveId){
        if let Some(from) = self.link_connections.get_mut(&from){
            *from = to;
        }
        else{
            self.link_connections.insert(from, to);
        }
    }
    
    pub fn file_ids(&self)->&BTreeMap<String, LiveFileId>{
        &self.file_ids
    }
    
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
    
    pub fn ptr_to_file_name_and_object_span(&self, ptr: LivePtr) -> (String, TextSpan){
        let doc = self.ptr_to_doc(ptr);
        let start_node = &doc.nodes[ptr.index as usize];
        let end_index = doc.nodes.skip_node(ptr.index as usize) - 1;
        let end_node = &doc.nodes[end_index];
        
        let start_token = self.token_id_to_token(start_node.origin.token_id().unwrap()).clone();
        let end_token = self.token_id_to_token(end_node.origin.token_id().unwrap()).clone();
        let start = start_token.span.start;
        let end = end_token.span.end;
        (
            self.file_id_to_file(ptr.file_id).file_name.clone(),
            TextSpan{
                file_id: ptr.file_id,
                start,
                end
            }
        )
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
    /*
    pub fn ptr_to_design_info(&self, live_ptr: LivePtr) -> Option<&LiveDesignInfo> {
        let doc = &self.live_files[live_ptr.file_id.to_index()];
        if doc.generation != live_ptr.generation {
            panic!("ptr_to_nodes_index generation invalid for file {} gen:{} ptr:{}", doc.file_name, doc.generation, live_ptr.generation);
        }
        match &doc.expanded.nodes[live_ptr.index as usize].value{
            LiveValue::Clone{design_info,..}|
            LiveValue::Deref{design_info,..}|
            LiveValue::Class {design_info,..}=>{
                // alright lets fetch the original doc
                if !design_info.is_invalid(){
                    // alright we parse the nodes
                    return Some(&doc.original.design_info[design_info.index()])
                }
            }
            _=>()
        }
        None
    }*/
    
    pub fn path_str_to_file_id(&self, path: &str) -> Option<LiveFileId> {
        for (index, file) in self.live_files.iter().enumerate() {
            if file.file_name == path {
                return Some(LiveFileId(index as u16))
            }
        }
        None
    }
    
    pub fn path_end_to_file_id(&self, path: &str) -> Option<LiveFileId> {
        for (index, file) in self.live_files.iter().enumerate() {
            if file.file_name.ends_with(path) {
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
    
    pub fn main_file_id(&self) -> Option<LiveFileId> {
        if let Some(m) = &self.main_module{
            if let Some(m) =  self.module_id_to_file_id.get(&m.module_id){
                return Some(m.clone())
            }
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
            LiveValue::Font (v) => {
                Some(v.path.as_str().to_string())
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
        if let LiveValue::Root(root) = &nodes[0].value {
            root.locals.get(&item).cloned()
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
                        next_new_line = last_index + i + 1;
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
                        next_new_line = last_index + i + 1;
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
                match Self::tokenize_from_str_live_design(&change.content, TextPos::default(), file_id, None) {
                    Err(msg) => errors.push(msg), //panic!("Lex error {}", msg),
                    Ok(new_tokens) => {
                        let live_file = self.file_id_to_file_mut(file_id);
                        
                        match LiveParser::parse_live_document(&new_tokens, &live_file.live_type_infos, file_id) {
                            Err(msg) => {
                                errors.push(msg);
                            },
                            Ok(mut ld) => { // only swap it out when it parses
                                any_changes = true;
                                ld.tokens = new_tokens;
                                self.build_imports_and_exports(&mut ld, module_id, &change.file_name);
                                let live_file = self.file_id_to_file_mut(file_id);
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
        
        let mut original = match LiveParser::parse_live_document(&tokens, &live_type_infos, file_id) {
            Err(msg) => return Err(msg.into_live_file_error(file_name)), //panic!("Parse error {}", msg.to_live_file_error(file, &source)),
            Ok(ld) => ld
        };
        original.tokens = tokens;
        // if we set a link, store it and point to our file_id
        if let Some(link) = original.link{
            // lets append our module_id
            if let Some(lt) = self.link_targets.get_mut(&link){
                lt.targets.push(file_id);
            }
            else{
                self.link_targets.insert(link, LiveLinkTarget{
                    targets: vec![file_id],
                    combined_exports: None
                });
            }
        }
        
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
        
        //let mut deps = BTreeSet::new();
        // lets populate all our exports on an exports table
        // lets only walk the root level
        self.build_imports_and_exports(&mut original, own_module_id, &file_name);
        
        let live_file = LiveFile {
            cargo_manifest_path: cargo_manifest_path.to_string(),
            reexpand: true,
            module_id: own_module_id,
            file_name: file_name.to_string(),
            start_pos,
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
    
    fn doc_original_raw_imports_to_resolved_recur(&mut self, file_id: LiveFileId, errors: &mut Vec<LiveError>, dep_order: &mut Vec<LiveFileId>){
        
        // lets see if we are already in the dep order, and ifso put us at the end
        if dep_order.iter().find(|v| **v == file_id).is_some(){
            
            fn recur_dep_order(file_id:LiveFileId, dep_order: &mut Vec<LiveFileId>, docs:&[LiveFile], recur_block: &mut Vec<LiveFileId>){
                if recur_block.contains(&file_id){
                    for recur in recur_block{
                        println!("Dependency recursion in: {:?}", docs[recur.to_index()].file_name);
                    }
                    return
                }
                recur_block.push(file_id);
                let doc = &docs[file_id.to_index()];
                let imports = &doc.original.resolved_imports.as_ref().unwrap();
                let pos = if let Some(pos) = dep_order.iter().position(|v| *v == file_id){
                    pos
                }
                else{
                    println!("Invalid state in dependency list for {}", docs[file_id.to_index()].file_name);
                    return
                };
                dep_order.remove(pos);
                dep_order.push(file_id);
                for import in imports.values(){
                    recur_dep_order(*import, dep_order, docs, recur_block)
                }
                for type_file_id in &doc.original.type_imports{
                    if file_id != *type_file_id{
                        recur_dep_order(*type_file_id, dep_order, docs, recur_block)
                    }
                }
                recur_block.pop();
            }
            let mut recur_block = Vec::new();
            recur_dep_order(file_id, dep_order, &mut self.live_files, &mut recur_block);
            return;
        }
        else{
            dep_order.push(file_id);
        }
        
        let doc = &self.live_files[file_id.to_index()];
        
        // per link targets we should collect all exports in one hashtable
        let mut ident_to_file:BTreeMap<LiveId,LiveFileId> = Default::default();
        for (origin,import) in &doc.original.raw_imports{
            // alright so if module id is link, we have to get a list of files
            if import.module_id.0 == live_id!(link){
                // module_id.1 is the link name
                let link_to = if let Some(link_to) = self.link_connections.get(&import.module_id.1){
                    *link_to
                }
                else{
                    import.module_id.1
                };
                
                if let Some(link_target) = self.link_targets.get(&link_to){
                    let combined_exports = link_target.combined_exports.as_ref().unwrap();
                    if import.import_id == LiveId(0){ // its a wildcard
                        // compare our entire symbolset with the combined_exports
                        for ident in &doc.original.identifiers{
                            if let Some(inner_file_id) = combined_exports.get(&ident){
                                if file_id != *inner_file_id{ // block refs to self
                                    ident_to_file.insert(*ident, *inner_file_id);
                                }
                            }
                        }
                    }
                    else{ // specific thing
                        if let Some(file_id) = combined_exports.get(&import.import_id){
                            ident_to_file.insert(import.import_id, *file_id);
                        }
                        else{
                            errors.push(LiveError {
                                origin: live_error_origin!(),
                                span: doc.original.token_id_to_span(origin.token_id().unwrap()).into(),
                                message: format!("Cannot find use target {}",import.import_id)
                            });
                        }
                    }
                }
                else{
                    errors.push(LiveError {
                        origin: live_error_origin!(),
                        span: doc.original.token_id_to_span(origin.token_id().unwrap()).into(),
                        message: format!("No link connection targets {}",link_to)
                    });
                }
            }
            else if let Some(inner_file_id) = self.module_id_to_file_id(import.module_id){
                let doc2 = &self.live_files[inner_file_id.to_index()];
                if import.import_id == LiveId(0){ // its a wildcard
                    // compare our entire symbolset with the combined_exports
                    for ident in &doc.original.identifiers{
                        if doc2.original.exports.get(&ident).is_some(){
                            if file_id != inner_file_id{ // block refs to self
                                ident_to_file.insert(*ident, inner_file_id);
                            }
                        }
                    }
                }
                else{ // specific thing
                    if  doc2.original.exports.get(&import.import_id).is_some(){
                        ident_to_file.insert(import.import_id, inner_file_id);
                    }
                    else{
                        errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: doc.original.token_id_to_span(origin.token_id().unwrap()).into(),
                            message: format!("Cannot find use target {}",import.import_id)
                        });
                    }
                }
            }
        }
        // store an empty one to block recursion
        let type_imports = doc.original.type_imports.clone();
        self.live_files[file_id.to_index()].original.resolved_imports = Some(ident_to_file.clone());
        // recur over all imported files
        for file_id in type_imports{
            self.doc_original_raw_imports_to_resolved_recur(file_id, errors, dep_order);
        }
        for file_id in ident_to_file.values(){
            self.doc_original_raw_imports_to_resolved_recur(*file_id, errors, dep_order);
        }
        
    }
    
    fn build_imports_and_exports(&self, doc:&mut LiveOriginal, own_module_id:LiveModuleId, _name:&str){
        let mut node_iter = doc.nodes.first_child(0);
                                
        while let Some(node_index) = node_iter {
                                                
            let node = &mut doc.nodes[node_index];
            if node.origin.node_has_prefix(){
                let prev_token = &doc.tokens[node.origin.first_def().unwrap().token_index()-1];
                if prev_token.token == LiveToken::Ident(live_id!(pub)){
                    doc.exports.insert(node.id, node.origin);
                }
            }
                                                
            match &mut node.value {
                LiveValue::Import(live_import) => {
                    if live_import.module_id.0 == live_id!(crate) { // patch up crate refs
                        live_import.module_id.0 = own_module_id.0
                    };
                    // ok lets emit the imports to the original
                    doc.raw_imports.push((node.origin, *live_import.clone()));
                },
                LiveValue::Class {live_type, ..} => {
                    // get our direct typed dependencies
                    let live_type_info = self.live_type_infos.get(live_type).unwrap();
                    for field in &live_type_info.fields {
                        let lti = &field.live_type_info;
                        
                        if let Some(file_id) = self.module_id_to_file_id.get(&lti.module_id) {
                            doc.type_imports.insert(*file_id);
                        }
                    }
                }
                _ => {
                }
            }
                                                             
            node_iter = doc.nodes.next_child(node_index);
        }
    }
    
    fn collect_combined_exports(&mut self, errors: &mut Vec<LiveError>){
        for link_target in self.link_targets.values_mut(){
            let mut combined_exports = HashMap::new();
            for target in &link_target.targets{
                // lets grab the file
                let doc = &self.live_files[target.to_index()];
                for (ident,origin) in &doc.original.exports{
                    if combined_exports.get(ident).is_some(){
                        errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: doc.original.token_id_to_span(origin.token_id().unwrap()).into(),
                            message: format!("Target already present in link set: {}",ident)
                        });
                    }
                    combined_exports.insert(*ident, *target);
                }
            }
            link_target.combined_exports = Some(combined_exports);
        }
    }
    
    
    pub fn expand_all_documents(&mut self, errors: &mut Vec<LiveError>) {
        // ok so first off
        // we need to run over our link_connections to gather our combined_exports
        self.collect_combined_exports(errors);
        
        // alright lets start at the main module
        // and then we have to hop from dependency to dependency
        let main_module_id = self.main_module.as_ref().unwrap().module_id;
        let main_file_id = self.module_id_to_file_id.get(&main_module_id).cloned().unwrap();
        
        let mut dep_order = Vec::new();
        self.doc_original_raw_imports_to_resolved_recur(main_file_id, errors, &mut dep_order);
        
        // FIX dont hardcode this, will fix it up with the icon refactor
        let fixup_file_id = self.path_end_to_file_id("draw_trapezoid.rs").unwrap();
        self.doc_original_raw_imports_to_resolved_recur(fixup_file_id, errors, &mut dep_order);
        
        
        /*for dep in &dep_order{
            println!("{}", self.file_id_to_file_name(*dep));
        }*/
                
        for file_id in dep_order.iter().rev() {
            /*
            if !self.live_files[file_id.to_index()].reexpand {
                continue;
            }*/
            let module_id = self.file_id_to_module_id(*file_id).unwrap();
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
/*
#[derive(Debug)]
pub struct DesignInfoRange{
    pub line: u32,
    pub start_column: u32,
    pub end_column: u32
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
}*/