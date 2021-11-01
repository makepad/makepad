use crate::id::{Id, IdPack, IdUnpack, IdFmt};
use crate::liveerror::{LiveError, LiveErrorOrigin};
use makepad_id_macros::*;
use crate::livedocument::LiveDocument;
use crate::livenode::LiveNode;
use crate::livenode::LiveValue;
use crate::id::FileId;
use crate::id::LocalPtr;
use crate::id::LivePtr;
use crate::token::TokenId;
use crate::id::ModulePath;
use std::collections::HashMap;
use crate::livedocument::LiveScopeTarget;
use crate::livedocument::LiveScopeItem;


pub struct LiveExpander<'a> {
    pub module_path_to_file_id: &'a HashMap<ModulePath, FileId>,
    pub expanded: &'a Vec<LiveDocument >,
    pub in_crate: Id,
    pub in_file_id: FileId,
    pub errors: &'a mut Vec<LiveError>,
    pub scope_stack: &'a mut ScopeStack,
}

impl<'a> LiveExpander<'a> {
    pub fn is_baseclass(id: IdPack) -> bool {
        id == id_pack!(Component)
            || id == id_pack!(Enum)
            || id == id_pack!(Variant)
            || id == id_pack!(Struct)
            || id == id_pack!(Namespace)
            || id == id_pack!(DrawShader)
            || id == id_pack!(Geometry)
    }
    
