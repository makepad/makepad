use {
    crate::{
        makepad_id_macros::*,
        makepad_live_tokenizer::LiveId,
        live_ptr::{LiveFileId, LivePtr, LiveFileGeneration},
        live_error::{LiveError, LiveErrorOrigin},
        live_document::{LiveOriginal, LiveExpanded},
        live_node::{LiveValue, LiveNode, /*LiveTypeKind*/},
        live_node_vec::{LiveNodeSlice, LiveNodeVec},
        live_registry::{LiveRegistry, LiveScopeTarget},
    }
};

pub struct LiveExpander<'a> {
    pub live_registry: &'a LiveRegistry,
    pub in_crate: LiveId,
    pub in_file_id: LiveFileId,
    pub errors: &'a mut Vec<LiveError>,
}

impl<'a> LiveExpander<'a> {
    pub fn is_baseclass(id: LiveId) -> bool {
        id == id!(Component)
            || id == id!(Enum)
            || id == id!(Struct)
            || id == id!(Namespace)
            || id == id!(DrawShader)
            || id == id!(Geometry)
    }
    
    pub fn shift_parent_stack(&self, parents: &mut Vec<(LiveId, usize)>, nodes: &[LiveNode], after_point: usize, old_size: usize, new_size: usize) {
        for (live_id, item) in parents {
            if *item > after_point {
                if old_size > new_size {
                    *item -= old_size - new_size;
                }
                else if old_size < new_size {
                    *item += new_size - old_size;
                }
                if nodes[*item].id != *live_id {
                    panic!()
                }
                if !nodes[*item].is_open() {
                    panic!()
                }
            }
        }
    }
    
