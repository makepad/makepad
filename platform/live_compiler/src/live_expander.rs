use std::cmp::Ordering;

use {
    std::sync::Arc,
    crate::{
        makepad_live_id::*,
        makepad_live_tokenizer::{live_error_origin, LiveErrorOrigin},
        live_ptr::{LiveFileId, LivePtr, LiveFileGeneration},
        live_error::{LiveError},
        live_eval::live_eval_value,
        live_document::{LiveOriginal, LiveExpanded},
        live_node::{LiveFont, LiveValue, LiveNode, LiveFieldKind, LivePropType, LiveNodeRoot},
        live_node_vec::{LiveNodeSliceApi, LiveNodeVecApi},
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
        id == live_id!(struct)
    }
    
    pub fn shift_parent_stack(&self, parents: &mut Vec<(LiveId, usize)>, nodes: &[LiveNode], after_point: usize, old_size: usize, new_size: usize) {
        for (live_id, item) in parents {
            if *item > after_point {
                match old_size.cmp(&new_size) {
                    Ordering::Less => *item += new_size - old_size,
                    Ordering::Greater => *item -= old_size - new_size,
                    _ => (),
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
    
    fn resolve_path(&self, file_id:LiveFileId, path:&str)->Option<Arc<String>>{
        if let Some(path) = path.strip_prefix("crate://self/") {
            let mut final_path = self.live_registry.file_id_to_cargo_manifest_path(file_id);
            final_path.push('/');
            final_path.push_str(path);
            return Some(Arc::new(final_path))
        } else if let Some(path) = path.strip_prefix("crate://") {
            let mut split = path.split('/');
            if let Some(crate_name) = split.next() {
                if let Some(mut final_path) = self.live_registry.crate_name_to_cargo_manifest_path(crate_name) {
                    for next in split {
                        final_path.push('/');
                        final_path.push_str(next);
                    }
                    return Some(Arc::new(final_path))
                }
            }
        }
        None
    }
    
    pub fn expand(&mut self, in_doc: &LiveOriginal, out_doc: &mut LiveExpanded, generation: LiveFileGeneration) {
        
        let mut root = LiveNodeRoot::default();
        
        // lets build up the final local pointers from our imports
        let resolved_imports = in_doc.resolved_imports.as_ref().unwrap();
        for (ident, file_id) in resolved_imports{
            let other = &self.live_registry.live_files[file_id.to_index()].expanded;
            if let LiveValue::Root(root2) = &other.nodes[0].value {
                let index = root2.exports.get(&ident).unwrap();
                root.locals.insert(
                    *ident,
                    LiveScopeTarget::LivePtr(
                        self.live_registry.file_id_index_to_live_ptr(*file_id, *index)
                    )
                );
            }
        }
        
        //out_doc.nodes.push(in_doc.nodes[0].clone());
        out_doc.nodes.push(LiveNode {
            origin: in_doc.nodes[0].origin,
            id: LiveId(0),
            value: LiveValue::Root(Box::new(root))
        });
        // alright lets build the locals from the import table
        
        let mut current_parent = vec![(LiveId(0), 0usize)];
        let mut in_index = 1;
        let mut lazy_define_value = None;
        loop {
            
            if let Some((node_id, ptr)) = lazy_define_value.take() {
                if let LiveValue::Root(root) = &mut out_doc.nodes[0].value {
                    root.locals.insert(node_id, ptr);
                }
            }
            
            if in_index >= in_doc.nodes.len() - 1 {
                break;
            }
            
            let in_node = &in_doc.nodes[in_index];
            let in_value = &in_node.value;
            
            match in_value {
                
                LiveValue::Close => {
                    // inherit the close node origin so we can properly map to the original text range
                    let close_index = out_doc.nodes.skip_node(current_parent.last().unwrap().1) - 1;
                    let out_close = &mut out_doc.nodes[close_index];
                    let mut origin = in_node.origin;
                    origin.inherit_origin(out_close.origin);
                    out_close.origin = origin;
                    
                    current_parent.pop();
                    in_index += 1;
                    continue;
                }
                LiveValue::Import(_live_import) => {
                    /*
                    // lets verify it points anywhere
                    let mut found = false;
                    let is_glob = in_node.id == LiveId::empty();
                    
                    if let Some(nodes) = self.live_registry.module_id_to_expanded_nodes(live_import.module_id) {
                        let file_id = self.live_registry.module_id_to_file_id(live_import.module_id).unwrap();
                        let mut node_iter = Some(1);
                        while let Some(index) = node_iter {
                            if is_glob{// its *
                                if let LiveValue::Root(root) = &mut out_doc.nodes[0].value {
                                    root.locals.insert(nodes[index].id, LiveScopeTarget::LivePtr(
                                        self.live_registry.file_id_index_to_live_ptr(file_id, index)
                                    ));
                                }
                                found = true;
                            }
                            else if nodes[index].id == live_import.import_id { 
                                // ok so what do we store...
                                if let LiveValue::Root(root) = &mut out_doc.nodes[0].value {
                                    root.locals.insert(in_node.id , LiveScopeTarget::LivePtr(
                                        self.live_registry.file_id_index_to_live_ptr(file_id, index)
                                    ));
                                }
                                found = true;
                            }
                            node_iter = nodes.next_child(index);
                        }
                    }
                    if !found {
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_node.origin.token_id().unwrap().into(),
                            message: format!("Import statement nothing found {}::{} as {}", live_import.module_id, live_import.import_id, in_node.id)
                        });
                    }*/
                    in_index += 1;
                    continue;
                }
                _ => ()
            }
            
            //// determine node overwrite rules
            let out_index = match out_doc.nodes.child_or_append_index_by_name(current_parent.last().unwrap().1, in_node.prop()) {
                Ok(overwrite) => {
                    if current_parent.len() == 1 {
                        lazy_define_value = Some((in_node.id, LiveScopeTarget::LocalPtr(overwrite)));
                    }
                    let out_value = &out_doc.nodes[overwrite].value;
                    let out_origin = out_doc.nodes[overwrite].origin;
                    
                    if in_node.origin.edit_info().is_some() {
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                            message: format!("Cannot define edit info after first prop def of {}", in_node.id)
                        });
                    }
                    
                    if out_value.is_expr(){
                        panic!("No expressions expected in out_value")
                    }
                    // object override
                    if in_value.is_object() && (out_value.is_clone() || out_value.is_class() || out_value.is_object()) { // lets set the target ptr
                        // do nothing
                    }
                    else if in_value.is_expr(){
                        
                        if !out_value.is_single_node(){
                            panic!("overriding is_expr on not is_single_node in file {:?}", self.live_registry.file_id_to_file_name(self.in_file_id));
                        }
                        // lets expand it and output a single LiveValue instead
                        let mut index = in_index;
                        match live_eval_value(&self.live_registry, &mut index, &in_doc.nodes, &out_doc.nodes){
                            Ok(v)=>{
                                out_doc.nodes[overwrite] = in_node.clone();
                                out_doc.nodes[overwrite].value = v;
                            }
                            Err(e)=>{ // output a None 
                                out_doc.nodes[overwrite] = in_node.clone();
                                out_doc.nodes[overwrite].value = LiveValue::None;
                                self.errors.push(e);
                            }
                        }
                        // lets skip the nodes
                        out_doc.nodes[overwrite].origin.inherit_origin(out_origin);
                        in_index = in_doc.nodes.skip_node(in_index);
                        continue;
                    }
                    else if out_value.is_open() && in_value.is_open() { // just replace the whole thing
                        let next_index = out_doc.nodes.skip_node(overwrite);
                        let old_len = out_doc.nodes.len();
                        out_doc.nodes.drain(overwrite + 1..next_index - 1);
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                        out_doc.nodes[overwrite] = in_node.clone();
                    }
                    // replace object with single value
                    else if out_value.is_open() {
                        let next_index = out_doc.nodes.skip_node(overwrite);
                        let old_len = out_doc.nodes.len();
                        out_doc.nodes.drain(overwrite + 1..next_index);
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                        out_doc.nodes[overwrite] = in_node.clone();
                    }
                    // replace single value with object
                    else if in_value.is_open() {
                        let old_len = out_doc.nodes.len();
                        out_doc.nodes[overwrite] = in_node.clone();
                        out_doc.nodes.insert(overwrite + 1, in_node.clone());
                        out_doc.nodes[overwrite + 1].value = LiveValue::Close;
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, overwrite, old_len, out_doc.nodes.len());
                    }
                    else {
                        out_doc.nodes[overwrite] = in_node.clone();
                    };
                    out_doc.nodes[overwrite].origin.inherit_origin(out_origin);
                    overwrite
                }
                Err(insert_point) => {
                    if current_parent.len() == 1 {
                        lazy_define_value = Some((in_node.id, LiveScopeTarget::LocalPtr(insert_point)));
                    }
                    
                    // ok so. if we are inserting an expression, just do the whole thing in one go.
                    if in_node.is_expr() {
                        
                        // lets eval the expression
                        let mut index = in_index;
                        let old_len = out_doc.nodes.len();
                                                
                        match live_eval_value(&self.live_registry, &mut index, &in_doc.nodes, &out_doc.nodes){
                            Ok(v)=>{
                                out_doc.nodes.insert(insert_point, in_node.clone());
                                out_doc.nodes[insert_point].value = v; 
                            }
                            Err(e)=>{
                                out_doc.nodes.insert(insert_point, in_node.clone());
                                out_doc.nodes[insert_point].value = LiveValue::None;
                                self.errors.push(e);
                            } 
                        }
                        self.shift_parent_stack(&mut current_parent, &out_doc.nodes, insert_point - 1, old_len, out_doc.nodes.len());
                        // lets skip the nodes
                        in_index = in_doc.nodes.skip_node(in_index);
                        continue;
                    }
                    
                    let old_len = out_doc.nodes.len();
                    out_doc.nodes.insert(insert_point, in_node.clone());
                    if in_node.is_open() {
                        out_doc.nodes.insert(insert_point + 1, in_node.clone());
                        out_doc.nodes[insert_point + 1].value = LiveValue::Close;
                    }
                    self.shift_parent_stack(&mut current_parent, &out_doc.nodes, insert_point - 1, old_len, out_doc.nodes.len());
                    
                    insert_point
                }
            };
            
            
            // process stacks
            match in_value {
                LiveValue::Dependency(path) => {
                     let file_id = in_node.origin.token_id().unwrap().file_id().unwrap();
                    if let Some(final_path) = self.resolve_path(file_id, &path){
                        out_doc.nodes[out_index].value = LiveValue::Dependency(final_path);
                    }
                },
                LiveValue::Font(font) => {
                    let file_id = in_node.origin.token_id().unwrap().file_id().unwrap();
                    if let Some(final_path) = self.resolve_path(file_id, &font.path){
                        out_doc.nodes[out_index].value = LiveValue::Font(LiveFont{
                            path:final_path,
                            ..*font
                        });
                    }
                },
                LiveValue::Clone{clone,..} | LiveValue::Deref{clone,..}=> {
                    if let Some(target) = self.live_registry.find_scope_target(*clone, &out_doc.nodes) {
                        match target {
                            LiveScopeTarget::LocalPtr(local_ptr) => {
                                
                                let old_len = out_doc.nodes.len();
                                
                                out_doc.nodes.insert_children_from_self(local_ptr, out_index + 1);
                                self.shift_parent_stack(&mut current_parent, &out_doc.nodes, out_index, old_len, out_doc.nodes.len());
                                
                                // we should overwrite our class live_type
                                if let LiveValue::Deref{live_type:new_live_type,..} = in_value{
                                    out_doc.nodes[out_index].value = out_doc.nodes[local_ptr].value.clone();
                                    if let LiveValue::Class {live_type, ..} = &mut out_doc.nodes[out_index].value {
                                        *live_type = *new_live_type
                                    }
                                }
                                else{
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
                                }
                                
                                if let LiveValue::Class {class_parent,..} = &mut out_doc.nodes[out_index].value {
                                    //*class_parent = Some(LivePtr {file_id: self.in_file_id, index: out_index as u32, generation});
                                    *class_parent = LivePtr {file_id: self.in_file_id, index: local_ptr as u32, generation};
                                }
                            }
                            LiveScopeTarget::LivePtr(live_ptr) => {
                                let doc = &self.live_registry.live_files[live_ptr.file_id.to_index()].expanded;
                                
                                let old_len = out_doc.nodes.len();
                                out_doc.nodes.insert_children_from_other(live_ptr.node_index(), out_index + 1, &doc.nodes);
                                self.shift_parent_stack(&mut current_parent, &out_doc.nodes, out_index, old_len, out_doc.nodes.len());
                                
                                out_doc.nodes[out_index].value = doc.nodes[live_ptr.node_index()].value.clone();
                                
                                if let LiveValue::Class {class_parent,live_type, ..} = &mut out_doc.nodes[out_index].value {
                                    if let LiveValue::Deref{live_type:new_live_type,..} = in_value{
                                        *live_type = *new_live_type;
                                    }
                                    else{
                                        *class_parent = live_ptr;
                                    }
                                    //*class_parent = Some(LivePtr {file_id: self.in_file_id, index: out_index as u32, generation});
                                }
                            }
                        };
                        //overwrite value, this copies the Class
                    }
                    else if !Self::is_baseclass(*clone) { //if !self.live_registry.ignore_no_dsl.contains(clone) {
                        self.errors.push(LiveError {
                            origin: live_error_origin!(),
                            span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                            message: format!("Can't find live definition of {} did you forget to call live_design for it, put the definition above the place where you use it or if imported put pub before the definition?", clone)
                        });
                    }
                    current_parent.push((out_doc.nodes[out_index].id, out_index));
                },
                LiveValue::Class {live_type, ..} => {
                    
                    // store the class context
                    if let LiveValue::Class {class_parent, ..} = &mut out_doc.nodes[out_index].value {
                        *class_parent = LivePtr {file_id: self.in_file_id, index: out_index as u32, generation};
                    }
                    
                    let mut insert_point = out_index + 1;
                    let live_type_info = self.live_registry.live_type_infos.get(live_type).unwrap();
                    
                    if let Some(field) = live_type_info.fields.iter().find( | f | f.live_field_kind == LiveFieldKind::Deref) {
                        if !field.live_type_info.live_ignore {
                            let live_type_info = &field.live_type_info;
                            if let Some(file_id) = self.live_registry.module_id_to_file_id.get(&live_type_info.module_id) {
                                let doc = &self.live_registry.live_files[file_id.to_index()].expanded;
                                
                                let mut index = 1;
                                let mut found = None;
                                while index < doc.nodes.len() {
                                    if let LiveValue::Class {live_type, ..} = &doc.nodes[index].value {
                                        if *live_type == live_type_info.live_type {
                                            found = Some(index);
                                            break
                                        }
                                    }
                                    index = doc.nodes.skip_node(index);
                                }
                                if let Some(index) = found {
                                    let old_len = out_doc.nodes.len();
                                    out_doc.nodes.insert_children_from_other(index, out_index + 1, &doc.nodes);
                                    self.shift_parent_stack(&mut current_parent, &out_doc.nodes, out_index, old_len, out_doc.nodes.len());
                                }
                            }
                        }
                    }
                    // else {
                    for field in &live_type_info.fields {
                        if field.live_field_kind == LiveFieldKind::Deref {
                            continue;
                        }
                        let lti = &field.live_type_info;
                        if let Some(file_id) = self.live_registry.module_id_to_file_id.get(&lti.module_id) {
                            
                            if *file_id == self.in_file_id { // clone on self
                                let mut index = 1;
                                let mut found = None;
                                while index < out_doc.nodes.len() - 1 {
                                    if let LiveValue::Class {live_type, ..} = &out_doc.nodes[index].value {
                                        if *live_type == lti.live_type {
                                            found = Some(index);
                                            break
                                        }
                                    }
                                    index = out_doc.nodes.skip_node(index);
                                }
                                if let Some(index) = found {
                                    let node_insert_point = insert_point;
                                    
                                    let old_len = out_doc.nodes.len();
                                    insert_point = out_doc.nodes.insert_node_from_self(index, insert_point);
                                    self.shift_parent_stack(&mut current_parent, &out_doc.nodes, node_insert_point - 1, old_len, out_doc.nodes.len());
                                    
                                    out_doc.nodes[node_insert_point].id = field.id;
                                    out_doc.nodes[node_insert_point].origin.set_prop_type(LivePropType::Field);
                                    
                                }
                                else if !lti.live_ignore {
                                    self.errors.push(LiveError {
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                                        message: format!("Can't find live definition of {} did you forget to call live_design for it, put the definition above the place where you use it or if imported put pub before the definition?", lti.type_name)
                                    });
                                }
                            }
                            else {
                                let other_nodes = &self.live_registry.live_files[file_id.to_index()].expanded.nodes;
                                if other_nodes.is_empty() {
                                    panic!(
                                        "Dependency order bug finding {}, file {} not registered before {}",
                                        lti.type_name,
                                        self.live_registry.file_id_to_file_name(*file_id),
                                        self.live_registry.file_id_to_file_name(self.in_file_id),
                                    );
                                }
                                let mut index = 1;
                                let mut found = None;
                                while index < other_nodes.len() - 1 {
                                    if let LiveValue::Class {live_type, ..} = &other_nodes[index].value {
                                        if *live_type == lti.live_type {
                                            found = Some(index);
                                            break
                                        }
                                    }
                                    index = other_nodes.skip_node(index);
                                }
                                if let Some(index) = found {
                                    let node_insert_point = insert_point;
                                    
                                    let old_len = out_doc.nodes.len();
                                    insert_point = out_doc.nodes.insert_node_from_other(index, insert_point, other_nodes);
                                    self.shift_parent_stack(&mut current_parent, &out_doc.nodes, node_insert_point - 1, old_len, out_doc.nodes.len());
                                    
                                    out_doc.nodes[node_insert_point].id = field.id;
                                    out_doc.nodes[node_insert_point].origin.set_prop_type(LivePropType::Field);
                                }
                                else if !lti.live_ignore && lti.type_name != LiveId(0) {
                                    self.errors.push(LiveError {
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                                        message: format!("Typename {}, not defined in file where it was expected", lti.type_name)
                                    });
                                }
                            }
                        }
                        else if !lti.live_ignore {
                            self.errors.push(LiveError {
                                origin: live_error_origin!(),
                                span: in_doc.token_id_to_span(in_node.origin.token_id().unwrap()).into(),
                                message: format!("Can't find live definition of {} did you forget to call live_design for it, put the definition above the place where you use it or if imported put pub before the definition?", lti.type_name)
                            });
                        }
                    }
                    //}
                    
                    current_parent.push((out_doc.nodes[out_index].id, out_index));
                }
                LiveValue::Expr {..} => {panic!()},
                LiveValue::Array |
                LiveValue::TupleEnum {..} |
                LiveValue::NamedEnum {..} |
                LiveValue::Object => { // lets check what we are overwriting
                    current_parent.push((out_doc.nodes[out_index].id, out_index));
                },
                LiveValue::DSL {..} => {
                    //println!("{}",std::mem::size_of::<TextSpan>());
                },
                _ => {}
            }
            
            in_index += 1;
        }
        out_doc.nodes.push(in_doc.nodes.last().unwrap().clone());
        // this stores the node index on nodes that don't have a node index
        let mut level = 0;
        
        for i in 1..out_doc.nodes.len() {
            // check if we have a prefix and its export
            // store this pointer
            //let exports: HashMap<LiveId, usize> = Default::default();
            let node = &mut out_doc.nodes[i];
            if node.value.is_dsl() {
                node.value.set_dsl_expand_index_if_none(i);
            }
            if level == 0 && in_doc.exports.get(&node.id).is_some(){
                let id = node.id;
                if let LiveValue::Root(root) = &mut out_doc.nodes[0].value{
                    root.exports.insert(id, i);
                }
            }
            let node = &mut out_doc.nodes[i];
            if node.value.is_open(){
                level += 1
            }
            else if node.value.is_close() && level > 0{
                level -= 1
            }

        }
    }
    
}

