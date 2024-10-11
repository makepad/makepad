use crate::{
    makepad_draw::*,
    widget::*,
    makepad_platform::studio::DesignerComponentPosition,
    makepad_live_compiler::LiveTokenId,
};
use std::collections::HashMap;
use std::fmt::Write;

pub enum OutlineNode{
    Virtual{
        name: String,
        children: SmallVec<[LiveId;4]>
    },
    File{
        file_id: LiveFileId,
        name: String,
        children: SmallVec<[LiveId;4]>
    },
    Folder{
        name: String,        
        children: SmallVec<[LiveId;4]>
    },
    Component{
        name: String,        
        id: LiveId,
        class: LiveId,
        prop_type: LivePropType,
        token_id: LiveTokenId,
        ptr: LivePtr,
        children: SmallVec<[LiveId;4]>
    }
}

impl OutlineNode{
    fn children(&self)->&[LiveId]{
        match self{
            Self::Virtual{children,..}=>children,
            Self::File{children,..}=>children,
            Self::Folder{children,..}=>children,
            Self::Component{children,..}=>children,
        }
    }
    
    fn name(&self)->&str{
        match self{
            Self::Virtual{name,..}=>name,
            Self::File{name,..}=>name,
            Self::Folder{name,..}=>name,
            Self::Component{name,..}=>name,
        }
    }
}

#[derive(Default)]
pub struct DesignerData{
    pub root: LiveId,
    pub node_map: HashMap<LiveId, OutlineNode>,
    pub selected: Option<LiveId>,
    pub positions: Vec<DesignerComponentPosition>
}

