use crate::id::{Id, IdPack, IdUnpack};
use crate::liveerror::{LiveError, LiveFileError, LiveErrorOrigin};
use makepad_id_macros::*;
use crate::livedocument::LiveDocument;
use crate::livenode::LiveNode;
use crate::livenode::LiveValue;
use crate::livenode::LiveType;
use crate::liveparser::LiveParser;
use crate::liveparser::LiveEnumInfo;
use crate::id::FileId;
use crate::id::LocalPtr;
use crate::id::LivePtr;
use crate::token::TokenId;
use crate::span::Span;
use crate::id::ModulePath;
use std::collections::HashMap;
use std::collections::HashSet;
use crate::lex::lex;
use crate::liveexpander::LiveExpander;
use crate::liveexpander::ScopeStack;
//use std::fmt;

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

pub struct LiveObjectIterator {
    file_id: FileId,
    level: usize,
    node_start: usize,
    node_count: usize,
    index: usize
}

impl LiveObjectIterator {
    pub fn next_id(&mut self, live_registry: &LiveRegistry) -> Option<(Id, LivePtr)> {
        // ok so we get the
        loop {
            if self.index >= self.node_count {
                return None
            }
            
            let id = live_registry.expanded[self.file_id.to_index()]
                .nodes[self.level][self.index + self.node_start].id;
            
            self.index += 1;
            
            return Some((id, LivePtr {
                file_id: self.file_id,
                local_ptr: LocalPtr {
                    level: self.level,
                    index: self.index - 1 + self.node_start
                }
            }));
        }
    }
    pub fn next_prop(&mut self) -> Option<(usize, LivePtr)> {
        // ok so we get the
        loop {
            if self.index >= self.node_count {
                return None
            }
            
            self.index += 1;
            
            return Some((self.index - 1, LivePtr {
                file_id: self.file_id,
                local_ptr: LocalPtr {
                    level: self.level,
                    index: self.index - 1 + self.node_start
                }
            }));
        }
    }
}

impl LiveRegistry {
    
    pub fn live_ptr_from_path(&self, module_path: ModulePath, object_path: &[Id]) -> Option<LivePtr> {
        if let Some(file_id) = self.module_path_to_file_id.get(&module_path) {
            let doc = &self.expanded[file_id.to_index()];
            if let Some(local_ptr) = doc.scan_for_object_path(object_path) {
                return Some(LivePtr {
                    file_id: *file_id,
                    local_ptr
                })
            }
        }
        None
    }
    
    pub fn resolve_doc_ptr(&self, live_ptr: LivePtr) -> (&LiveDocument, &LiveNode) {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        (doc, &doc.resolve_ptr(live_ptr.local_ptr))
    }
    
    pub fn live_object_iterator(&self, live_ptr: LivePtr, node_start: u32, node_count: u16) -> LiveObjectIterator {
        LiveObjectIterator {
            file_id: live_ptr.file_id,
            level: live_ptr.local_ptr.level + 1,
            index: 0,
            node_start: node_start as usize,
            node_count: node_count as usize,
        }
    }
    
    pub fn live_class_iterator(&self, live_ptr: LivePtr) -> Option<LiveObjectIterator> {
        let node = self.resolve_ptr(live_ptr);
        if let LiveValue::Class {node_start, node_count, ..} = node.value {
            Some(LiveObjectIterator {
                file_id: live_ptr.file_id,
                level: live_ptr.local_ptr.level + 1,
                index: 0,
                node_start: node_start as usize,
                node_count: node_count as usize,
            })
        }
        else {
            return None
        }
    }
    
    pub fn resolve_ptr(&self, live_ptr: LivePtr) -> &LiveNode {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        &doc.resolve_ptr(live_ptr.local_ptr)
    }
    
