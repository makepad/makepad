#![allow(unused_variables)]
use crate::id::{Id, IdType, IdFmt};
use crate::liveerror::{LiveError, LiveFileError};
use makepad_live_derive::*;
use crate::livedocument::LiveDocument;
use crate::livedocument::{LiveNodePtr, LiveScopeTarget, LiveScopeItem};
use crate::livenode::{LiveNode, LiveValue};
use crate::liveparser::LiveParser;
use crate::span::LiveFileId;
use crate::token::TokenId;
use crate::span::Span;
use std::collections::HashMap;
use std::collections::HashSet;
use crate::lex::lex;
use std::fmt;

pub struct LiveFile {
    file: String,
    source: String,
    document: LiveDocument,
}

#[derive(Clone, Eq, Hash, Debug, Copy, PartialEq)]
pub struct CrateModule(pub Id, pub Id); 


impl fmt::Display for CrateModule {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}::{}", self.0, self.1)
    }
}

#[derive(Default)]
pub struct LiveRegistry {
    live_file_ids: HashMap<String, LiveFileId>,
    crate_module_to_file_id: HashMap<CrateModule, LiveFileId>,
    live_files: Vec<LiveFile>,
    dep_order: Vec<(CrateModule, TokenId)>,
    dep_graph: HashMap<CrateModule, HashSet<CrateModule >>, // this contains all the dependencies a crate has
    expanded: HashMap<CrateModule, LiveDocument>
}

impl LiveRegistry {
    pub fn is_baseclass(id: Id) -> bool {
        id == id!(Component) || id == id!(Enum) || id == id!(Struct) || id == id!(Shader)
    }
    
    pub fn token_id_to_span(&self, token_id: TokenId) -> Span {
        self.live_files[token_id.live_file_id.0 as usize].document.token_id_to_span(token_id)
    }
    
    
    pub fn parse_live_file(&mut self, file: &str, crate_id: Id, module_id: Id, source: String) -> Result<LiveFileId, LiveFileError> {
        
        let (is_new_file_id, file_id) = if let Some(file_id) = self.live_file_ids.get(file) {
            (false, *file_id)
        }
        else {
            let file_id = LiveFileId(self.live_files.len() as u16);
            (true, file_id)
        };
        
        let lex_result = match lex(source.chars(), file_id) {
            Err(msg) => panic!("Lex error {}", msg),
            Ok(lex_result) => lex_result
        };
        
        let mut parser = LiveParser::new(&lex_result);
        
        let mut document = match parser.parse_live_document() {
            Err(msg) => panic!("Parse error {}", msg.to_live_file_error(file, &source)),
            Ok(ld) => ld
        };
        document.strings = lex_result.strings;
        document.tokens = lex_result.tokens;
        
        let own_crate_module = CrateModule(crate_id, module_id);
        
        if self.dep_order.iter().position( | v | v.0 == own_crate_module).is_none() {
            self.dep_order.push((own_crate_module, TokenId::default()));
        }
        else {
            // marks dependencies dirty recursively (removes the expanded version)
            fn mark_dirty(cm: CrateModule, registry: &mut LiveRegistry) {
                registry.expanded.remove(&cm);
                
                let mut dirty = Vec::new();
                for (cm_iter, hs) in &registry.dep_graph {
                    if hs.contains(&cm) { // this
                        dirty.push(*cm_iter);
                    }
                }
                for d in dirty {
                    mark_dirty(d, registry);
                }
            }
            mark_dirty(own_crate_module, self);
        }
        
        let mut dep_graph_set = HashSet::new();
        
        // lets emit all our and check our imports
        for (level, nodes) in document.nodes.iter().enumerate() {
            for node in nodes {
                match node.value {
                    LiveValue::Use {crate_module} => {
                        let crate_module = document.fetch_crate_module(crate_module, crate_id);
                        dep_graph_set.insert(crate_module);
                        let self_index = self.dep_order.iter().position( | v | v.0 == own_crate_module).unwrap();
                        if let Some(other_index) = self.dep_order.iter().position( | v | v.0 == crate_module) {
                            if other_index > self_index {
                                self.dep_order.remove(other_index);
                                self.dep_order.insert(self_index, (crate_module, node.token_id));
                            }
                        }
                        else {
                            self.dep_order.insert(self_index, (crate_module, node.token_id));
                        }
                        
                    }, // import
                    _ => {
                    }
                }
            }
        }
        self.dep_graph.insert(own_crate_module, dep_graph_set);
        
        // move these on there
        let live_file = LiveFile {
            file: file.to_string(),
            source,
            document
        };
        self.crate_module_to_file_id.insert(own_crate_module, file_id);
        // only insert the file id if it parsed
        if is_new_file_id {
            self.live_file_ids.insert(file.to_string(), file_id);
        }
        self.live_files.push(live_file);
        
        return Ok(file_id)
    }
    
