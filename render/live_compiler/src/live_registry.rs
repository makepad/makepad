//use crate::id::Id;
use {
    std::collections::{HashMap, HashSet},
    makepad_id_macros::*,
    crate::{
        live_error::{LiveError, LiveFileError},
        live_parser::LiveParser,
        live_document::LiveDocument,
        live_node::{LiveNode, LiveValue, LiveType, LiveTypeInfo,LiveNodeOrigin},
        live_node_vec::{LiveNodeSlice},
        live_id::{LiveId, LiveFileId, LivePtr, LiveModuleId},
        token::TokenId,
        span::{Span, TextPos},
        lex::lex,
        live_expander::{LiveExpander} 
    }
};


#[derive(Default)]
pub struct LiveFile {
    pub module_id: LiveModuleId,
    pub start_pos: TextPos,
    pub file_name: String,
    pub source: String,
    pub document: LiveDocument,
}

pub struct LiveRegistry {
    pub file_ids: HashMap<String, LiveFileId>,
    pub module_id_to_file_id: HashMap<LiveModuleId, LiveFileId>,
    pub live_files: Vec<LiveFile>,
    pub live_type_infos: HashMap<LiveType, LiveTypeInfo>,
    pub dep_order: Vec<(LiveModuleId, TokenId)>,
    pub dep_graph: HashMap<LiveModuleId, HashSet<LiveModuleId >>, // this contains all the dependencies a crate has
    pub expanded: Vec<LiveDocument>,
}

