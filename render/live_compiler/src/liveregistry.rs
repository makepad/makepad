//use crate::id::Id;
use crate::liveerror::{LiveError, LiveFileError, LiveErrorOrigin};
use makepad_id_macros::*;
use crate::livedocument::LiveDocument;
use crate::livenode::LiveNode;
use crate::livenode::LiveValue;
use crate::livenode::LiveType;
use crate::livenode::LiveNodeSlice;
use crate::liveparser::LiveParser;
use crate::id::Id;
use crate::id::FileId;
use crate::id::LivePtr;
use crate::token::TokenId;
use crate::span::Span;
use crate::id::ModulePath;
use std::collections::HashMap;
use std::collections::HashSet;
use crate::lex::lex;
use crate::liveexpander::LiveExpander;
use crate::liveexpander::ScopeStack;

pub struct LiveFile {
    pub module_path: ModulePath,
    pub line_offset: usize,
    pub file: String,
    pub source: String,
    pub document: LiveDocument,
}


#[derive(Default)]
pub struct LiveRegistry {
    pub file_ids: HashMap<String, FileId>,
    pub module_path_to_file_id: HashMap<ModulePath, FileId>,
    pub live_files: Vec<LiveFile>,
    pub dep_order: Vec<(ModulePath, TokenId)>,
    pub dep_graph: HashMap<ModulePath, HashSet<ModulePath >>, // this contains all the dependencies a crate has
    pub expanded: Vec<LiveDocument >,
}

impl LiveRegistry {
    pub fn resolve_ptr(&self, live_ptr: LivePtr) -> &LiveNode {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        &doc.resolve_ptr(live_ptr.local_ptr)
    }
    
    pub fn resolve_doc_ptr(&self, live_ptr: LivePtr) -> (&LiveDocument, &LiveNode) {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        (doc, &doc.resolve_ptr(live_ptr.local_ptr))
    }
    
    pub fn origin_doc_from_token_id(&self, token_id: TokenId) -> &LiveDocument {
        &self.live_files[token_id.file_id().to_index()].document
    }

    pub fn expanded_doc_from_token_id(&self, token_id: TokenId) -> &LiveDocument {
        &self.expanded[token_id.file_id().to_index()]
    }
    
    pub fn clone_from_module_path(&self, module_path: &str) -> Option<(FileId,Vec<LiveNode>)> {
        if let Some(file_id) = self.module_path_to_file_id.get(&ModulePath::from_str(module_path).unwrap()) {
            let doc = &self.expanded[file_id.to_index()];
            return Some((*file_id,doc.nodes.clone_child(0)));
        }
        None
    }
    
    pub fn live_error_to_live_file_error(&self, live_error: LiveError) -> LiveFileError {
        let live_file = &self.live_files[live_error.span.file_id().to_index()];
        live_error.to_live_file_error(&live_file.file, &live_file.source, live_file.line_offset)
    }
    
    
    pub fn token_id_to_span(&self, token_id: TokenId) -> Span {
        self.live_files[token_id.file_id().to_index()].document.token_id_to_span(token_id)
    }
    
    pub fn parse_live_file(
        &mut self,
        file: &str,
        own_module_path: ModulePath,
        source: String,
        live_types: Vec<LiveType>,
        line_offset: usize
    ) -> Result<FileId, LiveFileError> {
        
        let (is_new_file_id, file_id) = if let Some(file_id) = self.file_ids.get(file) {
            (false, *file_id)
        }
        else {
            let file_id = FileId::index(self.live_files.len());
            (true, file_id)
        };
        
        let lex_result = match lex(source.chars(), file_id) {
            Err(msg) => return Err(msg.to_live_file_error(file, &source, line_offset)), //panic!("Lex error {}", msg),
            Ok(lex_result) => lex_result
        };
        
        let mut parser = LiveParser::new(&lex_result.tokens, &live_types, file_id);
        
        let mut document = match parser.parse_live_document() {
            Err(msg) => return Err(msg.to_live_file_error(file, &source, line_offset)), //panic!("Parse error {}", msg.to_live_file_error(file, &source)),
            Ok(ld) => ld
        };
        document.strings = lex_result.strings;
        document.tokens = lex_result.tokens;
        
        // let own_crate_module = CrateModule(crate_id, module_id);
        
        if self.dep_order.iter().position( | v | v.0 == own_module_path).is_none() {
            self.dep_order.push((own_module_path, TokenId::new(file_id, 0)));
        }
        else {
            // marks dependencies dirty recursively (removes the expanded version)
            fn mark_dirty(mp: ModulePath, registry: &mut LiveRegistry) {
                if let Some(id) = registry.module_path_to_file_id.get(&mp) {
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
            mark_dirty(own_module_path, self);
        }
        
        let mut dep_graph_set = HashSet::new();
        
        for node in &mut document.nodes {
            match &mut node.value {
                LiveValue::Use {crate_id, module_id, ..} => {
                    if *crate_id == id!(crate){ // patch up crate refs
                        *crate_id = own_module_path.0
                    };

                    let module_path = ModulePath(*crate_id, *module_id); //document.use_ids_to_module_path(use_ids, own_module_path.0);
                    dep_graph_set.insert(module_path);
                    let self_index = self.dep_order.iter().position( | v | v.0 == own_module_path).unwrap();
                    if let Some(other_index) = self.dep_order.iter().position( | v | v.0 == module_path) {
                        if other_index > self_index {
                            self.dep_order.remove(other_index);
                            self.dep_order.insert(self_index, (module_path, node.token_id.unwrap()));
                        }
                    }
                    else {
                        self.dep_order.insert(self_index, (module_path, node.token_id.unwrap()));
                    }
                    
                }, // import
                _ => {
                }
            }
        }
        self.dep_graph.insert(own_module_path, dep_graph_set);
        
        let live_file = LiveFile {
            module_path: own_module_path,
            file: file.to_string(),
            line_offset,
            source,
            document
        };
        self.module_path_to_file_id.insert(own_module_path, file_id);
        
        if is_new_file_id {
            self.file_ids.insert(file.to_string(), file_id);
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
        
        for (crate_module, token_id) in &self.dep_order {
            let file_id = if let Some(file_id) = self.module_path_to_file_id.get(crate_module) {
                file_id
            }
            else {
                // ok so we have a token_id. now what.
                errors.push(LiveError {
                    origin: live_error_origin!(),
                    span: self.token_id_to_span(*token_id),
                    message: format!("Cannot find dependency: {}::{}", crate_module.0, crate_module.1)
                });
                continue
            };
            
            if !self.expanded[file_id.to_index()].recompile {
                continue;
            }
            let live_file = &self.live_files[file_id.to_index()];
            let in_doc = &live_file.document;
            
            let mut out_doc = LiveDocument::new();
            std::mem::swap(&mut out_doc, &mut self.expanded[file_id.to_index()]);
            out_doc.restart_from(&in_doc);
            
            let mut scope_stack = ScopeStack {
                stack: vec![Vec::new()]
            };
            //let len = in_doc.nodes[0].len();
            
            let mut live_document_expander = LiveExpander {
                module_path_to_file_id: &self.module_path_to_file_id,
                expanded: &self.expanded,
                in_crate: crate_module.0,
                in_file_id: *file_id,
                scope_stack: &mut scope_stack,
                errors
            };
            // OK now what. how will we do this.
            live_document_expander.expand(in_doc, &mut out_doc);
            
            
            out_doc.recompile = false;
            
            std::mem::swap(&mut out_doc, &mut self.expanded[file_id.to_index()]);
        }
    }
}

