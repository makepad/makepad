use crate::id::Id;
use crate::liveerror::{LiveError, LiveErrorOrigin};
use makepad_id_macros::*;
use crate::livedocument::LiveDocument;
use crate::livenode::LiveValue;
use crate::livenode::LiveNodeSlice;
use crate::livenode::LiveNodeVec;
use crate::id::FileId;
use crate::id::LocalPtr;
use crate::id::LivePtr;
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
    
    pub fn store_scopes(&self, out_doc: &mut LiveDocument) -> (usize, usize) {
        let scope_start = out_doc.scopes.len();
        for i in 0..self.scope_stack.stack.len() {
            let scope = &self.scope_stack.stack[i];
            for j in 0..scope.len() {
                out_doc.scopes.push(scope[j]);
            }
        }
        (scope_start, out_doc.scopes.len() - scope_start)
    }
    
    pub fn expand(&mut self, in_doc: &LiveDocument, out_doc: &mut LiveDocument) {
        out_doc.nodes.push(in_doc.nodes[0].clone());
        let mut current_parent = vec![0usize];
        let mut level_overwrite = vec![false];
        let mut in_index = 1;
        
        loop {
            if in_index >= in_doc.nodes.len() - 1 {
                break;
            }
            
            let in_node = &in_doc.nodes[in_index];
            let in_value = &in_node.value;
            
            match in_value {
                LiveValue::Close => {
                    if !level_overwrite.last().unwrap() {
                        let index = out_doc.nodes.seek_child_append(*current_parent.last().unwrap());
                        out_doc.nodes.insert(index, in_node.clone());
                    }
                    current_parent.pop();
                    level_overwrite.pop();
                    self.scope_stack.stack.pop();
                    in_index += 1;
                    continue;
                }
                LiveValue::Use {crate_id, module_id, object_id} => {
                    // add items to the scope
                    if let Some(file_id) = self.module_path_to_file_id.get(&ModulePath(*crate_id, *module_id)){
                        // ok now find object_id and get us a pointer
                        let other_doc = &self.expanded[file_id.to_index()];
                        let mut other_index = 1;
                        while other_index < other_doc.nodes.len()-1{
                            let node = &other_doc.nodes[other_index];
                            if node.id.is_empty(){
                                continue;
                            }
                            if object_id.is_empty() || *object_id == node.id{
                                self.scope_stack.stack.last_mut().unwrap().push(LiveScopeItem {
                                    id: node.id,
                                    target: LiveScopeTarget::LivePtr(LivePtr{file_id:*file_id, local_ptr:LocalPtr(other_index)})
                                });
                                if !object_id.is_empty(){
                                    break
                                }
                            }
                            other_index = other_doc.nodes.skip_value(other_index);
                        }
                    }
                    in_index += 1;
                    continue;
                }
                _ => ()
            }
            
            // determine node overwrite activity
            let out_index = match out_doc.nodes.seek_child_by_name(*current_parent.last().unwrap(), in_node.id) {
                Ok(overwrite) => {
                    let out_value = &out_doc.nodes[overwrite].value;
                    if out_value.variant_id() == in_value.variant_id() { // same type
                        match in_value {
                            LiveValue::Array |
                            LiveValue::TupleEnum {..} |
                            LiveValue::NamedEnum {..} |
                            LiveValue::NamedClass {..} => {
                                let next_index = out_doc.nodes.skip_value(overwrite);
                                out_doc.nodes[overwrite] = in_node.clone();
                                out_doc.nodes.drain(overwrite + 1..next_index - 1);
                                level_overwrite.push(true);
                            },
                            LiveValue::BareClass => {
                                out_doc.nodes[overwrite] = in_node.clone();
                                level_overwrite.push(true);
                            }
                            _ => {
                                out_doc.nodes[overwrite] = in_node.clone();
                            }
                        }
                    }
                    else if in_value.is_enum() && out_value.is_enum() &&
                    in_value.enum_base_id() == out_value.enum_base_id() { // enum switch is allowed
                        if in_value.is_tree() {
                            if out_value.is_tree() {
                                let next_index = out_doc.nodes.skip_value(overwrite);
                                out_doc.nodes[overwrite] = in_node.clone();
                                out_doc.nodes.drain(overwrite + 1..next_index - 1);
                                level_overwrite.push(true);
                            }
                            else { // in is a tree, out isnt
                                out_doc.nodes[overwrite] = in_node.clone();
                                level_overwrite.push(false);
                            }
                        }
                        else if out_value.is_tree() { // out is a tree
                            let next_index = out_doc.nodes.skip_value(overwrite);
                            out_doc.nodes[overwrite] = in_node.clone();
                            out_doc.nodes.drain(overwrite + 1..next_index);
                        }
                        else {
                            panic!()
                        }
                        
                    }
                    else if in_value.is_bare_class() && out_value.is_named_class() {
                        // this is also allowed to overwrite
                        out_doc.nodes[overwrite] = in_node.clone();
                        level_overwrite.push(true);
                    }
                    else { // not allowed
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.token_id.unwrap()),
                            message: format!("Cannot switch node type for {} {:?} to {:?}", in_node.id, in_value, out_value)
                        });
                        in_index = in_doc.nodes.skip_value(in_index);
                        continue;
                    }
                    overwrite
                }
                Err(insert_point) => {
                    out_doc.nodes.insert(insert_point, in_node.clone());
                    if in_node.value.is_tree() {
                        level_overwrite.push(false);
                    }
                    insert_point
                }
            };
            
            self.scope_stack.stack.last_mut().unwrap().push(LiveScopeItem {
                id: in_node.id,
                target: LiveScopeTarget::LocalPtr(LocalPtr(out_index))
            });
            
            // process stacks
            match in_value {
                LiveValue::NamedClass {class} => {
                    
                    if let Some(target) = self.scope_stack.find_item(*class) {
                        match target {
                            LiveScopeTarget::LocalPtr(local_ptr) => {
                                out_doc.nodes.clone_children_self(local_ptr.0, Some(out_index + 1));
                            }
                            LiveScopeTarget::LivePtr(live_ptr) => {
                                let doc = &self.expanded[live_ptr.file_id.to_index()];
                                out_doc.nodes.clone_children_from(live_ptr.local_ptr.0, Some(out_index + 1), &doc.nodes);
                            }
                        }
                    }
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                }, 
                LiveValue::Array => {
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                },
                LiveValue::TupleEnum {..} => { 
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                },
                LiveValue::NamedEnum {..} => { 
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                },
                LiveValue::BareClass => {
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                },
                LiveValue::Fn {..} => {
                    let (start, count) = self.store_scopes(out_doc);
                    out_doc.nodes[out_index].value.set_scope(start, count as u32);
                },
                LiveValue::Const {..} => {
                    let (start, count) = self.store_scopes(out_doc);
                    out_doc.nodes[out_index].value.set_scope(start, count as u32);
                },
                LiveValue::VarDef {..} => {
                    let (start, count) = self.store_scopes(out_doc);
                    out_doc.nodes[out_index].value.set_scope(start, count as u32);
                },
                _ => {}
            }
            in_index += 1;
        }
        out_doc.nodes.push(in_doc.nodes.last().unwrap().clone());
    }
    
}

