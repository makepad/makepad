use crate::id::Id;
use crate::liveerror::{LiveError}; //, LiveErrorOrigin};
use makepad_id_macros::*;
use crate::livedocument::LiveDocument;
//use crate::livenode::LiveNode;
use crate::livenode::LiveValue;
use crate::livenode::LiveNodeSlice;
use crate::livenode::LiveNodeVec;
use crate::id::FileId;
use crate::id::LocalPtr;
use crate::id::LivePtr;
//use crate::token::TokenId;
use crate::id::ModulePath;
use std::collections::HashMap;
use crate::livedocument::LiveScopeTarget;
use crate::livedocument::LiveScopeItem;

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


pub struct LiveExpander<'a> {
    pub module_path_to_file_id: &'a HashMap<ModulePath, FileId>,
    pub expanded: &'a Vec<LiveDocument >,
    pub in_crate: Id,
    pub in_file_id: FileId,
    pub errors: &'a mut Vec<LiveError>,
    pub scope_stack: &'a mut ScopeStack,
}

impl<'a> LiveExpander<'a> {
    pub fn is_baseclass(id: Id) -> bool {
        id == id!(Component)
            || id == id!(Enum)
            || id == id!(Struct)
            || id == id!(Namespace)
            || id == id!(DrawShader)
            || id == id!(Geometry)
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
    
    pub fn expand(&mut self, in_doc: &LiveDocument, out_doc: &mut LiveDocument) {
        out_doc.nodes.push(in_doc.nodes[0].clone());
        let mut current_parent = vec![0usize];
        let mut level_overwrite = vec![false];
        let mut in_index = 1;
        
        loop{
            let in_node = &in_doc.nodes[in_index];
            if in_index >= in_doc.nodes.len() -1 {
                break;
            }

            if !in_node.id.is_empty() {
                
                let out_index = match out_doc.nodes.seek_child_by_name(*current_parent.last().unwrap(), in_node.id) {
                    Ok(overwrite)=>{
                        // we can only overwrite if the types are the same 
                        if out_doc.nodes[overwrite].value.variant_id() != in_node.value.variant_id(){
                            println!("TYPE DIFFERENT")
                        }
                        else{ // if we are overwriting, we have to eat the closes
                            out_doc.nodes[overwrite] = in_node.clone();
                        }
                        if in_node.value.is_tree(){
                            level_overwrite.push(true);
                        }
                        overwrite
                    }
                    Err(insert_point)=>{
                        if !in_node.value.is_close() || !level_overwrite.last().unwrap(){
                            out_doc.nodes.insert(insert_point, in_node.clone());
                        }
                        if in_node.value.is_tree(){
                            level_overwrite.push(false);
                        }
                        insert_point
                    }
                };

                self.scope_stack.stack.last_mut().unwrap().push(LiveScopeItem {
                    id: in_node.id,
                    target: LiveScopeTarget::LocalPtr(LocalPtr(out_index))
                });

                // ok now match the rest of things
                match &in_node.value {
                    LiveValue::NamedClass {class} => { // ok this one matters
                        if let Some(target) = self.scope_stack.find_item(*class) {
                            // OK now we need to copy target in.
                            match target{
                                LiveScopeTarget::LocalPtr(local_ptr)=>{
                                    out_doc.nodes.clone_children_self(local_ptr.0, Some(out_index+1));
                                }
                                LiveScopeTarget::LivePtr(live_ptr)=>{
                                    let doc = &self.expanded[live_ptr.file_id.to_index()];
                                    out_doc.nodes.clone_children_from(live_ptr.local_ptr.0, Some(out_index+1), &doc.nodes);
                                    // todo fix up the scopestacks for the shaders
                                }
                            }
                        }
                        // else just ignore it
                        self.scope_stack.stack.push(Vec::new());
                        current_parent.push(out_index);
                    }, // subnodes including this one
                    LiveValue::Array => {
                        self.scope_stack.stack.push(Vec::new());
                        current_parent.push(out_index);
                    },
                    LiveValue::TupleEnum {..} => { //base, variant} => {
                        // ok here we kinda have to remove the old ones
                        
                        self.scope_stack.stack.push(Vec::new());
                        current_parent.push(out_index);
                    },
                    LiveValue::NamedEnum {..} => { //{base, variant} => {
                        self.scope_stack.stack.push(Vec::new());
                        current_parent.push(out_index);
                    },
                    LiveValue::BareClass => {
                        self.scope_stack.stack.push(Vec::new());
                        current_parent.push(out_index);
                    }, // subnodes including this one
                    LiveValue::Close => {
                        // we have to scan for our previous insert point / parent
                        current_parent.pop();
                        level_overwrite.pop();
                        self.scope_stack.stack.pop();
                    },
                    _ => {}
                }

            }
            in_index += 1;
        }
        out_doc.nodes.push(in_doc.nodes.last().unwrap().clone());
    }
    
    /*
    fn copy_recur(
        &mut self,
        in_doc: Option<(&LiveDocument, FileId)>,
        in_level: usize,
        in_index: usize,
        out_doc: &mut LiveDocument,
        out_level: usize,
        skip_level_id: Id,
        skip_level: usize,
    ) -> Option<MultiPack> {
        
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
            },
            LiveValue::ClassOverride {..} => {
                // COPY RECUR.. SHOULD NEVER HAPPEN?
                panic!();
                /*
                let out_start = out_doc.get_level_len(out_level + 1);
                for i in 0..node_count {
                    self.copy_recur(in_doc, in_level + 1, i as usize + node_start as usize, out_doc, out_level + 1, skip_level_id, skip_level, node_start);
                }
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::ClassOverride {
                        node_start: out_start as u32,
                        node_count: node_count
                    }
                });*/
            },
            LiveValue::Use {..} => { // no need to output there.
            }
            LiveValue::Class {class, node_start, node_count} => {
                let out_start = out_doc.get_level_len(out_level + 1);
               
                for i in 0..node_count {
                    self.copy_recur(in_doc, in_level + 1, i as usize + node_start as usize, out_doc, out_level + 1, skip_level_id, skip_level);
                }

                let out_count =  out_doc.get_level_len(out_level + 1) - out_start;
                
                if skip_level != in_level {
                    out_doc.push_node(out_level, LiveNode {
                        token_id: node.token_id,
                        id: node.id,
                        value: LiveValue::Class {
                            class: class,
                            node_start: out_start as u32,
                            node_count: out_count as u16
                        }
                    });
                }
                return Some(class);
            },
            LiveValue::String {string_start, string_count} => {
                /*let new_string_start = if let Some((in_doc, _)) = in_doc { // copy the string if its from another doc
                    let nsi = out_doc.strings.len();
                    for i in 0..string_count {
                        out_doc.strings.push(in_doc.strings[(i + string_start) as usize]);
                    }
                    nsi
                } 
                else {
                    string_start as usize
                };*/
                
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::String {
                        string_start,
                        string_count
                    }
                });
            }
            LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                /*let (new_token_start, new_scope_start) = if let Some((in_doc, in_file_id)) = in_doc { // copy the string if its from another doc
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
                };*/
                
                let new_scope_start = if let Some((in_doc, in_file_id)) = in_doc { // copy scope
                    let nss = out_doc.scopes.len();
                    Self::clone_scope(in_doc, out_doc, scope_start as usize, scope_count as usize, in_file_id);
                    nss as u32
                } else{
                    scope_start
                };
                
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::Fn {
                        token_start,
                        scope_start: new_scope_start,
                        token_count,
                        scope_count
                    }
                });
            }
            LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                /*let (new_token_start, new_scope_start) = if let Some((in_doc, in_file_id)) = in_doc { // copy the string if its from another doc
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
                };*/
                let new_scope_start = if let Some((in_doc, in_file_id)) = in_doc { // copy scope
                    let nss = out_doc.scopes.len();
                    Self::clone_scope(in_doc, out_doc, scope_start as usize, scope_count as usize, in_file_id);
                    nss as u32
                } else{
                    scope_start
                };
                
                
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: LiveValue::VarDef {
                        token_start,
                        scope_start: new_scope_start,
                        token_count,
                        scope_count
                    }
                });
            }
            _ => {
                out_doc.push_node(out_level, LiveNode {
                    token_id: node.token_id,
                    id: node_id,
                    value: node.value
                });
            }
        }
        None
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
        //println!("Write or add node {} {} {} {} {:?}", out_level, out_start, out_count, in_node.id, in_node.value);
        
        // OK this function.
        if in_node.id == Id(0) {
            let nodes = &mut out_doc.nodes[out_level];
            nodes.push(*in_node);
            return
        }
        else {
            let nodes = &mut out_doc.nodes[out_level];
            for i in 0..out_count {
                if nodes[i + out_start].id == in_node.id {
                    if nodes[i + out_start].value.get_type_nr() != in_node.value.get_type_nr() {
                        if nodes[i + out_start].value.is_var_def() {
                            continue;
                        }
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.token_id),
                            message: format!("Cannot inherit with different node type {}", in_node.id)
                        });
                        return;
                    }
                    nodes[i + out_start] = *in_node;
                    return
                }
            }
            let index = if nodes.len() == out_start + out_count {
                nodes.push(*in_node);
                nodes.len() - 1
            }
            else {
                for i in 0..out_count {
                    nodes.push(nodes[i + out_start]);
                }
                nodes.push(*in_node);
                nodes.len() - 1
            };
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
        resolve_id: MultiPack,
        token_id: TokenId,
        in_doc: &LiveDocument,
        out_doc: &mut LiveDocument,
        out_level: usize,
        out_start: usize,
    ) -> Result<(Option<FileId>, LocalPtr), LiveError> {
        match resolve_id.unpack() {
            MultiUnpack::MultiId {index: id_start, count: id_count} => {
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
                else if Self::is_baseclass(MultiPack::single_id(base)) {
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
                                        message: format!("Property is not a class {} of {}", base, MultiFmt::new(&in_doc.multi_ids, resolve_id))
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
                                        message: format!("Property is not a class {} of {}", base, MultiFmt::new(&in_doc.multi_ids, resolve_id))
                                    });
                                }
                            }
                        }
                        None => { // scope item not found, error
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: in_doc.token_id_to_span(token_id),
                                message: format!("Cannot find item on scope: {} of {}", base, MultiFmt::new(&in_doc.multi_ids, resolve_id))
                            });
                        }
                    }
                }
            }
            MultiUnpack::SingleId(id) if !Self::is_baseclass(MultiPack::single_id(id)) => {
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
            LiveValue::Int(_) => {
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node);
            },
            LiveValue::Float(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Color(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Vec2(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::Vec3(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::LiveType(_) => self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node),
            LiveValue::MultiPack(pack) => {
                // lets resolve ID
                let out_index = out_doc.get_level_len(out_level);
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, node);
                if pack != MultiPack::single_id(id!(Self)) && !Self::is_baseclass(pack) {
                    let result = self.resolve_id(
                        pack,
                        node.token_id,
                        in_doc,
                        out_doc,
                        out_level,
                        out_start,
                    );
                    match result {
                        Ok((None, found_node)) => {
                            let new_pack = MultiPack::live_ptr(self.in_file_id, found_node);
                            let written_node = &mut out_doc.nodes[out_level][out_index];
                            if let LiveValue::MultiPack(pack) = &mut written_node.value {
                                *pack = new_pack;
                            }
                        }
                        Ok((Some(found_file_id), found_node)) => {
                            let new_pack = MultiPack::live_ptr(found_file_id, found_node);
                            let written_node = &mut out_doc.nodes[out_level][out_index];
                            if let LiveValue::MultiPack(pack) = &mut written_node.value {
                                *pack = new_pack;
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
                if target.as_single_id() != id!(Self) && !Self::is_baseclass(target) {
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
                                    message: format!("Target not a call {}", MultiFmt::new(&in_doc.multi_ids, target))
                                });
                                return
                            }
                            let new_pack = MultiPack::live_ptr(self.in_file_id, found_node);
                            let written_node = &mut out_doc.nodes[out_level][out_index];
                            if let LiveValue::Call {target, ..} = &mut written_node.value {
                                *target = new_pack;
                            }
                        }
                        Ok((Some(found_file_id), found_node)) => {
                            let f_n = &self.expanded[found_file_id.to_index()].nodes[found_node.level][found_node.index];
                            if let LiveValue::Call {..} = f_n.value {}
                            else {
                                self.errors.push(LiveError {
                                    origin: live_error_origin!(),
                                    span: in_doc.token_id_to_span(node.token_id),
                                    message: format!("Target not a call {}", MultiFmt::new(&in_doc.multi_ids, target))
                                });
                                return
                            }
                            let new_pack = MultiPack::live_ptr(found_file_id, found_node);
                            let written_node = &mut out_doc.nodes[out_level][out_index];
                            if let LiveValue::Call {target, ..} = &mut written_node.value {
                                *target = new_pack;
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
                // first off we find the parent node we need to extend.
                for i in 0..out_count {
                    let out_node = &out_doc.nodes[out_level][i + out_start];
                    if out_node.id == node.id {
                        // it HAS to be a class
                        if let LiveValue::Class {node_start:class_node_start, node_count:class_node_count, ..} = out_node.value {

                            let start_len = out_doc.nodes[out_level + 1].len();
                            let is_at_end = start_len == class_node_start as usize + class_node_count as usize;
                            
                            self.scope_stack.stack.push(Vec::new());
                            for i in 0..node_count {
                                self.walk_node(
                                    in_doc,
                                    in_level + 1,
                                    node_start as usize + i as usize,
                                    out_doc,
                                    out_level + 1,
                                    class_node_start as usize,
                                    class_node_count as usize
                                );
                            }
                            self.scope_stack.stack.pop();
                            
                            // something got added
                            let new_len = out_doc.nodes[out_level + 1].len();
                            if start_len != new_len {
                                let mut_out_node = &mut out_doc.nodes[out_level][i + out_start];
                                if let LiveValue::Class {node_start, node_count, ..} = &mut mut_out_node.value {
                                    if is_at_end { // just got extended
                                        *node_count += (new_len - start_len) as u16;
                                    }
                                    else { // we also have to shift node_start
                                        *node_start = start_len as u32;
                                        *node_count = (new_len - start_len) as u16;
                                    }
                                }
                            }
                            
                            return
                        }
                        else{
                            self.errors.push(LiveError {
                                origin: live_error_origin!(),
                                span: in_doc.token_id_to_span(node.token_id),
                                message: format!("Cannot override {}, it is not a class node", node.id)
                            });
                        }
                    }
                }
                // OK so. in case if it doesnt exist, create a 0 class
                self.scope_stack.stack.push(Vec::new());
                let new_out_start = out_doc.get_level_len(out_level + 1);

                // walk the subobject
                for i in 0..node_count {
                    let new_out_count = out_doc.get_level_len(out_level + 1) - new_out_start;
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
                        class: MultiPack::zero_class(),
                        node_start: new_out_start as u32,
                        node_count: new_out_count as u16
                    }
                };
                self.scope_stack.stack.pop();
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
                
                
                return
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
                        message: format!("Cannot find import {}", MultiFmt::new(&in_doc.multi_ids, use_ids))
                    });
                    return
                };
                let other_doc = &self.expanded[file_id.to_index()];
                
                match use_ids.unpack() {
                    MultiUnpack::MultiId {index, count} => {
                        let shifted_count = count - 2;
                        // lets validate if it exists!
                        let mut node_start = 0 as usize;
                        let mut node_count = other_doc.nodes[0].len();
                        for level in 0..shifted_count {
                            let id = in_doc.multi_ids[level + 2 + index];
                            if id.is_empty() { // its a *
                                if level != shifted_count - 1 { // cant appear except at end
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
                                    if level == shifted_count - 1 { // last level
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
                                        message: format!("Use path not found {}", MultiFmt::new(&in_doc.multi_ids, use_ids))
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
                
                let mut class_pack = class;
                
                if !Self::is_baseclass(class) { // not baseclass
                    let result = self.resolve_id(
                        class,
                        node.token_id,
                        in_doc,
                        out_doc,
                        out_level,
                        out_start,
                    );
                    let is_value = match result {
                        Ok((None, found_node)) => { // found locally
                            if let Some(_) = self.copy_recur(None, found_node.level, found_node.index, out_doc, out_level, node.id, found_node.level) {
                                class_pack = MultiPack::live_ptr(self.in_file_id, found_node);
                                false
                            }
                            else {
                                true
                            }
                        }
                        Ok((Some(found_file_id), found_node)) => { // found in another file
                            let other_doc = &self.expanded[found_file_id.to_index()];
                            if let Some(_) = self.copy_recur(Some((other_doc, found_file_id)), found_node.level, found_node.index, out_doc, out_level, node.id, found_node.level) {
                                class_pack = MultiPack::live_ptr(found_file_id, found_node);
                                false
                            }
                            else {
                                true
                            }
                        }
                        Err(err) => {
                            self.errors.push(err);
                            return
                        }
                    };
                    if is_value {
                        self.scope_stack.stack.pop();
                        if node_count >0 {
                            self.errors.push(LiveError {
                                origin: live_error_origin!(),
                                span: in_doc.token_id_to_span(node.token_id),
                                message: format!("Cannot override items in non-class: {}", MultiFmt::new(&in_doc.multi_ids, class))
                            });
                        }
                        return
                    }
                }
                
                // walk the subobject
                for i in 0..node_count {
                    let new_out_count = out_doc.get_level_len(out_level + 1) - new_out_start;
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
                        class: class_pack,
                        node_start: new_out_start as u32,
                        node_count: new_out_count as u16
                    }
                };
                self.scope_stack.stack.pop();
                self.write_or_add_node(out_doc, out_level, out_start, out_count, in_doc, &new_node);
            }
        }
    }*/
}