    fn clone_scope(
        in_doc: &LiveDocument,
        out_doc: &mut LiveDocument,
        scope_start: usize,
        scope_count: usize,
        in_file_id: FileId
    ) {
        for i in 0..scope_count {
            let item = &in_doc.scopes[i + scope_start];
            // if item is local, it is now 'remote'.
            match item.target {
                LiveScopeTarget::LocalPtr(local_ptr) => {
                    out_doc.scopes.push(LiveScopeItem {
                        id: item.id,
                        target: LiveScopeTarget::LivePtr(
                            LivePtr {
                                file_id: in_file_id,
                                local_ptr
                            }
                        )
                    });
                },
                LiveScopeTarget::LivePtr {..} => {
                    out_doc.scopes.push(*item);
                }
            }
        }
    }
    
    
    fn copy_recur(
        &mut self,
        in_doc: Option<(&LiveDocument, FileId)>,
        in_level: usize,
        in_index: usize,
        out_doc: &mut LiveDocument,
        out_level: usize,
        skip_level_id: Id,
        skip_level: usize,
    ) -> CopyRecurResult {
        let node = if let Some((in_doc, _)) = in_doc {
            in_doc.nodes[in_level][in_index]
        }
        else {
            out_doc.nodes[in_level][in_index]
        };
        let node_id = if skip_level == in_level {
            skip_level_id
        }
        else {
            node.id
        };
        
        match node.value {
            LiveValue::Call {target, node_start, node_count} => {
                let out_start = out_doc.get_level_len(out_level + 1);
                for i in 0..node_count {
                    self.copy_recur(in_doc, in_level + 1, i as usize + node_start as usize, out_doc, out_level + 1, skip_level_id, skip_level);
                }
                
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::Call {
                        target: target,
                        node_start: out_start as u32,
                        node_count: node_count
                    }
                });
                return CopyRecurResult::Noop
            },
            LiveValue::Array {node_start, node_count} => {
                let out_start = out_doc.get_level_len(out_level + 1);
                for i in 0..node_count {
                    self.copy_recur(in_doc, in_level + 1, i as usize + node_start as usize, out_doc, out_level + 1, skip_level_id, skip_level);
                }
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::Array {
                        node_start: out_start as u32,
                        node_count: node_count
                    }
                });
                return CopyRecurResult::Noop
            },
            LiveValue::ClassOverride {node_start, node_count} => {
                // COPY RECUR.. SHOULD NEVER HAPPEN?
                println!("copy_recur of Override SHOULD NEVER HAPPEN");
                let out_start = out_doc.get_level_len(out_level + 1);
                for i in 0..node_count {
                    self.copy_recur(in_doc, in_level + 1, i as usize + node_start as usize, out_doc, out_level + 1, skip_level_id, skip_level);
                }
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::ClassOverride {
                        node_start: out_start as u32,
                        node_count: node_count
                    }
                });
                return CopyRecurResult::Noop
            },
            LiveValue::Use {..} => { // no need to output there.
            }
            LiveValue::Class {class, node_start, node_count} => {
                if class == id_pack!(Self) {
                    return CopyRecurResult::Noop
                }
                let out_start = out_doc.get_level_len(out_level + 1);
                for i in 0..node_count {
                    self.copy_recur(in_doc, in_level + 1, i as usize + node_start as usize, out_doc, out_level + 1, skip_level_id, skip_level);
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
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::String {
                        string_start: new_string_start as u32,
                        string_count
                    }
                });
                return CopyRecurResult::Noop
            }
            LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                let (new_token_start, new_scope_start) = if let Some((in_doc, in_file_id)) = in_doc { // copy the string if its from another doc
                    let nts = out_doc.tokens.len();
                    let nss = out_doc.scopes.len();
                    for i in 0..(token_count as usize) {
                        out_doc.tokens.push(in_doc.tokens[i + token_start as usize]);
                    }
                    Self::clone_scope(in_doc, out_doc, scope_start as usize, scope_count as usize, in_file_id);
                    (nts as u32, nss as u32)
                }
                else {
                    (token_start, scope_start)
                };
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::Fn {
                        token_start: new_token_start,
                        scope_start: new_scope_start,
                        token_count,
                        scope_count
                    }
                });
                return CopyRecurResult::Noop
            }
            LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                let (new_token_start, new_scope_start) = if let Some((in_doc, in_file_id)) = in_doc { // copy the string if its from another doc
                    let nts = out_doc.tokens.len();
                    let nss = out_doc.scopes.len();
                    for i in 0..(token_count as usize) {
                        out_doc.tokens.push(in_doc.tokens[i + token_start as usize]);
                    }
                    Self::clone_scope(in_doc, out_doc, scope_start as usize, scope_count as usize, in_file_id);
                    (nts as u32, nss as u32)
                }
                else {
                    (token_start, scope_start)
                };
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::VarDef {
                        token_start: new_token_start,
                        scope_start: new_scope_start,
                        token_count,
                        scope_count
                    }
                });
                return CopyRecurResult::Noop
            }
            _ => {
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: node.value
                });
                return CopyRecurResult::Noop
            }
        }
        return CopyRecurResult::Noop
    }
    
    fn write_or_add_node(
        &mut self,
        out_doc: &mut LiveDocument,
        out_level: usize,
        out_start: usize,
        out_count: usize,
        in_doc: &LiveDocument,
        in_node: &LiveNode
    ) {
        if in_node.id == Id(0) {
            let nodes = &mut out_doc.nodes[out_level];
            let index = nodes.len();
            nodes.push(*in_node);
            return
        }
        else {
            let nodes = &mut out_doc.nodes[out_level];
            for i in 0..out_count {
                if nodes[i].id == in_node.id { // overwrite and exit
                    // lets error if the overwrite value type changed.
                    if nodes[i].value.get_type_nr() != in_node.value.get_type_nr() {
                        if nodes[i].value.is_var_def() { // we can replace a vardef with something else
                            continue;
                        }
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.token_id),
                            message: format!("Cannot inherit with different node type {}", in_node.id)
                        });
                        return;
                    }
                    nodes[i] = *in_node;
                    return
                }
            }
            // not found
            let index = nodes.len();
            nodes.push(*in_node);
            if self.scope_stack.stack.len() - 1 == out_level {
                self.scope_stack.stack[out_level].push(LiveScopeItem {
                    id: in_node.id,
                    target: LiveScopeTarget::LocalPtr(LocalPtr {level: out_level, index: index})
                });
            }
        }
    }
    
    fn resolve_id(
        &self,
        resolve_id: IdPack,
        token_id: TokenId,
        in_doc: &LiveDocument,
        out_doc: &mut LiveDocument,
        out_level: usize,
        out_start: usize,
    ) -> Result<(Option<FileId>, LocalPtr), LiveError> {
        match resolve_id.unpack() {
            IdUnpack::Multi {index: id_start, count: id_count} => {
                let base = in_doc.multi_ids[id_start];
                // base id can be Self or a scope target
                if base == id!(Self) {
                    // lets find our sub id chain on self
                    let out_count = out_doc.get_level_len(out_level) - out_start;
                    match out_doc.scan_for_multi_for_expand(out_level, out_start, out_count, id_start, id_count, &in_doc.multi_ids,) {
                        Ok(found_node) => {
                            return Ok((None, found_node))
                        }
                        Err(message) => {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: out_doc.token_id_to_span(token_id),
                                message
                            });
                        }
                    }
                }
                else if Self::is_baseclass(IdPack::single(base)) {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: in_doc.token_id_to_span(token_id),
                        message: format!("Cannot use baseclass {}", base)
                    });
                }
                else {
                    match self.scope_stack.find_item(base) {
                        Some(LiveScopeTarget::LocalPtr(node_ptr)) => {
                            match &out_doc.nodes[node_ptr.level][node_ptr.index].value {
                                LiveValue::Class {node_start, node_count, ..} => {
                                    match out_doc.scan_for_multi_for_expand(node_ptr.level + 1, *node_start as usize, *node_count as usize, id_start, id_count, &in_doc.multi_ids) {
                                        Ok(found_node) => {
                                            return Ok((None, found_node))
                                        }
                                        Err(message) => {
                                            return Err(LiveError {
                                                origin: live_error_origin!(),
                                                span: out_doc.token_id_to_span(token_id),
                                                message
                                            });
                                        }
                                    }
                                }
                                _ => {
                                    return Err(LiveError {
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(token_id),
                                        message: format!("Property is not a class {} of {}", base, IdFmt::col(&in_doc.multi_ids, resolve_id))
                                    });
                                }
                            }
                        }
                        Some(LiveScopeTarget::LivePtr(live_ptr)) => {
                            let other_doc = &self.expanded[live_ptr.file_id.to_index()];
                            match &other_doc.nodes[live_ptr.local_ptr.level][live_ptr.local_ptr.index].value {
                                LiveValue::Class {node_start, node_count, ..} => {
                                    match other_doc.scan_for_multi_for_expand(live_ptr.local_ptr.level + 1, *node_start as usize, *node_count as usize, id_start, id_count, &in_doc.multi_ids) {
                                        Ok(found_node) => {
                                            return Ok((Some(live_ptr.file_id), found_node))
                                        }
                                        Err(message) => {
                                            return Err(LiveError {
                                                origin: live_error_origin!(),
                                                span: out_doc.token_id_to_span(token_id),
                                                message
                                            });
                                        }
                                    }
                                }
                                _ => {
                                    return Err(LiveError {
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(token_id),
                                        message: format!("Property is not a class {} of {}", base, IdFmt::col(&in_doc.multi_ids, resolve_id))
                                    });
                                }
                            }
                        }
                        None => { // scope item not found, error
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: in_doc.token_id_to_span(token_id),
                                message: format!("Cannot find item on scope: {} of {}", base, IdFmt::col(&in_doc.multi_ids, resolve_id))
                            });
                        }
                    }
                }
            }
            IdUnpack::Single(id) if !Self::is_baseclass(IdPack::single(id)) => {
                match self.scope_stack.find_item(id) {
                    Some(LiveScopeTarget::LocalPtr(local_ptr)) => {
                        return Ok((None, local_ptr));
                    }
                    Some(LiveScopeTarget::LivePtr(live_ptr)) => {
                        return Ok((Some(live_ptr.file_id), live_ptr.local_ptr));
                    }
                    _ => {}
                }
            }
            _ => ()
        }
        return Err(LiveError {
            origin: live_error_origin!(),
            span: in_doc.token_id_to_span(token_id),
            message: format!("Cannot find item on scope: {}", resolve_id)
        });
    }
    
    pub fn walk_node(
        &mut self,
        in_doc: &LiveDocument,
        in_level: usize,
        in_node_index: usize,
        out_doc: &mut LiveDocument,
        out_level: usize,
        out_start: usize,
        out_count: usize
    ) {
        let node = &in_doc.nodes[in_level][in_node_index];
        
        //let (row,col) = byte_to_row_col(node.span.start(), &ld.source);
        //let _ = write!(f, "/*{},{} {}*/", row+1, col, node.span.len());
        match node.value {
            LiveValue::String {..} => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Bool(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Int(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Float(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Color(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Vec2(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Vec3(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::LiveType(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::IdPack(id_value) => {
                // lets resolve ID
                let out_index = out_doc.get_level_len(out_level);
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node);
                if id_value != id_pack!(Self) && !Self::is_baseclass(id_value) {
                    let result = self.resolve_id(
                        id_value,
                        node.token_id,
                        in_doc,
                        out_doc,
                        out_level,
                        out_start,
                    );
                    match result {
                        Ok((None, found_node)) => {
                            let new_id = IdPack::node_ptr(self.in_file_id, found_node);
                            let written_node = &mut out_doc.nodes[out_level][out_index];
                            if let LiveValue::IdPack(id) = &mut written_node.value {
                                *id = new_id;
                            }
                        }
                        Ok((Some(found_file_id), found_node)) => {
                            let new_id = IdPack::node_ptr(found_file_id, found_node);
                            let written_node = &mut out_doc.nodes[out_level][out_index];
                            if let LiveValue::IdPack(id) = &mut written_node.value {
                                *id = new_id;
                            }
                        }
                        Err(err) => {
                            self.errors.push(err);
                            return
                        }
                    }
                }
                
            }
            LiveValue::Call {target, node_start, node_count} => {
                let new_node_start = out_doc.get_level_len(out_level + 1);
                for i in 0..node_count {
                    self.walk_node(
                        in_doc,
                        in_level + 1,
                        i as usize + node_start as usize,
                        out_doc,
                        out_level + 1,
                        out_start,
                        0
                    );
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
                let out_index = out_doc.get_level_len(out_level);
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
                if target != id_pack!(Self) && !Self::is_baseclass(target) {
                    let result = self.resolve_id(
                        target,
                        node.token_id,
                        in_doc,
                        out_doc,
                        out_level,
                        out_start,
                    );
                    match result {
                        Ok((None, found_node)) => {
                            // found node has to be a call too
                            let f_n = &out_doc.nodes[found_node.level][found_node.index];
                            if let LiveValue::Call {..} = f_n.value {}
                            else {
                                self.errors.push(LiveError {
                                    origin: live_error_origin!(),
                                    span: in_doc.token_id_to_span(node.token_id),
                                    message: format!("Target not a call {}", IdFmt::col(&in_doc.multi_ids, target))
                                });
                                return
                            }
                            let new_id = IdPack::node_ptr(self.in_file_id, found_node);
                            let written_node = &mut out_doc.nodes[out_level][out_index];
                            if let LiveValue::Call {target, ..} = &mut written_node.value {
                                *target = new_id;
                            }
                        }
                        Ok((Some(found_file_id), found_node)) => {
                            let f_n = &self.expanded[found_file_id.to_index()].nodes[found_node.level][found_node.index];
                            if let LiveValue::Call {..} = f_n.value {}
                            else {
                                self.errors.push(LiveError {
                                    origin: live_error_origin!(),
                                    span: in_doc.token_id_to_span(node.token_id),
                                    message: format!("Target not a call {}", IdFmt::col(&in_doc.multi_ids, target))
                                });
                                return
                            }
                            let new_id = IdPack::node_ptr(found_file_id, found_node);
                            let written_node = &mut out_doc.nodes[out_level][out_index];
                            if let LiveValue::Call {target, ..} = &mut written_node.value {
                                *target = new_id;
                            }
                            // store pointer
                        }
                        Err(err) => {
                            self.errors.push(err);
                            return
                        }
                    }
                }
            },
            LiveValue::Array {node_start, node_count} => { // normal array
                
                let new_node_start = out_doc.get_level_len(out_level + 1);
                for i in 0..node_count {
                    self.walk_node(
                        in_doc,
                        in_level + 1,
                        i as usize + node_start as usize,
                        out_doc,
                        out_level + 1,
                        out_start,
                        0
                    );
                }
                let new_node = LiveNode {
                    token_id: node.token_id,
                    id: node.id,
                    value: LiveValue::Array {
                        node_start: new_node_start as u32,
                        node_count: node_count as u32
                    }
                };
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
            },
            LiveValue::ClassOverride {node_start, node_count} => {
                // OK SO what do we do.
                // first off we find the parent node we need to extend.
                
                // ok we are in the indoc.
                // in the out_doc we should already have the property.
                // now what we do is walk all our children and the outdoc property
                let new_node_start = out_doc.get_level_len(out_level + 1);
                
                //for i in 0..node_count {
                //    walk_node(expanded, module_path_to_file_id, in_crate, in_file_id, errors, scope_stack, in_doc, out_doc, in_level + 1, out_level + 1, i as usize + node_start as usize, out_start, 0);
                //}
                
                let new_node = LiveNode {
                    token_id: node.token_id,
                    id: node.id,
                    value: LiveValue::ClassOverride {
                        node_start: new_node_start as u32,
                        node_count: node_count as u32
                    }
                };
                // println!("{} {}", out_start, out_doc.get_level_len(out_level))
                // we dont know yet yet
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
            },
            LiveValue::Fn {token_start, token_count, ..} => {
                // we should store the scopestack here so the shader compiler can find symbols.
                let new_scope_start = out_doc.scopes.len();
                for i in 0..self.scope_stack.stack.len() {
                    let scope = &self.scope_stack.stack[i];
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
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
            },
            LiveValue::VarDef {token_start, token_count, ..} => {
                // we should store the scopestack here so the shader compiler can find symbols.
                let new_scope_start = out_doc.scopes.len();
                for i in 0..self.scope_stack.stack.len() {
                    let scope = &self.scope_stack.stack[i];
                    for j in 0..scope.len() {
                        out_doc.scopes.push(scope[j]);
                    }
                }
                let new_node = LiveNode {
                    token_id: node.token_id,
                    id: node.id,
                    value: LiveValue::VarDef {
                        token_start,
                        token_count,
                        scope_start: new_scope_start as u32,
                        scope_count: (out_doc.scopes.len() - new_scope_start) as u16
                    }
                };
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
            },
            LiveValue::Const {token_start, token_count, ..} => {
                // we should store the scopestack here so the shader compiler can find symbols.
                let new_scope_start = out_doc.scopes.len();
                for i in 0..self.scope_stack.stack.len() {
                    let scope = &self.scope_stack.stack[i];
                    for j in 0..scope.len() {
                        out_doc.scopes.push(scope[j]);
                    }
                }
                let new_node = LiveNode {
                    token_id: node.token_id,
                    id: node.id,
                    value: LiveValue::Const {
                        token_start,
                        token_count,
                        scope_start: new_scope_start as u32,
                        scope_count: (out_doc.scopes.len() - new_scope_start) as u16
                    }
                };
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
            },
            
            LiveValue::Use {use_ids} => { // import things on the scope from Use
                let module_path = in_doc.use_ids_to_module_path(use_ids, self.in_crate);
                let file_id = if let Some(file_id) = self.module_path_to_file_id.get(&module_path) {
                    file_id
                }
                else {
                    self.errors.push(LiveError {
                        origin: live_error_origin!(),
                        span: in_doc.token_id_to_span(node.token_id),
                        message: format!("Cannot find import {}", IdFmt::col(&in_doc.multi_ids, use_ids))
                    });
                    return
                };
                let other_doc = &self.expanded[file_id.to_index()];
                
                match use_ids.unpack() {
                    IdUnpack::Multi {index, count} => {
                        let shifted_count = count - 2;
                        // lets validate if it exists!
                        let mut node_start = 0 as usize;
                        let mut node_count = other_doc.nodes[0].len();
                        for level in 0..shifted_count {
                            let id = in_doc.multi_ids[level + 2 + index];
                            if id.is_empty() { // its a *
                                if level != count - 1 { // cant appear except at end
                                    panic!()
                                }
                                for i in 0..node_count {
                                    let other_node = &other_doc.nodes[level][i + node_start];
                                    self.scope_stack.stack[out_level].push(LiveScopeItem {
                                        id: other_node.id,
                                        target: LiveScopeTarget::LivePtr(
                                            LivePtr {
                                                file_id: *file_id,
                                                local_ptr: LocalPtr {level, index: i + node_start}
                                            }
                                        )
                                    });
                                }
                            }
                            else {
                                let mut found = false;
                                for i in 0..node_count {
                                    let other_node = &other_doc.nodes[level][i + node_start];
                                    if level == count - 1 { // last level
                                        if id == other_node.id {
                                            self.scope_stack.stack[out_level].push(LiveScopeItem {
                                                id: id,
                                                target: LiveScopeTarget::LivePtr(
                                                    LivePtr {
                                                        file_id: *file_id,
                                                        local_ptr: LocalPtr {level, index: i + node_start}
                                                    }
                                                )
                                            });
                                            found = true;
                                            break;
                                        }
                                    }
                                    if id == other_node.id {
                                        match other_node.value {
                                            LiveValue::Class {node_start: ns, node_count: nc, ..} => {
                                                node_start = ns as usize;
                                                node_count = nc as usize;
                                                found = true;
                                                break;
                                            },
                                            _ => {
                                                break;
                                            }
                                        }
                                    }
                                }
                                if !found {
                                    self.errors.push(LiveError {
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(node.token_id),
                                        message: format!("Use path not found {}", IdFmt::col(&in_doc.multi_ids, use_ids))
                                    });
                                }
                            }
                        }
                    }
                    _ => panic!()
                }
            }
            LiveValue::Class {class, node_start, node_count} => {
                //let out_index = out_doc.get_level_len(out_level);
                self.scope_stack.stack.push(Vec::new());
                let new_out_start = out_doc.get_level_len(out_level + 1);
                
                // result values of the below scan
                let mut copy_result = CopyRecurResult::IsClass {class};
                let mut value_ptr = None;
                let mut other_file_id = None;
                
                if class == id_pack!(Self) {
                    // recursively clone self
                    for i in out_start..out_doc.get_level_len(out_level) {
                        self.copy_recur(None, out_level, i, out_doc, out_level + 1, node.id, 0, );
                    }
                }
                else if !Self::is_baseclass(class) {
                    let result = self.resolve_id(
                        class,
                        node.token_id,
                        in_doc,
                        out_doc,
                        out_level,
                        out_start,
                    );
                    match result {
                        Ok((None, found_node)) => {
                            copy_result = self.copy_recur(None, found_node.level, found_node.index,  out_doc, out_level, node.id, found_node.level);
                            value_ptr = Some(found_node);
                        }
                        Ok((Some(found_file_id), found_node)) => {
                            let other_doc = &self.expanded[found_file_id.to_index()];
                            other_file_id = Some(found_file_id);
                            copy_result = self.copy_recur(Some((other_doc, found_file_id)), found_node.level, found_node.index, out_doc, out_level, node.id, found_node.level);
                            value_ptr = Some(found_node);
                        }
                        Err(err) => {
                            self.errors.push(err);
                            return
                        }
                    }
                }
                
                if let CopyRecurResult::IsClass {..} = copy_result {}
                else if node_count >0 {
                    self.errors.push(LiveError {
                        origin: live_error_origin!(),
                        span: in_doc.token_id_to_span(node.token_id),
                        message: format!("Cannot override items in non-class: {}", IdFmt::col(&in_doc.multi_ids, class))
                    });
                    return
                }
                
                match copy_result {
                    CopyRecurResult::IsClass {class} => {
                        
                        let new_class_id = if let Some(other_file_id) = other_file_id {
                            if let Some(value_ptr) = value_ptr {
                                IdPack::node_ptr(other_file_id, value_ptr)
                            }
                            else {
                                class
                            }
                        }
                        else {
                            if let Some(value_ptr) = value_ptr {
                                IdPack::node_ptr(self.in_file_id, value_ptr)
                            }
                            else {
                                class
                            }
                        };
                        
                        let new_out_count = out_doc.get_level_len(out_level + 1) - new_out_start;
                        for i in 0..node_count {
                            self.walk_node(
                                in_doc,
                                in_level + 1,
                                i as usize + node_start as usize,
                                out_doc,
                                out_level + 1,
                                new_out_start,
                                new_out_count
                            );
                        }
                        let new_out_count = out_doc.get_level_len(out_level + 1) - new_out_start;
                        
                        let new_node = LiveNode {
                            token_id: node.token_id,
                            id: node.id,
                            value: LiveValue::Class {
                                class: new_class_id,
                                node_start: new_out_start as u32,
                                node_count: new_out_count as u16
                            }
                        };
                        self.scope_stack.stack.pop();
                        self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
                    }
                    CopyRecurResult::Noop | CopyRecurResult::Error => {
                        self.scope_stack.stack.pop();
                    }
                }
            }
        }
    }
}

pub struct ScopeStack {
    pub stack: Vec<Vec<LiveScopeItem >>
}

impl ScopeStack {
    fn find_item(&self, id: Id) -> Option<LiveScopeTarget> {
        for items in self.stack.iter().rev() {
            for item in items.iter().rev() {
                if item.id == id {
                    return Some(item.target)
                }
            }
        }
        return None
    }
}

#[derive(Debug)]
pub enum CopyRecurResult {
    IsClass {class: IdPack},
    Noop,
    Error
}