impl Default for LiveRegistry{
    fn default()->Self{
        
        Self{
            file_ids: HashMap::new(),
            module_id_to_file_id: HashMap::new(),
            live_files: vec![LiveFile::default()],
            live_type_infos: HashMap::new(),
            dep_order: Vec::new(),
            dep_graph: HashMap::new(),// this contains all the dependencies a crate has
            expanded: vec![LiveDocument::default()]
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

impl LiveRegistry {
    pub fn ptr_to_node(&self, live_ptr: LivePtr) -> &LiveNode {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        &doc.resolve_ptr(live_ptr.index as usize)
    }
    
    pub fn file_id_to_file_name(&self, file_id: LiveFileId) -> &str {
        &self.live_files[file_id.to_index()].file_name
    }
    
    pub fn ptr_to_doc_node(&self, live_ptr: LivePtr) -> (&LiveDocument, &LiveNode) {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        (doc, &doc.resolve_ptr(live_ptr.index as usize))
    }
    
    pub fn ptr_to_doc(&self, live_ptr: LivePtr) -> &LiveDocument {
        &self.expanded[live_ptr.file_id.to_index()]
    }
    
    pub fn file_id_to_doc(&self, file_id: LiveFileId) -> &LiveDocument {
        &self.expanded[file_id.to_index()]
    }
    
    pub fn ptr_to_nodes_index(&self, live_ptr: LivePtr) -> (&[LiveNode], usize) {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        (&doc.nodes, live_ptr.index as usize)
    }
    
    pub fn token_id_to_origin_doc(&self, token_id: TokenId) -> &LiveDocument {
        &self.live_files[token_id.file_id().to_index()].document
    }
    
    pub fn token_id_to_expanded_doc(&self, token_id: TokenId) -> &LiveDocument {
        &self.expanded[token_id.file_id().to_index()]
    }

    pub fn module_id_to_file_id(&self, module_id: LiveModuleId) -> Option<LiveFileId> {
        self.module_id_to_file_id.get(&module_id).cloned()
    }

    pub fn module_id_and_name_to_doc(&self, module_id: LiveModuleId, name: LiveId) -> Option<LiveDocNodes> {
        if let Some(file_id) = self.module_id_to_file_id.get(&module_id) {
            let doc = &self.expanded[file_id.to_index()];
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
    
    pub fn find_scope_target_via_start(&self, item: LiveId, index:usize, nodes:&[LiveNode]) -> Option<LiveScopeTarget> {
        if let Some(index) = nodes.scope_up_down_by_name(index, item) {
            if let LiveValue::Use(module_id) = &nodes[index].value {
                // ok lets find it in that other doc
                if let Some(file_id) = self.module_id_to_file_id(*module_id) {
                    let doc = self.file_id_to_doc(file_id);
                    if let Some(index) = doc.nodes.child_by_name(0, item){
                        return Some(LiveScopeTarget::LivePtr(
                            LivePtr{file_id:file_id, index:index as u32}
                        ))
                    }
                }
            }
            else{
                return Some(LiveScopeTarget::LocalPtr(index))
            }
        }
        // ok now look at the glob use * things
        let mut node_iter = Some(1);
        while let Some(index) = node_iter {
            if let LiveValue::Use(module_id) = &nodes[index].value {
                if nodes[index].id == LiveId::empty(){ // glob
                    if let Some(file_id) = self.module_id_to_file_id(*module_id) {
                        let doc = self.file_id_to_doc(file_id);
                        if let Some(index) = doc.nodes.child_by_name(0, item){
                            return Some(LiveScopeTarget::LivePtr(
                                LivePtr{file_id:file_id, index:index as u32}
                            ))
                        }
                    }
                }
            }
            node_iter = nodes.next_child(index);
        }
        None
    }
    
    pub fn find_scope_ptr_via_origin(&self, origin:LiveNodeOrigin, item: LiveId) -> Option<LivePtr> {
        // ok lets start
        let token_id = origin.token_id().unwrap();
        let index = origin.node_index().unwrap();
        let file_id = token_id.file_id();
        let doc = self.file_id_to_doc(file_id);
        match self.find_scope_target_via_start(item, index, &doc.nodes){
            Some(LiveScopeTarget::LocalPtr(index))=>Some(LivePtr{file_id:file_id, index:index as u32}),
            Some(LiveScopeTarget::LivePtr(ptr))=>Some(ptr),
            None=>None
        }
    }
    
    
    pub fn live_error_to_live_file_error(&self, live_error: LiveError) -> LiveFileError {
        let live_file = &self.live_files[live_error.span.file_id.to_index()];
        live_error.to_live_file_error(&live_file.file_name)
    }
    
    
    pub fn token_id_to_span(&self, token_id: TokenId) -> Span {
        self.live_files[token_id.file_id().to_index()].document.token_id_to_span(token_id)
    }
    
    pub fn insert_dep_order(&mut self, module_id: LiveModuleId, token_id: TokenId, own_module_id: LiveModuleId) {
        let self_index = self.dep_order.iter().position( | v | v.0 == own_module_id).unwrap();
        if let Some(other_index) = self.dep_order.iter().position( | v | v.0 == module_id) {
            // if other_index is > self index. we should move self later
            
            if other_index > self_index {
                self.dep_order.remove(other_index);
                self.dep_order.insert(self_index, (module_id, token_id));
            }
        }
        else {
            self.dep_order.insert(self_index, (module_id, token_id));
        }
    }
    
    pub fn parse_live_file(
        &mut self,
        file_name: &str,
        own_module_id: LiveModuleId,
        source: String,
        live_type_infos: Vec<LiveTypeInfo>,
        start_pos: TextPos,
    ) -> Result<LiveFileId, LiveFileError> {
        
        // lets register our live_type_infos
        
        let (is_new_file_id, file_id) = if let Some(file_id) = self.file_ids.get(file_name) {
            (false, *file_id)
        }
        else {
            let file_id = LiveFileId::index(self.live_files.len());
            (true, file_id)
        };
        
        let lex_result = match lex(source.chars(), start_pos, file_id) {
            Err(msg) => return Err(msg.to_live_file_error(file_name)), //panic!("Lex error {}", msg),
            Ok(lex_result) => lex_result
        };
        
        let mut parser = LiveParser::new(&lex_result.tokens, &live_type_infos, file_id);
        
        let mut document = match parser.parse_live_document() {
            Err(msg) => return Err(msg.to_live_file_error(file_name)), //panic!("Parse error {}", msg.to_live_file_error(file, &source)),
            Ok(ld) => ld
        };
        document.strings = lex_result.strings;
        document.tokens = lex_result.tokens;
        
        // update our live type info
        for live_type_info in live_type_infos {
            if let Some(info) = self.live_type_infos.get(&live_type_info.live_type) {
                if info.module_id != live_type_info.module_id ||
                info.live_type != live_type_info.live_type {
                    panic!()
                }
            };
            self.live_type_infos.insert(live_type_info.live_type, live_type_info);
        }
        
        // let own_crate_module = CrateModule(crate_id, module_id);
        
        if self.dep_order.iter().position( | v | v.0 == own_module_id).is_none() {
            self.dep_order.push((own_module_id, TokenId::new(file_id, 0)));
        }
        else {
            // marks dependencies dirty recursively (removes the expanded version)
            fn mark_dirty(mp: LiveModuleId, registry: &mut LiveRegistry) {
                if let Some(id) = registry.module_id_to_file_id.get(&mp) {
                    registry.expanded[id.to_index()].recompile = true;
                }
                //registry.expanded.remove(&cm);
                
                let mut dirty = Vec::new();
                for (mp_iter, hs) in &registry.dep_graph {
                    if hs.contains(&mp) { // this
                        dirty.push(*mp_iter);
                    }
                }
                for d in dirty {
                    mark_dirty(d, registry);
                }
            }
            mark_dirty(own_module_id, self);
        }
        
        let mut dep_graph_set = HashSet::new();
        
        for node in &mut document.nodes {
            match &mut node.value {
                LiveValue::Use(module_id) => {
                    if module_id.0 == id!(crate) { // patch up crate refs
                        module_id.0 = own_module_id.0
                    };
                    
                    dep_graph_set.insert(*module_id);
                    self.insert_dep_order(*module_id, node.origin.token_id().unwrap(), own_module_id);
                    
                }, // import
                LiveValue::Class {live_type, ..} => { // hold up. this is always own_module_path
                    let infos = self.live_type_infos.get(&live_type).unwrap();
                    for sub_type in infos.fields.clone() {
                        let sub_module_id = sub_type.live_type_info.module_id;
                        if sub_module_id != own_module_id {
                            dep_graph_set.insert(sub_module_id);
                            
                            self.insert_dep_order(sub_module_id, node.origin.token_id().unwrap(), own_module_id);
                        }
                    }
                }
                _ => {
                }
            }
        }
        self.dep_graph.insert(own_module_id, dep_graph_set);
        
        let live_file = LiveFile {
            module_id: own_module_id,
            file_name: file_name.to_string(),
            start_pos,
            source,
            document
        };
        self.module_id_to_file_id.insert(own_module_id, file_id);
        
        if is_new_file_id {
            self.file_ids.insert(file_name.to_string(), file_id);
            self.live_files.push(live_file);
            self.expanded.push(LiveDocument::new());
        }
        else {
            self.live_files[file_id.to_index()] = live_file;
            self.expanded[file_id.to_index()].recompile = true;
        }
        
        return Ok(file_id)
    }
    
    pub fn expand_all_documents(&mut self, errors: &mut Vec<LiveError>) {

        for (crate_module, _token_id) in &self.dep_order {
            let file_id = if let Some(file_id) = self.module_id_to_file_id.get(crate_module) {
                file_id
            }
            else {
                //println!("DEP NOT FOUND {}", crate_module);
                // ok so we have a token_id. now what.
                /*Errors.push(LiveError {
                    origin: live_error_origin!(),
                    span: self.token_id_to_span(*token_id),
                    message: format!("Cannot find dependency: {}::{}", crate_module.0, crate_module.1)
                });*/
                continue
            };
            //println!("DEP ORDER {} {}", crate_module, file_id.0);
            
            if !self.expanded[file_id.to_index()].recompile {
                continue;
            }
            let live_file = &self.live_files[file_id.to_index()];
            let in_doc = &live_file.document;
            
            let mut out_doc = LiveDocument::new();
            std::mem::swap(&mut out_doc, &mut self.expanded[file_id.to_index()]);
            out_doc.restart_from(&in_doc);
            
            /*let mut scope_stack = ScopeStack {
                stack: vec![Vec::new()]
            };*/
            //let len = in_doc.nodes[0].len();
            
            let mut live_document_expander = LiveExpander {
                live_registry: self,
                in_crate: crate_module.0,
                in_file_id: *file_id,
                //scope_stack: &mut scope_stack,
                errors
            };
            // OK now what. how will we do this.
            live_document_expander.expand(in_doc, &mut out_doc);
            
            
            out_doc.recompile = false;
            
            std::mem::swap(&mut out_doc, &mut self.expanded[file_id.to_index()]);
        }
    }
}

