use{
    std::fmt,
    makepad_id_macros::*,
    crate::{
        liveid::{LiveId, LiveFileId, LivePtr},
        liveerror::{LiveError, LiveErrorOrigin},
        livedocument::{LiveDocument,LiveScopeTarget, LiveScopeItem},
        livenode::{LiveValue, LiveTypeKind},
        livenodevec::{LiveNodeSlice, LiveNodeVec},
        liveregistry::LiveRegistry
    }
};

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

    fn find_item(&self, id: LiveId) -> Option<LiveScopeTarget> {
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
    pub live_registry: &'a LiveRegistry,
    pub in_crate: LiveId,
    pub in_file_id: LiveFileId,
    pub errors: &'a mut Vec<LiveError>,
    pub scope_stack: &'a mut ScopeStack,
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
                LiveValue::Use(module_path)=> {
                    let object_id = in_node.id;
                    // add items to the scope
                    if let Some(file_id) = self.live_registry.module_id_to_file_id.get(&module_path){
                        // ok now find object_id and get us a pointer
                        let other_doc = &self.live_registry.expanded[file_id.to_index()];

                        let mut node_iter = other_doc.nodes.first_child(0);
                        let mut items_added = 0;
                        while let Some(node_index) = node_iter{
                            let node = &other_doc.nodes[node_index];
                            if !node.id.is_empty() && (object_id.is_empty() || object_id == node.id){
                                self.scope_stack.stack.last_mut().unwrap().push(LiveScopeItem {
                                    id: node.id,
                                    target: LiveScopeTarget::LivePtr(LivePtr{file_id:*file_id, index:node_index as u32})
                                });
                                items_added += 1;
                                if !object_id.is_empty(){
                                    break
                                }
                            }
                            node_iter = other_doc.nodes.next_child(node_index);
                        }
                        if items_added == 0{
                            self.errors.push(LiveError {
                                origin: live_error_origin!(),
                                span: in_doc.token_id_to_span(in_node.token_id.unwrap()),
                                message: format!("Cannot find use {}::{}", module_path, object_id)
                            });
                        }
                    }
                    let index = out_doc.nodes.append_child_index(*current_parent.last().unwrap());
                    out_doc.nodes.insert(index, in_node.clone());
                    in_index += 1;
                    continue;
                }
                _ => ()
            }
            
            // determine node overwrite rules
            let out_index = match out_doc.nodes.child_or_append_index_by_name(*current_parent.last().unwrap(), in_node.id) {
                Ok(overwrite) => {
                    let out_value = &out_doc.nodes[overwrite].value;
                    
                    if in_value.is_expr() && out_value.is_expr() || in_value.is_expr() && out_value.is_value_type(){
                        // replace range
                        let next_index = out_doc.nodes.skip_node(overwrite);
                        out_doc.nodes.splice(overwrite..next_index, in_doc.nodes.node_slice(in_index).iter().cloned());
                        in_index = in_doc.nodes.skip_node(in_index);
                        continue;
                    }
                    else if !in_value.is_class() && out_value.variant_id() == in_value.variant_id() { // same type
                        match in_value {
                            LiveValue::Array |
                            LiveValue::TupleEnum {..} |
                            LiveValue::NamedEnum {..} |
                            LiveValue::Clone {..} => {
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
                    else if in_value.is_object() && out_value.is_clone() {
                        level_overwrite.push(true);
                        overwrite
                    }
                    else if in_value.is_object() && out_value.is_class() { // lets set the target ptr
                        // where is the thing in out_value from? we don't really know eh
                        //if let LiveValue::Class{class_parent,..} = &mut out_doc.nodes[overwrite].value{
                        //    *class_parent = Some(live_ptr);
                        //}
                        level_overwrite.push(true);
                        overwrite
                    }
                    else if out_value.is_dsl() && in_value.is_value_type(){ // this is allowed
                        // see if we have a node after to overwrite
                        if let Some(overwrite) = out_doc.nodes.next_child_by_name(overwrite+1, in_node.id){
                            out_doc.nodes[overwrite] = in_node.clone();
                            overwrite
                        }
                        else{
                            out_doc.nodes.insert(overwrite+1, in_node.clone());
                            overwrite + 1
                        }
                    }
                    else{
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.token_id.unwrap()),
                            message: format!("Cannot switch node type for {} {:?} to {:?}", in_node.id, out_value, in_value)
                        });
                        in_index = in_doc.nodes.skip_node(in_index);
                        continue;
                    }
                }
                Err(insert_point) => {
                    // ok so. if we are inserting an expression, just do the whole thing in one go.
                    if in_node.value.is_expr(){
                        // splice it in
                        out_doc.nodes.splice(insert_point..insert_point, in_doc.nodes.node_slice(in_index).iter().cloned());
                        in_index = in_doc.nodes.skip_node(in_index);
                        continue;
                    }
                    
                    out_doc.nodes.insert(insert_point, in_node.clone());
                    if in_node.value.is_open() {
                        level_overwrite.push(false);
                    }
                    insert_point
                }
            };
            
            self.scope_stack.stack.last_mut().unwrap().push(LiveScopeItem {
                id: in_node.id,
                target: LiveScopeTarget::LocalPtr(out_index)
            });
            
            // process stacks
            match in_value {
                LiveValue::Clone(clone) => {
                    if let Some(target) = self.scope_stack.find_item(*clone) {
                        match target {
                            LiveScopeTarget::LocalPtr(local_ptr) => {
                                if out_doc.nodes.insert_children_from_self(local_ptr, Some(out_index + 1)){
                                    self.errors.push(LiveError {
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(in_node.token_id.unwrap()),
                                        message: format!("Infinite recursion at {}", in_node.id)
                                    }); 
                                }
                                // clone the value and store a parent pointer
                                out_doc.nodes[out_index].value = out_doc.nodes[local_ptr].value.clone();
                                if let LiveValue::Class{class_parent,..} = &mut out_doc.nodes[out_index].value{
                                    *class_parent = Some(LivePtr{file_id: self.in_file_id, index:out_index as u32});
                                }
                            }
                            LiveScopeTarget::LivePtr(live_ptr) => {
                                let doc = &self.live_registry.expanded[live_ptr.file_id.to_index()];
                                out_doc.nodes.insert_children_from_other(live_ptr.node_index(), Some(out_index + 1), &doc.nodes);
                                // store the parent pointer
                                out_doc.nodes[out_index].value = doc.nodes[live_ptr.node_index()].value.clone();
                                if let LiveValue::Class{class_parent,..} = &mut out_doc.nodes[out_index].value{
                                    *class_parent = Some(LivePtr{file_id: self.in_file_id, index:out_index as u32});
                                }
                            }
                        };
                        //overwrite value, this copies the Class
                        
                    }
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                }, 
                LiveValue::Class{live_type,..}=>{
                    // store the class context
                    if let LiveValue::Class{class_parent,..} = &mut out_doc.nodes[out_index].value{
                        *class_parent = Some(LivePtr{file_id: self.in_file_id, index:out_index as u32});
                    }
                    

                    let mut insert_point = out_index + 1;
                    
                    // if we have a deref_target lets traverse all the way up
                    let mut live_type_info = self.live_registry.live_type_infos.get(live_type).unwrap();
                    
                    let mut has_deref_hop = false;
                    while let Some(field) = live_type_info.fields.iter().find(|f| f.id == id!(deref_target)){
                        has_deref_hop = true;
                        live_type_info = &field.live_type_info;
                    }
                    if has_deref_hop{
                        // ok so we need the lti of the deref hop and clone all children
                        if let Some(file_id) = self.live_registry.module_id_to_file_id.get(&live_type_info.module_id) {
                            let doc = &self.live_registry.expanded[file_id.to_index()];
                            if let Some(index) = doc.nodes.child_by_name(0, live_type_info.type_name){
                                out_doc.nodes.insert_children_from_other(index, Some(out_index + 1), &doc.nodes);
                            }
                        }
                    }
                    else{
                        for field in &live_type_info.fields{
                            let lti = &field.live_type_info; 
                            if let Some(file_id) = self.live_registry.module_id_to_file_id.get(&lti.module_id) {
                                if *file_id == self.in_file_id{ // clone on self
                                    if let Some(index) = out_doc.nodes.child_by_name(0, lti.type_name){
                                        let node_insert_point = insert_point;
                                        insert_point = out_doc.nodes.insert_node_from_self(index, Some(insert_point));
                                        out_doc.nodes[node_insert_point].id = field.id;
                                    }
                                    else if let LiveTypeKind::Class = lti.kind{
                                        self.errors.push(LiveError {
                                            origin: live_error_origin!(),
                                            span: in_doc.token_id_to_span(in_node.token_id.unwrap()),
                                            message: format!("Cannot find class {}, make sure its defined before its used", lti.type_name)
                                        }); 
                                    }
                                }
                                else{
                                    let other_nodes = &self.live_registry.expanded[file_id.to_index()].nodes;
                                    if other_nodes.len() == 0{
                                        panic!("Dependency order wrong, someday i'll learn to program. {} {} {}", file_id.0, self.in_file_id.0, lti.type_name);
                                    }
                                    if let Some(index) = other_nodes.child_by_name(0, lti.type_name){
                                        let node_insert_point = insert_point;
                                        insert_point = out_doc.nodes.insert_node_from_other(index, Some(insert_point), other_nodes);
                                        out_doc.nodes[node_insert_point].id = field.id;
                                    }
                                }
                            }
                        }
                    }

                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                }
                LiveValue::Expr=>{
                    panic!()
                },
                LiveValue::Array |
                LiveValue::TupleEnum {..} |
                LiveValue::NamedEnum {..} |
                LiveValue::Object => { // lets check what we are overwriting
                    
                    self.scope_stack.stack.push(Vec::new());
                    current_parent.push(out_index);
                },
                LiveValue::DSL {..} => {
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