    pub fn live_error_to_live_file_error(&self, live_error: LiveError) -> LiveFileError {
        let live_file = &self.live_files[live_error.span.file_id().to_index()];
        live_error.to_live_file_error(&live_file.file, &live_file.source, live_file.line_offset)
    }
    
    
    pub fn find_enum_origin(&self, start: IdPack, lhs: Id) -> Id {
        match start.unpack() {
            IdUnpack::LivePtr(live_ptr) => {
                let doc = &self.expanded[live_ptr.file_id.to_index()];
                let node = &doc.nodes[live_ptr.local_ptr.level][live_ptr.local_ptr.index];
                match node.value {
                    LiveValue::IdPack(id) => {
                        return self.find_enum_origin(id, node.id)
                    }
                    LiveValue::Class {class, ..} => {
                        return self.find_enum_origin(class, node.id)
                    },
                    LiveValue::Call {target, ..} => {
                        return self.find_enum_origin(target, node.id)
                    },
                    _ => ()
                }
            }
            _ => ()
        }
        lhs
    }
    /*
    pub fn find_full_node_ptr_from_ids(&self, crate_id: Id, module_id: Id, ids: &[Id]) -> Option<LivePtr> {
        let cm = CrateModule(crate_id, module_id);
        if let Some(file_id) = self.crate_module_to_file_id.get(&cm) {
            let exp = &self.expanded[file_id.to_index()];
            if let Some(local_ptr) = exp.scan_for_multi(ids) {
                let node = &exp.nodes[local_ptr.level][local_ptr.index];
                match node.value {
                    LiveValue::Class {..} => {
                        return Some(LivePtr {file_id: *file_id, local_ptr})
                    },
                    _ => ()
                }
            }
        }
        None
    }*/
    
    pub fn find_base_class_id(&self, class: IdPack) -> Option<IdPack> {
        let mut class_iter = class;
        while let IdUnpack::LivePtr(live_ptr) = class_iter.unpack() {
            let other_node = self.resolve_ptr(live_ptr);
            if let LiveValue::Class {class, ..} = other_node.value {
                class_iter = class;
            }
            else {
                return None
            }
        }
        Some(class_iter)
    }
    
    pub fn token_id_to_span(&self, token_id: TokenId) -> Span {
        self.live_files[token_id.file_id.to_index()].document.token_id_to_span(token_id)
    }
    
    pub fn find_module_path_by_file_id(&self, scan_file_id: FileId) -> Option<ModulePath> {
        for (module_path, file_id) in &self.module_path_to_file_id {
            if *file_id == scan_file_id {
                return Some(*module_path)
            }
        }
        return None
    }
    
    pub fn parse_live_file(
        &mut self,
        file: &str,
        own_module_path: ModulePath,
        source: String,
        live_types: Vec<LiveType>,
        live_enums: &HashMap<LiveType, LiveEnumInfo>,
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
        
        let mut parser = LiveParser::new(&lex_result.tokens, &live_types, live_enums, file_id);
        
        let mut document = match parser.parse_live_document() {
            Err(msg) => return Err(msg.to_live_file_error(file, &source, line_offset)), //panic!("Parse error {}", msg.to_live_file_error(file, &source)),
            Ok(ld) => ld
        };
        document.strings = lex_result.strings;
        document.tokens = lex_result.tokens;
        
        // let own_crate_module = CrateModule(crate_id, module_id);
        
        if self.dep_order.iter().position( | v | v.0 == own_module_path).is_none() {
            self.dep_order.push((own_module_path, TokenId {file_id, token_id: 0}));
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
        
        for (_, nodes) in document.nodes.iter().enumerate() {
            for node in nodes {
                match node.value {
                    LiveValue::Use {use_ids} => {
                        let module_path = document.use_ids_to_module_path(use_ids, own_module_path.0);
                        dep_graph_set.insert(module_path);
                        let self_index = self.dep_order.iter().position( | v | v.0 == own_module_path).unwrap();
                        if let Some(other_index) = self.dep_order.iter().position( | v | v.0 == module_path) {
                            if other_index > self_index {
                                self.dep_order.remove(other_index);
                                self.dep_order.insert(self_index, (module_path, node.token_id));
                            }
                        }
                        else {
                            self.dep_order.insert(self_index, (module_path, node.token_id));
                        }
                        
                    }, // import
                    _ => {
                    }
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
            let len = in_doc.nodes[0].len();
            
            let mut live_document_expander = LiveExpander {
                module_path_to_file_id: &self.module_path_to_file_id,
                expanded: &self.expanded,
                in_crate: crate_module.0,
                in_file_id: *file_id,
                scope_stack: &mut scope_stack,
                errors
            };
            
            for i in 0..len {
                live_document_expander.walk_node(
                    in_doc,
                    0,
                    i,
                    &mut out_doc,
                    0,
                    0,
                    0
                );
            }
            
            out_doc.recompile = false;
            
            std::mem::swap(&mut out_doc, &mut self.expanded[file_id.to_index()]);
        }
    }
}