    pub fn expand(&mut self, in_doc: &LiveOriginal, out_doc: &mut LiveExpanded, generation:LiveFileGeneration) {
        
        // ok first copy the edit_info over.
        //out_doc.edit_info = in_doc.edit_info.clone();
        
        out_doc.nodes.push(in_doc.nodes[0].clone());
        let mut current_parent = vec![(LiveId(0), 0usize)];
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
                        let old_len = out_doc.nodes.len();
                        let index = out_doc.nodes.append_child_index(current_parent.last().unwrap().1);
                        out_doc.nodes.insert(index, in_node.clone());
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, index, old_len, out_doc.nodes.len());
                    }
                    current_parent.pop();
                    level_overwrite.pop();
                    //self.scope_stack.stack.pop();
                    in_index += 1;
                    continue;
                }
                LiveValue::Use(module_id) => {
                    // lets verify it points anywhere
                    if self.live_registry.module_id_and_name_to_doc(*module_id, in_node.id).is_none() {
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_node.origin.token_id().unwrap().into(),
                            message: format!("Use statement invalid target {}::{}", module_id, in_node.id)
                        });
                    }
                    let index = out_doc.nodes.append_child_index(current_parent.last().unwrap().1);
                    let old_len = out_doc.nodes.len();
                    out_doc.nodes.insert(index, in_node.clone());
                    self.shift_parent_stack(&mut current_parent, &out_doc.nodes, index, old_len, out_doc.nodes.len());
                    in_index += 1;
                    continue;
                }
                _ => ()
            }
            
            //// determine node overwrite rules
            
            let out_index = match out_doc.nodes.child_or_append_index_by_name(current_parent.last().unwrap().1, in_node.id) {
                Ok(overwrite) => {
                    let out_value = &out_doc.nodes[overwrite].value;
                    
                    let out_origin = out_doc.nodes[overwrite].origin;
                    
                    if in_node.origin.edit_info().is_some() {
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                            message: format!("Cannot define edit info after first prop def of {}", in_node.id)
                        });
                    }
                    let ret_val = if in_value.is_expr() && out_value.is_expr() || in_value.is_expr() && out_value.is_value_type() {
                        // replace range
                        let next_index = out_doc.nodes.skip_node(overwrite);
                        
                        // POTENTIAL SHIFT
                        let old_len = out_doc.nodes.len();
                        out_doc.nodes.splice(overwrite..next_index, in_doc.nodes.node_slice(in_index).iter().cloned());
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                        
                        in_index = in_doc.nodes.skip_node(in_index);
                        out_doc.nodes[overwrite].origin.inherit_origin(out_origin);
                        continue;
                    }
                    else if !in_value.is_class() && out_value.variant_id() == in_value.variant_id() { // same type
                        match in_value {
                            LiveValue::Array |
                            LiveValue::TupleEnum {..} |
                            LiveValue::NamedEnum {..} |
                            LiveValue::Clone {..} => {
                                out_doc.nodes[overwrite] = in_node.clone();
                                if let Some(next_index)= out_doc.nodes.next_child(overwrite){
                                 //   let next_index = out_doc.nodes.next_child(overwrite).unwrap();
                                    // POTENTIAL SHIFT
                                    let old_len = out_doc.nodes.len();
                                    out_doc.nodes.drain(overwrite + 1..next_index - 1);
                                    self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                                }
                                
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
                                let next_index = out_doc.nodes.skip_node(overwrite);
                                out_doc.nodes[overwrite] = in_node.clone();
                                // POTENTIAL SHIFT
                                let old_len = out_doc.nodes.len();
                                out_doc.nodes.drain(overwrite + 1..next_index - 1);
                                self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                                
                                level_overwrite.push(true);
                            }
                            else { // in is a tree, out isnt
                                out_doc.nodes[overwrite] = in_node.clone();
                                level_overwrite.push(false);
                            }
                        }
                        else if out_value.is_open() { // out is a tree remove incl close
                            let next_index = out_doc.nodes.skip_node(overwrite);
                            out_doc.nodes[overwrite] = in_node.clone();
                            // POTENTIAL SHIFT
                            let old_len = out_doc.nodes.len();
                            out_doc.nodes.drain(overwrite + 1..next_index);
                            self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                        }
                        else {
                            panic!()
                        }
                        overwrite
                    }
                    else if in_value.is_object() && out_value.is_clone() {
                        level_overwrite.push(true);
                        overwrite
                    }
                    else if in_value.is_clone() && out_value.is_class() {
                        // throw away whats in there
                        let next_index = out_doc.nodes.skip_node(overwrite);
                        let old_len = out_doc.nodes.len();
                        out_doc.nodes.drain(overwrite + 1..next_index - 1);
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                        level_overwrite.push(true);
                        overwrite
                    } 
                    else if in_value.is_clone() && out_value.is_object() {
                        // throw away whats in there
                        let next_index = out_doc.nodes.skip_node(overwrite);
                        let old_len = out_doc.nodes.len();
                        out_doc.nodes.drain(overwrite + 1..next_index - 1);
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                        level_overwrite.push(true);
                        overwrite
                    }
                    else if in_value.is_object() && out_value.is_class() { // lets set the target ptr
                        level_overwrite.push(true);
                        overwrite
                    }
                    else if in_value.is_number_type() && out_value.is_number_type(){
                        out_doc.nodes[overwrite] = in_node.clone();
                        overwrite
                    }
                    else{
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                            message: format!("Cannot switch node type for {} {:?} to {:?}", in_node.id, out_value, in_value)
                        });
                        in_index = in_doc.nodes.skip_node(in_index);
                        continue;
                    };
                    
                    out_doc.nodes[overwrite].origin.inherit_origin(out_origin);
                    
                    ret_val
                }
                Err(insert_point) => {
                    // ok so. if we are inserting an expression, just do the whole thing in one go.
                    if in_node.is_expr() {
                        // splice it in
                        let old_len = out_doc.nodes.len();
                        out_doc.nodes.splice(insert_point..insert_point, in_doc.nodes.node_slice(in_index).iter().cloned());
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, insert_point - 1, old_len, out_doc.nodes.len());
                        
                        in_index = in_doc.nodes.skip_node(in_index);
                        continue;
                    }
                    
                    let old_len = out_doc.nodes.len();
                    out_doc.nodes.insert(insert_point, in_node.clone());
                    self.shift_parent_stack(&mut current_parent, &out_doc.nodes, insert_point - 1, old_len, out_doc.nodes.len());
                    
                    if in_node.is_open() {
                        level_overwrite.push(false);
                    }
                    insert_point
                }
            };
            
            // process stacks
            match in_value {
                LiveValue::Clone(clone) => {
                    if let Some(target) = self.live_registry.find_scope_target_via_start(*clone, out_index, &out_doc.nodes) {
                        match target {
                            LiveScopeTarget::LocalPtr(local_ptr) => {
                                //println!("CLONING LOCAL {}", clone);
                                let old_len = out_doc.nodes.len();
                                
                                out_doc.nodes.insert_children_from_self(local_ptr, out_index + 1);
                                self.shift_parent_stack(&mut current_parent, &out_doc.nodes, out_index, old_len, out_doc.nodes.len());
                                
                                // clone the value and store a parent pointer
                                if let LiveValue::Class {live_type: old_live_type, ..} = &out_doc.nodes[out_index].value {
                                    if let LiveValue::Class {live_type, ..} = &out_doc.nodes[local_ptr].value {
                                        if live_type != old_live_type {
                                            self.errors.push(LiveError {
                                                origin: live_error_origin!(),
                                                span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                                                message: format!("Class override with wrong type {}", in_node.id)
                                            });
                                        }
                                    }
                                }
                                
                                out_doc.nodes[out_index].value = out_doc.nodes[local_ptr].value.clone();
                                if let LiveValue::Class {class_parent, ..} = &mut out_doc.nodes[out_index].value {
                                    *class_parent = Some(LivePtr {file_id: self.in_file_id, index: out_index as u32, generation});
                                }
                            }
                            LiveScopeTarget::LivePtr(live_ptr) => {
                                let doc = &self.live_registry.live_files[live_ptr.file_id.to_index()].expanded;
                                
                                let old_len = out_doc.nodes.len();
                                out_doc.nodes.insert_children_from_other(live_ptr.node_index(), out_index + 1, &doc.nodes);
                                self.shift_parent_stack(&mut current_parent, &out_doc.nodes, out_index, old_len, out_doc.nodes.len());
                                
                                // store the parent pointer
                                if let LiveValue::Class {live_type: old_live_type, ..} = &out_doc.nodes[out_index].value {
                                    if let LiveValue::Class {live_type, ..} = &doc.nodes[live_ptr.node_index()].value {
                                        if live_type != old_live_type {
                                            self.errors.push(LiveError {
                                                origin: live_error_origin!(),
                                                span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                                                message: format!("Class override with wrong type {}", in_node.id)
                                            });
                                        }
                                    }
                                }
                                out_doc.nodes[out_index].value = doc.nodes[live_ptr.node_index()].value.clone();
                                if let LiveValue::Class {class_parent, ..} = &mut out_doc.nodes[out_index].value {
                                    *class_parent = Some(LivePtr {file_id: self.in_file_id, index: out_index as u32, generation});
                                }
                            }
                        };
                        //overwrite value, this copies the Class
                    }
                    else if !self.live_registry.ignore_no_dsl.contains(clone){
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                            message: format!("Can't find live definition of {} did you forget to call live_register for it?", clone)
                        });
                    }
                    current_parent.push((out_doc.nodes[out_index].id, out_index));
                },
                LiveValue::Class {live_type, ..} => {
                    // store the class context
                    if let LiveValue::Class {class_parent, ..} = &mut out_doc.nodes[out_index].value {
                        *class_parent = Some(LivePtr {file_id: self.in_file_id, index: out_index as u32, generation});
                    }
                    
                    let mut insert_point = out_index + 1;
                    let mut live_type_info = self.live_registry.live_type_infos.get(live_type).unwrap();
                    
                    let mut has_deref_hop = false;
                    while let Some(field) = live_type_info.fields.iter().find( | f | f.id == id!(deref_target)) {
                        has_deref_hop = true;
                        live_type_info = &field.live_type_info;
                    }
                    if has_deref_hop {
                        // ok so we need the lti of the deref hop and clone all children
                        if let Some(file_id) = self.live_registry.module_id_to_file_id.get(&live_type_info.module_id) {
                            let doc = &self.live_registry.live_files[file_id.to_index()].expanded;
                            if let Some(index) = doc.nodes.child_by_name(0, live_type_info.type_name) {
                                let old_len = out_doc.nodes.len();
                                out_doc.nodes.insert_children_from_other(index, out_index + 1, &doc.nodes);
                                self.shift_parent_stack(&mut current_parent, &out_doc.nodes, out_index, old_len, out_doc.nodes.len());
                            }
                        }
                    }
                    else {
                        for field in &live_type_info.fields {
                            let lti = &field.live_type_info;
                            if let Some(file_id) = self.live_registry.module_id_to_file_id.get(&lti.module_id) {
                                
                                if *file_id == self.in_file_id { // clone on self
                                    if let Some(index) = out_doc.nodes.child_by_name(0, lti.type_name) {
                                        let node_insert_point = insert_point;
                                        
                                        let old_len = out_doc.nodes.len();
                                        insert_point = out_doc.nodes.insert_node_from_self(index, insert_point);
                                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, node_insert_point - 1, old_len, out_doc.nodes.len());
                                        
                                        out_doc.nodes[node_insert_point].id = field.id;
                                    }
                                    else if !self.live_registry.ignore_no_dsl.contains(&lti.type_name){
                                        self.errors.push(LiveError {
                                            origin: live_error_origin!(),
                                            span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                                            message: format!("Can't find live definition of {} did you forget to call live_register for it?", lti.type_name)
                                        });
                                    }
                                }
                                else {
                                    let other_nodes = &self.live_registry.live_files[file_id.to_index()].expanded.nodes;
                                    if other_nodes.len() == 0 {
                                        panic!(
                                            "Dependency order bug finding {}, file {} not registered before {}",
                                            lti.type_name,
                                            self.live_registry.file_id_to_file_name(*file_id),
                                            self.live_registry.file_id_to_file_name(self.in_file_id),
                                        );
                                    }
                                    if let Some(index) = other_nodes.child_by_name(0, lti.type_name) {
                                        let node_insert_point = insert_point;
                                        
                                        let old_len = out_doc.nodes.len();
                                        insert_point = out_doc.nodes.insert_node_from_other(index, insert_point, other_nodes);
                                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, node_insert_point - 1, old_len, out_doc.nodes.len());
                                        
                                        out_doc.nodes[node_insert_point].id = field.id;
                                    }
                                    else if lti.type_name != LiveId(0){
                                        self.errors.push(LiveError {
                                            origin: live_error_origin!(),
                                            span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                                            message: format!("Typename {}, not defined in file where it was expected", lti.type_name)
                                        });
                                    }
                                }
                            }
                            else if !self.live_registry.ignore_no_dsl.contains(&lti.type_name){
                                self.errors.push(LiveError {
                                    origin: live_error_origin!(),
                                    span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                                    message: format!("Can't find live definition of {} did you forget to call live_register for it?", lti.type_name)
                                });
                            }
                        }
                    }
                    current_parent.push((out_doc.nodes[out_index].id, out_index));
                }
                LiveValue::Expr{..} => {
                    panic!()
                },
                LiveValue::Array |
                LiveValue::TupleEnum {..} |
                LiveValue::NamedEnum {..} |
                LiveValue::Object => { // lets check what we are overwriting
                    current_parent.push((out_doc.nodes[out_index].id, out_index));
                },
                LiveValue::DSL {..} => {},
                _ => {}
            }
            
            in_index += 1;
        }
        out_doc.nodes.push(in_doc.nodes.last().unwrap().clone());
        
        // this stores the node index on nodes that don't have a node index
        for i in 1..out_doc.nodes.len() {
            if out_doc.nodes[i].value.is_dsl() {
                out_doc.nodes[i].value.set_dsl_expand_index_if_none(i);
            }
            if out_doc.nodes[i].value.is_expr() {
                out_doc.nodes[i].value.set_expr_expand_index_if_none(i);
            }
        }
    }
    
}