    pub fn expand_all_documents(&mut self, errors: &mut Vec<LiveError>) {
        
        
        
        struct ScopeStack {
            stack: Vec<Vec<LiveScopeItem >>
        }
        
        impl ScopeStack {
            fn find_item(&self, id: Id) -> Option<LiveScopeTarget> {
                if id.is_single() {
                    for items in self.stack.iter().rev() {
                        for item in items.iter().rev() {
                            if item.id == id {
                                return Some(item.target)
                            }
                        }
                    }
                }
                else {
                    
                }
                return None
            }
        }
        
        #[derive(Debug)]
        enum CopyRecurResult {
            IsClass {class: Id},
            IsArray,
            IsObject,
            IsValue,
            IsString {string_start: u32, string_count: u32},
            IsFn {token_start: u32, token_count: u32, scope_start: u32, scope_count: u16},
            IsCall {target: Id},
            Noop,
            Error
        }
        
        fn copy_recur(
            scope_stack: &mut ScopeStack,
            in_doc: Option<(&LiveDocument, CrateModule)>,
            out_doc: &mut LiveDocument,
            skip_level: usize,
            in_level: usize,
            out_level: usize,
            in_index: usize,
        ) -> CopyRecurResult {
            let node = if let Some((in_doc, _)) = in_doc {
                in_doc.nodes[in_level][in_index]
            }
            else {
                out_doc.nodes[in_level][in_index]
            };
            
            let out_index = out_doc.get_level_len(out_level);
            if in_level == skip_level + 1 && scope_stack.stack.len() - 1 == out_level { // first level, store on scope
                scope_stack.stack[out_level].push(LiveScopeItem {
                    id: node.id,
                    target: LiveScopeTarget::Local {node_ptr: LiveNodePtr {level: out_level, index: out_index}}
                });
            }
            
            match node.value {
                LiveValue::Call {target, node_start, node_count} => {
                    let out_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        copy_recur(scope_stack, in_doc, out_doc, skip_level, in_level + 1, out_level + 1, i as usize + node_start as usize);
                    }
                    if skip_level != in_level {
                        out_doc.push_node(out_level, LiveNode {
                            token_id: node.token_id,
                            id: node.id,
                            value: LiveValue::Call {
                                target: target,
                                node_start: out_start as u32,
                                node_count: node_count
                            }
                        });
                    }
                    return CopyRecurResult::IsCall {target}
                },
                LiveValue::Array {node_start, node_count} => {
                    let out_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        copy_recur(scope_stack, in_doc, out_doc, skip_level, in_level + 1, out_level + 1, i as usize + node_start as usize);
                    }
                    if skip_level != in_level {
                        out_doc.push_node(out_level, LiveNode {
                            token_id: node.token_id,
                            id: node.id,
                            value: LiveValue::Array {
                                node_start: out_start as u32,
                                node_count: node_count
                            }
                        });
                    }
                    return CopyRecurResult::IsArray
                },
                LiveValue::Object {node_start, node_count} => {
                    let out_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        copy_recur(scope_stack, in_doc, out_doc, skip_level, in_level + 1, out_level + 1, i as usize + node_start as usize);
                    }
                    if skip_level != in_level {
                        out_doc.push_node(out_level, LiveNode {
                            token_id: node.token_id,
                            id: node.id,
                            value: LiveValue::Object {
                                node_start: out_start as u32,
                                node_count: node_count
                            }
                        });
                    }
                    return CopyRecurResult::IsObject
                },
                LiveValue::Use {crate_module} => { // no need to output there.
                }
                LiveValue::Class {class, node_start, node_count} => {
                    if class == id!(Self) {
                        return CopyRecurResult::Noop
                    }
                    let out_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        copy_recur(scope_stack, in_doc, out_doc, skip_level, in_level + 1, out_level + 1, i as usize + node_start as usize);
                    }
                    if skip_level != in_level {
                        out_doc.push_node(out_level, LiveNode {
                            token_id: node.token_id,
                            id: node.id,
                            value: LiveValue::Class {
                                class: class,
                                node_start: out_start as u32,
                                node_count: node_count
                            }
                        });
                    }
                    return CopyRecurResult::IsClass {class}
                },
                LiveValue::String {string_start, string_count} => {
                    let new_string_start = if let Some((in_doc, _)) = in_doc { // copy the string if its from another doc
                        let nsi = out_doc.strings.len();
                        for i in 0..string_count {
                            out_doc.strings.push(in_doc.strings[(i + string_start) as usize]);
                        }
                        nsi
                    }
                    else {
                        string_start as usize
                    };
                    if skip_level != in_level {
                        // we need to use another Id
                        out_doc.push_node(out_level, node);
                    }
                    return CopyRecurResult::IsString {string_start: new_string_start as u32, string_count: string_count as u32}
                }
                LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                    let (new_token_start, new_scope_start) = if let Some((in_doc, in_crate_module)) = in_doc { // copy the string if its from another doc
                        let nts = out_doc.tokens.len();
                        let nss = out_doc.scopes.len();
                        for i in 0..(token_count as usize) {
                            out_doc.tokens.push(in_doc.tokens[i + token_start as usize]);
                        }
                        for i in 0..(scope_count as usize) {
                            let item = &in_doc.scopes[i + scope_start as usize];
                            // if item is local, it is now 'remote'.
                            match item.target{
                                LiveScopeTarget::Local{node_ptr}=>{
                                    out_doc.scopes.push(LiveScopeItem{
                                        id:item.id,
                                        target:LiveScopeTarget::Use{
                                            crate_module: in_crate_module,
                                            node_ptr
                                        }
                                    });
                                },
                                LiveScopeTarget::Use{..}=>{
                                    out_doc.scopes.push(*item);
                                }
                            }
                        }
                        (nts as u32, nss as u32)
                    }
                    else {
                        (token_start, scope_start)
                    };
                    return CopyRecurResult::IsFn {token_start: new_token_start, token_count, scope_start: new_scope_start, scope_count}
                }
                _ => {
                    if skip_level != in_level {
                        // we need to use another Id
                        out_doc.push_node(out_level, node);
                    }
                    return CopyRecurResult::IsValue
                }
            }
            return CopyRecurResult::Noop
        }
        
        fn write_or_add_node(scope_stack: &mut ScopeStack, errors: &mut Vec<LiveError>, out_doc: &mut LiveDocument, level: usize, node_start: usize, node_count: usize, in_doc: &LiveDocument, in_node: &LiveNode) {
            match out_doc.write_or_add_node(level, node_start, node_count, in_doc, in_node) {
                Err(err) => errors.push(err),
                Ok(Some(index)) => {
                    if scope_stack.stack.len() - 1 == level {
                        scope_stack.stack[level].push(LiveScopeItem {
                            id: in_node.id,
                            target: LiveScopeTarget::Local {node_ptr: LiveNodePtr {level: level, index: index}}
                        });
                    }
                }
                _ => ()
            }
        }
        
        // This should we win me some kind of award. Absolute worst programmer in recent history or something like it.
        fn walk_node(
            expanded: &HashMap<CrateModule, LiveDocument>,
            in_crate: Id,
            errors: &mut Vec<LiveError>,
            scope_stack: &mut ScopeStack,
            in_doc: &LiveDocument,
            out_doc: &mut LiveDocument,
            in_level: usize,
            out_level: usize,
            in_node_index: usize,
            out_start: usize,
            out_count: usize
        ) {
            let node = &in_doc.nodes[in_level][in_node_index];
            
            //let (row,col) = byte_to_row_col(node.span.start(), &ld.source);
            //let _ = write!(f, "/*{},{} {}*/", row+1, col, node.span.len());
            match node.value {
                LiveValue::String {..} => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Bool(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Int(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Float(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Color(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Vec2(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Vec3(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Id(_val) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Call {target, node_start, node_count} => {
                    let new_node_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        walk_node(expanded, in_crate, errors, scope_stack, in_doc, out_doc, in_level + 1, out_level + 1, i as usize + node_start as usize, out_start, 0);
                    }
                    let new_node = LiveNode {
                        token_id: node.token_id,
                        id: node.id,
                        value: LiveValue::Call {
                            target,
                            node_start: new_node_start as u32,
                            node_count: node_count
                        }
                    };
                    write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                },
                LiveValue::Array {node_start, node_count} => { // normal array
                    let new_node_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        walk_node(expanded, in_crate, errors, scope_stack, in_doc, out_doc, in_level + 1, out_level + 1, i as usize + node_start as usize, out_start, 0);
                    }
                    let new_node = LiveNode {
                        token_id: node.token_id,
                        id: node.id,
                        value: LiveValue::Array {
                            node_start: new_node_start as u32,
                            node_count: node_count as u32
                        }
                    };
                    write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                },
                LiveValue::Object {node_start, node_count} => {
                    let new_node_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        walk_node(expanded, in_crate, errors, scope_stack, in_doc, out_doc, in_level + 1, out_level + 1, i as usize + node_start as usize, out_start, 0);
                    }
                    let new_node = LiveNode {
                        token_id: node.token_id,
                        id: node.id,
                        value: LiveValue::Object {
                            node_start: new_node_start as u32,
                            node_count: node_count as u32
                        }
                    };
                    write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                },
                LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                    // we should store the scopestack here so the shader compiler can find symbols.
                    let new_scope_start = out_doc.scopes.len();
                    for i in 0..scope_stack.stack.len() {
                        let scope = &scope_stack.stack[i];
                        for j in 0..scope.len() {
                            out_doc.scopes.push(scope[j]);
                        }
                    }
                    let new_node = LiveNode {
                        token_id: node.token_id,
                        id: node.id,
                        value: LiveValue::Fn {
                            token_start,
                            token_count,
                            scope_start: new_scope_start as u32,
                            scope_count: (out_doc.scopes.len() - new_scope_start) as u16
                        }
                    };
                    write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                },
                LiveValue::Use {crate_module} => { // import things on the scope from Use
                    let crate_module = in_doc.fetch_crate_module(crate_module, in_crate);
                    let other_doc = expanded.get(&crate_module).unwrap();
                    
                    match node.id.to_type() {
                        IdType::Empty => { // its a wildcard
                            let nodes = &other_doc.nodes[0];
                            for i in 0..nodes.len() {
                                let id = nodes[i].id;
                                scope_stack.stack[out_level].push(LiveScopeItem {
                                    id,
                                    target: LiveScopeTarget::Use {
                                        crate_module,
                                        node_ptr: LiveNodePtr {level: 0, index: i}
                                    }
                                });
                            }
                        },
                        IdType::Single(_) => {
                            let nodes = &other_doc.nodes[0];
                            let mut found = false;
                            for i in 0..nodes.len() {
                                if nodes[i].id == node.id { // found it
                                    scope_stack.stack[out_level].push(LiveScopeItem {
                                        id: node.id,
                                        target: LiveScopeTarget::Use {
                                            crate_module,
                                            node_ptr: LiveNodePtr {level: 0, index: i}
                                        }
                                    });
                                    found = true;
                                    break;
                                }
                            }
                            if !found {
                                errors.push(LiveError {
                                    span: in_doc.token_id_to_span(node.token_id),
                                    message: format!("Cannot find import {}", IdFmt::col(&in_doc.multi_ids, node.id))
                                });
                            }
                        }
                        IdType::Multi {index, count} => {
                            // lets validate if it exists!
                            let mut node_start = 0 as usize;
                            let mut node_count = other_doc.nodes[0].len();
                            for level in 0..count {
                                let id = in_doc.multi_ids[level + index];
                                match id.to_type() {
                                    IdType::Empty => { // wildcard
                                        if level != count - 1 {
                                            panic!()
                                        }
                                        for i in 0..node_count {
                                            let other_node = &other_doc.nodes[level][i + node_start];
                                            scope_stack.stack[out_level].push(LiveScopeItem {
                                                id: other_node.id,
                                                target: LiveScopeTarget::Use {
                                                    crate_module,
                                                    node_ptr: LiveNodePtr {level, index: i}
                                                }
                                            });
                                        }
                                    }
                                    IdType::Single(_) => { // a node
                                        for i in 0..node_count {
                                            let other_node = &other_doc.nodes[level][i + node_start];
                                            if level == count - 1 {
                                                if id == other_node.id {
                                                    scope_stack.stack[out_level].push(LiveScopeItem {
                                                        id: other_node.id,
                                                        target: LiveScopeTarget::Use {
                                                            crate_module,
                                                            node_ptr: LiveNodePtr {level, index: i}
                                                        }
                                                    });
                                                }
                                                else {
                                                    errors.push(LiveError {
                                                        span: in_doc.token_id_to_span(node.token_id),
                                                        message: format!("Use path not found {}", IdFmt::col(&in_doc.multi_ids, node.id))
                                                    });
                                                }
                                            }
                                            if id == other_node.id {
                                                match other_node.value {
                                                    LiveValue::Class {node_start: ns, node_count: nc, ..} => {
                                                        node_start = ns as usize;
                                                        node_count = nc as usize;
                                                    },
                                                    _ => {
                                                        if level != count - 1 {
                                                            errors.push(LiveError {
                                                                span: in_doc.token_id_to_span(node.token_id),
                                                                message: format!("Use path not found {}", IdFmt::col(&in_doc.multi_ids, node.id))
                                                            });
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    _ => panic!()
                                }
                            }
                        }
                        _ => {
                            errors.push(LiveError {
                                span: in_doc.token_id_to_span(node.token_id),
                                message: format!("Node type invalid {}", IdFmt::col(&in_doc.multi_ids, node.id))
                            });
                        }
                    }
                }
                LiveValue::Class {class, node_start, node_count} => {
                    //let out_index = out_doc.get_level_len(out_level);
                    scope_stack.stack.push(Vec::new());
                    // if our id is a multi-id, write the clone at the correct level
                    let shifted_out_level = if node.id.is_multi() {
                        let (_start, len) = node.id.get_multi();
                        out_level + (len - 1)
                    }
                    else {
                        out_level
                    };
                    
                    let new_out_start = out_doc.get_level_len(shifted_out_level + 1);
                    
                    // result values of the below scan
                    let mut copy_result = CopyRecurResult::IsClass {class};
                    let mut value_ptr = None;
                    let mut other_crate_module = None;
                    
                    if class.is_multi() {
                        let (id_start, id_count) = class.get_multi();
                        let base = in_doc.multi_ids[id_start];
                        // base id can be Self or a scope target
                        if base == id!(Self) {
                            // lets find our sub id chain on self
                            let out_count = out_doc.get_level_len(out_level) - out_start;
                            match out_doc.scan_for_multi(out_level, out_start, out_count, id_start, id_count, &in_doc.multi_ids, node.token_id) {
                                Ok(found_node) => {
                                    copy_result = copy_recur(scope_stack, None, out_doc, found_node.level, found_node.level, shifted_out_level, found_node.index);
                                    value_ptr = Some(found_node);
                                }
                                Err(err) => {
                                    errors.push(err);
                                    copy_result = CopyRecurResult::Error
                                }
                            }
                        }
                        else if LiveRegistry::is_baseclass(base) {
                            errors.push(LiveError {
                                span: in_doc.token_id_to_span(node.token_id),
                                message: format!("Setting property {} is not an object path", IdFmt::col(&in_doc.multi_ids, base))
                            });
                            copy_result = CopyRecurResult::Error
                        }
                        else {
                            match scope_stack.find_item(base) {
                                Some(LiveScopeTarget::Local {node_ptr}) => {
                                    match &out_doc.nodes[node_ptr.level][node_ptr.index].value {
                                        LiveValue::Class {node_start, node_count, ..} => {
                                            match out_doc.scan_for_multi(node_ptr.level + 1, *node_start as usize, *node_count as usize, id_start, id_count, &in_doc.multi_ids, node.token_id) {
                                                Ok(found_node) => {
                                                    copy_result = copy_recur(scope_stack, None, out_doc, found_node.level, found_node.level, shifted_out_level, found_node.index);
                                                    value_ptr = Some(found_node);
                                                }
                                                Err(err) => {
                                                    errors.push(err);
                                                    copy_result = CopyRecurResult::Error
                                                }
                                            }
                                        }
                                        _ => {
                                            errors.push(LiveError {
                                                span: in_doc.token_id_to_span(node.token_id),
                                                message: format!("Property is not a class {} of {}", base, IdFmt::col(&in_doc.multi_ids, class))
                                            });
                                        }
                                    }
                                }
                                Some(LiveScopeTarget::Use {crate_module, node_ptr}) => {
                                    // pull in a copy from the use import
                                    other_crate_module = Some(crate_module);
                                    let other_doc = expanded.get(&crate_module).unwrap();
                                    match &other_doc.nodes[node_ptr.level][node_ptr.index].value {
                                        LiveValue::Class {node_start, node_count, ..} => {
                                            match other_doc.scan_for_multi(node_ptr.level + 1, *node_start as usize, *node_count as usize, id_start, id_count, &in_doc.multi_ids, node.token_id) {
                                                Ok(found_node) => {
                                                    copy_result = copy_recur(scope_stack, Some((other_doc, crate_module)), out_doc, found_node.level, found_node.level, shifted_out_level, found_node.index);
                                                    //println!("COPYING {:?}", copy_result);
                                                    value_ptr = Some(found_node);
                                                }
                                                Err(err) => {
                                                    errors.push(err);
                                                    copy_result = CopyRecurResult::Error
                                                }
                                            }
                                        }
                                        _ => {
                                            errors.push(LiveError {
                                                span: in_doc.token_id_to_span(node.token_id),
                                                message: format!("Property is not a class {} of {}", base, IdFmt::col(&in_doc.multi_ids, class))
                                            });
                                        }
                                    }
                                }
                                None => { // scope item not found, error
                                    errors.push(LiveError {
                                        span: in_doc.token_id_to_span(node.token_id),
                                        message: format!("Cannot find item on scope: {} of {}", base, IdFmt::col(&in_doc.multi_ids, class))
                                    });
                                    copy_result = CopyRecurResult::Error
                                }
                            }
                        }
                    }
                    else if class == id!(Self) {
                        // recursively clone self
                        for i in out_start..out_doc.get_level_len(out_level) {
                            copy_recur(scope_stack, None, out_doc, 0, out_level, shifted_out_level + 1, i);
                        }
                    }
                    else if !LiveRegistry::is_baseclass(class) {
                        match scope_stack.find_item(class) {
                            Some(LiveScopeTarget::Local {node_ptr}) => {
                                copy_result = copy_recur(scope_stack, None, out_doc, node_ptr.level, node_ptr.level, shifted_out_level, node_ptr.index);
                                value_ptr = Some(node_ptr);
                            }
                            Some(LiveScopeTarget::Use {crate_module, node_ptr}) => {
                                other_crate_module = Some(crate_module);
                                let other_doc = expanded.get(&crate_module).unwrap();
                                copy_result = copy_recur(scope_stack, Some((other_doc, crate_module)), out_doc, node_ptr.level, node_ptr.level, shifted_out_level, node_ptr.index);
                                value_ptr = Some(node_ptr);
                            }
                            None => { // scope item not found, error
                                errors.push(LiveError {
                                    span: in_doc.token_id_to_span(node.token_id),
                                    message: format!("Cannot find item on scope: {}", class)
                                });
                                return
                            }
                        }
                    }
                    
                    if let CopyRecurResult::IsClass {..} = copy_result {}
                    else if node_count >0 {
                        errors.push(LiveError {
                            span: in_doc.token_id_to_span(node.token_id),
                            message: format!("Cannot override items in non-class: {}", IdFmt::col(&in_doc.multi_ids, class))
                        });
                        return
                    }
                    
                    match copy_result {
                        CopyRecurResult::IsValue => {
                            scope_stack.stack.pop();
                            if let Some(value_ptr) = value_ptr {
                                let mut new_node = if let Some(crate_module) = other_crate_module {
                                    let other_doc = expanded.get(&crate_module).unwrap();
                                    other_doc.nodes[value_ptr.level][value_ptr.index]
                                }
                                else {
                                    out_doc.nodes[value_ptr.level][value_ptr.index]
                                };
                                new_node.id = node.id;
                                write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                            }
                        },
                        CopyRecurResult::IsString {string_start, string_count} => {
                            scope_stack.stack.pop();
                            let new_node = LiveNode {
                                token_id: node.token_id,
                                id: node.id,
                                value: LiveValue::String {
                                    string_start,
                                    string_count
                                }
                            };
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        }
                        CopyRecurResult::IsFn {token_start, token_count, scope_start, scope_count} => {
                            scope_stack.stack.pop();
                            let new_node = LiveNode {
                                token_id: node.token_id,
                                id: node.id,
                                value: LiveValue::Fn {
                                    token_start,
                                    token_count,
                                    scope_start,
                                    scope_count
                                }
                            };
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        } 
                        CopyRecurResult::IsCall {target} => {
                            scope_stack.stack.pop();
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            let new_node = LiveNode {
                                token_id: node.token_id,
                                id: node.id,
                                value: LiveValue::Call {
                                    target,
                                    node_start: new_out_start as u32,
                                    node_count: new_out_count as u16
                                }
                            };
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        },
                        CopyRecurResult::IsArray => {
                            scope_stack.stack.pop();
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            let new_node = LiveNode {
                                token_id: node.token_id,
                                id: node.id,
                                value: LiveValue::Array {
                                    node_start: new_out_start as u32,
                                    node_count: new_out_count as u32
                                }
                            };
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        },
                        CopyRecurResult::IsObject => {
                            scope_stack.stack.pop();
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            let new_node = LiveNode {
                                token_id: node.token_id,
                                id: node.id,
                                value: LiveValue::Object {
                                    node_start: new_out_start as u32,
                                    node_count: new_out_count as u32
                                }
                            };
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        },
                        CopyRecurResult::IsClass {class} => {
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            for i in 0..node_count {
                                walk_node(expanded, in_crate, errors, scope_stack, in_doc, out_doc, in_level + 1, shifted_out_level + 1, i as usize + node_start as usize, new_out_start, new_out_count);
                            }
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            
                            let new_node = LiveNode {
                                token_id: node.token_id,
                                id: node.id,
                                value: LiveValue::Class {
                                    class,
                                    node_start: new_out_start as u32,
                                    node_count: new_out_count as u16
                                }
                            };
                            scope_stack.stack.pop();
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        }
                        CopyRecurResult::Noop | CopyRecurResult::Error =>{
                            scope_stack.stack.pop();
                        }
                    }
                }
            }
        }
        
        for (crate_module, token_id) in &self.dep_order {
            if self.expanded.get(crate_module).is_some() {
                continue;
            }
            let file_id = if let Some(file_id) = self.crate_module_to_file_id.get(crate_module) {
                file_id
            }
            else {
                // ok so we have a token_id. now what.
                errors.push(LiveError {
                    span: self.token_id_to_span(*token_id),
                    message: format!("Cannot find dependency: {}::{}", crate_module.0, crate_module.1)
                });
                continue
            };
            let live_file = &self.live_files[file_id.0 as usize];
            let in_doc = &live_file.document;
            
            let mut out_doc = LiveDocument::new();
            out_doc.strings = in_doc.strings.clone();
            out_doc.tokens = in_doc.tokens.clone();
            out_doc.multi_ids = in_doc.multi_ids.clone();
            let mut scope_stack = ScopeStack {
                stack: vec![Vec::new()]
            };
            let len = in_doc.nodes[0].len();
            
            for i in 0..len {
                walk_node(&self.expanded, crate_module.0, errors, &mut scope_stack, in_doc, &mut out_doc, 0, 0, i, 0, 0);
            }
            
            println!("{}", out_doc);
            
            self.expanded.insert(*crate_module, out_doc);
        }
    }
}
