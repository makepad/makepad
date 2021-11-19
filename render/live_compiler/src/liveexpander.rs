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
use std::fmt;

pub struct ScopeStack {
    pub stack: Vec<Vec<LiveScopeItem >>
}

impl fmt::Debug for ScopeStack {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for (level, items) in self.stack.iter().enumerate() {
            for item in items.iter() {
                writeln!(f, "{} {}", level, item.id).unwrap();
            }
        }
        fmt::Result::Ok(())
    }
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
                        let index = out_doc.nodes.append_child_index(*current_parent.last().unwrap());
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

                        let mut node_iter = other_doc.nodes.first_child(0);
                        while let Some(node_index) = node_iter{
                            let node = &other_doc.nodes[node_index];
                            if !node.id.is_empty() && (object_id.is_empty() || *object_id == node.id){
                                self.scope_stack.stack.last_mut().unwrap().push(LiveScopeItem {
                                    id: node.id,
                                    target: LiveScopeTarget::LivePtr(LivePtr{file_id:*file_id, local_ptr:LocalPtr(node_index)})
                                });
                                if !object_id.is_empty(){
                                    break
                                }
                            }
                            node_iter = other_doc.nodes.next_child(node_index);
                        }
                    }
                    in_index += 1;
                    continue;
                }
                _ => ()
            }
            
            // determine node overwrite rules
            let out_index = match out_doc.nodes.child_by_name(*current_parent.last().unwrap(), in_node.id) {
                Ok(overwrite) => {
                    let out_value = &out_doc.nodes[overwrite].value;
                    if out_value.variant_id() == in_value.variant_id() { // same type
                        match in_value {
                            LiveValue::Array |
                            LiveValue::TupleEnum {..} |
                            LiveValue::NamedEnum {..} |
                            LiveValue::Class {..} => {
                                let next_index = out_doc.nodes.next_child(overwrite).unwrap();
                                out_doc.nodes[overwrite] = in_node.clone();
                                out_doc.nodes.drain(overwrite + 1..next_index - 1);
                                level_overwrite.push(true);
                            },
                            LiveValue::Object => {
                                out_doc.nodes[overwrite] = in_node.clone();
                                level_overwrite.push(true);
                            }
                            _ => {
                                out_doc.nodes[overwrite] = in_node.clone();
                            }
                        }
                        overwrite
                    }
                    else if in_value.is_enum() && out_value.is_enum() &&
                    in_value.enum_base_id() == out_value.enum_base_id() { // enum switch is allowed
                        if in_value.is_open() {
                            if out_value.is_open() {
                                let next_index = out_doc.nodes.next_child(overwrite).unwrap();
                                out_doc.nodes[overwrite] = in_node.clone();
                                out_doc.nodes.drain(overwrite + 1..next_index - 1);
                                level_overwrite.push(true);
                            }
                            else { // in is a tree, out isnt
                                out_doc.nodes[overwrite] = in_node.clone();
                                level_overwrite.push(false);
                            }
                        }
                        else if out_value.is_open() { // out is a tree remove incl close
                            let next_index = out_doc.nodes.next_child(overwrite).unwrap();
                            out_doc.nodes[overwrite] = in_node.clone();
                            out_doc.nodes.drain(overwrite + 1..next_index);
                        }
                        else {
                            panic!()
                        }
                        overwrite
                    }
                    else if in_value.is_object() && out_value.is_class() {
                        // this is also allowed to overwrite but don't overwrite the name of the class
                        //out_doc.nodes[overwrite] = in_node.clone();
                        level_overwrite.push(true);
                        overwrite
                    }
                    else if out_value.is_var_def() && in_value.is_value_type(){ // this is allowed
                        // we 'insert' it right after the vardef
                        out_doc.nodes.insert(overwrite+1, in_node.clone());
                        overwrite + 1
                    }
                    else{
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.token_id.unwrap()),
                            message: format!("Cannot switch node type for {} {:?} to {:?}", in_node.id, in_value, out_value)
                        });
                        in_index = in_doc.nodes.next_child(in_index).unwrap();
                        continue;
                    }
                }
                Err(insert_point) => {
                    out_doc.nodes.insert(insert_point, in_node.clone());
                    if in_node.value.is_open() {
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
                LiveValue::Class {class} => {
                    if let Some(target) = self.scope_stack.find_item(*class) {
                        let cn = match target {
                            LiveScopeTarget::LocalPtr(local_ptr) => {
                                if out_doc.nodes.clone_children_self(local_ptr.0, Some(out_index + 1)){
                                    self.errors.push(LiveError {
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(in_node.token_id.unwrap()),
                                        message: format!("Infinite recursion at {}", in_node.id)
                                    }); 
                                }
                                //println!("LOCAL EXPANSION {}",out_doc.nodes[local_ptr.0].value.get_class_name());
                                out_doc.nodes[local_ptr.0].value.get_class_name()
                            }
                            LiveScopeTarget::LivePtr(live_ptr) => {
                                let doc = &self.expanded[live_ptr.file_id.to_index()];
                                out_doc.nodes.clone_children_from(live_ptr.node_index(), Some(out_index + 1), &doc.nodes);
                                //println!("REMOTE EXPANSION {}",doc.nodes[live_ptr.node_index()].value.get_class_name());
                                doc.nodes[live_ptr.node_index()].value.get_class_name()
                            }
                        };
                        out_doc.nodes[out_index].value.set_class_name(cn);
                    }/*
                    else{
                        if *class == id!(DrawDesktopButton){
                            println!("{:?} {} {:?}", self.in_file_id, class, self.scope_stack);
                        }
                    }*/
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                }, 
                LiveValue::Array |
                LiveValue::TupleEnum {..} |
                LiveValue::NamedEnum {..} |
                LiveValue::Object => {
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                },
                LiveValue::Fn {..} |
                LiveValue::Const {..} |
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