impl DesignerData{
    pub fn get_node_by_path(&self, root:LiveId, path:&str)->Option<LiveId>{
        let mut current = root;
        let mut split = path.split("/");
        'outer: while let Some(node) = self.node_map.get(&current){
            if let Some(next_name) = split.next(){
                for child in node.children(){
                    let node = self.node_map.get(&child).unwrap();
                    if node.name().starts_with(next_name){
                        current = *child;
                        continue 'outer;
                    }
                }
                return None
            }
            else{
                return Some(current)
            }
        }
        None
    }
    
    pub fn update_from_live_registry(&mut self, cx:&mut Cx){
        self.node_map.clear();
        
        let root_uid = live_id!(designer_root).into();
        self.root = root_uid;
        self.node_map.insert(root_uid, OutlineNode::Virtual{
            name: "root".into(),
            children: SmallVec::new()
        });
                
        // lets take the doc we need (app_mobile for instance)
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = &*live_registry_rc.borrow();
        let main_module_lti = live_registry.main_module.as_ref().unwrap().clone();
        let mut path_hash = String::new();
        for (file_path, file_id) in live_registry.file_ids(){
            // now we fetch the unexpanded nodes
            // and build a list
            let file = live_registry.file_id_to_file(*file_id);
            let nodes = &file.expanded.nodes;
            // lets run over the file
                        
            fn recur_walk_components(
                main_module_lti:&LiveTypeInfo, 
                live_registry: &LiveRegistry, 
                hash_id:LiveId, 
                base_ptr: LivePtr, 
                mut index: usize, 
                nodes: &[LiveNode],
                map: &mut HashMap<LiveId, OutlineNode>,
                parent_children: &mut SmallVec<[LiveId;4]>) -> usize {
                                        
                while index < nodes.len() - 1 { 
                    if let LiveValue::Class {live_type, class_parent, ..} = &nodes[index].value {
                        // lets check if its a widget
                        let wr = live_registry.components.get::<WidgetRegistry>();
                        if main_module_lti.live_type == *live_type || wr.map.get(live_type).is_some(){
                                                            
                            // lets emit a class at our level
                            let id = nodes[index].id;
                            let class = live_registry.ptr_to_node(*class_parent).id;
                            let ptr = base_ptr.with_index(index);
                            let prop_type =  nodes[index].origin.prop_type();
                            let token_id = nodes[index].origin.token_id();
                            let uid = hash_id.bytes_append(&id.0.to_be_bytes());
                            let mut children = SmallVec::new();
                                                            
                            index = recur_walk_components(
                                main_module_lti, 
                                live_registry, 
                                uid, 
                                base_ptr, 
                                index + 1, 
                                nodes, 
                                map, 
                                &mut children
                            );
                                                            
                            let uid = uid.into();
                            parent_children.push(uid);
                            
                            let mut name = String::new();
                            if !id.is_unique(){
                                if let LivePropType::Field = prop_type {
                                    write!(name, "{}: <{}>", id, class).unwrap();
                                }
                                else {
                                    write!(name, "{}=<{}>", id, class).unwrap();
                                }
                            }
                            else {
                                write!(name, "<{}>", class).unwrap();
                            }
                            
                            map.insert(uid, OutlineNode::Component {
                                id,
                                name,
                                token_id: token_id.unwrap(), 
                                prop_type,
                                class,
                                ptr,
                                children
                            });
                            // find all the components that start with app_ and make sure the folder is visible
                            
                        }
                        else{
                            //   log!("NOT A WIDGET {}", nodes[index].id);
                            index = nodes.skip_node(index);
                        }
                    }
                    else if nodes[index].value.is_close() {
                        return index + 1;
                    }
                    else {
                        index = nodes.skip_node(index);
                    }
                }
                index
            }
            // alright lets iterate over the files
                            
            let base_ptr = live_registry.file_id_index_to_live_ptr(*file_id, 0);
                            
            path_hash.clear();
                            

            let base_id = LiveId(0).bytes_append(&file_id.0.to_be_bytes());
                            
            let mut children_out = SmallVec::new();
            recur_walk_components(
                &main_module_lti, 
                live_registry, 
                base_id, 
                base_ptr, 
                1, 
                nodes,
                &mut self.node_map, 
                &mut children_out
            );
            if children_out.len() == 0{
                continue
            }
            let parent_id = path_split(self.root, &mut path_hash, file_path, &mut self.node_map, *file_id);
            if let Some(OutlineNode::File{children,..}) = self.node_map.get_mut(&parent_id){
                *children = children_out;
            }
                            
            fn path_split<'a>(parent_id: LiveId, path_hash:&mut String, name:&str, map:&mut HashMap<LiveId, OutlineNode>, file_id:LiveFileId)->LiveId{
                if let Some((folder,rest)) = name.split_once("/"){
                                            
                    if folder == "src"{ // flatten this
                        return path_split(parent_id, path_hash, rest, map, file_id)
                    }
                    
                    path_hash.push_str(folder);
                    path_hash.push_str("/");
                                                                                    
                    let what_uid =  LiveId::from_str(&path_hash).into();
                                            
                    // add node to pareht
                    if let Some(OutlineNode::Folder{children, ..}) | Some(OutlineNode::Virtual{children, ..})= map.get_mut(&parent_id){
                        if !children.contains(&what_uid){
                            children.push(what_uid);
                        }
                    }
                                            
                    if map.get_mut(&what_uid).is_some(){
                        return path_split(what_uid, path_hash, rest, map, file_id)
                    }
                    
                    map.insert(what_uid, OutlineNode::Folder{
                        name: folder.to_string(),
                        children: SmallVec::new()
                    });
                                            
                    return path_split(what_uid, path_hash, rest, map, file_id)
                }
                else{ // we're a file
                    path_hash.push_str(name);
                    path_hash.push_str("/");
                    let what_uid =  LiveId::from_str(&path_hash).into();
                    if let Some(OutlineNode::Folder{children, ..}) | Some(OutlineNode::Virtual{children, ..})= map.get_mut(&parent_id){
                        if !children.contains(&what_uid){
                            children.push(what_uid);
                        }
                    }
                    map.insert(what_uid, OutlineNode::File{
                        file_id,
                        name: name.to_string(),
                        children: SmallVec::new()
                    });
                                            
                    return what_uid
                }
            }
        }
    }
    
    pub fn find_component_by_ptr(&mut self, find_ptr:LivePtr)->Option<LiveId>{
        for (node_id, node) in &self.node_map{
            if let OutlineNode::Component{ptr,..} = node{
                if *ptr == find_ptr{
                    return Some(*node_id)
                }
            }
        }
        None
    }
    
    pub fn construct_path_ids(&self, find_node:LiveId)->Vec<LiveId>{
        let mut result = Vec::new();
        result.push(find_node);
        let mut iter = find_node;
        while let Some(parent) = self.find_parent(iter){
            result.insert(0, parent);
            iter = parent;
        }
        result
    }
    
    pub fn path_ids_to_string(&self, path:&[LiveId])->String{
        let mut path_str= String::new();
        for node_id in path{
            if let Some(node) = self.node_map.get(&node_id){
                match node{
                    OutlineNode::Folder{name,..} | OutlineNode::File{name,..}=>{
                        path_str.push_str(name);
                        path_str.push_str("/");
                    }
                    _=>()
                }
            }
        }
        path_str
    }
    
    pub fn path_str_to_path_ids(path:&str)->Vec<LiveId>{
        let mut path_id = Vec::new();
        for (idx,_) in path.match_indices('/'){
            let slice = &path[0..idx+1];
            let hash =  LiveId::from_str(slice).into();
            path_id.push(hash);
        }
        path_id
    }

    pub fn find_parent(&self, find_node:LiveId)->Option<LiveId>{
        for (node_id, node) in &self.node_map{
            match node{
                OutlineNode::Component{children,..} | OutlineNode::Virtual{children,..} | OutlineNode::File{children,..} | OutlineNode::Folder{children, ..} =>{
                    if children.iter().position(|v| *v == find_node).is_some(){
                        return Some(*node_id)
                    }
                }
            }
        }
        None
    }
    
    pub fn find_file_parent(&mut self, find_node:LiveId)->Option<LiveId>{
        let mut iter = find_node;
        while let Some(parent) = self.find_parent(iter){
            if let Some(OutlineNode::File{..}) = self.node_map.get(&parent){
                return Some(parent);
            }
            iter = parent;
        }
        None
    }
    
    pub fn _remove_child(&mut self, find_node:LiveId){
        for node in &mut self.node_map.values_mut(){
            match node{
                OutlineNode::Component{children,..} | OutlineNode::Virtual{children,..} | OutlineNode::File{children,..} | OutlineNode::Folder{children, ..} =>{
                    if let Some(i) = children.iter().position(|v| *v == find_node){
                        children.remove(i);
                        break;
                    }
                }
            }
        }
    }
    
    pub fn _find_component_by_path(&self, path:&[LiveId])->Option<LiveId>{
        fn get_node(node:LiveId, path:&[LiveId],  map:&HashMap<LiveId, OutlineNode>)->Option<LiveId>{
            match map.get(&node).as_ref(){
                Some(OutlineNode::Virtual{children,..}) |
                Some(OutlineNode::Folder{children,..}) |
                Some(OutlineNode::File{children,..}) =>{
                    for child in children{
                        if let Some(v) = get_node(*child, path, map){
                            return Some(v)
                        }
                    }
                }
                Some(OutlineNode::Component{children, id, ..}) => {
                    if *id == path[0]{
                        if path.len()>1{
                            for child in children{
                                 if let Some(v) = get_node(*child, &path[1..], map){
                                     return Some(v)
                                 }
                            }
                        }
                        else{
                            return Some(node)
                        }
                    }
                    for child in children{
                        if let Some(v) = get_node(*child, path, map){
                            return Some(v)
                        }
                    }
                }
                _=>()
            }
            None
        }
        get_node(self.root, path, &self.node_map)
    }
}
